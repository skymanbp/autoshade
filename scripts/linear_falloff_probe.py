"""Measure the falloff shape of a rendered vertical linear-gradient probe.

The probe is intentionally flat, so the row-average luma exposes the mask
coverage profile. 16-bit input is preserved as 16-bit values before
normalisation, making the first-difference corner measurable.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np


def load_luma(path: Path) -> np.ndarray:
    """Return row luma normalised to [0, 1], retaining the file bit depth."""
    try:
        import tifffile

        array = tifffile.imread(str(path))
    except Exception:
        from PIL import Image

        array = np.asarray(Image.open(path).convert("RGB"))
    if np.issubdtype(array.dtype, np.integer):
        peak = float(np.iinfo(array.dtype).max)
    else:
        peak = float(array.max()) if float(array.max()) > 1.0 else 1.0
    values = array.astype(np.float64) / peak
    if values.ndim == 2:
        return values
    return 0.299 * values[..., 0] + 0.587 * values[..., 1] + 0.114 * values[..., 2]


def profile(path: Path, zero_y: float, full_y: float) -> dict[str, object]:
    luma = load_luma(path)
    height, width = luma.shape
    rows = luma.mean(axis=1)
    slope = np.diff(rows)
    zero_row = int(zero_y * height)
    full_row = int(full_y * height)
    low, high = sorted((full_row, zero_row))

    def mean_slope(start: int, stop: int) -> float:
        segment = slope[max(start, 0) : min(stop, len(slope))]
        return float(segment.mean()) if len(segment) else float("nan")

    beyond_full = mean_slope(20, low - 15)
    through = mean_slope(low + 15, high - 15)
    beyond_zero = mean_slope(high + 15, height - 20)
    threshold = 0.25 * abs(through)

    def turnover(end: int, direction: int) -> int:
        rows_seen = 0
        for offset in range(80):
            row = end + direction * offset
            if not 0 <= row < len(slope):
                break
            if abs(float(slope[row]) - through) < threshold:
                break
            rows_seen += 1
        return rows_seen

    return {
        "name": path.name,
        "width": width,
        "height": height,
        "full_row": full_row,
        "zero_row": zero_row,
        "slope_full": beyond_full,
        "slope_ramp": through,
        "slope_zero": beyond_zero,
        "jump_full": abs(through - beyond_full),
        "jump_zero": abs(through - beyond_zero),
        "turn_full": turnover(low, +1),
        "turn_zero": turnover(high, -1),
    }


PROFILES = {
    "linear": lambda t: t,
    "smoothstep": lambda t: 3.0 * t**2 - 2.0 * t**3,
    "sin": lambda t: 0.5 - 0.5 * np.cos(np.pi * t),
}


def fit_profile(path: Path, zero_y: float, full_y: float, search: int = 150) -> dict[str, object]:
    """Fit each profile with BOTH handle rows free (integer grid, coarse then fine).

    The residual runs over the ramp plus both plateaus, so a soft profile can
    no longer look linear by shrinking its own span, and a manually placed
    Lightroom gradient is compared at the handles it actually has.
    """
    luma = load_luma(path)
    height = luma.shape[0]
    rows = luma.mean(axis=1)
    full_row = int(full_y * height)
    zero_row = int(zero_y * height)
    top = rows[20 : full_row - search].mean()
    bot = rows[zero_row + search : height - 20].mean()
    coverage = (rows - bot) / (top - bot)
    window = np.arange(full_row - search, zero_row + search + 1)
    target = coverage[window]

    def rms(model, full: int, zero: int) -> float:
        t = np.clip((zero - window) / (zero - full), 0.0, 1.0)
        residual = target - model(t)
        return float(np.sqrt((residual * residual).mean()))

    fits: dict[str, object] = {"name": path.name}
    for label, model in PROFILES.items():
        best = (float("inf"), full_row, zero_row)
        for step, (f0, z0, span) in ((2, (full_row, zero_row, search)), (1, (None, None, 3))):
            f0 = best[1] if f0 is None else f0
            z0 = best[2] if z0 is None else z0
            for full in range(f0 - span, f0 + span + 1, step):
                for zero in range(z0 - span, z0 + span + 1, step):
                    value = rms(model, full, zero)
                    if value < best[0]:
                        best = (value, full, zero)
        fits[label] = best
    return fits


def print_fit(fits: dict[str, object]) -> None:
    print(f"{fits['name']}: free-end profile fit (rms over ramp + plateaus; handle rows recovered)")
    for label in PROFILES:
        value, full, zero = fits[label]
        print(f"  {label:<11} rms={value:.4f}  full={full}  zero={zero}  span={zero - full}")


def print_profile(result: dict[str, object]) -> None:
    print(f"{result['name']}: {result['width']}x{result['height']}, ends at y={result['full_row']} (full) and y={result['zero_row']} (zero)")
    print(f"  slope beyond the full end   {result['slope_full']:+.4e}")
    print(f"  slope through the ramp      {result['slope_ramp']:+.4e}")
    print(f"  slope beyond the zero end   {result['slope_zero']:+.4e}")
    print(f"  jump at the full end        {result['jump_full']:.4e}")
    print(f"  jump at the zero end        {result['jump_zero']:.4e}")
    for name, key in (("full", "turn_full"), ("zero", "turn_zero")):
        rows = int(result[key])
        verdict = "hard corner (clamped ramp)" if rows <= 2 else f"eased over ~{rows} rows"
        print(f"  {name} end turnover: {rows} row(s) -> {verdict}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", type=Path, help="one rendered TIFF, two paths with --compare, or any number with --fit")
    parser.add_argument("--compare", action="store_true", help="print two profiles side by side")
    parser.add_argument("--fit", action="store_true", help="free-end profile fit (linear / smoothstep / sin) for every path")
    parser.add_argument("--zero-y", type=float, default=0.80, help="zero handle as a frame fraction (default: 0.80)")
    parser.add_argument("--full-y", type=float, default=0.35, help="full handle as a frame fraction (default: 0.35)")
    args = parser.parse_args()
    if args.fit:
        if not args.paths:
            parser.error("--fit needs at least one TIFF path")
        for path in args.paths:
            print_fit(fit_profile(path, args.zero_y, args.full_y))
        return
    expected = 2 if args.compare else 1
    if len(args.paths) != expected:
        parser.error(f"expected {expected} TIFF path(s), got {len(args.paths)}")
    results = [profile(path, args.zero_y, args.full_y) for path in args.paths]
    if args.compare:
        print("profile                         jump-full       jump-zero       turn-full  turn-zero")
        for result in results:
            print(f"{result['name']:<30} {result['jump_full']:.6e}  {result['jump_zero']:.6e}  {result['turn_full']:>9}  {result['turn_zero']:>9}")
        print()
    for result in results:
        print_profile(result)


if __name__ == "__main__":
    main()
