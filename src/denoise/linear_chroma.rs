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
//! **The read: the light is read first on its own evidence; the colour is
//! read through the light that was read; the slope is read where the read
//! light carries content** ([`Cells::guided`]). A photo's colour edges sit on
//! its light's edges, and the light — the cell's green `G`, the demosaic's
//! luma, four times the colour difference's samples — is the stage's
//! strongest measurement. Per cell `i`: `G_i`, the colour difference `d_i`
//! (each of the two, read alone), and `v_i` the model's variance of `G_i` at
//! its own level.
//!
//! (1) The light ([`Cells::read_light`]): `G` through `L` levels of the
//! à-trous B3 pyramid ([`SPLINE`]), each detail coefficient `w_l` kept where
//! `|w_l| > k_F·σ_{w_l}` (`σ_{w_l}` exact from the taps and the cells' `v`;
//! `k_F` [`fair_k`] of the frame's cells, the fairness of a question asked
//! once per cell) and nothing otherwise: `Ĝ = s_L + Σ read w_l`. `L` is the
//! colour's ([`levels`]), so the guide is smooth to the colour's support. Its
//! residual noise `r_i` is exact from the taps (`Ĝ_i = Σ_a c_a·(P_a·G)_i`,
//! `r_i = Σ_{a,b} c_a·c_b·((P_a∘P_b)⊗(P_a∘P_b)·v)_i`); on a flat frame it is
//! [`smooth_share`] of `v` (0.0036 at `L` 3). A gradient of the light has no
//! detail at any level; a light step is kept at the two cells beside it once
//! its coefficient there is read at every level — `0.351`, `0.506`, `0.514`
//! of the step per light sd at levels 0, 1, 2, so above `k_F/0.351` light sds
//! a cell (13.0 at `k_F` 4.56).
//!
//! (2) The colour, per box window `Ω` of half side `ρ` (`N = n²` cells,
//! `n = 2ρ + 1`; at the frame's edge the cells it holds):
//! `a_Ω = (cov_Ω(Ĝ, d) − κ_Ω)/(var_Ω(Ĝ) + r̄_Ω)`, `b_Ω = d̄ − a_Ω·Ĝ̄`, and the
//! cell reads `d̂_i = ā_i·Ĝ_i + b̄_i`, `ā`, `b̄` the means of `a_Ω`, `b_Ω` over
//! the windows holding `i`. The ridge `r̄_Ω` is the guide's own residual in
//! the window (the line is fitted on `Ĝ`). `κ_Ω` is the noise the read light
//! and the difference share: the difference is read against the green the
//! demosaic reads at its colour site, the mean of four green sites, two of
//! them the cell's own and one each of two neighbours' ([`Cells::light_noise`]:
//! `cov(G_i, d_i) = −σ²_G/4`, `−σ²_G/8` with each of the two neighbours), and
//! the read keeps of it `t1 = Σ_a c_a·(P_a(i, i)·κ_own + P_a(i, i±1)·κ_cross)`
//! a cell, less a window's whole share ([`shared_noise`], [`window_mass`]) —
//! on a flat frame at `L` 3 the derived −1.176e-8 against the measured
//! −1.186e-8 ± 0.014e-8 over twenty frames (rel1180 msg-e3 round 25); on the
//! raw light the line of a flat frame would be the noise's own, `a → −1/4`.
//! (3) The slope is read ([`slope_read`]) only where the read light varies
//! over the window beyond what its own residual makes there:
//! `var_Ω(Ĝ) − r̄_Ω > k_F·r̄_Ω·√(2/ν)`, `ν` the window's effective number of
//! independent guide samples ([`window_dof`]: the read light on a flat frame
//! is white noise through the composed kernel, and a window's variance of it
//! is `(μ·r̄/ν)·χ²_ν`, `μ` and `ν` from the kernel's autocorrelation over the
//! window — at 25, `ρ` 6, `L` 3: `ν` 3.32, `μ` 0.40); `a_Ω = 0` otherwise and
//! the window's line is its box mean. On a flat frame the read light is its
//! own noise smoothed, small and coherent over the window (3.3 independent
//! samples in 169 cells), and a slope fitted on it is a ratio of two small
//! numbers that draws a random line through any chroma ramp (round 24: 0.94 to
//! 2.82 cell sds on ramps of 0.5 to 1.5 sds a cell). Nothing in the read is a
//! chosen number: `k_F` is the frame's, `r`, `κ`, `ν` the model's and the
//! taps', `ρ` and `L` the slider's (below).
//!
//! What it does. On a flat frame no slope is read but at the false reads and
//! the read is the box mean of box means. A chroma ramp under a flat light is
//! the box mean's, exact. Where the light and the colour change together — a
//! cloud's band, a branch against the sky, a star against its sky — every
//! window's line follows the read light, and the cell is read at its own
//! light: a dark cell beside a bright one lies on every line through it. A
//! ramp of both copies the read light's noise at the line's slope `α`, less
//! what the windows' own lines take of it: `α²·v·Σ(K_L − h·K_L)²` of it, `h`
//! the mean of the windows' least-squares lines (0.00044 of a cell's variance
//! at 25 where `α²·v` is one cell's; on the raw light 0.41–0.95). Its known
//! limits, measured (round 25): a colour edge is kept where the light shows
//! an edge the stage can read; where the light's own edge is under its noise
//! the colour is averaged across it, as it was without the guide (a one-stop
//! step at 5.8 light sds bleeds 0.42 beside the edge, at 20 light sds 0.11);
//! a colour edge the light does not carry (iso-luminant) is the box mean
//! across it. A light coefficient read on noise (at `k_F`, about one a level
//! a frame) is carried whole by the guide, and where a slope `α` is read the
//! colour copies it: a speck of `k_F·σ_{w_0}·α` (4.1 cell sds on the ramp
//! fixture, where `α²·v` is one cell's variance).
//!
//! **The noise left on a flat frame, and the slider.** On a flat frame
//! `d̂_i = (1/N)·Σ_Ω d̄_Ω` but at the false reads, which weighs cell `j` by
//! `T_ij/N²`, `T_ij` the number of windows holding both — a triangle of base
//! `2n − 1` a side, `Σ t² = n·(2n² + 1)/3` — so it keeps
//! `Σ T²/N⁴ = ((2n² + 1)/(3n³))²` of white noise ([`box_share`]; ≈ `4/(9N)`,
//! less than one box's `1/N`: the mean of box means reaches twice as far); the
//! grid adds the shared green between axis neighbours' differences,
//! `4·ρ_G·τ·box_share` (`ρ_G = σ²_G/16/σ²_d`, `τ` the triangle's lag-1
//! autocorrelation). A falsely read window adds at most `σ²_d/N` of slope
//! energy over its cells, so the false reads keep at most the expected read
//! windows over `N·C` (`C` the frame's cells): the window's null, at most
//! `C·(x·e^(1−x))^(ν/2)` (Chernoff, `x = (1 + k_F·√(2/ν))/μ`; 0.40 a frame at
//! 25), and the windows the light's false reads lift (7.8e-5 of a cell's
//! variance at 25). The slider ([`window`]) keeps its meaning: at each
//! strength the stage leaves no more of the noise than the display pass it
//! replaces — `render::chroma_nr_flat_noise_share`, `1/display_samples` of a
//! pixel's, `4/display_samples` of a cell's — and `ρ` is the smallest half
//! side at which [`flat_share`] is at most that (at 25, 0.00398: `ρ` 6,
//! 0.00331). [`flat_share`] is round 23's: the box share and the slope term a
//! read line adds on white noise, `box_share/4` — above what the read keeps
//! now that a flat frame reads no slope. The least is one cell, so the slider
//! is never dead; 0 is off.
//!
//! **A cell's own noise.** On a flat frame the read's regression on a cell's
//! own sample is the box term's centre weight `T_ii/N² = 1/N` and its four
//! axis neighbours' through the green they share, `4·ρ_G·(n − 1)/(n·N)`.
//!
//! **γ = 1.** In linear light the noise is the only scale at a pixel; a ridge
//! that is the model's variance at each cell's own level is the only form that
//! does not introduce a scale of its own. (The `σ^0.5` that v1.17.1's round 3
//! predicted was measured on the display frame, where the encode bends the
//! scale; it does not apply in the linear domain.)
//!
//! **The slope's window.** The pixel rebuild's slope ([`Cells::slopes`]) is a
//! window statistic, read over the same window. The noise model is the
//! exposure's: read ONCE, off the sensor's own mosaic ([`SourceReading`]), and
//! kept with the source, so a develop runs the pass alone; a mosaic another
//! process has denoised (a master's AI denoise) is read at the sensor's
//! model, an upper bound of the noise left in it.
//!
//! **Cost.** The light's read: per level three separable variance passes and
//! the detail read, then `(L+1)(L+2)/2` separable passes for its residual;
//! the colour's: two passes of box means, O(cells) each — nine quantities a
//! cell (`Ĝ`, `Ĝ²`, both `d`, both `Ĝ·d`, `r`, and the shared noise's two
//! terms) for the lines, four (`a`, `b` of both) for their means.
//!
//! **A preview** ([`preview_level`]): white noise averages the same at any
//! scale, so the read is taken where it is cheap — a develop capped at a
//! preview's long edge runs it on the cell pyramid's level whose cell is no
//! larger than one of its pixels (`σ/2^level` there, the same `ρ`); an export and a master's bake run it at sensor scale.
//!
//! (Withdrawn, rel1180 msg-e3 rounds 8–24: a sigma filter — a skipped mean
//! over a window fair to its cells, then a local plane with a whiteness read
//! of its residuals and a fallback to the constant, then a window grown ring
//! by ring. A local polynomial over a fixed or grown geometric support cannot
//! be both as large as the display share asks, 17 cells, and as small as a
//! sunset's curvature, and a test of one neighbour at a time cannot read the
//! collective evidence an edge or a ramp carries: plateaus and blotches on a
//! cloud's chroma band, a 5-sd step bled into the column beside it. The read
//! of each coefficient of a multi-scale (spline) pyramid against its own noise
//! followed gradients exactly and kept no colour edge under ~15 sds: beside an
//! edge no single coefficient carries the evidence its length does (round 22).
//! Both read the colour difference by its own evidence alone; the light
//! carries a photo's colour edges, and the read through it replaces both. Read
//! through the raw light, the line copied the light's noise into the colour
//! wherever the two change together, 0.41–0.95 of a cell's variance (round
//! 23); the light is read first, by round 22's read. Read through the read
//! light everywhere, the slope of a flat window drew a random line through a
//! chroma ramp (round 24); it is read where the read light carries content.)
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

