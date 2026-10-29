//! Lens geometry: the profile every consumer shares, manual distortion, fill scale, the composed resample and its normalised-point maps.

use super::*;

/// Does the composed lens geometry MOVE the shared frame? THE predicate
/// every geometry consumer must use (Codex AL-review F1): distortion-off +
/// amount-0 is NOT identity once CA overshoots, because the composite fill
/// (L04-2) zooms every channel. Dims-free and conservatively OVER-inclusive
/// (any ca knot above 1 counts, whether or not the band max survives): a
/// false positive routes through maps that are then identity anyway; a
/// false negative desyncs masks/overlays/selections from the pixels.
pub fn geometry_moves_frame(profile: &crate::recipe::LensProfile, amount: f32) -> bool {
    (profile.distortion_on && !profile.distortion.is_empty())
        || amount.abs() >= 1e-3
        || (profile.ca_on
            && !profile.ca_r.is_empty()
            && !profile.ca_b.is_empty()
            && profile.ca_r.iter().chain(&profile.ca_b).any(|k| *k > 1.0))
}

/// Radial scale one unit of `ca_r` / `ca_b` asks for (R25 B3).
///
/// ±100 therefore means ±0.2 % of the half-diagonal — about ±7 px at the
/// corner of a 6000×4000 frame, where real lateral CA is one to three. That
/// is OUR calibration and it is stated as such: Adobe never published what
/// its own ±100 means, and no sidecar in the user's library carries a
/// non-zero `crs:ChromaticAberrationR/B` to measure one from (the PV2012
/// panel replaced the pair with de-fringe + the auto switch). It is sized to
/// cover the artefact with headroom rather than to match an unknown, and it
/// stays inside the ±2 % band [`crate::recipe::LensProfile::clamp`] holds
/// profile CA knots to, so a manual value can never ask for a scale the
/// engine would refuse from a camera.
pub const MANUAL_CA_PER_UNIT: f32 = 2.0e-5;

/// THE lens profile every geometry consumer must use: the in-camera one with
/// the recipe's MANUAL CA folded into its per-channel radius knots.
///
/// Manual CA is not a second operator — a lateral CA scales a channel
/// linearly with radius, so its factor is CONSTANT in radius, which is
/// exactly what one knot means to [`profile_knot_interp`]. Folding it in here
/// rather than threading two more arguments through
/// [`apply_lens_geometry`] / [`lens_geom_norm`] / [`geometry_moves_frame`] is
/// what keeps the C2 contract intact for free: every consumer divides by the
/// SAME composite fill ([`geometry_fill_scale`]), so masks, overlays and the
/// colour dropper cannot drift against the pixels when a manual value pushes
/// a channel past 1.
///
/// BORROWED when the pair is at rest — the common case allocates nothing and
/// renders bit-identically to the pre-B3 engine.
///
/// When the profile's own CA is switched OFF, its knots do NOT participate:
/// the manual pair stands alone (a user who unticked 「Chromatic aberration」
/// asked for the camera's correction to stop, not to be scaled).
///
/// v1.5.0 composes ONE more thing here for the same reason: Lightroom's
/// profile Distortion strength (`lens_profile_distortion_scale`, 0..200 with
/// 100 = the profile's own map). A strength on a radial map means scaling its
/// DEPARTURE FROM IDENTITY, so knot `k` becomes `1 + (k − 1)·s` — which leaves
/// 100 bit-identical and makes 0 exactly "no profile distortion". At 0 the
/// knots are DROPPED rather than flattened to ones: an identity spline still
/// satisfies `geometry_active()`, and the resample it would then run is not
/// free (it promotes an 8-bit preview to 16-bit on the way through).
pub fn geometry_profile(r: &EditRecipe) -> Cow<'_, crate::recipe::LensProfile> {
    let ca_moved = r.ca_r != 0.0 || r.ca_b != 0.0;
    let dist_scale = (r.lens_profile_distortion_scale / 100.0).clamp(0.0, 2.0);
    let dist_moved = dist_scale != 1.0 && !r.lens_profile.distortion.is_empty();
    if !ca_moved && !dist_moved {
        return Cow::Borrowed(&r.lens_profile);
    }
    let p = &r.lens_profile;
    let distortion = if !dist_moved {
        p.distortion.clone()
    } else if dist_scale == 0.0 {
        Vec::new()
    } else {
        p.distortion.iter().map(|k| 1.0 + (k - 1.0) * dist_scale).collect()
    };
    if !ca_moved {
        return Cow::Owned(crate::recipe::LensProfile { distortion, ..p.clone() });
    }
    let profile_ca_on = p.ca_on && !p.ca_r.is_empty() && !p.ca_b.is_empty();
    let fold = |knots: &[f32], slider: f32| -> Vec<f32> {
        let f = 1.0 + slider * MANUAL_CA_PER_UNIT;
        if profile_ca_on { knots.iter().map(|k| k * f).collect() } else { vec![f] }
    };
    Cow::Owned(crate::recipe::LensProfile {
        distortion,
        ca_r: fold(&p.ca_r, r.ca_r),
        ca_b: fold(&p.ca_b, r.ca_b),
        ca_on: true,
        ..p.clone()
    })
}

