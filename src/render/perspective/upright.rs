//! This engine's own Upright solver — what Lightroom's mode dropdown MEANS on a
//! photograph Lightroom never solved.
//!
//! Adobe publishes the result of its solver (`crs:UprightTransform_N`, read by
//! `super::upright_from_sidecar`) but not the solver, so a sidecar Lightroom
//! wrote renders Adobe's own numbers to the last digit and only this app's own
//! dropdown reaches this file.
//!
//! # The method
//!
//! A perspective correction is a statement about VANISHING POINTS: the
//! verticals of a building meet at one, its horizontals at another, and
//! straightening them means sending those points to infinity. So the solver
//! never looks for line SEGMENTS — no Hough transform, no linking, no threshold
//! on segment length. Every pixel with a strong enough edge already states a
//! line: the one through it, perpendicular to its gradient. In homogeneous
//! coordinates that line is `l = (gx, gy, −(gx·x + gy·y))`, every line of a
//! family passes through that family's vanishing point `p`, so `l·p` is zero
//! for all of them and
//!
//! ```text
//!     p = argmin_{|p| = 1} Σ_i w_i (l_i · p)²
//! ```
//!
//! is the smallest eigenvector of `Σ w lᵀl` — a 3×3 symmetric matrix, taken
//! here by Jacobi rotations rather than through the characteristic polynomial,
//! because that matrix is near-singular by construction (that is what HAVING a
//! vanishing point means) and the cubic's roots lose most of their digits
//! exactly there. One re-weighting pass follows, since a texture belonging to
//! neither family is an outlier and least squares is not robust to those.
//!
//! The modes differ only in which row goes underneath. Level is a turn alone;
//! Vertical sends the vertical family's point to infinity; Full sends the line
//! through BOTH points there, the classical horizon rectification; Auto is Full
//! with that row halved. Guided is refused — it needs the guides a photographer
//! draws, and no sidecar in this library carries them in a form this engine
//! models.
//!
//! Coordinates are CENTRED [0,1] (`x − ½`), so the answer conjugates onto the
//! frame centre the way Adobe's own matrices are pivoted (`super`'s header has
//! the measurement), and the last step scales the result until it covers the
//! frame — also the way Adobe's do, measured at +0.000000 excursion on all 13
//! matrices this library selected.

use rayon::prelude::*;

use super::Homography;

/// The solver's tuning, all of it, in one place. Every figure is OURS: Adobe
/// published none of them, and the LR kit's F6 case is what will measure the
/// answer they produce against a real Lightroom render.
struct Tune;

impl Tune {
    const STRIDE: usize = 2; // a VP is 2 numbers; a quarter frame is a million equations
    const MIN_GRAD: f32 = 0.05; // below this the "edge" is noise and its angle is random
    const CONE: f32 = 0.436_332_3; // 25°: keeps converging verticals, drops diagonals
    const MIN_SAMPLES: usize = 2_000; // fewer lines than this is a fit to nothing
    const MIN_SEPARATION: f64 = 8.0; // exactly parallel lines have NO finite VP
    const REJECT_SCALE: f32 = 0.02; // the residual the robust weight calls typical
    const AUTO_DAMP: f32 = 0.5; // Adobe's Auto sits between its Level and its Full
}

/// One family's accumulated normal equations, plus the doubled-angle sum the
/// levelling needs.
#[derive(Clone, Copy, Default)]
struct Family {
    /// Upper triangle of `Σ w lᵀl`, row-major: 00 01 02 11 12 22.
    m: [f64; 6],
    /// `Σ w cos 2φ` and `Σ w sin 2φ` over the EDGE direction — a DOUBLED angle,
    /// so an edge and the same edge upside down reinforce instead of cancelling.
    c2: f64,
    s2: f64,
    n: usize,
}

impl Family {
    fn add(&mut self, l: [f32; 3], w: f32, c2: f32, s2: f32) {
        let (a, b, c) = (l[0] as f64, l[1] as f64, l[2] as f64);
        let w = w as f64;
        self.m[0] += w * a * a;
        self.m[1] += w * a * b;
        self.m[2] += w * a * c;
        self.m[3] += w * b * b;
        self.m[4] += w * b * c;
        self.m[5] += w * c * c;
        self.c2 += w * c2 as f64;
        self.s2 += w * s2 as f64;
        self.n += 1;
    }

    fn merge(mut self, o: Family) -> Family {
        for i in 0..6 {
            self.m[i] += o.m[i];
        }
        self.c2 += o.c2;
        self.s2 += o.s2;
        self.n += o.n;
        self
    }

    /// The family's mean EDGE direction in radians — the doubled-angle mean,
    /// halved back.
    fn mean_angle(&self) -> f32 {
        0.5 * (self.s2.atan2(self.c2) as f32)
    }

