#!/usr/bin/env python3
"""How much of a COMPOSITE star's core does the RAW cleaner keep? Truth-injected, on a real frame.

The cleaner's third fine-tune (v3) exists because of this measurement. The star standard's line 6
read the v2 weights at 0.871 of the input's bright-star peaks against Lightroom's 0.941, and every
injected star tried first — Gaussian, Moffat, coma-tailed, red, blue, transplanted real stamps —
kept 0.96-1.02, so the loss was intrinsic to the real stars' pixels. Ranking the frame's 202
bright stars by v2's peak ratio, the strongest correlate (Spearman -0.53) was core sharpness: the
peak sample over its 3x3 mean. The worst stars are a bright one-sample core riding on a 10 px
trail; the best are round blobs. Every training star had been ONE Gaussian, so a spike on a smooth
bright blob had only ever been noise in what the network was shown. Injected here with truth
(2026-09-22): a trail alone kept 1.00, a core alone 0.95-1.01 from 15 sigma up, a core ON a trail
or halo 0.84-0.87 at every brightness under v2 and 0.92-0.96 under v1.

Five shapes are injected into empty sky of the operator's frame (the mosaic the cleaner received,
`--mosaic`, with the standard's own sites `--sites` avoided), with the core placed ON a G1
photosite — the worst case that plane can see — at G1 core peaks of 8, 15 and 30 local sigma:

    core          Gaussian FWHM 1.8 px alone
    streak        Gaussian 4.0 x 10.0 px at 119 deg (the frame's own trail angle) alone
    core+streak   40 % core + 60 % streak, one centre
    core+end      the same, the core 3 px from the streak's centre along its axis
    core+halo     40 % core + 60 % round halo FWHM 7 px

and the cleaner is run through its own `main()` (the shipped pipeline, strength 1) with the
candidate weights swapped in. The G1 peak excess at the core sample is read for truth, noisy
input and output; `peak_vs_truth` is the number the acceptance line reads.

    python scripts/denoise_composite_truth.py --weights CAND.pth --mosaic F1-input-mosaic.png \\
        --sites <standard work dir with fixed-sites.npz> --json out.json [--work DIR]
"""
import argparse
import importlib.util
import json
import math
import pathlib
import sys

import cv2
import numpy as np
import torch
from scipy.spatial import cKDTree

REPO = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "scripts"))
sys.path.insert(0, str(REPO / "python"))   # the sidecar imports its neighbours (_device) by bare name
import point_sources as ps  # noqa: E402  why: importable only after the sys.path insert

NAMES = ("R", "G1", "G2", "B")
SNRS = (8.0, 15.0, 30.0)
PER = 100
TRAIL_ANGLE = 119.2
SHAPES = (("core", 1.0, False, False, False), ("streak", 0.0, True, False, False),
          ("core+streak", 0.4, True, False, False), ("core+end", 0.4, True, False, True),
          ("core+halo", 0.4, False, True, False))
PY, PX = np.mgrid[-8:9, -8:9]
RING = (np.hypot(PY, PX) >= 5) & (np.hypot(PY, PX) <= 8)


def load_sidecar():
    spec = importlib.util.spec_from_file_location("denoise_raw", REPO / "python" / "denoise_raw.py")
    dr = importlib.util.module_from_spec(spec)
    sys.modules["denoise_raw"] = dr
    spec.loader.exec_module(dr)
    return dr


