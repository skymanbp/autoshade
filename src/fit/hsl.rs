//! The HSL stage: band refusals and withdrawals, the halved bands, and the stage itself.

use super::*;

// --------------------------------------------------------------------------
// per-band colour mixer (stage 4a)
// --------------------------------------------------------------------------

/// Why one colour band was left neutral. Typed, because "this band could not
/// be measured" and "this band already matched" are different claims and must
/// never reach the user as the same sentence — the standing evidence rule
/// (one-sided is UNMEASURABLE, not equal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HslBandRefusal {
    /// Populated on exactly one side.
    OneSided,
    /// Under the population line on both sides: nothing to measure.
    Sparse,
    /// Populated on both sides, but the structural evidence did not survive,
    /// so the two populations are not testimony about the same content.
    Divergent,
}

impl HslBandRefusal {
    fn label(self) -> &'static str {
        match self {
            HslBandRefusal::OneSided => "one-sided",
            HslBandRefusal::Sparse => "sparse on both sides",
            HslBandRefusal::Divergent => "structurally divergent",
        }
    }
}

/// The per-band stage gave back everything it fitted, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HslWithdrawal {
    /// The composed frame did not end closer to the target.
    Error,
    /// It would have carried pixels through hue bands no evidence covers.
    Blind,
}

/// What the per-band stage decided, for the disclosure. What it MOVED is not
/// here on purpose: that is a property of the recipe [`compose_report`] is
/// holding, so it is read off the recipe there and cannot go stale when a
/// later do-no-harm loop shrinks the mixer (or resets the whole recipe).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct HslStageFacts {
    /// Bands the frozen evidence called one-sided that the CURRENT render and
    /// the target's own cells between them admitted (R33 §E). Named, because
    /// "this band was unmeasurable" and "this band became measurable once the
    /// light was corrected" are different claims about the same band.
    pub(super) vouched: String,
    /// Bands the two-sided population gate refused, each with its reason.
    pub(super) refused: String,
    pub(super) withdrawn: Option<HslWithdrawal>,
}

/// Largest single-iteration step of the per-band chase, mirroring the global
/// chroma chase's own cap: the ratio is read through a chroma-gated renderer,
/// so one iteration must not be allowed to swing a band across the axis.
const HSL_BAND_STEP: f32 = 40.0;
/// A band mean below this carries no usable ratio — dividing by it turns
/// renderer rounding into a full-scale demand.
const HSL_BAND_MIN_MEAN: f64 = 0.02;

/// One shrink step of the mixer: halve every axis, and snap a band under one
/// unit to neutral so the shrink always REACHES zero instead of approaching
/// it (the same "below 4, go to zero" device the saturation loop uses).
pub(super) fn halved_hsl(hsl: &crate::recipe::Hsl) -> crate::recipe::Hsl {
    let axis = |values: &[f32; 8]| -> [f32; 8] {
        std::array::from_fn(|i| {
            let v = round1(values[i] * 0.5);
            if v.abs() < 1.0 { 0.0 } else { v }
        })
    };
    crate::recipe::Hsl {
        hue: hsl.hue,
        saturation: axis(&hsl.saturation),
        luminance: axis(&hsl.luminance),
    }
}

