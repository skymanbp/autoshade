// One part of the engine's tests (src/render/tests.rs includes it): straighten, distortion, lens profiles, the mask warp, camera models and where masks land under the geometry stage.

#[test]
fn export_refuses_a_recipe_whose_mask_raster_is_unreadable() {
    use crate::recipe::LocalAdjustment;
    let dir = std::env::temp_dir().join(format!("autoshade_maskgate_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("base.png");
    image::DynamicImage::new_rgb8(8, 8).save(&src).unwrap();
    let broken = LocalAdjustment {
        name: "sky".into(),
        amount: 1.0,
        exposure_ev: -0.5,
        mask: MaskGeometry::Bitmap { path: dir.join("gone.png").display().to_string() },
        ..Default::default()
    };
    let r = EditRecipe { masks: vec![broken.clone()], ..Default::default() };
    let out = dir.join("out.png");
    let err = render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).unwrap_err().to_string();
    assert!(err.contains("sky"), "the refusal names the mask: {err}");
    assert!(!out.exists(), "a refused export writes nothing");
    // amount = 0 is inert BY the recipe — nothing is being dropped.
    let mut disabled = broken;
    disabled.amount = 0.0;
    let r = EditRecipe { masks: vec![disabled], ..Default::default() };
    render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).expect("disabled mask exports fine");
    // A PARKED mask (default amount 1, every adjustment neutral) renders
    // nothing even with a healthy raster — its lost raster must not
    // block the export either.
    let parked = LocalAdjustment {
        name: "parked".into(),
        mask: MaskGeometry::Bitmap { path: dir.join("gone.png").display().to_string() },
        ..Default::default()
    };
    let r = EditRecipe { masks: vec![parked], ..Default::default() };
    render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).expect("engine-inert mask exports fine");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn straighten_rotation_geometry_and_direction() {
    // (a) 0° is the identity (dims + pixels untouched). A non-uniform
    // frame and a byte comparison: the old uniform frame + dims-only
    // check passed even if the zero-angle branch returned cleared or
    // arbitrary pixels (R12).
    let img = DynamicImage::ImageRgb8(RgbImage::from_fn(40, 30, |x, y| {
        image::Rgb([(x * 6) as u8, (y * 8) as u8, ((x + y) % 256) as u8])
    }));
    let same = rotate_straighten(&img, 0.0);
    assert_eq!((same.width(), same.height()), (40, 30));
    assert_eq!(
        same.to_rgb8().as_raw(),
        img.to_rgb8().as_raw(),
        "0° must return the pixels untouched"
    );

    // (b) inscribed_dims: identity at 0°, symmetric in ±θ, strictly smaller
    // than the frame for any real tilt.
    assert_eq!(inscribed_dims(120.0, 80.0, 0.0), (120.0, 80.0));
    let (w1, h1) = inscribed_dims(120.0, 80.0, 7.0);
    let (w2, h2) = inscribed_dims(120.0, 80.0, -7.0);
    assert!((w1 - w2).abs() < 1e-4 && (h1 - h2).abs() < 1e-4);
    assert!(w1 < 120.0 && h1 < 80.0 && w1 > 90.0 && h1 > 60.0, "({w1},{h1})");

    // (c) No black corners: an all-white frame stays all-white after any
    // tilt — the auto-crop must keep every sample inside the source.
    let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(120, 80, image::Rgb([255, 255, 255])));
    for deg in [3.0, 7.0, -12.0, 30.0] {
        let r = rotate_straighten(&white, deg).to_rgb8();
        let min = r.pixels().flat_map(|p| p.0).min().unwrap();
        assert!(min >= 250, "black bleed at {deg}°: min channel {min}");
    }

    // (d) Direction: positive = CLOCKWISE (the recipe contract). A vertical
    // red|blue split rotated clockwise tilts its divider top-to-the-right,
    // so just right of centre at the TOP row the red half now covers it.
    let mut split = RgbImage::new(100, 100);
    for (x, _y, p) in split.enumerate_pixels_mut() {
        *p = if x < 50 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) };
    }
    let rot = rotate_straighten(&DynamicImage::ImageRgb8(split), 10.0).to_rgb8();
    let (rw, _rh) = rot.dimensions();
    let probe = rot.get_pixel(rw / 2 + 3, 1);
    assert!(probe[0] > probe[2], "clockwise tilt must move red over top-centre-right: {probe:?}");
}

#[test]
fn distortion_maps_are_inverse_and_directionally_correct() {
    let dims = (120.0, 80.0);
    // (a) amount = 0 is the exact identity, both directions.
    assert_eq!(distort_norm(0.31, 0.77, dims, 0.0), (0.31, 0.77));
    assert_eq!(undistort_norm(0.31, 0.77, dims, 0.0), (0.31, 0.77));
    // (b) the centre is a fixed point at any amount.
    for amt in [-100.0f32, -45.0, 60.0, 100.0] {
        let (cx, cy) = distort_norm(0.5, 0.5, dims, amt);
        assert!((cx - 0.5).abs() < 1e-5 && (cy - 0.5).abs() < 1e-5, "centre moved at {amt}");
    }
    // (c) Round-trips. view→orig→view must hold everywhere in the frame;
    // orig→view→orig only for content the correction keeps (interior
    // points — a +100 barrel fix legitimately crops the outermost corners,
    // and those originals have no preimage by design).
    for amt in [-100.0f32, -45.0, 60.0, 100.0] {
        for (nx, ny) in [(0.0, 0.0), (1.0, 0.0), (0.1, 0.9), (0.3, 0.4), (0.62, 0.85), (0.5, 0.5)] {
            let (ox, oy) = distort_norm(nx, ny, dims, amt);
            let (bx, by) = undistort_norm(ox, oy, dims, amt);
            assert!(
                (bx - nx).abs() < 2e-3 && (by - ny).abs() < 2e-3,
                "view roundtrip @{amt}: ({nx},{ny}) → ({ox},{oy}) → ({bx},{by})"
            );
        }
        for (nx, ny) in [(0.3, 0.4), (0.6, 0.35), (0.25, 0.7), (0.45, 0.52)] {
            let (vx, vy) = undistort_norm(nx, ny, dims, amt);
            let (bx, by) = distort_norm(vx, vy, dims, amt);
            assert!(
                (bx - nx).abs() < 2e-3 && (by - ny).abs() < 2e-3,
                "orig roundtrip @{amt}: ({nx},{ny}) → ({vx},{vy}) → ({bx},{by})"
            );
        }
    }
    // (d) Direction, via the radial sampling ratio f = r_src/r_dst probed
    // along the x-axis: a barrel fix (+) pulls samples INWARD, harder at
    // the edge (f < 1, decreasing); a pincushion fix (−) samples RELATIVELY
    // further out at the edge than at the centre (f increasing).
    let ratio = |nx: f32, amt: f32| {
        let (ox, _) = distort_norm(nx, 0.5, dims, amt);
        (ox - 0.5) / (nx - 0.5)
    };
    assert!(
        ratio(0.95, 100.0) < ratio(0.6, 100.0) && ratio(0.6, 100.0) < 1.0,
        "barrel fix direction: f(edge)={} f(mid)={}",
        ratio(0.95, 100.0),
        ratio(0.6, 100.0)
    );
    assert!(
        ratio(0.95, -100.0) > ratio(0.6, -100.0),
        "pincushion fix direction: f(edge)={} f(mid)={}",
        ratio(0.95, -100.0),
        ratio(0.6, -100.0)
    );
}

#[test]
fn view_original_norm_maps_roundtrip_and_identity() {
    let dims = (1200.0, 800.0);
    let off = crate::recipe::LensProfile::default();
    // Identity when every control is zero.
    assert_eq!(view_to_original_norm(0.31, 0.77, dims, 0.0, &off, 0.0), (0.31, 0.77));
    assert_eq!(original_to_view_norm(0.31, 0.77, dims, 0.0, &off, 0.0), (0.31, 0.77));
    // Round-trip through straighten + manual distortion for interior
    // points (the composed map the web region box and every GUI mask
    // gesture ride on).
    for (deg, amt) in [(4.5f32, 0.0f32), (0.0, 35.0), (-3.0, -60.0), (7.0, 80.0)] {
        for (nx, ny) in [(0.3, 0.4), (0.55, 0.6), (0.42, 0.35), (0.5, 0.5)] {
            let (ox, oy) = view_to_original_norm(nx, ny, dims, deg, &off, amt);
            let (bx, by) = original_to_view_norm(ox, oy, dims, deg, &off, amt);
            assert!(
                (bx - nx).abs() < 3e-3 && (by - ny).abs() < 3e-3,
                "roundtrip deg={deg} amt={amt}: ({nx},{ny}) → ({ox},{oy}) → ({bx},{by})"
            );
            // A round trip alone proves only that the two maps invert
            // EACH OTHER: replace both with the identity and every
            // assertion above still passes while masks, brush strokes and
            // region boxes quietly stop tracking the displayed geometry.
            // So also require that active geometry actually MOVES an
            // off-centre point. (The frame centre is a fixed point of
            // both rotation and radial distortion — it is excluded.)
            if (deg != 0.0 || amt != 0.0) && (nx, ny) != (0.5, 0.5) {
                assert!(
                    (ox - nx).abs() > 1e-4 || (oy - ny).abs() > 1e-4,
                    "deg={deg} amt={amt} left ({nx},{ny}) unmoved — the map is inert"
                );
            }
        }
    }
}

