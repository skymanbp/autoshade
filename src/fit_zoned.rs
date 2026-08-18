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

use std::path::Path;

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage};

use crate::fit::{self, FitReport};
use crate::recipe::{LocalAdjustment, MaskGeometry, MaskRole};
use crate::render;
use crate::segment::{segment_file, SegmentOpts};

/// A zone must cover at least this weighted share of ITS frame on BOTH sides
/// to carry trustworthy moments (a real sky measures 10–40%; segmentation
/// misses and boundary slivers sit far below).
pub(crate) const MIN_ZONE_SHARE: f32 = 0.03;
/// Conservative local-exposure budget: ±2.5 EV covers any real sky-to-sky
/// brightness gap; a larger demand means the zones do not correspond.
const ZONE_EV_LIMIT: f32 = 2.5;
/// Local saturation shares the global fit's model cap (fit.rs stage 3).
const ZONE_SAT_LIMIT: f32 = 60.0;
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
// A SECOND reading of "how far apart do these two renders look", independent
// of `fit::look_err` — which is simultaneously the global fit's optimisation
// target, its acceptance gate and its reported score, so it can and does
// declare victory on its own terms (the metric self-reference this round
// exists to break). Built on THIS module's machinery rather than beside it:
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
// worth nothing. look_err's tonal term is 21 luma quantiles, so a plain
// luminance-band comparison IS a coarser copy of it. But its colour term is
// three UNCONDITIONAL channel means, and its hue term buckets by HUE with
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
// Calibrated on this repo's fixture set and NOWHERE inherited from the
// sky/land zones (their four constants each carry a measured real-pair
// anchor for a DIFFERENT quantity and must not be borrowed — R19). Every
// number below is pinned by `joint_family_is_calibrated_on_the_fixture_set`,
// which also records the whole measurement table.
//
// STATUS (R24 batch 2, 2026-08-17): NO LONGER PROVISIONAL. R23-6 set these
// against synthetic fixtures alone and recorded the debt in this very block
// ("wants a real-pair review before anyone treats a number here as measured
// truth"). Six real (RAW, finished JPEG) pairs off the user's own library —
// EXIF-timestamp-confirmed same frame — have since been measured through
// `autoshop match`, and they moved BOTH ends of the ladder. The table is
// `fit::tests::joint_family_is_calibrated_on_the_fixture_set`'s doc; the
// finding that forced the retune is that the ONE pair the user called
// nonsense (an astro composite: the Milky Way gone, the deep blue turned
// grey) read 0.141 — under the old 0.25 line, so it raised no warning and
// still reported 0.58 confidence.
//
// The separation the fixtures established is unchanged and still holds: a
// fit that REACHES its target lands the weighted reading at 0.001-0.06,
// while the pair whose target is a repaint the global model structurally
// cannot reach lands at 0.58. The real pairs simply showed where inside
// that gap the line actually belongs.

/// The weighted reading at which reported confidence hits its floor — the
/// joint family's counterpart of `fit`'s own FAR line, and the other end of
/// the same calibration as [`JOINT_CONFIDENCE_SLOPE`].
///
/// 0.10, distilled from eleven measured pairs (six real, five fixture) that
/// bracket it from both sides:
///   * ABOVE the line, and the reason it moved: the real astro-composite
///     pair reads 0.141 and MUST warn (it was 0.578 confidence and silent at
///     0.25 — the defect this retune closes), and the unreachable synthetic
///     repaint reads 0.581, 5.8× over.
///   * BELOW it, every fit that reached its target: the five honest real
///     pairs at 0.019/0.024/0.030/0.035/0.054 (worst = 1.85× under) and the
///     three fixtures that land at 0.001/0.004/0.045 (worst = 2.2× under).
///   * The TIGHTEST constraint, and the one to re-open first when a second
///     real failure pair arrives: the two fixtures where the solver
///     correctly REFUSED to chase a whole-scene regrade (canyon warm 0.061,
///     canyon gold 0.093). Those are policy refusals, not misses, so they
///     must not be accused of being far — and 0.093 leaves only 8% of
///     headroom. The line is deliberately biased the other way (41% of
///     headroom over the real failure): a real pair the user called nonsense
///     outranks a synthetic fixture whose silence is a judgement call.
pub(crate) const JOINT_FAR_ERR: f32 = 0.10;
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

