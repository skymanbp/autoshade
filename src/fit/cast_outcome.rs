//! The fit's quantum and its control catalogue: FIT_QUANT and the ships rules, the band readings, the controls' names and resets, colour-shaped residuals and the unrepresented note.

use super::*;

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

/// Tolerance for the fit's OWN quantisation, not a fixed error size: the
/// objective (J*) of an IDENTICAL pair after the fit's own rounding. Measured
/// by that definition (v1.18.0, `self_consistency::
/// fit_quant_is_the_objective_of_the_fits_own_rounding`): the app's render of
/// a corpus RAW's straight conversion with every control the global fit
/// stores displaced by half its storage step in one direction (exposure
/// 0.005 EV, the tone sliders, saturation, vibrance, tint and the 24 mixer
/// values 0.05, white balance 25 K), read against the straight conversion
/// through the fit's own preview develop, every present pixel judged at its
/// presence weight (R11), on the per-pixel ruler of 2026-11-07: p38
/// 0.001702 / 0.001595, p39 0.001818 / 0.001728, p40 0.001031 / 0.000835,
/// p41 0.001832 / 0.001657 (+ / -). The constant is the smallest two-figure
/// value not below the largest reading, 0.001832 — the rule that gave 2.3e-3
/// from 0.002250 while J* pooled the pixels at σ 2 and scored a hue and a
/// structural term beside the distance, and 1.8e-3 from 0.001786 when J*
/// read the estimation weights, which left p41's sky ranges out of the
/// reading. Since
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
pub(crate) const FIT_QUANT: f32 = 1.9e-3;

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

/// The names the elimination dropped, as the note prints them.
pub(super) fn control_names(dropped: &[Control]) -> Option<String> {
    (!dropped.is_empty()).then(|| dropped.iter().map(|c| c.name()).collect::<Vec<_>>().join(", "))
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
        // common one (the validity law refuses them far more often than it
        // keeps them).
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
