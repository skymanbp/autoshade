#!/usr/bin/env python3
"""AI segmentation sidecar — writes an 8-bit grayscale mask PNG (white = selected).

Companion to the Rust bridge (src/segment.rs), following the same sidecar
pattern as denoise.py: the Rust side shells out, this script does one job and
exits non-zero with a human-readable reason on stderr when it can't.

Usage:
  python segment.py --input photo.png --output mask.png --target subject|sky

Backends (weights auto-download to the USER's home cache on first run — nothing
is stored in the repo, consistent with .gitignore'ing python/weights):
  subject -> rembg, U^2-Net salient-object alpha  (pip install rembg; ~/.u2net)
  sky     -> OneFormer ADE20K Swin-L via transformers
             (pip install transformers torch; ~/.cache/huggingface)

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

  * SUBJECT, PINNED IN PLACE. `rembg.remove()` with no session argument
    resolves to whatever THAT INSTALL's default model is. On rembg 2.0.76 that
    is `new_session("u2net")` and U^2-Net is Apache-2.0 — fine — but upstream
    rembg has since moved its default to `bria-rmbg`, "released under a BRIA
    license that requires a paid agreement for commercial use", and warns that
    "Model weights carry their own licenses, independent of rembg's MIT
    license". So `pip install -U rembg` on any user's machine could silently
    swap a pay-to-use model in with zero change to Autoshop's source. The
    session is named explicitly below so the licence we checked is the licence
    that runs.

The output mask is soft (the model's own alpha / class probability), which the
render engine samples bilinearly — so edges come pre-feathered.
"""

import argparse
import os
import sys


def die(msg: str) -> None:
    print(f"segment.py: {msg}", file=sys.stderr)
    sys.exit(2)


def subject_mask(img_path: str):
    """Salient-subject alpha via an EXPLICITLY named U^2-Net rembg session."""
    try:
        from rembg import new_session, remove
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "subject segmentation needs rembg -> pip install rembg "
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
# PINNING is stricter here than the two older backends. Those resolve a pinned
# HF REVISION and let `transformers` fetch; this one fetches every file itself
# and gates each on its sha256 + byte count, then loads with
# `local_files_only=True` — `denoise.py`'s discipline, reused verbatim through
# its own `_fetch_verified`. The gap on `subject`/`sky` is registered in their
# own comments above, not closed here.
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
    # PREPROCESSING SPELLED OUT, not delegated to `Sam2Processor`. Two reasons,
    # and the first is not stylistic: transformers 5.2.0 resolves that
    # processor to `Sam2ImageProcessorFast`, which raises
    # "requires `torchvision` to be installed" — and torchvision is a
    # torch-version-coupled dependency this project has not taken. The second
    # is the embed.py reason: a resample filter that changed under us would
    # move every mask with nothing in this repo changing.
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
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(__file__), "weights"))
    a = ap.parse_args()

    if a.target == "object":
        if not a.reference_point:
            die("--target object needs --reference-point (the sidecar's crs:ReferencePoint)")
        mask = object_mask(a.input, parse_point(a.reference_point), a.cache, a.min_iou)
    elif a.target == "subject":
        mask = subject_mask(a.input)
    else:
        mask = sky_mask(a.input)
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
    print(f"segment.py: {a.target} mask -> {a.output}")


if __name__ == "__main__":
    main()
