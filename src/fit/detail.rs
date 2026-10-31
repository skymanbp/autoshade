//! The detail stage: detail energy, the detail controls, their evidence and residual, and the bounded regression.

use super::*;

/// Conservative detail budget: one fifth of each rendered control's range.
/// The +100 texture calibration recovers useful frequency energy at this cap
/// without giving a marginal statistic permission to drive a full-strength
/// microcontrast move.
pub(super) const DETAIL_CONTROL_LIMIT: f32 = 20.0;

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

pub(super) fn detail_residual(px: &[[f32; 3]], target: &[[f32; 3]], evidence: &EvidenceModel) -> f32 {
    let coarse = detail_energy(px, &evidence.source_weights, 4);
    let fine = detail_energy(px, &evidence.source_weights, 1);
    let target_coarse = detail_energy(target, &evidence.target_weights, 4);
    let target_fine = detail_energy(target, &evidence.target_weights, 1);
    (coarse - target_coarse).abs() + (fine - target_fine).abs()
}

pub(super) fn detail_regression_is_bounded(
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

pub(super) fn only_detail_and_quantized_companions(recipe: &EditRecipe, base: &EditRecipe) -> bool {
    let wb_gains_for = |r: &EditRecipe| {
        let law = render::WbLaw::of(r);
        law.gains(law.point_of(r))
    };
    let recipe_wb = wb_gains_for(recipe);
    let base_wb = wb_gains_for(base);
    let recipe_tone = render::curve_lut_for(recipe, &recipe.tone_curve);
    let base_tone = render::curve_lut_for(base, &base.tone_curve);
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
        && recipe.hsl == base.hsl
        && recipe_tone
            .iter()
            .zip(base_tone)
            .all(|(&candidate, baseline)| (candidate - baseline).abs() <= 1.0 / 255.0)
        && recipe.red_curve == base.red_curve
        && recipe.green_curve == base.green_curve
        && recipe.blue_curve == base.blue_curve
}

pub(super) fn fit_detail_stage(
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
