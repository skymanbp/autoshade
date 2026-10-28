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
//! * Contrast, Whites and Blacks act on the EXPOSED value. Exposure −1 with
//!   Blacks +40, and +1 with −40: composing predicts the toe within 3.0 L,
//!   adding the two on the original axis is 6.1 L off.
//!
//! # Highlights and Shadows: before Exposure, local, keyed to the frame
//!
//! Measured on seven frames' flat packets (every slider 0, Adobe Color, lens
//! as each sidecar says), Shadows +30/+60/+100 and Highlights −30/−60/−100
//! each (6144 px, compared at 768 px in `u`):
//!
//! * **Order.** Composing Camera Raw's own pointwise maps on two frames,
//!   Highlights and Shadows go BEFORE Exposure (mean |ΔL| 0.23–0.42 that way,
//!   1.17–3.68 the other) and before Contrast and Blacks too (0.11–0.22
//!   against 0.30–2.48). They are the first pass ([`apply_camera_raw_hs`]).
//! * **Local.** The push at a pixel follows the region it sits in: one
//!   frame's lifted foreground is lifted by the same `u` from 0.20 to 0.35
//!   and its sky at 0.36 by an eighth of that, a step in the pixel's own value
//!   no pointwise law has; next to a bright edge a dark pixel is lifted less
//!   over 200–400 film pixels. The push reads an edge-aware base
//!   ([`bilateral_base`], [`HS_SIGMA_S`] / [`HS_SIGMA_S2`] film pixels,
//!   [`HS_SIGMA_R`] stops) of `log2` linear luminance.
//! * **Keyed to the frame.** At one base value the push still differs 6×
//!   between frames (Highlights −60 at `u` 0.3: 0.022 on a daylight street,
//!   0.142 on a dark dusk frame); measured relative to the frame's mean
//!   linear luminance (`z = log2(base) − log2(mean)`) it does not: the mean
//!   ordered the frames, the median, log-average and high percentiles did
//!   not. Shadows read `(z, u)` ([`HS_SH`]); Highlights read `z` on the fine
//!   base plus `z` on the coarse one ([`HS_HL`], [`HS_HL2`]) — a second
//!   scale raised the held-out R² of the frame it helped most from 0.76 to
//!   0.94.
//! * **Colour.** Shadows keep the colour: every channel by one ratio over a
//!   floor ([`HS_CHROMA_FLOOR`]); per channel left the warm tones 4.75 b*
//!   short. Highlights take the hue of Adobe's channel rule with the pixel's
//!   own curve, as the other sliders do (the street's bright band 5.14 →
//!   0.80 b*).
//! * Fitted pooled over the seven frames and three magnitudes, then with the
//!   engine in the loop; the signs not measured across frames (Highlights
//!   above 0, Shadows below) keep the pointwise tables at the pixel's `u`.

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
pub(crate) fn ramp_inverse(r: f32) -> f32 {
    if r <= 0.0 {
        return 0.0;
    }
    let r = r.min(1.0);
    let span = RAMP_SHOULDER - RAMP_KNEE;
    let v = if r <= RAMP_KNEE { r } else { RAMP_KNEE - span * (1.0 - (r - RAMP_KNEE) / span).ln() };
    v * (1.0 - RAMP_BLACK) + RAMP_BLACK
}

/// The samples of the response tables: `u = i / 32`.
pub(super) const TABLE_N: usize = 33;

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

/// Dehaze's response over `u`, as the five's: a slider at `d ∈ [−1, 1]`
/// moves the value by `d · (T(u) + |d| · Q(u))`, `T`/`Q` the RISE pair for a
/// positive slider and the FALL pair for a negative one. Fitted like the five
/// (module head) from Dehaze +40/+80/−30; one negative magnitude, so
/// [`DEHAZE_FALL_Q`] is zero.
const DEHAZE_RISE: [f32; TABLE_N] = [-0.0201, -0.0298, -0.0361, -0.0363, -0.0376, -0.0449, -0.0538, -0.0612, -0.0667, -0.0672, -0.0639, -0.0606, -0.0545, -0.0451, -0.0389, -0.0399, -0.0453, -0.0495, -0.0535, -0.0587, -0.0610, -0.0783, -0.1257, -0.1594, -0.1491, -0.1357, -0.1419, -0.1297, -0.0853, -0.0426, -0.0281, -0.0502, -0.1007];
const DEHAZE_RISE_Q: [f32; TABLE_N] = [0.0014, 0.0009, 0.0019, 0.0041, 0.0052, 0.0037, -0.0006, -0.0068, -0.0121, -0.0136, -0.0121, -0.0107, -0.0092, -0.0071, -0.0059, -0.0062, -0.0074, -0.0077, -0.0076, -0.0082, -0.0090, -0.0249, -0.0796, -0.1277, -0.1155, -0.0913, -0.0899, -0.0769, -0.0422, -0.0139, -0.0094, -0.0238, -0.0457];
const DEHAZE_FALL: [f32; TABLE_N] = [-0.0794, -0.1223, -0.1613, -0.1608, -0.1357, -0.1274, -0.1350, -0.1464, -0.1563, -0.1585, -0.1536, -0.1454, -0.1313, -0.1138, -0.1039, -0.1066, -0.1156, -0.1228, -0.1310, -0.1400, -0.1421, -0.1487, -0.1659, -0.1684, -0.1513, -0.1453, -0.1540, -0.1413, -0.1015, -0.0641, -0.0500, -0.0852, -0.1799];
const DEHAZE_FALL_Q: [f32; TABLE_N] = [-0.0000, -0.0000, -0.0000, -0.0000, 0.0000, -0.0000, -0.0000, -0.0000, -0.0000, 0.0000, 0.0000, -0.0000, -0.0000, -0.0000, 0.0000, 0.0000, 0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, 0.0000, 0.0000, 0.0000, 0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000];

/// Dehaze's colour: Camera Raw deepens colour as it removes haze and pales it
/// as it adds haze, most in the bright range (the sky). The pixel's chroma is
/// scaled by `exp(d · DEHAZE_CHROMA(u))` about its luma, `u` the pixel's
/// own place before the sliders. Fitted like the tables, on the median
/// log-chroma change of coloured pixels (C* > 5) per bin of `u`: per unit
/// slider +0.3 (L 10–30), ≈ 0 (L 40–70), +0.8 (L 80–90), +1.7…2.2
/// (L 90–100), the same for +40, +80 and −30.
pub(super) const DEHAZE_CHROMA: [f32; TABLE_N] = [0.2942, 0.2942, 0.2427, 0.1092, 0.1802, 0.5409, 0.7253, 0.6303, 0.4594, 0.2431, 0.0317, -0.1653, -0.3211, -0.3277, -0.1799, 0.0185, 0.1835, 0.3382, 0.6228, 1.0963, 1.5320, 1.6586, 1.7833, 1.7908, 1.1677, 0.5824, 0.4696, 0.4696, 0.4696, 0.4696, 0.4696, 0.4696, 0.4696];

