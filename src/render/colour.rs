//! Per-colour stages: saturation and vibrance, HSL, colour grading, Calibration, the B&W mix and Point Color.

use super::*;

/// Saturation + vibrance around the pixel's luma. Vibrance boosts low-saturation
/// pixels more (so already-vivid colours don't blow out).
pub(super) fn apply_sat_vibrance(r: f32, g: f32, b: f32, sat: f32, vib: f32) -> [f32; 3] {
    let l = 0.299 * r + 0.587 * g + 0.114 * b;
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let pixel_sat = if mx > 1e-4 { (mx - mn) / mx } else { 0.0 };
    let factor = (1.0 + sat + vib * (1.0 - pixel_sat)).max(0.0);
    [
        (l + (r - l) * factor).clamp(0.0, 1.0),
        (l + (g - l) * factor).clamp(0.0, 1.0),
        (l + (b - l) * factor).clamp(0.0, 1.0),
    ]
}

/// Per-colour HSL (the 8 ACR bands). For each pixel: find which colour band(s)
/// its hue falls in (triangular partition of unity over the band centres), then
/// rotate hue / scale saturation / scale luminance by the band-weighted amounts.
/// Achromatic pixels (no hue) are untouched. Runs in sRGB-gamma space — a
/// tasteful approximation; the XMP→Lightroom path renders the exact ACR model.
pub(super) fn apply_hsl(data: &mut [[f32; 3]], hsl: &crate::recipe::Hsl) {
    if hsl.is_neutral() {
        return;
    }
    data.par_iter_mut().for_each(|px| {
        let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);
        // Fade the WHOLE HSL effect out on low-CHROMA pixels. Gate on chroma
        // (max−min), NOT HSL saturation: HSL `s` is ill-conditioned near white and
        // black — a bright, faintly-blue sea-foam pixel has chroma ≈ 0.12 yet HSL
        // s ≈ 1.0, so an HSL-`s` gate hits specular highlights at FULL strength and
        // a Blue-band luminance push crushes white foam to grey. Chroma is a true
        // colourfulness measure: ≈0 for near-grey (the overcast-sky blotch case)
        // AND for near-white foam, ramping to full only on genuinely saturated
        // colour, so both are protected while real colours are still adjusted.
        let chroma = chroma(px);
        let satw = smoothstep(CHROMA_GATE.0, CHROMA_GATE.1, chroma);
        if satw <= 0.0 {
            return; // (per-pixel closure: this pixel is untouched)
        }
        let (b0, b1, w1) = bracket_bands(h * 360.0, &HSL_CENTERS);
        let w0 = 1.0 - w1;
        let hue_adj = (w0 * hsl.hue[b0] + w1 * hsl.hue[b1]) * satw;
        let sat_adj = (w0 * hsl.saturation[b0] + w1 * hsl.saturation[b1]) * satw;
        let lum_adj = (w0 * hsl.luminance[b0] + w1 * hsl.luminance[b1]) * satw;
        // hue: ±100 → ±30° rotation; sat: ±100 → ±100%; lum gentler (×0.5).
        let new_h = (h + (hue_adj / 100.0) * (30.0 / 360.0)).rem_euclid(1.0);
        let new_s = (s * (1.0 + sat_adj / 100.0)).clamp(0.0, 1.0);
        let new_l = (l * (1.0 + 0.5 * lum_adj / 100.0)).clamp(0.0, 1.0);
        let (r, g, b) = hsl_to_rgb(new_h, new_s, new_l);
        *px = [r, g, b];
    });
}

/// ACR band centres in degrees (red..magenta), matching recipe::HSL_BANDS.
/// Shared with the reverse-fit so its per-band statistics use the SAME partition.
pub(crate) const HSL_CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

/// The colourfulness gate every per-colour stage fades in over — chroma
/// (max − min) from nothing at the first edge to full at the second — so the
/// colour mixer, the B&W mix, the point colours and the Point Color eyedropper
/// agree on which pixels have a colour to belong to ([`apply_hsl`] says why
/// chroma and not HSL saturation).
pub(super) const CHROMA_GATE: (f32, f32) = (0.05, 0.22);