def gauss_component(xy, amp, minor, major, angle_deg, shape, shift=(0.0, 0.0)):
    """(4,H,W) light of one Gaussian per star, unit peak at its centre times amp[i]; photosite box folded in."""
    H, W = shape
    light = np.zeros((4, H, W), np.float32)
    s = ps.FWHM_TO_SIGMA
    c, sn = math.cos(math.radians(angle_deg)), math.sin(math.radians(angle_deg))
    a, b = (major * s) ** 2 + 1 / 12, (minor * s) ** 2 + 1 / 12
    cxx, cyy, cxy = a * c * c + b * sn * sn, a * sn * sn + b * c * c, (a - b) * c * sn
    det = cxx * cyy - cxy * cxy
    inv = (cyy / det, -cxy / det, cxx / det)
    reach = 11
    colour = (0.45, 1.0, 1.0, 0.60)
    for plane, (oy, ox) in enumerate(ps.CFA_OFFSETS):
        for (x, y), pk in zip(xy, amp):
            x, y = x + shift[0], y + shift[1]
            i0, j0 = int(round((y - oy) / 2)), int(round((x - ox) / 2))
            ii, jj = np.mgrid[i0 - reach:i0 + reach + 1, j0 - reach:j0 + reach + 1]
            ey, ex = (2 * ii + oy) - y, (2 * jj + ox) - x
            q = inv[0] * ex * ex + 2 * inv[1] * ex * ey + inv[2] * ey * ey
            v = np.exp(-0.5 * q)
            ok = (ii >= 0) & (ii < H) & (jj >= 0) & (jj < W)
            light[plane, ii[ok], jj[ok]] += (colour[plane] * pk * v[ok]).astype(np.float32)
    return light