// --- Manual lens distortion (gap batch C, 第二片) ----------------------------
//
// Coordinate-space contract (the C2 design). The geometric pipeline is
//
//   original ──apply_lens_distortion──▶ corrected ──rotate_straighten──▶ view
//
// Masks / brush strokes / droppers / clone points live in the ORIGINAL frame
// (`apply_develop` runs before this remap); `recipe.crop` lives in the VIEW
// frame. The GUI maps every interaction through
// view → (un-rotate) → corrected → [`distort_norm`] → original, and displays
// stored original-frame geometry via [`undistort_norm`] → (rotate) → view, so
// a mask painted on screen lands on the same CONTENT in the export regardless
// of the slider values.
//
// Model: a pure radial resample about the frame centre, radius normalised by
// the half-diagonal (r = 1 exactly at the corners — invariant to the EXIF
// orientation step and identical between the 1280 px preview and the 61 MP
// export). Every corrected-frame point at radius r samples the original at
//
//   r_src = s · r · (1 + k · (s·r)²),      k = −amount/100 · DISTORT_STRENGTH
//
// Sign: ACR's Distortion slider is "+ straightens barrel", which must push
// edge content OUTWARD, i.e. pull samples INWARD ⇒ k < 0 for amount > 0
// (derived twice independently: pinhole magnification recovery, and the
// bow-direction of a mapped straight line — both agree). |k| ≤ 0.25 keeps
// d(r_src)/dr = s(1 + 3k(sr)²) > 0 on the frame, so the map stays monotonic
// and invertible. `s` is a fill scale: for k > 0 (pincushion fix) the Newton
// root of k·s³ + s − 1 = 0 zooms in just enough that corner samples stay
// inside the source (no black corners — the same auto-fill policy as
// `rotate_straighten`); for k ≤ 0 the map fills the frame as-is (s = 1) and
// the outermost source corners crop away instead, like LR's constrained crop.
// The amount → k gain is our calibration, not Adobe's published one (they
// don't publish it); ±100 ⇒ up to 25 % radial remap at the corners.

/// Slider-to-curvature gain: |k| at amount = ±100. Must stay < 1/3 or the
/// radial map loses monotonicity inside the frame (see module notes above).
const DISTORT_STRENGTH: f32 = 0.25;

/// amount → (k, fill scale s). See the coordinate-space contract above.
fn distort_params(amount: f32) -> (f32, f32) {
    let k = -amount.clamp(-100.0, 100.0) / 100.0 * DISTORT_STRENGTH;
    let s = if k > 0.0 {
        // Newton on f(s) = k·s³ + s − 1: strictly increasing ⇒ unique root,
        // convex ⇒ monotone convergence from s = 1.
        let mut s = 1.0f32;
        for _ in 0..8 {
            s -= (k * s * s * s + s - 1.0) / (3.0 * k * s * s + 1.0);
        }
        s
    } else {
        1.0
    };
    (k, s)
}

