//! The asks-carried tone: the control-validity law's brightness asks, stated as one master curve the strength dial admits.

use super::*;

/// The bound on the projection's sweeps. At most seventeen knots settle in a
/// few hundred (518 at most over the property test's two hundred generated
/// asks, `rel1180/tools/asks_curve_study.py`); the bound is numerical and
/// reads no photograph, and a projection it cuts short is still carried into
/// the set ([`project`]).
const PROJECTION_SWEEPS: usize = 20_000;
/// A sweep that moves no knot AND none of the correction steps by this many
/// codes has settled. The iterate alone is not the test: it can stand still
/// for many sweeps while the steps still move (Dykstra's stalling), and a
/// rule that read only the iterate stopped the property test's case 8 after
/// two sweeps, 6.1 codes outside the set.
const PROJECTION_SETTLED: f64 = 1e-7;
/// A knot the line through the kept knots passes within half a code of
/// rounds onto that line, and the curve is straight lines between its knots
/// (`render::curve_lut`): it is not a knot, and it is not priced as one.
const COLLINEAR_CODES: f64 = 0.5;

/// THE asks-carried tone (J142, 2026-11-11): the tone's third carrier in
/// Atmosphere mode, beside the exposure with its quantile curve and that
/// curve alone (`atmosphere::fit_atmosphere_from_parts`).
///
/// `cell_asks` is the law's own reading of the frame WITHOUT the tone
/// (`validity::Cells::luma_asks`): per brightness range with an ask, its
/// display level, the ask in ln linear luma, its mass. Each becomes a knot —
/// input the level, output that level moved by the ask — and the knots are
/// projected onto the curves the fit may ship:
///
/// * no range is moved PAST its ask or AGAINST it: the output stays between
///   the input and the asked output (the law admits `|move − ask| ≤ |ask| +
///   se`; the part of that a proposal may count on without the error bars is
///   `0 ≤ move ≤ ask`);
/// * every segment's slope, the two end segments included, stays inside the
///   strength dial's slope budget (`FitBudget::slope`);
/// * no segment that touches a HELD knot rises steeper than 1 (J155). A knot
///   is held where its range's ask is a population's and the fit withholds:
///   the law refuses a tone whose shape raises such a range's contrast
///   (`validity::Cells::stretched`, J154), a range's members lie on the two
///   segments at its knot, and a slope of at most 1 on both is the part of
///   that reading a proposal may count on without its error bars;
/// * among those, the mass-weighted least squares to the asks — the law's own
///   misfit (`Cells::misfit`), in display codes.
///
/// The set is convex and holds the identity, so the projection exists and is
/// one curve; Dykstra's alternating projections reach it. The knots the line
/// through the others passes within half a code of are then dropped, and the
/// rest are rounded to the nearest code — so each promise holds to a code: a
/// range ends at most half a code (a kept knot) or one (a dropped one) past
/// its ask, a segment's rise at most one code outside its slope bounds. A
/// held range's promise holds exactly: a knot is dropped only where the
/// chord that replaces its two segments is no steeper than the least bound
/// of the segments it stands for ([`chord_inside`]), and the inputs are
/// whole codes, so a chord that rises at most its run before the rounding
/// rises at most its run after it.
/// Empty where nothing asks or the asks are the identity — and where every
/// knot is held: from the fixed start to the fixed end at slopes of at most
/// 1 there is only the identity. The caller still offers the start there
/// (`atmosphere`: the frame the other classes leave, its tone absent) — the
/// hold changes what the start states, not whether it is solved.
///
/// Why the hold (J155, 2026-11-14). Until then the knots were the raw asks
/// under the budget alone. A population's asks of two neighbouring ranges
/// that differ ARE a contrast: on the checker fixture
/// (`tone_stage_keeps_withheld_luma_range_at_identity`) the top's lower
/// ranges ask down and its upper ranges up, the curve followed them (189 to
/// 182, 230 to 243: a slope of 1.49), J154's reading refused it, and the
/// repair scaled the WHOLE tone toward the base — the paired ranges under it
/// lost a correction no reading had refused (0.0467 to 0.0405, where the
/// curve read 0.0094 before J154). One ruler built the proposal and another
/// judged it. The proposal now states only what the law admits; the reading
/// in G stays the judge of every other start.
///
/// Why the stage needs it. A bounded point can only scale the shape it is
/// handed (user ruling 2026-11-09), and the stage's two shapes are the
/// exposure and three quantile knots. On a pair whose asks differ by
/// brightness range more than three knots can say — the different-photo pair
/// of the calibration corpus asks its ground −2.6 ln and its sky's top
/// nothing — the law refuses either shape at the ranges it overruns and the
/// repair scales the whole tone toward the base: the pair shipped J* 0.158591
/// of a base 0.165416 where the last release's recipe, which the law refuses
/// for darkening the sky by 0.36–0.47 ln, read 0.061139. Read on that pair
/// with the rest of the shipped recipe (`gates/j142-cands-c`, `-d`): the asks
/// as knots, unprojected, are admitted on every cell at J* 0.054399 but break
/// the slope budget (0.14 to 4.14 against 0.357 to 2.357); pushed through
/// [`super::project_curve_slopes`], which spreads a clamped slope's rise over
/// every segment, they overrun the range at 0.88–0.94 (move 0.0592 against an
/// ask of 0.0145) and are refused; projected as above they are admitted at
/// J* 0.058465 on six knots (F 0.071765; with all fourteen knots the same
/// curve reads 0.058402 at F 0.086902).
pub(super) fn asks_tone_curve(cell_asks: &[(f32, f32, f32, bool)], min_slope: f32, max_slope: f32) -> Vec<CurvePoint> {
    let knots: Vec<(f64, f64, f64, bool)> = cell_asks
        .iter()
        .filter(|&&(level, ask, mass, _)| level.is_finite() && ask.is_finite() && mass > 0.0)
        .map(|&(level, ask, mass, held)| {
            let x = (level * 255.0).round().clamp(1.0, 254.0);
            let moved = render::linear_to_srgb(render::srgb_to_linear(x / 255.0) * ask.exp()).clamp(0.0, 1.0);
            (x as f64, (moved * 255.0) as f64, mass as f64, held)
        })
        .collect();
    curve_within(&knots, min_slope as f64, max_slope as f64)
}

