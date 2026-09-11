use super::*;
use crate::fit;

/// A 96 x 64 analysis pair: `source` is a luma ramp, `target` the same ramp
/// lifted by `left` on the left half and by `right` on the right half.
fn pair(left: f32, right: f32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, u32, u32) {
    let (w, h) = (96u32, 64u32);
    let mut source = Vec::with_capacity((w * h) as usize);
    let mut target = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let v = 0.25 + 0.4 * (x as f32 / (w - 1) as f32) + 0.1 * (y as f32 / (h - 1) as f32);
            source.push([v, v, v]);
            let shift = if x < w / 2 { left } else { right };
            let t = (v + shift).clamp(0.02, 0.98);
            target.push([t, t, t]);
        }
    }
    (source, target, w, h)
}

fn cells_of(source: &[[f32; 3]], target: &[[f32; 3]], w: u32, h: u32) -> PairedCells {
    let evidence = fit::evidence_model_for(source, target, w, h);
    PairedCells::build(target, w, h, &evidence).expect("a populated pair builds cells")
}

/// Lift every pixel by `by`.
fn lifted(source: &[[f32; 3]], by: f32) -> Vec<[f32; 3]> {
    source.iter().map(|p| p.map(|v| (v + by).clamp(0.0, 1.0))).collect()
}

/// The left half of the frame, as a soft region membership.
fn left_half(w: u32, h: u32) -> Vec<f32> {
    (0..(w * h) as usize).map(|i| f32::from((i as u32 % w) < w / 2)).collect()
}

/// The positive case: an edit that moves the whole frame toward its target is
/// vouched, and nothing is counted as having moved away.
#[test]
fn cells_vouch_an_edit_that_moves_the_region_toward_its_target() {
    let (source, target, w, h) = pair(0.12, 0.10);
    let cells = cells_of(&source, &target, w, h);
    let verdict = cells.vouch(&source, &lifted(&source, 0.08), None);
    assert!(verdict.read > 0, "the grid resolved");
    assert!(
        verdict.vouched(),
        "an edit that closes most of the gap everywhere must be vouched: {verdict:?}"
    );
    assert!(verdict.diverged <= 1e-6, "nothing moved away: {verdict:?}");
}

/// The negative case, and the one the instrument exists for: an edit of the
/// same SIZE in the wrong direction is refused, not read as "it moved, so it
/// must be evidence".
#[test]
fn cells_refuse_an_edit_of_the_same_size_in_the_wrong_direction() {
    let (source, target, w, h) = pair(0.12, 0.10);
    let cells = cells_of(&source, &target, w, h);
    let verdict = cells.vouch(&source, &lifted(&source, -0.08), None);
    assert!(!verdict.vouched(), "moving away from the target must not vouch: {verdict:?}");
    assert!(verdict.diverged > VOUCH_MAX_DIVERGED, "…and it is counted: {verdict:?}");
}

/// An identity edit moves nothing, so nothing converged — the verdict is a
/// refusal, never "no harm done, therefore vouched".
#[test]
fn an_edit_that_changes_nothing_is_not_vouched() {
    let (source, target, w, h) = pair(0.12, 0.10);
    let cells = cells_of(&source, &target, w, h);
    let verdict = cells.vouch(&source, &source, None);
    assert_eq!(verdict.converged, 0.0, "nothing moved closer");
    assert_eq!(verdict.diverged, 0.0, "…and nothing moved away either");
    assert_eq!(verdict.aligned, 0.0, "…and a cell that wanted a move and got none is not aligned");
    assert!(!verdict.vouched(), "a no-op is not a vouch: {verdict:?}");
}

/// A 96 x 64 pair whose source is a neutral ramp and whose target is the same
/// ramp with its RED channel lifted: a recolour whose layout is intact and
/// whose direction, in every cell, is the same.
fn warm_pair(lift: f32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, u32, u32) {
    let (w, h) = (96u32, 64u32);
    let mut source = Vec::with_capacity((w * h) as usize);
    let mut target = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let v = 0.25 + 0.4 * (x as f32 / (w - 1) as f32) + 0.1 * (y as f32 / (h - 1) as f32);
            source.push([v, v, v]);
            target.push([(v + lift).clamp(0.02, 0.98), v, v]);
        }
    }
    (source, target, w, h)
}

