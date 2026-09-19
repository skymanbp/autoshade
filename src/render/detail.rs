//! Lightroom's Detail panel, rendered (v1.5.0).
//!
//! Until v1.5.0 eight of the panel's eleven controls reached the sidecar and
//! moved no pixel here: policy SF4-C (R25 B3) kept Adobe-only operators out of
//! the engine. The user revoked that policy on 2026-09-17 ("implement every
//! control we can set and do not render; slight deviation from Lightroom is
//! allowed, compatibility is the aim"), so this module renders all of them:
//!
//! * capture sharpening — Amount × Radius × Detail × Masking ([`sharpen`]);
//! * luminance noise reduction — Luminance × Detail × Contrast ([`luma_nr`]);
//! * colour noise reduction — Color × Detail × Smoothness ([`chroma_nr`]).
//!
//! Adobe has not published these operators. Each one is built from what Adobe
//! DOES document about its sliders (which way a result moves when a slider
//! moves), in the engine's gamma-encoded working domain, and every free
//! constant is named: the Lightroom kit's ladders (`SH-*`, `NR-*`, `CNR-*` in
//! `autoshade-lr-kit-v150/pack-spec.json`) exist to pin them against exported
//! pixels. Until that measurement lands, the constants are first-principles
//! values and say so.
//!
//! # Film pixels
//!
//! Lightroom states Radius, the noise-reduction neighbourhoods and grain in
//! pixels of the FULL-RESOLUTION image. This engine develops a working raster
//! that is the full frame on export and a downscaled copy everywhere a person
//! is looking (the GUI preview, the web preview, a capped render), so every
//! operator here converts film pixels to raster pixels through [`FilmScale`]
//! instead of reading a length off the raster. A preview therefore shows what
//! the export will look like after downscaling — which, for a one-pixel
//! sharpening radius on a 61 MP frame viewed at 1280 px, is very little.
//! Lightroom makes the same point in its own panel ("zoom to 100% for a more
//! accurate view").

use rayon::prelude::*;

use super::{
    bilinear_plane, box_blur_h, box_blur_v, gauss_blur_plane, luma601, neighbours4, smoothstep,
    write_luma_weighted,
};
use crate::recipe::EditRecipe;

/// How many full-resolution ("film") pixels one pixel of the raster being
/// developed stands for: 1 on a full-resolution export, the downscale ratio on
/// a preview or a capped render. Never below 1 — nothing develops a raster
/// LARGER than its own source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilmScale(f32);

impl FilmScale {
    /// The raster IS the film: every length in film pixels is a length in
    /// raster pixels. What a caller that knows nothing about the source's full
    /// size gets, and exactly right for the analysis surfaces (the reverse fit
    /// works on its own 384 px raster and compares like with like).
    pub const NATIVE: FilmScale = FilmScale(1.0);

    /// The scale of a `raster_w × raster_h` working raster developed from a
    /// source whose full-resolution SHORT edge is `film_short_edge` pixels.
    /// `None`, a zero raster, or a source no larger than the raster → native.
    pub fn of(film_short_edge: Option<u32>, raster_w: usize, raster_h: usize) -> FilmScale {
        let raster_short = raster_w.min(raster_h);
        match film_short_edge {
            Some(film) if raster_short > 0 && film as usize > raster_short => {
                FilmScale(film as f32 / raster_short as f32)
            }
            _ => FilmScale::NATIVE,
        }
    }

    /// Film pixels per raster pixel (≥ 1).
    pub fn factor(self) -> f32 {
        self.0
    }

    /// A length stated in film pixels, in raster pixels.
    pub fn raster_px(self, film_px: f32) -> f32 {
        film_px / self.0
    }
}

/// A slider-driven quantity that moves linearly: its value at slider 0 and at
/// slider 100. Every constant below that depends on a slider is one of these,
/// so each reads as "from … to …" and a kit measurement replaces two numbers.
///
/// Visible to the whole `render` module since v1.5.0's Effects pass
/// (`render/finish.rs`), whose vignette and grain constants are the same kind
/// of provisional "from … to …" and would otherwise be a second copy of it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Ramp(pub(super) f32, pub(super) f32);

