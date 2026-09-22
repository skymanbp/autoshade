#!/usr/bin/env python3
"""Ground-truth benchmark of the two AI-denoise paths (2026-09-15).

A clean low-ISO RAW of the camera under test is given synthetic Poisson-
Gaussian noise at a per-plane model ``var = a·x + b`` — measured on a real
high-ISO frame of the same camera by the shipping sidecar's own estimator
(``--noisy``), or given outright (``--model``) — injected into the WHOLE
frame and quantised to the sensor's step, so the self-measurement runs where
the sidecar runs it. Windows are cut afterwards and every output is scored
against the clean render of the same window (bilinear demosaic, camera white
balance, sRGB): PSNR, PSNR on the 20 % most-detailed 64-px blocks (dPSNR) and
on the 20 % flattest (fPSNR), fine (5×5 high-pass) and mid (5–15 px) energy
kept in the detail blocks, residual fine energy in the flat blocks and the
chroma error ``hf((out − clean) R−G)``.

The two paths are the shipped ones, called through their modules:

* RAW mosaic — ``python/denoise_raw.py``: noise model self-measured on the
  noisy frame, packed (R,G1,B)/(R,G2,B) triplets, GAT, DRUNet-colour, exact
  unbiased inverse (strength 1.0, the RAW default).
* SCUNet — ``python/denoise.py`` on the rendered noisy window at strength
  1.0 and at its 0.5 default (the baked-source path).

Acceptance (the line this benchmark was introduced to guard): the RAW path's
dPSNR leads SCUNet 1.0 by at least ``--margin`` dB in every window. The
script exits 1 when it does not. Every path is a parameter: no photograph is
named here.

``--weights`` scores a candidate RAW-cleaner state dict in place of the
shipped one, which is how a fine-tune is held to "denoising itself must not
pay for it". It cannot be done by pointing ``--cache`` at another directory:
``denoise_raw.load_model`` verifies that file against a pinned sha256 and a
candidate has a different one. So the architecture and every executed file
still come from the verified cache, exactly as in production, and only the
tensor values are replaced — the same split ``denoise_flux_truth.py`` uses,
loaded with ``weights_only=True`` because a ``.pth`` is a pickle.

    python scripts/denoise_bench.py --clean CLEAN.ARW --noisy NOISY.ARW \
        --cache python/weights --out bench_out [--weights RUN/best_state.pth]
"""
import argparse
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
PY = os.path.join(os.path.dirname(HERE), "python")
sys.path.insert(0, PY)

import denoise as scunet  # noqa: E402  why: the sidecar package dir is only on sys.path after the insert above
import denoise_raw as dnraw  # noqa: E402  why: same — imports must follow the sys.path insert

PLANES = dnraw.PLANES


# ── RAW loading and rendering ───────────────────────────────────────────────

def load_normalised(path):
    """The visible mosaic in [0,1] per site, plus what a render needs."""
    import rawpy

    raw = rawpy.imread(path)
    mosaic = raw.raw_image_visible.astype(np.float32)
    pattern = raw.raw_pattern
    desc = raw.color_desc
    black = np.array(raw.black_level_per_channel, np.float32)
    white = float(raw.white_level)
    wb = np.array(raw.camera_whitebalance, np.float32)
    wbn = (wb / wb[1])[:3]
    x = np.empty_like(mosaic)
    for dy in range(2):
        for dx in range(2):
            ci = pattern[dy, dx]
            x[dy::2, dx::2] = (mosaic[dy::2, dx::2] - black[ci]) / (white - black[ci])
    letters = "".join(chr(desc[pattern[dy, dx]]) for dy in range(2) for dx in range(2))
    return np.clip(x, 0, 1), letters, wbn, 1.0 / (white - black[0])


