//! Frame alignment for the stacking tracks (v1.5.0 Track S).
//!
//! Every merge in [`crate::stack`] needs the frames to sit on top of each
//! other first: a bracket shot handheld moves a few pixels between frames, a
//! focus stack breathes as the lens racks, and averaging misaligned frames is
//! just a blur. This module solves that once, for all four merges.
//!
//! # The shape of the answer
//!
//! A GLOBAL AFFINE plus a SMOOTH LOCAL RESIDUAL, which is the user's own
//! ruling for this batch and is also what the geometry asks for. Six
//! parameters carry everything a hand does between two frames of a bracket —
//! shift, roll, the small scale change of breathing, the shear a rolling
//! shutter leaves — and they are solved from the whole frame, so they are
//! robust in a way no local patch is. What an affine cannot carry is
//! PARALLAX: a near subject and a far one move by different amounts when the
//! camera translates, and no single matrix is right for both. That is the
//! residual's job, and it is deliberately a SMOOTH field rather than a free
//! per-pixel flow — a stack has a handful of frames and no budget for optical
//! flow, and a smooth field cannot invent the local tearing that would show up
//! as a doubled edge in the merge.
//!
//! # Why it aligns on LOG LUMA with the mean removed
//!
//! The hardest case is the one this batch exists for: an exposure bracket,
//! whose consecutive frames differ by one or two STOPS.
//!
//! In LINEAR light an exposure change is a pure multiplication. In log2 of
//! that it is a pure ADDITIVE CONSTANT — and subtracting the mean removes an
//! additive constant exactly. So the whole alignment runs on
//! `log2(max(luma, floor))` with the mean of each side removed at every
//! iteration. The same normalisation is a no-op on a same-exposure stack
//! (focus, noise), so there is ONE aligner here rather than one per merge.
//!
//! **Where that earns its keep, and where it does not — measured, because the
//! obvious claim is wrong.** It is tempting to write that a plain
//! sum-of-squared-differences solve is "dominated by the brightness
//! difference and locks onto nothing". Over a WHOLE frame that is false. The
//! exposure offset contributes `e₀·Σ∇I` to the gradient of the error, and a
//! frame's gradients already sum to about nothing, so the offset cancels on
//! its own: deleting either the mean subtraction or the linearisation leaves
//! this module's own two-stop bracket test passing, unchanged. The
//! cancellation is what fails on SUBSETS — a per-block solve sees a few
//! hundred pixels whose gradients sum to something real, and there the offset
//! walks the block sideways. So the normalisation is load-bearing for the
//! LOCAL pass, which is also where a bracket needs it most, because the local
//! pass is what handles the subject that moved between frames.
//!
//! The pixels this crate hands around are sRGB-encoded, so the transfer is
//! undone first — and this is the ONE claim in the module that no test binds,
//! which is worth writing down rather than leaving to be discovered. sRGB is
//! close to a pure power law, and a power law turns a linear GAIN into a
//! constant offset in log space exactly as linearisation does, only with a
//! different coefficient. The mean subtraction removes any constant offset, so
//! it removes that one too. What survives is sRGB's TOE, where the curve stops
//! being a power law at all — so skipping the linearisation costs accuracy in
//! the deep shadows of a bracket and nowhere else, which is too small a corner
//! for the fixtures here to catch: the mutation was driven against both the
//! whole-frame and the per-block tests and stayed green in each. It stays
//! because it is three lines and it is correct. What would bind it is a
//! fixture whose useful signal lies under sRGB 0.04.

use rayon::prelude::*;

use crate::render::{bilinear_plane, neighbours4};
use crate::stack::pyramid::reduce;

/// A 2×3 affine in CENTRED coordinates: `[a, b, tx, c, d, ty]` maps a point
/// measured from the frame centre to another point measured from the frame
/// centre.
///
/// Centred and not absolute on purpose. With raw `x ∈ [0, W]` the linear
/// columns of the normal matrix are ~W times the translation columns, and the
/// 6×6 solve is correspondingly ill-conditioned on a 9504 px frame; centring
/// brings the columns within a factor of ~W/2 of each other and costs one
/// subtraction per sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine(pub [f32; 6]);

impl Affine {
    /// The do-nothing warp.
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    /// Where an ABSOLUTE pixel `(x, y)` of a `w`×`h` frame lands.
    pub fn apply(&self, x: f32, y: f32, w: usize, h: usize) -> (f32, f32) {
        let (cx, cy) = centre(w, h);
        let (xc, yc) = (x - cx, y - cy);
        let p = &self.0;
        (p[0] * xc + p[1] * yc + p[2] + cx, p[3] * xc + p[4] * yc + p[5] + cy)
    }

    /// The same warp one pyramid level FINER: the linear part is
    /// scale-invariant and the two translations are in pixels, so they double.
    fn finer(self) -> Affine {
        self.rescaled(2.0)
    }

    /// The same warp with its translations scaled by `k` — `finer` with the
    /// factor spelled out, for carrying a level-0 answer back UP a pyramid
    /// (`k = 1/2ⁿ`) as well as down it.
    fn rescaled(self, k: f32) -> Affine {
        let p = self.0;
        Affine([p[0], p[1], p[2] * k, p[3], p[4], p[5] * k])
    }

    /// The warp that undoes this one, or `None` when the linear part is
    /// singular.
    ///
    /// Needed because the solver answers in the direction the RESAMPLER wants
    /// — `solve` returns the map that takes a reference pixel to where it sits
    /// in the moving frame, so `warp_rgb` can read straight through it — while
    /// a caller reasoning about "how far did the camera move" wants the other
    /// direction. Having both named stops that question being re-derived,
    /// wrongly, at each call site.
    pub fn inverse(&self) -> Option<Affine> {
        let p = &self.0;
        let det = p[0] * p[4] - p[1] * p[3];
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (p[4] / det, -p[1] / det, -p[3] / det, p[0] / det);
        Some(Affine([a, b, -(a * p[2] + b * p[5]), c, d, -(c * p[2] + d * p[5])]))
    }

    /// How far this warp moves the frame's worst corner, in pixels — the one
    /// number that means the same thing for a shift, a roll and a scale, and
    /// so the one a refusal threshold can be stated in.
    pub fn corner_travel(&self, w: usize, h: usize) -> f32 {
        let (fw, fh) = (w as f32, h as f32);
        [(0.0, 0.0), (fw, 0.0), (0.0, fh), (fw, fh)]
            .into_iter()
            .map(|(x, y)| {
                let (u, v) = self.apply(x, y, w, h);
                ((u - x).powi(2) + (v - y).powi(2)).sqrt()
            })
            .fold(0.0f32, f32::max)
    }
}

fn centre(w: usize, h: usize) -> (f32, f32) {
    (w as f32 * 0.5, h as f32 * 0.5)
}

/// A solved alignment: the global affine, plus a residual offset per block of
/// a coarse grid.
#[derive(Debug, Clone)]
pub struct Warp {
    /// The global part, in centred coordinates of the frame it was solved on.
    pub global: Affine,
    /// Residual `(dx, dy)` in pixels at each block CENTRE, row-major over
    /// `bx`×`by`. Empty when local refinement was not asked for or was refused
    /// everywhere — which is a real answer and not a failure: a tripod bracket
    /// has no parallax to correct.
    pub local: Vec<[f32; 2]>,
    /// Blocks across and down. Both 0 while `local` is empty.
    pub bx: usize,
    pub by: usize,
}

impl Warp {
    /// The identity, with no residual field.
    pub fn identity() -> Warp {
        Warp { global: Affine::IDENTITY, local: Vec::new(), bx: 0, by: 0 }
    }

    /// Where `(x, y)` lands under the global affine AND the residual field.
    ///
    /// The residual is read with the same bilinear interpolation the pixels
    /// are, over block CENTRES and clamped at the border blocks, so the field
    /// is continuous everywhere — a nearest-block lookup would put a visible
    /// step at every block boundary, which on a merge reads as tiling.
    pub fn apply(&self, x: f32, y: f32, w: usize, h: usize) -> (f32, f32) {
        let (u, v) = self.global.apply(x, y, w, h);
        if self.local.is_empty() {
            return (u, v);
        }
        // Block centres sit at (i + ½)·w/bx, so a pixel's grid coordinate is
        // x·bx/w − ½; `bilinear_plane` clamps the rest.
        let gx = x * self.bx as f32 / w as f32 - 0.5;
        let gy = y * self.by as f32 / h as f32 - 0.5;
        let comp = |c: usize| {
            let plane: Vec<f32> = self.local.iter().map(|d| d[c]).collect();
            bilinear_plane(&plane, self.bx, self.by, gx, gy)
        };
        (u + comp(0), v + comp(1))
    }
}

