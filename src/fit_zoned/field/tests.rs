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
    let (merged, admitted, read, _) =
        admit_cells(&current, &target, w, h, &report, pass_a.clone(), 0.60, &[]);
    assert!(read > 0, "the instrument was consulted: {admitted} of {read}");
    assert!(admitted > 0, "the repainted cells must be admitted: {admitted} of {read}");
    let bins = crate::fit_field::FIELD_B;
    let moved: Vec<usize> = (0..crate::fit_cells::CELLS_X * crate::fit_cells::CELLS_Y)
        .filter(|cell| {
            merged.grid[cell * bins..(cell + 1) * bins]
                != pass_a.grid[cell * bins..(cell + 1) * bins]
        })
        .collect();
    assert_eq!(moved.len(), admitted, "exactly the admitted cells took pass B");
    assert!(
        moved.iter().all(|cell| cell % crate::fit_cells::CELLS_X >= crate::fit_cells::CELLS_X / 4),
        "…and they are the repainted ones, not the untouched left edge: {moved:?}"
    );
}

/// The per-zone do-no-harm judges EVERY mask the recipe carries. A Custom-role
/// linear mask over the right half is hurt by a render that moves that half
/// away from the target, and the check names it by the mask's own name.
#[test]
fn the_field_do_no_harm_judges_a_custom_region_mask_too() {
    let (src, tgt) = (repainted_right_half(false), repainted_right_half(true));
    let (current, target, w, _h, mut report) = field_probe(&src, &tgt);
    report.recipe.masks.push(crate::recipe::LocalAdjustment {
        mask: crate::recipe::MaskGeometry::Linear { zero_x: 0.49, zero_y: 0.5, full_x: 0.51, full_y: 0.5 },
        name: "region-1".into(),
        role: crate::recipe::MaskRole::Custom,
        amount: 1.0,
        ..Default::default()
    });
    let (s_img, _) = fit::analysis_pair(&src, &tgt);
    assert_eq!(zone_regressed(&s_img, &report, &current, &target), None, "unchanged pixels hurt no mask");
    let mut hurt = current.clone();
    for (i, px) in hurt.iter_mut().enumerate() {
        if i % w as usize >= w as usize / 2 {
            *px = [0.02, 0.02, 0.98];
        }
    }
    let (label, was, now) = zone_regressed(&s_img, &report, &hurt, &target)
        .expect("the region mask's own residual grew past the tolerance");
    assert_eq!(label, "region-1");
    assert!(now > was + crate::fit_zoned::ZONE_GLOBAL_REGRESSION_TOL, "{was} -> {now}");
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
    let (merged, admitted, _, _) =
        admit_cells(&current, &target, w, h, &report, pass_a.clone(), 0.60, &[]);
    assert_eq!(admitted, 0, "a matched pair asks for no recolour");
    assert_eq!(merged.grid, pass_a.grid, "so the shipped grid is the analysis grid");
}

#[test]
fn field_stop_and_realized_helpers_are_well_conditioned() {
    let field = synthetic_field(1, 1, vec![0.0]);
    assert_eq!(realized_share(1.0, 0.5, 0.75), Some(0.5));
    assert_eq!(realized_share(1.0, 1.0 - 1e-7, 1.0), None);
    let mut field = field;
    field.ceiling = 0.5;
    assert!(stop_verdict(&field, 0.502));
    assert!(!stop_verdict(&field, 0.503));
    // A ceiling that never beat the producer-free frame (global = 1.0 here)
    // measured nothing about the headroom and must not veto the tile producer.
    field.ceiling = 1.0;
    assert!(!stop_verdict(&field, 0.9));
    field.ceiling = 1.2;
    assert!(!stop_verdict(&field, 0.9));
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
        .verdicts(current, &pass_a.render(current), None)
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
    let (without, _, _, none) =
        admit_cells(&current, &target, w, h, &report, pass_a.clone(), 0.60, &[]);
    assert_eq!(none, LayoutAdmission::default(), "no pairing: nothing is read on one");
    let pairing = top_third_pairing(w, h);
    let (merged, _, _, paired) =
        admit_cells(&current, &target, w, h, &report, pass_a.clone(), 0.60, &pairing);
    assert_eq!(paired.read, unread.len(), "every unread cell inside the pairing was read on it: {paired:?}");
    assert!(paired.admitted > 0, "the smooth recolour is admitted: {paired:?}");
    let bins = crate::fit_field::FIELD_B;
    for cell in &unread {
        let span = cell * bins..(cell + 1) * bins;
        assert_eq!(without.grid[span.clone()], pass_a.grid[span.clone()], "no pairing: cell {cell} keeps the analysis grid");
    }
    assert!(
        unread.iter().any(|cell| merged.grid[cell * bins..(cell + 1) * bins] != pass_a.grid[cell * bins..(cell + 1) * bins]),
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
