//! Tone: the paired robust tone solve, tone-slider fitting with its support, and the residual tone curve.

use super::*;

// --------------------------------------------------------------------------
// tone solve
// --------------------------------------------------------------------------

/// Robust conditional tone observations from the same evidence pixels on
/// both sides. Unlike marginal CDF pairing, these points retain the question
/// "what target value did this supported source range become?".
/// One robust paired-regression estimate of the tone map, shared by the
/// global and the zoned tone stages (one estimator, two call sites — the
/// solver family must not fork).
///
/// Identification: samples are PAIRED at equal raster index, so this is only
/// called on same-frame, same-grid pairs (the caller gates on that). The map
/// is estimated per luma bin as a Tukey-biweight IRLS MEAN (median start), so
/// a content-divergent sub-population — invented clouds, a moved subject —
/// loses weight by the estimator's own influence function instead of by a
/// hand-set mask; a plain least-squares mean would be dragged.
///
/// The returned per-pixel weights extend the verdict to the CHROMATIC
/// population (its own robust scale — a legitimate global colour edit
/// inflates every chromatic residual uniformly and must not mass-reject),
/// so the saturation and cast stages can compose them with the evidence
/// weights: evidence answers "is this pixel measurable at all", the robust
/// weight answers "is this pixel consistent with one global develop".
pub(crate) struct PairedRobustTone {
    /// Monotone map estimate: (weighted-mean x, robust y) per populated bin.
    pub points: Vec<(f32, f32)>,
    /// Evidence×robust mass behind each point — the model-selection score
    /// weights a point by the pixels that actually testify there.
    pub masses: Vec<f32>,
    /// Per-pixel robust weight on the shared raster (1.0 where not sampled).
    pub weights: Vec<f32>,
    /// Evidence-weighted share of sampled pixels with robust weight < 0.5.
    pub rejected_share: f32,
    /// Evidence luma-range labels holding at least 10% of the rejected mass.
    pub rejected_ranges: String,
}

const ROBUST_TUKEY_C: f32 = 4.685;
/// A channel at or above this is at the gamut ceiling (254/255 and 255/255 of
/// an 8-bit side both count: a JPEG's clipped white decodes as either). A
/// pixel with a channel there and its luma below it is not a tone reading —
/// its luma is where the ceiling left it, not where the map sent it — and the
/// paired arm does not sample it. Measured on the roundtrip fixture
/// (2026-10-21): the chromatic bands match the grey band's map within 0.005
/// wherever no channel clips and read it 0.028–0.053 short where one does;
/// read anyway, those pixels were the majority of their bins, the fit
/// followed them and rejected the honest orange band, and contrast solved
/// −6.1 against the truth's +18. A grey pixel at the ceiling keeps its say:
/// its luma IS the ceiling, which is what the map did there.
const TONE_CLIP_CEILING: f32 = 0.995;

/// A channel at the ceiling while the luma is not: the luma is capped, not
/// mapped.
fn luma_capped_by_ceiling(p: &[f32; 3]) -> bool {
    p[0].max(p[1]).max(p[2]) >= TONE_CLIP_CEILING && luma601(p) < TONE_CLIP_CEILING
}
const ROBUST_SCALE_FLOOR: f32 = 2.0 / 255.0;
const ROBUST_IRLS_ROUNDS: usize = 3;
/// Below this rejected share the disclosure stays silent — JPEG noise alone
/// rejects a stray pixel or two and a note for that would be crying wolf.
pub(crate) const ROBUST_REJECT_DISCLOSE_MIN: f32 = 0.02;
/// A chromatic pair is vouched only while its hue movement stays within this
/// many degrees of the class's dominant direction. One global develop moves
/// hues COHERENTLY (a cast is a smooth per-channel map, at most a few tens
/// of degrees, one way), so the residual-magnitude Tukey alone is blind to a
/// content flip that hides under a frame-wide recolour's inflated scale
/// (measured: the golden-sky fixture's 171° sky flip rode a warm rock
/// grade's scale and every pixel came back vouched). Casts stay comfortably
/// inside 60°; content flips live far outside it.
const HUE_VOUCH_COHERENCE_DEG: f32 = 60.0;
/// Enough testimony to trust a marginal map value in a luma range: an
/// absolute pixel count, deliberately NOT a frame share (1.4% of a 384-edge
/// frame is ~350 measured pixels — plenty; the share form of this gate
/// silenced whole regions). 32 matches the robust estimator's own sample
/// floor.
pub(crate) const SUPPORT_MIN_PIXELS: f32 = 32.0;

