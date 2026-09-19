//! Lightroom's Transform panel (v1.5.0 F6): ONE projective map, applied
//! between the lens-geometry resample and the straighten.
//!
//! R25 B4 made the eight `crs:Perspective*` keys the first and only members of
//! `Tier::PassThrough` — carried verbatim, never interpreted — because a
//! keystone is a frame operation and the engine had no frame operation to put
//! it in. This module is that operation.
//!
//! # The coordinate system is Adobe's own, and it was MEASURED
//!
//! Lightroom does publish the result of its Upright solver, one 3×3 matrix per
//! mode, in `crs:UprightTransform_0…5`. Over the 125 matrices in this
//! operator's library:
//!
//! * they are full projective maps — 30 of the 125 have a non-trivial bottom
//!   row, the strongest `h20 = −0.9458` — and not affine;
//! * `_0` is the identity in all 21 sidecars that carry the block (mode 0 is
//!   off) and `_5` in 20 of them (Guided, with no guides drawn);
//! * the frame centre in **[0,1] coordinates** is a fixed point to 8.6e-4 on
//!   the 64 near-identity ones, many of them to 5e-10, against 2.6e-2 for the
//!   sidecar's own `UprightCenterNorm` and 3.6e-1 for a [−1,1] centre. So the
//!   matrices live in [0,1] frame coordinates, pivoted on the frame centre;
//! * the normalisation is **per axis**, not aspect-aware: a 0.9786° rotation
//!   carries a scale of 1.017110, which is the UNIT SQUARE's cover factor
//!   `cos+sin = 1.016934` and not a 3:2 frame's 1.011237;
//! * Adobe has already folded the cover scale in. Inverting each of the 13
//!   matrices a photo actually SELECTED and mapping the four destination
//!   corners back, the worst excursion outside the source frame is
//!   +0.000000 — exactly covering, 13 of 13. So an Upright render needs no
//!   fill scaling of ours, and the empty corners a Transform can leave come
//!   from the MANUAL sliders alone.
//!
//! The seven manual sliders are therefore built in the same [0,1] space, so
//! Adobe's matrix and ours compose by plain multiplication.
//!
//! # The seven sliders were MEASURED too (2026-09-19)
//!
//! Adobe publishes no mapping from a Transform slider to a coefficient, so
//! [`KEYSTONE`], [`ASPECT_LOG`] and [`OFFSET`] began as this engine's own
//! calibration. The v1.5.0 Lightroom kit then exported fifteen `PERSP-*` cases
//! and each was fitted to a homography — a grid of phase-correlated blocks,
//! coarse to fine, the reference pre-warped by the running estimate each round,
//! taken WITHIN one renderer so demosaic and profile cancel. Eleven of the
//! fifteen converged to a residual under 0.05 px on 85–96 of 96 blocks, and the
//! instrument was checked against the two cases with an analytic answer before
//! any of it was believed.
//!
//! Five of the seven were wrong, and every constant below now carries the
//! reading that fixed it. What survived: `Scale` (measured 0.79995 × 0.80007 at
//! slider 80 and 1.19989 × 1.20011 at 120, against our `s/100`), and the
//! Upright path, whose four modes reproduce Lightroom to 3–4 decimal places
//! because they read Adobe's own matrix.
//!
//! The one thing the kit could not settle: every frame it exported is 3:2, so
//! the LENGTH UNIT the keystone divides by is indistinguishable between the
//! short edge, the long edge and the diagonal. [`KEYSTONE`] states which was
//! chosen and why, and a portrait or square frame is the case that would
//! falsify it.

use rayon::prelude::*;

use crate::recipe::{Crop, EditRecipe};
use image::{DynamicImage, ImageBuffer, Rgb};

mod upright;
pub(crate) use upright::solve_upright;

#[cfg(test)]
mod tests;

/// A projective map in [0,1] FRAME coordinates, row-major 3×3.
///
/// Row-major because that is the order Adobe writes
/// `crs:UprightTransform_N`'s nine comma-separated fields, so a sidecar's
/// matrix becomes one of these with no reordering to get wrong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f32; 9]);

