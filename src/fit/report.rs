//! The report: the measured facts, the composed report, the finished disclosure, and re-scoring.

use super::*;

/// What a [`FitReport`]'s notes need that only a MEASUREMENT can supply — all
/// of it re-derivable from a (source, target, recipe) triple at any later time.
pub(super) struct Measured<'a> {
    pub(super) err_before: f32,
    pub(super) err_after: f32,
    pub(super) joint_after: Option<crate::fit_zoned::JointReading>,
    /// The FINISHED render, i.e. the recipe applied to the source thumbnail.
    pub(super) after_px: &'a [[f32; 3]],
    /// The target thumbnail.
    pub(super) tp: &'a [[f32; 3]],
    pub(super) same_frame: bool,
    pub(super) mode: FitMode,
    pub(super) divergence: Option<Divergence>,
    pub(super) divergence_coarse: Option<Divergence>,
    pub(super) pairing: PairingScale,
    pub(super) evidence: &'a EvidenceModel,
    pub(super) structural_evidence: Option<&'a EvidenceModel>,
    pub(super) defer_disclosure: bool,
}

/// …and what only the SOLVE can supply: decisions the solver made on its way to
/// the recipe, which no later re-measurement of that recipe can recover.
///
/// Split out from [`Measured`] precisely because the split is the contract for
/// [`rescore_report`]: a recipe someone ADJUSTED after the solve can honestly
/// re-derive everything on the measured side and nothing on this one.
#[derive(Clone)]
pub(super) struct SolveFacts {
    /// Budget used by the atmosphere solve, when applicable.
    pub(super) budget: Option<FitBudget>,
    /// Panel strength used to derive the budget. Kept absent for historical
    /// rescoring and Full-mode reports so the shipped default remains stable.
    pub(super) strength: Option<f32>,
    /// Unsupported movement retained as a high-strength disclosure.
    pub(super) veto_luma: Option<String>,
    pub(super) veto_hue: Option<String>,
    pub(super) wb_clamped: Option<(f32, f32, f32, f32)>,
    /// Free white-balance search landed on the finite Kelvin domain edge.
    pub(super) wb_search_bound: Option<f32>,
    /// Coverage from the same WB rotation census used for its gate.
    pub(super) wb_rotation_coverage: Option<f32>,
    pub(super) wb_rotation_disclosure: Option<(f32, f32)>,
    /// R33 §E: the white balance the population asked for was solved, RENDERED
    /// and put to the target's own cells. A different claim from the two
    /// around it: those are capability vetoes on what the edit would do to
    /// hue, this is the target's verdict on whether it was the right edit.
    /// `None` = no WB was solved, or no cell resolved to judge one.
    pub(super) wb_cells: Option<WbCellVerdict>,
    /// The fitted WB was returned to as-shot because it created target-foreign hues.
    pub(super) wb_foreign_hue_withheld: bool,
    /// The fitted WB was returned to as-shot because it exceeded the strength
    /// budget's weighted region-rotation allowance.
    pub(super) wb_rotation_withheld: bool,
    /// The chroma chase hit this mode's model cap with demand to spare. The
    /// mode travels with the solve fact because `rescore_report` may classify
    /// an old adjusted recipe differently without changing what the original
    /// solve actually did.
    pub(super) sat_pegged: Option<FitMode>,
    /// Which of the colour stage's gates (if either) refused the cast curves.
    pub(super) cast: CastOutcome,
    /// Accepted cast whose measured error ratio is above the shipped gate but
    /// within a widened high-strength budget.
    pub(super) cast_admitted_by_strength: Option<(f32, f32)>,
    /// Every gate reading of a cast that was ADMITTED, so the shipped
    /// curves disclose the numbers they passed on. `None` = no curves
    /// shipped, in any of THREE ways: a gate refused them, none were fitted
    /// at all, or the TERMINAL do-no-harm check reset the whole recipe to
    /// the calibration base. The third leaves the gates' own verdict at
    /// "admitted" while nothing of the solve — curves included — ships,
    /// which is why the construction site ands in `!fit_regressed`.
    pub(super) cast_admitted: Option<CastReadings>,
    /// v1.2.3: the cast was PROJECTED — the fan gate convicted the fitted
    /// curves and a shrunk version of them shipped instead. Mutually
    /// exclusive with `cast_admitted` (a projected cast was not admitted as
    /// fitted) and carrying the same `!fit_regressed` guard, for the same
    /// reason: the terminal do-no-harm reset can leave the gates' verdict at
    /// "projected" while nothing of the solve ships.
    pub(super) cast_projected: Option<CastProjection>,
    /// The existing evidence gates withheld a one-sided range. This is the
    /// cause carried into the FAR classifier; it is not a second refusal flag.
    pub(super) evidence_refused: bool,
    /// `Some(sat_fitted)` when the do-no-harm loop shrank saturation away from
    /// the chroma-matched value the chase produced.
    pub(super) sat_fitted: Option<f32>,
    /// `Some(joint_arm_fired)` when the TERMINAL do-no-harm check reset the
    /// whole recipe to the calibration base.
    pub(super) regressed: Option<bool>,
    /// Per-stage do-no-harm (2026-10-21): the tone sliders, or the residual
    /// curve, rendered the frame farther from the target than it was without
    /// them (`before` → `after`) and were withheld at their own stage.
    pub(super) tone_harm: Option<(f32, f32)>,
    pub(super) curve_harm: Option<(f32, f32)>,
    pub(super) detail: (f32, f32),
    pub(super) detail_withheld: bool,
    /// Paired robust regression engaged and down-weighted this share of the
    /// comparable pixels (plus the luma ranges holding the rejected mass).
    /// `None` = the paired path did not run: nothing was rejected AND nothing
    /// was measured — two silences the disclosure keeps apart by speaking
    /// only when a measurement exists.
    pub(super) robust: Option<(f32, String)>,
    /// The tone map came from paired pixels, not marginal CDF transport — the
    /// summary must not claim the target is unaligned when the solve just
    /// used its alignment.
    pub(super) paired: bool,
    /// One-sided hue bands that vouched convergence carried movement through
    /// on the finished render — disclosed so the withheld-note's "vetoed
    /// movement" claim is never silently contradicted.
    pub(super) vouched_bands: Option<String>,
    /// R34 §D3: one-sided hue bands the REGION's own cells carried movement
    /// through, and the verdict they were carried on. A different claim from
    /// `vouched_bands` and therefore a different field: those pixels were
    /// never individually vouched, and saying they were would be false.
    pub(super) cast_cells: Option<(String, crate::fit_cells::CellVouch)>,
    /// The per-band colour mixer's own verdicts: which bands it could not
    /// measure, and whether it gave back what it fitted.
    pub(super) hsl: HslStageFacts,
    /// R30 R2: which population the Atmosphere white balance and exposure
    /// were read over. Only the solve knows — a re-measurement of the
    /// finished recipe cannot tell a whole-frame median from a shared-content
    /// one — which is why it rides here and not on [`Measured`].
    pub(super) atmosphere_reference: AtmosphereReference,
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
pub(super) fn compose_report(mut recipe: EditRecipe, m: Measured<'_>, solve: SolveFacts) -> FitReport {
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
                // An unread frame says so rather than printing a 0.000 that
                // would read as a measured perfect match.
                ("d", m.divergence.map_or_else(
                    || "unmeasured".to_string(), |r| format!("{:.3}", r.d))),
            ],
        ),
    );
    // BOTH readings and the pairing scale, on every mode. The pair of numbers
    // is the fit's own answer to "which of these two images am I allowed to
    // pair with which", and until R33 the Full-mode summaries carried `d` in
    // their arguments and printed none of it — a solve could land in Full or
    // in Atmosphere on a reading the user never saw.
    let reading = |r: Option<Divergence>| {
        r.map_or_else(|| "unmeasured".to_string(), |r| format!("{:.3}", r.d))
    };
    push_note(
        &mut rationale,
        &mut notes,
        Note::new(
            match m.pairing {
                PairingScale::Pixel => keys::FIT_NOTE_PAIRING_PIXEL,
                PairingScale::Cell => keys::FIT_NOTE_PAIRING_CELL,
            },
            vec![
                ("fine", reading(m.divergence)),
                ("coarse", reading(m.divergence_coarse)),
            ],
        ),
    );
    // …and when the mode was a near thing, the margin that decided it. A pair
    // 0.001 from the line and a pair 0.3 from it get the same one-word verdict
    // otherwise, and only one of them deserves to be believed.
    if let Some(d) = m.divergence
        && (d.d - DIVERGENCE_GLOBAL).abs() <= MODE_MARGIN_DISCLOSED
    {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_MODE_MARGIN,
                vec![
                    ("margin", format!("{:.3}", (d.d - DIVERGENCE_GLOBAL).abs())),
                    ("line", format!("{DIVERGENCE_GLOBAL:.2}")),
                ],
            ),
        );
    }
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
    for (harm, key) in [
        (solve.tone_harm, keys::FIT_NOTE_TONE_WITHHELD_HARM),
        (solve.curve_harm, keys::FIT_NOTE_CURVE_WITHHELD_HARM),
    ] {
        if let Some((before, after)) = harm {
            push_note(
                &mut rationale,
                &mut notes,
                Note::new(key, vec![("before", format!("{before:.4}")), ("after", format!("{after:.4}"))]),
            );
        }
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
    // The cells' verdict on a rendered white balance: BOTH outcomes are said
    // out loud. A silent admission would leave "the sky is finally the right
    // colour" indistinguishable from a lucky population statistic, and a
    // silent refusal would leave the commonest question about this stage — why
    // is temperature still as-shot on a pair whose light obviously changed? —
    // unanswered.
    if let Some(verdict) = solve.wb_cells {
        // The shares printed are the SHIPPED move's — the full move's where
        // it shipped or was refused, the share's where a share did — and the
        // share sentence prints the full move's refusal beside them.
        let (converged, diverged, aligned) =
            verdict.shipped_at.map_or(verdict.vouch, |(_, at_share)| at_share).shares();
        let mut args = vec![("converged", converged), ("diverged", diverged), ("aligned", aligned)];
        let key = match (verdict.admitted, verdict.shipped_at) {
            (true, Some((share, _))) => {
                let (full_converged, full_diverged, full_aligned) = verdict.vouch.shares();
                args.push(("share", format!("{share:.3}")));
                args.push((
                    "full",
                    format!(
                        "{full_converged} converged, {full_diverged} diverged, {full_aligned} aligned"
                    ),
                ));
                args.push(("full_converged", full_converged));
                args.push(("full_diverged", full_diverged));
                args.push(("full_aligned", full_aligned));
                keys::FIT_NOTE_WB_CELLS_VOUCHED_AT_SHARE
            }
            (true, None) => keys::FIT_NOTE_WB_CELLS_VOUCHED,
            (false, _) => keys::FIT_NOTE_WB_CELLS_REFUSED,
        };
        push_note(&mut rationale, &mut notes, Note::new(key, args));
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
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_VETO_DISCLOSED, vec![("kind", crate::rationale::values::LUMA_RANGES.into()), ("ranges", ranges.clone())]));
    }
    if let Some(ranges) = &solve.veto_hue {
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_VETO_DISCLOSED, vec![("kind", crate::rationale::values::HUE_BANDS.into()), ("ranges", ranges.clone())]));
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
    if let Some((bands, verdict)) = &solve.cast_cells {
        let (converged, diverged, aligned) = verdict.shares();
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_CAST_CELLS_VOUCHED,
                vec![
                    ("bands", bands.clone()),
                    ("converged", converged),
                    ("diverged", diverged),
                    ("aligned", aligned),
                ],
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
    if !solve.hsl.vouched.is_empty() {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_HSL_BANDS_VOUCHED,
                vec![("bands", solve.hsl.vouched.clone())],
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
        divergence_coarse: m.divergence_coarse,
        pairing: m.pairing,
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
/// `base` is the recipe the SOLVE started from — the caller's composed
/// calibration base (`pipeline::calibration_recipe`) on a photograph, the
/// default on a fixture — and the evidence model, the divergence reading and
/// the veto disclosures are all measured against ITS develop. Until
/// 2026-09-24 this function developed a bare default instead, so on every
/// photo whose calibration was not neutral (which, since v1.6.0's estimated
/// base look, is every photo) the rescored Atmosphere evidence model was not
/// the solve's, and `calibration_atmosphere_rescore_reproduces_report_ruler`
/// held only because the fixture's base is the default.
pub fn rescore_report(
    src: &DynamicImage,
    target: &DynamicImage,
    recipe: &EditRecipe,
    base: &EditRecipe,
    err_before: f32,
    prior: &[crate::rationale::Note],
) -> FitReport {
    use crate::rationale::keys;
    let same_frame = same_frame_plausible(src, target);
    let (s, t) = analysis_pair(src, target);
    let tp = pixels_of(&t);
    let after_px = pixels_of(&render::develop_preview(&s, recipe));
    let base_px = pixels_of(&render::develop_preview(&s, base));
    let structural = evidence_model_paired(&base_px, &tp, presence_of(&t).as_deref(), s.width(), s.height());
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
    let readings = divergence_pair_for(src, target, base);
    let divergence = readings.fine;
    // Same stance as the solve path: an unread frame is not promoted.
    let mode = if divergence.is_some_and(|r| r.d >= DIVERGENCE_GLOBAL)
        || carried(keys::FIT_SUMMARY_ATMOSPHERE)
    {
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
            divergence_coarse: readings.coarse,
            pairing: readings.scale(),
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
            // A refusal the CELLS made is a measurement of a render this
            // rescore did not take, so it rides the carried note like every
            // other gate verdict — the shares with it, or the abstention if
            // the note carried none.
            wb_cells: [
                (keys::FIT_NOTE_WB_CELLS_REFUSED, false),
                (keys::FIT_NOTE_WB_CELLS_VOUCHED, true),
                (keys::FIT_NOTE_WB_CELLS_VOUCHED_AT_SHARE, true),
            ]
            .into_iter()
            .find(|(key, _)| carried(key))
            .map(|(key, admitted)| {
                let read = |arg: &str| {
                    carried_arg(key, arg).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0)
                };
                let vouch_of = |prefix: &str| crate::fit_cells::CellVouch {
                    converged: read(&format!("{prefix}converged")),
                    diverged: read(&format!("{prefix}diverged")),
                    aligned: read(&format!("{prefix}aligned")),
                    read: 1,
                };
                // The share sentence carries two verdicts: the full move's
                // (its refusal) and the shipped share's.
                let at_share = carried_arg(key, "share")
                    .and_then(|v| v.parse::<f32>().ok())
                    .map(|share| (share, vouch_of("")));
                WbCellVerdict {
                    vouch: if at_share.is_some() { vouch_of("full_") } else { vouch_of("") },
                    admitted,
                    shipped_at: at_share,
                }
            }),
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
            tone_harm: None,
            curve_harm: None,
            detail: (recipe.clarity, recipe.texture),
            detail_withheld: false,
            robust: None,
            paired: carried(keys::FIT_SUMMARY_WITH_CURVE_PAIRED)
                || carried(keys::FIT_SUMMARY_NO_CURVE_PAIRED),
            vouched_bands: carried_arg(keys::FIT_NOTE_VOUCHED_CONVERGENCE, "bands"),
            // …and its region twin, recovered the way the WB cell verdict is:
            // a rescore re-renders but never re-runs the cell instrument, so
            // the measurement rides the note it was published in.
            cast_cells: carried_arg(keys::FIT_NOTE_CAST_CELLS_VOUCHED, "bands").map(|bands| {
                let read = |arg: &str| {
                    carried_arg(keys::FIT_NOTE_CAST_CELLS_VOUCHED, arg)
                        .and_then(|v| v.parse::<f32>().ok())
                        .unwrap_or(0.0)
                };
                (
                    bands,
                    crate::fit_cells::CellVouch {
                        converged: read("converged"),
                        diverged: read("diverged"),
                        aligned: read("aligned"),
                        read: 1,
                    },
                )
            }),
            // The mixer's evidence verdicts cross over for the same reason
            // the cast gates do: the deep step moves global dials, it never
            // re-runs the per-band population gate. What the mixer MOVED is
            // deliberately NOT carried — `compose_report` reads that straight
            // off the recipe in front of it, which here is the adjusted one.
            hsl: HslStageFacts {
                refused: carried_arg(keys::FIT_NOTE_HSL_BANDS, "refused")
                    .filter(|refused| refused != "none")
                    .unwrap_or_default(),
                vouched: carried_arg(keys::FIT_NOTE_HSL_BANDS_VOUCHED, "bands")
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
    .with_producer_notes(prior)
}

impl FitReport {
    /// R33 §H. [`rescore_report`] regenerates the GLOBAL solve's account from
    /// the recipe in front of it, field by field off [`SolveFacts`]. Every note
    /// a LOCAL PRODUCER wrote — zones, native ranges, spatial tiles, free
    /// masks, the boundary gate, the guided refiner, the local field — has no
    /// field there to ride on, so a `--zoned` fit that went through the deep
    /// arm reached the user with its whole local half deleted: the masks were
    /// still in the recipe and still rendering, and the rationale no longer
    /// mentioned that they existed.
    ///
    /// The carrying rule is a DENYLIST and the prefix IS the denylist:
    /// `FIT_*` is the global solve's own family, every key of it either
    /// regenerated above or deliberately dropped there (`FIT_NOTE_REGRESSED`,
    /// `FIT_NOTE_JOINT_REGRESSED`, `FIT_NOTE_SAT_REDUCED` — see the doc on
    /// `rescore_report`), and both of those verdicts are preserved unchanged.
    /// Everything else rides through, in its original order, after the
    /// regenerated head. An allowlist was the defect: it is a list a new
    /// producer forgets to join, and five families had.
    ///
    /// What this deliberately does NOT claim: that a producer note's NUMBERS
    /// survive the adjustment. "Zone residual 0.505 → 0.494" was measured
    /// before the deep step moved a global dial. It is kept because it is a
    /// true statement about an action the zoned pass TOOK and the adjustment
    /// did not undo — the mask is still there — and because the alternative
    /// on the table is silence about a correction the user can see. The deep
    /// step accounts for itself separately through `FIT_NOTE_DEEP_ADOPTED`.
    fn with_producer_notes(mut self, prior: &[crate::rationale::Note]) -> Self {
        let carried = prior
            .iter()
            .filter(|n| !crate::rationale::is_global_solve_key(n.key));
        for note in carried {
            crate::rationale::push_note(
                &mut self.recipe.rationale,
                &mut self.notes,
                note.clone(),
            );
        }
        self
    }
}
