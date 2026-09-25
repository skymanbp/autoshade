//! Read-only consumers of the analysis-only local field.

use image::DynamicImage;

use crate::fit::{self, FitReport};
use crate::fit_field::{smooth_3tap_luma, FieldSolveOpts, LocalField};
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
    let Some(mut field) = LocalField::solve(
        &current, &target_pixels, s_img.width(), s_img.height(), &report.evidence,
    ) else {
        note(report, crate::rationale::keys::FIELD_UNSOLVED, vec![("stage", "analysis".into())]);
        return None;
    };
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
    note(report, crate::rationale::keys::LOCAL_CEILING, vec![
        ("global", format!("{:.6}", field.global)), ("ceiling", format!("{:.6}", field.ceiling)),
        ("realized", realized), ("saturated", field.saturated.to_string()),
        ("iterations", field.solve.iterations.to_string()),
    ]);
    let bins = if reading.structured_bins.is_empty() { "0:blind".to_string() }
        else { format!("0:blind,{}", reading.structured_bins.iter().map(usize::to_string).collect::<Vec<_>>().join(",")) };
    note(report, crate::rationale::keys::LOCAL_SHAPE, vec![
        ("r2_tiles", format!("{:.3}", reading.r2_tiles)),
        ("r2_linear", format!("{:.3}", reading.r2_linear)), ("shape", reading.shape.label().into()),
        ("cap", reading.effective_tile_cap.to_string()), ("structured", bins),
    ]);
    // Bin 0 is named once in LOCAL_SHAPE (`0:blind`); only measured
    // structured bins earn a skip note of their own.
    for &bin in &reading.structured_bins {
        note(report, crate::rationale::keys::LOCAL_BAND_SKIPPED, vec![
            ("bin", bin.to_string()),
            ("dispersion", format!("{:.2}", field.band_dispersion[bin] * 255.0)),
            ("max", format!("{:.0}", BAND_DISPERSION_MAX * 255.0)),
        ]);
    }
    Some((field, reading))
}

/// ONE line for this module's disclosures. Every field note is the same
/// `push_note(rationale, notes, Note::new(key, args))` triple, and eight copies
/// of it were eight places for the rationale string and the note list to drift
/// apart.
fn note(report: &mut FitReport, key: &'static str, args: Vec<(&'static str, String)>) {
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(key, args),
    );
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
    note(report, crate::rationale::keys::LOCAL_REALIZED, vec![
        ("producer", producer.into()), ("err_after", format!("{:.6}", report.err_after)),
        ("ceiling", format!("{:.6}", field.ceiling)), ("realized", realized),
    ]);
}

