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
mod atmosphere;
mod budget;
mod cast_outcome;
mod curves;
mod detail;
mod evidence;
mod gates;
mod hsl;
mod hue_gates;
mod look;
mod pairing;
mod pixels;
mod report;
mod solve;
mod structure;
mod tone;

use atmosphere::{
    WB_SHARE_SIZE_EVIDENCE, WB_VOUCH_SHRINK_STEPS, WbCellVerdict, cast_admitted_by_strength,
    fit_atmosphere_from_parts, project_curve_slopes, solve_white_balance,
};
#[cfg(test)]
use atmosphere::{WbFacts, withhold_atmosphere_tone};
pub use budget::{DIVERGENCE_GLOBAL, FitBudget, FitOptions, GlobalCast, VetoPolicy};
pub(crate) use budget::{ANALYZE_EDGE, DIVERGENCE_ZONE, DIVERGENT_COVER_PROMOTES};
use budget::{
    ATMOSPHERE_CONFIDENCE_CAP, ATMOSPHERE_CURVE_SLOPE_MAX, ATMOSPHERE_SAT_LIMIT,
    COARSE_SIGMA_DIVISOR, HIST_BINS, MODE_MARGIN_DISCLOSED, RESIDUAL_SLOPE_CAP, WB_SEARCH_K,
    budgeted_wb, detect_global_cast, mean_chroma_vector, wb_gain_ratio, wb_gains_fit_budget,
    wb_path_between, wb_path_candidate,
};
#[cfg(test)]
use budget::{
    ATMOSPHERE_CURVE_SLOPE_MIN, ATMOSPHERE_EV_LIMIT, ATMOSPHERE_WB_GAIN_MAX, ATMOSPHERE_WB_GAIN_MIN,
    ATMOSPHERE_WB_GAIN_RATIO, HSL_BAND_LIMIT_DEFAULT, HSL_BAND_LIMIT_MAX, HSL_BAND_LIMIT_MIN,
};
pub(crate) use cast_outcome::{carried_strength_from_notes};
use cast_outcome::{
    CastOutcome, CastProjection, CastReadings, FIT_QUANT, cast_admission_notes,
    cast_gate_outcome_with_ratio, cast_projection_notes, terminal_harm, unrepresented_note,
};
#[cfg(test)]
use cast_outcome::{
    FIT_QUANT_CLEAN, TerminalHarm, UNREPRESENTED_CHROMATIC_ERR, UNREPRESENTED_CHROMATIC_LEAD,
    UNREPRESENTED_HUE_DEG, cast_gate_outcome, residual_is_colour_shaped,
};
use curves::{band_stats_weighted, residual_channel_curve_weighted, search_cast_projection};
#[cfg(test)]
use curves::{
    band_stats, cast_curves_are_identity, projected_cast_curves, residual_channel_curve,
    search_cast_projection_t,
};
use detail::{detail_regression_is_bounded, fit_detail_stage, only_detail_and_quantized_companions};
#[cfg(test)]
use detail::{DETAIL_CONTROL_LIMIT, detail_residual};
pub use evidence::{
    Divergence, EvidenceModel, EvidenceRange, FitMode, evidence_hue_band, evidence_luma_bin,
    evidence_model_for,
};
pub(crate) use evidence::{
    UNSUPPORTED_RANGE_MOVE, evidence_has_one_sided, luma_evidence_for_bins, withheld_range_names,
};
use evidence::{
    DETAIL_EVIDENCE_MIN_IDENTIFIABILITY, EVIDENCE_HUE_BANDS, EVIDENCE_LUMA_BINS, EVIDENCE_MIN_SHARE,
    EVIDENCE_RANGE_SURVIVAL_MIN, atmosphere_wb_from_populations, atmosphere_wb_pairing,
    divergent_range_names, movement_identifiability, source_hue_is_withheld,
    source_luma_is_withheld, weighted_cdf, weighted_mean, weighted_mean_chroma,
};
#[cfg(test)]
pub(crate) use evidence::{evidence_model};
#[cfg(test)]
use evidence::{SupportField, WB_OPPOSING_SHARE, aggregate_ranges, wb_axes_by_range, wb_consensus};
pub(crate) use gates::{P_CLIP, clamp_confidence};
use gates::{
    CAST_ACCEPT_RATIO, CONFIDENCE_CEIL, CONFIDENCE_FLOOR, FAN_DEG, FAN_HUE_CLASSES, FAN_PROJECT_DEG,
    FAN_SHARE, FIT_FAR_ERR, NEUTRAL_SHARED_MIN, PROJECT_GRID, PROJECT_REFINE, ROT_DEG,
    ROT_HUE_MEASURABLE_CHROMA, ROT_SHARE, UNSUPPORTED_MOVEMENT_CONFIDENCE_SLOPE, VETO_CREATED_SHARE,
    VETO_FAR_BINS, VETO_MIN_TARGET_CHROMATIC, VETO_SUPPORT_BIN_MIN, VETO_SUPPORT_CHROMA,
    VETO_TINT_CHROMA, confidence_from_look_err,
};
#[cfg(test)]
use gates::{CONFIDENCE_SLOPE, NEUTRAL_MISPREDICTION_MAX, ROT_VISIBLE_AFTER, ROT_VISIBLE_BEFORE};
use hsl::{HslStageFacts, HslWithdrawal, fit_hsl_stage, halved_hsl};
pub(crate) use hue_gates::{
    CellArm, PixelVouch, cell_vouched_hue_band_names, moved_unsupported_hue_range_names,
    moved_unsupported_hue_range_names_vouched, moved_unsupported_luma_range_names,
    moves_unsupported_range, vouched_hue_band_names,
};
use hue_gates::{
    cast_paints_foreign_hues, cast_paints_foreign_hues_weighted, cast_rotates_a_region_weighted,
    foreign_created_share_weighted, hue_fan_weighted, moves_unsupported_luma_range,
    rehued_coverage_weighted, rehued_share_weighted, wb_moves_pixels_into_foreign_hues,
    withdraw_curves_for_delivered_fan,
};
#[cfg(test)]
use hue_gates::{
    cast_rotates_a_region, delivered_fan_conviction, foreign_hue_bins, foreign_hue_bins_weighted,
    foreign_share, rehued_share,
};
pub(crate) use look::{look_err_with_evidence};
use look::{round1, round2};
#[cfg(test)]
pub(crate) use look::{calibration_corpus, calibration_recipe, look_err};
pub use pairing::{
    CorrespondenceProvider, FitReport, same_frame_plausible, same_frame_plausible_dims,
};
pub(crate) use pairing::{
    AtmosphereReference, CONFIDENT_MATCH, PairCorrespondence, SHARED_POPULATION_MIN_RETENTION,
    SharedPopulation, analysis_pair, correspondence_for_pair,
};
use pairing::{NOT_SAME_FRAME_CONFIDENCE_CAP, shared_content_population};
pub use pixels::{luma601, pixels_of};
pub(crate) use pixels::{cdf_at, quantile};
use pixels::{
    DEGENERATE_LUMA_VAR, is_neutralish, luma_variance, mean_chroma, tone_cdf_pair_weighted,
};
#[cfg(test)]
pub(crate) use pixels::{neutral_gate_misprediction};
#[cfg(test)]
use pixels::{luma_cdf, tone_cdf_pair};
pub use report::{rescore_report};
pub(crate) use report::{append_finished_disclosure};
use report::{Measured, SolveFacts, compose_report};
pub use solve::{fit_recipe, fit_recipe_from, fit_recipe_from_with, fit_recipe_with};
pub(crate) use solve::{fit_recipe_from_promoted_with_disclosure_opts};
#[cfg(test)]
pub(crate) use solve::{fit_recipe_from_promoted, fit_recipe_from_promoted_with_disclosure};
pub use structure::{DivergencePair, PairingScale, STRUCTURE_MIN_CORE_PX, structure_divergence};
pub(crate) use structure::{divergence_pair_for, divergence_raster, structure_divergence_for};
pub(crate) use tone::{
    PairedRobustTone, ROBUST_REJECT_DISCLOSE_MIN, SUPPORT_MIN_PIXELS, fit_tone_sliders_supported,
    knot_support_for, paired_robust_tone, refine_tone_sliders_on, sample_tone_points,
};
use tone::{converges_toward, paired_correspondence, residual_tone_curve_with_budget};
#[cfg(test)]
pub(crate) use tone::{fit_tone_sliders};
#[cfg(test)]
use tone::{TONE_PRIOR, residual_tone_curve};

