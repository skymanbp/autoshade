"""Measure this engine's mask laws against Lightroom's own exported pixels.

Reads the `me6-2026-09` Lightroom pack (46 sidecars, 46 exports, one wall
capture) and the renders the Rust probe
`render::lr_pack::export_lr_pack_renders_for_the_mask_measurement` writes, and
reports, per export, where Lightroom puts its mask and where this engine puts
its own.

    cargo test --offline --release --lib -- --ignored --nocapture \\
        export_lr_pack_renders_for_the_mask_measurement
    python scripts/lr_mask_parity.py --pack <me6 dir> --probe <AUTOSHADE_DATA_DIR>/lr-probe

# The instrument: reading coverage out of an 8-bit JPEG

Every mask in the pack carries the same local exposure, −1.00 EV
(`crs:LocalExposure2012="-0.25"`), on a flat wall. Lightroom applies a local
exposure in ITS linear working space and then a fixed tone curve, so the
exported value of a pixel whose coverage is α is

    v(α) = T( T⁻¹(v_ref) · 2^(EV·α) )

for the export's own maskless reference `v_ref`. Write `W(v) = log2 T⁻¹(v)` —
the value in STOPS — and the tone curve cancels:

    α = ( W(v_ref) − W(v) ) / |EV|          with EV = −1, so α = W(v_ref) − W(v)

`W` is not published by Adobe, so it is MEASURED from the pack itself. Two
facts pin it:

* Group A's feather-0 masks are α = 1 over a large region, so every pixel pair
  inside one satisfies `W(v) − W(v_x) = 1` — Abel's functional equation for
  the halving map, over the whole 24…244 DN range the wall covers.
* Inside one export, all pixels at the same coverage coordinate (elliptical ρ,
  or gradient t) share ONE α whatever their base brightness. The wall's own
  4:1 brightness range therefore over-determines `W`.

Both are linear in `W` once each bin's α is carried as its own unknown, so the
whole calibration is one weighted least-squares solve (`fit_tone_coordinate`)
with a second-difference smoothness term. The shipped weight is
`TONE_SMOOTHNESS`; it is the value at which the recovered α reproduces the two
anchors it was not free to move — group A reads α = 0.9989 inside and exactly
0 outside — and at which α is independent of base brightness to 0.005.

That 0.005 is this instrument's floor, and every residual below it is reported
as "at the floor" rather than as a number.
"""

from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path

import numpy as np

# ---------------------------------------------------------------------------
# the pack
# ---------------------------------------------------------------------------

#: Second-difference weight in `fit_tone_coordinate`. See the module header.
TONE_SMOOTHNESS = 50.0

#: Base-value buckets the calibration samples the halving map on, in DN.
TONE_BUCKETS = np.arange(24, 244, 6)

#: The coverage floor this instrument resolves, in α. Below it a residual is
#: reported as "at the floor".
ALPHA_FLOOR = 0.005

#: The exponent `render.rs`'s `LINEAR_FALLOFF_WARP` ships. It is fitted BELOW,
#: per gradient, as `fit_warp_q`; this constant only says which value the engine
#: was given, so `fit_warped` scores the shipped law rather than a fresh fit.
SHIPPED_WARP = 1.124


def read_green(path: Path) -> np.ndarray:
    """The green channel of a JPEG or PNG, as `int32` DN.

    Green because it is the channel every earlier Lightroom-parity round
    measured on, and because a Bayer sensor samples it twice as densely as the
    other two, so it carries the least demosaic invention.
    """
    import imageio.v3 as iio

    a = iio.imread(path)
    return (a[..., 1] if a.ndim == 3 else a).astype(np.int32)


def read_alpha(path: Path) -> np.ndarray:
    """A 16-bit grey α raster written by the Rust probe, as float in [0, 1]."""
    import imageio.v3 as iio

    return iio.imread(path).astype(np.float32) / 65535.0


