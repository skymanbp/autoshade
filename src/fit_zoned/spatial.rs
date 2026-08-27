use std::collections::BTreeSet;

use image::{DynamicImage, GrayImage, Luma};

use super::*;

pub(super) const SPATIAL_MAX_DEPTH: u8 = 2;
const SPATIAL_MAX_ATTACHMENTS: usize = 4;
const SPATIAL_RESIDUAL_MIN: f32 = 2.0 / 255.0;
const SPATIAL_FRAME_REGRESSION_TOL: f32 = 0.0;
const TILE_RASTER_EDGE: u32 = 2048;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct TileId {
    pub(super) depth: u8,
    pub(super) row: u8,
    pub(super) col: u8,
}

impl TileId {
    fn grid(self) -> u32 {
        1u32 << self.depth
    }

    fn label(self) -> String {
        format!("Spatial tile r{}c{}", self.row, self.col)
    }

    fn tag(self) -> String {
        format!("d{}r{}c{}", self.depth, self.row, self.col)
    }

}

#[derive(Clone, Debug)]
pub(super) struct TileReading {
    pub(super) id: TileId,
    pub(super) source_weights: Vec<f32>,
    pub(super) target_weights: Vec<f32>,
    pub(super) source_share: f32,
    pub(super) target_share: f32,
    pub(super) residual: f32,
    pub(super) ci95: f32,
    pub(super) divergence: fit::Divergence,
}

impl TileReading {
    fn score(&self) -> f32 {
        self.residual.abs() * self.source_share.min(self.target_share)
    }
}

fn in_tile(id: TileId, x: u32, y: u32, width: u32, height: u32) -> bool {
    x * id.grid() / width.max(1) == id.col as u32
        && y * id.grid() / height.max(1) == id.row as u32
}

fn weighted_residual(
    current: &[[f32; 3]],
    target: &[[f32; 3]],
    weights: &[f32],
) -> (f32, f32) {
    let mut sum_w = 0.0f64;
    let mut sum_w2 = 0.0f64;
    let mut sum = 0.0f64;
    for ((source, target), weight) in current.iter().zip(target).zip(weights) {
        let weight = weight.max(0.0) as f64;
        if weight == 0.0 {
            continue;
        }
        let residual = (fit::luma601(target) - fit::luma601(source)) as f64;
        sum_w += weight;
        sum_w2 += weight * weight;
        sum += weight * residual;
    }
    if sum_w <= 0.0 || sum_w2 <= 0.0 {
        return (0.0, f32::INFINITY);
    }
    let mean = sum / sum_w;
    let mut variance = 0.0f64;
    for ((source, target), weight) in current.iter().zip(target).zip(weights) {
        let weight = weight.max(0.0) as f64;
        if weight == 0.0 {
            continue;
        }
        let residual = (fit::luma601(target) - fit::luma601(source)) as f64;
        variance += weight * (residual - mean).powi(2);
    }
    variance /= sum_w;
    let effective_n = sum_w * sum_w / sum_w2;
    let ci95 = 1.96 * (variance / effective_n.max(1.0)).sqrt();
    (mean as f32, ci95 as f32)
}

fn read_tile(
    id: TileId,
    current: &[[f32; 3]],
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
) -> TileReading {
    let n = (evidence.width as usize * evidence.height as usize)
        .min(current.len())
        .min(target.len())
        .min(evidence.source_weights.len())
        .min(evidence.target_weights.len());
    let mut source_weights = vec![0.0f32; n];
    let mut target_weights = vec![0.0f32; n];
    let mut geometry = vec![0.0f32; n];
    for y in 0..evidence.height {
        for x in 0..evidence.width {
            let i = (y * evidence.width + x) as usize;
            if i < n && in_tile(id, x, y, evidence.width, evidence.height) {
                source_weights[i] = evidence.source_weights[i];
                target_weights[i] = evidence.target_weights[i];
                geometry[i] = 1.0;
            }
        }
    }
    let source_share = source_weights.iter().map(|v| *v as f64).sum::<f64>() as f32
        / n.max(1) as f32;
    let target_share = target_weights.iter().map(|v| *v as f64).sum::<f64>() as f32
        / n.max(1) as f32;
    let (residual, ci95) = weighted_residual(current, target, &source_weights);
    let divergence = fit::structure_divergence(
        &evidence.source_pixels[..n.min(evidence.source_pixels.len())],
        &target[..n],
        evidence.width,
        evidence.height,
        &geometry,
    );
    TileReading {
        id,
        source_weights,
        target_weights,
        source_share,
        target_share,
        residual,
        ci95,
        divergence,
    }
}

