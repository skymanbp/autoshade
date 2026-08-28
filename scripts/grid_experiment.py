"""Offline 12x8x8 bilateral-grid feasibility experiment (NumPy solver).

The Rust companion exports the production frozen evidence and local structural
support, and evaluates final PNGs with the production-equivalent objective.
This file owns only the phase-1 splat/solve/slice experiment and measurements.
"""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import os
import subprocess
import time
from pathlib import Path

import numpy as np
from PIL import Image


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target" / "fitgrid-experiment"
ANALYSIS = OUT / "analysis"
RENDERS = OUT / "results" / "renders"
PROBE_DATA = OUT / "results" / "probe"
GRID_OUT = OUT / "results" / "grid"
PROBE = OUT / "target" / "release" / "fitgrid-probe.exe"
SX, SY, SB, PARAMS = 12, 8, 8, 5
VERTICES = SX * SY * SB
BOUNDS_LOW = np.array([-1.25, -0.35, -0.35, -0.35, -0.50], dtype=np.float32)
BOUNDS_HIGH = np.array([1.25, 0.35, 0.35, 0.35, 0.50], dtype=np.float32)
OCCUPANCY_MIN = 8.0


def load_rgb(path: Path) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0


def save_rgb(path: Path, values: np.ndarray, icc_profile: bytes | None = None) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = np.rint(np.clip(values, 0.0, 1.0) * 255.0).astype(np.uint8)
    Image.fromarray(encoded, "RGB").save(path, icc_profile=icc_profile)


def smooth_3tap(values: np.ndarray) -> np.ndarray:
    padded = np.pad(values, ((1, 1), (1, 1)), mode="edge")
    horizontal = (padded[1:-1, :-2] + padded[1:-1, 1:-1] + padded[1:-1, 2:]) / 3.0
    padded_h = np.pad(horizontal, ((1, 1), (0, 0)), mode="edge")
    return (padded_h[:-2] + padded_h[1:-1] + padded_h[2:]) / 3.0


