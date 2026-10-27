//! Residual-earned semantic bands. The single-zone fit is the control arm;
//! every trial replaces it, uses the shared estimator, and is transactional.

use super::*;
use crate::rationale::values;

/// A majority of the vertical residual variance must have one coherent
/// two- or three-band explanation. Uniform residuals have no variance; eight
/// alternating colour stripes explain under a third with three bands, while
/// the coherent warm/cool fixture exceeds 0.99. A majority separates those arms.
const SUBZONE_R2_MIN: f32 = 0.50;
/// The base ramp width, as a share of the zone's own height; the widths tried
/// are [`SUBZONE_WIDTH_SCALES`] times it.
const SUBZONE_OVERLAP: f32 = 0.06;
/// The ramp widths a break is tried at, as multiples of [`SUBZONE_OVERLAP`]:
/// from a near-step to a ramp the whole zone long. A ramp width is a SCALE,
/// so the rungs are geometric.
const SUBZONE_WIDTH_SCALES: [f32; 6] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
/// How many places along the zone's own extent a break is tried at.
const SUBZONE_BREAK_STEPS: usize = 32;
/// How many partitions of each band count reach the render gates, best R2
/// first and no two with the same breaks. A trial costs three band solves and
/// a boundary bisection; the budget is what keeps the mechanism affordable on
/// every photo, not only on the one it helps (R36: an unbounded walk put 95 +
/// 106 trials on the reference pair and took a zoned match from 1 min 35 s to
/// 17 min).
const SUBZONE_MODELS_PER_COUNT: usize = 3;

/// A vertical band partition of a zone: `breaks[i]` is the centre of the ramp
/// between band `i` and band `i + 1`, `widths[i]` its full width, both as
/// shares of the frame height, and `r2` how much of the zone's row-profile
/// residual the partition explains ([`residual_models`]).
#[derive(Clone, Debug)]
struct BandModel {
    r2: f32,
    breaks: Vec<f32>,
    widths: Vec<f32>,
    /// A shaped model: the first ramp's falloff is this measured curve
    /// ([`shaped_model`]) rather than Lightroom's law.
    shape: Option<Vec<f32>>,
}

impl BandModel {
    /// The ramp between band `i` and `i + 1`: its Zero and Full handles.
    fn ramp(&self, i: usize) -> (f32, f32) {
        (self.breaks[i] - self.widths[i] * 0.5, self.breaks[i] + self.widths[i] * 0.5)
    }

    /// The coverage of the ramp between band `i` and `i + 1` at frame-height
    /// share `y`, the way the renderer composes it: Lightroom's law for a
    /// measured ramp, the zone's own profile for a shaped one.
    fn coverage(&self, i: usize, y: f32) -> f32 {
        let (zero, full) = self.ramp(i);
        let t = (y - zero) / (full - zero).max(1e-6);
        match &self.shape {
            Some(falloff) if i == 0 => render::shaped_ramp_coverage(t, falloff),
            _ => render::linear_ramp_coverage(t),
        }
    }
}

/// Samples of a shaped ramp's falloff over the zone's extent.
const SUBZONE_SHAPE_SAMPLES: usize = 64;
/// The smoothing window of the shaped ramp, as a share of the zone's own
/// height, each side: wide enough to average the stars and the texture out
/// of a row (eight rows of a 128-row sky on the analysis frame), narrow
/// against the horizon glow it has to follow (a sixth of that sky).
const SUBZONE_SHAPE_WINDOW: f32 = 1.0 / 32.0;

/// The one band model whose ramp IS the zone's residual profile: the rows'
/// mean Lab residual, smoothed over [`SUBZONE_SHAPE_WINDOW`], projected onto
/// the direction it moves from the zone's middle half to its bottom quarter,
/// and scaled so the profile's MEDIAN reads 0 and its peak 1 (negative
/// clamped) — a ramp from the zone's top to its bottom whose falloff is that
/// curve ([`crate::recipe::MaskGeometry::Shaped`]). `None` when the bottom
/// does not differ from the middle. The median, not the minimum, is the
/// zero: the lower band is the part of the zone that departs from its
/// typical level, so it is the glow, not half the sky at a third strength —
/// on the night pair a min-to-max scale gave the glow band 24 % of the
/// frame, its solve neutralised by the cell voucher (2026-10-20). Its two
/// levels are the least-squares answer like any other model's, so the
/// scale's offset is absorbed by them.
fn shaped_model(rows: &[ProfileRow], top: f32, bottom: f32) -> Option<BandModel> {
    let n = SUBZONE_SHAPE_SAMPLES;
    let half = (bottom - top) * SUBZONE_SHAPE_WINDOW;
    let sample = |y: f32| -> Option<[f64; 3]> {
        let (mut mass, mut delta) = (0.0f64, [0.0f64; 3]);
        for row in rows.iter().filter(|r| r.mass > 0.0 && (r.y - y).abs() <= half) {
            mass += row.mass;
            for (d, value) in delta.iter_mut().zip(&row.delta) { *d += value * row.mass; }
        }
        (mass > 0.0).then(|| delta.map(|d| d / mass))
    };
    let samples: Vec<[f64; 3]> = (0..n).map(|s| {
        let y = top + (bottom - top) * s as f32 / (n - 1) as f32;
        sample(y)
    }).collect::<Option<_>>()?;
    let quarter = n / 4;
    let mean = |range: std::ops::Range<usize>| -> [f64; 3] {
        let mut m = [0.0f64; 3];
        for s in range.clone() { for c in 0..3 { m[c] += samples[s][c]; } }
        m.map(|v| v / range.len() as f64)
    };
    let (middle, tail) = (mean(quarter..n - quarter), mean(n - quarter..n));
    let direction: [f64; 3] = std::array::from_fn(|c| tail[c] - middle[c]);
    let norm = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm <= f64::from(ZONE_MATCHED_ERR) * 100.0 { return None; }
    let projected: Vec<f64> = samples.iter()
        .map(|s| (0..3).map(|c| s[c] * direction[c] / norm).sum::<f64>()).collect();
    let mut sorted = projected.clone();
    sorted.sort_by(f64::total_cmp);
    let zero = sorted[n / 2];
    let peak = sorted[n - 1];
    if peak - zero <= f64::from(ZONE_MATCHED_ERR) * 100.0 { return None; }
    let falloff: Vec<f32> = projected.iter().map(|v| ((v - zero) / (peak - zero)).clamp(0.0, 1.0) as f32).collect();
    Some(BandModel { r2: 0.0, breaks: vec![(top + bottom) * 0.5], widths: vec![bottom - top], shape: Some(falloff) })
}

