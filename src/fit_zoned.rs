//! Zoned reverse-fit — the semantic-region extension of [`crate::fit`].
//!
//! The global fit is statistics over the WHOLE frame, and its gates refuse
//! regional regrades by design (fit.rs rotation budget: "true regional
//! regrades belong to the zoned fit"). This module supplies that fit: segment
//! the same semantic region (the sky) in BOTH the source and the target,
//! compare the two zones' colour statistics, and emit the difference as a
//! bitmap-masked [`LocalAdjustment`](crate::recipe::LocalAdjustment) driving
//! the engine's local dials (render.rs `apply_masks`).
//!
//! Identifiability stance (fit.rs's, one level down): zone MOMENTS (weighted
//! first moments) only — no per-zone CDFs or curves. A zone is a small,
//! soft-edged, non-pixel-aligned population; means are the only statistics
//! stable enough to trust there. The global fit must refuse per-channel
//! moves because it cannot tell a cast from content (WHERE is unknown); here
//! the mask answers WHERE, so exact per-channel gains on the zone are
//! identified — that is the entire expressiveness upgrade.
//!
//! Dial choice (measured, golden-sky pair): a palette transplant (pale-blue
//! sky → vivid gold) demands linear channel ratios of r/b ≈ 5.3×, while ANY
//! white-balance parametrisation caps near 1.9× (the full 2000–40000 K
//! blackbody range) and ±100 saturation only doubles chroma — Temp/Tint/Sat
//! physically cannot repaint. So the fit solves the move as **exact
//! per-channel linear gains** (`color_gains`, engine-rendered inside the
//! mask) with brightness split out into local exposure (the tone LUT's soft
//! shoulder handles it more gracefully than a raw linear gain would).
//! Saturation stays closed-loop through real renders ([`zone_sat_step`]),
//! matching the global fit's philosophy.

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage};

use crate::fit::{self, FitReport};
use crate::recipe::{LocalAdjustment, MaskGeometry, MaskRole, RangeMask};
use crate::render;
use crate::segment::{segment_file, SegmentOpts};

/// A zone must cover at least this weighted share of ITS frame on BOTH sides
/// to carry trustworthy moments (a real sky measures 10–40%; segmentation
/// misses and boundary slivers sit far below).
pub(crate) const MIN_ZONE_SHARE: f32 = 0.03;
/// Conservative local-exposure budget: ±2.5 EV covers any real sky-to-sky
/// brightness gap; a larger demand means the zones do not correspond.
const ZONE_EV_LIMIT: f32 = 2.5;
/// Atmosphere zones keep only a restrained local brightness move.
const ZONE_ATMOS_EV_LIMIT: f32 = 0.75;
/// Local saturation shares the global fit's model cap (fit.rs stage 3).
const ZONE_SAT_LIMIT: f32 = 60.0;
const ZONE_ATMOS_SAT_LIMIT: f32 = 20.0;
const ZONE_ATMOS_GAIN_MIN: f32 = 0.85;
const ZONE_ATMOS_GAIN_MAX: f32 = 1.18;
/// Mask-weighted mean-gradient energy may not fall below this ratio. Accepted
/// repository zones measure 0.730, 0.980, 1.084, 1.330, 1.684 and 1.918.
/// The saved generated-cloud correction measures 0.961 with zero clipped-share
/// growth, so this exact statistic cannot separate it from those accepted
/// zones; that calibration contradiction is pinned in the fixture test and
/// disclosed in the implementation report rather than hidden by a false cap.
const ZONE_TEXTURE_MIN: f32 = 0.70;
/// Mask-weighted mean-gradient energy may not grow above this ratio. The 2.05
/// ceiling leaves measured headroom over the accepted maximum of 1.972.
const ZONE_TEXTURE_MAX: f32 = 1.95;
/// Weighted clipped-luma share may grow by at most one percentage point.
const ZONE_CLIP_GROWTH: f32 = 0.01;
/// Maximum signed sky-side luma bump across the 5%-95% mask feather. The
/// statistic is the 90th percentile of `brightest sky-half - settled sky` on
/// rows/columns carrying BOTH settled interiors, so positive means the
/// feather bows into the bright-rim direction. At the 384-edge analysis size
/// the synthetic bright-half probe reads +0.120, the opposite-sign probe
/// +0.013, the same-sign probe -0.020, the four accepted repository fixture
/// entries -0.007, the no-zone calibration +0.013, the previous fitted pair
/// -0.009, and HEAD's opposite-sign pair +0.054. The real pair measures +0.038
/// before the gate and +0.012 after its largest passing shrink (k=0.093).
/// The calibrated round budget is +0.012; the supervisor's independent RAW
/// rim metric is the final regression check because it samples a 40px crossing
/// neighbourhood rather than this analysis-grid statistic.
const ZONE_BOUNDARY_RIM_MAX: f32 = 0.012;
/// Acceptance: the zone-local error ([`zone_err`]) must fall to ≤ this
/// fraction of its pre-correction value. The correction is judged on ITS
/// zone, not on the frame-global `look_err` — measured on the real pair
/// (2026-07-09, _DSC9621 × reimagine-5): the sky correction landed the zone
/// moments almost exactly on the target's (zone error 0.507 → 0.015) while
/// the FRAME-global metric moved 0.1768 → 0.1792, because the generative
/// target holds ~3× more sky area than the source (the composition differs —
/// no zone repaint can reconcile frame-level distributions) and a correct
/// blue→gold repaint migrates band mass, which the worst-band hue term can
/// only read as damage. A frame-global gate therefore vetoes exactly the
/// correction this module exists to make.
const ZONE_ACCEPT_RATIO: f32 = 0.5;
/// The relative gate above has no absolute yardstick, and that produced the
/// R17-era complaint: a zone that was ALREADY matched (sky 0.012 on the
/// murk-era pair) was "corrected", barely moved, and reported "dropped:
/// needs ≤ 50%" — reading like a discarded improvement when there was
/// nothing to improve. Two absolute yardsticks fix that, SPLIT on purpose
/// (R19 — one shared number either skipped fixable zones or dialled
/// matched ones): corrections that genuinely work LAND at 0.007–0.015
/// (this pair's sky 0.076 → 0.007; golden-sky 0.507 → 0.015), so this
/// figure — just above that landing range — is the ACCEPTANCE floor: a
/// correction ending at/below it with a real gain is accepted even when
/// the relative arm alone would refuse (started close, ended matched).
const ZONE_MATCHED_ERR: f32 = 0.02;
/// …while the observed already-matched zones read 0.009–0.012 and every
/// attempt at them regressed (land 0.009 → 0.029 on the live pair), so
/// THIS figure — the ceiling of that observed matched domain — is the
/// SKIP line: at/below it the zone is left alone with an honest "already
/// matches" note. Zones between the two figures are attempted and judged
/// by [`zone_accepts`]; nothing is declined untried above the matched
/// domain.
const ZONE_SKIP_ERR: f32 = 0.012;
/// `zone_err` lives in LINEAR light, so an absolute line alone means
/// different things at different zone levels — 0.012 of linear mean is a
/// hundredth of a stop on a bright sky but most of a stop in deep shadow
/// (sRGB ≈ 0.12 vs 0.173 zones score zone_err ≈ 0.012 while sitting
/// 0.9 EV apart; that zone must be FITTED, not declared matched). Both
/// absolute yardsticks therefore carry this quarter-stop EV companion —
/// the skip line refuses to declare such a zone matched, and the
/// acceptance floor refuses to call such a landing matched; the relative
/// acceptance arm needs no companion because ratios are scale-free.
const ZONE_MATCHED_EV: f32 = 0.25;
/// The floor-landing acceptance arm must still MOVE the zone — without a
/// minimum gain, a hairline 0.0201 → 0.0200 "landing" would buy the full
/// [`ZONE_GLOBAL_REGRESSION_TOL`] drift budget (200× the zone gain) and
/// overwrite `err_after` with the worse frame number. One fifth of the
/// starting error is the smallest move worth a mask.
const ZONE_FLOOR_MIN_GAIN: f32 = 0.8;
/// Insurance bound: the mask cannot touch pixels outside its raster (engine
/// guarantee, pinned by the rocks-bit-equal test), so the only frame-global
/// drift a correct zone repaint can cause is metric-visible band migration
/// inside its own region. Allow that small, measured drift (+0.0024 on the
/// real pair) but refuse anything larger — a big global regression means the
/// mask is NOT the region we thought it was.
const ZONE_GLOBAL_REGRESSION_TOL: f32 = 0.02;

/// Four bands is the existing measured stability ceiling for value-range
/// evidence; finer partitions routinely fall below the evidence floor.
const RANGE_MAX_BANDS: usize = 4;
/// Corrected rank-mean calibration keeps `0.03` between supported neutral
/// bins (01-07 and 12, at most `0.025`) and the coherent 08-11 run (starting
/// at `0.036`); the isolated supported bin 13 measures `0.223`.
const RANGE_RESIDUAL_TRIGGER: f32 = 0.03;
/// One 17-bin evidence interval is the minimum transition width; the retained
/// opposite-half-EV probe measured a 5/255 step when that protection vanished.
const RANGE_MIN_RAMP: f32 = 1.0 / 17.0;
/// Two evidence intervals provide measured transition headroom before the
/// shared boundary shrink has to reduce correction differentials.
const RANGE_MAX_RAMP: f32 = 2.0 / 17.0;
/// Native range transitions reuse the calibrated zoned signed-rim budget.
const RANGE_BOUNDARY_RIM_MAX: f32 = 0.012;
/// Native bands reuse the global evidence model's measured 1.5% population
/// floor; a smaller interval is not a two-sided measurement.
const RANGE_MIN_EVIDENCE_SHARE: f32 = 0.015;
/// Range bands must pay for themselves on the composed evidence-weighted
/// frame: the live regression measured `0.018 -> 0.024` after two bands.
/// Equality is acceptable, but any worse frame restores the running recipe.
const RANGE_FRAME_REGRESSION_TOL: f32 = 0.0;

const RANGE_HOST: MaskGeometry = MaskGeometry::Linear {
    zero_x: 0.5,
    zero_y: -0.8,
    full_x: 0.5,
    full_y: -0.4,
};

/// Per-zone policy selected from the same structural statistic as the global
/// solve, before any within-zone CDF fitting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneMode {
    Full,
    Atmosphere,
}

/// The zone-local acceptance predicate: halve the zone error, or land it in
/// matched territory — brightness included — with a real gain (see
/// [`ZONE_ACCEPT_RATIO`], [`ZONE_MATCHED_ERR`], [`ZONE_MATCHED_EV`] and
/// [`ZONE_FLOOR_MIN_GAIN`]). Pure so the regimes are unit-testable without
/// an end-to-end fit.
fn zone_accepts(zone_before: f32, zone_after: f32, ev_gap_after: f32) -> bool {
    zone_after <= zone_before * ZONE_ACCEPT_RATIO
        || (zone_after <= ZONE_MATCHED_ERR
            && ev_gap_after <= ZONE_MATCHED_EV
            && zone_after <= ZONE_FLOOR_MIN_GAIN * zone_before)
}

/// The skip decision, pure for the same reason: a zone at/below the
/// observed matched domain — brightness included — is left alone with the
/// honest note instead of being dialled.
fn zone_skips(zone_before: f32, ev_gap: f32) -> bool {
    zone_before <= ZONE_SKIP_ERR && ev_gap <= ZONE_MATCHED_EV
}

/// Mask-weighted first moments of one zone.
pub(crate) struct ZoneMoments {
    /// Weighted mean Rec.601 luma of the LINEAR-light channels (EV math needs
    /// linear; the engine's exact transfer curve via `srgb_to_linear`).
    pub luma_lin: f32,
    /// Weighted mean per-channel LINEAR values (colour gains act here).
    pub mean_lin: [f32; 3],
    /// Weighted mean HSV-style chroma (max−min) in sRGB — the same definition
    /// the global fit's `mean_chroma` uses, so the two stages agree on what
    /// "saturated" means.
    pub chroma: f32,
    /// Weighted zone share of the frame, Σw / n.
    pub share: f32,
}

/// Moments of the zone selected by `weights` (one weight per pixel, [0,1] —
/// a decoded segmentation mask, or anything else). Zero-weight pixels cost
/// nothing. A degenerate mask (Σw ≈ 0) returns `share == 0.0` and neutral
/// moments — callers gate on [`MIN_ZONE_SHARE`] anyway.
pub(crate) fn zone_moments(px: &[[f32; 3]], weights: &[f32]) -> ZoneMoments {
    debug_assert_eq!(px.len(), weights.len());
    let mut w_total = 0.0f64;
    let mut luma = 0.0f64;
    let mut mean = [0.0f64; 3];
    let mut chroma = 0.0f64;
    for (p, &w) in px.iter().zip(weights) {
        if w <= 0.0 {
            continue;
        }
        let w = w as f64;
        w_total += w;
        let lin = [
            render::srgb_to_linear(p[0]),
            render::srgb_to_linear(p[1]),
            render::srgb_to_linear(p[2]),
        ];
        luma += w * (0.299 * lin[0] + 0.587 * lin[1] + 0.114 * lin[2]) as f64;
        for c in 0..3 {
            mean[c] += w * lin[c] as f64;
        }
        chroma += w * (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) as f64;
    }
    if w_total <= 0.0 {
        return ZoneMoments { luma_lin: 0.0, mean_lin: [0.0; 3], chroma: 0.0, share: 0.0 };
    }
    ZoneMoments {
        luma_lin: (luma / w_total) as f32,
        mean_lin: [
            (mean[0] / w_total) as f32,
            (mean[1] / w_total) as f32,
            (mean[2] / w_total) as f32,
        ],
        chroma: (chroma / w_total) as f32,
        share: (w_total / px.len().max(1) as f64) as f32,
    }
}

/// The coarse zone correction: local exposure + the exact per-channel linear
/// gains the engine's mask stage renders. Saturation is deliberately NOT
/// here — it is closed-loop by construction (see [`zone_sat_step`]).
pub(crate) struct ZoneDials {
    pub exposure_ev: f32,
    pub color_gains: [f32; 3],
}

/// Solve the dials that move the source zone's moments onto the target
/// zone's. Pure moment math, no renders, and EXACT for the moments by
/// construction:
///
/// * **exposure** — linear-luma ratio in EV. Brightness rides the local tone
///   LUT (soft shoulder) instead of a raw gain, so bright zone texture rolls
///   off instead of clipping.
/// * **color_gains** — the remaining brightness-normalised per-channel
///   demand `(tgt/src) / 2^EV` in linear light: exactly the ratios the
///   engine multiplies in (`apply_masks`), exactly what a WB dial cannot
///   express (see the module doc).
pub(crate) fn fit_zone_dials(src: &ZoneMoments, tgt: &ZoneMoments) -> ZoneDials {
    let exposure_ev = (tgt.luma_lin.max(1e-5) / src.luma_lin.max(1e-5))
        .log2()
        .clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT);
    let bright = 2.0f32.powf(exposure_ev);
    let mut color_gains = [1.0f32; 3];
    for (c, gain) in color_gains.iter_mut().enumerate() {
        let want = tgt.mean_lin[c].max(1e-5) / src.mean_lin[c].max(1e-5);
        // Same legal range recipe::clamp enforces (0 would kill a channel).
        *gain = (want / bright).clamp(0.05, 8.0);
    }
    ZoneDials { exposure_ev, color_gains }
}

/// One closed-loop saturation step: the same mean-chroma chase as the global
/// fit's stage 3 (per-step ±40; the caller clamps the accumulated value with
/// [`clamp_zone_sat`]), fed with the zone chroma MEASURED on a real render of
/// the current recipe — open-loop chroma math after a recolour is not
/// trustworthy (the gains change chroma by themselves). Returns the step to
/// ADD to the current local saturation; `None` when converged (< 1 point) or
/// when the zone carries no chroma evidence.
pub(crate) fn zone_sat_step(cur_chroma: f32, tgt_chroma: f32) -> Option<f32> {
    if cur_chroma < 1e-4 {
        return None;
    }
    let step = ((tgt_chroma / cur_chroma - 1.0) * 100.0).clamp(-40.0, 40.0);
    if step.abs() < 1.0 {
        return None;
    }
    Some(step)
}

/// Clamp an accumulated local saturation to the zone model cap.
pub(crate) fn clamp_zone_sat(v: f32) -> f32 {
    v.clamp(-ZONE_SAT_LIMIT, ZONE_SAT_LIMIT)
}

fn clamp_zone_sat_for_mode(v: f32, mode: ZoneMode) -> f32 {
    match mode {
        ZoneMode::Full => clamp_zone_sat(v),
        ZoneMode::Atmosphere => v.clamp(-ZONE_ATMOS_SAT_LIMIT, ZONE_ATMOS_SAT_LIMIT),
    }
}

/// Shrink one atmosphere-zone recolour toward unity with a single scalar, so
/// every channel keeps the fitted direction and the ratios are not independently
/// clipped into a different hue.
fn shrink_atmosphere_gains(gains: [f32; 3]) -> [f32; 3] {
    let mut k = 1.0f32;
    for gain in gains {
        if gain > 1.0 {
            k = k.min((ZONE_ATMOS_GAIN_MAX - 1.0) / (gain - 1.0));
        } else if gain < 1.0 {
            k = k.min((1.0 - ZONE_ATMOS_GAIN_MIN) / (1.0 - gain));
        }
    }
    gains.map(|gain| 1.0 + k.clamp(0.0, 1.0) * (gain - 1.0))
}

#[derive(Clone, Copy, Debug)]
struct LocalQuality {
    texture_ratio: f32,
    clipped_before: f32,
    clipped_after: f32,
}

impl LocalQuality {
    fn texture_passes(self) -> bool {
        (ZONE_TEXTURE_MIN..=ZONE_TEXTURE_MAX).contains(&self.texture_ratio)
    }

    fn clipping_passes(self) -> bool {
        self.clipped_after <= self.clipped_before + ZONE_CLIP_GROWTH
    }

    fn passes(self) -> bool {
        self.texture_passes() && self.clipping_passes()
    }
}

/// Local-quality reading shared by every zone and mode. Texture is the
/// mask-weighted mean magnitude of the forward Rec.601-luma gradient; clipping
/// is the weighted share at <=1/255 or >=254/255.
fn local_quality(
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    weights: &[f32],
    width: u32,
    height: u32,
) -> LocalQuality {
    let (w, h) = (width as usize, height as usize);
    let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let reading = |px: &[[f32; 3]]| -> (f32, f32) {
        let mut total = 0.0f64;
        let mut texture = 0.0f64;
        let mut clipped = 0.0f64;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let weight = weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
                if weight == 0.0 || i >= px.len() {
                    continue;
                }
                let here = luma(&px[i]);
                let dx = if x + 1 < w && i + 1 < px.len() { luma(&px[i + 1]) - here } else { 0.0 };
                let dy = if y + 1 < h && i + w < px.len() { luma(&px[i + w]) - here } else { 0.0 };
                texture += weight * (dx * dx + dy * dy).sqrt() as f64;
                clipped += weight * (here <= 1.0 / 255.0 || here >= 254.0 / 255.0) as u8 as f64;
                total += weight;
            }
        }
        if total <= 1e-12 {
            (0.0, 0.0)
        } else {
            ((texture / total) as f32, (clipped / total) as f32)
        }
    };
    let ((texture_before, clipped_before), (texture_after, clipped_after)) =
        (reading(before), reading(after));
    let texture_ratio = if texture_before <= 1e-8 {
        if texture_after <= 1e-8 { 1.0 } else { f32::INFINITY }
    } else {
        texture_after / texture_before
    };
    LocalQuality {
        texture_ratio,
        clipped_before,
        clipped_after,
    }
}

const ZONE_BOUNDARY_LOW: f32 = 0.05;
const ZONE_BOUNDARY_HIGH: f32 = 0.95;
const ZONE_BOUNDARY_MID: f32 = 0.5;
const ZONE_BOUNDARY_PERCENTILE: f32 = 0.90;
const ZONE_BOUNDARY_INTERIOR_MIN: usize = 4;

#[derive(Clone, Copy, Debug)]
struct BoundaryReading {
    rim: f32,
    transitions: usize,
}

fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
}

/// Add signed readings for one row or column. A transition contributes only
/// when that SAME scan line reaches settled sky (>=95%) and settled land
/// (<=5%); this keeps a soft but one-sided mask edge from inventing an
/// interior. Within the 5%-95% run, the sky half is tested for a bright
/// overshoot against the median settled sky on that same row/column. The
/// settled land is required as the other side of a real crossing; the signed
/// sky-side amplitude deliberately matches the visible defect and the
/// supervisor's independent render metric.
fn boundary_line_rims(
    px: &[[f32; 3]],
    weights: &[f32],
    start: usize,
    step: usize,
    len: usize,
    out: &mut Vec<f32>,
) {
    let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let mut sky = Vec::new();
    let mut land = Vec::new();
    for p in 0..len {
        let i = start + p * step;
        if i >= px.len() || i >= weights.len() {
            break;
        }
        if weights[i] >= ZONE_BOUNDARY_HIGH {
            sky.push(luma(&px[i]));
        } else if weights[i] <= ZONE_BOUNDARY_LOW {
            land.push(luma(&px[i]));
        }
    }
    if sky.len() < ZONE_BOUNDARY_INTERIOR_MIN || land.len() < ZONE_BOUNDARY_INTERIOR_MIN {
        return;
    }
    let sky_settled = median(sky);
    let mut p = 0usize;
    while p < len {
        let i = start + p * step;
        if i >= weights.len()
            || !(ZONE_BOUNDARY_LOW..ZONE_BOUNDARY_HIGH).contains(&weights[i])
        {
            p += 1;
            continue;
        }
        let mut sky_max: Option<f32> = None;
        while p < len {
            let i = start + p * step;
            if i >= px.len()
                || i >= weights.len()
                || !(ZONE_BOUNDARY_LOW..ZONE_BOUNDARY_HIGH).contains(&weights[i])
            {
                break;
            }
            let here = luma(&px[i]);
            if weights[i] >= ZONE_BOUNDARY_MID {
                sky_max = Some(sky_max.map_or(here, |v| v.max(here)));
            }
            p += 1;
        }
        if let Some(sky_edge) = sky_max {
            out.push(sky_edge - sky_settled);
        }
    }
}

/// Boundary-continuity reading beside [`local_quality`]. Unlike that
/// mask-weighted in-zone average, this samples ONLY the transition band and
/// compares it with both settled interiors on the same rows/columns. The
/// signed 90th percentile is robust to isolated silhouette highlights while
/// retaining the systematic bright bow that repeats along an edge.
fn boundary_rim(
    px: &[[f32; 3]],
    weights: &[f32],
    width: u32,
    height: u32,
) -> BoundaryReading {
    let (w, h) = (width as usize, height as usize);
    let mut rims = Vec::new();
    for y in 0..h {
        boundary_line_rims(px, weights, y * w, 1, w, &mut rims);
    }
    for x in 0..w {
        boundary_line_rims(px, weights, x, w, h, &mut rims);
    }
    if rims.is_empty() {
        return BoundaryReading { rim: 0.0, transitions: 0 };
    }
    rims.sort_by(f32::total_cmp);
    let rank = ((rims.len() as f32 * ZONE_BOUNDARY_PERCENTILE).ceil() as usize)
        .saturating_sub(1)
        .min(rims.len() - 1);
    BoundaryReading { rim: rims[rank], transitions: rims.len() }
}

/// Apply one scalar to every correction in the accepted zone set. Each dial
/// is decomposed into its source-share-weighted common component plus its
/// per-zone differential; BOTH terms carry `k`, because `k=0` is required to
/// be no local correction (holding the common term would leave a full-frame
/// masked correction). Thus additive dials land at zero and gains at unity,
/// every zone keeps its fitted direction, and `k=1` is byte-for-byte the
/// candidate. The decomposition makes the common policy explicit even though
/// `k*c + k*(v-c)` deliberately simplifies to `k*v`.
fn shrink_zone_corrections(
    masks: &mut [LocalAdjustment],
    originals: &[LocalAdjustment],
    shares: &[f32],
    k: f32,
) {
    debug_assert_eq!(masks.len(), originals.len());
    debug_assert_eq!(masks.len(), shares.len());
    let k = k.clamp(0.0, 1.0);
    let share_total = shares.iter().copied().sum::<f32>().max(1e-6);
    macro_rules! shrink_additive {
        ($field:ident) => {{
            let common = originals
                .iter()
                .zip(shares)
                .map(|(m, share)| m.$field * *share)
                .sum::<f32>()
                / share_total;
            for ((dst, src), _) in masks.iter_mut().zip(originals).zip(shares) {
                dst.$field = k * common + k * (src.$field - common);
            }
        }};
    }
    shrink_additive!(exposure_ev);
    shrink_additive!(contrast);
    shrink_additive!(highlights);
    shrink_additive!(shadows);
    shrink_additive!(whites);
    shrink_additive!(blacks);
    shrink_additive!(saturation);
    for channel in 0..3 {
        let common = originals
            .iter()
            .zip(shares)
            .map(|(m, share)| (m.color_gains.unwrap_or([1.0; 3])[channel] - 1.0) * *share)
            .sum::<f32>()
            / share_total;
        for ((dst, src), _) in masks.iter_mut().zip(originals).zip(shares) {
            let fitted = src.color_gains.unwrap_or([1.0; 3])[channel] - 1.0;
            let gains = dst.color_gains.get_or_insert([1.0; 3]);
            gains[channel] = 1.0 + k * common + k * (fitted - common);
        }
    }
    if k == 0.0 {
        for mask in masks {
            mask.color_gains = None;
        }
    }
}

/// Zone-local look distance: mean |Δ| of the linear channel means plus the
/// chroma gap — the moments the fit steers, measured where the mask acts.
/// This is the yardstick the zoned do-no-harm judges by (see
/// [`ZONE_ACCEPT_RATIO`] for why the frame-global `look_err` cannot be).
pub(crate) fn zone_err(a: &ZoneMoments, b: &ZoneMoments) -> f32 {
    let mean: f32 =
        a.mean_lin.iter().zip(&b.mean_lin).map(|(x, y)| (x - y).abs()).sum::<f32>() / 3.0;
    mean + (a.chroma - b.chroma).abs()
}

