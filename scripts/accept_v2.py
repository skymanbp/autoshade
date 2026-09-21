#!/usr/bin/env python3
"""Hold a fine-tuned RAW cleaner to the lines that were written down BEFORE it ran.

The point of a pre-committed acceptance line is that nobody gets to move it after seeing the
result, so the lines live here as constants and this script only reads and compares. It runs
`denoise_flux_truth.py` on the candidate and judges two things mechanically:

  1. flux returned per brightness class, G plane -- the deficiency the fine-tune exists to fix;
  2. the mean shift of star-free sky per plane -- the way a network could fake (1) by moving
     the background instead of keeping the star.

The other two lines need a photograph and a long run, so this prints their exact commands
rather than pretending to have checked them.

WHICH G PLANE: the table was tabulated on G1, so G1 decides. G2 is printed beside it and a
disagreement between them is called out, because the two green planes seeing different things
would mean the measurement, not the network, is what changed.

NOT THE TRAINING LOG: `train_raw.py` prints its own "star flux returned" line every validation.
That number is not this one and must never be compared with it -- its field is dense enough
that the 5x5 windows overlap, and at 1.5-3 sigma only 27 % of the flux it divides by belongs
to the class being scored (measured 2026-09-21). The shipped weights read 0.838 there and 0.06
here. Same weights, different instruments.

    python scripts/accept_v2.py --run target-lane-train/v2 [--weights PATH] [--out DIR]
"""
import argparse
import json
import pathlib
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent


def candidate_from(run, out):
    """The file to measure when only --run was given, as a plain state dict.

    NOT `best_state.pth`. `train_raw.py` writes that one on held-out PSNR alone, against a
    threshold seeded from the PRETRAINED model's own validation (train_raw.py:369, 444) —
    so it appears only when a checkpoint beats the shipped weights at denoising. This
    fine-tune deliberately spends a little PSNR to buy back star flux, and §3 below allows
    0.15 dB of it, so a run doing exactly what it was asked can finish without ever writing
    that file. Selecting on it would be selecting against the point of the exercise.

    `last.pt` is a checkpoint (model, optimiser, schedule, iteration), not a state dict, so
    the model is unpacked into its own file for the instrument, which loads with
    `weights_only=True` and `strict=True` and should keep doing exactly that.
    """
    import torch

    best = run / "best_state.pth"
    last = run / "last.pt"
    if last.exists():
        blob = torch.load(last, map_location="cpu", weights_only=True)
        if isinstance(blob, dict) and "model" in blob and "iter" in blob:
            flat = out / f"candidate-it{blob['iter']}.pth"
            torch.save(blob["model"], flat)
            note = f"iteration {blob['iter']} (unpacked from {last.name})"
            if best.exists():
                note += "; best_state.pth also exists, PSNR-selected — not used, see docstring"
            return flat, note
        raise SystemExit(f"{last} is not a training checkpoint (no 'model'/'iter')")
    if best.exists():
        return best, f"{best.name} (no last.pt; this one is PSNR-selected)"
    raise SystemExit(f"no candidate in {run}: neither last.pt nor best_state.pth")

# Section 1, written 2026-09-21 before the run. "at least" unless the row says at most.
# The v1 column is the shipped weights read by this same instrument on the same date.
FLUX_LINES = [
    # class, direction, line, v1
    ("1.5", "at most", 0.35, 0.062),
    ("2.5", "at most", 0.35, 0.061),
    ("4", "at least", 0.50, 0.105),
    ("6", "at least", 0.75, 0.420),
    ("10", "at least", 0.90, 0.726),
    ("20", "at least", 0.95, 0.917),
    ("40", "at least", 0.95, 0.976),
]

# Section 2. Corrected 2026-09-21 BEFORE any candidate existed: the line first written was
# 0.30 DN, which the SHIPPED weights already miss on G2 (0.317), so it could never have been a
# "do not regress" line. The instrument's own noise floor is the noisy control, which must read
# 0 in expectation and reads -0.012 / -0.075 / -0.009 / +0.030, so a line tighter than ~0.1 DN
# is not resolvable. 0.40 DN is about five times that floor and above v1's worst plane.
SKY_LINE_DN = 0.40
V1_SKY = {"R": -0.016, "G1": 0.216, "G2": 0.317, "B": 0.097}


