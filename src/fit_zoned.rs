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
use image::{DynamicImage, GenericImageView, GrayImage};

use crate::fit::{self, FitReport};
use crate::recipe::{LocalAdjustment, MaskGeometry, MaskRole, RangeMask};
use crate::render;
use crate::segment::{segment_file, SegmentOpts};

mod field;
mod freemask;
mod range;
mod spatial;
pub mod semantic;

const MASK_REFINE_RADIUS: u32 = 8;
const MASK_REFINE_EPSILON: f32 = (4.0 / 255.0) * (4.0 / 255.0);

#[derive(Clone, Copy)]
struct ZonedLayerOpts {
    field: bool,
    spatial: bool,
    free_masks: bool,
    refine_masks: bool,
}

const SHIPPED_LAYERS: ZonedLayerOpts = ZonedLayerOpts {
    field: true,
    spatial: true,
    free_masks: true,
    refine_masks: true,
};

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
pub(super) const ZONE_BOUNDARY_RIM_MAX: f32 = 0.012;
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
/// Paired-sample stand-off for [`boundary_step`], in analysis pixels: each
/// sample of a pair sits `ZONE_STEP_OFFSET - 0.5` px from the 50% contour.
/// Two steps clear of the one-pixel ramp `render::sample_gray_norm` leaves on
/// a resampled 0/255 raster edge, while still reading each side's own plateau
/// rather than its neighbourhood.
const ZONE_STEP_OFFSET: usize = 2;

#[derive(Clone, Copy, Debug)]
pub(super) struct BoundaryReading {
    pub(super) rim: f32,
    pub(super) transitions: usize,
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

/// Add one scan line's cross-boundary steps to `out`.
///
/// Where [`boundary_line_rims`] reads INSIDE a feathered transition band,
/// this reads ACROSS the mask's 50% contour, so it has something to say about
/// a hard 0/255 raster — the shape `spatial::tile_mask` and the free-mask
/// producer write, whose transition band is empty by construction. That is
/// why the rim ruler returned `rim 0.0` from `0` transitions for every
/// spatial tile ever measured, and why a rectangular seam passed a gate that
/// was reporting a budget it had never been able to test.
///
/// A crossing is a neighbouring pair straddling [`ZONE_BOUNDARY_MID`]. Each
/// contributes ONE difference in differences:
///
/// ```text
///     (inside - outside) on `rendered` - (inside - outside) on `reference`
/// ```
///
/// `reference` is this same frame rendered WITHOUT the correction under test.
/// A luma step the subject already had at that border — a roof line the mask
/// follows, a horizon a tile edge grazes — appears in both terms and cancels,
/// so scene content cannot false-positive. What survives is only the
/// discontinuity the correction introduced, which is the seam itself.
///
/// "Inside" is the `>= mid` side, decided by the mask and never by the
/// direction of the scan, so the left and right edges of one brightened tile
/// report the SAME sign instead of cancelling. Both samples must still be on
/// their own side of the contour, which drops a pair straddling a sliver
/// thinner than the stand-off rather than reading a plateau that is not there.
fn boundary_line_steps(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    geometry: &[f32],
    start: usize,
    step: usize,
    len: usize,
    out: &mut Vec<f32>,
) {
    let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let index = |p: usize| -> Option<usize> {
        let i = start + p * step;
        (i < geometry.len() && i < rendered.len() && i < reference.len()).then_some(i)
    };
    let stand_off = ZONE_STEP_OFFSET.saturating_sub(1);
    for p in 1..len {
        let (Some(low), Some(high)) = (index(p - 1), index(p)) else {
            continue;
        };
        let inside = |i: usize| geometry[i] >= ZONE_BOUNDARY_MID;
        if inside(low) == inside(high) {
            continue;
        }
        // Walk each foot away from the contour, `stand_off` px in its own
        // direction, then require it to have stayed on its own side.
        let (near_in, near_out) = if inside(high) { (p, p - 1) } else { (p - 1, p) };
        let forward = near_in > near_out;
        let (Some(far_in), Some(far_out)) = (
            if forward { near_in.checked_add(stand_off) } else { near_in.checked_sub(stand_off) },
            if forward { near_out.checked_sub(stand_off) } else { near_out.checked_add(stand_off) },
        ) else {
            continue;
        };
        if far_in >= len || far_out >= len {
            continue;
        }
        let (Some(i_in), Some(i_out)) = (index(far_in), index(far_out)) else {
            continue;
        };
        if !inside(i_in) || inside(i_out) {
            continue;
        }
        let rendered_step = luma(&rendered[i_in]) - luma(&rendered[i_out]);
        let reference_step = luma(&reference[i_in]) - luma(&reference[i_out]);
        out.push(rendered_step - reference_step);
    }
}

/// Cross-boundary step reading — [`boundary_rim`]'s counterpart for masks
/// with no transition band to read into.
///
/// `geometry` is the mask's OWN alpha at analysis size, i.e. what the
/// renderer applies, and never the estimator weights: those are that alpha
/// times the zone's per-bin evidence verdicts, so their 50% contour is
/// punched full of interior holes that are not boundaries at all.
///
/// The result is a MAGNITUDE, ranked exactly as `range::range_transition_rim`
/// already ranks its own signed samples: a correction that darkens its side
/// of a border is as visible a seam as one that brightens it, and a signed
/// percentile would let a tile's dark edge hide behind its bright one.
fn boundary_step(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    geometry: &[f32],
    width: u32,
    height: u32,
) -> BoundaryReading {
    let (w, h) = (width as usize, height as usize);
    let mut steps = Vec::new();
    for y in 0..h {
        boundary_line_steps(reference, rendered, geometry, y * w, 1, w, &mut steps);
    }
    for x in 0..w {
        boundary_line_steps(reference, rendered, geometry, x, w, h, &mut steps);
    }
    if steps.is_empty() {
        return BoundaryReading { rim: 0.0, transitions: 0 };
    }
    steps.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
    let rank = ((steps.len() as f32 * ZONE_BOUNDARY_PERCENTILE).ceil() as usize)
        .saturating_sub(1)
        .min(steps.len() - 1);
    BoundaryReading { rim: steps[rank].abs(), transitions: steps.len() }
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
// `autoshade match`, and they provided independent review evidence. The table is
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

/// The comparison path (R23-6 E-15): `AUTOSHADE_FIT_JOINT=off` takes the
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
            crate::config::live_env("AUTOSHADE_FIT_JOINT").as_deref().map(str::trim),
            Some("off") | Some("0") | Some("false")
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
    options: fit::FitOptions<'_>,
) -> FitReport {
    fit_recipe_zoned_with_regions(
        src,
        target,
        seg,
        mask_path,
        base,
        options,
        semantic::DEFAULT_SEMANTIC_REGIONS,
    )
}

/// Multi-region entry point shared by the CLI and GUI.  `2` intentionally
/// routes through the historical sky/land implementation so its recipe bytes
/// and rationale remain unchanged. `options` carries the step-11 strength
/// budget and the step-7b provider into BOTH routes.
pub fn fit_recipe_zoned_with_regions(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    regions: usize,
) -> FitReport {
    if regions <= semantic::DEFAULT_SEMANTIC_REGIONS {
        return fit_recipe_zoned_inner_with_options(src, target, seg, mask_path, base, options, SHIPPED_LAYERS);
    }
    fit_recipe_zoned_multi_inner(
        src, target, seg, mask_path, base, options,
        regions.min(semantic::MAX_SEMANTIC_REGIONS),
    )
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
    fit_recipe_zoned_inner(src, target, seg, mask_path, base, None, SHIPPED_LAYERS)
}

fn fit_recipe_zoned_inner(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    provider: Option<fit::CorrespondenceProvider>,
    layers: ZonedLayerOpts,
) -> FitReport {
    fit_recipe_zoned_inner_with_options(src, target, seg, mask_path, base, fit::FitOptions { strength: crate::recipe::GradeStrength::default(), provider }, layers)
}

fn fit_recipe_zoned_inner_with_options(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    layers: ZonedLayerOpts,
) -> FitReport {
    fit_recipe_zoned_inner_seeded(
        src, target, seg, mask_path, base, options, layers, None,
    )
}

#[allow(clippy::too_many_arguments)]
fn fit_recipe_zoned_inner_seeded(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    layers: ZonedLayerOpts,
    segmented: Option<(GrayImage, GrayImage)>,
) -> FitReport {
    let segmented = match segmented {
        Some(pair) => Ok(pair),
        None => segment_both(src, target, seg, mask_path),
    };
    let (mut report, field, first_producer) = match segmented {
        Ok((mut src_mask, mut tgt_mask)) => {
            let refinements = if layers.refine_masks {
                let source = crate::mask_refine::guided_refine(
                    src,
                    &src_mask,
                    MASK_REFINE_RADIUS,
                    MASK_REFINE_EPSILON,
                );
                let target = crate::mask_refine::guided_refine(
                    target,
                    &tgt_mask,
                    MASK_REFINE_RADIUS,
                    MASK_REFINE_EPSILON,
                );
                Some((source, target))
            } else {
                None
            };
            if let Some((source_refinement, target_refinement)) = refinements {
                let mut readings = Vec::new();
                match source_refinement {
                    crate::mask_refine::RefineOutcome::Kept { mask, reading } => {
                        if mask.save(mask_path.path()).is_ok() {
                            src_mask = mask;
                            readings.push(("semantic source", true, reading));
                        } else {
                            readings.push(("semantic source", false, reading));
                        }
                    }
                    crate::mask_refine::RefineOutcome::Abstained { reading } => {
                        readings.push(("semantic source", false, reading));
                    }
                }
                match target_refinement {
                    crate::mask_refine::RefineOutcome::Kept { mask, reading } => {
                        tgt_mask = mask;
                        readings.push(("semantic target", true, reading));
                    }
                    crate::mask_refine::RefineOutcome::Abstained { reading } => {
                        readings.push(("semantic target", false, reading));
                    }
                }
                let zone_divergence = measure_zone_divergence(src, target, base, &src_mask);
                let divergent_cover = [zone_divergence.sky, zone_divergence.land]
                    .into_iter()
                    .filter(|zone| zone.divergence.d >= fit::DIVERGENCE_ZONE)
                    .map(|zone| zone.share)
                    .sum::<f32>();
                let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(
                    src,
                    target,
                    base,
                    divergent_cover >= fit::DIVERGENT_COVER_PROMOTES,
                    true,
                    options,
                );
                for (label, kept, reading) in readings {
                    crate::rationale::push_note(
                        &mut report.recipe.rationale,
                        &mut report.notes,
                        crate::rationale::Note::new(
                            if kept {
                                crate::rationale::keys::MASK_REFINEMENT_KEPT
                            } else {
                                crate::rationale::keys::MASK_REFINEMENT_ABSTAINED
                            },
                            vec![
                                ("label", label.to_string()),
                                ("coverage", format!("{:.6}", reading.coverage_delta)),
                                ("before", format!("{:.6}", reading.edge_before)),
                                ("after", format!("{:.6}", reading.edge_after)),
                                ("core", reading.core_changed.to_string()),
                            ],
                        ),
                    );
                }
                let field = layers.field
                    .then(|| field::solve_local_field(src, target, &mut report)).flatten();
                attach_zones_with_divergence(
                    src,
                    target,
                    &mut report,
                    &src_mask,
                    &tgt_mask,
                    mask_path,
                    zone_divergence,
                );
                (report, field, "zones")
            } else {
            let zone_divergence = measure_zone_divergence(src, target, base, &src_mask);
            let divergent_cover = [zone_divergence.sky, zone_divergence.land]
                .into_iter()
                .filter(|zone| zone.divergence.d >= fit::DIVERGENCE_ZONE)
                .map(|zone| zone.share)
                .sum::<f32>();
            let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(
                src,
                target,
                base,
                divergent_cover >= fit::DIVERGENT_COVER_PROMOTES,
                true,
                options,
            );
            let field = layers.field
                .then(|| field::solve_local_field(src, target, &mut report)).flatten();
            attach_zones_with_divergence(
                src,
                target,
                &mut report,
                &src_mask,
                &tgt_mask,
                mask_path,
                zone_divergence,
            );
            (report, field, "zones")
            }
        }
        Err(e) => {
            // The provider still rides the fallback: a failed segmentation
            // must not also cost the global fit its correspondence.
            let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(
                src,
                target,
                base,
                false,
                true,
                options,
            );
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONED_UNAVAILABLE,
                    vec![("e", crate::rationale::error_line(&e))],
                ),
            );
            let field = layers.field
                .then(|| field::solve_local_field(src, target, &mut report)).flatten();
            let proposals = field.as_ref()
                .map(|(_, reading)| reading.proposals.as_slice()).unwrap_or(&[]);
            range::attach_luminance_ranges(src, target, &mut report, proposals);
            (report, field, "ranges")
        }
    };
    run_local_sequencer(src, target, &mut report, &field, first_producer, mask_path, layers);
    report
}

