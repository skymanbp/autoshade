//! The hue gates as applied: foreign hues, rehued shares, rotation, the hue fan, and unsupported range movement.

use super::*;

/// The set of 15°-hue bins FOREIGN to the target: farther than
/// [`VETO_FAR_BINS`] bins (circularly) from every bin holding ≥
/// [`VETO_SUPPORT_BIN_MIN`] of the target's chromatic mass. `None` when the
/// target has fewer than [`VETO_MIN_TARGET_CHROMATIC`] chromatic pixels — no
/// reliable hue testimony, the veto stands down.
pub(super) fn foreign_hue_bins(tp: &[[f32; 3]]) -> Option<[bool; 24]> {
    let mut mass = [0.0f32; 24];
    let mut n = 0usize;
    for p in tp {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < VETO_SUPPORT_CHROMA {
            continue;
        }
        let (h, _s, _l) = render::rgb_to_hsl(p[0], p[1], p[2]);
        mass[((h * 24.0) as usize).min(23)] += 1.0;
        n += 1;
    }
    if n < VETO_MIN_TARGET_CHROMATIC {
        return None;
    }
    let populated: Vec<usize> =
        (0..24).filter(|&k| mass[k] / n as f32 >= VETO_SUPPORT_BIN_MIN).collect();
    let mut foreign = [true; 24];
    for (k, f) in foreign.iter_mut().enumerate() {
        for &p in &populated {
            let fwd = (k as isize - p as isize).rem_euclid(24) as usize;
            if fwd.min(24 - fwd) <= VETO_FAR_BINS {
                *f = false;
                break;
            }
        }
    }
    Some(foreign)
}

/// Fraction of the frame visibly tinted at a hue foreign to the target
/// (chroma ≥ [`VETO_TINT_CHROMA`], hue in a foreign bin).
pub(super) fn foreign_share(px: &[[f32; 3]], foreign: &[bool; 24]) -> f32 {
    let mut cnt = 0usize;
    for p in px {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < VETO_TINT_CHROMA {
            continue;
        }
        let (h, _s, _l) = render::rgb_to_hsl(p[0], p[1], p[2]);
        if foreign[((h * 24.0) as usize).min(23)] {
            cnt += 1;
        }
    }
    cnt as f32 / px.len().max(1) as f32
}

/// Did a global cast transform paint a REGION of the frame in hues the target holds
/// nowhere (≥ [`VETO_FAR_BINS`]·15° from all its populated hue mass)?
/// `cur`/`with_px` render the SAME source, so the share DELTA is exactly the
/// transform's own work — pre-existing content mismatch cancels out. Full-mode
/// channel curves and Atmosphere white balance intentionally share this law.
pub(super) fn cast_paints_foreign_hues(cur: &[[f32; 3]], with_px: &[[f32; 3]], tp: &[[f32; 3]]) -> bool {
    let Some(foreign) = foreign_hue_bins(tp) else {
        return false;
    };
    foreign_share(with_px, &foreign) - foreign_share(cur, &foreign) >= VETO_CREATED_SHARE
}

pub(super) fn foreign_hue_bins_weighted(
    tp: &[[f32; 3]],
    weights: &[f32],
) -> Option<[bool; 24]> {
    let mut mass = [0.0f32; 24];
    let mut total = 0.0f32;
    for (p, &weight) in tp.iter().zip(weights) {
        let weight = weight.max(0.0);
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if weight <= 0.0 || chroma < VETO_SUPPORT_CHROMA {
            continue;
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        mass[((h * 24.0) as usize).min(23)] += weight;
        total += weight;
    }
    if total < VETO_MIN_TARGET_CHROMATIC as f32 {
        return None;
    }
    let populated: Vec<usize> =
        (0..24).filter(|&bin| mass[bin] / total >= VETO_SUPPORT_BIN_MIN).collect();
    let mut foreign = [true; 24];
    for (bin, is_foreign) in foreign.iter_mut().enumerate() {
        for &populated_bin in &populated {
            let forward = (bin as isize - populated_bin as isize).rem_euclid(24) as usize;
            if forward.min(24 - forward) <= VETO_FAR_BINS {
                *is_foreign = false;
                break;
            }
        }
    }
    Some(foreign)
}

pub(super) fn cast_paints_foreign_hues_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    foreign_created_share_weighted(cur, with_px, tp, evidence)
        .is_some_and(|created| created >= VETO_CREATED_SHARE)
}

