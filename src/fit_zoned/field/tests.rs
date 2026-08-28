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