/// Mask-weighted luma CDF of a zone (sRGB Rec.601 luma, the same domain as
/// the global fit's tone stage). Drives the WITHIN-zone tone solve: a zone
/// can match the target's linear MEAN and still read far darker (the real
/// pair's land: the target holds sunlit mesa tops plus deep canyon shadows —
/// a few bright pixels dominate a linear mean, while perception follows the
/// distribution). Zones correspond semantically, so quantile-to-quantile
/// mapping is identified here — unlike the per-band statistics fit.rs bans
/// on the WHOLE frame, where region correspondence is unknown.
pub(crate) fn zone_luma_cdf(px: &[[f32; 3]], weights: &[f32]) -> Vec<f32> {
    const BINS: usize = 1024;
    let mut hist = vec![0.0f32; BINS];
    let mut total = 0.0f32;
    for (p, &w) in px.iter().zip(weights) {
        if w <= 0.0 {
            continue;
        }
        let l = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        hist[(l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize] += w;
        total += w;
    }
    let total = total.max(1e-6);
    let mut acc = 0.0f32;
    for h in hist.iter_mut() {
        acc += *h;
        *h = acc / total;
    }
    hist
}

// --------------------------------------------------------------------------
// the JOINT VALUE-RANGE family (R23-6, feedback #16)
// --------------------------------------------------------------------------
//
// A companion reading of "how far apart do these two renders look",
// conditioned on luminance and chroma rather than duplicating
// `fit::look_err`. Production callers multiply its buckets by the same
// per-pixel evidence weights as the objective. Built on THIS module's
// machinery rather than beside it:
// a bucket is just another weight vector for [`zone_moments`] (whose doc has
// always said "a decoded segmentation mask, or anything else"), and the
// mismatch is [`zone_err`]. No second partition mechanism, no second set of
// moment definitions, no second thing to keep in step.
//
// WHAT IT IS, stated precisely because the naming is load-bearing: a joint
// value-range bucket holds every pixel whose LUMINANCE falls in one band AND
// whose CHROMA falls on one side of the near-neutral line. Those pixels are
// scattered across the WHOLE frame. This is NOT a spatial region and must
// never be reported as one ("worst region" would answer a question it did
// not ask); the user-facing wording is "joint distribution check". The
// spatial question is answered by the sky/land zones above.
//
// WHY IT IS NEW INFORMATION — the one thing that had to be settled before
// writing a line of it, because a reading that merely re-derives look_err is
// worth nothing. look_err's tonal term is 21 weighted luma quantiles, so a plain
// luminance-band comparison IS a coarser copy of it. But its colour term is
// three brightness-unconditional channel means, and its hue term buckets by HUE with
// every pixel under 0.06 chroma skipped outright (`fit::band_stats`) —
// neither is conditioned on brightness. "The colour balance of the
// near-neutral pixels in the shadows" and "the chroma of the coloured pixels
// in the highlights" are therefore quantities no term of look_err computes,
// and they are exactly where a split-tone, a tinted black point or a
// white-balance drift lives. Hence JOINT buckets (luma × chroma), never
// luma-only.
//
// WHAT IT MAY DO — three roles, and the boundary between them was decided by
// measurement, not by taste (the numbers live in
// `fit::tests::joint_family_is_calibrated_on_the_fixture_set`):
//   1. REPORT. Always, when it has an opinion.
//   2. CAP the reported confidence. Only downward: a reading that cannot see
//      must never raise a claim (see [`JOINT_CONFIDENCE_SLOPE`]).
//   3. ONE additional bounded-drift veto, in the shape
//      [`ZONE_GLOBAL_REGRESSION_TOL`] already has and fail-open like
//      `fit::neutral_gate_misprediction` — but at the PIPELINE END only
//      (final render vs the untouched base), never inside a stage. Tried
//      inside the cast stage first and measured: the bucket that a change
//      fixes loses its members to a neighbour, so the per-stage comparison
//      inverts, rejecting the one correct cast in the fixture set and
//      admitting both wrecks (the numbers are on [`JointReading::worst`]).
// It is never mixed into look_err's weighted sum: R17-R19's constants were
// each calibrated against a real failure pair, and re-weighting that sum
// would invalidate all of them at once.

/// Luminance bands. Four, not more: every bucket must still hold enough
/// pixels for a MEAN to be stable, and a real photograph does not spread its
/// mass evenly — eight bands routinely leave two of them under the evidence
/// floor on a normally-exposed frame, which is a reading that silently
/// abstains rather than one that is finer.
pub(crate) const JOINT_LUMA_BANDS: usize = 4;
/// Chroma classes inside each band: near-neutral, and chromatic.
pub(crate) const JOINT_CHROMA_CLASSES: usize = 2;
/// The family size — `bucket = band * JOINT_CHROMA_CLASSES + class`.
pub(crate) const JOINT_BUCKETS: usize = JOINT_LUMA_BANDS * JOINT_CHROMA_CLASSES;

/// Stable ASCII tags, in bucket-index order. They ride note args verbatim
/// (the `{label}` convention the ZONE_* notes use), so they stay English in
/// every rendering and never need a font glyph beyond ASCII.
pub(crate) const JOINT_LABELS: [&str; JOINT_BUCKETS] = [
    "shadows/neutral",
    "shadows/colour",
    "low-mids/neutral",
    "low-mids/colour",
    "high-mids/neutral",
    "high-mids/colour",
    "highlights/neutral",
    "highlights/colour",
];

/// The chroma ramp's two ends — DEFINITIONS borrowed from the two chroma
/// landmarks this codebase has already measured, not new thresholds: 0.03 is
/// `fit`'s "a pale sky still testifies" level (`VETO_SUPPORT_CHROMA` /
/// `ROT_HUE_MEASURABLE_CHROMA`) and 0.06 is the band-statistics gate
/// (`fit::band_stats`) above which a pixel is treated as carrying hue. A RAMP
/// rather than a step because a hard cut puts the whole near-grey population
/// on a cliff that the fit's own saturation dial walks pixels across.
const JOINT_CHROMA_LO: f32 = 0.03;
const JOINT_CHROMA_HI: f32 = 0.06;

/// A bucket needs this weighted share of the frame ON BOTH SIDES before its
/// moments are read. Self-standing, NOT inherited from [`MIN_ZONE_SHARE`]
/// (0.03, a segmented sky's floor): eight buckets partition unity, so a
/// perfectly ordinary frame gives several of them well under a segmented
/// region's share, and 2% of the 384-edge analysis frame is ≈ 2 900 px —
/// still 5.7× the 512-px absolute evidence floor `fit::enough_evidence`
/// demands of the tone gate's population.
const JOINT_MIN_SHARE: f32 = 0.02;

// --- the joint family's OWN acceptance ladder --------------------------------
// Independent of the fixture values and NOWHERE inherited from the
// sky/land zones (their four constants each carry a measured real-pair
// anchor for a DIFFERENT quantity and must not be borrowed — R19). Every
// The fixture test records the measurements below and enforces the policy
// that the cause, not the measurement, chooses the typed FAR note.
//
// STATUS (R24 batch 2, 2026-08-17): the policy boundary is established. R23-6
// against synthetic fixtures alone and recorded the debt in this very block
// ("wants a real-pair review before anyone treats a number here as measured
// truth"). Six real (RAW, finished JPEG) pairs off the user's own library —
// EXIF-timestamp-confirmed same frame — have since been measured through
// `autoshop match`, and they provided independent review evidence. The table is
// `fit::tests::joint_family_is_calibrated_on_the_fixture_set`'s doc; the
// The fixture and real-pair values are retained as regression evidence; they
// do not widen the boundary when a refusal happens to read farther away.
// The ONE pair the user called
// nonsense (an astro composite: the Milky Way gone, the deep blue turned
// grey) read 0.141 — under the old 0.25 line, so it raised no warning and
// still reported 0.58 confidence.
//
// The separation the fixtures established is unchanged and still holds: a
// fit that REACHES its target lands the weighted reading at 0.001-0.06,
// while the pair whose target is a repaint the global model structurally
// cannot reach lands at 0.58. The real pairs simply showed where inside
// that gap is where the fixed policy line separates the two claims.

/// The weighted reading at which reported confidence hits its floor — the
/// joint family's counterpart of `fit`'s own FAR line, and the other end of
/// the same calibration as [`JOINT_CONFIDENCE_SLOPE`].
///
/// 0.10 is the policy FAR line, shared by global and zoned reports. The
/// measured pairs below are regression evidence, not a recipe for moving it:
///   * ABOVE the line, a genuine supported miss must warn: the real astro-composite
///     pair reads 0.141 and MUST warn (it was 0.578 confidence and silent at
///     0.25 — the defect this retune closes), and the unreachable synthetic
///     repaint reads 0.581, 5.8× over.
///   * BELOW it, every fit that reached its target: the five honest real
///     pairs at 0.019/0.024/0.030/0.035/0.054 (worst = 1.85× under) and the
///     three fixtures that land at 0.001/0.004/0.045 (worst = 2.2× under).
///   * A one-sided evidence reading is a refusal, even when it is beyond the
///     line; a supported but unsuccessful reading is a miss and must warn.
///     The fixture readings remain regression evidence, not a numeric ceiling.
///     (The former measurement table is retained below for audit context.)
//
///     real failure pair arrives: the two fixtures where the solver
///     correctly REFUSED to chase a whole-scene regrade (canyon warm 0.061,
///     canyon gold 0.093). Those are policy refusals, not misses, so they
///     must not be accused of being far — and 0.093 leaves only 8% of
///     headroom. The line is deliberately biased the other way (41% of
///     headroom over the real failure): a real pair the user called nonsense
///     outranks a synthetic fixture whose silence is a judgement call.
//
pub(crate) const JOINT_FAR_ERR: f32 = 0.10;

/// One FAR-line cause classifier shared by the global and zoned fit paths.
/// A one-sided evidence refusal is deliberately farther from the target; any
/// other FAR reading is a genuine miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JointFarCause {
    Refused,
    Miss,
}

impl JointFarCause {
    pub(crate) fn note_key(self) -> &'static str {
        match self {
            Self::Refused => crate::rationale::keys::FIT_NOTE_JOINT_REFUSED,
            Self::Miss => crate::rationale::keys::FIT_NOTE_JOINT_MISS,
        }
    }
}

/// Error for the control class that remains identifiable when hue evidence is
/// one-sided.  Chroma is intentionally excluded: refusing that movement must
/// not make a supported luminance correction look like a failed zone.
fn zone_luma_err(a: &ZoneMoments, b: &ZoneMoments) -> f32 {
    (a.luma_lin - b.luma_lin).abs()
}

pub(crate) fn classify_joint_far(weighted: f32, evidence_refused: bool) -> Option<JointFarCause> {
    (weighted >= JOINT_FAR_ERR).then_some(if evidence_refused {
        JointFarCause::Refused
    } else {
        JointFarCause::Miss
    })
}
/// Confidence slope on the weighted reading: `(1 − FLOOR) / JOINT_FAR_ERR`,
/// i.e. the two ends of ONE calibration, exactly as `fit`'s
/// `CONFIDENCE_SLOPE` / `FIT_FAR_ERR` pair now is.
///
/// The tie is KEPT through this retune because the real pairs endorsed it
/// from the other end independently: the second-best pair in the set (a
/// visibly greyer-than-target rendition) reported 0.910 confidence under the
/// old slope of 3.0, i.e. the ladder was loose at the TOP as well as at the
/// FAR line. Both ends therefore had to move the same way, which is exactly
/// what one calibration with two ends means. Breaking the tie would have
/// produced the incoherent report the tie exists to prevent — "treat this as
/// a starting point, not a match" printed beside 0.70 confidence.
pub(crate) const JOINT_CONFIDENCE_SLOPE: f32 = 7.5;
/// Bounded-drift tolerance for the pipeline-end guard (role 3 above): how
/// much WORSE the finished recipe's weighted reading may be than the
/// untouched base's before the fit is declared to have done harm the
/// look-error check could not see. Every fixture in the set IMPROVES this
/// reading (0.180→0.045, 0.177→0.061, 0.243→0.093, 0.587→0.581, 0.059→0.004)
/// except the identity pair, which regresses by 0.0009 of pure
/// quantisation — so 0.05 is 56× the only observed non-improvement, and far
/// under the smallest gap between fixtures. Deliberately loose: this guard
/// exists to catch a disaster the scalar cannot see, not to referee taste.
///
/// UNCHANGED by the R24 real-pair round, and now measured rather than
/// merely assumed: all six real pairs improve this reading by a wide margin
/// (0.402→0.064, 0.332→0.064, 0.089→0.028, 0.082→0.060, 0.052→0.034,
/// 0.050→0.014 — `pipeline::tests::r16_composed_fit_on_a_real_pair`), so
/// no real pair has yet come within 0.05 of the guard from the wrong side.
pub(crate) const JOINT_DRIFT_TOL: f32 = 0.05;

/// One bucket's mismatch: [`zone_err`]'s formula, read in the DISPLAY
/// domain.
///
/// `zone_err` compares linear-light channel means, and this module has
/// already written down what that costs (see [`ZONE_MATCHED_EV`]: sRGB 0.12
/// and 0.173 score the same 0.012 while sitting 0.9 EV apart). The sky/land
/// zones answer that with an EV companion on a single absolute line, because
/// there is one zone at one level. Here the buckets are DEFINED at different
/// brightness levels, so a linear-absolute error would hand "the worst
/// bucket" to the highlights in every photograph ever taken, and dividing by
/// the level instead (tried first, measured) hands it to the shadows just as
/// mechanically — a 4/255 difference in a black bucket reads 0.8 there.
/// Encoding both means to sRGB before differencing is the fix with a reason:
/// it is the domain `look_err`'s own channel-mean term uses, so 4/255 means
/// 4/255 wherever it happens, and the chroma term (already sRGB) needs no
/// separate treatment. Same two terms, same weights, same shape as
/// [`zone_err`] — only the domain differs, and it differs on purpose.
fn joint_bucket_err(s: &ZoneMoments, t: &ZoneMoments) -> f32 {
    let enc = |v: f32| render::linear_to_srgb(v.clamp(0.0, 1.0));
    let mean = (0..3)
        .map(|c| (enc(s.mean_lin[c]) - enc(t.mean_lin[c])).abs())
        .sum::<f32>()
        / 3.0;
    mean + (s.chroma - t.chroma).abs()
}

/// The weight vector of one joint bucket, written into `out` (reused across
/// buckets — eight full `Vec<f32>` per side is 4.7 MB of nothing).
///
/// The luminance half is a tent partition of unity over
/// [`JOINT_LUMA_BANDS`] centres, flat past the outer two, so the eight
/// buckets' weights sum to exactly 1 for every pixel and the shares are
/// readable as frame fractions. The chroma half is the ramp described at
/// [`JOINT_CHROMA_LO`].
fn joint_weights(px: &[[f32; 3]], bucket: usize, out: &mut Vec<f32>) {
    let band = bucket / JOINT_CHROMA_CLASSES;
    let chromatic = bucket % JOINT_CHROMA_CLASSES == 1;
    let n = JOINT_LUMA_BANDS as f32;
    let centre = (band as f32 + 0.5) / n;
    out.clear();
    out.reserve(px.len());
    for p in px {
        let l = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        let below = band == 0 && l < centre;
        let above = band + 1 == JOINT_LUMA_BANDS && l > centre;
        let lw = if below || above { 1.0 } else { (1.0 - (l - centre).abs() * n).clamp(0.0, 1.0) };
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        let cw = ((chroma - JOINT_CHROMA_LO) / (JOINT_CHROMA_HI - JOINT_CHROMA_LO)).clamp(0.0, 1.0);
        out.push(lw * if chromatic { cw } else { 1.0 - cw });
    }
}

/// One qualifying bucket of the joint family.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointBucket {
    pub label: &'static str,
    /// [`joint_bucket_err`] between the candidate's and the target's members.
    pub err: f32,
    /// The share the two sides AGREE on (the smaller of the two): a bucket
    /// holding 30% of the target and 3% of the candidate is thin evidence,
    /// and taking the minimum says so without needing a second rule.
    pub share: f32,
    /// The chroma class this bucket belongs to. Exposed because the
    /// difference between "the coloured pixels of this brightness disagree"
    /// and "the near-grey ones do" is the whole conditional information the
    /// family was built to produce: the first is a colour move (a per-band
    /// mixer, a split tone), the second is a white balance or a tinted
    /// black point. `fit::unrepresented_note` reads exactly that.
    pub chromatic: bool,
}

/// Legacy unweighted bucket reading. Production fit consumers call
/// [`joint_buckets_with_evidence`] with the shared evidence model.
pub fn joint_buckets(cand: &[[f32; 3]], tgt: &[[f32; 3]]) -> Vec<JointBucket> {
    joint_buckets_with_evidence(cand, tgt, None, None)
}

pub(crate) fn joint_buckets_with_evidence(
    cand: &[[f32; 3]],
    tgt: &[[f32; 3]],
    cand_evidence: Option<&[f32]>,
    tgt_evidence: Option<&[f32]>,
) -> Vec<JointBucket> {
    if !joint_family_enabled() {
        return Vec::new();
    }
    let mut wa: Vec<f32> = Vec::new();
    let mut wb: Vec<f32> = Vec::new();
    let mut out = Vec::with_capacity(JOINT_BUCKETS);
    for (b, label) in JOINT_LABELS.iter().enumerate() {
        joint_weights(cand, b, &mut wa);
        joint_weights(tgt, b, &mut wb);
        if let Some(evidence) = cand_evidence {
            for (weight, &gate) in wa.iter_mut().zip(evidence) {
                *weight *= gate.max(0.0);
            }
        }
        if let Some(evidence) = tgt_evidence {
            for (weight, &gate) in wb.iter_mut().zip(evidence) {
                *weight *= gate.max(0.0);
            }
        }
        let ms = zone_moments(cand, &wa);
        let mt = zone_moments(tgt, &wb);
        if ms.share < JOINT_MIN_SHARE || mt.share < JOINT_MIN_SHARE {
            continue;
        }
        out.push(JointBucket {
            label,
            err: joint_bucket_err(&ms, &mt),
            share: ms.share.min(mt.share),
            chromatic: b % JOINT_CHROMA_CLASSES == 1,
        });
    }
    out
}

/// The joint-family reading of one (candidate, target) pixel pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointReading {
    /// The largest [`joint_bucket_err`] among the qualifying buckets.
    ///
    /// REPORT-ONLY, never a gate. Measured on this repo's own fixtures
    /// (`fit::tests::joint_family_is_calibrated_on_the_fixture_set`): a
    /// change that fixes a bucket can push its members OUT of the bucket, so
    /// the surviving worst is not comparable across an edit — on the haze
    /// pair the correctly-accepted cast curves move it 0.073 → 0.082
    /// (worse) while on the two canyon pairs the curves that MUST be
    /// rejected move it 0.389 → 0.115 and 0.098 → 0.133 with the qualifying
    /// count collapsing 7 → 4. A "worst bucket must not get worse" veto
    /// would therefore reject the one correct cast in the set and admit the
    /// wrecks. This is the failure this module already recorded from the
    /// other side (see [`ZONE_ACCEPT_RATIO`]: "a correct blue→gold repaint
    /// migrates band mass, which the worst-band hue term can only read as
    /// damage"), reproduced first-hand on the new reading.
    pub worst: f32,
    /// Which bucket that was ([`JOINT_LABELS`]).
    pub worst_label: &'static str,
    /// Share-weighted mean over the qualifying buckets — the stable half,
    /// and the only one anything decides on.
    pub weighted: f32,
    /// How many of the [`JOINT_BUCKETS`] qualified.
    pub buckets: usize,
}

/// Read the joint value-range family for a candidate render against a target.
///
/// `None` — the FAIL-OPEN answer — when no bucket clears [`JOINT_MIN_SHARE`]
/// on both sides (a monochrome frame, a target sharing no value range with
/// the source) or when the family is switched off. Every caller must treat
/// `None` as "this reading has no opinion", never as "no problem": that is
/// the failure direction `fit::neutral_gate_misprediction` chose, and for
/// the same reason — a reading that cannot see is not evidence of health.
///
/// The two slices need NOT be the same length: buckets correspond by VALUE,
/// not by position, which is what makes this reading immune to the
/// composition differences that defeat frame-global distribution matching
/// (a generative target holding ~3× the sky area, measured — see
/// [`ZONE_ACCEPT_RATIO`]).
pub fn joint_reading(cand: &[[f32; 3]], tgt: &[[f32; 3]]) -> Option<JointReading> {
    let buckets = joint_buckets(cand, tgt);
    joint_reading_from_buckets(&buckets)
}

pub(crate) fn joint_reading_with_evidence(
    cand: &[[f32; 3]],
    tgt: &[[f32; 3]],
    cand_evidence: &[f32],
    tgt_evidence: &[f32],
) -> Option<JointReading> {
    let buckets = joint_buckets_with_evidence(
        cand,
        tgt,
        Some(cand_evidence),
        Some(tgt_evidence),
    );
    joint_reading_from_buckets(&buckets)
}

fn joint_reading_from_buckets(buckets: &[JointBucket]) -> Option<JointReading> {
    if buckets.is_empty() {
        return None;
    }
    let mut worst = 0.0f32;
    let mut worst_label = buckets[0].label;
    let mut acc = 0.0f64;
    let mut acc_w = 0.0f64;
    for b in buckets {
        acc += b.err as f64 * b.share as f64;
        acc_w += b.share as f64;
        if b.err > worst {
            worst = b.err;
            worst_label = b.label;
        }
    }
    Some(JointReading {
        worst,
        worst_label,
        weighted: if acc_w > 0.0 { (acc / acc_w) as f32 } else { 0.0 },
        buckets: buckets.len(),
    })
}

/// The comparison path (R23-6 E-15): `AUTOSHOP_FIT_JOINT=off` takes the
/// whole family out of the fit, so the R17-R19 baseline numbers can be
/// reproduced against the same binary instead of against a memory. Read
/// ONCE per process — the fit's "deterministic" contract is about its
/// arguments, and a switch that could flip between two calls of the same run
/// would not be.
///
/// With it off, [`joint_buckets`] returns empty and [`joint_reading`] `None`,
/// and each of the FOUR consumers degrades to exactly the pre-R23-6 behaviour:
///   1. the fit report's joint note — gone, replaced by the fail-open
///      disclosure (`FIT_NOTE_JOINT_NONE`), which is the point of that note;
///   2. the joint confidence cap (`fit::compose_report`, i.e. every fit report
///      and every `fit::rescore_report`) — gone; the look-error ladder and
///      shared evidence-identifiability cap remain;
///   3. the terminal do-no-harm veto's joint arm (`fit::terminal_harm`) — its
///      `(Some, Some)` match never fires, so only R16's scalar arm convicts;
///   4. `fit::unrepresented_note`, the one consumer that reads
///      [`joint_buckets`] DIRECTLY rather than through [`joint_reading`] — its
///      colour-shaped route sees an empty bucket list and can never fire, so
///      the note falls back to the band-centroid route and the channel-mean
///      white-balance test, both of which are joint-independent. That is a
///      QUIETER disclosure, not a wrong one, and it was missing from this list.
///
/// Verified by running the suite under it — every `fit` / `fit_zoned` test that
/// does not assert this family's existence passes unchanged (41 of them as of
/// R23's round review); the five that fail are exactly the ones asserting it
/// EXISTS, which is the correct answer to switching it off and is why the
/// variable is a diagnostic, not a supported test configuration.
fn joint_family_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            std::env::var("AUTOSHOP_FIT_JOINT").as_deref().map(str::trim),
            Ok("off") | Ok("0") | Ok("false")
        )
    })
}

// --------------------------------------------------------------------------
// orchestration
// --------------------------------------------------------------------------

/// The zoned reverse-fit: the global [`fit::fit_recipe`] first, then a
/// sky-to-sky zone correction on top — segment the sky in BOTH images
/// (`seg`, the same sidecar the GUI's mask panel uses), compare zone moments,
/// attach a Bitmap-masked [`LocalAdjustment`] when it measurably helps.
///
/// `mask_path` is where the SOURCE sky mask lands (the recipe references
/// it). Pass a FRESHLY CLAIMED raster name (`store::claim_raster`, prefix
/// `mask-zone-sky`, in the photo's develop dir — what the CLI and the GUI
/// both do), never a shared or pre-existing raster: when every zone is
/// skipped or rejected the file at `mask_path` is REMOVED to release the
/// claim, which would destroy a raster another mask still references (an
/// older version of this doc named the AI-select `out/<stem>.mask-sky.png`
/// convention — exactly such a shared raster).
/// GRACEFUL BY CONTRACT: segmentation missing/failing, a
/// degenerate sky, or a correction that does not improve the look all fall
/// back to the plain global fit with an honest rationale note — never an
/// error, because the global fit in hand is already a valid result.
pub fn fit_recipe_zoned(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
) -> FitReport {
    fit_recipe_zoned_from(src, target, seg, mask_path, &crate::recipe::EditRecipe::default())
}

/// [`fit_recipe_zoned`] with a correspondence provider (step 7b) — see
/// [`fit::fit_recipe_with`]. `None` is bit-for-bit the plain zoned fit.
pub fn fit_recipe_zoned_with(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    provider: Option<fit::CorrespondenceProvider>,
) -> FitReport {
    fit_recipe_zoned_inner(src, target, seg, mask_path, base, provider)
}

/// [`fit_recipe_zoned`] with a calibration-only base composed into the
/// solve — see [`fit::fit_recipe_from`]. The zone passes need no changes
/// of their own: every zone statistic is measured on a render of the
/// CURRENT recipe (which carries the base from the start), so the zones
/// already live in the canvas's one-pass domain.
pub fn fit_recipe_zoned_from(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
) -> FitReport {
    fit_recipe_zoned_inner(src, target, seg, mask_path, base, None)
}

