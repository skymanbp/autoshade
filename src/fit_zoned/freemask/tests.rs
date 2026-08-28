use super::*;
use image::{DynamicImage, Rgb, RgbImage};

const EDGE: u32 = fit::ANALYZE_EDGE;

fn textured_image() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(EDGE, EDGE, |x, y| {
        let value = 90 + ((x * 7 + y * 11 + (x / 5) * 13) % 90) as u8;
        Rgb([value; 3])
    }))
}

fn with_delta(source: &DynamicImage, region: &[bool], delta: i16) -> DynamicImage {
    let mut target = source.to_rgb8();
    for (i, pixel) in target.pixels_mut().enumerate() {
        if region[i] {
            for value in &mut pixel.0 {
                *value = (*value as i16 + delta).clamp(0, 255) as u8;
            }
        }
    }
    DynamicImage::ImageRgb8(target)
}

fn disc(cx: i32, cy: i32, radius: i32) -> Vec<bool> {
    (0..EDGE * EDGE).map(|i| {
        let (x, y) = ((i % EDGE) as i32, (i / EDGE) as i32);
        (x - cx).pow(2) + (y - cy).pow(2) <= radius.pow(2)
    }).collect()
}

fn field_from(region: &[bool], value: f32) -> LocalField {
    let remainder = region.iter().map(|inside| if *inside { value } else { 0.0 }).collect();
    LocalField {
        grid: Vec::new(),
        occupancy: Vec::new(),
        ceiling: 0.0,
        global: 1.0,
        band_marginal: [[0.0; 5]; 8],
        band_dispersion: [0.0; 8],
        remainder,
        weight: vec![1.0; (EDGE * EDGE) as usize],
        saturated: 0,
        solve: crate::fit_field::SolveInfo { iterations: 0, relative_residual: 0.0 },
        width: EDGE,
        height: EDGE,
    }
}

fn full_evidence(source: &DynamicImage, target: &DynamicImage) -> fit::EvidenceModel {
    let (source, target) = (fit::pixels_of(source), fit::pixels_of(target));
    let mut evidence = fit::evidence_model_for(&source, &target, EDGE, EDGE);
    evidence.source_pixels = source;
    evidence.source_membership.fill(1.0);
    evidence.spatial_weights.fill(1.0);
    evidence.spatial_divergence.fill(0.0);
    evidence.spatial_supported.fill(true);
    evidence.globally_same_content = true;
    evidence.source_weights.fill(1.0);
    evidence.target_weights.fill(1.0);
    evidence
}

fn proposal_inputs(
    source: &DynamicImage,
    target: &DynamicImage,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, fit::EvidenceModel) {
    let source_px = fit::pixels_of(source);
    let target_px = fit::pixels_of(target);
    let evidence = full_evidence(source, target);
    (source_px, target_px, evidence)
}

