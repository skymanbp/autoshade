//! The solver: the fit_recipe entry points and the promoted solve with its disclosure.

use super::*;

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
    let evidence =
        evidence_model_paired(&sp, &tp, presence_of(&t_img).as_deref(), s_img.width(), s_img.height());
    let err_before = look_err_with_evidence(&sp, &tp, &evidence);
    // The paired-CELL instrument, built once off the frozen evidence model and
    // the target raster: every admission below asks the SAME cells, so two
    // stages cannot vouch against two different targets. One pass over the
    // raster — nothing here runs a second structural instrument.
    let cells = crate::fit_cells::PairedCells::build(&tp, s_img.width(), s_img.height(), &evidence);
    // ONE reading per solve, at both scales, off one raster build. The mode
    // line and every evidence gate read the FINE number; the coarse one is
    // carried for the disclosure (see [`COARSE_SIGMA_DIVISOR`] for why it is
    // not a more forgiving reading and therefore gates nothing).
    let readings = divergence_pair_for(src, target, base);
    let divergence = readings.fine;
    let pairing = readings.scale();
    // Atmosphere is the claim that the structure was REPLACED, so an
    // abstention cannot promote into it: `is_some_and` refuses to fire on a
    // reading nobody took, where the old `d >= …` on a matched-by-default
    // value would have read 0.0 and refused for the wrong reason.
    let mode = if divergence.is_some_and(|r| r.d >= DIVERGENCE_GLOBAL) || divergent_zone_promotes {
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
            confidence_cap: 1.0,
            notes,
            mode,
            divergence,
            divergence_coarse: readings.coarse,
            pairing: readings.scale(),
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
            readings,
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

    // Already matched: the base reads within the objective's own rounding of
    // the target, so there is nothing a rounded recipe could correct
    // (FIT_QUANT, v1.18.0; a bare 0.001 with no record before) — on the
    // frame AND on every population a priced control answers for
    // (`priced_populations_matched`, 2026-11-10: a band that reaches 4 % of
    // the frame reads its whole ask divided by 25 on the frame, and a +20
    // band hue the fit itself had rendered returned here at 0.0008,
    // gates/j125).
    if err_before <= FIT_QUANT && priced_populations_matched(&sp, &tp, &evidence, base) {
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
                divergence_coarse: readings.coarse,
                pairing: readings.scale(),
                evidence: &evidence,
                structural_evidence: None,
                defer_disclosure,
            },
            SolveFacts {
                budget: Some(FitBudget::for_strength(options.strength)),
                strength: Some(options.strength.get()),
                veto_cells: None,
                wb_clamped: None,
                wb_search_bound: None,
                scaled: Vec::new(),
                sat_pegged: None,
                evidence_refused: false,
                regressed: None,
                unearned: None,
                detail: (0.0, 0.0),
                detail_withheld: false,
                robust: None,
                paired: false,
                hsl: HslStageFacts::default(),
                grade_held: None,
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
                divergence_coarse: readings.coarse,
                pairing: readings.scale(),
                evidence: &evidence,
                structural_evidence: None,
                defer_disclosure,
            },
            SolveFacts {
                budget: Some(FitBudget::for_strength(options.strength)),
                strength: Some(options.strength.get()),
                veto_cells: None,
                wb_clamped: None,
                wb_search_bound: None,
                scaled: Vec::new(),
                sat_pegged: None,
                evidence_refused: false,
                regressed: None,
                unearned: None,
                detail: (0.0, 0.0),
                detail_withheld: true,
                robust: None,
                paired: false,
                hsl: HslStageFacts::default(),
                grade_held: None,
                atmosphere_reference: AtmosphereReference::WholeFrame,
            },
        );
    }

    // The solve proper (RULING-1180-joint-r1): a read-only context built once
    // off the pair; the stages INITIALISE (their estimates only), and the
    // joint objective decides what ships (`stage_joint`).
    let ctx = SolveCtx::new(&s_img, &sp, &tp, &evidence, &cells, base, &options, pairing);
    let started = std::time::Instant::now();
    let state = alone_check(&ctx, run_from(&ctx, Stage::Tone, SolveState::at(base)));
    let SolveState { wb_facts, sat_pegged, grade_held, detail, detail_supported, finished, .. } = state;
    let finished = finished.expect("run_from runs the joint stage");
    let (budget, paired) = (ctx.budget, ctx.paired);
    let robust_facts = ctx.robust_facts.clone();
    let Finished { mut recipe, hsl_facts, unearned, end_px, err_after, f, scaled, spent } = finished;
    // Processing time is a reading, never a gate (user rule 2026-11-05):
    // printed with the render count when the trace is asked for.
    if crate::config::live_env_os("AUTOSHADE_FIT_TRACE").is_some() {
        eprintln!(
            "fit: joint solve {spent}, {:.1} s, J* {err_before:.5} -> {err_after:.5}, F {f:.5}",
            started.elapsed().as_secs_f32()
        );
    }
    // The white-balance facts describe a white balance only while the
    // shipped recipe carries one; the elimination may have dropped it.
    let wb_admitted = (recipe.temperature_k, recipe.tint) != (base.temperature_k, base.tint);
    // TERMINAL do-no-harm, the same law (joint-r1 (5)): the base is a
    // candidate with no degrees of freedom, F = err_before, and ships when the
    // solve is no better on F. The detail controls are outside J* and F, so a
    // recipe that is only detail and its quantised companions is read on the
    // detail ruler instead (`detail_regression_is_bounded`).
    let mut fit_regressed = f >= err_before;
    // The REGRESSED sentence is for a render that ended FARTHER than the
    // base by the objective's own resolution (`ships`: FIT_QUANT, the rule
    // every gain is read by); a base that ships because no control earned
    // its quantum — the render no closer, or farther by less than the ruler
    // resolves — is the UNEARNED sentence's case.
    let solve_regressed = crate::fit::ships(err_before, err_after);
    if fit_regressed
        && detail_supported
        && only_detail_and_quantized_companions(&recipe, base)
        && detail_regression_is_bounded(&sp, &end_px, &tp, &evidence, detail, err_before, err_after)
    {
        fit_regressed = false;
    }
    let joint_base = crate::fit_zoned::joint_reading_with_evidence(
        &sp,
        &tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    let (after_px, joint_after) = if fit_regressed {
        // Reset to the BASE, not to a bare default (R16): "do no harm" means
        // degrading to the calibration look the canvas would show with no
        // fit at all. By definition that render IS the err_before
        // measurement, so no re-render is needed.
        recipe = base.clone();
        (sp.clone(), joint_base)
    } else {
        let joint_after = crate::fit_zoned::joint_reading_with_evidence(
            &end_px,
            &tp,
            &evidence.source_weights,
            &evidence.target_weights,
        );
        (end_px, joint_after)
    };

    // --- report ---------------------------------------------------------------
    // The cells no evidence spoke for that the recipe moved (the law's blind
    // cells, `fit::validity`) and the ranges a population asks whose
    // contrast the tone's shape raised (J154): refused under Withhold,
    // disclosed here under Disclose. Read before the report takes the recipe.
    let veto_cells = (budget.vetoes == VetoPolicy::Disclose)
        .then(|| super::validity::disclosed_cells(&ctx.validity, ctx.s_img, base, &recipe, &sp, &after_px))
        .flatten();
    compose_report(
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
            divergence_coarse: readings.coarse,
            pairing: readings.scale(),
            evidence: &evidence,
            structural_evidence: None,
            defer_disclosure,
        },
        SolveFacts {
            budget: Some(budget),
            strength: Some(options.strength.get()),
            veto_cells,
            // The same budget disclosures Atmosphere makes, because it is the
            // same stage: a Full-mode WB that was scaled into the gains budget
            // or that landed on the Kelvin domain's edge says so in the same
            // words. Quoted only where a WB actually shipped.
            wb_clamped: wb_admitted.then_some(wb_facts.clamped).flatten(),
            wb_search_bound: wb_admitted.then_some(wb_facts.search_bound).flatten(),
            scaled,
            sat_pegged: sat_pegged.then_some(FitMode::Full),
            evidence_refused: evidence_has_one_sided(&evidence),
            regressed: (fit_regressed && solve_regressed).then_some(false),
            unearned: control_names(&unearned),
            detail,
            detail_withheld: !detail_supported,
            robust: robust_facts,
            paired,
            hsl: hsl_facts,
            grade_held: grade_held.filter(|_| !fit_regressed),
            atmosphere_reference: AtmosphereReference::WholeFrame,
        },
    )
}

