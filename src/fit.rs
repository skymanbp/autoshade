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
//! The solve has two parts (v1.18.0, RULING-1180-joint-r1/r2). The STAGES
//! estimate: each fits one statistic and writes its estimate into the
//! recipe, and their identification knowledge (knot support, withheld
//! ranges, band admission) shapes the start —
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
//!   2. **White balance** — the per-pixel chromaticity demand, closed-loop
//!      through the engine's own law, inside the strength budget's gains.
//!   3. **Saturation** — evidence-weighted mean-chroma ratio, secant-refined through real
//!      [`render::develop_preview`] renders (closed loop, not open-loop math);
//!      then the colour-grade wheels.
//!   4. **Detail** — coarse and fine local-luma energy drive `clarity` and
//!      `texture`, each capped at ±20 and enabled only when enough shared
//!      structural and luma-range evidence survives. Outside J* by
//!      construction: the detail ruler is its own.
//!   5. **Per-band colour mixer and the colour cast** — `hsl.saturation`,
//!      `hsl.luminance` and the band hues from each ACR band's OWN population
//!      statistics, admitted by the two-sided population gate the rest of the
//!      module reads and budgeted by strength; the per-channel CDF residuals
//!      as red/green/blue curves, whose SHAPE only the curve stage solves.
//!
//! Then the JOINT OBJECTIVE judges ([`joint`]): one function the global solve
//! minimises, `F(x) = J*(render(x)) + FIT_QUANT · dof(x)`, over every
//! coordinate the stages estimated, under constraints that only reject
//! (`G`, read on every candidate render: the strength budget's white-balance
//! gains, and the ONE control-validity law ([`validity`]) on each class
//! between its own two frames in the stages' order — the tone, the white
//! balance, the chroma dials, the channel curves — a class may leave no
//! evidence cell worse on its own axes than it found it, and may not move a
//! cell no evidence speaks for). A deterministic coordinate descent polishes the
//! start; the self-consistency law is then read whole (a control's ABSENCE
//! is the recipe with it at the base and pinned, re-polished; it is dropped
//! when the absence is no worse on F — a control that does not buy its own
//! rounding is no control); and the base recipe is itself a candidate with
//! no degrees of freedom, which ships when the solve is no better on F.
//! Nothing is judged twice on two rulers: no stage gives its estimate back,
//! halves it, or holds it to do-no-harm on its own; the elimination and the
//! base candidate are the only judgements, and they read F.
//!
//! `hsl.hue` is solved only where the target's own cells vouch the band
//! (`fit_band_hues`): rotating a band re-populates it, so its pixel evidence
//! is circular, and a plausible in-gate centroid delta applied as a
//! whole-band rotation is what caused the 2026-07-07 purple-sky failure. A
//! band's mean chroma and mean luma are ordinary marginal statistics of a
//! sub-population and carry no such trap.
//!
//! Every stage fits the RESIDUAL against a fresh render of the current recipe,
//! so stage interactions are absorbed instead of compounding, and the joint
//! descent renders every candidate; the report carries
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
mod grade;
mod hsl;
mod hue_gates;
mod joint;
mod look;
mod pairing;
mod pixels;
mod report;
mod solve;
mod structure;
mod tone;
mod validity;

