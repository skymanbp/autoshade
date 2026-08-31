#!/usr/bin/env python3
"""Look-description sidecar - image -> one short sentence about its GRADE.

Fifth member of the sidecar family (`denoise.py` = SCUNet, `segment.py` =
subject/sky/object masks, `embed.py` = SigLIP 2 vectors, `correspond.py` =
DIFT fields). Same contract as all four: the Rust side shells out
(src/describe.rs), this script does one job, writes its output atomically and
exits non-zero with a human-readable reason on stderr when it cannot.

Usage:
  python describe.py --input frame.png --output desc.json      # one image
  python describe.py --manifest-jsonl frames.jsonl --output descs.jsonl
  python describe.py --self-test                               # pins + template

WHAT IT WRITES, and what it must never write. The description is about the
PHOTOGRAPHIC GRADE - white balance lean, tonality, contrast, saturation and
colour treatment, finishing, mood - and never about the subject. That is the
whole reason a local model was chosen over the attribute vocabulary alone:
`embed.py --vocab-file` can only score a photograph against 33 fixed phrases,
while a sentence can say something the vocabulary has no word for. The prompt
FENCES the subject out; a description that named one could not be detected
after the fact, so the Rust side treats every description as UNTRUSTED text
and it reaches a prompt only through the bounded, fenced reference blocks.

MODEL - `Qwen/Qwen3-VL-2B-Instruct`, HF licence tag `apache-2.0` (not gated,
no remote code). Chosen over the three obvious alternatives on the same
criteria the rest of this tree uses:

  * a PAID vision call (the user's own ruling, 2026-08-29): rejected. A
    description is computed once per exemplar for a whole library - 169
    photographs on this machine - and a per-image API call would put a bill
    behind a library rebuild that is otherwise free and offline. It would also
    send the user's photographs to a third party to produce a field that lives
    in a local index.
  * Florence-2 (`microsoft/Florence-2-*`): MIT, but its HF repos ship an
    `auto_map`, i.e. it can only be loaded by enabling `trust_remote_code` -
    which downloads and EXECUTES upstream Python through HF's own cache, the
    one thing the digest gate in this family can never see. Disqualified on
    the same rule that keeps `trust_remote_code` out of every other sidecar.
  * BLIP / BLIP-2 captioners: natively supported, but they are SUBJECT
    captioners ("a man standing on a beach"). The one thing this sidecar must
    not produce is a subject, and a model whose training objective IS the
    subject is the wrong instrument even with a prompt in front of it.

Qwen3-VL is `architectures = ["Qwen3VLForConditionalGeneration"]` with NO
`auto_map`, and transformers 5.2.0 (this machine's version) implements it
natively - verified by reading the installed
`transformers/models/qwen3_vl/modeling_qwen3_vl.py` (class
`Qwen3VLForConditionalGeneration`, line 1323) and
`transformers/models/qwen3_vl/processing_qwen3_vl.py` (class
`Qwen3VLProcessor`, line 46). So the checkpoint loads through NAMED library
classes with `local_files_only=True` and `trust_remote_code` never appears.

THE NAMED CLASS, NOT THE `Auto*` FACTORY. The task book called this the
"`Qwen3VLForConditionalGeneration` + `AutoProcessor` native path"; what ships
is the same path through the named `Qwen3VLProcessor`. `embed.py`'s own root
cause (S1 F-11) was an `Auto*` factory silently resolving to a SECOND door
with different behaviour, and every other sidecar in this family already
constructs its processor by name (`OneFormerProcessor`, `CLIPTokenizer`,
`Sam2Model`). A factory here would be the only one, and the only place a
transformers upgrade could change which class opens the pinned files.

PINNING - `denoise.py`'s discipline, reused rather than reimplemented: this
module imports `_fetch_verified` from it, so there is exactly ONE
download-and-verify implementation in the tree, and the progress lines it
prints announce themselves as `[denoise]` because that is the module they live
in. Every file below is fetched from a URL pinned to a 40-hex HF COMMIT and
gated on its own sha256 + exact byte count, and the model is loaded from that
local directory - the digest is the only door.
"""

import argparse
import json
import os
import re
import sys

