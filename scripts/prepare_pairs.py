"""Prepare anonymous, registered analysis pairs and current-path baselines.

All inputs are discovered inside AUTOSHADE_FIT_CALIBRATION_DIR.  Output labels
are pair-0 (the calibration pair) and pair-1..pair-4 (the numbered corpus), so
the resulting artifacts never retain a library photo name.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from pathlib import Path

from PIL import Image


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target" / "fitgrid-experiment"
ANALYSIS = OUT / "analysis"
RECIPES = OUT / "recipes"
EDGE = (384, 256)


def required_env(name: str) -> Path:
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"{name} is required")
    return Path(value)


def discover_pairs(corpus: Path) -> list[tuple[Path, Path]]:
    raws = sorted(corpus.glob("*.arw"), key=lambda path: path.stem.casefold())
    numbered = [path for path in raws if re.fullmatch(r"p\d+", path.stem, re.I)]
    generic = [path for path in raws if path not in numbered]
    if len(generic) != 1 or len(numbered) != 4:
        raise SystemExit("expected one calibration RAW and four numbered corpus RAWs")

    band_targets = sorted(corpus.glob("*-target.jpg"), key=lambda path: path.stem.casefold())
    by_prefix = {path.stem.removesuffix("-target").casefold(): path for path in band_targets}
    numbered_pairs = [(raw, by_prefix[raw.stem.casefold()]) for raw in numbered]

    plain_jpegs = [
        path
        for path in corpus.glob("*.jpg")
        if path not in band_targets and "preview" not in path.stem.casefold()
    ]
    # The calibration target is the smaller of the two plain baked renditions;
    # this distinguishes it without embedding either source photo's file name.
    calibration_target = min(plain_jpegs, key=lambda path: path.stat().st_size)
    return [(generic[0], calibration_target), *numbered_pairs]


def run_cli(args: list[str]) -> str:
    exe = ROOT / "target" / "release" / "autoshade.exe"
    proc = subprocess.run(
        [str(exe), *map(str, args)],
        cwd=ROOT,
        env=os.environ.copy(),
        text=True,
        encoding="utf-8",
        errors="replace",
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if proc.returncode:
        tail = "\n".join(proc.stdout.splitlines()[-12:])
        raise SystemExit(f"CLI failed ({proc.returncode}):\n{tail}")
    return proc.stdout


def registered(src: Path, dst: Path) -> None:
    with Image.open(src) as image:
        image.convert("RGB").resize(EDGE, Image.Resampling.LANCZOS).save(dst)


def rationale_measure(recipe: Path) -> dict[str, object]:
    data = json.loads(recipe.read_text(encoding="utf-8"))
    rationale = data.get("rationale", "")
    number = r"([0-9]+(?:\.[0-9]+)?)"
    match = re.search(rf"Residual look error {number}[^0-9]+{number}", rationale)
    return {
        "reported_before": float(match.group(1)) if match else None,
        "reported_after": float(match.group(2)) if match else None,
        "mask_count": len(data.get("masks", [])),
        "mask_names": [mask.get("name", "") for mask in data.get("masks", [])],
    }


def main() -> None:
    corpus = required_env("AUTOSHADE_FIT_CALIBRATION_DIR")
    required_env("AUTOSHADE_DATA_DIR")
    ANALYSIS.mkdir(parents=True, exist_ok=True)
    RECIPES.mkdir(parents=True, exist_ok=True)
    summary: dict[str, object] = {"analysis_size": list(EDGE), "pairs": {}}

    # Same registered raster is consumed by both fits.  Deliberately disable
    # semantic and correspondence sidecars: this is the shipped deterministic
    # range/tile fallback, and avoids importing a model into the phase-1 probe.
    os.environ["AUTOSHADE_SEGMENT_SCRIPT"] = "D:/no-such-dir/none.py"
    os.environ["AUTOSHADE_CORRESPOND_SCRIPT"] = "D:/no-such-dir/none.py"

    for index, (raw, target) in enumerate(discover_pairs(corpus)):
        label = f"pair-{index}"
        pair_dir = ANALYSIS / label
        render_dir = OUT / "results" / "renders" / label
        pair_dir.mkdir(parents=True, exist_ok=True)
        render_dir.mkdir(parents=True, exist_ok=True)
        preview = pair_dir / "decoded-preview.jpg"
        source_png = pair_dir / "source.png"
        target_png = pair_dir / "target.png"
        global_recipe = RECIPES / f"{label}-global.json"
        shipped_recipe = RECIPES / f"{label}-shipped.json"
        global_png = render_dir / "global.png"
        shipped_png = render_dir / "shipped.png"

        if not preview.exists():
            run_cli(["decode", raw, "-o", preview])
        if not source_png.exists():
            registered(preview, source_png)
        if not target_png.exists():
            registered(target, target_png)

        global_log = ""
        shipped_log = ""
        if not global_recipe.exists():
            global_log = run_cli(["match", source_png, target_png, "-o", global_recipe])
        if not shipped_recipe.exists():
            shipped_log = run_cli(
                ["match", source_png, target_png, "--zoned", "-o", shipped_recipe]
            )
        if not global_png.exists():
            run_cli(["apply", source_png, global_recipe, "-o", global_png])
        if not shipped_png.exists():
            run_cli(["apply", source_png, shipped_recipe, "-o", shipped_png])

        number = r"([0-9]+(?:\.[0-9]+)?)"
        cli_global = re.search(rf"look error {number}[^0-9]+{number}", global_log)
        cli_shipped = re.search(rf"look error {number}[^0-9]+{number}", shipped_log)
        summary["pairs"][label] = {
            "global": rationale_measure(global_recipe),
            "shipped": rationale_measure(shipped_recipe),
            "cli_global": [float(v) for v in cli_global.groups()] if cli_global else None,
            "cli_shipped": [float(v) for v in cli_shipped.groups()] if cli_shipped else None,
        }
        print(f"{label}: registered and rendered")

    (OUT / "results" / "baseline-summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="ascii"
    )


if __name__ == "__main__":
    main()
