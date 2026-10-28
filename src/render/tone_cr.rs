//! The Basic panel's tone sliders on a camera-profile start (v1.17.0): Camera
//! Raw's law, measured.
//!
//! # Where it applies
//!
//! A recipe that names a camera profile ([`EditRecipe::names_profile`]) starts
//! where Lightroom starts: the profile stage (`render/profile.rs`) has already
//! put the body's baseline exposure, [`exposure_ramp`] and Adobe's default
//! curve on the frame. Camera Raw's Exposure, Contrast, Highlights, Shadows,
//! Whites and Blacks act BEFORE that curve, so on this start the tone LUT
//! undoes the curve, applies the sliders in the curve's own input (`u`, the
//! ramped scene value under the sRGB transfer) and puts the curve back; then
//! the creative Look's baked curve, the parametric curve and the point curve,
//! Lightroom's order. Every other recipe (a camera-matched `base_curve`, a
//! JPEG, every recipe that names no profile) renders through the engine's own
//! knot model, byte for byte.
//!
//! # What was measured
//!
//! Camera Raw rendered by script, one slider at a time, on one frame's exact
//! flat packet (Adobe Color, lens profile on; 6144 px, JPEG q12, compared at
//! 1000 px over near-neutral pixels binned by the flat render's L):
//!
//! * **Exposure** is a linear gain before the curve: over ev −3 … +2 (nine
//!   renders) a pure gain with Adobe's curve predicts every band to rms 1.2 L.
//!   What is left is a black subtracted after the gain and a shoulder
//!   ([`exposure_ramp`]); a black subtracted BEFORE the gain fits worse (rms
//!   0.75 L against 0.40, the +2 EV shadows 2.3 L off). The constants were
//!   scanned on the engine's own renders (flat and ev −3, −1, +1, +2, ten L
//!   bands each): rms 0.64 L, worst 2.7 L (ev −3, L 90–100).
//! * **The channel rule.** Exposure meets a pixel the way Adobe puts a curve
//!   on a colour ([`camera_raw_channels`]): the brightest and darkest
//!   channels through it, the middle one in proportion; Camera Raw's gain is
//!   per channel, before the curve. Measured: the luma ratio the engine law
//!   uses leaves Exposure −1's bright sky 4.3 b* too yellow and +2's warm
//!   tones 7.9 b* off; this rule leaves 0.4 and 1.9. The other five sliders
//!   take their hue from this rule and their lightness from the luma ratio
//!   ([`camera_raw_slider_pixel`]): the rule alone left Highlights −60's
//!   L 90–100 4.1 L bright, the luma ratio alone its sky 5.3 b* short of
//!   Camera Raw's blue. The photographer's parametric and point curves go
//!   through the channel rule, as Adobe puts a point curve on a colour; the
//!   Look's baked curve keeps the luma ratio the canvas start was calibrated
//!   with (step 3). Measured on the five carry pairs' own sidecars (ΔE, the
//!   point and parametric curves through the luma ratio → the channel
//!   rule): p36 9.29 → 8.52, p38 9.61 → 8.79, p39 7.03 → 6.89, p40 8.28 →
//!   8.18, p41 10.96 either way.
//! * **What the ramp cannot do.** Camera Raw compresses the flat frame's sky
//!   (L 90–97, 14 % of the frame) about 2 L further than any pointwise ramp
//!   that also leaves the midtones and the bright specular points where it
//!   puts them: its default highlight handling is image-adaptive. The
//!   residual is +0.8 L there, +1.5 L under Exposure −1.
//! * **The other five** are response tables over `u` ([`RISE`], [`FALL`],
//!   and their curvature [`RISE_Q`], [`FALL_Q`]): at each `u`, the push that
//!   best explains the renders at every measured magnitude (Contrast
//!   +25/±50/±100, Highlights −30/−60/−100/+60/+100, Shadows
//!   +30/+60/+100/−60/−100, Whites and Blacks ±40/±80). Fitted with the
//!   engine in the loop: each pass renders all 23 cases and moves the tables
//!   by the band L error over the curve's slope, pixels clipped at white in
//!   both renders left out; four passes. One table per SIGN:
//!   Camera Raw is not antisymmetric (Blacks −40 moves the toe 3.7× as far as
//!   +40, Whites +40 the shoulder 1.7× as far as −40).
//! * The five act on the EXPOSED value. Exposure −1 with Blacks +40, and +1
//!   with −40: composing predicts the toe within 3.0 L, adding the two on the
//!   original axis is 6.1 L off.