# The shared device rule. `_device.py` ships beside this script in `python/`,
# the same way `segment.py` requires its ADE20K class table to; `sys.path[0]`
# is the running script's own directory, which the Rust side resolves against
# the program's tree and never the working directory.
from _device import pick_device

# The download/verify half of the sidecar contract, imported rather than
# copied - same rule as embed.py and correspond.py: a relocated describe.py
# without denoise.py beside it fails HERE with a sentence instead of somewhere
# inside the fetch.
try:
    from denoise import _fetch_verified
except ImportError as e:  # pragma: no cover - environment shape, not logic
    print(
        f"describe.py: cannot import the shared sidecar downloader from denoise.py "
        f"({e}) - describe.py must sit beside denoise.py in python/.",
        file=sys.stderr,
    )
    sys.exit(2)


def log(msg):
    print(f"[describe] {msg}", file=sys.stderr, flush=True)


def die(msg: str) -> None:
    print(f"describe.py: {msg}", file=sys.stderr)
    sys.exit(2)


# The HF repo, its pinned commit, and every file we fetch from it with the
# sha256 + exact byte count that file must have. Digests computed 2026-08-29
# over the bytes actually downloaded from
# https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct/resolve/<revision>/<file>.
# `model.safetensors` is the repo's only LFS object and the HF tree API's
# `lfs.oid` for it EQUALS the digest computed here (sha256 is what an LFS oid
# is); the other nine are plain git blobs with no oid to compare against, so
# for those the computed digest is the only receipt.
MODEL = {
    "repo": "Qwen/Qwen3-VL-2B-Instruct",
    "revision": "89644892e4d85e24eaac8bacfd4f463576704203",
    "files": {
        "model.safetensors": {
            "sha256": "7de1838c87a5349b016c26a1c3f7d2bc400a3d485f95ef39a7059ffd734977a0",
            "bytes": 4255140312,
        },
        "config.json": {
            "sha256": "bec4b3d446efa05807365c9e1cec03ac590836879d02f3a6da879971154bdd3b",
            "bytes": 1505,
        },
        "generation_config.json": {
            "sha256": "1e241830b48b397cb0900101421df5450baddc7adf01e5fc86b5615865f3bae4",
            "bytes": 269,
        },
        "chat_template.json": {
            "sha256": "6f8a6a55027e3da5160105556cda5dd69f6423f1c32645f6730d32de7773d0c4",
            "bytes": 5502,
        },
        "preprocessor_config.json": {
            "sha256": "27225450ac9c6529872ee1924fcb0962ff5634834f817040f444118116f4e516",
            "bytes": 390,
        },
        "video_preprocessor_config.json": {
            "sha256": "7768af27c1fafa9cc9011c1dc20067e03f8915e03b63504550e11d5066986d13",
            "bytes": 385,
        },
        "tokenizer.json": {
            "sha256": "a5d85b6dcc535e6b93115a9ef287e6132fdbf30270da6218194ba742261173c7",
            "bytes": 7032403,
        },
        "tokenizer_config.json": {
            "sha256": "c2da771801886ad9ae98181793ffd3dfb7f1af30f6f7c6a4e15d7dbba52e2399",
            "bytes": 10868,
        },
        "vocab.json": {
            "sha256": "ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910",
            "bytes": 2776833,
        },
        "merges.txt": {
            "sha256": "599bab54075088774b1733fde865d5bd747cbcc7a547c5bc12610e874e26f5e3",
            "bytes": 1671839,
        },
    },
}

# THE PROMPT, one constant, English. Changing a single character changes every
# description in every index built afterwards, which is why PROMPT_VERSION sits
# beside it and travels in every record: the Rust description cache keys on
# (frame digest, model, revision, prompt_version), so a prompt edit that forgot
# to move the version would serve descriptions from the OLD prompt for ever.
PROMPT = (
    "Describe ONLY the photographic grade of this image in at most 45 words: "
    "white-balance lean, tonality (blacks, shadows, highlights), contrast, "
    "saturation and colour treatment, finishing (haze, clarity, grain) and mood. "
    "Do not name subjects, places, objects or people. Output one plain sentence."
)
PROMPT_VERSION = 1

