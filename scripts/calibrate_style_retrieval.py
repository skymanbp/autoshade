#!/usr/bin/env python3
"""Offline calibration for the additive style/look retrieval terms.

The real run consumes ``AUTOSHOP_STYLE_INDEX`` (normally the index built from
the read-only RAW library).  ``--self-test`` uses a deterministic in-memory
fixture so the leave-one-out metric, grid search, and bootstrap are exercised
without a model download or a photo corpus.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import random
from pathlib import Path

FEATURE_WEIGHTS = (1.5, 1.0, 1.0, 0.5, 0.5, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.5)
ZSCORE_DIMS = (0, 1, 2, 10)
K = 4
BOOTSTRAP_REPEATS = 2000
GRID = (0.0, 0.5, 1.0, 2.0, 4.0, 8.0)


def unit(values):
    norm = math.sqrt(sum(v * v for v in values)) or 1.0
    return tuple(v / norm for v in values)


def text_proxy(tags):
    """Stable stand-in for the held-out tag phrase text embedding.

    The production sidecar embeds the phrase with SigLIP's text tower. The
    harness deliberately avoids loading that 1.5 GB checkpoint: a SHA-256
    seeded vector gives deterministic text-proxy comparisons and records the
    limitation in the output.
    """
    seed = hashlib.sha256(" ".join(tags).encode("utf-8")).digest()
    values = []
    for i in range(768):
        b = seed[i % len(seed)]
        values.append((b / 127.5) - 1.0)
        if i % len(seed) == len(seed) - 1:
            seed = hashlib.sha256(seed).digest()
    return unit(values)


def synthetic_records():
    records = []
    for i in range(10):
        feat = [0.0] * 14
        feat[0] = float(i)
        feat[1] = float(i % 3)
        feat[2] = float(i // 3)
        feat[10] = float(i) / 10.0
        image = [0.0] * 768
        image[i] = 1.0
        tags = ["warm golden tones" if i % 2 else "cool blue tones"]
        records.append({"feat": feat, "settings": {"exposure": i / 10.0, "contrast": float(i * 3)}, "embed": image, "tags": tags, "desc_embed": None})
    return {"mean": [0.0] * 14, "std": [1.0] * 14, "exemplars": records}


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(float(value))


def load_index(path):
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict) or not isinstance(data.get("exemplars"), list):
        raise ValueError("index root/exemplars are not valid")
    if len(data.get("mean", [])) != 14 or len(data.get("std", [])) != 14:
        raise ValueError("index normalization vectors must contain 14 values")
    records = []
    for i, raw in enumerate(data["exemplars"]):
        feat = raw.get("feat")
        if not isinstance(feat, list) or len(feat) != 14 or not all(finite(v) for v in feat):
            raise ValueError(f"exemplar {i} has invalid features")
        settings = raw.get("settings") or {}
        if not isinstance(settings, dict):
            raise ValueError(f"exemplar {i} settings are not an object")
        embed = raw.get("embed")
        if embed is not None and (len(embed) != 768 or not all(finite(v) for v in embed)):
            embed = None
        tags = raw.get("tags") or []
        if not tags:
            tags = [raw.get("tag", "neutral")]
        records.append({"feat": [float(v) for v in feat], "settings": {k: float(v) for k, v in settings.items() if finite(v)}, "embed": tuple(embed) if embed else None, "tags": tags, "desc_embed": tuple(raw["desc_embed"]) if isinstance(raw.get("desc_embed"), list) and len(raw["desc_embed"]) == 768 else None})
    return {"mean": [float(v) for v in data["mean"]], "std": [float(v) for v in data["std"]], "exemplars": records}


def normalize(feat, mean, std):
    out = list(feat)
    for i in ZSCORE_DIMS:
        out[i] = (out[i] - mean[i]) / max(abs(std[i]), 1e-4)
    return out


def cosine_distance(a, b):
    if a is None or b is None or len(a) != len(b):
        return 0.0
    return 1.0 - max(-1.0, min(1.0, sum(x * y for x, y in zip(a, b))))


def prepare(data):
    """Precompute the four distance components once per query/candidate pair.

    The weight grid then only sorts scalar combinations. This keeps the real
    169-exemplar run practical while leaving every distance and tie-break
    exactly as the production calculation specifies.
    """
    mean, std, records = data["mean"], data["std"], data["exemplars"]
    normalized = [normalize(r["feat"], mean, std) for r in records]
    queries = [i for i, r in enumerate(records) if r["settings"]]
    stats = {}
    for i in queries:
        for key, value in records[i]["settings"].items():
            stats.setdefault(key, []).append(value)
    centers = {
        k: (sum(v) / len(v), math.sqrt(sum((x - sum(v) / len(v)) ** 2 for x in v) / len(v)) or 1.0)
        for k, v in stats.items()
    }
    components = {}
    for qi in queries:
        q = records[qi]
        qtext = text_proxy(q["tags"])
        pairs = []
        for ci, candidate in enumerate(records):
            if ci == qi:
                continue
            d14 = sum(w * (a - b) ** 2 for w, a, b in zip(FEATURE_WEIGHTS, normalized[qi], normalized[ci]))
            pairs.append((d14, cosine_distance(q["embed"], candidate["embed"]), cosine_distance(qtext, candidate["embed"]), cosine_distance(qtext, candidate["desc_embed"]), ci))
        components[qi] = pairs
    return records, queries, centers, components


def evaluate(prepared, weights):
    records, queries, centers, components = prepared
    errors = {}
    for qi in queries:
        q = records[qi]
        scored = [(d14 + weights[0] * emb + weights[1] * txt + weights[2] * desc, ci) for d14, emb, txt, desc, ci in components[qi]]
        neighbours = [ci for _, ci in sorted(scored)[:K]]
        for key, actual in q["settings"].items():
            values = [records[ci]["settings"][key] for ci in neighbours if key in records[ci]["settings"]]
            if values:
                center, scale = centers[key]
                errors[(qi, key)] = abs((sum(values) / len(values) - actual) / scale)
    return errors


def bootstrap_ci(values):
    if not values:
        return None
    rng = random.Random(0)
    samples = [sum(values[rng.randrange(len(values))] for _ in values) / len(values) for _ in range(BOOTSTRAP_REPEATS)]
    samples.sort()
    return samples[int(0.025 * (len(samples) - 1))], samples[int(0.975 * (len(samples) - 1))]


def run(data, label):
    prepared = prepare(data)
    results = {}
    for wi in GRID:
        for wt in GRID[:5]:
            for wd in GRID[:5]:
                errors = evaluate(prepared, (wi, wt, wd))
                results[(wi, wt, wd)] = (sum(errors.values()) / len(errors) if errors else float("nan"), errors)
    baseline = results[(0.0, 0.0, 0.0)]
    candidates = [(key, val) for key, val in results.items() if math.isfinite(val[0])]
    best_key, best = min(candidates, key=lambda item: (item[1][0], item[0]))
    print(f"index: {label}")
    print(f"exemplars: {len(data['exemplars'])}; queries with settings: {sum(bool(r['settings']) for r in data['exemplars'])}")
    print("objective: leave-one-out pooled z-scored settings MAE; query TEXT proxy is the held-out exemplar's own tag phrase string (hashed here, not SigLIP)")
    print("W_EMB W_TXT W_DESC   settings_MAE   improvement_vs_baseline   paired_bootstrap_95CI")
    for key in [(0.0, 0.0, 0.0), best_key]:
        mae, errors = results[key]
        differences = [baseline[1][k] - errors[k] for k in baseline[1].keys() & errors.keys()]
        ci = bootstrap_ci(differences)
        ci_text = "n/a" if ci is None else f"[{ci[0]:+.6f}, {ci[1]:+.6f}]"
        print(f"{key[0]:5.1f} {key[1]:5.1f} {key[2]:6.1f}   {mae:.6f}   {baseline[0] - mae:+.6f}   {ci_text}")
    print(f"recommended defaults: W_EMB={best_key[0]:g}, W_TXT={best_key[1]:g}, W_DESC={best_key[2]:g}")
    print("note: leave-one-out observations are correlated; bootstrap is descriptive, and the hashed text proxy is not a replacement for a SigLIP text-tower run")
    return best_key


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--index", type=Path)
    args = ap.parse_args()
    if args.self_test:
        run(synthetic_records(), "synthetic self-test")
        return
    path = args.index or (Path(os.environ["AUTOSHOP_STYLE_INDEX"]) if os.environ.get("AUTOSHOP_STYLE_INDEX") else None)
    if path is None:
        raise SystemExit("set AUTOSHOP_STYLE_INDEX or pass --index (or use --self-test)")
    if not path.exists():
        raise SystemExit(f"style index not found: {path}")
    run(load_index(path), str(path))


if __name__ == "__main__":
    main()
