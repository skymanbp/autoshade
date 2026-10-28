//! A mask's local controls on a camera-profile start (v1.17.0): Camera Raw's
//! law, measured.
//!
//! Measured on Camera Raw's renders of one radial mask over a frame, each
//! local key set alone (and a few paired with its global twin):
//!
//! - **Exposure** is the global Exposure: the same ramp
//!   (`tone_cr::camera_raw_exposure_lut`), and a mask's EV adds to the
//!   global one (a +1 EV frame under a −0.25 EV mask renders as +0.75).
//!   It runs right after the global Exposure.
//! - **Contrast, Highlights, Shadows, Whites, Blacks, Dehaze** and
//!   **Clarity's tone** are operators of their own: local Highlights,
//!   Shadows and Dehaze push 1.1–1.3× their global twins, and local
//!   Contrast, Whites and Blacks move other bands. Each gets a response
//!   table of the global tables' form ([`L_RISE`] …), fitted with the
//!   engine in the loop on the local renders at ±30 … ±100, and runs right
//!   after the global sliders.
//! - **Highlights below 0 and Shadows above 0** run instead the global law
//!   (`tone_cr`: local, keyed to the frame, before Exposure) on the frame's
//!   own bases, the push scaled by [`L_HS_GAIN`] (regressing the local
//!   renders on the global law: 1.21 for Highlights −60, 1.25 for Shadows
//!   +60), right after the global pass ([`apply_local_hs`]).
//! - **Texture** renders exactly as the global Texture and stays where the
//!   mask stage puts it.
//! - **Temp and Tint** move the global white ([`local_white`]): a white
//!   balance adaptation from the global white to the moved one, blended by
//!   the mask's weight in the light between the exposure ramp and Adobe's
//!   curve, right after the global white balance. Fitted per sign on
//!   ±25 … ±100; at ±100 the fit leaves 4.3 ab, which no constant of this
//!   placement removes.
//! - **Hue** turns hues by the slider value in degrees, in the profile
//!   stage's reference primaries ([`hue_px`]).
//! - Saturation, the curves and the colour gains keep the mask stage's
//!   model; the airlight Dehaze of that stage does not run on this law.

use super::*;
use super::tone_cr::{camera_raw_unstart, table_at, DEHAZE_CHROMA, TABLE_N};

/// Rows of the local response tables: the five tone sliders in the knot
/// model's order, then Dehaze and Clarity's tone.
const ROWS: usize = 7;

/// Push per unit slider for a POSITIVE local slider at `u`, rows as
/// [`ROWS`]: a slider at `s ∈ (0, 1]` moves the value by
/// `s · (L_RISE[k](u) + s · L_RISE_Q[k](u))` — the global tables' form
/// (`tone_cr`), fitted the same way on the local renders.
const L_RISE: [[f32; TABLE_N]; ROWS] = [
    // contrast
    [-0.0234, 0.0005, -0.0302, -0.0484, -0.0395, -0.0454, -0.0554, -0.0581, -0.0582, -0.0562, -0.0527, -0.0492, -0.0445, -0.0374, -0.0308, -0.0223, -0.0094, 0.0029, 0.0171, 0.0342, 0.0405, 0.0526, 0.0757, 0.0664, 0.0508, 0.0838, 0.1101, 0.0576, -0.0361, -0.1092, -0.0197, 0.1913, 0.2200],
    // highlights
    [0.0016, 0.0066, 0.0112, 0.0134, 0.0157, 0.0199, 0.0246, 0.0301, 0.0363, 0.0413, 0.0493, 0.0649, 0.0842, 0.1016, 0.1240, 0.1593, 0.2008, 0.2393, 0.2835, 0.3304, 0.3320, 0.3482, 0.4482, 0.5728, 0.7093, 0.8822, 0.9978, 0.8602, 0.5672, 0.4069, 0.3909, 0.4310, 0.5158],
    // shadows
    [0.0539, 0.1191, 0.1405, 0.1294, 0.1251, 0.1266, 0.1176, 0.1005, 0.0817, 0.0715, 0.0682, 0.0577, 0.0436, 0.0366, 0.0363, 0.0368, 0.0356, 0.0342, 0.0321, 0.0290, 0.0297, 0.0283, 0.0211, 0.0180, 0.0156, 0.0137, 0.0173, 0.0182, 0.0116, 0.0046, 0.0045, 0.0303, 0.0987],
    // whites
    [0.0063, 0.0144, 0.0209, 0.0216, 0.0218, 0.0273, 0.0358, 0.0449, 0.0526, 0.0589, 0.0654, 0.0720, 0.0819, 0.0974, 0.1143, 0.1328, 0.1544, 0.1744, 0.1935, 0.2241, 0.2768, 0.4232, 0.6216, 0.7148, 0.7996, 0.9076, 0.9032, 0.7993, 0.6060, 0.4381, 0.4401, 0.5327, 0.6317],
    // blacks
    [0.4157, 0.4022, 0.3429, 0.2526, 0.1961, 0.1753, 0.1615, 0.1487, 0.1348, 0.1211, 0.1087, 0.0997, 0.0941, 0.0903, 0.0880, 0.0860, 0.0837, 0.0818, 0.0778, 0.0716, 0.0704, 0.0678, 0.0566, 0.0479, 0.0414, 0.0434, 0.0566, 0.0579, 0.0451, 0.0343, 0.0351, 0.0868, 0.2205],
    // dehaze
    [-0.0352, -0.0478, -0.0577, -0.0542, -0.0501, -0.0596, -0.0729, -0.0833, -0.0912, -0.0887, -0.0803, -0.0772, -0.0678, -0.0492, -0.0380, -0.0421, -0.0536, -0.0593, -0.0635, -0.0687, -0.0558, -0.0554, -0.1123, -0.1511, -0.1287, -0.1133, -0.1212, -0.1090, -0.0495, 0.0116, 0.0386, 0.0639, 0.0967],
    // clarity
    [-0.0264, 0.0018, 0.0348, 0.0539, 0.0702, 0.0924, 0.1078, 0.1159, 0.1189, 0.1210, 0.1186, 0.1007, 0.0824, 0.0713, 0.0516, 0.0226, -0.0098, -0.0391, -0.0349, -0.0074, -0.0197, -0.0623, -0.1045, -0.1390, -0.1299, -0.0943, -0.0826, -0.0897, -0.0706, -0.0201, -0.0115, -0.0156, 0.1001],
];

