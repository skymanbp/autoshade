"""Which accelerator a sidecar runs on — the ONE answer all five of them use.

Sits beside the sidecars in `python/` and is imported by file name, exactly the
way `segment.py` requires `ade20k_class_table.json` to sit beside it. That is
not a new coupling and not a new trust surface: `sys.path[0]` is the running
SCRIPT's own directory (the sidecars are launched as `python -E <script>`, and
`-E` does not touch it), and the Rust side resolves that script against the
program's own tree and never the working directory (`config::bundled_helper`).
A module found there is exactly as trusted as the script that imports it.

Why one file instead of a copy per sidecar: six call sites used to spell
`"cuda" if torch.cuda.is_available() else "cpu"` by hand, which is how five
scripts end up disagreeing about a platform the sixth one already learned about
— and macOS is that platform.
"""


def pick_device(prefer_cpu: bool = False, cuda: str = "cuda") -> str:
    """`cuda` -> `mps` -> `cpu`, in that order. `prefer_cpu` short-circuits.

    `cuda` names the CUDA spelling the caller already used (`"cuda"` or
    `"cuda:0"`), so adding Metal changes nothing for anyone who has an NVIDIA
    card: the string handed to `.to()` on that path is the same string as
    before, byte for byte.

    MPS is Apple's Metal backend. It is checked SECOND and never preferred over
    CUDA, and every dtype gate in these sidecars keys on
    `device.startswith("cuda")` — so an MPS run stays in fp32 without a single
    dtype change, which is the intended behaviour rather than an oversight:
    fp16 on Metal is not the configuration any of these checkpoints was
    measured in.

    `torch.backends.mps` is reached defensively. It exists on every torch build
    that ships for macOS, but not on older builds elsewhere, and a sidecar must
    not die of an AttributeError while merely asking what hardware it has.
    """
    if prefer_cpu:
        return "cpu"
    import torch

    if torch.cuda.is_available():
        return cuda
    mps = getattr(torch.backends, "mps", None)
    if mps is not None and mps.is_available():
        return "mps"
    return "cpu"
