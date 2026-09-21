#!/usr/bin/env python3
"""Fine-tune DRUNet-colour for the RAW sidecar on realistic sensor noise.

The network is used EXACTLY as `python/denoise_raw.py` uses it, so nothing is
learned that inference cannot reproduce:

  planes x (R, G1, G2, B), noise model (a, b) per plane as the sidecar's
  estimator reports it → z = gat(x, a, b) → zn = z / span, span = max_n
  gat(1, a_n, b_n) → the model sees triplets (R, G1, B) and (R, G2, B) with
  the constant noise channel 1 / span (one z-unit is one sigma: no scale) →
  output ẑn → x̂ = igat(ẑn · span, a, b).

The TARGET is therefore in z: t = I_A⁻¹(x_clean / a + b / a²) / span, the
value whose exact unbiased inverse is the clean sample. v1's loss was L1 there
(`--loss l1z`), which weights shadows and highlights by their own noise.

v2 (2026-09-21) changes two things, both for one measured defect: v1 erased
faint point sources (`point_sources.py` has the numbers).
  * Half of all crops get STARS on the clean side before the noise is drawn, a
    real pair gets the stars' own photon counts on its noisy side
    (`point_sources.add_to_pair`, `--stars`).
  * The loss seeks the MEAN, in the units light adds in (`--loss l2x`, the
    default): (igat(out) − x_clean)² / (a·level + b + 3a²/8), `level` a 9×9
    mean of the NOISY input. L1's minimiser is the conditional median, and
    under a sky of stars too faint to stand alone the median is the empty sky:
    their flux went missing (0.4–1.9 DN per plane on the operator's star
    frame). A squared error's minimiser is the conditional mean, which keeps
    flux whatever it cannot resolve. It is taken after the inverse because
    E[x] is not igat(E[z]) across two hypotheses as far apart as "sky" and
    "star". The divisor is the variance the stabiliser itself assumes
    (1 / gat'² ), so shadows and highlights still weigh by their own noise, and
    it reads the noisy input only: a weight that knew the clean value would
    pull the minimiser off the mean, toward the darker hypothesis.
Validation reports, next to the PSNR of the star-free held-out pairs, the share
of injected star flux the network returns, by brightness class.

Half of every batch is a REAL pair (RawNIND, the dataset's train split: its
noisy frame, its own estimated (a, b), the gain-matched mean of its GT
frames); half is SYNTHETIC: a clean crop (the operator's low-ISO frames or a
RawNIND reference) with `noise_synth` noise drawn from a wide sensor family,
and the (a, b) the estimator would report for it.

Augmentation keeps the CFA geometry: mosaic-aware flips and transposes (the
one-sample crop that restores RGGB after a flip), exposure and white-balance
gains on the clean side of synthetic samples only.
Provenance — AutoShade v1.5.0 and its successor. With `--stars 0 --loss l1z`
and DPIR's `drunet_color` as the start this is the pipeline that produced
`autoshade-raw-denoise-v1.pth`: DPIR's released network (KAIR, MIT) fine-tuned
for the sidecar's own transform, so nothing is learned that inference cannot
reproduce. As it stands it starts from whatever `denoise_raw.load_model` loads
(the shipped weights) and produces their successor. The real
training pairs are RawNIND (Brummer & De Vleeschouwer, UCLouvain Dataverse,
doi:10.14428/DVN/DEQCIM, CC BY-SA 4.0); the synthetic half is drawn over
low-ISO frames the operator owns. Every path here is an argument or is derived
from this file's location: none of it knows the machine it was written on.

Reproduce, in order:

    python scripts/fetch_rawnind.py --root RAWNIND
    python scripts/prep_pairs.py    --root RAWNIND --out DATA/pairs
    python scripts/prep_clean.py    --library MY_RAWS --out DATA/clean_user
    python scripts/train_raw.py     --data DATA --out RUN
    python scripts/val_scales.py    --weights RUN/best_state.pth --data DATA
    python scripts/lr_realset.py    --library MY_LIGHTROOM_DNGS --out CMP
    python scripts/denoise_bench.py --clean LOW_ISO.ARW --noisy HIGH_ISO.ARW \
        --levels "1.0,1.0;5.0,5.0" --out BENCH

The last three are the acceptance measurements `SIGMA_SCALE`'s table in
`python/denoise_raw.py` reports.
"""
import argparse
import json
import math
import pathlib
import random
import sys
import time

