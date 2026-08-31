#!/usr/bin/env python3
"""Cross-image correspondence sidecar - DIFT diffusion features, as JSON.

Fourth member of the sidecar family (`denoise.py` = SCUNet, `segment.py` =
subject/sky masks, `embed.py` = style vectors). Same contract as all three:
the Rust side shells out (src/correspond.rs), this script does one job,
writes its output atomically and exits non-zero with a human-readable reason
on stderr when it cannot.

Usage:
  python correspond.py --source neutral.png --target look.png --output corr.json
                       [--cache DIR] [--cpu] [--ensemble 8] [--seed 0]

WHAT IT COMPUTES - a dense semantic correspondence between two renditions of
the same frame whose CONTENT partially diverges (the reverse-fit's
atmosphere-mode territory: a generated target that replaced the sky, moved a
cloud deck, invented texture). For every cell of a 48x48 feature grid over
the source it reports where that content sits in the target and how much the
match can be trusted. It NEVER generates pixels - the output is coordinates
and confidences, nothing else (plan step 7's contract).

METHOD - DIFT ("Emergent Correspondence from Image Diffusion", Tang et al.
2023): encode each 768x768 image into SD latents, add noise at timestep 261,
run ONE Stable Diffusion 2.1 UNet forward pass with an empty-prompt
embedding, and read the up_blocks[1] feature map (1280ch at 48x48). The
paper's ensemble (default 8 noise draws, averaged) runs as a LOOP of
single-sample passes so peak VRAM stays that of one pass. Matching is
mutual-nearest-neighbour over cosine similarity, and the reported confidence
is cyclic consistency x local flow smoothness:

  * CYCLIC - source cell -> best target cell -> best source cell must land
    near where it started; content that was REPLACED (an invented cloud) has
    no stable round trip and earns ~0.
  * SMOOTHNESS - the flow must agree with the median flow of its 3x3
    neighbourhood. This is load-bearing, not cosmetic: a pixel-shuffle
    permutation of the same frame (the fit's atmosphere-budget fixtures are
    exactly that) produces individually strong but spatially incoherent
    matches, and the smoothness term is what keeps such a pair honestly
    unmatchable. Raw cosine similarity is reported per cell (`sim`) for
    diagnostics but deliberately does NOT enter the confidence: feature
    norms drift across backbones and inputs, while the two terms above are
    scale-free.

MODEL - Stable Diffusion 2.1 (the backbone the DIFT paper measured), fp16
variant, ~2.6 GB total. LICENCE: the weights carry CreativeML Open RAIL++-M
- redistribution is permitted and commercial use is allowed, with the
licence's use-based behavioural restrictions travelling with the weights.
This repository redistributes nothing; the sidecar downloads at first use,
and the licence note here is the disclosure. PROVENANCE: the official
`stabilityai/stable-diffusion-2-1` repo was delisted upstream (verified
2026-08-26: anonymous HTTP 401, authenticated 404), so the pin points at the
highest-traffic community mirror. The mirror's faithfulness was established
byte-for-byte before pinning: its fp32 safetensors LFS digests equal an
independent uploader's (SfinOe/stable-diffusion-v2-1) for all three towers -
unet 1238522277c48923ff2751e238f2742c562e45643f3d50cc93d163cb30638b0c,
vae a1d993488569e928462932c8c38a0760b874d166399b14414135bd9c42df5815,
text_encoder cce6febb0b6d876ee5eb24af35e27e764eb4f9b1d0b7c026c8c3333d4cfc916c
- and the digest gate below is the only door at run time either way.

PINNING - the discipline is `denoise.py`'s, reused rather than reimplemented:
this module imports `_fetch_verified` from it, so there is exactly ONE
download-and-verify implementation in the tree (its progress lines announce
themselves as `[denoise]` because that is the module they live in). Every
file is fetched from a URL pinned to a 40-hex commit and gated on its own
sha256 + byte count, and the models load from that local directory only
(the `local_files_only` flag) - `transformers`/`diffusers` never open a socket.
`trust_remote_code` is never used and never will be.
"""