/// [`L_RISE`] for a NEGATIVE slider: the value moves by
/// `s · (L_FALL[k](u) + |s| · L_FALL_Q[k](u))`.
const L_FALL: [[f32; TABLE_N]; ROWS] = [
    // contrast
    [0.0125, -0.0213, -0.0364, -0.0328, -0.0325, -0.0391, -0.0445, -0.0476, -0.0482, -0.0478, -0.0459, -0.0435, -0.0398, -0.0341, -0.0288, -0.0213, -0.0096, 0.0024, 0.0153, 0.0311, 0.0380, 0.0417, 0.0555, 0.0633, 0.0641, 0.0623, 0.0715, 0.0982, 0.0868, 0.0436, 0.0427, 0.0727, 0.0822],
    // highlights
    [0.0035, 0.0074, 0.0117, 0.0141, 0.0163, 0.0205, 0.0253, 0.0310, 0.0373, 0.0426, 0.0505, 0.0658, 0.0850, 0.1020, 0.1226, 0.1551, 0.1974, 0.2407, 0.2888, 0.3401, 0.3405, 0.3407, 0.4210, 0.4740, 0.4913, 0.5433, 0.5758, 0.5652, 0.5251, 0.4834, 0.4265, 0.3769, 0.4409],
    // shadows
    [0.1309, 0.0958, 0.1194, 0.1422, 0.1274, 0.1156, 0.1121, 0.0990, 0.0811, 0.0700, 0.0660, 0.0560, 0.0428, 0.0363, 0.0357, 0.0362, 0.0353, 0.0340, 0.0320, 0.0292, 0.0288, 0.0271, 0.0217, 0.0182, 0.0154, 0.0140, 0.0176, 0.0151, 0.0050, 0.0026, 0.0079, 0.0279, 0.0761],
    // whites
    [0.0069, 0.0141, 0.0215, 0.0228, 0.0217, 0.0267, 0.0355, 0.0449, 0.0528, 0.0593, 0.0666, 0.0733, 0.0824, 0.0967, 0.1133, 0.1314, 0.1522, 0.1739, 0.2000, 0.2324, 0.2579, 0.2870, 0.3331, 0.3768, 0.4251, 0.4769, 0.5275, 0.6287, 0.8327, 1.0980, 1.3757, 1.6200, 1.7363],
    // blacks
    [0.3661, 0.3303, 0.3167, 0.3494, 0.3527, 0.2770, 0.1947, 0.1626, 0.1525, 0.1376, 0.1224, 0.1110, 0.1022, 0.0962, 0.0943, 0.0929, 0.0902, 0.0882, 0.0838, 0.0764, 0.0747, 0.0718, 0.0602, 0.0510, 0.0440, 0.0424, 0.0559, 0.0608, 0.0444, 0.0374, 0.0444, 0.0784, 0.1684],
    // dehaze
    [-0.1099, -0.1512, -0.1611, -0.1462, -0.1446, -0.1638, -0.1822, -0.1980, -0.2137, -0.2149, -0.2070, -0.2061, -0.1843, -0.1390, -0.1157, -0.1305, -0.1558, -0.1642, -0.1785, -0.2019, -0.1928, -0.1969, -0.2397, -0.2400, -0.2241, -0.2462, -0.2427, -0.1771, -0.0822, -0.0167, -0.0128, -0.1601, -0.5610],
    // clarity
    [-0.0006, 0.0073, 0.0133, 0.0138, 0.0106, 0.0080, 0.0069, 0.0050, -0.0043, -0.0241, -0.0464, -0.0616, -0.0705, -0.0794, -0.0898, -0.0955, -0.0947, -0.0872, -0.0644, -0.0379, -0.0313, -0.0247, -0.0114, -0.0105, -0.0060, 0.0002, 0.0006, 0.0065, 0.0118, 0.0071, 0.0024, 0.0355, 0.1248],
];

