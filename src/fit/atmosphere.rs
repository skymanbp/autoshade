//! White balance and atmosphere: the cell verdicts, the white-balance solve, the atmosphere fit from parts, and curve-slope projection.

use super::*;

/// The paired cells' verdict on a rendered white balance, and what came of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WbCellVerdict {
    /// The verdict on the FULL solved move.
    pub(super) vouch: crate::fit_cells::CellVouch,
    pub(super) admitted: bool,
    /// R36 for the white balance (2026-10-21): where the full move was
    /// refused on size with its direction agreed, the share of it that
    /// shipped and the verdict that admitted that share. `None` when the
    /// full move shipped or nothing did.
    pub(super) shipped_at: Option<(f32, crate::fit_cells::CellVouch)>,
}

/// Bisection renders of the white-balance share search, the zone search's
/// own count (`fit_zoned::ZONE_VOUCH_SHRINK_STEPS`): six halvings resolve the
/// share to 1/64, finer than the 50 K / 0.1 tint the path is quantised to.
pub(super) const WB_VOUCH_SHRINK_STEPS: usize = 6;

/// The SIZE evidence a shipped share must carry: its diverged share may be at
/// most this fraction of the full move's. "Refused on size" is a claim that
/// the refusing cells were pushed PAST their own targets, and the test of
/// that claim is that a smaller move un-pushes them; cells that keep
/// diverging at the share the search lands on were wrong for the move, not
/// too large for it. Measured 2026-10-21 on the seaside street: the full warm
/// cast read 0.105 diverged (the blue sky's cells), the bisection found
/// 0.969 of it reading 0.097 — under the 0.10 line by threshold-crossing, not
/// by size — and shipped an orange sky over a target whose sky is blue.
pub(super) const WB_SHARE_SIZE_EVIDENCE: f32 = 0.5;

/// What the white-balance stage decided, in the shape the report needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct WbFacts {
    /// `(ratio_before, ratio_after, rotated_share, rotation_coverage)` when
    /// the scalar was reduced at all.
    pub(super) clamped: Option<(f32, f32, f32, f32)>,
    /// The free search landed on the finite Kelvin domain's edge.
    pub(super) search_bound: Option<f32>,
    /// Coverage of the rotation census the gates were read on.
    pub(super) rotation_coverage: f32,
    /// `(share, coverage)` when the rotation budget alone forced it to zero.
    pub(super) rotation_disclosure: Option<(f32, f32)>,
    pub(super) foreign_hue_withheld: bool,
    pub(super) rotation_withheld: bool,
}

/// THE white-balance stage, for BOTH modes.
///
/// The caller brings the free `(k, tint)` its own population estimated and the
/// anchor it is measured from; this function decides how much of that demand
/// is spent — the shipped default persists an in-budget free WB verbatim and
/// leaves an out-of-budget one at as-shot, while above default [`budgeted_wb`]
/// is the sole producer and its scalar lambda is bisected down until neither
/// the foreign-hue checks nor the weighted rotation census objects.
///
/// EXTRACTED, not copied: this block used to live inside
/// [`fit_atmosphere_from_parts`], and that was the whole reason Full mode had
/// no white balance at all — the one bounded WB solve the crate owns was
/// reachable only through the divergent-pair branch, so the commonest pair
/// there is (the same frame, regraded) could never be told that its light had
/// changed colour. Behaviour is unchanged for the Atmosphere caller, line for
/// line; what is new is that a second caller exists.
/// What a white-balance STEP did to the frame, read by the white-balance
/// stage's two content gates: did it paint, or move pixels into, hues the
/// target holds nowhere, and what evidence-weighted share of the frame did it
/// re-hue. `before` is the render the stage found (tone, no white balance
/// yet), `after` the same recipe under the candidate white balance and
/// nothing else. One reader, two callers: [`solve_white_balance`] above the
/// default strength, and the cast→WB hand-off, whose candidate white balance
/// must clear the gates the stage's own would have had to (2026-10-26).
pub(super) struct WbStepReading {
    pub(super) foreign: bool,
    pub(super) rotated: f32,
}