/// Every bucket of the joint family that carries evidence on BOTH sides, in
/// [`JOINT_LABELS`] order. Empty when none qualifies or the family is off —
/// see [`joint_reading`] for what "empty" is allowed to mean.
pub fn joint_buckets(cand: &[[f32; 3]], tgt: &[[f32; 3]]) -> Vec<JointBucket> {
    if !joint_family_enabled() {
        return Vec::new();
    }
    let mut wa: Vec<f32> = Vec::new();
    let mut wb: Vec<f32> = Vec::new();
    let mut out = Vec::with_capacity(JOINT_BUCKETS);
    for (b, label) in JOINT_LABELS.iter().enumerate() {
        joint_weights(cand, b, &mut wa);
        joint_weights(tgt, b, &mut wb);
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
    if buckets.is_empty() {
        return None;
    }
    let mut worst = 0.0f32;
    let mut worst_label = buckets[0].label;
    let mut acc = 0.0f64;
    let mut acc_w = 0.0f64;
    for b in &buckets {
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
///   2. the confidence cap (`fit::compose_report`, i.e. every fit report and
///      every `fit::rescore_report`) — gone, so the reported confidence is the
///      look-error ladder alone;
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
    mask_path: &Path,
) -> FitReport {
    fit_recipe_zoned_from(src, target, seg, mask_path, &crate::recipe::EditRecipe::default())
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
    mask_path: &Path,
    base: &crate::recipe::EditRecipe,
) -> FitReport {
    let mut report = fit::fit_recipe_from(src, target, base);
    match segment_both(src, target, seg, mask_path) {
        Ok((src_mask, tgt_mask)) => {
            attach_zones(src, target, &mut report, &src_mask, &tgt_mask, mask_path);
        }
        Err(e) => {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONED_UNAVAILABLE,
                    vec![("e", format!("{e:#}"))],
                ),
            );
        }
    }
    report
}