/// The reverse fit's source as ONE text, for the source-text gates that read what
/// `fit.rs` alone held before it was split into files: the modules in their
/// declaration order, the root last (so `source_before_tests` cuts at the root's
/// own test modules and nowhere else). Test-only: nothing here is compiled into a
/// shipped binary.
#[cfg(test)]
pub(crate) const SOURCE_FILES: [(&str, &str); 17] = [
    ("src/fit/atmosphere.rs", include_str!("fit/atmosphere.rs")),
    ("src/fit/budget.rs", include_str!("fit/budget.rs")),
    ("src/fit/cast_outcome.rs", include_str!("fit/cast_outcome.rs")),
    ("src/fit/curves.rs", include_str!("fit/curves.rs")),
    ("src/fit/detail.rs", include_str!("fit/detail.rs")),
    ("src/fit/evidence.rs", include_str!("fit/evidence.rs")),
    ("src/fit/gates.rs", include_str!("fit/gates.rs")),
    ("src/fit/hsl.rs", include_str!("fit/hsl.rs")),
    ("src/fit/hue_gates.rs", include_str!("fit/hue_gates.rs")),
    ("src/fit/look.rs", include_str!("fit/look.rs")),
    ("src/fit/pairing.rs", include_str!("fit/pairing.rs")),
    ("src/fit/pixels.rs", include_str!("fit/pixels.rs")),
    ("src/fit/report.rs", include_str!("fit/report.rs")),
    ("src/fit/solve.rs", include_str!("fit/solve.rs")),
    ("src/fit/structure.rs", include_str!("fit/structure.rs")),
    ("src/fit/tone.rs", include_str!("fit/tone.rs")),
    ("src/fit.rs", include_str!("fit.rs")),
];

#[cfg(test)]
pub(crate) fn source_text() -> String {
    SOURCE_FILES.iter().map(|(_, text)| *text).collect()
}

#[cfg(test)]
mod tests;

