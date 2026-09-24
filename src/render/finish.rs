//! Lightroom's Effects panel, rendered (v1.5.0) — the post-crop vignette and
//! film grain — together with the tail of the render that reaches the frame
//! they act on.
//!
//! # Why the geometry tail lives here
//!
//! "Post-crop" is a statement about ORDER. This engine develops pixels first
//! and runs the geometric chain afterwards (lens geometry → straighten →
//! crop), so an operator defined on the cropped rectangle cannot live in
//! `apply_develop` at all: at that point the crop has not happened and the
//! frame is still the whole sensor. That is exactly the reason the nine
//! Effects controls were `Tier::CarriedOnly` until now — they round-tripped
//! through the sidecar and moved no pixel here.
//!
//! [`frame_and_finish`] is that tail, written ONCE for the four surfaces that
//! each spelled it out separately (the RAW render, the baked render, the GUI
//! preview and the web preview) and could therefore drift — two of them
//! already wrote the same geometry gate as two different expressions.
//!
//! # Where the crop is
//!
//! An export cuts the crop out ([`CropPolicy::Cut`]) and the finishing pass
//! then sees the delivered frame itself. Both previews stay full-frame on
//! purpose — whole-frame slider feedback — and pass [`CropPolicy::Keep`],
//! which POSITIONS the two operators on the crop rectangle without cutting it.
//! The canvas therefore shows the vignette where the export will put it, and
//! the grain lattice is anchored to the same corner of the same rectangle, so
//! what a photographer judges on screen is what ships.
//!
//! # Film pixels
//!
//! Grain is stated in pixels of the full-resolution frame, like the Detail
//! panel's radii, and converted through [`FilmScale`]. A preview therefore
//! shows grain at the size the export's grain will appear once it is
//! downscaled to that preview — not the export's individual grains, which no
//! downscaled raster can carry.
//!
//! # Deviations, stated
//!
//! Adobe publishes no model for either operator. Every constant here is
//! first-principles, named, and provisional until the Lightroom kit's `PCV-*`
//! and `GRAIN-*` ladders measure the real thing against exported pixels.

use image::DynamicImage;
use rayon::prelude::*;

use super::detail::Ramp;
use super::{
    luma601, sample_lut, smoothstep, to_u16, to_u8, transfer_luts, FilmScale,
    MASK_SAMPLE_CENTRE,
};
use crate::recipe::{Crop, EditRecipe, LensProfile};

/// What a surface does with the crop rectangle before the finishing pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropPolicy {
    /// Cut the crop out — what a render that delivers pixels does.
    Cut,
    /// Keep the whole frame and only POSITION the finishing pass on the crop
    /// rectangle. What both previews do, because they show the whole frame
    /// while a slider moves and the crop is drawn as an overlay on top.
    Keep,
}

/// The post-crop frame, in the pixels of the buffer being finished.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CropRect {
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
}

impl CropRect {
    /// The whole buffer — what [`CropPolicy::Cut`] leaves behind.
    fn whole(w: u32, h: u32) -> CropRect {
        CropRect { x0: 0.0, y0: 0.0, w: w.max(1) as f32, h: h.max(1) as f32 }
    }

    /// Where `crop` lands in a buffer that still holds the whole frame. The
    /// normalised rectangle is defined on exactly this frame — `render.rs`
    /// applies distortion and straighten BEFORE the crop, and this runs after
    /// both — so the mapping is a plain multiply, the same one the GUI's
    /// histogram uses.
    fn of(w: u32, h: u32, crop: Option<&Crop>) -> CropRect {
        let whole = CropRect::whole(w, h);
        let Some(c) = crop else { return whole };
        let (x0, y0) = (c.left.clamp(0.0, 1.0) * whole.w, c.top.clamp(0.0, 1.0) * whole.h);
        let (x1, y1) = (c.right.clamp(0.0, 1.0) * whole.w, c.bottom.clamp(0.0, 1.0) * whole.h);
        // A crop that arrives inverted or empty is not a frame to centre an
        // ellipse on; the whole buffer is the honest fallback (`apply_crop`
        // makes the same call for the same reason).
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
            return whole;
        }
        CropRect { x0, y0, w: x1 - x0, h: y1 - y0 }
    }

    fn cx(self) -> f32 {
        self.x0 + self.w * 0.5
    }

    fn cy(self) -> f32 {
        self.y0 + self.h * 0.5
    }
}

// ── the post-crop vignette ───────────────────────────────────────────────────

