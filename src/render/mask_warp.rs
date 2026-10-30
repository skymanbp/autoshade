//! The mask warp: Lightroom's stored mask frame and its export frame, solved from the in-camera knots, and the view / original maps.

use super::*;

// --- the MASK WARP: Lightroom's stored mask frame → Lightroom's export frame -
//
// Everything in this block is about LIGHTROOM's frames, not this engine's. The
// two are not the same and the difference is the whole reason the block needs a
// header rather than a one-line doc.
//
// WHAT WAS MEASURED (R27 Batches 8-10, R29 `D`, and D2 LINEAR through
// 2026-08-24). Lightroom stores geometry by mask type and LINEAR adds a
// topology distinction:
//
//   * BRUSH dabs are stored PRE-lens-correction. Eleven disjoint dabs on the
//     24 mm frame are displaced from their stored coordinates by exactly the
//     lens-profile distortion field: the `.lcp` model with ZERO free parameters
//     scores 4.63 px rms against a 4.19 px tangential noise floor, and the same
//     field read off the camera's own knots scores 5.13 px. On 138 pixel
//     patches of a `LensProfileEnable` 0→1 pair the two score 2.11 / 2.30 px.
//   * RADIAL points are stored POST-correction. On the 105 mm `D` pair the
//     PIXELS move +87.5 px at r ≈ 3250 (the `.lcp` model at 2.69 px rms,
//     30 NCC points, tangential rms 1.22 px) while the radial mask itself
//     measures a similarity of 0.99956 — identity to within 0.05 %, and
//     88.7 px away from the pixel field.
//   * LINEAR stores corrected-frame Zero/Full handles. With correction ON it
//     reconstructs the straight gradient in that corrected frame. With
//     correction OFF it maps only those two handles through D_fwd and rebuilds
//     one straight gradient in the raw pixel metric. Pointwise H1 is rejected:
//     its predicted full-contour sag is 10.8–24.4 px with the wrong sign.
//
// So `m(r)` below is one number per radius: where a stored radius LANDS in
// Lightroom's export. `LensProfile::mask_warp` holds it as knots.
//
// WHAT THIS ENGINE DOES WITH IT — and the part that is NOT the same question.
// This pipeline evaluates every mask in the PRE-lens-correction frame and then
// resamples the whole frame through the geometry stage (`apply_develop` runs
// `apply_masks`; `finish::frame_and_finish` then runs `apply_lens_geometry` on
// the developed buffer, and its doc states the order outright). A mask this
// engine draws is therefore carried by the
// distortion field exactly as the pixels are, without anyone applying `m`:
//
//   | mask kind   | downstream geometry | engine frame operation              |
//   |-------------|---------------------|-------------------------------------|
//   | brush       | either              | IDENTITY                            |
//   | radial      | active              | T_engine less T_engine(centre)−c   |
//   | radial      | inactive            | IDENTITY (stored coordinates)       |
//   | linear      | active              | sample at T_engine(p), no m_lr      |
//   | linear      | inactive            | D_fwd(z), D_fwd(f), rebuild straight|
//   | bitmap / AI | either              | IDENTITY                            |
//
// The brush row is why nothing here is wired into `mask_weight` for a dab:
// applying `m` to a dab centre AND letting the geometry stage move it would
// apply the field twice, putting a 24 mm corner dab ~186 px past where
// Lightroom puts it.
//
// The radial row WAS a live mismatch — every imported radial on a
// profile-corrected photo rendered up to 186 px (24 mm) / 88 px (105 mm) away
// from where Lightroom puts it. `MaskFrame` + `MaskUnwarp` now compose the two
// independent maps exactly once: `T_engine` cancels the downstream engine
// resample at rasterisation time, and the ellipse's CENTRE travels with the
// photograph. Since v1.17.0 only the centre, and by the engine's own map:
// Camera Raw keeps a radial's radii in the corrected frame (a pointwise
// `m_lr⁻¹` dilated a centred 105 mm ellipse to 1.045 of its radius, Camera
// Raw draws it at 1.005) and moves its centre as the pixels move, which the
// `m_lr` spline overshoots by 8–22 % on the four frames measured. So the
// sample is translated, not warped (`MaskUnwarp::radial_at`).
// The LINEAR rows are H2. Their zero-parameter absolute residual remains
// 9.748/7.025/6.336 px RMS for the active/stored line and
// 12.449/9.943/4.979 px for the inactive/transported line. They are topology
// evidence, not 1 px closure; the fitted anisotropic aspect candidate is not
// implemented. The named LINEAR tests pin active placement, all three wall
// handle pairs, and straightness independently of the RADIAL tests below.
//
// THE RADIUS, decided here because there is nowhere better. A brush dab is a
// circle of radius `r` and the map is not conformal, so a warped dab is an
// ELLIPSE: the local Jacobian's tangential eigenvalue is `m(r)` and its radial
// one is `d(r·m)/dr`, and they differ by up to 6.15 % at 24 mm and 7.66 % at
// 105 mm — MORE than the isotropic part a radius scale would fix (3.25 % and
// 4.25 %). No scalar radius can represent that, so no scalar is the right
// answer, and nothing in the measurement set observes a dab RADIUS at all (the
// eleven-dab ladder measures centres). The map below therefore takes POINTS and
// has no radius argument: the exact way to warp a brush mask is to rasterise
// the stroke in its stored frame and resample the resulting PLANE through
// `m` — which is precisely what this engine's geometry stage already does to
// every mask it draws.