fn weighted_median_of(mut pairs: Vec<(f32, f32)>) -> f32 {
    // (value, weight); callers guarantee non-empty with positive total weight.
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = pairs.iter().map(|v| v.1).sum();
    let mut acc = 0.0;
    for &(value, weight) in &pairs {
        acc += weight;
        if acc >= 0.5 * total {
            return value;
        }
    }
    pairs.last().map(|v| v.0).unwrap_or(0.0)
}

/// A vouched pixel's move counts as CONVERGENCE when it lands strictly
/// closer to its own paired target — the shared predicate behind every
/// hue-damage guard's paired exemption (one definition, rule 09).
///
/// Asked ALONG THE AXIS of the move being judged (2026-10-21), as the cells'
/// direction test is: a global cast or mixer moves all three channels and is
/// read on the display pixel as it always was; a zone's colour probe is
/// solved with its tone zeroed, and put to the whole distance it is put to a
/// question it has no control over — the seaside street's sky needed to be
/// both darker and bluer, the colour probe made it bluer, and only 2445 of
/// its 7388 paired pixels read "closer" in RGB, so the class was withheld
/// (36% unvouched against the 5% line). In chromaticity, the projection the
/// cells read (`fit_cells::project_along`), bluer IS closer.
pub(super) fn converges_toward(
    axis: crate::fit_cells::MoveAxis,
    target: &[f32; 3],
    before: &[f32; 3],
    after: &[f32; 3],
) -> bool {
    let dist = |p: &[f32; 3]| -> f32 {
        match axis {
            crate::fit_cells::MoveAxis::Full => {
                (0..3).map(|c| (p[c] - target[c]).abs()).fold(0.0f32, f32::max)
            }
            _ => {
                let (p, t) = (
                    crate::fit_cells::project_along(axis, p),
                    crate::fit_cells::project_along(axis, target),
                );
                (0..3).map(|c| (p[c] - t[c]).abs()).fold(0.0f32, f32::max)
            }
        }
    };
    dist(after) + 1e-3 < dist(before)
}

fn tukey_weight(residual: f32, scale: f32) -> f32 {
    let u = residual / (ROBUST_TUKEY_C * scale);
    if u.abs() >= 1.0 { 0.0 } else { (1.0 - u * u) * (1.0 - u * u) }
}

/// The robust map's points, or none, for the tone stage's PAIRED arm.
///
/// Two gates, and R33 §D added the first of them. `PairingScale::Cell` means
/// the fine structure reading did not vouch that index `i` on one side IS
/// index `i` on the other; a per-pixel transport fit over unvouched pairs is
/// then a well-formed answer to a question nobody asked, and the marginal
/// (population-quantile) arm beside it is the honest estimator. The second
/// gate is the robust fit's OWN diagnostics, unchanged: enough populated bins
/// to shape a map, and a majority-consistent pairing.
///
/// Returning the empty vec rather than a flag is deliberate: `correspondence`
/// is what every downstream consumer already reads — the knot support span,
/// the score set, the colour weights, the hue voucher and the summary key all
/// follow from its length, so a scale verdict cannot be applied to the map and
/// forgotten by the support rule.
///
/// The Cell branch is not reachable through the shipped solve TODAY and the
/// unit test beside this function is the only thing that exercises it: with
/// the mode line on the fine reading (see [`COARSE_SIGMA_DIVISOR`]), a Full
/// solve has D_fine < [`DIVERGENCE_GLOBAL`], and the only other way to reach
/// `Cell` is an abstention that a 384x256 all-ones raster cannot produce
/// ([`STRUCTURE_MIN_CORE_PX`] is 100 against ~94500 core pixels). It is
/// written and pinned anyway because the coupling is the rule: whoever moves
/// the mode line must not have to remember that the tone estimator was
/// supposed to move with it.
pub(super) fn paired_correspondence(
    robust: Option<&PairedRobustTone>,
    pairing: PairingScale,
) -> Vec<(f32, f32)> {
    match robust {
        Some(r)
            if pairing == PairingScale::Pixel
                && r.points.len() >= 6
                && r.rejected_share <= 0.5 =>
        {
            r.points.clone()
        }
        _ => Vec::new(),
    }
}

