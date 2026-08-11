#!/usr/bin/env python3
"""Autoshop AI-denoise sidecar.

Real-photo denoiser for high-ISO / astro / low-light frames, in the spirit of
ACR/Lightroom "Denoise". Uses SCUNet (cszn/SCUNet) `color_real` weights — a
Swin-Conv-UNet trained on a *practical* (real sensor) degradation model, so it
targets the noise you actually get from a camera, not synthetic Gaussian.

The Rust engine (src/render.rs) hands us a developed/linear RGB image as a
16-bit PNG or TIFF; we denoise the pixels and write the result back at the same
bit depth. The AI never decides *edits* here — it only cleans noise; the rest of
the develop pipeline (tone/colour/sharpen) runs in Rust afterward.

Design choices (all verified in this environment 2026-06-26):
  * torch 2.8 + CUDA on an RTX 4060 Ti  -> GPU inference.
  * cv2 reads/writes 16-bit TIFF/PNG    -> no tifffile needed.
  * einops present; timm is shimmed      -> SCUNet's network file loads verbatim.
  * 60 MP is too big for one forward pass -> overlap-tiled inference, feathered.

Usage:
    python denoise.py --input in.tif --output out.tif
        [--model color_real_psnr|color_real_gan] [--strength 0..1]
        [--tile 512] [--overlap 32] [--cache DIR] [--fp16] [--cpu]

Exit code 0 on success; non-zero with a message on stderr otherwise.
"""
import argparse
import importlib.util
import os
import sys
import types
import warnings

warnings.filterwarnings("ignore")  # silence requests/urllib3 version warnings only

import numpy as np

_BASE = "https://github.com/cszn/KAIR/releases/download/v1.0"
WEIGHT_URLS = {
    # Blind real-noise models (best for actual high-ISO / astro frames).
    "color_real_psnr": f"{_BASE}/scunet_color_real_psnr.pth",
    "color_real_gan": f"{_BASE}/scunet_color_real_gan.pth",
    # Non-blind AWGN models, trained for a fixed noise level (15/25/50 on 0..255).
    # Handy as explicit strength tiers when the noise is closer to synthetic.
    "color_15": f"{_BASE}/scunet_color_15.pth",
    "color_25": f"{_BASE}/scunet_color_25.pth",
    "color_50": f"{_BASE}/scunet_color_50.pth",
}
# PINNED to an immutable commit, not a branch. This file is EXECUTED
# (exec_module below), so `.../SCUNet/main/...` meant "run whatever upstream
# has at download time, with the user's privileges, beside their API keys and
# photo library" — an upstream compromise or force-push would have been enough.
# 9a6c650 (2022-03-23) is the last commit that touched this path.
NETWORK_COMMIT = "9a6c6507aaddde34712553babc5e1f7fb8522287"
NETWORK_URL = (
    f"https://raw.githubusercontent.com/cszn/SCUNet/{NETWORK_COMMIT}/models/network_scunet.py"
)
# SHA-256 of that exact file (11445 bytes), verified against the upstream
# commit on 2026-08-04. A poisoned cache, a corrupted download or a MITM now
# fails LOUDLY instead of executing.
NETWORK_SHA256 = "77aeefd31e37080db7f0bf46bca5efcecc800fcfddb502081340a10b2b949c60"
NETWORK_BYTES = 11445