/// [`LensProfile::mask_warp`]'s interpolator: the radial magnification at
/// normalised radius `rn` (1 = the corner half-diagonal).
///
/// The SAME spline the in-camera `distortion` knots are read with, deliberately
/// — the two sources write one field and a second interpolator is how two
/// conventions get in.
///
/// [`LensProfile::mask_warp`]: crate::recipe::LensProfile::mask_warp
pub fn mask_warp_factor(knots: &[f32], rn: f32) -> f32 {
    if knots.is_empty() {
        return 1.0;
    }
    profile_knot_interp(knots, rn)
}

/// Solve the mask warp from the IN-CAMERA knots — source A.
///
/// The camera's `distortion` spline is a BACKWARD map like Adobe's: at
/// corrected radius `rn` the source sample sits at `rn · g(rn)/s_p`. A mask
/// stored in the source frame needs the other direction, so this inverts that
/// map at each knot radius by bisection on its rising prefix — the same
/// construction (and the same fold guard) [`lens_ungeom_norm`] uses, because it
/// is the same inversion.
///
/// Deliberately NOT composed with the manual `lens_distortion` slider or the
/// CA fill scale. Those are this engine's own edit and this engine's own
/// resampling artefact; `mask_warp` models what LIGHTROOM's correction did, and
/// folding our slider into it would make the answer depend on the user's later
/// choices.
///
/// Empty in ⇒ empty out: no knots is not a warp of 1.0, it is no answer, and
/// `MaskWarpSource` is where that difference is stated.
pub fn mask_warp_from_camera_knots(distortion: &[f32], dims: (f32, f32), n: usize) -> Vec<f32> {
    if distortion.is_empty() || n < 2 {
        return Vec::new();
    }
    let s_p = profile_fill_scale(distortion, dims);
    let fwd = |rn: f32| rn * profile_knot_interp(distortion, rn) / s_p;
    // Peak scan first: past the fold the map is no longer injective and a
    // bisection would land on an arbitrary preimage.
    let mut hi_max = 2.0f32;
    let mut peak = 0.0f32;
    for i in 1..=256 {
        let rn = 2.0 * i as f32 / 256.0;
        let v = fwd(rn);
        if v < peak {
            hi_max = 2.0 * (i - 1) as f32 / 256.0;
            break;
        }
        peak = v;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let rho = (i as f32 + 0.5) / (n - 1) as f32;
        if fwd(hi_max) <= rho {
            // Beyond what the map reaches: clamp at the peak, exactly as
            // `lens_ungeom_norm` does, rather than extrapolate a factor.
            out.push(hi_max / rho);
            continue;
        }
        let (mut lo, mut hi) = (0.0f32, hi_max);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if fwd(mid) < rho {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        out.push(0.5 * (lo + hi) / rho);
    }
    out
}

/// STORED mask point → the point Lightroom EXPORTED it at, both normalised
/// 0..1 on the frame corner origin.
///
/// Identity when the profile carries no solved warp, which is the honest answer
/// for a photo whose frame nobody could model — see
/// [`crate::recipe::MaskWarpSource`] for which of the five "no warp" states
/// applies and why they are not one state.
///
/// Read the block header above before wiring this into a render path: which
/// mask kinds need it, and in which direction, depends on where that path
/// evaluates masks, and for THIS engine's own `mask_weight` the answer for a
/// brush is identity.
pub fn lr_mask_warp_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
) -> (f32, f32) {
    if profile.mask_warp.is_empty() {
        return (nx, ny);
    }
    let (w, h) = dims;
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let [cx, cy] = lr_mask_center_px(dims, profile);
    let (dx, dy) = (nx * w - cx, ny * h - cy);
    let f = mask_warp_factor(&profile.mask_warp, (dx * dx + dy * dy).sqrt() / rr);
    ((dx * f + cx) / w.max(1e-6), (dy * f + cy) / h.max(1e-6))
}

/// EXPORTED point → the point Lightroom STORED it as: the numeric inverse of
/// [`lr_mask_warp_norm`], by the same rising-prefix bisection
/// [`lens_ungeom_norm`] uses.
///
/// This is the Lightroom half of the sample composition for a radial or
/// linear geometry. [`MaskUnwarp`] calls it after the engine-map half; brush,
/// bitmap and AI geometry never take that arm.
pub fn lr_mask_unwarp_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
) -> (f32, f32) {
    unwarp_norm_over(nx, ny, dims, profile, &profile.mask_warp)
}