    /// The vanishing point, homogeneous and centred, or `None` when this family
    /// states no point.
    fn vanishing_point(&self) -> Option<[f32; 3]> {
        if self.n < Tune::MIN_SAMPLES {
            return None;
        }
        let (vals, vecs) = jacobi3(self.m);
        let mut idx = [0usize, 1, 2];
        idx.sort_by(|&a, &b| vals[a].total_cmp(&vals[b]));
        let (lo, next) = (vals[idx[0]], vals[idx[1]]);
        if !(lo.is_finite() && next.is_finite()) || lo < 0.0 || next < lo * Tune::MIN_SEPARATION {
            return None;
        }
        let p = vecs[idx[0]];
        let p = [p[0] as f32, p[1] as f32, p[2] as f32];
        p.iter().all(|v| v.is_finite()).then_some(p)
    }
}

/// Eigen-decomposition of a 3×3 symmetric matrix given as its upper triangle
/// (00 01 02 11 12 22), by cyclic Jacobi rotations. `vecs[i]` is the unit vector
/// for `vals[i]`.
fn jacobi3(up: [f64; 6]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut a = [[up[0], up[1], up[2]], [up[1], up[3], up[4]], [up[2], up[4], up[5]]];
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..24 {
        // The largest off-diagonal element decides the next rotation.
        let mut best = (0usize, 1usize, 0.0f64);
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() > best.2 {
                best = (p, q, a[p][q].abs());
            }
        }
        let (p, q, mag) = best;
        if mag < 1e-18 {
            break;
        }
        let theta = 0.5 * (2.0 * a[p][q]).atan2(a[p][p] - a[q][q]);
        let (s, c) = theta.sin_cos();
        // Columns p and q of A and of the accumulated rotation, then rows p
        // and q of A — the two sides of `Jᵀ A J`. The row pass takes its two
        // rows by value first, because it writes both while reading both.
        for row in &mut a {
            let (akp, akq) = (row[p], row[q]);
            row[p] = c * akp + s * akq;
            row[q] = -s * akp + c * akq;
        }
        let (rp, rq) = (a[p], a[q]);
        a[p] = std::array::from_fn(|k| c * rp[k] + s * rq[k]);
        a[q] = std::array::from_fn(|k| -s * rp[k] + c * rq[k]);
        for row in &mut v {
            let (vkp, vkq) = (row[p], row[q]);
            row[p] = c * vkp + s * vkq;
            row[q] = -s * vkp + c * vkq;
        }
    }
    let vals = [a[0][0], a[1][1], a[2][2]];
    let vecs = [
        [v[0][0], v[1][0], v[2][0]],
        [v[0][1], v[1][1], v[2][1]],
        [v[0][2], v[1][2], v[2][2]],
    ];
    (vals, vecs)
}

/// Accumulate both families over the frame. `reject` is the previous round's
/// answer for each family, used to down-weight the outliers it exposed — `None`
/// on the first pass, which is plain least squares.
fn gather(
    luma: &[f32],
    w: usize,
    h: usize,
    reject: (Option<[f32; 3]>, Option<[f32; 3]>),
) -> (Family, Family) {
    let (dw, dh) = ((w as f32 - 1.0).max(1.0), (h as f32 - 1.0).max(1.0));
    let tan_cone = Tune::CONE.tan();
    // Cauchy weight: an outlier is damped, never dropped, so the answer moves
    // continuously with the picture rather than jumping when one pixel crosses
    // a threshold.
    let robust = |p: Option<[f32; 3]>, l: [f32; 3]| -> f32 {
        let Some(p) = p else { return 1.0 };
        let r = (l[0] * p[0] + l[1] * p[1] + l[2] * p[2]) / Tune::REJECT_SCALE;
        1.0 / (1.0 + r * r)
    };
    (1..h.saturating_sub(1))
        .into_par_iter()
        .step_by(Tune::STRIDE)
        .map(|y| {
            let mut vert = Family::default();
            let mut horiz = Family::default();
            for x in (1..w.saturating_sub(1)).step_by(Tune::STRIDE) {
                let i = y * w + x;
                // Central differences in per-normalised-unit terms, so the two
                // axes are comparable on a frame that is not square.
                let gx = (luma[i + 1] - luma[i - 1]) * 0.5 * dw;
                let gy = (luma[i + w] - luma[i - w]) * 0.5 * dh;
                let mag = (gx * gx + gy * gy).sqrt();
                if mag < Tune::MIN_GRAD {
                    continue;
                }
                let (nx, ny) = (x as f32 / dw - 0.5, y as f32 / dh - 0.5);
                let l = [gx, gy, -(gx * nx + gy * ny)];
                // The EDGE runs perpendicular to the gradient, so a VERTICAL
                // edge has a horizontal gradient.
                let (agx, agy) = (gx.abs(), gy.abs());
                let (ex, ey) = (-gy / mag, gx / mag);
                let (c2, s2) = (ex * ex - ey * ey, 2.0 * ex * ey);
                if agy <= agx * tan_cone {
                    vert.add(l, mag * robust(reject.0, l), c2, s2);
                } else if agx <= agy * tan_cone {
                    horiz.add(l, mag * robust(reject.1, l), c2, s2);
                }
            }
            (vert, horiz)
        })
        .reduce(
            || (Family::default(), Family::default()),
            |a, b| (a.0.merge(b.0), a.1.merge(b.1)),
        )
}