/// Corrected-frame normalised point → ORIGINAL-frame normalised point: the
/// forward sampling map of the manual distortion correction. Identity when
/// the amount rounds to zero. Public — the GUI composes it into its
/// view→original interaction mapping.
pub fn distort_norm(nx: f32, ny: f32, dims: (f32, f32), amount: f32) -> (f32, f32) {
    if amount.abs() < 1e-3 {
        return (nx, ny);
    }
    let (w, h) = dims;
    let (k, s) = distort_params(amount);
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rn = (dx * dx + dy * dy).sqrt() / rr;
    let f = s * (1.0 + k * (s * rn) * (s * rn));
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// ORIGINAL-frame normalised point → corrected-frame normalised point (Newton
/// inverse of [`distort_norm`]). Original content the correction crops away
/// (a barrel fix pulls the outermost corners out of frame) has no preimage;
/// those points clamp to the map's monotonic limit and land OUTSIDE the unit
/// square, where the GUI's overlay painter clips them — honestly off-screen.
pub fn undistort_norm(nx: f32, ny: f32, dims: (f32, f32), amount: f32) -> (f32, f32) {
    if amount.abs() < 1e-3 {
        return (nx, ny);
    }
    let (w, h) = dims;
    let (k, s) = distort_params(amount);
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rho = (dx * dx + dy * dy).sqrt() / rr;
    if rho < 1e-6 {
        return (nx, ny); // centre is a fixed point
    }
    // Solve u(1 + k·u²) = ρ for u = s·r_corrected. g is concave-increasing up
    // to u_max for k < 0 (monotone Newton from the left, never overshoots) and
    // convex-increasing for k > 0 (monotone from the right); ρ beyond the k<0
    // reachable maximum clamps to u_max (the cropped-away case above).
    let u_max = if k < 0.0 { (1.0 / (3.0 * -k)).sqrt() } else { f32::INFINITY };
    let mut u = rho.min(u_max);
    for _ in 0..12 {
        let g = k * u * u * u + u - rho;
        let dg = 3.0 * k * u * u + 1.0;
        if dg.abs() < 1e-6 {
            break;
        }
        u = (u - g / dg).clamp(0.0, u_max);
    }
    let f = (u / s) / rho; // radial scale: r_corrected / r_original
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// Resample the frame through the manual distortion correction (bilinear,
/// 16-bit — the same precision policy as [`rotate_straighten`], so the export
/// path loses nothing and the 8-bit preview survives exactly). Output has the
/// SAME dimensions: the fill scale inside the map guarantees every output
/// pixel has an in-frame source sample. Identity when the amount rounds to 0.
pub fn apply_lens_distortion(img: &DynamicImage, amount: f32) -> DynamicImage {
    if amount.abs() < 1e-3 {
        return img.clone();
    }
    // Degenerate input: par_chunks_mut(0) panics on a zero-size chunk — the
    // same guard apply_lens_geometry carries; an empty frame maps to itself.
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let src = rgb16_source(img);
    let src = &*src; // the samplers take a plain &ImageBuffer
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (k, s) = distort_params(amount);
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (cx, cy) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let ow = src.width() as usize;
    let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(src.width(), src.height());
    let obuf: &mut [u16] = &mut out;
    // Output rows are independent → parallel; per-pixel math is unchanged.
    obuf.par_chunks_mut(ow * 3).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy;
        for x in 0..ow {
            let dx = x as f32 - cx;
            let rn = (dx * dx + dy * dy).sqrt() / rr;
            let f = s * (1.0 + k * (s * rn) * (s * rn));
            orow[x * 3..x * 3 + 3]
                .copy_from_slice(&sample_bilinear_rgb16(src, cx + dx * f, cy + dy * f).0);
        }
    });
    DynamicImage::ImageRgb16(out)
}

// --- In-camera lens profile geometry (lensmeta knots) -----------------------
//
// Same coordinate contract as the manual correction above: pure radial maps
// about the frame centre, radius normalised by the half-diagonal. The profile
// spline (knot i at (i+0.5)/(n−1), linear interpolation, clamped ends —
// RawTherapee's placement) runs FIRST, the manual amount composes on top, and
// CA multiplies the resulting map per channel (red/blue sample at a slightly
// different radius than green). Composition happens in ONE resample pass so
// the frame is only softened by a single bilinear step.

/// Linear interpolation over profile knots at (i+0.5)/(n−1), clamped outside.
pub(crate) fn profile_knot_interp(knots: &[f32], r: f32) -> f32 {
    let n = knots.len();
    if n == 0 {
        return 1.0;
    }
    if n == 1 {
        return knots[0];
    }
    let t = r * (n - 1) as f32 - 0.5;
    if t <= 0.0 {
        return knots[0];
    }
    if t >= (n - 1) as f32 {
        return knots[n - 1];
    }
    let i = t.floor() as usize;
    let f = t - i as f32;
    knots[i] * (1.0 - f) + knots[i + 1] * f
}