use super::*;

/// Which law a recipe's tone sliders render through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SliderLaw {
    /// The engine's own knot model ([`tone_model_knots`]).
    Engine,
    /// Camera Raw's, measured: this module.
    CameraRaw,
}

impl SliderLaw {
    /// The law `r`'s sliders render through: Camera Raw's exactly when the
    /// recipe starts from a camera profile.
    pub(crate) fn of(r: &EditRecipe) -> Self {
        if r.names_profile() { Self::CameraRaw } else { Self::Engine }
    }
}

/// The black Camera Raw subtracts after the exposure gain, in linear light
/// relative to white. The DNG SDK's default, 0.005, darkens the shadows 2–4 L
/// past Camera Raw's; none at all leaves the flat frame's L 0–30 1.2–1.6 L
/// bright.
pub(crate) const RAMP_BLACK: f32 = 0.0018;

/// Where the shoulder starts, in the black-subtracted linear value.
pub(crate) const RAMP_KNEE: f32 = 0.15;

/// The shoulder's asymptote: above [`RAMP_KNEE`] the value approaches it
/// exponentially with unit slope at the knee, and white is reached at
/// v ≈ 2.0 (clipped there).
pub(crate) const RAMP_SHOULDER: f32 = 1.2;

/// Camera Raw's exposure ramp on one linear channel already multiplied by the
/// total exposure gain: the black subtracted, then the shoulder, clipped at
/// white. It replaces the DNG SDK's hard clip at 1 in the profile stage.
pub(crate) fn exposure_ramp(t: f32) -> f32 {
    let v = (t - RAMP_BLACK).max(0.0) / (1.0 - RAMP_BLACK);
    let span = RAMP_SHOULDER - RAMP_KNEE;
    let r = if v <= RAMP_KNEE { v } else { RAMP_KNEE + span * (1.0 - (-(v - RAMP_KNEE) / span).exp()) };
    r.min(1.0)
}

/// [`exposure_ramp`]'s inverse on `[0, 1]`. A ramped 0 answers 0: whatever
/// sat at or under the black stays black under the sliders.
fn ramp_inverse(r: f32) -> f32 {
    if r <= 0.0 {
        return 0.0;
    }
    let r = r.min(1.0);
    let span = RAMP_SHOULDER - RAMP_KNEE;
    let v = if r <= RAMP_KNEE { r } else { RAMP_KNEE - span * (1.0 - (r - RAMP_KNEE) / span).ln() };
    v * (1.0 - RAMP_BLACK) + RAMP_BLACK
}

/// The samples of the response tables: `u = i / 32`.
const TABLE_N: usize = 33;