fn fit_recipe_zoned_inner(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    provider: Option<fit::CorrespondenceProvider>,
) -> FitReport {
    match segment_both(src, target, seg, mask_path) {
        Ok((src_mask, tgt_mask)) => {
            let zone_divergence = measure_zone_divergence(src, target, base, &src_mask);
            let divergent_cover = [zone_divergence.sky, zone_divergence.land]
                .into_iter()
                .filter(|zone| zone.divergence.d >= fit::DIVERGENCE_ZONE)
                .map(|zone| zone.share)
                .sum::<f32>();
            let mut report = fit::fit_recipe_from_promoted_with_disclosure(
                src,
                target,
                base,
                divergent_cover >= fit::DIVERGENT_COVER_PROMOTES,
                true,
                provider,
            );
            attach_zones_with_divergence(
                src,
                target,
                &mut report,
                &src_mask,
                &tgt_mask,
                mask_path,
                zone_divergence,
            );
            report
        }
        Err(e) => {
            // The provider still rides the fallback: a failed segmentation
            // must not also cost the global fit its correspondence.
            let mut report = fit::fit_recipe_from_promoted_with_disclosure(
                src,
                target,
                base,
                false,
                true,
                provider,
            );
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONED_UNAVAILABLE,
                    vec![("e", format!("{e:#}"))],
                ),
            );
            attach_luminance_ranges(src, target, &mut report);
            report
        }
    }
}

#[derive(Clone, Copy)]
struct ZoneDivergence {
    divergence: fit::Divergence,
    share: f32,
}

#[derive(Clone, Copy)]
struct ZoneDivergences {
    sky: ZoneDivergence,
    land: ZoneDivergence,
}

/// One estimator input owns both populations and the adjustment shape it may
/// emit. Estimator dispatch never depends on [`MaskRole`]: a persisted
/// Custom-role luminance band is as safe to re-fit as a semantic bitmap zone.
#[derive(Clone, Debug, PartialEq)]
struct ZoneAttachment {
    source_weights: Vec<f32>,
    target_weights: Vec<f32>,
    mask: MaskGeometry,
    range: Option<RangeMask>,
    name: String,
    role: MaskRole,
    inverted: bool,
    label: String,
    frame_regression_tol: f32,
}

#[derive(Clone, Debug)]
struct ResidualRun {
    first: usize,
    last: usize,
    target_first: usize,
    target_last: usize,
    residual: f32,
    score: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct RangeBand {
    attachment: ZoneAttachment,
    source: RangeMask,
    target: RangeMask,
    divergence: fit::Divergence,
}

#[derive(Clone, Debug, PartialEq)]
struct RangeAbstention {
    lo: f32,
    hi: f32,
    reason: String,
}

#[derive(Clone, Debug, PartialEq)]
struct RangeMerge {
    lo: f32,
    hi: f32,
    into_lo: f32,
    into_hi: f32,
    sign: &'static str,
}

#[derive(Debug, Default, PartialEq)]
struct RangeDerivation {
    bands: Vec<RangeBand>,
    abstentions: Vec<RangeAbstention>,
    merges: Vec<RangeMerge>,
}

#[cfg(test)]
thread_local! {
    static RANGE_DERIVATION_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RANGE_FRESH_RENDER_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RANGE_FINAL_FRAME_ERR_OVERRIDE: std::cell::Cell<Option<f32>> =
        const { std::cell::Cell::new(None) };
    static SEGMENT_BOTH_OVERRIDE: std::cell::RefCell<Option<(GrayImage, GrayImage)>> =
        const { std::cell::RefCell::new(None) };
}

fn display_luma(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

fn range_weights_for_pixels(range: &RangeMask, pixels: &[[f32; 3]]) -> Vec<f32> {
    pixels.iter().map(|p| render::range_weight(range, p)).collect()
}

fn normalize_partition_weights(weights: &mut [Vec<f32>]) {
    let n = weights.iter().map(Vec::len).min().unwrap_or(0);
    for i in 0..n {
        let total = weights.iter().map(|band| band[i]).sum::<f32>();
        if total > 1.0 {
            for band in &mut *weights {
                band[i] /= total;
            }
        }
    }
}

fn evidence_refusal(e: &fit::EvidenceRange) -> Option<String> {
    if !e.source_populated || e.source_share < RANGE_MIN_EVIDENCE_SHARE {
        Some(format!(
            "source population {:.1}% is below the {:.1}% evidence floor",
            e.source_share * 100.0,
            RANGE_MIN_EVIDENCE_SHARE * 100.0,
        ))
    } else if !e.target_populated || e.target_share < RANGE_MIN_EVIDENCE_SHARE {
        Some(format!(
            "target population {:.1}% is below the {:.1}% evidence floor",
            e.target_share * 100.0,
            RANGE_MIN_EVIDENCE_SHARE * 100.0,
        ))
    } else if e.two_sided_share <= 0.0 {
        Some("the paired population has zero two-sided evidence".to_string())
    } else if e.weight <= 0.0 {
        Some("the paired population has zero structural evidence".to_string())
    } else {
        None
    }
}

fn target_range_for_ranks(
    target_luma: &[f32],
    first: usize,
    last: usize,
) -> RangeMask {
    let n = target_luma.len();
    let first = first.min(n.saturating_sub(1));
    let last = last.max(first + 1).min(n);
    let lo = target_luma.get(first).copied().unwrap_or(0.0);
    let hi = target_luma.get(last - 1).copied().unwrap_or(lo);
    RangeMask::Luminance {
        lo_outer: (lo - RANGE_MAX_RAMP).max(0.0),
        lo,
        hi,
        hi_outer: (hi + RANGE_MAX_RAMP).min(1.0),
    }
}

fn source_ranges_for_runs(runs: &[ResidualRun]) -> Vec<RangeMask> {
    runs.iter()
        .enumerate()
        .map(|(i, run)| {
            let lo = run.first as f32 / 17.0;
            let hi = (run.last + 1) as f32 / 17.0;
            let left_gap = i
                .checked_sub(1)
                .map(|j| lo - (runs[j].last + 1) as f32 / 17.0)
                .unwrap_or(RANGE_MAX_RAMP);
            let right_gap = runs
                .get(i + 1)
                .map(|next| next.first as f32 / 17.0 - hi)
                .unwrap_or(RANGE_MAX_RAMP);
            let ramp = |gap: f32| gap.clamp(RANGE_MIN_RAMP, RANGE_MAX_RAMP);
            RangeMask::Luminance {
                lo_outer: (lo - ramp(left_gap)).max(0.0),
                lo,
                hi,
                hi_outer: (hi + ramp(right_gap)).min(1.0),
            }
        })
        .collect()
}

/// Derive coherent residual runs from the current rendered state. Target
/// populations are monotone rank partners, matching the fixed evidence model;
/// no spatial alignment is assumed.
fn derive_luminance_bands(
    source_px: &[[f32; 3]],
    target_px: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
) -> RangeDerivation {
    #[cfg(test)]
    RANGE_DERIVATION_CALLS.with(|calls| calls.set(calls.get() + 1));

    if source_px.is_empty() || target_px.is_empty() || evidence.luma.len() != 17 {
        return RangeDerivation::default();
    }
    let mut target_luma = target_px.iter().map(display_luma).collect::<Vec<_>>();
    target_luma.sort_by(f32::total_cmp);

    let mut bin_residual = [0.0f32; 17];
    let mut bin_count = [0usize; 17];
    for p in &evidence.source_pixels {
        bin_count[fit::evidence_luma_bin(display_luma(p))] += 1;
    }
    let mut target_cursor = 0usize;
    let mut target_bounds = [(0usize, 0usize); 17];
    for bin in 0..17 {
        let current_values = evidence
            .source_pixels
            .iter()
            .zip(source_px)
            .filter(|(base, _)| fit::evidence_luma_bin(display_luma(base)) == bin)
            .map(|(_, current)| display_luma(current))
            .collect::<Vec<_>>();
        let last = (target_cursor + bin_count[bin]).min(target_luma.len());
        target_bounds[bin] = (target_cursor.min(target_luma.len()), last);
        let target_values = &target_luma[target_cursor.min(target_luma.len())..last];
        if !current_values.is_empty() && !target_values.is_empty() {
            let current_mean = current_values.iter().sum::<f32>() / current_values.len() as f32;
            let target_mean = target_values.iter().sum::<f32>() / target_values.len() as f32;
            bin_residual[bin] = target_mean - current_mean;
        }
        target_cursor = last;
    }

    let mut out = RangeDerivation::default();
    let mut candidate = [false; 17];
    for bin in 0..17 {
        if bin_count[bin] == 0 || bin_residual[bin].abs() < RANGE_RESIDUAL_TRIGGER {
            continue;
        }
        if let Some(reason) = evidence_refusal(&evidence.luma[bin]) {
            out.abstentions.push(RangeAbstention {
                lo: bin as f32 / 17.0,
                hi: (bin + 1) as f32 / 17.0,
                reason,
            });
        } else {
            candidate[bin] = true;
        }
    }

    let mut runs = Vec::<ResidualRun>::new();
    let mut bin = 0usize;
    while bin < 17 {
        if !candidate[bin] {
            bin += 1;
            continue;
        }
        let first = bin;
        let sign = bin_residual[bin].is_sign_positive();
        let mut last = bin;
        while last + 1 < 17
            && candidate[last + 1]
            && bin_residual[last + 1].is_sign_positive() == sign
        {
            let weighted = (first..=last + 1)
                .map(|b| {
                    let w = evidence.luma[b].two_sided_share.max(1e-6);
                    (bin_residual[b] * w, w)
                })
                .fold((0.0f32, 0.0f32), |(rs, ws), (r, w)| (rs + r, ws + w));
            if weighted.1 <= 0.0 || weighted.0.is_sign_positive() != sign {
                break;
            }
            last += 1;
        }
        let qualifies = last > first
            || (first..=last).any(|b| bin_residual[b].abs() >= 2.0 * RANGE_RESIDUAL_TRIGGER);
        if qualifies {
            let verdict = fit::luma_evidence_for_bins(evidence, first, last);
            let residual_weight = verdict.two_sided_share.max(1e-6);
            let residual = (first..=last)
                .map(|b| bin_residual[b] * evidence.luma[b].two_sided_share.max(1e-6))
                .sum::<f32>()
                / (first..=last)
                    .map(|b| evidence.luma[b].two_sided_share.max(1e-6))
                    .sum::<f32>();
            runs.push(ResidualRun {
                first,
                last,
                target_first: target_bounds[first].0,
                target_last: target_bounds[last].1,
                residual,
                score: residual.abs() * residual_weight,
            });
        }
        bin = last + 1;
    }

    let mut valid = Vec::new();
    for run in runs {
        let verdict = fit::luma_evidence_for_bins(evidence, run.first, run.last);
        if let Some(reason) = evidence_refusal(&verdict) {
            out.abstentions.push(RangeAbstention {
                lo: run.first as f32 / 17.0,
                hi: (run.last + 1) as f32 / 17.0,
                reason,
            });
        } else {
            valid.push(run);
        }
    }

    if valid.len() > RANGE_MAX_BANDS {
        let mut ranked = (0..valid.len()).collect::<Vec<_>>();
        ranked.sort_by(|&a, &b| {
            valid[b]
                .score
                .total_cmp(&valid[a].score)
                .then_with(|| valid[a].first.cmp(&valid[b].first))
        });
        let mut keep = ranked[..RANGE_MAX_BANDS].to_vec();
        for positive in [false, true] {
            if valid.iter().any(|r| r.residual.is_sign_positive() == positive)
                && !keep
                    .iter()
                    .any(|&i| valid[i].residual.is_sign_positive() == positive)
            {
                let replacement = ranked
                    .iter()
                    .copied()
                    .find(|&i| valid[i].residual.is_sign_positive() == positive)
                    .expect("sign was observed");
                keep.pop();
                keep.push(replacement);
            }
        }
        keep.sort_unstable();
        keep.dedup();
        let dropped = (0..valid.len()).filter(|i| !keep.contains(i)).collect::<Vec<_>>();
        for drop in dropped {
            let nearest = keep
                .iter()
                .copied()
                .filter(|&i| {
                    valid[i].residual.is_sign_positive()
                        == valid[drop].residual.is_sign_positive()
                })
                .filter(|&into| {
                    let lo = valid[into].first.min(valid[drop].first);
                    let hi = valid[into].last.max(valid[drop].last);
                    keep.iter().copied().filter(|&other| other != into).all(|other| {
                        valid[other].last < lo || valid[other].first > hi
                    })
                })
                .min_by_key(|&i| valid[i].first.abs_diff(valid[drop].first));
            let old = valid[drop].clone();
            if let Some(nearest) = nearest {
                valid[nearest].first = valid[nearest].first.min(old.first);
                valid[nearest].last = valid[nearest].last.max(old.last);
                valid[nearest].target_first = valid[nearest].target_first.min(old.target_first);
                valid[nearest].target_last = valid[nearest].target_last.max(old.target_last);
                out.merges.push(RangeMerge {
                    lo: old.first as f32 / 17.0,
                    hi: (old.last + 1) as f32 / 17.0,
                    into_lo: valid[nearest].first as f32 / 17.0,
                    into_hi: (valid[nearest].last + 1) as f32 / 17.0,
                    sign: if old.residual.is_sign_positive() { "positive" } else { "negative" },
                });
            } else {
                out.abstentions.push(RangeAbstention {
                    lo: old.first as f32 / 17.0,
                    hi: (old.last + 1) as f32 / 17.0,
                    reason: "the four-band cap found no adjacent same-sign merge that preserved every retained core"
                        .to_string(),
                });
            }
        }
        valid = keep.into_iter().map(|i| valid[i].clone()).collect();
    }
    valid.sort_by_key(|run| run.first);
    debug_assert!(valid.windows(2).all(|pair| pair[0].last < pair[1].first));
    let source_ranges = source_ranges_for_runs(&valid);
    let target_ranges = valid
        .iter()
        .map(|run| target_range_for_ranks(&target_luma, run.target_first, run.target_last))
        .collect::<Vec<_>>();
    let mut source_weights = source_ranges
        .iter()
        .map(|range| range_weights_for_pixels(range, source_px))
        .collect::<Vec<_>>();
    normalize_partition_weights(&mut source_weights);
    for (i, run) in valid.into_iter().enumerate() {
        let verdict = fit::luma_evidence_for_bins(evidence, run.first, run.last);
        let d = if verdict.divergence.is_finite() { verdict.divergence } else { 1.0 };
        let source = source_ranges[i];
        let target = target_ranges[i];
        let name = format!("Luminance range {:02}", i + 1);
        out.bands.push(RangeBand {
            attachment: ZoneAttachment {
                source_weights: std::mem::take(&mut source_weights[i]),
                target_weights: Vec::new(),
                mask: RANGE_HOST,
                range: Some(source),
                name: name.clone(),
                role: MaskRole::Custom,
                inverted: false,
                label: name,
                frame_regression_tol: RANGE_FRAME_REGRESSION_TOL,
            },
            source,
            target,
            divergence: fit::Divergence {
                correlation: (1.0 - d).clamp(-1.0, 1.0),
                energy_error: 0.0,
                d,
            },
        });
    }
    out
}

fn range_transition_rim(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    ranges: &[RangeMask],
    width: u32,
    height: u32,
) -> BoundaryReading {
    let (w, h) = (width as usize, height as usize);
    let transitions = ranges
        .iter()
        .flat_map(|range| match *range {
            RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
                [(lo_outer, lo), (hi, hi_outer)]
            }
            RangeMask::Color { .. } => [(f32::NAN, f32::NAN); 2],
        })
        .filter(|(a, b)| a.is_finite() && b.is_finite() && b > a)
        .collect::<Vec<_>>();
    let mut rims = vec![Vec::new(); transitions.len()];
    let mut sample_pair = |a: usize, b: usize| {
        let Some((ra, rb, pa, pb)) = reference
            .get(a)
            .zip(reference.get(b))
            .zip(rendered.get(a).zip(rendered.get(b)))
            .map(|((ra, rb), (pa, pb))| (ra, rb, pa, pb))
        else {
            return;
        };
        let (la, lb) = (display_luma(ra), display_luma(rb));
        // A range rim is a bow in a locally smooth value crossing, not a
        // pre-existing subject edge. Two-and-a-half 8-bit levels preserves
        // the retained smooth-gradient stress while excluding real edges.
        if (la - lb).abs() > 2.5 / 255.0 {
            return;
        }
        let middle = (la + lb) * 0.5;
        let rendered_a = display_luma(pa);
        let rendered_b = display_luma(pb);
        let signed_bow = if la <= lb {
            rendered_b - rendered_a
        } else {
            rendered_a - rendered_b
        };
        for (transition, &(outer, inner)) in transitions.iter().enumerate() {
            if (outer..=inner).contains(&middle) {
                rims[transition].push(signed_bow);
            }
        }
    };
    for y in 0..h {
        for x in 1..w {
            sample_pair(y * w + x - 1, y * w + x);
        }
    }
    for y in 1..h {
        for x in 0..w {
            sample_pair((y - 1) * w + x, y * w + x);
        }
    }
    let transition_count = rims.iter().map(Vec::len).sum();
    if transition_count == 0 {
        return BoundaryReading { rim: 0.0, transitions: 0 };
    }
    let rim = rims
        .into_iter()
        .filter(|values| !values.is_empty())
        .map(|mut values| {
            // Budget the signed samples by magnitude so bright and dark bows
            // cannot cancel or hide on opposite sides of a range transition.
            values.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
            let rank = ((values.len() as f32 * ZONE_BOUNDARY_PERCENTILE).ceil() as usize)
                .saturating_sub(1)
                .min(values.len() - 1);
            values[rank].abs()
        })
        .fold(0.0f32, f32::max);
    BoundaryReading { rim, transitions: transition_count }
}

fn range_boundary_note_args(
    n: usize,
    k: f32,
    before: BoundaryReading,
    after: BoundaryReading,
) -> Vec<(&'static str, String)> {
    vec![
        ("n", n.to_string()),
        ("k", format!("{k:.3}")),
        ("before", format!("{:.3}", before.rim)),
        ("after", format!("{:.3}", after.rim)),
        ("max", format!("{RANGE_BOUNDARY_RIM_MAX:.3}")),
        ("transitions", after.transitions.to_string()),
    ]
}

fn enforce_range_boundary_gate(
    s_img: &DynamicImage,
    report: &mut FitReport,
    reference: &[[f32; 3]],
    ranges: &[RangeMask],
    correction_shares: &[f32],
    first_range: usize,
    initial_px: Vec<[f32; 3]>,
) -> BoundaryGateResult {
    let initial = range_transition_rim(
        reference,
        &initial_px,
        ranges,
        s_img.width(),
        s_img.height(),
    );
    let range_count = report.recipe.masks.len().saturating_sub(first_range);
    if initial.rim <= RANGE_BOUNDARY_RIM_MAX {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_BOUNDARY_PASSED,
                range_boundary_note_args(range_count, 1.0, initial, initial),
            ),
        );
        return BoundaryGateResult::Kept {
            k: 1.0,
            before: initial,
            after: initial,
            pixels: initial_px,
        };
    }

    let originals = report.recipe.masks[first_range..].to_vec();
    debug_assert_eq!(originals.len(), correction_shares.len());
    let render_at = |report: &mut FitReport, k: f32| -> (BoundaryReading, Vec<[f32; 3]>) {
        shrink_zone_corrections(
            &mut report.recipe.masks[first_range..],
            &originals,
            correction_shares,
            k,
        );
        let pixels = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let reading = range_transition_rim(
            reference,
            &pixels,
            ranges,
            s_img.width(),
            s_img.height(),
        );
        (reading, pixels)
    };
    let (zero, zero_px) = render_at(report, 0.0);
    if zero.rim > RANGE_BOUNDARY_RIM_MAX {
        report.recipe.masks.truncate(first_range);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_BOUNDARY_REFUSED,
                range_boundary_note_args(range_count, 0.0, initial, zero),
            ),
        );
        return BoundaryGateResult::Dropped;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut best = (zero, zero_px);
    for _ in 0..12 {
        let mid = (lo + hi) * 0.5;
        let measured = render_at(report, mid);
        if measured.0.rim <= RANGE_BOUNDARY_RIM_MAX {
            lo = mid;
            best = measured;
        } else {
            hi = mid;
        }
    }
    shrink_zone_corrections(
        &mut report.recipe.masks[first_range..],
        &originals,
        correction_shares,
        lo,
    );
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::RANGE_BOUNDARY_PASSED,
            range_boundary_note_args(range_count, lo, initial, best.0),
        ),
    );
    BoundaryGateResult::Kept { k: lo, before: initial, after: best.0, pixels: best.1 }
}

fn push_range_abstention(report: &mut FitReport, abstention: &RangeAbstention) {
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::RANGE_ABSTAINED,
            vec![
                ("lo", format!("{:.3}", abstention.lo)),
                ("hi", format!("{:.3}", abstention.hi)),
                ("reason", abstention.reason.clone()),
            ],
        ),
    );
}

fn range_weights_from_current_render(
    s_img: &DynamicImage,
    recipe: &crate::recipe::EditRecipe,
    ranges: &[RangeMask],
) -> Vec<Vec<f32>> {
    #[cfg(test)]
    RANGE_FRESH_RENDER_CALLS.with(|calls| calls.set(calls.get() + 1));

    let current = fit::pixels_of(&render::develop_preview(s_img, recipe));
    let mut weights = ranges
        .iter()
        .map(|range| range_weights_for_pixels(range, &current))
        .collect::<Vec<_>>();
    normalize_partition_weights(&mut weights);
    weights
}

fn final_range_frame_err(
    pixels: &[[f32; 3]],
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
) -> f32 {
    let measured = fit::look_err_with_evidence(pixels, target, evidence);
    #[cfg(test)]
    {
        RANGE_FINAL_FRAME_ERR_OVERRIDE.with(|value| value.take().unwrap_or(measured))
    }
    #[cfg(not(test))]
    {
        measured
    }
}

/// Automatic pure-Rust fallback after the global fit. Bands are attempted in
/// ascending luma order, and every attempt derives its source population from
/// the current rendered stack rather than the untouched source.
fn attach_luminance_ranges(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
) {
    let s_img = src.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let t_img = target.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let tgt_px = fit::pixels_of(&t_img);
    let global_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    // Preserve the global stage's own reported frame metric as the ceiling;
    // every accepted range must be no worse than the recipe already handed
    // to this fallback.
    let global_frame_err = report.err_after;
    let mut derived = derive_luminance_bands(&global_px, &tgt_px, &report.evidence);
    for merged in &derived.merges {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_MERGED,
                vec![
                    ("lo", format!("{:.3}", merged.lo)),
                    ("hi", format!("{:.3}", merged.hi)),
                    ("into_lo", format!("{:.3}", merged.into_lo)),
                    ("into_hi", format!("{:.3}", merged.into_hi)),
                    ("sign", merged.sign.to_string()),
                ],
            ),
        );
    }
    for abstention in &derived.abstentions {
        push_range_abstention(report, abstention);
    }
    if derived.bands.is_empty() {
        fit::append_finished_disclosure(report, &global_px, &tgt_px);
        return;
    }

    let all_ranges = derived.bands.iter().map(|band| band.source).collect::<Vec<_>>();
    let mut target_weights = derived
        .bands
        .iter()
        .map(|band| range_weights_for_pixels(&band.target, &tgt_px))
        .collect::<Vec<_>>();
    normalize_partition_weights(&mut target_weights);
    for (band, weights) in derived.bands.iter_mut().zip(target_weights) {
        band.attachment.target_weights = weights;
    }
    let first_range = report.recipe.masks.len();
    let mut frame_err = report.err_after;
    let corr = report.correspondence.take();
    let mut accepted = Vec::new();
    for i in 0..derived.bands.len() {
        let mut current_weights =
            range_weights_from_current_render(&s_img, &report.recipe, &all_ranges);
        derived.bands[i].attachment.source_weights = std::mem::take(&mut current_weights[i]);
        let accepted_band = attach_one_zone(
            &s_img,
            &tgt_px,
            report,
            &mut frame_err,
            &derived.bands[i].attachment,
            derived.bands[i].divergence,
            corr.as_ref(),
        );
        match accepted_band {
            Some(zone) => accepted.push(zone),
            None => push_range_abstention(
                report,
                &RangeAbstention {
                    lo: match derived.bands[i].source {
                        RangeMask::Luminance { lo, .. } => lo,
                        RangeMask::Color { .. } => 0.0,
                    },
                    hi: match derived.bands[i].source {
                        RangeMask::Luminance { hi, .. } => hi,
                        RangeMask::Color { .. } => 1.0,
                    },
                    reason: "the shared estimator or do-no-harm gates refused the correction"
                        .to_string(),
                },
            ),
        }
    }
    report.correspondence = corr;
    if accepted.is_empty() {
        fit::append_finished_disclosure(report, &global_px, &tgt_px);
        return;
    }

    let initial_px = accepted.last().expect("accepted range exists").rendered.clone();
    let accepted_ranges = accepted
        .iter()
        .filter_map(|zone| zone.range)
        .collect::<Vec<_>>();
    let shares = accepted
        .iter()
        .map(|zone| {
            zone.source_weights.iter().sum::<f32>()
                / zone.source_weights.len().max(1) as f32
        })
        .collect::<Vec<_>>();
    let final_px = match enforce_range_boundary_gate(
        &s_img,
        report,
        &global_px,
        &accepted_ranges,
        &shares,
        first_range,
        initial_px,
    ) {
        BoundaryGateResult::Kept { pixels, .. } => pixels,
        BoundaryGateResult::Dropped => {
            fit::append_finished_disclosure(report, &global_px, &tgt_px);
            return;
        }
    };
    let final_frame_err = final_range_frame_err(&final_px, &tgt_px, &report.evidence);
    if final_frame_err > global_frame_err + RANGE_FRAME_REGRESSION_TOL {
        report.recipe.masks.truncate(first_range);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_FRAME_REFUSED,
                vec![
                    ("n", accepted.len().to_string()),
                    ("global", format!("{global_frame_err:.3}")),
                    ("after", format!("{final_frame_err:.3}")),
                    ("tol", format!("{RANGE_FRAME_REGRESSION_TOL:+.3}")),
                ],
            ),
        );
        report.err_after = global_frame_err;
        fit::append_finished_disclosure(report, &global_px, &tgt_px);
        return;
    }
    for zone in &mut accepted {
        let after = zone_moments(&final_px, &zone.source_weights);
        let target = zone_moments(&tgt_px, &zone.target_weights);
        zone.after = zone_err(&after, &target);
        push_zone_attached_note(report, zone);
    }
    frame_err = final_frame_err;
    report.err_after = frame_err;
    let worst = accepted.iter().map(|zone| zone.after).fold(0.0f32, f32::max);
    let range_conf = fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE);
    report.recipe.confidence = report.recipe.confidence.min(range_conf);
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::RANGE_CONFIDENCE,
            vec![
                ("n", accepted.len().to_string()),
                ("worst", format!("{worst:.3}")),
                ("frame", format!("{frame_err:.3}")),
            ],
        ),
    );
    fit::append_finished_disclosure(report, &final_px, &tgt_px);
}