/// One row of the zone's residual profile: its vertical position as a share
/// of the frame height, its mask mass and the mass-weighted mean Lab residual
/// (target minus current) over its pixels.
struct ProfileRow {
    y: f32,
    mass: f64,
    delta: [f64; 3],
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

/// The frame-height share under which `fraction` of the zone's mass lies.
fn quantile_y(rows: &[ProfileRow], fraction: f64) -> f32 {
    let want = rows.iter().map(|r| r.mass).sum::<f64>() * fraction;
    let mut mass = 0.0;
    for (i, row) in rows.iter().enumerate() {
        if row.mass > 0.0 && mass + row.mass >= want {
            let next = rows.get(i + 1).map_or(row.y, |r| r.y);
            return row.y + (next - row.y) * ((want - mass) / row.mass) as f32;
        }
        mass += row.mass;
    }
    rows.last().map_or(1.0, |r| r.y)
}

/// The coverage of each band of `model` at frame-height share `y`, the way the
/// renderer composes them: band `i` is the ramp above it (`i - 1`) intersected
/// with the inverse of the ramp below it (`i`), through the measured linear
/// feather ([`render::linear_ramp_coverage`]).
fn band_coverages(model: &BandModel, y: f32) -> Vec<f32> {
    let ramp = |i: usize| -> f32 { model.coverage(i, y) };
    (0..=model.breaks.len())
        .map(|band| {
            let above = if band == 0 { 1.0 } else { ramp(band - 1) };
            let below = if band == model.breaks.len() { 0.0 } else { ramp(band) };
            above * (1.0 - below)
        })
        .collect()
}

/// The mass-weighted least-squares band levels (one Lab residual per band)
/// that `model` explains `rows` with, and the share of the rows' residual
/// variance it leaves unexplained: `(levels, residual)`.
fn fit_levels(model: &BandModel, rows: &[ProfileRow]) -> (Vec<[f64; 3]>, f64) {
    let n = model.breaks.len() + 1;
    // Normal equations: (A^T W A) levels = A^T W delta, A the coverages.
    let mut ata = vec![vec![0.0f64; n]; n];
    let mut atd = vec![[0.0f64; 3]; n];
    for row in rows {
        if row.mass <= 0.0 { continue; }
        let a = band_coverages(model, row.y);
        for i in 0..n {
            let ai = a[i] as f64 * row.mass;
            for j in 0..n { ata[i][j] += ai * a[j] as f64; }
            for (slot, value) in atd[i].iter_mut().zip(&row.delta) { *slot += ai * value; }
        }
    }
    // Gauss-Jordan with partial pivoting on the n x (n + 3) system; a
    // band no row reaches leaves a zero pivot and its level at 0.
    let mut m: Vec<Vec<f64>> = (0..n).map(|i| {
        let mut r = ata[i].clone(); r.extend_from_slice(&atd[i]); r
    }).collect();
    for col in 0..n {
        let pivot = (col..n).max_by(|a, b| m[*a][col].abs().total_cmp(&m[*b][col].abs())).unwrap_or(col);
        if m[pivot][col].abs() < 1e-12 { continue; }
        m.swap(col, pivot);
        let p = m[col][col];
        for v in m[col].iter_mut() { *v /= p; }
        for r in 0..n {
            if r == col { continue; }
            let f = m[r][col];
            if f == 0.0 { continue; }
            let pivot_row = m[col].clone();
            for (slot, v) in m[r].iter_mut().zip(&pivot_row) { *slot -= f * v; }
        }
    }
    let levels: Vec<[f64; 3]> = (0..n).map(|i| [m[i][n], m[i][n + 1], m[i][n + 2]]).collect();
    let residual = rows.iter().filter(|r| r.mass > 0.0).map(|row| {
        let a = band_coverages(model, row.y);
        (0..3).map(|c| {
            let pred: f64 = (0..n).map(|i| a[i] as f64 * levels[i][c]).sum();
            (row.delta[c] - pred).powi(2)
        }).sum::<f64>() * row.mass
    }).sum();
    (levels, residual)
}

/// The band partitions a zone's residual earns, measured from the target
/// ([`BandModel`]): the zone's row profile (mass-weighted mean Lab residual
/// per row) is fitted with every two- and three-band model whose breaks sit
/// at [`SUBZONE_BREAK_STEPS`] places along the zone's extent and whose ramps
/// take any of the [`SUBZONE_WIDTH_SCALES`] widths — the band LEVELS being the
/// least-squares answer for each ([`fit_levels`]) — and the best
/// [`SUBZONE_MODELS_PER_COUNT`] distinct partitions of each count that explain
/// a majority of the variance ([`SUBZONE_R2_MIN`]) and separate adjacent bands
/// by more than the zone acceptance floor are returned, fewer bands first and
/// higher R2 next. Also the best R2 of any fit, for the disclosure.
///
/// The ramp's centre and width are read off the target because they are what
/// the target shows: the rule used to bin the profile into eight equal-mass
/// steps, pick the largest jumps, and then try a fixed ladder of widths for
/// each. On a night pair (2026-10-20) the sky's residual was one continuous
/// gradient reaching the horizon — bluer and then much brighter row by row
/// over the lower half — and the step model's best fit (R2 0.62) left the
/// change to one band at one of five widths; whichever width the gates kept
/// painted a line where its ramp ended. Fitted ramps read R2 0.83 on the same
/// profile, the last one still rising at the horizon.
fn residual_models(
    current: &[[f32; 3]], target: &[[f32; 3]], weights: &[f32], width: u32, height: u32,
) -> (f32, Vec<BandModel>) {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || weights.len() != w * h { return (0.0, Vec::new()); }
    let rows: Vec<ProfileRow> = (0..h).map(|y| {
        let (mut mass, mut delta) = (0.0f64, [0.0f64; 3]);
        for x in 0..w {
            let i = y * w + x;
            if weights[i] <= 0.0 { continue; }
            let (b, t) = (lab(&current[i]), lab(&target[i]));
            mass += weights[i] as f64;
            for c in 0..3 { delta[c] += (t[c] - b[c]) as f64 * weights[i] as f64; }
        }
        if mass > 0.0 { for d in delta.iter_mut() { *d /= mass; } }
        ProfileRow { y: (y as f32 + 0.5) / h as f32, mass, delta }
    }).collect();
    let total = rows.iter().map(|r| r.mass).sum::<f64>();
    if total < MIN_MASK_PIXELS as f64 { return (0.0, Vec::new()); }
    let mean: [f64; 3] = std::array::from_fn(|c| rows.iter().map(|r| r.delta[c] * r.mass).sum::<f64>() / total);
    let variance: f64 = rows.iter().map(|r| (0..3).map(|c| (r.delta[c] - mean[c]).powi(2)).sum::<f64>() * r.mass).sum();
    if variance / total < 1e-8 { return (0.0, Vec::new()); }
    let (top, bottom) = (quantile_y(&rows, 0.005), quantile_y(&rows, 0.995));
    let own_height = bottom - top;
    if own_height <= 0.0 { return (0.0, Vec::new()); }
    let step = own_height / SUBZONE_BREAK_STEPS as f32;
    let centres: Vec<f32> = (1..SUBZONE_BREAK_STEPS).map(|i| top + step * i as f32).collect();
    let widths: Vec<f32> = SUBZONE_WIDTH_SCALES.iter().map(|s| own_height * SUBZONE_OVERLAP * s).collect();
    let mut best_r2 = 0.0f32;
    let mut models = Vec::new();
    // Every ordered pair of break centres (or one), every width at each;
    // ramps that would cross are not a partition.
    let mut consider = |breaks: Vec<f32>, widths: Vec<f32>| {
        let model = BandModel { r2: 0.0, breaks, widths, shape: None };
        if (1..model.breaks.len()).any(|i| model.ramp(i - 1).1 > model.ramp(i).0) { return; }
        let (levels, residual) = fit_levels(&model, &rows);
        let r2 = (1.0 - residual / variance).clamp(0.0, 1.0) as f32;
        best_r2 = best_r2.max(r2);
        let difference = levels.windows(2).map(|m| {
            (0..3).map(|c| (m[0][c] - m[1][c]).powi(2)).sum::<f64>().sqrt()
        }).fold(f64::INFINITY, f64::min) as f32;
        // Express the zone's 0.02 acceptance floor in Lab's 100-unit scale.
        // R2 alone would reward arbitrarily small row noise.
        if r2 <= SUBZONE_R2_MIN || difference <= ZONE_MATCHED_ERR * 100.0 { return; }
        models.push(BandModel { r2, ..model });
    };
    for (i, &a) in centres.iter().enumerate() {
        for &wa in &widths {
            consider(vec![a], vec![wa]);
            for &b in &centres[i + 1..] {
                for &wb in &widths {
                    consider(vec![a, b], vec![wa, wb]);
                }
            }
        }
    }
    // Residual R2 earns a trial, not the right to skip render gates for
    // every other partition: the best few DISTINCT partitions of each band
    // count go forward, two partitions being the same when every break is
    // within one step of the other's. Fewer bands first, then higher R2.
    models.sort_by(|a, b| b.r2.total_cmp(&a.r2));
    let mut kept: Vec<BandModel> = Vec::new();
    // The zone's own profile as the ramp, beside the measured ramps and not
    // in their quota: it is the one shape no ramp law can stand in for.
    if let Some(shaped) = shaped_model(&rows, top, bottom) {
        let (levels, residual) = fit_levels(&shaped, &rows);
        let r2 = (1.0 - residual / variance).clamp(0.0, 1.0) as f32;
        best_r2 = best_r2.max(r2);
        let difference = (0..3).map(|c| (levels[0][c] - levels[1][c]).powi(2)).sum::<f64>().sqrt() as f32;
        if r2 > SUBZONE_R2_MIN && difference > ZONE_MATCHED_ERR * 100.0 {
            kept.push(BandModel { r2, ..shaped });
        }
    }
    for count in 1..=2 {
        let mut taken = 0;
        for model in models.iter().filter(|m| m.breaks.len() == count) {
            if taken == SUBZONE_MODELS_PER_COUNT { break; }
            let same = |k: &BandModel| k.breaks.len() == count && k.shape.is_none()
                && k.breaks.iter().zip(&model.breaks).all(|(a, b)| (a - b).abs() <= step * 1.001);
            if kept.iter().any(same) { continue; }
            kept.push(model.clone());
            taken += 1;
        }
    }
    kept.sort_by(|a, b| a.breaks.len().cmp(&b.breaks.len()).then(b.r2.total_cmp(&a.r2)));
    (best_r2, kept)
}

fn band_components(model: &BandModel, band: usize) -> Vec<MaskComponent> {
    model.breaks.iter().enumerate().filter_map(|(i, _)| {
        if i != band && i + 1 != band { return None; }
        let (zero_y, full_y) = model.ramp(i);
        let geometry = match &model.shape {
            Some(falloff) if i == 0 => MaskGeometry::Shaped {
                zero_x: 0.5, zero_y, full_x: 0.5, full_y, falloff: falloff.clone(),
            },
            _ => MaskGeometry::Linear { zero_x: 0.5, zero_y, full_x: 0.5, full_y },
        };
        Some(MaskComponent { geometry, mode: MaskCombine::Intersect, inverted: i == band })
    }).collect()
}

/// The band's weights in the PREVIEW's frame (R38): its linear components
/// are transported by the recipe's lens profile exactly as `develop_preview`
/// transports them, so the ruler's band is the band the render paints.
fn band_weights(
    model: &BandModel, band: usize, image: &DynamicImage, recipe: &crate::recipe::EditRecipe,
) -> Vec<f32> {
    let coverage = render::preview_mask_coverage(&LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
        components: band_components(model, band),
        ..Default::default()
    }, image, recipe);
    coverage.as_raw().iter().map(|v| *v as f32 / 255.0).collect()
}

