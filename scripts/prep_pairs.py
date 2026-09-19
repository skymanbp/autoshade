#!/usr/bin/env python3
"""Real noisy/clean crop pairs from RawNIND's Bayer scenes.

Per scene: the clean reference is the MEAN of its GT frames (normalised per
CFA phase, planes R, G1, G2, B). Per noisy frame of that scene:
  1. normalise the same way; measure its noise model with the SIDECAR'S OWN
     estimator on the whole frame (`denoise_raw.noise_model`), which is what
     inference will feed the network;
  2. align to the reference by an integer translation (coarse search on 8x
     box-downsampled G planes within +-128 mosaic px, then +-4 px at full
     resolution) — translation in whole CFA periods only, so phases agree;
  3. per-plane gain: least squares of the noisy low-pass against the
     reference low-pass over unclipped samples;
  4. validity: a 32-px block is kept when its gain-matched low-pass residual
     is within 4 sigma of the noise expected at that level (motion, flicker
     and parallax fail it), and nothing in it is clipped;
  5. cut --per-frame crops (P x P planes) whose blocks are all valid.
Shards: pairs_{split}_{k}.npy (N, 8, P, P) float16 = noisy 4 planes then clean
4 planes, and pairs_{split}_{k}_ab.npy (N, 4, 2) float32 = (a, b) per plane.
Split `val` = the dataset's own unknown_sensor + test_reserve scenes; `train`
= everything else. Scene names never leave this process.
Provenance — AutoShade v1.5.0. This is the pipeline that produced
`autoshade-raw-denoise-v1.pth`, the weights `python/denoise_raw.py` ships:
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
import pathlib
import sys

import numpy as np
import yaml

REPO = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "python"))
CANON = ("R", "G1", "G2", "B")
RAW_EXT = {".arw", ".cr2", ".nef", ".crw", ".dng"}


def is_raw(item):
    """dataset.yaml also lists .xmp sidecars next to some frames; only raw images count."""
    return pathlib.Path(item["filename"]).suffix.lower() in RAW_EXT


def load_planes(path):
    import rawpy
    with rawpy.imread(str(path)) as raw:
        mos = raw.raw_image_visible.astype(np.float32)
        blacks = np.array(raw.black_level_per_channel, np.float32)
        white = float(raw.white_level)
        pat = raw.raw_pattern
        desc = raw.color_desc.decode() if isinstance(raw.color_desc, bytes) else raw.color_desc
    letters = "".join(desc[pat[dy, dx]] for dy in range(2) for dx in range(2))
    phases, g = {}, 0
    for i, c in enumerate(letters):
        pos = (i // 2, i % 2)
        if c == "G":
            g += 1
            phases[f"G{g}"] = pos
        elif c in "RB":
            phases[c] = pos
    if sorted(phases) != sorted(CANON):
        raise ValueError(f"CFA {letters}")
    H, W = mos.shape[0] // 2 * 2, mos.shape[1] // 2 * 2
    planes = []
    for n in CANON:
        dy, dx = phases[n]
        b = float(blacks[pat[dy, dx]])
        planes.append(np.minimum((mos[dy:H:2, dx:W:2] - b) / (white - b), 1.0))
    return np.stack(planes).astype(np.float32)


def down8(p):
    h, w = p.shape[-2] // 8 * 8, p.shape[-1] // 8 * 8
    return p[..., :h, :w].reshape(*p.shape[:-2], h // 8, 8, w // 8, 8).mean(axis=(-3, -1))


def best_shift(ref, img, radius, step=1):
    """Integer (dy, dx) maximising the correlation of img shifted onto ref."""
    best, bs = None, (0, 0)
    H, W = ref.shape
    m = radius
    r = ref[m:H - m, m:W - m]
    r = (r - r.mean()) / (r.std() + 1e-9)
    for dy in range(-radius, radius + 1, step):
        for dx in range(-radius, radius + 1, step):
            c = img[m + dy:H - m + dy, m + dx:W - m + dx]
            if c.shape != r.shape:
                continue
            c = (c - c.mean()) / (c.std() + 1e-9)
            s = float((r * c).mean())
            if best is None or s > best:
                best, bs = s, (dy, dx)
    return bs, best


def shift_planes(p, dy, dx):
    """Planes of the noisy frame translated by (dy, dx) plane px onto the
    reference grid; uncovered borders are NaN."""
    out = np.full_like(p, np.nan)
    H, W = p.shape[-2:]
    ys, yd = (slice(dy, H), slice(0, H - dy)) if dy >= 0 else (slice(0, H + dy), slice(-dy, H))
    xs, xd = (slice(dx, W), slice(0, W - dx)) if dx >= 0 else (slice(0, W + dx), slice(-dx, W))
    out[..., yd, xd] = p[..., ys, xs]
    return out


def process_scene(args):
    name, scene, root, P, per_frame, seed = args
    import denoise_raw as dr
    import cv2
    rng = np.random.default_rng(seed)
    raw_dir = pathlib.Path(root) / "raw"
    try:
        gts = [load_planes(raw_dir / i["filename"]) for i in scene.get("clean_images", [])
               if is_raw(i) and (raw_dir / i["filename"]).exists()]
    except Exception as e:  # noqa: BLE001  why: an undecodable scene is reported by count, not fatal
        return name, [], [], f"gt unreadable ({type(e).__name__}: {e})"
    if not gts:
        return name, [], [], "no gt on disk"
    shape = gts[0].shape
    gts = [g for g in gts if g.shape == shape]
    ref = np.mean(gts, axis=0)
    ref_clip = np.max(gts, axis=0) >= 0.95
    crops, abs_, notes = [], [], []
    for item in filter(is_raw, scene.get("noisy_images", [])):
        path = raw_dir / item["filename"]
        if not path.exists():
            notes.append("missing")
            continue
        try:
            noisy = load_planes(path)
        except Exception as e:  # noqa: BLE001  why: same — counted per frame
            notes.append(f"unreadable {type(e).__name__}")
            continue
        if noisy.shape != shape:
            notes.append("shape")
            continue
        try:
            ab = dr.noise_model({n: noisy[i] for i, n in enumerate(CANON)})
        except SystemExit:
            notes.append("noise model refused")
            continue
        # alignment on the mean green plane
        g_ref = down8((ref[1] + ref[2]) / 2)
        g_noi = down8((noisy[1] + noisy[2]) / 2)
        (cy, cx), _ = best_shift(g_ref, g_noi, radius=8)  # +-8 down8 px = +-64 plane px = +-128 mosaic px
        dy, dx = cy * 8, cx * 8
        fine_ref = (ref[1] + ref[2]) / 2
        H, W = fine_ref.shape
        crop = (slice(H // 3, H // 3 + 512), slice(W // 3, W // 3 + 512))
        best = None
        for ddy in range(-4, 5):
            for ddx in range(-4, 5):
                s = shift_planes(noisy[1:3].mean(axis=0, keepdims=True), dy + ddy, dx + ddx)[0][crop]
                r = fine_ref[crop]
                ok = ~np.isnan(s)
                if ok.mean() < 0.9:
                    continue
                c = np.corrcoef(r[ok], s[ok])[0, 1]
                if best is None or c > best[0]:
                    best = (c, ddy, ddx)
        if best is None:
            notes.append("align")
            continue
        dy, dx = dy + best[1], dx + best[2]
        al = shift_planes(noisy, dy, dx)
        # per-plane gain on low-pass, unclipped
        matched = np.empty_like(al)
        gains = np.ones(4, np.float32)
        valid = ~np.isnan(al).any(axis=0) & ~ref_clip.any(axis=0) & ~(np.nan_to_num(al, nan=1.0) >= 0.95).any(axis=0)
        for i in range(4):
            lr = cv2.blur(ref[i], (9, 9))
            ln = cv2.blur(np.nan_to_num(al[i]), (9, 9))
            m = valid & (lr > 0.01)
            if m.sum() >= 1000:
                gains[i] = float((lr[m] * ln[m]).sum() / max((ln[m] ** 2).sum(), 1e-12))
            matched[i] = al[i] * gains[i]
        # block validity: gain-matched low-pass residual within 4 expected sigma
        B = 32
        bh, bw = H // B, W // B
        ok_blocks = np.ones((bh, bw), bool)
        for i, n in enumerate(CANON):
            a, b = ab[n]
            res = cv2.blur(np.nan_to_num(matched[i] - ref[i]), (B, B))[B // 2::B, B // 2::B][:bh, :bw]
            lvl = cv2.blur(ref[i], (B, B))[B // 2::B, B // 2::B][:bh, :bw]
            sig = np.sqrt(np.clip(a * lvl + b, 1e-12, None)) / B  # std of a 32x32 mean
            ok_blocks &= np.abs(res) <= 4.0 * sig + 0.002 * lvl
        vb = valid[: bh * B, : bw * B].reshape(bh, B, bw, B).all(axis=(1, 3))
        ok_blocks &= vb
        k = P // B
        cand = np.argwhere(np.lib.stride_tricks.sliding_window_view(ok_blocks, (k, k)).all(axis=(2, 3)))
        if len(cand) == 0:
            notes.append("no valid window")
            continue
        pick = cand[rng.choice(len(cand), size=min(per_frame, len(cand)), replace=False)]
        for (by, bx) in pick:
            y, x = by * B, bx * B
            # The noisy frame keeps its OWN values, so its noise and its (a, b)
            # stay consistent; the gain moves the clean reference into the
            # noisy frame's scale instead.
            n4 = np.nan_to_num(al[:, y:y + P, x:x + P])
            c4 = ref[:, y:y + P, x:x + P] / gains[:, None, None]
            crops.append(np.concatenate([n4, c4]).astype(np.float16))
            abs_.append(np.array([ab[n] for n in CANON], np.float32))
        notes.append("ok")
    return name, crops, abs_, ",".join(sorted(set(notes)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--patch", type=int, default=256)
    ap.add_argument("--per-frame", type=int, default=6)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--shard", type=int, default=1024)
    ap.add_argument("--only", choices=["train", "val"], default=None)
    args = ap.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    scenes = yaml.safe_load(open(pathlib.Path(args.root) / "dataset.yaml", encoding="utf-8"))["Bayer"]
    jobs = []
    for i, (name, s) in enumerate(sorted(scenes.items())):
        split = "val" if (s.get("unknown_sensor") or s.get("test_reserve")) else "train"
        if args.only and split != args.only:
            continue
        jobs.append((split, (name, s, args.root, args.patch, args.per_frame, 7000 + i)))
    bufs = {"train": ([], []), "val": ([], [])}
    counts = {"train": 0, "val": 0}
    shard_i = {"train": 0, "val": 0}
    summary = {}

    def flush(split, force=False):
        c, a = bufs[split]
        while len(c) >= args.shard or (force and c):
            take = min(args.shard, len(c))
            np.save(out / f"pairs_{split}_{shard_i[split]:03d}.npy", np.stack(c[:take]))
            np.save(out / f"pairs_{split}_{shard_i[split]:03d}_ab.npy", np.stack(a[:take]))
            del c[:take], a[:take]
            shard_i[split] += 1

    with cf.ProcessPoolExecutor(args.workers) as ex:
        futs = {ex.submit(process_scene, job): split for split, job in jobs}
        for n, fut in enumerate(cf.as_completed(futs), 1):
            split = futs[fut]
            name, crops, abs_, note = fut.result()
            bufs[split][0].extend(crops)
            bufs[split][1].extend(abs_)
            counts[split] += len(crops)
            summary[note] = summary.get(note, 0) + 1
            flush(split)
            print(f"[{n}/{len(jobs)}] {split}: {len(crops)} crops ({note}); totals {counts}", flush=True)
    for split in bufs:
        flush(split, force=True)
    (out / "pairs_meta.json").write_text(json.dumps({"counts": counts, "shards": shard_i, "patch": args.patch,
                                                     "notes": summary}, indent=1), encoding="utf-8")
    print("done", counts, shard_i, flush=True)


if __name__ == "__main__":
    sys.exit(main())