#[test]
fn free_mask_proposes_the_blob_the_tiles_cannot_box() {
    let truth = disc(221, 167, 64);
    assert!(221 % (EDGE as i32 / 4) != 0 && 167 % (EDGE as i32 / 4) != 0);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(EDGE, EDGE, |x, y| {
        let i = (y * EDGE + x) as usize;
        let value = if truth[i] { 40 + ((x * 37 + y * 53) % 180) as u8 } else { 10 };
        Rgb([value; 3])
    }));
    let current = render::develop_preview(&source, &crate::recipe::EditRecipe::default());
    let target = with_delta(&current, &truth, -26);
    let field = field_from(&truth, -0.1);
    let (source_px, target_px, evidence) = proposal_inputs(&current, &target);
    let dir = std::env::temp_dir().join(format!("autoshop-free-disc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let home = crate::store::OwnedRaster::scratch(dir.join("mask-semantic.png"));
    let mut report = super::super::tests::neutral_report(&source, &target);
    report.evidence = evidence;
    let excluded = spatial::attach_tiles(&source, &target, &mut report, &home, false, 0);
    assert!(report.recipe.masks.is_empty(), "the zero tile cap attached a tile");
    let search = search_free_masks(
        &field, &excluded, &source_px, &target_px, &report.evidence, 2,
    );
    assert_eq!(search.proposals.len(), 1, "refusals={:?}", search.refusals);
    let proposal = &search.proposals[0].1;
    let (mut intersection, mut union) = (0usize, 0usize);
    for (actual, expected) in proposal.mask.as_raw().iter().map(|v| *v != 0).zip(&truth) {
        intersection += usize::from(actual && *expected);
        union += usize::from(actual || *expected);
    }
    assert!(intersection as f32 / union as f32 >= 0.8);
    assert!(proposal.sign < 0.0);

    report.err_after = fit::look_err_with_evidence(&source_px, &target_px, &report.evidence);
    let rerendered = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
    assert_eq!(rerendered, source_px);
    let geometry = proposal.mask.as_raw().iter().map(|v| *v as f32 / 255.0).collect::<Vec<_>>();
    let scoped = spatial::scoped_mask_evidence(&target_px, &report.evidence, &geometry);
    let robust = fit::paired_robust_tone(&rerendered, &target_px, &|i| {
        scoped.source_weights[i].min(scoped.target_weights[i])
    }, false).unwrap();
    let robust_share = scoped.source_weights.iter().zip(&robust.weights)
        .map(|(a, b)| a * b).sum::<f32>() / geometry.len() as f32;
    assert!(robust_share >= MIN_ZONE_SHARE, "robust share {robust_share}");
    let before = report.err_after;
    let stage = attach_free_masks(
        &source, &target, &mut report, &home, &field, &excluded, false, 2,
    );
    assert_eq!(stage.components, stage.disclosed);
    assert!(report.recipe.masks.iter().any(|mask| mask.name == "field-zone-1"),
        "{}", report.recipe.rationale);
    assert!(report.err_after < before, "{before} -> {}", report.err_after);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn free_masks_never_merge_opposite_signs() {
    let left = disc(136, 192, 60);
    let right = disc(253, 192, 57);
    let source = textured_image();
    let (source_px, target_px, evidence) = proposal_inputs(&source, &source);
    let mut field = field_from(&vec![false; (EDGE * EDGE) as usize], 0.0);
    for i in 0..field.remainder.len() {
        field.remainder[i] = if left[i] { -0.1 } else if right[i] { 0.1 } else { 0.0 };
    }
    let proposals = propose_free_masks(
        &field, &vec![0.0; field.remainder.len()], &source_px, &target_px, &evidence, 2,
    );
    assert_eq!(proposals.len(), 2, "{proposals:?}");
    assert!(proposals[0].mass >= proposals[1].mass);
    assert!((proposals[0].mass - proposals[1].mass).abs() > 1e-3);
    assert_ne!(proposals[0].sign.signum(), proposals[1].sign.signum());
    assert!(left.iter().enumerate().any(|(i, inside)| {
        *inside && i % EDGE as usize + 1 < field.remainder.len()
            && field.remainder[i] * field.remainder[i + 1] < 0.0
    }), "the opposite-sign regions do not touch");
}

#[test]
fn free_mask_refuses_a_component_below_the_share_line() {
    let mut tiny = vec![false; (EDGE * EDGE) as usize];
    for y in 160..163 { for x in 200..203 { tiny[(y * EDGE + x) as usize] = true; } }
    let source = textured_image();
    let (source_px, target_px, evidence) = proposal_inputs(&source, &source);
    let search = search_free_masks(
        &field_from(&tiny, 0.1), &vec![0.0; tiny.len()],
        &source_px, &target_px, &evidence, 2,
    );
    assert!(search.proposals.is_empty());
    assert_eq!(search.refusals.len(), 1);
    assert_eq!(search.refusals[0].why, FreeMaskWhy::Footprint);
}

#[test]
fn free_mask_refuses_replaced_content() {
    let region = (0..EDGE * EDGE).map(|i| {
        let (x, y) = (i % EDGE, i / EDGE);
        (48..336).contains(&x) && (48..336).contains(&y)
    }).collect::<Vec<_>>();
    let source = textured_image();
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(EDGE, EDGE, |x, y| {
        let value = ((x * 73 + y * 151 + x * y * 17) % 256) as u8;
        Rgb([value, value.wrapping_mul(3), value.wrapping_mul(7)])
    }));
    let (source_px, target_px, evidence) = proposal_inputs(&source, &target);
    let field = field_from(&region, 0.1);
    let search = search_free_masks(
        &field, &vec![0.0; region.len()], &source_px, &target_px, &evidence, 2,
    );
    assert!(search.proposals.is_empty(), "{:?}", search.proposals);
    assert!(search.refusals.iter().any(|r| r.why == FreeMaskWhy::Divergence),
        "{:?}", search.refusals);
    let mut report = super::super::tests::neutral_report(&source, &target);
    report.evidence = evidence;
    let masks = report.recipe.masks.len();
    let home = crate::store::OwnedRaster::scratch(
        std::env::temp_dir().join(format!("autoshop-free-replaced-{}.png", std::process::id())),
    );
    attach_free_masks(&source, &target, &mut report, &home, &field, &vec![0.0; region.len()], false, 2);
    assert_eq!(report.recipe.masks.len(), masks);
    assert!(report.notes.iter().all(|note| note.key != crate::rationale::keys::FIELD_MASK_ATTACHED));
}

#[test]
fn free_masks_eat_only_what_tiles_left() {
    let truth = disc(221, 167, 72);
    let source = textured_image();
    let mut base = crate::recipe::EditRecipe::default();
    let dir = std::env::temp_dir().join(format!("autoshop-free-tile-exclusion-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let prior = crate::store::OwnedRaster::scratch(dir.join("prior.png"));
    let prior_mask = image::GrayImage::from_fn(EDGE, EDGE, |x, y| {
        let inside = x < EDGE / 4 && (EDGE / 2..3 * EDGE / 4).contains(&y);
        image::Luma([if inside { 255 } else { 0 }])
    });
    prior_mask.save(prior.path()).unwrap();
    base.masks.push(crate::recipe::LocalAdjustment {
        mask: crate::recipe::MaskGeometry::Bitmap { path: prior.path().to_string_lossy().into_owned() },
        role: crate::recipe::MaskRole::ZoneLand, exposure_ev: 0.15, ..Default::default()
    });
    let current = render::develop_preview(&source, &base);
    let mut target_image = current.to_rgb8();
    for y in 0..EDGE { for x in 0..EDGE {
        let inside = x < EDGE / 4 && (EDGE / 2..3 * EDGE / 4).contains(&y);
        if inside { for value in &mut target_image.get_pixel_mut(x, y).0 { *value = value.saturating_add(20); } }
    }}
    let target = DynamicImage::ImageRgb8(target_image);
    let source_px = fit::pixels_of(&current);
    let target_px = fit::pixels_of(&target);
    let mut evidence = fit::evidence_model_for(&source_px, &target_px, EDGE, EDGE);
    evidence.source_pixels = target_px.clone();
    evidence.source_membership.fill(1.0);
    evidence.spatial_weights.fill(1.0);
    evidence.spatial_supported.fill(true);
    evidence.target_weights.fill(1.0);
    let home = crate::store::OwnedRaster::scratch(dir.join("tile.png"));
    let mut report = super::super::tests::neutral_report(&source, &target);
    report.recipe = base;
    report.evidence = evidence;
    let excluded = spatial::attach_tiles(&source, &target, &mut report, &home, false, 1);
    assert!(report.recipe.masks.len() >= 2, "fixture did not accept a tile: {}", report.recipe.rationale);
    assert!(excluded.iter().any(|alpha| *alpha >= 0.5));
    let evidence = report.evidence.clone();
    let proposals = propose_free_masks(
        &field_from(&truth, -0.1), &excluded, &source_px, &target_px, &evidence, 2,
    );
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert!(proposals[0].mask.as_raw().iter().enumerate()
        .all(|(i, value)| excluded[i] < 0.5 || *value == 0));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn free_mask_proposals_are_deterministic() {
    let truth = disc(221, 167, 64);
    let source = textured_image();
    let target = with_delta(&source, &truth, -26);
    let field = field_from(&truth, -0.1);
    let (source_px, target_px, evidence) = proposal_inputs(&source, &target);
    let run = || propose_free_masks(
        &field, &vec![0.0; truth.len()], &source_px, &target_px, &evidence, 2,
    );
    let (a, b) = (run(), run());
    assert_eq!(a.len(), b.len());
    let run_full = || search_free_masks(
        &field, &vec![0.0; truth.len()], &source_px, &target_px, &evidence, 2,
    ).proposals;
    let (a_full, b_full) = (run_full(), run_full());
    for ((rank_a, a), (rank_b, b)) in a_full.iter().zip(&b_full) {
        assert_eq!(rank_a, rank_b);
        assert_eq!(a.mask.as_raw(), b.mask.as_raw());
        assert_eq!(a.mass.to_bits(), b.mass.to_bits());
        assert_eq!(a.sign.to_bits(), b.sign.to_bits());
        assert_eq!(a.share.0.to_bits(), b.share.0.to_bits());
        assert_eq!(a.share.1.to_bits(), b.share.1.to_bits());
    }
}

#[test]
fn free_masks_never_merge_diagonal_same_sign_blobs() {
    let mut region = vec![false; (EDGE * EDGE) as usize];
    for y in 80..150 { for x in 80..150 { region[(y * EDGE + x) as usize] = true; } }
    for y in 150..220 { for x in 150..220 { region[(y * EDGE + x) as usize] = true; } }
    let source = textured_image();
    let (source_px, target_px) = (fit::pixels_of(&source), fit::pixels_of(&source));
    let evidence = full_evidence(&source, &source);
    let search = search_free_masks(&field_from(&region, -0.1), &vec![0.0; region.len()],
        &source_px, &target_px, &evidence, 2);
    assert_eq!(search.proposals.len(), 2, "diagonal contact must not bridge 4-connectivity");
}

#[test]
fn free_mask_cap_discloses_the_third_component() {
    let mut region = vec![false; (EDGE * EDGE) as usize];
    for (x0, y0) in [(20usize, 20usize), (150, 20), (20, 150)] {
        for y in y0..y0 + 70 { for x in x0..x0 + 70 { region[y * EDGE as usize + x] = true; } }
    }
    let source = textured_image();
    let (source_px, target_px, evidence) = proposal_inputs(&source, &source);
    let search = search_free_masks(&field_from(&region, 0.1), &vec![0.0; region.len()],
        &source_px, &target_px, &evidence, 2);
    assert_eq!(search.proposals.len(), 2);
    assert!(search.refusals.iter().any(|r| r.n == 3 && r.why == FreeMaskWhy::Cap), "missing cap refusal");
}

#[test]
fn free_mask_refusal_keeps_refinement_and_typed_reason() {
    let truth = disc(221, 167, 64);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(EDGE, EDGE, |x, y| {
        let i = (y * EDGE + x) as usize;
        let value = if truth[i] { 40 + ((x * 37 + y * 53) % 180) as u8 } else { 10 };
        Rgb([value; 3])
    }));
    let current = render::develop_preview(&source, &crate::recipe::EditRecipe::default());
    let target = with_delta(&current, &truth, -26);
    let field = field_from(&truth, -0.1);
    let mut report = super::super::tests::neutral_report(&source, &target);
    report.evidence = full_evidence(&current, &target);
    let dir = std::env::temp_dir().join(format!("autoshop-free-refusal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let home = crate::store::OwnedRaster::scratch(dir.join("mask-zone-field.png"));
    let excluded = vec![0.0; truth.len()];
    FREE_MASK_FORCE_ZONE_REFUSAL.with(|force| force.set(true));
    attach_free_masks(&current, &target, &mut report, &home, &field, &excluded, true, 2);
    assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::MASK_REFINEMENT_KEPT
        || n.key == crate::rationale::keys::MASK_REFINEMENT_ABSTAINED), "{}", report.recipe.rationale);
    assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIELD_MASK_REFUSED));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn free_mask_attachment_emits_typed_note_with_improvement() {
    let truth = disc(221, 167, 64);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(EDGE, EDGE, |x, y| {
        let i = (y * EDGE + x) as usize;
        let value = if truth[i] { 40 + ((x * 37 + y * 53) % 180) as u8 } else { 10 };
        Rgb([value; 3])
    }));
    let current = render::develop_preview(&source, &crate::recipe::EditRecipe::default());
    let target = with_delta(&current, &truth, -26);
    let field = field_from(&truth, -0.1);
    let (source_px, target_px, evidence) = proposal_inputs(&current, &target);
    let mut report = super::super::tests::neutral_report(&source, &target);
    report.evidence = evidence;
    let dir = std::env::temp_dir().join(format!("autoshop-free-attached-note-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let home = crate::store::OwnedRaster::scratch(dir.join("mask-zone-field.png"));
    let excluded = vec![0.0; truth.len()];
    attach_free_masks(&source, &target, &mut report, &home, &field, &excluded, false, 2);
    let note = report.notes.iter().find(|n| n.key == crate::rationale::keys::FIELD_MASK_ATTACHED)
        .expect("synthetic fixture must attach one free mask");
    let value = |key: &str| note.args.iter().find(|(k, _)| *k == key).map(|(_, v)| v.parse::<f32>().unwrap()).unwrap();
    assert!(value("err_after") < value("err_before"));
    assert!(value("rim") <= ZONE_BOUNDARY_RIM_MAX);
    std::fs::remove_dir_all(dir).ok();
    let _ = (source_px, target_px);
}

#[test]
fn free_mask_bitmap_recipe_round_trip_and_xmp_loss_is_named() {
    let mut recipe = crate::recipe::EditRecipe::default();
    recipe.masks.push(crate::recipe::LocalAdjustment {
        mask: crate::recipe::MaskGeometry::Bitmap { path: "mask-zone-field.png".into() },
        name: "field-zone-1".into(),
        role: crate::recipe::MaskRole::Custom,
        exposure_ev: -0.1,
        ..Default::default()
    });
    let bytes = serde_json::to_vec(&recipe).unwrap();
    let decoded: crate::recipe::EditRecipe = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), bytes);
    let (_, losses) = crate::xmp::recipe_to_xmp_with_losses(&recipe);
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].name, "field-zone-1");
    assert_eq!(losses[0].reason, crate::xmp::MaskLossReason::Bitmap);
}

