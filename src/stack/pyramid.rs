//! The image pyramid a stack needs, in one definition for both halves of it.
//!
//! The aligner walks a pyramid to find a warp it could not have found at full
//! resolution; the merges walk one to join two pictures without leaving a seam
//! where the blend changed its mind. Those are different jobs sharing one
//! primitive, and the primitive lives here rather than inside either of them
//! because a merge that halved a frame differently from the aligner would be
//! blending frames the warp was never solved for.
//!
//! Everything here works on a single f32 PLANE. Colour is three planes, and
//! the callers split it themselves, because the weights a merge builds are per
//! pixel and not per channel — a weight that differed between channels would
//! not be a weight, it would be a colour cast.

use crate::render::{bilinear_plane, gauss_blur_plane};
use rayon::prelude::*;

/// One level of a pyramid: the plane, and the size it actually is.
///
/// The size travels WITH the plane because [`reduce`] truncates an odd edge —
/// 49 px halves to 24 — so the dimensions of a level cannot be recovered by
/// halving the base that many times.
pub(crate) type Level = (Vec<f32>, usize, usize);

/// One level down: LOW-PASS, then a 2×2 box average, odd edges included by
/// clamping.
///
/// The blur is not a quality nicety. A 2×2 box alone leaves every frequency
/// above the new Nyquist folded back into the coarse level as ALIAS, and an
/// alias is not a photograph: in an alignment it moves differently in the two
/// frames, and in a merge it appears in one frame's band and not in another's.
/// σ = 1 in the SOURCE level's pixels is the standard half-octave Gaussian —
/// enough to put the folded band well down, little enough that the structure
/// the next level needs survives.
///
/// Measured, and it took a large enough displacement to show. On the six-pixel
/// shifts most of `align`'s tests use, deleting the blur changes NONE of them:
/// both frames alias the same way and the difference cancels. On the 48 px
/// shift of `a_shift_that_pushes_a_fifth_of_the_frame_out_of_view_still_comes_back`
/// — the one case that needs every level of the pyramid to be telling the
/// truth — deleting it breaks the solve outright. A guard whose only evidence
/// is the textbook is worth suspecting; this one has a test.
///
/// NOT `render::downscale_f32`, which takes `[f32; 3]` pixels and an arbitrary
/// target edge. This is a single plane at exactly half size, and the exactness
/// is what lets a translation scale by a clean factor of two between levels.
pub(crate) fn reduce(src: &[f32], w: usize, h: usize) -> Level {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let lp = gauss_blur_plane(src, w, h, 1.0);
    let mut out = vec![0.0f32; nw * nh];
    out.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (x0, y0) = (x * 2, y * 2);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            *o = 0.25 * (lp[y0 * w + x0] + lp[y0 * w + x1] + lp[y1 * w + x0] + lp[y1 * w + x1]);
        }
    });
    (out, nw, nh)
}

/// One level UP, to an EXPLICIT target size.
///
/// The target is given rather than doubled for the reason [`Level`] carries
/// its size: 49 reduces to 24, and 24 doubled is 48. A Laplacian band has to
/// land back on the level it was subtracted from, to the pixel, or the
/// reconstruction drifts by a column at every odd level.
///
/// The half-pixel in the mapping is the grid, not a fudge: [`reduce`] makes
/// output pixel `x` out of source pixels `2x` and `2x+1`, whose centre is
/// `2x + 0.5`, so the inverse of that is `(x − 0.5) / 2`. Dropping it shifts
/// every expanded band half a coarse pixel against the band it is added to,
/// which reads as a soft double edge rather than as a wrong answer.
pub(crate) fn expand(src: &[f32], lw: usize, lh: usize, w: usize, h: usize) -> Vec<f32> {
    (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (x, y) = ((k % w) as f32, (k / w) as f32);
            bilinear_plane(src, lw, lh, (x - 0.5) * 0.5, (y - 0.5) * 0.5)
        })
        .collect()
}

/// The Gaussian pyramid of a plane: `levels` entries counting the base, and
/// stopping early rather than reducing a level whose short side is under 8 px,
/// where a Gaussian has nothing left to average.
pub(crate) fn gaussian(base: Vec<f32>, w: usize, h: usize, levels: usize) -> Vec<Level> {
    let mut out = vec![(base, w, h)];
    while out.len() < levels.max(1) {
        let (p, lw, lh) = out.last().expect("a pyramid always has its base");
        if (*lw).min(*lh) < 8 {
            break;
        }
        let (r, nw, nh) = reduce(p, *lw, *lh);
        out.push((r, nw, nh));
    }
    out
}

/// The Laplacian pyramid of a Gaussian one: every level minus the level above
/// it expanded back, and the coarsest level kept WHOLE.
///
/// The coarsest level is kept whole because it is the only thing carrying the
/// picture's overall brightness; every other level is a difference and
/// averages to nothing. A blend that dropped it would produce a correctly
/// detailed image of a uniform grey.
pub(crate) fn laplacian(g: &[Level]) -> Vec<Level> {
    let mut out = Vec::with_capacity(g.len());
    for (k, (p, w, h)) in g.iter().enumerate() {
        if k + 1 == g.len() {
            out.push((p.clone(), *w, *h));
            break;
        }
        let (up, uw, uh) = &g[k + 1];
        let e = expand(up, *uw, *uh, *w, *h);
        out.push((p.iter().zip(&e).map(|(a, b)| a - b).collect(), *w, *h));
    }
    out
}