/// Clarity's TONE on Camera Raw's law: besides its local contrast (the
/// develop's clarity stage, unchanged) Camera Raw's Clarity lifts the
/// midtones — measured on the flat packet, +50 raises the band means of
/// L 10–80 by 6–11 L, where the local stage alone moves them by 1–3. A
/// response pair over `u` like Dehaze's, fitted the same way with the local
/// stage in the loop, from Clarity +50/+100/−50 (one negative magnitude, so
/// [`CLARITY_FALL_Q`] is zero).
const CLARITY_RISE: [f32; TABLE_N] = [-0.0091, 0.0069, 0.0280, 0.0441, 0.0553, 0.0680, 0.0788, 0.0856, 0.0888, 0.0876, 0.0799, 0.0650, 0.0487, 0.0319, 0.0100, -0.0154, -0.0412, -0.0625, -0.0624, -0.0456, -0.0500, -0.0725, -0.0895, -0.0997, -0.0923, -0.0763, -0.0707, -0.0687, -0.0578, -0.0317, -0.0129, -0.0054, 0.0297];
const CLARITY_RISE_Q: [f32; TABLE_N] = [0.0040, 0.0052, 0.0048, 0.0051, 0.0043, 0.0011, -0.0036, -0.0087, -0.0134, -0.0160, -0.0141, -0.0089, -0.0016, 0.0075, 0.0154, 0.0211, 0.0276, 0.0381, 0.0450, 0.0369, 0.0272, 0.0242, 0.0146, 0.0036, -0.0016, -0.0048, -0.0038, -0.0019, -0.0077, -0.0227, -0.0406, -0.0672, -0.1205];
const CLARITY_FALL: [f32; TABLE_N] = [0.0015, 0.0079, 0.0126, 0.0129, 0.0103, 0.0074, 0.0042, -0.0009, -0.0108, -0.0249, -0.0389, -0.0498, -0.0571, -0.0615, -0.0645, -0.0656, -0.0636, -0.0570, -0.0429, -0.0261, -0.0149, -0.0061, -0.0032, -0.0078, -0.0050, 0.0025, 0.0080, 0.0167, 0.0251, 0.0246, 0.0243, 0.0537, 0.1202];
const CLARITY_FALL_Q: [f32; TABLE_N] = [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000];

/// Clarity's push per unit slider at the exposed value `y` for a slider at `c`.
fn clarity_row(c: f32, y: f32) -> f32 {
    let (t, q) = if c < 0.0 { (&CLARITY_FALL, &CLARITY_FALL_Q) } else { (&CLARITY_RISE, &CLARITY_RISE_Q) };
    table_at(t, y) + c.abs() * table_at(q, y)
}

/// Dehaze's push per unit slider at the exposed value `y` for a slider at `d`.
fn dehaze_row(d: f32, y: f32) -> f32 {
    let (t, q) = if d < 0.0 { (&DEHAZE_FALL, &DEHAZE_FALL_Q) } else { (&DEHAZE_RISE, &DEHAZE_RISE_Q) };
    table_at(t, y) + d.abs() * table_at(q, y)
}

