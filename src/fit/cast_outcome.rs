//! The cast outcome: admission and projection notes, the gate outcome, terminal harm, colour-shaped residuals and the unrepresented note.

use super::*;

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
pub(super) struct CastReadings {
    /// Weighted look error WITH the curves over the error without them.
    pub(super) ratio: f32,
    /// The bound that ratio was judged against on THIS path —
    /// `budget.cast_ratio`, which the strength budget widens (2.0 at the
    /// shipped default, up to 3.0), not the [`CAST_ACCEPT_RATIO`] anchor.
    pub(super) bound: f32,
    /// Foreign-hue population share the curves CREATE (with − without).
    /// `None` = not measurable: the target carries no hue evidence for a hue
    /// to be foreign TO.
    pub(super) foreign: Option<f32>,
    /// Weighted share of the population re-hued past [`ROT_DEG`].
    pub(super) rehued: f32,
    /// Hue spread the curves ADD inside the widest hue class, in degrees.
    /// SIGNED — the curves can also NARROW a class. `None` = not measurable:
    /// no hue class holds [`FAN_SHARE`] of the census population across two
    /// populated luma slices.
    pub(super) fan: Option<f32>,
}

/// The notes an ADMITTED cast writes: the head, which carries the three
/// readings that are always measured, then one clause for each reading that
/// can ABSTAIN.
///
/// It is a named function and not three inline `push_note`s so the
/// abstention wording is reachable from a test without needing a pair that
/// happens to abstain — the case that shipped the fabricated `0.000` is by
/// construction the rare one.
pub(super) fn cast_admission_notes(r: CastReadings) -> Vec<crate::rationale::Note> {
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
pub(super) struct CastProjection {
    /// Share of the census population held by the class the FITTED curves
    /// would have fanned. A hue population, not a region — see
    /// [`hue_fan_weighted`].
    pub(super) share: f32,
    /// Added spread, in degrees, the FITTED curves would have opened in it.
    pub(super) fan_before: f32,
    /// Where on the projection path the shipped curves sit: 1.0 would be
    /// the fitted cast, 0.5 one curve shared by all three channels, 0.0 no
    /// curves at all. See [`projected_cast_curves`].
    pub(super) t: f32,
    /// Added spread the PROJECTED curves open, measured on their own render.
    /// SIGNED, and `None` = not measurable — the same abstention
    /// [`CastReadings::fan`] carries, for the same reason, and never `0.0`.
    pub(super) fan_after: Option<f32>,
    /// The PROJECTED candidate's look-error ratio and the bound it was judged
    /// against. The projected curves are what ships, so these are its
    /// readings, not the fitted cast's.
    pub(super) ratio: f32,
    pub(super) bound: f32,
    /// The two PIXEL-ALIGNED readings, carried for the same reason the
    /// admission carries them and with more force: these are curves the fit
    /// INVENTED to answer a conviction rather than curves it measured off the
    /// pair, so the readings that say what they did to the frame matter more
    /// here, not less. Same meanings and the same abstention as
    /// [`CastReadings::rehued`] / [`CastReadings::foreign`], measured on the
    /// PROJECTED candidate's own render.
    pub(super) rehued: f32,
    pub(super) foreign: Option<f32>,
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
pub(super) fn cast_projection_notes(p: CastProjection) -> Vec<crate::rationale::Note> {
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
pub(super) struct CastOutcome {
    /// A pixel-aligned hue gate fired (foreign hues, or a region re-hued).
    pub(super) rehue_blocked: bool,
    /// The aggregate ratio refused: the curves did not buy enough.
    pub(super) ratio_rejected: bool,
    /// The fan gate fired: (class share, added spread in degrees). Carries
    /// its readings because the refusal DISCLOSES them.
    pub(super) hue_fanned: Option<(f32, f32)>,
    /// What the gates measured, present whenever curves were actually
    /// fitted and judged. `None` = the stage produced no curves to judge.
    pub(super) readings: Option<CastReadings>,
    /// v1.2.3: the fan gate convicted the FITTED curves and the projection
    /// found a milder cast that clears [`FAN_PROJECT_DEG`] and all four
    /// gates. Then `hue_fanned` is `None` and `readings` describe the
    /// PROJECTED candidate — the one that ships — while this carries what
    /// the fitted curves would have done and how far they were shrunk.
    pub(super) projected: Option<CastProjection>,
}

#[cfg(test)]
pub(super) fn cast_gate_outcome(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> CastOutcome {
    cast_gate_outcome_with_ratio(cur, with_px, tp, evidence, vouch, cells, CAST_ACCEPT_RATIO)
}

/// The panel Strength a report was solved at, recovered from its own notes.
/// `pub(crate)` because the zoned passes run AFTER the global solve and are
/// handed a report rather than the options: the note is the seam, and reading
/// the fact the report states beats threading a second copy of it.
pub(crate) fn carried_strength_from_notes(prior: &[crate::rationale::Note]) -> crate::recipe::GradeStrength {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn cast_gate_outcome_with_ratio(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
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
            || moved_unsupported_hue_range_names_vouched(cur, with_px, evidence, vouch, cells)
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
pub(super) struct TerminalHarm {
    /// The frame-global look error regressed past the fit's own quantisation
    /// budget (R16's rule, unchanged).
    pub(super) scalar: bool,
    /// The joint value-range distributions drifted apart past
    /// [`crate::fit_zoned::JOINT_DRIFT_TOL`] (R23-6's additional veto).
    pub(super) joint: bool,
}

impl TerminalHarm {
    pub(super) fn any(self) -> bool {
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
pub(super) const FIT_QUANT: f32 = 1.8e-3;

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
pub(super) fn terminal_harm(
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
    pub(super) fn note(self) -> Option<crate::rationale::Note> {
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
    pub(super) fn refused(self) -> bool {
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
    pub(super) fn earns_projection(self) -> Option<(f32, f32)> {
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
pub(super) fn residual_is_colour_shaped(
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
pub(super) fn unrepresented_note(
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
pub(super) const FIT_QUANT_CLEAN: f32 = 0.025;
/// Worst populated-band centroid disagreement (degrees) that counts as
/// "this target used a per-band colour move". 20° is well past the ±13.5°
/// the engine's own HSL hue axis can even express, so a gap this size cannot
/// be a rounding artefact of a band the fit did reach.
pub(super) const UNREPRESENTED_HUE_DEG: f32 = 20.0;
/// A chromatic bucket must miss by at least this much before the residual
/// is called colour-shaped. On the fixture set the fits that LAND leave
/// every chromatic bucket under 0.05 (the haze pair's worst is 0.041), while
/// the region-graded canyon leaves 0.098 and the unreachable repaint 0.71.
pub(super) const UNREPRESENTED_CHROMATIC_ERR: f32 = 0.06;
/// …and it must miss by this much MORE than the neutral buckets at the same
/// brightness, or the difference is a tone/exposure gap the solver does
/// address rather than a colour one it cannot.
pub(super) const UNREPRESENTED_CHROMATIC_LEAD: f32 = 0.02;
/// Red-minus-blue mean offset (in 0..1 channel units) that counts as a
/// white-balance-shaped residual. 0.02 ≈ 5/255 across the whole frame —
/// visible as a cast, and an order above the fit's own rounding.
const UNREPRESENTED_WB_RB: f32 = 0.02;
