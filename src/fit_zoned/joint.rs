//! The joint family: luma bands and chroma classes, the far-error cause, joint buckets and readings, and the family's own acceptance ladder.

use super::*;

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
// number below is this family's own. The fixture test records those
// measurements and enforces the policy that the cause, not the measurement,
// chooses the typed FAR note.
//
// STATUS (R24 batch 2, 2026-08-17): the policy boundary is established. R23-6
// set it against synthetic fixtures alone and recorded the debt in this very
// block ("wants a real-pair review before anyone treats a number here as
// measured truth"). Six real (RAW, finished JPEG) pairs off the user's own
// library — EXIF-timestamp-confirmed same frame — have since been measured
// through `autoshade match`, and they provided that review. Their table is
// `fit::tests::joint_family_is_calibrated_on_the_fixture_set`'s doc. The
// fixture and real-pair values are retained as regression evidence; they do
// not widen the boundary when a refusal happens to read farther away. The ONE
// pair the user called nonsense (an astro composite: the Milky Way gone, the
// deep blue turned grey) read 0.141 — under the old 0.25 line, so it raised
// no warning and still reported 0.58 confidence.
//
// The separation the fixtures established is unchanged and still holds: a
// fit that REACHES its target lands the weighted reading at 0.001-0.06,
// while the pair whose target is a repaint the global model structurally
// cannot reach lands at 0.58. The real pairs simply showed where inside
// that gap the fixed policy line has to sit.

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
///   * WHERE INSIDE THE GAP the line sits is a choice, and this is the
///     reasoning behind it: the two fixtures where the solver correctly
///     REFUSED to chase a whole-scene regrade (canyon warm 0.061, canyon
///     gold 0.093) are policy refusals, not misses, so they must not be
///     accused of being far — and 0.093 leaves only 8% of headroom under
///     the line. The line is deliberately biased the other way (41% of
///     headroom under the real failure at 0.141): a real pair the user
///     called nonsense outranks a synthetic fixture whose silence is a
///     judgement call.
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
pub(super) fn zone_luma_err(a: &ZoneMoments, b: &ZoneMoments) -> f32 {
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
pub(super) fn joint_weights(px: &[[f32; 3]], bucket: usize, out: &mut Vec<f32>) {
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
/// Verified by running the WHOLE lib suite under it, re-measured 2026-09-02
/// on v1.2.4: `AUTOSHADE_FIT_JOINT=off cargo test --release --lib` reports
/// 1322 passed, 9 failed, 12 ignored. Every failure is in `fit` or
/// `fit_zoned` — 265 of the 274 tests in those two modules pass unchanged —
/// and every one of the nine reads this family's own output through one of
/// the four consumers above: `fit::joint_family_is_calibrated_on_the_fixture_set`
/// and `fit::the_worst_bucket_cannot_gate_a_stage` read the reading itself,
/// `fit::wb_default_strength_is_byte_identical_to_head` pins a confidence the
/// cap produces, `fit::same_content_evidence_diagnosis_reports_cornwall_support_and_terminal_readings`
/// reads the terminal arm, `fit::a_residual_the_mixer_cannot_reach_is_still_named`,
/// `fit::a_per_band_residual_stays_named_until_the_full_strength_fit_closes_it`,
/// `fit::the_unsolvable_controls_are_named_for_this_pair` and
/// `fit_zoned::unrepresented_note_is_derived_from_the_finished_zoned_render`
/// assert the colour-shaped route of `unrepresented_note` that consumer 4
/// describes falling back, and
/// `fit_zoned::the_joint_family_matches_by_value_not_by_position` asserts the
/// family exists at all. That is the correct answer to switching it off and
/// is why the variable is a diagnostic, not a supported test configuration.
/// (The count moved from R23's "41 of them / five that fail": the suite has
/// grown and three of the four consumers gained tests since.)
fn joint_family_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            crate::config::live_env("AUTOSHADE_FIT_JOINT").as_deref().map(str::trim),
            Some("off") | Some("0") | Some("false")
        )
    })
}
