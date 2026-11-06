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

/// Tolerance for the fit's OWN quantisation, not a fixed error size: the
/// objective (J*) of an IDENTICAL pair after the fit's own rounding. Measured
/// by that definition (v1.18.0, `self_consistency::
/// fit_quant_is_the_objective_of_the_fits_own_rounding`): the app's render of
/// a corpus RAW's straight conversion with every control the global fit
/// stores displaced by half its storage step in one direction (exposure
/// 0.005 EV, the tone sliders, saturation, vibrance, tint and the 24 mixer
/// values 0.05, white balance 25 K), read against the straight conversion
/// through the fit's own preview develop, every present pixel judged at its
/// presence weight (R11): p38 0.001397 / 0.001841, p39 0.001093 / 0.001109,
/// p40 0.000614 / 0.000533, p41 0.002250 / 0.002132 (+ / -). The constant is
/// the smallest two-figure value not below the largest reading, 0.002250 —
/// the rule that gave 1.8e-3 from 0.001786 when J* read the estimation
/// weights, which left p41's sky ranges out of the reading. Since
/// v1.18.0 it is also the ONE margin every do-no-harm of the fit reads (the
/// nine bare 1e-4 margins, the tone rulers' 1e-4, the bands' end state, the
/// zones' frame allowance): a difference the objective cannot tell from its
/// own rounding is no difference. A bare +1e-4 margin used to fire on an
/// identical pair's rounding, wiping a perfectly good near-neutral solve and
/// reporting "outside the global model's reach" directly beneath a printed
/// residual of 0.000 -> 0.000.
///
/// A flat FLOOR is the wrong correction though: it would also wave through a
/// fit that is genuinely worse whenever both numbers are small (err_before
/// 0.0010 -> err_after 0.0029 is nearly 3× worse, and no absolute floor below
/// 0.003 catches it). Scale with the error instead and add the quantisation
/// budget once.
///
/// What FIT_QUANT does NOT hold is the commutation error: the search previews
/// a candidate at the analysis size on a plate clamped where the export clamps
/// (the zero point exact), while the export renders the candidate and then
/// resamples, and clamp-then-average differs from average-then-clamp wherever
/// a window holds clipped and unclipped pixels. Measured at the finished
/// recipes of the seven corpus fits on the clamped plate (v1.18.0 R13b /
/// R14, `self_consistency::
/// the_preview_of_a_finished_recipe_against_its_resampled_export`): 0.0032 to
/// 0.0214 of J*, 1.4x to 9.3x this constant. That gap is why the terminal arm
/// re-reads the finished recipe at full size and ships on that reading
/// (`pipeline::reread_at_full_size`); it is a property of the recipe, not of
/// the fit's rounding, and is not folded in here.
pub(crate) const FIT_QUANT: f32 = 2.3e-3;

/// THE stage-vs-absence law (v1.18.0, R1): a stage SHIPS only when its
/// objective gain over its absence is at least [`FIT_QUANT`]. A gain under
/// FIT_QUANT is no difference after the fit's own rounding, and no difference
/// goes to the untouched, simpler recipe; a loss of any size refuses. Every
/// stage-vs-absence decision calls this — the mixer (4a'), the band hue (4a),
/// the 4b step-down, the cast curves and their hand-off, the Atmosphere
/// colour stage, the terminal arm, and the local layers (zones, bands, tint,
/// tiles, free masks, ranges, the colour field) — so the stages' allowances
/// cannot add: each one that ships has earned FIT_QUANT. `with` / `without`
/// are the two frames' readings on one ruler.
pub(crate) fn ships(with: f32, without: f32) -> bool {
    without - with >= FIT_QUANT
}

/// R19 (v1.18.0), the LOCAL form of [`ships`]: a local correction (zone,
/// tile, band set, range set, zone tint) ships when it earns [`FIT_QUANT`]
/// on its OWN population (`own_with` / `own_without`: the objective read over
/// the region's own present pixels and bands), and the frame is a guard
/// only — the correction may not raise the frame's reading by more than
/// FIT_QUANT. FIT_QUANT is the rounding of a per-pixel mean, so it reads the
/// same on a region's pixels as on the frame's; read on the frame, a
/// region's gain is divided by its share.
pub(crate) fn ships_local(own_with: f32, own_without: f32, frame_with: f32, frame_without: f32) -> bool {
    ships(own_with, own_without) && frame_with - frame_without <= FIT_QUANT
}

