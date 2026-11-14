//! White balance and transfer curves: blackbody colour, WB gains and their solver, the sRGB transfer functions.

use super::*;

/// Shared, parameter-free transfer-curve LUTs: `[0]` = sRGB→linear, `[1]` =
/// linear→sRGB. Built once per process (OnceLock). Dehaze and vignette used to
/// evaluate both `powf` curves for every pixel — the identical 6-powf/px
/// pattern the v0.8.1 colour-gain fix removed (measured there: 609 ms vs 53 ms
/// per 1280×853 frame). 4096-entry linear interpolation keeps the error in the
/// same sub-8-bit-quantisation envelope `colour_gain_luts` pinned with a test;
/// dehaze's airlight HISTOGRAM keeps the exact powf (a 1e-5 shift across a bin
/// edge could move the whole-frame airlight estimate — not worth the few ms).
pub(super) fn transfer_luts() -> &'static ([f32; LUT_N], [f32; LUT_N]) {
    use std::sync::OnceLock;
    static LUTS: OnceLock<([f32; LUT_N], [f32; LUT_N])> = OnceLock::new();
    LUTS.get_or_init(|| {
        let mut dec = [0.0f32; LUT_N];
        let mut enc = [0.0f32; LUT_N];
        for i in 0..LUT_N {
            let x = i as f32 / (LUT_N - 1) as f32;
            dec[i] = srgb_to_linear(x);
            enc[i] = linear_to_srgb(x);
        }
        (dec, enc)
    })
}

/// Decode one working-space value to linear light — the LUT inside 0..1, the
/// exact transfer outside it, so a wide-gamut develop's negative or >1
/// component survives a linear-light stage instead of being clipped by it.
pub(super) fn decode_wide(dec: &[f32], c: f32) -> f32 {
    if (0.0..=1.0).contains(&c) { sample_lut(dec, c) } else { srgb_to_linear(c) }
}

/// [`decode_wide`]'s inverse.
pub(super) fn encode_wide(enc: &[f32], c: f32) -> f32 {
    if (0.0..=1.0).contains(&c) { sample_lut(enc, c) } else { linear_to_srgb(c) }
}

/// Blackbody colour at temperature `k` Kelvin as RGB in [0,1].
/// Tanner-Helland piecewise fit [verified: tannerhelland.com/2012/09/18,
/// R²>0.987], with the cool-side branches RESCALED so every branch seam is
/// continuous. Valid 1000–40000 K.
///
/// The published constants leave cliffs at the 6600 K seam — green jumps
/// 1.31 % and blue 0.96 % — and the cool red branch starts at 259.7, which
/// the clamp holds flat at 255 until 6688 K. `wb_gains` divides one of these
/// curves by another, so at the seam two near-identical temperatures produced
/// visibly different gains, and inside the 88 K red plateau the r/b ratio —
/// the eyedropper's temperature signal — did not move at all. Each cool
/// branch keeps the published EXPONENT (the fit's shape) and gets its
/// coefficient(s) recalibrated on the seam values instead:
///   red   329.69873 → 323.73796  (branch(66) = 255 exactly, plateau gone)
///   green 288.12216 → 291.94575  (branch(66) = 255 = warm branch, clamped)
///   blue  a·ln(t−10)−b with a 138.51773 → 139.48702, b 305.0448 → 306.48430
///         (two-point: blue(6600 K) = 255 AND blue(1900 K) = 0, so repairing
///         the top seam does not open one at the bottom).
/// Worst mid-range deviation from the published fit is under 2 % of
/// full-scale — smaller than the discontinuities it removes.
pub(super) fn kelvin_to_rgb(k: f32) -> [f32; 3] {
    let t = k.clamp(1000.0, 40000.0) / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        (323.737_96 * (t - 60.0).powf(-0.133_204_76)).clamp(0.0, 255.0)
    };
    let green = if t <= 66.0 {
        (99.470_8 * t.ln() - 161.119_57).clamp(0.0, 255.0)
    } else {
        (291.945_75 * (t - 60.0).powf(-0.075_514_846)).clamp(0.0, 255.0)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        (139.487_02 * (t - 10.0).ln() - 306.484_3).clamp(0.0, 255.0)
    };
    [red / 255.0, green / 255.0, blue / 255.0]
}