/// How much larger a band's solved controls must be than the solve read
/// them, for the band to deliver its level at full coverage: the inverse of
/// the band ramp's mean coverage over the band's own population (sum w*c^2 /
/// sum w*c, `w` the parent zone's weight, `c` the ramp's), capped at
/// [`BAND_LIFT_MAX`].
///
/// A correction applied through coverage `c` delivers `c` of itself at each
/// pixel; a band's controls are solved on the population its ramp reaches,
/// weighted by that ramp, so the solve matched the band's level AS SEEN
/// THROUGH the ramp and the render delivered that level times the ramp
/// again. A plateau band reads ~1 and is unchanged. A ramp still rising at
/// the zone's edge - the band shape the night pair's sky profile measured
/// (2026-10-20, `residual_models`) - reads ~0.6, and until then shipped at
/// 60 % of its level: the fitted ramp to the horizon (R2 0.83) lost to a
/// plateau band (R2 0.64) by deltaE 14.9 to 8.5 on that pair, and the
/// plateau band's ramp end was the line across the sky. The residual fit
/// chose the ramp whose SHAPE the correction should take; this gives it the
/// level that shape needs.
fn full_coverage_factor(parent: &[f32], ramp: &[f32]) -> f32 {
    let (mut c1, mut c2) = (0.0f64, 0.0f64);
    for (w, c) in parent.iter().zip(ramp) {
        let (w, c) = (*w as f64, *c as f64);
        c1 += w * c;
        c2 += w * c * c;
    }
    if c1 <= 0.0 || c2 <= 0.0 { return 1.0; }
    ((c1 / c2) as f32).clamp(1.0, BAND_LIFT_MAX)
}

/// The most a band's controls are lifted for coverage ([`full_coverage_factor`]):
/// a band whose ramp reaches its population at under a quarter coverage is
/// asking for a level nothing in the zone shows.
const BAND_LIFT_MAX: f32 = 4.0;

/// One band's controls at `factor` times its solved move from `anchor` (the
/// parent's controls, or neutral): the same blend the boundary gate shrinks
/// with ([`shrink_to_parent`]), run past 1, inside the limits the solve
/// itself works to and rounded as it rounds.
fn stretch_band(band: &mut LocalAdjustment, original: &LocalAdjustment, anchor: &LocalAdjustment, factor: f32) {
    shrink_to_parent(std::slice::from_mut(band), std::slice::from_ref(original), anchor, factor);
    let round1 = |v: f32| (v * 10.0).round() / 10.0;
    let round2 = |v: f32| (v * 100.0).round() / 100.0;
    band.exposure_ev = round2(band.exposure_ev.clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT));
    for dial in [&mut band.contrast, &mut band.highlights, &mut band.shadows, &mut band.whites, &mut band.blacks] {
        *dial = round1(*dial);
    }
    if let Some(gains) = band.color_gains {
        band.color_gains = Some(gains.map(|g| round2(g.max(0.05))));
    }
    band.saturation = clamp_zone_sat(band.saturation.round());
}

/// Golden-section steps of the band strength search: eight analysis renders
/// resolve the factor to under 2 % of its range.
const BAND_LEVEL_STEPS: usize = 6;

/// The strength a ramp band ships at, MEASURED: the band's controls are
/// stretched from `anchor` ([`stretch_band`]) by the factor in `1..=bound`
/// whose render leaves the zone's residual ([`delta_e`] over
/// `zone_weights`) smallest, the solved move itself never losing to the
/// search. The last band attached to `report` is the band, and it is left at
/// the chosen factor.
///
/// Why measured and not computed: the band was solved on the population its
/// ramp reaches, weighted by the ramp, and the render delivers the solve
/// through the ramp again, so a ramp still rising at the zone's edge ships
/// under its level ([`full_coverage_factor`], the bound). But each control
/// reads that shortfall differently - the saturation chase is closed-loop on
/// the render and already at level, the gains are solved from means and are
/// short by the ramp's mean coverage, the tone sliders sit between - so one
/// factor computed for all of them is wrong for most: on the night pair
/// (2026-10-20) the computed lift took the horizon band to +3.58 EV, past
/// the +2.5 the solve may reach, and the boundary gate then shrank the whole
/// sky to 0.415 of it.
fn level_the_ramp_needs(
    image: &DynamicImage, target: &[[f32; 3]], report: &mut FitReport, zone_weights: &[f32],
    anchor: &LocalAdjustment, bound: f32,
) -> f32 {
    if bound <= 1.0 { return 1.0; }
    let original = report.recipe.masks.last().cloned().expect("band just attached");
    let measure = |report: &mut FitReport, factor: f32| -> f32 {
        stretch_band(report.recipe.masks.last_mut().expect("band just attached"), &original, anchor, factor);
        delta_e(&fit::pixels_of(&render::develop_preview(image, &report.recipe)), target, zone_weights)
    };
    let golden = 0.618_034f32;
    let (mut lo, mut hi) = (1.0f32, bound);
    let (mut a, mut b) = (hi - golden * (hi - lo), lo + golden * (hi - lo));
    let (mut at_a, mut at_b) = (measure(report, a), measure(report, b));
    for _ in 0..BAND_LEVEL_STEPS {
        if at_a < at_b {
            hi = b; b = a; at_b = at_a;
            a = hi - golden * (hi - lo); at_a = measure(report, a);
        } else {
            lo = a; a = b; at_a = at_b;
            b = lo + golden * (hi - lo); at_b = measure(report, b);
        }
    }
    let (best, at_best) = if at_a < at_b { (a, at_a) } else { (b, at_b) };
    let at_one = measure(report, 1.0);
    let chosen = if at_best < at_one { best } else { 1.0 };
    measure(report, chosen);
    chosen
}