/// **v1.5.0 F5**: Lightroom's two PROFILE strength sliders — and the
/// property that matters most about them, which is that 100 renders the
/// pre-v1.5.0 bytes EXACTLY. A strength that only nearly vanished at its
/// own neutral would rewrite every photo in the library the day it shipped.
#[test]
fn the_profile_strengths_scale_the_correction_and_100_changes_nothing() {
    use crate::recipe::LensProfile;
    let profile = LensProfile {
        vignette: (0..16).map(|i| 1.0 + 0.42 * (i as f32 / 15.0).powi(2)).collect(),
        distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
        vignette_on: true,
        distortion_on: true,
        ..Default::default()
    };
    let base =
        DynamicImage::ImageRgb8(RgbImage::from_pixel(120, 80, image::Rgb([100, 100, 100])));
    let at = |vig: f32, dist: f32| EditRecipe {
        lens_profile: profile.clone(),
        lens_profile_vignetting_scale: vig,
        lens_profile_distortion_scale: dist,
        ..Default::default()
    };
    // (a) 100 is the identity, to the byte — and through the WHOLE tail, so
    // the distortion strength's effect on the resample counts too.
    let tail = |r: &EditRecipe| {
        frame_and_finish(
            develop_preview(&base, r),
            r,
            &geometry_profile(r),
            FilmScale::NATIVE,
            CropPolicy::Cut,
        )
        .to_rgb8()
    };
    let neutral = at(100.0, 100.0);
    let mut bare = neutral.clone();
    bare.lens_profile_vignetting_scale = 100.0;
    bare.lens_profile_distortion_scale = 100.0;
    assert_eq!(tail(&neutral).as_raw(), tail(&bare).as_raw(), "100 must be the identity");
    assert!(
        matches!(geometry_profile(&neutral), std::borrow::Cow::Borrowed(_)),
        "and at 100 with the CA pair at rest nothing is even allocated"
    );

    // (b) The VIGNETTING strength scales the lift: half at 50, none at 0,
    // and twice at 200. Read at a corner, where the gain is largest.
    let corner = |vig: f32| tail(&at(vig, 100.0))[(0, 0)][0] as f32;
    let (full, half, none, double) = (corner(100.0), corner(50.0), corner(0.0), corner(200.0));
    assert!(none < half && half < full && full < double, "{none} {half} {full} {double}");
    assert_eq!(none, 100.0, "0 is exactly no profile vignetting, so the corner is untouched");
    // HOW FAR is asserted on the gain LUT rather than on the rendered byte.
    // The claim is exact — Camera Raw's law: half the slider, half the EV;
    // 200, twice the departure from 1 — and an 8-bit corner cannot witness
    // it: the byte is rounded and the transfer pair is itself a LUT. The law
    // against Camera Raw's own renders is
    // `the_vignetting_strength_is_camera_raws_on_two_flat_fields`.
    // Ordering and the two endpoints above are exact and stay where the
    // photographer sees them; the arithmetic is checked where it lives.
    let gains = |s: f32| profile_vignette_lut(&profile.vignette, s);
    let (g100, g50, g200) = (gains(100.0), gains(50.0), gains(200.0));
    assert_eq!(
        g100,
        (0..LUT_N)
            .map(|i| profile_knot_interp(&profile.vignette, i as f32 / (LUT_N - 1) as f32))
            .collect::<Vec<_>>(),
        "100 hands the profile's own gains straight through"
    );
    for i in 0..LUT_N {
        assert!(
            (g50[i] - g100[i].sqrt()).abs() < 1e-5,
            "gain {i}: 50 must halve the lift in EV, {} vs {}",
            g50[i],
            g100[i]
        );
        assert!(
            ((g200[i] - 1.0) - (g100[i] - 1.0) * 2.0).abs() < 1e-6,
            "gain {i}: 200 must double it, {} vs {}",
            g200[i],
            g100[i]
        );
    }
    assert!(gains(0.0).iter().all(|g| *g == 1.0), "0 is exactly no correction");

    // (c) The DISTORTION strength scales the resample, and 0 switches the
    // stage OFF rather than running an identity spline — which is visible
    // in the profile it composes, not just in the pixels.
    let zero = at(100.0, 0.0);
    let composed = geometry_profile(&zero);
    assert!(composed.distortion.is_empty(), "0 drops the knots: {:?}", composed.distortion);
    assert!(!composed.geometry_active(), "…so the resample does not run at all");
    let dist_only = |d: f32| {
        let r = EditRecipe {
            lens_profile: LensProfile { vignette_on: false, ..profile.clone() },
            lens_profile_distortion_scale: d,
            ..Default::default()
        };
        geometry_profile(&r).distortion.clone()
    };
    let (k100, k50) = (dist_only(100.0), dist_only(50.0));
    assert_eq!(k100, profile.distortion, "100 hands the profile's own knots straight through");
    for (i, (a, b)) in k50.iter().zip(&k100).enumerate() {
        assert!(
            ((a - 1.0) - (b - 1.0) * 0.5).abs() < 1e-6,
            "knot {i}: 50 must halve the DEPARTURE from identity, {a} vs {b}"
        );
    }
}

#[test]
fn lens_profile_vignette_lifts_corners_only_and_geometry_roundtrips() {
    use crate::recipe::LensProfile;
    // Real A7RIV-shaped data (P26 conversions): rising corner gains,
    // falling distortion factors (barrel), near-unity CA.
    let profile = LensProfile {
        vignette: (0..16).map(|i| 1.0 + 0.42 * (i as f32 / 15.0).powi(2)).collect(),
        distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
        ca_r: vec![1.0005; 16],
        ca_b: vec![0.9995; 16],
        vignette_on: true,
        distortion_on: true,
        ca_on: true,
        // The mask warp plays no part in the PIXEL maps this test scores —
        // it is a mask-frame quantity and no resampler reads it.
        ..Default::default()
    };
    // (a) Vignette: corners brighten, the centre stays put.
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(120, 80, image::Rgb([100, 100, 100])));
    let vig_only = EditRecipe {
        lens_profile: LensProfile { distortion_on: false, ca_on: false, ..profile.clone() },
        ..Default::default()
    };
    let out = develop_preview(&base, &vig_only).to_rgb8();
    assert_eq!(out[(60, 40)][0], 100, "centre untouched (gain 1.0)");
    assert!(out[(0, 0)][0] > 110, "corner lifted, got {}", out[(0, 0)][0]);

    // (b) Geometry: forward/inverse round-trip through the composed map.
    let dims = (1200.0, 800.0);
    for (nx, ny) in [(0.1, 0.1), (0.5, 0.2), (0.85, 0.7), (0.5, 0.5)] {
        let (ox, oy) = lens_geom_norm(nx, ny, dims, &profile, 20.0);
        let (bx, by) = lens_ungeom_norm(ox, oy, dims, &profile, 20.0);
        assert!(
            (bx - nx).abs() < 2e-3 && (by - ny).abs() < 2e-3,
            "roundtrip ({nx},{ny}) → ({ox},{oy}) → ({bx},{by})"
        );
    }
    // Inactive profile must be EXACTLY the manual path.
    let off = LensProfile::default();
    assert_eq!(lens_geom_norm(0.3, 0.8, dims, &off, 33.0), distort_norm(0.3, 0.8, dims, 33.0));

    // (c) Resample: a barrel profile pulls samples inward (like the manual
    // barrel fix) and leaves no unfilled pixels; identity-profile resample
    // with CA stays within a hair of the plain image.
    let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(121, 81, image::Rgb([255, 255, 255])));
    let r = apply_lens_geometry(&white, &profile, 0.0).to_rgb16();
    let min = r.pixels().flat_map(|p| p.0).min().unwrap();
    assert!(min >= 65000, "unfilled pixels through the profile map: min {min}");
    // A flat frame cannot see WHERE samples come from — an identity
    // resampler passes the white probe (U14). Encode x into the value:
    // the barrel profile must pull the left edge inward (nonzero source
    // x → nonzero value), and CA must sample R and B at their own radii
    // (ca_r > 1 reaches farther out than green; ca_b < 1 less far).
    let ramp = DynamicImage::ImageRgb16(ImageBuffer::from_fn(121, 81, |x, _| {
        Rgb([(x as u16) * 500; 3])
    }));
    let g = apply_lens_geometry(&ramp, &profile, 0.0).to_rgb16();
    let p = g.get_pixel(0, 40).0;
    assert!(p[1] > 300, "profile distortion inert at the left edge: {p:?}");
    assert!(
        p[0] < p[1] && p[1] < p[2],
        "CA directions: ca_r > 1 samples farther out (smaller ramp value \
         at the left edge), ca_b < 1 nearer (larger) — a symmetric split \
         also passed a swapped R/B correction (Codex batch 40): {p:?}"
    );
}

/// The A7RIV frame every mask-frame measurement in R27 Batches 8-10 and
/// R29's `D` adjudication was made on.
const MASK_WARP_DIMS: (f32, f32) = (9504.0, 6336.0);

/// The 24 mm mask warp, from the SAME `.lcp` node the recon solved — one
/// fixture, so a change to either solver shows up as a disagreement rather
/// than as two independently drifting sets of numbers.
fn lcp_24mm_warp() -> Vec<f32> {
    let n = crate::lcp::PerspectiveModel {
        focal_mm: Some(24.0),
        focus_distance: Some(10000.0),
        scale: 1.027391,
        k: [-0.127336, 0.087661, -0.019675],
        focal_x: None,
        sensor_format_factor: 1.0,
        vignette: None,
    };
    n.mask_warp_knots(MASK_WARP_DIMS, crate::recipe::MASK_WARP_KNOTS).expect("solvable")
}

/// Source A and source B are two readings of ONE physical field, so they
/// have to agree — and the recon says by how much: the camera's own knot
/// map and Adobe's polynomial score 2.30 px and 2.11 px against the same
/// 138-patch pixel field, and differ from each other by 6.74 px rms over a
/// warp that reaches 185.7 px at the corner.
///
/// Asserted as a BAND, both ends. Too-large means one solver drifted; the
/// too-small end is the one that matters, because a source A that silently
/// became a copy of source B (or of the identity) would pass every other
/// test in this file.
#[test]
#[allow(clippy::excessive_precision)] // exact decoded 0x7037 fixture decimals
fn the_two_mask_warp_sources_agree_to_the_measured_tolerance() {
    // `exp_C_ref.ARW`'s OWN `0x7037` array, converted by `lensmeta`'s
    // `v·2⁻¹⁴ + 1` and dumped on this machine — the real A7RIV @ 24 mm
    // barrel profile, not a curve shaped like one.
    let camera_native: Vec<f32> = vec![
        1.0007934570,
        0.9998779297,
        0.9981079102,
        0.9959716797,
        0.9927368164,
        0.9890136719,
        0.9846191406,
        0.9800415039,
        0.9749145508,
        0.9696044922,
        0.9641113281,
        0.9589843750,
        0.9538574219,
        0.9492187500,
        0.9448242188,
        0.9412231445,
    ];
    let camera = crate::lensmeta::resample_sony_distortion(
        &camera_native,
        crate::lensmeta::SONY_DISTORTION_CANONICAL_KNOTS,
    );
    // The engine's own fill scale on that array, against the value the
    // recon computed independently: `profile_fill_scale` is the s_p this
    // whole inversion divides by, so a drift there is a silent drift in
    // every mask-warp knot below.
    assert!(
        (profile_fill_scale(&camera, MASK_WARP_DIMS) - 0.9755544).abs() < 1e-5,
        "fill scale {} vs the zero-parameter model's 0.9755544",
        profile_fill_scale(&camera, MASK_WARP_DIMS)
    );
    let a = mask_warp_from_camera_knots(
        &camera,
        MASK_WARP_DIMS,
        crate::recipe::MASK_WARP_KNOTS,
    );
    let b = lcp_24mm_warp();
    assert_eq!(a.len(), crate::recipe::MASK_WARP_KNOTS);
    assert_eq!(b.len(), crate::recipe::MASK_WARP_KNOTS);
    let half_diag = 0.5 * (MASK_WARP_DIMS.0.hypot(MASK_WARP_DIMS.1));
    let mut worst = 0.0f32;
    for i in 0..crate::recipe::MASK_WARP_KNOTS {
        let r = (i as f32 + 0.5) / (crate::recipe::MASK_WARP_KNOTS - 1) as f32 * half_diag;
        worst = worst.max(((a[i] - b[i]) * r).abs());
    }
    assert!(worst < 40.0, "the two sources diverged by {worst:.1} px");
    // PREMISE: both really are a warp, not the identity dressed up as one.
    for (name, k) in [("camera", &a), ("lcp", &b)] {
        let corner = (k[k.len() - 1] - 1.0) * half_diag;
        assert!(corner.abs() > 100.0, "{name} corner displacement {corner:.1} px is not a warp");
        assert!(k[0] < 0.995, "{name} centre magnification {} is not a warp", k[0]);
    }
}

