#!/usr/bin/env python3
"""Track B, step 1: where did the then-shipped RAW denoise (v1.4.1, DRUNet on the
GAT-stabilised mosaic) stand against Lightroom's Enhance->Denoise on REAL
high-ISO frames of the operator's own camera?

For every `-已增强-NR.dng` in the library at or above --min-iso, next to its
source ARW:
  * the noise model is measured on the WHOLE frame, exactly as the sidecar does;
  * three 2048x2048 mosaic windows are denoised: the flattest, the most detailed
    and the centre (picked on the input's own block statistics);
  * Lightroom's answer is its NewSubfileType=16 enhanced layer, aligned by
    correlation and re-mosaiced onto the CFA sites, so everything is compared
    per plane in the linear sensor domain — no demosaic, no tone curve;
  * per window and plane: fine grain in the flat blocks (finest Haar diagonal),
    blotch (32-px box-mean scatter) in the flat blocks, band-pass energy and its
    correlation with Lightroom's in the detail blocks, and the RMS distance to
    Lightroom as a fraction of the input's distance to it;
  * a 1:1 developed crop triple (input / ours / Lightroom) of the detail window
    is written so the numbers can be checked by eye.

No photograph is named in the output files: frames are numbered in the order
they are listed (the mapping is printed to the log only).
Provenance — AutoShade v1.5.0. This is the pipeline that produced
`autoshade-raw-denoise-v1.pth` and, continued from it with point sources in v1.5.2,
the `autoshade-raw-denoise-v2.pth` that `python/denoise_raw.py` ships:
DPIR's released `drunet_color` (KAIR, MIT) fine-tuned for that sidecar's own
transform, so nothing is learned that inference cannot reproduce. The real
training pairs are RawNIND (Brummer & De Vleeschouwer, UCLouvain Dataverse,
doi:10.14428/DVN/DEQCIM, CC BY-SA 4.0); the synthetic half is drawn over
low-ISO frames the operator owns. Every path here is an argument or is derived
from this file's location: none of it knows the machine it was written on.
"""
import argparse
import json
import os
import pathlib
import re
import sys
import time

import cv2
import numpy as np
import rawpy
import tifffile

REPO_PY = pathlib.Path(__file__).resolve().parents[0]
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "python"))
import denoise_raw as dr  # noqa: E402  why: the sidecar directory is only importable after the sys.path insert above
from _device import pick_device  # noqa: E402  why: same — lives beside denoise_raw.py

WIN = 2048


def iso_of(dng):
    with tifffile.TiffFile(dng) as t:
        exif = t.pages[0].tags.get("ExifTag")
        if exif is None:
            return None
        v = exif.value.get("ISOSpeedRatings") or exif.value.get("PhotographicSensitivity")
        return int(v[0] if isinstance(v, (tuple, list)) else v)


def enhanced_layer(dng):
    with tifffile.TiffFile(dng) as t:
        for sub in t.pages[0].pages:
            if int(sub.subfiletype) == 16 and int(sub.samplesperpixel) == 3:
                return sub.asarray()
    raise SystemExit(f"{dng}: no enhanced layer")


def haar_diag(x):
    h, w = x.shape[0] // 2 * 2, x.shape[1] // 2 * 2
    x = x[:h, :w]
    return (x[0::2, 0::2] - x[0::2, 1::2] - x[1::2, 0::2] + x[1::2, 1::2]) / 2.0


def mad(v):
    v = np.asarray(v).ravel()
    return float(np.median(np.abs(v - np.median(v))) / 0.6745) if v.size else float("nan")


def bandpass(x):
    return cv2.blur(x, (3, 3)) - cv2.blur(x, (9, 9))