/// The numeric inverse itself, over WHICHEVER spline the caller names.
///
/// The radial and LINEAR arms are the same 45 lines — same centre, same fold
/// guard, same 40-step bisection law — differing only in where the knots come
/// from, so the knots are the parameter. They were a copy while the linear arm
/// was being settled (D2's handle-transport rule could not take a second knot
/// source without touching the byte-for-byte settled radial path); the copy
/// outlived that reason, and a fold guard that exists twice is a fold guard
/// that can be fixed once.
fn unwarp_norm_over(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    knots: &[f32],
) -> (f32, f32) {
    if knots.is_empty() {
        return (nx, ny);
    }
    let (w, h) = dims;
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let [cx, cy] = lr_mask_center_px(dims, profile);
    let (dx, dy) = (nx * w - cx, ny * h - cy);
    let rho = (dx * dx + dy * dy).sqrt() / rr;
    if rho < 1e-6 {
        return (nx, ny);
    }
    let fwd = |rn: f32| rn * mask_warp_factor(knots, rn);
    let mut hi = 2.0f32;
    let mut peak = 0.0f32;
    for i in 1..=256 {
        let rn = 2.0 * i as f32 / 256.0;
        let v = fwd(rn);
        if v < peak {
            hi = 2.0 * (i - 1) as f32 / 256.0;
            break;
        }
        peak = v;
    }
    if fwd(hi) <= rho {
        let f = hi / rho;
        return ((dx * f + cx) / w.max(1e-6), (dy * f + cy) / h.max(1e-6));
    }
    let mut lo = 0.0f32;
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if fwd(mid) < rho {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let f = 0.5 * (lo + hi) / rho;
    ((dx * f + cx) / w.max(1e-6), (dy * f + cy) / h.max(1e-6))
}

/// Lightroom's full-raw centre in the dimensions currently being rendered.
/// Legacy recipes carry no frame fact and retain stored-frame-centre behaviour.
pub(super) fn lr_mask_center_px(
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
) -> [f32; 2] {
    let (w, h) = dims;
    profile.mask_warp_center.map_or([w * 0.5, h * 0.5], |c| {
        [c.stored_px[0] * w / c.stored_dims[0], c.stored_px[1] * h / c.stored_dims[1]]
    })
}

/// View-frame normalised point (the straightened, auto-cropped frame the user
/// SEES, before the user crop) → ORIGINAL-frame normalised point: un-rotate
/// (view → corrected, the counter-clockwise matrix), then the forward sampling
/// map (corrected → original). Clamped once, at the end. This is the ONE
/// shared implementation of the C2 interaction mapping — the GUI wraps it and
/// the web server maps analyze region boxes through it, so they cannot drift.
pub fn view_to_original_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    deg: f32,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let (w, h) = dims;
    // Same identity threshold as BOTH raster rotators (rotate_straighten /
    // rotate_straighten_rgba use |deg| < 1e-3): an exact-zero test here made
    // the maps rotate for a sub-threshold angle the pixels never got.
    let (cx, cy) = if deg.abs() < 1e-3 {
        (nx, ny)
    } else {
        let (cw, ch) = inscribed_dims(w, h, deg);
        let rad = deg.to_radians();
        let (s, c) = (rad.sin(), rad.cos());
        let (dx, dy) = ((nx - 0.5) * cw, (ny - 0.5) * ch);
        // Content was rotated clockwise; undo with the counter-clockwise matrix.
        (((c * dx + s * dy) / w) + 0.5, ((-s * dx + c * dy) / h) + 0.5)
    };
    let (ox, oy) = lens_geom_norm(cx, cy, dims, profile, amount);
    (ox.clamp(0.0, 1.0), oy.clamp(0.0, 1.0))
}

/// ORIGINAL-frame normalised point → view normalised point: the inverse
/// geometry map (original → corrected), then the forward rotation. NOT
/// clamped: an original point can legitimately fall outside the view window
/// (content a barrel fix crops away lands just outside the unit square);
/// callers clip.
pub fn original_to_view_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    deg: f32,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let (nx, ny) = lens_ungeom_norm(nx, ny, dims, profile, amount);
    // Same 1e-3 identity threshold as the raster rotators — see
    // view_to_original_norm.
    if deg.abs() < 1e-3 {
        return (nx, ny);
    }
    let (w, h) = dims;
    let (cw, ch) = inscribed_dims(w, h, deg);
    let rad = deg.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rx = c * dx - s * dy; // clockwise forward
    let ry = s * dx + c * dy;
    (rx / cw.max(1e-3) + 0.5, ry / ch.max(1e-3) + 0.5)
}
