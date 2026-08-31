#!/usr/bin/env python3
"""Style-embedding sidecar — image -> one 768-dim L2-normalised vector, as JSON.

Third member of the sidecar family (`denoise.py` = SCUNet, `segment.py` =
subject/sky/object masks). Same contract as both: the Rust side shells out
(src/embed.rs), this script does one job, writes its output atomically and
exits non-zero with a human-readable reason on stderr when it cannot.

Usage:
  python embed.py --input photo.png  --output vec.json        # one vector
  python embed.py --manifest paths.txt --output index.jsonl   # batch, one
                                                              # record per line
  python embed.py --text-manifest tags.jsonl --output txt.json  # N texts ->
                                                              # N text vectors

The vector joins the style index beside the 14-dim hand feature
(`src/style.rs`) — it does not replace it. An index built WITHOUT this sidecar
still loads and still retrieves; the embedding block simply contributes
nothing. That is why the field is optional on both sides.

MODEL — `google/siglip2-base-patch16-384`, HF licence tag `apache-2.0`
(verified against the HF API 2026-08-19, not gated, no remote code). Chosen
over OpenAI CLIP ViT-L/14 and LAION's OpenCLIP H/14 on the LICENCE, not on the
benchmark: this is a PUBLIC repository whose product is being copyright
registered, and both CLIP mirrors carry model cards saying "Any deployed use
case of the model - whether commercial or not - is currently out of scope"
(and the OpenAI HF mirror carries no licence tag at all). SigLIP 2 is
unambiguously Apache-2.0 and its abstract claims it "outperforms the original
SigLIP at every model scale on zero-shot classification, image-text retrieval
and dense-prediction tasks" — so the licence-clean pick costs nothing here.
Full comparison table: ~/.claude/plans/r27-materials/D1-ml-sidecar-design.md
section 2.2.

THE COST of base/384: of its 375.5 M parameters, the TEXT tower's token
embedding alone is 256,000 x 768 ~ 197 M — over half the file. Since the style
retrieval gained its text terms both towers are used, so the 1.50 GB is no
longer half dead weight; it was never separable in any case (upstream publishes
one monolithic `model.safetensors`, and range-reading selected tensors out of it
would defeat whole-file digest pinning).

PINNING — the discipline is `denoise.py`'s, reused rather than reimplemented:
this module imports `_fetch_verified` from it, so there is exactly ONE
download-and-verify implementation in the tree, and the progress lines it
prints announce themselves as `[denoise]` because that is the module they live
in. Every file below is fetched by us from a URL pinned to a 40-hex HF COMMIT
(a branch resolves to whatever upstream has today) and then gated on its own
sha256 + byte count, and the model is loaded from that local directory with
`local_files_only=True` — so `transformers` never opens a socket and the digest
gate is the only door. `trust_remote_code` is never used and never will be: it
downloads and EXECUTES upstream Python through HF's own cache, which our digest
gate would never see.
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
# copied. Python puts this script's own directory on sys.path[0], so a
# relocated embed.py without denoise.py beside it fails HERE with a sentence
# instead of somewhere inside the fetch.
try:
    import _sidecar
except ImportError as e:  # pragma: no cover - environment shape, not logic
    print(
        f"embed.py: cannot import the shared sidecar plumbing from _sidecar.py "
        f"({e}) — embed.py must sit beside _sidecar.py in python/.",
        file=sys.stderr,
    )
    sys.exit(2)


# The sidecar plumbing itself — logging, refusal, the pinned checkout's
# directory, the verified fetch and the atomic publish — is shared with the
# other two model sidecars (`_sidecar.py`). All that is per-script is WHICH
# model and WHICH name to say, so that is all that is bound here; every call
# site below keeps its own spelling.
from _sidecar import publish  # noqa: F401  (same signature; re-exported for the call sites)


def log(msg):
    _sidecar.log('embed', msg)


def die(msg):
    _sidecar.die('embed', msg)


def model_dir(cache_dir):
    return _sidecar.model_dir(MODEL, cache_dir)


def fetch_model(cache_dir):
    return _sidecar.fetch_model(MODEL, cache_dir, 'SigLIP 2')


# The HF repo, its pinned commit, and every file we fetch from it with the
# sha256 + exact byte count that file must have. Digests taken 2026-08-19 from
# the HF tree API at this revision (the LFS `oid` for model.safetensors) and,
# for the two small JSONs, computed over the actual downloaded bytes.
#
# `preprocessor_config.json` is pinned even though the transform below is
# hard-coded: it is the RECEIPT for that hard-coding (see `_check_preprocessing`).
# A revision that changed the resize filter or the normalisation would move
# every vector in the index while the model id stayed the same.
MODEL = {
    "repo": "google/siglip2-base-patch16-384",
    "revision": "f775b65a79762255128c981547af89addcfe0f88",
    "dim": 768,
    "files": {
        "model.safetensors": {
            "sha256": "ed72c0ace85020ae610fc817c2538b9cae5a477b012a50859c60af5b3ad30857",
            "bytes": 1501968264,
        },
        "config.json": {
            "sha256": "b1b2481aa448d0cd29001bcf5244e72fa8f916dd8348bd18abf9e5533105b96c",
            "bytes": 276,
        },
        "preprocessor_config.json": {
            "sha256": "fb2817d3523ca3b666c859f15320c7138416bc38ffc515e2963f78c868c51c90",
            "bytes": 394,
        },
        "tokenizer.json": {"sha256": "cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322", "bytes": 34363039},
        "tokenizer.model": {"sha256": "61a7b147390c64585d6c3543dd6fc636906c9af3865a5548f27f31aee1d4c8e2", "bytes": 4241003},
        # These two JSON files are ordinary Git blobs at the pinned revision
        # (not Xet objects). Their digests are kept here with the byte counts
        # so a cache filled from another revision cannot pass the gate.
        "tokenizer_config.json": {"sha256": "14afe629fe4959b9e0d51e1852b8d9f7ad074f90a1a7125a4fcdd17f06e78fc8", "bytes": 47164},
        "special_tokens_map.json": {"sha256": "baec30ea10906f16adb8c18af7a34023002c1746542612b8b41c9f09e1351351", "bytes": 636},
    },
}

# The preprocessing this sidecar implements, asserted against the pinned
# `preprocessor_config.json` at load time. PIL resample 2 is BILINEAR.
EXPECT_SIZE = 384
EXPECT_RESAMPLE = 2
EXPECT_MEAN = [0.5, 0.5, 0.5]
EXPECT_STD = [0.5, 0.5, 0.5]
TEXT_PADDING = "max_length"
TEXT_MAX_LENGTH = 64
# A pinned CONFIG VALUE, named as one. The tokenizer does not lowercase; the
# behaviour is asserted directly by `_case_is_preserved_self_test`, so this
# constant is only ever compared against the file it came from.
TEXT_CONFIG_DO_LOWER_CASE = True
# The ONLY tensor the text tower is fed. SigLIP is trained with a fixed
# 64-position context and NO padding mask — the pinned `tokenizer_config.json`
# says so itself (`"model_input_names": ["input_ids"]`, verified 2026-08-29
# against the cached file), which is why the loaded tokenizer emits one tensor
# even though its Gemma class declares two by default. This is a CONTRACT, not
# a convenience: feeding the tower a mask changes the pooled vector (measured
# cosine 0.72-0.78 against the unmasked one over five phrases), so two indexes
# built through two different doors are not comparable at all.
TEXT_MODEL_INPUT_NAMES = ["input_ids"]
# The one tokenizer class the pin names. The `Auto*` tokenizer factory is NOT a
# second door: on transformers 5.2.0 its SigLIP mapping resolves to None and the
# factory raises, and the hand-written fallback that used to sit beside it was
# how this sidecar grew two vector spaces in the first place.
TEXT_TOKENIZER_CLASS = "GemmaTokenizer"
# Named contract: the tokenizer must reproduce the checkpoint's training-time
# text path. Keeping this beside EXPECT_SIZE makes a pin change fail loudly
# instead of quietly changing every text cosine in an existing index.
#
# `do_lower_case` is a CONFIG VALUE this sidecar does not act on: the pinned
# config carries it (inherited Gemma metadata) and the fast tokenizer does not
# apply it. That is not asserted from the file — `--self-test` encodes an
# upper-case string and its lower-case twin and requires the ids to DIFFER, so
# the claim is checked against the tokenizer instead of restated from its
# metadata. SigLIP 2's own processor performs no canonicalisation either
# (`transformers/models/siglip2/processing_siglip2.py:25,27` sets only
# `padding="max_length"`, `max_length=64`).
TEXT_TOKENIZER_CONTRACT = {
    "padding": TEXT_PADDING,
    "max_length": TEXT_MAX_LENGTH,
    "do_lower_case": TEXT_CONFIG_DO_LOWER_CASE,
    "model_input_names": TEXT_MODEL_INPUT_NAMES,
    "tokenizer_class": TEXT_TOKENIZER_CLASS,
    "unk_token": "<unk>",
    "pad_token": "<pad>",
    "eos_token": "<eos>",
    "bos_token": "<bos>",
    "additional_special_tokens": ("<start_of_turn>", "<end_of_turn>"),
    "add_bos_token": False,
    "add_eos_token": True,
}
# Five golden strings and the ids the pinned tokenizer really produced
# (revision f775b65a79762255128c981547af89addcfe0f88, `tokenizer.json`
# sha256 cb9140fa…, read back 2026-08-29). Each list is the encoding up to and
# including the EOS id; everything after it is pad id 0 out to 64. A tokenizer
# swap that kept the file names but changed the graph moves these ids, and
# `--self-test` fails instead of the index quietly re-ranking.
TEXT_GOLDEN_IDS = {
    "a photo with warm golden tones": [235250, 2686, 675, 8056, 13658, 38622, 1],
    "a monochrome black-and-white photo": [235250, 103304, 2656, 235290, 639, 235290, 7384, 2686, 1],
    "a golden-hour photo": [235250, 13658, 235290, 14420, 2686, 1],
    "warmer, moodier, deeper blacks": [3216, 977, 235269, 35298, 11153, 235269, 22583, 61001, 1],
    "": [1],
}
TEXT_EOS_ID = 1
TEXT_PAD_ID = 0
TEXT_BOS_ID = 2
TEXT_SPECIAL_TOKENS = (
    TEXT_TOKENIZER_CONTRACT["unk_token"],
    TEXT_TOKENIZER_CONTRACT["pad_token"],
    TEXT_TOKENIZER_CONTRACT["eos_token"],
    TEXT_TOKENIZER_CONTRACT["bos_token"],
    *TEXT_TOKENIZER_CONTRACT["additional_special_tokens"],
)


def _tokenizer_config_problems(cfg):
    problems = []
    # SigLIP2's tokenizer metadata uses the Transformers sentinel for an
    # effectively unbounded tokenizer, while the model itself is trained for
    # 64 positions. The sidecar therefore pins max_length=64 explicitly and
    # only requires the metadata not to advertise a smaller context.
    try:
        model_max_length = int(cfg.get("model_max_length"))
    except (TypeError, ValueError):
        model_max_length = 0
    if model_max_length < TEXT_MAX_LENGTH:
        problems.append(f"model_max_length {cfg.get('model_max_length')} is below {TEXT_MAX_LENGTH}")
    if cfg.get("do_lower_case", TEXT_CONFIG_DO_LOWER_CASE) is not TEXT_CONFIG_DO_LOWER_CASE:
        problems.append("do_lower_case disagrees")
    # The mask exclusion lives in the PIN, not in our call: this is the line
    # that makes the loaded tokenizer emit one tensor instead of two.
    if list(cfg.get("model_input_names", [])) != TEXT_MODEL_INPUT_NAMES:
        problems.append(f"model_input_names {cfg.get('model_input_names')!r} != {TEXT_MODEL_INPUT_NAMES!r}")
    if cfg.get("tokenizer_class") != TEXT_TOKENIZER_CLASS:
        problems.append(f"tokenizer_class {cfg.get('tokenizer_class')!r} != {TEXT_TOKENIZER_CLASS!r}")
    for key in ("unk_token", "pad_token", "eos_token", "bos_token"):
        expected = TEXT_TOKENIZER_CONTRACT[key]
        if cfg.get(key) != expected:
            problems.append(f"{key} {cfg.get(key)!r} != {expected!r}")
    if tuple(cfg.get("additional_special_tokens", ())) != TEXT_TOKENIZER_CONTRACT["additional_special_tokens"]:
        problems.append("additional_special_tokens disagrees")
    for key in ("add_bos_token", "add_eos_token"):
        if cfg.get(key) is not TEXT_TOKENIZER_CONTRACT[key]:
            problems.append(f"{key} disagrees")
    return problems


def _check_tokenizer_config(d):
    with open(os.path.join(d, "tokenizer_config.json"), encoding="utf-8") as f:
        cfg = json.load(f)
    problems = _tokenizer_config_problems(cfg)
    if problems:
        raise SystemExit("refusing to embed text: pinned tokenizer config disagrees (" + "; ".join(problems) + ")")


def _check_preprocessing(d):
    """The hard-coded transform must be what the pinned processor config says.

    Not decoration: the vectors in a style index are only comparable to each
    other if every one of them was produced by the same resize + normalise. A
    silent change here would not crash anything — it would quietly re-rank the
    user's whole library.
    """
    with open(os.path.join(d, "preprocessor_config.json"), encoding="utf-8") as f:
        cfg = json.load(f)
    size = cfg.get("size") or {}
    problems = []
    if (size.get("height"), size.get("width")) != (EXPECT_SIZE, EXPECT_SIZE):
        problems.append(f"size {size} != {EXPECT_SIZE}x{EXPECT_SIZE}")
    if cfg.get("resample") != EXPECT_RESAMPLE:
        problems.append(f"resample {cfg.get('resample')} != {EXPECT_RESAMPLE}")
    if cfg.get("image_mean") != EXPECT_MEAN or cfg.get("image_std") != EXPECT_STD:
        problems.append(
            f"mean/std {cfg.get('image_mean')}/{cfg.get('image_std')} != "
            f"{EXPECT_MEAN}/{EXPECT_STD}"
        )
    if not cfg.get("do_resize") or not cfg.get("do_rescale") or not cfg.get("do_normalize"):
        problems.append("do_resize/do_rescale/do_normalize are not all true")
    if problems:
        raise SystemExit(
            "refusing to embed: the pinned preprocessor config does not match the "
            "transform this sidecar implements (" + "; ".join(problems) + "). "
            "Re-derive the transform before moving the revision pin."
        )


def load_model(cache_dir, device, fp16):
    import torch

    try:
        from transformers import SiglipModel
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "style embedding needs transformers + torch -> pip install transformers "
            "(SigLIP 2 base/384, ~1.5 GB, downloads to python/weights on first run)"
        )
    d = fetch_model(cache_dir)
    _check_preprocessing(d)
    _check_tokenizer_config(d)
    # Determinism knobs BEFORE the load, same reasoning as segment.py: an
    # embedding becomes a number in a saved index that ranks the user's
    # library, so cuDNN autotuning and TF32 picking different kernels run to
    # run would make the same photo retrieve different neighbours.
    torch.manual_seed(0)
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    # local_files_only: the digest gate above is the ONLY door. Without it a
    # missing file (or a cache we just refused) would be silently re-fetched
    # by transformers from the moving branch.
    model = SiglipModel.from_pretrained(d, local_files_only=True)
    model.eval()
    if fp16 and device.startswith("cuda"):
        model.half()
    model.to(device)
    return model


def load_tokenizer(d):
    """THE tokenizer door — one class, one call, no fallback.

    `GemmaTokenizerFast` is the class the pinned `tokenizer_config.json` names
    (`"tokenizer_class": "GemmaTokenizer"`; on transformers 5.2.0 the two
    spellings are the same object). It reads the pinned `tokenizer.json`
    directly, so it needs `tokenizers` and never `sentencepiece`, and
    `local_files_only` keeps the digest gate the only door.

    THERE WAS A SECOND DOOR AND IT WAS NOT EQUIVALENT. Until this batch the
    sidecar tried the `Auto*` factory first and fell back to a hand-written
    adapter over `tokenizer.json` on `AttributeError/ImportError/OSError`. On
    this machine (transformers 5.2.0, no sentencepiece) the factory raises
    `AttributeError` — its SigLIP mapping resolves to `None` — so the fallback
    always ran, and the fallback returned a padding mask that `embed_texts`
    forwarded into the tower. Same ids, DIFFERENT pooled vectors: cosine
    0.73 / 0.76 / 0.73 / 0.78 / 0.72 over five phrases. Two doors meant two
    vector spaces in one index, which is the one thing a cosine cannot survive.
    """
    from transformers import GemmaTokenizerFast

    return GemmaTokenizerFast.from_pretrained(d, local_files_only=True)


def self_test(cache_dir):
    """Check the cheap, pinned parts without importing torch/transformers.

    The model itself is intentionally optional in CI and on a fresh install;
    the test reports a clean skip in that case. If the cache exists, all files
    must be present at their exact pinned sizes and the tokenizer JSON must
    agree with the text contract above.
    """
    import re

    if TEXT_PADDING != "max_length" or TEXT_MAX_LENGTH != 64:
        raise SystemExit(
            "text_tower_padding_rule_is_the_pinned_one: sidecar must use "
            'padding="max_length" and max_length=64'
        )

    for name, pin in MODEL["files"].items():
        digest = pin.get("sha256", "")
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise SystemExit(f"tokenizer_files_are_pinned_by_digest: {name} has no 64-hex sha256")
        if not isinstance(pin.get("bytes"), int) or pin["bytes"] <= 0:
            raise SystemExit(f"tokenizer_files_are_pinned_by_digest: {name} has no byte count")
    d = model_dir(cache_dir)
    missing = [name for name in MODEL["files"] if not os.path.exists(os.path.join(d, name))]
    if missing:
        print("text_tower_padding_rule_is_the_pinned_one: SKIP (SigLIP 2 cache is absent)")
        print("tokenizer_files_are_pinned_by_digest: PASS (all pins are 64-hex with byte counts)")
        return
    for name, pin in MODEL["files"].items():
        path = os.path.join(d, name)
        size = os.path.getsize(path)
        if size != pin["bytes"]:
            raise SystemExit(f"tokenizer_files_are_pinned_by_digest: {name} is {size} bytes, expected {pin['bytes']}")
    with open(os.path.join(d, "tokenizer_config.json"), encoding="utf-8") as f:
        problems = _tokenizer_config_problems(json.load(f))
    if problems:
        raise SystemExit("text_tower_padding_rule_is_the_pinned_one: " + "; ".join(problems))
    print(
        "text_tower_padding_rule_is_the_pinned_one: PASS (max_length/64, one input tensor, "
        "and the special tokens)"
    )
    print("tokenizer_files_are_pinned_by_digest: PASS (cached files match size pins)")
    tokenizer = _golden_ids_self_test(d)
    if tokenizer is not None:
        _case_is_preserved_self_test(tokenizer)
        _text_forward_pass_self_test(cache_dir, d, tokenizer)


def _golden_ids_self_test(d):
    """The tokenizer's OWN output against pinned ids — the half a config read
    cannot cover.

    `tokenizer_config.json` says what the graph should do; only running it says
    what it does. Skips (loudly) when `transformers` is not installed, because
    the model half of this sidecar is optional in CI by construction.
    """
    try:
        tokenizer = load_tokenizer(d)
    except ImportError as error:
        print(f"the_text_door_is_the_pinned_tokenizer: SKIP (transformers absent: {error})")
        return None
    texts = list(TEXT_GOLDEN_IDS)
    ids = tokenize(tokenizer, texts).tolist()
    for text, row in zip(texts, ids):
        want = TEXT_GOLDEN_IDS[text]
        if row[: len(want)] != want:
            raise SystemExit(
                f"the_text_door_is_the_pinned_tokenizer: {text!r} tokenised to "
                f"{row[: len(want) + 2]}, pinned {want}"
            )
        if any(v != TEXT_PAD_ID for v in row[len(want):]):
            raise SystemExit(f"the_text_door_is_the_pinned_tokenizer: {text!r} is not pad-filled to {TEXT_MAX_LENGTH}")
        if row[len(want) - 1] != TEXT_EOS_ID:
            raise SystemExit(f"the_text_door_is_the_pinned_tokenizer: {text!r} has no EOS after the last token")
        if row[0] == TEXT_BOS_ID:
            raise SystemExit(f"the_text_door_is_the_pinned_tokenizer: {text!r} gained a BOS the pin forbids")
    print(
        f"the_text_door_is_the_pinned_tokenizer: PASS ({len(texts)} golden strings, "
        f"no BOS, EOS {TEXT_EOS_ID} after the last token, pad {TEXT_PAD_ID} to {TEXT_MAX_LENGTH})"
    )
    return tokenizer


def _case_is_preserved_self_test(tokenizer):
    """`do_lower_case` in the pinned config is not something this door does.

    Asserted against the TOKENIZER, not against its metadata: a receipt read
    back out of the same file it was copied from cannot be wrong, which is
    exactly why it was not worth having. If a re-pin ever did start
    canonicalising case, every text vector in every existing index would move
    and this is the line that says so.
    """
    upper, lower = "A PHOTO WITH DEEP BLACKS", "a photo with deep blacks"
    ids = tokenize(tokenizer, [upper, lower]).tolist()
    if ids[0] == ids[1]:
        raise SystemExit(
            "the_text_door_does_not_canonicalise_case: the tokenizer folded case, so "
            "do_lower_case is now BEHAVIOUR and every stored text vector moved"
        )
    print("the_text_door_does_not_canonicalise_case: PASS (case-distinct ids; do_lower_case is a config value only)")


def _text_forward_pass_self_test(cache_dir, d, tokenizer):
    """One real text vector out of the pinned tower.

    The checks above all stop at the tokenizer. This one loads the checkpoint
    and runs `text_model(input_ids=...)` once, so "the text half works" is a
    measurement rather than an inference from two file reads. Skips loudly
    without torch/transformers, like every other model-dependent check here.
    """
    try:
        import numpy as np
        import torch
    except ImportError as error:
        print(f"the_text_tower_answers_a_unit_vector: SKIP (torch/numpy absent: {error})")
        return
    del d
    model = load_model(cache_dir, "cpu", False)
    probe = next(iter(TEXT_GOLDEN_IDS))
    vecs = embed_texts(model, tokenizer, "cpu", [probe], np)
    if vecs.shape != (1, MODEL["dim"]):
        raise SystemExit(f"the_text_tower_answers_a_unit_vector: shape {vecs.shape}, expected (1, {MODEL['dim']})")
    norm = float(np.linalg.norm(vecs[0]))
    if abs(norm - 1.0) > 1e-4:
        raise SystemExit(f"the_text_tower_answers_a_unit_vector: |v| = {norm}, expected 1")
    del torch
    print(f"the_text_tower_answers_a_unit_vector: PASS ({MODEL['dim']}-dim, |v| = {norm:.6f})")


def preprocess(path, np):
    """One image -> float32 (3, 384, 384) in [-1, 1].

    The transform is spelled out rather than delegated to `SiglipImageProcessor`
    so the resample filter is a NAMED constant in this file: Pillow's default
    resample has changed across majors, and a default that moved would move
    every vector without a line of this repo changing.
    """
    from PIL import Image

    with Image.open(path) as im:
        # `convert` forces the full decode: a truncated JPEG raises here rather
        # than producing a half-grey frame that embeds as if it were a photo.
        im = im.convert("RGB")
        im = im.resize((EXPECT_SIZE, EXPECT_SIZE), resample=EXPECT_RESAMPLE)
        a = np.asarray(im, dtype=np.float32)
    a = a / 255.0
    a = (a - np.asarray(EXPECT_MEAN, dtype=np.float32)) / np.asarray(
        EXPECT_STD, dtype=np.float32
    )
    return np.ascontiguousarray(a.transpose(2, 0, 1))


def embed_batch(model, device, arrays, np):
    """(N, 3, 384, 384) -> (N, dim) float32, L2-normalised.

    Normalisation happens HERE, in the sidecar, so the Rust side never has to
    agree about it: a cosine similarity over vectors normalised by two
    different pieces of code is a bug that shows up as slightly wrong ranking
    and nothing else.
    """
    import torch

    with torch.no_grad():
        t = torch.from_numpy(np.stack(arrays)).to(device)
        if next(model.parameters()).dtype == torch.float16:
            t = t.half()
        # `vision_model(...).pooler_output`, not `model.get_image_features(...)`.
        # The two are the same tensor, but the WRAPPER's return type moved:
        # transformers 4.x returned the pooled tensor itself, 5.2.0 returns the
        # whole `BaseModelOutputWithPooling` (verified by reading
        # `modeling_siglip.SiglipModel.get_image_features`, which is now a bare
        # `return self.vision_model(...)`). Naming the tensor here means this
        # sidecar cannot silently start embedding a different quantity because
        # a library return type changed under it.
        v = model.vision_model(pixel_values=t).pooler_output
        # Compute the norm in fp32 even under --fp16: a half-precision norm of
        # a 768-dim vector loses digits the cosine would inherit.
        v = v.float()
        n = v.norm(dim=-1, keepdim=True).clamp_min(1e-12)
        v = v / n
    return v.cpu().numpy().astype(np.float32)


def tokenize(tokenizer, texts):
    """N strings -> the ONE tensor the text tower takes, shape (N, 64).

    The shape and the key set are asserted rather than assumed: an encoding
    that grew a second tensor would mean the pinned `model_input_names` moved,
    and the vectors this process writes would silently stop being comparable
    with every vector already in the user's index.
    """
    enc = tokenizer(list(texts), padding=TEXT_PADDING, max_length=TEXT_MAX_LENGTH,
                    truncation=True, return_tensors="pt")
    keys = set(enc.keys())
    if keys != set(TEXT_MODEL_INPUT_NAMES):
        raise SystemExit(
            f"refusing to embed text: the tokenizer returned {sorted(keys)}, and this "
            f"checkpoint's text tower takes exactly {TEXT_MODEL_INPUT_NAMES}"
        )
    ids = enc["input_ids"]
    if tuple(ids.shape) != (len(texts), TEXT_MAX_LENGTH):
        raise SystemExit(
            f"refusing to embed text: ids are {tuple(ids.shape)}, expected "
            f"({len(texts)}, {TEXT_MAX_LENGTH})"
        )
    return ids


def embed_texts(model, tokenizer, device, texts, np):
    import torch
    if not texts:
        return np.empty((0, MODEL["dim"]), dtype=np.float32)
    ids = tokenize(tokenizer, texts).to(device)
    with torch.no_grad():
        # `input_ids=` by NAME and nothing else — never `**enc`, which is how a
        # padding mask reached the tower and moved every text vector.
        out = model.text_model(input_ids=ids).pooler_output.float()
        out = out / out.norm(dim=-1, keepdim=True).clamp_min(1e-12)
    return out.cpu().numpy().astype(np.float32)


def cosine_scores(image_vec, text_vecs, np):
    if text_vecs is None or len(text_vecs) == 0:
        return []
    return [float(np.dot(image_vec, row)) for row in text_vecs]


def vec_json(np, v):
    """The vector as JSON array text, printed at float32's own shortest
    round-tripping precision.

    `json.dumps` on Python floats would print the DOUBLE widening of each
    float32 (0.036123457550048828 for a value carrying 8 significant digits) —
    roughly 20 bytes an element, 15 KB of an index record that says nothing
    extra. `str(np.float32(x))` is the shortest decimal that reads back as the
    same float32, which is exactly what the Rust side parses it into.
    """
    return "[" + ",".join(str(np.float32(x)) for x in v) + "]"


def main() -> None:
    ap = argparse.ArgumentParser(description="AutoShade style embedding (SigLIP 2)")
    ap.add_argument("--input", help="one image (any PIL-readable format)")
    ap.add_argument(
        "--manifest",
        help="newline-delimited image paths; writes one JSON record per line",
    )
    ap.add_argument("--manifest-jsonl", help='JSONL manifest of {"path": ..., "text": ...}')
    ap.add_argument("--text-file", help="UTF-8 file containing one text query")
    ap.add_argument(
        "--text-manifest",
        help='JSONL manifest of {"text": ...} — N strings in, N text vectors out, in the '
        "same order, from ONE process",
    )
    ap.add_argument("--vocab-file", help="UTF-8 phrases, one per line")
    ap.add_argument("--output")
    ap.add_argument("--batch", type=int, default=8, help="images per forward pass")
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(__file__), "weights"))
    ap.add_argument("--fp16", action="store_true")
    ap.add_argument("--cpu", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        self_test(a.cache)
        return
    if not a.output:
        die("--output is required")
    if sum(bool(v) for v in (a.input, a.manifest, a.manifest_jsonl, a.text_manifest)) != 1:
        die("give exactly one of --input, --manifest, --manifest-jsonl or --text-manifest")
    # REFUSED, not ignored. `--text-file` is one query for one image; in the
    # batch modes the per-record text comes from the manifest (or, for a pure
    # text batch, IS the manifest), and silently dropping the flag meant a
    # caller could ask for a text vector, get a clean exit and no text vector.
    if a.text_file and not a.input:
        die("--text-file applies to --input only; the batch modes carry their text in the manifest")

    import numpy as np
    import torch

    device = pick_device(a.cpu, "cuda:0")
    model = load_model(a.cache, device, a.fp16)
    tokenizer = load_tokenizer(model_dir(a.cache))
    vocab = []
    if a.vocab_file:
        with open(a.vocab_file, encoding="utf-8") as f:
            vocab = [line.strip() for line in f if line.strip()]
    vocab_vecs = embed_texts(model, tokenizer, device, vocab, np) if vocab else None
    dtype = "float16" if a.fp16 and device.startswith("cuda") else "float32"
    log(f"device={device} dtype={dtype} model={MODEL['repo']}")

    head = (
        f'"model":{json.dumps(MODEL["repo"])},'
        f'"revision":"{MODEL["revision"]}",'
        f'"dim":{MODEL["dim"]},"norm":"l2","dtype":"{dtype}"'
    )

    if a.text_manifest:
        # THE BATCH TEXT DOOR. It exists because the index builders need one
        # text vector per record and the alternative was one PROCESS per
        # record: the look build used to re-invoke this sidecar — 1.5 GB of
        # weights, re-loaded — once per photograph purely to embed its own tag
        # string. N strings, one load, one answer, in order.
        with open(a.text_manifest, encoding="utf-8") as f:
            texts = [str(json.loads(ln)["text"]) for ln in f if ln.strip()]
        if not texts:
            die(f"text manifest {a.text_manifest} lists no texts")
        rows = []
        # Chunked for the same reason the image path is: a manifest is
        # unbounded input, and a 5,000-string forward pass is not a budget
        # anyone signed off. 64 tokens x 8 x batch is small either way.
        chunk = max(1, int(a.batch)) * 8
        for start in range(0, len(texts), chunk):
            for v in embed_texts(model, tokenizer, device, texts[start : start + chunk], np):
                rows.append(vec_json(np, v))
        publish(a.output, "{" + head + ',"text_vectors":[' + ",".join(rows) + "]}\n")
        log(f"wrote {a.output} ({len(rows)} text vector(s))")
        return

    if a.input:
        v = embed_batch(model, device, [preprocess(a.input, np)], np)[0]
        if v.shape[0] != MODEL["dim"]:
            raise SystemExit(
                f"refusing to write a {v.shape[0]}-dim vector: the pinned model is "
                f"declared {MODEL['dim']}-dim, so the checkpoint is not the one we pinned"
            )
        rec = "{" + head + ',"vector":' + vec_json(np, v)
        if a.text_file:
            with open(a.text_file, encoding="utf-8") as f:
                value = f.read().strip()
            if value:
                rec += ',"text_vector":' + vec_json(np, embed_texts(model, tokenizer, device, [value], np)[0])
        if vocab_vecs is not None:
            rec += ',"vocab_scores":' + json.dumps(cosine_scores(v, vocab_vecs, np), separators=(",", ":"))
        publish(a.output, rec + "}\n")
        log(f"wrote {a.output} ({MODEL['dim']}-dim)")
        return

    out = []
    if a.manifest_jsonl:
        # FAIL-SOFT PER LINE, here too. `[json.loads(ln) for ln in f]` aborted
        # the whole rebuild on one malformed line, which is the opposite of the
        # contract the image loop below states and keeps.
        paths, texts_by_path = [], {}
        with open(a.manifest_jsonl, encoding="utf-8") as f:
            for lineno, ln in enumerate(f, 1):
                if not ln.strip():
                    continue
                try:
                    entry = json.loads(ln)
                    path = str(entry["path"])
                except (ValueError, KeyError, TypeError) as e:  # noqa: BLE001 - per-line failure IS the contract
                    out.append(json.dumps({"path": f"<line {lineno}>", "error": f"{type(e).__name__}: {e}"}))
                    continue
                paths.append(path)
                texts_by_path[path] = entry.get("text")
    else:
        with open(a.manifest, encoding="utf-8") as f:
            paths = [ln.strip() for ln in f if ln.strip()]
        texts_by_path = {}
    if not paths and not out:
        die(f"manifest {a.manifest or a.manifest_jsonl} lists no paths")
    # FAIL-SOFT PER LINE. The Rust index builder already skips individual
    # photos on decode/sidecar failure and keeps the run going; a batch mode
    # that aborted a 150-photo rebuild on one unreadable file would be a
    # regression against that. A refused DOWNLOAD is still fatal — that is a
    # property of the run, not of one photo.
    batch = max(1, int(a.batch))
    for start in range(0, len(paths), batch):
        chunk = paths[start : start + batch]
        arrays, live = [], []
        for p in chunk:
            try:
                arrays.append(preprocess(p, np))
                live.append(p)
            except Exception as e:  # noqa: BLE001 - per-line failure IS the contract
                out.append(json.dumps({"path": p, "error": f"{type(e).__name__}: {e}"}))
        if not arrays:
            continue
        vs = embed_batch(model, device, arrays, np)
        for p, v in zip(live, vs):
            rec = '{"path":' + json.dumps(p) + "," + head + ',"vector":' + vec_json(np, v)
            txt = texts_by_path.get(p)
            if txt:
                rec += ',"text_vector":' + vec_json(np, embed_texts(model, tokenizer, device, [txt], np)[0])
            if vocab_vecs is not None:
                rec += ',"vocab_scores":' + json.dumps(cosine_scores(v, vocab_vecs, np), separators=(",", ":"))
            out.append(rec + "}")
        log(f"{min(start + batch, len(paths))} / {len(paths)}")
    publish(a.output, "\n".join(out) + "\n")
    log(f"wrote {a.output} ({len(out)} record(s))")


if __name__ == "__main__":
    main()
