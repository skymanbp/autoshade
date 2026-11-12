use super::*;
use crate::fit_field::{SolveInfo, LocalField};
use crate::fit_field::tests::{plant, ramp};

fn synthetic_field(width: u32, height: u32, remainder: Vec<f32>) -> LocalField {
    LocalField {
        grid: vec![[0.0; 5]; 12 * 8 * 8],
        occupancy: vec![8.0; 12 * 8 * 8],
        ceiling: 0.0,
        global: 1.0,
        band_marginal: [[0.0; 5]; 8],
        band_dispersion: [0.0; 8],
        weight: vec![1.0; remainder.len()],
        remainder,
        saturated: 0,
        solve: SolveInfo { iterations: 0, relative_residual: 0.0 },
        width,
        height,
        zones: None,
        frame: crate::render::FieldFrame::default(),
    }
}

#[test]
fn field_band_proposal_matches_a_two_band_remap() {
    let (width, height) = (144usize, 96usize);
    let current = ramp(width, height, 0.03, 0.95, 0x5eed_0001);
    let target = plant(&current, width, height,
        |_, guide| if guide < 0.5 { 0.15 } else { -0.15 });
    let (width, height) = (width as u32, height as u32);
    let evidence = fit::evidence_model_for(&current, &target, width, height);
    let field = LocalField::solve(&current, &target, width, height, &evidence).unwrap();
    let reading = read_shape(&field, &current, &target, &evidence);
    assert_eq!(reading.proposals.len(), 2, "{reading:?}");
    assert!(reading.proposals[0].sign > 0.0, "{reading:?}");
    assert!(reading.proposals[1].sign < 0.0, "{reading:?}");
    assert!(field.band_dispersion[1..].iter().all(|&d| d < 10.0 / 255.0));
}

#[test]
fn field_band_proposal_skips_a_spatially_structured_bin() {
    let (width, height) = (144usize, 96usize);
    let current = ramp(width, height, 0.30, 0.62, 0x5eed_1002);
    let target = plant(&current, width, height,
        |i, _| if i % width < width / 2 { 0.5 } else { -0.5 });
    let evidence = fit::evidence_model_for(&current, &target, width as u32, height as u32);
    let field = LocalField::solve(
        &current, &target, width as u32, height as u32, &evidence,
    ).unwrap();
    let reading = read_shape(&field, &current, &target, &evidence);
    let bin = (1..8).max_by(|&a, &b| field.band_dispersion[a]
        .total_cmp(&field.band_dispersion[b])).unwrap();
    assert!(field.band_dispersion[bin] > BAND_DISPERSION_MAX);
    assert!(reading.structured_bins.contains(&bin));
    let (lo, hi) = field_span(bin);
    assert!(reading.proposals.iter().all(|p| p.hi <= lo || p.lo >= hi), "{reading:?}");
}

/// A neutral pair whose evidence model lets `read_shape` run on a synthetic
/// remainder: the band marginals are zero, so no proposal survives and the
/// verdict is the remainder's shape alone.
fn shape_probe(width: u32, height: u32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, fit::EvidenceModel) {
    let current = ramp(width as usize, height as usize, 0.10, 0.90, 0x5eed_2003);
    let target = current.clone();
    let evidence = fit::evidence_model_for(&current, &target, width, height);
    (current, target, evidence)
}

#[test]
fn field_shape_reads_a_bright_quadrant_as_tile_shaped() {
    let (width, height) = (64u32, 48u32);
    let remainder = (0..width * height).map(|i| {
        let (x, y) = (i % width, i / width);
        if x >= width / 2 && y < height / 2 { 1.0 } else { 0.0 }
    }).collect();
    let field = synthetic_field(width, height, remainder);
    let (current, target, evidence) = shape_probe(width, height);
    let reading = read_shape(&field, &current, &target, &evidence);
    assert!(reading.r2_tiles >= TILE_SHAPE_MIN, "{reading:?}");
    assert_eq!(reading.shape, FieldShape::TileShaped, "{reading:?}");
    assert_eq!(reading.effective_tile_cap, SPATIAL_MAX_ATTACHMENTS, "{reading:?}");
}

/// The same bright quadrant, but every pixel inside it carries zero fit weight
/// (no evidence / no support / clipped): an unmeasured region is not structure.
#[test]
fn field_shape_ignores_unmeasured_pixels() {
    let (width, height) = (64u32, 48u32);
    let quadrant = |i: u32| { let (x, y) = (i % width, i / width); x >= width / 2 && y < height / 2 };
    let remainder = (0..width * height).map(|i| if quadrant(i) { 1.0 } else { 0.0 }).collect();
    let mut field = synthetic_field(width, height, remainder);
    field.weight = (0..width * height).map(|i| if quadrant(i) { 0.0 } else { 1.0 }).collect();
    let (current, target, evidence) = shape_probe(width, height);
    let reading = read_shape(&field, &current, &target, &evidence);
    assert_eq!(reading.r2_tiles, 0.0, "{reading:?}");
    assert_eq!(reading.shape, FieldShape::None, "{reading:?}");
}

