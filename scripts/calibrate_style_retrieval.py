#!/usr/bin/env python3
"""Offline calibration for the additive style/look retrieval terms.

What it measures
----------------
Leave-one-out pooled, z-scored settings MAE.  Every exemplar that carries
Lightroom settings is held out in turn, the remaining ones are ranked by the
production distance, the top ``K`` neighbours' settings are averaged, and the
error against the held-out exemplar's own settings is pooled across keys after
dividing by each key's spread (so a Kelvin and a contrast slider count the
same).  Lower is better; a WEIGHT is worth shipping only if it lowers this AND
its paired bootstrap CI against the 14-dim baseline excludes 0.

The query TEXT proxy is REAL
----------------------------
Until this batch the "text proxy" was a SHA-256-seeded vector, so W_TXT and
W_DESC were never calibrated at all: the harness measured the geometry of a
hash.  The proxy is now the held-out exemplar's own description text through
the pinned SigLIP 2 text tower, produced by ONE ``python/embed.py
--text-manifest`` batch over the whole index and cached
(``--proxies``/``--build-proxies``), so the 1.5 GB checkpoint is loaded once
per corpus and never during a grid sweep.

Raw vs standardised
-------------------
SigLIP image-to-text cosines are tiny and tightly clustered, so the raw
``1 - cos`` text term barely reorders anything and a grid over it "finds" 0 for
the wrong reason.  ``src/style.rs`` therefore z-scores the text and description
terms over each query's CANDIDATE SET before weighting them, and this harness
evaluates BOTH variants so the shipped one is the measured one.

``--self-test`` runs the whole pipeline on a deterministic in-memory fixture
with no model, no photo corpus and no cached proxies, and ASSERTS its
invariants rather than printing a table nobody reads.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

FEATURE_WEIGHTS = np.array(
    (1.5, 1.0, 1.0, 0.5, 0.5, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.5), dtype=np.float64
)
ZSCORE_DIMS = (0, 1, 2, 10)
K = 4
EMBED_DIM = 768
BOOTSTRAP_REPEATS = 2000
# Printed with every table: a CI that cannot be reproduced is a decoration.
BOOTSTRAP_SEED = 20260829
# The full grid the fix book names.  W_EMB has no 8.0 arm because the term is
# bounded by 2 and a weight that large makes the 14-dim block irrelevant rather
# than secondary; 2.0 IS evaluated, which the two-row table that replaced it
# with 4.0 never did.
GRID_EMB = (0.0, 1.0, 2.0, 4.0)
GRID_TXT = (0.0, 0.5, 1.0, 2.0, 4.0)
GRID_DESC = (0.0, 0.5, 1.0, 2.0, 4.0)
# Mirrors `style::MIN_STANDARDISATION_CANDIDATES`.
MIN_STANDARDISATION_CANDIDATES = 3
# Mirrors `style::MAX_DESC_CHARS`.
MAX_DESC_CHARS = 512


# ---------------------------------------------------------------- index I/O


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(float(value))


def desc_text(desc, tags):
    """The text a record's `desc_embed` is built from — `style::desc_text`.

    Kept identical on purpose: a proxy built from a different string than the
    stored vector would measure the wrong pair.
    """
    if isinstance(desc, str) and desc.strip():
        return desc.strip()[:MAX_DESC_CHARS]
    if tags:
        return ", ".join(tags)
    return None


def _vec(raw):
    if isinstance(raw, list) and len(raw) == EMBED_DIM and all(finite(v) for v in raw):
        return np.asarray(raw, dtype=np.float64)
    return None


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
        tags = raw.get("tags") or []
        if not tags:
            tags = [raw.get("tag", "neutral")]
        records.append(
            {
                "feat": np.asarray([float(v) for v in feat], dtype=np.float64),
                "settings": {k: float(v) for k, v in settings.items() if finite(v)},
                "embed": _vec(raw.get("embed")),
                "tags": [str(t) for t in tags],
                "desc": raw.get("desc"),
                "desc_embed": _vec(raw.get("desc_embed")),
            }
        )
    return {
        "mean": np.asarray([float(v) for v in data["mean"]], dtype=np.float64),
        "std": np.asarray([float(v) for v in data["std"]], dtype=np.float64),
        "exemplars": records,
        "provenance": data.get("embed_provenance"),
    }


def synthetic_records():
    """A fixture with the properties the metric needs and nothing else.

    Ten exemplars whose settings are a linear function of a feature, so the
    14-dim baseline already ranks well; an image-vector split by parity and a
    description split by thirds, both CUTTING ACROSS that ordering and both
    scaled to matter against it, so every weight in the grid is measurable.
    Everything is deterministic.
    """
    records = []
    for i in range(10):
        # Scaled so the 14-dim block and the cosine terms are COMPARABLE in
        # size: with a unit step per exemplar the feature block dwarfs a term
        # bounded by 2, and no weight in the grid could reorder anything.
        feat = np.zeros(14)
        feat[0] = 0.25 * i
        feat[1] = 0.25 * (i % 3)
        feat[2] = 0.25 * (i // 3)
        feat[10] = float(i) / 40.0
        # Two image clusters and three description clusters, both CUTTING
        # ACROSS the feature ordering: one-hot-per-exemplar vectors would be
        # mutually orthogonal, so every cosine gap would be the same constant
        # and no weight could reorder anything.
        image = np.zeros(EMBED_DIM)
        image[i % 2] = 1.0
        desc = np.zeros(EMBED_DIM)
        desc[100 + (i % 3)] = 1.0
        tags = ["warm golden tones" if i % 2 else "cool blue tones"]
        records.append(
            {
                "feat": feat,
                "settings": {"exposure": i / 10.0, "contrast": float(i * 3)},
                "embed": image,
                "tags": tags,
                "desc": None,
                "desc_embed": desc,
            }
        )
    return {
        "mean": np.zeros(14),
        "std": np.ones(14),
        "exemplars": records,
        "provenance": "synthetic",
    }


# -------------------------------------------------------------- text proxies


def proxy_texts(data):
    """The string whose SigLIP text vector stands in for each query's direction."""
    return [desc_text(r.get("desc"), r["tags"]) for r in data["exemplars"]]