# Greedy, and pinned as constants so the Rust side can assert them from the
# source. `generation_config.json` at this revision ships `do_sample: true`
# with a temperature/top_p/top_k triple; a description that changed run to run
# would make the description cache a lie and two rebuilds of one library
# incomparable. The sampling knobs are therefore passed as None explicitly -
# leaving them in place merely produces a warning while the values sit unused.
MAX_NEW_TOKENS = 80
DO_SAMPLE = False
NUM_BEAMS = 1
SEED = 0

# The description hygiene this sidecar applies before writing. The Rust door
# (`describe::sanitize_desc`) applies the SAME bounds again, because a sidecar
# is a program on disk and the bound that protects the prompt has to hold even
# if this file is replaced.
MAX_DESC_CHARS = 512

# The processor's image budget. `smart_resize` UP-scales when a frame carries
# fewer than `min_pixels` (its `elif h_bar * w_bar < min_pixels` branch), and
# the pinned `preprocessor_config.json` asks for 65,536 = 256x256 - so a small
# staged frame would be enlarged, spending tokens on interpolation. The frames
# this sidecar is handed are `style::EMBED_FRAME_EDGE` = 512 px on the long
# edge, so 512x512 is the ceiling and one merged patch (32x32, i.e. patch_size
# 16 x merge_size 2) is the floor: between those two, `smart_resize` only ever
# rounds to the patch grid.
FRAME_EDGE = 512
MAX_PIXELS = FRAME_EDGE * FRAME_EDGE
MIN_PIXELS = 32 * 32

# The pinned preprocessing this sidecar asserts before it describes anything -
# embed.py's `_check_preprocessing`, for the same reason: a revision that
# changed the normalisation or the patch geometry would move every description
# while the model id stayed the same.
EXPECT_MEAN = [0.5, 0.5, 0.5]
EXPECT_STD = [0.5, 0.5, 0.5]
EXPECT_PATCH_SIZE = 16
EXPECT_MERGE_SIZE = 2
EXPECT_TEMPORAL_PATCH_SIZE = 2
EXPECT_IMAGE_PROCESSOR = "Qwen2VLImageProcessorFast"
EXPECT_PROCESSOR_CLASS = "Qwen3VLProcessor"
# The one architecture the pin names. A checkpoint that declared another one
# would load through a different graph and answer different text.
EXPECT_ARCHITECTURE = "Qwen3VLForConditionalGeneration"
# The chat template's vision markers, asserted against the RENDERED prompt in
# `--self-test`: this is what proves the pinned template really puts the image
# where the model expects it, rather than dropping it silently.
VISION_TOKENS = ("<|vision_start|>", "<|image_pad|>", "<|vision_end|>")
CHAT_TURN_TOKENS = ("<|im_start|>", "<|im_end|>")

# Cf (format) characters that survive a printable filter and are exactly what a
# prompt-injection payload hides behind: the soft hyphen, the zero-width
# joiners, the bidi overrides, the interlinear annotations and the BOM. Spelled
# by CODE POINT because the characters themselves are invisible in an editor,
# so a literal class could not be reviewed.
_INVISIBLE = re.compile(
    "[\u00ad\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u2069\ufeff]"
)


def model_dir(cache_dir):
    """One directory per pinned (repo, revision) - a re-pin never reuses a
    cache filled at the old revision, and the digest gate never has to be the
    thing that catches it."""
    slug = MODEL["repo"].replace("/", "--")
    return os.path.join(cache_dir, f"{slug}@{MODEL['revision'][:12]}")


def fetch_model(cache_dir):
    d = model_dir(cache_dir)
    os.makedirs(d, exist_ok=True)
    for name, pin in MODEL["files"].items():
        url = (
            f"https://huggingface.co/{MODEL['repo']}/resolve/"
            f"{MODEL['revision']}/{name}"
        )
        # The same small slack denoise.py leaves on its cap: an overshoot
        # message should be about the ENDPOINT, not an off-by-one.
        _fetch_verified(
            url,
            os.path.join(d, name),
            pin["sha256"],
            pin["bytes"] + 4096,
            f"the Qwen3-VL '{name}'",
        )
    return d