import argparse
import json
import os
import sys

# The shared device rule. `_device.py` ships beside this script in `python/`,
# the same way `segment.py` requires its ADE20K class table to; `sys.path[0]`
# is the running script's own directory, which the Rust side resolves against
# the program's tree and never the working directory.
from _device import pick_device

# The download/verify half of the sidecar contract, imported rather than
# copied - same rule as embed.py: a relocated correspond.py without
# denoise.py beside it fails HERE with a sentence.
try:
    from denoise import _fetch_verified
except ImportError as e:  # pragma: no cover - environment shape, not logic
    print(
        f"correspond.py: cannot import the shared sidecar downloader from "
        f"denoise.py ({e}) - correspond.py must sit beside denoise.py in python/.",
        file=sys.stderr,
    )
    sys.exit(2)


def log(msg):
    print(f"[correspond] {msg}", file=sys.stderr, flush=True)


def die(msg: str) -> None:
    print(f"correspond.py: {msg}", file=sys.stderr)
    sys.exit(2)


# The DIFT recipe constants, from the paper's SD featurizer: input edge,
# noise timestep, and which UNet up-block's output is the feature.
INPUT_SIZE = 768
TIMESTEP = 261
UP_BLOCK = 1
GRID = 48  # 768 / 16 - up_blocks[1] output resolution
FEATURE_DIM = 1280

# Confidence shape parameters (grid-cell units). Cyclic sigma is tight - a
# round trip that lands 2+ cells away is already suspect; smoothness sigma is
# looser - real correspondence fields bend at object boundaries.
CYCLIC_SIGMA = 1.5
SMOOTH_SIGMA = 2.0

# The HF mirror, its pinned commit, and every file we fetch with the sha256 +
# exact byte count that file must have. LFS digests taken 2026-08-26 from the
# HF tree API at this revision; the small JSONs hashed over the actual
# downloaded bytes at the same revision. Cross-mirror provenance: docstring.
MODEL = {
    "repo": "sd2-community/stable-diffusion-2-1",
    "revision": "bb2154823665391b4fb29b0b9cf82a198964ee05",
    "files": {
        "unet/config.json": {
            "sha256": "dc987e214928b191cb832df4e19c376e862af1e4c4a5f36aac054a53ee251ccf",
            "bytes": 939,
        },
        "unet/diffusion_pytorch_model.fp16.safetensors": {
            "sha256": "8a3a4d7978884c5e4ef00b62641b1b544b257be2f6715d984188610ad6475ad2",
            "bytes": 1731904736,
        },
        "vae/config.json": {
            "sha256": "d69281aa3f6a0f3c41aaf6778e35464fc6ee8a92e6ac8a8b1eb679f6df6423eb",
            "bytes": 611,
        },
        "vae/diffusion_pytorch_model.fp16.safetensors": {
            "sha256": "3e4c08995484ee61270175e9e7a072b66a6e4eeb5f0c266667fe1f45b90daf9a",
            "bytes": 167335342,
        },
        "text_encoder/config.json": {
            "sha256": "6b34b0bf6cff02e2afe88145740ef5e0316caf7add8307c8ec0b021a7923cc42",
            "bytes": 633,
        },
        "text_encoder/model.fp16.safetensors": {
            "sha256": "681c555376658c81dc273f2d737a2aeb23ddb6d1d8e5b3a7064636d359a22668",
            "bytes": 680821096,
        },
        "tokenizer/vocab.json": {
            "sha256": "e089ad92ba36837a0d31433e555c8f45fe601ab5c221d4f607ded32d9f7a4349",
            "bytes": 1059962,
        },
        "tokenizer/merges.txt": {
            "sha256": "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
            "bytes": 524619,
        },
        "tokenizer/tokenizer_config.json": {
            "sha256": "87a3154f0990fd992fd59f9d42c39520155b3d77cd543efe3f2bf011726f379d",
            "bytes": 824,
        },
        "tokenizer/special_tokens_map.json": {
            "sha256": "f118ab3a983206e4f32583448de6bd6aae4ee21869135cef1f5848a753cdaab6",
            "bytes": 460,
        },
        "scheduler/scheduler_config.json": {
            "sha256": "4cd9b9597ca64549df35016ca02bd3450ecbac70ccd8b0465b018be4ba54fe4b",
            "bytes": 345,
        },
    },
}