class Pack:
    """The me6 fixture plus the probe's renders, with the frame it is measured in."""

    def __init__(self, pack: Path, probe: Path) -> None:
        self.pack = pack
        self.probe = probe
        self.spec = json.loads((pack / "pack-spec.json").read_text(encoding="utf-8"))
        self.width, self.height = (int(v) for v in self.spec["frame"])
        y, x = np.mgrid[0 : self.height, 0 : self.width]
        # Pixel CENTRES — the convention `render::MASK_SAMPLE_CENTRE` names, and
        # the one Lightroom was measured to use (ARCHITECTURE, "A mask is
        # sampled at PIXEL CENTRES").
        self.nx = ((x + 0.5) / self.width).astype(np.float32)
        self.ny = ((y + 0.5) / self.height).astype(np.float32)
        self._green: dict[str, np.ndarray] = {}

    def exports(self) -> list[dict]:
        return self.spec["exports"]

    def lr(self, code: str) -> np.ndarray:
        if code not in self._green:
            self._green[code] = read_green(self.pack / "lr" / f"{code}.jpg")
        return self._green[code]

    def alpha(self, code: str, stored: bool = False) -> np.ndarray:
        suffix = "alpha-stored" if stored else "alpha"
        return read_alpha(self.probe / f"{code}.{suffix}.png")

    def reference_of(self, code: str) -> str:
        """The maskless export this one is differenced against.

        Group E has no REF of its own and is lens-correction OFF, so it borrows
        C's — the two are the same develop with no mask.
        """
        group = code.split("-")[0]
        if group in ("A", "B"):
            return f"{group}-REF-{'ON' if code.endswith('-ON') else 'OFF'}"
        return {"C": "C-REF", "D": "D-REF", "E": "C-REF"}[group]

    def radial(self, corr: dict) -> tuple[float, float, float, float, float]:
        """`(cx, cy, rx, ry, angle_deg)` in frame fractions for a radial spec.

        The spec's `args` are `[cx, cy, width, height, feather, roundness?,
        angle?]` — the same numbers `MANIFEST.md` tabulates.
        """
        a = corr["args"]
        cx, cy, w, h = a[0], a[1], a[2], a[3]
        angle = a[6] if len(a) > 6 else 0.0
        return cx, cy, w / 2.0, h / 2.0, angle

    def rho(self, corr: dict) -> np.ndarray:
        """Elliptical radius for a radial correction, in the STORED frame.

        The angle is folded the way `xmp::lr_to_engine` folds it: the stored
        box half-extents are the ellipse's own semi-axes ROTATED by θ in PIXEL
        space, so the semi-axes come back by rotating them out again. That
        fold is what group D measures, so it is spelled out rather than
        assumed — with `abs`, because an ellipse is defined by rx², ry².
        """
        cx, cy, xn, yn, angle = self.radial(corr)
        s = self.width / self.height
        th = math.radians(angle)
        a = xn * s * math.cos(th) + yn * math.sin(th)
        b = -xn * s * math.sin(th) + yn * math.cos(th)
        # Back to frame fractions: `a`/`b` are in HEIGHT units along the
        # ellipse's own axes.
        rx, ry = abs(a) / s, abs(b)
        px = self.nx - cx
        py = self.ny - cy
        u = px * math.cos(th) + py * math.sin(th) * (self.height / self.width)
        v = -px * math.sin(th) * (self.width / self.height) + py * math.cos(th)
        return np.sqrt((u / rx) ** 2 + (v / ry) ** 2)

    def gradient_t(self, corr: dict) -> np.ndarray:
        """The handle-axis parameter of a linear gradient: 0 at Zero, 1 at Full."""
        zx, zy, fx, fy = corr["args"]
        vx, vy = fx - zx, fy - zy
        return ((self.nx - zx) * vx + (self.ny - zy) * vy) / (vx * vx + vy * vy)


# ---------------------------------------------------------------------------
# the tone coordinate
# ---------------------------------------------------------------------------


