//! The colour noise reduction of a RAW, in the camera's own linear light
//! (E3 of v1.18.0): one stage, on the mosaic, before the demosaic and before
//! every per-pixel nonlinear stage of the develop.
//!
//! **Why here.** A render's dark colour is biased by its own chroma noise:
//! the black clip after the white balance and the matrix, the transfer
//! encode, saturation and the final pack each read a dark pixel's noisy
//! colour, and the mean of a nonlinear function of a noisy value is not the
//! function of its mean (measured on the night pair: its dark deciles sit
//! 2.4–6.0 Lab off their binned version, mostly too blue). A colour noise
//! reduction that runs after those stages, as the display-frame pass does,
//! finds the bias already made. Before them, on the linear mosaic, it takes
//! the noise away before anything can rectify it.
//!
//! **The grid.** The mosaic's 2×2 cells: per cell its red, the mean of its two
//! greens and its blue, and the two colour differences `wR·R − G_R`,
//! `wB·B − G_B` (camera white balance, green = 1, so a neutral reads 0). A
//! colour difference is measured at one place: the colour site, against the
//! green the demosaic reads there (`G_R`: the mean of the red site's four
//! green neighbours; the same at the blue site) — the demosaic's own colour
//! difference, so the red's grid sits on the red sites and the blue's on the
//! blue sites. (Against the cell's mean green instead, the difference mixes
//! two places half a cell apart, and on a sharp star the red lands half a
//! pixel down-right of the light and the blue half a pixel up-left: rel1180
//! msg-e3 round 11.) The mosaic's own noise model ([`super::noise_model`])
//! states the grid's noise exactly: one site's difference carries
//! `wR²·var(R) + σ²_G/4`, and two neighbouring sites of a colour share one
//! green, `σ²_G/16`, the only covariance a colour difference's grid has
//! (after a demosaic, neighbouring pixels are interpolated from the same
//! sites). The green sites, which carry half the mosaic and the luminance a
//! Bayer demosaic is built around, are never written; each red and blue site
//! takes the green the demosaic reads there plus its filtered difference.
//!
//! **The pixels** ([`CellChroma::rebuild`]): the sensor measures colour once
//! per cell; pixel-scale chroma is the demosaic's, so the stage's cell chroma
//! is the pixel's — and a pixel's colour follows its light: the cell's
//! colour, carried along the cell's own slope against green, believed only
//! where the green varies more than its noise. After the demosaic every
//! pixel's red and blue are rebuilt on its green from the
//! filtered cell chroma moved along the cell's slope by the pixel's green
//! against the cell's (`c + a·(G − G_cell)`, [`Cells::slopes`]: the display
//! pass's own `a·l + b` in linear light, its `ε` the model's noise variance),
//! all read at that pixel bilinearly in the cell grid (the coordinate of the
//! display pass's own upsample, `render::upsample_at`). Without the rebuild
//! each green site's own noise, which no cell pass writes, becomes
//! pixel-scale colour after the demosaic (26–32 % of the night sky's residual
//! grain, rel1180 msg-e3 round 7); with it that noise is luminance. Without
//! the slope one cell's chroma lands on pixels whose green differs from the
//! cell's: a dark twig inside a bright orange cell takes the sky's colour, and
//! a star's core, brighter than its cell, takes the mean's small difference
//! while the ramp around it takes the rest (round 11: on a sunset's texture window
//! 79 % of the L* edge energy lost sat on the quarter of pixels whose
//! luminance is 20 % or more off their cell's; the night sky's star cores
//! went from R/G 1.06 to 0.80). Colour noise has no content at a pixel's
//! scale; the pixel's light has — the stage decides the colour, the
//! demosaic's luma stays: the rebuilt pixel is moved along the grey, which
//! changes no colour difference, until its luma in the working space
//! ([`camera_luma`]) is the demosaic's own (with only R and B rebuilt, a
//! window-wide chroma flattened a twig's red and blue edge and with them a
//! fifth of a sunset texture's L* edge energy, round 11). No pixel pass: box sums over the cells and a
//! few reads per pixel. The demosaic reads the sensor's own mosaic: its
//! green at a red or blue site is built from that site's own sample (PPG:
//! `(3·G₁ + G₋₁ + R − R₂)/4` along the flattest direction), so a filtered
//! sample written back would move the demosaic's green, and with it the
//! pixel's luma — the rebuild writes the colour after it. (Only a develop
//! that does not demosaic takes the filtered samples into its mosaic.)
//!
//! **The filter.** A sigma filter: a neighbour votes when the evidence says
//! it shares the cell's TRUE colour — it lies within `k` noise sds of an
//! estimate of that colour. The cell's own sample is not that colour: a test
//! anchored on it keeps the neighbours on the sample's side, and with them a
//! share of the sample's own noise (measured on the night sky's flat patches,
//! rel1180 msg-e3 round 8: 0.18–0.22 of it at its `k`, where the plain
//! window mean keeps 1/N = 0.02). So the sample anchors only the first
//! estimate (within `k·σ_d`, `σ_d` the model's sd of a DIFFERENCE of two
//! cells at the anchor cell's own level, `√2` times one cell's `σ`, per
//! colour difference, the red and blue predicted from the green fit — the
//! test's noise is the hypothesis's: the anchor's own level, since a
//! neighbour sharing its true value carries that value's noise; at a patch's
//! mean level a dark branch against a bright sky was tested at the sky's
//! noise, 2.3 times its own, and took the sky's colour, rel1180 msg-e3 round
//! 14); each next pass is anchored on
//! the last estimate, a mean of `n` cells, against which a sample differs by
//! `σ·√(1 + 1/n)`, until an estimate repeats. That fixed point is the mean of
//! the window's samples within `t` of itself — the skipped mean, whose share
//! of one sample's noise is `1 / (N·(Z − 2t·φ(t)))` (`t` in `σ`, `Z` the
//! normal mass within `±t`; its influence function), which falls to the
//! window mean's `1/N` as `k` grows. A neighbour further from the estimate than
//! the noise can carry is scene, and does not vote.
//!
//! **The cell's local plane** ([`local_plane`]). The test asks whether a
//! neighbour shares the cell's true value, and on a chroma gradient the true
//! values of a window are not one constant: on a ramp steeper than `t/ρ` per
//! cell only a strip of neighbours lies within the test of the cell's value,
//! the estimate is that strip's mean and every further pass walks along it —
//! plateaus, blotches and specks on a cloud's chroma ramp (rel1180 msg-e3
//! rounds 15–16: on a 1-sd-per-cell ramp, half side 8, the constant's passes
//! left 3.7 of a cell's variance more step energy than the ramp has, more than
//! the noisy input's own 2.2). A constant is the zero-order local model of
//! smooth content; the first-order one is a plane, which contains it. Per
//! colour difference the plane `d(u, v) = d₀ + gₓ·u + g_y·v` (`u, v` a
//! sample's place against the cell, in cells) is fitted by least squares over
//! the accepted samples, each at the anchor's own-level variance `σ²/c` (`c`
//! the sample's count, a cell's one; the hypothesis puts every accepted
//! sample at the anchor's level, as the test's noise is), and read at the
//! cell: `d₀`. Its error at a place: with `x = (1, u, v)` and
//! `S = Σ c·x·xᵀ` over the accepted set, the fit `β = S⁻¹·Σ c·x·q` has the
//! variance `σ²·S⁻¹`, so the plane's value at `x` has `σ²·h(x)`,
//! `h(x) = xᵀ·S⁻¹·x` (its leverage there; at the cell of a full symmetric
//! window `1/n`, growing as `u²/Σu² + v²/Σv²` toward the window's edges and
//! most where the accepted set is one-sided). A neighbour at `x` that lies on
//! the plane differs from the plane's value there, read without it, by
//! `σ²·(1/c + h(x))`: it is in when `(q − d(x))² ≤ k²·σ²·(1/c + h(x))` — on a
//! flat accepted set at the cell the constant's `σ·√(1 + 1/n)`. The cell's own
//! sample is always in; the first pass is anchored on the plane of zero slope
//! through the sample (its error the sample's, `h = 1`: the width `k·σ·√2`,
//! as before), and the passes repeat as the constant's do, until a value at
//! the cell repeats. A flat window reads the constant's value (a plane of
//! zero slope through the same set).
//!
//! **A plane is read where its own residuals are noise; where they keep a
//! shape, the cell is read as it was.** One neighbour at a time, a step of
//! 4–7 noise sds is within a plane's noise: a few far-side cells admitted on
//! the first pass tilt the plane, the tilted plane's wider test at the far
//! places admits more, and the step is fitted as a ramp whose every residual
//! is within the test (round 17: at half side 8 a 5-sd step bled 0.46 of
//! itself into the column beside it, where the constant bled 0.04). Together
//! the residuals of a plane through a step are not noise: a sawtooth, of one
//! sign along the step and alternating across it. The final accepted set's
//! residuals `e = √c·(q − d(x))` are read for that shape: their lag-1
//! autocorrelation along `x` and along `y`, `r = Σ e_a·e_b / ½Σ (e_a² + e_b²)`
//! over the `P` pairs of axis neighbours both accepted. Under the hypothesis
//! (the plane is the truth, the samples carry the model's noise) the residuals
//! are `e = (I − H)·ε`, `H` the fit's hat matrix
//! (`H_ab = √(c_a·c_b)·x_aᵀ·S⁻¹·x_b`), and `ε` has the cell's variance `v`
//! and, between axis neighbours, the covariance `c_G` of the green their sites
//! share (`ρ_G = c_G/v`, the only covariance the grid has, at the anchor's own
//! level); to first order in `H` and `ρ_G`,
//! `E[Σ e_a·e_b] = v·(ρ_G·P − Σ H_ab)` and
//! `E[½Σ (e_a² + e_b²)] = v·(P − ½Σ (H_aa + H_bb))` over the pairs — the fit's
//! three degrees of freedom: `Σ H_ab ≈ 3` on a full window, so `r` sits near
//! `ρ_G − 3/P`, not 0 — and the sums are `tr(S⁻¹·M)` of the pairs' moments
//! `M`. Their ratio is `r`'s expectation `r₀`, and its sd over `P` white
//! pairs `1/√P`. The plane is refuted on this window when either
//! `|r − r₀| > k_F/√P`: the cell then keeps the constant's
//! reading on the same window ([`skipped_mean`], the law above, unchanged);
//! else the plane's `d₀` is the estimate. A flat window or a ramp reads white
//! (the plane); a step of a few sds, a real texture, a one-sided edge read a
//! shape (the constant). Nothing is clamped and no slope capped: the plane is
//! kept exactly where the data cannot tell it from the truth.
//!
//! **A question is fair over all the questions of its kind the stage asks —
//! the neighbour test over the window's cells, the model read over the
//! frame's.** [`fair_k`] is one derivation: the width at which, over the
//! population a question is asked of, fewer than one true answer is expected
//! to be refused. The neighbour test is asked of a window's `N` cells, so its
//! `k` is `fair_k(N)` (below). The whiteness read is asked once per cell and
//! direction, so its population is the frame's `C = cw·ch` cells and its
//! `k_F = fair_k(C)` (≈ 5.1 at a million cells): fewer than one true plane
//! is expected to be refuted per frame and direction. At the window's `k`
//! it would refute `2/N` of the planes of a clean frame (3.5 % at half side
//! 3), and each false refusal on a ramp is a speck of the constant's strip
//! mean (rel1180 msg-e3 round 18: 0.04–0.13 of a cell's variance of excess
//! step energy on a 1–1.5-sd-per-cell ramp).
//!
//! **A model is read only where its own check can refuse it; where the check
//! has no reach, the cell is read as it was.** The whiteness statistic `r` is
//! bounded by 1, so where its threshold `k_F/√P` is 1 or more the read can
//! refute no plane, however shaped its residuals: the plane there has no
//! check. It is read only where the accepted set's axis pairs exceed `k_F²`
//! in both directions; elsewhere the cell keeps the constant's reading
//! ([`Read::NoReach`]). No number is chosen: `k_F` is the law's and 1 the
//! statistic's range. A window of half side 1 (colour NR 1–20) has at most
//! 12 pairs a direction, under `k_F²` on any frame of more than 1 879 cells
//! (21 at 0.2 million, 24 at a million): it reads the constant, as shipped.
//! At half side 8 (colour NR 25) a full window has 272.
//! *The read's known limit:* between the two, at half side 3 (42 pairs, the
//! threshold 0.70 at `k_F` 4.56), the read has reach but little power on a
//! step of 3–5 sds, which the plane bleeds where the constant does not
//! (rel1180 msg-e3 round 19: into the column beside the edge 0.28 / 0.38–0.41
//! / 0.25–0.29 of a 3 / 4 / 5-sd step, the constant 0.11 / 0.08–0.11 / 0.01).
//! No strength of the slider runs that window ([`fair_test`]: half side 1 to
//! colour NR 20, then 8, 12, 19, 23); a pyramid level's window is the
//! strength's.
//!
//! **The plane's share of a cell's own noise.** At its fixed point the plane
//! solves `Σᵢ cᵢ·xᵢ·ψᵢ(qᵢ − xᵢᵀβ) = 0`, `ψᵢ(e) = e·1{|e| ≤ tᵢ·σ}`,
//! `tᵢ = k·√(1/cᵢ + hᵢ)`: an M-estimator of the plane, whose influence on `β`
//! is `M⁻¹·xᵢ·ψᵢ`, `M = Σ cᵢ·xᵢ·xᵢᵀ·(Z(tᵢ) − 2tᵢ·φ(tᵢ))`. On a symmetric window
//! `tᵢ` is an even function of the place, so `M`'s intercept row is orthogonal
//! to its slopes, and `d₀`'s share of one sample's noise is
//! `1/Σᵢ (Z(tᵢ) − 2tᵢ·φ(tᵢ))` — the skipped mean's `1/(N·(Z − 2t·φ(t)))` with
//! `t` read at each place, `hᵢ = (1/N + uᵢ²/Σu² + vᵢ²/Σv²)/Z` over the
//! accepted share `Z` of the window (first order: the corners' wider test
//! makes the share a little smaller than the constant's). The cells refuted
//! on a flat window keep the constant's share; they are the whiteness test's
//! false refusals, of order `2/N` of the cells.
//!
//! **γ = 1.** In linear light the noise is the only scale at a pixel; a
//! threshold that is a multiple of `σ(L)` is the only form that does not
//! introduce a scale of its own. (The `σ^0.5` that v1.17.1's round 3
//! predicted was measured on the display frame, where the encode bends the
//! scale; it does not apply in the linear domain.)
//!
//! **k is the test's fairness to its window** ([`fair_k`]). The sigma test
//! is a hypothesis test with the model's `σ` known; its level is its only
//! free quantity, and the window sets it. The window asks its `N` cells
//! whether they share the anchor's true value, and a true neighbour lies
//! outside `±k·σ` of that value with probability `2·Φ(−k)` (against the
//! estimate, a mean of `n`, the width `k·σ·√(1 + 1/n)` holds the same
//! probability). Over the window the expected false refusals are
//! `2·N·Φ(−k)`, and `k` is where they reach one: `k = Φ⁻¹(1 − 1/(2N))`,
//! `N = (2ρ + 1)²` the window's cells. The test is fair to its window: of N
//! true neighbours, fewer than one is expected to be refused. The window
//! depends on `k` ([`window`]) and `k` on `N`: the two are read in turn until
//! both repeat ([`fair_test`]). `k` moves with the log of `N` and nothing
//! else — 2.32 over 49 cells, 3.11 over 529, 3.35 over 1225 — so no photo can
//! push it to 1 or to 15. (The law before read `k` off the photo: where the
//! edge energy the filter removed from its textured patches met the noise's
//! share there. The filter ADDS edge energy to texture — an edge-preserving
//! selection assigns a transition cell to one side, and a step holds more
//! squared difference than a ramp — so the removal is not monotone in `k`;
//! its first crossing read 15 on a sunset, where the test had become the
//! window's plain mean and a sky 8 σ off a dark branch's colour voted in the
//! branch's cells, and 1.02 on a night sky: rel1180 msg-e3 round 14.)
//!
//! **The window** ([`window`]): the slider keeps its meaning — at each
//! strength the stage averages as many samples as the pass it replaces, so a
//! flat sky is as clean as before and the texture is now kept by the sigma
//! test, not by a blur. The display pass (`render::detail::chroma_nr`) leaves
//! on a flat region the share `Σw²` of white noise its kernel's weights give
//! (`render::chroma_nr_flat_noise_share`, from its own constants); a plain
//! mean leaves `1/N` of it, so a plain mean's window holds `N = 1/Σw²`
//! pixels, `N/4` cells ([`plain_window`]): at 25, `Σw² = 0.000995`, 1005
//! pixels, a half side of `(√251 − 1)/2 = 7.4`, so 7 cells (rel1180 msg-e3
//! round 9). The stage is not a plain mean: the skipped mean at its test
//! width `t` keeps `1/(N·(Z − 2t·φ(t)))` of a sample's noise (above), so its
//! window holds the `N` cells at which `N·(Z − 2t·φ(t))` is the display
//! pass's `N/4` ([`window`]; `t` the width the stage runs at, the fair
//! test's `k` for that window, read with it to their fixed point,
//! [`fair_test`]). The display pass's reach (its radius law) stays that
//! pass's own, for the sources it still runs on. The noise model is the
//! exposure's: read ONCE, off the sensor's own mosaic ([`SourceReading`]),
//! and kept with the source, so a develop runs the pass alone; a mosaic
//! another process has denoised (a master's AI denoise) is filtered at the
//! sensor's model, an upper bound of the noise left in it.
//!
//! **A preview** ([`preview_level`]): white noise averages the same at any
//! scale, so the wide window is taken where it is cheap — a develop capped at
//! a preview's long edge runs the pass on the cell pyramid's level whose cell
//! is no larger than one of its pixels (`σ/2^L` there, the base window);
//! an export and a master's bake run it at sensor scale.
//!
//! **The slider.** "Colour noise reduction" sets the window as above, and the
//! window its fair test: 0 is off; each strength is one fixed point of the
//! window and `k` ([`fair_test`]), the same for every photo. Below 25 the
//! display pass blends its result with the input and keeps more than one
//! cell's worth of noise; the stage's window is then its least, one cell, so
//! the slider is never dead. Detail and Smoothness belong to the display-frame pass,
//! which a RAW no longer runs; the window reads that pass at Lightroom's own
//! Smoothness, the value an absent one resolves to.
//!
//! **One stage per develop.** The linear stage where a Bayer mosaic the
//! stage can read exists ([`reads`]: one sample per site on a 2×2 RGB
//! pattern); the display-frame pass otherwise — a JPEG, a TIFF, a phone
//! picture, a RAW whose mosaic is not a 2×2 Bayer (X-Trans, a linear DNG),
//! and a master baked without this stage (every master before v1.18.0, until
//! it is baked again). The develop reports which one ran: a master baked
//! with this stage is marked (`render::BakedColourNr`) so that its card runs
//! no second one, and only then. A readable mosaic whose noise cannot be
//! measured (fewer than 16 usable patches: a frame clipped or black all
//! over) has no noise the stage can see, and passes through it unchanged.