/// [`curve_within`] on knots `(input code, asked output code, mass)`, none
/// of them held.
#[cfg(test)]
fn curve_through(knots: &[(f64, f64, f64)], smin: f64, smax: f64) -> Vec<CurvePoint> {
    let free: Vec<(f64, f64, f64, bool)> = knots.iter().map(|&(x, t, w)| (x, t, w, false)).collect();
    curve_within(&free, smin, smax)
}

/// The upper slope bound of each of the `n + 1` segments of a chain of `n`
/// knots (0: the fixed start to knot 0; `i`: knot `i − 1` to knot `i`; `n`:
/// knot `n − 1` to the fixed end): 1 where the segment touches a held knot,
/// the budget's elsewhere.
fn segment_caps(held: &[bool], smax: f64) -> Vec<f64> {
    let n = held.len();
    (0..=n).map(|j| if (j >= 1 && held[j - 1]) || (j < n && held[j]) { smax.min(1.0) } else { smax }).collect()
}

/// [`asks_tone_curve`] on knots `(input code, asked output code, mass, held)`.
fn curve_within(knots: &[(f64, f64, f64, bool)], smin: f64, smax: f64) -> Vec<CurvePoint> {
    // The identity must be a curve of the set, or the set may be empty.
    if !(smin > 0.0 && smin <= 1.0 && smax >= 1.0) {
        return Vec::new();
    }
    // One knot per input code: ranges that share a level speak as one, by
    // mass, and are held if either is.
    let mut sorted = knots.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64, f64, bool)> = Vec::new();
    for (x, t, w, held) in sorted {
        match merged.last_mut() {
            Some(last) if last.0 == x => {
                last.1 = (last.1 * last.2 + t * w) / (last.2 + w);
                last.2 += w;
                last.3 |= held;
            }
            _ => merged.push((x, t, w, held)),
        }
    }
    let xs: Vec<f64> = merged.iter().map(|k| k.0).collect();
    let ts: Vec<f64> = merged.iter().map(|k| k.1).collect();
    let ws: Vec<f64> = merged.iter().map(|k| k.2).collect();
    let held: Vec<bool> = merged.iter().map(|k| k.3).collect();
    let caps = segment_caps(&held, smax);
    let asked: Vec<(f64, f64)> = xs.iter().zip(&ts).map(|(&x, &t)| (x.min(t), x.max(t))).collect();
    let bounds = feasible(&xs, &asked, smin, &caps);
    let y = project(&xs, &ts, &ws, &bounds, smin, &caps);
    let keep = kept_knots(&xs, &y, &caps);
    // A chord's slope is a mean of its segments', so the kept chain is inside
    // the budget wherever the full one was, and inside a held range's bound
    // by `chord_inside`. Rounded to the NEAREST code: a
    // rounding held strictly inside the bounds from the knot before it drifts
    // along a chain of knots at a slope bound, a code a knot, and carried a
    // dropped knot's range two codes past its ask.
    let mut points = vec![CurvePoint { input: 0, output: 0 }];
    points.extend(keep.iter().map(|&i| CurvePoint { input: xs[i] as u8, output: y[i].round().clamp(0.0, 255.0) as u8 }));
    points.push(CurvePoint { input: 255, output: 255 });
    if points.iter().all(|p| p.output == p.input) {
        return Vec::new();
    }
    points
}

