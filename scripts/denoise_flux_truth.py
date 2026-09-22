#!/usr/bin/env python3
"""Does the RAW cleaner keep the light of a star? A photometric truth test.

A synthetic mosaic whose every flux is KNOWN: a night sky at the levels of the
operator's star frame, brightening down the frame, with isolated stars on a
grid in seven brightness classes, Poisson-Gaussian noise from one physical
gain, quantised to the sensor's integers. It goes through the sidecar's own
code — `denoise_raw.noise_model`, then `denoise_raw.denoise_planes` at strength
1 — and three things are read against the truth, never against an estimate:

  * per class and CFA plane, the flux the cleaner returns over the flux that
    was there, summed over a 5x5 plane window at every star of the class;
  * the same for the NOISY INPUT: the control. Photon noise is unbiased, so it
    has to read 1 within its scatter, or the instrument is wrong;
  * the mean shift of star-free sky, per plane, in DN.

A class is the star's G-plane peak in units of the noise sigma of the sky it
stands on. `autoshade-raw-denoise-v1` reads 0.06-0.07 / 0.06-0.07 / 0.10-0.11 /
0.38-0.42 / 0.73 / 0.91-0.92 / 0.97-0.98 in G at 1.5 / 2.5 / 4 / 6 / 10 / 20 /
40 sigma (2026-09-21): it removes what a star and a sensor defect have in
common on one plane. That measurement is why `train_raw.py` has `--stars`.

The sky, colour and noise constants below are numbers measured on that frame;
no photograph is read. `--weights` measures a candidate state dict in place of
the shipped one (architecture and every other file as shipped).

    python scripts/denoise_flux_truth.py --out OUT [--weights RUN/best_state.pth] [--plate OUT/plate.png]
"""
import argparse
import json
import pathlib
import sys

import numpy as np

REPO = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "python"))
import denoise_raw as dr  # noqa: E402  because it is importable only after the sys.path insert above

BLACK, WHITE = 512.0, 15360.0
GAIN, READ = 4.9e-4, 6.2e-6      # var = GAIN * x + READ in normalised units: the G fit on the star frame
SKY = {"R": 0.00626, "G1": 0.01536, "G2": 0.01536, "B": 0.0064}   # its sky above black, normalised
COLOUR = {"R": 0.35, "G1": 1.0, "G2": 1.0, "B": 0.61}             # plane flux shares of its bright stars
PHASES = {"R": (0, 0), "G1": (0, 1), "G2": (1, 0), "B": (1, 1)}
PEAK_SIGMAS = (1.5, 2.5, 4.0, 6.0, 10.0, 20.0, 40.0)
FWHM = 2.6                       # mosaic pixels
PITCH = 64


def scene(size, rng):
    """(clean, sky, stars): normalised mosaics and one (y, x, class) per star."""
    sigma = FWHM / 2.3548
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    gradient = (0.7 + 0.8 * yy / size).astype(np.float32)          # 0.7x at the top to 1.5x at the bottom
    clean = np.zeros((size, size), np.float32)
    for n, (py, px) in PHASES.items():
        clean[py::2, px::2] = SKY[n] * gradient[py::2, px::2]
    sky = clean.copy()
    one_sigma = float(np.sqrt(GAIN * SKY["G1"] + READ))
    stars = []
    centres = [(y, x) for y in range(PITCH, size - PITCH, PITCH) for x in range(PITCH, size - PITCH, PITCH)]
    for i, (cy, cx) in enumerate(centres):
        klass = PEAK_SIGMAS[i % len(PEAK_SIGMAS)]
        oy, ox = rng.uniform(-1, 1, 2)
        y0, x0 = cy + oy, cx + ox
        ys, xs = slice(cy - 12, cy + 13), slice(cx - 12, cx + 13)
        g = np.exp(-((yy[ys, xs] - y0) ** 2 + (xx[ys, xs] - x0) ** 2) / (2 * sigma * sigma)).astype(np.float32)
        for n, (py, px) in PHASES.items():
            sel = np.zeros_like(g, bool)
            sel[(py - (cy - 12)) % 2::2, (px - (cx - 12)) % 2::2] = True
            clean[ys, xs][sel] += COLOUR[n] * klass * one_sigma * g[sel]
        stars.append((float(y0), float(x0), klass))
    return clean, sky, np.array(stars)


def expose(clean, rng):
    """The sensor's integers for a clean normalised mosaic."""
    electrons = rng.poisson(np.clip(clean, 0, None) / GAIN).astype(np.float32)
    noisy = GAIN * electrons + rng.normal(0, np.sqrt(READ), clean.shape).astype(np.float32)
    return np.clip(np.round(noisy * (WHITE - BLACK) + BLACK), 0, 65535).astype(np.uint16)


def clean_with(model, mosaic, device, fp16):
    """The sidecar's steps 1-6 at strength 1, on its own functions."""
    planes = {n: np.minimum((p.astype(np.float32) - BLACK) / (WHITE - BLACK), 1.0)
              for n, p in dr.split_planes(mosaic, PHASES).items()}
    ab = dr.noise_model(planes)
    den = dr.denoise_planes(model, planes, ab, device, 512, 32, fp16)
    packed = {n: dr.blend_and_quantise(planes[n], den[n], 1.0, BLACK, WHITE) for n in dr.PLANES}
    return dr.merge_planes(packed, PHASES, mosaic.shape, mosaic), {n: [float(v) for v in ab[n]] for n in dr.PLANES}


