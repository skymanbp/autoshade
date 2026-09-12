//! Residual-earned semantic bands. The single-zone fit is the control arm;
//! every trial replaces it, uses the shared estimator, and is transactional.

use super::*;
use crate::rationale::values;

const SUBZONE_BINS: usize = 8;
/// A majority of the horizontal residual variance must have one coherent
/// two/three-step explanation. Uniform residuals have no variance; eight
/// alternating colour bands explain only R2=0.25 with three steps, while the
/// coherent warm/cool fixture exceeds 0.99. A majority separates those arms.
const SUBZONE_R2_MIN: f32 = 0.50;
const SUBZONE_OVERLAP: f32 = 0.06;
/// R36. How many residual models reach the render gates per zone, best
/// first (fewer bands, then higher R2). R35 sent every qualifying partition
/// at sixteen ramp widths each — 95 + 106 trials on the reference pair, all
/// refused — and a zoned match went from 1 min 35 s to 17 min. A trial costs
/// three band solves and a boundary bisection; the budget is what keeps the
/// mechanism affordable on every photo, not only on the one it helps.
const SUBZONE_MODEL_BUDGET: usize = 3;

/// The ramp widths one band model is tried at. A ramp width is a SCALE, so
/// the ladder is geometric — each rung twice the last, from the 6% floor to
/// the 96% ceiling, five rungs where R35 walked sixteen six-point steps. The
/// cap folds the upper rungs onto one width; equal widths are tried once.
fn overlap_ladder(base: f32, cap: f32) -> Vec<f32> {
    let mut rungs = Vec::new();
    for scale in [1.0f32, 2.0, 4.0, 8.0, 16.0] {
        let overlap = (base * scale).min(cap);
        if rungs.last().is_none_or(|last: &f32| overlap > *last + 1e-6) {
            rungs.push(overlap);
        }
    }
    rungs
}

#[derive(Clone, Debug)]
struct BandModel {
    r2: f32,
    breaks: Vec<f32>,
    overlap: f32,
}

#[derive(Clone, Copy, Default)]
struct ResidualBin {
    mass: f64,
    delta: [f64; 3],
}