def calibration_cells(pack: Pack) -> list[dict]:
    """Sample the (v_ref, v_x) transfer per coverage bin per brightness bucket.

    One cell = one export × one coverage bin. Its `loc` rows are the median
    (reference, exported) pair inside one brightness bucket, with the pixel
    count. `anchor` is the coverage where it is KNOWN — 1 well inside a
    feather-0 mask, 0 outside any mask — and `None` where the whole point is
    that it is not known.
    """
    picks = [
        ("C-A12-F25", "rho"),
        ("C-A12-F75", "rho"),
        ("C-A40-F25", "rho"),
        ("C-A40-F75", "rho"),
        ("C-F7", "rho"),
        ("B-V2-OFF", "t"),
        ("B-H2-OFF", "t"),
        ("A-S-OFF", "rho"),
        ("A-M-OFF", "rho"),
        ("A-L-OFF", "rho"),
    ]
    by_code = {e["code"]: e for e in pack.exports()}
    cells: list[dict] = []
    for code, kind in picks:
        corr = by_code[code]["corrections"][0]
        cov = pack.rho(corr) if kind == "rho" else pack.gradient_t(corr)
        edges = np.linspace(0.05, 1.55, 61) if kind == "rho" else np.linspace(-0.05, 1.05, 61)
        ref = pack.lr(pack.reference_of(code))
        val = pack.lr(code)
        index = np.digitize(cov, edges) - 1
        for b in range(len(edges) - 1):
            inside = index == b
            if inside.sum() < 20_000:
                continue
            rb, vb = ref[inside], val[inside]
            rows = []
            for k in range(len(TONE_BUCKETS) - 1):
                s = (rb >= TONE_BUCKETS[k]) & (rb < TONE_BUCKETS[k + 1])
                if s.sum() < 1_500:
                    continue
                rows.append([float(np.median(rb[s])), float(np.median(vb[s])), int(s.sum())])
            if len(rows) < 3:
                continue
            centre = 0.5 * (edges[b] + edges[b + 1])
            # A coverage is ANCHORED only where the mask's own definition fixes
            # it: inside and outside a FEATHER-0 ellipse, and beyond either
            # handle of a gradient. A feathered radial is deliberately not
            # anchored anywhere — its tail reaches ρ ≈ √2, so calling the far
            # field zero would write the answer into the instrument.
            anchor = None
            if code.startswith("A-"):
                if centre < 0.90:
                    anchor = 1.0
                elif centre > 1.05:
                    anchor = 0.0
            elif kind == "t" and centre < -0.02:
                anchor = 0.0
            elif kind == "t" and centre > 1.02:
                anchor = 1.0
            cells.append({"code": code, "centre": centre, "loc": rows, "anchor": anchor})
    return cells


def fit_tone_coordinate(cells: list[dict], smoothness: float = TONE_SMOOTHNESS) -> np.ndarray:
    """Solve for `W(v)`, the exported DN expressed in STOPS. See the header.

    Unknowns are `W[0..255]` and one α per cell. Every equation is linear:
    `W(v_ref) − W(v_x) − α_cell = 0` for each brightness bucket, `α_cell =
    anchor` where the coverage is known, a second-difference penalty on `W`,
    and one gauge (`W(200) = 0`) to fix the free additive constant.
    """
    import scipy.sparse as sp
    from scipy.sparse.linalg import lsqr

    n = 256
    rows: list[int] = []
    cols: list[int] = []
    vals: list[float] = []
    rhs: list[float] = []
    weight: list[float] = []
    eq = 0

    def interp(t: float) -> tuple[tuple[int, float], tuple[int, float]]:
        t = float(np.clip(t, 0, n - 1.001))
        i = int(math.floor(t))
        f = t - i
        return (i, 1.0 - f), (i + 1, f)

    for b, cell in enumerate(cells):
        for v_ref, v_x, count in cell["loc"]:
            for j, c in interp(v_ref):
                rows.append(eq), cols.append(j), vals.append(c)
            for j, c in interp(v_x):
                rows.append(eq), cols.append(j), vals.append(-c)
            rows.append(eq), cols.append(n + b), vals.append(-1.0)
            rhs.append(0.0)
            # The count is capped so one enormous bucket cannot outvote the
            # rest of the ladder; the square root is the usual counting weight.
            weight.append(math.sqrt(min(count, 20_000)) / 50.0)
            eq += 1
        if cell["anchor"] is not None:
            rows.append(eq), cols.append(n + b), vals.append(1.0)
            rhs.append(cell["anchor"])
            weight.append(30.0)
            eq += 1
    for i in range(n - 2):
        for j, c in ((i, 1.0), (i + 1, -2.0), (i + 2, 1.0)):
            rows.append(eq), cols.append(j), vals.append(smoothness * c)
        rhs.append(0.0), weight.append(1.0)
        eq += 1
    rows.append(eq), cols.append(200), vals.append(100.0)
    rhs.append(0.0), weight.append(1.0)
    eq += 1

    a = sp.csr_matrix((np.array(vals), (np.array(rows), np.array(cols))), shape=(eq, n + len(cells)))
    w = np.array(weight)
    solution = lsqr(sp.diags(w) @ a, np.array(rhs) * w, atol=1e-13, btol=1e-13, iter_lim=200_000)[0]
    return solution[:n]


