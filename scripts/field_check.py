"""Cross-check `src/fit_field.rs` against the validated NumPy solver.

Reads the artefacts written by the Rust probe test
`export_calibration_field_inputs_for_numpy` (target/field-probe/*.bin + meta.json),
re-derives the guide and the unclipped mask from the exported pixels, runs
`grid_experiment.GridSystem` with the production regulariser (tikhonov 1.0,
smooth (1, 1, 1), 90 iterations) on exactly those numbers, and writes the NumPy
grid and render back for `compare_calibration_field_with_numpy` to score with the
production objective.

    python scripts/field_check.py
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

from grid_experiment import PARAMS, VERTICES, GridSystem, smooth_3tap, splat_table

ROOT = Path(__file__).resolve().parents[1]
PROBE = ROOT / "target" / "field-probe"
TIKHONOV = 1.0
SMOOTH = (1.0, 1.0, 1.0)
ITERATIONS = 90
LOW, HIGH = 1.0 / 255.0, 254.0 / 255.0


def load(name: str) -> np.ndarray:
    path = PROBE / name
    if not path.is_file():
        raise SystemExit(
            f"{path} is missing — run the Rust probe first:\n"
            "  cargo test --release -- --ignored export_calibration_field_inputs_for_numpy"
        )
    return np.fromfile(path, dtype="<f4")


def main() -> None:
    meta = json.loads((PROBE / "meta.json").read_text(encoding="utf-8"))
    width, height = int(meta["width"]), int(meta["height"])
    current = load("current.bin").reshape(height, width, 3)
    target = load("target.bin").reshape(height, width, 3)
    guide = load("guide.bin").reshape(height, width)
    support = load("support.bin").reshape(height, width)
    evidence = load("evidence.bin").reshape(height, width)
    exported_weight = load("weights.bin").reshape(height, width)

    # 1. the guide must be the 3-tap smoothed luma601 of the current render
    luma = current @ np.array([0.299, 0.587, 0.114], dtype=np.float32)
    guide_delta = float(np.max(np.abs(smooth_3tap(luma) - guide)))

    # 2. the fit weight must be evidence x local support x unclipped.  "Unclipped"
    # is both frames strictly inside (1/255, 254/255) on all three channels, which
    # over the six stacked channels is one min/max pair.
    stacked = np.concatenate([current, target], axis=2)
    unclipped = ((stacked.min(axis=2) > LOW) & (stacked.max(axis=2) < HIGH)).astype(np.float32)
    fit_weight = evidence * support * unclipped
    weight_delta = float(np.max(np.abs(fit_weight - exported_weight)))
    if weight_delta > 1e-6:
        raise SystemExit(
            f"the exported fit weight disagrees with evidence x support x unclipped by "
            f"{weight_delta} — the two sides are not solving the same system"
        )

    ids, tri = splat_table(guide)
    system = GridSystem(current, target, guide, ids, tri, fit_weight)
    grid, info = system.solve(TIKHONOV, SMOOTH, iterations=ITERATIONS)
    render = system.render(grid)

    grid.astype("<f4").tofile(PROBE / "numpy-grid.bin")
    np.clip(render, 0.0, 1.0).astype("<f4").tofile(PROBE / "numpy-render.bin")

    rust_grid = load("rust-grid.bin").reshape(VERTICES, PARAMS)
    report = {
        "numpy": np.__version__,
        "tikhonov": TIKHONOV,
        "smooth": list(SMOOTH),
        "iterations_budget": ITERATIONS,
        "iterations": int(info["iterations"]),
        "relative_residual": float(info["relative_residual"]),
        "occupancy_supported": int(np.count_nonzero(system.occupancy >= 8.0)),
        "fit_weight_mass": float(fit_weight.sum()),
        "fit_weight_nonzero": int(np.count_nonzero(fit_weight)),
        "guide_max_abs_delta": guide_delta,
        "weight_max_abs_delta": weight_delta,
        "grid_max_abs_delta_vs_rust": float(np.max(np.abs(grid - rust_grid))),
        "grid_max_abs": [float(v) for v in np.max(np.abs(grid), axis=0)],
        "pixels": width * height,
    }
    (PROBE / "numpy-solve.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="ascii"
    )
    print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
