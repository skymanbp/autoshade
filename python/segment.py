#!/usr/bin/env python3
"""AI segmentation sidecar — writes an 8-bit grayscale mask PNG (white = selected).

Companion to the Rust bridge (src/segment.rs), following the same sidecar
pattern as denoise.py: the Rust side shells out, this script does one job and
exits non-zero with a human-readable reason on stderr when it can't.

Usage:
  python segment.py --input photo.png --output mask.png --target subject|sky

Backends (weights auto-download on first run — nothing is stored in the repo,
consistent with .gitignore'ing python/weights):
  subject -> BiRefNet (general checkpoint), sha256-pinned into python/weights
             (pip install torchvision timm einops; 444,473,596 B)
             ... falling back to rembg / U^2-Net when BiRefNet cannot run
             (pip install rembg; ~/.u2net) — see SUBJECT below
  sky     -> OneFormer ADE20K Swin-L via transformers
             (pip install transformers torch; ~/.cache/huggingface)
  object  -> SAM 2.1 Hiera-Large, point-prompted (see the OBJECT section)

LICENCES — this is a PUBLIC repository and the product is being copyright
registered, so the weights a user's machine fetches on our instruction have to
be licensed for the use we are instructing. Both backends were re-checked
2026-08-19 (R27 Batch-4, D1) and one of them had to be replaced:

  * SKY, REPLACED. It was `nvidia/segformer-b0-finetuned-ade-512-512`, whose
    weights carry the "NVIDIA Source Code License for SegFormer":
      "The Work and any derivative works thereof only may be used or intended
       for use non-commercially. Notwithstanding the foregoing, NVIDIA and its
       affiliates may use the Work and any derivative works commercially. As
       used herein, 'non-commercially' means for research or evaluation
       purposes only."
    (verbatim, https://raw.githubusercontent.com/NVlabs/SegFormer/master/LICENSE
     — the model card links to exactly that file and tags the model `other`.)
    That restricts USE, not redistribution, so "we only fetch it, we don't ship
    it" does not cure it. Replaced by `shi-labs/oneformer_ade20k_swin_large`,
    HF licence tag `mit` (verified against the HF API, 2026-08-19), which is
    also the STRONGER model: ADE20K mIoU 57.0 single-scale against SegFormer-B5's
    ~51 and far above the B0 that was actually shipped here. The licence-clean
    pick is the better pick; there is no quality trade.

  * SUBJECT, PINNED IN PLACE — and since R29 B4 it is the FALLBACK tier rather
    than the primary. `rembg.remove()` with no session argument
    resolves to whatever THAT INSTALL's default model is. On rembg 2.0.76 that
    is `new_session("u2net")` and U^2-Net is Apache-2.0 — fine — but upstream
    rembg has since moved its default to `bria-rmbg`, "released under a BRIA
    license that requires a paid agreement for commercial use", and warns that
    "Model weights carry their own licenses, independent of rembg's MIT
    license". So `pip install -U rembg` on any user's machine could silently
    swap a pay-to-use model in with zero change to Autoshop's source. The
    session is named explicitly below so the licence we checked is the licence
    that runs.

  * SUBJECT, PRIMARY SINCE R29 B4: `ZhengPeng7/BiRefNet`, the GENERAL
    checkpoint. MIT, ungated, on both the weight repo and the code repository
    it points at — the LICENSE file at
    https://raw.githubusercontent.com/ZhengPeng7/BiRefNet/main/LICENSE is the
    unmodified MIT text (1,066 B, sha256
    92a7089e0915fc32bc40067560b398f1e6a7a5958abd7d04eda393629a5acefb),
    with no field-of-use clause, no acceptable-use policy and no
    non-commercial carve-out — i.e. none of what disqualified SegFormer, SAM 3
    or CLIP. `cardData.license` is `mit` and `gated` is false on the HF API
    (re-verified 2026-08-21, this session). What the review did NOT resolve:
    the training sets the card names (DIS-TR / AM-2k / P3M-500-NP) carry their
    own terms and were not audited.

    NOT `BiRefNet_HR-matting`, which the R27 design document picked
    (D1-ml-sidecar-design.md:134). Measured on the photographer's own library
    (R29 B4 comparison pack, nine frames): HR-matting returns a LITERALLY
    EMPTY alpha — max value 0..4 of 255 — on 4 of 9, because it is a
    portrait/animal matting model and the frames it declines hold a vehicle, a
    street lamp and a small figure. Adopting it as designed would have deleted
    masks the user's photographs already carry. The general checkpoint fires on
    8 of 9; the one it declines is the no-salient-subject control, which is the
    right answer there. U^2-Net's own score is 0/9 empty, but its edges are
    the reason for the swap: on wind-blown hair it renders a soft wedge with no
    strand structure, and on a dark subject against a bright sky 48.1 % of the
    frame lands strictly between alpha 0.05 and 0.95 — a "maybe" spread over
    half the picture. It also invents a subject on a landscape that has none.

  * THE DEPENDENCY, and it REVERSES an earlier decision. BiRefNet imports
    `torchvision.ops.deform_conv2d` and `torchvision.models.vgg16/resnet50` at
    module scope, and `timm.layers` for its Swin backbone. R27 Batch-5
    declined exactly that (`docs/ROADMAP.md:1335`, and the note in
    `object_mask` below): torchvision is version-coupled to torch and the
    project had not taken it. That declination was explicitly NOT a ruling —
    the same adjudication parked the subject-backend question on a release
    inspection pack and said the user's eye on real photographs was the gate.
    The pack was built (R29 B4) and the user ruled on 2026-08-21: switch to the
    general BiRefNet, keep U^2-Net as the no-weights fallback tier, accept
    ~+16 s per mask (process startup dominates: 20.0 s total against U^2-Net's
    4.1 s, of which only 0.85 s is the forward pass), 444 MB of weights and a
    one-off re-derivation of every subject alpha. So torchvision + timm are
    taken here on measured evidence, not appetite. `kornia` — upstream's third
    new dependency — is NOT taken: `_kornia_shim` below explains why it is
    provably unreachable at inference.

The output mask is soft (the model's own alpha / class probability), which the
render engine samples bilinearly — so edges come pre-feathered.
"""