/// Per-channel gains to move WB from `as_shot_k` to `target_k` (+ tint), green
/// normalised to 1.0 (WB changes colour, not brightness). Lightroom convention:
/// higher target K = warmer result (boosts red, cuts blue). `pub(crate)` so the
/// zoned fit can INVERT the engine's own model instead of duplicating it.
pub(crate) fn wb_gains(as_shot_k: f32, target_k: f32, tint: f32) -> [f32; 3] {
    let a = kelvin_to_rgb(as_shot_k);
    let t = kelvin_to_rgb(target_k);
    let g1 = a[1] / t[1].max(1e-4);
    let gr = (a[0] / t[0].max(1e-4)) / g1;
    let gb = (a[2] / t[2].max(1e-4)) / g1;
    // Tint: positive = magenta (less green), negative = green.
    let gg = 1.0 - ENGINE_TINT_GREEN * (tint / 100.0);
    [gr, gg, gb]
}

/// Map the local mask Temp slider — a RELATIVE warm/cool shift, ±100 (see
/// `LocalAdjustment::temperature`) — to the target Kelvin the shared
/// [`wb_gains`] model expects: a linear shift in MIRED (1e6/K, the unit
/// photographic conversion gels are specified in, ~perceptually uniform for
/// WB) around a FIXED 5500 K anchor. (The global stage anchored there too
/// until batch 29 taught it the photo's stamped as-shot Kelvin; a local
/// slider is a relative gel, so it keeps the fixed anchor and the two
/// deliberately differ — R12.) Full scale ±100
/// ⇒ ∓80 mired (≈ half a CTO/CTB gel): +100 → ~9823 K (warmer — matching
/// wb_gains' "higher target K = warmer" convention), −100 → ~3820 K. Both
/// endpoints sit inside kelvin_to_rgb's 1000–40000 K validity. ACR's exact
/// local-temp model is proprietary — this is our documented approximation
/// (same stance as [`manual_vignette_lut`]); the XMP carries the raw slider value,
/// so Lightroom re-renders with its own model.
// `pub`, not `pub(crate)`: the mask panel's Temp-shift tooltip states the
// equivalent Kelvin for the value on the slider, and it must be THIS
// function's answer — a number retyped into the GUI would drift the moment
// the anchor or the mired scale moves.
pub fn local_temp_to_kelvin(t: f32) -> f32 {
    const ANCHOR_K: f32 = 5500.0;
    const MIRED_FULL_SCALE: f32 = 80.0;
    let mired = 1e6 / ANCHOR_K - (t.clamp(-100.0, 100.0) / 100.0) * MIRED_FULL_SCALE;
    1e6 / mired
}

/// Build one LUT per channel for a linear-light RGB gain. The exact transform
/// is `linear_to_srgb(srgb_to_linear(x) * gain)`, but evaluating both transfer
/// curves (`powf`) for every pixel/channel dominated v0.8 zoned preview time:
/// measured on the production-shaped 1280×853 probe, one colour-gain bitmap
/// mask cost 609 ms vs 53 ms for the SAME mask without colour gains; the
/// sky+land pair cost 1188 ms vs 92 ms. A 4096-entry LUT evaluates the exact
/// formula only 12k times per adjustment, then the existing linear sampler
/// handles millions of pixels. `LUT_N=4096` keeps interpolation error below
/// the engine's 8/16-bit output quantisation (pinned by the test below).
pub(super) fn colour_gain_luts(g: [f32; 3]) -> [Vec<f32>; 3] {
    std::array::from_fn(|ch| {
        (0..LUT_N)
            .map(|i| {
                let x = i as f32 / (LUT_N - 1) as f32;
                linear_to_srgb((srgb_to_linear(x) * g[ch]).clamp(0.0, 1.0))
            })
            .collect()
    })
}

/// A channel at or above this is at the white clip (254/255 and 255/255 of an
/// 8-bit side both count: a JPEG's clipped white decodes as either): the
/// range's ceiling, not a measurement. ONE number for the two sides that
/// read it — the reverse fit's rule for what testifies (`fit::tone`'s
/// ceiling; RULING-1180-joint-r2 §4, "a channel at the white clip is not a
/// value") and, since 2026-11-10, the develop's rule for what an ESTIMATED
/// colour correction may scale ([`keep_blown_white`]).
pub(crate) const WHITE_CLIP: f32 = 0.995;