pub(super) fn table_at(t: &[f32; TABLE_N], u: f32) -> f32 {
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

/// The law at one `u` without the band guard [`camera_raw_tone`] adds, for a
/// frame whose key (`log2` of its mean linear luminance) is `key`: Highlights
/// and Shadows first, on `u`, with the base taken as the pixel itself
/// ([`HsSite::diagonal`]); then Exposure; then Contrast, Whites and Blacks on
/// the exposed value; clamped to [0, 1]. What the reverse fit scores a
/// candidate with (it scores the knot model without its limiter too,
/// `fit::tone`).
pub(crate) fn camera_raw_response_keyed(ev: f32, s: [f32; 5], u: f32, key: f32) -> f32 {
    let u1 = (u + hs_push(s[1], s[2], HsSite::diagonal(u, key), u)).clamp(0.0, 1.0);
    let y = camera_raw_exposure(u1, ev);
    let push: f32 = [0usize, 3, 4].iter().map(|&k| s[k] * camera_raw_row(k, s[k], y)).sum();
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
fn camera_raw_tone(ev: f32, s: [f32; 5], local: Local) -> Vec<f32> {
    const KEEP: f32 = 0.01;
    let mut out = Vec::with_capacity(LUT_N);
    let (mut prev_y, mut prev) = (0.0f32, 0.0f32);
    for i in 0..LUT_N {
        let u = i as f32 / (LUT_N - 1) as f32;
        // Highlights and Shadows on `u` before the exposure (module head),
        // on the diagonal: a LUT has no base layer to read.
        let u1 = (u + hs_push(s[1], s[2], HsSite::diagonal(u, HS_KEY_NOMINAL), u)).clamp(0.0, 1.0);
        let y = camera_raw_exposure(u1, ev);
        let push: f32 =
            [0usize, 3, 4].iter().map(|&k| s[k] * camera_raw_row(k, s[k], y)).sum::<f32>()
                + local.dehaze * dehaze_row(local.dehaze, y)
                + local.clarity * clarity_row(local.clarity, y);
        let mut v = (y + push).clamp(0.0, 1.0);
        // The floor is a share of the plain exposed curve's rise: Highlights
        // and Shadows have moved `y` itself, and on the diagonal at their
        // extremes they can turn it back.
        let y_ref = camera_raw_exposure(u, ev);
        if i > 0 {
            v = v.max((prev + KEEP * (y_ref - prev_y)).min(1.0));
        }
        out.push(v);
        (prev_y, prev) = (y_ref, v);
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
pub(super) fn camera_raw_slider_pixel(rgb: [f32; 3], pass: &SliderPass) -> [f32; 3] {
    let l_in = luma601(&rgb);
    let a = camera_raw_channels(rgb, &pass.lut);
    let target = sample_lut(&pass.lut, l_in);
    let la = luma601(&a);
    if la <= 1e-6 {
        return [target; 3];
    }
    let k = target / la;
    let mut out = [a[0] * k, a[1] * k, a[2] * k];
    if let Some(c) = &pass.chroma {
        // Dehaze's chroma, about the luma so the lightness stays the tone's.
        let g = sample_lut(c, l_in);
        for v in &mut out {
            *v = target + (*v - target) * g;
        }
    }
    out.map(|v| v.clamp(0.0, 1.0))
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

/// Dehaze and Clarity's tone, each in −1..1: the two sliders that ride the
/// slider pass besides the five.
#[derive(Clone, Copy, Default)]
pub(crate) struct Local {
    pub(crate) dehaze: f32,
    pub(crate) clarity: f32,
}

/// The five sliders, Dehaze and Clarity's tone alone over the profile stage's
/// output, `x → u → the sliders → x`, at 0 EV (the develop applies exposure
/// before it): `None` when all seven are 0. Dehaze and Clarity ride here, not
/// in [`camera_raw_tone_lut`]: like the engine law's dehaze and clarity
/// stages they are no part of the tone the reverse fit solves.
pub(super) fn camera_raw_slider_lut(sliders: [f32; 5], local: Local) -> Option<SliderPass> {
    let dehaze = local.dehaze;
    (dehaze != 0.0 || local.clarity != 0.0 || sliders.iter().any(|&v| v != 0.0)).then(|| {
        let tone = camera_raw_tone(0.0, sliders, local);
        let lut = (0..LUT_N)
            .map(|i| camera_raw_start(sample_lut(&tone, camera_raw_unstart(i as f32 / (LUT_N - 1) as f32))))
            .collect();
        let chroma = (dehaze != 0.0).then(|| {
            (0..LUT_N)
                .map(|i| {
                    let u = camera_raw_unstart(i as f32 / (LUT_N - 1) as f32);
                    (dehaze * table_at(&DEHAZE_CHROMA, u)).exp()
                })
                .collect()
        });
        SliderPass { lut, chroma }
    })
}

/// The slider pass: the tone LUT over the profile stage's output and, under
/// Dehaze, the chroma gain over the same axis (`None` without Dehaze).
pub(crate) struct SliderPass {
    pub(crate) lut: Vec<f32>,
    pub(crate) chroma: Option<Vec<f32>>,
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
    let tone = camera_raw_tone(ev, sliders, Local::default());
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

// ---------------------------------------------------------------------------
// Highlights and Shadows: Camera Raw's local, frame-keyed law.
// ---------------------------------------------------------------------------

/// The spatial sigma of the base layer Highlights and Shadows read, in film
/// pixels (the module head: measured).
pub(crate) const HS_SIGMA_S: f32 = 400.0;

/// The second, coarser base Highlights reads besides [`HS_SIGMA_S`]'s, in
/// film pixels (module head).
pub(crate) const HS_SIGMA_S2: f32 = 1600.0;

/// Both bases' range sigma, in stops: across an edge of more than about this
/// many stops the two sides keep bases of their own.
pub(crate) const HS_SIGMA_R: f32 = 1.0;

/// The floor under linear luminance before the log, relative to white.
const HS_FLOOR: f32 = 1e-4;

/// The key a tone LUT assumes when it has no frame to read (`log2` of the
/// mean linear luminance): the median of the seven calibration frames'.
pub(crate) const HS_KEY_NOMINAL: f32 = -3.94;

/// The `z` grid: `HS_Z0 + i · HS_ZSTEP`, `i < HS_ZN`.
const HS_Z0: f32 = -10.0;
const HS_ZSTEP: f32 = 0.5;
const HS_ZN: usize = 29;
/// The `u` grid of [`HS_SH`]: `j / (HS_UN − 1)`, `j < HS_UN`.
const HS_UN: usize = 17;
/// The coarse base's grid of [`HS_HL2`]: `HS_Z0 + i · HS_Z2STEP`, `i < HS_Z2N`.
const HS_Z2STEP: f32 = 0.4375;
const HS_Z2N: usize = 33;

/// Shadows' push per unit slider over `(z, u)` (`z = log2(base) − key` on
/// the [`HS_SIGMA_S`] base, `u` the pixel's own value), row-major in `z`:
/// `[T, Q]`; a slider at `s > 0` adds `s · (T + s · Q)` to `u`. Fitted
/// pooled over seven frames at three magnitudes each, then with the engine
/// in the loop (module head).
const HS_SH: [[f32; HS_ZN * HS_UN]; 2] = [
    [-0.03582, 0.07911, 0.19479, 0.30393, 0.39542, 0.46265, 0.50567, 0.52844, 0.53578, 0.53175, 0.51929, 0.50050, 0.47699, 0.45014, 0.42117, 0.39110, 0.36066, -0.03592, 0.08632, 0.20418, 0.30664, 0.38635, 0.44254, 0.47816, 0.49715, 0.50312, 0.49904, 0.48720, 0.46943, 0.44732, 0.42225, 0.39540, 0.36766, 0.33963, -0.03601, 0.09354, 0.21357, 0.30934, 0.37727, 0.42243, 0.45066, 0.46585, 0.47046, 0.46633, 0.45511, 0.43837, 0.41766, 0.39437, 0.36963, 0.34422, 0.31860, -0.03648, 0.10478, 0.22488, 0.30978, 0.36488, 0.40036, 0.42274, 0.43493, 0.43835, 0.43405, 0.42326, 0.40742, 0.38807, 0.36659, 0.34400, 0.32095, 0.29777, -0.03696, 0.11602, 0.23619, 0.31021, 0.35249, 0.37829, 0.39481, 0.40401, 0.40623, 0.40178, 0.39141, 0.37646, 0.35848, 0.33881, 0.31838, 0.29768, 0.27693, -0.03342, 0.13791, 0.24574, 0.30182, 0.33305, 0.35330, 0.36676, 0.37414, 0.37534, 0.37039, 0.36007, 0.34581, 0.32919, 0.31141, 0.29320, 0.27489, 0.25657, -0.02989, 0.15981, 0.25528, 0.29342, 0.31361, 0.32830, 0.33870, 0.34426, 0.34444, 0.33900, 0.32872, 0.31516, 0.29990, 0.28401, 0.26803, 0.25210, 0.23621, -0.01010, 0.18855, 0.24552, 0.26918, 0.28745, 0.30201, 0.31161, 0.31615, 0.31517, 0.30869, 0.29803, 0.28509, 0.27132, 0.25747, 0.24376, 0.23017, 0.21664, 0.00969, 0.21730, 0.23576, 0.24494, 0.26129, 0.27573, 0.28452, 0.28804, 0.28590, 0.27837, 0.26734, 0.25503, 0.24275, 0.23093, 0.21949, 0.20825, 0.19707, 0.02391, 0.16784, 0.18790, 0.21130, 0.23436, 0.25069, 0.25948, 0.26222, 0.25819, 0.24891, 0.23745, 0.22609, 0.21560, 0.20590, 0.19665, 0.18760, 0.17859, 0.03814, 0.11839, 0.14005, 0.17766, 0.20742, 0.22564, 0.23444, 0.23641, 0.23048, 0.21944, 0.20756, 0.19715, 0.18846, 0.18088, 0.17382, 0.16695, 0.16011, 0.03689, 0.10246, 0.11701, 0.15383, 0.18298, 0.20231, 0.21293, 0.21311, 0.20344, 0.19046, 0.17904, 0.17035, 0.16369, 0.15809, 0.15293, 0.14793, 0.14297, 0.03563, 0.08654, 0.09398, 0.13001, 0.15853, 0.17897, 0.19143, 0.18981, 0.17639, 0.16148, 0.15052, 0.14355, 0.13892, 0.13530, 0.13205, 0.12892, 0.12583, 0.02547, 0.07338, 0.09171, 0.12515, 0.13695, 0.15757, 0.17678, 0.16732, 0.14838, 0.13361, 0.12497, 0.12031, 0.11747, 0.11534, 0.11352, 0.11187, 0.11028, 0.01531, 0.06023, 0.08944, 0.12030, 0.11537, 0.13616, 0.16212, 0.14482, 0.12036, 0.10573, 0.09942, 0.09706, 0.09602, 0.09538, 0.09500, 0.09481, 0.09473, 0.02059, 0.04720, 0.07252, 0.10000, 0.09923, 0.12000, 0.15345, 0.11742, 0.09216, 0.08226, 0.07932, 0.07864, 0.07849, 0.07865, 0.07917, 0.08000, 0.08098, 0.02588, 0.03417, 0.05560, 0.07969, 0.08308, 0.10385, 0.14478, 0.09001, 0.06395, 0.05879, 0.05921, 0.06021, 0.06096, 0.06191, 0.06334, 0.06520, 0.06724, 0.00933, 0.02911, 0.04269, 0.05830, 0.05240, 0.07899, 0.09197, 0.06149, 0.04563, 0.04446, 0.04614, 0.04699, 0.04759, 0.04871, 0.05057, 0.05293, 0.05547, -0.00721, 0.02405, 0.02979, 0.03692, 0.02171, 0.05414, 0.03916, 0.03298, 0.02730, 0.03014, 0.03306, 0.03378, 0.03422, 0.03552, 0.03779, 0.04067, 0.04370, -0.00951, 0.01670, 0.02350, 0.02090, 0.01724, 0.02554, 0.02620, 0.02375, 0.02205, 0.02454, 0.02650, 0.02555, 0.02530, 0.02639, 0.02854, 0.03122, 0.03399, -0.01180, 0.00934, 0.01722, 0.00488, 0.01278, -0.00306, 0.01324, 0.01453, 0.01679, 0.01895, 0.01995, 0.01733, 0.01638, 0.01726, 0.01928, 0.02176, 0.02428, -0.01010, 0.00351, 0.00499, 0.00996, 0.02082, 0.01170, 0.01482, 0.01153, 0.01251, 0.01389, 0.01555, 0.01423, 0.01270, 0.01295, 0.01402, 0.01523, 0.01662, -0.00840, -0.00233, -0.00725, 0.01504, 0.02887, 0.02647, 0.01641, 0.00853, 0.00823, 0.00883, 0.01115, 0.01114, 0.00901, 0.00863, 0.00877, 0.00871, 0.00896, -0.00632, -0.00008, 0.00317, 0.02516, 0.02634, 0.02248, 0.01829, 0.01458, 0.01200, 0.01235, 0.00909, 0.01232, 0.01073, 0.00950, 0.00789, 0.00503, 0.00333, -0.00424, 0.00216, 0.01358, 0.03528, 0.02380, 0.01850, 0.02018, 0.02063, 0.01576, 0.01588, 0.00702, 0.01351, 0.01245, 0.01036, 0.00700, 0.00135, -0.00229, 0.00009, 0.00850, 0.02040, 0.03531, 0.02666, 0.02092, 0.01823, 0.01735, 0.01334, 0.01421, 0.01135, 0.01297, 0.00909, 0.00628, 0.00515, -0.00071, -0.00604, 0.00443, 0.01484, 0.02722, 0.03535, 0.02951, 0.02334, 0.01627, 0.01407, 0.01091, 0.01255, 0.01568, 0.01242, 0.00573, 0.00221, 0.00329, -0.00276, -0.00980, 0.01038, 0.02119, 0.03137, 0.03618, 0.03181, 0.02534, 0.01727, 0.01172, 0.00432, 0.00127, 0.00796, 0.00938, 0.00651, 0.00463, 0.00270, -0.00450, -0.01287, 0.01633, 0.02754, 0.03552, 0.03701, 0.03412, 0.02734, 0.01828, 0.00936, -0.00227, -0.01000, 0.00024, 0.00634, 0.00729, 0.00705, 0.00211, -0.00623, -0.01594],
    [0.04538, 0.06020, 0.07523, 0.08915, 0.09974, 0.10552, 0.10618, 0.10235, 0.09507, 0.08540, 0.07423, 0.06219, 0.04969, 0.03697, 0.02417, 0.01136, -0.00146, 0.04342, 0.06028, 0.07630, 0.08937, 0.09793, 0.10159, 0.10073, 0.09617, 0.08884, 0.07960, 0.06912, 0.05789, 0.04624, 0.03439, 0.02247, 0.01054, -0.00138, 0.04146, 0.06037, 0.07737, 0.08959, 0.09612, 0.09766, 0.09528, 0.08999, 0.08261, 0.07379, 0.06400, 0.05358, 0.04278, 0.03181, 0.02077, 0.00973, -0.00130, 0.03938, 0.06122, 0.07885, 0.08944, 0.09367, 0.09324, 0.08959, 0.08376, 0.07645, 0.06809, 0.05897, 0.04933, 0.03936, 0.02925, 0.01909, 0.00893, -0.00122, 0.03730, 0.06207, 0.08032, 0.08929, 0.09122, 0.08882, 0.08390, 0.07754, 0.07028, 0.06239, 0.05395, 0.04508, 0.03595, 0.02669, 0.01741, 0.00813, -0.00114, 0.03591, 0.06491, 0.08158, 0.08757, 0.08726, 0.08344, 0.07779, 0.07127, 0.06431, 0.05692, 0.04912, 0.04097, 0.03263, 0.02420, 0.01577, 0.00735, -0.00107, 0.03452, 0.06775, 0.08285, 0.08586, 0.08330, 0.07806, 0.07167, 0.06501, 0.05833, 0.05146, 0.04429, 0.03686, 0.02930, 0.02172, 0.01414, 0.00657, -0.00100, 0.03570, 0.07170, 0.08080, 0.08110, 0.07739, 0.07160, 0.06515, 0.05887, 0.05271, 0.04636, 0.03974, 0.03296, 0.02616, 0.01937, 0.01259, 0.00583, -0.00094, 0.03689, 0.07565, 0.07874, 0.07634, 0.07148, 0.06513, 0.05864, 0.05274, 0.04709, 0.04125, 0.03518, 0.02906, 0.02301, 0.01702, 0.01105, 0.00508, -0.00089, 0.02309, 0.06311, 0.07000, 0.06886, 0.06384, 0.05775, 0.05206, 0.04705, 0.04197, 0.03651, 0.03092, 0.02545, 0.02012, 0.01488, 0.00965, 0.00440, -0.00086, 0.00929, 0.05056, 0.06125, 0.06139, 0.05619, 0.05038, 0.04548, 0.04137, 0.03684, 0.03177, 0.02667, 0.02183, 0.01724, 0.01274, 0.00824, 0.00371, -0.00084, -0.01258, 0.04216, 0.05598, 0.05252, 0.04680, 0.04255, 0.03968, 0.03652, 0.03213, 0.02729, 0.02274, 0.01862, 0.01474, 0.01089, 0.00701, 0.00309, -0.00084, -0.03445, 0.03375, 0.05071, 0.04366, 0.03740, 0.03473, 0.03388, 0.03166, 0.02742, 0.02281, 0.01882, 0.01541, 0.01223, 0.00903, 0.00577, 0.00246, -0.00085, -0.01381, 0.02127, 0.04167, 0.03498, 0.02781, 0.02775, 0.03022, 0.02746, 0.02270, 0.01858, 0.01543, 0.01278, 0.01018, 0.00748, 0.00471, 0.00191, -0.00090, 0.00682, 0.00879, 0.03262, 0.02631, 0.01822, 0.02077, 0.02657, 0.02325, 0.01798, 0.01435, 0.01204, 0.01014, 0.00813, 0.00593, 0.00365, 0.00135, -0.00094, 0.02850, -0.00205, 0.02132, 0.01828, 0.01274, 0.01662, 0.02517, 0.01832, 0.01319, 0.01086, 0.00954, 0.00818, 0.00652, 0.00467, 0.00277, 0.00087, -0.00102, 0.05018, -0.01289, 0.01002, 0.01025, 0.00726, 0.01247, 0.02378, 0.01340, 0.00839, 0.00738, 0.00703, 0.00622, 0.00492, 0.00340, 0.00188, 0.00039, -0.00109, 0.03999, -0.00764, 0.00460, 0.00556, 0.00268, 0.00889, 0.01352, 0.00815, 0.00538, 0.00544, 0.00555, 0.00483, 0.00366, 0.00242, 0.00121, 0.00002, -0.00119, 0.02981, -0.00238, -0.00082, 0.00087, -0.00189, 0.00531, 0.00327, 0.00291, 0.00237, 0.00350, 0.00406, 0.00343, 0.00239, 0.00143, 0.00054, -0.00035, -0.00128, 0.02103, -0.00064, -0.00176, -0.00128, -0.00096, 0.00065, 0.00159, 0.00148, 0.00175, 0.00281, 0.00319, 0.00231, 0.00142, 0.00079, 0.00014, -0.00060, -0.00141, 0.01226, 0.00110, -0.00271, -0.00343, -0.00003, -0.00401, -0.00009, 0.00005, 0.00113, 0.00212, 0.00231, 0.00119, 0.00045, 0.00014, -0.00026, -0.00085, -0.00153, 0.00711, 0.00024, -0.00365, -0.00193, 0.00105, -0.00152, -0.00023, -0.00032, 0.00040, 0.00061, 0.00055, 0.00022, -0.00006, -0.00004, -0.00036, -0.00099, -0.00166, 0.00196, -0.00061, -0.00459, -0.00043, 0.00213, 0.00096, -0.00038, -0.00068, -0.00034, -0.00089, -0.00121, -0.00075, -0.00058, -0.00021, -0.00046, -0.00113, -0.00178, -0.00101, -0.00133, -0.00228, 0.00170, 0.00132, 0.00008, -0.00007, 0.00038, 0.00023, -0.00030, -0.00202, -0.00092, -0.00040, -0.00011, -0.00032, -0.00118, -0.00185, -0.00399, -0.00204, 0.00003, 0.00382, 0.00051, -0.00080, 0.00024, 0.00144, 0.00079, 0.00030, -0.00283, -0.00109, -0.00021, -0.00000, -0.00018, -0.00123, -0.00191, -0.00554, -0.00236, 0.00066, 0.00359, 0.00129, 0.00011, 0.00016, 0.00087, 0.00055, 0.00060, -0.00082, -0.00047, -0.00083, -0.00118, -0.00067, -0.00129, -0.00186, -0.00710, -0.00269, 0.00129, 0.00335, 0.00207, 0.00102, 0.00009, 0.00030, 0.00031, 0.00090, 0.00120, 0.00015, -0.00146, -0.00235, -0.00115, -0.00134, -0.00180, -0.00806, -0.00307, 0.00110, 0.00318, 0.00276, 0.00186, 0.00071, 0.00020, -0.00059, -0.00081, 0.00039, 0.00011, -0.00108, -0.00174, -0.00127, -0.00145, -0.00175, -0.00902, -0.00346, 0.00090, 0.00301, 0.00346, 0.00270, 0.00133, 0.00011, -0.00150, -0.00252, -0.00042, 0.00007, -0.00071, -0.00113, -0.00140, -0.00155, -0.00169],
];

/// Highlights' push per unit slider over the fine base's `z`: `[T, Q]`; a
/// slider at `s < 0` adds `s · (T + |s| · Q)` to `u`, plus [`HS_HL2`]'s
/// terms at the coarse base's `z`.
const HS_HL: [[f32; HS_ZN]; 2] = [
    [0.01408, 0.01242, 0.01077, 0.00911, 0.00745, 0.00579, 0.00413, 0.00317, 0.00600, 0.00592, 0.00867, 0.00788, 0.00875, 0.00965, 0.01359, 0.01904, 0.02227, 0.02301, 0.03602, 0.06433, 0.09159, 0.09091, 0.10090, 0.11198, 0.16821, 0.20607, 0.34311, 0.46159, 0.58007],
    [0.00424, 0.00369, 0.00314, 0.00258, 0.00201, 0.00142, 0.00079, 0.00017, 0.00014, -0.00056, -0.00104, -0.00107, -0.00140, -0.00117, -0.00047, -0.00063, -0.00057, -0.00016, -0.00053, -0.00086, -0.00406, -0.00862, -0.00727, -0.00168, -0.00795, -0.04251, -0.10489, -0.20374, -0.30240],
];

/// Highlights' push per unit slider over the coarse base's `z`
/// ([`HS_SIGMA_S2`]), `[T, Q]`, added to [`HS_HL`]'s.
const HS_HL2: [[f32; HS_Z2N]; 2] = [
    [-0.00439, -0.00431, -0.00423, -0.00415, -0.00407, -0.00399, -0.00392, -0.00384, -0.00376, -0.00368, -0.00360, -0.00222, -0.00436, -0.00404, -0.00319, 0.00006, -0.00000, -0.00237, -0.00504, -0.00799, 0.00033, -0.01812, -0.01410, -0.03288, -0.00990, 0.03956, 0.08245, 0.15571, 0.19379, 0.00607, -0.13470, -0.27547, -0.41625],
    [-0.00026, -0.00034, -0.00042, -0.00049, -0.00057, -0.00064, -0.00071, -0.00077, -0.00083, -0.00088, -0.00091, -0.00075, 0.00016, 0.00048, 0.00076, 0.00054, -0.00005, -0.00088, -0.00074, -0.00053, -0.00038, -0.00115, -0.00186, -0.00209, 0.00062, -0.00255, -0.01542, -0.02968, -0.00885, 0.02895, 0.26734, 0.50450, 0.74118],
];

fn line_at(t: &[f32], x: f32, x0: f32, step: f32) -> f32 {
    let p = ((x - x0) / step).clamp(0.0, (t.len() - 1) as f32);
    let i = (p.floor() as usize).min(t.len() - 2);
    t[i] + (t[i + 1] - t[i]) * (p - i as f32)
}

/// [`HS_SH`] row `r` at `(z, u)`, bilinear, clamped to the grid.
fn sh_grid(r: usize, z: f32, u: f32) -> f32 {
    let t = &HS_SH[r];
    let p = ((z - HS_Z0) / HS_ZSTEP).clamp(0.0, (HS_ZN - 1) as f32);
    let q = (u * (HS_UN - 1) as f32).clamp(0.0, (HS_UN - 1) as f32);
    let (i, j) = ((p.floor() as usize).min(HS_ZN - 2), (q.floor() as usize).min(HS_UN - 2));
    let (fi, fj) = (p - i as f32, q - j as f32);
    let at = |a: usize, b: usize| t[a * HS_UN + b];
    let lo = at(i, j) + (at(i, j + 1) - at(i, j)) * fj;
    let hi = at(i + 1, j) + (at(i + 1, j + 1) - at(i + 1, j)) * fj;
    lo + (hi - lo) * fi
}

/// Where a pixel sits for Highlights and Shadows: `z` on the fine base,
/// `z2` on the coarse one (both `log2(base) − key`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HsSite {
    pub(crate) z: f32,
    pub(crate) z2: f32,
}

impl HsSite {
    /// The site a pointwise tone LUT assumes at `u`: both bases are the
    /// pixel itself, the frame's key `key` ([`HS_KEY_NOMINAL`] without a
    /// frame).
    pub(crate) fn diagonal(u: f32, key: f32) -> Self {
        let z = (srgb_to_linear(u) + HS_FLOOR).log2() - key;
        Self { z, z2: z }
    }
}

/// The push per unit slider of Highlights (`k = 1`) or Shadows (`k = 2`) at
/// `s`, for a pixel at `u` at `site`: the push is `s · row`. The measured
/// signs (Highlights below 0, Shadows above) follow the local law; the other
/// two keep their pointwise tables ([`camera_raw_row`]) at the pixel's own
/// `u`.
pub(crate) fn hs_row(k: usize, s: f32, site: HsSite, u: f32) -> f32 {
    let m = s.abs();
    if k == 2 && s > 0.0 {
        sh_grid(0, site.z, u) + m * sh_grid(1, site.z, u)
    } else if k == 1 && s < 0.0 {
        line_at(&HS_HL[0], site.z, HS_Z0, HS_ZSTEP)
            + m * line_at(&HS_HL[1], site.z, HS_Z0, HS_ZSTEP)
            + line_at(&HS_HL2[0], site.z2, HS_Z0, HS_Z2STEP)
            + m * line_at(&HS_HL2[1], site.z2, HS_Z0, HS_Z2STEP)
    } else {
        camera_raw_row(k, s, u)
    }
}

/// Highlights `hl` and Shadows `sh` (each −1..1) at one pixel: how far `u`
/// moves.
pub(crate) fn hs_push(hl: f32, sh: f32, site: HsSite, u: f32) -> f32 {
    let mut p = 0.0;
    if hl != 0.0 {
        p += hl * hs_row(1, hl, site, u);
    }
    if sh != 0.0 {
        p += sh * hs_row(2, sh, site, u);
    }
    p
}

/// Every pixel's [`HsSite`] on a profile-stage frame: each base is the
/// edge-aware ([`bilateral_base`]) smoothing of `log2` linear luminance at
/// its film-pixel sigma and [`HS_SIGMA_R`] stops, the key `log2` of the
/// frame's mean linear luminance. `coarse = false` skips the coarse base
/// (only Highlights reads it) and copies the fine one.
pub(crate) fn hs_field(data: &[[f32; 3]], w: usize, h: usize, film: FilmScale, coarse: bool) -> Vec<HsSite> {
    let y: Vec<f32> = data.par_iter().map(|px| srgb_to_linear(camera_raw_unstart(luma601(px)))).collect();
    let n = y.len().max(1);
    let mean = y.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let key = (mean as f32).max(HS_FLOOR).log2();
    let lg: Vec<f32> = y.par_iter().map(|&v| (v + HS_FLOOR).log2()).collect();
    let fine = bilateral_base(&lg, w, h, film.raster_px(HS_SIGMA_S), HS_SIGMA_R);
    let wide = coarse.then(|| bilateral_base(&lg, w, h, film.raster_px(HS_SIGMA_S2), HS_SIGMA_R));
    (0..fine.len())
        .into_par_iter()
        .map(|i| HsSite { z: fine[i] - key, z2: wide.as_ref().map_or(fine[i], |c| c[i]) - key })
        .collect()
}

/// A piecewise-linear bilateral filter (Durand and Dorsey): the plane
/// box-averaged onto a grid a third of `sigma_s` apart, one Gaussian-weighted
/// mean per range level half a `sigma_r` apart, each read back bilinearly and
/// between the two levels that bracket the pixel's own value.
pub(crate) fn bilateral_base(i: &[f32], w: usize, h: usize, sigma_s: f32, sigma_r: f32) -> Vec<f32> {
    if w == 0 || h == 0 || i.len() != w * h {
        return i.to_vec();
    }
    let (lo, hi) = i.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
    if hi - lo < 1e-6 {
        return i.to_vec();
    }
    let f = ((sigma_s / 3.0).floor() as usize).max(1);
    let (cw, ch) = (w.div_ceil(f), h.div_ceil(f));
    let mut coarse = vec![0.0f32; cw * ch];
    let mut count = vec![0.0f32; cw * ch];
    for y in 0..h {
        for x in 0..w {
            let c = (y / f) * cw + x / f;
            coarse[c] += i[y * w + x];
            count[c] += 1.0;
        }
    }
    for (c, n) in coarse.iter_mut().zip(&count) {
        *c /= n.max(1.0);
    }
    let levels = (((hi - lo) / (sigma_r * 0.5)).ceil() as usize + 1).clamp(8, 64);
    let sigma_c = sigma_s / f as f32;
    let planes: Vec<Vec<f32>> = (0..levels)
        .into_par_iter()
        .map(|j| {
            let k = lo + (hi - lo) * j as f32 / (levels - 1) as f32;
            let wgt: Vec<f32> = coarse.iter().map(|&v| (-0.5 * ((v - k) / sigma_r).powi(2)).exp()).collect();
            let num: Vec<f32> = coarse.iter().zip(&wgt).map(|(v, g)| v * g).collect();
            let num = gauss_blur_plane(&num, cw, ch, sigma_c);
            let den = gauss_blur_plane(&wgt, cw, ch, sigma_c);
            num.iter().zip(&den).map(|(a, b)| if *b > 1e-12 { a / b } else { k }).collect()
        })
        .collect();
    let read = |p: &[f32], gx: f32, gy: f32| {
        let x0 = (gx.floor().max(0.0) as usize).min(cw - 1);
        let y0 = (gy.floor().max(0.0) as usize).min(ch - 1);
        let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
        let (fx, fy) = ((gx - x0 as f32).clamp(0.0, 1.0), (gy - y0 as f32).clamp(0.0, 1.0));
        let top = p[y0 * cw + x0] + (p[y0 * cw + x1] - p[y0 * cw + x0]) * fx;
        let bot = p[y1 * cw + x0] + (p[y1 * cw + x1] - p[y1 * cw + x0]) * fx;
        top + (bot - top) * fy
    };
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let gy = (y as f32 + 0.5) / f as f32 - 0.5;
        for (x, o) in row.iter_mut().enumerate() {
            let gx = (x as f32 + 0.5) / f as f32 - 0.5;
            let v = i[y * w + x];
            let t = ((v - lo) / (hi - lo) * (levels - 1) as f32).clamp(0.0, (levels - 1) as f32 - 1e-4);
            let j = t.floor() as usize;
            let a = read(&planes[j], gx, gy);
            let b = read(&planes[j + 1], gx, gy);
            *o = a + (b - a) * (t - j as f32);
        }
    });
    out
}

/// The offset under the channels' ratio in [`hs_pixel`], in the profile
/// stage's encoding: every channel scales as `(c + floor) · k − floor`, so a
/// near-black pixel's colour is not blown up with its lift. Measured on
/// Shadows +100 over a frame of large near-black areas: neutral b* 21.8 too
/// blue with no floor, 3.9 at 0.01, 1.2 at 0.03, 3.5 yellow at 0.06; the
/// warm tones of a daylight frame within 0.4 b* at all four.
const HS_CHROMA_FLOOR: f32 = 0.03;

/// One pixel through Highlights and Shadows: its lightness moved by
/// [`hs_push`] in `u`, its colour kept (every channel scaled by one ratio
/// over [`HS_CHROMA_FLOOR`]; module head: measured against the per-channel
/// rule).
pub(crate) fn hs_pixel(px: [f32; 3], hl: f32, sh: f32, site: HsSite) -> [f32; 3] {
    hs_pixel_scaled(px, hl, sh, site, [1.0, 1.0])
}

/// [`hs_pixel`] with each slider's push scaled (`[highlights, shadows]`):
/// a mask's own Highlights and Shadows (`mask_cr`).
pub(crate) fn hs_pixel_scaled(px: [f32; 3], hl: f32, sh: f32, site: HsSite, gain: [f32; 2]) -> [f32; 3] {
    let l = luma601(&px);
    let u = camera_raw_unstart(l);
    let mut push = 0.0;
    if hl != 0.0 {
        push += gain[0] * hl * hs_row(1, hl, site, u);
    }
    if sh != 0.0 {
        push += gain[1] * sh * hs_row(2, sh, site, u);
    }
    let target = camera_raw_start((u + push).clamp(0.0, 1.0));
    // Highlights shape the hue by Adobe's channel rule (`camera_raw_channels`)
    // with this pixel's own curve, as the other sliders do; Shadows keep it.
    let shaped = if hl < 0.0 {
        let curve = |v: f32| {
            let uv = camera_raw_unstart(v);
            camera_raw_start((uv + gain[0] * hl * hs_row(1, hl, site, uv)).clamp(0.0, 1.0))
        };
        channel_rule(px, curve)
    } else {
        px
    };
    let k = (target + HS_CHROMA_FLOOR) / (luma601(&shaped) + HS_CHROMA_FLOOR);
    shaped.map(|c| ((c + HS_CHROMA_FLOOR) * k - HS_CHROMA_FLOOR).clamp(0.0, 1.0))
}

/// [`camera_raw_channels`] with a curve given as a function: the brightest
/// and darkest channels through it, the middle one in proportion.
fn channel_rule(rgb: [f32; 3], curve: impl Fn(f32) -> f32) -> [f32; 3] {
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
    let (a, b) = (curve(rgb[hi]), curve(rgb[lo]));
    let span = rgb[hi] - rgb[lo];
    let mut out = [0.0f32; 3];
    out[hi] = a;
    out[mid] = if span > 1e-9 { b + (a - b) * (rgb[mid] - rgb[lo]) / span } else { b };
    out[lo] = b;
    out
}

/// Highlights and Shadows over a profile-stage frame whose sites are `field`
/// ([`hs_field`]), before Exposure (Camera Raw's order, module head).
pub(crate) fn apply_camera_raw_hs(data: &mut [[f32; 3]], field: &[HsSite], hl: f32, sh: f32) {
    if hl == 0.0 && sh == 0.0 {
        return;
    }
    data.par_iter_mut().zip(field).for_each(|(px, &site)| *px = hs_pixel(*px, hl, sh, site));
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
                let t = camera_raw_tone(ev, *s, Local::default());
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

    /// Dehaze alone makes a slider pass with a chroma gain; at 0 there is no
    /// pass at all.
    #[test]
    fn dehaze_alone_makes_a_slider_pass() {
        assert!(camera_raw_slider_lut([0.0; 5], Local::default()).is_none());
        let pass = camera_raw_slider_lut([0.0; 5], Local { dehaze: 0.4, clarity: 0.0 }).expect("Dehaze is a pass");
        assert!(pass.chroma.is_some(), "with its colour");
        assert!(camera_raw_slider_lut([0.2, 0.0, 0.0, 0.0, 0.0], Local::default()).is_some_and(|p| p.chroma.is_none()));
        let clarity = camera_raw_slider_lut([0.0; 5], Local { dehaze: 0.0, clarity: 0.5 }).expect("Clarity is a pass");
        assert!(clarity.chroma.is_none(), "Clarity's tone has no colour of its own");
        let mid = LUT_N / 2;
        assert!(pass.lut[mid] < mid as f32 / (LUT_N - 1) as f32, "+40 darkens the midtones");
    }

    /// The slider pixel gives a grey the luma rule's value and keeps a
    /// colour's lightness at the luma rule's while taking the channel rule's
    /// hue.
    #[test]
    fn the_slider_pixel_takes_the_luma_lightness_and_the_channel_hue() {
        let lut: Vec<f32> = (0..LUT_N).map(|i| (i as f32 / (LUT_N - 1) as f32).powf(1.4)).collect();
        let g = camera_raw_slider_pixel([0.5; 3], &SliderPass { lut: lut.clone(), chroma: None });
        let want = sample_lut(&lut, 0.5);
        assert!(g.iter().all(|&c| (c - want).abs() < 1e-5), "a grey: {g:?} vs {want}");
        let px = [0.7f32, 0.45, 0.2];
        let out = camera_raw_slider_pixel(px, &SliderPass { lut: lut.clone(), chroma: None });
        assert!((luma601(&out) - sample_lut(&lut, luma601(&px))).abs() < 1e-4, "lightness by luma");
        let a = camera_raw_channels(px, &lut);
        let ratio = |p: [f32; 3]| [p[0] / p[1], p[2] / p[1]];
        let (ro, ra) = (ratio(out), ratio(a));
        assert!((ro[0] - ra[0]).abs() < 1e-4 && (ro[1] - ra[1]).abs() < 1e-4, "hue by channels");
    }

    /// The base is edge-aware: across a 4-stop step each side keeps its own
    /// level, inside a side the texture is smoothed away, and a flat plane
    /// comes back flat.
    ///
    /// MUTATION: drop the range weight (a plain Gaussian smears the step), or
    /// read the levels at the coarse value instead of the pixel's own.
    #[test]
    fn the_base_keeps_an_edge_and_smooths_a_side() {
        let (w, h) = (96usize, 48usize);
        let plane: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let tex = if (x + y) % 2 == 0 { 0.1 } else { -0.1 };
                if x < w / 2 { tex } else { 4.0 + tex }
            })
            .collect();
        let b = bilateral_base(&plane, w, h, 12.0, 1.0);
        for y in 0..h {
            assert!(b[y * w + 2].abs() < 0.05, "dark side at row {y}: {}", b[y * w + 2]);
            assert!((b[y * w + w - 3] - 4.0).abs() < 0.05, "bright side at row {y}: {}", b[y * w + w - 3]);
            assert!(b[y * w + w / 2 - 2] < 0.6, "the dark side next to the step: {}", b[y * w + w / 2 - 2]);
            assert!(b[y * w + w / 2 + 1] > 3.4, "the bright side next to the step: {}", b[y * w + w / 2 + 1]);
        }
        let flat = vec![-2.5f32; w * h];
        assert!(bilateral_base(&flat, w, h, 12.0, 1.0).iter().all(|&v| v == -2.5));
    }

    /// A frame of one value sits at its own key: every site's `z` is 0.
    #[test]
    fn a_uniform_frame_sits_at_its_key() {
        let data = vec![[0.4f32; 3]; 32 * 24];
        // Within the floor under the log ([`HS_FLOOR`], 1e-4 of white): a
        // few thousandths of a stop at this level.
        for s in hs_field(&data, 32, 24, FilmScale::NATIVE, true) {
            assert!(s.z.abs() < 0.01 && s.z2.abs() < 0.01, "{s:?}");
        }
    }

    /// Camera Raw's Highlights and Shadows measured on synthetic neutral
    /// frames (the calibration body's own RAW with its pixels replaced, so
    /// profile, baseline and lens are the frame's), pinned here until the
    /// law reproduces them (TECH_STACK, "Highlights and Shadows: what the
    /// synthetic frames showed"). `u` per pixel before and after one slider.
    fn hs_moves(levels: &[f32], hl: f32, sh: f32) -> Vec<f32> {
        let (w, h) = (96usize, 64usize);
        let film = FilmScale::of(Some(6336), w, h);
        let mut data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| [camera_raw_start(levels[(i % w) * levels.len() / w]); 3])
            .collect();
        let before: Vec<f32> = data.iter().map(|px| camera_raw_unstart(luma601(px))).collect();
        let field = hs_field(&data, w, h, film, true);
        apply_camera_raw_hs(&mut data, &field, hl, sh);
        data.iter().zip(&before).map(|(px, &u)| camera_raw_unstart(luma601(px)) - u).collect()
    }

    /// A uniform frame is left alone: Camera Raw 18.4 moved a uniform field
    /// by 0.0000 in `u` at every level from 0.13 to 0.78 under Highlights −60
    /// and under Shadows +60; the frame-keyed law moves it −0.03 / +0.015.
    #[test]
    #[ignore = "measured Camera Raw behaviour the frame-keyed law does not reproduce yet (TECH_STACK)"]
    fn a_uniform_frame_is_left_alone_by_highlights_and_shadows() {
        for u in [0.13f32, 0.26, 0.44, 0.60, 0.78] {
            for (hl, sh) in [(-0.6f32, 0.0f32), (0.0, 0.6)] {
                let m = hs_moves(&[u], hl, sh);
                let worst = m.iter().fold(0.0f32, |a, &d| a.max(d.abs()));
                assert!(worst < 0.003, "u {u} hl {hl} sh {sh}: moved {worst}");
            }
        }
    }

    /// A frame of two levels, half and half (u 0.13 | u 0.70): Camera Raw
    /// moves each half as one, at every distance from the edge (Highlights
    /// −60: the bright half −0.208 at the edge to −0.238 4000 film px away,
    /// the dark half −0.016 to −0.022; Shadows +60: the dark half +0.017 to
    /// +0.018, the bright half 0.000). The law: −0.062 to −0.090 and +0.078
    /// to +0.081.
    #[test]
    #[ignore = "measured Camera Raw behaviour the frame-keyed law does not reproduce yet (TECH_STACK)"]
    fn a_two_level_frame_moves_each_level_as_one() {
        let (w, h) = (96usize, 64usize);
        let side = |m: &[f32], bright: bool| {
            let v: Vec<f32> = (0..w * h).filter(|i| ((i % w) >= w / 2) == bright).map(|i| m[i]).collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        let hl = hs_moves(&[0.13, 0.70], -0.6, 0.0);
        let sh = hs_moves(&[0.13, 0.70], 0.0, 0.6);
        assert!((side(&hl, true) + 0.225).abs() < 0.03, "bright half under Highlights: {}", side(&hl, true));
        assert!((side(&hl, false) + 0.019).abs() < 0.01, "dark half under Highlights: {}", side(&hl, false));
        assert!((side(&sh, false) - 0.018).abs() < 0.01, "dark half under Shadows: {}", side(&sh, false));
        assert!(side(&sh, true).abs() < 0.005, "bright half under Shadows: {}", side(&sh, true));
    }

    /// Highlights and Shadows run before Exposure: the response is the
    /// exposure of the moved `u`, not a push on the exposed value.
    ///
    /// MUTATION: apply the two after `camera_raw_exposure`, as the five
    /// sliders' first law did.
    #[test]
    fn highlights_and_shadows_run_before_the_exposure() {
        let key = HS_KEY_NOMINAL;
        for (hl, sh) in [(-0.6f32, 0.0f32), (0.0, 0.6), (-0.4, 0.5)] {
            for i in 1..20 {
                let u = i as f32 / 20.0;
                let site = HsSite::diagonal(u, key);
                let moved = (u + hs_push(hl, sh, site, u)).clamp(0.0, 1.0);
                let want = camera_raw_exposure(moved, 1.0);
                let got = camera_raw_response_keyed(1.0, [0.0, hl, sh, 0.0, 0.0], u, key);
                assert!((got - want).abs() < 1e-5, "hl {hl} sh {sh} at {u}: {got} vs {want}");
            }
        }
    }

    /// The measured signs push the way Camera Raw does, harder where the frame
    /// says so: Shadows lift a dark site more than a bright one, Highlights
    /// pull a bright site down more than a dark one, and the same pixel value
    /// is pulled harder in a darker frame (the key).
    #[test]
    fn the_law_reads_the_site_and_the_key() {
        let u = 0.3f32;
        let at = |z: f32| HsSite { z, z2: z };
        assert!(hs_push(0.0, 0.6, at(-3.0), 0.08) > hs_push(0.0, 0.6, at(1.0), 0.08));
        assert!(hs_push(-0.6, 0.0, at(1.5), u) < hs_push(-0.6, 0.0, at(-2.0), u));
        let dark_frame = HsSite::diagonal(u, -5.5);
        let bright_frame = HsSite::diagonal(u, -2.5);
        assert!(hs_push(-0.6, 0.0, dark_frame, u) < hs_push(-0.6, 0.0, bright_frame, u));
    }

    /// A Shadows lift keeps the colour: every channel scales by one ratio
    /// over the floor, so a warm pixel keeps its channel ratios there, and a
    /// near-black pixel's colour is not blown up with its lift.
    ///
    /// MUTATION: scale by the plain luma ratio (no floor), or move each
    /// channel through the curve on its own.
    #[test]
    fn a_shadow_lift_keeps_the_colour_over_the_floor() {
        let site = HsSite { z: -3.0, z2: -3.0 };
        let f = HS_CHROMA_FLOOR;
        let warm = [0.30f32, 0.18, 0.08];
        let out = hs_pixel(warm, 0.0, 0.6, site);
        assert!(luma601(&out) > luma601(&warm) + 0.02, "lifted: {out:?}");
        let r = |p: [f32; 3]| [(p[0] + f) / (p[1] + f), (p[2] + f) / (p[1] + f)];
        let (a, b) = (r(warm), r(out));
        assert!((a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4, "{a:?} vs {b:?}");
        let near_black = [0.004f32, 0.004, 0.012];
        let lifted = hs_pixel(near_black, 0.0, 1.0, site);
        let spread = lifted[2] - lifted[0];
        assert!(spread < 3.0 * (near_black[2] - near_black[0]), "near-black blue grew to {spread}: {lifted:?}");
    }
}