use rayon::prelude::*;

use super::noise_model::{bayer_sites, patches, NoiseModel, Patch, PATCH};

/// Can the stage read `raw`'s mosaic — one sample per site on a 2×2 RGB
/// Bayer pattern? The rule the develop runs the stage by and every surface
/// that asks whether a RAW's pixels carry it (`render::SourceFrame::of`,
/// off a metadata-only decode) reads, so the two cannot disagree.
pub(crate) fn reads(raw: &rawler::RawImage) -> bool {
    use rawler::rawimage::RawPhotometricInterpretation as Photo;
    matches!(&raw.photometric, Photo::Cfa(c) if raw.cpp == 1 && bayer_sites(&c.cfa).is_some())
}

/// Lightroom's own default for Color (the strength the lane's probes read at).
#[cfg(test)]
pub(crate) const DEFAULT_AMOUNT: f32 = 25.0;

/// The number of pixels the display pass averages on a flat region at slider
/// value `color_nr` (0..=100), Smoothness at Lightroom's own default: the
/// plain mean that leaves the same share of white noise,
/// `1 / render::chroma_nr_flat_noise_share`.
pub(crate) fn display_samples(color_nr: f32) -> f64 {
    let smooth = crate::recipe::LR_COMPANION_DEFAULTS.iter().find(|(n, _)| *n == "color_nr_smooth").expect("Smoothness is a companion").1;
    1.0 / crate::render::chroma_nr_flat_noise_share(color_nr.clamp(0.0, 100.0) / 100.0, smooth / 100.0)
}

