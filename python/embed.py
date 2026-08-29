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

THE HONEST COST of base/384: of its 375.5 M parameters, the TEXT tower's token
embedding alone is 256,000 x 768 ~ 197 M — over half the file. This sidecar
uses the VISION tower only, so roughly 1.1 GB of the 1.50 GB download is dead
weight today. It is not separable: upstream publishes one monolithic
`model.safetensors`, and range-reading selected tensors out of it would defeat
whole-file digest pinning. Stated here rather than discovered later.

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

# The download/verify half of the sidecar contract, imported rather than
# copied. Python puts this script's own directory on sys.path[0], so a
# relocated embed.py without denoise.py beside it fails HERE with a sentence
# instead of somewhere inside the fetch.
try:
    from denoise import _fetch_verified
except ImportError as e:  # pragma: no cover - environment shape, not logic
    print(
        f"embed.py: cannot import the shared sidecar downloader from denoise.py "
        f"({e}) — embed.py must sit beside denoise.py in python/.",
        file=sys.stderr,
    )
    sys.exit(2)


def log(msg):
    print(f"[embed] {msg}", file=sys.stderr, flush=True)


def die(msg: str) -> None:
    print(f"embed.py: {msg}", file=sys.stderr)
    sys.exit(2)


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
TEXT_DO_LOWER_CASE = True
# Named contract: the tokenizer must reproduce the checkpoint's training-time
# text path. Keeping this beside EXPECT_SIZE makes a pin change fail loudly
# instead of quietly changing every text cosine in an existing index.
TEXT_TOKENIZER_CONTRACT = {
    "padding": TEXT_PADDING,
    "max_length": TEXT_MAX_LENGTH,
    "do_lower_case": TEXT_DO_LOWER_CASE,
    "unk_token": "<unk>",
    "pad_token": "<pad>",
    "eos_token": "<eos>",
    "bos_token": "<bos>",
    "additional_special_tokens": ("<start_of_turn>", "<end_of_turn>"),
    "add_bos_token": False,
    "add_eos_token": True,
}
TEXT_SPECIAL_TOKENS = (
    TEXT_TOKENIZER_CONTRACT["unk_token"],
    TEXT_TOKENIZER_CONTRACT["pad_token"],
    TEXT_TOKENIZER_CONTRACT["eos_token"],
    TEXT_TOKENIZER_CONTRACT["bos_token"],
    *TEXT_TOKENIZER_CONTRACT["additional_special_tokens"],
)