import argparse
import os
import sys


def die(msg: str) -> None:
    print(f"segment.py: {msg}", file=sys.stderr)
    sys.exit(2)


# --- SUBJECT: BiRefNet (general), U^2-Net as the fallback tier --------------
#
# Pinned the way SAM is, not the way `sky` is: every file is fetched by us and
# gated on its sha256 + byte count, then loaded from the local directory. That
# matters more here than anywhere else in the tree, because ONE OF THESE FILES
# IS EXECUTED — `birefnet.py` is the model's source and this sidecar runs it
# through `importlib`. A revision pin would fix which tree upstream serves; only
# the digest proves the bytes that reach `exec_module`.
#
# Digests: `model.safetensors` is the HF tree API's LFS `oid` at this revision
# (re-verified against the API 2026-08-21 and against the local cache byte for
# byte). The four non-LFS files carry no `oid`, so their digests are computed
# over the bytes downloaded — the same treatment R27 Batch-5 gave the SAM 2.1
# JSONs. `README.md` is pinned because it is the SOURCE OF THE PREPROCESSING
# this sidecar reimplements (its lines 120-124: Resize(1024,1024), ToTensor,
# ImageNet normalise): a revision that moved the recipe without moving the
# weights would otherwise change our alpha with nothing here changing.
BIREFNET = {
    "repo": "ZhengPeng7/BiRefNet",
    "revision": "e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4",
    "files": {
        "model.safetensors": {
            "sha256": "9ab37426bf4de0567af6b5d21b16151357149139362e6e8992021b8ce356a154",
            "bytes": 444473596,
        },
        "birefnet.py": {
            "sha256": "208771ae626f653d64128fbf2d6ac9f8e645c5cc5e286258a73ec3322bbfe5ef",
            "bytes": 91896,
        },
        "BiRefNet_config.py": {
            "sha256": "e7b8c2a74f6cea6a59553d517f71d47f2c1d90e670a13416af17c25fe2f3dc52",
            "bytes": 298,
        },
        "config.json": {
            "sha256": "c97ea21569daf66b205491a4635147dd3bc42c7c168b89d7d75b53f67ef548ae",
            "bytes": 405,
        },
        "README.md": {
            "sha256": "ceac4a1bb69b807eac5510bff80ff5599f606ef7458bee3b54b68d866e868532",
            "bytes": 9965,
        },
    },
}

# The square edge the frame is squashed to before the forward pass. 1024 is the
# card's own `image_size` and what R29 B4 measured end to end (0.850 s forward,
# 1,613 MiB peak VRAM on an 8 GB card). 1536 also fits (0.89-1.55 s, 3,074 MiB)
# and changes the alpha very little on people (mean |delta| 0.0005-0.0012) but
# noticeably on fine lace-like subjects (0.0382). 2048 — HR-matting's native
# training resolution — took 519.5 s for ONE forward pass on that card and is
# not a usable setting here.
BIREFNET_EDGE = 1024

# The two labels, and they are deliberately not one label. A photographer whose
# machine quietly ran the fallback saw "AI mask re-derived" either way; these
# strings are what makes the two states distinguishable in the sidecar's own
# output.
BIREFNET_LABEL = "BiRefNet " + BIREFNET["revision"][:12]
U2NET_LABEL = "U^2-Net (FALLBACK - BiRefNet did not run)"


def _birefnet_cache(cache_dir):
    """Fetch every pinned BiRefNet file into one directory and return it.

    Same shape as `_sam_cache`, and the same shared downloader — one
    implementation of the download-and-refuse rule in the tree. It RAISES
    rather than calling `die()`: this runs inside `subject_mask`'s fallback
    try-block, and a `SystemExit` there would be caught and re-reported as
    "BiRefNet did not run", which is true but says nothing useful.
    """
    try:
        from denoise import _fetch_verified
    except ImportError as e:
        raise RuntimeError(
            f"the shared sidecar downloader from denoise.py is not importable ({e}) "
            "- segment.py must sit beside denoise.py in python/"
        ) from e

    d = os.path.join(cache_dir, "ZhengPeng7--BiRefNet@" + BIREFNET["revision"][:12])
    os.makedirs(d, exist_ok=True)
    for name, pin in BIREFNET["files"].items():
        url = f"https://huggingface.co/{BIREFNET['repo']}/resolve/{BIREFNET['revision']}/{name}"
        _fetch_verified(
            url,
            os.path.join(d, name),
            pin["sha256"],
            pin["bytes"] + 4096,
            f"the BiRefNet '{name}'",
        )
    return d


