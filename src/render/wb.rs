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
    apply_recipe_wb_in(data, r, ExportColorSpace::Srgb);
}

/// [`apply_recipe_wb`] on a buffer in the working space `space` — the RAW
/// render's, whose primaries a recipe on Camera Raw's law is undone from.
pub(super) fn apply_recipe_wb_in(data: &mut [[f32; 3]], r: &EditRecipe, space: ExportColorSpace) {
    if r.temperature_k.is_some() || r.tint != 0.0 {
        let anchor = r.as_shot_k.unwrap_or(5500.0);
        if SliderLaw::of(r) == SliderLaw::CameraRaw {
            let target = (r.temperature_k.unwrap_or(anchor), r.tint);
            let as_shot = (anchor, r.as_shot_tint.unwrap_or(0.0));
            // As Shot is no move: a fresh develop opens there
            // (`EditRecipe::open_white_balance_at_as_shot`), and rendering it
            // must be what the unstamped recipe rendered.
            if target == as_shot {
                return;
            }
            let m = camera_raw_wb(target, as_shot);
            apply_camera_raw_wb(data, &m, &profile::UnderTone::new(space, r.camera_start.unwrap_or_default()));
            return;
        }
        apply_wb(data, anchor, r.temperature_k.unwrap_or(anchor), r.tint);
    }
}

/// The white a recipe on Camera Raw's law names when it moves from As Shot:
/// `dng_temperature::Get_xy_coord` of its Temperature and Tint, at Camera
/// Raw's own slider ranges (2000–50000 K, ±150). `None` on the engine's law
/// and at As Shot. The RAW render puts this white into the camera stage
/// (`profile::camera_neutral_of`), where Camera Raw puts it.
pub(super) fn camera_raw_white(r: &EditRecipe) -> Option<[f64; 2]> {
    if SliderLaw::of(r) != SliderLaw::CameraRaw || (r.temperature_k.is_none() && r.tint == 0.0) {
        return None;
    }
    let anchor = r.as_shot_k.unwrap_or(5500.0);
    let target = (r.temperature_k.unwrap_or(anchor), r.tint);
    if target == (anchor, r.as_shot_tint.unwrap_or(0.0)) {
        return None;
    }
    Some(profile::temp_tint_to_xy(
        f64::from(target.0.clamp(2000.0, 50000.0)),
        f64::from(target.1.clamp(-150.0, 150.0)),
    ))
}

/// Camera Raw's white balance (v1.17.0, measured): the frame is adapted from
/// the white its Temperature and Tint name to the as-shot white it was
/// rendered at — the DNG SDK's chromaticity of a temperature and tint
/// ([`profile::temp_tint_to_xy`]) and its Bradford adaptation
/// ([`profile::white_shift`]) — on the light the white balance acts on in
/// Camera Raw: before the exposure ramp and the curve, which the profile
/// stage had already put on the frame and [`profile::UnderTone`] undoes and
/// redoes around the shift.
///
/// Measured on p39 (Temperature 4000 / 5000 / 7500 / 10000 K and Tint ±30
/// from its as-shot 5400 K / +14, 1000 px, lens-matched): Camera Raw moves
/// the white a little MORE than the slider's own mired distance
/// ([`WB_MIRED_SCALE`]), the tint by its own ([`WB_TINT_SCALE`]), adds green
/// as it cools and magenta as it warms
/// ([`WB_TINT_PER_MIRED`]) and darkens as it cools ([`WB_EV_PER_MIRED`]) —
/// what its profile's temperature-weighted matrices and tables do that one
/// adaptation does not. The four constants are fitted with the engine in the
/// loop (Nelder–Mead on the band errors of the six renders, the frame's
/// as-shot white held at Lightroom's 5400 K / +14).
///
/// `target` and `as_shot` are (Kelvin, tint) in Camera Raw's units; the
/// matrix acts on the profile stage's linear reference RGB.
pub(super) fn camera_raw_wb(target: (f32, f32), as_shot: (f32, f32)) -> [[f32; 3]; 3] {
    let m0 = 1e6 / f64::from(as_shot.0);
    let m1 = (m0 + WB_MIRED_SCALE * (1e6 / f64::from(target.0) - m0)).clamp(1e6 / 40000.0, 1e6 / 2000.0);
    let shift = m1 - m0;
    let t0 = f64::from(as_shot.1);
    let tint = t0 + WB_TINT_SCALE * (f64::from(target.1) - t0) + WB_TINT_PER_MIRED * shift;
    let from = profile::temp_tint_to_xy(1e6 / m1, tint);
    let to = profile::temp_tint_to_xy(f64::from(as_shot.0), t0);
    let k = (-(WB_EV_PER_MIRED * shift / 100.0) as f32).exp2();
    profile::white_shift(from, to).map(|row| row.map(|v| v * k))
}