def judge(value, direction, line):
    return value <= line if direction == "at most" else value >= line


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--run", help="a training output directory; its best_state.pth is the candidate")
    ap.add_argument("--weights", help="the candidate state dict, if not --run/best_state.pth")
    ap.add_argument("--out", help="where flux-truth.json goes (default: <run>/accept)")
    ap.add_argument("--reuse", action="store_true", help="read an existing flux-truth.json instead of measuring")
    args = ap.parse_args()

    out = pathlib.Path(args.out) if args.out else pathlib.Path(args.run) / "accept"
    out.mkdir(parents=True, exist_ok=True)
    report = out / "flux-truth.json"
    if args.weights:
        weights, chosen = pathlib.Path(args.weights), str(args.weights)
        if not weights.exists():
            raise SystemExit(f"no candidate at {weights}")
    else:
        weights, chosen = candidate_from(pathlib.Path(args.run), out)
    print(f"candidate: {chosen}")

    if not args.reuse:
        cmd = [sys.executable, "-B", str(REPO / "scripts" / "denoise_flux_truth.py"),
               "--out", str(out), "--weights", str(weights), "--plate", str(out / "plate.png")]
        print("$ " + " ".join(cmd), flush=True)
        done = subprocess.run(cmd, cwd=REPO)
        if done.returncode:
            raise SystemExit(f"the flux instrument exited {done.returncode}")
    got = json.loads(report.read_text(encoding="utf-8"))

    failures = []
    print(f"\ncandidate: {got['weights']}   stars {got['stars']} ({got['per_class']} per class)\n")
    print(f"{'class':>6} {'G1':>8} {'G2':>8} {'v1 G1':>8} {'line':>14} {'verdict':>8}")
    for klass, direction, line, v1 in FLUX_LINES:
        rows = got["classes"][klass]
        g1, g2 = rows["G1"]["cleaner"], rows["G2"]["cleaner"]
        ok = judge(g1, direction, line)
        if not ok:
            failures.append(f"flux {klass}s: G1 {g1:.3f}, needs {direction} {line}")
        note = ""
        if judge(g2, direction, line) != ok:
            note = "  <- G1 and G2 disagree; look at the measurement before the network"
        print(f"{klass:>6} {g1:8.3f} {g2:8.3f} {v1:8.3f} {direction + ' ' + str(line):>14} "
              f"{'PASS' if ok else 'FAIL':>8}{note}")

    print(f"\nstar-free sky, mean(out - truth) in DN; line is |d| <= {SKY_LINE_DN}")
    print(f"{'plane':>6} {'candidate':>10} {'v1':>8} {'verdict':>8}")
    for plane, row in got["sky_dn"].items():
        d = row["cleaner"]
        ok = abs(d) <= SKY_LINE_DN
        if not ok:
            failures.append(f"sky {plane}: {d:+.3f} DN, needs |d| <= {SKY_LINE_DN}")
        print(f"{plane:>6} {d:+10.3f} {V1_SKY.get(plane, float('nan')):+8.3f} {'PASS' if ok else 'FAIL':>8}")

    print("\nStill to run by hand, because each needs a photograph or a long run:")
    print(f"  3. denoising must not pay for it (dPSNR within 0.15 dB of v1, v1 measured +3.13 / +5.92 dB):")
    print(f"     $ python scripts/denoise_bench.py --clean CLEAN.ARW --noisy NOISY.ARW \\")
    print(f"           --out {out / 'bench-v2'} --weights {weights}")
    print(f"     $ python scripts/denoise_bench.py --clean CLEAN.ARW --noisy NOISY.ARW --out {out / 'bench-v1'}")
    print(f"  4. the star standard's cleaner group must not lose a line it passes today:")
    print(f"     $ python scripts/denoise_star_standard.py --help   # for this lane's current invocation")

    if failures:
        print("\nFAIL:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("\nPASS: every line this script can judge is met.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
