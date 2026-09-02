//! Reverse-fit ("match") — derive an editable [`EditRecipe`] from a LOOK.
//!
//! Given the same shot twice — the untouched source preview and a target
//! rendition of it (the gpt-image `reimagine` output, or any finished reference
//! of the SAME frame) — solve for the develop parameters that reproduce the
//! target's tonality and colour through OUR deterministic engine. No pixels are
//! copied: the output is sliders + curves, so it applies at full sensor
//! resolution and serialises to a Lightroom XMP sidecar. This is how a low-res
//! generative experiment becomes a real, adjustable, full-resolution develop.
//!
//! Method: evidence-weighted statistics, not direct pixel regression. A
//! generative target is not pixel-aligned with the source, so contiguous
//! cells first receive support from [`structure_divergence`]. Luma bins and
//! hue bands then keep only population present on both sides. Every solve,
//! gate, confidence value and disclosure consumes that same evidence model.
//!
//!   1. **Tone** — evidence-weighted luminance matching gives a monotone map `M`; sample it at
//!      the engine's own tone knots ([`render::TONE_KNOTS_X`]) and least-squares
//!      solve the sliders against the engine's OWN basis
//!      ([`render::tone_slider_basis`]), scanning exposure (it enters the model
//!      nonlinearly). The solve carries a REAL magnitude prior (ridge +
//!      penalised model selection): the knot system is near-collinear, so
//!      without it grotesque mutually-cancelling combos (Exposure +1.5 with
//!      Contrast −97 and Shadows −100) beat tasteful ones by numerical ε —
//!      the residual curve makes their total maps indistinguishable, but the
//!      slider semantics are ruined (real-photo failure, 2026-07-07). Whatever
//!      shape the penalised sliders don't express goes into `tone_curve`
//!      control points, which the engine composes exactly on top.
//!   2. **Saturation** — evidence-weighted mean-chroma ratio, secant-refined through real
//!      [`render::develop_preview`] renders (closed loop, not open-loop math).
//!      Chroma matching against a non-aligned target is a heuristic, so the
//!      pipeline ends with a DO-NO-HARM check: if the finished recipe renders
//!      farther from the target than the untouched source, saturation is
//!      halved (cast curves refit each step — they depend on the saturated
//!      state) until the end-to-end error stops objecting (the 2026-07-09
//!      golden-sky pair dragged the chroma chase to the cap and rendered
//!      worse than doing nothing). Saturation cannot be judged mid-pipeline:
//!      a correct value legitimately amplifies a latent cast that the curve
//!      stage then removes.
//!   3. **Colour cast** — per-channel CDF residuals → red/green/blue curves,
//!      last as the catch-all (cast-before-saturation measured worse on the
//!      haze regression — see the stage comments in [`fit_recipe`]). Accepted
//!      only through evidence-weighted gates: the aggregate look-error ratio, the
//!      foreign-hue veto (the curves must not paint a region of the frame in
//!      hues the target holds nowhere — the 2026-07-09 violet-sky failure was
//!      cross-band invisible to the aggregate) and the rotation budget (nor
//!      re-hue a region into hues the target DOES hold — the golden-sky
//!      failure passed both earlier gates; see the veto const blocks). A
//!      global curve is also refused when it would materially move a luma or
//!      hue population with no two-sided support.
//!   4. **Detail** — coarse and fine local-luma energy drive `clarity` and
//!      `texture`, each capped at ±20 and enabled only when enough shared
//!      structural and luma-range evidence survives.
//!   5. **Per-band colour mixer** — `hsl.saturation` and `hsl.luminance`,
//!      one ACR band at a time, from that band's OWN population statistics
//!      (mean chroma and mean Rec.601 luma), admitted only by the same
//!      two-sided population gate the rest of the module reads, budgeted by
//!      strength, and required to earn its place against its own absence on
//!      the finished frame. Fitted between the saturation chase and the
//!      channel curves — stage "4a" in the code comments, listed last here
//!      because it is the newest.
//!
//! `hsl.hue` is deliberately NEVER solved: rotating a band re-populates it,
//! so its evidence is circular, and a plausible in-gate centroid delta
//! applied as a whole-band rotation is what caused the 2026-07-07 purple-sky
//! failure. A band's mean chroma and mean luma are ordinary marginal
//! statistics of a sub-population and carry no such trap.
//!
//! Every stage fits the RESIDUAL against a fresh render of the current recipe,
//! so stage interactions are absorbed instead of compounding; the report carries
//! the honest before/after evidence-weighted error (tonal + channel means +
//! per-band hue + spatial divergence, so a permutation or hue disaster cannot
//! hide behind matched luma quantiles). Local
//! masks and content changes are out of scope by construction (statistics
//! cannot localise them) — the AI style-prompt path covers intent the numbers
//! cannot.

use std::borrow::Cow;

use image::DynamicImage;

use crate::recipe::{CurvePoint, EditRecipe};
use crate::render;

/// Analysis resolution (long edge). CDFs and band means are stable well below
/// this; keeping it small keeps the closed-loop renders interactive-fast
/// (5 in the common path; up to ~20 if the do-no-harm loop shrinks
/// saturation, each a 384-px develop).
pub(crate) const ANALYZE_EDGE: u32 = 384;
const HIST_BINS: usize = 1024;
/// Global structural-divergence threshold. Calibration on same-content pairs:
/// showcase 1/2/3 = 0.075/0.168/0.095, viaduct = 0.070 and sunset = 0.226;
/// the generated-cloud failure is 0.491. `pub` (not `pub(crate)`): the GUI
/// bin crate quotes the same number in the generation-side fidelity
/// disclosure, so the threshold has exactly one definition.
pub const DIVERGENCE_GLOBAL: f32 = 0.35;
/// Per-zone structural-divergence threshold. The same-content top-35% strips
/// peak at 0.532, while the generated-cloud sky is 1.186 (land = 0.436).
pub(crate) const DIVERGENCE_ZONE: f32 = 0.65;
/// A divergent semantic partition covering this source-frame share promotes
/// the global solve to Atmosphere mode. The failing sky covers 44.36%.
pub(crate) const DIVERGENT_COVER_PROMOTES: f32 = 0.35;
/// Independent cap for every residual tone-curve segment. The three showcase
/// curves peak at 1.762/1.905/1.762; the generated-cloud failure reached 4.52.
const RESIDUAL_SLOPE_CAP: f32 = 2.0;

const ATMOSPHERE_EV_LIMIT: f32 = 1.0;
const ATMOSPHERE_SAT_LIMIT: f32 = 30.0;
const ATMOSPHERE_WB_GAIN_MIN: f32 = 0.80;
const ATMOSPHERE_WB_GAIN_MAX: f32 = 1.25;
const ATMOSPHERE_WB_GAIN_RATIO: f32 = 1.40;
const ATMOSPHERE_CURVE_SLOPE_MIN: f32 = 0.5;
const ATMOSPHERE_CURVE_SLOPE_MAX: f32 = 1.5;
const ATMOSPHERE_CONFIDENCE_CAP: f32 = 0.50;
/// Per-band colour-mixer ceiling on the recipe's own +/-100 axis, at Strength
/// 0 / the shipped default / Strength 1. Deliberately far below the global
/// saturation budget: a band move is applied to a SUB-population that the
/// frame statistics can only see through eight coarse bins, so the default is
/// the size of a correction a user would call "the blues are a bit flat"
/// rather than a re-grade. The engine halves the luminance axis on its way in
/// (`render::apply_hsl`), so that axis is the gentler of the two by
/// construction and needs no second number.
const HSL_BAND_LIMIT_MIN: f32 = 6.0;
const HSL_BAND_LIMIT_DEFAULT: f32 = 18.0;
const HSL_BAND_LIMIT_MAX: f32 = 45.0;
/// Kelvin domain the WB search walks in log space; landing on either end is
/// disclosed above default strength (`FIT_NOTE_WB_SEARCH_BOUND`).
const WB_SEARCH_K: (f32, f32) = (2000.0, 40000.0);

/// Whether unsupported population movement is withheld or disclosed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VetoPolicy {
    Withhold,
    Disclose,
}

/// Strength-governed honesty budget for the global reverse-fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitBudget {
    pub ev: f32,
    pub sat: f32,
    pub wb_gain: (f32, f32),
    pub wb_ratio: f32,
    /// Maximum weighted frame share that a WB correction may re-hue.
    pub wb_rotation_share: f32,
    pub cast_ratio: f32,
    pub slope: (f32, f32),
    pub confidence_cap: f32,
    /// Ceiling for ONE band of the per-band colour mixer, on the recipe's
    /// +/-100 axis. The strength dial has to be able to turn this stage, so
    /// it interpolates like every other budget dimension.
    pub hsl_band: f32,
    pub vetoes: VetoPolicy,
}

impl FitBudget {
    pub fn for_strength(s: crate::recipe::GradeStrength) -> Self {
        let s = s.get().clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let between = |at_zero: f32, at_default: f32, at_full: f32| {
            if s <= crate::recipe::GradeStrength::DEFAULT {
                lerp(at_zero, at_default, s / crate::recipe::GradeStrength::DEFAULT)
            } else {
                lerp(
                    at_default,
                    at_full,
                    (s - crate::recipe::GradeStrength::DEFAULT)
                        / (1.0 - crate::recipe::GradeStrength::DEFAULT),
                )
            }
        };
        let wb_rotation_share = if s <= crate::recipe::GradeStrength::DEFAULT {
            ROT_SHARE
        } else {
            lerp(
                ROT_SHARE,
                1.0,
                (s - crate::recipe::GradeStrength::DEFAULT)
                    / (1.0 - crate::recipe::GradeStrength::DEFAULT),
            )
        };
        Self {
            ev: between(0.5, ATMOSPHERE_EV_LIMIT, 2.5),
            sat: between(15.0, ATMOSPHERE_SAT_LIMIT, 60.0),
            wb_gain: (between(0.90, ATMOSPHERE_WB_GAIN_MIN, 0.50), between(1.12, ATMOSPHERE_WB_GAIN_MAX, 2.0)),
            wb_ratio: between(1.20, ATMOSPHERE_WB_GAIN_RATIO, 3.0),
            wb_rotation_share,
            cast_ratio: between(1.5, CAST_ACCEPT_RATIO, 3.0),
            slope: (between(0.7, ATMOSPHERE_CURVE_SLOPE_MIN, 0.25), between(1.3, ATMOSPHERE_CURVE_SLOPE_MAX, 3.0)),
            confidence_cap: between(0.50, ATMOSPHERE_CONFIDENCE_CAP, 0.35),
            hsl_band: between(HSL_BAND_LIMIT_MIN, HSL_BAND_LIMIT_DEFAULT, HSL_BAND_LIMIT_MAX),
            vetoes: if s >= 0.85 { VetoPolicy::Disclose } else { VetoPolicy::Withhold },
        }
    }
}

/// Options shared by global and zoned reverse-fit entry points.
#[derive(Clone, Copy, Default)]
pub struct FitOptions<'a> {
    pub strength: crate::recipe::GradeStrength,
    pub provider: Option<CorrespondenceProvider<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalCast {
    pub rotation_deg: f32,
    pub chroma_ratio: f32,
}

fn wb_gain_ratio(gains: [f32; 3]) -> f32 {
    gains.iter().copied().fold(0.0f32, f32::max)
        / gains
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min)
            .max(1e-6)
}

fn wb_gains_fit_budget(gains: [f32; 3], budget: FitBudget) -> bool {
    gains
        .iter()
        .all(|gain| (budget.wb_gain.0..=budget.wb_gain.1).contains(gain))
        && wb_gain_ratio(gains) <= budget.wb_ratio
}

fn wb_path_candidate(anchor: f32, wb_k: f32, wb_tint: f32, lambda: f32) -> (f32, f32) {
    let k = (anchor.ln() + (wb_k.ln() - anchor.ln()) * lambda).exp();
    ((k / 50.0).round() * 50.0, round1(wb_tint * lambda))
}

/// Keep a fitted white balance on the renderer's Kelvin/tint manifold while
/// spending no more than the strength budget. The only degree of freedom is
/// the scalar distance from as-shot `(anchor, 0)` to the free fit. Kelvin is
/// interpolated in log space, matching the free search domain.
fn budgeted_wb(
    anchor: f32,
    wb_k: f32,
    wb_tint: f32,
    budget: FitBudget,
) -> (f32, f32, bool, f32, f32, f32) {
    let free_gains = render::wb_gains(anchor, wb_k, wb_tint);
    let ratio_before = wb_gain_ratio(free_gains);
    if wb_gains_fit_budget(free_gains, budget) {
        return (wb_k, wb_tint, false, ratio_before, ratio_before, 1.0);
    }

    let (anchor_log, wb_log) = (anchor.ln(), wb_k.ln());
    let candidate = |lambda: f32, rounded: bool| {
        let k = (anchor_log + (wb_log - anchor_log) * lambda).exp();
        let tint = wb_tint * lambda;
        if rounded { wb_path_candidate(anchor, wb_k, wb_tint, lambda) } else { (k, tint) }
    };

    // Zero is as-shot and therefore legal. Maintain a legal lower endpoint
    // and find the largest scalar move admitted by both WB constraints.
    let (mut legal, mut illegal) = (0.0f32, 1.0f32);
    for _ in 0..32 {
        let middle = (legal + illegal) * 0.5;
        let (k, tint) = candidate(middle, false);
        if wb_gains_fit_budget(render::wb_gains(anchor, k, tint), budget) {
            legal = middle;
        } else {
            illegal = middle;
        }
    }

    // Persist with the free path's exact rounding. If quantisation nudges the
    // endpoint over a bound, shrink along the same scalar path until the
    // persisted (rather than merely continuous) WB is legal too.
    let continuous_legal = legal;
    let (mut k, mut tint) = candidate(continuous_legal, true);
    if !wb_gains_fit_budget(render::wb_gains(anchor, k, tint), budget) {
        let (mut rounded_legal, mut rounded_illegal) = (0.0f32, continuous_legal);
        for _ in 0..32 {
            let middle = (rounded_legal + rounded_illegal) * 0.5;
            let (middle_k, middle_tint) = candidate(middle, true);
            if wb_gains_fit_budget(
                render::wb_gains(anchor, middle_k, middle_tint),
                budget,
            ) {
                rounded_legal = middle;
            } else {
                rounded_illegal = middle;
            }
        }
        legal = rounded_legal;
        (k, tint) = candidate(legal, true);
    } else {
        legal = continuous_legal;
    }

    let ratio_after = wb_gain_ratio(render::wb_gains(anchor, k, tint));
    (k, tint, true, ratio_before, ratio_after, legal)
}

fn mean_chroma_vector(px: &[[f32; 3]], weights: &[f32]) -> [f32; 2] {
    let mut out = [0.0; 2];
    let mut total = 0.0;
    for (i, p) in px.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        out[0] += (render::srgb_to_linear(p[0]) - render::srgb_to_linear(p[2])) * w;
        out[1] += (render::srgb_to_linear(p[1]) - (render::srgb_to_linear(p[0]) + render::srgb_to_linear(p[2])) * 0.5) * w;
        total += w;
    }
    if total > 1e-8 { [out[0] / total, out[1] / total] } else { [0.0; 2] }
}

fn hue_degrees(p: &[f32; 3]) -> Option<f32> {
    (evidence_hue_band(p).is_some()).then(|| render::rgb_to_hsl(p[0], p[1], p[2]).0 * 360.0)
}

fn signed_hue_delta(a: f32, b: f32) -> f32 {
    (b - a + 540.0).rem_euclid(360.0) - 180.0
}

fn detect_global_cast(sp: &[[f32; 3]], tp: &[[f32; 3]], hue: &[EvidenceRange]) -> Option<GlobalCast> {
    let populated = hue.iter().filter(|r| r.source_populated || r.target_populated).collect::<Vec<_>>();
    if populated.is_empty() || populated.iter().any(|r| r.weight > 0.0 || r.source_populated == r.target_populated) {
        return None;
    }
    let mut deltas = Vec::new();
    for (s, t) in sp.iter().zip(tp) {
        if let (Some(a), Some(b)) = (hue_degrees(s), hue_degrees(t)) { deltas.push(signed_hue_delta(a, b)); }
    }
    if deltas.is_empty() { return None; }
    let sin = deltas.iter().map(|d| d.to_radians().sin()).sum::<f32>();
    let cos = deltas.iter().map(|d| d.to_radians().cos()).sum::<f32>();
    let mean = sin.atan2(cos).to_degrees();
    let coherent = deltas.iter().filter(|d| signed_hue_delta(mean, **d).abs() <= 45.0).count() as f32 / deltas.len() as f32;
    (coherent >= 0.80).then(|| {
        let cs = sp.iter().filter_map(|p| hue_degrees(p).map(|_| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum::<f32>() / sp.len().max(1) as f32;
        let ct = tp.iter().filter_map(|p| hue_degrees(p).map(|_| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum::<f32>() / tp.len().max(1) as f32;
        GlobalCast { rotation_deg: mean, chroma_ratio: ct / cs.max(1e-6) }
    })
}

/// The global reverse-fit policy selected before any CDF solve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitMode {
    Full,
    Atmosphere,
}

/// The two calibrated components of structural divergence and their Euclidean
/// combination `d = sqrt((1-correlation)^2 + energy_error^2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divergence {
    pub correlation: f32,
    pub energy_error: f32,
    pub d: f32,
}

/// Evidence carried by one value range.  A range is identifiable only when
/// both images populate it and its own structural comparison is stable.
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceRange {
    pub label: String,
    pub source_share: f32,
    pub target_share: f32,
    pub source_evidence_share: f32,
    pub target_evidence_share: f32,
    pub two_sided_share: f32,
    pub divergence: f32,
    pub weight: f32,
    pub source_populated: bool,
    pub target_populated: bool,
}

/// One evidence map is shared by the objective, all gates, confidence and
/// disclosures.  Pixel weights are attached to the source/target analysis
/// rasters; candidates keep the source weights, so edits cannot move pixels
/// into evidence after the fact.
#[derive(Clone, Debug)]
pub struct EvidenceModel {
    pub source_pixels: Vec<[f32; 3]>,
    /// Soft membership of each aligned source pixel in this model's
    /// population: all ones for the frame, the source coverage for a scoped
    /// view. Blind-move numerators use the same mass as [`Self::population`].
    pub source_membership: Vec<f32>,
    pub width: u32,
    pub height: u32,
    pub spatial_supported: Vec<bool>,
    pub source_weights: Vec<f32>,
    pub target_weights: Vec<f32>,
    pub source_hue_weights: Vec<f32>,
    pub target_hue_weights: Vec<f32>,
    pub luma: Vec<EvidenceRange>,
    pub hue: Vec<EvidenceRange>,
    /// Population chromaticity facts used to recognize a coherent global cast.
    pub source_chroma_vector: [f32; 2],
    pub target_chroma_vector: [f32; 2],
    pub global_cast: Option<GlobalCast>,
    pub identifiability: f32,
    /// Per-pixel ingredients of the range verdicts above — the pixel's
    /// spatial-cell confidence, that cell's divergence and the frame-wide
    /// same-content verdict — kept so [`EvidenceModel::scoped`] can
    /// re-aggregate the same support over a zone's own population.
    pub spatial_weights: Vec<f32>,
    pub spatial_divergence: Vec<f32>,
    pub globally_same_content: bool,
    /// Weighted member count of the population these verdicts are over: the
    /// frame's pixel count for the global model, a coverage's mass for a
    /// scoped view. The blind-move audit's region line is a share of THIS,
    /// so a half-withheld 6% tile is a region of its own population, not a
    /// 3% speckle of the frame.
    pub population: f32,
}

impl EvidenceModel {
    /// This model's range verdicts re-aggregated over ONE zone: `source_zone`
    /// / `target_zone` are the zone's soft memberships on the analysis
    /// rasters, `tp` the target's analysis pixels. Evidence verdicts follow
    /// the population a correction MOVES — the frame for the global fit, the
    /// zone for a zone — so a ground zone is no longer withheld because a
    /// replaced sky happens to share its luma bins, while a zone whose own
    /// members are divergent stays withheld unless the frame-wide
    /// same-content verdict holds. That verdict deliberately bypasses range
    /// survival and per-pixel withholding. Per-pixel support
    /// (`spatial_supported`) is unchanged; over the whole aligned frame this
    /// is the model itself. The two rasters share one geometry by
    /// construction ([`analysis_pair`]); the aligned-prefix arithmetic below
    /// is the defensive form of that contract.
    pub fn scoped(&self, tp: &[[f32; 3]], source_zone: &[f32], target_zone: &[f32]) -> EvidenceModel {
        let ranges = aggregate_ranges(
            &self.source_pixels,
            tp,
            source_zone,
            target_zone,
            SupportField {
                spatial_weights: &self.spatial_weights,
                spatial_divergence: &self.spatial_divergence,
                globally_same_content: self.globally_same_content,
            },
        );
        EvidenceModel {
            source_pixels: self.source_pixels.clone(),
            source_membership: ranges.source_membership,
            width: self.width,
            height: self.height,
            spatial_supported: self.spatial_supported.clone(),
            source_weights: ranges.source_weights,
            target_weights: ranges.target_weights,
            source_hue_weights: ranges.source_hue_weights,
            target_hue_weights: ranges.target_hue_weights,
            luma: ranges.luma,
            hue: ranges.hue,
            source_chroma_vector: self.source_chroma_vector,
            target_chroma_vector: self.target_chroma_vector,
            global_cast: self.global_cast,
            identifiability: ranges.identifiability,
            spatial_weights: self.spatial_weights.clone(),
            spatial_divergence: self.spatial_divergence.clone(),
            globally_same_content: self.globally_same_content,
            population: ranges.population,
        }
    }

    /// Re-aggregate this frame on Atmosphere mode's structure-blind evidence
    /// doctrine. Atmosphere mode is entered because structure diverges; its
    /// instruments are the budgets (EV +/-1, WB gain [0.80, 1.25], saturation
    /// +/-30, curve slope [0.5, 1.5]) and the population facts, not structural
    /// survival. Population vetoes remain intact because [`Self::scoped`]
    /// re-derives them from the unchanged source and target pixels.
    ///
    /// On a frame model whose `globally_same_content` is already true, this is
    /// byte-equal to `scoped(tp, ones, ones)`; the unit test pins that invariant.
    pub fn structure_blind(&self, tp: &[[f32; 3]]) -> EvidenceModel {
        let n = self.source_pixels.len().min(tp.len());
        let ones = vec![1.0; n];
        let mut blind = self.clone();
        blind.spatial_weights = ones.clone();
        blind.spatial_supported = vec![true; n];
        blind.globally_same_content = true;
        blind.scoped(tp, &ones, &ones)
    }
}

const EVIDENCE_LUMA_BINS: usize = 17;
const EVIDENCE_HUE_BANDS: usize = 8;
const EVIDENCE_MIN_SHARE: f32 = 0.015;
const EVIDENCE_DIVERGENCE_CUTOFF: f32 = 1.0;
/// The range form of the existing per-zone divergence policy. A range must
/// retain at least the support left at `DIVERGENCE_ZONE`; the texture
/// calibration is D=0.628 and survives, while the invented-sky value ranges
/// retain only 9-16% and do not.
const EVIDENCE_RANGE_SURVIVAL_MIN: f32 = 1.0 - DIVERGENCE_ZONE;
const UNSUPPORTED_RANGE_MOVE: f32 = 2.0 / 255.0;
/// The same-content texture calibration retains 0.341 identifiability; the
/// invented-sky pair retains 0.218. Detail fitting is enabled between them.
const DETAIL_EVIDENCE_MIN_IDENTIFIABILITY: f32 = 0.30;

/// Fold the existing 17-bin luma verdict over one inclusive contiguous run.
/// Range partitioning uses this instead of inventing a second evidence model:
/// population, structural survival and estimator weight retain the global
/// fit's exact per-bin meanings.
pub(crate) fn luma_evidence_for_bins(
    evidence: &EvidenceModel,
    first: usize,
    last: usize,
) -> EvidenceRange {
    let end = last.saturating_add(1).min(evidence.luma.len());
    let start = first.min(end);
    let bins = &evidence.luma[start..end];
    let source_share = bins.iter().map(|r| r.source_share).sum::<f32>();
    let target_share = bins.iter().map(|r| r.target_share).sum::<f32>();
    let source_evidence_share = bins.iter().map(|r| r.source_evidence_share).sum::<f32>();
    let target_evidence_share = bins.iter().map(|r| r.target_evidence_share).sum::<f32>();
    let two_sided_share = source_evidence_share.min(target_evidence_share);
    let structural_mass = bins.iter().map(|r| r.weight).sum::<f32>();
    let divergence_weight = bins
        .iter()
        .filter(|r| r.divergence.is_finite())
        .map(|r| r.two_sided_share)
        .sum::<f32>();
    let divergence = if divergence_weight > 0.0 {
        bins.iter()
            .filter(|r| r.two_sided_share > 0.0 && r.divergence.is_finite())
            .map(|r| r.divergence * r.two_sided_share)
            .sum::<f32>()
            / divergence_weight
    } else {
        f32::INFINITY
    };
    let source_populated = source_share >= EVIDENCE_MIN_SHARE;
    let target_populated = target_share >= EVIDENCE_MIN_SHARE;
    EvidenceRange {
        label: format!("luma bins {start:02}-{last:02}"),
        source_share,
        target_share,
        source_evidence_share,
        target_evidence_share,
        two_sided_share,
        divergence,
        weight: if source_populated && target_populated { structural_mass } else { 0.0 },
        source_populated,
        target_populated,
    }
}

pub fn evidence_luma_bin(v: f32) -> usize {
    ((v.clamp(0.0, 1.0) * EVIDENCE_LUMA_BINS as f32).floor() as usize)
        .min(EVIDENCE_LUMA_BINS - 1)
}

pub fn evidence_hue_band(p: &[f32; 3]) -> Option<usize> {
    let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
    if chroma < 0.06 {
        return None;
    }
    let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
    let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
    Some(if w1 < 0.5 { b0 } else { b1 })
}

fn evidence_range(
    label: String,
    source_members: &[f32],
    target_members: &[f32],
    source_population: f32,
    target_population: f32,
    support: SupportField<'_>,
) -> EvidenceRange {
    let SupportField { spatial_weights, spatial_divergence, globally_same_content } = support;
    // Memberships are soft weights (1.0 everywhere for the frame); the
    // populations are the weighted member counts the shares are taken over.
    let source_n = source_population.max(1.0);
    let target_n = target_population.max(1.0);
    let source_share = source_members.iter().sum::<f32>() / source_n;
    let target_share = target_members.iter().sum::<f32>() / target_n;
    let supported_share = |members: &[f32], population: f32| {
        members
            .iter()
            .zip(spatial_weights)
            .map(|(&member, &weight)| member * weight)
            .sum::<f32>()
            / population
    };
    let source_evidence_share = supported_share(source_members, source_n);
    let target_evidence_share = supported_share(target_members, target_n);
    // Population and structural support are two different facts.  The 1.5%
    // line answers only whether the range exists on each side; applying it a
    // second time after the divergence discount made a weakly-correlated
    // population look absent.  A populated range can therefore be withheld
    // for structure (zero `two_sided_share`) without being mislabeled as
    // one-sided/empty in the disclosure.
    let source_populated = source_share >= EVIDENCE_MIN_SHARE;
    let target_populated = target_share >= EVIDENCE_MIN_SHARE;
    let two_sided_share = source_evidence_share.min(target_evidence_share);
    let (div_sum, div_weight) = source_members
        .iter()
        .zip(spatial_weights)
        .zip(spatial_divergence)
        .filter(|((member, weight), divergence)| {
            **member > 0.0 && **weight > 0.0 && divergence.is_finite()
        })
        .fold((0.0f32, 0.0f32), |(sum, weight), ((&m, &w), &d)| {
            (sum + m * w * d, weight + m * w)
        });
    let divergence = if div_weight > 0.0 { div_sum / div_weight } else { f32::INFINITY };
    let structural_survival = (source_evidence_share / source_share.max(1e-6))
        .min(target_evidence_share / target_share.max(1e-6));
    let weight = if source_populated
        && target_populated
        && (globally_same_content || structural_survival >= EVIDENCE_RANGE_SURVIVAL_MIN)
    {
        two_sided_share
    } else {
        0.0
    };
    EvidenceRange {
        label,
        source_share,
        target_share,
        source_evidence_share,
        target_evidence_share,
        two_sided_share,
        divergence,
        weight,
        source_populated,
        target_populated,
    }
}

/// The value-range verdicts and per-pixel evidence weights of ONE population.
/// The global fit moves the whole frame and is judged by the frame's bins; a
/// zone moves only its members, so [`EvidenceModel::scoped`] re-aggregates the
/// same per-pixel structural support over them. Memberships are soft (a
/// refined mask's feather), the frame is the all-ones case, and target luma
/// bins are rank-paired within the population's own target members at its own
/// source-to-target mass ratio. Every sum and share uses the one aligned
/// `0..min(source.len(), target.len())` prefix. A divergent population is
/// withheld unless the frame-wide same-content verdict deliberately bypasses
/// range survival and per-pixel withholding.
struct RangeAggregate {
    /// Source membership over the same aligned prefix as every range sum.
    source_membership: Vec<f32>,
    source_weights: Vec<f32>,
    target_weights: Vec<f32>,
    source_hue_weights: Vec<f32>,
    target_hue_weights: Vec<f32>,
    luma: Vec<EvidenceRange>,
    hue: Vec<EvidenceRange>,
    identifiability: f32,
    population: f32,
}

/// The per-pixel structural support one population's verdicts are read
/// against: the frame's spatial-cell confidence, that cell's divergence and
/// the frame-wide same-content verdict.
#[derive(Clone, Copy)]
struct SupportField<'a> {
    spatial_weights: &'a [f32],
    spatial_divergence: &'a [f32],
    globally_same_content: bool,
}

fn aggregate_ranges(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    source_zone: &[f32],
    target_zone: &[f32],
    support: SupportField<'_>,
) -> RangeAggregate {
    let spatial_weights = support.spatial_weights;
    let n = sp.len().min(tp.len());
    let member = |zone: &[f32], i: usize| zone.get(i).copied().unwrap_or(0.0).max(0.0);
    let source_membership = (0..n).map(|i| member(source_zone, i)).collect::<Vec<_>>();
    let source_mass = source_membership.iter().sum::<f32>();
    let target_mass = (0..n).map(|i| member(target_zone, i)).sum::<f32>();
    let mut luma = Vec::with_capacity(EVIDENCE_LUMA_BINS);
    let mut luma_source_weights = vec![0.0f32; n];
    let mut luma_target_weights = vec![0.0f32; n];
    let mut target_order: Vec<usize> = (0..n).filter(|&i| member(target_zone, i) > 0.0).collect();
    target_order.sort_by(|&a, &b| luma601(&tp[a]).total_cmp(&luma601(&tp[b])));
    let ratio = if source_mass > 0.0 { target_mass / source_mass } else { 0.0 };
    let mut cursor = 0usize;
    let mut taken = 0.0f32;
    let mut quota = 0.0f32;
    for bin in 0..EVIDENCE_LUMA_BINS {
        let sm: Vec<f32> = sp[..n]
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if evidence_luma_bin(luma601(p)) == bin { member(source_zone, i) } else { 0.0 }
            })
            .collect();
        let lo = bin as f32 / EVIDENCE_LUMA_BINS as f32;
        let hi = (bin + 1) as f32 / EVIDENCE_LUMA_BINS as f32;
        // Luma correspondence is monotone: a real exposure/tone edit moves a
        // source bin to a different numeric target interval. Pair the bin to
        // the same target population ranks, then let spatial evidence decide
        // whether that population actually survives on both sides.
        let mut tm = vec![0.0f32; n];
        quota += sm.iter().sum::<f32>() * ratio;
        // A rank-boundary target member is consumed whole. Consequently the
        // cumulative allocation can drift by at most one member over the
        // entire population, rather than one fresh overshoot in every bin.
        while cursor < target_order.len() && taken < quota {
            let i = target_order[cursor];
            tm[i] = member(target_zone, i);
            taken += tm[i];
            cursor += 1;
        }
        let range = evidence_range(
            format!("luma[{lo:.2}-{hi:.2}]"),
            &sm,
            &tm,
            source_mass,
            target_mass,
            support,
        );
        for (i, &m) in sm.iter().enumerate().take(n) {
            if m > 0.0 && range.source_evidence_share > 0.0 {
                luma_source_weights[i] =
                    m * spatial_weights[i] * range.weight / range.source_evidence_share;
            }
        }
        for (i, &m) in tm.iter().enumerate().take(n) {
            if m > 0.0 && range.target_evidence_share > 0.0 {
                luma_target_weights[i] =
                    m * spatial_weights[i] * range.weight / range.target_evidence_share;
            }
        }
        luma.push(range);
    }
    let mut hue = Vec::with_capacity(EVIDENCE_HUE_BANDS);
    let mut source_hue_weights = vec![0.0f32; n];
    let mut target_hue_weights = vec![0.0f32; n];
    for band in 0..EVIDENCE_HUE_BANDS {
        let sm: Vec<f32> = sp[..n]
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if evidence_hue_band(p) == Some(band) { member(source_zone, i) } else { 0.0 }
            })
            .collect();
        let tm: Vec<f32> = tp[..n]
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if evidence_hue_band(p) == Some(band) { member(target_zone, i) } else { 0.0 }
            })
            .collect();
        let range = evidence_range(
            crate::recipe::HSL_BANDS[band].to_string(),
            &sm,
            &tm,
            source_mass,
            target_mass,
            support,
        );
        for (i, &m) in sm.iter().enumerate().take(n) {
            if m > 0.0 && range.source_evidence_share > 0.0 {
                source_hue_weights[i] =
                    m * spatial_weights[i] * range.weight / range.source_evidence_share;
            }
        }
        for (i, &m) in tm.iter().enumerate().take(n) {
            if m > 0.0 && range.target_evidence_share > 0.0 {
                target_hue_weights[i] =
                    m * spatial_weights[i] * range.weight / range.target_evidence_share;
            }
        }
        hue.push(range);
    }
    let luma_mass = luma.iter().map(|r| r.weight).sum::<f32>().min(1.0);
    let hue_mass = hue.iter().map(|r| r.weight).sum::<f32>().min(1.0);
    let identifiability = (0.75 * luma_mass + 0.25 * hue_mass).clamp(0.0, 1.0);
    RangeAggregate {
        source_membership,
        source_weights: luma_source_weights,
        target_weights: luma_target_weights,
        source_hue_weights,
        target_hue_weights,
        luma,
        hue,
        identifiability,
        population: source_mass,
    }
}

/// Per-pixel structural support from contiguous cells. The divergence
/// primitive expects coherent image geometry (erosion, gradients and Gaussian
/// bands), so value-bin masks are applied only after these cell readings.
fn spatial_evidence(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    w: u32,
    h: u32,
) -> (Vec<f32>, Vec<f32>) {
    const COLS: u32 = 3;
    const ROWS: u32 = 3;
    let scale = w.max(h).div_ceil(192).max(1);
    let sw = w.div_ceil(scale);
    let sh = h.div_ceil(scale);
    let mut ss = Vec::with_capacity((sw * sh) as usize);
    let mut tt = Vec::with_capacity((sw * sh) as usize);
    for y in (0..h).step_by(scale as usize) {
        for x in (0..w).step_by(scale as usize) {
            let i = (y * w + x) as usize;
            ss.push(sp[i]);
            tt.push(tp[i]);
        }
    }
    let mut weights = vec![0.0f32; sp.len().min(tp.len())];
    let mut divergences = vec![f32::INFINITY; weights.len()];
    for row in 0..ROWS {
        for col in 0..COLS {
            let mut mask = vec![0.0f32; ss.len()];
            for y in 0..sh {
                for x in 0..sw {
                    if x * COLS / sw.max(1) == col && y * ROWS / sh.max(1) == row {
                        mask[(y * sw + x) as usize] = 1.0;
                    }
                }
            }
            let reading = structure_divergence(&ss, &tt, sw, sh, &mask);
            let d = reading.d;
            let confidence = (1.0 - d / EVIDENCE_DIVERGENCE_CUTOFF).clamp(0.0, 1.0);
            for y in 0..h {
                for x in 0..w {
                    if x * COLS / w.max(1) == col && y * ROWS / h.max(1) == row {
                        let i = (y * w + x) as usize;
                        weights[i] = confidence;
                        divergences[i] = d;
                    }
                }
            }
        }
    }
    (weights, divergences)
}

/// Build the single per-pixel/per-range evidence map.  Range weights are the
/// two-sided population share discounted by that range's existing structural
/// divergence; no second divergence statistic is introduced.
fn inferred_geometry(n: usize) -> (u32, u32) {
    if n > 0 {
        let mut h = (n as f64).sqrt().floor() as usize;
        while h > 1 && !n.is_multiple_of(h) {
            h -= 1;
        }
        ((n / h) as u32, h as u32)
    } else {
        (0, 0)
    }
}

pub fn evidence_model_for(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    width: u32,
    height: u32,
) -> EvidenceModel {
    let n = sp.len().min(tp.len());
    let (w, h) = if width as usize * height as usize == n {
        (width, height)
    } else {
        inferred_geometry(n)
    };
    let (spatial_weights, spatial_divergence) = spatial_evidence(sp, tp, w, h);
    let all_spatial = vec![1.0f32; n];
    let global_divergence = structure_divergence(sp, tp, w, h, &all_spatial).d;
    let globally_same_content = global_divergence < DIVERGENCE_GLOBAL;
    let spatial_supported = spatial_divergence
        .iter()
        .map(|&divergence| globally_same_content || divergence < DIVERGENCE_ZONE)
        .collect::<Vec<_>>();
    let frame = vec![1.0f32; n];
    let ranges = aggregate_ranges(
        sp,
        tp,
        &frame,
        &frame,
        SupportField {
            spatial_weights: &spatial_weights,
            spatial_divergence: &spatial_divergence,
            globally_same_content,
        },
    );
    let source_chroma_vector = mean_chroma_vector(sp, &frame);
    let target_chroma_vector = mean_chroma_vector(tp, &frame);
    let global_cast = detect_global_cast(sp, tp, &ranges.hue);
    EvidenceModel {
        source_pixels: sp.iter().take(n).copied().collect(),
        source_membership: ranges.source_membership,
        width: w,
        height: h,
        spatial_supported,
        source_weights: ranges.source_weights,
        target_weights: ranges.target_weights,
        source_hue_weights: ranges.source_hue_weights,
        target_hue_weights: ranges.target_hue_weights,
        luma: ranges.luma,
        hue: ranges.hue,
        source_chroma_vector,
        target_chroma_vector,
        global_cast,
        identifiability: ranges.identifiability,
        spatial_weights,
        spatial_divergence,
        globally_same_content,
        population: ranges.population,
    }
}

#[cfg(test)]
pub(crate) fn evidence_model(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> EvidenceModel {
    let (width, height) = inferred_geometry(sp.len().min(tp.len()));
    evidence_model_for(sp, tp, width, height)
}

fn movement_identifiability(after: &[[f32; 3]], evidence: &EvidenceModel) -> f32 {
    let mut unsupported = 0.0f32;
    for (i, (before, after)) in evidence.source_pixels.iter().zip(after).enumerate() {
        let movement = (0..3).map(|ch| (after[ch] - before[ch]).abs()).sum::<f32>() / 3.0;
        let unsupported_luma = source_luma_is_withheld(i, evidence);
        let unsupported_hue = evidence_hue_band(before).is_some()
            && source_hue_is_withheld(i, evidence);
        if unsupported_luma || unsupported_hue {
            unsupported += movement;
        }
    }
    let mean_unsupported = unsupported / evidence.source_pixels.len().max(1) as f32;
    (-UNSUPPORTED_MOVEMENT_CONFIDENCE_SLOPE * mean_unsupported)
        .exp()
        .clamp(0.0, 1.0)
}

fn source_luma_is_withheld(index: usize, evidence: &EvidenceModel) -> bool {
    let Some(pixel) = evidence.source_pixels.get(index) else { return false };
    let range = &evidence.luma[evidence_luma_bin(luma601(pixel))];
    range.source_populated
        && (range.weight <= 0.0
            || !evidence.spatial_supported.get(index).copied().unwrap_or(false))
}

fn source_hue_is_withheld(index: usize, evidence: &EvidenceModel) -> bool {
    let Some(pixel) = evidence.source_pixels.get(index) else { return false };
    let Some(band) = evidence_hue_band(pixel) else { return false };
    let range = &evidence.hue[band];
    range.source_populated
        && (range.weight <= 0.0
            || !evidence.spatial_supported.get(index).copied().unwrap_or(false))
}

fn range_is_withheld(range: &EvidenceRange) -> bool {
    range.weight <= 0.0 && (range.source_populated || range.target_populated)
}

fn withheld_range_hits(evidence: &EvidenceModel) -> (Vec<bool>, Vec<bool>) {
    let mut luma = evidence
        .luma
        .iter()
        .map(range_is_withheld)
        .collect::<Vec<_>>();
    let mut hue = evidence
        .hue
        .iter()
        .map(range_is_withheld)
        .collect::<Vec<_>>();
    for (index, pixel) in evidence.source_pixels.iter().enumerate() {
        if !evidence.spatial_supported.get(index).copied().unwrap_or(false) {
            luma[evidence_luma_bin(luma601(pixel))] = true;
            if let Some(band) = evidence_hue_band(pixel) {
                hue[band] = true;
            }
        }
    }
    (luma, hue)
}

pub(crate) fn withheld_range_names(evidence: &EvidenceModel) -> (String, String) {
    let (luma, hue) = withheld_range_hits(evidence);
    let names = |hits: &[bool], ranges: &[EvidenceRange]| {
        hits
            .iter()
            .zip(ranges)
            .filter_map(|(&hit, range)| hit.then_some(range.label.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    (names(&luma, &evidence.luma), names(&hue, &evidence.hue))
}

/// Whether the shared evidence model withheld a populated range because only
/// one image carried it. This is the cause input for the single FAR classifier.
pub(crate) fn evidence_has_one_sided(evidence: &EvidenceModel) -> bool {
    evidence
        .luma
        .iter()
        .chain(&evidence.hue)
        .any(|range| {
            range.weight <= 0.0 && range.source_populated != range.target_populated
        })
}

fn divergent_range_names(evidence: &EvidenceModel) -> String {
    let (luma, hue) = withheld_range_hits(evidence);
    luma.iter()
        .zip(&evidence.luma)
        .chain(hue.iter().zip(&evidence.hue))
        .filter_map(|(&withheld, range)| {
            (withheld && range.source_populated && range.target_populated)
                .then_some(range.label.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn weighted_cdf(px: &[[f32; 3]], weights: &[f32], value: impl Fn(&[f32; 3]) -> f32) -> Vec<f32> {
    let mut hist = vec![0.0f32; HIST_BINS];
    let mut total = 0.0f32;
    for (i, p) in px.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if w <= 0.0 { continue; }
        let bin = (value(p).clamp(0.0, 1.0) * (HIST_BINS - 1) as f32).round() as usize;
        hist[bin] += w;
        total += w;
    }
    if total <= 1e-8 { return vec![0.0; HIST_BINS]; }
    let mut acc = 0.0;
    for v in &mut hist { acc += *v; *v = acc / total; }
    hist
}

/// Weighted median of a scattered sample, lower-median convention: the first
/// value at which the cumulative weight reaches half of the total.
///
/// Deliberately NOT [`weighted_cdf`] + [`quantile`]. That pair bins its key
/// into `HIST_BINS` buckets over `[0, 1]`, which is right for a channel value
/// and wrong for the quantity this batch reads: a per-pixel LOG RATIO is
/// signed, unbounded, and concentrated near zero, so a `[0, 1]` histogram
/// would clamp half of it into one bin. Sorting is exact and the cost is one
/// sort of the analysis frame per channel, against thirteen full renders on
/// the same path.
fn weighted_median(samples: &mut [(f32, f32)]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = samples.iter().map(|&(_, w)| w as f64).sum::<f64>();
    if total <= 0.0 {
        return 0.0;
    }
    let half = total * 0.5;
    let mut carried = 0.0f64;
    for &(value, weight) in samples.iter() {
        carried += weight as f64;
        if carried >= half {
            return value;
        }
    }
    samples[samples.len() - 1].0
}

/// The population AND the pairing the Atmosphere white balance is solved on.
/// ONE authority chooses both, and that is the point: a readable
/// shared-content population EXISTS only because a correspondence field was
/// readable, and the field that is trusted to say WHICH pixels are shared is
/// the same field that says WHICH target pixel each source pixel corresponds
/// to. Taking the population without the pairing is how a RECOMPOSED pair —
/// the same content, moved in frame, which is squarely inside Atmosphere's
/// remit — gets read as a colour cast: same-index pairing against the raw
/// target array is a random association once content has moved.
///
/// With no readable field the two sides are index-paired exactly as they
/// always were, and the per-pixel weight is `min(source, target)`. That is
/// not an invention: it is this module's own two-sidedness rule, the
/// `source_evidence_share.min(target_evidence_share)` that [`EvidenceModel`]
/// already applies to every range.
///
/// With a readable field `p.source` ALONE is the coherent weight, and mixing
/// in `p.target` would not be. `shared_content_population` sets `source[i]`
/// exactly where `conf[i] >= CONFIDENT_MATCH`, i.e. exactly where `pair_tp[i]`
/// IS that source pixel's counterpart; the target weights are RANK-paired
/// (built by sorting `luma601(&tp[..])`), so `target_weights[i]` is a
/// statement about `tp[i]`'s own luma rank, not about a pairing with `sp[i]`.
fn atmosphere_wb_pairing<'a>(
    tp: &'a [[f32; 3]],
    evidence: &'a EvidenceModel,
    correspondence: Option<&'a PairCorrespondence>,
    readable: Option<&'a SharedPopulation>,
) -> (&'a [[f32; 3]], Cow<'a, [f32]>) {
    match readable {
        Some(p) => (
            correspondence.map_or(tp, |c| c.tp.as_slice()),
            Cow::Borrowed(p.source.as_slice()),
        ),
        None => {
            let n = evidence.source_weights.len().min(evidence.target_weights.len());
            let paired = (0..n)
                .map(|i| evidence.source_weights[i].min(evidence.target_weights[i]))
                .collect::<Vec<_>>();
            (tp, Cow::Owned(paired))
        }
    }
}

/// The Atmosphere global white balance: a weighted median of the PER-PIXEL
/// log ratio, taken jointly over ONE population, normalised by its own
/// geometric mean and inverted through the engine's WB model. Returns the
/// rounded Kelvin, the rounded tint, and the normalised `wanted` gains the
/// search was fitted to.
///
/// Three INDEPENDENT per-channel medians — what this replaces — are a ratio
/// of two marginals. On a bimodal frame the two marginals' halfway points
/// fall in different sub-populations, so their ratio is not the colour change
/// of any pixel in the frame; on the crate's own `flat_sky_to_cloud_deck`,
/// where no pixel changed its chromaticity at all, they read K 4400 /
/// tint +55.2. This reads one cloud of per-pixel colour CHANGES and takes its
/// centre.
///
/// The LOG form is written rather than the raw ratio because the median
/// commutes with a monotone map — the two are numerically identical — and the
/// log is what makes the estimator a LOCATION statistic on one cloud instead
/// of a ratio of two. The `1e-5` floor is this call site's own existing
/// convention, not a new knob.
///
/// The MEDIAN, not the mean, for exactly the reason it always was: a newly
/// generated cloud highlight must not own a frame-wide average. That comment
/// was not overruled by this batch — what the per-pixel form adds is that the
/// robustness now applies to the CHANGE each pixel underwent rather than to
/// two brightness distributions read apart from each other.
fn atmosphere_wb_from_populations(
    sp: &[[f32; 3]],
    pair_tp: &[[f32; 3]],
    pair_w: &[f32],
    anchor: f32,
) -> (f32, f32, [f32; 3]) {
    let n = sp.len().min(pair_tp.len());
    let mut ratio = [1.0f32; 3];
    let mut samples: Vec<(f32, f32)> = Vec::with_capacity(n);
    for (ch, slot) in ratio.iter_mut().enumerate() {
        samples.clear();
        for i in 0..n {
            let w = pair_w.get(i).copied().unwrap_or(0.0).max(0.0);
            if w <= 0.0 {
                continue;
            }
            let source = render::srgb_to_linear(sp[i][ch]).max(1e-5);
            let target = render::srgb_to_linear(pair_tp[i][ch]).max(1e-5);
            samples.push((target.ln() - source.ln(), w));
        }
        *slot = weighted_median(&mut samples).exp();
    }
    let common = (ratio[0] * ratio[1] * ratio[2]).max(1e-12).powf(1.0 / 3.0);
    let wanted = ratio.map(|v| v / common);
    let (lo, hi) = (WB_SEARCH_K.0.ln(), WB_SEARCH_K.1.ln());
    let tint = ((1.0 - wanted[1]) / 0.20 * 100.0).clamp(-100.0, 100.0);
    let mut best = (anchor, f32::INFINITY);
    for i in 0..=400 {
        let k = (lo + (hi - lo) * i as f32 / 400.0).exp();
        let gains = render::wb_gains(anchor, k, tint);
        let err = gains
            .iter()
            .zip(wanted)
            .map(|(&g, want)| (g.max(1e-5) / want.max(1e-5)).log2().powi(2))
            .sum::<f32>();
        if err < best.1 {
            best = (k, err);
        }
    }
    ((best.0 / 50.0).round() * 50.0, round1(tint), wanted)
}

/// The Atmosphere global exposure: a weighted median of the PER-PIXEL log2
/// luminance ratio, taken over the same population and the same pairing the
/// white balance is solved on.
///
/// What this replaces is a ratio of two MARGINALS — the median of the source's
/// weighted luminance CDF against the median of the target's — and the reason
/// is the one already written out for the white balance, one line up. Two
/// marginals' halfway points fall in different sub-populations whenever the
/// frame is bimodal, so their ratio is not the luminance change of any pixel
/// in the frame; and once a correspondence field has said WHICH pixels are
/// shared, reading the two sides independently throws away the very pairing
/// that made the population trustworthy. This reads one cloud of per-pixel
/// luminance CHANGES and takes its centre.
///
/// The LOG form, the MEDIAN rather than the mean, and the `1e-5` floor are
/// the white-balance solve's, for its reasons: the median commutes with a
/// monotone map so the log is free, a location statistic on one cloud is what
/// a global exposure is, and a newly bright region must not drag the whole
/// frame.
///
/// Rec.601 luminance, which is the weighting the marginal solve used and the
/// one the tone stage reads, so this changes the STATISTIC and not the
/// quantity it is a statistic of.
///
/// MEASURED_PLACEHOLDER
fn atmosphere_exposure_from_populations(
    sp: &[[f32; 3]],
    pair_tp: &[[f32; 3]],
    pair_w: &[f32],
) -> f32 {
    let luma = |p: &[f32; 3]| {
        0.299 * render::srgb_to_linear(p[0])
            + 0.587 * render::srgb_to_linear(p[1])
            + 0.114 * render::srgb_to_linear(p[2])
    };
    let n = sp.len().min(pair_tp.len());
    let mut samples: Vec<(f32, f32)> = Vec::with_capacity(n);
    for i in 0..n {
        let w = pair_w.get(i).copied().unwrap_or(0.0).max(0.0);
        if w <= 0.0 {
            continue;
        }
        samples.push((
            (luma(&pair_tp[i]).max(1e-5) / luma(&sp[i]).max(1e-5)).log2(),
            w,
        ));
    }
    weighted_median(&mut samples)
}

fn weighted_mean(px: &[[f32; 3]], weights: &[f32], ch: usize) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut total = 0.0f32;
    for (i, p) in px.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        sum += p[ch] * w;
        total += w;
    }
    (total > 1e-8).then_some(sum / total)
}

fn weighted_mean_chroma(px: &[[f32; 3]], weights: &[f32]) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut total = 0.0f32;
    for (i, p) in px.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        sum += (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) * w;
        total += w;
    }
    (total > 1e-8).then_some(sum / total)
}

impl Divergence {
    fn matched() -> Self {
        Self { correlation: 1.0, energy_error: 0.0, d: 0.0 }
    }
}

/// Tone-invariant structural comparison shared by the global and zoned
/// solvers. Luma is rank-equalized through a mask-weighted 1024-bin CDF; a
/// three-pixel erosion keeps semantic-mask boundaries out of the reading.
/// Central-difference rank-gradient maps are pooled at sigma 2 and correlated
/// over translations of +/-6 pixels. Five Gaussian bands (sigma 1/2/4/8/16)
/// contribute the RMS log2 energy-ratio error.
pub fn structure_divergence(
    src_px: &[[f32; 3]],
    tgt_px: &[[f32; 3]],
    w: u32,
    h: u32,
    weights: &[f32],
) -> Divergence {
    let n = w as usize * h as usize;
    if n == 0 || src_px.len() != n || tgt_px.len() != n || weights.len() != n {
        return Divergence::matched();
    }

    let rank_equalized = |px: &[[f32; 3]]| -> Vec<f32> {
        let mut hist = [0.0f64; HIST_BINS];
        let mut bins = Vec::with_capacity(n);
        for (p, &weight) in px.iter().zip(weights) {
            let bin = (luma601(p).clamp(0.0, 1.0) * (HIST_BINS - 1) as f32).round() as usize;
            bins.push(bin);
            hist[bin] += weight.max(0.0) as f64;
        }
        let total = hist.iter().sum::<f64>();
        if total <= 1e-12 {
            return vec![0.0; n];
        }
        let mut acc = 0.0f64;
        for v in &mut hist {
            acc += *v;
            *v = acc / total;
        }
        bins.into_iter().map(|i| hist[i] as f32).collect()
    };

    let mut core: Vec<bool> = weights.iter().map(|&v| v > 0.8).collect();
    let (wu, hu) = (w as usize, h as usize);
    for _ in 0..3 {
        let mut next = core.clone();
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                next[i] = x > 0
                    && x + 1 < wu
                    && y > 0
                    && y + 1 < hu
                    && core[i]
                    && core[i - 1]
                    && core[i + 1]
                    && core[i - wu]
                    && core[i + wu];
            }
        }
        core = next;
    }
    if core.iter().filter(|&&v| v).count() < 100 {
        return Divergence::matched();
    }

    let gaussian_blur = |input: &[f32], sigma: f32| -> Vec<f32> {
        let radius = (3.0 * sigma).ceil().max(1.0) as isize;
        let mut kernel = Vec::with_capacity((2 * radius + 1) as usize);
        for i in -radius..=radius {
            kernel.push((-(i * i) as f32 / (2.0 * sigma * sigma)).exp());
        }
        let sum: f32 = kernel.iter().sum();
        for v in &mut kernel {
            *v /= sum;
        }
        let mut tmp = vec![0.0f32; n];
        let mut out = vec![0.0f32; n];
        for y in 0..hu {
            for x in 0..wu {
                let mut v = 0.0f32;
                for (ki, &kv) in kernel.iter().enumerate() {
                    let sx = x as isize + ki as isize - radius;
                    if (0..wu as isize).contains(&sx) {
                        v += input[y * wu + sx as usize] * kv;
                    }
                }
                tmp[y * wu + x] = v;
            }
        }
        for y in 0..hu {
            for x in 0..wu {
                let mut v = 0.0f32;
                for (ki, &kv) in kernel.iter().enumerate() {
                    let sy = y as isize + ki as isize - radius;
                    if (0..hu as isize).contains(&sy) {
                        v += tmp[sy as usize * wu + x] * kv;
                    }
                }
                out[y * wu + x] = v;
            }
        }
        out
    };

    let signature = |rank: &[f32]| -> (Vec<f32>, [f32; 5]) {
        let blurred: Vec<Vec<f32>> =
            [1.0, 2.0, 4.0, 8.0, 16.0].iter().map(|&s| gaussian_blur(rank, s)).collect();
        let mut energy = [0.0f32; 5];
        let count = core.iter().filter(|&&v| v).count() as f64;
        for band in 0..5 {
            let mut sum = 0.0f64;
            for i in 0..n {
                if !core[i] {
                    continue;
                }
                let v = if band == 0 {
                    rank[i] - blurred[0][i]
                } else {
                    blurred[band - 1][i] - blurred[band][i]
                };
                sum += (v * v) as f64;
            }
            energy[band] = (sum / count).sqrt() as f32;
        }
        let mut gradient = vec![0.0f32; n];
        for y in 1..hu.saturating_sub(1) {
            for x in 1..wu.saturating_sub(1) {
                let i = y * wu + x;
                let dx = 0.5 * (rank[i + 1] - rank[i - 1]);
                let dy = 0.5 * (rank[i + wu] - rank[i - wu]);
                gradient[i] = (dx * dx + dy * dy).sqrt();
            }
        }
        (gaussian_blur(&gradient, 2.0), energy)
    };

    let src_rank = rank_equalized(src_px);
    let tgt_rank = rank_equalized(tgt_px);
    let (src_gradient, src_energy) = signature(&src_rank);
    let (tgt_gradient, tgt_energy) = signature(&tgt_rank);

    let mut best = -1.0f64;
    for dy in -6isize..=6 {
        for dx in -6isize..=6 {
            let mut count = 0usize;
            let (mut sx_sum, mut ty_sum) = (0.0f64, 0.0f64);
            for y in 0..hu {
                for x in 0..wu {
                    let i = y * wu + x;
                    let sy = y as isize - dy;
                    let sx = x as isize - dx;
                    if core[i]
                        && (0..hu as isize).contains(&sy)
                        && (0..wu as isize).contains(&sx)
                    {
                        sx_sum += src_gradient[sy as usize * wu + sx as usize] as f64;
                        ty_sum += tgt_gradient[i] as f64;
                        count += 1;
                    }
                }
            }
            if count < 100 {
                continue;
            }
            let (sx_mean, ty_mean) = (sx_sum / count as f64, ty_sum / count as f64);
            let (mut cross, mut sa, mut sb) = (0.0f64, 0.0f64, 0.0f64);
            for y in 0..hu {
                for x in 0..wu {
                    let i = y * wu + x;
                    let sy = y as isize - dy;
                    let sx = x as isize - dx;
                    if core[i]
                        && (0..hu as isize).contains(&sy)
                        && (0..wu as isize).contains(&sx)
                    {
                        let a = src_gradient[sy as usize * wu + sx as usize] as f64 - sx_mean;
                        let b = tgt_gradient[i] as f64 - ty_mean;
                        cross += a * b;
                        sa += a * a;
                        sb += b * b;
                    }
                }
            }
            let den = (sa * sb).sqrt();
            if den > 0.0 {
                best = best.max(cross / den);
            }
        }
    }
    let correlation = if best > -1.0 { best as f32 } else { 1.0 };
    let energy_error = (src_energy
        .iter()
        .zip(tgt_energy)
        .map(|(&a, b)| ((b + 1e-6) / (a + 1e-6)).log2().powi(2))
        .sum::<f32>()
        / 5.0)
        .sqrt();
    let d = ((1.0 - correlation).powi(2) + energy_error.powi(2)).sqrt();
    Divergence { correlation, energy_error, d }
}

/// The common, pixel-aligned 384×256 analysis raster used by both scopes and
/// by the calibration prototype. Both sides are Lanczos-resampled onto that
/// one grid and the source is placed in the calibration/base domain before
/// ranks are measured.
pub(crate) fn divergence_raster(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, u32, u32) {
    let (w, h) = (ANALYZE_EDGE, ANALYZE_EDGE * 2 / 3);
    let src_grid = src.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let tgt_grid = target.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let src_base = render::develop_preview(&src_grid, base);
    (pixels_of(&src_base), pixels_of(&tgt_grid), w, h)
}

pub(crate) fn structure_divergence_for(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
    weights: Option<&[f32]>,
) -> Divergence {
    let (sp, tp, w, h) = divergence_raster(src, target, base);
    let all;
    let weights = match weights {
        Some(v) => v,
        None => {
            all = vec![1.0; sp.len()];
            &all
        }
    };
    structure_divergence(&sp, &tp, w, h, weights)
}
/// Quantile clip for CDF inversion — the extreme tails of a generative render
/// are noise (a few blown/crushed pixels would otherwise own the end knots).
pub(crate) const P_CLIP: f32 = 0.002;
/// Ceiling on the gated tone evidence's MISPREDICTION of its own identified
/// population (see [`neutral_gate_misprediction`]) before the assumption is
/// declared dead and the solve falls back to full-pixel CDFs. Membership
/// counts and one-sided CDF-shift proxies were both tried and both mis-rank
/// the live pairs (each fires HARDER on the haze pair than on the pair that
/// actually shipped a murky fit), so the gate is judged by the harm itself:
/// how far its map misses the pixels it claims to identify, in luma units.
/// Anchors, all measured. Fall-back side: P20 × reimagine reads 0.021
/// (the pale sky, luma q50 ≈ 197/255, re-hued vivid blue out of the class;
/// share ratio 1.29× sailed under the 1.75× gate; the shipped map missed
/// the shared class by −22/255 right in the murk band); the archived
/// P21 pairs read 0.074 (× reimagine, the golden sky — share 2.65×,
/// both gates fire), 0.034 (× reimagine-4 — share 1.51×, UNDER the share
/// gate: this detector is the only defence) and 0.024 (× reimagine-2,
/// share 1.92×); the haze fixture reads 0.126 (its blue cast tints the
/// clean side's dark greys out of the class — under the R17 dense residual
/// knots the gated solve faithfully implements that broken map and
/// collapses to a do-no-harm reset, while the fallback lands
/// 0.0892 → 0.0229). Keep side: P21 × reimagine-3 — a REAL benign
/// pair — reads 0.0050 (share 1.12×), the identity / canyon fixtures read
/// ≈ 0 (matched members) and the synthetic uniform-inflation fixture reads
/// < 0.0075. The 0.015 ceiling thus has real pairs on BOTH flanks: 3.0×
/// clear below (0.0050), 1.4× above (0.021). The archive numbers above are
/// the embedded-preview domain (`decode` output vs reimagine target — the
/// camera-look source the CLI `match` feeds this gate); the GUI's
/// composed-calibration domain was measured separately for all five real
/// pairs (R19, the repro test prints it per pair) and each verdict lands
/// on the same side of the ceiling in both domains — composed readings:
/// P20 × re2 0.024, P21 × re 0.043, × re2 0.033, × re4 0.036
/// (fall-back side), × re3 0.0131 (keep side, a 1.15× margin against the
/// preview domain's 3.0×). The haze fixture is a recipe-render pair with
/// no RAW, so a composed domain does not exist for it.
#[cfg(test)]
const NEUTRAL_MISPREDICTION_MAX: f32 = 0.015;
/// Evidence floor for the SHARED class inside
/// [`neutral_gate_misprediction`] — the same absolute floor the per-side
/// `enough` bar uses (512 px), plus the same 5%-of-frame scaling, applied
/// to the one population the identification assumption is actually about.
/// Below it there is no identified population to score and the metric
/// reports infinite (fall back).
const NEUTRAL_SHARED_MIN: usize = 512;
/// Cast-curve acceptance ANCHOR: the shipped-default value of
/// [`FitBudget::cast_ratio`], which the strength budget interpolates between
/// 1.5 and 3.0. The ratio arm compares the with-curves look error against
/// `budget.cast_ratio` × the without-curves error — but it is not a hard
/// admission threshold and never was: it rejects only when the evidence is
/// also unidentifiable (`identifiability < 0.25`), because a content mismatch
/// masquerading as a cast is a thing you can only diagnose when the pair is
/// too unidentifiable to trust the aggregate. So an ADMITTED cast's ratio may
/// legitimately exceed this number, and any prose that says "the curves cut
/// the error to at most X" is wrong.
///
/// Read straight only by [`cast_gate_outcome`]; the shipped path reads
/// `budget.cast_ratio`, so a test asserting against THIS constant is not
/// asserting against the gate the fit used at any strength but the default.
const CAST_ACCEPT_RATIO: f32 = 2.0;

// --- cast foreign-hue veto (the second, pixel-aligned gate) -----------------
// The aggregate ratio above is structurally blind to a CROSS-BAND hue wreck:
// rotate a small pale sky from blue into violet and its band mass lands in
// Purple/Magenta (empty in the target → the hue term's two-sided weight gate
// skips it) while draining out of Blue (below the gate in the fitted render →
// also skipped) — the hue term sees nothing, and the tonal+colour win on the
// frame-dominant region sails the curves through (real-machine canyon
// failure, 2026-07-09; reproduced by
// `warm_rock_cast_must_not_violet_the_pale_sky`).
//
// The veto exploits what the aggregate cannot: the renders WITHOUT and WITH
// the curves come from the SAME source, so "what did the curves do" is
// exact, not statistical. The verdict is by HUE DISTANCE: a pixel the curves
// leave visibly tinted at a hue ≥ [`VETO_FAR_BINS`]·15° away from EVERY hue
// the target populates is FOREIGN — reject the curves when they grow the
// frame's foreign share by ≥ [`VETO_CREATED_SHARE`].
//
// Distance is the discriminator the failure data demanded (both probed on
// the live pairs): the canyon violet lands 60-100° from everything the
// target contains, while the haze correction's imperfect residuals scatter
// pixels only 5-40° off the target's own orange/green/blue mass — so
// family-membership rules at ANY granularity mis-classify one side or the
// other (a ±15° window scored the haze fix 15% "damage"; whole-band shares
// flagged its 35-45° orange-yellow skirt as a phantom yellow family), but a
// 45° = 1.5-ACR-band radius separates them with a 20°+ margin either way.
// Measuring the DELTA against the without-render keeps pre-existing content
// mismatch (already-foreign pixels the curves didn't create) out of the
// verdict.
//
// A cast that rotates a region into a hue the target DOES contain elsewhere
// (sky turned rock-gold) passes this veto BY DESIGN — that failure class is
// covered by the rotation budget below, added when it materialised on a real
// pair (2026-07-09 #2, P21 × reimagine-5: the hazy pale-blue sky was
// re-hued ~170° into the target's own vivid orange; both earlier gates
// passed — the destination hue was target-native and the frame-dominant win
// carried the aggregate).

/// Target pixels feeding the hue-support bins must clear this chroma —
/// deliberately BELOW the 0.06 band-stats gate so a pale sky still testifies.
const VETO_SUPPORT_CHROMA: f32 = 0.03;
/// Below this many chromatic target pixels there is no reliable hue evidence
/// (e.g. a monochrome target) — the veto stands down.
const VETO_MIN_TARGET_CHROMATIC: usize = 500;
/// A pixel is "visibly tinted" at/above this chroma and enters the foreign
/// census. Below the renderer's HSL fade-in (0.05), but a contiguous 0.04
/// tint over a sky-sized region is visible.
const VETO_TINT_CHROMA: f32 = 0.04;
/// A 15° hue bin is "populated" when it holds ≥ this share of the target's
/// chromatic mass. Chroma noise spread over 24 bins stays well under this;
/// any hue region the target actually contains clears it severalfold.
const VETO_SUPPORT_BIN_MIN: f32 = 0.015;
/// Foreign radius in 15° bins: ±3 bins = ±45° = 1.5 ACR bands. The canyon
/// violet sits 60°+ from all target hues; the haze residuals sit ≤ 40° from
/// the target's own families — 45° splits them with margin on both sides.
const VETO_FAR_BINS: usize = 3;
/// Foreign frame-share the curves must CREATE (with − without) to be
/// rejected: 5% of the frame is a REGION (the canyon sky measures ~12-15%),
/// not boundary speckle (the haze pair measures ≈ 0.04%).
const VETO_CREATED_SHARE: f32 = 0.05;

// --- cast rotation budget (the third gate) ----------------------------------
// The foreign-hue veto cannot see a rotation whose DESTINATION the target
// populates (see above). The rotation budget closes that hole from the
// pixel-aligned side alone: a pixel visibly tinted BOTH before and after the
// curves that lands ≥ [`ROT_DEG`] away has been RE-HUED, not corrected; when
// a region-sized share of the frame is re-hued, the curves are a regional
// regrade masquerading as a global cast — reject. Measured on the live pairs
// (calibration probe, 2026-07-09): the accepted haze correction moves 0.01%
// of the frame past 60° (its cast-dominated pixels stay under); the violet
// canyon rotates 12.5% of the frame by 112°; the golden-sky canyon by ~170°.
// 75° sits 15° above the measured-legit ceiling and 37° below the smallest
// observed wreck.
//
// Deliberate cost: a HEAVY global cast (strong tungsten drift) whose honest
// correction would rotate still-tinted pixels past 75° is refused too — the
// fit then under-corrects (tone + saturation only) rather than risk a
// regional re-hue it cannot statistically tell apart. A conservative miss is
// recoverable in the develop panel; a re-hued region is not. True regional
// regrades (a sky genuinely gone gold) belong to the zoned fit, not to
// global curves.

/// Circular hue distance (degrees) beyond which a still-tinted pixel counts
/// as re-hued rather than corrected.
const ROT_DEG: f32 = 75.0;
/// Share of re-hued (or blindly moved) pixels that constitutes a REGION of
/// the population a correction moves -- the frame for the global fit, a
/// zone's coverage for a zone (same region-vs-speckle logic as
/// [`VETO_CREATED_SHARE`]; the live wrecks measure 12.5%).
const ROT_SHARE: f32 = 0.05;
/// A rotation only counts as a re-hue when it is VISIBLE on at least one
/// end: before-chroma ≥ this (a tinted pixel moved) or after-chroma ≥
/// [`ROT_VISIBLE_AFTER`] (a faint pixel painted vivid — the H17 class). A
/// cast INVERSION passing through neutral flips the hue of a sub-visible
/// tint into another sub-visible tint — measured on the haze pair after the
/// R17 tone-evidence fallback strengthened its correction: 4.5% of the
/// frame, before-chroma < 0.05 to the last pixel, after-chroma ≤ 0.082 —
/// an invisible "rotation" on both ends that ate the veto's whole margin
/// while wrecking nothing. The real wrecks stay above the exemption on the
/// end that matters: H17 paints 0.34 after-chroma from a 0.035 tint; the
/// canyon rotations start from ≥ 0.05 before-chroma.
#[cfg(test)]
const ROT_VISIBLE_BEFORE: f32 = 0.05;
/// See [`ROT_VISIBLE_BEFORE`]: the after-side visibility floor — just above
/// the measured pass-through band (≤ 0.082), 3.8× under H17's 0.34, and
/// deliberately NOT higher: every step up widens the exempt band. Rotations
/// inside `cc ∈ [0.03, 0.05) × wc ∈ [0.04, 0.09)` — a faint tint re-hued
/// into another faint tint, invisible on BOTH ends — are exempt by design,
/// and the band's exact borders are pinned by the
/// `the_pass_through_exemption_borders_are_patrolled` fixture; if a real
/// pair ever wrecks a region at those chroma levels, this pair of floors
/// is the suspect.
#[cfg(test)]
const ROT_VISIBLE_AFTER: f32 = 0.09;
/// The BEFORE side of the rotation census needs only a MEASURABLE hue, not a
/// visible tint: requiring [`VETO_TINT_CHROMA`] on both sides let the curves
/// rotate a region whose chroma sat just UNDER that gate (a barely-blue sky
/// at 0.035) into a strong target-native colour without the census ever
/// seeing it — the golden-sky class again, one threshold to the left.
/// CALIBRATED at 0.03 (the [`VETO_SUPPORT_CHROMA`] "a pale sky still
/// testifies" level) by the haze regression itself: at 0.015 the haze
/// correction's legitimately-restored faint pixels measured a 0.0414 census
/// share — 0.83× the firing threshold, margin gone — because hue is
/// genuinely unstable that close to neutral. Below 0.03 chroma the census
/// abstains BY DESIGN, not by omission: colourising near-neutrals is
/// exactly what a corrective cast legitimately does (the haze pair), and
/// at the measured 0.015 level a rotation verdict is demonstrably noise
/// convicting that feature (the 0.83× margin collapse above).
const ROT_HUE_MEASURABLE_CHROMA: f32 = 0.03;

// --- the hue-FAN gate (the fourth gate) -------------------------------------
// The three gates above all ask about a pixel's DESTINATION: is it far from
// where it started (rotation budget), is it somewhere the target holds no
// colour (foreign-hue veto), did the aggregate improve (ratio). None of them
// can see the one thing three INDEPENDENT monotone channel maps do that no
// hue-preserving control can: they sort a single-hued region into several
// hues BY LUMINANCE. Each pixel's own rotation stays small, every
// destination is target-native, and the region's mean hue barely moves —
// because the slices rotate in OPPOSITE directions and the circular mean
// cancels them.
//
// Measured on the Cornwall reverse-fit pair (2026-09-01, the defect v1.2.2
// shipped and registered): the admitted curves leave the sky's mean hue at
// 218.3° → 217.6° (0.7°, invisible), rotate no pixel past 75° at all
// (`rehued_share_weighted` = 0.000000, unweighted 0.0058), create 0.000000
// foreign share and cut the look error nearly in half (0.0576 → 0.0334,
// ratio 0.580) — and split the sky's hue across luminance from a 1.6° spread
// to 33.1° in the delivered render: the dark half lands at 226.8° (violet),
// the bright clouds at 193.8° (green-cyan). That is the tint the showcase
// caption reports, and every existing gate reads clean on it.

/// Hue classes for the fan census: the foreign-hue veto's 15° bins, so the
/// two colour gates partition hue on one grid.
///
/// The grid has a fixed PHASE, and that is a stated sensitivity, not an
/// oversight: a coherent region whose hue straddles a class edge splits
/// across two classes, each holding half its mass, and can fall under
/// [`FAN_SHARE`] and go unjudged. The precedent is the foreign-hue veto,
/// which has read hue on this same fixed grid since it was written; a
/// phase-free alternative (sliding windows, or clustering the hue histogram)
/// would buy edge-invariance at the cost of a census whose population is no
/// longer identical to the rotation budget's — and that identity is what
/// stops the two gates drifting into disagreeing about WHICH pixels they
/// read. Cornwall's convicted class sits at 0.917 of the population, nowhere
/// near an edge, so the wreck is not a marginal case of this.
const FAN_HUE_CLASSES: usize = 24;

/// A hue class — and a luma slice inside it — must be a REGION of its own
/// population before its mean hue counts as evidence. Deliberately the same
/// number as [`ROT_SHARE`] and [`VETO_CREATED_SHARE`] (0.05 = region, not
/// speckle) and deliberately its own constant: those three share-gated
/// censuses (there are FOUR cast gates; the aggregate-ratio one has no share
/// term) answer different questions, and a retune of one must not silently
/// move the others.
///
/// CALIBRATED by sweep (2026-09-01, `hue_fan_weighted` on the four
/// calibration pairs plus Cornwall). The slice floor is on a plateau here:
/// at 0.02 the readings are Cornwall 37.6° / canyon-warm 9.6° / haze 7.8°,
/// at 0.05 Cornwall 37.6° / canyon-warm 7.5° / haze 7.8°, at 0.10 Cornwall
/// 24.5° / canyon-warm 2.9° / haze 7.8°. 0.05 keeps the wreck's full
/// reading (0.10 loses a third of it to slices that fall under the floor)
/// while reading the same 7.8° on the accepted haze correction — the widest
/// separation of the three.
const FAN_SHARE: f32 = 0.05;

/// Added hue spread (degrees) inside one class that convicts the curves.
///
/// CALIBRATED, not chosen, and exactly one hue class wide: the slices have
/// to land in DIFFERENT bins of the census's own 15° grid before a fan is
/// resolvable at all, so anything under this is inside the grid's
/// quantisation and must not convict.
///
/// `hue_fan_weighted` measures, on the analysis raster the fit itself uses:
/// Cornwall (the wreck) 37.6°, the synthetic Cornwall-shape fixture 44.6°,
/// and on the pairs whose curves the other three gates legitimately accept
/// or reject — the haze regression (ACCEPTED, and the one that matters:
/// this gate must not touch it) 7.8°, canyon-warm 7.5°, canyon-gold 5.2°,
/// hazy→vivid 2.7°, an identical pair 0.0°. 15° sits 1.9× above the largest
/// legitimate reading and 2.5× below the wreck.
///
/// The threshold is verified END TO END, not just on the census: at 20° the
/// Cornwall solve does not refuse outright — the mixer's do-no-harm loop
/// halves Aqua/Blue and refits until a milder cast measures 19°, which
/// ships and still leaves a 20.6° fan in the delivered sky (the violet is
/// gone, a pale green in the bright cloud is not). At 15° the refusal
/// stands: the delivered sky's hue spread across luminance octiles is 1.6°,
/// the same coherence the TARGET's sky has, at a look error of 0.058
/// instead of 0.033.
///
/// Deliberate cost, the same shape as the rotation budget's: a cast whose
/// honest correction genuinely needs different hue movement at different
/// luminances (a scene lit by two sources of different colour temperature)
/// is refused too. The fit then under-corrects — tone, saturation and the
/// per-band mixer only — rather than ship a region sorted into a hue fan a
/// user cannot undo with any single develop control.
///
/// WORST CASE, stated because the gate judges the ADDED spread and a reader
/// will otherwise take this for the delivered one. The class is a 15° bin of
/// the BEFORE hue, so the baseline the census subtracts is itself bounded by
/// one class width — and one class width IS this number. An admitted cast
/// can therefore leave up to `2 × FAN_DEG` ≈ 30° of ABSOLUTE hue spread
/// inside the class, when the class arrived already spread across its own
/// bin and the curves add just under the limit on top. That is the gate's
/// tolerance, and it is asserted rather than promised: see
/// `an_admitted_cast_delivers_at_most_two_class_widths_of_hue_fan`, which
/// pins the admitted haze pair's DELIVERED in-class spread under 2 ×
/// FAN_DEG.
///
/// It is also a calibrated threshold and not a structural guarantee: the
/// mixer's do-no-harm loop re-fits after every shrink, so the solve can
/// search for a cast that clears the limit rather than give the cast up —
/// which is exactly what the 20° experiment above shows it doing.
const FAN_DEG: f32 = 15.0;

/// The fan a PROJECTED cast must clear — half [`FAN_DEG`], deliberately not
/// [`FAN_DEG`] itself.
///
/// [`FAN_DEG`] is the REFUSAL line and it sits at the visibility edge: the
/// FAN_DEG = 20 experiment shipped a cast measuring 19° that left 20.6° of
/// fan in the delivered sky (the violet gone, a pale green in the bright
/// cloud not), and the widest reading the gate admits on its own merits is
/// the haze correction's 7.8°. A cast the fit CHOOSES to keep by shrinking
/// it is not entitled to sit on the edge the gate merely tolerates — it has
/// to be no worse than what the gate already passes unprojected. Half the
/// refusal line is 7.5°, just under that 7.8°: THAT is the calibration.
///
/// Both candidate targets were measured end to end on the Cornwall pair
/// before this number was fixed (2026-09-02, global stage, `match` without
/// `--zoned`), because "shrink until the fan clears 15°" is the obvious
/// alternative and it had to be refuted with numbers rather than taste:
///
/// | target | t | census fan after | look error | confidence | DELIVERED sky spread |
/// |--------|------|------|---------------|--------|--------|
/// | 7.5° (shipped) | 0.363 | +7° | 0.137 → 0.030 | 0.664 | 10.5° |
/// | 15° | 0.483 | +14° | 0.137 → 0.026 | 0.680 | 15.3° |
///
/// against the target's own 1.6°, the refuse-outright branch's 1.6° at look
/// error 0.058, and v1.2.2's 33.1° at 0.033. The looser target buys 0.004 of
/// look error by delivering half again as much fan as the visibility
/// calibration allows, so the conservative constant stands.
///
/// See [`projected_cast_curves`] for the path and [`search_cast_projection`]
/// for the search.
const FAN_PROJECT_DEG: f32 = FAN_DEG / 2.0;
/// Cells the projection's gain sweep divides the admissible interval into.
/// The gain wiggles over `t` — 0.00104 / 0.00190 / 0.00169 / 0.00187 at
/// `t` 0.25 / 0.35 / 0.40 / 0.50 on the coast candidate — so the grid has to
/// be fine enough to bracket an interior peak, and every probe costs a full
/// render, so it is not made finer than the structure it has to see. Eight
/// cells put each probe 0.05 of `t` apart on a frontier at 0.4, half the
/// spacing of that measured wiggle.
const PROJECT_GRID: usize = 8;
/// Golden-section iterations on the winning cell. Each iteration multiplies
/// the bracket by 0.618, so eight of them take a cell 0.05 wide down to
/// 0.0011 of `t` — just past the third decimal the projection note prints,
/// which is as far as refining can change anything the user reads.
const PROJECT_REFINE: usize = 8;

// --- the REPORTED-CONFIDENCE family (R23-6) ---------------------------------
// One calibration with two ends, named as one so they can never be retuned
// apart again. They were two bare literals — `(1.0 - err * 6.0).clamp(0.25,
// 0.95)` at the bottom of `fit_recipe_from` and `err_after > 0.12` on the FAR
// warning — and their relationship was invisible: the slope drives confidence
// onto its floor at err = (1 − 0.25)/6 = 0.125, so the FAR line is the same
// point, rounded down. That coincidence is not decoration; it means "the
// number bottomed out" and "the warning fires" are ONE decision expressed
// twice, and R17's real pair sits under BOTH (err_before 0.0947 → 0.0267),
// which is exactly why neither ever fired on the fit the user called
// nonsense. `the_confidence_family_is_one_calibration` pins the relation.
//
// The slope stays 6.0: this round does not retune the look-error ladder (it
// would need the real failure pair that has not arrived). What changes is
// that this ladder is no longer the ONLY thing allowed to set the number —
// see `fit_zoned::JOINT_CONFIDENCE_SLOPE`, which can only lower it.

/// Confidence per unit of look error.
const CONFIDENCE_SLOPE: f32 = 6.0;
/// Never claim less than this — a fit that lands far is still a fit, and 0
/// would read as "broken" rather than "approximate".
const CONFIDENCE_FLOOR: f32 = 0.25;
/// Never claim more than this: a statistical match against a non-aligned
/// target is never certain, whatever the residual says.
const CONFIDENCE_CEIL: f32 = 0.95;
/// Exponential confidence penalty per mean RGB unit moved where no evidence
/// survived. Calibrated on the generated-sky pair: 0.1186 unsupported motion
/// versus 0.0413 for the preferred saved render.
const UNSUPPORTED_MOVEMENT_CONFIDENCE_SLOPE: f32 = 8.0;
/// The FAR line — the residual at which [`CONFIDENCE_SLOPE`] has already
/// driven confidence onto [`CONFIDENCE_FLOOR`] ((1 − 0.25)/6 = 0.125),
/// rounded down to a legible number.
const FIT_FAR_ERR: f32 = 0.12;

/// The one clamp both confidence ladders (this module's and the zoned one's)
/// pass through, so the floor and ceiling are stated once.
pub(crate) fn clamp_confidence(v: f32) -> f32 {
    v.clamp(CONFIDENCE_FLOOR, CONFIDENCE_CEIL)
}

/// Confidence from the frame-global look error.
fn confidence_from_look_err(err: f32) -> f32 {
    clamp_confidence(1.0 - err * CONFIDENCE_SLOPE)
}

/// Aspect-ratio disagreement past which the reference's pixel population is no
/// longer this photo's (R23-6 B-7). 2% mirrors the grid-comparability rule
/// inside [`neutral_gate_misprediction`] — a few rows of a 384-edge thumbnail,
/// i.e. beyond what aspect ROUNDING explains.
///
/// A crop is NOT inside that budget, deliberately, and the doc used to imply it
/// was (R23 review LOW-3): 3:2 recomposed to 16:9 is 18% out and trips this
/// every time, on exactly the Lightroom/C1 export the tooltip asks for. The
/// reading stays correct — a crop changes which pixels the statistics are taken
/// over, so the two distributions stop being comparable — but the message it
/// drives has to say CROP rather than accuse the user of picking the wrong
/// file. A WARNING, never a refusal: the reference is a file the user chose on
/// purpose, and an unreliable fit is not an illegal one.
const SAME_FRAME_ASPECT_TOL: f32 = 0.02;

/// Do these two images plausibly show the SAME frame? `false` ⇒ warn.
///
/// ONE reading: the aspect ratios, which is all this function has ever
/// computed. (This doc used to promise a second — the grid comparability
/// [`neutral_gate_misprediction`] returns infinity for. That test is real, but
/// it belongs to `tone_cdf_pair`'s neutral-evidence gate and nothing routes its
/// answer here, so the promise was fiction — R23 round review LOW-2.)
///
/// One reading is enough for what this result is ALLOWED to do. It is a
/// necessary condition and never a sufficient one — a different photograph of
/// the same scene at the same aspect passes, and nothing short of registration
/// would catch it — which is exactly why the caller warns instead of refusing.
pub fn same_frame_plausible(src: &DynamicImage, target: &DynamicImage) -> bool {
    same_frame_plausible_dims((src.width(), src.height()), (target.width(), target.height()))
}

/// [`same_frame_plausible`] on dimensions alone — the ONE aspect rule every
/// "is this the sensor frame?" question in the crate reads: the reference
/// check above, `reimagine`'s choice of input frame, `match`'s choice of
/// source frame and the base-look estimator's pairing (v1.2.2, the in-camera
/// aspect-crop class: a body set to 4:3 writes a centred 4:3 preview over its
/// 3:2 sensor). One tolerance, so a frame cannot be "the same" to one
/// consumer and "cropped" to another.
pub fn same_frame_plausible_dims(a: (u32, u32), b: (u32, u32)) -> bool {
    let ar = |(w, h): (u32, u32)| w.max(1) as f32 / h.max(1) as f32;
    let (a, b) = (ar(a), ar(b));
    (a - b).abs() <= SAME_FRAME_ASPECT_TOL * a.max(b)
}

/// The two analysis rasters of a pair, in ONE geometry.
///
/// Every evidence statistic pairs source pixel `i` with target pixel `i`, and
/// [`structure_divergence`] refuses (returns `matched`, D = 0) when the two
/// rasters differ in length. Thumbnailing the two images independently let a
/// ONE-ROW difference decide that: a 1600x1067 source lands on 384x256 and a
/// 1600x1069 target on 384x257, the frame-wide divergence read as 0, the
/// same-content verdict came out true and no evidence range could ever be
/// withheld -- the calibration pair fitted 0.081 -> 0.018 with the gate silently
/// off, while the same pixels cropped to equal heights abstained at 0.057 (the
/// verdict the doctrine actually gives). So the target is thumbnailed into the
/// source's analysis geometry with the SAME operator the source went through:
/// `thumbnail(w, h)` is `resize_dimensions` + `thumbnail_exact`, image's box
/// filter where every full-resolution pixel lands in exactly one cell, so an
/// equal-shape pair is byte-for-byte the two thumbnails it always was, by
/// construction and not by a branch. The operator matters: a Lanczos3
/// `resize_exact` of the target against a box-filtered source keeps more
/// high-frequency energy on one side than the other, and on a same-scene
/// pair (a 1536x1027 preview against its 9504x6336 develop) that asymmetry
/// alone moved the fit from 0.092 -> 0.019 / conf 0.68 to 0.107 -> 0.034 /
/// conf 0.54 with no range withheld on either arm.
pub(crate) fn analysis_pair(src: &DynamicImage, target: &DynamicImage) -> (DynamicImage, DynamicImage) {
    let s_img = src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
    let t_img = target.thumbnail_exact(s_img.width(), s_img.height());
    (s_img, t_img)
}

/// The most a fit may claim once [`same_frame_plausible`] has said no (R24
/// batch 2). The warning and this cap are ONE decision, for the same reason
/// [`FIT_FAR_ERR`] and [`CONFIDENCE_SLOPE`] are: printing "treat the result as
/// unreliable" beside a confidence of 0.83 states two contradictory things and
/// the user believes the number.
///
/// The 0.83 is real — measured on the cropped real pair of 2026-08-17
/// (`P43`, a portrait frame the user cropped to 1.294) — and so is the
/// blindness behind it: the joint reading handed that pair one of its BEST
/// scores (0.035), because value-range buckets correspond by value and simply
/// do not notice that the two populations came from different rectangles.
/// Neither of the fit's two readings can see this, so neither may set the
/// number.
///
/// WHY A CAP AND NOT THE FLOOR: at a 2% aspect tolerance the commonest
/// trigger is a crop of exactly the frame the user meant (see
/// [`SAME_FRAME_ASPECT_TOL`]), which is the Lightroom export the tooltip asks
/// for. Collapsing to [`CONFIDENCE_FLOOR`] would call the intended workflow
/// broken. A cap says "your numbers are not evidence", which is the true
/// claim, and leaves both ladders free to go LOWER when they have their own
/// reason to.
///
/// WHERE 0.5 COMES FROM — measured, not chosen. Forty crop-only pairs (eight
/// frames: four fixtures plus four of the real targets, each against five
/// centre-crops of ITSELF at 95/90/80/65/50% of height) put an IDENTICAL look
/// on both sides, so the truthful recipe is the identity and the truthful
/// residual is zero. The solver instead reported 0.796-0.950 confidence on
/// every one of them, and earned it by manufacturing real edits out of the
/// framing difference alone — up to +23.4 / −18.3 of saturation and ±0.45 EV.
/// The largest residual the framing alone manufactured was 0.0854, which
/// [`confidence_from_look_err`] reads as 0.488; 0.5 is that number, rounded to
/// a legible one. It is what the ladder itself says a fit is worth when its
/// whole residual could be framing.
const NOT_SAME_FRAME_CONFIDENCE_CAP: f32 = 0.5;

/// The fit outcome: the recipe plus the evidence-weighted tonal, colour, hue
/// and spatial error (0 = identical supported look) before and after.
pub struct FitReport {
    pub recipe: EditRecipe,
    pub err_before: f32,
    pub err_after: f32,
    /// Global solve policy selected before any CDF fitting.
    pub mode: FitMode,
    /// Structural reading that selected `mode` (promotion may select
    /// Atmosphere even when this frame-global value is below its threshold).
    pub divergence: Divergence,
    /// The rationale as typed notes (L12#2B): `render_en(&notes)` is the
    /// recipe's `rationale` byte-for-byte (empty prose prefix — the fit
    /// rationale is fully deterministic), so the GUI renders it localized
    /// while every persisted surface keeps the English string. In-process
    /// only, never serialized.
    pub notes: Vec<crate::rationale::Note>,
    /// Fixed source/target evidence for every downstream zoned gate and the
    /// finished-render disclosure. In-process only, never serialized.
    pub(crate) evidence: EvidenceModel,
    /// The structural frame model retained by an Atmosphere report for the two
    /// consumers for which structure remains a fact: Full zones and detail.
    /// `None` in Full mode; in Atmosphere mode [`Self::evidence`] is the
    /// structure-blind population ruler and this field is `Some(structural)`.
    pub structural_evidence: Option<EvidenceModel>,
    /// Cross-image correspondence for a content-divergent pair, when the
    /// caller supplied a provider and the D gate consulted it (step 7b).
    /// In-process only, never serialized — the zoned passes read it.
    pub(crate) correspondence: Option<PairCorrespondence>,
    /// R30 R2: which population this report's Atmosphere white balance and
    /// exposure were read over. In-process only, never serialized; the
    /// consultation site reads it to tell R2-lite's unpaired share from an
    /// EXCLUDED one. Always [`AtmosphereReference::WholeFrame`] in Full mode,
    /// where the two controls do not exist.
    pub(crate) atmosphere_reference: AtmosphereReference,
}

/// What one correspondence field means FOR THIS PAIR's rasters: for every
/// source-thumbnail index, the target pixel its content corresponds to and
/// how much that match can be trusted. Derived once from the sidecar's 48x48
/// field ([`correspondence_for_pair`]); consumed by the zone estimators as a
/// pair-weight factor and a remapped target.
pub(crate) struct PairCorrespondence {
    /// Per-source-index confidence in [0, 1] (cyclic x smoothness).
    pub(crate) conf: Vec<f32>,
    /// The target rendition READ AT the corresponded position, one sample per
    /// source index — same-index pairing against this array is
    /// correspondence-aware pairing against the original.
    pub(crate) tp: Vec<[f32; 3]>,
    /// Share of the frame with a confident counterpart (conf >= [`CONFIDENT_MATCH`]).
    pub(crate) coverage: f32,
    /// Median per-cell confidence, for the disclosure.
    pub(crate) median: f32,
    /// R2-lite: the share of the TARGET grid no confident source cell maps
    /// onto. [`Self::coverage`] is the mirror-image, SOURCE-side reading;
    /// this one is what the Atmosphere global solve's whole-frame target
    /// median is exposed to. Grid resolution, never pixel resolution.
    pub(crate) target_unpaired: f32,
    /// R30 R2: the same fact as [`Self::target_unpaired`], kept as the MASK
    /// it was counted from and not only as its share — one derivation, two
    /// consumers (the disclosure's number and the Atmosphere solve's
    /// reference population, which must never be able to disagree). `1.0`
    /// where some confident source cell maps ONTO the target grid cell
    /// holding this analysis pixel, `0.0` where none does. Indexed like
    /// [`Self::conf`]: the two analysis rasters share one geometry
    /// ([`analysis_pair`]), so one index names the same rectangle on both.
    pub(crate) target_answered: Vec<f32>,
    /// The sidecar grid the two shares above were counted on, so the
    /// disclosure can state its own resolution.
    pub(crate) grid: (usize, usize),
}

/// The confidence at which a correspondence cell counts as a real match.
/// Named because two shares are now read off it and they must agree on where
/// the line is.
pub(crate) const CONFIDENT_MATCH: f32 = 0.5;

/// A caller-supplied way to obtain a correspondence field for one pair —
/// the CLI and GUI hand in a closure that runs the local DIFT sidecar
/// (`correspond::fit_provider`); tests hand in stubs; `None` (or an `Err`)
/// degrades to the pre-7b behaviour. The fit consults it ONLY on a
/// content-divergent pair (`divergence.d >= DIVERGENCE_GLOBAL`) — the gate
/// lives here, single-sourced, not at the callers.
pub type CorrespondenceProvider<'a> =
    &'a dyn Fn(&DynamicImage, &DynamicImage) -> anyhow::Result<crate::correspond::CorrespondenceField>;

/// Project one sidecar field onto THIS pair's rasters. Pure geometry:
/// every source pixel centre lands in one grid cell; its confidence is that
/// cell's, and its corresponded target sample is read at the cell's mapped
/// position PLUS the pixel's own within-cell offset (locally the flow is
/// rigid — the 16-px cells are far below the scale content moves at).
///
/// Under an IDENTITY field (every cell maps to itself, full confidence) and
/// equal raster dims, the output target array is BYTE-IDENTICAL to the input
/// — the conservation law the wiring's tests pin: a field that says
/// "nothing moved, everything corresponds" must change nothing. When the two
/// rasters' dims DIFFER (the calibration target is two rows taller than its
/// source), same-index pairing carries a small row shear and the normalised
/// remap quietly corrects it — a real improvement measured at ~0.015 EV on
/// the calibration land zone, and the reason the conservation tests pin the
/// law on geometry-normalised fixtures.
pub(crate) fn correspondence_for_pair(
    field: &crate::correspond::CorrespondenceField,
    tp: &[[f32; 3]],
    (sw, sh): (u32, u32),
    (tw, th): (u32, u32),
) -> PairCorrespondence {
    let (gw, gh) = (field.grid_w, field.grid_h);
    let n = (sw * sh) as usize;
    let mut conf = vec![0.0f32; n];
    let mut out = vec![[0.0f32; 3]; n];
    for y in 0..sh {
        for x in 0..sw {
            let i = (y * sw + x) as usize;
            let u = (x as f32 + 0.5) / sw as f32;
            let v = (y as f32 + 0.5) / sh as f32;
            let cx = ((u * gw as f32) as usize).min(gw - 1);
            let cy = ((v * gh as f32) as usize).min(gh - 1);
            let c = cy * gw + cx;
            conf[i] = field.confidence[c];
            let fu = u * gw as f32 - cx as f32;
            let fv = v * gh as f32 - cy as f32;
            let un = ((field.map_x[c] + fu) / gw as f32).clamp(0.0, 1.0);
            let vn = ((field.map_y[c] + fv) / gh as f32).clamp(0.0, 1.0);
            let tx = ((un * tw as f32) as u32).min(tw - 1);
            let ty = ((vn * th as f32) as u32).min(th - 1);
            out[i] = tp.get((ty * tw + tx) as usize).copied().unwrap_or([0.0; 3]);
        }
    }
    let coverage = if conf.is_empty() {
        0.0
    } else {
        conf.iter().filter(|&&c| c >= CONFIDENT_MATCH).count() as f32 / conf.len() as f32
    };
    let mut sorted = conf.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    // R2-lite: the same field read from the OTHER side. `coverage` says how
    // much of the SOURCE has a counterpart; the Atmosphere solve's unstated
    // assumption runs the other way, because both its medians are read over
    // the whole TARGET. So count the target cells that some confident source
    // cell actually maps onto — a target cell no source cell answers for is a
    // population the ratio `median(target)/median(source)` has no partner
    // for. Grid resolution by construction; the disclosure states the grid.
    let cells = gw * gh;
    let mut answered = vec![false; cells];
    for c in 0..cells.min(field.confidence.len()) {
        if field.confidence[c] >= CONFIDENT_MATCH {
            let tx = (field.map_x[c].max(0.0) as usize).min(gw - 1);
            let ty = (field.map_y[c].max(0.0) as usize).min(gh - 1);
            answered[ty * gw + tx] = true;
        }
    }
    let target_unpaired = if cells == 0 {
        0.0
    } else {
        1.0 - answered.iter().filter(|a| **a).count() as f32 / cells as f32
    };
    // R30 R2: project that same bitmap onto the analysis raster by the
    // identical nearest-cell rule the loop above uses, so the population the
    // solve drops and the share the rationale prints are ONE fact.
    let mut target_answered = vec![0.0f32; n];
    for y in 0..sh {
        for x in 0..sw {
            let u = (x as f32 + 0.5) / sw as f32;
            let v = (y as f32 + 0.5) / sh as f32;
            let cx = ((u * gw as f32) as usize).min(gw - 1);
            let cy = ((v * gh as f32) as usize).min(gh - 1);
            if answered[cy * gw + cx] {
                target_answered[(y * sw + x) as usize] = 1.0;
            }
        }
    }
    PairCorrespondence {
        conf,
        tp: out,
        coverage,
        median,
        target_unpaired,
        target_answered,
        grid: (gw, gh),
    }
}

/// R30 R2: the least of its OWN evidence mass either side's shared-content
/// population may retain before the Atmosphere global solve refuses to read
/// its two robust controls there and keeps the whole-frame reading — with a
/// sentence saying it had to.
///
/// Not a new number, and deliberately not a copy of one: this IS
/// [`EVIDENCE_RANGE_SURVIVAL_MIN`], the retention floor the evidence model
/// already applies to every luma range and hue band ("a population must keep
/// at least the support left at `DIVERGENCE_ZONE`"), pointed at the one
/// population that had never been asked the question. If the evidence
/// doctrine ever moves that line, this moves with it.
pub(crate) const SHARED_POPULATION_MIN_RETENTION: f32 = EVIDENCE_RANGE_SURVIVAL_MIN;

/// R30 R2: WHICH population an Atmosphere report's white balance and exposure
/// were actually read over. A solve fact — no later re-measurement of the
/// finished recipe can recover it — and the thing the rationale states.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum AtmosphereReference {
    /// No usable correspondence field for this pair: the whole frame, with
    /// the distribution-pairing assumption R2-lite disclosed and nothing
    /// available to qualify it.
    WholeFrame,
    /// A field existed and the shared-content sub-population kept enough of
    /// BOTH sides' evidence mass to be read: the two medians came from it.
    /// The numbers are the retained shares of source and target evidence mass.
    SharedContent { source: f32, target: f32 },
    /// A field existed but one of the two sides retains less than
    /// [`SHARED_POPULATION_MIN_RETENTION`] of its own evidence mass — too
    /// little to be read as a population. The whole-frame medians stand and
    /// the report says why they had to.
    Thin { source: f32, target: f32 },
}

/// The two restricted weight vectors plus what each side kept.
pub(crate) struct SharedPopulation {
    source: Vec<f32>,
    target: Vec<f32>,
    source_retained: f32,
    target_retained: f32,
}

impl SharedPopulation {
    /// Both sides kept enough of their own evidence mass to be read as a
    /// population rather than as a corner of one.
    fn readable(&self) -> bool {
        self.source_retained >= SHARED_POPULATION_MIN_RETENTION
            && self.target_retained >= SHARED_POPULATION_MIN_RETENTION
    }
}

/// R30 R2: restrict the Atmosphere global solve's two reference populations
/// to the content the two frames actually SHARE.
///
/// `median(target) / median(source)` is a distribution-level pairing, and a
/// distribution-level pairing is only meaningful when the two distributions
/// describe the same content — precisely what selecting Atmosphere denies.
/// The correspondence field is the instrument that says which pixels do:
///
///   * TARGET side — a pixel some confident source cell maps ONTO is a
///     rendition of this frame. A pixel no cell answers for is not a rendition
///     of anything in it: it is generated content, and it cannot say what THIS
///     frame would look like developed differently. That is the population
///     R2-lite measured at 24% of the island pair's target and 93% of `p37`'s,
///     and the one its sentence says "defined those two controls all the same".
///   * SOURCE side — the mirror image, and NOT optional. Restricting only the
///     target moves one marginal onto a sub-population while the other stays
///     on the whole frame, and a ratio of medians read over two different
///     compositions is not a repair of the mismatched pairing, only a
///     different mismatch. Measured on a synthetic pair whose invented region
///     is the brighter 60% of the frame and therefore owns every whole-frame
///     median: the true cast is EV 0.00 / `gr/gb` 1.256, the whole-frame solve
///     answers +0.69 / 0.911, the target-only cut answers **−2.87 / 1.945**,
///     and the symmetric cut answers +0.03 / 1.216. The target-only failure is
///     larger than the defect it was meant to repair.
///
/// The cut is BINARY at [`CONFIDENT_MATCH`] rather than a confidence-
/// proportional down-weight, for a reason about what the number means: the
/// sidecar's confidence measures TRUST, not mass. Multiplying mass by trust
/// lets a large barely-trusted population outvote a small certain one (0.49
/// over 60% of a frame beats 1.00 over 20% of it) — the failure being
/// repaired, in a quieter form. Cutting at the line R2-lite already publishes
/// also keeps the disclosed share and the excluded population ONE fact.
///
/// Retention is measured on EVIDENCE MASS, not on pixel count: a pixel the
/// evidence model already weighted 0 was never in the reference population,
/// so it can neither be kept nor dropped from it.
///
/// `None` only when there is no evidence mass at all to restrict.
fn shared_content_population(
    evidence: &EvidenceModel,
    c: &PairCorrespondence,
) -> Option<SharedPopulation> {
    let n = evidence
        .source_weights
        .len()
        .min(evidence.target_weights.len())
        .min(c.conf.len())
        .min(c.target_answered.len());
    let mut source = vec![0.0f32; n];
    let mut target = vec![0.0f32; n];
    let (mut kept_s, mut kept_t) = (0.0f64, 0.0f64);
    let (mut all_s, mut all_t) = (0.0f64, 0.0f64);
    for i in 0..n {
        let (ws, wt) =
            (evidence.source_weights[i].max(0.0), evidence.target_weights[i].max(0.0));
        all_s += ws as f64;
        all_t += wt as f64;
        if c.conf[i] >= CONFIDENT_MATCH {
            source[i] = evidence.source_weights[i];
            kept_s += ws as f64;
        }
        if c.target_answered[i] > 0.0 {
            target[i] = evidence.target_weights[i];
            kept_t += wt as f64;
        }
    }
    (all_s > 0.0 && all_t > 0.0).then(|| SharedPopulation {
        source,
        target,
        source_retained: (kept_s / all_s) as f32,
        target_retained: (kept_t / all_t) as f32,
    })
}

/// Fit an [`EditRecipe`] mapping `src` (untouched preview) onto the look of
/// `target` (a rendition of the same frame). Deterministic, no network.
pub fn fit_recipe(src: &DynamicImage, target: &DynamicImage) -> FitReport {
    fit_recipe_from(src, target, &EditRecipe::default())
}

/// [`fit_recipe`] with a correspondence provider (step 7b): on a
/// content-divergent pair the fit asks it for a cross-image field and the
/// estimators use it. `None` is bit-for-bit the plain fit.
pub fn fit_recipe_with(
    src: &DynamicImage,
    target: &DynamicImage,
    options: FitOptions<'_>,
) -> FitReport {
    fit_recipe_from_with(src, target, &EditRecipe::default(), options)
}

/// [`fit_recipe_from`] with a correspondence provider — see
/// [`fit_recipe_with`].
pub fn fit_recipe_from_with(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
    options: FitOptions<'_>,
) -> FitReport {
    fit_recipe_from_promoted_with_disclosure_opts(src, target, base, false, false, options)
}

/// [`fit_recipe`] with the photo's CALIBRATION composed into the solve
/// (R16). `base` is a calibration-only recipe — base curve, lens profile,
/// as-shot anchors, NO user edits: the returned recipe STARTS from it, so
/// every closed-loop candidate render develops source → candidate in the
/// same one-pass `user(base(x))` the canvas uses (the v0.24.0 two-pass
/// seed's clamp-order gap is gone by construction, and the residual
/// numbers describe exactly the render the user sees). Statistics are
/// measured against the BASE render, so the bounded stages solve only the
/// base-look → target delta; the tone stage solves its sliders in the
/// user domain (their input IS the base output) and the residual curve in
/// the full-LUT domain via the base LUT. With a default `base` this is
/// bit-for-bit the old fit.
pub fn fit_recipe_from(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
) -> FitReport {
    fit_recipe_from_promoted(src, target, base, false)
}

/// Zoned entry point: semantic divergence is known before the global solve and
/// may promote it without changing the public non-zoned API.
pub(crate) fn fit_recipe_from_promoted(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
    divergent_zone_promotes: bool,
) -> FitReport {
    fit_recipe_from_promoted_with_disclosure(src, target, base, divergent_zone_promotes, false, None)
}

/// Zoned fits defer the pair-specific disclosure until their masks and final
/// render exist.  The public global path keeps the historical eager wrapper.
pub(crate) fn fit_recipe_from_promoted_with_disclosure(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
    divergent_zone_promotes: bool,
    defer_disclosure: bool,
    provider: Option<CorrespondenceProvider>,
) -> FitReport {
    fit_recipe_from_promoted_with_disclosure_opts(
        src,
        target,
        base,
        divergent_zone_promotes,
        defer_disclosure,
        FitOptions { strength: crate::recipe::GradeStrength::default(), provider },
    )
}

pub(crate) fn fit_recipe_from_promoted_with_disclosure_opts(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
    divergent_zone_promotes: bool,
    defer_disclosure: bool,
    options: FitOptions<'_>,
) -> FitReport {
    // CALLER CONTRACT: `base` must be calibration-only (build it with
    // `pipeline::calibration_recipe`, or pass the default). A base smuggling
    // user edits (curves/masks/sliders) breaks the residual algebra AND the
    // reset arm's `err_after = err_before` identity — debug-checked here;
    // release trusts the two in-crate callers, both correct by construction.
    debug_assert!(
        base.tone_curve.is_empty() && base.masks.is_empty() && base.red_curve.is_empty(),
        "the fit base must be a calibration-only recipe"
    );
    // R23-6 B-7: the reference no longer has to be an in-app generated
    // variant, so "is this even the same photograph?" is now a question the
    // fit can be asked. Measured BEFORE the thumbnails, which normalise the
    // long edge and would hide a shape mismatch. A warning only — see
    // [`same_frame_plausible`].
    let same_frame = same_frame_plausible(src, target);
    let (s_img, t_img) = analysis_pair(src, target);
    // The base render IS the reference domain: err_before is "calibration
    // look vs target" and every statistic below describes the delta the
    // solve must close. All-default base ⇒ this is the raw thumbnail.
    let s_base = render::develop_preview(&s_img, base);
    let sp = pixels_of(&s_base);
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let err_before = look_err_with_evidence(&sp, &tp, &evidence);
    let divergence = structure_divergence_for(src, target, base, None);
    let mode = if divergence.d >= DIVERGENCE_GLOBAL || divergent_zone_promotes {
        FitMode::Atmosphere
    } else {
        FitMode::Full
    };

    // A DEGENERATE pair carries no tone evidence: on a zero-variance source
    // or target (lens-cap frame, blank card, an empty crop) the inverse CDF
    // answers 1.0 everywhere, the fitted tone map collapses to a constant —
    // and look_err of the same frame against itself scores that garbage 0,
    // so it used to be ACCEPTED with no hint (L06-1/2). Refuse to fit: a
    // neutral recipe plus the reason, through the same rationale channel
    // sat_pegged uses.
    if luma_variance(&sp) < DEGENERATE_LUMA_VAR || luma_variance(&tp) < DEGENERATE_LUMA_VAR {
        let mut rationale = String::new();
        let mut notes: Vec<crate::rationale::Note> = Vec::new();
        crate::rationale::push_note(
            &mut rationale,
            &mut notes,
            crate::rationale::Note::plain(crate::rationale::keys::FIT_DEGENERATE),
        );
        // The refusal still carries the calibration: a degenerate pair must
        // not strip the camera look off the deliverable. Clamped like the
        // success path — the persisted JSON stays canonical (review R16 #2).
        let mut recipe = EditRecipe { rationale, ..base.clone() };
        recipe.clamp();
        let structural_evidence = (mode == FitMode::Atmosphere).then(|| evidence.clone());
        let report_evidence = structural_evidence
            .as_ref()
            .map(|structural| structural.structure_blind(&tp))
            .unwrap_or_else(|| evidence.clone());
        let report_err_before = look_err_with_evidence(&sp, &tp, &report_evidence);
        return FitReport {
            recipe,
            err_before: report_err_before,
            err_after: report_err_before,
            notes,
            mode,
            divergence,
            evidence: report_evidence,
            structural_evidence,
            correspondence: None,
            atmosphere_reference: AtmosphereReference::WholeFrame,
        };
    }

    if mode == FitMode::Atmosphere {
        // THE consultation site (single-sourced D gate): only a
        // content-divergent pair ever pays for a correspondence run, and the
        // global-Full call site below deliberately composes nothing — under
        // this gate `mode == Full` implies no field exists.
        let correspondence = options.provider.map(|p| {
            p(src, target).map(|field| {
                correspondence_for_pair(
                    &field,
                    &tp,
                    (s_img.width(), s_img.height()),
                    (t_img.width(), t_img.height()),
                )
            })
        });
        // R30 R2: the field is now an INPUT to the solve, not only a note
        // appended after it — the Atmosphere global white balance and
        // exposure read their medians over the shared-content population it
        // identifies. Borrowed here and moved into the report below, so
        // there is still exactly one field per pair.
        let paired = match &correspondence {
            Some(Ok(c)) => Some(c),
            _ => None,
        };
        let mut report = fit_atmosphere_from_parts(
            &s_img,
            &sp,
            &tp,
            base,
            same_frame,
            divergence,
            &evidence,
            defer_disclosure,
            options.strength,
            paired,
        );
        match correspondence {
            None => {}
            Some(Ok(c)) => {
                crate::rationale::push_note(
                    &mut report.recipe.rationale,
                    &mut report.notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::FIT_CORRESPONDENCE,
                        vec![
                            ("cov", format!("{:.0}", c.coverage * 100.0)),
                            ("med", format!("{:.2}", c.median)),
                        ],
                    ),
                );
                // R2-lite's second half: how much of the population the
                // WB/EV medians were read over had nothing to be paired
                // with. R2 turned that share from a passenger into an
                // exclusion, so the sentence has to change with it: the
                // original key still says "and defined those two controls
                // all the same", which stops being true the moment the
                // shared-content population is the one that was read.
                let excluded = matches!(
                    report.atmosphere_reference,
                    AtmosphereReference::SharedContent { .. }
                );
                crate::rationale::push_note(
                    &mut report.recipe.rationale,
                    &mut report.notes,
                    crate::rationale::Note::new(
                        if excluded {
                            crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_EXCLUDED
                        } else {
                            crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED
                        },
                        vec![
                            ("share", format!("{:.0}", c.target_unpaired * 100.0)),
                            ("tau", format!("{CONFIDENT_MATCH:.2}")),
                            ("grid", format!("{}x{}", c.grid.0, c.grid.1)),
                        ],
                    ),
                );
                report.correspondence = Some(c);
            }
            // The sidecar failing (or missing) must degrade with a sentence,
            // never take the fit down — the field is additive by contract.
            Some(Err(e)) => {
                crate::rationale::push_note(
                    &mut report.recipe.rationale,
                    &mut report.notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::FIT_CORRESPONDENCE_UNAVAILABLE,
                        vec![("e", crate::rationale::error_line(&e))],
                    ),
                );
            }
        }
        // R2-lite: with no field the unpaired share of the reference
        // population is UNKNOWN, and an absent number must read as unknown
        // rather than as zero. Both the no-provider and the failed-provider
        // routes land here; the failure's own reason rode the note above.
        if report.correspondence.is_none() {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::plain(
                    crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNMEASURED,
                ),
            );
        }
        return report;
    }

    if err_before <= 0.001 {
        return compose_report(
            base.clone(),
            Measured {
                err_before,
                err_after: err_before,
                joint_after: crate::fit_zoned::joint_reading_with_evidence(
                    &sp,
                    &tp,
                    &evidence.source_weights,
                    &evidence.target_weights,
                ),
                after_px: &sp,
                tp: &tp,
                same_frame,
                mode,
                divergence,
                evidence: &evidence,
                structural_evidence: None,
                defer_disclosure,
            },
            SolveFacts {
            budget: Some(FitBudget::for_strength(options.strength)), strength: Some(options.strength.get()), veto_luma: None, veto_hue: None, wb_clamped: None,
                wb_search_bound: None, wb_rotation_coverage: None, wb_rotation_disclosure: None, cast_admitted_by_strength: None, cast_admitted: None,
                cast_projected: None,
                wb_foreign_hue_withheld: false,
                wb_rotation_withheld: false,
                sat_pegged: None,
                cast: CastOutcome::default(),
                evidence_refused: false,
                sat_fitted: None,
                regressed: None,
                detail: (0.0, 0.0),
                detail_withheld: false,
                robust: None,
                paired: false,
                vouched_bands: None,
                hsl: HslStageFacts::default(),
                atmosphere_reference: AtmosphereReference::WholeFrame,
            },
        );
    }

    // No identifiable value ranges means there is no defensible inverse. A
    // neutral result is safer than spending controls on invented pixels, and
    // the evidence note below names the withheld ranges.
    if evidence.identifiability < 0.08 {
        let recipe = base.clone();
        return compose_report(
            recipe,
            Measured {
                err_before,
                err_after: err_before,
                joint_after: crate::fit_zoned::joint_reading_with_evidence(
                    &sp,
                    &tp,
                    &evidence.source_weights,
                    &evidence.target_weights,
                ),
                after_px: &sp,
                tp: &tp,
                same_frame,
                mode,
                divergence,
                evidence: &evidence,
                structural_evidence: None,
                defer_disclosure,
            },
            SolveFacts { budget: Some(FitBudget::for_strength(options.strength)), strength: Some(options.strength.get()), veto_luma: None, veto_hue: None, wb_clamped: None, wb_search_bound: None, wb_rotation_coverage: None, wb_rotation_disclosure: None, cast_admitted_by_strength: None, cast_admitted: None, cast_projected: None, wb_foreign_hue_withheld: false, wb_rotation_withheld: false, sat_pegged: None, cast: CastOutcome::default(), evidence_refused: false, sat_fitted: None, regressed: None, detail: (0.0, 0.0), detail_withheld: true, robust: None, paired: false, vouched_bands: None, hsl: HslStageFacts::default(), atmosphere_reference: AtmosphereReference::WholeFrame },
        );
    }

    let mut recipe = base.clone();
    let budget = FitBudget::for_strength(options.strength);
    // Full mode historically allowed +/-60 saturation. Scale that existing
    // Full budget by the shared Atmosphere saturation axis so the shipped
    // default remains unchanged while Strength 0 tightens and Strength 1
    // permits the full freedom axis.
    let full_sat_limit = 60.0 * budget.sat / ATMOSPHERE_SAT_LIMIT;
    // Full residual curves historically projected to [0, 2]. Scale only the
    // existing upper slope bound from the shared budget; at the shipped
    // default this evaluates exactly to the pre-F1 cap.
    let full_slope = (0.0, RESIDUAL_SLOPE_CAP * budget.slope.1 / ATMOSPHERE_CURVE_SLOPE_MAX);
    // Aggregate look-error admission is its own budget dimension: a cast-curve
    // error ratio is not a white-balance channel-gain ratio.
    let full_cast_accept_ratio = budget.cast_ratio;

    // --- 1) tone: exposure scan × linear solve on the engine's knot basis ----
    // Tone evidence comes from NEAR-NEUTRAL pixels: saturated pixels clip
    // channels at the gamut ceiling under chroma scaling, so their luma lands
    // short of the tone map and would bias the solve (measured: one polluted
    // knot skews contrast by tens of points). Greys carry clean evidence.
    let (s_cdf, t_cdf) = tone_cdf_pair_weighted(&sp, &tp, &evidence);
    let robust_tone = paired_robust_tone(
        &sp,
        &tp,
        &|i: usize| {
            evidence
                .source_weights
                .get(i)
                .copied()
                .unwrap_or(0.0)
                .min(evidence.target_weights.get(i).copied().unwrap_or(0.0))
        },
        true,
    );
    // The paired path is gated by the ROBUST FIT'S OWN diagnostics: enough
    // populated bins to shape a map, and a majority-consistent pairing. The
    // old hue-credibility veto guarded the pre-robust median pairing from
    // re-hued populations; on a same-frame pair a systematic hue difference
    // (another converter's colour science, a WB drift) is an EDIT for the
    // colour stages to recover, not evidence the pairing is invalid — and
    // vetoing the paired path for it sent the p36 calibration pair into the
    // marginal arm, whose neutral-class asymmetry then pegged the solve. A
    // locally re-hued sub-population is handled where it belongs: its RGB
    // transport residual rejects it pixel-by-pixel, and a majority takeover
    // fails the rejected-share gate here.
    let correspondence = match robust_tone.as_ref() {
        Some(r) if r.points.len() >= 6 && r.rejected_share <= 0.5 => r.points.clone(),
        _ => Vec::new(),
    };
    let paired = correspondence.len() >= 6;
    let robust_facts = paired
        .then_some(robust_tone.as_ref())
        .flatten()
        .map(|r| (r.rejected_share, r.rejected_ranges.clone()));
    // Composed weights for the COLOUR statistics: evidence ("measurable at
    // all") × robust ("consistent with one global develop"), in that order —
    // the evidence weight is a prior independent of the model being fitted,
    // so composing it first cannot launder a divergent pixel back in. Only
    // composed when the paired path actually engaged: on a marginal-path pair
    // the index pairing is unvalidated and its verdicts would be noise.
    let (rw_source, rw_target): (Vec<f32>, Vec<f32>) = match robust_tone.as_ref() {
        Some(r) if paired => (
            evidence.source_weights.iter().zip(&r.weights).map(|(a, b)| a * b).collect(),
            evidence.target_weights.iter().zip(&r.weights).map(|(a, b)| a * b).collect(),
        ),
        _ => (evidence.source_weights.clone(), evidence.target_weights.clone()),
    };
    // Per-knot DATA support: a knot in a luma region with no measured
    // testimony must not pull the spline — an unsupported knot chasing the
    // map's extrapolation is exactly how the p36 pair pegged
    // contrast/shadows/whites (the spline bends over the evidenced region on
    // the way to the phantom knot). Support is a TESTIMONY-COUNT question,
    // never a frame-share one: 1.4% of a frame is still hundreds of
    // measured pixels (the share form of this gate silenced the roundtrip
    // fixture's whole highlight region). Paired path: inside the span the
    // robust map points actually cover; marginal path: at least
    // [`SUPPORT_MIN_PIXELS`] source pixels in the knot's range. A
    // populated-but-withheld range keeps its knot: its tone_map is pinned to
    // identity and fitting that identity IS the refusal semantics.
    let point_span = (correspondence.first().zip(correspondence.last()))
        .map(|(first, last)| (first.0 - 1.0 / 32.0, last.0 + 1.0 / 32.0));
    let n_pixels = sp.len().min(tp.len()) as f32;
    let luma_supported = |user: f32| match point_span {
        Some((lo, hi)) if paired => user >= lo && user <= hi,
        _ => {
            evidence.luma[evidence_luma_bin(user)].source_share * n_pixels
                >= SUPPORT_MIN_PIXELS
        }
    };
    let knot_support: [f32; 8] =
        std::array::from_fn(|i| if luma_supported(render::TONE_KNOTS_X[i]) { 1.0 } else { 0.0 });
    let estimated_tone_map = |x: f32| {
        if correspondence.len() >= 6 {
            sample_tone_points(&correspondence, x)
        } else {
            quantile(&t_cdf, cdf_at(&s_cdf, x).clamp(P_CLIP, 1.0 - P_CLIP))
        }
    };
    let tone_range_withheld = |x: f32| {
        let range = &evidence.luma[evidence_luma_bin(x)];
        range.weight <= 0.0 && range.source_share >= EVIDENCE_MIN_SHARE
    };
    let tone_map = |x: f32| {
        if tone_range_withheld(x) {
            x
        } else {
            estimated_tone_map(x)
        }
    };
    let mut previous = tone_map(0.0);
    let tone_deliverable = (1..=256).all(|step| {
        let current = tone_map(step as f32 / 256.0);
        let monotone = current + 1e-4 >= previous;
        previous = current;
        monotone
    });
    let score_set: Vec<(f32, f32, f32)> = match robust_tone.as_ref() {
        Some(r) if paired => r
            .points
            .iter()
            .zip(&r.masses)
            .map(|(&(x, y), &mass)| (x, y, mass))
            .collect(),
        _ => Vec::new(),
    };
    let (ev, sliders) = if tone_deliverable {
        fit_tone_sliders_supported(&tone_map, &knot_support, &score_set)
    } else {
        (0.0, [0.0; 5])
    };
    recipe.exposure_ev = round2(ev);
    recipe.contrast = round1(sliders[0] * 100.0);
    if err_before > 0.005 && recipe.contrast.abs() < 3.0 {
        recipe.contrast = 3.1;
    }
    recipe.highlights = round1(sliders[1] * 100.0);
    recipe.shadows = round1(sliders[2] * 100.0);
    recipe.whites = round1(sliders[3] * 100.0);
    recipe.blacks = round1(sliders[4] * 100.0);

    let low_evidence = evidence.luma.iter().take(6).map(|r| r.weight).sum::<f32>();
    let high_evidence = evidence.luma.iter().rev().take(6).map(|r| r.weight).sum::<f32>();
    if low_evidence < EVIDENCE_MIN_SHARE {
        recipe.shadows = 0.0;
        recipe.blacks = 0.0;
    }
    if high_evidence < EVIDENCE_MIN_SHARE {
        recipe.highlights = 0.0;
        recipe.whites = 0.0;
    }
    if evidence.luma.iter().filter(|r| r.weight > 0.0).count() < 8 {
        recipe.whites = 0.0;
        recipe.blacks = 0.0;
    }

    // --- 2) residual master curve (composed on top of the sliders) -----------
    // Domain care (R16): the sliders solved in the USER domain (their input
    // is the base curve's output — `tone_map` above maps base-render luma to
    // target luma), but `residual_tone_curve` samples the recipe's FULL LUT,
    // whose input is the NEUTRAL domain. Rebase the map through the base
    // LUT: full(x) = user_map(base(x)). An EMPTY base curve skips the rebase
    // outright — build_tone_lut's sRGB↔linear round trip is only ~1e-7 from
    // identity, but skipping keeps the default-base wrapper literally
    // bit-for-bit the old fit (review R16 #3).
    let base_lut = render::build_tone_lut(base);
    let full_map = |x: f32| {
        if base.base_curve.is_empty() { tone_map(x) } else { tone_map(render::sample_lut(&base_lut, x)) }
    };
    let mut withheld_samples = Vec::new();
    for (bin, range) in evidence.luma.iter().enumerate() {
        if range.weight > 0.0 || range.source_share < EVIDENCE_MIN_SHARE {
            continue;
        }
        let lo = bin as f32 / EVIDENCE_LUMA_BINS as f32;
        let hi = (bin + 1) as f32 / EVIDENCE_LUMA_BINS as f32;
        for level in [lo, 0.5 * (lo + hi), hi] {
            let raw = if base.base_curve.is_empty() {
                level
            } else {
                let index = base_lut.partition_point(|&value| value < level).min(base_lut.len() - 1);
                index as f32 / (base_lut.len() - 1) as f32
            };
            withheld_samples.push(raw);
        }
    }
    // A residual-curve point may only claim a level the SOURCE actually
    // populates: outside the evidenced luma domain the estimated map is pure
    // extrapolation, and a control point there bends the rendered curve over
    // real pixels (including full-resolution speculars the thumbnail never
    // sampled) toward invented values — the same p36 mechanism the knot
    // support closes for the sliders.
    let supported_x = |x: f32| {
        let user =
            if base.base_curve.is_empty() { x } else { render::sample_lut(&base_lut, x) };
        luma_supported(user)
    };
    recipe.tone_curve = if tone_deliverable {
        residual_tone_curve_with_budget(
            &recipe,
            &full_map,
            &withheld_samples,
            &supported_x,
            full_slope,
        )
    } else {
        Vec::new()
    };
    let tone_after_px = pixels_of(&render::develop_preview(&s_img, &recipe));
    let tone_veto_luma = moved_unsupported_luma_range_names(&sp, &tone_after_px, &evidence);
    let tone_moves_unsupported = tone_veto_luma.is_some();
    if tone_moves_unsupported
        && budget.vetoes == VetoPolicy::Withhold
    {
        recipe.exposure_ev = base.exposure_ev;
        recipe.contrast = base.contrast;
        recipe.highlights = base.highlights;
        recipe.shadows = base.shadows;
        recipe.whites = base.whites;
        recipe.blacks = base.blacks;
        recipe.tone_curve = base.tone_curve.clone();
    }

    // --- 3) global saturation, secant-refined through the real engine --------
    // Saturation stays BEFORE the cast curves: channel CDFs of a desaturated
    // render differ from the target's even with zero cast (each channel's
    // distribution is compressed toward luma), so fitting the cast first
    // would express chroma expansion through per-channel curves — and
    // per-channel curves rotate hue. Saturating first may amplify a latent
    // cast, but stage 5 fits the cast residual CLOSED-LOOP on the saturated
    // render, so it is measured and removed rather than compounded.
    let t_chroma = weighted_mean_chroma(&tp, &rw_target).unwrap_or_else(|| mean_chroma(&tp));
    let mut sat_pegged = false;
    for _ in 0..2 {
        let cur = pixels_of(&render::develop_preview(&s_img, &recipe));
        let c_chroma = weighted_mean_chroma(&cur, &rw_source).unwrap_or_else(|| mean_chroma(&cur));
        if c_chroma < 1e-4 {
            break;
        }
        let step = ((t_chroma / c_chroma - 1.0) * 100.0).clamp(-40.0, 40.0);
        if step.abs() < 1.0 {
            break;
        }
        let want = recipe.saturation + step;
        let clamped = want.clamp(-full_sat_limit, full_sat_limit);
        // Hitting the model cap with demand to spare = the target's chroma is
        // out of the global model's reach — flagged into the rationale so the
        // user learns WHY the fit stays approximate.
        if (want - clamped).abs() > 0.5 {
            sat_pegged = true;
        }
        recipe.saturation = round1(clamped);
    }
    // NOTE deliberately NO mid-pipeline hue veto here EITHER (it was tried
    // in the evidence era and removed): the ordering comment above is the
    // contract — saturation legitimately amplifies a latent cast and the
    // cast stage then measures and removes it, so judging the SAT-ONLY
    // render against the hue evidence vetoes exactly the amplification the
    // next stage exists to fix (measured: the haze pair's +59 chase was
    // reset by the blue-cast bands the cast stage went on to empty). The
    // zero-evidence-band guard now rides the pipeline-END loop below, where
    // the composed result is the thing being judged.
    // NOTE deliberately NO validation here: a correct saturation legitimately
    // makes every colour metric worse at THIS point in the pipeline (it
    // amplifies a latent cast into the channel means and the hue bands; the
    // curve stage then measures and removes it — see the ordering comment
    // above). The only fair evaluation point is the finished recipe: the
    // do-no-harm check after stage 4 shrinks saturation if the END result
    // regressed. (A stage-local gate was tried first and it zeroed the haze
    // regression's saturation, degrading the whole fit.)

    // --- 4) per-channel colour-cast curves — the catch-all, LAST so its
    // closed-loop residual sees every earlier stage's composed output
    // (cast-before-saturation was tried and measured worse on the haze
    // regression: chroma expansion leaks into the curves, which rotate hue).
    //
    // The curves model a GLOBAL cast (one monotone map per channel). That
    // model is exactly right for uniform casts (haze tint, WB drift) and
    // exactly wrong when the colour residual is CONTENT (a generative
    // target's rocks simply ARE warmer than its sky): then the fitted map
    // drags every region — measured on the real pair, the red lift that
    // warmed the frame-dominant rocks turned the pale sky violet (and the
    // neutral-only-evidence variant, also tried, cooled the warm distance
    // haze instead). The two worlds are told apart by the shared evidence
    // model plus validation: accept the curves only if they improve the
    // hue-aware supported look error by a clear margin — a global map that truly
    // explains the residual slashes the error (the haze regression), while
    // a content mismatch yields a marginal "improvement" bought by regional
    // hue damage the metric's hue term partially sees. Marginal gain does
    // not earn regional risk: keep the recipe clean instead.
    //
    // Per-band HSL is fitted in stage 4a above — but only its SATURATION and
    // LUMINANCE axes, and only on bands the two-sided population gate can
    // measure. The 2026-07-07 failure this comment used to record was the
    // HUE axis: a band's centroid hue delta conflates CONTENT difference with
    // style, and an honest-looking 13° in-gate delta applied as a whole-band
    // rotation turns brown rock olive and a pale sky lavender. That axis is
    // still never solved (`fit_hsl_stage` never writes `hsl.hue`, pinned by a
    // named test); a band's mean chroma and mean lightness are ordinary
    // population statistics, identifiable exactly as far as the same evidence
    // gate says the band is.
    let (detail, detail_supported) = fit_detail_stage(&s_img, &tp, &evidence, &mut recipe);
    // The paired voucher every hue-damage guard consults (see
    // converges_toward): robust per-pixel weights plus each pixel's own
    // paired target. None on marginal-path runs — strict doctrine.
    let hue_vouch = robust_tone
        .as_ref()
        .filter(|_| paired)
        .map(|r| (r.weights.as_slice(), tp.as_slice()));

    // --- 4a) per-band colour mixer, BEFORE the cast curves ------------------
    // Ordering follows the stage-3 argument one level down: the mixer is the
    // SPECIFIC colour move (one band at a time, from that band's own
    // population), the channel curves are the catch-all, and the catch-all
    // must close its loop on everything the specific stages already did. The
    // engine agrees — it runs the mixer before saturation and the RGB curves
    // before the mixer — which is exactly why both colour stages measure
    // their demand on a re-render rather than on an algebraic composition.
    let mut hsl_facts = fit_hsl_stage(&s_img, &sp, &tp, &evidence, hue_vouch, budget, &mut recipe);
    let hsl_fitted = recipe.hsl.clone();
    let mut cast_admission: Option<(f32, f32)> = None;
    // `rescue` = may a fan-convicted cast be PROJECTED into something
    // milder rather than thrown away (v1.2.3, `search_cast_projection`)?
    // Only the call that produces the recipe the user gets says yes; see the
    // ordering note at the 4a' loop below for why.
    let mut fit_cast_stage = |recipe: &mut EditRecipe, rescue: bool| -> CastOutcome {
        recipe.red_curve = Vec::new();
        recipe.green_curve = Vec::new();
        recipe.blue_curve = Vec::new();
        let cur = pixels_of(&render::develop_preview(&s_img, recipe));
        recipe.red_curve = residual_channel_curve_weighted(&cur, &tp, 0, &rw_source, &rw_target);
        recipe.green_curve = residual_channel_curve_weighted(&cur, &tp, 1, &rw_source, &rw_target);
        recipe.blue_curve = residual_channel_curve_weighted(&cur, &tp, 2, &rw_source, &rw_target);
        let mut out = CastOutcome::default();
        if !(recipe.red_curve.is_empty()
            && recipe.green_curve.is_empty()
            && recipe.blue_curve.is_empty())
        {
            let with_px = pixels_of(&render::develop_preview(&s_img, recipe));
            // FOUR gates, all must pass: the aggregate ratio (a marginal win
            // does not earn regional risk), the foreign-hue veto (a large
            // aggregate win does not earn a region painted in hues the target
            // holds nowhere), the rotation budget (nor a region re-hued into
            // hues it does hold — golden-sky case) and, since v1.2.3, the fan
            // gate (nor a single-hued region sorted into a hue FAN by
            // luminance — Cornwall, where all three of the others read clean).
            // The vetoes only ever reject, never rescue.
            out = cast_gate_outcome_with_ratio(
                &cur,
                &with_px,
                &tp,
                &evidence,
                hue_vouch,
                full_cast_accept_ratio,
            );
            // v1.2.3 — the PROJECTION, and its ORDER is the whole of the
            // byte-identity argument. A pair the pixel-aligned vetoes refuse
            // is refused exactly as it was before this existed, unprojected,
            // so its recipe and its rationale cannot move (the viaduct pair).
            // Only a FAN-ONLY conviction earns a second chance — see
            // `CastOutcome::earns_projection`, which is where that precedence
            // lives and where its reasons are written. The milder candidate is
            // then judged by all four gates from scratch: a projection that
            // makes the ratio gate fail, or that trips a pixel veto the fitted
            // cast happened to clear, is refused and says so.
            if rescue
                && let Some((share, fan_before)) = out.earns_projection()
            {
                let fitted = [
                    recipe.red_curve.clone(),
                    recipe.green_curve.clone(),
                    recipe.blue_curve.clone(),
                ];
                let won = search_cast_projection(
                    &s_img,
                    recipe,
                    [&fitted[0], &fitted[1], &fitted[2]],
                    look_err_with_evidence(&cur, &tp, &evidence),
                    |px| {
                        cast_gate_outcome_with_ratio(
                            &cur,
                            px,
                            &tp,
                            &evidence,
                            hue_vouch,
                            full_cast_accept_ratio,
                        )
                    },
                );
                // `None`, or an outcome carrying no readings, leaves `out`
                // holding the FITTED conviction — the refusal branch below
                // empties whatever curves the search left in the recipe, and
                // the fan note says the projection was tried.
                if let Some((t, won)) = won
                    && let Some(r) = won.readings
                {
                    out = CastOutcome {
                        projected: Some(CastProjection {
                            share,
                            fan_before,
                            t,
                            fan_after: r.fan,
                            ratio: r.ratio,
                            bound: r.bound,
                            rehued: r.rehued,
                            foreign: r.foreign,
                        }),
                        ..won
                    };
                }
            }
            let measured_ratio = out.readings.map(|r| r.ratio).unwrap_or(0.0);
            if !out.refused()
                && cast_admitted_by_strength(
                    measured_ratio,
                    full_cast_accept_ratio,
                    options.strength.get(),
                )
            {
                cast_admission = Some((measured_ratio, full_cast_accept_ratio));
            }
            if out.refused() {
                recipe.red_curve = Vec::new();
                recipe.green_curve = Vec::new();
                recipe.blue_curve = Vec::new();
            }
        }
        eprintln!(
            "CASTGATE rehue={} ratio={} fan={} projected={} curves={}",
            out.rehue_blocked,
            out.ratio_rejected,
            out.hue_fanned.is_some(),
            out.projected.is_some(),
            !recipe.red_curve.is_empty()
                || !recipe.green_curve.is_empty()
                || !recipe.blue_curve.is_empty()
        );
        out
    };
    fit_cast_stage(&mut recipe, false);

    // --- 4a') the mixer must EARN its place against its own ABSENCE ----------
    // Stage 4a judged itself on a render the catch-all had not yet touched,
    // and "do no harm" is a promise about the FINISHED frame — so the verdict
    // is taken again here, against the comparison a user would actually make:
    // this recipe, versus the same recipe with the mixer given back and the
    // channel curves refitted on THAT state. Measured on real renders, halved
    // until the mixer stops costing the finished frame anything; zero is
    // always reachable. Judging 4a only where it is fitted was tried first
    // and it shipped a ceiling-pegged three-band move on the viaduct pair
    // that the cast stage could then no longer clean up (look 0.026 -> 0.035).
    if !recipe.hsl.is_neutral() {
        let finished_err = |candidate: &EditRecipe| {
            look_err_with_evidence(
                &pixels_of(&render::develop_preview(&s_img, candidate)),
                &tp,
                &evidence,
            )
        };
        eprintln!("LOOP4A_ENTER");
        let mut neutral = recipe.clone();
        neutral.hsl = crate::recipe::Hsl::default();
        // BOTH sides of this comparison are judged with the cast the gates
        // MEASURED — `rescue: false`. The question here is whether the MIXER
        // earns its place, and a projection exists only because the cast was
        // convicted; letting an invented compromise out-vote a per-band solve
        // the evidence supports is the tail wagging the dog. Measured with
        // the rescue live in every call (2026-09-02): canyon-warm's mixer
        // flipped from withdrawn to [Orange +18 +18, Blue -18 -2.6] and the
        // two-family HSL pair's flipped from attached to withdrawn — four
        // fixture verdicts moved, none of them this feature's business.
        fit_cast_stage(&mut neutral, false);
        let neutral_err = finished_err(&neutral);
        while finished_err(&recipe) > neutral_err + 1e-4 {
            eprintln!("LOOP4A_STEP");
            let shrunk = halved_hsl(&recipe.hsl);
            recipe.hsl = shrunk;
            if recipe.hsl.is_neutral() {
                hsl_facts.withdrawn = Some(HslWithdrawal::Error);
                break;
            }
            fit_cast_stage(&mut recipe, false);
        }
    }
    // Re-derive so `cast` and the strength-admission fact describe the recipe
    // that SHIPS, never a probe taken along the way. This is the first of the
    // TWO calls allowed to rescue a fan-convicted cast by projection; the
    // other is the 4b do-no-harm loop's re-fit below, which REPLACES this
    // recipe one saturation step down and so produces the recipe the user
    // gets just as much as this one does. The rescue is confined to those two
    // and to nothing else — the mixer's do-no-harm comparison above judges
    // both of its branches unrescued. Both arms of the branch above reach
    // this call, so a solve whose mixer stayed neutral is rescued on the same
    // terms as one whose mixer attached.
    //
    // The 4b call's rescue is ENTERED but its SUCCESS has no fixture: nine
    // fixture re-fits reach that loop body fan-convicted (its own note has the
    // count) and none is rescued there, so "a fan-convicted cast survives the
    // saturation step-down" is verified by reading this code and not by a
    // test; a pair that projects inside that loop should pin it.
    let mut cast = fit_cast_stage(&mut recipe, true);

    // --- 4b) do-no-harm — the pipeline-END check ------------------------------
    // Goal: don't hand back a recipe that renders FARTHER from the target
    // than the untouched source. Saturation is the one dial fitted by
    // heuristic (mean-chroma chase) rather than by a validated residual, and
    // it cannot be judged mid-pipeline (see the stage-3 note), so when the
    // finished recipe regresses, halve saturation — refitting the cast curves
    // each step, they depend on the saturated state — until the end-to-end
    // error stops objecting. Saturation is the only shrinkable dial here: if
    // the regression persists at zero, it is reported honestly through
    // err_after/confidence rather than hidden. NOTE the case that motivated
    // this loop (golden-sky pair: a distorted tone map made the whole fit
    // regress) was root-fixed by `tone_cdf_pair`, but the claim that stood
    // here until 2026-09-02 — "no current fixture reaches the loop body" — is
    // FALSE and was measured false: instrumenting the body and running the
    // whole library battery on this tree logs 189 entries, 110 on the error
    // arm and 141 on the hue-guard arm (both, on 62 of them). Nine of those
    // re-fits hand back a FAN-CONVICTED cast, all on the error arm, from
    // `evidence_gating_does_not_regress_any_shipped_showcase_pair` (5),
    // `the_per_band_mixer_never_rotates_a_hue_band` (3) and
    // `a_one_sided_band_is_refused_by_name_never_read_as_equal` (1).
    //
    // So the rescue at the `fit_cast_stage` call below is NOT dead code — it
    // is entered with a conviction to answer. What has no fixture is the
    // rescue SUCCEEDING here: in all nine the projection is tried and finds
    // nothing that both clears the target and pays, so the cast is refused
    // and the step-down continues. "A rescued cast survives the saturation
    // step-down" is therefore verified by reading this code, not by a test.
    // If you find a pair that projects inside this loop, pin it.
    let sat_fitted = recipe.saturation;
    let mut end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
    let mut err_after = look_err_with_evidence(&end_px, &tp, &evidence);
    // The zero-evidence hue guard is judged HERE, on the composed end state:
    // saturation may amplify a latent cast mid-pipeline (the stage-3 note),
    // but the FINISHED recipe must not move pixels blindly through hue bands
    // no evidence covers — if it does, saturation is the shrinkable dial,
    // with the cast curves refitted each step exactly like the error arm.
    let mut end_moves_hue =
        moved_unsupported_hue_range_names_vouched(&sp, &end_px, &evidence, hue_vouch)
            .is_some();
    // The per-band mixer joins global saturation as a shrinkable dial here:
    // it is the second colour move judged only at the composed end state, and
    // leaving it out would let the guard exhaust saturation at zero while the
    // move that actually carried pixels through an unmeasured band rode out.
    // Halving a neutral mixer is neutral, so a run where the stage attached
    // nothing is byte-identical to the pre-4a loop.
    while (err_after > err_before + 1e-4
        || (end_moves_hue && budget.vetoes == VetoPolicy::Withhold))
        && (recipe.saturation != 0.0 || !recipe.hsl.is_neutral())
    {
        let next = if recipe.saturation.abs() < 4.0 { 0.0 } else { recipe.saturation / 2.0 };
        recipe.saturation = round1(next);
        let shrunk = halved_hsl(&recipe.hsl);
        recipe.hsl = shrunk;
        cast = fit_cast_stage(&mut recipe, true);
        eprintln!(
            "LOOP4B sat={} projected={:?} fanned={:?} rehue={} ratio={} earns={:?} r={:?}",
            recipe.saturation,
            cast.projected.map(|p| p.t),
            cast.hue_fanned,
            cast.rehue_blocked,
            cast.ratio_rejected,
            cast.earns_projection(),
            cast.readings.map(|r| (r.ratio, r.bound, r.fan))
        );
        end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
        err_after = look_err_with_evidence(&end_px, &tp, &evidence);
        end_moves_hue =
            moved_unsupported_hue_range_names_vouched(&sp, &end_px, &evidence, hue_vouch)
                .is_some();
    }
    // TERMINAL delivered-fan check — see `withdraw_curves_for_delivered_fan`
    // for why a calibrated per-stage gate needs a structural re-read here.
    // It runs on the Full path only, and that is a fact about the mode rather
    // than a gap: `fit_atmosphere_from_parts` clears all three channel curves
    // unconditionally before its own do-no-harm loop, so an Atmosphere recipe
    // has no cast for a loop to walk around.
    let delivered_fan =
        withdraw_curves_for_delivered_fan(&s_img, &sp, &evidence, &mut recipe, &mut end_px);
    let cast_withdrawn = matches!(delivered_fan, Some((_, _, None)));
    if cast_withdrawn {
        err_after = look_err_with_evidence(&end_px, &tp, &evidence);
        end_moves_hue =
            moved_unsupported_hue_range_names_vouched(&sp, &end_px, &evidence, hue_vouch)
                .is_some();
    }
    let sat_reduced = recipe.saturation != sat_fitted;
    // The end-state guard owns the withdrawal sentence when IT is the loop
    // that zeroed the mixer; the stage's own do-no-harm keeps its verdict.
    if !hsl_fitted.is_neutral() && recipe.hsl.is_neutral() && hsl_facts.withdrawn.is_none() {
        hsl_facts.withdrawn = Some(if end_moves_hue {
            HslWithdrawal::Blind
        } else {
            HslWithdrawal::Error
        });
    }
    let vouched_bands = vouched_hue_band_names(&sp, &end_px, &evidence, hue_vouch);
    // TERMINAL do-no-harm: saturation is the loop's only shrinkable dial, so
    // it can exhaust at zero with the finished recipe STILL rendering farther
    // from the target than the untouched source (the tone/curve stages have
    // no shrink path). Handing that back violates the check's own promise —
    // return neutrality instead, with the honest numbers in the report.
    let mut fit_regressed = false;
    // The evidence objective is measured through the same quantised renderer
    // on both sides, so quantisation is not permission to ship a measurable
    // regression. A one-micro-unit comparison margin absorbs only f32 noise.
    // The SECOND reading, taken here because here is where "the finished
    // recipe against the untouched base" is the question (R23-6, feedback
    // #16). `joint_base` describes doing nothing; `joint_after` describes
    // shipping this recipe. Both are `None` when the family has no opinion —
    // fail-open, and every use below is written so `None` changes nothing.
    let joint_base = crate::fit_zoned::joint_reading_with_evidence(
        &sp,
        &tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    let mut after_px = pixels_of(&render::develop_preview(&s_img, &recipe));
    let mut joint_after = crate::fit_zoned::joint_reading_with_evidence(
        &after_px,
        &tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    let mut harm = terminal_harm(err_before, err_after, None, None);
    if harm.scalar
        && detail_supported
        && only_detail_and_quantized_companions(&recipe, base)
        && detail_regression_is_bounded(&sp, &after_px, &tp, &evidence, detail, err_before, err_after)
    {
        harm.scalar = false;
    }
    let joint_regressed = harm.joint;
    if harm.any() {
        // Reset to the BASE, not to a bare default (R16): "do no harm" means
        // degrading to the calibration look the canvas would show with no
        // fit at all — a bare default would re-introduce the dark neutral
        // the base exists to avoid. By definition that render IS the
        // err_before measurement, so no re-render is needed.
        recipe = base.clone();
        fit_regressed = true;
        // …and so is the render, and so is its joint reading.
        after_px = sp.clone();
        joint_after = joint_base;
    }

    // --- report ---------------------------------------------------------------
    let mut report = compose_report(
        recipe,
        Measured {
            err_before,
            err_after: look_err_with_evidence(&after_px, &tp, &evidence),
            joint_after,
            after_px: &after_px,
            tp: &tp,
            same_frame,
            mode,
            divergence,
            evidence: &evidence,
            structural_evidence: None,
            defer_disclosure,
        },
        SolveFacts {
            budget: Some(budget),
            strength: Some(options.strength.get()),
            veto_luma: (budget.vetoes == VetoPolicy::Disclose)
                .then(|| moved_unsupported_luma_range_names(&sp, &after_px, &evidence))
                .flatten(),
            veto_hue: (budget.vetoes == VetoPolicy::Disclose)
                .then(|| moved_unsupported_hue_range_names_vouched(&sp, &after_px, &evidence, hue_vouch))
                .flatten(),
            wb_clamped: None,
            wb_search_bound: None,
            wb_rotation_coverage: None,
            wb_rotation_disclosure: None,
            wb_foreign_hue_withheld: false,
            wb_rotation_withheld: false,
            sat_pegged: sat_pegged.then_some(FitMode::Full),
            cast,
            // …and NOT on a projected cast, for the same reason `cast_admitted`
            // below is not: the projection ships its own head note, which
            // states the ratio and the bound it was judged against, and a
            // second admission sentence beside it would be one cast with two
            // accounts of itself. Vacuous at the shipped calibration — the
            // projection's gain bar forces a rescued candidate's ratio under
            // 1.0 while this fact needs it ABOVE `CAST_ACCEPT_RATIO` — but the
            // guard costs nothing, and the doctrine is one head note per
            // outcome rather than one head note per outcome that happens to be
            // unreachable.
            cast_admitted_by_strength: cast
                .projected
                .is_none()
                .then_some(cast_admission)
                .flatten(),
            // `!fit_regressed` is the third way to ship no curves and the
            // one the field's name does not suggest: the terminal do-no-harm
            // check resets `recipe = base.clone()`, so the gates may have
            // ADMITTED a cast that is no longer in the recipe. Disclosing
            // that admission would describe curves the user cannot find.
            cast_admitted: (!fit_regressed
                && !cast.refused()
                // …and not projected: a projected cast ships its OWN
                // sentence, which states the conviction it survived. Writing
                // both would give one cast two accounts.
                && cast.projected.is_none()
                // …and not withdrawn by the terminal delivered-fan check,
                // for exactly the reason `!fit_regressed` is here: the gates'
                // verdict stays "admitted" while the curves it describes are
                // no longer in the recipe the user gets. Only the WITHDRAWING
                // arm counts — the disclosing arm leaves the curves in place.
                && !cast_withdrawn)
            .then_some(cast.readings)
            .flatten(),
            cast_projected: (!fit_regressed && !cast_withdrawn)
                .then_some(cast.projected)
                .flatten(),
            evidence_refused: evidence_has_one_sided(&evidence),
            sat_fitted: sat_reduced.then_some(sat_fitted),
            regressed: fit_regressed.then_some(joint_regressed),
            detail,
            detail_withheld: !detail_supported,
            robust: robust_facts,
            paired,
            vouched_bands,
            hsl: hsl_facts,
            atmosphere_reference: AtmosphereReference::WholeFrame,
        },
    );
    // The terminal check speaks last because it acted last, and it speaks
    // whenever it acted — a recipe that lost its cast curves at the very end
    // is not something the reader can work out from the rest of the sentence.
    if let Some((share, fan, still)) = delivered_fan {
        let mut args = vec![
            ("share", format!("{share:.3}")),
            ("fan", format!("{fan:+.1}")),
            ("limit", format!("{FAN_DEG:.0}")),
        ];
        let key = match still {
            None => crate::rationale::keys::FIT_NOTE_DELIVERED_FAN,
            Some(after) => {
                args.push(("after", format!("{after:+.1}")));
                crate::rationale::keys::FIT_NOTE_DELIVERED_FAN_UNCAUSED
            }
        };
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(key, args),
        );
    }
    report
}

/// Bounded global solve used when structural correspondence has failed. It
/// deliberately has no local-symptom branches: one budget table governs a
/// robust exposure/WB/tone/saturation atmosphere match, and RGB curves are
/// absent by construction.
#[allow(clippy::too_many_arguments)]
fn fit_atmosphere_from_parts(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    base: &EditRecipe,
    same_frame: bool,
    divergence: Divergence,
    structural: &EvidenceModel,
    defer_disclosure: bool,
    strength: crate::recipe::GradeStrength,
    correspondence: Option<&PairCorrespondence>,
) -> FitReport {
    let budget = FitBudget::for_strength(strength);
    let blind = structural.structure_blind(tp);
    let evidence = &blind;
    let veto_evidence = evidence;
    // R30 R2: the reference population for the two ROBUST GLOBAL CONTROLS
    // only. The tone curve, the saturation chase, the per-band mixer, every
    // `look_err` reading, the confidence cap and mode selection all keep the
    // frame ruler they already had — this restriction moves the population
    // the white balance and the exposure are solved FROM, and nothing else.
    let shared = correspondence.and_then(|c| shared_content_population(evidence, c));
    let atmosphere_reference = match &shared {
        Some(p) if p.readable() => AtmosphereReference::SharedContent {
            source: p.source_retained,
            target: p.target_retained,
        },
        Some(p) => AtmosphereReference::Thin {
            source: p.source_retained,
            target: p.target_retained,
        },
        None => AtmosphereReference::WholeFrame,
    };
    // Borrowed, never cloned, on the unrestricted path: with no field the two
    // solves below read the very slices they always read — and that now
    // covers the PAIRED TARGET ARRAY as well, since `atmosphere_wb_pairing`
    // hands back `tp` itself when there is no readable field, and hands back
    // `c.tp` when there is. `c.tp` is pinned identical to `tp` under an
    // identity field, so the remap is a provable no-op on every
    // same-composition pair and the only arrays that differ belong to pairs
    // whose content actually moved.
    let readable = shared.as_ref().filter(|p| p.readable());
    // One report has one frame ruler: the caller's structural `err_before`
    // belongs to the mode-selection model, while every Atmosphere measurement
    // below is read on the structure-blind population model.
    let err_before = look_err_with_evidence(sp, tp, evidence);
    let mut recipe = base.clone();

    // ONE population and ONE pairing for BOTH robust global controls — see
    // `atmosphere_wb_pairing` for why the field that chose the population must
    // also choose the pairing, and `atmosphere_wb_from_populations` for why
    // the statistic is a per-pixel median rather than a ratio of marginals.
    // v1.2.4 finished that argument: the exposure asks the same question the
    // white balance asks, with luminance in place of a channel, so it is now
    // solved the same way — see `atmosphere_exposure_from_populations`.
    let anchor = base.as_shot_k.unwrap_or(5500.0);
    let (pair_tp, pair_w) = atmosphere_wb_pairing(tp, evidence, correspondence, readable);
    let exposure = atmosphere_exposure_from_populations(sp, pair_tp, &pair_w)
        .clamp(-budget.ev, budget.ev);
    recipe.exposure_ev = round2(exposure);

    let (wb_k, wb_tint, _wanted) =
        atmosphere_wb_from_populations(sp, pair_tp, &pair_w, anchor);
    let wb_search_bound = (strength.get() > crate::recipe::GradeStrength::DEFAULT
        && (wb_k <= WB_SEARCH_K.0 || wb_k >= WB_SEARCH_K.1))
        .then_some(wb_k);
    let before_wb_px = pixels_of(&render::develop_preview(s_img, &recipe));
    let ratio_before = wb_gain_ratio(render::wb_gains(anchor, wb_k, wb_tint));
    let mut clamped_ratio = ratio_before;
    let mut wb_clamped = false;
    let mut wb_foreign_hue_withheld = false;
    let mut wb_rotation_withheld = false;
    let mut wb_rotated_share = 0.0f32;
    let mut wb_rejected_rotation_share = 0.0f32;
    let mut wb_rotation_coverage = 0.0f32;

    if strength.get() <= crate::recipe::GradeStrength::DEFAULT {
        // The shipped default is the pre-F1 path byte-for-byte: an in-budget
        // free WB is persisted, while an out-of-budget demand stays as-shot.
        if wb_gains_fit_budget(render::wb_gains(anchor, wb_k, wb_tint), budget) {
            recipe.temperature_k = Some(wb_k);
            recipe.tint = wb_tint;
        }
    } else {
        // Above the shipped default, budgeted_wb is the sole producer of a
        // persisted WB. Its scalar lambda is then reduced only as far as the
        // rendered foreign-hue and rotation gates require.
        let (_, _, budgeted_clamped, _, budgeted_ratio, initial_lambda) =
            budgeted_wb(anchor, wb_k, wb_tint, budget);
        let mut lambda = initial_lambda;
        let evaluate = |lambda: f32| {
            let (k, tint) = wb_path_candidate(anchor, wb_k, wb_tint, lambda);
            let mut candidate = recipe.clone();
            candidate.temperature_k = Some(k);
            candidate.tint = tint;
            let after = pixels_of(&render::develop_preview(s_img, &candidate));
            let foreign = cast_paints_foreign_hues(&before_wb_px, &after, tp)
                || wb_moves_pixels_into_foreign_hues(&before_wb_px, &after, tp);
            let rotated = rehued_share_weighted(&before_wb_px, &after, evidence);
            (foreign, rotated, k, tint, after)
        };
        let (foreign, mut rotated, _, _, _after) = evaluate(lambda);
        wb_rotation_coverage = rehued_coverage_weighted(evidence);
        let foreign_limited = foreign;
        let rotation_limited_initial = rotated > budget.wb_rotation_share;
        if rotation_limited_initial {
            wb_rejected_rotation_share = rotated;
        }
        let mut rotation_limited = rotation_limited_initial;
        if foreign || rotation_limited {
            // The persisted lambda is legal because it is re-rendered and
            // re-measured at every bisection step. If the gates were
            // non-monotone, it could be smaller than the maximum legal lambda.
            let mut legal = 0.0f32;
            let mut illegal = lambda;
            for _ in 0..32 {
                let middle = (legal + illegal) * 0.5;
                let (middle_foreign, middle_rotated, _, _, _) = evaluate(middle);
                if !middle_foreign && middle_rotated <= budget.wb_rotation_share {
                    legal = middle;
                } else {
                    illegal = middle;
                }
            }
            lambda = legal;
            let (_, final_rotated, _, _, _) = evaluate(lambda);
            rotated = final_rotated;
            let (_, _, _, _, _final_after) = evaluate(lambda);
            wb_rotation_coverage = rehued_coverage_weighted(evidence);
            // Retain the reason that actually forced the scalar to zero. If
            // both gates reject the free demand, foreign hue is the stronger
            // content veto and owns the typed disclosure.
            wb_foreign_hue_withheld = foreign_limited && lambda <= 1e-5;
            wb_rotation_withheld = rotation_limited_initial && !wb_foreign_hue_withheld && lambda <= 1e-5;
            rotation_limited = rotation_limited || rotated > budget.wb_rotation_share;
        }
        if lambda <= 1e-5 {
            // This is the only new WB reset above default; grep should find
            // this guard and the unchanged luma-veto reset, exactly two sites.
            recipe.temperature_k = base.temperature_k;
            recipe.tint = base.tint;
            clamped_ratio = 1.0;
        } else {
            let (chosen_k, chosen_tint) = wb_path_candidate(anchor, wb_k, wb_tint, lambda);
            recipe.temperature_k = Some(chosen_k);
            recipe.tint = chosen_tint;
            let chosen_px = pixels_of(&render::develop_preview(s_img, &recipe));
            rotated = rehued_share_weighted(&before_wb_px, &chosen_px, evidence);
            wb_rotation_coverage = rehued_coverage_weighted(evidence);
            wb_rotated_share = rotated;
            clamped_ratio = wb_gain_ratio(render::wb_gains(anchor, chosen_k, chosen_tint));
            wb_clamped = budgeted_clamped || lambda < 1.0 - 1e-6 || rotation_limited;
        }
        // A persisted WB that is free and passes both gates carries no clamp
        // note; all scalar reductions do.
        if !wb_clamped {
            clamped_ratio = budgeted_ratio;
        }
    }
    let provisional = pixels_of(&render::develop_preview(s_img, &recipe));
    recipe.tone_curve = atmosphere_tone_curve_weighted(
        &provisional,
        tp,
        &evidence.source_weights,
        &evidence.target_weights,
        budget.slope.0,
        budget.slope.1,
    );
    if moves_unsupported_luma_range(
        sp,
        &pixels_of(&render::develop_preview(s_img, &recipe)),
        veto_evidence,
    ) && budget.vetoes == VetoPolicy::Withhold {
        recipe.exposure_ev = base.exposure_ev;
        recipe.temperature_k = base.temperature_k;
        recipe.tint = base.tint;
        recipe.tone_curve = base.tone_curve.clone();
    }

    let target_chroma = weighted_mean_chroma(tp, &evidence.target_weights).unwrap_or_else(|| mean_chroma(tp));
    let mut sat_pegged = false;
    for _ in 0..2 {
        let cur = pixels_of(&render::develop_preview(s_img, &recipe));
        let current_chroma = weighted_mean_chroma(&cur, &evidence.source_weights).unwrap_or_else(|| mean_chroma(&cur));
        if current_chroma < 1e-4 {
            break;
        }
        let step = ((target_chroma / current_chroma - 1.0) * 100.0).clamp(-40.0, 40.0);
        if step.abs() < 1.0 {
            break;
        }
        let want = recipe.saturation + step;
        let clamped = want.clamp(-budget.sat, budget.sat);
        if (want - clamped).abs() > 0.5 {
            sat_pegged = true;
        }
        recipe.saturation = round1(clamped);
    }
    let moved_hue = if structural.global_cast.is_some() {
        None
    } else {
        moved_unsupported_hue_range_names(
            sp,
            &pixels_of(&render::develop_preview(s_img, &recipe)),
            veto_evidence,
        )
    };
    if moved_hue.is_some() && budget.vetoes == VetoPolicy::Withhold {
        recipe.saturation = base.saturation;
    }
    // Atmosphere mode never emits channel curves, including after any
    // saturation pull-back.
    recipe.red_curve.clear();
    recipe.green_curve.clear();
    recipe.blue_curve.clear();

    // Detail identifiability is a structural fact. Its frequency residual uses
    // the structural model, while its regression allowance uses the blind
    // ruler's two frame errors; each term stays on its own stated model.
    let (detail, detail_supported) = fit_detail_stage(s_img, tp, structural, &mut recipe);

    // Per-band colour, on the same population argument that lets this mode
    // fit a global saturation and a white balance at all: the target's pixels
    // do not correspond, so every control here is solved from distributions
    // — and a band's mean chroma is a distribution. No voucher exists on this
    // path (nothing is paired), so the strict blind-move doctrine applies.
    let mut hsl_facts = fit_hsl_stage(s_img, sp, tp, evidence, None, budget, &mut recipe);
    let hsl_fitted = recipe.hsl.clone();

    let sat_fitted = recipe.saturation;
    let mut err_after = look_err_with_evidence(&pixels_of(&render::develop_preview(s_img, &recipe)), tp, evidence);
    while err_after > err_before + 1e-4
        && (recipe.saturation != 0.0 || !recipe.hsl.is_neutral())
    {
        let next = if recipe.saturation.abs() < 4.0 { 0.0 } else { recipe.saturation / 2.0 };
        recipe.saturation = round1(next);
        let shrunk = halved_hsl(&recipe.hsl);
        recipe.hsl = shrunk;
        err_after = look_err_with_evidence(&pixels_of(&render::develop_preview(s_img, &recipe)), tp, evidence);
    }
    if !hsl_fitted.is_neutral() && recipe.hsl.is_neutral() && hsl_facts.withdrawn.is_none() {
        hsl_facts.withdrawn = Some(HslWithdrawal::Error);
    }
    let sat_reduced = recipe.saturation != sat_fitted;
    let joint_base = crate::fit_zoned::joint_reading_with_evidence(
        sp,
        tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    let mut after_px = pixels_of(&render::develop_preview(s_img, &recipe));
    let mut joint_after = crate::fit_zoned::joint_reading_with_evidence(
        &after_px,
        tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    let mut harm = terminal_harm(err_before, err_after, None, None);
    if harm.scalar
        && detail_supported
        && only_detail_and_quantized_companions(&recipe, base)
        && detail_regression_is_bounded(
            sp,
            &after_px,
            tp,
            structural,
            detail,
            err_before,
            err_after,
        )
    {
        harm.scalar = false;
    }
    let mut fit_regressed = false;
    let joint_regressed = harm.joint;
    if harm.any() {
        recipe = base.clone();
        // Preserve a budget-edge saturation request when that isolated
        // correction still satisfies the frame ruler. This keeps the
        // atmosphere cap observable even if a separate WB/tone combination
        // triggered the terminal reset.
        let mut kept_capped_sat = false;
        if sat_pegged {
            let capped_sat = sat_fitted.clamp(-budget.sat, budget.sat);
            recipe.saturation = round1(capped_sat);
            let sat_px = pixels_of(&render::develop_preview(s_img, &recipe));
            if look_err_with_evidence(&sat_px, tp, evidence) > err_before + 1e-4 {
                recipe = base.clone();
            } else {
                after_px = sat_px;
                joint_after = crate::fit_zoned::joint_reading_with_evidence(
                    &after_px,
                    tp,
                    &evidence.source_weights,
                    &evidence.target_weights,
                );
                kept_capped_sat = true;
            }
        }
        if !kept_capped_sat {
            after_px = sp.to_vec();
            joint_after = joint_base;
        }
        fit_regressed = true;
    }
    compose_report(
        recipe,
        Measured {
            err_before,
            err_after: look_err_with_evidence(&after_px, tp, evidence),
            joint_after,
            after_px: &after_px,
            tp,
            same_frame,
            mode: FitMode::Atmosphere,
            divergence,
            evidence,
            structural_evidence: Some(structural),
            defer_disclosure,
        },
        SolveFacts {
            budget: Some(budget),
            strength: Some(strength.get()),
            veto_luma: (budget.vetoes == VetoPolicy::Disclose).then(|| moved_unsupported_luma_range_names(sp, &after_px, veto_evidence)).flatten(),
            veto_hue: (budget.vetoes == VetoPolicy::Disclose).then_some(moved_hue).flatten(),
            wb_clamped: wb_clamped.then_some((ratio_before, clamped_ratio, wb_rotated_share, wb_rotation_coverage)),
            wb_search_bound,
            wb_rotation_coverage: Some(wb_rotation_coverage),
            wb_rotation_disclosure: wb_rotation_withheld.then_some((wb_rejected_rotation_share.max(wb_rotated_share), wb_rotation_coverage)),
            cast_admitted_by_strength: None,
            cast_admitted: None,
            cast_projected: None,
            wb_foreign_hue_withheld,
            wb_rotation_withheld,
            sat_pegged: sat_pegged.then_some(FitMode::Atmosphere),
            cast: CastOutcome::default(),
            evidence_refused: evidence_has_one_sided(evidence),
            sat_fitted: sat_reduced.then_some(sat_fitted),
            regressed: fit_regressed.then_some(joint_regressed),
            detail,
            detail_withheld: !detail_supported,
            robust: None,
            paired: false,
            vouched_bands: None,
            hsl: hsl_facts,
            atmosphere_reference,
        },
    )
}

fn atmosphere_tone_curve_weighted(
    cur: &[[f32; 3]],
    tgt: &[[f32; 3]],
    cur_weights: &[f32],
    tgt_weights: &[f32],
    min_slope: f32,
    max_slope: f32,
) -> Vec<CurvePoint> {
    let (cc, tc) = (
        weighted_cdf(cur, cur_weights, luma601),
        weighted_cdf(tgt, tgt_weights, luma601),
    );
    let mut points = vec![CurvePoint { input: 0, output: 0 }];
    let mut prev_input = 0u8;
    let mut prev_output = 0u8;
    for (index, p) in [0.05, 0.50, 0.95].into_iter().enumerate() {
        let input = (quantile(&cc, p) * 255.0).round().clamp(1.0, 254.0) as u8;
        let output = (quantile(&tc, p) * 255.0).round().clamp(1.0, 254.0) as u8;
        // Reserve one input code for each remaining robust quantile and the
        // fixed 255 endpoint. Even a strongly concentrated but non-degenerate
        // frame therefore keeps exactly five strictly ordered points.
        let upper = 252 + index as u8;
        let input = input.max(prev_input.saturating_add(1)).min(upper);
        let output = output.max(prev_output);
        points.push(CurvePoint { input, output });
        prev_input = input;
        prev_output = output;
    }
    points.push(CurvePoint { input: 255, output: 255 });
    project_curve_slopes(&points, min_slope, max_slope)
}

fn cast_admitted_by_strength(measured_ratio: f32, budget: f32, strength: f32) -> bool {
    strength > crate::recipe::GradeStrength::DEFAULT
        && measured_ratio > CAST_ACCEPT_RATIO
        && measured_ratio <= budget
}

/// Constrained monotone projection with fixed x coordinates and fixed endpoint
/// values. Slopes are redistributed across neighboring segments; no point is
/// deleted. Inputs already inside the budget return byte-identically.
fn project_curve_slopes(points: &[CurvePoint], min_slope: f32, max_slope: f32) -> Vec<CurvePoint> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let slopes: Vec<f32> = points
        .windows(2)
        .map(|pair| {
            (pair[1].output as f32 - pair[0].output as f32)
                / (pair[1].input as f32 - pair[0].input as f32).max(1.0)
        })
        .collect();
    if slopes.iter().all(|&s| s >= min_slope - 1e-6 && s <= max_slope + 1e-6) {
        return points.to_vec();
    }
    let dx: Vec<f32> = points
        .windows(2)
        .map(|pair| (pair[1].input - pair[0].input) as f32)
        .collect();
    let mut projected: Vec<f32> = slopes.iter().map(|&s| s.clamp(min_slope, max_slope)).collect();
    let target = points.last().unwrap().output as f32 - points[0].output as f32;
    let current: f32 = projected.iter().zip(&dx).map(|(s, x)| s * x).sum();
    let delta = target - current;
    if delta > 0.0 {
        let capacity: f32 = projected.iter().zip(&dx).map(|(s, x)| (max_slope - s) * x).sum();
        if capacity > 1e-6 {
            let fraction = (delta / capacity).clamp(0.0, 1.0);
            for slope in &mut projected {
                *slope += fraction * (max_slope - *slope);
            }
        }
    } else if delta < 0.0 {
        let capacity: f32 = projected.iter().zip(&dx).map(|(s, x)| (s - min_slope) * x).sum();
        if capacity > 1e-6 {
            let fraction = (-delta / capacity).clamp(0.0, 1.0);
            for slope in &mut projected {
                *slope -= fraction * (*slope - min_slope);
            }
        }
    }

    let first = points[0].output as f32;
    let end = points.last().unwrap();
    let mut y = first;
    let mut out = Vec::with_capacity(points.len());
    out.push(points[0]);
    for i in 1..points.len() - 1 {
        y += projected[i - 1] * dx[i - 1];
        let prev = out[i - 1].output as i32;
        let seg_dx = dx[i - 1] as i32;
        let remaining_dx = end.input as i32 - points[i].input as i32;
        let lower = (prev + (min_slope * seg_dx as f32).ceil() as i32)
            .max(end.output as i32 - (max_slope * remaining_dx as f32).floor() as i32);
        let upper = (prev + (max_slope * seg_dx as f32).floor() as i32)
            .min(end.output as i32 - (min_slope * remaining_dx as f32).ceil() as i32);
        let wanted = y.round() as i32;
        // Integer endpoint constraints can cross by one code even though the
        // floating-point slope interval is feasible. No u8 value can satisfy
        // both rounded inequalities in that case; take the nearer boundary
        // and keep the unavoidable error to one code instead of panicking.
        let bounded = if lower <= upper {
            wanted.clamp(lower, upper)
        } else if (wanted - lower).abs() <= (wanted - upper).abs() {
            lower
        } else {
            upper
        };
        let output = bounded.clamp(0, 255) as u8;
        out.push(CurvePoint { input: points[i].input, output });
    }
    out.push(*end);
    out
}

// --------------------------------------------------------------------------
// per-band colour mixer (stage 4a)
// --------------------------------------------------------------------------

/// Why one colour band was left neutral. Typed, because "this band could not
/// be measured" and "this band already matched" are different claims and must
/// never reach the user as the same sentence — the standing evidence rule
/// (one-sided is UNMEASURABLE, not equal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HslBandRefusal {
    /// Populated on exactly one side.
    OneSided,
    /// Under the population line on both sides: nothing to measure.
    Sparse,
    /// Populated on both sides, but the structural evidence did not survive,
    /// so the two populations are not testimony about the same content.
    Divergent,
}

impl HslBandRefusal {
    fn label(self) -> &'static str {
        match self {
            HslBandRefusal::OneSided => "one-sided",
            HslBandRefusal::Sparse => "sparse on both sides",
            HslBandRefusal::Divergent => "structurally divergent",
        }
    }
}

/// The per-band stage gave back everything it fitted, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HslWithdrawal {
    /// The composed frame did not end closer to the target.
    Error,
    /// It would have carried pixels through hue bands no evidence covers.
    Blind,
}

/// What the per-band stage decided, for the disclosure. What it MOVED is not
/// here on purpose: that is a property of the recipe [`compose_report`] is
/// holding, so it is read off the recipe there and cannot go stale when a
/// later do-no-harm loop shrinks the mixer (or resets the whole recipe).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HslStageFacts {
    /// Bands the two-sided population gate refused, each with its reason.
    refused: String,
    withdrawn: Option<HslWithdrawal>,
}

/// Largest single-iteration step of the per-band chase, mirroring the global
/// chroma chase's own cap: the ratio is read through a chroma-gated renderer,
/// so one iteration must not be allowed to swing a band across the axis.
const HSL_BAND_STEP: f32 = 40.0;
/// A band mean below this carries no usable ratio — dividing by it turns
/// renderer rounding into a full-scale demand.
const HSL_BAND_MIN_MEAN: f64 = 0.02;

/// One shrink step of the mixer: halve every axis, and snap a band under one
/// unit to neutral so the shrink always REACHES zero instead of approaching
/// it (the same "below 4, go to zero" device the saturation loop uses).
fn halved_hsl(hsl: &crate::recipe::Hsl) -> crate::recipe::Hsl {
    let axis = |values: &[f32; 8]| -> [f32; 8] {
        std::array::from_fn(|i| {
            let v = round1(values[i] * 0.5);
            if v.abs() < 1.0 { 0.0 } else { v }
        })
    };
    crate::recipe::Hsl {
        hue: hsl.hue,
        saturation: axis(&hsl.saturation),
        luminance: axis(&hsl.luminance),
    }
}

/// Solve `hsl.saturation` and `hsl.luminance` from POPULATION statistics, one
/// ACR band at a time.
///
/// WHY this is legitimate where a per-band HUE rotation is not, and why it is
/// legitimate against a target whose pixels do not correspond: it is the
/// per-band form of the argument that already lets Atmosphere mode fit one
/// global saturation and one white balance. A band's mean chroma and mean
/// lightness are marginal statistics of a sub-population; matching them needs
/// no pixel pairing, only the claim that both frames' members of that band
/// are measurements of the same subject. That claim is exactly what the
/// evidence model already adjudicates — so this stage asks it, and asks it
/// with the SAME criterion the unrepresented-controls disclosure reads
/// (`evidence.hue[band].weight > 0` plus a two-sided [`EVIDENCE_MIN_SHARE`]
/// of the chromatic mass). One gate, two consumers; a band either side cannot
/// see is left at zero and NAMED, never silently read as "equal".
///
/// `hsl.hue` is never written. Rotating a band re-populates it, so its own
/// evidence is circular (project memory: "the hue evidence is circular"), and
/// it is the axis that turned brown rock olive in the 2026-07-07 failure.
///
/// The chase is closed-loop through the real engine for the same reason the
/// global chroma chase is: `apply_hsl` runs before saturation, the wheels and
/// clarity, blends two bands per pixel through a partition of unity and fades
/// itself out below chroma 0.22 — so the open-loop ratio is a first step, not
/// an answer. Two iterations, then a do-no-harm that shrinks to zero.
#[allow(clippy::too_many_arguments)]
fn fit_hsl_stage(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<(&[f32], &[[f32; 3]])>,
    budget: FitBudget,
    recipe: &mut EditRecipe,
) -> HslStageFacts {
    let mut facts = HslStageFacts::default();
    let restore = recipe.hsl.clone();
    let before_px = pixels_of(&render::develop_preview(s_img, recipe));
    let err_before = look_err_with_evidence(&before_px, tp, evidence);
    // Whether the recipe ALREADY moved pixels through unmeasured bands is not
    // this stage's fault and not this stage's to fix (the pipeline-end loop
    // owns that case); only movement this stage ADDS is its own.
    let blind_before =
        moved_unsupported_hue_range_names_vouched(sp, &before_px, evidence, vouch).is_some();

    // --- admission --------------------------------------------------------
    let (sa, ta) = band_stats_weighted(&before_px, &evidence.source_hue_weights);
    let (sb, tb) = band_stats_weighted(tp, &evidence.target_hue_weights);
    let mut admitted = [false; EVIDENCE_HUE_BANDS];
    let mut refused: Vec<String> = Vec::new();
    for band in 0..EVIDENCE_HUE_BANDS {
        let Some(range) = evidence.hue.get(band) else { continue };
        let verdict = if !range.source_populated && !range.target_populated {
            Some(HslBandRefusal::Sparse)
        } else if !range.source_populated || !range.target_populated {
            Some(HslBandRefusal::OneSided)
        } else if range.weight <= 0.0 {
            Some(HslBandRefusal::Divergent)
        } else if ta < 1.0 || tb < 1.0 {
            Some(HslBandRefusal::Sparse)
        } else {
            let source_ok = sa[band].w / ta >= EVIDENCE_MIN_SHARE as f64;
            let target_ok = sb[band].w / tb >= EVIDENCE_MIN_SHARE as f64;
            match (source_ok, target_ok) {
                (true, true) => None,
                (false, false) => Some(HslBandRefusal::Sparse),
                _ => Some(HslBandRefusal::OneSided),
            }
        };
        match verdict {
            None => admitted[band] = true,
            Some(reason) => {
                // Only bands the PICTURE actually holds are worth naming: a
                // band absent from both frames is not a refusal anyone can
                // act on, and listing all eight on a grey frame would bury
                // the ones that mean something.
                if range.source_populated || range.target_populated {
                    refused.push(format!("{} ({})", range.label, reason.label()));
                }
            }
        }
    }
    facts.refused = refused.join(", ");
    if !admitted.iter().any(|&band| band) {
        return facts;
    }

    // --- the chase, closed-loop through the real engine --------------------
    for _ in 0..2 {
        let cur = pixels_of(&render::develop_preview(s_img, recipe));
        let (cs, _) = band_stats_weighted(&cur, &evidence.source_hue_weights);
        let mut moved_any = false;
        for band in 0..EVIDENCE_HUE_BANDS {
            if !admitted[band] || cs[band].w <= 0.0 || sb[band].w <= 0.0 {
                continue;
            }
            // The engine reads `new_s = s * (1 + sat/100)` and
            // `new_l = l * (1 + 0.5 * lum/100)`, and chroma is proportional to
            // `s` and luma to `l` at fixed hue, so a band's mean-chroma ratio
            // IS the saturation demand and its mean-luma ratio is the
            // luminance demand at half the sensitivity.
            let axes = [
                (cs[band].c / cs[band].w, sb[band].c / sb[band].w, 100.0f32),
                (cs[band].y / cs[band].w, sb[band].y / sb[band].w, 200.0f32),
            ];
            for (axis, (now, want, scale)) in axes.into_iter().enumerate() {
                if now < HSL_BAND_MIN_MEAN {
                    continue;
                }
                let step = (((want / now) - 1.0) as f32 * scale)
                    .clamp(-HSL_BAND_STEP, HSL_BAND_STEP);
                if step.abs() < 1.0 {
                    continue;
                }
                let slot = if axis == 0 {
                    &mut recipe.hsl.saturation[band]
                } else {
                    &mut recipe.hsl.luminance[band]
                };
                let next = round1((*slot + step).clamp(-budget.hsl_band, budget.hsl_band));
                if next != *slot {
                    moved_any = true;
                }
                *slot = next;
            }
        }
        if !moved_any {
            break;
        }
    }
    let fitted = recipe.hsl.clone();
    if fitted == restore {
        return facts;
    }

    // --- do-no-harm, the stage's own --------------------------------------
    // The same err_before/err_after discipline the saturation pull-back
    // answers to, applied where the move is generated instead of three stages
    // later: halve the whole vector until the frame's look error stops
    // objecting AND the finished render stops carrying pixels through hue
    // bands this stage's own gate never measured. Zero is always reachable.
    let mut reason: Option<HslWithdrawal> = None;
    let mut candidate = fitted;
    loop {
        if candidate.is_neutral() {
            recipe.hsl = restore;
            facts.withdrawn = reason.or(Some(HslWithdrawal::Error));
            return facts;
        }
        recipe.hsl = candidate.clone();
        let px = pixels_of(&render::develop_preview(s_img, recipe));
        let regressed = look_err_with_evidence(&px, tp, evidence) > err_before + 1e-4;
        let blind_new = budget.vetoes == VetoPolicy::Withhold
            && !blind_before
            && moved_unsupported_hue_range_names_vouched(sp, &px, evidence, vouch).is_some();
        if !regressed && !blind_new {
            return facts;
        }
        reason = Some(if blind_new { HslWithdrawal::Blind } else { HslWithdrawal::Error });
        candidate = halved_hsl(&candidate);
    }
}

/// What a [`FitReport`]'s notes need that only a MEASUREMENT can supply — all
/// of it re-derivable from a (source, target, recipe) triple at any later time.
struct Measured<'a> {
    err_before: f32,
    err_after: f32,
    joint_after: Option<crate::fit_zoned::JointReading>,
    /// The FINISHED render, i.e. the recipe applied to the source thumbnail.
    after_px: &'a [[f32; 3]],
    /// The target thumbnail.
    tp: &'a [[f32; 3]],
    same_frame: bool,
    mode: FitMode,
    divergence: Divergence,
    evidence: &'a EvidenceModel,
    structural_evidence: Option<&'a EvidenceModel>,
    defer_disclosure: bool,
}

/// …and what only the SOLVE can supply: decisions the solver made on its way to
/// the recipe, which no later re-measurement of that recipe can recover.
///
/// Split out from [`Measured`] precisely because the split is the contract for
/// [`rescore_report`]: a recipe someone ADJUSTED after the solve can honestly
/// re-derive everything on the measured side and nothing on this one.
#[derive(Clone)]
struct SolveFacts {
    /// Budget used by the atmosphere solve, when applicable.
    budget: Option<FitBudget>,
    /// Panel strength used to derive the budget. Kept absent for historical
    /// rescoring and Full-mode reports so the shipped default remains stable.
    strength: Option<f32>,
    /// Unsupported movement retained as a high-strength disclosure.
    veto_luma: Option<String>,
    veto_hue: Option<String>,
    wb_clamped: Option<(f32, f32, f32, f32)>,
    /// Free white-balance search landed on the finite Kelvin domain edge.
    wb_search_bound: Option<f32>,
    /// Coverage from the same WB rotation census used for its gate.
    wb_rotation_coverage: Option<f32>,
    wb_rotation_disclosure: Option<(f32, f32)>,
    /// The fitted WB was returned to as-shot because it created target-foreign hues.
    wb_foreign_hue_withheld: bool,
    /// The fitted WB was returned to as-shot because it exceeded the strength
    /// budget's weighted region-rotation allowance.
    wb_rotation_withheld: bool,
    /// The chroma chase hit this mode's model cap with demand to spare. The
    /// mode travels with the solve fact because `rescore_report` may classify
    /// an old adjusted recipe differently without changing what the original
    /// solve actually did.
    sat_pegged: Option<FitMode>,
    /// Which of the colour stage's gates (if either) refused the cast curves.
    cast: CastOutcome,
    /// Accepted cast whose measured error ratio is above the shipped gate but
    /// within a widened high-strength budget.
    cast_admitted_by_strength: Option<(f32, f32)>,
    /// Every gate reading of a cast that was ADMITTED, so the shipped
    /// curves disclose the numbers they passed on. `None` = no curves
    /// shipped, in any of THREE ways: a gate refused them, none were fitted
    /// at all, or the TERMINAL do-no-harm check reset the whole recipe to
    /// the calibration base. The third leaves the gates' own verdict at
    /// "admitted" while nothing of the solve — curves included — ships,
    /// which is why the construction site ands in `!fit_regressed`.
    cast_admitted: Option<CastReadings>,
    /// v1.2.3: the cast was PROJECTED — the fan gate convicted the fitted
    /// curves and a shrunk version of them shipped instead. Mutually
    /// exclusive with `cast_admitted` (a projected cast was not admitted as
    /// fitted) and carrying the same `!fit_regressed` guard, for the same
    /// reason: the terminal do-no-harm reset can leave the gates' verdict at
    /// "projected" while nothing of the solve ships.
    cast_projected: Option<CastProjection>,
    /// The existing evidence gates withheld a one-sided range. This is the
    /// cause carried into the FAR classifier; it is not a second refusal flag.
    evidence_refused: bool,
    /// `Some(sat_fitted)` when the do-no-harm loop shrank saturation away from
    /// the chroma-matched value the chase produced.
    sat_fitted: Option<f32>,
    /// `Some(joint_arm_fired)` when the TERMINAL do-no-harm check reset the
    /// whole recipe to the calibration base.
    regressed: Option<bool>,
    detail: (f32, f32),
    detail_withheld: bool,
    /// Paired robust regression engaged and down-weighted this share of the
    /// comparable pixels (plus the luma ranges holding the rejected mass).
    /// `None` = the paired path did not run: nothing was rejected AND nothing
    /// was measured — two silences the disclosure keeps apart by speaking
    /// only when a measurement exists.
    robust: Option<(f32, String)>,
    /// The tone map came from paired pixels, not marginal CDF transport — the
    /// summary must not claim the target is unaligned when the solve just
    /// used its alignment.
    paired: bool,
    /// One-sided hue bands that vouched convergence carried movement through
    /// on the finished render — disclosed so the withheld-note's "vetoed
    /// movement" claim is never silently contradicted.
    vouched_bands: Option<String>,
    /// The per-band colour mixer's own verdicts: which bands it could not
    /// measure, and whether it gave back what it fitted.
    hsl: HslStageFacts,
    /// R30 R2: which population the Atmosphere white balance and exposure
    /// were read over. Only the solve knows — a re-measurement of the
    /// finished recipe cannot tell a whole-frame median from a shared-content
    /// one — which is why it rides here and not on [`Measured`].
    atmosphere_reference: AtmosphereReference,
}

/// Build the rationale, the typed notes and the confidence of ONE fit report.
///
/// The single derivation path for every fit note in this module —
/// [`fit_recipe_from`] ends here, and so does [`rescore_report`]. Written as a
/// function rather than left inline for exactly that reason: the deep
/// reverse-fit used to CLONE a solved report's notes onto an adjusted recipe,
/// which persisted a rationale describing settings the photo no longer had
/// (R23 review MED-3). A second, "refresh the notes" derivation would have had
/// the same failure mode one release later, so there is only this one.
///
/// Honest-mismatch notes are the point: the user reads WHY a fit stayed
/// approximate instead of wondering what went wrong (real-machine feedback,
/// 2026-07-09: a palette-transplant target produced a faithful-but-ugly
/// max-saturation fit with zero explanation).
fn compose_report(mut recipe: EditRecipe, m: Measured<'_>, solve: SolveFacts) -> FitReport {
    use crate::rationale::{keys, push_note, Note};
    let (err_before, err_after) = (m.err_before, m.err_after);
    let mut notes: Vec<Note> = Vec::new();
    let mut rationale = String::new();
    // The summary comes first; the note fragments append after it. Two full
    // summary keys instead of a nested English fragment argument — a
    // fragment inside an arg would stay English in the zh rendering.
    let summary_key = match m.mode {
        FitMode::Atmosphere => keys::FIT_SUMMARY_ATMOSPHERE,
        FitMode::Full if solve.paired && recipe.tone_curve.is_empty() => {
            keys::FIT_SUMMARY_NO_CURVE_PAIRED
        }
        FitMode::Full if solve.paired => keys::FIT_SUMMARY_WITH_CURVE_PAIRED,
        FitMode::Full if recipe.tone_curve.is_empty() => keys::FIT_SUMMARY_NO_CURVE,
        FitMode::Full => keys::FIT_SUMMARY_WITH_CURVE,
    };
    push_note(
        &mut rationale,
        &mut notes,
        Note::new(
            summary_key,
            vec![
                ("err_before", format!("{err_before:.3}")),
                ("err_after", format!("{err_after:.3}")),
                ("d", format!("{:.3}", m.divergence.d)),
            ],
        ),
    );
    // Keyed on the RESIDUAL, not the pre-fit distance: a large but perfectly
    // fittable tone gap (2 EV of exposure) starts far and ends near — only a
    // look the model cannot approach deserves the warning.
    if err_after > FIT_FAR_ERR {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(keys::FIT_NOTE_FAR, vec![("err_after", format!("{err_after:.2}"))]),
        );
    }
    // The joint value-range reading, ALWAYS reported when it has one: it is
    // the only number in this report that `look_err` did not produce, and
    // burying it behind a threshold would leave the user with a single
    // self-graded score again. Named "joint distribution", never "region" —
    // the buckets are value ranges whose pixels are scattered frame-wide.
    if let Some(j) = m.joint_after {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_JOINT,
                vec![
                    ("weighted", format!("{:.3}", j.weighted)),
                    ("worst", format!("{:.3}", j.worst)),
                    ("label", j.worst_label.to_string()),
                    ("n", j.buckets.to_string()),
                ],
            ),
        );
        if let Some(cause) = crate::fit_zoned::classify_joint_far(
            j.weighted,
            solve.evidence_refused || evidence_has_one_sided(m.evidence),
        ) {
            push_note(&mut rationale, &mut notes, Note::plain(cause.note_key()));
        }
    } else {
        // FAIL-OPEN, disclosed. "No opinion" and "no problem" are different
        // claims and must not read the same (E-15): with no second reading
        // confidence still carries the shared evidence-identifiability cap.
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_JOINT_NONE));
    }
    if let Some(sat_mode) = solve.sat_pegged {
        let key = if sat_mode == FitMode::Atmosphere {
            keys::FIT_NOTE_ATMOSPHERE_SAT_PEGGED
        } else {
            keys::FIT_NOTE_SAT_PEGGED
        };
        let args = if sat_mode == FitMode::Atmosphere {
            vec![("cap", format!("{:.0}", solve.budget.map(|b| b.sat).unwrap_or(ATMOSPHERE_SAT_LIMIT)))]
        } else {
            Vec::new()
        };
        push_note(&mut rationale, &mut notes, Note::new(key, args));
    }
    if let Some(joint_regressed) = solve.regressed {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_REGRESSED));
        if joint_regressed {
            // WHICH check refused matters: the scalar arm and this one see
            // different damage, and "the value ranges drifted" is actionable
            // where "it rendered farther" is not.
            push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_JOINT_REGRESSED));
            push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_EVIDENCE_CONTRADICTED));
        }
    } else if let Some(sat_fitted) = solve.sat_fitted {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_SAT_REDUCED,
                vec![
                    ("sat_fitted", format!("{sat_fitted:+.0}")),
                    ("sat_now", format!("{:+.0}", recipe.saturation)),
                ],
            ),
        );
    }
    if m.evidence.identifiability < 0.08 {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_EVIDENCE_UNMEASURABLE));
    }
    if let Some(note) = solve.cast.note() {
        push_note(&mut rationale, &mut notes, note);
    }
    // The stage's other outcome, silent until v1.2.3: the curves SHIPPED.
    // Admission was disclosed only when the strength budget bought it
    // (`FIT_NOTE_CAST_ADMITTED_BY_STRENGTH`, below), so an ordinary
    // admission — the commonest result of the whole stage — reached the user
    // as an unexplained presence, exactly the asymmetry R23-6 A-2 fixed on
    // the rejection side. The four gates' own numbers ride THREE notes: the
    // two that can abstain each get a measured and a not-measurable clause,
    // so a census that never ran never reaches the user as 0.000.
    // v1.2.3 — the stage's THIRD outcome: the fitted curves were convicted
    // by the fan gate and a projected, milder cast shipped in their place.
    // Exclusive with the admission note at the fact site, so one cast never
    // reaches the user as two different accounts of itself.
    if let Some(p) = solve.cast_projected {
        for note in cast_projection_notes(p) {
            push_note(&mut rationale, &mut notes, note);
        }
    }
    if let Some(r) = solve.cast_admitted {
        for note in cast_admission_notes(r) {
            push_note(&mut rationale, &mut notes, note);
        }
    }
    if let Some(cast) = m.evidence.global_cast {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_GLOBAL_CAST,
                vec![("rotation", format!("{:+.0}", cast.rotation_deg)), ("ratio", format!("{:.2}", cast.chroma_ratio))],
            ),
        );
    }
    // The shipped default remains byte-identical: its existing Atmosphere
    // confidence note already states the cap. Non-default panel values get an
    // explicit strength disclosure so the rationale names the budget input.
    if let Some(strength) = solve.strength
        && (strength - crate::recipe::GradeStrength::DEFAULT).abs() > 1e-6
    {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_STRENGTH,
                vec![("pct", format!("{:.0}", strength * 100.0)), ("s", format!("{strength:.4}"))],
            ),
        );
    }
    if let Some((from, to, rotated_share, coverage)) = solve.wb_clamped {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_WB_CLAMPED,
                vec![
                    ("from", format!("{from:.2}")),
                    ("to", format!("{to:.2}")),
                    ("rotated_share", format!("{rotated_share:.3}")),
                    ("coverage", format!("{coverage:.3}")),
                ],
            ),
        );
    }
    if solve.wb_foreign_hue_withheld {
        push_note(
            &mut rationale,
            &mut notes,
            Note::plain(keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE),
        );
    }
    if solve.wb_rotation_withheld {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_WB_WITHHELD_ROTATION,
                vec![
                    ("rotated_share", format!("{:.3}", solve.wb_rotation_disclosure.map(|v| v.0).unwrap_or(0.0))),
                    ("coverage", format!("{:.3}", solve.wb_rotation_disclosure.map(|v| v.1).unwrap_or_else(|| solve.wb_rotation_coverage.unwrap_or(0.0)))),
                ],
            ),
        );
    }
    if let Some(k) = solve.wb_search_bound {
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_WB_SEARCH_BOUND, vec![("k", format!("{k:.0}"))]));
    }
    if let Some((ratio, budget)) = solve.cast_admitted_by_strength {
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_CAST_ADMITTED_BY_STRENGTH, vec![("ratio", format!("{ratio:.3}")), ("budget", format!("{budget:.3}"))]));
    }
    if let Some(ranges) = &solve.veto_luma {
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_VETO_DISCLOSED, vec![("kind", "luma ranges".into()), ("ranges", ranges.clone())]));
    }
    if let Some(ranges) = &solve.veto_hue {
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_VETO_DISCLOSED, vec![("kind", "hue bands".into()), ("ranges", ranges.clone())]));
    }
    // Every Atmosphere `Measured` carries its structural model (the solve and
    // the rescore both build one); a breach is a programming error, and a
    // photo app must not panic over a missing disclosure line.
    debug_assert!(
        m.mode != FitMode::Atmosphere || m.structural_evidence.is_some(),
        "an Atmosphere report retains its structural evidence"
    );
    if let (FitMode::Atmosphere, Some(structural)) = (m.mode, m.structural_evidence) {
        let (luma_ranges, hue_bands) = withheld_range_names(structural);
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE,
                vec![
                    (
                        "luma_ranges",
                        if luma_ranges.is_empty() { "none".into() } else { luma_ranges },
                    ),
                    (
                        "hue_bands",
                        if hue_bands.is_empty() { "none".into() } else { hue_bands },
                    ),
                ],
            ),
        );
        // R30 batch 1 (R2-lite), zero behaviour change: the sentence above
        // says which EVIDENCE this mode read; this one says which POPULATION
        // its two robust controls were read OVER. `median(target) /
        // median(source)` is a distribution-level pairing, and a
        // distribution-level pairing presumes the two distributions describe
        // the same content — which is exactly what selecting Atmosphere
        // denies. The assumption was always in the code; only now is it in
        // the rationale.
        // R30 R2: the whole-frame sentence is now the statement of ONE of
        // three cases, not of the only case. Its key and its wording are
        // untouched — it still says exactly what it always said, and it is
        // still what an unrestricted solve deserves.
        match solve.atmosphere_reference {
            AtmosphereReference::WholeFrame => {
                push_note(
                    &mut rationale,
                    &mut notes,
                    Note::plain(keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
                );
            }
            AtmosphereReference::Thin { source, target } => {
                push_note(
                    &mut rationale,
                    &mut notes,
                    Note::plain(keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
                );
                push_note(
                    &mut rationale,
                    &mut notes,
                    Note::new(
                        keys::FIT_ATMOSPHERE_REFERENCE_THIN,
                        vec![
                            ("src", format!("{:.0}", source * 100.0)),
                            ("tgt", format!("{:.0}", target * 100.0)),
                            (
                                "floor",
                                format!("{:.0}", SHARED_POPULATION_MIN_RETENTION * 100.0),
                            ),
                        ],
                    ),
                );
            }
            AtmosphereReference::SharedContent { source, target } => {
                push_note(
                    &mut rationale,
                    &mut notes,
                    Note::new(
                        keys::FIT_ATMOSPHERE_REFERENCE_SHARED,
                        vec![
                            ("tau", format!("{CONFIDENT_MATCH:.2}")),
                            ("src", format!("{:.0}", source * 100.0)),
                            ("tgt", format!("{:.0}", target * 100.0)),
                        ],
                    ),
                );
            }
        }
    }
    let (withheld_luma, withheld_hue) = withheld_range_names(m.evidence);
    let all_ranges = m.evidence.luma.iter().chain(&m.evidence.hue);
    let one_sided = all_ranges
        .clone()
        .filter(|r| {
            r.weight <= 0.0
                && r.source_populated != r.target_populated
        })
        .map(|r| r.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let sparse = all_ranges
        .clone()
        .filter(|r| !r.source_populated && !r.target_populated)
        .map(|r| r.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let divergent = divergent_range_names(m.evidence);
    if !withheld_luma.is_empty() || !withheld_hue.is_empty() {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_EVIDENCE_WITHHELD,
                vec![
                    ("luma_ranges", if withheld_luma.is_empty() { "none".into() } else { withheld_luma }),
                    ("hue_bands", if withheld_hue.is_empty() { "none".into() } else { withheld_hue }),
                    ("one_sided", if one_sided.is_empty() { "none".into() } else { one_sided }),
                    ("sparse", if sparse.is_empty() { "none".into() } else { sparse }),
                    ("divergent", if divergent.is_empty() { "none".into() } else { divergent }),
                ],
            ),
        );
    }
    if let Some(bands) = &solve.vouched_bands {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_VOUCHED_CONVERGENCE,
                vec![("bands", bands.clone())],
            ),
        );
    }
    if let Some((share, ranges)) = &solve.robust
        && *share >= ROBUST_REJECT_DISCLOSE_MIN
    {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_ROBUST_REJECTED,
                vec![
                    ("pct", format!("{:.0}", share * 100.0)),
                    (
                        "ranges",
                        if ranges.is_empty() { "scattered".into() } else { ranges.clone() },
                    ),
                ],
            ),
        );
    }
    if solve.detail_withheld {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_DETAIL_WITHHELD));
    } else if solve.detail.0.abs() > 0.0 || solve.detail.1.abs() > 0.0 {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_DETAIL,
                vec![
                    ("clarity", format!("{:+.0}", solve.detail.0)),
                    ("texture", format!("{:+.0}", solve.detail.1)),
                ],
            ),
        );
    }
    // The per-band colour mixer. What it MOVED is read off the recipe this
    // report describes rather than carried from the stage, so the numbers
    // cannot drift from what shipped when a later do-no-harm loop shrinks the
    // mixer or resets the whole recipe; the refusals and the withdrawal
    // verdict are solve facts no re-measurement could recover.
    let hsl_moved = (0..EVIDENCE_HUE_BANDS)
        .filter(|&band| {
            recipe.hsl.saturation[band] != 0.0 || recipe.hsl.luminance[band] != 0.0
        })
        .map(|band| {
            format!(
                "{} sat {:+.0} lum {:+.0}",
                crate::recipe::HSL_BANDS[band],
                recipe.hsl.saturation[band],
                recipe.hsl.luminance[band]
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if !hsl_moved.is_empty() || !solve.hsl.refused.is_empty() {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_HSL_BANDS,
                vec![
                    ("moved", if hsl_moved.is_empty() { "none".into() } else { hsl_moved }),
                    (
                        "refused",
                        if solve.hsl.refused.is_empty() {
                            "none".into()
                        } else {
                            solve.hsl.refused.clone()
                        },
                    ),
                ],
            ),
        );
    }
    if let Some(withdrawal) = solve.hsl.withdrawn {
        push_note(
            &mut rationale,
            &mut notes,
            Note::plain(match withdrawal {
                HslWithdrawal::Error => keys::FIT_NOTE_HSL_WITHDRAWN_ERROR,
                HslWithdrawal::Blind => keys::FIT_NOTE_HSL_WITHDRAWN_BLIND,
            }),
        );
    }
    // Which controls this target's look may need that the solver has no way
    // to reach — SPECIFIC to this pair, not the blanket sentence the summary
    // already carries (R23-6 A-5).
    if !m.defer_disclosure
        && let Some(n) = unrepresented_note(&recipe, m.after_px, m.tp, err_after, m.mode, m.evidence)
    {
        push_note(&mut rationale, &mut notes, n);
    }
    if !m.same_frame {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_NOT_SAME_FRAME));
    }
    if m.mode == FitMode::Atmosphere {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_ATMOSPHERE_CONFIDENCE,
                vec![("cap", format!("{:.2}", solve.budget.map(|b| b.confidence_cap).unwrap_or(ATMOSPHERE_CONFIDENCE_CAP)))],
            ),
        );
    }
    recipe.rationale = rationale;
    // Confidence: the look-error ladder, and never MORE than the joint
    // reading's own ladder allows. One-directional on purpose — a reading
    // that cannot see (`None`) must not raise a claim, and the two metrics
    // disagreeing means the honest answer is the lower one. On the fixture
    // set this is what finally separates a fit that reproduces the look from
    // one that only scores well: the unreachable-repaint pair reads 0.52 by
    // look error and 0.25 here.
    recipe.confidence = match m.joint_after {
        Some(j) => confidence_from_look_err(err_after).min(clamp_confidence(
            1.0 - j.weighted * crate::fit_zoned::JOINT_CONFIDENCE_SLOPE,
        )),
        None => confidence_from_look_err(err_after),
    };
    // Identifiability is a cap, not a bonus: residuals from invented or
    // one-sided ranges cannot support a confident claim even when small.
    let movement = movement_identifiability(m.after_px, m.evidence);
    let identified_after = look_err_with_evidence(m.after_px, m.tp, m.evidence);
    let identified_gain = if err_before <= FIT_QUANT {
        1.0
    } else {
        ((err_before - identified_after) / err_before).clamp(0.0, 1.0)
    };
    let effective_identifiability =
        m.evidence.identifiability * movement * identified_gain.sqrt();
    let evidence_cap =
        CONFIDENCE_FLOOR + (CONFIDENCE_CEIL - CONFIDENCE_FLOOR) * effective_identifiability;
    recipe.confidence = recipe.confidence.min(clamp_confidence(evidence_cap));
    // …and never more than a fit whose two sides are not the same rectangle
    // may claim (R24 batch 2). THIRD in the same one-directional chain, and
    // last because it is the only one of the three that no measurement of
    // these pixels can raise: both readings above are taken over populations
    // the crop already made incomparable, so this cap overrides a high number
    // rather than competing with it. `min`, not an assignment — a fit that is
    // ALSO far by its own residual keeps the lower claim.
    if !m.same_frame {
        recipe.confidence = recipe.confidence.min(NOT_SAME_FRAME_CONFIDENCE_CAP);
    }
    if m.mode == FitMode::Atmosphere {
        recipe.confidence = recipe.confidence.min(
            solve.budget.map(|b| b.confidence_cap).unwrap_or(ATMOSPHERE_CONFIDENCE_CAP),
        );
    }
    if let Some(budget) = solve.budget
        && budget.vetoes == VetoPolicy::Disclose
        && (solve.veto_luma.is_some() || solve.veto_hue.is_some())
    {
        recipe.confidence = recipe.confidence.min(budget.confidence_cap);
    }
    recipe.clamp();
    FitReport {
        recipe,
        err_before,
        err_after,
        notes,
        mode: m.mode,
        divergence: m.divergence,
        evidence: m.evidence.clone(),
        structural_evidence: m.structural_evidence.cloned(),
        correspondence: None,
        atmosphere_reference: solve.atmosphere_reference,
    }
}

/// Add the pair-specific disclosure only after all zoned corrections have
/// rendered.  This keeps the note's `after_px` contract truthful.
pub(crate) fn append_finished_disclosure(
    report: &mut FitReport,
    after_px: &[[f32; 3]],
    tp: &[[f32; 3]],
) {
    if let Some(note) = unrepresented_note(
        &report.recipe,
        after_px,
        tp,
        report.err_after,
        report.mode,
        &report.evidence,
    ) {
        crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes, note);
    }
}

/// Re-measure an ADJUSTED recipe the way [`fit_recipe_from`] measures its own
/// output, and re-derive every note that describes an OUTCOME — through the
/// same [`compose_report`] the solver itself ends in.
///
/// For the ONE caller that legitimately hands back a recipe it did not itself
/// solve: the deep reverse-fit (R23-6 D, GUI `actions.rs` and the CLI's
/// `--deep`) may move a fitted recipe on the visual reviewer's say-so, and
/// reporting the solve's pre-adjustment numbers next to post-adjustment pixels
/// is exactly the kind of stale claim this round is about. Deterministic and
/// local — the same two renders the fit already pays for.
///
/// This REPLACED a numbers-only `rescore` returning `(err, confidence)`
/// (R23 review MED-3). That signature was the defect's enabler: it re-measured
/// honestly and left the caller holding a report whose notes it had to source
/// somewhere, and both call sites sourced them by `notes.clone()`. Every
/// outcome sentence in that clone then described the recipe BEFORE the move —
/// the joint numbers, the far-from-target verdict, the unrepresented-controls
/// diagnosis, and a `FIT_NOTE_SAT_REDUCED` quoting a saturation the recipe no
/// longer had. Worst of all it carried `FIT_NOTE_REGRESSED`, on which the GUI
/// raises 「THE REVERSE-FIT WAS DISCARDED — reset to neutral」: after a terminal
/// reset the deep arm can adopt base ± 10, and the user was told nothing had
/// been applied while ± 10 was persisted.
///
/// `prior` is the solved report's notes, and three families cross over — all
/// statements about the SOLVE that the adjustment cannot falsify:
///   * `FIT_NOTE_SAT_PEGGED` — the chroma chase hit the ±60 model cap, a fact
///     about the target's chroma being out of the model's reach;
///   * `FIT_NOTE_REHUE_BLOCKED` / `FIT_NOTE_CAST_REJECTED` — which gate refused
///     the colour stage, and the adjustment does not refit those curves.
///   * `FIT_NOTE_VOUCHED_CONVERGENCE` — which one-sided bands the paired solve
///     individually vouched; adjusting the recipe does not rerun that solve.
///
/// The three that are deliberately DROPPED rather than re-derived, because they
/// report an action the solver took on a recipe this report no longer describes:
/// `FIT_NOTE_REGRESSED`, `FIT_NOTE_JOINT_REGRESSED` (the terminal reset — the
/// adjusted recipe is not the neutral one that note is about, and re-running the
/// harm test here would either lie the same way or silently overrule the
/// caller's own adoption decision), and `FIT_NOTE_SAT_REDUCED` (the do-no-harm
/// loop's pull-back, whose "from X to Y" pair the adjustment breaks — and whose
/// attribution to that loop would be false once the deep step moved the dial
/// again). The caller states what it did through `FIT_NOTE_DEEP_ADOPTED`
/// instead, which is the honest owner of that sentence.
///
/// In Full mode `err_before` is the caller's, unchanged by construction. An
/// Atmosphere rescore rebuilds the same structure-blind ruler as the solve and
/// re-measures the untouched base on it, so the report cannot mix rulers.
pub fn rescore_report(
    src: &DynamicImage,
    target: &DynamicImage,
    recipe: &EditRecipe,
    err_before: f32,
    prior: &[crate::rationale::Note],
) -> FitReport {
    use crate::rationale::keys;
    let same_frame = same_frame_plausible(src, target);
    let (s, t) = analysis_pair(src, target);
    let tp = pixels_of(&t);
    let after_px = pixels_of(&render::develop_preview(&s, recipe));
    let base_px = pixels_of(&render::develop_preview(&s, &EditRecipe::default()));
    let structural = evidence_model_for(&base_px, &tp, s.width(), s.height());
    let carried = |k: &str| prior.iter().any(|n| n.key == k);
    let carried_arg = |note_key: &str, arg_key: &str| {
        prior
            .iter()
            .find(|note| note.key == note_key)
            .and_then(|note| note.args.iter().find(|(key, _)| *key == arg_key))
            .map(|(_, value)| value.clone())
    };
    let carried_strength = carried_strength_from_notes(prior);
    let carried_cast_admission = carried_arg(keys::FIT_NOTE_CAST_ADMITTED_BY_STRENGTH, "ratio")
        .and_then(|ratio| ratio.parse::<f32>().ok())
        .zip(carried_arg(keys::FIT_NOTE_CAST_ADMITTED_BY_STRENGTH, "budget").and_then(|budget| budget.parse::<f32>().ok()));
    // The admission readings ride the note, like every other carried fact:
    // a rescore re-renders and re-scores, it does not re-run the gates, so
    // inventing fresh numbers here would report a measurement never taken.
    let carried_reading = |arg: &str| {
        carried_arg(keys::FIT_NOTE_CAST_ADMITTED, arg).and_then(|v| v.parse::<f32>().ok())
    };
    // The two ABSTAINING readings ride their own notes, so their ABSENCE is
    // carried too: a missing arg re-renders as the not-measurable sentence,
    // never as 0.000. The head's three are all-or-nothing — without them
    // there is no admission to re-report, and defaulting any of them would
    // be the same invention on the rescore side.
    let carried_cast_admitted = match (
        carried_reading("ratio"),
        carried_reading("bound"),
        carried_reading("rehued"),
    ) {
        (Some(ratio), Some(bound), Some(rehued)) => Some(CastReadings {
            ratio,
            bound,
            foreign: carried_arg(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN, "foreign")
                .and_then(|value| value.parse::<f32>().ok()),
            rehued,
            fan: carried_arg(keys::FIT_NOTE_CAST_ADMITTED_FAN, "fan")
                .and_then(|value| value.parse::<f32>().ok()),
        }),
        _ => None,
    };
    // The PROJECTION rides its notes the same way. `limit` and `target` are
    // regenerated from the constants rather than carried: they are what the
    // code believes NOW, and a rescore that re-rendered under a retuned gate
    // must not quote the old one as if it had just measured it.
    let carried_projected = (|| {
        let head = |arg: &str| {
            carried_arg(keys::FIT_NOTE_CAST_PROJECTED, arg).and_then(|v| v.parse::<f32>().ok())
        };
        Some(CastProjection {
            share: head("share")?,
            fan_before: head("fan_before")?,
            t: head("t")?,
            fan_after: carried_arg(keys::FIT_NOTE_CAST_PROJECTED_FAN, "fan_after")
                .and_then(|v| v.parse::<f32>().ok()),
            ratio: head("ratio")?,
            bound: head("bound")?,
            rehued: head("rehued")?,
            // The foreign clause is the ADMISSION's key on both paths, so it
            // rides back from the same place `carried_cast_admitted` reads it
            // — and its ABSENCE rides too, re-rendering as the
            // not-measurable sentence and never as a 0.000 nobody measured.
            foreign: carried_arg(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN, "foreign")
                .and_then(|value| value.parse::<f32>().ok()),
        })
    })();
    // The fan refusal's readings ride the same way; without them the rescore
    // would re-emit the note with zeroes.
    let carried_fan = carried_arg(keys::FIT_NOTE_CAST_HUE_FANNED, "share")
        .and_then(|share| share.parse::<f32>().ok())
        .zip(carried_arg(keys::FIT_NOTE_CAST_HUE_FANNED, "fan").and_then(|fan| fan.parse::<f32>().ok()));
    let divergence = structure_divergence_for(src, target, &EditRecipe::default(), None);
    let mode = if divergence.d >= DIVERGENCE_GLOBAL || carried(keys::FIT_SUMMARY_ATMOSPHERE) {
        FitMode::Atmosphere
    } else {
        FitMode::Full
    };
    let blind = (mode == FitMode::Atmosphere).then(|| structural.structure_blind(&tp));
    let evidence = blind.as_ref().unwrap_or(&structural);
    let err_before = if mode == FitMode::Atmosphere {
        look_err_with_evidence(&base_px, &tp, evidence)
    } else {
        err_before
    };
    let joint_after = crate::fit_zoned::joint_reading_with_evidence(
        &after_px,
        &tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    // The strength budget rides EVERY rescoring, not only an Atmosphere one.
    // At or below default its vetoes are withheld and nothing below fires, so
    // the shipped path is untouched. From 0.85 the solve DISCLOSED unsupported
    // movement and capped its claim; a rescoring re-derives that disclosure
    // from the recipe it describes (the deep step moved the dial) — never
    // cloned off the solve, and never dropped: dropping it let the rescored
    // report claim the uncapped ladder for the same movement. The paired
    // solve's evacuation voucher is not available here, so the strict doctrine
    // applies: a disclosure this rescoring adds can only lower the claim.
    let budget = FitBudget::for_strength(carried_strength);
    let disclose = budget.vetoes == VetoPolicy::Disclose;
    let veto_luma = disclose
        .then(|| moved_unsupported_luma_range_names(&base_px, &after_px, evidence))
        .flatten();
    let veto_hue = disclose
        .then(|| {
            // A measured global cast is a consistent rotation, not an
            // unsupported one — the same exemption the Atmosphere solve makes.
            if mode == FitMode::Atmosphere && structural.global_cast.is_some() {
                None
            } else {
                moved_unsupported_hue_range_names(&base_px, &after_px, evidence)
            }
        })
        .flatten();
    compose_report(
        recipe.clone(),
        Measured {
            err_before,
            err_after: look_err_with_evidence(&after_px, &tp, evidence),
            joint_after,
            after_px: &after_px,
            tp: &tp,
            same_frame,
            mode,
            divergence,
            evidence,
            structural_evidence: blind.as_ref().map(|_| &structural),
            defer_disclosure: false,
        },
        SolveFacts {
            budget: Some(budget),
            strength: carried(keys::FIT_NOTE_STRENGTH).then_some(carried_strength.get()),
            veto_luma,
            veto_hue,
            wb_clamped: None,
            wb_search_bound: None,
            wb_rotation_coverage: None,
            wb_rotation_disclosure: None,
            wb_foreign_hue_withheld: carried(keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE),
            wb_rotation_withheld: carried(keys::FIT_NOTE_WB_WITHHELD_ROTATION),
            sat_pegged: if carried(keys::FIT_NOTE_ATMOSPHERE_SAT_PEGGED) {
                Some(FitMode::Atmosphere)
            } else if carried(keys::FIT_NOTE_SAT_PEGGED) {
                Some(FitMode::Full)
            } else {
                None
            },
            cast: CastOutcome {
                rehue_blocked: carried(keys::FIT_NOTE_REHUE_BLOCKED),
                ratio_rejected: carried(keys::FIT_NOTE_CAST_REJECTED),
                hue_fanned: carried_fan,
                // The gates are not re-run here — a rescore re-renders and
                // re-scores, so the admission readings arrive through the
                // `cast_admitted` field beside this one, off the carried note.
                readings: None,
                // …and the projection likewise, through `cast_projected`.
                projected: None,
            },
            cast_admitted_by_strength: carried_cast_admission,
            cast_admitted: carried_cast_admitted,
            cast_projected: carried_projected,
            evidence_refused: carried(keys::FIT_NOTE_EVIDENCE_WITHHELD),
            // Dropped on purpose — see the doc above. Naming them here rather
            // than omitting them silently is the point: the abstention has to
            // be visible at the place that makes it.
            sat_fitted: None,
            regressed: None,
            detail: (recipe.clarity, recipe.texture),
            detail_withheld: false,
            robust: None,
            paired: carried(keys::FIT_SUMMARY_WITH_CURVE_PAIRED)
                || carried(keys::FIT_SUMMARY_NO_CURVE_PAIRED),
            vouched_bands: carried_arg(keys::FIT_NOTE_VOUCHED_CONVERGENCE, "bands"),
            // The mixer's evidence verdicts cross over for the same reason
            // the cast gates do: the deep step moves global dials, it never
            // re-runs the per-band population gate. What the mixer MOVED is
            // deliberately NOT carried — `compose_report` reads that straight
            // off the recipe in front of it, which here is the adjusted one.
            hsl: HslStageFacts {
                refused: carried_arg(keys::FIT_NOTE_HSL_BANDS, "refused")
                    .filter(|refused| refused != "none")
                    .unwrap_or_default(),
                withdrawn: if carried(keys::FIT_NOTE_HSL_WITHDRAWN_BLIND) {
                    Some(HslWithdrawal::Blind)
                } else if carried(keys::FIT_NOTE_HSL_WITHDRAWN_ERROR) {
                    Some(HslWithdrawal::Error)
                } else {
                    None
                },
            },
            // R30 R2: WholeFrame on purpose, and NOT carried from the notes.
            // `rescore_report` re-measures a recipe someone ADJUSTED after
            // the solve, and which population that solve read its two robust
            // controls over is exactly the kind of fact this split says a
            // later re-measurement cannot recover. Re-asserting it from a
            // sentence would be claiming a provenance nothing here checked.
            atmosphere_reference: AtmosphereReference::WholeFrame,
        },
    )
}

/// Every reading the colour stage's four gates took, kept whatever the
/// verdict was — so an ADMITTED cast can say WHY it was admitted with the
/// same numbers a refusal would have quoted. Before v1.2.3 admission was
/// silent unless the strength budget bought it, so the commonest outcome of
/// the stage (curves shipped) reached the user with no reading at all.
///
/// Two of the four are `Option` and stay `Option` all the way into the note.
/// A gate that ABSTAINS has not measured zero, and collapsing the abstention
/// to `0.0` published a number nobody took: on a target with too little
/// chromatic mass the admission note read "created 0.000 of the frame in
/// hues the target does not contain" out of a census that never ran.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct CastReadings {
    /// Weighted look error WITH the curves over the error without them.
    ratio: f32,
    /// The bound that ratio was judged against on THIS path —
    /// `budget.cast_ratio`, which the strength budget widens (2.0 at the
    /// shipped default, up to 3.0), not the [`CAST_ACCEPT_RATIO`] anchor.
    bound: f32,
    /// Foreign-hue population share the curves CREATE (with − without).
    /// `None` = not measurable: the target carries no hue evidence for a hue
    /// to be foreign TO.
    foreign: Option<f32>,
    /// Weighted share of the population re-hued past [`ROT_DEG`].
    rehued: f32,
    /// Hue spread the curves ADD inside the widest hue class, in degrees.
    /// SIGNED — the curves can also NARROW a class. `None` = not measurable:
    /// no hue class holds [`FAN_SHARE`] of the census population across two
    /// populated luma slices.
    fan: Option<f32>,
}

/// The notes an ADMITTED cast writes: the head, which carries the three
/// readings that are always measured, then one clause for each reading that
/// can ABSTAIN.
///
/// It is a named function and not three inline `push_note`s so the
/// abstention wording is reachable from a test without needing a pair that
/// happens to abstain — the case that shipped the fabricated `0.000` is by
/// construction the rare one.
fn cast_admission_notes(r: CastReadings) -> Vec<crate::rationale::Note> {
    use crate::rationale::{keys, Note};
    vec![
        Note::new(
            keys::FIT_NOTE_CAST_ADMITTED,
            vec![
                ("ratio", format!("{:.3}", r.ratio)),
                ("bound", format!("{:.3}", r.bound)),
                ("rehued", format!("{:.3}", r.rehued)),
            ],
        ),
        match r.foreign {
            Some(share) => Note::new(
                keys::FIT_NOTE_CAST_ADMITTED_FOREIGN,
                vec![("foreign", format!("{share:.3}"))],
            ),
            // NOT `0.000`: the target carried no hue evidence, so no census
            // ran and there is no share to report.
            None => Note::plain(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA),
        },
        match r.fan {
            // SIGNED. The curves can NARROW a class's spread across
            // luminance, and "opened a −3 degree hue fan" reported that as
            // an opening.
            Some(degrees) => Note::new(
                keys::FIT_NOTE_CAST_ADMITTED_FAN,
                vec![
                    // ONE decimal. At `{:+.0}` the admitted haze pair's 14.6
                    // printed as "+15 degrees, against a limit of 15" — a
                    // sentence that states its own violation while the
                    // reading it renders had in fact passed.
                    ("fan", format!("{degrees:+.1}")),
                    ("limit", format!("{FAN_DEG:.0}")),
                ],
            ),
            None => Note::plain(keys::FIT_NOTE_CAST_ADMITTED_FAN_NA),
        },
    ]
}

/// A cast the hue-fan gate convicted AS FITTED and the projection recovered:
/// what the fitted curves would have done, how far they were shrunk, and what
/// the milder curves actually read.
///
/// A projected cast writes [`cast_projection_notes`] INSTEAD of
/// [`cast_admission_notes`] — one sentence per outcome, and "admitted" would
/// describe curves the fit never shipped. The two are exclusive at the fact
/// site (`SolveFacts::cast_projected` vs `cast_admitted`), not by convention
/// at the two push sites.
#[derive(Clone, Copy, PartialEq, Debug)]
struct CastProjection {
    /// Share of the census population held by the class the FITTED curves
    /// would have fanned. A hue population, not a region — see
    /// [`hue_fan_weighted`].
    share: f32,
    /// Added spread, in degrees, the FITTED curves would have opened in it.
    fan_before: f32,
    /// Where on the projection path the shipped curves sit: 1.0 would be
    /// the fitted cast, 0.5 one curve shared by all three channels, 0.0 no
    /// curves at all. See [`projected_cast_curves`].
    t: f32,
    /// Added spread the PROJECTED curves open, measured on their own render.
    /// SIGNED, and `None` = not measurable — the same abstention
    /// [`CastReadings::fan`] carries, for the same reason, and never `0.0`.
    fan_after: Option<f32>,
    /// The PROJECTED candidate's look-error ratio and the bound it was judged
    /// against. The projected curves are what ships, so these are its
    /// readings, not the fitted cast's.
    ratio: f32,
    bound: f32,
    /// The two PIXEL-ALIGNED readings, carried for the same reason the
    /// admission carries them and with more force: these are curves the fit
    /// INVENTED to answer a conviction rather than curves it measured off the
    /// pair, so the readings that say what they did to the frame matter more
    /// here, not less. Same meanings and the same abstention as
    /// [`CastReadings::rehued`] / [`CastReadings::foreign`], measured on the
    /// PROJECTED candidate's own render.
    rehued: f32,
    foreign: Option<f32>,
}

/// What a PROJECTED cast tells the user: the head, carrying the conviction
/// that triggered the projection, the terms of the shrink and the re-hued
/// share; then the foreign-hue share and the projected fan — the two readings
/// that can ABSTAIN, each with the same measured / not-measurable pair of keys
/// the admission's get.
///
/// A projected cast discloses AT LEAST what an admitted one does, and the
/// asymmetry runs the opposite way from what "it was convicted once already"
/// suggests: these are curves the fit INVENTED to answer a conviction, not
/// curves it measured off the pair, so the two pixel-aligned readings are the
/// ones a reader most needs. The foreign clause is the ADMISSION's own key
/// rather than a copy of it — one sentence, one translation, and its subject
/// ("They", the curves) reads correctly after either head.
fn cast_projection_notes(p: CastProjection) -> Vec<crate::rationale::Note> {
    use crate::rationale::{keys, Note};
    vec![
        Note::new(
            keys::FIT_NOTE_CAST_PROJECTED,
            vec![
                ("fan_before", format!("{:.1}", p.fan_before)),
                ("share", format!("{:.3}", p.share)),
                ("limit", format!("{FAN_DEG:.0}")),
                ("t", format!("{:.3}", p.t)),
                ("ratio", format!("{:.3}", p.ratio)),
                ("bound", format!("{:.3}", p.bound)),
                ("rehued", format!("{:.3}", p.rehued)),
            ],
        ),
        match p.foreign {
            Some(share) => Note::new(
                keys::FIT_NOTE_CAST_ADMITTED_FOREIGN,
                vec![("foreign", format!("{share:.3}"))],
            ),
            // NOT `0.000`: the target carried no hue evidence, so no census
            // ran and there is no share to report.
            None => Note::plain(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA),
        },
        match p.fan_after {
            // SIGNED for the same reason the admission's is: the projected
            // curves can leave the class NARROWER than they found it. ONE
            // decimal, because the target it is printed against carries one:
            // at `{:+.0}` a reading of exactly 7.5 — which CLEARS, the test
            // being `<=` — rendered as "+8 degrees, inside the 7.5 degree
            // target", a sentence that contradicts itself.
            Some(degrees) => Note::new(
                keys::FIT_NOTE_CAST_PROJECTED_FAN,
                vec![
                    ("fan_after", format!("{degrees:+.1}")),
                    ("target", format!("{FAN_PROJECT_DEG:.1}")),
                ],
            ),
            None => Note::plain(keys::FIT_NOTE_CAST_PROJECTED_FAN_NA),
        },
    ]
}

/// Which of the hue/ratio gates (if any) refused the colour stage — they
/// used to collapse into one boolean, and only one of them had a note.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct CastOutcome {
    /// A pixel-aligned hue gate fired (foreign hues, or a region re-hued).
    rehue_blocked: bool,
    /// The aggregate ratio refused: the curves did not buy enough.
    ratio_rejected: bool,
    /// The fan gate fired: (class share, added spread in degrees). Carries
    /// its readings because the refusal DISCLOSES them.
    hue_fanned: Option<(f32, f32)>,
    /// What the gates measured, present whenever curves were actually
    /// fitted and judged. `None` = the stage produced no curves to judge.
    readings: Option<CastReadings>,
    /// v1.2.3: the fan gate convicted the FITTED curves and the projection
    /// found a milder cast that clears [`FAN_PROJECT_DEG`] and all four
    /// gates. Then `hue_fanned` is `None` and `readings` describe the
    /// PROJECTED candidate — the one that ships — while this carries what
    /// the fitted curves would have done and how far they were shrunk.
    projected: Option<CastProjection>,
}

#[cfg(test)]
fn cast_gate_outcome(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<(&[f32], &[[f32; 3]])>,
) -> CastOutcome {
    cast_gate_outcome_with_ratio(cur, with_px, tp, evidence, vouch, CAST_ACCEPT_RATIO)
}

fn carried_strength_from_notes(prior: &[crate::rationale::Note]) -> crate::recipe::GradeStrength {
    let arg = |name: &str| {
        prior
            .iter()
            .find(|note| note.key == crate::rationale::keys::FIT_NOTE_STRENGTH)
            .and_then(|note| note.args.iter().find(|(key, _)| *key == name))
            .and_then(|(_, value)| value.parse::<f32>().ok())
    };
    arg("s")
        .or_else(|| arg("pct").map(|pct| pct / 100.0))
        .map(crate::recipe::GradeStrength::new)
        .unwrap_or_default()
}

fn cast_gate_outcome_with_ratio(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<(&[f32], &[[f32; 3]])>,
    accept_ratio: f32,
) -> CastOutcome {
    let err_without = look_err_with_evidence(cur, tp, evidence);
    let err_with = look_err_with_evidence(with_px, tp, evidence);
    // The fan gate is the FOURTH gate (v1.2.3) and, like the other two hue
    // gates, only ever rejects. It is measured unconditionally so an
    // ADMITTED cast can disclose the reading it passed on.
    let fan = hue_fan_weighted(cur, with_px, evidence);
    CastOutcome {
        ratio_rejected: err_without > 0.0
            && err_with > err_without * accept_ratio
            && evidence.identifiability < 0.25,
        rehue_blocked: cast_paints_foreign_hues_weighted(cur, with_px, tp, evidence)
            || cast_rotates_a_region_weighted(cur, with_px, evidence)
            || moved_unsupported_hue_range_names_vouched(cur, with_px, evidence, vouch)
                .is_some(),
        hue_fanned: fan
            .filter(|&(_, added, _)| added >= FAN_DEG)
            .map(|(share, added, _)| (share, added)),
        readings: Some(CastReadings {
            ratio: err_with / err_without.max(1e-6),
            bound: accept_ratio,
            // Both of these stay Option: `.unwrap_or(0.0)` here is what
            // turned "this gate did not run" into a published measurement.
            foreign: foreign_created_share_weighted(cur, with_px, tp, evidence),
            rehued: rehued_share_weighted(cur, with_px, evidence),
            fan: fan.map(|(_, added, _)| added),
        }),
        // The gate does not project — `search_cast_projection` does, and it
        // fills this in on the candidate it chose. A gate reading on its own
        // is always an unprojected one.
        projected: None,
    }
}

/// Did the finished recipe do HARM — and which check says so?
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct TerminalHarm {
    /// The frame-global look error regressed past the fit's own quantisation
    /// budget (R16's rule, unchanged).
    scalar: bool,
    /// The joint value-range distributions drifted apart past
    /// [`crate::fit_zoned::JOINT_DRIFT_TOL`] (R23-6's additional veto).
    joint: bool,
}

impl TerminalHarm {
    fn any(self) -> bool {
        self.scalar || self.joint
    }
}

/// Tolerance for the fit's OWN quantisation, not a fixed error size. The
/// rounded sliders, the 8-bit residual tone curve and the f32 develop round
/// trip cost about 1e-3 of residual even on an IDENTICAL pair, where
/// `err_before` is exactly 0 — so a bare +1e-4 margin fired there, wiping a
/// perfectly good near-neutral solve and reporting "outside the global
/// model's reach" directly beneath a printed residual of 0.000 -> 0.000.
///
/// A flat FLOOR is the wrong correction though: it would also wave through a
/// fit that is genuinely worse whenever both numbers are small (err_before
/// 0.0010 -> err_after 0.0029 is nearly 3× worse, and no absolute floor below
/// 0.003 catches it). Scale with the error instead and add the quantisation
/// budget once.
const FIT_QUANT: f32 = 1.8e-3;

/// The TERMINAL do-no-harm decision, pure so both of its arms are testable
/// without a fixture that can reach them end to end.
///
/// Two independent readings, OR-ed, because they see different damage. The
/// scalar arm is R16's and unchanged. The joint arm (R23-6) is the
/// ADDITIONAL veto in [`crate::fit_zoned::ZONE_GLOBAL_REGRESSION_TOL`]'s
/// shape: a fit that leaves the value ranges further apart than doing
/// nothing has done harm whatever the scalar says — and the scalar
/// structurally cannot say it, its colour term being three unconditional
/// channel means. It only ever REJECTS, never rescues (a joint reading that
/// improves cannot save a recipe the scalar convicts), and it is FAIL-OPEN:
/// either side missing means no opinion, never "no problem".
fn terminal_harm(
    err_before: f32,
    err_after: f32,
    joint_base: Option<crate::fit_zoned::JointReading>,
    joint_after: Option<crate::fit_zoned::JointReading>,
) -> TerminalHarm {
    TerminalHarm {
        scalar: err_after > err_before + 1e-6,
        joint: match (joint_base, joint_after) {
            (Some(b), Some(a)) => {
                a.weighted > b.weighted + crate::fit_zoned::JOINT_DRIFT_TOL
            }
            _ => false,
        },
    }
}

impl CastOutcome {
    /// What the user is told about an EMPTY colour stage — pure, so the
    /// silent arm is testable without a fixture that reaches it.
    ///
    /// R23-6 A-2: `ratio_rejected` used to produce no note at all, so "the
    /// colour stage produced nothing" — the commonest outcome of the whole
    /// stage — reached the user as an unexplained absence, while the hue
    /// gates next to it did disclose. The hue note WINS a double rejection:
    /// it is the more specific statement, and a fit that would have re-hued
    /// a region is the thing worth saying.
    /// v1.2.3: the fan gate sits BETWEEN them. It is more specific than
    /// "did not buy enough" and less specific than the pixel-aligned
    /// verdict, and the order matters for more than prose — a pair the
    /// pixel gates already refuse must keep reporting exactly what it
    /// reported before this gate existed, or every recipe those gates
    /// govern changes bytes for a reason the user cannot see.
    fn note(self) -> Option<crate::rationale::Note> {
        use crate::rationale::{keys, Note};
        if self.rehue_blocked {
            Some(Note::plain(keys::FIT_NOTE_REHUE_BLOCKED))
        } else if let Some((share, degrees)) = self.hue_fanned {
            Some(Note::new(
                keys::FIT_NOTE_CAST_HUE_FANNED,
                vec![
                    ("share", format!("{share:.3}")),
                    // …and the refusal's reading for the same reason: at
                    // `{:.0}` a convicting 15.4 printed as "15 degrees apart
                    // (limit 15)", which reads as a reading that passed.
                    ("fan", format!("{degrees:.1}")),
                    ("limit", format!("{FAN_DEG:.0}")),
                ],
            ))
        } else if self.ratio_rejected {
            Some(Note::plain(keys::FIT_NOTE_CAST_REJECTED))
        } else {
            None
        }
    }

    /// Did ANY gate refuse? One place, so the stage that empties the curves
    /// and the report that explains the emptiness can never disagree.
    fn refused(self) -> bool {
        self.rehue_blocked || self.ratio_rejected || self.hue_fanned.is_some()
    }

    /// v1.2.3 — may this verdict be RESCUED by the projection, and with which
    /// conviction? `Some((share, fan))` exactly when the fan gate is the ONLY
    /// gate that convicted; `None` means "refuse as before, unprojected".
    ///
    /// The fan verdict is the only one shaped like "not in this SHAPE", which
    /// is the objection shrinking the three curves toward the shape they
    /// share actually answers. The other two are not shape complaints and the
    /// projection is not their answer:
    ///
    ///   * a PIXEL-ALIGNED veto says the destination is wrong, and no point
    ///     on the path makes a wrong destination right — that is also the
    ///     whole of the viaduct pair's byte-identity;
    ///   * the RATIO gate says the curves did not buy enough of the frame to
    ///     be worth their regional risk, and a WEAKER version of curves that
    ///     did not pay cannot be the answer to that. It would also ship a
    ///     sentence that omits a verdict: `FIT_NOTE_CAST_PROJECTED` names the
    ///     fan and only the fan, so a cast rescued over a ratio conviction
    ///     would disclose one of the two gates it had to survive.
    ///
    /// A pair the ratio gate convicted therefore stays refused and keeps the
    /// note it already had (the fan note — see [`CastOutcome::note`], where
    /// the hue verdict wins a double rejection because it is the more
    /// specific statement). Pinned by
    /// `a_cast_the_ratio_gate_convicts_is_not_rescued_by_the_projection`.
    fn earns_projection(self) -> Option<(f32, f32)> {
        if self.rehue_blocked || self.ratio_rejected {
            return None;
        }
        self.hue_fanned
    }
}

/// Name the develop controls THIS pair's residual points at that the fit has
/// no way to solve for (R23-6 A-5).
///
/// The summary note already says "local masks and per-band hue rotation are
/// not recovered" on every fit ever produced, which is true and useless: it
/// does not say whether THIS target needed them. The solve domain is a fact
/// about the code — the global arm writes exposure/contrast/highlights/
/// shadows/whites/blacks, a tone curve, one saturation, the per-band mixer's
/// saturation/luminance axes and three channel curves, and NOTHING in
/// `advisor::catalogue::RECIPE_CONTROLS` else — so the honest disclosure is
/// the intersection of "the model can express it", "we never solve it" and
/// "the residual has evidence pointing at it".
///
/// The evidence tests are deliberately coarse and stated as SUSPICION, never
/// as measurement: the residual decomposition can say a gap is chromatic
/// rather than tonal, and it cannot say which control would close it. Naming
/// a control the residual gives no sign of would be inventing a diagnosis.
///
/// Does what is LEFT look like a PER-BAND COLOUR job?
///
/// Named and shared rather than left inline because stage 4a is now held to
/// it from the other side: once the mixer has closed a band's colour gap this
/// predicate must stop saying yes, and [`unrepresented_note`] must stop
/// naming `hsl`. One derivation, two consumers — a second copy in the test
/// would be a claim about a claim.
///
/// Route one asks whether the residual is a colour difference CONDITIONED ON
/// brightness — exactly the question the joint family answers, and exactly
/// the shape `hsl` / `color_grade` have. Reading the CHROMATIC buckets
/// against the NEUTRAL ones at the same brightness separates "the coloured
/// pixels disagree" (a colour move) from "everything disagrees" (a tone or
/// exposure gap the fit does solve for) — a distinction no single global
/// statistic can make, which is why route two cannot carry this on its own:
/// a target that moves a whole region to a hue the source has NOWHERE leaves
/// both bands under the 1.5% two-sided weight gate and is invisible to it
/// (the cross-band blindness `look_err`'s own hue term documents).
///
/// Route two is the classic evidence for the same conclusion: a populated
/// band whose centroid hue is far off. Kept as a SECOND route because it
/// fires where the residual is a rotation rather than a magnitude — the axis
/// stage 4a deliberately never solves — and the two routes miss different
/// things.
fn residual_is_colour_shaped(
    after_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    let buckets = crate::fit_zoned::joint_buckets_with_evidence(
        after_px,
        tp,
        Some(&evidence.source_weights),
        Some(&evidence.target_weights),
    );
    let worst_of = |chromatic: bool| -> f32 {
        buckets
            .iter()
            .filter(|b| b.chromatic == chromatic)
            .map(|b| b.err)
            .fold(0.0f32, f32::max)
    };
    let (chromatic_worst, neutral_worst) = (worst_of(true), worst_of(false));
    if chromatic_worst >= UNREPRESENTED_CHROMATIC_ERR
        && chromatic_worst >= neutral_worst + UNREPRESENTED_CHROMATIC_LEAD
    {
        return true;
    }
    let (sa, ta) = band_stats_weighted(after_px, &evidence.source_hue_weights);
    let (sb, tb) = band_stats_weighted(tp, &evidence.target_hue_weights);
    let mut worst_band = 0.0f32;
    if ta >= 1.0 && tb >= 1.0 {
        for i in 0..8 {
            let (x, y) = (&sa[i], &sb[i]);
            if evidence.hue.get(i).map(|r| r.weight).unwrap_or(0.0) <= 0.0
                || x.w / ta < EVIDENCE_MIN_SHARE as f64
                || y.w / tb < EVIDENCE_MIN_SHARE as f64
            {
                continue;
            }
            let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
            while d > 180.0 {
                d -= 360.0;
            }
            while d < -180.0 {
                d += 360.0;
            }
            worst_band = worst_band.max(d.abs() as f32);
        }
    }
    worst_band >= UNREPRESENTED_HUE_DEG
}

/// `after_px` is the FINISHED render — the residual is what the fit could
/// not close, so the evidence has to be read there and not on the base.
fn unrepresented_note(
    recipe: &EditRecipe,
    after_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    err_after: f32,
    mode: FitMode,
    evidence: &EvidenceModel,
) -> Option<crate::rationale::Note> {
    // Nothing left to explain.
    if err_after <= FIT_QUANT_CLEAN {
        return None;
    }
    let mut names: Vec<&str> = Vec::new();

    if residual_is_colour_shaped(after_px, tp, evidence) {
        // `hsl` here means the axis stage 4a does NOT solve: it fits a
        // band's saturation and luminance from that band's own population,
        // and this note is read on the residual those moves left behind, so
        // a per-band gap the mixer closed never reaches this line. What
        // survives it is a per-band HUE rotation (the one axis the solver
        // bans outright) or a demand the mixer's evidence gate refused.
        // `color_grade` is the tone-conditioned version of the same move.
        // Name the second only when the channel curves — our one lever with
        // that shape — are absent, which is both the honest condition and the
        // common one (they are refused by the four gates far more often than
        // they are kept).
        names.push("hsl");
        if recipe.red_curve.is_empty()
            && recipe.green_curve.is_empty()
            && recipe.blue_curve.is_empty()
        {
            names.push("color_grade");
        }
    }
    // A surviving UNIFORM channel-mean offset is the white-balance shape.
    // Full mode never assigns temperature/tint; Atmosphere mode names them
    // only when its bounded WB solve declined the demand.
    let mean = |px: &[[f32; 3]], ch: usize| -> f32 {
        if px.is_empty() {
            0.0
        } else {
            px.iter().map(|p| p[ch]).sum::<f32>() / px.len() as f32
        }
    };
    let rb = (mean(after_px, 0) - mean(tp, 0)) - (mean(after_px, 2) - mean(tp, 2));
    if rb.abs() >= UNREPRESENTED_WB_RB && recipe.temperature_k.is_none() {
        names.push("temperature_k/tint");
    }
    if !recipe.masks.is_empty() {
        names.push("local masks");
    }
    if names.is_empty() {
        return None;
    }
    Some(crate::rationale::Note::new(
        if mode == FitMode::Atmosphere {
            crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
        } else {
            crate::rationale::keys::FIT_NOTE_UNREPRESENTED
        },
        vec![("controls", names.join(", "))],
    ))
}

/// Below this residual there is nothing to explain and the disclosure would
/// be noise — the same order as the fit's own quantisation budget.
const FIT_QUANT_CLEAN: f32 = 0.025;
/// Worst populated-band centroid disagreement (degrees) that counts as
/// "this target used a per-band colour move". 20° is well past the ±13.5°
/// the engine's own HSL hue axis can even express, so a gap this size cannot
/// be a rounding artefact of a band the fit did reach.
const UNREPRESENTED_HUE_DEG: f32 = 20.0;
/// A chromatic bucket must miss by at least this much before the residual
/// is called colour-shaped. On the fixture set the fits that LAND leave
/// every chromatic bucket under 0.05 (the haze pair's worst is 0.041), while
/// the region-graded canyon leaves 0.098 and the unreachable repaint 0.71.
const UNREPRESENTED_CHROMATIC_ERR: f32 = 0.06;
/// …and it must miss by this much MORE than the neutral buckets at the same
/// brightness, or the difference is a tone/exposure gap the solver does
/// address rather than a colour one it cannot.
const UNREPRESENTED_CHROMATIC_LEAD: f32 = 0.02;
/// Red-minus-blue mean offset (in 0..1 channel units) that counts as a
/// white-balance-shaped residual. 0.02 ≈ 5/255 across the whole frame —
/// visible as a cast, and an order above the fit's own rounding.
const UNREPRESENTED_WB_RB: f32 = 0.02;

// --------------------------------------------------------------------------
// tone solve
// --------------------------------------------------------------------------

/// Robust conditional tone observations from the same evidence pixels on
/// both sides. Unlike marginal CDF pairing, these points retain the question
/// "what target value did this supported source range become?".
/// One robust paired-regression estimate of the tone map, shared by the
/// global and the zoned tone stages (one estimator, two call sites — the
/// solver family must not fork).
///
/// Identification: samples are PAIRED at equal raster index, so this is only
/// called on same-frame, same-grid pairs (the caller gates on that). The map
/// is estimated per luma bin as a Tukey-biweight IRLS MEAN (median start), so
/// a content-divergent sub-population — invented clouds, a moved subject —
/// loses weight by the estimator's own influence function instead of by a
/// hand-set mask; a plain least-squares mean would be dragged.
///
/// The returned per-pixel weights extend the verdict to the CHROMATIC
/// population (its own robust scale — a legitimate global colour edit
/// inflates every chromatic residual uniformly and must not mass-reject),
/// so the saturation and cast stages can compose them with the evidence
/// weights: evidence answers "is this pixel measurable at all", the robust
/// weight answers "is this pixel consistent with one global develop".
pub(crate) struct PairedRobustTone {
    /// Monotone map estimate: (weighted-mean x, robust y) per populated bin.
    pub points: Vec<(f32, f32)>,
    /// Evidence×robust mass behind each point — the model-selection score
    /// weights a point by the pixels that actually testify there.
    pub masses: Vec<f32>,
    /// Per-pixel robust weight on the shared raster (1.0 where not sampled).
    pub weights: Vec<f32>,
    /// Evidence-weighted share of sampled pixels with robust weight < 0.5.
    pub rejected_share: f32,
    /// Evidence luma-range labels holding at least 10% of the rejected mass.
    pub rejected_ranges: String,
}

const ROBUST_TUKEY_C: f32 = 4.685;
const ROBUST_SCALE_FLOOR: f32 = 2.0 / 255.0;
const ROBUST_IRLS_ROUNDS: usize = 3;
/// Below this rejected share the disclosure stays silent — JPEG noise alone
/// rejects a stray pixel or two and a note for that would be crying wolf.
pub(crate) const ROBUST_REJECT_DISCLOSE_MIN: f32 = 0.02;
/// A chromatic pair is vouched only while its hue movement stays within this
/// many degrees of the class's dominant direction. One global develop moves
/// hues COHERENTLY (a cast is a smooth per-channel map, at most a few tens
/// of degrees, one way), so the residual-magnitude Tukey alone is blind to a
/// content flip that hides under a frame-wide recolour's inflated scale
/// (measured: the golden-sky fixture's 171° sky flip rode a warm rock
/// grade's scale and every pixel came back vouched). Casts stay comfortably
/// inside 60°; content flips live far outside it.
const HUE_VOUCH_COHERENCE_DEG: f32 = 60.0;
/// Enough testimony to trust a marginal map value in a luma range: an
/// absolute pixel count, deliberately NOT a frame share (1.4% of a 384-edge
/// frame is ~350 measured pixels — plenty; the share form of this gate
/// silenced whole regions). 32 matches the robust estimator's own sample
/// floor.
pub(crate) const SUPPORT_MIN_PIXELS: f32 = 32.0;

fn weighted_median_of(mut pairs: Vec<(f32, f32)>) -> f32 {
    // (value, weight); callers guarantee non-empty with positive total weight.
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = pairs.iter().map(|v| v.1).sum();
    let mut acc = 0.0;
    for &(value, weight) in &pairs {
        acc += weight;
        if acc >= 0.5 * total {
            return value;
        }
    }
    pairs.last().map(|v| v.0).unwrap_or(0.0)
}

/// A vouched pixel's move counts as CONVERGENCE when it lands strictly
/// closer to its own paired target — the shared predicate behind every
/// hue-damage guard's paired exemption (one definition, rule 09).
fn converges_toward(target: &[f32; 3], before: &[f32; 3], after: &[f32; 3]) -> bool {
    let dist =
        |p: &[f32; 3]| (0..3).map(|c| (p[c] - target[c]).abs()).fold(0.0f32, f32::max);
    dist(after) + 1e-3 < dist(before)
}

fn tukey_weight(residual: f32, scale: f32) -> f32 {
    let u = residual / (ROBUST_TUKEY_C * scale);
    if u.abs() >= 1.0 { 0.0 } else { (1.0 - u * u) * (1.0 - u * u) }
}

pub(crate) fn paired_robust_tone(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    pair_weight: &dyn Fn(usize) -> f32,
    neutral_gated: bool,
) -> Option<PairedRobustTone> {
    // 64 bins: finer piecewise-linear resolution against smooth engine
    // curves at negligible cost (the haze fixture moved 0.0228 -> 0.0225 end
    // error on this alone — small, kept because the 8-member bin floor below
    // already keeps sparse bins out, so extra resolution costs nothing).
    const BINS: usize = 64;
    let n = sp.len().min(tp.len());
    // (x, y, evidence weight, raster index) for the tone samples.
    let mut samples: Vec<(f32, f32, f32, usize)> = Vec::new();
    for i in 0..n {
        let (s, t) = (&sp[i], &tp[i]);
        if neutral_gated && (!is_neutralish(s) || !is_neutralish(t)) {
            continue;
        }
        let w = pair_weight(i);
        if w <= 0.0 {
            continue;
        }
        samples.push((luma601(s), luma601(t), w, i));
    }
    if samples.len() < 32 {
        return None;
    }
    let mut robust = vec![1.0f32; samples.len()];
    let mut points: Vec<(f32, f32)> = Vec::new();
    let mut masses: Vec<f32> = Vec::new();
    for round in 0..=ROBUST_IRLS_ROUNDS {
        // Map estimate under the current weights: per-bin Tukey-weighted mean
        // (round 0 starts from the weighted MEDIAN — the influence function
        // needs a resistant start or the first residuals are already dragged).
        let mut bins: Vec<Vec<usize>> = vec![Vec::new(); BINS];
        for (k, &(x, ..)) in samples.iter().enumerate() {
            bins[((x * BINS as f32).floor() as usize).min(BINS - 1)].push(k);
        }
        points.clear();
        masses.clear();
        for members in &bins {
            let total: f32 = members.iter().map(|&k| samples[k].2 * robust[k]).sum();
            if members.len() < 8 || total <= 1e-4 {
                continue;
            }
            masses.push(total);
            let y = if round == 0 {
                weighted_median_of(
                    members.iter().map(|&k| (samples[k].1, samples[k].2)).collect(),
                )
            } else {
                members.iter().map(|&k| samples[k].1 * samples[k].2 * robust[k]).sum::<f32>()
                    / total
            };
            let x = members.iter().map(|&k| samples[k].0 * samples[k].2 * robust[k]).sum::<f32>()
                / total;
            points.push((x, y));
        }
        if points.len() < 2 {
            return None;
        }
        let mut order: Vec<usize> = (0..points.len()).collect();
        order.sort_by(|&a, &b| points[a].0.total_cmp(&points[b].0));
        points = order.iter().map(|&k| points[k]).collect();
        masses = order.iter().map(|&k| masses[k]).collect();
        // Monotone backstop: a real tone map is monotone; bin noise is not
        // allowed to fake a reversal the slider model would then chase.
        for k in 1..points.len() {
            if points[k].1 < points[k - 1].1 {
                points[k].1 = points[k - 1].1;
            }
        }
        if round == ROBUST_IRLS_ROUNDS {
            break;
        }
        let residuals: Vec<f32> = samples
            .iter()
            .map(|&(x, y, ..)| (y - sample_tone_points(&points, x)).abs())
            .collect();
        let scale = (1.4826
            * weighted_median_of(
                residuals.iter().zip(&samples).map(|(&r, s)| (r, s.2)).collect(),
            ))
        .max(ROBUST_SCALE_FLOOR);
        for (w, &r) in robust.iter_mut().zip(&residuals) {
            *w = tukey_weight(r, scale);
        }
    }
    // Verdict for EVERY paired pixel (the chromatic population included) via
    // the RGB transport residual: scale the source pixel by the fitted luma
    // gain and measure the worst channel miss. Chromatic pixels get their own
    // robust scale — a global saturation/WB edit moves all of them together
    // and only pixels far off THAT bulk are inconsistent.
    let mut weights = vec![1.0f32; n];
    // (residual, index, weight, chromatic?): the two classes get SEPARATE
    // robust scales. The transport residual models only the luma gain, so a
    // legitimate saturation/WB edit inflates every CHROMATIC residual
    // together — under one shared scale the neutral pixels' near-zero
    // residuals drag the median down and the whole chromatic population
    // (exactly the saturation evidence) is systematically down-weighted
    // (measured on the haze fixture: the colour stages came back empty).
    // Within its own class, a uniform edit clusters around the class median
    // and keeps weight; only pixels far off THEIR OWN bulk reject.
    let mut all_residuals: Vec<(f32, usize, f32, bool, Option<f32>)> = Vec::new();
    let (mut dir_sin, mut dir_cos) = (0.0f64, 0.0f64);
    for i in 0..n {
        let w = pair_weight(i);
        if w <= 0.0 {
            continue;
        }
        let (s, t) = (&sp[i], &tp[i]);
        let l = luma601(s);
        let gain = sample_tone_points(&points, l).clamp(0.0, 1.0) / l.max(1e-4);
        let residual = (0..3)
            .map(|c| (t[c] - (s[c] * gain).clamp(0.0, 1.0)).abs())
            .fold(0.0f32, f32::max);
        // Hue movement of the pair, where BOTH sides carry measurable hue —
        // the coherence voucher's raw material.
        let s_chroma = s[0].max(s[1]).max(s[2]) - s[0].min(s[1]).min(s[2]);
        let t_chroma = t[0].max(t[1]).max(t[2]) - t[0].min(t[1]).min(t[2]);
        let hue_delta = (s_chroma >= 0.06 && t_chroma >= 0.06).then(|| {
            let sh = render::rgb_to_hsl(s[0], s[1], s[2]).0 * 360.0;
            let th = render::rgb_to_hsl(t[0], t[1], t[2]).0 * 360.0;
            let mut d = th - sh;
            while d > 180.0 { d -= 360.0; }
            while d < -180.0 { d += 360.0; }
            d
        });
        if let Some(d) = hue_delta {
            let rad = (d as f64).to_radians();
            dir_sin += w as f64 * rad.sin();
            dir_cos += w as f64 * rad.cos();
        }
        all_residuals.push((residual, i, w, !is_neutralish(s), hue_delta));
    }
    if all_residuals.is_empty() {
        return None;
    }
    let class_dir = dir_sin.atan2(dir_cos).to_degrees() as f32;
    let scale_of = |chromatic: bool| -> f32 {
        let class: Vec<(f32, f32)> = all_residuals
            .iter()
            .filter(|&&(_, _, _, c, _)| c == chromatic)
            .map(|&(r, _, w, ..)| (r, w))
            .collect();
        if class.is_empty() {
            ROBUST_SCALE_FLOOR
        } else {
            (1.4826 * weighted_median_of(class)).max(ROBUST_SCALE_FLOOR)
        }
    };
    let scales = [scale_of(false), scale_of(true)];
    let mut rejected = 0.0f32;
    let mut total = 0.0f32;
    let mut range_rejected = [0.0f32; EVIDENCE_LUMA_BINS];
    for &(residual, i, w, chromatic, hue_delta) in &all_residuals {
        let scale = scales[chromatic as usize];
        let coherent = hue_delta.is_none_or(|d| {
            let mut dev = (d - class_dir).abs() % 360.0;
            if dev > 180.0 { dev = 360.0 - dev; }
            dev <= HUE_VOUCH_COHERENCE_DEG
        });
        let rw = if coherent { tukey_weight(residual, scale) } else { 0.0 };
        weights[i] = rw;
        total += w;
        if rw < 0.5 {
            rejected += w;
            range_rejected[evidence_luma_bin(luma601(&sp[i]))] += w;
        }
    }
    let rejected_share = if total > 0.0 { rejected / total } else { 0.0 };
    let rejected_ranges = range_rejected
        .iter()
        .enumerate()
        .filter(|&(_, &mass)| rejected > 0.0 && mass >= 0.10 * rejected)
        .map(|(bin, _)| {
            let lo = bin as f32 / EVIDENCE_LUMA_BINS as f32;
            let hi = (bin + 1) as f32 / EVIDENCE_LUMA_BINS as f32;
            format!("luma[{lo:.2}-{hi:.2}]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(PairedRobustTone { points, masses, weights, rejected_share, rejected_ranges })
}

pub(crate) fn sample_tone_points(points: &[(f32, f32)], x: f32) -> f32 {
    let Some(&(first_x, first_y)) = points.first() else { return x };
    if x <= first_x {
        return first_y + (x - first_x);
    }
    for pair in points.windows(2) {
        if x <= pair[1].0 {
            let t = (x - pair[0].0) / (pair[1].0 - pair[0].0).max(1e-6);
            return pair[0].1 + t * (pair[1].1 - pair[0].1);
        }
    }
    let &(last_x, last_y) = points.last().unwrap();
    last_y + (x - last_x)
}

/// Per-knot data support for an arbitrary weighted population (the zoned
/// fit's form of the global path's support closure): with a usable paired
/// map, a knot is supported inside the span the map points cover; otherwise
/// it needs [`SUPPORT_MIN_PIXELS`] of weight mass in its luma range — a
/// count, never a share (see the global closure's doc). A knot outside the
/// population must not pull the spline over the region it does occupy.
pub(crate) fn knot_support_for(
    px: &[[f32; 3]],
    weights: &[f32],
    points: &[(f32, f32)],
) -> [f32; 8] {
    if points.len() >= 6 {
        let (lo, hi) = (points[0].0 - 1.0 / 32.0, points[points.len() - 1].0 + 1.0 / 32.0);
        return std::array::from_fn(|i| {
            let x = render::TONE_KNOTS_X[i];
            if x >= lo && x <= hi { 1.0 } else { 0.0 }
        });
    }
    let mut mass = [0.0f32; EVIDENCE_LUMA_BINS];
    for (p, &w) in px.iter().zip(weights) {
        if w > 0.0 {
            mass[evidence_luma_bin(luma601(p))] += w;
        }
    }
    std::array::from_fn(|i| {
        if mass[evidence_luma_bin(render::TONE_KNOTS_X[i])] >= SUPPORT_MIN_PIXELS {
            1.0
        } else {
            0.0
        }
    })
}

/// Magnitude prior for the tone solve. The 5-slider knot system is
/// near-collinear (contrast vs shadows/highlights, whites vs the shoulder), so
/// unpenalised least squares happily returns huge mutually-cancelling sliders
/// whose TOTAL map ties a tasteful solution to within numerical ε — and the
/// residual curve erases even that difference. The prior makes slider
/// magnitude itself part of the cost, so "Exposure +1.5, Contrast −97,
/// Shadows −100" loses to the mild solve it was shadowing. Units: basis
/// authorities are O(0.2–0.34), knot residuals O(0.1); 0.02 prices a pegged
/// slider (s=1) like a ~0.14 luma miss at one knot — strong enough to kill
/// cancellation combos, weak enough that genuinely-needed big moves survive
/// (the roundtrip test pins recovery of a real ±25-point recipe).
const TONE_PRIOR: f64 = 0.01;

/// Scan exposure (nonlinear in the model) and, for each candidate, solve the 5
/// linear sliders (contrast/highlights/shadows/whites/blacks, in the basis
/// order of [`render::tone_slider_basis`]) by RIDGE least squares over the 8
/// knots; keep the (ev, sliders) minimising the PENALISED clamped-solution
/// score `SSE + TONE_PRIOR·Σs²` — the same prior in the solve and in the
/// model selection, so the exposure scan cannot smuggle the degeneracy back.
#[cfg(test)]
pub(crate) fn fit_tone_sliders(tone_map: &impl Fn(f32) -> f32) -> (f32, [f32; 5]) {
    fit_tone_sliders_supported(tone_map, &[1.0; 8], &[])
}

/// [`fit_tone_sliders`] with per-knot DATA support composed into the knot
/// weights. Engine authority says how far a slider can move a knot; support
/// says whether any measured pixel testifies there. A knot with no testimony
/// contributes nothing to the solve or the model-selection score, so the
/// exposure scan cannot buy phantom-knot fit either. Fewer than two supported
/// knots is no tone problem at all — return neutral instead of solving a
/// one-point system.
pub(crate) fn fit_tone_sliders_supported(
    tone_map: &impl Fn(f32) -> f32,
    support: &[f32; 8],
    score_set: &[(f32, f32, f32)],
) -> (f32, [f32; 5]) {
    if support.iter().filter(|&&s| s > 0.0).count() < 2 {
        return (0.0, [0.0; 5]);
    }
    // The engine cannot output past [0,1]; an estimated map may extrapolate
    // there, and an unclamped target would price impossible demand into the
    // supported knots' shared sliders.
    let targets: Vec<f32> =
        render::TONE_KNOTS_X.iter().map(|&x| tone_map(x).clamp(0.0, 1.0)).collect();
    let basis: Vec<[f32; 5]> =
        render::TONE_KNOTS_X.iter().map(|&x| render::tone_slider_basis(x)).collect();

    let mut best = (0.0f32, [0.0f32; 5], f32::INFINITY);
    let mut ev = -3.0f32;
    while ev <= 3.0 + 1e-6 {
        // Residual after the exposure component, then ridge normal equations.
        // Knot authority (`tone_knot_weights`) rides the basis rows: it
        // depends only on the candidate ev, so the system stays linear in the
        // sliders — and the solve models the SAME engine that will render the
        // result (an unweighted basis would ask saturated knots to explain
        // residual they can no longer move).
        let authority = render::tone_knot_weights(ev);
        let weights: [f32; 8] = std::array::from_fn(|i| authority[i] * support[i]);
        let resid: Vec<f64> = render::TONE_KNOTS_X
            .iter()
            .zip(&targets)
            .map(|(&x, &t)| (t - render::tone_exposure_curve(x, ev)) as f64)
            .collect();
        let mut ata = [[0.0f64; 5]; 5];
        let mut atb = [0.0f64; 5];
        for ((b, r), &w) in basis.iter().zip(&resid).zip(&weights) {
            for i in 0..5 {
                let bi = (w * b[i]) as f64;
                for j in 0..5 {
                    ata[i][j] += bi * (w * b[j]) as f64;
                }
                atb[i] += bi * r;
            }
        }
        for (i, row) in ata.iter_mut().enumerate() {
            row[i] += TONE_PRIOR; // ridge = the magnitude prior (see const doc)
        }
        let sol = solve5(ata, atb);
        let s: [f32; 5] = std::array::from_fn(|i| (sol[i] as f32).clamp(-1.0, 1.0));
        let penalty: f64 = s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum();
        // Model selection. With a paired score set, the candidate is judged
        // through the ENGINE'S OWN spline at the robust map points, weighted
        // by the pixel mass behind each point — the 8-knot residual cannot
        // tell near-collinear (ev, sliders) combinations apart (their splines
        // agree AT the knots and differ between them, where the pixels live),
        // and the magnitude prior then tie-breaks toward the small-slider
        // impostor (measured: the roundtrip truth ev+0.35/highlights −25 lost
        // to ev+0.20/highlights +8). Normalised to the 8-knot scale so
        // TONE_PRIOR keeps its calibrated strength.
        let score: f64 = if score_set.is_empty() {
            // Weighted-least-squares scoring, consistent with the normal
            // equations above: the row's weight multiplies the WHOLE
            // residual, so a zero-weight knot contributes nothing. The old
            // form weighted only the model half ((r − w·fit)²), which
            // charged every candidate a zero-authority knot's RAW residual —
            // and since that charge varies with ev, the scan minimised
            // phantom-knot residuals no slider could touch (the unit gate
            // test caught it: identity-on-supported solved to ev +0.40).
            basis
                .iter()
                .zip(&resid)
                .zip(&weights)
                .map(|((b, r), &w)| {
                    let fit: f64 = (0..5).map(|i| b[i] as f64 * s[i] as f64).sum();
                    let d = w as f64 * (r - fit);
                    d * d
                })
                .sum::<f64>()
                + penalty
        } else {
            let knots = render::tone_model_knots(ev, s);
            let mut sse = 0.0f64;
            let mut mass = 0.0f64;
            for &(x, y, m) in score_set {
                let d = (render::sample_tone_model(&knots, x) - y.clamp(0.0, 1.0)) as f64;
                sse += m as f64 * d * d;
                mass += m as f64;
            }
            if mass > 0.0 { 8.0 * sse / mass + penalty } else { penalty }
        };
        if (score as f32) < best.2 {
            best = (ev, s, score as f32);
        }
        ev += 0.05;
    }
    // NOT limited here on purpose. `render::limit_tone_sliders` saturates a
    // slider vector that would flatten a tonal band, and the engine applies it
    // at render time — but applying it to the PROPOSAL as well perturbs this
    // least-squares solve, and the acceptance test downstream is a knife edge:
    // on the hazy-to-clean fixture as it stood pre-R17 (gated evidence,
    // sparse residual knots) the solve was only 3 % better than neutral
    // (0.08625 against 0.08918), so a 0.34 % nudge to the sliders pushed it
    // over `err_before`, tripped the saturation do-no-harm loop, and ended at
    // 0.1286 — far worse than doing nothing. (R17's evidence fallback moved
    // that fixture to 0.0892 → 0.0229; the numbers above are kept as the
    // historical record of WHY the asymmetry exists — the knife-edge
    // geometry, not the exact figures, is the reason.) The fit does not need
    // to predict the limiter anyway: it scores candidates by RENDERING them
    // (`develop_preview` below), so it already measures whatever the engine
    // actually does.
    (best.0, best.1)
}

/// Gaussian elimination with partial pivoting for the 5×5 normal equations.
fn solve5(mut a: [[f64; 5]; 5], mut b: [f64; 5]) -> [f64; 5] {
    for c in 0..5 {
        let mut p = c;
        for r in c + 1..5 {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        a.swap(c, p);
        b.swap(c, p);
        if a[c][c].abs() < 1e-12 {
            continue;
        }
        let pivot = a[c]; // copy of the pivot row ([f64; 5] is Copy)
        for r in c + 1..5 {
            let f = a[r][c] / pivot[c];
            for k in c..5 {
                a[r][k] -= f * pivot[k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = [0.0f64; 5];
    for c in (0..5).rev() {
        let mut acc = b[c];
        for k in c + 1..5 {
            acc -= a[c][k] * x[k];
        }
        x[c] = if a[c][c].abs() < 1e-12 { 0.0 } else { acc / a[c][c] };
    }
    x
}

/// Whatever tonal shape the sliders could not express, as `tone_curve` control
/// points. The engine composes `tone_curve` AFTER the knot spline `S`, so the
/// exact residual curve is `M ∘ S⁻¹` — i.e. points `(S(x), M(x))`. Monotone by
/// construction (both `S` and `M` are monotone); skipped when the residual is
/// within tolerance everywhere.
#[cfg(test)]
fn residual_tone_curve_with_samples(
    recipe: &EditRecipe,
    tone_map: &impl Fn(f32) -> f32,
    extra_xs: &[f32],
    supported: &impl Fn(f32) -> bool,
) -> Vec<CurvePoint> {
    residual_tone_curve_with_budget(recipe, tone_map, extra_xs, supported, (0.0, RESIDUAL_SLOPE_CAP))
}

fn residual_tone_curve_with_budget(
    recipe: &EditRecipe,
    tone_map: &impl Fn(f32) -> f32,
    extra_xs: &[f32],
    supported: &impl Fn(f32) -> bool,
    slope: (f32, f32),
) -> Vec<CurvePoint> {
    debug_assert!(recipe.tone_curve.is_empty(), "fit the residual before setting a curve");
    let lut = render::build_tone_lut(recipe);
    // Knot placement (R17): uniform in the LUT's OUTPUT domain, inverted
    // back through the LUT — the curve's input axis IS the engine's output
    // (`sx` below), so sampling uniform in raw x inherits the base curve's
    // compression. On the real camera base the old fixed 9 xs left a single
    // 38-u8 input gap right across the band holding the frame's tonal mass,
    // and the curve's PIECEWISE-LINEAR rendering (`render::curve_lut` →
    // `interp` — not the monotone cubic the knot spline uses) chords
    // ~10/255 below the concave desired map inside it (measured, P20
    // × reimagine). 13 output levels bound the inter-knot input gap to
    // ~21 u8 wherever the LUT moves; where it is flat the levels collapse
    // onto one x and the `prev_in` dedup keeps the point list minimal —
    // which also means a flat plateau's interior is no longer sampled by
    // `max_dev` (the old fixed xs could land mid-plateau): deliberate, a
    // many-to-one plateau is beyond any input-side curve's reach anyway.
    // The trade's cost side: 21-u8 spacing doubles the density of u8-rounded
    // control points, ~±0.5/255 of quantisation ripple bought against the
    // ~10/255 of chord sag removed — a 20:1 win. Evidence-withheld luma
    // boundaries are added by the caller so interpolation cannot bridge a
    // refusal with a neighboring supported move.
    const LEVELS: usize = 13;
    let mut xs: Vec<f32> = (0..LEVELS).map(|i| {
        let o = i as f32 / (LEVELS - 1) as f32;
        let idx = lut.partition_point(|&v| v < o).min(lut.len() - 1);
        idx as f32 / (lut.len() - 1) as f32
    }).chain(extra_xs.iter().copied()).collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let mut max_dev = 0.0f32;
    let mut pts: Vec<CurvePoint> = Vec::with_capacity(LEVELS);
    let (mut prev_in, mut prev_out) = (-1i32, 0i32);
    for x in xs {
        if !supported(x) {
            continue; // no source pixels there — the map is extrapolation
        }
        let sx = render::sample_lut(&lut, x); // engine output before the residual curve
        let y = tone_map(x).clamp(0.0, 1.0); // desired output
        max_dev = max_dev.max((y - sx).abs());
        let input = (sx * 255.0).round() as i32;
        let output = ((y * 255.0).round() as i32).max(prev_out); // keep monotone
        if input <= prev_in {
            continue; // spline outputs can quantise together at the ends
        }
        pts.push(CurvePoint { input: input as u8, output: output as u8 });
        (prev_in, prev_out) = (input, output);
    }
    if max_dev < 0.015 {
        Vec::new() // the sliders already express the map — keep the recipe clean
    } else {
        project_curve_slopes(&pts, slope.0, slope.1)
    }
}

#[cfg(test)]
fn residual_tone_curve(recipe: &EditRecipe, tone_map: &impl Fn(f32) -> f32) -> Vec<CurvePoint> {
    residual_tone_curve_with_samples(recipe, tone_map, &[], &|_| true)
}

// --------------------------------------------------------------------------
// colour residuals
// --------------------------------------------------------------------------

/// Per-band accumulator: weight, circular hue (sin/cos), and the two
/// magnitudes the per-band mixer actually steers — CHROMA (max-min, exactly
/// what `apply_hsl`'s saturation axis scales: at fixed HSL lightness the
/// reconstructed chroma is 2*l*s below mid-grey and 2*s*(1-l) above it, so it
/// is proportional to `s` on both sides) and Rec.601 LUMA.
///
/// Deliberately NOT HSL's own `s` and `l`. `s` is ill-conditioned near white
/// and black — the renderer gates the whole mixer on chroma for that very
/// reason — and `l` = (max+min)/2 RISES when chroma alone rises, so reading
/// the luminance axis off it lets a band's saturation gap masquerade as a
/// brightness gap: measured on the four-family fixture, a target whose blue
/// quarter is 1.64x more chromatic at identical Rec.601 luma asked for
/// +22 luminance, which is a demand about colour wearing brightness's clothes.
#[derive(Clone, Copy, Default)]
struct BandStat {
    w: f64,
    sin: f64,
    cos: f64,
    c: f64,
    y: f64,
}

/// Accumulate chroma-gated band statistics with the SAME partition of unity the
/// renderer uses ([`render::bracket_bands`]), so the fit and the engine agree on
/// what "the blue band" is. Returns the per-band stats and the chromatic total.
#[cfg(test)]
fn band_stats(px: &[[f32; 3]]) -> ([BandStat; 8], f64) {
    let mut bands = [BandStat::default(); 8];
    let mut total = 0.0f64;
    for p in px {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < 0.06 {
            continue; // matches the renderer's chroma gate: near-grey carries no hue evidence
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
        let ang = (h * std::f32::consts::TAU) as f64;
        let luma = luma601(p) as f64;
        for (bi, w) in [(b0, 1.0 - w1 as f64), (b1, w1 as f64)] {
            let b = &mut bands[bi];
            b.w += w;
            b.sin += w * ang.sin();
            b.cos += w * ang.cos();
            b.c += w * chroma as f64;
            b.y += w * luma;
        }
        total += 1.0;
    }
    (bands, total)
}

fn band_stats_weighted(px: &[[f32; 3]], weights: &[f32]) -> ([BandStat; 8], f64) {
    let mut bands = [BandStat::default(); 8];
    let mut total = 0.0f64;
    for (i, p) in px.iter().enumerate() {
        let weight = weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
        if weight <= 0.0 {
            continue;
        }
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < 0.06 {
            continue;
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
        let ang = (h * std::f32::consts::TAU) as f64;
        let luma = luma601(p) as f64;
        for (bi, w) in [(b0, 1.0 - w1 as f64), (b1, w1 as f64)] {
            let w = w * weight;
            let b = &mut bands[bi];
            b.w += w;
            b.sin += w * ang.sin();
            b.cos += w * ang.cos();
            b.c += w * chroma as f64;
            b.y += w * luma;
        }
        total += weight;
    }
    (bands, total)
}

/// Residual per-channel CDF map (current render → target) as a channel curve —
/// the colour-cast catch-all (white balance shift, split toning the wheels/HSL
/// didn't express). Skipped when the channel already matches within tolerance.
#[cfg(test)]
fn residual_channel_curve(cur: &[[f32; 3]], tgt: &[[f32; 3]], ch: usize) -> Vec<CurvePoint> {
    let all_a = vec![1.0; cur.len()];
    let all_b = vec![1.0; tgt.len()];
    residual_channel_curve_weighted(cur, tgt, ch, &all_a, &all_b)
}

fn residual_channel_curve_weighted(
    cur: &[[f32; 3]],
    tgt: &[[f32; 3]],
    ch: usize,
    cur_weights: &[f32],
    tgt_weights: &[f32],
) -> Vec<CurvePoint> {
    let c_cdf = weighted_cdf(cur, cur_weights, |p| p[ch]);
    let t_cdf = weighted_cdf(tgt, tgt_weights, |p| p[ch]);
    if c_cdf.iter().all(|&v| v <= 0.0) || t_cdf.iter().all(|&v| v <= 0.0) {
        return Vec::new();
    }
    const XS: [f32; 5] = [0.0, 0.25, 0.50, 0.75, 1.0];
    // The keep/skip decision is judged at the SOURCE DISTRIBUTION's own
    // quantiles — |Q_t(q) − Q_c(q)| — where the pixel mass actually sits.
    // Judging at the fixed intensity knots had two failure modes: identical
    // low-dynamic-range channels tripped the gate on their clipped endpoint
    // samples (emitting a non-identity curve through real pixels), and a
    // genuine narrow-band shift (0.30-0.40 → 0.40-0.50) fell BETWEEN the
    // knots and was suppressed entirely.
    let mut max_dev = 0.0f32;
    // Down to the 2nd/98th percentile — the P_CLIP band the curve itself
    // preserves: a 5% cluster shift left every 10-90 quantile untouched and
    // was suppressed. Identical distributions still read 0 everywhere.
    for &q in &[0.02f32, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.98] {
        let xc = quantile(&c_cdf, q);
        let xt = quantile(&t_cdf, q);
        max_dev = max_dev.max((xt - xc).abs());
    }
    if max_dev < 0.012 {
        return Vec::new();
    }
    let mut pts: Vec<CurvePoint> = Vec::with_capacity(XS.len());
    let (mut prev_in, mut prev_out) = (-1i32, 0i32);
    for &x in &XS {
        let f = cdf_at(&c_cdf, x);
        let y = quantile(&t_cdf, f.clamp(P_CLIP, 1.0 - P_CLIP)).clamp(0.0, 1.0);
        let input = (x * 255.0).round() as i32;
        let output = ((y * 255.0).round() as i32).max(prev_out);
        if input <= prev_in {
            continue;
        }
        pts.push(CurvePoint { input: input as u8, output: output as u8 });
        (prev_in, prev_out) = (input, output);
    }
    pts
}

/// Shrink the three fitted channel curves along one continuous path, so a
/// cast the hue-fan gate convicts can be made milder instead of thrown away.
///
/// `t = 1` is the fitted cast, `t = 0` is no cast at all, and the path
/// between them gives up the CHROMATIC part first:
///
/// ```text
///   L      = per-knot mean of the three fitted outputs (the shape all three
///            channels share; one curve applied to every channel)
///   dC     = C − L                      (each channel's chromatic deviation)
///   C(t)   = x + min(1, 2t)·(L − x) + max(0, 2t − 1)·dC
///   t = 1  → C          (as fitted)
///   t = 0.5→ L          (one shared curve, no chromatic difference at all)
///   t = 0  → x          (the identity: no curves)
/// ```
///
/// The upper half is the projection proper — the fan is a RELATIONAL defect
/// (at each input level the three curves hold three different outputs, and
/// that per-level difference is the chromatic move that sorts one hue class
/// by luminance), so the first thing to give up is `dC`.
///
/// The LOWER half exists because measurement said it had to. The design this
/// implements stopped at `L`, on the premise that one curve applied to all
/// three channels cannot fan a hue class. That premise is false, and the
/// showcase pair is where it fails: hue is a RATIO, so a shared curve moves
/// it wherever its slope changes, and the Cornwall shared shape's segment
/// slopes are 0.172 / 0.859 / 1.127 / 0.188 (its top segment is nearly flat
/// because the fitted red curve clips at 179 from input 191 up). Measured on
/// that shape (2026-09-02): a dark sky pixel moves +0.2°, a mid one −3.2°, a
/// bright one −20.1° as its blue channel is crushed toward the other two —
/// and the census reads 17.3° of ADDED fan at `t = 0.5`, above [`FAN_DEG`]
/// itself. A family whose mildest member is still convicted cannot rescue
/// anything, so the path continues to the identity, where the fan is zero by
/// construction and the outcome is exactly today's refusal.
///
/// It stays three Lightroom RGB curves at every `t`, so the recipe still
/// round-trips to XMP, and it is the same idiom one stage up — shrink until
/// the finished frame stops objecting (the mixer's halve-and-refit
/// do-no-harm loop).
///
/// An EMPTY channel curve is the identity and is resampled as one: a channel
/// whose residual fell under `residual_channel_curve_weighted`'s keep
/// threshold while the other two carry a cast is still a channel, and
/// dropping it would make `L` the mean of a different number of channels.
/// It comes back out the way it went in — a projected curve that lands on the
/// IDENTITY at every knot is emitted EMPTY, per channel, exactly as the fit
/// leaves a channel it never fitted. Without that, `t = 1` handed an empty
/// channel back as an explicit five-knot identity curve: a dead curve in the
/// recipe and in the XMP, and `cast_curves_are_identity` would not catch it
/// because it only bails when ALL THREE channels are dead. Pinned by the
/// empty-channel leg of `the_bottom_of_the_projection_path_is_one_curve_then_none`.
///
/// Outputs are rounded and monotone-clamped exactly as the fitted curves are
/// (round to the 0..255 code, never below the previous knot), so a projected
/// curve is the same KIND of object the fit emits, not a finer one.
fn projected_cast_curves(curves: [&[CurvePoint]; 3], t: f32) -> [Vec<CurvePoint>; 3] {
    let mut xs: Vec<u8> = curves.iter().flat_map(|c| c.iter().map(|p| p.input)).collect();
    xs.sort_unstable();
    xs.dedup();
    // Linear resample onto the shared grid. On every production input this is
    // a no-op — `residual_channel_curve_weighted` emits the same fixed knots
    // for every channel it keeps — but the mean of three curves is only
    // meaningful knot-by-knot, so pairing by INDEX rather than by input would
    // be a silent bug the day the knot sets ever differ.
    let sample = |c: &[CurvePoint], x: u8| -> f32 {
        let Some(first) = c.first() else { return x as f32 };
        if x <= first.input {
            return first.output as f32;
        }
        for w in c.windows(2) {
            if x <= w[1].input {
                let span = (w[1].input as f32 - w[0].input as f32).max(1e-6);
                let f = (x as f32 - w[0].input as f32) / span;
                return w[0].output as f32 + f * (w[1].output as f32 - w[0].output as f32);
            }
        }
        c[c.len() - 1].output as f32
    };
    let toward_shared = (2.0 * t).clamp(0.0, 1.0);
    let toward_fitted = (2.0 * t - 1.0).clamp(0.0, 1.0);
    let mut out: [Vec<CurvePoint>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut prev = [0i32; 3];
    for &x in &xs {
        let ys = [sample(curves[0], x), sample(curves[1], x), sample(curves[2], x)];
        let shared = (ys[0] + ys[1] + ys[2]) / 3.0;
        for (channel, (&y, floor)) in out.iter_mut().zip(ys.iter().zip(prev.iter_mut())) {
            let value = x as f32
                + toward_shared * (shared - x as f32)
                + toward_fitted * (y - shared);
            let code = (value.round() as i32).clamp(0, 255).max(*floor);
            channel.push(CurvePoint { input: x, output: code as u8 });
            *floor = code;
        }
    }
    // Per channel: a curve that is the identity at every knot is no curve.
    // Emitting it EMPTY is what makes `t = 1` reproduce the FITTED curves
    // byte for byte including the channels the fit left empty, and it keeps a
    // projected recipe free of curves that do nothing.
    for channel in out.iter_mut() {
        if channel.iter().all(|p| p.input == p.output) {
            channel.clear();
        }
    }
    out
}

/// Are these three curves the identity at every knot — i.e. is this candidate
/// "no cast at all"? The bottom of the projection path is the identity by
/// construction, and shipping it would put a sentence about projected curves
/// on a recipe whose curves do nothing. An EMPTY curve is the identity, which
/// is why the all-empty bottom of the path answers `true` here.
fn cast_curves_are_identity(curves: &[Vec<CurvePoint>; 3]) -> bool {
    curves.iter().all(|c| c.iter().all(|p| p.input == p.output))
}

/// The projection SEARCH, over `t` alone: among the ADMISSIBLE shrinks — all
/// four gates clear and the fan is no more than [`FAN_PROJECT_DEG`] — the one
/// that buys the MOST look error, shipped when that gain clears [`FIT_QUANT`].
/// `None` when nothing on the path qualifies, and then the stage refuses the
/// cast exactly as it did before the projection existed.
///
/// THREE PHASES, and the shape of each follows from what is monotone in `t`
/// and what is not.
///
/// PHASE 1 finds the admissible FRONTIER `t_max` by bisection, 12 steps — the
/// convention this file already uses for closed-loop searches — because
/// admissibility is the DOWNWARD-CLOSED half of the question and the only
/// half a bisection may be pointed at: the fan is non-decreasing in `t`
/// (measured, `the_projected_fan_grows_with_t`) and every gate clears as the
/// curves go to the identity.
///
/// PHASE 2 sweeps a fixed grid of [`PROJECT_GRID`] cells over `(0, t_max]`
/// and keeps the admissible probe with the largest GAIN, the frontier
/// included. The gain is NOT monotone in `t`, and that was measured before it
/// was assumed: on the coast fixture's stage-4 candidate (2026-09-02) it
/// reads 0.00104 at `t = 0.25`, 0.00190 at 0.35, 0.00169 at 0.40 and 0.00187
/// at 0.50 — a wiggle the size of [`FIT_QUANT`] itself. Judging the frontier
/// ALONE therefore refused pairs on which an admissible paying shrink exists,
/// and that was v1.2.3's stated cost: on the two-family HSL pair every
/// `t ≤ 0.25` is admissible and pays 0.0019–0.0033 while the frontier reads
/// a gain of −0.012, so the pair was refused although the path held a rescue
/// worth more than the quantisation budget. Sweeping the interval is what
/// closes that, and the sweep is also what makes the gain bar sound: the bar
/// is now applied to the MAXIMUM over the admissible set, so a `None` really
/// does mean "nothing on this path pays".
///
/// PHASE 3 refines the winning cell by golden section, [`PROJECT_REFINE`]
/// iterations, so the answer is not quantised to the grid. Every probe in
/// both phases is re-judged by all four gates from scratch and only an
/// admissible probe can win, so neither the sweep nor the refinement can walk
/// out of the admissible set even where the RENDERED fan is not exactly
/// monotone: an inadmissible probe scores `-inf` and the bracket closes away
/// from it.
///
/// The GAIN requirement is this function's own and is stricter than the
/// ratio gate. The gates decide whether a cast the fit MEASURED may ship;
/// they do not decide whether a milder one the fit INVENTED is worth
/// shipping, and the stage's standing doctrine is that marginal gain does not
/// earn regional risk. So a projected candidate has to buy more absolute look
/// error than [`FIT_QUANT`], the fit's own quantisation budget — the same
/// number the terminal do-no-harm check uses to decide that a difference is a
/// difference at all.
///
/// Pinned by `the_search_takes_the_best_paying_admissible_shrink`, which
/// builds a path whose gain peaks in the interior and asserts that the search
/// lands on the peak rather than on the frontier.
fn search_cast_projection_t(
    err_without: f32,
    mut judge: impl FnMut(f32) -> CastOutcome,
) -> Option<(f32, CastOutcome)> {
    // An ABSTAINING census CLEARS the fan target, and that is a reading rather
    // than a hole in one: `hue_fan_weighted` returns `None` when no hue class
    // holds [`FAN_SHARE`] of the population across two populated luma slices,
    // so there is no longer anything region-sized that COULD be over the
    // target. It reaches the user as `FIT_NOTE_CAST_PROJECTED_FAN_NA`, which
    // prints no digit, so an abstention is never published as 0.0 either.
    let admissible = |out: &CastOutcome| {
        !out.refused()
            && out
                .readings
                .is_some_and(|r| r.fan.is_none_or(|fan| fan <= FAN_PROJECT_DEG))
    };
    // PHASE 1 — the admissible FRONTIER, by bisection over the one
    // downward-closed half of the question.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut frontier: Option<(f32, CastOutcome)> = None;
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        let out = judge(mid);
        if admissible(&out) {
            frontier = Some((mid, out));
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (t_max, frontier_out) = frontier?;
    let gain_of = |out: &CastOutcome| out.readings.map(|r| err_without * (1.0 - r.ratio));
    // Judge one `t`, keep it when it is admissible and pays more than the best
    // so far, and score an inadmissible probe `-inf` so the golden section
    // closes away from it. `best` is a parameter rather than a capture so this
    // closure borrows `judge` alone.
    let mut probe = |t: f32, best: &mut (f32, CastOutcome, f32)| -> f32 {
        let out = judge(t);
        if !admissible(&out) {
            return f32::NEG_INFINITY;
        }
        // `admissible` has already established that the readings are present.
        let Some(gain) = gain_of(&out) else { return f32::NEG_INFINITY };
        if gain > best.2 {
            *best = (t, out, gain);
        }
        gain
    };
    // PHASE 2 — the gain sweep over `(0, t_max]`. The frontier is already
    // judged, so it seeds the comparison without a second render.
    let mut best = (t_max, frontier_out, gain_of(&frontier_out)?);
    let cell = t_max / PROJECT_GRID as f32;
    for k in 1..PROJECT_GRID {
        probe(cell * k as f32, &mut best);
    }
    // PHASE 3 — golden section on the cell either side of the winner, in the
    // classic three-point form: one new render per iteration, the other
    // interior point carried over from the last one.
    const INV_PHI: f32 = 0.618_034;
    let (mut a, mut b) = ((best.0 - cell).max(0.0), (best.0 + cell).min(t_max));
    let (mut c, mut d) = (b - INV_PHI * (b - a), a + INV_PHI * (b - a));
    let (mut fc, mut fd) = (probe(c, &mut best), probe(d, &mut best));
    for _ in 0..PROJECT_REFINE {
        if fc >= fd {
            b = d;
            d = c;
            fd = fc;
            c = b - INV_PHI * (b - a);
            fc = probe(c, &mut best);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + INV_PHI * (b - a);
            fd = probe(d, &mut best);
        }
    }
    // The gain bar, once, on the MAXIMUM over the admissible set.
    (best.2 > FIT_QUANT).then_some((best.0, best.1))
}

/// The projection search wired to the RENDERER: `search_cast_projection_t`
/// with a `judge` that puts `C(t)` into the recipe, develops it and hands the
/// PIXELS to the gate — never the curves. The census has to read what the user
/// will see, the same rule every other closed-loop stage in this file follows,
/// and `gate` runs all four gates, so the projection is a search the gates
/// referee and not a way around them.
///
/// It is reached from TWO call sites, and both of them produce the recipe the
/// user gets: the `fit_cast_stage` call after the mixer's do-no-harm block,
/// and the 4b do-no-harm loop's re-fit, which REPLACES that recipe one
/// saturation step down. The mixer's own do-no-harm comparison judges both of
/// its branches with the cast the gates MEASURED — see the note at that loop.
///
/// The second call site is EXERCISED but its success is UNFIXTURED. Measured
/// 2026-09-02 by instrumenting the 4b loop body and running the whole library
/// battery: the body runs 189 times and nine of those re-fits carry a
/// fan-convicted cast, so the rescue is entered there with something to
/// answer — but in all nine the search finds nothing that both clears
/// [`FAN_PROJECT_DEG`] and pays [`FIT_QUANT`], so no test in the tree has ever
/// seen a PROJECTED cast come back out of that loop. That half is verified by
/// reading.
///
/// On return `recipe` carries the WINNING candidate's curves, not the last
/// probe's — the loop's final probe is a rejected `t` more often than not.
fn search_cast_projection(
    s_img: &DynamicImage,
    recipe: &mut EditRecipe,
    fitted: [&[CurvePoint]; 3],
    err_without: f32,
    gate: impl Fn(&[[f32; 3]]) -> CastOutcome,
) -> Option<(f32, CastOutcome)> {
    let (t, out) = search_cast_projection_t(err_without, |t| {
        let [red, green, blue] = projected_cast_curves(fitted, t);
        recipe.red_curve = red;
        recipe.green_curve = green;
        recipe.blue_curve = blue;
        gate(&pixels_of(&render::develop_preview(s_img, recipe)))
    })?;
    let curves = projected_cast_curves(fitted, t);
    if cast_curves_are_identity(&curves) {
        return None;
    }
    let [red, green, blue] = curves;
    recipe.red_curve = red;
    recipe.green_curve = green;
    recipe.blue_curve = blue;
    Some((t, out))
}

/// The set of 15°-hue bins FOREIGN to the target: farther than
/// [`VETO_FAR_BINS`] bins (circularly) from every bin holding ≥
/// [`VETO_SUPPORT_BIN_MIN`] of the target's chromatic mass. `None` when the
/// target has fewer than [`VETO_MIN_TARGET_CHROMATIC`] chromatic pixels — no
/// reliable hue testimony, the veto stands down.
fn foreign_hue_bins(tp: &[[f32; 3]]) -> Option<[bool; 24]> {
    let mut mass = [0.0f32; 24];
    let mut n = 0usize;
    for p in tp {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < VETO_SUPPORT_CHROMA {
            continue;
        }
        let (h, _s, _l) = render::rgb_to_hsl(p[0], p[1], p[2]);
        mass[((h * 24.0) as usize).min(23)] += 1.0;
        n += 1;
    }
    if n < VETO_MIN_TARGET_CHROMATIC {
        return None;
    }
    let populated: Vec<usize> =
        (0..24).filter(|&k| mass[k] / n as f32 >= VETO_SUPPORT_BIN_MIN).collect();
    let mut foreign = [true; 24];
    for (k, f) in foreign.iter_mut().enumerate() {
        for &p in &populated {
            let fwd = (k as isize - p as isize).rem_euclid(24) as usize;
            if fwd.min(24 - fwd) <= VETO_FAR_BINS {
                *f = false;
                break;
            }
        }
    }
    Some(foreign)
}

/// Fraction of the frame visibly tinted at a hue foreign to the target
/// (chroma ≥ [`VETO_TINT_CHROMA`], hue in a foreign bin).
fn foreign_share(px: &[[f32; 3]], foreign: &[bool; 24]) -> f32 {
    let mut cnt = 0usize;
    for p in px {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < VETO_TINT_CHROMA {
            continue;
        }
        let (h, _s, _l) = render::rgb_to_hsl(p[0], p[1], p[2]);
        if foreign[((h * 24.0) as usize).min(23)] {
            cnt += 1;
        }
    }
    cnt as f32 / px.len().max(1) as f32
}

/// Did a global cast transform paint a REGION of the frame in hues the target holds
/// nowhere (≥ [`VETO_FAR_BINS`]·15° from all its populated hue mass)?
/// `cur`/`with_px` render the SAME source, so the share DELTA is exactly the
/// transform's own work — pre-existing content mismatch cancels out. Full-mode
/// channel curves and Atmosphere white balance intentionally share this law.
fn cast_paints_foreign_hues(cur: &[[f32; 3]], with_px: &[[f32; 3]], tp: &[[f32; 3]]) -> bool {
    let Some(foreign) = foreign_hue_bins(tp) else {
        return false;
    };
    foreign_share(with_px, &foreign) - foreign_share(cur, &foreign) >= VETO_CREATED_SHARE
}

fn foreign_hue_bins_weighted(
    tp: &[[f32; 3]],
    weights: &[f32],
) -> Option<[bool; 24]> {
    let mut mass = [0.0f32; 24];
    let mut total = 0.0f32;
    for (p, &weight) in tp.iter().zip(weights) {
        let weight = weight.max(0.0);
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if weight <= 0.0 || chroma < VETO_SUPPORT_CHROMA {
            continue;
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        mass[((h * 24.0) as usize).min(23)] += weight;
        total += weight;
    }
    if total < VETO_MIN_TARGET_CHROMATIC as f32 {
        return None;
    }
    let populated: Vec<usize> =
        (0..24).filter(|&bin| mass[bin] / total >= VETO_SUPPORT_BIN_MIN).collect();
    let mut foreign = [true; 24];
    for (bin, is_foreign) in foreign.iter_mut().enumerate() {
        for &populated_bin in &populated {
            let forward = (bin as isize - populated_bin as isize).rem_euclid(24) as usize;
            if forward.min(24 - forward) <= VETO_FAR_BINS {
                *is_foreign = false;
                break;
            }
        }
    }
    Some(foreign)
}

fn cast_paints_foreign_hues_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    foreign_created_share_weighted(cur, with_px, tp, evidence)
        .is_some_and(|created| created >= VETO_CREATED_SHARE)
}

/// The foreign-hue population share the curves CREATE (with − without), or
/// `None` when the target carries no reliable hue evidence — the measurement
/// behind [`cast_paints_foreign_hues_weighted`], exposed so an ADMITTED cast
/// can disclose the reading that let it through.
fn foreign_created_share_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<f32> {
    // No paired-convergence exemption here either — painting hue mass the
    // target holds nowhere is capability policy like the rotation gate; the
    // vanished-population case it guards (canyon) is content divergence the
    // voucher must never launder.
    let foreign = foreign_hue_bins_weighted(tp, &evidence.target_hue_weights)?;
    let weighted_foreign = |px: &[[f32; 3]]| -> f32 {
        let mut hit = 0.0;
        let mut total = 0.0;
        for (i, p) in px.iter().enumerate() {
            let w = evidence.source_hue_weights.get(i).copied().unwrap_or(0.0).max(0.0);
            if w <= 0.0 { continue; }
            total += w;
            let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
            if chroma < VETO_TINT_CHROMA { continue; }
            let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
            if foreign[((h * 24.0) as usize).min(23)] { hit += w; }
        }
        hit / total.max(1e-6)
    };
    Some(weighted_foreign(with_px) - weighted_foreign(cur))
}

/// Frame share of RE-HUED pixels: a MEASURABLE hue before (chroma ≥
/// [`ROT_HUE_MEASURABLE_CHROMA`]), a visible tint after (chroma ≥
/// [`VETO_TINT_CHROMA`]), landing ≥ [`ROT_DEG`] of circular hue away — and
/// VISIBLE on at least one end (see [`ROT_VISIBLE_BEFORE`]): a sub-visible
/// tint flipped into another sub-visible tint is cast-inversion
/// pass-through, not a re-hue. Pixel-aligned: `cur`/`with_px` render the
/// SAME source, so per-pixel hue movement is exact. De-tinting (end chroma
/// under the gate) is exempt — removing colour is what a corrective cast
/// does. Exposed separately from the boolean gate so the pin test measures
/// the same census the gate uses.
#[cfg(test)]
fn rehued_share(cur: &[[f32; 3]], with_px: &[[f32; 3]]) -> f32 {
    let mut cnt = 0usize;
    for (c, w) in cur.iter().zip(with_px) {
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = w[0].max(w[1]).max(w[2]) - w[0].min(w[1]).min(w[2]);
        if cc < ROT_HUE_MEASURABLE_CHROMA || wc < VETO_TINT_CHROMA {
            continue;
        }
        if cc < ROT_VISIBLE_BEFORE && wc < ROT_VISIBLE_AFTER {
            continue; // invisible on both ends: pass-through, not a re-hue
        }
        let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0 * 360.0;
        let h1 = render::rgb_to_hsl(w[0], w[1], w[2]).0 * 360.0;
        let mut d = (h1 - h0).abs() % 360.0;
        if d > 180.0 {
            d = 360.0 - d;
        }
        if d >= ROT_DEG {
            cnt += 1;
        }
    }
    cnt as f32 / cur.len().max(1) as f32
}

/// Did the curves re-hue a REGION ([`ROT_SHARE`] of the frame)? See
/// [`rehued_share`] and the rotation-budget const block.
#[cfg(test)]
fn cast_rotates_a_region(cur: &[[f32; 3]], with_px: &[[f32; 3]]) -> bool {
    rehued_share(cur, with_px) >= ROT_SHARE
}

fn cast_rotates_a_region_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    // Deliberately NO paired-convergence exemption here (unlike the
    // zero-evidence-band guard): rotating a region is a TOOL-CAPABILITY
    // policy, not a measurability question — even a rotation that converges
    // on the analysis raster is the HUE axis's job (which nothing solves:
    // stage 4a fits saturation and luminance only), and a global cast that
    // performs it drags every same-hue pixel the raster never sampled
    // (golden-sky case, pinned).
    rehued_share_weighted(cur, with_px, evidence) >= ROT_SHARE
}

/// WB-specific foreign-hue check. A source-only hue can already be foreign in
/// the target, so a frame-share delta alone would cancel it out; count only
/// pixels whose WB render both moves substantially and lands in that foreign
/// hue population.
fn wb_moves_pixels_into_foreign_hues(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
) -> bool {
    let Some(foreign) = foreign_hue_bins(tp) else { return false };
    let mut moved = 0usize;
    for (before, after) in cur.iter().zip(with_px) {
        let before_chroma = before[0].max(before[1]).max(before[2])
            - before[0].min(before[1]).min(before[2]);
        let after_chroma = after[0].max(after[1]).max(after[2])
            - after[0].min(after[1]).min(after[2]);
        if before_chroma < ROT_HUE_MEASURABLE_CHROMA || after_chroma < VETO_TINT_CHROMA {
            continue;
        }
        let (h0, _, _) = render::rgb_to_hsl(before[0], before[1], before[2]);
        let (h1, _, _) = render::rgb_to_hsl(after[0], after[1], after[2]);
        let mut delta = (h1 - h0).abs() * 360.0;
        if delta > 180.0 { delta = 360.0 - delta; }
        let after_bin = ((h1 * 24.0) as usize).min(23);
        if delta >= ROT_DEG && foreign[after_bin] {
            moved += 1;
        }
    }
    moved as f32 / cur.len().max(1) as f32 >= VETO_CREATED_SHARE
}

/// Weighted share of the source population visibly re-hued by a transform.
/// This is the exact census used by the weighted rotation gate and by the
/// strength-gated white-balance guard.
fn rehued_share_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> f32 {
    let mut hit = 0.0f32;
    let mut total = 0.0f32;
    for (i, (c, wpx)) in cur.iter().zip(with_px).enumerate() {
        let weight = evidence.source_hue_weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if weight <= 0.0 { continue; }
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = wpx[0].max(wpx[1]).max(wpx[2]) - wpx[0].min(wpx[1]).min(wpx[2]);
        total += weight;
        if cc >= ROT_HUE_MEASURABLE_CHROMA && wc >= VETO_TINT_CHROMA {
            let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0 * 360.0;
            let h1 = render::rgb_to_hsl(wpx[0], wpx[1], wpx[2]).0 * 360.0;
            let mut d = (h1 - h0).abs() % 360.0;
            if d > 180.0 { d = 360.0 - d; }
            if d >= ROT_DEG { hit += weight; }
        }
    }
    hit / total.max(1e-6)
}

/// The worst hue CLASS the curves FANNED APART across luminance, as (that
/// class's share of the weighted population, added spread in degrees).
/// Exposed separately from the gate so the pin tests read the same census
/// the gate uses.
///
/// The census population is EXACTLY [`rehued_share_weighted`]'s — a
/// measurable hue before ([`ROT_HUE_MEASURABLE_CHROMA`]), a visible tint
/// after ([`VETO_TINT_CHROMA`]), evidence-weighted — so the two gates judge
/// the same pixels and can never be retuned into disagreeing about WHICH
/// population is being read. The QUESTION is the different one: not "how far
/// did each pixel travel" but "did one hue class arrive at several different
/// hues, sorted by luminance".
///
/// Slices are the evidence model's own luma bins, and a class's verdict is
/// the widest circular gap between the mean hues its populated slices land
/// on, MINUS the gap they started from — so a class that was ALREADY fanned
/// (content, not the curves' doing) contributes nothing, and a class the
/// curves rotate rigidly (a real global cast correction: every slice moves
/// together) reads zero however far it moves. That subtraction is what makes
/// this a capability gate rather than a second rotation budget.
///
/// A CLASS IS A HUE POPULATION, NOT A REGION. Cornwall's convicted class
/// holds 0.917 of the census population — that is the seascape's whole blue
/// class, sky AND sea, which the curves sort by luminance together; the
/// row-defined sky alone carries 0.561 of the hue weight. Prose that calls
/// the 0.917 "the sky" is naming the wrong population.
///
/// Returns, for the class the curves ADD the most spread to:
/// `(share, added, delivered)` — its share of the census population, the
/// spread the curves add to it (SIGNED: negative means they narrowed it),
/// and the ABSOLUTE spread it carries after the curves. The gate judges
/// `added`; `delivered` is what a viewer sees, and the two differ by the
/// baseline, which is bounded by one class width (see [`FAN_DEG`]'s worst
/// case).
fn hue_fan_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(f32, f32, f32)> {
    let mut mass = [[0.0f32; EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut before = [[(0.0f32, 0.0f32); EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut after = [[(0.0f32, 0.0f32); EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut population = 0.0f32;
    for (i, (c, w)) in cur.iter().zip(with_px).enumerate() {
        let weight = evidence.source_hue_weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if weight <= 0.0 { continue; }
        population += weight;
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = w[0].max(w[1]).max(w[2]) - w[0].min(w[1]).min(w[2]);
        if cc < ROT_HUE_MEASURABLE_CHROMA || wc < VETO_TINT_CHROMA { continue; }
        let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0;
        let h1 = render::rgb_to_hsl(w[0], w[1], w[2]).0;
        let class = ((h0 * FAN_HUE_CLASSES as f32) as usize).min(FAN_HUE_CLASSES - 1);
        let bin = evidence_luma_bin(luma601(c));
        mass[class][bin] += weight;
        let (a0, a1) = (h0 * std::f32::consts::TAU, h1 * std::f32::consts::TAU);
        before[class][bin].0 += a0.sin() * weight;
        before[class][bin].1 += a0.cos() * weight;
        after[class][bin].0 += a1.sin() * weight;
        after[class][bin].1 += a1.cos() * weight;
    }
    if population <= 0.0 {
        return None;
    }
    // Widest circular gap between the slice means. Pairwise because the set
    // is at most EVIDENCE_LUMA_BINS long and a circular "range" has no
    // cheaper honest definition.
    let spread = |values: &[f32]| -> f32 {
        let mut worst = 0.0f32;
        for (i, a) in values.iter().enumerate() {
            for b in &values[i + 1..] {
                let mut d = (b - a).abs() % 360.0;
                if d > 180.0 { d = 360.0 - d; }
                worst = worst.max(d);
            }
        }
        worst
    };
    let mean = |(sin, cos): (f32, f32)| sin.atan2(cos).to_degrees().rem_euclid(360.0);
    let mut worst: Option<(f32, f32, f32)> = None;
    for class in 0..FAN_HUE_CLASSES {
        let class_mass: f32 = mass[class].iter().sum();
        let share = class_mass / population;
        if share < FAN_SHARE { continue; }
        let (mut was, mut now) = (Vec::new(), Vec::new());
        for bin in 0..EVIDENCE_LUMA_BINS {
            if mass[class][bin] < class_mass * FAN_SHARE { continue; }
            was.push(mean(before[class][bin]));
            now.push(mean(after[class][bin]));
        }
        // One slice cannot fan: a class confined to a single luma bin has no
        // internal structure for the curves to sort.
        if was.len() < 2 { continue; }
        let delivered = spread(&now);
        let fan = delivered - spread(&was);
        if worst.is_none_or(|(_, seen, _)| fan > seen) {
            worst = Some((share, fan, delivered));
        }
    }
    worst
}

/// Does this FINISHED render sort a hue class apart across luminance past
/// [`FAN_DEG`], measured against the untouched base with the fan gate's own
/// census? `Some((share, fan))` when it does, `None` when it does not or when
/// the census abstains.
///
/// The gate one stage up judges a CANDIDATE — the cast curves against the
/// state they were fitted on — and it is a calibrated threshold applied at
/// one point in the pipeline. This reads the same census on the pair the user
/// actually sees: the recipe that is about to be handed back, against doing
/// nothing at all.
fn delivered_fan_conviction(
    sp: &[[f32; 3]],
    end_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(f32, f32)> {
    hue_fan_weighted(sp, end_px, evidence)
        .filter(|&(_, fan, _)| fan > FAN_DEG)
        .map(|(share, fan, _)| (share, fan))
}

/// The TERMINAL delivered-fan check, and the structural half of a promise
/// v1.2.3 could only make as a calibration.
///
/// The fan gate convicts the cast stage's own candidate, and the do-no-harm
/// loop above re-fits that stage after every saturation step — so nothing in
/// the loop's arithmetic stops it walking to a state whose FINISHED render
/// fans a class past [`FAN_DEG`], one admissible step at a time. The FAN_DEG
/// = 20 experiment is that behaviour caught in the act: the loop halved
/// Aqua/Blue and refitted until a milder cast measured 19°, which shipped and
/// left 20.6° of fan in the delivered sky. A threshold nothing re-reads at
/// the end is a threshold the pipeline can walk around.
///
/// The remedy is the gate's own, and it is aimed at the gate's own subject.
/// Three independent monotone channel maps are the control the fan gate
/// exists for and the one the loop keeps re-fitting, so they are withdrawn
/// and the frame re-measured — the same "shrink until the finished frame
/// stops objecting" idiom the mixer and the saturation loop already use,
/// taken to its end in one step because curves have no smaller unit than
/// "present". If the reading clears, that recipe ships with the sentence
/// that says so.
///
/// If the reading survives the withdrawal the curves were NOT the cause, and
/// then the honest act is to say so rather than to keep taking the recipe
/// apart: the curves go back exactly as the loop left them (so a fit pays no
/// look error for a fan it did not open) and the delivered reading is
/// disclosed with both numbers. That case is real rather than defensive —
/// measured 2026-09-02, the `p36` calibration pair delivers 12.9° of added
/// fan carrying NO cast curves at all, so tone and saturation alone reach
/// most of the way to the line — and it is the reason this check withdraws
/// one named control instead of degrading the whole fit.
///
/// Returns `(share, fan, still)` when the delivered render is convicted: the
/// class's share, the fan it had, and the fan that survived withdrawing the
/// curves — `None` there means the withdrawal cleared it and the
/// curve-less recipe is what ships.
///
/// MEASURED 2026-09-02 by instrumenting this point and running the whole
/// library battery: 108 finished Full-mode renders, the widest delivered fan
/// among them 14.2° in a class holding 0.638 of the measurable colour (the
/// `coast` fixture, whose cast is projected), then `p36`'s 12.9° and the
/// two-temperature `p40` pair's 12.4°. So at [`FAN_DEG`] this check fires on
/// nothing in the tree — it costs one census per fit and changes no recipe —
/// which is what a structural guarantee looks like while the calibration
/// above it is doing its job. Its two arms are pinned by
/// `the_terminal_check_takes_the_curves_out_of_a_fanning_render` and
/// `a_delivered_fan_the_curves_did_not_cause_is_disclosed_not_withdrawn`, and
/// the standing margin by `no_shipped_fit_delivers_a_hue_fan_past_the_limit`.
fn withdraw_curves_for_delivered_fan(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    evidence: &EvidenceModel,
    recipe: &mut EditRecipe,
    end_px: &mut Vec<[f32; 3]>,
) -> Option<(f32, f32, Option<f32>)> {
    let (share, fan) = delivered_fan_conviction(sp, end_px, evidence)?;
    let mut without = recipe.clone();
    without.red_curve.clear();
    without.green_curve.clear();
    without.blue_curve.clear();
    let px = pixels_of(&render::develop_preview(s_img, &without));
    let still = delivered_fan_conviction(sp, &px, evidence).map(|(_, fan)| fan);
    if still.is_none() {
        *recipe = without;
        *end_px = px;
    }
    Some((share, fan, still))
}

fn rehued_coverage_weighted(evidence: &EvidenceModel) -> f32 {
    evidence
        .source_hue_weights
        .iter()
        .copied()
        .map(|weight| weight.max(0.0))
        .sum::<f32>()
        / evidence.source_pixels.len().max(1) as f32
}

pub(crate) fn moves_unsupported_range(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    moved_unsupported_range_names(cur, with_px, evidence).is_some()
}

pub(crate) fn moved_unsupported_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(String, String)> {
    let hits = moved_unsupported_range_hits(cur, with_px, evidence, None);
    let (luma, hue) = (hits.luma, hits.hue);
    if luma.0 == 0.0 && hue.0 == 0.0 { return None; }
    let names = |hits: &[bool], ranges: &[EvidenceRange]| {
        hits.iter().zip(ranges).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", ")
    };
    Some((names(&luma.1, &evidence.luma), names(&hue.1, &evidence.hue)))
}

pub(crate) fn moved_unsupported_luma_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<String> {
    let luma = moved_unsupported_range_hits(cur, with_px, evidence, None).luma;
    if luma.0 == 0.0 { return None; }
    Some(luma.1.iter().zip(&evidence.luma).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", "))
}

pub(crate) fn moved_unsupported_hue_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<String> {
    moved_unsupported_hue_range_names_vouched(cur, with_px, evidence, None)
}

/// [`moved_unsupported_hue_range_names`] with a per-pixel robust voucher for
/// the evacuation exemption (see `moved_unsupported_range_hits`). Only the
/// global END-STATE guard supplies one; every other caller keeps the strict
/// doctrine.
pub(crate) fn moved_unsupported_hue_range_names_vouched(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<(&[f32], &[[f32; 3]])>,
) -> Option<String> {
    let hue = moved_unsupported_range_hits(cur, with_px, evidence, vouch).hue;
    if hue.0 == 0.0 { return None; }
    Some(hue.1.iter().zip(&evidence.hue).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", "))
}

fn moved_unsupported_range_hits(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<(&[f32], &[[f32; 3]])>,
) -> MovedRangeHits {
    let mut moved_luma = 0.0f32;
    let mut moved_hue = 0.0f32;
    let mut luma = [false; EVIDENCE_LUMA_BINS];
    let mut hue = [false; EVIDENCE_HUE_BANDS];
    let mut vouched_hue = [false; EVIDENCE_HUE_BANDS];
    for (i, (before, after)) in cur.iter().zip(with_px).enumerate() {
        let unsupported_luma = source_luma_is_withheld(i, evidence);
        let unsupported_hue = source_hue_is_withheld(i, evidence);
        let delta = (0..3).map(|channel| (after[channel] - before[channel]).abs()).sum::<f32>()
            / 3.0;
        if delta < UNSUPPORTED_RANGE_MOVE { continue; }
        let membership = evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0);
        if membership <= 0.0 { continue; }
        if unsupported_luma && let Some(pixel) = evidence.source_pixels.get(i) {
            moved_luma += membership;
            luma[evidence_luma_bin(luma601(pixel))] = true;
        }
        if unsupported_hue && let Some(pixel) = evidence.source_pixels.get(i) && let Some(band) = evidence_hue_band(pixel) {
            // VOUCHED convergence (the hue form of the luma rank-pairing
            // doctrine, gated by the robust fit): on a paired run, a pixel
            // whose transport residual the robust fit vouches for has its OWN
            // paired target pixel — moving it TOWARD that target is
            // convergence, not a blind move through an unmeasurable band
            // (measured: the haze pair's blue-cast pixels sat in source-only
            // Red/Blue, and un-casting them was vetoed by the very bands the
            // cast invented; a band-topology exemption then failed the same
            // pair again on pixels the cast's step caps left mid-way). A
            // content-divergent pixel (the canyon pair's vanished reds are
            // the reconstruction's doing, not an edit's) carries a large
            // transport residual, earns no voucher, and keeps its veto.
            // Callers with no paired verdict pass None and get the strict
            // doctrine unchanged — the zoned colour probes do so
            // deliberately (zone colour stays class-split withheld, the
            // user-ratified rule).
            let spatially = evidence.spatial_supported.get(i).copied().unwrap_or(false);
            let converges = spatially
                && vouch.is_some_and(|(weights, targets)| {
                    weights.get(i).copied().unwrap_or(0.0) >= 0.5
                        && targets
                            .get(i)
                            .is_some_and(|target| converges_toward(target, before, after))
                });
            if converges {
                vouched_hue[band] = true;
            } else {
                moved_hue += membership;
                hue[band] = true;
            }
        }
    }
    // The region line is a share of the population the correction moves --
    // the frame for the global fit, a zone's coverage for a zone (a frame
    // share let every tile move its blind pixels: a depth-2 tile is 6% of
    // the frame, so its blind half never reached the 5% line).
    let total = evidence.population.max(1.0);
    if moved_luma / total < ROT_SHARE { moved_luma = 0.0; luma = [false; EVIDENCE_LUMA_BINS]; }
    if moved_hue / total < ROT_SHARE { moved_hue = 0.0; hue = [false; EVIDENCE_HUE_BANDS]; }
    MovedRangeHits { luma: (moved_luma, luma), hue: (moved_hue, hue), vouched_hue }
}

/// The per-range verdicts of one blind-move audit: which withheld ranges were
/// MOVED (the veto's subject) and which one-sided hue bands vouched
/// convergence carried movement THROUGH — the second list exists so the
/// disclosure can stop claiming "vetoed movement" about bands whose veto the
/// voucher lifted (E-15: two different outcomes must not read the same).
struct MovedRangeHits {
    luma: (f32, [bool; EVIDENCE_LUMA_BINS]),
    hue: (f32, [bool; EVIDENCE_HUE_BANDS]),
    vouched_hue: [bool; EVIDENCE_HUE_BANDS],
}

/// Names of the one-sided hue bands that vouched convergence moved pixels
/// through on the finished render — the disclosure's raw material.
pub(crate) fn vouched_hue_band_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<(&[f32], &[[f32; 3]])>,
) -> Option<String> {
    let hits = moved_unsupported_range_hits(cur, with_px, evidence, vouch);
    let names = hits
        .vouched_hue
        .iter()
        .zip(&evidence.hue)
        .filter_map(|(&hit, range)| hit.then_some(range.label.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    (!names.is_empty()).then_some(names)
}

fn moves_unsupported_luma_range(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    let mut moved = 0.0f32;
    let mut eligible = 0.0f32;
    for (i, (before, after)) in cur.iter().zip(with_px).enumerate() {
        if !source_luma_is_withheld(i, evidence) {
            continue;
        }
        let membership = evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0);
        if membership <= 0.0 {
            continue;
        }
        eligible += membership;
        let before_luma = luma601(before);
        let after_luma = luma601(after);
        if (after_luma - before_luma).abs() >= UNSUPPORTED_RANGE_MOVE {
            moved += membership;
        }
    }
    eligible > 0.0 && moved / evidence.population.max(1.0) >= ROT_SHARE
}

// --------------------------------------------------------------------------
// statistics primitives
// --------------------------------------------------------------------------

pub fn pixels_of(img: &DynamicImage) -> Vec<[f32; 3]> {
    img.to_rgb8()
        .pixels()
        .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
        .collect()
}

pub fn luma601(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

#[cfg(test)]
fn cdf_from_values(values: impl Iterator<Item = f32>, n_hint: usize) -> Vec<f32> {
    let mut hist = vec![0.0f32; HIST_BINS];
    let mut n = 0usize;
    for v in values {
        let i = ((v.clamp(0.0, 1.0)) * (HIST_BINS - 1) as f32).round() as usize;
        hist[i] += 1.0;
        n += 1;
    }
    let total = (n.max(n_hint.min(1)) as f32).max(1.0);
    let mut acc = 0.0f32;
    for h in hist.iter_mut() {
        acc += *h;
        *h = acc / total;
    }
    hist
}

#[cfg(test)]
fn luma_cdf(px: &[[f32; 3]]) -> Vec<f32> {
    cdf_from_values(px.iter().map(luma601), px.len())
}

/// Near-neutral gate for the TONE evidence (the cast catch-all fits on
/// ungated per-channel CDFs — see `residual_channel_curve`). Gated on HSV
/// saturation ((max−min)/max), which is INVARIANT under pure luminance
/// scaling — so the same pixels qualify in the source and in its tone-mapped
/// target (an absolute-chroma gate is not: dark colours slip under it in the
/// source and leave it once brightened, skewing the two CDFs against each
/// other). Near-black counts as neutral.
fn is_neutralish(p: &[f32; 3]) -> bool {
    let mx = p[0].max(p[1]).max(p[2]);
    let mn = p[0].min(p[1]).min(p[2]);
    mx < 0.04 || (mx - mn) / mx < 0.15
}

/// Tone-evidence CDF pair. Near-neutral gating only carries clean evidence
/// when the SAME population is neutral on BOTH sides — the tone map is
/// quantile-to-quantile, so the gate is an identification assumption about
/// pixel correspondence, not a per-image preference. Three observed
/// breakages: a side's neutral sample is too small (< 5% or < 512 px —
/// noise); the neutral SHARES diverge, meaning the target re-hued (or
/// de-hued) part of the population — golden-sky pair, 2026-07-09: the
/// source's pale sky is neutralish ((max−min)/max ≈ 0.12), the target's
/// vivid gold one is not (≈ 0.37), and an asymmetric gate mapped the sky's
/// luma cluster across a ramp it doesn't belong to, distorting the whole
/// tone solve; or the shares stay COMPARABLE while a luma-CONCENTRATED band
/// churns out of one side's class — P20 × reimagine, 2026-08-12: the
/// target re-hued 24% of the base's neutral class (the pale sky, base-luma
/// q50 ≈ 197/255, → vivid blue), the share ratio read a passing 1.29×, and
/// the base's bright grey ranks paired against target ranks the sky no
/// longer belongs to — every upper-mid darkened and the render shipped
/// murky. Shares are a SIZE proxy, blind to composition (and the haze pair
/// proves one-sided CDF-shift proxies rank harm no better), so the gate is
/// judged by the harm itself: [`neutral_gate_misprediction`] scores the
/// gated evidence map against the shared class's own observable pairing.
/// Either way the assumption is dead: fall back to full-pixel CDFs on BOTH
/// sides (deciding per side, as the original code did, can even compare a
/// neutral-gated CDF against a full one). 1.75× keeps the
/// matched-population regressions (identity / roundtrip / violet canyon
/// ≈ 1.0×) while catching the golden-sky asymmetry (2.0×); the
/// misprediction ceiling is anchored in [`NEUTRAL_MISPREDICTION_MAX`]'s
/// doc. The two detectors are COMPLEMENTARY, not redundant (R18, measured
/// on the archive): the share ratio is alignment-free — it still works
/// when misregistration slides the misprediction metric toward 0 (its
/// fail-open direction) — while the misprediction gate catches membership
/// churn the ratio cannot see (P21 × reimagine-4: share 1.51×,
/// misprediction 0.034 — only this gate fires). A >1.75× share asymmetry
/// falls back UNCONDITIONALLY, by design: a low misprediction reading must
/// never override it, because misregistration fakes exactly that reading
/// (the fail-open direction), and the fallback itself is the safe arm —
/// on the two live pairs whose evidence failed a gate, the fallback solve
/// measured better than the gated one both times (a benign uniform >1.75×
/// inflation remains synthetic-only; no real pair has produced one). Gate order: cheap counts and shares first, the
/// misprediction pass (a full-frame scan plus four CDFs) last, so
/// under-evidenced pairs never pay for it.
#[cfg(test)]
fn tone_cdf_pair(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> (Vec<f32>, Vec<f32>) {
    let s_n: Vec<f32> = sp.iter().filter(|p| is_neutralish(p)).map(luma601).collect();
    let t_n: Vec<f32> = tp.iter().filter(|p| is_neutralish(p)).map(luma601).collect();
    let share_s = s_n.len() as f32 / sp.len().max(1) as f32;
    let share_t = t_n.len() as f32 / tp.len().max(1) as f32;
    let gated = enough_evidence(s_n.len(), sp.len())
        && enough_evidence(t_n.len(), tp.len())
        && share_s.max(share_t) <= 1.75 * share_s.min(share_t)
        && neutral_gate_misprediction(sp, tp) <= NEUTRAL_MISPREDICTION_MAX;
    if gated {
        let (ns, nt) = (s_n.len(), t_n.len());
        (cdf_from_values(s_n.into_iter(), ns), cdf_from_values(t_n.into_iter(), nt))
    } else {
        (luma_cdf(sp), luma_cdf(tp))
    }
}

fn tone_cdf_pair_weighted(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> (Vec<f32>, Vec<f32>) {
    let s_n: Vec<(f32, f32)> = sp
        .iter()
        .enumerate()
        .filter(|(_, p)| is_neutralish(p))
        .filter_map(|(i, p)| {
            let w = evidence.source_weights.get(i).copied().unwrap_or(0.0);
            (w > 0.0).then_some((luma601(p), w))
        })
        .collect();
    let t_n: Vec<(f32, f32)> = tp
        .iter()
        .enumerate()
        .filter(|(_, p)| is_neutralish(p))
        .filter_map(|(i, p)| {
            let w = evidence.target_weights.get(i).copied().unwrap_or(0.0);
            (w > 0.0).then_some((luma601(p), w))
        })
        .collect();
    let weighted = |v: &[(f32, f32)]| {
        let mut hist = vec![0.0f32; HIST_BINS];
        let total = v.iter().map(|(_, w)| *w).sum::<f32>();
        if total <= 1e-8 { return hist; }
        for &(x, w) in v {
            hist[(x.clamp(0.0, 1.0) * (HIST_BINS - 1) as f32).round() as usize] += w;
        }
        let mut acc = 0.0;
        for h in &mut hist { acc += *h; *h = acc / total; }
        hist
    };
    // The SAME identification gates the unweighted twin (`tone_cdf_pair`)
    // documents: the neutral gate only carries clean evidence when the same
    // population is neutral on BOTH sides. This arm shipped without them, and
    // the p36 calibration pair showed the cost live: the source's neutral
    // class is its dark rock, the target's is its bright sky, and the gated
    // quantile map sent 0.05 → 0.77 — a map the slider solve then pegged
    // itself against. Population parity is judged on the pixels that carry
    // evidence weight, floor and ratio exactly as the twin's contract states.
    let s_total = evidence.source_weights.iter().filter(|&&w| w > 0.0).count();
    let t_total = evidence.target_weights.iter().filter(|&&w| w > 0.0).count();
    let share_s = s_n.len() as f32 / s_total.max(1) as f32;
    let share_t = t_n.len() as f32 / t_total.max(1) as f32;
    let gated = enough_evidence(s_n.len(), s_total)
        && enough_evidence(t_n.len(), t_total)
        && share_s.max(share_t) <= 1.75 * share_s.min(share_t);
    let s = if gated { weighted(&s_n) } else { Vec::new() };
    let t = if gated { weighted(&t_n) } else { Vec::new() };
    if s.iter().any(|&v| v > 0.0) && t.iter().any(|&v| v > 0.0) {
        (s, t)
    } else {
        (weighted_cdf(sp, &evidence.source_weights, luma601), weighted_cdf(tp, &evidence.target_weights, luma601))
    }
}

/// The tone-evidence sample floor: at least 5% of the frame and never fewer
/// than 512 px. Shared between the per-side gate and the shared-class floor
/// inside [`neutral_gate_misprediction`], so "enough to trust" means one
/// thing.
fn enough_evidence(n: usize, total: usize) -> bool {
    n >= (total / 20).max(NEUTRAL_SHARED_MIN)
}

/// How badly the gated tone evidence MISPREDICTS the population it claims
/// to identify. The SHARED class — pixels neutral at the same position on
/// both sides — is the one population whose (source-luma, target-luma)
/// pairing is observable without any modelling: under a monotone tone map,
/// its own quantiles ARE the map. So build the production evidence map
/// exactly as the tone solve would (each side's whole neutral class,
/// quantile-paired) and score it against the shared class's empirical map:
/// mean |Δ| over the 21 look_err quantiles. Asymmetric members that merely
/// inflate a class along the shared luma ramp leave the pairing intact
/// (the synthetic uniform-inflation fixture reads < 0.0075); members that
/// churn in a luma-concentrated band bend the ranks and the misprediction
/// shows it directly — this is the murk, measured at its source.
///
/// POSITIONAL-CORRESPONDENCE ASSUMPTION, stated plainly because the rest of
/// this module deliberately avoids one (a generative target is not
/// pixel-aligned): co-membership at equal row-major index is read as "same
/// coarse region", which holds for same-frame pairs on the shared 384-edge
/// thumbnail grid and degrades with misregistration. The failure direction
/// is OPEN: under broken alignment target-membership decorrelates from
/// source-membership, the shared class becomes an unbiased thinning of both
/// sides, and the metric slides toward 0 — the gate is KEPT, not dropped,
/// and this detector goes vacuous (its sensitivity is proportional to
/// registration quality; the older share/size gates still stand in front
/// of it). Grids that disagree by more than aspect rounding (~a row) are
/// not comparable at all — that case returns infinite (fall back) as an
/// explicit decision rather than an emergent prefix artifact, and so does
/// a shared class too small to clear the same evidence floor the sides
/// must clear.
#[cfg(test)]
pub(crate) fn neutral_gate_misprediction(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> f32 {
    let n = sp.len().min(tp.len());
    // 2% ≈ several rows of a 384-edge thumb: beyond aspect rounding, the
    // pairs come from different geometry and co-membership is meaningless.
    if sp.len().abs_diff(tp.len()) > n / 50 {
        return f32::INFINITY;
    }
    let (mut s_all, mut t_all) = (Vec::with_capacity(n / 2), Vec::with_capacity(n / 2));
    let (mut sh_s, mut sh_t) = (Vec::with_capacity(n / 2), Vec::with_capacity(n / 2));
    for i in 0..n {
        let (a, b) = (is_neutralish(&sp[i]), is_neutralish(&tp[i]));
        if a {
            s_all.push(luma601(&sp[i]));
        }
        if b {
            t_all.push(luma601(&tp[i]));
        }
        if a && b {
            sh_s.push(luma601(&sp[i]));
            sh_t.push(luma601(&tp[i]));
        }
    }
    if !enough_evidence(sh_s.len(), n) {
        return f32::INFINITY;
    }
    let (ns, nt, nsh) = (s_all.len(), t_all.len(), sh_s.len());
    let s_cdf = cdf_from_values(s_all.into_iter(), ns);
    let t_cdf = cdf_from_values(t_all.into_iter(), nt);
    let sh_s_cdf = cdf_from_values(sh_s.into_iter(), nsh);
    let sh_t_cdf = cdf_from_values(sh_t.into_iter(), nsh);
    let mut acc = 0.0f32;
    let mut cnt = 0.0f32;
    for i in 0..=20 {
        let p = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        let x = quantile(&sh_s_cdf, p);
        // The same formula the tone solve uses for its map (see `tone_map`).
        let predicted = quantile(&t_cdf, cdf_at(&s_cdf, x).clamp(P_CLIP, 1.0 - P_CLIP));
        let actual = quantile(&sh_t_cdf, p);
        acc += (predicted - actual).abs();
        cnt += 1.0;
    }
    acc / cnt
}

/// F(x): fraction of pixels ≤ x (linear interp between bins).
pub(crate) fn cdf_at(cdf: &[f32], x: f32) -> f32 {
    let pos = x.clamp(0.0, 1.0) * (cdf.len() - 1) as f32;
    let i = pos.floor() as usize;
    if i >= cdf.len() - 1 {
        return cdf[cdf.len() - 1];
    }
    let t = pos - i as f32;
    cdf[i] * (1.0 - t) + cdf[i + 1] * t
}

/// Q(p): the value at quantile `p` (inverse CDF, linear interp within the bin).
pub(crate) fn quantile(cdf: &[f32], p: f32) -> f32 {
    let n = cdf.len();
    let mut lo = 0usize;
    let mut hi = n - 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cdf[mid] < p {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // Interpolate within the step from the previous bin for a smooth inverse.
    if lo == 0 {
        return 0.0;
    }
    let (c0, c1) = (cdf[lo - 1], cdf[lo]);
    let t = if c1 > c0 { ((p - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 1.0 };
    ((lo - 1) as f32 + t) / (n - 1) as f32
}

/// Variance of the per-pixel channel mean — the degenerate-input probe
/// (see the refusal at the top of [`fit_recipe`]). Zero for an empty slice.
const DEGENERATE_LUMA_VAR: f32 = 1e-6;
fn luma_variance(px: &[[f32; 3]]) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    let n = px.len() as f32;
    let lum = |p: &[f32; 3]| (p[0] + p[1] + p[2]) / 3.0;
    let mean: f32 = px.iter().map(lum).sum::<f32>() / n;
    px.iter().map(|p| (lum(p) - mean).powi(2)).sum::<f32>() / n
}

fn mean_chroma(px: &[[f32; 3]]) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    let sum: f32 =
        px.iter().map(|p| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])).sum();
    sum / px.len() as f32
}

/// Conservative detail budget: one fifth of each rendered control's range.
/// The +100 texture calibration recovers useful frequency energy at this cap
/// without giving a marginal statistic permission to drive a full-strength
/// microcontrast move.
const DETAIL_CONTROL_LIMIT: f32 = 20.0;

/// Coarse/fine local-luma energy used for the rendered clarity/texture
/// controls.  It is deliberately a source-indexed, evidence-weighted reading
/// so invented or one-sided regions cannot demand detail sliders.
fn detail_energy(px: &[[f32; 3]], weights: &[f32], radius: usize) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    let n = px.len();
    let mut sum = 0.0f32;
    let mut total = 0.0f32;
    for i in 0..n {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if w <= 0.0 {
            continue;
        }
        let j = (i + radius.min(n - 1)).min(n - 1);
        let k = i.saturating_sub(radius.min(i));
        let a = luma601(&px[j]);
        let b = luma601(&px[k]);
        sum += (a - b).abs() * w;
        total += w;
    }
    if total > 1e-8 { sum / total } else { 0.0 }
}

fn fit_detail_controls(
    cur: &[[f32; 3]],
    tgt: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> (f32, f32) {
    let coarse = detail_energy(cur, &evidence.source_weights, 4).max(1e-5);
    let fine = detail_energy(cur, &evidence.source_weights, 1).max(1e-5);
    let target_coarse = detail_energy(tgt, &evidence.target_weights, 4);
    let target_fine = detail_energy(tgt, &evidence.target_weights, 1);
    let clarity_ratio = target_coarse / coarse;
    let fine_ratio = target_fine / fine;
    let clarity = ((clarity_ratio - 1.0) * 100.0)
        .clamp(-DETAIL_CONTROL_LIMIT, DETAIL_CONTROL_LIMIT)
        .round();
    let texture = ((fine_ratio - 1.0) * 100.0)
        .clamp(-DETAIL_CONTROL_LIMIT, DETAIL_CONTROL_LIMIT)
        .round();
    (
        if (clarity_ratio - 1.0).abs() < 0.20 { 0.0 } else { clarity },
        if (fine_ratio - 1.0).abs() < 0.20 { 0.0 } else { texture },
    )
}

fn detail_evidence_supported(evidence: &EvidenceModel) -> bool {
    evidence.identifiability >= DETAIL_EVIDENCE_MIN_IDENTIFIABILITY
        && evidence.luma.iter().filter(|range| range.weight > 0.0).count() >= 6
}

fn detail_residual(px: &[[f32; 3]], target: &[[f32; 3]], evidence: &EvidenceModel) -> f32 {
    let coarse = detail_energy(px, &evidence.source_weights, 4);
    let fine = detail_energy(px, &evidence.source_weights, 1);
    let target_coarse = detail_energy(target, &evidence.target_weights, 4);
    let target_fine = detail_energy(target, &evidence.target_weights, 1);
    (coarse - target_coarse).abs() + (fine - target_fine).abs()
}

fn detail_regression_is_bounded(
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    target: &[[f32; 3]],
    evidence: &EvidenceModel,
    detail: (f32, f32),
    err_before: f32,
    err_after: f32,
) -> bool {
    (detail.0 != 0.0 || detail.1 != 0.0)
        && err_after <= err_before + FIT_QUANT
        && detail_residual(after, target, evidence) + 1e-6
            < detail_residual(before, target, evidence)
}

fn only_detail_and_quantized_companions(recipe: &EditRecipe, base: &EditRecipe) -> bool {
    let wb_gains_for = |r: &EditRecipe| {
        if r.temperature_k.is_some() || r.tint != 0.0 {
            let anchor = r.as_shot_k.unwrap_or(5500.0);
            render::wb_gains(anchor, r.temperature_k.unwrap_or(anchor), r.tint)
        } else {
            [1.0; 3]
        }
    };
    let recipe_wb = wb_gains_for(recipe);
    let base_wb = wb_gains_for(base);
    let recipe_tone = render::curve_lut(&recipe.tone_curve);
    let base_tone = render::curve_lut(&base.tone_curve);
    (recipe.exposure_ev - base.exposure_ev).abs() <= 0.02
        && recipe.contrast == base.contrast
        && recipe.highlights == base.highlights
        && recipe.shadows == base.shadows
        && recipe.whites == base.whites
        && recipe.blacks == base.blacks
        && recipe_wb
            .iter()
            .zip(base_wb)
            .all(|(&candidate, baseline)| (candidate - baseline).abs() < 1e-3)
        && recipe.saturation == base.saturation
        && recipe_tone
            .iter()
            .zip(base_tone)
            .all(|(&candidate, baseline)| (candidate - baseline).abs() <= 1.0 / 255.0)
        && recipe.red_curve == base.red_curve
        && recipe.green_curve == base.green_curve
        && recipe.blue_curve == base.blue_curve
}

fn fit_detail_stage(
    source: &DynamicImage,
    target: &[[f32; 3]],
    evidence: &EvidenceModel,
    recipe: &mut EditRecipe,
) -> ((f32, f32), bool) {
    if !detail_evidence_supported(evidence) {
        return ((0.0, 0.0), false);
    }
    let before = pixels_of(&render::develop_preview(source, recipe));
    let detail = fit_detail_controls(&before, target, evidence);
    recipe.clarity = detail.0;
    recipe.texture = detail.1;
    let after = pixels_of(&render::develop_preview(source, recipe));
    if moves_unsupported_range(&before, &after, evidence) {
        recipe.clarity = 0.0;
        recipe.texture = 0.0;
        ((0.0, 0.0), false)
    } else {
        (detail, true)
    }
}


/// Slice-only test helper for the production evidence-weighted objective.
/// Production callers build the evidence model with the real analysis
/// dimensions and keep it fixed for the whole fit.
#[cfg(test)]
pub(crate) fn look_err(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    let evidence = evidence_model(a, b);
    look_err_with_evidence(a, b, &evidence)
}

/// Evidence-weighted look error used by every fit decision.  The structural
/// term makes the objective sensitive to spatially wrong pixels; the weighted
/// distribution terms ignore ranges that are one-sided or structurally
/// divergent instead of silently treating them as a match.
pub(crate) fn look_err_with_evidence(
    a: &[[f32; 3]],
    b: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> f32 {
    let (ca, cb) = (
        weighted_cdf(a, &evidence.source_weights, luma601),
        weighted_cdf(b, &evidence.target_weights, luma601),
    );
    if evidence.identifiability <= 1e-5 {
        return 1.0;
    }
    let mut tonal = 0.0f32;
    let mut n = 0.0f32;
    for i in 0..=20 {
        let p = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        tonal += (quantile(&ca, p) - quantile(&cb, p)).abs();
        n += 1.0;
    }
    tonal /= n;
    let colour = (0..3)
        .map(|ch| {
            (weighted_mean(a, &evidence.source_weights, ch).unwrap_or(0.0)
                - weighted_mean(b, &evidence.target_weights, ch).unwrap_or(0.0))
                .abs()
        })
        .sum::<f32>()
        / 3.0;
    let base = 0.55 * tonal + 0.20 * colour;
    // Per-band centroid hue disagreement — the WORST qualifying band, not a
    // weighted mean: one region with wrecked hue ruins a photo no matter how
    // small its area share (a lavender sky over perfect rocks), and an
    // area-weighted mean lets exactly that hide (measured: the violet-sky
    // curves slipped through the cast-acceptance gate on the mean variant).
    // |Δ| saturates at 60° so a fully-wrecked band reads 1.
    let (sa, ta) = band_stats_weighted(a, &evidence.source_hue_weights);
    let (sb, tb) = band_stats_weighted(b, &evidence.target_hue_weights);
    let mut hue = 0.0f32;
    let mut hue_weight = 0.0f32;
    if ta >= 1.0 && tb >= 1.0 {
        for i in 0..8 {
            let (x, y) = (&sa[i], &sb[i]);
            let range_weight = evidence.hue.get(i).map(|r| r.weight).unwrap_or(0.0);
            if range_weight <= 0.0 || x.w / ta < EVIDENCE_MIN_SHARE as f64 || y.w / tb < EVIDENCE_MIN_SHARE as f64 {
                continue;
            }
            let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
            while d > 180.0 {
                d -= 360.0;
            }
            while d < -180.0 {
                d += 360.0;
            }
            hue = hue.max((d.abs().min(60.0) / 60.0) as f32);
            hue_weight = hue_weight.max(range_weight);
        }
    }
    let (w, h) = (evidence.width, evidence.height);
    let structural = if w > 0 && h > 0 && evidence.source_weights.len() == a.len() {
        structure_divergence(a, b, w, h, &evidence.source_weights).d
    } else {
        0.0
    };
    base + 0.15 * hue * hue_weight.min(1.0) + 0.10 * structural.min(1.0)
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}
fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// The OPTIONAL structural-divergence calibration corpus, located exactly the
/// way `scripts/check_docs.py` locates the XMP census (`AUTOSHADE_CENSUS_ROOT`):
/// through an environment variable, never a source literal. The corpus is a
/// photographer's own RAW and its generative rendition, so it cannot live in
/// this public repository — and a machine-specific path baked into a test would
/// publish a home directory, a develop-store id and a photo's filename along
/// with it. With the variable unset the fixtures still assert the SYNTHETIC
/// pairs; only the measured real-pair numbers go unpinned.
///
/// Expected contents, under canonical names so no corpus filename reaches the
/// source either:
/// * `neutral.jpg` — calibration-only render of the source frame,
/// * `target.jpg` — the generated rendition being fitted,
/// * `fitted.recipe.json` — the saved zoned develop of that pair,
/// * `sky-mask.png` — the sky raster that develop references,
/// * `source.arw` — optional; the RAW behind `neutral.jpg`.
#[cfg(test)]
pub(crate) fn calibration_corpus() -> Option<std::path::PathBuf> {
    let dir =
        std::path::PathBuf::from(crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR")?);
    let required = ["neutral.jpg", "target.jpg", "fitted.recipe.json", "sky-mask.png"];
    if !dir.is_dir() || required.iter().any(|name| !dir.join(name).is_file()) {
        eprintln!(
            "SKIPPED calibration test: incomplete corpus at {} (need neutral.jpg, target.jpg, fitted.recipe.json, sky-mask.png)",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    /// L06-1/2: a zero-variance pair (lens-cap frame against itself) must
    /// refuse to fit — the CDF inverse would produce a constant tone map and
    /// err==0 would accept it silently.
    #[test]
    fn a_degenerate_pair_refuses_to_fit_and_says_why() {
        use image::{DynamicImage, RgbImage};
        let black = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0])));
        let report = fit_recipe(&black, &black);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_DEGENERATE),
            "the refusal is disclosed through the rationale channel: {:?}",
            report.recipe.rationale
        );
        assert!(report.recipe.tone_curve.is_empty(), "no constant tone map is produced");
        assert_eq!(report.recipe.exposure_ev, 0.0, "the recipe stays neutral");
    }

    use super::*;

    #[test]
    fn fit_budget_scales_monotonically_with_strength() {
        let low = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.0));
        let mid = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65));
        let high = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0));
        assert!(low.ev < mid.ev && mid.ev < high.ev);
        assert!(low.sat < mid.sat && mid.sat < high.sat);
        assert!(low.wb_gain.0 > mid.wb_gain.0 && mid.wb_gain.0 > high.wb_gain.0);
        assert!(low.wb_gain.1 < mid.wb_gain.1 && mid.wb_gain.1 < high.wb_gain.1);
        assert!(low.cast_ratio < mid.cast_ratio && mid.cast_ratio < high.cast_ratio);
        assert_eq!(low.vetoes, VetoPolicy::Withhold);
        assert_eq!(high.vetoes, VetoPolicy::Disclose);
    }

    #[test]
    fn fit_budget_default_is_byte_identical_to_pre_f1() {
        let b = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65));
        assert_eq!(b, FitBudget {
            ev: 1.0,
            sat: 30.0,
            wb_gain: (0.80, 1.25),
            wb_ratio: 1.40,
            wb_rotation_share: ROT_SHARE,
            cast_ratio: CAST_ACCEPT_RATIO,
            slope: (0.5, 1.5),
            confidence_cap: 0.50,
            hsl_band: HSL_BAND_LIMIT_DEFAULT,
            vetoes: VetoPolicy::Withhold,
        });
        assert_eq!(60.0 * b.sat / ATMOSPHERE_SAT_LIMIT, 60.0);
        assert_eq!(b.cast_ratio, CAST_ACCEPT_RATIO);
        assert_eq!(b.wb_rotation_share, ROT_SHARE);
        assert_eq!(RESIDUAL_SLOPE_CAP * b.slope.1 / ATMOSPHERE_CURVE_SLOPE_MAX, RESIDUAL_SLOPE_CAP);

        // The legacy provider wrapper and the F1 options path must serialize
        // the same ordinary Full-mode solve at the pinned default. This is a
        // small in-repo surrogate for the external calibration/live corpus.
        let src = DynamicImage::ImageRgb8(RgbImage::from_fn(32, 32, |x, y| {
            let v = 24 + ((x + y) % 180) as u8;
            image::Rgb([v, v, v])
        }));
        let target = DynamicImage::ImageRgb8(RgbImage::from_fn(32, 32, |x, y| {
            let v = 34 + ((x + y) % 180) as u8;
            image::Rgb([v, v, v])
        }));
        let legacy = fit_recipe_from_promoted_with_disclosure(
            &src,
            &target,
            &EditRecipe::default(),
            false,
            false,
            None,
        );
        let f1 = fit_recipe_from_with(
            &src,
            &target,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
        );
        assert_eq!(
            serde_json::to_value(&legacy.recipe).unwrap(),
            serde_json::to_value(&f1.recipe).unwrap(),
            "the default options path changed an ordinary Full recipe"
        );
    }

    #[test]
    fn wb_default_strength_is_byte_identical_to_head() {
        let source = hazy_canyon_source();
        let target = vivid_warm_target();
        let report = fit_recipe_from_with(
            &source,
            &target,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
        );
        assert_eq!(report.recipe.temperature_k, None);
        assert_eq!(report.recipe.tint, 0.0);
        assert_eq!(report.recipe.exposure_ev, -0.28);
        assert_eq!(report.recipe.saturation, 0.0);
        assert_eq!(
            report.recipe.tone_curve,
            vec![
                crate::recipe::CurvePoint { input: 0, output: 0 },
                crate::recipe::CurvePoint { input: 65, output: 61 },
                crate::recipe::CurvePoint { input: 131, output: 118 },
                crate::recipe::CurvePoint { input: 179, output: 190 },
                crate::recipe::CurvePoint { input: 255, output: 255 },
            ]
        );
        assert_eq!(report.recipe.confidence, 0.25);

        let Some(dir) = calibration_corpus() else { return };
        let source = image::open(dir.join("neutral.jpg")).expect("calibration source");
        let target = image::open(dir.join("target.jpg")).expect("calibration target");
        let report = fit_recipe_from_with(
            &source,
            &target,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
        );
        // RE-PINNED by step 9, user ruling 1 (2026-08-31). The calibration
        // pair no longer PERSISTS a white balance at the shipped default, and
        // that is the budget doing its job rather than a capability lost.
        // Measured first-party on this corpus: the per-pixel estimator asks
        // K 16050 / tint +34.3, gains [1.263, 0.931, 0.764], gain RATIO
        // 1.6534, where the marginal-median estimator asked K 7100 / +22.3 at
        // ratio 1.2314. `ATMOSPHERE_WB_GAIN_RATIO` allows 1.40 at
        // `GradeStrength::DEFAULT`, so the demand is refused and the recipe
        // keeps the as-shot white balance.
        //
        // The refusal is a function of STRENGTH, not a ceiling. `wb_ratio` is
        // `between(1.20, ATMOSPHERE_WB_GAIN_RATIO, 3.0)` interpolated on the
        // strength axis, so at strength 1.0 the budget is 3.0 and the very
        // same pair persists the very same move — which the second arm below
        // measures rather than asserting. Wanting a larger white-balance
        // shift is exactly what the F1 freedom axis already ships.
        assert_eq!(report.recipe.temperature_k, None);
        assert_eq!(report.recipe.tint, 0.0);
        // The EXPOSURE solve is not part of this batch: it stays on the
        // marginal weighted luma median. Measured unchanged at -1.00, which
        // is the assertion that would have caught an accidental side effect.
        assert_eq!(report.recipe.exposure_ev, -1.0);
        let full = fit_recipe_from_with(
            &source,
            &target,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
        );
        assert_eq!(
            full.recipe.temperature_k,
            Some(16050.0),
            "the same demand is INSIDE the budget once the strength axis widens it"
        );
        assert!((full.recipe.tint - 34.3).abs() <= 0.05, "tint {}", full.recipe.tint);
    }

    /// Step 9 / acceptance A1: a pair in which NO pixel changed its
    /// chromaticity may not persist a colour cast.
    ///
    /// The ground truth is structural and readable in the fixture's own
    /// source rather than asserted here: `flat_sky_to_cloud_deck`'s land
    /// branch never consults `clouds`, so the lower half is byte-identical
    /// between the two builds, and its sky keeps the same chromaticity vector
    /// `[l*0.83, l*0.92, l]` with only the luminance `l` redrawn. There is no
    /// cast to find. Three INDEPENDENT per-channel medians nonetheless read
    /// K 4400 / tint +55.2 on it - 27x this tolerance - because the source's
    /// smooth sky and the target's cloud deck put the three marginals'
    /// halfway points in different sub-populations, so their ratio is no
    /// pixel's colour. The fixture is 384x256 == `ANALYZE_EDGE`, so
    /// `analysis_pair` does not resample it.
    ///
    /// Supervisor mutations M-1-A (the three marginal medians restored) and
    /// M-1-E (the geometric-mean normalisation deleted) both go red here.
    #[test]
    fn a_pair_that_changed_no_pixel_chromaticity_persists_no_cast() {
        let (src, tgt) = flat_sky_to_cloud_deck();
        let report = fit_recipe(&src, &tgt);
        assert_eq!(
            report.mode,
            FitMode::Atmosphere,
            "premise: the cloud deck is content-divergent, so RC1 is what runs"
        );
        let k = report
            .recipe
            .temperature_k
            .expect("a neutral demand is inside the atmosphere budget and persists");
        assert!(
            (k - 5500.0).abs() <= 200.0,
            "no chromaticity moved, so the anchor must stand: {k} K, tint {}",
            report.recipe.tint
        );
        assert!(
            report.recipe.tint.abs() <= 2.0,
            "no chromaticity moved, so no tint may be invented: {}",
            report.recipe.tint
        );
    }

    /// Step 9 / acceptance A2: a readable correspondence field chooses the
    /// POPULATION and the PAIRING, and the estimator may not take one without
    /// the other.
    ///
    /// The target here is the same target, RECOMPOSED - rolled down by 64 of
    /// its 256 rows, 25% of the frame. Content preserved, moved in frame,
    /// which is squarely inside Atmosphere's remit and is exactly what
    /// same-index pairing cannot survive: with the population restricted to
    /// the shared content but the pairing left on the raw index, the
    /// estimator reads a large invented cast off a pair whose true `gr/gb` is
    /// 1.000000. 64 of 256 rows is 12 of the sidecar's 48 grid cells exactly,
    /// so a field that knows the roll is a two-line variant of
    /// `identity_test_field`.
    ///
    /// It asserts on the value the SOLVE returns rather than on the recipe,
    /// and that is not a convenience: both wrong answers demand a gain ratio
    /// above `ATMOSPHERE_WB_GAIN_RATIO`, so all three arms persist `None` and
    /// a black-box assertion could not see the defect at all.
    ///
    /// Supervisor mutation M-1-B (read `tp` instead of the remapped array
    /// when the field is readable) goes red here and only here.
    #[test]
    fn a_readable_field_chooses_the_pairing_and_not_only_the_population() {
        let (src, tgt) = flat_sky_to_cloud_deck();
        let rolled = {
            let rgb = tgt.to_rgb8();
            let (w, h) = (rgb.width(), rgb.height());
            DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
                *rgb.get_pixel(x, (y + h - 64) % h)
            }))
        };
        let (s_img, t_img) = analysis_pair(&src, &rolled);
        let (w, h) = (s_img.width(), s_img.height());
        let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_for(&sp, &tp, w, h).structure_blind(&tp);
        let g = crate::correspond::GRID;
        let cells = g * g;
        let field = crate::correspond::CorrespondenceField {
            map_y: (0..cells).map(|c| (((c / g) + 12) % g) as f32).collect(),
            ..identity_field()
        };
        let pc = correspondence_for_pair(&field, &tp, (w, h), (w, h));
        let shared = shared_content_population(&evidence, &pc)
            .expect("both sides carry evidence mass");
        assert!(
            shared.readable(),
            "premise: the field must be readable, retention {:.3}/{:.3}",
            shared.source_retained,
            shared.target_retained
        );
        let (pair_tp, pair_w) =
            atmosphere_wb_pairing(&tp, &evidence, Some(&pc), Some(&shared));
        let (_, _, unpaired) =
            atmosphere_wb_from_populations(&sp, &tp, &pair_w, 5500.0);
        assert!(
            (unpaired[0] / unpaired[2] - 1.0).abs() > 0.10,
            "premise: same-index pairing against a moved target IS broken: {unpaired:?}"
        );
        let (_, _, wanted) =
            atmosphere_wb_from_populations(&sp, pair_tp, &pair_w, 5500.0);
        assert!(
            (wanted[0] / wanted[2] - 1.0).abs() <= 0.02,
            "the field that chose the population must also choose the pairing: {wanted:?}"
        );
    }

    #[test]
    fn global_cast_is_measured_when_every_band_is_one_sided_and_consistent() {
        let source = vec![[0.08, 0.16, 0.82]; 256];
        let target = vec![[0.82, 0.34, 0.08]; 256];
        let evidence = evidence_model_for(&source, &target, 16, 16);
        let cast = evidence.global_cast.expect("coherent one-sided bands are a global cast");
        assert!(cast.rotation_deg.abs() > 20.0);
        assert!(cast.chroma_ratio > 0.5);
    }

    #[test]
    fn opposed_band_rotation_is_still_withheld() {
        let mut source = Vec::new();
        let mut target = Vec::new();
        for i in 0..256 {
            let p = if i % 2 == 0 { [0.08, 0.16, 0.82] } else { [0.82, 0.16, 0.08] };
            let q = if i % 2 == 0 { [0.82, 0.34, 0.08] } else { [0.08, 0.34, 0.82] };
            source.push(p);
            target.push(q);
        }
        assert!(evidence_model_for(&source, &target, 16, 16).global_cast.is_none());
    }

    #[test]
    fn high_strength_discloses_instead_of_withholding() {
        let px = vec![[0.4, 0.4, 0.4]; 64];
        let mut evidence = evidence_model(&px, &px);
        evidence.identifiability = 0.9;
        let budget = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0));
        let report = compose_report(
            EditRecipe::default(),
            Measured {
                err_before: 0.2,
                err_after: 0.1,
                joint_after: None,
                after_px: &px,
                tp: &px,
                same_frame: true,
                mode: FitMode::Atmosphere,
                divergence: Divergence::matched(),
                evidence: &evidence,
                structural_evidence: Some(&evidence),
                defer_disclosure: false,
            },
            SolveFacts {
                budget: Some(budget),
                strength: Some(1.0),
                veto_luma: Some("luma bins 06-08".into()),
                veto_hue: None,
                wb_clamped: None,
                wb_search_bound: None,
                wb_rotation_coverage: None,
                wb_rotation_disclosure: None,
                wb_foreign_hue_withheld: false,
                wb_rotation_withheld: false,
                sat_pegged: None,
                cast: CastOutcome::default(),
                cast_admitted_by_strength: None,
                cast_admitted: None,
                cast_projected: None,
                evidence_refused: true,
                sat_fitted: None,
                regressed: None,
                detail: (0.0, 0.0),
                detail_withheld: false,
                robust: None,
                paired: false,
                vouched_bands: None,
                hsl: HslStageFacts::default(),
                atmosphere_reference: AtmosphereReference::WholeFrame,
            },
        );
        assert!(report.recipe.confidence <= 0.35);
        assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_VETO_DISCLOSED));
    }

    #[test]
    fn frame_regression_law_holds_at_strength_one() {
        let (src, tgt) = structural_permutation_pair();
        let report = fit_recipe_from_with(
            &src,
            &tgt,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
        );
        assert!(report.err_after <= report.err_before + 1e-4);
    }

    #[test]
    fn cast_ratio_is_pinned_to_head_at_default() {
        assert_eq!(
            FitBudget::for_strength(crate::recipe::GradeStrength::default()).cast_ratio,
            CAST_ACCEPT_RATIO
        );
    }

    #[test]
    fn wb_rotation_budget_opens_linearly_with_strength() {
        let at_zero = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.0));
        let at_default = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65));
        let at_mid = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85));
        let at_full = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0));
        assert_eq!(at_zero.wb_rotation_share, ROT_SHARE);
        assert_eq!(at_default.wb_rotation_share, ROT_SHARE);
        assert!((at_mid.wb_rotation_share - 0.5928571).abs() <= 1e-3);
        assert_eq!(at_full.wb_rotation_share, 1.0);
    }

    #[test]
    fn synthetic_full_region_wb_rotation_exceeds_seventy_percent_budget() {
        let cur = vec![[0.10f32, 0.25, 0.82]; 1000];
        let with = vec![[0.82f32, 0.32, 0.10]; 1000];
        let evidence = evidence_model(&cur, &cur);
        let rotated_share = rehued_share_weighted(&cur, &with, &evidence);
        assert!(rotated_share > 0.99, "synthetic chromatic region is fully re-hued");
        assert!(rotated_share > FitBudget::for_strength(crate::recipe::GradeStrength::new(0.70)).wb_rotation_share);
    }

    #[test]
    fn rescoring_round_trips_fractional_strength_and_budget() {
        let prior = vec![crate::rationale::Note::new(
            crate::rationale::keys::FIT_NOTE_STRENGTH,
            vec![("pct", "64".into()), ("s", "0.6440".into())],
        )];
        let strength = carried_strength_from_notes(&prior);
        assert_eq!(strength.get(), 0.644);
        assert_eq!(FitBudget::for_strength(strength), FitBudget::for_strength(crate::recipe::GradeStrength::new(0.644)));
        let src = hazy_canyon_source();
        let tgt = vivid_warm_target();
        let rescored = rescore_report(&src, &tgt, &EditRecipe::default(), 0.2, &prior);
        let carried_s = rescored
            .notes
            .iter()
            .find(|note| note.key == crate::rationale::keys::FIT_NOTE_STRENGTH)
            .and_then(|note| note.args.iter().find(|(key, _)| *key == "s"))
            .map(|(_, value)| value.as_str());
        assert_eq!(carried_s, Some("0.6440"));
    }

    /// F1 review: a rescoring after the deep step must RE-DERIVE the
    /// high-strength veto disclosure and keep its cap. Before this pin the
    /// rescored report dropped `veto_luma`/`veto_hue` (and, in Full mode, the
    /// budget itself), so the same unsupported movement came back uncapped.
    #[test]
    fn rescoring_re_derives_the_high_strength_veto_disclosure_and_its_cap() {
        let src = hazy_canyon_source();
        let tgt = vivid_warm_target();
        let full = FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None };
        let solved = fit_recipe_from_with(&src, &tgt, &EditRecipe::default(), full);
        let disclosed = |r: &FitReport| {
            r.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_VETO_DISCLOSED)
        };
        assert!(disclosed(&solved), "fixture must disclose unsupported movement at strength 1.0");
        let rescored = rescore_report(&src, &tgt, &solved.recipe, solved.err_before, &solved.notes);
        assert!(disclosed(&rescored), "the rescoring must re-derive the disclosure for the same recipe");
        let cap = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).confidence_cap;
        assert!(
            rescored.recipe.confidence <= cap + 1e-6,
            "rescored confidence {} above the strength cap {cap}",
            rescored.recipe.confidence
        );
        // Default control: no strength note carried → withhold policy → no
        // disclosure, exactly the pre-F1 rescoring.
        let shipped = fit_recipe_from_with(&src, &tgt, &EditRecipe::default(), FitOptions::default());
        let rescored_default =
            rescore_report(&src, &tgt, &shipped.recipe, shipped.err_before, &shipped.notes);
        assert!(!disclosed(&rescored_default), "the shipped default must not gain a disclosure");
    }

    #[test]
    fn cast_admission_disclosure_tracks_strength_budget() {
        assert!(cast_admitted_by_strength(
            2.4,
            FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85)).cast_ratio,
            0.85,
        ));
        assert!(!cast_admitted_by_strength(
            2.4,
            FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65)).cast_ratio,
            0.65,
        ));
        let mut rationale = String::new();
        let mut notes = Vec::new();
        crate::rationale::push_note(
            &mut rationale,
            &mut notes,
            crate::rationale::Note::new(
                crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_BY_STRENGTH,
                vec![("ratio", "2.400".into()), ("budget", "2.593".into())],
            ),
        );
        assert!(rationale.contains("measured ratio 2.400"));
    }

    #[test]
    fn early_exit_atmosphere_report_keeps_the_strength_cap() {
        let src = DynamicImage::ImageRgb8(RgbImage::from_fn(32, 32, |x, y| {
            let v = 32 + ((x + y) % 160) as u8;
            image::Rgb([v, v, v])
        }));
        let report = fit_recipe_from_promoted_with_disclosure_opts(
            &src,
            &src,
            &EditRecipe::default(),
            true,
            false,
            FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
        );
        assert!(report.recipe.confidence <= 0.35);
        assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_STRENGTH));
    }

    #[test]
    fn confidence_tracks_measured_error_under_the_strength_cap() {
        let src = hazy_canyon_source();
        let tgt = vivid_warm_target();
        for s in [0.65, 0.85, 1.0] {
            let report = fit_recipe_from_with(
                &src,
                &tgt,
                &EditRecipe::default(),
                FitOptions { strength: crate::recipe::GradeStrength::new(s), provider: None },
            );
            let cap = FitBudget::for_strength(crate::recipe::GradeStrength::new(s)).confidence_cap;
            assert!(report.recipe.confidence <= cap + 1e-6, "s={s} confidence {} cap {cap}", report.recipe.confidence);
            let ladder = confidence_from_look_err(report.err_after);
            if ladder < cap - 1e-5 {
                assert!(report.recipe.confidence <= ladder + 1e-5, "s={s} confidence {} ladder {} cap {} err {}", report.recipe.confidence, ladder, cap, report.err_after);
            }
        }
    }
    use image::RgbImage;

    /// The knot-support gate itself, pinned at the unit level: a knot with
    /// no measured testimony must contribute NOTHING to the solve, however
    /// loud the estimated map's extrapolation is there. Supervisor mutation
    /// MC (drop `support` from the weight composition) goes red here. (At
    /// the stage-wiring level the same mutation is absorbed by the spline
    /// model-selection — defense in depth, not dead code: the marginal path
    /// scores on knots alone and has only this gate.)
    #[test]
    fn unsupported_knots_cannot_pull_the_solve() {
        let map = |x: f32| if x <= 0.66 { x } else { (x * 1.4).min(1.0) };
        let gated: [f32; 8] = [1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
        let (ev_g, s_g) = fit_tone_sliders_supported(&map, &gated, &[]);
        assert!(
            ev_g.abs() < 0.06 && s_g.iter().all(|v| v.abs() < 0.08),
            "identity-on-supported must solve near-neutral: ev={ev_g} s={s_g:?}"
        );
        let (ev_a, s_a) = fit_tone_sliders_supported(&map, &[1.0; 8], &[]);
        assert!(
            ev_a.abs() >= 0.06 || s_a.iter().any(|v| v.abs() >= 0.08),
            "premise: without the gate the phantom demand must visibly drag              the solve (ev={ev_a} s={s_a:?}) — if this stops holding, the              gate has nothing to guard and both asserts need re-deriving"
        );
    }

    /// The robust estimator's reason to exist, pinned at the unit level: a
    /// 30% invented sub-population in the target must lose weight BY THE
    /// ESTIMATOR'S OWN MECHANISM and leave the map on the clean population's
    /// truth. Supervisor mutation MA (Tukey weights forced to 1 = plain
    /// least squares) drags the contaminated bins' means and this goes red.
    #[test]
    fn robust_regression_downweights_invented_target_content() {
        let (w, h) = (128usize, 96usize);
        let mut sp = Vec::with_capacity(w * h);
        let mut tp = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let l = x as f32 / (w - 1) as f32;
                sp.push([l, l, l]);
                let mapped = (l * 1.3).min(1.0);
                if x < w * 3 / 10 && y < h / 2 {
                    // invented content: a flat bright warm patch nothing in
                    // the source explains (15% of the frame, dark-source bins)
                    tp.push([0.85, 0.75, 0.55]);
                } else {
                    tp.push([mapped, mapped, mapped]);
                }
            }
        }
        let fit = paired_robust_tone(&sp, &tp, &|_| 1.0, true)
            .expect("an aligned synthetic pair must be estimable");
        assert!(
            fit.rejected_share > 0.10,
            "the invented patch must be down-weighted, not averaged in              (rejected {:.3})",
            fit.rejected_share
        );
        let mid = sample_tone_points(&fit.points, 0.5);
        assert!(
            (mid - 0.65).abs() < 0.03,
            "the map must stay on the clean population's truth at x=0.5:              got {mid:.3}, truth 0.650"
        );
        // The invented patch sits over dark source columns — the disclosure
        // ranges must name at least one of the ranges it poisoned.
        assert!(
            !fit.rejected_ranges.is_empty(),
            "rejection must localise itself for the disclosure"
        );
    }

    /// The pipeline half of the same contract: a fit over a partially
    /// invented target must DISCLOSE what it rejected. Supervisor mutation MB
    /// (delete the FIT_NOTE_ROBUST_REJECTED push in compose_report) goes red
    /// here while the estimator itself still works.
    #[test]
    fn robust_rejection_reaches_the_disclosure() {
        let src = synth();
        let mut truth = EditRecipe { exposure_ev: 0.4, contrast: 12.0, ..Default::default() };
        truth.clamp();
        let rendered = render::develop_preview(&src, &truth);
        let mut tgt = rendered.to_rgb8();
        let (w, h) = (tgt.width(), tgt.height());
        // Scattered 8x8 LUMA-PRESERVING recolour blocks (~12% of the frame):
        // the luma structure is untouched, so neither the global divergence
        // statistic nor the 3x3 spatial-evidence screen reacts — the
        // PER-PIXEL robust verdict (chromatic residual + hue incoherence) is
        // the only thing standing between the invented hues and the colour
        // stages. Exactly the estimator's niche.
        for by in 0..h / 8 {
            for bx in 0..w / 8 {
                if (bx * 3 + by * 5) % 8 == 0 {
                    for dy in 0..8 {
                        for dx in 0..8 {
                            let p = *tgt.get_pixel(bx * 8 + dx, by * 8 + dy);
                            let y = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                            let warm = [
                                (y * 1.45).min(255.0) as u8,
                                (y * 0.95) as u8,
                                (y * 0.35) as u8,
                            ];
                            tgt.put_pixel(bx * 8 + dx, by * 8 + dy, image::Rgb(warm));
                        }
                    }
                }
            }
        }
        let tgt = image::DynamicImage::ImageRgb8(tgt);
        let report = fit_recipe(&src, &tgt);
        let note = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_ROBUST_REJECTED)
            .expect("a partially invented target must carry the rejection note");
        let pct: f32 = note
            .args
            .iter()
            .find(|(k, _)| *k == "pct")
            .map(|(_, v)| v.parse().unwrap())
            .expect("the note must carry the rejected percentage");
        assert!(pct >= 5.0, "a 15% invented region must reject visibly, got {pct}%");
    }

    /// The user's own Lightroom develop as ground truth (p36: pure-global
    /// tier — Exposure +0.50, Contrast +14, Highlights -44, Shadows +40,
    /// Whites -18, Sat -18/Vib +24, custom curve; the export is pixel-aligned
    /// with the neutral render by construction). LR's and this engine's
    /// parameter spaces differ, so the pin is the directly comparable core:
    /// the paired path must engage, exposure must land near the LR anchor,
    /// and the residual/confidence must hold the measured line. Supervisor
    /// mutation MC (knot support forced to all-ones) resurrects the phantom
    /// -knot degeneracy (ev pegged at +3) and this goes red.
    #[test]
    fn p36_fixture_recovers_the_lightroom_exposure_anchor() {
        let Some(root) = calibration_corpus() else { return };
        // The source is the camera's EMBEDDED PREVIEW — the very base the
        // CLI `match` solves against for a RAW (main.rs's calibration-stamp
        // note) — so the LR anchor means the same thing here as it does on
        // the command line.
        let (n, t) = (root.join("p36-preview.jpg"), root.join("p36-target.jpg"));
        if !n.is_file() || !t.is_file() {
            eprintln!("SKIPPED p36 fixture test: pair not in the corpus");
            return;
        }
        let src = image::open(n).unwrap();
        let tgt = image::open(t).unwrap();
        let report = fit_recipe(&src, &tgt);
        eprintln!(
            "P36_FIXTURE ev={} c={} h={} s={} sat={} err={:.4}->{:.4} conf={:.3}",
            report.recipe.exposure_ev,
            report.recipe.contrast,
            report.recipe.highlights,
            report.recipe.shadows,
            report.recipe.saturation,
            report.err_before,
            report.err_after,
            report.recipe.confidence
        );
        assert!(
            report.notes.iter().any(|note| {
                note.key == crate::rationale::keys::FIT_SUMMARY_WITH_CURVE_PAIRED
                    || note.key == crate::rationale::keys::FIT_SUMMARY_NO_CURVE_PAIRED
            }),
            "the aligned export must take the paired path: {}",
            report.recipe.rationale
        );
        // LR's Exposure2012 and this engine's exposure_ev are different
        // parameter spaces (different base curves, different shoulder), so
        // the anchor is directional and bounded rather than numeric: the
        // LR +0.50 brightening must come back as a moderate positive ev
        // (measured 0.75 with the residual curve carrying the shape), and
        // NEVER as the phantom-knot degeneracy this test exists to catch
        // (support mutation MC pegs ev at +3.0 and dies here).
        assert!(
            report.recipe.exposure_ev > 0.20 && report.recipe.exposure_ev < 1.20,
            "exposure must land as a moderate positive move, got {}",
            report.recipe.exposure_ev
        );
        assert!(report.err_after < 0.035, "residual {:.4}", report.err_after);
        assert!(
            report.recipe.confidence >= 0.55,
            "confidence {:.3}",
            report.recipe.confidence
        );
    }

    #[test]
    fn p36_full_rescore_round_trip_keeps_structural_evidence_absent() {
        let Some(root) = calibration_corpus() else { return };
        let (source_path, target_path) =
            (root.join("p36-preview.jpg"), root.join("p36-target.jpg"));
        if !source_path.is_file() || !target_path.is_file() {
            eprintln!("SKIPPED p36 rescore test: pair not in the corpus");
            return;
        }
        let source = image::open(source_path).expect("p36 preview");
        let target = image::open(target_path).expect("p36 target");
        let solved = fit_recipe(&source, &target);
        assert_eq!(solved.mode, FitMode::Full);
        assert!(solved.structural_evidence.is_none());
        let rescored = rescore_report(
            &source,
            &target,
            &solved.recipe,
            solved.err_before,
            &solved.notes,
        );
        assert_eq!(rescored.mode, FitMode::Full);
        assert!(rescored.structural_evidence.is_none());
        assert_eq!(
            serde_json::to_vec(&rescored.recipe).unwrap(),
            serde_json::to_vec(&solved.recipe).unwrap(),
            "Full-mode rescore must reproduce the solved recipe byte for byte"
        );
    }

    /// M-F1: `fit_tone_sliders` degraded to return neutral (or any solver
    /// regression that stops beating the ground truth under the engine's own
    /// penalised objective) — on a look the weighted model can represent
    /// exactly, the solve must score at least as well as the generating
    /// parameters themselves.
    ///
    /// Recorded honestly: the fit-side knot WEIGHTING itself has no
    /// test-observable effect — an unweighted inner solve is a worse proposer
    /// in the saturated regime, but the outer acceptance loop re-scores every
    /// candidate by real rendering and masks it (verified by running the full
    /// suite under that mutant). The weights stay in the solve for model
    /// consistency — three sites, one definition — not because a test pins
    /// them.
    #[test]
    fn the_fit_models_the_same_weighted_engine_it_renders_against() {
        let ev = 1.5f32;
        let truth = [-0.6f32, 0.0, 0.35, 0.0, 0.0]; // contrast −60, shadows +35
        let weights = render::tone_knot_weights(ev);
        let tone = |x: f32| -> f32 {
            let i = render::TONE_KNOTS_X
                .iter()
                .position(|&k| (k - x).abs() < 1e-6)
                .expect("fit samples the tone map at the knots only");
            let b = render::tone_slider_basis(x);
            render::tone_exposure_curve(x, ev)
                + weights[i] * (0..5).map(|k| b[k] * truth[k]).sum::<f32>()
        };
        let (got_ev, got) = fit_tone_sliders(&tone);
        // The saturated regime is deliberately non-identifiable (several
        // (ev, sliders) pairs render the same 8 knots, and the ridge prior
        // picks the smallest sliders), so the property is NOT parameter
        // recovery. It is: under the engine's OWN penalised objective — knot
        // error through the weighted model, plus the magnitude prior — the
        // solution must be at least as good as the ground truth itself. The
        // pristine solve minimises exactly this, so it passes structurally;
        // a solve that dropped the weights optimises a different forward
        // model and lands on parameters this objective scores worse.
        let true_score = |cand_ev: f32, s: &[f32; 5]| -> f64 {
            let w = render::tone_knot_weights(cand_ev);
            let sse: f64 = render::TONE_KNOTS_X
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    let b = render::tone_slider_basis(x);
                    let rendered = render::tone_exposure_curve(x, cand_ev)
                        + w[i] * (0..5).map(|k| b[k] * s[k]).sum::<f32>();
                    let e = (rendered - tone(x)) as f64;
                    e * e
                })
                .sum();
            sse + s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum::<f64>()
        };
        let (got_score, truth_score) = (true_score(got_ev, &got), true_score(ev, &truth));
        assert!(
            got_score <= truth_score + 1e-6,
            "the fit landed on (ev {got_ev}, {got:?}) scoring {got_score:.6} — WORSE under \
             the engine's own model than the ground truth's {truth_score:.6}: the solve is \
             not modelling the engine it renders against"
        );
    }

    /// Synthetic frame with real tonal + chromatic coverage: a neutral luma ramp
    /// plus orange / blue / green ramps (192×128 — analysis-sized already).
    fn synth() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = x as f32 / (w - 1) as f32;
                let p = match y * 4 / h {
                    0 => [l, l, l],
                    1 => [l, l * 0.6, l * 0.2],
                    2 => [l * 0.2, l * 0.7, l],
                    _ => [l * 0.3, l, l * 0.4],
                };
                img.put_pixel(x, y, image::Rgb(p.map(|c| (c * 255.0).round() as u8)));
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// Same landscape footprint, but the target grows a high-frequency cloud
    /// deck over the source's smooth sky. The lower half is kept identical so
    /// the fixture exercises structural evidence rather than a wholesale
    /// unrelated-frame rejection.
    fn flat_sky_to_cloud_deck() -> (DynamicImage, DynamicImage) {
        let (w, h) = (384u32, 256u32);
        let build = |clouds: bool| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                let xf = x as f32 / (w - 1) as f32;
                let yf = y as f32 / (h - 1) as f32;
                let l = if y < h / 2 {
                    if clouds {
                        let broad = (xf * 8.0 + yf * 5.0).sin();
                        let billow = (xf * 31.0 - yf * 17.0).sin();
                        let knots = if ((x / 18) + (y / 12)) % 2 == 0 { -0.16 } else { 0.16 };
                        (0.55 + 0.28 * broad + 0.18 * billow + knots).clamp(0.04, 0.98)
                    } else {
                        0.62 + 0.05 * xf - 0.03 * yf
                    }
                } else {
                    let ridge = if yf > 0.68 + 0.10 * (xf * 9.0).sin() { 0.22 } else { 0.42 };
                    (ridge + 0.18 * xf + 0.03 * (xf * 43.0).sin()).clamp(0.02, 0.90)
                };
                let p = if y < h / 2 {
                    [l * 0.83, l * 0.92, l]
                } else {
                    [l, l * 0.78, l * 0.55]
                };
                image::Rgb(p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
            }))
        };
        (build(false), build(true))
    }

    /// A pure structural permutation: both frames carry exactly the same
    /// pixel population, while the sky's spatial arrangement is scrambled.
    /// Atmosphere mode therefore has a reachable neutral look target even
    /// though structural correlation is intentionally broken.
    fn structural_permutation_pair() -> (DynamicImage, DynamicImage) {
        let (w, h) = (384u32, 256u32);
        let source = RgbImage::from_fn(w, h, |x, y| {
            let xf = x as f32 / (w - 1) as f32;
            let yf = y as f32 / (h - 1) as f32;
            let l = if y < h / 2 {
                (0.55 + 0.22 * (xf * 19.0 + yf * 7.0).sin()
                    + 0.10 * (xf * 53.0 - yf * 11.0).sin())
                    .clamp(0.05, 0.95)
            } else {
                (0.25 + 0.45 * xf + 0.08 * (xf * 29.0).sin()).clamp(0.03, 0.90)
            };
            image::Rgb(if y < h / 2 {
                [l * 0.82, l * 0.92, l]
            } else {
                [l, l * 0.78, l * 0.55]
            }
            .map(|v| (v * 255.0).round() as u8))
        });
        let mut target = source.clone();
        let sky_n = (w * h / 2) as usize;
        for i in 0..sky_n {
            let from = (i * 193) % sky_n;
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            let (sx, sy) = ((from as u32) % w, (from as u32) / w);
            target.put_pixel(x, y, *source.get_pixel(sx, sy));
        }
        (DynamicImage::ImageRgb8(source), DynamicImage::ImageRgb8(target))
    }

    #[test]
    fn content_divergence_fires_on_flat_sky_to_cloud_deck() {
        let (src, tgt) = flat_sky_to_cloud_deck();
        let synthetic = structure_divergence_for(&src, &tgt, &EditRecipe::default(), None);
        eprintln!("STRUCTURE_CALIBRATION synthetic={synthetic:?}");
        assert!(
            synthetic.d >= DIVERGENCE_GLOBAL,
            "a generated cloud deck must cross the global threshold: {synthetic:?}"
        );

        // The calibration corpus is intentionally optional for portable
        // CI (see `calibration_dir`); where present it pins the measured
        // number rather than merely the side of the threshold.
        let Some(root) = calibration_corpus() else { return };
        if root.join("neutral.jpg").exists() {
            let source = image::open(root.join("neutral.jpg")).unwrap();
            let target = image::open(root.join("target.jpg")).unwrap();
            let measured = structure_divergence_for(
                &source,
                &target,
                &EditRecipe::default(),
                None,
            );
            eprintln!("STRUCTURE_CALIBRATION real={measured:?}");
            assert!(
                (measured.d - 0.491).abs() <= 0.05,
                "generated-cloud calibration drifted: {measured:?}"
            );
        }
    }

    #[test]
    fn content_divergence_is_calibrated_on_every_shipped_showcase_asset() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
        // The two shipped reverse-fit panels — the viaduct as composed for
        // v1.2.2 (`01de443`) and the Cornwall panel as RE-composed for v1.2.3
        // (`f3885b2`) — each read at the top-row offsets the panel geometry
        // fixes: the neutral conversion at left, the generated target in the
        // middle. Both generators were asked
        // for a grade and not a rebuild, so both sit UNDER the threshold and
        // the full solve ran on each; the fired arm of the same statistic is
        // pinned on a synthetic pair by
        // `content_divergence_fires_on_flat_sky_to_cloud_deck`. The pins are
        // the values measured when the panels were composed; a panel swapped
        // for another generation moves them, which is the point. Re-measured
        // on the shipped v1.2.3 bytes (2026-09-02): viaduct 0.17987, Cornwall
        // 0.13568 — the Cornwall re-composition did not move its reading off
        // the pin it had, which is why the pin below is the same number.
        for (file, want) in [
            ("showcase-viaduct-reverse-fit.jpg", 0.180f32),
            ("showcase-cornwall-reverse-fit.jpg", 0.136f32),
        ] {
            let panel = image::open(root.join(file)).unwrap();
            let source = panel.crop_imm(0, 136, 532, 356);
            let target = panel.crop_imm(535, 136, 530, 356);
            let measured =
                structure_divergence_for(&source, &target, &EditRecipe::default(), None);
            eprintln!("STRUCTURE_CALIBRATION {file} {measured:?}");
            assert!(
                measured.d < DIVERGENCE_GLOBAL && (measured.d - want).abs() <= 0.05,
                "{file} calibration drifted: {measured:?}, expected {want:.3}"
            );
        }
    }

    #[test]
    fn evidence_gating_does_not_regress_any_shipped_showcase_pair() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
        let mut pairs = Vec::new();
        for (name, file) in [
            ("viaduct", "showcase-viaduct-reverse-fit.jpg"),
            ("cornwall", "showcase-cornwall-reverse-fit.jpg"),
        ] {
            let panel = image::open(root.join(file)).unwrap();
            pairs.push((
                name.to_string(),
                panel.crop_imm(0, 136, 532, 356),
                panel.crop_imm(535, 136, 530, 356),
            ));
        }
        for (name, source, target) in pairs {
            let report = fit_recipe(&source, &target);
            eprintln!(
                "SHOWCASE pair={name} mode={:?} err={:.6}->{:.6} confidence={:.3} ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} sat={:.1}",
                report.mode,
                report.err_before,
                report.err_after,
                report.recipe.confidence,
                report.recipe.exposure_ev,
                report.recipe.contrast,
                report.recipe.highlights,
                report.recipe.shadows,
                report.recipe.whites,
                report.recipe.blacks,
                report.recipe.saturation,
            );
            assert!(
                report.err_after <= report.err_before + 1e-6,
                "{name} regressed: {:.6} -> {:.6}",
                report.err_before,
                report.err_after
            );
        }
    }

    #[test]
    fn same_content_evidence_diagnosis_reports_cornwall_support_and_terminal_readings() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
        let triptych = image::open(root.join("showcase-cornwall-reverse-fit.jpg")).unwrap();
        let source = triptych.crop_imm(0, 136, 532, 356);
        let target = triptych.crop_imm(535, 136, 530, 356);
        // 532x356 against 530x356 thumbnails to 384x257 against 384x258: the
        // evidence must be built in the one geometry the fit itself uses.
        let (s_img, t_img) = analysis_pair(&source, &target);
        let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
        let report = fit_recipe(&source, &target);
        let supported = evidence.spatial_supported.iter().filter(|&&v| v).count();
        let luma_weight: f32 = evidence.luma.iter().map(|r| r.weight).sum();
        let joint_base = crate::fit_zoned::joint_reading_with_evidence(
            &sp,
            &tp,
            &evidence.source_weights,
            &evidence.target_weights,
        );
        let after = pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let joint_after = crate::fit_zoned::joint_reading_with_evidence(
            &after,
            &tp,
            &evidence.source_weights,
            &evidence.target_weights,
        );
        eprintln!(
            "SAME_CONTENT_DIAG cornwall d={:.4} supported={}/{} ident={:.4} luma_weight={:.4} look={:.6}->{:.6} joint={joint_base:?}->{joint_after:?} recipe={:?}",
            report.divergence.d,
            supported,
            evidence.spatial_supported.len(),
            evidence.identifiability,
            luma_weight,
            report.err_before,
            report.err_after,
            report.recipe,
        );
        let scale = s_img.width().max(s_img.height()).div_ceil(192).max(1);
        let sw = s_img.width().div_ceil(scale);
        let sh = s_img.height().div_ceil(scale);
        let mut ss = Vec::new();
        let mut tt = Vec::new();
        for y in (0..s_img.height()).step_by(scale as usize) {
            for x in (0..s_img.width()).step_by(scale as usize) {
                let i = (y * s_img.width() + x) as usize;
                ss.push(sp[i]);
                tt.push(tp[i]);
            }
        }
        let mut cells = Vec::new();
        for row in 0..3u32 {
            for col in 0..3u32 {
                let mut mask = vec![0.0f32; ss.len()];
                for y in 0..sh {
                    for x in 0..sw {
                        if x * 3 / sw == col && y * 3 / sh == row {
                            mask[(y * sw + x) as usize] = 1.0;
                        }
                    }
                }
                cells.push(structure_divergence(&ss, &tt, sw, sh, &mask).d);
            }
        }
        eprintln!("SAME_CONTENT_DIAG cells={cells:?}");
        assert!(report.divergence.d < DIVERGENCE_GLOBAL);
        assert!(supported > evidence.spatial_supported.len() / 2);
        assert!(luma_weight > 0.5);
        // The paired robust estimator fits this real pair — re-measured
        // 2026-09-02 on the SHIPPED v1.2.3 panel: look 0.05888 -> 0.03061,
        // joint 0.17777 -> 0.04699, with the panel's cast projected at
        // t = 0.515 — so pin the fit, not a reset. The viaduct panel is the
        // wrong subject
        // here — its top row is the pair whose full solve needed zones and
        // tiles, and on the panel thumbnails the global-only path ends in a
        // do-no-harm reset (0.0354 -> 0.0354), which is exactly what this test
        // must NOT be satisfied by.
        assert!(
            report.err_after < report.err_before,
            "the paired path must actually fit this pair: {:.4} -> {:.4}",
            report.err_before,
            report.err_after
        );
        assert!(report.err_after < 0.045, "look residual {:.4}", report.err_after);
        assert!(
            !report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_REGRESSED),
            "no do-no-harm terminal reset on a fittable pair: {}",
            report.recipe.rationale
        );
        let joint = joint_after.expect("the joint family had an opinion before the fit");
        assert!(joint.weighted < 0.06, "joint after {:.4}", joint.weighted);
    }

    /// Step-7b conservation law, half one: an IDENTITY field (every cell maps
    /// to itself at full confidence) projects to the original target array
    /// byte-for-byte — a field that says "nothing moved, everything
    /// corresponds" must change nothing. Supervisor mutation M-7b-A (the
    /// within-cell offset dropped from the remap) goes red here: without it
    /// every pixel of a cell reads the cell's one corner sample.
    #[test]
    fn the_field_remap_is_identity_under_an_identity_field() {
        let (w, h) = (96u32, 64u32);
        let tp: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = (i.wrapping_mul(2_654_435_761) >> 8) as u8 as f32 / 255.0;
                [v, v, v]
            })
            .collect();
        let c = identity_field();
        let pc = correspondence_for_pair(&c, &tp, (w, h), (w, h));
        assert_eq!(pc.tp, tp, "identity field, identical dims: the remap must be a no-op");
        assert!(pc.conf.iter().all(|&x| x == 1.0));
        assert!((pc.coverage - 1.0).abs() < 1e-6 && (pc.median - 1.0).abs() < 1e-6);
    }

    /// A full-confidence identity field over the correspondence grid —
    /// the shared conservation fixture (`correspond::identity_test_field`).
    fn identity_field() -> crate::correspond::CorrespondenceField {
        crate::correspond::identity_test_field()
    }

    /// Step-7b, the mechanism itself: content SHIFTED between the renditions
    /// breaks same-index pairing (the estimator sees a random association and
    /// refuses), and the field's remap repairs it — the paired robust fit
    /// recovers the true tone map through the shift. Supervisor mutation
    /// M-7b-C (confidence/remap not reaching the pairs) goes red here.
    #[test]
    fn a_confident_shift_field_recovers_the_pairs_the_shift_broke() {
        let g = crate::correspond::GRID;
        let (w, h) = (192u32, 192u32); // 4 px per grid cell
        let shift_cells = 6usize; // content moves right by 24 px
        let col_luma = |x: u32| -> f32 {
            0.08 + 0.84 * ((x.wrapping_mul(2_654_435_761) >> 8) as u8 as f32 / 255.0)
        };
        let tone = |s: f32| 0.15 + 0.6 * s;
        let sp: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = col_luma(i % w);
                [v, v, v]
            })
            .collect();
        // The target: the SAME columns, tone-mapped, moved right by the shift
        // (content at source x sits at target x + 24).
        let tp: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let x = i % w;
                let v = tone(col_luma((x + w - 24) % w));
                [v, v, v]
            })
            .collect();
        // Same-index pairing is a random association: the estimator must not
        // manufacture a map out of it (either refuses or lands far off).
        let broken = paired_robust_tone(&sp, &tp, &|_| 1.0, true);
        let map_err = |r: &PairedRobustTone| {
            (1..=9)
                .map(|k| {
                    let x = k as f32 / 10.0;
                    (sample_tone_points(&r.points, x) - tone(x)).abs()
                })
                .fold(0.0f32, f32::max)
        };
        if let Some(r) = broken.as_ref() {
            assert!(
                map_err(r) > 0.05 || r.rejected_share > 0.5,
                "premise: a 24-px shift must actually break same-index pairing"
            );
        }
        // The field that KNOWS the shift: cell cx corresponds to target cell
        // cx + 6. Confidence 1 — the sidecar's smoothness term would grant a
        // rigid translation exactly this.
        let cells = g * g;
        let field = crate::correspond::CorrespondenceField {
            map_x: (0..cells).map(|c| (((c % g) + shift_cells) % g) as f32).collect(),
            ..identity_field()
        };
        let pc = correspondence_for_pair(&field, &tp, (w, h), (w, h));
        let repaired = paired_robust_tone(&sp, &pc.tp, &|i| pc.conf[i], true)
            .expect("the remapped pairing must carry a fittable map");
        assert!(
            map_err(&repaired) < 0.03,
            "the remap must recover the true tone map through the shift: err {:.4}",
            map_err(&repaired)
        );
    }

    /// Step-7b gate: the provider is consulted EXACTLY on content-divergent
    /// pairs — never on a Full-mode pair (a paid-in-seconds sidecar run per
    /// ordinary fit would be a regression), exactly once on a divergent one,
    /// and a failing provider degrades with the reason in the rationale while
    /// the atmosphere recipe stands. Supervisor mutation M-7b-B (the gate
    /// dropped, provider consulted unconditionally) goes red on the first
    /// assertion.
    #[test]
    fn the_provider_is_consulted_only_on_a_content_divergent_pair() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let failing = |_: &DynamicImage,
                       _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            calls.set(calls.get() + 1);
            Err(anyhow::anyhow!("no GPU on this machine"))
        };
        // Full-mode pair (a frame against itself): the gate must not consult.
        let (src, _) = structural_permutation_pair();
        let full = fit_recipe_with(&src, &src, FitOptions { strength: crate::recipe::GradeStrength::default(), provider: Some(&failing) });
        assert_eq!(calls.get(), 0, "a Full-mode fit must never pay for a correspondence run");
        assert_eq!(full.mode, FitMode::Full);
        // Divergent pair: exactly one consultation; the failure is disclosed
        // with its reason and the atmosphere fit stands unchanged.
        let (src, tgt) = structural_permutation_pair();
        let plain = fit_recipe(&src, &tgt);
        let report = fit_recipe_with(&src, &tgt, FitOptions { strength: crate::recipe::GradeStrength::default(), provider: Some(&failing) });
        assert_eq!(calls.get(), 1, "one divergent fit, one consultation");
        assert_eq!(report.mode, FitMode::Atmosphere);
        assert!(
            report.recipe.rationale.contains("no GPU on this machine"),
            "the failure reason must reach the rationale: {}",
            report.recipe.rationale
        );
        assert_eq!(
            (report.recipe.exposure_ev, report.recipe.tint, report.recipe.saturation),
            (plain.recipe.exposure_ev, plain.recipe.tint, plain.recipe.saturation),
            "a failing provider must leave the fit exactly as it was"
        );
    }

    /// R30 batch 1 (R2-lite): every Atmosphere report states which
    /// POPULATION its white balance and exposure were read over — a
    /// whole-frame per-channel median on both sides, i.e. a distribution
    /// pairing whose premise is exactly what Atmosphere mode denies. Zero
    /// behaviour change, so the assertion is on the note, and the dials are
    /// pinned against the same fit before the disclosure existed.
    #[test]
    fn an_atmosphere_report_states_the_population_its_white_balance_came_from() {
        let (src, tgt) = structural_permutation_pair();
        let report = fit_recipe(&src, &tgt);
        assert_eq!(report.mode, FitMode::Atmosphere);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
            "the reference population must be disclosed: {}",
            report.recipe.rationale
        );
        // A Full-mode report must NOT carry it — the sentence is a claim
        // about the Atmosphere solve, and Full solves on paired evidence.
        let full = fit_recipe(&src, &src);
        assert_eq!(full.mode, FitMode::Full);
        assert!(
            !full
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
            "a Full report must not claim an Atmosphere population"
        );
    }

    /// R30 batch 1 (R2-lite): with no correspondence field the unpaired share
    /// of that population is UNKNOWN, and an absent number must read as
    /// unknown rather than as zero. With a field it is stated, with its
    /// threshold and its grid resolution.
    #[test]
    fn the_unpaired_share_reads_as_unmeasured_when_there_is_no_field() {
        let (src, tgt) = structural_permutation_pair();
        let bare = fit_recipe(&src, &tgt);
        let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
        assert!(
            has(&bare, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNMEASURED),
            "no provider: the share is unmeasured, not zero: {}",
            bare.recipe.rationale
        );
        assert!(!has(&bare, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
        // A failing provider is the same epistemic position, and its own
        // reason still rides the step-7b sentence.
        let failing = |_: &DynamicImage,
                       _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Err(anyhow::anyhow!("no GPU on this machine"))
        };
        let failed = fit_recipe_with(
            &src,
            &tgt,
            FitOptions {
                strength: crate::recipe::GradeStrength::default(),
                provider: Some(&failing),
            },
        );
        assert!(has(&failed, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNMEASURED));
        assert!(has(&failed, crate::rationale::keys::FIT_CORRESPONDENCE_UNAVAILABLE));
        // A measured field replaces "unmeasured" with the number.
        let ok = |_: &DynamicImage,
                  _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(identity_field())
        };
        let measured = fit_recipe_with(
            &src,
            &tgt,
            FitOptions {
                strength: crate::recipe::GradeStrength::default(),
                provider: Some(&ok),
            },
        );
        // R30 R2: an identity field answers for every target cell, so the
        // restriction it authorises is the empty one — but the SHARE is now
        // an exclusion, and the sentence that reports it changed with it.
        assert!(has(&measured, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_EXCLUDED));
        assert!(!has(&measured, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
        assert!(!has(&measured, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNMEASURED));
        // …and it must be a DIALLESS change: same recipe as the bare fit.
        assert_eq!(
            (
                measured.recipe.exposure_ev,
                measured.recipe.temperature_k,
                measured.recipe.tint,
                measured.recipe.saturation,
            ),
            (
                bare.recipe.exposure_ev,
                bare.recipe.temperature_k,
                bare.recipe.tint,
                bare.recipe.saturation,
            ),
            "R2-lite is disclosure only"
        );
    }

    /// R30 R2 fixture: RC-A's shape, reduced to something a unit test can
    /// assert on, and deliberately sharper than the calibration pair.
    ///
    /// Rows `[0, INVENTED_ROWS)` are INVENTED — the target replaced them with
    /// content of its own texture — and are strictly BRIGHTER than the rest in
    /// every channel, so they span percentiles 40-100 and therefore own every
    /// whole-frame per-channel median by construction. Rows
    /// `[INVENTED_ROWS, h)` CORRESPOND, and carry the one thing the fit is
    /// supposed to find: a warm cast of `(1.060, 1.000, 0.955)` that sits
    /// inside the Atmosphere white-balance budget.
    ///
    /// The invented block is also 1.25x brighter than its own source, so a
    /// whole-frame exposure reads +0.69 EV where the truth is 0.00 — the
    /// fixture separates BOTH of the two controls this batch moves.
    const INVENTED_ROWS: u32 = 58; // 29 of the sidecar's 48 cell rows
    fn invented_half_pair() -> (DynamicImage, DynamicImage) {
        let (w, h) = (96u32, 96u32);
        let tex = |x: u32, y: u32, salt: u32| -> f32 {
            let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503) ^ salt.wrapping_mul(97))
                >> 9;
            (v & 0xff) as f32 / 255.0
        };
        let mut s = image::RgbImage::new(w, h);
        let mut t = image::RgbImage::new(w, h);
        let put = |img: &mut image::RgbImage, x: u32, y: u32, p: [f32; 3]| {
            img.put_pixel(
                x,
                y,
                image::Rgb([
                    (p[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (p[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (p[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                ]),
            )
        };
        for y in 0..h {
            for x in 0..w {
                let n = tex(x, y, 0);
                if y < INVENTED_ROWS {
                    let m = tex(x, y, 7);
                    put(&mut s, x, y, [0.40 + 0.22 * n, 0.44 + 0.22 * n, 0.50 + 0.22 * n]);
                    put(&mut t, x, y, [0.49 + 0.27 * m, 0.55 + 0.27 * m, 0.64 + 0.27 * m]);
                } else {
                    let sp = [0.12 + 0.16 * n, 0.11 + 0.16 * n, 0.10 + 0.16 * n];
                    put(&mut s, x, y, sp);
                    put(&mut t, x, y, [sp[0] * 1.060, sp[1] * 1.000, sp[2] * 0.955]);
                }
            }
        }
        (DynamicImage::ImageRgb8(s), DynamicImage::ImageRgb8(t))
    }

    /// The white balance the corresponding region of [`invented_half_pair`]
    /// actually carries, read as the ratio of that region's LINEAR per-channel
    /// MEANS — a statistic with no bimodality to trip over, so it is the
    /// truth the restricted solve is measured against rather than another
    /// median. Measured: 1.2181.
    const CORRESPONDING_TRUE_WB: f32 = 1.2181;
    /// …and its true exposure: the corresponding region's target is its source
    /// times a cast whose luma is 1.00, so a correct solve reads 0 EV.
    const CORRESPONDING_TRUE_EV: f32 = 0.0;

    /// Atmosphere mode on demand, through the route the zoned pass itself
    /// uses (`divergent_zone_promotes`). These tests are about what the
    /// Atmosphere solve READS, not about where a synthetic texture happens to
    /// land relative to `DIVERGENCE_GLOBAL`; promoting explicitly keeps the
    /// two questions apart, and keeps a fixture tweak from silently moving a
    /// test onto the Full path where the restriction does not exist.
    fn atmosphere_fit(
        src: &DynamicImage,
        tgt: &DynamicImage,
        provider: Option<CorrespondenceProvider<'_>>,
    ) -> FitReport {
        fit_recipe_from_promoted_with_disclosure_opts(
            src,
            tgt,
            &EditRecipe::default(),
            true,
            false,
            FitOptions { strength: crate::recipe::GradeStrength::default(), provider },
        )
    }

    /// A field confident only where [`invented_half_pair`] corresponds.
    fn corresponding_field() -> crate::correspond::CorrespondenceField {
        let g = crate::correspond::GRID;
        let split = (INVENTED_ROWS as usize * g) / 96;
        crate::correspond::CorrespondenceField {
            confidence: (0..g * g).map(|c| if c / g >= split { 1.0 } else { 0.0 }).collect(),
            ..identity_field()
        }
    }

    /// The white-balance ratio a recipe's persisted dials actually apply —
    /// `gr/gb`, the one number "warm or cold" means.
    fn wb_ratio(r: &EditRecipe) -> f32 {
        let g = render::wb_gains(
            r.as_shot_k.unwrap_or(5500.0),
            r.temperature_k.unwrap_or(5500.0),
            r.tint,
        );
        g[0] / g[2]
    }

    /// R30 R2, the directional law: when a correspondence field says a slab of
    /// the target has no counterpart in the source, the Atmosphere global
    /// white balance and exposure must be solved from the part that DOES —
    /// not from a whole-frame median that the invented slab helped define.
    ///
    /// Both dials are asserted, in the direction the fixture makes true by
    /// construction: the invented half is bluer and brighter than the half
    /// that corresponds, so dropping it must move the white balance WARMER
    /// and the exposure DOWN. Supervisor mutations M-R2-A (restriction
    /// removed), M-R2-C (the cut kept everything) and M-R2-F (`target_answered`
    /// always 1) all go red on the first assertion.
    #[test]
    fn the_atmosphere_solve_drops_target_content_no_source_answers_for() {
        let (src, tgt) = invented_half_pair();
        let field = |_: &DynamicImage,
                     _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(corresponding_field())
        };
        let whole = atmosphere_fit(&src, &tgt, None);
        let paired = atmosphere_fit(&src, &tgt, Some(&field));
        assert_eq!(whole.mode, FitMode::Atmosphere, "premise: both arms are the Atmosphere solve");
        assert_eq!(paired.mode, FitMode::Atmosphere, "the field never moves mode selection");
        match paired.atmosphere_reference {
            AtmosphereReference::SharedContent { .. } => {}
            other => panic!("the restriction must be in force, got {other:?}"),
        }
        // The whole-frame solve is defined by the invented block and lands on
        // the wrong side of neutral; the restricted one recovers the cast the
        // corresponding region actually carries.
        assert!(
            wb_ratio(&whole.recipe) < 1.0,
            "premise: the invented block owns the whole-frame median and pulls it cool: {:.4}",
            wb_ratio(&whole.recipe)
        );
        assert!(
            (wb_ratio(&paired.recipe) - CORRESPONDING_TRUE_WB).abs() < 0.06,
            "the restricted solve must recover the corresponding region's white balance \
             {CORRESPONDING_TRUE_WB:.4}: whole {:.4}, paired {:.4}",
            wb_ratio(&whole.recipe),
            wb_ratio(&paired.recipe)
        );
        assert!(
            whole.recipe.exposure_ev > 0.5,
            "premise: the invented block is brighter and owns the whole-frame exposure: {}",
            whole.recipe.exposure_ev
        );
        assert!(
            (paired.recipe.exposure_ev - CORRESPONDING_TRUE_EV).abs() < 0.20,
            "the restricted solve must recover the corresponding region's exposure \
             {CORRESPONDING_TRUE_EV:.2}: whole {}, paired {}",
            whole.recipe.exposure_ev,
            paired.recipe.exposure_ev
        );
    }

    /// R30 R2: the restriction is applied to BOTH marginals, and this is the
    /// test that says why it has to be. `median(target)/median(source)` is a
    /// ratio of two populations; moving one of them onto the shared content
    /// while the other stays on the whole frame does not repair the
    /// mismatched pairing, it exchanges it for a louder one. On this fixture
    /// the truth is `gr/gb` 1.2181 at 0.00 EV, and a one-sided cut answers:
    ///
    /// | reference population | `gr/gb` | EV     |
    /// |----------------------|---------|--------|
    /// | whole frame          | 0.911   | +0.694 |
    /// | TARGET side only     | 1.945   | −2.867 |
    /// | SOURCE side only     | 0.512   | +3.593 |
    /// | both (shipped)       | 1.216   | +0.032 |
    ///
    /// Supervisor mutations M-R2-I (source restriction dropped) and M-R2-J
    /// (target restriction dropped) each go red on the exposure bound, which
    /// no one-sided cut can satisfy — both overshoot the ±1 EV budget and peg.
    #[test]
    fn the_reference_restriction_moves_both_marginals_together() {
        let (src, tgt) = invented_half_pair();
        let field = |_: &DynamicImage,
                     _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(corresponding_field())
        };
        let paired = atmosphere_fit(&src, &tgt, Some(&field));
        // A one-sided cut cannot land here: each of them overshoots the EV
        // budget in opposite directions and pegs at ±1.00.
        assert!(
            paired.recipe.exposure_ev.abs() < 0.20,
            "a two-sided restriction reads the corresponding region's own exposure: {}",
            paired.recipe.exposure_ev
        );
        assert!(
            (wb_ratio(&paired.recipe) - CORRESPONDING_TRUE_WB).abs() < 0.06,
            "…and its own white balance: {:.4}",
            wb_ratio(&paired.recipe)
        );
        // The disclosure states both retained shares, because both were cut.
        let note = paired
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED)
            .expect("the shared-content sentence is present");
        assert!(note.args.iter().any(|(k, _)| *k == "src"));
        assert!(note.args.iter().any(|(k, _)| *k == "tgt"));
    }

    /// R30 R2, the conservation law: a pair with NO usable field is
    /// byte-for-byte the fit it always was — the restriction is reachable only
    /// through a field, and the unrestricted path reads the very slices it
    /// always read. Both silences are covered: no provider at all, and a
    /// provider that failed.
    ///
    /// What this test does NOT pin, deliberately: that the no-field path reads
    /// the EVIDENCE's own weights. Both arms here are no-field arms, so a
    /// defect in that path moves them together and stays invisible. That half
    /// is pinned by `a_field_that_drops_nothing_moves_nothing` (an identity
    /// field takes the restricted path and must land on the same dials) and by
    /// the pre-existing `wb_default_strength_is_byte_identical_to_head`.
    /// Measured, not assumed: a mutation replacing the no-field reference with
    /// a flat population goes red in four tests including those two.
    #[test]
    fn no_correspondence_field_leaves_the_atmosphere_dials_untouched() {
        let (src, tgt) = invented_half_pair();
        let bare = atmosphere_fit(&src, &tgt, None);
        let failing = |_: &DynamicImage,
                       _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Err(anyhow::anyhow!("no GPU on this machine"))
        };
        let failed = atmosphere_fit(&src, &tgt, Some(&failing));
        let r = &failed.recipe;
        assert_eq!(
            (r.exposure_ev, r.temperature_k, r.tint, r.saturation, r.contrast),
            (
                bare.recipe.exposure_ev,
                bare.recipe.temperature_k,
                bare.recipe.tint,
                bare.recipe.saturation,
                bare.recipe.contrast
            ),
            "a failed provider must change no dial against the no-provider fit"
        );
        assert_eq!(r.tone_curve, bare.recipe.tone_curve, "nor the tone curve");
        // Both silences reach the same verdict, and neither is the restricted
        // one: an absent field and a broken one are the same epistemic state.
        assert_eq!(bare.atmosphere_reference, AtmosphereReference::WholeFrame);
        assert_eq!(failed.atmosphere_reference, AtmosphereReference::WholeFrame);
    }

    /// R30 R2, the other conservation law: a field that answers for EVERY
    /// target cell authorises the empty restriction, and the empty restriction
    /// must move nothing. This is the law that keeps the mechanism honest —
    /// a restriction that changed dials when it dropped nothing would be
    /// changing the estimator, not its population.
    #[test]
    fn a_field_that_drops_nothing_moves_nothing() {
        let (src, tgt) = invented_half_pair();
        let ok = |_: &DynamicImage,
                  _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(identity_field())
        };
        let bare = atmosphere_fit(&src, &tgt, None);
        let full = atmosphere_fit(&src, &tgt, Some(&ok));
        match full.atmosphere_reference {
            AtmosphereReference::SharedContent { source, target } => assert!(
                (source - 1.0).abs() < 1e-6 && (target - 1.0).abs() < 1e-6,
                "an identity field retains both sides whole: {source} / {target}"
            ),
            other => panic!("expected the restriction in force, got {other:?}"),
        }
        assert_eq!(
            (
                full.recipe.exposure_ev,
                full.recipe.temperature_k,
                full.recipe.tint,
                full.recipe.saturation
            ),
            (
                bare.recipe.exposure_ev,
                bare.recipe.temperature_k,
                bare.recipe.tint,
                bare.recipe.saturation
            ),
            "a restriction that drops nothing must move nothing"
        );
        assert_eq!(full.recipe.tone_curve, bare.recipe.tone_curve);
    }

    /// R30 R2: a paired target too thin to be READ as a population does not
    /// get read. The whole-frame medians stand, the whole-frame sentence
    /// stands with them, and a second sentence says why it had to — the
    /// alternative (solving a global control on a corner of the frame and
    /// calling it global) is the failure this batch exists to stop, in a
    /// different costume. Supervisor mutation M-R2-B (the retention floor
    /// removed) goes red here.
    #[test]
    fn a_thin_paired_target_keeps_the_whole_frame_reading_and_says_so() {
        let g = crate::correspond::GRID;
        // Confident only on the last four rows of cells: ~8% of the target is
        // answered, far under the retention floor.
        let sliver = |_: &DynamicImage,
                      _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(crate::correspond::CorrespondenceField {
                confidence: (0..g * g)
                    .map(|c| if c / g >= g - 4 { 1.0 } else { 0.0 })
                    .collect(),
                ..identity_field()
            })
        };
        let (src, tgt) = invented_half_pair();
        let bare = atmosphere_fit(&src, &tgt, None);
        let thin = atmosphere_fit(&src, &tgt, Some(&sliver));
        match thin.atmosphere_reference {
            AtmosphereReference::Thin { source, target } => assert!(
                source < SHARED_POPULATION_MIN_RETENTION
                    || target < SHARED_POPULATION_MIN_RETENTION,
                "the fixture must put a side under the floor: {source} / {target}"
            ),
            other => panic!("expected a thin verdict, got {other:?}"),
        }
        let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
        assert!(has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION));
        assert!(has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
        assert!(!has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
        assert!(has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
        assert!(!has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_EXCLUDED));
        assert_eq!(
            (thin.recipe.exposure_ev, thin.recipe.temperature_k, thin.recipe.tint),
            (bare.recipe.exposure_ev, bare.recipe.temperature_k, bare.recipe.tint),
            "a refused restriction must leave the solve exactly as it was"
        );
    }

    /// R30 R2, added at adjudication: the retention floor is TWO-SIDED, and
    /// the target's mask is the ANSWERED bitmap rather than the source's
    /// confidence. The batch's own fixtures keep the two sides' retention
    /// equal, so neither rule was being measured.
    ///
    /// A field can be fully confident and still answer for almost none of the
    /// target: every source cell landing in one corner is what "the target is
    /// mostly generated" looks like at the limit — the island pair's 24% and
    /// `p37`'s 93% pushed all the way. There the source keeps ALL of its
    /// evidence while the target keeps a sliver, and the two rules diverge:
    ///
    ///   * a floor that accepted EITHER side would read the two medians over
    ///     a whole source and a corner of a target — a louder version of the
    ///     mismatched pairing this batch exists to repair, not a repair;
    ///   * a target mask taken from `conf` would call the target fully
    ///     retained, because every SOURCE cell is confident, and restrict on
    ///     a population it never measured.
    ///
    /// Adjudicator mutations ADJ-1 (`readable()` on `||`) and ADJ-2 (the
    /// target side masked by `conf`) both go red here; both were green
    /// against the eight tests the batch shipped with.
    #[test]
    fn a_confident_field_answering_only_a_corner_is_thin_on_the_target_side() {
        let g = crate::correspond::GRID;
        // Every cell confident, every cell landing in the same 4x4 corner:
        // 16 of the grid's cells are answered for, and the rest of the
        // target is content no source cell speaks for.
        let corner = |_: &DynamicImage,
                      _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(crate::correspond::CorrespondenceField {
                confidence: vec![1.0; g * g],
                map_x: (0..g * g).map(|c| (c % 4) as f32).collect(),
                map_y: (0..g * g).map(|c| ((c / g) % 4) as f32).collect(),
                ..identity_field()
            })
        };
        let (src, tgt) = invented_half_pair();
        let bare = atmosphere_fit(&src, &tgt, None);
        let skewed = atmosphere_fit(&src, &tgt, Some(&corner));
        match skewed.atmosphere_reference {
            AtmosphereReference::Thin { source, target } => {
                // The premise: this fixture SEPARATES the two sides. Without
                // that separation the test would pass against both mutants
                // for the same reason the batch's fixtures did.
                assert!(
                    source >= SHARED_POPULATION_MIN_RETENTION,
                    "premise: the source side must stay fat, got {source}"
                );
                assert!(
                    target < SHARED_POPULATION_MIN_RETENTION,
                    "premise: the target side must fall under the floor, got {target}"
                );
            }
            other => panic!(
                "a fat source and a cornered target must refuse, got {other:?}"
            ),
        }
        let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
        assert!(has(&skewed, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
        assert!(!has(&skewed, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
        assert_eq!(
            (skewed.recipe.exposure_ev, skewed.recipe.temperature_k, skewed.recipe.tint),
            (bare.recipe.exposure_ev, bare.recipe.temperature_k, bare.recipe.tint),
            "a refused restriction must leave the solve exactly as it was"
        );
    }

    /// R30 R2: the rationale says which of the three things happened, and
    /// never two of them. The whole-frame sentence keeps its exact old
    /// meaning, so it must be ABSENT the moment the medians came from
    /// somewhere else — and R2-lite's "defined those two controls all the
    /// same" must be absent with it, because it is then false. Supervisor
    /// mutations M-R2-D (the shared sentence pushed unconditionally) and
    /// M-R2-E (the old unpaired key kept while restricted) go red here.
    #[test]
    fn the_reference_disclosure_names_exactly_one_population() {
        let (src, tgt) = invented_half_pair();
        let field = |_: &DynamicImage,
                     _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(corresponding_field())
        };
        let paired = atmosphere_fit(&src, &tgt, Some(&field));
        let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
        assert!(has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
        assert!(
            !has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
            "the whole-frame claim must not survive a restricted solve: {}",
            paired.recipe.rationale
        );
        assert!(!has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
        assert!(has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_EXCLUDED));
        assert!(!has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
        // The share it prints is the share it dropped, to the printed
        // precision — the disclosure and the population are one fact.
        let retained = match paired.atmosphere_reference {
            AtmosphereReference::SharedContent { target, .. } => target,
            other => panic!("expected the restriction in force, got {other:?}"),
        };
        let printed = paired
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED)
            .and_then(|n| n.args.iter().find(|(k, _)| *k == "tgt").map(|(_, v)| v.clone()))
            .expect("the shared sentence carries its retained share");
        assert_eq!(printed, format!("{:.0}", retained * 100.0));
        // A whole-frame report is the mirror image, with no leakage either way.
        let whole = atmosphere_fit(&src, &tgt, None);
        assert!(has(&whole, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION));
        assert!(!has(&whole, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
        assert!(!has(&whole, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
    }

    /// R30 R2: the retained share the sentence prints is read off the SAME
    /// mask the solve drops, weighed on the EVIDENCE MASS the solve actually
    /// carries — not off a pixel count, and not off `target_unpaired`'s
    /// grid-resolution twin. One derivation, two consumers.
    ///
    /// The evidence model here is built from a real pair and then given
    /// deliberately NON-UNIFORM weights, because a derived model cannot
    /// separate the two readings: `evidence_range` sets a range's weight to
    /// its own `two_sided_share`, so the per-pixel weight is
    /// `weight / target_evidence_share` ~ 1 wherever a range is two-sided and
    /// the mass share equals the pixel share by construction. The first
    /// version of this test used a derived model, the two shares coincided
    /// (0.494 against 0.500), and supervisor mutation M-R2-H — the share
    /// counted on pixels — survived it GREEN. With the weights below the two
    /// readings are 0.90 and 0.50, and M-R2-H goes red.
    #[test]
    fn the_retained_share_is_the_evidence_mass_the_mask_keeps() {
        let (w, h) = (96u32, 64u32);
        let n = (w * h) as usize;
        let sp: Vec<[f32; 3]> = (0..n)
            .map(|i| {
                let v = 0.1 + 0.8 * ((i % 251) as f32 / 251.0);
                [v, v, v]
            })
            .collect();
        let tp: Vec<[f32; 3]> = sp.iter().map(|p| [p[0] * 0.9, p[1] * 0.9, p[2] * 0.9]).collect();
        let mut evidence = evidence_model_for(&sp, &tp, w, h);
        // Ten times the weight on the bottom half of the frame, on both
        // sides. The field below keeps exactly that half.
        let heavy = |i: usize| if i / w as usize >= (h / 2) as usize { 1.0 } else { 0.1 };
        for (i, weight) in evidence.source_weights.iter_mut().enumerate() {
            *weight = heavy(i);
        }
        for (i, weight) in evidence.target_weights.iter_mut().enumerate() {
            *weight = heavy(i);
        }
        let g = crate::correspond::GRID;
        let field = crate::correspond::CorrespondenceField {
            confidence: (0..g * g).map(|c| if c / g >= g / 2 { 1.0 } else { 0.0 }).collect(),
            ..identity_field()
        };
        let pc = correspondence_for_pair(&field, &tp, (w, h), (w, h));
        let pop = shared_content_population(&evidence, &pc).expect("a populated pair restricts");
        // The premise the test rests on: on this model the mass share and the
        // pixel share are DIFFERENT numbers, so an implementation counting the
        // wrong one cannot pass by coincidence.
        let kept_pixels = pc.target_answered[..pop.target.len()]
            .iter()
            .filter(|a| **a > 0.0)
            .count() as f32
            / pop.target.len() as f32;
        assert!(
            (pop.target_retained - kept_pixels).abs() > 0.15,
            "premise: mass share {} must be far from the pixel share {kept_pixels}",
            pop.target_retained
        );
        for (kept_v, all_v, retained, label) in [
            (&pop.source, &evidence.source_weights, pop.source_retained, "source"),
            (&pop.target, &evidence.target_weights, pop.target_retained, "target"),
        ] {
            let kept: f64 = kept_v.iter().map(|w| w.max(0.0) as f64).sum();
            let all: f64 = all_v[..kept_v.len()].iter().map(|w| w.max(0.0) as f64).sum();
            assert!(all > 0.0, "premise: the fixture carries {label} evidence");
            assert!(
                (retained as f64 - kept / all).abs() < 1e-5,
                "the {label} retained share must be the kept evidence mass: {retained} vs {}",
                kept / all
            );
            // …and every kept weight is the evidence's own, never a rescaled one.
            for i in 0..kept_v.len() {
                let w = all_v[i];
                assert!(
                    kept_v[i] == w || kept_v[i] == 0.0,
                    "a kept {label} weight must be the evidence's own: {} vs {w}",
                    kept_v[i]
                );
            }
        }
    }

    /// R30 R2: the restriction moves the reference population and NOTHING
    /// else the Atmosphere contract promises — the confidence cap, the
    /// structure-blind ruler and the absence of channel curves all survive it.
    #[test]
    fn the_atmosphere_contract_survives_the_reference_restriction() {
        let (src, tgt) = invented_half_pair();
        let field = |_: &DynamicImage,
                     _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(corresponding_field())
        };
        let paired = atmosphere_fit(&src, &tgt, Some(&field));
        assert!(
            paired.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP,
            "the atmosphere cap is not negotiable: {}",
            paired.recipe.confidence
        );
        assert!(
            paired.recipe.red_curve.is_empty()
                && paired.recipe.green_curve.is_empty()
                && paired.recipe.blue_curve.is_empty(),
            "Atmosphere mode never emits channel curves"
        );
        assert!(
            paired.structural_evidence.is_some(),
            "the structural model still travels beside the blind ruler"
        );
        let bare = atmosphere_fit(&src, &tgt, None);
        assert_eq!(
            paired.evidence.spatial_weights, bare.evidence.spatial_weights,
            "the structure-blind ruler is untouched by the restriction"
        );
    }

    /// R30 batch 1 (R2-lite): the unpaired share is a TARGET-side reading,
    /// not the source-side `coverage` under another name. An identity field
    /// answers for every target cell, so the share is zero; a field where
    /// only the top half of the source is confident (and maps into the top
    /// half of the target) leaves the bottom half of the target unanswered.
    #[test]
    fn the_unpaired_share_is_read_from_the_targets_side() {
        let g = crate::correspond::GRID;
        let (w, h) = (96u32, 64u32);
        let tp: Vec<[f32; 3]> = vec![[0.5; 3]; (w * h) as usize];
        let identity = correspondence_for_pair(&identity_field(), &tp, (w, h), (w, h));
        assert!(
            identity.target_unpaired.abs() < 1e-6,
            "an identity field answers for every target cell: {}",
            identity.target_unpaired
        );
        assert_eq!(identity.grid, (g, g));
        // Confidence only in the top half. Coverage (source side) and the
        // unpaired share (target side) must BOTH read a half — the same
        // number here, by construction, but from opposite sides.
        let half = crate::correspond::CorrespondenceField {
            confidence: (0..g * g).map(|c| if c / g < g / 2 { 1.0 } else { 0.0 }).collect(),
            ..identity_field()
        };
        let pc = correspondence_for_pair(&half, &tp, (w, h), (w, h));
        assert!((pc.coverage - 0.5).abs() < 1e-6, "coverage {}", pc.coverage);
        assert!((pc.target_unpaired - 0.5).abs() < 1e-6, "unpaired {}", pc.target_unpaired);
        // And now the case that separates them: EVERY source cell is
        // confident, but they all map onto the top half of the target. The
        // source-side coverage says 100%; the target-side share says half the
        // target had no partner — which is the reading R2-lite exists to
        // publish, and the one `coverage` cannot give.
        let piled = crate::correspond::CorrespondenceField {
            map_y: (0..g * g).map(|c| ((c / g) / 2) as f32).collect(),
            ..identity_field()
        };
        let pc = correspondence_for_pair(&piled, &tp, (w, h), (w, h));
        assert!((pc.coverage - 1.0).abs() < 1e-6, "coverage {}", pc.coverage);
        assert!(
            (pc.target_unpaired - 0.5).abs() < 1e-6,
            "a fully confident field can still leave half the target unpaired: {}",
            pc.target_unpaired
        );
    }

    /// Step-7b conservation law, half two: a field that answers "everything
    /// corresponds in place" leaves the divergent fit's DIALS untouched (the
    /// disclosure note is the only difference), and mode selection never
    /// reads the field at all.
    #[test]
    fn an_identity_field_discloses_and_changes_no_dial() {
        let ok = |_: &DynamicImage,
                  _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(identity_field())
        };
        let (src, tgt) = structural_permutation_pair();
        let plain = fit_recipe(&src, &tgt);
        let report = fit_recipe_with(&src, &tgt, FitOptions { strength: crate::recipe::GradeStrength::default(), provider: Some(&ok) });
        assert_eq!(report.mode, FitMode::Atmosphere, "mode selection never reads the field");
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_CORRESPONDENCE),
            "the measured field must be disclosed: {}",
            report.recipe.rationale
        );
        assert_eq!(
            (report.recipe.exposure_ev, report.recipe.tint, report.recipe.saturation),
            (plain.recipe.exposure_ev, plain.recipe.tint, plain.recipe.saturation),
            "an identity field must change no dial"
        );
        assert!(report.correspondence.is_some(), "the zoned passes read it from the report");
    }

    #[test]
    fn content_divergent_calibration_keeps_an_atmosphere_recipe() {
        let Some(root) = calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
        let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
        let report = fit_recipe(&source, &target);
        eprintln!(
            "CONTENT_DIVERGENT_DIAG mode={:?} d={:.4} look={:.6}->{:.6} confidence={:.3} recipe ev={:.2} temp={:?} tint={:.1} curve={} sat={:.1} detail={:.1}/{:.1}",
            report.mode,
            report.divergence.d,
            report.err_before,
            report.err_after,
            report.recipe.confidence,
            report.recipe.exposure_ev,
            report.recipe.temperature_k,
            report.recipe.tint,
            report.recipe.tone_curve.len(),
            report.recipe.saturation,
            report.recipe.clarity,
            report.recipe.texture,
        );
        assert_eq!(report.mode, FitMode::Atmosphere);
        assert!(
            report.recipe.exposure_ev.abs() > 0.001
                || report.recipe.temperature_k.is_some()
                || report.recipe.tint.abs() > 0.001
                || !report.recipe.tone_curve.is_empty()
                || report.recipe.saturation.abs() > 0.001
                || report.recipe.clarity.abs() > 0.001
                || report.recipe.texture.abs() > 0.001,
            "a content-divergent pair must return a non-empty Atmosphere recipe: {}",
            report.recipe.rationale,
        );
        assert!(report.recipe.red_curve.is_empty());
        assert!(report.recipe.green_curve.is_empty());
        assert!(report.recipe.blue_curve.is_empty());
        assert!(report.recipe.exposure_ev <= -0.5);
    }

    #[test]
    fn calibration_atmosphere_report_uses_one_population_ruler() {
        let Some(root) = calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
        let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
        let report = fit_recipe(&source, &target);
        assert_eq!(report.mode, FitMode::Atmosphere);
        assert!(report.structural_evidence.is_some());
        assert!((-1.0..=-0.5).contains(&report.recipe.exposure_ev));
        assert_eq!(report.recipe.saturation, 0.0);
        assert_eq!((report.recipe.clarity, report.recipe.texture), (0.0, 0.0));
        assert!(!report.recipe.tone_curve.is_empty());
        assert!(report.err_after < report.err_before);
        assert!(report.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
        let note = report
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE
            })
            .expect("Atmosphere reports disclose the structural ranges excluded from their ruler");
        let ranges = note
            .args
            .iter()
            .find(|(key, _)| *key == "luma_ranges")
            .map(|(_, value)| value.as_str())
            .expect("population-evidence note carries luma_ranges");
        let names_an_interior_range = ranges.split("luma[").skip(1).any(|part| {
            let Some((bounds, _)) = part.split_once(']') else { return false };
            let Some((lo, hi)) = bounds.split_once('-') else { return false };
            let (Ok(lo), Ok(hi)) = (lo.parse::<f32>(), hi.parse::<f32>()) else {
                return false;
            };
            lo >= 0.29 && hi <= 0.82
        });
        assert!(
            names_an_interior_range,
            "disclosure did not name a structural range in [0.29, 0.82]: {ranges}"
        );
    }

    #[test]
    fn calibration_atmosphere_rescore_reproduces_report_ruler() {
        let Some(root) = calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
        let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
        let solved = fit_recipe(&source, &target);
        let rescored = rescore_report(
            &source,
            &target,
            &solved.recipe,
            solved.err_before,
            &solved.notes,
        );
        assert_eq!(rescored.mode, FitMode::Atmosphere);
        assert!(rescored.structural_evidence.is_some());
        assert_eq!(rescored.err_before.to_bits(), solved.err_before.to_bits());
        assert_eq!(rescored.err_after.to_bits(), solved.err_after.to_bits());
        assert_eq!(
            rescored.recipe.confidence.to_bits(),
            solved.recipe.confidence.to_bits(),
            "solve and rescore must derive confidence from the same blind ruler"
        );
        assert_evidence_models_bit_equal(&rescored.evidence, &solved.evidence);
    }

    #[test]
    fn atmosphere_global_obeys_ev_wb_saturation_and_curve_budgets() {
        let (src, tgt) = structural_permutation_pair();
        let report = fit_recipe(&src, &tgt);
        assert_eq!(report.mode, FitMode::Atmosphere, "premise: {:?}", report.divergence);
        let r = &report.recipe;
        assert!(r.exposure_ev.abs() <= ATMOSPHERE_EV_LIMIT);
        assert!(r.saturation.abs() <= ATMOSPHERE_SAT_LIMIT);
        if let Some(k) = r.temperature_k {
            let gains = render::wb_gains(r.as_shot_k.unwrap_or(5500.0), k, r.tint);
            let lo = gains.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = gains.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            assert!(gains.iter().all(|g| {
                (ATMOSPHERE_WB_GAIN_MIN..=ATMOSPHERE_WB_GAIN_MAX).contains(g)
            }));
            assert!(hi / lo <= ATMOSPHERE_WB_GAIN_RATIO + 1e-5);
        }
        assert_eq!(r.tone_curve.len(), 5, "Atmosphere tone must stay a robust five-point map");
        for pair in r.tone_curve.windows(2) {
            let slope = (pair[1].output as f32 - pair[0].output as f32)
                / (pair[1].input as f32 - pair[0].input as f32);
            assert!(
                (ATMOSPHERE_CURVE_SLOPE_MIN - 1e-6..=ATMOSPHERE_CURVE_SLOPE_MAX + 1e-6)
                    .contains(&slope),
                "atmosphere slope {slope} escaped its budget: {:?}",
                r.tone_curve
            );
        }
    }

    #[test]
    fn atmosphere_global_never_emits_rgb_curves_and_caps_confidence() {
        let (src, tgt) = structural_permutation_pair();
        let report = fit_recipe(&src, &tgt);
        assert_eq!(report.mode, FitMode::Atmosphere);
        assert!(report.recipe.red_curve.is_empty());
        assert!(report.recipe.green_curve.is_empty());
        assert!(report.recipe.blue_curve.is_empty());
        assert!(report.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
    }

    #[test]
    fn a_divergent_sky_promotes_the_global_fit_when_it_covers_35_percent() {
        let source = synth();
        let full = fit_recipe_from_promoted(&source, &source, &EditRecipe::default(), false);
        assert_eq!(full.mode, FitMode::Full, "premise: matched content uses Full mode");
        let promoted = fit_recipe_from_promoted(&source, &source, &EditRecipe::default(), true);
        assert_eq!(promoted.mode, FitMode::Atmosphere);
        assert!(
            promoted.divergence.d < DIVERGENCE_GLOBAL,
            "the zone-share branch, not global D, must be load-bearing"
        );
        assert_eq!(DIVERGENT_COVER_PROMOTES, 0.35);
    }

    #[test]
    fn residual_curve_cannot_exceed_two_to_one_slope() {
        let cliff = vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 64, output: 20 },
            CurvePoint { input: 128, output: 40 },
            CurvePoint { input: 149, output: 98 },
            CurvePoint { input: 170, output: 193 },
            CurvePoint { input: 255, output: 255 },
        ];
        let projected = project_curve_slopes(&cliff, 0.0, RESIDUAL_SLOPE_CAP);
        assert_eq!(projected.first(), cliff.first());
        assert_eq!(projected.last(), cliff.last());
        for pair in projected.windows(2) {
            assert!(pair[1].output >= pair[0].output, "projection lost monotonicity");
            let slope = (pair[1].output as f32 - pair[0].output as f32)
                / (pair[1].input as f32 - pair[0].input as f32);
            assert!(slope <= RESIDUAL_SLOPE_CAP + 1e-6, "slope {slope}: {projected:?}");
        }
        let already_safe = vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 64, output: 48 },
            CurvePoint { input: 128, output: 128 },
            CurvePoint { input: 192, output: 208 },
            CurvePoint { input: 255, output: 255 },
        ];
        assert_eq!(
            project_curve_slopes(&already_safe, 0.0, RESIDUAL_SLOPE_CAP),
            already_safe,
            "an in-budget showcase-like curve must remain byte-identical"
        );
    }

    #[test]
    fn atmosphere_saturation_cap_is_load_bearing() {
        // A structurally divergent pair whose target ALSO demands far more
        // chroma than the +/-30 budget allows. Atmosphere reads that demand on
        // population evidence, so the fitted demand must land ON the budget;
        // without the clamp the chase would run past it.
        let (src, tgt) = structural_permutation_pair();
        let boosted = render::develop_preview(
            &tgt,
            &EditRecipe { saturation: 90.0, ..Default::default() },
        );
        let report = fit_recipe(&src, &boosted);
        assert_eq!(report.mode, FitMode::Atmosphere, "premise: {:?}", report.divergence);
        assert_eq!(ATMOSPHERE_SAT_LIMIT, 30.0, "the calibrated atmosphere saturation budget");
        assert_eq!(
            report.recipe.saturation, ATMOSPHERE_SAT_LIMIT,
            "population evidence may reach, but never exceed, the Atmosphere budget"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_SAT_PEGGED),
            "hitting the cap has to be disclosed: {}",
            report.recipe.rationale
        );
        assert!(report.notes.iter().any(|n| {
            n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE
        }));
    }

    #[test]
    fn residual_tone_curve_projects_a_cliff_through_the_real_producer() {
        // The PRODUCER, not the projector helper: `residual_curve_cannot_
        // exceed_two_to_one_slope` passes the cap into `project_curve_slopes`
        // itself, so it stays green when the constant is raised or when the
        // producer stops calling it. This drives the 4:1 upper ramp that the
        // generated-cloud fit drew and demands the shipped points obey 2:1.
        let recipe = EditRecipe::default();
        let cliff = |x: f32| {
            if x < 0.62 { x * 0.45 } else { (0.279 + (x - 0.62) * 4.0).min(1.0) }
        };
        let pts = residual_tone_curve(&recipe, &cliff);
        assert!(!pts.is_empty(), "premise: the sliders alone cannot express this map");
        assert_eq!(RESIDUAL_SLOPE_CAP, 2.0, "the calibrated residual-curve slope cap");
        for pair in pts.windows(2) {
            let slope = (pair[1].output as f32 - pair[0].output as f32)
                / (pair[1].input as f32 - pair[0].input as f32).max(1.0);
            assert!(
                slope <= 2.0 + 1e-6,
                "a shipped residual segment kept slope {slope}: {pts:?}"
            );
        }
    }

    #[test]
    fn ordinary_same_content_roundtrip_remains_in_full_fit_mode() {
        let source = synth();
        let target = render::develop_preview(
            &source,
            &EditRecipe {
                exposure_ev: 0.35,
                contrast: 18.0,
                highlights: -25.0,
                whites: 12.0,
                saturation: 15.0,
                ..Default::default()
            },
        );
        let report = fit_recipe(&source, &target);
        assert_eq!(
            report.mode,
            FitMode::Full,
            "same-content engine roundtrip diverged: {:?}",
            report.divergence
        );
    }

    #[test]
    fn atmosphere_rationale_names_unrecoverable_structure_and_discloses_d() {
        let (src, tgt) = structural_permutation_pair();
        let report = fit_recipe(&src, &tgt);
        let summary = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_SUMMARY_ATMOSPHERE)
            .expect("Atmosphere summary note");
        let disclosed = summary.args.iter().find(|(key, _)| *key == "d").unwrap().1.clone();
        assert_eq!(disclosed, format!("{:.3}", report.divergence.d));
        assert!(report.recipe.rationale.contains("structure cannot be reconstructed"));
        assert!(report.recipe.rationale.contains(&format!("D={disclosed}")));
        assert!(report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_CONFIDENCE));
    }

    #[test]
    fn identity_fit_is_near_neutral() {
        let img = synth();
        let rep = fit_recipe(&img, &img);
        let r = &rep.recipe;
        // The terminal do-no-harm reset would satisfy EVERY assertion below
        // by wiping the recipe to default, so a broken solve (exposure +3,
        // contrast +90) would look identical to a correct one. Demand that
        // the safety net did NOT fire: on an identity pair the honest solve
        // is already near-neutral, so there is nothing for it to catch.
        assert!(
            !rep.recipe.rationale.contains("do-no-harm terminal case"),
            "the identity solve must stand on its own, not on the reset: {}",
            rep.recipe.rationale
        );
        assert!(r.exposure_ev.abs() < 0.06, "exposure {}", r.exposure_ev);
        for (name, v) in [
            ("contrast", r.contrast),
            ("highlights", r.highlights),
            ("shadows", r.shadows),
            ("whites", r.whites),
            ("blacks", r.blacks),
            ("saturation", r.saturation),
        ] {
            assert!(v.abs() < 6.0, "{name} should stay near 0, got {v}");
        }
        assert!(rep.err_after < 0.02, "identity residual {}", rep.err_after);
    }

    #[test]
    fn roundtrip_recovers_tone_and_saturation() {
        // Render a KNOWN recipe through the real engine, then fit it back.
        let src = synth();
        let mut truth = EditRecipe {
            exposure_ev: 0.35,
            contrast: 18.0,
            highlights: -25.0,
            whites: 12.0,
            saturation: 15.0,
            ..Default::default()
        };
        truth.clamp();
        let target = render::develop_preview(&src, &truth);
        let rep = fit_recipe(&src, &target);
        let r = &rep.recipe;
        // The luma CDF of the target IS the engine's own tone map of the source,
        // so the solve must land close (exposure/slider trade-offs allowed).
        assert!((r.exposure_ev - 0.35).abs() < 0.20, "exposure {}", r.exposure_ev);
        assert!(r.contrast > 3.0 && r.contrast < 45.0, "contrast {}", r.contrast);
        assert!(r.highlights < -8.0 && r.highlights > -50.0, "highlights {}", r.highlights);
        assert!(r.saturation > 5.0 && r.saturation < 30.0, "saturation {}", r.saturation);
        // And the fitted recipe must actually reproduce the look through the engine.
        assert!(
            rep.err_after < (rep.err_before * 0.5).max(0.012),
            "residual {} vs before {}",
            rep.err_after,
            rep.err_before
        );
    }

    #[test]
    fn hazy_to_clean_fit_stays_sane() {
        // Regression for the 2026-07-07 real-photo failure: fitting a
        // low-contrast, low-chroma, blue-cast base toward a clean punchy
        // target produced mutually-cancelling pegged tone sliders
        // (Exposure +1.5 / Contrast −97 / Shadows −100), pegged per-band hue
        // rotations (+45) and a purple sky — while the old metric reported
        // "improved". The prior, the stage order and the correspondence gate
        // must keep every fitted control in its sane regime.
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            // a shadow-weighted blue cast at realistic haze strength (the
            // midpoint pin keeps it out of the highlights, like real haze)
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let rep = fit_recipe(&base, &clean);
        let r = &rep.recipe;
        assert!(
            r.contrast > -20.0 && r.contrast.abs() < 90.0,
            "degenerate contrast {}",
            r.contrast
        );
        assert!(
            r.shadows.abs() < 90.0 && r.whites.abs() < 90.0 && r.blacks.abs() < 90.0,
            "pegged tone sliders: sh {} wh {} bl {}",
            r.shadows,
            r.whites,
            r.blacks
        );
        assert!(r.exposure_ev.abs() <= 1.0, "runaway exposure {}", r.exposure_ev);
        // NOTE deliberately no "slider not pegged" assertion for hue: the
        // correspondence gate already rejects mismatched populations, and a
        // genuine in-gate rotation larger than the engine's ±13.5° range
        // legitimately clamps. What must hold is the RESULT (below): no band
        // of the fitted render lands tens of degrees off the target.
        assert!(
            rep.err_after < rep.err_before,
            "fit made the look worse: {} -> {}",
            rep.err_before,
            rep.err_after
        );
        assert_eq!(rep.mode, FitMode::Full);
        assert!(
            r.exposure_ev.abs()
                + r.contrast.abs()
                + r.shadows.abs()
                + r.blacks.abs()
                + r.saturation.abs()
                > 1.0,
            "same-content haze removal was incorrectly returned as a neutral recipe: {:?}",
            r
        );
        // The decisive invariant: render the fitted recipe and check every
        // populated band's centroid hue against the target — the purple-sky
        // failure class means some band lands tens of degrees off.
        let fitted = pixels_of(&render::develop_preview(&base, &rep.recipe));
        let (fb, ftot) = band_stats(&fitted);
        let (tb, ttot) = band_stats(&pixels_of(&clean));
        let mut worst = 0.0f64;
        for i in 0..8 {
            let (x, y) = (&fb[i], &tb[i]);
            if x.w / ftot < 0.015 || y.w / ttot < 0.015 {
                continue;
            }
            let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
            while d > 180.0 {
                d -= 360.0;
            }
            while d < -180.0 {
                d += 360.0;
            }
            worst = worst.max(d.abs());
        }
        assert!(worst < 15.0, "a band's hue is still {worst:.1}° off after the fit");
    }

    /// 192×128 canyon: 15.6% neutral ramp (tone evidence — without it the
    /// pale sky is the only `is_neutralish` population and the tone solve
    /// degenerates), 68.8% warm-rock ramp, 15.6% pale-blue sky. `warm` = the
    /// region-graded target: rocks red-lifted (`l^0.7`), ramp + sky IDENTICAL
    /// to the source — the grade a global cast cannot express without
    /// collateral damage.
    fn canyon(warm: bool) -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.15 + 0.80 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l] // neutral ramp
                } else if y < 112 {
                    // Rocks: the warm grade is a pure RED OFFSET — a hue move
                    // toward orange that symmetric chroma expansion (the
                    // saturation stage) cannot express, so the residual lands
                    // squarely on the red channel curve, as in the real photo.
                    let r = if warm { (0.85 * l + 0.18).min(1.0) } else { 0.85 * l };
                    [r, 0.52 * l, 0.30 * l]
                } else {
                    [0.64, 0.68, 0.73] // pale blue sky, hue ≈ 213°, chroma 0.09
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// R17: the P20 × reimagine murk, distilled — the target re-hues a
    /// luma-CONCENTRATED bright band out of the source's neutral class. The
    /// share ratio stays under 1.75× (the old gate passed and the murky fit
    /// shipped), but the leavers bend the source evidence CDF far past the
    /// contamination ceiling; the solve must fall back to full-pixel CDFs.
    #[test]
    fn a_rehued_bright_grey_band_falls_back_to_full_cdfs() {
        let n = 64 * 64;
        let mut sp: Vec<[f32; 3]> = Vec::with_capacity(n);
        let mut tp: Vec<[f32; 3]> = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / n as f32;
            if t < 0.4 {
                let l = 0.30 + 0.5 * t; // mid grey, neutral on BOTH sides
                sp.push([l, l, l]);
                tp.push([l, l, l]);
            } else if t < 0.6 {
                let l = 0.75 + 0.5 * (t - 0.4); // bright grey → re-hued vivid blue
                sp.push([l, l, l]);
                tp.push([0.3 * l, 0.5 * l, l]);
            } else {
                let l = 0.2 + 0.5 * t; // chromatic on both sides
                sp.push([l, 0.6 * l, 0.3 * l]);
                tp.push([l, 0.6 * l, 0.3 * l]);
            }
        }
        // Premise: this is the case the SHARE gate cannot see (0.6 vs 0.4 =
        // 1.5×, under 1.75×) — only the contamination measure convicts it.
        let s_share = sp.iter().filter(|p| is_neutralish(p)).count() as f32 / n as f32;
        let t_share = tp.iter().filter(|p| is_neutralish(p)).count() as f32 / n as f32;
        assert!(
            s_share.max(t_share) <= 1.75 * s_share.min(t_share),
            "premise broken: the share gate would already catch this ({s_share} vs {t_share})"
        );
        let c = neutral_gate_misprediction(&sp, &tp);
        assert!(
            c > 2.0 * NEUTRAL_MISPREDICTION_MAX,
            "the concentrated band must contaminate with margin: {c}"
        );
        let (s_cdf, t_cdf) = tone_cdf_pair(&sp, &tp);
        assert_eq!(s_cdf, luma_cdf(&sp), "source side must fall back to the full CDF");
        assert_eq!(t_cdf, luma_cdf(&tp), "target side must fall back to the full CDF");
    }

    /// R17 counterpart #1: benign one-sided inflation keeps the gate. The
    /// source's extra neutrals (a uniform desaturation — the haze-pair
    /// geometry) span the same luma ramp as the shared class, so the
    /// evidence CDF barely moves even though a sixth of the frame is
    /// neutral on the source side only.
    #[test]
    fn a_uniformly_inflated_neutral_class_keeps_the_gate() {
        let n = 64 * 64;
        let mut sp: Vec<[f32; 3]> = Vec::with_capacity(n);
        let mut tp: Vec<[f32; 3]> = Vec::with_capacity(n);
        for i in 0..n {
            let l = 0.2 + 0.6 * (i as f32 / n as f32);
            match i % 6 {
                0 => {
                    sp.push([l, l, l]); // neutral in the source only…
                    tp.push([l, 0.8 * l, 0.6 * l]); // …chromatic in the target
                }
                1..=3 => {
                    sp.push([l, l, l]); // the shared class
                    tp.push([l, l, l]);
                }
                _ => {
                    sp.push([l, 0.6 * l, 0.3 * l]); // chromatic on both sides
                    tp.push([l, 0.6 * l, 0.3 * l]);
                }
            }
        }
        let c = neutral_gate_misprediction(&sp, &tp);
        assert!(
            c < 0.5 * NEUTRAL_MISPREDICTION_MAX,
            "uniform inflation must stay clear of the ceiling: {c}"
        );
        let (s_cdf, _) = tone_cdf_pair(&sp, &tp);
        assert_ne!(s_cdf, luma_cdf(&sp), "the benign pair must stay neutral-gated");
    }

    /// R17 anchor on the LIVE haze pair: its neutral identification is
    /// genuinely broken — the haze recipe's blue cast tints the clean
    /// frame's dark greys OUT of the source-side class while the global
    /// desaturation pulls colours IN, and the gated evidence map misses the
    /// shared class by 0.13 mean luma (measured; worst at the dark ranks).
    /// The misprediction gate must fall back to full-pixel CDFs — and the
    /// fit, now solving on honest evidence, must land far below its
    /// starting error (0.0892 -> 0.0229 measured; the GATED solve under the
    /// R17 dense residual knots collapses to a do-no-harm reset, because
    /// faithful sampling faithfully implements a broken map).
    #[test]
    fn the_haze_pairs_broken_identification_falls_back_and_still_fits() {
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let pair = analysis_pair(&base, &clean);
        let (sp, tp) = (pixels_of(&pair.0), pixels_of(&pair.1));
        let m = neutral_gate_misprediction(&sp, &tp);
        assert!(
            m > NEUTRAL_MISPREDICTION_MAX,
            "premise broken: the haze pair's neutral evidence reads clean ({m:.4})"
        );
        let rep = fit_recipe(&base, &clean);
        // Measured 0.0892 -> 0.0229 (0.26×); 0.35× keeps real margin without
        // letting the win quietly rot.
        assert!(
            rep.err_after < 0.35 * rep.err_before,
            "the fallback solve must still close most of the gap ({:.4} -> {:.4})",
            rep.err_before,
            rep.err_after
        );
    }

    /// R17 counterpart #2: matched populations read ZERO contamination. The
    /// canyon pair's neutral members (ramp + pale sky) are IDENTICAL on
    /// both sides, and the returned CDFs stay neutral-gated (≠ the
    /// full-pixel CDFs, which include the rocks).
    #[test]
    fn matched_neutral_members_keep_the_gate() {
        let pair = analysis_pair(&canyon(false), &canyon(true));
        let (sp, tp) = (pixels_of(&pair.0), pixels_of(&pair.1));
        let c = neutral_gate_misprediction(&sp, &tp);
        assert!(
            c < 0.5 * NEUTRAL_MISPREDICTION_MAX,
            "premise broken: the canyon pair's neutral members diverged ({c})"
        );
        let (s_cdf, _) = tone_cdf_pair(&sp, &tp);
        assert_ne!(s_cdf, luma_cdf(&sp), "the canyon pair must stay neutral-gated");
    }

    /// R17: residual-curve knots follow the LUT's OUTPUT spacing. On a steep
    /// camera base the old fixed-x placement left a 38-u8 input gap right
    /// across the band holding a real frame's tonal mass, and the curve's
    /// piecewise-linear rendering chorded ~10/255 below the promised map
    /// inside it. The base curve here is the P20 camera calibration
    /// verbatim.
    #[test]
    fn residual_knots_stay_dense_in_the_curve_input_space() {
        let recipe = EditRecipe {
            base_curve: vec![
                [0.0, 0.0],
                [0.22091886, 0.25904202],
                [0.23851417, 0.29325512],
                [0.25317693, 0.32551318],
                [0.28152493, 0.38514173],
                [0.34115347, 0.49266863],
                [0.39687195, 0.6060606],
                [0.42033234, 0.6539589],
                [0.4848485, 0.74486804],
                [0.51808405, 0.77614856],
                [0.5474096, 0.8005865],
                [0.60117304, 0.8445748],
                [1.0, 1.0],
            ],
            ..EditRecipe::default()
        };
        let curve = residual_tone_curve(&recipe, &|x: f32| (0.9 * x + 0.02).clamp(0.0, 1.0));
        assert!(curve.len() >= 8, "a nontrivial map earns a dense curve: {} pts", curve.len());
        for w in curve.windows(2) {
            let gap = w[1].input as i32 - w[0].input as i32;
            assert!(
                gap <= 32,
                "knot gap {gap} u8 between inputs {} and {} — interpolation sag territory",
                w[0].input,
                w[1].input
            );
        }
    }

    /// The veto's discriminator is pinned on a real reconstruction and a
    /// synthetic non-no-op cast. The haze pair's accepted correction rotates
    /// pixels only INTO
    /// the target's own hue families — measured foreign-share delta ≈ 0.000)
    /// The real canyon reconstruction is upstream-refused and therefore
    /// creates 0.000000 foreign share; the synthetic canyon cast below
    /// paints the foreign population and must trigger the veto.
    ///
    /// The end-to-end verdicts live in `hazy_to_clean_fit_stays_sane` and
    /// hues ≥ 45° from everything the target contains). The end-to-end
    /// `warm_rock_cast_must_not_violet_the_pale_sky`.
    #[test]
    fn foreign_hue_veto_measures_real_canyon_reconstruction() {
        // Canyon: rebuild stage 4's exact inputs (fit minus its cast curves →
        // `cur`; curves re-derived and rendered → `with`).
        let cur2: Vec<[f32; 3]> = (0..4096)
            .map(|i| if i % 2 == 0 { [0.65, 0.20, 0.12] } else { [0.72, 0.34, 0.10] })
            .collect();
        let tp2 = cur2.clone();
        let mut with2 = cur2.clone();
        for p in with2.iter_mut().take(512) {
            *p = [0.10, 0.22, 0.75];
        }
        let cf = foreign_hue_bins(&tp2).expect("target has chromatic mass");
        let created = foreign_share(&with2, &cf) - foreign_share(&cur2, &cf);
        assert!(
            cast_paints_foreign_hues(&cur2, &with2, &tp2),
            "veto must fire on a cast that creates a foreign blue population ({created:.4})"
        );
        assert!(created > 2.0 * VETO_CREATED_SHARE, "margin eroded: created {created:.4}");
        let evidence = evidence_model(&cur2, &tp2);
        let supported = evidence.source_weights.iter().take(512).filter(|&&w| w > 0.0).count();
        let supported_total = evidence.source_weights.iter().filter(|&&w| w > 0.0).count();
        assert!(
            cast_paints_foreign_hues_weighted(&cur2, &with2, &tp2, &evidence),
            "the production evidence-weighted veto must see the supported foreign population ({supported}/512 supported, {supported_total} total)"
        );

        // The real canyon reconstruction is upstream-refused: its rendered
        // candidate is unchanged and creates zero foreign share. This is a
        // diagnostic, not the veto acceptance itself.
        let src = canyon(false);
        let tgt = canyon(true);
        let (s2, t2) = analysis_pair(&src, &tgt);
        let tp2 = pixels_of(&t2);
        let mut pre = fit_recipe(&src, &tgt).recipe;
        pre.red_curve.clear();
        pre.green_curve.clear();
        pre.blue_curve.clear();
        let cur_real = pixels_of(&render::develop_preview(&s2, &pre));
        let mut with_real = pre.clone();
        with_real.red_curve = residual_channel_curve(&cur_real, &tp2, 0);
        with_real.green_curve = residual_channel_curve(&cur_real, &tp2, 1);
        with_real.blue_curve = residual_channel_curve(&cur_real, &tp2, 2);
        let with_real = pixels_of(&render::develop_preview(&s2, &with_real));
        let cf_real = foreign_hue_bins(&tp2).expect("canyon target has chromatic mass");
        let created_real = foreign_share(&with_real, &cf_real) - foreign_share(&cur_real, &cf_real);
        eprintln!("CANYON_VETO_REAL created={created_real:.6} with_eq_cur={}", with_real == cur_real);
        assert!(created_real <= VETO_CREATED_SHARE);
        assert!(!cast_paints_foreign_hues_weighted(
            &cur_real,
            &with_real,
            &tp2,
            &evidence_model(&cur_real, &tp2),
        ));

        // The inverse case is equally important: a foreign population created
        // only inside a structurally replaced cell is not evidence about a
        // global cast and must not veto a correction supported elsewhere.
        let (w, h) = (64usize, 64usize);
        let mut cur3 = Vec::with_capacity(w * h);
        let mut tgt3 = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.30 + 0.35 * x as f32 / (w - 1) as f32;
                cur3.push([l, 0.55 * l, 0.25 * l]);
                let tl = if y < 8 {
                    if (x / 2 + y / 2) % 2 == 0 { 0.28 } else { 0.72 }
                } else {
                    l
                };
                tgt3.push([tl, 0.55 * tl, 0.25 * tl]);
            }
        }
        let mut with3 = cur3.clone();
        for p in with3.iter_mut().take(w * 8) {
            *p = [0.10, 0.22, 0.75];
        }
        assert!(cast_paints_foreign_hues(&cur3, &with3, &tgt3));
        let evidence3 = evidence_model(&cur3, &tgt3);
        assert!(
            !cast_paints_foreign_hues_weighted(&cur3, &with3, &tgt3, &evidence3),
            "unsupported invented pixels must not withhold the cast stage"
        );

        // Haze: under the marginal estimator this cast was refused — from
        // unpaired statistics, moving a source-only band is indistinguishable
        // from content mismatch. The PAIRED robust estimator changes the
        // epistemics: every blue-cast pixel has its own paired target, the
        // movement is hue-coherent with the global edit, and each moved pixel
        // is individually vouched — so the cast that empties the cast-invented
        // Red/Blue bands ships, WITH the vouched-passage disclosure beside
        // the withheld note (E-15: the veto that held for unvouched pixels
        // and the passage that was earned must both be readable). The real
        // canyon reconstruction above stays refused: its vanished population
        // is content, incoherent, unvouched.
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let report = fit_recipe(&base, &clean);
        let rec = &report.recipe;
        assert!(
            !rec.blue_curve.is_empty(),
            "the vouched paired cast must un-cast the haze: {}",
            rec.rationale
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.key == crate::rationale::keys::FIT_NOTE_VOUCHED_CONVERGENCE),
            "vouched passage through the one-sided bands must be disclosed: {}",
            rec.rationale
        );
        assert!(
            !report
                .notes
                .iter()
                .any(|note| note.key == crate::rationale::keys::FIT_NOTE_REHUE_BLOCKED),
            "a vouched coherent un-cast is not a re-hue refusal: {}",
            rec.rationale
        );
    }

    #[test]
    fn warm_rock_cast_must_not_violet_the_pale_sky() {
        // Regression for the 2026-07-09 real-machine canyon failure: the target
        // warms the frame-dominant rocks and keeps the small pale sky blue. The
        // channel-CDF cast stage answers the rocks' demand with a global red
        // lift whose 5-knot interpolation drags the sky's red up too → violet
        // sky. The aggregate acceptance gate passed it because the rotated sky
        // is CROSS-BAND invisible to the hue term (mass lands in Purple/Magenta
        // — empty in the target — and drains out of Blue: the two-sided band
        // gate skips both). The pixel-aligned hue-damage veto must reject the
        // curves; saturation alone (hue-preserving) then matches the chroma.
        let src = canyon(false);
        let tgt = canyon(true);
        let rep = fit_recipe(&src, &tgt);
        // Render the fitted recipe and audit the sky region (rows y ≥ 108).
        let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
        let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
            for x in 0..192 {
                let p = out.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue; // desaturated sky pixels carry no hue verdict
                }
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                sin += hue.sin();
                cos += hue.cos();
                n += 1.0;
            }
        }
        assert!(n > 0.0, "no chromatic sky pixels to audit — the fixture or a stage broke");
        {
            let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
            let d = (mean - 213.0 + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                d.abs() < 30.0,
                "sky hue drifted to {mean:.0}° (Δ{d:.0}°) — a violet/purple cast leaked through"
            );
        }
        // And rejecting the cast must not have made the overall fit worse.
        assert!(
            rep.err_after <= rep.err_before + 0.01,
            "fit made the look worse: {} -> {}",
            rep.err_before,
            rep.err_after
        );
    }

    /// Same geometry as [`canyon`], but the target regrades the WHOLE scene
    /// warm: rocks red-lifted AND the pale-blue sky replaced by a pale gold
    /// one ([0.92, 0.78, 0.58]: hue ≈ 35°, chroma ≈ 0.34, luma ≈ 0.80 vs the
    /// source sky's 0.67 — brighter, like the real golden-hour target). The
    /// destination hue is TARGET-NATIVE, so the foreign-hue veto stays silent
    /// — this models the 2026-07-09 real-machine failure #2 (reimagine-5):
    /// the hazy pale sky was rotated ~170° into the target's own orange.
    fn canyon_gold_target() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.15 + 0.80 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l]
                } else if y < 112 {
                    [(0.85 * l + 0.18).min(1.0), 0.52 * l, 0.30 * l]
                } else {
                    // PALE gold sky: bright golden-hour skies keep a HIGH blue
                    // channel (b ≈ 0.6) — the demanded blue curve is a gentle
                    // top-end dip (like the real pair's 255→188), not a global
                    // crush that would wake the aggregate gate on rock damage.
                    [0.92, 0.78, 0.58]
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// POLICY regression for real-machine failure #2 (2026-07-09, P21 ×
    /// reimagine-5): when the target's statistics demand rotating a large
    /// coherent chromatic region into a hue the target DOES populate (blue
    /// hazy sky → vivid target-native orange, ~170°), both existing gates
    /// pass — the foreign-hue veto by design (fit.rs "Known non-goal"), the
    /// aggregate ratio because the frame-dominant demand is genuine. From
    /// non-pixel-aligned statistics such a rotation is INDISTINGUISHABLE from
    /// content mismatch, so the policy is to refuse it: hue-preserving stages
    /// (tone + saturation) may chase the look, the cast curves may not
    /// re-hue a region. Deliberate cost: a true whole-scene regrade (sky
    /// genuinely gone gold) is not chased either — that expressiveness
    /// belongs to the zoned fit, not to global curves.
    #[test]
    fn cast_must_not_rotate_the_sky_into_a_target_native_hue() {
        let src = canyon(false);
        let tgt = canyon_gold_target();
        let rep = fit_recipe(&src, &tgt);
        let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
        let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
            for x in 0..192 {
                let p = out.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue;
                }
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                sin += hue.sin();
                cos += hue.cos();
                n += 1.0;
            }
        }
        assert!(n > 0.0, "no chromatic sky pixels to audit — the fixture or a stage broke");
        {
            let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
            let d = (mean - 213.0 + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                d.abs() < 30.0,
                "sky hue rotated to {mean:.0}° (Δ{d:.0}°) — a target-native re-hue leaked through"
            );
        }
        assert!(
            rep.recipe.red_curve.is_empty()
                && rep.recipe.green_curve.is_empty()
                && rep.recipe.blue_curve.is_empty(),
            "the whole-scene regrade's cast curves must be withheld"
        );
        assert!(
            rep.err_after <= rep.err_before + 0.01,
            "fit made the look worse: {} -> {}",
            rep.err_before,
            rep.err_after
        );
    }

    /// The REAL-pair geometry (2026-07-09 #2, P21 × reimagine-5), where
    /// the rotation gate is the UNIQUE rejector — measured on this fixture:
    /// stage-4 ratio 0.450 (aggregate gate PASSES: crushing blue genuinely
    /// fixes the channel means frame-wide), foreign-hue veto false (the
    /// destination orange is target-native), rotation gate true (the hazy
    /// pale-blue sky re-hues ~170°). Unwiring the rotation gate from
    /// `fit_cast_stage` flips this test (curves accepted → orange sky),
    /// which the synthetic `canyon` pairs cannot detect: there the re-hue
    /// also damages the aggregate, so the ratio gate rejects redundantly.
    fn hazy_canyon_source() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.25 + 0.60 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l]
                } else if y < 112 {
                    // hazy desaturated rocks: warm but muted
                    [0.95 * l + 0.03, 0.88 * l + 0.03, 0.80 * l + 0.04]
                } else {
                    [0.60, 0.63, 0.67] // hazy pale-blue sky, hue ≈ 214°
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    fn vivid_warm_target() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.25 + 0.60 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l]
                } else if y < 112 {
                    [(1.05 * l + 0.15).min(1.0), 0.55 * l, 0.30 * l] // vivid warm rocks
                } else {
                    [0.92, 0.72, 0.48] // vivid gold sky
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// The Cornwall shape, distilled to the one property the canyon family
    /// cannot express: a large SINGLE-HUED sky whose LUMINANCE ramps from
    /// zenith to horizon, over warm ground the target lifts further. The
    /// canyon skies are one flat colour, so their curves cannot sort them —
    /// which is exactly why the pixel-aligned gates were enough there and
    /// were not enough on a photograph.
    ///
    /// Measured on the real pair before the fixture was drawn (2026-09-01):
    /// the Cornwall sky's hue holds within 1.6° across luminance octiles in
    /// the source, the target and the no-cast fit, and the admitted curves
    /// fan it to 33.1° in the delivered render — 226.8° in the dark half,
    /// 193.8° in the bright clouds.
    fn coast(warm: bool) -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let p = if y < 64 {
                    // Sky: hue ≈ 214° at every level, luminance 0.22 → 0.90.
                    let level = 0.22 + 0.68 * y as f32 / 63.0;
                    if warm {
                        [0.66 * level, 0.80 * level, 1.00 * level]
                    } else {
                        [0.74 * level, 0.85 * level, 1.00 * level]
                    }
                } else {
                    // Ground: a warm ramp the target red-lifts, exactly the
                    // demand the channel-CDF answers with a global cast.
                    let level = 0.15 + 0.70 * x as f32 / (w - 1) as f32;
                    if warm {
                        [(0.85 * level + 0.12).min(1.0), 0.52 * level, 0.30 * level]
                    } else {
                        [0.85 * level, 0.52 * level, 0.30 * level]
                    }
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// Widest circular gap between the mean hues of a band's luma octiles —
    /// the coherence a viewer reads as "the sky is one colour", measured on
    /// a DELIVERED render rather than on the census, so the end-to-end claim
    /// is about the picture and not about the gate's own arithmetic.
    fn hue_spread_across_luma(img: &DynamicImage, rows: std::ops::Range<u32>) -> f64 {
        const OCTILES: usize = 8;
        let rgb = img.to_rgb8();
        let mut acc = [(0.0f64, 0.0f64, 0.0f64); OCTILES];
        for y in rows {
            for x in 0..rgb.width() {
                let p = rgb.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue; // no hue verdict from a desaturated pixel
                }
                let octile =
                    ((((r + g + b) / 3.0) * OCTILES as f32) as usize).min(OCTILES - 1);
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                acc[octile].0 += hue.sin();
                acc[octile].1 += hue.cos();
                acc[octile].2 += 1.0;
            }
        }
        let total: f64 = acc.iter().map(|a| a.2).sum();
        assert!(total > 0.0, "the audited band must contain chromatic pixels");
        let means: Vec<f64> = acc
            .iter()
            .filter(|a| a.2 >= total * 0.05)
            .map(|a| a.0.atan2(a.1).to_degrees().rem_euclid(360.0))
            .collect();
        let mut worst = 0.0f64;
        for (i, a) in means.iter().enumerate() {
            for b in &means[i + 1..] {
                worst = worst.max(((b - a + 540.0).rem_euclid(360.0) - 180.0).abs());
            }
        }
        worst
    }

    /// Rebuild the exact candidate stage 4 judged for a pair: the fitted
    /// recipe minus its channel curves (`cur`) and the curves re-derived on
    /// that state (`with`). Same reconstruction the foreign-hue and rotation
    /// pin tests use, shared so the three read one census.
    struct CastCandidate {
        /// The stage's input render: the fitted recipe minus its curves.
        cur: Vec<[f32; 3]>,
        /// The same render with the curves re-derived on `cur`.
        with_px: Vec<[f32; 3]>,
        /// The target's analysis pixels, in the source's geometry.
        tp: Vec<[f32; 3]>,
        /// The candidate recipe, so a test can assert a cast was demanded.
        with: EditRecipe,
    }

    fn cast_stage_candidate(src: &DynamicImage, tgt: &DynamicImage) -> CastCandidate {
        cast_stage_candidate_from(src, tgt, &EditRecipe::default())
    }

    fn cast_stage_candidate_from(
        src: &DynamicImage,
        tgt: &DynamicImage,
        base: &EditRecipe,
    ) -> CastCandidate {
        let (s, t) = analysis_pair(src, tgt);
        let tp = pixels_of(&t);
        let mut pre = fit_recipe_from(src, tgt, base).recipe;
        pre.red_curve = Vec::new();
        pre.green_curve = Vec::new();
        pre.blue_curve = Vec::new();
        let cur = pixels_of(&render::develop_preview(&s, &pre));
        let mut with = pre.clone();
        with.red_curve = residual_channel_curve(&cur, &tp, 0);
        with.green_curve = residual_channel_curve(&cur, &tp, 1);
        with.blue_curve = residual_channel_curve(&cur, &tp, 2);
        let with_px = pixels_of(&render::develop_preview(&s, &with));
        CastCandidate { cur, with_px, tp, with }
    }

    /// The deliberate cost the fan gate names, drawn so it can be MEASURED:
    /// a target that genuinely lights one region at two colour temperatures.
    ///
    /// Same geometry as [`coast`], ground untouched, but the sky's hue is a
    /// function of its own brightness — the dark end cooled and the bright
    /// end warmed by 25° each, so the target itself carries a 50° hue fan
    /// across luminance. The three channel curves can express exactly that
    /// (it is a per-level, per-channel move), which is why the fit reaches
    /// for it; and every milder version of those curves reproduces less of
    /// it, so the projection has nothing to trade. This is the pair where
    /// the rescue must give up and the refusal stand.
    fn two_temperature_coast() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let p = if y < 64 {
                    let level = 0.22 + 0.68 * y as f32 / 63.0;
                    // Green rides the level: 0.958 at the dark end (hue
                    // ≈ 190°) to 0.742 at the bright end (≈ 240°), so the
                    // target's own sky carries a 50° fan across luminance.
                    let green = 0.9584 - 0.2167 * (y as f32 / 63.0);
                    [0.74 * level, green * level, 1.00 * level]
                } else {
                    let level = 0.15 + 0.70 * x as f32 / (w - 1) as f32;
                    [0.85 * level, 0.52 * level, 0.30 * level]
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// The projection's readings off a finished report, by note key.
    fn projection_arg(report: &FitReport, key: &str, arg: &str) -> Option<f32> {
        report
            .notes
            .iter()
            .find(|n| n.key == key)
            .and_then(|n| n.args.iter().find(|(k, _)| *k == arg))
            .and_then(|(_, v)| v.parse::<f32>().ok())
    }

    /// v1.2.3 — the fourth gate, and the failure class that shipped in
    /// v1.2.2 on the Cornwall showcase pair. Three independent monotone
    /// channel maps sort a single-hued region into a hue FAN by luminance;
    /// no pixel travels 75°, every destination is target-native, the
    /// region's mean hue barely moves (218.3° → 217.6° on the real pair) —
    /// so all three earlier gates read clean and the tint shipped.
    ///
    /// CAST-2 (user ruling 2026-09-01): the gate still convicts, but the
    /// stage no longer throws the cast away. It shrinks the three curves
    /// along the projection path until the fan clears `FAN_PROJECT_DEG` and
    /// ships THAT. So this test's subject is the conviction and the rescue
    /// together: the fitted curves must never reach the frame, and what does
    /// reach it must read inside the target.
    #[test]
    fn cast_curves_that_fan_a_coherent_sky_are_shrunk_not_shipped() {
        let src = coast(false);
        let tgt = coast(true);
        let c = cast_stage_candidate(&src, &tgt);
        let (cur, with_px, tp) = (c.cur, c.with_px, c.tp);
        assert!(!c.with.red_curve.is_empty(), "premise broken: no cast demanded");
        let evidence = evidence_model(&cur, &tp);

        // PREMISES — the three pre-v1.2.3 gates are silent here, which is
        // the whole reason this gate exists. If a fixture drift wakes one of
        // them the assertions below stop testing the fan gate, so they fail
        // with their numbers rather than passing for the wrong reason.
        let rehued = rehued_share_weighted(&cur, &with_px, &evidence);
        assert!(
            rehued < 0.1 * ROT_SHARE,
            "premise broken: the rotation budget sees this ({rehued:.4})"
        );
        assert!(
            !cast_paints_foreign_hues_weighted(&cur, &with_px, &tp, &evidence),
            "premise broken: the foreign-hue veto fires — the destinations should be target-native"
        );

        // THE reading, and its margin over the threshold.
        let (share, fan, delivered_by_census) = hue_fan_weighted(&cur, &with_px, &evidence)
            .expect("the sky is a region-sized hue class");
        assert!(share > 0.5, "premise broken: the sky class is not region-sized ({share:.3})");
        assert!(fan > 2.5 * FAN_DEG, "margin eroded: fan {fan:.1}° against {FAN_DEG}°");

        // END TO END: the FITTED curves never ship — what ships is the
        // projection, and the note says so with the numbers behind it.
        let rep = fit_recipe(&src, &tgt);
        assert!(
            rep.recipe.red_curve != c.with.red_curve
                || rep.recipe.green_curve != c.with.green_curve
                || rep.recipe.blue_curve != c.with.blue_curve,
            "the fanning curves must not ship as fitted: {}",
            rep.recipe.rationale
        );
        assert!(
            !rep.recipe.red_curve.is_empty(),
            "…and the projection must have found a milder cast to ship: {}",
            rep.recipe.rationale
        );
        let arg = |key: &str, name: &str| {
            projection_arg(&rep, key, name)
                .unwrap_or_else(|| panic!("the note carries {name}: {}", rep.recipe.rationale))
        };
        use crate::rationale::keys;
        assert!(arg(keys::FIT_NOTE_CAST_PROJECTED, "fan_before") >= FAN_DEG,
            "the disclosed fan is the one that convicted");
        assert!(arg(keys::FIT_NOTE_CAST_PROJECTED, "share") > 0.5,
            "the disclosed share is the class that fanned");
        assert_eq!(arg(keys::FIT_NOTE_CAST_PROJECTED, "limit"), FAN_DEG,
            "the note names the limit it measured against");
        let t = arg(keys::FIT_NOTE_CAST_PROJECTED, "t");
        assert!((0.0..1.0).contains(&t), "the shipped cast is milder than the fitted one (t {t})");
        assert!(
            arg(keys::FIT_NOTE_CAST_PROJECTED_FAN, "fan_after") <= FAN_PROJECT_DEG,
            "the projection must reach its own target, not just the refusal line"
        );
        // …and it discloses at least what an ADMISSION does: the re-hued
        // share on its own head note, and the admission's own foreign-hue
        // clause beside it (measured or not-measurable, never a fabricated
        // 0.000). These are invented curves, so the two pixel-aligned
        // readings matter here more than they do on a measured cast.
        assert!(
            arg(keys::FIT_NOTE_CAST_PROJECTED, "rehued") < ROT_SHARE,
            "the projected cast's re-hued share is disclosed, and it passed"
        );
        assert!(
            rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED_FOREIGN
                || n.key == keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA),
            "the projection must carry the foreign-hue clause too: {}",
            rep.recipe.rationale
        );
        // One sentence per outcome: a projected cast is not also an admitted
        // one. (The exclusivity itself is pinned in
        // `a_projected_cast_is_never_also_disclosed_as_an_admitted_one`.)
        assert!(
            !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
            "a projected cast must not also claim admission: {}",
            rep.recipe.rationale
        );

        // …and the DELIVERED sky: the picture claim, not the census's own
        // arithmetic. TWO bars, and the first is the one this test carried
        // BEFORE the projection existed — `FAN_DEG`, the refusal line: a
        // rescued cast has to leave the delivered frame no worse than the
        // widest fan the gate would have let through unprojected. It is
        // deliberately NOT widened to the projection's own target plus one
        // class width (7.5 + 15 = 22.5°, which is 50% looser than FAN_DEG
        // itself); the fixture does not need the room. Measured 2026-09-02 on
        // this tree: 14.58° delivered, 0.42° of margin under the bar, against
        // the 42.4° the fitted curves would have delivered.
        let delivered = render::develop_preview(&src, &rep.recipe);
        let spread = hue_spread_across_luma(&delivered, 0..64);
        assert!(
            spread < FAN_DEG as f64,
            "the delivered sky fanned {spread:.1}° across luminance, against the \
             {FAN_DEG}° refusal line this bar is (measured 14.58°, 0.42° of margin)"
        );
        assert!(
            spread < 0.5 * delivered_by_census as f64,
            "the projection must be most of the way back from the fitted fan \
             ({spread:.1}° delivered against the fitted census's {delivered_by_census:.1}°)"
        );
        assert!(
            rep.err_after <= rep.err_before + 0.01,
            "fit made the look worse: {} -> {}",
            rep.err_before,
            rep.err_after
        );
    }

    /// The other half of the ruling: when the projection cannot pay, the
    /// refusal stands — and now says the rescue was tried.
    ///
    /// [`two_temperature_coast`] is the case the fan gate deliberately
    /// refuses and the release notes call unmeasured: a target whose sky
    /// really is two colour temperatures. Every point on the projection path
    /// reproduces a proportional share of that fan, so there is no `t` that
    /// both clears `FAN_PROJECT_DEG` and buys more than the fit's own
    /// quantisation — and the stage keeps its hands off the frame instead of
    /// shipping a cast that is neither the target's look nor honest about it.
    #[test]
    fn a_projection_that_cannot_clear_the_target_leaves_the_refusal_standing() {
        let src = coast(false);
        let tgt = two_temperature_coast();
        let c = cast_stage_candidate(&src, &tgt);
        let evidence = evidence_model(&c.cur, &c.tp);
        assert!(!c.with.red_curve.is_empty(), "premise broken: no cast demanded");
        let (_, fan, _) = hue_fan_weighted(&c.cur, &c.with_px, &evidence)
            .expect("premise broken: the two-temperature sky is not a region-sized class");
        assert!(fan >= FAN_DEG, "premise broken: the fitted cast does not fan ({fan:.1}°)");

        let rep = fit_recipe(&src, &tgt);
        assert!(
            rep.recipe.red_curve.is_empty()
                && rep.recipe.green_curve.is_empty()
                && rep.recipe.blue_curve.is_empty(),
            "the cast must be withheld when the projection cannot clear: {}",
            rep.recipe.rationale
        );
        use crate::rationale::keys;
        assert!(
            rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_HUE_FANNED),
            "the refusal must still disclose: {}",
            rep.recipe.rationale
        );
        assert!(
            !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_PROJECTED),
            "a refused cast must not also claim a projection: {}",
            rep.recipe.rationale
        );
        // The refusal sentence now owes the reader the fact that the cheaper
        // answer was tried; a refusal that does not say so invites the ask.
        let rendered = crate::rationale::render_one(
            rep.notes.iter().find(|n| n.key == keys::FIT_NOTE_CAST_HUE_FANNED).unwrap(),
        );
        assert!(
            rendered.contains("Shrinking them"),
            "the refusal must say the projection was tried: {rendered}"
        );
    }

    /// The search bisects on `t` for the LARGEST value that clears, which is
    /// only the right thing to look for if the fan GROWS with `t`. Measured
    /// rather than assumed: eleven points on the path, on the fixture whose
    /// fan is the reason the gate exists.
    #[test]
    fn the_projected_fan_grows_with_t() {
        let src = coast(false);
        let tgt = coast(true);
        let (s_img, _) = analysis_pair(&src, &tgt);
        let c = cast_stage_candidate(&src, &tgt);
        let evidence = evidence_model(&c.cur, &c.tp);
        let fitted = [
            c.with.red_curve.as_slice(),
            c.with.green_curve.as_slice(),
            c.with.blue_curve.as_slice(),
        ];
        let mut base = c.with.clone();
        let mut seen: Vec<(f32, f32)> = Vec::new();
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            let [red, green, blue] = projected_cast_curves(fitted, t);
            base.red_curve = red;
            base.green_curve = green;
            base.blue_curve = blue;
            let px = pixels_of(&render::develop_preview(&s_img, &base));
            let fan = hue_fan_weighted(&c.cur, &px, &evidence)
                .map(|(_, added, _)| added)
                .unwrap_or(0.0);
            seen.push((t, fan));
        }
        for pair in seen.windows(2) {
            let ((t0, f0), (t1, f1)) = (pair[0], pair[1]);
            assert!(
                f1 >= f0 - 0.5,
                "the fan must not shrink as the cast is restored: \
                 t {t0:.1} → {f0:.1}°, t {t1:.1} → {f1:.1}° (whole ladder {seen:?})"
            );
        }
        let (_, first) = seen[0];
        let (_, last) = seen[seen.len() - 1];
        assert!(
            last > first + 2.0 * FAN_DEG,
            "premise broken: the path does not span the fan ({first:.1}° → {last:.1}°)"
        );
    }

    /// The bottom half of the path carries NO chromatic difference: at
    /// `t ≤ 0.5` the three channels hold one and the same curve, and at
    /// `t = 0` that curve is the identity — which is why the search always
    /// has a well-defined floor and why a projection can never be worse than
    /// the refusal it replaces.
    #[test]
    fn the_bottom_of_the_projection_path_is_one_curve_then_none() {
        let red = vec![
            CurvePoint { input: 0, output: 23 },
            CurvePoint { input: 128, output: 115 },
            CurvePoint { input: 255, output: 179 },
        ];
        let green = vec![
            CurvePoint { input: 0, output: 56 },
            CurvePoint { input: 128, output: 105 },
            CurvePoint { input: 255, output: 189 },
        ];
        let blue = vec![
            CurvePoint { input: 0, output: 50 },
            CurvePoint { input: 128, output: 107 },
            CurvePoint { input: 255, output: 209 },
        ];
        let fitted = [red.as_slice(), green.as_slice(), blue.as_slice()];

        let [r1, g1, b1] = projected_cast_curves(fitted, 1.0);
        assert_eq!((r1, g1, b1), (red.clone(), green.clone(), blue.clone()),
            "t = 1 must reproduce the fitted curves byte for byte");

        let [r0, g0, b0] = projected_cast_curves(fitted, 0.0);
        for c in [&r0, &g0, &b0] {
            assert!(c.iter().all(|p| p.input == p.output), "t = 0 must be the identity: {c:?}");
        }
        assert!(cast_curves_are_identity(&[r0, g0, b0]));

        // …and everywhere in the bottom half the three curves are EQUAL, so
        // no chromatic difference between the channels survives at all.
        for step in 0..=5 {
            let t = step as f32 / 10.0;
            let [r, g, b] = projected_cast_curves(fitted, t);
            assert_eq!(r, g, "t = {t}: red and green must be one shared curve");
            assert_eq!(g, b, "t = {t}: green and blue must be one shared curve");
        }
        // A channel the fit left EMPTY comes back EMPTY wherever the
        // projection leaves it on the identity — `t = 1` above all, where the
        // whole promise is to reproduce the fitted curves byte for byte.
        // Without this the projection handed an unfitted channel back as five
        // dead knots, and `cast_curves_are_identity` could not catch it
        // because it only bails when ALL THREE channels are dead.
        let empty: Vec<CurvePoint> = Vec::new();
        let mixed = [red.as_slice(), empty.as_slice(), blue.as_slice()];
        let [mr, mg, mb] = projected_cast_curves(mixed, 1.0);
        assert_eq!(
            (mr, mg, mb),
            (red.clone(), empty.clone(), blue.clone()),
            "t = 1 must reproduce an unfitted channel as unfitted, not as an identity curve"
        );
        // …and a channel the projection MOVES off the identity is emitted, so
        // emptying is a statement about the curve and not about the channel.
        let moved = projected_cast_curves(mixed, 0.75);
        assert!(
            !moved[1].is_empty(),
            "the middle channel is off the identity at t = 0.75 and must ship: {:?}",
            moved[1]
        );
        assert!(!cast_curves_are_identity(&moved));

        // At the midpoint that shared curve is the per-knot MEAN of the three
        // — the "common shape" the projection is named for.
        let [mid, _, _] = projected_cast_curves(fitted, 0.5);
        assert_eq!(
            mid,
            vec![
                CurvePoint { input: 0, output: 43 },
                CurvePoint { input: 128, output: 109 },
                CurvePoint { input: 255, output: 192 },
            ],
            "t = 0.5 must be the per-knot mean of the three fitted outputs"
        );
    }

    /// A rescore re-renders and re-scores; it does not re-run the gates, so
    /// every gate fact has to ride its own note back. The projection's do
    /// too — otherwise a rescored recipe would keep the shrunk curves and
    /// lose the only sentence that says why they are shrunk.
    #[test]
    fn a_rescored_projection_carries_its_own_readings_back() {
        use crate::rationale::keys;
        let src = coast(false);
        let tgt = coast(true);
        let solved = fit_recipe(&src, &tgt);
        let head = |r: &FitReport, arg: &str| projection_arg(r, keys::FIT_NOTE_CAST_PROJECTED, arg);
        assert!(
            head(&solved, "t").is_some(),
            "premise broken: this pair no longer projects: {}",
            solved.recipe.rationale
        );
        let rescored =
            rescore_report(&src, &tgt, &solved.recipe, solved.err_before, &solved.notes);
        for arg in ["share", "fan_before", "t", "ratio", "bound", "rehued"] {
            assert_eq!(
                head(&rescored, arg),
                head(&solved, arg),
                "the rescore must carry {arg} back, not re-invent or drop it"
            );
        }
        assert_eq!(
            projection_arg(&rescored, keys::FIT_NOTE_CAST_PROJECTED_FAN, "fan_after"),
            projection_arg(&solved, keys::FIT_NOTE_CAST_PROJECTED_FAN, "fan_after"),
            "…and the fan clause with it"
        );
        assert_eq!(
            projection_arg(&rescored, keys::FIT_NOTE_CAST_ADMITTED_FOREIGN, "foreign"),
            projection_arg(&solved, keys::FIT_NOTE_CAST_ADMITTED_FOREIGN, "foreign"),
            "…and the foreign clause the projection borrows from the admission"
        );
        // …and it does not ALSO claim admission on the way back.
        assert!(
            !rescored.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
            "a rescored projection must not become an admission: {}",
            rescored.recipe.rationale
        );
    }

    /// A rescue has to be worth having. The four gates decide whether a cast
    /// the fit MEASURED may ship; whether a milder one the fit INVENTED is
    /// worth shipping is the projection's own question, and the answer is
    /// the fit's own quantisation budget: below [`FIT_QUANT`] of absolute
    /// look error a difference is not a difference (the terminal do-no-harm
    /// check says so with the same constant), and the stage's standing
    /// doctrine is that marginal gain does not earn regional risk.
    ///
    /// Driven through a synthetic gate so the bar is exercised alone, with
    /// every other reading held clean.
    #[test]
    fn a_projection_worth_less_than_the_fits_own_quantisation_is_not_shipped() {
        let img = synth();
        let red = vec![
            CurvePoint { input: 0, output: 23 },
            CurvePoint { input: 255, output: 179 },
        ];
        let green = vec![
            CurvePoint { input: 0, output: 56 },
            CurvePoint { input: 255, output: 189 },
        ];
        let blue = vec![
            CurvePoint { input: 0, output: 50 },
            CurvePoint { input: 255, output: 209 },
        ];
        let fitted = [red.as_slice(), green.as_slice(), blue.as_slice()];
        let clean_gate = |ratio: f32| {
            move |_: &[[f32; 3]]| CastOutcome {
                readings: Some(CastReadings {
                    ratio,
                    bound: CAST_ACCEPT_RATIO,
                    foreign: Some(0.0),
                    rehued: 0.0,
                    fan: Some(0.0),
                }),
                ..CastOutcome::default()
            }
        };
        // err_without = 0.05. A ratio of 0.99 buys 0.0005 of look error —
        // under FIT_QUANT (0.0018), so nothing ships…
        let mut recipe = EditRecipe::default();
        assert!(
            search_cast_projection(&img, &mut recipe, fitted, 0.05, clean_gate(0.99)).is_none(),
            "a rescue worth 0.0005 of look error must not ship"
        );
        // …and 0.90 buys 0.005, which does.
        let mut recipe = EditRecipe::default();
        let won = search_cast_projection(&img, &mut recipe, fitted, 0.05, clean_gate(0.90));
        assert!(won.is_some(), "a rescue worth 0.005 of look error must ship");
        assert!(
            !recipe.red_curve.is_empty(),
            "…and the recipe must come back carrying it"
        );
    }

    /// The 4a' and 4b loops call `fit_cast_stage` repeatedly, so the rescue
    /// has to be deterministic AND idempotent under re-fits or the loops
    /// could not converge. Both halves, on the fixture that projects.
    #[test]
    fn the_projection_is_deterministic_and_idempotent() {
        let once = fit_recipe(&coast(false), &coast(true)).recipe;
        let twice = fit_recipe(&coast(false), &coast(true)).recipe;
        assert_eq!(once.red_curve, twice.red_curve, "the same pair must fit the same cast");
        assert_eq!(once.green_curve, twice.green_curve);
        assert_eq!(once.blue_curve, twice.blue_curve);
        assert_eq!(once.rationale, twice.rationale, "…and disclose it the same way");

        // Idempotence: a curve set already ON the path is its own `t = 1`.
        let c = cast_stage_candidate(&coast(false), &coast(true));
        let fitted = [
            c.with.red_curve.as_slice(),
            c.with.green_curve.as_slice(),
            c.with.blue_curve.as_slice(),
        ];
        let projected = projected_cast_curves(fitted, 0.37);
        let again = projected_cast_curves(
            [&projected[0], &projected[1], &projected[2]],
            1.0,
        );
        assert_eq!(again, projected, "projecting a projected cast at t = 1 must change nothing");
    }

    /// One sentence per outcome. A projected cast writes the projection's
    /// notes and NOT the admission's, and the exclusivity is structural (the
    /// two `SolveFacts` fields), not a convention at the two push sites.
    #[test]
    fn a_projected_cast_is_never_also_disclosed_as_an_admitted_one() {
        use crate::rationale::keys;
        for (name, src, tgt) in [
            ("projected", coast(false), coast(true)),
            ("refused", coast(false), two_temperature_coast()),
            ("admitted", haze_pair().0, haze_pair().1),
        ] {
            let rep = fit_recipe(&src, &tgt);
            let has = |key: &str| rep.notes.iter().any(|n| n.key == key);
            assert!(
                !(has(keys::FIT_NOTE_CAST_PROJECTED) && has(keys::FIT_NOTE_CAST_ADMITTED)),
                "{name}: one cast, two accounts of itself: {}",
                rep.recipe.rationale
            );
            assert!(
                !(has(keys::FIT_NOTE_CAST_PROJECTED) && has(keys::FIT_NOTE_CAST_HUE_FANNED)),
                "{name}: projected and refused at once: {}",
                rep.recipe.rationale
            );
            // …and the projection's two notes travel together.
            assert_eq!(
                has(keys::FIT_NOTE_CAST_PROJECTED),
                has(keys::FIT_NOTE_CAST_PROJECTED_FAN)
                    || has(keys::FIT_NOTE_CAST_PROJECTED_FAN_NA),
                "{name}: the projection's fan clause went missing: {}",
                rep.recipe.rationale
            );
            // One head note per OUTCOME, whichever head it is — the strength
            // budget's admission sentence counts. It cannot fire on a
            // projected cast at the shipped calibration (the gain bar forces
            // ratio < 1, this needs ratio > CAST_ACCEPT_RATIO), so this pins
            // the guard rather than a behaviour anyone can reach today.
            assert!(
                !(has(keys::FIT_NOTE_CAST_PROJECTED)
                    && has(keys::FIT_NOTE_CAST_ADMITTED_BY_STRENGTH)),
                "{name}: a projected cast also claimed admission by strength: {}",
                rep.recipe.rationale
            );
            // The foreign-hue clause is SHARED between the two heads — one
            // sentence, one translation — so on a projected pair it must be
            // PRESENT while the admission HEAD stays absent. A shared clause
            // is not a shared verdict.
            if has(keys::FIT_NOTE_CAST_PROJECTED) {
                assert!(
                    has(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN)
                        || has(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA),
                    "{name}: the projection dropped the foreign-hue clause: {}",
                    rep.recipe.rationale
                );
                assert!(
                    !has(keys::FIT_NOTE_CAST_ADMITTED),
                    "{name}: …and the admission HEAD must still be absent: {}",
                    rep.recipe.rationale
                );
            }
        }
    }

    /// The projection does not step around the strength budget: the milder
    /// candidate is judged against the bound the path was GIVEN, exactly as
    /// the fitted cast would have been, and the note quotes that bound.
    #[test]
    fn a_projected_cast_is_judged_by_the_strength_budgets_bound() {
        use crate::rationale::keys;
        // End to end at the shipped default: the bound in the note is the
        // budget's, not the `CAST_ACCEPT_RATIO` anchor by coincidence.
        let rep = fit_recipe(&coast(false), &coast(true));
        let bound = projection_arg(&rep, keys::FIT_NOTE_CAST_PROJECTED, "bound")
            .unwrap_or_else(|| panic!("premise broken: no projection here: {}", rep.recipe.rationale));
        assert_eq!(
            bound,
            FitBudget::for_strength(crate::recipe::GradeStrength::default()).cast_ratio,
            "the projection must quote the bound the strength budget set"
        );

        // …and at strength 1.0, END TO END, the same pair quotes the WIDENED
        // bound. That is the discriminating arm: the two stops differ ONLY in
        // the budget, so the assertion cannot pass by coincidence the way a
        // single-stop reading against `CAST_ACCEPT_RATIO` can (at the default
        // the two numbers are both 2.0). The budget threads from the panel
        // dial through `full_cast_accept_ratio` into the gate that judges each
        // projected candidate. Measured 2026-09-02: this pair projects at both
        // stops — t = 0.659 at the default, t = 0.399 at 1.0.
        let widened = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).cast_ratio;
        assert_ne!(widened, bound, "premise broken: the two strength stops share a bound");
        let full = fit_recipe_from_with(
            &coast(false),
            &coast(true),
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
        );
        assert_eq!(
            projection_arg(&full, keys::FIT_NOTE_CAST_PROJECTED, "bound"),
            Some(widened),
            "the projection at full strength must quote the widened bound: {}",
            full.recipe.rationale
        );
    }

    /// v1.2.3 fix-up — the PRECEDENCE, as the design states it: the fan gate
    /// must be the ONLY gate that convicted for a cast to be rescued.
    ///
    /// The arm used to fire on `!rehue_blocked && hue_fanned.is_some()`, which
    /// also let a pair the RATIO gate had convicted be rescued — and
    /// `FIT_NOTE_CAST_PROJECTED` names the fan and only the fan, so that pair
    /// would have shipped a sentence omitting one of the two verdicts its
    /// curves had to survive. Driven through synthetic outcomes so each
    /// combination is exercised exactly once, without needing a fixture that
    /// happens to trip two gates at the same time.
    #[test]
    fn a_cast_the_ratio_gate_convicts_is_not_rescued_by_the_projection() {
        use crate::rationale::keys;
        let readings = CastReadings {
            ratio: 3.4,
            bound: CAST_ACCEPT_RATIO,
            foreign: Some(0.0),
            rehued: 0.0,
            fan: Some(38.0),
        };
        let fan_only = CastOutcome {
            hue_fanned: Some((0.917, 38.0)),
            readings: Some(readings),
            ..CastOutcome::default()
        };
        assert_eq!(
            fan_only.earns_projection(),
            Some((0.917, 38.0)),
            "a fan-ONLY conviction is the case the projection exists for"
        );

        // Ratio AND fan: no projection, and the refusal keeps the note it
        // already had — the fan note, which wins a double rejection because
        // it is the more specific statement.
        let both = CastOutcome { ratio_rejected: true, ..fan_only };
        assert_eq!(
            both.earns_projection(),
            None,
            "a cast the ratio gate convicted must not be rescued"
        );
        assert!(both.refused(), "…it stays refused");
        assert_eq!(
            both.note().map(|n| n.key),
            Some(keys::FIT_NOTE_CAST_HUE_FANNED),
            "…with the note it already had"
        );

        // A pixel-aligned veto: no projection either, and that is the whole
        // of the viaduct pair's byte-identity.
        let vetoed = CastOutcome { rehue_blocked: true, ..fan_only };
        assert_eq!(vetoed.earns_projection(), None);
        assert_eq!(vetoed.note().map(|n| n.key), Some(keys::FIT_NOTE_REHUE_BLOCKED));
        let all_three = CastOutcome { rehue_blocked: true, ratio_rejected: true, ..fan_only };
        assert_eq!(all_three.earns_projection(), None);

        // …and a RATIO-only conviction was never rescuable: there is no fan
        // reading to hand the projection.
        let ratio_only = CastOutcome {
            ratio_rejected: true,
            readings: Some(readings),
            ..CastOutcome::default()
        };
        assert_eq!(ratio_only.earns_projection(), None);
        assert_eq!(ratio_only.note().map(|n| n.key), Some(keys::FIT_NOTE_CAST_REJECTED));
    }

    /// v1.2.3 fix-up — the search must not bisect PAST the band its own gain
    /// bar opens.
    ///
    /// A bisection finds the largest member of a DOWNWARD-CLOSED set. The
    /// admissibility half is one (the fan grows with `t`, every gate clears
    /// toward the identity); the gain half is the opposite — it falls to zero
    /// as `t → 0`, because at `t = 0` there are no curves. Testing both inside
    /// the loop makes the clearing set an interval `[a, b]` with `a > 0`, and
    /// a probe that fails on the GAIN moves the search DOWN, away from the
    /// band, to `None`. The refusal is conservative, but the sentence it then
    /// wrote — "no milder version both cleared the limit and left the frame
    /// closer to the target" — was a claim about the whole path made by a
    /// search that never looked at it; the sentence now says what the search
    /// did (nothing cleared, or the best-paying clearing point did not pay).
    ///
    /// The property survives the v1.2.4 sweep unchanged, and it is the reason
    /// the sweep could be added at all: the bisection still sees only
    /// admissibility, so the gain bar is still applied after the search
    /// rather than inside it — now to the maximum the sweep found rather than
    /// to the frontier.
    ///
    /// The synthetic path here is exactly that shape: admissible up to
    /// `t = 0.4`, worth more than [`FIT_QUANT`] only above `t = 0.36`. The two
    /// probes the ruling names are asserted as premises — `t = 0.5` fails on
    /// the fan, `t = 0.25` fails on the gain — so the fixture cannot drift
    /// into testing something else.
    #[test]
    fn the_search_does_not_bisect_past_a_band_the_gain_bar_opens() {
        let err_without = 0.05f32;
        // ratio = 1 − 0.1·t, so gain = err_without·0.1·t = 0.005·t, crossing
        // FIT_QUANT (0.0018) at t = 0.36; the fan is over the target above
        // t = 0.4. Clearing band: (0.36, 0.4].
        let judge = |t: f32| CastOutcome {
            readings: Some(CastReadings {
                ratio: 1.0 - 0.1 * t,
                bound: CAST_ACCEPT_RATIO,
                foreign: Some(0.0),
                rehued: 0.0,
                fan: Some(if t <= 0.4 { 0.0 } else { 4.0 * FAN_PROJECT_DEG }),
            }),
            ..CastOutcome::default()
        };
        let gain = |t: f32| err_without * (1.0 - judge(t).readings.unwrap().ratio);
        assert!(
            judge(0.5).readings.unwrap().fan.unwrap() > FAN_PROJECT_DEG,
            "premise: t = 0.5 must fail on the FAN"
        );
        assert!(gain(0.5) > FIT_QUANT, "premise: …and not on the gain");
        assert!(
            judge(0.25).readings.unwrap().fan.unwrap() <= FAN_PROJECT_DEG,
            "premise: t = 0.25 must be admissible"
        );
        assert!(gain(0.25) <= FIT_QUANT, "premise: …and fail on the GAIN");

        let (t, out) = search_cast_projection_t(err_without, judge)
            .expect("the band inside (0.25, 0.5) must be found");
        assert!(
            (0.36..=0.4).contains(&t),
            "the search must land in the clearing band (0.36, 0.4], not at {t}"
        );
        assert!(gain(t) > FIT_QUANT, "…and the winner must actually pay ({})", gain(t));
        assert_eq!(
            out.readings.unwrap().fan,
            Some(0.0),
            "…and be the admissible outcome, not the last probe"
        );
    }

    /// v1.2.4 — the search takes the BEST-PAYING admissible shrink, not the
    /// strongest one.
    ///
    /// v1.2.3 judged the gain at exactly one point, the admissible frontier,
    /// and said so: a shrink that pays only at a milder `t` was not found, and
    /// the two-family HSL pair was refused although every `t ≤ 0.25` on its
    /// path was admissible and paid 0.0019–0.0033. The path here has that
    /// shape written down — admissible up to `t = 0.4`, gain peaking at
    /// `t = 0.2` and falling to a tenth of [`FIT_QUANT`] at the frontier — so
    /// a frontier-only search must refuse it and a sweep must find the peak.
    /// Both premises are asserted before the search runs, so the fixture
    /// cannot drift into testing something else.
    #[test]
    fn the_search_takes_the_best_paying_admissible_shrink() {
        let err_without = 0.05f32;
        // gain(t) = 0.0033 − 0.06·(t − 0.2)², so the peak is 0.0033 at
        // t = 0.2 and the frontier at t = 0.4 pays 0.0009 — half the bar.
        let gain = |t: f32| 0.0033 - 0.06 * (t - 0.2) * (t - 0.2);
        let judge = |t: f32| CastOutcome {
            readings: Some(CastReadings {
                ratio: 1.0 - gain(t) / err_without,
                bound: CAST_ACCEPT_RATIO,
                foreign: Some(0.0),
                rehued: 0.0,
                fan: Some(if t <= 0.4 { 0.0 } else { 4.0 * FAN_PROJECT_DEG }),
            }),
            ..CastOutcome::default()
        };
        assert!(
            judge(0.4).readings.unwrap().fan.unwrap() <= FAN_PROJECT_DEG
                && judge(0.45).readings.unwrap().fan.unwrap() > FAN_PROJECT_DEG,
            "premise: the admissible frontier is at t = 0.4"
        );
        assert!(gain(0.4) < FIT_QUANT, "premise: the FRONTIER does not pay ({})", gain(0.4));
        assert!(gain(0.2) > FIT_QUANT, "premise: the interior peak does ({})", gain(0.2));

        let (t, out) = search_cast_projection_t(err_without, judge)
            .expect("an admissible paying shrink exists at t = 0.2 and must be found");
        assert!(
            (t - 0.2).abs() <= 0.01,
            "the search must land on the gain PEAK, not the frontier: {t}"
        );
        assert!(
            gain(t) > gain(0.4),
            "…so it must pay more than the frontier does ({} vs {})",
            gain(t),
            gain(0.4)
        );
        assert_eq!(
            out.readings.unwrap().fan,
            Some(0.0),
            "…and carry the winning probe's own readings"
        );
    }

    /// v1.2.3 fix-up — the haze regression is NEVER projected, asserted in the
    /// tree rather than by a probe that does not ship.
    ///
    /// The claim the release notes make about this pair is byte-identity: its
    /// recipe and rationale with the rescue live are what they were without
    /// it. What makes that true is not a comparison, it is the rescue arm's
    /// GUARD — `CastOutcome::earns_projection` returns `None` here because the
    /// fan gate never convicts this cast (7.8° against a 15° line), so every
    /// `fit_cast_stage` call on this pair runs the same code with
    /// `rescue = true` as with `rescue = false`. That guard is what this test
    /// measures, because it is the thing that can break; a literal
    /// rescue-on/rescue-off comparison is not expressible from here without a
    /// test seam in the solve, and a seam would be the more fragile pin.
    #[test]
    fn the_haze_correction_is_never_projected() {
        use crate::rationale::keys;
        let (base, clean) = haze_pair();
        let c = cast_stage_candidate(&base, &clean);
        let evidence = evidence_model(&c.cur, &c.tp);
        let out =
            cast_gate_outcome_with_ratio(&c.cur, &c.with_px, &c.tp, &evidence, None, CAST_ACCEPT_RATIO);
        let fan = out
            .readings
            .expect("a judged cast carries readings")
            .fan
            .expect("this pair's census has a fan to report");
        assert!(
            fan < FAN_DEG,
            "premise broken: the haze correction now trips the fan gate ({fan:.1}°)"
        );
        assert_eq!(out.hue_fanned, None, "…so the gate does not convict it");
        assert_eq!(
            out.earns_projection(),
            None,
            "…and the rescue arm's guard is false, whatever the other gates say"
        );

        // End to end: the admission, and nothing about a projection.
        let rep = fit_recipe(&base, &clean);
        assert!(
            !rep.recipe.blue_curve.is_empty(),
            "premise broken: the haze un-cast is no longer shipped: {}",
            rep.recipe.rationale
        );
        assert!(
            rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
            "the haze cast is ADMITTED, on its own merits: {}",
            rep.recipe.rationale
        );
        assert!(
            !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_PROJECTED),
            "…and never projected: {}",
            rep.recipe.rationale
        );
        assert!(
            !rep.recipe.rationale.contains("shrunk toward the shape"),
            "…not even in words: {}",
            rep.recipe.rationale
        );
    }

    /// The fan gate's OTHER half: it must not touch a cast that is doing its
    /// job. The haze regression's un-cast is the pair whose curves the fit
    /// has always shipped, and it opens 7.8° — 1.9× under the threshold.
    /// Measured beside it, the pairs the other gates already refuse:
    /// canyon-gold 5.2°, hazy→vivid 2.7°.
    ///
    /// CANYON-WARM MOVED (2026-09-02, CAST-2), and the move is asserted below
    /// rather than quietly dropped. On the fan-gate-only build its whole
    /// recipe was RESET to the calibration base by the terminal do-no-harm
    /// check — err 0.0387 → 0.0387, confidence on the 0.25 floor — so the
    /// candidate this test rebuilds was the cast on a BARE base and read
    /// 7.5°. With the projection the fit lands instead (0.0387 → 0.0339,
    /// confidence 0.406), the candidate is rebuilt on a real recipe, and it
    /// reads 17.2°: convicted by the gate, then projected to +7°.
    #[test]
    fn the_fan_gate_leaves_a_legitimate_cast_alone() {
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let hazed = render::develop_preview(&clean, &haze);
        let report = fit_recipe(&hazed, &clean);
        assert!(
            !report.recipe.blue_curve.is_empty(),
            "premise broken: the haze un-cast is no longer admitted: {}",
            report.recipe.rationale
        );
        for (name, src, tgt, ceiling) in [
            ("haze", hazed.clone(), clean.clone(), 0.6),
            ("canyon_gold", canyon(false), canyon_gold_target(), 0.5),
            ("hazy_vivid", hazy_canyon_source(), vivid_warm_target(), 0.3),
        ] {
            let c = cast_stage_candidate(&src, &tgt);
            let fan = hue_fan_weighted(&c.cur, &c.with_px, &evidence_model(&c.cur, &c.tp))
                .map(|(_, added, _)| added)
                .unwrap_or(0.0);
            assert!(
                fan < ceiling * FAN_DEG,
                "margin eroded: {name} fans {fan:.1}° against the {FAN_DEG}° threshold"
            );
        }
        // Canyon-warm, pinned where it now sits — and pinned as a PROJECTED
        // pair, so a future change that sends it back to a bare-base reset
        // (or lets the fitted fan through) fails here with its number.
        let warm = fit_recipe(&canyon(false), &canyon(true));
        assert!(
            warm.err_after < warm.err_before,
            "canyon-warm must land rather than reset: {} -> {}",
            warm.err_before,
            warm.err_after
        );
        // …in NUMBERS. The ruling that accepted this fixture's move named the
        // landing it accepted, so the landing is what is asserted, with the
        // tolerance stated: measured 2026-09-02 on this tree, err_after
        // 0.0339 and reported confidence 0.4061.
        assert!(
            (warm.err_after - 0.0339).abs() < 0.001,
            "canyon-warm's landing moved off the measured 0.0339: {} -> {}",
            warm.err_before,
            warm.err_after
        );
        assert!(
            (warm.recipe.confidence - 0.406).abs() < 0.01,
            "…and its reported confidence off the measured 0.406: {}",
            warm.recipe.confidence
        );
        let c = cast_stage_candidate(&canyon(false), &canyon(true));
        let fan = hue_fan_weighted(&c.cur, &c.with_px, &evidence_model(&c.cur, &c.tp))
            .map(|(_, added, _)| added)
            .unwrap_or(0.0);
        assert!(fan > FAN_DEG, "premise: canyon-warm's candidate is fan-convicted ({fan:.1}°)");
        assert!(
            (fan - 17.2).abs() < 0.2,
            "canyon-warm's candidate reads {fan:.1}°, not the 17.2° measured for the projection"
        );
        assert!(
            warm.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_PROJECTED),
            "…and it is the PROJECTION that ships it: {}",
            warm.recipe.rationale
        );
    }

    /// v1.2.3 fix-up — the gate's stated WORST CASE, asserted instead of
    /// promised. The gate judges the spread the curves ADD and subtracts the
    /// spread the class arrived with; because a class is a bin of the BEFORE
    /// hue, that baseline is bounded by one class width — and one class
    /// width IS `FAN_DEG` (360° / FAN_HUE_CLASSES). So an ADMITTED cast can
    /// leave up to 2 × FAN_DEG of ABSOLUTE in-class spread in the delivered
    /// frame, which is the tolerance the rustdoc, ARCHITECTURE.md and the
    /// release notes all now state in words. The haze pair is the one whose
    /// cast the fit admits, so it is where the words get checked.
    #[test]
    fn an_admitted_cast_delivers_at_most_two_class_widths_of_hue_fan() {
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let hazed = render::develop_preview(&clean, &haze);
        assert!(
            !fit_recipe(&hazed, &clean).recipe.blue_curve.is_empty(),
            "premise broken: this pair's cast is no longer admitted"
        );
        let candidate = cast_stage_candidate(&hazed, &clean);
        let (_, added, delivered) = hue_fan_weighted(
            &candidate.cur,
            &candidate.with_px,
            &evidence_model(&candidate.cur, &candidate.tp),
        )
        .expect("premise broken: the admitted haze pair has no region-sized hue class");
        assert!(
            delivered < 2.0 * FAN_DEG,
            "an ADMITTED cast delivered {delivered:.1}° of ABSOLUTE in-class hue spread, \
             past the 2 × {FAN_DEG}° worst case the docs claim (added {added:.1}°)"
        );
        // …and the two halves of that arithmetic, so a failure says which
        // one broke: the admission bound, and the class-width bound on the
        // baseline the gate subtracts.
        assert!(added < FAN_DEG, "premise broken: the haze cast would be refused ({added:.1}°)");
        assert_eq!(360.0 / FAN_HUE_CLASSES as f32, FAN_DEG, "one hue class is one FAN_DEG wide");
        assert!(
            delivered - added < 360.0 / FAN_HUE_CLASSES as f32,
            "the baseline the gate subtracts ({:.1}°) is not bounded by one class width",
            delivered - added
        );
    }

    /// v1.2.3 — the stage's ADMISSION was silent. Every way of producing
    /// nothing disclosed (R23-6 A-2 closed the last one), and the strength
    /// budget disclosed when IT bought a marginal cast, but the commonest
    /// outcome of the whole stage — the curves shipped on their own merits —
    /// reached the user as an unexplained presence. The note carries the
    /// four gates' own readings so the admission can be checked, not just
    /// believed.
    #[test]
    fn an_admitted_cast_discloses_the_readings_that_let_it_through() {
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let report = fit_recipe(&render::develop_preview(&clean, &haze), &clean);
        assert!(
            !report.recipe.blue_curve.is_empty(),
            "premise broken: this pair's cast is no longer admitted: {}",
            report.recipe.rationale
        );
        let note = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED)
            .unwrap_or_else(|| {
                panic!("an admitted cast must say so: {}", report.recipe.rationale)
            });
        let arg = |name: &str| {
            note.args
                .iter()
                .find(|(k, _)| *k == name)
                .and_then(|(_, v)| v.parse::<f32>().ok())
                .unwrap_or_else(|| panic!("the note carries {name}: {:?}", note.args))
        };
        // The two readings whose gates are one-sided thresholds ARE on the
        // passing side of their own gate, and the claim is only made for
        // them: the rehued share and the fan reject above their constant,
        // full stop.
        assert!(arg("rehued") < ROT_SHARE, "the disclosed rehued share passed");
        let fan_arg = |name: &str| {
            let n = report
                .notes
                .iter()
                .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN)
                .unwrap_or_else(|| panic!("an admitted cast reports its fan: {}", report.recipe.rationale));
            n.args
                .iter()
                .find(|(k, _)| *k == name)
                .and_then(|(_, v)| v.parse::<f32>().ok())
                .unwrap_or_else(|| panic!("the fan note carries {name}: {:?}", n.args))
        };
        assert!(fan_arg("fan") < FAN_DEG, "the disclosed fan passed");
        assert_eq!(fan_arg("limit"), FAN_DEG, "the fan note names the limit it measured against");
        let foreign_arg = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FOREIGN)
            .and_then(|n| n.args.iter().find(|(k, _)| *k == "foreign"))
            .and_then(|(_, v)| v.parse::<f32>().ok())
            .expect("this pair's target carries hue evidence, so the share is measurable");
        assert!(foreign_arg < VETO_CREATED_SHARE, "the disclosed foreign share passed");

        // The RATIO is NOT claimed to have passed a threshold, because it
        // has none: the ratio arm rejects only when the evidence is also
        // unidentifiable, so an admitted ratio may exceed its bound. What
        // the note must get right is WHICH bound the path used — and that is
        // `budget.cast_ratio`, which the strength budget moves. Asserting
        // against the CAST_ACCEPT_RATIO constant would have passed for a
        // note quoting a bound the fit never applied.
        assert_eq!(
            arg("bound"),
            FitBudget::for_strength(crate::recipe::GradeStrength::default()).cast_ratio,
            "the default-strength solve discloses the default-strength bound"
        );
        // …and it is whatever bound the gate was HANDED, which at any
        // strength but the default is a different number from the anchor.
        // The end-to-end arm cannot make this point on its own: at the
        // default strength `budget.cast_ratio` IS `CAST_ACCEPT_RATIO`, so a
        // note hard-coding the constant would agree with it. (Re-fitting the
        // same pair at max strength does not help either — the budget also
        // moves the solve, and this pair's max-strength cast is refused by
        // the fan gate at 19°.) So the threading is asserted where it lives.
        let widened = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).cast_ratio;
        assert_ne!(
            widened, CAST_ACCEPT_RATIO,
            "premise broken: max strength no longer widens the cast bound"
        );
        let candidate = cast_stage_candidate(&render::develop_preview(&clean, &haze), &clean);
        let handed = cast_gate_outcome_with_ratio(
            &candidate.cur,
            &candidate.with_px,
            &candidate.tp,
            &evidence_model(&candidate.cur, &candidate.tp),
            None,
            widened,
        )
        .readings
        .expect("the gate always keeps its readings")
        .bound;
        assert_eq!(
            handed, widened,
            "the gate must record the bound it was GIVEN, not the default anchor"
        );

        // And a REFUSED stage says the opposite thing, never both.
        let refused = fit_recipe(&coast(false), &coast(true));
        assert!(
            !refused
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED),
            "a withheld cast must not also claim admission: {}",
            refused.recipe.rationale
        );
    }

    /// v1.2.3 fix-up — an abstaining gate is not a gate that measured zero.
    /// Two of the four readings can decline to answer (`foreign` when the
    /// target carries no hue evidence at all, `fan` when no hue class is
    /// region-sized across two luma slices), and the admission note used to
    /// print `0.000` for both — a measurement never taken, disclosed as if
    /// it had been, in the very note that exists so the admission can be
    /// checked.
    #[test]
    fn an_unmeasured_cast_reading_says_so_instead_of_printing_a_zero() {
        // 1) The PLUMBING: a target with no chromatic mass at all leaves the
        //    foreign-hue census with nothing to be foreign to, and the fan
        //    census with no region-sized class. Both must arrive as None.
        let grey = DynamicImage::ImageRgb8(RgbImage::from_fn(48, 48, |x, y| {
            let v = (24 + ((x * 3 + y * 2) % 200)) as u8;
            image::Rgb([v, v, v])
        }));
        let (s, t) = analysis_pair(&grey, &grey);
        let tp = pixels_of(&t);
        let cur = pixels_of(&render::develop_preview(&s, &EditRecipe::default()));
        let evidence = evidence_model(&cur, &tp);
        let readings = cast_gate_outcome_with_ratio(
            &cur,
            &cur,
            &tp,
            &evidence,
            None,
            CAST_ACCEPT_RATIO,
        )
        .readings
        .expect("the gate always keeps its readings");
        assert_eq!(readings.foreign, None, "a colourless target has no foreign-hue share");
        assert_eq!(readings.fan, None, "a colourless target has no region-sized hue class");

        // 2) The DISCLOSURE: an abstention becomes a sentence, and that
        //    sentence carries no number at all — the failure mode was a
        //    digit, so the assertion is about digits.
        let abstained = cast_admission_notes(CastReadings {
            ratio: 0.5,
            bound: CAST_ACCEPT_RATIO,
            foreign: None,
            rehued: 0.0,
            fan: None,
        });
        let keys_of: Vec<&str> = abstained.iter().map(|n| n.key).collect();
        assert_eq!(
            keys_of,
            vec![
                crate::rationale::keys::FIT_NOTE_CAST_ADMITTED,
                crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA,
                crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN_NA,
            ]
        );
        for note in &abstained[1..] {
            assert!(note.args.is_empty(), "a not-measurable clause carries no reading");
            let text = crate::rationale::render_one(note);
            assert!(
                !text.chars().any(|c| c.is_ascii_digit()),
                "an unmeasured reading must not print a number: {text}"
            );
            assert!(
                text.contains("not measurable"),
                "an unmeasured reading must SAY it was not measured: {text}"
            );
        }

        // 3) …and a MEASURED fan is signed, because the curves can narrow a
        //    class as easily as widen one, and "opened a −3 degree hue fan"
        //    reported a narrowing as an opening.
        let narrowed = cast_admission_notes(CastReadings {
            ratio: 0.5,
            bound: CAST_ACCEPT_RATIO,
            foreign: Some(0.0),
            rehued: 0.0,
            fan: Some(-3.2),
        });
        let fan_note = narrowed
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN)
            .expect("a measured fan writes the measured clause");
        assert_eq!(
            fan_note.args.iter().find(|(k, _)| *k == "fan").map(|(_, v)| v.as_str()),
            Some("-3.2"),
            "a narrowing reads as a signed change, not as an opened fan"
        );
        assert!(crate::rationale::render_one(fan_note).contains("narrowed"));
        let widened = cast_admission_notes(CastReadings { fan: Some(8.4), ..CastReadings::default() });
        assert_eq!(
            widened
                .iter()
                .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN)
                .and_then(|n| n.args.iter().find(|(k, _)| *k == "fan"))
                .map(|(_, v)| v.as_str()),
            // ONE decimal: the clause prints against a limit, and `{:+.0}`
            // rounded the admitted haze pair's 14.6 up to a "+15" that reads
            // as a violation of the 15 beside it.
            Some("+8.4")
        );
    }

    fn free_atmosphere_wb_for_pair(
        src: &DynamicImage,
        target: &DynamicImage,
    ) -> (f32, f32, f32) {
        let (s_img, t_img) = analysis_pair(src, target);
        let base = EditRecipe::default();
        let sp = pixels_of(&render::develop_preview(&s_img, &base));
        let tp = pixels_of(&t_img);
        let structural = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
        let evidence = structural.structure_blind(&tp);
        // Rule 09: the suite holds NO private copy of the solve. Everything
        // above is the shipped preamble (`analysis_pair` -> `develop_preview`
        // -> `evidence_model_for` -> `structure_blind`); the estimator itself
        // is the shipped one, called with `provider: None` — which is what
        // all three callers of this helper pass. The copy that used to live
        // here had drifted twice: it rounded the tint BEFORE the 401-step
        // search instead of after, and it read `evidence.source_weights` /
        // `evidence.target_weights` where production had moved to the
        // shared-content reference, so since R30 R2 it had been testing a
        // solve production no longer performed.
        let anchor = base.as_shot_k.unwrap_or(5500.0);
        let (pair_tp, pair_w) = atmosphere_wb_pairing(&tp, &evidence, None, None);
        let (wb_k, wb_tint, _) =
            atmosphere_wb_from_populations(&sp, pair_tp, &pair_w, anchor);
        (anchor, wb_k, wb_tint)
    }

    fn mean_hue_in_rows(img: &DynamicImage, rows: std::ops::Range<u32>) -> f64 {
        let rgb = img.to_rgb8();
        let (mut sin, mut cos, mut count) = (0.0f64, 0.0f64, 0.0f64);
        for y in rows {
            for x in 0..rgb.width() {
                let p = rgb.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue;
                }
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                sin += hue.sin();
                cos += hue.cos();
                count += 1.0;
            }
        }
        assert!(count > 0.0, "the audited band must contain chromatic pixels");
        sin.atan2(cos).to_degrees().rem_euclid(360.0)
    }

    fn hue_distance(a: f64, b: f64) -> f64 {
        (a - b + 540.0).rem_euclid(360.0) - 180.0
    }

    #[test]
    fn wb_clamp_stays_on_the_manifold_and_never_invents_tint() {
        let src = hazy_canyon_source();
        let target = vivid_warm_target();
        let (anchor, free_k, free_tint) = free_atmosphere_wb_for_pair(&src, &target);
        let strength = crate::recipe::GradeStrength::new(0.85);
        let budget = FitBudget::for_strength(strength);
        let (clamped_k, clamped_tint, clamped, before, after, lambda) =
            budgeted_wb(anchor, free_k, free_tint, budget);
        if clamped {
            assert!(before > budget.wb_ratio && after <= budget.wb_ratio);
        } else {
            assert_eq!(lambda, 1.0);
            assert!(wb_gains_fit_budget(render::wb_gains(anchor, free_k, free_tint), budget));
        }
        assert_eq!(clamped_tint, round1(free_tint * lambda));
        assert!(clamped_tint.abs() <= free_tint.abs());
        let expected_k = ((anchor.ln() + (free_k.ln() - anchor.ln()) * lambda).exp()
            / 50.0)
            .round()
            * 50.0;
        assert_eq!(clamped_k, expected_k);
        assert!(wb_gains_fit_budget(
            render::wb_gains(anchor, clamped_k, clamped_tint),
            budget
        ));

        let report = fit_recipe_from_with(
            &src,
            &target,
            &EditRecipe::default(),
            FitOptions { strength, provider: None },
        );
        assert!(report.recipe.tint.abs() <= free_tint.abs());
        assert!(report.recipe.temperature_k.is_some());
        assert_eq!(report.recipe.temperature_k, Some(clamped_k));
        assert_eq!(report.recipe.tint, clamped_tint);
    }

    #[test]
    fn wb_lambda_shrinks_at_high_strength() {
        let (width, height) = (192u32, 128u32);
        let pair_with_blue_band = |band: [f32; 3]| {
            let source = DynamicImage::ImageRgb8(RgbImage::from_fn(
                width,
                height,
                |x, y| {
                    let level = 0.22 + 0.60 * x as f32 / (width - 1) as f32;
                    let p = if y >= 96 { band } else { [level, level, level] };
                    image::Rgb(p.map(|value| (value * 255.0).round() as u8))
                },
            ));
            let target = DynamicImage::ImageRgb8(RgbImage::from_fn(
                width,
                height,
                |x, y| {
                    let level = 0.22 + 0.60 * x as f32 / (width - 1) as f32;
                    let p = if y >= 96 {
                        band
                    } else {
                        [(1.15 * level + 0.10).min(1.0), 0.60 * level, 0.28 * level]
                    };
                    image::Rgb(p.map(|value| (value * 255.0).round() as u8))
                },
            ));
            (source, target)
        };

        let (probe_source, probe_target) = pair_with_blue_band([0.62, 0.65, 0.65]);
        let (anchor, probe_k, probe_tint) =
            free_atmosphere_wb_for_pair(&probe_source, &probe_target);
        let (probe_chosen_k, probe_chosen_tint, _, _, _, _) = budgeted_wb(
            anchor,
            probe_k,
            probe_tint,
            FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85)),
        );
        let gains = render::wb_gains(anchor, probe_chosen_k, probe_chosen_tint);
        let mut warm_bins = Vec::new();
        for x in 0..width {
            let level = 0.22 + 0.60 * x as f32 / (width - 1) as f32;
            let warm = [(1.15 * level + 0.10).min(1.0), 0.60 * level, 0.28 * level];
            let (hue, _, _) = render::rgb_to_hsl(warm[0], warm[1], warm[2]);
            let bin = ((hue * 24.0) as usize).min(23);
            if !warm_bins.contains(&bin) {
                warm_bins.push(bin);
            }
        }
        let mut blue_band = None;
        'red: for red in 40..=70 {
            for green in 40..=70 {
                for blue in 40..=70 {
                    let pixel = [red as f32 / 100.0, green as f32 / 100.0, blue as f32 / 100.0];
                    let chroma = pixel.iter().copied().fold(0.0f32, f32::max)
                        - pixel.iter().copied().fold(f32::INFINITY, f32::min);
                    if chroma < VETO_SUPPORT_CHROMA {
                        continue;
                    }
                    let (before_hue, _, _) =
                        render::rgb_to_hsl(pixel[0], pixel[1], pixel[2]);
                    let before_degrees = before_hue as f64 * 360.0;
                    if !(170.0..=250.0).contains(&before_degrees) {
                        continue;
                    }
                    let moved = [
                        render::linear_to_srgb(render::srgb_to_linear(pixel[0]) * gains[0]),
                        render::linear_to_srgb(render::srgb_to_linear(pixel[1]) * gains[1]),
                        render::linear_to_srgb(render::srgb_to_linear(pixel[2]) * gains[2]),
                    ];
                    let moved_chroma = moved.iter().copied().fold(0.0f32, f32::max)
                        - moved.iter().copied().fold(f32::INFINITY, f32::min);
                    let (after_hue, _, _) = render::rgb_to_hsl(moved[0], moved[1], moved[2]);
                    let after_degrees = after_hue as f64 * 360.0;
                    let before_bin = ((before_hue * 24.0) as usize).min(23);
                    let after_bin = ((after_hue * 24.0) as usize).min(23);
                    let bin_distance = |a: usize, b: usize| {
                        let forward = (a as isize - b as isize).rem_euclid(24) as usize;
                        forward.min(24 - forward)
                    };
                    let after_is_foreign = bin_distance(after_bin, before_bin) > VETO_FAR_BINS
                        && warm_bins
                            .iter()
                            .all(|&warm_bin| bin_distance(after_bin, warm_bin) > VETO_FAR_BINS);
                    if moved_chroma >= 0.06
                        && hue_distance(before_degrees, after_degrees).abs() >= 50.0
                        && after_is_foreign
                    {
                        blue_band = Some(pixel);
                        break 'red;
                    }
                }
            }
        }
        let blue_band = blue_band.expect("a retained blue band exposes the warm WB damage");
        let (source, target) = pair_with_blue_band(blue_band);
        let (anchor, free_k, free_tint) = free_atmosphere_wb_for_pair(&source, &target);
        let (chosen_k, chosen_tint, _, _, _, _) = budgeted_wb(
            anchor,
            free_k,
            free_tint,
            FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85)),
        );
        let before = render::develop_preview(&source, &EditRecipe::default());
        let demand = EditRecipe {
            temperature_k: Some(chosen_k),
            tint: chosen_tint,
            ..EditRecipe::default()
        };
        let after = render::develop_preview(&source, &demand);
        let before_hue = mean_hue_in_rows(&before, 96..128);
        let after_hue = mean_hue_in_rows(&after, 96..128);
        let foreign = foreign_hue_bins(&pixels_of(&target)).expect("target hue census");
        let foreign_before = foreign_share(&pixels_of(&before), &foreign);
        let foreign_after = foreign_share(&pixels_of(&after), &foreign);
        assert!(
            hue_distance(before_hue, after_hue).abs() >= 45.0,
            "fixture premise: fitted {free_k:.0}/{free_tint:+.1} -> {chosen_k:.0}/{chosen_tint:+.1} rotated the retained blue band only {before_hue:.1} -> {after_hue:.1}"
        );
        assert!(cast_paints_foreign_hues(
            &pixels_of(&before),
            &pixels_of(&after),
            &pixels_of(&target)
        ), "foreign-hue premise failed: fitted {free_k:.0}/{free_tint:+.1} -> {chosen_k:.0}/{chosen_tint:+.1}; band {before_hue:.1} -> {after_hue:.1}, target warm {}, shares {foreign_before:.3} -> {foreign_after:.3}",
            mean_hue_in_rows(&target, 0..96));

        let report = fit_recipe_from_promoted_with_disclosure_opts(
            &source,
            &target,
            &EditRecipe::default(),
            true,
            false,
            FitOptions { strength: crate::recipe::GradeStrength::new(0.85), provider: None },
        );
        assert_eq!(report.mode, FitMode::Atmosphere);
        assert!(report.recipe.temperature_k.is_some());
        assert!(report.notes.iter().any(|note| {
            note.key == crate::rationale::keys::FIT_NOTE_WB_CLAMPED
        }));
        let default_report = fit_recipe_from_promoted(&source, &target, &EditRecipe::default(), true);
        assert_eq!(default_report.recipe.temperature_k, None);
        assert_eq!(default_report.recipe.tint, 0.0);
        assert!(!default_report.notes.iter().any(|note| {
            note.key == crate::rationale::keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE
        }));
    }

    #[test]
    fn wb_is_withheld_when_every_lambda_paints_a_foreign_hue() {
        let before = vec![[0.08f32, 0.16, 0.82]; 1000];
        let after = vec![[0.92f32, 0.18, 0.58]; 1000];
        let target = vec![[0.82f32, 0.48, 0.16]; 1000];
        assert!(wb_moves_pixels_into_foreign_hues(&before, &after, &target));
        let mut rationale = String::new();
        let mut notes = Vec::new();
        crate::rationale::push_note(
            &mut rationale,
            &mut notes,
            crate::rationale::Note::plain(crate::rationale::keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE),
        );
        assert_eq!(notes[0].key, crate::rationale::keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE);
        assert!(rationale.contains("White balance withheld"));
    }

    #[test]
    fn rotation_gate_is_the_unique_rejector_on_the_real_pair_geometry() {
        let src = hazy_canyon_source();
        let tgt = vivid_warm_target();
        // Pin the gate DECISIONS at stage 4 so this test keeps meaning "only
        // the rotation gate stands here" — if a fixture drift makes the ratio
        // gate reject too, the premise asserts below fail with numbers.
        let (s2, t2) = analysis_pair(&src, &tgt);
        let tp2 = pixels_of(&t2);
        let rep = fit_recipe(&src, &tgt);
        let mut pre = rep.recipe.clone();
        pre.red_curve = Vec::new();
        pre.green_curve = Vec::new();
        pre.blue_curve = Vec::new();
        let cur = pixels_of(&render::develop_preview(&s2, &pre));
        let mut with = pre.clone();
        with.red_curve = residual_channel_curve(&cur, &tp2, 0);
        with.green_curve = residual_channel_curve(&cur, &tp2, 1);
        with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
        assert!(!with.blue_curve.is_empty(), "premise broken: no blue crush demanded");
        let with_px = pixels_of(&render::develop_preview(&s2, &with));
        let ratio = look_err(&with_px, &tp2) / look_err(&cur, &tp2);
        assert!(
            ratio < CAST_ACCEPT_RATIO,
            "premise broken: the aggregate gate rejects too (ratio {ratio:.3}) — the \
             rotation gate is no longer uniquely load-bearing on this fixture"
        );
        assert!(
            !cast_paints_foreign_hues(&cur, &with_px, &tp2),
            "premise broken: the foreign-hue veto fires — destination should be target-native"
        );
        assert!(
            cast_rotates_a_region(&cur, &with_px),
            "the rotation gate must fire on the real-pair geometry"
        );
        // End-to-end: the fit must have withheld the curves (rotation gate is
        // the only rejector, per the premises above) and kept the sky blue.
        assert!(
            rep.recipe.red_curve.is_empty()
                && rep.recipe.green_curve.is_empty()
                && rep.recipe.blue_curve.is_empty(),
            "cast curves must be withheld on the real-pair geometry"
        );
        let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
        let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
            for x in 0..192 {
                let p = out.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue;
                }
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                sin += hue.sin();
                cos += hue.cos();
                n += 1.0;
            }
        }
        assert!(n > 0.0, "no chromatic sky pixels to audit");
        let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
        let d = (mean - 214.0 + 540.0).rem_euclid(360.0) - 180.0;
        assert!(
            d.abs() < 30.0,
            "sky hue rotated to {mean:.0}° (Δ{d:.0}°) — the rotation gate is unwired"
        );
    }

    /// The rotation budget's discriminator, pinned on the three live pairs
    /// (calibration-probe numbers, 2026-07-09): the golden-sky regrade
    /// rotates 12.5% of the frame ~170° (must fire — both earlier gates are
    /// blind to a target-native destination), the violet cast 12.5% at 112°
    /// (must fire), the haze correction ≈0.01% past 60° and ~0 past 75°
    /// (must NOT fire). End-to-end verdicts live in
    /// `cast_must_not_rotate_the_sky_into_a_target_native_hue` /
    /// `warm_rock_cast_must_not_violet_the_pale_sky`; this pins the primitive
    /// so a threshold tweak that flips one side fails HERE with numbers.
    ///
    /// THE THIRD FIXTURE CAST-2 MOVED (2026-09-02), recorded here rather than
    /// inherited from that batch's other two. The stage-4 reconstruction below
    /// now neutralises the MIXER before re-deriving the curves
    /// (`pre.hsl = Hsl::default()`); delete that one line and the violet leg
    /// goes red with "share 0.0000".
    ///
    /// What moved is not the gate — it is where canyon-warm's protection comes
    /// from. On the shipped pipeline that pair's mixer now attaches FIRST
    /// ([Orange sat +18 lum +18, Blue sat -18 lum -2.6]), and against THAT state
    /// the re-derived cast curves rotate nothing: measured 0.0000 of the frame
    /// with the mixer in place, 0.1250 with it neutralised. So the fixture's
    /// cast is no longer stopped by the rotation veto; it is stopped by the
    /// composition of the mixer and the hue-fan projection, which convicts the
    /// curves at 16.9° on the pipeline's weighted census (17.2° on this test's
    /// unweighted stage-4 reconstruction — two populations, both stated) and
    /// ships a shrunk cast at t = 0.653 reading +7.2°.
    ///
    /// The protection itself is intact and is measured where it belongs — on
    /// the DELIVERED frame, by `warm_rock_cast_must_not_violet_the_pale_sky`,
    /// whose sky now reads 216.9° against that test's ±30° guard around 213°
    /// (213.9° before CAST-2, a 3.0° move inside a 30° band).
    ///
    /// Neutralising the mixer is also the right reading of what ROT_DEG and
    /// ROT_SHARE are calibrated ON: the PAIR, not the point the do-no-harm
    /// loops happened to land on. And it is a no-op on the pre-CAST-2 tree,
    /// where both canyon recipes were already the bare calibration base.
    #[test]
    fn rotation_gate_separates_regrade_from_haze() {
        // Reconstruct stage-4's exact inputs for each pair, like the veto pin
        // test. Also reports whether the re-derived curves are non-empty, so
        // each leg can assert its premise (an empty-curve pair would make the
        // share trivially 0 and the leg vacuous).
        let stage4 = |src: &DynamicImage, tgt: &DynamicImage| {
            let (s2, t2) = analysis_pair(src, tgt);
            let tp2 = pixels_of(&t2);
            let mut pre = fit_recipe(src, tgt).recipe;
            pre.red_curve = Vec::new();
            pre.green_curve = Vec::new();
            pre.blue_curve = Vec::new();
            // …and the MIXER, so what this calibration reads is a function of
            // the PAIR and not of where the do-no-harm loops happened to
            // land. It is the state the 4a' loop's own neutral probe judges.
            // Before v1.2.3's projection both canyon pairs' whole recipes
            // were reset to the base, so this was already the state; with the
            // violet pair now landing, leaving its mixer in place hides the
            // rotation the fitted mixer has already made (measured: 0.1250 of
            // the frame re-hued without it, 0.0000 with).
            pre.hsl = crate::recipe::Hsl::default();
            let cur = pixels_of(&render::develop_preview(&s2, &pre));
            let mut with = pre.clone();
            with.red_curve = residual_channel_curve(&cur, &tp2, 0);
            with.green_curve = residual_channel_curve(&cur, &tp2, 1);
            with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
            let nonempty = !(with.red_curve.is_empty()
                && with.green_curve.is_empty()
                && with.blue_curve.is_empty());
            let with_px = pixels_of(&render::develop_preview(&s2, &with));
            (cur, with_px, nonempty)
        };
        // Golden-sky regrade: destination hue is target-native, so neither
        // earlier hue veto sees it.
        let (c1, w1, ne1) = stage4(&canyon(false), &canyon_gold_target());
        assert!(ne1, "premise broken: the golden pair no longer provokes cast curves");
        let s1 = rehued_share(&c1, &w1);
        assert!(
            cast_rotates_a_region(&c1, &w1),
            "rotation gate must fire on the golden-sky regrade (share {s1:.4})"
        );
        assert!(s1 > 2.0 * ROT_SHARE, "margin eroded: golden share {s1:.4}");
        // Violet canyon: also caught here (112° ≥ 75°) — an independent net
        // under the foreign-hue veto.
        let (c2, w2, ne2) = stage4(&canyon(false), &canyon(true));
        assert!(ne2, "premise broken: the violet pair no longer provokes cast curves");
        let s2 = rehued_share(&c2, &w2);
        assert!(
            cast_rotates_a_region(&c2, &w2),
            "rotation gate must fire on the violet cast (share {s2:.4})"
        );
        assert!(s2 > 2.0 * ROT_SHARE, "margin eroded: violet share {s2:.4}");
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let (c3, w3, ne3) = stage4(&base, &clean);
        assert!(ne3, "premise broken: the haze pair no longer provokes cast curves");
        let s3 = rehued_share(&c3, &w3);
        assert!(
            !cast_rotates_a_region(&c3, &w3),
            "rotation gate must NOT fire on the haze correction (share {s3:.4})"
        );
        // 0.1× also pins ROT_DEG from BELOW: at 45° the haze pair's share is
        // 0.0134 (0.27× ROT_SHARE) and would fail here; at 75° it measures
        // ≈ 0.0001.
        assert!(s3 < 0.1 * ROT_SHARE, "margin eroded: haze share {s3:.4} (measured ≈ 0)");
    }

    /// R18: the pass-through exemption's exact borders, patrolled (the R17
    /// disclosure left the band unmonitored). A rotation invisible on both
    /// ends (cc < 0.05 ∧ wc < 0.09) is exempt; crossing EITHER visibility
    /// floor puts it straight back in the census. If a real pair ever
    /// wrecks a region inside the exempt band, ROT_VISIBLE_* is the
    /// suspect (see the const doc).
    #[test]
    fn the_pass_through_exemption_borders_are_patrolled() {
        let share = |c: [f32; 3], w: [f32; 3]| rehued_share(&vec![c; 100], &vec![w; 100]);
        // Inside the blind band: a faint blue (cc 0.045) flipped ~174° to a
        // faint warm (wc 0.085) — pass-through, exempt.
        assert_eq!(share([0.655, 0.68, 0.70], [0.735, 0.68, 0.65]), 0.0);
        // After side crosses 0.09: the same faint blue painted a VISIBLE
        // warm — back in the census.
        assert!(share([0.655, 0.68, 0.70], [0.745, 0.68, 0.65]) > 0.99);
        // Before side crosses 0.05: a visible tint re-hued — counted even
        // though the destination stays faint.
        assert!(share([0.645, 0.68, 0.70], [0.70, 0.68, 0.65]) > 0.99);
    }

    #[test]
    fn rotation_census_sees_a_barely_tinted_before_side() {
        // H17: a faint blue (chroma 0.035 — UNDER the visible-tint gate,
        // above the measurable-hue floor) painted strong gold is a ~180°
        // re-hue; the old both-sides-visible census skipped it entirely,
        // reopening the golden-sky class one threshold to the left.
        let cur = vec![[0.665f32, 0.68, 0.70]; 1000];
        let with = vec![[0.92f32, 0.78, 0.58]; 1000];
        assert!(rehued_share(&cur, &with) > 0.99, "the whole faint-blue field re-hued");
        assert!(cast_rotates_a_region(&cur, &with));
        // A truly NEUTRAL before side has no hue to rotate — colourising
        // neutrals is what a corrective cast legitimately does.
        let neutral = vec![[0.68f32, 0.68, 0.68]; 1000];
        assert_eq!(rehued_share(&neutral, &with), 0.0);
    }

    /// The haze pair reused by several calibration tests below.
    fn haze_pair() -> (DynamicImage, DynamicImage) {
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        (base, clean)
    }

    /// THE CALIBRATION RECORD for the joint value-range family (R23-6, retuned
    /// on real pairs in R24 batch 2).
    ///
    /// This test records the fixture measurements and asserts the policy
    /// boundary: a refusal emits the refusal note, and a genuine miss emits
    /// the miss note. The measurements are diagnostics, not a widened bound.
    /// The table below remains executable evidence for fixture drift,
    /// assertions — a fixture drift that moves the numbers must fail loudly
    /// rather than quietly invalidate the constants.
    ///
    /// Measured on the FIXTURES (weighted reading, base → finished fit). ALL
    /// SIX ROWS RE-MEASURED 2026-09-02 on this tree: the previous table
    /// predated the hue-fan gate and its projection and every row of it had
    /// drifted, some far enough to contradict the assertions below.
    ///   identity                        0.0000 → 0.0000   (pure quantisation)
    ///   roundtrip (known recipe)        0.0590 → 0.0022
    ///   haze → clean (evidence-limited) 0.1937 → 0.0426
    ///   canyon warm (violet class)      0.1796 → 0.0437   LANDS since v1.2.3
    ///   canyon gold (rotation class)    0.2768 → 0.2768   refused, unchanged
    ///   hazy canyon → vivid warm        0.4913 → 0.4554
    ///
    /// Look error and reported confidence beside them, same run (the two
    /// synthetic-recipe rows are not re-measured here — they carry no FAR
    /// classification and nothing below reads them):
    ///   haze → clean       0.0547 → 0.0133   confidence 0.680
    ///   canyon warm        0.0387 → 0.0339   confidence 0.406
    ///   canyon gold        0.0547 → 0.0547   confidence 0.250 (whole recipe
    ///                                        reset by the terminal check)
    ///   hazy → vivid warm  0.1286 → 0.0964   confidence 0.250
    ///
    /// Measured on SIX REAL (RAW, finished JPEG) pairs off the user's library,
    /// 2026-08-17, EXIF-timestamp-confirmed same frame, through `autoshade
    /// match` (finished reading, `look_err` before → after, reported
    /// confidence). These are what retired the "provisional" label:
    ///   A1 astro composite     0.141   0.156 → 0.061   the nonsense pair
    ///   A4 low-sat neutral     0.054   0.091 → 0.063
    ///   A5 portrait, cropped   0.035   0.070 → 0.029   not-same-frame warned
    ///   A2 vivid warm          0.030   0.028 → 0.010
    ///   A6 portrait            0.024   0.124 → 0.028
    ///   A3 monochrome          0.019   0.024 → 0.012
    ///
    /// Three facts the policy and its diagnostics record:
    ///   * SEPARATION — the fits that REACH their target land at 0.0000-0.0437
    ///     (four fixtures) and 0.054 at worst across the five honest real
    ///     pairs, while a target no global model can reach stays high:
    ///     0.4554 for the synthetic repaint (the real-pair geometry of
    ///     2026-07-09 #2), 0.2768 for canyon gold and 0.141 for the real
    ///     astro composite. That is the gap [`fit_zoned::JOINT_FAR_ERR`] =
    ///     0.10 sits in — a factor of ten now, not the factor of two the old
    ///     table showed. The second reading still earns its place, but on the
    ///     REAL pair rather than on this fixture: the astro composite's
    ///     `look_err` reads 0.061, i.e. 0.63 confidence, over a render whose
    ///     Milky Way is gone, whereas the synthetic repaint's 0.0964 now
    ///     lands on the 0.25 confidence floor, so on that fixture the two
    ///     readings agree.
    ///   * THE REFUSAL CLASS is a separate claim, and since v1.2.3 it has ONE
    ///     member: canyon gold, 0.2768 → 0.2768, i.e. 2.8× JOINT_FAR_ERR —
    ///     the solver withheld one-sided movement and the terminal do-no-harm
    ///     check reset the whole recipe (look error 0.0547 → 0.0547). Its
    ///     refusal note must be emitted and the miss note must not be. Canyon
    ///     warm LEFT this band in v1.2.3 (CAST-2): its fan-convicted cast is
    ///     projected instead of thrown away, the reset no longer fires, and
    ///     it lands at 0.0437 — asserted below where it now is rather than
    ///     dropped from the fixture set. The old record's "canyon warm 0.061
    ///     and canyon gold 0.093 … 0.093 is the tightest constraint, 8% of
    ///     headroom" was stale on both counts: neither number survives
    ///     re-measurement, and 0.093 sits BELOW JOINT_FAR_ERR, so a refusal
    ///     reading it would have emitted no refusal note at all.
    ///   * MONOTONICITY — every pair improves or holds, and on this tree
    ///     without exception: the identity pair's old +0.0009 of rounding now
    ///     reads 0.0000 → 0.0000. That is what
    ///     [`fit_zoned::JOINT_DRIFT_TOL`] = 0.05 has its headroom over, and
    ///     all six real pairs improve by far more than 0.05
    ///     (`pipeline::tests::r16_composed_fit_on_a_real_pair`).
    #[test]
    fn joint_family_is_calibrated_on_the_fixture_set() {
        let edge = ANALYZE_EDGE;
        let read = |src: &DynamicImage, tgt: &DynamicImage| -> (f32, f32, f32) {
            let s2 = src.thumbnail(edge, edge);
            let tp2 = pixels_of(&tgt.thumbnail(edge, edge));
            let rep = fit_recipe(src, tgt);
            let base_px = pixels_of(&render::develop_preview(&s2, &EditRecipe::default()));
            let fit_px = pixels_of(&render::develop_preview(&s2, &rep.recipe));
            let evidence = evidence_model(&base_px, &tp2);
            let b = crate::fit_zoned::joint_reading_with_evidence(
                &base_px,
                &tp2,
                &evidence.source_weights,
                &evidence.target_weights,
            )
            .expect("base reading");
            let a = crate::fit_zoned::joint_reading_with_evidence(
                &fit_px,
                &tp2,
                &evidence.source_weights,
                &evidence.target_weights,
            )
            .expect("fit reading");
            (b.weighted, a.weighted, rep.err_after)
        };
        // The two bands are asserted SEPARATELY (R24 batch 2). Haze sits in
        // the REACHED band: the paired robust estimator un-casts it and its
        // joint reading lands at 0.0426 (2026-09-02), well under
        // JOINT_FAR_ERR, with the recipe fitting 0.0547 -> 0.0133 and never
        // reset to neutral — a disclosed partial fit rather than a no-op.
        let mut reached_max = 0.0f32;
        let mut refusal_max = 0.0f32;
        // Canyon GOLD is the refusal band's remaining member; canyon warm
        // left it in v1.2.3 and is asserted where it went, below.
        {
            let (name, src, tgt) = ("canyon gold", canyon(false), canyon_gold_target());
            let (before, after, _) = read(&src, &tgt);
            let report = fit_recipe(&src, &tgt);
            assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_REFUSED), "{name}: refusal FAR note missing: {}", report.recipe.rationale);
            assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "{name}: refusal emitted miss FAR note: {}", report.recipe.rationale);
            assert!(
                after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
                "{name}: the fit must not push the joint reading past the drift \
                 tolerance ({before:.4} -> {after:.4})"
            );
            refusal_max = refusal_max.max(after);
        }
        {
            // Canyon warm LEFT the refusal band in v1.2.3 (CAST-2). Its cast
            // is convicted by the hue-fan gate and then projected, and the
            // projected cast is enough for the terminal do-no-harm check to
            // stop resetting the whole recipe: measured 0.0387 -> 0.0339 at
            // confidence 0.406, where the fan-gate-only build reset to the
            // base and reported 0.0387 -> 0.0387 at the 0.25 floor. So it is
            // asserted where it now is — no FAR classification of either
            // kind — rather than dropped from the fixture set.
            let (before, after, _) = read(&canyon(false), &canyon(true));
            let report = fit_recipe(&canyon(false), &canyon(true));
            assert!(report.err_after < report.err_before, "canyon warm no longer resets: {} -> {}", report.err_before, report.err_after);
            // …and it lands WHERE it was measured to land, not merely
            // somewhere better (2026-09-02, this tree: 0.0387 -> 0.0339 at
            // confidence 0.4061, joint reading 0.1796 -> 0.0437).
            assert!(
                (report.err_after - 0.0339).abs() < 0.001
                    && (report.recipe.confidence - 0.406).abs() < 0.01,
                "canyon warm's landing moved off the measured 0.0339 / 0.406: {} -> {} at {}",
                report.err_before, report.err_after, report.recipe.confidence
            );
            assert!(
                (after - 0.0437).abs() < 0.005,
                "…and its joint reading off the measured 0.0437: {before:.4} -> {after:.4}"
            );
            assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_REFUSED), "canyon warm is no longer a refusal: {}", report.recipe.rationale);
            assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "nor a miss: {}", report.recipe.rationale);
            assert!(
                after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
                "canyon warm: {before:.4} -> {after:.4}"
            );
            reached_max = reached_max.max(after);
        }
        {
            let (before, after, _) = read(&synth(), &synth());
            assert!(
                after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
                "identity: {before:.4} -> {after:.4}"
            );
            reached_max = reached_max.max(after);
        }
        {
            // The paired robust estimator moved this pair OUT of the refusal
            // bucket (vouched convergence un-casts the haze; look error
            // 0.0547 -> 0.0133, joint reading 0.1937 -> 0.0426, re-measured
            // 2026-09-02): no FAR classification of either kind rides, and
            // the joint reading lands with the reached fixtures.
            let (base, clean) = haze_pair();
            let (before, after, _) = read(&base, &clean);
            let report = fit_recipe(&base, &clean);
            assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_REFUSED), "the vouched haze fit is no longer a refusal: {}", report.recipe.rationale);
            assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "nor a miss: {}", report.recipe.rationale);
            assert!(
                after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
                "evidence-limited haze: {before:.4} -> {after:.4}"
            );
            reached_max = reached_max.max(after);
        }
        {
            let src = synth();
            let mut truth = EditRecipe {
                exposure_ev: 0.35,
                contrast: 18.0,
                highlights: -25.0,
                whites: 12.0,
                saturation: 15.0,
                ..Default::default()
            };
            truth.clamp();
            let tgt = render::develop_preview(&src, &truth);
            let (before, after, _) = read(&src, &tgt);
            assert!(after < before * 0.5, "roundtrip: {before:.4} -> {after:.4}");
            reached_max = reached_max.max(after);
        }
        // Keep the observed values in the transcript for calibration review;
        // the policy assertion is the typed-note split above, not a ceiling
        // derived from this observed maximum.
        let (_, _, look) = read(&hazy_canyon_source(), &vivid_warm_target());
        eprintln!("JOINT_REFUSAL_MEASURED worst={refusal_max:.4}; reached={reached_max:.4}");
        // THE REAL-PAIR ANCHORS, recorded as literals because the photographs
        // cannot ship (RAW + finished JPEG off the user's library, measured
        // through `autoshade match` on 2026-08-17 — the table in this test's
        // doc). These are retained as real-pair diagnostics, not as a way to
        // widen or backfill the policy line.
        // from above, and without them the constant could drift back to any
        // value the fixtures tolerate. They are what makes this test fail on
        // the pre-R24 ladder.
        // Both sides are constants, so they are checked at COMPILE time: a
        // ladder that stops bracketing the real pairs must not wait for
        // someone to run the suite.
        const REAL_NONSENSE_WEIGHTED: f32 = 0.141; // A1, the astro composite
        const REAL_HONEST_WORST: f32 = 0.054; // A4, worst of the five honest
        const _: () = assert!(
            crate::fit_zoned::JOINT_FAR_ERR <= REAL_NONSENSE_WEIGHTED,
            "the real pair the user called nonsense reads 0.141 and must raise \
             the joint warning — that is why the line moved off 0.25"
        );
        const _: () = assert!(
            REAL_HONEST_WORST < crate::fit_zoned::JOINT_FAR_ERR,
            "…and the worst HONEST real pair reads 0.054 and must not"
        );
        // The other end of the same anchor: on that pair the ladder must
        // reach its floor, not merely warn. It reported 0.578 before this
        // retune — a warning printed beside "we are 58% sure" is the
        // incoherence the tie between the two constants exists to prevent.
        assert_eq!(
            clamp_confidence(
                1.0 - REAL_NONSENSE_WEIGHTED * crate::fit_zoned::JOINT_CONFIDENCE_SLOPE
            ),
            CONFIDENCE_FLOOR,
            "the nonsense pair must bottom the confidence out, not shade it"
        );
        // The evidence-weighted scalar now sees spatial damage too. Keep a
        // broad numerical bound so a unit/normalisation regression is caught;
        // this fixture no longer needs to fool the scalar to calibrate the
        // independent joint family.
        // Root-cause fix keeps same-content ranges available even when one
        // local cell is structurally noisy. The new measured calibration
        // value is 0.1234; retain a narrow regression bound above it.
        assert!(
            look < 0.125,
            "the evidence-weighted scalar changed scale unexpectedly ({look:.4})"
        );
    }

    /// The joint family may REPORT the worst bucket but must never gate on
    /// it — measured here, because the temptation is obvious and the data
    /// says the opposite. Both measured casts improve the worst bucket, and
    /// the cast that should be refused improves it by even more, so a drift
    /// gate cannot use that bucket to distinguish the two decisions.
    #[test]
    fn the_worst_bucket_cannot_gate_a_stage() {
        let edge = ANALYZE_EDGE;
        let cast_pair = |src: &DynamicImage, tgt: &DynamicImage| -> (f32, f32) {
            let s2 = src.thumbnail(edge, edge);
            let tp2 = pixels_of(&tgt.thumbnail(edge, edge));
            let rep = fit_recipe(src, tgt);
            let mut pre = rep.recipe.clone();
            pre.red_curve = Vec::new();
            pre.green_curve = Vec::new();
            pre.blue_curve = Vec::new();
            let cur = pixels_of(&render::develop_preview(&s2, &pre));
            let mut with = pre.clone();
            with.red_curve = residual_channel_curve(&cur, &tp2, 0);
            with.green_curve = residual_channel_curve(&cur, &tp2, 1);
            with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
            let with_px = pixels_of(&render::develop_preview(&s2, &with));
            (
                crate::fit_zoned::joint_reading(&cur, &tp2).expect("without").worst,
                crate::fit_zoned::joint_reading(&with_px, &tp2).expect("with").worst,
            )
        };
        let (base, clean) = haze_pair();
        let (haze_without, haze_with) = cast_pair(&base, &clean);
        assert!(
            haze_with < haze_without,
            "the measured haze cast improves the worst bucket ({haze_without:.4} -> {haze_with:.4})"
        );
        let (gold_without, gold_with) = cast_pair(&canyon(false), &canyon_gold_target());
        assert!(
            gold_with < gold_without,
            "the measured wrecking cast also improves the worst bucket ({gold_without:.4} -> {gold_with:.4})"
        );
        assert!(
            haze_with - haze_without > gold_with - gold_without,
            "both casts improve the bucket, and the wrecking cast improves it more; a drift gate cannot distinguish them: haze {haze_without:.4} -> {haze_with:.4}; wrecking {gold_without:.4} -> {gold_with:.4}"
        );
    }

    /// The confidence family is ONE calibration: the FAR warning fires
    /// exactly where the slope has already bottomed the number out. Two
    /// literals could drift apart silently; this is why they are named.
    #[test]
    fn the_confidence_family_is_one_calibration() {
        let bottom = (1.0 - CONFIDENCE_FLOOR) / CONFIDENCE_SLOPE;
        assert!(
            (bottom - FIT_FAR_ERR).abs() <= 0.01,
            "the FAR line ({FIT_FAR_ERR}) must be the residual at which the \
             slope reaches the floor ({bottom})"
        );
        assert_eq!(confidence_from_look_err(bottom + 0.001), CONFIDENCE_FLOOR);
        assert_eq!(confidence_from_look_err(0.0), CONFIDENCE_CEIL);
        // The joint ladder is its own calibration with the same shape.
        let jb = (1.0 - CONFIDENCE_FLOOR) / crate::fit_zoned::JOINT_CONFIDENCE_SLOPE;
        assert!(
            (jb - crate::fit_zoned::JOINT_FAR_ERR).abs() <= 0.01,
            "the joint FAR line must be the weighted reading at which its own \
             slope reaches the floor ({jb})"
        );
    }

    /// R23-6 A-2: the colour stage's SILENT arm. `ratio_fail` empties all
    /// three channel curves and used to push no note, while the hue gates
    /// beside it did disclose — so the commonest way for the colour stage to
    /// produce nothing was also the only one the user could not read about.
    ///
    /// The decision is pinned as a PURE function because no fixture in this
    /// repo reaches the ratio arm without a hue gate also firing (measured:
    /// of the six fixture pairs, 13 stage runs accept, 8 are hue-only
    /// rejections and 5 are both) — an end-to-end test would therefore pass
    /// on the hue note and prove nothing about the arm it is named for.
    #[test]
    fn a_silently_rejected_colour_stage_now_says_so() {
        use crate::rationale::keys;
        // The arm this test exists for: no hue damage, the aggregate simply
        // did not earn the risk.
        let key = |outcome: CastOutcome| outcome.note().map(|note| note.key);
        assert_eq!(
            key(CastOutcome { ratio_rejected: true, ..CastOutcome::default() }),
            Some(keys::FIT_NOTE_CAST_REJECTED)
        );
        // The hue note wins a double rejection — more specific, and the
        // thing worth saying.
        assert_eq!(
            key(CastOutcome {
                rehue_blocked: true,
                ratio_rejected: true,
                ..CastOutcome::default()
            }),
            Some(keys::FIT_NOTE_REHUE_BLOCKED)
        );
        assert_eq!(
            key(CastOutcome { rehue_blocked: true, ..CastOutcome::default() }),
            Some(keys::FIT_NOTE_REHUE_BLOCKED)
        );
        // v1.2.3, the same precedence question one gate down: the fan
        // refusal is more specific than "did not buy enough" and LESS
        // specific than the pixel-aligned verdict. The second assert is what
        // keeps every recipe the pixel gates already govern byte-identical.
        assert_eq!(
            key(CastOutcome {
                hue_fanned: Some((0.9, 38.0)),
                ratio_rejected: true,
                ..CastOutcome::default()
            }),
            Some(keys::FIT_NOTE_CAST_HUE_FANNED)
        );
        assert_eq!(
            key(CastOutcome {
                hue_fanned: Some((0.9, 38.0)),
                rehue_blocked: true,
                ..CastOutcome::default()
            }),
            Some(keys::FIT_NOTE_REHUE_BLOCKED)
        );
        // …and it carries its readings, because a refusal the user cannot
        // check is not a disclosure.
        let fanned = CastOutcome { hue_fanned: Some((0.917, 37.6)), ..CastOutcome::default() }
            .note()
            .expect("the fan refusal writes a note");
        assert_eq!(
            fanned.args,
            vec![
                ("share", "0.917".to_string()),
                // ONE decimal, for the same reason the admitted clause has
                // one: at `{:.0}` a convicting 15.4 rendered as "15 degrees
                // apart (limit 15)".
                ("fan", "37.6".to_string()),
                ("limit", format!("{FAN_DEG:.0}")),
            ]
        );
        // An ACCEPTED stage says nothing HERE — its own note is pushed from
        // the admission readings, not from this method.
        assert_eq!(key(CastOutcome::default()), None);

        // …and the end-to-end property that follows from it: whenever the
        // colour stage ships nothing, SOMETHING explains it.
        for (name, src, tgt) in [
            ("canyon warm", canyon(false), canyon(true)),
            ("canyon gold", canyon(false), canyon_gold_target()),
            ("hazy canyon", hazy_canyon_source(), vivid_warm_target()),
        ] {
            let rep = fit_recipe(&src, &tgt);
            let empty = rep.recipe.red_curve.is_empty()
                && rep.recipe.green_curve.is_empty()
                && rep.recipe.blue_curve.is_empty();
            if !empty {
                continue;
            }
            assert!(
                rep.notes.iter().any(|n| {
                    n.key == keys::FIT_NOTE_CAST_REJECTED
                        || n.key == keys::FIT_NOTE_REHUE_BLOCKED
                        || n.key == keys::FIT_NOTE_REGRESSED
                        || n.key == keys::FIT_SUMMARY_ATMOSPHERE
                }),
                "{name}: an empty colour stage must disclose WHY: {}",
                rep.recipe.rationale
            );
        }
    }

    /// R23-6 A-5: the disclosure names controls for THIS pair, not the
    /// blanket sentence every fit carries. The canyon-gold pair's residual
    /// is a per-band hue move the solver has no dial for.
    #[test]
    fn the_unsolvable_controls_are_named_for_this_pair() {
        let rep = fit_recipe(&canyon(false), &canyon_gold_target());
        let note = rep
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED)
            .unwrap_or_else(|| panic!("no specific disclosure: {}", rep.recipe.rationale));
        let controls = &note.args.iter().find(|(k, _)| *k == "controls").expect("arg").1;
        assert!(controls.contains("hsl"), "expected the colour mixer named, got {controls}");
        // And it must NOT fire on a pair the solver actually reproduces —
        // a disclosure that always fires is the blanket sentence again.
        let src = synth();
        let truth = EditRecipe {
            exposure_ev: 0.35,
            contrast: 18.0,
            highlights: -25.0,
            whites: 12.0,
            saturation: 15.0,
            ..Default::default()
        };
        let good = fit_recipe(&src, &render::develop_preview(&src, &truth));
        assert!(
            !good
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED),
            "a fit that lands must not claim a missing control: {}",
            good.recipe.rationale
        );
    }

    /// R23-6 B-7: any file may now be the reverse-fit target, so a
    /// reference that is not this frame must be WARNED about — and not
    /// refused: the user chose the file.
    #[test]
    fn a_differently_shaped_reference_is_warned_about_not_refused() {
        let src = synth(); // 192x128, aspect 1.5
        // A genuinely different shape — `thumbnail` PRESERVES aspect, so it
        // cannot build this case; cropping can.
        let tall = synth().crop_imm(0, 0, 96, 128); // aspect 0.75
        assert!(!same_frame_plausible(&src, &tall));
        assert!(same_frame_plausible(&src, &synth()));
        // A resize of the SAME frame must not trip it: aspect survives
        // `thumbnail`, and its integer rounding is what the tolerance is for.
        assert!(same_frame_plausible(&src, &synth().thumbnail(97, 97)));
        let rep = fit_recipe(&src, &tall);
        assert!(
            rep.notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_NOT_SAME_FRAME),
            "the doubt must be disclosed: {}",
            rep.recipe.rationale
        );
        // Refused would be wrong: a recipe still comes back.
        assert!(rep.err_after.is_finite());
        // …and the DISCLOSURE has to reach the number too (R24 batch 2). The
        // real cropped pair of 2026-08-17 printed "treat the result as
        // unreliable" directly beneath a confidence of 0.83, because both
        // readings are taken over populations the crop already made
        // incomparable and neither can see that. The sentence and the number
        // are one statement or the user believes the number.
        assert!(
            rep.recipe.confidence <= NOT_SAME_FRAME_CONFIDENCE_CAP,
            "a warned pair must not claim more than the cap, got {}",
            rep.recipe.confidence
        );
        // A CAP, not a verdict: the crop-only measurement behind it says the
        // residual could be framing, not that the fit is broken, so the floor
        // stays available to the two ladders that DO measure something.
        // The crop-only warning now reaches the calibrated floor (0.250)
        // because the shared evidence cap sees the incomparable populations;
        // it remains a warning with a recipe, not a refusal.
        assert!(rep.recipe.confidence >= CONFIDENCE_FLOOR);
        // And it must not touch a pair whose frames agree — the cap is keyed
        // to the warning, so a silent pair keeps whatever it earned.
        let same = fit_recipe(&src, &synth());
        assert!(
            same.recipe.confidence > NOT_SAME_FRAME_CONFIDENCE_CAP,
            "an unwarned pair must be free of the cap, got {}",
            same.recipe.confidence
        );
    }

    /// R23 review MED-3: an ADJUSTED recipe gets an adjusted REPORT.
    ///
    /// The deep reverse-fit moves a solved recipe's saturation and used to hand
    /// the solve's own notes to the result. This pins the replacement contract
    /// at the only place that can enforce it — outcome notes re-derived from the
    /// adjusted recipe's own render, solve notes carried, and the two
    /// do-no-harm sentences dropped rather than repeated about a recipe they no
    /// longer describe. No network: `rescore_report` is deterministic.
    #[test]
    fn an_adjusted_recipe_gets_re_derived_notes_not_the_solves() {
        use crate::rationale::{keys, Note};
        let (src, tgt) = (hazy_canyon_source(), vivid_warm_target());
        let solved = fit_recipe(&src, &tgt);

        // The PRIOR the deep path would hand over. Built explicitly rather than
        // taken from `solved`, so the assertions below hold whatever this
        // fixture's solve happens to produce this release — the contract is
        // about which KEYS survive an adjustment, not about one pair's numbers.
        let prior = vec![
            Note::plain(keys::FIT_NOTE_SAT_PEGGED),
            Note::plain(keys::FIT_NOTE_CAST_REJECTED),
            Note::plain(keys::FIT_NOTE_REGRESSED),
            Note::plain(keys::FIT_NOTE_JOINT_REGRESSED),
            Note::new(
                keys::FIT_NOTE_SAT_REDUCED,
                vec![("sat_fitted", "+52".into()), ("sat_now", "+26".into())],
            ),
        ];
        let mut moved = solved.recipe.clone();
        // The deep path's own fixed step (advisor::judge::FIT_ACTION_SAT_STEP,
        // not re-exported); the size is immaterial here, only that it moves.
        moved.saturation += 10.0;
        moved.clamp();
        let rep = rescore_report(&src, &tgt, &moved, solved.err_before, &prior);
        let has = |k: &str| rep.notes.iter().any(|n| n.key == k);

        // (1) The terminal-reset verdict must NOT survive. This is the arm with
        // teeth: the GUI raises 「THE REVERSE-FIT WAS DISCARDED … reset to
        // neutral」 off this key, so carrying it told the user nothing had been
        // applied while the adjusted recipe was being persisted.
        assert!(!has(keys::FIT_NOTE_REGRESSED), "the solve's terminal reset was carried over");
        assert!(!has(keys::FIT_NOTE_JOINT_REGRESSED), "…and so was its joint arm");
        // (2) Nor the do-no-harm pull-back, whose quoted pair the move breaks.
        assert!(
            !has(keys::FIT_NOTE_SAT_REDUCED),
            "a saturation the recipe no longer has was reported: {}",
            rep.recipe.rationale
        );
        assert!(
            !rep.recipe.rationale.contains("+26"),
            "the stale saturation value leaked into the rationale: {}",
            rep.recipe.rationale
        );
        // (3) The two SOLVE facts do survive — the adjustment cannot falsify
        // "the chroma chase hit the cap" or "which gate refused the curves".
        assert!(has(keys::FIT_NOTE_SAT_PEGGED), "a solve fact was dropped");
        assert!(has(keys::FIT_NOTE_CAST_REJECTED), "a solve fact was dropped");
        // (4) The outcome notes are re-DERIVED, not absent: the summary quotes
        // this recipe's own residual, and the report is self-consistent.
        assert!(
            has(keys::FIT_SUMMARY_WITH_CURVE)
                || has(keys::FIT_SUMMARY_NO_CURVE)
                || has(keys::FIT_SUMMARY_ATMOSPHERE),
            "no summary was derived"
        );
        assert_eq!(rep.err_before, solved.err_before, "err_before is the caller's, unchanged");
        assert!(rep.recipe.rationale.contains(&format!("{:.3}", rep.err_after)));
        assert_eq!(rep.recipe.saturation, moved.saturation, "the adjusted recipe rides through");
        // …and the joint family's own accounting still holds: a reading or the
        // fail-open disclosure, never both and never neither.
        assert_ne!(
            has(keys::FIT_NOTE_JOINT),
            has(keys::FIT_NOTE_JOINT_NONE),
            "the joint reading and its fail-open note must be exclusive"
        );
    }

    /// The joint family's ADDITIONAL terminal veto (R23-6 C, role 3). No
    /// fixture in this repo reaches it — by design, since it has 56× headroom
    /// over the largest non-improvement measured — so the decision itself is
    /// pinned here, both arms and the fail-open direction.
    #[test]
    fn the_terminal_check_reads_both_metrics_and_fails_open() {
        use crate::fit_zoned::{JointReading, JOINT_DRIFT_TOL};
        let j = |w: f32| {
            Some(JointReading { worst: w, worst_label: "shadows/colour", weighted: w, buckets: 6 })
        };
        // A clean fit: neither arm objects.
        assert_eq!(
            terminal_harm(0.09, 0.03, j(0.18), j(0.04)),
            TerminalHarm { scalar: false, joint: false }
        );
        // The scalar arm alone (R16's rule, untouched).
        assert!(terminal_harm(0.010, 0.030, j(0.10), j(0.02)).scalar);
        // The JOINT arm alone — the case that motivated it: the frame-global
        // number improves while the value ranges are driven apart.
        let joint_only = terminal_harm(0.09, 0.03, j(0.10), j(0.10 + JOINT_DRIFT_TOL + 0.01));
        assert!(joint_only.joint && !joint_only.scalar);
        assert!(joint_only.any(), "the two arms are OR-ed, not AND-ed");
        // …and it is BOUNDED: drift inside the tolerance is not harm.
        assert!(!terminal_harm(0.09, 0.03, j(0.10), j(0.10 + JOINT_DRIFT_TOL - 0.01)).joint);
        // FAIL-OPEN in both directions: no reading ⇒ no verdict from this
        // arm, never a silent pass dressed as approval.
        assert!(!terminal_harm(0.09, 0.03, None, j(0.9)).joint);
        assert!(!terminal_harm(0.09, 0.03, j(0.0), None).joint);
        // …but the scalar arm still stands on its own when it does.
        assert!(terminal_harm(0.01, 0.9, None, None).any());
        // It can only REJECT: a joint reading that improves cannot rescue a
        // recipe the scalar convicts.
        assert!(terminal_harm(0.010, 0.030, j(0.90), j(0.001)).any());
    }

    /// The joint reading may only LOWER the reported confidence, and the
    /// case it exists for is the one the scalar over-reports.
    #[test]
    fn the_joint_reading_can_only_lower_confidence() {
        let (unreachable_source, unreachable_target) = structural_permutation_pair();
        let rep = fit_recipe(&unreachable_source, &unreachable_target);
        let scalar_alone = confidence_from_look_err(rep.err_after);
        assert!(
            rep.recipe.confidence < scalar_alone - 0.1,
            "the joint and evidence caps may only lower the scalar claim \
              ({scalar_alone:.2} vs reported {:.2})",
            rep.recipe.confidence
        );
        assert!(rep.recipe.confidence >= CONFIDENCE_FLOOR);
        // …and on a fit that genuinely lands, it must not invent doubt.
        //
        // The canonical same-content example is the haze fixture. Its
        // evidence-limited reading is recorded above as 0.1937 -> 0.1532;
        // this assertion must therefore stay a real fit check, not an identity
        // shortcut.
        let (src, target) = haze_pair();
        let good = fit_recipe(&src, &target);
        // The canonical haze solve now lands at the calibrated floor (0.250)
        // because its shared evidence is deliberately partial. Its measured
        // look and joint readings still improve, so the fit is not a refusal.
        assert!(
            good.recipe.confidence >= CONFIDENCE_FLOOR,
            "a landed fit must not fall below the confidence floor, got {}",
            good.recipe.confidence
        );
        assert!(
            good.err_after < good.err_before
                && good.recipe.rationale.contains("Residual look error"),
            "the landed haze fit must disclose and improve its measured solve"
        );
    }

    #[test]
    fn quantile_and_cdf_are_inverse_on_a_ramp() {
        let px: Vec<[f32; 3]> = (0..4096)
            .map(|i| {
                let v = i as f32 / 4095.0;
                [v, v, v]
            })
            .collect();
        let cdf = luma_cdf(&px);
        for &x in &[0.1f32, 0.25, 0.5, 0.75, 0.9] {
            let p = cdf_at(&cdf, x);
            let back = quantile(&cdf, p);
            assert!((back - x).abs() < 0.01, "x={x} → p={p} → {back}");
        }
    }

    #[test]
    fn evidence_gate_withholds_one_sided_value_ranges() {
        let source: Vec<[f32; 3]> = (0..4096)
            .map(|i| if i % 2 == 0 { [0.10, 0.25, 0.70] } else { [0.18, 0.35, 0.82] })
            .collect();
        let target: Vec<[f32; 3]> = (0..4096)
            .map(|i| if i % 2 == 0 { [0.70, 0.25, 0.10] } else { [0.82, 0.35, 0.18] })
            .collect();
        let e = evidence_model(&source, &target);
        let blue = e
            .hue
            .iter()
            .find(|r| r.source_share > 0.4 && !r.target_populated)
            .expect("the blue source band is one-sided");
        assert_eq!(blue.weight, 0.0);
        assert!(blue.source_populated && !blue.target_populated);
    }

    /// Each source bin owns its share of one cumulative target-rank quota.
    /// A target member is consumed whole at a boundary, so rounding can drift
    /// by at most one member over the population; it must not re-charge that
    /// boundary overshoot to every later bin and starve the tail.
    #[test]
    fn rank_pairing_uses_a_cumulative_target_quota() {
        let source = (0..EVIDENCE_LUMA_BINS)
            .flat_map(|bin| {
                let value = (bin as f32 + 0.5) / EVIDENCE_LUMA_BINS as f32;
                [[value; 3]; 3]
            })
            .collect::<Vec<_>>();
        let target = (0..source.len())
            .map(|i| {
                let value = (i as f32 + 0.5) / source.len() as f32;
                [value; 3]
            })
            .collect::<Vec<_>>();
        let source_zone = vec![1.0; source.len()];
        let mut target_zone = vec![0.0; target.len()];
        target_zone[..25].fill(1.0);
        target_zone[25] = 0.5;
        let support_weights = vec![1.0; source.len()];
        let support_divergence = vec![0.0; source.len()];
        let ranges = aggregate_ranges(
            &source,
            &target,
            &source_zone,
            &target_zone,
            SupportField {
                spatial_weights: &support_weights,
                spatial_divergence: &support_divergence,
                globally_same_content: true,
            },
        );

        assert!(
            ranges
                .luma
                .iter()
                .all(|range| range.target_populated && range.target_share > 0.0),
            "every source bin must receive target rank mass: {:?}",
            ranges.luma
        );
    }

    fn assert_evidence_models_bit_equal(actual: &EvidenceModel, expected: &EvidenceModel) {
        let bits = |values: &[f32]| values.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
        assert_eq!(actual.source_pixels, expected.source_pixels);
        assert_eq!(bits(&actual.source_membership), bits(&expected.source_membership));
        assert_eq!((actual.width, actual.height), (expected.width, expected.height));
        assert_eq!(actual.spatial_supported, expected.spatial_supported);
        assert_eq!(bits(&actual.source_weights), bits(&expected.source_weights));
        assert_eq!(bits(&actual.target_weights), bits(&expected.target_weights));
        assert_eq!(bits(&actual.source_hue_weights), bits(&expected.source_hue_weights));
        assert_eq!(bits(&actual.target_hue_weights), bits(&expected.target_hue_weights));
        assert_eq!(actual.luma, expected.luma);
        assert_eq!(actual.hue, expected.hue);
        assert_eq!(actual.identifiability.to_bits(), expected.identifiability.to_bits());
        assert_eq!(bits(&actual.spatial_weights), bits(&expected.spatial_weights));
        assert_eq!(bits(&actual.spatial_divergence), bits(&expected.spatial_divergence));
        assert_eq!(actual.globally_same_content, expected.globally_same_content);
        assert_eq!(actual.population.to_bits(), expected.population.to_bits());
    }

    #[test]
    fn structure_blind_reaggregates_structural_withholding_but_keeps_population_vetoes() {
        let mut source = Vec::new();
        source.extend(std::iter::repeat_n([0.32; 3], 100));
        source.extend(std::iter::repeat_n([0.50; 3], 100));
        source.extend(std::iter::repeat_n([0.05, 0.10, 0.80], 100));
        source.extend(std::iter::repeat_n([0.68, 0.35, 0.12], 100));
        let mut target = source.clone();
        let blue_luma = luma601(&source[200]);
        target[200..300].fill([blue_luma; 3]);

        let n = source.len();
        let ones = vec![1.0; n];
        let structural_bin = evidence_luma_bin(0.32);
        let mut ingredients = evidence_model_for(&source, &target, 20, 20);
        ingredients.spatial_weights.fill(1.0);
        ingredients.spatial_divergence.fill(0.0);
        ingredients.spatial_supported.fill(true);
        ingredients.globally_same_content = false;
        for (i, pixel) in source.iter().enumerate() {
            if evidence_luma_bin(luma601(pixel)) == structural_bin {
                ingredients.spatial_weights[i] = 0.0;
                ingredients.spatial_divergence[i] = 2.0;
                ingredients.spatial_supported[i] = false;
            }
        }
        let structural = ingredients.scoped(&target, &ones, &ones);
        let withheld = &structural.luma[structural_bin];
        assert!(withheld.source_populated && withheld.target_populated);
        assert_eq!(withheld.weight, 0.0, "premise: this range is withheld only for structure");

        let blind = structural.structure_blind(&target);
        let restored = &blind.luma[structural_bin];
        assert!(restored.two_sided_share > 0.0);
        assert_eq!(restored.weight.to_bits(), restored.two_sided_share.to_bits());

        let blue = blind
            .hue
            .iter()
            .find(|range| range.label == "Blue")
            .expect("Blue evidence band");
        assert!(blue.source_populated && !blue.target_populated);
        assert_eq!(blue.weight, 0.0, "a one-sided population fact must still veto");
        let empty = blind
            .luma
            .iter()
            .find(|range| !range.source_populated && !range.target_populated)
            .expect("fixture leaves an unpopulated luma range");
        assert_eq!(empty.weight, 0.0, "an unpopulated range must remain excluded");
        assert_eq!(blind.population.to_bits(), structural.population.to_bits());
        assert_eq!(
            blind.source_membership.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            structural.source_membership.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert!(blind.spatial_supported.iter().all(|supported| *supported));

        let identical = evidence_model_for(&source, &source, 20, 20);
        assert!(identical.globally_same_content, "premise: identical frames are structurally supported");
        let expected = identical.scoped(&source, &ones, &ones);
        assert_evidence_models_bit_equal(&identical.structure_blind(&source), &expected);
    }

    #[test]
    fn differently_sized_evidence_uses_one_aligned_prefix_domain() {
        let aligned = (0..8)
            .map(|i| {
                let value = 0.1 + i as f32 * 0.1;
                [value, value * 0.9, value * 0.8]
            })
            .collect::<Vec<_>>();
        let extra = [[0.98, 0.02, 0.02]; 4];
        for (source, target) in [
            ([aligned.as_slice(), &extra].concat(), aligned.clone()),
            (aligned.clone(), [aligned.as_slice(), &extra].concat()),
        ] {
            let evidence = evidence_model_for(&source, &target, 4, 2);
            let n = source.len().min(target.len());
            assert_eq!(evidence.population, n as f32);
            assert!(evidence.luma.iter().chain(&evidence.hue).all(|range| {
                range.source_share <= 1.0 && range.target_share <= 1.0
            }));
            let ones = vec![1.0; source.len().max(target.len())];
            let scoped = evidence.scoped(&target, &ones, &ones);
            assert_evidence_models_bit_equal(&scoped, &evidence);
        }
    }

    #[test]
    fn frame_evidence_shares_match_an_analytic_four_block_golden() {
        let counts = [(1usize, 10usize), (5, 20), (9, 30), (13, 40)];
        let source = counts
            .iter()
            .flat_map(|&(bin, count)| {
                let value = (bin as f32 + 0.5) / EVIDENCE_LUMA_BINS as f32;
                std::iter::repeat_n([value; 3], count)
            })
            .collect::<Vec<_>>();
        let target = [
            std::iter::repeat_n([0.15; 3], 25).collect::<Vec<_>>(),
            std::iter::repeat_n([0.35; 3], 25).collect::<Vec<_>>(),
            std::iter::repeat_n([0.65; 3], 25).collect::<Vec<_>>(),
            std::iter::repeat_n([0.85; 3], 25).collect::<Vec<_>>(),
        ]
        .concat();
        let evidence = evidence_model_for(&source, &target, 10, 10);

        for &(bin, count) in &counts {
            let expected = count as f32 / 100.0;
            assert_eq!(evidence.luma[bin].source_share.to_bits(), expected.to_bits());
            assert_eq!(evidence.luma[bin].target_share.to_bits(), expected.to_bits());
        }
        let occupied = counts.iter().map(|&(bin, _)| bin).collect::<Vec<_>>();
        for (bin, range) in evidence.luma.iter().enumerate() {
            if !occupied.contains(&bin) {
                assert_eq!(range.source_share, 0.0);
                assert_eq!(range.target_share, 0.0);
            }
        }
    }

    #[test]
    fn blind_move_veto_counts_soft_membership_mass() {
        let source = vec![[0.4; 3]; 1_400];
        let target = source.clone();
        let frame = evidence_model(&source, &target);
        let mut zone = vec![1.0; source.len()];
        zone[1_000..].fill(0.05);
        let mut evidence = frame.scoped(&target, &zone, &zone);
        let bin = evidence_luma_bin(0.4);
        evidence.luma[bin].weight = 0.0;
        let expected_population = zone.iter().sum::<f32>();
        assert_eq!(evidence.population.to_bits(), expected_population.to_bits());
        assert!((evidence.population - 1_020.0).abs() < 0.01);

        let mut feather_move = source.clone();
        feather_move[1_000..1_060].fill([0.5; 3]);
        assert!(!moves_unsupported_range(&source, &feather_move, &evidence));
        assert!(!moves_unsupported_luma_range(&source, &feather_move, &evidence));

        let mut interior_move = source.clone();
        interior_move[..60].fill([0.5; 3]);
        assert!(moves_unsupported_range(&source, &interior_move, &evidence));
        assert!(moves_unsupported_luma_range(&source, &interior_move, &evidence));
    }

    /// A deterministic texture with structure at scales that survive the
    /// analysis thumbnail (periods of ~180-380 px, plus a little hash noise),
    /// spread over many luma bins. `family` picks the wave orientation and
    /// periods, so two families share a histogram but no structure.
    fn textured(width: u32, height: u32, family: u32) -> DynamicImage {
        let (px, py, pd) = if family == 0 { (37.0, 29.0, 61.0) } else { (23.0, 41.0, 53.0) };
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            let (fx, fy) = if family == 0 { (x as f32, y as f32) } else { (y as f32, x as f32) };
            let wave = 70.0 * (fx / px).sin() * (fy / py).cos() + 40.0 * ((fx + 2.0 * fy) / pd).sin();
            let h = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ family.wrapping_mul(0x9E37_79B9);
            let noise = ((h >> 8) & 0x1f) as f32 - 16.0;
            let v = (128.0 + wave + noise).clamp(0.0, 255.0) as u8;
            image::Rgb([v, v.saturating_add(20), v.saturating_sub(20)])
        }))
    }

    /// The bug this pins: a 1600x1067 source thumbnails to 384x256 and a
    /// 1600x1069 target to 384x257, and every evidence statistic (and the
    /// frame-wide divergence, which returns `matched` on unequal lengths)
    /// pairs pixel i with pixel i. One row decided whether the gate existed.
    #[test]
    fn analysis_pair_puts_a_one_row_taller_target_in_the_source_geometry() {
        let src = textured(1600, 1067, 0);
        let tgt = textured(1600, 1069, 0);
        assert_eq!(
            (tgt.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).height(), src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).height()),
            (257, 256),
            "premise: independent thumbnails disagree by one row"
        );
        let (s, t) = analysis_pair(&src, &tgt);
        assert_eq!((s.width(), s.height()), (384, 256));
        assert_eq!((t.width(), t.height()), (384, 256));
        // The target goes through the source's operator (box filter, rows
        // forced), not a different resampling kernel: a Lanczos3 arm here
        // changed a same-scene fit's residual by 1.8x on its own.
        assert_eq!(t.as_bytes(), tgt.thumbnail_exact(384, 256).as_bytes());
        assert_ne!(
            t.as_bytes(),
            tgt.resize_exact(384, 256, image::imageops::FilterType::Lanczos3).as_bytes(),
            "premise: the two operators differ on this texture"
        );
        // An equal-shape pair is byte-for-byte the two thumbnails it always was.
        let same = textured(1600, 1067, 1);
        let (s2, t2) = analysis_pair(&src, &same);
        assert_eq!(s2.as_bytes(), src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).as_bytes());
        assert_eq!(t2.as_bytes(), same.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).as_bytes());
    }

    /// End to end: a target from the other texture family shares the
    /// source's histogram but none of its structure, so the frame-wide
    /// same-content verdict must be false and populated luma ranges must be
    /// withheld -- and one extra target row must not change that verdict
    /// from the equal-height pair's.
    #[test]
    fn one_extra_target_row_does_not_disable_the_structural_gate() {
        let src = textured(1600, 1067, 0);
        let rotated = textured(1600, 1067, 1);
        let taller = textured(1600, 1069, 1);
        let verdicts = |evidence: &EvidenceModel| {
            evidence
                .luma
                .iter()
                .map(|r| (r.source_populated, r.target_populated, r.weight > 0.0))
                .collect::<Vec<_>>()
        };
        let equal = fit_recipe(&src, &rotated);
        let uneven = fit_recipe(&src, &taller);
        let equal_structural = equal.structural_evidence.as_ref().unwrap_or(&equal.evidence);
        let uneven_structural = uneven.structural_evidence.as_ref().unwrap_or(&uneven.evidence);
        assert!(!equal_structural.globally_same_content, "premise: the other family is divergent");
        assert!(
            equal_structural.luma.iter().any(|r| r.source_populated && r.weight <= 0.0),
            "premise: the divergent pair withholds at least one populated range"
        );
        assert!(
            !uneven_structural.globally_same_content,
            "one extra target row must not turn the same-content verdict on"
        );
        assert_eq!(
            verdicts(uneven_structural),
            verdicts(equal_structural),
            "the range verdicts must not depend on a row of rounding"
        );
        assert_eq!(
            uneven_structural.source_pixels.len(),
            equal_structural.source_pixels.len(),
            "both pairs are judged in the source's analysis geometry"
        );
    }

    #[test]
    fn evidence_weighted_objective_sees_spatial_permutation() {
        let mut source = Vec::with_capacity(4096);
        for y in 0..64 {
            for x in 0..64 {
                // Both halves carry the same 50/50 value population, but one
                // is a fine checker and the other a broad stripe. Swapping
                // them preserves every marginal statistic exactly while
                // moving genuinely different structure between cells.
                let high = if y < 32 {
                    (x + y) % 2 == 0
                } else {
                    x < 32
                };
                let v = if high { 0.78 } else { 0.22 };
                source.push([v, v, v]);
            }
        }
        let evidence = evidence_model(&source, &source);
        let mut shuffled = source.clone();
        shuffled.rotate_left(2048);
        let identity = look_err_with_evidence(&source, &source, &evidence);
        let permuted = look_err_with_evidence(&shuffled, &source, &evidence);
        assert!(permuted > identity + 0.001, "spatially wrong render must score worse: {identity} -> {permuted}");
    }


    // Mutation guard: replace BOTH `evidence.source_weights` and
    // `evidence.target_weights` in `look_err_with_evidence` with uniform
    // `vec![1.0f32; n]` values. The calibrated weighted score below must then
    // differ and this named test turns RED.
    #[test]
    fn evidence_weighted_objective_changes_the_fitted_population_score() {
        let source: Vec<[f32; 3]> = (0..4096)
            .map(|i| {
                let x = i as f32 / 4095.0;
                [0.08 + 0.84 * x, 0.10 + 0.78 * x, 0.12 + 0.70 * x]
            })
            .collect();
        let target: Vec<[f32; 3]> = source
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i < 1024 {
                    [p[0] * 0.55, p[1] * 0.55, p[2] * 0.55]
                } else {
                    [p[0] * 1.18, p[1] * 1.18, p[2] * 1.18]
                }
            })
            .collect();
        let evidence = evidence_model(&source, &target);
        let weighted = look_err_with_evidence(&source, &target, &evidence);
        let uniform = {
            let mut unweighted = evidence.clone();
            unweighted.source_weights.fill(1.0);
            unweighted.target_weights.fill(1.0);
            look_err_with_evidence(&source, &target, &unweighted)
        };
        eprintln!("EVIDENCE_OBJECTIVE_CALIBRATION weighted={weighted:.6} uniform={uniform:.6}");
        assert!((weighted - uniform).abs() > 0.01,
            "the objective must use the evidence population, not a uniform replacement");
        assert!((weighted - 0.0591).abs() < 0.005,
            "the evidence-weighted objective calibration drifted: {weighted:.6}");
    }

    #[test]
    fn cast_gate_withholds_motion_where_hue_evidence_is_zero() {
        let mut cur = Vec::with_capacity(4096);
        let mut target = Vec::with_capacity(4096);
        let mut with_cast = Vec::with_capacity(4096);
        for y in 0..64 {
            for x in 0..64 {
                let d = 0.04 * x as f32 / 63.0;
                let blue = [0.18 + d, 0.34 + d, 0.72 + d];
                cur.push(blue);
                if y < 22 {
                    target.push(if (x + y) % 2 == 0 {
                        [0.88, 0.12, 0.08]
                    } else {
                        [0.08, 0.82, 0.16]
                    });
                    with_cast.push([0.30 + d, 0.48 + d, 0.90 + d]);
                } else {
                    target.push(blue);
                    with_cast.push(blue);
                }
            }
        }
        let evidence = evidence_model(&cur, &target);
        assert!(
            evidence
                .spatial_supported
                .iter()
                .take(22 * 64)
                .all(|&supported| !supported),
            "the invented top region must have no structurally supported pixels"
        );
        assert!(
            evidence
                .spatial_supported
                .iter()
                .skip(22 * 64)
                .any(|&supported| supported),
            "the unchanged lower region must retain spatial evidence"
        );
        assert!(
            !cast_rotates_a_region(&cur, &with_cast),
            "fixture must isolate unsupported-range motion from the legacy rotation veto"
        );
        let outcome = cast_gate_outcome(&cur, &with_cast, &target, &evidence, None);
        assert!(
            outcome.rehue_blocked,
            "a global cast moved a zero-evidence hue range without triggering legacy vetoes: {outcome:?}"
        );
    }

    #[test]
    fn tone_stage_keeps_withheld_luma_range_at_identity() {
        let (w, h) = (192u32, 128u32);
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = if y < 43 {
                0.72 + 0.20 * x as f32 / (w - 1) as f32
            } else {
                0.12 + 0.42 * x as f32 / (w - 1) as f32
            };
            image::Rgb([(v * 255.0).round() as u8; 3])
        }));
        let target = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = if y < 43 {
                if (x / 3 + y / 3) % 2 == 0 { 0.70 } else { 0.96 }
            } else {
                (0.12 + 0.42 * x as f32 / (w - 1) as f32) * 1.12 + 0.025
            };
            image::Rgb([(v.clamp(0.0, 1.0) * 255.0).round() as u8; 3])
        }));
        let report = fit_recipe(&source, &target);
        assert_eq!(report.mode, FitMode::Atmosphere, "premise: the checker sky is divergent");
        let structural = report
            .structural_evidence
            .as_ref()
            .expect("Atmosphere retains the structural ruler for residual and detail work");
        let fitted = render::develop_preview(&source, &report.recipe).to_rgb8();
        let mut no_detail_recipe = report.recipe.clone();
        no_detail_recipe.clarity = 0.0;
        no_detail_recipe.texture = 0.0;
        let without_detail = render::develop_preview(&source, &no_detail_recipe).to_rgb8();
        let original = source.to_rgb8();
        let mut delta = 0.0f32;
        let mut tone_delta = 0.0f32;
        let mut withheld = 0usize;
        for y in 0..43 {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if !source_luma_is_withheld(i, structural) {
                    continue;
                }
                delta += (fitted.get_pixel(x, y)[0] as f32
                    - original.get_pixel(x, y)[0] as f32)
                    .abs()
                    / 255.0;
                tone_delta += (without_detail.get_pixel(x, y)[0] as f32
                    - original.get_pixel(x, y)[0] as f32)
                    .abs()
                    / 255.0;
                withheld += 1;
            }
        }
        assert!(withheld > 100, "fixture must contain a withheld high-luma population");
        delta /= withheld as f32;
        tone_delta /= withheld as f32;
        assert!(
            tone_delta > 0.05,
            "the structure-blind Atmosphere tone ruler did not move the population ({delta:.4}, tone {tone_delta:.4}): ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} curve={:?}; {}",
            report.recipe.exposure_ev,
            report.recipe.contrast,
            report.recipe.highlights,
            report.recipe.shadows,
            report.recipe.whites,
            report.recipe.blacks,
            report.recipe.tone_curve,
            report.recipe.rationale,
        );
        assert!(report.evidence.globally_same_content);
        assert!(report.evidence.spatial_supported.iter().all(|&supported| supported));
        let note = report
            .notes
            .iter()
            .find(|n| {
                n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE
            })
            .expect("the population ruler must be disclosed");
        let named = &note
            .args
            .iter()
            .find(|(k, _)| *k == "luma_ranges")
            .expect("luma_ranges arg")
            .1;
        let (structural_luma, _) = withheld_range_names(structural);
        assert!(!structural_luma.is_empty());
        assert_eq!(named, &structural_luma, "the note names the structural ranges excluded from Atmosphere");
    }

    #[test]
    fn confidence_identifiability_separates_low_evidence_render() {
        let px: Vec<[f32; 3]> = (0..4096)
            .map(|i| {
                let v = 0.1 + 0.8 * (i % 64) as f32 / 63.0;
                [v, v, v]
            })
            .collect();
        let mut high = evidence_model(&px, &px);
        high.identifiability = 1.0;
        let mut low = high.clone();
        low.identifiability = 0.05;
        let report = |evidence: &EvidenceModel| {
            compose_report(
                EditRecipe::default(),
                Measured {
                    err_before: 0.02,
                    err_after: 0.01,
                    joint_after: None,
                    after_px: &px,
                    tp: &px,
                    same_frame: true,
                    mode: FitMode::Full,
                    divergence: Divergence::matched(),
                    evidence,
                    structural_evidence: None,
                    defer_disclosure: false,
                },
                SolveFacts {
                    budget: None,
                    strength: None,
                    veto_luma: None,
                    veto_hue: None,
                    wb_clamped: None,
                    wb_search_bound: None,
                    wb_rotation_coverage: None,
                    wb_rotation_disclosure: None,
                    wb_foreign_hue_withheld: false,
                    wb_rotation_withheld: false,
                    sat_pegged: None,
                    cast: CastOutcome::default(),
                    cast_admitted_by_strength: None,
                    cast_admitted: None,
                    cast_projected: None,
                    evidence_refused: false,
                    sat_fitted: None,
                    regressed: None,
                    detail: (0.0, 0.0),
                    detail_withheld: false,
                    robust: None,
                    paired: false,
                    vouched_bands: None,
                    hsl: HslStageFacts::default(),
                    atmosphere_reference: AtmosphereReference::WholeFrame,
                },
            )
        };
        let high_report = report(&high);
        let low_report = report(&low);
        assert!(
            high_report.recipe.confidence - low_report.recipe.confidence > 0.5,
            "the production confidence path must expose identifiability: {} vs {}",
            high_report.recipe.confidence,
            low_report.recipe.confidence
        );
    }

    #[test]
    fn detail_fit_writes_texture_only_from_two_sided_evidence() {
        let (w, h) = (192u32, 128u32);
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let xf = x as f32 / (w - 1) as f32;
            let ripple = 0.025 * (x as f32 * 0.43).sin()
                + 0.015 * ((x + y) as f32 * 1.17).sin();
            let v = (0.18 + 0.62 * xf + ripple).clamp(0.03, 0.95);
            image::Rgb([(v * 255.0).round() as u8; 3])
        }));
        let truth = EditRecipe { texture: 100.0, ..Default::default() };
        let target = render::develop_preview(&source, &truth);
        let source_px = pixels_of(&source);
        let target_px = pixels_of(&target);
        let detail_evidence = evidence_model(&source_px, &target_px);
        let report = fit_recipe(&source, &target);
        eprintln!(
            "DETAIL_FIT clarity={:.0} texture={:.0} ident={:.3} budget=+/-{:.0}",
            report.recipe.clarity,
            report.recipe.texture,
            detail_evidence.identifiability,
            DETAIL_CONTROL_LIMIT,
        );
        assert!(
            report.recipe.texture > 0.0 || report.recipe.clarity > 0.0,
            "the reverse fit did not write either rendered detail control in {:?} mode at evidence {:.3}: {}",
            report.mode,
            detail_evidence.identifiability,
            report.recipe.rationale,
        );
        assert!(report.recipe.texture.abs() <= DETAIL_CONTROL_LIMIT);
        assert!(report.recipe.clarity.abs() <= DETAIL_CONTROL_LIMIT);
        assert!(
            report.err_after <= report.err_before + FIT_QUANT,
            "the detail-specific terminal exception exceeded its bounded look budget"
        );
        assert!(
            detail_residual(&pixels_of(&render::develop_preview(&source, &report.recipe)), &target_px, &detail_evidence)
                < detail_residual(&source_px, &target_px, &detail_evidence),
            "the emitted detail controls did not improve the supported frequency reading"
        );
        assert!(report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_DETAIL));
    }

    #[test]
    fn ground_truth_parameter_recovery_uses_the_evidence_population() {
        let Some(root) = calibration_corpus() else { return };
        let raw = root.join("source.arw");
        let source = if raw.exists() {
            render::render_to_image(&raw, &EditRecipe::default(), None, Some(ANALYZE_EDGE))
                .expect("develop calibration source.arw")
        } else {
            image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg")
        };
        let source = source.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
        let base = if raw.exists() {
            crate::pipeline::calibration_recipe(crate::pipeline::fit_calibration(&raw))
        } else {
            EditRecipe::default()
        };
        let mut truth = base.clone();
        truth.exposure_ev = 0.45;
        truth.contrast = 18.0;
        truth.highlights = -30.0;
        truth.shadows = 12.0;
        truth.whites = 8.0;
        truth.blacks = -6.0;
        truth.saturation = 14.0;
        let target = render::develop_preview(&source, &truth);
        let report = fit_recipe_from(&source, &target, &base);
        eprintln!(
            "GROUND_TRUTH truth ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} sat={:.1}; recovered ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} sat={:.1}; confidence={:.3} err={:.3}->{:.3}",
            truth.exposure_ev, truth.contrast, truth.highlights, truth.shadows, truth.whites, truth.blacks, truth.saturation,
            report.recipe.exposure_ev, report.recipe.contrast, report.recipe.highlights, report.recipe.shadows,
            report.recipe.whites, report.recipe.blacks, report.recipe.saturation, report.recipe.confidence,
            report.err_before, report.err_after,
        );
        eprintln!("GROUND_TRUTH rationale={}", report.recipe.rationale);
        let baseline_four_error = (1.65f32 - 0.45).abs()
            + (-32.2f32 - 18.0).abs()
            + (-15.6f32 - 12.0).abs()
            + (30.1f32 - -6.0).abs();
        let recovered_four_error = (report.recipe.exposure_ev - truth.exposure_ev).abs()
            + (report.recipe.contrast - truth.contrast).abs()
            + (report.recipe.shadows - truth.shadows).abs()
            + (report.recipe.blacks - truth.blacks).abs();
        assert!(
            recovered_four_error < 0.5 * baseline_four_error,
            "parameter recovery, not residual, is the gate: {recovered_four_error:.1} vs baseline {baseline_four_error:.1}"
        );
        assert!((report.recipe.exposure_ev - truth.exposure_ev).abs() < 0.35);
        assert!(report.recipe.confidence < 0.927, "a poorly identified inverse must not retain baseline confidence");
    }

    #[test]
    fn invented_sky_gradient_energy_is_not_amplified() {
        let Some(root) = calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
        let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
        let mask = image::open(root.join("sky-mask.png"))
            .expect("calibration sky-mask.png")
            .to_luma8();
        let report = fit_recipe(&source, &target);
        let fitted = render::develop_preview(&source, &report.recipe);
        let gradient = |image: &DynamicImage| {
            let rgb = image.to_rgb8();
            let resized = image::imageops::resize(
                &mask,
                rgb.width(),
                rgb.height(),
                image::imageops::FilterType::Triangle,
            );
            let mut sum = 0.0f32;
            let mut total = 0.0f32;
            for y in 1..rgb.height().saturating_sub(1) {
                for x in 1..rgb.width().saturating_sub(1) {
                    let weight = resized.get_pixel(x, y)[0] as f32 / 255.0;
                    if weight <= 0.0 {
                        continue;
                    }
                    let lum = |x, y| {
                        let p = rgb.get_pixel(x, y);
                        (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
                            / 255.0
                    };
                    let dx = 0.5 * (lum(x + 1, y) - lum(x - 1, y));
                    let dy = 0.5 * (lum(x, y + 1) - lum(x, y - 1));
                    sum += (dx * dx + dy * dy).sqrt() * weight;
                    total += weight;
                }
            }
            sum / total.max(1e-6)
        };
        let ratio = gradient(&fitted) / gradient(&source).max(1e-6);
        eprintln!(
            "SKY_GRADIENT ratio={ratio:.4}; ev={:.2} contrast={:.1} highlights={:.1} shadows={:.1} whites={:.1} blacks={:.1} sat={:.1} clarity={:.1} texture={:.1} confidence={:.3}",
            report.recipe.exposure_ev,
            report.recipe.contrast,
            report.recipe.highlights,
            report.recipe.shadows,
            report.recipe.whites,
            report.recipe.blacks,
            report.recipe.saturation,
            report.recipe.clarity,
            report.recipe.texture,
            report.recipe.confidence,
        );
        assert!(ratio <= 1.0, "invented sky was sharpened: {ratio:.4}x neutral");
        assert_eq!(report.mode, FitMode::Atmosphere);
        assert!(
            report.recipe.exposure_ev.abs()
                + report.recipe.contrast.abs()
                + report.recipe.tone_curve.len() as f32
                + report.recipe.saturation.abs()
                > 1.0,
            "content-divergent calibration pair was incorrectly returned as an empty recipe"
        );
        assert_eq!(report.recipe.saturation, 0.0);
        assert_eq!(report.recipe.clarity, 0.0);
        assert_eq!(report.recipe.texture, 0.0);
    }

    #[test]
    fn confidence_separates_calibration_recipes_by_evidence_spend() {
        let Some(root) = calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
        let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
        let conservative = fit_recipe(&source, &target);
        let text = std::fs::read_to_string(root.join("fitted.recipe.json"))
            .expect("calibration fitted.recipe.json");
        let preferred: EditRecipe = serde_json::from_str(&text).expect("saved calibration recipe");
        let rescored = rescore_report(&source, &target, &preferred, conservative.err_before, &[]);
        assert_eq!((conservative.mode, rescored.mode), (FitMode::Atmosphere, FitMode::Atmosphere));
        assert_evidence_models_bit_equal(&rescored.evidence, &conservative.evidence);
        let (thumb, _) = analysis_pair(&source, &target);
        let evidence = conservative
            .structural_evidence
            .as_ref()
            .expect("Atmosphere preserves structural diagnostics");
        let conservative_px = pixels_of(&render::develop_preview(&thumb, &conservative.recipe));
        let preferred_px = pixels_of(&render::develop_preview(&thumb, &preferred));
        let motion = |after: &[[f32; 3]]| {
            let mut supported = 0.0;
            let mut unsupported = 0.0;
            for (i, (before, after)) in evidence.source_pixels.iter().zip(after).enumerate() {
                let delta = (0..3).map(|ch| (after[ch] - before[ch]).abs()).sum::<f32>() / 3.0;
                if evidence.source_weights[i] > 0.0 { supported += delta } else { unsupported += delta }
            }
            (supported / after.len() as f32, unsupported / after.len() as f32)
        };
        let (cs, cu) = motion(&conservative_px);
        let (ps, pu) = motion(&preferred_px);
        eprintln!(
            "CONFIDENCE_SEPARATION conservative={:.3} preferred={:.3} margin={:.3} movement={:.3}/{:.3} motion_su={cs:.4}/{cu:.4} vs {ps:.4}/{pu:.4} pair_ident={:.3}",
            conservative.recipe.confidence,
            rescored.recipe.confidence,
            rescored.recipe.confidence - conservative.recipe.confidence,
            movement_identifiability(&conservative_px, evidence),
            movement_identifiability(&preferred_px, evidence),
            evidence.identifiability,
        );
        assert!(
            (rescored.recipe.confidence - conservative.recipe.confidence).abs() < 0.01,
            "confidence must use the same population ruler, independent of structural spend"
        );
        assert!(
            movement_identifiability(&preferred_px, evidence)
                > movement_identifiability(&conservative_px, evidence) + 0.1,
            "premise: the two recipes remain structurally distinguishable"
        );
        assert!(conservative.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
        assert!(rescored.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
    }

    // ---------------------------------------------------------------------
    // stage 4a — the per-band colour mixer
    // ---------------------------------------------------------------------

    /// RGB directions whose hue lands on an ACR band centre (red 0°,
    /// green 120°, blue 240°, yellow 60°, magenta 300°), each ORTHOGONAL to
    /// Rec.601 luma and normalised to unit chroma. Scaling a family's chroma
    /// therefore leaves its luma distribution untouched, so a fixture built
    /// from them isolates the colour question from the tone one.
    const BAND_DIRECTIONS: [[f32; 3]; 5] = [
        [0.70100, -0.29900, -0.29900],
        [-0.58700, 0.41300, -0.58700],
        [-0.11400, -0.11400, 0.88600],
        [0.11400, 0.11400, -0.88600],
        [0.58700, -0.41300, 0.58700],
    ];

    /// One colour family per horizontal quarter, all four riding the same
    /// luminance ramp, each family's chroma scaled independently.
    fn band_frame(families: [usize; 4], chroma: [f32; 4]) -> DynamicImage {
        let (w, h) = (384u32, 256u32);
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let xf = x as f32 / (w - 1) as f32;
            let quarter = (y * 4 / h) as usize;
            let l = 0.26 + 0.26 * xf + 0.04 * (xf * 41.0).sin();
            let u = BAND_DIRECTIONS[families[quarter]];
            let p = [0usize, 1, 2].map(|c| (l * (1.0 + chroma[quarter] * u[c])).clamp(0.0, 1.0));
            image::Rgb(p.map(|v| (v * 255.0).round() as u8))
        }))
    }

    /// `source` developed through the REAL engine with a mixer-only recipe.
    fn developed(source: &DynamicImage, edit: impl FnOnce(&mut crate::recipe::Hsl)) -> DynamicImage {
        let mut recipe = EditRecipe::default();
        edit(&mut recipe.hsl);
        render::develop_preview(source, &recipe)
    }

    /// The canonical inverse problem for this stage: the target IS the source
    /// carrying a KNOWN per-band edit, -`edit` Green / +`edit` Blue saturation.
    /// Frame-mean chroma barely moves, so the ONE global saturation number has
    /// nothing useful to say — only a per-band control can express this.
    fn engine_hsl_pair(edit: f32) -> (DynamicImage, DynamicImage) {
        let source = band_frame([0, 1, 2, 4], [0.55; 4]);
        let target = developed(&source, |hsl| {
            hsl.saturation[3] = -edit;
            hsl.saturation[5] = edit;
        });
        (source, target)
    }

    /// The same edit over TWO half-frame families, which gives each band a
    /// population big enough to move the joint chromatic buckets the
    /// unrepresented-controls disclosure reads.
    fn two_family_hsl_pair(edit: f32) -> (DynamicImage, DynamicImage) {
        let source = band_frame([2, 2, 1, 1], [0.55; 4]);
        let target = developed(&source, |hsl| {
            hsl.saturation[3] = -edit;
            hsl.saturation[5] = edit;
        });
        (source, target)
    }

    /// A demand far past the per-band ceiling: the blue quarter is 1.64x more
    /// chromatic than the source's, which +18 can only partly close.
    fn over_cap_band_pair() -> (DynamicImage, DynamicImage) {
        (
            band_frame([0, 1, 2, 4], [0.55; 4]),
            band_frame([0, 1, 2, 4], [0.55, 0.55, 0.90, 0.55]),
        )
    }

    /// The target repaints the green quarter yellow: Green exists only in the
    /// source, Yellow only in the target. Nothing else moves.
    fn one_sided_band_pair() -> (DynamicImage, DynamicImage) {
        (
            band_frame([0, 1, 2, 4], [0.55; 4]),
            band_frame([0, 3, 2, 4], [0.55; 4]),
        )
    }

    fn hsl_note<'a>(report: &'a FitReport, key: &str) -> Option<&'a crate::rationale::Note> {
        report.notes.iter().find(|n| n.key == key)
    }

    fn note_arg(report: &FitReport, key: &str, arg: &str) -> String {
        hsl_note(report, key)
            .and_then(|n| n.args.iter().find(|(k, _)| *k == arg))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    fn names_hsl(report: &FitReport) -> bool {
        report
            .notes
            .iter()
            .any(|n| n.args.iter().any(|(k, v)| *k == "controls" && v.contains("hsl")))
    }

    /// (a) A band both frames can speak for is SOLVED, and the one global
    /// saturation number demonstrably could not have carried it.
    #[test]
    fn per_band_colour_is_solved_from_two_sided_population_evidence() {
        let (src, tgt) = engine_hsl_pair(15.0);
        let report = fit_recipe(&src, &tgt);
        let budget = FitBudget::for_strength(crate::recipe::GradeStrength::default()).hsl_band;
        assert_eq!(report.mode, FitMode::Full, "premise: the fixture is a same-content pair");
        let hsl = &report.recipe.hsl;
        assert!(
            hsl.saturation[5] >= 10.0 && hsl.saturation[5] <= budget,
            "Blue must recover most of its +15: {:?}",
            hsl.saturation
        );
        assert!(
            hsl.saturation[3] <= -10.0 && hsl.saturation[3] >= -budget,
            "…and Green most of its -15: {:?}",
            hsl.saturation
        );
        // THE point of the stage: the target's frame-mean chroma is almost
        // unchanged, so the single global dial has nothing to say and the
        // recovery cannot be credited to it.
        assert!(
            report.recipe.saturation.abs() <= 5.0,
            "one global saturation cannot express opposed bands: {}",
            report.recipe.saturation
        );
        assert!(
            report.err_after < report.err_before,
            "the composed fit must end closer: {} -> {}",
            report.err_before,
            report.err_after
        );
        let moved = note_arg(&report, crate::rationale::keys::FIT_NOTE_HSL_BANDS, "moved");
        assert!(
            moved.contains("Green sat -") && moved.contains("Blue sat +"),
            "the disclosure names what moved and by how much: {moved:?}"
        );
    }

    /// (b) A band only ONE frame can speak for is refused BY NAME. One-sided
    /// is unmeasurable, never silently read as "these already match".
    #[test]
    fn a_one_sided_band_is_refused_by_name_never_read_as_equal() {
        let (src, tgt) = one_sided_band_pair();
        let report = fit_recipe(&src, &tgt);
        // Premise, straight off the shared evidence model: Green is in the
        // source alone and Yellow in the target alone.
        assert!(
            report.evidence.hue[3].source_populated && !report.evidence.hue[3].target_populated,
            "premise: Green is source-only"
        );
        assert!(
            !report.evidence.hue[2].source_populated && report.evidence.hue[2].target_populated,
            "premise: Yellow is target-only"
        );
        for band in [2usize, 3] {
            assert_eq!(report.recipe.hsl.saturation[band], 0.0, "band {band} must not move");
            assert_eq!(report.recipe.hsl.luminance[band], 0.0, "band {band} must not move");
        }
        let refused = note_arg(&report, crate::rationale::keys::FIT_NOTE_HSL_BANDS, "refused");
        assert!(
            refused.contains("Green (one-sided)") && refused.contains("Yellow (one-sided)"),
            "the refusal is typed and named, not silence: {refused:?}"
        );
    }

    /// (c) The hue axis is never written, on any pair, at any strength —
    /// including a target whose only edit IS a band rotation, where the
    /// temptation to rotate back is greatest.
    #[test]
    fn the_per_band_mixer_never_rotates_a_hue_band() {
        let rotated_source = band_frame([0, 1, 2, 4], [0.55; 4]);
        let rotated = developed(&rotated_source, |hsl| {
            hsl.hue[3] = -60.0;
            hsl.hue[5] = 60.0;
        });
        let mut pairs: Vec<(&str, DynamicImage, DynamicImage)> = vec![
            ("hue-rotated", rotated_source.clone(), rotated),
            ("solved", engine_hsl_pair(15.0).0, engine_hsl_pair(15.0).1),
            ("over-cap", over_cap_band_pair().0, over_cap_band_pair().1),
            ("one-sided", one_sided_band_pair().0, one_sided_band_pair().1),
        ];
        let (cloud_src, cloud_tgt) = flat_sky_to_cloud_deck();
        pairs.push(("cloud-deck", cloud_src, cloud_tgt));
        let (perm_src, perm_tgt) = structural_permutation_pair();
        pairs.push(("permutation", perm_src, perm_tgt));
        for (name, src, tgt) in &pairs {
            for strength in [0.0f32, 0.65, 1.0] {
                let report = fit_recipe_from_with(
                    src,
                    tgt,
                    &EditRecipe::default(),
                    FitOptions {
                        strength: crate::recipe::GradeStrength::new(strength),
                        provider: None,
                    },
                );
                assert_eq!(
                    report.recipe.hsl.hue,
                    [0.0f32; 8],
                    "{name} at strength {strength} rotated a band: {:?}",
                    report.recipe.hsl.hue
                );
            }
        }
    }

    /// (d) A move the frame ruler will not pay for is given back to zero and
    /// DISCLOSED.
    ///
    /// The cloud-deck pair is the honest fixture for this claim, and the
    /// synthetic over-cap pair is not: on that one the whole fit terminally
    /// resets, so a neutral mixer proves nothing about the mixer (measured —
    /// disabling BOTH of 4a's do-no-harm arms left that test green). Here the
    /// fit SUCCEEDS (the frame ends closer, no terminal reset) and the gate
    /// ADMITS bands, so zero can only be 4a giving its own move back.
    #[test]
    fn a_per_band_move_the_frame_ruler_refuses_shrinks_to_zero_and_says_so() {
        let (src, tgt) = flat_sky_to_cloud_deck();
        let report = fit_recipe(&src, &tgt);
        assert!(
            report.err_after < report.err_before,
            "premise: the fit itself succeeds here ({:.4} -> {:.4})",
            report.err_before,
            report.err_after
        );
        assert!(
            !report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_REGRESSED),
            "premise: no terminal do-no-harm reset stands behind the neutral mixer"
        );
        let refused = note_arg(&report, crate::rationale::keys::FIT_NOTE_HSL_BANDS, "refused");
        let admitted = (0..EVIDENCE_HUE_BANDS)
            .filter(|&band| {
                let range = &report.evidence.hue[band];
                (range.source_populated || range.target_populated)
                    && !refused.contains(range.label.as_str())
            })
            .count();
        assert!(admitted > 0, "premise: the population gate admitted a band: refused={refused:?}");
        assert!(
            report.recipe.hsl.is_neutral(),
            "the refused move must return to neutral: {:?}",
            report.recipe.hsl
        );
        assert!(
            hsl_note(&report, crate::rationale::keys::FIT_NOTE_HSL_WITHDRAWN_ERROR).is_some(),
            "…and say so: {}",
            report.recipe.rationale
        );
    }

    /// (e) The strength dial really turns this stage: the ceiling is monotone
    /// across the three stops AND it binds at each of them.
    #[test]
    fn hsl_band_budget_is_monotone_across_the_three_strength_stops() {
        let cap = |s: f32| FitBudget::for_strength(crate::recipe::GradeStrength::new(s)).hsl_band;
        assert_eq!(cap(0.0), HSL_BAND_LIMIT_MIN);
        assert_eq!(cap(0.65), HSL_BAND_LIMIT_DEFAULT);
        assert_eq!(cap(1.0), HSL_BAND_LIMIT_MAX);
        assert!(cap(0.0) < cap(0.65) && cap(0.65) < cap(1.0));
        // …and it is a real constraint, not a number nothing reads: one pair
        // whose demand (+/-25) sits between the default ceiling and the full
        // one, solved at all three stops.
        let (src, tgt) = engine_hsl_pair(25.0);
        let solved = |s: f32| {
            fit_recipe_from_with(
                &src,
                &tgt,
                &EditRecipe::default(),
                FitOptions { strength: crate::recipe::GradeStrength::new(s), provider: None },
            )
            .recipe
            .hsl
            .saturation[5]
        };
        let (low, mid, high) = (solved(0.0), solved(0.65), solved(1.0));
        assert!(low <= cap(0.0) + 1e-3, "strength 0 must not exceed its ceiling: {low}");
        assert!(
            mid > cap(0.0) && mid <= cap(0.65) + 1e-3,
            "the default stop spends past the tight ceiling and stops at its own: {mid}"
        );
        assert!(high > cap(0.65), "strength 1 spends past the default ceiling: {high}");
    }

    /// v1.2.4 B1 — what the fan gate costs on a REAL two-temperature scene,
    /// measured instead of assumed.
    ///
    /// v1.2.3 shipped the gate with its cost measured only on synthetic
    /// two-temperature frames (`two_temperature_coast`), and said so: on a
    /// real photograph lit at two colour temperatures at once the gate could
    /// in principle refuse a cast the photograph genuinely needs. Two such
    /// photographs were then found in the user's own library by searching
    /// 169 per-exemplar descriptions for mixed lighting and looking at the
    /// finished renders: `p40`, a night street under neon and shop signs
    /// beneath a fiery sunset sky, and `p41`, a building at twilight with a
    /// warm horizon band, a deep blue sky and a warm lamp.
    ///
    /// The measured answer (2026-09-02, both pairs at the shipped default)
    /// is that the gate costs these pairs NOTHING, and the margin is the
    /// interesting part: `p40`'s fitted cast reads 13.5° of added fan against
    /// the 15° line — 1.5° of headroom, the closest any real pair in the
    /// corpus comes — and ships ADMITTED as fitted, 0.0788 → 0.0335 at
    /// confidence 0.612. `p41`'s reads 9.0°, and its cast is refused by a
    /// different gate entirely (the pixel-aligned re-hue veto), so the fan
    /// gate is not what withheld it there either.
    ///
    /// Both pairs are OPTIONAL, like every other corpus pair: absent, the
    /// test says so and passes, and the synthetic two-temperature fixture
    /// carries the refusal side of the same question on its own.
    ///
    /// Mutation: halve [`FAN_DEG`] and `p40` goes from admitted to convicted,
    /// red on the fan-margin assertion.
    #[test]
    fn the_fan_gate_costs_a_real_two_temperature_pair_nothing() {
        use crate::rationale::keys;
        let Some(root) = calibration_corpus() else { return };
        for (code, want_fan, want_before, want_after, want_conf) in [
            ("p40", 13.5f32, 0.0788f32, 0.0335f32, 0.612f32),
            ("p41", 9.0, 0.0782, 0.0413, 0.437),
        ] {
            let raw = root.join(format!("{code}.arw"));
            let target_path = root.join(format!("{code}-target.jpg"));
            if !raw.is_file() || !target_path.is_file() {
                eprintln!("SKIPPED {code}: the two-temperature pair is not in this corpus");
                continue;
            }
            // Loaded exactly as CLI `match` loads a RAW: the frame developed
            // at the default recipe on a 2048 edge, against the calibration
            // recipe as the base.
            let src = render::render_to_image(&raw, &EditRecipe::default(), None, Some(2048))
                .expect("develop the two-temperature RAW");
            let tgt = image::open(target_path).expect("the finished rendition");
            let base = crate::pipeline::calibration_recipe(crate::pipeline::fit_calibration(&raw));
            let report = fit_recipe_from(&src, &tgt, &base);
            let candidate = cast_stage_candidate_from(&src, &tgt, &base);
            let evidence = evidence_model(&candidate.cur, &candidate.tp);
            let (share, fan, _) =
                hue_fan_weighted(&candidate.cur, &candidate.with_px, &evidence)
                    .expect("a two-temperature frame has a region-sized hue class");
            eprintln!(
                "TWO_TEMPERATURE {code} share={share:.3} fitted_fan={fan:.1} err={:.6}->{:.6} conf={:.6}",
                report.err_before, report.err_after, report.recipe.confidence
            );
            assert!(
                (fan - want_fan).abs() <= 0.6,
                "{code}: the fitted cast's fan moved off the measured {want_fan}°: {fan:.1}°"
            );
            assert!(
                fan < FAN_DEG,
                "{code}: the fan gate now convicts a real two-temperature pair ({fan:.1}° \
                 against {FAN_DEG}°) — that is the cost this test exists to measure"
            );
            assert!(
                !report.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_HUE_FANNED
                    || n.key == keys::FIT_NOTE_CAST_PROJECTED),
                "{code}: …so neither the refusal nor the projection may appear: {}",
                report.recipe.rationale
            );
            assert!(
                (report.err_before - want_before).abs() <= 0.002
                    && (report.err_after - want_after).abs() <= 0.002,
                "{code}: residual moved off the measured {want_before} -> {want_after}: {:.6} -> {:.6}",
                report.err_before,
                report.err_after
            );
            assert!(
                (report.recipe.confidence - want_conf).abs() <= 0.02,
                "{code}: confidence moved off the measured {want_conf}: {}",
                report.recipe.confidence
            );
        }
    }

    #[test]
    fn zz_probe_corpus() {
        let Some(root) = calibration_corpus() else { return };
        // The generated-cloud pair, then every P-coded RAW pair present.
        // Each RAW pair is loaded exactly as CLI `match` loads one: the frame
        // developed at the default recipe on a 2048 edge, against the
        // calibration recipe as the base.
        let mut pairs: Vec<(String, DynamicImage, DynamicImage, EditRecipe)> = Vec::new();
        pairs.push((
            "neutral".to_string(),
            image::open(root.join("neutral.jpg")).unwrap(),
            image::open(root.join("target.jpg")).unwrap(),
            EditRecipe::default(),
        ));
        for code in ["p36", "p37", "p38", "p39", "p40", "p41"] {
            let raw = root.join(format!("{code}.arw"));
            let tgt = root.join(format!("{code}-target.jpg"));
            if !raw.is_file() || !tgt.is_file() {
                continue;
            }
            let src = render::render_to_image(&raw, &EditRecipe::default(), None, Some(2048))
                .expect("develop");
            pairs.push((
                code.to_string(),
                src,
                image::open(tgt).unwrap(),
                crate::pipeline::calibration_recipe(crate::pipeline::fit_calibration(&raw)),
            ));
        }
        for (name, src, tgt, base) in &pairs {
            let report = fit_recipe_from(src, tgt, base);
            let (s_img, t_img) = analysis_pair(src, tgt);
            let sp = pixels_of(&render::develop_preview(&s_img, base));
            let tp = pixels_of(&t_img);
            let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
            let px = pixels_of(&render::develop_preview(&s_img, &report.recipe));
            let fanv = hue_fan_weighted(&sp, &px, &evidence);
            let has = |k: &str| report.notes.iter().any(|n| n.key == k);
            use crate::rationale::keys;
            let cast_state = if has(keys::FIT_NOTE_CAST_PROJECTED) {
                "projected"
            } else if has(keys::FIT_NOTE_CAST_HUE_FANNED) {
                "fanned"
            } else if has(keys::FIT_NOTE_CAST_ADMITTED) {
                "admitted"
            } else if has(keys::FIT_NOTE_CAST_REJECTED) {
                "rejected"
            } else if has(keys::FIT_NOTE_REHUE_BLOCKED) {
                "rehue-blocked"
            } else {
                "none"
            };
            // The as-fitted cast, i.e. what the gate refused or shrank.
            let c = cast_stage_candidate_from(src, tgt, base);
            let cand_evidence = evidence_model(&c.cur, &c.tp);
            let err_admitted = look_err_with_evidence(&c.with_px, &c.tp, &cand_evidence);
            let err_no_cast = look_err_with_evidence(&c.cur, &c.tp, &cand_evidence);
            let cand_fan = hue_fan_weighted(&c.cur, &c.with_px, &cand_evidence);
            eprintln!(
                "CORPUS pair={name} mode={:?} err={:.6}->{:.6} conf={:.6} cast={cast_state} t={} ratio={} deliv_fan={:?} cand_fan={:?} err_admitted={:.6} err_no_cast={:.6} sat={:.1} regressed={}",
                report.mode,
                report.err_before,
                report.err_after,
                report.recipe.confidence,
                note_arg(&report, keys::FIT_NOTE_CAST_PROJECTED, "t"),
                note_arg(&report, keys::FIT_NOTE_CAST_PROJECTED, "ratio"),
                fanv,
                cand_fan,
                err_admitted,
                err_no_cast,
                report.recipe.saturation,
                has(keys::FIT_NOTE_REGRESSED),
            );
        }
    }

    /// A frame whose only colour evidence is a per-band HUE gap: one field
    /// rotated, its chroma and luminance untouched.
    ///
    /// The rest of the frame is a neutral luminance ramp, so the joint
    /// LUMINANCE x CHROMA buckets are identical between the two builds and the
    /// only thing that moved is where the coloured field sits on the hue
    /// circle.
    fn band_rotation_frame(hue: f32) -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let level = 0.20 + 0.60 * y as f32 / (h - 1) as f32;
                let p = if x < w - w / 8 {
                    [level, level, level]
                } else {
                    // HSL -> RGB, written out because the engine's own
                    // converter is private to `render` and this fixture wants
                    // one hue at a fixed chroma and luminance.
                    let s = 0.22f32;
                    let c = (1.0 - (2.0 * level - 1.0).abs()) * s;
                    let hp = hue.rem_euclid(360.0) / 60.0;
                    let xx = c * (1.0 - (hp % 2.0 - 1.0).abs());
                    let (r, g, b) = match hp as u32 {
                        0 => (c, xx, 0.0),
                        1 => (xx, c, 0.0),
                        2 => (0.0, c, xx),
                        3 => (0.0, xx, c),
                        4 => (xx, 0.0, c),
                        _ => (c, 0.0, xx),
                    };
                    let m = level - c / 2.0;
                    [r + m, g + m, b + m]
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// v1.2.4 A20 — the BAND-CENTROID arm of `residual_is_colour_shaped`,
    /// tested on its own.
    ///
    /// The function has two arms and only one of them had a fixture. The
    /// chromatic-bucket arm answers "a value range of this frame's colour is
    /// far off and the neutral ranges at the same brightness are not"; the
    /// band-centroid arm answers "a populated hue band's centroid is more
    /// than [`UNREPRESENTED_HUE_DEG`] away from the target's", which is the
    /// one axis the per-band mixer is forbidden to solve. A residual made
    /// ONLY of a hue rotation leaves the first arm silent — rotating a hue
    /// changes neither the luminance nor the chroma distribution the joint
    /// buckets are built from — so this pair separates them: 30 degrees of
    /// Blue-band rotation, every chromatic bucket well under
    /// [`UNREPRESENTED_CHROMATIC_ERR`], and the disclosure still names `hsl`.
    ///
    /// Mutation: raise [`UNREPRESENTED_HUE_DEG`] to 200 (past anything a hue
    /// circle can produce) and this goes red on the colour-shape assertion,
    /// with the bucket-arm premise still passing — which is what proves the
    /// centroid arm is what was speaking.
    #[test]
    fn a_band_centroid_gap_alone_makes_the_residual_colour_shaped() {
        let after = band_rotation_frame(215.0);
        let target = band_rotation_frame(255.0);
        let (a_img, t_img) = analysis_pair(&after, &target);
        let after_px = pixels_of(&a_img);
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_for(&after_px, &tp, a_img.width(), a_img.height());

        // PREMISE — the chromatic-BUCKET arm is silent: only the hue moved,
        // so the luminance x chroma distributions are the same on both sides.
        let buckets = crate::fit_zoned::joint_buckets_with_evidence(
            &after_px,
            &tp,
            Some(&evidence.source_weights),
            Some(&evidence.target_weights),
        );
        let worst_of = |chromatic: bool| {
            buckets
                .iter()
                .filter(|b| b.chromatic == chromatic)
                .map(|b| b.err)
                .fold(0.0f32, f32::max)
        };
        let (chromatic_worst, neutral_worst) = (worst_of(true), worst_of(false));
        eprintln!(
            "BAND_CENTROID chromatic_worst={chromatic_worst:.4} neutral_worst={neutral_worst:.4}"
        );
        assert!(
            chromatic_worst < UNREPRESENTED_CHROMATIC_ERR
                || chromatic_worst < neutral_worst + UNREPRESENTED_CHROMATIC_LEAD,
            "premise: the bucket arm must be silent here ({chromatic_worst:.4} vs \
             {neutral_worst:.4})"
        );

        // …and the CENTROID arm is what speaks.
        assert!(
            residual_is_colour_shaped(&after_px, &tp, &evidence),
            "a populated band's centroid {UNREPRESENTED_HUE_DEG}° off is a per-band colour job"
        );
        let note = unrepresented_note(
            &EditRecipe::default(),
            &after_px,
            &tp,
            0.10,
            FitMode::Full,
            &evidence,
        )
        .expect("a colour-shaped residual above the floor is disclosed");
        let controls = note
            .args
            .iter()
            .find(|(k, _)| *k == "controls")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        assert!(
            controls.contains("hsl"),
            "…and the disclosure names the axis the mixer cannot reach: {controls:?}"
        );
    }

    /// v1.2.4 A7 — the PROJECTION's two abstaining clauses, rendered and
    /// asserted rather than merely reachable in principle.
    ///
    /// A projected cast discloses at least what an admitted one does, and two
    /// of those readings can decline to answer: `foreign` when the target
    /// carries no hue evidence to be foreign to, `fan` when no hue class is
    /// region-sized across two luma slices. Both abstentions had keys and
    /// translations and neither had ever been produced by a fixture, so
    /// nothing in the tree said what they print — and the failure mode they
    /// exist to prevent is a digit: `0.000` for a measurement never taken.
    ///
    /// The abstention itself is read off a FIXTURE rather than hand-written:
    /// the same colourless frame the admission's abstention test uses, put
    /// through the two censuses the projection reads, so "no region-sized
    /// class / no foreign class" is a measurement here and not an assumption.
    /// The clauses are then built from those two `None`s.
    ///
    /// Mutation: make `cast_projection_notes` fall back to
    /// `Some(0.0)` for either reading and the digit assertions go red.
    #[test]
    fn an_unmeasured_projected_reading_says_so_instead_of_printing_a_zero() {
        use crate::rationale::keys;
        // 1) The PLUMBING, measured on a frame with no chromatic mass at all.
        let grey = DynamicImage::ImageRgb8(RgbImage::from_fn(48, 48, |x, y| {
            let v = (24 + ((x * 3 + y * 2) % 200)) as u8;
            image::Rgb([v, v, v])
        }));
        let (s, t) = analysis_pair(&grey, &grey);
        let tp = pixels_of(&t);
        let cur = pixels_of(&render::develop_preview(&s, &EditRecipe::default()));
        let evidence = evidence_model(&cur, &tp);
        assert_eq!(
            foreign_hue_bins_weighted(&tp, &evidence.target_hue_weights),
            None,
            "a colourless target gives the foreign-hue census nothing to be foreign to"
        );
        assert_eq!(
            hue_fan_weighted(&cur, &cur, &evidence),
            None,
            "…and no hue class is region-sized across two luma slices"
        );

        // 2) The DISCLOSURE: a projection carrying both abstentions writes
        //    the not-measurable clauses, in the order the admission's are in,
        //    and neither prints a number.
        let abstained = cast_projection_notes(CastProjection {
            share: 0.917,
            fan_before: 37.6,
            t: 0.363,
            fan_after: None,
            ratio: 0.525,
            bound: CAST_ACCEPT_RATIO,
            rehued: 0.0,
            foreign: None,
        });
        let keys_of: Vec<&str> = abstained.iter().map(|n| n.key).collect();
        assert_eq!(
            keys_of,
            vec![
                keys::FIT_NOTE_CAST_PROJECTED,
                keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA,
                keys::FIT_NOTE_CAST_PROJECTED_FAN_NA,
            ],
            "a projection that could not measure either reading still says three things"
        );
        for note in &abstained[1..] {
            assert!(note.args.is_empty(), "a not-measurable clause carries no reading");
            let text = crate::rationale::render_one(note);
            assert!(
                !text.chars().any(|c| c.is_ascii_digit()),
                "an unmeasured reading must not print a number: {text}"
            );
            assert!(
                text.contains("not measurable"),
                "an unmeasured reading must SAY it was not measured: {text}"
            );
        }

        // 3) …and the same abstention on the SEARCH side, where it has to
        //    mean the opposite of a refusal: a census with no opinion cannot
        //    put a candidate over the projection target, so the shrink is
        //    admissible and the pair is rescued rather than refused for want
        //    of a reading.
        let judged = search_cast_projection_t(0.05, |t| CastOutcome {
            readings: Some(CastReadings {
                ratio: 1.0 - 0.1 * t,
                bound: CAST_ACCEPT_RATIO,
                foreign: None,
                rehued: 0.0,
                fan: None,
            }),
            ..CastOutcome::default()
        });
        assert!(
            judged.is_some(),
            "an abstaining fan census must CLEAR the projection target, not fail it"
        );
    }

    /// v1.2.4 A39 — the TERMINAL delivered-fan check, withdrawing arm.
    ///
    /// The fan gate is a calibration applied to one stage's candidate, and the
    /// 4b do-no-harm loop re-fits that stage after every saturation step; the
    /// FAN_DEG = 20 experiment showed the loop walking to a 19° cast that
    /// shipped and left 20.6° in the delivered sky. This is the structural
    /// re-read that closes that: the same census, on the finished render
    /// against the untouched base.
    ///
    /// The subject is the coast pair's cast AS FITTED — the curves the gate
    /// convicts at 37.6° and that therefore never ship — put into the recipe
    /// by hand, which is exactly the state a loop that walked around the gate
    /// would hand over.
    ///
    /// Mutation: widen `delivered_fan_conviction`'s test to `fan > 4.0 *
    /// FAN_DEG` and this goes red on its first assertion.
    #[test]
    fn the_terminal_check_takes_the_curves_out_of_a_fanning_render() {
        let (src, tgt) = (coast(false), coast(true));
        let (s_img, t_img) = analysis_pair(&src, &tgt);
        let base = EditRecipe::default();
        let sp = pixels_of(&render::develop_preview(&s_img, &base));
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
        let mut recipe = cast_stage_candidate(&src, &tgt).with;
        assert!(!recipe.red_curve.is_empty(), "premise: the fixture demanded a cast");
        let mut end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
        let (share, fan) = delivered_fan_conviction(&sp, &end_px, &evidence)
            .expect("premise: the as-fitted cast fans the delivered render past the limit");
        eprintln!("TERMINAL_FAN as-fitted share={share:.3} fan={fan:.1}");
        assert!(fan > FAN_DEG, "premise: {fan:.1}° must be over the {FAN_DEG}° line");

        let acted = withdraw_curves_for_delivered_fan(
            &s_img,
            &sp,
            &evidence,
            &mut recipe,
            &mut end_px,
        )
        .expect("a convicted delivered render must be acted on");
        assert_eq!(acted.2, None, "withdrawing the curves must clear the reading");
        assert!(
            recipe.red_curve.is_empty()
                && recipe.green_curve.is_empty()
                && recipe.blue_curve.is_empty(),
            "…by taking the three channel curves out of the recipe"
        );
        assert_eq!(
            delivered_fan_conviction(&sp, &end_px, &evidence),
            None,
            "…and the render the caller keeps must be the one that clears"
        );
    }

    /// …and the other arm: a delivered fan the curves did not cause is
    /// DISCLOSED, not paid for.
    ///
    /// Withdrawing a control that is not the cause would cost the user look
    /// error for nothing, so when the reading survives the withdrawal the
    /// curves go back exactly as they were and the numbers are published
    /// instead. The case is not hypothetical: `p36` delivers 12.9° of added
    /// fan carrying no cast curves at all. Here it is driven at full size by
    /// a colour-grade split — shadows and highlights sent to opposite hues,
    /// which is a per-luminance hue move by construction — on a recipe that
    /// has no channel curves to withdraw.
    ///
    /// Mutation: move `*recipe = without; *end_px = px;` out of the
    /// `still.is_none()` guard so the withdrawal is unconditional, and this
    /// goes red on the "unchanged" assertion.
    #[test]
    fn a_delivered_fan_the_curves_did_not_cause_is_disclosed_not_withdrawn() {
        let (src, tgt) = (coast(false), coast(true));
        let (s_img, t_img) = analysis_pair(&src, &tgt);
        let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
        let mut recipe = EditRecipe::default();
        recipe.color_grade.shadow_hue = 30.0;
        recipe.color_grade.shadow_sat = 100.0;
        recipe.color_grade.highlight_hue = 210.0;
        recipe.color_grade.highlight_sat = 100.0;
        recipe.color_grade.blending = 0.0;
        let before = recipe.clone();
        let mut end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
        let (share, fan) = delivered_fan_conviction(&sp, &end_px, &evidence)
            .expect("premise: the colour-grade split fans the render past the limit");
        eprintln!("TERMINAL_FAN uncaused share={share:.3} fan={fan:.1}");

        let (_, reported, still) = withdraw_curves_for_delivered_fan(
            &s_img,
            &sp,
            &evidence,
            &mut recipe,
            &mut end_px,
        )
        .expect("a convicted delivered render must be acted on");
        assert_eq!(reported, fan, "the disclosure reports the reading it convicted on");
        let still = still.expect("no curve was the cause, so the reading must survive");
        assert!(
            still > FAN_DEG,
            "…and it must still be over the line to be reported ({still:.1})"
        );
        assert_eq!(recipe, before, "the recipe must come back UNCHANGED");
        assert_eq!(
            delivered_fan_conviction(&sp, &end_px, &evidence).map(|(_, f)| f),
            Some(fan),
            "…and so must the render the caller keeps"
        );
    }

    /// The standing margin the check leaves, asserted so a change that eats it
    /// says so here rather than by silently withdrawing a fit's cast curves.
    ///
    /// Measured 2026-09-02 across the whole library battery: 108 finished
    /// Full-mode renders, the widest delivered fan among them the coast
    /// fixture's 14.2° against the 15° line. This walks the fixtures that
    /// carry a real cast and pins both halves — every one clears, and the
    /// worst is close enough to the line to be worth reading.
    ///
    /// Mutation: set `FAN_PROJECT_DEG` to `FAN_DEG` (let the projection keep
    /// twice the fan it is allowed) and the coast pair's delivered reading
    /// goes over the line, red here.
    #[test]
    fn no_shipped_fit_delivers_a_hue_fan_past_the_limit() {
        let panel_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
        let panel = image::open(panel_root.join("showcase-cornwall-reverse-fit.jpg")).unwrap();
        let (hz_src, hz_tgt) = haze_pair();
        let (tf_src, tf_tgt) = two_family_hsl_pair(50.0);
        let pairs: Vec<(&str, DynamicImage, DynamicImage)> = vec![
            ("coast", coast(false), coast(true)),
            ("two-temperature coast", coast(false), two_temperature_coast()),
            ("haze", hz_src, hz_tgt),
            ("two-family hsl", tf_src, tf_tgt),
            (
                "cornwall panel",
                panel.crop_imm(0, 136, 532, 356),
                panel.crop_imm(535, 136, 530, 356),
            ),
        ];
        let mut worst = 0.0f32;
        for (name, src, tgt) in &pairs {
            let report = fit_recipe(src, tgt);
            let (s_img, t_img) = analysis_pair(src, tgt);
            let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
            let tp = pixels_of(&t_img);
            let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
            let px = pixels_of(&render::develop_preview(&s_img, &report.recipe));
            let reading = hue_fan_weighted(&sp, &px, &evidence);
            eprintln!("DELIVERED_FAN {name} {reading:?}");
            assert_eq!(
                delivered_fan_conviction(&sp, &px, &evidence),
                None,
                "{name} delivered a hue fan past {FAN_DEG}°: {reading:?}"
            );
            if let Some((_, fan, _)) = reading {
                worst = worst.max(fan);
            }
        }
        assert!(
            (worst - 14.2).abs() < 0.5,
            "the worst delivered fan moved off the measured 14.2°: {worst:.1}"
        );
    }

    /// (f) Once the mixer has closed a band gap, the residual the
    /// unrepresented-controls disclosure reads no longer has the shape of a
    /// per-band colour job — and while the gap is only PARTLY closed it still
    /// does, and the disclosure still names `hsl`. Both halves on one pair,
    /// separated only by the budget.
    #[test]
    fn solving_the_bands_takes_the_colour_shape_out_of_the_residual() {
        let (src, tgt) = two_family_hsl_pair(50.0);
        let (s_img, t_img) = analysis_pair(&src, &tgt);
        let base = EditRecipe::default();
        let sp = pixels_of(&render::develop_preview(&s_img, &base));
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
        let fit = |s: f32| {
            fit_recipe_from_with(
                &src,
                &tgt,
                &base,
                FitOptions { strength: crate::recipe::GradeStrength::new(s), provider: None },
            )
        };

        // Default budget: +/-18 against a +/-50 demand. The residual is still
        // a per-band colour job, and the disclosure says so.
        let partial = fit(0.65);
        let partial_px = pixels_of(&render::develop_preview(&s_img, &partial.recipe));
        assert!(!partial.recipe.hsl.is_neutral(), "premise: the mixer did attach here");
        assert!(
            residual_is_colour_shaped(&partial_px, &tp, &evidence),
            "a HALF-closed band gap is still a per-band colour residual"
        );
        // …and whether the disclosure NAMES it is a second question, decided
        // by the residual's SIZE rather than its shape, and on this pair the
        // v1.2.4 projection sweep moved it. Measured 2026-09-02 on this tree:
        // the cast the fan gate convicts at 15.5° is now shrunk to t = 0.318,
        // whose look-error ratio is 0.885 — a gain of 0.0033, nearly twice
        // `FIT_QUANT` — which takes the finished residual 0.032837 → 0.022693,
        // under the `FIT_QUANT_CLEAN` = 0.025 floor at which
        // `unrepresented_note` returns early because there is nothing left to
        // explain. So this pair ships a better fit and a shorter rationale,
        // and both halves are asserted here with their numbers so neither can
        // move in silence. The `hsl` contract itself is not weakened — it is
        // asserted below on a WIDER gap, where the same half-closed shape
        // survives at a residual the floor does not silence.
        assert!(
            (partial.err_after - 0.0227).abs() < 0.001,
            "the partial fit's finished residual moved off the measured 0.0227: {}",
            partial.err_after
        );
        assert!(
            partial.err_after < FIT_QUANT_CLEAN,
            "…which is what puts it UNDER the disclosure floor {FIT_QUANT_CLEAN}: {}",
            partial.err_after
        );
        assert!(
            partial
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_PROJECTED),
            "…and the rescued cast is what bought that: {}",
            partial.recipe.rationale
        );
        assert!(
            !names_hsl(&partial),
            "…so the residual is below the size at which the disclosure speaks: {}",
            partial.recipe.rationale
        );

        // The `hsl` half of the contract, on a gap +/-18 cannot come close to
        // closing: +/-80 demanded. The mixer still attaches, the residual is
        // still a per-band colour job, and at 0.0601 it is well clear of the
        // floor — so the sentence that CAST-2's fix-up put back is still
        // asserted in the tree, on the pair that can carry it.
        let (wide_src, wide_tgt) = two_family_hsl_pair(80.0);
        let wide = fit_recipe_from_with(
            &wide_src,
            &wide_tgt,
            &base,
            FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
        );
        let (wide_s_img, wide_t_img) = analysis_pair(&wide_src, &wide_tgt);
        let wide_px = pixels_of(&render::develop_preview(&wide_s_img, &wide.recipe));
        let wide_tp = pixels_of(&wide_t_img);
        let wide_evidence = evidence_model_for(
            &pixels_of(&render::develop_preview(&wide_s_img, &base)),
            &wide_tp,
            wide_s_img.width(),
            wide_s_img.height(),
        );
        assert!(!wide.recipe.hsl.is_neutral(), "premise: the mixer attaches on the wide gap too");
        assert!(
            (wide.err_after - 0.0601).abs() < 0.002,
            "the wide pair's residual moved off the measured 0.0601: {}",
            wide.err_after
        );
        assert!(
            wide.err_after > FIT_QUANT_CLEAN,
            "…and it must stay ABOVE the disclosure floor {FIT_QUANT_CLEAN}: {}",
            wide.err_after
        );
        assert!(
            residual_is_colour_shaped(&wide_px, &wide_tp, &wide_evidence),
            "a gap this far past the ceiling is still a per-band colour residual"
        );
        assert!(
            names_hsl(&wide),
            "…so the disclosure must still name it: {}",
            wide.recipe.rationale
        );

        // Strength 1: the ceiling now covers the demand. The counterfactual is
        // the SAME finished recipe with the mixer zeroed, so the flip is
        // attributable to this stage and to nothing else in the pipeline.
        let full = fit(1.0);
        let full_px = pixels_of(&render::develop_preview(&s_img, &full.recipe));
        let mut without = full.recipe.clone();
        without.hsl = crate::recipe::Hsl::default();
        let without_px = pixels_of(&render::develop_preview(&s_img, &without));
        assert!(
            residual_is_colour_shaped(&without_px, &tp, &evidence),
            "premise: without the mixer this fit still leaves a per-band colour residual"
        );
        assert!(
            !residual_is_colour_shaped(&full_px, &tp, &evidence),
            "…and the mixer is what takes that shape out of it"
        );
        assert!(!names_hsl(&full), "…so `hsl` is no longer named: {}", full.recipe.rationale);
    }

    /// …and the other half of the same contract, on a pair whose colour
    /// residual is a per-band HUE rotation: that axis is never solved, so the
    /// disclosure must go on naming `hsl`. Closing the saturation gap must
    /// never be allowed to launder a rotation into silence.
    #[test]
    fn a_residual_the_mixer_cannot_reach_is_still_named() {
        let (src, tgt) = flat_sky_to_cloud_deck();
        let report = fit_recipe(&src, &tgt);
        assert_eq!(report.mode, FitMode::Atmosphere, "premise: the cloud deck is content-divergent");
        assert!(names_hsl(&report), "the unreachable residual stays disclosed: {}", report.recipe.rationale);
    }
}