def build_proxies(data, out_path, python_bin, script):
    """ONE `--text-manifest` batch over the whole index.

    A proxy per exemplar, in index order, cached with the provenance of the run
    that made it — the checkpoint, the tokenizer door and the exact texts —
    because a cache built through the OTHER tokenizer door (this batch's F-11)
    is not comparable and must not look identical.
    """
    texts = proxy_texts(data)
    live = [i for i, t in enumerate(texts) if t]
    if not live:
        raise SystemExit("no exemplar carries a description or tags, so there is no proxy to build")
    with tempfile.TemporaryDirectory() as tmp:
        manifest = Path(tmp) / "proxies.jsonl"
        result = Path(tmp) / "proxies.json"
        manifest.write_text(
            "\n".join(json.dumps({"text": texts[i]}, ensure_ascii=False) for i in live) + "\n",
            encoding="utf-8",
        )
        cmd = [python_bin, "-E", str(script), "--text-manifest", str(manifest), "--output", str(result)]
        print("proxy batch:", " ".join(cmd), file=sys.stderr)
        subprocess.run(cmd, check=True)
        payload = json.loads(result.read_text(encoding="utf-8"))
    vectors = payload["text_vectors"]
    if len(vectors) != len(live):
        raise SystemExit(f"sidecar returned {len(vectors)} vectors for {len(live)} texts")
    out = {
        "model": payload.get("model"),
        "revision": payload.get("revision"),
        "tokenizer": "GemmaTokenizer",
        "index_provenance": data.get("provenance"),
        "count": len(live),
        "order": live,
        "texts": [texts[i] for i in live],
        "vectors": vectors,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out), encoding="utf-8")
    print(f"wrote {out_path} ({len(live)} proxy vectors)")
    return out


def load_proxies(data, path):
    """The cached proxies, checked against the index they claim to describe."""
    cached = json.loads(path.read_text(encoding="utf-8"))
    texts = proxy_texts(data)
    live = [i for i, t in enumerate(texts) if t]
    if cached.get("order") != live or cached.get("texts") != [texts[i] for i in live]:
        raise SystemExit(
            f"{path} was built for a different index (different texts or order) — rebuild it "
            "with --build-proxies"
        )
    out = [None] * len(texts)
    for slot, vec in zip(cached["order"], cached["vectors"]):
        out[slot] = np.asarray(vec, dtype=np.float64)
    return out, cached


# ------------------------------------------------------------------- metric


def normalize_all(data):
    feats = np.stack([r["feat"] for r in data["exemplars"]])
    mean, std = data["mean"], data["std"]
    out = feats.copy()
    for i in ZSCORE_DIMS:
        out[:, i] = (out[:, i] - mean[i]) / max(abs(std[i]), 1e-4)
    return out


