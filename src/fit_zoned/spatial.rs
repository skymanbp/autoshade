use std::collections::BTreeSet;

use image::{DynamicImage, GrayImage, Luma};

use super::*;

pub(super) const SPATIAL_MAX_DEPTH: u8 = 2;
pub(super) const SPATIAL_MAX_ATTACHMENTS: usize = 4;
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
    let mut geometry = vec![0.0f32; n];
    for y in 0..evidence.height {
        for x in 0..evidence.width {
            let i = (y * evidence.width + x) as usize;
            if i < n && in_tile(id, x, y, evidence.width, evidence.height) {
                geometry[i] = 1.0;
            }
        }
    }
    // The tile is judged by ITS population: the frame's per-bin verdicts are
    // re-aggregated over the tile's own members, so a mid-tone ground tile
    // keeps the evidence that a replaced sky's identical luma bins withheld
    // frame-wide.
    let scoped = evidence.scoped(target, &geometry, &geometry);
    let mut source_weights = scoped.source_weights;
    let mut target_weights = scoped.target_weights;
    source_weights.resize(n, 0.0);
    target_weights.resize(n, 0.0);
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
    coverage: ZoneCoverage,
) -> ZoneAttachment {
    ZoneAttachment {
        source_weights,
        target_weights,
        coverage: Some(coverage),
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
    cap: usize,
) {
    // One analysis geometry for both rasters (`fit::analysis_pair`), so the
    // coverage and estimator vectors below are congruent by construction —
    // the two asserts pin that contract.
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let mut attached = BTreeSet::new();
    let mut refused = BTreeSet::new();
    let mut generation = 0usize;
    let corr = report.correspondence.take();
    while attached.len() < cap {
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
        // The raster is what the correction moves; the estimator weights are
        // that raster times the tile's own evidence reading.
        let coverage = ZoneCoverage {
            source: mask_weights(&mask, s_img.width(), s_img.height()),
            target: mask_weights(&mask, t_img.width(), t_img.height()),
        };
        assert_eq!(coverage.source.len(), reading.source_weights.len());
        assert_eq!(coverage.target.len(), reading.target_weights.len());
        let (source_weights, target_weights) = if refined {
            let source = coverage
                .source
                .iter()
                .zip(&reading.source_weights)
                .map(|(mask, evidence)| mask * evidence)
                .collect::<Vec<_>>();
            let target = coverage
                .target
                .iter()
                .zip(&reading.target_weights)
                .map(|(mask, evidence)| mask * evidence)
                .collect::<Vec<_>>();
            (source, target)
        } else {
            (reading.source_weights.clone(), reading.target_weights.clone())
        };
        let attachment =
            tile_attachment(&reading, owned.path(), source_weights, target_weights, coverage);
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
                ("cap", cap.to_string()),
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

    /// Pretend every pixel is structurally supported. `read_tile` re-aggregates
    /// the range verdicts over the tile's own members from the model's
    /// per-pixel ingredients, so a fixture injects support THERE; the frame's
    /// per-pixel weight vectors are filled too for anything still reading them.
    fn pretend_full_support(evidence: &mut fit::EvidenceModel) {
        evidence.spatial_weights.fill(1.0);
        evidence.spatial_divergence.fill(0.0);
        evidence.spatial_supported.fill(true);
        evidence.globally_same_content = true;
        evidence.source_weights.fill(1.0);
        evidence.target_weights.fill(1.0);
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
        pretend_full_support(&mut evidence);
        (current, target, evidence)
    }

    /// Both share gates own a falsifier. The first case removes support from
    /// both sides and reaches `source-share`. The second swaps bright/dark
    /// halves: rank pairing places the bright target members on the right,
    /// while support exists only on the left, so the bright tile reading
    /// reaches `target-share`.
    #[test]
    fn tile_requires_source_and_target_evidence_share() {
        let current = flat_pixels(64, 64, 100);
        let target = flat_pixels(64, 64, 120);
        let id = TileId { depth: 2, row: 0, col: 0 };
        let mut evidence = fit::evidence_model_for(&current, &target, 64, 64);
        pretend_full_support(&mut evidence);
        let supported = read_tile(id, &current, &target, &evidence);
        assert!(supported.source_share >= MIN_ZONE_SHARE, "{supported:?}");
        assert!(
            (supported.source_share - supported.target_share).abs() < 1e-4,
            "a tile's two shares are one population: {supported:?}"
        );
        evidence.spatial_weights.fill(0.0);
        let unsupported = read_tile(id, &current, &target, &evidence);
        assert_eq!(unsupported.source_share, 0.0, "{unsupported:?}");
        assert_eq!(unsupported.target_share, 0.0, "{unsupported:?}");
        assert_eq!(eligible(&unsupported, 0.0), Err("source-share"));

        let width = 64u32;
        let height = 64u32;
        let source = (0..width * height)
            .map(|i| if i % width < width / 2 { [0.8; 3] } else { [0.2; 3] })
            .collect::<Vec<_>>();
        let target = (0..width * height)
            .map(|i| if i % width < width / 2 { [0.2; 3] } else { [0.8; 3] })
            .collect::<Vec<_>>();
        let mut evidence = fit::evidence_model_for(&source, &target, width, height);
        evidence.spatial_weights.iter_mut().enumerate().for_each(|(i, weight)| {
            *weight = if i % (width as usize) < width as usize / 2 { 1.0 } else { 0.0 };
        });
        evidence.spatial_divergence.fill(0.0);
        evidence.spatial_supported.fill(true);
        evidence.globally_same_content = true;
        let frame = vec![1.0; source.len()];
        let scoped = evidence.scoped(&target, &frame, &frame);
        let bright = &scoped.luma[fit::evidence_luma_bin(0.8)];
        assert!(bright.source_evidence_share >= MIN_ZONE_SHARE, "{bright:?}");
        assert!(bright.target_evidence_share < MIN_ZONE_SHARE, "{bright:?}");
        let target_missing = TileReading {
            id: TileId { depth: 0, row: 0, col: 0 },
            source_weights: Vec::new(),
            target_weights: Vec::new(),
            source_share: bright.source_evidence_share,
            target_share: bright.target_evidence_share,
            residual: 0.1,
            ci95: 0.0,
            divergence: fit::Divergence { correlation: 1.0, energy_error: 0.0, d: 0.0 },
        };
        assert!(target_missing.source_share >= MIN_ZONE_SHARE, "{target_missing:?}");
        assert!(target_missing.target_share < MIN_ZONE_SHARE, "{target_missing:?}");
        assert_eq!(eligible(&target_missing, 0.0), Err("target-share"));
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
        pretend_full_support(&mut evidence);
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
        pretend_full_support(&mut evidence);
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

    /// One rendered tile (d2r2c0) is 20/255 brighter in the target than in the
    /// current render: a pair with exactly one attachable tile, shared by the
    /// fit-order test and the cap test so neither fixture can go vacuous alone.
    struct CurrentRenderFixture {
        source: DynamicImage,
        target: DynamicImage,
        evidence: fit::EvidenceModel,
        base: crate::recipe::EditRecipe,
        raster_home: crate::store::OwnedRaster,
        dir: std::path::PathBuf,
        expected: f32,
        stale: f32,
    }

    fn current_render_fixture(tag: &str) -> CurrentRenderFixture {
        let edge = fit::ANALYZE_EDGE;
        let id = TileId { depth: 2, row: 2, col: 0 };
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
            let value = 80 + ((x * 3 + y * 5) % 80) as u8;
            Rgb([value, value, value])
        }));
        let dir = std::env::temp_dir().join(format!(
            "autoshop-tile-{tag}-{}",
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
        pretend_full_support(&mut evidence);
        evidence.source_pixels = target_pixels.clone();
        let expected = read_tile(id, &current_pixels, &target_pixels, &evidence).residual;
        let stale = read_tile(id, &source_pixels, &target_pixels, &evidence).residual;
        assert!(expected.abs() > 0.02, "fixture has no current-render residual");
        assert!((expected - stale).abs() > 0.02, "fixture does not distinguish fit order");
        CurrentRenderFixture { source, target, evidence, base, raster_home, dir, expected, stale }
    }

    impl CurrentRenderFixture {
        /// Runs the production traversal under `cap` on a fresh report and
        /// returns it with the number of masks the traversal attached.
        fn attach(&self, cap: usize) -> (FitReport, usize) {
            let mut report = super::super::tests::neutral_report(&self.source, &self.target);
            report.recipe = self.base.clone();
            report.evidence = self.evidence.clone();
            let masks_before = report.recipe.masks.len();
            attach_tiles(&self.source, &self.target, &mut report, &self.raster_home, false, cap);
            let added = report.recipe.masks.len() - masks_before;
            (report, added)
        }
    }

    #[test]
    fn tiles_fit_the_current_render_not_the_global_source() {
        let fixture = current_render_fixture("current-render");
        let (expected, stale) = (fixture.expected, fixture.stale);
        let (report, added) = fixture.attach(2);
        assert!(added <= 2, "the effective two-tile cap was exceeded");
        let cap_note = report.notes.iter()
            .find(|note| note.key == crate::rationale::keys::TILE_DEPTH_CAP).unwrap();
        assert!(cap_note.args.iter().any(|(key, value)| *key == "cap" && value == "2"));
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
        std::fs::remove_dir_all(&fixture.dir).ok();
    }

    /// Behavioural, not textual: the same pair that attaches its one tile under
    /// the shipped cap attaches nothing under a cap of zero, and the depth-cap
    /// note names the cap it was actually given. The `>= 1` guard keeps the
    /// falsifier honest — a fixture that attached nothing could not tell a
    /// respected cap from an ignored one.
    #[test]
    fn tile_attachment_cap_is_parameterized() {
        let fixture = current_render_fixture("cap");
        let cap_of = |report: &FitReport| report.notes.iter()
            .find(|note| note.key == crate::rationale::keys::TILE_DEPTH_CAP)
            .and_then(|note| note.args.iter()
                .find_map(|(key, value)| (*key == "cap").then(|| value.clone())))
            .unwrap();
        let (zero, added_zero) = fixture.attach(0);
        assert_eq!(added_zero, 0, "a zero cap must attach nothing: {}", zero.recipe.rationale);
        assert_eq!(cap_of(&zero), "0");
        let (shipped, added_shipped) = fixture.attach(SPATIAL_MAX_ATTACHMENTS);
        assert!(added_shipped >= 1,
            "fixture attached nothing, so the cap is untestable: {}", shipped.recipe.rationale);
        assert!(added_shipped <= SPATIAL_MAX_ATTACHMENTS);
        assert_eq!(cap_of(&shipped), SPATIAL_MAX_ATTACHMENTS.to_string());
        std::fs::remove_dir_all(&fixture.dir).ok();
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
            ZoneCoverage {
                source: tile_geometry(reading.id, 64, 64),
                target: tile_geometry(reading.id, 64, 64),
            },
        );
        assert_eq!(SPATIAL_FRAME_REGRESSION_TOL.to_bits(), 0.0f32.to_bits());
        assert_eq!(attachment.frame_regression_tol.to_bits(), 0.0f32.to_bits());
    }

    fn tile_geometry(id: TileId, width: u32, height: u32) -> Vec<f32> {
        (0..width * height)
            .map(|i| if in_tile(id, i % width, i / width, width, height) { 1.0 } else { 0.0 })
            .collect()
    }

    /// The vetoes are asked over the raster a tile MOVES, not over its
    /// evidence-weighted estimator weights: a tile whose upper half is
    /// replaced content (withheld over the tile's own view) and whose lower
    /// half asks for +0.12 EV would move the withheld half too, and must be
    /// refused with the tone-withheld note. Scoping the veto over the
    /// estimator weights would drop those pixels from the population and let
    /// the tile through.
    #[test]
    fn a_tile_is_vetoed_over_the_raster_it_moves_not_its_estimator_weights() {
        let edge = fit::ANALYZE_EDGE;
        // A same-content checker texture on both sides keeps the local-quality
        // texture ratio finite; the halves stay inside luma bins 5 and 10.
        let build = |lower: f32| -> DynamicImage {
            DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
                let base = if y < 48 { 0.30f32 } else { lower };
                let v = base + 0.02 * ((x + y) % 2) as f32;
                Rgb([(v * 255.0).round() as u8; 3])
            }))
        };
        let source = build(0.60);
        let target = build(0.65);
        let id = TileId { depth: 2, row: 0, col: 0 };
        let source_px = fit::pixels_of(&source);
        let target_px = fit::pixels_of(&target);
        let mut evidence = fit::evidence_model_for(&source_px, &target_px, edge, edge);
        pretend_full_support(&mut evidence);
        for y in 0..48u32 {
            for x in 0..edge {
                evidence.spatial_weights[(y * edge + x) as usize] = 0.0;
            }
        }
        let reading = read_tile(id, &source_px, &target_px, &evidence);
        assert!(reading.source_share >= MIN_ZONE_SHARE, "{reading:?}");
        let upper = (10 * edge + 10) as usize;
        let lower = (70 * edge + 10) as usize;
        assert_eq!(reading.source_weights[upper], 0.0, "the replaced half carries no weight");
        assert!(reading.source_weights[lower] > 0.0, "the supported half carries the fit");

        let (_, raw_mask) = tile_mask(&source, id);
        let path = super::super::tests::fixture_mask_path("tile-coverage-veto");
        raw_mask.save(path.path()).unwrap();
        let coverage = ZoneCoverage {
            source: mask_weights(&raw_mask, edge, edge),
            target: mask_weights(&raw_mask, edge, edge),
        };
        let attachment = tile_attachment(
            &reading,
            path.path(),
            reading.source_weights.clone(),
            reading.target_weights.clone(),
            coverage,
        );
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.evidence = evidence;
        let mut frame_err = report.err_after;
        let accepted = attach_one_zone(
            &source,
            &target_px,
            &mut report,
            &mut frame_err,
            &attachment,
            fit::Divergence { correlation: 1.0, energy_error: 0.0, d: 0.0 },
            None,
        );
        path.remove();
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE),
            "moving the withheld half must withhold the tone controls: {}",
            report.recipe.rationale
        );
        assert!(
            accepted.is_none() && report.recipe.masks.is_empty(),
            "a tile that would move withheld pixels must not attach: {}",
            report.recipe.rationale
        );
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

    /// The frame withholds luma bin 6 because a replaced sky dominates it; a
    /// pure-ground tile that owns a few of that bin's supported pixels must
    /// count them -- its reading is taken over its own population.
    #[test]
    fn a_tile_reading_keeps_the_mid_tones_the_frame_withheld() {
        let (w, h) = (fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
        let build = |target: bool| {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
                let v: f32 = if y < h * 2 / 3 {
                    if target {
                        if (y / 8) % 2 == 0 { 0.05 } else { 0.15 }
                    } else {
                        0.36 + 0.04 * x as f32 / (w - 1) as f32
                    }
                } else {
                    let ground = if ((x / 32) + (y / 32)) % 4 == 0 { 0.40 } else { 0.20 };
                    if target { ground + 0.08 } else { ground }
                };
                image::Rgb([(v.clamp(0.0, 1.0) * 255.0).round() as u8; 3])
            }))
        };
        let sp = fit::pixels_of(&build(false));
        let tp = fit::pixels_of(&build(true));
        let evidence = fit::evidence_model_for(&sp, &tp, w, h);
        assert!(
            evidence.luma[6].weight <= 0.0,
            "premise: bin 6 is withheld frame-wide: {:?}",
            evidence.luma[6]
        );
        let id = TileId { depth: 2, row: 3, col: 2 };
        let reading = read_tile(id, &sp, &tp, &evidence);
        // A 0.40 ground pixel inside r3c2: the frame gives it no weight, the
        // tile's own population keeps it.
        let probe = (288 * w + 224) as usize;
        assert!(in_tile(id, 224, 288, w, h));
        assert_eq!(fit::evidence_luma_bin(fit::luma601(&sp[probe])), 6);
        assert_eq!(evidence.source_weights[probe], 0.0);
        assert!(reading.source_weights[probe] > 0.0, "{}", reading.source_weights[probe]);
        let frame_share = evidence
            .source_weights
            .iter()
            .enumerate()
            .filter(|(i, _)| in_tile(id, *i as u32 % w, *i as u32 / w, w, h))
            .map(|(_, weight)| *weight)
            .sum::<f32>()
            / sp.len() as f32;
        assert!(
            reading.source_share > frame_share,
            "{} must exceed the frame-masked share {frame_share}",
            reading.source_share
        );
    }

    #[test]
    fn calibration_strong_r2c0_survives_derivation_and_changed_sky_does_not() {
        let Some(root) = fit::calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).unwrap();
        let target = image::open(root.join("target.jpg")).unwrap();
        let recipe: crate::recipe::EditRecipe =
            serde_json::from_slice(&std::fs::read(root.join("fitted.recipe.json")).unwrap())
                .unwrap();
        let (s_img, t_img) = fit::analysis_pair(&source, &target);
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