def _preprocessing_problems(cfg):
    problems = []
    if cfg.get("image_mean") != EXPECT_MEAN or cfg.get("image_std") != EXPECT_STD:
        problems.append(
            f"mean/std {cfg.get('image_mean')}/{cfg.get('image_std')} != "
            f"{EXPECT_MEAN}/{EXPECT_STD}"
        )
    for key, want in (
        ("patch_size", EXPECT_PATCH_SIZE),
        ("merge_size", EXPECT_MERGE_SIZE),
        ("temporal_patch_size", EXPECT_TEMPORAL_PATCH_SIZE),
    ):
        if cfg.get(key) != want:
            problems.append(f"{key} {cfg.get(key)!r} != {want}")
    if cfg.get("image_processor_type") != EXPECT_IMAGE_PROCESSOR:
        problems.append(
            f"image_processor_type {cfg.get('image_processor_type')!r} != "
            f"{EXPECT_IMAGE_PROCESSOR!r}"
        )
    if cfg.get("processor_class") != EXPECT_PROCESSOR_CLASS:
        problems.append(
            f"processor_class {cfg.get('processor_class')!r} != {EXPECT_PROCESSOR_CLASS!r}"
        )
    return problems


def _check_preprocessing(d):
    """The pinned processor config must be the transform this sidecar bounds.

    Not decoration. MIN_PIXELS/MAX_PIXELS above override the pinned `size`,
    and the patch geometry is what turns those pixel counts into a token
    count - so a revision that moved `patch_size` or `merge_size` would
    silently change how much of the frame the model actually sees.
    """
    with open(os.path.join(d, "preprocessor_config.json"), encoding="utf-8") as f:
        cfg = json.load(f)
    problems = _preprocessing_problems(cfg)
    if problems:
        raise SystemExit(
            "refusing to describe: the pinned preprocessor config does not match the "
            "transform this sidecar bounds (" + "; ".join(problems) + "). "
            "Re-derive the bounds before moving the revision pin."
        )


def _check_architecture(d):
    """The pinned checkpoint is the graph this sidecar loads BY NAME, and it
    carries no `auto_map`.

    `auto_map` is what makes a repo need `trust_remote_code`. This checkpoint
    has none, and a re-pin that grew one must fail HERE rather than at a
    `from_pretrained` that would then ask to execute upstream Python.
    """
    with open(os.path.join(d, "config.json"), encoding="utf-8") as f:
        cfg = json.load(f)
    arch = list(cfg.get("architectures") or [])
    if arch != [EXPECT_ARCHITECTURE]:
        raise SystemExit(
            f"refusing to describe: the pinned checkpoint declares architectures {arch!r}, "
            f"and this sidecar loads {EXPECT_ARCHITECTURE} by name"
        )
    if cfg.get("auto_map"):
        raise SystemExit(
            "refusing to describe: the pinned checkpoint grew an `auto_map`, which means "
            "loading it would execute upstream Python that the digest gate never sees"
        )


def load_processor(d):
    """THE processor door - one named class, one call, no factory.

    `min_pixels`/`max_pixels` override the pinned `size` for the reason spelled
    out on the constants: the pinned floor would UP-scale a staged frame.
    `local_files_only` keeps the digest gate the only door.
    """
    from transformers import Qwen3VLProcessor

    return Qwen3VLProcessor.from_pretrained(
        d,
        local_files_only=True,
        min_pixels=MIN_PIXELS,
        max_pixels=MAX_PIXELS,
    )


def load_model(cache_dir, device, bf16):
    import torch

    try:
        from transformers import Qwen3VLForConditionalGeneration
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "look descriptions need transformers >= 5.2 + torch -> pip install -U transformers "
            "(Qwen3-VL-2B-Instruct, ~4.3 GB, downloads to python/weights on first run)"
        )

    d = fetch_model(cache_dir)
    _check_architecture(d)
    _check_preprocessing(d)
    # Determinism knobs BEFORE the load, same reasoning as embed.py and
    # segment.py: a description becomes a string in a saved index AND a vector
    # that ranks the user's library, so cuDNN autotuning and TF32 picking
    # different kernels run to run would make the same photo describe itself
    # differently and retrieve different neighbours.
    torch.manual_seed(SEED)
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    # bfloat16, not float16: this checkpoint's own `config.json` declares
    # `"dtype": "bfloat16"`, so bf16 is the precision it was trained and
    # released in - and bf16 keeps fp32's exponent range, which is what makes
    # a 2 B decoder safe to run with no loss scaling at all.
    dtype = torch.bfloat16 if (bf16 and device.startswith("cuda")) else torch.float32
    model = Qwen3VLForConditionalGeneration.from_pretrained(
        d, dtype=dtype, local_files_only=True
    )
    model.eval()
    model.to(device)
    return model, d, dtype


