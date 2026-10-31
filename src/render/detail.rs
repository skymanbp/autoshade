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
//! pixels. Since v1.17.0 the sharpening law is measured against Adobe Camera
//! Raw rendering the star frame by script ([`SHARPEN`] carries its readings);
//! the two noise-reduction operators are still first-principles values and
//! say so.
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
    bilinear_plane, box_blur_h, box_blur_v, gauss_blur_plane, luma601, smoothstep, write_luma_weighted,
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

/// The encoded lumas the law's four tables are stated at. Dense where the
/// star frame's sky is (0.09–0.18); the last two intervals held 0.6 % and
/// 0.3 % of the fitted pixels (bright stars), the first 0.2 %.
const SHARPEN_KNOTS: [f32; 11] = [0.0, 0.06, 0.09, 0.11, 0.13, 0.15, 0.18, 0.22, 0.30, 0.45, 1.0];

/// A law table read at `v` (piecewise linear between [`SHARPEN_KNOTS`], held
/// at its ends).
fn table_at(table: &[f32; 11], v: f32) -> f32 {
    let k = &SHARPEN_KNOTS;
    if v <= k[0] {
        return table[0];
    }
    for i in 1..k.len() {
        if v <= k[i] {
            let t = (v - k[i - 1]) / (k[i] - k[i - 1]);
            return table[i - 1] + (table[i] - table[i - 1]) * t;
        }
    }
    table[k.len() - 1]
}

/// What Detail does to one side of the law, at Detail 0 and at Detail 100
/// (Detail 25, the default, is 1 throughout): a factor on the side's gain and
/// one on its cap.
#[derive(Clone, Copy)]
struct DetailEnd {
    gain_bright: f32,
    gain_dark: f32,
    cap_bright: f32,
    cap_dark: f32,
}

/// Capture sharpening's law, MEASURED against Adobe Camera Raw 18.4 (Lightroom
/// 9.4's engine) on 2026-10-27 (v1.17.0): the star frame (one 61 MP night
/// frame, ISO 2500, no AI Denoise) rendered by script at the sensor's own size
/// in 16 bits through ten sidecars that differ only in the four Sharpening
/// sliders — Amount 0 / 40 / 80 / 150 at Radius 1.0, Radius 0.5 / 2.0 / 3.0,
/// Detail 0 / 100, Masking 50 / 100 around Lightroom's 40 / 1.0 / 25 / 0 —
/// and every constant below fitted jointly to what moved against the Amount 0
/// render, per pixel in encoded luma, on the oracle's own Amount 0 pixels (the
/// lane's `fitjoint2.py`). Per-pixel R² on the fitted sky crop / a held-out
/// crop (horizon glow, buttes, ground): Amount 40 0.966 / 0.963, 80 0.944 /
/// 0.952, 150 0.865 / 0.879; Radius 0.5 0.956 / 0.956, 2.0 0.955 / 0.945, 3.0
/// 0.915 / 0.857; Detail 0 0.716 / 0.661, 100 0.947 / 0.956; Masking 50 0.961
/// / 0.942, 100 0.957 / 0.943. The v1.6.5 law this replaces (fitted on
/// Lightroom's denoised 6144 px exports of the same frame) read 0.908 at 40 and
/// 0.82 / 0.74 at 80 / 150 here, and below 0.5 on Radius 3, both Details and
/// Masking 100.
///
/// Per pixel, with `l` its luma, `G_σ` a Gaussian of σ = `sigma_per_radius` ×
/// Radius film pixels, `Lb = G_σ∗l` and `s = l − Lb`:
///
/// ```text
///   s ≥ 0:  f = GB(Lb)·a_b·r·d_gb·s,     move =  f/√(1 + (f/cap)²), cap = CB(l)·k_b·d_cb
///   s < 0:  f = GD(Lb)·a_d·r·d_gd·|s|,   move = −f/√(1 + (f/cap)²), cap = CD(l)·k_d·d_cd
///   a = (A/40)^power·(A/80)^knee past 80,  k = (A/40)^cap_power,  r = Radius^−radius_power
///   move ×= smoothstep(0, mask_edge·Masking^mask_power, G_β∗|∇Lb|)     (Masking > 0)
///   l' = l + move, the pixel's colour scaled with its luma
/// ```
///
/// * The four tables (`gain_*` over the blurred luma, `cap_*` over the pixel's
///   own luma) carry what v1.6.5 wrote as a Hill rolloff and a cap linear in
///   Y₀: Camera Raw sharpens before its tone curve, so the gain an encoded
///   luma sees follows that curve's slope. Deep shadow (below encoded 0.06) is
///   left alone; the dark side's cap grows with the pixel's own luma (0.0007
///   at 0.06, 0.17 at 0.45) — the ring around a bright star digs, the sky
///   beside a faint one does not.
/// * Amount: both gains grow faster than the slider (× 2.8 bright, × 5.4 dark
///   from 40 to 80; v1.6.5 read × 2.2 / × 2.0 on its denoised exports) while
///   both caps tighten a little (`cap_power` < 0), and past 80 the growth
///   bends (`knee`): Amount 150 is × 0.80 of the power law's gain.
/// * Radius: σ = 0.75 × Radius held at all four radii (free fits read 0.73 –
///   0.75); the gain falls as Radius^−0.88, so a wider radius widens the halo
///   without deepening it.
/// * Detail: at 0 the gains drop to 0.36 / 0.61 and the dark cap to 0.71; at
///   100 the gains rise to 2.4 / 4.4 (the caps stay within 0.82–0.99) — Detail
///   is the gain on the finest signal, not a cap release. Between the three
///   measured values a factor runs linearly in its logarithm.
/// * Masking: the gate is the gradient magnitude of the blurred luma, per film
///   pixel, blurred again by `mask_blur` film pixels, through a smoothstep from
///   0 that opens fully at 0.034 (Masking 50) / 0.151 (100): the mask is soft
///   and grows as Masking^2.15. A gate of this shape on the oracle's own
///   Masking-0 move explains 0.986 / 0.985 of the Masking 50 / 100 moves.
///
/// Unmet (numbers in TECH_STACK): Detail 0 (0.72 here; no shape tried — free
/// tables and σ, a band-pass signal, a blurred move — reached 0.85) and Amount
/// 150 (0.87; a per-case free table reaches 0.897). One frame, one camera, one
/// ISO.
struct SharpenLaw {
    sigma_per_radius: f32,
    gain_bright: [f32; 11],
    gain_dark: [f32; 11],
    cap_bright: [f32; 11],
    cap_dark: [f32; 11],
    power_bright: f32,
    power_dark: f32,
    knee: f32,
    cap_power_bright: f32,
    cap_power_dark: f32,
    radius_power: f32,
    detail0: DetailEnd,
    detail100: DetailEnd,
    mask_edge: f32,
    mask_power: f32,
    mask_blur: f32,
}

