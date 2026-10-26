//! AI lens blur (2026-10-13, user: 「AI 景深虚化」 — the AI estimates depth and
//! blurs the background progressively; the user picks the focus point).
//!
//! The depth map is a 16-bit nearness PNG `python/depth.py` (MoGe-2) writes
//! into the photo's develop folder: 65535 the nearest surface, 0 the farthest
//! and the sky. The blur radius at a pixel follows how far its nearness is from
//! the focus point's — the thin-lens circle of confusion is proportional to
//! |1/d − 1/d_focus|, and nearness IS normalised 1/d — so the foreground blurs
//! as well as the background, by the same law. `range` widens the band around
//! the focus that stays sharp; `amount` sets the largest radius, as a share of
//! the frame's long edge, so the preview and a full-resolution export blur the
//! same picture.
//!
//! The blur runs in linear light (bokeh is a sum of light, and in gamma space a
//! highlight's spread comes out dull). It is LAYERED: [`LEVELS`] Gaussian
//! blurs at evenly spaced radii, and each pixel takes the two levels
//! bracketing its own radius. Each level averages only pixels that are
//! themselves at least half that blurred, so an in-focus subject does not
//! bleed a halo of its own colour into the blurred background around it.
//!
//! Like a pixel layer it runs LAST in the develop, on the finished picture, at
//! stored (pre-geometry) coordinates, and like one it has no place in a
//! Lightroom sidecar: the save line names it.
//!
//! **Protection (v1.14.0, the Portraits mode).** A blur may name a soft mask
//! raster, [`LensBlur::protect`] — in practice the subject matte — and every
//! pixel's radius is multiplied by `1 − alpha` of it, so white keeps the sharp
//! source and the mask's own soft edge is the transition. The depth map still
//! decides everything outside it. A protected pixel's radius is 0, so the halo
//! guard below also keeps it out of every blur level: the person does not
//! bleed into the background beside them. Lightroom's own Lens Blur has no
//! such input, which changes nothing about the sidecar — the blur was never
//! written there — and the matte itself is an ordinary bitmap mask, skipped by
//! the writer and named on the save line like any other.

use super::layers::{decode_cached, load_budgeted, DecodeSlot, RasterKind};
use super::mask_raster::{sample_gray_norm, sample_texels_norm};
use super::*;

/// The lens blur a card carries. See the module docs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LensBlur {
    /// The 16-bit nearness PNG, in the photo's develop folder.
    pub depth_map: String,
    /// The focus point, 0..=1 across and down the develop's (uncropped) frame.
    pub focus: [f32; 2],
    /// 0..=100: the largest blur radius, as a share of [`MAX_RADIUS_SHARE`].
    pub amount: f32,
    /// 0..=100: how wide the sharp band around the focus distance is.
    pub range: f32,
    /// Off = carried and saved, not drawn.
    pub enabled: bool,
    /// An 8-bit soft mask raster (white = keep sharp) in the photo's develop
    /// folder, or `None` for no protection — see the module docs. Named by its
    /// raster path, the way `depth_map` and every bitmap mask name theirs.
    ///
    /// **Skipped when absent**, by `EditRecipe::lens_blur`'s own argument: a
    /// blur without protection serialises byte for byte as before. A build
    /// from before v1.14.0 refuses a recipe that carries one, by field name
    /// (`deny_unknown_fields` above) — the class-① forward break, confined to
    /// the recipes that use it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protect: Option<String>,
}

impl Default for LensBlur {
    fn default() -> Self {
        LensBlur { depth_map: String::new(), focus: [0.5, 0.5], amount: 50.0, range: 10.0, enabled: true, protect: None }
    }
}

/// The largest blur radius at `amount` 100, as a share of the long edge —
/// 2 % is a fast portrait lens' background on a full frame, and past it the
/// layered approximation starts to show its steps.
pub const MAX_RADIUS_SHARE: f32 = 0.02;
/// Blur levels between sharp and the largest radius.
pub const LEVELS: usize = 8;
/// Nearness the sharp band spans at `range` 100 (either side of the focus).
const BAND_AT_FULL_RANGE: f32 = 0.25;
/// Nearness beyond the band at which the blur reaches its largest radius.
const RAMP: f32 = 0.35;

impl LensBlur {
    /// Does this blur change any pixel?
    pub fn draws(&self) -> bool {
        self.enabled && self.amount > 0.0 && !self.depth_map.is_empty()
    }

    /// Clamp every number to its range; a non-finite one takes its default.
    pub fn clamp(&mut self) {
        let d = LensBlur::default();
        let fix = |v: f32, lo: f32, hi: f32, dv: f32| if v.is_finite() { v.clamp(lo, hi) } else { dv };
        self.amount = fix(self.amount, 0.0, 100.0, d.amount);
        self.range = fix(self.range, 0.0, 100.0, d.range);
        self.focus = [fix(self.focus[0], 0.0, 1.0, 0.5), fix(self.focus[1], 0.0, 1.0, 0.5)];
    }
}