/// The profile's fill scale s_p = max g over the frame EDGE (Stannum's `s`):
/// dividing the map by it is the minimal zoom that keeps every edge source
/// sample inside the frame — no black borders, minimal crop. Edge radii sweep
/// [min(w,h)/diagonal, 1] continuously, so a max over that interval suffices.
pub(super) fn profile_fill_scale(knots: &[f32], dims: (f32, f32)) -> f32 {
    if knots.is_empty() {
        return 1.0;
    }
    let (w, h) = dims;
    let rmin = (w.min(h) / (w * w + h * h).sqrt().max(1e-6)).clamp(0.0, 1.0);
    // The spline is piecewise LINEAR (knot i at r = (i+0.5)/(n−1)), so its
    // maximum over [rmin, 1] sits at an interval endpoint or an interior
    // knot — evaluate those EXACTLY. The old 257-point uniform sweep could
    // undershoot a peak that fell between samples, and a factor above the
    // true edge maximum sends edge samples outside the source (clamped and
    // smeared by the RGB sampler).
    let mut s = profile_knot_interp(knots, rmin).max(profile_knot_interp(knots, 1.0));
    let n = knots.len();
    if n > 1 {
        let denom = (n - 1) as f32;
        for (j, k) in knots.iter().enumerate() {
            let rj = (j as f32 + 0.5) / denom;
            if rj >= rmin && rj <= 1.0 {
                s = s.max(*k);
            }
        }
    }
    s.max(1e-3)
}

/// Composed forward radial factor at normalised radius `rn` (green/base
/// channel): profile spline (over its fill scale) then the manual amount.
pub(super) fn lens_geom_factor(rn: f32, dist_knots: &[f32], s_p: f32, k: f32, s: f32) -> f32 {
    let f1 = if dist_knots.is_empty() { 1.0 } else { profile_knot_interp(dist_knots, rn) / s_p };
    let r1 = rn * f1;
    let f2 = s * (1.0 + k * (s * r1) * (s * r1));
    f1 * f2
}

/// Composite fill scale over ALL channels (L04-2): the minimal extra zoom
/// that keeps every channel's edge source sample inside the frame. The
/// GREEN map is bounded on the edge band by construction
/// ([`profile_fill_scale`] divides the spline; the manual term is exactly 1
/// at r=1), but CA MULTIPLIES red/blue past it — a ca knot above 1 sent
/// edge samples outside the source, where [`sample_bilinear_ch`] clamps and
/// smears them into a radial plateau along the border (worst in the CA-only
/// case, where s_p was hard-wired to 1 and no fill existed at all; present
/// with distortion ON too, since the fill drives green to exactly 1 at the
/// worst edge radius and CA multiplies past it).
///
/// Evaluated on the SAME [`LUT_N`] node grid the render interpolates over —
/// the rendered per-channel factor is piecewise linear with node values
/// `base[i]·ca(rn_i)`, so its band maximum sits AT a node and this bound is
/// exact for the resampler. Returns ≥ 1.0, and exactly 1.0 whenever CA is
/// off or no channel overshoots — those paths divide by 1.0 and stay
/// bit-identical. All four map consumers (RGB render, RGBA overlay,
/// forward/inverse norm) divide by the SAME value, so masks, the colour
/// dropper and clone points cannot drift against the pixels (C2).
pub(super) fn geometry_fill_scale(
    profile: &crate::recipe::LensProfile,
    amount: f32,
    dims: (f32, f32),
) -> f32 {
    let ca_on = profile.ca_on && !profile.ca_r.is_empty() && !profile.ca_b.is_empty();
    if !ca_on {
        return 1.0;
    }
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, dims) } else { 1.0 };
    let rmin = (w.min(h) / (w * w + h * h).sqrt().max(1e-6)).clamp(0.0, 1.0);
    // Start at the last node ≤ rmin: linear interpolation between nodes
    // means the band maximum is covered by the nodes bracketing it.
    let start = ((rmin * (LUT_N - 1) as f32).floor() as usize).min(LUT_N - 1);
    let mut m = 1.0f32;
    for i in start..LUT_N {
        let rn = i as f32 / (LUT_N - 1) as f32;
        let g = lens_geom_factor(rn, dist_knots, s_p, k, s);
        let ca = profile_knot_interp(&profile.ca_r, rn)
            .max(profile_knot_interp(&profile.ca_b, rn));
        m = m.max(g * ca);
    }
    m
}