def _kornia_shim():
    """Satisfy `birefnet.py`'s module-scope `from kornia.filters import laplacian`.

    `laplacian` is referenced in exactly ONE place (`birefnet.py:2083`) and that
    place is guarded by `if self.training and self.config.out_ref`. This sidecar
    calls `model.eval()` and asserts `not model.training` before the forward
    pass, so the branch is unreachable — which is what makes a stub honest here
    rather than a guess. `denoise.py`'s `_install_timm_shim` makes the identical
    argument for SCUNet's timm imports.

    The stub RAISES instead of returning a plausible tensor. If a future
    revision ever calls it at inference, that has to be a loud failure: a
    silently wrong Laplacian would become a silently wrong mask, and this
    sidecar's whole discipline is that a mask is never guessed at.

    Forced, not `setdefault`: one code path on every machine, whether or not the
    user happens to have kornia installed. It is process-local — nothing outside
    this interpreter sees it.
    """
    import types

    def laplacian(*_a, **_k):
        raise RuntimeError(
            "segment.py: BiRefNet called kornia.filters.laplacian on the INFERENCE "
            "path, which the pinned revision never does (it is guarded by "
            "self.training). The model code has changed and the kornia stub is no "
            "longer safe - re-derive it before moving the revision pin."
        )

    kornia = types.ModuleType("kornia")
    filters = types.ModuleType("kornia.filters")
    filters.laplacian = laplacian
    kornia.filters = filters
    sys.modules["kornia"] = kornia
    sys.modules["kornia.filters"] = filters


def _load_birefnet_module(weights_dir):
    """Import the digest-verified `birefnet.py` as a member of a synthetic package.

    `birefnet.py:1976` does `from .BiRefNet_config import BiRefNetConfig`, a
    RELATIVE import, so a plain `spec_from_file_location("birefnet", ...)` fails
    with "attempted relative import with no known parent package". A synthetic
    package whose `__path__` is the verified directory gives it exactly the
    parent it asks for, without copying either file out of that directory and
    without `trust_remote_code` — which would fetch and execute upstream Python
    through HF's own cache, where our digest gate cannot see it.
    """
    import importlib.util
    import types

    _kornia_shim()
    pkg_name = "autoshop_birefnet"
    pkg = types.ModuleType(pkg_name)
    pkg.__path__ = [weights_dir]
    sys.modules[pkg_name] = pkg
    for mod, fn in (
        (f"{pkg_name}.BiRefNet_config", "BiRefNet_config.py"),
        (f"{pkg_name}.birefnet", "birefnet.py"),
    ):
        spec = importlib.util.spec_from_file_location(mod, os.path.join(weights_dir, fn))
        m = importlib.util.module_from_spec(spec)
        sys.modules[mod] = m
        spec.loader.exec_module(m)
    return sys.modules[f"{pkg_name}.birefnet"]


