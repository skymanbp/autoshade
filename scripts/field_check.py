"""Cross-check `src/fit_field.rs` against the validated NumPy solver.

Reads the artefacts written by the Rust probe test
`export_calibration_field_inputs_for_numpy` (target/field-probe/*.bin + meta.json),
re-derives the guide and the unclipped mask from the exported pixels, runs
`grid_experiment.GridSystem` at production's grid and regulariser (the 12x8x8
experiment refined to 32 vertex rows, tikhonov and smooth rescaled with it, 90
iterations — `src/fit_field.rs`) on exactly those numbers, with production's
bound-constrained active-set loop around the experiment's conjugate gradients
(`System::solve`, 2026-10-19: bounds honoured INSIDE the solve, not clipped
onto an unconstrained answer) and its per-CELL occupancy floor, and writes the
NumPy grid and render back for `compare_calibration_field_with_numpy` to score
with the production objective.

    python scripts/field_check.py
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

import grid_experiment
from grid_experiment import BOUNDS_HIGH, BOUNDS_LOW, PARAMS, GridSystem, smooth_3tap, splat_table

ROOT = Path(__file__).resolve().parents[1]
PROBE = ROOT / "target" / "field-probe"
# `src/fit_field.rs`: FIELD_Y_REFINE, CELL_ROWS, TIKHONOV, SMOOTH, OCCUPANCY_MIN.
ROWS_PER_CELL = 4
CELL_ROWS = 8
SY = CELL_ROWS * ROWS_PER_CELL
TIKHONOV = 1.0 / ROWS_PER_CELL
SMOOTH = (1.0 / ROWS_PER_CELL, 1.0, 1.0 / ROWS_PER_CELL)
OCCUPANCY_MIN = 8.0
ITERATIONS = 90
ACTIVE_SET_PASSES = 40
LOW, HIGH = 1.0 / 255.0, 254.0 / 255.0


def load(name: str) -> np.ndarray:
    path = PROBE / name
    if not path.is_file():
        raise SystemExit(
            f"{path} is missing — run the Rust probe first:\n"
            "  cargo test --release -- --ignored export_calibration_field_inputs_for_numpy"
        )
    return np.fromfile(path, dtype="<f4")


def conjugate_gradients(system, rhs, start, free, iterations):
    """`System::conjugate_gradients`: CG restricted to the `free` parameters,
    warm-started from `start`; the held ones keep their value and take no step."""
    vector = start.copy()
    if np.all(vector == 0.0):
        residual = rhs.copy()
    else:
        residual = rhs - system.matvec(vector, TIKHONOV, SMOOTH)
    residual[~free] = 0.0
    direction = residual.copy()
    rr = float(np.dot(residual.astype(np.float64), residual.astype(np.float64)))
    initial = max(rr, 1e-30)
    used = 0
    for used in range(1, iterations + 1):
        product = system.matvec(direction, TIKHONOV, SMOOTH)
        product[~free] = 0.0
        denominator = float(np.dot(direction.astype(np.float64), product.astype(np.float64)))
        if denominator <= 1e-20:
            break
        alpha = np.float32(rr / denominator)
        vector += alpha * direction
        residual -= alpha * product
        next_rr = float(np.dot(residual.astype(np.float64), residual.astype(np.float64)))
        if next_rr <= initial * 1e-10:
            rr = next_rr
            break
        direction = residual + np.float32(next_rr / max(rr, 1e-30)) * direction
        rr = next_rr
    return vector, {"iterations": used, "relative_residual": (rr / initial) ** 0.5}


def solve_bounded(system):
    """`System::solve`: the active-set loop. Each pass clamps, pins the parameters
    sitting on a bound whose gradient points outward, re-solves the free ones with
    the pinned ones held, and stops when the pinned set repeats with every free
    parameter strictly inside its bounds."""
    rhs = system.rhs()
    low = np.tile(BOUNDS_LOW, grid_experiment.VERTICES).astype(np.float32)
    high = np.tile(BOUNDS_HIGH, grid_experiment.VERTICES).astype(np.float32)
    free = np.ones(rhs.shape, dtype=bool)
    vector, info = conjugate_gradients(system, rhs, np.zeros_like(rhs), free, ITERATIONS)
    for _ in range(ACTIVE_SET_PASSES):
        vector = np.clip(vector, low, high)
        descent = rhs - system.matvec(vector, TIKHONOV, SMOOTH)
        pinned = ~(((vector <= low) & (descent < 0.0)) | ((vector >= high) & (descent > 0.0)))
        if np.array_equal(pinned, free) and np.all(~free | ((low < vector) & (vector < high))):
            break
        free = pinned
        vector, info = conjugate_gradients(system, rhs, vector, free, ITERATIONS)
    vector = np.clip(vector, low, high)
    return vector.reshape(grid_experiment.VERTICES, PARAMS), info


def supported(occupancy):
    """`fit_field::supported`: a vertex is measured when its CELL (its column and
    luma bin, summed over the ROWS_PER_CELL vertex rows that refine the cell's
    row) carries at least OCCUPANCY_MIN of mass."""
    sx, sb = grid_experiment.SX, grid_experiment.SB
    mass = occupancy.reshape(CELL_ROWS, ROWS_PER_CELL, sx, sb).sum(axis=1, keepdims=True)
    return np.broadcast_to(mass >= OCCUPANCY_MIN, (CELL_ROWS, ROWS_PER_CELL, sx, sb)).reshape(-1)


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

    grid_experiment.configure(SY, OCCUPANCY_MIN)
    ids, tri = splat_table(guide)
    system = GridSystem(current, target, guide, ids, tri, fit_weight)
    grid, info = solve_bounded(system)
    grid[~supported(system.occupancy)] = 0.0
    render = system.render(grid)

    grid.astype("<f4").tofile(PROBE / "numpy-grid.bin")
    np.clip(render, 0.0, 1.0).astype("<f4").tofile(PROBE / "numpy-render.bin")

    rust_grid = load("rust-grid.bin").reshape(grid_experiment.VERTICES, PARAMS)
    report = {
        "numpy": np.__version__,
        "tikhonov": TIKHONOV,
        "smooth": list(SMOOTH),
        "iterations_budget": ITERATIONS,
        "iterations": int(info["iterations"]),
        "relative_residual": float(info["relative_residual"]),
        "grid": [grid_experiment.SX, SY, grid_experiment.SB],
        "occupancy_supported": int(np.count_nonzero(supported(system.occupancy))),
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