/// The solve's stages, in the order [`run_from`] runs them: the estimators,
/// then the joint objective (RULING-1180-joint-r1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    /// 1: the six tone sliders.
    Tone,
    /// 2: the residual master curve.
    MasterCurve,
    /// 2b: the white balance.
    WhiteBalance,
    /// 3: global saturation.
    Saturation,
    /// 3b: the colour-grade wheels.
    Grade,
    /// The detail controls.
    Detail,
    /// 4a: the per-band mixer, as fitted.
    Mixer,
    /// 4: the cast curves, as fitted.
    Cast,
    /// The joint objective: repair, polish, eliminate.
    Joint,
}

impl Stage {
    const ALL: [Stage; 9] = [
        Stage::Tone,
        Stage::MasterCurve,
        Stage::WhiteBalance,
        Stage::Saturation,
        Stage::Grade,
        Stage::Detail,
        Stage::Mixer,
        Stage::Cast,
        Stage::Joint,
    ];

    fn run(self, ctx: &SolveCtx<'_>, st: &mut SolveState) {
        match self {
            Stage::Tone => stage_tone(ctx, st),
            Stage::MasterCurve => stage_master_curve(ctx, st),
            Stage::WhiteBalance => stage_white_balance(ctx, st),
            Stage::Saturation => stage_saturation(ctx, st),
            Stage::Grade => stage_grade(ctx, st),
            Stage::Detail => stage_detail(ctx, st),
            Stage::Mixer => stage_mixer(ctx, st),
            Stage::Cast => stage_cast(ctx, st),
            Stage::Joint => stage_joint(ctx, st),
        }
    }
}

/// The base matched on every population a priced control answers for: each
/// mixer band and each grade region with standing (a share of at least
/// [`EVIDENCE_MIN_SHARE`]) reads within FIT_QUANT on ITS OWN pixels — the
/// objective scoped to the population's membership, as a local correction is
/// read ([`crate::fit::ships_local`]'s own-population form). The frame-wide
/// reading alone cannot say this: it divides a band's ask by its share.
fn priced_populations_matched(sp: &[[f32; 3]], tp: &[[f32; 3]], evidence: &EvidenceModel, base: &EditRecipe) -> bool {
    let n = sp.len().max(1) as f64;
    joint::Reach::populations(sp, &base.color_grade).iter().all(|members| {
        let share = (members.iter().map(|w| *w as f64).sum::<f64>() / n) as f32;
        share < EVIDENCE_MIN_SHARE
            || look_err_with_evidence(sp, tp, &evidence.scoped(tp, members, members)) <= FIT_QUANT
    })
}

