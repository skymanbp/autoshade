#!/usr/bin/env python3
"""AutoShade RAW-domain AI-denoise sidecar.

Denoises the sensor MOSAIC — before demosaic, before white balance, before
any tone work — the way Lightroom's Denoise does, and unlike `denoise.py`,
which runs a BLIND model on the demosaiced, gamma-encoded frame. Measured
on the user's ILCE-7RM4A frames (2026-09-15): the noise on the mosaic is
white per CFA plane (box-5 / Haar sigma ratio 0.975–0.995) and follows
var = a·x + b; after demosaic it is spatially correlated and no sRGB-domain
model, blind or not, separates it from texture. A ground-truth benchmark
(an ISO-100 frame + synthetic noise at the ISO-640 model this estimator
measured on a real frame; `scripts/denoise_bench.py`) put this path
3.2–4.9 dB above SCUNet 1.0 on the whole frame and 3.1–5.9 dB above it on
the most-detailed blocks; SCUNet 1.0's detail blocks were BELOW the noisy
input on one window.

Contract with the Rust side (`denoise::denoise_mosaic`):
    python -E denoise_raw.py --input mosaic.png --output staged.png \
        --pattern RGGB --black b00,b01,b10,b11 --white W --strength 0..1 \
        [--cache DIR] [--tile 512] [--overlap 32] [--fp16] [--cpu]
  * ONE 16-bit GRAYSCALE PNG in: the uncropped sensor mosaic, u16 values as
    decoded. ONE 16-bit grayscale PNG out: the denoised mosaic, identical
    dimensions and dtype. No colour image, so no channel-order question.
  * `--pattern`: the 2×2 CFA letters at mosaic (0,0), (0,1), (1,0), (1,1) —
    exactly one R, one B and two G, or the run is refused.
  * `--black`: four values in that same order (a per-frame level is repeated
    four times by the caller); `--white`: one value.
  * `--strength`: the standalone sidecar keeps its per-plane RAW blend —
    0 reproduces input bytes, 1 is the complete model output (default).
    Rust never spawns for 0 and requests 1 for every positive RAW dial;
    the renderer applies that dial as a neutral luminance return AFTER
    demosaic and calibration, in linear light.

Method (every step measured in the 2026-09-15 probe, none assumed):
  1. Normalise per phase, x = (v - black[phase]) / (white - black[phase]),
     split into the four half-resolution planes R, G1, G2, B. Only the TOP is
     bounded: a sample below the black level is real sensor data — 14–22 % of
     a 15 s ISO-8000 frame — and clamping it at 0 rectifies the noise. Measured
     2026-09-17, that clamp inflated the fitted a by 3–19 % and left the
     darkest band carrying 0.39–0.65 of the unit variance step 3 promises the
     model, so the model was told the shadows were 1.5–2.5× noisier than they
     look and smoothed them accordingly. The GAT has its own floor (its
     radicand is clipped at 0) and takes over from here.
  2. NOISE MODEL, self-measured on the whole frame per plane: 32×32 blocks;
     per block the mean, the white-noise variance (finest-scale Haar diagonal
     detail, MAD / 0.6745, squared) and the box-5 high-pass variance; a block
     is textureless when box5 var <= 1.25 × 0.96 × Haar var (0.96 is the
     box-5 residual of white noise) and its mean sits in (0.002, 0.9);
     least squares var = a·x + b on those blocks, then `physical_model` — a
     sensor has neither a negative read-noise floor nor a variance that falls
     as signal rises, and an a <= 0 is fatal to step 3's radicand.
  3. Generalized Anscombe transform per plane, z = 2·sqrt(x/a + 3/8 + b/a²),
     which leaves unit-variance noise; ONE shared affine puts all four planes
     in [0,1], so the model's sigma is one number. The affine is read off the
     NOISE MODEL — 0, the GAT's own floor, to max gat(1), the largest z any
     plane can reach — never off percentiles of this frame's data, so no real
     sample can fall outside it. `SIGMA_SCALE` × its slope is the sigma.
  4. DRUNet-colour (KAIR's architecture, non-blind: the sigma rides in as a
     4th channel) on two triplets, (R, G1, B) and (R, G2, B); R and B are the
     mean of their two estimates, G1 / G2 come from their own triplet. Since
     v1.5.0 the WEIGHTS are AutoShade's own: DPIR's released `drunet_color`
     fine-tuned on this exact pipeline — real RawNIND pairs and synthetic
     sensor noise over the operator's own low-ISO frames, the loss taken in
     the stabilised z domain the model actually sees (`scripts/train_raw.py`).
     It is trained for THIS transform, so it beats the generic weights by
     1.98 dB on held-out pairs at the former 0.78 operating point and by 7.8 dB on
     the noisiest bin at its own.
  5. Exact unbiased inverse of the GAT (Mäkitalo & Foi 2013): with D the
     denormalised z, I_A(D) = ¼D² + ¼√(3/2)·D⁻¹ − 11/8·D⁻² + 5/8√(3/2)·D⁻³ − 1/8
     and x̂ = a·(I_A(D) − b/a²).
  6. out = x + s·(x̂ − x); re-quantise with the phase's black level, clip to
     [0, white], reassemble, publish (tmp + fsync + os.replace). Below-black
     samples are data, not a reason to rectify returned or inferred noise.

Every downloaded file — the weights and the two network files — is fetched
through `denoise._fetch_verified` and verified against a pinned sha256 and
byte count BEFORE it is executed or unpickled. KAIR's architecture files are
MIT; the weights are AutoShade's, released under the project's own licence,
and the pairs they were fine-tuned on come from RawNIND (Brummer & De Vleeschouwer,
CC BY-SA 4.0) — credited in `docs/TECH_STACK.md` with the training recipe.

Exit code 0 on success; 2 with one sentence on stderr for a refusal.
"""
import argparse
import importlib.util
import os
import sys
import types
import warnings

