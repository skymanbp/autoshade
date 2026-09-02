#!/usr/bin/env python3
"""Measure the Metal (MPS) path the Mac port added, on a machine that has one.

M2 routed all five sidecars through one `cuda -> mps -> cpu` ladder
(`python/_device.py`) and the release shipped saying the Metal path was
UNMEASURED: nobody had timed a forward pass on Apple silicon, nobody knew what
it cost in memory, and nobody had checked whether the one operator BiRefNet
needs that Metal has historically not implemented --- `torchvision.ops.
deform_conv2d` --- was quietly running on the CPU under
`PYTORCH_ENABLE_MPS_FALLBACK`, which the app itself sets. Three unknowns, all
answerable by a script the release runner can run in a minute.

WHAT IT REPORTS, and why each number is here:

* **device** --- what `_device.pick_device()` actually chose on this machine.
  The ladder is the thing under test; asking torch directly would test torch.
* **forward pass** --- wall time for a fixed convolution workload, after a
  warm-up iteration and with the device synchronised on both sides. A first
  iteration on Metal pays shader compilation, so timing it would measure the
  compiler.
* **peak memory** --- what the device allocator held at its high-water mark
  (`torch.mps.driver_allocated_memory` / `torch.cuda.max_memory_allocated`).
  On CPU there is no device allocator and the field says so rather than
  reporting a zero that reads like a measurement.
* **deform_conv2d** --- whether Metal implements it, answered in a CHILD
  process with `PYTORCH_ENABLE_MPS_FALLBACK` REMOVED from the environment.
  That is the only honest way to ask: with the fallback on (the app's own
  setting) an unimplemented operator runs on the CPU and hands back a tensor
  that claims the MPS device, so the parent process cannot tell the difference
  by looking at the result. torch reads the variable once, at import, so the
  question needs a fresh interpreter.

Runs anywhere: with no accelerator it measures the CPU path through the same
code, which is how it is validated off a Mac.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

# `python/_device.py` is the ladder under test. Imported by path rather than
# copied, for the reason that module exists at all: a second spelling of the
# device choice is a second thing to drift.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "python"))


def _deform_inputs(torch, device: str):
    """A minimal deformable-convolution call: one 3x3 kernel over a 64x64 tile.

    Small on purpose. The question is whether the OPERATOR exists on this
    backend, and the smallest call that can answer it is the one least likely
    to fail for an unrelated reason (memory, a shape the backend dislikes).
    """
    x = torch.zeros(1, 4, 64, 64, device=device)
    weight = torch.zeros(4, 4, 3, 3, device=device)
    # 2 * kh * kw offsets per output position, the operator's own contract.
    offset = torch.zeros(1, 2 * 3 * 3, 62, 62, device=device)
    return x, offset, weight


def probe_operator(device: str) -> dict:
    """CHILD MODE: does this backend implement `deform_conv2d` itself?"""
    import torch
    from torchvision.ops import deform_conv2d

    x, offset, weight = _deform_inputs(torch, device)
    try:
        out = deform_conv2d(x, offset, weight)
        if device != "cpu":
            getattr(torch, device.split(":")[0]).synchronize()
        return {"native": True, "device": str(out.device)}
    except (NotImplementedError, RuntimeError) as exc:
        # A backend that does not implement the operator raises here once the
        # CPU fallback is off. Both exception types are reported the same way:
        # what matters to the caller is that the work did not run on device.
        return {"native": False, "why": str(exc).splitlines()[0][:200]}


def ask_operator_in_a_clean_child(device: str) -> dict:
    """Ask [`probe_operator`] with the CPU fallback removed from the child."""
    env = dict(os.environ)
    env.pop("PYTORCH_ENABLE_MPS_FALLBACK", None)
    proc = subprocess.run(
        [sys.executable, "-E", str(Path(__file__).resolve()), "--operator-probe", device],
        capture_output=True,
        text=True,
        env=env,
        timeout=600,
    )
    for line in proc.stdout.splitlines():
        if line.startswith("{"):
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                break
    return {
        "native": None,
        "why": (proc.stderr.strip().splitlines() or ["the probe child said nothing"])[-1][:200],
    }


def time_forward(device: str, iterations: int) -> tuple[float, str]:
    """Wall time for a fixed convolution workload, and the peak memory it held."""
    import torch

    torch.manual_seed(0)
    net = torch.nn.Sequential(
        torch.nn.Conv2d(3, 32, 3, padding=1),
        torch.nn.ReLU(),
        torch.nn.Conv2d(32, 32, 3, padding=1),
        torch.nn.ReLU(),
        torch.nn.Conv2d(32, 1, 3, padding=1),
    ).to(device)
    x = torch.randn(1, 3, 512, 512, device=device)

    backend = getattr(torch, device.split(":")[0], None) if device != "cpu" else None
    if backend is not None and hasattr(backend, "reset_peak_memory_stats"):
        backend.reset_peak_memory_stats()

    def sync():
        if backend is not None and hasattr(backend, "synchronize"):
            backend.synchronize()

    with torch.no_grad():
        net(x)  # warm-up: the first Metal iteration compiles shaders
        sync()
        start = time.perf_counter()
        for _ in range(iterations):
            net(x)
        sync()
        elapsed = time.perf_counter() - start

    if backend is None:
        peak = "n/a (no device allocator on the CPU path)"
    elif hasattr(backend, "driver_allocated_memory"):
        peak = f"{backend.driver_allocated_memory() / (1 << 20):.1f} MiB (driver)"
    elif hasattr(backend, "max_memory_allocated"):
        peak = f"{backend.max_memory_allocated() / (1 << 20):.1f} MiB (allocator)"
    else:
        peak = "n/a (this backend reports no allocator statistics)"
    return elapsed / iterations, peak


def main() -> int:
    ap = argparse.ArgumentParser(description="Measure the Metal path, or the CPU one.")
    ap.add_argument("--operator-probe", metavar="DEVICE", help=argparse.SUPPRESS)
    ap.add_argument("--cpu", action="store_true", help="force the CPU arm of the ladder")
    ap.add_argument("--iterations", type=int, default=20)
    a = ap.parse_args()

    if a.operator_probe:
        print(json.dumps(probe_operator(a.operator_probe)))
        return 0

    from _device import pick_device

    import torch

    device = pick_device(prefer_cpu=a.cpu)
    per_pass, peak = time_forward(device, a.iterations)
    op = ask_operator_in_a_clean_child(device)
    if device == "cpu":
        verdict = "n/a (already on the CPU)"
    elif op["native"] is True:
        verdict = "no --- implemented natively on this backend"
    elif op["native"] is False:
        verdict = f"YES --- falls back to the CPU ({op['why']})"
    else:
        verdict = f"unknown --- the probe could not answer ({op['why']})"

    rows = [
        ("torch", torch.__version__),
        ("device chosen by `_device.pick_device`", f"`{device}`"),
        (f"forward pass (mean of {a.iterations}, 1x3x512x512)", f"{per_pass * 1000:.1f} ms"),
        ("peak memory", peak),
        ("`deform_conv2d` falls back to the CPU", verdict),
    ]
    table = ["| Measurement | Value |", "|---|---|"]
    table += [f"| {name} | {value} |" for name, value in rows]
    report = "### Metal (MPS) measurement\n\n" + "\n".join(table) + "\n"
    print(report)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
