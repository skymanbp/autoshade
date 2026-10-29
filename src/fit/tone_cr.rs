//! The global tone solve on Camera Raw's law (v1.17.0): the exposure scan ×
//! ridge least squares of [`super::tone`], against `render::tone_cr` instead
//! of the knot model, for a recipe that starts from a camera profile.
//!
//! The law's sliders act on `u`, the input of Adobe's default curve, so the
//! measured map (base-render luma → target luma) is carried into `u` through
//! the start `D = the Look's baked curve ∘ Adobe's curve` and solved there:
//! `D⁻¹ ∘ map ∘ D` at the eight knots, the score set's points the same way,
//! a knot's support asked at `D(knot)`. The law has one response table per
//! slider sign and a curvature in the magnitude, so the solve picks the signs
//! from a symmetric solve, then re-solves linearly with every row held at the
//! previous answer until the answer stops moving, and drops a slider whose
//! answer still contradicts the table it was solved with.
//!
//! Highlights and Shadows run BEFORE Exposure on this law and read the
//! frame's statistics (`render::tone_cr`); a tone map has no base layer, so
//! the solve takes every base as the pixel itself (`render::HsSite::diagonal`)
//! and the statistics from the score set's own masses (the mass-weighted
//! mean linear value of its points, and the mean and standard deviation of
//! their `log2`), [`render::HsKey::NOMINAL`] without one — the render's LUT
//! form ([`render::camera_raw_response_keyed`]) at those statistics.

use super::tone::{TONE_PRIOR, solve5};
use super::*;