def chat_text(processor):
    """The rendered prompt the model is fed, from the PINNED chat template.

    A separate function so `--self-test` can assert what the template did
    without loading 4.3 GB of weights.
    """
    messages = [
        {
            "role": "user",
            "content": [{"type": "image"}, {"type": "text", "text": PROMPT}],
        }
    ]
    return processor.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True
    )


def sanitize(text):
    """One model answer -> the single bounded line a record may carry.

    Control characters (newlines included) become spaces, the invisible Cf
    block is stripped, runs of whitespace collapse, and the result is cut to
    MAX_DESC_CHARS. An empty answer stays empty and the caller reports it as a
    per-line failure rather than writing a description that says nothing.
    """
    if not isinstance(text, str):
        return ""
    cleaned = "".join(" " if (ord(c) < 0x20 or ord(c) == 0x7F) else c for c in text)
    cleaned = _INVISIBLE.sub(" ", cleaned)
    cleaned = re.sub(r"\s+", " ", cleaned).strip()
    return cleaned[:MAX_DESC_CHARS]


def describe_one(model, processor, device, path):
    """One image -> one sanitized sentence.

    ONE image per forward pass, deliberately. Batched generation needs left
    padding and a padding mask, and a padded batch does not produce token-for-
    token the same greedy answer as the same image alone - which would make a
    description depend on which other photographs happened to share its build.
    The cost this leaves on the table is small: the expensive part is the
    4.3 GB model LOAD, and the manifest door below pays that once for a whole
    library.
    """
    import torch
    from PIL import Image

    with Image.open(path) as im:
        # `convert` forces the full decode: a truncated PNG raises here rather
        # than describing a half-grey frame as if it were a photograph.
        image = im.convert("RGB")
        inputs = processor(
            text=[chat_text(processor)], images=[image], return_tensors="pt"
        )
    inputs = {k: (v.to(device) if hasattr(v, "to") else v) for k, v in inputs.items()}
    with torch.no_grad():
        out = model.generate(
            **inputs,
            max_new_tokens=MAX_NEW_TOKENS,
            do_sample=DO_SAMPLE,
            num_beams=NUM_BEAMS,
            # Explicit None: the pinned generation_config carries a sampling
            # triple, and leaving it in place under do_sample=False merely
            # warns while the values sit unused.
            temperature=None,
            top_p=None,
            top_k=None,
        )
    # Only the NEW tokens: the answer, not the prompt echoed back.
    prompt_len = inputs["input_ids"].shape[1]
    text = processor.batch_decode(
        out[:, prompt_len:], skip_special_tokens=True, clean_up_tokenization_spaces=False
    )[0]
    return sanitize(text)


def publish(path, text):
    """tmp + fsync + os.replace, like every other sidecar (L03): the caller
    stages this file and an index build reads it, so a payload still in the
    page cache must not vanish under a power cut."""
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