import numpy as np
import torch
import torch.nn.functional as F

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
REPO = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "python"))
import denoise_raw as dr  # noqa: E402  why: importable only after the sys.path insert
import noise_synth as ns  # noqa: E402  why: same
import point_sources as ps  # noqa: E402  because it is importable only after the sys.path insert above


# ── data ─────────────────────────────────────────────────────────────────────

class Shards:
    def __init__(self, pattern_dir, prefix, with_ab=False):
        d = pathlib.Path(pattern_dir)
        self.files = sorted(p for p in d.glob(f"{prefix}_*.npy") if not p.name.endswith("_ab.npy"))
        self.arrays = [np.load(p, mmap_mode="r") for p in self.files]
        self.abs = [np.load(str(p)[:-4] + "_ab.npy") for p in self.files] if with_ab else None
        self.index = [(i, j) for i, a in enumerate(self.arrays) for j in range(a.shape[0])]

    def __len__(self):
        return len(self.index)

    def get(self, k):
        i, j = self.index[k]
        arr = np.asarray(self.arrays[i][j], dtype=np.float32)
        return arr, (self.abs[i][j] if self.abs is not None else None)


def cfa_flip(p, rng):
    """p: (C, H, W) numpy with C = 4 (R, G1, G2, B) or 8 (two such stacks).
    A random mosaic-geometry-preserving flip / transpose; returns (C, H-1, W-1)."""
    C = p.shape[0]
    stacks = [p[k:k + 4] for k in range(0, C, 4)]
    hflip, vflip, trans = rng.random() < 0.5, rng.random() < 0.5, rng.random() < 0.5
    out = []
    for s in stacks:
        R, G1, G2, B = s
        H, W = R.shape
        if hflip:  # R, G2 keep columns 0..W-2 of the flipped plane; G1, B keep 1..W-1
            R, G2 = R[:, ::-1][:, :W - 1], G2[:, ::-1][:, :W - 1]
            G1, B = G1[:, ::-1][:, 1:], B[:, ::-1][:, 1:]
        else:
            R, G1, G2, B = R[:, :W - 1], G1[:, :W - 1], G2[:, :W - 1], B[:, :W - 1]
        if vflip:  # R, G1 keep rows 0..H-2 of the flipped plane; G2, B keep 1..H-1
            R, G1 = R[::-1][:H - 1], G1[::-1][:H - 1]
            G2, B = G2[::-1][1:], B[::-1][1:]
        else:
            R, G1, G2, B = R[:H - 1], G1[:H - 1], G2[:H - 1], B[:H - 1]
        if trans:  # transposing an RGGB mosaic swaps the two greens
            R, G1, G2, B = R.T, G2.T, G1.T, B.T
        out.extend([R, G1, G2, B])
    return np.ascontiguousarray(np.stack(out))


def random_crop(p, size, rng):
    H, W = p.shape[-2:]
    y = rng.integers(0, H - size + 1)
    x = rng.integers(0, W - size + 1)
    return p[..., y:y + size, x:x + size]


# ── the sidecar's use of the network ─────────────────────────────────────────

def to_triplets(zn):
    """(B,4,H,W) → (2B,3,H,W): (R,G1,B) then (R,G2,B)."""
    t1 = zn[:, [0, 1, 3]]
    t2 = zn[:, [0, 2, 3]]
    return torch.cat([t1, t2], 0)


def forward(model, zn, sigma):
    """zn (B,4,H,W) normalised stabilised planes, sigma (B,) → (2B,3,H,W)."""
    tri = to_triplets(zn)
    s = torch.cat([sigma, sigma]).view(-1, 1, 1, 1).expand(-1, 1, tri.shape[2], tri.shape[3])
    H, W = tri.shape[-2:]
    pb, pr = (-H) % 8, (-W) % 8
    inp = torch.cat([tri, s], 1)
    if pb or pr:
        inp = F.pad(inp, (0, pr, 0, pb), mode="replicate")
    return model(inp.contiguous(memory_format=torch.channels_last))[..., :H, :W]


def merge_triplets(out, B):
    """(2B,3,H,W) → (B,4,H,W) planes, R and B averaged over the two triplets."""
    t1, t2 = out[:B], out[B:]
    return torch.stack([(t1[:, 0] + t2[:, 0]) / 2, t1[:, 1], t2[:, 1], (t1[:, 2] + t2[:, 2]) / 2], 1)


