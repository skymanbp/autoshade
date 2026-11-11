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
    /// The evidence cells no evidence spoke for that the recipe moved,
    /// retained as a high-strength disclosure (the validity law's blind
    /// cells, `fit::validity`).
    pub(super) veto_cells: Option<String>,
    /// `(ratio_before, ratio_after)`: the white balance was scaled into the
    /// strength budget's gains.
    pub(super) wb_clamped: Option<(f32, f32)>,
    /// Free white-balance search landed on the finite Kelvin domain edge.
    pub(super) wb_search_bound: Option<f32>,
    /// The classes the joint solve's repair scaled the stages' estimate on
    /// and the fraction each kept (`rationale::values`, translated).
    pub(super) scaled: Vec<(&'static str, f32)>,
    /// The chroma chase hit this mode's model cap with demand to spare. The
    /// mode travels with the solve fact because `rescore_report` may classify
    /// an old adjusted recipe differently without changing what the original
    /// solve actually did.
    pub(super) sat_pegged: Option<FitMode>,
    /// The existing evidence gates withheld a one-sided range. This is the
    /// cause carried into the FAR classifier; it is not a second refusal flag.
    pub(super) evidence_refused: bool,
    /// `Some(joint_arm_fired)` when the TERMINAL do-no-harm check reset the
    /// whole recipe to the calibration base.
    pub(super) regressed: Option<bool>,
    /// v1.18.0 R8: the controls `eliminate_unearned` set back, named.
    pub(super) unearned: Option<String>,
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
    /// The per-band colour mixer's own verdicts: which bands it could not
    /// measure, and whether it gave back what it fitted.
    pub(super) hsl: HslStageFacts,
    /// The colour grade was held at the strength budget's wheel ceiling
    /// (`FitBudget::grade_wheel`): `(ceiling, asked)`, the Saturation it was
    /// held at and the largest one the paired pixels asked for.
    pub(super) grade_held: Option<(f32, f32)>,
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
    if let Some(controls) = &solve.unearned {
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_UNEARNED, vec![("controls", controls.clone())]));
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
    }
    if m.evidence.identifiability < 0.08 {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_EVIDENCE_UNMEASURABLE));
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
    if let Some((from, to)) = solve.wb_clamped {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(keys::FIT_NOTE_WB_BUDGETED, vec![("from", format!("{from:.2}")), ("to", format!("{to:.2}"))]),
        );
    }
    if let Some(k) = solve.wb_search_bound {
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_WB_SEARCH_BOUND, vec![("k", format!("{k:.0}"))]));
    }
    // The joint solve's start: a class the validity law refused as the stages
    // proposed it entered the descent scaled toward the base, and says so.
    for &(class, lambda) in &solve.scaled {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(keys::FIT_NOTE_SEED_SCALED, vec![("class", class.into()), ("lambda", format!("{lambda:.2}"))]),
        );
    }
    if let Some(cells) = &solve.veto_cells {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_VETO_DISCLOSED,
                vec![("kind", crate::rationale::values::EVIDENCE_CELLS.into()), ("ranges", cells.clone())],
            ),
        );
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
    if let Some(mixer) = mixer_note(&recipe, &solve.hsl.refused) {
        push_note(&mut rationale, &mut notes, mixer);
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
    // The colour grade, read off the recipe for the same reason as the
    // mixer: a later do-no-harm loop may have shrunk or reset it.
    if let Some(grade) = colour_grade_note(&recipe) {
        push_note(&mut rationale, &mut notes, grade);
        if let Some((ceiling, asked)) = solve.grade_held {
            push_note(
                &mut rationale,
                &mut notes,
                Note::new(
                    keys::FIT_NOTE_COLOR_GRADE_HELD,
                    vec![("ceiling", format!("{ceiling:.0}")), ("asked", format!("{asked:.0}"))],
                ),
            );
        }
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
    let mut cap = match m.joint_after {
        Some(j) => clamp_confidence(1.0 - j.weighted * crate::fit_zoned::JOINT_CONFIDENCE_SLOPE),
        None => 1.0,
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
    cap = cap.min(clamp_confidence(evidence_cap));
    // …and never more than a fit whose two sides are not the same rectangle
    // may claim (R24 batch 2). THIRD in the same one-directional chain, and
    // last because it is the only one of the three that no measurement of
    // these pixels can raise: both readings above are taken over populations
    // the crop already made incomparable, so this cap overrides a high number
    // rather than competing with it. `min`, not an assignment — a fit that is
    // ALSO far by its own residual keeps the lower claim.
    if !m.same_frame {
        cap = cap.min(NOT_SAME_FRAME_CONFIDENCE_CAP);
    }
    if m.mode == FitMode::Atmosphere {
        cap = cap.min(solve.budget.map(|b| b.confidence_cap).unwrap_or(ATMOSPHERE_CONFIDENCE_CAP));
    }
    if let Some(budget) = solve.budget
        && budget.vetoes == VetoPolicy::Disclose
        && solve.veto_cells.is_some()
    {
        cap = cap.min(budget.confidence_cap);
    }
    // The look-error ladder, and never more than the caps allow.
    recipe.confidence = confidence_from_look_err(err_after).min(cap);
    recipe.clamp();
    FitReport {
        recipe,
        err_before,
        err_after,
        confidence_cap: cap,
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
/// rendered.  This keeps the note's `after_px` contract truthful. Every
/// zoned producer calls it on its own finished render, so a later call
/// SUPERSEDES the earlier reading: the sentence describes the frame that
/// ships, and there is one of those (until 2026-11-07 the earlier sentence
/// stayed, and a frame whose residual the global solve no longer lowers read
/// it after the ranges and again after the tiles).
pub(crate) fn append_finished_disclosure(
    report: &mut FitReport,
    after_px: &[[f32; 3]],
    tp: &[[f32; 3]],
) {
    crate::rationale::withdraw_notes(&mut report.recipe.rationale, &mut report.notes, |n| {
        n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
            || n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
    });
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
/// `prior` is the solved report's notes, and four families cross over — all
/// statements about the SOLVE that the adjustment cannot falsify:
///   * `FIT_NOTE_SAT_PEGGED` — the chroma chase hit the ±60 model cap, a fact
///     about the target's chroma being out of the model's reach;
///   * `FIT_NOTE_REHUE_BLOCKED` / `FIT_NOTE_CAST_REJECTED` — which gate refused
///     the colour stage, and the adjustment does not refit those curves.
///   * `FIT_NOTE_VOUCHED_CONVERGENCE` — which one-sided bands the paired solve
///     individually vouched; adjusting the recipe does not rerun that solve.
///   * `FIT_NOTE_SEED_SCALED` — at what fraction of the stages' estimate each
///     class the validity law refused entered the joint solve; the adjustment
///     does not rerun that solve either (carried since 2026-11-08, when the
///     Full-mode round trip stopped being byte-identical without it).
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
    // the shipped path is untouched. From 0.85 the solve DISCLOSED the cells
    // no evidence spoke for and capped its claim; a rescoring re-derives that
    // disclosure from the recipe it describes (the deep step moved the dial)
    // on the pair's own cells — never cloned off the solve, and never
    // dropped: dropping it let the rescored report claim the uncapped ladder
    // for the same movement. A disclosure this rescoring adds can only lower
    // the claim.
    let budget = FitBudget::for_strength(carried_strength);
    let veto_cells = (budget.vetoes == VetoPolicy::Disclose)
        .then(|| super::validity::Cells::new(&base_px, &tp, evidence).blind_cell_names(&base_px, &after_px))
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
            veto_cells,
            wb_clamped: None,
            wb_search_bound: None,
            // The seed's scaling is the solve's own act, a statement about the
            // SOLVE the adjustment cannot falsify, so it crosses over from the
            // solved report's notes (the class named by its reported value).
            scaled: prior
                .iter()
                .filter(|n| n.key == keys::FIT_NOTE_SEED_SCALED)
                .filter_map(|n| {
                    let arg = |k: &str| n.args.iter().find(|(a, _)| *a == k).map(|(_, v)| v.as_str());
                    use crate::rationale::values;
                    let class = match arg("class")? {
                        c if c == values::CLASS_TONE => values::CLASS_TONE,
                        c if c == values::CLASS_WHITE_BALANCE => values::CLASS_WHITE_BALANCE,
                        c if c == values::CLASS_CHROMA_DIALS => values::CLASS_CHROMA_DIALS,
                        c if c == values::CLASS_CHANNEL_CURVES => values::CLASS_CHANNEL_CURVES,
                        _ => return None,
                    };
                    Some((class, arg("lambda")?.parse::<f32>().ok()?))
                })
                .collect(),
            sat_pegged: if carried(keys::FIT_NOTE_ATMOSPHERE_SAT_PEGGED) {
                Some(FitMode::Atmosphere)
            } else if carried(keys::FIT_NOTE_SAT_PEGGED) {
                Some(FitMode::Full)
            } else {
                None
            },
            evidence_refused: carried(keys::FIT_NOTE_EVIDENCE_WITHHELD),
            // Dropped on purpose — see the doc above. Naming them here rather
            // than omitting them silently is the point: the abstention has to
            // be visible at the place that makes it.
            regressed: None,
            unearned: carried_arg(keys::FIT_NOTE_UNEARNED, "controls"),
            detail: (recipe.clarity, recipe.texture),
            detail_withheld: false,
            robust: None,
            paired: carried(keys::FIT_SUMMARY_WITH_CURVE_PAIRED)
                || carried(keys::FIT_SUMMARY_NO_CURVE_PAIRED),
            // The mixer's evidence verdicts cross over for the same reason
            // the blind-cell disclosure does: the deep step moves global dials, it never
            // re-runs the per-band population gate. What the mixer MOVED is
            // deliberately NOT carried — `compose_report` reads that straight
            // off the recipe in front of it, which here is the adjusted one.
            hsl: HslStageFacts {
                refused: carried_arg(keys::FIT_NOTE_HSL_BANDS, "refused")
                    .filter(|refused| refused != "none")
                    .unwrap_or_default(),
                vouched: carried_arg(keys::FIT_NOTE_HSL_BANDS_VOUCHED, "bands")
                    .unwrap_or_default(),
            },
            // Carried like the mixer's verdicts: the deep step never re-runs
            // the grade stage, so whether the dial held it is the solve's.
            grade_held: carried_arg(keys::FIT_NOTE_COLOR_GRADE_HELD, "ceiling")
                .zip(carried_arg(keys::FIT_NOTE_COLOR_GRADE_HELD, "asked"))
                .and_then(|(c, a)| Some((c.parse::<f32>().ok()?, a.parse::<f32>().ok()?))),
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

/// Whether `recipe` carries a mixer value off zero on `band`.
fn band_moved(recipe: &EditRecipe, band: usize) -> bool {
    recipe.hsl.hue[band] != 0.0 || recipe.hsl.saturation[band] != 0.0 || recipe.hsl.luminance[band] != 0.0
}

/// The mixer's sentence for `recipe`: what it moved, read off the recipe, and
/// the bands the stage's population gate refused that the recipe LEAVES
/// neutral (`refused`: the stage's `"<band> (<reason>)"` entries, comma
/// separated; empty or "none" for none). The joint solve moves every
/// coordinate the law admits, a refused band's among them, and a band it
/// moved was not left neutral: it is stated once, under what moved. `None`
/// when nothing moved and nothing is left to name.
fn mixer_note(recipe: &EditRecipe, refused: &str) -> Option<crate::rationale::Note> {
    let or_none = |text: String| if text.is_empty() { "none".to_string() } else { text };
    let moved = hsl_moved_text(recipe);
    let left: Vec<&str> = refused
        .split(", ")
        .filter(|entry| !entry.is_empty() && *entry != "none")
        .filter(|entry| {
            let name = entry.split(" (").next().unwrap_or(entry);
            !(0..EVIDENCE_HUE_BANDS).any(|band| crate::recipe::HSL_BANDS[band] == name && band_moved(recipe, band))
        })
        .collect();
    (!moved.is_empty() || !left.is_empty()).then(|| {
        crate::rationale::Note::new(
            crate::rationale::keys::FIT_NOTE_HSL_BANDS,
            vec![("moved", or_none(moved)), ("refused", or_none(left.join(", ")))],
        )
    })
}

/// What the mixer moved, as the report states it: read off `recipe`, one
/// entry per band with a value off zero; empty when none has.
fn hsl_moved_text(recipe: &EditRecipe) -> String {
    (0..EVIDENCE_HUE_BANDS)
        .filter(|&band| band_moved(recipe, band))
        .map(|band| {
            let hue = recipe.hsl.hue[band];
            format!(
                "{} {}sat {:+.0} lum {:+.0}",
                crate::recipe::HSL_BANDS[band],
                if hue != 0.0 { format!("hue {hue:+.0} ") } else { String::new() },
                recipe.hsl.saturation[band],
                recipe.hsl.luminance[band]
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The colour grade's sentence, read off `recipe`; `None` at a neutral grade.
fn colour_grade_note(recipe: &EditRecipe) -> Option<crate::rationale::Note> {
    let cg = &recipe.color_grade;
    let wheel = |hue: f32, sat: f32| format!("hue {hue:.0} sat {sat:.0}");
    (!cg.is_neutral()).then(|| {
        crate::rationale::Note::new(
            crate::rationale::keys::FIT_NOTE_COLOR_GRADE,
            vec![
                ("shadows", wheel(cg.shadow_hue, cg.shadow_sat)),
                ("midtones", wheel(cg.midtone_hue, cg.midtone_sat)),
                ("highlights", wheel(cg.highlight_hue, cg.highlight_sat)),
            ],
        )
    })
}

/// The mixer's and the grade's sentences stated again in place — in `notes`
/// and in the rendered `rationale` — from what `recipe` carries NOW
/// ([`mixer_note`], on the refusals the sentence already names;
/// [`colour_grade_note`]). A mixer sentence with nothing moved and nothing
/// left to name, and a grade sentence (with its ceiling's) at a neutral
/// grade, are withdrawn.
fn restate_notes(rationale: &mut String, notes: &mut Vec<crate::rationale::Note>, recipe: &EditRecipe) {
    use crate::rationale::{keys, render_one};
    let grade = colour_grade_note(recipe);
    let mut restated = Vec::with_capacity(notes.len());
    for note in std::mem::take(notes) {
        let next = if note.key == keys::FIT_NOTE_HSL_BANDS {
            let refused = note.args.iter().find(|(k, _)| *k == "refused").map_or("", |(_, v)| v.as_str());
            mixer_note(recipe, refused)
        } else if note.key == keys::FIT_NOTE_COLOR_GRADE {
            grade.clone()
        } else if note.key == keys::FIT_NOTE_COLOR_GRADE_HELD && grade.is_none() {
            None
        } else {
            Some(note.clone())
        };
        let (was, is) = (render_one(&note), next.as_ref().map(render_one).unwrap_or_default());
        if was != is && let Some(at) = rationale.find(&was) {
            rationale.replace_range(at..at + was.len(), &is);
        }
        restated.extend(next);
    }
    *notes = restated;
}

impl FitReport {
    /// A reading of J* taken after the report was composed — the terminal
    /// full-size one — becomes the report's residual: the confidence is read
    /// from it under the caps the report was composed with, and the
    /// far-from-target note is keyed on it (review D D-2: until v1.18.0 the
    /// report printed one J* and claimed a confidence derived from another).
    pub fn set_terminal_reading(&mut self, err_after: f32) {
        use crate::rationale::{Note, keys};
        self.err_after = err_after;
        self.recipe.confidence = confidence_from_look_err(err_after).min(self.confidence_cap);
        self.recipe.clamp();
        crate::rationale::withdraw_notes(&mut self.recipe.rationale, &mut self.notes, |n| n.key == keys::FIT_NOTE_FAR);
        if err_after > FIT_FAR_ERR {
            crate::rationale::push_note(
                &mut self.recipe.rationale,
                &mut self.notes,
                Note::new(keys::FIT_NOTE_FAR, vec![("err_after", format!("{err_after:.2}"))]),
            );
        }
    }

    /// The mixer's and the grade's sentences stated again from the recipe
    /// (v1.18.0, J135): the terminal re-read may set a band value or a wheel
    /// back after the report was composed
    /// (`pipeline::reread_at_full_size`, [`super::confirm_at_export`]), and
    /// every sentence about a control is derived from the recipe that ships.
    pub(crate) fn restate_mixer_and_grade(&mut self) {
        let mut rationale = std::mem::take(&mut self.recipe.rationale);
        restate_notes(&mut rationale, &mut self.notes, &self.recipe);
        self.recipe.rationale = rationale;
    }

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

#[cfg(test)]
mod restatement {
    use super::*;
    use crate::rationale::{Note, keys, push_note, render_one};

    /// J135. A report composed for a recipe with two band values and a grade,
    /// after one band value and the grade were set back: the mixer's sentence
    /// names the band that is left (its refusals unchanged), the grade's
    /// sentence and its ceiling's are gone, and every other sentence stands
    /// byte for byte — in the notes and in the rendered rationale alike. A
    /// band the stage refused and the recipe moves is stated once, under
    /// what moved, and is listed as left neutral again only when the recipe
    /// leaves it so. MUTATION: restate from the composed arguments instead of
    /// the recipe; list every refused band whatever the recipe carries.
    #[test]
    fn the_mixer_and_grade_sentences_follow_the_recipe() {
        let mut recipe = EditRecipe::default();
        recipe.hsl.hue[5] = 20.0;
        recipe.hsl.saturation[4] = 0.7;
        recipe.color_grade.shadow_sat = 12.0;
        let (mut rationale, mut notes) = (String::from("Summary."), Vec::new());
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_JOINT_NONE));
        let composed = mixer_note(&recipe, "Red (one-sided), Aqua (one-sided)").expect("two band values");
        assert_eq!(composed.args[1], ("refused", "Red (one-sided)".to_string()), "Aqua is moved, not left neutral");
        push_note(&mut rationale, &mut notes, composed);
        push_note(&mut rationale, &mut notes, colour_grade_note(&recipe).expect("a grade off neutral"));
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(keys::FIT_NOTE_COLOR_GRADE_HELD, vec![("ceiling", "10".into()), ("asked", "12".into())]),
        );
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_NOT_SAME_FRAME));
        assert!(rationale.contains("Aqua sat +1") && rationale.contains("Blue hue +20"), "premise: {rationale}");

        recipe.hsl.saturation[4] = 0.0;
        recipe.color_grade = Default::default();
        restate_notes(&mut rationale, &mut notes, &recipe);

        let keys_left: Vec<&str> = notes.iter().map(|n| n.key).collect();
        assert_eq!(keys_left, vec![keys::FIT_NOTE_JOINT_NONE, keys::FIT_NOTE_HSL_BANDS, keys::FIT_NOTE_NOT_SAME_FRAME]);
        let mixer = &notes[1];
        assert_eq!(mixer.args, vec![("moved", "Blue hue +20 sat +0 lum +0".to_string()), ("refused", "Red (one-sided)".to_string())]);
        let rendered: String = std::iter::once("Summary.".to_string()).chain(notes.iter().map(render_one)).collect();
        assert_eq!(rationale, rendered, "the rationale is its notes, in order");
        assert!(!rationale.contains("Aqua"), "{rationale}");

        // Nothing moved and nothing refused: the mixer's sentence is withdrawn.
        let (mut rationale, mut notes) = (String::new(), Vec::new());
        push_note(&mut rationale, &mut notes, Note::new(keys::FIT_NOTE_HSL_BANDS, vec![("moved", "Aqua sat +1 lum +0".into()), ("refused", "none".into())]));
        restate_notes(&mut rationale, &mut notes, &EditRecipe::default());
        assert!(notes.is_empty() && rationale.is_empty(), "{rationale:?}");
        assert!(mixer_note(&EditRecipe::default(), "").is_none() && mixer_note(&EditRecipe::default(), "none").is_none());
    }
}