def self_test(cache_dir):
    """Check the cheap, pinned parts - and the chat template's real output.

    The 4.3 GB checkpoint is intentionally optional in CI and on a fresh
    install; the test reports a clean skip in that case. What it does NOT skip
    when transformers is present is the template: `apply_chat_template` needs
    only the tokenizer + `chat_template.json`, and a template that dropped the
    image placeholder would produce a description of nothing at all.
    """
    if not PROMPT.strip() or "Do not name subjects" not in PROMPT:
        raise SystemExit(
            "the_prompt_fences_the_subject_out: the prompt constant no longer forbids subjects"
        )
    if not isinstance(PROMPT_VERSION, int) or PROMPT_VERSION < 1:
        raise SystemExit(
            "the_prompt_fences_the_subject_out: PROMPT_VERSION must be a positive integer"
        )
    if DO_SAMPLE or NUM_BEAMS != 1:
        raise SystemExit(
            "decoding_is_greedy_and_deterministic: do_sample/num_beams are not the greedy pair"
        )
    print(
        f"the_prompt_fences_the_subject_out: PASS (v{PROMPT_VERSION}, {len(PROMPT)} chars, "
        "subjects forbidden)"
    )
    print(
        f"decoding_is_greedy_and_deterministic: PASS (do_sample={DO_SAMPLE}, "
        f"num_beams={NUM_BEAMS}, max_new_tokens={MAX_NEW_TOKENS}, seed {SEED})"
    )

    for name, pin in MODEL["files"].items():
        digest = pin.get("sha256", "")
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise SystemExit(f"every_pinned_file_has_a_digest: {name} has no 64-hex sha256")
        if not isinstance(pin.get("bytes"), int) or pin["bytes"] <= 0:
            raise SystemExit(f"every_pinned_file_has_a_digest: {name} has no byte count")
    print(
        f"every_pinned_file_has_a_digest: PASS ({len(MODEL['files'])} files, all 64-hex with "
        "byte counts)"
    )

    # The sanitizer, on the four answers that would break the door.
    if sanitize("a warm,\nlifted grade\t") != "a warm, lifted grade":
        raise SystemExit("the_description_is_one_bounded_line: newlines/tabs are not collapsed")
    if len(sanitize("x" * (MAX_DESC_CHARS + 400))) != MAX_DESC_CHARS:
        raise SystemExit("the_description_is_one_bounded_line: the character cap does not hold")
    if sanitize("   ") != "":
        raise SystemExit("the_description_is_one_bounded_line: a blank answer is not empty")
    if sanitize("cool\u202eblue\u200b tones") != "cool blue tones":
        raise SystemExit("the_description_is_one_bounded_line: invisible Cf characters survive")
    print(f"the_description_is_one_bounded_line: PASS (<= {MAX_DESC_CHARS} chars, single line)")

    d = model_dir(cache_dir)
    missing = [name for name in MODEL["files"] if not os.path.exists(os.path.join(d, name))]
    if missing:
        print("the_pinned_files_are_present_at_their_sizes: SKIP (Qwen3-VL cache is absent)")
        print("the_chat_template_places_the_image: SKIP (Qwen3-VL cache is absent)")
        return
    for name, pin in MODEL["files"].items():
        size = os.path.getsize(os.path.join(d, name))
        if size != pin["bytes"]:
            raise SystemExit(
                f"the_pinned_files_are_present_at_their_sizes: {name} is {size} bytes, "
                f"expected {pin['bytes']}"
            )
    _check_architecture(d)
    _check_preprocessing(d)
    print(
        f"the_pinned_files_are_present_at_their_sizes: PASS ({len(MODEL['files'])} files, "
        f"{EXPECT_ARCHITECTURE}, mean/std {EXPECT_MEAN[0]}, patch {EXPECT_PATCH_SIZE} x merge "
        f"{EXPECT_MERGE_SIZE})"
    )
    try:
        processor = load_processor(d)
    except ImportError as error:
        print(f"the_chat_template_places_the_image: SKIP (transformers absent: {error})")
        return
    rendered = chat_text(processor)
    for token in VISION_TOKENS + CHAT_TURN_TOKENS:
        if token not in rendered:
            raise SystemExit(
                f"the_chat_template_places_the_image: the rendered prompt has no {token!r} - "
                "the pinned template would feed the model no image"
            )
    if PROMPT not in rendered:
        raise SystemExit(
            "the_chat_template_places_the_image: the rendered prompt does not carry the prompt "
            "constant verbatim"
        )
    if not rendered.rstrip().endswith("assistant"):
        raise SystemExit(
            "the_chat_template_places_the_image: the rendered prompt does not end on the "
            "assistant turn, so the model would continue the USER's message"
        )
    # ...and the image budget is really the one the constants declare.
    size = processor.image_processor.size
    if (size.get("shortest_edge"), size.get("longest_edge")) != (MIN_PIXELS, MAX_PIXELS):
        raise SystemExit(
            f"the_chat_template_places_the_image: the processor pixel budget is {size}, "
            f"expected shortest {MIN_PIXELS} / longest {MAX_PIXELS}"
        )
    print(
        "the_chat_template_places_the_image: PASS (vision markers, verbatim prompt, assistant "
        f"turn, pixel budget {MIN_PIXELS}..{MAX_PIXELS})"
    )