/// The two band indices bracketing hue `deg` and the blend weight toward the
/// second (partition of unity). Centres are non-uniform and wrap (magenta 300°
/// → red 360°/0°), so the last segment spans 300..360 back to red.
pub(crate) fn bracket_bands(deg: f32, centers: &[f32; 8]) -> (usize, usize, f32) {
    let d = deg.rem_euclid(360.0);
    for i in 0..8 {
        let lo = centers[i];
        let hi = if i + 1 < 8 { centers[i + 1] } else { 360.0 };
        if d >= lo && d < hi {
            let upper = if i + 1 < 8 { i + 1 } else { 0 };
            return (i, upper, (d - lo) / (hi - lo));
        }
    }
    (0, 1, 0.0) // unreachable: the segments tile [0,360)
}

/// sRGB-gamma RGB → HSL, all in [0,1] (hue normalised to turns).
pub(crate) fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l); // achromatic
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h.rem_euclid(1.0), s, l)
}

/// A pixel's CHROMA in this engine's working domain: the spread of its three
/// channels, which is what every colour gate here measures (`CHROMA_GATE`).
/// Spelled out at four sites before v1.5.0 and at three more in `fit.rs`; one
/// name, so a gate and the census that reports on it cannot drift.
pub(crate) fn chroma(px: &[f32; 3]) -> f32 {
    px[0].max(px[1]).max(px[2]) - px[0].min(px[1]).min(px[2])
}

/// HSL → sRGB-gamma RGB (inverse of [`rgb_to_hsl`]).
pub(super) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s < 1e-6 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue2rgb = |mut t: f32| -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (hue2rgb(h + 1.0 / 3.0), hue2rgb(h), hue2rgb(h - 1.0 / 3.0))
}

/// Lightroom-style colour grading: tint + lift the shadow / midtone / highlight
/// tonal regions (and a global wheel) by their hue/sat/lum. Region membership is a
/// smoothstep split on luma; `blending` scales the regional effect, `balance`
/// shifts the shadow/highlight split. Approximation; XMP→Lightroom is exact.
pub(super) fn apply_color_grade(data: &mut [[f32; 3]], cg: &crate::recipe::ColorGrade) {
    if cg.is_neutral() {
        return;
    }
    data.par_iter_mut().for_each(|px| {
        let [w_sh, w_mid, w_hi] = color_grade_regions(cg, luma601(px));
        apply_wheel(px, cg.shadow_hue, cg.shadow_sat, cg.shadow_lum, w_sh);
        apply_wheel(px, cg.midtone_hue, cg.midtone_sat, cg.midtone_lum, w_mid);
        apply_wheel(px, cg.highlight_hue, cg.highlight_sat, cg.highlight_lum, w_hi);
        apply_wheel(px, cg.global_hue, cg.global_sat, cg.global_lum, 1.0); // global: all tones
    });
}

/// The shadow / midtone / highlight weights the grade gives a pixel of luma
/// `l`. One function for the renderer and for the fit's colour-grade stage
/// (`fit::grade`), which solves the wheels on the same split it is rendered
/// with.
pub(crate) fn color_grade_regions(cg: &crate::recipe::ColorGrade, l: f32) -> [f32; 3] {
    // balance shifts the shadow/highlight midpoint: positive leans toward highlights.
    let mid = (0.5 - 0.25 * (cg.balance / 100.0)).clamp(0.05, 0.95);
    // `blending` sets how much the tonal regions OVERLAP — the schema's own
    // words (recipe.rs) and Lightroom's. It used to scale the regional
    // AMPLITUDE instead, with two consequences: a legal Blending of 0 silently
    // erased all three regional wheels (only the global wheel survived, which
    // read as "grading is half broken"), and ACR's DEFAULT of 50 rendered
    // every graded photo at half strength — so our render disagreed with the
    // Lightroom render the XMP hands off, for essentially every graded photo.
    // 100 reproduces the previous weights EXACTLY (ramps spanning mid..1 and
    // 0..mid); lower values tighten the ramps around `mid` instead of fading
    // the effect out.
    let overlap = (cg.blending / 100.0).clamp(0.0, 1.0);
    // A floor keeps the tightest split one smoothstep wide: a true step would
    // band visibly on a smooth gradient.
    const MIN_SPAN: f32 = 0.02;
    let hi_end = (mid + ((1.0 - mid) * overlap).max(MIN_SPAN)).min(1.0);
    let sh_start = (mid - (mid * overlap).max(MIN_SPAN)).max(0.0);
    let w_hi = smoothstep(mid, hi_end, l);
    let w_sh = 1.0 - smoothstep(sh_start, mid, l);
    let w_mid = (1.0 - w_hi - w_sh).clamp(0.0, 1.0);
    [w_sh, w_mid, w_hi]
}