/// The curvature of [`L_RISE`].
const L_RISE_Q: [[f32; TABLE_N]; ROWS] = [
    // contrast
    [-0.0956, -0.0701, -0.0113, 0.0033, -0.0128, -0.0060, -0.0010, -0.0049, -0.0071, -0.0080, -0.0071, -0.0063, -0.0066, -0.0076, -0.0084, -0.0079, -0.0050, -0.0012, 0.0018, 0.0075, 0.0112, 0.0003, -0.0069, 0.0148, 0.0410, 0.0347, 0.0295, 0.1027, 0.2247, 0.3634, 0.3880, 0.1899, 0.0157],
    // highlights
    [0.0001, -0.0001, -0.0012, -0.0019, -0.0019, -0.0020, -0.0026, -0.0034, -0.0042, -0.0046, -0.0051, -0.0056, -0.0055, -0.0058, -0.0076, -0.0115, -0.0155, -0.0169, -0.0173, -0.0045, 0.0494, 0.1220, 0.1589, 0.1144, 0.0010, -0.1675, -0.3135, -0.2467, -0.0858, -0.0477, -0.0795, -0.1149, -0.1544],
    // shadows
    [0.0817, 0.0153, -0.0087, 0.0042, 0.0092, 0.0007, -0.0067, -0.0106, -0.0103, -0.0090, -0.0085, -0.0079, -0.0071, -0.0061, -0.0055, -0.0052, -0.0054, -0.0054, -0.0053, -0.0050, -0.0041, -0.0032, -0.0036, -0.0045, -0.0036, -0.0028, -0.0040, -0.0054, -0.0048, -0.0017, 0.0030, 0.0118, 0.0254],
    // whites
    [-0.0009, -0.0010, -0.0020, -0.0028, -0.0022, -0.0014, -0.0010, -0.0014, -0.0022, -0.0024, -0.0009, 0.0030, 0.0090, 0.0160, 0.0245, 0.0371, 0.0563, 0.0887, 0.1505, 0.2233, 0.2480, 0.1931, 0.1431, 0.1767, 0.1565, 0.0188, -0.0572, 0.0525, 0.1982, 0.1491, -0.0299, -0.1577, -0.2184],
    // blacks
    [-0.1751, -0.2050, -0.2026, -0.1308, -0.0612, -0.0358, -0.0294, -0.0281, -0.0252, -0.0209, -0.0159, -0.0110, -0.0083, -0.0079, -0.0076, -0.0074, -0.0075, -0.0079, -0.0086, -0.0093, -0.0086, -0.0080, -0.0086, -0.0084, -0.0047, -0.0021, -0.0054, -0.0082, -0.0061, -0.0020, 0.0194, 0.0434, 0.0204],
    // dehaze
    [-0.0037, 0.0009, 0.0020, -0.0009, 0.0013, 0.0086, 0.0075, -0.0054, -0.0215, -0.0304, -0.0299, -0.0273, -0.0217, -0.0134, -0.0085, -0.0087, -0.0111, -0.0102, -0.0074, -0.0068, -0.0086, -0.0607, -0.2275, -0.3564, -0.2755, -0.1386, -0.0822, -0.0463, -0.0059, 0.0189, 0.0251, 0.0045, -0.0469],
    // clarity
    [0.0083, 0.0096, -0.0014, -0.0095, -0.0130, -0.0204, -0.0300, -0.0361, -0.0404, -0.0466, -0.0497, -0.0462, -0.0422, -0.0423, -0.0414, -0.0312, -0.0155, -0.0028, 0.0017, -0.0075, -0.0104, 0.0082, 0.0287, 0.0400, 0.0353, 0.0190, 0.0186, 0.0211, -0.0047, -0.0398, -0.0473, -0.1090, -0.3505],
];