/// The classes each proposed ALONE from the base, in the stages' own order
/// (the detail controls ride every start: they are outside J* and F).
const ALONE_STARTS: [(&str, &[Stage]); 6] = [
    ("tone", &[Stage::Tone, Stage::MasterCurve, Stage::Detail]),
    ("white balance", &[Stage::WhiteBalance, Stage::Detail]),
    ("saturation", &[Stage::Saturation, Stage::Detail]),
    ("grade", &[Stage::Grade, Stage::Detail]),
    ("mixer", &[Stage::Detail, Stage::Mixer]),
    ("cast", &[Stage::Detail, Stage::Cast]),
];

/// The alone check (RULING-1180-joint-r2, addendum 2026-11-10; RULING-1180-sc
/// Q-SC-5 and Q-SC-6 at the class's granularity). The stages run in sequence
/// and each one estimates on the frame the ones before it left, so a target
/// that ONE class explains reaches the joint solve as a start every earlier
/// stage has already spent on — the white balance the fit itself had
/// rendered entered at 17 controls, and the solve kept the wheels and band
/// dials that rationalised them and dropped the temperature (gates/j126; the
/// cast curve, the grade wheel and the band toys read the same, gates/j125).
/// The law (Q-SC-5): a control is judged against its absence with the stages
/// after it re-fitted. Read from the other side it is cheap: each class is
/// proposed ALONE from the base and repaired into G; a start that already
/// reads a lower F, or a lower J*, than the whole solve finished on is a
/// basin the sequence never visited, and the solve is run again from the
/// best such start — every coordinate free, as in the first — and the lower
/// F ships. No start beats the finished solve on a pair the sequence served
/// (the starts cost their estimates and one repair each); class granularity,
/// not control granularity, is what this reads.
fn alone_check(ctx: &SolveCtx<'_>, mut main: SolveState) -> SolveState {
    let (j_main, f_main) = {
        let done = main.finished.as_ref().expect("run_from runs the joint stage");
        (done.err_after, done.f)
    };
    let trace = crate::config::live_env_os("AUTOSHADE_G_TRACE").is_some();
    let mut renders = 0u32;
    let mut triggered: Option<(f32, &'static str, SolveState)> = None;
    for (name, stages) in ALONE_STARTS {
        let mut state = SolveState::at(ctx.base);
        for stage in stages {
            stage.run(ctx, &mut state);
        }
        if joint::dof_of(&state.recipe, ctx.base, ctx.law) == 0 {
            continue;
        }
        let sub = joint::Joint::new(ctx);
        let mut point = joint::Point::new(state.recipe.clone());
        let e = sub.repair(&mut point);
        renders += sub.renders.get();
        if trace {
            eprintln!(
                "alone {name}: repaired J* {:.5} price {:.3} F {:.5} (the solve: J* {j_main:.5} F {f_main:.5})",
                e.j,
                e.price,
                e.f()
            );
        }
        if (e.f() < f_main || e.j < j_main) && triggered.as_ref().is_none_or(|(f, _, _)| e.f() < *f) {
            triggered = Some((e.f(), name, state));
        }
    }
    let read = |state: &mut SolveState, tail: String| {
        if let Some(done) = state.finished.as_mut() {
            done.spent = format!("{}; alone check {renders} renders{tail}", done.spent);
        }
    };
    let Some((_, name, mut alt)) = triggered else {
        read(&mut main, String::new());
        return main;
    };
    stage_joint(ctx, &mut alt);
    let (f_alt, spent_alt) = {
        let done = alt.finished.as_ref().expect("stage_joint finishes");
        (done.f, done.spent.clone())
    };
    if trace {
        eprintln!("alone {name}: solved F {f_alt:.5} against the sequence's {f_main:.5}");
    }
    if f_alt < f_main {
        read(&mut alt, format!(", solved again from the {name} alone and shipped (the sequence: F {f_main:.5})"));
        alt
    } else {
        read(&mut main, format!(", solved again from the {name} alone: F {f_alt:.5}, {spent_alt}"));
        main
    }
}

/// Run the solve from `from` on `state`: that stage and every one after it,
/// in order. From [`Stage::Tone`] on [`SolveState::at`] the base, this is
/// the whole global solve.
fn run_from(ctx: &SolveCtx<'_>, from: Stage, mut state: SolveState) -> SolveState {
    for stage in Stage::ALL.into_iter().filter(|s| *s >= from) {
        stage.run(ctx, &mut state);
    }
    state
}

/// What the stages carry from one to the next: the recipe as they leave it
/// and the facts each one wrote for the report.
#[derive(Clone, Default)]
struct SolveState {
    /// The recipe through the stages run so far.
    recipe: EditRecipe,
    wb_facts: super::atmosphere::WbFacts,
    sat_pegged: bool,
    grade_held: Option<(f32, f32)>,
    detail: (f32, f32),
    detail_supported: bool,
    hsl_facts: HslStageFacts,
    /// The joint stage's result.
    finished: Option<Finished>,
}

impl SolveState {
    /// The state no stage has touched: the base recipe, no facts.
    fn at(base: &EditRecipe) -> Self {
        SolveState { recipe: base.clone(), ..SolveState::default() }
    }
}


/// `base` with `recipe`'s exposure, five tone sliders and master curve: the
/// frame the white-balance stage reads its demand on, and the tone's own
/// work for G's reading of the tone class (`joint::class_frames`).
pub(super) fn tone_part(base: &EditRecipe, recipe: &EditRecipe) -> EditRecipe {
    let mut toned = base.clone();
    toned.exposure_ev = recipe.exposure_ev;
    toned.contrast = recipe.contrast;
    toned.highlights = recipe.highlights;
    toned.shadows = recipe.shadows;
    toned.whites = recipe.whites;
    toned.blacks = recipe.blacks;
    toned.tone_curve = recipe.tone_curve.clone();
    toned
}

/// `recipe` with the tone's SHAPE at the base's — the contrast, the four
/// range sliders and the master curve — and its exposure kept: the frame
/// the tone's contrast reading starts from (`validity::Cells::stretched`).
/// What an exposure does to a range's display contrast is the level move's
/// own consequence and sits in both of that reading's frames.
pub(super) fn without_tone_shape(base: &EditRecipe, recipe: &EditRecipe) -> EditRecipe {
    let mut exposed = recipe.clone();
    exposed.contrast = base.contrast;
    exposed.highlights = base.highlights;
    exposed.shadows = base.shadows;
    exposed.whites = base.whites;
    exposed.blacks = base.blacks;
    exposed.tone_curve = base.tone_curve.clone();
    exposed
}

/// The solve's read-only context: the pair, its evidence and cells, the
/// budget, and every value the stages derive from the pair alone, computed
/// once at the base.
pub(super) struct SolveCtx<'a> {
    pub(super) s_img: &'a DynamicImage,
    pub(super) sp: &'a [[f32; 3]],
    pub(super) tp: &'a [[f32; 3]],
    pub(super) evidence: &'a EvidenceModel,
    pub(super) cells: &'a Option<crate::fit_cells::PairedCells>,
    pub(super) base: &'a EditRecipe,
    strength: crate::recipe::GradeStrength,
    pub(super) budget: FitBudget,
    pub(super) full_sat_limit: f32,
    /// The exposure's bound in the descent: the recipe's own reach in Full
    /// mode, the strength budget's EV axis in Atmosphere mode — the budget
    /// is a constraint of G like the white-balance gains, not only the
    /// seed's clamp (2026-11-07: p41 at 0.85 shipped +2.65 EV past the
    /// budget's 1.86 because only the stage's seed was clamped).
    pub(super) ev_limit: f32,
    /// The cells of the control-validity law G reads every candidate on
    /// (`Joint::refusal`, 2026-11-07), built once off the pair.
    pub(super) validity: super::validity::Cells,
    full_slope: (f32, f32),
    s_cdf: Vec<f32>,
    t_cdf: Vec<f32>,
    robust_tone: Option<PairedRobustTone>,
    correspondence: Vec<(f32, f32)>,
    paired: bool,
    robust_facts: Option<(f32, String)>,
    rw_source: Vec<f32>,
    rw_target: Vec<f32>,
    point_span: Option<(f32, f32)>,
    n_pixels: f32,
    tone_deliverable: bool,
    pub(super) law: render::WbLaw,
    pub(super) wb_weights: Vec<f32>,
    t_chroma: f32,
    /// The share of the frame each priced control reaches, read once on the
    /// base render ([`joint::Reach`]).
    pub(super) reach: joint::Reach,
}