# ---------------------------------------------------------------------------
# measurements
# ---------------------------------------------------------------------------


def bilinear(a: np.ndarray, xs: np.ndarray, ys: np.ndarray) -> np.ndarray:
    x0 = np.floor(xs).astype(int)
    y0 = np.floor(ys).astype(int)
    x1 = np.clip(x0 + 1, 0, a.shape[1] - 1)
    y1 = np.clip(y0 + 1, 0, a.shape[0] - 1)
    x0 = np.clip(x0, 0, a.shape[1] - 1)
    y0 = np.clip(y0, 0, a.shape[0] - 1)
    fx = xs - np.floor(xs)
    fy = ys - np.floor(ys)
    return (
        a[y0, x0] * (1 - fx) * (1 - fy)
        + a[y0, x1] * fx * (1 - fy)
        + a[y1, x0] * (1 - fx) * fy
        + a[y1, x1] * fx * fy
    )


def boundary_rho(field: np.ndarray, cx: float, cy: float, rx: float, ry: float, rays: int = 720) -> np.ndarray:
    """ρ of the half-amplitude contour along `rays` rays, in units of the nominal ellipse.

    The half amplitude is taken against a LOCAL plateau just inside and a local
    zero just outside, so the wall's own shading cannot bias the crossing; the
    crossing itself is the LAST one before the field settles, which is what
    makes a dust speck inside the mask harmless. On a hard edge the estimator
    is unbiased by construction — it reads 1.000004 on the engine's own
    feather-0 α raster, whose edge is exact.
    """
    out = []
    for th in np.linspace(0, 2 * np.pi, rays + 1)[:-1]:
        ux, uy = math.cos(th), math.sin(th)
        rho = np.linspace(0.85, 1.15, 4001)
        v = bilinear(field, cx + rho * rx * ux - 0.5, cy + rho * ry * uy - 0.5)
        hi = np.median(v[(rho >= 0.94) & (rho <= 0.98)])
        lo = np.median(v[(rho >= 1.02) & (rho <= 1.06)])
        t = v - 0.5 * (hi + lo)
        idx = np.where(t > 0)[0]
        if len(idx) == 0 or idx[-1] + 1 >= len(t):
            continue
        k = idx[-1]
        out.append(rho[k] + t[k] * (rho[k + 1] - rho[k]) / (t[k] - t[k + 1]))
    return np.array(out)


#: Pixels a coverage bin must hold before its mean is reported. The innermost
#: rho bins of a radial mask are a few hundred pixels of the wall's dead
#: centre, where one blemish moves the recovered alpha by 0.2 — the same bins
#: `render::RADIAL_FALLOFF` holds at 1 outright "because those bins carry too
#: few pixels to measure".
BIN_FLOOR = 2000


def binned(cov: np.ndarray, values: list[np.ndarray], edges: np.ndarray, floor: int = BIN_FLOOR):
    """Bin-mean each of `values` against the coverage coordinate `cov`.

    Only bins with at least `floor` pixels are returned, so a bin the mask
    barely touches cannot contribute an average made of a handful of pixels.
    """
    keep = (cov >= edges[0]) & (cov < edges[-1])
    index = np.digitize(cov[keep], edges) - 1
    n = len(edges) - 1
    count = np.bincount(index, minlength=n)[:n]
    ok = count >= floor
    means = [np.bincount(index, weights=v[keep], minlength=n)[:n][ok] / count[ok] for v in values]
    centres = (0.5 * (edges[:-1] + edges[1:]))[ok]
    return centres, means