/// `(ev, sliders)` on Camera Raw's law for a fit whose start is `base`.
/// `tone_map`, `supported` and `score_set` are the knot solve's, stated in
/// the base render's luma ([`super::fit_tone_sliders_supported`]).
pub(crate) fn fit_camera_raw_tone(
    base: &EditRecipe,
    tone_map: &impl Fn(f32) -> f32,
    supported: &impl Fn(f32) -> bool,
    score_set: &[(f32, f32, f32)],
) -> (f32, [f32; 5]) {
    const N: usize = 4096;
    let base_lut = render::build_tone_lut(base);
    let start = |u: f32| render::sample_lut(&base_lut, render::camera_raw_start(u));
    let table: Vec<f32> = (0..N).map(|i| start(i as f32 / (N - 1) as f32)).collect();
    // D is monotone (both curves are); its inverse is a search and a lerp.
    let unstart = |y: f32| {
        let y = y.clamp(0.0, 1.0);
        let i = table.partition_point(|&v| v < y).clamp(1, N - 1);
        let (a, b) = (table[i - 1], table[i]);
        let f = if b > a { ((y - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        (i as f32 - 1.0 + f) / (N - 1) as f32
    };
    let knots = render::TONE_KNOTS_X;
    let support: [f32; 8] =
        std::array::from_fn(|i| if supported(start(knots[i])) { 1.0 } else { 0.0 });
    if support.iter().filter(|&&s| s > 0.0).count() < 2 {
        return (0.0, [0.0; 5]);
    }
    let targets: [f32; 8] = std::array::from_fn(|i| unstart(tone_map(start(knots[i]))));
    let score: Vec<(f32, f32, f32)> =
        score_set.iter().map(|&(x, y, m)| (unstart(x), unstart(y), m)).collect();
    let key = frame_key(&score);

    // The exposure at each candidate, on the frame's own camera start.
    let start = base.camera_start.unwrap_or_default();
    let mut best = (0.0f32, [0.0f32; 5], f32::INFINITY);
    let mut ev = -3.0f32;
    while ev <= 3.0 + 1e-6 {
        let exposure = render::Exposure::new(start, ev);
        let e = exposure.as_ref();
        let ys: [f32; 8] = std::array::from_fn(|i| render::camera_raw_exposure(knots[i], e));
        // Highlights and Shadows move `u` before the exposure: their row on
        // the exposed axis is their push times the exposure's slope there.
        let slope: [f32; 8] = std::array::from_fn(|i| {
            let d = 2e-3;
            let (a, b) = ((knots[i] - d).max(0.0), (knots[i] + d).min(1.0));
            (render::camera_raw_exposure(b, e) - render::camera_raw_exposure(a, e)) / (b - a)
        });
        let row_at = |k: usize, s: f32, i: usize| {
            if k == 1 || k == 2 {
                slope[i] * render::hs_row(k, s, render::HsSite::diagonal(knots[i], key), knots[i])
            } else {
                render::camera_raw_row(k, s, ys[i])
            }
        };
        // Knot authority as the knot model has it: full while one adjacent
        // interval of the exposed curve is still open by 1 % of the axis.
        let weights: [f32; 8] = std::array::from_fn(|i| {
            let left = if i > 0 { ys[i] - ys[i - 1] } else { 0.0 };
            let right = if i < 7 { ys[i + 1] - ys[i] } else { 0.0 };
            (left.max(right) / 0.01).clamp(0.0, 1.0) * support[i]
        });
        let resid: [f64; 8] = std::array::from_fn(|i| (targets[i] - ys[i]) as f64);
        let solve = |row: &dyn Fn(usize, usize) -> f32| {
            let mut ata = [[0.0f64; 5]; 5];
            let mut atb = [0.0f64; 5];
            for i in 0..8 {
                let b: [f64; 5] = std::array::from_fn(|k| (weights[i] * row(k, i)) as f64);
                for p in 0..5 {
                    for q in 0..5 {
                        ata[p][q] += b[p] * b[q];
                    }
                    atb[p] += b[p] * weights[i] as f64 * resid[i];
                }
            }
            for (p, r) in ata.iter_mut().enumerate() {
                r[p] += TONE_PRIOR;
            }
            let x = solve5(ata, atb);
            std::array::from_fn::<f32, 5, _>(|k| (x[k] as f32).clamp(-1.0, 1.0))
        };
        let symmetric = solve(&|k, i| 0.5 * (row_at(k, 1e-6, i) + row_at(k, -1e-6, i)));
        let mut negative: [bool; 5] = std::array::from_fn(|k| symmetric[k] < 0.0);
        // Each pass holds every row at the previous answer: its sign picks
        // the table, its magnitude the curvature.
        let mut s = symmetric;
        for _ in 0..4 {
            let (signs, at) = (negative, s);
            s = solve(&|k, i| {
                let held = at[k].abs().max(1e-6);
                row_at(k, if signs[k] { -held } else { held }, i)
            });
            let again: [bool; 5] = std::array::from_fn(|k| if s[k] == 0.0 { signs[k] } else { s[k] < 0.0 });
            let moved = (0..5).map(|k| (s[k] - at[k]).abs()).fold(0.0f32, f32::max);
            if again == negative && moved < 0.005 {
                break;
            }
            negative = again;
        }
        // A slider solved on the other sign's table has no meaning on its own.
        let signs = negative;
        for (k, v) in s.iter_mut().enumerate() {
            if (*v < 0.0) != signs[k] && *v != 0.0 {
                *v = 0.0;
            }
        }
        let penalty: f64 = s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum();
        let cost = if score.is_empty() {
            (0..8)
                .map(|i| {
                    let d = weights[i] as f64
                        * (render::camera_raw_response_keyed(e, s, knots[i], key) - targets[i]) as f64;
                    d * d
                })
                .sum::<f64>()
                + penalty
        } else {
            let (mut sse, mut mass) = (0.0f64, 0.0f64);
            for &(x, y, m) in &score {
                let d = (render::camera_raw_response_keyed(e, s, x, key) - y) as f64;
                sse += m as f64 * d * d;
                mass += m as f64;
            }
            if mass > 0.0 { 8.0 * sse / mass + penalty } else { penalty }
        };
        if (cost as f32) < best.2 {
            best = (ev, s, cost as f32);
        }
        ev += 0.05;
    }
    (best.0, best.1)
}

/// The frame statistics the solve assumes ([`render::HsKey`]): `log2` of
/// the mean linear value, and the mean and standard deviation of `log2`
/// linear value, over the score set's points weighted by their masses, in
/// `u`; the render's nominal frame without a score set.
fn frame_key(score: &[(f32, f32, f32)]) -> render::HsKey {
    let (mut sum, mut lg, mut lg2, mut mass) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for &(u, _, m) in score {
        let y = f64::from(render::srgb_to_linear(u.clamp(0.0, 1.0)));
        let l = (y + f64::from(render::HS_SH_FLOOR)).log2();
        let m = f64::from(m);
        sum += m * y;
        lg += m * l;
        lg2 += m * l * l;
        mass += m;
    }
    if mass > 0.0 && sum > 0.0 {
        let mean = lg / mass;
        render::HsKey {
            key: ((sum / mass) as f32).max(1e-4).log2(),
            log_mean: mean as f32,
            spread: (lg2 / mass - mean * mean).max(0.0).sqrt() as f32,
        }
    } else {
        render::HsKey::NOMINAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map made by the law itself is solved back: the fitted sliders
    /// reproduce the made response, rms within half of the move it made
    /// (the engine's own round trip's bound), and land on the right side of every slider the recipe moved by
    /// a quarter or more. (Not the same numbers: the ridge prior and the
    /// 0.05 EV scan trade exposure against the sliders where the response
    /// cannot tell them apart — the engine's own round trip,
    /// `roundtrip_recovers_tone_and_saturation`, allows the same.)
    ///
    /// MUTATION: solve with the rows held at zero magnitude (drop the
    /// curvature passes), or carry the map into `u` the wrong way round.
    #[test]
    fn a_map_made_by_the_law_is_solved_back() {
        let base = EditRecipe { camera_profile: "Adobe Standard".into(), ..Default::default() };
        let base_lut = render::build_tone_lut(&base);
        let d = |u: f32| render::sample_lut(&base_lut, render::camera_raw_start(u));
        const N: usize = 4096;
        let table: Vec<f32> = (0..N).map(|i| d(i as f32 / (N - 1) as f32)).collect();
        let d_inv = |y: f32| {
            let i = table.partition_point(|&v| v < y).clamp(1, N - 1);
            let (a, b) = (table[i - 1], table[i]);
            let f = if b > a { ((y - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
            (i as f32 - 1.0 + f) / (N - 1) as f32
        };
        // The score set below is uniform in `u`: the key the solve reads off
        // it is the one the map is made at.
        let key = frame_key(&(1..100).map(|i| (i as f32 / 100.0, 0.0, 1.0)).collect::<Vec<_>>());
        let response =
            |ev: f32, s: [f32; 5], u: f32| render::camera_raw_response_keyed(render::Exposure::new(Default::default(), ev).as_ref(), s, u, key);
        for (ev, s) in [(0.5f32, [0.3f32, -0.4, 0.2, 0.0, -0.2]), (-0.7, [-0.3, 0.0, 0.6, 0.4, 0.3])] {
            let map = |x: f32| d(response(ev, s, d_inv(x)));
            let score: Vec<(f32, f32, f32)> = (1..100)
                .map(|i| {
                    let x = d(i as f32 / 100.0);
                    (x, map(x), 1.0)
                })
                .collect();
            let (fev, fs) = fit_camera_raw_tone(&base, &map, &|_| true, &score);
            let (mut miss, mut moved) = (0.0f32, 0.0f32);
            for i in 1..100 {
                let u = i as f32 / 100.0;
                let want = response(ev, s, u);
                miss += (response(fev, fs, u) - want).powi(2);
                moved += (want - u).powi(2);
            }
            let (miss, moved) = ((miss / 99.0).sqrt(), (moved / 99.0).sqrt());
            assert!(miss < 0.5 * moved, "ev {ev} {s:?} came back as {fev} {fs:?}: rms {miss} of {moved}");
            for k in 0..5 {
                if s[k].abs() >= 0.25 {
                    assert!(fs[k] * s[k] > 0.0, "slider {k}: {} solved as {}", s[k], fs[k]);
                }
            }
        }
    }

    /// The solve's Highlights and Shadows rows are the render's law: at every
    /// knot, the response's change under a small slider move is the row
    /// times the slider (the row the solve builds from `render::hs_row` and
    /// the exposure's slope), on both sides of 0 EV — so the fit cannot
    /// drift from what the develop renders.
    ///
    /// MUTATION: build the rows at the exposed value (the first law's
    /// order), or drop the exposure's slope.
    #[test]
    fn the_solve_rows_are_the_render_law() {
        let key = render::HsKey::NOMINAL;
        let start = crate::recipe::CameraStart { ev: 0.35, headroom: 1.0, tables: 0 };
        for ev in [0.0f32, 0.7, -0.5] {
            let exposure = render::Exposure::new(start, ev);
            let e = exposure.as_ref();
            for (k, s) in [(1usize, -0.02f32), (2, 0.02)] {
                for &u in &render::TONE_KNOTS_X[1..7] {
                    let mut sl = [0.0f32; 5];
                    sl[k] = s;
                    let moved = render::camera_raw_response_keyed(e, sl, u, key) - render::camera_raw_response_keyed(e, [0.0; 5], u, key);
                    let d = 2e-3;
                    let (a, b) = ((u - d).max(0.0), (u + d).min(1.0));
                    let slope = (render::camera_raw_exposure(b, e) - render::camera_raw_exposure(a, e)) / (b - a);
                    let row = slope * render::hs_row(k, s, render::HsSite::diagonal(u, key), u);
                    assert!((moved - s * row).abs() <= 0.1 * (s * row).abs() + 2e-5, "ev {ev} slider {k} at {u}: moved {moved}, row {}", s * row);
                }
            }
        }
    }
}