pub(super) type DepthImage = image::ImageBuffer<image::Luma<u16>, Vec<u16>>;

const DEPTH: RasterKind = RasterKind { what: "depth map", unloaded: "the lens blur is not drawn" };
/// Not drawing is the honest fallback for an unreadable protect mask: a blur
/// drawn without it would blur exactly what the photographer asked to keep.
const PROTECT: RasterKind = RasterKind { what: "lens blur protect mask", unloaded: "the lens blur is not drawn" };

/// Load the recipe's depth map under the raster budget the masks and layers
/// share ([`load_budgeted`]: an export refuses an unreadable map, the preview
/// skips it). A re-estimate writes a new claimed name, so two cache entries
/// (the live card and the one it was copied from) are all the hot path needs.
pub(super) fn load_depth(
    recipe: &EditRecipe,
    budget_bytes: usize,
    held: &mut usize,
    strict: bool,
    diag: &crate::diag::Diag<'_>,
) -> Result<Option<std::sync::Arc<DepthImage>>> {
    static DECODED: std::sync::OnceLock<std::sync::Mutex<Vec<DecodeSlot<DepthImage>>>> =
        std::sync::OnceLock::new();
    let Some(lb) = recipe.lens_blur.as_ref().filter(|l| l.draws()) else {
        return Ok(None);
    };
    let path = lb.depth_map.as_str();
    let decode = || {
        decode_cached(DECODED.get_or_init(Default::default), 2, path, &DEPTH, |img| img.to_luma16(), diag)
            .map(|img| {
                let bytes = img.as_raw().len() * 2;
                (img, bytes)
            })
    };
    load_budgeted(path, &DEPTH, 2, budget_bytes, held, strict, diag, decode)
}

/// Load the blur's protect mask, when it names one, under the same budget and
/// with the same strictness as [`load_depth`].
pub(super) fn load_protect(
    recipe: &EditRecipe,
    budget_bytes: usize,
    held: &mut usize,
    strict: bool,
    diag: &crate::diag::Diag<'_>,
) -> Result<Option<std::sync::Arc<image::GrayImage>>> {
    static DECODED: std::sync::OnceLock<std::sync::Mutex<Vec<DecodeSlot<image::GrayImage>>>> =
        std::sync::OnceLock::new();
    let Some(path) = recipe.lens_blur.as_ref().filter(|l| l.draws()).and_then(|l| l.protect.as_deref()) else {
        return Ok(None);
    };
    let decode = || {
        decode_cached(DECODED.get_or_init(Default::default), 2, path, &PROTECT, |img| img.to_luma8(), diag)
            .map(|img| {
                let bytes = img.as_raw().len();
                (img, bytes)
            })
    };
    load_budgeted(path, &PROTECT, 1, budget_bytes, held, strict, diag, decode)
}

/// The depth map's nearness (0..=1) resampled to `w`×`h` at pixel centres,
/// through the mask rasters' texel-centre reading — a 1536 px map lands where
/// it was estimated on a full-resolution export too.
fn nearness_at(depth: &DepthImage, w: usize, h: usize) -> Vec<f32> {
    let texel = |x: u32, y: u32| depth.get_pixel(x, y)[0] as f32 / 65535.0;
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let ny = (y as f32 + 0.5) / h as f32;
        for (x, o) in row.iter_mut().enumerate() {
            *o = sample_texels_norm(depth.width(), depth.height(), (x as f32 + 0.5) / w as f32, ny, texel);
        }
    });
    out
}

/// The focus point's nearness: the median over a small window round it, so a
/// click that lands on a one-pixel gap in a railing still reads the railing.
pub(super) fn focus_nearness(near: &[f32], w: usize, h: usize, focus: [f32; 2]) -> f32 {
    let r = (w.max(h) / 200).max(2) as isize;
    let cx = ((focus[0] * w as f32) as isize).clamp(0, w as isize - 1);
    let cy = ((focus[1] * h as f32) as isize).clamp(0, h as isize - 1);
    let mut v: Vec<f32> = Vec::new();
    for y in (cy - r).max(0)..=(cy + r).min(h as isize - 1) {
        for x in (cx - r).max(0)..=(cx + r).min(w as isize - 1) {
            v.push(near[y as usize * w + x as usize]);
        }
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// Each pixel's blur radius in pixels for a `w`×`h` frame, and the largest.
/// With a protect mask every radius is scaled by `1 − alpha` there (the mask
/// read at pixel centres in normalised coordinates, as every mask raster is);
/// without one the field is exactly the depth law's.
fn radii(lb: &LensBlur, near: &[f32], w: usize, h: usize, protect: Option<&image::GrayImage>) -> (Vec<f32>, f32) {
    let max_r = lb.amount / 100.0 * MAX_RADIUS_SHARE * w.max(h) as f32;
    let nf = focus_nearness(near, w, h, lb.focus);
    let band = lb.range / 100.0 * BAND_AT_FULL_RANGE;
    let mut r: Vec<f32> =
        near.par_iter().map(|n| ((n - nf).abs() - band).clamp(0.0, RAMP) / RAMP * max_r).collect();
    if let Some(mask) = protect {
        r.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let ny = (y as f32 + 0.5) / h as f32;
            for (x, v) in row.iter_mut().enumerate() {
                let alpha = sample_gray_norm(mask, (x as f32 + 0.5) / w as f32, ny).clamp(0.0, 1.0);
                *v *= 1.0 - alpha;
            }
        });
    }
    (r, max_r)
}