def half_crossing(x: np.ndarray, y: np.ndarray, rising: bool) -> float:
    """Where `y` crosses 0.5, by linear interpolation on the first crossing."""
    idx = np.where(y >= 0.5)[0] if rising else np.where(y < 0.5)[0]
    if len(idx) == 0 or idx[0] == 0:
        return float("nan")
    k = idx[0]
    return float(x[k - 1] + (0.5 - y[k - 1]) * (x[k] - x[k - 1]) / (y[k] - y[k - 1]))


def measure(pack: Pack, tone: np.ndarray) -> dict:
    """Every measurement the report quotes, per export code."""
    by_code = {e["code"]: e for e in pack.exports()}
    result: dict[str, dict] = {}
    for spec in pack.exports():
        code = spec["code"]
        ref_code = pack.reference_of(code)
        row: dict = {"group": spec["group"], "lens": spec["lens_profile_enable"]}
        corrections = spec["corrections"]
        active = [c for c in corrections if c["active"]]
        row["corrections"] = len(corrections)
        row["active"] = len(active)
        lr = pack.lr(code)
        ref = pack.lr(ref_code)
        row["lr_max_dn"] = int(np.abs(lr - ref).max())
        alpha_lr = tone[ref] - tone[lr]
        alpha_engine = pack.alpha(code)
        row["engine_alpha_max"] = float(alpha_engine.max())
        row["lr_alpha_max"] = float(np.percentile(alpha_lr, 99.99))
        if not active:
            result[code] = row
            continue
        corr = active[0]
        if corr["kind"] == "radial":
            cx, cy, xn, yn, angle = pack.radial(corr)
            rho = pack.rho(corr)
            feather = corr["args"][4]
            row["feather"] = feather
            if feather == 0 and angle == 0 and len(active) == 1:
                b_lr = boundary_rho(alpha_lr, cx * pack.width, cy * pack.height, xn * pack.width, yn * pack.height)
                b_en = boundary_rho(alpha_engine, cx * pack.width, cy * pack.height, xn * pack.width, yn * pack.height)
                row["lr_boundary_rho"] = float(np.median(b_lr))
                row["engine_boundary_rho"] = float(np.median(b_en))
                row["boundary_px"] = float((np.median(b_lr) - np.median(b_en)) * xn * pack.width)
            edges = np.arange(0, 1.6001, 0.005)
            centres, (ml, me) = binned(rho, [alpha_lr, alpha_engine], edges)
            if len(centres) > 10:
                d = ml - me
                row["alpha_rms"] = float(np.sqrt((d * d).mean()))
                row["alpha_max"] = float(np.abs(d).max())
                cl = half_crossing(centres, ml, rising=False)
                ce = half_crossing(centres, me, rising=False)
                row["rho50_lr"], row["rho50_engine"] = cl, ce
                major = max(xn * pack.width, yn * pack.height)
                row["contour_px"] = (cl - ce) * major if cl == cl and ce == ce else float("nan")
        elif corr["kind"] == "linear":
            t = pack.gradient_t(corr)
            zx, zy, fx, fy = corr["args"]
            span = math.hypot((fx - zx) * pack.width, (fy - zy) * pack.height)
            edges = np.linspace(-0.35, 1.35, 341)
            centres, (ml, me) = binned(t, [alpha_lr, alpha_engine], edges)
            plateau = ml[centres > 1.2].mean()
            d = ml - me
            row["alpha_rms"] = float(np.sqrt((d * d).mean()))
            row["alpha_max"] = float(np.abs(d).max())
            row["plateau"] = float(plateau)
            cl = half_crossing(centres, ml / plateau, rising=True)
            ce = half_crossing(centres, me, rising=True)
            row["t50_lr"], row["t50_engine"] = cl, ce
            row["contour_px"] = (cl - ce) * span
            clip = np.clip(centres, 0.0, 1.0)
            n = ml / plateau

            def hermite(x):
                return 3 * x**2 - 2 * x**3

            for name, model in (
                ("smoothstep", hermite(clip)),
                ("linear", clip),
                ("sin", 0.5 - 0.5 * np.cos(np.pi * clip)),
                ("warped", hermite(clip**SHIPPED_WARP)),
            ):
                row[f"fit_{name}"] = float(np.sqrt(((n - model) ** 2).mean()))
            # …and the exponent THIS gradient would have chosen. The spread of
            # `fit_warp_q` over the twelve is the constant's own error bar, and
            # a grid is honest here: the residual is not quadratic in q, and a
            # 5 x 10^-4 grid is two orders under the spread it has to resolve.
            grid = np.arange(0.90, 1.4001, 0.0005)
            row["fit_warp_q"] = float(
                grid[int(np.argmin([((n - hermite(clip**q)) ** 2).sum() for q in grid]))]
            )
        result[code] = row

    # Group-level LR-vs-LR comparisons: the two questions that need no engine.
    pairs = {}
    for f in (25, 50, 75):
        base = pack.lr(f"D-R+0-F{f}")
        for r in ("-100", "+100"):
            other = pack.lr(f"D-R{r}-F{f}")
            pairs[f"D-R{r}-F{f} vs D-R+0-F{f}"] = int(np.abs(other - base).max())
    for a, b in (("E-CLICK", "E-EYE-OFF"), ("E-BOTH", "E-EYE-OFF")):
        pairs[f"{a} vs {b}"] = int(np.abs(pack.lr(a) - pack.lr(b)).max())
    return {"exports": result, "lr_pairs": pairs}