/// ACCEPTANCE ⑦ (engine half). The forward map and its bisection inverse
/// compose to the identity across the frame — the property every consumer
/// of a coordinate map in this file is held to.
#[test]
fn the_mask_warp_point_map_round_trips() {
    let profile = crate::recipe::LensProfile {
        mask_warp: lcp_24mm_warp(),
        mask_warp_src: crate::recipe::MaskWarpSource::Lcp,
        ..Default::default()
    };
    let mut moved = 0.0f32;
    for i in 0..=10 {
        for j in 0..=10 {
            let (nx, ny) = (i as f32 / 10.0, j as f32 / 10.0);
            let (wx, wy) = lr_mask_warp_norm(nx, ny, MASK_WARP_DIMS, &profile);
            let (bx, by) = lr_mask_unwarp_norm(wx, wy, MASK_WARP_DIMS, &profile);
            assert!((bx - nx).abs() < 1e-4 && (by - ny).abs() < 1e-4, "({nx},{ny})");
            moved = moved.max(((wx - nx) * MASK_WARP_DIMS.0).abs());
        }
    }
    // PREMISE: a round trip through two identities also round-trips.
    assert!(moved > 50.0, "the map moved at most {moved:.1} px — it is not a warp");
    // The frame CENTRE is a fixed point of a radial map, exactly.
    assert_eq!(lr_mask_warp_norm(0.5, 0.5, MASK_WARP_DIMS, &profile), (0.5, 0.5));
}

fn d2_camera_profile(native: &[f32]) -> crate::recipe::LensProfile {
    let distortion = crate::lensmeta::resample_sony_distortion(
        native,
        crate::lensmeta::SONY_DISTORTION_CANONICAL_KNOTS,
    );
    crate::recipe::LensProfile {
        mask_warp: mask_warp_from_camera_knots(
            &distortion,
            MASK_WARP_DIMS,
            crate::recipe::MASK_WARP_KNOTS,
        ),
        mask_warp_src: crate::recipe::MaskWarpSource::CameraMetadata,
        mask_warp_center: Some(crate::recipe::MaskWarpCenter {
            stored_px: [4768.0, 3168.0],
            stored_dims: [9504.0, 6336.0],
        }),
        ..Default::default()
    }
}

#[allow(clippy::excessive_precision)]
fn d2_linear_wall_native() -> [f32; 16] {
    [
        1.0007934570,
        0.9998779297,
        0.9981079102,
        0.9959716797,
        0.9927368164,
        0.9890136719,
        0.9846191406,
        0.9800415039,
        0.9749145508,
        0.9696044922,
        0.9641113281,
        0.9589843750,
        0.9538574219,
        0.9492187500,
        0.9448242188,
        0.9412231445,
    ]
}

fn d2_linear_probe(
    zero: (f32, f32),
    full: (f32, f32),
) -> crate::recipe::LocalAdjustment {
    crate::recipe::LocalAdjustment {
        mask: MaskGeometry::Linear {
            zero_x: zero.0,
            zero_y: zero.1,
            full_x: full.0,
            full_y: full.1,
        },
        ..Default::default()
    }
}

fn d2_disabled_linear_profile(
    downstream_geometry: bool,
) -> (crate::recipe::LensProfile, crate::recipe::LensProfile) {
    let native = d2_linear_wall_native();
    let camera = d2_camera_profile(&native);
    let mut disabled = camera.clone();
    disabled.distortion = native.to_vec();
    disabled.distortion_on = downstream_geometry;
    disabled.linear_handle_warp = std::mem::take(&mut disabled.mask_warp);
    disabled.mask_warp_src = crate::recipe::MaskWarpSource::DisabledInSidecar;
    disabled.clamp();
    (camera, disabled)
}

fn d2_linear_handles(mask: &crate::recipe::LocalAdjustment) -> [(f32, f32); 2] {
    let MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } = &mask.mask else {
        panic!("expected LINEAR fixture")
    };
    [(*zero_x, *zero_y), (*full_x, *full_y)]
}

fn d2_midline_x_at_y(handles: [(f32, f32); 2], y: f32, dims: (f32, f32)) -> f32 {
    let [(zx, zy), (fx, fy)] = handles.map(|(x, y)| (x * dims.0, y * dims.1));
    let (mx, my) = ((zx + fx) * 0.5, (zy + fy) * 0.5);
    mx - (y - my) * (fy - zy) / (fx - zx)
}

fn d2_midline_y_at_x(handles: [(f32, f32); 2], x: f32, dims: (f32, f32)) -> f32 {
    let [(zx, zy), (fx, fy)] = handles.map(|(x, y)| (x * dims.0, y * dims.1));
    let (mx, my) = ((zx + fx) * 0.5, (zy + fy) * 0.5);
    my - (x - mx) * (fx - zx) / (fy - zy)
}

/// The coverage the shipped LINEAR profile puts at the geometric midline.
/// `linear_coverage` is not symmetric about t = ½ (me6-2026-09 group B
/// measured the abscissa warp), so a bisection hunting for the midline
/// must hunt for THIS level rather than for a hard-coded 0.5.
fn d2_midline_coverage() -> f32 {
    linear_coverage(0.5, LINEAR_FALLOFF)
}

fn d2_coverage_crossing_x(
    mask: &crate::recipe::LocalAdjustment,
    y: f32,
    dims: (f32, f32),
) -> f32 {
    let ny = y / dims.1;
    let end = |nx| combined_mask_weight(mask, nx, ny, None, &[], None, dims);
    let increasing = end(1.0) > end(0.0);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if (end(mid) < d2_midline_coverage()) == increasing {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) * 0.5 * dims.0
}

fn d2_coverage_crossing_y(
    mask: &crate::recipe::LocalAdjustment,
    x: f32,
    dims: (f32, f32),
) -> f32 {
    let nx = x / dims.0;
    let end = |ny| combined_mask_weight(mask, nx, ny, None, &[], None, dims);
    let increasing = end(1.0) > end(0.0);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if (end(mid) < d2_midline_coverage()) == increasing {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) * 0.5 * dims.1
}

fn d2_gray_crossing_x(image: &image::GrayImage, y: u32) -> f32 {
    for x in 0..image.width() - 1 {
        let a = image.get_pixel(x, y)[0] as f32 - 127.5;
        let b = image.get_pixel(x + 1, y)[0] as f32 - 127.5;
        if a != b && a.signum() != b.signum() {
            return x as f32 + 0.5 + (-a) / (b - a);
        }
    }
    panic!("coverage row {y} never crossed 50%")
}

fn d2_rgb16_crossing_x_at(image: &DynamicImage, y: u32, target: f32) -> f32 {
    let image = image.to_rgb16();
    let mut range = (u16::MAX, u16::MIN);
    for x in 0..image.width() - 1 {
        range.0 = range.0.min(image.get_pixel(x, y)[1]);
        range.1 = range.1.max(image.get_pixel(x, y)[1]);
        let a = image.get_pixel(x, y)[1] as f32 - target;
        let b = image.get_pixel(x + 1, y)[1] as f32 - target;
        if a != b && a.signum() != b.signum() {
            return x as f32 + 0.5 + (-a) / (b - a);
        }
    }
    panic!("coverage row {y} range {range:?} never crossed target {target}")
}

/// LINEAR under an active lens geometry (v1.17.0, Camera Raw): both handles
/// are sensor-frame points carried with the photograph by the engine's own
/// map, and the gradient is a straight line between the carried handles in
/// the corrected output. Camera Raw at 0.25 EV on p39 and p38 (three
/// gradients): this rule's weight map against Camera Raw's own scores IoU½
/// 0.9962 / 0.9980 / 0.9902 where the stored-handles line scored 0.9891 (the
/// engine then) / 0.9918 / 0.9815.
///
/// MUTATION THIS KILLS: drop the handle carry in
/// `MaskFrame::prepare_linear_handles` (the line lands on the stored handles,
/// `moved` px off), or carry the handles without sampling through
/// `MaskUnwarp::engine_at` (the line lands at the carried handles carried
/// twice).
#[test]
fn linear_with_active_camera_profile_lands_on_the_line_through_its_carried_handles() {
    let (w, h) = (1920u32, 1280u32);
    let dims = (w as f32, h as f32);
    let mut profile = d2_camera_profile(&d2_linear_wall_native());
    profile.distortion = d2_linear_wall_native().to_vec();
    profile.distortion_on = true;
    let (zero, full) = ((0.33f32, 0.5f32), (0.27f32, 0.5f32));
    let mut mask = d2_linear_probe(zero, full);
    mask.exposure_ev = -4.0;
    let frame = MaskFrame::downstream(&profile, 0.0);
    assert!(frame.warps(), "premise: camera geometry must be active");

    // Where the photograph carries each handle, and the output midline q
    // between them (both handles sit on the centre row, which the radial map
    // keeps, so q is on that row too).
    let carried = [zero, full].map(|(x, y)| lens_ungeom_norm(x, y, dims, &profile, 0.0));
    let q = (0.5 * (carried[0].0 + carried[1].0), 0.5f32);
    let moved = (q.0 - 0.5 * (zero.0 + full.0)).abs() * dims.0;
    assert!(moved > 2.0, "premise: the photograph carries the midline only {moved:.3}px");

    // Engine-math pin: the output midline q asks the pre-geometry coverage
    // at its source p, through the prepared (carried) handles.
    let p = lens_geom_norm(q.0, q.1, dims, &profile, 0.0);
    let unwarp = frame.unwarp(dims).expect("active non-identity engine map");
    let prepared = frame.prepare_linear_handles(&mask, dims);
    let weight = mask_weight_in(&prepared.mask, p.0, p.1, None, Some(&unwarp), dims);
    // The midline's coverage is `d2_midline_coverage()`, not 0.5: the
    // shipped profile is eased on a warped abscissa. Dividing the coverage
    // error by the span rather than by the profile's own slope (1.535 at
    // the midline) makes this bound 1.5x TIGHTER than the pixel error it
    // names, which is the safe direction.
    let math_error_px = (weight - d2_midline_coverage()).abs() * 0.06 * dims.0;
    assert!(math_error_px < 0.1, "active LINEAR midline math is {math_error_px:.4}px off");

    // Raster pin: run the exact coverage-then-geometry composition used by
    // the GUI overlay and measure its 50% line in corrected output pixels.
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    let coverage = DynamicImage::ImageLuma8(mask_coverage(&mask, &base, frame));
    let rendered = apply_lens_geometry(&coverage, &profile, 0.0);
    // 255 codes then 257 per code: the same 8-bit ladder the old 32767.5
    // target rode, re-aimed at the midline's coverage (0.43837 -> 28728.6).
    let got = d2_rgb16_crossing_x_at(&rendered, h / 2, d2_midline_coverage() * 255.0 * 257.0);
    let expected = q.0 * dims.0;
    // The public coverage raster is 8-bit, so its rounded midline code adds
    // a measured sub-0.2 px crossing bias; the float engine-law assertion
    // above is the sub-0.1 px contract, while this pins end-to-end wiring.
    assert!((got - expected).abs() < 0.3, "rendered {got:.4}px vs carried {expected:.4}px");

    // The actual local-adjustment renderer uses the same frame preparation,
    // independently of the coverage-overlay entry point above.
    let recipe = EditRecipe {
        masks: vec![mask],
        lens_profile: profile.clone(),
        ..Default::default()
    };
    let effect = apply_lens_geometry(&develop_preview(&base, &recipe), &profile, 0.0);
    let effect_rgb = effect.to_rgb16();
    // The mask blend `p·(1 − w) + t·w` runs in the same sRGB-gamma domain
    // the output is rounded from, so the rendered value is AFFINE in the
    // coverage `w` and the midline's level is simply the two plateaus
    // interpolated at `d2_midline_coverage()`. This was the mean of the
    // two while the profile was symmetric about t = ½; it no longer is.
    // Interpolating (rather than reading the level off a probe render)
    // also keeps the target off the 8-bit ladder — on this 0.84-code-per-
    // pixel ramp a whole-code target is worth 0.6 px of bias.
    let zero_end = effect_rgb.get_pixel(w - 1, h / 2)[1] as f32;
    let full_end = effect_rgb.get_pixel(0, h / 2)[1] as f32;
    let target = zero_end + d2_midline_coverage() * (full_end - zero_end);
    let effect_crossing = d2_rgb16_crossing_x_at(&effect, h / 2, target);
    assert!(
        (effect_crossing - expected).abs() < 0.35,
        "active render {effect_crossing:.4}px vs carried {expected:.4}px"
    );
}