/// Amount ±100 is this many stops of exposure at the corner of the crop.
/// Lightroom's own −100 darkens hard without reaching black, which four stops
/// (a sixteenth) describes; the exposure domain also makes + and − symmetric
/// instead of one saturating before the other (`PCV-100`, `PCV+100`).
const VIGNETTE_STOPS: f32 = 4.0;

/// Where the falloff sits, as a fraction of the distance from the crop's
/// centre to its corner, over Midpoint 0..100 (`PCV-M0`, `PCV-M100`).
const MID_RADIUS: Ramp = Ramp(0.40, 1.00);

/// Half the width of the transition, in the same units, over Feather 0..100:
/// a hard edge at 0, most of the frame at 100 (`PCV-F0`, `PCV-F100`).
const FEATHER_HALF: Ramp = Ramp(0.02, 0.60);

/// The superellipse exponent over NEGATIVE roundness 0..−100. 2 is an ellipse
/// inscribed in the crop; higher exponents push the contour out toward the
/// crop's own edges, which is the "more oval / rectangular" end of Adobe's
/// description (`PCV-R-100`).
const ROUND_EXP: Ramp = Ramp(2.0, 6.0);

/// The linear-light window over which Highlights opens: a pixel below the
/// first number is darkened in full, one above the second is spared as far as
/// the slider asks (`PCV-HL100`).
const HL_OPEN: (f32, f32) = (0.35, 1.0);

/// Lightroom's three Styles. They differ in ONE respect each, exactly as
/// Adobe's own descriptions differ: whether the highlight recovery is computed
/// per channel (which is what shifts colour), from the pixel's luminance (which
/// cannot), or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// 1 — Highlight Priority: "enables highlight recovery but can lead to
    /// colour shifts in darkened areas".
    Highlight,
    /// 2 — Colour Priority: "minimises colour shifts ... cannot perform
    /// highlight recovery" to the same degree.
    Colour,
    /// 3 — Paint Overlay: "mixes the cropped image values with black or white
    /// pixels. Can result in a flat appearance."
    Paint,
}

/// The post-crop vignette, resolved from the recipe.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Vignette {
    /// Signed: the exposure change where the falloff is complete.
    stops: f32,
    /// Radius of the transition's centre, 1 = the crop's corner.
    mid: f32,
    /// Half-width of the transition, same units.
    half: f32,
    /// Superellipse exponent (2 = the inscribed ellipse).
    exp: f32,
    /// 0 = the crop's own aspect, 1 = a circle in pixel space (Roundness > 0,
    /// Adobe's "more circular").
    circle: f32,
    style: Style,
    /// Highlights, 0..=1.
    hl: f32,
}

impl Vignette {
    /// `None` when the Amount is zero — the one slider that turns the operator
    /// on, exactly as the sidecar treats it (the other five are companions and
    /// mean nothing on their own).
    fn of(r: &EditRecipe) -> Option<Vignette> {
        if r.post_crop_vignette == 0.0 {
            return None;
        }
        // The five companions read through `resolved`: an absent key means
        // Lightroom's own default (Midpoint 50, Feather 50, Style 1), not 0,
        // and rendering a stored 0 as 0 would put a vignette on the frame that
        // Lightroom does not put there.
        let round = (r.post_crop_vignette_round / 100.0).clamp(-1.0, 1.0);
        Some(Vignette {
            stops: VIGNETTE_STOPS * (r.post_crop_vignette / 100.0).clamp(-1.0, 1.0),
            mid: MID_RADIUS.at((r.resolved("post_crop_vignette_mid") / 100.0).clamp(0.0, 1.0)),
            half: FEATHER_HALF
                .at((r.resolved("post_crop_vignette_feather") / 100.0).clamp(0.0, 1.0))
                .max(1e-3),
            exp: ROUND_EXP.at((-round).max(0.0)),
            circle: round.max(0.0),
            style: match r.resolved("post_crop_vignette_style").round() as i32 {
                2 => Style::Colour,
                3 => Style::Paint,
                _ => Style::Highlight,
            },
            hl: (r.post_crop_vignette_hl / 100.0).clamp(0.0, 1.0),
        })
    }