/// Whether a pixel is at the white clip in EVERY channel ([`WHITE_CLIP`]): the
/// pixel [`keep_blown_white`] keeps neutral under an estimated correction,
/// and the one whose channels the colour field's solve does not ask about one
/// by one (`fit_field::testifying`, J153). ONE predicate for the two, so the
/// solve never asks a pixel for a move the develop will not render on it.
#[inline]
pub(crate) fn blown_white(p: &[f32; 3]) -> bool {
    p[0].min(p[1]).min(p[2]) >= WHITE_CLIP
}

/// A pixel that reaches an estimated colour correction with EVERY channel at
/// the white clip carries no ratio between its channels — each one says only
/// "at least the ceiling" — so a correction read from measured pixels has
/// nothing of it to scale: the pixel leaves the stage neutral, at the
/// brightest of the values the stage gave it (each is a lower bound of what
/// that channel would read, and the highest bound is the one every channel
/// meets). Any other pixel is returned as the stage rendered it, bit for bit
/// — a pixel with one or two channels at the clip keeps its measured
/// channels' say.
///
/// The stages are the two the reverse fit ESTIMATES and no slider sets: a
/// mask's colour gains (`LocalAdjustment::color_gains`, `masks`) and the
/// colour field (`develop`). An estimate speaks for the pixels it was read
/// from: a channel at the white clip never testifies to the tone fit
/// (`fit::tone`), and the colour field's solve asks nothing of these pixels'
/// channels one by one ([`blown_white`], `fit_field::testifying`). The white
/// balance and a mask's own Temp and Tint are the
/// photographer's instruction and apply to every pixel they are set on,
/// white included (a cooler mask over a blown sky is how that sky is given
/// colour); the curves, the wheels, the mixer and a mask's Color tint map the
/// rendered value itself. None of those is touched.
///
/// Until 2026-11-10 a blown pixel was scaled like a measurement: on the
/// night pair the brightest stars' cores, white in the base and white under
/// the recipe's global controls, rendered (0.73, 1.00, 1.00) under the sky
/// zone's gains (R ×0.52) — cyan cores the correction had never been read
/// from; with the rule they read (0.96, 1.00, 1.00) over the pixels that
/// moved, 0.005 % of the frame, and the seven daylight pairs move by at most
/// 6 codes on at most 0.16 % of their pixels (rel1180/star).
#[inline]
pub(super) fn keep_blown_white(before: &[f32; 3], after: [f32; 3]) -> [f32; 3] {
    if !blown_white(before) {
        return after;
    }
    [after[0].max(after[1]).max(after[2]); 3]
}

/// Apply white-balance gains in linear light. No-op when gains are ~neutral.
/// Uses [`colour_gain_luts`] so preview cost scales with pixels, not with six
/// transcendental operations per pixel.
fn apply_wb(data: &mut [[f32; 3]], as_shot_k: f32, target_k: f32, tint: f32) {
    let g = wb_gains(as_shot_k, target_k, tint);
    if (g[0] - 1.0).abs() < 1e-3 && (g[1] - 1.0).abs() < 1e-3 && (g[2] - 1.0).abs() < 1e-3 {
        return;
    }
    let luts = colour_gain_luts(g);
    // Row-parallel like every other per-pixel stage (the v0.11.0 sweep took
    // the tone, HSL, grading, curve, dehaze and vignette passes; this one was
    // left serial and is the stage EVERY Temp/Tint tick runs through, at full
    // sensor resolution on export). Each pixel reads only itself and the
    // read-only LUTs, so the result is bit-identical — no accumulation and no
    // order dependence to change.
    data.par_iter_mut().for_each(|px| {
        for c in 0..3 {
            px[c] = sample_lut(&luts[c], px[c]);
        }
    });
}

