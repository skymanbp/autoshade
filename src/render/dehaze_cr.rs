//! Camera Raw's Dehaze on a camera-profile start (v1.17.0): a dark-channel
//! scattering inversion, spatial, measured against Camera Raw's own renders.
//!
//! Each channel `I` of the light under Adobe's curve (the profile stage's
//! reference primaries, sRGB-encoded — the domain the slider tables read)
//! goes to `J = (I − A) / t + A`, with
//! - the airlight `A`, per channel, the MEAN of the pixels in the top 0.1 %
//!   of the dark channel (each pixel's smallest channel, min-filtered over a
//!   square of radius [`PATCH`] × the short edge). He et al. take the
//!   brightest of those pixels instead; on a night frame that picks a star or
//!   a lamp, measured: the mean gives the airlight the frame's own windows
//!   regress to (≈ 0.02 linear on the star frame), the brightest 0.45
//!   encoded — and the frame does not blow up although its dark channel's top
//!   is flat (0.062 … 0.071 encoded);
//! - the transmission `t = 1 − ω · S`, `S` the dark channel of `I / A`
//!   (min-filtered at [`PATCH`]) under a box of radius [`SPREAD`] × the short
//!   edge; positive `t` floored at [`T_MIN`].
//!
//! Where it sits, measured on p39 with one other control at a time (the model
//! run on Camera Raw's render with that control, scored against Camera Raw's
//! render with that control and Dehaze +40; against the Dehaze move taken on
//! the flat render and added on): after Shadows +60 (1.78 vs 2.24 mean ΔE),
//! after Highlights −60 (1.37 vs 1.84), after Exposure ±1 (1.43 / 1.12 vs
//! 1.96 / 1.99), and before Contrast +50 (adding the moves 1.24, reading the
//! contrasted frame 1.40) — so after Highlights/Shadows and Exposure, before
//! Contrast, Whites, Blacks and the curves. In which light, measured on p39
//! +80 with the rest of the model fixed: under the curve and encoded 2.00,
//! after the curve encoded 2.31, under the curve linear 2.87, after it linear
//! 3.49.
//!
//! The strength `ω` per slider value, fitted per frame and value on the flat
//! renders of six frames (+20 … +100, −30 … −100; `curves` lane, gl/dcpw.out):
//! positive `ω = OMEGA_POS · s`; negative `ω = s · (OMEGA_NEG + OMEGA_NEG_Q · |s|)`,
//! the haze added back growing faster than the slider. Per frame the
//! positive `ω / s` spreads 0.60 … 1.13 about this law (the star frame the
//! lowest): a frame quantity the law does not yet carry.

use super::profile::UnderTone;
use super::wb::{decode_wide, encode_wide, transfer_luts};
use rayon::prelude::*;

/// Radius of the dark channel's min filter, as a share of the short edge.
const PATCH: f32 = 0.0146;
/// Radius of the box the transmission is spread under, as a share of the
/// short edge.
const SPREAD: f32 = 0.125;
/// The long edge the airlight and the transmission are read at: the scale of
/// the measurement (Camera Raw's renders boxed to 1536 wide).
const MEASURED_EDGE: usize = 1536;
/// Share of the pixels whose dark channel names the airlight.
const TOP: f32 = 0.001;
/// Positive Dehaze: `ω` per unit slider.
const OMEGA_POS: f32 = 0.896;
/// Negative Dehaze: `ω = s · (OMEGA_NEG + OMEGA_NEG_Q · |s|)`.
const OMEGA_NEG: f32 = 1.698;
const OMEGA_NEG_Q: f32 = 2.802;
/// Floor of a positive Dehaze's transmission.
const T_MIN: f32 = 0.05;

/// The strength `ω` for a slider value `s` in −1..1.
fn omega(s: f32) -> f32 {
    if s >= 0.0 {
        OMEGA_POS * s
    } else {
        s * (OMEGA_NEG + OMEGA_NEG_Q * s.abs())
    }
}