/// Push per unit slider for a POSITIVE slider at `u`, in the knot model's
/// order `[contrast, highlights, shadows, whites, blacks]`: a slider at
/// `s ∈ (0, 1]` moves the value by `s · (RISE[k](u) + s · RISE_Q[k](u))`.
/// Fitted from the renders (module head), three-tap smoothed.
const RISE: [[f32; TABLE_N]; 5] = [
    // contrast
    [-0.0556, -0.0556, -0.0846, -0.0933, -0.0726, -0.0620, -0.0622, -0.0588, -0.0496, -0.0357, -0.0183, -0.0004, 0.0184, 0.0374, 0.0522, 0.0642, 0.0759, 0.0843, 0.0922, 0.1019, 0.1028, 0.1073, 0.1168, 0.1013, 0.0864, 0.1111, 0.1380, 0.1157, 0.0679, 0.0336, 0.0480, 0.1284, 0.2200],
    // highlights
    [0.0004, 0.0046, 0.0082, 0.0100, 0.0121, 0.0156, 0.0194, 0.0238, 0.0284, 0.0317, 0.0379, 0.0512, 0.0671, 0.0807, 0.0981, 0.1264, 0.1600, 0.1913, 0.2287, 0.2672, 0.2625, 0.2700, 0.3575, 0.4953, 0.6394, 0.6842, 0.6509, 0.6054, 0.5017, 0.4069, 0.3909, 0.4310, 0.5158],
    // shadows
    [0.0395, 0.0802, 0.0966, 0.0934, 0.0928, 0.0951, 0.0893, 0.0761, 0.0614, 0.0541, 0.0525, 0.0446, 0.0332, 0.0275, 0.0275, 0.0283, 0.0276, 0.0264, 0.0247, 0.0227, 0.0232, 0.0216, 0.0160, 0.0140, 0.0128, 0.0115, 0.0142, 0.0158, 0.0109, 0.0052, 0.0054, 0.0218, 0.0660],
    // whites
    [-0.0001, 0.0002, 0.0002, 0.0001, 0.0002, 0.0005, 0.0008, 0.0014, 0.0018, 0.0018, 0.0028, 0.0040, 0.0052, 0.0072, 0.0087, 0.0103, 0.0124, 0.0119, 0.0119, 0.0148, 0.0049, -0.0030, 0.0054, 0.0080, 0.0510, 0.1539, 0.2376, 0.3355, 0.4243, 0.4186, 0.4401, 0.5327, 0.6317],
    // blacks
    [0.0375, 0.0478, 0.0506, 0.0414, 0.0342, 0.0332, 0.0326, 0.0317, 0.0294, 0.0270, 0.0256, 0.0251, 0.0243, 0.0230, 0.0222, 0.0216, 0.0207, 0.0198, 0.0182, 0.0159, 0.0163, 0.0154, 0.0111, 0.0100, 0.0093, 0.0083, 0.0105, 0.0116, 0.0074, 0.0040, 0.0047, 0.0117, 0.0347],
];

/// [`RISE`] for a NEGATIVE slider: the value moves by `s · FALL[k](u)`, so a
/// positive entry is a push down.
const FALL: [[f32; TABLE_N]; 5] = [
    // contrast
    [-0.0247, -0.0711, -0.0815, -0.0683, -0.0609, -0.0608, -0.0595, -0.0542, -0.0445, -0.0319, -0.0164, -0.0004, 0.0164, 0.0335, 0.0473, 0.0588, 0.0696, 0.0781, 0.0865, 0.0967, 0.0993, 0.0985, 0.1054, 0.1088, 0.1077, 0.1024, 0.1015, 0.1221, 0.1226, 0.0807, 0.0565, 0.0669, 0.0651],
    // highlights
    [0.0043, 0.0070, 0.0096, 0.0112, 0.0131, 0.0162, 0.0197, 0.0240, 0.0286, 0.0322, 0.0383, 0.0507, 0.0662, 0.0793, 0.0946, 0.1194, 0.1525, 0.1863, 0.2238, 0.2610, 0.2557, 0.2573, 0.3314, 0.3674, 0.3521, 0.3817, 0.4175, 0.4091, 0.3654, 0.3043, 0.2210, 0.1485, 0.1520],
    // shadows
    [0.1009, 0.0726, 0.0873, 0.1061, 0.0990, 0.0905, 0.0872, 0.0771, 0.0628, 0.0541, 0.0512, 0.0435, 0.0330, 0.0276, 0.0272, 0.0279, 0.0274, 0.0263, 0.0246, 0.0225, 0.0221, 0.0208, 0.0169, 0.0138, 0.0114, 0.0098, 0.0116, 0.0108, 0.0041, 0.0009, 0.0040, 0.0191, 0.0537],
    // whites
    [0.0006, 0.0015, 0.0023, 0.0031, 0.0044, 0.0069, 0.0103, 0.0146, 0.0189, 0.0236, 0.0299, 0.0369, 0.0450, 0.0545, 0.0637, 0.0734, 0.0846, 0.0957, 0.1080, 0.1215, 0.1261, 0.1300, 0.1430, 0.1485, 0.1523, 0.1530, 0.1505, 0.1733, 0.1895, 0.1519, 0.1039, 0.1063, 0.1456],
    // blacks
    [0.2528, 0.2394, 0.2289, 0.1942, 0.1129, 0.0519, 0.0399, 0.0376, 0.0328, 0.0276, 0.0237, 0.0226, 0.0215, 0.0189, 0.0172, 0.0163, 0.0151, 0.0142, 0.0128, 0.0108, 0.0115, 0.0111, 0.0075, 0.0062, 0.0053, 0.0038, 0.0065, 0.0070, 0.0004, -0.0003, 0.0042, 0.0073, 0.0160],
];