def _birefnet_subject_mask(img_path: str, cache_dir: str, edge: int):
    """Salient-subject alpha from the pinned general BiRefNet checkpoint."""
    # THE DEPENDENCY PROBE COMES FIRST, before the fetch. `birefnet.py` imports
    # torchvision, timm and einops at ITS module scope, so a machine missing any
    # of them cannot run this backend — and finding that out AFTER
    # `_birefnet_cache` has hashed 444 MB costs ~20 s per mask on exactly the
    # machines the fallback tier exists for. Measured, one whole fallback run:
    # 22.3 s before this probe, 3.1 s after.
    # A real import, not `find_spec`: a torchvision built against another torch
    # resolves and then raises, and that is the failure worth catching here.
    import importlib

    for dep in ("torchvision", "timm", "einops"):
        try:
            importlib.import_module(dep)
        except ImportError as e:
            raise RuntimeError(
                f"BiRefNet needs torchvision + timm + einops ({e}) -> "
                "pip install torchvision timm einops, with a torchvision matching your "
                "torch (they are version-coupled: torch 2.8.0 <-> torchvision 0.23.0)"
            ) from e
    import torch
    from safetensors.torch import load_file

    import numpy as np
    from PIL import Image

    d = _birefnet_cache(cache_dir)

    # The pinned config, asserted against what this sidecar assumes — the same
    # gate `object_mask` puts on SAM's preprocessor_config.json. `bb_pretrained`
    # is the load-bearing one: true would send `build_backbone` to torchvision
    # for ImageNet weights at construction time, i.e. an unpinned download on a
    # path our digest gate does not cover.
    import json

    with open(os.path.join(d, "config.json"), encoding="utf-8") as f:
        cfg_json = json.load(f)
    auto_map = cfg_json.get("auto_map") or {}
    if (
        cfg_json.get("bb_pretrained") is not False
        or cfg_json.get("architectures") != ["BiRefNet"]
        or auto_map.get("AutoModelForImageSegmentation") != "birefnet.BiRefNet"
    ):
        raise SystemExit(
            "refusing to segment: the pinned BiRefNet config does not match what this "
            "sidecar builds - re-derive it before moving the revision pin."
        )

    bn = _load_birefnet_module(d)
    # `bb_pretrained=False` on BOTH the argument and the config: `BiRefNet.__init__`
    # overwrites its own argument from `config.bb_pretrained`, so the config is the
    # one that decides, and passing only the argument would be a no-op.
    model = bn.BiRefNet(bb_pretrained=False, config=bn.BiRefNetConfig(bb_pretrained=False))
    sd = load_file(os.path.join(d, "model.safetensors"))
    # strict in effect, with a READABLE message: `strict=True` raises a
    # thousand-key wall of text. Any missing or unexpected key means the module
    # this build constructed is not the module these weights were saved from,
    # and a half-initialised network would still produce a plausible-looking
    # mask — the one failure this must not have.
    missing, unexpected = model.load_state_dict(sd, strict=False)
    if missing or unexpected:
        raise RuntimeError(
            f"BiRefNet state-dict mismatch: {len(missing)} missing "
            f"{missing[:3]}, {len(unexpected)} unexpected {unexpected[:3]}"
        )
    model.eval()
    # Determinism: this mask becomes a FILE that a saved recipe references, and
    # R21's version fingerprinting compares those bytes. Same knobs, same reason
    # as sky_mask and object_mask.
    torch.manual_seed(0)
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    device = "cuda" if torch.cuda.is_available() else "cpu"
    # fp16 on CUDA — the checkpoint is stored F16 and that is the configuration
    # R29 B4 measured. A CPU box gets fp32 and it is SLOW (the Swin-L backbone
    # at 1024^2); saying so beats discovering it, exactly as sky_mask argues.
    dtype = torch.float16 if device == "cuda" else torch.float32
    if device == "cpu":
        print(
            "segment.py: no CUDA device - running BiRefNet on the CPU in fp32. "
            "This is minutes, not seconds, per mask.",
            file=sys.stderr,
        )
    model.to(device, dtype=dtype)

    img = Image.open(img_path).convert("RGB")
    # PREPROCESSING SPELLED OUT, not delegated to torchvision's `transforms` —
    # the `object_mask` reason: a resample filter that changed under us would
    # move every mask with nothing in this repo changing. This IS the card's
    # recipe (`README.md:120-124` of the pinned weight repo): `Resize((1024,
    # 1024))` is a plain BILINEAR squash to square with no letterbox and no
    # padding to undo, `ToTensor` is /255, then the ImageNet statistics.
    small = img.resize((edge, edge), resample=Image.BILINEAR)
    arr = np.asarray(small, dtype=np.float32) / 255.0
    arr = (arr - np.asarray([0.485, 0.456, 0.406], dtype=np.float32)) / np.asarray(
        [0.229, 0.224, 0.225], dtype=np.float32
    )
    x = torch.from_numpy(np.ascontiguousarray(arr.transpose(2, 0, 1))).unsqueeze(0)
    # The guard the kornia stub rests on, checked rather than assumed.
    if model.training:
        raise RuntimeError("BiRefNet is in training mode after .eval() - refusing to infer")
    with torch.no_grad():
        # `[-1]` is the finest of the multi-scale predictions; `.sigmoid()` is
        # the card's own read-out. LOGITS through a sigmoid, never a hard
        # threshold: the render engine samples this bilinearly and multiplies it
        # in, so the soft transition IS the feathering.
        pred = model(x.to(device, dtype=dtype))[-1].sigmoid().float()
    # Back to the SOURCE frame. Exact, because the forward transform was a plain
    # squash with no padding to undo.
    alpha = torch.nn.functional.interpolate(
        pred, size=(img.height, img.width), mode="bilinear", align_corners=False
    )[0, 0]
    m = alpha.clamp(0.0, 1.0).cpu().numpy()
    m = (m * 255.0).clip(0, 255).astype(np.uint8)
    return Image.fromarray(m, mode="L")


def subject_mask(img_path: str, cache_dir: str, edge: int):
    """Subject alpha, and the NAME of the backend that produced it.

    Two tiers, and the caller is told which one ran. BiRefNet is the model the
    user ruled for; U^2-Net is the fallback for a machine that cannot run it —
    no torchvision/timm, no network for the first fetch, a card that will not
    hold the weights. The two are NOT interchangeable (R29 B4 measured the
    difference on the user's own photographs), so a run that silently degraded
    to the second while reporting the first would be the sidecar lying about
    provenance.

    `SystemExit` is caught alongside `Exception` on purpose: `_fetch_verified`
    refuses a digest mismatch by raising it, and a machine whose cached weights
    do not match the pin is precisely a machine that cannot run BiRefNet. It
    falls back — loudly, naming the reason — rather than aborting the mask
    entirely, because a mask with no alpha at all makes the render SKIP the
    whole adjustment (`segment::resolve_ai_masks`).
    """
    try:
        return _birefnet_subject_mask(img_path, cache_dir, edge), BIREFNET_LABEL
    except (Exception, SystemExit) as e:
        print(
            f"segment.py: WARNING - the BiRefNet subject backend did not run ({type(e).__name__}: {e}); "
            "falling back to rembg / U^2-Net. The alpha below is the FALLBACK model's: "
            "its edges are materially softer (R29 B4 measured no strand structure on "
            "wind-blown hair and a low-confidence spread over half the frame on a dark "
            "subject), and it can invent a subject where there is none. "
            "Install torchvision + timm + einops matching your torch to get the pinned "
            "BiRefNet instead.",
            file=sys.stderr,
        )
        return _u2net_subject_mask(img_path), U2NET_LABEL


