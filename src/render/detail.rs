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
//! pixels. The sharpening ladder was exported and fitted on 2026-09-27
//! ([`SHARPEN`] carries the measured law and its readings); the two
//! noise-reduction operators are still first-principles values and say so.
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
    bilinear_plane, box_blur_h, box_blur_v, gauss_blur_plane, luma601, smoothstep, write_luma_additive,
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

/// Capture sharpening's law, MEASURED against Lightroom 9.4 on 2026-09-27
/// (v1.6.5): one 61 MP frame — ILCE-7RM4A, ISO 2500, 20 s, a star field over
/// dark ground, Lightroom's Denoise 50 on — exported at Sharpness 0 / 40 / 80
/// with Radius 1.0, Detail 25, Masking 0, and this operator's constants fitted
/// to what moved between the exports, in gamma-encoded luma (the calibration
/// lane's `fit_enc6.py`, a per-pixel least squares on a sample stratified over
/// local luminance × signal amplitude × edge side: per-pixel R² 0.93 at 40 and
/// 0.92 at 80 over the whole frame, 0.95 on the pixels whose unsharp signal is
/// past 0.03 — the edges and stars one sees — against 0.62 for the plain
/// unsharp mask this replaced). What each constant is, and what the frame read:
///
/// * `sigma_per_radius` — the Gaussian σ in film pixels per unit of Radius.
///   Radius 1.0 is a 0.75 px kernel BEFORE the limiters below; they broaden
///   the apparent kernel to the 1.17 px a plain unsharp-mask fit reads off the
///   Amount 40 export (this law reads 1.09 there, 1.05 at 80 against 0.86).
/// * `gain_bright` / `gain_dark` — the gain on the unsharp signal at Amount
///   40 in a region brighter than the rolloff, on the BRIGHT side of an edge
///   (`s ≥ 0`) and on its DARK side, each ∝ (Amount ÷ 40)^`power_*` — both
///   about linear in the slider (× 2.2 and × 2.0 from 40 to 80). The dark
///   side's gain is the larger; what holds dark halos is its cap.
/// * `rolloff_y0` / `rolloff_p` — the gain falls to half at a local luminance
///   of 0.014 in the exports' gamma-2.2 encoding (0.15 encoded), Hill slope
///   2.8: the frame's ground (0.002) was sharpened at 0.03 of the sky's gain.
///   Deep shadows, and the noise in them, are left alone.
/// * `cap_bright` (∝ (Amount ÷ 40)^`cap_power`) — the soft limit on the
///   bright side's move, `f / √(1 + (f / cap)²)` on the free move `f`, at
///   Detail 25: 0.145 at 40, 0.167 at 80. Star peaks lifted +0.07 / +0.11 /
///   +0.12 at 40 (faint / mid / bright) and +0.13 / +0.15 / +0.13 at 80; the
///   law reads +0.08 / +0.11 / +0.13 and +0.13 / +0.15 / +0.16.
/// * `dark_cap_luma` / `dark_cap_free` — the dark side's cap is not a
///   constant: `dark_cap_luma × Y₀ − dark_cap_free × f`, with `Y₀` the pixel's
///   OWN luminance (gamma 2.2) and `f` its free move. A pixel may lose about
///   1.1 × its luminance, less the more it is being pushed. The ring around a
///   star therefore dips with the star: 0.02 / 0.04 / 0.08 at 40 for the three
///   classes (the law reads 0.02 / 0.04 / 0.07), and a ring below encoded
///   ~0.15 does not dip at all; the same limiter is what leaves shadow noise
///   alone on the dark side. [`cap_at_detail`] carries both caps to the rest
///   of the Detail band.
/// * `edge` — Masking's gate (the blurred-luma gradient per FILM pixel at
///   which it opens fully; Masking 0 applies everywhere), unmeasured: every
///   export was at Masking 0 (`SH-100-M50`, `SH-100-M100` still to pin).
struct SharpenLaw {
    sigma_per_radius: f32,
    gain_bright: f32,
    power_bright: f32,
    gain_dark: f32,
    power_dark: f32,
    rolloff_y0: f32,
    rolloff_p: f32,
    cap_bright: f32,
    cap_power: f32,
    dark_cap_luma: f32,
    dark_cap_free: f32,
    edge: Ramp,
}