def cosine_gaps(query, matrix, present):
    """`1 - cos` per candidate, NaN where either side has no vector.

    NaN — not 0 — because "no vector" is not "distance 0": the standardisation
    below must exclude those candidates from the mean and the spread, exactly
    as `style::standardise` excludes `None`.
    """
    out = np.full(matrix.shape[0], np.nan)
    if query is None:
        return out
    dots = matrix @ query
    out[present] = 1.0 - np.clip(dots[present], -1.0, 1.0)
    return out


def prepare(data, proxies):
    """Every query's four raw distance components, once."""
    records = data["exemplars"]
    normalized = normalize_all(data)
    n = len(records)

    def stack(key):
        mat = np.zeros((n, EMBED_DIM))
        present = np.zeros(n, dtype=bool)
        for i, r in enumerate(records):
            if r[key] is not None:
                mat[i] = r[key]
                present[i] = True
        return mat, present

    img, img_present = stack("embed")
    desc, desc_present = stack("desc_embed")

    queries = [i for i, r in enumerate(records) if r["settings"]]
    stats = {}
    for i in queries:
        for key, value in records[i]["settings"].items():
            stats.setdefault(key, []).append(value)
    centers = {}
    for k, v in stats.items():
        m = sum(v) / len(v)
        centers[k] = (m, math.sqrt(sum((x - m) ** 2 for x in v) / len(v)) or 1.0)

    components = {}
    for qi in queries:
        others = np.array([c for c in range(n) if c != qi])
        d14 = ((normalized[others] - normalized[qi]) ** 2 * FEATURE_WEIGHTS).sum(axis=1)
        emb = cosine_gaps(records[qi]["embed"], img[others], img_present[others])
        qt = proxies[qi] if proxies else None
        txt = cosine_gaps(qt, img[others], img_present[others])
        dsc = cosine_gaps(qt, desc[others], desc_present[others])
        components[qi] = (others, d14, np.nan_to_num(emb, nan=0.0), txt, dsc)
    return records, queries, centers, components


def apply_term(gaps, w, standardised):
    """One weighted text term — `style::standardise` in numpy.

    A zero weight is the term's ABSENCE (exact zeros, no `0 * z`); below three
    comparable candidates, or a degenerate spread, the raw gap is used.
    """
    if w == 0.0:
        return np.zeros(gaps.shape[0])
    live = ~np.isnan(gaps)
    raw = np.where(live, np.nan_to_num(gaps, nan=0.0) * w, 0.0)
    if not standardised or live.sum() < MIN_STANDARDISATION_CANDIDATES:
        return raw
    vals = gaps[live]
    sd = vals.std()
    if not math.isfinite(sd) or sd <= 0.0:
        return raw
    z = (gaps - vals.mean()) / sd
    return np.where(live, np.nan_to_num(z, nan=0.0) * w, 0.0)


def evaluate(prepared, weights, standardised):
    """The pooled per-(query, setting) errors for one grid point."""
    records, queries, centers, components = prepared
    w_emb, w_txt, w_desc = weights
    errors = {}
    for qi in queries:
        others, d14, emb, txt, dsc = components[qi]
        score = d14 + w_emb * emb + apply_term(txt, w_txt, standardised) + apply_term(dsc, w_desc, standardised)
        order = np.lexsort((others, score))[:K]
        neighbours = others[order]
        for key, actual in records[qi]["settings"].items():
            values = [records[ci]["settings"][key] for ci in neighbours if key in records[ci]["settings"]]
            if values:
                _, scale = centers[key]
                errors[(qi, key)] = abs((sum(values) / len(values) - actual) / scale)
    return errors


def bootstrap_ci(differences, seed=BOOTSTRAP_SEED):
    """Paired bootstrap over the per-observation differences, SEEDED.

    The old spelling reseeded `random.Random(0)` per call, which is
    reproducible, but nothing printed the seed — so a reader could not tell a
    reproducible interval from an accidental one.
    """
    if len(differences) == 0:
        return None
    rng = np.random.default_rng(seed)
    idx = rng.integers(0, len(differences), size=(BOOTSTRAP_REPEATS, len(differences)))
    means = differences[idx].mean(axis=1)
    return float(np.quantile(means, 0.025)), float(np.quantile(means, 0.975))


def sweep(prepared):
    """Every grid point, in both variants."""
    rows = {}
    for standardised in (False, True):
        for w_emb in GRID_EMB:
            for w_txt in GRID_TXT:
                for w_desc in GRID_DESC:
                    # With both text weights at zero the two variants are the
                    # SAME point, not two: standardisation of an absent term is
                    # not a variant of anything.
                    if standardised and w_txt == 0.0 and w_desc == 0.0:
                        continue
                    key = (w_emb, w_txt, w_desc, standardised)
                    rows[key] = evaluate(prepared, (w_emb, w_txt, w_desc), standardised)
    return rows