/// The foreign-hue population share the curves CREATE (with − without), or
/// `None` when the target carries no reliable hue evidence — the measurement
/// behind [`cast_paints_foreign_hues_weighted`], exposed so an ADMITTED cast
/// can disclose the reading that let it through.
pub(super) fn foreign_created_share_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<f32> {
    // No paired-convergence exemption here either — painting hue mass the
    // target holds nowhere is capability policy like the rotation gate; the
    // vanished-population case it guards (canyon) is content divergence the
    // voucher must never launder.
    let foreign = foreign_hue_bins_weighted(tp, &evidence.target_hue_weights)?;
    let weighted_foreign = |px: &[[f32; 3]]| -> f32 {
        let mut hit = 0.0;
        let mut total = 0.0;
        for (i, p) in px.iter().enumerate() {
            let w = evidence.source_hue_weights.get(i).copied().unwrap_or(0.0).max(0.0);
            if w <= 0.0 { continue; }
            total += w;
            let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
            if chroma < VETO_TINT_CHROMA { continue; }
            let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
            if foreign[((h * 24.0) as usize).min(23)] { hit += w; }
        }
        hit / total.max(1e-6)
    };
    Some(weighted_foreign(with_px) - weighted_foreign(cur))
}

/// Frame share of RE-HUED pixels: a MEASURABLE hue before (chroma ≥
/// [`ROT_HUE_MEASURABLE_CHROMA`]), a visible tint after (chroma ≥
/// [`VETO_TINT_CHROMA`]), landing ≥ [`ROT_DEG`] of circular hue away — and
/// VISIBLE on at least one end (see [`ROT_VISIBLE_BEFORE`]): a sub-visible
/// tint flipped into another sub-visible tint is cast-inversion
/// pass-through, not a re-hue. Pixel-aligned: `cur`/`with_px` render the
/// SAME source, so per-pixel hue movement is exact. De-tinting (end chroma
/// under the gate) is exempt — removing colour is what a corrective cast
/// does. Exposed separately from the boolean gate so the pin test measures
/// the same census the gate uses.
#[cfg(test)]
pub(super) fn rehued_share(cur: &[[f32; 3]], with_px: &[[f32; 3]]) -> f32 {
    let mut cnt = 0usize;
    for (c, w) in cur.iter().zip(with_px) {
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = w[0].max(w[1]).max(w[2]) - w[0].min(w[1]).min(w[2]);
        if cc < ROT_HUE_MEASURABLE_CHROMA || wc < VETO_TINT_CHROMA {
            continue;
        }
        if cc < ROT_VISIBLE_BEFORE && wc < ROT_VISIBLE_AFTER {
            continue; // invisible on both ends: pass-through, not a re-hue
        }
        let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0 * 360.0;
        let h1 = render::rgb_to_hsl(w[0], w[1], w[2]).0 * 360.0;
        let mut d = (h1 - h0).abs() % 360.0;
        if d > 180.0 {
            d = 360.0 - d;
        }
        if d >= ROT_DEG {
            cnt += 1;
        }
    }
    cnt as f32 / cur.len().max(1) as f32
}

/// Did the curves re-hue a REGION ([`ROT_SHARE`] of the frame)? See
/// [`rehued_share`] and the rotation-budget const block.
#[cfg(test)]
pub(super) fn cast_rotates_a_region(cur: &[[f32; 3]], with_px: &[[f32; 3]]) -> bool {
    rehued_share(cur, with_px) >= ROT_SHARE
}