#[test]
fn field_shape_reads_a_diagonal_ramp_as_linear() {
    let (width, height) = (64u32, 48u32);
    let remainder = (0..width * height).map(|i| {
        (i % width) as f32 / (width - 1) as f32 + (i / width) as f32 / (height - 1) as f32
    }).collect();
    let field = synthetic_field(width, height, remainder);
    let (current, target, evidence) = shape_probe(width, height);
    let reading = read_shape(&field, &current, &target, &evidence);
    assert!(reading.r2_linear >= LINEAR_SHAPE_MIN, "{reading:?}");
    assert!(reading.r2_tiles < TILE_SHAPE_MIN, "{reading:?}");
    assert_eq!(reading.shape, FieldShape::Linear, "{reading:?}");
    assert_eq!(reading.effective_tile_cap, 2, "{reading:?}");
}

/// A 192x128 pair whose RIGHT half was repainted: the same slow base, an
/// independent per-pixel texture draw on each side of it (so `local_support`
/// there is ~0, which is the starvation R34 §D4 cures), and a warm recolour.
/// The left half is byte-identical on both sides.
fn repainted_right_half(target: bool) -> image::DynamicImage {
    let (w, h) = (192u32, 128u32);
    let hash = |i: u32, seed: u32| {
        let mut v = i.wrapping_mul(747796405).wrapping_add(seed.wrapping_mul(2891336453));
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 10_000) as f32 / 10_000.0 - 0.5
    };
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        let base = 0.32 + 0.26 * (y as f32 / (h - 1) as f32);
        let repainted = x >= w / 2;
        let seed = if repainted && target { 9_999 } else { 1 };
        let v = (base + 0.12 * hash(y * w + x, seed)).clamp(0.05, 0.95);
        let p = if repainted && target {
            [(v * 1.35).min(0.98), v, (v * 0.72).max(0.02)]
        } else {
            [v, v, v]
        };
        image::Rgb(p.map(|c| (c * 255.0).round() as u8))
    }))
}

fn field_probe(
    src: &image::DynamicImage, tgt: &image::DynamicImage,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, u32, u32, FitReport) {
    let report = super::super::tests::neutral_report(src, tgt);
    let (s_img, t_img) = fit::analysis_pair(src, tgt);
    let (w, h) = (s_img.width(), s_img.height());
    (fit::pixels_of(&s_img), fit::pixels_of(&t_img), w, h, report)
}

/// R34 §D4. The support-free solve is not trusted, it is TESTED: the cells of
/// the repainted half vouch what their pixels cannot, so those cells — and no
/// others — take pass B's vertices.
#[test]
fn the_cells_of_a_repainted_region_admit_the_support_free_field() {
    let (src, tgt) = (repainted_right_half(false), repainted_right_half(true));
    let (current, target, w, h, report) = field_probe(&src, &tgt);
    let support = crate::fit_field::local_support(&current, &target, w, h);
    let mean = |right: bool| -> f32 {
        let picked: Vec<f32> = (0..(w * h) as usize)
            .filter(|i| (*i % w as usize >= w as usize / 2) == right)
            .map(|i| support[i])
            .collect();
        picked.iter().sum::<f32>() / picked.len().max(1) as f32
    };
    let (kept, repainted) = (mean(false), mean(true));
    // The starvation is RELATIVE and that is the whole point: the same solve
    // weights the untouched half at 0.92 and the repainted half at 0.24, so
    // the residual the repaint left is the part of the frame the analysis
    // solve is least able to answer.
    assert!(
        kept > 0.80 && repainted < 0.35 * kept,
        "premise: the repainted half is starved of structural support and the \
         untouched half is not (kept {kept:.3}, repainted {repainted:.3})"
    );
    let pass_a = LocalField::solve(&current, &target, w, h, &report.evidence)
        .expect("the analysis field solves");
    let (merged, admitted, read, _, by_axis) =
        admit_cells(&current, &target, w, h, &report.evidence, pass_a.clone(), 0.60, &[], None);
    assert!(read > 0, "the instrument was consulted: {admitted} of {read}");
    assert!(admitted > 0, "the repainted cells must be admitted: {admitted} of {read}");
    let moved: Vec<usize> = (0..crate::fit_cells::CELLS_X * crate::fit_cells::CELLS_Y)
        .filter(|cell| {
            crate::fit_field::cell_vertex_spans(*cell)
                .any(|span| merged.grid[span.clone()] != pass_a.grid[span])
        })
        .collect();
    assert_eq!(
        moved.len(),
        admitted + by_axis.cells(),
        "exactly the admitted cells took pass B, whole or by axis ({by_axis:?})"
    );
    assert!(
        moved.iter().all(|cell| cell % crate::fit_cells::CELLS_X >= crate::fit_cells::CELLS_X / 4),
        "…and they are the repainted ones, not the untouched left edge: {moved:?}"
    );
}

