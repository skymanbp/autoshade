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
//! cells at that level, `√2` times one cell's `σ`, per colour difference, the
//! red and blue predicted from the green fit); each next pass is anchored on
//! the last estimate, a mean of `n` cells, against which a sample differs by
//! `σ·√(1 + 1/n)`, until an estimate repeats. That fixed point is the mean of
//! the window's samples within `t` of itself — the skipped mean, whose share
//! of one sample's noise is `1 / (N·(Z − 2t·φ(t)))` (`t` in `σ`, `Z` the
//! normal mass within `±t`; its influence function), which falls to the
//! window mean's `1/N` as `k` grows. A neighbour further from the estimate than
//! the noise can carry is scene, and does not vote.
//!
//! **γ = 1.** In linear light the noise is the only scale at a pixel; a
//! threshold that is a multiple of `σ(L)` is the only form that does not
//! introduce a scale of its own. (The `σ^0.5` that v1.17.1's round 3
//! predicted was measured on the display frame, where the encode bends the
//! scale; it does not apply in the linear domain.)
//!
//! **k follows the photo** ([`photo_k`]). It is read where the stop is
//! visible, on texture: over the photo's own textured patches (the flat
//! rule's complement, [`NoiseModel::is_flat`]) the filter removes the noise
//! and nothing more while the edge energy it takes away (the squared cell
//! differences of both colour differences) stays within the edge energy the
//! noise model attributes to noise there; `k` is where the two meet. A flat
//! patch cannot say where to stop — more smoothing is always closer to the
//! truth there — so the flat patches are the CHECK of `k`, never its source.
//! It is one law, not one constant: on the most textured 1000 px windows of
//! two ordinary scenes the meeting point is 2.62 and 3.69 (rel1180 msg-e3
//! round 4), further apart than the model's own 2–8 % error on the noise
//! can move it; over all their textured patches, with the estimate-anchored
//! test and the window below, the law reads 6.21 and 13.61, and 1.43 on a
//! night sky (round 9; at round 8's 3-cell window 5.86, 10.36, 1.63: the
//! crossing moves with the window, the law is the same). A photo with no texture has nothing to
//! protect, and its `k` is unbounded: the filter is then the plain mean of
//! its window, and the window is the bound.
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
//! pass's `N/4` ([`window`]; `t` the width the stage runs at, so it follows
//! the photo): the same on daylight, where `t` is wide and `Z − 2t·φ(t)` is 1
//! (7 cells at 25), wider on a night sky (11 cells at its `k` 1.43, rel1180
//! msg-e3 round 10). The display pass's reach (its radius law) stays that
//! pass's own, for the sources it still runs on. `k` itself is read over the
//! plain mean's window at the default strength ([`DEFAULT_AMOUNT`]) — the one
//! window that does not depend on the reading — so it is one reading per
//! photo. That reading and the noise model are the exposure's: read ONCE,
//! off the sensor's own mosaic ([`SourceReading`]), and kept with the source,
//! so a develop runs the pass alone; a mosaic another process has denoised (a
//! master's AI denoise) is filtered at the sensor's model, an upper bound of
//! the noise left in it.
//!
//! **A preview** ([`preview_level`]): white noise averages the same at any
//! scale, so the wide window is taken where it is cheap — a develop capped at
//! a preview's long edge runs the pass on the cell pyramid's level whose cell
//! is no larger than one of its pixels (`σ/2^L` there, the base window);
//! an export and a master's bake run it at sensor scale.
//!
//! **The slider.** "Colour noise reduction" scales `k` linearly and sets the
//! window as above: 0 is off, the default strength (Lightroom's own 25) is the
//! photo's own `k` over (on daylight) 7 cells, 100 is four times it over 23. Below 25 the
//! display pass blends its result with the input and keeps more than one
//! cell's worth of noise; the stage's window is then its least, one cell.
//! Where `k` is unbounded (no texture) the scale cannot weaken it, and the
//! window does: at 10 the mean runs over 1 cell, at 25 over 7, so the slider
//! is never dead. Detail and Smoothness belong to the display-frame pass,
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

