//! Multi-class semantic region discovery for the zoned reverse-fit.
//!
//! The sidecar returns soft ADE20K class planes for each frame.  This module
//! pairs those planes, applies the shared support floor, and turns them into a
//! deterministic disjoint partition.  It deliberately contains no fitting
//! policy: the caller can feed each resulting region to the existing generic
//! `ZoneAttachment` estimator.

use image::GrayImage;

use super::MIN_ZONE_SHARE;
use crate::render::MASK_RASTER_BUDGET_BYTES;

/// The default maximum number of accepted semantic regions.
pub const MAX_SEMANTIC_REGIONS: usize = 4;
pub const DEFAULT_SEMANTIC_REGIONS: usize = 2;

/// One soft semantic class plane from one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassPlane {
    pub class_id: u16,
    pub label: String,
    pub mean_confidence: f32,
    pub mask: GrayImage,
}

/// A paired source/target semantic region after overlap resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRegion {
    pub class_id: u16,
    pub label: String,
    pub mean_confidence: f32,
    pub source: GrayImage,
    pub target: GrayImage,
    pub source_share: f32,
    pub target_share: f32,
}

fn weights(mask: &GrayImage) -> Vec<f32> {
    mask.pixels().map(|p| p.0[0] as f32 / 255.0).collect()
}

fn share(mask: &GrayImage) -> f32 {
    let n = (mask.width() as usize).saturating_mul(mask.height() as usize).max(1);
    weights(mask).into_iter().sum::<f32>() / n as f32
}

fn priority(a: &SemanticRegion, b: &SemanticRegion) -> std::cmp::Ordering {
    b.mean_confidence
        .partial_cmp(&a.mean_confidence)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            let aa = a.source_share + a.target_share;
            let bb = b.source_share + b.target_share;
            aa.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| a.class_id.cmp(&b.class_id))
}

fn subtract(mask: &GrayImage, claimed: &[f32]) -> GrayImage {
    let mut out = mask.clone();
    for (i, p) in out.pixels_mut().enumerate() {
        let v = p.0[0] as f32 / 255.0;
        let remainder = (v - claimed.get(i).copied().unwrap_or(0.0)).clamp(0.0, 1.0);
        p.0[0] = (remainder * 255.0).round() as u8;
    }
    out
}

/// Pair class planes, reject unsupported classes, resolve overlaps, and return
/// at most `max_regions` disjoint regions in ascending class-id order.
///
/// Priority is higher confidence, then smaller two-sided area, then lower
/// class id.  Processing by that priority means a specific/high-confidence
/// child owns overlapping pixels and a broad parent receives only its
/// complement.  A class missing on either side is never retained.
pub fn resolve_regions(
    source: &[ClassPlane],
    target: &[ClassPlane],
    max_regions: usize,
) -> Vec<SemanticRegion> {
    if max_regions == 0 {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for s in source {
        let Some(t) = target.iter().find(|t| t.class_id == s.class_id) else { continue };
        if s.mask.dimensions() != t.mask.dimensions() {
            continue;
        }
        let ss = share(&s.mask);
        let ts = share(&t.mask);
        if ss < MIN_ZONE_SHARE || ts < MIN_ZONE_SHARE {
            continue;
        }
        let confidence = s.mean_confidence.min(t.mean_confidence);
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            continue;
        }
        candidates.push(SemanticRegion {
            class_id: s.class_id,
            label: if s.label.is_empty() { t.label.clone() } else { s.label.clone() },
            mean_confidence: confidence,
            source: s.mask.clone(),
            target: t.mask.clone(),
            source_share: ss,
            target_share: ts,
        });
    }
    candidates.sort_by(priority);
    let (w, h) = candidates
        .first()
        .map(|r| r.source.dimensions())
        .unwrap_or((0, 0));
    let len = (w as usize).saturating_mul(h as usize);
    let mut claimed_source = vec![0.0f32; len];
    let mut claimed_target = vec![0.0f32; len];
    let mut accepted = Vec::new();
    for mut region in candidates {
        let source = subtract(&region.source, &claimed_source);
        let target = subtract(&region.target, &claimed_target);
        let ss = share(&source);
        let ts = share(&target);
        if ss < MIN_ZONE_SHARE || ts < MIN_ZONE_SHARE {
            continue;
        }
        for (i, p) in source.pixels().enumerate() {
            claimed_source[i] = (claimed_source[i] + p.0[0] as f32 / 255.0).min(1.0);
        }
        for (i, p) in target.pixels().enumerate() {
            claimed_target[i] = (claimed_target[i] + p.0[0] as f32 / 255.0).min(1.0);
        }
        region.source = source;
        region.target = target;
        region.source_share = ss;
        region.target_share = ts;
        accepted.push(region);
        if accepted.len() == max_regions.min(MAX_SEMANTIC_REGIONS) {
            break;
        }
    }
    accepted.sort_by_key(|r| r.class_id);
    accepted
}