/// The twin with the correction OFF (a sidecar that says
/// `crs:LensProfileEnable="0"`): nothing moves the photograph, so nothing
/// moves the handles — the gradient sits on its STORED line, even though the
/// profile still holds the camera map the 2026-08 H2 rule sent the handles
/// through (`D_fwd`, about 30 px on these three wall fixtures). Camera Raw at
/// 0.25 EV, correction off, stored handles against `D_fwd`: IoU½ 0.9983 vs
/// 0.9956 and 0.9905 vs 0.9810 (p38), 0.9967 vs 0.9944 (p39).
///
/// MUTATION THIS KILLS: any transport of the handles when no geometry follows.
#[test]
fn linear_with_correction_off_stays_on_its_stored_handles() {
    let (camera, disabled) = d2_disabled_linear_profile(false);
    assert!(disabled.mask_warp.is_empty(), "RADIAL map must be disabled");
    assert_eq!(disabled.linear_handle_warp, camera.mask_warp, "premise: the camera map is still held");
    let frame = MaskFrame::downstream(&disabled, 0.0);
    assert!(!frame.warps(), "corrections-off fixture must have no downstream geometry");

    let fixtures = [
        ("L1", (0.33, 0.50), (0.27, 0.50), true, 3168.0),
        ("L2", (0.69, 0.50), (0.75, 0.50), true, 3168.0),
        ("L3", (0.50, 0.27), (0.50, 0.21), false, 4752.0),
    ];
    for (name, zero, full, vertical, along) in fixtures {
        let stored = d2_linear_probe(zero, full);
        let rendered = frame.prepare_linear_handles(&stored, MASK_WARP_DIMS);
        assert_eq!(d2_linear_handles(rendered.as_ref()), [zero, full], "{name}: the handles moved");
        let (got, expected) = if vertical {
            (
                d2_coverage_crossing_x(rendered.as_ref(), along, MASK_WARP_DIMS),
                d2_midline_x_at_y([zero, full], along, MASK_WARP_DIMS),
            )
        } else {
            (
                d2_coverage_crossing_y(rendered.as_ref(), along, MASK_WARP_DIMS),
                d2_midline_y_at_x([zero, full], along, MASK_WARP_DIMS),
            )
        };
        assert!((got - expected).abs() < 0.1, "{name}: render {got:.4}px vs stored {expected:.4}px");
        // PREMISE: the old rule really would have moved this one.
        let d_fwd = [zero, full].map(|(x, y)| lr_mask_unwarp_norm(x, y, MASK_WARP_DIMS, &camera));
        let h2 = if vertical {
            d2_midline_x_at_y(d_fwd, along, MASK_WARP_DIMS)
        } else {
            d2_midline_y_at_x(d_fwd, along, MASK_WARP_DIMS)
        };
        assert!((h2 - expected).abs() > 20.0, "premise: D_fwd moves {name} only {:.3}px", h2 - expected);
    }
}