/// What the solver is allowed to do.
#[derive(Debug, Clone, Copy)]
pub struct AlignParams {
    /// Pyramid levels, including the full-resolution one. Each level halves
    /// both axes, and [`solve`] stops early rather than reducing below 32 px
    /// on the short side, so a small frame gets fewer levels than asked for.
    ///
    /// **This is the number that sets the REACH**, because a Gauss–Newton step
    /// is only valid inside the radius where the gradient still describes the
    /// picture. Measured on this module's fixture, the global fit recovers
    /// about two to four pixels at its COARSEST level, and the pyramid
    /// multiplies that by 2^(levels−1): 12 px on a 192×144 frame, which the
    /// size guard leaves with four levels, against 48 px on 384×288 and 64 px
    /// on 768×576, which get five. The reach is therefore not a fraction of
    /// the frame — 12.5 % of the width came back on 384×288 and did not on
    /// either of the other two. See the test
    /// `a_shift_that_pushes_a_fifth_of_the_frame_out_of_view_still_comes_back`.
    pub levels: usize,
    /// Gauss–Newton iterations per level.
    pub iters: usize,
    /// Blocks across the LONG edge for the local pass; 0 disables it.
    pub blocks: usize,
    /// A block's residual is kept only when the smaller eigenvalue of its own
    /// 2×2 normal matrix, per sample, is at least this. Flat sky has no
    /// texture to measure a shift against, and a block that reports seven
    /// pixels of motion out of noise is worse than one that reports nothing.
    pub min_texture: f32,
    /// A residual longer than this many pixels is refused whatever its texture
    /// score: past it the block has matched something else.
    pub max_residual: f32,
}

impl AlignParams {
    /// Five levels and eight iterations is the shape that converges on a
    /// handheld bracket without a search costing more than the merge itself.
    ///
    /// The two thresholds are FIRST-PRINCIPLES and say so: `1e-4` is an order
    /// of magnitude above the per-sample eigenvalue a flat patch of sensor
    /// noise produces in this log-luma domain, and 24 px is wider than the
    /// parallax a handheld bracket shows on a 24 MP frame while staying well
    /// inside a block, so a refused residual is one that matched the wrong
    /// thing rather than one that simply moved a lot. Track S's kit cases are
    /// what would replace them with measurements.
    pub const DEFAULT: AlignParams =
        AlignParams { levels: 5, iters: 8, blocks: 12, min_texture: 1e-4, max_residual: 24.0 };
}

impl Default for AlignParams {
    fn default() -> Self {
        AlignParams::DEFAULT
    }
}

/// The photometric domain every alignment here works in: `log2` of linear
/// luma, with a floor that keeps a black pixel finite.
///
/// The floor is 1/4096 — two stops below a 10-bit code value, so it bites only
/// on pixels carrying no information at all, and it bounds the log range below
/// at −12 so one black pixel cannot drag a block's mean.
pub fn log_luma(rgb: &[[f32; 3]]) -> Vec<f32> {
    const FLOOR: f32 = 1.0 / 4096.0;
    rgb.par_iter().map(|p| super::luma(p).max(FLOOR).log2()).collect()
}


/// Central-difference gradients, over the clamped four-neighbourhood.
fn gradients(p: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let mut gx = vec![0.0f32; w * h];
    let mut gy = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let [l, r, u, d] = neighbours4(p, w, h, x, y);
            gx[y * w + x] = 0.5 * (r - l);
            gy[y * w + x] = 0.5 * (d - u);
        }
    }
    (gx, gy)
}

/// Bilinear sample, or `None` outside the frame.
///
/// The bounds question lives HERE rather than in `bilinear_plane`, which
/// clamps: a warped sample that fell off the edge must contribute to neither
/// the error nor the normal matrix, or the border pulls a large shift back
/// toward zero by matching the replicated edge against itself.
fn sample(p: &[f32], w: usize, h: usize, x: f32, y: f32) -> Option<f32> {
    let inside = x >= 0.0 && y >= 0.0 && x <= (w - 1) as f32 && y <= (h - 1) as f32;
    inside.then(|| bilinear_plane(p, w, h, x, y))
}

/// Solve `M x = g` for an `N`×`N` system by Gauss–Jordan with partial
/// pivoting. `None` when the system is singular to working precision, which is
/// the honest answer for a patch with no texture along one direction.
fn solve_lin<const N: usize>(mut m: [[f64; N]; N], mut g: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let (piv, mag) = (col..N).fold((col, 0.0f64), |(bi, bm), r| {
            let v = m[r][col].abs();
            if v > bm { (r, v) } else { (bi, bm) }
        });
        if mag < 1e-12 {
            return None;
        }
        m.swap(col, piv);
        g.swap(col, piv);
        let d = m[col][col];
        for v in m[col].iter_mut().skip(col) {
            *v /= d;
        }
        g[col] /= d;
        // The pivot row is COPIED out before the elimination, because the
        // inner loop reads it while writing a different row and a borrow of
        // `m` cannot do both. At N ≤ 6 that copy is six doubles.
        let (pivot, gp) = (m[col], g[col]);
        for r in 0..N {
            if r == col || m[r][col] == 0.0 {
                continue;
            }
            let f = m[r][col];
            for (v, pv) in m[r].iter_mut().zip(&pivot).skip(col) {
                *v -= f * pv;
            }
            g[r] -= f * gp;
        }
    }
    Some(g)
}

/// The zero-mean error between `t` and `i` warped by `at`, together with the
/// two Jacobian columns, accumulated over `pixels`.
///
/// The means are recomputed on every call because the VALID SET changes as the
/// warp moves, and a mean taken over a different set of pixels than the error
/// is not the mean of that error. This is the exposure invariance the module
/// is built around, so it lives in one place for both solvers.
struct Normals<const N: usize> {
    m: [[f64; N]; N],
    g: [f64; N],
    n: usize,
    /// The robust cost at these parameters, summed over the valid samples.
    /// It is what makes a step CHECKABLE: a Gauss–Newton step is a guess that
    /// the cost is locally quadratic, and when it is not the step can make
    /// things worse. Carrying the cost out of the same loop that builds the
    /// normal equations makes checking free.
    cost: f64,
}

/// How far a residual may be from the middle of the pack before it stops
/// steering the fit, as a multiple of the median absolute residual.
///
/// Three, for [`super::merge::composite`]'s reason and with the same
/// arithmetic: ordinary disagreement — noise, interpolation, the last tenth of
/// a pixel — is barely touched, while something that is not a registration
/// error at all stops counting. The scale is the MEDIAN, so a region that
/// changed between the frames cannot inflate the very number that is supposed
/// to detect it: at 8 % of the frame it does not move the median at all.
const RESIDUAL_SPREADS: f32 = 3.0;

/// Every 97th pixel, which is the sample [`super::merge::composite`] takes for
/// the same purpose and for the same reasons: a scale is not a measurement,
/// every 97th pixel of a 24 MP frame is still a quarter of a million samples,
/// and 97 is prime so the sample cannot land on a column or a Bayer phase.
const SCALE_STRIDE: usize = 97;

/// How much better than DOING NOTHING a block's answer has to be, as a
/// fraction of the cost of doing nothing.
///
/// A near-periodic texture — a brick wall, a crowd, a field of grass, the
/// fixture this was measured on — hands a gradient solver many equally good
/// answers: shift by one period and the texture matches itself again. Along a
/// direction the slow structure happens to be invariant in, the costs at those
/// shifts are equal to within the grain, and nothing in the arithmetic prefers
/// the right one. Measured on this module's own burst fixture, with a
/// beats-doing-nothing test but no margin on it: the local field came out at
/// 7.5 to 23.3 px on frames that were registered EXACTLY, and the noise stack
/// built on it was no better than a single frame.
///
/// The tie is broken by the prior the physics offers — consecutive frames of a
/// burst moved a little, not a lot — so an answer that is not CLEARLY better
/// than doing nothing loses to doing nothing. Ten per cent: a genuine parallax
/// leaves a residual several times smaller than doing nothing does, and a
/// period re-lock is inside the grain of it.
const BLOCK_MARGIN: f64 = 0.10;