/// The half side in cells of the square holding `n` cells, `(√n − 1)/2`
/// rounded, at least one cell.
fn half_side(n: f64) -> usize {
    ((n.sqrt() - 1.0) / 2.0).round().max(1.0) as usize
}

/// The plain mean's window at slider value `color_nr` (0..=100): the square
/// of cells that holds as many samples as the display pass averages
/// ([`display_samples`] pixels, four to a cell): the window at an unbounded
/// `k`, where [`fair_test`] starts.
pub(crate) fn plain_window(color_nr: f32) -> usize {
    half_side(display_samples(color_nr) / 4.0)
}

/// The standard normal's mass within `±t` and its density at `t` (Simpson's
/// rule on `0..t` in 2000 steps, exact to 1e-12 at any `t` the stage reads;
/// `(1, 0)` at an unbounded `t`).
pub(crate) fn normal_mass(t: f64) -> (f64, f64) {
    let phi = |x: f64| (-x * x / 2.0).exp() / std::f64::consts::TAU.sqrt();
    if !t.is_finite() {
        return (1.0, 0.0);
    }
    let steps = 2000;
    let h = t / steps as f64;
    let inner: f64 = (1..steps).map(|i| phi(i as f64 * h) * if i % 2 == 1 { 4.0 } else { 2.0 }).sum();
    (2.0 * (phi(0.0) + phi(t) + inner) * h / 3.0, phi(t))
}

/// The window's half side in cells at slider value `color_nr` (0..=100) for
/// the stage running at `k` (in `σ_d`): the `N` cells at which the skipped
/// mean at its test width `t = k·√(1 + 1/n)` (in one cell's `σ`, against an
/// estimate of `n = N·Z(t)` samples) leaves the display pass's share,
/// `N·(Z(t) − 2t·φ(t)) = display_samples / 4` (see the module); `t` and `N`
/// depend on each other, and are read in turn until the half side repeats.
/// Unbounded `k` is the plain mean's window.
pub(crate) fn window(color_nr: f32, k: f64) -> usize {
    let plain = display_samples(color_nr) / 4.0;
    if !k.is_finite() {
        return half_side(plain);
    }
    let (mut t, mut half) = (k, 0);
    loop {
        let (z, phi) = normal_mass(t);
        let n = plain / (z - 2.0 * t * phi);
        t = k * (1.0 + 1.0 / (n * z)).sqrt();
        let next = half_side(n);
        if next == half {
            return half;
        }
        half = next;
    }
}

/// The test's width (in `σ`, as [`skipped_mean`] reads it) fair to a window
/// of `n` cells: of `n` true neighbours, fewer than one is expected to be
/// refused — `2·n·Φ(−k) = 1`, `k = Φ⁻¹(1 − 1/(2n))` (see the module), read by
/// bisection on [`normal_mass`] (`n·(1 − Z(k))` falls with `k`).
pub(crate) fn fair_k(n: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, 10.0f64);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if n * (1.0 - normal_mass(mid).0) > 1.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

/// The stage's test at slider value `color_nr` (0..=100): its width `k`
/// ([`fair_k`] of the window's `(2ρ + 1)²` cells) and the window's half side
/// `ρ` ([`window`] at that `k`), read in turn from the plain mean's window
/// until both repeat; `None` when off (the colour-NR control's own rule,
/// `ChromaNrParams::global`). Where the window's whole half side has no fixed
/// point (the share's `N` falls between two odd squares, so `ρ` asks for
/// `ρ + 1` and `ρ + 1` for `ρ`), the larger of the two: its test is fair to
/// it, and it leaves no more than the display pass's share.
pub(crate) fn fair_test(color_nr: f32) -> Option<(f64, usize)> {
    if color_nr.is_nan() || color_nr <= 0.0 {
        return None;
    }
    let color_nr = color_nr.min(100.0);
    let at = |rho: usize| fair_k(((2 * rho + 1) * (2 * rho + 1)) as f64);
    let (mut rho, mut seen) = (plain_window(color_nr), Vec::new());
    loop {
        let next = window(color_nr, at(rho));
        if next == rho {
            return Some((at(rho), rho));
        }
        if seen.contains(&next) {
            let rho = rho.max(next);
            return Some((at(rho), rho));
        }
        seen.push(rho);
        rho = next;
    }
}