#[test]
fn linear_off_path_rendered_boundary_stays_straight_instead_of_h1_bowing() {
    let (camera, disabled) = d2_disabled_linear_profile(false);
    let (w, h) = (1188u32, 792u32);
    let dims = (w as f32, h as f32);
    let mut mask = d2_linear_probe((0.33, 0.5), (0.27, 0.5));
    mask.exposure_ev = -4.0;
    let frame = MaskFrame::downstream(&disabled, 0.0);
    let coverage = mask_coverage(&mask, &DynamicImage::new_rgb8(w, h), frame);
    let rows = [h / 10, h / 2, h - h / 10 - 1];
    let crossings = rows.map(|y| d2_gray_crossing_x(&coverage, y));
    let sag = crossings[1] - 0.5 * (crossings[0] + crossings[2]);
    assert!(sag.abs() < 0.5, "correction-off rendered boundary sagged {sag:.3}px: {crossings:?}");

    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    let recipe = EditRecipe {
        masks: vec![mask],
        lens_profile: disabled.clone(),
        ..Default::default()
    };
    let effect = develop_preview(&base, &recipe);
    let effect_rgb = effect.to_rgb16();
    let effect_crossings = rows.map(|y| {
        let target = 0.5
            * (effect_rgb.get_pixel(0, y)[1] as f32
                + effect_rgb.get_pixel(w - 1, y)[1] as f32);
        d2_rgb16_crossing_x_at(&effect, y, target)
    });
    for (y, (got, coverage_got)) in rows.into_iter().zip(effect_crossings.into_iter().zip(crossings)) {
        assert!(
            (got - coverage_got).abs() < 0.5,
            "row {y}: local render {got:.3}px vs coverage {coverage_got:.3}px"
        );
    }

    // Adversarial control: pointwise H1 on this same fixture bows by
    // multiple working-frame pixels, so the straightness gate is not an
    // axis-aligned identity test that both topologies can pass.
    let h1_crossing = |raw_y: f32| {
        let target_y = raw_y / dims.1;
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..40 {
            let mid = (lo + hi) * 0.5;
            if lr_mask_unwarp_norm(0.30, mid, dims, &camera).1 < target_y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lr_mask_unwarp_norm(0.30, (lo + hi) * 0.5, dims, &camera).0 * dims.0
    };
    let h1 = rows.map(|y| h1_crossing(y as f32 + 0.5));
    let h1_sag = h1[1] - 0.5 * (h1[0] + h1[2]);
    assert!(
        h1_sag.abs() > 1.5,
        "premise: pointwise H1 sag is only {h1_sag:.3}px on {h1:?}"
    );
}

/// A radial follows the photograph whatever the sidecar said about
/// Lightroom's correction: its centre is a point ON the picture (v1.17.0,
/// Camera Raw: off-centre ellipses move exactly as the content moves there),
/// so when a downstream geometry does run on a disabled-sidecar profile —
/// a state the pipeline no longer builds, since `obey_disabled_profile_switch`
/// turns the components off, but a profile can still be handed in — the
/// effect lands where that geometry carries its stored centre, and the
/// retained LINEAR map plays no part. (It used to sit at the stored
/// coordinates through the empty `mask_warp`, off its own content.)
///
/// MUTATION THIS KILLS: route the radial through `linear_handle_warp`, or
/// back to the stored point on a disabled-sidecar profile.
#[test]
fn radial_with_disabled_profile_and_retained_linear_map_follows_the_photograph() {
    let (_, disabled) = d2_disabled_linear_profile(true);
    assert!(disabled.geometry_active(), "premise: downstream geometry is active");
    assert!(disabled.mask_warp.is_empty(), "the disabled RADIAL spline stays empty");
    assert!(!disabled.linear_handle_warp.is_empty(), "premise: LINEAR map was retained");

    let (w, h) = (1920u32, 1280u32);
    let (cx, cy) = (0.30f32, 0.50f32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    let recipe = EditRecipe {
        masks: vec![probe_radial(cx, cy, 0.025)],
        lens_profile: disabled.clone(),
        ..Default::default()
    };
    let rendered = apply_lens_geometry(&develop_preview(&base, &recipe), &disabled, 0.0);
    let got = effect_centroid(&rendered);
    let landed = lens_ungeom_norm(cx, cy, (w as f32, h as f32), &disabled, 0.0);
    let expected = (landed.0 as f64 * w as f64 - 0.5, landed.1 as f64 * h as f64 - 0.5);
    let moved = (expected.0 - (cx as f64 * w as f64 - 0.5)).hypot(expected.1 - (cy as f64 * h as f64 - 0.5));
    let error = (got.0 - expected.0).hypot(got.1 - expected.1);
    assert!(moved > 2.0, "premise: the geometry moves this centre ({moved:.3}px)");
    assert!(error < 1.0, "disabled RADIAL off its content by {error:.3}px: {got:?} vs {expected:?}");
}

#[test]
#[allow(clippy::excessive_precision)] // exact D2 knot/vector fixture decimals
fn d2_zero_parameter_camera_model_closes_all_41_measured_vectors() {
    const WALL_NATIVE: [f32; 16] = [
        1.0007934570,
        0.9998779297,
        0.9981079102,
        0.9959716797,
        0.9927368164,
        0.9890136719,
        0.9846191406,
        0.9800415039,
        0.9749145508,
        0.9696044922,
        0.9641113281,
        0.9589843750,
        0.9538574219,
        0.9492187500,
        0.9448242188,
        0.9412231445,
    ];
    const DSC_NATIVE: [f32; 16] = [
        1.0007934570,
        0.9998779297,
        0.9982299805,
        0.9961547852,
        0.9932250977,
        0.9898071289,
        0.9856567383,
        0.9813842773,
        0.9766235352,
        0.9719238281,
        0.9668579102,
        0.9622802734,
        0.9576416016,
        0.9538574219,
        0.9503173828,
        0.9478149414,
    ];
    type Row = (&'static str, (f32, f32), (f32, f32));
    const WALL: [Row; 20] = [
        ("G1", (1710.72, 1267.20), (-20.897, -12.931)),
        ("G2", (4752.00, 1267.20), (-0.011, 31.992)),
        ("G3", (7793.28, 1267.20), (19.025, -12.135)),
        ("G4", (1710.72, 3168.00), (4.924, -0.025)),
        ("G5", (4752.00, 3168.00), (0.244, -0.019)),
        ("G6", (7793.28, 3168.00), (-6.669, 0.026)),
        ("G7", (1710.72, 5068.80), (-20.836, 12.932)),
        ("G8", (4752.00, 5068.80), (0.097, -31.970)),
        ("G9", (7793.28, 5068.80), (19.105, 12.141)),
        ("R1", (2851.20, 2027.52), (24.874, 14.890)),
        ("R2", (6652.80, 3991.68), (-28.390, -12.427)),
        ("centre_S", (4752.00, 3168.00), (0.535, 0.035)),
        ("centre_M", (4752.00, 3168.00), (0.393, 0.009)),
        ("centre_L", (4752.00, 3168.00), (0.303, 0.010)),
        ("edge_S", (1710.72, 3168.00), (4.962, -0.040)),
        ("edge_M", (1710.72, 3168.00), (5.085, -0.083)),
        ("edge_L", (1710.72, 3168.00), (4.990, 0.037)),
        ("corner_S", (1710.72, 1267.20), (-20.927, -12.969)),
        ("corner_M", (1710.72, 1267.20), (-20.710, -12.895)),
        ("corner_L", (1710.72, 1267.20), (-20.880, -12.903)),
    ];
    const DSC: [Row; 21] = [
        ("G1", (1710.72, 1267.20), (-18.900, -11.770)),
        ("G2", (4752.00, 1267.20), (0.140, 29.250)),
        ("G3", (7793.28, 1267.20), (17.480, -11.020)),
        ("G4", (1710.72, 3168.00), (4.610, -0.010)),
        ("G5", (4752.00, 3168.00), (0.300, -0.070)),
        ("G6", (7793.28, 3168.00), (-5.920, -0.020)),
        ("G7", (1710.72, 5068.80), (-18.360, 11.960)),
        ("G8", (4752.00, 5068.80), (0.230, -29.280)),
        ("G9", (7793.28, 5068.80), (17.570, 11.000)),
        ("original", (2283.03, 951.41), (-5.820, -5.100)),
        ("R1", (2851.20, 2027.52), (22.950, 13.600)),
        ("R2", (6652.80, 3991.68), (-26.060, -11.330)),
        ("centre_S", (4752.00, 3168.00), (0.620, -0.080)),
        ("centre_M", (4752.00, 3168.00), (0.420, -0.030)),
        ("centre_L", (4752.00, 3168.00), (0.400, 0.040)),
        ("edge_S", (1710.72, 3168.00), (4.760, -0.050)),
        ("edge_M", (1710.72, 3168.00), (4.380, -0.030)),
        ("edge_L", (1710.72, 3168.00), (4.550, -0.070)),
        ("corner_S", (1710.72, 1267.20), (-18.790, -11.690)),
        ("corner_M", (1710.72, 1267.20), (-18.740, -11.730)),
        ("corner_L", (1710.72, 1267.20), (-18.930, -11.730)),
    ];

    let check = |label: &str, native: &[f32], rows: &[Row], expected_rms: f32| {
        let profile = d2_camera_profile(native);
        let mut sum_sq = 0.0f32;
        let mut worst = 0.0f32;
        for &(cell, (x, y), (mx, my)) in rows {
            let (wx, wy) =
                lr_mask_warp_norm(x / MASK_WARP_DIMS.0, y / MASK_WARP_DIMS.1, MASK_WARP_DIMS, &profile);
            let (px, py) = (wx * MASK_WARP_DIMS.0 - x, wy * MASK_WARP_DIMS.1 - y);
            let err = (px - mx).hypot(py - my);
            sum_sq += err * err;
            worst = worst.max(err);
            assert!(err <= 1.0, "{label}/{cell}: predicted ({px},{py}), measured ({mx},{my}), error {err}");
        }
        let rms = (sum_sq / rows.len() as f32).sqrt();
        assert!((rms - expected_rms).abs() < 0.02, "{label}: rms {rms}, max {worst}");
    };
    check("wall", &WALL_NATIVE, &WALL, 0.568);
    check("P26", &DSC_NATIVE, &DSC, 0.243);
}

#[test]
fn mask_warp_uses_the_full_raw_centre_in_stored_coordinates() {
    let shifted = crate::recipe::LensProfile {
        mask_warp: vec![1.05; crate::recipe::MASK_WARP_KNOTS],
        mask_warp_src: crate::recipe::MaskWarpSource::CameraMetadata,
        mask_warp_center: Some(crate::recipe::MaskWarpCenter {
            stored_px: [4768.0, 3168.0],
            stored_dims: [9504.0, 6336.0],
        }),
        ..Default::default()
    };
    let fixed = (4768.0 / MASK_WARP_DIMS.0, 3168.0 / MASK_WARP_DIMS.1);
    assert_eq!(lr_mask_warp_norm(fixed.0, fixed.1, MASK_WARP_DIMS, &shifted), fixed);
    let stored_centre = lr_mask_warp_norm(0.5, 0.5, MASK_WARP_DIMS, &shifted);
    assert!(stored_centre.0 < 0.5, "shifted centre was ignored: {stored_centre:?}");

    // The same stored-pixel centre scales with a working-resolution
    // preview instead of remaining thousands of pixels off-frame.
    let preview_dims = (950.4, 633.6);
    let preview_fixed = (476.8 / preview_dims.0, 316.8 / preview_dims.1);
    assert_eq!(
        lr_mask_warp_norm(preview_fixed.0, preview_fixed.1, preview_dims, &shifted),
        preview_fixed
    );

    let legacy = crate::recipe::LensProfile { mask_warp_center: None, ..shifted };
    assert_eq!(lr_mask_warp_norm(0.5, 0.5, MASK_WARP_DIMS, &legacy), (0.5, 0.5));
}

/// RADIAL's sample on an active geometry is the engine map less the
/// displacement the same map gives the stored centre: a translation of the
/// ellipse, its radii in the corrected frame, its centre carried with the
/// photograph (v1.17.0, measured against Camera Raw). A profile whose
/// Lightroom mask-warp spline differs from its render map is the case that
/// tells the two transports apart.
///
/// MUTATION: sample through the Lightroom inverse pointwise again (the
/// ellipse dilates by the local magnification), move the centre by the
/// mask-warp spline, or drop the centre's displacement (off-centre
/// ellipses stop following the lens).
#[test]
fn mask_frame_translates_a_radial_by_its_centres_engine_displacement() {
    let profile = crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 - 0.12 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        mask_warp: (0..crate::recipe::MASK_WARP_KNOTS)
            .map(|i| 0.98 + 0.05 * i as f32 / (crate::recipe::MASK_WARP_KNOTS - 1) as f32)
            .collect(),
        mask_warp_src: crate::recipe::MaskWarpSource::CameraMetadata,
        mask_warp_center: Some(crate::recipe::MaskWarpCenter {
            stored_px: [4768.0, 3168.0],
            stored_dims: [9504.0, 6336.0],
        }),
        ..Default::default()
    };
    let dims = MASK_WARP_DIMS;
    let u = MaskUnwarp::new(&profile, 0.0, dims).expect("the engine map is active");
    let centre = (0.08f32, 0.12f32);
    let landed = lens_ungeom_norm(centre.0, centre.1, dims, &profile, 0.0);
    assert!((landed.0 - centre.0).abs() > 1e-3, "premise: the map moves this centre");
    let spline = lr_mask_warp_norm(centre.0, centre.1, dims, &profile);
    assert!((spline.0 - landed.0).abs() > 1e-3, "premise: the mask-warp spline would move it elsewhere");
    for (nx, ny) in [(0.15, 0.2), (0.5, 0.5), (0.82, 0.74)] {
        let engine = lens_ungeom_norm(nx, ny, dims, &profile, 0.0);
        let expected = (engine.0 - (landed.0 - centre.0), engine.1 - (landed.1 - centre.1));
        let got = u.radial_at(nx, ny, centre.0, centre.1);
        assert!((got.0 - expected.0).abs() < 2e-4 && (got.1 - expected.1).abs() < 2e-4, "{got:?} vs {expected:?}");
    }
    // The centre itself samples the stored centre: the ellipse is not moved
    // off its own content.
    let at_centre = u.radial_at(centre.0, centre.1, centre.0, centre.1);
    assert!((at_centre.0 - centre.0).abs() < 1e-6 && (at_centre.1 - centre.1).abs() < 1e-6);
}

/// ACCEPTANCE ⑤. With no solved warp the map is the IDENTITY — bit-for-bit,
/// not approximately — and the reason is available by name rather than
/// inferred from an empty vector.
#[test]
fn an_absent_profile_is_an_identity_warp_with_a_named_reason() {
    use crate::recipe::MaskWarpSource as S;
    let none = crate::recipe::LensProfile::default();
    assert_eq!(none.mask_warp_src, S::Absent);
    for i in 0..=7 {
        for j in 0..=7 {
            let (nx, ny) = (i as f32 / 7.0, j as f32 / 7.0);
            assert_eq!(lr_mask_warp_norm(nx, ny, MASK_WARP_DIMS, &none), (nx, ny));
            assert_eq!(lr_mask_unwarp_norm(nx, ny, MASK_WARP_DIMS, &none), (nx, ny));
        }
    }
    // Every "no warp" state names itself, and none of them is silent.
    for s in S::ALL {
        assert!(!s.en().is_empty(), "{s:?} has no prose");
        let mut p = crate::recipe::LensProfile { mask_warp_src: s, ..Default::default() };
        // A tag that is not SOLVED cannot keep knots — `clamp` enforces it,
        // so a hand-edited recipe cannot claim "fisheye refused" and warp.
        p.mask_warp = vec![1.02; 16];
        p.clamp();
        if s.is_solved() {
            assert_eq!(p.mask_warp.len(), 16, "{s:?} is a solved source");
        } else {
            assert!(p.mask_warp.is_empty(), "{s:?} kept knots it has no claim to");
            assert_eq!(lr_mask_warp_norm(0.3, 0.7, MASK_WARP_DIMS, &p), (0.3, 0.7));
        }
    }
    // Empty knots in, empty out: "no data" is not a warp of 1.0.
    assert!(mask_warp_from_camera_knots(&[], MASK_WARP_DIMS, 16).is_empty());
}

/// A strong, real-shaped barrel profile for the frame-wiring tests:
/// falling radius factors, the shape every Sony `0x7037` array has.
fn barrel_profile() -> crate::recipe::LensProfile {
    crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 - 0.12 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        ..Default::default()
    }
}

/// A hard-edged radial at a KNOWN stored centre, dark enough to find.
fn probe_radial(cx: f32, cy: f32, r: f32) -> crate::recipe::LocalAdjustment {
    crate::recipe::LocalAdjustment {
        mask: MaskGeometry::Radial {
            top: cy - r,
            left: cx - r,
            bottom: cy + r,
            right: cx + r,
            feather: 0.0,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 0,
        },
        exposure_ev: -4.0,
        ..Default::default()
    }
}

/// Centroid of the darkened pixels, in output-frame pixels.
fn effect_centroid(img: &DynamicImage) -> (f64, f64) {
    let g = img.to_rgb8();
    let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y, p) in g.enumerate_pixels() {
        if p.0[1] < 90 {
            sx += x as f64;
            sy += y as f64;
            n += 1.0;
        }
    }
    assert!(n > 40.0, "the mask must darken a real region, got {n} px");
    (sx / n, sy / n)
}


