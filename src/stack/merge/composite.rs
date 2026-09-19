//! The three merges that never form a radiance.
//!
//! Exposure fusion and the focus stack are the SAME operator with different
//! weights — pick each frequency band from wherever it looks best, through
//! [`pyramid::blend_rgb`] — and they are written that way on purpose, because
//! what a photographer means by "blend where it is best" is one idea and only
//! the definition of "best" changes. The noise stack is not that idea at all:
//! it is an estimate of one unchanging scene from repeated readings, so it
//! averages in linear light and its only judgement is which readings to throw
//! out.

use super::{detail, saturation, well_exposed};
use crate::render::{gauss_blur_plane, linear_to_srgb, srgb_to_linear};
use crate::stack::pyramid;

/// The plane the local measures are taken on: the mean of the three ENCODED
/// channels.
///
/// NOT [`crate::stack::luma`], which is linear luminance and is the right
/// thing for measuring LIGHT. Everything in this file is a judgement about how
/// a picture looks, made in the space the picture is displayed in, and using a
/// linear luminance here would quietly weight a shadow's detail at a fraction
/// of an identical highlight's.
fn grey(f: &[[f32; 3]]) -> Vec<f32> {
    f.iter().map(|p| (p[0] + p[1] + p[2]) / 3.0).collect()
}

/// A floor under every weight, so a region that is flat, grey and dull in
/// every frame still has to come from somewhere rather than from a division by
/// zero. It is small enough to be invisible wherever any frame has an opinion.
const FLOOR: f32 = 1e-4;

/// Mertens' exposure fusion: each band of the picture taken from wherever it
/// is best exposed, with no radiance in between.
///
/// Three weights multiplied, which is Mertens' own recipe and each one covers
/// a case the others miss: DETAIL, so a blown-out region loses to one that
/// still has texture; SATURATION, so a washed-out region loses to one that
/// still has colour, which is what catches a highlight on its way to clipping
/// before the texture has gone; and WELL-EXPOSEDNESS, so what is left is
/// decided by which frame put this pixel nearest the middle.
pub(super) fn fuse(
    frames: &[Vec<[f32; 3]>],
    cover: &[Vec<bool>],
    w: usize,
    h: usize,
) -> Vec<[f32; 3]> {
    let weights: Vec<Vec<f32>> = frames
        .iter()
        .zip(cover)
        .map(|(f, c)| {
            let d = detail(&grey(f), w, h);
            (0..w * h)
                .map(|k| {
                    if !c[k] {
                        return 0.0;
                    }
                    let exposed: f32 = f[k].iter().copied().map(well_exposed).product();
                    (d[k] + FLOOR) * (saturation(&f[k]) + FLOOR) * exposed
                })
                .collect()
        })
        .collect();
    pyramid::blend_rgb(frames, &weights, w, h, pyramid::depth(w, h))
}

/// How far a sharpness measure is spread before it decides anything, as a
/// fraction of the frame's short side.
///
/// A Laplacian is large on an EDGE and zero on the smooth ground between
/// edges, so a weight map taken from it raw is a stencil of edges: the blend
/// would pick frame A along a contour and frame B a pixel to either side of
/// it, which is not what "this region is in focus" means. Blurring first turns
/// the measure into a statement about a neighbourhood. 0.6 % of the short side
/// is about 20 px on a 24 MP frame — wider than any edge, narrower than the
/// smallest thing a photographer focuses on deliberately.
const SHARPNESS_SPREAD: f32 = 0.006;

/// How hard the sharpest frame wins.
///
/// Cubing is a compromise with a reason on both sides. A linear weight blends
/// a sharp frame with a soft one wherever they are close, and a blend of sharp
/// and soft is soft — the whole failure a focus stack exists to avoid. A
/// winner-takes-all picks a single frame per pixel and shows every place it
/// changed its mind. The cube leaves a clear region entirely to the frame that
/// resolved it while still crossfading where two frames genuinely agree.
const SHARPNESS_DECISIVENESS: i32 = 3;