def _u2net_subject_mask(img_path: str):
    """Salient-subject alpha via an EXPLICITLY named U^2-Net rembg session."""
    try:
        from rembg import new_session, remove
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "subject segmentation needs BiRefNet (pip install torchvision timm einops) "
            "or, for the fallback tier, rembg -> pip install rembg "
            "(the U^2-Net model auto-downloads to ~/.u2net on first run)"
        )
    from PIL import Image

    img = Image.open(img_path).convert("RGB")
    # The session is NAMED, not defaulted. `remove(img, only_mask=True)` with
    # no session resolves to `new_session(<whatever this rembg's default is>)`,
    # and rembg's default is upstream's choice, not ours: on 2.0.76 it is
    # "u2net" (Apache-2.0), and current upstream has moved it to "bria-rmbg",
    # which needs a paid agreement for commercial use. That is a licence change
    # a user could import into this product with `pip install -U rembg` and no
    # change to this file. U^2-Net is the model whose licence was checked, so
    # U^2-Net is the model this asks for. See the module docstring.
    session = new_session("u2net")
    # only_mask=True returns the soft alpha as a single-channel PIL image.
    return remove(img, only_mask=True, session=session)


# The exact HF commit the sky weights are taken from — a 40-hex tree sha, not a
# branch. `from_pretrained(name)` alone resolves the MOVING `main` of a remote
# repo, so the model a user ran last week and the model they run today need not
# be the same file, and `id2label` deciding which plane is "sky" is part of what
# could move. Verified against the HF API on 2026-08-19: sha
# 4a5bac8e64f82681a12db2e151a4c2f4ce6092b2, cardData.license "mit", not gated,
# config.json id2label["2"] == "sky" over 150 classes.
#
# This is WEAKER than denoise.py's discipline, which sha256-pins the network
# file and every weight blob and refuses a mismatch loudly. A revision pin fixes
# WHICH tree is fetched; only a digest gate proves the BYTES. Closing that gap
# means fetching each file ourselves and loading from a local directory (see
# D1 §3.3) — a larger change than a licence fix, and registered here rather than
# left to be rediscovered.
#
# NARROWED, R29 B4: this registration used to cover `subject` as well. It no
# longer does — the subject backend is now the digest-gated BiRefNet above and
# its fallback tier is rembg, which gates its own download on its own checksum.
# `sky` is the LAST holdout, and it is the awkward one: `OneFormerProcessor`
# pulls a CLIP tokenizer tree alongside the weights, so the local-directory
# treatment is not the four-line copy of `_birefnet_cache` it looks like.
SKY_MODEL = "shi-labs/oneformer_ade20k_swin_large"
SKY_REVISION = "4a5bac8e64f82681a12db2e151a4c2f4ce6092b2"