/// The guided read's window at slider value `color_nr` (0..=100): the
/// smallest half side `ρ` (at least one cell, so the slider is never dead) at
/// which the slider's share of a flat frame's noise ([`flat_share`]) is at most
/// the display pass's share of a cell's, `4 / display_samples` (see the
/// module); `None` when off (the colour-NR control's own rule,
/// `ChromaNrParams::global`).
pub(crate) fn window(color_nr: f32) -> Option<usize> {
    if color_nr.is_nan() || color_nr <= 0.0 {
        return None;
    }
    let share = 4.0 / display_samples(color_nr.min(100.0));
    (1usize..).find(|&rho| flat_share(rho) <= share)
}

/// The share of a cell's white noise the box mean of the windows' box means
/// keeps (the read's `b̄` on a flat frame): `Σ T²/N⁴`, `T` the count of
/// windows of `N = n²` cells (`n = 2ρ + 1`) holding two cells — a triangle
/// of base `2n − 1` along each axis, `Σ t² = n·(2n² + 1)/3` — so
/// `((2n² + 1)/(3n³))²` (see the module).
pub(crate) fn box_share(rho: usize) -> f64 {
    let n = (2 * rho + 1) as f64;
    ((2.0 * n * n + 1.0) / (3.0 * n * n * n)).powi(2)
}