impl Ramp {
    /// The value at `t` (the slider ÷ 100).
    pub(super) fn at(self, t: f32) -> f32 {
        self.0 + (self.1 - self.0) * t
    }
}

// ── capture sharpening ───────────────────────────────────────────────────────

/// Lightroom's Radius band is 0.5..=3.0 film pixels; a stored radius below it
/// (the recipe's "absent" 0 resolves to 1.0 before it gets here) is lifted.
const SHARPEN_RADIUS_MIN: f32 = 0.5;

/// The smallest σ, in raster pixels, the Gaussian is actually sampled at. Below
/// ~0.6 px a sampled kernel stops being the continuous Gaussian it names (the
/// observation `TEXTURE_MIN_SIGMA_PX` records for Texture), so a smaller true
/// σ runs at this one with its amount scaled by the ratio of the two transfers
/// at the raster's Nyquist frequency ([`usm_transfer`]) — what the export's
/// sharpening still contributes once it is downscaled to this raster.
const SHARPEN_MIN_SIGMA_PX: f32 = 0.6;

/// The shaping laws, all provisional until the kit pins them:
///
/// * `gain` — Amount 100 lifts this many times the unsharp detail signal
///   (`SH-40`, `SH-100`);
/// * `halo` — the soft limiter's amplitude `L` (gamma-encoded luma) over
///   Detail: Adobe documents Detail as halo suppression, so the boost goes
///   through `L·tanh(boost/L)` with `L` a small floor at Detail 0 and no
///   practical limit at Detail 100 (`SH-100-D0`, quadratic in between);
/// * `fine` — the weight of the finest band (a 4-neighbour Laplacian), the
///   "higher Detail brings out texture" half of the description (`SH-100-D100`);
/// * `edge` — the blurred-luma gradient, per FILM pixel, at which Masking
///   opens fully; Masking 0 applies everywhere (`SH-100-M50`, `SH-100-M100`).
struct SharpenLaw {
    gain: f32,
    halo: Ramp,
    fine: Ramp,
    edge: Ramp,
}

const SHARPEN: SharpenLaw =
    SharpenLaw { gain: 1.0, halo: Ramp(0.02, 1.0), fine: Ramp(0.0, 0.5), edge: Ramp(0.0, 0.04) };

/// The Sharpening sliders, resolved: `amount` is the slider ÷ 100 (negative
/// only for a mask's local Sharpness), `radius` in film pixels, `detail` and
/// `masking` 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SharpenParams {
    pub amount: f32,
    pub radius: f32,
    pub detail: f32,
    pub masking: f32,
}

impl SharpenParams {
    /// The global stage, or `None` at Amount 0.
    pub(crate) fn global(r: &EditRecipe) -> Option<Self> {
        (r.sharpening > 0.0).then(|| Self::at_amount(r, r.sharpening))
    }

    /// The recipe's Radius / Detail / Masking at a caller's own amount (the
    /// slider value, -100..=150). A mask's local Sharpness rides the GLOBAL
    /// shaping axes — the only ones Lightroom has.
    pub(crate) fn at_amount(r: &EditRecipe, amount: f32) -> Self {
        SharpenParams {
            amount: amount / 100.0,
            radius: r.resolved("sharpen_radius").max(SHARPEN_RADIUS_MIN),
            detail: (r.resolved("sharpen_detail") / 100.0).clamp(0.0, 1.0),
            masking: (r.sharpen_mask / 100.0).clamp(0.0, 1.0),
        }
    }
}

/// `1 − Ĝ_σ(f)` at the raster's Nyquist frequency (f = ½ cycle per pixel): how
/// much of the finest detail the raster can hold an unsharp mask of σ lifts.
fn usm_transfer(sigma: f32) -> f32 {
    let pi = std::f32::consts::PI;
    1.0 - (-2.0 * pi * pi * sigma * sigma * 0.25).exp()
}

