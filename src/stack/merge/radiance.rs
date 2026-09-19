//! The HDR merge: a bracket into one measurement of light, and a record of how
//! much light that turned out to be.
//!
//! Two things come out of here and the second is the reason the first is
//! usable. The estimate is a RADIANCE in the reference frame's own exposure,
//! so 1.0 is still the reference's white and everything the darker frames
//! recovered sits above it — which is exactly the range a 16-bit master
//! cannot hold. So the estimate is folded back into [0, 1] by a shoulder, and
//! how far it was folded is returned as `headroom_ev` for the recipe to carry
//! in `hdr_max_ev`. The pixels stay a photograph every existing path can
//! open, and the stops are not lost, they are written down.

use super::trustworthy;
use crate::render::{linear_to_srgb, srgb_to_linear};

/// Merge a bracket, returning the frame and the stops it recovered.
pub(super) fn merge(
    frames: &[Vec<[f32; 3]>],
    cover: &[Vec<bool>],
    exposures: &[f32],
    w: usize,
    h: usize,
) -> (Vec<[f32; 3]>, f32) {
    let radiance = estimate(frames, cover, exposures, w * h);
    let headroom = headroom_of(&radiance);
    let curve = Shoulder::reaching(headroom);
    let pixels = radiance
        .iter()
        .map(|r| {
            let t = curve.apply(*r);
            let mut o = [0.0f32; 3];
            for (c, v) in o.iter_mut().enumerate() {
                *v = linear_to_srgb(t[c].clamp(0.0, 1.0));
            }
            o
        })
        .collect();
    (pixels, headroom)
}

/// The radiance every frame agrees on, in the REFERENCE frame's own exposure.
///
/// Each frame's samples are divided by its own exposure to put them on the
/// reference's scale, and weighted BY that same exposure because a frame that
/// collected twice the light carries twice the confidence per sample — which
/// is Debevec's weighting, and it is why a bracket is worth shooting rather
/// than just pushing one frame.
fn estimate(
    frames: &[Vec<[f32; 3]>],
    cover: &[Vec<bool>],
    exposures: &[f32],
    n: usize,
) -> Vec<[f32; 3]> {
    let mut num = vec![[0.0f32; 3]; n];
    let mut den = vec![0.0f32; n];
    for ((f, cov), ev) in frames.iter().zip(cover).zip(exposures) {
        let (scale, conf) = (2f32.powf(-ev), 2f32.powf(*ev));
        for k in 0..n {
            if !cov[k] {
                continue;
            }
            // ONE weight for the whole pixel, taken as the least trustworthy
            // of its three channels — never one weight per channel. A
            // per-channel weight withdraws a single channel of a pixel on its
            // own, so the pixel is rebuilt from a different mixture of frames
            // in red than in green, and its HUE moves. That is how a merged
            // sunset goes magenta right where it clips, and it is invisible in
            // any per-channel test.
            let t = f[k].iter().copied().map(trustworthy).fold(f32::INFINITY, f32::min);
            if t <= 0.0 {
                continue;
            }
            let wt = t * conf;
            den[k] += wt;
            for (c, acc) in num[k].iter_mut().enumerate() {
                *acc += wt * srgb_to_linear(f[k][c]) * scale;
            }
        }
    }
    (0..n)
        .map(|k| {
            if den[k] > 0.0 {
                let mut o = num[k];
                for v in o.iter_mut() {
                    *v /= den[k];
                }
                return o;
            }
            // No frame had anything to say here: the pixel is clipped in all
            // of them, or crushed in all of them. Falling back to an average
            // would be averaging two kinds of nothing, so take the ONE frame
            // that is least far from the middle — the darkest exposure for a
            // clipped pixel, the brightest for a crushed one — and take it
            // whole.
            let pick = (0..frames.len())
                .filter(|i| cover[*i][k])
                .min_by(|&i, &j| middling(&frames[i][k], exposures[i])
                    .total_cmp(&middling(&frames[j][k], exposures[j])))
                .unwrap_or(0);
            let s = 2f32.powf(-exposures[pick]);
            let mut o = [0.0f32; 3];
            for (c, v) in o.iter_mut().enumerate() {
                *v = srgb_to_linear(frames[pick][k][c]) * s;
            }
            o
        })
        .collect()
}