/// Full-slider keystone strength, per SHORT EDGE of the frame.
///
/// MEASURED (2026-09-19). Five readings, two axes, three magnitudes, every one
/// at a residual under 0.05 px: `V+50` −0.65005, `V−50` −0.65137, `V+100`
/// −0.65365, `H+50` −0.65079, `H−50` −0.65136. Exactly linear in the slider
/// (the two vertical magnitudes stand at 0.32521 and 0.65050, a ratio of
/// 2.0002) and NEGATIVE, which is the opposite of the 1.0 this engine shipped
/// with — a full-throw keystone used to bend the frame the wrong way and half
/// again too hard.
///
/// **Per short edge, and that is a choice the kit could not make.** The two
/// axes do not share a coefficient in frame-normalised units: `H` reads 1.5009×
/// `V` on a 3:2 frame, exactly the aspect ratio, so Lightroom divides a PIXEL
/// offset by one length for both axes. Which length is unknowable from this kit,
/// where every frame is 3:2 — the short edge, the long edge and the diagonal all
/// reproduce these five numbers. The short edge is taken because it is
/// orientation-independent (turning the camera does not change which edge is
/// short) and because a keystone is physically `offset / focal length`, a ratio
/// with no reference to the frame's shape. A portrait or square export is the
/// one case that would tell it apart, and there is none.
///
/// # Lightroom also stretches the keystone's own axis, and this engine does not
///
/// A DELIBERATE, measured deviation (user ruling, 2026-09-19), recorded here
/// because it is the largest known gap between this stage and Lightroom.
///
/// Every `PERSP-V*`/`PERSP-H*` fit carries a scale along the keystone's own
/// axis, the other axis staying at 1.00: on `K3-REF` 1.27788 at slider 50 (four
/// readings across both axes, spread 0.00053) and 2.29547 at 100.
///
/// It was nearly shipped as a quadratic through those magnitudes. `PERSP14-V+50`
/// falsified that: the SAME slider of 50 on `K2-REF` stretches by 0.9487. The
/// two sidecars differ in exactly one thing — the lens, a SIGMA 14-24 at 15.5 mm
/// against a Sony 24-105 at 51 mm — so the stretch follows the LENS and not the
/// slider. And 0.9487 is below 1, which no `1/cos φ` can produce, so it is not a
/// physical tilt term either. Six readings over two photographs, with only one
/// magnitude on the second, do not identify a law with a free variable in it.
///
/// So no stretch is applied. The error is bounded and stated: −22 % on the
/// 51 mm frame, +5 % on the 15.5 mm one. A curve fitted to one lens would be
/// exact on that lens and 35 % wrong on the other, and would apply a 2.3×
/// stretch at the slider's end stop on evidence from a single photograph.
///
/// TO SETTLE IT: one photograph at V = 25, 50, 75, 100, plus one case at V = 50
/// on each of two or three further focal lengths. Ten exports.
const KEYSTONE: f32 = 0.65;

/// Full-slider Aspect, as a natural logarithm: ±100 stretches one axis by 1.1×
/// and compresses the other by the same factor, so the move is
/// area-preserving and symmetric about the neutral (`exp(+x)` against
/// `exp(−x)`, not `1+x` against `1−x`, which is not).
///
/// MEASURED (2026-09-19): `ASP+50` reads 0.095563 and `ASP−50` 0.095844, both at
/// a residual under 0.05 px, against `ln(1.1) = 0.095310` — and the frame's area
/// comes back 1.00035 and 0.99939, so Lightroom's Aspect is reciprocal and
/// area-preserving exactly as this engine already had it. Only the THROW was
/// wrong: `ln(1.5)` shipped, which is 4.25× too strong.
const ASPECT_LOG: f32 = 0.095_310_2; // ln(1.1)

/// Full-slider X/Y Offset, as a fraction of the frame's own width or height.
///
/// MEASURED (2026-09-19): `X+20` slides by +0.1621 of the width and `Y+20` by
/// −0.1625 of the height, both fitted as pure translations at a residual of
/// 0.00 px on 72–80 of 96 blocks, giving 0.81146 and 0.81272 per full slider.
/// This engine shipped 0.25 — it moved the frame 3.25× too little.
///
/// Normalised PER AXIS, unlike [`KEYSTONE`]: the two readings agree to 0.16 %
/// once each is divided by its own dimension, and would differ by the 1.5 aspect
/// under any shared length unit. So the two sliders really do speak different
/// languages, and that is measured rather than assumed.
///
/// LINEARITY IS NOT MEASURED. The kit moves each offset to 20 and no other
/// value, so one magnitude per axis fixes the coefficient and says nothing about
/// the shape between there and the end stop.
const OFFSET: f32 = 0.8121;