/// How far a wheel at Saturation 100 and full region weight carries a pixel
/// toward its pure hue: the blend amount is `sat / 100 * weight * this`.
pub(crate) const WHEEL_SAT_REACH: f32 = 0.4;

/// The pure hue a wheel at `hue_deg` tints a pixel of luma `l` toward
/// ([`apply_wheel`]'s own target), as an RGB triple.
pub(crate) fn wheel_tint(hue_deg: f32, l: f32) -> [f32; 3] {
    let (r, g, b) = hsl_to_rgb((hue_deg / 360.0).rem_euclid(1.0), 1.0, l);
    [r, g, b]
}

/// Apply one colour-grade wheel to a pixel: shift chroma toward the wheel's hue
/// (scaled by sat × weight) and scale brightness by its luminance — both gentle.
fn apply_wheel(px: &mut [f32; 3], hue_deg: f32, sat: f32, lum: f32, weight: f32) {
    if weight <= 1e-4 {
        return;
    }
    if sat.abs() > 1e-4 {
        // Tint toward the pure hue AT THIS PIXEL'S OWN LUMINANCE (not a fixed
        // 0.5-grey anchor) and blend — this keeps luma roughly constant, so deep
        // shadows / bright highlights aren't crushed past [0,1] the way a fixed
        // additive push does. Closer to ACR's luma-aware toning.
        let tint = wheel_tint(hue_deg, luma601(px));
        let amt = (sat / 100.0) * weight * WHEEL_SAT_REACH;
        for c in 0..3 {
            px[c] = (px[c] + (tint[c] - px[c]) * amt).clamp(0.0, 1.0);
        }
    }
    if lum.abs() > 1e-4 {
        let k = (1.0 + (lum / 100.0) * weight * 0.5).max(0.0);
        for c in px.iter_mut() {
            *c = (*c * k).clamp(0.0, 1.0);
        }
    }
}