/// ACCEPTANCE ⑥, REWRITTEN twice — the 2026-08-20 user ruling, then
/// v1.17.0 against Camera Raw: a radial under an active geometry stage
/// FOLLOWS THE PHOTOGRAPH at its STORED SIZE.
///
/// # What this replaces, and why
///
/// The 2026-08 version pinned the radial's effect on its STORED centre in
/// the output, from the `D` adjudication's reading that Lightroom does not
/// move a parametric shape. Camera Raw, rendering feather-0 ellipses with
/// the lens correction on and off on four frames (24–105 mm at 45 / 73 /
/// 105 mm, 14–24 mm at 20 mm), moves an off-centre ellipse's centre exactly
/// as the frame's own content moves there (0.0286 / 0.0298 of the centre's
/// offset against the content's 0.029) and keeps its radii at the stored
/// size (a centred ellipse's edge at 1.0025–1.0055, not the 1.045 a
/// pointwise transport gives). So the wired chain lands the effect where
/// the engine's own map carries the stored centre, at the stored area,
/// while the pointwise control ([`MaskFrame::AsRendered`], the mask drawn
/// in the sensor frame and warped with it) lands on the same centre but
/// with its area scaled by the local magnification.
///
/// MUTATION THIS KILLS: dropping or retargeting the explicit RADIAL/LINEAR
/// arms in `mask_weight_in` so RADIAL no longer uses `MaskUnwarp::radial_at`
/// or LINEAR no longer uses `MaskUnwarp::engine_at`; dropping the centre's
/// displacement in `radial_at` (the effect sits on the stored centre).
#[test]
fn a_parametric_radial_follows_the_photograph_at_its_stored_size_under_lens_geometry() {
    let (w, h) = (960u32, 640u32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    // A stronger barrel than `barrel_profile`, so the field is many pixels
    // rather than one: corner factor 0.75, fill scale 0.923.
    let profile = crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 - 0.25 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        ..Default::default()
    };
    let darkened = |img: &DynamicImage| img.to_rgb8().pixels().filter(|p| p.0[1] < 90).count() as f64;
    // TWO placements at different radii: the frame centre is a fixed point
    // of every radial map, so one off-centre mask could pass by accident.
    for cx in [0.10f32, 0.32f32] {
        let (cy, r) = (0.5f32, 0.05f32);
        let recipe = EditRecipe {
            masks: vec![probe_radial(cx, cy, r)],
            lens_profile: profile.clone(),
            ..Default::default()
        };
        let landed = lens_ungeom_norm(cx, cy, (w as f32, h as f32), &profile, 0.0);
        let want = (landed.0 as f64 * w as f64 - 0.5, landed.1 as f64 * h as f64 - 0.5);
        let moved = (want.0 - (cx as f64 * w as f64 - 0.5)).hypot(want.1 - (cy as f64 * h as f64 - 0.5));
        let area = std::f64::consts::PI * (r * w as f32) as f64 * (r * h as f32) as f64;
        let wired = apply_lens_geometry(&develop_preview(&base, &recipe), &profile, 0.0);
        let control = apply_lens_geometry(
            &develop_preview_framed(&base, &recipe, &crate::diag::pixels(), MaskFrame::AsRendered, None, false),
            &profile,
            0.0,
        );
        let got = effect_centroid(&wired);
        let err = (got.0 - want.0).hypot(got.1 - want.1);
        let (a_wired, a_control) = (darkened(&wired) / area, darkened(&control) / area);
        eprintln!("cx={cx}: moved {moved:.2} px, centroid error {err:.2} px, area wired {a_wired:.4} control {a_control:.4}");
        assert!(moved > 5.0, "premise: the field moves this centre ({moved:.2} px)");
        assert!(err < 1.5, "cx={cx}: the effect must sit where the photograph carries its centre: {err:.2} px off ({got:?} vs {want:?})");
        assert!((a_wired - 1.0).abs() < 0.03, "cx={cx}: the stored size, not a warped one: area x{a_wired:.4}");
        assert!((a_control - 1.0).abs() > 3.0 * (a_wired - 1.0).abs().max(0.01), "premise: the pointwise control changes the size (x{a_control:.4})");
    }
}

/// The centred feather-0 radial's EDGE under a lens field that magnifies
/// the frame centre, pinned against Camera Raw (v1.17.0): with the
/// correction on, a 105 mm frame's centred ellipse ends at 1.0052 of its
/// stored radius in Camera Raw's render (lens off 1.0003), where the
/// 2026-08 pointwise transport put it at 1.0449. The wired chain must put
/// the edge within 0.5 % of Camera Raw's reading — at the stored radius;
/// the pointwise control dilates it by the centre's magnification.
///
/// MUTATION THIS KILLS: sample the ellipse through any pointwise map (the
/// Lightroom spline or the engine's) instead of translating it.
#[test]
fn a_centred_radial_ends_at_camera_raws_edge_under_lens_geometry() {
    const CAMERA_RAW_EDGE: f64 = 1.0052;
    let (w, h) = (1200u32, 800u32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    // Pincushion knots: the correction pulls the corners in and the fill
    // scale magnifies the centre, as at 105 mm.
    let profile = crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 + 0.10 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        ..Default::default()
    };
    let r = 0.3f32;
    let recipe = EditRecipe { masks: vec![probe_radial(0.5, 0.5, r)], lens_profile: profile.clone(), ..Default::default() };
    // The darkened run along the centre row, in units of the stored radius.
    let edge = |img: &DynamicImage| {
        let g = img.to_rgb8();
        let n = (0..w).filter(|&x| g.get_pixel(x, h / 2).0[1] < 90).count() as f64;
        n / (2.0 * (r * w as f32) as f64)
    };
    let wired = edge(&apply_lens_geometry(&develop_preview(&base, &recipe), &profile, 0.0));
    let control = edge(&apply_lens_geometry(
        &develop_preview_framed(&base, &recipe, &crate::diag::pixels(), MaskFrame::AsRendered, None, false),
        &profile,
        0.0,
    ));
    eprintln!("edge: wired {wired:.4}, pointwise control {control:.4}, Camera Raw {CAMERA_RAW_EDGE}");
    assert!(control > 1.03, "premise: the field magnifies the centre (control edge {control:.4})");
    assert!((wired - CAMERA_RAW_EDGE).abs() < 0.005 + 1.0 / (2.0 * (r * w as f32) as f64), "edge {wired:.4} vs Camera Raw {CAMERA_RAW_EDGE}");
}