def g1_peak(mosaic, xy):
    """G1 peak excess (sample minus the 5-8 sample ring's median) at the plane sample nearest each site."""
    plane = np.asarray(mosaic[0::2, 1::2], np.float32)
    cx, cy = (xy[:, 0].astype(int) - 1) // 2, xy[:, 1].astype(int) // 2
    out = np.full(len(xy), np.nan)
    ok = np.flatnonzero((cx >= 8) & (cy >= 8) & (cx < plane.shape[1] - 8) & (cy < plane.shape[0] - 8))
    p = plane[cy[ok, None, None] + PY, cx[ok, None, None] + PX]
    out[ok] = p[:, 8, 8] - np.median(p[:, RING], axis=1)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--weights", required=True, help="candidate state dict (strict, weights_only)")
    ap.add_argument("--mosaic", required=True, help="16-bit RGGB mosaic the cleaner received")
    ap.add_argument("--sites", required=True, help="star-standard work directory holding fixed-sites.npz")
    ap.add_argument("--offset", default="32,20", help="mosaic offset of the standard's sites, x,y")
    ap.add_argument("--black", type=float, default=512.0)
    ap.add_argument("--white", type=float, default=15360.0)
    ap.add_argument("--sky", default="128,128,9216,4096", help="x,y,w,h of empty sky in render coordinates")
    ap.add_argument("--json", required=True)
    ap.add_argument("--work", help="where the injected mosaics and the cleaner's outputs go (cached by shape)")
    ap.add_argument("--cache", default=str(REPO / "python" / "weights"))
    ap.add_argument("--seed", type=int, default=37)
    args = ap.parse_args()
    work = pathlib.Path(args.work or (pathlib.Path(args.json).resolve().parent / "composite-work"))
    work.mkdir(parents=True, exist_ok=True)
    dr = load_sidecar()
    dr.log = lambda *a, **k: None
    pinned = dr.load_model
    state = torch.load(args.weights, map_location="cpu", weights_only=True)

    def load_model(cache_dir, device):
        model = pinned(cache_dir, device)
        model.load_state_dict(state, strict=True)
        return model.eval().to(device)

    dr.load_model = load_model
    tag = pathlib.Path(args.weights).stem
    mos16 = cv2.imread(args.mosaic, cv2.IMREAD_UNCHANGED)
    if mos16 is None or mos16.ndim != 2:
        raise SystemExit(f"not a single-channel mosaic: {args.mosaic}")
    black, white = args.black, args.white
    planes = {n: (mos16[oy::2, ox::2].astype(np.float32) - black) / (white - black) for n, (oy, ox) in zip(NAMES, ps.CFA_OFFSETS)}
    ab = dr.noise_model({n: np.minimum(p, 1.0) for n, p in planes.items()})
    fs = np.load(pathlib.Path(args.sites) / "fixed-sites.npz")
    off = np.array([int(v) for v in args.offset.split(",")])
    known = fs["sites"][:, :2] + off
    x0, y0, w0, h0 = (int(v) for v in args.sky.split(","))
    rng = np.random.default_rng(args.seed)
    gx, gy = np.meshgrid(np.arange(x0 + off[0] + 48, x0 + off[0] + w0 - 48, 48), np.arange(y0 + off[1] + 48, y0 + off[1] + h0 - 48, 48))
    cand = np.column_stack((gx.ravel(), gy.ravel())).astype(np.float64)
    d, _ = cKDTree(known).query(cand)
    cand = cand[d >= 30]
    rng.shuffle(cand)
    n = len(SNRS) * PER
    xy = cand[:n].copy()
    xy[:, 0] = 2 * np.floor(xy[:, 0] / 2) + 1   # a G1 photosite: row even, col odd
    xy[:, 1] = 2 * np.floor(xy[:, 1] / 2)
    snr = np.repeat(np.array(SNRS), PER)
    g1 = planes["G1"]
    bg = np.array([np.median(g1[int(y / 2) - 8:int(y / 2) + 9, int(x / 2) - 8:int(x / 2) + 9]) for x, y in xy])
    a_g, b_g = ab["G1"]
    peak = snr * np.sqrt(a_g * np.maximum(bg, 0) + b_g)
    ux, uy = math.cos(math.radians(TRAIL_ANGLE)), math.sin(math.radians(TRAIL_ANGLE))
    shape = g1.shape

    def to_mosaic(pl):
        out = np.zeros(mos16.shape, np.uint16)
        for name, (oy, ox) in zip(NAMES, ps.CFA_OFFSETS):
            out[oy::2, ox::2] = np.clip(np.round(pl[name] * (white - black) + black), 0, white).astype(np.uint16)
        return out

    def run(in16, name):
        out_png = work / f"{name}-{tag}.png"
        if not out_png.exists():
            in_png = work / f"{name}-in.png"
            cv2.imwrite(str(in_png), in16)
            sys.argv = ["denoise_raw.py", "--input", str(in_png), "--output", str(out_png), "--pattern", "RGGB",
                        "--black", ",".join([str(int(black))] * 4), "--white", str(int(white)), "--strength", "1.0",
                        "--cache", args.cache]
            dr.main()
        return cv2.imread(str(out_png), cv2.IMREAD_UNCHANGED)

    report = {"weights": args.weights, "mosaic": args.mosaic, "shapes": {}}
    for name, core_share, streak, halo, end in SHAPES:
        light = np.zeros((4,) + shape, np.float32)
        if core_share > 0:
            light += gauss_component(xy, core_share * peak, 1.8, 1.8, 0.0, shape)
        if streak:
            light += gauss_component(xy, (1 - core_share) * peak, 4.0, 10.0, TRAIL_ANGLE, shape,
                                     (3.0 * ux, 3.0 * uy) if end else (0.0, 0.0))
        if halo:
            light += gauss_component(xy, (1 - core_share) * peak, 7.0, 7.0, 0.0, shape)
        # Under `end` the streak's centre moved 3 px along its axis, so the core stays at xy.
        noise_rng = np.random.default_rng(args.seed + 1)
        noisy = {k: (planes[k] + noise_rng.poisson(light[i] / ab[k][0]) * ab[k][0]).astype(np.float32) for i, k in enumerate(NAMES)}
        noisy16, truth16 = to_mosaic(noisy), to_mosaic({k: light[i] for i, k in enumerate(NAMES)})
        after16 = run(noisy16, name)
        t, nz, a = g1_peak(truth16, xy), g1_peak(noisy16, xy), g1_peak(after16, xy)
        rows = {}
        for s in SNRS:
            m = (snr == s) & np.isfinite(t) & np.isfinite(a) & np.isfinite(nz) & (t > 0) & (nz > 0)
            rows[f"snr {s:g}"] = {"n": int(m.sum()), "peak_vs_truth": float(np.mean(a[m] / t[m])),
                                  "peak_vs_noisy": float(np.mean(a[m] / nz[m]))}
        report["shapes"][name] = rows
        print(f"{name:12s} G1 core peak vs truth  " + "  ".join(f"snr {s:g}: {rows[f'snr {s:g}']['peak_vs_truth']:.3f}" for s in SNRS), flush=True)
    pathlib.Path(args.json).write_text(json.dumps(report, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