/// The curvature of [`L_FALL`].
const L_FALL_Q: [[f32; TABLE_N]; ROWS] = [
    // contrast
    [-0.0161, -0.0009, 0.0108, 0.0101, 0.0070, 0.0081, 0.0099, 0.0115, 0.0114, 0.0105, 0.0095, 0.0082, 0.0069, 0.0060, 0.0051, 0.0039, 0.0021, -0.0002, -0.0024, -0.0048, -0.0068, -0.0080, -0.0110, -0.0136, -0.0128, -0.0083, -0.0162, -0.0365, -0.0252, 0.0036, -0.0069, -0.0390, -0.0510],
    // highlights
    [-0.0015, -0.0026, -0.0031, -0.0029, -0.0038, -0.0050, -0.0061, -0.0075, -0.0086, -0.0102, -0.0131, -0.0181, -0.0247, -0.0312, -0.0386, -0.0496, -0.0662, -0.0859, -0.1055, -0.1226, -0.1177, -0.0412, 0.1026, 0.1573, 0.0159, -0.1559, -0.1980, -0.1702, -0.1438, -0.1287, -0.0877, -0.0256, -0.0263],
    // shadows
    [-0.0020, -0.0106, -0.0558, -0.0657, -0.0441, -0.0407, -0.0391, -0.0285, -0.0201, -0.0163, -0.0150, -0.0130, -0.0103, -0.0086, -0.0081, -0.0081, -0.0077, -0.0072, -0.0067, -0.0062, -0.0062, -0.0055, -0.0043, -0.0038, -0.0029, -0.0036, -0.0054, -0.0023, 0.0012, -0.0005, -0.0039, -0.0051, -0.0040],
    // whites
    [-0.0019, -0.0035, -0.0047, -0.0040, -0.0041, -0.0061, -0.0087, -0.0110, -0.0127, -0.0140, -0.0158, -0.0181, -0.0219, -0.0283, -0.0355, -0.0430, -0.0512, -0.0609, -0.0724, -0.0859, -0.1005, -0.1175, -0.1381, -0.1625, -0.1957, -0.2285, -0.2572, -0.3309, -0.5189, -0.7557, -0.9059, -1.0348, -1.2463],
    // blacks
    [-0.1866, -0.1286, -0.0970, -0.1393, -0.1570, -0.0684, 0.0174, 0.0291, 0.0184, 0.0146, 0.0110, 0.0071, 0.0032, -0.0018, -0.0054, -0.0065, -0.0064, -0.0061, -0.0055, -0.0054, -0.0061, -0.0057, -0.0038, -0.0025, 0.0010, -0.0019, -0.0133, -0.0097, 0.0011, -0.0016, -0.0094, -0.0282, -0.0720],
    // dehaze
    [0.0000, 0.0000, 0.0000, -0.0000, -0.0000, -0.0000, -0.0000, 0.0000, 0.0000, -0.0000, -0.0000, -0.0000, 0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, -0.0000, 0.0000, 0.0000, 0.0000, -0.0000, -0.0000, -0.0000, 0.0000, 0.0000, 0.0000, 0.0000, -0.0000],
    // clarity
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
];

/// Local Dehaze's colour: the global stage's chroma gain
/// (`tone_cr::DEHAZE_CHROMA`) at this many times the slider.
const L_DEHAZE_CHROMA: f32 = 1.3;

fn local_row(k: usize, s: f32, y: f32) -> f32 {
    let (t, q) = if s < 0.0 { (&L_FALL[k], &L_FALL_Q[k]) } else { (&L_RISE[k], &L_RISE_Q[k]) };
    table_at(t, y) + s.abs() * table_at(q, y)
}

/// One adjustment's seven table sliders in −1..1, rows as [`ROWS`].
/// Highlights below 0 and Shadows above 0 are not table sliders on this law:
/// they run in the pass before Exposure ([`apply_local_hs`]).
fn table_sliders(m: &crate::recipe::LocalAdjustment) -> [f32; ROWS] {
    let [hl, sh] = local_hs(m);
    let mut s = [m.contrast, m.highlights, m.shadows, m.whites, m.blacks, m.dehaze, m.clarity].map(|v| (v / 100.0).clamp(-1.0, 1.0));
    if hl != 0.0 {
        s[1] = 0.0;
    }
    if sh != 0.0 {
        s[2] = 0.0;
    }
    s
}

