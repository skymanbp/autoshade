use std::collections::BTreeSet;

use image::{DynamicImage, GrayImage, Luma};

use super::*;
use crate::rationale::values;

pub(super) const SPATIAL_MAX_DEPTH: u8 = 2;
pub(super) const SPATIAL_MAX_ATTACHMENTS: usize = 4;
pub(super) const SPATIAL_RESIDUAL_MIN: f32 = 2.0 / 255.0;
pub(super) const SPATIAL_FRAME_REGRESSION_TOL: f32 = 0.0;
pub(super) const TILE_RASTER_EDGE: u32 = 2048;
/// Half a pixel of the tile analysis raster, in normalised frame units.
/// The Zero/Full ramp fits between adjacent pixel centres, preserving the
/// integer cell exactly. Unrelated to the renderer's guided-filter tile size.
const TILE_GRADIENT_RAMP: f32 = 0.5 / TILE_RASTER_EDGE as f32;

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
    /// The tile's own footprint on the analysis grid, in pixels — the quantity
    /// [`MIN_MASK_PIXELS`] gates, and not the same thing as the evidence-scoped
    /// `source_share` beside it (a share is a weighted fraction of the frame;
    /// this is a count of the pixels the raster actually covers).
    pub(super) pixels: usize,
    pub(super) residual: f32,
    pub(super) ci95: f32,
    /// `None` is [`fit::structure_divergence`]'s abstention. It is NOT a matched
    /// reading, so [`eligible`] refuses the tile rather than letting a footprint
    /// nothing measured clear a gate that exists to require survival.
    pub(super) divergence: Option<fit::Divergence>,
}

impl TileReading {
    fn score(&self) -> f32 {
        self.residual.abs() * self.source_share.min(self.target_share)
    }
}

pub(super) struct ScopedMaskEvidence {
    pub(super) source_weights: Vec<f32>,
    pub(super) target_weights: Vec<f32>,
    pub(super) source_share: f32,
    pub(super) target_share: f32,
}

/// Re-aggregate the frozen frame evidence over one analysis-grid mask. Tiles
/// and free-form components share this exact population ruler.
pub(super) fn scoped_mask_evidence(
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    geometry: &[f32],
) -> ScopedMaskEvidence {
    let n = (evidence.width as usize * evidence.height as usize)
        .min(target.len())
        .min(evidence.source_weights.len())
        .min(evidence.target_weights.len())
        .min(geometry.len());
    let scoped = evidence.scoped(target, &geometry[..n], &geometry[..n]);
    let mut source_weights = scoped.source_weights;
    let mut target_weights = scoped.target_weights;
    source_weights.resize(n, 0.0);
    target_weights.resize(n, 0.0);
    let source_share = source_weights.iter().map(|v| *v as f64).sum::<f64>() as f32
        / n.max(1) as f32;
    let target_share = target_weights.iter().map(|v| *v as f64).sum::<f64>() as f32
        / n.max(1) as f32;
    ScopedMaskEvidence { source_weights, target_weights, source_share, target_share }
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

/// Everything a tile reading holds that does NOT depend on the current render:
/// its geometry, the frame evidence re-aggregated over that geometry, its
/// footprint and its structural reading.  The traversal re-reads the same
/// node once per generation — a four-tile attachment sweeps the same 20 nodes
/// four times — and only the residual moves between those passes, because the
/// evidence model is frozen before the first producer runs and the structural
/// reading is taken against `evidence.source_pixels`, not against the render.
/// Keyed by [`TileId`], so the recompute happens once per node per fit.
struct TileEvidence {
    scoped: ScopedMaskEvidence,
    pixels: usize,
    divergence: Option<fit::Divergence>,
}

type TileEvidenceCache = std::collections::BTreeMap<TileId, std::rc::Rc<TileEvidence>>;

// Reads and recomputes of `tile_evidence`, for the test that pins what the
// cache is worth. Test-only: production keeps no counters, and the numbers are
// only meaningful inside one traversal anyway.
#[cfg(test)]
thread_local! {
    static TILE_EVIDENCE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TILE_EVIDENCE_COMPUTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn tile_evidence(
    id: TileId,
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    cache: &mut TileEvidenceCache,
) -> std::rc::Rc<TileEvidence> {
    #[cfg(test)]
    TILE_EVIDENCE_READS.with(|reads| reads.set(reads.get() + 1));
    if let Some(hit) = cache.get(&id) {
        return std::rc::Rc::clone(hit);
    }
    #[cfg(test)]
    TILE_EVIDENCE_COMPUTES.with(|computes| computes.set(computes.get() + 1));
    let n = (evidence.width as usize * evidence.height as usize)
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
    let scoped = scoped_mask_evidence(target, evidence, &geometry);
    let divergence = fit::structure_divergence(
        &evidence.source_pixels[..n.min(evidence.source_pixels.len())],
        &target[..n],
        evidence.width,
        evidence.height,
        &geometry,
    );
    let entry = std::rc::Rc::new(TileEvidence {
        pixels: geometry.iter().filter(|value| **value > 0.0).count(),
        scoped,
        divergence,
    });
    cache.insert(id, std::rc::Rc::clone(&entry));
    entry
}

fn read_tile(
    id: TileId,
    current: &[[f32; 3]],
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    cache: &mut TileEvidenceCache,
) -> TileReading {
    let held = tile_evidence(id, target, evidence, cache);
    let (residual, ci95) = weighted_residual(current, target, &held.scoped.source_weights);
    TileReading {
        id,
        source_weights: held.scoped.source_weights.clone(),
        target_weights: held.scoped.target_weights.clone(),
        // ONE share ruler: this reading is the same number the attachment gate
        // now applies (`fit_zoned::attach_one_zone`), so a tile admitted here
        // can no longer be refused there for a size it was never measured at.
        source_share: held.scoped.source_share,
        target_share: held.scoped.target_share,
        pixels: held.pixels,
        residual,
        ci95,
        divergence: held.divergence,
    }
}

fn eligible(reading: &TileReading, parent_residual: f32) -> Result<(), &'static str> {
    // The footprint floor comes FIRST because it is the cheapest true statement
    // about the candidate, and because the gates under it read statistics taken
    // over that footprint: a share and a structural correlation measured over a
    // handful of pixels are not more informative for having been computed.
    if reading.pixels < MIN_MASK_PIXELS {
        Err("footprint")
    } else if reading.source_share < MIN_ZONE_SHARE {
        Err("source-share")
    } else if reading.target_share < MIN_ZONE_SHARE {
        Err("target-share")
    } else if reading.divergence.is_none() {
        // No reading, so no claim that the structure survived — and survival is
        // exactly what the next arm requires. Until v1.2.4 the instrument
        // answered an unmeasurable footprint with D = 0 and this gate passed it.
        Err("structure-unmeasured")
    } else if reading.divergence.is_some_and(|d| d.d >= fit::DIVERGENCE_ZONE) {
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
        // An unread footprint says so; a 0.000 here would read as a measured
        // perfect structural match.
        ("d", reading.divergence.map_or_else(
            || values::UNMEASURED.to_string(), |d| format!("{:.3}", d.d))),
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

pub(super) fn push_refinement_note(
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
    cache: &mut TileEvidenceCache,
) -> TileSearch {
    let root = read_tile(TileId { depth: 0, row: 0, col: 0 }, current, target, evidence, cache);
    let mut pending = Vec::new();
    for row in 0..2 {
        for col in 0..2 {
            pending.push(PendingTile {
                reading: read_tile(
                    TileId { depth: 1, row, col },
                    current,
                    target,
                    evidence,
                    cache,
                ),
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
                        cache,
                    ),
                    parent_residual: node.reading.residual,
                });
            }
        }
    }
    (visited, None)
}

fn tile_geometry(id: TileId, size: (u32, u32)) -> (MaskGeometry, Vec<MaskComponent>) {
    // Integer cell arithmetic places a break at ceil(k * size / grid), not
    // k / grid when the raster has an odd dimension. Keep that exact edge.
    let edge = |k: u32, n: u32| (k * n).div_ceil(id.grid()) as f32 / n.max(1) as f32;
    let (left, right) = (edge(id.col as u32, size.0), edge(id.col as u32 + 1, size.0));
    let (top, bottom) = (edge(id.row as u32, size.1), edge(id.row as u32 + 1, size.1));
    let r = TILE_GRADIENT_RAMP * 0.5;
    let shapes = [
        MaskGeometry::Linear { zero_x: left - r, zero_y: 0.5, full_x: left + r, full_y: 0.5 },
        MaskGeometry::Linear { zero_x: right + r, zero_y: 0.5, full_x: right - r, full_y: 0.5 },
        MaskGeometry::Linear { zero_x: 0.5, zero_y: top - r, full_x: 0.5, full_y: top + r },
        MaskGeometry::Linear { zero_x: 0.5, zero_y: bottom + r, full_x: 0.5, full_y: bottom - r },
    ];
    let mut shapes = shapes.into_iter();
    let base = shapes.next().expect("four half planes");
    let components = shapes.map(|geometry| MaskComponent { inverted: false, geometry, mode: MaskCombine::Intersect }).collect();
    (base, components)
}

fn tile_attachment(
    reading: &TileReading,
    size: (u32, u32),
    source_weights: Vec<f32>,
    target_weights: Vec<f32>,
    coverage: ZoneCoverage,
) -> ZoneAttachment {
    let (mask, components) = tile_geometry(reading.id, size);
    ZoneAttachment {
        source_weights,
        target_weights,
        coverage: Some(coverage),
        mask,
        components,
        range: None,
        name: reading.id.label(),
        role: MaskRole::Custom,
        inverted: false,
        label: reading.id.label(),
        min_share: MIN_ZONE_SHARE,
        frame_regression_tol: SPATIAL_FRAME_REGRESSION_TOL,
    }
}

/// R36. Which carrier ships once a refined raster and its native
/// four-gradient twin have BOTH passed the estimator and the boundary gate:
/// the native one, unless it fits the photo worse — on the hard cell's own
/// population or on the frame — by more than the tie band. R35 asked instead
/// whether the two RENDERS resembled each other within the boundary budget,
/// which kept three of the reference pair's four tiles as rasters whose edge
/// alpha the guide had moved without ever asking which edge fits the photo
/// better. The raster is an intermediate, not the truth; equal fidelity is a
/// tie, and a tie goes to the carrier Lightroom can read.
fn native_carrier_fits(raster_zone: f32, native_zone: f32, raster_frame: f32, native_frame: f32) -> bool {
    native_zone <= raster_zone + CARRIER_TIE && native_frame <= raster_frame + CARRIER_TIE
}
const CARRIER_TIE: f32 = 1e-6;

/// Alpha error bounds the compositing error for channel values in [0,1].
/// Compare every pixel, not only coverage and the frozen cores: equal mass
/// could otherwise hide an edge moved from one place to another.
fn refinement_alpha_delta(hard: &GrayImage, refined: &GrayImage) -> f32 {
    if hard.dimensions() != refined.dimensions() { return f32::INFINITY; }
    hard.as_raw().iter().zip(refined.as_raw())
        .map(|(a, b)| (*a as f32 - *b as f32).abs() / 255.0)
        .fold(0.0, f32::max)
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
        ("before", format!("{:.4}", before.rim)),
        ("after", format!("{:.4}", after.rim)),
        ("asked", format!("{:.4}", after.asked)),
        ("charged", format!("{:.4}", after.charged)),
        ("colour", format!("{:.4}", after.colour)),
        ("colour_charged", format!("{:.4}", after.colour_charged)),
        ("max", format!("{ZONE_BOUNDARY_STEP_MAX:.3}")),
        ("transitions", after.transitions.to_string()),
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BitmapBoundaryWhy {
    Frame,
    Rim,
    /// The mask reaches the gate with a contour that could not be sampled at
    /// all. An unmeasurable boundary is a REFUSAL, never a pass: reading
    /// `0.000` off `0` transitions is exactly what let every hard-edged tile
    /// through the gate that was supposed to be holding the seam budget.
    Unmeasured,
    /// The correction that survived the gate does not move a single pixel of
    /// the analysis render: the accepted render is byte-identical to the
    /// frame WITHOUT it. Step 9 made this reachable — under the transported
    /// differential a `k=0` render reads exactly 0.0 by construction, so the
    /// budget can never refuse it and the bisection returns the largest `k`
    /// that passes, which for a correction whose every visible strength
    /// introduces a seam is a `k` that renders to nothing.
    ///
    /// The test is BYTE IDENTITY of the render, deliberately not a threshold
    /// on `k`. `k == 0.0` alone would catch almost nothing: the reading falls
    /// continuously to zero with `k`, and the 8-bit analysis render quantises
    /// a small enough `k` into a literal no-op, so the bisection almost
    /// always lands on a tiny POSITIVE `k` rather than on zero. And a
    /// threshold ON `k` is not a threshold on visibility at all — `k` scales
    /// whatever dials the zone happens to carry, so the same `k` moves
    /// different numbers of pixels for different corrections. Comparing the
    /// two renders asks the question directly and needs no constant.
    ///
    /// An inert attachment is strictly worse than a refusal: it occupies the
    /// exclusion budget the next candidate needs, consumes the attachment
    /// cap, keeps a raster on disk, and discloses a before/after pair it did
    /// not produce.
    Inert,
}

pub(super) struct BitmapBoundaryAccepted {
    pub(super) pixels: Vec<[f32; 3]>,
    pub(super) reading: BoundaryReading,
    pub(super) initial: BoundaryReading,
    pub(super) k: f32,
}

#[derive(Debug)]
pub(super) struct BitmapBoundaryRefusal {
    pub(super) why: BitmapBoundaryWhy,
    pub(super) initial: BoundaryReading,
}

/// Which ruler measures this mask family's boundary. These are two readings
/// of two different shapes, not two estimates of one number, so the caller
/// states which shape it is handing over rather than letting one ruler
/// silently return 0.0 on the other's geometry.
pub(super) enum BoundaryRuler<'a> {
    /// Soft, feathered masks — semantic segmentation rasters. The signed
    /// overshoot INSIDE the transition band, measured against the settled
    /// interiors on the same scan line ([`boundary_rim`]). `weights` is the
    /// exact vector this ruler has always been handed: on the semantic-region
    /// path that is the segmentation raster's OWN alpha at analysis size
    /// (`mask_weights` of `region.source`), so it is already a contour and
    /// not an evidence-scoped product.
    TransitionBand {
        weights: &'a [f32],
        /// The same frame rendered WITHOUT this correction, for exactly the
        /// reason [`BoundaryRuler::CrossBoundaryStep`] carries one: the rim
        /// this ruler now reports is the rim the correction INTRODUCED, so a
        /// bow the scene already had under the feather must cancel instead of
        /// being charged. Carried in the ENUM rather than as a loose
        /// argument, so a caller cannot hand over a stale reference by
        /// omitting it.
        reference: &'a [[f32; 3]],
    },
    /// Hard 0/255 rasters — spatial tiles and free masks, which have no
    /// transition band for the rim ruler to read and therefore always scored
    /// `rim 0.000 / 0 transitions` and passed. The correction's own induced
    /// step ACROSS the 50% contour ([`boundary_step`]).
    CrossBoundaryStep {
        /// The mask's own alpha at analysis size — the contour the renderer
        /// applies. NOT the estimator weights: those carry the zone's per-bin
        /// evidence verdicts, so their 50% contour is punched full of interior
        /// holes that no correction can ever make visible.
        geometry: &'a [f32],
        /// The same frame rendered WITHOUT this correction. The gated quantity
        /// is a difference in differences against it, which is what makes a
        /// hard raster measurable at all and what keeps a real subject edge
        /// under the mask border from reading as a seam nobody introduced.
        reference: &'a [[f32; 3]],
    },
}

pub(super) struct BitmapBoundaryInput<'a> {
    pub(super) ruler: BoundaryRuler<'a>,
    /// R37. The paired target the boundary is allowed to reproduce, at the
    /// analysis geometry: the step IT carries at a crossing is not a seam
    /// (`fit_zoned::unasked`). `None` charges every introduced step — the
    /// context rule alone, which is what the fixtures pinned on it measure.
    pub(super) target_boundary: Option<&'a [[f32; 3]]>,
    pub(super) initial_px: Vec<[f32; 3]>,
    pub(super) frame_before: f32,
}