def model_dir(cache_dir):
    """One directory per pinned (repo, revision) - same rule as embed.py."""
    slug = MODEL["repo"].replace("/", "--")
    return os.path.join(cache_dir, f"{slug}@{MODEL['revision'][:12]}")


def fetch_model(cache_dir):
    d = model_dir(cache_dir)
    for name, pin in MODEL["files"].items():
        dest = os.path.join(d, name)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        url = (
            f"https://huggingface.co/{MODEL['repo']}/resolve/"
            f"{MODEL['revision']}/{name}"
        )
        _fetch_verified(
            url,
            dest,
            pin["sha256"],
            pin["bytes"] + 4096,
            f"the SD 2.1 '{name}'",
        )
    return d


def load_models(cache_dir, device):
    import torch

    try:
        from diffusers import AutoencoderKL, UNet2DConditionModel
        from transformers import CLIPTextModel, CLIPTokenizer
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "correspondence needs diffusers + transformers + torch -> "
            "python -m pip install diffusers (SD 2.1 fp16, ~2.6 GB, downloads "
            "to python/weights on first run)"
        )
    d = fetch_model(cache_dir)
    # Determinism knobs BEFORE the load, same reasoning as embed.py: the
    # correspondence decides which pixels a saved recipe was estimated from,
    # so cuDNN autotuning or TF32 picking different kernels run to run would
    # make the same pair fit differently on the same machine.
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    dtype = torch.float16 if device.startswith("cuda") else torch.float32
    # local_files_only: the digest gate above is the ONLY door (embed.py's
    # rule). variant="fp16" names the exact pinned weight files.
    unet = UNet2DConditionModel.from_pretrained(
        os.path.join(d, "unet"), variant="fp16", torch_dtype=dtype, local_files_only=True
    )
    vae = AutoencoderKL.from_pretrained(
        os.path.join(d, "vae"), variant="fp16", torch_dtype=dtype, local_files_only=True
    )
    text_encoder = CLIPTextModel.from_pretrained(
        os.path.join(d, "text_encoder"), variant="fp16", torch_dtype=dtype, local_files_only=True
    )
    tokenizer = CLIPTokenizer.from_pretrained(os.path.join(d, "tokenizer"), local_files_only=True)
    with open(os.path.join(d, "scheduler", "scheduler_config.json"), encoding="utf-8") as f:
        sched = json.load(f)
    for m in (unet, vae, text_encoder):
        m.eval()
        m.to(device)
    return unet, vae, text_encoder, tokenizer, sched


def alpha_cumprod_at(sched, t, torch):
    """sqrt(alpha_bar_t) and sqrt(1-alpha_bar_t) from the PINNED scheduler
    config - the noising schedule is part of what "DIFT at t=261" means, so it
    is derived from the receipt rather than hard-coded."""
    n = int(sched["num_train_timesteps"])
    b0 = float(sched["beta_start"])
    b1 = float(sched["beta_end"])
    kind = sched.get("beta_schedule", "scaled_linear")
    if kind == "scaled_linear":
        betas = torch.linspace(b0**0.5, b1**0.5, n, dtype=torch.float64) ** 2
    elif kind == "linear":
        betas = torch.linspace(b0, b1, n, dtype=torch.float64)
    else:
        die(f"unsupported beta_schedule '{kind}' in the pinned scheduler config")
    bar = torch.cumprod(1.0 - betas, dim=0)[t].item()
    return bar**0.5, (1.0 - bar) ** 0.5