def sky_mask(img_path: str):
    """ADE20K semantic segmentation, sky-class probability as the mask."""
    try:
        import torch
        from transformers import (
            OneFormerForUniversalSegmentation,
            OneFormerProcessor,
        )
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "sky segmentation needs transformers + torch -> pip install transformers "
            "(OneFormer ADE20K Swin-L, ~880 MB, auto-downloads to ~/.cache/huggingface)"
        )
    import numpy as np
    from PIL import Image

    name = SKY_MODEL
    processor = OneFormerProcessor.from_pretrained(name, revision=SKY_REVISION)
    model = OneFormerForUniversalSegmentation.from_pretrained(name, revision=SKY_REVISION)
    model.eval()
    # Determinism: this mask becomes a FILE that a saved recipe references, and
    # R21's version fingerprinting compares those bytes. cuDNN autotuning and
    # TF32 both pick different kernels run to run / card to card, which would
    # make the same recipe render two different masks.
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    # Swin-L is ~60x the parameters of the B0 this replaced, so CPU-only would
    # turn a ~1 s call into a minute. Use the GPU when there is one; the CPU
    # path still works, it is just slow, and saying so beats discovering it.
    device = "cuda" if torch.cuda.is_available() else "cpu"
    model.to(device)

    # Resolve the sky class from the model's own label table instead of
    # hard-coding an index — survives label-map revisions. EXACT match first:
    # ADE20K has both `2 = sky` and `48 = skyscraper`, and the old substring
    # test matched both, then took `[0]` and trusted dict insertion order to
    # put the right one first. A building is not the sky.
    labels = {int(i): str(l) for i, l in model.config.id2label.items()}
    exact = sorted(i for i, l in labels.items() if l.strip().lower() == "sky")
    loose = sorted(i for i, l in labels.items() if "sky" in l.lower())
    sky_ids = exact or loose
    if not sky_ids:
        die(f"model {name} has no 'sky' class in id2label — cannot build a sky mask")
    sky_id = sky_ids[0]

    img = Image.open(img_path).convert("RGB")
    with torch.no_grad():
        # OneFormer is task-conditioned: the text prompt selects which head's
        # semantics the queries are decoded against.
        inputs = processor(images=img, task_inputs=["semantic"], return_tensors="pt")
        inputs = {k: (v.to(device) if hasattr(v, "to") else v) for k, v in inputs.items()}
        out = model(**inputs)
        # OneFormer is a MASK-CLASSIFICATION model, so there is no per-pixel
        # class logit tensor to softmax: it emits Q query masks plus a class
        # distribution per query. The semantic map transformers itself builds
        # (OneFormerImageProcessor.post_process_semantic_segmentation) is
        #     seg[b,c,h,w] = sum_q softmax(class_logits)[b,q,c] * sigmoid(mask_logits)[b,q,h,w]
        # and it then argmaxes that, which throws the soft alpha away. We want
        # the alpha, and we want ONE class of 150 — so contract the query axis
        # against the sky column only. Identical arithmetic to the einsum
        # above, restricted to c = sky_id, and it never materialises the
        # 150-plane intermediate.
        cls = out.class_queries_logits.softmax(dim=-1)[..., :-1]  # drop the null class
        masks = out.masks_queries_logits.sigmoid()  # (1, Q, h/4, w/4)
        sky_lo = torch.einsum("bq,bqhw->bhw", cls[..., sky_id], masks).unsqueeze(1)
        # Upsample ONLY the sky plane, at the model's own resolution -> the
        # input's. Doing it before the class contraction would put all ~150
        # planes at full size: on a 61 MP frame that is ~36 GB of float32.
        sky = torch.nn.functional.interpolate(
            sky_lo, size=(img.height, img.width), mode="bilinear", align_corners=False
        )[0, 0]

    # A sum of query contributions is not normalised the way a softmax is, so
    # clamp rather than assume: the alpha this writes is a coverage weight the
    # engine multiplies in, and >1 there would over-apply the adjustment.
    m = sky.float().clamp(0.0, 1.0).cpu().numpy()
    m = (m * 255.0).clip(0, 255).astype(np.uint8)
    return Image.fromarray(m, mode="L")


# --- OBJECT: SAM 2.1, point-prompted at the sidecar's own click -------------
#
# The third backend (R27 Batch-5, L-08 Arm C). Lightroom's `Mask/Image` carries
# `crs:ReferencePoint` on 218/218 real instances — the photographer's own
# normalised click — and `MaskSubType=0` means "the object (or background)
# there". A point-promptable model is a LITERAL match for that: the file hands
# us a click and SAM's native interface IS a click.
#
# LICENCE: "The SAM 2 model checkpoints, SAM 2 demo code (front-end and
# back-end), and SAM 2 training code are licensed under Apache 2.0"
# (https://github.com/facebookresearch/sam2). The HF repo tags `apache-2.0` and
# is NOT gated (verified against the HF API 2026-08-19). SAM 3 was rejected on
# mechanism as well as licence: its repo is gated, so "download on first run"
# cannot work without a token and a click-through.
#
# PINNING is stricter here than the sky backend. That one resolves a pinned
# HF REVISION and lets `transformers` fetch; this one fetches every file itself
# and gates each on its sha256 + byte count, then loads with
# `local_files_only=True` — `denoise.py`'s discipline, reused verbatim through
# its own `_fetch_verified`. R29 B4 put `subject` on the same footing; `sky` is
# the remaining gap and is registered in its own comment above, not closed here.
SAM = {
    "repo": "facebook/sam2.1-hiera-large",
    "revision": "665f8e2ad61cf5f53d65644ff27c8ee525124610",
    "files": {
        "model.safetensors": {
            "sha256": "dc407dce21301fd94abb395c5099b4f2c455fdc8a8f261ac3d0ea6d4cd197230",
            "bytes": 897897416,
        },
        "config.json": {
            "sha256": "00446988cf4d617118d2d347eabe2c46aebed744628facdd540508be30b69ec3",
            "bytes": 5705,
        },
        "preprocessor_config.json": {
            "sha256": "6ebf229ee259368ce4a8d4f2fe893a72b053023710853e257253939e601f583d",
            "bytes": 683,
        },
        "processor_config.json": {
            "sha256": "f8a68e865cfad115c1c2763f3d93eca7b1c622da06da2a9273eb437fb2389b6d",
            "bytes": 95,
        },
    },
}


def _sam_cache(cache_dir):
    """Fetch every pinned SAM file into one directory and return it.

    Imports `denoise.py`'s verified downloader rather than copying it, exactly
    as `embed.py` does — one implementation of the download-and-refuse rule in
    the tree. Its progress lines say `[denoise]` because that is the module
    they live in.
    """
    try:
        from denoise import _fetch_verified
    except ImportError as e:
        die(
            f"object segmentation needs the shared sidecar downloader from denoise.py ({e}) "
            "-> segment.py must sit beside denoise.py in python/"
        )
    d = os.path.join(cache_dir, "facebook--sam2.1-hiera-large@" + SAM["revision"][:12])
    os.makedirs(d, exist_ok=True)
    for name, pin in SAM["files"].items():
        url = f"https://huggingface.co/{SAM['repo']}/resolve/{SAM['revision']}/{name}"
        _fetch_verified(
            url,
            os.path.join(d, name),
            pin["sha256"],
            pin["bytes"] + 4096,
            f"the SAM 2.1 '{name}'",
        )
    return d