/// How far a neighbouring block's answer may sit from this one's and still
/// COUNT AS AGREEING, as a fraction of the longer of the two.
///
/// The margin above is a test each block takes alone, and alone is not enough:
/// where the ambiguity is near-exact, the wrong answer beats doing nothing
/// honestly, by fitting the grain. Measured on this module's own burst at
/// 384×256, on frames that differ only by a painted rectangle — six of the 96
/// blocks answered, and FIVE of them nowhere near the rectangle, every one of
/// them at ±(4.8, −5.7) px. The fixture's texture is
/// `sin(1.3x)·cos(1.1y)`: its periods are 4.83 and 5.71 px, so those five had
/// locked onto the texture one period over, and along that diagonal the slow
/// structure that should have told them apart moves 0.006 — a fifth of the
/// grain.
///
/// What separates the two is not the block, it is the FIELD. A subject that
/// moved is several blocks wide and its blocks agree with each other; a period
/// re-lock is one block disagreeing with everything around it. So a block's
/// answer survives only if at least one of its four neighbours corroborates
/// it. One neighbour and not a majority: the smallest moving subject this
/// module's own tests draw is two blocks by two, where every block has exactly
/// two neighbours inside the subject and five outside it.
const FIELD_AGREEMENT: f32 = 0.5;

/// The scale the robust weighting is measured in: `RESIDUAL_SPREADS` times the
/// median absolute residual at `at`, over a subsample.
///
/// Recomputed once per pyramid level rather than once per iteration, so that
/// every cost inside a level is the SAME function and two of them can be
/// compared. A scale that moved with the fit would make each iteration's cost
/// incomparable with the last, and the comparison is the whole point.
///
/// The estimator is a MEDIAN for the textbook reason — a mean is inflated by
/// the very samples it exists to down-weight — but nothing here has measured
/// that reason, and saying so is cheaper than implying otherwise. Driven on
/// this module's burst with a disagreeing rectangle over 6 %, 25 % and 56 % of
/// the frame, that frame's corner travel (truth: 0) came out 0.000 / 0.052 /
/// 9.795 px with the median and 0.000 / 0.024 / 9.817 px with a mean. Past
/// half the frame neither works, which is the median's own breakdown point and
/// not a fault in it. What IS measured is that SOME robust scale is needed:
/// with none, the same fixture's ghost frame walked 539.7 px.
fn robust_scale(p: &Pair, pixels: &[(usize, usize)], at: impl Fn(f32, f32) -> (f32, f32)) -> f32 {
    let mut pairs: Vec<(f32, f32)> = Vec::with_capacity(pixels.len() / SCALE_STRIDE + 1);
    for &(x, y) in pixels.iter().step_by(SCALE_STRIDE) {
        let (u, v) = at(x as f32, y as f32);
        if let Some(iw) = sample(p.i, p.w, p.h, u, v) {
            pairs.push((p.t[y * p.w + x], iw));
        }
    }
    if pairs.len() < 8 {
        // Too few samples to say anything about the spread. Zero means
        // UNWEIGHTED below — an honest refusal to guess a scale, not a
        // guessed scale of zero, which would reject every sample.
        return 0.0;
    }
    let n = pairs.len() as f32;
    let (mt, mi) = (
        pairs.iter().map(|(t, _)| *t).sum::<f32>() / n,
        pairs.iter().map(|(_, i)| *i).sum::<f32>() / n,
    );
    let mut e: Vec<f32> = pairs.iter().map(|(t, i)| ((t - mt) - (i - mi)).abs()).collect();
    let mid = e.len() / 2;
    let (_, med, _) = e.select_nth_unstable_by(mid, f32::total_cmp);
    // A frame pair that agrees EXACTLY (a duplicate file, a synthetic test)
    // has a median of zero, and a scale of zero would then reject everything.
    // The floor is one part in ten thousand of the [0,1] range these planes
    // live in — below any real disagreement, above exact equality.
    (RESIDUAL_SPREADS * *med).max(1e-4)
}

/// The two frames a solve is comparing, with `i`'s gradients and the size all
/// four planes share.
///
/// They travel together because they are meaningless apart: the gradients are
/// OF `i`, and one `w`/`h` describes every plane here. Six positional
/// arguments is how a call site ends up handing `gy` where `gx` belongs, and
/// nothing about the types would have stopped it.
struct Pair<'a> {
    t: &'a [f32],
    i: &'a [f32],
    gx: &'a [f32],
    gy: &'a [f32],
    w: usize,
    h: usize,
}

fn accumulate<const N: usize>(
    p: &Pair,
    pixels: &[(usize, usize)],
    at: impl Fn(f32, f32) -> (f32, f32) + Sync,
    jac: impl Fn(f32, f32, f32, f32) -> [f32; N] + Sync,
    scale: f32,
) -> Normals<N> {
    let (mut st, mut si, mut n) = (0.0f64, 0.0f64, 0usize);
    for &(x, y) in pixels {
        let (u, v) = at(x as f32, y as f32);
        if let Some(iw) = sample(p.i, p.w, p.h, u, v) {
            st += p.t[y * p.w + x] as f64;
            si += iw as f64;
            n += 1;
        }
    }
    let mut out = Normals { m: [[0.0; N]; N], g: [0.0; N], n, cost: 0.0 };
    if n == 0 {
        return out;
    }
    let (mt, mi) = ((st / n as f64) as f32, (si / n as f64) as f32);
    let s2 = (scale as f64) * (scale as f64);
    for &(x, y) in pixels {
        let (u, v) = at(x as f32, y as f32);
        let (Some(iw), Some(dx), Some(dy)) = (
            sample(p.i, p.w, p.h, u, v),
            sample(p.gx, p.w, p.h, u, v),
            sample(p.gy, p.w, p.h, u, v),
        ) else {
            continue;
        };
        let j = jac(x as f32, y as f32, dx, dy);
        let e = ((p.t[y * p.w + x] - mt) - (iw - mi)) as f64;
        // Geman–McClure, which is least squares wherever the disagreement is
        // ordinary and stops growing where it is not. `u` is the ratio the
        // cost saturates by; the IRLS weight on the normal equations is its
        // square (the derivative of that cost), and the cost itself is e²·u.
        //
        // Least squares alone has NO defence against a region whose content
        // genuinely differs between two frames — somebody who walked through
        // one of them — because such a region's residual is enormous and the
        // normal equations are a sum. Measured on this module's own noise
        // fixture: an 8 % ghost rectangle walked the affine to 539.7 px of
        // corner travel on frames that were already registered, and threw
        // away 69 % of the frame as uncovered.
        let (w, c) = if s2 > 0.0 {
            let ratio = s2 / (s2 + e * e);
            (ratio * ratio, e * e * ratio)
        } else {
            (1.0, e * e)
        };
        out.cost += c;
        for r in 0..N {
            out.g[r] += w * j[r] as f64 * e;
            for c in 0..N {
                out.m[r][c] += w * j[r] as f64 * j[c] as f64;
            }
        }
    }
    out
}