/// Bilinear sample of an RGBA8 buffer; out-of-frame reads are TRANSPARENT —
/// an overlay raster must vanish where the remap leaves the source, not
/// smear its edge pixels (the RGB16 paths clamp instead, correct for photos).
fn sample_bilinear_rgba8(src: &image::RgbaImage, x: f32, y: f32) -> image::Rgba<u8> {
    let (w, h) = (src.width() as i32, src.height() as i32);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let px = |xi: i32, yi: i32| -> [f32; 4] {
        if xi < 0 || yi < 0 || xi >= w || yi >= h {
            [0.0; 4]
        } else {
            let p = src.get_pixel(xi as u32, yi as u32).0;
            // PREMULTIPLIED components: interpolating straight RGBA drags the
            // colour toward transparent neighbours' arbitrary (zero) RGB and
            // then attenuates AGAIN at composite time — dark fringes on every
            // overlay edge under geometry.
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
        }
    };
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    let mut acc = [0f32; 4];
    for i in 0..4 {
        let top = a[i] * (1.0 - fx) + b[i] * fx;
        let bot = c[i] * (1.0 - fx) + d[i] * fx;
        acc[i] = top * (1.0 - fy) + bot * fy;
    }
    let alpha = acc[3];
    let mut o = [0u8; 4];
    if alpha > 0.0 {
        for i in 0..3 {
            // Un-premultiply by the interpolated alpha (255·a normalisation
            // cancels) so straight-alpha consumers see the true colour.
            o[i] = (acc[i] * 255.0 / alpha).round().clamp(0.0, 255.0) as u8;
        }
        o[3] = alpha.round().clamp(0.0, 255.0) as u8;
    }
    image::Rgba(o)
}

/// Alpha-preserving twin of [`apply_lens_geometry`] for UI overlay rasters
/// (the paint canvas): the RGB16 photo path flattens transparency to opaque,
/// which turned the whole canvas into a red wash the moment any geometry was
/// active. Green map only — an overlay needs no chromatic refinement, but it
/// MUST carry the composite CA fill scale (L04-2): the render's green map is
/// zoomed by 1/fill, and an overlay skipping that drifts off the pixels.
pub fn apply_lens_geometry_rgba(
    src: &image::RgbaImage,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> image::RgbaImage {
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    // Degenerate frame: par_chunks_mut(0) below would panic — same guard the
    // RGB16 twin (apply_lens_geometry) carries.
    if src.width() == 0 || src.height() == 0 {
        return src.clone();
    }
    let (w, h) = (src.width() as f32, src.height() as f32);
    let fill = geometry_fill_scale(profile, amount, (w, h));
    // fill > 1 means the RGB render moved every pixel even with distortion
    // off — the overlay must move with it, so the early-out gains the
    // fill==1 condition.
    if !dist_on && amount.abs() < 1e-3 && fill == 1.0 {
        return src.clone();
    }
    let inv_fill = 1.0 / fill;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, (w, h)) } else { 1.0 };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (cx, cy) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let ow = src.width() as usize;
    let mut out = image::RgbaImage::new(src.width(), src.height());
    let obuf: &mut [u8] = &mut out;
    obuf.par_chunks_mut(ow * 4).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy;
        for x in 0..ow {
            let dx = x as f32 - cx;
            let rn = ((dx * dx + dy * dy).sqrt() / rr).clamp(0.0, 1.0);
            let f = lens_geom_factor(rn, dist_knots, s_p, k, s) * inv_fill;
            orow[x * 4..x * 4 + 4]
                .copy_from_slice(&sample_bilinear_rgba8(src, cx + dx * f, cy + dy * f).0);
        }
    });
    out
}