impl<'a> SolveCtx<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        s_img: &'a DynamicImage,
        sp: &'a [[f32; 3]],
        tp: &'a [[f32; 3]],
        evidence: &'a EvidenceModel,
        cells: &'a Option<crate::fit_cells::PairedCells>,
        base: &'a EditRecipe,
        options: &'a FitOptions<'a>,
        pairing: PairingScale,
    ) -> Self {
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
        // The MARGINAL arm's evidence comes from NEAR-NEUTRAL pixels: saturated
        // pixels clip channels at the gamut ceiling under chroma scaling, so
        // their luma lands short of the tone map and would bias a quantile map
        // (measured: one polluted knot skews contrast by tens of points) — and
        // `tone_cdf_pair_weighted` drops that gate itself where the two sides'
        // neutral classes are not one population. The PAIRED arm below reads
        // EVERY paired pixel (2026-10-21): it has each pixel's own counterpart
        // and a robust fit to reject the inconsistent ones, so the gate was not
        // identifying anything there — it was choosing a population. On the
        // seaside street the near-neutral eighth of the frame is its shadow
        // walls, which the target crushes (0.06–0.28 → 0.024), while the
        // chromatic seven eighths at the same brightness go 0.06 → 0.058 and
        // 0.25 → 0.156; the gated map solved contrast 100 and the lower-left
        // shadows went black. The frame's map is the frame's.
        let (s_cdf, t_cdf) = tone_cdf_pair_weighted(sp, tp, evidence);
        let robust_tone = paired_robust_tone(
            sp,
            tp,
            &|i: usize| {
                evidence
                    .source_weights
                    .get(i)
                    .copied()
                    .unwrap_or(0.0)
                    .min(evidence.target_weights.get(i).copied().unwrap_or(0.0))
            },
            false,
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
        let correspondence = paired_correspondence(robust_tone.as_ref(), pairing);
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
        let law = render::WbLaw::of(base);
        let wb_weights: Vec<f32> = {
            let n = sp.len().min(tp.len());
            // At PIXEL scale a pixel's robust tone weight is a statement about its
            // own pairing; at CELL scale it is not, and the cell's structural
            // trust takes its place.
            let robust = robust_tone
                .as_ref()
                .filter(|_| paired && pairing == PairingScale::Pixel);
            (0..n)
                .map(|i| {
                    let trust = match (&robust, &cells) {
                        (Some(r), _) => r.weights.get(i).copied().unwrap_or(0.0),
                        (None, Some(c)) => c.pixel_trust(i),
                        (None, None) => 1.0,
                    };
                    evidence.source_weights[i].min(evidence.target_weights[i]).max(0.0) * trust
                })
                .collect()
        };
        let t_chroma = weighted_mean_chroma(tp, &rw_target).unwrap_or_else(|| mean_chroma(tp));
        let mut ctx = SolveCtx {
            s_img,
            sp,
            tp,
            evidence,
            cells,
            base,
            strength: options.strength,
            budget,
            full_sat_limit,
            ev_limit: 5.0,
            validity: super::validity::Cells::new(sp, tp, evidence),
            full_slope,
            s_cdf,
            t_cdf,
            robust_tone,
            correspondence,
            paired,
            robust_facts,
            rw_source,
            rw_target,
            point_span,
            n_pixels,
            tone_deliverable: false,
            law,
            wb_weights,
            t_chroma,
            reach: joint::Reach::of(sp, &base.color_grade),
        };
        ctx.tone_deliverable = {
            let tone_map = |x: f32| ctx.tone_map(x);
            let mut previous = tone_map(0.0);
            (1..=256).all(|step| {
                let current = tone_map(step as f32 / 256.0);
                let monotone = current + 1e-4 >= previous;
                previous = current;
                monotone
            })
        };
        ctx
    }

    /// The context of the Atmosphere solve's joint objective: this mode's
    /// ruler (the structure-blind population model) on the frame, no cells,
    /// no pairing, the strength budget's own saturation axis as the bound.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn atmosphere(
        s_img: &'a DynamicImage,
        sp: &'a [[f32; 3]],
        tp: &'a [[f32; 3]],
        evidence: &'a EvidenceModel,
        base: &'a EditRecipe,
        strength: crate::recipe::GradeStrength,
        budget: FitBudget,
        law: render::WbLaw,
        wb_weights: Vec<f32>,
    ) -> Self {
        static NO_CELLS: Option<crate::fit_cells::PairedCells> = None;
        let (s_cdf, t_cdf) = tone_cdf_pair_weighted(sp, tp, evidence);
        let n = sp.len().min(tp.len());
        SolveCtx {
            s_img,
            sp,
            tp,
            evidence,
            cells: &NO_CELLS,
            base,
            strength,
            budget,
            full_sat_limit: budget.sat,
            ev_limit: budget.ev,
            validity: super::validity::Cells::new(sp, tp, evidence),
            full_slope: budget.slope,
            s_cdf,
            t_cdf,
            robust_tone: None,
            correspondence: Vec::new(),
            paired: false,
            robust_facts: None,
            rw_source: evidence.source_weights.clone(),
            rw_target: evidence.target_weights.clone(),
            point_span: None,
            n_pixels: n as f32,
            tone_deliverable: false,
            law,
            wb_weights,
            t_chroma: weighted_mean_chroma(tp, &evidence.target_weights).unwrap_or_else(|| mean_chroma(tp)),
            reach: joint::Reach::of(sp, &base.color_grade),
        }
    }

    fn luma_supported(&self, user: f32) -> bool {
        let (point_span, paired, evidence, n_pixels) = (self.point_span, self.paired, self.evidence, self.n_pixels);
        match point_span {
            Some((lo, hi)) if paired => user >= lo && user <= hi,
            _ => {
                evidence.luma[evidence_luma_bin(user)].source_share * n_pixels
                    >= SUPPORT_MIN_PIXELS
            }
        }
    }

    fn estimated_tone_map(&self, x: f32) -> f32 {
        let (correspondence, s_cdf, t_cdf) = (&self.correspondence, &self.s_cdf, &self.t_cdf);
        if correspondence.len() >= 6 {
            sample_tone_points(correspondence, x)
        } else {
            quantile(t_cdf, cdf_at(s_cdf, x).clamp(P_CLIP, 1.0 - P_CLIP))
        }
    }

    fn tone_range_withheld(&self, x: f32) -> bool {
        let evidence = self.evidence;
        let range = &evidence.luma[evidence_luma_bin(x)];
        range.weight <= 0.0 && range.source_share >= EVIDENCE_MIN_SHARE
    }

    fn tone_map(&self, x: f32) -> f32 {
        let tone_range_withheld = |x: f32| self.tone_range_withheld(x);
        let estimated_tone_map = |x: f32| self.estimated_tone_map(x);
        if tone_range_withheld(x) {
            x
        } else {
            estimated_tone_map(x)
        }
    }

    /// The saturation chase: the mean-chroma ratio, secant-refined through
    /// the engine. Returns whether the chase hit the model cap.
    fn chase_saturation(&self, recipe: &mut EditRecipe) -> bool {
        let (s_img, rw_source, t_chroma, full_sat_limit) = (self.s_img, &self.rw_source, self.t_chroma, self.full_sat_limit);
        let mut pegged = false;
        for _ in 0..2 {
            let cur = pixels_of(&render::develop_preview(s_img, recipe));
            let c_chroma =
                weighted_mean_chroma(&cur, rw_source).unwrap_or_else(|| mean_chroma(&cur));
            if c_chroma < 1e-4 {
                break;
            }
            let step = ((t_chroma / c_chroma - 1.0) * 100.0).clamp(-40.0, 40.0);
            if step.abs() < 1.0 {
                break;
            }
            let want = recipe.saturation + step;
            let clamped = want.clamp(-full_sat_limit, full_sat_limit);
            // Hitting the model cap with demand to spare = the target's chroma
            // is out of the global model's reach — flagged into the rationale
            // so the user learns WHY the fit stays approximate.
            if (want - clamped).abs() > 0.5 {
                pegged = true;
            }
            recipe.saturation = round1(clamped);
        }
        pegged
    }

    // The paired voucher every hue-damage guard consults (see
    // converges_toward): robust per-pixel weights plus each pixel's own
    // paired target, judged on the full display pixel because the mixer and
    // the cast curves move brightness and colour together. None on
    // marginal-path runs — strict doctrine.
    pub(super) fn hue_vouch(&self) -> Option<PixelVouch<'_>> {
        let (robust_tone, paired, tp) = (&self.robust_tone, self.paired, self.tp);
        robust_tone
            .as_ref()
            .filter(|_| paired)
            .map(|r| (r.weights.as_slice(), tp, crate::fit_cells::MoveAxis::Full))
    }

    /// The recipe as the tone stages leave it: `recipe`'s exposure, five tone
    /// sliders and master curve on the base. The white-balance stage reads its
    /// demand and its cells' verdict from this frame, and the cast hand-off
    /// reads the same verdict from it, so it is derived from the state the
    /// caller holds, never carried (rel1180 RULING-1180-sc-r7: a carried
    /// render would read a state that no longer exists).
    fn through_tone(&self, recipe: &EditRecipe) -> EditRecipe {
        tone_part(self.base, recipe)
    }

    /// [`Self::through_tone`]'s render.
    fn render_through_tone(&self, recipe: &EditRecipe) -> Vec<[f32; 3]> {
        pixels_of(&render::develop_preview(self.s_img, &self.through_tone(recipe)))
    }

}