/// The white-balance law a recipe's Temperature and Tint render by, as the
/// reverse fit reasons about it (v1.17.0). The fit estimates a colour change
/// from pixels, puts it on (Temperature, Tint), budgets it, walks it and
/// hands it back — and every one of those steps must predict what the render
/// will do with the pair it writes, so they all ask this, and this asks the
/// render's own stage: [`wb_gains`] on the engine's law, [`camera_raw_wb`]
/// under the profile stage's tone on Camera Raw's. Two facts differ between
/// the laws and both were the fit's undoing on a camera-profile start until
/// it asked here: Camera Raw's Tint is ABSOLUTE, so As Shot is the stamped
/// as-shot tint and not 0 (a fit that read "no tint change" as 0 wrote a
/// green shift of the as-shot tint into every recipe whose white balance it
/// moved), and its Temperature moves the light several times as far per
/// mired as the engine law's Tanner–Helland ratio, so a demand inverted on
/// the engine's law ran to the 40000 K edge for a move Camera Raw makes at a
/// fraction of the distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WbLaw {
    camera_raw: bool,
    anchor: f32,
    as_shot_tint: f32,
    /// The camera start the stage's transfer ran at, under which Camera
    /// Raw's white balance acts.
    start: crate::recipe::CameraStart,
}

/// The post-tone linear level the fit's white-balance gains are read at:
/// mid grey. On the engine's law the gains do not depend on level; on Camera
/// Raw's they act under the curve and are read where a frame's mass is.
const WB_LAW_GREY: f32 = 0.18;

impl WbLaw {
    /// The law `r` renders its white balance by, and the as-shot white it
    /// moves from.
    pub(crate) fn of(r: &EditRecipe) -> Self {
        let camera_raw = SliderLaw::of(r) == SliderLaw::CameraRaw;
        WbLaw {
            camera_raw,
            anchor: r.as_shot_k.unwrap_or(5500.0),
            as_shot_tint: if camera_raw { r.as_shot_tint.unwrap_or(0.0) } else { 0.0 },
            start: r.camera_start.unwrap_or_default(),
        }
    }

    /// The pair that renders no move: (as-shot Kelvin, 0) on the engine's
    /// law, where Tint is relative; (as-shot Kelvin, as-shot tint) on
    /// Camera Raw's, where it is absolute.
    pub(crate) fn as_shot(&self) -> (f32, f32) {
        (self.anchor, self.as_shot_tint)
    }

    /// The (Temperature, Tint) `r` renders at, exactly as
    /// [`apply_recipe_wb_in`] reads it: no Temperature and no Tint is As Shot.
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
    /// neutral when it renders `wb`, in the fit's sRGB working light, green
    /// normalised to 1 (white balance moves colour; exposure owns
    /// brightness). The engine's law is [`wb_gains`] itself; Camera Raw's is
    /// [`camera_raw_wb`] on mid grey under [`profile::UnderTone`], the path
    /// [`apply_recipe_wb_in`] runs every pixel through.
    pub(crate) fn gains(&self, wb: (f32, f32)) -> [f32; 3] {
        if !self.camera_raw {
            return wb_gains(self.anchor, wb.0, wb.1);
        }
        if wb == self.as_shot() {
            return [1.0; 3];
        }
        let under = srgb_under_tone(self.start);
        let m = camera_raw_wb(wb, self.as_shot());
        let out = under.redo(mat_vec3(&m, &under.undo([WB_LAW_GREY; 3])));
        let g = out[1].max(1e-6);
        out.map(|v| v.max(1e-6) / g)
    }