/// How far this sample sits from a mid-grey reading, once its own exposure is
/// divided out — the tie-break for a pixel no frame could measure.
fn middling(p: &[f32; 3], ev: f32) -> f32 {
    const MID: f32 = 0.18;
    (crate::stack::luma(p) * 2f32.powf(-ev) - MID).abs()
}

/// How many stops of highlight the merge recovered above the reference frame's
/// own white.
///
/// The 99.9th percentile rather than the maximum. One hot pixel, or one
/// specular glint off a chrome bumper, is a legitimate radiance and a terrible
/// headroom: taking the maximum would let it push the entire picture down the
/// shoulder to make room for a highlight nobody is going to look into. The
/// clamp at 8 EV is the same one `EditRecipe::hdr_max_ev` applies, so the
/// number handed over is one the recipe will keep.
fn headroom_of(radiance: &[[f32; 3]]) -> f32 {
    if radiance.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f32> = radiance.iter().map(|p| p[0].max(p[1]).max(p[2])).collect();
    let at = ((v.len() - 1) as f32 * 0.999) as usize;
    let (_, top, _) = v.select_nth_unstable_by(at, f32::total_cmp);
    top.max(1.0).log2().clamp(0.0, 8.0)
}

/// The curve that folds recovered highlights back into a frame which still has
/// to fit in [0, 1].
///
/// **In STOPS, not in linear light, and applied to the pixel's brightest
/// channel rather than to each channel on its own.** Both of those were got
/// wrong on the first attempt and both are visible in the photograph, so both
/// are worth keeping written down:
///
/// * a hyperbola in LINEAR light with its knee at 0.8 leaves 0.2 of the output
///   range for everything above white — which is 0.09 of the sRGB range. A
///   2.6 EV recovery came back spanning sRGB 0.990 to 1.000: the window the
///   merge had just recovered was still, to an eye and to a measurement, flat
///   white. Vision is logarithmic and a rolloff has to be too;
/// * applied per channel, the curve compresses a clipping highlight's red more
///   than its blue, because red sits further up the curve, and the pixel's
///   ratios move. Measured on this module's own warm highlight: linear
///   1 : 0.55 : 0.25 came back 1 : 0.975 : 0.750, which is to say white. So
///   the curve reads the BRIGHTEST channel and scales all three by the same
///   factor, which is the one way to change a pixel's brightness without
///   touching its colour.
///
/// Identity below the knee, a hyperbola above it, joined so the slope does not
/// jump, and scaled so the top of the recovered range lands exactly on white.
struct Shoulder {
    /// Stops below white at which the rolloff starts.
    knee: f32,
    /// The hyperbola's scale; `0.0` means there was no headroom and the curve
    /// is the identity.
    a: f32,
}

impl Shoulder {
    /// Two stops below white, which on an sRGB output is 0.54 — so a picture's
    /// midtones and all of its shadows pass through exactly as the reference
    /// frame had them, and the top two stops are what absorb the recovery.
    /// That is where a film shoulder puts it and where an eye goes looking for
    /// it; a knee much lower would produce the flat grey an HDR merge is
    /// infamous for, and would do it to pixels that never needed the help.
    const KNEE_EV: f32 = 2.0;

    /// The shoulder that maps `headroom_ev` stops above white onto white.
    ///
    /// With no headroom there is nothing to fold and the curve is the identity
    /// — a bracket of one exposure must come out of an HDR merge looking like
    /// the frame it went in as.
    fn reaching(headroom_ev: f32) -> Shoulder {
        let k = Self::KNEE_EV;
        // Solving `−k + a·(H+k)/(a + H + k) = 0` for `a`, with the slope at
        // the knee being `a²/a² = 1` so the join is smooth.
        let a = if headroom_ev > 1e-4 { k * (headroom_ev + k) / headroom_ev } else { 0.0 };
        Shoulder { knee: k, a }
    }

    /// Where a brightness in stops-below-white comes out, in the same units.
    fn stops(&self, x: f32) -> f32 {
        if self.a <= 0.0 || x <= -self.knee {
            return x;
        }
        let u = x + self.knee;
        -self.knee + self.a * u / (self.a + u)
    }