#[test]
fn free_mask_layer_off_is_byte_identical() {
    let (current, target, width, height) = crate::fit_field::tests::two_band_pair();
    let image = |pixels: &[[f32; 3]]| DynamicImage::ImageRgb8(RgbImage::from_fn(
        width, height, |x, y| Rgb(pixels[(y * width + x) as usize]
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)),
    ));
    let (source, target) = (image(&current), image(&target));
    let seg = crate::segment::SegmentOpts {
        python_bin: "autoshop-test-no-such-python".into(),
        script: "target/b3-no-segment.py".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let path = super::super::tests::fixture_mask_path("free-mask-layer-off");
    FREE_MASK_CALLS.with(|calls| calls.set(0));
    super::super::field::FIELD_CEILING_OVERRIDE.with(|value| value.set(Some(1.0)));
    let disabled = super::super::fit_recipe_zoned_inner(
        &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
        super::super::ZonedLayerOpts {
            field: true, spatial: false, free_masks: false, refine_masks: false,
        },
    );
    assert_eq!(FREE_MASK_CALLS.with(|calls| calls.get()), 0,
        "the disabled layer still entered the stage");

    super::super::field::FIELD_CEILING_OVERRIDE.with(|value| value.set(Some(1.0)));
    FREE_MASK_BYPASS.with(|bypass| bypass.set(true));
    let without_stage = super::super::fit_recipe_zoned_inner(
        &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
        super::super::ZonedLayerOpts {
            field: true, spatial: false, free_masks: true, refine_masks: false,
        },
    );
    assert_eq!(FREE_MASK_CALLS.with(|calls| calls.get()), 1,
        "the control run did not reach the bypassed stage");
    assert_eq!(serde_json::to_vec(&disabled.recipe).unwrap(),
        serde_json::to_vec(&without_stage.recipe).unwrap());
    assert_eq!(disabled.recipe.rationale.as_bytes(), without_stage.recipe.rationale.as_bytes());
    path.remove();

    for (override_value, spatial) in [(Some(1.0), false), (Some(0.003), true), (Some(0.0), true)] {
        let off_path = super::super::tests::fixture_mask_path("free-mask-layer-off-branch");
        super::super::field::FIELD_CEILING_OVERRIDE.with(|value| value.set(override_value));
        FREE_MASK_CALLS.with(|calls| calls.set(0));
        let off = super::super::fit_recipe_zoned_inner(
            &source, &target, &seg, &off_path, &crate::recipe::EditRecipe::default(), None,
            super::super::ZonedLayerOpts { field: true, spatial, free_masks: false, refine_masks: false },
        );
        assert_eq!(FREE_MASK_CALLS.with(|calls| calls.get()), 0);
        if let Some(stop) = off.notes.iter().find(|note| note.key == crate::rationale::keys::LOCAL_STOP) {
            let skipped = stop.args.iter().find(|(key, _)| *key == "skipped").map(|(_, value)| value.as_str()).unwrap();
            assert!(!skipped.contains("free masks"), "disabled stage leaked into stop disclosure: {skipped}");
        }
        assert!(!off.recipe.rationale.contains("Field mask "));
        off_path.remove();
    }
}