    /// The (Temperature, Tint) `r` renders at on the ABSOLUTE scale a
    /// sidecar's `crs:Temperature` / `crs:Tint` carry. Camera Raw's law reads
    /// the recipe's Tint as absolute already; the engine's reads it as a delta
    /// from the as-shot green, so there the photo's as-shot tint is added
    /// back. Comparing a recipe against a sidecar must go through this, or a
    /// Camera Raw recipe's tint counts its as-shot twice.
    pub(crate) fn absolute_point_of(r: &EditRecipe) -> (f32, f32) {
        let law = WbLaw::of(r);
        let (k, t) = law.point_of(r);
        if law.camera_raw { (k, t) } else { (k, r.as_shot_tint.unwrap_or(0.0) + t) }
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
    /// The engine's law is separable and inverts exactly: the Kelvin on the
    /// red/blue axis alone (401 steps in log Kelvin), then the tint as the
    /// green against the red/blue mean of the residual at that Kelvin, so an
    /// unreachable remainder of warmth at the domain's edge stays warmth and
    /// is not booked as green. Camera Raw's is not separable — its
    /// Temperature carries green with it ([`WB_TINT_PER_MIRED`]) and its
    /// Tint moves red and blue as well as green — so it is a joint search
    /// over (mired, tint): a 41 × 41 grid over the domain and three finer
    /// grids about the best point, on the full chromaticity residual.
    pub(crate) fn invert(&self, want_log: [f32; 3], kelvin: (f32, f32)) -> (f32, f32) {
        let quantise = |k: f32, t: f32| ((k / 50.0).round() * 50.0, (t * 10.0).round() / 10.0);
        if !self.camera_raw {
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
            return quantise(best.0, tint);
        }
        let miss = |k: f32, t: f32| {
            let g = self.log_gains((k, t));
            let r = [want_log[0] - g[0], want_log[1] - g[1], want_log[2] - g[2]];
            (r[0] - r[1]).powi(2) + (r[2] - r[1]).powi(2)
        };
        let (lo, hi) = (1e6 / kelvin.1, 1e6 / kelvin.0);
        let (mut mired, mut tint) = (1e6 / self.anchor.clamp(kelvin.0, kelvin.1), self.as_shot_tint);
        let (mut span_m, mut span_t) = (hi - lo, 100.0f32);
        let mut best = (miss(1e6 / mired, tint), mired, tint);
        for _ in 0..4 {
            let centre = (mired, tint);
            for i in 0..=40 {
                let mm = (centre.0 + span_m * (i as f32 / 20.0 - 1.0)).clamp(lo, hi);
                for j in 0..=40 {
                    let tt = (centre.1 + span_t * (j as f32 / 20.0 - 1.0)).clamp(-100.0, 100.0);
                    let e = miss(1e6 / mm, tt);
                    if e < best.0 {
                        best = (e, mm, tt);
                    }
                }
            }
            (mired, tint) = (best.1, best.2);
            span_m /= 10.0;
            span_t /= 10.0;
        }
        let (k, t) = quantise((1e6 / mired).clamp(kelvin.0, kelvin.1), tint);
        (k.clamp(kelvin.0, kelvin.1), t)
    }
}

/// The engine law's green cut at Tint 100 ([`wb_gains`]).
const ENGINE_TINT_GREEN: f32 = 0.20;

/// The sRGB working space's [`profile::UnderTone`] at a camera start, built
/// once per start: the fit asks [`WbLaw::gains`] thousands of times per
/// solve, and a process meets a handful of starts (one per body and white
/// balance headroom). What an under-tone holds depends on the start AND on
/// whether this process has registered the start's tail yet
/// ([`profile::tail_resolves`]): built before the profile is read, it is the
/// fallback without the profile's look tables. So that is part of the key —
/// a fallback is never handed out once the tail is registered.
fn srgb_under_tone(start: crate::recipe::CameraStart) -> std::sync::Arc<profile::UnderTone> {
    use std::sync::{Arc, Mutex, OnceLock};
    type Cache = Mutex<Vec<((crate::recipe::CameraStart, bool), Arc<profile::UnderTone>)>>;
    static UNDER: OnceLock<Cache> = OnceLock::new();
    let key = (start, profile::tail_resolves(start.tables));
    let cache = UNDER.get_or_init(Cache::default);
    let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, u)) = cache.iter().find(|(k, _)| *k == key) {
        return Arc::clone(u);
    }
    let u = Arc::new(profile::UnderTone::new(ExportColorSpace::Srgb, start));
    if cache.len() >= 16 {
        cache.remove(0);
    }
    cache.push((key, Arc::clone(&u)));
    u
}

/// Camera Raw's mired distance per mired the Temperature slider asks for.
const WB_MIRED_SCALE: f64 = 1.042;
/// …its tint per tint unit the Tint slider asks for.
const WB_TINT_SCALE: f64 = 0.999;
/// …the tint it adds per mired of cooling (green; warming adds magenta).
const WB_TINT_PER_MIRED: f64 = -0.078;
/// …and the exposure it takes per 100 mired of cooling, in EV.
const WB_EV_PER_MIRED: f64 = 0.099;

/// [`camera_raw_wb`]'s matrix on every pixel, under the profile stage's tone.
fn apply_camera_raw_wb(data: &mut [[f32; 3]], m: &[[f32; 3]; 3], under: &profile::UnderTone) {
    let (dec, enc) = transfer_luts();
    data.par_iter_mut().for_each(|px| {
        let lin = px.map(|v| sample_lut(dec, v));
        let out = under.redo(mat_vec3(m, &under.undo(lin)));
        *px = out.map(|v| sample_lut(enc, v.clamp(0.0, 1.0)));
    });
}