/// The curvature of [`RISE`]: a slider at `s ∈ (0, 1]` moves the value by
/// `s · (RISE[k](u) + s · RISE_Q[k](u))`. Camera Raw's sliders are not linear
/// in their magnitude (Blacks −80 moves the L 20–30 band 2.8× as far as −40);
/// one table per sign fitted to both magnitudes left −40 2.3 L too dark and
/// −80 1.8 L too light.
const RISE_Q: [[f32; TABLE_N]; 5] = [
    // contrast
    [-0.0556, -0.0327, 0.0079, 0.0110, -0.0131, -0.0133, -0.0028, -0.0004, -0.0012, -0.0015, -0.0008, -0.0002, 0.0008, 0.0027, 0.0052, 0.0086, 0.0128, 0.0168, 0.0214, 0.0289, 0.0361, 0.0316, 0.0317, 0.0574, 0.0836, 0.0862, 0.0856, 0.1159, 0.1314, 0.1398, 0.1606, 0.1056, 0.0157],
    // highlights
    [0.0008, 0.0008, 0.0003, 0.0002, 0.0005, 0.0007, 0.0007, 0.0006, 0.0008, 0.0015, 0.0024, 0.0036, 0.0057, 0.0077, 0.0088, 0.0092, 0.0111, 0.0157, 0.0217, 0.0405, 0.0896, 0.1560, 0.1816, 0.0943, -0.0471, -0.0929, -0.0651, -0.0472, -0.0373, -0.0477, -0.0795, -0.1149, -0.1544],
    // shadows
    [0.0625, 0.0361, 0.0277, 0.0325, 0.0308, 0.0215, 0.0126, 0.0071, 0.0051, 0.0042, 0.0032, 0.0021, 0.0012, 0.0010, 0.0012, 0.0013, 0.0010, 0.0008, 0.0007, 0.0005, 0.0009, 0.0016, 0.0011, -0.0004, -0.0006, -0.0004, -0.0012, -0.0024, -0.0026, -0.0015, 0.0001, 0.0040, 0.0102],
    // whites
    [0.0005, 0.0013, 0.0023, 0.0034, 0.0050, 0.0076, 0.0114, 0.0160, 0.0211, 0.0273, 0.0352, 0.0442, 0.0550, 0.0681, 0.0824, 0.0989, 0.1189, 0.1430, 0.1746, 0.2184, 0.2747, 0.3481, 0.4137, 0.4338, 0.4056, 0.3462, 0.3155, 0.3217, 0.2822, 0.1491, -0.0299, -0.1577, -0.2184],
    // blacks
    [-0.0043, 0.0010, 0.0025, 0.0121, 0.0233, 0.0284, 0.0304, 0.0303, 0.0298, 0.0295, 0.0286, 0.0272, 0.0256, 0.0238, 0.0223, 0.0207, 0.0188, 0.0164, 0.0136, 0.0108, 0.0094, 0.0080, 0.0049, 0.0020, 0.0012, 0.0008, 0.0007, 0.0005, 0.0005, 0.0009, 0.0016, 0.0076, 0.0204],
];