/// The pixel written where the map pulls from outside the source frame.
///
/// WHITE, and MEASURED rather than chosen (2026-09-19). The v1.5.0 Lightroom
/// kit was exported and four of its cases vacate part of the frame with
/// Constrain Crop off; every one fills what it vacates with pure white.
/// `PERSP-X+20`'s left column and `PERSP-Y+20`'s bottom row are 100.0 %
/// `255,255,255`, `PERSP-SCALE80` fills all four edges, and `PERSP-ROT+5`
/// fills the rotated corners — about half of each edge. The fill follows the
/// WARP and not the scene: on the same frame `PERSP-V+50` whitens the TOP
/// corners while `PERSP-V-50` whitens the BOTTOM ones, so this is Lightroom's
/// fill rather than a blown sky.
///
/// It was black through this batch's development, on the stated reasoning that
/// black is what every other "no data here" in this engine is, and that —
/// unlike the clamping samplers used by the lens resamplers — it cannot smear
/// an edge pixel outward into a radial band. The second half still holds and is
/// still why this is a constant and not a clamp. The first half was a sound
/// argument for a wrong answer, which is what the kit was for.
const VOID: Rgb<u16> = Rgb([u16::MAX, u16::MAX, u16::MAX]);

impl Homography {
    pub const IDENTITY: Homography = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// `self ∘ rhs` — apply `rhs` first.
    fn then(self, outer: Homography) -> Homography {
        let (a, b) = (outer.0, self.0);
        let mut m = [0.0f32; 9];
        for r in 0..3 {
            for c in 0..3 {
                m[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
            }
        }
        Homography(m)
    }

    /// Where `(x, y)` goes. `None` when the point sits on the map's own horizon,
    /// where the projective divide has no answer.
    pub fn map(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        if !w.is_finite() || w.abs() < 1e-9 {
            return None;
        }
        let (u, v) = ((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w);
        (u.is_finite() && v.is_finite()).then_some((u, v))
    }

    /// The inverse map, or `None` for a degenerate one.
    pub fn inverse(&self) -> Option<Homography> {
        let [a, b, c, d, e, f, g, h, k] = self.0;
        let (ca, cb, cc) = (e * k - f * h, f * g - d * k, d * h - e * g);
        let det = a * ca + b * cb + c * cc;
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let m = [
            ca,
            c * h - b * k,
            b * f - c * e,
            cb,
            a * k - c * g,
            c * d - a * f,
            cc,
            b * g - a * h,
            a * e - b * d,
        ];
        let inv = Homography(m.map(|v| v / det));
        inv.0.iter().all(|v| v.is_finite()).then_some(inv)
    }

    /// Conjugate a map written about the ORIGIN into one about the frame centre:
    /// `T(+½) · m · T(−½)`. Every slider below is built about the origin,
    /// because that is where a keystone's algebra is legible.
    fn about_centre(m: [f32; 9]) -> Homography {
        const HALF: f32 = 0.5;
        let to = Homography([1.0, 0.0, -HALF, 0.0, 1.0, -HALF, 0.0, 0.0, 1.0]);
        let back = Homography([1.0, 0.0, HALF, 0.0, 1.0, HALF, 0.0, 0.0, 1.0]);
        to.then(Homography(m)).then(back)
    }

    /// Scale about the frame centre by `s`, for the fill search.
    fn scaled(self, s: f32) -> Homography {
        self.then(Homography::about_centre([s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0]))
    }

    /// Does this map's image of the source frame COVER the whole destination
    /// frame? Answered on the inverse: every destination corner must pull from
    /// inside `[0,1]²`. The image of a square under a projective map has
    /// straight edges, so the four corners are the whole test.
    fn covers(&self) -> bool {
        let Some(inv) = self.inverse() else { return false };
        [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].iter().all(|&(x, y)| {
            inv.map(x, y).is_some_and(|(u, v)| (-1e-6..=1.0 + 1e-6).contains(&u) && (-1e-6..=1.0 + 1e-6).contains(&v))
        })
    }
}

/// The seven manual sliders as one map about the frame centre, or `None` when
/// every one of them is at rest.
///
/// `aspect` is the frame's width over its height. Two of the sliders need it:
/// the keystone divides a pixel offset by a single length for both axes, and
/// Rotate is a rigid rotation in pixels. The other five are pure functions of
/// the slider.
///
/// Composed in the order the panel reads top to bottom, because that is the
/// order a photographer thinks in and Adobe publishes no other: the two
/// keystones first (they are the camera-geometry correction), then Rotate,
/// Aspect, Scale and finally the two Offsets, which slide whatever the rest
/// produced.
pub(crate) fn manual(r: &EditRecipe, aspect: f32) -> Option<Homography> {
    // How many SHORT EDGES each normalised axis spans — the conversion between
    // the [0,1]-per-axis box this matrix lives in and the single pixel length
    // Lightroom's keystone was measured to divide by. See `KEYSTONE`.
    let (ux, uy) = if aspect >= 1.0 { (aspect, 1.0) } else { (1.0, 1.0 / aspect) };
    let kv = -(r.perspective_vertical / 100.0) * KEYSTONE * uy;
    let kh = -(r.perspective_horizontal / 100.0) * KEYSTONE * ux;
    let rot = r.perspective_rotate.to_radians();
    let g = ((r.perspective_aspect / 100.0) * ASPECT_LOG).exp();
    let s = r.perspective_scale / 100.0;
    // X is Lightroom's own sign and Y is its opposite — measured, not chosen:
    // the same +20 slides the frame one way on one axis and the other way on the
    // other, so the panel's two offsets differ in handedness.
    let (tx, ty) = ((r.perspective_x / 100.0) * OFFSET, -(r.perspective_y / 100.0) * OFFSET);
    if kv == 0.0 && kh == 0.0 && rot == 0.0 && g == 1.0 && s == 1.0 && tx == 0.0 && ty == 0.0 {
        return None;
    }
    // A keystone is a perspective divide that varies along ONE axis. `kv` is
    // NEGATIVE for a positive slider, so the divisor grows toward the top of
    // the frame (y < 0 about the centre): the top GATHERS and the bottom opens.
    // That direction is measured, not reasoned — Lightroom's `PERSP-V+50` fits
    // a keystone of −0.325, and the same export independently fills its TOP
    // corners with the void, which is what gathering the top leaves behind.
    // This engine had it the other way round until 2026-09-19.
    let keystone_v = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, kv, 1.0]);
    let keystone_h = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, kh, 0.0, 1.0]);
    let (sin, cos) = rot.sin_cos();
    // A RIGID rotation, in PIXELS — which in a box normalised per axis is not
    // the plain rotation matrix. Measured: Lightroom's slider is degrees of true
    // rotation (slider +5 reads 4.9884°, −10 reads −10.0143°), so the same
    // dropped horizon comes back level whatever shape the frame is. Written as
    // the plain matrix, as it was until 2026-09-19, the frame both under-rotates
    // by its aspect ratio and SHEARS, because a per-axis box has no notion of
    // a circle.
    let rotate =
        Homography([cos, -sin / aspect, 0.0, sin * aspect, cos, 0.0, 0.0, 0.0, 1.0]);
    // Area-preserving: one axis by `g`, the other by its reciprocal. Named
    // `aspect_map` because `aspect` is now the FRAME's shape, which the
    // keystone and the rotation above both read — two different things that
    // must not share one name in one scope.
    let aspect_map = Homography([1.0 / g, 0.0, 0.0, 0.0, g, 0.0, 0.0, 0.0, 1.0]);
    let scale = Homography([s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0]);
    let offset = Homography([1.0, 0.0, tx, 0.0, 1.0, ty, 0.0, 0.0, 1.0]);
    let m = keystone_v
        .then(keystone_h)
        .then(rotate)
        .then(aspect_map)
        .then(scale)
        .then(offset);
    Some(Homography::about_centre(m.0))
}