/// What the stage reads of an exposure: its noise model, a property of the
/// exposure, not of a develop or a recipe: it is read ONCE, off the sensor's
/// own mosaic ([`read_source`]), and carried (the develop keeps it with the
/// source, `render::SourceFrame::noise_reading`), so a develop runs the pass
/// alone. The model is the sensor's: a mosaic another process has
/// already denoised (a master's AI denoise) does not carry the sensor's
/// Poisson noise, so `var = g·a·m + g²·b` is not its law and a model fitted
/// on it is not a measurement. Run over such a mosaic, the stage's test reads
/// the sensor's `σ` as an UPPER bound of the noise left in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SourceReading {
    pub(crate) model: NoiseModel,
}

/// Read the exposure ([`SourceReading`]) off its normalised signed sensor
/// `mosaic` (`width × height`, sites of `cfa`, white-balanced by `wb` as
/// [`filter_mosaic`] reads it): `None` when the pattern is not a 2×2 RGB
/// Bayer or the frame has too few usable patches for a model.
pub(crate) fn read_source(mosaic: &[f32], width: usize, height: usize, cfa: &rawler::cfa::CFA, wb: [f64; 3]) -> Option<SourceReading> {
    let sites = bayer_sites(cfa)?;
    if mosaic.len() != width * height || wb[1].is_nan() || wb[1] <= 0.0 {
        return None;
    }
    let wb = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    let pts = patches(mosaic, width, height, sites, wb);
    Some(SourceReading { model: NoiseModel::fit(&pts, width, height)? })
}

/// The pyramid level a develop capped at `max_edge` (its long edge) reads
/// the stage's pass at, for a mosaic `width × height`: the largest whose cell
/// (`2^(level + 1)` sensor pixels a side) is no larger than one of its pixels
/// (`max(width, height) / max_edge` sensor pixels) — white noise averages the
/// same at any scale, so the wide window is taken where it is cheap, and a
/// preview pays for no detail it cannot show. 0 — the sensor's own cells —
/// for a develop that is not capped: an export and a master's bake always
/// run at sensor scale.
pub(crate) fn preview_level(width: usize, height: usize, max_edge: Option<u32>) -> u32 {
    let Some(edge) = max_edge.filter(|e| *e > 0) else { return 0 };
    let pixels = width.max(height) as f64 / f64::from(edge);
    (pixels / 2.0).log2().floor().max(0.0) as u32
}

/// Run the stage on the normalised signed `mosaic` (`width × height`, sites
/// of `cfa`), white-balanced by `wb` (any scale; green is divided out), at
/// the test `(k, rho)`: its width and the window's half side in cells
/// ([`fair_test`]). The model is the exposure's `source` reading
/// ([`SourceReading`], read once off the sensor's own mosaic), or, without
/// one, read off `mosaic` itself ([`read_source`]: `mosaic` is then the
/// sensor's). Returns the reading and the filtered cell chroma the demosaiced pixels are rebuilt on
/// ([`CellChroma::rebuild`]), or `None` — and the mosaic untouched — when the
/// pattern is not a 2×2 RGB Bayer or the frame has too few usable patches for
/// a model. The filtered samples are written back into `mosaic` only when
/// `write` (a develop that does not demosaic): a demosaic reads the sensor's
/// own mosaic and takes its colour from the rebuild. At `level` > 0
/// ([`preview_level`]) the pass runs on that level of
/// the cell pyramid ([`Cells::level`], the base window there) and each cell
/// takes the level's filtered chroma read bilinearly at its centre.
#[allow(clippy::too_many_arguments)] // the mosaic's frame plus the stage's test, the exposure's reading, the level and the write
pub(crate) fn filter_mosaic(
    mosaic: &mut [f32],
    width: usize,
    height: usize,
    cfa: &rawler::cfa::CFA,
    wb: [f64; 3],
    (k, rho): (f64, usize),
    source: Option<&SourceReading>,
    level: u32,
    write: bool,
) -> Option<(SourceReading, CellChroma)> {
    let sites = bayer_sites(cfa)?;
    if mosaic.len() != width * height || wb[1].is_nan() || wb[1] <= 0.0 || k.is_nan() || k < 0.0 {
        return None;
    }
    let wb = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    let pts = patches(mosaic, width, height, sites, wb);
    let model = match source {
        Some(r) => r.model,
        None => NoiseModel::fit(&pts, width, height)?,
    };
    let cells = Cells::of(mosaic, width, height, sites, wb, &model, &pts)?;
    let reading = SourceReading { model };
    let filtered: Vec<[f32; 2]> = if level == 0 {
        (0..cells.cw * cells.ch).into_par_iter().map_init(Scratch::default, |s, i| cells.filtered_in(i, k, rho, s).0).collect()
    } else {
        let coarse = cells.level(level);
        let f: Vec<[f32; 2]> =
            (0..coarse.cw * coarse.ch).into_par_iter().map_init(Scratch::default, |s, i| coarse.filtered_in(i, k, rho, s).0).collect();
        let planes = [0, 1].map(|j| f.iter().map(|q| q[j]).collect::<Vec<f32>>());
        let s = (1u32 << level) as f32;
        (0..cells.cw * cells.ch)
            .into_par_iter()
            .map(|i| {
                let (u, v) = (((i % cells.cw) as f32 + 0.5) / s - 0.5, ((i / cells.cw) as f32 + 0.5) / s - 0.5);
                [0, 1].map(|j| crate::render::bilinear_plane(&planes[j], coarse.cw, coarse.ch, u, v))
            })
            .collect()
    };
    if write {
        cells.write(mosaic, width, sites, wb, &filtered);
    }
    let chroma = cells.chroma(&filtered, &model, wb, rho);
    Some((reading, chroma))
}

/// The stage's filtered cell chroma, `wR·R − G` and `wB·B − G` per cell of
/// the mosaic, with each cell's green and the slope of its chroma against
/// green: the planes the demosaiced pixels are rebuilt on.
pub(crate) struct CellChroma {
    cw: usize,
    ch: usize,
    /// `[wR, wB]`, green = 1.
    wb: [f32; 2],
    /// The red and the blue site of a cell, `(dy, dx)`: each colour
    /// difference's plane sits on its own sites.
    origin: [(usize, usize); 2],
    planes: [Vec<f32>; 2],
    /// The cell's green, the mean of its two green sites.
    green: Vec<f32>,
    /// Per colour difference, the cell's slope of chroma against green
    /// ([`Cells::slopes`]).
    slopes: [Vec<f32>; 2],
}