/// R19's population on the HUE axis (rel1180 RULING-1180-sc-r1 Q-SC-3): the
/// pixels the `moving` mixer bands reach on `px` — each pixel's partition
/// weight in the moving bands ([`render::hsl_band_reach`], the engine's own
/// reach), times its membership.
pub(crate) fn mixer_reach(px: &[[f32; 3]], moving: &[bool; 8], membership: &[f32]) -> Vec<f32> {
    px.iter()
        .enumerate()
        .map(|(i, p)| {
            let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
            let within = match render::hsl_band_reach(h, render::chroma(p)) {
                Some((b0, b1, w1, _)) => {
                    (if moving[b0] { 1.0 - w1 } else { 0.0 }) + if moving[b1] { w1 } else { 0.0 }
                }
                None => 0.0,
            };
            within * membership.get(i).copied().unwrap_or(0.0).max(0.0)
        })
        .collect()
}

/// THE MIXER'S SHIP LAW (rel1180 RULING-1180-sc-r1 Q-SC-3, RULING-1180-sc-r4
/// Q-SC-2b (2)): a mixer move ships when it earns [`FIT_QUANT`] on its OWN
/// bands' population ([`mixer_reach`] of the `moving` bands on `without_px`,
/// J* read on [`EvidenceModel::scoped`] to it), the frame a guard only —
/// R19's local law ([`ships_local`]) on the hue axis. Both readings are a
/// local correction's (S-mode, [`super::look::LookTerms::local`]): the
/// structural term is the D threshold's, and read on the frame it moved by
/// up to 0.0087 of J* for a band move whose own population halved its error
/// (rel1180 msg-sc round 5, the engine HSL pair). No band reached, or no
/// band moving: the frame's law.
pub(crate) fn mixer_ships(
    moving: &[bool; 8],
    with_px: &[[f32; 3]],
    without_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    let (frame_with, frame_without) =
        (look_err_with_evidence(with_px, tp, evidence), look_err_with_evidence(without_px, tp, evidence));
    let reach = mixer_reach(without_px, moving, &evidence.source_membership);
    if !moving.iter().any(|&m| m) || reach.iter().sum::<f32>() <= 0.0 {
        return ships(frame_with, frame_without);
    }
    let mut own = evidence.scoped(tp, &reach, &reach);
    own.local = true;
    let frame_local = |px: &[[f32; 3]]| super::look::look_terms(px, tp, evidence).local();
    ships_local(
        look_err_with_evidence(with_px, tp, &own),
        look_err_with_evidence(without_px, tp, &own),
        frame_local(with_px),
        frame_local(without_px),
    )
}

/// One recipe's readings for R22 (a)'s verdict: frame J*, `E_paired` per
/// luminance band (J* units, [`look::LumaBands::paired_readings`]), and the
/// render's L* per pixel (for the control's own response).
pub(super) type BandReading = (f32, Vec<f32>, Vec<f32>);

/// R22 (a): "the band a global control earns on is the band its own curve
/// acts on". A control that earns FIT_QUANT on the frame ships. Otherwise
/// only the four tone sliders, which act on a luminance band by construction
/// (whites / highlights the top of the L* range, blacks / shadows the
/// bottom), may earn on ONE band: the band their own response weights most
/// ([`look::LumaBands::acting_band`], fixed before any error is read), with
/// the frame a guard only ([`ships_local`]). Contrast, exposure and the tone
/// curve act on the frame; saturation, the mixer and the per-band controls
/// act on hue / chroma bands, which H_ab reads on the colour side.
pub(super) fn earns_on_its_band(
    c: Control, bands: &look::LumaBands, solved: &BandReading, without: &BandReading,
) -> bool {
    if ships(solved.0, without.0) {
        return true;
    }
    if !matches!(c, Control::Whites | Control::Highlights | Control::Shadows | Control::Blacks) {
        return false;
    }
    bands.acting_band(&solved.2, &without.2).is_some_and(|k| match (solved.1.get(k), without.1.get(k)) {
        (Some(&with), Some(&own_without)) => ships_local(with, own_without, solved.0, without.0),
        _ => false,
    })
}