/// The single local producer sequencer shared by the historical two-region
/// route, the semantic multi-region route, and the range fallback.  Keep the
/// order and disclosures stable: first producer -> stop -> tiles -> stop ->
/// free masks.
#[allow(clippy::too_many_arguments)]
fn run_local_sequencer(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    field: &Option<(crate::fit_field::LocalField, field::ShapeReading)>,
    first_producer: &str,
    mask_path: &crate::store::OwnedRaster,
    layers: ZonedLayerOpts,
) {
    if let Some((local, _)) = field {
        field::push_realized(report, local, first_producer);
        if layers.free_masks && field::stop_verdict(local, report.err_after) {
            let skipped = match (layers.spatial, layers.free_masks) {
                (true, true) => "tiles, free masks",
                (true, false) => "tiles",
                (false, true) => "free masks",
                (false, false) => "none",
            };
            field::push_stop(report, first_producer, skipped);
            return;
        }
    }
    let mut excluded = field
        .as_ref()
        .map(|(local, _)| vec![0.0f32; local.remainder.len()])
        .unwrap_or_default();
    if layers.spatial {
        let cap = field
            .as_ref()
            .map(|(_, reading)| reading.effective_tile_cap)
            .unwrap_or(spatial::SPATIAL_MAX_ATTACHMENTS);
        excluded = spatial::attach_tiles(src, target, report, mask_path, layers.refine_masks, cap);
        if let Some((local, _)) = field {
            field::push_realized(report, local, "tiles");
            if layers.free_masks && field::stop_verdict(local, report.err_after) {
                field::push_stop(report, "tiles", "free masks");
                return;
            }
        }
    }
    if layers.free_masks && let Some((local, _)) = field {
        let stage = freemask::attach_free_masks(
            src,
            target,
            report,
            mask_path,
            local,
            &excluded,
            layers.refine_masks,
            freemask::FREE_MASK_MAX_ATTACHMENTS,
        );
        debug_assert_eq!(stage.components, stage.disclosed);
        if stage.ran { field::push_realized(report, local, "free masks"); }
    }
}

/// Multi-class semantic path.  It intentionally shares the global solve and
/// downstream field/tile stages with the legacy path, while replacing only the
/// sky/land producer with one attachment per disjoint class region.
fn fit_recipe_zoned_multi_inner(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    max_regions: usize,
) -> FitReport {
    let semantic = segment_multiclass_both(src, target, seg, mask_path, max_regions);
    let (regions, rasters, sky_pair, refinements) = match semantic {
        Ok(pair) => pair,
        Err(e) => {
            // A multi-manifest failure is a semantic-layer failure, not a
            // reason to switch producers. Re-enter the historical two-region
            // route; it owns its own range fallback and sequencer, and its
            // result remains the byte-identity reference. Disclosed under its
            // OWN key: `ZONED_UNAVAILABLE` narrates a luminance-range fallback,
            // and the route that ran here is the sky/land pass.
            let mut report = fit_recipe_zoned_inner_with_options(
                src, target, seg, mask_path, base, options, SHIPPED_LAYERS,
            );
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::SEMANTIC_REGIONS_UNAVAILABLE,
                    // THE door, like every other `{e}` note: one line, absolute
                    // paths reduced to a basename (`[path]` inside a home
                    // directory), 160 characters. `{e:#}` flattened the whole
                    // anyhow chain and carried the sidecar's paths into a
                    // rationale that travels into XMP and into bug reports.
                    vec![("e", crate::rationale::error_line(&e))],
                ),
            );
            return report;
        }
    };
    if regions.is_empty() {
        // No class cleared the shared support floor on both frames. The
        // historical route is the reference result here too: it judges the
        // sky partition on its own numbers (and drops its anchor when that
        // fails) and runs the same sequencer — a typed hand-off, not a fourth
        // exit that would render bare placeholders and strand the anchor.
        let mut report = fit_recipe_zoned_inner_seeded(
            src, target, seg, mask_path, base, options, SHIPPED_LAYERS, Some(sky_pair),
        );
        push_refinement_notes(&mut report, &refinements);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::SEMANTIC_REGIONS_NONE,
                vec![("n", max_regions.to_string())],
            ),
        );
        return report;
    }
    let (mut report, field, first_producer) = {
        let (sp, tp, w, h) = fit::divergence_raster(src, target, base);
        let divergences = regions.iter().map(|r| {
            let weights = mask_weights(&r.source, w, h);
            ZoneDivergence { divergence: fit::structure_divergence(&sp, &tp, w, h, &weights), share: weights.iter().sum::<f32>() / weights.len().max(1) as f32 }
        }).collect::<Vec<_>>();
        let divergent_cover = divergences.iter().filter(|d| d.divergence.d >= fit::DIVERGENCE_ZONE).map(|d| d.share).sum::<f32>();
        let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(src, target, base,
            divergent_cover >= fit::DIVERGENT_COVER_PROMOTES, true, options);
        // The same disclosure the sky/land route makes for ITS refinement.
        push_refinement_notes(&mut report, &refinements);
        let field = SHIPPED_LAYERS.field.then(|| field::solve_local_field(src, target, &mut report)).flatten();
        attach_semantic_regions(src, target, &mut report, &regions, &rasters, &divergences);
        (report, field, "semantic regions")
    };
    run_local_sequencer(src, target, &mut report, &field, first_producer, mask_path, SHIPPED_LAYERS);
    // The four-region producer may leave pixels to the global fit by design;
    // the historical two-region producer owns the inverse-sky complement.
    // Compare against the unchanged legacy sequencer and keep the multi-class
    // candidate only when it is no worse. This is a strict selection gate,
    // not extra segmentation and not a tolerance. The multi-class sky plane
    // remains available for the region producer, but the legacy path is the
    // byte-identity reference and runs through the seeded sky bridge.
    let two = fit_recipe_zoned_inner_seeded(
        src,
        target,
        seg,
        mask_path,
        base,
        options,
        SHIPPED_LAYERS,
        Some(sky_pair),
    );
    let remove_unselected = |candidate: &FitReport, kept: &FitReport| {
        release_unselected_rasters(candidate, kept, mask_path)
    };
    // One ruler for the comparison. Each report's `err_after` was measured
    // under ITS OWN evidence model, and the two global solves can land in
    // different modes — a Full ruler is structural, an Atmosphere ruler
    // structure-blind — so those numbers are not comparable across reports.
    // Both finished renders are re-measured under the reference's ruler, and
    // those are the numbers the refusal discloses.
    let multi_error = frame_err_under(src, target, &report, &two.evidence);
    let two_error = frame_err_under(src, target, &two, &two.evidence);
    if multi_error >= two_error {
        let verdict_name = |key: &str| match key {
            crate::rationale::keys::ZONE_ATTACHED => "ZONE_ATTACHED",
            crate::rationale::keys::ZONE_ALREADY_MATCHED => "ZONE_ALREADY_MATCHED",
            crate::rationale::keys::ZONE_SHARE_NO_CORRECTION => "ZONE_NO_CORRECTION",
            crate::rationale::keys::ZONE_TOO_SMALL => "ZONE_TOO_SMALL",
            crate::rationale::keys::ZONE_SHARE_MISMATCH => "ZONE_SHARE_MISMATCH",
            crate::rationale::keys::ZONE_BOUNDARY_PASSED => "ZONE_BOUNDARY_PASSED",
            crate::rationale::keys::REGION_BOUNDARY_REFUSED => "REGION_BOUNDARY_REFUSED",
            crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED => "ZONE_QUALITY_TEXTURE_FAILED",
            crate::rationale::keys::ZONE_QUALITY_CLIPPING_FAILED => "ZONE_QUALITY_CLIPPING_FAILED",
            crate::rationale::keys::ZONE_DROPPED => "ZONE_DROPPED",
            crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED => "ZONE_ATMOSPHERE_DROPPED",
            crate::rationale::keys::ZONE_MODE_FULL => "ZONE_MODE_FULL",
            crate::rationale::keys::ZONE_MODE_ATMOSPHERE => "ZONE_MODE_ATMOSPHERE",
            crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR => "ZONE_EVIDENCE_WITHHELD_COLOUR",
            crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE => "ZONE_EVIDENCE_WITHHELD_TONE",
            crate::rationale::keys::ZONE_QUALITY_PASSED => "ZONE_QUALITY_PASSED",
            // An honest "something else", never a verdict the zone did not get.
            _ => "ZONE_OTHER",
        };
        let regions_text = regions.iter().map(|region| {
            let label = format!("region-{}-{}", region.class_id, region.label);
            let verdict = report.notes.iter().rev()
                .find(|note| note.args.iter().any(|(name, value)| *name == "label" && value == &label))
                .map(|note| note.key)
                .unwrap_or(crate::rationale::keys::ZONE_SHARE_NO_CORRECTION);
            format!("{} {}: {}", region.class_id, region.label, verdict_name(verdict))
        }).collect::<Vec<_>>().join("; ");
        remove_unselected(&report, &two);
        let mut chosen = two;
        let refusal = crate::rationale::Note::new(
            crate::rationale::keys::REGION_FRAME_REFUSED,
            vec![
                ("multi", format!("{multi_error:.6}")),
                ("two", format!("{two_error:.6}")),
                ("regions", regions_text),
            ],
        );
        chosen.recipe.rationale.push_str(&crate::rationale::render_one(&refusal));
        // Keep the truncation sentinel in place.  Appending the arbitration
        // verdict after it preserves the consumer's raw-English fallback
        // while still exposing the typed decision to the GUI/test callers.
        chosen.notes.push(refusal);
        return chosen;
    }
    // Keep the selected semantic candidate and release only the seeded legacy
    // candidate's unreferenced raster claims.
    remove_unselected(&two, &report);
    report
}