fn eligible(reading: &TileReading, parent_residual: f32) -> Result<(), &'static str> {
    if reading.source_share < MIN_ZONE_SHARE {
        Err("source-share")
    } else if reading.target_share < MIN_ZONE_SHARE {
        Err("target-share")
    } else if reading.divergence.d >= fit::DIVERGENCE_ZONE {
        Err("structural-divergence")
    } else if !reading.ci95.is_finite() || reading.residual.abs() <= reading.ci95 {
        Err("confidence-interval")
    } else if (reading.residual - parent_residual).abs() < SPATIAL_RESIDUAL_MIN {
        Err("parent-residual")
    } else {
        Ok(())
    }
}

fn reading_args(
    reading: &TileReading,
    parent_residual: f32,
) -> Vec<(&'static str, String)> {
    vec![
        ("id", reading.id.tag()),
        ("s", format!("{:.3}", reading.source_share)),
        ("t", format!("{:.3}", reading.target_share)),
        ("d", format!("{:.3}", reading.divergence.d)),
        ("residual", format!("{:+.5}", reading.residual)),
        ("parent", format!("{:+.5}", parent_residual)),
        ("ci", format!("{:.5}", reading.ci95)),
    ]
}

fn tile_mask(src: &DynamicImage, id: TileId) -> (DynamicImage, GrayImage) {
    let guide = src.thumbnail(TILE_RASTER_EDGE, TILE_RASTER_EDGE);
    let mut mask = GrayImage::new(guide.width(), guide.height());
    for y in 0..mask.height() {
        for x in 0..mask.width() {
            let value = if in_tile(id, x, y, mask.width(), mask.height()) { 255 } else { 0 };
            mask.put_pixel(x, y, Luma([value]));
        }
    }
    (guide, mask)
}

fn push_refinement_note(
    report: &mut FitReport,
    label: &str,
    kept: bool,
    reading: crate::mask_refine::RefineReading,
) {
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            if kept {
                crate::rationale::keys::MASK_REFINEMENT_KEPT
            } else {
                crate::rationale::keys::MASK_REFINEMENT_ABSTAINED
            },
            vec![
                ("label", label.to_string()),
                ("coverage", format!("{:.6}", reading.coverage_delta)),
                ("before", format!("{:.6}", reading.edge_before)),
                ("after", format!("{:.6}", reading.edge_after)),
                ("core", reading.core_changed.to_string()),
            ],
        ),
    );
}

fn push_abstention(
    report: &mut FitReport,
    reading: &TileReading,
    parent_residual: f32,
    reason: &str,
    generation: usize,
) {
    let mut args = reading_args(reading, parent_residual);
    args.push(("reason", reason.to_string()));
    args.push(("generation", generation.to_string()));
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(crate::rationale::keys::TILE_ABSTAINED, args),
    );
}

#[derive(Clone, Debug)]
struct PendingTile {
    reading: TileReading,
    parent_residual: f32,
}

type TileVisit = (TileReading, f32, Result<(), &'static str>);
type TileSearch = (Vec<TileVisit>, Option<TileReading>);

fn rank_pending(pending: &mut [PendingTile]) {
    pending.sort_by(|a, b| {
        b.reading
            .score()
            .total_cmp(&a.reading.score())
            .then_with(|| a.reading.id.cmp(&b.reading.id))
    });
}

fn next_tile(
    current: &[[f32; 3]],
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    attached: &BTreeSet<TileId>,
    refused: &BTreeSet<TileId>,
) -> TileSearch {
    let root = read_tile(TileId { depth: 0, row: 0, col: 0 }, current, target, evidence);
    let mut pending = Vec::new();
    for row in 0..2 {
        for col in 0..2 {
            pending.push(PendingTile {
                reading: read_tile(TileId { depth: 1, row, col }, current, target, evidence),
                parent_residual: root.residual,
            });
        }
    }
    let mut visited = Vec::new();
    while !pending.is_empty() {
        rank_pending(&mut pending);
        let node = pending.remove(0);
        let verdict = eligible(&node.reading, node.parent_residual);
        let eligible_node = verdict.is_ok();
        visited.push((node.reading.clone(), node.parent_residual, verdict));
        if node.reading.id.depth == SPATIAL_MAX_DEPTH {
            if eligible_node
                && !attached.contains(&node.reading.id)
                && !refused.contains(&node.reading.id)
            {
                return (visited, Some(node.reading));
            }
            continue;
        }
        let depth = node.reading.id.depth + 1;
        for row_offset in 0..2 {
            for col_offset in 0..2 {
                pending.push(PendingTile {
                    reading: read_tile(
                        TileId {
                            depth,
                            row: node.reading.id.row * 2 + row_offset,
                            col: node.reading.id.col * 2 + col_offset,
                        },
                        current,
                        target,
                        evidence,
                    ),
                    parent_residual: node.reading.residual,
                });
            }
        }
    }
    (visited, None)
}

fn tile_attachment(
    reading: &TileReading,
    path: &std::path::Path,
    source_weights: Vec<f32>,
    target_weights: Vec<f32>,
) -> ZoneAttachment {
    ZoneAttachment {
        source_weights,
        target_weights,
        mask: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() },
        range: None,
        name: reading.id.label(),
        role: MaskRole::Custom,
        inverted: false,
        label: reading.id.label(),
        min_share: MIN_ZONE_SHARE,
        frame_regression_tol: SPATIAL_FRAME_REGRESSION_TOL,
    }
}