/// The eyedropper's solve for `r`: the (Temperature, Tint) that renders the
/// base-plate pixel `px` neutral under the law `r` renders by —
/// [`solve_wb_from_neutral`] on the engine's, a search of [`camera_raw_wb`]
/// on Camera Raw's (Tint then in Camera Raw's absolute units).
pub fn solve_wb_for(px: [f32; 3], r: &EditRecipe) -> (f32, f32) {
    let anchor = r.as_shot_k.unwrap_or(5500.0);
    if SliderLaw::of(r) != SliderLaw::CameraRaw {
        return solve_wb_from_neutral(px, anchor);
    }
    let as_shot = (anchor, r.as_shot_tint.unwrap_or(0.0));
    let l = profile::UnderTone::new(ExportColorSpace::Srgb, r.camera_start.unwrap_or_default())
        .undo(px.map(srgb_to_linear))
        .map(|v| v.max(1e-5));
    // Off-neutral of the shifted pixel (white is equal channels in the
    // reference RGB): its chroma over its level.
    let miss = |k: f32, t: f32| {
        let o = mat_vec3(&camera_raw_wb((k, t), as_shot), &l);
        let g = o[1].max(1e-6);
        ((o[0] - g) / g).powi(2) + ((o[2] - g) / g).powi(2)
    };
    // A coarse grid over the legal range (mired, tint), then two finer grids
    // around the best point.
    let (lo, hi) = (1e6 / 40000.0f32, 1e6 / 2000.0f32);
    let (mut mired, mut tint) = (0.5 * (lo + hi), 0.0f32);
    let (mut span_m, mut span_t) = (0.5 * (hi - lo), 100.0f32);
    for _ in 0..3 {
        let mut best = (f32::INFINITY, mired, tint);
        for i in 0..=40 {
            let mm = (mired + span_m * (i as f32 / 20.0 - 1.0)).clamp(lo, hi);
            for j in 0..=40 {
                let tt = (tint + span_t * (j as f32 / 20.0 - 1.0)).clamp(-100.0, 100.0);
                let e = miss(1e6 / mm, tt);
                if e < best.0 {
                    best = (e, mm, tt);
                }
            }
        }
        (mired, tint) = (best.1, best.2);
        span_m /= 10.0;
        span_t /= 10.0;
    }
    ((1e6 / mired).round(), tint)
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

#[cfg(test)]
mod under_tone_cache_tests {
    use super::*;

    /// [`srgb_under_tone`] asked for a start before the process has
    /// registered its tail gets the fallback (no look table), and asked again
    /// after the registration gets the start's own tail, the same as an
    /// under-tone built fresh — not the fallback it cached first. The profile
    /// carries a look table that halves saturation, so the two differ on a
    /// warm colour.
    ///
    /// MUTATION: key the cache on the start alone again.
    #[test]
    fn the_under_tone_cache_uses_the_look_tables_once_the_profile_is_registered() {
        let table = crate::dcp::Table { hue: 2, sat: 2, val: 1, srgb_value: false, data: vec![[0.0, 0.5, 1.0]; 4] };
        let p = crate::dcp::Profile {
            name: "Cache test".into(),
            unique_model: "TEST-CACHE".into(),
            illuminant: [Some(17), Some(21)],
            color_matrix: [None, None],
            forward_matrix: [None, None],
            hue_sat_map: [None, None],
            look_table: Some(table),
            // A curve no other test registers, so the key is this test's.
            tone_curve: vec![[0.0, 0.0], [0.5, 0.4713], [1.0, 1.0]],
            baseline_exposure_offset: 0.0,
            has_third_illuminant: false,
        };
        let key = profile::tail_key(&p, None, None);
        profile::forget_tail(key);
        let start = crate::recipe::CameraStart { ev: 0.0, headroom: 0.0, tables: key };
        let warm = [0.6f32, 0.3, 0.1];
        let before = srgb_under_tone(start).undo(warm);
        assert_eq!(profile::tail_key(&p, None, None), key, "premise: the same profile registers the same key");
        let after = srgb_under_tone(start).undo(warm);
        let fresh = profile::UnderTone::new(ExportColorSpace::Srgb, start).undo(warm);
        assert_eq!(after, fresh, "after the registration the cache hands out the start's own tail");
        let apart = before.iter().zip(&fresh).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(apart > 0.01, "premise: the fallback differs from the tail with its look table ({apart})");
    }
}