def report(data, label, proxies, proxy_note, out=sys.stdout):
    prepared = prepare(data, proxies)
    rows = sweep(prepared)
    baseline_key = (0.0, 0.0, 0.0, False)
    baseline = rows[baseline_key]
    base_mae = sum(baseline.values()) / len(baseline)

    table = []
    for key, errors in rows.items():
        mae = sum(errors.values()) / len(errors)
        shared = sorted(baseline.keys() & errors.keys())
        diffs = np.array([baseline[k] - errors[k] for k in shared])
        ci = bootstrap_ci(diffs)
        table.append((mae, key, base_mae - mae, ci))
    table.sort(key=lambda r: (r[0], r[1]))

    print(f"index: {label}", file=out)
    print(
        f"exemplars: {len(data['exemplars'])}; queries with settings: "
        f"{sum(bool(r['settings']) for r in data['exemplars'])}; K={K}",
        file=out,
    )
    print(f"text proxy: {proxy_note}", file=out)
    print(
        "objective: leave-one-out pooled z-scored settings MAE; paired bootstrap "
        f"{BOOTSTRAP_REPEATS} resamples, seed {BOOTSTRAP_SEED}",
        file=out,
    )
    print("W_EMB W_TXT W_DESC  variant       settings_MAE  improvement  paired_bootstrap_95CI", file=out)
    for mae, key, gain, ci in table:
        w_emb, w_txt, w_desc, std = key
        ci_text = "n/a" if ci is None else f"[{ci[0]:+.6f}, {ci[1]:+.6f}]"
        variant = "standardised" if std else "raw         "
        marker = "  <- baseline" if key == baseline_key else ""
        print(
            f"{w_emb:5.1f} {w_txt:5.1f} {w_desc:6.1f}  {variant}  {mae:12.6f}  {gain:+11.6f}  {ci_text}{marker}",
            file=out,
        )

    # SHIPPABLE = beats the baseline with a CI that excludes 0. Anything else
    # ships at the baseline's own value, which for the text terms is 0.
    shippable = [r for r in table if r[3] is not None and r[3][0] > 0.0 and r[2] > 0.0]
    print(file=out)
    if shippable:
        mae, key, gain, ci = shippable[0]
        print(
            f"recommended defaults: W_EMB={key[0]:g}, W_TXT={key[1]:g}, W_DESC={key[2]:g}, "
            f"variant={'standardised' if key[3] else 'raw'} "
            f"(MAE {mae:.6f}, {gain:+.6f} vs baseline, CI [{ci[0]:+.6f}, {ci[1]:+.6f}])",
            file=out,
        )
    else:
        print("recommended defaults: none — no grid point beat the 14-dim baseline with a CI excluding 0", file=out)
    print(
        "limitation: the leave-one-out observations are CORRELATED (every exemplar appears in "
        "other queries' neighbourhoods), so the bootstrap interval is descriptive of this corpus "
        "and is not a population confidence interval",
        file=out,
    )
    return table, shippable


# ---------------------------------------------------------------- self-test