def preprocess(path, np):
    """One image -> float32 (1, 3, 768, 768) in [-1, 1]. Spelled out like
    embed.py's transform so the resample filter is a named constant here."""
    from PIL import Image

    with Image.open(path) as im:
        im = im.convert("RGB")
        im = im.resize((INPUT_SIZE, INPUT_SIZE), resample=2)  # 2 = BILINEAR
        a = np.asarray(im, dtype=np.float32)
    a = a / 127.5 - 1.0
    return np.ascontiguousarray(a.transpose(2, 0, 1))[None]


def dift_features(unet, vae, text_encoder, tokenizer, sched, device, image, ensemble, seed, np):
    """The DIFT featurizer: (1,3,768,768) -> (FEATURE_DIM, GRID, GRID) float32.

    One seeded generator drives every noise draw, and the ensemble runs as a
    LOOP of batch-1 UNet passes: averaging over draws is the paper's variance
    reduction, and one-at-a-time keeps peak VRAM at a single pass on an 8 GB
    card."""
    import torch

    dtype = next(unet.parameters()).dtype
    x = torch.from_numpy(image).to(device=device, dtype=dtype)
    with torch.no_grad():
        lat = vae.encode(x).latent_dist.mean * vae.config.scaling_factor
        tok = tokenizer(
            "", padding="max_length", max_length=tokenizer.model_max_length, return_tensors="pt"
        )
        emb = text_encoder(tok.input_ids.to(device))[0].to(dtype)
        sa, s1 = alpha_cumprod_at(sched, TIMESTEP, torch)
        t = torch.tensor([TIMESTEP], device=device)
        gen = torch.Generator(device=device).manual_seed(seed)
        grabbed = {}

        def hook(_m, _i, out):
            grabbed["f"] = out

        h = unet.up_blocks[UP_BLOCK].register_forward_hook(hook)
        acc = None
        try:
            for _ in range(max(1, ensemble)):
                noise = torch.randn(lat.shape, generator=gen, device=device, dtype=dtype)
                noised = sa * lat + s1 * noise
                unet(noised, t, encoder_hidden_states=emb)
                f = grabbed.pop("f").float()
                acc = f if acc is None else acc + f
        finally:
            h.remove()
        f = (acc / max(1, ensemble))[0].cpu().numpy().astype(np.float32)
    if f.shape != (FEATURE_DIM, GRID, GRID):
        die(
            f"UNet up_block[{UP_BLOCK}] returned {f.shape}; the pinned SD 2.1 "
            f"yields ({FEATURE_DIM}, {GRID}, {GRID}) at {INPUT_SIZE}px - the "
            f"checkpoint is not the one this recipe was written for"
        )
    return f


def match(fs, ft, np):
    """Mutual-NN matching + the two-term confidence. All fp32 numpy.

    Returns (map_x, map_y, confidence, sim) - each a flat (GRID*GRID,) array
    over source cells; map_* are target-cell coordinates."""
    n = GRID * GRID
    a = fs.reshape(FEATURE_DIM, n).T.copy()
    b = ft.reshape(FEATURE_DIM, n).T.copy()
    for m in (a, b):
        m /= np.maximum(np.linalg.norm(m, axis=1, keepdims=True), 1e-12)
    sims = a @ b.T  # (n_src, n_tgt)
    fwd = sims.argmax(axis=1)
    bwd = sims.argmax(axis=0)
    sim = sims[np.arange(n), fwd]

    gy, gx = np.divmod(np.arange(n), GRID)  # cell i -> (row gy, col gx)
    ty, tx = np.divmod(fwd, GRID)
    # CYCLIC: how far the round trip lands from where it started, in cells.
    back = bwd[fwd]
    by, bx = np.divmod(back, GRID)
    cyc = np.hypot(bx - gx, by - gy)
    conf_cyc = np.exp(-(cyc**2) / (2.0 * CYCLIC_SIGMA**2))

    # SMOOTHNESS: the flow against the median flow of its 3x3 neighbourhood
    # (median, not mean - one wild neighbour must not drag the reference).
    fx = (tx - gx).astype(np.float32).reshape(GRID, GRID)
    fy = (ty - gy).astype(np.float32).reshape(GRID, GRID)
    pad_x = np.pad(fx, 1, mode="edge")
    pad_y = np.pad(fy, 1, mode="edge")
    nx = np.stack([pad_x[r : r + GRID, c : c + GRID] for r in range(3) for c in range(3)])
    ny = np.stack([pad_y[r : r + GRID, c : c + GRID] for r in range(3) for c in range(3)])
    dev = np.hypot(fx - np.median(nx, axis=0), fy - np.median(ny, axis=0)).reshape(-1)
    conf_smooth = np.exp(-(dev**2) / (2.0 * SMOOTH_SIGMA**2))

    conf = conf_cyc * conf_smooth
    return tx.astype(np.float32), ty.astype(np.float32), conf.astype(np.float32), sim.astype(np.float32)