/// Alpha-preserving twin of [`rotate_straighten`] for UI overlay rasters —
/// same inverse rotation matrix and inscribed-crop output size.
pub fn rotate_straighten_rgba(src: &image::RgbaImage, deg: f32) -> image::RgbaImage {
    if deg.abs() < 1e-3 {
        return src.clone();
    }
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (cw, ch) = inscribed_dims(w, h, deg);
    let (ow, oh) = ((cw.floor() as u32).max(1), (ch.floor() as u32).max(1));
    let rad = deg.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let (cx_src, cy_src) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let (cx_dst, cy_dst) = ((ow as f32 - 1.0) * 0.5, (oh as f32 - 1.0) * 0.5);
    let ow_px = ow as usize;
    let mut out = image::RgbaImage::new(ow, oh);
    let obuf: &mut [u8] = &mut out;
    obuf.par_chunks_mut(ow_px * 4).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy_dst;
        for x in 0..ow_px {
            let dx = x as f32 - cx_dst;
            let sx = cx_src + c * dx + s * dy;
            let sy = cy_src - s * dx + c * dy;
            orow[x * 4..x * 4 + 4].copy_from_slice(&sample_bilinear_rgba8(src, sx, sy).0);
        }
    });
    out
}

/// Corrected-frame normalised point → ORIGINAL-frame normalised point through
/// the COMPOSED geometry (profile distortion + manual amount — the green map).
/// CA's chromatic split stays render-only, but its composite FILL SCALE moves
/// the shared map by a scalar (L04-2) — skipping it here drifted masks, the
/// dropper and clone points off the rendered pixels. Falls back to
/// [`distort_norm`]'s exact math when the whole map is the manual one.
pub fn lens_geom_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    let fill = geometry_fill_scale(profile, amount, dims);
    if !dist_on && fill == 1.0 {
        return distort_norm(nx, ny, dims, amount);
    }
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, dims) } else { 1.0 };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rn = (dx * dx + dy * dy).sqrt() / rr;
    let f = lens_geom_factor(rn, dist_knots, s_p, k, s) / fill;
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// ORIGINAL-frame normalised point → corrected-frame point: numeric inverse of
/// [`lens_geom_norm`] by bisection on the forward radial map (monotone for
/// every real profile — factors live in `clamp()`'s 0.7..1.3 band and the
/// spline slopes are gentle; a crafted zigzag would merely land on ONE valid
/// preimage). Falls back to [`undistort_norm`] when the profile is inactive.
pub fn lens_ungeom_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    // Same composite fill as the forward map (L04-2) — inverting a map the
    // render did not draw would un-roundtrip every C2 consumer.
    let fill = geometry_fill_scale(profile, amount, dims);
    if !dist_on && fill == 1.0 {
        return undistort_norm(nx, ny, dims, amount);
    }
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, dims) } else { 1.0 };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rho = (dx * dx + dy * dy).sqrt() / rr;
    if rho < 1e-6 {
        return (nx, ny);
    }
    // Bisection over output radius on the RISING PREFIX of the forward map:
    // fwd(rn) = rn · factor(rn) increases and then — under a strong manual
    // barrel fix — folds back (the same shape undistort_norm's u_max clamp
    // handles). Scan for the peak first; originals beyond the reachable
    // maximum clamp there and land honestly off-screen, like undistort_norm.
    let fwd = |rn: f32| rn * lens_geom_factor(rn, dist_knots, s_p, k, s) / fill;
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
        return ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5);
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
    let rn = 0.5 * (lo + hi);
    let f = rn / rho; // r_corrected / r_original
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// Resample the frame through the COMPOSED lens geometry: profile distortion
/// (+ per-channel CA) and the manual amount in one bilinear pass. Identity →
/// clone. Same 16-bit precision policy as [`apply_lens_distortion`], which
/// remains as the manual-only special case this generalises.
pub fn apply_lens_geometry(
    img: &DynamicImage,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> DynamicImage {
    // Degenerate input: rayon's par_chunks_mut(0) panics on a zero-size
    // chunk — an empty frame maps to itself.
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    let ca_on = profile.ca_on && !profile.ca_r.is_empty() && !profile.ca_b.is_empty();
    if !dist_on && !ca_on {
        return apply_lens_distortion(img, amount);
    }
    let src = rgb16_source(img);
    let src = &*src; // the samplers take a plain &ImageBuffer
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, (w, h)) } else { 1.0 };
    // Composite CA fill (L04-2): every channel's LUT divides by ONE scalar
    // (≥ 1; exactly 1 when no channel overshoots, keeping those paths
    // bit-identical), so a ca knot above 1 zooms the whole frame in by up
    // to that knot instead of sending red/blue edge samples outside the
    // source, where the clamping sampler smeared them into a radial band.
    // Per-channel renormalisation is NOT an option — dividing ca_r by its
    // own max would cancel the near-constant correction it encodes.
    let fill = geometry_fill_scale(profile, amount, (w, h));
    // Per-channel radial factor LUTs over rn ∈ [0,1]: one lookup per channel
    // per pixel instead of spline walks. CA multiplies the green map.
    let luts: [Vec<f32>; 3] = {
        let base: Vec<f32> = (0..LUT_N)
            .map(|i| lens_geom_factor(i as f32 / (LUT_N - 1) as f32, dist_knots, s_p, k, s))
            .collect();
        let chan = |knots: &[f32]| -> Vec<f32> {
            if !ca_on || knots.is_empty() {
                return base.iter().map(|f| f / fill).collect();
            }
            (0..LUT_N)
                .map(|i| {
                    let rn = i as f32 / (LUT_N - 1) as f32;
                    base[i] * profile_knot_interp(knots, rn) / fill
                })
                .collect()
        };
        [chan(&profile.ca_r), chan(&[]), chan(&profile.ca_b)]
    };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (cx, cy) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let ow = src.width() as usize;
    let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(src.width(), src.height());
    let obuf: &mut [u16] = &mut out;
    obuf.par_chunks_mut(ow * 3).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy;
        for x in 0..ow {
            let dx = x as f32 - cx;
            let rn = ((dx * dx + dy * dy).sqrt() / rr).clamp(0.0, 1.0);
            if ca_on {
                // Red and blue sample at their own CA-refined radius — one
                // channel per fetch (the full-RGB sampler computed all three
                // channels only to keep one, tripling the interpolation work
                // across a 61 MP frame).
                for (c, lut) in luts.iter().enumerate() {
                    let f = sample_lut(lut, rn);
                    orow[x * 3 + c] = sample_bilinear_ch(src, cx + dx * f, cy + dy * f, c);
                }
            } else {
                let f = sample_lut(&luts[1], rn);
                orow[x * 3..x * 3 + 3]
                    .copy_from_slice(&sample_bilinear_rgb16(src, cx + dx * f, cy + dy * f).0);
            }
        }
    });
    DynamicImage::ImageRgb16(out)
}