/// The ONE recipe→WB stage, shared by the full-res render, the baked-image
/// render and the UI preview so they can never disagree. The buffer arrives
/// at as-shot WB; the shift is anchored at the photo's STAMPED as-shot
/// Kelvin (`as_shot_k`, engine-only — [`as_shot_wb`]), so `temperature_k`
/// finally speaks ABSOLUTE Kelvin: target == as-shot is a true no-op, and
/// the number agrees with what Lightroom shows for the same XMP. A legacy
/// recipe (`None`) keeps the historical 5500 K daylight anchor —
/// byte-identical rendering of every old archive. `temperature_k = None`
/// only means "no Kelvin shift" (the target IS the anchor) — tint still
/// applies on its own, matching the recipe contract (tint 0 = neutral) and
/// what the GUI slider promises.
pub(super) fn apply_recipe_wb(data: &mut [[f32; 3]], r: &EditRecipe) {
    if r.temperature_k.is_some() || r.tint != 0.0 {
        let anchor = r.as_shot_k.unwrap_or(5500.0);
        apply_wb(data, anchor, r.temperature_k.unwrap_or(anchor), r.tint);
    }
}

/// The white-balance law a recipe's Temperature and Tint render by, as the
/// reverse fit reasons about it. The fit estimates a colour change from
/// pixels, puts it on (Temperature, Tint), budgets it, walks it and hands it
/// back — and every one of those steps must predict what the render will do
/// with the pair it writes, so they all ask this, and this asks the render's
/// own stage ([`wb_gains`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WbLaw {
    anchor: f32,
}

impl WbLaw {
    /// The law `r` renders its white balance by, and the as-shot white it
    /// moves from.
    pub(crate) fn of(r: &EditRecipe) -> Self {
        WbLaw { anchor: r.as_shot_k.unwrap_or(5500.0) }
    }

    /// The pair that renders no move: (as-shot Kelvin, 0) — Tint is relative.
    pub(crate) fn as_shot(&self) -> (f32, f32) {
        (self.anchor, 0.0)
    }

    /// The (Temperature, Tint) `r` renders at, exactly as
    /// [`apply_recipe_wb`] reads it: no Temperature and no Tint is As Shot.
    pub(crate) fn point_of(&self, r: &EditRecipe) -> (f32, f32) {
        self.point(r.temperature_k, r.tint)
    }

    /// [`WbLaw::point_of`] for a recipe's two fields.
    pub(crate) fn point(&self, temperature_k: Option<f32>, tint: f32) -> (f32, f32) {
        match temperature_k {
            None if tint == 0.0 => self.as_shot(),
            k => (k.unwrap_or(self.anchor), tint),
        }
    }

    /// The per-channel gains the render's white-balance stage puts on a
    /// neutral when it renders `wb`, in the fit's sRGB working light
    /// ([`wb_gains`]).
    pub(crate) fn gains(&self, wb: (f32, f32)) -> [f32; 3] {
        wb_gains(self.anchor, wb.0, wb.1)
    }

    /// The (Temperature, Tint) `r` renders at on the ABSOLUTE scale a
    /// sidecar's `crs:Temperature` / `crs:Tint` carry: the recipe's Tint is a
    /// delta from the as-shot green, so the photo's as-shot tint is added
    /// back. Comparing a recipe against a sidecar must go through this.
    pub(crate) fn absolute_point_of(r: &EditRecipe) -> (f32, f32) {
        let (k, t) = WbLaw::of(r).point_of(r);
        (k, r.as_shot_tint.unwrap_or(0.0) + t)
    }

    /// [`WbLaw::gains`] as natural logs.
    pub(crate) fn log_gains(&self, wb: (f32, f32)) -> [f32; 3] {
        self.gains(wb).map(|v| v.max(1e-5).ln())
    }

    /// A wanted log-gain triple put onto this law's (Kelvin, tint) manifold:
    /// the pair over `kelvin` (and the recipe's ±100 tint) whose
    /// [`WbLaw::log_gains`] match it up to a common scalar. The Kelvin comes
    /// back rounded to 50, the tint to 0.1, as the recipe persists them.
    ///
    /// The law is separable and inverts exactly: the Kelvin on the red/blue
    /// axis alone (401 steps in log Kelvin), then the tint as the green
    /// against the red/blue mean of the residual at that Kelvin, so an
    /// unreachable remainder of warmth at the domain's edge stays warmth and
    /// is not booked as green.
    pub(crate) fn invert(&self, want_log: [f32; 3], kelvin: (f32, f32)) -> (f32, f32) {
        let quantise = |k: f32, t: f32| ((k / 50.0).round() * 50.0, (t * 10.0).round() / 10.0);
        {
            let (lo, hi) = (kelvin.0.ln(), kelvin.1.ln());
            let log_gains = |k: f32| self.log_gains((k, 0.0));
            let mut best = (self.anchor, f32::INFINITY);
            for i in 0..=400 {
                let k = (lo + (hi - lo) * i as f32 / 400.0).exp();
                let g = log_gains(k);
                let err = ((want_log[0] - g[0]) - (want_log[2] - g[2])).powi(2);
                if err < best.1 {
                    best = (k, err);
                }
            }
            // With the common scalar `s` and the green log-gain `t` free, the
            // least squares of `(r_R − s, r_G − s − t, r_B − s)` is exact in
            // `s = (r_R + r_B)/2` and `t = r_G − s`.
            let g = log_gains(best.0);
            let r = [want_log[0] - g[0], want_log[1] - g[1], want_log[2] - g[2]];
            let t = r[1] - 0.5 * (r[0] + r[2]);
            let tint = ((1.0 - t.exp()) / ENGINE_TINT_GREEN * 100.0).clamp(-100.0, 100.0);
            quantise(best.0, tint)
        }
    }
}