/// A focus stack: each band taken from the frame that resolved it.
pub(super) fn focus(
    frames: &[Vec<[f32; 3]>],
    cover: &[Vec<bool>],
    w: usize,
    h: usize,
) -> Vec<[f32; 3]> {
    let sigma = (SHARPNESS_SPREAD * w.min(h) as f32).max(2.0);
    let weights: Vec<Vec<f32>> = frames
        .iter()
        .zip(cover)
        .map(|(f, c)| {
            let d = gauss_blur_plane(&detail(&grey(f), w, h), w, h, sigma);
            (0..w * h)
                .map(|k| {
                    if c[k] {
                        (d[k] + FLOOR).powi(SHARPNESS_DECISIVENESS)
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect();
    pyramid::blend_rgb(frames, &weights, w, h, pyramid::depth(w, h))
}

/// How many robust spreads away a reading has to be before it stops counting.
///
/// Three, so ordinary noise is barely touched — a Gaussian puts 99.7 % of its
/// readings inside three sigma — while something that is not noise at all,
/// like a person who walked through one frame of the stack, is gone.
const GHOST_SPREADS: f32 = 3.0;

/// The noise stack: repeated readings of one scene, averaged in LINEAR light,
/// with the readings that disagree withdrawn.
///
/// Linear light because that is where a photon count adds. Averaging the
/// ENCODED values would average through a curve and darken every edge between
/// two brightnesses — the same mistake as resizing in gamma space, and just as
/// invisible until it is pointed out.
///
/// The withdrawal is per pixel and per channel, against the MEDIAN of that
/// pixel's readings rather than their mean, because the mean is exactly what a
/// ghost moves. The spread it is measured in is that pixel's own median
/// absolute deviation, floored at a frame-wide estimate: a per-pixel spread
/// from three or four frames is a crude number, and the floor stops a pixel
/// whose readings happen to agree perfectly from rejecting its neighbours over
/// a rounding difference.
pub(super) fn average(
    frames: &[Vec<[f32; 3]>],
    cover: &[Vec<bool>],
    w: usize,
    h: usize,
) -> Vec<[f32; 3]> {
    let n = w * h;
    let floor = frame_spread(frames, cover, n);
    (0..n)
        .map(|k| {
            let mut out = [0.0f32; 3];
            for (c, o) in out.iter_mut().enumerate() {
                let mut v: Vec<f32> = frames
                    .iter()
                    .zip(cover)
                    .filter(|(_, cv)| cv[k])
                    .map(|(f, _)| srgb_to_linear(f[k][c]))
                    .collect();
                if v.is_empty() {
                    *o = frames[0][k][c];
                    continue;
                }
                let med = median(&mut v);
                let spread = (mad(&v, med) * GHOST_SPREADS).max(floor);
                let (mut num, mut den) = (0.0f32, 0.0f32);
                for x in &v {
                    let z = (x - med) / spread;
                    let wt = (-z * z).exp();
                    num += wt * x;
                    den += wt;
                }
                *o = linear_to_srgb(if den > 0.0 { num / den } else { med });
            }
            out
        })
        .collect()
}

/// A frame-wide floor for the rejection spread: the median, over a sample of
/// pixels, of how far that pixel's readings sit from their own median.
///
/// Sampled rather than exhaustive because it is a scale, not a measurement —
/// every 97th pixel of a 24 MP frame is still a quarter of a million readings,
/// and 97 is prime so the sample cannot land on a column or a Bayer phase.
fn frame_spread(frames: &[Vec<[f32; 3]>], cover: &[Vec<bool>], n: usize) -> f32 {
    let mut s: Vec<f32> = (0..n)
        .step_by(97)
        .filter_map(|k| {
            let mut v: Vec<f32> = frames
                .iter()
                .zip(cover)
                .filter(|(_, cv)| cv[k])
                .map(|(f, _)| srgb_to_linear(f[k][1]))
                .collect();
            (v.len() >= 2).then(|| {
                let m = median(&mut v);
                mad(&v, m)
            })
        })
        .collect();
    if s.is_empty() {
        return 1e-4;
    }
    (median(&mut s) * GHOST_SPREADS).max(1e-5)
}

fn median(v: &mut [f32]) -> f32 {
    let mid = v.len() / 2;
    let (_, m, _) = v.select_nth_unstable_by(mid, f32::total_cmp);
    *m
}

fn mad(v: &[f32], med: f32) -> f32 {
    let mut d: Vec<f32> = v.iter().map(|x| (x - med).abs()).collect();
    median(&mut d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stack::fixture::grain;
    use crate::stack::merge::{merge as run, MergeOptions, StackKind};


    /// A scene with fine texture, so a blur has something to destroy and a
    /// detail measure has something to find.
    fn textured(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|k| {
                let (x, y) = ((k % w) as f32, (k / w) as f32);
                let v = 0.45 + 0.2 * (x * 1.1).sin() * (y * 0.9).cos() + 0.1 * (x * 0.13).sin();
                [v, v * 0.98, v * 1.02]
            })
            .collect()
    }

    /// Hard bars on DEAD FLAT ground.
    ///
    /// Flat between the bars on purpose, and that is the whole reason the
    /// sharpness measure is blurred before it decides anything: beside a bar a
    /// defocused frame has a halo where the sharp frame has nothing at all, so
    /// a raw per-pixel Laplacian rates the DEFOCUSED frame higher there and the
    /// blend takes its halo. Over the fine texture of [`textured`] that never
    /// happens — the sharp frame has the larger Laplacian at every pixel — so
    /// the first version of the focus test could not see the blur at all.
    fn barred(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|k| {
                let on = (k % w) % 24 < 4 || (k / w) % 32 < 4;
                let v = if on { 0.72 } else { 0.32 };
                [v, v * 0.98, v * 1.02]
            })
            .collect()
    }

    fn all_covered(n: usize, frames: usize) -> Vec<Vec<bool>> {
        vec![vec![true; n]; frames]
    }

    /// The total detail energy over a half of the frame — the measure a focus
    /// stack is judged by, since "sharp" has no meaning per pixel.
    fn energy(f: &[[f32; 3]], w: usize, h: usize, left: bool) -> f32 {
        let d = detail(&grey(f), w, h);
        (0..w * h).filter(|k| (k % w < w / 2) == left).map(|k| d[k]).sum()
    }

    /// v1.5.0 Track S: a focus stack is sharp on BOTH sides of a sweep, and
    /// away from the sweep's seam it does not merely match the sharp frame's
    /// detail — it reproduces the sharp frame.
    ///
    /// Two frames, each soft where the other is sharp. The result has to carry
    /// the sharp frame's detail in both halves — and the test asserts both,
    /// because a stack that simply returned frame A would pass a test that
    /// only looked at A's good half.
    ///
    /// The bar fixture, the tightened window and the third assertion were all
    /// added after a mutation sweep showed that energy per half, at the 0.7
    /// window it started with, could not see either of the sharpness weight's
    /// own decisions. Both are small effects measured against a big total, and
    /// each needs the measure that is sensitive to it:
    ///
    /// * the CUBE is worth 5 % of the detail energy — measured 1.013 of the
    ///   sharp frame's own energy with it and 0.961 without — so the window is
    ///   0.99, which is also the honest statement of the claim: the answer
    ///   carries essentially ALL of the detail of the frame that resolved it.
    ///   (It is above 1.0 because a Laplacian pyramid's recombination adds a
    ///   little of its own at the crossfade.)
    /// * the BLUR is invisible to that total — a defocused frame's halo has
    ///   detail energy of its own, so taking the halo instead of the flat
    ///   ground the sharp frame has there costs the sum nothing. What it moves
    ///   is one pixel's value, which is what the third assertion reads.
    ///
    /// MUTATION: drop the sharpness blur — the weight becomes a stencil of
    /// edges and takes the defocused frame's halo on the flat ground beside
    /// every bar; make the weight linear instead of cubed — a fifth of the
    /// soft frame comes back and the energy falls out of its window.
    #[test]
    fn a_focus_stack_is_sharp_on_both_sides_of_the_sweep() {
        let (w, h) = (128usize, 96usize);
        let sharp = barred(w, h);
        let planes: Vec<Vec<f32>> =
            (0..3).map(|c| sharp.iter().map(|p| p[c]).collect::<Vec<f32>>()).collect();
        let soft: Vec<Vec<f32>> =
            planes.iter().map(|p| gauss_blur_plane(p, w, h, 3.0)).collect();
        // Frame A: sharp on the left. Frame B: sharp on the right.
        let mix = |left_sharp: bool| -> Vec<[f32; 3]> {
            (0..w * h)
                .map(|k| {
                    let take_sharp = (k % w < w / 2) == left_sharp;
                    let mut p = [0.0f32; 3];
                    for (c, v) in p.iter_mut().enumerate() {
                        *v = if take_sharp { sharp[k][c] } else { soft[c][k] };
                    }
                    p
                })
                .collect()
        };
        let frames = vec![mix(true), mix(false)];
        let cover = all_covered(w * h, 2);
        // The premise: each frame really is soft on its own side.
        for (i, want_left) in [(0usize, true), (1, false)] {
            let (s, b) = (energy(&frames[i], w, h, want_left), energy(&frames[i], w, h, !want_left));
            assert!(s > 3.0 * b, "premise: frame {i} must be much sharper on one side, {s} vs {b}");
        }

        let out = focus(&frames, &cover, w, h);
        for left in [true, false] {
            let got = energy(&out, w, h, left);
            let want = energy(&sharp, w, h, left);
            assert!(
                got > 0.99 * want,
                "the stack must keep the sharp frame's detail on the {} side: {got} of {want}",
                if left { "left" } else { "right" }
            );
        }
        // Away from the seam — where a blend has an honest reason to crossfade
        // — and away from the border, the answer is not merely as sharp as the
        // sharp frame. It IS the sharp frame.
        let seam = w / 2 - 12..w / 2 + 12;
        let worst = (0..w * h)
            .filter(|k| (8..w - 8).contains(&(k % w)) && (8..h - 8).contains(&(k / w)))
            .filter(|k| !seam.contains(&(k % w)))
            .map(|k| (out[k][1] - sharp[k][1]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "the stack must reproduce the frame that resolved it, worst {worst}");
    }

    /// v1.5.0 Track S: an exposure fusion shows the scene in both halves of a
    /// bracket neither frame could hold.
    ///
    /// One frame exposed for the shadows and blown in the highlights, one the
    /// reverse. The fusion has to have usable contrast in both regions, and it
    /// has to reach them without a seam.
    ///
    /// MUTATION: drop the saturation term, or the well-exposedness term, from
    /// the weight; blend at one level (the seam appears).
    #[test]
    fn an_exposure_fusion_shows_what_neither_frame_could_hold() {
        let (w, h) = (128usize, 96usize);
        // A scene four stops apart between its halves, so no single exposure
        // holds both: the left is deep shadow, the right is bright.
        let base: Vec<f32> = (0..w * h)
            .map(|k| {
                let (x, y) = ((k % w) as f32, (k / w) as f32);
                let t = 0.5 + 0.35 * (x * 0.9).sin() * (y * 0.7).cos();
                if k % w < w / 2 { 0.012 * t } else { 0.75 * t }
            })
            .collect();
        let shot = |ev: f32| -> Vec<[f32; 3]> {
            let g = 2f32.powf(ev);
            base.iter()
                .map(|lin| {
                    let v = linear_to_srgb((lin * g).min(1.0));
                    [v, v * 0.97, v * 1.03]
                })
                .collect()
        };
        let frames = vec![shot(0.0), shot(4.0)];
        let cover = all_covered(w * h, 2);
        let contrast = |f: &[[f32; 3]], left: bool| {
            let v: Vec<f32> = (0..w * h)
                .filter(|k| (k % w < w / 2) == left)
                .map(|k| f[k][1])
                .collect();
            v.iter().fold(0.0f32, |m, x| m.max(*x)) - v.iter().fold(1.0f32, |m, x| m.min(*x))
        };
        // The premise: each frame has lost one half.
        assert!(contrast(&frames[0], true) < 0.1, "premise: the base frame's shadow is crushed");
        assert!(contrast(&frames[1], false) < 0.1, "premise: the lifted frame's highlight is blown");

        let out = fuse(&frames, &cover, w, h);
        for left in [true, false] {
            assert!(
                contrast(&out, left) > 0.12,
                "the fusion must show the {} half, contrast {}",
                if left { "shadow" } else { "highlight" },
                contrast(&out, left)
            );
        }
    }

    /// v1.5.0 Track S: a noise stack lowers the noise, and a ghost does not
    /// survive it.
    ///
    /// Both halves, because each is satisfied by something wrong on its own: a
    /// plain mean lowers the noise and keeps a sixth of the ghost, and a
    /// median kills the ghost while averaging nothing.
    ///
    /// MUTATION: weight every reading alike (drop the exponential) — the ghost
    /// comes back; return the median instead of the weighted mean — the noise
    /// stops falling.
    #[test]
    fn a_noise_stack_lowers_the_noise_and_drops_a_ghost() {
        let (w, h) = (96usize, 72usize);
        let truth: Vec<f32> = (0..w * h)
            .map(|k| 0.45 + 0.15 * ((k % w) as f32 * 0.05).sin())
            .collect();
        const GRAIN: f32 = 0.02;
        let ghost: Vec<usize> = (0..w * h)
            .filter(|k| (20..40).contains(&(k % w)) && (20..40).contains(&(k / w)))
            .collect();
        let frames: Vec<Vec<[f32; 3]>> = (0..6u32)
            .map(|s| {
                (0..w * h)
                    .map(|k| {
                        let mut v = truth[k] + GRAIN * grain(s, k);
                        // One frame has somebody walking through it.
                        if s == 2 && ghost.binary_search(&k).is_ok() {
                            v = 0.95;
                        }
                        [v, v, v]
                    })
                    .collect()
            })
            .collect();
        let cover = all_covered(w * h, 6);

        let out = average(&frames, &cover, w, h);
        let rms = |f: &[[f32; 3]]| {
            let s: f32 = (0..w * h).map(|k| (f[k][1] - truth[k]).powi(2)).sum();
            (s / (w * h) as f32).sqrt()
        };
        let one = rms(&frames[0]);
        assert!(one > 0.003, "premise: a single frame must be visibly noisy, rms {one}");
        assert!(
            rms(&out) < one / 2.0,
            "six frames must halve the noise at least: {} against {one}",
            rms(&out)
        );
        let worst = ghost.iter().map(|k| (out[*k][1] - truth[*k]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.03, "the ghost must not survive the stack, worst {worst}");
    }

    /// v1.5.0 Track S: the fusion weighs COLOUR and EXPOSURE, not only detail.
    ///
    /// The bracket test above cannot see either term, and that is not a
    /// tuning detail — it is the shape of a real bracket: the frame that is
    /// better exposed somewhere is also the frame with more texture there, so
    /// detail alone reproduces the whole answer and the other two weights can
    /// be deleted with every assertion still green. Both pairs here take
    /// detail out of the argument by carrying the SAME texture in both frames,
    /// and leave exactly one term able to decide.
    ///
    /// Two separate fusions rather than two halves of one frame, because a
    /// pyramid blend's coarsest weights are smeared across tens of pixels and
    /// each half would then be partly decided by the other half's argument.
    ///
    /// MUTATION: drop the saturation term — the first pair washes out; drop
    /// the well-exposedness term — the second floats up toward the pale frame.
    #[test]
    fn the_fusion_weighs_colour_and_exposure_where_the_detail_is_equal() {
        let (w, h) = (96usize, 72usize);
        let tex = |k: usize| {
            let (x, y) = ((k % w) as f32, (k / w) as f32);
            0.07 * (x * 1.1).sin() * (y * 0.9).cos()
        };
        let cover = all_covered(w * h, 2);
        let over = |f: &[[f32; 3]], pick: fn(&[f32; 3]) -> f32| {
            let v: Vec<f32> = (0..w * h)
                .filter(|k| (8..w - 8).contains(&(k % w)) && (8..h - 8).contains(&(k / w)))
                .map(|k| pick(&f[k]))
                .collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        let same_detail = |a: &[[f32; 3]], b: &[[f32; 3]]| {
            let (da, db) = (detail(&grey(a), w, h), detail(&grey(b), w, h));
            (0..w * h).map(|k| (da[k] - db[k]).abs()).fold(0.0f32, f32::max)
        };

        // Only SATURATION can choose: one frame coloured, one grey, at the same
        // brightness and with the same texture. Well-exposedness actually
        // prefers the grey one — three channels at 0.5 beat one at 0.65 and one
        // at 0.35 — so a fusion that lost the colour term would wash this out.
        const SPREAD: f32 = 0.15;
        let coloured: Vec<[f32; 3]> = (0..w * h)
            .map(|k| {
                let v = 0.5 + tex(k);
                [v + SPREAD, v, v - SPREAD]
            })
            .collect();
        let middle: Vec<[f32; 3]> = (0..w * h)
            .map(|k| {
                let v = 0.5 + tex(k);
                [v, v, v]
            })
            .collect();
        let gap = same_detail(&coloured, &middle);
        assert!(gap < 1e-5, "premise: the coloured pair must carry one texture, gap {gap}");
        let out = fuse(&[coloured.clone(), middle.clone()], &cover, w, h);
        let (got, want) = (over(&out, saturation), over(&coloured, saturation));
        assert!(
            got > 0.7 * want,
            "the colour must survive where only saturation can choose: {got} of {want}"
        );

        // Only WELL-EXPOSEDNESS can choose: both frames grey, so saturation has
        // nothing to say, one sitting in the middle and one nearly white.
        let pale: Vec<[f32; 3]> = (0..w * h)
            .map(|k| {
                let v = 0.88 + tex(k);
                [v, v, v]
            })
            .collect();
        let gap = same_detail(&pale, &middle);
        assert!(gap < 1e-5, "premise: the pale pair must carry one texture, gap {gap}");
        let out = fuse(&[middle, pale], &cover, w, h);
        let level = over(&out, |p| p[1]);
        assert!(
            level < 0.6,
            "the middle exposure must win where only well-exposedness can choose: {level}"
        );
    }

    /// v1.5.0 Track S: what the alignment could not cover contributes nothing.
    ///
    /// A warped frame keeps its own pixels in the border wedge — see
    /// `align::warp_rgb` — and those pixels describe a different part of the
    /// scene. Every merge here multiplies its weights by the coverage mask, so
    /// this asserts the one thing that matters: a frame marked uncovered
    /// everywhere changes nothing, however good its pixels look.
    ///
    /// MUTATION: drop the `if !c[k]` guard from any of the three weightings.
    #[test]
    fn a_frame_the_warp_could_not_cover_is_not_averaged_in() {
        let (w, h) = (64usize, 48usize);
        let good = textured(w, h);
        // Textured, colourful and well exposed on purpose. A frame that scores
        // badly on every weight — the flat `[0.9, 0.1, 0.1]` this used to hold
        // — is ignored with or without the mask, so it proved nothing. It is
        // the frame that WOULD win the argument on its merits that shows the
        // mask is consulted at all, and that is also the honest picture of the
        // case: the border wedge of a warped frame is a perfectly good
        // photograph of the wrong part of the scene.
        let elsewhere: Vec<[f32; 3]> = (0..w * h)
            .map(|k| {
                let (x, y) = ((k % w) as f32, (k / w) as f32);
                let v = 0.5 + 0.17 * (x * 0.7).cos() * (y * 1.3).sin();
                [v * 1.2, v * 0.85, v * 0.6]
            })
            .collect();
        let frames = vec![good.clone(), elsewhere];
        let cover = vec![vec![true; w * h], vec![false; w * h]];
        for (name, got) in [
            ("fuse", fuse(&frames, &cover, w, h)),
            ("focus", focus(&frames, &cover, w, h)),
            ("noise", average(&frames, &cover, w, h)),
        ] {
            let worst =
                (0..w * h).map(|k| (got[k][0] - good[k][0]).abs()).fold(0.0f32, f32::max);
            assert!(worst < 0.01, "{name} let an uncovered frame in, worst {worst}");
        }
    }

    /// v1.5.0 Track S: every kind survives the front door, and the front door
    /// refuses what it cannot merge.
    ///
    /// The four kinds are reached through one `match`, and a kind added to the
    /// enum without an arm is a compile error — but a kind wired to the WRONG
    /// arm is not, and neither is a refusal that forgets to refuse. So each
    /// kind is run end to end, and each way of asking for the impossible is
    /// asked.
    ///
    /// MUTATION: return `Ok` for a single frame; drop the size check.
    #[test]
    fn every_kind_merges_and_the_impossible_ones_are_refused() {
        let (w, h) = (48usize, 32usize);
        let frames = vec![textured(w, h), textured(w, h)];
        for kind in StackKind::ALL {
            let out = run(&frames, w, h, &MergeOptions { kind, align: None })
                .unwrap_or_else(|e| panic!("{} must merge two frames: {e}", kind.store_str()));
            assert_eq!(out.pixels.len(), w * h, "{} changed the frame size", kind.store_str());
            assert_eq!(out.exposures.len(), 2, "{} lost a frame's exposure", kind.store_str());
        }
        let one = vec![textured(w, h)];
        assert!(run(&one, w, h, &MergeOptions::new(StackKind::Hdr)).is_err(), "one frame is not a stack");
        let ragged = vec![textured(w, h), textured(w, h - 1)];
        assert!(
            run(&ragged, w, h, &MergeOptions::new(StackKind::Hdr)).is_err(),
            "frames of different sizes are not a stack"
        );
        for kind in StackKind::ALL {
            assert_eq!(
                StackKind::from_store_str(kind.store_str()),
                Some(kind),
                "the spelling table must round-trip {kind:?}"
            );
        }
    }
}