/// D2 continuation of the wired-centroid test above: when the Lightroom
/// transport is non-identity, the same exact-once engine cancellation must
/// land on `m_lr(stored)`, not on the stored point and not on a second copy
/// of either lens field.
#[test]
#[allow(clippy::excessive_precision)] // exact decoded 0x7037 fixture decimals
fn d2_wired_camera_mask_lands_at_the_lr_transported_coordinate() {
    const WALL_NATIVE: [f32; 16] = [
        1.0007934570,
        0.9998779297,
        0.9981079102,
        0.9959716797,
        0.9927368164,
        0.9890136719,
        0.9846191406,
        0.9800415039,
        0.9749145508,
        0.9696044922,
        0.9641113281,
        0.9589843750,
        0.9538574219,
        0.9492187500,
        0.9448242188,
        0.9412231445,
    ];
    let (w, h) = (1920u32, 1280u32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    let mut profile = d2_camera_profile(&WALL_NATIVE);
    // The render-side adjudication retained this established 16-knot
    // calibration; only the mask solve uses the corrected dense spline.
    profile.distortion = WALL_NATIVE.to_vec();
    profile.distortion_on = true;

    for cx in [0.10f32, 0.32f32] {
        let cy = 0.5f32;
        let recipe = EditRecipe {
            masks: vec![probe_radial(cx, cy, 0.025)],
            lens_profile: profile.clone(),
            ..Default::default()
        };
        let target = lr_mask_warp_norm(cx, cy, (w as f32, h as f32), &profile);
        let want = (target.0 as f64 * w as f64, target.1 as f64 * h as f64);
        let got = effect_centroid(&apply_lens_geometry(
            &develop_preview(&base, &recipe),
            &profile,
            0.0,
        ));
        let err = (got.0 - want.0).hypot(got.1 - want.1);
        let transport = (want.0 - cx as f64 * w as f64)
            .hypot(want.1 - cy as f64 * h as f64);
        eprintln!(
            "D2 wired centroid cx={cx:.2}: error={err:.3}px, Lightroom transport={transport:.3}px"
        );
        assert!(transport > 1.0, "premise: D2 transport is only {transport:.3}px");
        assert!(err < 1.0, "cx={cx}: centroid {got:?}, target {want:?}, error {err:.3}px");
    }
}

/// The COMPANION pin: with the geometry stage inactive, the mask chain is
/// unchanged — bit for bit, not approximately.
///
/// This is the other half of the "mask map and pixel warp travel together"
/// invariant, and the half that protects every photograph that has no lens
/// profile and no manual distortion (every non-Sony frame, and every Sony
/// frame whose photographer switched the correction off). The wiring must
/// cost them nothing at all.
///
/// MUTATION THIS KILLS: making `MaskFrame::downstream` return
/// `WarpedDownstream` unconditionally, or dropping `MaskUnwarp::new`'s
/// identity check.
#[test]
fn with_the_geometry_stage_inactive_the_mask_chain_is_untouched() {
    let (w, h) = (240u32, 160u32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    let masks = vec![probe_radial(0.24, 0.5, 0.1)];
    // (a) No profile at all, and a profile whose data is present but
    //     TOGGLED OFF — both are inert, and the second is the one a naive
    //     `!profile.distortion.is_empty()` gate would get wrong.
    let none = EditRecipe { masks: masks.clone(), ..Default::default() };
    let toggled_off = EditRecipe {
        lens_profile: crate::recipe::LensProfile {
            distortion_on: false,
            ..barrel_profile()
        },
        ..none.clone()
    };
    let a = develop_preview(&base, &none);
    let b = develop_preview(&base, &toggled_off);
    assert_eq!(a.to_rgb8().as_raw(), b.to_rgb8().as_raw(), "an inert profile moved a mask");
    // …and both equal the explicitly-unadapted chain, which is what
    // "unchanged from before this batch" means.
    let c = develop_preview_framed(&base, &none, &crate::diag::pixels(), MaskFrame::AsRendered, None, false);
    assert_eq!(a.to_rgb8().as_raw(), c.to_rgb8().as_raw());

    // (a2) The case the identity short-circuit in `MaskUnwarp::new` exists
    //      for, and the one `downstream` cannot catch: a profile that IS
    //      active by the gate — sixteen knots, toggle on — whose map is
    //      nevertheless the identity, because the lens has no distortion to
    //      correct. `geometry_active()` is true, so the frame says
    //      `WarpedDownstream`; the map must then recognise itself as inert
    //      and return `None` rather than push every mask coordinate through
    //      a float round trip.
    let flat = EditRecipe {
        lens_profile: crate::recipe::LensProfile {
            distortion: vec![1.0; 16],
            distortion_on: true,
            ..Default::default()
        },
        ..none.clone()
    };
    assert!(
        MaskFrame::downstream(&flat.lens_profile, 0.0).warps(),
        "premise: a flat profile is still ACTIVE by the gate"
    );
    assert!(
        MaskUnwarp::new(&flat.lens_profile, 0.0, (w as f32, h as f32)).is_none(),
        "an identity map must short-circuit, not round-trip every coordinate"
    );
    let d = develop_preview(&base, &flat);
    assert_eq!(
        a.to_rgb8().as_raw(),
        d.to_rgb8().as_raw(),
        "a distortion-free lens profile moved a mask"
    );

    // (b) The decision itself, at the source: an inert profile answers
    //     `AsRendered`, so the geometry stage is not run either.
    let inert = crate::recipe::LensProfile::default();
    assert!(!MaskFrame::downstream(&inert, 0.0).warps());
    assert!(MaskFrame::downstream(&inert, 0.0).unwarp((240.0, 160.0)).is_none());
    assert!(!MaskFrame::downstream(&toggled_off.lens_profile, 0.0).warps());
    // …and an ACTIVE one answers the other way, in both of the two ways it
    // can be active (profile knots, and the manual amount alone).
    assert!(MaskFrame::downstream(&barrel_profile(), 0.0).warps());
    assert!(MaskFrame::downstream(&inert, 25.0).warps(), "the manual amount alone warps");
    assert!(
        MaskFrame::downstream(&inert, 25.0).unwarp((240.0, 160.0)).is_some(),
        "the manual lens_distortion must be covered, not half-covered"
    );
}

/// R38: the fit's rulers judge `develop_preview`'s pixels, so the contour
/// they build must be the one that develop evaluates — the clamped
/// recipe's composed profile and manual amount, exactly as
/// `develop_preview_inner` decides them. Under a barrel profile a linear
/// edge is painted where the geometry stage carries its stored handles'
/// content (v1.17.0, Camera Raw), and the stored-frame contour leaves it on
/// the rows away from the handles, where the corrected-frame line and the
/// carried content part; with no geometry to follow the two frames are one.
#[test]
fn preview_mask_coverage_is_the_frame_the_preview_develops_in() {
    let (w, h) = (480u32, 320u32);
    let base = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([120, 120, 120])));
    // The barrel of `a_parametric_mask_lands_on_its_stored_coordinates_under_lens_geometry`
    // (corner factor 0.75, fill scale 0.923): its composite map magnifies
    // the frame's middle by 8 %, so a hard edge 72 px left of centre paints
    // 2–5 px away from its stored column, farthest on the centre row.
    // `barrel_profile()` moves it under a pixel at this size.
    let profile = crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 - 0.25 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        ..Default::default()
    };
    let edge = crate::recipe::LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.35 + 0.5 / w as f32, zero_y: 0.5, full_x: 0.35, full_y: 0.5 },
        exposure_ev: 0.6,
        ..Default::default()
    };
    let recipe = EditRecipe { lens_profile: profile.clone(), masks: vec![edge.clone()], ..Default::default() };
    let validated = crate::recipe::ValidatedRecipe::new(&recipe);
    let expected = mask_coverage(
        &edge,
        &base,
        MaskFrame::downstream(&geometry_profile(&validated), validated.lens_distortion),
    );
    let got = preview_mask_coverage(&edge, &base, &recipe);
    assert_eq!(got.as_raw(), expected.as_raw(), "the preview's own frame, from the recipe");
    let stored = mask_coverage(&edge, &base, MaskFrame::AsRendered);
    let rendered = develop_preview(&base, &recipe).to_rgb8();
    let plain = develop_preview(&base, &EditRecipe { lens_profile: profile, ..Default::default() }).to_rgb8();
    let lift = |x: u32, y: u32| rendered.get_pixel(x, y)[1] as i32 - plain.get_pixel(x, y)[1] as i32;
    let full = lift(8, 160);
    assert!(full > 20, "premise: the edge lifts its side by {full} codes");
    // `develop_preview` paints in the pre-geometry (sensor) frame. The edge's
    // stored handles are carried by the geometry stage's map to a vertical
    // line in the corrected frame; on row y the edge is the sensor column the
    // same map puts on that line — the stored column on the handles' own row.
    let (dims, lens) = ((w as f32, h as f32), geometry_profile(&validated));
    let to_corrected = |x: f32, y: f32| lens_ungeom_norm(x / dims.0, y / dims.1, dims, &lens, validated.lens_distortion).0;
    let line = to_corrected((0.35 + 0.25 / w as f32) * dims.0, 0.5 * dims.1);
    let mut parted = 0;
    for y in [60u32, 160, 260] {
        let painted = (0..w).rev().find(|&x| lift(x, y) * 2 > full).expect("a lifted pixel");
        let edge = (0..w).rev().find(|&x| to_corrected(x as f32 + 0.5, y as f32 + 0.5) <= line).expect("edge column");
        assert!(
            edge.abs_diff(painted) <= 1,
            "row {y}: the edge ({painted}) is not the column carried onto the handles' line ({edge})"
        );
        assert!(
            contour_column(&got, y).abs_diff(painted) <= 1,
            "row {y}: the preview-frame contour ({}) is where the develop paints the edge ({painted})",
            contour_column(&got, y)
        );
        if contour_column(&stored, y).abs_diff(painted) >= 2 {
            parted += 1;
        }
    }
    assert!(parted > 0, "premise: on some row the stored-frame contour is not where the develop paints");
    let flat = EditRecipe { masks: vec![edge.clone()], ..Default::default() };
    assert_eq!(preview_mask_coverage(&edge, &base, &flat).as_raw(), stored.as_raw(), "no geometry: one frame");
}

/// The last column of row `y` that a coverage raster calls covered.
fn contour_column(cov: &image::GrayImage, y: u32) -> u32 {
    (0..cov.width()).rev().find(|&x| cov.get_pixel(x, y)[0] >= 128).expect("coverage")
}

/// The GUI's red coverage wash must advertise exactly what the render
/// applies — including the frame adaptation.
///
/// The overlay is built by `mask_coverage` and then warped by the caller
/// (`bin/gui/canvas.rs`) so it follows the rendered pixels. Both halves take
/// the SAME [`MaskFrame`]; if the coverage build skipped the adaptation
/// while the render performed it, the wash would sit a whole field away
/// from the effect it claims to show — up to 186 px at 24 mm.
///
/// Asserted as agreement between the two, not against a hand-computed
/// expectation: the property is that the overlay and the render say the
/// same thing, whatever that thing is.
///
/// MUTATION THIS KILLS: dropping the `unwarp` from `mask_coverage`.
#[test]
fn the_gui_coverage_overlay_matches_what_the_render_applies() {
    let (w, h) = (480u32, 320u32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255])));
    let profile = crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 - 0.25 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        ..Default::default()
    };
    let adj = probe_radial(0.20, 0.5, 0.08);
    let recipe =
        EditRecipe { masks: vec![adj.clone()], lens_profile: profile.clone(), ..Default::default() };
    let frame = MaskFrame::downstream(&profile, 0.0);
    assert!(frame.warps(), "premise: this fixture's geometry is active");

    // The overlay, exactly as the GUI builds it: coverage then the same warp.
    let cov = DynamicImage::ImageLuma8(mask_coverage(&adj, &base, frame));
    let cov = apply_lens_geometry(&cov, &profile, 0.0).to_luma8();

    // The render, exactly as a preview surface builds it.
    let rendered = apply_lens_geometry(&develop_preview(&base, &recipe), &profile, 0.0).to_rgb8();

    // Where the overlay claims full coverage, the render must have applied
    // the effect (-4 EV on white); where it claims none, it must not have.
    let (mut claimed, mut agreed, mut clear, mut clean) = (0u32, 0u32, 0u32, 0u32);
    for (x, y, p) in cov.enumerate_pixels() {
        let lit = rendered.get_pixel(x, y).0[1];
        if p.0[0] > 200 {
            claimed += 1;
            if lit < 160 {
                agreed += 1;
            }
        } else if p.0[0] < 20 {
            clear += 1;
            if lit > 200 {
                clean += 1;
            }
        }
    }
    assert!(claimed > 200 && clear > 200, "premise: {claimed} covered / {clear} clear px");
    assert!(
        agreed * 100 >= claimed * 97,
        "the overlay claims coverage the render does not apply: {agreed}/{claimed}"
    );
    assert!(
        clean * 100 >= clear * 97,
        "the render applies an effect the overlay does not show: {clean}/{clear}"
    );
}