/// The Rec. 2020 primaries (CIE xy): the reference gamut the Calibration
/// panel moves its three primaries in. Wide enough that every working space
/// this engine develops in (sRGB, Display P3, Adobe RGB) sits inside it, and —
/// unlike ProPhoto's — every primary is a real colour, so moving one never
/// leaves the spectral locus's neighbourhood or sends a chromaticity through
/// the degenerate v′ = 0 edge.
const REC2020_PRIM: [[f32; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];

/// ±100 on a primary's Hue turns that primary this far about D65 in the CIE
/// 1976 u′v′ plane (+ toward the next primary in hue order: red → yellow,
/// green → cyan, blue → magenta). PROVISIONAL — first principles until the
/// Lightroom kit's `CAL-*` exports pin the reach.
const CAL_HUE_REACH_RAD: f32 = 30.0 * std::f32::consts::PI / 180.0;

/// ±100 on a primary's Saturation scales its u′v′ distance from D65 by
/// 1 ± this. PROVISIONAL, like the hue reach.
const CAL_SAT_REACH: f32 = 0.5;

/// ±100 Shadows tint takes this fraction off the green gain at black (+,
/// magenta) or puts it on (−, green). PROVISIONAL.
const CAL_SHADOW_TINT_REACH: f32 = 0.15;

/// The linear luminance by which the shadows tint has faded out (18 % grey).
const CAL_SHADOW_TINT_KNEE: f32 = 0.18;

/// sRGB (Rec. 709) luminance weights — the grey the B&W treatment and the
/// shadows tint measure a pixel by.
const REC709_Y: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn xy_to_uv(p: [f32; 2]) -> [f32; 2] {
    let d = -2.0 * p[0] + 12.0 * p[1] + 3.0;
    [4.0 * p[0] / d, 9.0 * p[1] / d]
}

fn uv_to_xy(p: [f32; 2]) -> [f32; 2] {
    let d = 6.0 * p[0] - 16.0 * p[1] + 12.0;
    [9.0 * p[0] / d, 4.0 * p[1] / d]
}

/// Lightroom's Calibration panel (v1.5.0) as one linear-light operator.
///
/// **The primaries.** The panel's Red/Green/Blue Hue and Saturation move the
/// PRIMARIES of the camera's colour space, not a band of pixel hues: every
/// colour is a mix of the three, so turning the red primary shifts every
/// colour in proportion to how much red it holds, and a neutral — equal parts
/// of all three — stays neutral. That is a change of basis, so it is ONE 3×3
/// matrix: the working pixel's coordinates are taken in the Rec. 2020 basis
/// ([`REC2020_PRIM`], standing in for the camera's own), reinterpreted under
/// primaries moved in u′v′ (each turned about D65 by its Hue, its distance from
/// D65 scaled by its Saturation), and carried back. `rgb_to_xyz` normalises
/// both bases to D65, so white — and with it every grey — maps to itself by
/// construction.
///
/// **The shadows tint** then moves green against red and blue in the dark
/// tones, weighted toward black and gone by [`CAL_SHADOW_TINT_KNEE`], with the
/// red/blue gain chosen so a grey keeps its luminance.
///
/// The working primaries are taken as sRGB's; a wide-gamut export conjugates
/// through the same basis, a second-order difference inside the provisional
/// reach constants.
struct Calibration {
    /// Working linear RGB → the same space under the moved primaries.
    matrix: [[f32; 3]; 3],
    /// The shadows tint's green-gain change at black (+ = magenta).
    shadow_tint: f32,
}

impl Calibration {
    fn of(r: &EditRecipe) -> Option<Self> {
        let [tint, red_hue, red_sat, green_hue, green_sat, blue_hue, blue_sat] = r.calibration()?;
        let white = xy_to_uv(D65_XY);
        let moves = [(red_hue, red_sat), (green_hue, green_sat), (blue_hue, blue_sat)];
        let moved: [[f32; 2]; 3] = std::array::from_fn(|i| {
            let (hue, sat) = moves[i];
            let uv = xy_to_uv(REC2020_PRIM[i]);
            let (du, dv) = (uv[0] - white[0], uv[1] - white[1]);
            let rho = (du * du + dv * dv).sqrt() * (1.0 + CAL_SAT_REACH * sat / 100.0).max(0.05);
            let theta = dv.atan2(du) + CAL_HUE_REACH_RAD * hue / 100.0;
            uv_to_xy([white[0] + rho * theta.cos(), white[1] + rho * theta.sin()])
        });
        let working = rgb_to_xyz(SRGB_PRIM, D65_XY);
        let reference = rgb_to_xyz(REC2020_PRIM, D65_XY);
        let to_reference = mat_mul3(&inv3(&reference), &working);
        let matrix = mat_mul3(&inv3(&working), &mat_mul3(&rgb_to_xyz(moved, D65_XY), &to_reference));
        Some(Self { matrix, shadow_tint: CAL_SHADOW_TINT_REACH * tint / 100.0 })
    }

    fn apply(&self, lin: [f32; 3]) -> [f32; 3] {
        let t = mat_vec3(&self.matrix, &lin);
        if self.shadow_tint == 0.0 {
            return t;
        }
        let y = REC709_Y[0] * t[0] + REC709_Y[1] * t[1] + REC709_Y[2] * t[2];
        let k = self.shadow_tint * (1.0 - smoothstep(0.0, CAL_SHADOW_TINT_KNEE, y));
        let rb = 1.0 + k * REC709_Y[1] / (REC709_Y[0] + REC709_Y[2]);
        [t[0] * rb, t[1] * (1.0 - k), t[2] * rb]
    }
}

/// The Calibration panel over a frame, in place — nothing at all while every
/// slider is 0 (see [`Calibration`]).
pub(super) fn apply_calibration(data: &mut [[f32; 3]], r: &EditRecipe) {
    let Some(cal) = Calibration::of(r) else { return };
    let (dec, enc) = transfer_luts();
    data.par_iter_mut().for_each(|px| {
        let lin = [decode_wide(dec, px[0]), decode_wide(dec, px[1]), decode_wide(dec, px[2])];
        *px = cal.apply(lin).map(|c| encode_wide(enc, c));
    });
}

/// ±100 on a B&W mixer band scales a fully coloured pixel's grey by
/// 2^±this in linear light. PROVISIONAL until the Lightroom kit's `BW-*`.
const GRAY_MIX_STOPS: f32 = 1.5;

/// Lightroom's B&W treatment (v1.5.0): every pixel becomes the grey of its
/// linear luminance ([`REC709_Y`]), lightened or darkened by the mixer band
/// its hue falls in.
///
/// The band weights and the colourfulness gate are [`apply_hsl`]'s own — the
/// same partition over [`HSL_CENTERS`], the same chroma smoothstep — so B&W
/// Red +100 moves exactly the pixels HSL Red Luminance would, and a grey
/// (which has no hue to belong to a band) is mixed by nobody.
pub(super) fn apply_gray_mix(data: &mut [[f32; 3]], mix: &[f32; 8]) {
    let (dec, enc) = transfer_luts();
    let mixing = mix.iter().any(|v| *v != 0.0);
    data.par_iter_mut().for_each(|px| {
        let lin = [decode_wide(dec, px[0]), decode_wide(dec, px[1]), decode_wide(dec, px[2])];
        let mut y = REC709_Y[0] * lin[0] + REC709_Y[1] * lin[1] + REC709_Y[2] * lin[2];
        if mixing {
            let chroma = chroma(px);
            let satw = smoothstep(CHROMA_GATE.0, CHROMA_GATE.1, chroma);
            if satw > 0.0 {
                let (h, _, _) = rgb_to_hsl(px[0], px[1], px[2]);
                let (b0, b1, w1) = bracket_bands(h * 360.0, &HSL_CENTERS);
                let band = (1.0 - w1) * mix[b0] + w1 * mix[b1];
                y *= (GRAY_MIX_STOPS * satw * band / 100.0).exp2();
            }
        }
        let g = sample_lut(enc, y.clamp(0.0, 1.0));
        *px = [g, g, g];
    });
}

/// The relative hue window of a point colour spans this far either side of
/// its sampled hue: the stored 0..1 coordinate is 0.5 at the swatch and 0 / 1
/// this many radians away. PROVISIONAL until the kit's `PC-*` pins it.
const POINT_HUE_HALF_SPAN: f32 = std::f32::consts::PI / 3.0;

/// A point colour's ±1 hue shift turns its colours this far. PROVISIONAL.
pub(super) const POINT_HUE_SHIFT_TURNS: f32 = 30.0 / 360.0;

/// A point colour's ±1 luminance shift scales HSL lightness by 1 ± this.
/// PROVISIONAL.
const POINT_LUM_REACH: f32 = 0.5;

/// A window's membership: 0 outside the none edges, 1 between the full ones,
/// linear in between. An edge pair that coincides is a hard step.
fn trapezoid(x: f32, w: &[f32; 4]) -> f32 {
    if x < w[0] || x > w[3] {
        0.0
    } else if x < w[1] {
        (x - w[0]) / (w[1] - w[0])
    } else if x <= w[2] {
        1.0
    } else {
        (w[3] - x) / (w[3] - w[2])
    }
}

/// The swatch Lightroom's Point Color eyedropper makes of a picked colour,
/// in the colour model [`apply_point_colors`] matches in — the GUI's
/// eyedropper calls this, so a swatch always matches the pixel it came from.
/// `None` for a colour [`CHROMA_GATE`] gives to nobody: a swatch of a near-grey
/// would be a row of sliders that move no pixel.
pub fn point_color_at(px: [f32; 3]) -> Option<crate::recipe::PointColor> {
    let chroma = chroma(&px);
    if chroma <= CHROMA_GATE.0 {
        return None;
    }
    let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);
    Some(crate::recipe::PointColor::sampled(h * std::f32::consts::TAU, s, l))
}