/// Capture sharpening on luma (chroma-preserving, like every detail pass in
/// this engine). `weight(x, y, px)` is the mask arm's coverage; the global
/// stage passes 1.
///
/// Per pixel, with `l` the luma and `G_σ` a Gaussian of the Radius:
///
/// ```text
///   signal = (l − G_σ∗l) + fine(detail)·(l − mean₄(l))
///   boost  = amount·gain·signal          (then  L·tanh(boost/L), L = halo(detail²))
///   l'     = l + boost·edge_mask·weight
/// ```
///
/// A NEGATIVE amount (a mask softening its background) skips the limiter and
/// the fine band: it is a blur toward `G_σ∗l`, which is what the old engine's
/// signed unsharp mask did and what Lightroom's local −Sharpness looks like.
pub(crate) fn sharpen(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    p: &SharpenParams,
    film: FilmScale,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if w == 0 || h == 0 || p.amount == 0.0 || !p.amount.is_finite() {
        return;
    }
    let sigma_true = film.raster_px(p.radius.max(SHARPEN_RADIUS_MIN));
    let sigma = sigma_true.max(SHARPEN_MIN_SIGMA_PX);
    let fade = (usm_transfer(sigma_true) / usm_transfer(sigma)).clamp(0.0, 1.0);
    if fade < 1e-3 {
        return;
    }
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let blur = gauss_blur_plane(&luma, w, h, sigma);
    let edge = (p.masking > 0.0).then(|| edge_mask(&blur, w, h, p.masking, film));
    let softening = p.amount < 0.0;
    let k = p.amount * SHARPEN.gain * fade;
    let limit = SHARPEN.halo.at(p.detail * p.detail);
    let fine = if softening { 0.0 } else { SHARPEN.fine.at(p.detail) };
    write_luma_weighted(data, w, weight, |i, l, wgt| {
        let mut signal = l - blur[i];
        if fine > 0.0 {
            let n = neighbours4(&luma, w, h, i % w, i / w);
            signal += fine * (l - 0.25 * n.iter().sum::<f32>());
        }
        let boost = k * signal;
        let boost = if softening { boost } else { limit * (boost / limit).tanh() };
        l + boost * edge.as_ref().map_or(1.0, |e| e[i]) * wgt
    });
}

/// Masking's edge map: 0 in flat areas, 1 on edges, from the gradient of the
/// Radius-blurred luma measured per FILM pixel (so a preview and an export
/// agree on which edges are edges).
fn edge_mask(blur: &[f32], w: usize, h: usize, masking: f32, film: FilmScale) -> Vec<f32> {
    let full = SHARPEN.edge.at(masking);
    let start = 0.25 * full;
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let gx = 0.5 * (blur[y * w + (x + 1).min(w - 1)] - blur[y * w + x.saturating_sub(1)]);
            let gy = 0.5 * (blur[(y + 1).min(h - 1) * w + x] - blur[y.saturating_sub(1) * w + x]);
            let g = (gx * gx + gy * gy).sqrt() / film.factor();
            smoothstep(start, full, g)
        })
        .collect()
}

// ── luminance noise reduction ────────────────────────────────────────────────

/// Luminance NR's laws, provisional until `NR-50`, `NR-100`, `NR-50-D0`,
/// `NR-50-D100` and `NR-50-C100` pin them:
///
/// * `radius` — the neighbourhood, in film pixels, over Luminance;
/// * `noise` — the luma variation (gamma units, film scale) the smoother
///   treats as noise, over Luminance: a guided filter keeps structure whose
///   local standard deviation is well above it and flattens what is below;
/// * `keep` — Detail's multiplier on that threshold (1 at Lightroom's default
///   50): higher Detail preserves more, "but may produce noisier results".
struct LumaNrLaw {
    radius: Ramp,
    noise: Ramp,
    keep: Ramp,
}

const LUMA_NR: LumaNrLaw =
    LumaNrLaw { radius: Ramp(1.0, 4.0), noise: Ramp(0.004, 0.054), keep: Ramp(1.6, 0.4) };