# Every weight download is pinned exactly like the network file: a .pth is a
# PICKLE handed to torch, so the CHANNEL must be authenticated, not just the
# loader flagged (weights_only defends the load; this defends the bytes).
# All five digests + byte counts verified 2026-08-11 two independent ways:
# streamed from github.com/cszn/KAIR/releases/download/v1.0/ and hashed, and
# (for color_real_psnr / color_25) matched byte-for-byte against the local
# cache this machine downloaded on 2026-06-26 — so existing caches revalidate
# without a re-download. The KAIR v1.0 release assets are immutable tags; if
# upstream ever re-cuts them the fetch fails loudly by design.
WEIGHT_SHA256 = {
    "color_real_psnr": "fa78899ba2caec9d235a900e91d96c689da71c42029230c2028b00f09f809c2e",
    "color_real_gan": "892c83f812c59173273b74f4f34a14ecaf57a2fdb68df056664589beb55c966e",
    "color_15": "fa3a95efb4add693a78917e70757a3d535c5a8c905ace9f93ba7e5897351e1b2",
    "color_25": "6b4e572fe69b1530aade8b7856b18b9e6ddf9cf2bd87c21bf045b51662096320",
    "color_50": "11f6839726c10dad327a75ce578be661a3e208f01fd7ab6d3eb763a5464bfdfe",
}
# Exact upstream sizes: the in-stream download cap (an endpoint that serves
# more than the pinned asset is refused mid-transfer, before the disk fills).
WEIGHT_BYTES = {
    "color_real_psnr": 71982841,
    "color_real_gan": 71982835,
    "color_15": 71982831,
    "color_25": 71982761,
    "color_50": 71982757,
}


def log(msg):
    print(f"[denoise] {msg}", file=sys.stderr, flush=True)


def _download(url, dest, max_bytes):
    import requests

    log(f"downloading {os.path.basename(dest)} ...")
    # Unique temp per process: two first-time runs racing on ONE ".part"
    # truncated each other and could publish a corrupted download.
    tmp = f"{dest}.{os.getpid()}.part"
    try:
        with requests.get(url, stream=True, timeout=60) as r:
            r.raise_for_status()
            total = int(r.headers.get("Content-Length", 0))
            done = 0
            with open(tmp, "wb") as f:
                for chunk in r.iter_content(chunk_size=1 << 20):
                    done += len(chunk)
                    # Enforced DURING the stream, not after — a post-hoc
                    # check runs with the disk already full. The cap comes
                    # from the pinned byte tables, so overshooting it means
                    # the endpoint is not serving the pinned asset.
                    if done > max_bytes:
                        raise SystemExit(
                            f"refusing {os.path.basename(dest)}: the download exceeded "
                            f"its pinned size ({max_bytes} bytes) — the endpoint is not "
                            f"serving the pinned asset")
                    f.write(chunk)
                    if total:
                        pct = 100 * done / total
                        print(f"\r[denoise]   {done >> 20}/{total >> 20} MB ({pct:4.1f}%)",
                              end="", file=sys.stderr, flush=True)
        print("", file=sys.stderr)
        # A server (or proxy) that closed early must not publish a SHORT
        # file onto the cache name — that surfaced later as a confusing
        # torch/pickle error instead of the true reason.
        if total and done != total:
            raise SystemExit(
                f"refusing {os.path.basename(dest)}: the download stopped at "
                f"{done} of {total} bytes")
        os.replace(tmp, dest)
    finally:
        # A failed / interrupted download must not accumulate per-process
        # .part litter in the cache (after os.replace this is a no-op).
        if os.path.exists(tmp):
            try:
                os.remove(tmp)
            except OSError:
                # why: best-effort cleanup on the ERROR path — the original
                # download exception is already propagating; an unremovable
                # .part (AV lock) must not replace it with a cleanup error.
                pass


def _install_timm_shim():
    """SCUNet's network file imports trunc_normal_ + DropPath from timm. Those are
    init-only / inference-noop, so a tiny shim satisfies the import without pulling
    timm (which would drag in torchvision). State-dict keys are unaffected."""
    import torch.nn as nn

    def trunc_normal_(tensor, mean=0.0, std=1.0, a=-2.0, b=2.0):
        return nn.init.trunc_normal_(tensor, mean, std, a, b)

    class DropPath(nn.Module):
        def __init__(self, drop_prob=0.0):
            super().__init__()
            self.drop_prob = drop_prob

        def forward(self, x):  # identity at inference (eval mode)
            return x

    timm = types.ModuleType("timm")
    models = types.ModuleType("timm.models")
    layers = types.ModuleType("timm.models.layers")
    layers.trunc_normal_ = trunc_normal_
    layers.DropPath = DropPath
    sys.modules.setdefault("timm", timm)
    sys.modules.setdefault("timm.models", models)
    sys.modules["timm.models.layers"] = layers

    # thop is a FLOPs counter the network file imports only for its __main__
    # self-test; a no-op stub satisfies the top-level import without the dep.
    thop = types.ModuleType("thop")
    thop.profile = lambda *a, **k: (0, 0)
    sys.modules.setdefault("thop", thop)


