//! Lightroom's measured mask falloffs: the linear gradient's profile and warp, and the radial α(ρ; feather) table.

/// The linear-gradient profile selected by the Lightroom falloff measurement.
///
/// `Measured` is shipped after the me6-2026-09 pack measured Lightroom's own
/// COVERAGE rather than its exported luma — see [`LINEAR_FALLOFF_WARP`].
pub(super) const LINEAR_FALLOFF: LinearFalloff = LinearFalloff::Measured;

#[allow(dead_code)] // Clamped and Eased remain pinned by the historical tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinearFalloff {
    /// The historical piecewise-linear ramp.
    Clamped,
    /// C1 Hermite smoothstep, with zero slope at both handles.
    Eased,
    /// [`LinearFalloff::Eased`] on a measured abscissa —
    /// `smoothstep(t^`[`LINEAR_FALLOFF_WARP`]`)`, the shipped law.
    Measured,
}

/// Lightroom's linear gradient is SKEWED: its half-coverage contour sits at
/// `t = 0.5436` of the handle axis, not at the midpoint. Measured on the
/// me6-2026-09 pack's twelve isolated gradients — three positions × two
/// orientations × lens correction on and off, handles 20 % of the short side
/// apart on a 6240 × 4160 frame. The three positions and the two lens states
/// agree to 0.0002; the two ORIENTATIONS do not, and that is the whole of the
/// residual: 0.5411 on the six vertical gradients, 0.5460 on the six
/// horizontal.
///
/// # Why the shipped smoothstep was symmetric, and why that was wrong
///
/// The `Eased` fit that chose it (`scripts/linear_falloff_probe.py --fit`)
/// scored candidate profiles against the exported row LUMA, normalised between
/// the two plateaus — which is `T(coverage)`, the tone curve composed with the
/// mask, not the mask. A monotone tone curve cannot turn a straight ramp into
/// an S with zero slope at both ends, so `Eased` beating `Clamped` survives
/// that confusion; the SHAPE does not. `scripts/lr_mask_parity.py` inverts the
/// tone curve first — it recovers Lightroom's own value-in-stops from the
/// pack's feather-0 masks and the wall's 4:1 brightness range — and the
/// coverage that comes out is not symmetric.
///
/// # The number
///
/// `scripts/lr_mask_parity.py` fits an exponent PER GRADIENT (`fit_warp_q`).
/// The twelve span 1.1210…1.1300, mean 1.1254, sd 0.0041, and the spread is
/// the orientation split again: 1.1215 on the vertical six, 1.1293 on the
/// horizontal six. The shipped `1.124` sits inside that range, 0.0014 (a third
/// of an sd) under the pooled mean; carrying the mean instead would move the
/// α = ½ contour by 0.2 px on the pack's 832 px handle span, which is a fifth
/// of the residual the best single exponent leaves anyway.
///
/// The tone model is not what decides it. Re-solving the tone coordinate with
/// the second-difference weight at 20 and at 120 instead of 50
/// (`--tone-smoothness`) moves the pooled exponent to 1.1253 and 1.1224 — a
/// 0.0030 band, narrower than the between-gradient sd and containing the
/// shipped value.
///
/// Scored against the measured coverage, pooled over all twelve gradients:
/// rms(α) 0.0064 for this law, 0.0315 for the unwarped smoothstep, 0.0323 for
/// a raised cosine and 0.0598 for the linear ramp. End to end — the engine's
/// own α field against Lightroom's, on the rendered pixels — the pooled
/// residual falls from 0.0293 to 0.0074, and the α = ½ contour from
/// +34.2 / +38.2 px to +0.9 / +5.0 px (vertical / horizontal). What is left is
/// the orientation split, and one exponent cannot be inside both halves of it.
///
/// ⚠ RENDER-BEHAVIOUR CHANGE: every LINEAR mask renders differently from this
/// version on. The coverage moves by at most 0.0624 in α (at `t = 0.4606`),
/// and the half-coverage contour moves 3.97 % of the handle span toward the
/// Full end. Radial, bitmap, brush and AI masks are byte-identical.
const LINEAR_FALLOFF_WARP: f32 = 1.124;

/// Reshape the existing handle-axis parameter without changing handle
/// transport, coordinate frames, or geometry metrics.
pub(super) fn linear_coverage(t: f32, profile: LinearFalloff) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match profile {
        LinearFalloff::Clamped => t,
        LinearFalloff::Eased => t * t * (3.0 - 2.0 * t),
        LinearFalloff::Measured => {
            // `powf` at the two ends is exact — `0^q = 0`, `1^q = 1` — so the
            // handles stay where the sidecar put them and the plateaus stay
            // flat, which is what makes this a reshape and not a transport
            // change.
            let u = t.powf(LINEAR_FALLOFF_WARP);
            u * u * (3.0 - 2.0 * u)
        }
    }
}

/// A [`crate::recipe::MaskGeometry::Shaped`] gradient's coverage at handle
/// parameter `t`: `falloff` sampled at evenly spaced `t` in 0..=1, linearly
/// interpolated and clamped to 0..=1. An empty table is Lightroom's own
/// law ([`linear_coverage`] at [`LINEAR_FALLOFF`]); a one-entry table is
/// that constant. NaN `t` reads as the Zero end, as a NaN handle parameter
/// would otherwise index the table with a NaN and render a NaN weight.
pub(super) fn shaped_coverage(t: f32, falloff: &[f32]) -> f32 {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    match falloff.len() {
        0 => linear_coverage(t, LINEAR_FALLOFF),
        1 => falloff[0].clamp(0.0, 1.0),
        n => {
            let x = t * (n - 1) as f32;
            let k = (x as usize).min(n - 2);
            let u = x - k as f32;
            (falloff[k] * (1.0 - u) + falloff[k + 1] * u).clamp(0.0, 1.0)
        }
    }
}