def model_dir(cache_dir):
    """One directory per pinned (repo, revision) — a re-pin never reuses a
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
        # Same small slack denoise.py leaves on its cap: an overshoot message
        # should be about the ENDPOINT, not an off-by-one.
        _fetch_verified(
            url,
            os.path.join(d, name),
            pin["sha256"],
            pin["bytes"] + 4096,
            f"the SigLIP 2 '{name}'",
        )
    return d


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
    if cfg.get("do_lower_case", TEXT_DO_LOWER_CASE) is not TEXT_DO_LOWER_CASE:
        problems.append("do_lower_case disagrees")
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
    from transformers import AutoTokenizer
    # `local_files_only` is the only loading door; AutoTokenizer's default is
    # to refuse remote code, and the source-level gate below forbids opting in.
    try:
        return AutoTokenizer.from_pretrained(d, local_files_only=True)
    except (AttributeError, ImportError, OSError) as error:
        # Transformers 5.2 can lack the SigLIP mapping (and the slow tokenizer
        # needs sentencepiece, which is intentionally optional here). The
        # pinned tokenizer.json is itself a complete tokenizers graph, so use
        # its local fast backend with the same explicit 64-token contract.
        log(f"AutoTokenizer unavailable ({type(error).__name__}); using pinned tokenizer.json")
        return PinnedFastTokenizer(d)


class PinnedFastTokenizer:
    """Minimal tokenizer.json adapter for environments without sentencepiece."""

    def __init__(self, directory):
        from tokenizers import Tokenizer

        self._tokenizer = Tokenizer.from_file(os.path.join(directory, "tokenizer.json"))
        pad_id = self._tokenizer.token_to_id(TEXT_TOKENIZER_CONTRACT["pad_token"])
        if pad_id is None:
            raise RuntimeError("pinned tokenizer has no <pad> token")
        self._tokenizer.enable_truncation(max_length=TEXT_MAX_LENGTH)
        self._tokenizer.enable_padding(
            length=TEXT_MAX_LENGTH,
            pad_id=pad_id,
            pad_token=TEXT_TOKENIZER_CONTRACT["pad_token"],
        )

    def __call__(self, texts, padding, max_length, truncation, return_tensors):
        if padding != TEXT_PADDING or max_length != TEXT_MAX_LENGTH or not truncation:
            raise ValueError("pinned tokenizer only supports max_length padding at 64 tokens")
        if return_tensors != "pt":
            raise ValueError("pinned tokenizer adapter only returns PyTorch tensors")
        import torch

        encoded = self._tokenizer.encode_batch(list(texts))
        return {
            "input_ids": torch.tensor([item.ids for item in encoded], dtype=torch.long),
            "attention_mask": torch.tensor([item.attention_mask for item in encoded], dtype=torch.long),
        }


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
    print("text_tower_padding_rule_is_the_pinned_one: PASS (max_length/64, lower-case, and special tokens)")
    print("tokenizer_files_are_pinned_by_digest: PASS (cached files match size pins)")


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


def embed_texts(model, tokenizer, device, texts, np):
    import torch
    if not texts:
        return np.empty((0, MODEL["dim"]), dtype=np.float32)
    enc = tokenizer(list(texts), padding=TEXT_PADDING, max_length=TEXT_MAX_LENGTH,
                    truncation=True, return_tensors="pt")
    enc = {k: v.to(device) for k, v in enc.items()}
    with torch.no_grad():
        out = model.text_model(**enc).pooler_output.float()
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


def publish(path, text):
    """tmp + fsync + os.replace, like both existing sidecars (L03): the caller
    stages this file and a recipe/index may reference it, so a payload still in
    the page cache must not vanish under a power cut."""
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
                # Best-effort cleanup on the error path — an unremovable temp
                # (an AV lock, on Windows) must not become the reported fault.
                # why: the original write exception is already propagating.
                pass


def main() -> None:
    ap = argparse.ArgumentParser(description="Autoshop style embedding (SigLIP 2)")
    ap.add_argument("--input", help="one image (any PIL-readable format)")
    ap.add_argument(
        "--manifest",
        help="newline-delimited image paths; writes one JSON record per line",
    )
    ap.add_argument("--manifest-jsonl", help='JSONL manifest of {"path": ..., "text": ...}')
    ap.add_argument("--text-file", help="UTF-8 file containing one text query")
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
    if sum(bool(v) for v in (a.input, a.manifest, a.manifest_jsonl)) != 1:
        die("give exactly one of --input, --manifest or --manifest-jsonl")

    import numpy as np
    import torch

    device = "cpu" if a.cpu or not torch.cuda.is_available() else "cuda:0"
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

    if a.manifest_jsonl:
        with open(a.manifest_jsonl, encoding="utf-8") as f:
            entries = [json.loads(ln) for ln in f if ln.strip()]
        paths = [str(e.get("path", "")) for e in entries]
        texts_by_path = {str(e.get("path", "")): e.get("text") for e in entries}
    else:
        with open(a.manifest, encoding="utf-8") as f:
            paths = [ln.strip() for ln in f if ln.strip()]
        texts_by_path = {}
    if not paths:
        die(f"manifest {a.manifest} lists no paths")
    # FAIL-SOFT PER LINE. The Rust index builder already skips individual
    # photos on decode/sidecar failure and keeps the run going; a batch mode
    # that aborted a 150-photo rebuild on one unreadable file would be a
    # regression against that. A refused DOWNLOAD is still fatal — that is a
    # property of the run, not of one photo.
    out = []
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