const SHARPEN: SharpenLaw = SharpenLaw {
    sigma_per_radius: 0.75,
    gain_bright: 1.892,
    power_bright: 1.124,
    gain_dark: 2.524,
    power_dark: 1.007,
    rolloff_y0: 0.0139,
    rolloff_p: 2.78,
    cap_bright: 0.145,
    cap_power: 0.206,
    dark_cap_luma: 1.107,
    dark_cap_free: 0.0707,
    edge: Ramp(0.0, 0.04),
};

/// The encoding the law's luminances are stated in: the exports' gamma 2.2,
/// applied to the engine's encoded luma as it was to the export's. The law is
/// a function of the encoded value a viewer sees, so the same encoded luma
/// gets the same move here as it did in Lightroom.
const FIT_GAMMA: f32 = 2.2;

/// The smallest dark-side cap: the move a pixel too dark for any dark halo
/// still takes, a 150th of an 8-bit code.
const DARK_CAP_FLOOR: f32 = 1e-4;

/// A cap stated at Detail 25 (the one Detail the kit exported), carried to the
/// rest of the band on a log ramp over Detail²: no cap (1.0, the whole luma
/// range) at Detail 100, the pinned value at 25, a little below it at 0.
/// Adobe documents Detail as halo suppression, the low end suppressing most.
fn cap_at_detail(cap25: f32, detail: f32) -> f32 {
    cap25.powf(detail_exponent(detail))
}

/// The exponent [`cap_at_detail`] raises a cap to: 1 at Detail 25.
fn detail_exponent(detail: f32) -> f32 {
    const PIN: f32 = 0.25 * 0.25;
    (1.0 - detail * detail) / (1.0 - PIN)
}

/// `f / √(1 + (f / cap)²)`: `f` for a free move well under the cap, the cap
/// for one well over it, the quadratic-mean knee between (the knee the exports
/// have; `tanh` sits 12 % high through the middle of it).
fn soft_cap(f: f32, cap: f32) -> f32 {
    f * cap / (cap * cap + f * f).sqrt()
}

/// Steps of [`rolloff_table`] over the encoded luma 0..=1.
const ROLLOFF_STEPS: usize = 1024;

