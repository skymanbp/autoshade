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

pub(super) fn residual_channel_curve_weighted(
    cur: &[[f32; 3]],
    tgt: &[[f32; 3]],
    ch: usize,
    cur_weights: &[f32],
    tgt_weights: &[f32],
) -> Vec<CurvePoint> {
    let c_cdf = weighted_cdf(cur, cur_weights, |p| p[ch]);
    let t_cdf = weighted_cdf(tgt, tgt_weights, |p| p[ch]);
    if c_cdf.iter().all(|&v| v <= 0.0) || t_cdf.iter().all(|&v| v <= 0.0) {
        return Vec::new();
    }
    const XS: [f32; 5] = [0.0, 0.25, 0.50, 0.75, 1.0];
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

/// Shrink the three fitted channel curves along one continuous path, so a
/// cast the hue-fan gate convicts can be made milder instead of thrown away.
///
/// `t = 1` is the fitted cast, `t = 0` is no cast at all, and the path
/// between them gives up the CHROMATIC part first:
///
/// ```text
///   L      = per-knot mean of the three fitted outputs (the shape all three
///            channels share; one curve applied to every channel)
///   dC     = C − L                      (each channel's chromatic deviation)
///   C(t)   = x + min(1, 2t)·(L − x) + max(0, 2t − 1)·dC
///   t = 1  → C          (as fitted)
///   t = 0.5→ L          (one shared curve, no chromatic difference at all)
///   t = 0  → x          (the identity: no curves)
/// ```
///
/// The upper half is the projection proper — the fan is a RELATIONAL defect
/// (at each input level the three curves hold three different outputs, and
/// that per-level difference is the chromatic move that sorts one hue class
/// by luminance), so the first thing to give up is `dC`.
///
/// The LOWER half exists because measurement said it had to. The design this
/// implements stopped at `L`, on the premise that one curve applied to all
/// three channels cannot fan a hue class. That premise is false, and the
/// showcase pair is where it fails: hue is a RATIO, so a shared curve moves
/// it wherever its slope changes, and the Cornwall shared shape's segment
/// slopes are 0.172 / 0.859 / 1.127 / 0.188 (its top segment is nearly flat
/// because the fitted red curve clips at 179 from input 191 up). Measured on
/// that shape (2026-09-02): a dark sky pixel moves +0.2°, a mid one −3.2°, a
/// bright one −20.1° as its blue channel is crushed toward the other two —
/// and the census reads 17.3° of ADDED fan at `t = 0.5`, above [`FAN_DEG`]
/// itself. A family whose mildest member is still convicted cannot rescue
/// anything, so the path continues to the identity, where the fan is zero by
/// construction and the outcome is exactly today's refusal.
///
/// It stays three Lightroom RGB curves at every `t`, so the recipe still
/// round-trips to XMP, and it is the same idiom one stage up — shrink until
/// the finished frame stops objecting (the mixer's halve-and-refit
/// do-no-harm loop).
///
/// An EMPTY channel curve is the identity and is resampled as one: a channel
/// whose residual fell under `residual_channel_curve_weighted`'s keep
/// threshold while the other two carry a cast is still a channel, and
/// dropping it would make `L` the mean of a different number of channels.
/// It comes back out the way it went in — a projected curve that lands on the
/// IDENTITY at every knot is emitted EMPTY, per channel, exactly as the fit
/// leaves a channel it never fitted. Without that, `t = 1` handed an empty
/// channel back as an explicit five-knot identity curve: a dead curve in the
/// recipe and in the XMP, and `cast_curves_are_identity` would not catch it
/// because it only bails when ALL THREE channels are dead. Pinned by the
/// empty-channel leg of `the_bottom_of_the_projection_path_is_one_curve_then_none`.
///
/// Outputs are rounded and monotone-clamped exactly as the fitted curves are
/// (round to the 0..255 code, never below the previous knot), so a projected
/// curve is the same KIND of object the fit emits, not a finer one.
pub(super) fn projected_cast_curves(curves: [&[CurvePoint]; 3], t: f32) -> [Vec<CurvePoint>; 3] {
    let mut xs: Vec<u8> = curves.iter().flat_map(|c| c.iter().map(|p| p.input)).collect();
    xs.sort_unstable();
    xs.dedup();
    // Linear resample onto the shared grid. On every production input this is
    // a no-op — `residual_channel_curve_weighted` emits the same fixed knots
    // for every channel it keeps — but the mean of three curves is only
    // meaningful knot-by-knot, so pairing by INDEX rather than by input would
    // be a silent bug the day the knot sets ever differ.
    let sample = |c: &[CurvePoint], x: u8| -> f32 {
        let Some(first) = c.first() else { return x as f32 };
        if x <= first.input {
            return first.output as f32;
        }
        for w in c.windows(2) {
            if x <= w[1].input {
                let span = (w[1].input as f32 - w[0].input as f32).max(1e-6);
                let f = (x as f32 - w[0].input as f32) / span;
                return w[0].output as f32 + f * (w[1].output as f32 - w[0].output as f32);
            }
        }
        c[c.len() - 1].output as f32
    };
    let toward_shared = (2.0 * t).clamp(0.0, 1.0);
    let toward_fitted = (2.0 * t - 1.0).clamp(0.0, 1.0);
    let mut out: [Vec<CurvePoint>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut prev = [0i32; 3];
    for &x in &xs {
        let ys = [sample(curves[0], x), sample(curves[1], x), sample(curves[2], x)];
        let shared = (ys[0] + ys[1] + ys[2]) / 3.0;
        for (channel, (&y, floor)) in out.iter_mut().zip(ys.iter().zip(prev.iter_mut())) {
            let value = x as f32
                + toward_shared * (shared - x as f32)
                + toward_fitted * (y - shared);
            let code = (value.round() as i32).clamp(0, 255).max(*floor);
            channel.push(CurvePoint { input: x, output: code as u8 });
            *floor = code;
        }
    }
    // Per channel: a curve that is the identity at every knot is no curve.
    // Emitting it EMPTY is what makes `t = 1` reproduce the FITTED curves
    // byte for byte including the channels the fit left empty, and it keeps a
    // projected recipe free of curves that do nothing.
    for channel in out.iter_mut() {
        if channel.iter().all(|p| p.input == p.output) {
            channel.clear();
        }
    }
    out
}

/// Are these three curves the identity at every knot — i.e. is this candidate
/// "no cast at all"? The bottom of the projection path is the identity by
/// construction, and shipping it would put a sentence about projected curves
/// on a recipe whose curves do nothing. An EMPTY curve is the identity, which
/// is why the all-empty bottom of the path answers `true` here.
pub(super) fn cast_curves_are_identity(curves: &[Vec<CurvePoint>; 3]) -> bool {
    curves.iter().all(|c| c.iter().all(|p| p.input == p.output))
}

/// The projection SEARCH, over `t` alone: among the ADMISSIBLE shrinks — all
/// four gates clear and the fan is no more than [`FAN_PROJECT_DEG`] — the one
/// that buys the MOST look error, shipped when that gain clears [`FIT_QUANT`].
/// `None` when nothing on the path qualifies, and then the stage refuses the
/// cast exactly as it did before the projection existed.
///
/// THREE PHASES, and the shape of each follows from what is monotone in `t`
/// and what is not.
///
/// PHASE 1 finds the admissible FRONTIER `t_max` by bisection, 12 steps — the
/// convention this file already uses for closed-loop searches — because
/// admissibility is the DOWNWARD-CLOSED half of the question and the only
/// half a bisection may be pointed at: the fan is non-decreasing in `t`
/// (measured, `the_projected_fan_grows_with_t`) and every gate clears as the
/// curves go to the identity.
///
/// PHASE 2 sweeps a fixed grid of [`PROJECT_GRID`] cells over `(0, t_max]`
/// and keeps the admissible probe with the largest GAIN, the frontier
/// included. The gain is NOT monotone in `t`, and that was measured before it
/// was assumed: on the coast fixture's stage-4 candidate (2026-09-02) it
/// reads 0.00104 at `t = 0.25`, 0.00190 at 0.35, 0.00169 at 0.40 and 0.00187
/// at 0.50 — a wiggle the size of [`FIT_QUANT`] itself. Judging the frontier
/// ALONE therefore refused pairs on which an admissible paying shrink exists,
/// and that was v1.2.3's stated cost: on the two-family HSL pair every
/// `t ≤ 0.25` is admissible and pays 0.0019–0.0033 while the frontier reads
/// a gain of −0.012, so the pair was refused although the path held a rescue
/// worth more than the quantisation budget. Sweeping the interval is what
/// closes that, and the sweep is also what makes the gain bar sound: the bar
/// is now applied to the MAXIMUM over the admissible set, so a `None` really
/// does mean "nothing on this path pays".
///
/// PHASE 3 refines the winning cell by golden section, [`PROJECT_REFINE`]
/// iterations, so the answer is not quantised to the grid. Every probe in
/// both phases is re-judged by all four gates from scratch and only an
/// admissible probe can win, so neither the sweep nor the refinement can walk
/// out of the admissible set even where the RENDERED fan is not exactly
/// monotone: an inadmissible probe scores `-inf` and the bracket closes away
/// from it.
///
/// The GAIN requirement is this function's own and is stricter than the
/// ratio gate. The gates decide whether a cast the fit MEASURED may ship;
/// they do not decide whether a milder one the fit INVENTED is worth
/// shipping, and the stage's standing doctrine is that marginal gain does not
/// earn regional risk. So a projected candidate has to buy more absolute look
/// error than [`FIT_QUANT`], the fit's own quantisation budget — the same
/// number the terminal do-no-harm check uses to decide that a difference is a
/// difference at all.
///
/// Pinned by `the_search_takes_the_best_paying_admissible_shrink`, which
/// builds a path whose gain peaks in the interior and asserts that the search
/// lands on the peak rather than on the frontier.
pub(super) fn search_cast_projection_t(
    err_without: f32,
    mut judge: impl FnMut(f32) -> CastOutcome,
) -> Option<(f32, CastOutcome)> {
    // An ABSTAINING census CLEARS the fan target, and that is a reading rather
    // than a hole in one: `hue_fan_weighted` returns `None` when no hue class
    // holds [`FAN_SHARE`] of the population across two populated luma slices,
    // so there is no longer anything region-sized that COULD be over the
    // target. It reaches the user as `FIT_NOTE_CAST_PROJECTED_FAN_NA`, which
    // prints no digit, so an abstention is never published as 0.0 either.
    let admissible = |out: &CastOutcome| {
        !out.refused()
            && out
                .readings
                .is_some_and(|r| r.fan.is_none_or(|fan| fan <= FAN_PROJECT_DEG))
    };
    // PHASE 1 — the admissible FRONTIER, by bisection over the one
    // downward-closed half of the question.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut frontier: Option<(f32, CastOutcome)> = None;
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        let out = judge(mid);
        if admissible(&out) {
            frontier = Some((mid, out));
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (t_max, frontier_out) = frontier?;
    let gain_of = |out: &CastOutcome| out.readings.map(|r| err_without * (1.0 - r.ratio));
    // Judge one `t`, keep it when it is admissible and pays more than the best
    // so far, and score an inadmissible probe `-inf` so the golden section
    // closes away from it. `best` is a parameter rather than a capture so this
    // closure borrows `judge` alone.
    let mut probe = |t: f32, best: &mut (f32, CastOutcome, f32)| -> f32 {
        let out = judge(t);
        if !admissible(&out) {
            return f32::NEG_INFINITY;
        }
        // `admissible` has already established that the readings are present.
        let Some(gain) = gain_of(&out) else { return f32::NEG_INFINITY };
        if gain > best.2 {
            *best = (t, out, gain);
        }
        gain
    };
    // PHASE 2 — the gain sweep over `(0, t_max]`. The frontier is already
    // judged, so it seeds the comparison without a second render.
    let mut best = (t_max, frontier_out, gain_of(&frontier_out)?);
    let cell = t_max / PROJECT_GRID as f32;
    for k in 1..PROJECT_GRID {
        probe(cell * k as f32, &mut best);
    }
    // PHASE 3 — golden section on the cell either side of the winner, in the
    // classic three-point form: one new render per iteration, the other
    // interior point carried over from the last one.
    const INV_PHI: f32 = 0.618_034;
    let (mut a, mut b) = ((best.0 - cell).max(0.0), (best.0 + cell).min(t_max));
    let (mut c, mut d) = (b - INV_PHI * (b - a), a + INV_PHI * (b - a));
    let (mut fc, mut fd) = (probe(c, &mut best), probe(d, &mut best));
    for _ in 0..PROJECT_REFINE {
        if fc >= fd {
            b = d;
            d = c;
            fd = fc;
            c = b - INV_PHI * (b - a);
            fc = probe(c, &mut best);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + INV_PHI * (b - a);
            fd = probe(d, &mut best);
        }
    }
    // The gain bar, once, on the MAXIMUM over the admissible set.
    (best.2 > FIT_QUANT).then_some((best.0, best.1))
}

/// The projection search wired to the RENDERER: `search_cast_projection_t`
/// with a `judge` that puts `C(t)` into the recipe, develops it and hands the
/// PIXELS to the gate — never the curves. The census has to read what the user
/// will see, the same rule every other closed-loop stage in this file follows,
/// and `gate` runs all four gates, so the projection is a search the gates
/// referee and not a way around them.
///
/// It is reached from TWO call sites, and both of them produce the recipe the
/// user gets: the `fit_cast_stage` call after the mixer's do-no-harm block,
/// and the 4b do-no-harm loop's re-fit, which REPLACES that recipe one
/// saturation step down. The mixer's own do-no-harm comparison judges both of
/// its branches with the cast the gates MEASURED — see the note at that loop.
///
/// The second call site is EXERCISED and its success is UNREACHABLE. Measured
/// 2026-09-02 by instrumenting the 4b loop body and running the whole library
/// battery with the calibration corpus present: the body runs 186 times and
/// 9 of those re-fits carry a fan-convicted cast — and every one of the 9
/// is rotation-blocked as well, so `earns_projection` answers `None` and this
/// function is not reached from there at all. At the FIRST call site the two
/// gates do come apart (67 fan-only refusals in 545 stage runs), so what
/// couples them is the stepped-down state, not the gates. This function's own
/// arithmetic is pinned by
/// `the_search_takes_the_best_paying_admissible_shrink`; what no test in the
/// tree witnesses is a PROJECTED cast coming back out of that loop, and the
/// census at the loop says why that is a dead end rather than a gap.
///
/// On return `recipe` carries the WINNING candidate's curves, not the last
/// probe's — the loop's final probe is a rejected `t` more often than not.
pub(super) fn search_cast_projection(
    s_img: &DynamicImage,
    recipe: &mut EditRecipe,
    fitted: [&[CurvePoint]; 3],
    err_without: f32,
    gate: impl Fn(&[[f32; 3]]) -> CastOutcome,
) -> Option<(f32, CastOutcome)> {
    let (t, out) = search_cast_projection_t(err_without, |t| {
        let [red, green, blue] = projected_cast_curves(fitted, t);
        recipe.red_curve = red;
        recipe.green_curve = green;
        recipe.blue_curve = blue;
        gate(&pixels_of(&render::develop_preview(s_img, recipe)))
    })?;
    let curves = projected_cast_curves(fitted, t);
    if cast_curves_are_identity(&curves) {
        return None;
    }
    let [red, green, blue] = curves;
    recipe.red_curve = red;
    recipe.green_curve = green;
    recipe.blue_curve = blue;
    Some((t, out))
}

/// How many white balances the cast→WB hand-off tries, and the farthest
/// along the curves' direction it may look. The walk's EXTENT is found first:
/// `λ` doubles from 1 until the point it names inverts onto an edge of the
/// Kelvin domain ([`WB_SEARCH_K`]) or reaches [`HANDOFF_REACH_MAX`], and the
/// candidates are then `HANDOFF_STEPS` even steps of that extent. Doubling to
/// the domain rather than stopping at a fixed `λ` is measured, not assumed:
/// on the neon street pair the estimator reads the curves' pull as mostly
/// "less green", the tint is already at +100 there, so only the red/blue part
/// moves the Kelvin — `λ = 2` reached 13900 K where the cast it was asked to
/// take back wants the domain's 40000 K edge. The strength budget, checked by
/// the caller on every candidate, stops the walk earlier on a frame whose
/// budget is tighter than the domain. Candidates that invert onto the same
/// `(K, tint)` are judged once.
pub(super) const HANDOFF_STEPS: usize = 24;
pub(super) const HANDOFF_REACH_MAX: f32 = 64.0;

/// The direction in white-balance space the as-fitted cast curves pull,
/// read by the white-balance stage's OWN estimator
/// ([`atmosphere_wb_from_populations`]) with the render the curves were
/// fitted on as the source and the render WITH them as the target: per
/// brightness range, each pixel's red and blue log change against its green,
/// the mass-weighted median across ranges. The result is the estimator's
/// normalised gain triple as log gains (unit geometric mean — chromaticity
/// only). `None` when the estimator finds no change in common.
///
/// The estimator and not a mean, because a mean is not a white-balance
/// reading. Measured on the neon street pair (2026-10-23): the ratio of the
/// evidence-weighted mean LINEAR channel values read
/// `[+0.222, −0.030, +0.287]` — blue up more than red, so the walk went
/// COOLER — because the curves' blue lift in the shadows outweighs its crush
/// of the few bright pixels in a linear sum, while the cast the fan gate
/// convicted is a warm one. The per-pixel ratio read range by range is the
/// question a white balance answers, and it is the question the stage that
/// failed to answer it here was asked.
pub(super) fn cast_wb_direction(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    weights: &[f32],
    anchor: f32,
) -> Option<[f32; 3]> {
    let (_, _, wanted) = atmosphere_wb_from_populations(cur, with_px, weights, anchor);
    let direction = wanted.map(|g| g.max(1e-6).ln());
    (direction.iter().all(|v| v.is_finite()) && direction.iter().any(|v| v.abs() > 1e-4))
        .then_some(direction)
}

/// One white balance the cast→WB hand-off judged admissible: the curves
/// re-fitted under it, the four cast gates' verdict on them, the paired
/// cells' verdict on the white balance itself, the finished frame's look
/// error, and how much white-balance-shaped work the re-fitted curves still
/// do ([`cast_wb_direction`] read on their own move, as a log-gain length).
#[derive(Clone, Debug)]
pub(super) struct CastHandOff {
    pub(super) wb: (f32, f32),
    /// Saturation as chased again on `wb`.
    pub(super) saturation: f32,
    pub(super) curves: [Vec<CurvePoint>; 3],
    pub(super) outcome: CastOutcome,
    pub(super) vouch: crate::fit_cells::CellVouch,
    pub(super) err: f32,
    pub(super) pull: f32,
}

/// What the report says about a hand-off that shipped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HandOffFacts {
    pub(super) from: (f32, f32),
    pub(super) to: (f32, f32),
    pub(super) share: f32,
    pub(super) fan_before: f32,
    pub(super) fan_after: f32,
    pub(super) vouch: crate::fit_cells::CellVouch,
}

/// The cast→WB hand-off SEARCH (2026-10-23), tried BEFORE the projection
/// when the fan gate alone convicts the as-fitted curves.
///
/// A fan is what three independent channel curves do when the move they are
/// asked for is a change of illuminant the white balance did not make: each
/// channel's keep/skip and clip is decided on its own
/// ([`residual_channel_curve_weighted`]), so one channel can be left alone
/// while the other two carry the cast, and a hue class is sorted apart by
/// luminance. Measured on the neon street pair: at the 10100 K the
/// per-pixel median read, green got no curve and blue was clamped at 127,
/// the deep reds of the bright slices turned orange (+44.9° of fan, limit
/// 15) and the projection kept t = 0.398 of the cast — one shared curve, no
/// colour at all; from 11700 K up the re-fitted curves give green a curve
/// too and read +6.1° falling to +2.6° at 40000 K. Only a white balance
/// moves every channel of every pixel by the same ratio, so the red push is
/// its job.
///
/// The walk: from the shipped white balance `from` along the log-gain
/// `direction` the as-fitted curves pull ([`cast_wb_direction`]), each point
/// inverted onto the engine's (Kelvin, tint) manifold for its own Kelvin by
/// the estimator's inversion, which keeps it inside [`WB_SEARCH_K`], over the
/// extent [`HANDOFF_STEPS`] describes. `judge` renders a candidate, chases
/// saturation and re-fits the curves on it, and returns `Some` only when
/// every gate admits it.
///
/// SELECTION: the look error decides, and `pull` only breaks ties inside
/// the fit's own quantisation band. Let `best` be the lowest look error among
/// the admissible candidates; every candidate within [`FIT_QUANT`] of it is a
/// tie — a difference the terminal do-no-harm check would not call a
/// difference — and among the ties the one with the least `pull` ships. It
/// ships only when it buys more than [`FIT_QUANT`] of look error over the
/// frame without curves, the projection's own gain bar, so the two rescues
/// are held to one standard. `None` = the stage projects exactly as it did
/// before the hand-off existed.
///
/// `pull` is how much white-balance-shaped work the re-fitted curves are
/// still doing: the white-balance stage's own estimator
/// ([`cast_wb_direction`]) read on the curves' own move — the candidate
/// render without them against the same render with them — as the L2 length
/// of its log-gain triple. Zero means the curves change no illuminant at all.
/// The tie-break exists because the hand-off exists to give the white balance
/// the white-balance-shaped work; inside the band the look error cannot tell
/// the candidates apart, so it is the reason for the stage that picks.
///
/// Why not the lowest look error alone, measured on the neon street pair
/// (2026-10-23, every candidate re-chased and re-fitted): from 13100 K to
/// 40000 K the admissible look errors lie between 0.03651 and 0.03868, a
/// plateau whose structure is the curves' keep/skip decisions (the step at
/// 17050 K), not the white balance. Its minimum, 15900 K, shipped a frame
/// whose mean CIE76 ΔE to the target was 8.29 — worse than the projection's
/// 8.22 — with the building +3.4 a* past its target; 40000 K, inside the
/// band (0.03780 against the best 0.03651) with the least pull (0.24 against
/// 0.43 at 15900 K), finished at 7.52. 25350 K (0.03832) falls just outside
/// the band.
pub(super) fn search_cast_hand_off(
    anchor: f32,
    from: (f32, f32),
    direction: [f32; 3],
    err_without: f32,
    mut judge: impl FnMut((f32, f32)) -> Option<CastHandOff>,
) -> Option<CastHandOff> {
    let start = render::wb_gains(anchor, from.0, from.1).map(|v| v.max(1e-5).ln());
    let at = |lambda: f32| {
        let want: [f32; 3] = std::array::from_fn(|c| start[c] + lambda * direction[c]);
        super::evidence::invert_wb_log_gains(anchor, want)
    };
    let on_edge = |(k, _): (f32, f32)| k <= WB_SEARCH_K.0 || k >= WB_SEARCH_K.1;
    let mut reach = 1.0f32;
    while reach < HANDOFF_REACH_MAX && !on_edge(at(reach)) {
        reach *= 2.0;
    }
    let mut tried: Vec<(f32, f32)> = vec![from];
    let mut admitted: Vec<CastHandOff> = Vec::new();
    for k in 1..=HANDOFF_STEPS {
        let wb = at(reach * k as f32 / HANDOFF_STEPS as f32);
        if tried.contains(&wb) {
            continue;
        }
        tried.push(wb);
        admitted.extend(judge(wb));
    }
    pick_cast_hand_off(admitted, err_without)
}

/// The selection rule of [`search_cast_hand_off`], pure so it is testable on
/// its own: the least `pull` among the candidates within [`FIT_QUANT`] of
/// the lowest look error, shipped only when it beats `err_without` by more
/// than [`FIT_QUANT`]. The first of equal pulls wins, so the walk's order
/// (nearest white balance first) breaks an exact tie.
pub(super) fn pick_cast_hand_off(
    admitted: Vec<CastHandOff>,
    err_without: f32,
) -> Option<CastHandOff> {
    let best = admitted.iter().map(|c| c.err).fold(f32::INFINITY, f32::min);
    let mut pick: Option<CastHandOff> = None;
    for candidate in admitted {
        if candidate.err <= best + FIT_QUANT
            && pick.as_ref().is_none_or(|p| candidate.pull < p.pull)
        {
            pick = Some(candidate);
        }
    }
    pick.filter(|p| err_without - p.err > FIT_QUANT)
}