/// Structural mode evidence is measured before the global solve. The source
/// segmentation weights are deliberately applied to both images; the target
/// segmentation remains exclusively the moment-matching population.
fn measure_zone_divergence(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &crate::recipe::EditRecipe,
    src_mask: &GrayImage,
) -> ZoneDivergences {
    let (sp, tp, w, h) = fit::divergence_raster(src, target, base);
    let sky_weights = mask_weights(src_mask, w, h);
    let land_weights: Vec<f32> = sky_weights.iter().map(|v| 1.0 - v).collect();
    let measured = |weights: &[f32]| ZoneDivergence {
        divergence: fit::structure_divergence(&sp, &tp, w, h, weights),
        share: weights.iter().sum::<f32>() / weights.len().max(1) as f32,
    };
    ZoneDivergences { sky: measured(&sky_weights), land: measured(&land_weights) }
}

/// Run the segmentation sidecar on both images. The source mask persists at
/// `mask_path` (the recipe references it); the target's inputs/mask are
/// temporary siblings, removed before returning. Any failure aborts the
/// whole zoned attempt — the caller degrades to the global fit.
fn segment_both(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
) -> Result<(GrayImage, GrayImage)> {
    #[cfg(test)]
    if let Some(masks) = SEGMENT_BOTH_OVERRIDE.with(|value| value.borrow_mut().take()) {
        return Ok(masks);
    }

    let sibling = |suffix: &str| -> std::path::PathBuf {
        let mut s = mask_path.path().as_os_str().to_owned();
        s.push(suffix);
        s.into()
    };
    let tmp_src = sibling(".src-in.png");
    let tmp_tgt = sibling(".tgt-in.png");
    let tmp_tgt_mask = sibling(".tgt-mask.png");
    // Segmentation reads scene SEMANTICS, not pixels: a ≤2048 input finds the
    // sky exactly as well as a 61 MP master while skipping a ~180 MB PNG
    // round-trip per side (the CLI fit hands full-res frames here). The
    // persisted mask raster is normalised-coordinate data — the engine
    // resamples it at whatever resolution the develop runs, and the GUI's own
    // reverse-fit already segments preview-res frames.
    let small_src;
    let src = if src.width().max(src.height()) > 2048 {
        small_src = src.thumbnail(2048, 2048);
        &small_src
    } else {
        src
    };
    let small_tgt;
    let target = if target.width().max(target.height()) > 2048 {
        small_tgt = target.thumbnail(2048, 2048);
        &small_tgt
    } else {
        target
    };
    let run = || -> Result<(GrayImage, GrayImage)> {
        src.to_rgb8().save(&tmp_src).context("write segmentation input (source)")?;
        target.to_rgb8().save(&tmp_tgt).context("write segmentation input (target)")?;
        segment_file(seg, &tmp_src, mask_path.path()).context("segment source sky")?;
        segment_file(seg, &tmp_tgt, &tmp_tgt_mask).context("segment target sky")?;
        let sm = crate::render::open_mask_bounded(mask_path.path())
            .context("read source sky mask")?
            .to_luma8();
        let tm = crate::render::open_mask_bounded(&tmp_tgt_mask)
            .context("read target sky mask")?
            .to_luma8();
        Ok((sm, tm))
    };
    let out = run();
    for p in [&tmp_src, &tmp_tgt, &tmp_tgt_mask] {
        std::fs::remove_file(p).ok();
    }
    if out.is_err() {
        // The source segmentation writes mask_path FIRST — a later failure
        // (target segmentation, mask decode) otherwise leaves a partial mask
        // file no zone will ever reference (the caller degrades to the global
        // fit). Removing it also releases a claimed unique raster name.
        mask_path.remove();
    }
    out
}

/// The post-segmentation half (separable so tests drive it with hand-built
/// masks, no python): correct the SKY zone, then the LAND zone — the same
/// raster reused with `inverted = true`, so one segmentation buys the whole
/// frame (the first real-pair render showed why land is not optional: the
/// distant haze-terrain outside the sky mask kept its global-fit blue and
/// clashed against the repainted gold sky as a hard halo). Each zone is
/// gated independently; the raster file is removed only when NO zone kept
/// it. Requires a VALID sky partition first: an empty/degenerate sky mask
/// makes "land" mean "everything", which would just be a weaker-gated
/// re-run of the global fit.
#[cfg(test)]
fn attach_zones(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    src_mask: &GrayImage,
    tgt_mask: &GrayImage,
    mask_path: &crate::store::OwnedRaster,
) {
    let divergence = measure_zone_divergence(
        src,
        target,
        &crate::recipe::EditRecipe::default(),
        src_mask,
    );
    attach_zones_with_divergence(
        src,
        target,
        report,
        src_mask,
        tgt_mask,
        mask_path,
        divergence,
    );
}

#[allow(clippy::too_many_arguments)]
fn attach_zones_with_divergence(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    src_mask: &GrayImage,
    tgt_mask: &GrayImage,
    mask_path: &crate::store::OwnedRaster,
    divergence: ZoneDivergences,
) {
    let s_img = src.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let t_img = target.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let tgt_px = fit::pixels_of(&t_img);
    let (aw, ah) = {
        let c = render::develop_preview(&s_img, &report.recipe);
        (c.width(), c.height())
    };
    let sw = mask_weights(src_mask, aw, ah);
    let tw = mask_weights(tgt_mask, t_img.width(), t_img.height());
    // Partition validity — judged on the raw mask shares (Σw/n), before any
    // zone-specific gating.
    let share = |w: &[f32]| w.iter().sum::<f32>() / w.len().max(1) as f32;
    let (s_share, t_share) = (share(&sw), share(&tw));
    if !(MIN_ZONE_SHARE..=1.0 - MIN_ZONE_SHARE).contains(&s_share)
        || !(MIN_ZONE_SHARE..=1.0 - MIN_ZONE_SHARE).contains(&t_share)
    {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONED_NO_PARTITION,
                vec![
                    ("s", format!("{:.0}", s_share * 100.0)),
                    ("t", format!("{:.0}", t_share * 100.0)),
                ],
            ),
        );
        mask_path.remove();
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(
            &mut *report,
            &finished,
            &tgt_px,
        );
        return;
    }
    let (lo, hi) = (s_share.min(t_share), s_share.max(t_share));
    if hi > 2.0 * lo {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_SHARE_MISMATCH,
                vec![
                    ("label", "frame".to_string()),
                    ("s", format!("{:.0}", s_share * 100.0)),
                    ("t", format!("{:.0}", t_share * 100.0)),
                ],
            ),
        );
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::plain(crate::rationale::keys::ZONE_SHARE_NO_CORRECTION),
        );
        mask_path.remove();
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(report, &finished, &tgt_px);
        return;
    }
    let swl: Vec<f32> = sw.iter().map(|w| 1.0 - w).collect();
    let twl: Vec<f32> = tw.iter().map(|w| 1.0 - w).collect();
    // The running FRAME-GLOBAL error, threaded through the zone passes for
    // the bounded-drift insurance ONLY (R23-6 honesty fix). It used to be
    // `report.err_after` itself, read and overwritten in place — which made
    // the number handed to the user, and the confidence derived from it, the
    // very frame-global metric this module's own [`ZONE_ACCEPT_RATIO`] doc
    // proves cannot judge a zone (0.507 → 0.015 zone-local while the frame
    // moved 0.1768 → 0.1792). Drift still has to be measured incrementally,
    // so the value still has to be carried; it is simply no longer the same
    // variable as the report's verdict.
    let mut frame_err = report.err_after;
    // Taken, not borrowed: each zone needs the field WHILE holding the
    // report mutably; restored below so the report keeps carrying it.
    let corr = report.correspondence.take();
    let sky_attachment = ZoneAttachment {
        source_weights: sw.clone(),
        target_weights: tw.clone(),
        mask: MaskGeometry::Bitmap { path: mask_path.path().to_string_lossy().into_owned() },
        range: None,
        name: String::new(),
        role: MaskRole::ZoneSky,
        inverted: false,
        label: MaskRole::ZoneSky.tag().to_string(),
        frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
    };
    let land_attachment = ZoneAttachment {
        source_weights: swl,
        target_weights: twl,
        mask: MaskGeometry::Bitmap { path: mask_path.path().to_string_lossy().into_owned() },
        range: None,
        name: String::new(),
        role: MaskRole::ZoneLand,
        inverted: true,
        label: MaskRole::ZoneLand.tag().to_string(),
        frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
    };
    let sky = attach_one_zone(
        &s_img,
        &tgt_px,
        report,
        &mut frame_err,
        &sky_attachment,
        divergence.sky.divergence,
        corr.as_ref(),
    );
    let land = attach_one_zone(
        &s_img,
        &tgt_px,
        report,
        &mut frame_err,
        &land_attachment,
        divergence.land.divergence,
        corr.as_ref(),
    );
    report.correspondence = corr;
    let mut accepted: Vec<AcceptedZone> = [sky, land].into_iter().flatten().collect();
    if accepted.is_empty() {
        mask_path.remove();
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(
            report,
            &finished,
            &tgt_px,
        );
        return;
    }
    let first_zone = report.recipe.masks.len() - accepted.len();
    let initial_px = accepted.last().expect("at least one accepted zone").rendered.clone();
    let correction_shares = accepted
        .iter()
        .map(|zone| {
            zone.source_weights.iter().sum::<f32>()
                / zone.source_weights.len().max(1) as f32
        })
        .collect::<Vec<_>>();
    let final_px = match enforce_boundary_gate(
        &s_img,
        report,
        &sw,
        &correction_shares,
        first_zone,
        initial_px,
    ) {
        BoundaryGateResult::Kept { k, before, after, pixels } => {
            debug_assert!((0.0..=1.0).contains(&k));
            debug_assert!(before.rim.is_finite() && after.rim.is_finite());
            pixels
        }
        BoundaryGateResult::Dropped => {
            mask_path.remove();
            let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
            fit::append_finished_disclosure(
                report,
                &finished,
                &tgt_px,
            );
            return;
        }
    };
    // The boundary shrink changes the actual zone landings, so the attached
    // notes and confidence are measured again from the kept render rather
    // than repeating the pre-gate candidate's dials/residuals.
    for zone in &mut accepted {
        let after = zone_moments(&final_px, &zone.source_weights);
        let target = zone_moments(&tgt_px, &zone.target_weights);
        zone.after = zone_err(&after, &target);
        push_zone_attached_note(report, zone);
    }
    frame_err = fit::look_err_with_evidence(&final_px, &tgt_px, &report.evidence);
    // `err_after` keeps its CONTRACT — the frame-global look distance of the
    // recipe that actually ships, in the same unit as `err_before`, which is
    // what every printout pairs it with. What changes is that it no longer
    // doubles as the zone stage's verdict.
    report.err_after = frame_err;
    // CONFIDENCE, on the other hand, is a verdict, and it now comes from what
    // was actually judged: the WORST accepted zone's own residual, on the
    // zone scale, floored against the global stage's own claim so neither
    // stage can promise what the other did not deliver. Worst, not
    // area-weighted: a perfectly matched sky over a wrecked foreground is not
    // a 70%-confident fit, and the zones are few and large enough that the
    // worst one is never a sliver.
    let worst = accepted.iter().map(|z| z.after).fold(0.0f32, f32::max);
    let zone_conf = fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE);
    report.recipe.confidence = report.recipe.confidence.min(zone_conf);
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::ZONE_CONFIDENCE,
            vec![
                ("n", accepted.len().to_string()),
                ("worst", format!("{worst:.3}")),
                ("frame", format!("{frame_err:.3}")),
            ],
        ),
    );
    fit::append_finished_disclosure(
        report,
        &final_px,
        &tgt_px,
    );
}

/// A zone whose correction was kept — what [`attach_zones`] needs to report
/// on the stage as a whole.
struct AcceptedZone {
    /// Display/rationale identity carried by the attachment itself.
    label: String,
    /// Optional native range refinement; `None` is a semantic bitmap zone.
    range: Option<RangeMask>,
    /// Exact correction index, independent of role or free-text identity.
    mask_index: usize,
    /// Estimator populations retained for the final-stack remeasurement.
    source_weights: Vec<f32>,
    target_weights: Vec<f32>,
    /// The zone-local residual before this correction.
    before: f32,
    /// The zone-local residual it landed at ([`zone_err`]).
    after: f32,
    /// The candidate render, retained so the boundary gate reuses the final
    /// analysis render instead of buying another full candidate render.
    rendered: Vec<[f32; 3]>,
}

enum BoundaryGateResult {
    Kept {
        k: f32,
        before: BoundaryReading,
        after: BoundaryReading,
        pixels: Vec<[f32; 3]>,
    },
    Dropped,
}

fn boundary_note_args(
    n: usize,
    k: f32,
    before: BoundaryReading,
    after: BoundaryReading,
) -> Vec<(&'static str, String)> {
    vec![
        ("n", n.to_string()),
        ("k", format!("{k:.3}")),
        ("before", format!("{:.3}", before.rim)),
        ("after", format!("{:.3}", after.rim)),
        ("max", format!("{ZONE_BOUNDARY_RIM_MAX:.3}")),
        ("transitions", after.transitions.to_string()),
    ]
}

/// Enforce the pair-level boundary budget after the independent zone-local
/// gates. `initial_px` is the analysis render the last accepted zone already
/// made. Only re-measurements during an actual shrink render again, always at
/// analysis size; no full-resolution render is introduced.
fn enforce_boundary_gate(
    s_img: &DynamicImage,
    report: &mut FitReport,
    sky_weights: &[f32],
    correction_shares: &[f32],
    first_zone: usize,
    initial_px: Vec<[f32; 3]>,
) -> BoundaryGateResult {
    let initial = boundary_rim(&initial_px, sky_weights, s_img.width(), s_img.height());
    let zone_count = report.recipe.masks.len().saturating_sub(first_zone);
    if initial.rim <= ZONE_BOUNDARY_RIM_MAX {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_BOUNDARY_PASSED,
                boundary_note_args(zone_count, 1.0, initial, initial),
            ),
        );
        return BoundaryGateResult::Kept {
            k: 1.0,
            before: initial,
            after: initial,
            pixels: initial_px,
        };
    }

    let originals = report.recipe.masks[first_zone..].to_vec();
    let shares = correction_shares.to_vec();
    debug_assert_eq!(shares.len(), originals.len());
    let render_at = |report: &mut FitReport, k: f32| -> (BoundaryReading, Vec<[f32; 3]>) {
        shrink_zone_corrections(
            &mut report.recipe.masks[first_zone..],
            &originals,
            &shares,
            k,
        );
        let pixels = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let reading = boundary_rim(&pixels, sky_weights, s_img.width(), s_img.height());
        (reading, pixels)
    };

    let (zero, zero_px) = render_at(report, 0.0);
    if zero.rim > ZONE_BOUNDARY_RIM_MAX {
        report.recipe.masks.truncate(first_zone);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_BOUNDARY_DROPPED,
                boundary_note_args(zone_count, 0.0, initial, zero),
            ),
        );
        return BoundaryGateResult::Dropped;
    }

    // Monotone in the differential for the bounded pointwise zone dials.
    // Twelve bisections resolve k to <0.00025, much finer than the displayed
    // three decimals or the 8-bit analysis render can distinguish.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut best = (zero, zero_px);
    for _ in 0..12 {
        let mid = (lo + hi) * 0.5;
        let measured = render_at(report, mid);
        if measured.0.rim <= ZONE_BOUNDARY_RIM_MAX {
            lo = mid;
            best = measured;
        } else {
            hi = mid;
        }
    }
    shrink_zone_corrections(
        &mut report.recipe.masks[first_zone..],
        &originals,
        &shares,
        lo,
    );
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::ZONE_BOUNDARY_PASSED,
            boundary_note_args(zone_count, lo, initial, best.0),
        ),
    );
    BoundaryGateResult::Kept { k: lo, before: initial, after: best.0, pixels: best.1 }
}

fn push_zone_attached_note(report: &mut FitReport, zone: &AcceptedZone) {
    let label = zone.label.as_str();
    let (ev, gains, saturation) = {
        let mask = report
            .recipe
            .masks
            .get(zone.mask_index)
            .expect("accepted zone mask remains attached");
        (mask.exposure_ev, mask.color_gains.unwrap_or([1.0; 3]), mask.saturation)
    };
    if let Some(RangeMask::Luminance { lo, hi, .. }) = zone.range {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_ATTACHED,
                vec![
                    ("label", label.to_string()),
                    ("lo", format!("{lo:.3}")),
                    ("hi", format!("{hi:.3}")),
                    ("ev", format!("{ev:+.2}")),
                    ("g0", format!("{:.2}", gains[0])),
                    ("g1", format!("{:.2}", gains[1])),
                    ("g2", format!("{:.2}", gains[2])),
                    ("sat", format!("{saturation:+.0}")),
                    ("before", format!("{:.3}", zone.before)),
                    ("after", format!("{:.3}", zone.after)),
                ],
            ),
        );
        return;
    }
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::ZONE_ATTACHED,
            vec![
                ("label", label.to_string()),
                ("ev", format!("{ev:+.2}")),
                ("g0", format!("{:.2}", gains[0])),
                ("g1", format!("{:.2}", gains[1])),
                ("g2", format!("{:.2}", gains[2])),
                ("sat", format!("{saturation:+.0}")),
                ("before", format!("{:.3}", zone.before)),
                ("after", format!("{:.3}", zone.after)),
            ],
        ),
    );
}

/// Confidence slope on the ZONE scale — the joint family's and the global
/// fit's slopes each belong to their own metric, and this is the third.
/// Anchored on this module's own measured landings: corrections that work
/// land at 0.007-0.015 and [`ZONE_MATCHED_ERR`] (0.02) is the ceiling of
/// "matched", so 0.02 must still read as high confidence (0.90 here) while
/// the floor is reached at a zone residual of 0.15 — ten times the observed
/// good landing, i.e. a zone that was not corrected at all.
const ZONE_CONFIDENCE_SLOPE: f32 = 5.0;

