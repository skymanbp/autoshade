"""Mask-free boundary-overshoot metric (supervisor, 2026-08-24).

Why this exists: the earlier rim statistic was measured relative to a REFERENCE
MASK, so renders that used different masks were not comparable -- each scored
best against its own mask, which is how "mask hardening beats the no-zone
floor" got into a task book and then failed to reproduce.

This metric uses no mask for the reading itself. A local correction applied
through any mask must, across a boundary, interpolate between two plateau
levels: the level it settles to on one side and the level on the other. Any
excursion OUTSIDE that interval is a halo/rim -- the thing the eye sees.

    d(y)      = luma(zoned) - luma(its own no-zones twin)
    P_s, P_l  = median of d in a window well outside the transition, each side
    overshoot = max over the +/-half band of the distance of d outside [P_s,P_l]

A monotone transition of any steepness scores 0. Control (a render against an
identical render) reads exactly 0.0000.

A rough boundary locator is still needed to place the windows; a few px of
error there is harmless because the windows carry 60 px of margin.

Usage:
    python scripts/rim_overshoot.py <zoned.jpg> <its-no-zones.jpg> <locator-mask.png>
"""

import sys
from pathlib import Path

import numpy as np
from PIL import Image

HALF = 60           # px each side of the boundary that counts as "the transition"
PLATEAU_GAP = 60    # px of margin before a plateau window starts
PLATEAU_WIDTH = 60  # px of plateau window


def luma(path: Path) -> np.ndarray:
    rgb = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0
    return rgb @ np.array([0.299, 0.587, 0.114], dtype=np.float32)


def resized_mask(path: Path, size: tuple[int, int]) -> np.ndarray:
    return np.asarray(
        Image.open(path).convert("L").resize(size, Image.Resampling.BILINEAR),
        dtype=np.float32,
    ) / 255.0


def locator(path, size):
    """Per-column y of the first 0.5 crossing -- only used to place windows."""
    mask = resized_mask(Path(path), size)
    ys = np.full(mask.shape[1], np.nan)
    for x in range(mask.shape[1]):
        column = mask[:, x]
        crossings = np.flatnonzero((column[:-1] - 0.5) * (column[1:] - 0.5) <= 0.0)
        crossings = [y for y in crossings if column[y] != column[y + 1]]
        if crossings:
            ys[x] = crossings[0]
    return ys


def overshoot(zoned, no_zones, locator_mask):
    values = luma(Path(zoned)) - luma(Path(no_zones))
    height, width = values.shape
    ys = locator(locator_mask, (width, height))
    readings = []
    for x in range(width):
        if np.isnan(ys[x]):
            continue
        centre = int(ys[x])
        sky0 = centre - HALF - PLATEAU_GAP - PLATEAU_WIDTH
        sky1 = centre - HALF - PLATEAU_GAP
        land0 = centre + HALF + PLATEAU_GAP
        land1 = centre + HALF + PLATEAU_GAP + PLATEAU_WIDTH
        if sky0 < 0 or land1 > height:
            continue
        column = values[:, x]
        sky = float(np.median(column[sky0:sky1]))
        land = float(np.median(column[land0:land1]))
        low, high = min(sky, land), max(sky, land)
        band = column[max(0, centre - HALF) : min(height, centre + HALF + 1)]
        band = np.convolve(band, np.ones(3) / 3.0, mode="same")  # kill jpeg noise
        readings.append(float(np.maximum(0.0, np.maximum(band - high, low - band)).max()))
    return np.asarray(readings, dtype=np.float32)


def main():
    if len(sys.argv) != 4:
        print(__doc__)
        raise SystemExit(2)
    readings = overshoot(sys.argv[1], sys.argv[2], sys.argv[3])
    if not len(readings):
        print("n=0")
        return
    print(
        f"n={len(readings)} mean={readings.mean():.4f} "
        f"p90={np.quantile(readings, 0.9):.4f} max={readings.max():.4f}"
    )


if __name__ == "__main__":
    main()