/// CIE Lab, D65, from the analysis raster's display sRGB. Keep signed a/b:
/// equally strong warm/cool residuals have equal deltaE and opposite intent.
fn lab(rgb: &[f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(render::srgb_to_linear);
    let f = |t: f32| if t > 216.0 / 24389.0 {
        t.cbrt()
    } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let x = f((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047);
    let y = f(0.2126729 * r + 0.7151522 * g + 0.0721750 * b);
    let z = f((0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883);
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

fn delta_e(before: &[[f32; 3]], target: &[[f32; 3]], weights: &[f32]) -> f32 {
    let (mut sum, mut mass) = (0.0f64, 0.0f64);
    for ((b, t), w) in before.iter().zip(target).zip(weights) {
        if *w <= 0.0 { continue; }
        let (b, t) = (lab(b), lab(t));
        let distance = b.iter().zip(t).map(|(b, t)| (b - t).powi(2)).sum::<f32>().sqrt();
        sum += distance as f64 * *w as f64;
        mass += *w as f64;
    }
    if mass > 0.0 { (sum / mass) as f32 } else { 0.0 }
}

fn quantile_y(rows: &[f64], fraction: f64) -> f32 {
    let want = rows.iter().sum::<f64>() * fraction;
    let mut mass = 0.0;
    for (y, row) in rows.iter().enumerate() {
        if *row > 0.0 && mass + row >= want {
            return ((y as f64 + (want - mass) / row) / rows.len() as f64) as f32;
        }
        mass += row;
    }
    1.0
}

fn residual_models(
    current: &[[f32; 3]], target: &[[f32; 3]], weights: &[f32], width: u32, height: u32,
) -> (f32, Vec<BandModel>) {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || weights.len() != w * h { return (0.0, Vec::new()); }
    let rows: Vec<f64> = weights.chunks_exact(w)
        .map(|row| row.iter().map(|v| *v as f64).sum()).collect();
    let total = rows.iter().sum::<f64>();
    if total < MIN_MASK_PIXELS as f64 { return (0.0, Vec::new()); }
    let edges: Vec<f32> = (0..=SUBZONE_BINS)
        .map(|i| quantile_y(&rows, i as f64 / SUBZONE_BINS as f64)).collect();
    let mut bins = [ResidualBin::default(); SUBZONE_BINS];
    // A row can straddle a quantile. Split its mass between bins, never let
    // rounding give one narrow band a different population currency.
    let mut cumulative = 0.0;
    for (y, row_mass) in rows.iter().enumerate() {
        if *row_mass <= 0.0 { continue; }
        let mut row_delta = [0.0f64; 3];
        for x in 0..w {
            let i = y * w + x;
            if weights[i] <= 0.0 { continue; }
            let (b, t) = (lab(&current[i]), lab(&target[i]));
            for c in 0..3 { row_delta[c] += (t[c] - b[c]) as f64 * weights[i] as f64; }
        }
        for (i, bin) in bins.iter_mut().enumerate() {
            let lo = total * i as f64 / SUBZONE_BINS as f64;
            let hi = total * (i + 1) as f64 / SUBZONE_BINS as f64;
            let share = ((cumulative + row_mass).min(hi) - cumulative.max(lo)).max(0.0);
            bin.mass += share;
            for (sum, delta) in bin.delta.iter_mut().zip(row_delta) {
                *sum += delta * share / row_mass;
            }
        }
        cumulative += row_mass;
    }
    let mean = |lo: usize, hi: usize| {
        let mass = bins[lo..hi].iter().map(|b| b.mass).sum::<f64>().max(1e-12);
        std::array::from_fn::<_, 3, _>(|c| bins[lo..hi].iter().map(|b| b.delta[c]).sum::<f64>() / mass)
    };
    let error = |lo: usize, hi: usize, centre: [f64; 3]| {
        bins[lo..hi].iter().map(|b| {
            (0..3).map(|c| (b.delta[c] / b.mass.max(1e-12) - centre[c]).powi(2)).sum::<f64>() * b.mass
        }).sum::<f64>()
    };
    let variance = error(0, SUBZONE_BINS, mean(0, SUBZONE_BINS));
    if variance / total < 1e-8 { return (0.0, Vec::new()); }
    let mut jumps: Vec<(usize, f64)> = (1..SUBZONE_BINS).map(|i| {
        let (a, b) = (mean(i - 1, i), mean(i, i + 1));
        (i, (0..3).map(|c| (a[c] - b[c]).powi(2)).sum())
    }).collect();
    jumps.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut candidates: Vec<usize> = jumps.iter().take(4).map(|j| j.0).collect();
    for i in 1..SUBZONE_BINS {
        let (a, b) = (mean(i - 1, i), mean(i, i + 1));
        if (0..3).any(|c| a[c] * b[c] < 0.0) { candidates.push(i); }
    }
    candidates.sort_unstable();
    candidates.dedup();
    let mut best_r2 = 0.0f32;
    let mut models = Vec::new();
    for k in [2, 3] {
        for &a in &candidates {
            let seconds: Vec<usize> = if k == 2 { vec![SUBZONE_BINS] }
                else { candidates.iter().copied().filter(|b| *b > a).collect() };
            for b in seconds {
                let cuts = if k == 2 { vec![0, a, SUBZONE_BINS] }
                    else { vec![0, a, b, SUBZONE_BINS] };
                let means: Vec<_> = cuts.windows(2).map(|p| mean(p[0], p[1])).collect();
                let residual = cuts.windows(2).zip(&means).map(|(p, m)| error(p[0], p[1], *m)).sum::<f64>();
                let r2 = (1.0 - residual / variance).clamp(0.0, 1.0) as f32;
                best_r2 = best_r2.max(r2);
                let difference = means.windows(2).map(|m| {
                    (0..3).map(|c| (m[0][c] - m[1][c]).powi(2)).sum::<f64>().sqrt()
                }).fold(f64::INFINITY, f64::min) as f32;
                // Express the zone's 0.02 acceptance floor in Lab's 100-unit
                // scale. R2 alone would reward arbitrarily small row noise.
                if r2 <= SUBZONE_R2_MIN || difference <= ZONE_MATCHED_ERR * 100.0 { continue; }
                let breaks: Vec<f32> = cuts[1..cuts.len()-1].iter().map(|c| edges[*c]).collect();
                let own_height = quantile_y(&rows, 0.995) - quantile_y(&rows, 0.005);
                let gap = breaks.windows(2).map(|p| p[1] - p[0]).fold(own_height, f32::min);
                let model = BandModel { r2, breaks, overlap: (own_height * SUBZONE_OVERLAP).min(gap * 0.9) };
                models.push(model);
            }
        }
    }
    // Residual R2 earns a trial, not the right to skip render gates for
    // every other partition. Prefer fewer bands and stronger explanations
    // only when their measured render improvements tie.
    models.sort_by(|a, b| a.breaks.len().cmp(&b.breaks.len()).then(b.r2.total_cmp(&a.r2)));
    (best_r2, models)
}

fn band_components(model: &BandModel, band: usize) -> Vec<MaskComponent> {
    model.breaks.iter().enumerate().filter_map(|(i, edge)| {
        if i != band && i + 1 != band { return None; }
        Some(MaskComponent {
            geometry: MaskGeometry::Linear {
                zero_x: 0.5, zero_y: edge - model.overlap * 0.5,
                full_x: 0.5, full_y: edge + model.overlap * 0.5,
            },
            mode: MaskCombine::Intersect,
            inverted: i == band,
        })
    }).collect()
}

fn band_weights(model: &BandModel, band: usize, image: &DynamicImage) -> Vec<f32> {
    let coverage = render::mask_coverage(&LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
        components: band_components(model, band),
        ..Default::default()
    }, image, render::MaskFrame::AsRendered);
    coverage.as_raw().iter().map(|v| *v as f32 / 255.0).collect()
}

fn band_attachment(parent: &ZoneAttachment, model: &BandModel, band: usize, image: &DynamicImage) -> ZoneAttachment {
    let weights = band_weights(model, band, image);
    let mut attachment = parent.clone();
    attachment.source_weights.iter_mut().zip(&weights).for_each(|(w, a)| *w *= a);
    attachment.target_weights.iter_mut().zip(&weights).for_each(|(w, a)| *w *= a);
    attachment.coverage = Some(ZoneCoverage {
        source: attachment.source_weights.clone(), target: attachment.target_weights.clone(),
    });
    // Restrict the LAND, not the complement of sky-intersect-band. The base
    // geometry owns the parent's inversion before any band is composed.
    if attachment.inverted {
        if let MaskGeometry::AiMask { inverted, .. } = &mut attachment.mask { *inverted = !*inverted; }
        attachment.inverted = false;
    }
    attachment.components = band_components(model, band);
    attachment.name = format!("{} · band {}/{}", parent.label, band + 1, model.breaks.len() + 1);
    attachment.label = attachment.name.clone();
    attachment
}

fn seams(
    target: &[[f32; 3]], pixels: &[[f32; 3]], frozen: &[[f32; 3]],
    rims: &[Vec<f32>], size: (u32, u32),
) -> Vec<[f32; 3]> {
    rims.iter().map(|weights| {
        let rim = boundary_rim(target, pixels, frozen, weights, size.0, size.1);
        [rim.rim, rim.charged, rim.colour_charged]
    }).collect()
}

/// The soft-rim ruler transports a settled correction through the feather.
/// It is not the target's own cross-boundary step, and a hard boundary has
/// no soft-rim samples at all. Read that independent quantity directly in
/// luma, RGB and Lab chroma. Keep each evidence cell separate: a quiet hazy
/// segment must not disappear below a whole-horizon percentile.
fn target_steps(
    target: &[[f32; 3]], pixels: &[[f32; 3]], rims: &[Vec<f32>], size: (u32, u32),
) -> Vec<[f32; 3]> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let (nx, ny) = (crate::fit_cells::CELLS_X, crate::fit_cells::CELLS_Y);
    let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let mut scores = Vec::new();
    for weights in rims {
        let mut cells = vec![Vec::<[f32; 3]>::new(); nx * ny];
        let mut walk = |start: usize, stride: usize, len: usize| {
            for p in 1..len {
                let (lo, hi) = (start + (p - 1) * stride, start + p * stride);
                if (weights[lo] >= 0.5) == (weights[hi] >= 0.5) { continue; }
                let offset = ZONE_STEP_OFFSET.saturating_sub(1);
                let Some(a) = (p - 1).checked_sub(offset) else { continue; };
                let b = p + offset;
                if b >= len { continue; }
                let (a, b) = (start + a * stride, start + b * stride);
                if (weights[a] >= 0.5) != (weights[lo] >= 0.5)
                    || (weights[b] >= 0.5) != (weights[hi] >= 0.5) { continue; }
                let gap = std::array::from_fn::<_, 3, _>(|c|
                    (pixels[a][c] - pixels[b][c]) - (target[a][c] - target[b][c]));
                let (pa, pb, ta, tb) = (lab(&pixels[a]), lab(&pixels[b]), lab(&target[a]), lab(&target[b]));
                let chroma = (1..3).map(|c| ((pa[c] - pb[c]) - (ta[c] - tb[c])).powi(2)).sum::<f32>().sqrt() / 100.0;
                let cell = ((hi / w) * ny / h).min(ny - 1) * nx + ((hi % w) * nx / w).min(nx - 1);
                cells[cell].push([luma(gap).abs(), gap.into_iter().map(f32::abs).fold(0.0, f32::max), chroma]);
            }
        };
        for y in 0..h { walk(y * w, 1, w); }
        for x in 0..w { walk(x, w, h); }
        scores.extend(cells.iter().map(|samples| std::array::from_fn(|c| {
            if samples.is_empty() { 0.0 } else {
                magnitude_rank(&mut samples.iter().map(|s| s[c]).collect::<Vec<_>>())
            }
        })));
    }
    scores
}

fn seams_do_not_regress(before: &[[f32; 3]], after: &[[f32; 3]]) -> bool {
    !before.is_empty() && before.len() == after.len()
        && before.iter().zip(after).all(|(b, a)| a.iter().zip(b).all(|(a, b)| *a <= b + 1e-6))
}

fn worst_seams(scores: &[[f32; 3]]) -> [f32; 3] {
    scores.iter().fold([0.0f32; 3], |worst, score| std::array::from_fn(|i| worst[i].max(score[i])))
}

fn scores_text(scores: &[[f32; 3]]) -> String {
    // A compact disclosure, never the acceptance test: a better worst rim
    // cannot pay for a regression at another break or at the horizon.
    worst_seams(scores).map(|v| format!("{v:.5}")).join("/")
}

fn parent_band_anchor(band: &LocalAdjustment, parent: &LocalAdjustment) -> LocalAdjustment {
    LocalAdjustment {
        mask: band.mask.clone(), components: band.components.clone(),
        name: band.name.clone(), role: band.role, inverted: band.inverted,
        range: band.range, ..parent.clone()
    }
}

/// A replacement shrinks its NEW differences, not the accepted correction
/// it replaces. Geometry remains a set of native bands; the parent is never
/// stacked underneath. Final gates still compare against the single parent,
/// so the bands' overlap cannot assume that duplicated controls are exact.
fn shrink_to_parent(
    masks: &mut [LocalAdjustment], originals: &[LocalAdjustment], parent: &LocalAdjustment, k: f32,
) {
    for (dst, original) in masks.iter_mut().zip(originals) {
        let anchor = parent_band_anchor(original, parent);
        if k == 0.0 { *dst = anchor; continue; }
        *dst = original.clone();
        if k == 1.0 { continue; }
        macro_rules! blend {
            ($($field:ident),+ $(,)?) => { $(dst.$field = anchor.$field + k * (original.$field - anchor.$field);)+ };
        }
        blend!(exposure_ev, contrast, highlights, shadows, whites, blacks, saturation,
            clarity, dehaze, texture, sharpness, hue, temperature, tint, noise_reduction, amount);
        let a = anchor.color_gains.unwrap_or([1.0; 3]);
        let b = original.color_gains.unwrap_or([1.0; 3]);
        dst.color_gains = Some(std::array::from_fn(|c| a[c] + k * (b[c] - a[c])));
    }
}

/// Called after the single-zone boundary verdict and before the local
/// sequencer. Every rejected trial restores recipe, notes, and report error.
/// Only its one typed outcome is added to the control arm's disclosure.
pub(super) fn replace_zone(
    image: &DynamicImage, target: &[[f32; 3]], report: &mut FitReport,
    parent: &ZoneAttachment, divergence: Option<fit::Divergence>,
) {
    let parent_index = report.recipe.masks.iter().position(|m|
        m.role == parent.role && m.components.is_empty()
    );
    let single_px = fit::pixels_of(&render::develop_preview(image, &report.recipe));
    let (r2, models) = residual_models(&single_px, target, &parent.source_weights, image.width(), image.height());
    if models.is_empty() {
        crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(crate::rationale::keys::ZONE_SUBZONES_NOT_EARNED,
                vec![("label", parent.label.clone()), ("r2", format!("{r2:.4}"))]));
        return;
    }
    let saved_recipe = report.recipe.clone();
    let saved_notes = report.notes.clone();
    let saved_error = report.err_after;
    let correspondence = report.correspondence.take();
    let before = delta_e(&single_px, target, &parent.source_weights);
    let mut best = None;
    let mut rejected = None;
    // The initial 6% overlap is a starting measurement, not a fixed seam.
    // Trial wider native ramps against the same residual and all the same
    // gates. The smallest model wins ties; no reference-pair special case.
    let models: Vec<_> = models.into_iter().take(SUBZONE_MODEL_BUDGET).flat_map(|model| {
        let cap = model.breaks.windows(2).map(|p| (p[1] - p[0]) * 0.9).fold(1.0, f32::min);
        overlap_ladder(model.overlap, cap)
            .into_iter()
            .map(|overlap| BandModel { overlap, ..model.clone() })
            .collect::<Vec<_>>()
    }).collect();
    let trial_count = models.len();
    // The control arm without its parent renders the same for every trial:
    // one render, not one per trial.
    let uncorrected = {
        let mut without_parent = saved_recipe.clone();
        if let Some(index) = parent_index { without_parent.masks.remove(index); }
        fit::pixels_of(&render::develop_preview(image, &without_parent))
    };
    let uncorrected_error = fit::look_err_with_evidence(&uncorrected, target, &report.evidence);
    for model in models {
        report.recipe = saved_recipe.clone();
        report.notes = saved_notes.clone();
        if let Some(index) = parent_index { report.recipe.masks.remove(index); }
        let mut frame_error = uncorrected_error;
        let first_band = report.recipe.masks.len();
        let attachments: Vec<_> = (0..=model.breaks.len())
            .map(|band| band_attachment(parent, &model, band, image)).collect();
        let rims: Vec<_> = std::iter::once(parent.source_weights.clone())
            .chain(attachments.iter().map(|a| a.source_weights.clone())).collect();
        let mut solved = true;
        for attachment in &attachments {
            if attach_one_zone(image, target, report, &mut frame_error, attachment, divergence, correspondence.as_ref()).is_none() {
                solved = false;
                break;
            }
        }
        let mut pixels = fit::pixels_of(&render::develop_preview(image, &report.recipe));
        let mut boundary_passed = solved;
        if solved {
            let shares: Vec<f32> = rims[1..].iter().map(|w| w.iter().sum::<f32>() / w.len().max(1) as f32).collect();
            let boundaries: Vec<&[f32]> = rims.iter().map(Vec::as_slice).collect();
            let verdict = if let Some(index) = parent_index {
                let control = &saved_recipe.masks[index];
                let originals = report.recipe.masks[first_band..].to_vec();
                shrink_to_parent(&mut report.recipe.masks[first_band..], &originals, control, 0.0);
                let anchor_px = fit::pixels_of(&render::develop_preview(image, &report.recipe));
                report.recipe.masks[first_band..].clone_from_slice(&originals);
                enforce_boundary_gates_with_shrink(image, report, &boundaries, &shares, first_band,
                    (&anchor_px, pixels), |masks, originals, _, k| shrink_to_parent(masks, originals, control, k))
            } else {
                enforce_boundary_gates(image, report, &boundaries, &shares, first_band, &uncorrected, pixels)
            };
            match verdict {
                BoundaryGateResult::Kept { pixels: kept, .. } => pixels = kept,
                BoundaryGateResult::Dropped => {
                    boundary_passed = false;
                    pixels = uncorrected.clone();
                }
            }
        }
        let after = delta_e(&pixels, target, &parent.source_weights);
        let size = (image.width(), image.height());
        let seam_before = seams(target, &single_px, &single_px, &rims, size);
        let seam_after = seams(target, &pixels, &single_px, &rims, size);
        let step_before = target_steps(target, &single_px, &rims, size);
        let step_after = target_steps(target, &pixels, &rims, size);
        let no_seam_regression = seams_do_not_regress(&seam_before, &seam_after)
            && seams_do_not_regress(&step_before, &step_after);
        let zone_before = zone_err(&zone_moments(&single_px, &parent.source_weights), &zone_moments(target, &parent.target_weights));
        let zone_after = zone_err(&zone_moments(&pixels, &parent.source_weights), &zone_moments(target, &parent.target_weights));
        let frame_after = fit::look_err_with_evidence(&pixels, target, &report.evidence);
        let reason = if !solved { values::BAND_ESTIMATOR }
            else if !boundary_passed { values::BOUNDARY_BUDGET }
            else if after >= before - ZONE_GLOBAL_REGRESSION_TOL { values::DELTA_E_MARGIN }
            else if zone_after > zone_before + ZONE_GLOBAL_REGRESSION_TOL { values::ZONE_DO_NO_HARM }
            else if frame_after > saved_error + parent.frame_regression_tol { values::FRAME_DO_NO_HARM }
            else if !no_seam_regression { values::BOUNDARY_REGRESSION } else { "accepted" };
        let complete = solved && boundary_passed;
        let args = vec![
            ("label", parent.label.clone()), ("k", (model.breaks.len() + 1).to_string()),
            ("trials", trial_count.to_string()),
            ("r2", format!("{:.4}", model.r2)),
            ("overlap", format!("{:.5}", model.overlap)),
            ("breaks", model.breaks.iter().map(|v| format!("{v:.5}")).collect::<Vec<_>>().join(",")),
            ("before", format!("{before:.4}")),
            ("after", if complete { format!("{after:.4}") } else { values::UNMEASURED.to_string() }),
            ("seam_before", scores_text(&seam_before)),
            ("seam_after", if complete { scores_text(&seam_after) } else { values::UNMEASURED.to_string() }),
            ("step_before", scores_text(&step_before)),
            ("step_after", if complete { scores_text(&step_after) } else { values::UNMEASURED.to_string() }),
        ];
        if reason == "accepted" && best.as_ref().is_none_or(|(_, _, _, error): &(_, _, _, f32)| after < *error - ZONE_GLOBAL_REGRESSION_TOL) {
            report.err_after = frame_after;
            best = Some((report.recipe.clone(), report.notes.clone(), args.clone(), after));
        }
        // Report the best fully solved rejected render, not whichever
        // partition happened to run last. A partial band's deltaE is not a
        // measurement of the proposed set.
        let rank = (u8::from(complete), if complete { -after } else { model.r2 });
        if rejected.as_ref().is_none_or(|(_, _, old): &(_, _, (u8, f32))| rank > *old) {
            rejected = Some((args, reason.to_string(), rank));
        }
    }
    report.correspondence = correspondence;
    if let Some((recipe, notes, args, _)) = best {
        report.recipe = recipe;
        report.notes = notes;
        let pixels = fit::pixels_of(&render::develop_preview(image, &report.recipe));
        report.err_after = fit::look_err_with_evidence(&pixels, target, &report.evidence);
        crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(crate::rationale::keys::ZONE_SUBZONES_ATTACHED, args));
    } else {
        report.recipe = saved_recipe;
        report.notes = saved_notes;
        report.err_after = saved_error;
        if let Some((mut args, reason, _)) = rejected {
            args.push(("reason", reason));
            crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
                crate::rationale::Note::new(crate::rationale::keys::ZONE_SUBZONES_REGRESSED, args));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn fixture(split: bool) -> (DynamicImage, DynamicImage, ZoneAttachment, FitReport) {
        let (w, h) = (384, 256);
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = if y < 160 { 90 + (x % 96) as u8 } else { 50 + (x % 80) as u8 };
            Rgb([v; 3])
        }));
        let root = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
            .join("test-rasters");
        std::fs::create_dir_all(&root).unwrap();
        // Independent test threads must never rewrite an alpha another
        // fixture is rendering, even when their intended pixels are identical.
        static NEXT_RASTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT_RASTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = root.join(format!("bands-{}-{serial}.png", std::process::id()));
        let sky = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < 160 { 255 } else { 0 }]));
        sky.save(&path).unwrap();
        let weights = mask_weights(&sky, w, h);
        let parent = ZoneAttachment {
            mask: MaskGeometry::select_sky(0.5, 0.25, false, path.to_string_lossy().into_owned()),
            components: Vec::new(), source_weights: weights.clone(), target_weights: weights,
            coverage: None, range: None, role: MaskRole::ZoneSky, inverted: false,
            name: String::new(), label: "sky".into(), min_share: MIN_ZONE_SHARE,
            frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
        };
        let model = BandModel { r2: 1.0, breaks: vec![0.3125], overlap: 0.625 * SUBZONE_OVERLAP };
        let masks = if split {
            (0..2).map(|band| {
                let a = band_attachment(&parent, &model, band, &source);
                LocalAdjustment {
                    mask: a.mask, components: a.components, role: a.role,
                    exposure_ev: 0.15, color_gains: Some(if band == 0 { [1.16, 1.0, 0.84] } else { [0.84, 1.0, 1.16] }),
                    ..Default::default()
                }
            }).collect()
        } else {
            vec![LocalAdjustment {
                mask: parent.mask.clone(), role: parent.role, exposure_ev: 0.15,
                color_gains: Some([1.12, 1.0, 0.88]), ..Default::default()
            }]
        };
        let target = render::develop_preview(&source, &crate::recipe::EditRecipe { masks, ..Default::default() });
        let mut report = super::super::tests::neutral_report(&source, &target);
        // The single-correction control matches the average brightness but
        // cannot carry the opposing vertical colours in this fixture.
        report.recipe.masks.push(LocalAdjustment {
            mask: parent.mask.clone(), role: parent.role, exposure_ev: 0.15, ..Default::default()
        });
        report.err_after = fit::look_err_with_evidence(
            &fit::pixels_of(&render::develop_preview(&source, &report.recipe)),
            &fit::pixels_of(&target), &report.evidence,
        );
        (source, target, parent, report)
    }

    #[test]
    fn the_overlap_ladder_is_geometric_and_folds_onto_its_cap() {
        assert_eq!(overlap_ladder(0.06, 1.0), vec![0.06, 0.12, 0.24, 0.48, 0.96]);
        assert_eq!(overlap_ladder(0.06, 0.30), vec![0.06, 0.12, 0.24, 0.30]);
        assert_eq!(overlap_ladder(0.06, 0.05), vec![0.05], "a cap under the floor is one width, tried once");
    }

    #[test]
    fn an_improved_worst_rim_cannot_hide_another_band_boundary_regression() {
        let before = [[0.10, 0.12, 0.15], [0.30, 0.32, 0.35], [0.20, 0.22, 0.25]];
        let after = [[0.11, 0.13, 0.16], [0.29, 0.31, 0.34], [0.19, 0.21, 0.24]];
        assert!(worst_seams(&after).iter().zip(worst_seams(&before)).all(|(a, b)| *a < b),
            "the old maximum-only comparison would admit this set");
        assert!(!seams_do_not_regress(&before, &after), "the horizon still regressed");
        let conserved = [[0.10, 0.12, 0.15], after[1], after[2]];
        assert!(seams_do_not_regress(&before, &conserved));
        assert!(seams_do_not_regress(&before, &before));
        assert!(!seams_do_not_regress(&before, &conserved[..2]), "a missing rim is not a pass");
    }

    #[test]
    fn replacement_shrink_retains_the_parent_tone_at_zero_and_the_fitted_bands_at_one() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(96, 64, Rgb([96, 112, 144])));
        let parent = LocalAdjustment { exposure_ev: 0.15, color_gains: Some([1.02, 1.0, 0.98]), ..Default::default() };
        let model = BandModel { r2: 1.0, breaks: vec![0.5], overlap: 0.06 };
        let originals: Vec<_> = (0..2).map(|i| LocalAdjustment {
            components: band_components(&model, i), exposure_ev: if i == 0 { 0.3 } else { -0.1 },
            color_gains: Some([1.1, 1.0, 0.9]), ..Default::default()
        }).collect();
        let anchors: Vec<_> = originals.iter().map(|m| parent_band_anchor(m, &parent)).collect();
        let mut shrunk = originals.clone();
        shrink_to_parent(&mut shrunk, &originals, &parent, 0.0);
        assert_eq!(shrunk, anchors);
        assert!(shrunk.iter().all(|m| m.exposure_ev == 0.15));
        let render = |masks| render::develop_preview(&image, &crate::recipe::EditRecipe { masks, ..Default::default() });
        assert_eq!(render(shrunk.clone()), render(anchors));
        shrink_to_parent(&mut shrunk, &originals, &parent, 1.0);
        assert_eq!(shrunk, originals);
    }

    #[test]
    fn target_step_regression_cannot_hide_in_an_empty_soft_rim() {
        let (w, h) = (48usize, 48usize);
        let target: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.4; 3] } else { [0.2; 3] }).collect();
        let weights: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { 1.0 } else { 0.0 }).collect();
        let before: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.38; 3] } else { [0.2; 3] }).collect();
        let after: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.36; 3] } else { [0.2; 3] }).collect();
        let rims = vec![weights];
        let size = (w as u32, h as u32);
        assert!(seams_do_not_regress(&seams(&target, &before, &before, &rims, size),
            &seams(&target, &after, &before, &rims, size)), "a hard edge has no soft-rim samples");
        assert!(!seams_do_not_regress(&target_steps(&target, &before, &rims, size),
            &target_steps(&target, &after, &rims, size)), "the target step still got worse");
    }

    #[test]
    fn a_regressing_boundary_cell_cannot_hide_below_the_horizon_percentile() {
        let (w, h) = (192usize, 96usize);
        let target: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.5; 3] } else { [0.2; 3] }).collect();
        let before: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.49; 3] } else { [0.2; 3] }).collect();
        let after: Vec<_> = (0..w*h).map(|i| if i/w < h/2 {
            if i%w < w/crate::fit_cells::CELLS_X { [0.48; 3] } else { [0.5; 3] }
        } else { [0.2; 3] }).collect();
        let weights = (0..w*h).map(|i| if i/w < h/2 { 1.0 } else { 0.0 }).collect();
        let rims = vec![weights];
        let size = (w as u32, h as u32);
        assert!(!seams_do_not_regress(&target_steps(&target, &before, &rims, size),
            &target_steps(&target, &after, &rims, size)), "one of twelve columns regressed");
        assert!(seams_do_not_regress(&target_steps(&target, &before, &rims, size),
            &target_steps(&target, &target, &rims, size)));
    }

    #[test]
    fn adjacent_intersection_bands_sum_to_one_for_the_measured_feather() {
        let source = DynamicImage::new_rgb8(384, 256);
        for breaks in [vec![0.3], vec![0.2, 0.5]] {
            let model = BandModel { r2: 1.0, breaks, overlap: 0.06 };
            let weights: Vec<_> = (0..=model.breaks.len()).map(|band| band_weights(&model, band, &source)).collect();
            for i in 0..384 * 256 {
                let sum = weights.iter().map(|w| w[i]).sum::<f32>();
                assert!((sum - 1.0).abs() <= 1.0 / 255.0, "pixel {i}: {sum}");
            }
            assert!((0..=model.breaks.len()).flat_map(|band| band_components(&model, band))
                .all(|c| c.mode == MaskCombine::Intersect));
        }
    }

    #[test]
    fn an_alternating_residual_does_not_earn_a_coarse_band_model() {
        let (w, h) = (384, 256);
        let current = vec![[0.5; 3]; w * h];
        let target: Vec<_> = (0..w * h).map(|i| {
            if (i / w / (h / SUBZONE_BINS)).is_multiple_of(2) { [0.58, 0.5, 0.42] }
            else { [0.42, 0.5, 0.58] }
        }).collect();
        let (r2, models) = residual_models(&current, &target, &vec![1.0; w * h], w as u32, h as u32);
        assert!((r2 - 0.25).abs() < 1e-3, "alternating residual R2={r2}");
        assert!(models.is_empty(), "a coarse step explanation must earn a majority of variance");
    }

    #[test]
    fn uniform_recolour_earns_no_subzones_and_keeps_recipe_bytes_except_its_note() {
        let (source, target, parent, mut report) = fixture(false);
        let mut control = report.recipe.clone();
        replace_zone(&source, &fit::pixels_of(&target), &mut report, &parent, None);
        assert_eq!(report.notes.len(), 1);
        assert_eq!(report.notes[0].key, crate::rationale::keys::ZONE_SUBZONES_NOT_EARNED);
        control.rationale = report.recipe.rationale.clone();
        assert_eq!(serde_json::to_vec(&control).unwrap(), serde_json::to_vec(&report.recipe).unwrap());
    }

    #[test]
    fn a_rejected_subzone_set_restores_the_complete_single_zone_control() {
        let (source, target, mut parent, mut report) = fixture(true);
        parent.min_share = 1.1;
        let mut control = report.recipe.clone();
        let error = report.err_after;
        replace_zone(&source, &fit::pixels_of(&target), &mut report, &parent, None);
        assert_eq!(report.notes.len(), 1, "trial notes must not leak into the control arm");
        assert_eq!(report.notes[0].key, crate::rationale::keys::ZONE_SUBZONES_REGRESSED);
        control.rationale = report.recipe.rationale.clone();
        assert_eq!(serde_json::to_vec(&control).unwrap(), serde_json::to_vec(&report.recipe).unwrap());
        assert_eq!(report.err_after, error);
    }

    #[test]
    fn a_residual_earns_alternative_partitions_until_render_gates_choose() {
        let (source, target, parent, report) = fixture(true);
        let current = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        let (_, models) = residual_models(&current, &fit::pixels_of(&target),
            &parent.source_weights, source.width(), source.height());
        let two: Vec<_> = models.iter().filter(|m| m.breaks.len() == 1).collect();
        assert!(two.len() > 1, "a secondary earned partition must reach the render gate: {models:?}");
        assert!(two.iter().any(|m| m.r2 < two[0].r2));
        assert!(models.iter().all(|m| m.r2 > SUBZONE_R2_MIN));
    }

    #[test]
    fn a_vertical_warm_cool_sky_earns_bands_that_replace_the_single_correction() {
        let (source, target, parent, mut report) = fixture(true);
        let target = fit::pixels_of(&target);
        let before = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        let (r2, models) = residual_models(&before, &target, &parent.source_weights, source.width(), source.height());
        assert!(r2 > SUBZONE_R2_MIN && !models.is_empty(), "R2={r2}, {models:?}");
        replace_zone(&source, &target, &mut report, &parent, None);
        eprintln!("subzone fixture: {}", report.recipe.rationale);
        assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_SUBZONES_ATTACHED));
        assert!((2..=3).contains(&report.recipe.masks.len()));
        assert!(report.recipe.masks.iter().all(|m| m.role == MaskRole::ZoneSky && !m.components.is_empty()));
        let after = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        assert!(delta_e(&after, &target, &parent.source_weights) < delta_e(&before, &target, &parent.source_weights) - ZONE_GLOBAL_REGRESSION_TOL);
    }
}
