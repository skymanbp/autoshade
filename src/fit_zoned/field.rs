//! Read-only consumers of the analysis-only local field.

use image::DynamicImage;

use crate::fit::{self, FitReport};
use crate::fit_field::{smooth_3tap_luma, LocalField};
use crate::render;
use super::range::{RANGE_MAX_BANDS, RANGE_MIN_EVIDENCE_SHARE};
use super::spatial::SPATIAL_MAX_ATTACHMENTS;

/// Phase-A sweeps put spatially uniform edits at at most 9.2/255 (including
/// sparse, just-supported vertices), structured edits at 21.9-51.8/255, and
/// calibration mid-tones at 28.7-29.1/255.  The fixed 15/255 line is in that gap.
pub(super) const BAND_DISPERSION_MAX: f32 = 15.0 / 255.0;
pub(super) const TILE_SHAPE_MIN: f32 = 0.5;
const LINEAR_SHAPE_MIN: f32 = 0.6;
const BAND_MERGE_STEP: f32 = 2.0 / 255.0;
pub(super) const LOCAL_STOP_MARGIN: f32 = 0.002;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FieldShape { BandShaped, TileShaped, FreeForm, Linear, None }
impl FieldShape {
    fn label(self) -> &'static str {
        match self {
            Self::BandShaped => "band_shaped",
            Self::TileShaped => "tile_shaped",
            Self::FreeForm => "free_form",
            Self::Linear => "linear",
            Self::None => "none",
        }
    }
}
/// A luma span `[lo, hi)` of the CURRENT render (the field's guide domain) over
/// which the band marginal reads one uniform correction of sign `sign`. The range
/// producer maps it into its own evidence-bin domain (the ORIGINAL source luma)
/// through the pixels that actually occupy the span; the field never names an
/// evidence bin itself, because after a global tone move the two domains differ.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FieldBandProposal {
    pub(super) lo: f32,
    pub(super) hi: f32,
    pub(super) sign: f32,
}
#[derive(Clone, Debug)]
pub(super) struct ShapeReading {
    pub(super) r2_tiles: f32,
    pub(super) r2_linear: f32,
    pub(super) structured_bins: Vec<usize>,
    pub(super) shape: FieldShape,
    pub(super) effective_tile_cap: usize,
    pub(super) proposals: Vec<FieldBandProposal>,
}
fn effect(parameters: &[f32; 5], luma: f32) -> f32 {
    std::f32::consts::LN_2 * luma * parameters[0]
        + luma * (0.299 * parameters[1] + 0.587 * parameters[2] + 0.114 * parameters[3])
}
fn field_span(bin: usize) -> (f32, f32) {
    (((bin as f32 - 0.5) / 7.0).max(0.0), ((bin as f32 + 0.5) / 7.0).min(1.0))
}
fn in_span(value: f32, lo: f32, hi: f32) -> bool {
    value >= lo && (value < hi || hi == 1.0 && value <= hi)
}
#[derive(Clone)]
struct Candidate { first_bin: usize, last_bin: usize, value: f32 }
fn band_proposals(
    field: &LocalField,
    current: &[[f32; 3]],
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
) -> Vec<FieldBandProposal> {
    let (width, height) = (field.width as usize, field.height as usize);
    // One luma ruler for both shares: the field's guide domain on both frames.
    let guide = smooth_3tap_luma(current, width, height);
    let target_guide = smooth_3tap_luma(target, width, height);
    let source_total = evidence.source_weights.iter().sum::<f32>().max(1e-12);
    let target_total = evidence.target_weights.iter().sum::<f32>().max(1e-12);
    let mut raw = Vec::<Candidate>::new();
    // Bin 0 is deliberately blind: every term in the delta's luma effect is
    // multiplied by c=0, so it can neither prove uniformity nor move pure black.
    for bin in 1..8 {
        if field.band_dispersion[bin] > BAND_DISPERSION_MAX { continue; }
        let (lo, hi) = field_span(bin);
        let source_share = guide.iter().zip(&evidence.source_weights)
            .filter(|(luma, _)| in_span(**luma, lo, hi)).map(|(_, w)| *w).sum::<f32>()
            / source_total;
        let target_share = target_guide.iter().zip(&evidence.target_weights)
            .filter(|(luma, _)| in_span(**luma, lo, hi)).map(|(_, w)| *w).sum::<f32>()
            / target_total;
        let value = effect(&field.band_marginal[bin], bin as f32 / 7.0);
        if source_share < RANGE_MIN_EVIDENCE_SHARE
            || target_share < RANGE_MIN_EVIDENCE_SHARE
            || value.abs() < BAND_MERGE_STEP
        {
            continue;
        }
        if let Some(previous) = raw.last_mut() {
            let boundary = (bin as f32 - 0.5) / 7.0;
            let delta = std::array::from_fn(|p| {
                field.band_marginal[bin][p] - field.band_marginal[previous.last_bin][p]
            });
            if bin == previous.last_bin + 1 && effect(&delta, boundary).abs() < BAND_MERGE_STEP {
                previous.last_bin = bin;
                if value.abs() > previous.value.abs() { previous.value = value; }
                continue;
            }
        }
        raw.push(Candidate { first_bin: bin, last_bin: bin, value });
    }
    raw.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs())
        .then_with(|| a.first_bin.cmp(&b.first_bin)));
    raw.truncate(RANGE_MAX_BANDS);
    raw.sort_by_key(|candidate| candidate.first_bin);
    // Field bins partition [0, 1], so two surviving candidates never overlap:
    // neighbours that did not merge share one boundary value at most.
    raw.into_iter().map(|candidate| FieldBandProposal {
        lo: field_span(candidate.first_bin).0,
        hi: field_span(candidate.last_bin).1,
        sign: candidate.value,
    }).filter(|proposal| proposal.lo < proposal.hi).collect()
}
/// Weighted least-squares plane `a + b*x + c*y` over the remainder; `weights`
/// are the solve's per-pixel fit weights, so unmeasured pixels carry nothing.
fn solve_plane(
    values: &[f32], weights: &[f32], width: usize, height: usize,
) -> Option<(Vec<f64>, f64)> {
    let mut normal = [[0.0f64; 4]; 3];
    for (i, (&value, &weight)) in values.iter().zip(weights).enumerate() {
        let x = if width > 1 { (i % width) as f64 / (width - 1) as f64 } else { 0.0 };
        let y = if height > 1 { (i / width) as f64 / (height - 1) as f64 } else { 0.0 };
        let row = [1.0, x, y];
        let weight = weight.max(0.0) as f64;
        for r in 0..3 {
            for c in 0..3 { normal[r][c] += weight * row[r] * row[c]; }
            normal[r][3] += weight * row[r] * value as f64;
        }
    }
    for pivot in 0..3 {
        let best = (pivot..3).max_by(|&a, &b| normal[a][pivot].abs()
            .total_cmp(&normal[b][pivot].abs()))?;
        normal.swap(pivot, best);
        if normal[pivot][pivot].abs() < 1e-12 { return None; }
        let scale = normal[pivot][pivot];
        for value in &mut normal[pivot][pivot..] { *value /= scale; }
        let pivot_row = normal[pivot];
        for (row_index, row) in normal.iter_mut().enumerate() {
            if row_index == pivot { continue; }
            let scale = row[pivot];
            for (value, base) in row[pivot..].iter_mut().zip(&pivot_row[pivot..]) {
                *value -= scale * base;
            }
        }
    }
    let coefficients = [normal[0][3], normal[1][3], normal[2][3]];
    let predicted = values.iter().enumerate().map(|(i, _)| {
        let x = if width > 1 { (i % width) as f64 / (width - 1) as f64 } else { 0.0 };
        let y = if height > 1 { (i / width) as f64 / (height - 1) as f64 } else { 0.0 };
        coefficients[0] + coefficients[1] * x + coefficients[2] * y
    }).collect::<Vec<_>>();
    Some((predicted, coefficients[0]))
}
/// `(R2 tiles, R2 linear, has_variance)` of the remainder, every sum weighted
/// by the solve's per-pixel fit weight: pixels the field never measured (zero
/// evidence, structural divergence, clipping, occupancy-floor vertices) do not
/// vote on the remainder's shape.
fn shape_metrics(field: &LocalField) -> (f32, f32, bool) {
    let (width, height) = (field.width as usize, field.height as usize);
    let n = field.remainder.len();
    if width * height != n || n == 0 || field.weight.len() != n { return (0.0, 0.0, false); }
    let weights = field.weight.iter().map(|&w| w.max(0.0) as f64).collect::<Vec<_>>();
    let mass = weights.iter().sum::<f64>();
    if mass <= 0.0 { return (0.0, 0.0, false); }
    let mean = field.remainder.iter().zip(&weights).map(|(&v, w)| w * v as f64).sum::<f64>() / mass;
    let total = field.remainder.iter().zip(&weights)
        .map(|(&v, w)| w * (v as f64 - mean).powi(2)).sum::<f64>();
    if total < 1e-12 { return (0.0, 0.0, false); }
    let predicted = solve_plane(&field.remainder, &field.weight, width, height)
        .map(|(predicted, _)| predicted).unwrap_or_else(|| vec![mean; n]);
    let linear_sse = field.remainder.iter().zip(&predicted).zip(&weights)
        .map(|((&v, &p), w)| w * (v as f64 - p).powi(2)).sum::<f64>();
    let r2_linear = (1.0 - linear_sse / total).clamp(0.0, 1.0);
    let residual = field.remainder.iter().zip(&predicted)
        .map(|(&v, &p)| v as f64 - p).collect::<Vec<_>>();
    // Once a plane earns the linear verdict, tile shape is its INCREMENTAL
    // share; otherwise it is the ordinary 4x4-means R2. This prevents a smooth
    // plane (94% captured by coarse means) from masquerading as tile-shaped.
    let tiled = if r2_linear >= LINEAR_SHAPE_MIN as f64 { residual }
        else { field.remainder.iter().map(|&v| v as f64 - mean).collect() };
    let tile_of = |i: usize| (i / width * 4 / height) * 4 + (i % width * 4 / width);
    let mut sums = [0.0f64; 16];
    let mut tile_mass = [0.0f64; 16];
    for (i, (&value, w)) in tiled.iter().zip(&weights).enumerate() {
        sums[tile_of(i)] += w * value;
        tile_mass[tile_of(i)] += w;
    }
    let tile_explained = (0..16)
        .filter(|&tile| tile_mass[tile] > 0.0)
        .map(|tile| sums[tile].powi(2) / tile_mass[tile])
        .sum::<f64>();
    ((tile_explained / total).clamp(0.0, 1.0) as f32, r2_linear as f32, true)
}
fn read_shape(
    field: &LocalField, current: &[[f32; 3]], target: &[[f32; 3]], evidence: &fit::EvidenceModel,
) -> ShapeReading {
    let proposals = band_proposals(field, current, target, evidence);
    let structured_bins = (1..8)
        .filter(|&bin| field.band_dispersion[bin] > BAND_DISPERSION_MAX).collect::<Vec<_>>();
    let (r2_tiles, r2_linear, has_variance) = shape_metrics(field);
    let effective_tile_cap = if r2_tiles < TILE_SHAPE_MIN { 2 } else { SPATIAL_MAX_ATTACHMENTS };
    let shape = if !proposals.is_empty() && r2_tiles < TILE_SHAPE_MIN {
        FieldShape::BandShaped
    } else if r2_linear >= LINEAR_SHAPE_MIN {
        FieldShape::Linear
    } else if r2_tiles >= TILE_SHAPE_MIN {
        FieldShape::TileShaped
    } else if has_variance {
        FieldShape::FreeForm
    } else {
        FieldShape::None
    };
    ShapeReading { r2_tiles, r2_linear, structured_bins, shape, effective_tile_cap, proposals }
}
#[cfg(test)]
thread_local! {
    pub(super) static FIELD_CEILING_OVERRIDE: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    pub(super) static FIELD_FORCE_NONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn solve_local_field(
    src: &DynamicImage, target: &DynamicImage, report: &mut FitReport,
) -> Option<(LocalField, ShapeReading)> {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let current = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let target_pixels = fit::pixels_of(&t_img);
    #[allow(unused_mut)]
    let mut field = LocalField::solve(
        &current, &target_pixels, s_img.width(), s_img.height(), &report.evidence,
    )?;
    // The forced-None override fires AFTER the solve so the byte-identity test
    // proves the analysis itself leaves the report untouched, not merely that an
    // early return does.
    #[cfg(test)]
    if FIELD_FORCE_NONE.with(|value| value.replace(false)) { return None; }
    // Test override = headroom BELOW the measured `global`, so a forced stop
    // still satisfies the "ceiling actually beat the frame" guard.
    #[cfg(test)]
    FIELD_CEILING_OVERRIDE.with(|value| {
        if let Some(headroom) = value.take() { field.ceiling = field.global - headroom; }
    });
    let reading = read_shape(&field, &current, &target_pixels, &report.evidence);
    // Measured, not assumed: the share is 0.000 only when the producer-free
    // frame `report.err_after` and the field's own `global` agree, and a ruler
    // mismatch between the two would show here instead of being papered over.
    let realized = realized_share(field.global, field.ceiling, report.err_after)
        .map_or_else(|| "n/a".to_string(), |value| format!("{value:.3}"));
    crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
        crate::rationale::Note::new(crate::rationale::keys::LOCAL_CEILING, vec![
            ("global", format!("{:.6}", field.global)), ("ceiling", format!("{:.6}", field.ceiling)),
            ("realized", realized), ("saturated", field.saturated.to_string()),
            ("iterations", field.solve.iterations.to_string()),
        ]));
    let bins = if reading.structured_bins.is_empty() { "0:blind".to_string() }
        else { format!("0:blind,{}", reading.structured_bins.iter().map(usize::to_string).collect::<Vec<_>>().join(",")) };
    crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
        crate::rationale::Note::new(crate::rationale::keys::LOCAL_SHAPE, vec![
            ("r2_tiles", format!("{:.3}", reading.r2_tiles)),
            ("r2_linear", format!("{:.3}", reading.r2_linear)), ("shape", reading.shape.label().into()),
            ("cap", reading.effective_tile_cap.to_string()), ("structured", bins),
        ]));
    // Bin 0 is named once in LOCAL_SHAPE (`0:blind`); only measured
    // structured bins earn a skip note of their own.
    for &bin in &reading.structured_bins {
        crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(crate::rationale::keys::LOCAL_BAND_SKIPPED, vec![
                ("bin", bin.to_string()),
                ("dispersion", format!("{:.2}", field.band_dispersion[bin] * 255.0)),
                ("max", format!("{:.0}", BAND_DISPERSION_MAX * 255.0)),
            ]));
    }
    Some((field, reading))
}