# The shared device rule and the shared sidecar plumbing, beside this script
# in `python/` (see `_device.py` for why that directory is trusted).
from _device import pick_device

warnings.filterwarnings("ignore")  # requests/urllib3 version warnings only

import numpy as np

import _sidecar
from denoise import _fetch_verified, _tile_window

TAG = "denoise_raw"

_KAIR_RELEASE = "https://github.com/cszn/KAIR/releases/download/v1.0"
_KAIR_RAW = "https://raw.githubusercontent.com/cszn/KAIR"
# The fine-tuned weights ride with the release that introduced them, so a
# given AutoShade always fetches the network it was measured with.
_AUTOSHADE_RELEASE = "https://github.com/skymanbp/autoshade/releases/download/v1.5.0"
# PINNED to immutable commits (the last commit that touched each file, read
# from the GitHub commits API on 2026-09-15) — `network_unet.py` is EXECUTED
# and `basicblock.py` is imported by it, so a branch name here would mean
# "run whatever upstream has at download time, as the user".
NETWORK_COMMIT = "345c87f8364322c40eef52e575f98af893f04126"
BASICBLOCK_COMMIT = "5d55a5fb88d20eb811dc7ccf6342b921039191cf"
# sha256 + exact byte count of every file this sidecar downloads, verified
# 2026-09-15 (the two architecture files) and 2026-09-17 (the weights) by
# fetching each at its pinned commit / release asset and hashing the bytes.
# The weights are a PICKLE handed to torch, so the CHANNEL is authenticated
# here and the loader is flagged below (weights_only).
#
# `drunet_color.pth` (sha256 479abe3c…, 130,579,305 B) is no longer fetched:
# it is what the fine-tune STARTED from, and every one of its 32,640,960
# parameters moved, so the release below is self-contained and the sidecar
# downloads 130 MB once instead of twice.
PINS = {
    "autoshade-raw-denoise-v1.pth": {
        "url": f"{_AUTOSHADE_RELEASE}/autoshade-raw-denoise-v1.pth",
        "sha256": "6929ddd6b11b3f27baf3537d92a4552a6a5c39d53ff4167e4f7df26e80413a99",
        "bytes": 130585417,
    },
    "network_unet.py": {
        "url": f"{_KAIR_RAW}/{NETWORK_COMMIT}/models/network_unet.py",
        "sha256": "8043b6350f1589d5f08892e3be0b4d12c5a502058014285107b7360696d12bf5",
        "bytes": 3484,
    },
    "basicblock.py": {
        "url": f"{_KAIR_RAW}/{BASICBLOCK_COMMIT}/models/basicblock.py",
        "sha256": "48406db8867394ac5ae233ebeec7711ac10acfc3a6bbf0072c33aa77d659b6fd",
        "bytes": 24138,
    },
}