/// The exact interval of every knot on its chain: the asked intervals and
/// the slope bounds — `smin` below, each segment's own bound above
/// ([`segment_caps`]) — carried from the fixed start to the fixed end and
/// back. A value inside a knot's interval extends to a whole curve of the set.
fn feasible(xs: &[f64], asked: &[(f64, f64)], smin: f64, caps: &[f64]) -> Vec<(f64, f64)> {
    let mut b = asked.to_vec();
    let (mut px, mut plo, mut phi) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..xs.len() {
        let dx = xs[i] - px;
        b[i] = (b[i].0.max(plo + smin * dx), b[i].1.min(phi + caps[i] * dx));
        (px, plo, phi) = (xs[i], b[i].0, b[i].1);
    }
    let (mut nx, mut nlo, mut nhi) = (255.0f64, 255.0f64, 255.0f64);
    for i in (0..xs.len()).rev() {
        let dx = nx - xs[i];
        b[i] = (b[i].0.max(nlo - caps[i + 1] * dx), b[i].1.min(nhi - smin * dx));
        (nx, nlo, nhi) = (xs[i], b[i].0, b[i].1);
    }
    b
}

/// Dykstra's alternating projections of `ts` onto the knots' intervals and
/// the segments' slope bounds, in the masses' metric — and then one pass
/// that carries the result INTO the set whatever the sweeps returned: each
/// knot, in order, held in its exact interval and in the slope bounds from
/// the knot before it (a value in a knot's interval extends to a whole curve
/// of the set, [`feasible`], so the pass never runs out of room). Settled,
/// the sweeps' limit is the closest curve of the set and the pass moves
/// nothing; the two promises do not depend on the sweeps having settled.
fn project(xs: &[f64], ts: &[f64], ws: &[f64], bounds: &[(f64, f64)], smin: f64, caps: &[f64]) -> Vec<f64> {
    let n = xs.len();
    let hold = |i: usize, v: f64| v.max(bounds[i].0).min(bounds[i].1);
    let settled = |a: f64, b: f64| (a - b).abs() < PROJECTION_SETTLED;
    let mut y = ts.to_vec();
    let mut interval_step = vec![0.0f64; n];
    let mut slope_step = vec![(0.0f64, 0.0f64); n.saturating_sub(1)];
    for _ in 0..PROJECTION_SWEEPS {
        let (before, interval_before, slope_before) = (y.clone(), interval_step.clone(), slope_step.clone());
        for i in 0..n {
            let z = y[i] + interval_step[i];
            y[i] = hold(i, z);
            interval_step[i] = z - y[i];
        }
        for i in 0..n.saturating_sub(1) {
            let (a, b) = (y[i] + slope_step[i].0, y[i + 1] + slope_step[i].1);
            let dx = xs[i + 1] - xs[i];
            let rise = b - a;
            let smax = caps[i + 1];
            let excess = if rise > smax * dx {
                rise - smax * dx
            } else if rise < smin * dx {
                rise - smin * dx
            } else {
                0.0
            };
            let (ia, ib) = (1.0 / ws[i], 1.0 / ws[i + 1]);
            let (pa, pb) = (a + excess * ia / (ia + ib), b - excess * ib / (ia + ib));
            slope_step[i] = (a - pa, b - pb);
            (y[i], y[i + 1]) = (pa, pb);
        }
        if y.iter().zip(&before).all(|(p, q)| settled(*p, *q))
            && interval_step.iter().zip(&interval_before).all(|(p, q)| settled(*p, *q))
            && slope_step.iter().zip(&slope_before).all(|(p, q)| settled(p.0, q.0) && settled(p.1, q.1))
        {
            break;
        }
    }
    let (mut px, mut py) = (0.0f64, 0.0f64);
    for i in 0..n {
        let dx = xs[i] - px;
        y[i] = y[i].max(bounds[i].0.max(py + smin * dx)).min(bounds[i].1.min(py + caps[i] * dx));
        (px, py) = (xs[i], y[i]);
    }
    y
}

