"""Our own copy of every pinned download, tried before the upstream host.

Every model this program runs is fetched from somebody else's server at a
pinned revision: five Hugging Face repos we do not own, a GitHub release from
2022, and three source files served from `raw.githubusercontent.com`. Each of
those can go away — an account renamed, a release re-cut, a repo made private,
a file deleted — and when one does, the pin that makes the download safe is
exactly what makes it unrecoverable: nothing else on the internet is that
revision. The failure would not be a slow download, it would be a feature that
cannot start on any machine whose cache is cold.

So every pinned byte this program fetches also lives in a repo of ours, and
this table is the one place that says where. The bytes were copied from the
pins themselves (uploaded only after hashing against the same sha256 the
sidecars enforce) and read back from the server afterwards; the mirrors were
then verified once more anonymously, without a token, which is the only access
a user's machine ever has.

What this does NOT change:

* the pins. `sha256` and the byte cap are the authority, here as before. A
  source only decides WHERE bytes come from, never whether they are
  acceptable, so a stale or wrong mirror is refused by exactly the gate a
  wrong upstream would be — and then the upstream is tried.
* the cache identity. `_sidecar.model_dir` still names a directory after the
  UPSTREAM repo and revision, because that is what the files are. A machine
  that already holds them keeps them; nothing is re-downloaded for this.
* what is pinned. Adding a mirror for a re-pinned revision is a deliberate
  act, so an upstream re-pin without one simply has no entry here and goes
  straight to the source, rather than quietly fetching yesterday's tree.

`autoshade-raw-denoise-v1.pth` — our own fine-tuned RAW denoiser — is not in
the table: it is served from our own GitHub release, so it has no upstream to
fall back to and nothing to be cut off from.

Licences travel with the bytes: mirroring makes us a redistributor, and each
mirror repo carries the upstream licence (Apache-2.0, MIT, or in Stable
Diffusion 2.1's case CreativeML Open RAIL++-M, whose use-based restrictions
must reach whoever pulls our copy). The per-model list is in the licence
table of `README.md` and `docs/ARCHITECTURE.md`.

Stdlib only, and nothing imported from it: this module is read at import time
by `denoise.py`, which every other sidecar reaches the network through, and
`segment.py --help` must not pay for a table of strings.
"""

# Verified 2026-09-20 at the revisions below, anonymously, against the pin each
# sidecar enforces: 52 files over seven repos, the large ones by the server's
# own LFS digest and the rest pulled back and hashed here, zero disagreements.
# Each revision is a tree that carries the upstream licence as well as the
# bytes — the repos were re-pinned after the licence files landed, so what a
# user fetches from us states the terms it travels under.
MIRROR_OWNER = "Azng0"

# The GitHub-hosted pins share one repo, flat-named: five SCUNet weight sets
# and the three architecture files that are EXECUTED after verification.
# One line on purpose: a Rust source invariant reads every revision hash in
# this file, and a literal split across two lines would hide one from it.
_KAIR = "https://huggingface.co/Azng0/autoshade-mirror-kair/resolve/632af04fb80d703044bbdd577583548b6b1ede1d/"

# upstream URL prefix -> ours. Both sides end in `/`, so the file path the
# caller asked for is appended unchanged: a nested name (SD 2.1's `unet/…`)
# keeps its shape in the mirror, which is how it was uploaded.
MIRRORS = {
    # segment.py BIREFNET — the subject matte
    "https://huggingface.co/ZhengPeng7/BiRefNet/resolve/e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4/":
        "https://huggingface.co/Azng0/autoshade-mirror-birefnet/resolve/af1535164ff0a7d6c228f1000aade339dee01147/",
    # segment.py SKY — OneFormer ADE20K, the sky plane
    "https://huggingface.co/shi-labs/oneformer_ade20k_swin_large/resolve/4a5bac8e64f82681a12db2e151a4c2f4ce6092b2/":
        "https://huggingface.co/Azng0/autoshade-mirror-oneformer-ade20k-swin-large/resolve/747d8577c8aaa2be21b77c0ee827d3a077bbe4d5/",
    # segment.py SAM — the click-to-select mask
    "https://huggingface.co/facebook/sam2.1-hiera-large/resolve/665f8e2ad61cf5f53d65644ff27c8ee525124610/":
        "https://huggingface.co/Azng0/autoshade-mirror-sam2.1-hiera-large/resolve/d649a31a90c94860a2ec8d3ab2b4ea86efa58e03/",
    # embed.py MODEL — SigLIP2, the style index's two towers
    "https://huggingface.co/google/siglip2-base-patch16-384/resolve/f775b65a79762255128c981547af89addcfe0f88/":
        "https://huggingface.co/Azng0/autoshade-mirror-siglip2-base-patch16-384/resolve/12b6bb3a273df699df2cc7e6153361d3a5c47bf1/",
    # describe.py MODEL — Qwen3-VL, the scene sentence
    "https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct/resolve/89644892e4d85e24eaac8bacfd4f463576704203/":
        "https://huggingface.co/Azng0/autoshade-mirror-qwen3-vl-2b-instruct/resolve/98a81c7fe41ce524020d242aa49a1d2ce4c63de8/",
    # correspond.py MODEL — Stable Diffusion 2.1, the generative fill
    "https://huggingface.co/sd2-community/stable-diffusion-2-1/resolve/bb2154823665391b4fb29b0b9cf82a198964ee05/":
        "https://huggingface.co/Azng0/autoshade-mirror-stable-diffusion-2-1/resolve/9c4a4c5bac209d2ad06dce8ed03c79609dbee112/",
    # denoise.py WEIGHT_URLS — the KAIR v1.0 release assets
    "https://github.com/cszn/KAIR/releases/download/v1.0/": _KAIR,
    # denoise.py NETWORK_URL — SCUNet's network file, at its pinned commit
    "https://raw.githubusercontent.com/cszn/SCUNet/9a6c6507aaddde34712553babc5e1f7fb8522287/models/": _KAIR,
    # denoise_raw.py PINS — DRUNet's network file and the blocks it imports
    "https://raw.githubusercontent.com/cszn/KAIR/345c87f8364322c40eef52e575f98af893f04126/models/": _KAIR,
    "https://raw.githubusercontent.com/cszn/KAIR/5d55a5fb88d20eb811dc7ccf6342b921039191cf/models/": _KAIR,
}


def mirror_of(url):
    """Our URL for `url`, or None when nothing here covers it."""
    for upstream, mirror in MIRRORS.items():
        if url.startswith(upstream):
            return mirror + url[len(upstream):]
    return None


def sources(url):
    """Where to try, in order: ours first, then the source it came from.

    The upstream keeps the ONE re-try it has always had, so a URL with no
    mirror behaves exactly as it did before this module existed — two attempts
    at the same address — and a mirrored one spends its first attempt on us
    instead of repeating the request that just failed.
    """
    mirror = mirror_of(url)
    return ([mirror] if mirror else []) + [url, url]