impl CellChroma {
    /// Rebuild the demosaiced camera-native `rgb` (`width` wide, its pixel
    /// `(0, 0)` at mosaic site `(ox, oy)`): a pixel's colour follows its
    /// light — the cell's colour, carried along the cell's own slope against
    /// green, believed only where the green varies more than its noise. The
    /// sensor measures colour once per cell; pixel-scale chroma is the
    /// demosaic's, so the pixel's chroma is the cell's, `c`, moved along the
    /// cell's slope `a` by the pixel's own green against the cell's,
    /// `c + a·(G − G_cell)`. Each pixel keeps its green and takes
    /// `R = (G + c_R + a_R·(G − G_cell))/wR`, the same for blue. A colour
    /// difference is measured at one place: the colour site, against the green
    /// the demosaic reads there — so `c` is read at the pixel bilinearly in
    /// its own site grid (the red's plane on the red sites, the blue's on the
    /// blue sites), while `a` and `G_cell`, window statistics of the cell, are
    /// read at the pixel's centre bilinearly in the cell grid
    /// (`render::upsample_at` at 2×, the display pass's own coordinate).
    /// Where `a` is 0 (a flat cell) the pixel's chroma is the cell's and the
    /// green's own noise stays luminance; where the green carries a star or a
    /// twig, the chroma follows it at the cell's own colour ratio.
    ///
    /// Colour noise has no content at a pixel's scale; the pixel's light has —
    /// the stage decides the colour, the demosaic's luma stays. `luma` is the
    /// working space's luminance of a camera-native pixel, `Y = luma·rgb`
    /// (the Y row of the develop's own camera-to-working matrix, white balance
    /// included: [`camera_luma`]). The colour differences `c` are written as
    /// decided and the pixel's three channels are then moved together along
    /// the grey `(1/wR, 1, 1/wB)` — the one direction that changes no colour
    /// difference — until its luma is the demosaic's own `Y` again: `R`, `B`
    /// moved by a filtered `c` no longer carry the pixel's light away with
    /// them (on a twig a window-wide `c` flattened its red and blue edge, and
    /// with them a fifth of the L* edge energy, rel1180 msg-e3 round 11). A
    /// move along the grey rather than a scale by `Y/Y'`: the colour
    /// differences are what the stage filtered and are kept exactly, and
    /// `Y/Y'` has no bound where the signed dark noise puts `Y'` at 0. Where
    /// that would put a channel over the white level (1, the normalised
    /// mosaic's clip), the display pass's own into-range rule
    /// (`render::detail::chroma_into_range`) applies: the chroma is pulled
    /// toward the grey of the same luma by the largest factor in `[0, 1]`
    /// that fits, and where even that grey does not fit (a clipped core) the
    /// pixel stays the demosaic's. The noise below the black stays signed, as
    /// the develop keeps it (`render::source`). With `luma = [0, 1, 0]` the
    /// luma is the green and the pixel keeps its green bit for bit. Returns
    /// the number of pixels brought into range.
    pub(crate) fn rebuild(&self, rgb: &mut [[f32; 3]], width: usize, ox: usize, oy: usize, luma: [f32; 3]) -> usize {
        let (cw, ch) = (self.cw, self.ch);
        let [wr, wbl] = self.wb;
        let l = luma.map(f64::from);
        let grey = [1.0 / f64::from(wr), 1.0, 1.0 / f64::from(wbl)];
        let grey_y = l[0] * grey[0] + l[1] + l[2] * grey[2];
        let at = |plane: &[f32], sx: usize, sy: usize| crate::render::upsample_at(plane, cw, ch, 2, sx, sy);
        // Plane `j`'s value of cell `(cx, cy)` sits at mosaic site
        // `(2cx + dx, 2cy + dy)`.
        let on_sites = |j: usize, sx: usize, sy: usize| {
            let (dy, dx) = self.origin[j];
            let (u, v) = ((sx as f32 - dx as f32) / 2.0, (sy as f32 - dy as f32) / 2.0);
            crate::render::bilinear_plane(&self.planes[j], cw, ch, u, v)
        };
        let y_of = |p: [f64; 3]| l[0] * p[0] + l[1] * p[1] + l[2] * p[2];
        rgb.par_chunks_mut(width.max(1))
            .enumerate()
            .map(|(y, row)| {
                let mut pulled = 0;
                for (x, px) in row.iter_mut().enumerate() {
                    let (sx, sy) = (x + ox, y + oy);
                    let own = px.map(f64::from);
                    let dg = px[1] - at(&self.green, sx, sy);
                    let cr = on_sites(0, sx, sy) + at(&self.slopes[0], sx, sy) * dg;
                    let cb = on_sites(1, sx, sy) + at(&self.slopes[1], sx, sy) * dg;
                    let v = [f64::from((px[1] + cr) / wr), own[1], f64::from((px[1] + cb) / wbl)];
                    let y = y_of(own);
                    let lift = (y - y_of(v)) / grey_y;
                    let v = [0, 1, 2].map(|c| v[c] + lift * grey[c]);
                    if v.iter().all(|&c| c <= 1.0) {
                        if lift != 0.0 {
                            *px = v.map(|c| c as f32);
                        } else {
                            // The luma is already the demosaic's (always so at
                            // `luma = [0, 1, 0]`): the green is the pixel's own.
                            px[0] = v[0] as f32;
                            px[2] = v[2] as f32;
                        }
                        continue;
                    }
                    pulled += 1;
                    let n = grey.map(|g| g * y / grey_y);
                    if n.iter().any(|&c| c > 1.0) {
                        continue;
                    }
                    let t = (0..3).filter(|&c| v[c] > 1.0).map(|c| (1.0 - n[c]) / (v[c] - n[c])).fold(1.0f64, f64::min).max(0.0);
                    *px = [0, 1, 2].map(|c| (n[c] + t * (v[c] - n[c])) as f32);
                }
                pulled
            })
            .sum()
    }
}

/// The working space's luminance of a camera-native pixel, as coefficients
/// on its camera `R, G, B`: the Y row of the working space (`xyz` the
/// working-to-XYZ matrix's luminance row) through the develop's own
/// camera-to-working matrix `cam2rgb`, after its white balance `wb` —
/// `luma_i = wb_i · Σ_j xyz_j · cam2rgb[j][i]`. The luma the colour stage
/// keeps ([`CellChroma::rebuild`]); `None` (a develop with no camera matrix)
/// reads the green, the luminance a Bayer demosaic is built around.
pub(crate) fn camera_luma(calibration: Option<(&[[f32; 4]; 3], [f32; 4])>, xyz: [f32; 3]) -> [f32; 3] {
    match calibration {
        Some((cam2rgb, wb)) => [0, 1, 2].map(|i| wb[i] * (0..3).map(|j| xyz[j] * cam2rgb[j][i]).sum::<f32>()),
        None => [0.0, 1.0, 0.0],
    }
}

/// The sigma filter's constant reading of the true value of `centre` (one
/// sample of noise sd `sigma`) over `window` (its samples, the centre among
/// them) — the cell's estimate where its local plane is refuted
/// ([`local_plane`]):
/// starting from the sample itself, a mean of `n = 1`, the mean of the window's
/// samples within `k·σ·√(1 + 1/n)` of the current estimate — a sample against
/// a mean of `n` samples differs by that sd — until an estimate repeats (a
/// fixed point, or a cycle: from there the passes only repeat; the estimates
/// are means of subsets of a finite window, so a repeat comes). Returns the
/// estimate and the `n` of the last pass. The first pass accepts the centre
/// (|0| ≤ t), so `n ≥ 1`; a later pass that accepts no sample leaves the
/// estimate it was anchored on, itself a mean of accepted samples.
/// `seen` keeps the estimates seen (cleared first): a worker's buffer, reused
/// across its cells.
pub(crate) fn skipped_mean(window: &[f32], centre: f32, sigma: f64, k: f64, seen: &mut Vec<u32>) -> (f32, u32) {
    let (mut estimate, mut n) = (centre, 1u32);
    seen.clear();
    loop {
        let t = (k * sigma * (1.0 + 1.0 / f64::from(n)).sqrt()) as f32;
        let (mut sum, mut m) = (0.0f64, 0u32);
        for &q in window {
            let inside = (q - estimate).abs() <= t;
            sum += if inside { f64::from(q) } else { 0.0 };
            m += u32::from(inside);
        }
        if m == 0 {
            return (estimate, n);
        }
        let next = (sum / f64::from(m)) as f32;
        n = m;
        if seen.contains(&next.to_bits()) {
            return (next, n);
        }
        seen.push(next.to_bits());
        estimate = next;
    }
}

/// The window a cell's estimate is read over ([`local_plane`]): per colour
/// difference its samples, each sample's place against the cell's (in cells)
/// and its count (a cell's sample is one), the index of the cell's own
/// sample, and the window's row length (a sample's axis neighbours are the
/// next in its row and the one a row on).
#[derive(Default)]
pub(crate) struct Window {
    pub(crate) value: [Vec<f32>; 2],
    pub(crate) place: Vec<[f32; 2]>,
    pub(crate) count: Vec<f32>,
    pub(crate) centre: usize,
    pub(crate) cols: usize,
}

impl Window {
    fn clear(&mut self) {
        self.value.iter_mut().for_each(Vec::clear);
        self.place.clear();
        self.count.clear();
    }
}

/// A plane over a window, `d(u, v) = d₀ + gₓ·u + g_y·v` (the cell at
/// `u = v = 0`), and its error `S⁻¹` (symmetric, `[s00, s01, s02, s11, s12,
/// s22]`), the plane's variance in one cell's `σ²`: at place `x = (1, u, v)`
/// the plane's value has the variance `σ²·xᵀS⁻¹x` (see the module).
#[derive(Clone, Copy)]
struct Plane {
    d: [f64; 3],
    inv: [f64; 6],
}

impl Plane {
    /// The plane's value at `(u, v)` and its variance there, in `σ²`.
    fn at(&self, u: f64, v: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.inv;
        (self.d[0] + self.d[1] * u + self.d[2] * v, a + 2.0 * (b * u + c * v) + d * u * u + 2.0 * e * u * v + f * v * v)
    }

    /// `tr(S⁻¹·M)` of a symmetric moment matrix `m`, stored as `inv` is.
    fn trace(&self, m: [f64; 6]) -> f64 {
        let i = self.inv;
        i[0] * m[0] + 2.0 * (i[1] * m[1] + i[2] * m[2] + i[4] * m[4]) + i[3] * m[3] + i[5] * m[5]
    }