/// A Custom-role linear mask over the repainted right half, as the
/// multi-region route attaches one.
fn right_half_region() -> crate::recipe::LocalAdjustment {
    crate::recipe::LocalAdjustment {
        mask: crate::recipe::MaskGeometry::Linear { zero_x: 0.49, zero_y: 0.5, full_x: 0.51, full_y: 0.5 },
        name: "region-1".into(),
        role: crate::recipe::MaskRole::Custom,
        amount: 1.0,
        ..Default::default()
    }
}

/// J147. The admission ladder of one read cell, rung by rung. The whole of
/// pass B on its own verdict first; then the whole of it when it landed
/// closer and points toward the target along tone and along colour apart
/// (the measured cell: closer, 0.36 on the whole move); then one part alone
/// on its own axis — the closer landing when both are vouched; and nothing
/// for a cell no candidate takes closer.
/// MUTATION: drop the `vouched_by_axis` rung in `cell_take` — the second
/// assertion fails.
#[test]
fn the_admission_ladder_of_one_cell() {
    let reading = |converged: bool, landed: f32, whole: bool, tone: bool, colour: bool| CellMove {
        converged,
        landed,
        toward_whole: whole,
        toward_tone: tone,
        toward_colour: colour,
    };
    let refused = reading(false, 0.046, false, true, true);
    assert_eq!(cell_take(reading(true, 0.01, true, true, true), None, None), CellTake::Whole);
    assert_eq!(
        cell_take(reading(true, 0.045, false, true, true), None, None),
        CellTake::WholeByAxis,
        "closer and toward on each axis: the whole move is taken"
    );
    // The whole move overshot (not closer): a part alone, on its own axis.
    let colour_alone = Some(reading(true, 0.025, false, false, true));
    let tone_alone = Some(reading(true, 0.020, false, true, false));
    assert_eq!(cell_take(refused, None, colour_alone), CellTake::Colour);
    assert_eq!(cell_take(refused, tone_alone, None), CellTake::Tone);
    assert_eq!(cell_take(refused, tone_alone, colour_alone), CellTake::Tone, "the closer landing of the two");
    assert_eq!(
        cell_take(refused, Some(reading(true, 0.030, false, true, false)), colour_alone),
        CellTake::Colour,
        "the closer landing of the two"
    );
    // A part read on the WRONG axis, or one that did not land closer, is not taken.
    assert_eq!(cell_take(refused, colour_alone, tone_alone), CellTake::Nothing, "each part answers on its own axis");
    assert_eq!(
        cell_take(refused, Some(reading(false, 0.01, true, true, true)), Some(reading(false, 0.01, true, true, true))),
        CellTake::Nothing,
        "a cell nothing takes closer takes nothing"
    );
    assert_eq!(cell_take(refused, None, None), CellTake::Nothing);
}

/// J147. A composed vertex carries the tone of one vertex and the colour of
/// the other: its EV and slope and the level its gains share (their Rec.601
/// mean) are the tone's, its gains' offsets from that level the colour's.
/// Composing a vertex with itself returns it, and the gains stay inside the
/// bound.
#[test]
fn a_composed_vertex_carries_one_tone_and_the_other_colour() {
    let level = |v: &[f32; 5]| 0.299 * v[1] + 0.587 * v[2] + 0.114 * v[3];
    let (b, a) = ([-0.26f32, -0.15, -0.16, -0.03, -0.02], [0.02f32, 0.03, 0.01, -0.02, 0.0]);
    assert_eq!(compose_axes(&b, &b, 0.35), b);
    let tone_of_b = compose_axes(&b, &a, 0.35);
    assert_eq!((tone_of_b[0], tone_of_b[4]), (b[0], b[4]), "EV and slope are the tone's");
    assert!((level(&tone_of_b) - level(&b)).abs() < 1e-6, "the gains' level is the tone's");
    for c in 1..4 {
        let (got, want) = (tone_of_b[c] - level(&tone_of_b), a[c] - level(&a));
        assert!((got - want).abs() < 1e-6, "channel {c}: the offset from the level is the colour's");
    }
    let colour_of_b = compose_axes(&a, &b, 0.35);
    assert_eq!((colour_of_b[0], colour_of_b[4]), (a[0], a[4]));
    assert!((colour_of_b[2] - colour_of_b[3] - (b[2] - b[3])).abs() < 1e-6, "pass B's green against its blue");
    // Inside the bound the solve was held to.
    let wide = compose_axes(&[0.0, 0.35, 0.35, 0.35, 0.0], &[0.0, 0.35, -0.35, -0.35, 0.0], 0.35);
    assert!(wide[1..4].iter().all(|g| g.abs() <= 0.35), "{wide:?}");
}