/// A lens-CORRECTED raster in the SENSOR frame: every sensor pixel takes the
/// corrected pixel the geometry stage would put it at. The map is the engine's
/// own inverse ([`lens_ungeom_norm`], through the [`MaskUnwarp`] table the mask
/// chain samples it with) — the same map, run the other way, never a second
/// model of the lens.
///
/// The alpha is the answer to "is there a corrected pixel here at all": opaque
/// inside the corrected field, clear for the sensor pixels the correction
/// crops away, whose colour is the field's clamped edge so a reader that
/// ignores alpha sees a plausible picture. `None` when the map moves nothing.
/// The second value is the clear share of the frame.
pub fn corrected_to_sensor_frame(
    img: &DynamicImage,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> Option<(DynamicImage, f32)> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let (wf, hf) = (w as f32, h as f32);
    let unwarp = MaskUnwarp::new(profile, amount, (wf, hf))?;
    let src = rgb16_source(img);
    let src = &*src;
    let mut out = image::RgbaImage::new(w, h);
    let obuf: &mut [u8] = &mut out;
    let absent: usize = obuf
        .par_chunks_mut(w as usize * 4)
        .enumerate()
        .map(|(y, orow)| {
            let mut clear = 0usize;
            for x in 0..w as usize {
                let (qx, qy) = unwarp.engine_at((x as f32 + 0.5) / wf, (y as f32 + 0.5) / hf);
                let (sx, sy) = (qx * wf - 0.5, qy * hf - 0.5);
                let inside = (0.0..=wf - 1.0).contains(&sx) && (0.0..=hf - 1.0).contains(&sy);
                let p = sample_bilinear_rgb16(src, sx, sy);
                let to8 = |v: u16| (v as f32 / 257.0).round().clamp(0.0, 255.0) as u8;
                orow[x * 4..x * 4 + 4].copy_from_slice(&[to8(p[0]), to8(p[1]), to8(p[2]), if inside { 255 } else { 0 }]);
                clear += usize::from(!inside);
            }
            clear
        })
        .sum();
    Some((DynamicImage::ImageRgba8(out), absent as f32 / (w as f32 * h as f32)))
}
