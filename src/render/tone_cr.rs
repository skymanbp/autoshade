//! The Basic panel's tone sliders on a camera-profile start (v1.17.0): Camera
//! Raw's law, measured.
//!
//! # Where it applies
//!
//! A recipe that names a camera profile ([`EditRecipe::names_profile`]) starts
//! where Lightroom starts: the profile stage (`render/profile.rs`) has already
//! put the body's baseline exposure (Camera Raw's exposure transfer at the
//! photograph's camera start, [`Ramp`]) and Adobe's default curve on the
//! frame. Camera Raw's Exposure, Contrast, Highlights, Shadows,
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
//! * **Exposure** is one transfer at the TOTAL exposure, the start's baseline
//!   plus the slider ([`Exposure`]): the frame's light is taken back under
//!   the transfer it was started at, multiplied, and put through the
//!   transfer at the sum. The transfer is measured on Camera Raw's renders
//!   at 54 total exposures (`tone_cr/exposure.rs`); it is pointwise, and
//!   BaselineExposure +1 renders exactly as the slider at +1.
//! * **The channel rule.** Exposure meets a pixel in the profile's reference
//!   primaries (ProPhoto's), linear: the brightest and darkest channels
//!   through the transfer, the middle one at its LINEAR place between them
//!   ([`Ramp::transfer_rgb`]; ΔE median 0.10–0.14 on 288 patches at every
//!   exposure, against 38–96 for per-channel, by the brightest channel or by
//!   luminance). The tone curves take the same rule on the encoded values
//!   ([`camera_raw_channels`]). Measured on the earlier ramp: the luma ratio the engine law
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
//! # Highlights and Shadows: before Exposure, keyed to the frame
//!
//! * **Order.** Composing Camera Raw's own pointwise maps on two frames,
//!   Highlights and Shadows go BEFORE Exposure (mean |ΔL| 0.23–0.42 that way,
//!   1.17–3.68 the other) and before Contrast and Blacks too (0.11–0.22
//!   against 0.30–2.48). They are the first pass ([`apply_camera_raw_hs`]).
//! * **Shadows: a linear-light gain set by the frame's spread.** Measured on
//!   19 frames (the seven calibration frames and twelve census frames of
//!   other scenes, each rendered flat and at Shadows +60 by Camera Raw 18.4
//!   at sensor size, 16 bit, read as `log2` linear luminance at the curve's
//!   input on a map 768 cells across): the push in stops at a cell is
//!   `s · spread · T(b)`, `b` the cell minus the map's mean, `spread` the
//!   map's standard deviation, `T` one monotone shape shared by every frame
//!   ([`HS_SH_LIFT`]), no constant fitted per frame. Held out one frame at a
//!   time, the law explains 0.86 of the push over all 19 (the worst a
//!   portrait-orientation night frame); a shared shape with a free amplitude
//!   per frame reaches 0.93, and the amplitude follows the spread (r 0.95).
//!   On two frames rendered at +30/+60/+100 the push is linear in the slider
//!   (0.50 : 1 : 1.64 and 0.51 : 1 : 1.62). Below 0 the same form reads its
//!   own table ([`HS_SH_DEEPEN`], four frames at −60, held out 0.62): the
//!   signs agree through the middle, −60 darkens the deepest levels further
//!   than +60 lifts them. The residual left at full resolution moves 1-stop
//!   band means by 0.24 L at most: texture, left out. Measured in stops on
//!   a map, the law is unmoved by an exposure gain before it (both `b` and
//!   the spread are differences of `log2`); clipped pixels aside, so is its
//!   key.
//! * **Highlights: local, keyed to the mean.** The push at a pixel follows
//!   the region it sits in, over 200–400 film pixels next to a bright edge;
//!   it reads an edge-aware base ([`bilateral_base`], [`HS_SIGMA_S`] /
//!   [`HS_SIGMA_S2`] film pixels, [`HS_SIGMA_R`] stops) of `log2` linear
//!   luminance against the frame's mean linear luminance (`z = log2(base) −
//!   log2(mean)`; Highlights −60 at `u` 0.3 is 0.022 on a daylight street,
//!   0.142 on a dark dusk frame), the fine base plus the coarse one
//!   ([`HS_HL`], [`HS_HL2`]), fitted pooled over the seven calibration
//!   frames at −30/−60/−100 (6144 px, 768 px in `u`) with the engine in the
//!   loop; above 0 it keeps the pointwise table at the pixel's `u`. Its
//!   amplitude on the 19 frames has no frame statistic (the best, the mean,
//!   holds out at 0.72): dark frames also take a flat offset over their
//!   deepest levels (−0.27 stops on a night frame, −0.30 on another, 0 on
//!   bright frames) that no statistic tried predicts (best held out 0.43).
//! * **Colour.** Shadows keep the colour: every channel by one ratio over a
//!   floor ([`HS_CHROMA_FLOOR`]); per channel left the warm tones 4.75 b*
//!   short. Highlights take the hue of Adobe's channel rule with the pixel's
//!   own curve, as the other sliders do (the street's bright band 5.14 →
//!   0.80 b*).

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