/// The knots that stay: the smallest deviation first, a knot goes while
/// every knot of the projection stays within [`COLLINEAR_CODES`] of the line
/// through the ones kept and the chord that replaces its two segments stays
/// inside their bounds ([`chord_inside`]).
fn kept_knots(xs: &[f64], y: &[f64], caps: &[f64]) -> Vec<usize> {
    let mut keep: Vec<usize> = (0..xs.len()).collect();
    loop {
        let mut best: Option<(f64, usize)> = None;
        for &k in &keep {
            let rest: Vec<(f64, f64)> = keep.iter().filter(|&&i| i != k).map(|&i| (xs[i], y[i])).collect();
            let dev = (0..xs.len()).map(|i| (line_at(&rest, xs[i]) - y[i]).abs()).fold(0.0f64, f64::max);
            if dev <= COLLINEAR_CODES && chord_inside(xs, y, caps, &keep, k) && best.is_none_or(|(d, _)| dev < d) {
                best = Some((dev, k));
            }
        }
        match best {
            Some((_, k)) => keep.retain(|&i| i != k),
            None => return keep,
        }
    }
}

/// With knot `k` gone, is the chord between its kept neighbours no steeper
/// than the LEAST upper bound of the segments it stands for? A chord's slope
/// is a mean of its segments' slopes, so it is always inside the greatest of
/// their bounds — the budget's, where no knot is held — but half a code of
/// collinearity can hide a segment at the budget's slope beside a held
/// range's, and the chord would carry that slope across the held range.
/// `keep` is in the knots' order.
fn chord_inside(xs: &[f64], y: &[f64], caps: &[f64], keep: &[usize], k: usize) -> bool {
    let p = keep.iter().copied().rfind(|&i| i < k);
    let q = keep.iter().copied().find(|&i| i > k);
    let (x0, y0) = p.map_or((0.0, 0.0), |i| (xs[i], y[i]));
    let (x1, y1) = q.map_or((255.0, 255.0), |i| (xs[i], y[i]));
    let cap = caps[p.map_or(0, |i| i + 1)..=q.unwrap_or(xs.len())].iter().copied().fold(f64::INFINITY, f64::min);
    y1 - y0 <= cap * (x1 - x0) + 1e-9
}