pub(super) fn cast_rotates_a_region_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    // Deliberately NO paired-convergence exemption here (unlike the
    // zero-evidence-band guard): rotating a region is a TOOL-CAPABILITY
    // policy, not a measurability question — even a rotation that converges
    // on the analysis raster is the HUE axis's job (which nothing solves:
    // stage 4a fits saturation and luminance only), and a global cast that
    // performs it drags every same-hue pixel the raster never sampled
    // (golden-sky case, pinned).
    rehued_share_weighted(cur, with_px, evidence) >= ROT_SHARE
}

/// WB-specific foreign-hue check. A source-only hue can already be foreign in
/// the target, so a frame-share delta alone would cancel it out; count only
/// pixels whose WB render both moves substantially and lands in that foreign
/// hue population.
pub(super) fn wb_moves_pixels_into_foreign_hues(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    tp: &[[f32; 3]],
) -> bool {
    let Some(foreign) = foreign_hue_bins(tp) else { return false };
    let mut moved = 0usize;
    for (before, after) in cur.iter().zip(with_px) {
        let before_chroma = before[0].max(before[1]).max(before[2])
            - before[0].min(before[1]).min(before[2]);
        let after_chroma = after[0].max(after[1]).max(after[2])
            - after[0].min(after[1]).min(after[2]);
        if before_chroma < ROT_HUE_MEASURABLE_CHROMA || after_chroma < VETO_TINT_CHROMA {
            continue;
        }
        let (h0, _, _) = render::rgb_to_hsl(before[0], before[1], before[2]);
        let (h1, _, _) = render::rgb_to_hsl(after[0], after[1], after[2]);
        let mut delta = (h1 - h0).abs() * 360.0;
        if delta > 180.0 { delta = 360.0 - delta; }
        let after_bin = ((h1 * 24.0) as usize).min(23);
        if delta >= ROT_DEG && foreign[after_bin] {
            moved += 1;
        }
    }
    moved as f32 / cur.len().max(1) as f32 >= VETO_CREATED_SHARE
}

/// Weighted share of the source population visibly re-hued by a transform.
/// This is the exact census used by the weighted rotation gate and by the
/// strength-gated white-balance guard.
pub(super) fn rehued_share_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> f32 {
    let mut hit = 0.0f32;
    let mut total = 0.0f32;
    for (i, (c, wpx)) in cur.iter().zip(with_px).enumerate() {
        let weight = evidence.source_hue_weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if weight <= 0.0 { continue; }
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = wpx[0].max(wpx[1]).max(wpx[2]) - wpx[0].min(wpx[1]).min(wpx[2]);
        total += weight;
        if cc >= ROT_HUE_MEASURABLE_CHROMA && wc >= VETO_TINT_CHROMA {
            let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0 * 360.0;
            let h1 = render::rgb_to_hsl(wpx[0], wpx[1], wpx[2]).0 * 360.0;
            let mut d = (h1 - h0).abs() % 360.0;
            if d > 180.0 { d = 360.0 - d; }
            if d >= ROT_DEG { hit += weight; }
        }
    }
    hit / total.max(1e-6)
}