/// One pyramid level of forward-additive Gauss–Newton on the affine.
///
/// **How many of the six are fitted depends on the level.** Six parameters
/// need a level big enough to SHOW a rotation: on a 24 px level a 1° roll
/// moves the corner by a fifth of a pixel, which is under the noise, and a
/// solve asked for it anyway answers with an invented shear. The pyramid then
/// doubles that invention at every step down, so it arrives at full resolution
/// as a ruined fit rather than as a small error. Measured, on a 24 px shift of
/// a 192×144 frame: fitting all six at every level returned `a = 0.979,
/// b = 0.160, d = 0.785` and found 16 of the 24 pixels, while fitting the
/// translation alone on the two coarsest levels returned `a = 1.008,
/// d = 1.029` — the linear part stopped being invented, and what was left was
/// an honest reach limit rather than a wrong answer.
///
/// So a level under 64 px on its short side solves the TRANSLATION only, and
/// the linear part joins the moment a level can see it. The translations
/// carry down the pyramid unchanged by this; only who is allowed to add to
/// the other four changes.
///
/// **Every step is checked, and the answer is the best one MEASURED.** A
/// Gauss–Newton step is a guess that the cost is locally quadratic; where it
/// is not, the step makes the fit worse and the next step worse still, and
/// nothing in the arithmetic stops it. The loop therefore measures the cost at
/// the parameters it is standing on, keeps the lowest it has seen, and returns
/// that — so a step that made things worse is not merely survived, it is
/// discarded. It runs one pass more than it takes steps, because the last
/// step's parameters would otherwise be returned without ever being measured.
fn lk_affine(t: &[f32], i: &[f32], w: usize, h: usize, mut a: Affine, iters: usize) -> Affine {
    let (gx, gy) = gradients(i, w, h);
    let (cx, cy) = centre(w, h);
    let pixels: Vec<(usize, usize)> = (0..w * h).map(|k| (k % w, k / w)).collect();
    let pair = Pair { t, i, gx: &gx, gy: &gy, w, h };
    let linear = w.min(h) >= 64;
    let start = a;
    let scale = robust_scale(&pair, &pixels, |x, y| start.apply(x, y, w, h));
    let (mut best, mut best_cost, mut first_n) = (a, f64::INFINITY, 0usize);
    for k in 0..=iters {
        let cur = a;
        let at = |x: f32, y: f32| cur.apply(x, y, w, h);
        let (n, cost, step) = if linear {
            let nm = accumulate::<6>(
                &pair,
                &pixels,
                at,
                |x, y, dx, dy| {
                    let (xc, yc) = (x - cx, y - cy);
                    [dx * xc, dx * yc, dx, dy * xc, dy * yc, dy]
                },
                scale,
            );
            (nm.n, nm.cost, solve_lin(nm.m, nm.g).map(|d| d.map(|v| v as f32)))
        } else {
            let nm = accumulate::<2>(&pair, &pixels, at, |_, _, dx, dy| [dx, dy], scale);
            (
                nm.n,
                nm.cost,
                solve_lin(nm.m, nm.g).map(|d| [0.0, 0.0, d[0] as f32, 0.0, 0.0, d[1] as f32]),
            )
        };
        // Under 64 valid samples the warp has walked the frames apart; the
        // last good answer beats a fit to a handful of corner pixels.
        if n < 64 {
            break;
        }
        if k == 0 {
            first_n = n;
        }
        // A MEAN cost, so the comparison survives the valid set changing as
        // the warp moves — and a floor under that set, because a warp which
        // pushed most of the frame out of view could otherwise win by keeping
        // only the handful of samples that happened to agree.
        if 2 * n < first_n || cost / n as f64 >= best_cost {
            break;
        }
        (best, best_cost) = (a, cost / n as f64);
        if k == iters {
            break; // the extra pass exists to measure the last step, not to take another
        }
        let Some(d) = step else { break };
        // THE SIGN, and it is the one thing in this loop that has no second
        // chance. The normal equations are H·Δp = Σ Jᵀ·e with e = T − I∘W, so
        // Δp already points DOWNHILL and the update ADDS it. Subtracting is
        // gradient ascent: the warp walks away from the answer until the
        // iteration limit stops it, which reads as a plausible-looking
        // twenty-pixel translation rather than as a crash.
        let mut p = a.0;
        for (k, v) in p.iter_mut().enumerate() {
            *v += d[k];
        }
        if !p.iter().all(|v| v.is_finite()) {
            break;
        }
        a = Affine(p);
        if d.iter().all(|v| v.abs() < 1e-6) {
            break;
        }
    }
    best
}

/// Per-block translation refinement, coarse to fine, after the global fit.
///
/// **The pyramid here is not a speed trick, it is the reach.** A single-scale
/// Gauss–Newton step is only valid inside the radius where the gradient still
/// describes the image — about a pixel on textured content — so a
/// full-resolution-only pass creeps roughly one pixel per iteration and stops
/// wherever the iteration budget runs out. Measured on this module's own
/// parallax scene: five pixels of true residual came back as one. Running the
/// same 2-dof solve down the pyramid the global pass already built puts a
/// five-pixel residual at well under a pixel on the coarse level, where one
/// step reaches it, and [`AlignParams::max_residual`] becomes a statement
/// about what is BELIEVED rather than about what can be found.
///
/// The block GRID is the same count at every level — only the pixels per
/// block change — so one residual vector is carried down and doubled at each
/// step, exactly as the global affine is.
fn refine_blocks(
    t: &[(Vec<f32>, usize, usize)],
    i: &[(Vec<f32>, usize, usize)],
    global: Affine,
    p: &AlignParams,
) -> (Vec<[f32; 2]>, usize, usize) {
    let (_, w, h) = t[0];
    let long = w.max(h);
    let bx = (p.blocks * w / long).max(2);
    let by = (p.blocks * h / long).max(2);
    // Three levels of reach is 4× the single-scale radius and still cheap,
    // because each level above the first costs a quarter of the one below.
    //
    // Whether a given block can USE a given level is not decided here — it is
    // decided per block, by how many samples that block actually has in view,
    // down in the loop. A rule stated up here in block SIDES instead (the
    // first shape of this, "no side under 16 px") throws away a level that
    // some blocks could have measured perfectly well, which costs reach on
    // exactly the small frames that need it most.
    let top = (t.len() - 1).min(2);
    let mut d = vec![[0.0f32; 2]; bx * by];
    let mut alive = vec![true; bx * by];
    for lvl in (0..=top).rev() {
        let (tp, lw, lh) = &t[lvl];
        let (ip, _, _) = &i[lvl];
        let (gx, gy) = gradients(ip, *lw, *lh);
        let pair = Pair { t: tp, i: ip, gx: &gx, gy: &gy, w: *lw, h: *lh };
        // The global affine restated in THIS level's pixels: the linear part
        // is scale-free and the translations shrink by 2 per level up.
        let g = global.rescaled(1.0 / (1 << lvl) as f32);
        if lvl < top {
            for v in d.iter_mut() {
                v[0] *= 2.0;
                v[1] *= 2.0;
            }
        }
        let step: Vec<([f32; 2], bool)> = (0..bx * by)
            .into_par_iter()
            .map(|b| {
                if !alive[b] {
                    return ([0.0, 0.0], false);
                }
                let (bi, bj) = (b % bx, b / bx);
                let (x0, x1) = (bi * lw / bx, ((bi + 1) * lw / bx).min(*lw));
                let (y0, y1) = (bj * lh / by, ((bj + 1) * lh / by).min(*lh));
                let pixels: Vec<(usize, usize)> =
                    (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).collect();
                let mut cur = d[b];
                // The same rule the global fit follows, for the same reason
                // and against the same failure: a Gauss–Newton step is a
                // GUESS, and a block asked to match two frames that differ
                // only by grain will happily walk several pixels to fit that
                // grain. Measured through the CLI on a five-frame noise
                // fixture whose frames are registered exactly: the local field
                // pushed 3.10 % of the frame out of view and the stacked rms
                // went from 0.01472 (no alignment) to 0.05862 — the alignment
                // was doing four times more damage than the noise it was
                // there to help average away.
                //
                // What this rule contributes ON ITS OWN, however, is smaller
                // than that and nothing in the battery separates it: dropped
                // as a mutation it leaves both the end-to-end test and the
                // parallax test green, while a probe at the same moment shows
                // it changing individual blocks by up to 1.1 px at the coarse
                // levels and 0.28 px at the finest. It stays because the
                // IDENTICAL rule in `lk_affine` is bound (see that test's
                // table, where dropping it moves the whole fit), and because a
                // pass that returns a step it never measured is the shape of
                // the failure described above, not a different one.
                let (mut best, mut best_cost, mut first_n) = (cur, f64::INFINITY, 0usize);
                for k in 0..=p.iters {
                    let at = cur;
                    let nm = accumulate::<2>(
                        &pair,
                        &pixels,
                        |x, y| {
                            let (u, v) = g.apply(x, y, *lw, *lh);
                            (u + at[0], v + at[1])
                        },
                        |_, _, dx, dy| [dx, dy],
                        // UNWEIGHTED, deliberately, and it is the one place in
                        // this module that must be. The global fit weights a
                        // disagreeing region down because one transform cannot
                        // be right for a region that moved on its own — but
                        // that region is exactly what this pass exists to
                        // find. A scale taken over the frame is the GROUND's
                        // residual, which after the global fit is nothing, so
                        // robust weighting here rejects the subject's own
                        // motion as an outlier: driven that way, the parallax
                        // test read −0.36 px of a −6 px truth and the small
                        // frame's grid went to zeros.
                        0.0,
                    );
                    // Too few samples IN VIEW is a statement about this LEVEL,
                    // not about this block: the warp has pushed the block
                    // mostly off the frame here and the finer levels still get
                    // their turn. Killing the block on it — which this loop
                    // did first — silently disabled the whole local pass,
                    // because the coarsest level is exactly where a block is
                    // smallest.
                    if nm.n < 32 {
                        return (best, true);
                    }
                    if k == 0 {
                        first_n = nm.n;
                    }
                    // Shi–Tomasi: the SMALLER eigenvalue of the 2×2, per
                    // sample. A block textured along one axis only (a horizon,
                    // a wall edge) has a large larger-eigenvalue and cannot
                    // measure the shift ALONG that edge at all — the case a
                    // determinant or a trace would wave straight through.
                    let (a11, a12, a22) = (nm.m[0][0], nm.m[0][1], nm.m[1][1]);
                    let tr = a11 + a22;
                    let disc = (tr * tr / 4.0 - (a11 * a22 - a12 * a12)).max(0.0).sqrt();
                    if (tr / 2.0 - disc) / nm.n as f64 <= p.min_texture as f64 {
                        return ([0.0, 0.0], false);
                    }
                    // A MEAN cost, and a floor under the sample count, for the
                    // global fit's reasons (see `lk_affine`).
                    if 2 * nm.n < first_n || nm.cost / nm.n as f64 >= best_cost {
                        break;
                    }
                    (best, best_cost) = (cur, nm.cost / nm.n as f64);
                    if k == p.iters {
                        break; // the extra pass measures the last step, it does not take another
                    }
                    let Some(s) = solve_lin(nm.m, nm.g) else { return ([0.0, 0.0], false) };
                    cur[0] += s[0] as f32;
                    cur[1] += s[1] as f32;
                    if !cur[0].is_finite() || !cur[1].is_finite() {
                        return ([0.0, 0.0], false);
                    }
                    if s.iter().all(|v| v.abs() < 1e-4) {
                        break;
                    }
                }
                // …and at the finest level, where the offsets are in real
                // pixels, the answer has to beat DOING NOTHING. Every level
                // above this one can only improve on what it was handed, but
                // what it was handed was doubled from the level above without
                // anyone asking whether that was worth having — and on frames
                // with no local motion at all, the honest answer is zero.
                if lvl == 0 && (best[0] != 0.0 || best[1] != 0.0) {
                    let zero = accumulate::<2>(
                        &pair,
                        &pixels,
                        |x, y| g.apply(x, y, *lw, *lh),
                        |_, _, dx, dy| [dx, dy],
                        0.0,
                    );
                    if zero.n >= 32
                        && (zero.cost / zero.n as f64) * (1.0 - BLOCK_MARGIN) <= best_cost
                    {
                        return ([0.0, 0.0], true);
                    }
                }
                ([best[0], best[1]], true)
            })
            .collect();
        for (b, (v, ok)) in step.into_iter().enumerate() {
            alive[b] = alive[b] && ok;
            d[b] = if alive[b] { v } else { [0.0, 0.0] };
        }

    }
    for (b, v) in d.iter_mut().enumerate() {
        if !alive[b] || v[0].hypot(v[1]) > p.max_residual {
            *v = [0.0, 0.0];
        }
    }
    // …and then the field is asked whether it believes itself. Read from a
    // COPY, so that this is one judgement on the field the blocks produced
    // rather than a wave travelling across the grid.
    let field = d.clone();
    for (b, v) in d.iter_mut().enumerate() {
        if *v == [0.0, 0.0] {
            continue;
        }
        let (bi, bj) = (b % bx, b / bx);
        let neighbours = [
            (bi.wrapping_sub(1), bj),
            (bi + 1, bj),
            (bi, bj.wrapping_sub(1)),
            (bi, bj + 1),
        ];
        let mine = v[0].hypot(v[1]);
        let agreed = neighbours.iter().any(|&(nx, ny)| {
            // `wrapping_sub` on 0 lands past the grid, which this rejects
            // along with the far edge — an edge block simply has fewer
            // neighbours to be corroborated by.
            if nx >= bx || ny >= by {
                return false;
            }
            let n = field[ny * bx + nx];
            (v[0] - n[0]).hypot(v[1] - n[1]) <= FIELD_AGREEMENT * mine.max(n[0].hypot(n[1]))
        });
        if !agreed {
            *v = [0.0, 0.0];
        }
    }
    (d, bx, by)
}