/// The per-zone do-no-harm judges EVERY mask the recipe carries. A Custom-role
/// linear mask over the right half is hurt by a render that moves that half
/// away from the target, and the check names it by the mask's own name.
#[test]
fn the_field_do_no_harm_judges_a_custom_region_mask_too() {
    let (src, tgt) = (repainted_right_half(false), repainted_right_half(true));
    let (current, target, w, _h, mut report) = field_probe(&src, &tgt);
    report.recipe.masks.push(right_half_region());
    let (s_img, _) = fit::analysis_pair(&src, &tgt);
    assert_eq!(zone_regressed(&s_img, &report, &current, &target), None, "unchanged pixels hurt no mask");
    let mut hurt = current.clone();
    for (i, px) in hurt.iter_mut().enumerate() {
        if i % w as usize >= w as usize / 2 {
            *px = [0.02, 0.02, 0.98];
        }
    }
    let (label, was, now, _) = zone_regressed(&s_img, &report, &hurt, &target)
        .expect("the region mask's own residual grew past the tolerance");
    assert_eq!(label, "region-1");
    assert!(now > was + crate::fit::FIT_QUANT, "{was} -> {now}");
}

/// J146: the per-zone check is a GUARD and reads `FIT_QUANT`, both ways. The
/// region's pixels are pushed against the repaint (the target's right half is
/// warmer; the push takes red down and blue up) by a step FOUND on the
/// objective, so that the region's own reading rises by a part of FIT_QUANT:
/// a rise the objective cannot tell from its own rounding, and the check
/// names no mask. The same push grown until the rise is past FIT_QUANT is
/// refused, by the mask's name. From R1 until 2026-11-12 the first half
/// refused too ("a loss of any size"), and a real pair gave its whole solved
/// colour field back for a free mask's rise of 0.15 of FIT_QUANT (round V10,
/// the reference pair at strength 0.65; gates/j146).
/// MUTATION: restore `now > was` in `zone_regressed` — the first half fails.
#[test]
fn the_field_do_no_harm_reads_the_fits_own_rounding_both_ways() {
    let (src, tgt) = (repainted_right_half(false), repainted_right_half(true));
    let (_, target, w, _h, mut report) = field_probe(&src, &tgt);
    report.recipe.masks.push(right_half_region());
    let (s_img, _) = fit::analysis_pair(&src, &tgt);
    // The frame the check itself starts from, so the rise read here is the
    // rise it reads.
    let base = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let pushed = |step: f32| -> Vec<[f32; 3]> {
        let mut out = base.clone();
        for (i, px) in out.iter_mut().enumerate() {
            if i % w as usize >= w as usize / 2 {
                *px = [(px[0] - step).max(0.0), px[1], (px[2] + step).min(1.0)];
            }
        }
        out
    };
    let reading = |px: &[[f32; 3]]| {
        zone_reading(&s_img, &report, "region-1", px, &target).expect("the mask covers the right half")
    };
    let was = reading(&base);
    let rise = |step: f32| reading(&pushed(step)) - was;
    let quant = crate::fit::FIT_QUANT;
    // The steps are found, not pinned: the reading is the objective's own.
    let step_past = |part: f32| (1..=400u16).map(|k| f32::from(k) * 2.5e-4).find(|s| rise(*s) > part * quant);
    let small = step_past(0.25).expect("some push raises the region's reading");
    let large = step_past(2.0).expect("a larger push raises it past the margin");
    let (under, over) = (rise(small), rise(large));
    eprintln!(
        "FIELD_GUARD was {was:.6}; step {small:.5} rises {under:.6}, step {large:.5} rises {over:.6} (FIT_QUANT {quant})"
    );
    assert!(under < 0.75 * quant, "premise: the small push stays under the margin ({under} of {quant})");
    assert_eq!(
        zone_regressed(&s_img, &report, &pushed(small), &target).map(|hit| hit.0),
        None,
        "a rise of {under} is inside the fit's own rounding ({quant}) and costs no zone"
    );
    let (label, before, after, _) = zone_regressed(&s_img, &report, &pushed(large), &target)
        .expect("a rise past the fit's own rounding is refused");
    assert_eq!(label, "region-1");
    assert!(after - before > quant, "{before} -> {after}");
}

