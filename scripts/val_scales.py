#!/usr/bin/env python3
"""Held-out PSNR of one state dict at several sigma scales.

`train_raw.py` validates the fine-tune at its training condition (scale 1).
The shipped sidecar feeds 0.85. Once a fine-tuned network denoises harder at
its own estimate, the operating point has to be re-chosen on the SAME held-out
split the training used, which is what this prints — the trainer's own
validate(), lifted out so a finished run can be re-measured without retraining.

Usage: python val_scales.py --weights <state.pth|pretrained> --scales 0.5,0.7,0.85,1.0

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
import math
import pathlib

import numpy as np
import torch

import train_raw as T  # the data, the forward and the batch maths, unchanged

BINS = (("low", 0.0, 3e-4), ("mid", 3e-4, 1.5e-3), ("high", 1.5e-3, 1.0))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--cache", default=str(T.REPO / "python" / "weights"))
    ap.add_argument("--weights", required=True, help="a state dict, or 'pretrained'")
    ap.add_argument("--scales", default="0.5,0.6,0.7,0.85,1.0")
    ap.add_argument("--val-crops", type=int, default=512)
    args = ap.parse_args()
    dev = "cuda"

    pairs_val = T.Shards(pathlib.Path(args.data) / "pairs", "pairs_val", with_ab=True)
    vrng = np.random.default_rng(12345)  # the trainer's own val seed: the same crops
    vidx = vrng.choice(len(pairs_val), size=min(args.val_crops, len(pairs_val)), replace=False)
    val = [pairs_val.get(int(k)) for k in vidx]
    print(f"{len(val)} held-out crops", flush=True)

    model = T.dr.load_model(args.cache, dev)
    if args.weights != "pretrained":
        model.load_state_dict(torch.load(args.weights, map_location="cpu", weights_only=True))
    model = model.to(dev).to(memory_format=torch.channels_last).eval()

    for scale in [float(s) for s in args.scales.split(",")]:
        rows = []
        with torch.no_grad():
            for k in range(0, len(val), 8):
                chunk = val[k:k + 8]
                xn = torch.from_numpy(np.stack([c[0][:4] for c in chunk])).to(dev)
                xc = torch.from_numpy(np.stack([c[0][4:] for c in chunk])).to(dev)
                ab = torch.from_numpy(np.stack([c[1] for c in chunk])).to(dev)
                a, b = ab[..., 0].clamp_min(1e-7), ab[..., 1].clamp_min(0)
                zn, tn, sigma, span = T.make_batch(xn, xc, a, b)
                with torch.autocast("cuda", dtype=torch.float16):
                    o = T.forward(model, zn, sigma * scale)
                zhat = T.merge_triplets(o.float(), zn.shape[0]) * span[:, None, None, None]
                xh = T.ns.igat(zhat, a[:, :, None, None], b[:, :, None, None]).clamp(0, 1)
                for i in range(xh.shape[0]):
                    c = xc[i].clamp(0, 1)
                    row = {"a": float(a[i, 1])}
                    row["lin"] = 10 * math.log10(1.0 / max(float(((xh[i] - c) ** 2).mean()), 1e-12))
                    row["sqrt"] = 10 * math.log10(
                        1.0 / max(float(((xh[i].sqrt() - c.sqrt()) ** 2).mean()), 1e-12))
                    rows.append(row)
        bins = []
        for name, lo, hi in BINS:
            sel = [x["sqrt"] for x in rows if lo <= x["a"] < hi]
            bins.append(f"{name} {np.mean(sel):.3f}/{len(sel)}" if sel else f"{name} -/0")
        print(f"[s{scale:g}] lin {np.mean([x['lin'] for x in rows]):.3f} "
              f"sqrt {np.mean([x['sqrt'] for x in rows]):.3f}  by level: {' '.join(bins)}", flush=True)


if __name__ == "__main__":
    main()