fn boundary_args(
    id: TileId,
    k: f32,
    before: BoundaryReading,
    after: BoundaryReading,
) -> Vec<(&'static str, String)> {
    vec![
        ("id", id.tag()),
        ("k", format!("{k:.3}")),
        ("before", format!("{:.3}", before.rim)),
        ("after", format!("{:.3}", after.rim)),
        ("max", format!("{ZONE_BOUNDARY_RIM_MAX:.3}")),
        ("transitions", after.transitions.to_string()),
    ]
}

fn enforce_tile_boundary(
    s_img: &DynamicImage,
    tgt_px: &[[f32; 3]],
    report: &mut FitReport,
    first_tile: usize,
    input: TileBoundaryInput<'_>,
) -> Option<(Vec<[f32; 3]>, BoundaryReading)> {
    let initial = boundary_rim(
        &input.initial_px,
        input.weights,
        s_img.width(),
        s_img.height(),
    );
    let original = report.recipe.masks[first_tile].clone();
    let render_at = |report: &mut FitReport, k: f32| {
        shrink_zone_corrections(
            &mut report.recipe.masks[first_tile..=first_tile],
            std::slice::from_ref(&original),
            &[1.0],
            k,
        );
        let pixels = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let reading = boundary_rim(&pixels, input.weights, s_img.width(), s_img.height());
        let frame = fit::look_err_with_evidence(&pixels, tgt_px, &report.evidence);
        (reading, pixels, frame)
    };
    let mut kept = if initial.rim <= ZONE_BOUNDARY_RIM_MAX {
        let frame = fit::look_err_with_evidence(&input.initial_px, tgt_px, &report.evidence);
        Some((1.0, initial, input.initial_px, frame))
    } else {
        let zero = render_at(report, 0.0);
        if zero.0.rim > ZONE_BOUNDARY_RIM_MAX {
            None
        } else {
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            let mut best = (0.0, zero.0, zero.1, zero.2);
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                let measured = render_at(report, mid);
                if measured.0.rim <= ZONE_BOUNDARY_RIM_MAX {
                    lo = mid;
                    best = (mid, measured.0, measured.1, measured.2);
                } else {
                    hi = mid;
                }
            }
            Some(best)
        }
    };
    if kept.as_ref().is_some_and(|(_, _, _, frame)| {
        *frame > input.frame_before + SPATIAL_FRAME_REGRESSION_TOL
    }) {
        kept = None;
    }
    match kept {
        Some((k, reading, pixels, _)) => {
            shrink_zone_corrections(
                &mut report.recipe.masks[first_tile..=first_tile],
                std::slice::from_ref(&original),
                &[1.0],
                k,
            );
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::TILE_BOUNDARY_PASSED,
                    boundary_args(input.id, k, initial, reading),
                ),
            );
            Some((pixels, reading))
        }
        None => {
            report.recipe.masks.truncate(first_tile);
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::TILE_BOUNDARY_REFUSED,
                    boundary_args(input.id, 0.0, initial, initial),
                ),
            );
            None
        }
    }
}

struct TileBoundaryInput<'a> {
    weights: &'a [f32],
    initial_px: Vec<[f32; 3]>,
    frame_before: f32,
    id: TileId,
}

