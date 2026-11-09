//! Curves: band statistics, residual channel curves, projected cast curves, and the cast-projection search.

use super::*;

// --------------------------------------------------------------------------
// colour residuals
// --------------------------------------------------------------------------

/// Per-band accumulator: weight, circular hue (sin/cos), and the two
/// magnitudes the per-band mixer actually steers — CHROMA (max-min, exactly
/// what `apply_hsl`'s saturation axis scales: at fixed HSL lightness the
/// reconstructed chroma is 2*l*s below mid-grey and 2*s*(1-l) above it, so it
/// is proportional to `s` on both sides) and Rec.601 LUMA.
///
/// Deliberately NOT HSL's own `s` and `l`. `s` is ill-conditioned near white
/// and black — the renderer gates the whole mixer on chroma for that very
/// reason — and `l` = (max+min)/2 RISES when chroma alone rises, so reading
/// the luminance axis off it lets a band's saturation gap masquerade as a
/// brightness gap: measured on the four-family fixture, a target whose blue
/// quarter is 1.64x more chromatic at identical Rec.601 luma asked for
/// +22 luminance, which is a demand about colour wearing brightness's clothes.
#[derive(Clone, Copy, Default)]
pub(super) struct BandStat {
    pub(super) w: f64,
    pub(super) sin: f64,
    pub(super) cos: f64,
    pub(super) c: f64,
    pub(super) y: f64,
}