def main() -> None:
    ap = argparse.ArgumentParser(
        description="AutoShade look description (Qwen3-VL-2B-Instruct)"
    )
    ap.add_argument("--input", help="one image (any PIL-readable format)")
    ap.add_argument(
        "--manifest-jsonl",
        help='JSONL manifest of {"path": ...}; writes one JSON record per line',
    )
    ap.add_argument("--output")
    ap.add_argument("--cache", default=os.path.join(os.path.dirname(__file__), "weights"))
    ap.add_argument("--bf16", action="store_true", help="bfloat16 weights on CUDA")
    ap.add_argument("--cpu", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        self_test(a.cache)
        return
    if not a.output:
        die("--output is required")
    if sum(bool(v) for v in (a.input, a.manifest_jsonl)) != 1:
        die("give exactly one of --input or --manifest-jsonl")

    import torch

    device = pick_device(a.cpu, "cuda:0")
    if device.startswith("cuda"):
        # No device argument: the string form is rejected before CUDA is
        # initialised, and this process only ever touches the default device.
        torch.cuda.reset_peak_memory_stats()
    model, d, dtype = load_model(a.cache, device, a.bf16)
    processor = load_processor(d)
    dtype_name = str(dtype).replace("torch.", "")
    log(f"device={device} dtype={dtype_name} model={MODEL['repo']}")

    head = (
        f'"model":{json.dumps(MODEL["repo"])},'
        f'"revision":"{MODEL["revision"]}",'
        f'"prompt_version":{PROMPT_VERSION},'
        f'"dtype":"{dtype_name}"'
    )

    def peak_note():
        if not device.startswith("cuda"):
            return ""
        peak = torch.cuda.max_memory_allocated() / (1024 ** 3)
        reserved = torch.cuda.max_memory_reserved() / (1024 ** 3)
        return f" peak VRAM {peak:.2f} GiB allocated / {reserved:.2f} GiB reserved"

    if a.input:
        desc = describe_one(model, processor, device, a.input)
        if not desc:
            die(f"the model returned an empty description for {a.input}")
        publish(a.output, "{" + head + ',"desc":' + json.dumps(desc, ensure_ascii=False) + "}\n")
        log(f"wrote {a.output} ({len(desc)} chars){peak_note()}")
        return

    # FAIL-SOFT PER LINE, exactly like embed.py's batch door: the Rust index
    # builder already keeps going when one photo has no description, and a
    # batch that aborted a 169-photo rebuild on one unreadable frame would be a
    # regression against that. A refused DOWNLOAD is still fatal - that is a
    # property of the run, not of one photo.
    out, paths = [], []
    with open(a.manifest_jsonl, encoding="utf-8") as f:
        for lineno, ln in enumerate(f, 1):
            if not ln.strip():
                continue
            try:
                path = str(json.loads(ln)["path"])
            except (ValueError, KeyError, TypeError) as e:  # noqa: BLE001 - per-line failure IS the contract
                out.append(
                    json.dumps({"path": f"<line {lineno}>", "error": f"{type(e).__name__}: {e}"})
                )
                continue
            paths.append(path)
    if not paths and not out:
        die(f"manifest {a.manifest_jsonl} lists no paths")
    for i, p in enumerate(paths, 1):
        try:
            desc = describe_one(model, processor, device, p)
            if not desc:
                raise ValueError("the model returned an empty description")
            out.append(
                '{"path":'
                + json.dumps(p)
                + ","
                + head
                + ',"desc":'
                + json.dumps(desc, ensure_ascii=False)
                + "}"
            )
        except Exception as e:  # noqa: BLE001 - per-line failure IS the contract
            out.append(json.dumps({"path": p, "error": f"{type(e).__name__}: {e}"}))
        if i % 10 == 0 or i == len(paths):
            log(f"{i} / {len(paths)}")
    publish(a.output, "\n".join(out) + "\n")
    log(f"wrote {a.output} ({len(out)} record(s)){peak_note()}")


if __name__ == "__main__":
    main()