/// The worst hue CLASS the curves FANNED APART across luminance, as (that
/// class's share of the weighted population, added spread in degrees).
/// Exposed separately from the gate so the pin tests read the same census
/// the gate uses.
///
/// The census population is EXACTLY [`rehued_share_weighted`]'s — a
/// measurable hue before ([`ROT_HUE_MEASURABLE_CHROMA`]), a visible tint
/// after ([`VETO_TINT_CHROMA`]), evidence-weighted — so the two gates judge
/// the same pixels and can never be retuned into disagreeing about WHICH
/// population is being read. The QUESTION is the different one: not "how far
/// did each pixel travel" but "did one hue class arrive at several different
/// hues, sorted by luminance".
///
/// Slices are the evidence model's own luma bins, and a class's verdict is
/// the widest circular gap between the mean hues its populated slices land
/// on, MINUS the gap they started from — so a class that was ALREADY fanned
/// (content, not the curves' doing) contributes nothing, and a class the
/// curves rotate rigidly (a real global cast correction: every slice moves
/// together) reads zero however far it moves. That subtraction is what makes
/// this a capability gate rather than a second rotation budget.
///
/// A CLASS IS A HUE POPULATION, NOT A REGION. Cornwall's convicted class
/// holds 0.917 of the census population — that is the seascape's whole blue
/// class, sky AND sea, which the curves sort by luminance together; the
/// row-defined sky alone carries 0.561 of the hue weight. Prose that calls
/// the 0.917 "the sky" is naming the wrong population.
///
/// Returns, for the class the curves ADD the most spread to:
/// `(share, added, delivered)` — its share of the census population, the
/// spread the curves add to it (SIGNED: negative means they narrowed it),
/// and the ABSOLUTE spread it carries after the curves. The gate judges
/// `added`; `delivered` is what a viewer sees, and the two differ by the
/// baseline, which is bounded by one class width (see [`FAN_DEG`]'s worst
/// case).
pub(super) fn hue_fan_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(f32, f32, f32)> {
    let mut mass = [[0.0f32; EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut before = [[(0.0f32, 0.0f32); EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut after = [[(0.0f32, 0.0f32); EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut population = 0.0f32;
    for (i, (c, w)) in cur.iter().zip(with_px).enumerate() {
        let weight = evidence.source_hue_weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if weight <= 0.0 { continue; }
        population += weight;
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = w[0].max(w[1]).max(w[2]) - w[0].min(w[1]).min(w[2]);
        if cc < ROT_HUE_MEASURABLE_CHROMA || wc < VETO_TINT_CHROMA { continue; }
        let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0;
        let h1 = render::rgb_to_hsl(w[0], w[1], w[2]).0;
        let class = ((h0 * FAN_HUE_CLASSES as f32) as usize).min(FAN_HUE_CLASSES - 1);
        let bin = evidence_luma_bin(luma601(c));
        mass[class][bin] += weight;
        let (a0, a1) = (h0 * std::f32::consts::TAU, h1 * std::f32::consts::TAU);
        before[class][bin].0 += a0.sin() * weight;
        before[class][bin].1 += a0.cos() * weight;
        after[class][bin].0 += a1.sin() * weight;
        after[class][bin].1 += a1.cos() * weight;
    }
    if population <= 0.0 {
        return None;
    }
    // Widest circular gap between the slice means. Pairwise because the set
    // is at most EVIDENCE_LUMA_BINS long and a circular "range" has no
    // cheaper honest definition.
    let spread = |values: &[f32]| -> f32 {
        let mut worst = 0.0f32;
        for (i, a) in values.iter().enumerate() {
            for b in &values[i + 1..] {
                let mut d = (b - a).abs() % 360.0;
                if d > 180.0 { d = 360.0 - d; }
                worst = worst.max(d);
            }
        }
        worst
    };
    let mean = |(sin, cos): (f32, f32)| sin.atan2(cos).to_degrees().rem_euclid(360.0);
    let mut worst: Option<(f32, f32, f32)> = None;
    for class in 0..FAN_HUE_CLASSES {
        let class_mass: f32 = mass[class].iter().sum();
        let share = class_mass / population;
        if share < FAN_SHARE { continue; }
        let (mut was, mut now) = (Vec::new(), Vec::new());
        for bin in 0..EVIDENCE_LUMA_BINS {
            if mass[class][bin] < class_mass * FAN_SHARE { continue; }
            was.push(mean(before[class][bin]));
            now.push(mean(after[class][bin]));
        }
        // One slice cannot fan: a class confined to a single luma bin has no
        // internal structure for the curves to sort.
        if was.len() < 2 { continue; }
        let delivered = spread(&now);
        let fan = delivered - spread(&was);
        if worst.is_none_or(|(_, seen, _)| fan > seen) {
            worst = Some((share, fan, delivered));
        }
    }
    worst
}

/// Does this FINISHED render sort a hue class apart across luminance past
/// [`FAN_DEG`], measured against the untouched base with the fan gate's own
/// census? `Some((share, fan))` when it does, `None` when it does not or when
/// the census abstains.
///
/// The gate one stage up judges a CANDIDATE — the cast curves against the
/// state they were fitted on — and it is a calibrated threshold applied at
/// one point in the pipeline. This reads the same census on the pair the user
/// actually sees: the recipe that is about to be handed back, against doing
/// nothing at all.
pub(super) fn delivered_fan_conviction(
    sp: &[[f32; 3]],
    end_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(f32, f32)> {
    hue_fan_weighted(sp, end_px, evidence)
        .filter(|&(_, fan, _)| fan > FAN_DEG)
        .map(|(share, fan, _)| (share, fan))
}

/// The TERMINAL delivered-fan check, and the structural half of a promise
/// v1.2.3 could only make as a calibration.
///
/// The fan gate convicts the cast stage's own candidate, and the do-no-harm
/// loop above re-fits that stage after every saturation step — so nothing in
/// the loop's arithmetic stops it walking to a state whose FINISHED render
/// fans a class past [`FAN_DEG`], one admissible step at a time. The FAN_DEG
/// = 20 experiment is that behaviour caught in the act: the loop halved
/// Aqua/Blue and refitted until a milder cast measured 19°, which shipped and
/// left 20.6° of fan in the delivered sky. A threshold nothing re-reads at
/// the end is a threshold the pipeline can walk around.
///
/// The remedy is the gate's own, and it is aimed at the gate's own subject.
/// Three independent monotone channel maps are the control the fan gate
/// exists for and the one the loop keeps re-fitting, so they are withdrawn
/// and the frame re-measured — the same "shrink until the finished frame
/// stops objecting" idiom the mixer and the saturation loop already use,
/// taken to its end in one step because curves have no smaller unit than
/// "present". If the reading clears, that recipe ships with the sentence
/// that says so.
///
/// If the reading survives the withdrawal the curves were NOT the cause, and
/// then the honest act is to say so rather than to keep taking the recipe
/// apart: the curves go back exactly as the loop left them (so a fit pays no
/// look error for a fan it did not open) and the delivered reading is
/// disclosed with both numbers. That case is real rather than defensive —
/// measured 2026-09-02, the `p36` calibration pair delivers 12.9° of added
/// fan carrying NO cast curves at all, so tone and saturation alone reach
/// most of the way to the line — and it is the reason this check withdraws
/// one named control instead of degrading the whole fit.
///
/// Returns `(share, fan, still)` when the delivered render is convicted: the
/// class's share, the fan it had, and the fan that survived withdrawing the
/// curves — `None` there means the withdrawal cleared it and the
/// curve-less recipe is what ships.
///
/// MEASURED 2026-09-02 by instrumenting this point and running the whole
/// library battery: 108 finished Full-mode renders, the widest delivered fan
/// among them 14.2° in a class holding 0.638 of the measurable colour (the
/// `coast` fixture, whose cast is projected), then `p36`'s 12.9° and the
/// two-temperature `p40` pair's 12.4°. So at [`FAN_DEG`] this check fires on
/// nothing in the tree — it costs one census per fit and changes no recipe —
/// which is what a structural guarantee looks like while the calibration
/// above it is doing its job. Its two arms are pinned by
/// `the_terminal_check_takes_the_curves_out_of_a_fanning_render` and
/// `a_delivered_fan_the_curves_did_not_cause_is_disclosed_not_withdrawn`, and
/// the standing margin by `no_shipped_fit_delivers_a_hue_fan_past_the_limit`.
pub(super) fn withdraw_curves_for_delivered_fan(
    s_img: &DynamicImage,
    sp: &[[f32; 3]],
    evidence: &EvidenceModel,
    recipe: &mut EditRecipe,
    end_px: &mut Vec<[f32; 3]>,
) -> Option<(f32, f32, Option<f32>)> {
    let (share, fan) = delivered_fan_conviction(sp, end_px, evidence)?;
    let mut without = recipe.clone();
    without.red_curve.clear();
    without.green_curve.clear();
    without.blue_curve.clear();
    let px = pixels_of(&render::develop_preview(s_img, &without));
    let still = delivered_fan_conviction(sp, &px, evidence).map(|(_, fan)| fan);
    if still.is_none() {
        *recipe = without;
        *end_px = px;
    }
    Some((share, fan, still))
}

pub(super) fn rehued_coverage_weighted(evidence: &EvidenceModel) -> f32 {
    evidence
        .source_hue_weights
        .iter()
        .copied()
        .map(|weight| weight.max(0.0))
        .sum::<f32>()
        / evidence.source_pixels.len().max(1) as f32
}

pub(crate) fn moves_unsupported_range(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    moved_unsupported_range_names(cur, with_px, evidence).is_some()
}

pub(crate) fn moved_unsupported_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(String, String)> {
    let hits = moved_unsupported_range_hits(cur, with_px, evidence, None, None);
    let (luma, hue) = (hits.luma, hits.hue);
    if luma.0 == 0.0 && hue.0 == 0.0 { return None; }
    let names = |hits: &[bool], ranges: &[EvidenceRange]| {
        hits.iter().zip(ranges).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", ")
    };
    Some((names(&luma.1, &evidence.luma), names(&hue.1, &evidence.hue)))
}

pub(crate) fn moved_unsupported_luma_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<String> {
    let luma = moved_unsupported_range_hits(cur, with_px, evidence, None, None).luma;
    if luma.0 == 0.0 { return None; }
    Some(luma.1.iter().zip(&evidence.luma).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", "))
}

pub(crate) fn moved_unsupported_hue_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<String> {
    moved_unsupported_hue_range_names_vouched(cur, with_px, evidence, None, None)
}

/// [`moved_unsupported_hue_range_names`] with a per-pixel robust voucher for
/// the evacuation exemption (see `moved_unsupported_range_hits`). The global
/// end-state guard, the mixer and the cast outcome supply one on a paired
/// run, and since 2026-10-21 so does the zone colour probe of a region that
/// pairs at pixel scale; every other caller keeps the strict doctrine.
pub(crate) fn moved_unsupported_hue_range_names_vouched(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> Option<String> {
    let hue = moved_unsupported_range_hits(cur, with_px, evidence, vouch, cells).hue;
    if hue.0 == 0.0 { return None; }
    Some(hue.1.iter().zip(&evidence.hue).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", "))
}

fn moved_unsupported_range_hits(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> MovedRangeHits {
    let mut moved_luma = 0.0f32;
    let mut moved_hue = 0.0f32;
    let mut luma = [false; EVIDENCE_LUMA_BINS];
    let mut hue = [false; EVIDENCE_HUE_BANDS];
    let mut vouched_hue = [false; EVIDENCE_HUE_BANDS];
    let mut cell_vouched_hue = [false; EVIDENCE_HUE_BANDS];
    for (i, (before, after)) in cur.iter().zip(with_px).enumerate() {
        let unsupported_luma = source_luma_is_withheld(i, evidence);
        let unsupported_hue = source_hue_is_withheld(i, evidence);
        let delta = (0..3).map(|channel| (after[channel] - before[channel]).abs()).sum::<f32>()
            / 3.0;
        if delta < UNSUPPORTED_RANGE_MOVE { continue; }
        let membership = evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0);
        if membership <= 0.0 { continue; }
        if unsupported_luma && let Some(pixel) = evidence.source_pixels.get(i) {
            moved_luma += membership;
            luma[evidence_luma_bin(luma601(pixel))] = true;
        }
        if unsupported_hue && let Some(pixel) = evidence.source_pixels.get(i) && let Some(band) = evidence_hue_band(pixel) {
            // VOUCHED convergence (the hue form of the luma rank-pairing
            // doctrine, gated by the robust fit): on a paired run, a pixel
            // whose transport residual the robust fit vouches for has its OWN
            // paired target pixel — moving it TOWARD that target is
            // convergence, not a blind move through an unmeasurable band
            // (measured: the haze pair's blue-cast pixels sat in source-only
            // Red/Blue, and un-casting them was vetoed by the very bands the
            // cast invented; a band-topology exemption then failed the same
            // pair again on pixels the cast's step caps left mid-way). A
            // content-divergent pixel (the canyon pair's vanished reds are
            // the reconstruction's doing, not an edit's) carries a large
            // transport residual, earns no voucher, and keeps its veto.
            // Callers with no paired verdict pass None and get the strict
            // doctrine unchanged. The zoned colour probe did so until
            // 2026-10-21 (zone colour stayed class-split withheld); the
            // user's ruling of that date hands it the zone's own robust
            // pairing where the region pairs at pixel scale, because a
            // one-sided band on such a region is band membership and not a
            // lack of evidence — the seaside street's source sky sits in
            // Aqua, the target's in Blue, and the zone shipped unity gains
            // over a sky whose every pixel had its counterpart. A Cell-scale
            // region still passes None and asks its cells. The voucher names
            // the AXIS its move is judged along (`PixelVouch`): a zone's
            // colour probe is read in chromaticity, a global move in full.
            let spatially = evidence.spatial_supported.get(i).copied().unwrap_or(false);
            let converges = spatially
                && vouch.is_some_and(|(weights, targets, axis)| {
                    weights.get(i).copied().unwrap_or(0.0) >= 0.5
                        && targets
                            .get(i)
                            .is_some_and(|target| converges_toward(axis, target, before, after))
                });
            // R34 §D3. THE cell arm, for the pixels the paired one cannot
            // be ASKED about — a strictly smaller set than the pixels it did
            // not vouch, and the difference is the whole policy. A pixel
            // whose texture SURVIVED has a counterpart; moving it away from
            // that counterpart is evidence against the move, never a reason
            // to ask its neighbours for a second opinion. Guarding on
            // `!converges` alone let a surviving region overrule its own
            // pixels: measured on the canyon-gold fixture, whose synthetic
            // pair is aligned to the pixel (D 0.028), the cells read
            // 0.921 / 0.079 / 0.981 and rotated a pale-blue sky 158° into the
            // target's native gold — the exact policy
            // `cast_must_not_rotate_the_sky_into_a_target_native_hue` exists
            // to refuse, and the sentence this arm prints ("those pixels are
            // re-synthesised and have no paired counterpart") would have been
            // false about every one of them.
            //
            // The reading is the PIXEL's OWN structural confidence against
            // `DIVERGENCE_GLOBAL` — the same line the mode split and the zone
            // tone estimator use — and deliberately NOT `spatially`.
            // `spatial_supported` is `globally_same_content || d <
            // DIVERGENCE_ZONE`: a frame that pairs overall marks every pixel
            // of a repainted sky supported, which is R33's "region is not
            // frame" inside the flag's own definition. The reference frame
            // reads 0.275, so `spatially` is true right across a sky whose own
            // reading is 0.617 and whose pixels are therefore not each other's
            // counterparts. Measured through this predicate rather than
            // through that flag, the arm still carries [Aqua, Blue] on the
            // reference pair (0.966 / 0.017 / 0.937) and is refused on the
            // canyon, which is the whole difference.
            let unpaired =
                evidence.spatial_weights.get(i).copied().unwrap_or(1.0) < 1.0 - DIVERGENCE_GLOBAL;
            let by_cells = unpaired
                && !converges
                && cells.is_some_and(|(grid, verdicts)| {
                    grid.cell_of(i)
                        .and_then(|cell| verdicts.get(cell).copied().flatten())
                        == Some(true)
                });
            if converges {
                vouched_hue[band] = true;
            } else if by_cells {
                cell_vouched_hue[band] = true;
            } else {
                moved_hue += membership;
                hue[band] = true;
            }
        }
    }
    // The region line is a share of the population the correction moves --
    // the frame for the global fit, a zone's coverage for a zone (a frame
    // share let every tile move its blind pixels: a depth-2 tile is 6% of
    // the frame, so its blind half never reached the 5% line).
    let total = evidence.population.max(1.0);
    if moved_luma / total < ROT_SHARE { moved_luma = 0.0; luma = [false; EVIDENCE_LUMA_BINS]; }
    if moved_hue / total < ROT_SHARE { moved_hue = 0.0; hue = [false; EVIDENCE_HUE_BANDS]; }
    MovedRangeHits { luma: (moved_luma, luma), hue: (moved_hue, hue), vouched_hue, cell_vouched_hue }
}

/// The per-range verdicts of one blind-move audit: which withheld ranges were
/// MOVED (the veto's subject) and which one-sided hue bands vouched
/// convergence carried movement THROUGH — the second list exists so the
/// disclosure can stop claiming "vetoed movement" about bands whose veto the
/// voucher lifted (E-15: two different outcomes must not read the same).
struct MovedRangeHits {
    luma: (f32, [bool; EVIDENCE_LUMA_BINS]),
    hue: (f32, [bool; EVIDENCE_HUE_BANDS]),
    vouched_hue: [bool; EVIDENCE_HUE_BANDS],
    /// R34 §D3: the bands the REGION's cells carried movement through, kept
    /// apart from `vouched_hue` because they are a different claim — "these
    /// pixels have no counterpart and their cell vouched for them" is not
    /// "each of these pixels was individually vouched", and a disclosure that
    /// printed one sentence for both would be saying something untrue about
    /// one of them.
    cell_vouched_hue: [bool; EVIDENCE_HUE_BANDS],
}

/// The per-pixel voucher: each pixel's robust pairing weight, its own paired
/// target pixel, and the axis the move it is asked about can move along
/// (`fit_cells::MoveAxis` — full RGB for a global cast or mixer, chromaticity
/// for a zone's colour probe solved with its tone zeroed).
pub(crate) type PixelVouch<'a> = (&'a [f32], &'a [[f32; 3]], crate::fit_cells::MoveAxis);

/// One candidate render's REGION verdicts, and the grid that indexes them:
/// `fit_cells`'s answer for the pixels the per-pixel voucher cannot reach.
/// Recomputed per candidate — it is a verdict about what an edit DID, so it
/// cannot outlive the edit it judged.
pub(crate) type CellArm<'a> = (&'a crate::fit_cells::PairedCells, &'a [Option<bool>]);

/// Names of the one-sided hue bands that vouched convergence moved pixels
/// through on the finished render — the disclosure's raw material.
pub(crate) fn vouched_hue_band_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> Option<String> {
    let hits = moved_unsupported_range_hits(cur, with_px, evidence, vouch, cells);
    band_names(&hits.vouched_hue, evidence)
}

/// R34 §D3. The other half of the same audit: the one-sided bands the
/// REGION's cells carried movement through. Two outcomes, two lists, two
/// sentences — see [`MovedRangeHits::cell_vouched_hue`].
pub(crate) fn cell_vouched_hue_band_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> Option<String> {
    let hits = moved_unsupported_range_hits(cur, with_px, evidence, vouch, cells);
    band_names(&hits.cell_vouched_hue, evidence)
}

fn band_names(hits: &[bool; EVIDENCE_HUE_BANDS], evidence: &EvidenceModel) -> Option<String> {
    let names = hits
        .iter()
        .zip(&evidence.hue)
        .filter_map(|(&hit, range)| hit.then_some(range.label.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    (!names.is_empty()).then_some(names)
}

pub(super) fn moves_unsupported_luma_range(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    let mut moved = 0.0f32;
    let mut eligible = 0.0f32;
    for (i, (before, after)) in cur.iter().zip(with_px).enumerate() {
        if !source_luma_is_withheld(i, evidence) {
            continue;
        }
        let membership = evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0);
        if membership <= 0.0 {
            continue;
        }
        eligible += membership;
        let before_luma = luma601(before);
        let after_luma = luma601(after);
        if (after_luma - before_luma).abs() >= UNSUPPORTED_RANGE_MOVE {
            moved += membership;
        }
    }
    eligible > 0.0 && moved / evidence.population.max(1.0) >= ROT_SHARE
}