    /// The whole pixel, scaled by what the rolloff does to its brightest
    /// channel.
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let m = p[0].max(p[1]).max(p[2]);
        if self.a <= 0.0 || m <= 0.0 {
            return p;
        }
        let x = m.log2();
        if x <= -self.knee {
            return p;
        }
        let s = 2f32.powf(self.stops(x)) / m;
        [p[0] * s, p[1] * s, p[2] * s]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stack::fixture::grain;
    use crate::stack::merge::{merge as run, MergeOptions, StackKind};

    /// A frame of a scene with a window in it: a mid-toned room, and a bright
    /// rectangle that a longer exposure cannot hold.
    ///
    /// `ev` is the exposure in stops relative to the base, applied in LINEAR
    /// light and clipped there, which is what a sensor does.
    fn window(w: usize, h: usize, ev: f32) -> Vec<[f32; 3]> {
        let k = 2f32.powf(ev);
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                // The room: a gentle gradient around a quarter tone. The
                // window: eight stops brighter, with structure of its own that
                // only a short exposure can see.
                let room = 0.06 + 0.04 * (x as f32 / w as f32) + 0.02 * (y as f32 / h as f32);
                let lin = if (w / 2..w).contains(&x) && (h / 4..3 * h / 4).contains(&y) {
                    2.0 + 10.0 * ((x - w / 2) as f32 / (w / 2) as f32)
                } else {
                    room
                };
                let v = linear_to_srgb((lin * k).min(1.0));
                [v, v * 0.98, v * 1.03]
            })
            .collect()
    }

    /// v1.5.0 Track S: the bracket recovers a highlight the reference frame
    /// had already clipped, and says how far it reached.
    ///
    /// The window is flat white in the reference frame — every pixel of it
    /// reads 1.0, so the reference has no information about it at all. After
    /// the merge it has to have STRUCTURE again, which is the only claim worth
    /// making about an HDR merge, and `headroom_ev` has to say so.
    ///
    /// MUTATION: weight every frame alike (drop `conf`); drop the division by
    /// each frame's own exposure; return 0.0 from `headroom_of`.
    #[test]
    fn a_bracket_puts_structure_back_into_a_window_the_long_frame_clipped() {
        let (w, h) = (96usize, 72usize);
        let frames = vec![window(w, h, 0.0), window(w, h, -3.0), window(w, h, -6.0)];
        // The premise: the reference really has lost the window.
        let win: Vec<usize> = (0..w * h)
            .filter(|i| (w / 2 + 4..w - 4).contains(&(i % w)) && (h / 3..2 * h / 3).contains(&(i / w)))
            .collect();
        let spread = |f: &[[f32; 3]]| {
            let (lo, hi) = win.iter().fold((1.0f32, 0.0f32), |(lo, hi), i| {
                (lo.min(f[*i][1]), hi.max(f[*i][1]))
            });
            hi - lo
        };
        assert!(
            spread(&frames[0]) < 0.001,
            "premise: the reference frame's window must be flat clipped, spread {}",
            spread(&frames[0])
        );

        let out = run(&frames, w, h, &MergeOptions { kind: StackKind::Hdr, align: None })
            .expect("three frames of one size merge");
        assert!(
            spread(&out.pixels) > 0.05,
            "the merge must put the window's structure back, spread {}",
            spread(&out.pixels)
        );
        assert!(
            out.headroom_ev > 1.5,
            "…and must say how far above white it reached, got {} EV",
            out.headroom_ev
        );
        // The room is below the knee, so it comes through as it was. This is
        // the half that stops "recovered the window" being satisfied by a
        // merge that simply darkened everything.
        let room: Vec<usize> = (0..w * h).filter(|i| i % w < w / 2 - 4).collect();
        let worst = room
            .iter()
            .map(|i| (out.pixels[*i][1] - frames[0][*i][1]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "the room must survive the merge unchanged, worst {worst}");
    }

    /// v1.5.0 Track S: a pixel is withdrawn as a PIXEL, never a channel at a
    /// time, so a clipping highlight keeps its colour.
    ///
    /// A saturated highlight clips in one channel before the others. If each
    /// channel chose its own frames, the recovered pixel would be built from a
    /// different mixture in red than in blue, and its ratio — its hue — would
    /// move. Here the window is strongly warm, and what is asserted is that
    /// the merged hue matches the frame that could actually see it.
    ///
    /// The ratios are 1 : 0.30 : 0.15 and the highlight sits at 1.5, so in the
    /// BRIGHT frame red is clipped while green and blue are still perfectly
    /// good readings. That combination is the whole test: a weight taken from
    /// any one channel, or from each channel separately, keeps trusting that
    /// frame and pulls a clipped red into the answer. Taking the minimum over
    /// the three withdraws the pixel whole. The first version of this used
    /// 1 : 0.55 : 0.25 at linear 3.0, where every channel clipped together and
    /// a green-channel weight behaved exactly like the minimum — the mutation
    /// stayed green and the test proved nothing.
    ///
    /// MUTATION: weight on one channel (`trustworthy(f[k][1])`); weight per
    /// channel inside the channel loop.
    #[test]
    fn a_clipping_highlight_is_withdrawn_as_a_pixel_so_its_colour_survives() {
        let (w, h) = (64usize, 48usize);
        let tint = |lin: f32, k: f32| {
            let f = |m: f32| linear_to_srgb((lin * m * k).min(1.0));
            [f(1.0), f(0.30), f(0.15)]
        };
        let frames: Vec<Vec<[f32; 3]>> = [0.0f32, -2.0, -4.0]
            .iter()
            .map(|ev| {
                let k = 2f32.powf(*ev);
                (0..w * h)
                    .map(|i| {
                        let lin = if i % w >= w / 2 { 1.5 } else { 0.10 };
                        tint(lin, k)
                    })
                    .collect()
            })
            .collect();
        // The premise this rests on: in the brightest frame, red really has
        // clipped and green really has not. Red is asserted just under 1.0
        // rather than at it because `linear_to_srgb(1.0)` is 0.99999994 in f32
        // — 1.055 and 0.055 do not cancel exactly — and a clipped channel that
        // reads a sixteen-millionth low is still a clipped channel.
        let i = (h / 2) * w + w * 3 / 4;
        assert!(frames[0][i][0] > 0.99, "premise: red must be clipped in the bright frame");
        assert!(frames[0][i][1] < 0.95, "premise: green must NOT be clipped in it");

        let out = run(&frames, w, h, &MergeOptions { kind: StackKind::Hdr, align: None })
            .expect("three frames of one size merge");
        let lin: Vec<f32> = out.pixels[i].iter().map(|v| srgb_to_linear(*v)).collect();
        assert!(lin[0] > 1e-4, "premise: the highlight must not have merged to black");
        let (gr, br) = (lin[1] / lin[0], lin[2] / lin[0]);
        assert!(
            (gr - 0.30).abs() < 0.04 && (br - 0.15).abs() < 0.04,
            "the highlight's colour must survive the merge: got {gr:.3} : {br:.3}, want 0.30 : 0.15"
        );
    }

    /// v1.5.0 Track S: where two frames can both see a thing, the one that
    /// collected MORE LIGHT carries more of the answer.
    ///
    /// Debevec's weighting, and the only reason a bracket beats pushing one
    /// frame: in the midtones every frame of a bracket is a valid reading, and
    /// they are not equally good readings. The darker frame's is the same
    /// scene measured with a quarter of the photons, so its noise arrives
    /// magnified by four when its exposure is divided back out. Weighting the
    /// frames alike would let that noise into a region the bright frame had
    /// already recorded cleanly.
    ///
    /// The scene is flat on purpose — the claim is about NOISE, and structure
    /// would only make it harder to measure. The grain is added in LINEAR
    /// light, at the same size in every frame, and that is the whole fixture:
    /// it is a read-noise-limited sensor, which is the case Debevec's
    /// weighting is derived for. The first version of this test added the
    /// grain to the ENCODED value instead, where sRGB's curve is four times
    /// shallower at the dark frame's level than at the bright one's — the dark
    /// frame came out only 1.9× noisier rather than 4×, the two weightings
    /// landed 20 % apart, and the mutation stayed green.
    ///
    /// MUTATION: drop `conf` (weight every frame alike); square it.
    #[test]
    fn a_frame_that_collected_more_light_carries_more_of_the_answer() {
        let (w, h) = (128usize, 96usize);
        const TRUTH: f32 = 0.10; // linear
        const READ: f32 = 0.002; // linear sensor units, the same in every frame
        let shot = |seed: u32, ev: f32| -> Vec<[f32; 3]> {
            let g = 2f32.powf(ev);
            (0..w * h)
                .map(|k| {
                    let v = linear_to_srgb((TRUTH * g + READ * grain(seed, k)).max(0.0));
                    [v, v, v]
                })
                .collect()
        };
        // Bright first (the reference), then two stops down.
        let frames = vec![shot(1, 0.0), shot(2, -2.0)];
        let out = run(&frames, w, h, &MergeOptions { kind: StackKind::Hdr, align: None })
            .expect("two frames of one size merge");
        // In LINEAR light, where the claim lives: each frame's own reading of
        // the scene, with its exposure divided back out exactly as the merge
        // divides it.
        let rms = |f: &[[f32; 3]], scale: f32| {
            let s: f32 =
                f.iter().map(|p| (srgb_to_linear(p[1]) * scale - TRUTH).powi(2)).sum::<f32>();
            (s / f.len() as f32).sqrt()
        };
        let (bright, dark) = (rms(&frames[0], 1.0), rms(&frames[1], 4.0));
        assert!(
            dark > 3.0 * bright,
            "premise: dividing the dark frame's exposure out must magnify its error, {dark} vs {bright}"
        );
        // Weighting the two alike would land at 2.1× the bright frame's own
        // error; weighting them by the light they collected lands at 1.1×.
        let got = rms(&out.pixels, 1.0);
        assert!(
            got < 1.4 * bright,
            "the brighter frame must carry the answer: merged {got}, the bright frame alone {bright}"
        );
    }

    /// v1.5.0 Track S: the shoulder leaves the picture alone below its knee,
    /// lands exactly on white at the top of the recovered range, and spends a
    /// real part of the output getting there.
    ///
    /// Three claims, because each pair alone permits something useless:
    /// "lands on white" is satisfied by scaling everything down by the
    /// headroom, "leaves the midtones alone" by hard clipping, and both
    /// together by the linear-light curve this replaced — which met them and
    /// still delivered the entire recovery inside the top 1 % of the output.
    ///
    /// MUTATION: return `x` unconditionally from `stops`; drop the `+ k` from
    /// the numerator of `a`; move the knee to 0.
    #[test]
    fn the_shoulder_is_identity_below_the_knee_and_reaches_white_at_the_top() {
        let grey = |v: f32| Shoulder::reaching(0.0).apply([v; 3]);
        assert_eq!(grey(0.95), [0.95; 3], "a single-exposure bracket must pass straight through");
        for ev in [1.0f32, 2.0, 3.5] {
            let s = Shoulder::reaching(ev);
            let at = |v: f32| s.apply([v; 3])[0];
            // Below the knee — two stops under white, so 0.25 linear.
            for r in [0.0f32, 0.01, 0.08, 0.2, 0.249] {
                assert_eq!(at(r), r, "below the knee nothing may move, at {r} with {ev} EV");
            }
            let top = 2f32.powf(ev);
            assert!(
                (at(top) - 1.0).abs() < 1e-4,
                "the top of the range must land on white: {} at {ev} EV",
                at(top)
            );
            // Monotone, or the shoulder folds two radiances onto one value and
            // invents a false edge inside a highlight.
            let mut last = 0.0f32;
            for i in 0..=200 {
                let v = at(top * i as f32 / 200.0);
                assert!(v >= last - 1e-6, "the shoulder must be monotone, {v} after {last}");
                last = v;
            }
            // And the recovery is SPENT, not hidden: white itself has to come
            // down far enough to leave the recovered stops somewhere to go.
            let white = linear_to_srgb(at(1.0));
            assert!(
                white < 0.97,
                "white must make room for the {ev} EV above it, got sRGB {white}"
            );
        }
    }

    /// v1.5.0 Track S: one hot pixel does not set the headroom for the whole
    /// picture.
    ///
    /// A specular glint is a real radiance and a terrible headroom. Taking the
    /// maximum would push the entire frame down a shoulder built to make room
    /// for a highlight nobody will look into.
    ///
    /// MUTATION: use the maximum instead of the 99.9th percentile.
    #[test]
    fn one_hot_pixel_does_not_set_the_headroom() {
        let n = 10_000;
        let mut r = vec![[1.9f32; 3]; n];
        r[n / 2] = [200.0; 3];
        let got = headroom_of(&r);
        assert!(
            (got - 1.9f32.log2()).abs() < 0.05,
            "the bulk must set the headroom, not the glint: got {got}, want {}",
            1.9f32.log2()
        );
    }
}