/// The colour a swatch was sampled from, in the working space — the chip the
/// panel draws beside the swatch's sliders. [`point_color_at`]'s inverse.
pub fn point_color_rgb(p: &crate::recipe::PointColor) -> [f32; 3] {
    let (r, g, b) = hsl_to_rgb(p.src_hue / std::f32::consts::TAU, p.src_sat, p.src_lum);
    [r, g, b]
}

/// The recipe whose develop shows a photo as it stands where the point colours
/// run: every stage [`apply_develop_with_rasters`] takes after them — the
/// colour grade, clarity, texture, saturation and vibrance, the Detail panel's
/// three passes, the masks and the colour field — switched off, the point
/// colours themselves removed, and everything before them left as it is. The
/// Point Color eyedropper samples this, so a swatch sits on the pixels
/// [`apply_point_colors`] will test rather than on the finished look.
pub fn point_color_sampling_recipe(r: &EditRecipe) -> EditRecipe {
    EditRecipe {
        point_colors: Vec::new(),
        color_grade: crate::recipe::ColorGrade::default(),
        clarity: 0.0,
        texture: 0.0,
        saturation: 0.0,
        vibrance: 0.0,
        color_nr: 0.0,
        noise_reduction: 0.0,
        sharpening: 0.0,
        masks: Vec::new(),
        colour_field: None,
        ..r.clone()
    }
}