/// One bitmap boundary/composed-frame gate shared by tiles, free masks and
/// the multi-region semantic path, each naming its own ruler.
///
/// For [`BoundaryRuler::CrossBoundaryStep`] a reading of `0` transitions is a
/// REFUSAL. That combination is precisely what shipped a seam: every hard
/// raster scored `rim 0.000` off an empty transition band, and the gate read
/// that as "well inside budget".
pub(super) fn enforce_bitmap_boundary(
    s_img: &DynamicImage,
    tgt_px: &[[f32; 3]],
    report: &mut FitReport,
    first_mask: usize,
    input: BitmapBoundaryInput<'_>,
) -> Result<BitmapBoundaryAccepted, BitmapBoundaryRefusal> {
    let BitmapBoundaryInput { ruler, target_boundary, initial_px, frame_before } = input;
    let measure = |rendered: &[[f32; 3]]| match ruler {
        BoundaryRuler::TransitionBand { weights, reference } => {
            // `initial_px` is this gate's k=1 candidate, so BOTH rulers read
            // the correction's own slope off the same frozen frame.
            boundary_rim_toward(
                target_boundary,
                reference,
                rendered,
                &initial_px,
                weights,
                s_img.width(),
                s_img.height(),
            )
        }
        BoundaryRuler::CrossBoundaryStep { geometry, reference } => {
            boundary_step_toward(
                target_boundary,
                reference,
                rendered,
                &initial_px,
                geometry,
                s_img.width(),
                s_img.height(),
            )
        }
    };
    // One ruler, one budget. The gate body is ruler-agnostic, so the budget
    // has to be chosen HERE rather than read from a constant both rulers
    // share — otherwise re-deriving the rim silently re-tunes every spatial
    // tile and free mask that passes through this same comparison.
    let budget = match ruler {
        BoundaryRuler::TransitionBand { .. } => ZONE_BOUNDARY_RIM_MAX,
        BoundaryRuler::CrossBoundaryStep { .. } => ZONE_BOUNDARY_STEP_MAX,
    };
    let initial = measure(&initial_px);
    let hard_edged = matches!(ruler, BoundaryRuler::CrossBoundaryStep { .. });
    if hard_edged && initial.transitions == 0 {
        report.recipe.masks.truncate(first_mask);
        return Err(BitmapBoundaryRefusal { why: BitmapBoundaryWhy::Unmeasured, initial });
    }
    let original = report.recipe.masks[first_mask].clone();
    let render_at = |report: &mut FitReport, k: f32| {
        shrink_zone_corrections(
            &mut report.recipe.masks[first_mask..=first_mask],
            std::slice::from_ref(&original),
            &[1.0],
            k,
        );
        let pixels = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let reading = measure(&pixels);
        let frame = fit::look_err_with_evidence(&pixels, tgt_px, &report.evidence);
        (reading, pixels, frame)
    };
    // `gated()`, not `rim`: every crossing ranked after its own context
    // charge, in luma AND in colour, whichever is worse. On a fully textured
    // or genuinely ramped border the charge branch returns the raw reading
    // bit for bit, so everything that passed on context before this batch
    // still does; what is new is that the soft family charges too, and that
    // a correction which moves one channel without moving luma is no longer
    // read as 0.
    let kept = if initial.gated() <= budget {
        let frame = fit::look_err_with_evidence(&initial_px, tgt_px, &report.evidence);
        Some((1.0, initial, initial_px, frame))
    } else {
        let zero = render_at(report, 0.0);
        if zero.0.gated() > budget {
            None
        } else {
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            let mut best = (0.0, zero.0, zero.1, zero.2);
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                let measured = render_at(report, mid);
                if measured.0.gated() <= budget {
                    lo = mid;
                    best = (mid, measured.0, measured.1, measured.2);
                } else {
                    hi = mid;
                }
            }
            Some(best)
        }
    };
    let Some((k, reading, pixels, frame)) = kept else {
        report.recipe.masks.truncate(first_mask);
        return Err(BitmapBoundaryRefusal { why: BitmapBoundaryWhy::Rim, initial });
    };
    // Both rulers now carry the frame rendered WITHOUT this correction, so
    // the question "did the surviving k actually do anything" is answerable
    // for free and exactly. See `BitmapBoundaryWhy::Inert`.
    let reference = match ruler {
        BoundaryRuler::TransitionBand { reference, .. } => reference,
        BoundaryRuler::CrossBoundaryStep { reference, .. } => reference,
    };
    if pixels == reference {
        report.recipe.masks.truncate(first_mask);
        return Err(BitmapBoundaryRefusal { why: BitmapBoundaryWhy::Inert, initial });
    }
    if frame > frame_before + SPATIAL_FRAME_REGRESSION_TOL {
        report.recipe.masks.truncate(first_mask);
        return Err(BitmapBoundaryRefusal { why: BitmapBoundaryWhy::Frame, initial });
    }
    shrink_zone_corrections(
        &mut report.recipe.masks[first_mask..=first_mask],
        std::slice::from_ref(&original),
        &[1.0],
        k,
    );
    Ok(BitmapBoundaryAccepted { pixels, reading, initial, k })
}