#[test]
fn free_masks_are_skipped_when_the_ceiling_is_met() {
    let source = textured_image();
    let mut target = source.to_rgb8();
    for y in EDGE / 2..3 * EDGE / 4 {
        for x in 0..EDGE / 4 {
            let pixel = target.get_pixel_mut(x, y);
            for value in &mut pixel.0 { *value = value.saturating_add(20); }
        }
    }
    let target = DynamicImage::ImageRgb8(target);
    let seg = crate::segment::SegmentOpts {
        python_bin: "autoshop-test-no-such-python".into(),
        script: "target/b3-no-segment.py".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let path = super::super::tests::fixture_mask_path("free-mask-stop-after-tiles");
    super::super::field::FIELD_CEILING_OVERRIDE.with(|value| value.set(Some(0.003)));
    FREE_MASK_CALLS.with(|calls| calls.set(0));
    let report = super::super::fit_recipe_zoned_inner(
        &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
        super::super::ZonedLayerOpts {
            field: true, spatial: true, free_masks: true, refine_masks: false,
        },
    );
    let stop = report.notes.iter().find(|note| note.key == crate::rationale::keys::LOCAL_STOP)
        .unwrap_or_else(|| panic!("missing stop: {}", report.recipe.rationale));
    assert!(stop.args.iter().any(|(key, value)| *key == "producer" && value == "tiles"),
        "{:?}", stop.args);
    assert!(stop.args.iter().any(|(key, value)| *key == "skipped" && value == "free masks"),
        "{:?}", stop.args);
    assert_eq!(FREE_MASK_CALLS.with(|calls| calls.get()), 0,
        "the stopped sequencer entered the free-mask stage");
    assert!(report.notes.iter().all(|note| note.key != crate::rationale::keys::FIELD_MASK_PROPOSED));
    path.remove();
}

fn corpus_segment_off() -> crate::segment::SegmentOpts {
    crate::segment::SegmentOpts {
        python_bin: "autoshop-test-no-such-python".into(),
        script: "target/b3-no-segment.py".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    }
}

fn corpus_layers(free_masks: bool) -> super::super::ZonedLayerOpts {
    super::super::ZonedLayerOpts {
        field: true,
        spatial: true,
        free_masks,
        refine_masks: true,
    }
}

fn assert_every_proposal_has_an_outcome(report: &fit::FitReport) {
    let stage = FREE_MASK_LAST_STAGE.with(|last| last.borrow_mut().take())
        .expect("free-mask stage did not publish its structured outcome");
    assert_eq!(stage.components, stage.disclosed,
        "every component must have a typed verdict");
    assert!(report.recipe.rationale.len() < 16 * 1024,
        "rationale grew to {} bytes", report.recipe.rationale.len());
    assert!(!report.recipe.rationale.to_ascii_lowercase().contains("truncat"),
        "pinned live rationale must not contain a truncation warning");
}

fn cleanup_corpus_run(report: &fit::FitReport, home: &crate::store::OwnedRaster) {
    for mask in &report.recipe.masks {
        if let crate::recipe::MaskGeometry::Bitmap { path } = &mask.mask {
            std::fs::remove_file(path).ok();
        }
    }
    home.remove();
}

#[test]
fn calibration_free_masks_disclose_every_component() {
    let Some(root) = fit::calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).unwrap();
    let target = image::open(root.join("target.jpg")).unwrap();
    let segment = corpus_segment_off();
    let head_home = super::super::tests::fixture_mask_path("free-mask-corpus-head");
    let live_home = super::super::tests::fixture_mask_path("free-mask-corpus-live");
    let head = super::super::fit_recipe_zoned_inner(
        &source, &target, &segment, &head_home, &crate::recipe::EditRecipe::default(), None,
        corpus_layers(false),
    );
    let report = super::super::fit_recipe_zoned_inner(
        &source, &target, &segment, &live_home, &crate::recipe::EditRecipe::default(), None,
        corpus_layers(true),
    );
    assert_every_proposal_has_an_outcome(&report);
    assert!(report.err_after <= head.err_after + 1e-6,
        "free masks regressed the frame: {} -> {}\n{}", head.err_after, report.err_after,
        report.recipe.rationale);
    cleanup_corpus_run(&head, &head_home);
    cleanup_corpus_run(&report, &live_home);
}