/// Run the segmentation sidecar on both images. The source mask persists at
/// `mask_path` (the recipe references it); the target's inputs/mask are
/// temporary siblings, removed before returning. Any failure aborts the
/// whole zoned attempt — the caller degrades to the global fit.
fn segment_both(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &Path,
) -> Result<(GrayImage, GrayImage)> {
    let sibling = |suffix: &str| -> std::path::PathBuf {
        let mut s = mask_path.as_os_str().to_owned();
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
        segment_file(seg, &tmp_src, mask_path).context("segment source sky")?;
        segment_file(seg, &tmp_tgt, &tmp_tgt_mask).context("segment target sky")?;
        let sm = crate::render::open_mask_bounded(mask_path)
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
        std::fs::remove_file(mask_path).ok();
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
fn attach_zones(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    src_mask: &GrayImage,
    tgt_mask: &GrayImage,
    mask_path: &Path,
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
        std::fs::remove_file(mask_path).ok();
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
    let sky = attach_one_zone(
        &s_img,
        &tgt_px,
        report,
        &mut frame_err,
        &sw,
        &tw,
        mask_path,
        MaskRole::ZoneSky,
        false,
    );
    let land = attach_one_zone(
        &s_img,
        &tgt_px,
        report,
        &mut frame_err,
        &swl,
        &twl,
        mask_path,
        MaskRole::ZoneLand,
        true,
    );
    let accepted: Vec<AcceptedZone> = [sky, land].into_iter().flatten().collect();
    if accepted.is_empty() {
        std::fs::remove_file(mask_path).ok();
        return;
    }
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
}

/// A zone whose correction was kept — what [`attach_zones`] needs to report
/// on the stage as a whole.
struct AcceptedZone {
    /// The zone-local residual it landed at ([`zone_err`]).
    after: f32,
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
/// `frame_err` carries the running frame-global look distance IN and OUT and
/// is used for the bounded-drift insurance only — it is deliberately not the
/// report's own field any more (R23-6).
#[allow(clippy::too_many_arguments)] // internal seam; a struct would just rename the args
fn attach_one_zone(
    s_img: &DynamicImage,
    tgt_px: &[[f32; 3]],
    report: &mut FitReport,
    frame_err: &mut f32,
    sw: &[f32],
    tw: &[f32],
    mask_path: &Path,
    role: MaskRole,
    inverted: bool,
) -> Option<AcceptedZone> {
    // `label` drives the rationale prose; it's the zone's stable ASCII tag, so
    // the text stays English/identical regardless of the GUI's display language.
    let label = role.tag();
    let cur_px = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    let ms = zone_moments(&cur_px, sw);
    let mt = zone_moments(tgt_px, tw);
    if ms.share < MIN_ZONE_SHARE || mt.share < MIN_ZONE_SHARE {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_TOO_SMALL,
                vec![
                    ("label", label.to_string()),
                    ("s", format!("{:.0}", ms.share * 100.0)),
                    ("t", format!("{:.0}", mt.share * 100.0)),
                ],
            ),
        );
        return None;
    }
    let zone_before = zone_err(&ms, &mt);
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
    let d = fit_zone_dials(&ms, &mt);
    let round1 = |v: f32| (v * 10.0).round() / 10.0;
    let round2 = |v: f32| (v * 100.0).round() / 100.0;
    report.recipe.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
        // Identity lives in `role`, not the (empty) display name — the GUI
        // derives a localised label from the role. `name` stays default ("").
        role,
        amount: 1.0,
        inverted,
        color_gains: Some(d.color_gains.map(round2)),
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
    {
        let rp = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let s_cdf = zone_luma_cdf(&rp, sw);
        let src_iqr = fit::quantile(&s_cdf, 0.75) - fit::quantile(&s_cdf, 0.25);
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        if src_iqr >= 0.05 {
            let t_cdf = zone_luma_cdf(tgt_px, tw);
            let tone_map = |x: f32| {
                fit::quantile(&t_cdf, fit::cdf_at(&s_cdf, x).clamp(fit::P_CLIP, 1.0 - fit::P_CLIP))
            };
            let (ev, sliders) = fit::fit_tone_sliders(&tone_map);
            m.exposure_ev = round2(ev.clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT));
            m.contrast = round1(sliders[0] * 100.0);
            m.highlights = round1(sliders[1] * 100.0);
            m.shadows = round1(sliders[2] * 100.0);
            m.whites = round1(sliders[3] * 100.0);
            m.blacks = round1(sliders[4] * 100.0);
        } else {
            m.exposure_ev = round2(d.exposure_ev.clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT));
        }
    }
    // Closed-loop zone saturation on real renders (the gains change chroma
    // by themselves — only a render knows where the zone landed).
    for _ in 0..2 {
        let rp = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let zone_chroma = zone_moments(&rp, sw).chroma;
        let Some(step) = zone_sat_step(zone_chroma, mt.chroma) else { break };
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        let next = clamp_zone_sat((m.saturation + step).round());
        if next == m.saturation {
            break;
        }
        m.saturation = next;
    }
    let zoned_px = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    let zoned_err = fit::look_err(&zoned_px, tgt_px);
    let m_after = zone_moments(&zoned_px, sw);
    let zone_after = zone_err(&m_after, &mt);
    let ev_after = (mt.luma_lin.max(1e-6) / m_after.luma_lin.max(1e-6)).log2().abs();
    if zone_accepts(zone_before, zone_after, ev_after)
        && zoned_err <= *frame_err + ZONE_GLOBAL_REGRESSION_TOL
    {
        let m = report.recipe.masks.last().expect("zone mask just pushed");
        let g = m.color_gains.unwrap_or([1.0; 3]);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_ATTACHED,
                vec![
                    ("label", label.to_string()),
                    ("ev", format!("{:+.2}", m.exposure_ev)),
                    ("g0", format!("{:.2}", g[0])),
                    ("g1", format!("{:.2}", g[1])),
                    ("g2", format!("{:.2}", g[2])),
                    ("sat", format!("{:+.0}", m.saturation)),
                    ("before", format!("{zone_before:.3}")),
                    ("after", format!("{zone_after:.3}")),
                ],
            ),
        );
        // Honest composition note: when the zone covers very different frame
        // shares on the two sides, the GLOBAL distributions cannot fully
        // reconcile no matter how well the zone matches (the real pair's
        // sky: 7.5% vs 22.6%).
        let (lo, hi) = (ms.share.min(mt.share), ms.share.max(mt.share));
        if hi > 2.0 * lo {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_SHARE_MISMATCH,
                    vec![
                        ("label", label.to_string()),
                        ("s", format!("{:.0}", ms.share * 100.0)),
                        ("t", format!("{:.0}", mt.share * 100.0)),
                    ],
                ),
            );
        }
        // The running frame-global value advances so the NEXT zone's drift
        // budget is measured from here — but neither `err_after` nor
        // `confidence` is written from it any more (R23-6): see the comment
        // in [`attach_zones`] and this module's own [`ZONE_ACCEPT_RATIO`]
        // proof that this number cannot judge a zone.
        *frame_err = zoned_err;
        Some(AcceptedZone { after: zone_after })
    } else {
        report.recipe.masks.pop();
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_DROPPED,
                vec![
                    ("label", label.to_string()),
                    ("before", format!("{zone_before:.3}")),
                    ("after", format!("{zone_after:.3}")),
                    ("ratio", format!("{:.0}", ZONE_ACCEPT_RATIO * 100.0)),
                    ("floor", format!("{ZONE_MATCHED_ERR:.3}")),
                    ("gain", format!("{:.0}", (1.0 - ZONE_FLOOR_MIN_GAIN) * 100.0)),
                    ("drift", format!("{:+.3}", zoned_err - *frame_err)),
                    ("tol", format!("{ZONE_GLOBAL_REGRESSION_TOL:+.3}")),
                ],
            ),
        );
        None
    }
}