/// Claim hygiene after arbitration: every bitmap raster the losing
/// `candidate` references and the `kept` report does not is released, and so
/// is the shared anchor when nothing kept references it. Only files inside
/// the anchor's own directory (the develop store) are touched, so a recipe
/// that names a raster elsewhere can never make the fit delete it.
fn release_unselected_rasters(
    candidate: &FitReport,
    kept: &FitReport,
    anchor: &crate::store::OwnedRaster,
) {
    let bitmaps = |report: &FitReport| {
        report
            .recipe
            .masks
            .iter()
            .filter_map(|mask| match &mask.mask {
                MaskGeometry::Bitmap { path } => Some(path.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let keep = bitmaps(kept).into_iter().collect::<std::collections::HashSet<_>>();
    let parent = anchor.path().parent();
    for path in bitmaps(candidate) {
        let file = std::path::Path::new(&path);
        if !keep.contains(&path) && file.parent() == parent {
            let _ = std::fs::remove_file(file);
        }
    }
    let anchor_name = anchor.path().to_string_lossy().into_owned();
    if !keep.contains(&anchor_name) {
        anchor.remove();
    }
}

/// A report's finished render measured under a GIVEN evidence ruler — the
/// only way two reports that may have solved in different modes can be
/// compared. Analysis geometry, like every zone gate.
fn frame_err_under(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &FitReport,
    evidence: &fit::EvidenceModel,
) -> f32 {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    fit::look_err_with_evidence(&px, &tgt_px, evidence)
}

/// One guided-refinement reading per class plane, as the bridge took it.
type PlaneRefinement = (String, bool, crate::mask_refine::RefineReading);

fn push_refinement_notes(report: &mut FitReport, refinements: &[PlaneRefinement]) {
    for (label, kept, reading) in refinements {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                if *kept {
                    crate::rationale::keys::MASK_REFINEMENT_KEPT
                } else {
                    crate::rationale::keys::MASK_REFINEMENT_ABSTAINED
                },
                vec![
                    ("label", label.clone()),
                    ("coverage", format!("{:.6}", reading.coverage_delta)),
                    ("before", format!("{:.6}", reading.edge_before)),
                    ("after", format!("{:.6}", reading.edge_after)),
                    ("core", reading.core_changed.to_string()),
                ],
            ),
        );
    }
}

/// Run the one-inference-per-frame multi-class sidecar and materialise source
/// planes as owned rasters for the recipe. Temporary manifests and sidecar
/// plane files are removed before returning; the recipe owns only the claimed
/// source rasters it actually receives. The refinement readings ride along so
/// the caller can disclose them once it has a report to disclose into.
type MultiClassSegments = (
    Vec<semantic::SemanticRegion>,
    Vec<crate::store::OwnedRaster>,
    (GrayImage, GrayImage),
    Vec<PlaneRefinement>,
);

fn segment_multiclass_both(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    max_regions: usize,
) -> Result<MultiClassSegments> {
    let sibling = |suffix: &str| {
        let mut p = mask_path.path().as_os_str().to_owned();
        p.push(suffix);
        std::path::PathBuf::from(p)
    };
    let src_in = sibling(".multi-src.png");
    let tgt_in = sibling(".multi-tgt.png");
    let src_manifest = sibling(".multi-src.json");
    let tgt_manifest = sibling(".multi-tgt.json");
    let run = (|| -> Result<MultiClassSegments> {
        // The single-class bridge's sizing, through the ONE helper both
        // bridges share: the seeded legacy run is byte-identical to an
        // unseeded one only if the sidecar saw identical inputs.
        let source_input = segmentation_input(src);
        let target_input = segmentation_input(target);
        source_input.to_rgb8().save(&src_in).context("write multi-class source input")?;
        target_input.to_rgb8().save(&tgt_in).context("write multi-class target input")?;
        let sm = crate::segment::segment_multiclass_file(seg, &src_in, &src_manifest, max_regions)?;
        let tm = crate::segment::segment_multiclass_file(seg, &tgt_in, &tgt_manifest, max_regions)?;
        let source_sky = sm
            .planes
            .iter()
            .find(|plane| plane.label.trim().eq_ignore_ascii_case("sky"))
            .map(|plane| plane.mask.clone())
            .context("multi-class source manifest has no sky plane")?;
        let target_sky = tm
            .planes
            .iter()
            .find(|plane| plane.label.trim().eq_ignore_ascii_case("sky"))
            .map(|plane| plane.mask.clone())
            .context("multi-class target manifest has no sky plane")?;
        // Keep the historical anchor valid for the seeded legacy run. The
        // legacy path may abstain from refinement and still reference this
        // raster, so it must be materialised before returning the pair.
        source_sky.save(mask_path.path()).context("save seeded legacy sky mask")?;
        let source_dims = (sm.width, sm.height);
        let mut source = sm.planes.into_iter().map(|p| semantic::ClassPlane {
            class_id: p.class_id, label: p.label, mean_confidence: p.mean_confidence, mask: p.mask,
        }).collect::<Vec<_>>();
        let mut target_planes = tm.planes.into_iter().map(|p| semantic::ClassPlane {
            class_id: p.class_id,
            label: p.label,
            mean_confidence: p.mean_confidence,
            // Segmentation runs independently on each input, so their native
            // raster sizes can differ by a row/column.  The fit's evidence
            // geometry is source-owned; resample the target plane into it
            // before pairing counterparts.
            mask: if p.mask.dimensions() == source_dims {
                p.mask
            } else {
                image::imageops::resize(&p.mask, source_dims.0, source_dims.1, image::imageops::FilterType::Triangle)
            },
        }).collect::<Vec<_>>();
        // The shipped sky/land producer refines both semantic planes before
        // fitting. Apply the same bounded guide to every class before overlap
        // resolution, so the disjoint partition is built from the alphas the
        // renderer will actually persist rather than from a second, rougher
        // semantic path.
        let mut refinements: Vec<PlaneRefinement> = Vec::new();
        for (side, frame, planes) in [("source", src, &mut source), ("target", target, &mut target_planes)] {
            for plane in planes.iter_mut() {
                let label = format!("semantic {side} class {} {}", plane.class_id, plane.label);
                match crate::mask_refine::guided_refine(
                    frame,
                    &plane.mask,
                    MASK_REFINE_RADIUS,
                    MASK_REFINE_EPSILON,
                ) {
                    crate::mask_refine::RefineOutcome::Kept { mask, reading } => {
                        plane.mask = mask;
                        refinements.push((label, true, reading));
                    }
                    crate::mask_refine::RefineOutcome::Abstained { reading } => {
                        refinements.push((label, false, reading));
                    }
                }
            }
        }
        let regions = semantic::resolve_regions(&source, &target_planes, max_regions);
        if let Some(first) = regions.first()
            && !semantic::bitmap_budget_allows(
                0,
                first.source.width(),
                first.source.height(),
                regions.len(),
            )
        {
            anyhow::bail!(
                "semantic region bitmap budget refused {} regions at {}x{}",
                regions.len(), first.source.width(), first.source.height()
            );
        }
        let mut rasters: Vec<crate::store::OwnedRaster> = Vec::with_capacity(regions.len());
        for region in &regions {
            let safe = region.label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>();
            let prefix = format!("mask-region-{}-{}", region.class_id, safe.trim_matches('-'));
            let raster = match mask_path.claim_sibling(&prefix) {
                Ok(raster) => raster,
                Err(e) => {
                    for claimed in &rasters { claimed.remove(); }
                    return Err(e).with_context(|| format!("claim semantic region {}", region.class_id));
                }
            };
            if let Err(e) = region.source.save(raster.path()) {
                rasters.push(raster);
                for claimed in rasters { claimed.remove(); }
                return Err(e).with_context(|| format!("write semantic region {}", region.class_id));
            }
            rasters.push(raster);
        }
        Ok((regions, rasters, (source_sky, target_sky), refinements))
    })();
    for p in [&src_in, &tgt_in, &src_manifest, &tgt_manifest] { let _ = std::fs::remove_file(p); }
    run
}

fn attach_semantic_regions(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    regions: &[semantic::SemanticRegion],
    rasters: &[crate::store::OwnedRaster],
    divergences: &[ZoneDivergence],
) {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let preview = render::develop_preview(&s_img, &report.recipe);
    let (aw, ah) = preview.dimensions();
    let mut frame_err = report.err_after;
    let corr = report.correspondence.take();
    let mut accepted = Vec::new();
    for ((region, raster), divergence) in regions.iter().zip(rasters).zip(divergences) {
        let sw = mask_weights(&region.source, aw, ah);
        let tw = mask_weights(&region.target, t_img.width(), t_img.height());
        let attachment = ZoneAttachment {
            source_weights: sw,
            target_weights: tw,
            coverage: None,
            mask: MaskGeometry::Bitmap { path: raster.path().to_string_lossy().into_owned() },
            range: None,
            name: format!("region-{}-{}", region.class_id, region.label),
            role: MaskRole::Custom,
            inverted: false,
            label: format!("region-{}-{}", region.class_id, region.label),
            min_share: MIN_ZONE_SHARE,
            frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
        };
        let frame_before = frame_err;
        if let Some(mut zone) = attach_one_zone(
            &s_img,
            &tgt_px,
            report,
            &mut frame_err,
            &attachment,
            divergence.divergence,
            corr.as_ref(),
        ) {
            let boundary = spatial::enforce_bitmap_boundary(
                &s_img,
                &tgt_px,
                report,
                zone.mask_index,
                spatial::BitmapBoundaryInput {
                    // Segmentation rasters are feathered, so the transition
                    // band this ruler reads is the real one. Measured against
                    // the cross-boundary step on the calibration corpus and
                    // the viaduct pair (2026-08-30); see the batch report.
                    ruler: spatial::BoundaryRuler::TransitionBand {
                        weights: &zone.source_weights,
                    },
                    initial_px: zone.rendered,
                    frame_before,
                },
            );
            match boundary {
                Ok(boundary) => {
                    let target_moments = zone_moments(&tgt_px, &zone.target_weights);
                    zone.after = zone_err(
                        &zone_moments(&boundary.pixels, &zone.source_weights),
                        &target_moments,
                    );
                    zone.rendered = boundary.pixels;
                    frame_err = fit::look_err_with_evidence(
                        &zone.rendered,
                        &tgt_px,
                        &report.evidence,
                    );
                    crate::rationale::push_note(
                        &mut report.recipe.rationale,
                        &mut report.notes,
                        crate::rationale::Note::new(
                            crate::rationale::keys::ZONE_BOUNDARY_PASSED,
                            vec![
                                ("label", attachment.label.clone()),
                                ("n", "1".to_string()),
                                ("before", format!("{:.3}", boundary.initial.rim)),
                                ("after", format!("{:.3}", boundary.reading.rim)),
                                ("k", format!("{:.3}", boundary.k)),
                                ("max", format!("{:.3}", ZONE_BOUNDARY_RIM_MAX)),
                                ("transitions", boundary.reading.transitions.to_string()),
                            ],
                        ),
                    );
                    accepted.push(zone);
                }
                Err(refusal) => {
                    raster.remove();
                    // The shared gate hands back only what it measured — the
                    // candidate's rim and WHY it refused. Nothing is invented
                    // for a shrink that was never accepted.
                    let why = match refusal.why {
                        spatial::BitmapBoundaryWhy::Rim => "no shared shrink met the rim budget",
                        spatial::BitmapBoundaryWhy::Frame => "the composed frame regressed",
                        // Unreachable on this arm: a feathered region is read
                        // by the transition-band ruler, which never refuses
                        // for want of a measurement. Named, not wildcarded,
                        // so adding a third ruler here has to come back.
                        spatial::BitmapBoundaryWhy::Unmeasured => {
                            "the region boundary could not be sampled"
                        }
                    };
                    crate::rationale::push_note(
                        &mut report.recipe.rationale,
                        &mut report.notes,
                        crate::rationale::Note::new(
                            crate::rationale::keys::REGION_BOUNDARY_REFUSED,
                            vec![
                                ("label", attachment.label.clone()),
                                ("why", why.to_string()),
                                ("before", format!("{:.3}", refusal.initial.rim)),
                                ("max", format!("{:.3}", ZONE_BOUNDARY_RIM_MAX)),
                                ("transitions", refusal.initial.transitions.to_string()),
                            ],
                        ),
                    );
                    frame_err = frame_before;
                }
            }
        }
    }
    report.correspondence = corr;
    if accepted.is_empty() {
        for raster in rasters { raster.remove(); }
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(report, &finished, &tgt_px);
        return;
    }
    // A class can pass partition support yet fail its own local quality or
    // acceptance gate.  Release that unreferenced claim; only masks that made
    // it into the recipe survive the run.
    for raster in rasters {
        let path = raster.path().to_string_lossy();
        if !report.recipe.masks.iter().any(|m| matches!(&m.mask, MaskGeometry::Bitmap { path: p } if p == path.as_ref())) {
            raster.remove();
        }
    }
    for zone in &accepted { push_zone_attached_note(report, zone); }
    report.err_after = frame_err;
    let worst = semantic::worst_region_residual(&accepted.iter().map(|z| z.after).collect::<Vec<_>>());
    report.recipe.confidence = report.recipe.confidence.min(fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE));
    crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
        crate::rationale::Note::new(crate::rationale::keys::ZONE_CONFIDENCE,
            vec![("n", accepted.len().to_string()), ("worst", format!("{worst:.3}")), ("frame", format!("{frame_err:.3}"))]));
    let final_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    fit::append_finished_disclosure(report, &final_px, &tgt_px);
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
    /// The population the correction MOVES when it differs from the estimator
    /// weights. `None`: the weights are the coverage (a semantic mask, a
    /// luminance ramp). A tile passes its raster: its estimator weights are
    /// evidence-weighted, so asking the evidence vetoes over them would hide
    /// exactly the withheld pixels the raster still moves.
    coverage: Option<ZoneCoverage>,
    mask: MaskGeometry,
    range: Option<RangeMask>,
    name: String,
    role: MaskRole,
    inverted: bool,
    label: String,
    min_share: f32,
    frame_regression_tol: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct ZoneCoverage {
    source: Vec<f32>,
    target: Vec<f32>,
}

#[cfg(test)]
thread_local! {
    static SEGMENT_BOTH_OVERRIDE: std::cell::RefCell<Option<(GrayImage, GrayImage)>> =
        const { std::cell::RefCell::new(None) };
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

/// The long edge past which a frame is thumbnailed before segmentation.
const SEGMENTATION_INPUT_EDGE: u32 = 2048;

/// THE input-sizing rule every segmentation bridge hands the sidecar: native
/// pixels through [`SEGMENTATION_INPUT_EDGE`], otherwise the same thumbnail.
/// Segmentation reads scene SEMANTICS, not pixels: a ≤2048 input finds the
/// sky exactly as well as a 61 MP master while skipping a ~180 MB PNG
/// round-trip per side (the CLI fit hands full-res frames here). The
/// persisted mask raster is normalised-coordinate data — the engine resamples
/// it at whatever resolution the develop runs, and the GUI's own reverse-fit
/// already segments preview-res frames.
///
/// One copy on purpose. `image::thumbnail` has no ratio>1 guard, so an
/// unconditional call UPSCALES a smaller frame; the multi-class bridge once
/// carried its own unconditional copy and its sky plane differed from the
/// single-class mask on every ≤2048 frame — which is what the seeded legacy
/// run's byte identity rests on.
fn segmentation_input(img: &DynamicImage) -> std::borrow::Cow<'_, DynamicImage> {
    if img.width().max(img.height()) > SEGMENTATION_INPUT_EDGE {
        std::borrow::Cow::Owned(img.thumbnail(SEGMENTATION_INPUT_EDGE, SEGMENTATION_INPUT_EDGE))
    } else {
        std::borrow::Cow::Borrowed(img)
    }
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
    // Sizing lives in `segmentation_input` — shared with the multi-class
    // bridge, which is why the seeded legacy run can be byte-identical.
    let src = segmentation_input(src);
    let target = segmentation_input(target);
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
    let (s_img, t_img) = fit::analysis_pair(src, target);
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
        coverage: None,
        mask: MaskGeometry::Bitmap { path: mask_path.path().to_string_lossy().into_owned() },
        range: None,
        name: String::new(),
        role: MaskRole::ZoneSky,
        inverted: false,
        label: MaskRole::ZoneSky.tag().to_string(),
        min_share: MIN_ZONE_SHARE,
        frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
    };
    let land_attachment = ZoneAttachment {
        source_weights: swl,
        target_weights: twl,
        coverage: None,
        mask: MaskGeometry::Bitmap { path: mask_path.path().to_string_lossy().into_owned() },
        range: None,
        name: String::new(),
        role: MaskRole::ZoneLand,
        inverted: true,
        label: MaskRole::ZoneLand.tag().to_string(),
        min_share: MIN_ZONE_SHARE,
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
    let worst = semantic::worst_region_residual(&accepted.iter().map(|z| z.after).collect::<Vec<_>>());
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
            (ms.share >= attachment.min_share && mt.share >= attachment.min_share)
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
    if gate_s_share < attachment.min_share || gate_t_share < attachment.min_share {
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
    // Evidence verdicts follow both the population a correction MOVES and its
    // mode. Atmosphere zones scope the report's one frame ruler (population
    // evidence in an Atmosphere report). Full zones retain structural survival,
    // so inside an Atmosphere frame they scope the separately carried structural
    // model. This single branch covers semantic zones, ranges and tiles.
    let (moved_source, moved_target) = match &attachment.coverage {
        Some(coverage) => (coverage.source.as_slice(), coverage.target.as_slice()),
        None => (sw, tw),
    };
    let frame_evidence = match mode {
        ZoneMode::Atmosphere => &report.evidence,
        ZoneMode::Full => report.structural_evidence.as_ref().unwrap_or(&report.evidence),
    };
    let zone_evidence = frame_evidence.scoped(tgt_px, moved_source, moved_target);
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
        &zone_evidence,
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
        &zone_evidence,
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
    // The skip line, re-asked of the class that can still move: with colour
    // withheld the acceptance below judges the luma-only residual, so a zone
    // already matched THERE is left alone with the honest note instead of
    // being dialled for a hairline tone gain against a chroma gap it may not
    // touch (the calibration land: luma 0.004, chroma-dominated 0.045).
    if hue_bands.is_some() && luma_ranges.is_none() {
        let luma_before = zone_luma_err(&ms, &mt);
        if zone_skips(luma_before, ev_gap) {
            report.recipe.masks.pop();
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_ALREADY_MATCHED,
                    vec![("label", label.to_string()), ("before", format!("{luma_before:.3}"))],
                ),
            );
            return None;
        }
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
                    ("drift", format!("{:+.5}", zoned_err - *frame_err)),
                    ("tol", format!("{:+.5}", attachment.frame_regression_tol)),
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
pub(super) fn mask_weights(mask: &GrayImage, w: u32, h: u32) -> Vec<f32> {
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
    pub(super) fn zoned_pair() -> (DynamicImage, DynamicImage, GrayImage) {
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
    pub(super) fn fixture_mask_path(name: &str) -> crate::store::OwnedRaster {
        crate::store::OwnedRaster::scratch(
            std::env::temp_dir().join(format!("autoshade-{name}-{}.png", std::process::id())),
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
            coverage: None,
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            range: None,
            name: String::new(),
            role: MaskRole::ZoneSky,
            inverted: false,
            label: MaskRole::ZoneSky.tag().to_string(),
            min_share: MIN_ZONE_SHARE,
            frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
        }
    }

    fn divergence(d: f32) -> fit::Divergence {
        fit::Divergence { correlation: 1.0 - d, energy_error: 0.0, d }
    }

    fn pretend_full_support(evidence: &mut fit::EvidenceModel) {
        evidence.spatial_weights.fill(1.0);
        evidence.spatial_divergence.fill(0.0);
        evidence.spatial_supported.fill(true);
        evidence.globally_same_content = true;
        evidence.source_weights.fill(1.0);
        evidence.target_weights.fill(1.0);
    }

    pub(super) fn neutral_report(src: &DynamicImage, tgt: &DynamicImage) -> fit::FitReport {
        let (s, t) = fit::analysis_pair(src, tgt);
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
            structural_evidence: None,
        }
    }

    #[test]
    fn full_zone_in_atmosphere_frame_reads_structural_evidence() {
        let edge = 64u32;
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
            let value = 0.30 + 0.02 * ((x + y) % 2) as f32;
            image::Rgb([(value * 255.0).round() as u8; 3])
        }));
        let divergent = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
            let value = if ((x / 3) + (y / 5)) % 2 == 0 { 0.12f32 } else { 0.88 };
            image::Rgb([(value * 255.0).round() as u8; 3])
        }));
        let carried = fit::fit_recipe(&source, &divergent);
        assert_eq!(carried.mode, fit::FitMode::Atmosphere, "premise: divergent frame");
        assert!(
            carried.structural_evidence.is_some(),
            "an Atmosphere frame must carry structural evidence for its Full zones"
        );
        let target = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
            let value = 0.50 + 0.02 * ((x + y) % 2) as f32;
            image::Rgb([(value * 255.0).round() as u8; 3])
        }));
        let (s_img, t_img) = fit::analysis_pair(&source, &target);
        let (sp, tp) = (fit::pixels_of(&s_img), fit::pixels_of(&t_img));
        let mut ingredients = fit::evidence_model_for(&sp, &tp, edge, edge);
        pretend_full_support(&mut ingredients);
        ingredients.spatial_weights.fill(0.0);
        ingredients.spatial_divergence.fill(1.0);
        ingredients.spatial_supported.fill(false);
        ingredients.globally_same_content = false;
        let ones = vec![1.0; sp.len()];
        let structural = ingredients.scoped(&tp, &ones, &ones);
        assert!(
            structural
                .luma
                .iter()
                .any(|range| range.source_populated && range.target_populated && range.weight == 0.0),
            "premise: the synthetic structural model withholds a populated tone range"
        );
        let blind = structural.structure_blind(&tp);
        let frame_err = fit::look_err_with_evidence(&sp, &tp, &blind);
        let build_report = || {
            let mut report = neutral_report(&source, &target);
            report.mode = fit::FitMode::Atmosphere;
            report.divergence = divergence(0.8);
            report.err_before = frame_err;
            report.err_after = frame_err;
            report.evidence = blind.clone();
            report.structural_evidence = Some(structural.clone());
            report
        };
        let mask = GrayImage::from_pixel(edge, edge, image::Luma([255u8]));
        let path = fixture_mask_path("atmosphere-frame-full-zone-structural");
        mask.save(path.path()).unwrap();
        let attachment = semantic_attachment(ones.clone(), ones, &path);

        let mut full_report = build_report();
        let mut full_frame_err = frame_err;
        let _ = attach_one_zone(
            &s_img,
            &tp,
            &mut full_report,
            &mut full_frame_err,
            &attachment,
            divergence(0.0),
            None,
        );
        assert!(
            full_report.notes.iter().any(|note| {
                note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE
                    && note.args.iter().any(|(key, value)| {
                        *key == "label" && value == MaskRole::ZoneSky.tag()
                    })
            }),
            "a Full zone must retain structural withholding: {}",
            full_report.recipe.rationale
        );

        let mut atmosphere_report = build_report();
        let mut atmosphere_frame_err = frame_err;
        let _ = attach_one_zone(
            &s_img,
            &tp,
            &mut atmosphere_report,
            &mut atmosphere_frame_err,
            &attachment,
            divergence(0.8),
            None,
        );
        assert!(
            !atmosphere_report.notes.iter().any(|note| {
                note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE
            }),
            "an Atmosphere zone must read the blind report ruler: {}",
            atmosphere_report.recipe.rationale
        );
        path.remove();
    }

    fn legacy_zoned_fit(
        src: &DynamicImage,
        target: &DynamicImage,
        seg: &SegmentOpts,
        mask_path: &crate::store::OwnedRaster,
        base: &crate::recipe::EditRecipe,
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
                    None,
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
                let mut report = fit::fit_recipe_from_promoted_with_disclosure(
                    src,
                    target,
                    base,
                    false,
                    true,
                    None,
                );
                crate::rationale::push_note(
                    &mut report.recipe.rationale,
                    &mut report.notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::ZONED_UNAVAILABLE,
                        vec![("e", crate::rationale::error_line(&e))],
                    ),
                );
                range::attach_luminance_ranges(src, target, &mut report, &[]);
                report
            }
        }
    }

    #[test]
    fn layered_disabled_is_byte_identical_to_current_zoned_fit() {
        let (source, target, sky) = zoned_pair();
        let seg = SegmentOpts {
            python_bin: "autoshade-test-no-such-python".into(),
            script: "Cargo.toml".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let layers = ZonedLayerOpts {
            field: false, spatial: false, free_masks: false, refine_masks: false,
        };

        let semantic_path = fixture_mask_path("layered-disabled-semantic");
        sky.save(semantic_path.path()).unwrap();
        SEGMENT_BOTH_OVERRIDE.with(|value| *value.borrow_mut() = Some((sky.clone(), sky.clone())));
        let disabled_semantic = fit_recipe_zoned_inner(
            &source,
            &target,
            &seg,
            &semantic_path,
            &crate::recipe::EditRecipe::default(),
            None,
            layers,
        );
        SEGMENT_BOTH_OVERRIDE.with(|value| *value.borrow_mut() = Some((sky.clone(), sky)));
        let legacy_semantic = legacy_zoned_fit(
            &source,
            &target,
            &seg,
            &semantic_path,
            &crate::recipe::EditRecipe::default(),
        );
        assert_eq!(
            serde_json::to_vec(&disabled_semantic.recipe).unwrap(),
            serde_json::to_vec(&legacy_semantic.recipe).unwrap(),
            "disabled semantic layers changed the pre-layer recipe bytes",
        );
        assert_eq!(disabled_semantic.err_after.to_bits(), legacy_semantic.err_after.to_bits());
        semantic_path.remove();

        let range_path = fixture_mask_path("layered-disabled-range");
        let disabled_range = fit_recipe_zoned_inner(
            &source,
            &target,
            &seg,
            &range_path,
            &crate::recipe::EditRecipe::default(),
            None,
            layers,
        );
        let legacy_range = legacy_zoned_fit(
            &source,
            &target,
            &seg,
            &range_path,
            &crate::recipe::EditRecipe::default(),
        );
        assert_eq!(
            serde_json::to_vec(&disabled_range.recipe).unwrap(),
            serde_json::to_vec(&legacy_range.recipe).unwrap(),
            "disabled range layers changed the pre-layer recipe bytes",
        );
        assert_eq!(disabled_range.err_after.to_bits(), legacy_range.err_after.to_bits());
        range_path.remove();

        let (Some(head_semantic), Some(head_range)) = (
            crate::config::live_env("AUTOSHADE_LAYERED_HEAD_SEMANTIC"),
            crate::config::live_env("AUTOSHADE_LAYERED_HEAD_RANGE"),
        ) else {
            return;
        };
        let root = fit::calibration_corpus().expect("HEAD equivalence needs calibration corpus");
        let source = image::open(root.join("neutral.jpg")).unwrap();
        let target = image::open(root.join("target.jpg")).unwrap();
        let cfg = crate::config::Config::load();
        let corr = crate::correspond::fit_provider(
            crate::correspond::CorrespondOpts::from_config(&cfg),
        );
        let semantic_path = fixture_mask_path("layered-head-semantic");
        let mut semantic = fit_recipe_zoned_inner(
            &source,
            &target,
            &SegmentOpts::from_config(&cfg, "sky"),
            &semantic_path,
            &crate::recipe::EditRecipe::default(),
            Some(&corr),
            layers,
        );
        crate::pipeline::stamp_fit_calibration(
            &mut semantic.recipe,
            crate::pipeline::fit_calibration(&root.join("neutral.jpg")),
        );
        let head_semantic: crate::recipe::EditRecipe =
            serde_json::from_slice(&std::fs::read(head_semantic).unwrap()).unwrap();
        assert_head_equivalent(
            &semantic.recipe,
            &head_semantic,
            &root.join("neutral.jpg"),
            "disabled semantic layers",
        );
        semantic_path.remove();

        let range_path = fixture_mask_path("layered-head-range");
        let range_seg = SegmentOpts {
            python_bin: cfg.python_bin.clone(),
            script: "D:/no-such-dir/none.py".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let mut range = fit_recipe_zoned_inner(
            &source,
            &target,
            &range_seg,
            &range_path,
            &crate::recipe::EditRecipe::default(),
            Some(&corr),
            layers,
        );
        crate::pipeline::stamp_fit_calibration(
            &mut range.recipe,
            crate::pipeline::fit_calibration(&root.join("neutral.jpg")),
        );
        let head_range: crate::recipe::EditRecipe =
            serde_json::from_slice(&std::fs::read(head_range).unwrap()).unwrap();
        assert_head_equivalent(
            &range.recipe,
            &head_range,
            &root.join("neutral.jpg"),
            "disabled range layers",
        );
        range_path.remove();
    }

    #[test]
    fn field_disabled_layer_is_byte_identical() {
        let (source, target, sky) = zoned_pair();
        let seg = SegmentOpts {
            python_bin: "unused-field-none".into(),
            script: "unused-field-none".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let path = fixture_mask_path("field-disabled-byte-identity");
        sky.save(path.path()).unwrap();
        SEGMENT_BOTH_OVERRIDE.with(|value|
            *value.borrow_mut() = Some((sky.clone(), sky.clone())));
        let disabled = fit_recipe_zoned_inner(
            &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
            ZonedLayerOpts {
                field: false, spatial: false, free_masks: false, refine_masks: false,
            },
        );
        SEGMENT_BOTH_OVERRIDE.with(|value|
            *value.borrow_mut() = Some((sky.clone(), sky)));
        field::FIELD_FORCE_NONE.with(|value| value.set(true));
        let refused = fit_recipe_zoned_inner(
            &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
            ZonedLayerOpts {
                field: true, spatial: false, free_masks: false, refine_masks: false,
            },
        );
        assert_eq!(serde_json::to_vec(&disabled.recipe).unwrap(),
            serde_json::to_vec(&refused.recipe).unwrap());
        assert_eq!(disabled.recipe.rationale, refused.recipe.rationale);
        assert_eq!(disabled.err_after.to_bits(), refused.err_after.to_bits());
        path.remove();
    }

    #[test]
    fn field_stop_rule_skips_the_tile_producer_and_names_it() {
        let (current, target, width, height) = crate::fit_field::tests::two_band_pair();
        let image = |pixels: &[[f32; 3]]| DynamicImage::ImageRgb8(RgbImage::from_fn(
            width, height, |x, y| image::Rgb(pixels[(y * width + x) as usize]
                .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)),
        ));
        let (source, target) = (image(&current), image(&target));
        let seg = SegmentOpts {
            python_bin: "autoshade-test-no-such-python".into(),
            script: "target/b2-no-segment.py".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let path = fixture_mask_path("field-stop");
        // Ceiling forced to sit 1e-4 under the producer-free frame: any
        // producer that does not regress the frame then lands inside the
        // 0.002 stop margin, and the guard `ceiling < global` still holds.
        field::FIELD_CEILING_OVERRIDE.with(|value| value.set(Some(1e-4)));
        let report = fit_recipe_zoned_inner(
            &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
            ZonedLayerOpts {
                field: true, spatial: true, free_masks: true, refine_masks: false,
            },
        );
        let stop = report.notes.iter().position(|note| note.key == crate::rationale::keys::LOCAL_STOP)
            .unwrap_or_else(|| panic!("missing stop: {}", report.recipe.rationale));
        assert!(report.notes[stop].args.iter().any(|(key, value)|
            *key == "skipped" && value == "tiles, free masks"));
        assert!(report.notes.iter().skip(stop + 1).all(|note| !note.key.starts_with(" Spatial")));
        let finished = report.notes.iter().filter(|note| {
            note.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
                || note.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
        }).count();
        assert_eq!(finished, 1, "finished disclosure count: {}", report.recipe.rationale);
        path.remove();
    }

    #[test]
    fn calibration_local_field_discloses_ceiling_and_realized_share() {
        let Some(root) = fit::calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).unwrap();
        let target = image::open(root.join("target.jpg")).unwrap();
        let seg = SegmentOpts {
            python_bin: "autoshade-test-no-such-python".into(),
            script: "target/b2-no-segment.py".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let head_path = fixture_mask_path("field-calibration-head");
        let head = fit_recipe_zoned_inner(
            &source, &target, &seg, &head_path, &crate::recipe::EditRecipe::default(), None,
            ZonedLayerOpts {
                field: false, spatial: true, free_masks: false, refine_masks: true,
            },
        );
        let path = fixture_mask_path("field-calibration");
        let report = fit_recipe_zoned(&source, &target, &seg, &path);
        let ceiling = report.notes.iter()
            .find(|note| note.key == crate::rationale::keys::LOCAL_CEILING)
            .expect("calibration field ceiling disclosure");
        let number = |name: &str| ceiling.args.iter()
            .find_map(|(key, value)| (*key == name).then(|| value.parse::<f32>().unwrap()))
            .unwrap();
        assert!(number("ceiling") <= number("global"));
        // The producer-free share is measured, not written: `field.global` and
        // the report's own `err_after` are the same objective on the same
        // pixels, so the disclosure must read exactly 0.000 before any producer.
        assert_eq!(number("realized"), 0.0, "{}", report.recipe.rationale);
        // The quadtree must run under the cap the analyzer disclosed, end to end.
        let arg = |key: &str, name: &str| report.notes.iter().find(|note| note.key == key)
            .and_then(|note| note.args.iter().find_map(|(k, v)| (*k == name).then(|| v.clone())));
        assert_eq!(arg(crate::rationale::keys::LOCAL_SHAPE, "cap"),
            arg(crate::rationale::keys::TILE_DEPTH_CAP, "cap"),
            "{}", report.recipe.rationale);
        assert!(arg(crate::rationale::keys::LOCAL_SHAPE, "cap").is_some());
        assert!(report.notes.iter().any(|note| note.key == crate::rationale::keys::LOCAL_REALIZED));
        // 2026-08-30, tile-boundary root fix. This was an exact-or-better
        // claim; it is now a bounded one, and the bound is measured, not
        // guessed. The analyzer's OWN cap is tighter than the default the
        // head arm runs under on this pair -- `free_form` verdict, effective
        // tile cap 2 against 4 -- so the head arm attaches one extra tile
        // (d2r3c2, d2r3c1, then d2r2c0) that the field path never reaches.
        // That third tile is not a seam: its cross-boundary step is 0.0031
        // and it is kept whole at k=1. Until the seam budget could actually
        // bind, the free-mask stage happened to cover the gap; with the
        // budget binding, both of this pair's free-mask proposals are
        // refused by the zone estimator instead, so the cap's own cost is
        // now visible in the arithmetic: 9.6e-5, 0.13% of the reading. What
        // is being paid for is the analyzer's stopping rule, not a
        // regression in the fit, so the guard keeps its direction under a
        // stated ceiling rather than a claim it can no longer make.
        const FIELD_CAP_COST: f32 = 2e-4;
        assert!(report.err_after <= head.err_after + FIELD_CAP_COST,
            "field path {} regressed HEAD semantics {} by more than the              analyzer's disclosed tile-cap cost", report.err_after, head.err_after);
        assert!(report.recipe.rationale.len() < 16 * 1024,
            "rationale is {} bytes", report.recipe.rationale.len());
        eprintln!("calibration HEAD={} field={} rationale={}",
            head.err_after, report.err_after, report.recipe.rationale.len());
        head_path.remove();
        path.remove();
    }

    /// The pre-batch executable clamped its persisted rationale at 4096
    /// bytes (this batch raised that bound after the clamp ate the tile
    /// attachment disclosure): its text must be a clamped prefix of ours,
    /// never a different story, and every other field must survive the same
    /// store normalization byte for byte.
    fn assert_head_equivalent(
        current: &crate::recipe::EditRecipe,
        head: &crate::recipe::EditRecipe,
        raw: &std::path::Path,
        what: &str,
    ) {
        let mut current = current.clone();
        let mut head = head.clone();
        let current_rationale = std::mem::take(&mut current.rationale);
        let head_rationale = std::mem::take(&mut head.rationale);
        assert!(
            current_rationale.starts_with(&head_rationale)
                && (current_rationale.len() == head_rationale.len()
                    || head_rationale.len() >= 4096 - 4),
            "{what}: the pre-batch rationale is not a clamped prefix \
             (head {} bytes, current {} bytes)",
            head_rationale.len(),
            current_rationale.len(),
        );
        assert_eq!(
            normalized_persisted_recipe(&current, raw),
            normalized_persisted_recipe(&head, raw),
            "{what}: disabled layers differ from the pre-batch executable",
        );
    }

    fn normalized_persisted_recipe(
        recipe: &crate::recipe::EditRecipe,
        raw: &std::path::Path,
    ) -> Vec<u8> {
        let bytes = crate::pipeline::recipe_store_bytes(raw, recipe, crate::diag::stderr()).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if let Some(masks) = value.get_mut("masks").and_then(serde_json::Value::as_array_mut) {
            for (index, adjustment) in masks.iter_mut().enumerate() {
                if let Some(mask) = adjustment.get_mut("mask")
                    && mask.get("kind").and_then(serde_json::Value::as_str) == Some("bitmap")
                    && let Some(path) = mask.get_mut("path")
                {
                    *path = serde_json::Value::String(format!("<bitmap-{index}>"));
                }
            }
        }
        serde_json::to_vec(&value).unwrap()
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

    #[test]
    fn semantic_regions_select_independent_modes_and_worst_confidence() {
        let (w, h) = (12u32, 4u32);
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
            image::Rgb([80 + (x * 3) as u8, 100, 120])
        }));
        let target = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
            image::Rgb([if x < 4 { 180 } else if x < 8 { 60 } else { 130 }, 100, 120])
        }));
        let dir = std::env::temp_dir().join(format!("autoshade-semantic-product-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut regions = Vec::new();
        let mut rasters = Vec::new();
        for (index, class_id) in [10u16, 20, 30].into_iter().enumerate() {
            let mask = GrayImage::from_fn(w, h, |x, _| {
                image::Luma([if x / 4 == index as u32 { 255 } else { 0 }])
            });
            let path = crate::store::OwnedRaster::scratch(dir.join(format!("region-{class_id}.png")));
            mask.save(path.path()).unwrap();
            regions.push(semantic::SemanticRegion {
                class_id,
                label: format!("class-{class_id}"),
                mean_confidence: 0.9,
                source: mask.clone(),
                target: mask,
                source_share: 1.0 / 3.0,
                target_share: 1.0 / 3.0,
            });
            rasters.push(path);
        }
        let mut report = neutral_report(&source, &target);
        let divergences = [0.1f32, 0.8, 0.2]
            .into_iter()
            .map(|d| ZoneDivergence { divergence: divergence(d), share: 1.0 / 3.0 })
            .collect::<Vec<_>>();
        let global_confidence = report.recipe.confidence;
        attach_semantic_regions(&source, &target, &mut report, &regions, &rasters, &divergences);
        assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_MODE_FULL));
        assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE));
        let residuals = report.notes.iter().filter(|n| n.key == crate::rationale::keys::ZONE_ATTACHED)
            .filter_map(|n| n.args.iter().find(|(name, _)| *name == "after").and_then(|(_, value)| value.parse::<f32>().ok()))
            .collect::<Vec<_>>();
        if !residuals.is_empty() {
            let worst = semantic::worst_region_residual(&residuals);
            let expected = global_confidence.min(fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE));
            assert!((report.recipe.confidence - expected).abs() <= 1e-6,
                "confidence did not use the worst accepted region: got {}, expected {}",
                report.recipe.confidence, expected);
            let disclosed = report.notes.iter()
                .find(|n| n.key == crate::rationale::keys::ZONE_CONFIDENCE)
                .and_then(|n| n.args.iter().find(|(name, _)| *name == "worst"))
                .and_then(|(_, value)| value.parse::<f32>().ok())
                .expect("semantic confidence must disclose the worst accepted region");
            assert!((disclosed - worst).abs() <= 0.001, "confidence disclosure used {disclosed}, expected worst {worst}");
        }
        for raster in rasters { raster.remove(); }
        let _ = std::fs::remove_dir_all(dir);
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
        let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
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
        let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
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
                let (s_img, t_img) = fit::analysis_pair(&source, &target);
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

    /// One luma bin, two populations, built at the analysis size so no
    /// thumbnail resampling blends its edges. The top two thirds are REPLACED
    /// content whose source ramp lives entirely in luma bin 6 (0.353-0.412);
    /// the ground is a near-flat 0.34-0.40 ramp -- identical on both sides,
    /// then +0.08 brighter on the target -- whose upper part shares that bin.
    /// Frame-wide the bin keeps well under 35% structural survival and is
    /// withheld; the ground alone keeps all of it.
    fn poisoned_bin_fixture() -> (DynamicImage, DynamicImage, GrayImage) {
        let (w, h) = (fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let sky_rows = h * 2 / 3;
        let build = |target: bool| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                let v: f32 = if y < sky_rows {
                    if target {
                        if (y / 8) % 2 == 0 { 0.05 } else { 0.15 }
                    } else {
                        0.36 + 0.04 * x as f32 / (w - 1) as f32
                    }
                } else {
                    let ground = 0.34 + 0.06 * x as f32 / (w - 1) as f32;
                    if target { ground + 0.08 } else { ground }
                };
                image::Rgb([(v.clamp(0.0, 1.0) * 255.0).round() as u8; 3])
            }))
        };
        let sky_mask = GrayImage::from_fn(w, h, |_, y| {
            image::Luma([if y < sky_rows { 255u8 } else { 0 }])
        });
        (build(false), build(true), sky_mask)
    }

    #[test]
    fn a_zone_is_judged_by_its_own_members_not_the_frames_bins() {
        let (src, tgt, sky_mask) = poisoned_bin_fixture();
        let sp = fit::pixels_of(&src);
        let tp = fit::pixels_of(&tgt);
        let evidence = fit::evidence_model(&sp, &tp);
        let sky = mask_weights(&sky_mask, src.width(), src.height());
        let ground: Vec<f32> = sky.iter().map(|w| 1.0 - w).collect();
        let unsupported = evidence.spatial_supported.iter().filter(|&&s| !s).count();
        assert_eq!(
            unsupported,
            (src.width() * src.height() * 2 / 3) as usize,
            "premise: exactly the replaced sky is structurally unsupported"
        );
        let frame_bin = &evidence.luma[6];
        assert!(
            frame_bin.source_populated && frame_bin.weight <= 0.0,
            "premise: frame-wide bin 6 is populated yet withheld: {frame_bin:?}"
        );
        let ground_view = evidence.scoped(&tp, &ground, &ground);
        assert!(
            ground_view.luma[6].weight > 0.0,
            "the ground's own bin 6 must carry evidence: {:?}",
            ground_view.luma[6]
        );
        assert!(ground_view.luma[5].weight > 0.0, "{:?}", ground_view.luma[5]);
        let sky_view = evidence.scoped(&tp, &sky, &sky);
        assert!(
            sky_view.luma[6].source_populated && sky_view.luma[6].weight <= 0.0,
            "the sky's own bin 6 stays withheld: {:?}",
            sky_view.luma[6]
        );
        // Over the whole frame the scoped view IS the frame model, bit for bit.
        let ones = vec![1.0f32; sp.len()];
        let frame_view = evidence.scoped(&tp, &ones, &ones);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(frame_view.luma, evidence.luma);
        assert_eq!(frame_view.hue, evidence.hue);
        assert_eq!(bits(&frame_view.source_weights), bits(&evidence.source_weights));
        assert_eq!(bits(&frame_view.target_weights), bits(&evidence.target_weights));
        assert_eq!(bits(&frame_view.source_hue_weights), bits(&evidence.source_hue_weights));
        assert_eq!(bits(&frame_view.target_hue_weights), bits(&evidence.target_hue_weights));
        assert_eq!(frame_view.identifiability.to_bits(), evidence.identifiability.to_bits());
        assert_eq!(frame_view.population.to_bits(), evidence.population.to_bits());
        assert_eq!(evidence.population, evidence.source_pixels.len() as f32);
    }

    /// The ground zone's tone move touches only ground pixels, all of them
    /// structurally supported; the frame-wide verdict would still have vetoed
    /// it through the sky-poisoned bin. Judged by its own members it attaches
    /// with a real exposure move and no tone refusal.
    #[test]
    fn a_ground_zone_is_not_vetoed_by_the_sky_it_does_not_touch() {
        let (src, tgt, sky_mask) = poisoned_bin_fixture();
        let path = fixture_mask_path("poisoned-bin-land");
        sky_mask.save(path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
        let t_px = fit::pixels_of(&t_img);
        let sw = mask_weights(&sky_mask, s_img.width(), s_img.height());
        let tw = mask_weights(&sky_mask, t_img.width(), t_img.height());
        let land = ZoneAttachment {
            source_weights: sw.iter().map(|w| 1.0 - w).collect(),
            target_weights: tw.iter().map(|w| 1.0 - w).collect(),
            coverage: None,
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            range: None,
            name: String::new(),
            role: MaskRole::ZoneLand,
            inverted: true,
            label: MaskRole::ZoneLand.tag().to_string(),
            min_share: MIN_ZONE_SHARE,
            frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
        };
        let before = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let mut frame_err = report.err_after;
        let accepted = attach_one_zone(
            &s_img,
            &t_px,
            &mut report,
            &mut frame_err,
            &land,
            measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &sky_mask)
                .land
                .divergence,
            None,
        );
        assert!(accepted.is_some(), "the ground zone must attach: {}", report.recipe.rationale);
        let zone = report.recipe.masks.last().expect("attached land mask");
        assert!(zone.exposure_ev > 0.0, "a real tone move must survive: {zone:?}");
        assert!(
            !report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE),
            "the ground zone must not be vetoed through the sky's bins: {}",
            report.recipe.rationale
        );
        // Premise, stated by the frame-wide model itself: this very move would
        // have been vetoed through the replaced sky's identical luma bins.
        let after = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        assert!(
            fit::moved_unsupported_luma_range_names(&before, &after, &report.evidence).is_some(),
            "premise: the frame-wide verdict names the poisoned bin for this move"
        );
        path.remove();
    }

    /// The calibration land is a Full zone inside an Atmosphere frame. Its own
    /// rerendered mid-tones retain only 10-33% structural survival, so the Full
    /// zone must read the carried structural model and withhold those ranges;
    /// the frame's blind ruler would allow them.
    #[test]
    fn calibration_land_zone_is_withheld_by_its_own_rerendered_mid_tones() {
        let Some(root) = fit::calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
        let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
        let sky_mask = image::open(root.join("sky-mask.png"))
            .expect("calibration sky-mask.png")
            .to_luma8();
        let mask_path = fixture_mask_path("calibration-land-scratch");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&source, &target);
        assert_eq!(report.mode, fit::FitMode::Atmosphere);
        assert!(report.structural_evidence.is_some());
        attach_zones(&source, &target, &mut report, &sky_mask, &sky_mask, &mask_path);
        let land_tag = MaskRole::ZoneLand.tag();
        let note = report
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE
                    && note.args.iter().any(|(key, value)| {
                        *key == "label" && value == land_tag
                    })
            })
            .expect("the land Full zone must withhold its structurally unsupported tone move");
        let ranges = note
            .args
            .iter()
            .find(|(key, _)| *key == "luma_ranges")
            .map(|(_, value)| value.as_str())
            .expect("land tone note carries luma_ranges");
        for expected in [
            "luma[0.29-0.35]",
            "luma[0.35-0.41]",
            "luma[0.41-0.47]",
            "luma[0.47-0.53]",
            "luma[0.53-0.59]",
        ] {
            assert!(ranges.contains(expected), "land note missed {expected}: {ranges}");
        }
        assert!(
            !["luma[0.59-0.65]", "luma[0.65-0.71]", "luma[0.71-0.76]", "luma[0.76-0.82]"]
                .iter()
                .any(|range| ranges.contains(range)),
            "the land note inherited the sky's bright bins: {ranges}"
        );

        let (s_img, t_img) = fit::analysis_pair(&source, &target);
        let tgt_px = fit::pixels_of(&t_img);
        let land_source = mask_weights(&sky_mask, s_img.width(), s_img.height())
            .iter()
            .map(|weight| 1.0 - weight)
            .collect::<Vec<_>>();
        let land_target = mask_weights(&sky_mask, t_img.width(), t_img.height())
            .iter()
            .map(|weight| 1.0 - weight)
            .collect::<Vec<_>>();
        let blind_land = report.evidence.scoped(&tgt_px, &land_source, &land_target);
        let structural_land = report
            .structural_evidence
            .as_ref()
            .unwrap()
            .scoped(&tgt_px, &land_source, &land_target);
        for label in [
            "luma[0.29-0.35]",
            "luma[0.35-0.41]",
            "luma[0.41-0.47]",
            "luma[0.47-0.53]",
            "luma[0.53-0.59]",
        ] {
            let blind = blind_land
                .luma
                .iter()
                .find(|range| range.label == label)
                .unwrap_or_else(|| panic!("unknown land range {label}"));
            let structural = structural_land
                .luma
                .iter()
                .find(|range| range.label == label)
                .unwrap();
            assert!(blind.weight > 0.0, "blind scope would also withhold {label}");
            assert_eq!(structural.weight, 0.0, "structural scope did not withhold {label}");
        }
        mask_path.remove();
    }

    /// With its colour class withheld a zone is judged on tone alone -- so the
    /// skip line is asked of tone alone too. A zone whose luma already matches
    /// is left alone with the honest note instead of being dialled for a
    /// hairline tone gain against a chroma gap it may not touch.
    #[test]
    fn a_zone_whose_movable_class_already_matches_is_left_alone() {
        let (w, h) = (16u32, 16u32);
        let build = |sky: [f32; 3]| -> DynamicImage {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |_, y| {
                let p = if y >= 12 { sky } else { [0.55f32, 0.45, 0.35] };
                image::Rgb(p.map(|c| (c * 255.0).round() as u8))
            }))
        };
        // Same luma, opposite hue: the blue sky's band exists only on the
        // source side and the warm target's only on the target side.
        let src = build([0.60, 0.63, 0.67]);
        let tgt = build([0.67, 0.62, 0.59]);
        let sky_mask =
            GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]));
        let path = fixture_mask_path("movable-class-matched");
        sky_mask.save(path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
        let t_px = fit::pixels_of(&t_img);
        let sw = mask_weights(&sky_mask, s_img.width(), s_img.height());
        let tw = mask_weights(&sky_mask, t_img.width(), t_img.height());
        let attachment = semantic_attachment(sw, tw, &path);
        let mut frame_err = report.err_after;
        let accepted = attach_one_zone(
            &s_img,
            &t_px,
            &mut report,
            &mut frame_err,
            &attachment,
            measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &sky_mask)
                .sky
                .divergence,
            None,
        );
        assert!(
            report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR),
            "premise: the one-sided hue withholds colour: {}",
            report.recipe.rationale
        );
        assert!(
            accepted.is_none() && report.recipe.masks.is_empty(),
            "a tone-matched zone must not be dialled: {}",
            report.recipe.rationale
        );
        assert!(
            report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_ALREADY_MATCHED),
            "the movable class already matches and must say so: {}",
            report.recipe.rationale
        );
        path.remove();
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
        assert!((-0.15..=-0.12).contains(&sky.exposure_ev));
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
        let colour_note = report
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR
                    && note.args.iter().any(|(key, value)| {
                        *key == "label" && value == MaskRole::ZoneSky.tag()
                    })
            })
            .expect("the one-sided calibration sky band must refuse colour");
        assert!(
            colour_note
                .args
                .iter()
                .any(|(key, value)| *key == "hue_bands" && value.contains("Aqua"))
        );
        assert!(!report.notes.iter().any(|note| {
            note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE
                && note.args.iter().any(|(key, value)| {
                    *key == "label" && value == MaskRole::ZoneSky.tag()
                })
        }));
        mask_path.remove();
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
            python_bin: "autoshade-test-no-such-python".into(),
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
        assert!(
            report.notes.iter().any(|note| note.key == crate::rationale::keys::ZONED_UNAVAILABLE),
            "typed fallback verdict must name segmentation unavailability",
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

    enum PlaneSource {
        Input,
        Black,
    }

    /// A loopback stand-in for BOTH segmentation bridges. The single-class
    /// call gets its input copied back as the mask (argv 4 → 6); the
    /// `--multi` call (argv 9) gets one "sky" plane beside the manifest —
    /// the input again, or an all-black fixture — and a manifest that names
    /// it. `width`/`height` are what the test EXPECTS the bridge to have
    /// handed over: the bridge's own dimension check turns a wrong sizing
    /// into a refusal instead of a silently different plane.
    fn loopback_segment_opts(
        dir: &std::path::Path,
        plane: PlaneSource,
        width: u32,
        height: u32,
    ) -> SegmentOpts {
        let black = dir.join("black.png");
        GrayImage::from_pixel(width, height, image::Luma([0u8])).save(&black).unwrap();
        let manifest = |name: &str| {
            format!(
                "{{\"version\":1,\"width\":{width},\"height\":{height},\"planes\":[{{\"class_id\":2,\
                 \"label\":\"sky\",\"mean_confidence\":0.5,\"share\":0.5,\"path\":\"{name}\"}}]}}"
            )
        };
        // sh reads the plane name from the manifest path at run time via a
        // template; batch expands `%~n6` inline.
        std::fs::write(dir.join("manifest.tmpl"), manifest("PLANE")).unwrap();
        let (bat_plane, sh_plane) = match plane {
            PlaneSource::Input => ("%4".to_string(), "$4".to_string()),
            PlaneSource::Black => ("%~dp0black.png".to_string(), black.display().to_string()),
        };
        let bat = format!(
            "@echo off\r\nif \"%9\"==\"--multi\" (\r\n  copy /y \"{bat_plane}\" \"%~dpn6.class-2.png\" >nul\r\n  \
             echo {}>\"%6\"\r\n) else (\r\n  copy /y \"%4\" \"%6\" >nul\r\n)\r\nexit /b 0\r\n",
            manifest("%~n6.class-2.png")
        );
        let sh = format!(
            "if [ \"$9\" = \"--multi\" ]; then\n  stem=$(basename \"$6\" .json)\n  \
             cp \"{sh_plane}\" \"$(dirname \"$6\")/$stem.class-2.png\"\n  \
             sed \"s/PLANE/$stem.class-2.png/\" \"{}\" > \"$6\"\nelse\n  cp \"$4\" \"$6\"\nfi\nexit 0\n",
            dir.join("manifest.tmpl").display()
        );
        let python_bin = crate::write_stand_in(dir, "segment-stub", &bat, &sh);
        // The script must merely exist — the bridge refuses a missing one.
        let script = dir.join("segment.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        SegmentOpts {
            python_bin,
            script,
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        }
    }

    /// `segmentation_input` is the one sizing rule: borrow through 2048 px on
    /// the long edge (2048 itself included), thumbnail above it.
    #[test]
    fn segmentation_input_downscales_only_above_the_edge() {
        let frame = |w, h| DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([9, 9, 9])));
        assert!(matches!(segmentation_input(&frame(1600, 1067)), std::borrow::Cow::Borrowed(_)));
        assert!(matches!(segmentation_input(&frame(2048, 1365)), std::borrow::Cow::Borrowed(_)));
        let big = frame(2400, 1600);
        let large = segmentation_input(&big);
        assert!(matches!(large, std::borrow::Cow::Owned(_)));
        assert_eq!(large.dimensions(), (2048, 1365));
    }

    /// The two bridges hand the sidecar identical inputs — the Rust-layer
    /// falsifier behind the seeded legacy run's byte identity. The
    /// multi-class bridge used to thumbnail unconditionally, and
    /// `image::thumbnail` UPSCALES a smaller frame, so on every ≤2048 frame
    /// (the calibration corpus is 1600 px) its sky plane differed from the
    /// single-class mask while the Python-layer identity test, which fed both
    /// modes the same file, stayed green. The stand-ins copy their input back
    /// as mask/plane, so the bytes each bridge returns ARE what it sent.
    #[test]
    fn multi_and_single_class_inputs_are_prepared_identically() {
        for (tag, w, h, ew, eh) in [
            ("seg-input-native", 300u32, 200u32, 300u32, 200u32),
            ("seg-input-large", 2400, 1600, 2048, 1365),
        ] {
            let dir = crate::test_dir(tag);
            let seg = loopback_segment_opts(&dir, PlaneSource::Input, ew, eh);
            let frame = |seed: u8| {
                DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                    image::Rgb([
                        seed.wrapping_add((x % 7) as u8 * 9),
                        60u8.wrapping_add((y % 11) as u8 * 13),
                        128,
                    ])
                }))
            };
            let (src, tgt) = (frame(40), frame(90));
            let legacy_path = crate::store::OwnedRaster::scratch(dir.join("legacy.png"));
            let multi_path = crate::store::OwnedRaster::scratch(dir.join("multi.png"));
            let (sm, tm) = segment_both(&src, &tgt, &seg, &legacy_path)
                .unwrap_or_else(|e| panic!("{tag}: single-class bridge: {e:#}"));
            let (regions, rasters, (ms, mt), _refinements) =
                segment_multiclass_both(&src, &tgt, &seg, &multi_path, 4)
                    .unwrap_or_else(|e| panic!("{tag}: multi-class bridge: {e:#}"));
            assert_eq!(sm.dimensions(), (ew, eh), "{tag}: single-class input sizing");
            assert_eq!(ms.dimensions(), (ew, eh), "{tag}: multi-class input sizing");
            assert!(
                sm.as_raw() == ms.as_raw() && tm.as_raw() == mt.as_raw(),
                "{tag}: the two bridges handed the sidecar different bytes"
            );
            assert_eq!(regions.len(), 1, "{tag}: the one plane pairs into one region");
            for raster in rasters {
                raster.remove();
            }
            legacy_path.remove();
            multi_path.remove();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The multi-class layer failing is not the sky fit failing. With a broken
    /// interpreter the multi bridge fails, the historical route (stubbed masks)
    /// succeeds, and the report is that route's report plus ONE typed
    /// `SEMANTIC_REGIONS_UNAVAILABLE` note — never `ZONED_UNAVAILABLE`, whose
    /// text promises a luminance-range fallback that did not run.
    #[test]
    fn multi_segmentation_failure_keeps_the_legacy_zones_with_its_own_note() {
        let (src, tgt, sky) = zoned_pair();
        // An ABSOLUTE interpreter path, because that is the real shape: the
        // bundled helper resolves one, and `AUTOSHADE_PYTHON` is one. A bare
        // name produces a sidecar error with no path in it, and a disclosure
        // test written against that fixture cannot fail — which is how the
        // first version of the guard below passed against its own mutation.
        // Derived from `temp_dir`, never written as a literal.
        let missing = std::env::temp_dir()
            .join("autoshade-e-leak-probe")
            .join("no-such-python");
        let seg = SegmentOpts {
            python_bin: missing.to_string_lossy().into_owned(),
            script: "Cargo.toml".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let path = fixture_mask_path("multi-unavailable");
        sky.save(path.path()).unwrap();
        let base = crate::recipe::EditRecipe::default();
        SEGMENT_BOTH_OVERRIDE.with(|v| *v.borrow_mut() = Some((sky.clone(), sky.clone())));
        let multi = fit_recipe_zoned_with_regions(&src, &tgt, &seg, &path, &base, fit::FitOptions::default(), 4);
        SEGMENT_BOTH_OVERRIDE.with(|v| *v.borrow_mut() = Some((sky.clone(), sky.clone())));
        let legacy = fit_recipe_zoned_with_regions(&src, &tgt, &seg, &path, &base, fit::FitOptions::default(), 2);
        let own = multi
            .notes
            .iter()
            .filter(|n| n.key == crate::rationale::keys::SEMANTIC_REGIONS_UNAVAILABLE)
            .collect::<Vec<_>>();
        assert_eq!(own.len(), 1, "exactly one typed hand-off: {}", multi.recipe.rationale);
        // …and the hand-off's reason went through the disclosure door. The
        // sidecar's own error names paths; a rationale is user-visible and is
        // pasted into bug reports, so neither an absolute path nor an unbounded
        // traceback may reach it.
        let reason = own[0].args.iter().find(|(k, _)| *k == "e").map(|(_, v)| v.as_str());
        let reason = reason.expect("the hand-off note carries its reason");
        assert!(
            !reason.contains("autoshade-e-leak-probe") && !reason.contains('\n'),
            "the hand-off reason leaked this machine's layout or a multi-line trace: {reason}"
        );
        assert!(
            reason.contains("no-such-python"),
            "…while still SAYING which program could not be launched: {reason}"
        );
        assert!(
            reason.chars().count() <= 160,
            "the hand-off reason is unbounded ({} chars): {reason}",
            reason.chars().count()
        );
        assert!(
            !multi.notes.iter().any(|n| n.key == crate::rationale::keys::ZONED_UNAVAILABLE)
                && !multi.recipe.rationale.contains("luminance-range fallback"),
            "the sky/land route ran; no range fallback may be narrated: {}",
            multi.recipe.rationale
        );
        // Everything but that one appended sentence IS the historical route.
        let appended = crate::rationale::render_one(own[0]);
        assert_eq!(
            multi.recipe.rationale.strip_suffix(appended.as_str()),
            Some(legacy.recipe.rationale.as_str()),
            "the hand-off note is appended to the historical rationale, nothing else changes"
        );
        let (mut a, mut b) = (multi.recipe.clone(), legacy.recipe.clone());
        a.rationale.clear();
        b.rationale.clear();
        assert_eq!(serde_json::to_vec(&a).unwrap(), serde_json::to_vec(&b).unwrap());
        assert_eq!(multi.err_after.to_bits(), legacy.err_after.to_bits());
        path.remove();
    }

    /// A manifest whose every plane misses the support floor resolves to NO
    /// region. That is a hand-off to the SEEDED historical route — which
    /// judges the sky partition on its own numbers, drops its anchor and runs
    /// the sequencer — plus one typed `SEMANTIC_REGIONS_NONE` note; not a
    /// fourth exit that rendered `{s}` placeholders and stranded the anchor.
    #[test]
    fn empty_semantic_region_set_hands_off_to_the_seeded_legacy_route() {
        let dir = crate::test_dir("seg-no-region");
        let (src, tgt, _) = zoned_pair();
        let seg = loopback_segment_opts(&dir, PlaneSource::Black, src.width(), src.height());
        let path = crate::store::OwnedRaster::scratch(dir.join("anchor.png"));
        let report = fit_recipe_zoned_with_regions(
            &src, &tgt, &seg, &path, &crate::recipe::EditRecipe::default(), fit::FitOptions::default(), 4,
        );
        assert!(
            report.notes.iter().any(|n| n.key == crate::rationale::keys::SEMANTIC_REGIONS_NONE
                && n.args.iter().any(|(k, v)| *k == "n" && v == "4")),
            "typed hand-off naming the requested count: {}",
            report.recipe.rationale
        );
        let no_partition = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::ZONED_NO_PARTITION)
            .unwrap_or_else(|| panic!("the historical route judges the partition: {}", report.recipe.rationale));
        assert!(no_partition.args.iter().any(|(k, _)| *k == "s"));
        assert!(
            report.recipe.rationale.contains("sky covers 0% of the source frame")
                && !report.recipe.rationale.contains("{s}"),
            "numbers, not placeholders: {}",
            report.recipe.rationale
        );
        assert!(!path.path().exists(), "the anchor raster must not outlive a failed partition");
        assert!(report.recipe.masks.iter().all(|m| match &m.mask {
            MaskGeometry::Bitmap { path } => !path.contains("mask-region-"),
            _ => true,
        }));
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("multi") || n.contains("mask-region"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "sidecar inputs/planes leaked: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The release rule behind arbitration, on synthetic reports: the loser's
    /// own rasters go, a raster both reports share stays, the anchor goes
    /// only when nothing kept references it, and a raster outside the store
    /// is never touched. The loopback arbitration test below exercises the
    /// call sites; this pins the rule itself on every branch.
    #[test]
    fn release_unselected_rasters_keeps_exactly_what_the_kept_recipe_references() {
        let dir = crate::test_dir("release-unselected");
        let outside = crate::test_dir("release-unselected-outside");
        let file = |d: &std::path::Path, n: &str| {
            let p = d.join(n);
            std::fs::write(&p, b"x").unwrap();
            p.to_string_lossy().into_owned()
        };
        let report_with = |paths: &[&str]| {
            let mut report = neutral_report(
                &DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([90, 90, 90]))),
                &DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([110, 110, 110]))),
            );
            for p in paths {
                report.recipe.masks.push(crate::recipe::LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: (*p).to_string() },
                    role: MaskRole::Custom,
                    ..Default::default()
                });
            }
            report
        };
        let anchor = crate::store::OwnedRaster::scratch(dir.join("anchor.png"));
        std::fs::write(anchor.path(), b"x").unwrap();
        let (loser_only, shared, foreign) = (
            file(&dir, "mask-region-2-sky.png"),
            file(&dir, "mask-zone-tile.png"),
            file(&outside, "mask-region-9-far.png"),
        );
        // Branch 1: the candidate loses, the kept report references the
        // shared raster and the anchor.
        let candidate = report_with(&[&loser_only, &shared, &foreign]);
        let kept = report_with(&[&shared, &anchor.path().to_string_lossy()]);
        release_unselected_rasters(&candidate, &kept, &anchor);
        assert!(!std::path::Path::new(&loser_only).exists(), "the loser's own raster is released");
        assert!(std::path::Path::new(&shared).exists(), "a raster the kept recipe references stays");
        assert!(anchor.path().exists(), "the anchor stays while the kept recipe references it");
        assert!(std::path::Path::new(&foreign).exists(), "a raster outside the store is never touched");
        // Branch 2: nothing kept references the anchor — it goes too.
        let kept = report_with(&[&shared]);
        release_unselected_rasters(&candidate, &kept, &anchor);
        assert!(!anchor.path().exists(), "an unreferenced anchor is released");
        assert!(std::path::Path::new(&shared).exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// The multi arm's arbitration and claim hygiene, deterministically. With
    /// the loopback sidecar both routes see the SAME sky plane (the frame's
    /// luma), so the seeded two-region run inside the multi arm is exactly an
    /// independent unseeded one. A refusal must hand that report back byte
    /// for byte plus ONE note — never a transplant — with every region
    /// raster gone; a win must beat it on the shared ruler. Either way no
    /// claim outlives the recipe that references it and no sidecar file
    /// survives the run.
    #[test]
    fn loopback_multi_run_arbitrates_against_the_seeded_two() {
        let (src, tgt, _) = zoned_pair();
        let base = crate::recipe::EditRecipe::default();
        let run = |tag: &str, regions: usize| {
            let dir = crate::test_dir(tag);
            let seg = loopback_segment_opts(&dir, PlaneSource::Input, src.width(), src.height());
            let anchor = crate::store::OwnedRaster::scratch(dir.join("anchor.png"));
            let report = fit_recipe_zoned_with_regions(&src, &tgt, &seg, &anchor, &base, fit::FitOptions::default(), regions);
            (dir, report)
        };
        let (four_dir, four) = run("seg-arb-four", 4);
        let (two_dir, two) = run("seg-arb-two", 2);
        let files = |dir: &std::path::Path| {
            std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let referenced = |report: &FitReport| {
            report.recipe.masks.iter().filter_map(|m| match &m.mask {
                MaskGeometry::Bitmap { path } => std::path::Path::new(path)
                    .file_name().map(|n| n.to_string_lossy().into_owned()),
                _ => None,
            }).collect::<std::collections::HashSet<_>>()
        };
        // Claim hygiene holds on both branches: every mask raster on disk is
        // referenced by the recipe, every referenced raster exists, and no
        // sidecar input/manifest/plane survived the run.
        let on_disk = files(&four_dir);
        let refs = referenced(&four);
        for name in &on_disk {
            assert!(!name.contains(".multi"), "sidecar file leaked: {name}");
            if name.starts_with("mask-") || name == "anchor.png" {
                assert!(refs.contains(name), "orphan claim {name}; recipe references {refs:?}");
            }
        }
        for name in &refs {
            assert!(on_disk.contains(name), "referenced raster {name} missing from {on_disk:?}");
        }
        let refusals = four.notes.iter()
            .filter(|n| n.key == crate::rationale::keys::REGION_FRAME_REFUSED)
            .collect::<Vec<_>>();
        if let [refusal] = refusals[..] {
            // The reference report, byte for byte, plus exactly the one note.
            assert_eq!(
                four.recipe.rationale,
                format!("{}{}", two.recipe.rationale, crate::rationale::render_one(refusal)),
                "a refusal appends one note to the reference rationale and transplants nothing"
            );
            let normalise = |r: &FitReport, dir: &std::path::Path| {
                let mut recipe = r.recipe.clone();
                recipe.rationale.clear();
                serde_json::to_string(&recipe).unwrap().replace(&dir.to_string_lossy().replace('\\', "\\\\"), "<dir>")
                    .replace(&dir.to_string_lossy().into_owned(), "<dir>")
            };
            assert_eq!(normalise(&four, &four_dir), normalise(&two, &two_dir));
            assert_eq!(four.err_after.to_bits(), two.err_after.to_bits());
            assert!(refs.iter().all(|n| !n.starts_with("mask-region-")), "refused regions must not ship: {refs:?}");
        } else {
            assert!(refusals.is_empty(), "at most one arbitration note: {}", four.recipe.rationale);
            let ruler = &two.evidence;
            assert!(
                frame_err_under(&src, &tgt, &four, ruler) < frame_err_under(&src, &tgt, &two, ruler),
                "a kept multi result beats the reference on the reference's own ruler"
            );
        }
        let _ = std::fs::remove_dir_all(&four_dir);
        let _ = std::fs::remove_dir_all(&two_dir);
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

}