def _sha256(path):
    import hashlib

    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# Reclamation age for orphaned .part files: a HARD kill (the Rust side's
# 30-minute budget is TerminateProcess — the `finally` never runs) orphans a
# ~72 MB temp beside the cache. Well past 2x the default sidecar budget, so a
# live peer's in-flight transfer can never be swept; do not tune down without
# re-deriving from AUTOSHOP_SIDECAR_TIMEOUT_SECS.
STALE_PART_SECS = 24 * 3600


def _reclaim_stale_parts(dest):
    """Remove day-old `<dest>.<pid>.part` orphans (and ONLY those — globbing
    on the dest prefix keeps the sweep away from the image-output .part names
    in the user's out/ directory)."""
    import glob
    import time

    now = time.time()
    for p in glob.glob(f"{glob.escape(dest)}.*.part"):
        # ONLY the documented `<dest>.<pid>.part` shape — the wildcard alone
        # would also delete an unrelated stale `<dest>.backup.part`.
        middle = p[len(dest) + 1 : -len(".part")]
        if not middle.isdigit():
            continue
        try:
            if now - os.path.getmtime(p) > STALE_PART_SECS:
                os.remove(p)
                log(f"reclaimed orphaned download {os.path.basename(p)}")
        except OSError:
            pass  # why: a live peer's file or an AV lock — reclamation is never fatal


def _fetch_verified(url, dest, want_sha256, max_bytes, what):
    """Download (if absent) and REFUSE to proceed unless the bytes match.

    A cache filled by an older build — which fetched this file from a moving
    branch — is exactly the case that must not be trusted, so an existing file
    is verified too. One mismatch triggers a single re-download (the benign
    legacy-cache / truncated-download case); a second mismatch is fatal.
    """
    _reclaim_stale_parts(dest)
    for attempt in (0, 1):
        if not os.path.exists(dest):
            _download(url, dest, max_bytes)
        if not os.path.exists(dest):
            # A download that produced nothing without raising must not crash
            # this gate with FileNotFoundError — refuse honestly instead.
            break
        got = _sha256(dest)
        if got == want_sha256:
            return
        log(f"{what}: checksum mismatch (expected {want_sha256}, got {got})")
        try:
            os.remove(dest)
        except OSError:
            # why: cannot re-fetch over a file we may not delete — fail below
            # rather than execute unverified bytes.
            break
        if attempt == 0:
            log(f"{what}: re-downloading from the pinned source ...")
    raise SystemExit(
        f"refusing to run {what}: its bytes do not match the pinned checksum. "
        f"Delete the cache directory and retry; if it persists, the upstream "
        f"download is not trustworthy."
    )