/// A mask's Highlights (below 0) and Shadows (above 0) in −1..1 — the signs
/// the local law covers; 0 for the other sign.
fn local_hs(m: &crate::recipe::LocalAdjustment) -> [f32; 2] {
    let hl = (m.highlights / 100.0).clamp(-1.0, 1.0);
    let sh = (m.shadows / 100.0).clamp(-1.0, 1.0);
    [hl.min(0.0), sh.max(0.0)]
}

/// How much harder a mask's Highlights and Shadows push than the global
/// sliders at the same value, on the same base and key (`[highlights,
/// shadows]`): fitted on the local renders (module head).
const L_HS_GAIN: [f32; 2] = [1.2, 1.25];

/// Does any drawn adjustment of `r` move Highlights or Shadows on the local law?
pub(super) fn any_local_hs(r: &EditRecipe) -> bool {
    r.masks.iter().any(|m| m.enabled && m.amount.clamp(0.0, 1.0) != 0.0 && local_hs(m) != [0.0, 0.0])
}

/// A mask's Highlights and Shadows: the global law ([`tone_cr::hs_pixel_scaled`])
/// on the frame's own sites `field`, at [`L_HS_GAIN`], blended by the mask's
/// weight; right after the global pass, before Exposure.
#[allow(clippy::too_many_arguments)] // the develop's frame, the recipe and the field
pub(super) fn apply_local_hs(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
    field: &[super::tone_cr::HsSite],
) {
    with_covers(r, rasters, frame, (w, h), |m| local_hs(m) != [0.0, 0.0], |covers| {
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                for (m, c) in covers {
                    let wt = c.at_px(x, y, px);
                    if wt <= 0.001 {
                        continue;
                    }
                    let [hl, sh] = local_hs(m);
                    let t = super::tone_cr::hs_pixel_scaled(*px, hl, sh, field[y * w + x], L_HS_GAIN);
                    for ch in 0..3 {
                        px[ch] += wt * (t[ch] - px[ch]);
                    }
                }
            }
        });
    });
}

/// The local slider pass over the profile stage's output, `x → u → the
/// sliders → x`, with the band guard of the global pass; under Dehaze, its
/// chroma gain. `None` when all seven are 0.
fn local_slider_lut(s: [f32; ROWS]) -> Option<SliderPass> {
    s.iter().any(|&v| v != 0.0).then(|| {
        const KEEP: f32 = 0.01;
        let mut tone = Vec::with_capacity(LUT_N);
        let (mut prev_y, mut prev) = (0.0f32, 0.0f32);
        for i in 0..LUT_N {
            let y = i as f32 / (LUT_N - 1) as f32;
            let push: f32 = (0..ROWS).map(|k| s[k] * local_row(k, s[k], y)).sum();
            let mut v = (y + push).clamp(0.0, 1.0);
            if i > 0 {
                v = v.max((prev + KEEP * (y - prev_y)).min(1.0));
            }
            tone.push(v);
            (prev_y, prev) = (y, v);
        }
        let lut = (0..LUT_N)
            .map(|i| camera_raw_start(sample_lut(&tone, camera_raw_unstart(i as f32 / (LUT_N - 1) as f32))))
            .collect();
        let d = s[5];
        let chroma = (d != 0.0).then(|| {
            (0..LUT_N)
                .map(|i| {
                    let u = camera_raw_unstart(i as f32 / (LUT_N - 1) as f32);
                    (L_DEHAZE_CHROMA * d * table_at(&DEHAZE_CHROMA, u)).exp()
                })
                .collect()
        });
        SliderPass { lut, chroma }
    })
}

/// Does `m` move anything the tone stage renders on this law: its Exposure,
/// a table slider, Dehaze or Clarity's tone?
pub(super) fn moves_tone(m: &crate::recipe::LocalAdjustment) -> bool {
    m.exposure_ev != 0.0 || table_sliders(m).iter().any(|&v| v != 0.0) || local_hs(m) != [0.0, 0.0]
}

/// Does `m` move the white balance?
pub(super) fn moves_white(m: &crate::recipe::LocalAdjustment) -> bool {
    m.temperature != 0.0 || m.tint != 0.0
}