/// [`RISE_Q`] for a NEGATIVE slider: the value moves by
/// `s · (FALL[k](u) + |s| · FALL_Q[k](u))`.
const FALL_Q: [[f32; TABLE_N]; 5] = [
    // contrast
    [-0.0392, -0.0027, 0.0108, 0.0051, 0.0009, 0.0024, 0.0053, 0.0079, 0.0080, 0.0062, 0.0034, -0.0002, -0.0039, -0.0070, -0.0091, -0.0103, -0.0107, -0.0107, -0.0102, -0.0095, -0.0085, -0.0059, -0.0039, -0.0037, -0.0023, 0.0056, 0.0060, -0.0140, -0.0163, 0.0132, 0.0191, -0.0052, -0.0098],
    // highlights
    [-0.0016, -0.0014, -0.0009, -0.0004, -0.0006, -0.0008, -0.0009, -0.0010, -0.0008, -0.0008, -0.0015, -0.0035, -0.0061, -0.0081, -0.0104, -0.0146, -0.0217, -0.0302, -0.0382, -0.0428, -0.0290, 0.0305, 0.1303, 0.2016, 0.1580, 0.0298, -0.0418, -0.0242, 0.0125, 0.0479, 0.0940, 0.1398, 0.1746],
    // shadows
    [-0.0074, 0.0002, -0.0242, -0.0333, -0.0206, -0.0168, -0.0163, -0.0105, -0.0051, -0.0030, -0.0028, -0.0025, -0.0016, -0.0011, -0.0010, -0.0012, -0.0010, -0.0007, -0.0004, -0.0003, -0.0005, -0.0005, -0.0002, 0.0002, 0.0009, 0.0005, -0.0007, 0.0005, 0.0022, 0.0014, -0.0007, -0.0045, -0.0108],
    // whites
    [-0.0005, -0.0010, -0.0017, -0.0024, -0.0034, -0.0051, -0.0075, -0.0101, -0.0129, -0.0158, -0.0191, -0.0226, -0.0262, -0.0299, -0.0333, -0.0361, -0.0378, -0.0381, -0.0363, -0.0328, -0.0291, -0.0222, -0.0101, 0.0004, 0.0047, 0.0173, 0.0308, 0.0143, -0.0044, 0.0295, 0.0715, 0.0450, -0.0181],
    // blacks
    [-0.1126, -0.0703, -0.0336, 0.0053, 0.0847, 0.1408, 0.1208, 0.0910, 0.0801, 0.0701, 0.0592, 0.0508, 0.0457, 0.0420, 0.0389, 0.0363, 0.0343, 0.0328, 0.0305, 0.0269, 0.0238, 0.0225, 0.0206, 0.0168, 0.0153, 0.0144, 0.0127, 0.0147, 0.0152, 0.0104, 0.0070, 0.0143, 0.0363],
];

fn table_at(t: &[f32; TABLE_N], u: f32) -> f32 {
    let p = u.clamp(0.0, 1.0) * (TABLE_N - 1) as f32;
    let i = (p.floor() as usize).min(TABLE_N - 2);
    let f = p - i as f32;
    t[i] + (t[i + 1] - t[i]) * f
}

/// Slider `k`'s response row at the exposed value `y` for a slider at `s`:
/// the push per unit slider, so the push is `s · row`. The row depends on
/// `s` only through its sign and, by the curvature tables, its magnitude; the
/// reverse fit's least squares solves with the row held at the previous
/// pass's answer (`fit::tone_cr`).
pub(crate) fn camera_raw_row(k: usize, s: f32, y: f32) -> f32 {
    let (t, q) = if s < 0.0 { (&FALL[k], &FALL_Q[k]) } else { (&RISE[k], &RISE_Q[k]) };
    table_at(t, y) + s.abs() * table_at(q, y)
}

/// Exposure on `u`: undo the ramp, multiply, ramp again.
pub(crate) fn camera_raw_exposure(u: f32, ev: f32) -> f32 {
    if ev == 0.0 {
        return u;
    }
    linear_to_srgb(exposure_ramp(ramp_inverse(srgb_to_linear(u)) * ev.exp2()))
}

/// The law at one `u` without the band guard [`camera_raw_tone`] adds: the
/// exposed value plus the five pushes there, clamped to [0, 1]. What the
/// reverse fit scores a candidate with (it scores the knot model without its
/// limiter too, `fit::tone`).
pub(crate) fn camera_raw_response(ev: f32, s: [f32; 5], u: f32) -> f32 {
    let y = camera_raw_exposure(u, ev);
    let push: f32 = (0..5).map(|k| s[k] * camera_raw_row(k, s[k], y)).sum();
    (y + push).clamp(0.0, 1.0)
}

