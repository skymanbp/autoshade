//! The HSL stage: band refusals and withdrawals, the halved bands, and the stage itself.

use super::*;
use super::evidence::{EVIDENCE_LUMA_BINS, evidence_luma_bin, wb_consensus, weighted_median};

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

/// What the per-band stage decided, for the disclosure. What it MOVED is not
/// here on purpose: that is a property of the recipe [`compose_report`] is
/// holding, so it is read off the recipe there and cannot go stale when the
/// joint objective moves or zeroes the mixer (a proposed value it zeroed is
/// named by `FIT_NOTE_UNEARNED`, with every other control it set back).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct HslStageFacts {
    /// Bands the frozen evidence called one-sided that the CURRENT render and
    /// the target's own cells between them admitted (R33 §E). Named, because
    /// "this band was unmeasurable" and "this band became measurable once the
    /// light was corrected" are different claims about the same band.
    pub(super) vouched: String,
    /// Bands the two-sided population gate refused, each with its reason.
    pub(super) refused: String,
}

/// Largest single-iteration step of the per-band chase, mirroring the global
/// chroma chase's own cap: the ratio is read through a chroma-gated renderer,
/// so one iteration must not be allowed to swing a band across the axis.
const HSL_BAND_STEP: f32 = 40.0;
/// A band mean below this carries no usable ratio — dividing by it turns
/// renderer rounding into a full-scale demand.
const HSL_BAND_MIN_MEAN: f64 = 0.02;

/// Solve the per-band colour mixer — `hsl.saturation`, `hsl.luminance` and
/// `hsl.hue`, one ACR band at a time — on the bands the population gate
/// admits ([`admit_bands`]).
///
/// WHERE THE PIXELS PAIR (`vouch`), every axis reads the pairs: each paired
/// pixel's own change is testimony of what happened to THAT pixel, and the
/// band a pixel belongs to is read off the source, never off the move
/// ([`fit_band_amounts`] for saturation and luminance, [`fit_band_hues`] for
/// the rotation). Until v1.18.0 (J134) the two amounts were read from each
/// band's POPULATION even on a paired frame, and a population answers for
/// more than its band's amount: a rotation re-populates the bands it moves
/// pixels between, and members at the top of the engine's saturation scale
/// do not answer an increase at all. Measured on the self-consistency gate's
/// seaside frame, whose sky lies between the Aqua and the Blue centres (hue
/// 198-221 degrees) with seven tenths of it at full HSL saturation: a target
/// made by Blue hue +20 ALONE was proposed Aqua -33.4 / Blue +33.4 on
/// saturation and again on luminance, beside the right rotation (Blue
/// +18.7), and a target made by Blue saturation +30 alone was proposed Aqua
/// +22.5 / Blue -11.5 — a fan across two bands that share a population. The
/// validity law set both proposals aside (the first re-estimated whole, the
/// rotation with it; the second scaled to a fifth), and neither control came
/// back (rel1180 gates/j134-hue-trace and j134-sat-trace, 2026-11-10).
///
/// WHERE THEY DO NOT (Atmosphere mode), the amounts are solved from
/// POPULATION statistics ([`fit_band_populations`]) and the hue is not solved
/// at all. WHY a population is legitimate there for the amounts and never
/// for the hue: it is the per-band form of the argument that already lets
/// Atmosphere mode fit one global saturation and one white balance. A band's
/// mean chroma and mean lightness are marginal statistics of a
/// sub-population; matching them needs no pixel pairing, only the claim that
/// both frames' members of that band are measurements of the same subject.
/// That claim is exactly what the evidence model already adjudicates — so
/// the admission asks it, and asks it with the SAME criterion the
/// unrepresented-controls disclosure reads (`evidence.hue[band].weight > 0`
/// plus a two-sided [`EVIDENCE_MIN_SHARE`] of the chromatic mass). One gate,
/// two consumers; a band either side cannot see is left at zero and NAMED,
/// never silently read as "equal". A per-band HUE from populations is
/// circular — rotating a band re-populates it — and a centroid hue delta
/// conflating content with style is what turned brown rock olive in the
/// 2026-07-07 failure.
///
/// What ships is the joint objective's question (RULING-1180-joint-r1): the
/// stage estimates, and judges nothing.
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
    let (facts, admitted, target_bands) = admit_bands(s_img, sp, tp, evidence, cells, recipe);
    if !admitted.iter().any(|&band| band) {
        return facts;
    }
    match vouch {
        // The pixels pair: every axis of the mixer reads the pairs.
        Some(vouch) => {
            fit_band_amounts(s_img, tp, vouch, budget, &admitted, recipe);
            fit_band_hues(s_img, tp, vouch, budget, &admitted, recipe);
        }
        // No pairing: the amounts from the populations, and no hue.
        None => fit_band_populations(s_img, evidence, &target_bands, &admitted, budget, recipe),
    }
    facts
}