    /// How far into the falloff this pixel is, 0 (untouched centre) to 1.
    fn weight(&self, rect: CropRect, x: f32, y: f32) -> f32 {
        let (mut ax, mut ay) = (rect.w * 0.5, rect.h * 0.5);
        if self.circle > 0.0 {
            // Roundness > 0 blends both half-extents toward the SHORTER one:
            // at +100 the contour is a circle in PIXEL space whatever the
            // crop's aspect (Adobe's "more circular"), and a circle inscribed
            // in the short edge is the one that still darkens — blending
            // toward the LONG edge would grow the contour past the frame and
            // lighten the picture instead of rounding the vignette.
            let short = ax.min(ay);
            ax += (short - ax) * self.circle;
            ay += (short - ay) * self.circle;
        }
        let u = ((x - rect.cx()) / ax.max(1e-3)).abs();
        let v = ((y - rect.cy()) / ay.max(1e-3)).abs();
        // r = 1 at the corner of the contour for every exponent, so Midpoint
        // means the same fraction of the way out whatever Roundness is.
        let r = if self.exp == 2.0 {
            (u * u + v * v).sqrt() * std::f32::consts::FRAC_1_SQRT_2
        } else {
            (u.powf(self.exp) + v.powf(self.exp)).powf(1.0 / self.exp)
                / 2f32.powf(1.0 / self.exp)
        };
        smoothstep(self.mid - self.half, self.mid + self.half, r)
    }

    /// One pixel, in the gamma-encoded working domain.
    fn apply(&self, px: &mut [f32; 3], w: f32, to_lin: &[f32], to_gam: &[f32]) {
        if w <= 0.0 {
            return;
        }
        if self.style == Style::Paint {
            // A flat mix toward black or white, in the encoded domain — the
            // "flat appearance" is the point, so no transfer and no recovery.
            let t = (w * (self.stops / VIGNETTE_STOPS).abs()).clamp(0.0, 1.0);
            let target = if self.stops < 0.0 { 0.0 } else { 1.0 };
            for c in px.iter_mut() {
                *c += (target - *c) * t;
            }
            return;
        }
        let lin = [sample_lut(to_lin, px[0]), sample_lut(to_lin, px[1]), sample_lut(to_lin, px[2])];
        // Adobe's Highlights slider acts "when Amount is negative": there is
        // nothing to spare in a vignette that brightens.
        let recovering = self.hl > 0.0 && self.stops < 0.0;
        let scalar = (self.style == Style::Colour).then(|| luma601(&lin));
        for (c, out) in px.iter_mut().enumerate() {
            let keep = if recovering {
                // Colour Priority reads ONE number for the pixel, so the three
                // channels keep their ratios and the hue cannot move; Highlight
                // Priority reads each channel's own level, which recovers a
                // clipped channel further than its neighbours — the colour
                // shift Adobe warns about, and the reason the two styles exist.
                self.hl * smoothstep(HL_OPEN.0, HL_OPEN.1, scalar.unwrap_or(lin[c]))
            } else {
                0.0
            };
            let gain = (self.stops * w * (1.0 - keep)).exp2();
            *out = sample_lut(to_gam, (lin[c] * gain).clamp(0.0, 1.0));
        }
    }
}

// ── film grain ───────────────────────────────────────────────────────────────

/// The amplitude at Grain 100, in the gamma-encoded domain, at the tone where
/// grain is strongest (`GRAIN-100`).
const GRAIN_SIGMA: Ramp = Ramp(0.0, 0.09);

/// The size of one grain lattice cell in FILM pixels over Size 0..100
/// (`GRAIN-S0`, `GRAIN-S100`; Lightroom's default Size is 25).
const GRAIN_CELL_FILM: Ramp = Ramp(1.0, 12.0);

/// How much coarser the clumping octave is than the grain itself. Roughness
/// mixes it in as an amplitude modulation, which is what makes film grain
/// uneven rather than merely larger (`GRAIN-R0`, `GRAIN-R100`).
const GRAIN_CLUMP: f32 = 4.0;

/// Grain fades out into pure black and pure white — film has no grain where it
/// has no silver and none where it is fully exposed.
const GRAIN_FADE: (f32, f32) = (0.12, 0.88);

/// Film grain, resolved from the recipe and the raster's scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Grain {
    sigma: f32,
    /// Lattice cell in RASTER pixels (from film pixels through [`FilmScale`]).
    cell: f32,
    rough: f32,
}

impl Grain {
    /// `None` when the Amount is zero (Size and Roughness are companions and
    /// say nothing on their own).
    fn of(r: &EditRecipe, film: FilmScale) -> Option<Grain> {
        if r.grain == 0.0 {
            return None;
        }
        let size = (r.resolved("grain_size") / 100.0).clamp(0.0, 1.0);
        Some(Grain {
            sigma: GRAIN_SIGMA.at((r.grain / 100.0).clamp(0.0, 1.0)),
            cell: film.raster_px(GRAIN_CELL_FILM.at(size)).max(0.25),
            rough: (r.resolved("grain_rough") / 100.0).clamp(0.0, 1.0),
        })
    }