    /// The least-squares plane of the moments `s` (`Σc, Σcu, Σcv, Σcu², Σcuv,
    /// Σcv²`) and `r` (`Σcq, Σcqu, Σcqv`), each sample at the variance `σ²/c`:
    /// `S·β = r`, `β`'s variance `σ²·S⁻¹`. An accepted set on one line (or at
    /// one place) has no evidence on a slope across it: the plane there is
    /// its mean, of zero slope, and its error the mean's.
    fn fit(s: [f64; 6], r: [f64; 3]) -> Self {
        let [s00, s01, s02, s11, s12, s22] = s;
        // The adjugate of the symmetric S.
        let c = [s11 * s22 - s12 * s12, s02 * s12 - s01 * s22, s01 * s12 - s02 * s11, s00 * s22 - s02 * s02, s01 * s02 - s00 * s12, s00 * s11 - s01 * s01];
        let det = s00 * c[0] + s01 * c[1] + s02 * c[2];
        if det.is_nan() || det <= 0.0 {
            return Self { d: [r[0] / s00, 0.0, 0.0], inv: [1.0 / s00, 0.0, 0.0, 0.0, 0.0, 0.0] };
        }
        let inv = c.map(|x| x / det);
        let d = [
            inv[0] * r[0] + inv[1] * r[1] + inv[2] * r[2],
            inv[1] * r[0] + inv[3] * r[1] + inv[4] * r[2],
            inv[2] * r[0] + inv[4] * r[1] + inv[5] * r[2],
        ];
        Self { d, inv }
    }
}

/// Colour difference `j`'s local plane over `window` at the anchor's sd
/// `sigma` and the test's `k` (see the module): each pass fits the
/// least-squares plane over the samples within `k·σ·√(1/c + h)` of the last
/// plane's value at their place, the cell's own sample always in, the first
/// pass anchored on the plane of zero slope through the cell's sample, until
/// a value at the cell repeats (a fixed point, or a cycle: the planes are
/// fits of subsets of a finite window, so a repeat comes). Returns the last
/// plane and its accepted count; `inside` holds the accepted set it was
/// fitted on, `seen` the values seen (both cleared first: a worker's buffers).
fn plane_pass(window: &Window, j: usize, sigma: f64, k: f64, seen: &mut Vec<u32>, inside: &mut Vec<bool>) -> (Plane, u32) {
    let c = window.centre;
    let values = &window.value[j];
    let own = f64::from(window.count[c]);
    let mut plane = Plane { d: [f64::from(values[c]), 0.0, 0.0], inv: [1.0 / own, 0.0, 0.0, 0.0, 0.0, 0.0] };
    let width = (k * sigma).powi(2);
    seen.clear();
    loop {
        inside.clear();
        let (mut s, mut r) = ([0.0f64; 6], [0.0f64; 3]);
        for (i, ((&q, &[u, v]), &m)) in values.iter().zip(&window.place).zip(&window.count).enumerate() {
            let (q, u, v, m) = (f64::from(q), f64::from(u), f64::from(v), f64::from(m));
            let (p, h) = plane.at(u, v);
            let ok = i == c || (q - p).powi(2) <= width * (1.0 / m + h);
            inside.push(ok);
            if ok {
                let (mu, mv) = (m * u, m * v);
                s = [s[0] + m, s[1] + mu, s[2] + mv, s[3] + mu * u, s[4] + mu * v, s[5] + mv * v];
                r = [r[0] + m * q, r[1] + mu * q, r[2] + mv * q];
            }
        }
        let next = Plane::fit(s, r);
        let bits = (next.d[0] as f32).to_bits();
        if seen.contains(&bits) {
            return (next, s[0] as u32);
        }
        seen.push(bits);
        plane = next;
    }
}

/// How a cell's colour difference was read ([`local_plane`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    /// On its plane: the residuals read white where the read has its reach.
    Plane,
    /// As the constant reads it: the plane's residuals keep a shape.
    Shape,
    /// As the constant reads it: the read has no reach (`P ≤ k_F²` accepted
    /// pairs in a direction), so it could refuse nothing.
    NoReach,
}

/// Are `plane`'s residuals over the accepted set `inside` of colour
/// difference `j`'s `window` noise? Their lag-1 autocorrelation along `x` and
/// along `y` against its expectation under the plane, `rho_g` the noise's own
/// correlation of axis neighbours, at the frame's `k` (see the module). A
/// direction of `P ≤ k²` accepted pairs is out of the read's reach (its
/// threshold `k/√P` over the statistic's range, 1): [`Read::NoReach`]. A
/// direction whose residuals are all zero shows no shape.
fn residuals_read(window: &Window, j: usize, plane: &Plane, inside: &[bool], rho_g: f64, k: f64) -> Read {
    let values = &window.value[j];
    let (n, cols) = (values.len(), window.cols);
    let at = |i: usize| {
        let [u, v] = window.place[i].map(f64::from);
        let w = f64::from(window.count[i]).sqrt();
        (w * (f64::from(values[i]) - plane.at(u, v).0), [w, w * u, w * v])
    };
    let mut shaped = false;
    for step in [1, cols] {
        let (mut num, mut den, mut pairs) = (0.0f64, 0.0f64, 0.0f64);
        // The pairs' moments, each `x` carrying its `√c`: `Σ ½(x_a·x_bᵀ +
        // x_b·x_aᵀ)` and `Σ ½(x_a·x_aᵀ + x_b·x_bᵀ)`.
        let (mut cross, mut own) = ([0.0f64; 6], [0.0f64; 6]);
        for a in 0..n.saturating_sub(step) {
            let b = a + step;
            if !inside[a] || !inside[b] || step == 1 && b % cols == 0 {
                continue;
            }
            let ((ea, xa), (eb, xb)) = (at(a), at(b));
            num += ea * eb;
            den += (ea * ea + eb * eb) / 2.0;
            pairs += 1.0;
            for (m, (p, q)) in [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)].into_iter().enumerate() {
                cross[m] += (xa[p] * xb[q] + xb[p] * xa[q]) / 2.0;
                own[m] += (xa[p] * xa[q] + xb[p] * xb[q]) / 2.0;
            }
        }
        if pairs <= k * k {
            return Read::NoReach;
        }
        if den.is_nan() || den <= 0.0 {
            continue;
        }
        let expected = (rho_g * pairs - plane.trace(cross)) / (pairs - plane.trace(own));
        shaped |= (num / den - expected).abs() > k / pairs.sqrt();
    }
    if shaped { Read::Shape } else { Read::Plane }
}

/// The sigma filter's estimate of the true values of a cell's two colour
/// differences over `window` (each difference's samples, the cell's own among
/// them; `sigma` one cell's noise sd of each at the cell's own level, `rho_g`
/// the correlation of that noise with an axis neighbour's): per difference
/// the cell's local plane read at the cell where the plane's residuals are
/// noise, and the constant's reading on the same window ([`skipped_mean`])
/// where they keep a shape (see the module); `k` the neighbour test's (fair to
/// the window), `k_frame` the whiteness read's (fair to the frame). Returns the two values, each
/// difference's accepted count, and how each was read ([`Read`]); `seen` and
/// `inside` are a worker's buffers.
fn local_plane(window: &Window, sigma: [f64; 2], rho_g: [f64; 2], [k, k_frame]: [f64; 2], seen: &mut Vec<u32>, inside: &mut Vec<bool>) -> ([f32; 2], [u32; 2], [Read; 2]) {
    // A window whose pairs in a direction are all out of the read's reach
    // leaves every accepted set in it out of reach: no plane is fitted.
    let (cols, rows) = (window.cols, window.place.len() / window.cols);
    let reach = (rows * (cols - 1)).min((rows - 1) * cols) as f64 > k_frame * k_frame;
    let (mut out, mut count, mut read) = ([0.0f32; 2], [0u32; 2], [Read::NoReach; 2]);
    for j in 0..2 {
        if reach {
            let (plane, n) = plane_pass(window, j, sigma[j], k, seen, inside);
            read[j] = residuals_read(window, j, &plane, inside, rho_g[j], k_frame);
            if read[j] == Read::Plane {
                (out[j], count[j]) = (plane.d[0] as f32, n);
                continue;
            }
        }
        (out[j], count[j]) = skipped_mean(&window.value[j], window.value[j][window.centre], sigma[j], k, seen);
    }
    (out, count, read)
}

/// One worker's buffers for the pass: the window, the values seen and the
/// accepted set.
#[derive(Default)]
struct Scratch {
    window: Window,
    seen: Vec<u32>,
    inside: Vec<bool>,
}