/// The mixer's population gate ([`fit_hsl_stage`]): the bands it admits on
/// the frame as the stage finds it, what it has to disclose about the rest,
/// and the target's band statistics the population solve matches.
fn admit_bands(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    cells: Option<&crate::fit_cells::PairedCells>,
    recipe: &EditRecipe,
) -> (HslStageFacts, [bool; EVIDENCE_HUE_BANDS], [super::curves::BandStat; 8]) {
    let mut facts = HslStageFacts::default();
    let before_px = pixels_of(&render::develop_preview(s_img, recipe));

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
    (facts, admitted, sb)
}

/// The saturation and luminance of a pair whose pixels do NOT pair
/// ([`fit_hsl_stage`]): each admitted band's population matched to the
/// target's (`sb`, [`admit_bands`]).
///
/// The amounts are solved JOINTLY: `apply_hsl` blends two bands per pixel
/// through a partition of unity and fades itself out below chroma 0.22, so
/// one band's amount moves the members of both bands its pixels belong to,
/// and a band's population answers to its neighbour's amount too
/// ([`band_population_reach`]). Read band by band, as a ratio chase, an
/// Orange-only move of −25 luminance on a real RAW was split into Red −10.7
/// and Orange −14.5, and the elimination then dropped Red and left Orange
/// short (rel1180 msg-sc case 2). Every admitted band's population demand is
/// one equation over the reach of its members, least squares over the
/// admitted bands. The solve is closed-loop through the real engine for the
/// same reason the global chroma chase is (`apply_hsl` runs before
/// saturation, the wheels and clarity, so the linearised answer is a first
/// step): two rounds.
fn fit_band_populations(
    s_img: &DynamicImage,
    evidence: &EvidenceModel,
    sb: &[super::curves::BandStat; 8],
    admitted: &[bool; EVIDENCE_HUE_BANDS],
    budget: FitBudget,
    recipe: &mut EditRecipe,
) {
    // --- the joint solve, closed-loop through the real engine --------------
    const B: usize = EVIDENCE_HUE_BANDS;
    for _ in 0..2 {
        let cur = pixels_of(&render::develop_preview(s_img, recipe));
        let (cs, _) = band_stats_weighted(&cur, &evidence.source_hue_weights);
        let reach = band_population_reach(&cur, &evidence.source_hue_weights);
        let mut moved_any = false;
        for (axis, reach_axis) in reach.iter().enumerate() {
            // A band's mean chroma (saturation) or mean luma (luminance), now
            // and in the target, and its members' reach per unit amount of
            // every band, relative to its own mean: the equation
            // `Σ_c reach_c · x_c = want / now − 1`.
            let mut equations: Vec<(f64, [f64; B], f64)> = Vec::new();
            let mut free: Vec<usize> = Vec::new();
            for band in 0..B {
                if !admitted[band] || cs[band].w <= 0.0 || sb[band].w <= 0.0 {
                    continue;
                }
                let (sum, want) = if axis == 0 { (cs[band].c, sb[band].c / sb[band].w) } else { (cs[band].y, sb[band].y / sb[band].w) };
                let now = sum / cs[band].w;
                if now < HSL_BAND_MIN_MEAN {
                    continue;
                }
                let row: [f64; B] = std::array::from_fn(|c| reach_axis[band][c] / sum);
                equations.push((cs[band].w, row, want / now - 1.0));
                free.push(band);
            }
            let total: f64 = equations.iter().map(|e| e.0).sum();
            let Some(amount) = least_squares_over_bands(&equations, &free, total) else { continue };
            for (&band, &x) in free.iter().zip(&amount) {
                let step = (x as f32).clamp(-HSL_BAND_STEP, HSL_BAND_STEP);
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
}

/// One of the mixer's two AMOUNT axes ([`fit_band_amounts`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BandAmount {
    Saturation,
    Luminance,
}

impl BandAmount {
    /// The quantity the axis scales, in display units: a pixel's chroma for
    /// saturation, its HSL lightness for luminance ([`render::apply_hsl`]).
    fn quantity(self, p: &[f32; 3]) -> f32 {
        match self {
            BandAmount::Saturation => render::chroma(p),
            BandAmount::Luminance => render::rgb_to_hsl(p[0], p[1], p[2]).2,
        }
    }

    fn amount(self, recipe: &EditRecipe, band: usize) -> f32 {
        match self {
            BandAmount::Saturation => recipe.hsl.saturation[band],
            BandAmount::Luminance => recipe.hsl.luminance[band],
        }
    }

    fn slot(self, recipe: &mut EditRecipe, band: usize) -> &mut f32 {
        match self {
            BandAmount::Saturation => &mut recipe.hsl.saturation[band],
            BandAmount::Luminance => &mut recipe.hsl.luminance[band],
        }
    }
}

/// Each band's testifying pixels by source luma range: `(pixel, mass)`.
type BandCells = [Vec<Vec<(u32, f32)>>; EVIDENCE_HUE_BANDS];

/// Solve `hsl.saturation` and `hsl.luminance` from the PAIRED pixels, in the
/// form the band hues are solved in ([`fit_band_hues`]) — one solve form for
/// the mixer's three axes (v1.18.0, J134; why not the populations:
/// [`fit_hsl_stage`]).
///
/// DEMAND. On the frame as the round finds it, each paired pixel's own
/// change of the quantity the axis scales — its chroma for saturation, its
/// HSL lightness for luminance — target minus current, in display units,
/// weighted by its trust (the paired voucher's robust weight) and by the
/// weight the engine gives its source hue in each of the two bands
/// bracketing it ([`render::hsl_band_reach`]). A pixel under the evidence
/// chroma floor on the current frame ([`evidence_hue_band`]) is in no band.
/// A band the population gate did not admit, or holding under
/// [`EVIDENCE_MIN_SHARE`] of the testifying mass, is silent; a band's pixels
/// are binned by source luma into the evidence luma ranges, a range under
/// [`EVIDENCE_MIN_SHARE`] of the band's mass silent, each range testifying
/// its weighted median.
///
/// MOVE, read THROUGH THE ENGINE. A band's reach on a range is the range's
/// weighted median of what the engine itself does to those pixels when that
/// band's amount moves: the round's largest step toward the band's own
/// mass-weighted ask (`HSL_BAND_STEP`, inside the strength's band budget) is
/// rendered, and read per unit of amount. The same statistic as the demand,
/// on the same pixels — so where most of a range's pixels cannot follow the
/// control (an HSL saturation already at the top of its scale under an
/// increase, a lightness at its clamp, the chroma gate shut) the range's
/// reach is zero and it says nothing about the amount, whatever it asks. A
/// reach linearised on the finished frame (`1 + adj / 100` on every member)
/// knows none of that: on the seaside frame it read 0.35 % of the Aqua
/// population's chroma per unit of Aqua where the engine moves it 0.038 %
/// (rel1180 tools/band_fd.py on the app's own renders, 2026-11-10).
///
/// CONSENSUS. A band's ask is [`wb_consensus`] over its ranges that the
/// band's own amount moves (the white balance's rule on the white balance's
/// testimony unit): a band whose movable ranges ask opposite amounts in
/// force asks nothing, and then every one of its ranges asks to stay.
///
/// SOLVE. Every testifying range of a heard band is one equation, its ask
/// against its reach per unit of every asking band; the amounts are the
/// mass-weighted least squares over the combinations the frame MEASURES
/// ([`measured_least_squares`]), each round's step inside `HSL_BAND_STEP`
/// and the amounts inside the strength's band budget, kept only when they
/// leave less of the demand than no move does. Two rounds, closed loop: the
/// second reads what the first left.
fn fit_band_amounts(
    s_img: &DynamicImage,
    tp: &[[f32; 3]],
    vouch: PixelVouch<'_>,
    budget: FitBudget,
    admitted: &[bool; EVIDENCE_HUE_BANDS],
    recipe: &mut EditRecipe,
) {
    let trust = vouch.0;
    for _ in 0..2 {
        let at = recipe.clone();
        let cur = pixels_of(&render::develop_preview(s_img, &at));
        let n = cur.len().min(tp.len()).min(trust.len());
        let mut cells: BandCells = std::array::from_fn(|_| vec![Vec::new(); EVIDENCE_LUMA_BINS]);
        for i in 0..n {
            let e = trust[i].max(0.0);
            let p = &cur[i];
            if e <= 0.0 || evidence_hue_band(p).is_none() {
                continue;
            }
            let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
            let Some((b0, b1, w1, _)) = render::hsl_band_reach(h, render::chroma(p)) else {
                continue;
            };
            let range = evidence_luma_bin(luma601(p));
            for (band, w) in [(b0, 1.0 - w1), (b1, w1)] {
                if e * w > 0.0 {
                    cells[band][range].push((i as u32, e * w));
                }
            }
        }
        let mut moved_any = false;
        for axis in [BandAmount::Saturation, BandAmount::Luminance] {
            let Some(step) = paired_band_steps(axis, s_img, &at, &cur, tp, &cells, admitted, budget) else {
                continue;
            };
            for (band, &x) in step.iter().enumerate() {
                if x.abs() < 1.0 {
                    continue;
                }
                let slot = axis.slot(recipe, band);
                let next = round1((*slot + x).clamp(-budget.hsl_band, budget.hsl_band));
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
}

/// One round of [`fit_band_amounts`] on one axis: the step each band's
/// amount takes from `at` (whose preview is `cur`), or `None` when no band
/// asks, the engine moves none that does, or the steps leave no less of the
/// demand than standing still.
#[allow(clippy::too_many_arguments)]
fn paired_band_steps(
    axis: BandAmount,
    s_img: &DynamicImage,
    at: &EditRecipe,
    cur: &[[f32; 3]],
    tp: &[[f32; 3]],
    cells: &BandCells,
    admitted: &[bool; EVIDENCE_HUE_BANDS],
    budget: FitBudget,
) -> Option<[f32; EVIDENCE_HUE_BANDS]> {
    const B: usize = EVIDENCE_HUE_BANDS;
    let cell_mass = |b: usize, r: usize| cells[b][r].iter().map(|&(_, m)| m as f64).sum::<f64>();
    let mass: [f64; B] = std::array::from_fn(|b| (0..EVIDENCE_LUMA_BINS).map(|r| cell_mass(b, r)).sum());
    let total: f64 = mass.iter().sum();
    if total <= 0.0 {
        return None;
    }
    // A range's weighted median of one per-pixel reading.
    let median_of = |b: usize, r: usize, value: &dyn Fn(usize) -> f32| -> f32 {
        let mut samples: Vec<(f32, f32)> = cells[b][r].iter().map(|&(i, m)| (value(i as usize), m)).collect();
        weighted_median(&mut samples)
    };
    // The heard bands' testifying ranges: (range, mass, what it asks).
    let mut testifying: [Vec<(usize, f64, f32)>; B] = Default::default();
    for band in 0..B {
        if !admitted[band] || mass[band] / total < EVIDENCE_MIN_SHARE as f64 {
            continue;
        }
        for r in 0..EVIDENCE_LUMA_BINS {
            let m = cell_mass(band, r);
            if m / mass[band] < EVIDENCE_MIN_SHARE as f64 {
                continue;
            }
            let ask = median_of(band, r, &|i| axis.quantity(&tp[i]) - axis.quantity(&cur[i]));
            testifying[band].push((r, m, ask));
        }
    }
    // The engine's own answer to each heard band's largest step toward its
    // mass-weighted ask, per pixel and per unit of amount.
    let mut response: [Option<Vec<f32>>; B] = Default::default();
    for band in 0..B {
        let lean: f64 = testifying[band].iter().map(|&(_, m, ask)| m * ask as f64).sum();
        if lean == 0.0 {
            continue;
        }
        let now = axis.amount(at, band);
        let step = (now + HSL_BAND_STEP * lean.signum() as f32).clamp(-budget.hsl_band, budget.hsl_band) - now;
        if step.abs() < 1.0 {
            continue;
        }
        let mut probe = at.clone();
        *axis.slot(&mut probe, band) = now + step;
        let moved = pixels_of(&render::develop_preview(s_img, &probe));
        response[band] =
            Some(moved.iter().zip(cur).map(|(p, c)| (axis.quantity(p) - axis.quantity(c)) / step).collect());
    }
    let mut equations: Vec<(f64, [f64; B], f64)> = Vec::new();
    let mut asks_any = [false; B];
    for band in 0..B {
        if testifying[band].is_empty() {
            continue;
        }
        let rows: Vec<[f64; B]> = testifying[band]
            .iter()
            .map(|&(r, _, _)| {
                std::array::from_fn(|c| response[c].as_ref().map_or(0.0, |moved| median_of(band, r, &|i| moved[i]) as f64))
            })
            .collect();
        // A range the band's own amount does not move has nothing to say
        // about that amount.
        let movable: Vec<(f32, f32)> = testifying[band]
            .iter()
            .zip(&rows)
            .filter(|(_, row)| row[band] != 0.0)
            .map(|(&(_, m, ask), _)| (ask, m as f32))
            .collect();
        asks_any[band] = wb_consensus(&movable).0 != 0.0;
        for (&(_, m, ask), row) in testifying[band].iter().zip(rows) {
            equations.push((m, row, if asks_any[band] { ask as f64 } else { 0.0 }));
        }
    }
    let free: Vec<usize> = (0..B).filter(|&b| asks_any[b]).collect();
    let heard: f64 = equations.iter().map(|e| e.0).sum();
    let solved = measured_least_squares(&equations, &free, heard, budget.hsl_band as f64)?;
    let mut step = [0.0f32; B];
    for (&band, &x) in free.iter().zip(&solved) {
        step[band] = (x as f32).clamp(-HSL_BAND_STEP, HSL_BAND_STEP);
    }
    let left = |x: &[f32; B]| -> f64 {
        equations
            .iter()
            .map(|(w, row, want)| {
                let moved: f64 = (0..B).map(|c| row[c] * x[c] as f64).sum();
                w * (want - moved).powi(2)
            })
            .sum()
    };
    (left(&step) < left(&[0.0; B])).then_some(step)
}

/// Half an output code: the frames the fit reads are rounded to one
/// ([`pixels_of`]), and a change under half of one is not in them.
const FRAME_ROUNDING: f64 = 0.5 / 255.0;

/// The mass-weighted least-squares amounts of the `free` bands for
/// `equations` (each `(mass, reach per unit of every band, what it asks)`,
/// in display units; `heard` their mass), over the combinations of amounts
/// the frame MEASURES, and zero along the rest — the minimum-norm answer.
///
/// Two bands that share a population reach its ranges in near proportion
/// (the seaside sky between Aqua and Blue: every testifying range answers
/// the two amounts in one ratio), so the equations fix one combination of
/// the two and say nothing of the other. Solved as if they said something,
/// that other combination is fitted to the rounding of the readings and
/// comes out as a fan — one band far up, its neighbour far down (the gate's
/// readings in [`fit_hsl_stage`]). A combination is measured when, taken to
/// the strength's whole band `budget`, it moves the heard mass by at least
/// [`FRAME_ROUNDING`] (root mean square): below that the frames cannot tell
/// the budget's two ends apart along it. Along a measured combination the
/// answer is the least squares; along an unmeasured one the amounts stay
/// where they are, so the evidence is carried by the smallest amounts that
/// explain it, and the elimination asks each band against its absence.
/// `None` when nothing is measured (no band asks, or the engine moves none
/// that does).
fn measured_least_squares(
    equations: &[(f64, [f64; EVIDENCE_HUE_BANDS], f64)],
    free: &[usize],
    heard: f64,
    budget: f64,
) -> Option<Vec<f64>> {
    let k = free.len();
    if k == 0 || heard <= 0.0 {
        return None;
    }
    let mut normal = vec![vec![0.0f64; k]; k];
    let mut asked = vec![0.0f64; k];
    for (w, reach_r, want) in equations {
        for (r, &cr) in free.iter().enumerate() {
            for (c, &cc) in free.iter().enumerate() {
                normal[r][c] += w * reach_r[cr] * reach_r[cc];
            }
            asked[r] += w * reach_r[cr] * want;
        }
    }
    // `normal`'s eigenvalue along a unit combination `v` is `Σ w·(reach·v)²`:
    // the heard mass's squared move per unit of amount along it.
    let (values, vectors) = symmetric_eigen(normal);
    let mut amounts = vec![0.0f64; k];
    let mut measured = false;
    for (j, &value) in values.iter().enumerate() {
        if value <= 0.0 || (value / heard).sqrt() * budget < FRAME_ROUNDING {
            continue;
        }
        measured = true;
        let along: f64 = (0..k).map(|r| vectors[r][j] * asked[r]).sum::<f64>() / value;
        for (r, amount) in amounts.iter_mut().enumerate() {
            *amount += along * vectors[r][j];
        }
    }
    measured.then_some(amounts)
}

/// The eigenvalues of a small symmetric matrix and its eigenvectors (the
/// COLUMNS of the second value, in the eigenvalues' order), by cyclic Jacobi
/// rotations — at most eight bands, so a handful of sweeps.
fn symmetric_eigen(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let k = a.len();
    let mut v = vec![vec![0.0f64; k]; k];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..64 {
        let off: f64 = (0..k).flat_map(|p| (p + 1..k).map(move |q| (p, q))).map(|(p, q)| a[p][q] * a[p][q]).sum();
        let scale: f64 = (0..k).map(|p| a[p][p] * a[p][p]).sum();
        if off <= f64::EPSILON * f64::EPSILON * scale {
            break;
        }
        for p in 0..k {
            for q in p + 1..k {
                if a[p][q] == 0.0 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                // A ← Jᵀ·A·J and V ← V·J for the rotation J in the (p, q) plane.
                for row in a.iter_mut() {
                    let (x, y) = (row[p], row[q]);
                    row[p] = c * x - s * y;
                    row[q] = s * x + c * y;
                }
                let (upper, lower) = a.split_at_mut(q);
                for (at_p, at_q) in upper[p].iter_mut().zip(lower[0].iter_mut()) {
                    let (x, y) = (*at_p, *at_q);
                    *at_p = c * x - s * y;
                    *at_q = s * x + c * y;
                }
                for row in v.iter_mut() {
                    let (x, y) = (row[p], row[q]);
                    row[p] = c * x - s * y;
                    row[q] = s * x + c * y;
                }
            }
        }
    }
    ((0..k).map(|i| a[i][i]).collect(), v)
}

/// The mixer's reach on each band's POPULATION, per unit amount of every
/// band: `reach[axis][b][c]` is the change of band `b`'s summed chroma
/// (axis 0, saturation) or summed luma (axis 1, luminance), in the band
/// statistics' own membership weights ([`band_stats_weighted`]: the
/// pixel's weight times its partition weight in `b`), per unit of band `c`'s
/// amount — the engine's law ([`render::apply_hsl`]): a pixel's saturation
/// scales by `1 + adj / 100` and its HSL lightness by `1 + 0.5 · adj / 100`,
/// `adj` its two bracketing bands' amounts in its partition weights times
/// the chroma gate's fade ([`render::hsl_band_reach`]); at fixed hue chroma
/// is proportional to saturation and luma to lightness, linearised at the
/// frame as it stands.
fn band_population_reach(px: &[[f32; 3]], weights: &[f32]) -> [[[f64; EVIDENCE_HUE_BANDS]; EVIDENCE_HUE_BANDS]; 2] {
    let mut reach = [[[0.0f64; EVIDENCE_HUE_BANDS]; EVIDENCE_HUE_BANDS]; 2];
    for (i, p) in px.iter().enumerate() {
        let weight = weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
        // The band statistics' own chroma gate.
        if weight <= 0.0 || render::chroma(p) < 0.06 {
            continue;
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let Some((b0, b1, w1, fade)) = render::hsl_band_reach(h, render::chroma(p)) else {
            continue;
        };
        let (w1, fade) = (w1 as f64, fade as f64);
        let quantity = [render::chroma(p) as f64 / 100.0, luma601(p) as f64 * 0.5 / 100.0];
        for (member, m) in [(b0, 1.0 - w1), (b1, w1)] {
            for (axis, q) in quantity.iter().enumerate() {
                reach[axis][member][b0] += weight * m * q * fade * (1.0 - w1);
                reach[axis][member][b1] += weight * m * q * fade * w1;
            }
        }
    }
    reach
}

/// The mixer's one solve form: the mass-weighted least-squares amounts of the
/// `free` bands for `equations` (each `(mass, reach per unit of every band,
/// what the equation asks)`), by Gaussian elimination with partial pivoting
/// on the normal equations. `None` when a free band has no pivot — no
/// equation reaches it, so the solve has no answer for it. `total` is the
/// equations' mass, the scale of the pivot test.
fn least_squares_over_bands(
    equations: &[(f64, [f64; EVIDENCE_HUE_BANDS], f64)],
    free: &[usize],
    total: f64,
) -> Option<Vec<f64>> {
    let k = free.len();
    if k == 0 {
        return None;
    }
    let mut normal = vec![vec![0.0f64; k + 1]; k];
    for (w, reach_r, want) in equations {
        for (r, &cr) in free.iter().enumerate() {
            for (c, &cc) in free.iter().enumerate() {
                normal[r][c] += w * reach_r[cr] * reach_r[cc];
            }
            normal[r][k] += w * reach_r[cr] * want;
        }
    }
    for col in 0..k {
        let pivot = (col..k)
            .max_by(|&a, &b| normal[a][col].abs().total_cmp(&normal[b][col].abs()))
            .unwrap_or(col);
        normal.swap(col, pivot);
        if normal[col][col].abs() <= f64::EPSILON * total {
            return None;
        }
        let pivot_row = normal[col].clone();
        for (row, line) in normal.iter_mut().enumerate() {
            if row != col {
                let f = line[col] / pivot_row[col];
                for (x, p) in line[col..].iter_mut().zip(&pivot_row[col..]) {
                    *x -= f * p;
                }
            }
        }
    }
    Some((0..k).map(|r| normal[r][k] / normal[r][r]).collect())
}

/// Solve `hsl.hue`, one band at a time, from the PAIRED pixels: the carrier
/// of the per-hue colour remainder the white balance and the grade wheels
/// leave (a wheel moves every hue of its tonal region alike, so a change
/// one hue family asks and its neighbours do not is a band's).
///
/// DEMAND. On the frame as this stage finds it, each paired pixel's own hue
/// change, target minus source, in degrees — testimony of what happened to
/// THAT pixel, which is why the circularity of a band's population hue (see
/// [`fit_hsl_stage`]) does not reach it. Each pixel is weighted by its trust
/// (the paired voucher's robust weight) and by the weight the engine gives
/// its source hue in each of the two bands bracketing it
/// ([`render::hsl_band_reach`]). Pixels under the evidence chroma floor on
/// either side ([`evidence_hue_band`]) have no hue to testify with. A band
/// the population gate did not admit, or holding under
/// [`EVIDENCE_MIN_SHARE`] of the testifying mass, is silent. A band's demand
/// is the white balance's own reading on the white balance's own testimony
/// unit ([`super::evidence::wb_axes_by_range`]): its pixels binned by source
/// luma into the evidence luma ranges, a range under [`EVIDENCE_MIN_SHARE`]
/// of the band's mass silent, each range testifying its weighted median, and
/// [`wb_consensus`] over the ranges — so a band whose brightness ranges ask
/// opposite rotations in force asks nothing. One pixel's hue change is not
/// testimony on its own: measured 2026-11-02 on a real sunset pair, every
/// band's pixels scattered so widely about a median of a degree or two that
/// 24-47 % of them sat on the far side, and a consensus over pixels vetoed
/// every band of every photo in the leg; a range's median is the unit the
/// rule's constants were set on.
///
/// MOVE. The engine's hue law, which is linear in the amounts: a pixel
/// rotates by `HSL_HUE_REACH_DEG / 100` degrees per unit of each bracketing
/// band's amount, times its weight in that band and the chroma gate's fade.
/// Averaged over each testifying range of each band that is a reach per
/// range and band (two bands share the pixels between their centres). Every
/// testifying range of a heard band is one equation: its own median against
/// its own reach when the band asks, zero when the band's consensus asks
/// nothing (it asks to stay). The amounts are the mass-weighted
/// least-squares answer, clamped to the
/// strength's band budget (`budget.hsl_band`, the bound the saturation and
/// luminance chase obey) and kept only when they leave less of the demand
/// than no rotation does.
///
/// What ships of it is the joint objective's question (RULING-1180-joint-r1):
/// the validity law reads the chroma dials on the cells they move (a blind
/// cell withheld under `Withhold`), and the elimination judges each
/// band against its absence on F.
#[allow(clippy::too_many_arguments)]
fn fit_band_hues(
    s_img: &DynamicImage,
    tp: &[[f32; 3]],
    vouch: PixelVouch<'_>,
    budget: FitBudget,
    admitted: &[bool; EVIDENCE_HUE_BANDS],
    recipe: &mut EditRecipe,
) {
    const B: usize = EVIDENCE_HUE_BANDS;
    let restore = recipe.hsl.hue;
    let before = pixels_of(&render::develop_preview(s_img, recipe));
    let trust = vouch.0;
    let n = before.len().min(tp.len()).min(trust.len());
    // Each band's testimony, binned by the pixel's source luma range.
    let mut asks: [Vec<Vec<(f32, f32)>>; B] = std::array::from_fn(|_| vec![Vec::new(); EVIDENCE_LUMA_BINS]);
    let mut range_mass = [[0.0f64; EVIDENCE_LUMA_BINS]; B];
    let mut mass = [0.0f64; B];
    // reach[b][r][c]: the rotation, in degrees per unit of band c's amount,
    // of band b's pixels in luma range r, mass-weighted (divided through
    // below) — the reach of the same unit the range's median testifies for.
    let mut reach = [[[0.0f64; B]; EVIDENCE_LUMA_BINS]; B];
    for i in 0..n {
        let e = trust[i].max(0.0);
        let (p, t) = (before[i], tp[i]);
        if e <= 0.0 || evidence_hue_band(&p).is_none() || evidence_hue_band(&t).is_none() {
            continue;
        }
        let (hp, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let (ht, _, _) = render::rgb_to_hsl(t[0], t[1], t[2]);
        let Some((b0, b1, w1, fade)) = render::hsl_band_reach(hp, render::chroma(&p)) else {
            continue;
        };
        let turn = ((ht - hp + 0.5).rem_euclid(1.0) - 0.5) * 360.0;
        let per_unit = (fade * render::HSL_HUE_REACH_DEG / 100.0) as f64;
        let range = evidence_luma_bin(luma601(&p));
        for (band, w) in [(b0, 1.0 - w1), (b1, w1)] {
            let m = e * w;
            if m <= 0.0 {
                continue;
            }
            asks[band][range].push((turn, m));
            range_mass[band][range] += m as f64;
            mass[band] += m as f64;
            reach[band][range][b0] += m as f64 * per_unit * (1.0 - w1) as f64;
            reach[band][range][b1] += m as f64 * per_unit * w1 as f64;
        }
    }
    let total: f64 = mass.iter().sum();
    if total <= 0.0 {
        return;
    }
    // The equations: (band, its range's mass, its range's reach, what the
    // range asks of the move) for every testifying range of a heard band.
    let mut equations: Vec<(f64, [f64; B], f64)> = Vec::new();
    let mut asks_any = [false; B];
    for band in 0..B {
        if !admitted[band] || mass[band] / total < EVIDENCE_MIN_SHARE as f64 {
            continue;
        }
        let testifying: Vec<usize> = (0..EVIDENCE_LUMA_BINS)
            .filter(|&r| range_mass[band][r] / mass[band] >= EVIDENCE_MIN_SHARE as f64)
            .collect();
        let medians: Vec<f32> = testifying.iter().map(|&r| weighted_median(&mut asks[band][r])).collect();
        let ranges: Vec<(f32, f32)> =
            testifying.iter().zip(&medians).map(|(&r, &m)| (m, range_mass[band][r] as f32)).collect();
        asks_any[band] = wb_consensus(&ranges).0 != 0.0;
        for (&r, &median) in testifying.iter().zip(&medians) {
            let w = range_mass[band][r];
            let reach_r: [f64; B] = std::array::from_fn(|c| reach[band][r][c] / w);
            equations.push((w, reach_r, if asks_any[band] { median as f64 } else { 0.0 }));
        }
    }
    // The unknowns are the bands that ask.
    let free: Vec<usize> = (0..B).filter(|&b| asks_any[b]).collect();
    if free.is_empty() {
        return;
    }
    // A band whose pixels the mixer cannot reach (no pivot) leaves the solve
    // unanswerable.
    let Some(solved) = least_squares_over_bands(&equations, &free, total) else { return };
    let mut amount = [0.0f32; B];
    for (&band, &x) in free.iter().zip(&solved) {
        amount[band] = round1((x as f32).clamp(-budget.hsl_band, budget.hsl_band));
    }
    let left = |x: &[f32; B]| -> f64 {
        equations
            .iter()
            .map(|(w, reach_r, want)| {
                let moved: f64 = (0..B).map(|c| reach_r[c] * x[c] as f64).sum();
                w * (want - moved).powi(2)
            })
            .sum()
    };
    if left(&amount) >= left(&[0.0; B]) {
        return;
    }
    recipe.hsl.hue = std::array::from_fn(|b| round1((restore[b] + amount[b]).clamp(-100.0, 100.0)));
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: usize = EVIDENCE_HUE_BANDS;

    fn row(aqua: f64, blue: f64) -> [f64; B] {
        let mut r = [0.0f64; B];
        r[4] = aqua;
        r[5] = blue;
        r
    }

    /// The Jacobi decomposition rebuilds the matrix it was given, and its
    /// vectors are orthonormal.
    #[test]
    fn the_eigen_decomposition_rebuilds_its_matrix() {
        let m = vec![vec![4.0, 1.0, -2.0], vec![1.0, 3.0, 0.5], vec![-2.0, 0.5, 5.0]];
        let (values, vectors) = symmetric_eigen(m.clone());
        for r in 0..3 {
            for c in 0..3 {
                let rebuilt: f64 = (0..3).map(|j| values[j] * vectors[r][j] * vectors[c][j]).sum();
                assert!((rebuilt - m[r][c]).abs() < 1e-12, "[{r}][{c}]: {rebuilt} against {}", m[r][c]);
                let dot: f64 = (0..3).map(|i| vectors[i][r] * vectors[i][c]).sum();
                assert!((dot - if r == c { 1.0 } else { 0.0 }).abs() < 1e-12, "columns {r}, {c}: {dot}");
            }
        }
    }

    /// Where the ranges answer two bands in DIFFERENT ratios the frame
    /// measures both amounts, and the measured solve is the least squares:
    /// the amounts that made the asks come back.
    #[test]
    fn the_measured_solve_is_the_least_squares_where_the_frame_measures_both() {
        let (aqua, blue) = (6.0, -21.0);
        let rows = [row(0.0010, 0.0002), row(0.0003, 0.0011), row(0.0006, 0.0006)];
        let equations: Vec<(f64, [f64; B], f64)> =
            rows.iter().map(|r| (1.0, *r, r[4] * aqua + r[5] * blue)).collect();
        let solved = measured_least_squares(&equations, &[4, 5], 3.0, 33.4).expect("both combinations are measured");
        assert!((solved[0] - aqua).abs() < 1e-6 && (solved[1] - blue).abs() < 1e-6, "{solved:?}");
    }

    /// The seaside sky's case: every testifying range answers Aqua and Blue
    /// in ONE ratio, and the asks carry a code's rounding. The equations fix
    /// one combination of the two amounts; the measured solve carries the
    /// ask on that combination alone — both bands the same way, in the ratio
    /// of their reach — where the plain least squares fits the other
    /// combination to the rounding and fans (one band far up, its neighbour
    /// far down). Mutation: the measured line dropped, the solve IS the plain
    /// one and the fan returns.
    #[test]
    fn two_bands_that_reach_a_population_alike_share_its_ask_and_do_not_fan() {
        // reach per unit, display units: Blue reaches the sky half again as far as Aqua
        let (aqua_reach, blue_reach) = (0.00024, 0.00036);
        // Blue +30 made the target; the two ranges' asks are rounded to the 8-bit grid,
        // and the second range's reach to Aqua is off the ratio by one part in a thousand
        let ask = |reach: f64| ((reach * 30.0) * 255.0).round() / 255.0;
        let equations = vec![
            (0.6, row(aqua_reach, blue_reach), ask(blue_reach)),
            (0.4, row(0.5 * aqua_reach * 1.001, 0.5 * blue_reach), ask(0.5 * blue_reach)),
        ];
        let solved = measured_least_squares(&equations, &[4, 5], 1.0, 33.4).expect("the shared combination is measured");
        let plain = least_squares_over_bands(&equations, &[4, 5], 1.0).expect("the plain solve has a pivot");
        eprintln!("measured Aqua {:+.1} Blue {:+.1}; plain Aqua {:+.1} Blue {:+.1}", solved[0], solved[1], plain[0], plain[1]);
        assert!(solved[0] > 0.0 && solved[1] > 0.0, "both bands carry the ask the same way: {solved:?}");
        assert!(
            (solved[1] / solved[0] - blue_reach / aqua_reach).abs() < 0.01,
            "in the ratio of their reach: {solved:?}"
        );
        assert!(solved[1] > solved[0], "the band that reaches farther carries more: {solved:?}");
        assert!(
            plain[0] * plain[1] < 0.0 && plain[0].abs().max(plain[1].abs()) > 100.0,
            "premise: the plain least squares fans on the same equations: {plain:?}"
        );
    }

    /// A band the engine does not move here (every reach zero) is not
    /// measured: no amount, rather than a division by nothing.
    #[test]
    fn a_band_the_engine_does_not_move_is_given_no_amount() {
        let equations = vec![(1.0, row(0.0, 0.0), 0.02), (1.0, row(0.0, 0.0), 0.03)];
        assert!(measured_least_squares(&equations, &[4, 5], 2.0, 33.4).is_none());
        assert!(measured_least_squares(&equations, &[], 2.0, 33.4).is_none());
    }
}