/// The shadow rolloff `ρ(Y) = Yᵖ / (Yᵖ + Y₀ᵖ)` on the local luminance (the
/// blurred luma, [`FIT_GAMMA`]-decoded), tabulated over the encoded luma the
/// blur plane holds — ρ is smooth, and the nearest of 1025 steps is within a
/// thousandth of the value.
fn rolloff_table() -> Vec<f32> {
    let y0p = SHARPEN.rolloff_y0.powf(SHARPEN.rolloff_p);
    (0..=ROLLOFF_STEPS)
        .map(|i| {
            let yp = (i as f32 / ROLLOFF_STEPS as f32).powf(FIT_GAMMA * SHARPEN.rolloff_p);
            yp / (yp + y0p)
        })
        .collect()
}

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
    /// The global stage, or `None` at Amount 0. `raw_source` says which
    /// default an absent amount renders at — Lightroom's 40 on a RAW
    /// negative, nothing on a baked raster (`EditRecipe::capture_sharpening`,
    /// v1.6.0).
    pub(crate) fn global(r: &EditRecipe, raw_source: bool) -> Option<Self> {
        let amount = r.capture_sharpening(raw_source);
        (amount > 0.0).then(|| Self::at_amount(r, amount))
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

/// Capture sharpening on luma, [`SHARPEN`]'s measured law. `weight(x, y, px)`
/// is the mask arm's coverage; the global stage passes 1.
///
/// Per pixel, with `l` the luma, `G_σ` a Gaussian of the Radius (σ =
/// `sigma_per_radius` film px per unit), `s = l − G_σ∗l` the unsharp signal,
/// `ρ` the shadow rolloff read at the local (blurred) luma and `Y₀ = l^2.2`:
///
/// ```text
///   s ≥ 0:  f = gain_bright·ρ·s,    cap = cap_bright
///   s < 0:  f = gain_dark·ρ·|s|,    cap = dark_cap_luma·Y₀ − dark_cap_free·f
///   move    = ±f / √(1 + (f / cap)²)                       (the sign of s)
///   l'      = l + move·edge_mask·weight          (added to R, G and B alike)
/// ```
///
/// The move is ADDED to the three channels — neutral light in or out — which
/// is how the Lightroom export moved (`write_luma_additive`); every other luma
/// pass in this engine scales chroma instead.
///
/// A NEGATIVE amount (a mask softening its background) is the plain signed
/// unsharp mask it always was — a blur toward `G_σ∗l` through the
/// chroma-scaling write, no rolloff and no cap: Lightroom's local −Sharpness is
/// unmeasured, and this is what it looks like.
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
    let sigma_true = film.raster_px(SHARPEN.sigma_per_radius * p.radius.max(SHARPEN_RADIUS_MIN));
    let sigma = sigma_true.max(SHARPEN_MIN_SIGMA_PX);
    let fade = (usm_transfer(sigma_true) / usm_transfer(sigma)).clamp(0.0, 1.0);
    if fade < 1e-3 {
        return;
    }
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let blur = gauss_blur_plane(&luma, w, h, sigma);
    let edge = (p.masking > 0.0).then(|| edge_mask(&blur, w, h, p.masking, film));
    let gate = |i: usize| edge.as_ref().map_or(1.0, |e| e[i]);
    if p.amount < 0.0 {
        let k = p.amount * fade;
        write_luma_weighted(data, w, weight, |i, l, wgt| l + k * (l - blur[i]) * gate(i) * wgt);
        return;
    }
    // The slider over Lightroom's RAW default of 40, the amount the law is
    // stated at (`amount` is the slider ÷ 100).
    let over40 = p.amount / 0.4;
    let gain_bright = SHARPEN.gain_bright * over40.powf(SHARPEN.power_bright) * fade;
    let gain_dark = SHARPEN.gain_dark * over40.powf(SHARPEN.power_dark) * fade;
    let cap_bright = cap_at_detail(SHARPEN.cap_bright * over40.powf(SHARPEN.cap_power), p.detail);
    let detail_exp = detail_exponent(p.detail);
    let rolloff = rolloff_table();
    write_luma_additive(data, w, weight, |i, l, wgt| {
        let s = l - blur[i];
        let rho = rolloff[(blur[i].clamp(0.0, 1.0) * ROLLOFF_STEPS as f32).round() as usize];
        let mv = if s >= 0.0 {
            soft_cap(gain_bright * rho * s, cap_bright)
        } else {
            let f = gain_dark * rho * -s;
            let y0 = l.clamp(0.0, 1.0).powf(FIT_GAMMA);
            let cap25 = (SHARPEN.dark_cap_luma * y0 - SHARPEN.dark_cap_free * f).max(DARK_CAP_FLOOR);
            let cap = if detail_exp == 1.0 { cap25 } else { cap25.powf(detail_exp) };
            -soft_cap(f, cap)
        };
        l + mv * gate(i) * wgt
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

/// The fast guided filter's geometry at a reach of `r_raster` raster pixels
/// and Smoothness `smoothness` (0..=1): the subsampling `s` (at most `cap`),
/// the box radius on the subsampled plane and the offset plane's widened one.
fn chroma_nr_geometry(r_raster: f32, smoothness: f32, cap: usize) -> (usize, usize, usize) {
    let s = ((r_raster / 2.5).floor() as usize).max(2).min(cap);
    let radius = ((r_raster / s as f32).round() as usize).max(1);
    let smooth_radius = ((radius as f32 * CHROMA_NR.widen.at(smoothness)).round() as usize).max(radius);
    (s, radius, smooth_radius)
}

/// The share of white noise [`chroma_nr`] leaves on a flat region of a
/// full-size develop (film = raster) at slider fraction `amount` and
/// Smoothness `smoothness` (both 0..=1): `Σw²` of its whole chain there. On a
/// flat region the guide carries no variation, the coefficient `a` is 0 and
/// the output is the offset plane — the `s × s` block mean, the box of
/// `radius` cells, the box of `smooth_radius` cells, the bilinear upsample —
/// blended with the input by `min(amount / full_at, 1)`. Every step is
/// separable, so `Σw²` is a product of two 1-D sums per output pixel's phase
/// in its block, averaged over the `s²` phases. 1 when the pass does not run.
/// The RAW's linear stage reads it (`denoise::linear_chroma::window`): at each
/// strength it averages as many samples as this pass would.
pub(crate) fn chroma_nr_flat_noise_share(amount: f32, smoothness: f32) -> f64 {
    let r = CHROMA_NR.radius.at(amount);
    if amount <= 0.0 || r < 0.5 {
        return 1.0;
    }
    let (s, radius, smooth_radius) = chroma_nr_geometry(r, smoothness, usize::MAX);
    let blend = f64::from((amount / CHROMA_NR.full_at).min(1.0));
    // The two boxes on the cells, convolved: the offset plane's cell weights
    // about the cell the bilinear tap reads.
    let (a, b) = (2 * radius + 1, 2 * smooth_radius + 1);
    let mut cell = vec![0.0f64; a + b - 1];
    for i in 0..a {
        for j in 0..b {
            cell[i + j] += 1.0 / (a * b) as f64;
        }
    }
    let c0 = (a + b - 2) / 2;
    // Per phase `p` of an interior output pixel `x = n·s + p`: the bilinear
    // tap (`upsample_at`'s coordinate) spreads the cell weights over two
    // cells, and each cell's weight over its `s` pixels. Returns `Σw²` over
    // the pixels and the weight of the output pixel's own pixel.
    let phase = |p: usize| -> (f64, f64) {
        let x = 64 * s + p;
        let u = (x as f64 + 0.5) / s as f64 - 0.5;
        let x0 = u.floor();
        let fx = u - x0;
        let mut wc = vec![0.0f64; cell.len() + 1];
        for (j, w) in cell.iter().enumerate() {
            wc[j] += (1.0 - fx) * w;
            wc[j + 1] += fx * w;
        }
        // `wc[m]` is cell `x0 + m − c0`; the pixel's own cell is `x / s`.
        let own = ((x / s) as f64 - x0) as usize + c0;
        (wc.iter().map(|w| w * w).sum::<f64>() / s as f64, wc.get(own).map_or(0.0, |w| w / s as f64))
    };
    let phases: Vec<(f64, f64)> = (0..s).map(phase).collect();
    let mut total = 0.0f64;
    for &(sx, ox) in &phases {
        for &(sy, oy) in &phases {
            total += (1.0 - blend).powi(2) + 2.0 * blend * (1.0 - blend) * ox * oy + blend * blend * sx * sy;
        }
    }
    total / (s * s) as f64
}

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
pub(crate) fn upsample_at(plane: &[f32], lw: usize, lh: usize, s: usize, x: usize, y: usize) -> f32 {
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
    let (s, radius, smooth_radius) = chroma_nr_geometry(r_raster, p.smoothness, w.max(h));
    let (lw, lh) = (w.div_ceil(s), h.div_ceil(s));
    let edge = CHROMA_NR.edge.at(p.detail) / film.factor();
    let eps = edge * edge;
    let guide = block_mean(data, w, h, s, luma601);
    let mean_g = box_mean(&guide, lw, lh, radius);
    let mut var_g = box_mean_owned(guide.iter().map(|g| g * g).collect(), lw, lh, radius);
    var_g.iter_mut().zip(&mean_g).for_each(|(v, m)| *v = (*v - m * m).max(0.0));
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
            *px = chroma_into_range(l, [r, g, b]);
        }
    });
}

/// A rebuilt pixel brought back into `[0, 1]` along its colour difference,
/// keeping its luma `l`: the chroma `px − l` is scaled by the largest factor
/// in `[0, 1]` that keeps every channel in range (the grey `[l; 3]` is the
/// limit). Clamping each channel instead moves the luma — a pure black whose
/// neighbours lend it colour asks for one negative channel, and cutting only
/// that one lifts it off 0 (measured on a night frame: all 2.0 % of its
/// pixels at luma 0 left 0 under the clamp at Color 25).
fn chroma_into_range(l: f32, px: [f32; 3]) -> [f32; 3] {
    let l = l.clamp(0.0, 1.0);
    let mut t = 1.0f32;
    for v in px {
        let c = v - l;
        if v > 1.0 {
            t = t.min((1.0 - l) / c);
        } else if v < 0.0 {
            t = t.min(l / -c);
        }
    }
    px.map(|v| (l + (v - l) * t).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests;