def make_batch(x_noisy, x_clean, a, b):
    """a, b: (B,4). Returns zn, target tn, sigma, span."""
    a4, b4 = a[:, :, None, None], b[:, :, None, None]
    span = torch.stack([ns.gat(torch.ones_like(a[:, i]), a[:, i], b[:, i]) for i in range(4)], 1).max(1).values
    sp = span[:, None, None, None]
    zn = ns.gat(x_noisy, a4, b4) / sp
    tn = ns.i_a_inverse(torch.clamp(x_clean, min=0.0) / a4 + b4 / (a4 * a4)) / sp
    return zn, tn, 1.0 / span, span


# The floor the network's own answer is held to before the inverse transform.
# `ns.gat` is 2*sqrt(x/a + 3/8 + b/a^2), smallest at x = 0, so a TARGET can never
# come below 2*sqrt(3/8) = 1.2247: `denoise_raw.physical_model` returns a > 0 and
# b >= 0 on every path, and the synthetic family draws b/a^2 in [0.3, 60], whose
# own worst case is 2*sqrt(3/8 + 0.3) = 1.6432. This is therefore a guard on the
# network's OUTPUT and never on the thing it is learning — which is what
# `test_the_clamp_never_touches_a_target` holds it to.
Z_FLOOR = 1.0


def loss_mean_seeking(out, zn_span, x_noisy, x_clean, a, b):
    """The v2 loss (see the module docstring). out (2B,3,H,W) is the network's
    answer in normalised z; a, b (B,4)."""
    pick = lambda v: torch.cat([v[:, [0, 1, 3]], v[:, [0, 2, 3]]], 0)[:, :, None, None]
    a3, b3 = pick(a), pick(b)
    span = torch.cat([zn_span, zn_span]).view(-1, 1, 1, 1)
    # z >= 1 is where I_A is monotone, and every target lies above it (Z_FLOOR)
    x_hat = ns.igat((out * span).clamp_min(Z_FLOOR), a3, b3)
    level = F.avg_pool2d(F.pad(to_triplets(x_noisy), (4, 4, 4, 4), mode="reflect"), 9, stride=1).clamp_min(0.0)
    variance = a3 * level + b3 + 0.375 * a3 * a3
    return ((x_hat - to_triplets(x_clean)) ** 2 / variance).mean()


FLUX_CLASSES = ((1.5, 3.0), (3.0, 6.0), (6.0, 12.0), (12.0, 25.0), (25.0, 400.0))


def flux_field(clean, dev):
    """A fixed star field over fixed clean crops at the operator's star-frame
    noise (a 4.9e-4, b 6.2e-6, measured 2026-09-21): what `validate` asks the
    network to give back. Returns (noisy, a, b, stars, light, ground)."""
    rng = np.random.default_rng(777)
    gen = torch.Generator(device=dev)
    gen.manual_seed(777)
    n = clean.shape[0]
    a = torch.full((n, 4), 4.9e-4, device=dev)
    b = torch.full((n, 4), 6.2e-6, device=dev)
    ground = (clean * 0.0 + clean.mean(dim=(2, 3), keepdim=True).clamp(0.005, 0.05))
    stars = ps.draw(rng, n, clean.shape[2], clean.shape[3], share=1.0, field_share=1.0)
    sample = torch.as_tensor(stars["sample"], device=dev)
    sigma = torch.sqrt(a[sample, 1] * ground[sample, 1, 0, 0] + b[sample, 1])
    light = ps.render(stars, torch.as_tensor(stars["snr"], device=dev).float() * sigma, clean.shape[2], clean.shape[3], dev)
    p = {"a": a[:, 1], "b": b[:, 1], "tukey": torch.zeros(n, dtype=torch.bool, device=dev),
         "lam": torch.zeros(n, device=dev), "rho_row": torch.zeros(n, device=dev), "rho_col": torch.zeros(n, device=dev),
         "hot_rate": torch.zeros(n, device=dev), "blur": torch.zeros(n, device=dev),
         "step": torch.full((n,), 1.0 / 15871.0, device=dev)}
    noisy, _ = ns.synthesize(ground + light, p, gen)
    return noisy, a, b, stars, light, ground