def object_mask(img_path: str, point, cache_dir: str, min_iou: float):
    """Soft alpha for the object under `point` (normalised x, y)."""
    try:
        import torch
        from transformers import Sam2Model
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "object segmentation needs transformers + torch -> pip install transformers "
            "(SAM 2.1 Hiera-Large, ~898 MB, downloads to python/weights on first run)"
        )
    import numpy as np
    from PIL import Image

    d = _sam_cache(cache_dir)
    model = Sam2Model.from_pretrained(d, local_files_only=True)
    model.eval()
    # Same determinism knobs, same reason as sky_mask: this mask becomes a FILE
    # a saved recipe references.
    torch.manual_seed(0)
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    device = "cuda" if torch.cuda.is_available() else "cpu"
    model.to(device)

    img = Image.open(img_path).convert("RGB")
    # PREPROCESSING SPELLED OUT, not delegated to `Sam2Processor`. The reason
    # this was written for has EXPIRED and the code stays anyway, which is worth
    # saying plainly: transformers 5.2.0 resolves that processor to
    # `Sam2ImageProcessorFast`, which raises "requires `torchvision` to be
    # installed", and torchvision was a dependency this project had not taken —
    # until R29 B4 took it for BiRefNet. What survives is the embed.py reason,
    # which was always the stronger one: a resample filter that changed under us
    # would move every mask with nothing in this repo changing. Delegating now
    # would be a behaviour change on a mask a saved recipe references, bought
    # for nothing.
    #
    # The transform is exactly the pinned `preprocessor_config.json`, asserted
    # against it below: squash to 1024x1024 (`default_to_square: true`, so
    # there is no letterbox and no padding to undo), resample 2 = BILINEAR,
    # rescale 1/255, normalise by the ImageNet statistics.
    import json

    with open(os.path.join(d, "preprocessor_config.json"), encoding="utf-8") as f:
        pc = json.load(f)
    size = pc.get("size") or {}
    mean, std = pc.get("image_mean"), pc.get("image_std")
    if (
        (size.get("height"), size.get("width")) != (1024, 1024)
        or pc.get("resample") != 2
        or not pc.get("default_to_square")
        or mean != [0.485, 0.456, 0.406]
        or std != [0.229, 0.224, 0.225]
    ):
        raise SystemExit(
            "refusing to segment: the pinned SAM 2.1 preprocessor config does not match the "
            "transform this sidecar implements — re-derive it before moving the revision pin."
        )
    edge = 1024
    arr = np.asarray(
        img.resize((edge, edge), resample=2), dtype=np.float32
    ) / 255.0
    arr = (arr - np.asarray(mean, dtype=np.float32)) / np.asarray(std, dtype=np.float32)
    pixel_values = torch.from_numpy(
        np.ascontiguousarray(arr.transpose(2, 0, 1))
    ).unsqueeze(0)
    # The normalised click -> MODEL-frame pixels. The Rust bridge hands over
    # the ORIGINAL-frame preview, so the point and the pixels are in the same
    # frame by construction; the squash above is a pure scale, so the click
    # maps by multiplying with the model edge. Clamping keeps a click written
    # at exactly 1.0 (or a hair outside, which Lightroom does write) on the
    # last addressable pixel instead of one past the edge.
    px = min(max(point[0], 0.0), 1.0) * (edge - 1)
    py = min(max(point[1], 0.0), 1.0) * (edge - 1)
    pts = torch.tensor([[[[float(px), float(py)]]]], dtype=torch.float32)
    labels = torch.tensor([[[1]]], dtype=torch.long)
    with torch.no_grad():
        out = model(
            pixel_values=pixel_values.to(device),
            input_points=pts.to(device),
            input_labels=labels.to(device),
            multimask_output=True,
        )
    # LOGITS, then sigmoid — never `binarize=True`'s hard 0/1. The render
    # engine samples this bilinearly and multiplies it in, so a hard mask is a
    # hard edge on every adjustment; the logits carry the model's own soft
    # transition. The low-res masks come back at 256x256 and are resized to the
    # SOURCE frame here, which is exact because the forward transform was a
    # plain squash with no padding to undo.
    low = out.pred_masks.float().cpu()
    low = low.reshape(-1, low.shape[-2], low.shape[-1]).unsqueeze(1)
    masks = torch.nn.functional.interpolate(
        low, size=(img.height, img.width), mode="bilinear", align_corners=False
    ).squeeze(1)
    scores = out.iou_scores.float().cpu().reshape(-1)
    # argmax with the LOWEST-INDEX tie-break written out: `torch.argmax`'s tie
    # behaviour is not contractually specified, and this choice decides which
    # of three candidate masks becomes the user's selection.
    best = int(max(range(len(scores)), key=lambda i: (float(scores[i]), -i)))
    iou = float(scores[best])
    if iou < min_iou:
        # EXIT 3, not a bad mask. The Rust bridge treats a written-but-refused
        # mask as a failure and discards it; declining loudly is what lets the
        # caller say "the segmenter did not find an object there" instead of
        # adopting a blob.
        print(
            f"segment.py: declining the object mask - best predicted IoU {iou:.3f} is below "
            f"--min-iou {min_iou:.3f} at reference point ({point[0]:.4f}, {point[1]:.4f})",
            file=sys.stderr,
        )
        sys.exit(3)
    alpha = torch.sigmoid(masks[best])
    m = alpha.clamp(0.0, 1.0).numpy()
    m = (m * 255.0).clip(0, 255).astype(np.uint8)
    return Image.fromarray(m, mode="L")


