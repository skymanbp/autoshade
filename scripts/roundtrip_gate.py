# -*- coding: utf-8 -*-
"""The round-trip gate: does Lightroom show what the app shows for a reverse-fitted recipe?

For each pair of the calibration corpus (p36..p42) the release CLI reverse-fits the target
(`match --zoned --strength 0.85`), and the fitted recipe is rendered twice at the same long
edge: by this engine from the recipe (what the app shows) and by Adobe Camera Raw from the
Lightroom sidecar the fit wrote (what Lightroom shows; Camera Raw 18 is Lightroom's own
engine). The two renders are compared in CIE Lab (sRGB D65, CIE76) at a 1000 px long edge,
whole frame and by band of the Camera Raw render -- sky-ish (hue 190..280, C > 8), neutral
(C < 6), warm (hue 20..90, C > 12) -- twice: as rendered ("plain"), and with the engine's
lens-correction magnification undone first ("aligned": one scale about the frame centre,
estimated by tile phase correlation at 3000 px; up to +0.26 % on the 24-105 mm frames, a
quantified known difference of the distortion model, see ARCHITECTURE).

LINES (per pair, design of 2026-10-27, judged on the aligned comparison): whole-frame
dE <= 3; each of the sky / neutral / warm bands within +-2 L and +-3 a*, b* (mean of
engine - Camera Raw). A band with fewer than 300 pixels is not judged.

p42, the night pair, is judged on its own lines as well (its target was made in the app,
not in Lightroom): the engine render of the fitted recipe at 1536 px against the target,
measured by scripts/night_pair_measure.py -- whole-frame dE <= 5.31, horizon ripple <= 0.63,
horizon a* not more than 2 below the target's; star contrast is reported, never judged.
The same measure is run on the Camera Raw render, so "Lightroom would show this" is on
record for the night pair too.

REGRESSION. scripts/roundtrip_gate_expected.json holds the numbers of the first run with a
tolerance. A number worse than its expected value by more than the tolerance fails the gate
by name, and so does a line that passed when the table was written and fails now. A line
that already failed then is reported as a known miss with its number and does not fail the
gate (it is a ledger item, not a regression). `--record` rewrites the table from this run.

PHOTOSHOP. The oracle needs Photoshop (Camera Raw is its plug-in) driven through COM, which
only exists on a Windows machine with Photoshop installed. Without it the gate prints one
line saying it is skipped and why, and exits 0 -- CI has no Photoshop.

usage:
  python -X utf8 scripts/roundtrip_gate.py --probe                  # exit 0: can run; 3: skipped
  python -X utf8 scripts/roundtrip_gate.py --cli <autoshade.exe> [--corpus DIR] [--work DIR]
         [--pairs p36,p38] [--fits DIR] [--record]

--corpus defaults to $AUTOSHADE_FIT_CALIBRATION_DIR. --fits DIR reuses fits made earlier
(DIR/<pair>/ holding match.log and data/develops/<one folder>/ with recipe.json, <pair>.xmp
and the mask bitmaps) instead of fitting again. The sidecar scripts and weights reach the
fit through the usual AUTOSHADE_* environment (defaulted to <repo>/python/*.py). Camera Raw
renders take a machine-wide lock directory (AUTOSHADE_ACR_LOCK, default
<temp>/autoshade-acr.render.lock) so two runs never share the COM server at once.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

try:
    import numpy as np
    from PIL import Image
    Image.MAX_IMAGE_PIXELS = None
except ImportError as missing:  # the probe and the skip line need neither
    np = Image = None
    IMPORT_ERROR = missing
REPO = Path(__file__).resolve().parent.parent
HERE = Path(__file__).resolve().parent
EXPECTED = HERE / "roundtrip_gate_expected.json"
PAIRS = ["p36", "p37", "p38", "p39", "p40", "p41", "p42"]
NIGHT = "p42"
STRENGTH = "0.85"
CMP_W, ALIGN_W, NIGHT_LONG = 1000, 3000, 1536
LINE_DE, LINE_L, LINE_AB, MIN_BAND_PX = 3.0, 2.0, 3.0, 300
TOL = {"dE": 0.25, "L": 0.5, "ab": 0.5, "night_dE": 0.25, "ripple": 0.15, "night_a": 0.5}
BANDS = ("sky", "neutral", "warm")
SKIP_LINE = ("roundtrip_gate: SKIPPED round-trip gate (scripts/roundtrip_gate.py): {why}; "
             "the Camera Raw oracle needs Photoshop driven through COM, so the gate cannot run here.")

M = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]])


def lab(im):
    rgb = np.asarray(im.convert("RGB"), dtype=np.float64) / 255.0
    lin = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    xyz = lin @ M.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 0.008856, np.cbrt(xyz), 7.787 * xyz + 16 / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)


# ----------------------------------------------------------------- Photoshop
def photoshop_absent():
    """None when Photoshop's COM server is registered, else why not."""
    if os.name != "nt":
        return "not a Windows machine"
    if shutil.which("powershell") is None:
        return "powershell is not on PATH"
    r = subprocess.run(["powershell", "-NoProfile", "-Command",
                        "if ([type]::GetTypeFromProgID('Photoshop.Application')) { 'yes' } else { 'no' }"],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.stdout.strip() != "yes":
        return "Photoshop's COM server (Photoshop.Application) is not registered"
    return None


def set_attr(text, key, value):
    pat = re.compile(rf'(crs:{re.escape(key)}=")[^"]*(")')
    if pat.search(text):
        return pat.sub(lambda m: f"{m.group(1)}{value}{m.group(2)}", text, count=1)
    return re.sub(r'(crs:ProcessVersion="[^"]*")', rf'\1\n   crs:{key}="{value}"', text, count=1)


def oracle_render(raw, xmp, case_dir, size="MAXIMUM"):
    """Camera Raw's render of `raw` with the sidecar `xmp`, as a JPEG in case_dir."""
    case_dir.mkdir(parents=True, exist_ok=True)
    # A packet that says crs:AlreadyApplied="True" (one lifted from an export) makes Camera Raw
    # ignore every setting in it and render the RAW at its defaults; a sidecar beside a RAW says
    # False, as Lightroom's own do. The fit writes False; forced anyway so the gate cannot be fooled.
    text = set_attr(Path(xmp).read_text(encoding="utf-8"), "AlreadyApplied", "False")
    link = case_dir / Path(raw).name
    (case_dir / (link.stem + ".xmp")).write_text(text, encoding="utf-8", newline="\n")
    if not link.exists():
        try:
            os.link(raw, link)
        except OSError:
            shutil.copyfile(raw, link)
    out = case_dir / (link.stem + "-acr.jpg")
    if out.exists():
        out.unlink()
    lock = Path(os.environ.get("AUTOSHADE_ACR_LOCK") or Path(tempfile.gettempdir()) / "autoshade-acr.render.lock")
    while True:
        try:
            lock.mkdir()
            break
        except FileExistsError:
            time.sleep(2)
    try:
        r = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(HERE / "acr_render.ps1"),
                            str(link), str(out), size, "12"], capture_output=True, text=True, encoding="utf-8", errors="replace")
    finally:
        lock.rmdir()
    if not out.exists():
        raise RuntimeError(f"Camera Raw wrote no render for {link.name}: {(r.stdout + r.stderr).strip()[:300]}")
    return out