/// Rebuild a plane from a Laplacian pyramid.
///
/// On an untouched pyramid this is the exact inverse of [`laplacian`], and
/// that exactness is what makes a per-level weighted sum a seamless BLEND
/// rather than a soft-focus copy: whatever the weights do not change comes
/// back unchanged.
pub(crate) fn collapse(l: &[Level]) -> Level {
    let mut cur = l.last().expect("a pyramid always has its base").clone();
    for (band, w, h) in l.iter().take(l.len().saturating_sub(1)).rev() {
        let e = expand(&cur.0, cur.1, cur.2, *w, *h);
        cur = (band.iter().zip(&e).map(|(a, b)| a + b).collect(), *w, *h);
    }
    cur
}

/// How deep a blend pyramid a frame allows.
///
/// As deep as it can be: a blend's whole job is to move each seam into a band
/// where it is invisible, and the lowest band is the widest seam. Halving
/// stops while the short side is still 16 px, so the coarsest level is never
/// so small that a weight map there is a single blob.
pub(crate) fn depth(w: usize, h: usize) -> usize {
    let (mut n, mut s) = (1usize, w.min(h));
    while s >= 16 {
        s /= 2;
        n += 1;
    }
    n
}

/// Blend several frames by one weight map each, band by band.
///
/// THE reason the pyramid is in this module. A per-pixel weighted average of
/// two exposures leaves a visible seam wherever the weights change faster than
/// the two pictures agree; blurring the weights to hide it puts a halo around
/// every subject instead. Blending each frequency band with the weights
/// blurred to THAT band's own scale does neither — it is Burt & Adelson's
/// multiresolution spline, and Mertens' exposure fusion is exactly this with
/// three particular weights.
///
/// The weights are normalised HERE, once, so that no caller can forget: a band
/// summed with weights that do not add to one is that band scaled by whatever
/// they did add to, which reads as a brightness stain in the shape of the
/// weight map. Where every frame's weight is zero — a pixel clipped in all of
/// them — the FIRST frame is taken whole, because a stack's first frame is its
/// reference and an invented average of nothing is worse than a known pixel.
///
/// Memory: one channel's frame pyramids at a time, so the peak is roughly
/// `frames × 1.4 × w × h` floats for that channel plus the same again for the
/// weights — not three times over. Callers working at full sensor resolution
/// should still expect this to be the largest allocation in the run.
pub(crate) fn blend_rgb(
    frames: &[Vec<[f32; 3]>],
    weights: &[Vec<f32>],
    w: usize,
    h: usize,
    levels: usize,
) -> Vec<[f32; 3]> {
    let mut norm = weights.to_vec();
    for k in 0..w * h {
        let s: f32 = norm.iter().map(|m| m[k]).sum();
        for (i, m) in norm.iter_mut().enumerate() {
            m[k] = if s > 1e-12 {
                m[k] / s
            } else {
                f32::from(i == 0)
            };
        }
    }
    let wp: Vec<Vec<Level>> = norm.into_iter().map(|m| gaussian(m, w, h, levels)).collect();
    let depth = wp[0].len();
    let mut out = [Vec::new(), Vec::new(), Vec::new()];
    for (c, o) in out.iter_mut().enumerate() {
        let bands: Vec<Vec<Level>> = frames
            .iter()
            .map(|f| laplacian(&gaussian(f.iter().map(|p| p[c]).collect(), w, h, depth)))
            .collect();
        let mixed: Vec<Level> = (0..depth)
            .map(|lv| {
                let (lw, lh) = (bands[0][lv].1, bands[0][lv].2);
                let mut acc = vec![0.0f32; lw * lh];
                for (band, wt) in bands.iter().zip(&wp) {
                    for (k, a) in acc.iter_mut().enumerate() {
                        *a += band[lv].0[k] * wt[lv].0[k];
                    }
                }
                (acc, lw, lh)
            })
            .collect();
        *o = collapse(&mixed).0;
    }
    (0..w * h).map(|k| [out[0][k], out[1][k], out[2][k]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stack::fixture::plane;

    /// v1.5.0 Track S: taking a plane apart into bands and adding them back up
    /// returns the plane.
    ///
    /// Everything a merge does is "change one band and put it back", so this
    /// identity is the floor under all of it. Without it, a merge that changed
    /// NOTHING would still soften the picture — and the softening would be
    /// blamed on the weights.
    ///
    /// MUTATION: drop the coarsest level from `laplacian`. (NOT the half-pixel
    /// in `expand` — the test below says why this identity cannot see it.)
    #[test]
    fn a_plane_taken_apart_into_bands_adds_back_up_to_itself() {
        // 97×61 on purpose: both odd, so every level truncates and the
        // expand-to-an-explicit-target rule is doing real work.
        let (w, h) = (97usize, 61usize);
        let p = plane(w, h);
        let g = gaussian(p.clone(), w, h, 5);
        assert!(g.len() >= 3, "premise: the pyramid needs levels to test, got {}", g.len());
        let back = collapse(&laplacian(&g));
        assert_eq!((back.1, back.2), (w, h), "the rebuilt plane must be the size it started");
        let worst = p.iter().zip(&back.0).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "reconstruction must be exact to float noise, worst {worst}");
    }

    /// v1.5.0 Track S: a ramp comes back from a level down without SLIDING.
    ///
    /// The reconstruction test above cannot catch a wrong sampling grid, and
    /// it took a mutation to notice: that identity subtracts and then re-adds
    /// the SAME `expand`, so any expand operator at all reconstructs exactly,
    /// a half-pixel error included. It is a statement about `laplacian` and
    /// `collapse` being inverses of each other, not about either being right.
    ///
    /// A ramp is the probe that does notice, because a constant offset in the
    /// sampling grid moves a ramp by a constant amount — half a fine pixel is
    /// 0.006 of this one, and the assertion sits well under that. A ramp is
    /// also the one signal a Gaussian and a box filter both reproduce exactly,
    /// so whatever is left over IS the grid.
    ///
    /// MUTATION: drop the −0.5 from `expand`'s mapping, or make it +0.5.
    #[test]
    fn a_ramp_comes_back_from_a_level_down_without_sliding() {
        let (w, h) = (64usize, 48usize);
        let ramp: Vec<f32> =
            (0..w * h).map(|k| 0.1 + 0.8 * (k % w) as f32 / (w - 1) as f32).collect();
        let (small, sw, sh) = reduce(&ramp, w, h);
        let back = expand(&small, sw, sh, w, h);
        // The border is where a clamped filter has nothing to work with; the
        // interior is the claim.
        let worst = (0..w * h)
            .filter(|k| (4..w - 4).contains(&(k % w)) && (4..h - 4).contains(&(k / w)))
            .map(|k| (back[k] - ramp[k]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.002, "a ramp must come back where it was, worst {worst}");
    }

    /// v1.5.0 Track S: a blend across a HARD weight edge leaves no edge.
    ///
    /// The whole reason the pyramid is in this module. Two frames a long way
    /// apart in brightness, and a weight map that switches from one to the
    /// other in a single column: a per-pixel average would step by the full
    /// difference at that column, and this must not.
    ///
    /// Both halves are asserted, because each alone is satisfied by something
    /// useless: "no step" is satisfied by ignoring the weights and returning
    /// one frame everywhere, and "follows the weights" is satisfied by the
    /// per-pixel average that steps.
    ///
    /// The weights are 4 and 7, not 1 and 0, on purpose: they are meant to be
    /// RATIOS, and a pair that already sums to one lets an unnormalised blend
    /// pass while scaling every band by whatever the weights happened to add
    /// up to. That is the mutation this missed the first time round. The depth
    /// comes from [`depth`] rather than a literal, for the same reason — a
    /// number written here is a number the shipped path does not use.
    ///
    /// MUTATION: make `depth` return 1 — the step comes back at full height;
    /// skip the normalisation in `blend_rgb` — each half comes out four or
    /// seven times its own frame.
    #[test]
    fn a_hard_weight_edge_blends_without_a_step() {
        let (w, h) = (128usize, 96usize);
        let base = plane(w, h);
        let dark: Vec<[f32; 3]> = base.iter().map(|v| [*v * 0.25; 3]).collect();
        let bright: Vec<[f32; 3]> = base.iter().map(|v| [*v * 0.25 + 0.5; 3]).collect();
        let cut = w / 2;
        let left: Vec<f32> = (0..w * h).map(|k| if k % w < cut { 4.0 } else { 0.0 }).collect();
        let right: Vec<f32> = (0..w * h).map(|k| if k % w < cut { 0.0 } else { 7.0 }).collect();
        let out = blend_rgb(&[dark.clone(), bright.clone()], &[left, right], w, h, depth(w, h));

        // Follows the weights: well away from the cut each half IS its own
        // frame, so the blend really did choose rather than average.
        let far = |x: usize, f: &[[f32; 3]]| {
            (0..h).map(|y| (out[y * w + x][1] - f[y * w + x][1]).abs()).fold(0.0f32, f32::max)
        };
        assert!(far(8, &dark) < 0.02, "the left side must be the dark frame, off by {}", far(8, &dark));
        assert!(
            far(w - 9, &bright) < 0.02,
            "the right side must be the bright frame, off by {}",
            far(w - 9, &bright)
        );
        // And no step: the biggest column-to-column jump anywhere has to be
        // far below the 0.5 the two frames differ by.
        let step = (1..w)
            .flat_map(|x| (0..h).map(move |y| (x, y)))
            .map(|(x, y)| (out[y * w + x][1] - out[y * w + x - 1][1]).abs())
            .fold(0.0f32, f32::max);
        assert!(step < 0.08, "a hard weight edge must not become a visible one, worst step {step}");
    }
}