def self_test():
    """Assertions, not a printed table.

    The old `--self-test` ran `run()` and returned: it could not fail unless
    the code crashed, and it printed the baseline row twice.
    """
    data = synthetic_records()
    n = len(data["exemplars"])

    # The proxy rule is the sidecar's rule.
    assert desc_text("  a hazy dawn  ", ["x"]) == "a hazy dawn"
    assert desc_text(None, ["warm", "vivid"]) == "warm, vivid"
    assert desc_text(None, []) is None
    assert len(desc_text("d" * 900, [])) == MAX_DESC_CHARS

    # A zero weight is the term's ABSENCE, bit for bit, in both variants.
    gaps = np.array([0.1, 0.2, np.nan, 0.4])
    for standardised in (False, True):
        zero = apply_term(gaps, 0.0, standardised)
        assert np.array_equal(zero, np.zeros(4)), zero
        assert not np.signbit(zero).any()

    # Standardisation is centred and scaled over the LIVE candidates only.
    z = apply_term(gaps, 1.0, True)
    live = ~np.isnan(gaps)
    assert abs(z[live].mean()) < 1e-12, z
    assert abs(z[live].std() - 1.0) < 1e-12, z
    assert z[~live] == 0.0

    # …and it falls back to raw below three live candidates.
    few = np.array([0.1, 0.2, np.nan])
    assert np.allclose(apply_term(few, 2.0, True), np.array([0.2, 0.4, 0.0])), apply_term(few, 2.0, True)

    # A degenerate spread also falls back rather than dividing by zero.
    flat = np.array([0.3, 0.3, 0.3, 0.3])
    assert np.allclose(apply_term(flat, 2.0, True), np.full(4, 0.6))

    # The metric moves with the weights, and the baseline is reproducible.
    proxies = [r["desc_embed"] for r in data["exemplars"]]
    prepared = prepare(data, proxies)
    base = evaluate(prepared, (0.0, 0.0, 0.0), False)
    again = evaluate(prepared, (0.0, 0.0, 0.0), False)
    assert base == again, "the metric must be deterministic"
    assert len(base) > 0
    moved = evaluate(prepared, (4.0, 0.0, 0.0), False)
    assert moved != base, "a non-zero W_EMB must be able to change the ranking"
    # W_EMB = 0 reproduces the 14-dim ranking exactly, whatever the text
    # weights' variant — the property `style.rs` pins on the Rust side.
    assert evaluate(prepared, (0.0, 0.0, 0.0), True) == base

    # The bootstrap is seeded: same input, same interval, and a shifted input
    # moves it.
    diffs = np.linspace(-0.01, 0.03, 50)
    assert bootstrap_ci(diffs) == bootstrap_ci(diffs)
    assert bootstrap_ci(diffs + 1.0)[0] > bootstrap_ci(diffs)[0]
    assert bootstrap_ci(np.array([])) is None

    # The grid is the full one, and the two variants do not double-count the
    # rows where both text weights are zero.
    rows = sweep(prepared)
    expected = len(GRID_EMB) * len(GRID_TXT) * len(GRID_DESC) * 2 - len(GRID_EMB)
    assert len(rows) == expected, (len(rows), expected)
    assert (0.0, 0.0, 0.0, False) in rows
    assert (2.0, 0.0, 0.0, False) in rows, "the shipped-before weight must be evaluated"

    # The report prints EVERY row, not two.
    import io as _io

    buf = _io.StringIO()
    table, _ = report(data, "synthetic self-test", proxies, "synthetic fixture vectors", out=buf)
    text = buf.getvalue()
    assert len(table) == expected
    assert text.count("\n") >= expected + 5, "every grid row must be printed"
    assert str(BOOTSTRAP_SEED) in text, "the seed must be printed with the table"
    assert "<- baseline" in text
    assert n == 10

    print(f"calibrate_style_retrieval self-test: PASS ({expected} grid rows, {len(base)} observations)")


# --------------------------------------------------------------------- main


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--self-test", action="store_true", help="assert the harness's own invariants; needs no corpus")
    ap.add_argument("--index", type=Path, help="style-index.json (default: $AUTOSHOP_STYLE_INDEX)")
    ap.add_argument("--proxies", type=Path, help="cached SigLIP text proxies (default: <index dir>/text-proxies.json)")
    ap.add_argument("--build-proxies", action="store_true", help="run ONE sidecar text batch and write the proxy cache")
    ap.add_argument("--python", default=os.environ.get("AUTOSHOP_PYTHON", sys.executable))
    ap.add_argument("--script", type=Path, default=Path(__file__).resolve().parent.parent / "python" / "embed.py")
    args = ap.parse_args()

    if args.self_test:
        self_test()
        return

    path = args.index or (Path(os.environ["AUTOSHOP_STYLE_INDEX"]) if os.environ.get("AUTOSHOP_STYLE_INDEX") else None)
    if path is None:
        raise SystemExit("set AUTOSHOP_STYLE_INDEX or pass --index (or use --self-test)")
    if not path.exists():
        raise SystemExit(f"style index not found: {path}")
    data = load_index(path)

    proxy_path = args.proxies or path.parent / "text-proxies.json"
    if args.build_proxies:
        build_proxies(data, proxy_path, args.python, args.script)
        return
    if proxy_path.exists():
        proxies, cached = load_proxies(data, proxy_path)
        note = (
            f"SigLIP 2 text tower, {cached.get('model')}@{str(cached.get('revision'))[:12]} "
            f"tokenizer={cached.get('tokenizer')}, {cached.get('count')} vectors, cached at {proxy_path}"
        )
    else:
        raise SystemExit(
            f"no text proxies at {proxy_path} — run with --build-proxies first (ONE sidecar batch), "
            "or the text terms would be measured against nothing"
        )
    report(data, str(path), proxies, note)


if __name__ == "__main__":
    main()