/// A pair the field solver cannot fit (every evidence weight zero) says so
/// in the report instead of leaving the field's absence unexplained.
#[test]
fn an_unsolvable_field_is_disclosed_not_silent() {
    let (src, tgt) = (repainted_right_half(false), repainted_right_half(true));
    let (_, _, _, _, mut report) = field_probe(&src, &tgt);
    for w in &mut report.evidence.source_weights {
        *w = 0.0;
    }
    assert!(solve_local_field(&src, &tgt, &mut report).is_none(), "premise: nothing to fit");
    let unsolved = report.notes.iter().find(|n| n.key == crate::rationale::keys::FIELD_UNSOLVED)
        .expect("the abstention is a note");
    assert_eq!(unsolved.args, vec![("stage", "analysis".to_string())]);
    assert!(report.recipe.rationale.contains("No colour field was solved"));
}

/// …and the abstention that keeps the pre-R34 field byte for byte: a pair with
/// nothing to recolour has no cell that moved closer to anything, so no cell
/// is admitted and the shipped grid IS the analysis grid.
#[test]
fn a_field_no_cell_vouches_is_the_support_weighted_field_byte_for_byte() {
    let src = repainted_right_half(false);
    let (current, target, w, h, report) = field_probe(&src, &src);
    let Some(pass_a) = LocalField::solve(&current, &target, w, h, &report.evidence) else {
        return;
    };
    let (merged, admitted, _, _, _) =
        admit_cells(&current, &target, w, h, &report.evidence, pass_a.clone(), 0.60, &[], None);
    assert_eq!(admitted, 0, "a matched pair asks for no recolour");
    assert_eq!(merged.grid, pass_a.grid, "so the shipped grid is the analysis grid");
}

#[test]
fn field_stop_and_realized_helpers_are_well_conditioned() {
    let field = synthetic_field(1, 1, vec![0.0]);
    assert_eq!(realized_share(1.0, 0.5, 0.75), Some(0.5));
    assert_eq!(realized_share(1.0, 1.0 - 1e-7, 1.0), None);
    // R3 (v1.18.0) rewrote this pin: the stop compares the field's ceiling with
    // the frame of the SAME state (`global`, re-read with it), so the second
    // argument is gone. The stop fires when the ceiling cannot gain FIT_QUANT
    // over that frame (`fit::ships`), and a ceiling at or above it measured
    // nothing and stops nothing.
    let mut field = field;
    field.global = 0.5;
    field.ceiling = 0.5 - crate::fit::FIT_QUANT * 0.5;
    assert!(stop_verdict(&field), "a gain under FIT_QUANT stops the field-derived producers");
    field.ceiling = 0.5 - crate::fit::FIT_QUANT * 2.0;
    assert!(!stop_verdict(&field), "a gain of FIT_QUANT or more does not");
    field.ceiling = 0.5;
    assert!(!stop_verdict(&field));
    field.ceiling = 0.6;
    assert!(!stop_verdict(&field));
}

/// R42. A 192x128 pair whose top-right NINTH — one 3x3 spatial-evidence cell,
/// as on the reference pair — the target re-synthesised SMOOTH: the same slow
/// base on both sides, the source carrying a per-pixel texture draw
/// everywhere, the target carrying the same draw outside the ninth and a warm
/// recolour inside it. The pixel-scale reading's energy term reads the ninth
/// past its cutoff (noise against smoothness), so its evidence is zero, the
/// way the reference pair's featureless sky reads; the rest of the frame is
/// identical on both sides, so the frame stays same-content and its ranges
/// keep their weight (a smoothed HALF collapses every range's structural
/// survival under the line and no field solves at all).
fn smoothed_top_right_ninth(target: bool) -> image::DynamicImage {
    let (w, h) = (192u32, 128u32);
    let hash = |i: u32| {
        let mut v = i.wrapping_mul(747796405).wrapping_add(2891336453);
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 10_000) as f32 / 10_000.0 - 0.5
    };
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        let base = 0.32 + 0.26 * (y as f32 / (h - 1) as f32);
        let ninth = x * 3 / w == 2 && y * 3 / h == 0;
        let texture = if ninth && target { 0.0 } else { 0.12 * hash(y * w + x) };
        let v = (base + texture).clamp(0.05, 0.95);
        let p = if ninth && target {
            [(v * 1.35).min(0.98), v, (v * 0.72).max(0.02)]
        } else {
            [v, v, v]
        };
        image::Rgb(p.map(|c| (c * 255.0).round() as u8))
    }))
}