#[test]
fn p36_remainder_is_realised_or_honestly_refused() {
    let Some(root) = fit::calibration_corpus() else { return };
    let raw = root.join("p36.arw");
    let target_path = root.join("p36-target.jpg");
    if !raw.exists() || !target_path.exists() { return; }
    let source = crate::decode::preview_only(&raw).unwrap();
    let target = image::open(target_path).unwrap();
    let segment = corpus_segment_off();
    let head_home = super::super::tests::fixture_mask_path("free-mask-p36-head");
    let live_home = super::super::tests::fixture_mask_path("free-mask-p36-live");
    let head = super::super::fit_recipe_zoned_inner(
        &source, &target, &segment, &head_home, &crate::recipe::EditRecipe::default(), None,
        corpus_layers(false),
    );
    let report = super::super::fit_recipe_zoned_inner(
        &source, &target, &segment, &live_home, &crate::recipe::EditRecipe::default(), None,
        corpus_layers(true),
    );
    assert_every_proposal_has_an_outcome(&report);
    let attached = report.notes.iter()
        .filter(|note| note.key == crate::rationale::keys::FIELD_MASK_ATTACHED)
        .collect::<Vec<_>>();
    if attached.is_empty() {
        assert!(report.recipe.rationale.contains("Field mask component(s)")
                || report.recipe.rationale.contains("No field mask qualified:"),
            "the p36 remainder had no honest verdict: {}", report.recipe.rationale);
    } else {
        assert!(report.err_after < head.err_after,
            "an attached p36 mask did not improve HEAD: {} -> {}", head.err_after,
            report.err_after);
        for note in attached {
            let value = |key: &str| note.args.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.parse::<f32>().unwrap())
                .unwrap();
            let rim = value("rim");
            assert!(value("err_after") < value("err_before"));
            assert!(rim <= ZONE_BOUNDARY_RIM_MAX, "rim {rim} exceeded the shared budget");
        }
    }
    cleanup_corpus_run(&head, &head_home);
    cleanup_corpus_run(&report, &live_home);
}