def bilinear_demosaic(mosaic, letters):
    """Plain bilinear demosaic of a [0,1] Bayer mosaic → HxWx3 RGB."""
    import cv2

    h, w = mosaic.shape
    out = np.zeros((h, w, 3), np.float32)
    k_rb = np.array([[0.25, 0.5, 0.25], [0.5, 1.0, 0.5], [0.25, 0.5, 0.25]], np.float32)
    k_g = np.array([[0.0, 0.25, 0.0], [0.25, 1.0, 0.25], [0.0, 0.25, 0.0]], np.float32)
    for i, letter in enumerate(letters):
        dy, dx = i // 2, i % 2
        ch = "RGB".index(letter)
        mask = np.zeros((h, w), np.float32)
        mask[dy::2, dx::2] = 1.0
        k = k_g if letter == "G" else k_rb
        out[:, :, ch] += cv2.filter2D(mosaic * mask, -1, k, borderType=cv2.BORDER_REFLECT)
    return out


def srgb(x):
    x = np.clip(x, 0, 1)
    return np.where(x <= 0.0031308, 12.92 * x, 1.055 * np.power(x, 1 / 2.4) - 0.055).astype(np.float32)


def render(mosaic, letters, wbn):
    return srgb(bilinear_demosaic(mosaic, letters) * wbn)


# ── Metrics ─────────────────────────────────────────────────────────────────

def luma(rgb):
    return 0.2126 * rgb[:, :, 0] + 0.7152 * rgb[:, :, 1] + 0.0722 * rgb[:, :, 2]


def hf(x):
    import cv2

    return x - cv2.blur(x, (5, 5))


def mid(x):
    import cv2

    return cv2.blur(x, (5, 5)) - cv2.blur(x, (15, 15))