# The noise-model estimator's block geometry and admission (step 2 above).
BLOCK = 32
# A box-5 residual of WHITE noise has 0.96 of the noise variance; a block whose
# box-5 variance exceeds that by more than 25 % carries texture.
WHITE_BOX5 = 0.96
TEXTURELESS_SLACK = 1.25
MIN_BLOCKS_PER_PLANE = 50
MIN_BLOCKS_POOLED = 20

# One z-unit of the stabilised frame IS one sigma (step 3), so the affine's
# slope is the noise and this is what the model is TOLD that noise is, as a
# fraction of the measured one.
#
# RE-MEASURED 2026-09-17 for the fine-tuned weights, because they are a much
# stronger denoiser at the same estimate than the generic ones v1.4.1 shipped:
# the operating point is a property of the network, not a constant, so it was
# chosen again from the same four measurements (v1.4.1's own table for the
# generic weights is in that release's history).
#
#   * 15 Lightroom pairs (`scripts/lr_realset.py`): the residual grain of our
#     output over Lightroom's own Enhance→Denoise layer, median over frames
#     and planes, in a flat / a detailed / a centre window;
#   * one ISO-8000 astro frame: the same grain ratio in the flat sky, and the
#     share of the input's faint and very-faint star flux still standing;
#   * 512 held-out RawNIND crops, PSNR in the sqrt domain;
#   * `scripts/denoise_bench.py`, dPSNR over SCUNet 1.0 on the two detail
#     windows of its ×5 synthetic level (the bar is +1.5).
#
#   scale   LR 15 frames    stars: sky / faint / v.faint   held-out   bench ×5
#   1.00    0.03/0.11/0.06  0.03×   58 %  24 %             46.95      +2.02 +2.77
#   0.85    0.68/0.55/0.65  0.21×   64 %  32 %             45.95      +1.55 +2.31
#   0.78    1.06/1.01/1.01  0.53×   69 %  40 %             44.68      +0.81 +1.72
#   0.72    1.42/1.29/1.32  0.97×   73 %  48 %             43.10      +0.08 +1.17
#   Lightroom itself: 1.00 everywhere, and it keeps 71 % / 43 % of the stars.
#   The generic weights at 0.85 (v1.4.1): 0.58/0.71/0.73, 0.59×, 71 % / 47 %,
#   42.71, +0.53 +1.60.
#
# Historical 0.78 point (2026-09-17): it matched the window-based Lightroom
# texture measurements above by under-telling sigma. Full-frame measurements
# on 2026-09-20 instead found residual grain 0.42 / 0.25 / 0.11 across three
# astro noise levels, against Lightroom's 0.28–0.30. Honest sigma leaves
# 0.07 / 0.02 / 0.01. The 1.00 row remains the held-out PSNR record: neither
# the network nor its transform changed, and the shards are no longer local.
# The cleaner now receives its training noise level. Rust always asks for
# its whole output, demosaics and calibrates both frames identically, then
# returns only the original's linear-light luminance residual along RGB grey.
# A CFA-quad return was measured and withdrawn: demosaic turned its fine
# neutral grain into colour speckle. Grain return therefore lives AFTER it.
SIGMA_SCALE = 1.0


def log(msg):
    _sidecar.log(TAG, msg)


def die(msg):
    _sidecar.die(TAG, msg)


# ── CFA packing ─────────────────────────────────────────────────────────────

PLANES = ("R", "G1", "G2", "B")