/// One control a global solve sets, as [`eliminate_unearned`] reads it: a
/// slider, one white-balance axis, one mixer value, one grade wheel, one
/// curve. Its "zero" is the value the solve started from (`base`, the
/// calibration-only recipe), which is the slider's own zero on a photograph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Control {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Temperature,
    Tint,
    Vibrance,
    Saturation,
    HslHue(usize),
    HslSaturation(usize),
    HslLuminance(usize),
    GradeShadows,
    GradeMidtones,
    GradeHighlights,
    GradeGlobal,
    ToneCurve,
    RedCurve,
    GreenCurve,
    BlueCurve,
}

/// The tone stage's joint solve.
pub(super) const TONE_SLIDERS: [Control; 6] = [
    Control::Exposure,
    Control::Contrast,
    Control::Highlights,
    Control::Shadows,
    Control::Whites,
    Control::Blacks,
];

/// The white balance's joint solve: its two axes.
pub(super) const WB_AXES: [Control; 2] = [Control::Temperature, Control::Tint];

/// The colour grade's joint solve: its four wheels.
pub(super) const GRADE_WHEELS: [Control; 4] =
    [Control::GradeShadows, Control::GradeMidtones, Control::GradeHighlights, Control::GradeGlobal];

/// The cast stage's joint solve: the three channel curves.
pub(super) const CAST_CURVES: [Control; 3] = [Control::RedCurve, Control::GreenCurve, Control::BlueCurve];

/// The mixer's joint solve: the 24 band values.
pub(super) fn mixer_controls() -> Vec<Control> {
    (0..8).flat_map(|band| [Control::HslHue(band), Control::HslSaturation(band), Control::HslLuminance(band)]).collect()
}

/// Every control the finished global recipe can carry whose effect the
/// objective reads. The detail controls (clarity, texture) are not here:
/// their gain is read by their own ruler (`detail::detail_residual`), which
/// J* does not carry, so J* would call every one of them unearned.
pub(super) fn global_controls() -> Vec<Control> {
    let mut all = vec![
        Control::Exposure,
        Control::Contrast,
        Control::Highlights,
        Control::Shadows,
        Control::Whites,
        Control::Blacks,
        Control::Temperature,
        Control::Tint,
        Control::Vibrance,
        Control::Saturation,
        Control::GradeShadows,
        Control::GradeMidtones,
        Control::GradeHighlights,
        Control::GradeGlobal,
        Control::ToneCurve,
        Control::RedCurve,
        Control::GreenCurve,
        Control::BlueCurve,
    ];
    all.extend(mixer_controls());
    all
}

impl Control {
    pub(super) fn name(self) -> String {
        let band = |b: usize| crate::recipe::HSL_BANDS[b];
        match self {
            Control::Exposure => "exposure".into(),
            Control::Contrast => "contrast".into(),
            Control::Highlights => "highlights".into(),
            Control::Shadows => "shadows".into(),
            Control::Whites => "whites".into(),
            Control::Blacks => "blacks".into(),
            Control::Temperature => "temperature".into(),
            Control::Tint => "tint".into(),
            Control::Vibrance => "vibrance".into(),
            Control::Saturation => "saturation".into(),
            Control::HslHue(b) => format!("{} hue", band(b)),
            Control::HslSaturation(b) => format!("{} saturation", band(b)),
            Control::HslLuminance(b) => format!("{} luminance", band(b)),
            Control::GradeShadows => "grade shadows".into(),
            Control::GradeMidtones => "grade midtones".into(),
            Control::GradeHighlights => "grade highlights".into(),
            Control::GradeGlobal => "grade global".into(),
            Control::ToneCurve => "tone curve".into(),
            Control::RedCurve => "red curve".into(),
            Control::GreenCurve => "green curve".into(),
            Control::BlueCurve => "blue curve".into(),
        }
    }