/// R33 §G. The field as a SHIPPED control, and the last producer in the
/// sequencer — after ranges, tiles and free masks, because it exists to carry
/// the residual none of them could reach.
///
/// Two gates, and the first is the strength dial. Below
/// [`GradeStrength::DEFAULT`] the field stays exactly what it has always been,
/// an analysis instrument, and this function returns without touching the
/// recipe: the dial means "how far past Lightroom may this fit go", and the
/// colour field is the first control that leaves Lightroom entirely — a
/// sidecar cannot carry a coordinate system it has no keys for. R41
/// (2026-09-22) moved the gate down from "above the default" to "at the
/// default and above": on the reference pair the default fit carried no
/// field at all, and that was most of the distance between 0.65 and 0.85 the
/// user saw; the calibration point one click below the default still stays
/// inside what a sidecar can carry. The second is
/// the field's own ceiling: it may only run when there is measurably more than
/// [`LOCAL_STOP_MARGIN`] still on the table, which is the same verdict
/// [`stop_verdict`] uses to END the sequencer, read the other way round.
///
/// The field is RE-SOLVED here rather than reused. The one from the head of
/// the sequencer measured the producer-free frame; every producer since has
/// moved pixels, and a field solved against a render that no longer exists
/// would spend its budget re-doing work already attached.
///
/// Finally, do-no-harm: the attached field is kept only if the frame it
/// renders is closer to the target than the frame without it. A field that
/// regresses the frame is dropped and said so.
pub(super) fn attach_colour_field(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    strength: crate::recipe::GradeStrength,
    entry: &LocalField,
    layout: &[f32],
) {
    if strength.get() < crate::recipe::GradeStrength::DEFAULT {
        return;
    }
    if !stop_verdict_has_headroom(entry, report.err_after) {
        note(report, crate::rationale::keys::FIELD_WITHHELD, vec![
            ("ceiling", format!("{:.6}", entry.ceiling)),
            ("err_after", format!("{:.6}", report.err_after)),
            ("margin", format!("{LOCAL_STOP_MARGIN:.3}")),
        ]);
        return;
    }
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let current = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let target_pixels = fit::pixels_of(&t_img);
    let (w, h) = (s_img.width(), s_img.height());
    let Some(pass_a) = LocalField::solve(&current, &target_pixels, w, h, &report.evidence)
    else {
        note(report, crate::rationale::keys::FIELD_UNSOLVED, vec![("stage", "attach".into())]);
        return;
    };
    let budget = fit::FitBudget::for_strength(strength);
    let (solved, admitted, read, paired) =
        admit_cells(&current, &target_pixels, w, h, report, pass_a, budget.field_gain, layout);
    // Rendered from the field the recipe will CARRY — rounded the way
    // `ColourField::round` stores it — so the do-no-harm verdicts and
    // `FIELD_ATTACHED`'s numbers describe the render a reopen produces.
    let mut field = solved.as_recipe_field();
    field.round();
    let after = {
        let mut out = current.clone();
        render::apply_colour_field(&mut out, w as usize, h as usize, Some(&field));
        out
    };
    let err_with = fit::look_err_with_evidence(&after, &target_pixels, &report.evidence);
    if err_with >= report.err_after {
        note(report, crate::rationale::keys::FIELD_REGRESSED, vec![
            ("before", format!("{:.6}", report.err_after)),
            ("after", format!("{:.6}", err_with)),
        ]);
        return;
    }
    // …and the do-no-harm the FRAME ruler cannot do. A field that pays for the
    // sky out of the land wins on the frame and loses the picture, and with
    // pass B's wider gains that trade is newly affordable. Every attached zone
    // is re-measured on its own membership, against the same tolerance a zone
    // is allowed to cost the frame.
    if let Some((label, was, now)) = zone_regressed(&s_img, report, &after, &target_pixels) {
        note(report, crate::rationale::keys::FIELD_ZONE_REGRESSED, vec![
            ("label", label), ("before", format!("{was:.6}")),
            ("after", format!("{now:.6}")),
            ("tol", format!("{:.4}", super::ZONE_GLOBAL_REGRESSION_TOL)),
        ]);
        return;
    }
    note(report, crate::rationale::keys::FIELD_CELLS_ADMITTED, vec![
        ("admitted", admitted.to_string()), ("read", read.to_string()),
        ("bound", format!("{:.2}", budget.field_gain)),
    ]);
    if paired.read > 0 {
        note(report, crate::rationale::keys::FIELD_LAYOUT_ADMITTED, vec![
            ("read", paired.read.to_string()), ("admitted", paired.admitted.to_string()),
        ]);
    }
    let realized = realized_share(solved.global, solved.ceiling, err_with)
        .map_or_else(|| "n/a".to_string(), |value| format!("{value:.3}"));
    note(report, crate::rationale::keys::FIELD_ATTACHED, vec![
        ("x", field.x.to_string()), ("y", field.y.to_string()), ("b", field.b.to_string()),
        ("before", format!("{:.6}", report.err_after)),
        ("after", format!("{:.6}", err_with)),
        ("ceiling", format!("{:.6}", solved.ceiling)),
        ("realized", realized),
        ("saturated", solved.saturated.to_string()),
    ]);
    report.recipe.colour_field = Some(field);
    report.err_after = err_with;
}

