#!/usr/bin/env python3
"""Clean training crops for the RAW denoiser fine-tune.

From every low-ISO Bayer RAW under --library (ISO <= --max-iso), cut --per-frame
random 2*P x 2*P mosaic windows (even-aligned, so every crop starts on the
CFA's (0,0) phase), normalise per phase x = (v - black) / (white - black) with
the top clipped at 1, and store the four half-resolution planes in the
canonical order (R, G1, G2, B) as float16 shards of shape (N, 4, P, P).

A crop is rejected when it holds almost no signal (mean < 0.004), when more
than 5 % of its samples are clipped, or when it is nearly flat (plane std of
the G1 plane below 0.002) — a denoiser learns nothing from a black or blown
square. No file name is written anywhere in the output: crops carry only a
running index, and the list of sources stays in this process's memory.
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
import concurrent.futures as cf
import json
import os
import pathlib
import sys

import numpy as np

CANON = ("R", "G1", "G2", "B")


def phases_of(raw):
    desc = raw.color_desc.decode() if isinstance(raw.color_desc, bytes) else raw.color_desc
    pat = raw.raw_pattern
    letters = "".join(desc[pat[dy, dx]] for dy in range(2) for dx in range(2))
    out, g = {}, 0
    for i, c in enumerate(letters):
        pos = (i // 2, i % 2)
        if c == "G":
            g += 1
            out[f"G{g}"] = pos
        elif c in "RB":
            out[c] = pos
        else:
            return None, letters
    if sorted(out) != sorted(CANON):
        return None, letters
    return out, letters


def iso_of(path):
    import tifffile
    try:
        with tifffile.TiffFile(path) as t:
            ex = t.pages[0].tags.get("ExifTag")
            v = ex.value.get("ISOSpeedRatings") if ex else None
            v = v[0] if isinstance(v, (tuple, list)) else v
            return int(v) if v else None
    except Exception:  # noqa: BLE001  why: an unreadable header just means the frame is not considered
        return None


def crops_of(path, P, per_frame, seed):
    import rawpy
    rng = np.random.default_rng(seed)
    try:
        with rawpy.imread(str(path)) as raw:
            mos = raw.raw_image_visible.astype(np.float32)
            blacks = np.array(raw.black_level_per_channel, np.float32)
            white = float(raw.white_level)
            pat = raw.raw_pattern
            phases, letters = phases_of(raw)
    except Exception as e:  # noqa: BLE001  why: a corrupt or unsupported RAW is skipped and counted, not fatal
        return [], f"unreadable ({type(e).__name__})"
    if phases is None:
        return [], f"not RGGB-family Bayer ({letters})"
    H, W = mos.shape[0] // 2 * 2, mos.shape[1] // 2 * 2
    S = 2 * P
    if H < S or W < S:
        return [], "too small"
    out, tries = [], 0
    while len(out) < per_frame and tries < per_frame * 8:
        tries += 1
        y = int(rng.integers(0, (H - S) // 2 + 1)) * 2
        x = int(rng.integers(0, (W - S) // 2 + 1)) * 2
        win = mos[y:y + S, x:x + S]
        planes = []
        for n in CANON:
            dy, dx = phases[n]
            b = float(blacks[pat[dy, dx]])
            planes.append(np.minimum((win[dy::2, dx::2] - b) / (white - b), 1.0))
        arr = np.stack(planes)
        if arr.mean() < 0.004 or (arr >= 0.999).mean() > 0.05 or arr[1].std() < 0.002:
            continue
        out.append(arr.astype(np.float16))
    return out, None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--library", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--max-iso", type=int, default=200)
    ap.add_argument("--patch", type=int, default=256, help="plane size P (mosaic window 2P)")
    ap.add_argument("--per-frame", type=int, default=8)
    ap.add_argument("--shard", type=int, default=1024)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--ext", default=".arw")
    args = ap.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    frames = sorted(p for p in pathlib.Path(args.library).rglob("*") if p.suffix.lower() == args.ext)
    chosen = []
    with cf.ThreadPoolExecutor(8) as ex:
        for p, iso in zip(frames, ex.map(iso_of, frames)):
            if iso is not None and iso <= args.max_iso:
                chosen.append(p)
    print(f"{len(frames)} {args.ext} files, {len(chosen)} at ISO <= {args.max_iso}", flush=True)
    buf, shard_i, total, skipped = [], 0, 0, {}

    def flush():
        nonlocal buf, shard_i
        if buf:
            np.save(out / f"clean_{shard_i:03d}.npy", np.stack(buf))
            shard_i += 1
            buf = []

    with cf.ProcessPoolExecutor(args.workers) as ex:
        futs = [ex.submit(crops_of, p, args.patch, args.per_frame, 1000 + i) for i, p in enumerate(chosen)]
        for i, fut in enumerate(futs):
            crops, why = fut.result()
            if why:
                skipped[why] = skipped.get(why, 0) + 1
            for c in crops:
                buf.append(c)
                total += 1
                if len(buf) == args.shard:
                    flush()
            if (i + 1) % 100 == 0:
                print(f"  {i + 1}/{len(chosen)} frames, {total} crops", flush=True)
    flush()
    (out / "clean_meta.json").write_text(json.dumps(
        {"frames": len(chosen), "crops": total, "patch": args.patch, "shards": shard_i,
         "max_iso": args.max_iso, "skipped": skipped}, indent=1), encoding="utf-8")
    print(f"done: {total} crops in {shard_i} shards; skipped {skipped}", flush=True)


if __name__ == "__main__":
    sys.exit(main())
