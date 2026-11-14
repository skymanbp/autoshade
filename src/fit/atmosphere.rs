//! White balance and atmosphere: the white-balance solve, the atmosphere fit from parts, and curve-slope projection.

use super::*;

/// What the white-balance stage decided, in the shape the report needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct WbFacts {
    /// `(ratio_before, ratio_after)` when the strength budget's gains scaled
    /// the free demand (`budgeted_wb`).
    pub(super) clamped: Option<(f32, f32)>,
    /// The free search landed on the finite Kelvin domain's edge.
    pub(super) search_bound: Option<f32>,
}

/// THE white-balance stage, for BOTH modes.
///
/// The caller brings the free `(k, tint)` its own population estimated and the
/// anchor it is measured from; this function decides how much of that demand
/// the STRENGTH BUDGET spends — the shipped default persists an in-budget
/// free WB verbatim and leaves an out-of-budget one at as-shot, while above
/// default [`budgeted_wb`] is the sole producer: the free demand scaled into
/// the gains budget along the gain-space path. The budget is the user's dial,
/// not evidence; what the EVIDENCE allows is the one validity law's reading
/// of the white balance between its own frames (`fit::validity`,
/// [`super::joint::Joint::refusal`]), and the repair scales a refused seed
/// toward the base. Until v1.18.0 this stage bisected the scalar against two
/// content gates of its own (foreign hue, re-hued share) — instances of the
/// law, retired with it (rel1180 design-control-validity-1180 §2).
///
/// EXTRACTED, not copied: this block used to live inside
/// [`fit_atmosphere_from_parts`], and that was the whole reason Full mode had
/// no white balance at all — the one bounded WB solve the crate owns was
/// reachable only through the divergent-pair branch, so the commonest pair
/// there is (the same frame, regraded) could never be told that its light had
/// changed colour.
pub(super) fn solve_white_balance(
    recipe: &mut EditRecipe,
    (wb_k, wb_tint): (f32, f32),
    law: render::WbLaw,
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
    // without being it. "The anchor" is the law's As Shot.
    let as_shot = law.as_shot();
    if wb_k == (as_shot.0 / 50.0).round() * 50.0 && wb_tint == round1(as_shot.1) {
        return WbFacts::default();
    }
    let search_bound = (strength.get() > crate::recipe::GradeStrength::DEFAULT
        && (wb_k <= WB_SEARCH_K.0 || wb_k >= WB_SEARCH_K.1))
        .then_some(wb_k);
    if strength.get() <= crate::recipe::GradeStrength::DEFAULT {
        // The shipped default is the pre-F1 path byte-for-byte: an in-budget
        // free WB is persisted, while an out-of-budget demand stays as-shot.
        if wb_gains_fit_budget(law.gains((wb_k, wb_tint)), budget) {
            recipe.temperature_k = Some(wb_k);
            recipe.tint = wb_tint;
        }
        return WbFacts { clamped: None, search_bound };
    }
    // Above the shipped default, budgeted_wb is the sole producer of a
    // persisted WB: the free demand, or the largest fraction of it along the
    // gain-space path whose gains the budget admits.
    let (k, tint, clamped, ratio_before, ratio_after, _lambda) = budgeted_wb(law, wb_k, wb_tint, budget);
    recipe.temperature_k = Some(k);
    recipe.tint = tint;
    WbFacts { clamped: clamped.then_some((ratio_before, ratio_after)), search_bound }
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
    // R30 R2: the reference population the two ROBUST GLOBAL CONTROLS are
    // solved FROM. Since the one validity law (2026-11-07, design §1 Silent)
    // it is also the population the solve is JUDGED on: the target pixels no
    // source pixel answers for are a scene, not an edit, and content with no
    // standing to refuse a move has none to score one — J*, the base
    // candidate and the law's cells read the shared population against the
    // corresponded target (`judged`, below). The estimators of the tone
    // curve, the saturation chase and the mixer, the confidence cap and mode
    // selection keep the frame model.
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
    // One report has one ruler: the caller's structural `err_before` belongs
    // to the mode-selection model, while every Atmosphere measurement below
    // is read on ONE population model — the structure-blind frame model,
    // which keeps the structural pairing (2026-11-07), or, with a readable
    // field, the same construction on the field's own pairing
    // (`shared_content_pairing`: the remapped target, the structural
    // instrument read on that pair, scoped to the field's confident
    // matches). The same model judges J*, the base candidate, the law's
    // cells, the white balance's pair weight and the report's
    // identifiability (RULING-1180-int-r2: what the restriction excludes, no
    // guard reads). With no field the slices are the frame's own, borrowed;
    // under an identity field the model is the frame's field for field, so
    // a restriction that drops nothing moves nothing.
    let readable = shared.as_ref().filter(|p| p.readable());
    let (pair_tp, judged_scoped) = shared_content_pairing(sp, tp, structural, correspondence, readable);
    let judged: &EvidenceModel = judged_scoped.as_ref().unwrap_or(evidence);
    let judged_tp: &[[f32; 3]] = pair_tp;
    let pair_w = atmosphere_wb_pairing(judged);
    let err_before = look_err_with_evidence(sp, judged_tp, judged);
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
    // `shared_content_pairing` hands the white balance — buys NOTHING on the
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
    // changes — see `shared_content_pairing` for why the field that chose the
    // population must also choose the pairing, and
    // `atmosphere_wb_from_populations` for why the statistic is a per-pixel
    // median rather than three independent marginal ones.
    let law = render::WbLaw::of(base);
    let (wb_k, wb_tint, _wanted) =
        atmosphere_wb_from_populations(sp, pair_tp, &pair_w, law);
    let facts = solve_white_balance(&mut recipe, (wb_k, wb_tint), law, budget, strength);
    let provisional = pixels_of(&render::develop_preview(s_img, &recipe));
    // The withheld luma ranges keep their identity in the OVERALL tone (the
    // Full stage's `tone_map` rule): the curve undoes the exposure there.
    // The ranges are the STRUCTURAL ruler's (the blind model weights every
    // range; gates/j121: read from it, no range was withheld and no pin
    // was made).
    let pins = withheld_identity_pins(sp, &provisional, structural);
    recipe.tone_curve = atmosphere_tone_curve_weighted(
        &provisional,
        tp,
        &evidence.source_weights,
        &evidence.target_weights,
        &pins,
        budget.slope.0,
        budget.slope.1,
    );
    // The tone's OTHER carrier (2026-11-10). Where brightness ranges are
    // withheld the stage has two ways to state one tone: the exposure with a
    // curve that undoes it over the withheld ranges (above), or the curve
    // alone — no exposure, the withheld ranges pinned where they stand, the
    // rest matched by the same quantile knots. They are not the same
    // proposal once the exposure meets a limit the curve cannot undo: a
    // withheld range near white is pushed into clipping, many source levels
    // land on one, and no knot inside the slope budget maps them back; the
    // bounded solve can only scale the shape it is given, so the law refused
    // that tone at every fraction and nothing shipped (the withheld-top toy,
    // gates/j120–j121: 0.0467 either way). Which carrier serves a pair is
    // read, not assumed, by the alone check's law (`solve::alone_check`):
    // the curve-carried start is repaired into the law, and where it
    // already reads a lower F or a lower J* than the exposure's solve
    // FINISHED on, it is solved the same way and the lower finished F ships
    // (below). Offered wherever the stage's exposure is off the base. Until
    // J142 (2026-11-11) it was offered only where a range was withheld, on
    // the reading that the two carriers differ only there; they also differ
    // at the frame's two ends, which the exposure moves and the curve keeps.
    // On the different-photo pair, whose brightest ranges ask nothing, the
    // exposure's solve finished at J* 0.15871 and the curve's at 0.12427
    // (the diagnostic build `probe5`, rel1180 j142/p5-both, 2026-11-11).
    let curve_carried = (recipe.exposure_ev != base.exposure_ev).then(|| {
        let mut unexposed = recipe.clone();
        unexposed.exposure_ev = base.exposure_ev;
        unexposed.tone_curve = base.tone_curve.clone();
        let at_base = pixels_of(&render::develop_preview(s_img, &unexposed));
        let pins = withheld_identity_pins(sp, &at_base, structural);
        atmosphere_tone_curve_weighted(
            &at_base,
            tp,
            &evidence.source_weights,
            &evidence.target_weights,
            &pins,
            budget.slope.0,
            budget.slope.1,
        )
    });

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
    // Atmosphere mode never emits channel curves.
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
    // — and a band's mean chroma is a distribution. Atmosphere passes no
    // cells on purpose: its whole doctrine is that the structure was
    // replaced, so a cell mean over one rectangle is not a statement about
    // the same content on both sides.
    let hsl_facts = fit_hsl_stage(s_img, sp, tp, evidence, None, None, budget, &mut recipe);

    // The joint objective on this mode's ruler (RULING-1180-joint-r1, applied
    // to Atmosphere 2026-11-06): ONE ruler, J* on the one pairing of the
    // population with standing — the frame's (the blind model keeps it,
    // 2026-11-07), or the shared content's where a field is readable
    // (`judged`; RULING-1180-int-r2 and the law's Silent cells). The
    // per-control scoped earning of DEF-2 was a second ruler beside the
    // frame's and went with the per-stage eliminations; this is the one
    // ruler, read on the one population. The coordinates this mode does not
    // solve stay pinned at the base.
    let ctx = super::solve::SolveCtx::atmosphere(
        s_img,
        sp,
        judged_tp,
        judged,
        base,
        strength,
        budget,
        law,
        pair_w,
    );
    let started = std::time::Instant::now();
    // The starts beside the stage's own: each is the same recipe with its
    // tone stated another way.
    let mut alternatives: Vec<(&'static str, EditRecipe)> = Vec::new();
    if let Some(curve) = curve_carried {
        let mut alt = recipe.clone();
        alt.exposure_ev = base.exposure_ev;
        alt.tone_curve = curve;
        alternatives.push(("the curve carries the tone", alt));
    }
    // The tone's THIRD carrier (J142, 2026-11-11): the law's own asks, as the
    // curve (`asks_curve::asks_tone_curve`, where the construction and its
    // readings are). The asks are the tone class's, read as G will read them
    // — on the frame the other classes leave (`joint::without_class`) — so
    // the shape proposes, range by range, what the law will admit, as far as
    // the slope budget lets one curve say it. The exposure and three quantile
    // knots cannot say "the ground down 2.6 ln, the sky's top where it is";
    // this can, and which carrier serves a pair is read on the finished F
    // like the other one's (below).
    {
        let toneless = super::joint::without_class(base, &recipe, super::joint::Refusal::Tone);
        let before = pixels_of(&render::develop_preview(s_img, &toneless));
        // A range whose ask is a population's is HELD while the fit withholds
        // (J155): the curve raises no contrast G's reading would refuse
        // (`Cells::stretched`, J154), so the ranges that pair keep what the
        // law admits of their asks.
        let withhold = budget.vetoes == VetoPolicy::Withhold;
        let cell_asks: Vec<(f32, f32, f32, bool)> = ctx
            .validity
            .luma_asks(&ctx.validity.asks(super::validity::Class::Tone, &before))
            .into_iter()
            .map(|(level, ask, mass, paired)| (level, ask, mass, withhold && !paired))
            .collect();
        let curve = super::asks_curve::asks_tone_curve(&cell_asks, budget.slope.0, budget.slope.1);
        if crate::config::live_env_os("AUTOSHADE_FIT_TRACE").is_some() {
            eprintln!(
                "fit: the asks' tone, cells (level, ask, mass, held) {cell_asks:?} -> {:?}",
                curve.iter().map(|k| (k.input, k.output)).collect::<Vec<_>>()
            );
        }
        if !curve.is_empty() {
            let mut alt = toneless;
            alt.tone_curve = curve;
            alternatives.push(("the asks carry the tone", alt));
        }
    }
    // A BOUNDED point: this mode's evidence is the population's
    // distribution, which cannot say which pixel goes where, so each curve
    // keeps the stages' shape and carries one coordinate, its strength
    // (user ruling 2026-11-09, `joint::Coord::CurveStrength`).
    let mut point = super::joint::Point::bounded(recipe);
    let mut joint = super::joint::Joint::with_pinned(&ctx, super::joint::Coord::atmosphere_pinned(&point));
    joint.trace_start(&point);
    let start = joint.repair(&mut point);
    let (mut point, mut best, mut unearned) = solve_bounded(&mut joint, point, start, base);
    // The other carriers (see `curve_carried` and the asks' tone above), one
    // after the other against the best so far, EACH SOLVED from its own
    // repaired start. A finished F is the only F two starts can be compared
    // on — a repaired start still carries every control the elimination has
    // not priced, and until 2026-11-10 the two were compared there: on the
    // night RAW pair the exposure's start had fallen to the base (F 0.17560
    // at price 0) and beat the curve's J* 0.16520 under nine controls
    // (F 0.18230), so the global solve shipped nothing (legs/V1).
    //
    // Until 2026-11-13 (J152) a carrier was solved only where its repaired
    // start already read below the best FINISHED solve, in F or in J* — the
    // same comparison across the two kinds of reading, kept as a shortcut
    // past the solve, and processing time is never a gate (user rule
    // 2026-11-05). What a repair leaves is where the law's edge stopped the
    // re-estimate, not what the descent reaches from there. On the
    // different-photograph pair the asks' start read J* 0.11685 after
    // its repair beside a best of 0.12456 and was solved to 0.05855
    // (legs/V14); once the fit's plate kept the light its base cuts (J151)
    // the same repair stopped at 0.15385 beside a best of 0.11687,
    // the carrier was never solved, and the pair shipped at 0.11687 —
    // its frame 34.23 from the target where v1.17.1 reads 32.75
    // and the round before 31.70 (legs/V15). Solved, it ends at
    // 0.05834 and the frame reads 31.74.
    for (carrier, alt) in alternatives {
        let mut alt_point = super::joint::Point::bounded(alt);
        let mut alt_joint =
            super::joint::Joint::with_pinned(&ctx, super::joint::Coord::atmosphere_pinned(&alt_point));
        let alt_start = alt_joint.repair(&mut alt_point);
        alt_joint.trace(&format!("repair ({carrier})"), &alt_point, &alt_start);
        let (alt_point, alt_best, alt_unearned) = solve_bounded(&mut alt_joint, alt_point, alt_start, base);
        if alt_best.f() < best.f() {
            (point, best, unearned) = (alt_point, alt_best, alt_unearned);
            alt_joint.absorb(&joint);
            joint = alt_joint;
        } else {
            joint.absorb(&alt_joint);
        }
    }
    let mut recipe = point.recipe;
    let f = best.f();
    let mut err_after = best.j;
    let mut after_px = best.px;
    // Processing time is a reading, never a gate (user rule 2026-11-05).
    if crate::config::live_env_os("AUTOSHADE_FIT_TRACE").is_some() {
        eprintln!(
            "fit: joint solve (atmosphere) {}, {:.1} s, J* {err_before:.5} -> {err_after:.5}, F {f:.5}",
            joint.spent_line(),
            started.elapsed().as_secs_f32()
        );
    }
    // The base candidate (RULING-1180-joint-r1 (5)): dof 0, F = err_before.
    // It ships when the solve's F is no lower, unless the detail exemption
    // holds.
    let mut fit_regressed = false;
    // As in the Full solve: the REGRESSED sentence only where the solve's
    // own render ended farther than the base by the ruler's resolution.
    let solve_regressed = crate::fit::ships(err_before, err_after);
    if f >= err_before
        && !(detail_supported
            && only_detail_and_quantized_companions(&recipe, base)
            && detail_regression_is_bounded(sp, &after_px, tp, structural, detail, err_before, err_after))
    {
        recipe = base.clone();
        after_px = sp.to_vec();
        err_after = err_before;
        fit_regressed = true;
    }
    let joint_after = crate::fit_zoned::joint_reading_with_evidence(
        &after_px,
        judged_tp,
        &judged.source_weights,
        &judged.target_weights,
    );
    // The white-balance clamp describes a white balance only while the
    // shipped recipe carries one.
    let wb_shipped = (recipe.temperature_k, recipe.tint) != (base.temperature_k, base.tint);
    // The cells no evidence spoke for that the recipe moved, on this solve's
    // own cells (the shared population where a field is readable, design §1
    // Silent), and the ranges a population asks whose contrast the tone's
    // shape raised (J154). Read before the report takes the recipe.
    let veto_cells = (budget.vetoes == VetoPolicy::Disclose)
        .then(|| super::validity::disclosed_cells(&ctx.validity, s_img, base, &recipe, sp, &after_px))
        .flatten();
    compose_report(
        recipe,
        Measured {
            err_before,
            err_after,
            joint_after,
            after_px: &after_px,
            tp: judged_tp,
            same_frame,
            mode: FitMode::Atmosphere,
            divergence: readings.fine,
            divergence_coarse: readings.coarse,
            pairing: readings.scale(),
            evidence: judged,
            structural_evidence: Some(structural),
            defer_disclosure,
        },
        SolveFacts {
            budget: Some(budget),
            strength: Some(strength.get()),
            veto_cells,
            wb_clamped: wb_shipped.then_some(facts.clamped).flatten(),
            wb_search_bound: facts.search_bound,
            scaled: joint.scaled(),
            sat_pegged: sat_pegged.then_some(FitMode::Atmosphere),
            evidence_refused: evidence_has_one_sided(judged),
            regressed: (fit_regressed && solve_regressed).then_some(false),
            unearned: control_names(&unearned),
            detail,
            detail_withheld: !detail_supported,
            robust: None,
            paired: false,
            hsl: hsl_facts,
            // Atmosphere mode solves no colour grade.
            grade_held: None,
            atmosphere_reference,
        },
    )
}

/// One bounded solve from its repaired start: descended, polished, the
/// controls that do not earn their price eliminated, polished again at the
/// fine steps. Returns the point, its reading and the controls the solve
/// dropped or walked back to the base.
fn solve_bounded(
    joint: &mut super::joint::Joint<'_, '_>,
    mut point: super::joint::Point,
    start: super::joint::Eval,
    base: &EditRecipe,
) -> (super::joint::Point, super::joint::Eval, Vec<Control>) {
    joint.mark("repair");
    joint.trace("repair", &point, &start);
    // The stages' proposal as it entered the descent (see `stage_joint`).
    let proposed = point.recipe.clone();
    let best = joint.descend(&mut point, start, &super::joint::STEP_MULTIPLIERS);
    joint.mark("descend");
    joint.trace("descend", &point, &best);
    let mut best = joint.polish(&mut point, best, &super::joint::STEP_MULTIPLIERS);
    joint.mark("polish");
    joint.trace("polish", &point, &best);
    let mut unearned = joint.eliminate(&mut point, &mut best);
    joint.mark("eliminate");
    joint.trace("eliminate", &point, &best);
    let best = joint.polish(&mut point, best, &super::joint::STEP_MULTIPLIERS[2..]);
    joint.mark("polish");
    joint.trace("final polish", &point, &best);
    super::joint::walked_back(&proposed, &point.recipe, base, &mut unearned);
    (point, best, unearned)
}

/// The identity a withheld luma range keeps in the OVERALL tone: the Full
/// stage pins its `tone_map` to the identity there (a populated range with
/// no evidence weight is refused, not extrapolated); this mode's curve sits
/// behind the exposure, so its knot for such a range maps the range's
/// PROVISIONAL luma (after the exposure) back to its SOURCE luma — the
/// medians of the range's own pixels, one knot per withheld range with
/// [`SUPPORT_MIN_PIXELS`] of them. Under the free knots the solver rebuilt
/// this itself (the tone_stage toy lifted one knot and held the top,
/// gates/j118); the bounded curve of 2026-11-09 can only scale the shape the
/// stage gives it, so the shape carries the identification (gates/j120: the
/// exposure alone brightened the withheld top against its asks and the
/// whole tone was refused).
///
/// A frame nothing pairs on has no withheld range (J142, 2026-11-11). A
/// range is withheld FROM a pairing — its pairs disagree, so the paired
/// reading of it is not trusted and the tone leaves it where it stands. On
/// two photographs ([`EvidenceModel::pairs_a_population`], the reading the
/// colour field is refused on) the structural model gives no range any
/// weight, every populated range was pinned, and the stage's curve undid
/// its own exposure everywhere: the proposal was the identity on the one
/// kind of pair this mode's doctrine fits by distribution alone, while the
/// law read those same ranges on the population's quantile asks (user
/// ruling 2026-11-07) — on the different-photo pair, −2.0 to −2.6 ln for
/// the ground (rel1180 j142/p5-both).
pub(super) fn withheld_identity_pins(sp: &[[f32; 3]], provisional: &[[f32; 3]], evidence: &EvidenceModel) -> Vec<CurvePoint> {
    let n = sp.len().min(provisional.len());
    let mut pins = Vec::new();
    if !evidence.pairs_a_population() {
        return pins;
    }
    for (bin, range) in evidence.luma.iter().enumerate() {
        if range.weight > 0.0 || range.source_share < EVIDENCE_MIN_SHARE {
            continue;
        }
        let mut ins: Vec<(f32, f32)> = Vec::new();
        let mut outs: Vec<(f32, f32)> = Vec::new();
        for i in 0..n {
            let l = luma601(&sp[i]);
            if evidence_luma_bin(l) != bin {
                continue;
            }
            ins.push((luma601(&provisional[i]), 1.0));
            outs.push((l, 1.0));
        }
        if (ins.len() as f32) < SUPPORT_MIN_PIXELS {
            continue;
        }
        let input = (super::evidence::weighted_median(&mut ins) * 255.0).round().clamp(1.0, 253.0) as u8;
        let output = (super::evidence::weighted_median(&mut outs) * 255.0).round().clamp(1.0, 254.0) as u8;
        pins.push(CurvePoint { input, output });
    }
    pins
}

/// The stage's curve: the 5 / 50 / 95 % quantiles of the two weighted luma
/// distributions as knots, the withheld ranges' identity `pins` among them
/// (a pin replaces a quantile knot at its input; the knots around a pin
/// are held on its side of it — the identification wins), the endpoints
/// fixed, the slopes projected into the budget.
fn atmosphere_tone_curve_weighted(
    cur: &[[f32; 3]],
    tgt: &[[f32; 3]],
    cur_weights: &[f32],
    tgt_weights: &[f32],
    pins: &[CurvePoint],
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
    for pin in pins {
        if pin.input == 0 || pin.input >= 254 || points.iter().any(|p| p.input == pin.input && pins.contains(p)) {
            continue;
        }
        points.retain(|p| p.input != pin.input);
        points.push(*pin);
    }
    points.push(CurvePoint { input: 255, output: 255 });
    points.sort_by_key(|p| p.input);
    let is_pin = |p: &CurvePoint| p.input == 0 || p.input == 255 || pins.contains(p);
    // Every free knot stays between the pins on either side of it, then the
    // sequence is made monotone from the top down (a knot never rises above
    // its successor).
    for i in 0..points.len() {
        if is_pin(&points[i]) {
            continue;
        }
        let lo = points[..i].iter().rev().find(|p| is_pin(p)).map_or(0, |p| p.output);
        let hi = points[i + 1..].iter().find(|p| is_pin(p)).map_or(255, |p| p.output);
        points[i].output = points[i].output.clamp(lo, hi.max(lo));
    }
    for i in (0..points.len() - 1).rev() {
        if points[i].output > points[i + 1].output {
            points[i].output = points[i + 1].output;
        }
    }
    project_curve_slopes(&points, min_slope, max_slope)
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