/// R34 §D4. PASS B and its admission, the only thing that separates the field
/// this function ships from the field the analyzer measures.
///
/// The analyzer's fit weight is `evidence x local_support x unclipped`, and
/// `local_support` is `1 - clamp(D)` over the same 12x8 cells this grid is
/// solved on. On a generative repaint that term is ~0 exactly where the
/// residual is: on the desert-dusk pair the two rows above the horizon carry
/// almost no weight, so the field solves them at nothing and the row that DOES
/// carry weight saturates 15 of its red vertices trying to cover for them.
/// Dropping the term un-starves the solve, and widening the gain bound gives it
/// somewhere to go — but a field solved without a structural term is exactly
/// the "invent your own evidence" this crate refuses by default.
///
/// So pass B is not trusted, it is TESTED: solved, rendered, and put to the
/// target's own cell means. A cell whose mean moved closer to its target AND in
/// the direction its target asks for takes pass B's eight luma-bin vertices;
/// every other cell keeps pass A's, unchanged. No cells, no pass B, no change —
/// the abstention is the pre-R34 field, byte for byte.
///
/// R42 (v1.6.1). Inside a region the segmenter found in BOTH frames
/// (`layout`, at analysis size — `fit_zoned::paired_layout`), pass B reads
/// every pixel as population evidence: the structure-blind weight the
/// frame's own ranges give it, wherever the fine reading gave it less. The
/// fine reading answers whether a PIXEL has a counterpart, and on the
/// reference pair it answered the featureless top-centre ninth of the sky
/// with "texture gone" — the source's noise against the target's smooth
/// re-synthesis: correlation +0.479, energy term 1.093, D 1.210, confidence
/// 0 — so eight of this field's cells carried no weight at all, neither
/// solve could put a vertex there, and the sky's one zone gain left a blue
/// block where the target's purple deepens. A class the segmenter found in
/// the same place on both sides is the same scene in the same place
/// whatever its texture did, which is all a cell statistic needs
/// (`fit_cells`'s own doctrine, applied to the one stage that is spatial).
/// The cells the fine reading could not read AT ALL are then read on the
/// pairing (`PairedCells::with_layout`, the same target means) and take
/// pass B's vertices on the same verdict a fine-read cell takes them on —
/// converged and aligned against the target's own cell mean — and on no
/// further guard. Two were tried on the reference pair and withdrawn
/// (2026-09-25): "the target's cell must be no more structured than the
/// source's" refused all eight, because a re-synthesised target carries
/// grain a sixteenfold-reduced develop does not, in every sky cell, read or
/// not; "no more structure grown than the pairing's own vouched cells
/// grew" refused one — a cell holding a faint contrail the target had
/// brightened — and a lone zero vertex inside a corrected block is a patch
/// more conspicuous than the block it replaced (B−R excess against the
/// target +5.7 codes between −1.2 and −5.2 in its neighbours). A cell mean
/// inside a pairing is the statistic the zone stage already ships at
/// region scale, clouds and all, under the same convergence reading, and
/// the field's two do-no-harm checks judge the result as they judge every
/// field. Cells the fine reading reads keep their own admission, on the
/// pass B that now carries the pairing's weight; no pairing, no change —
/// the field is R34's byte for byte.
#[allow(clippy::too_many_arguments)]
fn admit_cells(
    current: &[[f32; 3]], target: &[[f32; 3]], width: u32, height: u32,
    report: &FitReport, pass_a: LocalField, gain: f32, layout: &[f32],
) -> (LocalField, usize, usize, LayoutAdmission) {
    let Some(cells) = crate::fit_cells::PairedCells::build(target, width, height, &report.evidence)
    else {
        return (pass_a, 0, 0, LayoutAdmission::default());
    };
    let paired = (layout.len() == current.len() && layout.iter().any(|m| *m > 0.0)).then(|| {
        let blind = report.evidence.structure_blind(target);
        let mut model = report.evidence.clone();
        for (i, weight) in model.source_weights.iter_mut().enumerate() {
            let via_layout = blind.source_weights.get(i).copied().unwrap_or(0.0).max(0.0)
                * layout[i].clamp(0.0, 1.0);
            if via_layout > *weight {
                *weight = via_layout;
            }
        }
        (model, blind)
    });
    let Some(pass_b) = LocalField::solve_with(
        current, target, width, height,
        paired.as_ref().map_or(&report.evidence, |(model, _)| model),
        crate::fit_field::TIKHONOV, crate::fit_field::SMOOTH, crate::fit_field::ITERATIONS,
        FieldSolveOpts { gain, local_support: false },
    ) else {
        return (pass_a, 0, 0, LayoutAdmission::default());
    };
    let rendered = pass_b.render(current);
    let verdicts = cells.verdicts(current, &rendered, None);
    let paired_verdicts = paired
        .as_ref()
        .and_then(|(_, blind)| cells.with_layout(blind, layout))
        .map(|paired_cells| paired_cells.verdicts(current, &rendered, None));
    let mut merged = pass_a;
    let (mut admitted, mut read) = (0usize, 0usize);
    let mut paired_read = LayoutAdmission::default();
    // Vertex `(iy * FIELD_X + ix) * FIELD_B + ib`: a cell owns the `FIELD_B`
    // consecutive luma-bin vertices at its own (ix, iy), which is why
    // `fit_cells` takes its geometry from this grid rather than choosing one.
    let bins = crate::fit_field::FIELD_B;
    for (cell, verdict) in verdicts.iter().enumerate() {
        let admit = match verdict {
            Some(vouched) => {
                read += 1;
                if *vouched {
                    admitted += 1;
                }
                *vouched
            }
            None => {
                // The fine reading could not read this cell at all: inside
                // the pairing it is read on the pairing, and takes pass B on
                // the same verdict a fine-read cell takes it on.
                let Some(Some(vouched)) = paired_verdicts.as_ref().map(|v| v[cell]) else {
                    continue;
                };
                paired_read.read += 1;
                if vouched {
                    paired_read.admitted += 1;
                }
                vouched
            }
        };
        if !admit {
            continue;
        }
        let span = cell * bins..(cell + 1) * bins;
        if merged.grid.len() >= span.end && pass_b.grid.len() >= span.end {
            merged.grid[span.clone()].clone_from_slice(&pass_b.grid[span]);
        }
    }
    (merged, admitted, read, paired_read)
}