/// One running-sum box pass along each row of `w`, radius `rb`, edges extended.
fn box_rows(src: &[[f32; 4]], dst: &mut [[f32; 4]], w: usize, rb: usize) {
    let norm = 1.0 / (2 * rb + 1) as f32;
    dst.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, row)| {
        let at = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
        let mut acc = [0f32; 4];
        for i in -(rb as isize)..=(rb as isize) {
            let p = at(i);
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
        for (x, o) in out.iter_mut().enumerate() {
            *o = acc.map(|a| a * norm);
            let (add, sub) = (at(x as isize + rb as isize + 1), at(x as isize - rb as isize));
            for c in 0..4 {
                acc[c] += add[c] - sub[c];
            }
        }
    });
}

/// `src` is rows of `w`; `dst` becomes rows of `h` (columns of `src`).
fn transpose(src: &[[f32; 4]], dst: &mut [[f32; 4]], w: usize, h: usize) {
    dst.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, o) in col.iter_mut().enumerate() {
            *o = src[y * w + x];
        }
    });
}

/// An approximate Gaussian of `sigma` (three box passes each way) over a
/// premultiplied RGBW buffer, in place.
fn gaussian(buf: &mut Vec<[f32; 4]>, w: usize, h: usize, sigma: f32) {
    let rb = sigma.round().max(1.0) as usize;
    let mut tmp = vec![[0f32; 4]; w * h];
    for (len, other) in [(w, h), (h, w)] {
        for _ in 0..3 {
            box_rows(buf, &mut tmp, len, rb);
            std::mem::swap(buf, &mut tmp);
        }
        transpose(buf, &mut tmp, len, other);
        std::mem::swap(buf, &mut tmp);
    }
}

/// Blur `data` (the develop's sRGB-gamma [0, 1] frame) by depth, in place,
/// keeping `protect`'s white sharp when one is given.
pub(super) fn apply(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    lb: &LensBlur,
    depth: &DepthImage,
    protect: Option<&image::GrayImage>,
) {
    if w == 0 || h == 0 || !lb.draws() {
        return;
    }
    let near = nearness_at(depth, w, h);
    let (radius, max_r) = radii(lb, &near, w, h, protect);
    if max_r < 0.5 {
        return;
    }
    let lin: Vec<[f32; 3]> = data.par_iter().map(|p| p.map(wb::srgb_to_linear)).collect();
    // Position on the level ladder: 0 = sharp, LEVELS = the largest radius.
    let pos: Vec<f32> = radius.par_iter().map(|r| r / max_r * LEVELS as f32).collect();
    let mut out: Vec<[f32; 3]> =
        lin.par_iter().zip(&pos).map(|(p, t)| p.map(|c| c * (1.0 - t).max(0.0))).collect();
    for k in 1..=LEVELS {
        let kf = k as f32;
        // A level only matters to pixels within one step of it.
        if !pos.par_iter().any(|t| (t - kf).abs() < 1.0) {
            continue;
        }
        let rk = max_r * kf / LEVELS as f32;
        // Only pixels at least half this blurred feed it (the halo guard); a
        // sliver of weight on the rest keeps an all-sharp neighbourhood defined.
        let mut buf: Vec<[f32; 4]> = lin
            .par_iter()
            .zip(&radius)
            .map(|(p, r)| {
                let wt = if *r >= 0.5 * rk { 1.0 } else { 1e-3 };
                [p[0] * wt, p[1] * wt, p[2] * wt, wt]
            })
            .collect();
        gaussian(&mut buf, w, h, rk * 0.5);
        out.par_iter_mut().zip(&buf).zip(&pos).for_each(|((o, b), t)| {
            let tri = 1.0 - (t - kf).abs();
            if tri > 0.0 && b[3] > 0.0 {
                for c in 0..3 {
                    o[c] += b[c] / b[3] * tri;
                }
            }
        });
    }
    data.par_iter_mut().zip(&out).for_each(|(d, o)| *d = o.map(wb::linear_to_srgb));
}