/// Solve `hsl.saturation` and `hsl.luminance` from POPULATION statistics, one
/// ACR band at a time.
///
/// WHY this is legitimate where a per-band HUE rotation is not, and why it is
/// legitimate against a target whose pixels do not correspond: it is the
/// per-band form of the argument that already lets Atmosphere mode fit one
/// global saturation and one white balance. A band's mean chroma and mean
/// lightness are marginal statistics of a sub-population; matching them needs
/// no pixel pairing, only the claim that both frames' members of that band
/// are measurements of the same subject. That claim is exactly what the
/// evidence model already adjudicates — so this stage asks it, and asks it
/// with the SAME criterion the unrepresented-controls disclosure reads
/// (`evidence.hue[band].weight > 0` plus a two-sided [`EVIDENCE_MIN_SHARE`]
/// of the chromatic mass). One gate, two consumers; a band either side cannot
/// see is left at zero and NAMED, never silently read as "equal".
///
/// `hsl.hue` is never written. Rotating a band re-populates it, so its own
/// evidence is circular (project memory: "the hue evidence is circular"), and
/// it is the axis that turned brown rock olive in the 2026-07-07 failure.
///
/// The chase is closed-loop through the real engine for the same reason the
/// global chroma chase is: `apply_hsl` runs before saturation, the wheels and
/// clarity, blends two bands per pixel through a partition of unity and fades
/// itself out below chroma 0.22 — so the open-loop ratio is a first step, not
/// an answer. Two iterations, then a do-no-harm that shrinks to zero.
#[allow(clippy::too_many_arguments)]
pub(super) fn fit_hsl_stage(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<&crate::fit_cells::PairedCells>,
    budget: FitBudget,
    recipe: &mut EditRecipe,
) -> HslStageFacts {
    let mut facts = HslStageFacts::default();
    let restore = recipe.hsl.clone();
    let before_px = pixels_of(&render::develop_preview(s_img, recipe));
    let err_before = look_err_with_evidence(&before_px, tp, evidence);
    // Whether the recipe ALREADY moved pixels through unmeasured bands is not
    // this stage's fault and not this stage's to fix (the pipeline-end loop
    // owns that case); only movement this stage ADDS is its own.
    let blind_before =
        // R34 §D3 deliberately stops here: the mixer has its own cell
        // admission (`one_sided_vouch` below), asked of the BAND's population
        // rather than the frame's, and two cell verdicts on one stage would be
        // two instruments answering one question.
        moved_unsupported_hue_range_names_vouched(sp, &before_px, evidence, vouch, None).is_some();

    // --- admission --------------------------------------------------------
    let (sa, ta) = band_stats_weighted(&before_px, &evidence.source_hue_weights);
    let (sb, tb) = band_stats_weighted(tp, &evidence.target_hue_weights);
    let mut admitted = [false; EVIDENCE_HUE_BANDS];
    let mut refused: Vec<String> = Vec::new();
    let mut vouched: Vec<String> = Vec::new();
    // R33 §E. A band the FROZEN evidence calls one-sided was one-sided on the
    // pair as it arrived — and the stages before this one have since moved the
    // frame. The sky that carried no Orange because a neutral develop has no
    // chroma at all carries Orange now, on the render this stage is looking
    // at, because the white balance and the tone solve put it there.
    //
    // That is exactly the laundering this crate refuses by default: an edit
    // must not create its own evidence. The ONE thing that makes it honest is
    // the target's own verdict on the pixels the edit created — so the band is
    // admitted only when it is two-sided ON THE CURRENT RENDER *and* the cells
    // holding those members say the solve so far took them toward the target.
    // Without cells, or without that verdict, the band is refused by name
    // exactly as before.
    let population = evidence.population.max(1.0);
    let band_share = |px: &[[f32; 3]], band: usize| {
        px.iter()
            .enumerate()
            .filter(|(_, p)| evidence_hue_band(p) == Some(band))
            .map(|(i, _)| evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0))
            .sum::<f32>()
            / population
    };
    let one_sided_vouch = |band: usize| -> Option<crate::fit_cells::CellVouch> {
        let cells = cells?;
        if band_share(&before_px, band) < EVIDENCE_MIN_SHARE
            || band_share(tp, band) < EVIDENCE_MIN_SHARE
        {
            return None;
        }
        let region: Vec<f32> = before_px
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if evidence_hue_band(p) == Some(band) {
                    evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0)
                } else {
                    0.0
                }
            })
            .collect();
        let verdict = cells.vouch(sp, &before_px, Some(&region));
        verdict.vouched().then_some(verdict)
    };
    for band in 0..EVIDENCE_HUE_BANDS {
        let Some(range) = evidence.hue.get(band) else { continue };
        let verdict = if !range.source_populated && !range.target_populated {
            Some(HslBandRefusal::Sparse)
        } else if !range.source_populated || !range.target_populated {
            Some(HslBandRefusal::OneSided)
        } else if range.weight <= 0.0 {
            Some(HslBandRefusal::Divergent)
        } else if ta < 1.0 || tb < 1.0 {
            Some(HslBandRefusal::Sparse)
        } else {
            let source_ok = sa[band].w / ta >= EVIDENCE_MIN_SHARE as f64;
            let target_ok = sb[band].w / tb >= EVIDENCE_MIN_SHARE as f64;
            match (source_ok, target_ok) {
                (true, true) => None,
                (false, false) => Some(HslBandRefusal::Sparse),
                _ => Some(HslBandRefusal::OneSided),
            }
        };
        match verdict {
            None => admitted[band] = true,
            Some(HslBandRefusal::OneSided) if one_sided_vouch(band).is_some() => {
                admitted[band] = true;
                vouched.push(range.label.clone());
            }
            Some(reason) => {
                // Only bands the PICTURE actually holds are worth naming: a
                // band absent from both frames is not a refusal anyone can
                // act on, and listing all eight on a grey frame would bury
                // the ones that mean something.
                if range.source_populated || range.target_populated {
                    refused.push(format!("{} ({})", range.label, reason.label()));
                }
            }
        }
    }
    facts.refused = refused.join(", ");
    facts.vouched = vouched.join(", ");
    if !admitted.iter().any(|&band| band) {
        return facts;
    }

    // --- the chase, closed-loop through the real engine --------------------
    for _ in 0..2 {
        let cur = pixels_of(&render::develop_preview(s_img, recipe));
        let (cs, _) = band_stats_weighted(&cur, &evidence.source_hue_weights);
        let mut moved_any = false;
        for band in 0..EVIDENCE_HUE_BANDS {
            if !admitted[band] || cs[band].w <= 0.0 || sb[band].w <= 0.0 {
                continue;
            }
            // The engine reads `new_s = s * (1 + sat/100)` and
            // `new_l = l * (1 + 0.5 * lum/100)`, and chroma is proportional to
            // `s` and luma to `l` at fixed hue, so a band's mean-chroma ratio
            // IS the saturation demand and its mean-luma ratio is the
            // luminance demand at half the sensitivity.
            let axes = [
                (cs[band].c / cs[band].w, sb[band].c / sb[band].w, 100.0f32),
                (cs[band].y / cs[band].w, sb[band].y / sb[band].w, 200.0f32),
            ];
            for (axis, (now, want, scale)) in axes.into_iter().enumerate() {
                if now < HSL_BAND_MIN_MEAN {
                    continue;
                }
                let step = (((want / now) - 1.0) as f32 * scale)
                    .clamp(-HSL_BAND_STEP, HSL_BAND_STEP);
                if step.abs() < 1.0 {
                    continue;
                }
                let slot = if axis == 0 {
                    &mut recipe.hsl.saturation[band]
                } else {
                    &mut recipe.hsl.luminance[band]
                };
                let next = round1((*slot + step).clamp(-budget.hsl_band, budget.hsl_band));
                if next != *slot {
                    moved_any = true;
                }
                *slot = next;
            }
        }
        if !moved_any {
            break;
        }
    }
    let fitted = recipe.hsl.clone();
    if fitted == restore {
        return facts;
    }

    // --- do-no-harm, the stage's own --------------------------------------
    // The same err_before/err_after discipline the saturation pull-back
    // answers to, applied where the move is generated instead of three stages
    // later: halve the whole vector until the frame's look error stops
    // objecting AND the finished render stops carrying pixels through hue
    // bands this stage's own gate never measured. Withdrawal is always
    // reachable: at neutral, or at the round bound.
    let verdict = halve_mixer_until_harmless(fitted, |candidate| {
        recipe.hsl = candidate.clone();
        let px = pixels_of(&render::develop_preview(s_img, recipe));
        let regressed = look_err_with_evidence(&px, tp, evidence) > err_before + 1e-4;
        let blind_new = budget.vetoes == VetoPolicy::Withhold
            && !blind_before
            && moved_unsupported_hue_range_names_vouched(sp, &px, evidence, vouch, None)
                .is_some();
        if blind_new {
            Some(HslWithdrawal::Blind)
        } else if regressed {
            Some(HslWithdrawal::Error)
        } else {
            None
        }
    });
    match verdict {
        Ok(kept) => recipe.hsl = kept,
        Err(reason) => {
            recipe.hsl = restore;
            facts.withdrawn = Some(reason);
        }
    }
    facts
}