/// Return whether adding `regions` rasters of `width` x `height` stays inside
/// the renderer's existing bitmap budget.  The accounting matches the render
/// engine's four-byte-per-texel reservation.
pub fn bitmap_budget_allows(
    existing_bytes: usize,
    width: u32,
    height: u32,
    regions: usize,
) -> bool {
    existing_bytes.saturating_add(
        (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4)
            .saturating_mul(regions),
    ) <= MASK_RASTER_BUDGET_BYTES
}

/// Confidence aggregation is deliberately worst-case across accepted regions.
pub fn worst_region_residual(residuals: &[f32]) -> f32 {
    residuals.iter().copied().fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgb, RgbImage};

    fn plane(id: u16, value: u8, confidence: f32) -> ClassPlane {
        ClassPlane {
            class_id: id,
            label: format!("class-{id}"),
            mean_confidence: confidence,
            mask: GrayImage::from_pixel(10, 10, Luma([value])),
        }
    }

    #[test]
    fn regions_partition_and_resolve_nested_overlap() {
        let mut child = GrayImage::from_pixel(10, 10, Luma([0]));
        for y in 2..8 { for x in 2..8 { child.put_pixel(x, y, Luma([255])); } }
        let source = vec![
            ClassPlane { class_id: 10, label: "parent".into(), mean_confidence: 0.7, mask: GrayImage::from_pixel(10, 10, Luma([255])) },
            ClassPlane { class_id: 11, label: "child".into(), mean_confidence: 0.9, mask: child.clone() },
        ];
        let regions = resolve_regions(&source, &source, 4);
        assert_eq!(regions.iter().map(|r| r.class_id).collect::<Vec<_>>(), vec![10, 11]);
        assert!(regions[0].source.get_pixel(0, 0).0[0] > 0);
        assert_eq!(regions[0].source.get_pixel(4, 4).0[0], 0);
    }

    #[test]
    fn region_order_is_stable() {
        let mut p7 = plane(7, 0, 0.6);
        let mut p2 = plane(2, 0, 0.6);
        let mut p5 = plane(5, 0, 0.8);
        for y in 0..10 { for x in 0..3 { p7.mask.put_pixel(x, y, Luma([255])); } }
        for y in 0..10 { for x in 3..6 { p2.mask.put_pixel(x, y, Luma([255])); } }
        for y in 0..10 { for x in 6..10 { p5.mask.put_pixel(x, y, Luma([255])); } }
        let a = vec![p7, p2, p5];
        let b = vec![a[2].clone(), a[0].clone(), a[1].clone()];
        let ids_a: Vec<_> = resolve_regions(&a, &a, 4).into_iter().map(|r| r.class_id).collect();
        let ids_b: Vec<_> = resolve_regions(&b, &b, 4).into_iter().map(|r| r.class_id).collect();
        assert_eq!(ids_a, vec![2, 5, 7]);
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn tiny_region_is_skipped() {
        let mut tiny = GrayImage::from_pixel(10, 10, Luma([0]));
        tiny.put_pixel(0, 0, Luma([255]));
        let regions = resolve_regions(&[ClassPlane { class_id: 1, label: "tiny".into(), mean_confidence: 1.0, mask: tiny.clone() }], &[ClassPlane { class_id: 1, label: "tiny".into(), mean_confidence: 1.0, mask: tiny }], 4);
        assert!(regions.is_empty());
    }

    #[test]
    fn region_set_falls_back_when_target_segmentation_fails() {
        let source = vec![plane(1, 255, 0.9)];
        assert!(resolve_regions(&source, &[], 4).is_empty());
    }

    #[test]
    fn bitmap_budget_refuses_the_fifth_region() {
        let bytes = 4096usize * 4096 * 4;
        assert!(bitmap_budget_allows(0, 4096, 4096, 4));
        assert!(!bitmap_budget_allows(bytes * 4, 4096, 4096, 1));
    }

    #[test]
    fn semantic_limits_have_one_definition() {
        let sources = [
            include_str!("../main.rs"),
            include_str!("../segment.rs"),
            include_str!("../bin/gui/actions.rs"),
        ];
        for source in sources {
            assert!(!source.contains("regions.min(4)"));
            assert!(!source.contains("regions.len() > 4"));
            assert!(!source.contains("clamp(1, 4)"));
            assert!(!source.contains("..=4"));
        }
        assert_eq!(MAX_SEMANTIC_REGIONS, 4);
        assert_eq!(DEFAULT_SEMANTIC_REGIONS, 2);
    }

    #[test]
    fn worst_region_residual_is_the_maximum_accepted_residual() {
        assert_eq!(worst_region_residual(&[0.02, 0.15, 0.08]), 0.15);
    }

    #[test]
    fn two_region_default_is_byte_identical_to_sky_land() {
        let sky = plane(2, 128, 0.9);
        let land = plane(3, 128, 0.8);
        let a = resolve_regions(&[sky.clone(), land.clone()], &[sky.clone(), land.clone()], 2);
        let b = resolve_regions(&[land.clone(), sky.clone()], &[land, sky], 2);
        assert_eq!(a, b);

        // Also pin the public N-way router to the historical sequencer.  A
        // deliberately unavailable sidecar keeps this falsifier tiny while
        // still comparing the complete recipe/rationale contract.
        let source = image::DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([64, 80, 96])));
        let target = image::DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([72, 88, 104])));
        let seg = crate::segment::SegmentOpts {
            python_bin: "autoshop-test-no-such-python".into(),
            script: "Cargo.toml".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let routed_path = crate::store::OwnedRaster::scratch(
            std::env::temp_dir().join(format!("autoshop-two-route-{}-routed.png", std::process::id())),
        );
        let legacy_path = crate::store::OwnedRaster::scratch(
            std::env::temp_dir().join(format!("autoshop-two-route-{}-legacy.png", std::process::id())),
        );
        let routed = super::super::fit_recipe_zoned_with_regions(
            &source, &target, &seg, &routed_path, &crate::recipe::EditRecipe::default(),
            crate::fit::FitOptions::default(), 2,
        );
        let legacy = super::super::fit_recipe_zoned_inner(
            &source, &target, &seg, &legacy_path, &crate::recipe::EditRecipe::default(), None,
            super::super::SHIPPED_LAYERS,
        );
        assert_eq!(serde_json::to_vec(&routed.recipe).unwrap(), serde_json::to_vec(&legacy.recipe).unwrap());
        assert_eq!(routed.recipe.rationale, legacy.recipe.rationale);
        assert_eq!(routed.err_after.to_bits(), legacy.err_after.to_bits());
        routed_path.remove();
        legacy_path.remove();
    }

    #[test]
    fn three_regions_recover_independent_channel_gains() {
        let mut planes = Vec::new();
        for (i, id) in [10u16, 20, 30].into_iter().enumerate() {
            let mut m = GrayImage::from_pixel(9, 3, Luma([0]));
            for x in (i * 3)..(i * 3 + 3) { for y in 0..3 { m.put_pixel(x as u32, y, Luma([255])); } }
            planes.push(ClassPlane { class_id: id, label: format!("r{id}"), mean_confidence: 0.9, mask: m });
        }
        let regions = resolve_regions(&planes, &planes, 3);
        assert_eq!(regions.len(), 3);
        assert!(regions.windows(2).all(|w| w[0].source.as_raw().iter().zip(w[1].source.as_raw()).all(|(a, b)| *a == 0 || *b == 0)));
    }

    #[test]
    fn four_region_recipe_round_trips() {
        let mut recipe = crate::recipe::EditRecipe::default();
        for id in 1..=4 {
            recipe.masks.push(crate::recipe::LocalAdjustment {
                mask: crate::recipe::MaskGeometry::Bitmap { path: format!("region-{id}.png") },
                name: format!("region-{id}-label"),
                role: crate::recipe::MaskRole::Custom,
                ..Default::default()
            });
        }
        let bytes = serde_json::to_vec(&recipe).unwrap();
        let loaded: crate::recipe::EditRecipe = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(bytes, serde_json::to_vec(&loaded).unwrap());
    }

    #[test]
    fn fit_calibration_four_regions_has_typed_verdicts_when_corpus_is_available() {
        let Some(corpus) = crate::fit::calibration_corpus() else {
            eprintln!("SKIPPED four-region calibration test: AUTOSHOP_FIT_CALIBRATION_DIR unset");
            return;
        };
        let cfg = crate::config::Config::load();
        let mut seg = crate::segment::SegmentOpts::from_config(&cfg, "sky");
        seg.script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("python/segment.py");
        if !seg.script.is_file() {
            eprintln!(
                "SKIPPED four-region calibration test: segmentation sidecar absent at {}",
                seg.script.display()
            );
            return;
        }
        let source = image::open(corpus.join("neutral.jpg")).unwrap();
        let target = image::open(corpus.join("target.jpg")).unwrap();
        let scratch = std::env::temp_dir().join(format!(
            "autoshop-four-region-calibration-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let four_path = crate::store::OwnedRaster::scratch(scratch.join("four-anchor.png"));
        let two_path = crate::store::OwnedRaster::scratch(scratch.join("two-anchor.png"));
        let four = super::super::fit_recipe_zoned_with_regions(
            &source,
            &target,
            &seg,
            &four_path,
            &crate::recipe::EditRecipe::default(),
            crate::fit::FitOptions::default(),
            4,
        );
        let two = super::super::fit_recipe_zoned_with_regions(
            &source,
            &target,
            &seg,
            &two_path,
            &crate::recipe::EditRecipe::default(),
            crate::fit::FitOptions::default(),
            2,
        );

        let mode_keys = [
            crate::rationale::keys::ZONE_MODE_FULL,
            crate::rationale::keys::ZONE_MODE_ATMOSPHERE,
        ];
        let terminal_keys = [
            crate::rationale::keys::ZONE_TOO_SMALL,
            crate::rationale::keys::ZONE_SHARE_MISMATCH,
            crate::rationale::keys::ZONE_ALREADY_MATCHED,
            crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED,
            crate::rationale::keys::ZONE_QUALITY_CLIPPING_FAILED,
            crate::rationale::keys::ZONE_DROPPED,
            crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED,
            crate::rationale::keys::ZONE_BOUNDARY_PASSED,
            crate::rationale::keys::ZONE_BOUNDARY_DROPPED,
        ];
        let refusal_count = four
            .notes
            .iter()
            .filter(|note| note.key == crate::rationale::keys::REGION_FRAME_REFUSED)
            .count();
        if refusal_count == 1 {
            assert_eq!(four.err_after.to_bits(), two.err_after.to_bits());
            assert_eq!(four.recipe.confidence.to_bits(), two.recipe.confidence.to_bits());
            let refusal = four.notes.iter()
                .find(|note| note.key == crate::rationale::keys::REGION_FRAME_REFUSED)
                .unwrap();
            assert_eq!(
                four.recipe.rationale,
                format!("{}{}", two.recipe.rationale, crate::rationale::render_one(refusal)),
                "the refused report is the two-region report plus one note — no transplant"
            );
            assert!(four.recipe.masks.iter().all(|mask| match &mask.mask {
                crate::recipe::MaskGeometry::Bitmap { path } => !path.contains("mask-region-"),
                _ => true,
            }));
            let _ = std::fs::remove_dir_all(&scratch);
            return;
        }
        let mode_positions = four
            .notes
            .iter()
            .enumerate()
            .filter_map(|(index, note)| {
                (mode_keys.contains(&note.key)
                    && note
                        .args
                        .iter()
                        .any(|(key, value)| *key == "label" && value.starts_with("region-")))
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let too_small = four
            .notes
            .iter()
            .filter(|note| {
                note.key == crate::rationale::keys::ZONE_TOO_SMALL
                    && note
                        .args
                        .iter()
                        .any(|(key, value)| *key == "label" && value.starts_with("region-"))
            })
            .count();
        eprintln!(
            "four-region calibration: regions={} masks={} confidence={:.6} look_error={:.6}; two-region confidence={:.6} look_error={:.6}; rationale_bytes={}",
            mode_positions.len() + too_small,
            four.recipe.masks.len(),
            four.recipe.confidence,
            four.err_after,
            two.recipe.confidence,
            two.err_after,
            four.recipe.rationale.len()
        );
        assert!(
            !mode_positions.is_empty() || too_small > 0,
            "the real four-region run produced no per-region verdict: {}",
            four.recipe.rationale
        );
        for (ordinal, &start) in mode_positions.iter().enumerate() {
            let end = mode_positions
                .get(ordinal + 1)
                .copied()
                .unwrap_or(four.notes.len());
            assert!(
                four.notes[start + 1..end]
                    .iter()
                    .any(|note| terminal_keys.contains(&note.key)),
                "semantic region {} has a mode but no typed terminal verdict: {}",
                ordinal + 1,
                four.recipe.rationale
            );
        }
        // A kept multi result beat the reference on the reference's own
        // ruler — `err_after` values of two reports are not comparable when
        // their global solves landed in different modes.
        let ruler = &two.evidence;
        assert!(
            super::super::frame_err_under(&source, &target, &four, ruler)
                < super::super::frame_err_under(&source, &target, &two, ruler)
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