/// The monotone backstop of the robust paired map: pool-adjacent-violators
/// on the bins' MASS (2026-10-21). Where two neighbouring bins reverse they
/// are pooled at their mass-weighted mean, so the bin that speaks for the
/// pixels wins. The clamp this replaces lifted every later bin to the
/// highest earlier one, which let a bin of negligible mass dictate the whole
/// map: the seaside street's sky zone holds its mass (11,000) at luma
/// 0.84–0.97 reading a monotone map 0.04 under identity, and a few mask-edge
/// pixels (bins of mass 0.0–2.9) at 0.34–0.80 reading targets up to 0.967 —
/// the clamp carried that 0.967 across every bin above it, and a sky that
/// asked for −0.18 EV solved as +1.25 EV against a flat map. `points` and
/// `masses` are index-aligned and sorted by x.
fn isotonic_by_mass(points: &mut [(f32, f32)], masses: &[f32]) {
    // Blocks of (start, end exclusive, mass, mass-weighted y).
    let mut blocks: Vec<(usize, usize, f64, f64)> = Vec::with_capacity(points.len());
    for (k, (&(_, y), &m)) in points.iter().zip(masses).enumerate() {
        let m = m.max(1e-6) as f64;
        blocks.push((k, k + 1, m, m * y as f64));
        while blocks.len() >= 2 {
            let n = blocks.len();
            let (a, b) = (blocks[n - 2], blocks[n - 1]);
            if b.3 / b.2 + 1e-9 >= a.3 / a.2 {
                break;
            }
            blocks.truncate(n - 2);
            blocks.push((a.0, b.1, a.2 + b.2, a.3 + b.3));
        }
    }
    for (start, end, m, my) in blocks {
        let y = (my / m) as f32;
        for p in &mut points[start..end] {
            p.1 = y;
        }
    }
}