/// The Luminance NR sliders, resolved to 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LumaNrParams {
    pub amount: f32,
    pub detail: f32,
    pub contrast: f32,
}

impl LumaNrParams {
    pub(crate) fn global(r: &EditRecipe) -> Option<Self> {
        (r.noise_reduction > 0.0).then(|| Self::at_amount(r, r.noise_reduction))
    }

    /// A mask's local Noise rides the global Detail / Contrast.
    pub(crate) fn at_amount(r: &EditRecipe, amount: f32) -> Self {
        LumaNrParams {
            amount: (amount / 100.0).clamp(0.0, 1.0),
            detail: (r.resolved("nr_detail") / 100.0).clamp(0.0, 1.0),
            contrast: (r.nr_contrast / 100.0).clamp(0.0, 1.0),
        }
    }
}

/// One box pass per axis — the guided filter's own mean, not the three-pass
/// Gaussian approximation `blur_plane` builds. Holds `src`, the horizontal
/// pass and the output at once.
fn box_mean(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    box_blur_v(&box_blur_h(src, w, h, radius), w, h, radius)
}

/// [`box_mean`] of a plane the caller gives up: `src` is dropped before the
/// vertical pass allocates, so the call itself never holds three planes.
fn box_mean_owned(src: Vec<f32>, w: usize, h: usize, radius: usize) -> Vec<f32> {
    let horizontal = box_blur_h(&src, w, h, radius);
    drop(src);
    box_blur_v(&horizontal, w, h, radius)
}

/// Luminance noise reduction: a self-guided filter (He et al. 2010) on luma.
/// `q = ā·l + b̄` with `a = var/(var + ε)` — where the neighbourhood's own
/// variation is far above ε (an edge, real texture) `a → 1` and the pixel
/// keeps its value; where it is far below (flat, noisy) `a → 0` and the pixel
/// takes the neighbourhood mean. ε comes from Luminance and Detail; Contrast
/// adds back the LOW-frequency part of what the filter removed, which is the
/// local contrast Adobe says that slider preserves (and the mottling it says
/// that slider risks).
///
/// ε is stated at film scale and divided by the film factor squared on a
/// downscaled raster: averaging `k × k` film pixels into one already cut the
/// noise variance by `k²`, and a threshold that did not follow would flatten
/// real texture on every preview.
///
/// MEMORY: never more than THREE f32 planes beside the frame (12 B/px — the
/// spatial-pass row of `decode::PIPELINE_BYTES_PER_PIXEL`'s accounting). The
/// squared luma is written over the luma, every source plane is handed to
/// [`box_mean_owned`], and luma is re-read off the frame rather than kept.
pub(crate) fn luma_nr(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    p: &LumaNrParams,
    film: FilmScale,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if w == 0 || h == 0 || p.amount <= 0.0 {
        return;
    }
    let r_raster = film.raster_px(LUMA_NR.radius.at(p.amount));
    if r_raster < 0.25 {
        return;
    }
    let radius = (r_raster.round() as usize).max(1);
    let noise = LUMA_NR.noise.at(p.amount) * LUMA_NR.keep.at(p.detail) / film.factor();
    let eps = noise * noise;
    let mut luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let mut b = box_mean(&luma, w, h, radius);
    luma.par_iter_mut().for_each(|v| *v *= *v);
    let mut a = box_mean_owned(luma, w, h, radius);
    a.par_iter_mut().zip(b.par_iter_mut()).for_each(|(a, b)| {
        let m = *b;
        let var = (*a - m * m).max(0.0);
        let coef = var / (var + eps);
        *a = coef;
        *b = m - coef * m;
    });
    let mut q = box_mean_owned(a, w, h, radius);
    let coef_b = box_mean_owned(b, w, h, radius);
    // q = ā·l + b̄, into ā's plane; b̄ dies before Contrast's planes exist.
    q.par_iter_mut().enumerate().for_each(|(i, v)| *v = *v * luma601(&data[i]) + coef_b[i]);
    drop(coef_b);
    let lift = (p.contrast > 0.0).then(|| {
        let residual: Vec<f32> =
            q.par_iter().enumerate().map(|(i, q)| luma601(&data[i]) - q).collect();
        box_mean_owned(residual, w, h, 2 * radius)
    });
    write_luma_weighted(data, w, weight, |i, l, wgt| {
        let target = q[i] + lift.as_ref().map_or(0.0, |r| p.contrast * r[i]);
        l + (target - l) * wgt
    });
}