/// The slider tone over `u` as a [`LUT_N`] table, with the band guard: no
/// interval of the result may rise by less than 1 % of what the exposed curve
/// rises there. A LOCAL floor, not the knot model's one factor on all five
/// sliders: the tables are Camera Raw's own compression (Highlights −100 all
/// but flattens the top of its range, and a factor that kept that interval
/// open would scale every slider in the frame down with it, measured: −60
/// rendered at 0.38 of its strength), so the guard only stops the curve
/// turning back where two pushes overlap.
fn camera_raw_tone(ev: f32, s: [f32; 5]) -> Vec<f32> {
    const KEEP: f32 = 0.01;
    let mut out = Vec::with_capacity(LUT_N);
    let (mut prev_y, mut prev) = (0.0f32, 0.0f32);
    for i in 0..LUT_N {
        let y = camera_raw_exposure(i as f32 / (LUT_N - 1) as f32, ev);
        let push: f32 = (0..5).map(|k| s[k] * camera_raw_row(k, s[k], y)).sum();
        let mut v = (y + push).clamp(0.0, 1.0);
        if i > 0 {
            v = v.max((prev + KEEP * (y - prev_y)).min(1.0));
        }
        out.push(v);
        (prev_y, prev) = (y, v);
    }
    out
}

/// One pixel through a tone LUT the way Adobe puts a curve on a colour
/// (`render/profile.rs`'s `adobe_tone`, on the encoded values): the brightest
/// and darkest channels go through the curve, the middle one keeps its
/// relative place between them. The develop's exposure pass and its curve
/// pass on a Camera Raw recipe ([`camera_raw_exposure_lut`],
/// [`camera_raw_after_lut`]), and the hue of its slider pass
/// ([`camera_raw_slider_pixel`]).
pub(super) fn camera_raw_channels(rgb: [f32; 3], lut: &[f32]) -> [f32; 3] {
    let (mut hi, mut mid, mut lo) = (0usize, 1usize, 2usize);
    if rgb[hi] < rgb[mid] {
        std::mem::swap(&mut hi, &mut mid);
    }
    if rgb[mid] < rgb[lo] {
        std::mem::swap(&mut mid, &mut lo);
    }
    if rgb[hi] < rgb[mid] {
        std::mem::swap(&mut hi, &mut mid);
    }
    let (a, b) = (sample_lut(lut, rgb[hi]), sample_lut(lut, rgb[lo]));
    let span = rgb[hi] - rgb[lo];
    let m = if span > 1e-9 { b + (a - b) * (rgb[mid] - rgb[lo]) / span } else { b };
    let mut out = [0.0f32; 3];
    out[hi] = a;
    out[mid] = m;
    out[lo] = b;
    out
}

/// One pixel through the slider LUT ([`camera_raw_slider_lut`]): the hue and
/// saturation [`camera_raw_channels`] gives, at the lightness the luma ratio
/// gives (`luma601` through the LUT). Measured on Highlights −60: the luma
/// ratio alone left the bright sky's b* 5.3 short of Camera Raw's (Camera Raw
/// deepens the blue as it darkens), the channel rule alone left L 90–100
/// 4.1 L bright; this leaves 1.0 L and 2.3 b*.
pub(super) fn camera_raw_slider_pixel(rgb: [f32; 3], lut: &[f32]) -> [f32; 3] {
    let a = camera_raw_channels(rgb, lut);
    let target = sample_lut(lut, luma601(&rgb));
    let la = luma601(&a);
    if la <= 1e-6 {
        return [target; 3];
    }
    let k = target / la;
    [(a[0] * k).clamp(0.0, 1.0), (a[1] * k).clamp(0.0, 1.0), (a[2] * k).clamp(0.0, 1.0)]
}

/// Adobe's default curve under the sRGB transfer, `u → x`, as a [`LUT_N`]
/// table: what the profile stage did to a ramped value, in the encoding the
/// tone LUT reads.
fn start_curve() -> &'static [f32] {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| {
        (0..LUT_N)
            .map(|i| {
                let u = i as f32 / (LUT_N - 1) as f32;
                linear_to_srgb(profile::default_curve(srgb_to_linear(u)))
            })
            .collect()
    })
}

/// `u → x`: [`start_curve`] read at `u`.
pub(crate) fn camera_raw_start(u: f32) -> f32 {
    sample_lut(start_curve(), u)
}