/// R42. What the layout reading did: the cells the fine reading could not
/// read that lie inside the pairing, and how many of them took pass B. Both
/// zero when there is no pairing or nothing unread inside it, and the
/// disclosure is then not printed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LayoutAdmission {
    pub(super) read: usize,
    pub(super) admitted: usize,
}


/// The per-ZONE half of the field's do-no-harm: the first attached mask whose
/// own look distance the field made worse by more than a zone is ever allowed
/// to cost the frame, or `None` if it hurt none of them. EVERY mask the fit
/// carries is judged — the two zone roles, and the Custom-role region masks
/// of the multi-region route; until 2026-09-24 the role filter left the
/// check inert on that route.
///
/// Membership comes from [`render::preview_mask_coverage`] — the engine's OWN
/// weight for that mask in the preview's frame, the same number the render
/// applies — so this check and the pixels it is judging cannot disagree about
/// where the zone is (a banded zone's linear components move with the lens
/// profile exactly as `develop_preview` moves them; R38).
fn zone_regressed(
    s_img: &DynamicImage, report: &FitReport, after: &[[f32; 3]], target: &[[f32; 3]],
) -> Option<(String, f32, f32)> {
    let current = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    report.recipe.masks.iter().find_map(|mask| {
        let coverage = render::preview_mask_coverage(mask, s_img, &report.recipe);
        let weights: Vec<f32> =
            coverage.as_raw().iter().map(|v| *v as f32 / 255.0).collect();
        if weights.len() != current.len() || weights.iter().sum::<f32>() <= 0.0 {
            return None;
        }
        let err = |px: &[[f32; 3]]| {
            fit::look_err_with_evidence(px, target, &report.evidence.scoped(target, &weights, &weights))
        };
        let (was, now) = (err(&current), err(after));
        let label = if mask.name.is_empty() { mask.role.tag().to_string() } else { mask.name.clone() };
        (now > was + super::ZONE_GLOBAL_REGRESSION_TOL).then_some((label, was, now))
    })
}

/// [`stop_verdict`] read the other way: is there still more than the margin on
/// the table? One expression, two callers, so "the fit may stop here" and "the
/// field may run here" can never disagree about the same number.
fn stop_verdict_has_headroom(field: &LocalField, err_after: f32) -> bool {
    !stop_verdict(field, err_after) && field.ceiling < field.global
}

pub(super) fn push_stop(report: &mut FitReport, producer: &str, skipped: &str) {
    note(report, crate::rationale::keys::LOCAL_STOP, vec![
        ("producer", producer.into()), ("skipped", skipped.into()),
        ("margin", format!("{LOCAL_STOP_MARGIN:.3}")),
    ]);
}

#[cfg(test)]
mod tests;