/// The mosaic's cells: the two colour differences and the green of each, and
/// each patch's `σ_d` per colour difference and the covariance two
/// neighbouring cells' differences share.
pub(crate) struct Cells {
    cw: usize,
    ch: usize,
    pw: usize,
    ph: usize,
    /// `[wR·R − G_R, wB·B − G_B, G]` per cell: each colour difference at its
    /// own site against the green the demosaic reads there (`G_R`, `G_B`: the
    /// mean of the site's four green neighbours), and the cell's green (the
    /// mean of its two green sites, the slope's).
    d: Vec<[f32; 3]>,
    /// `σ_d` per patch and colour difference: `√2` times one cell's sd,
    /// `√(wR²·var(R) + σ²_G/4)` (four greens a site).
    sd: Vec<[f32; 2]>,
    /// Per patch, the covariance of two axis-neighbouring cells' differences
    /// (same colour): their sites' green neighbourhoods share one green site,
    /// `σ²_G/16` (`σ²_G` one green site's variance).
    cov: Vec<f32>,
    /// The red and the blue site of a cell, `(dy, dx)`: where each colour
    /// difference's grid sits.
    origin: [(usize, usize); 2],
    /// The noise model, the white balance (green = 1) and the side `s` in
    /// base cells of one of these cells (1 at the base, `2^level` on the
    /// pyramid): what states a cell's noise at its own level ([`Self::own_noise`]).
    model: NoiseModel,
    wb: [f64; 3],
    side: usize,
    /// The whiteness read's `k` ([`residuals_read`]): [`fair_k`] of the
    /// frame's `cw·ch` cells, the population that question is asked over
    /// (once a cell and direction).
    frame_k: f64,
}