def blocks(a, size):
    h, w = a.shape[0] // size * size, a.shape[1] // size * size
    return a[:h, :w].reshape(h // size, size, w // size, size).transpose(0, 2, 1, 3)


def block_std(a, size=64):
    return blocks(a, size).std(axis=(2, 3))


def ssim_gray(a, b):
    import cv2

    c1, c2 = 0.01 ** 2, 0.03 ** 2
    g = lambda x: cv2.GaussianBlur(x, (11, 11), 1.5)
    mu_a, mu_b = g(a), g(b)
    va, vb, vab = g(a * a) - mu_a ** 2, g(b * b) - mu_b ** 2, g(a * b) - mu_a * mu_b
    s = ((2 * mu_a * mu_b + c1) * (2 * vab + c2)) / ((mu_a ** 2 + mu_b ** 2 + c1) * (va + vb + c2))
    return float(s.mean())


def psnr_of(mse):
    return float(10 * np.log10(1.0 / max(mse, 1e-12)))


def report(clean, out):
    lc, lo = luma(clean), luma(out)
    bc = block_std(hf(lc))
    detail = bc >= np.percentile(bc, 80)
    flat = bc <= np.percentile(bc, 20)
    bo = block_std(hf(lo))
    mc, mo = block_std(mid(lc)), block_std(mid(lo))
    err = blocks(((clean - out) ** 2).mean(axis=2), 64).mean(axis=(2, 3))
    return dict(
        psnr=psnr_of(float(err.mean())),
        ssim=ssim_gray(lc, lo),
        dpsnr=psnr_of(float(err[detail].mean())),
        fpsnr=psnr_of(float(err[flat].mean())),
        fine=bo[detail].mean() / bc[detail].mean(),
        midb=mo[detail].mean() / mc[detail].mean(),
        noise=bo[flat].mean() / bc[flat].mean(),
        cerr=hf((out[:, :, 0] - clean[:, :, 0]) - (out[:, :, 1] - clean[:, :, 1])).std() * 255,
    )


# ── The two paths ───────────────────────────────────────────────────────────

def measure(x, phases):
    """The sidecar's own estimator on a [0,1] mosaic → {plane: (a, b)}."""
    return dnraw.noise_model(dnraw.split_planes(x, phases))


def denoise_raw_window(model, xw, phases, ab, device, tile, overlap):
    planes = dnraw.split_planes(xw, phases)
    den = dnraw.denoise_planes(model, planes, ab, device, tile, overlap, False)
    return dnraw.merge_planes(den, phases, xw.shape, xw)


def denoise_scunet_window(model, noisy_rgb, device, strength, tile, overlap):
    den = scunet.denoise(model, noisy_rgb, device, tile=tile, overlap=overlap, fp16=False)
    return scunet.blend_luma_chroma(den, noisy_rgb, strength)


def save16(path, rgb):
    import cv2

    cv2.imwrite(path, (np.clip(rgb, 0, 1)[:, :, ::-1] * 65535 + 0.5).astype(np.uint16))


def parse_pairs(text):
    """'a,b;a,b;…' → a list of (a, b) tuples."""
    pairs = []
    for item in text.split(";"):
        a, b = item.split(",")
        pairs.append((float(a), float(b)))
    return pairs


def parse_model(text):
    """One 'a,b' pair for every plane, or one per plane in PLANES order."""
    pairs = parse_pairs(text)
    if len(pairs) == 1:
        pairs = pairs * len(PLANES)
    if len(pairs) != len(PLANES):
        raise SystemExit(f"--model: give one a,b pair or one per plane ({len(PLANES)})")
    return dict(zip(PLANES, pairs))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--clean", required=True, help="a clean low-ISO RAW (the ground truth)")
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--noisy", help="a high-ISO RAW of the same camera whose noise model to inject")
    src.add_argument("--model", help="the noise model to inject: 'a,b' or four 'a,b' pairs (R;G1;G2;B) in [0,1] units")
    ap.add_argument("--levels", default="1.0,1.0",
                    help="multipliers 'ka,kb;…' applied to the model, one benchmark level each (default 1,1)")
    ap.add_argument("--windows", default="2000,4000;4000,2000",
                    help="'row,col;…' top-left corners of the windows, default two")
    ap.add_argument("--window-size", type=int, default=2048)
    ap.add_argument("--cache", default=os.path.join(PY, "weights"), help="the sidecars' weight cache")
    ap.add_argument("--weights", help="a candidate RAW-cleaner state dict to score in place of the shipped one")
    ap.add_argument("--out", default="denoise_bench_out", help="where the PNG outputs go")
    ap.add_argument("--margin", type=float, default=1.5,
                    help="the RAW path must lead SCUNet 1.0 on dPSNR by this many dB in every window")
    ap.add_argument("--tile", type=int, default=512)
    ap.add_argument("--overlap", type=int, default=32)
    ap.add_argument("--seed", type=int, default=20260915)
    ap.add_argument("--cpu", action="store_true")
    args = ap.parse_args()

    t0 = time.time()
    os.makedirs(args.out, exist_ok=True)
    x, letters, wbn, step = load_normalised(args.clean)
    phases = dnraw.parse_pattern(letters)
    print(f"clean {x.shape[1]}x{x.shape[0]} pattern {letters} WB {wbn.round(3).tolist()}")
    own = measure(x, phases)
    if args.noisy:
        xn_ref, letters_n, _, _ = load_normalised(args.noisy)
        if letters_n != letters:
            raise SystemExit(f"--noisy is {letters_n}, --clean is {letters}: not the same sensor layout")
        base = measure(xn_ref, phases)
        del xn_ref
    else:
        base = parse_model(args.model)
    for n in PLANES:
        print(f"  model {n}: a {base[n][0]:.3e} b {base[n][1]:.3e} (clean frame's own a {own[n][0]:.3e})")

    device = "cpu" if args.cpu else dnraw.pick_device(False, "cuda:0")
    raw_model = dnraw.load_model(args.cache, device)
    if args.weights:
        import torch

        raw_model.load_state_dict(torch.load(args.weights, map_location="cpu", weights_only=True), strict=True)
        raw_model = raw_model.eval().to(device)
        print(f"RAW cleaner: candidate {os.path.basename(args.weights)} in the shipped architecture")
    scu_model = scunet.load_model("color_real_psnr", args.cache, device)
    windows = [tuple(int(v) for v in w.split(",")) for w in args.windows.split(";")]
    size = args.window_size
    failures = []

    for li, (ka, kb) in enumerate(parse_pairs(args.levels)):
        rng = np.random.default_rng(args.seed)
        truth = {n: (base[n][0] * ka, base[n][1] * kb) for n in PLANES}
        xn = np.empty_like(x)
        for n, (dy, dx) in phases.items():
            a6, b6 = truth[n]
            a1, b1 = own[n]
            xc = x[dy::2, dx::2]
            var = np.clip((a6 - a1) * xc + (b6 - max(b1, 0.0)), 0, None)
            xn[dy::2, dx::2] = xc + rng.normal(0.0, 1.0, var.shape).astype(np.float32) * np.sqrt(var)
        xn = np.clip(np.round(xn / step) * step, 0, 1).astype(np.float32)
        est = {n: (a, max(b, 0.0)) for n, (a, b) in measure(xn, phases).items()}
        lname = f"level{li}"
        print(f"\n===== {lname} (a×{ka}, b×{kb}); self-measured on the noisy frame:")
        for n in PLANES:
            print(f"    {n}: a {est[n][0]:.3e} = {est[n][0] / truth[n][0] * 100:.0f} % of truth, "
                  f"b {est[n][1]:.3e} vs {truth[n][1]:.3e}")
        for wi, (r0, c0) in enumerate(windows):
            r1, c1 = r0 + size, c0 + size
            wname = f"w{wi}"
            xc, xw = x[r0:r1, c0:c1], xn[r0:r1, c0:c1]
            clean, noisy = render(xc, letters, wbn), render(xw, letters, wbn)
            outs = {"noisy": noisy}
            xo = denoise_raw_window(raw_model, xw, phases, est, device, args.tile, args.overlap)
            outs["RAW DRUNet 1.0"] = render(xo, letters, wbn)
            for s in (1.0, 0.5):
                outs[f"SCUNet {s:.1f}"] = denoise_scunet_window(scu_model, noisy, device, s, args.tile, args.overlap)
            print(f"[{lname}/{wname}] {'output':16s} {'PSNR':>6s} {'SSIM':>6s} {'dPSNR':>6s} {'fPSNR':>6s} "
                  f"{'fineKept':>9s} {'midKept':>8s} {'flatHF':>7s} {'chromaErr':>9s}")
            scores = {}
            for k, v in outs.items():
                m = report(clean, v)
                scores[k] = m
                print(f"[{lname}/{wname}] {k:16s} {m['psnr']:6.2f} {m['ssim']:6.4f} {m['dpsnr']:6.2f} "
                      f"{m['fpsnr']:6.2f} {m['fine'] * 100:8.1f}% {m['midb'] * 100:7.1f}% "
                      f"{m['noise'] * 100:6.0f}% {m['cerr']:9.2f}")
                save16(os.path.join(args.out, f"{lname}_{wname}_{k.replace(' ', '_')}.png"), v)
            save16(os.path.join(args.out, f"{lname}_{wname}_clean.png"), clean)
            lead = scores["RAW DRUNet 1.0"]["dpsnr"] - scores["SCUNet 1.0"]["dpsnr"]
            verdict = "PASS" if lead >= args.margin else "FAIL"
            print(f"[{lname}/{wname}] RAW path leads SCUNet 1.0 on dPSNR by {lead:+.2f} dB "
                  f"(margin {args.margin:.1f}) → {verdict} ({time.time() - t0:.0f}s)")
            if verdict == "FAIL":
                failures.append(f"{lname}/{wname}: {lead:+.2f} dB")
    if failures:
        print("FAIL: " + "; ".join(failures))
        sys.exit(1)
    print("PASS: the RAW path leads SCUNet 1.0 on the detail blocks in every window")


if __name__ == "__main__":
    main()