/// The slider's share of a cell's white noise on a flat frame (see the
/// module): the box term [`box_share`] and the slope term a read line adds,
/// a quarter of it — `ā·(G − Ḡ)` with `var(ā) = box_share·var(d)/(4·v)` (the
/// ridge doubles the window's light variance) — for a light and a colour
/// difference of independent white noise. Round 23's form, kept as the
/// slider's: a flat frame reads no slope ([`slope_read`]), so the read keeps
/// the box share and at most this.
pub(crate) fn flat_share(rho: usize) -> f64 {
    1.25 * box_share(rho)
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

/// The width (in `σ`) of a question fair to the `n` questions of its kind:
/// fewer than one true answer is expected to be refused —
/// `2·n·Φ(−k) = 1`, `k = Φ⁻¹(1 − 1/(2n))` (see the module), read by
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
/// the window of half side `rho` cells ([`window`]). The model is the exposure's `source` reading
/// ([`SourceReading`], read once off the sensor's own mosaic), or, without
/// one, read off `mosaic` itself ([`read_source`]: `mosaic` is then the
/// sensor's). Returns the reading and the filtered cell chroma the demosaiced pixels are rebuilt on
/// ([`CellChroma::rebuild`]), or `None` — and the mosaic untouched — when the
/// pattern is not a 2×2 RGB Bayer or the frame has too few usable patches for
/// a model. The filtered samples are written back into `mosaic` only when
/// `write` (a develop that does not demosaic): a demosaic reads the sensor's
/// own mosaic and takes its colour from the rebuild. At `level` > 0
/// ([`preview_level`]) the pass runs on that level of
/// the cell pyramid ([`Cells::level`], the same window) and each cell
/// takes the level's filtered chroma read bilinearly at its centre.
#[allow(clippy::too_many_arguments)] // the mosaic's frame plus the read's window and levels, the exposure's reading, the level and the write
pub(crate) fn filter_mosaic(
    mosaic: &mut [f32],
    width: usize,
    height: usize,
    cfa: &rawler::cfa::CFA,
    wb: [f64; 3],
    rho: usize,
    levels: u32,
    source: Option<&SourceReading>,
    level: u32,
    write: bool,
) -> Option<(SourceReading, CellChroma)> {
    let sites = bayer_sites(cfa)?;
    if mosaic.len() != width * height || wb[1].is_nan() || wb[1] <= 0.0 {
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
        cells.guided(rho, levels)
    } else {
        let coarse = cells.level(level);
        let f = coarse.guided(rho, levels);
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

/// Over every cell `i` of a `cw × ch` grid, the means of `K` per-cell
/// quantities `q(j)` over the box window of half side `rho` about `i` (the
/// cells of the window inside the frame, `n` of them), finished by
/// `f(i, means, n)`. O(cells): column sums slid down a band of rows, row sums
/// slid along, in f64.
fn box_means<const K: usize, T: Send + Copy + Default>(
    (cw, ch): (usize, usize),
    rho: usize,
    q: impl Fn(usize) -> [f64; K] + Sync,
    f: impl Fn(usize, [f64; K], f64) -> T + Sync,
) -> Vec<T> {
    const BAND: usize = 64;
    let mut out = vec![T::default(); cw * ch];
    out.par_chunks_mut(cw * BAND).enumerate().for_each(|(band, rows)| {
        let y0 = band * BAND;
        let mut col = vec![[0.0f64; K]; cw];
        let slide = |col: &mut [[f64; K]], y: usize, sign: f64| {
            for (x, c) in col.iter_mut().enumerate() {
                let v = q(y * cw + x);
                for k in 0..K {
                    c[k] += sign * v[k];
                }
            }
        };
        for y in y0.saturating_sub(rho)..(y0 + rho + 1).min(ch) {
            slide(&mut col, y, 1.0);
        }
        for (dy, line) in rows.chunks_mut(cw).enumerate() {
            let y = y0 + dy;
            let rows_n = ((y + rho + 1).min(ch) - y.saturating_sub(rho)) as f64;
            let mut s = [0.0f64; K];
            for c in &col[..(rho + 1).min(cw)] {
                for k in 0..K {
                    s[k] += c[k];
                }
            }
            for (x, o) in line.iter_mut().enumerate() {
                let n = rows_n * ((x + rho + 1).min(cw) - x.saturating_sub(rho)) as f64;
                *o = f(y * cw + x, s.map(|v| v / n), n);
                if x + rho + 1 < cw {
                    for k in 0..K {
                        s[k] += col[x + rho + 1][k];
                    }
                }
                if x >= rho {
                    for k in 0..K {
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
    out
}

/// The light's levels at slider value `color_nr` (0..=100): the smallest
/// `L` (at least one) at which the noise the pyramid's smoothing leaves on a
/// flat frame ([`smooth_share`]) is at most the display pass's share of a
/// cell's, `4 / display_samples` — the colour's `L` of the detail read
/// (rel1180 msg-e3 round 22), so the guide is smooth to the colour's support;
/// `None` when off.
pub(crate) fn levels(color_nr: f32) -> Option<u32> {
    if color_nr.is_nan() || color_nr <= 0.0 {
        return None;
    }
    let share = 4.0 / display_samples(color_nr.min(100.0));
    (1u32..).find(|&l| smooth_share(l) <= share)
}

/// The share of a cell's white noise the pyramid's smoothing to level
/// `levels` keeps on a flat frame: `Σ K_L² = (Σ k_L²)²`, `k_L` the composed
/// 1-D kernel of [`SPLINE`] ([`composed_kernel`]) — exact from the taps.
pub(crate) fn smooth_share(levels: u32) -> f64 {
    let s: f64 = composed_kernel(levels).iter().map(|w| w * w).sum();
    s * s
}

/// The composed 1-D smoothing kernel to level `levels` on an unbounded line,
/// centred (its middle entry the centre weight).
pub(crate) fn composed_kernel(levels: u32) -> Vec<f64> {
    let mut k = vec![1.0f64];
    for l in 0..levels {
        let hole = 1usize << l;
        let mut next = vec![0.0f64; k.len() + 4 * hole];
        for (i, &w) in k.iter().enumerate() {
            for (a, &h) in SPLINE.iter().enumerate() {
                next[i + a * hole] += w * h;
            }
        }
        k = next;
    }
    k
}

/// The B3 spline's taps, `(1, 4, 6, 4, 1)/16`: the light's pyramid smoothing
/// along one axis ([`Cells::read_light`]). It reproduces linear content
/// exactly, so a gradient of the light has no detail at any level.
pub(crate) const SPLINE: [f64; 5] = [1.0 / 16.0, 0.25, 0.375, 0.25, 1.0 / 16.0];

/// Place `i` of a line of `n` cells reflected into it about its end cells
/// (`−1 → 1`, `n → n − 2`), as often as it takes.
fn mirror(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n as isize - 1);
    let i = i.rem_euclid(period);
    (if i >= n as isize { period - i } else { i }) as usize
}

/// A linear operator on a line of `n` cells, banded: output `x` is
/// `Σ_j w[x][j]·input[lo[x] + j]`. The pyramid's smoothing to a level along
/// one axis, composed exactly (the line's ends included), and the entrywise
/// products of two such operators the reads' variances are taken through.
#[derive(Clone)]
struct Band {
    lo: Vec<usize>,
    w: Vec<Vec<f64>>,
}

impl Band {
    fn identity(n: usize) -> Self {
        Self { lo: (0..n).collect(), w: vec![vec![1.0]; n] }
    }

    /// One pass of the pyramid's smoothing at hole `2^level` after `self`:
    /// row `x` is `Σₐ hₐ·self[mirror(x + a·2^level)]`.
    fn smoothed(&self, level: u32) -> Self {
        let n = self.lo.len();
        let hole = 1isize << level;
        let (mut lo, mut w) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for x in 0..n {
            let rows = [-2isize, -1, 0, 1, 2].map(|a| mirror(x as isize + a * hole, n));
            let l = rows.iter().map(|&r| self.lo[r]).min().unwrap_or(0);
            let h = rows.iter().map(|&r| self.lo[r] + self.w[r].len()).max().unwrap_or(0);
            let mut row = vec![0.0f64; h - l];
            for (&r, &t) in rows.iter().zip(&SPLINE) {
                for (j, &v) in self.w[r].iter().enumerate() {
                    row[self.lo[r] - l + j] += t * v;
                }
            }
            lo.push(l);
            w.push(row);
        }
        Self { lo, w }
    }

    /// The entrywise product `self[x][x']·other[x][x']` over `x'`.
    fn product(&self, other: &Self) -> Self {
        let n = self.lo.len();
        let (mut lo, mut w) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for x in 0..n {
            let (a0, a1) = (self.lo[x], self.lo[x] + self.w[x].len());
            let (b0, b1) = (other.lo[x], other.lo[x] + other.w[x].len());
            let (l, h) = (a0.max(b0), a1.min(b1));
            if h <= l {
                lo.push(0);
                w.push(Vec::new());
                continue;
            }
            lo.push(l);
            w.push((l..h).map(|p| self.w[x][p - a0] * other.w[x][p - b0]).collect());
        }
        Self { lo, w }
    }

    /// The weight of output `x` on input `x'` (0 off its band).
    fn at(&self, x: usize, xp: usize) -> f64 {
        xp.checked_sub(self.lo[x]).and_then(|j| self.w[x].get(j)).copied().unwrap_or(0.0)
    }
}

/// `field` (`cw × ch`, rows of `cw`) through `x` along its rows and then `y`
/// along its columns.
fn separable(field: &[f32], cw: usize, ch: usize, x: &Band, y: &Band) -> Vec<f32> {
    let mut t = vec![0.0f32; cw * ch];
    t.par_chunks_mut(cw).zip(field.par_chunks(cw)).for_each(|(row, src)| {
        for (i, o) in row.iter_mut().enumerate() {
            *o = x.w[i].iter().zip(&src[x.lo[i]..]).map(|(w, s)| w * f64::from(*s)).sum::<f64>() as f32;
        }
    });
    let mut out = vec![0.0f32; cw * ch];
    out.par_chunks_mut(cw).enumerate().for_each(|(r, row)| {
        let mut acc = vec![0.0f64; cw];
        for (j, &wj) in y.w[r].iter().enumerate() {
            let src = &t[(y.lo[r] + j) * cw..][..cw];
            for (a, s) in acc.iter_mut().zip(src) {
                *a += wj * f64::from(*s);
            }
        }
        for (o, a) in row.iter_mut().zip(&acc) {
            *o = *a as f32;
        }
    });
    out
}

/// The light as read ([`Cells::read_light`]): per cell the read green `Ĝ`,
/// its residual noise variance `r`, the covariance its noise keeps with the
/// cell's own colour differences' (`t1`), and the levels whose detail was
/// read there (bit `l`).
pub(crate) struct Light {
    pub(crate) g: Vec<f32>,
    pub(crate) r: Vec<f32>,
    t1: Vec<f32>,
    bits: Vec<u8>,
}

/// The weight of the read light's operator on level `a`'s smoothing at a
/// cell whose read levels are `bits`, of `levels`: `Ĝ = P_L·G + Σ_{l read}
/// (P_l − P_{l+1})·G`, so `c_L = 1`, and each read level `l` adds 1 to `c_l`
/// and takes 1 from `c_{l+1}`.
fn weight(bits: u8, a: u32, levels: u32) -> f64 {
    let read = |l: u32| l < levels && bits & (1 << l) != 0;
    f64::from(u8::from(a == levels)) + f64::from(u8::from(read(a))) - f64::from(u8::from(a > 0 && read(a - 1)))
}

/// The shared noise a window's light and colour difference keep after the
/// light's read (see [`Cells::read_light`]): `t1`, the window's mean of each
/// cell's own share, less the window's mean of the whole window's share,
/// `held`, over `n²` (`n` the window's cells).
fn shared_noise(t1: f64, held: f64, n: f64) -> f64 {
    t1 - held / (n * n)
}

/// Per level `a` of `0..=levels`, `Σ_{j,k∈Ω} P_a(j, k)` on a whole window of
/// half side `rho`: the mass of level `a`'s smoothing a window keeps of
/// itself, `(Σ_δ k_a(δ)·(n − |δ|))²` ([`shared_noise`]'s `held`).
fn window_mass(rho: usize, levels: u32) -> Vec<f64> {
    let n = (2 * rho + 1) as f64;
    (0..=levels)
        .map(|a| {
            let k = composed_kernel(a);
            let mid = (k.len() / 2) as f64;
            k.iter().enumerate().map(|(j, w)| w * (n - (j as f64 - mid).abs()).max(0.0)).sum::<f64>().powi(2)
        })
        .collect()
}

/// Is a window's slope read? Where the read light varies over the window
/// beyond what its own residual noise makes there, at the frame's fairness
/// `k`: `var_Ω(Ĝ) − r̄_Ω > k·r̄_Ω·√(2/ν)` (`var` the window's light variance,
/// `r` its mean residual, `ν` [`window_dof`]); nothing otherwise, and the
/// window's line is its box mean. On a flat frame the read light is its own
/// noise smoothed, small and coherent over the window, and its slope a ratio
/// of two small numbers: read where evidence, nothing otherwise (rel1180
/// RULING r24).
fn slope_read(var: f64, r: f64, k: f64, nu: f64) -> bool {
    var - r > k * r * (2.0 / nu).sqrt()
}

/// The effective number of independent samples `ν` of a window's light
/// variance on a flat frame, and the share `μ` of the light's residual a
/// window's variance holds in the mean, for the window of half side `rho`
/// and the light read to `levels`: the read light there is white noise
/// through the composed smoothing `K_L` ([`composed_kernel`]), whose
/// correlation between cells `δ` apart is `ρ(δ) = R(δx)·R(δy)`, `R` the 1-D
/// kernel's autocorrelation over its centre value. A window's population
/// variance is `Q = gᵀ·M·g/N` (`M = I − J/N`); with `C` the correlation over
/// the window, `E[Q] = r·tr(CM)/N` and `var(Q) = 2·r²·tr((CM)²)/N²`, so
/// `μ = tr(CM)/N` and, as a variance of `ν` independent samples
/// (Satterthwaite), `ν = tr(CM)²/tr((CM)²)` — from the taps, one value per
/// window and levels. `C` is separable (`C = Cx ⊗ Cy`, one `n × n` Toeplitz
/// matrix `T` a side), so `tr(CM) = tr(T)² − (1ᵀT1)²/N` and
/// `tr((CM)²) = tr(T²)² − 2·(1ᵀT²1)²/N + (1ᵀT1)⁴/N²`.
pub(crate) fn window_dof(rho: usize, levels: u32) -> (f64, f64) {
    let k = composed_kernel(levels);
    // `R(d)`, the kernel's autocorrelation at lag `d` (0 past its length).
    let auto = |d: usize| -> f64 { k.iter().zip(k.get(d..).unwrap_or(&[])).map(|(a, b)| a * b).sum() };
    let r0 = auto(0);
    let n = 2 * rho + 1;
    let t: Vec<Vec<f64>> = (0..n).map(|j| (0..n).map(|m| auto(j.abs_diff(m)) / r0).collect()).collect();
    let tr = (0..n).map(|j| t[j][j]).sum::<f64>();
    let one = t.iter().flatten().sum::<f64>();
    let t2: Vec<Vec<f64>> = (0..n).map(|j| (0..n).map(|m| (0..n).map(|q| t[j][q] * t[q][m]).sum()).collect()).collect();
    let tr2 = (0..n).map(|j| t2[j][j]).sum::<f64>();
    let one2 = t2.iter().flatten().sum::<f64>();
    let big_n = (n * n) as f64;
    let tcm = tr * tr - one * one / big_n;
    let tcm2 = tr2 * tr2 - 2.0 * one2 * one2 / big_n + one.powi(4) / (big_n * big_n);
    (tcm * tcm / tcm2, tcm / big_n)
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
        Some(Self { cw, ch, pw, ph, d, sd, cov, origin: [sites[0], sites[3]], model: *model, wb, side: 1 })
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
        Self { cw, ch, pw, ph, d, sd, cov, origin: self.origin, model: self.model, wb: self.wb, side: self.side * s }
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
    #[allow(dead_code)] // the pixel rebuild's slope at the cell's own level reads it ((b), rel1180 RULING r20)
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

    /// The light's noise at cell `i`'s OWN level (the test's noise is the
    /// hypothesis's, the cell's own level: S1): the variance `v` of its green
    /// `G` (the mean of `s²` cells' two green sites at a level of side `s`,
    /// `σ²_G/(2s²)`), and the covariance its noise shares with the cell's own
    /// colour differences and with its neighbours' — a colour site's green
    /// `G_R` is the mean of four green sites, two of them the cell's own and
    /// two its neighbours': `cov(G_i, d_i) = −σ²_G·(2s − 1)/(4s³)` and, summed
    /// over the cells whose green a cell's difference reads,
    /// `−σ²_G/(4s³)` (see the module). The same for both differences.
    fn light_noise(&self, i: usize) -> [f64; 3] {
        let (cx, cy) = (i % self.cw, i / self.cw);
        let s = self.side as f64;
        let r = self.model.radius((cx as f64 + 0.5) * 2.0 * s, (cy as f64 + 0.5) * 2.0 * s);
        let vg = self.model.var(f64::from(self.d[i][2]), r) * self.wb[1] * self.wb[1];
        [vg / (2.0 * s * s), -vg * (2.0 * s - 1.0) / (4.0 * s * s * s), -vg / (4.0 * s * s * s)]
    }

    /// The light read first on its own evidence (see the module): the cells'
    /// green through `levels` levels of the à-trous [`SPLINE`] pyramid, each
    /// detail coefficient read where it is resolved beyond its own noise at
    /// `k` (`|w_l| > k·σ_{w_l}`, `σ²_{w_l}` exact from the taps and each
    /// cell's light variance at its own level, [`Self::light_noise`]; the
    /// cells' greens share no site, so no pair term) and nothing otherwise;
    /// `Ĝ = s_L + Σ read w_l`. Its residual noise per cell is exact from the
    /// taps: `Ĝ_i = Σ_a c_a(i)·(P_a·G)_i` ([`weight`]), so
    /// `r_i = Σ_{a,b} c_a·c_b·((P_a∘P_b)⊗(P_a∘P_b)·v)_i`; and the covariance
    /// its noise keeps with the cell's own colour differences', the shared
    /// noise that survives the read: `Σ_a c_a·(P_a(i, i)·κ_own + P_a(i, i±1)·κ_cross)`
    /// (the cell's own greens and one green of each of two axis neighbours).
    fn read_light(&self, levels: u32, k: f64) -> Light {
        let (cw, ch) = (self.cw, self.ch);
        let noise: Vec<[f64; 3]> = (0..cw * ch).into_par_iter().map(|i| self.light_noise(i)).collect();
        let v: Vec<f32> = noise.iter().map(|q| q[0] as f32).collect();
        let (mut px, mut py) = (vec![Band::identity(cw)], vec![Band::identity(ch)]);
        for l in 0..levels {
            let (x, y) = (px[l as usize].smoothed(l), py[l as usize].smoothed(l));
            px.push(x);
            py.push(y);
        }
        let var_of = |a: usize, b: usize| separable(&v, cw, ch, &px[a].product(&px[b]), &py[a].product(&py[b]));
        let mut smooth: Vec<f32> = self.d.iter().map(|q| q[2]).collect();
        let mut g = vec![0.0f32; cw * ch];
        let mut bits = vec![0u8; cw * ch];
        let mut own = v.clone();
        for l in 0..levels as usize {
            let next = separable(&smooth, cw, ch, &Band::identity(cw).smoothed(l as u32), &Band::identity(ch).smoothed(l as u32));
            let (cross, own_next) = (var_of(l, l + 1), var_of(l + 1, l + 1));
            g.par_iter_mut()
                .zip(bits.par_iter_mut())
                .enumerate()
                .for_each(|(i, (o, b))| {
                    let w = smooth[i] - next[i];
                    let var = f64::from(own[i]) - 2.0 * f64::from(cross[i]) + f64::from(own_next[i]);
                    if f64::from(w).abs() > k * var.max(0.0).sqrt() {
                        *o += w;
                        *b |= 1 << l;
                    }
                });
            own = own_next;
            smooth = next;
        }
        g.par_iter_mut().zip(&smooth).for_each(|(o, s)| *o += s);
        let mut r = vec![0.0f64; cw * ch];
        for a in 0..=levels {
            for b in a..=levels {
                let f = var_of(a as usize, b as usize);
                let m = if a == b { 1.0 } else { 2.0 };
                r.par_iter_mut().enumerate().for_each(|(i, ri)| {
                    let c = weight(bits[i], a, levels) * weight(bits[i], b, levels);
                    if c != 0.0 {
                        *ri += m * c * f64::from(f[i]);
                    }
                });
            }
        }
        let t1 = (0..cw * ch)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % cw, i / cw);
                let nx = if x + 1 < cw { x + 1 } else { x.saturating_sub(1) };
                let ny = if y + 1 < ch { y + 1 } else { y.saturating_sub(1) };
                let [_, own_k, cross_k] = noise[i];
                (0..=levels)
                    .map(|a| {
                        let c = weight(bits[i], a, levels);
                        let (bx, by) = (&px[a as usize], &py[a as usize]);
                        let side = (bx.at(x, nx) * by.at(y, y) + bx.at(x, x) * by.at(y, ny)) / 2.0;
                        c * (bx.at(x, x) * by.at(y, y) * own_k + side * cross_k)
                    })
                    .sum::<f64>() as f32
            })
            .collect();
        Light { g, r: r.into_iter().map(|x| x as f32).collect(), t1, bits }
    }

    /// The cells' two colour differences read in two steps (see the module):
    /// the light is read first on its own evidence ([`Self::read_light`],
    /// `levels` levels at the frame's fairness `k_F`, [`fair_k`] of the
    /// cells' count); the colour is read through the light that was read:
    /// per window `Ω` of half side `rho`,
    /// `a_Ω = (cov_Ω(Ĝ, d) − κ_Ω)/(var_Ω(Ĝ) + ε_Ω)`, `b_Ω = d̄ − a_Ω·Ĝ̄`,
    /// `ε_Ω` the mean of the guide's residual `r` over `Ω` and `κ_Ω` the
    /// shared noise that survives the read; per cell `d̂ = ā·Ĝ + b̄`.
    fn guided(&self, rho: usize, levels: u32) -> Vec<[f32; 2]> {
        let light = self.read_light(levels, fair_k((self.cw * self.ch) as f64));
        self.guided_on(rho, levels, &light)
    }

    /// The colour read through `light` ([`Self::guided`]): a window's slope is
    /// read where the read light carries content beyond its own noise at the
    /// frame's fairness ([`slope_read`], `ν` of [`window_dof`]), and is 0
    /// otherwise — the window's line is then its box mean.
    fn guided_on(&self, rho: usize, levels: u32, light: &Light) -> Vec<[f32; 2]> {
        let (cw, ch) = (self.cw, self.ch);
        let k = fair_k((cw * ch) as f64);
        let nu = window_dof(rho, levels).0;
        let mass = window_mass(rho, levels);
        let lines: Vec<[f32; 4]> = box_means(
            (cw, ch),
            rho,
            |i| {
                let [d0, d1, _] = self.d[i].map(f64::from);
                let g = f64::from(light.g[i]);
                let [t1, held] = self.shared_terms(light, i, levels, &mass);
                [g, g * g, d0, d1, g * d0, g * d1, f64::from(light.r[i]), t1, held]
            },
            |_, [g, gg, d0, d1, gd0, gd1, r, t1, held], n| {
                let var = (gg - g * g).max(0.0);
                let den = var + r;
                let kappa = shared_noise(t1, held, n);
                let read = den > 0.0 && slope_read(var, r, k, nu);
                let a = [gd0 - g * d0, gd1 - g * d1].map(|c| if read { (c - kappa) / den } else { 0.0 });
                [a[0] as f32, a[1] as f32, (d0 - a[0] * g) as f32, (d1 - a[1] * g) as f32]
            },
        );
        box_means((cw, ch), rho, |i| lines[i].map(f64::from), |i, [a0, a1, b0, b1], _| {
            let g = f64::from(light.g[i]);
            [(a0 * g + b0) as f32, (a1 * g + b1) as f32]
        })
    }

    /// Cell `i`'s two terms of the shared noise ([`shared_noise`]) under
    /// `light`: its own share `t1`, and its light covariance with the cells
    /// whose green its differences read times the mass of its read levels a
    /// window keeps ([`window_mass`] `mass`).
    fn shared_terms(&self, light: &Light, i: usize, levels: u32, mass: &[f64]) -> [f64; 2] {
        let [_, own, cross] = self.light_noise(i);
        let held: f64 = (0..=levels).map(|a| weight(light.bits[i], a, levels) * mass[a as usize]).sum();
        [f64::from(light.t1[i]), (own + cross) * held]
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