pub(super) fn read_wb_step(
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> WbStepReading {
    WbStepReading {
        foreign: cast_paints_foreign_hues(before, after, tp)
            || wb_moves_pixels_into_foreign_hues(before, after, tp),
        rotated: rehued_share_weighted(before, after, evidence),
    }
}

impl WbStepReading {
    /// Both gates clear: no foreign hue, and a re-hued share inside the
    /// strength budget's `wb_rotation_share`.
    pub(super) fn admits(&self, budget: FitBudget) -> bool {
        !self.foreign && self.rotated <= budget.wb_rotation_share
    }
}

/// Whether a white-balance step answers to [`WbStepReading`] at all: above
/// the shipped default strength only. At or below it the stage persists an
/// in-budget white balance unread (the pre-F1 path, byte for byte).
pub(super) fn wb_step_gated(strength: crate::recipe::GradeStrength) -> bool {
    strength.get() > crate::recipe::GradeStrength::DEFAULT
}

#[allow(clippy::too_many_arguments)]
pub(super) fn solve_white_balance(
    s_img: &DynamicImage,
    tp: &[[f32; 3]],
    recipe: &mut EditRecipe,
    base: &EditRecipe,
    evidence: &EvidenceModel,
    (wb_k, wb_tint): (f32, f32),
    anchor: f32,
    budget: FitBudget,
    strength: crate::recipe::GradeStrength,
) -> WbFacts {
    // No demand is no stage (2026-10-21). The estimator answers the anchor
    // itself, at this solve's own 50 K quantisation and tint 0, when the
    // frame's brightness ranges have no colour change in common; persisting
    // that as a white balance wrote a rounded copy of the as-shot Kelvin into
    // the recipe and then had the cells "refuse" a move that moved nothing
    // (the seaside street: 5226 K as shot, 5200 K solved, 0.000 converged,
    // "returned to as-shot"). An exact comparison, not a tolerance: the
    // estimator returns exactly this pair and nothing else rounds to it
    // without being it.
    if wb_k == (anchor / 50.0).round() * 50.0 && wb_tint == 0.0 {
        return WbFacts::default();
    }
    let wb_search_bound = (strength.get() > crate::recipe::GradeStrength::DEFAULT
        && (wb_k <= WB_SEARCH_K.0 || wb_k >= WB_SEARCH_K.1))
        .then_some(wb_k);
    let before_wb_px = pixels_of(&render::develop_preview(s_img, recipe));
    let ratio_before = wb_gain_ratio(render::wb_gains(anchor, wb_k, wb_tint));
    let mut clamped_ratio = ratio_before;
    let mut wb_clamped = false;
    let mut wb_foreign_hue_withheld = false;
    let mut wb_rotation_withheld = false;
    let mut wb_rotated_share = 0.0f32;
    let mut wb_rejected_rotation_share = 0.0f32;
    let mut wb_rotation_coverage = 0.0f32;

    if !wb_step_gated(strength) {
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
            let WbStepReading { foreign, rotated } = read_wb_step(&before_wb_px, &after, tp, evidence);
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
                let (foreign, rotated, _, _, _) = evaluate(middle);
                if (WbStepReading { foreign, rotated }).admits(budget) {
                    legal = middle;
                } else {
                    illegal = middle;
                }
            }
            lambda = legal;
            let (_, final_rotated, _, _, _) = evaluate(lambda);
            rotated = final_rotated;
            wb_rotation_coverage = rehued_coverage_weighted(evidence);
            // Retain the reason that actually forced the scalar to zero. If
            // both gates reject the free demand, foreign hue is the stronger
            // content veto and owns the typed disclosure.
            wb_foreign_hue_withheld = foreign_limited && lambda <= 1e-5;
            wb_rotation_withheld = rotation_limited_initial && !wb_foreign_hue_withheld && lambda <= 1e-5;
            rotation_limited = rotation_limited || rotated > budget.wb_rotation_share;
        }
        if lambda <= 1e-5 {
            // This is the only new WB reset above default: grepping
            // `temperature_k = base.temperature_k` finds this guard and the
            // Atmosphere luma-veto reset (`withhold_atmosphere_tone`), exactly
            // two code sites. The terminal do-no-harm `recipe = base.clone()`
            // is a whole-recipe reset, not a WB one.
            recipe.temperature_k = base.temperature_k;
            recipe.tint = base.tint;
            clamped_ratio = 1.0;
        } else {
            let (chosen_k, chosen_tint) = wb_path_candidate(anchor, wb_k, wb_tint, lambda);
            recipe.temperature_k = Some(chosen_k);
            recipe.tint = chosen_tint;
            let chosen_px = pixels_of(&render::develop_preview(s_img, recipe));
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
    WbFacts {
        clamped: wb_clamped.then_some((
            ratio_before,
            clamped_ratio,
            wb_rotated_share,
            wb_rotation_coverage,
        )),
        search_bound: wb_search_bound,
        rotation_coverage: wb_rotation_coverage,
        rotation_disclosure: wb_rotation_withheld
            .then_some((wb_rejected_rotation_share.max(wb_rotated_share), wb_rotation_coverage)),
        foreign_hue_withheld: wb_foreign_hue_withheld,
        rotation_withheld: wb_rotation_withheld,
    }
}

/// The Atmosphere luma veto's WITHHOLD arm: the exposure, white-balance and
/// tone stages return to the base, and the WB clamp fact goes with them — its
/// "reduced from X to Y" sentence describes a white balance the shipped
/// recipe no longer carries. Reachable between the shipped default and the
/// 0.85 disclosure threshold, where the stage clamps and this arm still
/// withholds. The withheld flags stay: they name a scalar the stage had
/// already forced to zero, which the shipped as-shot value still is; the
/// search-bound fact is about the demand, not the delivered value.
pub(super) fn withhold_atmosphere_tone(recipe: &mut EditRecipe, base: &EditRecipe, facts: &mut WbFacts) {
    recipe.exposure_ev = base.exposure_ev;
    recipe.temperature_k = base.temperature_k;
    recipe.tint = base.tint;
    recipe.tone_curve = base.tone_curve.clone();
    facts.clamped = None;
}

/// Bounded global solve used when structural correspondence has failed. It
/// deliberately has no local-symptom branches: one budget table governs a
/// robust exposure/WB/tone/saturation atmosphere match, and RGB curves are
/// absent by construction.
#[allow(clippy::too_many_arguments)]
pub(super) fn fit_atmosphere_from_parts(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    base: &EditRecipe,
    same_frame: bool,
    readings: DivergencePair,
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

    let ref_source = readable.map_or(evidence.source_weights.as_slice(), |p| p.source.as_slice());
    let ref_target = readable.map_or(evidence.target_weights.as_slice(), |p| p.target.as_slice());
    let linear_luma_cdf = |px: &[[f32; 3]], weights: &[f32]| {
        weighted_cdf(px, weights, |p| {
            0.299 * render::srgb_to_linear(p[0])
                + 0.587 * render::srgb_to_linear(p[1])
                + 0.114 * render::srgb_to_linear(p[2])
        })
    };
    // The exposure reads the shared-content POPULATION, like the white
    // balance, but as the ratio of the two sides' MARGINAL weighted-luma
    // medians rather than as a location statistic on one cloud of per-pixel
    // changes. That asymmetry is deliberate as of 2026-09-02, and it is
    // deliberate because it was tried the other way and measured.
    //
    // The paired form — a weighted median of the per-pixel log2 luminance
    // ratio over exactly the population and pairing
    // `atmosphere_wb_pairing` hands the white balance — buys NOTHING on the
    // real pairs and costs a synthetic one its whole fit. Measured on this
    // tree: the generated-cloud calibration pair reads 0.188686 -> 0.099869
    // either way, `p37` reads 0.247107 -> 0.161220 either way, and no other
    // corpus pair reaches this code at all (Full-mode exposure comes from the
    // tone-slider solve). Against that, `hazy_canyon_source` ->
    // `vivid_warm_target` moves -0.28 EV -> -0.22 EV, and
    // `flat_sky_to_cloud_deck` stops fitting altogether: its finished recipe
    // regresses and the terminal do-no-harm check returns the base, reading
    // 0.1619 -> 0.1619 where the marginal solve delivers a fit that ends
    // closer (`a_per_band_move_the_frame_ruler_refuses_shrinks_to_zero_and_says_so`
    // asserts exactly that, and it is the test the paired form failed).
    //
    // The reason the two statistics differ here and agree on the white
    // balance is that they are not answering the same question. The WB's
    // cloud is a cloud of RATIOS between corresponding pixels, and its centre
    // is the cast. Luminance is what a content-divergent pair changes MOST —
    // a cloud deck is not the sky it replaced — so the centre of its
    // per-pixel ratio cloud is a statement about how much the CONTENT moved,
    // while the ratio of the two medians is a statement about the two frames'
    // levels, which is what an exposure control sets. On a pair whose content
    // genuinely changed, the paired statistic answers the wrong question
    // confidently. So the marginal median stays, on the shared-content
    // population, and this is the last word on it rather than a postponement.
    let (sl, tl) = (linear_luma_cdf(sp, ref_source), linear_luma_cdf(tp, ref_target));
    let exposure = (quantile(&tl, 0.5).max(1e-5) / quantile(&sl, 0.5).max(1e-5))
        .log2()
        .clamp(-budget.ev, budget.ev);
    recipe.exposure_ev = round2(exposure);

    // The white balance is solved on ONE population of per-pixel colour
    // changes — see `atmosphere_wb_pairing` for why the field that chose the
    // population must also choose the pairing, and
    // `atmosphere_wb_from_populations` for why the statistic is a per-pixel
    // median rather than three independent marginal ones.
    let anchor = base.as_shot_k.unwrap_or(5500.0);
    let (pair_tp, pair_w) = atmosphere_wb_pairing(tp, evidence, correspondence, readable);
    let (wb_k, wb_tint, _wanted) =
        atmosphere_wb_from_populations(sp, pair_tp, &pair_w, anchor);
    let mut facts = solve_white_balance(
        s_img,
        tp,
        &mut recipe,
        base,
        evidence,
        (wb_k, wb_tint),
        anchor,
        budget,
        strength,
    );
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
        withhold_atmosphere_tone(&mut recipe, base, &mut facts);
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
    // Atmosphere passes no cells on purpose: its whole doctrine is that the
    // structure was replaced, so a cell mean over one rectangle is not a
    // statement about the same content on both sides. Its per-band gate is
    // unchanged.
    let mut hsl_facts = fit_hsl_stage(s_img, sp, tp, evidence, None, None, budget, &mut recipe);
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
    let mut harm = terminal_harm(err_before, err_after, joint_base, joint_after);
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
            divergence: readings.fine,
            divergence_coarse: readings.coarse,
            pairing: readings.scale(),
            evidence,
            structural_evidence: Some(structural),
            defer_disclosure,
        },
        SolveFacts {
            budget: Some(budget),
            strength: Some(strength.get()),
            veto_luma: (budget.vetoes == VetoPolicy::Disclose).then(|| moved_unsupported_luma_range_names(sp, &after_px, veto_evidence)).flatten(),
            // Read of the render that SHIPS (`after_px`, after the detail, HSL,
            // shrink and terminal stages), like `veto_luma` beside it: the
            // `moved_hue` reading above was taken before those stages and
            // could name a range the terminal reset had already given back.
            veto_hue: (budget.vetoes == VetoPolicy::Disclose && structural.global_cast.is_none())
                .then(|| moved_unsupported_hue_range_names(sp, &after_px, veto_evidence))
                .flatten(),
            wb_clamped: facts.clamped,
            wb_search_bound: facts.search_bound,
            wb_rotation_coverage: Some(facts.rotation_coverage),
            wb_rotation_disclosure: facts.rotation_disclosure,
            cast_admitted_by_strength: None,
            cast_admitted: None,
            cast_projected: None,
            wb_cells: None,
            wb_foreign_hue_withheld: facts.foreign_hue_withheld,
            wb_rotation_withheld: facts.rotation_withheld,
            sat_pegged: sat_pegged.then_some(FitMode::Atmosphere),
            cast: CastOutcome::default(),
            evidence_refused: evidence_has_one_sided(evidence),
            sat_fitted: sat_reduced.then_some(sat_fitted),
            regressed: fit_regressed.then_some(joint_regressed),
            tone_harm: None,
            curve_harm: None,
            detail,
            detail_withheld: !detail_supported,
            robust: None,
            paired: false,
            vouched_bands: None,
            cast_cells: None,
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

pub(super) fn cast_admitted_by_strength(measured_ratio: f32, budget: f32, strength: f32) -> bool {
    strength > crate::recipe::GradeStrength::DEFAULT
        && measured_ratio > CAST_ACCEPT_RATIO
        && measured_ratio <= budget
}

/// Constrained monotone projection with fixed x coordinates and fixed endpoint
/// values. Slopes are redistributed across neighboring segments; no point is
/// deleted. Inputs already inside the budget return byte-identically.
pub(super) fn project_curve_slopes(points: &[CurvePoint], min_slope: f32, max_slope: f32) -> Vec<CurvePoint> {
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