fn band_attachment(
    parent: &ZoneAttachment, model: &BandModel, band: usize, image: &DynamicImage,
    recipe: &crate::recipe::EditRecipe,
) -> (ZoneAttachment, f32) {
    let weights = band_weights(model, band, image, recipe);
    let lift = full_coverage_factor(&parent.source_weights, &weights);
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
    (attachment, lift)
}

/// R37. The target-referenced band gap of every rim, per evidence cell
/// ([`rim_cell_gaps`]): the MEAN over the cell's transition-band pixels of
/// the render against the target's band through the settled-sky transport,
/// in luma, the widest channel and Lab chroma. A mean, because the target's
/// texture is re-synthesised where this runs: at one pixel it is zero-mean
/// noise of several codes, and ranking it refused, on the reference pair at
/// 0.85 (2026-09-11), a band set that improved the sky's deltaE 30.4 -> 25.4
/// and every worst seam and target step, for "regressing" one cell by that
/// noise. One entry per cell of every rim, 0 where a rim has no band pixel
/// in a cell, so two readings of the same rims compare positionally.
fn seams(target: &[[f32; 3]], pixels: &[[f32; 3]], rims: &[Vec<f32>], size: (u32, u32), presence: Option<&[f32]>) -> Vec<[f32; 3]> {
    rims.iter().flat_map(|weights| rim_cell_gaps(target, pixels, weights, size.0, size.1, presence)).collect()
}

/// The same gap across each rim's 50% contour ([`step_cell_gaps`]): a hard
/// boundary has no transition band to read into, and the soft ruler's
/// transported bow is not the target's own cross-boundary step. Cells stay
/// separate: a quiet hazy segment must not disappear into a whole horizon.
fn target_steps(target: &[[f32; 3]], pixels: &[[f32; 3]], rims: &[Vec<f32>], size: (u32, u32), presence: Option<&[f32]>) -> Vec<[f32; 3]> {
    rims.iter().flat_map(|weights| step_cell_gaps(target, pixels, weights, size.0, size.1, presence)).collect()
}

/// A band set does not regress the zone's boundary when, on average over the
/// contour's occupied evidence cells, no component of either trial ruler gets
/// farther from the target by more than the one-code floor
/// ([`mean_regression`]).
///
/// The mean and not the worst cell, because a visible seam is not this
/// ruler's to catch: the boundary gate before it
/// ([`enforce_boundary_gates_with_shrink`]) ranks every crossing the
/// correction introduces and shrinks the set until none in smooth sky or haze
/// passes its ceiling. What is left to ask here is whether the boundary as a
/// whole ended up closer to the target or farther from it. The rule used to
/// be a per-cell veto as well — the seam ceiling in luma and the widest
/// channel, one JND (2.3 dE) in chroma — and on a night pair (2026-10-20) it
/// refused every sky band set the gate had passed at k 0.59-0.88, sets that
/// took the sky's deltaE 23.4 -> 7.3 and the horizon glow from blue/green 1.50
/// to 2.22 against the target's 2.20 with no seam visible at 2.5x brightness,
/// for +0.05-0.10 of chroma in one or two of the contour's cells while its
/// mean improved by 0.006-0.019.
fn boundary_does_not_regress(before: &[[f32; 3]], after: &[[f32; 3]]) -> bool {
    !before.is_empty()
        && before.len() == after.len()
        && mean_regression(before, after).iter().all(|worse| *worse <= BOUNDARY_STEP_FLOOR)
}

/// The change of each component's mean over the cells either reading
/// occupies: an unoccupied cell reads 0 in both and would only dilute.
fn mean_regression(before: &[[f32; 3]], after: &[[f32; 3]]) -> [f32; 3] {
    let occupied = before
        .iter()
        .zip(after)
        .filter(|(b, a)| b.iter().chain(a.iter()).any(|v| *v > 0.0))
        .count()
        .max(1) as f32;
    std::array::from_fn(|c| before.iter().zip(after).map(|(b, a)| a[c] - b[c]).sum::<f32>() / occupied)
}

/// The largest increase of any entry between two readings of the same rims:
/// `(entry, component, increase)`, where `entry / cells` is the rim (always
/// 0, the zone's own contour) and `entry % cells` the evidence cell. None
/// when nothing got worse, or when the readings do not pair up.
fn worst_regression(before: &[[f32; 3]], after: &[[f32; 3]]) -> Option<(usize, usize, f32)> {
    if before.is_empty() || before.len() != after.len() {
        return None;
    }
    let mut worst: Option<(usize, usize, f32)> = None;
    for (entry, (b, a)) in before.iter().zip(after).enumerate() {
        for component in 0..3 {
            let worse = a[component] - b[component];
            if worse > 0.0 && worst.is_none_or(|(_, _, w)| worse > w) {
                worst = Some((entry, component, worse));
            }
        }
    }
    worst
}

/// The disclosure of [`worst_regression`] and [`mean_regression`] for both
/// trial rulers: which rim, cell and component each got worse in and by how
/// much, then the component whose mean moved most; `none` when no cell got
/// worse.
fn regression_text(seam: Ruled, step: Ruled) -> String {
    const COMPONENTS: [&str; 3] = ["luma", "channel", "chroma"];
    let cells = crate::fit_cells::CELLS_X * crate::fit_cells::CELLS_Y;
    let one = |(worst, mean): Ruled| {
        let most = (1..3).fold(0, |most, c| if mean[c] > mean[most] { c } else { most });
        let mean = format!("mean {:+.5}@{}", mean[most], COMPONENTS[most]);
        match worst {
            Some((entry, component, worse)) => format!(
                "+{worse:.5}@rim{}/cell{}/{} ({mean})", entry / cells, entry % cells, COMPONENTS[component]
            ),
            None => format!("none ({mean})"),
        }
    };
    format!("band {}, contour {}", one(seam), one(step))
}

/// How far apart, as a share of the frame height, the two rows are that a
/// band ramp's steepness is read between ([`ramp_excess`]). Under the narrowest
/// ramp a band model is tried at on a zone of ordinary height, and about the
/// span over which the night pair's (2026-10-20) too-narrow sky ramp read as a
/// line across the sky: +12 codes of brightness and +20 of blue over 2% of the
/// height, where the target brightened 1.5 codes over the same rows.
const RAMP_WINDOW: f32 = 0.02;
/// How much steeper than the control arm a band set's rows may change before
/// the extra is a line: one just-noticeable difference, in deltaE.
const RAMP_JND: f32 = 2.3;