/// `x → u`, the inverse of [`camera_raw_start`] (Adobe's table is strictly
/// increasing, so the inverse is a search and a lerp).
pub(crate) fn camera_raw_unstart(x: f32) -> f32 {
    let t = start_curve();
    let x = x.clamp(0.0, 1.0);
    let i = t.partition_point(|&v| v < x).clamp(1, LUT_N - 1);
    let (a, b) = (t[i - 1], t[i]);
    let f = if b > a { ((x - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
    (i as f32 - 1.0 + f) / (LUT_N - 1) as f32
}

/// Exposure alone over the profile stage's output, `x → u → ev → x`: the
/// pass the develop puts through [`camera_raw_channels`] before the rest of
/// the tone ([`super::camera_raw_tone_passes`]). `None` at 0 EV.
pub(crate) fn camera_raw_exposure_lut(ev: f32) -> Option<Vec<f32>> {
    (ev != 0.0).then(|| {
        (0..LUT_N)
            .map(|i| {
                let u = camera_raw_unstart(i as f32 / (LUT_N - 1) as f32);
                camera_raw_start(camera_raw_exposure(u, ev))
            })
            .collect()
    })
}

/// The five sliders alone over the profile stage's output, `x → u → the
/// sliders → x`, at 0 EV (the develop applies exposure before it): `None`
/// when all five are 0.
pub(super) fn camera_raw_slider_lut(sliders: [f32; 5]) -> Option<Vec<f32>> {
    sliders.iter().any(|&v| v != 0.0).then(|| {
        let tone = camera_raw_tone(0.0, sliders);
        (0..LUT_N)
            .map(|i| camera_raw_start(sample_lut(&tone, camera_raw_unstart(i as f32 / (LUT_N - 1) as f32))))
            .collect()
    })
}

/// What follows the sliders, as a [`LUT_N`] table: the Look's baked curve
/// (`base`, when there is one), the parametric curve, the point curve.
pub(super) fn camera_raw_after_lut(base: Option<&[f32]>, parametric: Option<&[f32]>, curve: &[f32]) -> Vec<f32> {
    (0..LUT_N)
        .map(|i| {
            let x = i as f32 / (LUT_N - 1) as f32;
            let looked = base.map_or(x, |b| sample_lut(b, x));
            let shaped = parametric.map_or(looked, |p| sample_lut(p, looked));
            sample_lut(curve, shaped)
        })
        .collect()
}

/// The full tone LUT of a recipe on Camera Raw's law: `x` (the profile
/// stage's output) → `u` → the sliders → Adobe's curve again → the Look's
/// baked curve (`base`, when there is one) → the parametric curve → the point
/// curve. `sliders` are in −1..1 with the Look's baked moves already added.
pub(super) fn camera_raw_tone_lut(
    ev: f32,
    sliders: [f32; 5],
    base: Option<&[f32]>,
    parametric: Option<&[f32]>,
    curve: &[f32],
) -> Vec<f32> {
    let tone = camera_raw_tone(ev, sliders);
    (0..LUT_N)
        .map(|i| {
            let x = i as f32 / (LUT_N - 1) as f32;
            let toned = camera_raw_start(sample_lut(&tone, camera_raw_unstart(x)));
            let looked = base.map_or(toned, |b| sample_lut(b, toned));
            let shaped = parametric.map_or(looked, |p| sample_lut(p, looked));
            sample_lut(curve, shaped)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ramp is the identity from its black to its knee, rises strictly
    /// to white, and its inverse undoes it over the open range.
    ///
    /// MUTATION: put the black before the gain's subtraction order back the
    /// other way (`v` from `t / (1 − b)`), or drop the shoulder's unit slope.
    #[test]
    fn the_exposure_ramp_is_monotone_and_inverts() {
        let v = |t: f32| (t - RAMP_BLACK) / (1.0 - RAMP_BLACK);
        for t in [0.01f32, 0.05, 0.1, 0.15] {
            assert!((exposure_ramp(t) - v(t)).abs() < 1e-6, "identity below the knee at {t}");
        }
        assert_eq!(exposure_ramp(0.0), 0.0);
        assert_eq!(exposure_ramp(RAMP_BLACK * 0.5), 0.0, "under the black is black");
        let mut prev = 0.0f32;
        for i in 1..=400 {
            let t = i as f32 / 200.0;
            let r = exposure_ramp(t);
            assert!(r >= prev && r <= 1.0, "monotone and capped at {t}");
            if r < 0.999 {
                assert!(r > prev, "strictly rising below white at {t}");
                assert!((ramp_inverse(r) - t).abs() < 1e-4, "inverse at {t}: {}", ramp_inverse(r));
            }
            prev = r;
        }
        assert!(exposure_ramp(1.0) < 1.0, "the shoulder keeps white's neighbourhood open");
    }

    /// Adobe's curve under the transfer and its inverse compose to the
    /// identity, so a recipe with no slider work leaves `u` where it was.
    #[test]
    fn the_start_and_its_inverse_compose_to_the_identity() {
        for i in 0..=100 {
            let u = i as f32 / 100.0;
            assert!((camera_raw_unstart(camera_raw_start(u)) - u).abs() < 2e-3, "at {u}");
        }
    }

    /// The law applies exactly to a recipe that names a camera profile.
    #[test]
    fn the_law_follows_the_profile() {
        assert_eq!(SliderLaw::of(&EditRecipe::default()), SliderLaw::Engine);
        let named = EditRecipe { camera_profile: "Adobe Standard".into(), ..Default::default() };
        assert_eq!(SliderLaw::of(&named), SliderLaw::CameraRaw);
    }

    /// Every slider at its extremes, alone and together, gives a
    /// non-decreasing LUT inside [0, 1] (the band guard), and Exposure raises
    /// every value it can.
    ///
    /// MUTATION: drop the band guard in `camera_raw_tone`, or let the
    /// exposure LUT read `u` without undoing the start.
    #[test]
    fn the_slider_tone_stays_monotone_at_every_extreme() {
        let mut cases = vec![[1.0f32, 1.0, 1.0, 1.0, 1.0], [-1.0; 5], [1.0, -1.0, 1.0, -1.0, 1.0], [-1.0, 1.0, -1.0, 1.0, -1.0]];
        for k in 0..5 {
            for s in [-1.0f32, 1.0] {
                let mut c = [0.0f32; 5];
                c[k] = s;
                cases.push(c);
            }
        }
        for ev in [-3.0f32, 0.0, 2.0] {
            for s in &cases {
                let t = camera_raw_tone(ev, *s);
                assert!(t.windows(2).all(|w| w[1] >= w[0]), "monotone at ev {ev} {s:?}");
                assert!(t.iter().all(|v| (0.0..=1.0).contains(v)), "in range at ev {ev} {s:?}");
            }
        }
        let up = camera_raw_exposure_lut(1.0).expect("a non-zero exposure");
        for (i, &v) in up.iter().enumerate() {
            let x = i as f32 / (LUT_N - 1) as f32;
            assert!(v >= x - 1e-4, "+1 EV raises {x}: {v}");
        }
        assert!(camera_raw_exposure_lut(0.0).is_none());
    }

    /// The slider pixel gives a grey the luma rule's value and keeps a
    /// colour's lightness at the luma rule's while taking the channel rule's
    /// hue.
    #[test]
    fn the_slider_pixel_takes_the_luma_lightness_and_the_channel_hue() {
        let lut: Vec<f32> = (0..LUT_N).map(|i| (i as f32 / (LUT_N - 1) as f32).powf(1.4)).collect();
        let g = camera_raw_slider_pixel([0.5; 3], &lut);
        let want = sample_lut(&lut, 0.5);
        assert!(g.iter().all(|&c| (c - want).abs() < 1e-5), "a grey: {g:?} vs {want}");
        let px = [0.7f32, 0.45, 0.2];
        let out = camera_raw_slider_pixel(px, &lut);
        assert!((luma601(&out) - sample_lut(&lut, luma601(&px))).abs() < 1e-4, "lightness by luma");
        let a = camera_raw_channels(px, &lut);
        let ratio = |p: [f32; 3]| [p[0] / p[1], p[2] / p[1]];
        let (ro, ra) = (ratio(out), ratio(a));
        assert!((ro[0] - ra[0]).abs() < 1e-4 && (ro[1] - ra[1]).abs() < 1e-4, "hue by channels");
    }
}