    pub(super) fn differs(self, r: &EditRecipe, b: &EditRecipe) -> bool {
        let (g, h) = (&r.color_grade, &b.color_grade);
        match self {
            Control::Exposure => r.exposure_ev != b.exposure_ev,
            Control::Contrast => r.contrast != b.contrast,
            Control::Highlights => r.highlights != b.highlights,
            Control::Shadows => r.shadows != b.shadows,
            Control::Whites => r.whites != b.whites,
            Control::Blacks => r.blacks != b.blacks,
            Control::Temperature => r.temperature_k != b.temperature_k,
            Control::Tint => r.tint != b.tint,
            Control::Vibrance => r.vibrance != b.vibrance,
            Control::Saturation => r.saturation != b.saturation,
            Control::HslHue(i) => r.hsl.hue[i] != b.hsl.hue[i],
            Control::HslSaturation(i) => r.hsl.saturation[i] != b.hsl.saturation[i],
            Control::HslLuminance(i) => r.hsl.luminance[i] != b.hsl.luminance[i],
            Control::GradeShadows => {
                (g.shadow_hue, g.shadow_sat, g.shadow_lum) != (h.shadow_hue, h.shadow_sat, h.shadow_lum)
            }
            Control::GradeMidtones => {
                (g.midtone_hue, g.midtone_sat, g.midtone_lum) != (h.midtone_hue, h.midtone_sat, h.midtone_lum)
            }
            Control::GradeHighlights => {
                (g.highlight_hue, g.highlight_sat, g.highlight_lum)
                    != (h.highlight_hue, h.highlight_sat, h.highlight_lum)
            }
            Control::GradeGlobal => {
                (g.global_hue, g.global_sat, g.global_lum) != (h.global_hue, h.global_sat, h.global_lum)
            }
            Control::ToneCurve => r.tone_curve != b.tone_curve,
            Control::RedCurve => r.red_curve != b.red_curve,
            Control::GreenCurve => r.green_curve != b.green_curve,
            Control::BlueCurve => r.blue_curve != b.blue_curve,
        }
    }

    pub(super) fn reset(self, r: &mut EditRecipe, b: &EditRecipe) {
        let h = &b.color_grade;
        let g = &mut r.color_grade;
        match self {
            Control::Exposure => r.exposure_ev = b.exposure_ev,
            Control::Contrast => r.contrast = b.contrast,
            Control::Highlights => r.highlights = b.highlights,
            Control::Shadows => r.shadows = b.shadows,
            Control::Whites => r.whites = b.whites,
            Control::Blacks => r.blacks = b.blacks,
            Control::Temperature => r.temperature_k = b.temperature_k,
            Control::Tint => r.tint = b.tint,
            Control::Vibrance => r.vibrance = b.vibrance,
            Control::Saturation => r.saturation = b.saturation,
            Control::HslHue(i) => r.hsl.hue[i] = b.hsl.hue[i],
            Control::HslSaturation(i) => r.hsl.saturation[i] = b.hsl.saturation[i],
            Control::HslLuminance(i) => r.hsl.luminance[i] = b.hsl.luminance[i],
            Control::GradeShadows => {
                (g.shadow_hue, g.shadow_sat, g.shadow_lum) = (h.shadow_hue, h.shadow_sat, h.shadow_lum)
            }
            Control::GradeMidtones => {
                (g.midtone_hue, g.midtone_sat, g.midtone_lum) = (h.midtone_hue, h.midtone_sat, h.midtone_lum)
            }
            Control::GradeHighlights => {
                (g.highlight_hue, g.highlight_sat, g.highlight_lum) =
                    (h.highlight_hue, h.highlight_sat, h.highlight_lum)
            }
            Control::GradeGlobal => {
                (g.global_hue, g.global_sat, g.global_lum) = (h.global_hue, h.global_sat, h.global_lum)
            }
            Control::ToneCurve => r.tone_curve = b.tone_curve.clone(),
            Control::RedCurve => r.red_curve = b.red_curve.clone(),
            Control::GreenCurve => r.green_curve = b.green_curve.clone(),
            Control::BlueCurve => r.blue_curve = b.blue_curve.clone(),
        }
    }
}

/// [`ships`] taken control by control (v1.18.0, R8). A joint solve moves
/// several controls at once, and a control that earns nothing rides out on
/// the ones that do: an exposure-only target came back with the exposure
/// exact and saturation 1.6 / whites 0.4 beside it. So after a joint solve
/// every control that differs from `base` is set back to it, one at a time in
/// order of smallest effect (the smallest reading without it first), and
/// stays there when the recipe without it does not lose FIT_QUANT against the
/// recipe the solve left: the control has to ship against its own absence,
/// the same law, and a tie goes to the simpler recipe. The loss is read
/// against the solved recipe, not against the last drop, so the drops cannot
/// add up past FIT_QUANT (R1: allowances never add). Repeated until a pass
/// drops nothing. `err` is the stage's own ruler. Returns what was dropped.
///
/// It runs after EVERY joint solve (R12) — the white balance's two axes, the
/// tone sliders, the grade wheels, the mixer's bands, the cast curves (with
/// the white balance when the cast→WB hand-off moved it) — and once more on
/// the finished recipe: a control that earns nothing must not be built upon
/// by a later stage, which would otherwise solve from a state that includes
/// it. One law, this function, at every site.
pub(super) fn eliminate_unearned(
    recipe: &mut EditRecipe,
    base: &EditRecipe,
    controls: &[Control],
    err: impl Fn(&EditRecipe) -> f32,
) -> Vec<Control> {
    eliminate_unearned_by(recipe, base, controls, err, |r| *r, |_, solved, without| ships(*solved, *without))
}