/// Stage 1: the tone sliders — the estimate (RULING-1180-joint-r1: no
/// per-stage judgement; the joint objective decides what ships).
fn stage_tone(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let evidence = ctx.evidence;
    let (robust_tone, paired, tone_deliverable) = (&ctx.robust_tone, ctx.paired, ctx.tone_deliverable);
    let luma_supported = |user: f32| ctx.luma_supported(user);
    let tone_map = |x: f32| ctx.tone_map(x);
    let mut recipe = std::mem::take(&mut st.recipe);
    // --- 1) tone: exposure scan × linear solve on the engine's knot basis ----
    let knot_support: [f32; 8] =
        std::array::from_fn(|i| if luma_supported(render::TONE_KNOTS_X[i]) { 1.0 } else { 0.0 });
    let score_set: Vec<(f32, f32, f32)> = match robust_tone.as_ref() {
        Some(r) if paired => r
            .points
            .iter()
            .zip(&r.masses)
            .map(|(&(x, y), &mass)| (x, y, mass))
            .collect(),
        _ => Vec::new(),
    };
    let (ev, sliders) = if !tone_deliverable {
        (0.0, [0.0; 5])
    } else {
        fit_tone_sliders_supported(&tone_map, &knot_support, &score_set)
    };
    recipe.exposure_ev = round2(ev);
    recipe.contrast = round1(sliders[0] * 100.0);
    recipe.highlights = round1(sliders[1] * 100.0);
    recipe.shadows = round1(sliders[2] * 100.0);
    recipe.whites = round1(sliders[3] * 100.0);
    recipe.blacks = round1(sliders[4] * 100.0);

    // A slider whose own range the evidence does not cover starts at zero:
    // the knot support decides what the solve may bend, this decides what
    // the start may claim (the descent can still move it).
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
    st.recipe = recipe;
}