/// How much STEEPER than the target the render changes from one row to a row
/// [`RAMP_WINDOW`] below it, anywhere inside the zone, in deltaE: per row pair
/// the lightness and the chroma step of the render's row mean each less the
/// target's (0 where the render is no steeper), combined. Each row mean is
/// over the columns settled inside the zone at both rows, because the
/// target's texture is re-synthesised where bands run and one column of it is
/// noise.
///
/// A band set's ramps are designed gradients, so they are not judged as a
/// seam against a settled treatment; but a gradient is only as smooth as it
/// is wide, and a ramp narrower than the change it carries is a line the
/// target does not have. This is that question, and only that one: a render
/// that changes no faster than the target anywhere reads 0 whatever its
/// level. Read in deltaE and not per channel: on the night pair a sky ramp
/// four times wider carried the horizon glow's blue a few codes faster than
/// the target with no line visible at 2.2x brightness (3.9 deltaE over the
/// target's steps against 3.1 for the render without bands), where the narrow
/// one that drew the line read 8.8.
fn ramp_excess(target: &[[f32; 3]], pixels: &[[f32; 3]], weights: &[f32], (width, height): (u32, u32)) -> f32 {
    let (w, h) = (width as usize, height as usize);
    let span = ((h as f32 * RAMP_WINDOW).round() as usize).max(1);
    if w == 0 || h <= span || weights.len() < w * h || pixels.len() < w * h || target.len() < w * h {
        return 0.0;
    }
    let mut excess = 0.0f32;
    for y in 0..h - span {
        let mut means = [[0.0f32; 3]; 4];
        let mut columns = 0usize;
        for x in 0..w {
            let (upper, lower) = (y * w + x, (y + span) * w + x);
            if weights[upper] < ZONE_BOUNDARY_HIGH || weights[lower] < ZONE_BOUNDARY_HIGH {
                continue;
            }
            for (mean, p) in means.iter_mut().zip([pixels[upper], pixels[lower], target[upper], target[lower]]) {
                for c in 0..3 {
                    mean[c] += p[c];
                }
            }
            columns += 1;
        }
        // A row pair reached by a sliver of the zone is a corner, not a ramp.
        if columns * 8 < w {
            continue;
        }
        let [ru, rl, tu, tl] = means.map(|m| lab(&m.map(|v| v / columns as f32)));
        let step = |a: [f32; 3], b: [f32; 3]| ((b[0] - a[0]).abs(), (b[1] - a[1]).hypot(b[2] - a[2]));
        let ((render_l, render_c), (target_l, target_c)) = (step(ru, rl), step(tu, tl));
        excess = excess.max((render_l - target_l).max(0.0).hypot((render_c - target_c).max(0.0)));
    }
    excess
}

/// A band set keeps its ramps no steeper than the target's own rows unless
/// the control arm already was: its [`ramp_excess`] may exceed the control's
/// by at most [`RAMP_JND`].
fn ramps_follow_the_target(before: f32, after: f32) -> bool {
    after <= before + RAMP_JND
}