/// Align `moving` onto `reference`, both `w`×`h` sRGB-encoded frames.
///
/// Coarse to fine: the global affine is solved on a pyramid and carried down,
/// then — when `params.blocks` allows — each block of the full-resolution
/// frame refines its own residual shift on top of it.
///
/// **Direction.** The answer maps a REFERENCE pixel to where that content sits
/// in the MOVING frame, which is the direction a resampler reads: `warp_rgb`
/// hands it straight to the sampler. If the moving frame was made by
/// resampling the reference through some `A`, the answer here is `A⁻¹` — see
/// [`Affine::inverse`], which exists so that no caller has to re-derive it.
pub fn solve(
    reference: &[[f32; 3]],
    moving: &[[f32; 3]],
    w: usize,
    h: usize,
    params: &AlignParams,
) -> Warp {
    if w < 2 || h < 2 || reference.len() != w * h || moving.len() != w * h {
        return Warp::identity();
    }
    let mut t = vec![(log_luma(reference), w, h)];
    let mut i = vec![(log_luma(moving), w, h)];
    for _ in 1..params.levels.max(1) {
        let (lt, lw, lh) = t.last().expect("at least the base level");
        if (*lw).min(*lh) < 32 {
            break;
        }
        let (rt, nw, nh) = reduce(lt, *lw, *lh);
        let (li, _, _) = reduce(&i.last().expect("levels are built in pairs").0, *lw, *lh);
        t.push((rt, nw, nh));
        i.push((li, nw, nh));
    }
    let mut a = Affine::IDENTITY;
    for lvl in (0..t.len()).rev() {
        let (tp, lw, lh) = &t[lvl];
        a = lk_affine(tp, &i[lvl].0, *lw, *lh, a, params.iters);
        if lvl > 0 {
            a = a.finer();
        }
    }
    if params.blocks == 0 {
        return Warp { global: a, local: Vec::new(), bx: 0, by: 0 };
    }
    let (local, bx, by) = refine_blocks(&t, &i, a, params);
    // Every block refused is not "no field" by accident — it is the answer a
    // tripod bracket gives, and carrying an all-zero grid would cost every
    // later sample a bilinear read of zeros.
    if local.iter().all(|d| d[0] == 0.0 && d[1] == 0.0) {
        return Warp { global: a, local: Vec::new(), bx: 0, by: 0 };
    }
    Warp { global: a, local, bx, by }
}

/// Resample `src` through `warp`, in sRGB-encoded RGB.
///
/// A pixel whose source lands outside the frame KEEPS THE VALUE IT HAD rather
/// than going black. A stack border is where one frame has no data, and the
/// merges downstream weigh frames per pixel; handing them a black wedge would
/// darken the border in exactly the way an unaligned merge does, hiding the
/// defect this module exists to remove. [`coverage`] says which pixels those
/// were, so a merge can drop them instead of trusting them.
pub fn warp_rgb(src: &[[f32; 3]], w: usize, h: usize, warp: &Warp) -> Vec<[f32; 3]> {
    let planes: Vec<Vec<f32>> =
        (0..3).map(|c| src.iter().map(|p| p[c]).collect::<Vec<f32>>()).collect();
    (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (u, v) = warp.apply((k % w) as f32, (k / w) as f32, w, h);
            let mut px = src[k];
            for (c, plane) in planes.iter().enumerate() {
                if let Some(s) = sample(plane, w, h, u, v) {
                    px[c] = s;
                }
            }
            px
        })
        .collect()
}