// ── colour noise reduction ───────────────────────────────────────────────────

/// Colour NR's laws, provisional until `CNR-25`, `CNR-100`, `CNR-100-D0/D100`
/// and `CNR-100-S0/S100` pin them:
///
/// * `radius` — the neighbourhood in film pixels over Color: colour noise after
///   demosaic is blotchy, several pixels across, so it reaches much further
///   than the luminance one;
/// * `edge` — the luma variation (gamma units, film scale) below which colour
///   is smoothed freely and above which it follows the luma edge, over Detail
///   (Detail 0 lets colour bleed across weak edges, Detail 100 keeps them);
/// * `widen` — over Smoothness, how much wider the offset plane is smoothed:
///   the "low-frequency colour mottling" Adobe says Smoothness removes lives
///   in the guided filter's offset term;
/// * `full_at` — the Color amount at which the smoothed chroma fully replaces
///   the original (linear blend below it). Lightroom's own default, 25, is
///   already a full colour clean-up on a high-ISO RAW.
struct ChromaNrLaw {
    radius: Ramp,
    edge: Ramp,
    widen: Ramp,
    full_at: f32,
}

const CHROMA_NR: ChromaNrLaw =
    ChromaNrLaw { radius: Ramp(2.0, 16.0), edge: Ramp(0.08, 0.01), widen: Ramp(1.0, 3.0), full_at: 0.25 };

/// The Color NR sliders, resolved to 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChromaNrParams {
    pub amount: f32,
    pub detail: f32,
    pub smoothness: f32,
}

impl ChromaNrParams {
    pub(crate) fn global(r: &EditRecipe) -> Option<Self> {
        (r.color_nr > 0.0).then(|| ChromaNrParams {
            amount: (r.color_nr / 100.0).clamp(0.0, 1.0),
            detail: (r.resolved("color_nr_detail") / 100.0).clamp(0.0, 1.0),
            smoothness: (r.resolved("color_nr_smooth") / 100.0).clamp(0.0, 1.0),
        })
    }
}

/// Bilinear read of a `lw × lh` plane at the centre of full-resolution pixel
/// `(x, y)`, the plane being that frame subsampled `s×`.
fn upsample_at(plane: &[f32], lw: usize, lh: usize, s: usize, x: usize, y: usize) -> f32 {
    // The COORDINATE math is this function's own — pixel centres of the full
    // frame expressed in the subsampled plane's grid — and the four taps are
    // `bilinear_plane`'s, which clamps.
    let u = (x as f32 + 0.5) / s as f32 - 0.5;
    let v = (y as f32 + 0.5) / s as f32 - 0.5;
    bilinear_plane(plane, lw, lh, u, v)
}

/// The mean of `value` over each `s × s` block of the frame: a plane of
/// `ceil(w/s) × ceil(h/s)` cells, an edge cell averaging what it holds.
fn block_mean(
    data: &[[f32; 3]],
    w: usize,
    h: usize,
    s: usize,
    value: impl Fn(&[f32; 3]) -> f32 + Sync,
) -> Vec<f32> {
    let lw = w.div_ceil(s);
    let mut out = vec![0.0f32; lw * h.div_ceil(s)];
    out.par_chunks_mut(lw).enumerate().for_each(|(ly, row)| {
        for (lx, cell) in row.iter_mut().enumerate() {
            let (x0, y0) = (lx * s, ly * s);
            let (x1, y1) = ((x0 + s).min(w), (y0 + s).min(h));
            let sum: f32 = (y0..y1).flat_map(|y| &data[y * w + x0..y * w + x1]).map(&value).sum();
            *cell = sum / ((x1 - x0) * (y1 - y0)) as f32;
        }
    });
    out
}