/// Lightroom's radial-mask falloff α(ρ), READ OUT OF THE MEASUREMENT.
///
/// `feather` is the recipe's 0..1 fraction, `d` the normalised elliptical
/// radius (1.0 = on the ellipse). Returns coverage BEFORE `flipped` flips it.
///
/// # Why a table and not a law
///
/// Three successive closed forms were wrong on this arm, and R29 Batch-7 plus
/// its supplement Batch-7-2 (`b7-analysis.md` and `b7-analysis-2.md` in the R29
/// materials ledger, outside the tree) closed the question rather than
/// proposing a fourth:
/// across all EIGHT rungs those batches measured, no two-parameter closed form
/// reaches the 0.003 measurement floor. The best is a Beta CDF in `1 − ρ/1.4335`
/// at 3.1× the floor; the free-endpoint smoothstep this engine shipped scores
/// 4.5×, `exp(−(ρ/s)^k)` 4.0×, a logistic 9.5× (B7-2 §4). The one candidate law
/// the four-rung batch had spotted, `a ≈ 1.9/f`, is refuted outright by the
/// supplement — it holds on f ∈ [25, 100] and misses by 58× at f = 1. So the
/// adjudicated landing shape is the measured α(ρ) itself.
///
/// # What it replaces, and by how much
///
/// `1 − smoothstep(1 − f, 1 + f/2, d)`. Scored on the batch's own grid (Δρ =
/// 0.005 bins of ≥400 px, ρ ≤ 1.45, green channel) that law reads rms(α)
/// 0.0093 / 0.0104 / 0.0285 / 0.0929 / 0.0974 / 0.1197 / 0.1403 / 0.1557 at
/// feather 1 / 5 / 10 / 25 / 50 / 75 / 90 / 100, and renders the α ≥ 0.5 region
/// 1.105× / 1.247× / 1.387× / 1.690× / 2.077× too large from f = 25 up. This
/// table reads 0.0009 / 0.0005 / 0.0005 / 0.0005 / 0.0004 / 0.0001 / 0.0000 /
/// 0.0000, and its α = 0.5 contour lands on the RAW measurement's to four
/// decimals in ρ on every rung, so the same area ratio is 1.000 across the
/// board — measured against the unconditioned profile, not against the table's
/// own conditioned copy. The three columns me3 inserted later (f = 15/35/65)
/// score the same way: the old law reads rms(α) 0.0567 / 0.1124 / 0.1073 there,
/// with the α ≥ 0.5 region 1.043× / 1.188× / 1.307× too large, and the table
/// carrying those columns reads 0.0000 (`a_08`).
///
/// Better on EVERY rung is the requirement, not a bonus: the old law was
/// already CORRECT for f ≤ 5 (rms 0.009–0.010, area ratio 0.995–0.997, B7-2
/// §6) and a replacement that only fixed the wide end would have broken the one
/// segment that worked.
///
/// # The table
///
/// Rows are the measurement's OWN ρ bins — centres `0.0025 + 0.005 i` — so the
/// eleven columns are reproduced exactly where they were measured rather than
/// resampled onto a rounder grid. Columns are Lightroom's feather 1 / 5 / 10 /
/// 15 / 25 / 35 / 50 / 65 / 75 / 90 / 100. Sources: `dense9.npz` from
/// `b7b_12_dense.py`, tabulated in `b7-analysis-2.md` §3.1, for eight of them
/// (f = 25/50/75/100 reproduce B7 §3.1 bit for bit; f = 1/5/10/90 are that
/// supplement's rungs), and the me3 package's own f = 15/35/65 exports for the
/// other three (`me3-a-report.md` §0-Q1 and §1 in the R29 materials ledger,
/// generator `scripts-archive/me3-a/a_09_table.py`).
///
/// The three me3 columns are INSERTED, not refitted. The eight B7-2 columns
/// come across bit for bit — max |Δ| = 0.000000 over all 2320 of their entries
/// (`a_09`) — and the insertion lands because BETWEEN columns is where the
/// table was still wrong: scored against the new exports the eight-column
/// version read rms(α) 0.0209 (max 0.0799) at f = 15 and 0.0212 (max 0.0490) at
/// f = 35, which is its α = 0.5 contour sitting 14.8 px and 24.9 px outside
/// Lightroom's on the measured frame's major axis (`a_07` §4). f = 65 was
/// already right at rms 0.0004 (0.2 px) and comes along only because the export
/// existed. Held-out over the whole ladder — drop each rung, predict it from
/// its neighbours — the mean rms goes 0.0268 → 0.0140 (`a_07` §2).
///
/// Two conditionings, both small enough to name outright:
///
/// * α is regressed non-increasing in ρ (pool-adjacent-violators, weighted by
///   bin count) and clamped to [0, 1]. Cost ≤ 0.0073 anywhere (≤ 0.0061 on the
///   eight B7-2 columns, 0.0073 / 0.0072 / 0.0060 on the inserted f = 15/35/65,
///   `a_09`), rms ≤ 0.0009 — the raw wiggle is 8-bit quantisation, 1 DN ≈ 0.004
///   in α.
/// * α is forced non-increasing in f for ρ ≤ 1 (running minimum across the
///   columns). Cost ≤ 0.000061, on 17 entries — all of them in the eight
///   columns that carried over unchanged; `a_09` reports the running-minimum
///   cost on the inserted f = 15/35 as 0.000000 and does not print it
///   separately for f = 65. OUTSIDE the ellipse the order genuinely reverses —
///   more feather reaches further — so the running minimum stops at ρ = 1, and
///   the f = 50 tail really is fatter than f = 75's and f = 100's out there
///   (B7 §3.1, independently reproduced in the raw DN profile).
///
/// `α(0) = 1` at every feather is measured, not fitted or normalised in: mask
/// centres are pixel-identical to the feather-0 frame on all eight rungs B7-2
/// measured (§3.4), and the me3 rungs land on the same near-centre rows. That is the fact that killed the free-endpoint refit, which wanted
/// `d_in = −0.228` and a 6 %-wrong centre at f = 100. Rows inside ρ = 0.0425
/// hold that 1 outright — those bins carry too few pixels to measure and the
/// disc they cover is 0.18 % of the ellipse.
///
/// # Feather 0 is ANALYTIC, not measured
///
/// The measured f = 0 column has a transition 0.0084 wide in ρ, but that is the
/// JPEG-plus-capture-sharpening blur floor (8.7 px on this frame's major axis),
/// not Lightroom's edge — at Feather 0 Lightroom draws a hard edge. So f = 0 is
/// a hard step here, exactly, with `d == 1.0` counting as OUTSIDE: the
/// behaviour the old degenerate-`ramp` guard produced and the one
/// `radial_feather_zero_stays_finite_on_the_boundary` pins.
///
/// # Interpolation
///
/// Linear in ρ between rows, linear in f between columns. Exact on all eleven
/// columns, and a convex combination throughout — so α stays inside [0, 1] and
/// stays monotone on both axes by construction, not by assertion.
///
/// Linear in f is the abscissa the DATA picks, not a default. The measured
/// transition width `W(f) = ρ(α=.05) − ρ(α=.95)` (B7-2 §3.2) spreads only 1.97×
/// as `W/f` across the whole ladder, against 7.5× as `W/√f` and 6.0× as
/// `W/log f`. A held-out check — drop a column, predict it from its neighbours
/// — puts linear-in-f at mean rms 0.027 against 0.018 for the best curved rival
/// (PCHIP in log f), the sign of the difference flipping rung to rung, and
/// log f is undefined at the f = 0 end this function has to reach anyway. A
/// 1.5× edge with mixed sign does not buy curvature that nothing measured.
///
/// me3 then settled it on rungs nothing had fitted, f = 15/35/65 (`a_08`):
/// linear-in-f scores mean rms 0.0141 there, PCHIP-in-log-f 0.0092, cubic in
/// log f 0.0093, Akima 0.0105, plain PCHIP 0.0113 — and INSERTING those three
/// measured columns scores 0.0000. Changing the family buys at most 1.5×;
/// carrying the measurement buys the whole residual, so the family stays and
/// the columns land.
///
/// The residual is what it is, and this is it: ON a column the table is within 0.0009 of
/// the measurement; BETWEEN two columns it is still unmeasured. The two widest
/// gaps of the eight-column version were probed and closed rather than
/// estimated (f = 15 and f = 35, above), which leaves the f ≤ 10 end as the
/// coarsest remaining seam — dropping f = 5 and predicting it from f = 1/10
/// costs rms 0.0253 (`a_07` §2). That seam stays open by decision: no export sits inside
/// that gap, and its transition is narrow enough (W ≤ 0.17 in ρ) that the same
/// α error is a far smaller contour displacement than at f = 15/35
/// (`me3-a-report.md` §4).
///
/// # Carried as measurement, not as a formula
///
/// * `d_out` deliberately does NOT land as a constant — it is baked into the
///   column tails, so the value below costs zero pixels either way. It is
///   **√2**, and me3 excluded `1.43` and B7's `1.4335` outright (`me3-a-report`
///   §0-Q2). Four SHAPE-FREE instruments agree: the sector-block correction
///   endpoint reads 1.41480 ± 0.00046 on the major axis across all eleven rungs
///   (`a_19`); a sign census puts the excess darkening below 2σ from ρ = 1.4160
///   and pure dither (exactly 0.500) over [1.418, 1.424) (`a_18`); the strict
///   all-darkened block bound is 1.41367 (`a_16`); and the last ring-mean band
///   significantly darker than nomask ends at 1.4142 (`a_10`). Forward check:
///   √2 predicts sector endpoints 1.4219 major / 1.4335 minor against measured
///   1.4231 / 1.4386, while 1.43 predicts 1.4377 and 1.4335 predicts 1.4412 —
///   both PAST what the pixels show. B7's ±0.002 was measuring JPEG 8×8 block
///   spill rather than mask support (twelve mod-8 alignment tests, p ≤ 1e−23;
///   B7-2 §3.3), and B7-2's own `1.43 ± 0.015` was the honest width of that
///   contaminated estimate. Residual systematic ±0.001, declared rather than
///   polished away.
/// * The f = 1 FAR TAIL is UNRESOLVED (B7-2 §8-2): its darkening is significant
///   out to ρ ≈ 1.25 and indistinguishable from zero past that, decaying too
///   slowly to separate "same support, small amplitude" from "smaller support".
///   The column carries what was measured and rounds to zero where the 8-bit
///   floor did.
/// * ASPECT INVARIANCE is SAMPLED, once: the shipped table scored against a
///   held-out aspect 1.2 export reads rms(α) 0.0009, max |dev| 0.0031, and its
///   α = 0.5 contour lands 0.04 px from the measurement (against 0.0004 on the
///   fitted aspect 2.5) — and the best single radial rescale between the two
///   geometries is k = 1.00076, so no part of the falloff is anchored in pixels
///   (`me3-b-report.md` H1/A1-A2). Scope, kept rather than generalised: ONE
///   extra aspect ratio, at f = 50 only, still centred. Every other rung is
///   still one geometry (aspect 2.5, centred, Angle 0).
/// * Every column carries the same residual measurement blur that makes f = 0's
///   own column 0.0084 wide, so the narrow rungs are marginally softer here
///   than Lightroom's truth. Deconvolving it would be inventing a kernel.
/// * `roundness` still does not enter — a measured no-op at +100 with feather
///   both 0 and 50 (B7-2 §5). See `MaskGeometry::Radial` in `mask_weight`.
pub(super) fn radial_falloff(feather: f32, d: f32) -> f32 {
    // Lightroom's own 0..100 feather units — the axis the columns sit on. The
    // recipe carries the same number as a 0..1 fraction; `xmp.rs` converts on
    // the boundary in both directions.
    //
    // NaN is spelled out rather than left to `clamp`, which PROPAGATES it: a
    // NaN feather would otherwise pass `f <= 0.0`, survive the `d` guard on any
    // finite sample point, and come back out as a NaN weight — which survives
    // `wgt <= 0.001` and casts to black. It degrades to the hard edge, the same
    // stance `brush_kernel_exponents` takes for a NaN hardness, and a
    // hand-edited `recipe.json` is the only way to produce one.
    let f = if feather.is_nan() { 0.0 } else { feather.clamp(0.0, 1.0) * 100.0 };
    if f <= 0.0 {
        // The analytic hard edge (see above).
        return if d < 1.0 { 1.0 } else { 0.0 };
    }
    // Past the last row every column is already 0, so this is the table's
    // extent and NOT a claim about `d_out`. The `is_finite` half is not
    // decoration: a NaN `d` would otherwise index row 0 (NaN casts to 0) and
    // blend with a NaN weight, and a NaN mask weight survives the `wgt <= 0.001`
    // early-out and casts to black — the same trap `brush_kernel_at` guards and
    // the one the old degenerate-`ramp` comment described.
    let last = RADIAL_FALLOFF.len() - 1;
    if !d.is_finite() || d >= RADIAL_FALLOFF_RHO0 + RADIAL_FALLOFF_DRHO * last as f32 {
        return 0.0;
    }
    let x = ((d - RADIAL_FALLOFF_RHO0) / RADIAL_FALLOFF_DRHO).max(0.0);
    let k = (x as usize).min(last - 1);
    let u = x - k as f32;
    // One column, interpolated in ρ. Row 0 is all 1, so clamping `x` at 0 IS
    // α(0) = 1 and needs no second branch.
    let col = |j: usize| RADIAL_FALLOFF[k][j] * (1.0 - u) + RADIAL_FALLOFF[k + 1][j] * u;
    let hi = RADIAL_FALLOFF_F
        .iter()
        .position(|&c| f <= c)
        .unwrap_or(RADIAL_FALLOFF_F.len() - 1);
    let (a_lo, f_lo) = if hi == 0 {
        // 0 < f < 1: the gap between Lightroom's hard edge and its first
        // feathered rung, which nothing sampled — Lightroom only ever writes
        // whole feather units, but this engine's own slider is continuous. The
        // lower end is the hard edge read ON THE SAME GRID, which keeps the
        // family continuous in `d` for every f > 0; f == 0 itself is still the
        // exact step above, and the two differ only across one row (0.005 in ρ,
        // ~5 px on the measured frame's major axis).
        let step = |i: usize| {
            if RADIAL_FALLOFF_RHO0 + RADIAL_FALLOFF_DRHO * i as f32 <= 1.0 { 1.0 } else { 0.0 }
        };
        (step(k) * (1.0 - u) + step(k + 1) * u, 0.0)
    } else {
        (col(hi - 1), RADIAL_FALLOFF_F[hi - 1])
    };
    let t = (f - f_lo) / (RADIAL_FALLOFF_F[hi] - f_lo);
    a_lo * (1.0 - t) + col(hi) * t
}