def flux_returned(x_hat, stars, light, ground):
    """Per brightness class: the flux the network returns over the flux that
    was there, both summed over a 5×5 G1 window at every star of the class
    (windows overlap in a dense field; both sums count the same neighbours)."""
    h, w = x_hat.shape[2:]
    box = lambda v: F.avg_pool2d(F.pad(v[:, 1:2], (2, 2, 2, 2), mode="reflect"), 5, stride=1)[:, 0] * 25.0
    got, was = box(x_hat - ground), box(light)
    i0 = torch.as_tensor(np.clip(np.round(stars["y"] / 2.0), 0, h - 1), device=x_hat.device).long()
    j0 = torch.as_tensor(np.clip(np.round((stars["x"] - 1.0) / 2.0), 0, w - 1), device=x_hat.device).long()
    sample = torch.as_tensor(stars["sample"], device=x_hat.device)
    inside = (i0 >= 4) & (i0 < h - 4) & (j0 >= 4) & (j0 < w - 4)
    snr = torch.as_tensor(stars["snr"], device=x_hat.device)
    report = {}
    for lo, hi in FLUX_CLASSES:
        pick = inside & (snr >= lo) & (snr < hi)
        report[f"{lo:g}-{hi:g}"] = (float(got[sample[pick], i0[pick], j0[pick]].sum() / was[sample[pick], i0[pick], j0[pick]].sum())
                                    if int(pick.sum()) else None)
    report["sky"] = float((x_hat[:, 1] - ground[:, 1] - light[:, 1]).mean() / 4.9e-4)
    return report


