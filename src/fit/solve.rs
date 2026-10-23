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
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
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
                divergence_coarse: readings.coarse,
                pairing: readings.scale(),
                evidence: &evidence,
                structural_evidence: None,
                defer_disclosure,
            },
            SolveFacts {
            budget: Some(FitBudget::for_strength(options.strength)), strength: Some(options.strength.get()), veto_luma: None, veto_hue: None, wb_clamped: None,
                wb_search_bound: None, wb_rotation_coverage: None, wb_rotation_disclosure: None, cast_admitted_by_strength: None, cast_admitted: None,
                cast_projected: None,
                wb_cells: None,
                wb_foreign_hue_withheld: false,
                wb_rotation_withheld: false,
                sat_pegged: None,
                cast: CastOutcome::default(),
                evidence_refused: false,
                sat_fitted: None,
                regressed: None,
                tone_harm: None,
                curve_harm: None,
                detail: (0.0, 0.0),
                detail_withheld: false,
                robust: None,
                paired: false,
                vouched_bands: None,
                cast_cells: None,
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
                divergence_coarse: readings.coarse,
                pairing: readings.scale(),
                evidence: &evidence,
                structural_evidence: None,
                defer_disclosure,
            },
            SolveFacts { budget: Some(FitBudget::for_strength(options.strength)), strength: Some(options.strength.get()), veto_luma: None, veto_hue: None, wb_clamped: None, wb_search_bound: None, wb_rotation_coverage: None, wb_rotation_disclosure: None, cast_admitted_by_strength: None, cast_admitted: None, cast_projected: None, wb_cells: None, wb_foreign_hue_withheld: false, wb_rotation_withheld: false, sat_pegged: None, cast: CastOutcome::default(), evidence_refused: false, sat_fitted: None, regressed: None, tone_harm: None, curve_harm: None, detail: (0.0, 0.0), detail_withheld: true, robust: None, paired: false, vouched_bands: None, cast_cells: None, hsl: HslStageFacts::default(), atmosphere_reference: AtmosphereReference::WholeFrame },
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
    // Do-no-harm, PER STAGE (2026-10-21). The terminal check below resets
    // the WHOLE recipe to the base when the finished render lands farther
    // from the target than the untouched source — and on the lantern street
    // it was the tone stage alone that had put it there: the paired robust
    // regression asked for +1.65 EV and contrast 7.5 against a target that
    // is darker and warmer (a sixth of the pairs are content the two frames
    // do not share, and the survivors still disagree with the population),
    // the look error went 0.0595 → 0.0815 on the sliders alone, every later
    // stage inherited the deficit, and the terminal threw away a white
    // balance the cells had vouched along with the sliders that caused the
    // harm. A stage that renders the frame farther from the target than it
    // found it ships nothing; the stages after it solve from the frame as
    // it was. The ruler is the TONAL term of the terminal's objective — the
    // term the sliders control (`look::tonal_err_with_evidence`). Held to
    // the whole objective, a correct luma move was charged for the
    // colour-mean drift it brings to a muted frame whose target is vivid
    // (the land-and-sky toy: whole objective 0.1233 → 0.1240 on the sliders
    // alone, and the zones downstream lost the global move they were solved
    // beside), drift the saturation, white-balance and cast stages exist to
    // correct; the lantern street's sliders raised the tonal term itself,
    // 0.0468 → 0.0748, and are refused on it alone.
    let tonal_before = look::tonal_err_with_evidence(&sp, &tp, &evidence);
    let mut tone_harm = None;
    {
        let toned = pixels_of(&render::develop_preview(&s_img, &recipe));
        let err_toned = look::tonal_err_with_evidence(&toned, &tp, &evidence);
        if err_toned > tonal_before + 1e-4 {
            recipe.exposure_ev = base.exposure_ev;
            recipe.contrast = base.contrast;
            recipe.highlights = base.highlights;
            recipe.shadows = base.shadows;
            recipe.whites = base.whites;
            recipe.blacks = base.blacks;
            tone_harm = Some((tonal_before, err_toned));
        }
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
    // The residual curve's own do-no-harm (2026-10-21), the same rule as the
    // sliders': it fits the paired map more exactly than five sliders can,
    // and where that map disagrees with the population the exactness is the
    // harm — the seaside street's sliders took the frame 0.0746 → 0.0538 and
    // the curve on top of them took it back to 0.0649 (its tonal term
    // 0.0555 → 0.0745). Judged against the render WITHOUT it, so a curve on
    // withheld sliders is judged against the base and not against a harm
    // the sliders already undid; and on the tonal term, as the sliders are,
    // for the same reason. Measured 2026-10-21 on four real pairs, the
    // tonal term, the whole objective and the cells' vouch along luminance
    // agreed on every curve they refused (the seaside street's cells read
    // 0.174 converged / 0.509 diverged under its curve).
    let mut curve_harm = None;
    if recipe.tone_curve != base.tone_curve {
        let with_curve = pixels_of(&render::develop_preview(&s_img, &recipe));
        let err_with = look::tonal_err_with_evidence(&with_curve, &tp, &evidence);
        let mut without = recipe.clone();
        without.tone_curve = base.tone_curve.clone();
        let err_without = look::tonal_err_with_evidence(
            &pixels_of(&render::develop_preview(&s_img, &without)),
            &tp,
            &evidence,
        );
        if err_with > err_without + 1e-4 {
            recipe.tone_curve = base.tone_curve.clone();
            curve_harm = Some((err_without, err_with));
        }
    }

    // --- 2b) white balance ---------------------------------------------------
    // Full mode had no white-balance stage AT ALL until R33 — not a gate that
    // could open, an absent stage — so the one control that says "the light
    // was a different colour" existed only on the branch taken when the
    // structure was judged unrecoverable. A same-frame regrade that turned a
    // grey sky orange therefore had to express the whole cast through
    // saturation, the per-band mixer and three channel curves, and the hue
    // gates (correctly) refused most of it.
    //
    // It sits AFTER tone and BEFORE saturation for the reason stage 3 already
    // gives about the cast curves: the chroma chase reads mean chroma, and
    // reading it on a frame whose illuminant is still wrong asks saturation to
    // pay for a white-balance error.
    //
    // ADMISSION, not estimation. The demand is solved from the population
    // exactly as Atmosphere solves it; what decides whether it SHIPS is the
    // target's own verdict on the render it produces — the paired cells over
    // the whole frame. A cast the cells say took the frame AWAY from its
    // target is returned to as-shot and named, because an edit may not create
    // its own evidence.
    let anchor = base.as_shot_k.unwrap_or(5500.0);
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
    // The demand is read from the frame AS THE STAGE FINDS IT (2026-10-21):
    // the render after the tone stage, not the base. The tone sliders act on
    // every channel, so a strong contrast move changes the three channel
    // ratios between the base and the target on its own, and a demand read
    // from the base charges that change to the light: the seaside street
    // (contrast 100) read 40000 K / tint −24.2 from the base and 17400 K /
    // +4.8 from the toned render against the same target. The chroma chase
    // below has always read the current render for the same reason.
    let before_wb = pixels_of(&render::develop_preview(&s_img, &recipe));
    let (free_k, free_tint, _wanted) =
        atmosphere_wb_from_populations(&before_wb, &tp, &wb_weights, anchor);
    let wb_restore = (recipe.temperature_k, recipe.tint);
    let wb_facts = solve_white_balance(
        &s_img,
        &tp,
        &mut recipe,
        base,
        &evidence,
        (free_k, free_tint),
        anchor,
        budget,
        options.strength,
    );
    // The verdict is on what the stage DID, not on what it wanted — and it
    // is asked along the axis the stage can move. A white balance moves
    // chromaticity only, so its direction test reads chromaticity
    // (`MoveAxis::Colour`), as a zone's colour probe has since 2026-10-19;
    // put to the FULL direction it was refused wherever the target was also
    // darker or brighter than the tone stage left the cell, because that
    // direction is mostly luminance and no white balance can point along
    // it. Measured 2026-10-21: the lantern street's warm target read 0.866
    // converged / 0.065 diverged and still 0.502 aligned, the seaside
    // street 0.847 / 0.117 / 0.546; both casts went back to as-shot.
    let wb_moved = (recipe.temperature_k, recipe.tint) != wb_restore;
    let vouch_wb = |recipe: &EditRecipe| {
        cells.as_ref().map(|c| {
            c.vouch_along(
                crate::fit_cells::MoveAxis::Colour,
                &before_wb,
                &pixels_of(&render::develop_preview(&s_img, recipe)),
                None,
            )
        })
    };
    let wb_cells = wb_moved.then(|| vouch_wb(&recipe)).flatten();
    // R36 for the white balance (2026-10-21). A refusal on SIZE is not a
    // refusal on DIRECTION: the seaside street's cast read 0.847 converged /
    // 0.117 diverged / 0.954 aligned along chromaticity — nineteen cells in
    // twenty asked for the move and a ninth of the mass was pushed PAST its
    // own target, because the one white balance the population solved was
    // too large for those cells, not wrong for them — and the stage returned
    // the whole cast to as-shot. As the zone probes do (`vouched_share`), the
    // largest share of the solved move whose RENDER the same voucher admits
    // is searched for instead, along the as-shot-to-solved path the budget
    // itself walks (`wb_path_between`: the log-gain segment, each point
    // inverted for its own Kelvin, quantised as the solve quantises).
    // Admission stays evidence — the white balance that ships is
    // one the cells vouched — and the search is a budget on size. It is asked
    // only where the direction agrees: a frame whose cells want opposite
    // moves is not asking for less of one move and stays refused. And a
    // share is accepted only on SIZE evidence (`WB_SHARE_SIZE_EVIDENCE`):
    // the cells that diverged at the full move must mostly stop diverging at
    // the share, or they were not pushed past their targets but away from
    // them — the seaside street's sky, a tenth of the frame under a cast the
    // other nine tenths asked for, kept diverging at 0.969 of the move and
    // the threshold alone would have shipped it.
    let mut wb_shipped_at = None;
    if let Some(full) = wb_cells
        && !full.vouched()
        && full.direction_agrees()
    {
        let solved = (recipe.temperature_k.unwrap_or(anchor), recipe.tint);
        let from = (wb_restore.0.unwrap_or(anchor), wb_restore.1);
        let at = |share: f32| -> (Option<f32>, f32) {
            let (k, tint) = wb_path_between(anchor, from, solved, share);
            (Some(k), tint)
        };
        let (mut lo, mut hi, mut best) = (0.0f32, 1.0f32, None);
        for _ in 0..WB_VOUCH_SHRINK_STEPS {
            let mid = 0.5 * (lo + hi);
            (recipe.temperature_k, recipe.tint) = at(mid);
            match vouch_wb(&recipe) {
                Some(v) if v.vouched() && v.diverged <= full.diverged * WB_SHARE_SIZE_EVIDENCE => {
                    best = Some((mid, v));
                    lo = mid;
                }
                _ => hi = mid,
            }
        }
        if let Some((share, _)) = best {
            (recipe.temperature_k, recipe.tint) = at(share);
        }
        wb_shipped_at = best;
    }
    // An ABSTENTION cannot admit a control either: where no cell resolved
    // there is no verdict, and a stage that did not exist before does not get
    // to ship on silence.
    if wb_moved && !wb_cells.is_some_and(|v| v.vouched()) && wb_shipped_at.is_none() {
        (recipe.temperature_k, recipe.tint) = wb_restore;
    }
    let wb_admitted = (recipe.temperature_k, recipe.tint) != wb_restore;
    let wb_cells = wb_cells.map(|vouch| WbCellVerdict {
        vouch,
        admitted: wb_admitted,
        shipped_at: wb_shipped_at.filter(|_| wb_admitted),
    });
    // The recipe as the white-balance stage left it, so the cast→WB hand-off
    // (stage 4) can put a moved white balance to the SAME cell check this
    // stage just ran — the same before, the same render of nothing but tone
    // and white balance, the same colour axis — rather than to a second one.
    let wb_stage = recipe.clone();

    // --- 3) global saturation, secant-refined through the real engine --------
    // Saturation stays BEFORE the cast curves: channel CDFs of a desaturated
    // render differ from the target's even with zero cast (each channel's
    // distribution is compressed toward luma), so fitting the cast first
    // would express chroma expansion through per-channel curves — and
    // per-channel curves rotate hue. Saturating first may amplify a latent
    // cast, but stage 5 fits the cast residual CLOSED-LOOP on the saturated
    // render, so it is measured and removed rather than compounded.
    let t_chroma = weighted_mean_chroma(&tp, &rw_target).unwrap_or_else(|| mean_chroma(&tp));
    // One closure, because the cast→WB hand-off (stage 4) moves the white
    // balance this chase was read on and has to chase again on the frame it
    // leaves — the ordering argument above, applied to the white balance it
    // ships rather than to the one stage 2b shipped. Returns whether the
    // chase hit the model cap.
    let chase_saturation = |recipe: &mut EditRecipe| -> bool {
        let mut pegged = false;
        for _ in 0..2 {
            let cur = pixels_of(&render::develop_preview(&s_img, recipe));
            let c_chroma =
                weighted_mean_chroma(&cur, &rw_source).unwrap_or_else(|| mean_chroma(&cur));
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
    };
    let sat_pegged = chase_saturation(&mut recipe);
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
    // paired target, judged on the full display pixel because the mixer and
    // the cast curves move brightness and colour together. None on
    // marginal-path runs — strict doctrine.
    let hue_vouch = robust_tone
        .as_ref()
        .filter(|_| paired)
        .map(|r| (r.weights.as_slice(), tp.as_slice(), crate::fit_cells::MoveAxis::Full));

    // --- 4a) per-band colour mixer, BEFORE the cast curves ------------------
    // Ordering follows the stage-3 argument one level down: the mixer is the
    // SPECIFIC colour move (one band at a time, from that band's own
    // population), the channel curves are the catch-all, and the catch-all
    // must close its loop on everything the specific stages already did. The
    // engine agrees — it runs the mixer before saturation and the RGB curves
    // before the mixer — which is exactly why both colour stages measure
    // their demand on a re-render rather than on an algebraic composition.
    let mut hsl_facts =
        fit_hsl_stage(&s_img, &sp, &tp, &evidence, hue_vouch, cells.as_ref(), budget, &mut recipe);
    let hsl_fitted = recipe.hsl.clone();
    let mut cast_admission: Option<(f32, f32)> = None;
    // NOT cleared per call, unlike `cast_admission`: a hand-off moves the
    // white balance, and every later call re-fits the curves ON that white
    // balance, so the move it describes is still in the recipe after them.
    let mut cast_handoff: Option<HandOffFacts> = None;
    // `rescue` = may a fan-convicted cast be PROJECTED into something
    // milder rather than thrown away (v1.2.3, `search_cast_projection`)?
    // Only the call that produces the recipe the user gets says yes; see the
    // ordering note at the 4a' loop below for why.
    let mut fit_cast_stage = |recipe: &mut EditRecipe, rescue: bool| -> CastOutcome {
        // Cleared per call: the fact must describe the recipe this call
        // produces, not a probe an earlier call admitted.
        cast_admission = None;
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
            // R34 §D3. The REGION arm of the hue-damage guard, recomputed for
            // THIS candidate: a cell verdict is a statement about what an edit
            // did, so it is as short-lived as the edit.
            let with_cells = cells.as_ref().and_then(|c| c.region_arm(&cur, &with_px));
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
                cells.as_ref().zip(with_cells.as_ref().map(|(_, v)| v.as_slice())),
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
                // The cast→WB HAND-OFF (2026-10-23), asked BEFORE the
                // projection: a fan the white balance can take back is a
                // white-balance job the curves were handed (see
                // `search_cast_hand_off`). A candidate ships only when the
                // curves re-fitted on it pass all four gates with a MEASURED
                // fan, when the white balance stays inside the strength
                // budget the white-balance stage spends, and when the target's
                // own cells vouch it exactly as they vouched the stage's own
                // white balance (`vouch_wb` on the stage's recipe). Anything
                // else falls through to the projection below with `recipe`
                // untouched — every probe is a clone — so a pair the hand-off
                // cannot help ships byte for byte what it shipped before.
                let from = (recipe.temperature_k.unwrap_or(anchor), recipe.tint);
                let err_without = look_err_with_evidence(&cur, &tp, &evidence);
                let probe_base = recipe.clone();
                let handed = cast_wb_direction(&cur, &with_px, &wb_weights, anchor).and_then(|direction| {
                    search_cast_hand_off(anchor, from, direction, err_without, |wb| {
                        if !wb_gains_fit_budget(render::wb_gains(anchor, wb.0, wb.1), budget) {
                            return None;
                        }
                        let mut probe = probe_base.clone();
                        (probe.temperature_k, probe.tint) = (Some(wb.0), wb.1);
                        probe.red_curve = Vec::new();
                        probe.green_curve = Vec::new();
                        probe.blue_curve = Vec::new();
                        // Saturation is chased again on the moved white
                        // balance, from where stage 3 started and on the
                        // frame stage 3 read (no mixer yet), because the
                        // chase reads mean chroma and a warmer white balance
                        // carries more of it: kept at the value read on the
                        // old one, the neon street's 18.9 shipped over a
                        // 13000 K white balance and left the building +3.1
                        // a* past its target (measured 2026-10-23).
                        probe.saturation = wb_stage.saturation;
                        let mixer = std::mem::take(&mut probe.hsl);
                        chase_saturation(&mut probe);
                        probe.hsl = mixer;
                        let under = pixels_of(&render::develop_preview(&s_img, &probe));
                        probe.red_curve =
                            residual_channel_curve_weighted(&under, &tp, 0, &rw_source, &rw_target);
                        probe.green_curve =
                            residual_channel_curve_weighted(&under, &tp, 1, &rw_source, &rw_target);
                        probe.blue_curve =
                            residual_channel_curve_weighted(&under, &tp, 2, &rw_source, &rw_target);
                        if probe.red_curve.is_empty()
                            && probe.green_curve.is_empty()
                            && probe.blue_curve.is_empty()
                        {
                            return None;
                        }
                        let px = pixels_of(&render::develop_preview(&s_img, &probe));
                        let arm = cells.as_ref().and_then(|c| c.region_arm(&under, &px));
                        let outcome = cast_gate_outcome_with_ratio(
                            &under,
                            &px,
                            &tp,
                            &evidence,
                            hue_vouch,
                            cells.as_ref().zip(arm.as_ref().map(|(_, v)| v.as_slice())),
                            full_cast_accept_ratio,
                        );
                        if outcome.refused() || outcome.readings.and_then(|r| r.fan).is_none() {
                            return None;
                        }
                        let mut wb_probe = wb_stage.clone();
                        (wb_probe.temperature_k, wb_probe.tint) = (Some(wb.0), wb.1);
                        let vouch = vouch_wb(&wb_probe).filter(|v| v.vouched())?;
                        Some(CastHandOff {
                            wb,
                            saturation: probe.saturation,
                            curves: [probe.red_curve, probe.green_curve, probe.blue_curve],
                            outcome,
                            vouch,
                            err: look_err_with_evidence(&px, &tp, &evidence),
                            pull: cast_wb_direction(&under, &px, &wb_weights, anchor).map_or(
                                0.0,
                                |d| d.iter().map(|v| v * v).sum::<f32>().sqrt(),
                            ),
                        })
                    })
                });
                let won = if let Some(h) = handed {
                    (recipe.temperature_k, recipe.tint) = (Some(h.wb.0), h.wb.1);
                    recipe.saturation = h.saturation;
                    let [red, green, blue] = h.curves;
                    recipe.red_curve = red;
                    recipe.green_curve = green;
                    recipe.blue_curve = blue;
                    cast_handoff = Some(HandOffFacts {
                        from: cast_handoff.map_or(from, |prior| prior.from),
                        to: h.wb,
                        share,
                        fan_before,
                        fan_after: h.outcome.readings.and_then(|r| r.fan).unwrap_or(0.0),
                        vouch: h.vouch,
                    });
                    out = h.outcome;
                    None
                } else {
                    search_cast_projection(
                        &s_img,
                        recipe,
                        [&fitted[0], &fitted[1], &fitted[2]],
                        err_without,
                        |px| {
                            let arm = cells.as_ref().and_then(|c| c.region_arm(&cur, px));
                            cast_gate_outcome_with_ratio(
                                &cur,
                                px,
                                &tp,
                                &evidence,
                                hue_vouch,
                                cells.as_ref().zip(arm.as_ref().map(|(_, v)| v.as_slice())),
                                full_cast_accept_ratio,
                            )
                        },
                    )
                };
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
    // The 4b call's rescue is ENTERED and its success is UNREACHABLE from any
    // state this tree can reach — the census at that loop has the numbers and
    // the reason, which is a fact about how the two colour gates behave in a
    // stepped-down state rather than a hole in the fixture set.
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
    // FALSE and was measured false. Instrumenting the body and running the
    // whole library battery on this tree (2026-09-02, with the calibration
    // corpus present) logs 186 entries: 107 on the error arm, 140 on the
    // hue-guard arm, 62 on both.
    //
    // So the rescue at the `fit_cast_stage` call below is NOT dead code — it
    // is entered with a conviction to answer: 9 of those 186 re-fits hand
    // back a FAN-CONVICTED cast. What no fixture reaches is the rescue
    // SUCCEEDING here, and the measurement says why. All 9 are ALSO
    // rotation-blocked, so `earns_projection` returns `None` on every one of
    // them and the projection is never even attempted — 0 of the 186
    // earn it, 0 come back projected. The coupling belongs to the
    // stepped-down states rather than to the gates: at the stage's own call
    // site the same battery separates them on 67 of 545 stage runs (the
    // census in `a_silently_rejected_colour_stage_now_says_so`).
    //
    // That is a measured dead end, not a postponement. About forty pairs were
    // built for it — one-sided-band wedges to drive the hue-guard arm,
    // split-chroma frames to drive the error arm — and every one reproduced
    // the coupling. The projection arithmetic is pinned where it lives, at
    // `search_cast_projection`
    // (`the_search_takes_the_best_paying_admissible_shrink`); what has no
    // end-to-end witness is the ROUTE through this loop, and the reason it
    // has none is the census above.
    let sat_fitted = recipe.saturation;
    let mut end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
    let mut err_after = look_err_with_evidence(&end_px, &tp, &evidence);
    // The zero-evidence hue guard is judged HERE, on the composed end state:
    // saturation may amplify a latent cast mid-pipeline (the stage-3 note),
    // but the FINISHED recipe must not move pixels blindly through hue bands
    // no evidence covers — if it does, saturation is the shrinkable dial,
    // with the cast curves refitted each step exactly like the error arm.
    let mut end_cells = cells.as_ref().and_then(|c| c.region_arm(&sp, &end_px));
    let mut end_moves_hue = moved_unsupported_hue_range_names_vouched(
        &sp,
        &end_px,
        &evidence,
        hue_vouch,
        cells.as_ref().zip(end_cells.as_ref().map(|(_, v)| v.as_slice())),
    )
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
        end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
        err_after = look_err_with_evidence(&end_px, &tp, &evidence);
        end_cells = cells.as_ref().and_then(|c| c.region_arm(&sp, &end_px));
        end_moves_hue = moved_unsupported_hue_range_names_vouched(
            &sp,
            &end_px,
            &evidence,
            hue_vouch,
            cells.as_ref().zip(end_cells.as_ref().map(|(_, v)| v.as_slice())),
        )
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
        end_cells = cells.as_ref().and_then(|c| c.region_arm(&sp, &end_px));
        end_moves_hue = moved_unsupported_hue_range_names_vouched(
            &sp,
            &end_px,
            &evidence,
            hue_vouch,
            cells.as_ref().zip(end_cells.as_ref().map(|(_, v)| v.as_slice())),
        )
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
    let end_arm = cells.as_ref().zip(end_cells.as_ref().map(|(_, v)| v.as_slice()));
    let vouched_bands = vouched_hue_band_names(&sp, &end_px, &evidence, hue_vouch, end_arm);
    // R34 §D3. The bands the REGION carried, with the verdict it was admitted
    // on — the same one `region_arm` gated on, not a second reading of it.
    // Two outcomes, two sentences: this and `vouched_bands` never describe the
    // same band, because a pixel the paired arm vouches never reaches the cell
    // arm.
    let cast_cells = cell_vouched_hue_band_names(&sp, &end_px, &evidence, hue_vouch, end_arm)
        .zip(end_cells.as_ref().map(|(vouch, _)| *vouch));
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
    let mut harm = terminal_harm(err_before, err_after, joint_base, joint_after);
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
            divergence_coarse: readings.coarse,
            pairing: readings.scale(),
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
                .then(|| {
                    let arm = cells.as_ref().and_then(|c| c.region_arm(&sp, &after_px));
                    moved_unsupported_hue_range_names_vouched(
                        &sp,
                        &after_px,
                        &evidence,
                        hue_vouch,
                        cells.as_ref().zip(arm.as_ref().map(|(_, v)| v.as_slice())),
                    )
                })
                .flatten(),
            // The same budget disclosures Atmosphere makes, because it is the
            // same stage: a Full-mode WB that was scaled back, that landed on
            // the Kelvin domain's edge, or that the hue gates zeroed says so in
            // the same words. Quoted only where a WB actually shipped — a
            // demand the cells refused carries its own note instead.
            wb_clamped: wb_admitted.then_some(wb_facts.clamped).flatten(),
            wb_search_bound: wb_admitted.then_some(wb_facts.search_bound).flatten(),
            wb_rotation_coverage: wb_admitted.then_some(wb_facts.rotation_coverage),
            wb_rotation_disclosure: wb_admitted.then_some(wb_facts.rotation_disclosure).flatten(),
            wb_cells,
            wb_foreign_hue_withheld: wb_admitted && wb_facts.foreign_hue_withheld,
            wb_rotation_withheld: wb_admitted && wb_facts.rotation_withheld,
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
            tone_harm,
            curve_harm,
            detail,
            detail_withheld: !detail_supported,
            robust: robust_facts,
            paired,
            vouched_bands,
            cast_cells,
            hsl: hsl_facts,
            atmosphere_reference: AtmosphereReference::WholeFrame,
        },
    );
    // The hand-off's sentence, wherever the moved white balance is still in
    // the recipe: not after the terminal do-no-harm reset, which put the base
    // back. It stays when the terminal delivered-fan check withdrew the
    // re-fitted curves — the white balance did not go with them, and the
    // delivered-fan sentence that follows says what happened to the curves.
    if let Some(h) = cast_handoff.filter(|_| !fit_regressed) {
        let (converged, diverged, aligned) = h.vouch.shares();
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::FIT_NOTE_CAST_HANDED_TO_WB,
                vec![
                    ("k_before", format!("{:.0}", h.from.0)),
                    ("tint_before", format!("{:+.1}", h.from.1)),
                    ("fan_before", format!("{:.1}", h.fan_before)),
                    ("share", format!("{:.3}", h.share)),
                    ("limit", format!("{FAN_DEG:.0}")),
                    ("k_after", format!("{:.0}", h.to.0)),
                    ("tint_after", format!("{:+.1}", h.to.1)),
                    ("fan_after", format!("{:+.1}", h.fan_after)),
                    ("converged", converged),
                    ("diverged", diverged),
                    ("aligned", aligned),
                ],
            ),
        );
        // A hand-off that stopped on the Kelvin domain's edge says so in the
        // white-balance stage's own words and on the stage's own terms
        // (`solve_white_balance` discloses the bound above the default
        // strength only), so the clamp is as visible here as it is there.
        if options.strength.get() > crate::recipe::GradeStrength::DEFAULT
            && (h.to.0 <= WB_SEARCH_K.0 || h.to.0 >= WB_SEARCH_K.1)
        {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::FIT_NOTE_WB_SEARCH_BOUND,
                    vec![("k", format!("{:.0}", h.to.0))],
                ),
            );
        }
    }
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