    /// One pixel, in the gamma-encoded working domain. `x`/`y` are measured
    /// from the crop's own corner, so a preview and the export it predicts put
    /// the lattice in the same place.
    fn apply(&self, px: &mut [f32; 3], x: f32, y: f32) {
        let tone = (px[0] + px[1] + px[2]) / 3.0;
        let shape = smoothstep(0.0, GRAIN_FADE.0, tone) * (1.0 - smoothstep(GRAIN_FADE.1, 1.0, tone));
        if shape <= 0.0 {
            return;
        }
        let clump = 1.0 + self.rough * value_noise(x, y, self.cell * GRAIN_CLUMP, 7);
        let d = self.sigma * value_noise(x, y, self.cell, 0) * clump.max(0.0) * shape;
        for c in px.iter_mut() {
            // ONE offset for all three channels: film grain is a luminance
            // texture, and per-channel noise would read as colour speckle —
            // which is what the noise reduction upstream exists to remove.
            *c = (*c + d).clamp(0.0, 1.0);
        }
    }
}

/// A deterministic value in `[0, 1)` for one lattice point.
///
/// Not a random number generator: the grain has to be the same in the preview,
/// in the export, and in the export repeated tomorrow, so the value is a pure
/// function of the lattice coordinate. (Integer hash, xorshift-multiply.)
fn hash01(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B9) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Value noise in `[-1, 1]`: bilinear interpolation between lattice points on a
/// `cell`-pixel grid, with a smoothstep fade so the cells do not show as a
/// diamond pattern. `salt` picks an independent field for the second octave.
fn value_noise(x: f32, y: f32, cell: f32, salt: i32) -> f32 {
    let (u, v) = (x / cell, y / cell);
    let (ux, uy) = (u.floor(), v.floor());
    let (fx, fy) = (u - ux, v - uy);
    let (fx, fy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (ux as i32, uy as i32);
    let at = |dx: i32, dy: i32| hash01(ix + dx + salt * 131, iy + dy - salt * 37);
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * fx;
    let bot = at(0, 1) + (at(1, 1) - at(0, 1)) * fx;
    (top + (bot - top) * fy) * 2.0 - 1.0
}

// ── the tail ─────────────────────────────────────────────────────────────────

/// Lens geometry → TRANSFORM → straighten → crop → the finishing pass, for
/// every surface that turns a developed buffer into the frame a person looks at.
///
/// The Transform stage (v1.5.0 F6, `render::perspective`) sits between the lens
/// resample and the straighten because that is where Lightroom's own panel
/// order puts it — a keystone corrects the camera's attitude, which is a fact
/// about the lens's view, while the straighten belongs to the crop. Its masks
/// need no bookkeeping for the same reason `straighten_deg` needs none: the
/// resample carries mask pixels along with everything else, and
/// `MaskFrame::downstream` speaks only for the LENS geometry, whose profile it
/// is handed.
///
/// `geom` is the COMPOSED lens profile ([`super::geometry_profile`], which
/// folds the manual CA pair onto the camera's own knots), and `film` is the
/// develop's own [`FilmScale`] — the one the Detail panel used, not one
/// recomputed from the cropped dimensions, because cropping changes the frame's
/// size and not the size of a film pixel.
pub fn frame_and_finish(
    img: DynamicImage,
    r: &EditRecipe,
    geom: &LensProfile,
    film: FilmScale,
    policy: CropPolicy,
) -> DynamicImage {
    let mut img = img;
    // The SAME gate `MaskFrame::downstream` answers `warps()` with, so the mask
    // chain's frame adaptation and this resample stay one decision.
    if geom.geometry_active() || r.lens_distortion != 0.0 {
        img = super::apply_lens_geometry(&img, geom, r.lens_distortion);
    }
    // Lightroom's Transform panel. `transform` answers `None` for every photo
    // whose eight Perspective keys sit at neutral with Upright off, which is
    // 162 of the 175 sidecars in this operator's library — so the ordinary
    // render pays one comparison and no resample.
    let warp = super::perspective::transform(r, &img);
    if let Some(h) = warp {
        img = super::perspective::apply(&img, h);
    }
    if r.straighten_deg != 0.0 {
        img = super::rotate_straighten(&img, r.straighten_deg);
    }
    // Constrain Crop (`crs:CropConstrainToWarp`) shrinks the crop until it lies
    // inside the warped frame. Measured `"0"` on all 52 of this library's
    // sidecars that carry it, so the usual answer is that the crop stands and
    // the photographer's own hand crop is what removes the empty corners —
    // which is exactly what those sidecars show.
    //
    // The rect is derived in the WARPED frame, which is the crop's own frame
    // whenever `straighten_deg` is 0 — true of every photo in this library that
    // carries the flag, and of both that carry a manual Perspective slider. A
    // non-zero straighten re-inscribes the frame between the two, so the rect
    // would be off by that factor; the straighten is `CropAngle`, so pairing it
    // with Constrain Crop is a shape no sidecar here has and this is the stated
    // limit rather than a silent one.
    let constrained = warp
        .filter(|_| r.crop_constrain_to_warp)
        .and_then(|h| {
            // The dimensions of the frame the crop is defined on — after the
            // warp and the straighten, which is where `apply_crop` will
            // quantise it.
            super::perspective::constrain_crop(h, r.crop.as_ref(), (img.width(), img.height()))
        });
    let crop = constrained.as_ref().or(r.crop.as_ref());
    let rect = match policy {
        CropPolicy::Cut => {
            img = super::apply_crop(img, crop);
            CropRect::whole(img.width(), img.height())
        }
        CropPolicy::Keep => CropRect::of(img.width(), img.height(), crop),
    };
    finish(&mut img, r, rect, film);
    img
}

/// The finishing pass alone: post-crop vignette, then grain, on the frame as it
/// will be delivered. Ordered as Lightroom's own panel is, and as the physics
/// reads — the vignette is a lens-like gain on the picture, the grain is the
/// film it is printed on.
fn finish(img: &mut DynamicImage, r: &EditRecipe, rect: CropRect, film: FilmScale) {
    let vignette = Vignette::of(r);
    let grain = Grain::of(r, film);
    if vignette.is_none() && grain.is_none() {
        return;
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    match img {
        DynamicImage::ImageRgb16(b) => {
            let px = b.as_flat_samples_mut().samples;
            run(px, w, h, rect, vignette, grain, |s| s as f32 / 65535.0, to_u16);
        }
        DynamicImage::ImageRgb8(b) => {
            let px = b.as_flat_samples_mut().samples;
            run(px, w, h, rect, vignette, grain, |s| s as f32 / 255.0, to_u8);
        }
        other => {
            // Every producer in this engine hands the tail an Rgb8 preview or
            // an Rgb16 render; anything else converts once here rather than
            // growing a third traversal that nothing exercises.
            let mut rgb = other.to_rgb16();
            let px = rgb.as_flat_samples_mut().samples;
            run(px, w, h, rect, vignette, grain, |s| s as f32 / 65535.0, to_u16);
            *other = DynamicImage::ImageRgb16(rgb);
        }
    }
}

/// The traversal, once, for both sample depths.
#[allow(clippy::too_many_arguments)] // the buffer's geometry, the two operators and the sample pair
fn run<S: Copy + Send + Sync>(
    buf: &mut [S],
    w: usize,
    h: usize,
    rect: CropRect,
    vignette: Option<Vignette>,
    grain: Option<Grain>,
    dec: impl Fn(S) -> f32 + Sync,
    enc: impl Fn(f32) -> S + Sync,
) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts even on an empty slice (U14)
    }
    let (to_lin, to_gam) = transfer_luts();
    buf.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let fy = y as f32;
        for (x, px) in row.chunks_exact_mut(3).enumerate() {
            let fx = x as f32;
            let mut v = [dec(px[0]), dec(px[1]), dec(px[2])];
            if let Some(vg) = vignette {
                // At the PIXEL CENTRE, like every other spatial read in this
                // engine (`MASK_SAMPLE_CENTRE`): the crop's `cx()` / `cy()`
                // are continuous centres, and sampling the corner put the
                // falloff half a pixel up-left of the rectangle it belongs to.
                let weight = vg.weight(rect, fx + MASK_SAMPLE_CENTRE, fy + MASK_SAMPLE_CENTRE);
                vg.apply(&mut v, weight, to_lin, to_gam);
            }
            if let Some(gr) = grain {
                gr.apply(&mut v, fx - rect.x0, fy - rect.y0);
            }
            for (o, value) in px.iter_mut().zip(v) {
                *o = enc(value);
            }
        }
    });
}

#[cfg(test)]
mod tests;