/// Camera Raw's exposure transfer, measured (`tone_cr/exposure.rs`) from the
/// table of its renders (`tone_cr/ramp_table.rs`).
mod exposure;
mod ramp_table;
pub(crate) use exposure::{headroom_of, Exposure, Ramp};

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

/// Exposure on `u` (a grey after the start's transfer, under the sRGB
/// transfer): [`Exposure::grey`]; `None` is 0 EV.
pub(crate) fn camera_raw_exposure(u: f32, e: Option<&Exposure>) -> f32 {
    match e {
        Some(e) => linear_to_srgb(e.grey(srgb_to_linear(u))),
        None => u,
    }
}

/// The law at one `u` without the band guard [`camera_raw_tone`] adds, for a
/// frame whose statistics are `key` ([`HsKey`]): Highlights
/// and Shadows first, on `u`, with the base taken as the pixel itself
/// ([`HsSite::diagonal`]); then Exposure; then Contrast, Whites and Blacks on
/// the exposed value; clamped to [0, 1]. What the reverse fit scores a
/// candidate with (it scores the knot model without its limiter too,
/// `fit::tone`).
pub(crate) fn camera_raw_response_keyed(e: Option<&Exposure>, s: [f32; 5], u: f32, key: HsKey) -> f32 {
    let u1 = (u + hs_push(s[1], s[2], HsSite::diagonal(u, key), u)).clamp(0.0, 1.0);
    let y = camera_raw_exposure(u1, e);
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
fn camera_raw_tone(e: Option<&Exposure>, s: [f32; 5], local: Local) -> Vec<f32> {
    const KEEP: f32 = 0.01;
    let mut out = Vec::with_capacity(LUT_N);
    let (mut prev_y, mut prev) = (0.0f32, 0.0f32);
    for i in 0..LUT_N {
        let u = i as f32 / (LUT_N - 1) as f32;
        // Highlights and Shadows on `u` before the exposure (module head),
        // on the diagonal: a LUT has no base layer to read.
        let u1 = (u + hs_push(s[1], s[2], HsSite::diagonal(u, HsKey::NOMINAL), u)).clamp(0.0, 1.0);
        let y = camera_raw_exposure(u1, e);
        let push: f32 =
            [0usize, 3, 4].iter().map(|&k| s[k] * camera_raw_row(k, s[k], y)).sum::<f32>()
                + local.dehaze * dehaze_row(local.dehaze, y)
                + local.clarity * clarity_row(local.clarity, y);
        let mut v = (y + push).clamp(0.0, 1.0);
        // The floor is a share of the plain exposed curve's rise: Highlights
        // and Shadows have moved `y` itself, and on the diagonal at their
        // extremes they can turn it back.
        let y_ref = camera_raw_exposure(u, e);
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
/// relative place between them. The develop's curve pass on a Camera Raw
/// recipe ([`camera_raw_after_lut`]) and the hue of its slider pass
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
        let tone = camera_raw_tone(None, sliders, local);
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
/// curve. `sliders` are in −1..1 with the Look's baked moves already added;
/// `ev` is the Exposure slider on a frame started at `start`.
pub(super) fn camera_raw_tone_lut(
    start: crate::recipe::CameraStart,
    ev: f32,
    sliders: [f32; 5],
    base: Option<&[f32]>,
    parametric: Option<&[f32]>,
    curve: &[f32],
) -> Vec<f32> {
    let tone = camera_raw_tone(Exposure::new(start, ev).as_ref(), sliders, Local::default());
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
/// mean linear luminance): the median of the seven calibration frames'
/// ([`HsKey::NOMINAL`]).
const HS_KEY_NOMINAL: f32 = -3.94;

/// The `z` grid: `HS_Z0 + i · HS_ZSTEP`, `i < HS_ZN`.
const HS_Z0: f32 = -10.0;
const HS_ZSTEP: f32 = 0.5;
const HS_ZN: usize = 29;
/// The coarse base's grid of [`HS_HL2`]: `HS_Z0 + i · HS_Z2STEP`, `i < HS_Z2N`.
const HS_Z2STEP: f32 = 0.4375;
const HS_Z2N: usize = 33;

/// Shadows' map: the frame's `log2` linear luminance block-averaged onto this
/// many cells across its width (square cells; a frame narrower than this
/// keeps its pixels). The scale the real-frame law was measured at (module
/// head); the push reads the map, bilinearly, not the pixel.
const HS_SH_CELLS: usize = 768;

/// The floor under linear luminance before Shadows' log, relative to white:
/// the measurement's (a 16-bit render's darkest step is 1.2e-6).
pub(crate) const HS_SH_FLOOR: f32 = 1e-6;

/// The `b` grid of [`HS_SH_LIFT`] and [`HS_SH_DEEPEN`]: `HS_SH_B0 + i ·
/// HS_SH_BSTEP`, `i < HS_SH_BN`, clamped at both ends.
const HS_SH_B0: f32 = -10.0;
const HS_SH_BSTEP: f32 = 0.25;
const HS_SH_BN: usize = 69;

/// Shadows above 0: the push in stops of linear luminance per unit slider
/// and per stop of the frame's spread, over `b` (the map at the pixel minus
/// the map's mean, [`HsSite`]): a slider at `s > 0` multiplies the pixel's
/// linear luminance by `2^(s · spread · T(b))`. The monotone shape shared by
/// the 19 corpus frames at Shadows +60 (isotonic, smoothed over half a stop;
/// module head), divided by 0.6.
const HS_SH_LIFT: [f32; HS_SH_BN] = [1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13147, 1.13146, 1.13110, 1.12666, 1.10588, 1.06696, 1.03154, 1.00729, 0.98861, 0.96846, 0.94082, 0.90612, 0.87064, 0.83557, 0.79884, 0.75949, 0.71571, 0.66832, 0.61910, 0.56704, 0.51075, 0.45130, 0.38381, 0.31107, 0.24594, 0.19361, 0.15238, 0.12019, 0.09822, 0.08370, 0.06907, 0.05555, 0.04910, 0.04779, 0.04768, 0.04758, 0.04697, 0.04493, 0.04037, 0.03438, 0.03011, 0.02801, 0.02723, 0.02707, 0.02706, 0.02706, 0.02706, 0.02706, 0.02706, 0.02706, 0.02706];

/// Shadows below 0, the same form ([`HS_SH_LIFT`]) on its own table: the
/// four frames rendered at Shadows −60, divided by −0.6. The two signs share
/// the shape through the middle; −60 darkens the deepest levels further than
/// +60 lifts them (module head).
const HS_SH_DEEPEN: [f32; HS_SH_BN] = [1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72516, 1.72494, 1.72037, 1.67676, 1.50116, 1.22371, 1.05811, 1.02128, 1.01702, 1.01133, 1.00210, 0.99650, 0.99522, 0.99467, 0.99172, 0.98186, 0.96320, 0.93879, 0.91032, 0.87702, 0.83676, 0.78945, 0.73982, 0.69447, 0.65556, 0.61857, 0.57591, 0.52323, 0.45980, 0.38541, 0.30421, 0.23338, 0.19201, 0.17806, 0.17571, 0.17551, 0.17510, 0.17323, 0.16959, 0.16445, 0.15584, 0.14150, 0.11533, 0.08008, 0.05595, 0.04804, 0.04681, 0.04672, 0.04672, 0.04672, 0.04672, 0.04672, 0.04666, 0.04593, 0.04264, 0.03722];

/// A frame's statistics for Highlights and Shadows, as a tone LUT or the
/// solve assumes them ([`HsSite::diagonal`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HsKey {
    /// `log2` of the mean linear luminance (Highlights' key).
    pub(crate) key: f32,
    /// The mean of the Shadows map ([`HS_SH_CELLS`]), in stops.
    pub(crate) log_mean: f32,
    /// The standard deviation of the Shadows map, in stops (the spread).
    pub(crate) spread: f32,
}

impl HsKey {
    /// Without a frame: the seven calibration frames' medians, key
    /// [`HS_KEY_NOMINAL`], map mean −5.88 st, spread 2.31 st (their flat
    /// renders' maps).
    pub(crate) const NOMINAL: Self = Self { key: HS_KEY_NOMINAL, log_mean: -5.88, spread: 2.31 };
}

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

/// How far `u` moves when its linear luminance is multiplied by `2^stops`,
/// in `f64` (the solve asks it at a slider of 1e-6).
fn lift_u(u: f32, stops: f32) -> f32 {
    let u = f64::from(u.clamp(0.0, 1.0));
    let lin = if u <= 0.04045 { u / 12.92 } else { ((u + 0.055) / 1.055).powf(2.4) };
    let y = lin * f64::from(stops).exp2();
    let back = if y <= 0.0031308 { y * 12.92 } else { 1.055 * y.powf(1.0 / 2.4) - 0.055 };
    (back - u) as f32
}

/// Where a pixel sits for Highlights and Shadows: `z` on the fine base,
/// `z2` on the coarse one (both `log2(base) − key`, Highlights); `b` the
/// Shadows map at the pixel minus the map's mean, `spread` the map's
/// standard deviation (both in stops, Shadows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HsSite {
    pub(crate) z: f32,
    pub(crate) z2: f32,
    pub(crate) b: f32,
    pub(crate) spread: f32,
}

impl HsSite {
    /// The site a pointwise tone LUT assumes at `u`: every base is the
    /// pixel itself, the frame's statistics `key` ([`HsKey::NOMINAL`]
    /// without a frame).
    pub(crate) fn diagonal(u: f32, key: HsKey) -> Self {
        let y = srgb_to_linear(u);
        let z = (y + HS_FLOOR).log2() - key.key;
        Self { z, z2: z, b: (y + HS_SH_FLOOR).log2() - key.log_mean, spread: key.spread }
    }
}

/// The push per unit slider of Highlights (`k = 1`) or Shadows (`k = 2`) at
/// `s`, for a pixel at `u` at `site`: the push is `s · row`. Shadows (both
/// signs) multiply the pixel's linear luminance by `2^(s · spread · T(b))`
/// ([`HS_SH_LIFT`], [`HS_SH_DEEPEN`]); Highlights below 0 follow the local,
/// keyed law ([`HS_HL`], [`HS_HL2`]); Highlights above 0 keep the pointwise
/// table ([`camera_raw_row`]) at the pixel's own `u`.
pub(crate) fn hs_row(k: usize, s: f32, site: HsSite, u: f32) -> f32 {
    let m = s.abs();
    if k == 2 && s != 0.0 {
        let t = line_at(if s > 0.0 { &HS_SH_LIFT } else { &HS_SH_DEEPEN }, site.b, HS_SH_B0, HS_SH_BSTEP);
        lift_u(u, s * site.spread * t) / s
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

/// Shadows' map of a frame of linear luminance `y` ([`HS_SH_CELLS`]): per
/// pixel the map read bilinearly minus the map's mean, and the map's
/// standard deviation, in stops.
fn shadow_map(y: &[f32], w: usize, h: usize) -> (Vec<f32>, f32) {
    if w == 0 || h == 0 || y.len() != w * h {
        return (vec![0.0; y.len()], 0.0);
    }
    let c = (w as f32 / HS_SH_CELLS as f32).max(1.0);
    let (mw, mh) = (((w as f32 / c) as usize).max(1), ((h as f32 / c) as usize).max(1));
    let edge = |k: usize, n: usize| ((k as f32 * c) as usize).min(n);
    let map: Vec<f32> = (0..mh)
        .into_par_iter()
        .flat_map_iter(|i| {
            let (y0, y1) = (edge(i, h), edge(i + 1, h).max(edge(i, h) + 1).min(h));
            (0..mw).map(move |j| {
                let (x0, x1) = (edge(j, w), edge(j + 1, w).max(edge(j, w) + 1).min(w));
                let mut sum = 0.0f64;
                for row in y0..y1 {
                    for v in &y[row * w + x0..row * w + x1] {
                        sum += f64::from((v.max(0.0) + HS_SH_FLOOR).log2());
                    }
                }
                (sum / ((y1 - y0) * (x1 - x0)).max(1) as f64) as f32
            })
        })
        .collect();
    let n = map.len() as f64;
    let mean = map.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    let var = map.iter().map(|&v| (f64::from(v) - mean).powi(2)).sum::<f64>() / n;
    let mean = mean as f32;
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(py, row)| {
        let gy = ((py as f32 + 0.5) / c - 0.5).clamp(0.0, (mh - 1) as f32);
        let (r0, fy) = ((gy.floor() as usize).min(mh - 1), gy - gy.floor());
        let r1 = (r0 + 1).min(mh - 1);
        for (px, o) in row.iter_mut().enumerate() {
            let gx = ((px as f32 + 0.5) / c - 0.5).clamp(0.0, (mw - 1) as f32);
            let (c0, fx) = ((gx.floor() as usize).min(mw - 1), gx - gx.floor());
            let c1 = (c0 + 1).min(mw - 1);
            let top = map[r0 * mw + c0] + (map[r0 * mw + c1] - map[r0 * mw + c0]) * fx;
            let bot = map[r1 * mw + c0] + (map[r1 * mw + c1] - map[r1 * mw + c0]) * fx;
            *o = top + (bot - top) * fy - mean;
        }
    });
    (out, var.sqrt() as f32)
}

/// Every pixel's [`HsSite`] on a profile-stage frame. Highlights: each base
/// is the edge-aware ([`bilateral_base`]) smoothing of `log2` linear
/// luminance at its film-pixel sigma and [`HS_SIGMA_R`] stops, the key `log2`
/// of the frame's mean linear luminance; `coarse = false` skips the coarse
/// base and copies the fine one. Shadows: the frame's map ([`shadow_map`]).
pub(crate) fn hs_field(data: &[[f32; 3]], w: usize, h: usize, film: FilmScale, coarse: bool) -> Vec<HsSite> {
    let y: Vec<f32> = data.par_iter().map(|px| srgb_to_linear(camera_raw_unstart(luma601(px)))).collect();
    let n = y.len().max(1);
    let mean = y.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let key = (mean as f32).max(HS_FLOOR).log2();
    let lg: Vec<f32> = y.par_iter().map(|&v| (v + HS_FLOOR).log2()).collect();
    let fine = bilateral_base(&lg, w, h, film.raster_px(HS_SIGMA_S), HS_SIGMA_R);
    let wide = coarse.then(|| bilateral_base(&lg, w, h, film.raster_px(HS_SIGMA_S2), HS_SIGMA_R));
    let (b, spread) = shadow_map(&y, w, h);
    (0..fine.len())
        .into_par_iter()
        .map(|i| HsSite {
            z: fine[i] - key,
            z2: wide.as_ref().map_or(fine[i], |c| c[i]) - key,
            b: b.get(i).copied().unwrap_or(0.0),
            spread,
        })
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

/// [`hs_pixel`] with each slider scaled (`[highlights, shadows]`: the
/// Highlights push in `u`, the Shadows slider): a mask's own Highlights and
/// Shadows (`mask_cr`).
pub(crate) fn hs_pixel_scaled(px: [f32; 3], hl: f32, sh: f32, site: HsSite, gain: [f32; 2]) -> [f32; 3] {
    let l = luma601(&px);
    let u = camera_raw_unstart(l);
    let mut push = 0.0;
    if hl != 0.0 {
        push += gain[0] * hl * hs_row(1, hl, site, u);
    }
    if sh != 0.0 {
        // Shadows' gain scales the slider: the push is in stops, linear in it.
        let s = gain[1] * sh;
        push += s * hs_row(2, s, site, u);
    }
    // A value above the curve's top (over-range light the camera stage left
    // for the slider Exposure, `calibrate_camera_buffer`) moves as the top
    // does, in proportion: the push read at u = 1 and the light kept.
    let target = l.max(1.0) * camera_raw_start((u + push).clamp(0.0, 1.0));
    // Highlights shape the hue by Adobe's channel rule (`camera_raw_channels`)
    // with this pixel's own curve, as the other sliders do; Shadows keep it.
    let shaped = if hl < 0.0 {
        let curve = |v: f32| {
            let uv = camera_raw_unstart(v);
            v.max(1.0) * camera_raw_start((uv + gain[0] * hl * hs_row(1, hl, site, uv)).clamp(0.0, 1.0))
        };
        channel_rule(px, curve)
    } else {
        px
    };
    let k = (target + HS_CHROMA_FLOOR) / (luma601(&shaped) + HS_CHROMA_FLOOR);
    shaped.map(|c| ((c + HS_CHROMA_FLOOR) * k - HS_CHROMA_FLOOR).max(0.0))
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

    /// A camera start the tests develop from: a body's baseline and the
    /// headroom's full stop, where a daylight frame sits.
    const START: crate::recipe::CameraStart = crate::recipe::CameraStart { ev: 0.35, headroom: 1.0, tables: 0 };

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
    /// exposure read `u` without undoing the start.
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
            let e = Exposure::new(START, ev);
            for s in &cases {
                let t = camera_raw_tone(e.as_ref(), *s, Local::default());
                assert!(t.windows(2).all(|w| w[1] >= w[0]), "monotone at ev {ev} {s:?}");
                assert!(t.iter().all(|v| (0.0..=1.0).contains(v)), "in range at ev {ev} {s:?}");
            }
        }
        let up = Exposure::new(START, 1.0).expect("a non-zero exposure");
        for i in 0..LUT_N {
            let x = i as f32 / (LUT_N - 1) as f32;
            let v = camera_raw_exposure(x, Some(&up));
            assert!(v >= x - 1e-4, "+1 EV raises {x}: {v}");
        }
        assert!(Exposure::new(START, 0.0).is_none());
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

    /// A frame of one value sits at its own key: every site's `z` and `b`
    /// are 0, its spread 0.
    #[test]
    fn a_uniform_frame_sits_at_its_key() {
        let data = vec![[0.4f32; 3]; 32 * 24];
        // Within the floor under the log ([`HS_FLOOR`], 1e-4 of white): a
        // few thousandths of a stop at this level.
        for s in hs_field(&data, 32, 24, FilmScale::NATIVE, true) {
            assert!(s.z.abs() < 0.01 && s.z2.abs() < 0.01, "{s:?}");
            assert!(s.b.abs() < 1e-4 && s.spread < 1e-4, "{s:?}");
        }
    }

    /// Shadows' map: a frame of two halves `d` stops apart has
    /// each half at ±d/2 from the map's mean and a spread of d/2, at any
    /// width (the map's cells are a share of the width, not pixels).
    ///
    /// MUTATION: take the mean of linear luminance instead of `log2`, or
    /// count the cells in pixels.
    #[test]
    fn the_shadow_map_reads_levels_in_stops_against_the_frame() {
        for w in [96usize, 1600] {
            let h = w / 2;
            let (lo, hi) = (0.02f32, 0.32f32); // four stops apart
            let y: Vec<f32> = (0..w * h).map(|i| if i % w < w / 2 { lo } else { hi }).collect();
            let (b, spread) = shadow_map(&y, w, h);
            assert!((spread - 2.0).abs() < 0.01, "w {w}: spread {spread}");
            assert!((b[h / 2 * w + 2] + 2.0).abs() < 0.01 && (b[h / 2 * w + w - 3] - 2.0).abs() < 0.01, "w {w}");
        }
    }

    /// Shadows multiply a pixel's linear luminance by `2^(s · spread · T(b))`,
    /// each sign on its own table: the push in stops scales with the frame's
    /// spread and is nothing on a frame without one, and −0.6 darkens a deep
    /// site further than +0.6 lifts it.
    ///
    /// MUTATION: push `u` by the table instead of the luminance, drop the
    /// spread, or read the lift table for both signs.
    #[test]
    fn shadows_scale_linear_light_by_the_frame_spread() {
        let stops = |s: f32, b: f32, spread: f32, u: f32| {
            let site = HsSite { z: 0.0, z2: 0.0, b, spread };
            (srgb_to_linear(u + hs_push(0.0, s, site, u)) / srgb_to_linear(u)).log2()
        };
        let t = line_at(&HS_SH_LIFT, -2.0, HS_SH_B0, HS_SH_BSTEP);
        assert!((stops(0.6, -2.0, 2.0, 0.2) - 0.6 * 2.0 * t).abs() < 1e-3);
        assert!((stops(0.6, -2.0, 1.0, 0.2) - 0.6 * t).abs() < 1e-3);
        assert!(stops(0.6, -2.0, 0.0, 0.2).abs() < 1e-5);
        assert!(stops(0.6, -2.0, 2.0, 0.2) > stops(0.6, 1.0, 2.0, 0.2));
        assert!(-stops(-0.6, -8.0, 2.0, 0.05) > 1.2 * stops(0.6, -8.0, 2.0, 0.05));
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

    /// A uniform frame is left alone by Shadows: Camera Raw 18.4 moved a
    /// uniform field by 0.0000 in `u` at every level from 0.13 to 0.78 under
    /// Shadows +60; the law's push scales with the frame's spread, 0 there.
    ///
    /// MUTATION: a fixed spread instead of the frame's.
    #[test]
    fn a_uniform_frame_is_left_alone_by_shadows() {
        for u in [0.13f32, 0.26, 0.44, 0.60, 0.78] {
            for sh in [0.6f32, -0.6] {
                let m = hs_moves(&[u], 0.0, sh);
                let worst = m.iter().fold(0.0f32, |a, &d| a.max(d.abs()));
                assert!(worst < 1e-4, "u {u} sh {sh}: moved {worst}");
            }
        }
    }

    /// A uniform frame is left alone by Highlights: Camera Raw 18.4 moved a
    /// uniform field by 0.0000 in `u` at every level from 0.13 to 0.78 under
    /// Highlights −60; the frame-keyed law moves it −0.03.
    #[test]
    #[ignore = "measured Camera Raw behaviour the frame-keyed law does not reproduce yet (TECH_STACK)"]
    fn a_uniform_frame_is_left_alone_by_highlights() {
        for u in [0.13f32, 0.26, 0.44, 0.60, 0.78] {
            let m = hs_moves(&[u], -0.6, 0.0);
            let worst = m.iter().fold(0.0f32, |a, &d| a.max(d.abs()));
            assert!(worst < 0.003, "u {u}: moved {worst}");
        }
    }

    /// A frame of two levels, half and half (u 0.13 | u 0.70): Camera Raw
    /// moves each half as one, at every distance from the edge (Highlights
    /// −60: the bright half −0.208 at the edge to −0.238 4000 film px away,
    /// the dark half −0.016 to −0.022; Shadows +60: the dark half +0.017 to
    /// +0.018, the bright half 0.000). The law: −0.062 to −0.090; Shadows
    /// (the frame's spread 2.43 stops) about +0.081 on the dark half and
    /// +0.016 on the bright one.
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
        let key = HsKey::NOMINAL;
        let e = Exposure::new(START, 1.0);
        for (hl, sh) in [(-0.6f32, 0.0f32), (0.0, 0.6), (-0.4, 0.5)] {
            for i in 1..20 {
                let u = i as f32 / 20.0;
                let site = HsSite::diagonal(u, key);
                let moved = (u + hs_push(hl, sh, site, u)).clamp(0.0, 1.0);
                let want = camera_raw_exposure(moved, e.as_ref());
                let got = camera_raw_response_keyed(e.as_ref(), [0.0, hl, sh, 0.0, 0.0], u, key);
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
        let at = |z: f32| HsSite { z, z2: z, b: z, spread: 2.3 };
        assert!(hs_push(0.0, 0.6, at(-3.0), 0.08) > hs_push(0.0, 0.6, at(1.0), 0.08));
        assert!(hs_push(-0.6, 0.0, at(1.5), u) < hs_push(-0.6, 0.0, at(-2.0), u));
        let dark_frame = HsSite::diagonal(u, HsKey { key: -5.5, ..HsKey::NOMINAL });
        let bright_frame = HsSite::diagonal(u, HsKey { key: -2.5, ..HsKey::NOMINAL });
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
        let site = HsSite { z: -3.0, z2: -3.0, b: -3.0, spread: 2.3 };
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