/// [`eliminate_unearned`] with the reading and the verdict apart, for the
/// sites whose control ships on a population of its own (R19's family): an
/// Atmosphere white balance on the shared content it was solved from
/// (DEF-2), a global control on the luminance band it acts on (R22). `read`
/// takes every reading a verdict needs once per recipe, `order` is the frame
/// reading the drops are ordered by (smallest effect first, as above), and
/// `earns(c, solved, without)` is the control's ship law. The loop, the
/// order and "read against the solved recipe, never the last drop" are
/// [`eliminate_unearned`]'s, unchanged.
pub(super) fn eliminate_unearned_by<R>(
    recipe: &mut EditRecipe,
    base: &EditRecipe,
    controls: &[Control],
    read: impl Fn(&EditRecipe) -> R,
    order_of: impl Fn(&R) -> f32,
    earns: impl Fn(Control, &R, &R) -> bool,
) -> Vec<Control> {
    let solved = read(recipe);
    let without = |r: &EditRecipe, c: Control| {
        let mut z = r.clone();
        c.reset(&mut z, base);
        z
    };
    let mut dropped = Vec::new();
    loop {
        let mut order: Vec<(R, Control)> = controls
            .iter()
            .copied()
            .filter(|c| c.differs(recipe, base))
            .map(|c| (read(&without(recipe, c)), c))
            .collect();
        order.sort_by(|a, b| order_of(&a.0).total_cmp(&order_of(&b.0)));
        let mut changed = false;
        for (reading, c) in order {
            let candidate = without(recipe, c);
            // The pass's readings were taken on the state they judge; after
            // a drop the state moved, so the next one is read again.
            let e = if changed { read(&candidate) } else { reading };
            if !earns(c, &solved, &e) {
                *recipe = candidate;
                dropped.push(c);
                changed = true;
            }
        }
        if !changed {
            return dropped;
        }
    }
}

/// The names [`eliminate_unearned`] dropped, as the note prints them.
pub(super) fn control_names(dropped: &[Control]) -> Option<String> {
    (!dropped.is_empty()).then(|| dropped.iter().map(|c| c.name()).collect::<Vec<_>>().join(", "))
}

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
        scalar: !ships(err_after, err_before),
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
/// The summary note already says "local masks are not recovered" (and, on
/// the statistical path, "per-band hue rotation") on every fit, which is true
/// and useless: it does not say whether THIS target needed them. The solve domain is a fact
/// about the code — the global arm writes exposure/contrast/highlights/
/// shadows/whites/blacks, a tone curve, one saturation, the per-band mixer's
/// saturation/luminance axes (and, on paired fits, its hue), the grade
/// wheels and three channel curves, and NOTHING in
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
        // `hsl` here means what stage 4a left: it fits a band's saturation
        // and luminance from that band's own population and, on paired fits,
        // its hue from the pixels' own rotation, and this note is read on the
        // residual those moves left behind, so a per-band gap the mixer
        // closed never reaches this line. What survives it is a band
        // rotation the paired solve could not ship (no pairing, past the
        // band budget, or refused by the cells) or a demand the mixer's
        // evidence gate refused.
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
    // A surviving UNIFORM channel-mean offset is the white-balance shape,
    // named only when no white balance shipped (in either mode the white
    // balance is solved and may decline the demand).
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
/// be noise. 0.025 of the display objective, carried into J* by the corpus
/// exchange rate (v1.18.0): on the five Full-mode calibration pairs the SAME
/// base frame reads 1.39 / 2.20 / 1.53 / 2.32 / 1.30 times higher in J*
/// than in the display objective (p36 / p38 / p39 / p40 / p41, `err_before`
/// of the rel1180 legs I8 and C), median 1.53; 0.025 × 1.53 = 0.038.
pub(super) const FIT_QUANT_CLEAN: f32 = 0.038;
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