def load_model(model_name, cache_dir, device):
    import torch

    os.makedirs(cache_dir, exist_ok=True)
    net_path = os.path.join(cache_dir, "network_scunet.py")
    # Verified BEFORE exec_module — including a file an older build cached
    # from the moving branch. The small slack on the cap keeps an overshoot
    # message about the ENDPOINT, not an off-by-one.
    _fetch_verified(NETWORK_URL, net_path, NETWORK_SHA256, NETWORK_BYTES + 4096,
                    "the SCUNet network definition")
    if model_name not in WEIGHT_URLS or model_name not in WEIGHT_SHA256:
        raise SystemExit(f"unknown or unpinned model '{model_name}'")
    weight_path = os.path.join(cache_dir, f"scunet_{model_name}.pth")
    # Through the VERIFIED fetch, existing cache included — the whole point:
    # this machine's own cache was fetched by pre-pinning code, and a
    # poisoned cache is exactly the case the digest closes for the .py one
    # call above. No exists() guard: _fetch_verified skips the download when
    # the file is present and hashes it anyway.
    _fetch_verified(WEIGHT_URLS[model_name], weight_path, WEIGHT_SHA256[model_name],
                    WEIGHT_BYTES[model_name] + 4096,
                    f"the SCUNet '{model_name}' weights")

    _install_timm_shim()
    spec = importlib.util.spec_from_file_location("network_scunet", net_path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)

    # color_real models: 3-channel, dim=64, 7 stages of depth 4 (KAIR test config).
    model = mod.SCUNet(in_nc=3, config=[4, 4, 4, 4, 4, 4, 4], dim=64)
    # weights_only: a .pth is a PICKLE, and torch below 2.6 executes arbitrary
    # code while unpickling — a tampered or replaced weight download would run
    # as the user. A state dict is plain tensors, so the safe loader suffices.
    # A torch too old to know the flag is REFUSED, not degraded: the old
    # fallback logged to a captured stderr that only surfaces on failure, so
    # the unsandboxed load was silent in practice — and a code-execution
    # boundary is exactly where refuse-not-degrade is non-negotiable. The
    # except stays NARROW: a malformed checkpoint raises UnpicklingError /
    # RuntimeError under weights_only=True and must keep propagating.
    try:
        state = torch.load(weight_path, map_location="cpu", weights_only=True)
    except TypeError as e:
        raise SystemExit(
            f"refusing to load the SCUNet weights: this torch ({torch.__version__}) "
            f"predates weights_only=True, so the load would execute the weight "
            f"pickle unsandboxed. Upgrade torch (pip install -U torch) and retry. ({e})"
        )
    model.load_state_dict(state, strict=True)
    model.eval().to(device)
    return model