/// Camera Raw's Dehaze at `amount` (−100..100) on the encoded working frame
/// `data` (`w` × `h`), whose profile stage `under` names (module head).
pub(super) fn apply_camera_raw_dehaze(data: &mut [[f32; 3]], w: usize, h: usize, amount: f32, under: &UnderTone) {
    let s = (amount / 100.0).clamp(-1.0, 1.0);
    if s == 0.0 || data.is_empty() {
        return;
    }
    let (dec, enc) = transfer_luts();
    let light: Vec<[f32; 3]> = data
        .par_iter()
        .map(|px| under.undo_curve(px.map(|v| decode_wide(dec, v))).map(|c| encode_wide(enc, c.clamp(0.0, 1.0))))
        .collect();
    // The airlight and the transmission are read on the frame boxed down to
    // about MEASURED_EDGE on its long edge, the scale they were measured at:
    // at the sensor's own size the min filter finds a noise pixel near 0 in
    // every window of a night sky and the transmission goes to 1 (measured on
    // the star frame at 9504 x 6336: the dark channel's mean 1e-10).
    let f = ((w.max(h) as f32 / MEASURED_EDGE as f32).round() as usize).max(1);
    let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
    let small = box_down(&light, w, h, f);
    let short = sw.min(sh) as f32;
    let patch = ((PATCH * short).round() as usize).max(1);
    let spread = ((SPREAD * short).round() as usize).max(1);
    let a = airlight(&small, sw, sh, patch);
    let ratio: Vec<f32> = small.par_iter().map(|p| (p[0] / a[0]).min(p[1] / a[1]).min(p[2] / a[2])).collect();
    let dark = upsample(&box_mean(&min_filter(&ratio, sw, sh, patch), sw, sh, spread), sw, sh, f, w, h);
    let om = omega(s);
    data.par_iter_mut().zip(light.par_iter().zip(dark.par_iter())).for_each(|(px, (p, &d))| {
        let t = 1.0 - om * d;
        let t = if s > 0.0 { t.max(T_MIN) } else { t };
        let j = [0, 1, 2].map(|c| ((p[c] - a[c]) / t + a[c]).clamp(0.0, 1.0));
        *px = under.redo_curve(j.map(|c| decode_wide(dec, c))).map(|v| encode_wide(enc, v.max(0.0)));
    });
}

/// `src` (`w` x `h`) boxed down by `f`: each output pixel the mean of its
/// `f` x `f` block (the last row and column of blocks cut by the frame edge).
fn box_down(src: &[[f32; 3]], w: usize, h: usize, f: usize) -> Vec<[f32; 3]> {
    let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
    (0..sw * sh)
        .into_par_iter()
        .map(|i| {
            let (bx, by) = (i % sw, i / sw);
            let (x0, y0) = (bx * f, by * f);
            let (x1, y1) = ((x0 + f).min(w), (y0 + f).min(h));
            let mut acc = [0.0f32; 3];
            for y in y0..y1 {
                for p in &src[y * w + x0..y * w + x1] {
                    for c in 0..3 {
                        acc[c] += p[c];
                    }
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as f32;
            acc.map(|v| v / n)
        })
        .collect()
}

/// A plane read on the `f`-boxed grid (`sw` x `sh`) back at full size
/// (`w` x `h`), bilinear between block centres, edges clamped.
fn upsample(small: &[f32], sw: usize, sh: usize, f: usize, w: usize, h: usize) -> Vec<f32> {
    if f == 1 {
        return small.to_vec();
    }
    let at = |p: usize, n: usize| -> (usize, usize, f32) {
        let u = ((p as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let i = (u.floor() as usize).min(n - 1);
        (i, (i + 1).min(n - 1), u - i as f32)
    };
    let xs: Vec<(usize, usize, f32)> = (0..w).map(|x| at(x, sw)).collect();
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (y0, y1, fy) = at(y, sh);
        for (o, &(x0, x1, fx)) in row.iter_mut().zip(&xs) {
            let top = small[y0 * sw + x0] + (small[y0 * sw + x1] - small[y0 * sw + x0]) * fx;
            let bot = small[y1 * sw + x0] + (small[y1 * sw + x1] - small[y1 * sw + x0]) * fx;
            *o = top + (bot - top) * fy;
        }
    });
    out
}

/// The airlight: per channel the mean of the pixels whose dark channel
/// (smallest channel, min-filtered at `patch`) is in the top [`TOP`].
fn airlight(light: &[[f32; 3]], w: usize, h: usize, patch: usize) -> [f32; 3] {
    let dark = min_filter(&light.iter().map(|p| p[0].min(p[1]).min(p[2])).collect::<Vec<_>>(), w, h, patch);
    let n = ((dark.len() as f32 * TOP) as usize).max(1);
    let mut sorted = dark.clone();
    let k = sorted.len() - n;
    let (_, &mut cut, _) = sorted.select_nth_unstable_by(k, f32::total_cmp);
    let mut sum = [0.0f64; 3];
    let mut count = 0usize;
    for (p, &d) in light.iter().zip(&dark) {
        if d >= cut {
            for c in 0..3 {
                sum[c] += f64::from(p[c]);
            }
            count += 1;
        }
    }
    // The cut is the k-th value, so at least `n` pixels pass it.
    sum.map(|v| ((v / count as f64) as f32).max(1e-4))
}

/// Mean over the part of a square of radius `r` inside the frame (the
/// measurement's box: no edge pixel repeated), separable.
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let line = |v: &[f32]| -> Vec<f32> {
        let n = v.len();
        let mut acc = vec![0.0f64; n + 1];
        for (i, x) in v.iter().enumerate() {
            acc[i + 1] = acc[i] + f64::from(*x);
        }
        (0..n).map(|i| {
            let (lo, hi) = (i.saturating_sub(r), (i + r + 1).min(n));
            ((acc[hi] - acc[lo]) / (hi - lo) as f64) as f32
        }).collect()
    };
    let rows: Vec<f32> = src.par_chunks(w).flat_map_iter(&line).collect();
    let t: Vec<Vec<f32>> = (0..w).into_par_iter().map(|x| line(&(0..h).map(|y| rows[y * w + x]).collect::<Vec<_>>())).collect();
    let mut out = vec![0.0f32; w * h];
    for (x, col) in t.iter().enumerate() {
        for (y, &v) in col.iter().enumerate() {
            out[y * w + x] = v;
        }
    }
    out
}

/// Minimum over a square of radius `r`, edges clamped: separable, each pass
/// by van Herk / Gil–Werman (two running minima per block of `2r + 1`).
fn min_filter(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let rows: Vec<f32> = src.par_chunks(w).flat_map_iter(|row| min_1d(row, r)).collect();
    let mut cols = vec![0.0f32; w * h];
    let t: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| min_1d(&(0..h).map(|y| rows[y * w + x]).collect::<Vec<_>>(), r))
        .collect();
    for (x, col) in t.iter().enumerate() {
        for (y, &v) in col.iter().enumerate() {
            cols[y * w + x] = v;
        }
    }
    cols
}