/// [`RADIAL_FALLOFF`]'s feather columns, in Lightroom's own 0..100 units.
pub(super) const RADIAL_FALLOFF_F: [f32; 11] = [1.0, 5.0, 10.0, 15.0, 25.0, 35.0, 50.0, 65.0, 75.0, 90.0, 100.0];

/// ρ of [`RADIAL_FALLOFF`]'s first row and the row spacing — the measurement's
/// own bin centres (`b7b_12_dense.py` bins ρ at 0.005 from 0).
const RADIAL_FALLOFF_RHO0: f32 = 0.0025;
const RADIAL_FALLOFF_DRHO: f32 = 0.005;

/// The measured α(ρ; feather) itself: rows are ρ = `0.0025 + 0.005 i`, columns
/// are [`RADIAL_FALLOFF_F`]. Provenance and conditioning: [`radial_falloff`].
///
/// `approx_constant` is silenced because three of these 3190 measurements land
/// on a mathematical constant by coincidence: 0.4342 at (ρ = 0.7425, f = 75)
/// and 0.4343 at (ρ = 0.8225, f = 50) near `LOG10_E`, 0.3010 at (ρ = 0.7975,
/// f = 90) near `LOG10_2` — the three the lint reports with the `allow` lifted,
/// all of them in columns me3's insertion did not touch. They are photographs
/// of a mask edge, not logarithms, and substituting the constant would corrupt
/// the data the lint is pointing at.
#[rustfmt::skip]
#[allow(clippy::approx_constant)]
pub(super) const RADIAL_FALLOFF: [[f32; 11]; 290] = [
    // rho = 0.0025
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9970, 0.9955],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9986, 0.9951, 0.9915, 0.9883],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9956, 0.9926, 0.9861, 0.9815],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9944, 0.9882, 0.9801, 0.9743],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9899, 0.9833, 0.9726, 0.9656],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9884, 0.9781, 0.9656, 0.9574],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9884, 0.9781, 0.9629, 0.9537],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9884, 0.9781, 0.9629, 0.9529],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9859, 0.9741, 0.9562, 0.9452],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9822, 0.9701, 0.9513, 0.9387],
    // rho = 0.1025
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9811, 0.9670, 0.9469, 0.9335],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9794, 0.9648, 0.9430, 0.9284],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9771, 0.9614, 0.9383, 0.9226],
    [1.0000, 0.9996, 0.9996, 0.9996, 0.9996, 0.9995, 0.9982, 0.9736, 0.9574, 0.9327, 0.9162],
    [1.0000, 0.9993, 0.9993, 0.9993, 0.9993, 0.9993, 0.9981, 0.9718, 0.9546, 0.9284, 0.9109],
    [1.0000, 0.9991, 0.9991, 0.9991, 0.9991, 0.9990, 0.9972, 0.9695, 0.9514, 0.9238, 0.9056],
    [1.0000, 0.9991, 0.9991, 0.9991, 0.9990, 0.9990, 0.9969, 0.9677, 0.9486, 0.9195, 0.9003],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9988, 0.9967, 0.9664, 0.9460, 0.9155, 0.8957],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9986, 0.9962, 0.9643, 0.9433, 0.9111, 0.8902],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9960, 0.9624, 0.9401, 0.9067, 0.8847],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9953, 0.9604, 0.9371, 0.9023, 0.8794],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9953, 0.9587, 0.9348, 0.8987, 0.8747],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9950, 0.9570, 0.9318, 0.8944, 0.8693],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9947, 0.9555, 0.9295, 0.8901, 0.8644],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9944, 0.9538, 0.9264, 0.8858, 0.8590],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9944, 0.9520, 0.9239, 0.8822, 0.8545],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9939, 0.9505, 0.9213, 0.8778, 0.8491],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9982, 0.9933, 0.9480, 0.9180, 0.8733, 0.8442],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9982, 0.9930, 0.9463, 0.9154, 0.8693, 0.8386],
    [1.0000, 0.9988, 0.9988, 0.9988, 0.9988, 0.9979, 0.9925, 0.9441, 0.9123, 0.8648, 0.8336],
    // rho = 0.2025
    [1.0000, 0.9988, 0.9987, 0.9986, 0.9986, 0.9976, 0.9915, 0.9420, 0.9092, 0.8604, 0.8278],
    [1.0000, 0.9988, 0.9987, 0.9986, 0.9986, 0.9976, 0.9913, 0.9404, 0.9066, 0.8561, 0.8229],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9985, 0.9970, 0.9901, 0.9379, 0.9030, 0.8516, 0.8173],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9985, 0.9970, 0.9900, 0.9362, 0.9006, 0.8476, 0.8124],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9984, 0.9968, 0.9893, 0.9340, 0.8976, 0.8431, 0.8070],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9984, 0.9966, 0.9887, 0.9319, 0.8944, 0.8386, 0.8016],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9984, 0.9966, 0.9879, 0.9300, 0.8915, 0.8344, 0.7966],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9983, 0.9960, 0.9872, 0.9277, 0.8885, 0.8297, 0.7912],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9983, 0.9960, 0.9868, 0.9261, 0.8855, 0.8257, 0.7861],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9983, 0.9955, 0.9863, 0.9237, 0.8825, 0.8210, 0.7805],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9953, 0.9854, 0.9216, 0.8794, 0.8167, 0.7753],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9950, 0.9846, 0.9195, 0.8765, 0.8122, 0.7702],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9949, 0.9839, 0.9176, 0.8736, 0.8082, 0.7649],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9947, 0.9834, 0.9155, 0.8706, 0.8040, 0.7599],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9945, 0.9823, 0.9131, 0.8673, 0.7994, 0.7546],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9939, 0.9811, 0.9110, 0.8643, 0.7950, 0.7492],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9934, 0.9807, 0.9087, 0.8610, 0.7905, 0.7439],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9980, 0.9930, 0.9796, 0.9063, 0.8578, 0.7860, 0.7386],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9979, 0.9925, 0.9785, 0.9038, 0.8548, 0.7815, 0.7333],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9979, 0.9922, 0.9777, 0.9017, 0.8516, 0.7773, 0.7282],
    // rho = 0.3025
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9975, 0.9913, 0.9762, 0.8992, 0.8482, 0.7725, 0.7226],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9975, 0.9910, 0.9755, 0.8969, 0.8450, 0.7682, 0.7177],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9972, 0.9901, 0.9740, 0.8941, 0.8414, 0.7634, 0.7119],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9896, 0.9727, 0.8918, 0.8381, 0.7588, 0.7065],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9889, 0.9716, 0.8893, 0.8351, 0.7546, 0.7015],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9884, 0.9706, 0.8871, 0.8321, 0.7504, 0.6963],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9879, 0.9690, 0.8844, 0.8285, 0.7456, 0.6909],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9968, 0.9871, 0.9678, 0.8820, 0.8252, 0.7411, 0.6858],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9968, 0.9867, 0.9668, 0.8797, 0.8222, 0.7371, 0.6810],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9968, 0.9859, 0.9655, 0.8770, 0.8189, 0.7325, 0.6756],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9967, 0.9851, 0.9641, 0.8746, 0.8156, 0.7281, 0.6705],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9961, 0.9840, 0.9624, 0.8717, 0.8118, 0.7234, 0.6650],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9961, 0.9831, 0.9609, 0.8691, 0.8086, 0.7190, 0.6598],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9956, 0.9821, 0.9591, 0.8662, 0.8051, 0.7143, 0.6547],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9956, 0.9811, 0.9577, 0.8634, 0.8016, 0.7099, 0.6495],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9954, 0.9803, 0.9559, 0.8608, 0.7983, 0.7055, 0.6443],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9950, 0.9790, 0.9540, 0.8581, 0.7948, 0.7010, 0.6392],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9947, 0.9779, 0.9523, 0.8552, 0.7911, 0.6965, 0.6340],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9945, 0.9767, 0.9506, 0.8523, 0.7876, 0.6919, 0.6290],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9942, 0.9757, 0.9487, 0.8496, 0.7844, 0.6878, 0.6240],
    // rho = 0.4025
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9940, 0.9742, 0.9466, 0.8465, 0.7807, 0.6832, 0.6188],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9935, 0.9730, 0.9444, 0.8435, 0.7769, 0.6786, 0.6138],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9930, 0.9715, 0.9423, 0.8404, 0.7733, 0.6740, 0.6087],
    [1.0000, 0.9988, 0.9986, 0.9980, 0.9924, 0.9698, 0.9395, 0.8371, 0.7693, 0.6693, 0.6033],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9918, 0.9680, 0.9372, 0.8337, 0.7655, 0.6645, 0.5982],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9912, 0.9663, 0.9349, 0.8304, 0.7616, 0.6600, 0.5930],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9908, 0.9649, 0.9324, 0.8271, 0.7578, 0.6552, 0.5877],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9905, 0.9633, 0.9302, 0.8242, 0.7543, 0.6511, 0.5831],
    [1.0000, 0.9988, 0.9985, 0.9978, 0.9893, 0.9611, 0.9272, 0.8207, 0.7503, 0.6464, 0.5779],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9885, 0.9588, 0.9242, 0.8167, 0.7461, 0.6415, 0.5725],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9880, 0.9569, 0.9215, 0.8135, 0.7423, 0.6369, 0.5677],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9874, 0.9551, 0.9188, 0.8098, 0.7383, 0.6322, 0.5626],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9865, 0.9530, 0.9156, 0.8064, 0.7342, 0.6278, 0.5575],
    [1.0000, 0.9988, 0.9982, 0.9971, 0.9856, 0.9504, 0.9126, 0.8027, 0.7302, 0.6231, 0.5525],
    [1.0000, 0.9988, 0.9982, 0.9970, 0.9848, 0.9483, 0.9093, 0.7988, 0.7260, 0.6184, 0.5474],
    [1.0000, 0.9988, 0.9981, 0.9968, 0.9836, 0.9457, 0.9061, 0.7950, 0.7219, 0.6139, 0.5425],
    [1.0000, 0.9988, 0.9981, 0.9967, 0.9826, 0.9431, 0.9026, 0.7911, 0.7176, 0.6091, 0.5375],
    [1.0000, 0.9988, 0.9981, 0.9967, 0.9819, 0.9408, 0.8992, 0.7873, 0.7137, 0.6047, 0.5327],
    [1.0000, 0.9988, 0.9979, 0.9963, 0.9804, 0.9378, 0.8954, 0.7831, 0.7092, 0.5997, 0.5276],
    [1.0000, 0.9988, 0.9979, 0.9963, 0.9794, 0.9353, 0.8919, 0.7793, 0.7048, 0.5952, 0.5226],
    // rho = 0.5025
    [1.0000, 0.9988, 0.9979, 0.9960, 0.9781, 0.9322, 0.8879, 0.7749, 0.7005, 0.5903, 0.5177],
    [1.0000, 0.9988, 0.9977, 0.9958, 0.9768, 0.9292, 0.8838, 0.7705, 0.6958, 0.5856, 0.5127],
    [1.0000, 0.9988, 0.9977, 0.9956, 0.9754, 0.9262, 0.8798, 0.7663, 0.6915, 0.5807, 0.5077],
    [1.0000, 0.9988, 0.9977, 0.9956, 0.9740, 0.9230, 0.8757, 0.7619, 0.6869, 0.5762, 0.5028],
    [1.0000, 0.9988, 0.9977, 0.9954, 0.9727, 0.9198, 0.8716, 0.7575, 0.6824, 0.5715, 0.4980],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9709, 0.9164, 0.8670, 0.7529, 0.6779, 0.5667, 0.4932],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9697, 0.9130, 0.8627, 0.7485, 0.6733, 0.5622, 0.4885],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9682, 0.9096, 0.8582, 0.7440, 0.6690, 0.5577, 0.4839],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9663, 0.9060, 0.8535, 0.7393, 0.6642, 0.5530, 0.4791],
    [1.0000, 0.9988, 0.9977, 0.9948, 0.9647, 0.9020, 0.8485, 0.7342, 0.6592, 0.5480, 0.4741],
    [1.0000, 0.9988, 0.9971, 0.9936, 0.9619, 0.8975, 0.8430, 0.7289, 0.6540, 0.5428, 0.4690],
    [1.0000, 0.9988, 0.9971, 0.9935, 0.9599, 0.8935, 0.8379, 0.7239, 0.6488, 0.5379, 0.4642],
    [1.0000, 0.9988, 0.9971, 0.9934, 0.9579, 0.8893, 0.8326, 0.7188, 0.6441, 0.5330, 0.4595],
    [1.0000, 0.9988, 0.9970, 0.9929, 0.9556, 0.8848, 0.8271, 0.7135, 0.6389, 0.5281, 0.4546],
    [1.0000, 0.9988, 0.9970, 0.9928, 0.9535, 0.8805, 0.8216, 0.7083, 0.6339, 0.5234, 0.4500],
    [1.0000, 0.9988, 0.9970, 0.9923, 0.9512, 0.8761, 0.8162, 0.7030, 0.6287, 0.5185, 0.4453],
    [1.0000, 0.9988, 0.9970, 0.9923, 0.9490, 0.8718, 0.8106, 0.6979, 0.6238, 0.5137, 0.4407],
    [1.0000, 0.9988, 0.9967, 0.9916, 0.9458, 0.8667, 0.8045, 0.6922, 0.6183, 0.5086, 0.4357],
    [1.0000, 0.9988, 0.9965, 0.9911, 0.9431, 0.8618, 0.7984, 0.6865, 0.6130, 0.5036, 0.4312],
    [1.0000, 0.9988, 0.9965, 0.9907, 0.9405, 0.8570, 0.7924, 0.6811, 0.6078, 0.4987, 0.4265],
    // rho = 0.6025
    [1.0000, 0.9988, 0.9965, 0.9903, 0.9375, 0.8520, 0.7863, 0.6752, 0.6023, 0.4939, 0.4219],
    [1.0000, 0.9988, 0.9962, 0.9896, 0.9345, 0.8467, 0.7798, 0.6694, 0.5968, 0.4888, 0.4173],
    [1.0000, 0.9988, 0.9960, 0.9891, 0.9312, 0.8411, 0.7731, 0.6634, 0.5912, 0.4836, 0.4125],
    [1.0000, 0.9988, 0.9960, 0.9885, 0.9281, 0.8358, 0.7668, 0.6575, 0.5858, 0.4789, 0.4082],
    [1.0000, 0.9988, 0.9960, 0.9881, 0.9248, 0.8301, 0.7598, 0.6515, 0.5802, 0.4739, 0.4036],
    [1.0000, 0.9988, 0.9958, 0.9872, 0.9211, 0.8244, 0.7529, 0.6453, 0.5745, 0.4688, 0.3991],
    [1.0000, 0.9988, 0.9956, 0.9864, 0.9173, 0.8185, 0.7459, 0.6390, 0.5686, 0.4638, 0.3945],
    [1.0000, 0.9988, 0.9955, 0.9857, 0.9135, 0.8125, 0.7387, 0.6328, 0.5629, 0.4587, 0.3901],
    [1.0000, 0.9988, 0.9955, 0.9853, 0.9096, 0.8065, 0.7317, 0.6265, 0.5572, 0.4538, 0.3857],
    [1.0000, 0.9988, 0.9955, 0.9844, 0.9056, 0.8003, 0.7243, 0.6201, 0.5513, 0.4487, 0.3811],
    [1.0000, 0.9987, 0.9952, 0.9833, 0.9016, 0.7939, 0.7169, 0.6136, 0.5455, 0.4436, 0.3767],
    [1.0000, 0.9987, 0.9950, 0.9825, 0.8971, 0.7876, 0.7094, 0.6071, 0.5395, 0.4386, 0.3723],
    [1.0000, 0.9986, 0.9948, 0.9813, 0.8927, 0.7809, 0.7017, 0.6005, 0.5334, 0.4336, 0.3681],
    [1.0000, 0.9984, 0.9943, 0.9801, 0.8878, 0.7740, 0.6939, 0.5936, 0.5273, 0.4285, 0.3634],
    [1.0000, 0.9984, 0.9943, 0.9789, 0.8830, 0.7672, 0.6861, 0.5869, 0.5212, 0.4233, 0.3590],
    [1.0000, 0.9984, 0.9940, 0.9778, 0.8782, 0.7603, 0.6782, 0.5802, 0.5153, 0.4184, 0.3549],
    [1.0000, 0.9984, 0.9938, 0.9766, 0.8730, 0.7533, 0.6703, 0.5733, 0.5090, 0.4133, 0.3505],
    [1.0000, 0.9984, 0.9935, 0.9750, 0.8676, 0.7462, 0.6623, 0.5663, 0.5029, 0.4083, 0.3463],
    [1.0000, 0.9984, 0.9935, 0.9738, 0.8625, 0.7390, 0.6542, 0.5596, 0.4967, 0.4033, 0.3421],
    [1.0000, 0.9984, 0.9933, 0.9722, 0.8569, 0.7317, 0.6460, 0.5525, 0.4905, 0.3984, 0.3378],
    // rho = 0.7025
    [1.0000, 0.9984, 0.9930, 0.9706, 0.8512, 0.7242, 0.6377, 0.5455, 0.4842, 0.3933, 0.3335],
    [1.0000, 0.9984, 0.9929, 0.9690, 0.8455, 0.7168, 0.6295, 0.5384, 0.4781, 0.3884, 0.3295],
    [1.0000, 0.9984, 0.9927, 0.9671, 0.8393, 0.7092, 0.6213, 0.5315, 0.4720, 0.3835, 0.3255],
    [1.0000, 0.9984, 0.9924, 0.9652, 0.8332, 0.7014, 0.6128, 0.5244, 0.4658, 0.3785, 0.3212],
    [1.0000, 0.9984, 0.9921, 0.9630, 0.8268, 0.6937, 0.6044, 0.5173, 0.4593, 0.3735, 0.3171],
    [1.0000, 0.9984, 0.9920, 0.9611, 0.8203, 0.6856, 0.5960, 0.5101, 0.4530, 0.3684, 0.3130],
    [1.0000, 0.9984, 0.9920, 0.9590, 0.8138, 0.6780, 0.5877, 0.5030, 0.4470, 0.3639, 0.3091],
    [1.0000, 0.9983, 0.9912, 0.9560, 0.8067, 0.6697, 0.5790, 0.4956, 0.4404, 0.3587, 0.3048],
    [1.0000, 0.9983, 0.9910, 0.9534, 0.7996, 0.6615, 0.5705, 0.4886, 0.4342, 0.3538, 0.3008],
    [1.0000, 0.9983, 0.9907, 0.9504, 0.7922, 0.6531, 0.5616, 0.4811, 0.4276, 0.3488, 0.2968],
    [1.0000, 0.9983, 0.9901, 0.9473, 0.7848, 0.6449, 0.5533, 0.4741, 0.4217, 0.3440, 0.2929],
    [1.0000, 0.9983, 0.9895, 0.9441, 0.7771, 0.6364, 0.5446, 0.4667, 0.4152, 0.3391, 0.2888],
    [1.0000, 0.9983, 0.9892, 0.9408, 0.7692, 0.6280, 0.5360, 0.4597, 0.4092, 0.3344, 0.2851],
    [1.0000, 0.9981, 0.9883, 0.9368, 0.7612, 0.6193, 0.5274, 0.4523, 0.4028, 0.3294, 0.2810],
    [1.0000, 0.9980, 0.9876, 0.9327, 0.7527, 0.6107, 0.5187, 0.4451, 0.3965, 0.3247, 0.2771],
    [1.0000, 0.9978, 0.9867, 0.9284, 0.7442, 0.6019, 0.5099, 0.4377, 0.3901, 0.3198, 0.2729],
    [1.0000, 0.9978, 0.9860, 0.9241, 0.7358, 0.5933, 0.5016, 0.4310, 0.3842, 0.3151, 0.2693],
    [1.0000, 0.9978, 0.9852, 0.9192, 0.7269, 0.5844, 0.4929, 0.4237, 0.3780, 0.3103, 0.2654],
    [1.0000, 0.9978, 0.9841, 0.9142, 0.7178, 0.5755, 0.4845, 0.4167, 0.3719, 0.3056, 0.2614],
    [1.0000, 0.9978, 0.9832, 0.9089, 0.7087, 0.5668, 0.4761, 0.4098, 0.3659, 0.3010, 0.2578],
    // rho = 0.8025
    [1.0000, 0.9978, 0.9821, 0.9033, 0.6995, 0.5580, 0.4677, 0.4028, 0.3600, 0.2965, 0.2542],
    [1.0000, 0.9977, 0.9806, 0.8970, 0.6899, 0.5490, 0.4593, 0.3958, 0.3540, 0.2917, 0.2503],
    [1.0000, 0.9977, 0.9792, 0.8906, 0.6800, 0.5399, 0.4508, 0.3888, 0.3479, 0.2871, 0.2466],
    [1.0000, 0.9975, 0.9773, 0.8835, 0.6701, 0.5307, 0.4424, 0.3819, 0.3420, 0.2825, 0.2428],
    [1.0000, 0.9975, 0.9754, 0.8763, 0.6599, 0.5216, 0.4343, 0.3752, 0.3362, 0.2780, 0.2392],
    [1.0000, 0.9974, 0.9731, 0.8685, 0.6494, 0.5125, 0.4260, 0.3682, 0.3302, 0.2734, 0.2355],
    [1.0000, 0.9974, 0.9705, 0.8603, 0.6390, 0.5033, 0.4179, 0.3616, 0.3245, 0.2690, 0.2318],
    [1.0000, 0.9974, 0.9682, 0.8517, 0.6283, 0.4941, 0.4099, 0.3550, 0.3188, 0.2645, 0.2282],
    [1.0000, 0.9974, 0.9647, 0.8424, 0.6173, 0.4848, 0.4018, 0.3483, 0.3129, 0.2599, 0.2245],
    [1.0000, 0.9973, 0.9613, 0.8325, 0.6062, 0.4756, 0.3939, 0.3418, 0.3073, 0.2556, 0.2209],
    [1.0000, 0.9973, 0.9574, 0.8225, 0.5950, 0.4664, 0.3861, 0.3353, 0.3016, 0.2512, 0.2174],
    [1.0000, 0.9973, 0.9530, 0.8117, 0.5836, 0.4571, 0.3783, 0.3288, 0.2960, 0.2468, 0.2139],
    [1.0000, 0.9971, 0.9480, 0.8002, 0.5719, 0.4479, 0.3707, 0.3225, 0.2906, 0.2425, 0.2104],
    [1.0000, 0.9971, 0.9425, 0.7883, 0.5602, 0.4387, 0.3632, 0.3162, 0.2851, 0.2383, 0.2069],
    [1.0000, 0.9970, 0.9360, 0.7755, 0.5482, 0.4295, 0.3557, 0.3100, 0.2797, 0.2340, 0.2035],
    [1.0000, 0.9967, 0.9287, 0.7618, 0.5358, 0.4201, 0.3482, 0.3037, 0.2742, 0.2297, 0.1999],
    [1.0000, 0.9963, 0.9206, 0.7476, 0.5234, 0.4108, 0.3408, 0.2976, 0.2688, 0.2255, 0.1965],
    [1.0000, 0.9961, 0.9117, 0.7327, 0.5108, 0.4014, 0.3334, 0.2914, 0.2634, 0.2212, 0.1931],
    [1.0000, 0.9954, 0.9015, 0.7170, 0.4982, 0.3921, 0.3262, 0.2853, 0.2581, 0.2171, 0.1898],
    [1.0000, 0.9953, 0.8905, 0.7009, 0.4854, 0.3830, 0.3192, 0.2795, 0.2530, 0.2131, 0.1865],
    // rho = 0.9025
    [1.0000, 0.9941, 0.8778, 0.6836, 0.4724, 0.3737, 0.3121, 0.2734, 0.2477, 0.2089, 0.1830],
    [1.0000, 0.9934, 0.8640, 0.6659, 0.4593, 0.3647, 0.3052, 0.2677, 0.2427, 0.2049, 0.1797],
    [1.0000, 0.9922, 0.8489, 0.6473, 0.4462, 0.3557, 0.2983, 0.2619, 0.2376, 0.2009, 0.1763],
    [1.0000, 0.9901, 0.8315, 0.6278, 0.4329, 0.3465, 0.2915, 0.2562, 0.2325, 0.1970, 0.1731],
    [1.0000, 0.9873, 0.8124, 0.6075, 0.4194, 0.3374, 0.2849, 0.2504, 0.2274, 0.1929, 0.1698],
    [1.0000, 0.9836, 0.7913, 0.5862, 0.4058, 0.3282, 0.2781, 0.2447, 0.2224, 0.1889, 0.1664],
    [1.0000, 0.9787, 0.7683, 0.5644, 0.3924, 0.3194, 0.2716, 0.2392, 0.2176, 0.1850, 0.1633],
    [1.0000, 0.9720, 0.7427, 0.5417, 0.3786, 0.3104, 0.2651, 0.2338, 0.2128, 0.1812, 0.1601],
    [1.0000, 0.9630, 0.7149, 0.5183, 0.3649, 0.3015, 0.2589, 0.2284, 0.2080, 0.1774, 0.1570],
    [1.0000, 0.9511, 0.6846, 0.4940, 0.3513, 0.2926, 0.2525, 0.2230, 0.2034, 0.1737, 0.1538],
    [1.0000, 0.9347, 0.6515, 0.4692, 0.3374, 0.2838, 0.2463, 0.2178, 0.1987, 0.1700, 0.1508],
    [1.0000, 0.9132, 0.6157, 0.4437, 0.3237, 0.2750, 0.2402, 0.2126, 0.1940, 0.1663, 0.1476],
    [1.0000, 0.8845, 0.5774, 0.4177, 0.3102, 0.2664, 0.2341, 0.2074, 0.1896, 0.1627, 0.1447],
    [1.0000, 0.8468, 0.5363, 0.3913, 0.2965, 0.2577, 0.2283, 0.2024, 0.1851, 0.1591, 0.1416],
    [1.0000, 0.7978, 0.4926, 0.3647, 0.2832, 0.2494, 0.2226, 0.1975, 0.1808, 0.1556, 0.1388],
    [0.9990, 0.7351, 0.4465, 0.3377, 0.2696, 0.2409, 0.2168, 0.1926, 0.1764, 0.1522, 0.1358],
    [0.9929, 0.6559, 0.3984, 0.3106, 0.2561, 0.2325, 0.2112, 0.1878, 0.1722, 0.1487, 0.1330],
    [0.9719, 0.5582, 0.3486, 0.2833, 0.2426, 0.2241, 0.2056, 0.1829, 0.1679, 0.1451, 0.1300],
    [0.8960, 0.4415, 0.2982, 0.2562, 0.2293, 0.2159, 0.2000, 0.1782, 0.1637, 0.1417, 0.1271],
    [0.6457, 0.3112, 0.2477, 0.2293, 0.2161, 0.2078, 0.1946, 0.1737, 0.1596, 0.1384, 0.1243],
    // rho = 1.0025
    [0.0980, 0.1784, 0.1976, 0.2025, 0.2030, 0.1996, 0.1892, 0.1689, 0.1554, 0.1351, 0.1215],
    [0.0054, 0.0953, 0.1557, 0.1782, 0.1905, 0.1917, 0.1839, 0.1645, 0.1514, 0.1318, 0.1187],
    [0.0019, 0.0525, 0.1228, 0.1567, 0.1786, 0.1840, 0.1789, 0.1600, 0.1475, 0.1286, 0.1160],
    [0.0006, 0.0297, 0.0968, 0.1376, 0.1674, 0.1766, 0.1737, 0.1556, 0.1435, 0.1253, 0.1133],
    [0.0004, 0.0177, 0.0766, 0.1209, 0.1569, 0.1696, 0.1689, 0.1515, 0.1398, 0.1224, 0.1106],
    [0.0002, 0.0109, 0.0606, 0.1060, 0.1468, 0.1625, 0.1640, 0.1473, 0.1360, 0.1191, 0.1079],
    [0.0002, 0.0073, 0.0484, 0.0933, 0.1375, 0.1558, 0.1593, 0.1432, 0.1324, 0.1161, 0.1053],
    [0.0002, 0.0052, 0.0386, 0.0817, 0.1284, 0.1491, 0.1546, 0.1390, 0.1287, 0.1130, 0.1026],
    [0.0002, 0.0038, 0.0309, 0.0716, 0.1199, 0.1427, 0.1500, 0.1349, 0.1250, 0.1100, 0.1000],
    [0.0002, 0.0030, 0.0250, 0.0630, 0.1120, 0.1365, 0.1455, 0.1312, 0.1216, 0.1071, 0.0975],
    [0.0001, 0.0024, 0.0203, 0.0552, 0.1046, 0.1306, 0.1411, 0.1273, 0.1180, 0.1042, 0.0949],
    [0.0001, 0.0019, 0.0168, 0.0486, 0.0976, 0.1249, 0.1369, 0.1236, 0.1148, 0.1013, 0.0925],
    [0.0001, 0.0016, 0.0139, 0.0428, 0.0911, 0.1194, 0.1327, 0.1199, 0.1115, 0.0986, 0.0901],
    [0.0001, 0.0013, 0.0117, 0.0378, 0.0849, 0.1141, 0.1286, 0.1164, 0.1082, 0.0959, 0.0877],
    [0.0001, 0.0013, 0.0100, 0.0333, 0.0793, 0.1091, 0.1246, 0.1129, 0.1050, 0.0932, 0.0854],
    [0.0001, 0.0010, 0.0084, 0.0294, 0.0739, 0.1039, 0.1206, 0.1093, 0.1019, 0.0905, 0.0831],
    [0.0001, 0.0010, 0.0074, 0.0261, 0.0688, 0.0992, 0.1168, 0.1060, 0.0988, 0.0879, 0.0807],
    [0.0001, 0.0009, 0.0065, 0.0232, 0.0640, 0.0945, 0.1130, 0.1027, 0.0958, 0.0854, 0.0785],
    [0.0001, 0.0009, 0.0058, 0.0206, 0.0596, 0.0902, 0.1094, 0.0994, 0.0927, 0.0827, 0.0761],
    [0.0001, 0.0007, 0.0050, 0.0182, 0.0554, 0.0858, 0.1057, 0.0961, 0.0897, 0.0802, 0.0739],
    // rho = 1.1025
    [0.0001, 0.0007, 0.0046, 0.0164, 0.0516, 0.0818, 0.1022, 0.0930, 0.0870, 0.0778, 0.0717],
    [0.0001, 0.0007, 0.0042, 0.0148, 0.0480, 0.0779, 0.0989, 0.0901, 0.0842, 0.0754, 0.0696],
    [0.0001, 0.0007, 0.0039, 0.0132, 0.0447, 0.0741, 0.0955, 0.0871, 0.0814, 0.0730, 0.0673],
    [0.0001, 0.0007, 0.0035, 0.0120, 0.0415, 0.0704, 0.0922, 0.0841, 0.0787, 0.0706, 0.0652],
    [0.0001, 0.0007, 0.0033, 0.0110, 0.0387, 0.0669, 0.0890, 0.0812, 0.0761, 0.0684, 0.0632],
    [0.0001, 0.0006, 0.0028, 0.0099, 0.0359, 0.0635, 0.0857, 0.0783, 0.0734, 0.0660, 0.0610],
    [0.0001, 0.0006, 0.0027, 0.0090, 0.0334, 0.0602, 0.0827, 0.0756, 0.0709, 0.0638, 0.0590],
    [0.0001, 0.0005, 0.0024, 0.0083, 0.0309, 0.0570, 0.0797, 0.0728, 0.0683, 0.0616, 0.0569],
    [0.0001, 0.0005, 0.0023, 0.0077, 0.0289, 0.0543, 0.0768, 0.0703, 0.0660, 0.0594, 0.0552],
    [0.0001, 0.0005, 0.0022, 0.0071, 0.0269, 0.0515, 0.0740, 0.0677, 0.0636, 0.0573, 0.0532],
    [0.0001, 0.0005, 0.0020, 0.0066, 0.0250, 0.0487, 0.0713, 0.0652, 0.0612, 0.0552, 0.0512],
    [0.0001, 0.0005, 0.0017, 0.0060, 0.0232, 0.0460, 0.0684, 0.0627, 0.0589, 0.0531, 0.0493],
    [0.0001, 0.0005, 0.0017, 0.0056, 0.0216, 0.0435, 0.0657, 0.0602, 0.0565, 0.0510, 0.0474],
    [0.0001, 0.0005, 0.0016, 0.0053, 0.0201, 0.0412, 0.0633, 0.0580, 0.0545, 0.0490, 0.0457],
    [0.0001, 0.0005, 0.0015, 0.0049, 0.0188, 0.0390, 0.0608, 0.0557, 0.0524, 0.0472, 0.0439],
    [0.0001, 0.0005, 0.0014, 0.0045, 0.0176, 0.0369, 0.0583, 0.0535, 0.0503, 0.0454, 0.0421],
    [0.0001, 0.0004, 0.0014, 0.0044, 0.0163, 0.0348, 0.0559, 0.0513, 0.0482, 0.0435, 0.0404],
    [0.0001, 0.0004, 0.0014, 0.0041, 0.0154, 0.0330, 0.0538, 0.0493, 0.0463, 0.0419, 0.0389],
    [0.0001, 0.0004, 0.0012, 0.0038, 0.0144, 0.0311, 0.0514, 0.0472, 0.0443, 0.0400, 0.0372],
    [0.0001, 0.0004, 0.0011, 0.0035, 0.0133, 0.0293, 0.0493, 0.0451, 0.0424, 0.0382, 0.0355],
    // rho = 1.2025
    [0.0001, 0.0004, 0.0011, 0.0033, 0.0125, 0.0276, 0.0471, 0.0432, 0.0406, 0.0366, 0.0340],
    [0.0001, 0.0004, 0.0011, 0.0033, 0.0117, 0.0261, 0.0451, 0.0413, 0.0388, 0.0349, 0.0324],
    [0.0001, 0.0004, 0.0010, 0.0030, 0.0110, 0.0245, 0.0431, 0.0394, 0.0370, 0.0333, 0.0309],
    [0.0001, 0.0003, 0.0009, 0.0026, 0.0101, 0.0231, 0.0411, 0.0376, 0.0352, 0.0317, 0.0293],
    [0.0001, 0.0003, 0.0009, 0.0026, 0.0096, 0.0218, 0.0392, 0.0358, 0.0335, 0.0302, 0.0280],
    [0.0001, 0.0003, 0.0009, 0.0023, 0.0090, 0.0205, 0.0374, 0.0341, 0.0319, 0.0287, 0.0265],
    [0.0001, 0.0003, 0.0008, 0.0023, 0.0084, 0.0192, 0.0355, 0.0324, 0.0304, 0.0272, 0.0251],
    [0.0001, 0.0003, 0.0008, 0.0021, 0.0079, 0.0181, 0.0339, 0.0309, 0.0289, 0.0258, 0.0238],
    [0.0001, 0.0003, 0.0008, 0.0020, 0.0075, 0.0171, 0.0324, 0.0294, 0.0274, 0.0245, 0.0225],
    [0.0001, 0.0003, 0.0008, 0.0019, 0.0071, 0.0160, 0.0307, 0.0278, 0.0260, 0.0232, 0.0213],
    [0.0001, 0.0003, 0.0007, 0.0017, 0.0066, 0.0151, 0.0291, 0.0264, 0.0246, 0.0219, 0.0200],
    [0.0001, 0.0002, 0.0007, 0.0017, 0.0063, 0.0142, 0.0276, 0.0250, 0.0233, 0.0206, 0.0189],
    [0.0000, 0.0002, 0.0007, 0.0015, 0.0059, 0.0133, 0.0262, 0.0236, 0.0219, 0.0193, 0.0177],
    [0.0000, 0.0002, 0.0006, 0.0014, 0.0055, 0.0125, 0.0247, 0.0224, 0.0206, 0.0182, 0.0165],
    [0.0000, 0.0002, 0.0006, 0.0014, 0.0052, 0.0118, 0.0234, 0.0211, 0.0195, 0.0171, 0.0155],
    [0.0000, 0.0001, 0.0005, 0.0012, 0.0048, 0.0109, 0.0220, 0.0197, 0.0181, 0.0159, 0.0144],
    [0.0000, 0.0001, 0.0005, 0.0012, 0.0045, 0.0103, 0.0208, 0.0186, 0.0170, 0.0149, 0.0135],
    [0.0000, 0.0001, 0.0005, 0.0011, 0.0042, 0.0096, 0.0197, 0.0174, 0.0160, 0.0139, 0.0125],
    [0.0000, 0.0001, 0.0004, 0.0010, 0.0039, 0.0089, 0.0183, 0.0163, 0.0149, 0.0128, 0.0114],
    [0.0000, 0.0001, 0.0004, 0.0010, 0.0036, 0.0083, 0.0173, 0.0153, 0.0139, 0.0119, 0.0105],
    // rho = 1.3025
    [0.0000, 0.0001, 0.0004, 0.0009, 0.0033, 0.0078, 0.0162, 0.0143, 0.0130, 0.0110, 0.0097],
    [0.0000, 0.0001, 0.0004, 0.0008, 0.0031, 0.0072, 0.0151, 0.0133, 0.0120, 0.0102, 0.0088],
    [0.0000, 0.0001, 0.0004, 0.0008, 0.0030, 0.0069, 0.0142, 0.0126, 0.0112, 0.0094, 0.0081],
    [0.0000, 0.0001, 0.0003, 0.0008, 0.0026, 0.0063, 0.0131, 0.0114, 0.0103, 0.0085, 0.0072],
    [0.0000, 0.0001, 0.0003, 0.0008, 0.0026, 0.0059, 0.0123, 0.0105, 0.0095, 0.0078, 0.0067],
    [0.0000, 0.0001, 0.0003, 0.0007, 0.0022, 0.0054, 0.0114, 0.0097, 0.0086, 0.0071, 0.0060],
    [0.0000, 0.0001, 0.0003, 0.0007, 0.0022, 0.0050, 0.0106, 0.0090, 0.0079, 0.0064, 0.0054],
    [0.0000, 0.0001, 0.0003, 0.0006, 0.0019, 0.0046, 0.0097, 0.0082, 0.0072, 0.0057, 0.0048],
    [0.0000, 0.0001, 0.0003, 0.0006, 0.0018, 0.0043, 0.0090, 0.0074, 0.0065, 0.0052, 0.0043],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0016, 0.0038, 0.0081, 0.0067, 0.0058, 0.0044, 0.0036],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0013, 0.0034, 0.0074, 0.0061, 0.0052, 0.0040, 0.0030],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0013, 0.0030, 0.0068, 0.0054, 0.0046, 0.0033, 0.0026],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0011, 0.0027, 0.0060, 0.0048, 0.0040, 0.0029, 0.0021],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0010, 0.0024, 0.0054, 0.0043, 0.0036, 0.0026, 0.0018],
    [0.0000, 0.0001, 0.0002, 0.0003, 0.0008, 0.0020, 0.0048, 0.0037, 0.0030, 0.0021, 0.0014],
    [0.0000, 0.0000, 0.0002, 0.0003, 0.0007, 0.0018, 0.0041, 0.0033, 0.0027, 0.0018, 0.0012],
    [0.0000, 0.0000, 0.0002, 0.0002, 0.0006, 0.0015, 0.0036, 0.0028, 0.0022, 0.0014, 0.0009],
    [0.0000, 0.0000, 0.0002, 0.0002, 0.0006, 0.0013, 0.0030, 0.0023, 0.0019, 0.0013, 0.0007],
    [0.0000, 0.0000, 0.0002, 0.0002, 0.0006, 0.0011, 0.0025, 0.0019, 0.0015, 0.0010, 0.0006],
    [0.0000, 0.0000, 0.0001, 0.0001, 0.0004, 0.0008, 0.0019, 0.0014, 0.0011, 0.0007, 0.0003],
    // rho = 1.4025
    [0.0000, 0.0000, 0.0001, 0.0001, 0.0002, 0.0006, 0.0013, 0.0010, 0.0008, 0.0004, 0.0002],
    [0.0000, 0.0000, 0.0000, 0.0001, 0.0002, 0.0003, 0.0007, 0.0005, 0.0004, 0.0003, 0.0001],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0001, 0.0003, 0.0002, 0.0001, 0.0001, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
];