/// One ruler's two readings of a trial: its worst cell and its means.
type Ruled = (Option<(usize, usize, f32)>, [f32; 3]);

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
    // The parent's statistics are read over the pixels the target can pair;
    // its rim stays the raw mask, whose crossings the seam rulers skip where
    // the target is absent.
    let rim = parent.source_weights.clone();
    let presence = report.evidence.presence.clone();
    let paired = ZoneAttachment {
        source_weights: report.evidence.present_weights(&parent.source_weights).into_owned(),
        target_weights: report.evidence.present_weights(&parent.target_weights).into_owned(),
        ..parent.clone()
    };
    let parent = &paired;
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
    // Every earned partition is tried, ramps as measured, against all the
    // same gates. The smallest model wins ties; no reference-pair special case.
    let trial_count = models.len();
    // The control arm without its parent renders the same for every trial:
    // one render, not one per trial.
    let uncorrected = {
        let mut without_parent = saved_recipe.clone();
        if let Some(index) = parent_index { without_parent.masks.remove(index); }
        fit::pixels_of(&render::develop_preview(image, &without_parent))
    };
    let uncorrected_error = fit::look_err_with_evidence(&uncorrected, target, &report.evidence);
    let neutral = LocalAdjustment::default();
    for model in models {
        report.recipe = saved_recipe.clone();
        report.notes = saved_notes.clone();
        if let Some(index) = parent_index { report.recipe.masks.remove(index); }
        let mut frame_error = uncorrected_error;
        let first_band = report.recipe.masks.len();
        let attachments: Vec<_> = (0..=model.breaks.len())
            .map(|band| band_attachment(parent, &model, band, image, &report.recipe)).collect();
        // The one boundary a trial is judged at: the zone's own contour. A
        // band's ramp is not a boundary — it is the correction's designed
        // gradient between two fits of the same region, as wide as the target's
        // own profile measured it (`residual_models`), and how close it lands
        // is what `delta_e` over the zone already measures. Read as a rim it was
        // charged for falling short of the band's settled treatment, which a
        // ramp does by construction: on the night pair (2026-10-20) that held
        // every narrow-ramp sky trial at k 0.03-0.09.
        let rims = vec![rim.clone()];
        let mut solved = true;
        for (attachment, lift) in &attachments {
            if attach_one_zone(image, target, report, &mut frame_error, attachment, divergence, correspondence.as_ref()).is_none() {
                solved = false;
                break;
            }
            // The band was solved as seen through its ramp; ship it at the
            // level its ramp needs, measured (`level_the_ramp_needs`), and
            // read the frame again so the next band starts from what shipped.
            if *lift > 1.0 {
                let original = report.recipe.masks.last().cloned().expect("band just attached");
                let anchor = parent_band_anchor(&original, parent_index.map_or(&neutral, |i| &saved_recipe.masks[i]));
                level_the_ramp_needs(image, target, report, &parent.source_weights, &anchor, *lift);
                let lifted = fit::pixels_of(&render::develop_preview(image, &report.recipe));
                frame_error = fit::look_err_with_evidence(&lifted, target, &report.evidence);
            }
        }
        let mut pixels = fit::pixels_of(&render::develop_preview(image, &report.recipe));
        let mut boundary_passed = solved;
        if solved {
            let shares: Vec<f32> = attachments
                .iter()
                .map(|(a, _)| a.source_weights.iter().sum::<f32>() / a.source_weights.len().max(1) as f32)
                .collect();
            let boundaries: Vec<&[f32]> = rims.iter().map(Vec::as_slice).collect();
            let verdict = if let Some(index) = parent_index {
                let control = &saved_recipe.masks[index];
                let originals = report.recipe.masks[first_band..].to_vec();
                shrink_to_parent(&mut report.recipe.masks[first_band..], &originals, control, 0.0);
                let anchor_px = fit::pixels_of(&render::develop_preview(image, &report.recipe));
                report.recipe.masks[first_band..].clone_from_slice(&originals);
                enforce_boundary_gates_with_shrink(Some(target), image, report, &boundaries, &shares, first_band,
                    (&anchor_px, pixels), |masks, originals, _, k| shrink_to_parent(masks, originals, control, k))
            } else {
                enforce_boundary_gates(Some(target), image, report, &boundaries, &shares, first_band, &uncorrected, pixels)
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
        let seam_before = seams(target, &single_px, &rims, size, presence.as_deref());
        let seam_after = seams(target, &pixels, &rims, size, presence.as_deref());
        let step_before = target_steps(target, &single_px, &rims, size, presence.as_deref());
        let step_after = target_steps(target, &pixels, &rims, size, presence.as_deref());
        let no_seam_regression = boundary_does_not_regress(&seam_before, &seam_after)
            && boundary_does_not_regress(&step_before, &step_after);
        let ramp_before = ramp_excess(target, &single_px, &parent.source_weights, size);
        let ramp_after = ramp_excess(target, &pixels, &parent.source_weights, size);
        let ramps_follow = ramps_follow_the_target(ramp_before, ramp_after);
        let regression = format!(
            "{}, ramp steeper than target {ramp_before:.2} -> {ramp_after:.2}",
            regression_text(
                (worst_regression(&seam_before, &seam_after), mean_regression(&seam_before, &seam_after)),
                (worst_regression(&step_before, &step_after), mean_regression(&step_before, &step_after)),
            ),
        );
        let zone_before = zone_err(&zone_moments(&single_px, &parent.source_weights), &zone_moments(target, &parent.target_weights));
        let zone_after = zone_err(&zone_moments(&pixels, &parent.source_weights), &zone_moments(target, &parent.target_weights));
        let frame_after = fit::look_err_with_evidence(&pixels, target, &report.evidence);
        let reason = if !solved { values::BAND_ESTIMATOR }
            else if !boundary_passed { values::BOUNDARY_BUDGET }
            else if after >= before - ZONE_GLOBAL_REGRESSION_TOL { values::DELTA_E_MARGIN }
            else if zone_after > zone_before + ZONE_GLOBAL_REGRESSION_TOL { values::ZONE_DO_NO_HARM }
            else if frame_after > saved_error + parent.frame_regression_tol { values::FRAME_DO_NO_HARM }
            else if !no_seam_regression { values::BOUNDARY_REGRESSION }
            else if !ramps_follow { values::RAMP_STEEPER } else { "accepted" };
        let complete = solved && boundary_passed;
        let args = vec![
            ("label", parent.label.clone()), ("k", (model.breaks.len() + 1).to_string()),
            ("trials", trial_count.to_string()),
            ("r2", format!("{:.4}", model.r2)),
            ("overlap", model.widths.iter().map(|v| format!("{v:.5}")).collect::<Vec<_>>().join(",")),
            ("breaks", model.breaks.iter().map(|v| format!("{v:.5}")).collect::<Vec<_>>().join(",")),
            ("before", format!("{before:.4}")),
            ("after", if complete { format!("{after:.4}") } else { values::UNMEASURED.to_string() }),
            ("seam_before", scores_text(&seam_before)),
            ("seam_after", if complete { scores_text(&seam_after) } else { values::UNMEASURED.to_string() }),
            ("step_before", scores_text(&step_before)),
            ("step_after", if complete { scores_text(&step_after) } else { values::UNMEASURED.to_string() }),
            ("regression", if complete { regression } else { values::UNMEASURED.to_string() }),
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
        let model = BandModel { r2: 1.0, breaks: vec![0.3125], widths: vec![0.625 * SUBZONE_OVERLAP], shape: None };
        let masks = if split {
            (0..2).map(|band| {
                let (a, _) = band_attachment(&parent, &model, band, &source, &crate::recipe::EditRecipe::default());
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
    fn a_ramp_is_measured_from_the_target_where_it_starts_and_how_wide_it_is() {
        // A zone whose residual is one ramp: flat, then rising over a known
        // span, then flat again. The fit must put the break at the ramp's
        // centre and give it the ramp's width, not a preset.
        let (w, h) = (64, 256);
        let current = vec![[0.5f32; 3]; w * h];
        let profile = |y: usize| -> f32 {
            0.5 + 0.08 * render::linear_ramp_coverage((y as f32 + 0.5 - 96.0) / 64.0)
        };
        let target: Vec<_> = (0..w * h).map(|i| [profile(i / w); 3]).collect();
        let (r2, models) = residual_models(&current, &target, &vec![1.0; w * h], w as u32, h as u32);
        assert!(r2 > 0.98, "one ramp explains a one-ramp profile: {r2}");
        // The measured ramps (the zone's own profile is proposed beside them,
        // `shaped_model`, and is not what this test measures).
        let ramps = |models: &[BandModel]| -> Vec<BandModel> { models.iter().filter(|m| m.shape.is_none()).cloned().collect() };
        let models = ramps(&models);
        let best = models.iter().filter(|m| m.breaks.len() == 1).max_by(|a, b| a.r2.total_cmp(&b.r2)).unwrap();
        assert!((best.breaks[0] - 0.5).abs() <= 1.0 / 32.0, "centre at the ramp's middle: {:?}", best.breaks);
        assert!((best.widths[0] - 0.25).abs() <= 0.07, "the ramp's own width, not a preset: {:?}", best.widths);
        // The same profile four times narrower earns a four times narrower ramp.
        let narrow: Vec<_> = (0..w * h).map(|i| {
            [0.5 + 0.08 * render::linear_ramp_coverage(((i / w) as f32 + 0.5 - 120.0) / 16.0); 3]
        }).collect();
        let (_, models) = residual_models(&current, &narrow, &vec![1.0; w * h], w as u32, h as u32);
        let models = ramps(&models);
        let best = models.iter().filter(|m| m.breaks.len() == 1).max_by(|a, b| a.r2.total_cmp(&b.r2)).unwrap();
        assert!(best.widths[0] <= 0.1, "a steep profile earns a narrow ramp: {:?}", best.widths);
        // A ramp still rising at the zone's lower edge (Zero handle at row
        // 160, Full at 288 of 256) is measured as one: its Full handle lands
        // past the edge.
        let unfinished: Vec<_> = (0..w * h).map(|i| {
            [0.5 + 0.08 * render::linear_ramp_coverage(((i / w) as f32 + 0.5 - 160.0) / 128.0); 3]
        }).collect();
        let (_, models) = residual_models(&current, &unfinished, &vec![1.0; w * h], w as u32, h as u32);
        let models = ramps(&models);
        let best = models.iter().filter(|m| m.breaks.len() == 1).max_by(|a, b| a.r2.total_cmp(&b.r2)).unwrap();
        assert!(best.ramp(0).1 > 1.0, "the Full handle lies past the frame: {:?}", best.ramp(0));
    }

    #[test]
    fn a_band_is_lifted_to_the_level_its_ramp_needs() {
        let parent = vec![1.0f32; 100];
        let plateau = vec![1.0f32; 100];
        assert_eq!(full_coverage_factor(&parent, &plateau), 1.0, "a plateau band is shipped as solved");
        // A ramp from 0 to 1 over the whole population: mean coverage over
        // its own mass is sum c^2 / sum c = 2/3, so the level at full coverage is 1.5x.
        let ramp: Vec<f32> = (0..100).map(|i| i as f32 / 99.0).collect();
        assert!((full_coverage_factor(&parent, &ramp) - 1.5).abs() < 0.02);
        let outside = vec![0.0f32; 100];
        assert_eq!(full_coverage_factor(&outside, &ramp), 1.0, "no population, no lift");
        let sliver: Vec<f32> = (0..100).map(|i| i as f32 / 99.0 * 0.1).collect();
        assert_eq!(full_coverage_factor(&parent, &sliver), BAND_LIFT_MAX, "capped");
        let solved = LocalAdjustment { exposure_ev: 1.0, contrast: 10.0, saturation: 8.0,
            color_gains: Some([1.2, 1.0, 0.8]), ..Default::default() };
        let mut m = solved.clone();
        stretch_band(&mut m, &solved, &LocalAdjustment::default(), 2.0);
        assert_eq!((m.exposure_ev, m.contrast, m.saturation), (2.0, 20.0, 16.0));
        assert_eq!(m.color_gains, Some([1.4, 1.0, 0.6]));
        let mut same = solved.clone();
        stretch_band(&mut same, &solved, &LocalAdjustment::default(), 1.0);
        assert_eq!(same, solved, "a factor of one changes nothing");
        let mut capped = solved.clone();
        stretch_band(&mut capped, &solved, &LocalAdjustment::default(), 4.0);
        assert_eq!(capped.exposure_ev, ZONE_EV_LIMIT, "inside the limit the solve works to");
        // From the parent's controls, not from neutral: a band solved at the
        // parent's own level is not stretched anywhere.
        let parent = LocalAdjustment { exposure_ev: 0.5, color_gains: Some([1.1, 1.0, 0.9]), ..Default::default() };
        let mut from_parent = solved.clone();
        stretch_band(&mut from_parent, &solved, &parent, 2.0);
        assert_eq!(from_parent.exposure_ev, 1.5);
        assert_eq!(from_parent.color_gains, Some([1.3, 1.0, 0.7]));
    }

    #[test]
    fn a_ramp_band_ships_at_the_level_its_render_needs() {
        // The sky's lower part is brightened through a ramp whose Full handle
        // lies past the horizon (coverage under 0.5 everywhere in the sky);
        // the band that made the target was +1 EV, the band under test was
        // solved short, at +0.4 EV. The search must take it to where the
        // zone's residual is smallest - near +1 EV - and never past the
        // open-loop bound.
        let (source, _, parent, _) = fixture(false);
        let model = BandModel { r2: 1.0, breaks: vec![0.7], widths: vec![0.6], shape: None };
        let (band, lift) = band_attachment(&parent, &model, 1, &source, &crate::recipe::EditRecipe::default());
        assert!(lift > 1.5, "a ramp reaching its population at low coverage is short by more than half: {lift}");
        let at = |ev: f32| LocalAdjustment {
            mask: band.mask.clone(), components: band.components.clone(), role: band.role,
            exposure_ev: ev, ..Default::default()
        };
        let target = render::develop_preview(&source, &crate::recipe::EditRecipe { masks: vec![at(1.0)], ..Default::default() });
        let target_px = fit::pixels_of(&target);
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe.masks.push(at(0.4));
        let short = delta_e(&fit::pixels_of(&render::develop_preview(&source, &report.recipe)), &target_px, &parent.source_weights);
        let factor = level_the_ramp_needs(&source, &target_px, &mut report, &parent.source_weights, &LocalAdjustment::default(), lift);
        let shipped = report.recipe.masks.last().unwrap().exposure_ev;
        let after = delta_e(&fit::pixels_of(&render::develop_preview(&source, &report.recipe)), &target_px, &parent.source_weights);
        assert!(factor > 1.0 && factor <= lift, "{factor} within 1..={lift}");
        assert!((shipped - 1.0).abs() <= 0.15, "the level the render needs, not the computed one: {shipped} EV (factor {factor})");
        assert!(after < short * 0.5, "the zone residual fell: {short} -> {after}");
        // A band already at level is left alone.
        let mut report = super::super::tests::neutral_report(&source, &target);
        report.recipe.masks.push(at(1.0));
        let factor = level_the_ramp_needs(&source, &target_px, &mut report, &parent.source_weights, &LocalAdjustment::default(), lift);
        assert_eq!(factor, 1.0);
        assert_eq!(report.recipe.masks.last().unwrap().exposure_ev, 1.0);
    }

    #[test]
    fn a_glow_at_the_zones_edge_earns_a_shaped_ramp_that_follows_it() {
        // The night pair's sky profile: flat, then rising like an exponential
        // over the lowest sixth of the zone. No S-shaped ramp fits that; the
        // zone's own profile does.
        let (w, h) = (64, 256);
        let current = vec![[0.5f32; 3]; w * h];
        let glow = |y: usize| -> f32 { 0.5 + 0.12 * ((y as f32 + 0.5 - 256.0) / 20.0).exp() };
        let target: Vec<_> = (0..w * h).map(|i| [glow(i / w); 3]).collect();
        let (_, models) = residual_models(&current, &target, &vec![1.0; w * h], w as u32, h as u32);
        let shaped = models.iter().find(|m| m.shape.is_some()).expect("a shaped model is proposed");
        let best_ramp = models.iter().filter(|m| m.shape.is_none()).map(|m| m.r2).fold(0.0f32, f32::max);
        assert!(shaped.r2 > 0.97, "the zone's own profile explains the glow: {}", shaped.r2);
        assert!(shaped.r2 > best_ramp, "and better than any ramp law: {} vs {best_ramp}", shaped.r2);
        assert!(models[0].shape.is_some(), "fewest bands and highest R2 first");
        // Its falloff is the glow: 0 at the top, 1 at the bottom, convex.
        let falloff = shaped.shape.as_ref().unwrap();
        assert_eq!(falloff.len(), SUBZONE_SHAPE_SAMPLES);
        assert!(falloff[0] < 0.02 && falloff[falloff.len() - 1] > 0.98, "{falloff:?}");
        let mid = falloff[falloff.len() / 2];
        assert!(mid < 0.1, "convex: half way down the glow has barely begun: {mid}");
        // The band it composes to is a shaped gradient over the zone's extent,
        // the upper band its complement.
        let lower = band_components(shaped, 1);
        assert!(matches!(&lower[0].geometry, MaskGeometry::Shaped { falloff: f, .. } if f == falloff) && !lower[0].inverted);
        let upper = band_components(shaped, 0);
        assert!(matches!(&upper[0].geometry, MaskGeometry::Shaped { .. }) && upper[0].inverted);
        let y = 0.9;
        assert!((band_coverages(shaped, y).iter().sum::<f32>() - 1.0).abs() < 1e-5);
        // A zone whose two ends do not differ proposes no shape.
        let flat: Vec<_> = (0..w * h).map(|i| [0.5 + 0.02 * ((i / w) % 2) as f32; 3]).collect();
        let (_, models) = residual_models(&current, &flat, &vec![1.0; w * h], w as u32, h as u32);
        assert!(models.iter().all(|m| m.shape.is_none()));
    }

    #[test]
    fn earned_partitions_are_distinct_and_bounded_per_band_count() {
        let (w, h) = (64, 256);
        let current = vec![[0.5f32; 3]; w * h];
        let target: Vec<_> = (0..w * h).map(|i| {
            let y = i / w;
            [if y < 100 { 0.42 } else if y < 180 { 0.5 } else { 0.58 }; 3]
        }).collect();
        let (_, models) = residual_models(&current, &target, &vec![1.0; w * h], w as u32, h as u32);
        assert!(models.iter().filter(|m| m.shape.is_some()).count() <= 1, "at most one shaped model, outside the quota");
        for count in 1..=2 {
            let of: Vec<_> = models.iter().filter(|m| m.breaks.len() == count && m.shape.is_none()).collect();
            assert!(of.len() <= SUBZONE_MODELS_PER_COUNT);
            for (i, a) in of.iter().enumerate() {
                for b in &of[i + 1..] {
                    assert!(a.breaks.iter().zip(&b.breaks).any(|(x, y)| (x - y).abs() > 1.0 / 64.0),
                        "two trials with the same breaks: {a:?} {b:?}");
                }
            }
        }
        assert!(models.windows(2).all(|p| p[0].breaks.len() <= p[1].breaks.len()), "fewer bands first");
    }

    #[test]
    fn the_boundary_is_judged_on_its_mean_not_its_worst_cell() {
        let before = [[0.10, 0.12, 0.15], [0.30, 0.32, 0.35], [0.20, 0.22, 0.25]];
        assert!(boundary_does_not_regress(&before, &before));
        assert!(!boundary_does_not_regress(&before, &before[..2]), "a missing cell is not a pass");
        assert!(!boundary_does_not_regress(&[], &[]), "an unread boundary is not a pass");
        // One cell moves away from the target by more than the seam ceiling
        // and a JND of chroma; the other two land closer by more than that.
        // The per-cell veto refused exactly this; the mean admits it.
        let traded = [[0.113, 0.133, 0.18], [0.29, 0.31, 0.33], [0.19, 0.21, 0.23]];
        assert!(traded[0][0] - before[0][0] > ZONE_BOUNDARY_RIM_MAX && traded[0][2] - before[0][2] > 0.023,
            "premise: the first cell is past both old per-cell tolerances");
        assert!(boundary_does_not_regress(&before, &traded));
        // …and a drift every cell shares adds up: past the one-code floor on
        // average is a regression however small each cell's share.
        let drift = |worse: f32| -> [[f32; 3]; 3] {
            std::array::from_fn(|i| [before[i][0] + worse, before[i][1], before[i][2]])
        };
        assert!(boundary_does_not_regress(&before, &drift(BOUNDARY_STEP_FLOOR - 1e-4)), "a mean drift under a code is rounding");
        assert!(!boundary_does_not_regress(&before, &drift(BOUNDARY_STEP_FLOOR + 1e-4)), "past a code on average it is a regression");
        assert!((mean_regression(&before, &drift(0.003))[0] - 0.003).abs() < 1e-6);
        // Unoccupied cells dilute nothing.
        let sparse_before = [[0.10, 0.12, 0.15], [0.0; 3], [0.0; 3]];
        let sparse_after = [[0.10 + 0.005, 0.12, 0.15], [0.0; 3], [0.0; 3]];
        assert!((mean_regression(&sparse_before, &sparse_after)[0] - 0.005).abs() < 1e-6, "the mean is over occupied cells");
        assert!(!boundary_does_not_regress(&sparse_before, &sparse_after), "one occupied cell past a code is the rim's whole mean");
    }

    #[test]
    fn the_regression_disclosure_names_the_rim_cell_component_and_the_mean() {
        let cells = crate::fit_cells::CELLS_X * crate::fit_cells::CELLS_Y;
        let mut before = vec![[0.1f32; 3]; 2 * cells];
        let mut after = before.clone();
        after[cells + 46][2] += 0.0133; // rim 1, cell 46, chroma
        before[3][0] = 0.2;
        after[3][0] = 0.15; // an improvement elsewhere
        let text = regression_text(
            (worst_regression(&before, &after), mean_regression(&before, &after)),
            (None, [0.0; 3]),
        );
        assert_eq!(text, "band +0.01330@rim1/cell46/chroma (mean +0.00007@chroma), contour none (mean +0.00000@luma)");
    }

    #[test]
    fn replacement_shrink_retains_the_parent_tone_at_zero_and_the_fitted_bands_at_one() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(96, 64, Rgb([96, 112, 144])));
        let parent = LocalAdjustment { exposure_ev: 0.15, color_gains: Some([1.02, 1.0, 0.98]), ..Default::default() };
        let model = BandModel { r2: 1.0, breaks: vec![0.5], widths: vec![0.06], shape: None };
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
        assert!(boundary_does_not_regress(&seams(&target, &before, &rims, size, None),
            &seams(&target, &after, &rims, size, None)), "a hard edge has no soft-rim samples");
        assert!(!boundary_does_not_regress(&target_steps(&target, &before, &rims, size, None),
            &target_steps(&target, &after, &rims, size, None)), "the target step still got worse");
    }

    #[test]
    fn a_ramp_is_judged_by_how_much_steeper_than_the_target_it_changes() {
        let (w, h) = (64u32, 200u32);
        let weights = vec![1.0f32; (w * h) as usize];
        // A vertical profile: `level(y)` on every column.
        let frame = |level: &dyn Fn(f32) -> f32| -> Vec<[f32; 3]> {
            (0..h).flat_map(|y| std::iter::repeat_n([level(y as f32); 3], w as usize)).collect()
        };
        let ramp = |width: f32| move |y: f32| 0.2 + 0.1 * ((y - 100.0) / width + 0.5).clamp(0.0, 1.0);
        let target = frame(&ramp(100.0));
        let flat = frame(&|_| 0.2);
        let control = ramp_excess(&target, &flat, &weights, (w, h));
        assert_eq!(control, 0.0, "a flat render is never steeper than the target");
        let narrow = ramp_excess(&target, &frame(&ramp(4.0)), &weights, (w, h));
        assert!(!ramps_follow_the_target(control, narrow), "the same change over 4 rows is a line: {narrow}");
        let as_wide = ramp_excess(&target, &frame(&ramp(100.0)), &weights, (w, h));
        assert!(ramps_follow_the_target(control, as_wide), "the target's own ramp: {as_wide}");
        // The same narrow ramp where the target ITSELF steps there is the
        // target's edge, not one the bands drew.
        let stepped = frame(&ramp(4.0));
        assert!(ramps_follow_the_target(ramp_excess(&stepped, &flat, &weights, (w, h)),
            ramp_excess(&stepped, &stepped, &weights, (w, h))));
        // Rows outside the zone are not read.
        let outside = vec![0.0f32; (w * h) as usize];
        assert_eq!(ramp_excess(&target, &frame(&ramp(4.0)), &outside, (w, h)), 0.0);
    }

    #[test]
    fn one_regressing_contour_cell_is_weighed_against_the_rest_of_the_contour() {
        let (w, h) = (192usize, 96usize);
        let target: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.5; 3] } else { [0.2; 3] }).collect();
        let before: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.49; 3] } else { [0.2; 3] }).collect();
        // Eleven columns land on the target; the first moves away from it.
        let after = |column: f32| -> Vec<[f32; 3]> { (0..w*h).map(|i| if i/w < h/2 {
            if i%w < w/crate::fit_cells::CELLS_X { [column; 3] } else { [0.5; 3] }
        } else { [0.2; 3] }).collect() };
        let weights = (0..w*h).map(|i| if i/w < h/2 { 1.0 } else { 0.0 }).collect();
        let rims = vec![weights];
        let size = (w as u32, h as u32);
        let steps = |pixels: &[[f32; 3]]| target_steps(&target, pixels, &rims, size, None);
        assert!(boundary_does_not_regress(&steps(&before), &steps(&after(0.47))),
            "one column past the old per-cell ceiling while eleven land is a better contour");
        assert!(boundary_does_not_regress(&steps(&before), &steps(&target)));
        // The whole contour moving away is not.
        let away: Vec<_> = (0..w*h).map(|i| if i/w < h/2 { [0.47; 3] } else { [0.2; 3] }).collect();
        assert!(!boundary_does_not_regress(&steps(&before), &steps(&away)));
    }

    #[test]
    fn adjacent_intersection_bands_sum_to_one_for_the_measured_feather() {
        let source = DynamicImage::new_rgb8(384, 256);
        for breaks in [vec![0.3], vec![0.2, 0.5]] {
            let model = BandModel { r2: 1.0, widths: vec![0.06; breaks.len()], breaks, shape: None };
            let weights: Vec<_> = (0..=model.breaks.len())
                .map(|band| band_weights(&model, band, &source, &crate::recipe::EditRecipe::default())).collect();
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
            if (i / w / (h / 8)).is_multiple_of(2) { [0.58, 0.5, 0.42] }
            else { [0.42, 0.5, 0.58] }
        }).collect();
        let (r2, models) = residual_models(&current, &target, &vec![1.0; w * h], w as u32, h as u32);
        assert!(r2 < SUBZONE_R2_MIN, "eight alternating stripes are not two or three bands: R2={r2}");
        assert!(models.is_empty(), "a coarse band explanation must earn a majority of variance");
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