/// One line's running minimum over `[i − r, i + r]`, the ends clamped.
fn min_1d(line: &[f32], r: usize) -> Vec<f32> {
    let n = line.len();
    let k = 2 * r + 1;
    // Pad by r on both sides with the end values (clamped edges).
    let padded: Vec<f32> = (0..n + 2 * r).map(|i| line[i.saturating_sub(r).min(n - 1)]).collect();
    let m = padded.len();
    let mut fwd = vec![0.0f32; m];
    let mut bwd = vec![0.0f32; m];
    for i in 0..m {
        fwd[i] = if i % k == 0 { padded[i] } else { fwd[i - 1].min(padded[i]) };
    }
    for i in (0..m).rev() {
        bwd[i] = if i == m - 1 || (i + 1) % k == 0 { padded[i] } else { bwd[i + 1].min(padded[i]) };
    }
    (0..n).map(|i| bwd[i].min(fwd[i + k - 1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The running minimum is the brute-force minimum over the clamped
    /// window, at window sizes smaller and larger than the line.
    ///
    /// MUTATION: take `fwd[i + k − 2]`, or pad with zeros.
    #[test]
    fn the_running_minimum_is_the_window_minimum() {
        let line: Vec<f32> = (0..37).map(|i| ((i * 7919) % 31) as f32).collect();
        for r in [0usize, 1, 3, 8, 40] {
            let got = min_1d(&line, r);
            for (i, g) in got.iter().enumerate() {
                let lo = i.saturating_sub(r);
                let hi = (i + r).min(line.len() - 1);
                let want = line[lo..=hi].iter().copied().fold(f32::INFINITY, f32::min);
                assert_eq!(*g, want, "r {r} at {i}");
            }
        }
    }

    /// A flat frame has nothing to remove: with one value everywhere the
    /// airlight is that value and every pixel stays put; a frame with a hazy
    /// half and a clear dark half comes out with the clear half darker (the
    /// transmission there is below 1 near the haze) and the haze kept at its
    /// airlight; a negative value lifts the dark half toward the haze.
    ///
    /// MUTATION: pick the airlight as the brightest of the top pixels, drop
    /// the 1/t, or let the negative side floor its transmission.
    #[test]
    fn dehaze_removes_a_veil_and_negative_adds_it() {
        let under = UnderTone::new(super::super::ExportColorSpace::Srgb, crate::recipe::CameraStart::default());
        let (w, h) = (64usize, 48usize);
        let flat = vec![[0.5f32; 3]; w * h];
        let mut f = flat.clone();
        apply_camera_raw_dehaze(&mut f, w, h, 60.0, &under);
        for (a, b) in f.iter().zip(&flat) {
            assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3), "flat moved: {a:?}");
        }
        let half: Vec<[f32; 3]> = (0..w * h).map(|i| if i % w < w / 2 { [0.85; 3] } else { [0.25, 0.22, 0.2] }).collect();
        let mut plus = half.clone();
        apply_camera_raw_dehaze(&mut plus, w, h, 60.0, &under);
        let mut minus = half.clone();
        apply_camera_raw_dehaze(&mut minus, w, h, -60.0, &under);
        let near = h / 2 * w + w / 2 + 2;
        assert!(plus[near][1] < half[near][1] - 0.01, "+60 darkens the dark side by the haze: {:?}", plus[near]);
        assert!(minus[near][1] > half[near][1] + 0.01, "−60 veils it: {:?}", minus[near]);
        assert!((plus[h / 2 * w + 2][1] - 0.85).abs() < 0.02, "the haze itself stays: {:?}", plus[h / 2 * w + 2]);
    }
}