def _tile_window(th, tw, overlap):
    """Linear feather window so overlapping tiles blend without seams."""
    wy = np.ones(th, dtype=np.float32)
    wx = np.ones(tw, dtype=np.float32)
    # A tile smaller than the overlap (a tiny image or an edge sliver) must
    # shrink the ramp, or the [:overlap] slice is shorter than the ramp and
    # np.minimum fails to broadcast.
    overlap = min(overlap, th // 2, tw // 2)
    if overlap > 0:
        ramp = np.linspace(0, 1, overlap, dtype=np.float32)
        wy[:overlap] = np.minimum(wy[:overlap], ramp)
        wy[-overlap:] = np.minimum(wy[-overlap:], ramp[::-1])
        wx[:overlap] = np.minimum(wx[:overlap], ramp)
        wx[-overlap:] = np.minimum(wx[-overlap:], ramp[::-1])
    # Floor to a small positive value: the ramp reaches 0 at the very edge, and an
    # image-border pixel covered by only one tile would otherwise divide by ~0 and
    # turn black. With a floor, a lone tile normalises to exactly the model output,
    # while interior seams (where a neighbour has ~full weight) still blend cleanly.
    return np.clip(np.outer(wy, wx), 1e-3, 1.0)


def denoise(model, img, device, tile=512, overlap=32, fp16=False):
    """img: float32 HxWx3 in [0,1]. Returns denoised float32 HxWx3 in [0,1]."""
    import torch

    # Guard the tiling maths: overlap >= tile turns the step into 1px
    # (millions of forward passes) and edge windows of mismatched sizes.
    tile = max(64, int(tile))
    overlap = int(np.clip(overlap, 0, tile // 2))
    h, w, _ = img.shape
    acc = np.zeros((h, w, 3), dtype=np.float32)
    wsum = np.zeros((h, w, 1), dtype=np.float32)
    step = max(1, tile - overlap)
    ys = list(range(0, max(1, h - overlap), step)) if h > tile else [0]
    xs = list(range(0, max(1, w - overlap), step)) if w > tile else [0]

    autocast = torch.autocast(device_type=device.split(":")[0], dtype=torch.float16) \
        if fp16 and device.startswith("cuda") else _nullctx()

    with torch.no_grad():
        for y in ys:
            for x in xs:
                y0, x0 = y, x
                y1, x1 = min(y0 + tile, h), min(x0 + tile, w)
                y0, x0 = max(0, y1 - tile), max(0, x1 - tile)  # keep full tile near edges
                patch = img[y0:y1, x0:x1, :]
                t = torch.from_numpy(patch.transpose(2, 0, 1)).unsqueeze(0).to(device)
                with autocast:
                    out = model(t)  # SCUNet pads to /64 internally
                out = out.squeeze(0).float().clamp(0, 1).cpu().numpy().transpose(1, 2, 0)
                win = _tile_window(y1 - y0, x1 - x0, overlap)[:, :, None]
                acc[y0:y1, x0:x1, :] += out * win
                wsum[y0:y1, x0:x1, :] += win
    wsum[wsum == 0] = 1.0
    return acc / wsum


class _nullctx:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def main():
    ap = argparse.ArgumentParser(description="Autoshop AI denoise (SCUNet)")
    ap.add_argument("--input", required=True)
    ap.add_argument("--output", required=True)
    ap.add_argument("--model", default="color_real_psnr", choices=list(WEIGHT_URLS))
    ap.add_argument("--strength", type=float, default=1.0, help="0..1 blend with original")
    ap.add_argument("--tile", type=int, default=512)
    ap.add_argument("--overlap", type=int, default=32)
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(__file__), "weights"))
    ap.add_argument("--fp16", action="store_true")
    ap.add_argument("--cpu", action="store_true")
    args = ap.parse_args()

    import cv2
    import torch

    device = "cpu" if args.cpu or not torch.cuda.is_available() else "cuda:0"
    log(f"device={device} model={args.model} strength={args.strength}")

    raw = cv2.imread(args.input, cv2.IMREAD_UNCHANGED)
    if raw is None:
        raise SystemExit(f"cannot read image: {args.input}")
    if raw.ndim == 2:
        raw = cv2.cvtColor(raw, cv2.COLOR_GRAY2BGR)
    # Preserve alpha through the round-trip instead of silently dropping it.
    alpha = None
    if raw.shape[2] == 4:
        alpha = raw[:, :, 3].copy()
        raw = raw[:, :, :3]
    # Only 8/16-bit integer data is supported: a float TIFF interpreted as
    # 8-bit would be destroyed, so refuse loudly (the Rust bridge always
    # hands over u8/u16).
    if raw.dtype not in (np.uint8, np.uint16):
        raise SystemExit(f"unsupported pixel dtype {raw.dtype} (only uint8/uint16)")
    is16 = raw.dtype == np.uint16
    maxv = 65535.0 if is16 else 255.0
    rgb = cv2.cvtColor(raw, cv2.COLOR_BGR2RGB).astype(np.float32) / maxv

    model = load_model(args.model, args.cache, device)
    log(f"input {rgb.shape[1]}x{rgb.shape[0]} ; denoising ...")
    den = denoise(model, rgb, device, tile=args.tile, overlap=args.overlap, fp16=args.fp16)

    s = float(np.clip(args.strength, 0.0, 1.0))
    if s < 1.0:
        den = s * den + (1.0 - s) * rgb

    out = np.clip(den * maxv + 0.5, 0, maxv).astype(np.uint16 if is16 else np.uint8)
    out = cv2.cvtColor(out, cv2.COLOR_RGB2BGR)
    if alpha is not None:
        out = np.dstack([out, alpha])
    # tmp + os.replace: a direct imwrite could leave a NONZERO partial file
    # on interruption — which the caller deliberately preserves as "evidence"
    # while it also occupies the atomically claimed artifact name. The tmp
    # keeps the real extension so cv2 picks the right encoder.
    root, ext = os.path.splitext(args.output)
    tmp = f"{root}.{os.getpid()}.part{ext}"
    try:
        ok = cv2.imwrite(tmp, out)
        if not ok:
            raise SystemExit(f"cannot write image: {tmp}")
        # fsync before the replace lands it (L03): the caller stages this
        # output and republishes it durably, but an older caller adopted it
        # as-is — the payload must not be able to vanish with the page cache.
        with open(tmp, "rb+") as f:
            os.fsync(f.fileno())
        os.replace(tmp, args.output)
    finally:
        if os.path.exists(tmp):
            try:
                os.remove(tmp)
            except OSError:
                # why: best-effort cleanup on the error path — the original
                # write exception is already propagating.
                pass
    log(f"wrote {args.output} ({'16-bit' if is16 else '8-bit'})")


if __name__ == "__main__":
    main()