/// Lightroom's Upright mode, answered from ADOBE'S OWN matrix.
///
/// `None` when the mode is off, when the sidecar carried no matrix for it, or
/// when that matrix is the identity Lightroom writes for "off" and for a Guided
/// correction with no guides — in which case the caller falls back to
/// [`solve_upright`], which is what makes this app's own dropdown mean
/// something on a photograph Lightroom never solved.
pub(crate) fn upright_from_sidecar(r: &EditRecipe) -> Option<Homography> {
    let mode = r.perspective_upright.round();
    if mode <= 0.0 {
        return None;
    }
    let m = *r.upright_transform.get(mode as usize)?;
    let h = Homography(m);
    (h != Homography::IDENTITY).then_some(h)
}

/// The whole Transform stage as one map, or `None` when nothing moves.
///
/// `luma` is the frame the solver reads when the mode needs solving — the
/// caller's own working buffer, so a preview and an export answer the same
/// dropdown with the same lines.
pub(crate) fn transform(r: &EditRecipe, img: &DynamicImage) -> Option<Homography> {
    let up = upright_from_sidecar(r).or_else(|| solve_for_mode(r, img));
    // The frame this stage is about to warp, which two of the sliders are
    // defined against. Adobe's own matrix needs none of it — `upright.rs`
    // measured its normalisation to be per axis and shape-blind.
    let aspect = img.width().max(1) as f32 / img.height().max(1) as f32;
    match (up, manual(r, aspect)) {
        (None, None) => None,
        (Some(u), None) => Some(u),
        (None, Some(m)) => Some(m),
        // Upright FIRST: it is the correction, the sliders are the photographer
        // adjusting what it produced — which is the order Lightroom's own panel
        // puts them in, Upright above the manual group.
        (Some(u), Some(m)) => Some(u.then(m)),
    }
}