/// The premise every R42 test rests on, asserted rather than assumed: the
/// smoothed ninth has cells the pixel-scale evidence cannot read at all, and
/// only there. Returns those cells.
fn unread_cells(
    current: &[[f32; 3]], target: &[[f32; 3]], w: u32, h: u32, report: &FitReport, pass_a: &LocalField,
) -> Vec<usize> {
    let cells = crate::fit_cells::PairedCells::build(target, w, h, &report.evidence)
        .expect("the textured half carries weight");
    let unread: Vec<usize> = cells
        .moves(current, &pass_a.render(current), None)
        .iter()
        .enumerate()
        .filter_map(|(cell, verdict)| verdict.is_none().then_some(cell))
        .collect();
    let (cols, rows) = (crate::fit_cells::CELLS_X, crate::fit_cells::CELLS_Y);
    assert!(
        !unread.is_empty()
            && unread.iter().all(|cell| cell % cols >= cols * 2 / 3 && cell / cols <= rows / 3),
        "premise: the smoothed ninth has cells the pixel-scale evidence cannot read at all, \
         and only there: {unread:?}"
    );
    unread
}

/// The pairing: the frame's top third — the smoothed ninth and the two
/// textured ninths beside it — as the segmenter would hand a sky at analysis
/// size.
fn top_third_pairing(w: u32, h: u32) -> Vec<f32> {
    (0..(w * h) as usize).map(|i| f32::from((i as u32 / w) * 3 / h == 0)).collect()
}

/// Whether analysis pixel `i` lies in the smoothed ninth.
fn in_ninth(i: usize, w: u32, h: u32) -> bool {
    let (x, y) = (i as u32 % w, i as u32 / w);
    x * 3 / w == 2 && y * 3 / h == 0
}

/// R42. A cell the pixel-scale evidence cannot read at all keeps the analysis
/// grid without a pairing, and inside one is read on the pairing: it takes
/// pass B's vertices, and the render moves the smoothed ninth's mean toward
/// its target.
#[test]
fn a_cell_the_pixel_evidence_cannot_read_is_read_on_its_region_pairing() {
    let (src, tgt) = (smoothed_top_right_ninth(false), smoothed_top_right_ninth(true));
    let (current, target, w, h, report) = field_probe(&src, &tgt);
    let pass_a = LocalField::solve(&current, &target, w, h, &report.evidence)
        .expect("the analysis field solves");
    let unread = unread_cells(&current, &target, w, h, &report, &pass_a);
    let (without, _, _, none, _) =
        admit_cells(&current, &target, w, h, &report.evidence, pass_a.clone(), 0.60, &[], None);
    assert_eq!(none, LayoutAdmission::default(), "no pairing: nothing is read on one");
    let pairing = top_third_pairing(w, h);
    let (merged, _, _, paired, _) =
        admit_cells(&current, &target, w, h, &report.evidence, pass_a.clone(), 0.60, &pairing, None);
    assert_eq!(paired.read, unread.len(), "every unread cell inside the pairing was read on it: {paired:?}");
    assert!(paired.admitted > 0, "the smooth recolour is admitted: {paired:?}");
    for cell in &unread {
        for span in crate::fit_field::cell_vertex_spans(*cell) {
            assert_eq!(without.grid[span.clone()], pass_a.grid[span], "no pairing: cell {cell} keeps the analysis grid");
        }
    }
    assert!(
        unread.iter().any(|cell| crate::fit_field::cell_vertex_spans(*cell)
            .any(|span| merged.grid[span.clone()] != pass_a.grid[span])),
        "with the pairing, the unread cells carry pass B's vertices"
    );
    let ninth_mean = |px: &[[f32; 3]], ch: usize| -> f32 {
        let picked: Vec<f32> =
            (0..(w * h) as usize).filter(|i| in_ninth(*i, w, h)).map(|i| px[i][ch]).collect();
        picked.iter().sum::<f32>() / picked.len().max(1) as f32
    };
    let rendered = merged.render(&current);
    for ch in [0usize, 2] {
        let (before, after) = (
            (ninth_mean(&current, ch) - ninth_mean(&target, ch)).abs(),
            (ninth_mean(&rendered, ch) - ninth_mean(&target, ch)).abs(),
        );
        assert!(after < before, "channel {ch}: the smoothed ninth moved toward its target ({before:.4} -> {after:.4})");
    }
}