def blocks(a, b):
    h, w = a.shape[0] // b * b, a.shape[1] // b * b
    return a[:h, :w].reshape(h // b, b, w // b, b).transpose(0, 2, 1, 3)


def pick_windows(g1, ab_g1):
    """Top-left mosaic corners (even) of the flattest, the most detailed and
    the centre 2048 window, scored on the G1 plane's 128-px blocks by band-pass
    variance over the noise model's variance at the block's level."""
    B = 128
    m = blocks(g1, B).mean(axis=(2, 3))
    bp = blocks(bandpass(g1), B).var(axis=(2, 3))
    noise = np.clip(ab_g1[0] * m + ab_g1[1], 1e-12, None)
    score = bp / noise
    k = (WIN // 2) // B  # window size in blocks on the plane
    H, W = score.shape
    from numpy.lib.stride_tricks import sliding_window_view
    if H < k or W < k:
        return {}
    sw = sliding_window_view(score, (k, k)).mean(axis=(2, 3))
    mw = sliding_window_view(m, (k, k)).mean(axis=(2, 3))
    valid = (mw > 0.003) & (mw < 0.6)
    if not valid.any():
        valid = np.ones_like(sw, bool)
    flat = np.unravel_index(np.where(valid, sw, np.inf).argmin(), sw.shape)
    detail = np.unravel_index(np.where(valid, sw, -np.inf).argmax(), sw.shape)
    to_mosaic = lambda rc: (int(rc[0] * B * 2), int(rc[1] * B * 2))
    cy, cx = (g1.shape[0] - WIN // 2), (g1.shape[1] - WIN // 2)
    return {"flat": to_mosaic(flat), "detail": to_mosaic(detail), "centre": (cy // 2 * 2, cx // 2 * 2)}


def develop(planes_or_rgb, wbn, gain, is_planes):
    if is_planes:
        p = planes_or_rgb
        h, w = p["R"].shape
        rgb = np.zeros((h, w, 3), np.float32)
        rgb[:, :, 0] = p["R"]
        rgb[:, :, 1] = (p["G1"] + p["G2"]) / 2.0
        rgb[:, :, 2] = p["B"]
    else:
        rgb = planes_or_rgb
    img = np.clip(rgb * wbn[None, None, :] * gain, 0, 1)
    img = np.where(img <= 0.0031308, img * 12.92, 1.055 * img ** (1 / 2.4) - 0.055)
    return (np.clip(img, 0, 1)[:, :, ::-1] * 255 + 0.5).astype(np.uint8)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--library", required=True)
    ap.add_argument("--min-iso", type=int, default=1000)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cache", default=str(pathlib.Path(__file__).resolve().parent.parent / "python" / "weights"))
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    pairs = []
    for dng in sorted(pathlib.Path(args.library).rglob("*已增强-NR*.dng")):
        stem = re.sub(r"-已增强-NR.*$", "", dng.stem)
        arw = dng.with_name(stem + ".ARW")
        if not arw.exists():
            continue
        iso = iso_of(dng)
        if iso is None or iso < args.min_iso:
            continue
        if any(p[0] == arw for p in pairs):
            continue  # two enhanced variants of one frame: the first is enough
        pairs.append((arw, dng, iso))
    if args.limit:
        pairs = pairs[: args.limit]
    print(f"{len(pairs)} frames at ISO >= {args.min_iso}")

    device = pick_device(False, "cuda:0")
    model = dr.load_model(args.cache, device)
    results = []
    for fi, (arw, dng, iso) in enumerate(pairs):
        t0 = time.time()
        tag = f"F{fi:02d}"
        print(f"\n== {tag} ISO {iso}  ({arw.parent.name}/{arw.name})", flush=True)
        with rawpy.imread(str(arw)) as raw:
            mos = raw.raw_image_visible.astype(np.float32)
            blacks = np.array(raw.black_level_per_channel, np.float32)
            white = float(raw.white_level)
            pat = raw.raw_pattern
            desc = raw.color_desc.decode() if isinstance(raw.color_desc, bytes) else raw.color_desc
            wb = np.array(raw.camera_whitebalance[:3], np.float32)
        letters = "".join(desc[pat[dy, dx]] for dy in range(2) for dx in range(2))
        letters = letters.replace("G", "G")
        phases = dr.parse_pattern(letters)
        black_at = {n: float(blacks[pat[dy, dx]]) for n, (dy, dx) in phases.items()}
        full = {n: np.minimum((p - black_at[n]) / (white - black_at[n]), 1.0)
                for n, p in dr.split_planes(mos, phases).items()}
        ab = dr.noise_model(full)

        lr = enhanced_layer(dng).astype(np.float32)
        L = (lr - 2048.0) / (65535.0 - 2048.0)
        # column offset of the enhanced layer against the mosaic, by correlation on G1
        g1y, g1x = phases["G1"]
        best = None
        for dx in range(0, 65, 2):
            a = mos[3000:3200:2, 4000 + g1x:4400:2]
            b = L[3000:3200:2, 4000 + g1x + dx:4400 + dx:2, 1]
            if a.shape != b.shape:
                continue
            r = float(np.corrcoef(a.ravel(), b.ravel())[0, 1])
            if best is None or r > best[1]:
                best = (dx, r)
        dx_off, corr = best
        print(f"   enhanced layer offset {dx_off} px (r={corr:.3f}); model "
              + " ".join(f"{n}:a={ab[n][0]:.2e},b={ab[n][1]:.1e}" for n in dr.PLANES))
        wins = pick_windows(full["G1"], ab["G1"])
        wbn = wb / wb[1]
        frame = {"frame": tag, "iso": iso, "offset": dx_off, "corr": corr, "windows": {}}
        for wname, (y0, x0) in wins.items():
            y0 = min(max(0, y0), mos.shape[0] - WIN) // 2 * 2
            x0 = min(max(0, x0), mos.shape[1] - WIN - dx_off - 2) // 2 * 2
            planes = {n: np.ascontiguousarray(full[n][y0 // 2:(y0 + WIN) // 2, x0 // 2:(x0 + WIN) // 2]) for n in dr.PLANES}
            den = dr.denoise_planes(model, planes, ab, device, 512, 32, False)
            lrp = {}
            for n, (py, px) in phases.items():
                ch = {"R": 0, "G1": 1, "G2": 1, "B": 2}[n]
                lrp[n] = np.ascontiguousarray(L[y0 + py:y0 + WIN:2, x0 + px + dx_off:x0 + WIN + dx_off:2, ch])
            rows = {}
            for n in dr.PLANES:
                pin, pou, plr = planes[n], den[n], lrp[n]
                B = 32
                m = blocks(pin, B).mean(axis=(2, 3))
                bpv = blocks(bandpass(pin), B).var(axis=(2, 3))
                nv = np.clip(ab[n][0] * m + ab[n][1], 1e-12, None)
                ratio = bpv / nv
                flat = ratio <= np.percentile(ratio, 30)
                detail = ratio >= np.percentile(ratio, 80)

                def grain(p):
                    d = blocks(haar_diag(p), B // 2)
                    return mad(d[flat])

                def blotch(p):
                    return mad(blocks(p, B).mean(axis=(2, 3))[flat])

                def band(p):
                    return float(np.sqrt(blocks(bandpass(p), B).var(axis=(2, 3))[detail].mean()))

                bl = blocks(bandpass(plr), B)[detail].ravel()
                bo = blocks(bandpass(pou), B)[detail].ravel()
                bi = blocks(bandpass(pin), B)[detail].ravel()
                rows[n] = {
                    "grain_in": grain(pin), "grain_ours": grain(pou), "grain_lr": grain(plr),
                    "blotch_in": blotch(pin), "blotch_ours": blotch(pou), "blotch_lr": blotch(plr),
                    "band_in": band(pin), "band_ours": band(pou), "band_lr": band(plr),
                    "corr_ours_lr": float(np.corrcoef(bo, bl)[0, 1]), "corr_in_lr": float(np.corrcoef(bi, bl)[0, 1]),
                    "rms_ours_lr": float(np.sqrt(((pou - plr) ** 2).mean())),
                    "rms_in_lr": float(np.sqrt(((pin - plr) ** 2).mean())),
                    "mean_in": float(pin.mean()), "mean_lr": float(plr.mean()),
                }
            frame["windows"][wname] = rows
            g = lambda k: np.mean([rows[n][k] for n in dr.PLANES])
            print(f"   {wname:6s} grain ours/LR {g('grain_ours') / g('grain_lr'):5.2f}  (in/LR {g('grain_in') / g('grain_lr'):5.2f})"
                  f" | blotch ours/LR {g('blotch_ours') / g('blotch_lr'):5.2f}"
                  f" | band ours/LR {g('band_ours') / g('band_lr'):5.2f} (in/LR {g('band_in') / g('band_lr'):5.2f})"
                  f" | corr(ours,LR) {g('corr_ours_lr'):.3f} (in {g('corr_in_lr'):.3f})"
                  f" | rms ours→LR {g('rms_ours_lr') / g('rms_in_lr'):.2f} of input's", flush=True)
            if wname == "detail":
                cy, cx = WIN // 4 - 150, WIN // 4 - 225
                crop = lambda p: {n: p[n][cy:cy + 300, cx:cx + 450] for n in dr.PLANES}
                ref = crop(planes)
                gain = 0.9 / max(np.percentile(np.stack([ref["R"], ref["G1"], ref["B"]]), 99.5), 1e-4)
                tiles = [develop(crop(planes), wbn, gain, True), develop(crop(den), wbn, gain, True),
                         develop(crop(lrp), wbn, gain, True)]
                big = [cv2.resize(t, (t.shape[1] * 2, t.shape[0] * 2), interpolation=cv2.INTER_NEAREST) for t in tiles]
                cv2.imwrite(str(out / f"{tag}_iso{iso}_detail_in-ours-lr.png"), np.concatenate(big, axis=1))
        results.append(frame)
        del lr, L, mos, full
        print(f"   {time.time() - t0:.0f}s", flush=True)
        (out / "realset.json").write_text(json.dumps(results, indent=1), encoding="utf-8")
    print(f"\nwrote {out / 'realset.json'}")


if __name__ == "__main__":
    main()