/// Fit + gate ONE zone; returns its verdict when the correction was kept.
/// The zone is measured on a fresh render of the CURRENT recipe (including
/// any zone already attached), so corrections stack the way the engine
/// renders them. Judged by the ZONE-LOCAL error (see [`ZONE_ACCEPT_RATIO`]
/// for the measured reason the frame-global metric cannot be the judge);
/// `frame_err` carries the running frame-global look distance IN and OUT.
/// Semantic zones use it for bounded-drift insurance; range bands require a
/// neutral-or-better composed frame. It is deliberately not the report's own
/// field any more (R23-6).
fn attach_one_zone(
    s_img: &DynamicImage,
    tgt_px: &[[f32; 3]],
    report: &mut FitReport,
    frame_err: &mut f32,
    attachment: &ZoneAttachment,
    divergence: fit::Divergence,
    corr: Option<&fit::PairCorrespondence>,
) -> Option<AcceptedZone> {
    // `label` drives the rationale prose; it's the zone's stable ASCII tag, so
    // the text stays English/identical regardless of the GUI's display language.
    let label = attachment.label.as_str();
    let (sw, tw) = (
        attachment.source_weights.as_slice(),
        attachment.target_weights.as_slice(),
    );
    let cur_px = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    // Mode is derived FIRST since step 7b: the correspondence composes only
    // into a FULL zone's estimators. An Atmosphere zone is fitted as a
    // bounded distribution precisely BECAUSE its content was replaced, and
    // weighting its statistics by "does this pixel correspond" would starve
    // the very zone the divergent-zones-are-never-dropped ruling protects.
    let mode = if divergence.d >= fit::DIVERGENCE_ZONE {
        ZoneMode::Atmosphere
    } else {
        ZoneMode::Full
    };
    let compose = |base: &[f32], robust: &Option<fit::PairedRobustTone>| -> Vec<f32> {
        let mut out = base.to_vec();
        if let Some(r) = robust {
            for (o, w) in out.iter_mut().zip(&r.weights) {
                *o *= w;
            }
        }
        out
    };
    let min_wt =
        |i: usize| sw.get(i).copied().unwrap_or(0.0).min(tw.get(i).copied().unwrap_or(0.0));
    // The SAME robust paired estimator the global tone stage runs (one
    // mechanism, two call sites): pixels the two zones do not share — the
    // divergent cloud deck, a moved subject — lose weight by the influence
    // function, so the moments, the dials, the tone solve and the saturation
    // chase below all measure the population the zones have in common. No
    // neutral gate here: a zone is already one coherent population and its
    // chromatic body (a blue sky) is exactly the evidence.
    //
    // With a correspondence field (step 7b), a FULL zone pairs against the
    // CORRESPONDED target and weights each pair by the field's confidence —
    // shifted content becomes evidence again instead of an outlier. Two
    // invariants hold by construction: the share GATE below never reads the
    // confidence (zone size is a zone question — composing it would let a
    // heavily-shifted zone silently vanish), and a field whose composed
    // population collapses is dropped WHOLESALE, falling back to the plain
    // pairing — the field may refuse to help, never starve a zone.
    let field = match (mode, corr) {
        (ZoneMode::Full, Some(c)) => {
            let zr = fit::paired_robust_tone(
                &cur_px,
                &c.tp,
                &|i: usize| min_wt(i) * c.conf.get(i).copied().unwrap_or(0.0),
                false,
            );
            let with_conf = |base: &[f32]| -> Vec<f32> {
                compose(base, &zr)
                    .iter()
                    .enumerate()
                    .map(|(i, w)| w * c.conf.get(i).copied().unwrap_or(0.0))
                    .collect()
            };
            let zws = with_conf(sw);
            let zwt = with_conf(tw);
            let ms = zone_moments(&cur_px, &zws);
            let mt = zone_moments(&c.tp, &zwt);
            (ms.share >= MIN_ZONE_SHARE && mt.share >= MIN_ZONE_SHARE)
                .then_some((c, zr, zws, zwt, ms, mt))
        }
        _ => None,
    };
    let (tgt_eff, zone_robust, zw_source, zw_target, ms, mt, gate_s_share, gate_t_share) =
        match field {
            Some((c, zr, zws, zwt, ms, mt)) => {
                let gs = zone_moments(&cur_px, &compose(sw, &zr)).share;
                let gt = zone_moments(tgt_px, &compose(tw, &zr)).share;
                (c.tp.as_slice(), zr, zws, zwt, ms, mt, gs, gt)
            }
            None => {
                let zr = fit::paired_robust_tone(&cur_px, tgt_px, &|i: usize| min_wt(i), false);
                let zws = compose(sw, &zr);
                let zwt = compose(tw, &zr);
                let ms = zone_moments(&cur_px, &zws);
                let mt = zone_moments(tgt_px, &zwt);
                let (gs, gt) = (ms.share, mt.share);
                (tgt_px, zr, zws, zwt, ms, mt, gs, gt)
            }
        };
    if gate_s_share < MIN_ZONE_SHARE || gate_t_share < MIN_ZONE_SHARE {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_TOO_SMALL,
                vec![
                    ("label", label.to_string()),
                    ("s", format!("{:.0}", gate_s_share * 100.0)),
                    ("t", format!("{:.0}", gate_t_share * 100.0)),
                ],
            ),
        );
        return None;
    }
    let mode_key = match mode {
        ZoneMode::Full => crate::rationale::keys::ZONE_MODE_FULL,
        ZoneMode::Atmosphere => crate::rationale::keys::ZONE_MODE_ATMOSPHERE,
    };
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            mode_key,
            vec![
                ("label", label.to_string()),
                ("d", format!("{:.3}", divergence.d)),
            ],
        ),
    );
    let zone_before = zone_err(&ms, &mt);
    // Composition is an input fact, not an acceptance verdict. Disclose it
    // before any early evidence/quality return so a withheld correction still
    // explains why the two zone populations are not comparable.
    let (lo, hi) = (gate_s_share.min(gate_t_share), gate_s_share.max(gate_t_share));
    if hi > 2.0 * lo {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_SHARE_MISMATCH,
                vec![
                    ("label", label.to_string()),
                    ("s", format!("{:.0}", gate_s_share * 100.0)),
                    ("t", format!("{:.0}", gate_t_share * 100.0)),
                ],
            ),
        );
        return None;
    }
    // Already-matched zone: attempting a fit would be dialling noise — the
    // observed attempts regress (land 0.009 → 0.029 on the live pair), and
    // the old outcome message ("dropped: needs ≤ 50%") read as a discarded
    // improvement. Say what is true instead: there is nothing to correct.
    // The EV companion keeps the linear-light skip line honest in dark
    // zones (see [`ZONE_MATCHED_EV`]).
    let ev_gap = (mt.luma_lin.max(1e-6) / ms.luma_lin.max(1e-6)).log2().abs();
    if zone_skips(zone_before, ev_gap) {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_ALREADY_MATCHED,
                vec![("label", label.to_string()), ("before", format!("{zone_before:.3}"))],
            ),
        );
        return None;
    }
    if let Some(r) = zone_robust.as_ref()
        && r.rejected_share >= fit::ROBUST_REJECT_DISCLOSE_MIN
    {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_ROBUST_REJECTED,
                vec![
                    ("label", label.to_string()),
                    ("pct", format!("{:.0}", r.rejected_share * 100.0)),
                    (
                        "ranges",
                        if r.rejected_ranges.is_empty() {
                            "scattered".to_string()
                        } else {
                            r.rejected_ranges.clone()
                        },
                    ),
                ],
            ),
        );
    }
    let d = fit_zone_dials(&ms, &mt);
    let round1 = |v: f32| (v * 10.0).round() / 10.0;
    let round2 = |v: f32| (v * 100.0).round() / 100.0;
    report.recipe.masks.push(LocalAdjustment {
        mask: attachment.mask.clone(),
        range: attachment.range,
        name: attachment.name.clone(),
        role: attachment.role,
        amount: 1.0,
        inverted: attachment.inverted,
        color_gains: Some(
            match mode {
                ZoneMode::Full => d.color_gains,
                ZoneMode::Atmosphere => shrink_atmosphere_gains(d.color_gains),
            }
            .map(round2),
        ),
        ..Default::default()
    });
    // Within-zone tone: the zone's brightness/contrast is a DISTRIBUTION,
    // not a mean (see [`zone_luma_cdf`] — the real pair's land matched the
    // linear mean and still rendered far darker than the target). Map the
    // zone's weighted luma CDF onto the target zone's and solve the engine's
    // own local tone sliders from it — the same basis + magnitude prior as
    // the global stage 1. This SUPERSEDES the moment-EV from fit_zone_dials
    // (which now only normalises the gains): brightness lives here, on the
    // render of the gains-only mask so the solve sees the recoloured zone.
    //
    // IDENTIFIABILITY GUARD: a quantile map out of a near-uniform source
    // zone is degenerate — a monotone map cannot spread a luma spike into
    // the target's wide distribution, and the slider solve goes wild on the
    // violent pseudo-map instead (measured, real pair: the flat hazy sky
    // drew exposure −0.70 and its zone residual went 0.016 → 0.108). Below
    // an IQR floor, fall back to the moment-EV and leave the tone flat.
    if mode == ZoneMode::Full {
        let rp = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let s_cdf = zone_luma_cdf(&rp, &zw_source);
        let src_iqr = fit::quantile(&s_cdf, 0.75) - fit::quantile(&s_cdf, 0.25);
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        if src_iqr >= 0.05 {
            // Paired robust regression on the recoloured render (the gains
            // just changed every zone luma, so the map must be re-estimated),
            // falling back to the weighted quantile transport only when the
            // paired estimate is too thin — and either way solving only the
            // knots this zone's own population supports.
            let refit = fit::paired_robust_tone(
                &rp,
                tgt_eff,
                &|i: usize| {
                    zw_source
                        .get(i)
                        .copied()
                        .unwrap_or(0.0)
                        .min(zw_target.get(i).copied().unwrap_or(0.0))
                },
                false,
            );
            let points = refit.as_ref().map(|r| r.points.clone()).unwrap_or_default();
            let support = fit::knot_support_for(&rp, &zw_source, &points);
            let score_set: Vec<(f32, f32, f32)> = match refit.as_ref() {
                Some(r) if r.points.len() >= 6 => r
                    .points
                    .iter()
                    .zip(&r.masses)
                    .map(|(&(x, y), &mass)| (x, y, mass))
                    .collect(),
                _ => Vec::new(),
            };
            let t_cdf = zone_luma_cdf(tgt_eff, &zw_target);
            let tone_map = |x: f32| {
                if points.len() >= 6 {
                    fit::sample_tone_points(&points, x)
                } else {
                    fit::quantile(
                        &t_cdf,
                        fit::cdf_at(&s_cdf, x).clamp(fit::P_CLIP, 1.0 - fit::P_CLIP),
                    )
                }
            };
            let (ev, sliders) =
                fit::fit_tone_sliders_supported(&tone_map, &support, &score_set);
            m.exposure_ev = round2(ev.clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT));
            m.contrast = round1(sliders[0] * 100.0);
            m.highlights = round1(sliders[1] * 100.0);
            m.shadows = round1(sliders[2] * 100.0);
            m.whites = round1(sliders[3] * 100.0);
            m.blacks = round1(sliders[4] * 100.0);
        } else {
            m.exposure_ev = round2(d.exposure_ev.clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT));
        }
    } else {
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        m.exposure_ev = round2(d.exposure_ev.clamp(-ZONE_ATMOS_EV_LIMIT, ZONE_ATMOS_EV_LIMIT));
    }
    // Closed-loop zone saturation on real renders (the gains change chroma
    // by themselves — only a render knows where the zone landed).
    for _ in 0..2 {
        let rp = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let zone_chroma = zone_moments(&rp, &zw_source).chroma;
        let Some(step) = zone_sat_step(zone_chroma, mt.chroma) else { break };
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        let next = clamp_zone_sat_for_mode((m.saturation + step).round(), mode);
        if next == m.saturation {
            break;
        }
        m.saturation = next;
    }
    // Probe the two control classes independently. A one-sided hue band must
    // withhold only chroma movement; supported luminance evidence still earns
    // the zone correction.
    let mut luma_probe = report.recipe.clone();
    {
        let m = luma_probe.masks.last_mut().expect("zone mask just pushed");
        m.color_gains = Some([1.0; 3]);
        m.saturation = 0.0;
    }
    let luma_ranges = fit::moved_unsupported_luma_range_names(
        &cur_px,
        &fit::pixels_of(&render::develop_preview(s_img, &luma_probe)),
        &report.evidence,
    );
    let mut chroma_probe = report.recipe.clone();
    {
        let m = chroma_probe.masks.last_mut().expect("zone mask just pushed");
        m.exposure_ev = 0.0;
        m.contrast = 0.0;
        m.highlights = 0.0;
        m.shadows = 0.0;
        m.whites = 0.0;
        m.blacks = 0.0;
    }
    let hue_bands = fit::moved_unsupported_hue_range_names(
        &cur_px,
        &fit::pixels_of(&render::develop_preview(s_img, &chroma_probe)),
        &report.evidence,
    );
    if luma_ranges.is_some() {
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        m.exposure_ev = 0.0;
        m.contrast = 0.0;
        m.highlights = 0.0;
        m.shadows = 0.0;
        m.whites = 0.0;
        m.blacks = 0.0;
    }
    if hue_bands.is_some() {
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        m.color_gains = Some([1.0; 3]);
        m.saturation = 0.0;
    }
    // ONE NOTE PER CONTROL CLASS ACTUALLY WITHHELD. The two probes above
    // withhold independently, so a single "correction withheld" sentence
    // described none of the three outcomes correctly. Each note states only
    // its own class; when both fire, their conjunction is the whole-zone
    // refusal, and what SURVIVED is carried positively by the attach note.
    if let Some(hue_bands) = hue_bands.as_ref() {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR,
                vec![
                    ("label", label.to_string()),
                    ("hue_bands", hue_bands.clone()),
                ],
            ),
        );
    }
    if let Some(luma_ranges) = luma_ranges.as_ref() {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE,
                vec![
                    ("label", label.to_string()),
                    ("luma_ranges", luma_ranges.clone()),
                ],
            ),
        );
    }
    if hue_bands.is_some() && luma_ranges.is_none() {
        let original = {
            let m = report.recipe.masks.last().expect("zone mask just pushed");
            [m.exposure_ev, m.contrast, m.highlights, m.shadows, m.whites, m.blacks]
        };
        for factor in [0.75f32, 0.5, 0.25, 0.0] {
            {
                let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
                m.exposure_ev = original[0] * factor;
                m.contrast = original[1] * factor;
                m.highlights = original[2] * factor;
                m.shadows = original[3] * factor;
                m.whites = original[4] * factor;
                m.blacks = original[5] * factor;
            }
            let probe = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
            if local_quality(&cur_px, &probe, sw, s_img.width(), s_img.height()).passes() {
                break;
            }
        }
    }
    let neutral_zone = {
        let m = report.recipe.masks.last().expect("zone mask just pushed");
        m.exposure_ev.abs() <= 1e-4
            && m.contrast.abs() <= 1e-4
            && m.highlights.abs() <= 1e-4
            && m.shadows.abs() <= 1e-4
            && m.whites.abs() <= 1e-4
            && m.blacks.abs() <= 1e-4
            && m.saturation.abs() <= 1e-4
            && m.color_gains.map(|g| g.iter().all(|v| (*v - 1.0).abs() <= 1e-4)).unwrap_or(true)
    };
    if neutral_zone {
        report.recipe.masks.pop();
        return None;
    }
    let zoned_px = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    let zoned_err = fit::look_err_with_evidence(&zoned_px, tgt_px, &report.evidence);
    let m_after = zone_moments(&zoned_px, sw);
    let zone_after = zone_err(&m_after, &mt);
    let ev_after = (mt.luma_lin.max(1e-6) / m_after.luma_lin.max(1e-6)).log2().abs();
    let quality = local_quality(&cur_px, &zoned_px, sw, s_img.width(), s_img.height());
    if quality.passes() {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_QUALITY_PASSED,
                vec![
                    ("label", label.to_string()),
                    ("texture", format!("{:.3}", quality.texture_ratio)),
                    ("clip_before", format!("{:.2}", quality.clipped_before * 100.0)),
                    ("clip_after", format!("{:.2}", quality.clipped_after * 100.0)),
                ],
            ),
        );
    } else {
        report.recipe.masks.pop();
        if !quality.texture_passes() {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED,
                    vec![
                        ("label", label.to_string()),
                        ("ratio", format!("{:.3}", quality.texture_ratio)),
                        ("min", format!("{ZONE_TEXTURE_MIN:.2}")),
                        ("max", format!("{ZONE_TEXTURE_MAX:.2}")),
                    ],
                ),
            );
        }
        if !quality.clipping_passes() {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_QUALITY_CLIPPING_FAILED,
                    vec![
                        ("label", label.to_string()),
                        ("before", format!("{:.2}", quality.clipped_before * 100.0)),
                        ("after", format!("{:.2}", quality.clipped_after * 100.0)),
                        ("growth", format!("{:.2}", ZONE_CLIP_GROWTH * 100.0)),
                    ],
                ),
            );
        }
        return None;
    }
    let accepted_before = if hue_bands.is_some() && luma_ranges.is_none() {
        zone_luma_err(&ms, &mt)
    } else {
        zone_before
    };
    let accepted_after = if hue_bands.is_some() && luma_ranges.is_none() {
        zone_luma_err(&m_after, &mt)
    } else {
        zone_after
    };
    let zone_accepted = match mode {
        ZoneMode::Full => zone_accepts(accepted_before, accepted_after, ev_after),
        ZoneMode::Atmosphere => accepted_after <= accepted_before,
    };
    if zone_accepted && zoned_err <= *frame_err + attachment.frame_regression_tol {
        // The running frame-global value advances so the NEXT zone's drift
        // budget is measured from here — but neither `err_after` nor
        // `confidence` is written from it any more (R23-6): see the comment
        // in [`attach_zones`] and this module's own [`ZONE_ACCEPT_RATIO`]
        // proof that this number cannot judge a zone.
        *frame_err = zoned_err;
        Some(AcceptedZone {
            label: attachment.label.clone(),
            range: attachment.range,
            mask_index: report.recipe.masks.len() - 1,
            source_weights: attachment.source_weights.clone(),
            target_weights: attachment.target_weights.clone(),
            before: zone_before,
            after: zone_after,
            rendered: zoned_px,
        })
    } else {
        report.recipe.masks.pop();
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                match mode {
                    ZoneMode::Full => crate::rationale::keys::ZONE_DROPPED,
                    ZoneMode::Atmosphere => crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED,
                },
                vec![
                    ("label", label.to_string()),
                    ("before", format!("{zone_before:.3}")),
                    ("after", format!("{zone_after:.3}")),
                    ("ratio", format!("{:.0}", ZONE_ACCEPT_RATIO * 100.0)),
                    ("floor", format!("{ZONE_MATCHED_ERR:.3}")),
                    ("gain", format!("{:.0}", (1.0 - ZONE_FLOOR_MIN_GAIN) * 100.0)),
                    ("drift", format!("{:+.3}", zoned_err - *frame_err)),
                    ("tol", format!("{:+.3}", attachment.frame_regression_tol)),
                ],
            ),
        );
        None
    }
}

/// Per-pixel mask weights for an analysis frame of `w`×`h` — the SAME
/// normalisation and bilinear sampling the engine's mask stage uses
/// (`render::sample_gray_norm` at PIXEL CENTRES, through the shared
/// [`render::MASK_SAMPLE_CENTRE`]), so the moments are measured exactly where
/// the render will apply them. R29 C2 moved this and `apply_masks`' own
/// `weight_at` together; a zone measured on one grid and rendered on another
/// would put the gains half a pixel off the population they were solved from.
fn mask_weights(mask: &GrayImage, w: u32, h: u32) -> Vec<f32> {
    // usize-widen BEFORE multiplying: `w * h` is a u32 product and a frame
    // over u32::MAX pixels would overflow the reservation (panic in debug,
    // pathological reallocation in release) while the loops still push w×h.
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            out.push(render::sample_gray_norm(
                mask,
                (x as f32 + render::MASK_SAMPLE_CENTRE) / w as f32,
                (y as f32 + render::MASK_SAMPLE_CENTRE) / h as f32,
            ));
        }
    }
    out
}

// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with mass in every luminance band and both chroma classes —
    /// the joint family's own fixture, so its unit tests do not depend on
    /// `fit`'s.
    fn joint_fixture(warm_shift: f32) -> Vec<[f32; 3]> {
        let mut px = Vec::with_capacity(4096);
        for i in 0..4096 {
            let l = 0.05 + 0.9 * (i % 64) as f32 / 63.0;
            if (i / 64) % 2 == 0 {
                px.push([l, l, l]); // neutral
            } else {
                px.push([(l + warm_shift).clamp(0.0, 1.0), l * 0.7, l * 0.35]); // chromatic
            }
        }
        px
    }

    /// The eight bucket weights must partition unity — every pixel counted
    /// exactly once across the family, so the shares read as frame fractions
    /// and the area weighting means what it says.
    #[test]
    fn the_joint_buckets_partition_unity() {
        let px = joint_fixture(0.0);
        let mut total = vec![0.0f32; px.len()];
        let mut w = Vec::new();
        for b in 0..JOINT_BUCKETS {
            joint_weights(&px, b, &mut w);
            assert_eq!(w.len(), px.len());
            for (t, x) in total.iter_mut().zip(&w) {
                assert!((0.0..=1.0).contains(x), "weight out of range: {x}");
                *t += x;
            }
        }
        for (i, t) in total.iter().enumerate() {
            assert!((t - 1.0).abs() < 1e-5, "pixel {i} got total weight {t}");
        }
        // …and the shares of the qualifying buckets cannot exceed the frame.
        let sum: f32 = joint_buckets(&px, &px).iter().map(|b| b.share).sum();
        assert!(sum <= 1.0 + 1e-5, "shares sum to {sum}");
    }

    /// Buckets correspond by VALUE, not by position: two frames of totally
    /// different SIZE and layout that hold the same value populations must
    /// read as matched. This is the property that makes the reading immune
    /// to the composition differences frame-global matching dies on.
    #[test]
    fn the_joint_family_matches_by_value_not_by_position() {
        let a = joint_fixture(0.0);
        // Same populations, HALF the pixels, reversed order.
        let mut b: Vec<[f32; 3]> = a.iter().step_by(2).copied().collect();
        b.reverse();
        assert_ne!(a.len(), b.len());
        let r = joint_reading(&a, &b).expect("both sides carry the same values");
        assert!(r.weighted < 0.01, "same values must read matched: {r:?}");
        assert!(r.buckets >= 4, "the fixture must exercise most of the family: {r:?}");
        // A real difference must show up, and in a CHROMATIC bucket — the
        // shift only touches coloured pixels.
        let warm = joint_fixture(0.25);
        let r2 = joint_reading(&a, &warm).expect("reading");
        assert!(r2.weighted > 10.0 * r.weighted, "the warm shift must register: {r2:?}");
        assert!(
            r2.worst_label.ends_with("colour"),
            "a chroma-only difference must land in a colour bucket: {r2:?}"
        );
    }

    /// FAIL-OPEN: no evidence ⇒ no opinion, never "no problem".
    #[test]
    fn the_joint_family_abstains_without_evidence() {
        // A frame with two pixels cannot clear the share floor on 8 buckets.
        let tiny = vec![[0.5f32, 0.5, 0.5]; 2];
        let other = vec![[0.9f32, 0.1, 0.1]; 2];
        // Every bucket but one is empty on at least one side.
        let r = joint_reading(&tiny, &other);
        assert!(r.is_none() || r.unwrap().buckets < JOINT_BUCKETS);
        // An EMPTY side has no opinion at all.
        assert_eq!(joint_reading(&[], &[]), None);
        assert_eq!(joint_buckets(&[], &tiny).len(), 0);
    }

    #[test]
    fn joint_far_classification_keeps_refusals_out_of_the_miss_note() {
        assert_eq!(
            classify_joint_far(JOINT_FAR_ERR + 0.01, true),
            Some(JointFarCause::Refused)
        );
        assert_eq!(
            JointFarCause::Refused.note_key(),
            crate::rationale::keys::FIT_NOTE_JOINT_REFUSED
        );
        assert_eq!(
            classify_joint_far(JOINT_FAR_ERR + 0.01, false),
            Some(JointFarCause::Miss)
        );
        assert_eq!(
            JointFarCause::Miss.note_key(),
            crate::rationale::keys::FIT_NOTE_JOINT_MISS
        );
        assert_eq!(classify_joint_far(JOINT_FAR_ERR - 0.001, true), None);
    }

    /// The analysis grid IS the render's grid. [`mask_weights`] and
    /// `render::apply_masks`' own `weight_at` must ask about the same points,
    /// or every zone is SOLVED on a population half a pixel away from the one
    /// the mask will actually reach (R29 C2 — `render::MASK_SAMPLE_CENTRE`).
    ///
    /// The fixture is the discriminating one: a 2-wide raster [0, 255] read by
    /// a 4-wide frame. At pixel centres, `sx = nx·2 − 0.5` over
    /// nx = 0.125/0.375/0.625/0.875 gives −0.25, 0.25, 0.75, 1.25, which clamp
    /// to 0, ¼, ¾, 1 — symmetric about the frame centre and reaching BOTH
    /// ends. At the refuted `x/w` the same raster reads 0, ½, 1, 1.
    #[test]
    fn zone_moments_sample_on_the_renders_own_grid() {
        let mut m = GrayImage::new(2, 1);
        m.put_pixel(0, 0, image::Luma([0]));
        m.put_pixel(1, 0, image::Luma([255]));
        assert_eq!(mask_weights(&m, 4, 1), vec![0.0, 0.25, 0.75, 1.0]);
    }

    #[test]
    fn zone_moments_use_only_the_weighted_pixels() {
        // Two distinct populations; a binary mask must reproduce the selected
        // population's stats exactly, and `share` must count the weights.
        let px = [
            [0.8f32, 0.2, 0.2], // red-ish (masked out)
            [0.8, 0.2, 0.2],
            [0.2, 0.2, 0.8], // blue-ish (selected)
            [0.2, 0.2, 0.8],
        ];
        let m = zone_moments(&px, &[0.0, 0.0, 1.0, 1.0]);
        assert!((m.share - 0.5).abs() < 1e-6, "share {}", m.share);
        let b_lin = render::srgb_to_linear(0.8);
        let d_lin = render::srgb_to_linear(0.2);
        assert!((m.mean_lin[2] - b_lin).abs() < 1e-6, "blue mean {}", m.mean_lin[2]);
        assert!((m.mean_lin[0] - d_lin).abs() < 1e-6, "red mean {}", m.mean_lin[0]);
        assert!((m.chroma - 0.6).abs() < 1e-6, "chroma {}", m.chroma);
        // Soft weights: half-weight pixels still average to the same MEANS
        // (weights normalise out) but halve the share.
        let soft = zone_moments(&px, &[0.0, 0.0, 0.5, 0.5]);
        assert!((soft.mean_lin[2] - b_lin).abs() < 1e-6);
        assert!((soft.share - 0.25).abs() < 1e-6, "soft share {}", soft.share);
        // Degenerate mask: share 0, no NaNs.
        let dead = zone_moments(&px, &[0.0; 4]);
        assert_eq!(dead.share, 0.0);
        assert!(dead.luma_lin == 0.0 && dead.chroma == 0.0);
    }

    #[test]
    fn zone_dials_recover_a_known_channel_transform() {
        // Forward-transform a zone with known per-channel linear gains, then
        // ask the fit to recover them from moments alone. The TOTAL demand
        // (gains × 2^EV) must reproduce the true gains exactly — the EV/gain
        // SPLIT is a rendering choice, the product is the identified move.
        let g_true = [1.9f32, 1.1, 0.45];
        let src: Vec<[f32; 3]> = vec![
            [0.30, 0.35, 0.45],
            [0.40, 0.42, 0.50],
            [0.25, 0.30, 0.38],
            [0.35, 0.38, 0.46],
        ];
        let lin = |c: f32| render::srgb_to_linear(c);
        let srgb = |c: f32| {
            if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
        };
        let tgt: Vec<[f32; 3]> = src
            .iter()
            .map(|p| {
                let mut q = [0.0f32; 3];
                for c in 0..3 {
                    q[c] = srgb((lin(p[c]) * g_true[c]).clamp(0.0, 1.0));
                }
                q
            })
            .collect();
        let w = vec![1.0f32; src.len()];
        let ms = zone_moments(&src, &w);
        let mt = zone_moments(&tgt, &w);
        let d = fit_zone_dials(&ms, &mt);
        let bright = 2.0f32.powf(d.exposure_ev);
        for (c, &truth) in g_true.iter().enumerate() {
            let total = d.color_gains[c] * bright;
            assert!(
                (total - truth).abs() < 5e-3,
                "channel {c}: recovered {total} vs true {truth}"
            );
        }
        // The brightness split itself must be sane (the transform brightens
        // red, dims blue — net luma slightly up).
        assert!(d.exposure_ev.abs() < 1.0, "ev {}", d.exposure_ev);
    }

    #[test]
    fn zone_dials_are_neutral_for_matching_zones() {
        let px: Vec<[f32; 3]> = vec![[0.6, 0.63, 0.67], [0.55, 0.58, 0.62]];
        let w = vec![1.0f32; px.len()];
        let m1 = zone_moments(&px, &w);
        let m2 = zone_moments(&px, &w);
        let d = fit_zone_dials(&m1, &m2);
        assert!(d.exposure_ev.abs() < 0.01, "ev {}", d.exposure_ev);
        for c in 0..3 {
            assert!((d.color_gains[c] - 1.0).abs() < 0.01, "gain {c}: {}", d.color_gains[c]);
        }
        assert!(zone_sat_step(m1.chroma, m2.chroma).is_none(), "sat must converge");
    }

    #[test]
    fn zone_dials_turn_a_pale_sky_golden_through_the_engine() {
        // The acceptance geometry of the real failure (_DSC9621 ×
        // reimagine-5, batch #2): hazy pale-BLUE sky, vivid GOLD target sky
        // (the fixtures fit.rs's rotation-gate tests pin). The zoned dials,
        // applied through the engine's bitmap-mask recolour stage, must land
        // the sky in the target's warm family — exactly the regrade the
        // global fit refuses by design — and leave the rocks equal to the
        // control render. (A Temp/Tint-only variant of this test was tried
        // first and could NOT pass: WB gains cap at r/b ≈ 1.9× where this
        // repaint demands ≈ 5.3× — that measurement is why color_gains
        // exists.)
        use crate::recipe::{EditRecipe, LocalAdjustment, MaskGeometry};
        use image::{DynamicImage, GrayImage, RgbImage};

        let (w, h) = (16u32, 16u32);
        let sky_src = [0.60f32, 0.63, 0.67]; // hazy pale blue (hue ≈ 214°)
        let sky_tgt = [0.92f32, 0.72, 0.48]; // vivid gold
        let rock = [0.55f32, 0.45, 0.35];
        let build = |sky: [f32; 3]| -> DynamicImage {
            let img = RgbImage::from_fn(w, h, |_, y| {
                let p = if y >= 12 { sky } else { rock };
                image::Rgb(p.map(|c| (c * 255.0).round() as u8))
            });
            DynamicImage::ImageRgb8(img)
        };
        let src = build(sky_src);
        let tgt = build(sky_tgt);
        // Binary sky mask on disk — the production carrier (Bitmap geometry).
        let mask_path = fixture_mask_path("zoned-dials-mask");
        GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]))
            .save(mask_path.path())
            .unwrap();

        let px_of = |img: &DynamicImage| -> Vec<[f32; 3]> {
            img.to_rgb8()
                .pixels()
                .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
                .collect()
        };
        let weights: Vec<f32> = (0..w * h).map(|i| if i / w >= 12 { 1.0 } else { 0.0 }).collect();
        let ms = zone_moments(&px_of(&src), &weights);
        let mt = zone_moments(&px_of(&tgt), &weights);
        assert!(ms.share >= MIN_ZONE_SHARE && mt.share >= MIN_ZONE_SHARE);
        let d = fit_zone_dials(&ms, &mt);
        assert!(
            d.color_gains[0] > 1.2 && d.color_gains[2] < 0.6,
            "blue→gold demands strong warm gains: {:?}",
            d.color_gains
        );

        let recipe = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: mask_path.path().to_string_lossy().into_owned() },
                role: MaskRole::ZoneSky,
                amount: 1.0,
                exposure_ev: d.exposure_ev,
                color_gains: Some(d.color_gains),
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = px_of(&render::develop_preview(&src, &recipe));
        let control = px_of(&render::develop_preview(&src, &EditRecipe::default()));
        let sky_i = (14 * w + 8) as usize;
        let rock_i = (4 * w + 8) as usize;
        // Sky: source has b > r (blue); the zoned render must land it in the
        // target's warm family (r > g > b) with a clear warm margin, near
        // the target colour (the EV rides the tone LUT's shoulder, so exact
        // equality is not expected — family + proximity is the contract).
        let sky = out[sky_i];
        assert!(sky[0] > sky[2] + 0.10, "sky must turn warm (r >> b): {sky:?}");
        assert!(sky[0] > sky[1] && sky[1] > sky[2], "gold orders r > g > b: {sky:?}");
        for c in 0..3 {
            assert!(
                (sky[c] - sky_tgt[c]).abs() < 0.25,
                "sky channel {c} far from the target: {sky:?} vs {sky_tgt:?}"
            );
        }
        // Rocks: outside the mask — must match the control render.
        for c in 0..3 {
            assert!(
                (out[rock_i][c] - control[rock_i][c]).abs() < 1e-4,
                "rocks must be untouched: {:?} vs {:?}",
                out[rock_i],
                control[rock_i]
            );
        }
        mask_path.remove();
    }

    #[test]
    fn zoned_color_gains_cannot_move_a_zero_evidence_hue_band() {
        let (w, h) = (64u32, 64u32);
        let build = |gold: bool| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                let ripple = (x as f32 / (w - 1) as f32) * 0.08;
                let p = if y < h / 2 {
                    if gold {
                        [0.82 + ripple, 0.62 + 0.5 * ripple, 0.34 + 0.3 * ripple]
                    } else {
                        [0.34 + 0.3 * ripple, 0.55 + 0.5 * ripple, 0.82 + ripple]
                    }
                } else {
                    [0.42 + ripple, 0.34 + 0.5 * ripple, 0.25 + 0.3 * ripple]
                };
                image::Rgb(p.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8))
            }))
        };
        let src = build(false);
        let tgt = build(true);
        let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
        let path = fixture_mask_path("zoned-evidence-hue");
        mask.save(path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);

        attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

        let sky = report
            .recipe
            .masks
            .iter()
            .find(|mask| mask.role == MaskRole::ZoneSky)
            .expect("the supported luma correction must survive the refused hue band");
        assert_eq!(sky.color_gains, Some([1.0; 3]));
        assert_eq!(sky.saturation, 0.0);
        let note = report
            .notes
            .iter()
            .find(|note| note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR)
            .unwrap_or_else(|| panic!("the zoned refusal was silent: {}", report.recipe.rationale));
        assert!(
            note.args.iter().any(|(key, value)| *key == "hue_bands" && value != "none"),
            "the refused hue band must be named: {note:?}"
        );
        path.remove();
    }

    // ---- orchestration ----------------------------------------------------

    use image::{DynamicImage, GrayImage, RgbImage};

    /// The golden-pair toy geometry shared by the orchestration tests:
    /// identical warm rocks (top 12 rows), only the sky differs.
    fn zoned_pair() -> (DynamicImage, DynamicImage, GrayImage) {
        let (w, h) = (16u32, 16u32);
        let build = |sky: [f32; 3]| -> DynamicImage {
            let img = RgbImage::from_fn(w, h, |_, y| {
                let p = if y >= 12 { sky } else { [0.55f32, 0.45, 0.35] };
                image::Rgb(p.map(|c| (c * 255.0).round() as u8))
            });
            DynamicImage::ImageRgb8(img)
        };
        let sky_mask =
            GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]));
        (build([0.60, 0.63, 0.67]), build([0.92, 0.72, 0.48]), sky_mask)
    }

    /// A mask fixture no OTHER process can pull out from under this one.
    /// These tests write a mask, render through it, then delete it — at a
    /// FIXED relative path under ./out that every concurrent `cargo test` on
    /// the same checkout shared, so one run's cleanup made another run's mask
    /// inert (the zone then "failed to attach" nowhere near its own code).
    /// Process-unique, in the temp dir, and no ./out litter left behind.
    fn fixture_mask_path(name: &str) -> crate::store::OwnedRaster {
        crate::store::OwnedRaster::scratch(
            std::env::temp_dir().join(format!("autoshop-{name}-{}.png", std::process::id())),
        )
    }

    fn semantic_attachment(
        sw: Vec<f32>,
        tw: Vec<f32>,
        path: &crate::store::OwnedRaster,
    ) -> ZoneAttachment {
        ZoneAttachment {
            source_weights: sw,
            target_weights: tw,
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            range: None,
            name: String::new(),
            role: MaskRole::ZoneSky,
            inverted: false,
            label: MaskRole::ZoneSky.tag().to_string(),
            frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
        }
    }

    fn divergence(d: f32) -> fit::Divergence {
        fit::Divergence { correlation: 1.0 - d, energy_error: 0.0, d }
    }

    fn neutral_report(src: &DynamicImage, tgt: &DynamicImage) -> fit::FitReport {
        let s = src.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let t = tgt.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let err = fit::look_err(&fit::pixels_of(&s), &fit::pixels_of(&t));
        fit::FitReport {
            correspondence: None,
            recipe: crate::recipe::EditRecipe::default(),
            err_before: err,
            err_after: err,
            notes: Vec::new(),
            mode: fit::FitMode::Full,
            divergence: divergence(0.0),
            evidence: fit::evidence_model_for(
                &fit::pixels_of(&s),
                &fit::pixels_of(&t),
                s.width(),
                s.height(),
            ),
        }
    }

    #[test]
    fn zone_divergence_uses_the_same_source_mask_on_both_sides() {
        let (src, tgt, source_mask) = zoned_pair();
        let measured = measure_zone_divergence(
            &src,
            &tgt,
            &crate::recipe::EditRecipe::default(),
            &source_mask,
        );
        let (sp, tp, w, h) =
            fit::divergence_raster(&src, &tgt, &crate::recipe::EditRecipe::default());
        let source_weights = mask_weights(&source_mask, w, h);
        let direct = fit::structure_divergence(&sp, &tp, w, h, &source_weights);
        assert_eq!(measured.sky.divergence, direct);

        // A deliberately wrong target-derived population produces a different
        // reading. The production helper cannot receive this mask: D owns the
        // source correspondence while moment matching remains target-masked.
        let target_mask = GrayImage::from_fn(16, 16, |_, y| {
            image::Luma([if y < 6 { 255u8 } else { 0 }])
        });
        let wrong_weights = mask_weights(&target_mask, w, h);
        let wrong = fit::structure_divergence(&sp, &tp, w, h, &wrong_weights);
        assert!(
            (wrong.d - direct.d).abs() > 0.05,
            "the two mask populations must be discriminating: source={direct:?}, target={wrong:?}"
        );

        // Optional measured calibration; the corpus is located by an
        // environment variable (`fit::calibration_dir`), never by a path
        // literal in the source.
        let Some(fixture) = fit::calibration_corpus() else { return };
        let saved = fixture.join("sky-mask.png");
        if fixture.join("neutral.jpg").exists() && saved.exists() {
            let source = image::open(fixture.join("neutral.jpg")).unwrap();
            let target = image::open(fixture.join("target.jpg")).unwrap();
            let mask = image::open(&saved).unwrap().to_luma8();
            let actual = measure_zone_divergence(
                &source,
                &target,
                &crate::recipe::EditRecipe::default(),
                &mask,
            );
            eprintln!(
                "CALIBRATION_DIVERGENCE sky={:.6} land={:.6}",
                actual.sky.divergence.d,
                actual.land.divergence.d
            );
            assert!(
                (actual.sky.divergence.d - 1.186).abs() <= 0.05,
                "sky calibration drifted: {:?}",
                actual.sky.divergence
            );
            assert!(
                (actual.land.divergence.d - 0.436).abs() <= 0.05,
                "land calibration drifted: {:?}",
                actual.land.divergence
            );
        }
    }

    #[test]
    fn atmosphere_zone_shrinks_gains_toward_unity_but_keeps_their_direction() {
        let original = [1.49f32, 0.83, 0.69];
        let shrunk = shrink_atmosphere_gains(original);
        assert!(shrunk
            .iter()
            .all(|g| (ZONE_ATMOS_GAIN_MIN..=ZONE_ATMOS_GAIN_MAX).contains(g)));
        let mut common_k: Option<f32> = None;
        for channel in 0..3 {
            assert_eq!(
                (original[channel] - 1.0).signum(),
                (shrunk[channel] - 1.0).signum(),
                "channel {channel} reversed its hue direction"
            );
            assert!((shrunk[channel] - 1.0).abs() <= (original[channel] - 1.0).abs());
            let k = (shrunk[channel] - 1.0) / (original[channel] - 1.0);
            if let Some(first) = common_k {
                assert!((k - first).abs() <= 1e-6, "channels did not share one shrink scalar");
            } else {
                common_k = Some(k);
            }
        }
        assert!(
            shrunk.iter().any(|g| {
                (*g - ZONE_ATMOS_GAIN_MIN).abs() <= 1e-6
                    || (*g - ZONE_ATMOS_GAIN_MAX).abs() <= 1e-6
            }),
            "the largest legal k must reach a budget boundary: {shrunk:?}"
        );
    }

    #[test]
    fn a_divergent_zone_is_still_attached_in_atmosphere_mode() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-atmos-attached");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        attach_zones_with_divergence(
            &src,
            &tgt,
            &mut report,
            &sky_mask,
            &sky_mask,
            &mask_path,
            ZoneDivergences {
                sky: ZoneDivergence { divergence: divergence(0.80), share: 0.25 },
                land: ZoneDivergence { divergence: divergence(0.0), share: 0.75 },
            },
        );
        let sky = report
            .recipe
            .masks
            .iter()
            .find(|m| m.role == MaskRole::ZoneSky)
            .unwrap_or_else(|| panic!("Atmosphere luma correction was lost: {}", report.recipe.rationale));
        assert_eq!(sky.color_gains, Some([1.0; 3]));
        assert_eq!(sky.saturation, 0.0);
        assert!(report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE));
        assert!(report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR));
        mask_path.remove();
    }

    #[test]
    fn atmosphere_zone_skips_the_within_zone_cdf_solve() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-atmos-no-cdf");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        attach_zones_with_divergence(
            &src,
            &tgt,
            &mut report,
            &sky_mask,
            &sky_mask,
            &mask_path,
            ZoneDivergences {
                sky: ZoneDivergence { divergence: divergence(0.80), share: 0.25 },
                land: ZoneDivergence { divergence: divergence(0.0), share: 0.75 },
            },
        );
        let sky = report
            .recipe
            .masks
            .iter()
            .find(|mask| mask.role == MaskRole::ZoneSky)
            .expect("the Atmosphere luma correction must survive the refused hue band");
        assert_eq!(sky.color_gains, Some([1.0; 3]));
        assert_eq!(sky.saturation, 0.0);
        assert!(report
            .notes
            .iter()
            .any(|note| note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR));
        mask_path.remove();
    }

    #[test]
    fn a_matching_zone_next_to_a_divergent_one_keeps_full_mode() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-independent-modes");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        attach_zones_with_divergence(
            &src,
            &tgt,
            &mut report,
            &sky_mask,
            &sky_mask,
            &mask_path,
            ZoneDivergences {
                sky: ZoneDivergence { divergence: divergence(0.80), share: 0.25 },
                land: ZoneDivergence { divergence: divergence(0.20), share: 0.75 },
            },
        );
        assert!(report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE));
        assert!(report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_MODE_FULL));
        mask_path.remove();
    }

    fn boundary_fixture_pixels(rim_each_side: f32) -> (Vec<[f32; 3]>, Vec<f32>, u32, u32) {
        let (w, h) = (12u32, 4u32);
        let line_weights = [1.0, 1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.2, 0.0, 0.0, 0.0, 0.0];
        let mut pixels = Vec::with_capacity((w * h) as usize);
        let mut weights = Vec::with_capacity((w * h) as usize);
        for _ in 0..h {
            for (x, weight) in line_weights.iter().copied().enumerate() {
                let value = match x {
                    0..=3 => 0.20,
                    4..=5 => 0.20 + rim_each_side,
                    6..=7 => 0.40 - rim_each_side,
                    _ => 0.40,
                };
                pixels.push([value; 3]);
                weights.push(weight);
            }
        }
        (pixels, weights, w, h)
    }

    fn soft_zone_pair(
        name: &str,
        sky_ev: f32,
        land_ev: f32,
    ) -> (DynamicImage, GrayImage, crate::store::OwnedRaster, fit::FitReport) {
        let (w, h) = (32u32, 12u32);
        let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([115; 3])));
        let mask = GrayImage::from_fn(w, h, |x, _| {
            let value = if x < 8 {
                255
            } else if x >= 24 {
                0
            } else {
                (((23 - x) as f32 / 15.0) * 255.0).round() as u8
            };
            image::Luma([value])
        });
        let path = fixture_mask_path(name);
        mask.save(path.path()).unwrap();
        let geometry = MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() };
        let mut report = neutral_report(&source, &source);
        report.recipe.masks = vec![
            LocalAdjustment {
                mask: geometry.clone(),
                role: MaskRole::ZoneSky,
                amount: 1.0,
                exposure_ev: sky_ev,
                color_gains: Some([0.94, 0.98, 1.02]),
                ..Default::default()
            },
            LocalAdjustment {
                mask: geometry,
                role: MaskRole::ZoneLand,
                amount: 1.0,
                inverted: true,
                exposure_ev: land_ev,
                color_gains: Some([1.03, 1.01, 0.98]),
                ..Default::default()
            },
        ];
        (source, mask, path, report)
    }

    fn note_number(note: &crate::rationale::Note, name: &str) -> f32 {
        note.args
            .iter()
            .find(|(key, _)| *key == name)
            .unwrap_or_else(|| panic!("missing {name} in {:?}", note.args))
            .1
            .parse()
            .unwrap()
    }

    #[test]
    fn boundary_rim_is_measured_across_the_mask_transition_band() {
        let (pixels, weights, w, h) = boundary_fixture_pixels(0.12);
        let reading = boundary_rim(&pixels, &weights, w, h);
        assert_eq!(reading.transitions, h as usize, "one feather crossing per row");
        assert!(
            (reading.rim - 0.12).abs() <= 1e-6,
            "the brightest sky-half deviation must be measured against the settled sky: {reading:?}"
        );
    }

    #[test]
    fn opposite_sign_zone_pair_exceeds_the_rim_budget_before_shrinking() {
        assert_eq!(ZONE_BOUNDARY_RIM_MAX, 0.012, "the measured calibration is pinned");
        let (pixels, weights, w, h) = boundary_fixture_pixels(0.013);
        let reading = boundary_rim(&pixels, &weights, w, h);
        assert!(
            reading.rim > ZONE_BOUNDARY_RIM_MAX,
            "the just-over-budget opposite-sign shape must exercise the gate: {reading:?}"
        );
    }

    #[test]
    fn rim_shrink_keeps_each_zones_direction_and_lands_inside_the_budget() {
        let (source, mask, path, mut report) = soft_zone_pair("zoned-rim-shrink", -0.65, 0.20);
        let weights = mask_weights(&mask, source.width(), source.height());
        let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        let verdict =
            enforce_boundary_gate(&source, &mut report, &weights, &[0.5, 0.5], 0, initial);
        let BoundaryGateResult::Kept { k, before, after, .. } = verdict else {
            panic!("a shrinkable pair was dropped: {}", report.recipe.rationale);
        };
        assert!(before.rim > ZONE_BOUNDARY_RIM_MAX, "premise: {before:?}");
        assert!((0.0..1.0).contains(&k), "the largest passing k must really shrink: {k}");
        assert!(after.rim <= ZONE_BOUNDARY_RIM_MAX, "shrunk rim: {after:?}");
        let sky = &report.recipe.masks[0];
        let land = &report.recipe.masks[1];
        assert!(sky.exposure_ev < 0.0, "a darkening sky reversed: {}", sky.exposure_ev);
        assert!(land.exposure_ev > 0.0, "a brightening land reversed: {}", land.exposure_ev);
        assert!((sky.exposure_ev / -0.65 - k).abs() < 1e-5);
        assert!((land.exposure_ev / 0.20 - k).abs() < 1e-5);
        for (gain, original) in sky.color_gains.unwrap().into_iter().zip([0.94, 0.98, 1.02]) {
            assert_eq!((gain - 1.0).signum(), (original - 1.0f32).signum());
            assert!(((gain - 1.0) / (original - 1.0) - k).abs() < 1e-5);
        }
        path.remove();
    }

    #[test]
    fn same_sign_zone_pair_needs_no_shrink() {
        let (pixels, weights, w, h) = boundary_fixture_pixels(-0.02);
        let reading = boundary_rim(&pixels, &weights, w, h);
        assert!(reading.rim < 0.0, "the previous-fit direction must not look like a bright rim");

        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let p = pixels[(y * w + x) as usize];
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        }));
        let mut report = neutral_report(&source, &source);
        report.recipe.masks = vec![
            LocalAdjustment { role: MaskRole::ZoneSky, exposure_ev: -0.35, ..Default::default() },
            LocalAdjustment { role: MaskRole::ZoneLand, exposure_ev: -0.90, ..Default::default() },
        ];
        // Pin the same-sign policy independently of scene content: a monotone
        // transition reading is already inside budget, so the gate must keep
        // the candidate exactly at k=1.
        let verdict =
            enforce_boundary_gate(&source, &mut report, &weights, &[0.5, 0.5], 0, pixels);
        let BoundaryGateResult::Kept { k, after, .. } = verdict else {
            panic!("same-sign pair was dropped");
        };
        assert_eq!(k, 1.0);
        assert_eq!(after.rim, reading.rim);
        assert_eq!(report.recipe.masks[0].exposure_ev, -0.35);
        assert_eq!(report.recipe.masks[1].exposure_ev, -0.90);
    }

    #[test]
    fn every_accepted_fixture_zone_still_passes_the_boundary_gate() {
        let (src, tgt, sky_mask) = zoned_pair();
        let path = fixture_mask_path("zoned-boundary-calibration-sky");
        sky_mask.save(path.path()).unwrap();
        let mut sky_report = fit::fit_recipe(&src, &tgt);
        attach_zones(&src, &tgt, &mut sky_report, &sky_mask, &sky_mask, &path);

        let (w, h) = (16u32, 16u32);
        let build = |sky: [f32; 3], rock: [f32; 3]| -> DynamicImage {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |_, y| {
                let p = if y >= 12 { sky } else { rock };
                image::Rgb(p.map(|c| (c * 255.0).round() as u8))
            }))
        };
        let land_src = build([0.60, 0.63, 0.67], [0.45, 0.42, 0.40]);
        let land_tgt = build([0.92, 0.72, 0.48], [0.80, 0.50, 0.28]);
        let land_path = fixture_mask_path("zoned-boundary-calibration-land");
        sky_mask.save(land_path.path()).unwrap();
        let mut land_report = fit::fit_recipe(&land_src, &land_tgt);
        attach_zones(
            &land_src,
            &land_tgt,
            &mut land_report,
            &sky_mask,
            &sky_mask,
            &land_path,
        );

        let mut measured = Vec::new();
        for (fixture, report) in [("sky", &sky_report), ("sky+land", &land_report)] {
            let note = report
                .notes
                .iter()
                .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED)
                .unwrap_or_else(|| panic!("{fixture} lacked a boundary verdict: {}", report.recipe.rationale));
            let rim = note_number(note, "after");
            assert!(rim <= ZONE_BOUNDARY_RIM_MAX, "{fixture} rim {rim:.3}");
            for mask in &report.recipe.masks {
                measured.push((fixture, mask.role, rim));
            }
        }
        assert_eq!(measured.len(), 4, "every accepted zone must reach the boundary gate: {measured:?}");
        assert!(
            measured.iter().any(|(_, role, _)| *role == MaskRole::ZoneSky)
                && measured.iter().any(|(_, role, _)| *role == MaskRole::ZoneLand),
            "both supported zone classes may reach the boundary gate: {measured:?}"
        );
        let expected = [-0.004f32, -0.004, -0.004, -0.004];
        for ((fixture, role, rim), expected) in measured.iter().zip(expected) {
            assert!(
                (*rim - expected).abs() <= 0.002,
                "{fixture}/{role:?} boundary calibration drifted: {rim:.3} vs {expected:.3}"
            );
        }
        assert!(
            measured.iter().all(|(_, _, rim)| *rim <= ZONE_BOUNDARY_RIM_MAX),
            "accepted fixture calibration: {measured:?}"
        );
        path.remove();
        land_path.remove();
    }

    #[test]
    fn a_rim_that_cannot_be_shrunk_is_dropped_with_its_own_note() {
        let (pixels, line_weights, w, h) = boundary_fixture_pixels(0.03);
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let p = pixels[(y * w + x) as usize];
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        }));
        let mask = GrayImage::from_fn(w, h, |x, y| {
            image::Luma([(line_weights[(y * w + x) as usize] * 255.0).round() as u8])
        });
        let path = fixture_mask_path("zoned-rim-unshrinkable");
        mask.save(path.path()).unwrap();
        let geometry = MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() };
        let mut report = neutral_report(&source, &source);
        report.recipe.masks = vec![
            LocalAdjustment {
                mask: geometry.clone(),
                role: MaskRole::ZoneSky,
                amount: 1.0,
                exposure_ev: -0.2,
                ..Default::default()
            },
            LocalAdjustment {
                mask: geometry,
                role: MaskRole::ZoneLand,
                amount: 1.0,
                inverted: true,
                exposure_ev: 0.1,
                ..Default::default()
            },
        ];
        let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        let verdict = enforce_boundary_gate(
            &source,
            &mut report,
            &line_weights,
            &[0.5, 0.5],
            0,
            initial,
        );
        assert!(matches!(verdict, BoundaryGateResult::Dropped));
        assert!(report.recipe.masks.is_empty(), "the failed pair must be removed");
        let note = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_DROPPED)
            .expect("the boundary failure needs its own typed note");
        assert_eq!(note_number(note, "k"), 0.0);
        assert!(note_number(note, "after") > ZONE_BOUNDARY_RIM_MAX);
        assert!(report.recipe.rationale.contains("even shared shrink k=0"));
        path.remove();
    }

    #[test]
    fn boundary_gate_discloses_the_applied_shrink_and_measured_rim() {
        let (source, mask, path, mut report) = soft_zone_pair("zoned-rim-disclosure", -0.65, 0.20);
        let weights = mask_weights(&mask, source.width(), source.height());
        let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        let verdict =
            enforce_boundary_gate(&source, &mut report, &weights, &[0.5, 0.5], 0, initial);
        let BoundaryGateResult::Kept { k, before, after, .. } = verdict else {
            panic!("disclosure fixture dropped");
        };
        let note = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED)
            .expect("typed boundary pass note");
        assert!((note_number(note, "k") - k).abs() <= 0.0005);
        assert!((note_number(note, "before") - before.rim).abs() <= 0.0005);
        assert!((note_number(note, "after") - after.rim).abs() <= 0.0005);
        assert!(report.recipe.rationale.contains("signed transition rim"));
        assert!(report.recipe.rationale.contains("shared differential shrink k="));
        path.remove();
    }

    #[test]
    fn local_quality_gate_rejects_texture_amplification_and_crushing() {
        let (w, h) = (16u32, 16u32);
        let before: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let ripple = if (i + i / w) % 2 == 0 { -0.025 } else { 0.025 };
                [0.45 + ripple; 3]
            })
            .collect();
        let amplified: Vec<[f32; 3]> = before
            .iter()
            .map(|p| [0.45 + 3.0 * (p[0] - 0.45); 3])
            .collect();
        let crushed = vec![[0.45; 3]; before.len()];
        let clean: Vec<[f32; 3]> = before.iter().map(|p| [p[0] + 0.02; 3]).collect();
        let clipped: Vec<[f32; 3]> = (0..before.len())
            .map(|i| if i % 2 == 0 { [0.0; 3] } else { [1.0; 3] })
            .collect();
        let weights = vec![1.0; before.len()];
        let high = local_quality(&before, &amplified, &weights, w, h);
        let low = local_quality(&before, &crushed, &weights, w, h);
        let good = local_quality(&before, &clean, &weights, w, h);
        let clip = local_quality(&before, &clipped, &weights, w, h);
        assert!(!high.texture_passes() && !high.passes(), "amplification passed: {high:?}");
        assert!(!low.texture_passes() && !low.passes(), "crushing passed: {low:?}");
        assert!(good.passes(), "clean correction failed: {good:?}");
        assert!(!clip.clipping_passes() && !clip.passes(), "clipping half was bypassed: {clip:?}");
    }

    #[test]
    fn local_quality_gate_passes_every_accepted_fixture_zone() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-quality-calibration");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
        assert!(
            !report.recipe.masks.is_empty(),
            "the accepted zoned fixture was lost: {}",
            report.recipe.rationale
        );
        let passed = report
            .notes
            .iter()
            .filter(|n| n.key == crate::rationale::keys::ZONE_QUALITY_PASSED)
            .count();
        assert_eq!(passed, report.recipe.masks.len(), "every accepted zone needs a pass verdict");
        assert!(
            !report.notes.iter().any(|n| {
                n.key == crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED
                    || n.key == crate::rationale::keys::ZONE_QUALITY_CLIPPING_FAILED
            }),
            "an accepted calibration zone failed local quality: {}",
            report.recipe.rationale
        );
        mask_path.remove();

        // Calibrate the rejecting side on the saved generated-cloud correction
        // when the supervisor material is present. Rendering the same global
        // recipe with and without its two bitmap zones isolates local quality.
        let Some(material) = fit::calibration_corpus() else { return };
        let saved_mask = material.join("sky-mask.png");
        let raw = material.join("source.arw");
        if material.join("fitted.recipe.json").exists() && saved_mask.exists() && raw.exists() {
            let text = std::fs::read_to_string(material.join("fitted.recipe.json")).unwrap();
            let with_zones: crate::recipe::EditRecipe = serde_json::from_str(&text).unwrap();
            let mut without_zones = with_zones.clone();
            without_zones.masks.clear();
            let before_image =
                render::render_to_image(&raw, &without_zones, None, Some(384)).unwrap();
            let after_image =
                render::render_to_image(&raw, &with_zones, None, Some(384)).unwrap();
            let before = fit::pixels_of(&before_image);
            let after = fit::pixels_of(&after_image);
            let weights = mask_weights(
                &image::open(&saved_mask).unwrap().to_luma8(),
                before_image.width(),
                before_image.height(),
            );
            let saved = local_quality(&
                before,
                &after,
                &weights,
                before_image.width(),
                before_image.height(),
            );
            assert!(
                (saved.texture_ratio - 0.961).abs() <= 0.02,
                "saved generated-cloud quality calibration drifted: {saved:?}"
            );
            assert!(
                saved.passes(),
                "the specified mean-gradient/clipping statistic cannot honestly reject the saved correction; this measured contradiction is reported: {saved:?}"
            );
        }
    }

    /// R18: the acceptance predicate's regimes, pinned on the live numbers.
    /// Halving accepts (0.076 → 0.007) — and the relative arm must carry
    /// that verdict ALONE above the floor (0.500 → 0.200), so deleting it
    /// fails here. The floor arm accepts a sub-50% correction that lands
    /// matched with a real gain (0.035 → 0.018) but refuses a hairline
    /// move that would only buy the drift budget (0.021 → 0.0205). Neither
    /// arm rescues a correction that stays high (0.507 → 0.280) or lands
    /// above the floor at sub-50% (0.040 → 0.025).
    #[test]
    fn the_zone_gate_halves_or_lands_matched() {
        assert!(zone_accepts(0.076, 0.007, 0.1), "a real halving must pass");
        assert!(zone_accepts(0.500, 0.200, 0.9), "the relative arm alone must pass");
        assert!(zone_accepts(0.035, 0.018, 0.1), "landing under the matched floor must pass");
        // The (skip, floor] band is ATTEMPTED, not declined (R19): a zone
        // starting between 0.012 and 0.02 can still earn its correction.
        assert!(zone_accepts(0.016, 0.010, 0.1), "the between-yardsticks band must stay winnable");
        assert!(!zone_accepts(0.021, 0.0205, 0.1), "a hairline move must not buy the drift budget");
        assert!(!zone_accepts(0.507, 0.280, 0.1), "a large remaining error must refuse");
        assert!(!zone_accepts(0.040, 0.025, 0.1), "sub-50% above the floor must refuse");
        // The floor arm calls a landing "matched" only when its BRIGHTNESS
        // matches too — a dark zone can score 0.018 while a stop away.
        assert!(!zone_accepts(0.035, 0.018, 0.9), "an EV-far landing is not matched");
    }

    /// R19: the SKIP/floor split itself, pinned — setting the skip back to
    /// the acceptance floor would silently re-decline the (0.012, 0.02]
    /// band untried (the regression this split exists to prevent).
    #[test]
    fn the_skip_line_sits_below_the_acceptance_floor() {
        assert!(zone_skips(0.009, 0.1), "the observed matched domain skips");
        assert!(zone_skips(0.012, 0.1), "the domain ceiling itself skips");
        assert!(!zone_skips(0.0121, 0.1), "just above the ceiling is attempted");
        assert!(!zone_skips(0.016, 0.1), "the between-yardsticks band is attempted");
        assert!(!zone_skips(0.009, 0.5), "a matched score a stop apart is attempted");
    }

    /// R18: a zone that already matches the target is LEFT ALONE with an
    /// honest note — not "corrected", not reported as a dropped
    /// improvement (the murk-era pair's sky read 0.012, got dialled, and
    /// the "dropped: needs ≤ 50%" outcome line was mistaken for a
    /// discarded win three rounds running). Identical frames: both zones
    /// match, nothing attaches, the raster is reclaimed.
    #[test]
    fn an_already_matched_zone_is_left_alone_and_says_so() {
        let (src, _tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-matched-mask");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&src, &src);
        attach_zones(&src, &src, &mut report, &sky_mask, &sky_mask, &mask_path);
        assert!(
            report.recipe.masks.is_empty(),
            "nothing to correct on an identical pair: {}",
            report.recipe.rationale
        );
        assert!(
            report.recipe.rationale.contains("already matches the target"),
            "the honest note must replace the misleading drop line: {}",
            report.recipe.rationale
        );
        assert!(
            !report.recipe.rationale.contains("correction dropped"),
            "no drop line on a matched zone: {}",
            report.recipe.rationale
        );
        assert!(!mask_path.path().exists(), "no zone kept the raster — it must be reclaimed");
    }

    #[test]
    fn zoned_orchestration_attaches_the_sky_mask_and_improves_the_zone() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-orch-mask");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        let err_global = report.err_after;
        attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
        assert!(
            report.recipe.masks.iter().any(|m| {
                m.role == MaskRole::ZoneSky
                    && m.color_gains == Some([1.0; 3])
                    && m.saturation == 0.0
            }),
            "two-sided luma must retain the sky zone while one-sided hue is withheld: {}",
            report.recipe.rationale
        );
        assert!(
            report.recipe.masks.iter().any(|m| m.role == MaskRole::ZoneLand && m.inverted),
            "the independently supported land correction must still attach: {}",
            report.recipe.rationale
        );
        // The zoned gate judges each ZONE; frame-global error is only bounded
        // (the insurance tolerance, once per attached zone), never required
        // to improve.
        let bound = err_global
            + ZONE_GLOBAL_REGRESSION_TOL * report.recipe.masks.len() as f32;
        assert!(
            report.err_after <= bound,
            "zoned err {} exceeded the insurance bound {bound}",
            report.err_after
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR),
            "rationale must disclose the partial sky hue refusal: {}",
            report.recipe.rationale
        );
        assert!(
            report.recipe.rationale.contains("global fit only"),
            "rationale must carry the XMP honesty note: {}",
            report.recipe.rationale
        );
        // R23-6 A-4: confidence must NOT be the frame-global look error's
        // verdict any more. The old line was
        // `confidence = (1 - zoned_err * 6).clamp(0.25, 0.95)`, which on this
        // fixture reports a number derived from a metric the module's own
        // ZONE_ACCEPT_RATIO doc proves cannot see the zone. It now comes from
        // the accepted zones, and says so.
        assert!(
            report.recipe.rationale.contains("Confidence for this fit comes from"),
            "the zoned fit must say where its confidence came from: {}",
            report.recipe.rationale
        );
        let frame_verdict = fit::clamp_confidence(1.0 - report.err_after * 6.0);
        assert!(
            report.recipe.confidence != frame_verdict
                || (report.err_after - 0.0).abs() < 1e-6,
            "confidence still reads as the frame-global formula ({} vs {frame_verdict})",
            report.recipe.confidence
        );
        mask_path.remove();
    }

    #[test]
    fn synthetic_zone_survives_luminance_with_one_sided_hue_refusal() {
        let (w, h) = (64u32, 64u32);
        let build = |target: bool| DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = 0.30 + 0.30 * x as f32 / (w - 1) as f32;
            let p = if y < h / 2 {
                if target { [v * 1.5, v * 1.5, v * 1.5] } else { [v * 0.72, v * 0.86, v] }
            } else { [0.45, 0.38, 0.28] };
            image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        }));
        let src = build(false);
        let tgt = build(true);
        let sp = fit::pixels_of(&src);
        let tp = fit::pixels_of(&tgt);
        let evidence = fit::evidence_model_for(&sp, &tp, w, h);
        let blue = evidence.hue.iter().find(|r| r.label == "Blue").expect("blue evidence range");
        assert!(blue.source_populated && !blue.target_populated, "synthetic fixture must have one-sided hue evidence: {blue:?}");
        assert!(evidence.luma.iter().any(|r| r.weight > 0.0), "synthetic fixture must retain two-sided luma evidence");
        let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
        let path = fixture_mask_path("synthetic-zone-survival");
        mask.save(path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        let s_img = src.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let t_img = tgt.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let t_px = fit::pixels_of(&t_img);
        let sw = mask_weights(&mask, s_img.width(), s_img.height());
        let tw = mask_weights(&mask, t_img.width(), t_img.height());
        let attachment = semantic_attachment(sw, tw, &path);
        let mut frame_err = report.err_after;
        let accepted = attach_one_zone(
            &s_img,
            &t_px,
            &mut report,
            &mut frame_err,
            &attachment,
            measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &mask)
                .sky
                .divergence,
            None,
        );
        assert!(accepted.is_some(), "supported luminance must keep the synthetic sky zone: {}", report.recipe.rationale);
        let sky = report.recipe.masks.last().expect("accepted synthetic sky mask");
        assert_eq!(sky.color_gains, Some([1.0; 3]));
        assert_eq!(sky.saturation, 0.0);
        assert!(
            report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR),
            "the synthetic acceptance must disclose the refused hue band: {}",
            report.recipe.rationale
        );
        path.remove();
    }

    /// The mirror of the test above, and the branch nothing pinned: a
    /// structurally unsupported region must silence the zone's TONE controls
    /// and say so in its own words. A hand mutation that deleted the
    /// tone-zeroing left the whole library green, because every existing
    /// guard measured the COLOUR half of the same split.
    #[test]
    fn synthetic_zone_tone_refusal_is_named_by_luma_evidence() {
        let (w, h) = (96u32, 96u32);
        // ACHROMATIC everywhere: `fit::evidence_hue_band` returns None below
        // chroma 0.06, so the colour branch cannot fire and the note under
        // test is the only one that can appear.
        //
        // The sky half is STRUCTURALLY divergent — a smooth ramp against
        // hard stripes — because that is the only mechanism that can withhold
        // a luma range: bins are rank-paired (`fit::evidence_model_for`), so
        // a source bin is never target-empty at equal pixel counts, and the
        // withholding clause that remains is `!spatial_supported`. The ground
        // half is byte-identical on both sides, so its cells stay supported.
        let build = |target: bool| DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v: f32 = if y < h / 2 {
                if target {
                    if (y / 2) % 2 == 0 { 0.85 } else { 0.55 }
                } else {
                    0.35 + 0.20 * x as f32 / (w - 1) as f32
                }
            } else {
                0.18 + 0.12 * (((x / 8) + (y / 8)) % 2) as f32
            };
            image::Rgb([(v.clamp(0.0, 1.0) * 255.0).round() as u8; 3])
        }));
        let src = build(false);
        let tgt = build(true);
        let sp = fit::pixels_of(&src);
        let tp = fit::pixels_of(&tgt);
        let evidence = fit::evidence_model_for(&sp, &tp, w, h);
        let unsupported = evidence.spatial_supported.iter().filter(|&&s| !s).count();
        eprintln!(
            "TONE_FIXTURE unsupported={}/{} luma_withheld={} hue_bands_present={}",
            unsupported,
            evidence.spatial_supported.len(),
            evidence.luma.iter().filter(|r| r.source_populated && r.weight <= 0.0).count(),
            sp.iter().chain(tp.iter()).filter(|p| fit::evidence_hue_band(p).is_some()).count(),
        );
        assert!(
            sp.iter().chain(tp.iter()).all(|p| fit::evidence_hue_band(p).is_none()),
            "synthetic fixture must be achromatic so the colour branch cannot fire"
        );
        assert!(
            unsupported > 0,
            "synthetic fixture must contain structurally unsupported pixels"
        );
        let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
        let path = fixture_mask_path("synthetic-zone-tone-refusal");
        mask.save(path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        let s_img = src.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let t_img = tgt.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let t_px = fit::pixels_of(&t_img);
        let sw = mask_weights(&mask, s_img.width(), s_img.height());
        let tw = mask_weights(&mask, t_img.width(), t_img.height());
        let attachment = semantic_attachment(sw, tw, &path);
        let mut frame_err = report.err_after;
        attach_one_zone(
            &s_img,
            &t_px,
            &mut report,
            &mut frame_err,
            &attachment,
            measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &mask)
                .sky
                .divergence,
            None,
        );
        let note = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE)
            .unwrap_or_else(|| panic!("the zoned tone refusal was silent: {}", report.recipe.rationale));
        assert!(
            note.args.iter().any(|(k, v)| *k == "luma_ranges" && !v.is_empty() && v != "none"),
            "the refused luma range must be named: {note:?}"
        );
        assert!(
            !report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR),
            "an achromatic pair must not claim a refused hue band: {}",
            report.recipe.rationale
        );
        path.remove();
    }

    /// Step-7b conservation, zoned: an IDENTITY field (everything
    /// corresponds in place at full confidence) leaves every zone verdict
    /// byte-identical, and a ZERO-confidence field abstains wholesale — the
    /// field may refuse to help, never starve or drop a zone. The latter
    /// also pins the share GATE never reading the confidence (supervisor
    /// mutation M-7b-D: compose the gate with confidence and the zero-field
    /// run drops the zones this asserts equal).
    #[test]
    fn an_identity_or_abstaining_field_leaves_the_zone_verdicts_unchanged() {
        // The synthetic zoned fixture, not the calibration corpus: its two
        // renditions share one geometry by construction, which is what makes
        // the identity law EXACT (the corpus target is two rows taller than
        // its source, and on mismatched geometry an identity field genuinely
        // row-aligns the pairing — a real, wanted change documented on
        // `correspondence_for_pair`, not the law under test here).
        let (source, target, sky_mask) = zoned_pair();
        let fingerprint = |masks: &[crate::recipe::LocalAdjustment]| -> String {
            masks
                .iter()
                .map(|m| {
                    let mut m = m.clone();
                    m.mask = crate::recipe::MaskGeometry::Bitmap { path: String::new() };
                    serde_json::to_string(&m).unwrap()
                })
                .collect::<Vec<_>>()
                .join("|")
        };
        let run = |field: Option<crate::correspond::CorrespondenceField>| -> String {
            let mask_path = fixture_mask_path("corr-conserve");
            sky_mask.save(mask_path.path()).unwrap();
            let mut report = fit::fit_recipe(&source, &target);
            if let Some(f) = field {
                let s_img = source.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
                let t_img = target.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
                report.correspondence = Some(fit::correspondence_for_pair(
                    &f,
                    &fit::pixels_of(&t_img),
                    (s_img.width(), s_img.height()),
                    (t_img.width(), t_img.height()),
                ));
            }
            attach_zones(&source, &target, &mut report, &sky_mask, &sky_mask, &mask_path);
            mask_path.remove();
            assert!(
                !report.recipe.masks.is_empty(),
                "premise: zones attach on the calibration pair: {}",
                report.recipe.rationale
            );
            fingerprint(&report.recipe.masks)
        };
        let plain = run(None);
        assert_eq!(
            plain,
            run(Some(crate::correspond::identity_test_field())),
            "an identity field must change no zone verdict"
        );
        let mut zero = crate::correspond::identity_test_field();
        zero.confidence = vec![0.0; zero.confidence.len()];
        assert_eq!(
            plain,
            run(Some(zero)),
            "a zero-confidence field must abstain wholesale, never starve a zone"
        );
    }

    #[test]
    fn calibration_sky_zone_survives_luminance_with_partial_chroma_refusal() {
        let Some(root) = fit::calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
        let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
        // READ the corpus, never OWN it: `attach_zones` deletes the raster
        // it is handed when no zone survives, and this line used to hand it
        // the user's irreplaceable calibration mask. The scratch copy is the
        // convention every other test here already follows.
        let sky_mask = image::open(root.join("sky-mask.png"))
            .expect("calibration sky-mask.png")
            .to_luma8();
        let mask_path = fixture_mask_path("calibration-sky-scratch");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&source, &target);
        attach_zones(&source, &target, &mut report, &sky_mask, &sky_mask, &mask_path);
        let sky = report
            .recipe
            .masks
            .iter()
            .find(|mask| mask.role == MaskRole::ZoneSky)
            .expect("the calibration sky zone must survive");
        assert_eq!(sky.color_gains, Some([1.0; 3]));
        assert_eq!(sky.saturation, 0.0);
        let note = report
            .notes
            .iter()
            .find(|note| note.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED)
            .expect("calibration sky must reach the boundary gate");
        let after = note_number(note, "after");
        eprintln!(
            "CALIBRATION_SKY ev={:.3} gains={:?} sat={:.1} rim={:.4} rationale={}",
            sky.exposure_ev, sky.color_gains, sky.saturation, after, report.recipe.rationale
        );
        assert!(after <= ZONE_BOUNDARY_RIM_MAX);
        assert!(report.notes.iter().any(|note| note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR));
    }

    /// The zone stage's verdict is bounded by BOTH stages: it may not raise
    /// the global fit's own claim, and the global fit may not keep a claim
    /// the zones contradict.
    #[test]
    fn a_zoned_fits_confidence_comes_from_the_zones_it_accepted() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-confidence-mask");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        let global_conf = report.recipe.confidence;
        attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
        assert!(
            !report.recipe.masks.is_empty(),
            "premise: a zone attaches on this fixture: {}",
            report.recipe.rationale
        );
        assert!(
            report.recipe.confidence <= global_conf + 1e-6,
            "the zone stage must not raise the global claim ({} > {global_conf})",
            report.recipe.confidence
        );
        assert!(
            report.recipe.confidence >= 0.25,
            "…nor sink below the family floor: {}",
            report.recipe.confidence
        );
        mask_path.remove();
    }

    #[test]
    fn zoned_orchestration_corrects_the_land_through_the_inverted_raster() {
        // The first real-pair render's lesson: repainting ONLY the sky leaves
        // everything outside the mask with the global look — on the real
        // pair a blue haze band clashed against the new gold sky. The land
        // zone reuses the SAME raster inverted; when the target's land
        // differs too (muted vs vivid warm), the land zone must attach even
        // when the one-sided sky hue is withheld.
        let (w, h) = (16u32, 16u32);
        let build = |sky: [f32; 3], rock: [f32; 3]| -> DynamicImage {
            let img = RgbImage::from_fn(w, h, |_, y| {
                let p = if y >= 12 { sky } else { rock };
                image::Rgb(p.map(|c| (c * 255.0).round() as u8))
            });
            DynamicImage::ImageRgb8(img)
        };
        // Muted hazy land → bright vivid warm land (the real pair's demand).
        let src = build([0.60, 0.63, 0.67], [0.45, 0.42, 0.40]);
        let tgt = build([0.92, 0.72, 0.48], [0.80, 0.50, 0.28]);
        let sky_mask =
            GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]));
        let mask_path = fixture_mask_path("zoned-orch-land-mask");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
        let sky = report
            .recipe
            .masks
            .iter()
            .find(|m| m.role == MaskRole::ZoneSky)
            .unwrap_or_else(|| panic!("supported sky luminance correction was lost: {}", report.recipe.rationale));
        assert_eq!(sky.color_gains, Some([1.0; 3]));
        assert_eq!(sky.saturation, 0.0);
        let land = report
            .recipe
            .masks
            .iter()
            .find(|m| m.role == MaskRole::ZoneLand)
            .unwrap_or_else(|| panic!("land zone must attach: {}", report.recipe.rationale));
        assert!(land.inverted, "the land zone rides the INVERTED sky raster");
        assert!(
            report.recipe.rationale.contains("Zoned land correction attached"),
            "rationale must document the land zone: {}",
            report.recipe.rationale
        );
        // Render check: a land pixel must move toward the vivid warm target.
        let out = render::develop_preview(&src, &report.recipe).to_rgb8();
        let p = out.get_pixel(8, 4);
        let (r, b) = (p[0] as f32 / 255.0, p[2] as f32 / 255.0);
        assert!(r > b + 0.10, "land must turn warm (r >> b): {p:?}");
        mask_path.remove();
    }

    #[test]
    fn zoned_fit_survives_a_composition_share_mismatch() {
        // The real-pair failure geometry (2026-07-09): the generative target
        // holds ~3× more sky than the source, so the FRAME-global look_err
        // barely moves (or drifts up) when the zone is repainted correctly —
        // the first gate (frame-global improvement) dropped a correction
        // whose zone moments landed almost exactly on the target's (measured
        // zone residual 0.507 → 0.015, global drift +0.0024). The zone-local
        // gate must attach it; the rationale must surface the composition
        // difference honestly.
        let (w, h) = (16u32, 16u32);
        let build = |sky: [f32; 3], sky_rows: u32| -> DynamicImage {
            let img = RgbImage::from_fn(w, h, |_, y| {
                let p = if y >= h - sky_rows { sky } else { [0.55f32, 0.45, 0.35] };
                image::Rgb(p.map(|c| (c * 255.0).round() as u8))
            });
            DynamicImage::ImageRgb8(img)
        };
        let mask_of = |sky_rows: u32| {
            GrayImage::from_fn(w, h, |_, y| {
                image::Luma([if y >= h - sky_rows { 255u8 } else { 0 }])
            })
        };
        // Source: 2 sky rows (12.5%). Target: 6 gold rows (37.5%) — 3× more.
        let src = build([0.60, 0.63, 0.67], 2);
        let tgt = build([0.92, 0.72, 0.48], 6);
        let (sm, tm) = (mask_of(2), mask_of(6));
        let mask_path = fixture_mask_path("zoned-orch-share-mask");
        sm.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        attach_zones(&src, &tgt, &mut report, &sm, &tm, &mask_path);
        assert!(
            report.recipe.rationale.contains("compositions differ")
                && report
                    .notes
                    .iter()
                    .any(|note| note.key == crate::rationale::keys::ZONE_SHARE_NO_CORRECTION),
            "the structurally changed populations must disclose the bounded refusal: {}",
            report.recipe.rationale
        );
        assert!(
            report.recipe.masks.is_empty(),
            "neither composition-mismatched population carries two-sided evidence: {}",
            report.recipe.rationale
        );
        assert!(
            report.recipe.rationale.contains("compositions differ")
                || report.recipe.rationale.contains("share of the two frames differs"),
            "rationale must surface the share mismatch: {}",
            report.recipe.rationale
        );
        mask_path.remove();
    }

    #[test]
    fn zoned_orchestration_skips_a_degenerate_sky() {
        // An empty sky mask must skip BOTH zones: without a valid sky
        // partition, "land" would mean "everything" — a weaker-gated re-run
        // of the global fit, not a semantic zone.
        let (src, tgt, _) = zoned_pair();
        let empty = GrayImage::from_pixel(16, 16, image::Luma([0u8]));
        let mask_path = fixture_mask_path("zoned-orch-empty-mask");
        let mut report = fit::fit_recipe(&src, &tgt);
        attach_zones(&src, &tgt, &mut report, &empty, &empty, &mask_path);
        assert!(report.recipe.masks.is_empty(), "no mask on a degenerate partition");
        assert!(
            report.recipe.rationale.contains("no usable sky partition"),
            "rationale must say why: {}",
            report.recipe.rationale
        );
    }

    #[test]
    fn zoned_fit_degrades_gracefully_without_python() {
        // A missing/broken python must yield the plain global fit plus an
        // honest note — never an error (the graceful-fallback contract).
        let (src, tgt, _) = zoned_pair();
        let seg = SegmentOpts {
            python_bin: "autoshop-test-no-such-python".into(),
            // Must EXIST so the failure exercised is the launch, not the
            // script check.
            script: "Cargo.toml".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let mask_path = fixture_mask_path("zoned-orch-nopython-mask");
        let report = fit_recipe_zoned(&src, &tgt, &seg, &mask_path);
        assert!(report.recipe.masks.is_empty(), "fallback must not attach masks");
        assert!(
            report.recipe.rationale.contains("automatic luminance-range fallback"),
            "rationale must explain the fallback: {}",
            report.recipe.rationale
        );
        // The temporary segmentation inputs must not survive the fallback.
        for suffix in [".src-in.png", ".tgt-in.png", ".tgt-mask.png"] {
            let mut p = mask_path.path().as_os_str().to_owned();
            p.push(suffix);
            assert!(
                !std::path::Path::new(&p).exists(),
                "temp file {suffix} leaked past the fallback"
            );
        }
    }

    // Mutation guard: make `fit::append_finished_disclosure` an unconditional
    // early return. The required finished disclosure below then panics, so
    // deleting the disclosure cannot satisfy this test.
    #[test]
    fn unrepresented_note_is_derived_from_the_finished_zoned_render() {
        let (w, h) = (64u32, 64u32);
        let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
            let texture = (((x * 17) % 23) as f32 / 22.0 - 0.5) * 0.08;
            let v = (0.36 + 0.36 * x as f32 / (w - 1) as f32 + texture).clamp(0.08, 0.92);
            let p = [0.70 * v, 0.82 * v, v];
            image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        }));
        let sky_mask = GrayImage::from_fn(w, h, |_, y| {
            image::Luma([if y >= 40 { 255u8 } else { 0 }])
        });
        let path = fixture_mask_path("zoned-finished-disclosure");
        sky_mask.save(path.path()).unwrap();
        let truth = crate::recipe::EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
                role: MaskRole::ZoneSky,
                amount: 1.0,
                color_gains: Some([0.20, 0.65, 1.50]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let tgt = render::develop_preview(&src, &truth);
        let eager = fit::fit_recipe(&src, &tgt);
        let eager_note = eager
            .notes
            .iter()
            .find(|n| {
                n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
                    || n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
            })
            .unwrap_or_else(|| {
                panic!(
                    "premise: the pre-zone render leaves a measurable unrepresented colour residual: {:?} {:.3}->{:.3} {}",
                    eager.mode,
                    eager.err_before,
                    eager.err_after,
                    eager.recipe.rationale,
                )
            });
        let eager_controls = eager_note
            .args
            .iter()
            .find(|(key, _)| *key == "controls")
            .expect("controls arg")
            .1
            .clone();

        let mut report = fit::fit_recipe_from_promoted_with_disclosure(
            &src,
            &tgt,
            &crate::recipe::EditRecipe::default(),
            false,
            true,
            None,
        );
        assert!(!report
            .notes
            .iter()
            .any(|n| {
                n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
                    || n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
            }));
        attach_zones_with_divergence(
            &src,
            &tgt,
            &mut report,
            &sky_mask,
            &sky_mask,
            &path,
            ZoneDivergences {
                sky: ZoneDivergence { divergence: divergence(0.80), share: 0.375 },
                land: ZoneDivergence { divergence: divergence(0.0), share: 0.625 },
            },
        );
        assert!(
            !report.recipe.masks.is_empty(),
            "fixture must deliver a zoned render: {}",
            report.recipe.rationale,
        );
        let finished_controls = report
            .notes
            .iter()
            .find(|n| {
                n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
                    || n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
            })
            .and_then(|n| n.args.iter().find(|(key, _)| *key == "controls"))
            .map(|(_, value)| value.as_str());
        let finished_controls = finished_controls.unwrap_or_else(|| {
            panic!("the finished zoned render must carry its disclosure: {}", report.recipe.rationale)
        });
        assert_ne!(
            finished_controls,
            eager_controls.as_str(),
            "the disclosure was copied from the pre-zone render"
        );
        path.remove();
    }

    fn synthetic_range_case(residuals: [f32; 17]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, fit::EvidenceModel) {
        const PER_BIN: usize = 8;
        let mut base = Vec::with_capacity(17 * PER_BIN);
        let mut source = Vec::with_capacity(17 * PER_BIN);
        let mut target = Vec::with_capacity(17 * PER_BIN);
        for (bin, residual) in residuals.into_iter().enumerate() {
            let v = (bin as f32 + 0.5) / 17.0;
            for _ in 0..PER_BIN {
                base.push([v; 3]);
                source.push([(v - residual).clamp(0.0, 1.0); 3]);
                target.push([v; 3]);
            }
        }
        let share = 1.0 / 17.0;
        let luma = (0..17)
            .map(|bin| fit::EvidenceRange {
                label: format!("luma-{bin:02}"),
                source_share: share,
                target_share: share,
                source_evidence_share: share,
                target_evidence_share: share,
                two_sided_share: share,
                divergence: 0.1,
                weight: share,
                source_populated: true,
                target_populated: true,
            })
            .collect();
        let n = source.len();
        let evidence = fit::EvidenceModel {
            source_pixels: base,
            width: n as u32,
            height: 1,
            spatial_supported: vec![true; n],
            source_weights: vec![1.0; n],
            target_weights: vec![1.0; n],
            source_hue_weights: vec![0.0; n],
            target_hue_weights: vec![0.0; n],
            luma,
            hue: Vec::new(),
            identifiability: 1.0,
        };
        (source, target, evidence)
    }

    fn luminance_bounds(range: RangeMask) -> (f32, f32, f32, f32) {
        match range {
            RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
                (lo_outer, lo, hi, hi_outer)
            }
            RangeMask::Color { .. } => panic!("range derivation emitted a colour range"),
        }
    }

    #[test]
    fn range_band_derivation_follows_signed_residual_runs_and_caps_at_four() {
        let mut residuals = [0.0; 17];
        residuals[1] = 0.04;
        residuals[2] = 0.05;
        residuals[8] = 0.04;
        residuals[9] = 0.10;
        residuals[10] = 0.12;
        residuals[12] = -0.105;
        residuals[13] = -0.04;
        let (source, target, evidence) = synthetic_range_case(residuals);
        let derived = derive_luminance_bands(&source, &target, &evidence);
        let cores = derived
            .bands
            .iter()
            .map(|band| {
                let (_, lo, hi, _) = luminance_bounds(band.source);
                (lo, hi)
            })
            .collect::<Vec<_>>();
        assert_eq!(cores.len(), 3, "coherent neutral gaps must remain unmasked: {cores:?}");
        assert!((cores[0].0 - 1.0 / 17.0).abs() < 1e-6);
        assert!((cores[0].1 - 3.0 / 17.0).abs() < 1e-6);
        assert!((cores[1].0 - 8.0 / 17.0).abs() < 1e-6);
        assert!((cores[2].0 - 12.0 / 17.0).abs() < 1e-6);

        let mut five = [0.0; 17];
        for (bin, residual) in [(1, 0.07), (3, -0.07), (5, 0.07), (7, -0.07), (9, 0.07)] {
            five[bin] = residual;
        }
        let (source, target, evidence) = synthetic_range_case(five);
        let capped = derive_luminance_bands(&source, &target, &evidence);
        assert_eq!(capped.bands.len(), RANGE_MAX_BANDS);
        assert!(capped.merges.is_empty(), "a merge may not cross the retained opposite-sign core");
        assert!(capped.abstentions.iter().any(|a| a.reason.contains("no adjacent same-sign")));
        let cores = capped
            .bands
            .iter()
            .map(|band| {
                let (_, lo, hi, _) = luminance_bounds(band.source);
                (lo, hi)
            })
            .collect::<Vec<_>>();
        assert!(cores.windows(2).all(|pair| pair[0].1 < pair[1].0));

        let mut mergeable = [0.0; 17];
        for (bin, residual) in [(1, 0.061), (3, 0.07), (5, -0.10), (7, 0.10), (9, -0.10)] {
            mergeable[bin] = residual;
        }
        let (source, target, evidence) = synthetic_range_case(mergeable);
        let merged = derive_luminance_bands(&source, &target, &evidence);
        assert_eq!(merged.bands.len(), RANGE_MAX_BANDS);
        assert_eq!(merged.merges.len(), 1, "an adjacent same-sign run remains mergeable");
        let cores = merged
            .bands
            .iter()
            .map(|band| {
                let (_, lo, hi, _) = luminance_bounds(band.source);
                (lo, hi)
            })
            .collect::<Vec<_>>();
        assert!(cores.windows(2).all(|pair| pair[0].1 < pair[1].0));
    }

    #[test]
    fn range_band_derivation_is_invariant_to_target_pixel_positions() {
        let mut residuals = [0.0; 17];
        residuals[2] = 0.07;
        residuals[8] = -0.08;
        residuals[13] = 0.09;
        let (source, target, evidence) = synthetic_range_case(residuals);
        let expected = derive_luminance_bands(&source, &target, &evidence);
        let mut shuffled = target;
        shuffled.rotate_left(37);
        shuffled.reverse();
        let actual = derive_luminance_bands(&source, &shuffled, &evidence);
        assert_eq!(actual, expected, "target positions must not influence rank-derived bands");
    }

    #[test]
    fn range_band_abstains_with_one_sided_or_zero_structural_evidence() {
        let mut residuals = [0.0; 17];
        residuals[1] = 0.07;
        let (source, target, mut evidence) = synthetic_range_case(residuals);
        evidence.luma[1].target_populated = false;
        evidence.luma[1].target_share = 0.0;
        evidence.luma[1].target_evidence_share = 0.0;
        evidence.luma[1].two_sided_share = 0.0;
        evidence.luma[1].weight = 0.0;
        let one_sided = derive_luminance_bands(&source, &target, &evidence);
        assert!(one_sided.bands.is_empty());
        assert_eq!(one_sided.abstentions.len(), 1);
        assert!(one_sided.abstentions[0].reason.contains("target population"));

        evidence.luma[1].target_populated = true;
        evidence.luma[1].target_evidence_share = 1.0 / 17.0;
        evidence.luma[1].two_sided_share = 1.0 / 17.0;
        evidence.luma[1].weight = 1.0 / 17.0;
        let population_only = derive_luminance_bands(&source, &target, &evidence);
        assert!(population_only.bands.is_empty());
        assert_eq!(population_only.abstentions.len(), 1);
        assert!(population_only.abstentions[0].reason.contains("target population"));

        evidence.luma[1].target_share = 1.0 / 17.0;
        evidence.luma[1].weight = 0.0;
        let structural = derive_luminance_bands(&source, &target, &evidence);
        assert!(structural.bands.is_empty());
        assert!(structural.abstentions[0].reason.contains("zero structural evidence"));

        let mut adjacent = [0.0; 17];
        adjacent[1] = 0.07;
        adjacent[2] = 0.07;
        let (source, target, mut evidence) = synthetic_range_case(adjacent);
        evidence.luma[2].weight = 0.0;
        let no_hitchhike = derive_luminance_bands(&source, &target, &evidence);
        assert_eq!(no_hitchhike.bands.len(), 1);
        let (_, lo, hi, _) = luminance_bounds(no_hitchhike.bands[0].source);
        assert!((lo - 1.0 / 17.0).abs() < 1e-6 && (hi - 2.0 / 17.0).abs() < 1e-6);
        assert!(no_hitchhike.abstentions.iter().any(|a| {
            (a.lo - 2.0 / 17.0).abs() < 1e-6 && a.reason.contains("zero structural evidence")
        }));

        // The SOURCE population arm is its own gate: a bin can hold a few
        // actual pixels (bin_count > 0) while sitting under the 1.5% evidence
        // floor, and only this arm refuses it.
        let mut src_gap = [0.0; 17];
        src_gap[1] = 0.07;
        let (source, target, mut evidence) = synthetic_range_case(src_gap);
        evidence.luma[1].source_populated = false;
        evidence.luma[1].source_share = 0.0;
        let source_side = derive_luminance_bands(&source, &target, &evidence);
        assert!(source_side.bands.is_empty());
        assert!(source_side.abstentions[0].reason.contains("source population"));
    }

    #[test]
    fn range_band_ramps_are_ordered_and_partition_weights_do_not_exceed_one() {
        let mut residuals = [0.0; 17];
        residuals[5] = 0.07;
        residuals[6] = -0.07;
        let (source, target, evidence) = synthetic_range_case(residuals);
        let derived = derive_luminance_bands(&source, &target, &evidence);
        assert_eq!(derived.bands.len(), 2);
        for band in &derived.bands {
            let (lo_outer, lo, hi, hi_outer) = luminance_bounds(band.source);
            assert!(lo_outer <= lo && lo <= hi && hi <= hi_outer);
            assert!(lo - lo_outer > 0.0 || lo == 0.0, "interior lower ramp is hard");
            assert!(hi_outer - hi > 0.0 || hi == 1.0, "interior upper ramp is hard");
            assert!(lo - lo_outer <= RANGE_MAX_RAMP + 1e-6);
            assert!(hi_outer - hi <= RANGE_MAX_RAMP + 1e-6);
        }
        for i in 0..source.len() {
            let sum = derived
                .bands
                .iter()
                .map(|band| band.attachment.source_weights[i])
                .sum::<f32>();
            assert!(sum <= 1.0 + 1e-6, "source partition overlaps at {i}: {sum}");
        }
        let mut target_weights = derived
            .bands
            .iter()
            .map(|band| range_weights_for_pixels(&band.target, &target))
            .collect::<Vec<_>>();
        normalize_partition_weights(&mut target_weights);
        for i in 0..target.len() {
            let sum = target_weights.iter().map(|band| band[i]).sum::<f32>();
            assert!(sum <= 1.0 + 1e-6, "target partition overlaps at {i}: {sum}");
        }
    }

    fn range_boundary_fixture() -> (DynamicImage, Vec<[f32; 3]>, Vec<RangeMask>) {
        let (w, h) = (512u32, 8u32);
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(w, h, |x, _| {
            let v = (x as f32 / (w - 1) as f32 * 255.0).round() as u8;
            image::Rgb([v; 3])
        }));
        let ranges = vec![
            RangeMask::Luminance {
                lo_outer: 0.0,
                lo: 0.45,
                hi: 0.50,
                hi_outer: 0.55,
            },
            RangeMask::Luminance {
                lo_outer: 0.45,
                lo: 0.50,
                hi: 0.55,
                hi_outer: 0.60,
            },
        ];
        let reference = fit::pixels_of(&render::develop_preview(
            &source,
            &crate::recipe::EditRecipe::default(),
        ));
        (source, reference, ranges)
    }

    fn opposing_range_recipe(ranges: &[RangeMask]) -> crate::recipe::EditRecipe {
        crate::recipe::EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: RANGE_HOST,
                    range: Some(ranges[0]),
                    exposure_ev: 0.5,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: RANGE_HOST,
                    range: Some(ranges[1]),
                    exposure_ev: -0.5,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    fn rank_wave_target(source: &DynamicImage) -> DynamicImage {
        let (w, h) = (source.width(), source.height());
        DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(w, h, |x, _| {
            let v = x as f32 / (w - 1) as f32;
            let target = v + 0.15 * (std::f32::consts::TAU * v).sin();
            let value = (target.clamp(0.0, 1.0) * 255.0).round() as u8;
            image::Rgb([value; 3])
        }))
    }

    fn compact_rank_wave_fixture() -> (DynamicImage, DynamicImage) {
        let image = |target: bool| {
            DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(512, 16, |x, _| {
                let v = (x % 128) as f32 / 127.0;
                let value = if target {
                    v + 0.15 * (std::f32::consts::TAU * v).sin()
                } else {
                    v
                };
                let value = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                image::Rgb([value; 3])
            }))
        };
        (image(false), image(true))
    }

    #[test]
    fn range_boundary_rim_rejects_the_measured_opposite_half_ev_stress() {
        let (source, reference, ranges) = range_boundary_fixture();
        let rendered = fit::pixels_of(&render::develop_preview(
            &source,
            &opposing_range_recipe(&ranges),
        ));
        let reading = range_transition_rim(
            &reference,
            &rendered,
            &ranges,
            source.width(),
            source.height(),
        );
        assert!(
            reading.rim > RANGE_BOUNDARY_RIM_MAX && reading.rim < 0.05,
            "the retained 5/255-class stress must cross 0.012 but not 0.05: {reading:?}"
        );
    }

    #[test]
    fn range_boundary_gate_helper_shrinks_differentials_and_discloses_k() {
        let (source, reference, ranges) = range_boundary_fixture();
        let mut report = neutral_report(&source, &source);
        report.recipe = opposing_range_recipe(&ranges);
        let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        let weights = ranges
            .iter()
            .map(|range| range_weights_for_pixels(range, &reference))
            .collect::<Vec<_>>();
        let shares = weights
            .iter()
            .map(|band| band.iter().sum::<f32>() / band.len() as f32)
            .collect::<Vec<_>>();
        let verdict = enforce_range_boundary_gate(
            &source,
            &mut report,
            &reference,
            &ranges,
            &shares,
            0,
            initial,
        );
        let BoundaryGateResult::Kept { k, before, after, .. } = verdict else {
            panic!("zero differential must give the bisection a passing endpoint")
        };
        assert!(before.rim > RANGE_BOUNDARY_RIM_MAX);
        assert!(after.rim <= RANGE_BOUNDARY_RIM_MAX);
        assert!((0.0..1.0).contains(&k), "stress must require a real shrink: {k}");
        assert!(report.notes.iter().any(|n| {
            n.key == crate::rationale::keys::RANGE_BOUNDARY_PASSED
                && n.args.iter().any(|(key, value)| *key == "k" && value != "1.000")
        }));
    }

    #[test]
    fn range_boundary_gate_shrinks_differentials_and_discloses_k() {
        let (source, target) = compact_rank_wave_fixture();
        let mut report = neutral_report(&source, &target);
        let global_err = report.err_after;
        attach_luminance_ranges(&source, &target, &mut report);
        let note = report
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::RANGE_BOUNDARY_PASSED
                    && note.args.iter().any(|(key, value)| *key == "k" && value != "1.000")
            })
            .unwrap_or_else(|| {
                panic!("the fallback orchestration did not call the shrinking boundary gate: {:?}", report.notes)
            });
        assert!(note.args.iter().any(|(key, _)| *key == "before"));
        assert!(
            report.err_after <= global_err + RANGE_FRAME_REGRESSION_TOL,
            "the post-shrink stack escaped the global-only frame ceiling"
        );
    }

    #[test]
    fn range_final_frame_ceiling_refuses_a_post_shrink_regression() {
        let (source, target) = compact_rank_wave_fixture();
        let mut report = neutral_report(&source, &target);
        let global_frame_err = report.err_after;
        RANGE_FINAL_FRAME_ERR_OVERRIDE.with(|value| value.set(Some(global_frame_err + 0.001)));
        attach_luminance_ranges(&source, &target, &mut report);
        assert!(report.recipe.masks.is_empty(), "the complete range stack must be reverted");
        assert_eq!(report.err_after, global_frame_err);
        assert!(report.notes.iter().any(|note| {
            note.key == crate::rationale::keys::RANGE_FRAME_REFUSED
                && note.args.iter().any(|(key, value)| *key == "tol" && value == "+0.000")
        }));
    }

    #[test]
    fn range_conservation_all_frame_band_reproduces_global_fit() {
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(64, 8, |x, _| {
            let v = (32 + x * 3).min(255) as u8;
            image::Rgb([v, v.saturating_add(3), v.saturating_add(7)])
        }));
        let mut without_range = crate::recipe::EditRecipe::default();
        without_range.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            exposure_ev: 0.35,
            ..Default::default()
        });
        let mut with_range = without_range.clone();
        with_range.masks[0].range = Some(RangeMask::Luminance {
            lo_outer: 0.0,
            lo: 0.0,
            hi: 1.0,
            hi_outer: 1.0,
        });
        assert_eq!(
            render::develop_preview(&source, &without_range).to_rgb8(),
            render::develop_preview(&source, &with_range).to_rgb8(),
            "the sentinel plus an all-frame luminance range must conserve every pixel"
        );
    }

    #[test]
    fn range_xmp_round_trip_preserves_host_and_luminance_range() {
        let range = RangeMask::Luminance {
            lo_outer: 0.10,
            lo: 0.20,
            hi: 0.70,
            hi_outer: 0.80,
        };
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(range),
            name: "Luminance range 01".to_string(),
            exposure_ev: 0.25,
            role: MaskRole::Custom,
            ..Default::default()
        });
        let xmp = crate::xmp::recipe_to_xmp(&recipe);
        assert_eq!(xmp.matches("Mask/RangeMask").count(), 1);
        assert!(!xmp.contains("Mask/Bitmap"));
        let round_trip = crate::xmp::xmp_to_recipe(&xmp);
        assert_eq!(round_trip.masks.len(), 1);
        assert_eq!(round_trip.masks[0].mask, RANGE_HOST);
        assert_eq!(round_trip.masks[0].range, Some(range));
    }

    #[test]
    fn range_serde_old_recipe_reads_and_new_range_recipe_is_explicitly_scoped() {
        let old = r#"{"version":2,"masks":[{"name":"legacy"}]}"#;
        let read: crate::recipe::EditRecipe = serde_json::from_str(old).expect("old recipe");
        assert_eq!(read.masks[0].range, None);
        assert_eq!(read.masks[0].role, MaskRole::Custom);

        let mut new = crate::recipe::EditRecipe::default();
        new.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(RangeMask::Luminance {
                lo_outer: 0.1,
                lo: 0.2,
                hi: 0.7,
                hi_outer: 0.8,
            }),
            role: MaskRole::Custom,
            ..Default::default()
        });
        let value = serde_json::to_value(&new).unwrap();
        assert_eq!(value["schema_era"], 1);
        assert_eq!(value["masks"][0]["role"], "custom");
        assert_eq!(value["masks"][0]["range"]["kind"], "luminance");
        serde_json::from_value::<crate::recipe::EditRecipe>(value).expect("explicit range scope");
    }

    #[test]
    fn range_abstention_preserves_global_recipe_byte_for_byte() {
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(96, 12, |x, _| {
            let v = (x * 255 / 95) as u8;
            image::Rgb([v; 3])
        }));
        let global = fit::fit_recipe(&source, &source);
        let mut deferred = fit::fit_recipe_from_promoted_with_disclosure(
            &source,
            &source,
            &crate::recipe::EditRecipe::default(),
            false,
            true,
            None,
        );
        attach_luminance_ranges(&source, &source, &mut deferred);
        assert_eq!(
            serde_json::to_vec(&deferred.recipe).unwrap(),
            serde_json::to_vec(&global.recipe).unwrap(),
            "an entirely abstaining range pass must be indistinguishable from global-only"
        );
        assert_eq!(deferred.err_after, global.err_after);
    }

    #[test]
    fn range_band_composed_frame_regression_is_dropped_while_neutral_or_better_is_kept() {
        let (w, h) = (64u32, 64u32);
        let build = |target: bool| DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = 0.30 + 0.30 * x as f32 / (w - 1) as f32;
            let p = if y < h / 2 {
                if target { [v * 1.5, v * 1.5, v * 1.5] } else { [v * 0.72, v * 0.86, v] }
            } else { [0.45, 0.38, 0.28] };
            image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        }));
        let source = build(false);
        let target = build(true);
        let mask =
            GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
        let path = fixture_mask_path("range-frame-regression");
        mask.save(path.path()).unwrap();
        let s_img = source.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let t_img = target.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let tgt_px = fit::pixels_of(&t_img);
        let divergence = measure_zone_divergence(
            &source,
            &target,
            &crate::recipe::EditRecipe::default(),
            &mask,
        )
        .sky
        .divergence;
        let attachment = ZoneAttachment {
            source_weights: mask_weights(&mask, s_img.width(), s_img.height()),
            target_weights: mask_weights(&mask, t_img.width(), t_img.height()),
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            range: Some(RangeMask::Luminance {
                lo_outer: 0.0,
                lo: 0.0,
                hi: 1.0,
                hi_outer: 1.0,
            }),
            name: "Luminance range 01".to_string(),
            role: MaskRole::Custom,
            inverted: false,
            label: "Luminance range 01".to_string(),
            frame_regression_tol: RANGE_FRAME_REGRESSION_TOL,
        };

        // Obtain the deterministic candidate frame without making its
        // composed-frame arm binding; both assertions replay this attachment
        // against either side of the zero-tolerance line.
        let mut probe = neutral_report(&source, &target);
        let mut loose_frame_err = f32::MAX;
        let candidate = attach_one_zone(
            &s_img,
            &tgt_px,
            &mut probe,
            &mut loose_frame_err,
            &attachment,
            divergence,
            None,
        )
        .expect("the fixture must earn its band on local evidence");
        let candidate_err =
            fit::look_err_with_evidence(&candidate.rendered, &tgt_px, &probe.evidence);

        let mut regressing = neutral_report(&source, &target);
        let mut better_running_frame = candidate_err - 0.001;
        let dropped = attach_one_zone(
            &s_img,
            &tgt_px,
            &mut regressing,
            &mut better_running_frame,
            &attachment,
            divergence,
            None,
        );
        assert!(dropped.is_none(), "a +0.001 composed-frame regression must drop the band");
        assert!(regressing.recipe.masks.is_empty(), "the dropped band must be removed");
        let note = regressing
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::ZONE_DROPPED
                    || note.key == crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED
            })
            .expect("the dropped band must disclose its measured frame drift");
        assert!(note.args.iter().any(|(key, value)| *key == "drift" && value == "+0.001"));
        assert!(note.args.iter().any(|(key, value)| *key == "tol" && value == "+0.000"));

        let mut nonregressing = neutral_report(&source, &target);
        let mut no_worse_running_frame = candidate_err;
        let kept = attach_one_zone(
            &s_img,
            &tgt_px,
            &mut nonregressing,
            &mut no_worse_running_frame,
            &attachment,
            divergence,
            None,
        );
        assert!(kept.is_some(), "a neutral-or-better composed frame must keep the band");
        assert_eq!(nonregressing.recipe.masks.len(), 1);
        path.remove();
    }

    #[test]
    fn segmentation_success_does_not_derive_range_bands() {
        RANGE_DERIVATION_CALLS.with(|calls| calls.set(0));
        let (source, target, sky) = zoned_pair();
        let path = fixture_mask_path("range-no-work-segmentation-success");
        sky.save(path.path()).unwrap();
        SEGMENT_BOTH_OVERRIDE.with(|value| *value.borrow_mut() = Some((sky.clone(), sky)));
        let seg = SegmentOpts {
            python_bin: "unused-by-segmentation-test-override".into(),
            script: "unused-by-segmentation-test-override".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let report = fit_recipe_zoned_inner(
            &source,
            &target,
            &seg,
            &path,
            &crate::recipe::EditRecipe::default(),
            None,
        );
        let calls = RANGE_DERIVATION_CALLS.with(std::cell::Cell::get);
        assert_eq!(calls, 0, "semantic success must not even derive range candidates");
        assert!(report.recipe.masks.iter().any(|mask| mask.range.is_none()));
        path.remove();
    }

    #[test]
    fn range_bands_compose_in_current_render_order() {
        RANGE_FRESH_RENDER_CALLS.with(|calls| calls.set(0));
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            32,
            8,
            image::Rgb([118, 118, 118]),
        ));
        let first = RangeMask::Luminance {
            lo_outer: 0.35,
            lo: 0.40,
            hi: 0.50,
            hi_outer: 0.55,
        };
        let second = RangeMask::Luminance {
            lo_outer: 0.50,
            lo: 0.55,
            hi: 0.70,
            hi_outer: 0.75,
        };
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(first),
            exposure_ev: 0.5,
            ..Default::default()
        });
        recipe.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(second),
            exposure_ev: -0.5,
            ..Default::default()
        });
        let first_only = {
            let mut r = recipe.clone();
            r.masks.pop();
            render::develop_preview(&source, &r).to_rgb8()
        };
        let composed = render::develop_preview(&source, &recipe).to_rgb8();
        assert_ne!(
            composed, first_only,
            "the first band must move pixels into the later band's current-render range"
        );
        let current = fit::pixels_of(&DynamicImage::ImageRgb8(first_only));
        assert!(
            range_weights_for_pixels(&second, &current).iter().any(|&weight| weight > 0.0),
            "later estimator weights must be derivable from the current render"
        );

        let (fixture, _, _ranges) = range_boundary_fixture();
        let target = rank_wave_target(&fixture);
        let mut report = neutral_report(&fixture, &target);
        attach_luminance_ranges(&fixture, &target, &mut report);
        assert!(
            RANGE_FRESH_RENDER_CALLS.with(std::cell::Cell::get) >= 2,
            "the fallback loop must freshly render each candidate's current stack"
        );
    }

    #[test]
    fn refit_with_existing_custom_range_never_dispatches_on_mask_role() {
        let (source, _, _) = range_boundary_fixture();
        let target = rank_wave_target(&source);
        let existing = LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(RangeMask::Luminance {
                lo_outer: 0.05,
                lo: 0.10,
                hi: 0.25,
                hi_outer: 0.30,
            }),
            name: "Existing custom range".to_string(),
            exposure_ev: 0.0,
            role: MaskRole::Custom,
            ..Default::default()
        };
        let base = crate::recipe::EditRecipe {
            masks: vec![existing.clone()],
            ..Default::default()
        };
        let path = fixture_mask_path("custom-range-refit-role-dispatch");
        let seg = SegmentOpts {
            python_bin: "missing-custom-range-python".into(),
            script: "missing-custom-range-script".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let report = fit_recipe_zoned_from(&source, &target, &seg, &path, &base);
        assert_eq!(report.recipe.masks.first(), Some(&existing));
        assert!(report.notes.iter().any(|note| note.key == crate::rationale::keys::RANGE_ATTACHED));
        path.remove();
    }
}
