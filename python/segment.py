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


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--input", required=True, help="source image (any PIL-readable format)")
    ap.add_argument("--output", required=True, help="mask PNG to write (8-bit grayscale)")
    ap.add_argument("--target", required=True, choices=["subject", "sky"])
    a = ap.parse_args()

    mask = subject_mask(a.input) if a.target == "subject" else sky_mask(a.input)
    mask = mask.convert("L")
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