def arr_json(np, v):
    """Float array as JSON text at float32's shortest round-trip precision -
    embed.py's vec_json rule, same reason (a doubled float costs ~20 bytes to
    say nothing extra)."""
    return "[" + ",".join(str(np.float32(x)) for x in v) + "]"


def publish(path, text):
    """tmp + fsync + os.replace, like all three existing sidecars (L03)."""
    tmp = f"{path}.{os.getpid()}.tmp"
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            try:
                os.remove(tmp)
            except OSError:
                # Best-effort cleanup on the error path - an unremovable temp
                # (an AV lock, on Windows) must not become the reported fault.
                # why: the original write exception is already propagating.
                pass


def main() -> None:
    ap = argparse.ArgumentParser(
        description="AutoShade cross-image correspondence (DIFT / SD 2.1)"
    )
    ap.add_argument("--source", required=True, help="the source rendition (any PIL-readable)")
    ap.add_argument("--target", required=True, help="the target rendition of the same frame")
    ap.add_argument("--output", required=True, help="correspondence JSON to write")
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(__file__), "weights"))
    ap.add_argument("--ensemble", type=int, default=8, help="noise draws averaged (paper: 8)")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--cpu", action="store_true")
    a = ap.parse_args()

    import numpy as np
    import torch

    device = pick_device(a.cpu, "cuda:0")
    unet, vae, text_encoder, tokenizer, sched = load_models(a.cache, device)
    log(f"device={device} model={MODEL['repo']} t={TIMESTEP} ensemble={a.ensemble}")

    feats = []
    for what, path in (("source", a.source), ("target", a.target)):
        img = preprocess(path, np)
        feats.append(
            dift_features(
                unet, vae, text_encoder, tokenizer, sched, device, img, a.ensemble, a.seed, np
            )
        )
        log(f"{what} features extracted")
    map_x, map_y, conf, sim = match(feats[0], feats[1], np)

    body = (
        "{"
        f'"model":{json.dumps(MODEL["repo"])},'
        f'"revision":"{MODEL["revision"]}",'
        f'"backbone":"dift-sd21","timestep":{TIMESTEP},"ensemble":{max(1, a.ensemble)},'
        f'"grid_w":{GRID},"grid_h":{GRID},"input_size":{INPUT_SIZE},'
        f'"map_x":{arr_json(np, map_x)},'
        f'"map_y":{arr_json(np, map_y)},'
        f'"confidence":{arr_json(np, conf)},'
        f'"sim":{arr_json(np, sim)}'
        "}\n"
    )
    publish(a.output, body)
    strong = float((conf >= 0.5).mean())
    log(
        f"wrote {a.output} (grid {GRID}x{GRID}, median conf {float(np.median(conf)):.3f}, "
        f"share>=0.5 {strong:.3f})"
    )


if __name__ == "__main__":
    main()