def splat_table(guide: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    height, width = guide.shape
    x = np.broadcast_to(np.linspace(0.0, SX - 1, width, dtype=np.float32), (height, width))
    y = np.broadcast_to(
        np.linspace(0.0, SY - 1, height, dtype=np.float32)[:, None], (height, width)
    )
    b = np.clip(guide, 0.0, 1.0) * (SB - 1)
    coords = [x.ravel(), y.ravel(), b.ravel()]
    low = [np.floor(value).astype(np.int32) for value in coords]
    high = [np.minimum(value + 1, limit - 1) for value, limit in zip(low, (SX, SY, SB))]
    frac = [value - lower for value, lower in zip(coords, low)]
    ids = np.empty((guide.size, 8), dtype=np.int32)
    weights = np.empty((guide.size, 8), dtype=np.float32)
    slot = 0
    for choose_x in (0, 1):
        for choose_y in (0, 1):
            for choose_b in (0, 1):
                ix = high[0] if choose_x else low[0]
                iy = high[1] if choose_y else low[1]
                ib = high[2] if choose_b else low[2]
                wx = frac[0] if choose_x else 1.0 - frac[0]
                wy = frac[1] if choose_y else 1.0 - frac[1]
                wb = frac[2] if choose_b else 1.0 - frac[2]
                ids[:, slot] = (iy * SX + ix) * SB + ib
                weights[:, slot] = wx * wy * wb
                slot += 1
    return ids, weights


class GridSystem:
    def __init__(
        self,
        current: np.ndarray,
        target: np.ndarray,
        guide: np.ndarray,
        ids: np.ndarray,
        tri: np.ndarray,
        fit_weight: np.ndarray,
        anchor_delta: np.ndarray | None = None,
        anchor_weight: np.ndarray | None = None,
    ) -> None:
        self.shape = current.shape
        self.current = current.reshape(-1, 3).astype(np.float32)
        self.target = target.reshape(-1, 3).astype(np.float32)
        self.guide = guide.ravel().astype(np.float32)
        self.ids = ids
        self.tri = tri
        self.fit_weight = fit_weight.ravel().astype(np.float32)
        self.anchor_weight = (
            np.zeros_like(self.fit_weight)
            if anchor_weight is None
            else anchor_weight.ravel().astype(np.float32)
        )
        self.total_weight = self.fit_weight + self.anchor_weight
        self.target_delta = self.target - self.current
        self.anchor_delta = (
            np.zeros_like(self.target_delta)
            if anchor_delta is None
            else anchor_delta.reshape(-1, 3).astype(np.float32)
        )
        self.occupancy = np.bincount(
            ids.ravel(), weights=(tri * self.fit_weight[:, None]).ravel(), minlength=VERTICES
        ).astype(np.float32)

    def slice_params(self, vector: np.ndarray) -> np.ndarray:
        grid = vector.reshape(VERTICES, PARAMS)
        return np.sum(grid[self.ids] * self.tri[:, :, None], axis=1)

    def forward(self, vector: np.ndarray) -> np.ndarray:
        params = self.slice_params(vector)
        result = np.empty_like(self.current)
        result[:, 0] = (
            np.log(2.0) * self.current[:, 0] * params[:, 0]
            + self.current[:, 0] * params[:, 1]
            + (self.current[:, 0] - self.guide) * params[:, 4]
        )
        result[:, 1] = (
            np.log(2.0) * self.current[:, 1] * params[:, 0]
            + self.current[:, 1] * params[:, 2]
            + (self.current[:, 1] - self.guide) * params[:, 4]
        )
        result[:, 2] = (
            np.log(2.0) * self.current[:, 2] * params[:, 0]
            + self.current[:, 2] * params[:, 3]
            + (self.current[:, 2] - self.guide) * params[:, 4]
        )
        return result

    def adjoint(self, residual: np.ndarray) -> np.ndarray:
        per_pixel = np.empty((self.current.shape[0], PARAMS), dtype=np.float32)
        per_pixel[:, 0] = np.sum(np.log(2.0) * self.current * residual, axis=1)
        per_pixel[:, 1] = self.current[:, 0] * residual[:, 0]
        per_pixel[:, 2] = self.current[:, 1] * residual[:, 1]
        per_pixel[:, 3] = self.current[:, 2] * residual[:, 2]
        per_pixel[:, 4] = np.sum(
            (self.current - self.guide[:, None]) * residual, axis=1
        )
        out = np.empty((VERTICES, PARAMS), dtype=np.float32)
        flat_ids = self.ids.ravel()
        for parameter in range(PARAMS):
            out[:, parameter] = np.bincount(
                flat_ids,
                weights=(self.tri * per_pixel[:, parameter, None]).ravel(),
                minlength=VERTICES,
            )
        return out.ravel()

    def rhs(self) -> np.ndarray:
        observed = (
            self.fit_weight[:, None] * self.target_delta
            + self.anchor_weight[:, None] * self.anchor_delta
        )
        return self.adjoint(observed)

    def matvec(
        self, vector: np.ndarray, tikhonov: float, smooth: tuple[float, float, float]
    ) -> np.ndarray:
        predicted = self.forward(vector)
        out = self.adjoint(self.total_weight[:, None] * predicted)
        out += np.float32(tikhonov) * vector
        grid = vector.reshape(SY, SX, SB, PARAMS)
        lap = np.zeros_like(grid)
        for axis, weight in enumerate((smooth[1], smooth[0], smooth[2])):
            if weight <= 0.0:
                continue
            first = [slice(None)] * 4
            second = [slice(None)] * 4
            first[axis] = slice(0, -1)
            second[axis] = slice(1, None)
            first = tuple(first)
            second = tuple(second)
            delta = grid[second] - grid[first]
            lap[second] += np.float32(weight) * delta
            lap[first] -= np.float32(weight) * delta
        return out + lap.ravel()

    def solve(
        self,
        tikhonov: float,
        smooth: tuple[float, float, float],
        iterations: int = 60,
    ) -> tuple[np.ndarray, dict[str, float | int]]:
        rhs = self.rhs()
        vector = np.zeros_like(rhs)
        residual = rhs.copy()
        direction = residual.copy()
        rr = float(np.dot(residual.astype(np.float64), residual.astype(np.float64)))
        initial = max(rr, 1e-30)
        used = 0
        for used in range(1, iterations + 1):
            product = self.matvec(direction, tikhonov, smooth)
            denominator = float(
                np.dot(direction.astype(np.float64), product.astype(np.float64))
            )
            if denominator <= 1e-20:
                break
            alpha = rr / denominator
            vector += np.float32(alpha) * direction
            residual -= np.float32(alpha) * product
            next_rr = float(
                np.dot(residual.astype(np.float64), residual.astype(np.float64))
            )
            if next_rr <= initial * 1e-10:
                rr = next_rr
                break
            direction = residual + np.float32(next_rr / max(rr, 1e-30)) * direction
            rr = next_rr
        grid = vector.reshape(VERTICES, PARAMS)
        grid = np.clip(grid, BOUNDS_LOW, BOUNDS_HIGH)
        grid[self.occupancy < OCCUPANCY_MIN] = 0.0
        return grid, {"iterations": used, "relative_residual": (rr / initial) ** 0.5}

    def render(self, grid: np.ndarray) -> np.ndarray:
        delta = self.forward(grid.ravel())
        return np.clip(self.current + delta, 0.0, 1.0).reshape(self.shape)

    def weighted_rmse(self, rendered: np.ndarray) -> float:
        residual = rendered.reshape(-1, 3) - self.target
        denominator = max(float(self.fit_weight.sum()) * 3.0, 1e-12)
        return float(np.sqrt(np.sum(self.fit_weight[:, None] * residual * residual) / denominator))


def production_metric(base_path: Path, target_path: Path, candidate_path: Path) -> dict[str, float]:
    result = subprocess.run(
        [str(PROBE), "metric", str(base_path), str(target_path), str(candidate_path)],
        cwd=ROOT,
        env=os.environ.copy(),
        text=True,
        encoding="utf-8",
        errors="strict",
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    return json.loads(result.stdout)


def occupancy_map(grid: np.ndarray, occupancy: np.ndarray) -> list[list[str]]:
    active = (np.max(np.abs(grid), axis=1) > 1e-4) & (occupancy >= OCCUPANCY_MIN)
    maps: list[list[str]] = []
    for band in range(SB):
        rows = []
        for y in range(SY):
            row = ""
            for x in range(SX):
                index = (y * SX + x) * SB + band
                row += "#" if active[index] else ("+" if occupancy[index] >= OCCUPANCY_MIN else ".")
            rows.append(row)
        maps.append(rows)
    return maps


def locator_from_target(target: np.ndarray) -> np.ndarray:
    luma = target @ np.array([0.299, 0.587, 0.114], dtype=np.float32)
    gradient = np.abs(np.diff(luma, axis=0))
    height, width = luma.shape
    low, high = height // 4, 3 * height // 4
    crossings = low + np.argmax(gradient[low:high], axis=0)
    padded = np.pad(crossings, (15, 15), mode="edge")
    smooth = np.array(
        [np.median(padded[index : index + 31]) for index in range(width)], dtype=np.int32
    )
    mask = np.zeros((height, width), dtype=np.uint8)
    for x, crossing in enumerate(smooth):
        mask[crossing:, x] = 255
    return mask


def rim_module():
    spec = importlib.util.spec_from_file_location("rim_overshoot", ROOT / "scripts" / "rim_overshoot.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def rim_metric(
    pair: str, candidate_path: Path, base_path: Path, target: np.ndarray
) -> dict[str, float | int]:
    scale_size = (1536, 1024)
    rim_dir = OUT / "results" / "rim" / pair
    rim_dir.mkdir(parents=True, exist_ok=True)
    candidate_up = rim_dir / f"{candidate_path.stem}-up.png"
    base_up = rim_dir / "global-up.png"
    locator_up = rim_dir / "locator-up.png"
    if not candidate_up.exists():
        Image.open(candidate_path).convert("RGB").resize(
            scale_size, Image.Resampling.BILINEAR
        ).save(candidate_up)
    if not base_up.exists():
        Image.open(base_path).convert("RGB").resize(
            scale_size, Image.Resampling.BILINEAR
        ).save(base_up)
    if pair == "pair-0":
        corpus = Path(os.environ["AUTOSHOP_FIT_CALIBRATION_DIR"])
        supplied = list(corpus.glob("*.png"))
        if len(supplied) != 1:
            raise SystemExit("expected exactly one supplied calibration locator PNG")
        locator = np.asarray(
            Image.open(supplied[0]).convert("L").resize(
                (target.shape[1], target.shape[0]), Image.Resampling.BILINEAR
            ),
            dtype=np.uint8,
        )
        Image.fromarray(locator, "L").resize(scale_size, Image.Resampling.BILINEAR).save(locator_up)
    elif not locator_up.exists():
        locator = locator_from_target(target)
        Image.fromarray(locator, "L").resize(scale_size, Image.Resampling.BILINEAR).save(locator_up)
    readings = rim_module().overshoot(candidate_up, base_up, locator_up)
    if not len(readings):
        return {"n": 0, "mean": 0.0, "p90": 0.0, "max": 0.0}
    return {
        "n": int(len(readings)),
        "mean": float(readings.mean()),
        "p90": float(np.quantile(readings, 0.9)),
        "max": float(readings.max()),
    }


def projected_ranges(
    global_rgb: np.ndarray,
    grid_rgb: np.ndarray,
    guide: np.ndarray,
    fit_weight: np.ndarray,
) -> tuple[np.ndarray, list[list[float]]]:
    flat = global_rgb.reshape(-1, 3)
    desired = (grid_rgb - global_rgb).reshape(-1, 3)
    luma = guide.ravel()
    params = []
    for band in range(4):
        low, high = band / 4.0, (band + 1) / 4.0
        membership = (luma >= low) & (luma <= high)
        weights = fit_weight.ravel() * membership
        features = np.zeros((flat.shape[0] * 3, PARAMS), dtype=np.float64)
        observations = desired.ravel().astype(np.float64)
        for channel in range(3):
            rows = np.arange(flat.shape[0]) * 3 + channel
            features[rows, 0] = np.log(2.0) * flat[:, channel]
            features[rows, 1 + channel] = flat[:, channel]
            features[rows, 4] = flat[:, channel] - luma
        row_weight = np.repeat(weights, 3).astype(np.float64)
        normal = features.T @ (row_weight[:, None] * features) + 0.1 * np.eye(PARAMS)
        rhs = features.T @ (row_weight * observations)
        fitted = np.linalg.solve(normal, rhs).astype(np.float32)
        params.append(np.clip(fitted, BOUNDS_LOW, BOUNDS_HIGH))
    params_array = np.asarray(params, dtype=np.float32)
    centres = np.array([0.125, 0.375, 0.625, 0.875], dtype=np.float32)
    interpolation = np.empty((flat.shape[0], PARAMS), dtype=np.float32)
    for parameter in range(PARAMS):
        interpolation[:, parameter] = np.interp(
            luma, centres, params_array[:, parameter], left=params_array[0, parameter], right=params_array[-1, parameter]
        )
    projected = np.empty_like(flat)
    for channel in range(3):
        projected[:, channel] = (
            flat[:, channel]
            + np.log(2.0) * flat[:, channel] * interpolation[:, 0]
            + flat[:, channel] * interpolation[:, 1 + channel]
            + (flat[:, channel] - luma) * interpolation[:, 4]
        )
    return np.clip(projected, 0.0, 1.0).reshape(global_rgb.shape), params_array.tolist()


def payload_measurements(grid: np.ndarray) -> dict[str, int]:
    f16 = grid.astype("<f2").tobytes()
    f32 = grid.astype("<f4").tobytes()
    header_bytes = 4 + PARAMS * 2 * 4
    bounded_f16 = bytes(header_bytes) + f16
    return {
        "vertices": VERTICES,
        "parameters": VERTICES * PARAMS,
        "f16_raw": len(f16),
        "f16_base64": len(base64.b64encode(f16)),
        "f32_raw": len(f32),
        "f32_base64": len(base64.b64encode(f32)),
        "bounded_f16_header": header_bytes,
        "bounded_f16_raw": len(bounded_f16),
        "bounded_f16_base64": len(base64.b64encode(bounded_f16)),
    }


def pair_paths(index: int) -> tuple[Path, Path, Path]:
    pair = f"pair-{index}"
    global_path = RENDERS / pair / ("global-raw.png" if index == 0 else "global.png")
    shipped_path = RENDERS / pair / ("shipped-raw.png" if index == 0 else "shipped.png")
    return global_path, ANALYSIS / pair / "target.png", shipped_path


def main() -> None:
    for required in ("AUTOSHOP_DATA_DIR", "AUTOSHOP_FIT_CALIBRATION_DIR"):
        if not os.environ.get(required):
            raise SystemExit(f"{required} is required")
    GRID_OUT.mkdir(parents=True, exist_ok=True)
    all_results: dict[str, object] = {
        "grid": [SX, SY, SB],
        "parameter_order": ["ev", "gain_r_delta", "gain_g_delta", "gain_b_delta", "slope_delta"],
        "bounds": {"low": BOUNDS_LOW.tolist(), "high": BOUNDS_HIGH.tolist()},
        "occupancy_min": OCCUPANCY_MIN,
        "pairs": {},
    }
    selected_grid = None

    for index in range(5):
        pair = f"pair-{index}"
        pair_out = GRID_OUT / pair
        pair_out.mkdir(parents=True, exist_ok=True)
        global_path, target_path, shipped_path = pair_paths(index)
        icc_profile = Image.open(global_path).info.get("icc_profile")
        global_rgb = load_rgb(global_path)
        target_rgb = load_rgb(target_path)
        shipped_rgb = load_rgb(shipped_path)
        height, width = global_rgb.shape[:2]
        evidence = np.fromfile(PROBE_DATA / pair / "evidence.bin", dtype="<f4").reshape(height, width)
        structural = np.fromfile(
            PROBE_DATA / pair / "local-support.bin", dtype="<f4"
        ).reshape(height, width)
        unclipped = (
            (global_rgb.min(axis=2) > 1.0 / 255.0)
            & (global_rgb.max(axis=2) < 254.0 / 255.0)
            & (target_rgb.min(axis=2) > 1.0 / 255.0)
            & (target_rgb.max(axis=2) < 254.0 / 255.0)
        ).astype(np.float32)
        fit_weight = evidence * structural * unclipped
        guide = smooth_3tap(global_rgb @ np.array([0.299, 0.587, 0.114], dtype=np.float32))
        ids, tri = splat_table(guide)
        replace_system = GridSystem(global_rgb, target_rgb, guide, ids, tri, fit_weight)

        sweep_specs = []
        for tikhonov in (0.01, 0.1, 1.0, 10.0, 100.0, 1_000_000.0):
            sweep_specs.append((tikhonov, (1.0, 1.0, 1.0)))
        for smoothness in (0.0, 0.1, 10.0):
            sweep_specs.append((1.0, (smoothness, smoothness, smoothness)))
        sweep = []
        for sweep_index, (tikhonov, smoothness) in enumerate(sweep_specs):
            grid, solve_info = replace_system.solve(tikhonov, smoothness, iterations=50)
            rendered = replace_system.render(grid)
            candidate = pair_out / f"sweep-{sweep_index:02}.png"
            save_rgb(candidate, rendered, icc_profile)
            metric = production_metric(global_path, target_path, candidate)
            sweep.append(
                {
                    "tikhonov": tikhonov,
                    "smooth": list(smoothness),
                    "weighted_rmse": replace_system.weighted_rmse(rendered),
                    "metric": metric,
                    "max_abs": np.max(np.abs(grid), axis=0).tolist(),
                    **solve_info,
                    "candidate": candidate.name,
                }
            )
        finite = [item for item in sweep if np.isfinite(item["metric"]["total"])]
        best = min(finite, key=lambda item: item["metric"]["total"])
        best_spec = (float(best["tikhonov"]), tuple(float(v) for v in best["smooth"]))

        best_grid, best_solve = replace_system.solve(*best_spec, iterations=90)
        replace_rgb = replace_system.render(best_grid)
        replace_path = pair_out / "replace.png"
        save_rgb(replace_path, replace_rgb, icc_profile)
        replace_metric = production_metric(global_path, target_path, replace_path)

        stack_system = GridSystem(shipped_rgb, target_rgb, guide, ids, tri, fit_weight)
        stack_grid, stack_solve = stack_system.solve(*best_spec, iterations=90)
        stack_rgb = stack_system.render(stack_grid)
        stack_path = pair_out / "stack.png"
        save_rgb(stack_path, stack_rgb, icc_profile)
        stack_metric = production_metric(global_path, target_path, stack_path)

        shipped_delta = shipped_rgb - global_rgb
        correction = np.max(np.abs(shipped_delta), axis=2)
        anchor_weight = fit_weight * np.clip(correction / 0.05, 0.0, 1.0) * 2.0
        anchor_system = GridSystem(
            global_rgb,
            target_rgb,
            guide,
            ids,
            tri,
            fit_weight,
            anchor_delta=shipped_delta,
            anchor_weight=anchor_weight,
        )
        anchor_grid, anchor_solve = anchor_system.solve(*best_spec, iterations=90)
        anchor_rgb = anchor_system.render(anchor_grid)
        anchor_path = pair_out / "anchors.png"
        save_rgb(anchor_path, anchor_rgb, icc_profile)
        anchor_metric = production_metric(global_path, target_path, anchor_path)

        repeat_grid, _ = replace_system.solve(*best_spec, iterations=90)
        repeat_path = pair_out / "replace-repeat.png"
        save_rgb(repeat_path, replace_system.render(repeat_grid), icc_profile)
        sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()

        baseline = json.loads((PROBE_DATA / pair / "analysis.json").read_text(encoding="utf-8"))
        arrangements = {
            "replace": {
                "metric": replace_metric,
                "rim": rim_metric(pair, replace_path, global_path, target_rgb),
                "zero_law_before": baseline["global"]["total"],
                "zero_law_after": replace_metric["total"],
                "zero_law_holds": replace_metric["total"] <= baseline["global"]["total"],
                "solve": best_solve,
            },
            "stack": {
                "metric": stack_metric,
                "rim": rim_metric(pair, stack_path, global_path, target_rgb),
                "zero_law_before": baseline["shipped"]["total"],
                "zero_law_after": stack_metric["total"],
                "zero_law_holds": stack_metric["total"] <= baseline["shipped"]["total"],
                "solve": stack_solve,
            },
            "anchors": {
                "metric": anchor_metric,
                "rim": rim_metric(pair, anchor_path, global_path, target_rgb),
                "zero_law_before": baseline["global"]["total"],
                "zero_law_after": anchor_metric["total"],
                "zero_law_holds": anchor_metric["total"] <= baseline["global"]["total"],
                "anchor_weight_mass": float(anchor_weight.sum()),
                "solve": anchor_solve,
            },
        }
        baseline_rims = {
            "global": rim_metric(pair, global_path, global_path, target_rgb),
            "shipped": rim_metric(pair, shipped_path, global_path, target_rgb),
        }
        active = int(
            np.count_nonzero(
                (np.max(np.abs(best_grid), axis=1) > 1e-4)
                & (replace_system.occupancy >= OCCUPANCY_MIN)
            )
        )
        pair_result = {
            "fit_weight_mass": float(fit_weight.sum()),
            "fit_weight_nonzero": int(np.count_nonzero(fit_weight)),
            "baseline": baseline,
            "baseline_rim": baseline_rims,
            "sweep": sweep,
            "selected": {"tikhonov": best_spec[0], "smooth": list(best_spec[1])},
            "arrangements": arrangements,
            "occupancy": {
                "supported_vertices": int(np.count_nonzero(replace_system.occupancy >= OCCUPANCY_MIN)),
                "active_vertices": active,
                "maps": occupancy_map(best_grid, replace_system.occupancy),
            },
            "determinism": {
                "first_sha256": sha(replace_path),
                "second_sha256": sha(repeat_path),
                "equal": sha(replace_path) == sha(repeat_path),
            },
        }

        if index == 0:
            projection_rgb, projection_params = projected_ranges(
                global_rgb, replace_rgb, guide, fit_weight
            )
            projection_path = pair_out / "projection-4-ranges.png"
            save_rgb(projection_path, projection_rgb, icc_profile)
            projection_metric = production_metric(global_path, target_path, projection_path)
            grid_projection_rmse = float(np.sqrt(np.mean((projection_rgb - replace_rgb) ** 2)))
            pair_result["projection"] = {
                "constructs": 4,
                "metric_to_target": projection_metric,
                "rmse_to_grid": grid_projection_rmse,
                "parameters": projection_params,
                "rim": rim_metric(pair, projection_path, global_path, target_rgb),
            }
            pair_result["payload"] = payload_measurements(best_grid)
            start = time.perf_counter()
            for _ in range(20):
                replace_system.render(best_grid)
            elapsed = (time.perf_counter() - start) / 20.0
            pair_result["slice_timing"] = {
                "analysis_pixels": int(width * height),
                "analysis_seconds": elapsed,
                "linear_24mp_seconds": elapsed * 24_000_000 / (width * height),
            }
            selected_grid = best_grid

        all_results["pairs"][pair] = pair_result
        print(
            f"{pair}: global={baseline['global']['total']:.5f} "
            f"shipped={baseline['shipped']['total']:.5f} "
            f"grid={replace_metric['total']:.5f} stack={stack_metric['total']:.5f}"
        )

    assert selected_grid is not None
    (OUT / "results" / "experiment-results.json").write_text(
        json.dumps(all_results, indent=2, sort_keys=True) + "\n", encoding="ascii"
    )


if __name__ == "__main__":
    main()
