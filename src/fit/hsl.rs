//! The HSL stage: band refusals and withdrawals, the halved bands, and the stage itself.

use super::*;
use super::evidence::{EVIDENCE_LUMA_BINS, evidence_luma_bin, wb_consensus, weighted_median};
use super::grade::largest_vouched_share;

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
/// Hue is halved with the other two since the stage solves it
/// ([`fit_band_hues`], 2026-11-02): a do-no-harm loop that could not shrink
/// an axis the stage writes could only spin to its bound and withdraw.
pub(super) fn halved_hsl(hsl: &crate::recipe::Hsl) -> crate::recipe::Hsl {
    let axis = |values: &[f32; 8]| -> [f32; 8] {
        std::array::from_fn(|i| {
            let v = round1(values[i] * 0.5);
            if v.abs() < 1.0 { 0.0 } else { v }
        })
    };
    crate::recipe::Hsl {
        hue: axis(&hsl.hue),
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
/// `hsl.hue` is NOT solved here from populations, and never is: rotating a
/// band re-populates it, so a band's population evidence about its own hue is
/// circular, and a centroid hue delta conflating content with style is what
/// turned brown rock olive in the 2026-07-07 failure. It is solved after
/// this, from PAIRED pixels only ([`fit_band_hues`]), where each pixel's
/// own rotation is testimony and the band a pixel belongs to is read off the
/// source, never off the move.
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
    let (facts, admitted) = fit_band_populations(s_img, sp, tp, evidence, vouch, cells, budget, recipe);
    // The band hues are a paired measurement: no pairing, no cells, no hue.
    if let (Some(vouch), Some(cells)) = (vouch, cells) {
        fit_band_hues(s_img, sp, tp, evidence, vouch, cells, budget, &admitted, recipe);
    }
    facts
}

/// The saturation and luminance half of [`fit_hsl_stage`], and the bands
/// its population gate admitted.
#[allow(clippy::too_many_arguments)]
fn fit_band_populations(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<&crate::fit_cells::PairedCells>,
    budget: FitBudget,
    recipe: &mut EditRecipe,
) -> (HslStageFacts, [bool; EVIDENCE_HUE_BANDS]) {
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
        return (facts, admitted);
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
        return (facts, admitted);
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
    (facts, admitted)
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
/// ADMISSION. The target's cells along the colour axis, read over the
/// pixels the rotated bands reach (each pixel's membership, times its weight
/// in the bands that move) — the band-population admission's own reading
/// (`one_sided_vouch` asks the cells over the band's members), because a
/// band rotation leaves every other pixel where it was and the cells there
/// have nothing to converge on (measured on the one-band fixture: 0.233 of
/// the frame's cells converged, every one aligned, none diverged). The whole
/// move or the largest share they vouch ([`largest_vouched_share`], the
/// colour grade's search); then the stage's own do-no-harm — halve until the
/// frame's look error does not rise and no new pixel is carried through a
/// hue band the evidence never measured.
#[allow(clippy::too_many_arguments)]
fn fit_band_hues(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: PixelVouch<'_>,
    cells: &crate::fit_cells::PairedCells,
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
    let k = free.len();
    let mut normal = vec![vec![0.0f64; k + 1]; k];
    for (w, reach_r, want) in &equations {
        for (r, &cr) in free.iter().enumerate() {
            for (c, &cc) in free.iter().enumerate() {
                normal[r][c] += w * reach_r[cr] * reach_r[cc];
            }
            normal[r][k] += w * reach_r[cr] * want;
        }
    }
    // Gaussian elimination with partial pivoting; a band whose pixels the
    // mixer cannot reach (no pivot) leaves the solve unanswerable.
    for col in 0..k {
        let pivot = (col..k)
            .max_by(|&a, &b| normal[a][col].abs().total_cmp(&normal[b][col].abs()))
            .unwrap_or(col);
        normal.swap(col, pivot);
        if normal[col][col].abs() <= f64::EPSILON * total {
            return;
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
    let mut amount = [0.0f32; B];
    for (r, &band) in free.iter().enumerate() {
        amount[band] = round1(((normal[r][k] / normal[r][r]) as f32).clamp(-budget.hsl_band, budget.hsl_band));
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
    let reached: Vec<f32> = before
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
            let within = match render::hsl_band_reach(h, render::chroma(p)) {
                Some((b0, b1, w1, _)) => {
                    (if amount[b0] != 0.0 { 1.0 - w1 } else { 0.0 }) + if amount[b1] != 0.0 { w1 } else { 0.0 }
                }
                None => 0.0,
            };
            within * evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0)
        })
        .collect();
    let set = |recipe: &mut EditRecipe, share: f32| {
        recipe.hsl.hue = std::array::from_fn(|b| round1((restore[b] + amount[b] * share).clamp(-100.0, 100.0)));
    };
    let vouch_at = |recipe: &mut EditRecipe, share: f32| {
        set(recipe, share);
        let px = pixels_of(&render::develop_preview(s_img, recipe));
        Some(cells.vouch_along(crate::fit_cells::MoveAxis::Colour, &before, &px, Some(&reached)))
    };
    let full = vouch_at(recipe, 1.0);
    let Some(share) = largest_vouched_share(full, |share| vouch_at(recipe, share)) else {
        recipe.hsl.hue = restore;
        return;
    };
    set(recipe, share);

    // --- do-no-harm, the stage's own, on the hue it added ------------------
    let err_before = look_err_with_evidence(&before, tp, evidence);
    let blind_before =
        moved_unsupported_hue_range_names_vouched(sp, &before, evidence, Some(vouch), None).is_some();
    for _ in 0..super::solve::DO_NO_HARM_MAX_ROUNDS {
        if recipe.hsl.hue == restore {
            return;
        }
        let px = pixels_of(&render::develop_preview(s_img, recipe));
        let regressed = look_err_with_evidence(&px, tp, evidence) > err_before + 1e-4;
        let blind_new = budget.vetoes == VetoPolicy::Withhold
            && !blind_before
            && moved_unsupported_hue_range_names_vouched(sp, &px, evidence, Some(vouch), None).is_some();
        if !regressed && !blind_new {
            return;
        }
        recipe.hsl.hue = hue_halved_toward(&recipe.hsl.hue, &restore);
    }
    recipe.hsl.hue = restore;
}

/// One shrink step of the band hues toward `start` (the hue the stage found):
/// halve each band's rotation and snap one under a unit to `start`, so the
/// shrink REACHES it — [`halved_hsl`]'s device, on the one axis and toward
/// the one state the band-hue judgements compare against.
pub(super) fn hue_halved_toward(hue: &[f32; 8], start: &[f32; 8]) -> [f32; 8] {
    std::array::from_fn(|b| {
        let half = round1((hue[b] - start[b]) * 0.5);
        if half.abs() < 1.0 { start[b] } else { start[b] + half }
    })
}

/// The stage's do-no-harm control, separated from its measurements so its
/// termination is testable: halve `fitted` while `objects` names a reason,
/// and hand back the first candidate it does not object to. `Err` carries
/// the last objection when the halving reaches neutral, or when
/// [`super::solve::DO_NO_HARM_MAX_ROUNDS`] candidates have been judged —
/// the 4b loop's bound, for the same reason: a band at the budget ceiling
/// reaches neutral in eight halvings, so a mixer that needs more is not
/// descending. Since `halved_hsl` halves hue too no mixer is a fixed point
/// of the halving, and the bound is a guard. Either way the caller
/// withdraws to the state the stage started from.
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