impl Cells {
    /// `None` when the frame has no whole patch or `pts` is not its patches.
    pub(crate) fn of(
        mosaic: &[f32],
        width: usize,
        height: usize,
        sites: [(usize, usize); 4],
        wb: [f64; 3],
        model: &NoiseModel,
        pts: &[Patch],
    ) -> Option<Self> {
        let (cw, ch) = (width / 2, height / 2);
        let (pw, ph) = (cw / PATCH, ch / PATCH);
        if pw == 0 || ph == 0 || pts.len() != pw * ph {
            return None;
        }
        let at = |cx: usize, cy: usize, s: usize| {
            let (dy, dx) = sites[s];
            f64::from(mosaic[(2 * cy + dy) * width + 2 * cx + dx])
        };
        // The green the demosaic reads at colour site `s` of cell `(cx, cy)`:
        // the mean of the site's four axis neighbours, all green on a Bayer
        // pattern (those inside the frame, at its edge).
        let green_at = |cx: usize, cy: usize, s: usize| {
            let (dy, dx) = sites[s];
            let (x, y) = (2 * cx + dx, 2 * cy + dy);
            let mut sum = 0.0f64;
            let mut n = 0.0f64;
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < width && ny < height {
                    sum += f64::from(mosaic[ny * width + nx]);
                    n += 1.0;
                }
            }
            sum / n
        };
        let d = (0..cw * ch)
            .into_par_iter()
            .map(|i| {
                let (cx, cy) = (i % cw, i / cw);
                let g = (at(cx, cy, 1) + at(cx, cy, 2)) / 2.0;
                [
                    (wb[0] * at(cx, cy, 0) - green_at(cx, cy, 0)) as f32,
                    (wb[2] * at(cx, cy, 3) - green_at(cx, cy, 3)) as f32,
                    g as f32,
                ]
            })
            .collect();
        // One site's colour difference: its own sample and four greens,
        // `wR²·var(R) + σ²_G/4`; two axis-neighbouring sites of one colour
        // share one of their greens, `cov = σ²_G/16` (the red's and the blue's
        // differences of one cell share two, which the filter, one colour
        // difference at a time, does not read).
        let site_var = |p: &Patch| {
            let g = model.var(p.m, p.r) * wb[1] * wb[1];
            ([wb[0] * wb[0] * model.var(p.m_r, p.r) + g / 4.0, wb[2] * wb[2] * model.var(p.m_b, p.r) + g / 4.0], g / 16.0)
        };
        let sd = pts.iter().map(|p| site_var(p).0.map(|v| (2.0 * v).sqrt() as f32)).collect();
        let cov = pts.iter().map(|p| site_var(p).1 as f32).collect();
        Some(Self { cw, ch, pw, ph, d, sd, cov, origin: [sites[0], sites[3]], model: *model, wb, side: 1, frame_k: fair_k((cw * ch) as f64) })
    }

    /// These cells at pyramid level `level` (`level ≥ 1`): each the mean of a
    /// `2^level × 2^level` block of them (an edge block averaging what it
    /// holds; each colour difference's block in its own grid, so a level cell
    /// sits at its block's centre in that grid). White noise averages the same
    /// at any scale: a mean of `s² = 4^level` cells has the model's own
    /// variance at that scale, `(v + 4c·(1 − 1/s))/s²` — `v` one cell's, `c`
    /// the covariance of the block's `2s(s − 1)` axis-neighbouring pairs
    /// (counted both ways, `4s(s − 1)`) — no new number; two neighbouring level
    /// cells share the `s` greens along their border, `c/s³`. A level patch is
    /// [`PATCH`] level cells, as at the base. Built from the base cells.
    fn level(&self, level: u32) -> Self {
        let s = 1usize << level;
        let (cw, ch) = (self.cw.div_ceil(s), self.ch.div_ceil(s));
        let d: Vec<[f32; 3]> = (0..cw * ch)
            .into_par_iter()
            .map(|i| {
                let (x0, y0) = ((i % cw) * s, (i / cw) * s);
                let (x1, y1) = ((x0 + s).min(self.cw), (y0 + s).min(self.ch));
                let mut acc = [0.0f64; 3];
                for y in y0..y1 {
                    for q in &self.d[y * self.cw + x0..y * self.cw + x1] {
                        for c in 0..3 {
                            acc[c] += f64::from(q[c]);
                        }
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as f64;
                acc.map(|a| (a / n) as f32)
            })
            .collect();
        let (pw, ph) = (cw.div_ceil(PATCH).max(1), ch.div_ceil(PATCH).max(1));
        // The base patches under level patch `(px, py)`: `s` of them a side.
        let span = |p: usize, n: usize| {
            let lo = (p * s).min(n - 1);
            lo..(p * s + s).min(n).max(lo + 1)
        };
        let sf = s as f64;
        let (sd, cov): (Vec<[f32; 2]>, Vec<f32>) = (0..pw * ph)
            .map(|p| {
                let (mut v, mut c, mut n) = ([0.0f64; 2], 0.0f64, 0.0);
                for by in span(p / pw, self.ph) {
                    for bx in span(p % pw, self.pw) {
                        let q = self.sd[by * self.pw + bx];
                        // `σ_d² = 2v`: one cell's variance is half of it.
                        v[0] += f64::from(q[0]).powi(2) / 2.0;
                        v[1] += f64::from(q[1]).powi(2) / 2.0;
                        c += f64::from(self.cov[by * self.pw + bx]);
                        n += 1.0;
                    }
                }
                let c = c / n;
                let sd = v.map(|v| (2.0 * (v / n + 4.0 * c * (1.0 - 1.0 / sf)) / (sf * sf)).sqrt() as f32);
                (sd, (c / (sf * sf * sf)) as f32)
            })
            .unzip();
        Self { cw, ch, pw, ph, d, sd, cov, origin: self.origin, model: self.model, wb: self.wb, side: self.side * s, frame_k: fair_k((cw * ch) as f64) }
    }

    /// One cell's noise sd per colour difference at cell `i`'s OWN level —
    /// the test's noise is the hypothesis's: the anchor's own level. The test
    /// asks whether a neighbour shares the anchor's true value, and a sample
    /// of that value carries the noise the model gives that value's level, not
    /// the patch's mean level (on a dark branch against a bright sky the
    /// patch's mean is the sky's, 2.3 times the branch cells' sd, and the
    /// window's brighter cells passed the test: rel1180 msg-e3 round 14). The
    /// level is the cell's own measurement: its green `G`, its red
    /// `(d_R + G)/wR` and its blue `(d_B + G)/wB` (the colour site's green read
    /// as the cell's); one site's difference `wR²·var(R) + σ²_G/4`, and at a
    /// pyramid level of side `s` the mean of `s²` of them,
    /// `(v + 4c·(1 − 1/s))/s²` with `c = σ²_G/16` (as [`Self::level`] states
    /// a patch's), at the cell's own radius. With it, the correlation of the
    /// cell's noise with an axis neighbour's at the same level: the greens
    /// their sites share, `c/s³`, over that variance.
    fn own_noise(&self, i: usize) -> ([f64; 2], [f64; 2]) {
        let (cx, cy) = (i % self.cw, i / self.cw);
        let s = self.side as f64;
        let r = self.model.radius((cx as f64 + 0.5) * 2.0 * s, (cy as f64 + 0.5) * 2.0 * s);
        let [dr, db, g] = self.d[i].map(f64::from);
        let vg = self.model.var(g, r) * self.wb[1] * self.wb[1];
        let c = vg / 16.0;
        let site = |w: f64, dc: f64| w * w * self.model.var((dc + g) / w, r) + vg / 4.0;
        let var = [site(self.wb[0], dr), site(self.wb[2], db)].map(|v| (v + 4.0 * c * (1.0 - 1.0 / s)) / (s * s));
        (var.map(f64::sqrt), var.map(|v| c / (s * s * s) / v))
    }

    /// Cell `i`'s two colour differences through the sigma filter at `k`:
    /// the test is anchored on an estimate of the cell's true colour, not on
    /// its own noisy sample (see the module).
    #[cfg(test)]
    fn filtered(&self, i: usize, k: f64, rho: usize) -> [f32; 2] {
        self.filtered_counted(i, k, rho).0
    }

    /// [`Self::filtered`] with the count of window samples the last pass
    /// accepted and whether each difference was read on its plane.
    #[cfg(test)]
    fn filtered_counted(&self, i: usize, k: f64, rho: usize) -> ([f32; 2], [u32; 2], [Read; 2]) {
        self.filtered_in(i, k, rho, &mut Scratch::default())
    }

    /// [`Self::filtered_counted`] in a worker's reused buffers: the window's
    /// samples gathered once per centre, both colour differences in one sweep.
    fn filtered_in(&self, i: usize, k: f64, rho: usize, scratch: &mut Scratch) -> ([f32; 2], [u32; 2], [Read; 2]) {
        let (cw, ch) = (self.cw, self.ch);
        let (cx, cy) = (i % cw, i / cw);
        let (sd, rho_g) = self.own_noise(i);
        let (y0, y1, x0, x1) = (cy.saturating_sub(rho), (cy + rho + 1).min(ch), cx.saturating_sub(rho), (cx + rho + 1).min(cw));
        let Scratch { window, seen, inside } = scratch;
        window.clear();
        for y in y0..y1 {
            for (x, q) in (x0..x1).zip(&self.d[y * cw + x0..y * cw + x1]) {
                window.value[0].push(q[0]);
                window.value[1].push(q[1]);
                window.place.push([x as f32 - cx as f32, y as f32 - cy as f32]);
                window.count.push(1.0);
            }
        }
        (window.centre, window.cols) = ((cy - y0) * (x1 - x0) + cx - x0, x1 - x0);
        local_plane(window, sd, rho_g, [k, self.frame_k], seen, inside)
    }

    /// The planes the demosaiced pixels are rebuilt on ([`CellChroma`]):
    /// the filtered cell chroma, each cell's green and its slope
    /// ([`Self::slopes`]) over the window of half side `rho` cells.
    fn chroma(&self, filtered: &[[f32; 2]], model: &NoiseModel, wb: [f64; 3], rho: usize) -> CellChroma {
        CellChroma {
            cw: self.cw,
            ch: self.ch,
            wb: [wb[0] as f32, wb[2] as f32],
            origin: self.origin,
            planes: [0, 1].map(|j| filtered.iter().map(|f| f[j]).collect()),
            green: self.d.iter().map(|d| d[2]).collect(),
            slopes: self.slopes(filtered, model, rho),
        }
    }

    /// Each cell's slope of its filtered chroma (`filtered`) against its
    /// green over the window of half side `rho` cells, per colour difference:
    /// `a = cov(G, c) / (var(G) + σ²_G)`, the display pass's own `a·l + b`
    /// (`render::detail::chroma_nr`) brought to linear light with the model
    /// in place of a chosen `ε` — `σ²_G` the model's noise variance of a
    /// cell's green (the mean of two sites, half one site's) at the window's
    /// mean green, at the cell's radius. That is the only shrinkage, and it is
    /// derived: a slope is believed where the green's variance exceeds the
    /// noise's. On a flat sky `cov → 0`, so `a → 0` and the pixel keeps the
    /// cell's chroma (the green's noise is not re-injected into colour); on a
    /// star or a twig `var(G) ≫ σ²_G` and `a` is the true slope. Box sums,
    /// O(cells): column sums slid down a band of rows, row sums slid along.
    fn slopes(&self, filtered: &[[f32; 2]], model: &NoiseModel, rho: usize) -> [Vec<f32>; 2] {
        const BAND: usize = 64;
        let (cw, ch) = (self.cw, self.ch);
        let mut both = vec![[0.0f32; 2]; cw * ch];
        // Per cell: G, G², c_R, c_B, G·c_R, G·c_B.
        let q = |i: usize| {
            let g = f64::from(self.d[i][2]);
            let [c0, c1] = filtered[i].map(f64::from);
            [g, g * g, c0, c1, g * c0, g * c1]
        };
        both.par_chunks_mut(cw * BAND).enumerate().for_each(|(band, rows)| {
            let y0 = band * BAND;
            let mut col = vec![[0.0f64; 6]; cw];
            let slide = |col: &mut [[f64; 6]], y: usize, sign: f64| {
                for (x, c) in col.iter_mut().enumerate() {
                    let v = q(y * cw + x);
                    for k in 0..6 {
                        c[k] += sign * v[k];
                    }
                }
            };
            for y in y0.saturating_sub(rho)..(y0 + rho + 1).min(ch) {
                slide(&mut col, y, 1.0);
            }
            for (dy, out) in rows.chunks_mut(cw).enumerate() {
                let y = y0 + dy;
                let rows_n = ((y + rho + 1).min(ch) - y.saturating_sub(rho)) as f64;
                let mut s = [0.0f64; 6];
                for c in &col[..(rho + 1).min(cw)] {
                    for k in 0..6 {
                        s[k] += c[k];
                    }
                }
                for (x, a) in out.iter_mut().enumerate() {
                    let n = rows_n * ((x + rho + 1).min(cw) - x.saturating_sub(rho)) as f64;
                    let mg = s[0] / n;
                    let noise = model.var(mg, model.radius((2 * x + 1) as f64, (2 * y + 1) as f64)) / 2.0;
                    let den = (s[1] / n - mg * mg).max(0.0) + noise;
                    *a = [0, 1].map(|j| if den > 0.0 { ((s[4 + j] / n - mg * s[2 + j] / n) / den) as f32 } else { 0.0 });
                    if x + rho + 1 < cw {
                        for k in 0..6 {
                            s[k] += col[x + rho + 1][k];
                        }
                    }
                    if x >= rho {
                        for k in 0..6 {
                            s[k] -= col[x - rho][k];
                        }
                    }
                }
                if y + rho + 1 < ch {
                    slide(&mut col, y + rho + 1, 1.0);
                }
                if y >= rho {
                    slide(&mut col, y - rho, -1.0);
                }
            }
        });
        [0, 1].map(|j| both.iter().map(|a| a[j]).collect())
    }

    /// Write `filtered` back: each red and blue site takes the green the
    /// demosaic reads there plus its filtered colour difference,
    /// `G_site + c` — its own sample moved by the filter's change of its
    /// difference, `(c − d)/w` — so the demosaic sees the filtered difference
    /// at the site exactly. The greens untouched.
    fn write(&self, mosaic: &mut [f32], width: usize, sites: [(usize, usize); 4], wb: [f64; 3], filtered: &[[f32; 2]]) {
        let cw = self.cw;
        let (wr, wbl) = (wb[0] as f32, wb[2] as f32);
        let (r_at, b_at) = (sites[0], sites[3]);
        mosaic.par_chunks_mut(2 * width).take(self.ch).enumerate().for_each(|(cy, lines)| {
            for cx in 0..cw {
                let [dr, db] = filtered[cy * cw + cx];
                let d = self.d[cy * cw + cx];
                lines[r_at.0 * width + 2 * cx + r_at.1] += (dr - d[0]) / wr;
                lines[b_at.0 * width + 2 * cx + b_at.1] += (db - d[1]) / wbl;
            }
        });
    }
}

#[cfg(test)]
mod tests;