/// R34 §D1, and the reason the direction test exists: "closer" and "toward"
/// are different questions, and a region-scale gain can satisfy the first
/// while being no part of the recolour the target asks for.
///
/// The target wants RED. This edit lifts all three channels, green and blue
/// hardest: the Chebyshev distance really does shrink in every cell (the red
/// gap halves while the green and blue gaps stay under it), so the pre-R34
/// voucher would have carried it — and it is a desaturating lift, not a
/// warming one.
#[test]
fn a_move_that_shrinks_the_distance_sideways_is_refused_by_the_direction_test() {
    let (source, target, w, h) = warm_pair(0.20);
    let cells = cells_of(&source, &target, w, h);
    let after: Vec<[f32; 3]> = source
        .iter()
        .map(|p| [p[0] + 0.10, p[1] + 0.16, p[2] + 0.16])
        .collect();
    let verdict = cells.vouch(&source, &after, None);
    assert!(
        verdict.converged >= VOUCH_MIN_CONVERGED,
        "premise: the distance ruler alone would have carried this: {verdict:?}"
    );
    assert!(
        verdict.aligned < VOUCH_MIN_ALIGNED,
        "…and the direction ruler refuses it: {verdict:?}"
    );
    assert!(!verdict.vouched(), "so the verdict is a refusal: {verdict:?}");
    // The SAME size of move, spent where the target asked for it, is vouched.
    let warm: Vec<[f32; 3]> = source.iter().map(|p| [p[0] + 0.16, p[1], p[2]]).collect();
    assert!(
        cells.vouch(&source, &warm, None).vouched(),
        "the warming the target asks for is admitted"
    );
}

/// R34 §D1, the circularity answer. A zone-wide gain is solved from the zone's
/// MEAN, so a gain that matches the mean exactly is the best that estimator can
/// do — and the finer partition still refuses it when the region's own cells
/// wanted different things. The two are not one quantity.
#[test]
fn a_gain_that_matches_the_regions_mean_is_refused_by_the_regions_cells() {
    // Left half wants +0.16, right half wants nothing: the region mean is +0.08.
    let (source, target, w, h) = pair(0.16, 0.0);
    let cells = cells_of(&source, &target, w, h);
    let after = lifted(&source, 0.08);
    let verdict = cells.vouch(&source, &after, None);
    assert!(
        verdict.converged < VOUCH_MIN_CONVERGED,
        "the half that wanted nothing does not converge: {verdict:?}"
    );
    assert!(
        verdict.diverged > VOUCH_MAX_DIVERGED,
        "…it is dragged away, and the verdict counts it: {verdict:?}"
    );
    assert!(!verdict.vouched(), "a matched mean is not a matched region: {verdict:?}");
}

/// A region already AT its target asks for no move, so leaving it alone is
/// aligned — without that arm every matched cell would vote against an edit
/// that correctly did nothing to it.
#[test]
fn a_cell_already_at_its_target_is_aligned_by_being_left_alone() {
    let (source, _, w, h) = pair(0.0, 0.0);
    let cells = cells_of(&source, &source, w, h);
    let still = cells.vouch(&source, &source, None);
    assert!(still.read > 0, "premise: the grid resolved");
    assert_eq!(still.aligned, 1.0, "leaving a matched cell alone is aligned: {still:?}");
    assert_eq!(still.converged, 0.0, "…and there was nothing to converge");
    let moved = cells.vouch(&source, &lifted(&source, 0.08), None);
    assert_eq!(moved.aligned, 0.0, "moving a matched cell is not: {moved:?}");
}