/// The slider value at which the stage runs at the photo's own `k`:
/// Lightroom's own default for Color.
pub(crate) const DEFAULT_AMOUNT: f32 = 25.0;

/// The multiple of the photo's `k` the slider value `color_nr` (0..=100)
/// asks for: `None` when off (the colour-NR control's own rule,
/// `ChromaNrParams::global`).
pub(crate) fn scale_of(color_nr: f32) -> Option<f64> {
    (color_nr > 0.0).then(|| f64::from(color_nr.min(100.0)) / f64::from(DEFAULT_AMOUNT))
}

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
/// ([`display_samples`] pixels, four to a cell). The window `k` is read over.
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

/// What the stage reads of an exposure: its noise model and its own `k`
/// ([`photo_k`], at the default strength). Both are properties of the
/// exposure, not of a develop or a recipe: they are read ONCE, off the
/// sensor's own mosaic ([`read_source`]), and carried (the develop keeps them
/// with the source, `render::SourceFrame::noise_reading`), so a develop runs
/// the pass alone. The model is the sensor's: a mosaic another process has
/// already denoised (a master's AI denoise) does not carry the sensor's
/// Poisson noise, so `var = g·a·m + g²·b` is not its law and a model fitted
/// on it is not a measurement. Run over such a mosaic, the stage's test reads
/// the sensor's `σ` as an UPPER bound of the noise left in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SourceReading {
    pub(crate) model: NoiseModel,
    /// The photo's own `k` at the default strength (`f64::INFINITY` when it
    /// has no texture to protect).
    pub(crate) k: f64,
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
    let model = NoiseModel::fit(&pts, width, height)?;
    let cells = Cells::of(mosaic, width, height, sites, wb, &model, &pts)?;
    let k = photo_k(&cells, &model, &pts, wb, plain_window(DEFAULT_AMOUNT)).0;
    Some(SourceReading { model, k })
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
/// `scale` times the photo's own `k` over a window of half side `rho(k)`
/// cells for the `k` it runs at ([`window`]). The model and the `k` are the
/// exposure's `source` reading ([`SourceReading`], read once off the sensor's
/// own mosaic), or, without one, read off `mosaic` itself ([`read_source`]:
/// `mosaic` is then the sensor's). Returns the reading, the `k` it ran at
/// and the filtered cell chroma the demosaiced pixels are rebuilt on
/// ([`CellChroma::rebuild`]), or `None` — and the mosaic untouched — when the
/// pattern is not a 2×2 RGB Bayer or the frame has too few usable patches for
/// a model. The filtered samples are written back into `mosaic` only when
/// `write` (a develop that does not demosaic): a demosaic reads the sensor's
/// own mosaic and takes its colour from the rebuild. At `level` > 0
/// ([`preview_level`]) the pass runs on that level of
/// the cell pyramid ([`Cells::level`], the base window there) and each cell
/// takes the level's filtered chroma read bilinearly at its centre.
#[allow(clippy::too_many_arguments)] // the mosaic's frame plus the stage's strength, window, the exposure's reading and the write
pub(crate) fn filter_mosaic(
    mosaic: &mut [f32],
    width: usize,
    height: usize,
    cfa: &rawler::cfa::CFA,
    wb: [f64; 3],
    scale: f64,
    rho: impl Fn(f64) -> usize,
    source: Option<&SourceReading>,
    level: u32,
    write: bool,
) -> Option<(SourceReading, f64, CellChroma)> {
    let sites = bayer_sites(cfa)?;
    if mosaic.len() != width * height || wb[1].is_nan() || wb[1] <= 0.0 || scale.is_nan() || scale <= 0.0 {
        return None;
    }
    let wb = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    let pts = patches(mosaic, width, height, sites, wb);
    let (model, cells, k) = match source {
        Some(r) => (r.model, Cells::of(mosaic, width, height, sites, wb, &r.model, &pts)?, r.k),
        None => {
            let model = NoiseModel::fit(&pts, width, height)?;
            let cells = Cells::of(mosaic, width, height, sites, wb, &model, &pts)?;
            let k = photo_k(&cells, &model, &pts, wb, plain_window(DEFAULT_AMOUNT)).0;
            (model, cells, k)
        }
    };
    let reading = SourceReading { model, k };
    let k = k * scale;
    let rho = rho(k);
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
    Some((reading, k, chroma))
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

/// The rungs `photo_k` reads the removed edge energy on: `2^(j/2)`,
/// `j = 0..LADDER` (1 to 64, half an octave apart — a resolution: the
/// crossing is interpolated between rungs).
const LADDER: usize = 13;

/// The textured patches [`photo_k`] reads (the flat rule's complement): a
/// reading summed over the frame's patches, as the noise model's fit is, read
/// at the fit's own subsample rule (`noise_model::fit_stride`: the uniform
/// stride that keeps at least `FIT_PATCHES`, every one on a frame with
/// fewer). The reading is a reading, not the filter: it costs the filter's
/// window on the patches it reads, so it reads no more of them than the
/// model's own resolution needs.
fn reading_patches(model: &NoiseModel, pts: &[Patch], wb: [f64; 3]) -> Vec<usize> {
    let textured: Vec<usize> = (0..pts.len()).filter(|&i| pts[i].usable && !model.is_flat(&pts[i], wb)).collect();
    let stride = super::noise_model::fit_stride(textured.len());
    textured.into_iter().step_by(stride).collect()
}

/// The photo's own `k`: the threshold (in `σ_d`) at which the edge energy the
/// filter removes from the photo's TEXTURED patches meets the noise model's
/// share of that energy (see the module), linear between the rungs of
/// [`LADDER`]; `f64::INFINITY` when no textured patch exists or the removed
/// energy never reaches the share. Also returns, per rung read,
/// `(k, removed / share)` — the reading itself.
pub(crate) fn photo_k(cells: &Cells, model: &NoiseModel, pts: &[Patch], wb: [f64; 3], rho: usize) -> (f64, Vec<(f64, f64)>) {
    let textured = reading_patches(model, pts, wb);
    if textured.is_empty() {
        return (f64::INFINITY, Vec::new());
    }
    let (cw, ch, pw) = (cells.cw, cells.ch, cells.pw);
    let origin = |p: usize| ((p % pw) * PATCH, (p / pw) * PATCH);
    // A patch's forward differences (to the right and below, inside the
    // frame): each pairs two neighbouring cells.
    let neighbours = move |cx: usize, cy: usize| {
        [(cx + 1 < cw).then(|| cy * cw + cx + 1), (cy + 1 < ch).then(|| (cy + 1) * cw + cx)].into_iter().flatten()
    };
    // Each cell's value is read once per patch (the patch and the row and
    // column its forward differences reach), not once per difference it is in.
    let energy = |value: &(dyn Fn(usize) -> [f32; 2] + Sync)| -> f64 {
        textured
            .par_iter()
            .map(|&p| {
                let (x0, y0) = origin(p);
                let (x1, y1) = ((x0 + PATCH + 1).min(cw), (y0 + PATCH + 1).min(ch));
                let bw = x1 - x0;
                let at: Vec<[f32; 2]> = (y0..y1).flat_map(|cy| (x0..x1).map(move |cx| cy * cw + cx)).map(value).collect();
                let mut e = 0.0f64;
                for cy in y0..y0 + PATCH {
                    for cx in x0..x0 + PATCH {
                        let c = at[(cy - y0) * bw + cx - x0];
                        for n in neighbours(cx, cy) {
                            let q = at[(n / cw - y0) * bw + n % cw - x0];
                            e += f64::from(q[0] - c[0]).powi(2) + f64::from(q[1] - c[1]).powi(2);
                        }
                    }
                }
                e
            })
            .sum()
    };
    // The noise's share: each difference carries both cells' variance less
    // the green they share, at the patch's level (`2·(v − c)` per difference
    // and colour difference, `2v = σ_d²`).
    let share: f64 = textured
        .iter()
        .map(|&p| {
            let [sr, sb] = cells.sd[p].map(f64::from);
            let c = f64::from(cells.cov[p]);
            let (x0, y0) = origin(p);
            let diffs: usize =
                (y0..y0 + PATCH).flat_map(|cy| (x0..x0 + PATCH).map(move |cx| neighbours(cx, cy).count())).sum();
            (sr * sr + sb * sb - 4.0 * c) * diffs as f64
        })
        .sum();
    let e_in = energy(&|i| [cells.d[i][0], cells.d[i][1]]);
    let mut read: Vec<(f64, f64)> = Vec::with_capacity(LADDER);
    for j in 0..LADDER {
        let k = 2f64.powf(j as f64 / 2.0);
        let removed = (e_in - energy(&|i| cells.filtered(i, k, rho))) / share;
        read.push((k, removed));
        if removed >= 1.0 {
            let k = match read.len() {
                1 => k,
                n => {
                    let ((k0, r0), (k1, r1)) = (read[n - 2], read[n - 1]);
                    k0 + (1.0 - r0) * (k1 - k0) / (r1 - r0)
                }
            };
            return (k, read);
        }
    }
    (f64::INFINITY, read)
}

/// The sigma filter's estimate of the true value of `centre` (one sample of
/// noise sd `sigma`) over `window` (its samples, the centre among them):
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

/// One worker's buffers for the pass: the window per colour difference and
/// the estimates seen.
#[derive(Default)]
struct Scratch {
    window: [Vec<f32>; 2],
    seen: Vec<u32>,
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
        Some(Self { cw, ch, pw, ph, d, sd, cov, origin: [sites[0], sites[3]] })
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
        Self { cw, ch, pw, ph, d, sd, cov, origin: self.origin }
    }

    /// Cell `i`'s two colour differences through the sigma filter at `k`:
    /// the test is anchored on an estimate of the cell's true colour, not on
    /// its own noisy sample (see the module).
    fn filtered(&self, i: usize, k: f64, rho: usize) -> [f32; 2] {
        self.filtered_counted(i, k, rho).0
    }

    /// [`Self::filtered`] with the count of window samples the last pass accepted.
    fn filtered_counted(&self, i: usize, k: f64, rho: usize) -> ([f32; 2], [u32; 2]) {
        self.filtered_in(i, k, rho, &mut Scratch::default())
    }

    /// [`Self::filtered_counted`] in a worker's reused buffers: the window's
    /// samples gathered once per centre, both colour differences in one sweep.
    fn filtered_in(&self, i: usize, k: f64, rho: usize, scratch: &mut Scratch) -> ([f32; 2], [u32; 2]) {
        let (cw, ch) = (self.cw, self.ch);
        let (cx, cy) = (i % cw, i / cw);
        let sd = self.sd[(cy / PATCH).min(self.ph - 1) * self.pw + (cx / PATCH).min(self.pw - 1)];
        let (y0, y1, x0, x1) = (cy.saturating_sub(rho), (cy + rho + 1).min(ch), cx.saturating_sub(rho), (cx + rho + 1).min(cw));
        let Scratch { window: [w0, w1], seen } = scratch;
        w0.clear();
        w1.clear();
        for y in y0..y1 {
            for q in &self.d[y * cw + x0..y * cw + x1] {
                w0.push(q[0]);
                w1.push(q[1]);
            }
        }
        let (mut out, mut count) = ([0.0f32; 2], [0u32; 2]);
        for (j, window) in [&*w0, &*w1].into_iter().enumerate() {
            // One cell's sd is `σ_d / √2`.
            (out[j], count[j]) = skipped_mean(window, self.d[i][j], f64::from(sd[j]) / 2f64.sqrt(), k, seen);
        }
        (out, count)
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