/// The per-TYPE split, at the predicate that owns it: only the two shapes
/// Lightroom stores post-correction are adapted.
///
/// A brush would be moved TWICE if it were included here (Lightroom
/// rasterises it pre-correction and so does this engine); an engine-authored
/// raster has no Lightroom rendering to agree with; a range mask selects by
/// pixel value and is frame-invariant, so it is not a geometry at all.
#[test]
fn only_lightrooms_post_correction_shapes_are_frame_adapted() {
    assert!(is_lr_post_correction_geometry(&MaskGeometry::Linear {
        zero_x: 0.1,
        zero_y: 0.2,
        full_x: 0.8,
        full_y: 0.9
    }));
    assert!(is_lr_post_correction_geometry(&probe_radial(0.3, 0.3, 0.1).mask));
    assert!(!is_lr_post_correction_geometry(&MaskGeometry::Brush {
        name: "Brush 1".into(),
        blend_mode: 0,
        value: 1.0,
        inverted: false,
        strokes: Vec::new(),
    }));
    assert!(!is_lr_post_correction_geometry(&MaskGeometry::Bitmap { path: String::new() }));
    assert!(!is_lr_post_correction_geometry(&MaskGeometry::AiMask {
        name: String::new(),
        subtype: 0,
        ref_x: 0.5,
        ref_y: 0.5,
        blend_mode: 0,
        value: 1.0,
        inverted: false,
        mask_version: 0,
        gesture: Vec::new(),
        provenance: Vec::new(),
        raster: None,
    }));
    // And the adapter honours it: with the SAME unwarp in hand, a brush
    // and a bitmap are asked at the un-adapted point.
    let u = MaskUnwarp::new(&barrel_profile(), 0.0, (480.0, 320.0)).expect("active");
    let moved = u.engine_at(0.2, 0.5);
    assert!((moved.0 - 0.2).abs() > 1e-4, "premise: the map really moves this point");
    // A brush that actually PAINTS at the sample point (R29 Batch-6b). With
    // the empty stroke list this test used to carry, both sides of the
    // equality were the inert 0 and it held for the wrong reason. The
    // sample sits on the RIM of a hard dab (h = 1, so the falloff is a
    // near-step) — the one place a fraction of a per-cent of frame width
    // changes the weight by more than the 8-bit raster quantum.
    let brush = probe_brush(&[(1.0, 0.05, 1.0, 1.0, "d 0.2 0.5")]);
    let braster = brush_raster(&brush, 480, 320).expect("one dab");
    let rim = (0.2 + 0.05 * 0.94, 0.5);
    let rim_moved = u.engine_at(rim.0, rim.1);
    assert!(
        mask_weight(&brush, rim.0, rim.1, Some(&braster)) > 0.2,
        "premise: the brush really paints at the rim sample"
    );
    assert_eq!(
        mask_weight_in(&brush, rim.0, rim.1, Some(Bmp::sensor(&braster)), Some(&u), (480.0, 320.0)),
        mask_weight(&brush, rim.0, rim.1, Some(&braster)),
        "a brush must not be frame-adapted"
    );
    assert!(
        (mask_weight_in(&brush, rim.0, rim.1, Some(Bmp::sensor(&braster)), Some(&u), (480.0, 320.0))
            - mask_weight(&brush, rim_moved.0, rim_moved.1, Some(&braster)))
        .abs()
            > 1e-3,
        "…and the adapted point is a DIFFERENT weight, so the equality is not vacuous"
    );
    let rad = probe_radial(0.24, 0.5, 0.1).mask;
    assert_eq!(
        mask_weight_in(&rad, 0.2, 0.5, None, Some(&u), (480.0, 320.0)),
        mask_weight(&rad, moved.0, moved.1, None),
        "a radial must be asked at the adapted point"
    );
}

/// The frame table in the mask-warp block header, asserted rather than
/// asserted-in-prose: this engine evaluates masks BEFORE the geometry
/// stage, so a mask it draws is carried by the distortion field exactly as
/// the pixels are.
///
/// That is what makes an extra warp on the brush arm a DOUBLE application
/// (Lightroom rasterises brush dabs pre-correction too), and what makes the
/// radial arm — whose coordinates Lightroom stores POST-correction — a
/// mismatch this batch exposed rather than created.
///
/// Measured here rather than cited, in the two halves that make an order:
/// the mask stage's OUTPUT does not depend on the lens profile at all (it
/// runs first and cannot see it), and running the geometry stage over that
/// output MOVES the mask's footprint (so it runs second, over pixels the
/// mask is already baked into). That composition — `apply_develop` then
/// `apply_lens_geometry` — is exactly the one `render_to_file` performs.
#[test]
fn the_engine_evaluates_masks_before_the_geometry_stage() {
    // 480 px wide, not 240: the displacement this measures is a FRACTION of
    // the frame (~0.6 % of the width for this profile), so a small fixture
    // frame puts the whole effect inside a pixel and the test would pass on
    // rounding rather than on the property.
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(480, 320, image::Rgb([128, 128, 128])));
    let mask = crate::recipe::LocalAdjustment {
        mask: MaskGeometry::Radial {
            top: 0.30,
            left: 0.05,
            bottom: 0.70,
            right: 0.28,
            feather: 0.0,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 0,
        },
        exposure_ev: -4.0,
        ..Default::default()
    };
    // A strong barrel profile, shaped like a real one (falling factors).
    let profile = crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 - 0.12 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        ..Default::default()
    };
    let off = EditRecipe { masks: vec![mask.clone()], ..Default::default() };
    let on = EditRecipe { lens_profile: profile.clone(), ..off.clone() };
    let dark_centroid = |img: &DynamicImage| -> (f64, f64) {
        let g = img.to_rgb8();
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for (x, y, p) in g.enumerate_pixels() {
            if p.0[1] < 90 {
                sx += x as f64;
                sy += y as f64;
                n += 1.0;
            }
        }
        assert!(n > 50.0, "the mask must darken a real region, got {n} px");
        (sx / n, sy / n)
    };
    // HALF ONE: the mask stage runs FIRST — it rasterises into the
    // PRE-geometry buffer and nothing it does moves a pixel. Asked with
    // `AsRendered` (no geometry downstream) it renders the same pixels
    // whether or not a profile is present, because the profile is not a
    // tonal control and the frame adaptation is switched off.
    //
    // Deliberately NOT `develop_preview` here any more: since R29 Batch-3
    // that entry point DOES read the profile — to adapt a parametric mask's
    // frame for the resample it expects to follow (`MaskFrame`). That is
    // the fix, not a counter-example to the ordering, and asking with the
    // frame pinned is how the ordering stays measurable.
    let anon = crate::diag::pixels();
    let masked_off = develop_preview_framed(&base, &off, &anon, MaskFrame::AsRendered, None, false);
    let masked_on = develop_preview_framed(&base, &on, &anon, MaskFrame::AsRendered, None, false);
    assert_eq!(
        masked_off.to_rgb8().as_raw(),
        masked_on.to_rgb8().as_raw(),
        "the mask stage moved pixels by itself - it is not a pure rasteriser"
    );
    // HALF TWO: the geometry stage runs SECOND, over pixels the mask is
    // already baked into, so it CARRIES the mask exactly as it carries the
    // photograph. If masks were evaluated after geometry this would be
    // zero, and the brush arm would need the warp the block header
    // describes instead of already having it.
    let before = dark_centroid(&masked_off);
    let after = dark_centroid(&apply_lens_geometry(&masked_off, &profile, 0.0));
    assert!(
        (before.0 - after.0).abs() > 1.5,
        "the geometry stage did not carry the mask: centroids {before:?} vs {after:?}"
    );
}

#[test]
fn apply_lens_distortion_fills_the_frame_and_moves_content_radially() {
    // (a) 0 is the identity (pixels untouched); dims always preserved.
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(121, 81, image::Rgb([9, 200, 30])));
    assert_eq!(apply_lens_distortion(&img, 0.0).to_rgb8().as_raw(), img.to_rgb8().as_raw());
    let out = apply_lens_distortion(&img, 70.0);
    assert_eq!((out.width(), out.height()), (121, 81));

    // (b) No un-sampled (black) pixels for EITHER sign: k ≤ 0 fills by
    // construction, k > 0 relies on the Newton fill scale.
    let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(121, 81, image::Rgb([255, 255, 255])));
    for amt in [100.0f32, -100.0, 55.0, -55.0] {
        let r = apply_lens_distortion(&white, amt).to_rgb16();
        let min = r.pixels().flat_map(|p| p.0).min().unwrap();
        assert!(min >= 65000, "unfilled pixels at amount {amt}: min {min}");
    }

    // (c) The exact centre is a fixed point of the resample.
    let mut cdot = RgbImage::from_pixel(121, 81, image::Rgb([0, 0, 0]));
    cdot.put_pixel(60, 40, image::Rgb([255, 255, 255]));
    for amt in [100.0f32, -100.0] {
        let m = apply_lens_distortion(&DynamicImage::ImageRgb8(cdot.clone()), amt).to_rgb16();
        assert!(m.get_pixel(60, 40)[0] > 30000, "centre must be a fixed point at {amt}");
    }

    // (d) A +100 barrel fix (fill scale = 1) pushes content OUTWARD: a
    // white 3×3 dot centred at x=30 on the horizontal centreline (frame
    // centre x=60) must land further LEFT (predicted ≈ x 28.6).
    let mut dot = RgbImage::from_pixel(121, 81, image::Rgb([0, 0, 0]));
    for yy in 39..=41 {
        for xx in 29..=31 {
            dot.put_pixel(xx, yy, image::Rgb([255, 255, 255]));
        }
    }
    let moved = apply_lens_distortion(&DynamicImage::ImageRgb8(dot), 100.0).to_rgb16();
    let bright_x = (0..121u32).max_by_key(|&x| moved.get_pixel(x, 40)[0]).unwrap();
    assert!(
        moved.get_pixel(bright_x, 40)[0] > 30000 && bright_x <= 29,
        "barrel fix must move the dot outward (x<30), got x={bright_x}"
    );
}

/// The profile Vignetting strength against Camera Raw (2026-10-28): flat-field
/// frames rendered at each strength and read against their own raw-gain
/// ladders, so each number is the EV of the gain Camera Raw applied at that
/// radius. Rows are (EV at 100, then at 25, 50, 150 and 200); 14–24 mm at
/// 20 mm, radius 0.26, 0.51 and 0.71 of the half-diagonal, then 24–105 mm at
/// 80 mm, 0.51 and 0.91 (no 25 rendered there). The old reading,
/// `1 + (g − 1)·s`, misses the 14–24 mm's 0.71 row at 50 by 0.039 EV.
#[test]
fn the_vignetting_strength_is_camera_raws_on_two_flat_fields() {
    const MEASURED: [(f32, [Option<f32>; 4]); 5] = [
        (0.0992, [Some(0.0244), Some(0.0490), Some(0.1461), Some(0.1931)]),
        (0.3671, [Some(0.0904), Some(0.1821), Some(0.5126), Some(0.6596)]),
        (0.6624, [Some(0.1640), Some(0.3304), Some(0.8891), Some(1.1160)]),
        (0.1317, [None, Some(0.0643), Some(0.1909), Some(0.2519)]),
        (0.3562, [None, Some(0.1780), Some(0.4994), Some(0.6426)]),
    ];
    for (ev100, at) in MEASURED {
        let knots = vec![ev100.exp2(); 4];
        for (scale, want) in [25.0f32, 50.0, 150.0, 200.0].into_iter().zip(at) {
            let Some(want) = want else { continue };
            let got = profile_vignette_lut(&knots, scale)[0].log2();
            assert!((got - want).abs() < 0.004, "EV {ev100} at {scale}: {got:.4} against Camera Raw's {want}");
        }
    }
}