/// Stage 2: the residual master curve — the estimate.
fn stage_master_curve(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let (evidence, base) = (ctx.evidence, ctx.base);
    let (full_slope, tone_deliverable) = (ctx.full_slope, ctx.tone_deliverable);
    let luma_supported = |user: f32| ctx.luma_supported(user);
    let tone_map = |x: f32| ctx.tone_map(x);
    let mut recipe = std::mem::take(&mut st.recipe);
    // --- 2) residual master curve (composed on top of the sliders) -----------
    // Domain care (R16): the sliders solved in the USER domain (their input
    // is the base curve's output — `tone_map` above maps base-render luma to
    // target luma), but `residual_tone_curve` samples the recipe's FULL LUT,
    // whose input is the NEUTRAL domain. Rebase the map through the base
    // LUT: full(x) = user_map(base(x)). A base with no starting tone skips the rebase
    // outright — build_tone_lut's sRGB↔linear round trip is only ~1e-7 from
    // identity, but skipping keeps the default-base wrapper literally
    // bit-for-bit the old fit (review R16 #3).
    let base_toned = !base.base_curve.is_empty();
    let base_lut = render::build_tone_lut(base);
    let full_map = |x: f32| {
        if !base_toned { tone_map(x) } else { tone_map(render::sample_lut(&base_lut, x)) }
    };
    let mut withheld_samples = Vec::new();
    for (bin, range) in evidence.luma.iter().enumerate() {
        if range.weight > 0.0 || range.source_share < EVIDENCE_MIN_SHARE {
            continue;
        }
        let lo = bin as f32 / EVIDENCE_LUMA_BINS as f32;
        let hi = (bin + 1) as f32 / EVIDENCE_LUMA_BINS as f32;
        for level in [lo, 0.5 * (lo + hi), hi] {
            let raw = if !base_toned {
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
            if !base_toned { x } else { render::sample_lut(&base_lut, x) };
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
    st.recipe = recipe;
}

/// Stage 2b: the white balance — the demand, closed-loop through the
/// engine, inside the strength budget. Whether it ships is the joint
/// objective's question, asked on the target's own paired pixels.
fn stage_white_balance(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let (s_img, tp, evidence, budget, strength) = (ctx.s_img, ctx.tp, ctx.evidence, ctx.budget, ctx.strength);
    let (law, wb_weights) = (ctx.law, &ctx.wb_weights);
    let mut recipe = std::mem::take(&mut st.recipe);
    // It sits AFTER tone and BEFORE saturation for the reason stage 3 gives
    // about the cast curves: the chroma chase reads mean chroma, and reading
    // it on a frame whose illuminant is still wrong asks saturation to pay
    // for a white-balance error. The demand is read from the frame AS THE
    // STAGE FINDS IT (2026-10-21): the render after the tone stage, not the
    // base — a strong contrast move changes the three channel ratios on its
    // own, and a demand read from the base charges that change to the light.
    let before_wb = ctx.render_through_tone(&recipe);
    let (free_k, free_tint, _wanted) =
        paired_wb_from_populations(&before_wb, tp, wb_weights, law);
    // The demand closed-loop through the engine (`paired_wb_closed_loop`):
    // the open-loop reading under-asks. No demand stays no demand.
    let (free_k, free_tint) = if (free_k, free_tint) == ((law.as_shot().0 / 50.0).round() * 50.0, round1(law.as_shot().1)) {
        (free_k, free_tint)
    } else {
        paired_wb_closed_loop(
            (free_k, free_tint),
            tp,
            wb_weights,
            law,
            |wb| {
                let mut probe = recipe.clone();
                (probe.temperature_k, probe.tint) = (Some(wb.0), wb.1);
                pixels_of(&render::develop_preview(s_img, &probe))
            },
            |px| look_err_with_evidence(px, tp, evidence),
        )
    };
    let wb_facts = solve_white_balance(&mut recipe, (free_k, free_tint), law, budget, strength);
    st.recipe = recipe;
    st.wb_facts = wb_facts;
}


/// Stage 3: global saturation, chased on the frame the stages before left.
fn stage_saturation(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let chase_saturation = |recipe: &mut EditRecipe| ctx.chase_saturation(recipe);
    let mut recipe = std::mem::take(&mut st.recipe);
    // --- 3) global saturation, secant-refined through the real engine --------
    // Saturation stays BEFORE the cast curves: channel CDFs of a desaturated
    // render differ from the target's even with zero cast (each channel's
    // distribution is compressed toward luma), so fitting the cast first
    // would express chroma expansion through per-channel curves — and
    // per-channel curves rotate hue. Saturating first may amplify a latent
    // cast, but stage 5 fits the cast residual CLOSED-LOOP on the saturated
    // render, so it is measured and removed rather than compounded.
    let sat_pegged = chase_saturation(&mut recipe);
    st.recipe = recipe;
    st.sat_pegged = sat_pegged;
}

/// Stage 3b: the colour-grade wheels — the estimate.
fn stage_grade(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let (s_img, tp, budget, wb_weights) = (ctx.s_img, ctx.tp, ctx.budget, &ctx.wb_weights);
    let mut recipe = std::mem::take(&mut st.recipe);
    // --- 3b) colour grade: the white balance's brightness-dependent
    // remainder, on the shadow / midtone / highlight wheels (`grade`).
    let grade_held =
        grade::fit_grade_stage(s_img, tp, wb_weights, budget.grade_wheel, &mut recipe);
    st.recipe = recipe;
    st.grade_held = grade_held;
}


/// The detail controls.
fn stage_detail(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let (s_img, tp, evidence) = (ctx.s_img, ctx.tp, ctx.evidence);
    let mut recipe = std::mem::take(&mut st.recipe);
    // NOTE deliberately NO mid-pipeline hue veto here EITHER (it was tried
    // in the evidence era and removed): the ordering comment above is the
    // contract — saturation legitimately amplifies a latent cast and the
    // cast stage then measures and removes it, so judging the SAT-ONLY
    // render against the hue evidence vetoes exactly the amplification the
    // next stage exists to fix (measured: the haze pair's +59 chase was
    // reset by the blue-cast bands the cast stage went on to empty). The
    // validity law is read on the composed result, as G of the joint
    // objective (`Joint::admissible`).
    // NOTE deliberately NO validation here: a correct saturation legitimately
    // makes every colour metric worse at THIS point in the pipeline (it
    // amplifies a latent cast into the channel means and the hue bands; the
    // curve stage then measures and removes it — see the ordering comment
    // above). The only fair evaluation point is the finished recipe, where
    // the joint objective judges saturation with every other control. (A
    // stage-local gate was tried first and it zeroed the haze regression's
    // saturation, degrading the whole fit.)

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
    // Per-band HSL is fitted in stage 4a above, on bands the two-sided
    // population gate can measure: where the pixels pair, all three axes
    // from the paired pixels (`fit_band_amounts`, v1.18.0; `fit_band_hues`,
    // 2026-11-02), and where they do not, SATURATION and LUMINANCE from each
    // band's population and no hue. The 2026-07-07 failure this comment used
    // to record was the hue axis read from POPULATIONS: a band's centroid hue
    // delta conflates CONTENT difference with style, and an honest-looking
    // 13° in-gate delta applied as a whole-band rotation turns brown rock
    // olive and a pale sky lavender. That reading is still never made; the
    // paired solve reads each pixel's own rotation, on pairs whose pixels are
    // each other's counterparts, and ships only what the target's cells
    // vouch.
    let (detail, detail_supported) = fit_detail_stage(s_img, tp, evidence, &mut recipe);
    st.recipe = recipe;
    st.detail = detail;
    st.detail_supported = detail_supported;
}

/// Stage 4a: the per-band mixer, as fitted — the estimate. One mixer solve on
/// the frame the stages before left.
fn stage_mixer(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let (s_img, sp, tp, evidence, cells, budget) = (ctx.s_img, ctx.sp, ctx.tp, ctx.evidence, ctx.cells, ctx.budget);
    let hue_vouch = ctx.hue_vouch();
    let mut recipe = std::mem::take(&mut st.recipe);
    // --- 4a) per-band colour mixer, BEFORE the cast curves ------------------
    // Ordering follows the stage-3 argument one level down: the mixer is the
    // SPECIFIC colour move (one band at a time, from that band's own
    // pixels), the channel curves are the catch-all, and the catch-all
    // must close its loop on everything the specific stages already did.
    let hsl_facts = fit_hsl_stage(s_img, sp, tp, evidence, hue_vouch, cells.as_ref(), budget, &mut recipe);
    st.recipe = recipe;
    st.hsl_facts = hsl_facts;
}

/// Stage 4: the per-channel colour-cast curves, as fitted — the catch-all,
/// LAST so its closed-loop residual sees every earlier stage's composed
/// output: one cast fit on the frame the stages before left, refined on the
/// finished frame; the validity law (G) judges the curves on every candidate
/// the descent tries, and a refused start is scaled, not emptied
/// (`Joint::repair`). Its own stage since 2026-11-10 so the alone check can
/// propose the curves without the mixer ([`alone_check`]).
fn stage_cast(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let mut recipe = std::mem::take(&mut st.recipe);
    fit_cast_curves(ctx, &mut recipe);
    st.recipe = recipe;
}

/// The joint objective (RULING-1180-joint-r1): the start repaired into G
/// (a refused class re-estimated from the law's asks), descended on F with
/// the coordinates moving together, polished coordinate by coordinate at
/// every level, each control held to its absence, polished once more; what
/// ships and its readings.
fn stage_joint(ctx: &SolveCtx<'_>, st: &mut SolveState) {
    let mut joint = joint::Joint::new(ctx);
    let mut point = joint::Point::new(std::mem::take(&mut st.recipe));
    joint.trace_start(&point);
    let start = joint.repair(&mut point);
    joint.mark("repair");
    joint.trace("repair", &point, &start);
    // The stages' proposal as it entered the descent (after the repair,
    // which resets for admissibility, not for earning).
    let proposed = point.recipe.clone();
    let best = joint.descend(&mut point, start, &joint::STEP_MULTIPLIERS);
    joint.mark("descend");
    joint.trace("descend", &point, &best);
    let mut best = joint.polish(&mut point, best, &joint::STEP_MULTIPLIERS);
    joint.mark("polish");
    joint.trace("polish", &point, &best);
    let mut unearned = joint.eliminate(&mut point, &mut best);
    joint.mark("eliminate");
    joint.trace("eliminate", &point, &best);
    let best = joint.polish(&mut point, best, &joint::STEP_MULTIPLIERS[2..]);
    joint.mark("polish");
    joint.trace("final polish", &point, &best);
    joint::walked_back(&proposed, &point.recipe, ctx.base, &mut unearned);
    let hsl_facts = std::mem::take(&mut st.hsl_facts);
    let f = best.f();
    st.finished = Some(Finished {
        recipe: point.recipe,
        hsl_facts,
        unearned,
        end_px: best.px,
        err_after: best.j,
        f,
        scaled: joint.scaled(),
        spent: joint.spent_line(),
    });
}

/// The cast stage's estimate: the per-channel curves fitted on the frame's
/// residual and closed on the finished frame (RULING-1180-joint-r1: no
/// per-stage judgement; the one validity law reads the curves between their
/// own frames in G, `Joint::refusal`, and the repair scales a refused seed).
fn fit_cast_curves(ctx: &SolveCtx<'_>, recipe: &mut EditRecipe) {
    let (s_img, tp, evidence) = (ctx.s_img, ctx.tp, ctx.evidence);
    let (rw_source, rw_target) = (&ctx.rw_source, &ctx.rw_target);
    recipe.red_curve = Vec::new();
    recipe.green_curve = Vec::new();
    recipe.blue_curve = Vec::new();
    let cur = pixels_of(&render::develop_preview(s_img, recipe));
    recipe.red_curve = residual_channel_curve_weighted(&cur, tp, 0, rw_source, rw_target);
    recipe.green_curve = residual_channel_curve_weighted(&cur, tp, 1, rw_source, rw_target);
    recipe.blue_curve = residual_channel_curve_weighted(&cur, tp, 2, rw_source, rw_target);
    if recipe.red_curve.is_empty() && recipe.green_curve.is_empty() && recipe.blue_curve.is_empty() {
        return;
    }
    // v1.17.1: the curves are read off the finished frame's residual but
    // applied before the mixer, the grade and saturation, so what they
    // deliver on the finished frame is not the residual they were solved
    // for; the loop is closed on the finished frame
    // (`refine_curves_on_finished_frame`).
    refine_curves_on_finished_frame(s_img, recipe, &cur, tp, rw_source, rw_target, evidence);
}

/// What the joint stage leaves: the recipe that ships and its readings.
#[derive(Clone)]
struct Finished {
    recipe: EditRecipe,
    hsl_facts: HslStageFacts,
    unearned: Vec<Control>,
    end_px: Vec<[f32; 3]>,
    /// J* of the finished frame.
    err_after: f32,
    /// F of the finished recipe: J* plus its degrees of freedom.
    f: f32,
    /// The classes the repair scaled the stages' estimate on, with the
    /// fraction kept (`Joint::scaled`).
    scaled: Vec<(&'static str, f32)>,
    /// The joint's render count and where its seconds went (a reading).
    spent: String,
}