/// The luma plane the solver reads, built ONLY when a mode really needs solving
/// — which is this app's own dropdown on a photograph Lightroom never solved.
/// Every Lightroom photo takes [`upright_from_sidecar`] and never pays for this.
fn solve_for_mode(r: &EditRecipe, img: &DynamicImage) -> Option<Homography> {
    let mode = r.perspective_upright.round();
    if !(1.0..=4.0).contains(&mode) {
        return None;
    }
    let g = img.to_luma8();
    let (w, h) = (g.width() as usize, g.height() as usize);
    let plane: Vec<f32> = g.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    solve_upright(&plane, w, h, mode as u8)
}

/// Resample `img` through `h`, inverse-mapping every destination pixel.
///
/// The frame keeps its dimensions: a Transform is a reframing of the same
/// picture, and Lightroom's own matrices are pre-scaled to cover it (see the
/// module header). Where the map pulls from outside the source, the pixel is
/// [`VOID`] rather than a clamped edge sample — the smear that produced is the
/// defect `apply_lens_geometry`'s composite fill note describes.
///
/// One 16-bit path, exactly as `rotate_straighten` has: a frame RESAMPLE
/// promotes, because interpolating between 8-bit samples and rounding back to
/// 8 bits twice in a row (this stage, then the straighten) is where banding
/// comes from. The finishing pass's "it stays 8-bit" contract is about the
/// finishing pass, which runs after both.
pub fn apply(img: &DynamicImage, h: Homography) -> DynamicImage {
    if img.width() == 0 || img.height() == 0 || h == Homography::IDENTITY {
        return img.clone();
    }
    let Some(inv) = h.inverse() else { return img.clone() };
    let src = super::rgb16_source(img);
    let src = &*src;
    let (w, h_px) = (src.width(), src.height());
    let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(w, h_px);
    let obuf: &mut [u16] = &mut out;
    let ow = w as usize;
    obuf.par_chunks_mut(ow * 3).enumerate().for_each(|(y, orow)| {
        for x in 0..ow {
            let px = match source_of(&inv, x, y, w, h_px) {
                Some((sx, sy)) => super::sample_bilinear_rgb16(src, sx, sy),
                None => VOID,
            };
            orow[x * 3..x * 3 + 3].copy_from_slice(&px.0);
        }
    });
    DynamicImage::ImageRgb16(out)
}

/// Where destination pixel `(x, y)` reads from, in SOURCE pixels, or `None`
/// when that is outside the frame.
///
/// The normalisation divides by `w − 1`, matching every other resampler in this
/// engine (`rotate_straighten`'s `cx = (w−1)·0.5`), so the frame centre is
/// exactly 0.5 and Adobe's centre-pivoted matrices land where they mean to.
fn source_of(inv: &Homography, x: usize, y: usize, w: u32, h: u32) -> Option<(f32, f32)> {
    let (dw, dh) = ((w as f32 - 1.0).max(1.0), (h as f32 - 1.0).max(1.0));
    let (u, v) = inv.map(x as f32 / dw, y as f32 / dh)?;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    Some((u * dw, v * dh))
}