pub(super) fn attach_tiles(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    raster_home: &crate::store::OwnedRaster,
    refine: bool,
    cap: usize,
) -> Vec<f32> {
    // One analysis geometry for both rasters (`fit::analysis_pair`), so the
    // coverage and estimator vectors below are congruent by construction —
    // the two asserts pin that contract.
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let mut attached = BTreeSet::new();
    let mut refused = BTreeSet::new();
    let mut generation = 0usize;
    let mut excluded = vec![0.0f32; report.evidence.source_weights.len()];
    // One cache for the whole traversal: the render-independent half of every
    // node's reading is computed once and re-read on the later generations.
    let mut cache = TileEvidenceCache::new();
    let corr = report.correspondence.take();
    while attached.len() < cap {
        let current = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let root = read_tile(
            TileId { depth: 0, row: 0, col: 0 },
            &current,
            &tgt_px,
            &report.evidence,
            &mut cache,
        );
        let (visited, candidate) = next_tile(
            &current,
            &tgt_px,
            &report.evidence,
            &attached,
            &refused,
            &mut cache,
        );
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
        let mut refinement_delta = 0.0;
        let (mask, mut refined) = if refine {
            match crate::mask_refine::guided_refine(
                &guide,
                &raw_mask,
                8,
                (4.0f32 / 255.0).powi(2),
            ) {
                crate::mask_refine::RefineOutcome::Kept { mask, reading: refined } => {
                    push_refinement_note(report, &reading.id.label(), true, refined);
                    refinement_delta = refinement_alpha_delta(&raw_mask, &mask);
                    if refinement_delta == 0.0 {
                        (raw_mask, false)
                    } else {
                        (mask, true)
                    }
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
        let mut accepted_coverage = coverage.source.clone();
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
        let mut attachment =
            tile_attachment(&reading, mask.dimensions(), source_weights, target_weights, coverage);
        if refined {
            attachment.mask = MaskGeometry::Bitmap { path: owned.path().to_string_lossy().into_owned() };
            attachment.components.clear();
        }
        let frame_before = fit::look_err_with_evidence(&current, &tgt_px, &report.evidence);
        let mut frame_err = frame_before;
        let first_tile = report.recipe.masks.len();
        let notes_before_attach = report.notes.len();
        let rationale_before_attach = report.recipe.rationale.len();
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
        let boundary = enforce_bitmap_boundary(
            &s_img,
            &tgt_px,
            report,
            first_tile,
            BitmapBoundaryInput {
                ruler: BoundaryRuler::CrossBoundaryStep {
                    geometry: &accepted_coverage,
                    reference: &current,
                },
                target_boundary: Some(&tgt_px[..]),
                initial_px: accepted.rendered,
                frame_before,
            },
        );
        let mut boundary = match boundary {
            Ok(boundary) => {
                crate::rationale::push_note(
                    &mut report.recipe.rationale,
                    &mut report.notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::TILE_BOUNDARY_PASSED,
                        boundary_args(reading.id, boundary.k, boundary.initial, boundary.reading),
                    ),
                );
                boundary
            }
            Err(refusal) => {
                crate::rationale::push_note(
                    &mut report.recipe.rationale,
                    &mut report.notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::TILE_BOUNDARY_REFUSED,
                        boundary_args(reading.id, 0.0, refusal.initial, refusal.initial),
                    ),
                );
                owned.remove();
                refused.insert(reading.id);
                continue;
            }
        };
        let mut rendered_delta = values::UNMEASURED_HARD_MASK.to_string();
        let mut fidelity = values::UNMEASURED.to_string();
        if refined {
            // A guide can move edge alpha substantially while the fitted
            // correction barely moves the image. Put a native trial through
            // the SAME estimator and boundary gate, then compare both actual
            // renders at the 2048-edge tile raster. No coverage/core shortcut
            // and no slider-magnitude proxy decides this projection.
            let saved_recipe = report.recipe.clone();
            let saved_notes = report.notes.clone();
            report.recipe.masks.truncate(first_tile);
            report.recipe.rationale.truncate(rationale_before_attach);
            report.notes.truncate(notes_before_attach);
            let (native_mask, native_components) = tile_geometry(reading.id, mask.dimensions());
            // R38: the contour in the PREVIEW's frame. The pixels this trial
            // is judged on come from `develop_preview`, which evaluates a
            // parametric geometry through `MaskFrame::downstream`; under a lens
            // profile that edge is not at the stored coordinates a stored-frame
            // contour would put the ruler's feet around
            // (`preview_mask_coverage`, and the pin
            // `the_native_trial_gate_reads_the_edge_the_preview_paints`).
            let native_coverage = render::preview_mask_coverage(&LocalAdjustment {
                mask: native_mask.clone(), components: native_components.clone(), ..Default::default()
            }, &s_img, &report.recipe);
            let weights: Vec<f32> = native_coverage.as_raw().iter().map(|a| *a as f32 / 255.0).collect();
            let native_attachment = tile_attachment(&reading, mask.dimensions(),
                reading.source_weights.clone(), reading.target_weights.clone(),
                ZoneCoverage { source: weights.clone(), target: weights.clone() });
            let mut native_frame = frame_before;
            let native = attach_one_zone(&s_img, &tgt_px, report, &mut native_frame,
                &native_attachment, reading.divergence, corr.as_ref());
            rendered_delta = values::NATIVE_TRIAL_GATED.to_string();
            let mut projection = None;
            if let Some(mut native) = native {
                let trial = enforce_bitmap_boundary(&s_img, &tgt_px, report, first_tile,
                    BitmapBoundaryInput {
                        ruler: BoundaryRuler::CrossBoundaryStep { geometry: &weights, reference: &current },
                        target_boundary: Some(&tgt_px[..]),
                        initial_px: std::mem::take(&mut native.rendered), frame_before,
                    });
                if let Ok(trial) = trial {
                    let raster_pixels = render::develop_preview(&guide, &saved_recipe).to_rgb8();
                    let native_pixels = render::develop_preview(&guide, &report.recipe).to_rgb8();
                    let delta = raster_pixels.as_raw().iter().zip(native_pixels.as_raw())
                        .map(|(a,b)| a.abs_diff(*b) as f32 / 255.0).fold(0.0, f32::max);
                    rendered_delta = format!("{delta:.6}");
                    // R36: fidelity to the target decides (see
                    // `native_carrier_fits`); the rendered change is a reading.
                    // Both residuals are read over the HARD cell — the one
                    // population both carriers claim — and both frames over
                    // the same frozen evidence.
                    let hard_target = zone_moments(&tgt_px, &reading.target_weights);
                    let raster_zone = zone_err(
                        &zone_moments(&boundary.pixels, &reading.source_weights),
                        &hard_target,
                    );
                    let native_zone = zone_err(
                        &zone_moments(&trial.pixels, &reading.source_weights),
                        &hard_target,
                    );
                    let raster_frame_after =
                        fit::look_err_with_evidence(&boundary.pixels, &tgt_px, &report.evidence);
                    let native_frame_after =
                        fit::look_err_with_evidence(&trial.pixels, &tgt_px, &report.evidence);
                    fidelity = format!("{raster_zone:.6}/{native_zone:.6}");
                    if native_carrier_fits(raster_zone, native_zone, raster_frame_after, native_frame_after) {
                        projection = Some((native, trial, native_attachment));
                    }
                }
            }
            if let Some((native, trial, native_attachment)) = projection {
                accepted = native;
                boundary = trial;
                accepted_coverage = native_attachment.coverage.as_ref().expect("tile coverage").source.clone();
                attachment = native_attachment;
                refined = false;
                crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
                    crate::rationale::Note::new(crate::rationale::keys::TILE_BOUNDARY_PASSED,
                        boundary_args(reading.id, boundary.k, boundary.initial, boundary.reading)));
            } else {
                report.recipe = saved_recipe;
                report.notes = saved_notes;
            }
        }
        if !refined { owned.remove(); }
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::TILE_MASK_CARRIER,
                vec![
                    ("id", reading.id.tag()),
                    ("carrier", if refined { values::BITMAP_CARRIER } else { values::FOUR_GRADIENTS }.to_string()),
                    ("delta", format!("{refinement_delta:.6}")),
                    ("rendered", rendered_delta),
                    ("fidelity", fidelity),
                ],
            ),
        );
        let target_moments = zone_moments(&tgt_px, &attachment.target_weights);
        accepted.after = zone_err(
            &zone_moments(&boundary.pixels, &attachment.source_weights),
            &target_moments,
        );
        let frame_after =
            fit::look_err_with_evidence(&boundary.pixels, &tgt_px, &report.evidence);
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
                    ("boundary", format!("{:.5}", boundary.reading.rim)),
                ],
            ),
        );
        let _path = owned.into_path();
        // The SHRUNK alpha, not the raw raster. What this vector withholds from
        // the free-mask producer is the correction a tile actually delivers, and
        // the boundary gate may have negotiated that correction down to a
        // fraction `k` of itself (the calibration island's accepted tiles keep
        // k = 0.114 / 0.168 / 0.187). A tile shrunk to a ninth of its fitted
        // strength leaves most of its residual on the frame, so blocking the
        // next producer with the full alpha hid work nobody had done.
        for (dst, alpha) in excluded.iter_mut().zip(accepted_coverage) {
            *dst = dst.max(alpha * boundary.k);
        }
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
    excluded
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// R38 (2026-09-12). The native four-gradient trial's gate builds its
    /// contour from the tile's parametric geometry and then judges
    /// `develop_preview`'s pixels — which evaluate that geometry in
    /// `MaskFrame::downstream`. Under the reference pair's lens profile
    /// (sixteen distortion knots to 1.050 and its mask-warp table) the
    /// preview paints tile r1c0's right edge three to four analysis pixels
    /// right of its stored column, so a contour built `AsRendered` put the
    /// step ruler's feet (1.5 px each side) on two lifted pixels and read no
    /// step there, while the one undisplaced edge ran over dark land: the
    /// shipped fit carried a 27-code seam the gate had read as 5 codes. The
    /// contour must be the preview's own ([`render::preview_mask_coverage`]).
    /// This scene — bright only in a band around the displaced edge, black
    /// elsewhere — is that misread made deterministic: the stored-frame
    /// ruler passes a 31-code seam, the preview-frame ruler reads it.
    #[test]
    fn the_native_trial_gate_reads_the_edge_the_preview_paints() {
        use crate::recipe::{EditRecipe, LensProfile, LocalAdjustment};
        let distortion = vec![1.0002441f32, 1.0003052, 1.0007935, 1.00177, 1.0030518, 1.0049438, 1.0071411, 1.0097656, 1.0128174, 1.0164795, 1.0205078, 1.0252075, 1.0305176, 1.036438, 1.0429688, 1.0502319];
        let mask_warp = vec![1.0499613f32, 1.0499613, 1.0499614, 1.0499613, 1.0499487, 1.0499315, 1.0499144, 1.0498966, 1.0497608, 1.0496244, 1.0494881, 1.0493188, 1.049047, 1.0487754, 1.048504, 1.0481926, 1.0478381, 1.0474837, 1.0471296, 1.0466602, 1.0461413, 1.0456232, 1.0451062, 1.0445169, 1.0439203, 1.0433248, 1.0427231, 1.0420175, 1.0413136, 1.0406117, 1.0398878, 1.0390792, 1.0382727, 1.0374687, 1.0366169, 1.0356635, 1.0347136, 1.033767, 1.0327886, 1.0317588, 1.0307329, 1.029711, 1.0286266, 1.0274526, 1.0262835, 1.0251197, 1.0239081, 1.022614, 1.0213264, 1.0200449, 1.0187324, 1.0173283, 1.0159314, 1.014542, 1.0131469, 1.0116421, 1.0101459, 1.0086582, 1.0071787, 1.0055816, 1.0039712, 1.0023705, 1.0007797, 0.9999865];
        let profile = LensProfile { distortion, distortion_on: true, mask_warp, ..Default::default() };
        let (w, h) = (384u32, 256u32);
        // Bright only in a band around the tile's right edge (stored x = 95),
        // black elsewhere, so no other crossing can carry the reading.
        let scene = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
            Rgb([if (88..108).contains(&x) { 150 } else { 0 }; 3])
        }));
        let (mask, components) = super::tile_geometry(TileId { depth: 2, row: 1, col: 0 }, (2048, 1365));
        let tile = LocalAdjustment { mask, components, exposure_ev: 0.6, ..Default::default() };
        let recipe = EditRecipe { lens_profile: profile.clone(), masks: vec![tile.clone()], ..Default::default() };
        let plain = EditRecipe { lens_profile: profile, ..Default::default() };
        let rendered = fit::pixels_of(&render::develop_preview(&scene, &recipe));
        let reference = fit::pixels_of(&render::develop_preview(&scene, &plain));
        let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        let lift = |x: usize, y: usize| {
            (luma(&rendered[y * w as usize + x]) - luma(&reference[y * w as usize + x])) * 255.0
        };
        let seam = lift(92, 100);
        assert!(seam > 25.0, "premise: the tile lifts the bright band by {seam:.1} codes");
        let painted = (86..106).rev().find(|&x| lift(x, 100) > seam * 0.5).expect("the lifted edge") as u32;
        let stored = render::mask_coverage(&tile, &scene, render::MaskFrame::AsRendered);
        let preview = render::preview_mask_coverage(&tile, &scene, &recipe);
        let right = |cov: &image::GrayImage| (0..w).rev().find(|&x| cov.get_pixel(x, 100)[0] >= 128).expect("coverage");
        assert!(
            right(&stored).abs_diff(painted) >= 3,
            "premise: the profile paints the edge at {painted}, off the stored column {}",
            right(&stored)
        );
        assert!(
            right(&preview).abs_diff(painted) <= 1,
            "the preview-frame contour ({}) is the painted edge ({painted})",
            right(&preview)
        );
        let read = |cov: &image::GrayImage| {
            let weights: Vec<f32> = cov.as_raw().iter().map(|v| *v as f32 / 255.0).collect();
            crate::fit_zoned::boundary_step_toward(None, &reference, &rendered, &rendered, &weights, w, h).rim
        };
        let (on_stored, on_preview) = (read(&stored), read(&preview));
        assert!(
            on_stored < crate::fit_zoned::ZONE_BOUNDARY_STEP_MAX,
            "the misread this pins: the stored-frame ruler passes the seam ({:.1} codes)",
            on_stored * 255.0
        );
        assert!(on_preview > 0.1, "the preview-frame ruler reads the seam ({:.1} codes)", on_preview * 255.0);
    }

    /// The cache is a per-traversal accelerator, never a behaviour: every
    /// test below reads a node with an EMPTY cache, so what it asserts is
    /// the computed reading and not a stored one.
    fn read_tile_uncached(
        id: TileId,
        current: &[[f32; 3]],
        target: &[[f32; 3]],
        evidence: &fit::EvidenceModel,
    ) -> TileReading {
        read_tile(id, current, target, evidence, &mut TileEvidenceCache::new())
    }

    type BoundaryFixture = (
        DynamicImage,
        Vec<[f32; 3]>,
        FitReport,
        crate::store::OwnedRaster,
        Vec<f32>,
        Vec<[f32; 3]>,
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
        let supported = read_tile_uncached(id, &current, &target, &evidence);
        assert!(supported.source_share >= MIN_ZONE_SHARE, "{supported:?}");
        assert!(
            (supported.source_share - supported.target_share).abs() < 1e-4,
            "a tile's two shares are one population: {supported:?}"
        );
        evidence.spatial_weights.fill(0.0);
        let unsupported = read_tile_uncached(id, &current, &target, &evidence);
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
            pixels: MIN_MASK_PIXELS,
            divergence: Some(fit::Divergence { correlation: 1.0, energy_error: 0.0, d: 0.0 }),
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
        let reading = read_tile_uncached(
            TileId { depth: 2, row: 0, col: 0 },
            &current,
            &target,
            &evidence,
        );
        assert!(
            reading.divergence.is_some_and(|d| d.d >= fit::DIVERGENCE_ZONE),
            "{reading:?}"
        );
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
            &mut TileEvidenceCache::new(),
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
            "autoshade-tile-{tag}-{}",
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
        let expected = read_tile_uncached(id, &current_pixels, &target_pixels, &evidence).residual;
        let stale = read_tile_uncached(id, &source_pixels, &target_pixels, &evidence).residual;
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
        let reading = read_tile_uncached(
            TileId { depth: 2, row: 2, col: 0 },
            &current,
            &target,
            &evidence,
        );
        let attachment = tile_attachment(
            &reading,
            (64, 64),
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

    #[test]
    fn four_gradient_tiles_equal_integer_rasters_and_frozen_evidence_shares() {
        for (w, h) in [(384, 256), (385, 257), (2048, 1365)] {
            let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                Rgb([64 + ((x + y) % 96) as u8; 3])
            }));
            for (row, col) in [(0, 0), (1, 2), (3, 3)] {
                let id = TileId { depth: 2, row, col };
                // thumbnail() can upscale a small fixture. Compare coverage
                // on the raster actually stamped, never zip two different grids.
                let (guide, hard) = tile_mask(&src, id);
                let (mask, components) = super::tile_geometry(id, hard.dimensions());
                let native = render::mask_coverage(&LocalAdjustment {
                    mask, components, ..Default::default()
                }, &guide, render::MaskFrame::AsRendered);
                assert_eq!(native.dimensions(), hard.dimensions());
                assert!(native.as_raw().iter().zip(hard.as_raw()).all(|(a,b)| a.abs_diff(*b) <= 1),
                    "source {w}x{h}, raster {:?}, tile {row}/{col}", hard.dimensions());
                if w == 384 {
                    let px = fit::pixels_of(&src);
                    let evidence = fit::evidence_model_for(&px, &px, w, h);
                    let scope = |alpha: &GrayImage| scoped_mask_evidence(&px, &evidence,
                        &mask_weights(alpha, w, h));
                    let (a, b) = (scope(&native), scope(&hard));
                    assert!((a.source_share - b.source_share).abs() <= 1e-4);
                    assert!((a.target_share - b.target_share).abs() <= 1e-4);
                }
            }
        }
    }

    #[test]
    fn geometry_tiles_turn_every_intersection_with_the_image() {
        let source = DynamicImage::new_rgb8(385, 257);
        let id = TileId { depth: 2, row: 1, col: 2 };
        let (guide, hard) = tile_mask(&source, id);
        let (mask, components) = super::tile_geometry(id, hard.dimensions());
        let mut recipe = crate::recipe::EditRecipe { masks: vec![LocalAdjustment {
            mask, components, ..Default::default()
        }], ..Default::default() };
        render::orient_recipe_coords(&mut recipe, rawler::Orientation::Rotate90, None);
        let native = render::mask_coverage(&recipe.masks[0], &guide.rotate90(), render::MaskFrame::AsRendered);
        let expected = image::imageops::rotate90(&hard);
        assert_eq!(native.dimensions(), expected.dimensions());
        assert!(native.as_raw().iter().zip(expected.as_raw()).all(|(a,b)| a.abs_diff(*b) <= 1));
        assert_eq!(recipe.masks[0].components.len(), 3);
    }

    #[test]
    fn the_native_carrier_ships_on_equal_or_better_fidelity_and_never_on_worse() {
        assert!(native_carrier_fits(0.0300, 0.0300, 0.0200, 0.0200), "a tie goes to the native carrier");
        assert!(native_carrier_fits(0.0300, 0.0290, 0.0200, 0.0199));
        assert!(!native_carrier_fits(0.0300, 0.0301, 0.0200, 0.0200), "a worse hard-cell residual keeps the raster");
        assert!(!native_carrier_fits(0.0300, 0.0300, 0.0200, 0.0201), "a worse frame keeps the raster");
    }

    #[test]
    fn tile_refinement_compares_every_alpha_not_just_mass_or_core() {
        let hard = GrayImage::from_fn(32, 32, |x,_| Luma([if x < 16 { 255 } else { 0 }]));
        assert_eq!(refinement_alpha_delta(&hard, &hard), 0.0);
        let shifted = GrayImage::from_fn(32, 32, |x,_| Luma([if x > 0 && x <= 16 { 255 } else { 0 }]));
        assert_eq!(hard.as_raw().iter().map(|v| *v as u32).sum::<u32>(), shifted.as_raw().iter().map(|v| *v as u32).sum::<u32>());
        assert!(refinement_alpha_delta(&hard, &shifted) > ZONE_BOUNDARY_STEP_MAX);
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
        let reading = read_tile_uncached(id, &source_px, &target_px, &evidence);
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
            raw_mask.dimensions(),
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
            Some(fit::Divergence { correlation: 1.0, energy_error: 0.0, d: 0.0 }),
            None,
        );
        path.remove();
        assert!(
            report
                .notes
                .iter()
                .any(|n| super::super::is_tone_refusal(n.key)),
            "moving the withheld half must withhold the tone controls: {}",
            report.recipe.rationale
        );
        assert!(
            accepted.is_none() && report.recipe.masks.is_empty(),
            "a tile that would move withheld pixels must not attach: {}",
            report.recipe.rationale
        );
    }

    /// `target_ev` is the dose the target actually wants: `None` leaves the
    /// target as the untouched source (no correction is an improvement), and
    /// `Some(ev)` renders the same mask at a weaker dose, so shrinking the
    /// candidate towards it is a frame improvement rather than a regression.
    fn boundary_fixture(
        exposure_ev: f32,
        target_ev: Option<f32>,
        name: &str,
    ) -> BoundaryFixture {
        contextual_fixture(80, exposure_ev, target_ev, name)
    }

    /// `texture` is the sawtooth amplitude in 8-bit code values; 0 is a
    /// flat field. The DIAL is identical across the arms of a contextual
    /// test, so the arms differ ONLY in what the neighbourhood does —
    /// which is the whole claim: a contextual budget cannot be falsified
    /// with one neighbourhood.
    fn contextual_fixture(
        texture: u8,
        exposure_ev: f32,
        target_ev: Option<f32>,
        name: &str,
    ) -> BoundaryFixture {
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, y| {
            let base =
                if texture == 0 { 80 } else { 80 + ((x * 3 + y * 5) % texture as u32) as u8 };
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
        // The render WITHOUT the correction: what the gate differences against.
        let reference = fit::pixels_of(&render::develop_preview(
            &source,
            &crate::recipe::EditRecipe::default(),
        ));
        let target = match target_ev {
            Some(ev) => {
                let mut wanted = recipe.clone();
                wanted.masks[0].exposure_ev = ev;
                render::develop_preview(&source, &wanted)
            }
            None => source.clone(),
        };
        let target_pixels = fit::pixels_of(&target);
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe = recipe;
        let geometry = mask_weights(&mask, 64, 64);
        (source, target_pixels, report, path, geometry, reference, candidate)
    }

    /// A mask whose alpha RAMPS across 32 px instead of stepping. The
    /// correction it carries is continuous, so it is not a seam however large
    /// its in-zone delta is — the control that separates a paired
    /// cross-boundary reading from a one-sided in-zone one.
    fn feathered_fixture(exposure_ev: f32, name: &str) -> BoundaryFixture {
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |_, _| Rgb([128, 128, 128])));
        let mask = GrayImage::from_fn(64, 64, |x, _| {
            Luma([(((x as f32 - 16.0) / 32.0).clamp(0.0, 1.0) * 255.0).round() as u8])
        });
        let path = super::super::tests::fixture_mask_path(name);
        mask.save(path.path()).unwrap();
        let adjustment = LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            name: "Feathered ramp".to_string(),
            role: MaskRole::Custom,
            amount: 1.0,
            exposure_ev,
            ..Default::default()
        };
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(adjustment);
        let candidate = fit::pixels_of(&render::develop_preview(&source, &recipe));
        let reference = fit::pixels_of(&render::develop_preview(
            &source,
            &crate::recipe::EditRecipe::default(),
        ));
        let target = render::develop_preview(&source, &recipe);
        let target_pixels = fit::pixels_of(&target);
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe = recipe;
        let geometry = mask_weights(&mask, 64, 64);
        (source, target_pixels, report, path, geometry, reference, candidate)
    }

    /// A hard 0/255 tile raster over a SMOOTH, sloping field: the correction
    /// it carries has an in-zone gradient of its own because the SCENE has
    /// one, not because its alpha ramps.
    ///
    /// Every other fixture in this module holds the field flat (or fills it
    /// with a sawtooth) and varies the ALPHA, so the only same-side slope any
    /// of them offers the per-crossing budget is a mask shape. A cloudless sky
    /// falling off towards the horizon is the shape the seam batch was about,
    /// and under a hard tile edge it reads as a constant alpha over a varying
    /// dose — the one arrangement none of them builds.
    ///
    /// Two deliberate choices. DIAGONAL, because a tile has a vertical and a
    /// horizontal border and a one-axis gradient leaves the crossings on one
    /// of them flat; those would earn the floor, take the top of the ranking
    /// and decide the percentile by themselves. WARM with the three channels
    /// rising at different rates, because on a grey field every luma
    /// difference is a whole 8-bit code and the earned budget could then only
    /// land on the floor or near the ceiling — the same reason
    /// [`shoulder_fixture`] is not grey. Here the field's own luma rises 0.44
    /// code per pixel after the render's curve, so the scene's step across the
    /// 3-px baseline is 1.32 code — over the floor and under two — while a
    /// +1.5 EV dose slopes 0.80 code beside it, and three times that clears
    /// two codes without reaching the ceiling. That separation is the whole
    /// design: an 8-bit kept step can only tell the two budgets apart if they
    /// admit a different NUMBER of code values.
    fn gradient_fixture(exposure_ev: f32, name: &str) -> BoundaryFixture {
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, y| {
            let d = x + y;
            Rgb([(90 + 2 * d / 3) as u8, (72 + d / 2) as u8, (60 + d / 3) as u8])
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
        let reference = fit::pixels_of(&render::develop_preview(
            &source,
            &crate::recipe::EditRecipe::default(),
        ));
        let mut wanted = recipe.clone();
        wanted.masks[0].exposure_ev = exposure_ev * 0.5;
        let target = render::develop_preview(&source, &wanted);
        let target_pixels = fit::pixels_of(&target);
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe = recipe;
        let geometry = mask_weights(&mask, 64, 64);
        (source, target_pixels, report, path, geometry, reference, candidate)
    }

    /// Captured from a run of `gradient_fixture` at +1.5 EV (the test below
    /// says what moving them means).
    const GRADIENT_K: f32 = 0.030517578;
    const GRADIENT_RIM: f32 = 0.007396072;

    /// A22 (v1.2.4). The slope term is read on a correction whose gradient is
    /// the SCENE's, and it is read off the FROZEN k = 1 candidate.
    ///
    /// The dose is chosen so the earned budget clears TWO code values while
    /// the scene's own step across the same baseline clears only one: the
    /// kept step then lands on a different 8-bit code depending on whether
    /// the slope was consulted, which is the only way an 8-bit reading can
    /// tell the two apart.
    ///
    /// MUTATIONS, both run on 2026-09-02. Zero the term (`let slope_in =
    /// 0.0f32; let slope_out = 0.0f32;` in `boundary_line_steps`): this test
    /// fails on the charge comparison, and so do
    /// `a_ramp_earns_budget_only_where_it_persists_past_the_collar` and
    /// `tile_boundary_shrink_preserves_direction_and_budget` — 3 of 149.
    /// Read it off the render under bisection instead of the frozen candidate
    /// (`boundary_step(reference, rendered, rendered, ...)` in
    /// `enforce_bitmap_boundary`): the budget chases `k` down, and the pinned
    /// pair lands on (0.029785156, 0.005094141) — a kept step of 1.30 code
    /// where the frozen reading keeps 1.89.
    #[test]
    fn a_scene_gradient_under_a_hard_raster_earns_its_own_slope_budget() {
        const EV: f32 = 1.5;
        let (source, target, mut report, path, geometry, reference, candidate) =
            gradient_fixture(EV, "ctx-budget-gradient");
        let with_slope = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        // The SAME crossings with the slope term switched off and nothing
        // else changed: handing the ruler the reference as its own frozen
        // candidate makes `u1` identically zero on both sides, so each
        // crossing earns the scene's own step alone.
        let no_slope = boundary_step(&reference, &candidate, &reference, &geometry, 64, 64);
        assert_eq!(
            (with_slope.rim.to_bits(), with_slope.transitions),
            (no_slope.rim.to_bits(), no_slope.transitions),
            "the raw step is the same reading either way: {with_slope:?} vs {no_slope:?}",
        );
        assert!(
            with_slope.charged < no_slope.charged,
            "the scene's own gradient must buy budget: {with_slope:?} vs {no_slope:?}",
        );
        let frame_before = fit::look_err_with_evidence(&reference, &target, &report.evidence);
        let accepted = enforce_bitmap_boundary(
            &source,
            &target,
            &mut report,
            0,
            BitmapBoundaryInput {
                target_boundary: None,
                ruler: BoundaryRuler::CrossBoundaryStep {
                    geometry: &geometry,
                    reference: &reference,
                },
                initial_px: candidate,
                frame_before,
            },
        )
        .expect("a sloping-sky seam is negotiated down, not dropped");
        path.remove();
        assert_eq!(
            (accepted.k, accepted.reading.rim),
            (GRADIENT_K, GRADIENT_RIM),
            "the slope-earned budget must land on these exact bits: {:?} from \n             {with_slope:?} against {no_slope:?}",
            accepted.reading,
        );
        assert!(
            accepted.reading.rim > BOUNDARY_STEP_FLOOR
                && accepted.reading.rim < ZONE_BOUNDARY_STEP_MAX,
            "the earned budget sits strictly between floor and ceiling: {:?}",
            accepted.reading,
        );
    }

    /// A mask whose alpha JUMPS to 0.55 at the contour and then rises to
    /// 1.0 over `shoulder` px. With a short shoulder this is the shape of
    /// a hard raster's resample-and-refine collar: a step wearing a soft
    /// shoulder. With a long one it is a step carrying a genuine
    /// persistent ramp. The target sits at `target_ev`, so a shrink
    /// IMPROVES the frame and the boundary gate is the only judge.
    fn shoulder_fixture(
        shoulder: f32,
        exposure_ev: f32,
        target_ev: f32,
        name: &str,
    ) -> BoundaryFixture {
        // A warm, NON-grey field on purpose: on a grey field every luma
        // difference is a whole 8-bit code, so a same-side slope quantises to
        // 0 or 1 code and `3 x slope` can only ever be the floor or ~the
        // ceiling. Three channels rounding separately give luma sub-code
        // resolution, so the earned budget can sit strictly between the two.
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |_, _| Rgb([150, 120, 100])));
        let mask = GrayImage::from_fn(64, 64, |x, _| {
            Luma([if x < 32 {
                0
            } else {
                let alpha = 0.55 + 0.45 * (((x - 32) as f32 / shoulder).min(1.0));
                (alpha * 255.0).round() as u8
            }])
        });
        let path = super::super::tests::fixture_mask_path(name);
        mask.save(path.path()).unwrap();
        let adjustment = LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            name: "Shouldered step".to_string(),
            role: MaskRole::Custom,
            amount: 1.0,
            exposure_ev,
            ..Default::default()
        };
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(adjustment);
        let candidate = fit::pixels_of(&render::develop_preview(&source, &recipe));
        let reference = fit::pixels_of(&render::develop_preview(
            &source,
            &crate::recipe::EditRecipe::default(),
        ));
        let mut wanted = recipe.clone();
        wanted.masks[0].exposure_ev = target_ev;
        let target = render::develop_preview(&source, &wanted);
        let target_pixels = fit::pixels_of(&target);
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe = recipe;
        let geometry = mask_weights(&mask, 64, 64);
        (source, target_pixels, report, path, geometry, reference, candidate)
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

        // Rebuilt 2026-08-30. The previous body asked a 0/255 tile carrying
        // +0.01 EV to "pass the budget" and it did — but so would ANY tile,
        // because the rim ruler reads only mask weights inside [0.05, 0.95)
        // and a hard raster has none. The assertion was true and vacuous. The
        // fixture now carries a correction that really does step, and the
        // three things a mutation can break are pinned separately.
        let (source, target, mut report, path, geometry, reference, candidate) =
            boundary_fixture(-0.40, Some(-0.20), "tile-boundary-budget");

        // Premise 1: the old ruler is blind here, by construction.
        let unread = boundary_rim(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert_eq!(
            unread.transitions, 0,
            "premise: a 0/255 raster has no transition band to read: {unread:?}"
        );
        // Premise 2: the new ruler is not, and this tile is over budget.
        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert!(measured.transitions > 0, "the 50% contour must be measurable: {measured:?}");
        assert!(
            measured.rim > ZONE_BOUNDARY_STEP_MAX,
            "premise: a -0.40 EV hard tile steps across its own border: {measured:?}"
        );

        let frame_before =
            fit::look_err_with_evidence(&reference, &target, &report.evidence);
        let accepted = enforce_bitmap_boundary(
            &source,
            &target,
            &mut report,
            0,
            BitmapBoundaryInput {
                target_boundary: None,
                ruler: BoundaryRuler::CrossBoundaryStep {
                    geometry: &geometry,
                    reference: &reference,
                },
                initial_px: candidate,
                frame_before,
            },
        )
        .expect("a shrinkable tile must be negotiated, not dropped");
        assert!(accepted.k < 1.0, "an over-budget step must really shrink: k={}", accepted.k);
        assert!(
            accepted.reading.rim <= ZONE_BOUNDARY_STEP_MAX,
            "the kept reading must be inside the budget: {:?}",
            accepted.reading
        );
        assert!(
            accepted.reading.transitions > 0,
            "a pass may never rest on zero crossings again: {:?}",
            accepted.reading
        );
        let kept = report.recipe.masks[0].exposure_ev;
        assert!(kept < 0.0 && kept.abs() < 0.40, "direction kept and shrunk: {kept}");
        path.remove();

        // A ramp is not a step. The same in-zone delta that would fail a
        // one-sided reading is continuous across the contour, so the paired
        // difference keeps it whole. This is the assertion that dies if the
        // sampling is made one-sided.
        let (source, target, mut report, feather, geometry, reference, candidate) =
            feathered_fixture(0.55, "tile-boundary-feathered");
        let frame_before =
            fit::look_err_with_evidence(&reference, &target, &report.evidence);
        let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        // x=33 is the inside foot of the crossing at the 50% contour (x=32).
        let inside = 32 * 64 + 33;
        let one_sided = (luma(&candidate[inside]) - luma(&reference[inside])).abs();
        assert!(
            one_sided > ZONE_BOUNDARY_STEP_MAX,
            "premise: the in-zone delta alone is over budget ({one_sided})"
        );
        let paired = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert!(paired.transitions > 0, "the ramp still crosses 50%: {paired:?}");
        assert!(
            paired.rim <= ZONE_BOUNDARY_STEP_MAX,
            "a continuous ramp is not a cross-boundary step: {paired:?} vs {one_sided}"
        );
        // The MECHANISM, not just the outcome: the ramp's own same-side
        // slope saturates its per-crossing budget at the ceiling (3 x
        // ~0.0089 luma clamps to 0.012), so the charge branch returns the
        // raw step bit for bit. A future edit that keeps the ramp green for
        // any other reason moves these bits.
        assert_eq!(
            paired.charged.to_bits(),
            paired.rim.to_bits(),
            "a saturated ramp must charge nothing: {paired:?}"
        );
        let accepted = enforce_bitmap_boundary(
            &source,
            &target,
            &mut report,
            0,
            BitmapBoundaryInput {
                target_boundary: None,
                ruler: BoundaryRuler::CrossBoundaryStep {
                    geometry: &geometry,
                    reference: &reference,
                },
                initial_px: candidate,
                frame_before,
            },
        )
        .expect("a continuous ramp must not be refused");
        assert_eq!(accepted.k, 1.0, "a ramp must be kept whole, not shrunk");
        feather.remove();
    }

    /// Acceptance (v1.2.2 seam batch): the hard-raster gate charges each
    /// crossing against its own context, so the SAME dial that is
    /// over-budget in clean sky is untouched over texture. No scalar budget
    /// of any value can produce these verdicts at once, which is what pins
    /// the mechanism rather than one number.
    ///
    /// Neither arm here can see the frozen-candidate rule: both budgets are
    /// decided by the neighbourhood (a flat field earns the floor, texture at
    /// 2.94x the ceiling clamps), so the slope term is 0 or irrelevant and a
    /// mutation reading it off the render under bisection changes nothing on
    /// this fixture. Two other tests hold that rule, both measured under that
    /// exact mutation on 2026-09-02:
    /// `a_ramp_earns_budget_only_where_it_persists_past_the_collar` arm F
    /// moves from (k 0.24536133, rim 0.007843137) to (0.14794922,
    /// 0.0039215684), and
    /// `a_scene_gradient_under_a_hard_raster_earns_its_own_slope_budget`
    /// from (0.030517578, 0.007396072) to (0.029785156, 0.005094141). The
    /// second is the smooth in-zone gradient this note used to say no fixture
    /// built.
    #[test]
    fn contextual_budget_charges_smooth_borders_and_leaves_textured_ones_alone() {
        const ARM_EV: f32 = 0.09;
        // Captured from a run of the scalar-rule path (charged == rim on a
        // fully textured border makes the new gate walk it bit-for-bit).
        const ARM_B_SHIPPED_K: f32 = 0.80249023;
        const ARM_B_SHIPPED_RIM: f32 = 0.011764735;
        let gate = |source: &DynamicImage,
                    target: &[[f32; 3]],
                    report: &mut FitReport,
                    geometry: &[f32],
                    reference: &[[f32; 3]],
                    candidate: Vec<[f32; 3]>| {
            let frame_before = fit::look_err_with_evidence(reference, target, &report.evidence);
            enforce_bitmap_boundary(
                source,
                target,
                report,
                0,
                BitmapBoundaryInput {
                    target_boundary: None,
                    ruler: BoundaryRuler::CrossBoundaryStep { geometry, reference },
                    initial_px: candidate,
                    frame_before,
                },
            )
        };

        // Arm A — smooth field, hard mask: RED on the flat ceiling. The
        // dial sits INSIDE the old scalar budget, so the shipped rule
        // free-passed it at k=1.0 — that pass is the measured 7.8-sigma
        // sky seam — while its context (scene step 0, correction slope 0)
        // earns only the floor.
        let (source, target, mut report, path, geometry, reference, candidate) =
            contextual_fixture(0, ARM_EV, Some(ARM_EV * 0.5), "ctx-budget-flat");
        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert!(
            measured.rim > BOUNDARY_STEP_FLOOR && measured.rim <= ZONE_BOUNDARY_STEP_MAX,
            "premise: the dial must sit inside the old scalar budget: {measured:?}"
        );
        assert!(
            measured.charged > ZONE_BOUNDARY_STEP_MAX,
            "a flat neighbourhood must charge that same step over the ceiling: {measured:?}"
        );
        let accepted =
            gate(&source, &target, &mut report, &geometry, &reference, candidate)
                .expect("a smooth-sky seam is negotiated down, not dropped");
        path.remove();
        assert!(
            accepted.k < 1.0,
            "the shipped rule free-passed this; the contextual one must shrink: k={}",
            accepted.k
        );
        assert!(
            accepted.reading.rim <= BOUNDARY_STEP_FLOOR + 1e-6,
            "on a flat field every budget is the floor, so the kept raw step is one code: {:?}",
            accepted.reading
        );

        // Arm B — the SAME dial over texture: bit-identity with the scalar
        // rule. The sawtooth's minimum scene step across any crossing is 9
        // code = 0.0353 luma, 2.94x the ceiling, so every budget clamps to
        // the ceiling and the charge branch returns the raw step verbatim.
        let (source, target, mut report, path, geometry, reference, candidate) =
            contextual_fixture(80, ARM_EV, Some(ARM_EV * 0.5), "ctx-budget-textured");
        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert_eq!(
            measured.charged.to_bits(),
            measured.rim.to_bits(),
            "texture at 2.94x the ceiling charges nothing: {measured:?}"
        );
        let accepted =
            gate(&source, &target, &mut report, &geometry, &reference, candidate)
                .expect("the textured arm was never in question");
        path.remove();
        // Bit-identity with the SHIPPED scalar rule, pinned as literals: on
        // this arm `charged == rim` at every bisection step by construction
        // (the branch returns the raw step verbatim), so the gate walks the
        // exact path v1.2.1 walked and lands on the exact bits. A `<= budget`
        // assertion here would be worthless — any tightening satisfies it,
        // which is precisely how the original defect shipped.
        assert_eq!(
            accepted.reading.charged.to_bits(),
            accepted.reading.rim.to_bits(),
            "texture must still charge nothing at the accepted k: {:?}",
            accepted.reading
        );
        assert_eq!(
            (accepted.k, accepted.reading.rim),
            (ARM_B_SHIPPED_K, ARM_B_SHIPPED_RIM),
            "the textured arm must land on the shipped rule's exact bits"
        );

        // Arm C — texture earns AT MOST the ceiling, never more: the same
        // textured neighbourhood at 3x the dial is over the ceiling and
        // shrinks. With A this kills any additive rule (`budget = context +
        // constant`) as surely as A+B kill every scalar: the exchange is a
        // clamped ratio, not an offset.
        let (source, target, mut report, path, geometry, reference, candidate) =
            contextual_fixture(80, ARM_EV * 3.0, Some(ARM_EV * 1.5), "ctx-budget-3x");
        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert!(
            measured.rim > ZONE_BOUNDARY_STEP_MAX,
            "premise: three times the dial is over the ceiling even for texture: {measured:?}"
        );
        let accepted =
            gate(&source, &target, &mut report, &geometry, &reference, candidate)
                .expect("over-ceiling texture is negotiated, not dropped");
        path.remove();
        assert!(accepted.k < 1.0, "texture may buy the ceiling and nothing more: k={}", accepted.k);
    }

    /// R37: the hard family's half of
    /// `a_step_the_target_itself_carries_is_not_charged_as_a_seam`
    /// (fit_zoned.rs). A 128-px frame on purpose: the allowance is pooled on
    /// the 12x8 evidence grid and honoured from eight crossings per cell, and
    /// r2c0's edges on the module's 64-px fixtures put five in a cell. And
    /// +0.30 EV on purpose: half of it still asks four codes at grey 80, where
    /// half of the module's 0.09 EV dial asks one — the floor, a coin flip.
    #[test]
    fn a_tile_edge_the_target_itself_carries_is_not_charged_as_a_seam() {
        const EV: f32 = 0.30;
        const EDGE: u32 = 128;
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(EDGE, EDGE, |_, _| Rgb([80, 80, 80])));
        let mask = GrayImage::from_fn(EDGE, EDGE, |x, y| {
            Luma([if in_tile(TileId { depth: 2, row: 2, col: 0 }, x, y, EDGE, EDGE) { 255 } else { 0 }])
        });
        let path = super::super::tests::fixture_mask_path("r37-hard-asked");
        mask.save(path.path()).unwrap();
        let recipe_at = |ev: f32| {
            let mut recipe = crate::recipe::EditRecipe::default();
            recipe.masks.push(LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
                name: "Spatial tile r2c0".to_string(),
                role: MaskRole::Custom,
                amount: 1.0,
                exposure_ev: ev,
                ..Default::default()
            });
            recipe
        };
        let render_at = |ev: f32| fit::pixels_of(&render::develop_preview(&source, &recipe_at(ev)));
        let reference =
            fit::pixels_of(&render::develop_preview(&source, &crate::recipe::EditRecipe::default()));
        let candidate = render_at(EV);
        let geometry = mask_weights(&mask, EDGE, EDGE);
        let against = |target: Option<&[[f32; 3]]>| {
            boundary_step_toward(target, &reference, &candidate, &candidate, &geometry, EDGE, EDGE)
        };
        let none = against(None);
        assert!(
            none.transitions > 0 && none.charged > ZONE_BOUNDARY_STEP_MAX,
            "premise: a flat field charges this dial over the ceiling: {none:?}"
        );
        let whole = against(Some(&candidate));
        assert_eq!((whole.charged, whole.colour_charged), (0.0, 0.0), "{whole:?}");
        assert_eq!(whole.rim.to_bits(), none.rim.to_bits(), "the raw step is a reading, not a charge");
        assert!((whole.asked - whole.rim).abs() <= 1e-3, "{whole:?}");
        let untouched = against(Some(&reference));
        assert_eq!(untouched.charged.to_bits(), none.charged.to_bits(), "{untouched:?}");
        let opposite = render_at(-EV);
        assert_eq!(against(Some(&opposite)).charged.to_bits(), none.charged.to_bits());
        let half = render_at(EV * 0.5);
        let half_asked = against(Some(&half));
        assert!(half_asked.charged > 0.0 && half_asked.charged < none.charged, "{half_asked:?}");
        // A target whose boundary is a coin flip — the re-synthesised
        // texture's case — allows nothing: a three-code step that changes
        // sign from pixel to pixel along the whole contour.
        let flip: Vec<[f32; 3]> = reference
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (x, y) = (i % EDGE as usize, i / EDGE as usize);
                let sign = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
                p.map(|c| (c + sign * 3.0 / 255.0).clamp(0.0, 1.0))
            })
            .collect();
        let coin = against(Some(&flip));
        assert_eq!((coin.charged.to_bits(), coin.asked), (none.charged.to_bits(), 0.0), "{coin:?}");

        // The gate. The frame gate is given the candidate itself so it
        // improves with every k and cannot decide these arms.
        let gate = |target: Option<&[[f32; 3]]>| -> (f32, f32, f32) {
            let target_image = render::develop_preview(&source, &recipe_at(EV));
            let mut report = super::super::tests::neutral_report(&source, &target_image);
            report.recipe = recipe_at(EV);
            let frame_before = fit::look_err_with_evidence(&reference, &candidate, &report.evidence);
            let accepted = enforce_bitmap_boundary(
                &source,
                &candidate,
                &mut report,
                0,
                BitmapBoundaryInput {
                    target_boundary: target,
                    ruler: BoundaryRuler::CrossBoundaryStep { geometry: &geometry, reference: &reference },
                    initial_px: candidate.clone(),
                    frame_before,
                },
            )
            .unwrap_or_else(|refusal| panic!("negotiated, not dropped: {:?}", refusal.why));
            (accepted.k, accepted.reading.charged, accepted.reading.asked)
        };
        let (k_none, _, asked_none) = gate(None);
        let (k_whole, charged_whole, asked_whole) = gate(Some(&candidate));
        let (k_half, ..) = gate(Some(&half));
        let (k_opposite, ..) = gate(Some(&opposite));
        assert_eq!((k_whole, charged_whole), (1.0, 0.0), "the target's own edge is kept whole");
        assert!(asked_whole > 0.0 && asked_none == 0.0);
        assert_eq!(k_opposite.to_bits(), k_none.to_bits());
        assert!(k_none < k_half && k_half < 1.0, "none {k_none} half {k_half}");
        path.remove();
    }

    /// Captured from a run of the persistence rule on `shoulder_fixture`
    /// (arm F below says what moving them means).
    const ARM_F_K: f32 = 0.24536133;
    const ARM_F_RIM: f32 = 0.007843137;

    /// Acceptance (v1.2.2 seam batch, second finding of the same class):
    /// slope credit must PERSIST past the first baseline. Measured on the
    /// real seam: the resample-and-refine collar spans exactly the first
    /// baseline out, so a hard tile edge in CLEAN sky read an inner |u1|
    /// slope of 0.005-0.008 luma, and three times that bought the whole
    /// ceiling back for a 43-crossing sky band. The soft shoulder a seam
    /// wears must never fund the seam.
    #[test]
    fn a_ramp_earns_budget_only_where_it_persists_past_the_collar() {
        let gate = |source: &DynamicImage,
                    target: &[[f32; 3]],
                    report: &mut FitReport,
                    geometry: &[f32],
                    reference: &[[f32; 3]],
                    candidate: Vec<[f32; 3]>| {
            let frame_before = fit::look_err_with_evidence(reference, target, &report.evidence);
            enforce_bitmap_boundary(
                source,
                target,
                report,
                0,
                BitmapBoundaryInput {
                    target_boundary: None,
                    ruler: BoundaryRuler::CrossBoundaryStep { geometry, reference },
                    initial_px: candidate,
                    frame_before,
                },
            )
        };

        // Arm E: the collar shape. Alpha completes its rise within ONE
        // baseline, so the inner baseline reads a steep slope and the
        // second reads none. The minimum refuses the credit; the budget
        // floors; the dial shrinks. Reading the inner baseline alone
        // frees exactly the measured seam.
        let (source, target, mut report, path, geometry, reference, candidate) =
            shoulder_fixture(4.0, 0.09, 0.045, "collar-shoulder");
        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert!(
            measured.rim > BOUNDARY_STEP_FLOOR && measured.rim <= ZONE_BOUNDARY_STEP_MAX,
            "premise: the raw step sits inside the old scalar budget: {measured:?}"
        );
        assert!(
            measured.charged > ZONE_BOUNDARY_STEP_MAX,
            "a collar shoulder must not fund its own step: {measured:?}"
        );
        let accepted =
            gate(&source, &target, &mut report, &geometry, &reference, candidate)
                .expect("a collar-shouldered step is negotiated down, not dropped");
        path.remove();
        assert!(
            accepted.k < 1.0,
            "the shipped rule free-passed this shape; the persistence rule must shrink: k={}",
            accepted.k
        );
        assert!(
            accepted.reading.rim <= BOUNDARY_STEP_FLOOR + 1e-6,
            "with no persistent slope the budget is the floor: {:?}",
            accepted.reading
        );

        // Arm F: the same 0.55 step carrying a ramp that PERSISTS (to 1.0
        // over 32 px), at a dial whose earned budget clears two code values
        // so the 8-bit kept step can land strictly between floor and ceiling
        // and stays under three, so every mutation lands on a DIFFERENT
        // code (at 0.30 EV the budget fell between one and two codes and the
        // quantised step could only land on the floor; at 0.45 EV it cleared
        // three codes and a saturated SHAPE would land on the same step).
        // Consecutive
        // baselines agree, so the correction earns
        // three times its own slope and NO MORE. `k` and the kept reading
        // are pinned bit-for-bit: a mutation that reads the slope off the
        // render under bisection (the budget would chase k), deletes the
        // term, or saturates it (SHAPE = infinity) each lands on different
        // bits.
        let (source, target, mut report, path, geometry, reference, candidate) =
            shoulder_fixture(32.0, 0.37, 0.185, "persistent-ramp");
        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert!(
            measured.rim > ZONE_BOUNDARY_STEP_MAX,
            "premise: this dial is over even the ceiling: {measured:?}"
        );
        let accepted =
            gate(&source, &target, &mut report, &geometry, &reference, candidate)
                .expect("a persistent ramp earns its slope budget and shrinks to it");
        path.remove();
        assert_eq!(
            (accepted.k, accepted.reading.rim),
            (ARM_F_K, ARM_F_RIM),
            "the slope-earned budget must land on these exact bits"
        );
        assert!(
            accepted.reading.rim > BOUNDARY_STEP_FLOOR && accepted.reading.rim < ZONE_BOUNDARY_STEP_MAX,
            "the earned budget sits strictly between floor and ceiling: {:?}",
            accepted.reading
        );
    }

    /// The soft family's charge (2026-09-10) touches the SOFT family only,
    /// and the colour ruler that landed with it is ADDITIVE on this one.
    ///
    /// Both rulers are run over the same hard raster. The step ruler keeps
    /// its crossing count and its two luma ranks; the rim ruler still reads
    /// nothing at all, because a 0/255 raster has no band; and on a NEUTRAL
    /// frame under a pure exposure dial the per-channel ranks ARE the luma
    /// ranks — which is why every bit-pinned verdict in this module that runs
    /// on a grey fixture (`ARM_B_SHIPPED_K`, `ARM_F_K`, `GRADIENT_K`) is
    /// decided by exactly the numbers it was decided by before.
    #[test]
    fn the_hard_familys_readings_are_untouched_by_the_soft_familys_charge() {
        let (_, _, _, path, geometry, reference, candidate) =
            boundary_fixture(-0.40, Some(-0.20), "hard-family-unmoved");
        let blind = boundary_rim(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert_eq!(
            (blind.transitions, blind.rim, blind.charged, blind.colour, blind.colour_charged),
            (0, 0.0, 0.0, 0.0, 0.0),
            "the two rulers stay separate: a 0/255 raster still has no band to read: {blind:?}"
        );
        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert_eq!(
            measured.transitions, 48,
            "the same 48 crossings this module has always counted: {measured:?}"
        );
        assert!(
            (measured.colour - measured.rim).abs() <= 1e-6
                && (measured.colour_charged - measured.charged).abs() <= 1e-6,
            "R = G = B under a pure exposure dial makes the colour ruler the luma ruler: \
             {measured:?}"
        );
        path.remove();
    }

    /// Acceptance 1 (2026-08-30): a hard 0/255 mask now produces a REAL
    /// reading, and a boundary that cannot be sampled is refused instead of
    /// passing on an empty one. Before the fix both halves were the same
    /// number — `rim 0.000` off `0` transitions — and it counted as a pass.
    #[test]
    fn hard_tile_mask_yields_a_measured_cross_boundary_step() {
        let (_, _, _, path, geometry, reference, candidate) =
            boundary_fixture(-0.40, Some(-0.20), "tile-boundary-hard-reading");
        let blind = boundary_rim(&reference, &candidate, &candidate, &geometry, 64, 64);
        assert_eq!((blind.rim, blind.transitions), (0.0, 0), "the defect, pinned: {blind:?}");

        let measured = boundary_step(&reference, &candidate, &candidate, &geometry, 64, 64);
        // r2c0 of a 4x4 grid on 64x64 is cols 0..16, rows 32..48. Its left
        // edge is the frame edge, so the crossings are: 16 rows x 1 (right
        // edge) + 16 columns x 2 (top and bottom edges) = 48. Pinned exactly,
        // because "some crossings" is what a broken sampler also reports.
        assert_eq!(
            measured.transitions, 48,
            "every edge of the rectangle inside the frame must be sampled: {measured:?}"
        );
        assert!(
            measured.rim > 4.0 * ZONE_BOUNDARY_STEP_MAX,
            "the seam this tile makes is several times over budget: {measured:?}"
        );

        // The same correction measured against ITSELF introduces no step: the
        // difference in differences is zero when nothing changed, so scene
        // content at the border can never be blamed on the correction.
        let quiet = boundary_step(&candidate, &candidate, &candidate, &geometry, 64, 64);
        assert_eq!(quiet.rim, 0.0, "an unchanged render has no induced step: {quiet:?}");
        assert!(quiet.transitions > 0, "and it is still measured, not skipped");
        path.remove();
    }

    /// A mask with no sampleable contour is REFUSED. The old gate's "pass"
    /// was built on exactly this reading.
    /// Step 9, scope addition: the SAME rule on the bitmap gate that tiles,
    /// free masks and semantic regions share. A candidate whose accepted
    /// render is byte-identical to the frame without it is refused, so the
    /// tile sweep takes its `owned.remove()` / `refused.insert()` /
    /// `continue` path — the raster goes, the area stays available to the
    /// next candidate (`excluded` is only accumulated on the attach path at
    /// spatial.rs), and the disclosure is the refusal note rather than
    /// TILE_ATTACHED with a before/after pair nothing produced.
    /// Supervisor mutation M-4-B (the check deleted) goes red here.
    #[test]
    fn an_inert_bitmap_correction_is_refused_rather_than_attached() {
        let (source, target, mut report, path, geometry, reference, _candidate) =
            boundary_fixture(-0.40, Some(-0.20), "tile-boundary-inert");
        let frame_before =
            fit::look_err_with_evidence(&reference, &target, &report.evidence);
        let outcome = enforce_bitmap_boundary(
            &source,
            &target,
            &mut report,
            0,
            BitmapBoundaryInput {
                target_boundary: None,
                ruler: BoundaryRuler::CrossBoundaryStep {
                    geometry: &geometry,
                    reference: &reference,
                },
                // The candidate render IS the reference: nothing moved.
                initial_px: reference.clone(),
                frame_before,
            },
        );
        let Err(refusal) = outcome else {
            panic!("a correction that moves no pixel may not attach");
        };
        assert_eq!(refusal.why, BitmapBoundaryWhy::Inert);
        assert!(report.recipe.masks.is_empty(), "and its mask must not survive");
        path.remove();
    }

    #[test]
    fn unmeasurable_boundary_is_refused_never_passed() {
        let (source, target, mut report, path, _, reference, candidate) =
            boundary_fixture(-0.40, Some(-0.20), "tile-boundary-unmeasurable");
        // A geometry that is entirely inside the mask has no 50% contour, so
        // no pair can be placed on it.
        let geometry = vec![1.0f32; reference.len()];
        let result = enforce_bitmap_boundary(
            &source,
            &target,
            &mut report,
            0,
            BitmapBoundaryInput {
                target_boundary: None,
                ruler: BoundaryRuler::CrossBoundaryStep {
                    geometry: &geometry,
                    reference: &reference,
                },
                initial_px: candidate,
                frame_before: 1.0,
            },
        );
        // Matched rather than `expect_err`: the Ok payload carries a full
        // analysis-size pixel buffer and must never be Debug-printed.
        let Err(refusal) = result else {
            panic!("an unmeasurable boundary may not be passed");
        };
        assert_eq!(refusal.why, BitmapBoundaryWhy::Unmeasured);
        assert_eq!(refusal.initial.transitions, 0);
        assert!(report.recipe.masks.is_empty(), "the refused correction must be removed");
        path.remove();
    }

    #[test]
    fn refined_mask_is_rechecked_by_rim_and_frame_gates() {
        let (source, target, mut report, path, geometry, reference, candidate) =
            boundary_fixture(0.25, None, "tile-refined-recheck");
        let result = enforce_bitmap_boundary(
            &source,
            &target,
            &mut report,
            0,
            BitmapBoundaryInput {
                target_boundary: None,
                ruler: BoundaryRuler::CrossBoundaryStep {
                    geometry: &geometry,
                    reference: &reference,
                },
                initial_px: candidate,
                frame_before: 0.0,
            },
        );
        assert!(result.is_err(), "a refined alpha cannot bypass composed-frame arbitration");
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
        let reading = read_tile_uncached(id, &sp, &tp, &evidence);
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

    /// A38 (v1.2.4). The scoped-evidence cache: one recompute per NODE, not
    /// one per node per generation, and the same bytes either way.
    ///
    /// The traversal re-reads the whole visited set once per generation, and
    /// everything in a reading except the residual is a function of the frozen
    /// evidence model and the tile's own geometry — a full-frame
    /// `scoped_mask_evidence` and a `structure_divergence` per node per pass,
    /// none of which could have changed. On the calibration pair the counters
    /// below say what that was costing.
    ///
    /// The cache is keyed by `TileId` and holds an `Rc`, so a hit cannot
    /// return a different reading than a miss; the second assertion pins that
    /// the cached traversal attaches exactly what the uncached reader sees.
    ///
    /// MUTATION (2026-09-02): make `tile_evidence` skip its cache lookup
    /// (`if false { ... }`) and reads == computes, which fails here while
    /// every other tile test stays green.
    #[test]
    fn the_tile_evidence_cache_recomputes_each_node_once() {
        let Some(root) = fit::calibration_corpus() else { return };
        let source = image::open(root.join("neutral.jpg")).unwrap();
        let target = image::open(root.join("target.jpg")).unwrap();
        let mut report = super::super::tests::neutral_report(&source, &target);
        let dir = std::env::temp_dir()
            .join(format!("autoshade-tile-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let home = crate::store::OwnedRaster::scratch(dir.join("tile.png"));
        TILE_EVIDENCE_READS.with(|reads| reads.set(0));
        TILE_EVIDENCE_COMPUTES.with(|computes| computes.set(0));
        let excluded = attach_tiles(&source, &target, &mut report, &home, false, 2);
        let reads = TILE_EVIDENCE_READS.with(std::cell::Cell::get);
        let computes = TILE_EVIDENCE_COMPUTES.with(std::cell::Cell::get);
        std::fs::remove_dir_all(&dir).ok();
        eprintln!("TILE_EVIDENCE reads={reads} computes={computes}");
        assert!(
            reads > computes,
            "the traversal must re-read nodes it has already scoped: {reads} vs {computes}",
        );
        // 21 nodes is the whole quadtree above the leaf cap (1 + 4 + 16), so a
        // recompute count at or under it is one pass over the tree however
        // many generations the attachment cap allows.
        assert!(
            computes <= 21,
            "each node is scoped once per fit, not once per generation: {computes}",
        );
        assert!(
            excluded.iter().any(|alpha| *alpha > 0.0),
            "premise: this pair attaches a tile",
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
        let strong = read_tile_uncached(strong_id, &current, &target_pixels, &evidence);
        let parent = read_tile_uncached(
            TileId { depth: 1, row: 1, col: 0 },
            &current,
            &target_pixels,
            &evidence,
        );
        assert_eq!(eligible(&strong, parent.residual), Ok(()), "{strong:?}");
        for col in 0..4 {
            let sky = read_tile_uncached(
                TileId { depth: 2, row: 0, col },
                &current,
                &target_pixels,
                &evidence,
            );
            let sky_parent = read_tile_uncached(
                TileId { depth: 1, row: 0, col: col / 2 },
                &current,
                &target_pixels,
                &evidence,
            );
            assert!(eligible(&sky, sky_parent.residual).is_err(), "changed sky became a tile: {sky:?}");
        }
    }
}