/// Lightroom's Point Color (v1.5.0): each swatch moves the hue, saturation and
/// lightness of the pixels inside its three windows, by its window weight.
///
/// Membership is the PRODUCT of the three trapezoids the swatch stores — hue
/// relative to the sampled hue, saturation and lightness absolute — times
/// [`apply_hsl`]'s chroma gate, which keeps a near-grey (whose HSL hue is
/// arbitrary and whose HSL saturation can be high near white) out of every
/// window. Several swatches add their weighted shifts. A pixel no swatch
/// reaches is left exactly as it was.
pub(super) fn apply_point_colors(data: &mut [[f32; 3]], points: &[crate::recipe::PointColor]) {
    use std::f32::consts::{PI, TAU};
    let live: Vec<_> = points.iter().filter(|p| !p.is_neutral()).collect();
    if live.is_empty() {
        return;
    }
    data.par_iter_mut().for_each(|px| {
        let chroma = chroma(px);
        let gate = smoothstep(CHROMA_GATE.0, CHROMA_GATE.1, chroma);
        if gate <= 0.0 {
            return;
        }
        let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);
        let (mut dh, mut ds, mut dl) = (0.0f32, 0.0f32, 0.0f32);
        for p in &live {
            let off = (h * TAU - p.src_hue + PI).rem_euclid(TAU) - PI;
            let rel = 0.5 + off / (2.0 * POINT_HUE_HALF_SPAN);
            let w = gate
                * trapezoid(rel, &p.hue_range)
                * trapezoid(s, &p.sat_range)
                * trapezoid(l, &p.lum_range);
            dh += w * p.hue_shift;
            ds += w * p.sat_scale;
            dl += w * p.lum_scale;
        }
        if dh == 0.0 && ds == 0.0 && dl == 0.0 {
            return;
        }
        let new_h = (h + dh.clamp(-1.0, 1.0) * POINT_HUE_SHIFT_TURNS).rem_euclid(1.0);
        let new_s = (s * (1.0 + ds.clamp(-1.0, 1.0))).clamp(0.0, 1.0);
        let new_l = (l * (1.0 + POINT_LUM_REACH * dl.clamp(-1.0, 1.0))).clamp(0.0, 1.0);
        let (r, g, b) = hsl_to_rgb(new_h, new_s, new_l);
        *px = [r, g, b];
    });
}

pub(super) fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(super) fn to_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}