/// Colour noise reduction: luma is untouched; the two colour-difference
/// planes `R − Y` and `B − Y` are rebuilt from a luma-GUIDED filter, so colour
/// is flattened inside regions and kept at the edges the luma draws.
///
/// The fast guided filter (He & Sun 2015): the filter runs on planes
/// subsampled `s×` (colour noise has no content at a pixel's scale to lose),
/// its two coefficient planes are upsampled bilinearly, and the output is
/// assembled against the FULL-resolution luma — so an edge is as sharp as the
/// luma that draws it.
///
/// MEMORY: at most EIGHT planes at `1/s²` of the frame with `s ≥ 2` — two
/// full planes' worth (8 B/px), inside the spatial-pass budget. The guide and
/// its two moments stay for both channels; each colour plane is built from
/// the frame only when its turn comes and is gone before the next one is.
pub(crate) fn chroma_nr(data: &mut [[f32; 3]], w: usize, h: usize, p: &ChromaNrParams, film: FilmScale) {
    if w == 0 || h == 0 || p.amount <= 0.0 {
        return;
    }
    let r_raster = film.raster_px(CHROMA_NR.radius.at(p.amount));
    if r_raster < 0.5 {
        return;
    }
    let s = ((r_raster / 2.5).floor() as usize).max(2).min(w.max(h));
    let (lw, lh) = (w.div_ceil(s), h.div_ceil(s));
    let radius = ((r_raster / s as f32).round() as usize).max(1);
    let edge = CHROMA_NR.edge.at(p.detail) / film.factor();
    let eps = edge * edge;
    let guide = block_mean(data, w, h, s, luma601);
    let mean_g = box_mean(&guide, lw, lh, radius);
    let mut var_g = box_mean_owned(guide.iter().map(|g| g * g).collect(), lw, lh, radius);
    var_g.iter_mut().zip(&mean_g).for_each(|(v, m)| *v = (*v - m * m).max(0.0));
    let smooth_radius = ((radius as f32 * CHROMA_NR.widen.at(p.smoothness)).round() as usize).max(radius);
    // (ā, b̄) for one colour-difference plane, which it consumes.
    let coefficients = |plane: Vec<f32>| -> (Vec<f32>, Vec<f32>) {
        let mut b = box_mean(&plane, lw, lh, radius);
        let gp: Vec<f32> = guide.iter().zip(&plane).map(|(g, v)| g * v).collect();
        drop(plane);
        let mut a = box_mean_owned(gp, lw, lh, radius);
        for i in 0..lw * lh {
            let coef = (a[i] - mean_g[i] * b[i]) / (var_g[i] + eps);
            b[i] -= coef * mean_g[i];
            a[i] = coef;
        }
        (box_mean_owned(a, lw, lh, radius), box_mean_owned(b, lw, lh, smooth_radius))
    };
    let (a_r, b_r) = coefficients(block_mean(data, w, h, s, |px| px[0] - luma601(px)));
    let (a_b, b_b) = coefficients(block_mean(data, w, h, s, |px| px[2] - luma601(px)));
    drop((guide, mean_g, var_g));
    let blend = (p.amount / CHROMA_NR.full_at).min(1.0);
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let l = luma601(px);
            let (cr, cb) = (px[0] - l, px[2] - l);
            let cr_q = upsample_at(&a_r, lw, lh, s, x, y) * l + upsample_at(&b_r, lw, lh, s, x, y);
            let cb_q = upsample_at(&a_b, lw, lh, s, x, y) * l + upsample_at(&b_b, lw, lh, s, x, y);
            let r = l + cr + (cr_q - cr) * blend;
            let b = l + cb + (cb_q - cb) * blend;
            let g = (l - 0.299 * r - 0.114 * b) / 0.587;
            *px = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
        }
    });
}

#[cfg(test)]
mod tests;