# ── training ─────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cache", default=str(REPO / "python" / "weights"))
    ap.add_argument("--iters", type=int, default=60000)
    ap.add_argument("--batch", type=int, default=8)
    ap.add_argument("--size", type=int, default=192)
    ap.add_argument("--lr", type=float, default=2e-5)
    ap.add_argument("--real-share", type=float, default=0.5)
    ap.add_argument("--val-every", type=int, default=2000)
    ap.add_argument("--val-crops", type=int, default=512)
    ap.add_argument("--seed", type=int, default=20260917)
    ap.add_argument("--dry", type=int, default=0, help="run this many iterations, report speed and memory, exit")
    ap.add_argument("--resume", default="")
    ap.add_argument("--stars", type=float, default=0.5, help="share of crops that get point sources; 0 is v1")
    ap.add_argument("--loss", choices=("l2x", "l1z"), default="l2x", help="l1z is v1's")
    args = ap.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    dev = "cuda"
    torch.manual_seed(args.seed)
    rng = np.random.default_rng(args.seed)
    gen = torch.Generator(device=dev)
    gen.manual_seed(args.seed)

    clean_user = Shards(pathlib.Path(args.data) / "clean_user", "clean")
    pairs_train = Shards(pathlib.Path(args.data) / "pairs", "pairs_train", with_ab=True)
    pairs_val = Shards(pathlib.Path(args.data) / "pairs", "pairs_val", with_ab=True)
    print(f"clean_user {len(clean_user)} crops, pairs train {len(pairs_train)}, val {len(pairs_val)}", flush=True)

    model = dr.load_model(args.cache, dev)
    # channels_last: 0.358 -> 0.238 s per step on the RTX 4060 Ti (measured 2026-09-17)
    torch.backends.cudnn.benchmark = True
    model = model.to(memory_format=torch.channels_last)
    model.train()
    for p in model.parameters():
        p.requires_grad_(True)
    opt = torch.optim.Adam(model.parameters(), lr=args.lr, betas=(0.9, 0.99))
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(opt, T_max=args.iters, eta_min=args.lr * 0.05)
    scaler = torch.amp.GradScaler("cuda")
    start = 0
    if args.resume:
        ck = torch.load(args.resume, map_location="cpu", weights_only=False)
        model.load_state_dict(ck["model"])
        opt.load_state_dict(ck["opt"])
        sched.load_state_dict(ck["sched"])
        start = ck["iter"]

    # fixed validation set: val pairs (held-out cameras/scenes)
    vrng = np.random.default_rng(12345)
    vidx = vrng.choice(len(pairs_val), size=min(args.val_crops, len(pairs_val)), replace=False)
    val = [pairs_val.get(int(k)) for k in vidx]
    flux = flux_field(torch.from_numpy(np.stack([c[0][4:] for c in val[:32]])).to(dev), dev)

    # noise-level bins by the green shot gain: the operator's camera spans
    # 3e-4 (ISO 1000) .. 1.9e-3 (ISO 6400)
    BINS = (("low", 0.0, 3e-4), ("mid", 3e-4, 1.5e-3), ("high", 1.5e-3, 1.0))

    def validate(tag, sigma_scale=1.0):
        """Held-out PSNR (linear and sqrt domain) of the network as the sidecar
        runs it. sigma_scale multiplies the noise channel: v1.4.1's shipped
        sidecar fed 0.85 / span, the fine-tune trains and validates at 1."""
        model.eval()
        rows = []
        with torch.no_grad():
            for k in range(0, len(val), 8):
                chunk = val[k:k + 8]
                xn = torch.from_numpy(np.stack([c[0][:4] for c in chunk])).to(dev)
                xc = torch.from_numpy(np.stack([c[0][4:] for c in chunk])).to(dev)
                ab = torch.from_numpy(np.stack([c[1] for c in chunk])).to(dev)
                a, b = ab[..., 0].clamp_min(1e-7), ab[..., 1].clamp_min(0)
                zn, tn, sigma, span = make_batch(xn, xc, a, b)
                with torch.autocast("cuda", dtype=torch.float16):
                    o = forward(model, zn, sigma * sigma_scale)
                zhat = merge_triplets(o.float(), zn.shape[0]) * span[:, None, None, None]
                xh = ns.igat(zhat, a[:, :, None, None], b[:, :, None, None]).clamp(0, 1)
                for i in range(xh.shape[0]):
                    c = xc[i].clamp(0, 1)
                    n = xn[i].clamp(0, 1)
                    row = {"a": float(a[i, 1])}
                    for key, arr in (("lin", xh[i]), ("noisy_lin", n)):
                        row[key] = 10 * math.log10(1.0 / max(float(((arr - c) ** 2).mean()), 1e-12))
                    for key, arr in (("sqrt", xh[i]), ("noisy_sqrt", n)):
                        row[key] = 10 * math.log10(1.0 / max(float(((arr.sqrt() - c.sqrt()) ** 2).mean()), 1e-12))
                    rows.append(row)
            noisy, fa, fb, stars, light, ground = flux
            zn, _, sigma, span = make_batch(noisy, ground + light, fa, fb)
            parts = []
            for k in range(0, noisy.shape[0], 8):
                with torch.autocast("cuda", dtype=torch.float16):
                    o = forward(model, zn[k:k + 8], sigma[k:k + 8] * sigma_scale)
                zhat = merge_triplets(o.float(), o.shape[0] // 2) * span[k:k + 8, None, None, None]
                parts.append(ns.igat(zhat, fa[k:k + 8, :, None, None], fb[k:k + 8, :, None, None]))
            returned = flux_returned(torch.cat(parts), stars, light, ground)
        model.train()
        r = {"tag": tag, "sigma_scale": sigma_scale, "n": len(rows), "flux": returned}
        for key in ("lin", "sqrt", "noisy_lin", "noisy_sqrt"):
            r[f"psnr_{key}"] = float(np.mean([x[key] for x in rows]))
        for name, lo, hi in BINS:
            sel = [x for x in rows if lo <= x["a"] < hi]
            r[f"sqrt_{name}"] = float(np.mean([x["sqrt"] for x in sel])) if sel else None
            r[f"n_{name}"] = len(sel)
        bins = " ".join(f"{nm} {r[f'sqrt_{nm}']:.3f}/{r[f'n_{nm}']}" if r[f"sqrt_{nm}"] is not None else f"{nm} -/0"
                        for nm, _, _ in BINS)
        print(f"[val {tag} s{sigma_scale}] PSNR lin {r['psnr_lin']:.3f} sqrt {r['psnr_sqrt']:.3f} "
              f"(noisy {r['psnr_noisy_lin']:.3f} / {r['psnr_noisy_sqrt']:.3f}) sqrt by level: {bins}", flush=True)
        print(f"[val {tag} s{sigma_scale}] star flux returned by G peak sigma: "
              + " ".join(f"{k} {v:.3f}" for k, v in returned.items() if v is not None and k != "sky")
              + f"; sky {returned['sky']:+.3f} photons", flush=True)
        return r

    history = []
    if start == 0 and not args.dry:
        history.append(validate("pretrained", 0.85))
        history.append(validate("pretrained", 1.0))
    best = max((h["psnr_sqrt"] for h in history), default=-1)
    t0 = time.time()
    S = args.size
    for it in range(start, args.iters):
        xs_n, xs_c, as_, bs_ = [], [], [], []
        n_real = sum(rng.random() < args.real_share for _ in range(args.batch)) if len(pairs_train) else 0
        for _ in range(n_real):
            arr, ab = pairs_train.get(int(rng.integers(len(pairs_train))))
            arr = random_crop(cfa_flip(arr, rng), S, rng)
            xs_n.append(arr[:4]); xs_c.append(arr[4:])
            as_.append(ab[:, 0]); bs_.append(ab[:, 1])
        n_syn = args.batch - n_real
        syn_clean = []
        for _ in range(n_syn):
            use_pair_ref = len(pairs_train) and rng.random() < 0.25
            if use_pair_ref:
                arr, _ = pairs_train.get(int(rng.integers(len(pairs_train))))
                arr = arr[4:]
            else:
                arr, _ = clean_user.get(int(rng.integers(len(clean_user))))
            arr = random_crop(cfa_flip(arr, rng), S, rng)
            g = math.exp(rng.uniform(math.log(0.25), 0.0))
            wb = np.array([math.exp(rng.uniform(-0.35, 0.35)), 1.0, 1.0, math.exp(rng.uniform(-0.35, 0.35))], np.float32)
            syn_clean.append(np.clip(arr * g * wb[:, None, None], 0.0, 1.0))
        if n_syn:
            xc_syn = torch.from_numpy(np.stack(syn_clean)).to(dev)
            p = ns.sample_params(n_syn, gen, dev)
            if args.stars > 0:
                xc_syn, _, _ = ps.add_to_pair(xc_syn, None, p["a"][:, None].expand(-1, 4), p["b"][:, None].expand(-1, 4),
                                              rng, gen, args.stars)
            xn_syn, b_seen = ns.synthesize(xc_syn, p, gen)
            a_est = p["a"] * torch.exp(torch.randn(n_syn, generator=gen, device=dev) * 0.03)
            b_est = b_seen * torch.exp(torch.randn(n_syn, generator=gen, device=dev) * 0.15)
        if n_real:
            xn_real = torch.from_numpy(np.stack(xs_n)).to(dev)
            xc_real = torch.from_numpy(np.stack(xs_c)).to(dev)
            a_real = torch.from_numpy(np.stack(as_)).to(dev).clamp_min(1e-7)
            b_real = torch.from_numpy(np.stack(bs_)).to(dev).clamp_min(0)
            if args.stars > 0:
                xc_real, xn_real, _ = ps.add_to_pair(xc_real, xn_real, a_real, b_real, rng, gen, args.stars)
        if n_real and n_syn:
            xn = torch.cat([xn_real, xn_syn]); xc = torch.cat([xc_real, xc_syn])
            a = torch.cat([a_real, a_est[:, None].expand(-1, 4)]); b = torch.cat([b_real, b_est[:, None].expand(-1, 4)])
        elif n_real:
            xn, xc, a, b = xn_real, xc_real, a_real, b_real
        else:
            xn, xc, a, b = xn_syn, xc_syn, a_est[:, None].expand(-1, 4), b_est[:, None].expand(-1, 4)
        a, b = a.contiguous(), b.contiguous()
        zn, tn, sigma, span = make_batch(xn, xc, a, b)
        with torch.autocast("cuda", dtype=torch.float16):
            o = forward(model, zn, sigma)
        if args.loss == "l1z":
            loss = (o.float() - to_triplets(tn)).abs().mean()
        else:
            loss = loss_mean_seeking(o.float(), span, xn, xc, a, b)
        opt.zero_grad(set_to_none=True)
        scaler.scale(loss).backward()
        scaler.unscale_(opt)
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        scaler.step(opt)
        scaler.update()
        sched.step()
        if (it + 1) % 100 == 0 or args.dry:
            el = time.time() - t0
            print(f"it {it + 1} loss {float(loss):.5f} lr {sched.get_last_lr()[0]:.2e} "
                  f"{(it + 1 - start) / el:.2f} it/s mem {torch.cuda.max_memory_allocated() / 2**30:.2f} GiB",
                  flush=True)
        if args.dry and it + 1 - start >= args.dry:
            return
        if (it + 1) % args.val_every == 0 or it + 1 == args.iters:
            r = validate(f"it{it + 1}")
            r["iter"] = it + 1
            history.append(r)
            ck = {"model": model.state_dict(), "opt": opt.state_dict(), "sched": sched.state_dict(), "iter": it + 1}
            torch.save(ck, out / "last.pt")
            if r["psnr_sqrt"] > best:
                best = r["psnr_sqrt"]
                torch.save(model.state_dict(), out / "best_state.pth")
            (out / "history.json").write_text(json.dumps(history, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