/// Does any drawn adjustment of `r` move the tone stage?
pub(super) fn any_local_tone(r: &EditRecipe) -> bool {
    r.masks.iter().any(|m| m.enabled && m.amount.clamp(0.0, 1.0) != 0.0 && moves_tone(m))
}

/// The drawn adjustments of `r` that `acts` picks, each with its coverage on
/// this `w × h` frame, handed to `f` in the recipe's order; `None` when there
/// are none.
fn with_covers<R>(
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
    (w, h): (usize, usize),
    acts: impl Fn(&crate::recipe::LocalAdjustment) -> bool,
    f: impl FnOnce(&[(&crate::recipe::LocalAdjustment, Cover<'_>)]) -> R,
) -> Option<R> {
    let dims = (w as f32, h as f32);
    let picked: Vec<_> = r
        .masks
        .iter()
        .filter(|m| m.enabled && m.amount.clamp(0.0, 1.0) != 0.0 && acts(m))
        .map(|m| frame.linear_handles_to_raw(m, dims))
        .collect();
    if picked.is_empty() {
        return None;
    }
    let unwarp = frame.unwarp(dims);
    let brushes: Vec<Brushes> = picked.iter().map(|m| Brushes::of(m, w, h)).collect();
    let covers: Vec<_> = picked
        .iter()
        .zip(&brushes)
        .filter_map(|(m, b)| Cover::new(m, b, rasters, unwarp.as_ref(), w, h).map(|c| (m.as_ref(), c)))
        .collect();
    Some(f(&covers))
}

/// Each picked adjustment's move blended into every pixel by its weight, in
/// the recipe's order.
fn blend_each<T: Sync>(
    data: &mut [[f32; 3]],
    w: usize,
    covers: &[(&crate::recipe::LocalAdjustment, Cover<'_>)],
    moves: &[T],
    step: impl Fn(&T, [f32; 3]) -> [f32; 3] + Sync,
) {
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            for ((_, c), mv) in covers.iter().zip(moves) {
                let wt = c.at_px(x, y, px);
                if wt <= 0.001 {
                    continue;
                }
                let t = step(mv, *px);
                for ch in 0..3 {
                    px[ch] += wt * (t[ch] - px[ch]);
                }
            }
        }
    });
}

/// Local Exposure on this law: the global stage's ramp at the mask's EV
/// (`tone_cr::camera_raw_exposure_lut`, Adobe's channel rule), after the
/// global Exposure and before the five sliders, so the two add in EV.
pub(super) fn apply_local_exposure(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
) {
    with_covers(r, rasters, frame, (w, h), |m| m.exposure_ev != 0.0, |covers| {
        let luts: Vec<Vec<f32>> = covers.iter().filter_map(|(m, _)| camera_raw_exposure_lut(m.exposure_ev)).collect();
        blend_each(data, w, covers, &luts, |lut, px| camera_raw_channels(px, lut));
    });
}

/// The local table sliders on this law ([`L_RISE`] …), after the global
/// slider pass, each put on a pixel as the global pass is
/// (`tone_cr::camera_raw_slider_pixel`).
pub(super) fn apply_local_sliders(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
) {
    let acts = |m: &crate::recipe::LocalAdjustment| table_sliders(m).iter().any(|&v| v != 0.0);
    with_covers(r, rasters, frame, (w, h), acts, |covers| {
        let passes: Vec<SliderPass> = covers.iter().filter_map(|(m, _)| local_slider_lut(table_sliders(m))).collect();
        blend_each(data, w, covers, &passes, |p, px| camera_raw_slider_pixel(px, p));
    });
}

/// The white a mask's Temp and Tint (each −1..1) name, as a move of the
/// global white: mired (positive = cooler), tint units, and EV. Fitted on
/// the local renders, one (mired, tint, EV) per render, then one form per
/// sign across the magnitudes (module head).
fn local_white(temp: f32, tint: f32) -> (f64, f64, f32) {
    let (t, n) = (f64::from(temp), f64::from(tint));
    let (ma, mb, ea, eb) = if t < 0.0 {
        (LWB_MIRED_FALL, LWB_MIRED_FALL_Q, LWB_EV_FALL, LWB_EV_FALL_Q)
    } else {
        (LWB_MIRED_RISE, LWB_MIRED_RISE_Q, LWB_EV_RISE, LWB_EV_RISE_Q)
    };
    let mired = -t * (ma + t.abs() * mb) + LWB_MIRED_PER_TINT * n;
    let tint_units = n * if n < 0.0 { LWB_TINT_FALL } else { LWB_TINT_RISE } + LWB_TINT_PER_TEMP * t.abs();
    let ev = t * (ea + t.abs() * eb) + LWB_EV_PER_TINT * n;
    (mired, tint_units, ev as f32)
}

/// Mired of warming per unit Temp at small magnitudes, and its curvature.
const LWB_MIRED_RISE: f64 = 142.2;
const LWB_MIRED_RISE_Q: f64 = -49.84;
/// …of cooling, for a negative Temp.
const LWB_MIRED_FALL: f64 = 149.8;
const LWB_MIRED_FALL_Q: f64 = -28.88;
/// EV per unit Temp (brighter as it warms) and its curvature, per sign.
const LWB_EV_RISE: f64 = 0.162;
const LWB_EV_RISE_Q: f64 = -0.048;
const LWB_EV_FALL: f64 = 0.093;
const LWB_EV_FALL_Q: f64 = 0.126;
/// Tint units per unit Tint, per sign.
const LWB_TINT_RISE: f64 = 63.86;
const LWB_TINT_FALL: f64 = 51.08;
/// Magenta per unit Temp magnitude, either way.
const LWB_TINT_PER_TEMP: f64 = 11.05;
/// Mired and EV per unit Tint.
const LWB_MIRED_PER_TINT: f64 = -6.03;
const LWB_EV_PER_TINT: f64 = -0.165;

/// The global white `r` renders at, (Kelvin, tint) in Camera Raw's units, and
/// the as-shot white it is a move from.
fn global_white(r: &EditRecipe) -> ((f32, f32), (f32, f32)) {
    let as_shot = (r.as_shot_k.unwrap_or(5500.0), r.as_shot_tint.unwrap_or(0.0));
    let target = if r.temperature_k.is_some() || r.tint != 0.0 {
        (r.temperature_k.unwrap_or(as_shot.0), r.tint)
    } else {
        as_shot
    };
    (target, as_shot)
}

/// Local Temp and Tint on this law: the global white balance's adaptation
/// (`wb::camera_raw_wb`) to the white the mask moves the global one to,
/// relative to the global one, blended by the mask's weight in the light
/// after the profile stage's exposure ramp and before Adobe's curve
/// (`profile::UnderTone::undo_curve`). Runs right after the global white
/// balance, before the tone.
pub(super) fn apply_local_white(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
    space: ExportColorSpace,
) {
    with_covers(r, rasters, frame, (w, h), moves_white, |covers| {
        let (target, as_shot) = global_white(r);
        let back = inv3(&super::wb::camera_raw_wb(target, as_shot));
        let mired = 1e6 / f64::from(target.0);
        let moves: Vec<[[f32; 3]; 3]> = covers
            .iter()
            .map(|(m, _)| {
                let (dm, dt, ev) = local_white(m.temperature / 100.0, m.tint / 100.0);
                let to = ((1e6 / (mired + dm).max(1.0)) as f32, (f64::from(target.1) + dt) as f32);
                let k = ev.exp2();
                mat_mul3(&super::wb::camera_raw_wb(to, as_shot), &back).map(|row| row.map(|v| v * k))
            })
            .collect();
        let under = profile::UnderTone::new(space);
        let (dec, enc) = transfer_luts();
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let mut light: Option<[f32; 3]> = None;
                for ((_, c), mv) in covers.iter().zip(&moves) {
                    let wt = c.at_px(x, y, px);
                    if wt <= 0.001 {
                        continue;
                    }
                    let l = light.get_or_insert_with(|| under.undo_curve(px.map(|v| sample_lut(dec, v))));
                    let t = mat_vec3(mv, l);
                    for ch in 0..3 {
                        l[ch] += wt * (t[ch] - l[ch]);
                    }
                }
                if let Some(l) = light {
                    *px = under.redo_curve(l).map(|v| sample_lut(enc, v.clamp(0.0, 1.0)));
                }
            }
        });
    });
}