def parse_point(text: str):
    """`crs:ReferencePoint` verbatim — two space-separated normalised floats."""
    parts = text.split()
    if len(parts) != 2:
        die(f"--reference-point must be two space-separated numbers, got {text!r}")
    try:
        x, y = float(parts[0]), float(parts[1])
    except ValueError:
        die(f"--reference-point must be two numbers, got {text!r}")
    # Lightroom writes these inside [0,1]; a small margin absorbs a click on
    # the very edge without accepting a coordinate from another frame.
    if not (-0.05 <= x <= 1.05 and -0.05 <= y <= 1.05):
        die(f"--reference-point ({x}, {y}) is outside the normalised frame")
    return (x, y)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--input", required=True, help="source image (any PIL-readable format)")
    ap.add_argument("--output", required=True, help="mask PNG to write (8-bit grayscale)")
    ap.add_argument("--target", required=True, choices=["subject", "sky", "object"])
    ap.add_argument(
        "--reference-point",
        help="crs:ReferencePoint verbatim, e.g. \"0.517578 0.260997\" (required for --target object)",
    )
    ap.add_argument(
        "--min-iou",
        type=float,
        default=0.5,
        help="decline (exit 3) when SAM's own predicted IoU is below this",
    )
    ap.add_argument(
        "--mask-size",
        type=int,
        default=4096,
        help="cap the written mask's LONG EDGE (0 = no cap)",
    )
    ap.add_argument(
        "--infer-size",
        type=int,
        default=BIREFNET_EDGE,
        help="BiRefNet's square inference edge (--target subject; 1024 measured, 1536 fits)",
    )
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(__file__), "weights"))
    a = ap.parse_args()

    if a.target == "object":
        if not a.reference_point:
            die("--target object needs --reference-point (the sidecar's crs:ReferencePoint)")
        mask = object_mask(a.input, parse_point(a.reference_point), a.cache, a.min_iou)
        backend = "SAM 2.1 Hiera-Large " + SAM["revision"][:12]
    elif a.target == "subject":
        if a.infer_size < 32:
            die(f"--infer-size {a.infer_size} is too small to segment anything")
        mask, backend = subject_mask(a.input, a.cache, a.infer_size)
    else:
        mask = sky_mask(a.input)
        backend = "OneFormer ADE20K Swin-L " + SKY_REVISION[:12]
    mask = mask.convert("L")
    # LONG-EDGE CAP. The render engine charges every mask raster w*h*4 against a
    # 256 MiB budget (`render::raster_bytes` / `MASK_RASTER_BUDGET_BYTES`), so a
    # 9504x6336 mask alone would be 229.7 MiB and a recipe with two of them is
    # refused outright. 4096 costs 42.7 MiB, which leaves room for five, and it
    # is still 1.4x Adobe's own segmenter proxy (`crs:FullMaskSize` is
    # "2880,1920" on every real instance). The mask is sampled BILINEARLY in
    # normalised coordinates, so its resolution is independent of the render's
    # — this cap costs edge precision, not correctness.
    if a.mask_size and max(mask.size) > a.mask_size:
        from PIL import Image as _Image

        scale = a.mask_size / max(mask.size)
        w = max(1, round(mask.width * scale))
        h = max(1, round(mask.height * scale))
        # NAMED filter, for the reason every resize in this family is named.
        mask = mask.resize((w, h), resample=_Image.BILINEAR)
    # tmp + os.replace: a direct save truncates in place, so an interrupted /
    # failed / racing rerun could corrupt a mask a saved recipe already
    # references (fixed names like mask-sky.png outlive this process). The
    # .png suffix keeps PIL's format inference; os.replace is atomic on
    # Windows and POSIX.
    tmp = f"{a.output}.{os.getpid()}.tmp.png"
    try:
        mask.save(tmp)
        # fsync before the replace lands it (L03): the caller's recipe JSON
        # commits durably, so a mask still in the page cache must not vanish
        # out from under a reference a power cut preserved.
        with open(tmp, "rb+") as f:
            os.fsync(f.fileno())
        os.replace(tmp, a.output)
    finally:
        if os.path.exists(tmp):
            try:
                os.remove(tmp)
            except OSError:
                # why: best-effort cleanup on the error path — the original
                # save/replace exception is already propagating.
                pass
    # The BACKEND is named, not just the target. `--target subject` has two
    # possible answers now and they are not interchangeable, so a line that said
    # only "subject mask" would leave the one fact a reader needs out of it.
    print(f"segment.py: {a.target} mask [{backend}] -> {a.output}")


if __name__ == "__main__":
    main()