/// Per-pixel mask weights for an analysis frame of `w`×`h` — the SAME
/// normalisation and bilinear sampling the engine's mask stage uses
/// (`render::sample_gray_norm` with x/w, y/h), so the moments are measured
/// exactly where the render will apply them.
fn mask_weights(mask: &GrayImage, w: u32, h: u32) -> Vec<f32> {
    // usize-widen BEFORE multiplying: `w * h` is a u32 product and a frame
    // over u32::MAX pixels would overflow the reservation (panic in debug,
    // pathological reallocation in release) while the loops still push w×h.
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            out.push(render::sample_gray_norm(mask, x as f32 / w as f32, y as f32 / h as f32));
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
            .save(&mask_path)
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
                mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
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
        std::fs::remove_file(mask_path).ok();
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
    fn fixture_mask_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("autoshop-{name}-{}.png", std::process::id()))
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
        sky_mask.save(&mask_path).unwrap();
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
        assert!(!mask_path.exists(), "no zone kept the raster — it must be reclaimed");
    }

    #[test]
    fn zoned_orchestration_attaches_the_sky_mask_and_improves_the_zone() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-orch-mask");
        sky_mask.save(&mask_path).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        let err_global = report.err_after;
        attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
        assert!(
            report.recipe.masks.iter().any(|m| m.role == MaskRole::ZoneSky && !m.inverted),
            "sky correction must attach: {}",
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
            report.recipe.rationale.contains("Zoned sky correction attached"),
            "rationale must document the zone: {}",
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
        std::fs::remove_file(mask_path).ok();
    }

    /// The zone stage's verdict is bounded by BOTH stages: it may not raise
    /// the global fit's own claim, and the global fit may not keep a claim
    /// the zones contradict.
    #[test]
    fn a_zoned_fits_confidence_comes_from_the_zones_it_accepted() {
        let (src, tgt, sky_mask) = zoned_pair();
        let mask_path = fixture_mask_path("zoned-confidence-mask");
        sky_mask.save(&mask_path).unwrap();
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
        std::fs::remove_file(mask_path).ok();
    }

    #[test]
    fn zoned_orchestration_corrects_the_land_through_the_inverted_raster() {
        // The first real-pair render's lesson: repainting ONLY the sky leaves
        // everything outside the mask with the global look — on the real
        // pair a blue haze band clashed against the new gold sky. The land
        // zone reuses the SAME raster inverted; when the target's land
        // differs too (muted vs vivid warm), both zones must attach.
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
        sky_mask.save(&mask_path).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
        assert!(
            report.recipe.masks.iter().any(|m| m.role == MaskRole::ZoneSky && !m.inverted),
            "sky zone must attach: {}",
            report.recipe.rationale
        );
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
        std::fs::remove_file(mask_path).ok();
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
        sm.save(&mask_path).unwrap();
        let mut report = fit::fit_recipe(&src, &tgt);
        attach_zones(&src, &tgt, &mut report, &sm, &tm, &mask_path);
        assert!(
            report.recipe.rationale.contains("Zoned sky correction attached"),
            "the zone gate must attach a correct repaint despite the share \
             mismatch: {}",
            report.recipe.rationale
        );
        assert!(
            report.recipe.rationale.contains("compositions differ"),
            "rationale must surface the share mismatch: {}",
            report.recipe.rationale
        );
        std::fs::remove_file(mask_path).ok();
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
        };
        let mask_path = fixture_mask_path("zoned-orch-nopython-mask");
        let report = fit_recipe_zoned(&src, &tgt, &seg, &mask_path);
        assert!(report.recipe.masks.is_empty(), "fallback must not attach masks");
        assert!(
            report.recipe.rationale.contains("global fit only"),
            "rationale must explain the fallback: {}",
            report.recipe.rationale
        );
        // The temporary segmentation inputs must not survive the fallback.
        for suffix in [".src-in.png", ".tgt-in.png", ".tgt-mask.png"] {
            let mut p = mask_path.as_os_str().to_owned();
            p.push(suffix);
            assert!(
                !Path::new(&p).exists(),
                "temp file {suffix} leaked past the fallback"
            );
        }
    }
}