/// The curve through `kept` (and the two fixed ends) at input `x`.
fn line_at(kept: &[(f64, f64)], x: f64) -> f64 {
    let mut prev = (0.0f64, 0.0f64);
    for &next in kept.iter().chain(std::iter::once(&(255.0f64, 255.0f64))) {
        if x <= next.0 {
            return prev.1 + (next.1 - prev.1) * (x - prev.0) / (next.0 - prev.0).max(1e-9);
        }
        prev = next;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(points: &[CurvePoint]) -> Vec<(u8, u8)> {
        points.iter().map(|p| (p.input, p.output)).collect()
    }

    /// The asks of the calibration corpus's different-photo pair, as knots
    /// (`gates/j142-cands-c`, the candidate `asks-raw`), at the slope budget
    /// of strength 0.85.
    const DIFFERENT_PHOTO: [(f64, f64); 14] = [
        (23.0, 4.0), (38.0, 8.0), (54.0, 13.0), (68.0, 15.0), (82.0, 19.0), (97.0, 24.0), (112.0, 35.0),
        (128.0, 62.0), (173.0, 86.0), (187.0, 106.0), (201.0, 164.0), (217.0, 206.0), (232.0, 231.0), (247.0, 238.0),
    ];
    const BUDGET_085: (f64, f64) = (0.3571, 2.3571);
    const UNEVEN_FROM_MODEL: [(u8, u8); 6] = [(0, 0), (40, 60), (90, 124), (160, 159), (221, 204), (255, 255)];

    /// The Rust is the numpy model (`rel1180/tools/asks_curve_model.py`, the
    /// model the curve was read on before it was written here): the same
    /// knots on the different-photo pair's asks and on a second vector whose
    /// asks are not monotone and whose masses differ. MUTATION (J142-A): in
    /// `curve_through`, bound every knot by the whole range instead of by
    /// its ask (`asked` = (0, 255)) — the ranges at 173 and 187 share the
    /// clamped slope with the range at 201, which is then darkened past its
    /// ask, and the knots differ. MUTATION (J142-B): keep every knot
    /// (`kept_knots` returns them all) — fourteen knots where six say it.
    #[test]
    fn the_asks_curve_is_the_model_s() {
        let knots: Vec<(f64, f64, f64)> = DIFFERENT_PHOTO.iter().map(|&(x, t)| (x, t, 1.0)).collect();
        assert_eq!(
            pairs(&curve_through(&knots, BUDGET_085.0, BUDGET_085.1)),
            vec![(0, 0), (112, 40), (128, 62), (173, 102), (217, 206), (232, 231), (247, 238), (255, 255)]
        );
        // `asks_curve_model.py 0.5 1.5 40:60:2 90:150:1 160:150:3 221:182:1` (projected 60, 123.75, 158.75, 204)
        let uneven = [(40.0, 60.0, 2.0), (90.0, 150.0, 1.0), (160.0, 150.0, 3.0), (221.0, 182.0, 1.0)];
        assert_eq!(pairs(&curve_through(&uneven, 0.5, 1.5)), UNEVEN_FROM_MODEL.to_vec());
    }

    /// Asks the budget already admits ship as asked, and asks that move
    /// nothing are no curve.
    #[test]
    fn asks_inside_the_budget_ship_as_asked_and_the_identity_is_no_curve() {
        let inside = [(64.0, 48.0, 1.0), (128.0, 110.0, 1.0), (192.0, 200.0, 1.0)];
        assert_eq!(
            pairs(&curve_through(&inside, 0.25, 3.0)),
            vec![(0, 0), (64, 48), (128, 110), (192, 200), (255, 255)]
        );
        assert!(curve_through(&[(64.0, 64.0, 1.0), (128.0, 128.0, 1.0)], 0.25, 3.0).is_empty());
        assert!(curve_through(&[], 0.25, 3.0).is_empty());
        assert!(asks_tone_curve(&[(0.25, 0.0, 1.0, false), (0.5, 0.0, 2.0, false)], 0.25, 3.0).is_empty());
    }

    /// A level moved by an ask is moved in linear light: half the light at
    /// code 128 is code 92 (92.37 before the rounding), and a mass of zero
    /// or an unread ask is no knot.
    #[test]
    fn an_ask_moves_its_level_in_linear_light() {
        let half = (0.5f32).ln();
        let curve = asks_tone_curve(
            &[(128.0 / 255.0, half, 1.0, false), (0.2, f32::NAN, 1.0, false), (0.8, -1.0, 0.0, false)],
            0.25,
            3.0,
        );
        assert_eq!(pairs(&curve), vec![(0, 0), (128, 92), (255, 255)]);
    }

    /// The two promises, on a spread of generated asks (distinct levels,
    /// asks of either sign, uneven masses, budgets across the dial): no
    /// range ends past its ask or against it — to the one code a dropped
    /// knot (half) and its neighbours' rounding (half) may cost — and every
    /// slope is inside the budget, to the rounding's code.
    #[test]
    fn no_range_is_moved_past_its_ask_or_against_it_and_every_slope_is_in_the_budget() {
        let mut state = 0x2545_F491u32;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f64 / (1u32 << 24) as f64
        };
        let mut curves = 0;
        for case in 0..200 {
            let n = 3 + (next() * 13.0) as usize;
            let (smin, smax) = (0.25 + 0.45 * next(), 1.3 + 1.7 * next());
            let mut knots: Vec<(f64, f64, f64)> = Vec::new();
            for k in 0..n {
                let x = (1.0 + 253.0 * (k as f64 + 0.9 * next()) / n as f64).round();
                knots.push((x, (x + (next() - 0.5) * 160.0).clamp(0.0, 255.0), 0.05 + next()));
            }
            let curve = curve_through(&knots, smin, smax);
            if curve.is_empty() {
                continue;
            }
            curves += 1;
            let line: Vec<(f64, f64)> = curve[1..curve.len() - 1].iter().map(|p| (p.input as f64, p.output as f64)).collect();
            for &(x, t, _) in &knots {
                let y = line_at(&line, x);
                assert!(
                    y >= x.min(t) - 1.0 - 1e-6 && y <= x.max(t) + 1.0 + 1e-6,
                    "case {case}: the range at {x} asked {t} and ends at {y:.2}: {:?}",
                    pairs(&curve)
                );
            }
            for pair in curve.windows(2) {
                let dx = (pair[1].input - pair[0].input) as f64;
                let rise = pair[1].output as f64 - pair[0].output as f64;
                assert!(
                    rise >= smin * dx - 1.0 && rise <= smax * dx + 1.0,
                    "case {case}: slope {:.3} outside [{smin:.3}, {smax:.3}]: {:?}",
                    rise / dx,
                    pairs(&curve)
                );
            }
        }
        assert!(curves > 150, "premise: the generated asks ask for curves ({curves} of 200)");
    }

    /// The checker fixture's asks (`fit::tests::tone_stage_keeps_withheld_luma_range_at_identity`,
    /// read off the fit's trace on 2026-11-14): eight paired ranges of its
    /// bottom ramp and the four ranges of its checker top, whose asks are the
    /// population's — the lower two down, the upper two up.
    const CHECKER: [(f32, f32, f32, bool); 12] = [
        (0.14858389, 0.46033692, 9180.0, false),
        (0.20580393, 0.41971803, 8500.0, false),
        (0.26289037, 0.39421916, 9180.0, false),
        (0.32338935, 0.37347674, 9520.0, false),
        (0.38164708, 0.36060178, 8500.0, false),
        (0.4389252, 0.3509251, 9180.0, false),
        (0.49818447, 0.34338605, 9180.0, false),
        (0.535242, 0.33812118, 2069.0, false),
        (0.7408278, -0.13368863, 7052.0, true),
        (0.7916753, -0.145482, 9609.0, true),
        (0.85097194, 0.20291713, 9693.0, true),
        (0.90072244, 0.14316922, 6460.0, true),
    ];

    /// J155. A held range's segments rise no steeper than 1, and the Rust is
    /// the numpy model on held knots too (`rel1180/tools/asks_curve_model.py
    /// --self-test`). Unheld, the checker's top follows its asks — 189 down
    /// to 182, 230 up to 243, a slope of 1.49 the law's contrast reading
    /// refuses, and the bottom's last range is held at the least slope under
    /// the darkened top (155). Held, the top stands where it is on one knot
    /// (the rest lie on its line to the fixed end) and the bottom's last
    /// range reaches its ask (159). Held ranges asked up at growing shifts
    /// move together; held ranges that ask to be compressed keep their asks;
    /// and with every knot held there is only the identity.
    /// MUTATION (J155-A): in `segment_caps`, bound every segment by the
    /// budget alone — the held checker ships the unheld curve.
    /// MUTATION (J155-B): in `kept_knots`, drop the `chord_inside` test —
    /// `a_held_range_keeps_its_slope_on_the_curve_that_ships` finds a chord
    /// over 1 across a held knot.
    #[test]
    fn a_held_range_s_segments_rise_no_steeper_than_one() {
        let free: Vec<(f32, f32, f32, bool)> = CHECKER.iter().map(|&(l, a, m, _)| (l, a, m, false)).collect();
        assert_eq!(
            pairs(&asks_tone_curve(&free, 0.5, 1.5)),
            vec![(0, 0), (38, 49), (127, 149), (136, 155), (189, 182), (230, 243), (255, 255)]
        );
        assert_eq!(pairs(&asks_tone_curve(&CHECKER, 0.5, 1.5)), vec![(0, 0), (38, 49), (136, 159), (189, 189), (255, 255)]);
        // `asks_curve_model.py 0.5 1.5 40:60:2 90:150:1:1 160:190:3:1 221:240:1` (projected 60, 110, 180, 238)
        let up = [(40.0, 60.0, 2.0, false), (90.0, 150.0, 1.0, true), (160.0, 190.0, 3.0, true), (221.0, 240.0, 1.0, false)];
        assert_eq!(pairs(&curve_within(&up, 0.5, 1.5)), vec![(0, 0), (40, 60), (160, 180), (221, 238), (255, 255)]);
        let squeeze = [(60.0, 80.0, 1.0, false), (120.0, 130.0, 1.0, true), (180.0, 182.0, 1.0, true), (230.0, 230.0, 1.0, false)];
        assert_eq!(
            pairs(&curve_within(&squeeze, 0.5, 1.5)),
            vec![(0, 0), (60, 80), (120, 130), (180, 182), (230, 230), (255, 255)]
        );
        let all: Vec<(f64, f64, f64, bool)> = DIFFERENT_PHOTO.iter().map(|&(x, t)| (x, t, 1.0, true)).collect();
        assert!(curve_within(&all, BUDGET_085.0, BUDGET_085.1).is_empty(), "every knot held: only the identity");
    }

    /// J155, on a spread of generated asks with half the knots held (the
    /// model's `--study`): on the curve that SHIPS — after the dropped knots
    /// and the rounding — every chord a held knot lies on rises at most its
    /// run, exactly, in whole codes; and the two older promises still hold.
    #[test]
    fn a_held_range_keeps_its_slope_on_the_curve_that_ships() {
        let mut state = 0x1155_2026u32;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f64 / (1u32 << 24) as f64
        };
        let (mut curves, mut held_read) = (0, 0);
        for case in 0..400 {
            let n = 1 + (next() * 14.0) as usize;
            let (smin, smax) = (0.25 + 0.45 * next(), 1.3 + 1.7 * next());
            let mut knots: Vec<(f64, f64, f64, bool)> = Vec::new();
            for k in 0..n {
                let x = (1.0 + 253.0 * (k as f64 + 0.9 * next()) / n as f64).round();
                knots.push((x, (x + (next() - 0.5) * 120.0).clamp(0.0, 255.0), 0.05 + next(), next() < 0.5));
            }
            let curve = curve_within(&knots, smin, smax);
            if curve.is_empty() {
                continue;
            }
            curves += 1;
            let line: Vec<(f64, f64)> = curve[1..curve.len() - 1].iter().map(|p| (p.input as f64, p.output as f64)).collect();
            for &(x, t, _, held) in &knots {
                let y = line_at(&line, x);
                assert!(
                    y >= x.min(t) - 1.0 - 1e-6 && y <= x.max(t) + 1.0 + 1e-6,
                    "case {case}: the range at {x} asked {t} and ends at {y:.2}: {:?}",
                    pairs(&curve)
                );
                if !held {
                    continue;
                }
                held_read += 1;
                for pair in curve.windows(2) {
                    let (x0, x1) = (pair[0].input as f64, pair[1].input as f64);
                    if x0 <= x && x <= x1 {
                        assert!(
                            pair[1].output as i32 - pair[0].output as i32 <= pair[1].input as i32 - pair[0].input as i32,
                            "case {case}: the held range at {x} lies on a chord steeper than 1: {:?} of {:?}",
                            (pair[0].input, pair[0].output, pair[1].input, pair[1].output),
                            pairs(&curve)
                        );
                    }
                }
            }
            for pair in curve.windows(2) {
                let dx = (pair[1].input - pair[0].input) as f64;
                let rise = pair[1].output as f64 - pair[0].output as f64;
                assert!(
                    rise >= smin * dx - 1.0 && rise <= smax * dx + 1.0,
                    "case {case}: slope {:.3} outside [{smin:.3}, {smax:.3}]: {:?}",
                    rise / dx,
                    pairs(&curve)
                );
            }
        }
        assert!(curves > 150 && held_read > 300, "premise: curves with held knots were read ({curves} curves, {held_read} held knots)");
    }
}