/// Accumulate chroma-gated band statistics with the SAME partition of unity the
/// renderer uses ([`render::bracket_bands`]), so the fit and the engine agree on
/// what "the blue band" is. Returns the per-band stats and the chromatic total.
#[cfg(test)]
pub(super) fn band_stats(px: &[[f32; 3]]) -> ([BandStat; 8], f64) {
    let mut bands = [BandStat::default(); 8];
    let mut total = 0.0f64;
    for p in px {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < 0.06 {
            continue; // matches the renderer's chroma gate: near-grey carries no hue evidence
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
        let ang = (h * std::f32::consts::TAU) as f64;
        let luma = luma601(p) as f64;
        for (bi, w) in [(b0, 1.0 - w1 as f64), (b1, w1 as f64)] {
            let b = &mut bands[bi];
            b.w += w;
            b.sin += w * ang.sin();
            b.cos += w * ang.cos();
            b.c += w * chroma as f64;
            b.y += w * luma;
        }
        total += 1.0;
    }
    (bands, total)
}

pub(super) fn band_stats_weighted(px: &[[f32; 3]], weights: &[f32]) -> ([BandStat; 8], f64) {
    let mut bands = [BandStat::default(); 8];
    let mut total = 0.0f64;
    for (i, p) in px.iter().enumerate() {
        let weight = weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
        if weight <= 0.0 {
            continue;
        }
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < 0.06 {
            continue;
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
        let ang = (h * std::f32::consts::TAU) as f64;
        let luma = luma601(p) as f64;
        for (bi, w) in [(b0, 1.0 - w1 as f64), (b1, w1 as f64)] {
            let w = w * weight;
            let b = &mut bands[bi];
            b.w += w;
            b.sin += w * ang.sin();
            b.cos += w * ang.cos();
            b.c += w * chroma as f64;
            b.y += w * luma;
        }
        total += weight;
    }
    (bands, total)
}

/// Residual per-channel CDF map (current render → target) as a channel curve —
/// the colour-cast catch-all (white balance shift, split toning the wheels/HSL
/// didn't express). Skipped when the channel already matches within tolerance.
#[cfg(test)]
pub(super) fn residual_channel_curve(cur: &[[f32; 3]], tgt: &[[f32; 3]], ch: usize) -> Vec<CurvePoint> {
    let all_a = vec![1.0; cur.len()];
    let all_b = vec![1.0; tgt.len()];
    residual_channel_curve_weighted(cur, tgt, ch, &all_a, &all_b)
}

/// The channel curves' knot inputs: the five intensities the residual
/// estimator fits a cast curve on, and the grid the joint descent shapes an
/// empty channel curve on (`joint::Coord::Knot`).
pub(super) const CHANNEL_CURVE_KNOTS_X: [f32; 5] = [0.0, 0.25, 0.50, 0.75, 1.0];

pub(super) fn residual_channel_curve_weighted(
    cur: &[[f32; 3]],
    tgt: &[[f32; 3]],
    ch: usize,
    cur_weights: &[f32],
    tgt_weights: &[f32],
) -> Vec<CurvePoint> {
    // In the channels the curve is applied to: the working space's own
    // encoded channels (`render::apply_rgb_curves`).
    let c_cdf = weighted_cdf(cur, cur_weights, |p| p[ch]);
    let t_cdf = weighted_cdf(tgt, tgt_weights, |p| p[ch]);
    if c_cdf.iter().all(|&v| v <= 0.0) || t_cdf.iter().all(|&v| v <= 0.0) {
        return Vec::new();
    }
    const XS: [f32; 5] = CHANNEL_CURVE_KNOTS_X;
    // The keep/skip decision is judged at the SOURCE DISTRIBUTION's own
    // quantiles — |Q_t(q) − Q_c(q)| — where the pixel mass actually sits.
    // Judging at the fixed intensity knots had two failure modes: identical
    // low-dynamic-range channels tripped the gate on their clipped endpoint
    // samples (emitting a non-identity curve through real pixels), and a
    // genuine narrow-band shift (0.30-0.40 → 0.40-0.50) fell BETWEEN the
    // knots and was suppressed entirely.
    let mut max_dev = 0.0f32;
    // Down to the 2nd/98th percentile — the P_CLIP band the curve itself
    // preserves: a 5% cluster shift left every 10-90 quantile untouched and
    // was suppressed. Identical distributions still read 0 everywhere.
    for &q in &[0.02f32, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.98] {
        let xc = quantile(&c_cdf, q);
        let xt = quantile(&t_cdf, q);
        max_dev = max_dev.max((xt - xc).abs());
    }
    if max_dev < 0.012 {
        return Vec::new();
    }
    let mut pts: Vec<CurvePoint> = Vec::with_capacity(XS.len());
    let (mut prev_in, mut prev_out) = (-1i32, 0i32);
    for &x in &XS {
        let f = cdf_at(&c_cdf, x);
        let y = quantile(&t_cdf, f.clamp(P_CLIP, 1.0 - P_CLIP)).clamp(0.0, 1.0);
        let input = (x * 255.0).round() as i32;
        let output = ((y * 255.0).round() as i32).max(prev_out);
        if input <= prev_in {
            continue;
        }
        pts.push(CurvePoint { input: input as u8, output: output as u8 });
        (prev_in, prev_out) = (input, output);
    }
    pts
}

/// Bound of the secant rounds below: a knot's output is a byte, so a step
/// that still moves after sixteen rounds is not converging.
const CURVE_REFINE_ROUNDS: u32 = 16;

/// Close the channel curves' loop on the FINISHED frame (v1.17.1).
///
/// [`residual_channel_curve_weighted`] reads the curve off the finished
/// frame's residual (the recipe rendered without curves, `cur`, against the
/// target), but the engine applies the curves BEFORE the mixer, the grade,
/// clarity / texture and saturation (`render::develop`), so what a curve
/// delivers on the finished frame is not the residual it was solved for: the
/// stages after it scale and bend it. This applies the curves where the
/// render applies them, re-renders the finished frame, and reads the residual
/// it still shows, knot by knot: at each knot the pixels it governs are the
/// fraction `f = F_cur(x)` of the frame, the target asks `Q_target(f)` there
/// and the finished frame delivers `Q_with(f)`; one secant step per knot (the
/// slope from the knot's last two outputs and what each delivered, starting
/// from no curve, where the delivery is `Q_cur(f)`) moves its output toward
/// the ask. Rounds run while the finished frame's look error still improves
/// by the objective's own rounding ([`FIT_QUANT`] since v1.18.0), bounded by
/// [`CURVE_REFINE_ROUNDS`]; the best round is kept, so the first curve is
/// never made worse. Curves that came back empty stay empty.
#[allow(clippy::too_many_arguments)]
pub(super) fn refine_curves_on_finished_frame(
    s_img: &DynamicImage,
    recipe: &mut EditRecipe,
    cur: &[[f32; 3]],
    tp: &[[f32; 3]],
    cur_weights: &[f32],
    tgt_weights: &[f32],
    evidence: &EvidenceModel,
) -> u32 {
    let take = |r: &EditRecipe| [r.red_curve.clone(), r.green_curve.clone(), r.blue_curve.clone()];
    let put = |r: &mut EditRecipe, c: &[Vec<CurvePoint>; 3]| {
        (r.red_curve, r.green_curve, r.blue_curve) = (c[0].clone(), c[1].clone(), c[2].clone());
    };
    let first = take(recipe);
    if first.iter().all(|c| c.is_empty()) {
        return 0;
    }
    let c_cdf: [Vec<f32>; 3] = std::array::from_fn(|ch| weighted_cdf(cur, cur_weights, |p| p[ch]));
    let t_cdf: [Vec<f32>; 3] = std::array::from_fn(|ch| weighted_cdf(tp, tgt_weights, |p| p[ch]));
    let frac = |ch: usize, input: u8| cdf_at(&c_cdf[ch], input as f32 / 255.0).clamp(P_CLIP, 1.0 - P_CLIP);
    // Per channel, per knot: the last output tried and what it delivered;
    // the first pair is "no curve", which delivers the uncurved frame.
    let mut last: [Vec<(f32, f32)>; 3] = std::array::from_fn(|ch| {
        first[ch].iter().map(|k| (k.input as f32 / 255.0, quantile(&c_cdf[ch], frac(ch, k.input)))).collect()
    });
    let mut px = pixels_of(&render::develop_preview(s_img, recipe));
    let mut best = (look_err_with_evidence(&px, tp, evidence), first);
    let mut rounds = 0;
    while rounds < CURVE_REFINE_ROUNDS {
        let mut next = best.1.clone();
        for ch in 0..3 {
            if next[ch].is_empty() {
                continue;
            }
            let w_cdf = weighted_cdf(&px, cur_weights, |p| p[ch]);
            let mut prev_out = 0i32;
            for (k, knot) in next[ch].iter_mut().enumerate() {
                let f = frac(ch, knot.input);
                let (want, got) = (quantile(&t_cdf[ch], f), quantile(&w_cdf, f));
                let y = knot.output as f32 / 255.0;
                let (y0, d0) = last[ch][k];
                let slope = if (y - y0).abs() > 1e-4 { (got - d0) / (y - y0) } else { 1.0 };
                let slope = if slope > 0.05 { slope } else { 1.0 };
                last[ch][k] = (y, got);
                let out = (((y + (want - got) / slope).clamp(0.0, 1.0) * 255.0).round() as i32).max(prev_out);
                knot.output = out.min(255) as u8;
                prev_out = out;
            }
        }
        if next == best.1 {
            break;
        }
        put(recipe, &next);
        px = pixels_of(&render::develop_preview(s_img, recipe));
        let err = look_err_with_evidence(&px, tp, evidence);
        rounds += 1;
        // A refinement step must buy more than the objective's own rounding
        // (FIT_QUANT, v1.18.0).
        if !ships(err, best.0) {
            break;
        }
        best = (err, next);
    }
    put(recipe, &best.1);
    rounds
}