const SHARPEN: SharpenLaw = SharpenLaw {
    sigma_per_radius: 0.75,
    gain_bright: [0.03285, 0.4742, 0.6099, 0.7151, 0.8566, 1.009, 1.292, 1.598, 1.586, 1.516, 2.983],
    gain_dark: [0.0, 0.4441, 0.7000, 0.8425, 0.9240, 1.034, 1.414, 2.151, 4.046, 3.043, 2.496],
    cap_bright: [3.0, 3.0, 0.2693, 0.1637, 0.1444, 0.1432, 0.1508, 0.1926, 0.1922, 0.2264, 0.0],
    cap_dark: [0.0, 0.000730, 0.003347, 0.005715, 0.009394, 0.01416, 0.02228, 0.03512, 0.07415, 0.1739, 0.000589],
    power_bright: 1.4892,
    power_dark: 2.4303,
    knee: -0.3461,
    cap_power_bright: -0.1239,
    cap_power_dark: -0.1644,
    radius_power: 0.8776,
    detail0: DetailEnd { gain_bright: 0.3587, gain_dark: 0.6122, cap_bright: 0.9876, cap_dark: 0.7054 },
    detail100: DetailEnd { gain_bright: 2.3725, gain_dark: 4.4048, cap_bright: 0.9661, cap_dark: 0.8248 },
    mask_edge: 0.1506,
    mask_power: 2.1457,
    mask_blur: 1.5,
};

/// The smallest cap: the move a pixel the table gives no room still takes.
const SHARPEN_CAP_FLOOR: f32 = 1e-5;

/// A Detail factor at `detail` (0..=1): 1 at Lightroom's 25, the measured end
/// values at 0 and 100, linear in its logarithm between.
fn detail_factor(at0: f32, at100: f32, detail: f32) -> f32 {
    const PIN: f32 = 0.25;
    if detail <= PIN {
        at0.powf(1.0 - detail / PIN)
    } else {
        at100.powf((detail - PIN) / (1.0 - PIN))
    }
}