pub(crate) fn paired_robust_tone(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    pair_weight: &dyn Fn(usize) -> f32,
    neutral_gated: bool,
) -> Option<PairedRobustTone> {
    // 64 bins: finer piecewise-linear resolution against smooth engine
    // curves at negligible cost (the haze fixture moved 0.0228 -> 0.0225 end
    // error on this alone — small, kept because the 8-member bin floor below
    // already keeps sparse bins out, so extra resolution costs nothing).
    const BINS: usize = 64;
    let n = sp.len().min(tp.len());
    // (x, y, evidence weight, raster index) for the tone samples.
    let mut samples: Vec<(f32, f32, f32, usize)> = Vec::new();
    for i in 0..n {
        let (s, t) = (&sp[i], &tp[i]);
        if neutral_gated && (!is_neutralish(s) || !is_neutralish(t)) {
            continue;
        }
        if luma_capped_by_ceiling(s) || luma_capped_by_ceiling(t) {
            continue;
        }
        let w = pair_weight(i);
        if w <= 0.0 {
            continue;
        }
        samples.push((luma601(s), luma601(t), w, i));
    }
    if samples.len() < 32 {
        return None;
    }
    let mut robust = vec![1.0f32; samples.len()];
    let mut points: Vec<(f32, f32)> = Vec::new();
    let mut masses: Vec<f32> = Vec::new();
    for round in 0..=ROBUST_IRLS_ROUNDS {
        // Map estimate under the current weights: per-bin Tukey-weighted mean
        // (round 0 starts from the weighted MEDIAN — the influence function
        // needs a resistant start or the first residuals are already dragged).
        let mut bins: Vec<Vec<usize>> = vec![Vec::new(); BINS];
        for (k, &(x, ..)) in samples.iter().enumerate() {
            bins[((x * BINS as f32).floor() as usize).min(BINS - 1)].push(k);
        }
        points.clear();
        masses.clear();
        for members in &bins {
            let total: f32 = members.iter().map(|&k| samples[k].2 * robust[k]).sum();
            if members.len() < 8 || total <= 1e-4 {
                continue;
            }
            masses.push(total);
            let y = if round == 0 {
                weighted_median_of(
                    members.iter().map(|&k| (samples[k].1, samples[k].2)).collect(),
                )
            } else {
                members.iter().map(|&k| samples[k].1 * samples[k].2 * robust[k]).sum::<f32>()
                    / total
            };
            let x = members.iter().map(|&k| samples[k].0 * samples[k].2 * robust[k]).sum::<f32>()
                / total;
            points.push((x, y));
        }
        if points.len() < 2 {
            return None;
        }
        let mut order: Vec<usize> = (0..points.len()).collect();
        order.sort_by(|&a, &b| points[a].0.total_cmp(&points[b].0));
        points = order.iter().map(|&k| points[k]).collect();
        masses = order.iter().map(|&k| masses[k]).collect();
        // Monotone backstop: a real tone map is monotone; bin noise is not
        // allowed to fake a reversal the slider model would then chase.
        isotonic_by_mass(&mut points, &masses);
        if round == ROBUST_IRLS_ROUNDS {
            break;
        }
        let residuals: Vec<f32> = samples
            .iter()
            .map(|&(x, y, ..)| (y - sample_tone_points(&points, x)).abs())
            .collect();
        let scale = (1.4826
            * weighted_median_of(
                residuals.iter().zip(&samples).map(|(&r, s)| (r, s.2)).collect(),
            ))
        .max(ROBUST_SCALE_FLOOR);
        for (w, &r) in robust.iter_mut().zip(&residuals) {
            *w = tukey_weight(r, scale);
        }
    }
    // Verdict for EVERY paired pixel (the chromatic population included) via
    // the RGB transport residual: scale the source pixel by the fitted luma
    // gain and measure the worst channel miss. Chromatic pixels get their own
    // robust scale — a global saturation/WB edit moves all of them together
    // and only pixels far off THAT bulk are inconsistent.
    let mut weights = vec![1.0f32; n];
    // (residual, index, weight, chromatic?): the two classes get SEPARATE
    // robust scales. The transport residual models only the luma gain, so a
    // legitimate saturation/WB edit inflates every CHROMATIC residual
    // together — under one shared scale the neutral pixels' near-zero
    // residuals drag the median down and the whole chromatic population
    // (exactly the saturation evidence) is systematically down-weighted
    // (measured on the haze fixture: the colour stages came back empty).
    // Within its own class, a uniform edit clusters around the class median
    // and keeps weight; only pixels far off THEIR OWN bulk reject.
    let mut all_residuals: Vec<(f32, usize, f32, bool, Option<f32>)> = Vec::new();
    let (mut dir_sin, mut dir_cos) = (0.0f64, 0.0f64);
    for i in 0..n {
        let w = pair_weight(i);
        if w <= 0.0 {
            continue;
        }
        let (s, t) = (&sp[i], &tp[i]);
        let l = luma601(s);
        let gain = sample_tone_points(&points, l).clamp(0.0, 1.0) / l.max(1e-4);
        let residual = (0..3)
            .map(|c| (t[c] - (s[c] * gain).clamp(0.0, 1.0)).abs())
            .fold(0.0f32, f32::max);
        // Hue movement of the pair, where BOTH sides carry measurable hue —
        // the coherence voucher's raw material.
        let s_chroma = s[0].max(s[1]).max(s[2]) - s[0].min(s[1]).min(s[2]);
        let t_chroma = t[0].max(t[1]).max(t[2]) - t[0].min(t[1]).min(t[2]);
        let hue_delta = (s_chroma >= 0.06 && t_chroma >= 0.06).then(|| {
            let sh = render::rgb_to_hsl(s[0], s[1], s[2]).0 * 360.0;
            let th = render::rgb_to_hsl(t[0], t[1], t[2]).0 * 360.0;
            let mut d = th - sh;
            while d > 180.0 { d -= 360.0; }
            while d < -180.0 { d += 360.0; }
            d
        });
        if let Some(d) = hue_delta {
            let rad = (d as f64).to_radians();
            dir_sin += w as f64 * rad.sin();
            dir_cos += w as f64 * rad.cos();
        }
        all_residuals.push((residual, i, w, !is_neutralish(s), hue_delta));
    }
    if all_residuals.is_empty() {
        return None;
    }
    let class_dir = dir_sin.atan2(dir_cos).to_degrees() as f32;
    let scale_of = |chromatic: bool| -> f32 {
        let class: Vec<(f32, f32)> = all_residuals
            .iter()
            .filter(|&&(_, _, _, c, _)| c == chromatic)
            .map(|&(r, _, w, ..)| (r, w))
            .collect();
        if class.is_empty() {
            ROBUST_SCALE_FLOOR
        } else {
            (1.4826 * weighted_median_of(class)).max(ROBUST_SCALE_FLOOR)
        }
    };
    let scales = [scale_of(false), scale_of(true)];
    let mut rejected = 0.0f32;
    let mut total = 0.0f32;
    let mut range_rejected = [0.0f32; EVIDENCE_LUMA_BINS];
    for &(residual, i, w, chromatic, hue_delta) in &all_residuals {
        let scale = scales[chromatic as usize];
        let coherent = hue_delta.is_none_or(|d| {
            let mut dev = (d - class_dir).abs() % 360.0;
            if dev > 180.0 { dev = 360.0 - dev; }
            dev <= HUE_VOUCH_COHERENCE_DEG
        });
        let rw = if coherent { tukey_weight(residual, scale) } else { 0.0 };
        weights[i] = rw;
        total += w;
        if rw < 0.5 {
            rejected += w;
            range_rejected[evidence_luma_bin(luma601(&sp[i]))] += w;
        }
    }
    let rejected_share = if total > 0.0 { rejected / total } else { 0.0 };
    let rejected_ranges = range_rejected
        .iter()
        .enumerate()
        .filter(|&(_, &mass)| rejected > 0.0 && mass >= 0.10 * rejected)
        .map(|(bin, _)| {
            let lo = bin as f32 / EVIDENCE_LUMA_BINS as f32;
            let hi = (bin + 1) as f32 / EVIDENCE_LUMA_BINS as f32;
            format!("luma[{lo:.2}-{hi:.2}]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(PairedRobustTone { points, masses, weights, rejected_share, rejected_ranges })
}

pub(crate) fn sample_tone_points(points: &[(f32, f32)], x: f32) -> f32 {
    let Some(&(first_x, first_y)) = points.first() else { return x };
    if x <= first_x {
        return first_y + (x - first_x);
    }
    for pair in points.windows(2) {
        if x <= pair[1].0 {
            let t = (x - pair[0].0) / (pair[1].0 - pair[0].0).max(1e-6);
            return pair[0].1 + t * (pair[1].1 - pair[0].1);
        }
    }
    let &(last_x, last_y) = points.last().unwrap();
    last_y + (x - last_x)
}

/// Per-knot data support for an arbitrary weighted population (the zoned
/// fit's form of the global path's support closure): with a usable paired
/// map, a knot is supported inside the span the map points cover; otherwise
/// it needs [`SUPPORT_MIN_PIXELS`] of weight mass in its luma range — a
/// count, never a share (see the global closure's doc). A knot outside the
/// population must not pull the spline over the region it does occupy.
pub(crate) fn knot_support_for(
    px: &[[f32; 3]],
    weights: &[f32],
    points: &[(f32, f32)],
) -> [f32; 8] {
    if points.len() >= 6 {
        let (lo, hi) = (points[0].0 - 1.0 / 32.0, points[points.len() - 1].0 + 1.0 / 32.0);
        return std::array::from_fn(|i| {
            let x = render::TONE_KNOTS_X[i];
            if x >= lo && x <= hi { 1.0 } else { 0.0 }
        });
    }
    let mut mass = [0.0f32; EVIDENCE_LUMA_BINS];
    for (p, &w) in px.iter().zip(weights) {
        if w > 0.0 {
            mass[evidence_luma_bin(luma601(p))] += w;
        }
    }
    std::array::from_fn(|i| {
        if mass[evidence_luma_bin(render::TONE_KNOTS_X[i])] >= SUPPORT_MIN_PIXELS {
            1.0
        } else {
            0.0
        }
    })
}

/// Magnitude prior for the tone solve. The 5-slider knot system is
/// near-collinear (contrast vs shadows/highlights, whites vs the shoulder), so
/// unpenalised least squares happily returns huge mutually-cancelling sliders
/// whose TOTAL map ties a tasteful solution to within numerical ε — and the
/// residual curve erases even that difference. The prior makes slider
/// magnitude itself part of the cost, so "Exposure +1.5, Contrast −97,
/// Shadows −100" loses to the mild solve it was shadowing. Units: basis
/// authorities are O(0.2–0.34), knot residuals O(0.1); 0.02 prices a pegged
/// slider (s=1) like a ~0.14 luma miss at one knot — strong enough to kill
/// cancellation combos, weak enough that genuinely-needed big moves survive
/// (the roundtrip test pins recovery of a real ±25-point recipe).
pub(super) const TONE_PRIOR: f64 = 0.01;

/// Scan exposure (nonlinear in the model) and, for each candidate, solve the 5
/// linear sliders (contrast/highlights/shadows/whites/blacks, in the basis
/// order of [`render::tone_slider_basis`]) by RIDGE least squares over the 8
/// knots; keep the (ev, sliders) minimising the PENALISED clamped-solution
/// score `SSE + TONE_PRIOR·Σs²` — the same prior in the solve and in the
/// model selection, so the exposure scan cannot smuggle the degeneracy back.
#[cfg(test)]
pub(crate) fn fit_tone_sliders(tone_map: &impl Fn(f32) -> f32) -> (f32, [f32; 5]) {
    fit_tone_sliders_supported(tone_map, &[1.0; 8], &[])
}

/// [`fit_tone_sliders`] with per-knot DATA support composed into the knot
/// weights. Engine authority says how far a slider can move a knot; support
/// says whether any measured pixel testifies there. A knot with no testimony
/// contributes nothing to the solve or the model-selection score, so the
/// exposure scan cannot buy phantom-knot fit either. Fewer than two supported
/// knots is no tone problem at all — return neutral instead of solving a
/// one-point system.
pub(crate) fn fit_tone_sliders_supported(
    tone_map: &impl Fn(f32) -> f32,
    support: &[f32; 8],
    score_set: &[(f32, f32, f32)],
) -> (f32, [f32; 5]) {
    if support.iter().filter(|&&s| s > 0.0).count() < 2 {
        return (0.0, [0.0; 5]);
    }
    // The engine cannot output past [0,1]; an estimated map may extrapolate
    // there, and an unclamped target would price impossible demand into the
    // supported knots' shared sliders.
    let targets: Vec<f32> =
        render::TONE_KNOTS_X.iter().map(|&x| tone_map(x).clamp(0.0, 1.0)).collect();
    let basis: Vec<[f32; 5]> =
        render::TONE_KNOTS_X.iter().map(|&x| render::tone_slider_basis(x)).collect();

    let mut best = (0.0f32, [0.0f32; 5], f32::INFINITY);
    let mut ev = -3.0f32;
    while ev <= 3.0 + 1e-6 {
        // Residual after the exposure component, then ridge normal equations.
        // Knot authority (`tone_knot_weights`) rides the basis rows: it
        // depends only on the candidate ev, so the system stays linear in the
        // sliders — and the solve models the SAME engine that will render the
        // result (an unweighted basis would ask saturated knots to explain
        // residual they can no longer move).
        let authority = render::tone_knot_weights(ev);
        let weights: [f32; 8] = std::array::from_fn(|i| authority[i] * support[i]);
        let resid: Vec<f64> = render::TONE_KNOTS_X
            .iter()
            .zip(&targets)
            .map(|(&x, &t)| (t - render::tone_exposure_curve(x, ev)) as f64)
            .collect();
        let mut ata = [[0.0f64; 5]; 5];
        let mut atb = [0.0f64; 5];
        for ((b, r), &w) in basis.iter().zip(&resid).zip(&weights) {
            for i in 0..5 {
                let bi = (w * b[i]) as f64;
                for j in 0..5 {
                    ata[i][j] += bi * (w * b[j]) as f64;
                }
                atb[i] += bi * r;
            }
        }
        for (i, row) in ata.iter_mut().enumerate() {
            row[i] += TONE_PRIOR; // ridge = the magnitude prior (see const doc)
        }
        let sol = solve5(ata, atb);
        let s: [f32; 5] = std::array::from_fn(|i| (sol[i] as f32).clamp(-1.0, 1.0));
        let penalty: f64 = s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum();
        // Model selection. With a paired score set, the candidate is judged
        // through the ENGINE'S OWN spline at the robust map points, weighted
        // by the pixel mass behind each point — the 8-knot residual cannot
        // tell near-collinear (ev, sliders) combinations apart (their splines
        // agree AT the knots and differ between them, where the pixels live),
        // and the magnitude prior then tie-breaks toward the small-slider
        // impostor (measured: the roundtrip truth ev+0.35/highlights −25 lost
        // to ev+0.20/highlights +8). Normalised to the 8-knot scale so
        // TONE_PRIOR keeps its calibrated strength.
        let score: f64 = if score_set.is_empty() {
            // Weighted-least-squares scoring, consistent with the normal
            // equations above: the row's weight multiplies the WHOLE
            // residual, so a zero-weight knot contributes nothing. The old
            // form weighted only the model half ((r − w·fit)²), which
            // charged every candidate a zero-authority knot's RAW residual —
            // and since that charge varies with ev, the scan minimised
            // phantom-knot residuals no slider could touch (the unit gate
            // test caught it: identity-on-supported solved to ev +0.40).
            basis
                .iter()
                .zip(&resid)
                .zip(&weights)
                .map(|((b, r), &w)| {
                    let fit: f64 = (0..5).map(|i| b[i] as f64 * s[i] as f64).sum();
                    let d = w as f64 * (r - fit);
                    d * d
                })
                .sum::<f64>()
                + penalty
        } else {
            let knots = render::tone_model_knots(ev, s);
            let mut sse = 0.0f64;
            let mut mass = 0.0f64;
            for &(x, y, m) in score_set {
                let d = (render::sample_tone_model(&knots, x) - y.clamp(0.0, 1.0)) as f64;
                sse += m as f64 * d * d;
                mass += m as f64;
            }
            if mass > 0.0 { 8.0 * sse / mass + penalty } else { penalty }
        };
        if (score as f32) < best.2 {
            best = (ev, s, score as f32);
        }
        ev += 0.05;
    }
    // NOT limited here on purpose. `render::limit_tone_sliders` saturates a
    // slider vector that would flatten a tonal band, and the engine applies it
    // at render time — but applying it to the PROPOSAL as well perturbs this
    // least-squares solve, and the acceptance test downstream is a knife edge:
    // on the hazy-to-clean fixture as it stood pre-R17 (gated evidence,
    // sparse residual knots) the solve was only 3 % better than neutral
    // (0.08625 against 0.08918), so a 0.34 % nudge to the sliders pushed it
    // over `err_before`, tripped the saturation do-no-harm loop, and ended at
    // 0.1286 — far worse than doing nothing. (R17's evidence fallback moved
    // that fixture to 0.0892 → 0.0229; the numbers above are kept as the
    // historical record of WHY the asymmetry exists — the knife-edge
    // geometry, not the exact figures, is the reason.) The fit does not need
    // to predict the limiter anyway: it scores candidates by RENDERING them
    // (`develop_preview` below), so it already measures whatever the engine
    // actually does.
    (best.0, best.1)
}

/// Score of one `(ev, sliders)` candidate against a weighted score set — the
/// score-set branch of [`fit_tone_sliders_supported`]'s model selection,
/// verbatim: the engine's own spline at each point, mass-weighted, normalised
/// to the 8-knot scale, plus the magnitude prior.
fn tone_score_on(score_set: &[(f32, f32, f32)], ev: f32, s: [f32; 5]) -> f64 {
    let knots = render::tone_model_knots(ev, s);
    let mut sse = 0.0f64;
    let mut mass = 0.0f64;
    for &(x, y, m) in score_set {
        let d = (render::sample_tone_model(&knots, x) - y.clamp(0.0, 1.0)) as f64;
        sse += m as f64 * d * d;
        mass += m as f64;
    }
    let penalty: f64 = s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum();
    if mass > 0.0 { 8.0 * sse / mass + penalty } else { penalty }
}

/// Refine a knot solve's `(ev, sliders)` on the engine's RENDERED response at
/// a score set. Why it exists: [`fit_tone_sliders_supported`] solves the
/// sliders by least squares at the eight fixed knots, and a zone whose
/// population sits inside one or two knot intervals has a map whose bend the
/// knots never see — measured on the neon street's building (luma
/// 0.066–0.25, only the knots 0 / 0.10 / 0.25 near it): the map wanted p10
/// 0.068 → 0.044 and the knot solve rendered 0.078, the x = 0 knot (an
/// extrapolation below the population) dragging the blacks UP. The score set
/// samples the population itself, so minimising the score set's own score
/// fixes what the knots miss. It is done on the rendered response, not on
/// the knot ys, because the engine's limiter, authority weights and
/// Fritsch–Carlson Hermite make the response nonlinear in the sliders.
///
/// Deterministic coordinate descent over the six coordinates (ev in [−3, 3],
/// each slider in [−1, 1]): each coordinate tries `±step` and keeps an
/// improvement; a pass that keeps nothing halves the step; stop below
/// 1e-3 or after [`REFINE_PASS_CAP`] passes. Never worse than the input
/// (only improvements are accepted). An empty score set returns the input.
///
/// Caller: the zoned fit's distribution branch only, whose score set is the
/// zone population's quantiles through a deterministic transport. A paired
/// score set carries pairing noise the refinement would chase.
pub(crate) fn refine_tone_sliders_on(
    score_set: &[(f32, f32, f32)],
    ev: f32,
    sliders: [f32; 5],
) -> (f32, [f32; 5]) {
    if score_set.is_empty() {
        return (ev, sliders);
    }
    let mut x = [ev, sliders[0], sliders[1], sliders[2], sliders[3], sliders[4]];
    let split = |x: &[f32; 6]| (x[0], [x[1], x[2], x[3], x[4], x[5]]);
    let mut best = {
        let (e, s) = split(&x);
        tone_score_on(score_set, e, s)
    };
    let mut step = 0.25f32;
    for _ in 0..REFINE_PASS_CAP {
        if step < 1e-3 {
            break;
        }
        let mut accepted = false;
        for c in 0..6 {
            let limit = if c == 0 { 3.0 } else { 1.0 };
            for dir in [1.0f32, -1.0] {
                let mut trial = x;
                trial[c] = (x[c] + dir * step).clamp(-limit, limit);
                if trial[c] == x[c] {
                    continue;
                }
                let (e, s) = split(&trial);
                let score = tone_score_on(score_set, e, s);
                if score < best {
                    best = score;
                    x = trial;
                    accepted = true;
                    break;
                }
            }
        }
        if !accepted {
            step *= 0.5;
        }
    }
    split(&x)
}

/// Pass cap for [`refine_tone_sliders_on`]; 0.25 halves to below 1e-3 in
/// eight idle passes, so the cap only bounds a long accepting run.
const REFINE_PASS_CAP: usize = 40;

/// Gaussian elimination with partial pivoting for the 5×5 normal equations.
fn solve5(mut a: [[f64; 5]; 5], mut b: [f64; 5]) -> [f64; 5] {
    for c in 0..5 {
        let mut p = c;
        for r in c + 1..5 {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        a.swap(c, p);
        b.swap(c, p);
        if a[c][c].abs() < 1e-12 {
            continue;
        }
        let pivot = a[c]; // copy of the pivot row ([f64; 5] is Copy)
        for r in c + 1..5 {
            let f = a[r][c] / pivot[c];
            for k in c..5 {
                a[r][k] -= f * pivot[k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = [0.0f64; 5];
    for c in (0..5).rev() {
        let mut acc = b[c];
        for k in c + 1..5 {
            acc -= a[c][k] * x[k];
        }
        x[c] = if a[c][c].abs() < 1e-12 { 0.0 } else { acc / a[c][c] };
    }
    x
}

/// Whatever tonal shape the sliders could not express, as `tone_curve` control
/// points. The engine composes `tone_curve` AFTER the knot spline `S`, so the
/// exact residual curve is `M ∘ S⁻¹` — i.e. points `(S(x), M(x))`. Monotone by
/// construction (both `S` and `M` are monotone); skipped when the residual is
/// within tolerance everywhere.
#[cfg(test)]
fn residual_tone_curve_with_samples(
    recipe: &EditRecipe,
    tone_map: &impl Fn(f32) -> f32,
    extra_xs: &[f32],
    supported: &impl Fn(f32) -> bool,
) -> Vec<CurvePoint> {
    residual_tone_curve_with_budget(recipe, tone_map, extra_xs, supported, (0.0, RESIDUAL_SLOPE_CAP))
}

pub(super) fn residual_tone_curve_with_budget(
    recipe: &EditRecipe,
    tone_map: &impl Fn(f32) -> f32,
    extra_xs: &[f32],
    supported: &impl Fn(f32) -> bool,
    slope: (f32, f32),
) -> Vec<CurvePoint> {
    debug_assert!(recipe.tone_curve.is_empty(), "fit the residual before setting a curve");
    let lut = render::build_tone_lut(recipe);
    // Knot placement (R17): uniform in the LUT's OUTPUT domain, inverted
    // back through the LUT — the curve's input axis IS the engine's output
    // (`sx` below), so sampling uniform in raw x inherits the base curve's
    // compression. On the real camera base the old fixed 9 xs left a single
    // 38-u8 input gap right across the band holding the frame's tonal mass,
    // and the curve's PIECEWISE-LINEAR rendering (`render::curve_lut` →
    // `interp` — not the monotone cubic the knot spline uses) chords
    // ~10/255 below the concave desired map inside it (measured, P20
    // × reimagine). 13 output levels bound the inter-knot input gap to
    // ~21 u8 wherever the LUT moves; where it is flat the levels collapse
    // onto one x and the `prev_in` dedup keeps the point list minimal —
    // which also means a flat plateau's interior is no longer sampled by
    // `max_dev` (the old fixed xs could land mid-plateau): deliberate, a
    // many-to-one plateau is beyond any input-side curve's reach anyway.
    // The trade's cost side: 21-u8 spacing doubles the density of u8-rounded
    // control points, ~±0.5/255 of quantisation ripple bought against the
    // ~10/255 of chord sag removed — a 20:1 win. Evidence-withheld luma
    // boundaries are added by the caller so interpolation cannot bridge a
    // refusal with a neighboring supported move.
    const LEVELS: usize = 13;
    let mut xs: Vec<f32> = (0..LEVELS).map(|i| {
        let o = i as f32 / (LEVELS - 1) as f32;
        let idx = lut.partition_point(|&v| v < o).min(lut.len() - 1);
        idx as f32 / (lut.len() - 1) as f32
    }).chain(extra_xs.iter().copied()).collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let mut max_dev = 0.0f32;
    let mut pts: Vec<CurvePoint> = Vec::with_capacity(LEVELS);
    let (mut prev_in, mut prev_out) = (-1i32, 0i32);
    for x in xs {
        if !supported(x) {
            continue; // no source pixels there — the map is extrapolation
        }
        let sx = render::sample_lut(&lut, x); // engine output before the residual curve
        let y = tone_map(x).clamp(0.0, 1.0); // desired output
        max_dev = max_dev.max((y - sx).abs());
        let input = (sx * 255.0).round() as i32;
        let output = ((y * 255.0).round() as i32).max(prev_out); // keep monotone
        if input <= prev_in {
            continue; // spline outputs can quantise together at the ends
        }
        pts.push(CurvePoint { input: input as u8, output: output as u8 });
        (prev_in, prev_out) = (input, output);
    }
    if max_dev < 0.015 {
        Vec::new() // the sliders already express the map — keep the recipe clean
    } else {
        project_curve_slopes(&pts, slope.0, slope.1)
    }
}

#[cfg(test)]
pub(super) fn residual_tone_curve(recipe: &EditRecipe, tone_map: &impl Fn(f32) -> f32) -> Vec<CurvePoint> {
    residual_tone_curve_with_samples(recipe, tone_map, &[], &|_| true)
}