/// Hand mutation M-A (2026-08-28) went green: dropping the `LOCAL_REALIZED`
/// reading after the free-mask stage broke nothing, because every earlier
/// test only asked whether *some* realized note existed. The stage's own
/// producer reading is the disclosure that the field was re-priced after the
/// last producer, so it is pinned by producer name here, and its absence is
/// pinned when the layer is off.
#[test]
fn free_mask_stage_publishes_its_own_realized_reading() {
    let (current, target, width, height) = crate::fit_field::tests::two_band_pair();
    let image = |pixels: &[[f32; 3]]| DynamicImage::ImageRgb8(RgbImage::from_fn(
        width, height, |x, y| Rgb(pixels[(y * width + x) as usize]
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)),
    ));
    let (source, target) = (image(&current), image(&target));
    let seg = corpus_segment_off();
    let realized_by = |report: &fit::FitReport| report.notes.iter()
        .filter(|note| note.key == crate::rationale::keys::LOCAL_REALIZED)
        .filter_map(|note| note.args.iter()
            .find(|(key, _)| *key == "producer").map(|(_, value)| value.clone()))
        .collect::<Vec<_>>();
    for free_masks in [true, false] {
        let path = super::super::tests::fixture_mask_path("free-mask-realized-reading");
        super::super::field::FIELD_CEILING_OVERRIDE.with(|value| value.set(Some(1.0)));
        FREE_MASK_CALLS.with(|calls| calls.set(0));
        let report = super::super::fit_recipe_zoned_inner(
            &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
            super::super::ZonedLayerOpts {
                field: true, spatial: false, free_masks, refine_masks: false,
            },
        );
        let producers = realized_by(&report);
        assert_eq!(FREE_MASK_CALLS.with(|calls| calls.get()), usize::from(free_masks));
        assert_eq!(producers.iter().any(|p| p == "free masks"), free_masks,
            "realized producers {producers:?}
{}", report.recipe.rationale);
        path.remove();
    }
}