/// The engine law's green cut at Tint 100 ([`wb_gains`]).
const ENGINE_TINT_GREEN: f32 = 0.20;

/// The eyedropper's solve for `r`: the (Temperature, Tint) that renders the
/// base-plate pixel `px` neutral, anchored at `r`'s as-shot white
/// ([`solve_wb_from_neutral`]).
pub fn solve_wb_for(px: [f32; 3], r: &EditRecipe) -> (f32, f32) {
    solve_wb_from_neutral(px, r.as_shot_k.unwrap_or(5500.0))
}

/// Inverse white balance — the WB eyedropper's solver. Given an sRGB pixel the
/// user says SHOULD be neutral, find the (target Kelvin, tint) whose
/// [`wb_gains`] neutralise it, using the exact forward model the render then
/// applies — anchored at `as_shot_k` (the photo's stamped as-shot Kelvin, or
/// the legacy 5500 K), so the solved Kelvin lands in the same absolute scale
/// the Temp slider now speaks. Target K is scanned on a log grid (400 steps
/// over the recipe's legal 2000–40000 K) to equalise the red/blue channels;
/// tint then falls analytically out of the green residual
/// (gg = 1 − 0.20·tint/100). Returns (kelvin, tint clamped to ±100).
pub fn solve_wb_from_neutral(px: [f32; 3], as_shot_k: f32) -> (f32, f32) {
    let lr = srgb_to_linear(px[0]).max(1e-5);
    let lg = srgb_to_linear(px[1]).max(1e-5);
    let lb = srgb_to_linear(px[2]).max(1e-5);
    const N: usize = 400;
    let (lo, hi) = ((2000.0f32).ln(), (40000.0f32).ln());
    let mut best = (as_shot_k, f32::INFINITY);
    for i in 0..=N {
        let k = (lo + (hi - lo) * i as f32 / N as f32).exp();
        let g = wb_gains(as_shot_k, k, 0.0);
        let e = (lr * g[0] - lb * g[2]).abs();
        if e < best.1 {
            best = (k, e);
        }
    }
    let k = best.0;
    let g = wb_gains(as_shot_k, k, 0.0);
    // Green gain that lands green on the (now equal) red/blue level → tint.
    // Bounded to the gg range tint can actually express (tint ±100 ⇒ gg 0.8–1.2).
    let level = 0.5 * (lr * g[0] + lb * g[2]);
    let gg = (level / lg).clamp(0.8, 1.2);
    let tint = ((1.0 - gg) / 0.20 * 100.0).clamp(-100.0, 100.0);
    (k, tint)
}

// `pub(crate)`: the zoned fit computes zone moments in linear light with the
// engine's exact transfer curve (a duplicated constant would drift).
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
// `pub(crate)` for the same reason as its inverse above: the zoned fit's
// joint value-range family reads its bucket means back OUT of linear light
// (R23-6), and a second copy of this curve would be a second thing to keep
// in step with the engine.
pub(crate) fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
pub(super) fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `smoothstep` with a degenerate-edge guard: equal edges are a hard step
/// instead of the 0/0 NaN `smoothstep` returns there (and `clamp` propagates).
/// Identical to `smoothstep` whenever `e1 - e0 >= 1e-6`.
pub(super) fn ramp(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 - e0 < 1e-6 {
        if x < e0 { 0.0 } else { 1.0 }
    } else {
        smoothstep(e0, e1, x)
    }
}
