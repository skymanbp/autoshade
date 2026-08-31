"""Shared sidecar plumbing: the five helpers describe.py, embed.py and
correspond.py each used to write out for themselves.

The three scripts differ in the MODEL they pin, the frames they read and the
artifact they emit. Everything else about being a model sidecar — how a
progress line is logged, how a refusal exits, where a pinned checkout lives,
how the pinned files are fetched, how the artifact is published — is one
contract, and it was implemented three times. What actually differed between
the copies was the script's own name in a format string, a doc sentence
rephrased, and one strictly better `makedirs`; nothing that made three
implementations right.

This is the third shared module in `python/`, after `_device.pick_device` (the
cuda/mps/cpu ladder, pinned from Rust so no sidecar can grow its own) and
`denoise._fetch_verified` (the download-and-verify half, imported rather than
copied). Those two set the rule: a sidecar concern that is not about THIS
sidecar belongs beside them, not in each caller.

`_fetch_verified` deliberately stays in denoise.py. Moving it would edit the
one sidecar that does NOT use this module, for no gain — it is imported here
exactly as the three scripts imported it, and the early-failure chain is now
script -> `_sidecar` -> `denoise`, each link failing at IMPORT time with a
sentence rather than somewhere inside a fetch.

Two copies stay outside this module, each for a reason that is checkable
rather than a matter of taste:

* `denoise.log` — this module imports denoise, so denoise importing it back
  would be a cycle. That is the same constraint that keeps `_fetch_verified`
  where it is, and it is why every download line in the family still says
  `[denoise]`.
* `segment.die` — segment.py loads NO heavy module at import time (verified:
  after importing it, `numpy`, `torch` and `denoise` are all absent from
  `sys.modules`), while importing this module pulls in numpy through denoise.
  Folding a two-line `die` would move that cost to every `segment.py --help`
  and every early refusal. The deferral is worth more than the two lines.
"""

import os
import sys

try:
    from denoise import _fetch_verified
except ImportError as e:  # pragma: no cover - environment shape, not logic
    print(
        f"_sidecar.py: cannot import the shared sidecar downloader from denoise.py "
        f"({e}) - _sidecar.py must sit beside denoise.py in python/.",
        file=sys.stderr,
    )
    sys.exit(2)


def log(tag, msg):
    """One progress line on stderr, flushed.

    The Rust side reads these while the run is still going (`sidecar_tail`), so
    a buffered line would report a hang that is not happening.
    """
    print(f"[{tag}] {msg}", file=sys.stderr, flush=True)


def die(tag, msg):
    """Refuse with a sentence, on stderr, exiting 2.

    Not exit 1: the Rust side already treats "exited 0 but wrote no output" as
    a distinct failure, and a refusal must be distinguishable from a crash.
    """
    print(f"{tag}.py: {msg}", file=sys.stderr)
    sys.exit(2)


def model_dir(model, cache_dir):
    """One directory per pinned (repo, revision) — a re-pin never reuses a
    cache filled at the old revision, and the digest gate never has to be the
    thing that catches it."""
    slug = model["repo"].replace("/", "--")
    return os.path.join(cache_dir, f"{slug}@{model['revision'][:12]}")


def fetch_model(model, cache_dir, label):
    """Fetch every pinned file into that revision's own directory, each
    verified against its own sha256 and byte cap.

    The per-file `makedirs` is correspond.py's shape, which is the strict
    superset. For a NESTED file map (SD 2.1's `unet/…`, `vae/…`) it makes the
    subdirectory the file goes in, instead of failing on it. For a FLAT one
    (describe, embed) `os.path.dirname(dest)` IS the model directory, so the
    call repeats an `exist_ok` no-op once per file — the only difference this
    fold makes to those two, and a cheap one on a path that is about to move
    gigabytes.

    The root `makedirs` is kept above it so the postcondition holds without
    reading the pin: after this returns, the model directory exists whatever
    `files` contained.

    `label` names the model family in the byte-cap message, so an overshoot
    says WHICH download disagreed with its pin.
    """
    d = model_dir(model, cache_dir)
    os.makedirs(d, exist_ok=True)
    for name, pin in model["files"].items():
        dest = os.path.join(d, name)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        url = (
            f"https://huggingface.co/{model['repo']}/resolve/"
            f"{model['revision']}/{name}"
        )
        # The same small slack denoise.py leaves on its cap: an overshoot
        # message should be about the ENDPOINT, not an off-by-one.
        _fetch_verified(url, dest, pin["sha256"], pin["bytes"] + 4096, f"the {label} '{name}'")
    return d


def publish(path, text):
    """tmp + fsync + os.replace (L03): the caller stages this file and a build
    or a recipe reads it, so a payload still in the page cache must not vanish
    under a power cut."""
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