pub(super) fn attach_tiles(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    raster_home: &crate::store::OwnedRaster,
    refine: bool,
) {
    let s_img = src.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let t_img = target.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let tgt_px = fit::pixels_of(&t_img);
    let mut attached = BTreeSet::new();
    let mut refused = BTreeSet::new();
    let mut generation = 0usize;
    let corr = report.correspondence.take();
    while attached.len() < SPATIAL_MAX_ATTACHMENTS {
        let current = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let root = read_tile(
            TileId { depth: 0, row: 0, col: 0 },
            &current,
            &tgt_px,
            &report.evidence,
        );
        let (visited, candidate) =
            next_tile(&current, &tgt_px, &report.evidence, &attached, &refused);
        // ONE aggregated sweep note per generation: the full per-node map
        // re-rendered every generation was a transcript, and it truncated the
        // attachment disclosure off the persisted rationale. Leaf candidates
        // keep their full reading; every other verdict survives as id-in-
        // bucket; nodes already attached or refused told their story in their
        // own generation.
        let mut sweep: [(&str, Vec<String>); 7] = [
            ("eligible", Vec::new()),
            ("source-share", Vec::new()),
            ("target-share", Vec::new()),
            ("structural-divergence", Vec::new()),
            ("confidence-interval", Vec::new()),
            ("parent-residual", Vec::new()),
            ("other", Vec::new()),
        ];
        for (reading, parent_residual, verdict) in visited {
            if attached.contains(&reading.id) || refused.contains(&reading.id) {
                continue;
            }
            match verdict {
                Ok(()) if reading.id.depth == SPATIAL_MAX_DEPTH => {
                    let mut args = reading_args(&reading, parent_residual);
                    args.push(("generation", generation.to_string()));
                    crate::rationale::push_note(
                        &mut report.recipe.rationale,
                        &mut report.notes,
                        crate::rationale::Note::new(crate::rationale::keys::TILE_ELIGIBLE, args),
                    );
                }
                Ok(()) => sweep[0].1.push(reading.id.tag()),
                Err(reason) => {
                    let (bucket, tag) = match sweep.iter().position(|(key, _)| *key == reason) {
                        Some(found) => (found, reading.id.tag()),
                        None => (6, format!("{}({reason})", reading.id.tag())),
                    };
                    sweep[bucket].1.push(tag);
                }
            }
        }
        let list = |ids: &[String]| {
            if ids.is_empty() { "none".to_string() } else { ids.join(" ") }
        };
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::TILE_SWEEP,
                vec![
                    ("generation", generation.to_string()),
                    ("eligible", list(&sweep[0].1)),
                    ("s", list(&sweep[1].1)),
                    ("t", list(&sweep[2].1)),
                    ("d", list(&sweep[3].1)),
                    ("ci", list(&sweep[4].1)),
                    ("parent", list(&sweep[5].1)),
                    ("other", list(&sweep[6].1)),
                ],
            ),
        );
        let Some(reading) = candidate else { break };
        let owned = match raster_home.claim_sibling("mask-zone-tile") {
            Ok(path) => path,
            Err(e) => {
                push_abstention(
                    report,
                    &reading,
                    root.residual,
                    &format!("raster-claim: {e}"),
                    generation,
                );
                refused.insert(reading.id);
                continue;
            }
        };
        let (guide, raw_mask) = tile_mask(src, reading.id);
        let (mask, refined) = if refine {
            match crate::mask_refine::guided_refine(
                &guide,
                &raw_mask,
                8,
                (4.0f32 / 255.0).powi(2),
            ) {
                crate::mask_refine::RefineOutcome::Kept { mask, reading: refined } => {
                    push_refinement_note(report, &reading.id.label(), true, refined);
                    (mask, true)
                }
                crate::mask_refine::RefineOutcome::Abstained { reading: refined } => {
                    push_refinement_note(report, &reading.id.label(), false, refined);
                    (raw_mask, false)
                }
            }
        } else {
            (raw_mask, false)
        };
        if let Err(e) = mask.save(owned.path()) {
            owned.remove();
            push_abstention(
                report,
                &reading,
                root.residual,
                &format!("raster-write: {e}"),
                generation,
            );
            refused.insert(reading.id);
            continue;
        }
        let (source_weights, target_weights) = if refined {
            let source = mask_weights(&mask, s_img.width(), s_img.height())
                .into_iter()
                .zip(&report.evidence.source_weights)
                .map(|(mask, evidence)| mask * evidence)
                .collect::<Vec<_>>();
            let target = mask_weights(&mask, t_img.width(), t_img.height())
                .into_iter()
                .zip(&report.evidence.target_weights)
                .map(|(mask, evidence)| mask * evidence)
                .collect::<Vec<_>>();
            (source, target)
        } else {
            (reading.source_weights.clone(), reading.target_weights.clone())
        };
        let attachment =
            tile_attachment(&reading, owned.path(), source_weights, target_weights);
        let frame_before = fit::look_err_with_evidence(&current, &tgt_px, &report.evidence);
        let mut frame_err = frame_before;
        let first_tile = report.recipe.masks.len();
        let accepted = attach_one_zone(
            &s_img,
            &tgt_px,
            report,
            &mut frame_err,
            &attachment,
            reading.divergence,
            corr.as_ref(),
        );
        let Some(mut accepted) = accepted else {
            owned.remove();
            push_abstention(
                report,
                &reading,
                root.residual,
                "shared-estimator",
                generation,
            );
            refused.insert(reading.id);
            continue;
        };
        let boundary = enforce_tile_boundary(
            &s_img,
            &tgt_px,
            report,
            first_tile,
            TileBoundaryInput {
                weights: &attachment.source_weights,
                initial_px: accepted.rendered,
                frame_before,
                id: reading.id,
            },
        );
        let Some((pixels, boundary)) = boundary else {
            owned.remove();
            refused.insert(reading.id);
            continue;
        };
        let target_moments = zone_moments(&tgt_px, &attachment.target_weights);
        accepted.after = zone_err(&zone_moments(&pixels, &attachment.source_weights), &target_moments);
        let frame_after = fit::look_err_with_evidence(&pixels, &tgt_px, &report.evidence);
        report.err_after = frame_after;
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::TILE_ATTACHED,
                vec![
                    ("id", reading.id.tag()),
                    ("before", format!("{:.5}", accepted.before)),
                    ("after", format!("{:.5}", accepted.after)),
                    ("frame_before", format!("{frame_before:.5}")),
                    ("frame_after", format!("{frame_after:.5}")),
                    ("boundary", format!("{:.5}", boundary.rim)),
                ],
            ),
        );
        let _path = owned.into_path();
        attached.insert(reading.id);
        generation += 1;
    }
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::TILE_DEPTH_CAP,
            vec![
                ("depth", SPATIAL_MAX_DEPTH.to_string()),
                ("cap", SPATIAL_MAX_ATTACHMENTS.to_string()),
                ("attached", attached.len().to_string()),
            ],
        ),
    );
    report.correspondence = corr;
    let final_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    report.err_after = fit::look_err_with_evidence(&final_px, &tgt_px, &report.evidence);
    fit::append_finished_disclosure(report, &final_px, &tgt_px);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    type BoundaryFixture = (
        DynamicImage,
        Vec<[f32; 3]>,
        FitReport,
        crate::store::OwnedRaster,
        Vec<f32>,
        Vec<[f32; 3]>,
    );

    fn flat_pixels(width: u32, height: u32, value: u8) -> Vec<[f32; 3]> {
        vec![[value as f32 / 255.0; 3]; (width * height) as usize]
    }

    fn localized_residual() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, fit::EvidenceModel) {
        let (width, height) = (64u32, 64u32);
        let id = TileId { depth: 2, row: 2, col: 0 };
        let mut source = flat_pixels(width, height, 120);
        let mut current = source.clone();
        let mut target = source.clone();
        for y in 0..height {
            for x in 0..width {
                if in_tile(id, x, y, width, height) {
                    let i = (y * width + x) as usize;
                    source[i] = [130.0 / 255.0; 3];
                    current[i] = [140.0 / 255.0; 3];
                    target[i] = [160.0 / 255.0; 3];
                }
            }
        }
        let mut evidence = fit::evidence_model_for(&source, &target, width, height);
        evidence.source_weights.fill(1.0);
        evidence.target_weights.fill(1.0);
        (current, target, evidence)
    }

    #[test]
    fn tile_requires_source_and_target_evidence_share() {
        let current = flat_pixels(64, 64, 100);
        let target = flat_pixels(64, 64, 120);
        let mut evidence = fit::evidence_model_for(&current, &target, 64, 64);
        evidence.source_weights.fill(1.0);
        evidence.target_weights.fill(0.0);
        let reading = read_tile(
            TileId { depth: 2, row: 0, col: 0 },
            &current,
            &target,
            &evidence,
        );
        assert_eq!(eligible(&reading, 0.0), Err("target-share"));
    }

    #[test]
    fn changed_content_with_large_residual_cannot_become_a_tile() {
        let current = flat_pixels(64, 64, 30);
        let target = flat_pixels(64, 64, 120);
        let mut source = RgbImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let value = if (x + y) % 2 == 0 { 0 } else { 255 };
                source.put_pixel(x, y, image::Rgb([value, value, value]));
            }
        }
        let original = fit::pixels_of(&DynamicImage::ImageRgb8(source));
        let mut evidence = fit::evidence_model_for(&original, &target, 64, 64);
        evidence.source_weights.fill(1.0);
        evidence.target_weights.fill(1.0);
        let reading = read_tile(
            TileId { depth: 2, row: 0, col: 0 },
            &current,
            &target,
            &evidence,
        );
        assert!(reading.divergence.d >= fit::DIVERGENCE_ZONE, "{reading:?}");
        assert_eq!(eligible(&reading, 0.0), Err("structural-divergence"));
    }

    #[test]
    fn depth_three_never_attaches() {
        assert_eq!(SPATIAL_MAX_DEPTH, 2);
        let (width, height) = (64u32, 64u32);
        let current = flat_pixels(width, height, 120);
        let mut target = current.clone();
        for y in 32..48 {
            for x in 0..32 {
                let i = (y * width + x) as usize;
                target[i] = [150.0 / 255.0; 3];
            }
        }
        for y in 48..64 {
            for x in 0..32 {
                let i = (y * width + x) as usize;
                target[i] = [90.0 / 255.0; 3];
            }
        }
        let mut evidence = fit::evidence_model_for(&current, &target, width, height);
        evidence.source_weights.fill(1.0);
        evidence.target_weights.fill(1.0);
        evidence.source_pixels = current.clone();
        let (visited, candidate) = next_tile(
            &current,
            &target,
            &evidence,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(visited.iter().any(|(reading, _, verdict)| {
            reading.id == (TileId { depth: 1, row: 1, col: 0 }) && verdict.is_err()
        }));
        let candidate = candidate.expect("a supported descendant must survive parent cancellation");
        assert_eq!(candidate.id.depth, SPATIAL_MAX_DEPTH);
        assert_eq!(candidate.id.col, 0);
        assert!(visited.iter().all(|(reading, _, _)| reading.id.depth <= 2));
    }

    #[test]
    fn tile_bitmap_is_deterministic_and_partition_conserving() {
        let source = DynamicImage::ImageRgb8(RgbImage::new(101, 67));
        let mut total = vec![0u16; 101 * 67];
        let mut first = Vec::new();
        for row in 0..4 {
            for col in 0..4 {
                let (_, a) = tile_mask(&source, TileId { depth: 2, row, col });
                let (_, b) = tile_mask(&source, TileId { depth: 2, row, col });
                assert_eq!(a.as_raw(), b.as_raw());
                if row == 2 && col == 0 {
                    first = a.as_raw().clone();
                }
                for (sum, value) in total.iter_mut().zip(a.as_raw()) {
                    *sum += *value as u16;
                }
            }
        }
        assert!(!first.is_empty());
        assert!(total.iter().all(|value| *value == 255));
    }

    #[test]
    fn tiles_fit_the_current_render_not_the_global_source() {
        let edge = fit::ANALYZE_EDGE;
        let id = TileId { depth: 2, row: 2, col: 0 };
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
            let value = 80 + ((x * 3 + y * 5) % 80) as u8;
            Rgb([value, value, value])
        }));
        let dir = std::env::temp_dir().join(format!(
            "autoshop-tile-current-render-{}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let raster_home = crate::store::OwnedRaster::scratch(dir.join("mask-semantic.png"));
        let prior_mask = GrayImage::from_fn(edge, edge, |x, y| {
            Luma([if in_tile(id, x, y, edge, edge) { 255 } else { 0 }])
        });
        prior_mask.save(raster_home.path()).unwrap();
        let mut base = crate::recipe::EditRecipe::default();
        base.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap {
                path: raster_home.path().to_string_lossy().into_owned(),
            },
            role: MaskRole::ZoneLand,
            amount: 1.0,
            exposure_ev: 0.15,
            ..Default::default()
        });
        let current_image = render::develop_preview(&source, &base);
        let mut target_image = current_image.to_rgb8();
        for y in 0..edge {
            for x in 0..edge {
                if in_tile(id, x, y, edge, edge) {
                    let p = target_image.get_pixel_mut(x, y);
                    for value in &mut p.0 {
                        *value = value.saturating_add(20);
                    }
                }
            }
        }
        let target = DynamicImage::ImageRgb8(target_image);
        let source_pixels = fit::pixels_of(&source);
        let current_pixels = fit::pixels_of(&current_image);
        let target_pixels = fit::pixels_of(&target);
        let mut evidence =
            fit::evidence_model_for(&source_pixels, &target_pixels, edge, edge);
        evidence.source_weights.fill(1.0);
        evidence.target_weights.fill(1.0);
        evidence.source_pixels = target_pixels.clone();
        let expected = read_tile(id, &current_pixels, &target_pixels, &evidence).residual;
        let stale = read_tile(id, &source_pixels, &target_pixels, &evidence).residual;
        assert!(expected.abs() > 0.02, "fixture has no current-render residual");
        assert!((expected - stale).abs() > 0.02, "fixture does not distinguish fit order");

        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe = base;
        report.evidence = evidence;
        attach_tiles(&source, &target, &mut report, &raster_home, false);
        let note = report
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::TILE_ELIGIBLE
                    && note.args.iter().any(|(key, value)| *key == "id" && value == "d2r2c0")
            })
            .unwrap_or_else(|| panic!("production traversal did not visit r2c0: {:?}", report.notes));
        let measured = note
            .args
            .iter()
            .find_map(|(key, value)| (*key == "residual").then(|| value.parse::<f32>().unwrap()))
            .unwrap();
        assert!((measured - expected).abs() < 1e-5, "current={expected} stale={stale}");
        for mask in &report.recipe.masks {
            if let MaskGeometry::Bitmap { path } = &mask.mask {
                let path = std::path::Path::new(path);
                if path.starts_with(&dir) {
                    std::fs::remove_file(path).ok();
                }
            }
        }
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn tile_attachment_cannot_regress_the_composed_frame() {
        let (current, target, evidence) = localized_residual();
        let reading = read_tile(
            TileId { depth: 2, row: 2, col: 0 },
            &current,
            &target,
            &evidence,
        );
        let attachment = tile_attachment(
            &reading,
            std::path::Path::new("mask-zone-tile.png"),
            reading.source_weights.clone(),
            reading.target_weights.clone(),
        );
        assert_eq!(SPATIAL_FRAME_REGRESSION_TOL.to_bits(), 0.0f32.to_bits());
        assert_eq!(attachment.frame_regression_tol.to_bits(), 0.0f32.to_bits());
    }

    fn boundary_fixture(
        exposure_ev: f32,
        target_is_candidate: bool,
        name: &str,
    ) -> BoundaryFixture {
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, y| {
            let base = 80 + ((x * 3 + y * 5) % 80) as u8;
            Rgb([base, base, base])
        }));
        let mask = GrayImage::from_fn(64, 64, |x, y| {
            Luma([if in_tile(TileId { depth: 2, row: 2, col: 0 }, x, y, 64, 64) {
                255
            } else {
                0
            }])
        });
        let path = super::super::tests::fixture_mask_path(name);
        mask.save(path.path()).unwrap();
        let adjustment = LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            name: "Spatial tile r2c0".to_string(),
            role: MaskRole::Custom,
            amount: 1.0,
            exposure_ev,
            ..Default::default()
        };
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(adjustment);
        let candidate = fit::pixels_of(&render::develop_preview(&source, &recipe));
        let target = if target_is_candidate {
            render::develop_preview(&source, &recipe)
        } else {
            source.clone()
        };
        let target_pixels = fit::pixels_of(&target);
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe = recipe;
        let weights = mask_weights(&mask, 64, 64);
        (source, target_pixels, report, path, weights, candidate)
    }

    #[test]
    fn tile_boundary_shrink_preserves_direction_and_budget() {
        let mut original = LocalAdjustment {
            exposure_ev: -0.4,
            contrast: 30.0,
            saturation: -20.0,
            color_gains: Some([1.3, 0.8, 1.1]),
            ..Default::default()
        };
        let mut shrunk = original.clone();
        shrink_zone_corrections(
            std::slice::from_mut(&mut shrunk),
            std::slice::from_ref(&original),
            &[1.0],
            0.25,
        );
        assert!(shrunk.exposure_ev < 0.0 && shrunk.exposure_ev.abs() < original.exposure_ev.abs());
        assert!(shrunk.contrast > 0.0 && shrunk.contrast < original.contrast);
        assert!(shrunk.saturation < 0.0 && shrunk.saturation.abs() < original.saturation.abs());
        for (after, before) in shrunk.color_gains.unwrap().into_iter().zip(original.color_gains.take().unwrap()) {
            assert!((after - 1.0).signum() == (before - 1.0).signum());
            assert!((after - 1.0).abs() <= (before - 1.0).abs());
        }

        let (source, target, mut report, path, weights, candidate) =
            boundary_fixture(0.01, true, "tile-boundary-budget");
        let result = enforce_tile_boundary(
            &source,
            &target,
            &mut report,
            0,
            TileBoundaryInput {
                weights: &weights,
                initial_px: candidate,
                frame_before: 0.0,
                id: TileId { depth: 2, row: 2, col: 0 },
            },
        );
        let (_, reading) = result.expect("a matching, low-rim tile must pass");
        assert!(reading.rim <= ZONE_BOUNDARY_RIM_MAX, "{reading:?}");
        path.remove();
    }

    #[test]
    fn refined_mask_is_rechecked_by_rim_and_frame_gates() {
        let (source, target, mut report, path, weights, candidate) =
            boundary_fixture(0.25, false, "tile-refined-recheck");
        let result = enforce_tile_boundary(
            &source,
            &target,
            &mut report,
            0,
            TileBoundaryInput {
                weights: &weights,
                initial_px: candidate,
                frame_before: 0.0,
                id: TileId { depth: 2, row: 2, col: 0 },
            },
        );
        assert!(result.is_none(), "a refined alpha cannot bypass composed-frame arbitration");
        assert!(report.recipe.masks.is_empty(), "the refused correction must be removed");
        path.remove();
    }

    #[test]
    fn bitmap_tile_xmp_loss_is_named_and_recipe_round_trip_is_lossless() {
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "mask-zone-tile.png".to_string() },
            name: "Spatial tile r2c0".to_string(),
            role: MaskRole::Custom,
            amount: 1.0,
            exposure_ev: -0.2,
            ..Default::default()
        });
        let bytes = serde_json::to_vec(&recipe).unwrap();
        let decoded: crate::recipe::EditRecipe = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), bytes);
        let (_, losses) = crate::xmp::recipe_to_xmp_with_losses(&recipe);
        assert_eq!(losses.len(), 1, "one bitmap tile has one export loss");
        assert_eq!(losses[0].name, "Spatial tile r2c0");
        assert_eq!(losses[0].reason, crate::xmp::MaskLossReason::Bitmap);
    }

    #[test]
    fn calibration_strong_r2c0_survives_derivation_and_changed_sky_does_not() {
        let Some(root) = fit::calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).unwrap();
        let target = image::open(root.join("target.jpg")).unwrap();
        let recipe: crate::recipe::EditRecipe =
            serde_json::from_slice(&std::fs::read(root.join("fitted.recipe.json")).unwrap())
                .unwrap();
        let s_img = source.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let t_img = target.thumbnail(fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let original = fit::pixels_of(&s_img);
        let target_pixels = fit::pixels_of(&t_img);
        let evidence = fit::evidence_model_for(
            &original,
            &target_pixels,
            s_img.width(),
            s_img.height(),
        );
        let current = fit::pixels_of(&render::develop_preview(&s_img, &recipe));
        let strong_id = TileId { depth: 2, row: 2, col: 0 };
        let strong = read_tile(strong_id, &current, &target_pixels, &evidence);
        let parent = read_tile(
            TileId { depth: 1, row: 1, col: 0 },
            &current,
            &target_pixels,
            &evidence,
        );
        assert_eq!(eligible(&strong, parent.residual), Ok(()), "{strong:?}");
        for col in 0..4 {
            let sky = read_tile(
                TileId { depth: 2, row: 0, col },
                &current,
                &target_pixels,
                &evidence,
            );
            let sky_parent = read_tile(
                TileId { depth: 1, row: 0, col: col / 2 },
                &current,
                &target_pixels,
                &evidence,
            );
            assert!(eligible(&sky, sky_parent.residual).is_err(), "changed sky became a tile: {sky:?}");
        }
    }
}