/// The stage's do-no-harm control, separated from its measurements so its
/// termination is testable: halve `fitted` while `objects` names a reason,
/// and hand back the first candidate it does not object to. `Err` carries
/// the last objection when the halving reaches neutral, or when
/// [`super::solve::DO_NO_HARM_MAX_ROUNDS`] candidates have been judged —
/// the 4b loop's bound, for the same reason: a band at the budget ceiling
/// reaches neutral in eight halvings, so a mixer that needs more is not
/// descending. The one that cannot is a mixer carrying a hue rotation from
/// the base recipe, which `halved_hsl` keeps and `Hsl::is_neutral` reads —
/// a fixed point of the halving, every round rendering the same frame.
/// Either way the caller withdraws to the state the stage started from.
pub(super) fn halve_mixer_until_harmless(
    fitted: crate::recipe::Hsl,
    mut objects: impl FnMut(&crate::recipe::Hsl) -> Option<HslWithdrawal>,
) -> Result<crate::recipe::Hsl, HslWithdrawal> {
    let mut reason: Option<HslWithdrawal> = None;
    let mut candidate = fitted;
    let mut rounds = 0;
    loop {
        if candidate.is_neutral() || rounds == super::solve::DO_NO_HARM_MAX_ROUNDS {
            return Err(reason.unwrap_or(HslWithdrawal::Error));
        }
        rounds += 1;
        match objects(&candidate) {
            None => return Ok(candidate),
            Some(why) => reason = Some(why),
        }
        candidate = halved_hsl(&candidate);
    }
}