def measure(images, clean, sky, stars):
    norm = {k: (v.astype(np.float64) - BLACK) / (WHITE - BLACK) for k, v in images.items()}
    report = {"classes": {}, "sky_dn": {}}
    far = np.ones(clean.shape, bool)
    for y, x, _ in stars:
        far[int(y) - 20:int(y) + 21, int(x) - 20:int(x) + 21] = False
    for n, (py, px) in PHASES.items():
        sel = far[py::2, px::2]
        report["sky_dn"][n] = {k: float(((v - sky)[py::2, px::2][sel]).mean() * (WHITE - BLACK)) for k, v in norm.items()}
    truth = {n: (clean - sky)[py::2, px::2] for n, (py, px) in PHASES.items()}
    excess = {k: {n: (v - sky)[py::2, px::2] for n, (py, px) in PHASES.items()} for k, v in norm.items()}
    for klass in PEAK_SIGMAS:
        rows = {}
        for n, (py, px) in PHASES.items():
            there, got = 0.0, {k: 0.0 for k in norm}
            for y, x, _ in stars[stars[:, 2] == klass]:
                cy, cx = (int(round(y)) - py) // 2, (int(round(x)) - px) // 2
                window = (slice(cy - 2, cy + 3), slice(cx - 2, cx + 3))
                there += truth[n][window].sum()
                for k in norm:
                    got[k] += excess[k][n][window].sum()
            rows[n] = {k: got[k] / there for k in norm}
        report["classes"][f"{klass:g}"] = rows
    return report


def plate(path, images, clean, stars):
    """One star per class, G1 plane, truth / noisy / cleaned, one stretch."""
    import cv2
    py, px = PHASES["G1"]
    one_sigma = float(np.sqrt(GAIN * SKY["G1"] + READ))
    rows = [clean] + [(v.astype(np.float32) - BLACK) / (WHITE - BLACK) for v in images.values()]
    middle = len(stars) // 2
    picks = [next(s for s in stars[middle:] if s[2] == k) for k in PEAK_SIGMAS]
    tiles = []
    for img in rows:
        plane, line = img[py::2, px::2], []
        for y, x, _ in picks:
            cy, cx = (int(round(y)) - py) // 2, (int(round(x)) - px) // 2
            t = plane[cy - 16:cy + 17, cx - 16:cx + 17]
            ground = float(np.median(rows[0][py::2, px::2][cy - 16:cy + 17, cx - 16:cx + 17]))
            line.append(np.clip((t - (ground - 3 * one_sigma)) / (15 * one_sigma), 0, 1))
        tiles.append(np.hstack([np.pad(t, 1, constant_values=1) for t in line]))
    sheet = cv2.resize((np.vstack(tiles) * 255).astype(np.uint8), None, fx=8, fy=8, interpolation=cv2.INTER_NEAREST)
    h, w = sheet.shape
    canvas = np.full((h + 40, w + 230), 255, np.uint8)
    canvas[40:, 230:] = sheet
    for i, k in enumerate(PEAK_SIGMAS):
        cv2.putText(canvas, f"G peak {k:g} sigma", (230 + i * (w // len(PEAK_SIGMAS)) + 40, 27),
                    cv2.FONT_HERSHEY_SIMPLEX, 0.6, 0, 1, cv2.LINE_AA)
    for j, name in enumerate(["truth"] + list(images)):
        cv2.putText(canvas, name, (8, 40 + j * (h // len(rows)) + h // (2 * len(rows))), cv2.FONT_HERSHEY_SIMPLEX, 0.6, 0, 1,
                    cv2.LINE_AA)
    cv2.imwrite(str(path), canvas)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True, help="directory for flux-truth.json")
    ap.add_argument("--weights", help="a candidate state dict to measure in place of the shipped weights")
    ap.add_argument("--cache", default=str(REPO / "python" / "weights"))
    ap.add_argument("--size", type=int, default=3072, help="mosaic side, a multiple of 64")
    ap.add_argument("--seed", type=int, default=20260921)
    ap.add_argument("--plate")
    ap.add_argument("--fp16", action="store_true")
    ap.add_argument("--cpu", action="store_true")
    args = ap.parse_args()
    import torch

    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(args.seed)
    clean, sky, stars = scene(args.size, rng)
    noisy = expose(clean, rng)
    device = dr.pick_device(args.cpu, "cuda:0")
    model = dr.load_model(args.cache, device)
    if args.weights:
        model.load_state_dict(torch.load(args.weights, map_location="cpu", weights_only=True), strict=True)
        model = model.eval().to(device)
    cleaned, ab = clean_with(model, noisy, device, args.fp16)
    images = {"noisy input": noisy, "cleaner": cleaned}
    report = measure(images, clean, sky, stars)
    report.update({"weights": pathlib.Path(args.weights).name if args.weights else "shipped", "device": str(device),
                   "size": args.size, "seed": args.seed, "stars": len(stars), "per_class": len(stars) // len(PEAK_SIGMAS),
                   "noise_model_seen": ab, "noise_model_true": [GAIN, READ]})
    (out / "flux-truth.json").write_text(json.dumps(report, indent=1), encoding="utf-8")
    print(f"{len(stars)} stars, {report['per_class']} per class; flux returned / flux there (control in brackets)")
    for klass, rows in report["classes"].items():
        print(f"  G peak {klass:>4} sigma: " + "  ".join(
            f"{n} {r['cleaner']:.3f} ({r['noisy input']:.3f})" for n, r in rows.items()))
    print("  star-free sky, mean(out - truth) in DN: " + "  ".join(
        f"{n} {r['cleaner']:+.2f} ({r['noisy input']:+.2f})" for n, r in report["sky_dn"].items()))
    if args.plate:
        plate(pathlib.Path(args.plate), images, clean, stars)


if __name__ == "__main__":
    main()
