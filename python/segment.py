#!/usr/bin/env python3
"""AI segmentation sidecar — writes an 8-bit grayscale mask PNG (white = selected).

Companion to the Rust bridge (src/segment.rs), following the same sidecar
pattern as denoise.py: the Rust side shells out, this script does one job and
exits non-zero with a human-readable reason on stderr when it can't.

Usage:
  python segment.py --input photo.png --output mask.png --target subject|sky|object
      [--reference-point "x y"] [--prompt-file gp1.json]
  python segment.py --input photo.png --output manifest.json --target sky
      --multi --regions 4

Backends (weights auto-download on first run — nothing is stored in the repo,
consistent with .gitignore'ing python/weights):
  subject -> BiRefNet (general checkpoint), sha256-pinned into python/weights
             (pip install torchvision timm einops; 444,473,596 B)
             ... falling back to rembg / U^2-Net when BiRefNet cannot run
             (pip install rembg; ~/.u2net) — see SUBJECT below
  sky     -> OneFormer ADE20K Swin-L, sha256-pinned into python/weights
             (pip install transformers torch; 881,196,376 B over seven files;
             the 7,085 B ADE20K class table the processor needs is NOT
             downloaded — it ships in python/ — see the SKY section)
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

  * SKY + OBJECT TRAINING SETS, AUDITED AND CLOSED (R30 B4, 2026-08-22).
    ADE20K's terms (https://ade20k.csail.mit.edu/terms/) govern access to and
    use/redistribution of the database; they contain no clause binding trained
    models or their downstream users. SA-1B's terms likewise govern its images
    and metadata, not trained models, weights or outputs (canonical dataset page:
    https://ai.meta.com/datasets/segment-anything/; clause text checked in the
    secondary mirror at https://huggingface.co/datasets/xiuqhou/SA-Det-100k/blob/main/LICENSE).
    Autoshop downloads and executes only the separately licensed model files,
    never either training set. The ADE20K class table shipped here is our own,
    rebuilt from MIT model metadata and cross-checked against MIT OneFormer
    source as documented below; it is not an ADE20K annotation download.

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
    (re-verified 2026-08-21, this session).

    TRAINING-SET TERMS, AUDITED AND CLOSED (R29 C5, 2026-08-21). The card's one
    training-set sentence ("trained on DIS-TR") is BOILERPLATE -- the identical
    sentence sits in BiRefNet_HR and BiRefNet_lite, whose training sets are
    provably different, while the dedicated DIS checkpoints live at
    ZhengPeng7/BiRefNet-DIS5K. On the model zoo plus the -legacy card's
    "(w/o portrait seg data)" qualifier, this checkpoint is the `general use`
    swin_v1_large row: DIS5K-TR, DIS-TEs, DUTS-TR_TE, HRSOD-TR_TE, UHRSD-TR_TE,
    HRS10K-TR_TE, TR-P3M-10k, TE-P3M-500-NP, TE-P3M-500-P, TR-humans, over an
    ImageNet-22k swin backbone. AM-2k is NOT among them -- it belongs to the
    matting and 2048 rows, which we do not run.

    Of those, one carries a use restriction: DIS5K's Terms of Use say "The
    Dataset is available for non-commercial use in research or educational
    purpose ... commercial use of this dataset is prohibited even after copying,
    editing, processing or any operations of this database"
    (https://raw.githubusercontent.com/xuebinqin/DIS/main/DIS5K-Dataset-Terms-of-Use.pdf,
    sha256 d509ad2249225d698921e1e4ce5497ca2d987cb7cc476b81c3b3094b26b9af96).
    That does NOT reach us, and the difference from SegFormer is the whole point:
    NVIDIA restricted THE WORK -- the weights we execute, with us as licensee --
    so not redistributing cured nothing. DIS5K restricts A DATABASE WE NEVER
    OBTAIN, under a signed registration agreement (it has Name/Affiliation/E-mail
    fields and asks you to sign) to which we are not a party; its clause 1
    reserves rights and grants none, so no condition runs with the weights, and
    its clause 4 forbids distributing the database, which we do not do. The
    reading that would condemn it also condemns DUTS (built from ImageNet DET),
    the ImageNet-pretrained swin backbone, the torchvision vgg16/resnet50
    imported below, and U^2-Net -- our own fallback tier, trained on DUTS-TR.
    A rule with no reachable compliant state is not the rule.

    Everything else is permissive: P3M-10k and AM-2k are MIT by clause 1 of their
    release agreements; HRSOD is Flickr Creative Commons; UHRSD is Flickr/Pixabay
    "free copyright"; HRS10K is Unsplash/Pixabay; TR-humans is Apache-2.0 and
    synthetic. Recorded, not restrictive: P3M-500-NP is 500 identifiable
    NON-face-blurred celebrity images (the P3M agreement's own preamble calls all
    10,421 "face-blurred", which is wrong for that subset) -- no privacy clause
    exists in the agreement, and a 1024x1024 binary-alpha discriminative model
    cannot reproduce a training image. Upstream's own position, verbatim, when
    asked about website use: "Sure. I set all models in MIT License."
    (huggingface.co/ZhengPeng7/BiRefNet/discussions/15 -- also the only copyright
    complaint ever filed against the repo; its entire body was "111", it was
    never substantiated, and HF staff closed it.) Full audit with verbatim
    clauses and provenance digests:
    ~/.claude/plans/r29-materials/c5-dataset-terms-audit.md

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
import json
import math
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


def birefnet_deps_error():
    """`None` when BiRefNet's imports all resolve, else the reason they do not.

    ONE dependency list for the whole file: `_birefnet_subject_mask` gates its
    own run on this, and `--probe-backend` answers the Rust cache with it, so
    the two can never disagree about what "this machine can run BiRefNet" means.

    A real import, not `find_spec`: a torchvision built against another torch
    RESOLVES and then raises, and that is the failure worth catching here.
    `birefnet.py` imports all three at ITS module scope, so any one of them
    missing is fatal to the backend.
    """
    import importlib

    for dep in ("torchvision", "timm", "einops"):
        try:
            importlib.import_module(dep)
        except ImportError as e:
            return (
                f"BiRefNet needs torchvision + timm + einops ({e}) -> "
                "pip install torchvision timm einops, with a torchvision matching your "
                "torch (they are version-coupled: torch 2.8.0 <-> torchvision 0.23.0)"
            )
    return None


def _birefnet_subject_mask(img_path: str, cache_dir: str, edge: int):
    """Salient-subject alpha from the pinned general BiRefNet checkpoint."""
    # THE DEPENDENCY PROBE COMES FIRST, before the fetch, and finding that out
    # AFTER `_birefnet_cache` has hashed 444 MB costs ~20 s per mask on exactly
    # the machines the fallback tier exists for. Measured, one whole fallback
    # run: 22.3 s before this probe, 3.1 s after.
    missing = birefnet_deps_error()
    if missing:
        raise RuntimeError(missing)
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
# config.json id2label["2"] == "sky" over 150 classes (re-verified 2026-08-21,
# this session, and the digests below were computed over the fetched bytes).
#
# DIGEST-GATED SINCE R29 C3/C4 — the registration that lived here is CLOSED.
# It used to say: "a revision pin fixes WHICH tree is fetched; only a digest
# gate proves the BYTES", with `sky` the last backend still short of denoise.py
# / SAM / BiRefNet discipline, awkward because `OneFormerProcessor` pulls a CLIP
# tokenizer tree alongside the weights. It does — six files, not one — and they
# are all pinned below and loaded with `local_files_only=True`, so nothing on
# this path resolves a remote name at run time.
#
# CLOSING IT SURFACED A HOLE THE REVISION PIN NEVER COVERED, and it is the real
# reason this was not a four-line copy of `_birefnet_cache`. `OneFormerImage-
# Processor.__init__` (and the Fast one alike) ends in
#     self.metadata = prepare_metadata(load_metadata(repo_path, class_info_file))
# and `load_metadata` falls through to the hub downloader with
# repo_id "shi-labs/oneformer_demo", "ade20k_panoptic.json" and
# repo_type "dataset" — a SECOND repository, a DATASET repo, resolved at its
# moving `main`, on every single sky mask. `SKY_REVISION` never reached it and
# the local-files-only flag does not stop it (it is a separate call with its own
# kwargs; proved by `HF_HUB_OFFLINE=1`, which turns the load into
# `OfflineModeIsEnabled` on exactly that URL). The `metadata` key sitting in the
# pinned `preprocessor_config.json` does NOT help AT LOAD TIME: the constructor
# filters it out of the kwargs and recomputes from the file. (Its CONTENT is
# another matter — it is one of the sources the file below was rebuilt from; see
# the next block.) So the table is put in the verified directory HERE and handed
# back through `repo_path` — `load_metadata` prefers
# `os.path.join(repo_path, class_info_file)` when that is a real file, which is
# what makes the offline load work.
#
# THE SECOND REPO IS GONE — the class table is now OURS (R29 收口, ruling 11).
# What used to sit here was a registration, not a clearance:
# `shi-labs/oneformer_demo` declares NO licence (the HF API returns
# `cardData: null` and `tags: ["region:us"]`, checked 2026-08-21), so of every
# asset this tree fetched it was the only one that had never been through the
# criterion at the top of this file. Pinning its revision fixed WHICH bytes
# arrived; it did not answer under what terms. The user's ruling was to stop
# fetching it and rebuild the table from licence-clean facts instead — which is
# what the file beside this script is.
#
# WHAT THE FILE HAS TO BE. `prepare_metadata` (transformers 5.2.0,
# `models/oneformer/image_processing_oneformer.py:367`) is the whole consumer:
#     for key, info in class_info.items():
#         metadata[key] = info["name"]; class_names.append(info["name"])
#         if info["isthing"]: thing_ids.append(int(key))
# So the contract is exactly: string keys "0".."149" IN ASCENDING ORDER (the
# order IS `class_names`), each mapping to an object with `name` and `isthing`.
# Nothing else is read, and any EXTRA top-level key would be read as a 151st
# class — which is why the file carries no comment field and its provenance
# lives here instead.
#
# WHERE IT GOES, stated precisely because it is what makes the swap safe: the
# `metadata` this builds is read by `encode_inputs` only under
# `if annotations is not None` (i.e. when `segmentation_maps` is passed, which
# is the annotation path) and by `post_process_instance_segmentation` /
# `get_*_annotations`. `sky_mask` passes no segmentation maps and calls no
# `post_process_*` — it contracts the query axis itself, below — so on THIS
# path the table is consumed at construction and never read again. It still has
# to be well formed: `prepare_metadata` indexes `info["name"]` and
# `info["isthing"]` unguarded, so a missing field is a `KeyError` on every sky
# mask, not a silent degradation.
#
# WHERE THE FACTS COME FROM — three sources, all MIT, all agreeing on all 150
# rows before the file was written (the constructor asserted it row by row):
#   * names + ids: `config.json` `id2label` from the MODEL repo itself, already
#     pinned below. MIT, and the same file `sky_mask` resolves the sky class
#     against, so the table cannot drift from the label map the mask uses.
#   * thing/stuff split: `preprocessor_config.json` `metadata.thing_ids` from
#     the same MIT model repo, also already pinned. This is the part the old
#     note said "would have to be invented" — it does not: the checkpoint's own
#     processor config ships the answer, and the constructor filtering it out at
#     load time (see above) is a transformers quirk, not an absence.
#   * both, independently: `ADE20K_150_CATEGORIES` in SHI-Labs/OneFormer's
#     `oneformer/data/datasets/register_ade20k_panoptic.py` (MIT, LICENSE
#     retrieved 2026-08-21, 1,065 B) — the upstream factual table, used as a
#     third opinion rather than a source of bytes.
# The third-party file was then used ONE way and one way only: as a check. The
# reconstruction is dict-equal to it, key order included, and the
# `prepare_metadata()` of the two are equal — the 152-key dicts, `class_names`
# order and all 100 `thing_ids`. Verified at the PIXEL level besides, which is
# the claim that does not depend on reading transformers correctly: two
# photographs, a full sky run on each with the old table and with ours (both
# under `HF_HUB_OFFLINE=1`), mask PNGs byte-identical — sha256
# 3b6ad1f3b3557765761df38220258f4b64ebcfb252ea2b9b9239ed1805bf4d2f and
# 43f531a782558195bc1842a2ca45e5c07538d47f9888fcb3f0628d68e5461507, and a
# repeat run reproduced the first exactly. So `segment::AI_BACKEND_GENERATION`
# does NOT move for this: no cached alpha changes. It is not a byte copy of the
# file it replaces either: different key order inside each row, different
# formatting, 7,085 B against 7,084.
SKY_MODEL = "shi-labs/oneformer_ade20k_swin_large"
SKY_REVISION = "4a5bac8e64f82681a12db2e151a4c2f4ce6092b2"

# OUR class table, shipped in `python/` beside this script — no repo, no
# revision, nothing to fetch. Still digest-gated, for the reason every other
# asset here is: the pin is what turns "the file next to this script" into "the
# bytes this project audited", and a half-written checkout or a local edit is
# exactly as bad as a moving branch. `python/*.json` is pinned to LF in
# `.gitattributes` — without that, `core.autocrlf` would rewrite the newlines on
# a Windows checkout and this digest would read as tampering on a tree git
# considers identical.
SKY_CLASS_TABLE_FILE = "ade20k_class_table.json"

# Every file `OneFormerProcessor.from_pretrained` + `OneFormerForUniversal-
# Segmentation.from_pretrained` open, and nothing else: the repo's 949 MB
# `250_16_swin_l_oneformer_ade20k_160k.pth` is the ORIGINAL research checkpoint
# and `from_pretrained` never touches it, so pinning it would mean downloading
# it. Digests were computed over the bytes fetched this session; the one for
# `pytorch_model.bin` also equals the HF tree API's LFS `oid` at this revision
# (two independent derivations agreeing, the same double-check `_birefnet_cache`
# documents). `merges.txt` / `vocab.json` / `tokenizer_config.json` /
# `special_tokens_map.json` are the CLIP tokenizer tree — OneFormer is
# task-conditioned, so the text side is load-bearing, not decoration.
#
# `pytorch_model.bin` is a PICKLE (this revision predates safetensors and
# publishes no `.safetensors`), which is exactly why the digest matters more
# here than for a `.safetensors` sibling: transformers hands it to `torch.load`,
# and the gate below is what stands between those bytes and the interpreter.
SKY = {
    "repo": SKY_MODEL,
    "revision": SKY_REVISION,
    "files": {
        "pytorch_model.bin": {
            "sha256": "c0b2fe11dfecee6f2f1f315f466946e96f4e94813f3f6d660ff3747b83c28cc9",
            "bytes": 879517517,
        },
        "config.json": {
            "sha256": "27452b656a467dbdebdf879dc413d6f3facd2bfe3643824ae66c32c22884b4bd",
            "bytes": 84289,
        },
        "preprocessor_config.json": {
            "sha256": "49e2c8f207405d063cf7824f97c2814fa864f8f19ea9e02c9e20a9ff539c6d49",
            "bytes": 8709,
        },
        "merges.txt": {
            "sha256": "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
            "bytes": 524619,
        },
        "vocab.json": {
            "sha256": "e089ad92ba36837a0d31433e555c8f45fe601ab5c221d4f607ded32d9f7a4349",
            "bytes": 1059962,
        },
        "tokenizer_config.json": {
            "sha256": "968a6126200b3c8f68fe955d61da20f3537e641a1deb538dc39fdad142248d72",
            "bytes": 808,
        },
        "special_tokens_map.json": {
            "sha256": "c4864a9376a8401918425bed71fc14fc0e81f9b59ec45c1cf96cccb2df508eac",
            "bytes": 472,
        },
    },
}

SKY_CLASS_TABLE_PIN = {
    "sha256": "8b93934a55524e5a9320875336cb8bc6ba2a9e6307796e9f22e0cebbc89428d8",
    "bytes": 7085,
}


def _install_class_table(d):
    """Copy our own ADE20K class table into the verified directory `d`.

    A COPY, not a download — the file ships in `python/` (see the block above).
    The digest is still checked, and checked on the REPO file rather than the
    installed one: refusing before the bytes are in the loader's directory is
    the same order `_fetch_verified` uses, and it means a bad checkout cannot
    leave a poisoned cache behind for the next run to find.
    """
    try:
        from denoise import _sha256
    except ImportError as e:
        die(
            f"sky segmentation needs the shared sidecar digest helper from denoise.py ({e}) "
            "-> segment.py must sit beside denoise.py in python/"
        )
    src = os.path.join(os.path.dirname(os.path.abspath(__file__)), SKY_CLASS_TABLE_FILE)
    if not os.path.isfile(src):
        die(
            f"the ADE20K class table is missing -> {SKY_CLASS_TABLE_FILE} must sit beside "
            "segment.py in python/ (it ships with the sidecar; it is not downloaded)"
        )
    got, size = _sha256(src), os.path.getsize(src)
    if got != SKY_CLASS_TABLE_PIN["sha256"] or size != SKY_CLASS_TABLE_PIN["bytes"]:
        # ASCII-only: this line can reach a legacy-codepage console.
        die(
            f"the ADE20K class table {SKY_CLASS_TABLE_FILE} does not match its pin "
            f"(expected {SKY_CLASS_TABLE_PIN['sha256']} / {SKY_CLASS_TABLE_PIN['bytes']} B, "
            f"got {got} / {size} B) -> restore it from the repository"
        )
    dest = os.path.join(d, SKY_CLASS_TABLE_FILE)
    # A packaged or shared pinned cache may be read-only.  Once the installed
    # bytes already match the audited source, there is nothing to publish and
    # no reason to require write access merely to run inference.
    if os.path.isfile(dest):
        dest_got, dest_size = _sha256(dest), os.path.getsize(dest)
        if dest_got == SKY_CLASS_TABLE_PIN["sha256"] and dest_size == SKY_CLASS_TABLE_PIN["bytes"]:
            return
    # Unique temp per process, then an atomic rename: `load_metadata` opens this
    # path directly, so two sidecars racing must never expose a half-written one.
    tmp = f"{dest}.{os.getpid()}.part"
    with open(src, "rb") as f:
        payload = f.read()
    with open(tmp, "wb") as f:
        f.write(payload)
    os.replace(tmp, dest)


def _sky_cache(cache_dir):
    """Fetch every pinned OneFormer file into one directory and return it.

    Same shape and the same shared downloader as `_sam_cache` /
    `_birefnet_cache`. Our class table is installed into this directory too,
    under its own name, so `repo_path=<this dir>` makes `load_metadata` read it
    off disk instead of reaching for a repository.
    """
    try:
        from denoise import _fetch_verified
    except ImportError as e:
        die(
            f"sky segmentation needs the shared sidecar downloader from denoise.py ({e}) "
            "-> segment.py must sit beside denoise.py in python/"
        )
    d = os.path.join(cache_dir, "shi-labs--oneformer_ade20k_swin_large@" + SKY["revision"][:12])
    os.makedirs(d, exist_ok=True)
    for name, pin in SKY["files"].items():
        url = f"https://huggingface.co/{SKY['repo']}/resolve/{SKY['revision']}/{name}"
        _fetch_verified(
            url,
            os.path.join(d, name),
            pin["sha256"],
            pin["bytes"] + 4096,
            f"the OneFormer '{name}'",
        )
    _install_class_table(d)
    return d


def sky_mask(img_path: str, cache_dir: str):
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
            "(OneFormer ADE20K Swin-L, ~880 MB, downloads to python/weights on first run)"
        )
    import numpy as np
    from PIL import Image

    name = SKY_MODEL
    d = _sky_cache(cache_dir)
    # `local_files_only=True` on BOTH halves: the digest gate above is only a
    # gate if nothing downstream can quietly resolve a name over the network.
    #
    # `use_fast=False` is a PIN, not a default. transformers 5.2.0 swapped the
    # default to `OneFormerImageProcessorFast` and says so in a UserWarning that
    # ends "This is a breaking change and may produce slightly different
    # outputs" — and it does: measured on one 2000x1333 frame this session, the
    # two produce the same (1, 3, 640, 960) tensor with max |delta| 0.0175 and
    # mean |delta| 0.0023 in ImageNet-normalised units (~1/255 at the extreme).
    # That is a silent, transformers-version-dependent change in the BYTES of a
    # mask a saved recipe references and R21's fingerprinting compares. The slow
    # processor is the one this checkpoint was saved with and the one every
    # transformers before 5.x used, so pinning it keeps the mask stable rather
    # than tracking whichever default the installed library happens to carry. It
    # rides `segment::AI_BACKEND_GENERATION` 2, which has not shipped, so no
    # released alpha changes under anyone.
    processor = OneFormerProcessor.from_pretrained(
        d,
        local_files_only=True,
        use_fast=False,
        # BOTH, or the constructor reaches for a remote repo — see the block
        # above. `repo_path` pointing at a real directory is what makes
        # `load_metadata` take its local branch, and the name is OURS, not the
        # `ade20k_panoptic.json` the default `repo_path` would have served.
        repo_path=d,
        class_info_file=SKY_CLASS_TABLE_FILE,
    )
    model = OneFormerForUniversalSegmentation.from_pretrained(d, local_files_only=True)
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


def multi_class_masks(img_path: str, cache_dir: str, max_regions: int):
    """One OneFormer pass returning the sky and strongest ADE20K planes.

    The sky arithmetic intentionally mirrors ``sky_mask`` above.  The Rust
    bridge compares the sky plane byte-for-byte with a normal single-class
    call on calibration fixtures, so this function must not use the hard
    semantic-map argmax or a different resize path.
    """
    try:
        import torch
        from transformers import OneFormerForUniversalSegmentation, OneFormerProcessor
    except ImportError:
        die("multi-class sky segmentation needs transformers + torch -> pip install transformers")
    import numpy as np
    from PIL import Image

    d = _sky_cache(cache_dir)
    processor = OneFormerProcessor.from_pretrained(
        d, local_files_only=True, use_fast=False, repo_path=d,
        class_info_file=SKY_CLASS_TABLE_FILE,
    )
    model = OneFormerForUniversalSegmentation.from_pretrained(d, local_files_only=True)
    model.eval()
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    device = "cuda" if torch.cuda.is_available() else "cpu"
    model.to(device)
    labels = {int(i): str(l) for i, l in model.config.id2label.items()}
    exact = sorted(i for i, l in labels.items() if l.strip().lower() == "sky")
    loose = sorted(i for i, l in labels.items() if "sky" in l.lower())
    sky_ids = exact or loose
    if not sky_ids:
        die("OneFormer model has no sky class")
    sky_id = sky_ids[0]
    img = Image.open(img_path).convert("RGB")
    with torch.no_grad():
        inputs = processor(images=img, task_inputs=["semantic"], return_tensors="pt")
        inputs = {k: (v.to(device) if hasattr(v, "to") else v) for k, v in inputs.items()}
        out = model(**inputs)
        cls = out.class_queries_logits.softmax(dim=-1)[..., :-1]
        masks = out.masks_queries_logits.sigmoid()
        low_planes = {}
        for cid in range(cls.shape[-1]):
            lo = torch.einsum("bq,bqhw->bhw", cls[..., cid], masks).unsqueeze(1)
            # Preserve the single-sky operation order: interpolation first,
            # clamp second. Clamping here changed boundary bytes even though
            # the same query/class contraction was used.
            low_planes[cid] = lo
    # Sky is always first. Other classes are ordered by mean plane confidence,
    # then support, then ADE id; this order is deterministic across dict maps.
    # Rank on the model-resolution planes, then upsample only the requested
    # winners. Upsampling all 150 ADE planes at a 2048px input retained over a
    # gigabyte of float arrays even though at most four can leave the sidecar.
    candidates = []
    for cid, plane in low_planes.items():
        if cid == sky_id:
            continue
        mean = float(plane.float().clamp(0.0, 1.0).mean().item())
        if mean <= 0.0:
            continue
        support = float((plane.float() >= 0.5).float().mean().item())
        candidates.append((cid, mean, support))
    candidates = rank_candidates(candidates)
    selected_ids = [sky_id] + [cid for cid, _, _ in candidates[: max(0, max_regions - 1)]]
    selected = []
    for cid in selected_ids:
        up = torch.nn.functional.interpolate(
            low_planes[cid], size=(img.height, img.width), mode="bilinear", align_corners=False
        )[0, 0]
        selected.append((cid, up.float().clamp(0.0, 1.0).cpu().numpy()))
    return Image, np, labels, img, selected


def rank_candidates(candidates):
    """The product's candidate order: mean plane confidence desc, support
    desc, ADE class id asc — deterministic across dict maps."""
    return sorted(candidates, key=lambda x: (-x[1], -x[2], x[0]))


def plane_stats(arr8):
    """(mean_confidence, share) of one plane in [0, 1].

    ``share`` is the frame fraction the plane claims (its mean alpha).
    ``mean_confidence`` is the alpha's mass-weighted mean — how certain the
    plane is WHERE it claims pixels — so a small, crisp class outranks a broad,
    soft one in the Rust overlap policy (higher confidence first, then smaller
    area). Emitting the share under both names made "confidence" the area,
    which inverted that policy: the broadest plane won every overlap.
    """
    mass = float(arr8.sum())
    share = float(arr8.mean()) if arr8.size else 0.0
    confidence = float((arr8 * arr8).sum() / mass) if mass > 0.0 else 0.0
    return min(max(confidence, 0.0), 1.0), min(max(share, 0.0), 1.0)


def _self_test():
    """Exercise the product's ordering and plane statistics without loading
    torch or weights — the same functions the sidecar runs, not a copy."""
    import numpy as np
    got = [cid for cid, _, _ in rank_candidates([(7, 0.80, 0.25), (2, 0.80, 0.50), (5, 0.90, 0.10)])]
    expected = [5, 2, 7]
    if got != expected:
        raise AssertionError(f"semantic tie-break mismatch: {got} != {expected}")
    crisp = np.zeros((4, 4), dtype=np.float32)
    crisp[:2, :] = 1.0
    soft = np.full((4, 4), 0.5, dtype=np.float32)
    crisp_conf, crisp_share = plane_stats(crisp)
    soft_conf, soft_share = plane_stats(soft)
    if not (abs(crisp_share - 0.5) < 1e-6 and abs(soft_share - 0.5) < 1e-6):
        raise AssertionError(f"share must be the mean alpha: {crisp_share} {soft_share}")
    if not (abs(crisp_conf - 1.0) < 1e-6 and abs(soft_conf - 0.5) < 1e-6):
        raise AssertionError(f"confidence must be mass-weighted certainty: {crisp_conf} {soft_conf}")
    if crisp_conf <= soft_conf:
        raise AssertionError("a crisp plane must outrank a soft one of equal area")
    print("segment.py self-test: semantic tie-break mean, support, class-id OK; plane stats OK")


def write_multi_manifest(img_path: str, output: str, cache_dir: str, max_regions: int, mask_size: int, backend: str):
    Image, np, labels, img, selected = multi_class_masks(img_path, cache_dir, max_regions)
    from PIL import Image as PILImage
    import json
    import os
    base = os.path.dirname(os.path.abspath(output))
    stem = os.path.splitext(os.path.basename(output))[0]
    planes = []
    for cid, arr in selected:
        mask = PILImage.fromarray((arr * 255.0).clip(0, 255).astype(np.uint8), mode="L")
        if mask_size and max(mask.size) > mask_size:
            scale = mask_size / max(mask.size)
            mask = mask.resize((max(1, round(mask.width * scale)), max(1, round(mask.height * scale))), resample=PILImage.BILINEAR)
        name = f"{stem}.class-{cid}.png"
        path = os.path.join(base, name)
        tmp = f"{path}.{os.getpid()}.tmp.png"
        mask.save(tmp)
        with open(tmp, "rb+") as f:
            os.fsync(f.fileno())
        os.replace(tmp, path)
        arr8 = np.asarray(mask, dtype=np.float32) / 255.0
        mean_confidence, share = plane_stats(arr8)
        planes.append({
            "class_id": int(cid), "label": labels.get(cid, f"class-{cid}"),
            "mean_confidence": mean_confidence, "share": share,
            "path": name,
        })
    manifest = {"version": 1, "width": int(mask.width), "height": int(mask.height), "planes": planes}
    tmp = f"{output}.{os.getpid()}.tmp.json"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(manifest, f, separators=(",", ":"), sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, output)


# --- OBJECT: SAM 2.1, point-prompted from ReferencePoint + region dabs -------
#
# The third backend (R27 Batch-5, L-08 Arm C). Lightroom's `Mask/Image` carries
# `crs:ReferencePoint` on 218/218 real instances — the photographer's own
# normalised click — and `MaskSubType=0` means "the object (or background)
# there". R30 B3 additionally passes every ordered `d` coordinate from its
# optional region-hint gesture. SAM natively accepts that exact positive-point
# list; no inferred brush weights, boxes, negative points or dense mask enter it.
#
# LICENCE: "The SAM 2 model checkpoints, SAM 2 demo code (front-end and
# back-end), and SAM 2 training code are licensed under Apache 2.0"
# (https://github.com/facebookresearch/sam2). The HF repo tags `apache-2.0` and
# is NOT gated (verified against the HF API 2026-08-19). SAM 3 was rejected on
# mechanism as well as licence: its repo is gated, so "download on first run"
# cannot work without a token and a click-through.
#
# PINNING: every file fetched by us, gated on its sha256 + byte count, then
# loaded with `local_files_only=True` — `denoise.py`'s discipline, reused
# verbatim through its own `_fetch_verified`. R29 B4 put `subject` on the same
# footing and R29 C3/C4 put `sky` there too, so all four backends now share it
# and there is no weaker tier left to name here.
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

# Mirrored by `segment::MAX_GESTURE_PROMPT_POINTS`. This includes the leading
# ReferencePoint. The observed gesture is about 12 dabs; 2048 is a safety valve,
# never permission to truncate or sample the photographer's ordered evidence.
MAX_GESTURE_PROMPT_POINTS = 2048
GESTURE_PROMPT_VERSION = "gp1"
MAX_GESTURE_PROMPT_FILE_BYTES = 256 * 1024


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


def load_prompt_points(path: str, reference_point):
    """Read one bounded gp1 positive-point payload, preserving order/duplicates."""
    try:
        with open(path, "rb") as f:
            raw = f.read(MAX_GESTURE_PROMPT_FILE_BYTES + 1)
    except OSError as e:
        die(f"--prompt-file could not be read ({e})")
    if len(raw) > MAX_GESTURE_PROMPT_FILE_BYTES:
        die(
            f"--prompt-file exceeds the {MAX_GESTURE_PROMPT_FILE_BYTES}-byte IPC bound"
        )
    try:
        payload = json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError) as e:
        die(f"--prompt-file is not valid UTF-8 JSON ({e})")
    if not isinstance(payload, dict) or set(payload) != {"version", "points"}:
        die("--prompt-file must be an object with exactly 'version' and 'points'")
    if payload["version"] != GESTURE_PROMPT_VERSION:
        die(
            f"--prompt-file version must be {GESTURE_PROMPT_VERSION!r}, "
            f"got {payload['version']!r}"
        )
    raw_points = payload["points"]
    if not isinstance(raw_points, list) or not (
        2 <= len(raw_points) <= MAX_GESTURE_PROMPT_POINTS
    ):
        count = len(raw_points) if isinstance(raw_points, list) else "non-list"
        die(
            f"--prompt-file point count {count} is outside "
            f"2..={MAX_GESTURE_PROMPT_POINTS}"
        )
    points = []
    for i, point in enumerate(raw_points):
        if not isinstance(point, list) or len(point) != 2:
            die(f"--prompt-file point {i} must be a two-number [x,y] array")
        if any(isinstance(v, bool) or not isinstance(v, (int, float)) for v in point):
            die(f"--prompt-file point {i} must contain two numbers")
        point = (float(point[0]), float(point[1]))
        if not all(math.isfinite(v) for v in point):
            die(f"--prompt-file point {i} contains a non-finite coordinate")
        points.append(point)
    if points[0] != tuple(reference_point):
        die("--prompt-file point 0 must equal --reference-point")
    return points


def sam_prompt_values(points, edge=1024):
    """Nested values for [1,1,N,2] float points and [1,1,N] label-1 tensors."""
    mapped = [
        [
            float(min(max(point[0], 0.0), 1.0) * (edge - 1)),
            float(min(max(point[1], 0.0), 1.0) * (edge - 1)),
        ]
        for point in points
    ]
    return [[mapped]], [[[1 for _ in mapped]]]


def require_multi_point_capability(model, point_count):
    """Refuse an older/incompatible transformers API without a traceback."""
    if point_count <= 1:
        return
    import inspect

    capability = (
        "Sam2Model.forward accepting input_points [1,1,N,2] and "
        "input_labels [1,1,N] for N>1"
    )
    try:
        params = inspect.signature(model.forward).parameters.values()
    except (TypeError, ValueError) as e:
        die(f"multi-point SAM prompts need {capability}; this API cannot be inspected ({e})")
    names = {p.name for p in params}
    has_kwargs = any(p.kind == inspect.Parameter.VAR_KEYWORD for p in params)
    if not has_kwargs and not {"input_points", "input_labels"}.issubset(names):
        die(f"multi-point SAM prompts need {capability}; upgrade transformers")


def object_mask(img_path: str, points, cache_dir: str, min_iou: float):
    """Soft alpha for the object under ordered positive normalised points."""
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
    # Normalised recipe-frame points -> MODEL-frame pixels. ReferencePoint is
    # first and every ordered gesture dab follows. Orientation/quarter turns
    # already rewrote both in Rust; brush/AI coordinates are pre-lens, so there
    # is no crop/lens transform or pixel-centre offset here. Every point is
    # positive label 1. Negative labels, boxes, weights, centroids, sampling and
    # dense-mask prompts are deliberately not part of gp1.
    point_values, label_values = sam_prompt_values(points, edge)
    pts = torch.tensor(point_values, dtype=torch.float32)
    labels = torch.tensor(label_values, dtype=torch.long)
    require_multi_point_capability(model, len(points))
    with torch.no_grad():
        try:
            out = model(
                pixel_values=pixel_values.to(device),
                input_points=pts.to(device),
                input_labels=labels.to(device),
                multimask_output=True,
            )
        except (TypeError, ValueError) as e:
            if len(points) > 1:
                die(
                    "multi-point SAM prompts need Sam2Model.forward accepting "
                    f"input_points [1,1,N,2] and input_labels [1,1,N] for N>1 ({e})"
                )
            raise
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
            f"--min-iou {min_iou:.3f} at reference point "
            f"({points[0][0]:.4f}, {points[0][1]:.4f})",
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
    # NOT `required=True` any more, and the loss is covered below: --probe-backend
    # answers a question about THIS MACHINE, not about an image, so demanding a
    # photo it will never open would be a lie about the contract. Every other
    # invocation still gets the same "argument is required" refusal, just from an
    # explicit check instead of argparse.
    ap.add_argument("--input", help="source image (any PIL-readable format)")
    ap.add_argument("--output", help="mask PNG to write (8-bit grayscale)")
    ap.add_argument("--target", choices=["subject", "sky", "object"])
    ap.add_argument("--self-test", action="store_true", help="run dependency-free semantic ordering checks")
    ap.add_argument(
        "--probe-backend",
        action="store_true",
        help="with --target subject: report whether the pinned BiRefNet can run on this "
        "machine, then exit 0 without fetching weights or segmenting anything",
    )
    ap.add_argument(
        "--reference-point",
        help="crs:ReferencePoint verbatim, e.g. \"0.517578 0.260997\" (required for --target object)",
    )
    ap.add_argument(
        "--prompt-file",
        help="optional gp1 JSON positive-point payload for a subtype-0 gesture",
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
    ap.add_argument("--multi", action="store_true", help="emit a validated multi-class semantic manifest")
    ap.add_argument("--regions", type=int, default=4, help="maximum semantic class planes for --multi")
    ap.add_argument(
        "--infer-size",
        type=int,
        default=BIREFNET_EDGE,
        help="BiRefNet's square inference edge (--target subject; 1024 measured, 1536 fits)",
    )
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(__file__), "weights"))
    a = ap.parse_args()

    if a.self_test:
        _self_test()
        return
    if not a.target:
        die("--target is required unless --self-test is given")

    # THE CAPABILITY QUESTION, answered without touching an image or a weight
    # file. Costs one interpreter start plus three imports: measured 4.3 s when
    # torchvision IS installed (it pulls torch in), ~0.1 s when it is not,
    # against 0.06 s for a bare `python -c pass`.
    # `segment::birefnet_deps_available` spends that only when a CACHED alpha
    # says it was written by the fallback tier on a machine that could not run
    # BiRefNet — i.e. once, on the develop after the dependency lands.
    if a.probe_backend:
        # SUBJECT ONLY, and refusing the other two is the honest answer rather
        # than a limitation: `sky` and `object` have ONE backend each, so
        # "can the primary run here" is not a question their weights could
        # answer without being fetched, and printing "ok" for them would be a
        # verdict about nothing that a caller might later believe.
        if a.target != "subject":
            die(f"--probe-backend only applies to --target subject; {a.target} has one backend")
        why = birefnet_deps_error()
        print(f"segment.py: subject backend deps [{'missing' if why else 'ok'}]")
        if why:
            print(f"segment.py: {why}", file=sys.stderr)
        return
    for need in ("input", "output"):
        if not getattr(a, need):
            die(f"--{need} is required unless --probe-backend is given")

    if a.multi:
        if a.target != "sky":
            die("--multi is currently supported only for --target sky")
        # Rust's MAX_SEMANTIC_REGIONS is the authoritative application cap;
        # retain the sidecar's 1..4 validation so direct Python callers fail
        # closed before model work.
        if not (1 <= a.regions <= 4):
            die("--regions must be between 1 and 4")
        write_multi_manifest(a.input, a.output, a.cache, a.regions, a.mask_size,
                             "OneFormer ADE20K Swin-L " + SKY_REVISION[:12])
        print(f"segment.py: semantic manifest [OneFormer ADE20K Swin-L {SKY_REVISION[:12]}] -> {a.output}")
        return

    # Whether the PRIMARY backend's dependencies were present for THIS run, so
    # the caller's cache can tell "U^2-Net because this machine cannot run
    # BiRefNet" from "U^2-Net because the run failed for some other reason".
    # Only the first of those should ever be retried when the machine changes.
    deps_missing = None
    if a.target == "object":
        if not a.reference_point:
            die("--target object needs --reference-point (the sidecar's crs:ReferencePoint)")
        reference_point = parse_point(a.reference_point)
        points = (
            load_prompt_points(a.prompt_file, reference_point)
            if a.prompt_file
            else [reference_point]
        )
        mask = object_mask(a.input, points, a.cache, a.min_iou)
        backend = "SAM 2.1 Hiera-Large " + SAM["revision"][:12]
    elif a.target == "subject":
        if a.prompt_file:
            die("--prompt-file applies only to --target object")
        if a.infer_size < 32:
            die(f"--infer-size {a.infer_size} is too small to segment anything")
        # BEFORE the mask, not after: `subject_mask` imports torchvision itself
        # on the way to BiRefNet, so asking afterwards would answer "ok" for a
        # run that had already fallen back for a different reason.
        deps_missing = birefnet_deps_error()
        mask, backend = subject_mask(a.input, a.cache, a.infer_size)
    else:
        if a.prompt_file:
            die("--prompt-file applies only to --target object")
        mask = sky_mask(a.input, a.cache)
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
    # `segment::segment_file` reads the label out of the brackets and hands it
    # to the GUI and to the alpha cache — so the bracket is a CONTRACT, not
    # decoration, and `tests::the_backend_label_survives_the_sidecar_line`
    # pins this exact spelling from the Rust side.
    print(f"segment.py: {a.target} mask [{backend}] -> {a.output}")
    # SECOND LINE, same shape as --probe-backend's, so one parser reads both.
    # Only for the two-tier backend: printing "deps [ok]" for sky or object
    # would invite a reader to think those have a fallback tier too.
    if a.target == "subject":
        print(f"segment.py: subject backend deps [{'missing' if deps_missing else 'ok'}]")


if __name__ == "__main__":
    main()