/// Hand mutation M-C (2026-08-28) went green: deleting the typed refusal on
/// the REAL `attach_one_zone -> None` branch broke nothing, because the only
/// zone-refused test drove the `FREE_MASK_FORCE_ZONE_REFUSAL` shortcut. This
/// fixture reaches the real branch: the field claims a remainder on a disc
/// whose pixels already match the target, so the zone estimator answers
/// ZONE_ALREADY_MATCHED and the stage must still write `zone-refused`.
#[test]
fn free_mask_real_zone_refusal_is_typed() {
    let truth = disc(221, 167, 64);
    let source = textured_image();
    let field = field_from(&truth, -0.1);
    let (_, _, evidence) = proposal_inputs(&source, &source);
    let mut report = super::super::tests::neutral_report(&source, &source);
    report.evidence = evidence;
    let dir = std::env::temp_dir().join(format!("autoshop-free-real-refusal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let home = crate::store::OwnedRaster::scratch(dir.join("mask-zone-field.png"));
    let masks = report.recipe.masks.len();
    let stage = attach_free_masks(
        &source, &source, &mut report, &home, &field, &vec![0.0; truth.len()], false, 2,
    );
    assert!(stage.ran);
    assert_eq!(stage.components, stage.disclosed, "{}", report.recipe.rationale);
    assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIELD_MASK_PROPOSED),
        "the matching disc must still be proposed: {}", report.recipe.rationale);
    let refused = report.notes.iter()
        .filter(|n| n.key == crate::rationale::keys::FIELD_MASK_REFUSED)
        .filter(|n| n.args.iter().any(|(k, v)| *k == "why" && v == "zone-refused"))
        .count();
    assert_eq!(refused, 1, "{}", report.recipe.rationale);
    assert_eq!(report.recipe.masks.len(), masks, "a refused mask must not stay attached");
    assert!(report.notes.iter().all(|n| n.key != crate::rationale::keys::FIELD_MASK_ATTACHED));
    std::fs::remove_dir_all(dir).ok();
}