/// Lightroom's "Constrain Crop": the crop, shrunk about its own centre until it
/// lies inside the warped frame.
///
/// Shrunk about its centre and at its own ASPECT because that is what a crop
/// constraint has to preserve — a photographer who set 16:9 did not ask for
/// 1.77:1 minus a corner. `None` means the crop needs no change (or there is
/// nothing the constraint can save, in which case the crop stands and the void
/// shows, which is honest).
///
/// The factor is found by bisection rather than solved: the largest admissible
/// scale is the minimum over four corners of a projective inequality, and 40
/// halvings reach 1e-12 of it with arithmetic that cannot be got subtly wrong.
///
/// …and then ONE PIXEL is given back, which is not a fudge. The bisection
/// answers in continuous [0,1] coordinates and converges onto the boundary, so
/// the rectangle it returns TOUCHES the warped frame's edge; `render::apply_crop`
/// then quantises that rectangle with `round()`, which can move an edge outward
/// by half a pixel — and half a pixel outside a boundary-touching rectangle is
/// outside the picture. Measured: without the margin a 96 px probe came back
/// with 67 void pixels along one edge after a constrain that reported success.
/// The retreat is taken as a SCALE so the crop's aspect and centre survive it,
/// and it is the larger of the two axes' one-pixel fractions so neither edge is
/// left short.
pub(crate) fn constrain_crop(h: Homography, crop: Option<&Crop>, dims: (u32, u32)) -> Option<Crop> {
    let rect = crop.copied().unwrap_or(Crop { left: 0.0, top: 0.0, right: 1.0, bottom: 1.0 });
    let inv = h.inverse()?;
    let (cx, cy) = ((rect.left + rect.right) * 0.5, (rect.top + rect.bottom) * 0.5);
    let (hw, hh) = ((rect.right - rect.left) * 0.5, (rect.bottom - rect.top) * 0.5);
    let inside = |s: f32| {
        [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].iter().all(|&(sx, sy)| {
            inv.map(cx + sx * hw * s, cy + sy * hh * s)
                .is_some_and(|(u, v)| (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v))
        })
    };
    if inside(1.0) {
        return None; // already inside the warped frame: nothing to constrain
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if inside(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // One pixel of each axis, as a fraction of that axis's own half-extent at
    // the scale just found; the larger fraction moves both, so the shape is the
    // photographer's and the guard is the tighter axis's.
    let (px, py) = (1.0 / dims.0.max(2) as f32, 1.0 / dims.1.max(2) as f32);
    let back = (px / (hw * lo).max(f32::MIN_POSITIVE)).max(py / (hh * lo).max(f32::MIN_POSITIVE));
    let lo = lo * (1.0 - back).max(0.0);
    (lo > 0.0).then_some(Crop {
        left: cx - hw * lo,
        top: cy - hh * lo,
        right: cx + hw * lo,
        bottom: cy + hh * lo,
    })
}

/// Scale a solved map about the frame centre until it covers the frame, the way
/// Adobe's own Upright matrices already do (worst excursion +0.000000 over the
/// 13 this library selected).
///
/// Returns the map unchanged when it already covers, and `None` when no scale up
/// to [`FILL_MAX`] does. That refusal is not about strength: the scale is about
/// the CENTRE and every map here fixes the centre, so zooming pulls the
/// destination corners toward a point the map can always answer for and a
/// keystone of any strength eventually covers. What no scale recovers is a frame
/// that has SLID — what left on one side does not come back — which is why the
/// arm exists and why the solver, whose rows are bounded by
/// `upright::null_row_through`, never reaches it.
fn fill_the_frame(h: Homography) -> Option<Homography> {
    if h.covers() {
        return Some(h);
    }
    let (mut lo, mut hi) = (1.0f32, FILL_MAX);
    if !h.scaled(hi).covers() {
        return None;
    }
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if h.scaled(mid).covers() {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(h.scaled(hi))
}

/// The most a solved Upright may zoom to hide its own empty corners. Adobe's own
/// 13 needed 1.0027 to 1.0991, so four times that is a boundary rather than a
/// working range: a correction that wanted more has found a vanishing point the
/// picture does not have.
const FILL_MAX: f32 = 4.0;