/// Solve `mode` on this frame, or `None` when the picture does not support it.
///
/// `luma` is one plane of the caller's own working buffer, in the gamma-encoded
/// working domain — the SAME frame the resample will run on, so a preview and an
/// export answer the dropdown identically.
pub(crate) fn solve_upright(luma: &[f32], w: usize, h: usize, mode: u8) -> Option<Homography> {
    if w < 32 || h < 32 || luma.len() < w * h || !(1..=4).contains(&mode) {
        return None;
    }
    let (v0, h0) = gather(luma, w, h, (None, None));
    let (vert, horiz) = gather(luma, w, h, (v0.vanishing_point(), h0.vanishing_point()));
    let (pv, ph) = (vert.vanishing_point(), horiz.vanishing_point());

    // The turn that levels the frame, taken from whichever family has more
    // evidence: a photograph of a doorway may state its verticals far better
    // than its horizontals, and the vertical family's angle is the same fact a
    // quarter turn away.
    let level = if horiz.n >= vert.n {
        (horiz.n >= Tune::MIN_SAMPLES).then(|| horiz.mean_angle())
    } else {
        (vert.n >= Tune::MIN_SAMPLES).then(|| quarter_turn(vert.mean_angle()))
    };
    let rotate = {
        let (s, c) = (-level.unwrap_or(0.0)).sin_cos();
        Homography::about_centre([c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0])
    };
    if mode == 2 {
        return level.and_then(|_| super::fill_the_frame(rotate));
    }

    let row = match (mode, pv, ph) {
        (3, Some(a), _) => null_row_through(a),
        (3, None, _) => None,
        // Full and Auto want the horizon — the line through BOTH points. With
        // only one of them the correction degrades to that one, which is better
        // than refusing a photograph that states its verticals clearly and its
        // horizontals not at all.
        (_, Some(a), Some(b)) => horizon(a, b).or_else(|| null_row_through(a)),
        (_, Some(a), None) | (_, None, Some(a)) => null_row_through(a),
        _ => None,
    }?;
    let row = if mode == 1 { [row[0] * Tune::AUTO_DAMP, row[1] * Tune::AUTO_DAMP] } else { row };
    let warp = Homography::about_centre([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, row[0], row[1], 1.0]);
    super::fill_the_frame(rotate.then(warp))
}

/// Bring an angle a quarter turn, into the band the levelling measures in.
fn quarter_turn(a: f32) -> f32 {
    let q = std::f32::consts::FRAC_PI_2;
    if a > 0.0 {
        a - q
    } else {
        a + q
    }
}

/// The minimum-norm last row that sends `q` to infinity.
///
/// The constraint `g·q + 1 = 0` leaves a one-parameter family; the least-warping
/// member is `g = −q/|q|²`, which is the one taken. `None` when the point is
/// already at infinity (the lines are parallel, so there is nothing to correct)
/// or INSIDE the frame, where a "vanishing point" is a spurious fit and
/// correcting it would fold the picture.
pub(super) fn null_row_through(q: [f32; 3]) -> Option<[f32; 2]> {
    if q[2].abs() < 1e-9 {
        return None;
    }
    let (qx, qy) = (q[0] / q[2], q[1] / q[2]);
    let n2 = qx * qx + qy * qy;
    if !n2.is_finite() || n2 < 0.25 {
        return None;
    }
    let row = [-qx / n2, -qy / n2];
    row.iter().all(|v| v.is_finite()).then_some(row)
}

/// The horizon — the line through two vanishing points — as a last row. `None`
/// when it would pass through the frame centre (a divide by zero there) or
/// across the frame at all, for [`null_row_through`]'s reason.
fn horizon(a: [f32; 3], b: [f32; 3]) -> Option<[f32; 2]> {
    let l = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    if !l.iter().all(|v| v.is_finite()) || l[2].abs() < 1e-9 {
        return None;
    }
    let row = [l[0] / l[2], l[1] / l[2]];
    let d = (row[0] * row[0] + row[1] * row[1]).sqrt();
    (d.is_finite() && d > 0.0 && d <= 2.0).then_some(row)
}