# ---------------------------------------------------------------------------
# entry point
# ---------------------------------------------------------------------------


def default_pack() -> Path:
    env = os.environ.get("AUTOSHADE_LR_PACK")
    if env:
        return Path(env)
    return Path.home() / "autoshop-fixtures" / "me6-2026-09"


def default_probe() -> Path:
    env = os.environ.get("AUTOSHADE_DATA_DIR")
    if env:
        return Path(env) / "lr-probe"
    raise SystemExit("pass --probe, or set AUTOSHADE_DATA_DIR to the probe's data root")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pack", type=Path, default=None, help="the me6 fixture root")
    parser.add_argument("--probe", type=Path, default=None, help="<AUTOSHADE_DATA_DIR>/lr-probe")
    parser.add_argument("--json", type=Path, default=None, help="write the measurements as JSON")
    parser.add_argument(
        "--tone-smoothness",
        type=float,
        default=TONE_SMOOTHNESS,
        help="second-difference weight in the tone solve; re-run with another "
             "value to see how much of a fitted number is the tone model's",
    )
    args = parser.parse_args()
    pack = Pack(args.pack or default_pack(), args.probe or default_probe())
    print(f"pack  {pack.pack}\nprobe {pack.probe}\nframe {pack.width}x{pack.height}")
    cells = calibration_cells(pack)
    tone = fit_tone_coordinate(cells, args.tone_smoothness)
    print(f"tone coordinate: {len(cells)} cells, smoothness {args.tone_smoothness}, monotone "
          f"{bool(np.all(np.diff(tone[12:249]) > 0))}")
    out = measure(pack, tone)
    out["tone"] = tone.tolist()
    for code in sorted(out["exports"]):
        row = out["exports"][code]
        parts = [f"{code:<12}"]
        for key, fmt in (
            ("lr_boundary_rho", "{:.5f}"),
            ("engine_boundary_rho", "{:.5f}"),
            ("boundary_px", "{:+.2f}px"),
            ("alpha_rms", "rms {:.4f}"),
            ("alpha_max", "max {:.4f}"),
            ("rho50_lr", "ρ50lr {:.4f}"),
            ("rho50_engine", "ρ50en {:.4f}"),
            ("t50_lr", "t50lr {:.4f}"),
            ("t50_engine", "t50en {:.4f}"),
            ("contour_px", "Δ{:+.2f}px"),
            ("fit_smoothstep", "ss {:.4f}"),
            ("fit_warped", "warp {:.4f}"),
            ("fit_linear", "lin {:.4f}"),
            ("fit_warp_q", "q {:.4f}"),
        ):
            if key in row and row[key] == row[key]:
                parts.append(fmt.format(row[key]))
        parts.append(f"lrΔ {row['lr_max_dn']}DN")
        parts.append(f"engα {row['engine_alpha_max']:.3f}")
        print("  ".join(parts))
    print()
    for name, value in out["lr_pairs"].items():
        print(f"{name}: max|Δ| = {value} DN")
    if args.json:
        args.json.write_text(json.dumps(out, indent=1), encoding="utf-8")
        print(f"\nwrote {args.json}")


if __name__ == "__main__":
    main()