def parse_pattern(pattern):
    """The 2×2 letters → {plane name: (row phase, col phase)}; refuses anything
    that is not exactly one R, one B and two G (X-Trans, four-colour and
    monochrome sensors have no such packing)."""
    p = (pattern or "").strip().upper()
    # One R, one B, two G — and the two G on a diagonal, which is the only
    # arrangement a Bayer filter has (RGGB, BGGR, GRBG, GBRG).
    if len(p) != 4 or sorted(p) != ["B", "G", "G", "R"] or not (
        (p[0] == "G" and p[3] == "G") or (p[1] == "G" and p[2] == "G")
    ):
        die(f"--pattern {pattern!r} is not a 2x2 Bayer pattern (one R, one B, two G on a diagonal)")
    phases = {}
    g = 0
    for i, letter in enumerate(p):
        pos = (i // 2, i % 2)
        if letter == "G":
            g += 1
            phases[f"G{g}"] = pos
        else:
            phases[letter] = pos
    return phases


def split_planes(mosaic, phases):
    """The four half-resolution planes of the even-sized region of `mosaic`."""
    h2, w2 = mosaic.shape[0] // 2 * 2, mosaic.shape[1] // 2 * 2
    return {n: mosaic[dy:h2:2, dx:w2:2] for n, (dy, dx) in phases.items()}


def merge_planes(planes, phases, shape, border):
    """Reassemble; rows/cols beyond the even region come from `border`."""
    out = border.copy()
    h2, w2 = shape[0] // 2 * 2, shape[1] // 2 * 2
    for n, (dy, dx) in phases.items():
        out[dy:h2:2, dx:w2:2] = planes[n]
    return out


# ── Noise model ─────────────────────────────────────────────────────────────

def _blocks(a, b):
    bh, bw = a.shape[0] // b, a.shape[1] // b
    return a[: bh * b, : bw * b].reshape(bh, b, bw, b).transpose(0, 2, 1, 3)


def _haar_diag(x):
    """Finest-scale Haar diagonal detail: white noise of std s has std s here."""
    h, w = x.shape[0] // 2 * 2, x.shape[1] // 2 * 2
    x = x[:h, :w]
    return (x[0::2, 0::2] - x[0::2, 1::2] - x[1::2, 0::2] + x[1::2, 1::2]) / 2.0


def _box5_hf(x):
    import cv2

    return x - cv2.blur(x, (5, 5))


def block_statistics(plane):
    """Per 32×32 block of a normalised plane: (mean, white-noise variance,
    box-5 high-pass variance)."""
    m = _blocks(plane, BLOCK).mean(axis=(2, 3))
    d = _blocks(_haar_diag(plane), BLOCK // 2)
    med = np.median(d, axis=(2, 3), keepdims=True)
    mad = np.median(np.abs(d - med), axis=(2, 3)) / 0.6745
    bv = _blocks(_box5_hf(plane), BLOCK).var(axis=(2, 3))
    return m, mad ** 2, bv


def textureless(m, wv, bv):
    return (bv <= TEXTURELESS_SLACK * WHITE_BOX5 * wv) & (m > 0.002) & (m < 0.9)


# Relative sampling spread of one block's white-noise variance estimate: a
# 32×32 block holds 256 finest-scale Haar coefficients, and a MAD-based
# variance over 256 samples scatters by about 12 % of its value.
BLOCK_VARIANCE_SPREAD = 0.12


def fit_ab(means, variances):
    """var = a·x + b by least squares with iterative outlier rejection: a
    block whose variance sits more than three sampling spreads ABOVE the fit
    carries texture the admission rule let through (pixel-scale grain reads
    as white noise to every local estimator), and noise only ever sets the
    floor. Two passes settle it. `physical_model` then has the last word: least
    squares can return a model no sensor could have, and the transform below
    cannot use one."""
    m = np.asarray(means, np.float64)
    v = np.asarray(variances, np.float64)
    keep = np.ones(len(m), bool)
    a = b = 0.0
    for _ in range(3):
        A = np.stack([m[keep], np.ones(keep.sum())], axis=1)
        (a, b), *_ = np.linalg.lstsq(A, v[keep], rcond=None)
        expected = np.clip(a * m + b, 1e-12, None)
        above = (v - expected) > 3.0 * BLOCK_VARIANCE_SPREAD * expected
        refined = keep & ~above
        if refined.sum() < max(MIN_BLOCKS_POOLED, 2) or np.array_equal(refined, keep):
            break
        keep = refined
    return physical_model(float(a), float(b), m[keep])


# When the admitted blocks share one signal level — a flat sky, a flat-field,
# a frame lit to one tone — the least-squares SLOPE is unidentifiable and comes
# back negative as often as not. Half the variance is then attributed to shot
# noise and half to the floor: a prior, and stated as one, but a bounded one
# (both terms stay within a factor of two of the truth whatever the real split
# was), and the alternative is worse in every direction.
UNIDENTIFIABLE_SHOT_SHARE = 0.5


def physical_model(a, b, means):
    """Force `var = a·x + b` to be a noise model a sensor could have.

    `b < 0` is a negative read-noise floor; `a < 0` says variance FALLS as
    signal rises. Neither is physical, and `a <= 0` is not merely wrong but
    fatal: `gat`'s radicand `x/a + 3/8 + b/a²` then goes negative for the
    BRIGHT samples, so every highlight transforms to 0 and comes back inverted.
    On a synthetic flat sky the fit returns a = -3.9e-04 (2026-09-17).

    What survives an unidentifiable slope is the variance AT the blocks' own
    signal level — measured 1.01× the truth on that same fixture — so that is
    what is kept, split by `UNIDENTIFIABLE_SHOT_SHARE`. A frame whose fit is
    genuinely shot-dominated is untouched: it already has a > 0."""
    if a > 0.0:
        return a, max(b, 0.0)
    x0 = float(np.mean(means)) if len(means) else 0.0
    var0 = max(a * x0 + b, 0.0)
    if x0 <= 0.0 or var0 <= 0.0:
        # No usable level either: leave a floor-only model, which the GAT
        # handles (it degenerates towards the linear, constant-variance map).
        return 1e-6, max(b, 0.0)
    return UNIDENTIFIABLE_SHOT_SHARE * var0 / x0, (1.0 - UNIDENTIFIABLE_SHOT_SHARE) * var0


def noise_model(planes):
    """{plane: (a, b)} from the whole frame. Planes with too few textureless
    blocks borrow the pooled fit; a frame with too few in total is refused."""
    samples = {}
    for name, plane in planes.items():
        m, wv, bv = block_statistics(plane.astype(np.float32, copy=False))
        keep = textureless(m, wv, bv)
        samples[name] = (m[keep], wv[keep])
    pooled_m = np.concatenate([s[0] for s in samples.values()])
    pooled_v = np.concatenate([s[1] for s in samples.values()])
    if len(pooled_m) < MIN_BLOCKS_POOLED:
        die(
            f"the frame has too little textureless area to measure its noise "
            f"({len(pooled_m)} usable {BLOCK}x{BLOCK} blocks across the four planes)"
        )
    pooled = fit_ab(pooled_m, pooled_v)
    model = {}
    for name, (m, v) in samples.items():
        if len(m) >= MIN_BLOCKS_PER_PLANE:
            model[name] = fit_ab(m, v)
            log(f"plane {name}: a={model[name][0]:.4e} b={model[name][1]:.3e} blocks={len(m)}")
        else:
            model[name] = pooled
            log(f"plane {name}: {len(m)} textureless blocks — using the pooled fit "
                f"a={pooled[0]:.4e} b={pooled[1]:.3e}")
    return model


# ── Variance stabilisation ──────────────────────────────────────────────────

def gat(x, a, b):
    """Generalized Anscombe transform: unit-variance noise for var = a·x + b."""
    return 2.0 * np.sqrt(np.clip(x / a + 3.0 / 8.0 + b / (a * a), 0.0, None))


def igat(z, a, b):
    """Exact unbiased inverse of `gat` — Mäkitalo & Foi's closed form for the
    Anscombe inverse (2011), shifted by −b/a² for the generalized transform
    (2013). Applied to E[z] it returns E[x]; on a NOISELESS value it sits a
    quarter count (a/4 in x units) high, which is its documented bias."""
    z = np.clip(z, 1e-3, None)
    ia = (0.25 * z ** 2 + 0.25 * np.sqrt(1.5) / z - (11.0 / 8.0) / z ** 2
          + (5.0 / 8.0) * np.sqrt(1.5) / z ** 3 - 1.0 / 8.0)
    return a * (ia - b / (a * a))


def model_affine(ab):
    """The shared affine that carries the four stabilised planes into the
    model's [0,1] — `(lo, span)`, from the NOISE MODEL alone.

    It takes no pixels ON PURPOSE. Deriving it from the frame's data was the
    defect this function exists to make impossible: percentiles put the top at
    the 99.95th, which is BELOW the brightest samples of any frame whose bright
    content is rarer than 0.05 % of its pixels, and everything above came back
    as the single value igat(top). `gat` is monotone in x and floors its
    radicand at 0, so [0, max gat(1)] brackets every sample any plane can hold
    and nothing can fall outside it."""
    lo = 0.0
    hi = max(float(gat(1.0, *ab[n])) for n in PLANES)
    return lo, max(hi - lo, 1e-6)


# ── Model ───────────────────────────────────────────────────────────────────

def fetch_pinned(name, cache_dir):
    pin = PINS[name]
    dest = os.path.join(cache_dir, name)
    # The small slack on the cap keeps an overshoot message about the
    # ENDPOINT, not an off-by-one (the same rule denoise.py applies).
    _fetch_verified(pin["url"], dest, pin["sha256"], pin["bytes"] + 4096, f"the DRUNet '{name}'")
    return dest


def _load_verified_module(qualname, path):
    spec = importlib.util.spec_from_file_location(qualname, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[qualname] = mod
    spec.loader.exec_module(mod)
    return mod


def load_model(cache_dir, device):
    """AutoShade's RAW denoiser: KAIR's DRUNet-colour architecture carrying
    OUR fine-tuned weights, every file verified before it is executed or
    unpickled."""
    import torch

    os.makedirs(cache_dir, exist_ok=True)
    basicblock = fetch_pinned("basicblock.py", cache_dir)
    network = fetch_pinned("network_unet.py", cache_dir)
    weights = fetch_pinned("autoshade-raw-denoise-v1.pth", cache_dir)
    # `network_unet.py` says `import models.basicblock as B`: a synthetic
    # `models` package satisfies the import from the VERIFIED file, so the
    # upstream text is never rewritten and no other `models` package on the
    # path is consulted.
    pkg = types.ModuleType("models")
    pkg.__path__ = []
    sys.modules["models"] = pkg
    _load_verified_module("models.basicblock", basicblock)
    unet = _load_verified_module("models.network_unet", network)
    # bias=False: neither DPIR's released weights nor the fine-tune of them
    # carries bias tensors (a strict load with bias=True lists every `*.bias`
    # as missing), and the fine-tune kept the architecture byte for byte so
    # that this file — the one that is EXECUTED — never had to change.
    model = unet.UNetRes(in_nc=4, out_nc=3, nc=[64, 128, 256, 512], nb=4, act_mode="R",
                         downsample_mode="strideconv", upsample_mode="convtranspose", bias=False)
    # weights_only: a .pth is a PICKLE; the safe loader suffices for a state
    # dict and a torch too old to know the flag is refused, not degraded.
    try:
        state = torch.load(weights, map_location="cpu", weights_only=True)
    except TypeError as e:
        raise SystemExit(
            f"refusing to load the denoiser's weights: this torch ({torch.__version__}) predates "
            f"weights_only=True, so the load would execute the weight pickle unsandboxed. "
            f"Upgrade torch and retry. ({e})"
        )
    model.load_state_dict(state, strict=True)
    return model.eval().to(device)


def run_tiled(model, rgb, sigma, device, tile=512, overlap=32, fp16=False):
    """rgb: HxWx3 float32 in [0,1]; sigma: the model's noise level, one
    number. The sidecar's own tiling (re-anchored edge tiles, feathered
    window) around a forward that concatenates the sigma map as channel 4."""
    import torch

    tile = max(64, int(tile))
    overlap = int(np.clip(overlap, 0, tile // 2))
    h, w, _ = rgb.shape
    acc = np.zeros((h, w, 3), np.float32)
    wsum = np.zeros((h, w, 1), np.float32)
    step = max(1, tile - overlap)
    ys = list(range(0, max(1, h - overlap), step)) if h > tile else [0]
    xs = list(range(0, max(1, w - overlap), step)) if w > tile else [0]
    autocast = (torch.autocast(device_type="cuda", dtype=torch.float16)
                if fp16 and device.startswith("cuda") else _nullctx())
    with torch.no_grad():
        for y in ys:
            for x in xs:
                y1, x1 = min(y + tile, h), min(x + tile, w)
                y0, x0 = max(0, y1 - tile), max(0, x1 - tile)
                patch = rgb[y0:y1, x0:x1, :]
                ph, pw = patch.shape[:2]
                # DRUNet needs /8 dims: replicate-pad, crop after.
                pb, pr = (-ph) % 8, (-pw) % 8
                padded = np.pad(patch, ((0, pb), (0, pr), (0, 0)), mode="edge")
                t = torch.from_numpy(padded.transpose(2, 0, 1)).unsqueeze(0)
                s = torch.full((1, 1, t.shape[2], t.shape[3]), float(sigma))
                with autocast:
                    out = model(torch.cat((t, s), dim=1).to(device))
                out = out.squeeze(0).float().clamp(0, 1).cpu().numpy().transpose(1, 2, 0)[:ph, :pw]
                win = _tile_window(ph, pw, overlap)[:, :, None]
                acc[y0:y1, x0:x1, :] += out * win
                wsum[y0:y1, x0:x1, :] += win
    wsum[wsum == 0] = 1.0
    return acc / wsum


class _nullctx:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def denoise_planes(model, planes, ab, device, tile, overlap, fp16):
    """Steps 3–5 on the four normalised planes → the four denoised planes."""
    z = {n: gat(p, *ab[n]) for n, p in planes.items()}
    # `model_affine` — from the noise model, never from this frame's data. The
    # 0.05 / 99.95 percentiles it replaced put a hard ceiling at igat(top): on a
    # 15 s ISO-3200 star field that ceiling sat at 7.6–14.7 % of full scale per
    # plane, so every star came back as the same grey dot, keeping 9.7 % of its
    # excess over the sky against Lightroom's 100 % (measured 2026-09-17).
    lo, span = model_affine(ab)
    sigma = SIGMA_SCALE / span
    log(f"model sigma {sigma:.4f} ({sigma*255:.2f}/255) over z range {lo:.1f}..{lo+span:.1f} "
        f"at scale {SIGMA_SCALE:g}")

    def norm(a):
        return np.clip((a - lo) / span, 0.0, 1.0).astype(np.float32)

    estimates = {}
    for g in ("G1", "G2"):
        rgb = np.stack([norm(z["R"]), norm(z[g]), norm(z["B"])], axis=2)
        estimates[g] = run_tiled(model, rgb, sigma, device, tile, overlap, fp16) * span + lo
    dz = {
        "R": (estimates["G1"][:, :, 0] + estimates["G2"][:, :, 0]) / 2.0,
        "G1": estimates["G1"][:, :, 1],
        "G2": estimates["G2"][:, :, 1],
        "B": (estimates["G1"][:, :, 2] + estimates["G2"][:, :, 2]) / 2.0,
    }
    return {n: np.clip(igat(dz[n], *ab[n]), 0.0, 1.0).astype(np.float32) for n in PLANES}


def blend_and_quantise(x_in, x_den, strength, black, white):
    """Step 6 for one plane: RAW-domain blend, back to the sensor's integers."""
    s = float(np.clip(strength, 0.0, 1.0))
    out = x_in + s * (x_den - x_in)
    v = np.round(out * (white - black) + black)
    return np.clip(v, 0, white).astype(np.uint16)


# ── Entry ───────────────────────────────────────────────────────────────────

def _parse_levels(black, white):
    try:
        blacks = [float(v) for v in black.split(",")]
        white = float(white)
    except ValueError:
        die(f"--black {black!r} / --white {white!r} are not numbers")
    if len(blacks) == 1:
        blacks *= 4
    if len(blacks) != 4:
        die(f"--black needs one or four values, got {len(blacks)}")
    if any(white <= b for b in blacks):
        die(f"--white {white} does not exceed the black level(s) {blacks}")
    return blacks, white


def publish(tmp, output):
    """tmp + fsync + os.replace (L03): the caller stages this artifact and
    publishes it durably; a partial file must never occupy the claimed name."""
    try:
        with open(tmp, "rb+") as f:
            os.fsync(f.fileno())
        os.replace(tmp, output)
    finally:
        if os.path.exists(tmp):
            try:
                os.remove(tmp)
            except OSError:
                pass  # why: the original error is already propagating; an unremovable .part (an AV lock) must not replace it


def main():
    ap = argparse.ArgumentParser(description="AutoShade RAW-domain AI denoise (DRUNet on the mosaic)")
    ap.add_argument("--input", required=True, help="16-bit grayscale PNG: the sensor mosaic")
    ap.add_argument("--output", required=True, help="16-bit grayscale PNG: the denoised mosaic")
    ap.add_argument("--pattern", required=True, help="2x2 CFA letters at (0,0),(0,1),(1,0),(1,1), e.g. RGGB")
    ap.add_argument("--black", required=True, help="black level(s): one value, or four in pattern order")
    ap.add_argument("--white", required=True, help="white level")
    # default = the Rust side's DEFAULT_STRENGTH_RAW (pinned textually by a test).
    ap.add_argument("--strength", type=float, default=1.0,
                    help="0..1: RAW-domain blend, 0 = the input bytes, 1 = the model's whole output")
    ap.add_argument("--tile", type=int, default=512)
    ap.add_argument("--overlap", type=int, default=32)
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "weights"))
    ap.add_argument("--fp16", action="store_true")
    ap.add_argument("--cpu", action="store_true")
    args = ap.parse_args()

    import cv2

    phases = parse_pattern(args.pattern)
    blacks, white = _parse_levels(args.black, args.white)
    strength = float(np.clip(args.strength, 0.0, 1.0))

    raw = cv2.imread(args.input, cv2.IMREAD_UNCHANGED)
    if raw is None:
        die(f"cannot read the mosaic: {args.input}")
    if raw.ndim != 2 or raw.dtype != np.uint16:
        die(f"the mosaic must be a single-channel 16-bit image, got shape {raw.shape} dtype {raw.dtype}")
    h, w = raw.shape
    if h < 2 * BLOCK or w < 2 * BLOCK:
        die(f"the mosaic is too small to denoise ({w}x{h}); each plane needs at least {BLOCK} px on a side")
    log(f"mosaic {w}x{h} pattern {args.pattern.upper()} black {blacks} white {white:g} strength {strength:g}")

    if strength <= 0.0:
        # The identity, byte for byte — the Rust side does not spawn for it,
        # but a bare run must keep the same promise.
        out = raw
    else:
        device = pick_device(args.cpu, "cuda:0")
        log(f"device={device}")
        black_at = {n: blacks[dy * 2 + dx] for n, (dy, dx) in phases.items()}
        planes = {}
        for n, p in split_planes(raw, phases).items():
            b = black_at[n]
            # Only the top is bounded (step 1): a sample below the black level
            # carries the lower half of the noise distribution, and rectifying
            # it at 0 biases both the fit and the model. `gat` floors it.
            planes[n] = np.minimum((p.astype(np.float32) - b) / (white - b), 1.0)
        ab = noise_model(planes)
        model = load_model(args.cache, device)
        log("denoising the four CFA planes ...")
        den = denoise_planes(model, planes, ab, device, args.tile, args.overlap, args.fp16)
        packed = {n: blend_and_quantise(planes[n], den[n], strength, black_at[n], white) for n in PLANES}
        out = merge_planes(packed, phases, raw.shape, raw)
        if (h % 2) or (w % 2):
            log(f"odd frame size {w}x{h}: the last row/column is passed through untouched")

    root, ext = os.path.splitext(args.output)
    tmp = f"{root}.{os.getpid()}.part{ext}"
    if not cv2.imwrite(tmp, out):
        die(f"cannot write the mosaic: {tmp}")
    publish(tmp, args.output)
    log(f"wrote {args.output} ({w}x{h}, 16-bit)")


if __name__ == "__main__":
    main()