pub(super) fn realized_share(global: f32, ceiling: f32, err_after: f32) -> Option<f32> {
    let possible = global - ceiling;
    (possible > 1e-6).then_some((global - err_after) / possible)
}

/// Only a ceiling that actually beat the producer-free frame can end the fit: a
/// field that saturated or regularised its way ABOVE `global` measured nothing
/// about what is left to win, so it never vetoes the tile producer.
pub(super) fn stop_verdict(field: &LocalField, err_after: f32) -> bool {
    field.ceiling < field.global && err_after - field.ceiling <= LOCAL_STOP_MARGIN
}

pub(super) fn push_realized(report: &mut FitReport, field: &LocalField, producer: &str) {
    let realized = realized_share(field.global, field.ceiling, report.err_after)
        .map_or_else(|| "n/a".to_string(), |value| format!("{value:.3}"));
    crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
        crate::rationale::Note::new(crate::rationale::keys::LOCAL_REALIZED, vec![
            ("producer", producer.into()), ("err_after", format!("{:.6}", report.err_after)),
            ("ceiling", format!("{:.6}", field.ceiling)), ("realized", realized),
        ]));
}

pub(super) fn push_stop(report: &mut FitReport, producer: &str, skipped: &str) {
    crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
        crate::rationale::Note::new(crate::rationale::keys::LOCAL_STOP, vec![
            ("producer", producer.into()), ("skipped", skipped.into()),
            ("margin", format!("{LOCAL_STOP_MARGIN:.3}")),
        ]));
}

#[cfg(test)]
mod tests;