/// `f / √(1 + (f / cap)²)`: `f` for a free move well under the cap, the cap
/// for one well over it, the quadratic-mean knee between (the knee the oracle
/// has; `tanh` sits 12 % high through the middle of it).
fn soft_cap(f: f32, cap: f32) -> f32 {
    f * cap / (cap * cap + f * f).sqrt()
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
/// The move scales the pixel's colour with its luma (`write_luma_weighted`):
/// on the oracle the channels moved as a RATIO of the luma move — 0.6–0.7 % of
/// the per-channel move unexplained that way at Amount 40 / 80 / Detail 100,
/// 10–12 % if the move is added to R, G and B alike (v1.6.5's reading of the
/// denoised exports, whose colour noise was already gone).
///
/// A NEGATIVE amount (a mask softening its background) is the plain signed
/// unsharp mask it always was — a blur toward `G_σ∗l`, no table and no cap:
/// Lightroom's local −Sharpness is unmeasured, and this is what it looks like.
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
    let radius = p.radius.max(SHARPEN_RADIUS_MIN);
    let sigma_true = film.raster_px(SHARPEN.sigma_per_radius * radius);
    let sigma = sigma_true.max(SHARPEN_MIN_SIGMA_PX);
    let fade = (usm_transfer(sigma_true) / usm_transfer(sigma)).clamp(0.0, 1.0);
    if fade < 1e-3 {
        return;
    }
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let blur = gauss_blur_plane(&luma, w, h, sigma);
    drop(luma);
    let edge = (p.masking > 0.0).then(|| edge_mask(&blur, w, h, p.masking, film));
    let gate = |i: usize| edge.as_ref().map_or(1.0, |e| e[i]);
    if p.amount < 0.0 {
        let k = p.amount * fade;
        write_luma_weighted(data, w, weight, |i, l, wgt| l + k * (l - blur[i]) * gate(i) * wgt);
        return;
    }
    // The slider over Lightroom's RAW default of 40, the amount the tables are
    // stated at (`amount` is the slider ÷ 100), and its bend past 80.
    let over40 = p.amount / 0.4;
    let past80 = (p.amount / 0.8).max(1.0).powf(SHARPEN.knee);
    let (d0, d100) = (SHARPEN.detail0, SHARPEN.detail100);
    let df = |at0: f32, at100: f32| detail_factor(at0, at100, p.detail);
    let common = radius.powf(-SHARPEN.radius_power) * fade * past80;
    let gain_bright = over40.powf(SHARPEN.power_bright) * common * df(d0.gain_bright, d100.gain_bright);
    let gain_dark = over40.powf(SHARPEN.power_dark) * common * df(d0.gain_dark, d100.gain_dark);
    let cap_bright = over40.powf(SHARPEN.cap_power_bright) * df(d0.cap_bright, d100.cap_bright);
    let cap_dark = over40.powf(SHARPEN.cap_power_dark) * df(d0.cap_dark, d100.cap_dark);
    write_luma_weighted(data, w, weight, |i, l, wgt| {
        let s = l - blur[i];
        let mv = if s >= 0.0 {
            let f = table_at(&SHARPEN.gain_bright, blur[i]) * gain_bright * s;
            soft_cap(f, (table_at(&SHARPEN.cap_bright, l) * cap_bright).max(SHARPEN_CAP_FLOOR))
        } else {
            let f = table_at(&SHARPEN.gain_dark, blur[i]) * gain_dark * -s;
            -soft_cap(f, (table_at(&SHARPEN.cap_dark, l) * cap_dark).max(SHARPEN_CAP_FLOOR))
        };
        l + mv * gate(i) * wgt
    });
}

/// Masking's edge map: 0 in flat areas, 1 on edges — the gradient of the
/// Radius-blurred luma measured per FILM pixel (so a preview and an export
/// agree on which edges are edges), blurred by `mask_blur` film pixels, through
/// a smoothstep from 0 to `mask_edge · Masking^mask_power`.
fn edge_mask(blur: &[f32], w: usize, h: usize, masking: f32, film: FilmScale) -> Vec<f32> {
    let full = SHARPEN.mask_edge * masking.powf(SHARPEN.mask_power);
    let grad: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let gx = 0.5 * (blur[y * w + (x + 1).min(w - 1)] - blur[y * w + x.saturating_sub(1)]);
            let gy = 0.5 * (blur[(y + 1).min(h - 1) * w + x] - blur[y.saturating_sub(1) * w + x]);
            (gx * gx + gy * gy).sqrt() / film.factor()
        })
        .collect();
    let spread = film.raster_px(SHARPEN.mask_blur);
    let grad = if spread >= 0.3 { gauss_blur_plane(&grad, w, h, spread) } else { grad };
    grad.into_par_iter().map(|g| smoothstep(0.0, full, g)).collect()
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