use atmosphere::{fit_atmosphere_from_parts, project_curve_slopes, solve_white_balance};
pub use budget::{DIVERGENCE_GLOBAL, FitBudget, FitOptions, GlobalCast, VetoPolicy};
pub(crate) use budget::{ANALYZE_EDGE, DIVERGENCE_ZONE, DIVERGENT_COVER_PROMOTES};
use budget::{
    ATMOSPHERE_CONFIDENCE_CAP, ATMOSPHERE_CURVE_SLOPE_MAX, ATMOSPHERE_SAT_LIMIT,
    COARSE_SIGMA_DIVISOR, HIST_BINS, MODE_MARGIN_DISCLOSED, RESIDUAL_SLOPE_CAP, WB_SEARCH_K,
    budgeted_wb, detect_global_cast, mean_chroma_vector, wb_gains_fit_budget,
};
#[cfg(test)]
use budget::{
    ATMOSPHERE_CURVE_SLOPE_MIN, ATMOSPHERE_EV_LIMIT, ATMOSPHERE_WB_GAIN_MAX, ATMOSPHERE_WB_GAIN_MIN,
    ATMOSPHERE_WB_GAIN_RATIO, HSL_BAND_LIMIT_DEFAULT, HSL_BAND_LIMIT_MAX, HSL_BAND_LIMIT_MIN,
    wb_path_candidate,
};
pub(crate) use cast_outcome::{FIT_QUANT, carried_strength_from_notes, ships, ships_local};
use cast_outcome::{
    control_names, earns_on_its_band, global_controls, BandReading, Control, CAST_CURVES,
    unrepresented_note,
};
#[cfg(test)]
use cast_outcome::{
    FIT_QUANT_CLEAN, UNREPRESENTED_CHROMATIC_ERR, UNREPRESENTED_CHROMATIC_LEAD,
    UNREPRESENTED_HUE_DEG, residual_is_colour_shaped,
};
use curves::{
    band_stats_weighted,
    refine_curves_on_finished_frame, residual_channel_curve_weighted,
};
#[cfg(test)]
use curves::{
    band_stats, residual_channel_curve,
};
use detail::{detail_regression_is_bounded, fit_detail_stage, only_detail_and_quantized_companions};
#[cfg(test)]
use detail::{DETAIL_CONTROL_LIMIT, detail_residual};
pub use evidence::{
    Divergence, EvidenceModel, EvidenceRange, FitMode, evidence_hue_band, evidence_luma_bin,
    evidence_model_paired,
};
#[cfg(test)]
pub use evidence::{evidence_model_for};
pub(crate) use evidence::{
    UNSUPPORTED_RANGE_MOVE, evidence_has_one_sided, luma_evidence_for_bins, withheld_range_names,
};
use evidence::{
    DETAIL_EVIDENCE_MIN_IDENTIFIABILITY, EVIDENCE_HUE_BANDS, EVIDENCE_LUMA_BINS, EVIDENCE_MIN_SHARE,
    EVIDENCE_RANGE_SURVIVAL_MIN, atmosphere_wb_from_populations, atmosphere_wb_pairing, paired_wb_closed_loop, paired_wb_from_populations,
    divergent_range_names, movement_identifiability, source_hue_is_withheld,
    source_luma_is_withheld, weighted_cdf, weighted_mean_chroma,
};
#[cfg(test)]
pub(crate) use evidence::{evidence_model};
#[cfg(test)]
use evidence::{SupportField, WB_OPPOSING_SHARE, aggregate_ranges, wb_axes_by_range, wb_axes_by_region, wb_common, wb_consensus};
pub(crate) use gates::{P_CLIP, clamp_confidence};
use gates::{
    CONFIDENCE_CEIL, CONFIDENCE_FLOOR, FIT_FAR_ERR, NEUTRAL_SHARED_MIN, ROT_SHARE,
    UNSUPPORTED_MOVEMENT_CONFIDENCE_SLOPE, confidence_from_look_err,
};
#[cfg(test)]
use gates::{CONFIDENCE_SLOPE, NEUTRAL_MISPREDICTION_MAX};
use hsl::{HslStageFacts, fit_hsl_stage};
pub(crate) use hue_gates::{
    PixelVouch, moved_unsupported_hue_range_names_vouched, moved_unsupported_luma_range_names,
    moves_unsupported_range, vouched_hue_band_names,
};
#[cfg(test)]
pub(crate) use hue_gates::moved_unsupported_hue_range_names;
pub(crate) use look::{look_err_with_evidence, look_terms};
use look::{round1, round2};
#[cfg(test)]
pub(crate) use look::{calibration_corpus, calibration_recipe};
pub use pairing::{
    CorrespondenceProvider, FitReport, same_frame_plausible, same_frame_plausible_dims,
};
pub(crate) use pairing::{
    AtmosphereReference, CONFIDENT_MATCH, PairCorrespondence, SHARED_POPULATION_MIN_RETENTION,
    SharedPopulation, analysis_pair, correspondence_for_pair, presence_of, with_presence,
};
use pairing::{NOT_SAME_FRAME_CONFIDENCE_CAP, shared_content_population};
pub use pixels::{luma601, pixels_of};
pub(crate) use pixels::cie_lab;
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
pub(crate) use structure::{divergence_pair_for, divergence_raster, structure_divergence_for, DivergenceRaster};
pub(crate) use tone::{
    PairedRobustTone, ROBUST_REJECT_DISCLOSE_MIN, SUPPORT_MIN_PIXELS, fit_tone_sliders_supported,
    knot_support_for, paired_robust_tone, refine_tone_sliders_on, sample_tone_points,
};
use tone::{converges_toward, paired_correspondence, residual_tone_curve_with_budget};
#[cfg(test)]
pub(crate) use tone::{fit_tone_sliders};
#[cfg(test)]
use tone::{TONE_PRIOR, residual_tone_curve};

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod self_consistency;