/// J141's displaced-contour toy at analysis size: a flat sky over a flat land
/// with one butte whose left edge is at `x0`, and its sky raster. Two of these
/// 12 pixels apart are the same picture with the butte moved — nothing else
/// changed, so the only residual is the two bands the contour moved over.
fn displaced_butte(x0: u32) -> (image::DynamicImage, image::GrayImage) {
    let (w, h) = (384u32, 256u32);
    let land = move |x: u32, y: u32| y >= 128 || ((x0..x0 + 64).contains(&x) && y >= 64);
    let picture = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        let v = if land(x, y) { 0.20 } else { 0.62 } + 0.03 * ((x * 7 + y * 13) % 5) as f32 / 4.0;
        image::Rgb([(v * 255.0).round() as u8; 3])
    }));
    let sky = image::GrayImage::from_fn(w, h, |x, y| image::Luma([if land(x, y) { 0 } else { 255 }]));
    (picture, sky)
}

/// The toy's pair and the layout the zoned entry reads off its two sky
/// rasters: (current, target, the layout, the source's sky coverage, w, h).
#[allow(clippy::type_complexity)]
fn displaced_pair() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, PairedLayout, Vec<f32>, u32, u32) {
    let ((src, sky_s), (tgt, sky_t)) = (displaced_butte(160), displaced_butte(172));
    let layout = super::super::entry::paired_layout(&src, &tgt, &[(&sky_s, &sky_t)]);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let (w, h) = (s_img.width(), s_img.height());
    assert_eq!((w, h), (384, 256), "premise: the toy is at analysis size");
    let sky = super::super::rasters::mask_weights(&sky_s, w, h);
    (fit::pixels_of(&s_img), fit::pixels_of(&t_img), layout, sky, w, h)
}

/// J141. The layout reads a displaced contour as SPLIT: the band of source
/// rock whose target pixel is sky and the band of source sky whose target
/// pixel is rock, and nothing else — a pixel both frames give the same region
/// is not split, the sky both hold is paired, and the land ("not sky" on both
/// sides) is neither. MUTATION: in `paired_layout`, take `split` as zero —
/// the two bands read as agreed.
#[test]
fn the_layout_reads_a_displaced_contour_as_split() {
    let (_, _, layout, _, w, h) = displaced_pair();
    let band = |x: u32, y: u32| (64..128).contains(&y) && ((160..172).contains(&x) || (224..236).contains(&x));
    let (mut wrong, mut split) = (0usize, 0usize);
    for i in 0..(w * h) as usize {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let expected = f32::from(band(x, y));
        split += usize::from(layout.split[i] > 0.5);
        wrong += usize::from((layout.split[i] - expected).abs() > 1e-3);
        let sky_on_both = !(y >= 128 || ((160..236).contains(&x) && y >= 64));
        wrong += usize::from((layout.paired[i] - f32::from(sky_on_both)).abs() > 1e-3);
    }
    assert_eq!(split, 2 * 12 * 64, "the two displaced bands, 12 px wide over the butte's 64 rows");
    assert_eq!(wrong, 0, "…and every other pixel agreed, the sky both frames hold paired");
}

/// J141. A pair split across the two frames' zones does not testify. Weighed
/// like any other, the band of source sky whose target is rock pulls the
/// sky-owned vertices beside the butte down, and the sky NEXT to the band —
/// sky on both sides, its target its own pixel — darkens: the smudge the
/// reference pair carried beside its buttes. On the testimony that sky does
/// not move. MUTATION: in `field_testimony`, drop the `*weight *=
/// together(i)` scaling — the sky beside the butte darkens as on the plain
/// weights.
#[test]
fn a_pair_split_across_the_two_frames_zones_does_not_testify() {
    let (current, target, layout, sky, w, h) = displaced_pair();
    let evidence = fit::evidence_model_for(&current, &target, w, h);
    let (testimony, paired) = field_testimony(&evidence, &layout);
    let n = current.len();
    let split: Vec<usize> = (0..n).filter(|i| layout.split[*i] > 0.5).collect();
    assert!(
        split.iter().all(|&i| evidence.source_weights[i] > 0.0),
        "premise: the split pixels carry evidence weight like their neighbours"
    );
    assert!(
        split.iter().all(|&i| testimony.source_weights[i] == 0.0 && paired[i] == 0.0),
        "a split pixel weighs nothing, in the model and in the layout plane"
    );
    assert!(
        (0..n).filter(|i| layout.split[*i] == 0.0).all(|i| {
            testimony.source_weights[i] == evidence.source_weights[i] && paired[i] == layout.paired[i]
        }),
        "…and every other pixel weighs what it weighed"
    );
    // The shipped solve's form: zone-owned, support-free, refined, pass B's gain.
    let solve = |model: &fit::EvidenceModel| {
        LocalField::solve_zoned(
            &current, &target, w, h, model, crate::fit_field::TIKHONOV, crate::fit_field::SMOOTH,
            crate::fit_field::ITERATIONS, FieldSolveOpts { gain: 0.607, local_support: false, refine: true },
            Some(&sky),
        )
        .expect("the toy solves")
        .render(&current)
    };
    // The sky just right of the band the butte moved onto: sky on both sides.
    let beside = |out: &[[f32; 3]]| -> f32 {
        let picked: Vec<f32> = (0..n)
            .filter(|i| (238..250).contains(&(*i as u32 % w)) && (72..120).contains(&(*i as u32 / w)))
            .map(|i| fit::luma601(&out[i]) - fit::luma601(&current[i]))
            .collect();
        picked.iter().sum::<f32>() / picked.len() as f32
    };
    let (plain, lawful) = (beside(&solve(&evidence)), beside(&solve(&testimony)));
    assert!(plain < -0.008, "premise: weighed like a pair, the split band darkens the sky beside it ({plain:+.4})");
    assert!(lawful.abs() < 0.002, "on the testimony the sky beside the butte does not move ({lawful:+.4})");
}