# -------------------------------------------------------------------- engine
def sidecar_env(data_dir):
    env = dict(os.environ)
    env["AUTOSHADE_DATA_DIR"] = str(data_dir)
    for s in ("segment", "embed", "describe", "correspond", "denoise", "depth", "face", "denoise_raw"):
        key = f"AUTOSHADE_{s.upper()}_SCRIPT"
        if key not in env and (REPO / "python" / f"{s}.py").exists():
            env[key] = str(REPO / "python" / f"{s}.py")
    if "AUTOSHADE_WEIGHTS_DIR" not in env and (REPO / "python" / "weights").is_dir():
        env["AUTOSHADE_WEIGHTS_DIR"] = str(REPO / "python" / "weights")
    return env


def fit(cli, raw, target, work):
    """Reverse-fit; returns (develop dir, look error [before, after])."""
    if work.exists():
        shutil.rmtree(work)
    (work / "out").mkdir(parents=True)
    r = subprocess.run([cli, "match", "--zoned", "--strength", STRENGTH, "-o", str(work / "out" / "matched.json"),
                        str(raw), str(target)], cwd=work / "out", env=sidecar_env(work / "data"),
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    (work / "match.log").write_text(r.stdout + r.stderr + f"\nmatch-exit={r.returncode}\n", encoding="utf-8")
    if r.returncode != 0:
        raise RuntimeError(f"match exited {r.returncode}: {(r.stderr or r.stdout).strip()[-300:]}")
    return develop_dir(work), look_error(r.stdout + r.stderr)


def develop_dir(work):
    devs = sorted((work / "data" / "develops").glob("*/recipe.json"))
    if len(devs) != 1:
        raise RuntimeError(f"expected one develop folder under {work}, found {len(devs)}")
    return devs[0].parent


def look_error(log):
    m = re.search(r"look error ([0-9.]+) → ([0-9.]+)", log)
    return [float(m.group(1)), float(m.group(2))] if m else None


def engine_render(cli, raw, recipe, out, long_edge, cwd, data_dir):
    if out.exists():
        out.unlink()  # apply refuses to overwrite, and a stale render would pass for this one
    r = subprocess.run([cli, "apply", str(raw), str(recipe), "--long-edge", str(long_edge), "--out", str(out)],
                       cwd=cwd, env=sidecar_env(data_dir), capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode != 0 or not out.exists():
        raise RuntimeError(f"apply exited {r.returncode}: {(r.stderr or r.stdout).strip()[-300:]}")
    return out


# --------------------------------------------------------------- comparison
def magnification(engine, oracle):
    """Engine scale relative to the oracle, by tile phase correlation (63 tiles) at 3000 px."""
    def g(im):
        return np.asarray(im.convert("L").resize((ALIGN_W, round(im.height * ALIGN_W / im.width)), Image.LANCZOS), float)
    a, b = g(engine), g(oracle)
    h, w = a.shape
    s, pts = 256, []
    for fy in np.linspace(0.05, 0.95, 7):
        for fx in np.linspace(0.05, 0.95, 9):
            x, y = int(fx * (w - s)), int(fy * (h - s))
            ta = a[y:y + s, x:x + s] - a[y:y + s, x:x + s].mean()
            tb = b[y:y + s, x:x + s] - b[y:y + s, x:x + s].mean()
            f = np.fft.fft2(ta) * np.conj(np.fft.fft2(tb))
            r = np.fft.ifft2(f / (np.abs(f) + 1e-9)).real
            if r.max() < 0.1:
                continue
            dy, dx = [v if v < s // 2 else v - s for v in np.unravel_index(r.argmax(), r.shape)]
            pts.append((x + s / 2 - w / 2, y + s / 2 - h / 2, dx, dy))
    if len(pts) < 8:
        return 1.0, len(pts)
    p = np.array(pts, float)
    k = (np.sum(p[:, 0] * p[:, 2]) + np.sum(p[:, 1] * p[:, 3])) / (np.sum(p[:, 0] ** 2) + np.sum(p[:, 1] ** 2))
    return 1.0 + k, len(pts)


def unmagnify(im, sc):
    """Output pixel (u, v) samples the engine render at centre + (u - centre) * sc."""
    w, h = im.size
    cx, cy = w / 2, h / 2
    return im.transform((w, h), Image.AFFINE, (sc, 0, cx - sc * cx, 0, sc, cy - sc * cy), Image.BICUBIC)


def rows(engine, oracle):
    """Whole-frame dE and the band rows, engine - oracle, at CMP_W, bands read on the oracle."""
    b = oracle.resize((CMP_W, round(oracle.height * CMP_W / oracle.width)), Image.LANCZOS)
    a = engine.resize(b.size, Image.LANCZOS)
    A, B = lab(a), lab(b)
    d = np.linalg.norm(A - B, axis=-1)
    C = np.hypot(B[..., 1], B[..., 2])
    h = np.degrees(np.arctan2(B[..., 2], B[..., 1])) % 360
    masks = {"sky": (h > 190) & (h < 280) & (C > 8), "neutral": C < 6, "warm": (h > 20) & (h < 90) & (C > 12)}
    out = {"whole": float(d.mean())}
    for name, m in masks.items():
        n = int(m.sum())
        out[name] = None if n < MIN_BAND_PX else {
            "dE": float(d[m].mean()), "dLab": [float(v) for v in (A[m] - B[m]).mean(0)], "n": n}
    return out


def judge_rows(r):
    """{line name: (value, passed)} for one comparison."""
    lines = {"whole dE": (r["whole"], r["whole"] <= LINE_DE)}
    for band in BANDS:
        if r[band] is None:
            continue
        dl, da, db = r[band]["dLab"]
        lines[f"{band} L"] = (dl, abs(dl) <= LINE_L)
        lines[f"{band} a"] = (da, abs(da) <= LINE_AB)
        lines[f"{band} b"] = (db, abs(db) <= LINE_AB)
    return lines


def night_lines(render, corpus):
    sys.path.insert(0, str(HERE))
    from night_pair_measure import measure, DE_LINE, RIPPLE_LINE, GREEN_MARGIN
    mask = corpus / "mask-region-2-sky.png"
    rep = measure(str(render), str(corpus / "p42-target.png"), str(mask) if mask.exists() else None)
    da = rep["green"]["delta_a"]
    return {"dE": (rep["dE"]["whole"], rep["dE"]["whole"] <= DE_LINE),
            "ripple": (rep["ripple"]["ripple"], rep["ripple"]["ripple"] <= RIPPLE_LINE),
            "horizon da": (da, da >= -GREEN_MARGIN),
            "stars (info)": (rep["stars_info_only"]["ratio_median"], None)}


# --------------------------------------------------------------------- main
def run_pair(name, args, corpus, work):
    raw = corpus / f"{name}.arw"
    target = corpus / (f"{name}-target.png" if name == NIGHT else f"{name}-target.jpg")
    for f in (raw, target):
        if not f.exists():
            raise RuntimeError(f"corpus file missing: {f.name}")
    pw = work / name
    t0 = time.time()
    if args.fits:
        dev = develop_dir(Path(args.fits) / name)
        log = Path(args.fits) / name / "match.log"
        le = look_error(log.read_text(encoding="utf-8", errors="replace")) if log.exists() else None
    else:
        dev, le = fit(args.cli, raw, target, pw / "fit")
    xmp = dev / f"{name}.xmp"
    if not xmp.exists():
        raise RuntimeError(f"the fit wrote no sidecar {xmp.name}")
    oracle = oracle_render(raw, xmp, pw / "oracle")
    o_im = Image.open(oracle).convert("RGB")
    long_edge = max(o_im.size)
    eng = engine_render(args.cli, raw, dev / "recipe.json", pw / "engine-recipe.png", long_edge, dev, pw / "data-apply")
    e_im = Image.open(eng).convert("RGB")
    if e_im.size != o_im.size:
        o_im = o_im.resize(e_im.size, Image.LANCZOS)
    sc, tiles = magnification(e_im, o_im)
    res = {"look_error": le, "long_edge": long_edge, "magnification": sc, "tiles": tiles,
           "plain": rows(e_im, o_im), "aligned": rows(unmagnify(e_im, sc), o_im)}
    if name == NIGHT:
        night = engine_render(args.cli, raw, dev / "recipe.json", pw / "engine-night.png", NIGHT_LONG, dev, pw / "data-apply")
        res["night_engine"] = night_lines(night, corpus)
        res["night_oracle"] = night_lines(oracle, corpus)
    res["seconds"] = round(time.time() - t0)
    return res


def fmt_row(r, band):
    x = r[band]
    if x is None:
        return f"{band:7s}   n/a (< {MIN_BAND_PX} px)"
    dl, da, db = x["dLab"]
    return f"{band:7s} dE {x['dE']:5.2f}  dL {dl:+6.2f}  da {da:+6.2f}  db {db:+6.2f}  n={x['n']}"


def compare_expected(name, res, exp):
    """Regression messages for one pair against its expected entry."""
    if exp is None:
        return [f"{name}: no expected entry (run with --record once)"]
    bad = []
    for view in ("plain", "aligned"):
        was = exp[view]
        for line, (v, ok) in judge_rows(res[view]).items():
            if line not in was:
                continue
            ev, eok = was[line]
            tol = TOL["dE"] if line == "whole dE" else (TOL["L"] if line.endswith(" L") else TOL["ab"])
            if abs(v) > abs(ev) + tol:
                bad.append(f"{name} {view} {line}: {v:+.2f} (expected {ev:+.2f}, tolerance {tol})")
            elif eok and not ok:
                bad.append(f"{name} {view} {line}: line failed now ({v:+.2f}), passed when recorded ({ev:+.2f})")
    for side in ("night_engine", "night_oracle"):
        for line, (v, ok) in res.get(side, {}).items():
            if ok is None or line not in exp.get(side, {}):
                continue
            ev, eok = exp[side][line]
            tol = {"dE": TOL["night_dE"], "ripple": TOL["ripple"], "horizon da": TOL["night_a"]}[line]
            worse = (v < ev - tol) if line == "horizon da" else (v > ev + tol)
            if worse:
                bad.append(f"{name} {side} {line}: {v:+.2f} (expected {ev:+.2f}, tolerance {tol})")
            elif eok and not ok:
                bad.append(f"{name} {side} {line}: line failed now ({v:+.2f}), passed when recorded ({ev:+.2f})")
    return bad


def main():
    ap = argparse.ArgumentParser(description="round-trip gate: engine render vs Camera Raw render of the fitted sidecar")
    ap.add_argument("--probe", action="store_true", help="exit 0 when the gate can run here, 3 (with the skip line) when not")
    ap.add_argument("--cli")
    ap.add_argument("--corpus", default=os.environ.get("AUTOSHADE_FIT_CALIBRATION_DIR"))
    ap.add_argument("--work", default=str(REPO / "target" / "battery" / "roundtrip"))
    ap.add_argument("--pairs", default=",".join(PAIRS))
    ap.add_argument("--fits")
    ap.add_argument("--expected", default=str(EXPECTED))
    ap.add_argument("--record", action="store_true")
    args = ap.parse_args()
    why = photoshop_absent()
    if why:
        print(SKIP_LINE.format(why=why))
        return 3 if args.probe else 0
    if args.probe:
        print("roundtrip_gate: Photoshop's COM server is registered; the gate can run")
        return 0
    if np is None:
        print(f"roundtrip_gate: cannot measure without numpy and Pillow ({IMPORT_ERROR})", file=sys.stderr)
        return 2
    if not args.cli or not Path(args.cli).exists():
        print(f"roundtrip_gate: --cli {args.cli!r} is not a file", file=sys.stderr)
        return 2
    if not args.corpus or not Path(args.corpus).is_dir():
        print("roundtrip_gate: no corpus (--corpus or AUTOSHADE_FIT_CALIBRATION_DIR)", file=sys.stderr)
        return 2
    corpus, work = Path(args.corpus), Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    exp_path = Path(args.expected)
    expected = json.loads(exp_path.read_text(encoding="utf-8")) if exp_path.exists() else {"pairs": {}}
    version = subprocess.run([args.cli, "--version"], capture_output=True, text=True).stdout.strip()
    print(f"roundtrip_gate: {version}; strength {STRENGTH}; lines (aligned) whole dE <= {LINE_DE}, "
          f"sky/neutral/warm |dL| <= {LINE_L}, |da|,|db| <= {LINE_AB}; p42 also dE <= 5.31, ripple <= 0.63, horizon da >= -2")
    results, failures, misses = {}, [], []
    for name in [p.strip() for p in args.pairs.split(",") if p.strip()]:
        print(f"\n== {name}", flush=True)
        try:
            res = run_pair(name, args, corpus, work)
        except Exception as e:  # a pair that cannot be measured fails the gate by name
            failures.append(f"{name}: {e}")
            print(f"   ERROR {e}")
            continue
        results[name] = res
        le = res["look_error"]
        print(f"   fit look error {le[0]:.3f} -> {le[1]:.3f}" if le else "   fit look error: not read",
              f"| long edge {res['long_edge']} | engine magnification vs Camera Raw {res['magnification']:.5f} "
              f"({res['tiles']} tiles) | {res['seconds']} s")
        for view in ("plain", "aligned"):
            r = res[view]
            lines = judge_rows(r)
            verdict = " ".join(f"{k}:{'PASS' if ok else 'FAIL'}" for k, (_, ok) in lines.items())
            print(f"   {view:7s} whole dE {r['whole']:5.2f}   [{verdict}]")
            for band in BANDS:
                print(f"            {fmt_row(r, band)}")
            if view == "aligned":
                misses += [f"{name} {k} {val:+.2f}" for k, (val, ok) in lines.items() if not ok]
        for side in ("night_engine", "night_oracle"):
            if side in res:
                print(f"   {side.replace('_', ' ')} vs target: " + "  ".join(
                    f"{k} {val:+.2f}" + ("" if ok is None else (" PASS" if ok else " FAIL")) for k, (val, ok) in res[side].items()))
                misses += [f"{name} {side} {k} {val:+.2f}" for k, (val, ok) in res[side].items() if ok is False]
        if not args.record:
            failures += compare_expected(name, res, expected["pairs"].get(name))
        sys.stdout.flush()
    (work / "roundtrip_report.json").write_text(json.dumps(results, indent=1), encoding="utf-8")
    if args.record:
        table = {"strength": float(STRENGTH), "tolerance": TOL, "recorded_with": version,
                 "recorded": time.strftime("%Y-%m-%d"), "pairs": dict(expected.get("pairs", {}))}
        for name, res in results.items():
            entry = {view: {k: [round(val, 3), ok] for k, (val, ok) in judge_rows(res[view]).items()}
                     for view in ("plain", "aligned")}
            for side in ("night_engine", "night_oracle"):
                if side in res:
                    entry[side] = {k: [round(val, 3), ok] for k, (val, ok) in res[side].items()}
            entry["look_error"] = res["look_error"]
            table["pairs"][name] = entry
        exp_path.write_text(json.dumps(table, indent=1) + "\n", encoding="utf-8", newline="\n")
        print(f"\nroundtrip_gate: expected table written: {exp_path}")
    print()
    for m in misses:
        print(f"roundtrip_gate: line missed: {m}")
    for f in failures:
        print(f"roundtrip_gate: FAILED {f}")
    print(f"roundtrip_gate: {len(results)} pairs measured, {len(misses)} lines missed, {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