/// Local Hue on this law: Lightroom's slider is the angle itself, in degrees
/// (`crs:LocalHue` × 180), and it turns the hue of the colour in the
/// profile stage's reference primaries (ProPhoto's), linear light.
pub(super) fn hue_px(px: [f32; 3], degrees: f32, under: &profile::UnderTone, dec: &[f32], enc: &[f32]) -> [f32; 3] {
    let p = under.to_reference(px.map(|v| sample_lut(dec, v))).map(|v| v.clamp(0.0, 1.0));
    let (hh, ss, ll) = rgb_to_hsl(p[0], p[1], p[2]);
    let (r2, g2, b2) = hsl_to_rgb((hh + degrees / 360.0).rem_euclid(1.0), ss, ll);
    under.to_working([r2, g2, b2]).map(|v| sample_lut(enc, v.clamp(0.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every local table slider at its extremes, alone and all together,
    /// gives a non-decreasing pass inside [0, 1] (the band guard); at 0 there
    /// is no pass, and only Dehaze carries a chroma gain.
    ///
    /// MUTATION: drop the band guard in `local_slider_lut`, or read the
    /// FALL tables for a positive slider.
    #[test]
    fn the_local_slider_pass_stays_monotone_at_every_extreme() {
        assert!(local_slider_lut([0.0; ROWS]).is_none());
        let mut cases = vec![[1.0f32; ROWS], [-1.0; ROWS]];
        for k in 0..ROWS {
            for s in [-1.0f32, -0.3, 0.3, 1.0] {
                let mut c = [0.0f32; ROWS];
                c[k] = s;
                cases.push(c);
            }
        }
        for s in &cases {
            let p = local_slider_lut(*s).expect("a moving slider is a pass");
            assert!(p.lut.windows(2).all(|w| w[1] >= w[0]), "monotone at {s:?}");
            assert!(p.lut.iter().all(|v| (0.0..=1.0).contains(v)), "in range at {s:?}");
            assert_eq!(p.chroma.is_some(), s[5] != 0.0, "chroma only under Dehaze at {s:?}");
        }
        let mid = LUT_N / 2;
        let x = mid as f32 / (LUT_N - 1) as f32;
        let shadows_up = local_slider_lut([0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.0]).expect("a pass");
        assert!(shadows_up.lut[LUT_N / 8] > (LUT_N / 8) as f32 / (LUT_N - 1) as f32, "+60 Shadows lifts the shadows");
        let blacks_down = local_slider_lut([0.0, 0.0, 0.0, 0.0, -0.4, 0.0, 0.0]).expect("a pass");
        assert!(blacks_down.lut[LUT_N / 16] < (LUT_N / 16) as f32 / (LUT_N - 1) as f32, "−40 Blacks deepens them");
        assert!((shadows_up.lut[LUT_N - 1] - 1.0).abs() < 1e-3 && shadows_up.lut[mid] >= x - 1e-3, "white stays white");
    }

    /// A mask's white move is nothing at Temp and Tint 0; warming lowers the
    /// mired and brightens, cooling the other way, and a magenta Tint adds
    /// tint units.
    ///
    /// MUTATION: flip a sign in `local_white`, or read the RISE constants for
    /// a negative Temp.
    #[test]
    fn the_local_white_is_still_at_zero_and_signs_its_moves() {
        assert_eq!(local_white(0.0, 0.0), (0.0, 0.0, 0.0));
        let (warm_m, _, warm_ev) = local_white(0.5, 0.0);
        let (cool_m, _, cool_ev) = local_white(-0.5, 0.0);
        assert!(warm_m < -50.0 && cool_m > 50.0, "{warm_m} / {cool_m}");
        assert!(warm_ev > 0.0 && cool_ev < 0.0, "{warm_ev} / {cool_ev}");
        let (_, magenta, _) = local_white(0.0, 0.5);
        let (_, green, _) = local_white(0.0, -0.5);
        assert!(magenta > 20.0 && green < -20.0, "{magenta} / {green}");
    }

    /// The local Hue is a turn: 0° and a whole turn give the colour back, a
    /// grey has no hue to turn, and +120° takes a red toward green.
    ///
    /// MUTATION: read the angle as a fraction of ±100 again, or turn in the
    /// working primaries instead of the reference ones.
    #[test]
    fn the_local_hue_turns_by_degrees_in_the_reference_primaries() {
        let under = profile::UnderTone::new(ExportColorSpace::Srgb);
        let (dec, enc) = transfer_luts();
        let red = [0.70f32, 0.30, 0.25];
        for deg in [0.0f32, 360.0] {
            let p = hue_px(red, deg, &under, dec, enc);
            assert!(p.iter().zip(&red).all(|(a, b)| (a - b).abs() < 4e-3), "{deg}°: {p:?}");
        }
        let grey = hue_px([0.4; 3], 90.0, &under, dec, enc);
        assert!(grey.iter().all(|&c| (c - 0.4).abs() < 4e-3), "a grey: {grey:?}");
        let turned = hue_px(red, 120.0, &under, dec, enc);
        assert!(turned[1] > turned[0], "+120° takes red toward green: {turned:?}");
    }
}