/// The per-cell view and the region view are two readings of ONE pass, so a
/// pixel can never be admitted by a cell the region verdict counted the other
/// way. Pinned both ways round.
#[test]
fn the_per_cell_verdicts_are_the_same_measurement_as_the_region_verdict() {
    let (source, target, w, h) = pair(0.12, -0.12);
    let cells = cells_of(&source, &target, w, h);
    let after = lifted(&source, 0.08);
    let verdicts = cells.verdicts(&source, &after, None);
    assert_eq!(verdicts.len(), CELLS_X * CELLS_Y, "one entry per cell");
    let frame = cells.vouch(&source, &after, None);
    assert_eq!(
        verdicts.iter().filter(|v| v.is_some()).count(),
        frame.read,
        "the cells the region read are exactly the cells that answered"
    );
    // The left half is helped and the right half hurt, so both verdicts appear
    // and the split follows the region's own geometry.
    let left = (0..CELLS_X * CELLS_Y)
        .filter(|c| c % CELLS_X < CELLS_X / 2)
        .filter(|c| verdicts[*c] == Some(true))
        .count();
    let right = (0..CELLS_X * CELLS_Y)
        .filter(|c| c % CELLS_X >= CELLS_X / 2)
        .filter(|c| verdicts[*c] == Some(true))
        .count();
    assert!(left > 0 && right == 0, "left {left} admitted, right {right} admitted");
    for (i, expected) in [(0usize, true), ((CELLS_X - 1), false)] {
        let cell = cells.cell_of(if expected { 0 } else { (w - 1) as usize })
            .expect("a pixel of the analysis raster sits in a cell");
        assert_eq!(cell % CELLS_X, i, "cell_of indexes the same grid verdicts does");
    }
}

/// The denominator is the WHOLE region, not the cells the edit happened to
/// touch: an edit that is right where it acts but leaves most of the region
/// where it was has not earned the region's verdict.
#[test]
fn an_edit_that_reaches_half_the_region_does_not_carry_the_whole_region() {
    let (source, target, w, h) = pair(0.12, 0.0);
    let cells = cells_of(&source, &target, w, h);
    let left = left_half(w, h);
    let after: Vec<[f32; 3]> = source
        .iter()
        .zip(&left)
        .map(|(p, &member)| p.map(|v| v + 0.08 * member))
        .collect();
    let frame = cells.vouch(&source, &after, None);
    assert!(
        !frame.vouched() && frame.converged < VOUCH_MIN_CONVERGED,
        "half a frame's worth of convergence is not the frame's verdict: {frame:?}"
    );
    assert!(frame.diverged <= 1e-6, "…and it did no harm either: {frame:?}");
    assert!(
        cells.vouch(&source, &after, Some(&left)).vouched(),
        "the half it DID reach vouches it"
    );
}

/// The region argument is what lets a ZONE be judged on its own cells: the
/// same global edit is vouched over the half it improves and refused over the
/// half it damages.
#[test]
fn a_region_is_judged_on_its_own_cells_and_no_others() {
    let (source, target, w, h) = pair(0.12, -0.12);
    let cells = cells_of(&source, &target, w, h);
    let after = lifted(&source, 0.08);
    let left = left_half(w, h);
    let right: Vec<f32> = left.iter().map(|v| 1.0 - v).collect();
    let on_left = cells.vouch(&source, &after, Some(&left));
    let on_right = cells.vouch(&source, &after, Some(&right));
    assert!(on_left.vouched(), "the half it helps vouches it: {on_left:?}");
    assert!(!on_right.vouched(), "the half it hurts does not: {on_right:?}");
    assert!(on_right.diverged > VOUCH_MAX_DIVERGED, "…and says so: {on_right:?}");
}

/// An empty region is an ABSTENTION (`read == 0`), which is never a vouch —
/// the same doctrine `structure_divergence` states for its own `None`.
#[test]
fn an_unread_region_abstains_and_never_vouches() {
    let (source, target, w, h) = pair(0.12, 0.10);
    let cells = cells_of(&source, &target, w, h);
    let nothing = vec![0.0f32; (w * h) as usize];
    let verdict = cells.vouch(&source, &source, Some(&nothing));
    assert_eq!(verdict.read, 0, "no cell carried any of this region");
    assert!(!verdict.vouched(), "an abstention is not a vouch: {verdict:?}");
}

/// Determinism: the same inputs give the same verdict, because a vouch decides
/// whether a control ships and a wobbling admission is not an admission.
#[test]
fn the_verdict_is_deterministic() {
    let (source, target, w, h) = pair(0.12, 0.10);
    let cells = cells_of(&source, &target, w, h);
    let after = lifted(&source, 0.05);
    assert_eq!(cells.vouch(&source, &after, None), cells.vouch(&source, &after, None));
}