/// J141. Two photographs get no colour field. The repainted fixture's target
/// turned upside down shares no place with its source, and the structural
/// reading of two photographs is imposed on the model — every cell past the
/// zone line, no pixel paired (the instrument reads two draws of one noise as
/// the same texture, so a synthetic pair cannot earn that reading; the
/// different-photo pair of the calibration corpus does, at D 0.78 and up).
/// The population model an Atmosphere report carries still weighs the whole
/// frame — its ranges are populated on both sides — and a field solved from
/// it draws the target's layout onto the source while the frame objective,
/// which reads unpaired pixels as a distribution, improves. The testimony is
/// empty, the solver finds nothing to fit, and the report says so. MUTATION:
/// in `field_testimony`, drop the `pairs_a_population` refusal — the field is
/// solved, ships, and the recipe carries it.
#[test]
fn two_photographs_get_no_colour_field() {
    let src = repainted_right_half(false);
    let tgt = repainted_right_half(true).rotate180();
    let (current, target, _, _, mut report) = field_probe(&src, &tgt);
    let mut ingredients = report.evidence.clone();
    ingredients.spatial_weights.fill(0.0);
    ingredients.spatial_divergence.fill(1.2);
    ingredients.spatial_supported.fill(false);
    ingredients.paired.fill(false);
    ingredients.globally_same_content = false;
    let ones = vec![1.0; current.len()];
    let structural = ingredients.scoped(&target, &ones, &ones);
    assert_eq!(structural.paired_share(), 0.0, "premise: no cell of the frame pairs");
    // The Atmosphere report's form: the structure-blind population model,
    // which keeps the structural pairing, beside the structural model.
    report.evidence = structural.structure_blind(&target);
    report.structural_evidence = Some(structural);
    report.mode = fit::FitMode::Atmosphere;
    assert!(!report.evidence.pairs_a_population(), "the population model keeps the structural pairing");
    let err = fit::look_err_with_evidence(&current, &target, &report.evidence);
    (report.err_before, report.err_after) = (err, err);
    let weights = &report.evidence.source_weights;
    let weighed = weights.iter().sum::<f32>() / weights.len() as f32;
    assert!(weighed > 0.5, "premise: the population model weighs the frame (mean {weighed:.3})");
    let (testimony, layout) = field_testimony(&report.evidence, &PairedLayout::default());
    assert!(
        testimony.source_weights.iter().all(|w| *w == 0.0) && layout.is_empty(),
        "nothing pairs, so nothing testifies"
    );
    attach_colour_field(
        &src, &tgt, &mut report, crate::recipe::GradeStrength::new(0.85), &PairedLayout::default(),
    );
    assert!(report.recipe.colour_field.is_none(), "no field is attached: {}", report.recipe.rationale);
    let unsolved = report.notes.iter().find(|n| n.key == crate::rationale::keys::FIELD_UNSOLVED)
        .expect("the abstention is a note");
    assert_eq!(unsolved.args, vec![("stage", "attach".to_string())]);
}

/// …and a frame that pairs somewhere is one scene: the repainted fixture as
/// it is — its left half byte-identical on both sides — keeps every weight
/// the report gave it, with no segmentation to say any pair is split.
#[test]
fn a_frame_that_pairs_somewhere_testifies_as_it_did() {
    let (src, tgt) = (repainted_right_half(false), repainted_right_half(true));
    let (_, _, _, _, report) = field_probe(&src, &tgt);
    assert!(report.evidence.pairs_a_population(), "premise: the untouched half pairs");
    let (testimony, layout) = field_testimony(&report.evidence, &PairedLayout::default());
    assert_eq!(testimony.source_weights, report.evidence.source_weights);
    assert!(layout.is_empty(), "no segmentation, no layout");
}