/// Which pixels of a warped frame actually came from inside the source —
/// `false` where [`warp_rgb`] had to keep the original value.
pub fn coverage(w: usize, h: usize, warp: &Warp) -> Vec<bool> {
    (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (u, v) = warp.apply((k % w) as f32, (k / w) as f32, w, h);
            u >= 0.0 && v >= 0.0 && u <= (w - 1) as f32 && v <= (h - 1) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stack::fixture::{grain, scene};

    /// A noise stack's own scene: a texture near Nyquist over a slow one, plus
    /// per-frame grain, plus a ghost walking through one frame. It is the CLI
    /// fixture this module's alignment defects were measured on, in Rust.
    ///
    /// The fine term matters. A gradient solver on a near-periodic texture has
    /// many almost-equally-good answers, which is exactly the case where an
    /// unchecked step wanders — [`crate::stack::fixture::scene`], whose
    /// structure is spread over several scales, is too forgiving to show it.
    fn burst(w: usize, h: usize, frames: usize, ghost: usize) -> Vec<Vec<[f32; 3]>> {
        (0..frames)
            .map(|s| {
                (0..w * h)
                    .map(|k| {
                        let (x, y) = ((k % w) as f32, (k / w) as f32);
                        if s == ghost
                            && (w / 3..w / 3 + w / 5).contains(&(k % w))
                            && (h / 3..h / 3 + h / 4).contains(&(k / w))
                        {
                            return [0.93; 3];
                        }
                        let v = 0.45
                            + 0.18 * (x * 1.3).sin() * (y * 1.1).cos()
                            + 0.12 * (x * 0.07 + y * 0.05).sin()
                            // [−0.5, 0.5] uniform, so this is σ ≈ 0.029 — the
                            // same grain the CLI fixture the numbers above
                            // were measured on carries.
                            + 0.10 * grain(s as u32 + 1, k);
                        [v, v * 0.99, v * 1.01]
                    })
                    .collect()
            })
            .collect()
    }

    /// v1.5.0 Track S: aligning frames that are ALREADY registered must not
    /// cost the stack anything.
    ///
    /// This is the assertion the aligner is actually FOR, and it is the one
    /// that catches what a per-warp number cannot. Measured through the CLI on
    /// the five-frame noise fixture whose frames are registered exactly, at
    /// three points in this module's history:
    ///
    /// | | corner travel | uncovered | stacked rms |
    /// |---|---|---|---|
    /// | no alignment at all | — | 0 % | 0.01472 (÷2.03) |
    /// | plain least squares, unchecked steps | 539.7 px (the ghost frame) | 69.42 % | — |
    /// | global fit checked, local pass not | 0.0–0.1 px | 3.10 % | 0.05862 |
    /// | + the block pass checks its own step | 0.0–0.1 px | 1.33 % | 0.02914 |
    /// | + `BLOCK_MARGIN` | 0.0–0.1 px | 0.92 % | 0.01597 (÷1.87) |
    /// | + `FIELD_AGREEMENT` | 0.0–0.1 px | 0.87 % | 0.01514 (÷1.97) |
    ///
    /// The third row is why this test exists and why a travel number is not
    /// enough: the global fit was by then correct to a tenth of a pixel, and
    /// the stack was still four times worse than doing nothing, because the
    /// LOCAL pass was inventing multi-pixel motion out of grain. Nothing about
    /// the warp said so; only the merged picture did.
    ///
    /// Both alignments run on the same frames, so the comparison is free of
    /// every other thing that could be wrong with the merge.
    ///
    /// MUTATION: `best` → `a` in `lk_affine`; `scale` → 0 there (plain least
    /// squares); drop the block pass's beats-doing-nothing gate; set
    /// `BLOCK_MARGIN` to 0; let a block keep an answer no neighbour
    /// corroborates.
    #[test]
    fn aligning_frames_that_are_already_registered_costs_the_stack_nothing() {
        use crate::stack::merge::{merge as run, MergeOptions, StackKind};
        // The SIZE of the CLI fixture the table above was measured on, and it
        // matters: the ambiguity a near-periodic texture creates is a question
        // about how many periods sit inside one block, so at 256×192 the same
        // frames survive `BLOCK_MARGIN = 0` and the test would be claiming a
        // guard nothing needs.
        let (w, h) = (384usize, 256usize);
        let frames = burst(w, h, 5, 2);
        // The truth is the scene without grain and without the ghost, which is
        // frame 0's own generator at zero noise — read off a sixth frame whose
        // seed contributes nothing.
        let truth: Vec<[f32; 3]> = (0..w * h)
            .map(|k| {
                let (x, y) = ((k % w) as f32, (k / w) as f32);
                let v = 0.45 + 0.18 * (x * 1.3).sin() * (y * 1.1).cos()
                    + 0.12 * (x * 0.07 + y * 0.05).sin();
                [v, v * 0.99, v * 1.01]
            })
            .collect();
        // The borders are where a warp has nothing to read and every merge
        // falls back; the claim is about the picture.
        let rms = |f: &[[f32; 3]]| {
            let (mut se, mut n) = (0.0f64, 0usize);
            for k in 0..w * h {
                if !(8..w - 8).contains(&(k % w)) || !(8..h - 8).contains(&(k / w)) {
                    continue;
                }
                se += (f[k][1] - truth[k][1]).powi(2) as f64;
                n += 1;
            }
            (se / n as f64).sqrt()
        };
        let stack = |align| {
            run(&frames, w, h, &MergeOptions { kind: StackKind::Noise, align })
                .expect("five frames of one size merge")
        };
        let still = stack(None);
        let aligned = stack(Some(AlignParams::DEFAULT));
        // The premise: without alignment the stack does its job, so anything
        // the aligned one loses is the alignment's doing and nothing else.
        let (one, flat) = (rms(&frames[0]), rms(&still.pixels));
        assert!(one > 0.02 && flat < one / 1.5, "premise: the stack must work at all, {one} → {flat}");
        // PARITY, near enough, and the bound is measured rather than chosen:
        // this fixture answers 0.9850 of the unaligned stack's error shipped —
        // the aligner comes out very slightly AHEAD, because the ghost frame's
        // own blocks are handled — against 1.0217 with `BLOCK_MARGIN` at zero
        // and 1.0172 with the field-agreement test removed. 1.005 sits between
        // them with room on both sides for a platform's last f32 digit; the
        // 1.3 that stood here while the damage was 4× could see neither.
        assert!(
            rms(&aligned.pixels) < 1.005 * flat,
            "aligning registered frames must not cost the stack: {} against {flat} unaligned",
            rms(&aligned.pixels)
        );
        assert!(
            aligned.uncovered < 0.02,
            "…nor push the frame out of its own view: {:.2}% uncovered",
            100.0 * aligned.uncovered
        );
        // The global fit, frame by frame, including the one carrying the
        // ghost — which is the fit that walked 539.7 px. These frames are
        // registered exactly, so every answer but identity is something in the
        // picture talking the solver into a move.
        for k in 1..frames.len() {
            let warp = solve(&frames[0], &frames[k], w, h, &AlignParams::DEFAULT);
            let travel = warp.global.corner_travel(w, h);
            assert!(travel < 1.0, "frame {k} is registered already; the fit moved it {travel} px");
        }
        // …and the ghost is still dropped, which is the other half of what a
        // noise stack is for and the thing an over-cautious aligner could buy
        // its safety with.
        let worst = (0..w * h)
            .filter(|k| {
                (w / 3 + 4..w / 3 + w / 5 - 4).contains(&(k % w))
                    && (h / 3 + 4..h / 3 + h / 4 - 4).contains(&(k / w))
            })
            .map(|k| (aligned.pixels[k][1] - truth[k][1]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.1, "the ghost must not survive an aligned stack either, worst {worst}");
    }

    /// Resample a scene through a KNOWN affine, to make a "moving" frame whose
    /// true answer the test knows. Samples outside the frame wrap to the
    /// clamped edge, which is what the solver's border handling expects.
    fn shift(src: &[[f32; 3]], w: usize, h: usize, a: Affine) -> Vec<[f32; 3]> {
        let planes: Vec<Vec<f32>> =
            (0..3).map(|c| src.iter().map(|p| p[c]).collect::<Vec<f32>>()).collect();
        (0..w * h)
            .map(|k| {
                let (u, v) = a.apply((k % w) as f32, (k / w) as f32, w, h);
                let mut px = [0.0f32; 3];
                for c in 0..3 {
                    px[c] = bilinear_plane(&planes[c], w, h, u, v);
                }
                px
            })
            .collect()
    }

    /// Multiply a frame by `stops` of exposure IN LINEAR LIGHT, then re-encode
    /// — what a bracket actually does, as opposed to scaling the sRGB values.
    fn expose(src: &[[f32; 3]], stops: f32) -> Vec<[f32; 3]> {
        let k = 2.0f32.powf(stops);
        src.iter()
            .map(|p| {
                let mut o = [0.0f32; 3];
                for c in 0..3 {
                    let lin = crate::render::srgb_to_linear(p[c]);
                    o[c] = crate::render::linear_to_srgb((lin * k).min(1.0));
                }
                o
            })
            .collect()
    }

    /// A background that stayed put, a rectangle of it that MOVED, and a flat
    /// region that can say nothing — the three things the local pass has to
    /// tell apart.
    ///
    /// The regions are drawn on BLOCK boundaries, so no block has to average
    /// two different truths and blunt an assertion into a shrug.
    struct SubjectScene {
        reference: Vec<[f32; 3]>,
        moving: Vec<[f32; 3]>,
        cols: std::ops::Range<usize>,
        rows: std::ops::Range<usize>,
        flat: std::ops::Range<usize>,
    }

    /// Build one against a `bx`×`by` block grid: the subject is the middle
    /// third of the columns, the flat region is the last third, and the
    /// subject's content is read through `dx` pixels — so putting it back
    /// takes `−dx`, the direction [`solve`] answers in.
    fn subject_scene(w: usize, h: usize, bx: usize, by: usize, dx: f32, ev: f32) -> SubjectScene {
        let base = scene(w, h);
        let moved = shift(&base, w, h, Affine([1.0, 0.0, dx, 0.0, 1.0, 0.0]));
        let (cols, rows) = ((bx / 3)..(2 * bx / 3), (by / 4)..(by - by / 4));
        let flat = (2 * bx / 3)..bx;
        let (px0, px1) = (cols.start * w / bx, cols.end * w / bx);
        let (py0, py1) = (rows.start * h / by, rows.end * h / by);
        let fx = flat.start * w / bx;
        let (mut reference, mut moving) = (base.clone(), base);
        for k in 0..w * h {
            let (x, y) = (k % w, k / w);
            if (px0..px1).contains(&x) && (py0..py1).contains(&y) {
                moving[k] = moved[k];
            }
            if x >= fx {
                reference[k] = [0.5, 0.5, 0.5];
                moving[k] = [0.5, 0.5, 0.5];
            }
        }
        // The bracket, applied LAST so the flat region darkens with the rest
        // of the frame: a real bracket does not exempt the sky.
        let moving = expose(&moving, ev);
        SubjectScene { reference, moving, cols, rows, flat }
    }

    /// The residual-x each block reported, split into the three regions the
    /// scene was built from, in the order (subject, ground, flat).
    fn read_regions(warp: &Warp, s: &SubjectScene) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let (mut subject, mut ground, mut flat) = (Vec::new(), Vec::new(), Vec::new());
        for bj in 0..warp.by {
            for bi in 0..warp.bx {
                let d = warp.local[bj * warp.bx + bi][0];
                if s.flat.contains(&bi) {
                    flat.push(d);
                } else if s.cols.contains(&bi) && s.rows.contains(&bj) {
                    subject.push(d);
                } else {
                    ground.push(d);
                }
            }
        }
        (subject, ground, flat)
    }

    fn mean(v: &[f32]) -> f32 {
        v.iter().sum::<f32>() / v.len() as f32
    }

    /// v1.5.0 Track S: the global fit survives a TWO-STOP exposure difference
    /// with a six-pixel shift under it — end to end, what a bracket presents.
    ///
    /// What this test does NOT establish is that the log-luma normalisation is
    /// what carried it. Both of the mutations that were written for it —
    /// deleting the mean subtraction, and deleting `srgb_to_linear` from
    /// `log_luma` — were driven, and both stayed GREEN. A whole frame's
    /// gradients sum to about nothing, so the exposure offset cancels without
    /// help. The normalisation is bound by the LOCAL pass's test instead,
    /// whose blocks are subsets small enough that the cancellation fails; see
    /// the module doc.
    ///
    /// MUTATION: flip the sign of the Gauss–Newton update in `lk_affine`.
    #[test]
    fn a_two_stop_bracket_still_aligns_because_the_match_is_exposure_blind() {
        let (w, h) = (192usize, 144usize);
        let reference = scene(w, h);
        let truth = Affine([1.0, 0.0, 6.0, 0.0, 1.0, -4.0]);
        let moving = expose(&shift(&reference, w, h, truth), 2.0);
        // The premise: two stops really did change these pixels a lot, so the
        // solve below is not being handed two near-identical frames.
        let mean = |f: &[[f32; 3]]| f.iter().map(|p| p[1] as f64).sum::<f64>() / f.len() as f64;
        assert!(
            mean(&moving) - mean(&reference) > 0.2,
            "premise: the bracket must be a real exposure difference: {} vs {}",
            mean(&reference),
            mean(&moving)
        );

        let warp = solve(&reference, &moving, w, h, &AlignParams { blocks: 0, ..Default::default() });
        let p = warp.global.0;
        // The INVERSE, because `moving` was built by reading `reference`
        // through `truth`: the solver answers in the resampler's direction.
        let want = truth.inverse().expect("a translation is invertible").0;
        assert!(
            (p[2] - want[2]).abs() < 0.5 && (p[5] - want[5]).abs() < 0.5,
            "the translation must come back within half a pixel: got {p:?}, want {want:?}"
        );
        assert!(
            (p[0] - 1.0).abs() < 0.01 && (p[4] - 1.0).abs() < 0.01,
            "…and no scale must be invented: {p:?}"
        );
    }

    /// v1.5.0 Track S: a rotation and a scale come back too, not just a shift.
    ///
    /// Six parameters are claimed and six are probed. A solver that only ever
    /// moved the two translations would pass the bracket test above.
    ///
    /// MUTATION: zero the four linear columns of the Jacobian in `lk_affine`.
    #[test]
    fn the_affine_recovers_a_roll_and_a_breath_not_only_a_shift() {
        let (w, h) = (192usize, 144usize);
        let reference = scene(w, h);
        // ~1.15° of roll and 1.2 % of scale — the size of a handheld frame's
        // drift, and small enough that the corners stay inside.
        let (c, s, k) = (0.9998f32, 0.0201f32, 1.012f32);
        let truth = Affine([c * k, -s * k, 2.0, s * k, c * k, 1.0]);
        let moving = shift(&reference, w, h, truth);

        let warp = solve(&reference, &moving, w, h, &AlignParams { blocks: 0, ..Default::default() });
        let p = warp.global.0;
        let want = truth.inverse().expect("a roll with scale is invertible").0;
        for (k, what) in [(0, "a"), (1, "b"), (3, "c"), (4, "d")] {
            assert!(
                (p[k] - want[k]).abs() < 0.004,
                "{what}: got {}, want {} in {p:?}",
                p[k],
                want[k]
            );
        }
        assert!(
            (p[2] - want[2]).abs() < 0.5 && (p[5] - want[5]).abs() < 0.5,
            "…and the translation with it: got {p:?}, want {want:?}"
        );
    }

    /// v1.5.0 Track S: the local pass finds a SUBJECT that moved, and refuses
    /// to invent a residual where there is no texture to measure one.
    ///
    /// Both halves of that claim in one frame, because each is worthless
    /// alone: a pass that refused every block would satisfy "no residual on
    /// flat ground", and a pass with no texture gate would satisfy "the
    /// subject is found". So the frame has three regions — a background that
    /// stayed put, a rectangle of it that moved, and a flat quarter that can
    /// say nothing.
    ///
    /// **What the scene has to avoid, and why it took three tries.** The local
    /// pass only ever sees what the global affine LEFT, so a scene the affine
    /// can represent tests nothing:
    ///
    /// * a uniform shift is absorbed entirely — every block correctly reports
    ///   zero, and the test fails on its own premise;
    /// * two bands moving OPPOSITELY are absorbed too, which is much less
    ///   obvious: in centred coordinates they are two points, and a horizontal
    ///   scale is a line through two points. Measured, on the ±4 px version of
    ///   this scene: the global fit came back a = 1.0467, tx = 2.909 — the
    ///   least-squares line through (−127.5, −4) and (+0.5, +4) — and the
    ///   local pass then correctly found the −0.96 / +1.07 that was left. The
    ///   solver was right and the test was wrong.
    ///
    /// A rectangle moving against its background is the case an affine cannot
    /// take: the displacement is piecewise, and the rectangle is centred, so
    /// it contributes nothing to the linear part and only a fraction of its
    /// own shift to the translation. It is also what parallax actually looks
    /// like in a handheld bracket.
    ///
    /// The geometry is chosen so every region lands on block boundaries
    /// exactly — 384×288 with a 6×4 grid is 64×72 px per block — because a
    /// block straddling two truths averages them and would blunt the
    /// assertion rather than test it.
    ///
    /// **The two numbers are chosen to bind things nothing else does.** The
    /// subject moves TWELVE pixels, which is past what one Gauss–Newton step
    /// can see on this content, so the block pyramid has to carry it; at six
    /// pixels a single-scale pass got there on its own and the pyramid was
    /// free to be broken. And the moving frame is a stop and a half DARKER,
    /// because that is where the log-luma normalisation is actually
    /// load-bearing — a block is a small enough subset that the exposure
    /// offset no longer cancels itself (see the module doc).
    ///
    /// MUTATION: drop the Shi–Tomasi gate (`min_texture`) — the flat quarter
    /// starts reporting motion; use the LARGER eigenvalue or the trace — same;
    /// drop the local pass — the subject reports nothing; kill a block on
    /// `n < 32` instead of skipping the level — the coarse level, where a
    /// block is smallest, wipes most of the grid; give the block pass one
    /// level; drop the mean subtraction in `accumulate`. (Dropping
    /// `srgb_to_linear` from `luma` does NOT drive this red — the module doc
    /// says why that one has no test anywhere.)
    #[test]
    fn the_local_pass_finds_a_subject_that_moved_and_refuses_flat_ground() {
        let (w, h) = (384usize, 288usize);
        let params = AlignParams { blocks: 6, ..Default::default() };
        let s = subject_scene(w, h, 6, 4, 12.0, -1.5);
        let warp = solve(&s.reference, &s.moving, w, h, &params);
        assert!(!warp.local.is_empty(), "a moving subject must leave a residual field");
        assert_eq!(
            (warp.bx, warp.by),
            (6, 4),
            "premise: the grid the three regions were drawn against"
        );
        let (subject, ground, flat) = read_regions(&warp, &s);
        // Twelve pixels apart before the global affine takes its share; the
        // assertion asks for eight, so a pass that recovered less than two
        // thirds of the subject's motion still fails.
        assert!(
            mean(&ground) - mean(&subject) > 8.0,
            "the subject moved against its ground and must read that way: {:?} vs {:?}",
            mean(&subject),
            mean(&ground)
        );
        assert!(
            flat.iter().all(|d| *d == 0.0),
            "a block with no texture must report nothing, got {flat:?}"
        );
    }

    /// v1.5.0 Track S: a SMALL frame keeps the block grid it can still
    /// measure, instead of losing most of it on the coarsest level.
    ///
    /// The frame matters because block size follows it: 192×144 with a 9×6
    /// grid puts a block at 5×6 px two levels up, which is under the sample
    /// floor the per-block solve needs. Treating that as "this block is
    /// unusable" rather than "this LEVEL cannot help this block" killed the
    /// block for every finer level too, and since the coarsest level is
    /// exactly where blocks are smallest, it wiped six of nine columns —
    /// leaving a field assembled from whichever blocks happened to round up
    /// to 36 px, whose 2×2 normal equations are noise. It read as a plausible
    /// small residual with the wrong sign, not as a failure.
    ///
    /// So the assertion is about the GRID, not about a magnitude: every block
    /// over the moving subject must still be reporting something. It is the
    /// SUBJECT's blocks and not every textured block, because a block with
    /// nothing to report answers zero on purpose since the local pass started
    /// checking its answer against doing nothing — and the ground here has
    /// nothing to report. A killed block and an honest zero look the same from
    /// out here; a killed SUBJECT block does not, because the subject moved.
    ///
    /// MUTATION: kill the block on the sample floor (`return ([0.0, 0.0],
    /// false)`) instead of skipping the level; cap the block pyramid by block
    /// SIDE instead — the levels that carry the reach go with it.
    #[test]
    fn a_small_frame_keeps_every_block_it_can_still_measure() {
        let (w, h) = (192usize, 144usize);
        let params = AlignParams { blocks: 9, ..Default::default() };
        // No exposure difference here: this one is about the GRID surviving,
        // and a second variable in it would only make a failure ambiguous.
        let s = subject_scene(w, h, 9, 6, 3.0, 0.0);
        let warp = solve(&s.reference, &s.moving, w, h, &params);
        assert!(!warp.local.is_empty(), "a moving subject must leave a residual field");
        assert_eq!((warp.bx, warp.by), (9, 6), "premise: the grid the regions were drawn against");
        let (subject, ground, flat) = read_regions(&warp, &s);
        assert!(
            subject.iter().all(|d| d.abs() > 1e-6),
            "every block over the moving subject must still be measuring: {subject:?}"
        );
        assert!(
            mean(&ground) - mean(&subject) > 1.5,
            "…and the subject must still read as having moved: {:?} vs {:?}",
            mean(&subject),
            mean(&ground)
        );
        assert!(
            flat.iter().all(|d| *d == 0.0),
            "a block with no texture must report nothing, got {flat:?}"
        );
    }

    /// v1.5.0 Track S: `warp_rgb` puts the frame back where it came from,
    /// `coverage` marks the wedge that had no source, and that wedge KEEPS
    /// what it had.
    ///
    /// The third clause is asserted separately because the first two cannot
    /// see it: they only ever look at covered pixels, so a version that filled
    /// the wedge with black passed them both. A merge downstream weighs frames
    /// per pixel and drops whatever `coverage` marks; handing it black instead
    /// would darken exactly the border that an unaligned merge darkens, hiding
    /// the defect this module exists to remove.
    ///
    /// MUTATION: fill outside samples with black instead of keeping the
    /// original; return `true` everywhere from `coverage`.
    #[test]
    fn warping_a_frame_back_reproduces_it_and_names_the_uncovered_wedge() {
        let (w, h) = (96usize, 72usize);
        let reference = scene(w, h);
        let truth = Affine([1.0, 0.0, 5.0, 0.0, 1.0, 3.0]);
        let moving = shift(&reference, w, h, truth);
        // Warping `moving` by the INVERSE puts it back on `reference` — the
        // same direction `solve` answers in.
        let back = Warp {
            global: truth.inverse().expect("a translation is invertible"),
            local: Vec::new(),
            bx: 0,
            by: 0,
        };
        let restored = warp_rgb(&moving, w, h, &back);
        let cov = coverage(w, h, &back);
        let (mut n, mut worst) = (0usize, 0.0f32);
        for k in 0..w * h {
            if !cov[k] {
                continue;
            }
            n += 1;
            worst = worst.max((restored[k][1] - reference[k][1]).abs());
        }
        assert!(n > w * h / 2, "most of the frame must be covered, got {n} of {}", w * h);
        assert!(worst < 0.02, "a round trip must reproduce the frame, worst {worst}");
        let uncovered = cov.iter().filter(|c| !**c).count();
        assert_eq!(
            uncovered,
            w * h - n,
            "coverage and the loop above must be talking about the same pixels"
        );
        assert!(uncovered > 0, "a 5x3 px shift really does leave a wedge with no source");
        let kept = (0..w * h)
            .filter(|k| !cov[*k])
            .map(|k| (restored[k][1] - moving[k][1]).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(kept, 0.0, "an uncovered pixel must keep the value it had, worst {kept}");
    }

    /// v1.5.0 Track S: a shift big enough to push a FIFTH of the frame out of
    /// view still comes back.
    ///
    /// This is the test that makes `sample`'s bounds check cost something. It
    /// returns `None` outside the frame rather than letting `bilinear_plane`
    /// clamp, and at the six-pixel shifts the other tests use only 3 % of the
    /// frame is off the edge, so letting it clamp instead changed none of
    /// them. Here it is 22 %, and a solver matching the replicated edge
    /// against real content would be fitting a band of nonsense a fifth of the
    /// frame wide — pulling a large shift back toward zero, which is the one
    /// direction the error is not symmetric in.
    ///
    /// The shift is 12.5 % of the width and 11 % of the height, which is a
    /// long way past what a handheld bracket drifts — chosen so the wedge is
    /// large enough to matter, not because a stack is expected to need it.
    /// What a stack IS expected to need is written on
    /// [`AlignParams::levels`], with the measurements behind it.
    ///
    /// MUTATION: let `sample` clamp instead of refusing (`inside = true`);
    /// fit all six affine entries at every level.
    #[test]
    fn a_shift_that_pushes_a_fifth_of_the_frame_out_of_view_still_comes_back() {
        let (w, h) = (384usize, 288usize);
        let reference = scene(w, h);
        let truth = Affine([1.0, 0.0, 48.0, 0.0, 1.0, -32.0]);
        let moving = shift(&reference, w, h, truth);
        let warp = solve(&reference, &moving, w, h, &AlignParams { blocks: 0, ..Default::default() });
        let want = truth.inverse().expect("a translation is invertible").0;
        let p = warp.global.0;
        assert!(
            (p[2] - want[2]).abs() < 0.5 && (p[5] - want[5]).abs() < 0.5,
            "the translation must come back within half a pixel: got {p:?}, want {want:?}"
        );
        let out = coverage(w, h, &warp).iter().filter(|c| !**c).count();
        assert!(
            out > w * h / 6,
            "premise: a fifth of the frame really is out of view, got {out} of {}",
            w * h
        );
    }
}
