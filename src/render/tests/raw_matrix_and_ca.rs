// One part of the engine's tests (src/render/tests.rs includes it): raw colour matrix refusals, chromatic aberration and the tier that renders nothing.

/// A RAW with no colour matrix is DISCLOSED, not developed without its
/// profile in silence: `resolve_camera_profile` forwards this error
/// verbatim into its `camera profile "…" is not rendered: …` warning, so
/// the text has to name the missing thing. (That arm sits behind
/// `dcp::find`, which needs an installed profile pool; the error it
/// forwards is what can be pinned without one.)
///
/// MUTATION: answer an identity for an empty map, or reword the error.
#[test]
fn a_raw_without_a_colour_matrix_names_the_missing_matrix() {
    use rawler::cfa::{PlaneColor, CFA};
    use rawler::decoders::Camera;
    use rawler::rawimage::{BlackLevel, CFAConfig, RawImageData, RawPhotometricInterpretation, WhiteLevel};
    let raw = rawler::RawImage::new_with_data(
        Camera::new(),
        RawImageData::Integer(vec![512u16; 16]),
        4,
        4,
        1,
        [1.0, 1.0, 1.0, f32::NAN],
        RawPhotometricInterpretation::Cfa(CFAConfig::new(&CFA::new("RGGB"), &PlaneColor::new("RGB"))),
        Some(BlackLevel::new(&[0u16], 1, 1, 1)),
        Some(WhiteLevel::new(vec![4095])),
        false,
    );
    assert!(raw.color_matrix.is_empty(), "premise: a body rawler knows nothing about");
    let e = camera_matrix(&raw).unwrap_err().to_string();
    assert!(e.contains("no camera colour matrix"), "the refusal names the mechanism: {e}");
}

/// L04-1: a file-supplied camera matrix that cannot be inverted is
/// REFUSED with the measured determinant — never rendered into a
/// silently-black frame — while a real, well-conditioned matrix passes.
#[test]
fn singular_camera_matrix_is_refused_not_rendered_black() {
    let good = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
    assert!(
        validate_calibration(
            &good,
            [1.9, 1.0, 1.6, f32::NAN],
            ExportColorSpace::Srgb,
            Path::new("x.dng"),
        )
        .is_ok(),
        "a real matrix + real WB validates"
    );
    let mut twin = good;
    twin[1] = twin[0]; // two identical rows ⇒ det == 0 after row-norm
    let e = validate_calibration(&twin, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("determinant"), "the refusal quotes the measurement: {e}");
    assert!(e.contains("x.dng"), "the refusal names the file: {e}");
    let mut nan = good;
    nan[2][1] = f32::NAN;
    let e = validate_calibration(&nan, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("non-finite"), "{e}");
}

/// L04-1: a matrix row summing to zero used to be SILENTLY left
/// un-normalised by camera_to_space_matrix (the DNG white-preservation
/// rule quietly dropped) — the validator refuses it instead.
#[test]
fn degenerate_matrix_row_is_disclosed_not_silently_unnormalised() {
    let mut degen = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
    degen[0] = [0.5, -1.0, 0.5]; // sums to 0
    let e = validate_calibration(&degen, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("degenerate row"), "{e}");
    // Codex AL-review F3: a row can be healthy RAW but near-orthogonal
    // to the white-weighted PRODUCT the render actually inverts - the
    // validator must judge that product, not the raw rows alone.
    let m = rgb_to_xyz(SRGB_PRIM, D65_XY);
    let col_sums = [
        m[0][0] + m[0][1] + m[0][2],
        m[1][0] + m[1][1] + m[1][2],
        m[2][0] + m[2][1] + m[2][2],
    ];
    let mut ortho = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
    // row = [1, t, 0] with 1*col0 + t*col1 = 0  =>  raw sum 1+t != 0.
    let t = -col_sums[0] / col_sums[1];
    ortho[0] = [1.0, t, 0.0];
    let e = validate_calibration(&ortho, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("degenerate row") || e.contains("determinant"),
        "a white-orthogonal row must refuse even with a healthy raw sum: {e}"
    );
}

/// L04-1: rawler turns a zero AsShotNeutral component into an INFINITE
/// wb coefficient (dng.rs builds 1/levels), which the old
/// `wb[0].is_nan()`-only guard let straight into the pixel chain — as
/// did a partial NaN and a negative. All refuse now; the documented
/// "unknown" convention (wb[0] NaN ⇒ neutral) still validates.
#[test]
fn zero_as_shot_neutral_becomes_an_infinite_coefficient_and_is_refused() {
    let good = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
    for bad in [
        [f32::INFINITY, 1.0, 1.0, f32::NAN], // the DNG 1/0 case
        [1.0, f32::NAN, 1.0, f32::NAN],      // partial NaN — invisible to a [0]-only check
        [1.0, -0.5, 1.0, f32::NAN],
        [1.0, 0.0, 1.0, f32::NAN],
    ] {
        let e = validate_calibration(&good, bad, ExportColorSpace::Srgb, Path::new("x.dng"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("AsShotNeutral"), "{bad:?} must refuse: {e}");
    }
    // A REAL (non-NaN) fourth coefficient is validated too (Codex F4):
    // rawler consumes it on 4-colour sensors.
    let e = validate_calibration(&good, [1.0, 1.0, 1.0, f32::INFINITY], ExportColorSpace::Srgb, Path::new("x.dng"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("fourth WB coefficient"), "{e}");
    assert!(validate_calibration(&good, [1.0, 1.0, 1.0, 1.2], ExportColorSpace::Srgb, Path::new("x.dng")).is_ok());
    assert_eq!(
        normalise_wb([f32::NAN, 9.0, 9.0, 9.0]),
        [1.0, 1.0, 1.0],
        "wb[0] NaN is rawler's documented UNKNOWN — neutral, not corrupt"
    );
    assert!(validate_calibration(&good, [f32::NAN; 4], ExportColorSpace::Srgb, Path::new("x")).is_ok());
    assert_eq!(
        normalise_wb([f32::INFINITY, 1.0, 1.0, f32::NAN]),
        [f32::INFINITY, 1.0, 1.0],
        "an infinite coefficient is NOT collapsed to unknown — it must reach the refusal"
    );
}

/// L04-1: WHY the refusal exists — the packer saturates silently. NaN
/// quantises to black, inf to white, and render_to_file would return Ok.
/// Pinned so a future refactor cannot re-open the silent path.
#[test]
fn nan_calibration_never_reaches_the_packer() {
    assert_eq!(to_u16(f32::NAN), 0, "NaN clamps to black");
    assert_eq!(to_u16(f32::INFINITY), 65535, "inf saturates to white");
    assert_eq!(to_u16(f32::NEG_INFINITY), 0);
}

/// L04-2 (v1.17.0): CA moves each channel by its own knots and nothing
/// else — Camera Raw's rule, measured on p42, where its render keeps green
/// and red in place to 0.03 px and moves only blue. Until v1.16.0 a knot
/// above 1 zoomed all three channels by one composite fill scale. Here a
/// CA-only profile with red past 1 leaves green byte for byte where it
/// was, red samples at its own magnified radius, and a red sample past the
/// edge reads the clamped edge pixel, so a white frame stays white.
///
/// MUTATION: divide the channel LUTs by a composite overshoot scale again.
#[test]
fn ca_moves_each_channel_by_its_own_knots_only() {
    use crate::recipe::LensProfile;
    let profile = LensProfile {
        ca_r: vec![1.02; 16],
        ca_b: vec![0.98; 16],
        ca_on: true,
        ..Default::default()
    };
    let ramp = ImageBuffer::from_fn(200, 100, |x, y| {
        Rgb([(x as u16) * 300, (x as u16) * 200 + (y as u16) * 7, (y as u16) * 500])
    });
    let out = apply_lens_geometry(&DynamicImage::ImageRgb16(ramp.clone()), &profile, 0.0).to_rgb16();
    assert!(
        out.pixels().zip(ramp.pixels()).all(|(p, q)| p.0[1] == q.0[1]),
        "green must not move under a CA-only profile, whatever the red knots ask"
    );
    // Red at a named pixel on the centre row (dy = 0.5 below the centre
    // line on an even height, which the vertical ramp-free red ignores):
    // the source's own lerp at the radius scaled by the red knot.
    let (cx, x) = (99.5f32, 180u32);
    let sx = cx + (x as f32 - cx) * 1.02;
    let i0 = sx.floor() as u32;
    let frac = sx - sx.floor();
    let want = ramp.get_pixel(i0, 50).0[0] as f32 * (1.0 - frac) + ramp.get_pixel(i0 + 1, 50).0[0] as f32 * frac;
    let got = out.get_pixel(x, 50).0[0] as f32;
    assert!((got - want).abs() <= 1.0, "red at x={x}: got {got}, want the source at {sx} ({want})");
    // Past the edge red reads the clamped edge pixel: no unfilled pixels.
    let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(201, 101, image::Rgb([255; 3])));
    let w = apply_lens_geometry(&white, &profile, 0.0).to_rgb16();
    let min = w.pixels().flat_map(|p| p.0).min().unwrap();
    assert!(min >= 65000, "unfilled pixels at the edge: min {min}");
}

/// L04-2 (v1.17.0): with the profile distortion on, a CA channel past 1
/// still leaves green byte for byte what the CA-off render draws — the
/// shared frame is the distortion's alone.
#[test]
fn ca_never_moves_green_even_when_a_channel_overshoots() {
    use crate::recipe::LensProfile;
    let dist: Vec<f32> =
        (0..16).map(|i| 1.0008 - 0.02 * (i as f32 / 15.0).powi(2)).collect();
    let with_ca = LensProfile {
        distortion: dist.clone(),
        distortion_on: true,
        ca_r: vec![1.004; 16],
        ca_b: vec![0.999; 16],
        ca_on: true,
        ..Default::default()
    };
    let no_ca =
        LensProfile { distortion: dist, distortion_on: true, ..Default::default() };
    assert!(
        !geometry_moves_frame(
            &LensProfile { ca_r: vec![1.02; 16], ca_b: vec![1.02; 16], ca_on: true, ..Default::default() },
            0.0
        ),
        "CA alone never moves the shared frame"
    );
    let ramp = DynamicImage::ImageRgb16(ImageBuffer::from_fn(160, 90, |x, y| {
        Rgb([(x as u16) * 300, (x as u16) * 300 + (y as u16), (y as u16) * 500])
    }));
    let a = apply_lens_geometry(&ramp, &with_ca, 0.0).to_rgb16();
    let b = apply_lens_geometry(&ramp, &no_ca, 0.0).to_rgb16();
    assert!(
        a.pixels().zip(b.pixels()).all(|(p, q)| p.0[1] == q.0[1]),
        "green must be byte-identical with and without an overshooting CA"
    );
    assert!(
        a.pixels().zip(b.pixels()).any(|(p, q)| p.0[0] != q.0[0]),
        "premise: the red knot really moved red"
    );
}

/// R25 B3: the manual CA pair really moves the red channel's sampling
/// radius — and moves NOTHING else when it asks for a shrink.
///
/// Both directions leave green and blue byte for byte identical to the
/// CA-off render, red the only thing that moved (L04-2 as of v1.17.0: CA
/// moves each channel by its own knots; until v1.16.0 a magnifying value
/// zoomed the whole frame through a composite fill scale).
#[test]
fn manual_ca_shifts_the_red_channel_radius() {
    // Concentric rings: a pattern that is a function of RADIUS, so a
    // radial rescale of one channel is visible and a translation-only bug
    // could not fake it. All three channels identical at the source.
    let target = DynamicImage::ImageRgb16(ImageBuffer::from_fn(161, 161, |x, y| {
        let (dx, dy) = (x as f32 - 80.0, y as f32 - 80.0);
        let r = (dx * dx + dy * dy).sqrt();
        let v = (((r * 0.7).sin() * 0.5 + 0.5) * 60000.0) as u16;
        Rgb([v; 3])
    }));
    let off = EditRecipe::default();
    let shrink = EditRecipe { ca_r: -100.0, ..Default::default() };
    let grow = EditRecipe { ca_r: 100.0, ..Default::default() };

    // The composed profile is what carries it — a photo with no in-camera
    // CA data of its own still gets one.
    assert!(!geometry_profile(&off).geometry_active(), "premise: a rest recipe adds nothing");
    assert!(geometry_profile(&shrink).geometry_active(), "the manual pair alone is geometry");

    let base = apply_lens_geometry(&target, &geometry_profile(&off), 0.0).to_rgb16();
    let out = apply_lens_geometry(&target, &geometry_profile(&shrink), 0.0).to_rgb16();
    assert!(
        out.pixels().zip(base.pixels()).all(|(p, q)| p.0[1] == q.0[1] && p.0[2] == q.0[2]),
        "a shrinking manual CA must leave green and blue byte-identical"
    );
    assert!(
        out.pixels().zip(base.pixels()).any(|(p, q)| p.0[0] != q.0[0]),
        "…and it must actually move red"
    );
    // The DIRECTION, at one named pixel: ca_r < 0 means red samples at a
    // SMALLER radius, so the far-off-centre red is the source from nearer
    // the middle. SUB-PIXEL by construction — ±100 is 0.2 % of the radius
    // and no test frame makes that a whole pixel — so the expectation is
    // the source's own bilinear value at the shrunk coordinate, which on
    // the centre row (dy = 0) is a plain lerp along x.
    let src = target.to_rgb16();
    let f = 1.0 + (-100.0) * MANUAL_CA_PER_UNIT;
    let (x, y) = (158u32, 80u32);
    let sx = 80.0 + ((x as f32) - 80.0) * f;
    let (i0, frac) = (sx.floor() as u32, sx - sx.floor());
    let want = src.get_pixel(i0, y).0[0] as f32 * (1.0 - frac)
        + src.get_pixel(i0 + 1, y).0[0] as f32 * frac;
    let got = out.get_pixel(x, y).0[0] as f32;
    assert!(
        (got - want).abs() < 64.0,
        "red at x={x} should be the source at the shrunk radius {sx}: got {got}, want {want}"
    );
    // …and that really is a MOVE, not a rounding: the unshifted source
    // pixel is far away on this ring pattern.
    let unshifted = src.get_pixel(x, y).0[0] as f32;
    assert!(
        (got - unshifted).abs() > 1000.0,
        "the probe pixel must sit on a steep part of the ring pattern"
    );

    // The magnifying direction moves red alone too, and neither moves the
    // shared frame the coordinate maps follow (C2).
    let grown = apply_lens_geometry(&target, &geometry_profile(&grow), 0.0).to_rgb16();
    assert!(
        grown.pixels().zip(base.pixels()).all(|(p, q)| p.0[1] == q.0[1] && p.0[2] == q.0[2]),
        "a magnifying manual CA must leave green and blue byte-identical"
    );
    assert!(
        !geometry_moves_frame(&geometry_profile(&grow), 0.0)
            && !geometry_moves_frame(&geometry_profile(&shrink), 0.0),
        "manual CA in either direction does not move the shared frame"
    );
}

/// R25 B3: the pair at rest costs NOTHING — no allocation, no engine
/// change, and a render byte for byte what v0.30 produced.
#[test]
fn ca_zero_is_bit_identical_to_no_ca() {
    use crate::recipe::LensProfile;
    let profile = LensProfile {
        distortion: (0..16).map(|i| 1.0008 - 0.02 * (i as f32 / 15.0).powi(2)).collect(),
        distortion_on: true,
        ca_r: vec![0.999; 16],
        ca_b: vec![1.001; 16],
        ca_on: true,
        ..Default::default()
    };
    let r = EditRecipe { lens_profile: profile.clone(), ..Default::default() };
    assert!(
        matches!(geometry_profile(&r), std::borrow::Cow::Borrowed(_)),
        "a recipe with ca_r = ca_b = 0 must borrow the in-camera profile unchanged"
    );
    assert_eq!(*geometry_profile(&r), profile, "…and it is that profile, not a copy of one");
    let ramp = DynamicImage::ImageRgb16(ImageBuffer::from_fn(160, 90, |x, y| {
        Rgb([(x as u16) * 300, (x as u16) * 300 + (y as u16), (y as u16) * 500])
    }));
    let a = apply_lens_geometry(&ramp, &geometry_profile(&r), 0.0).to_rgb16();
    let b = apply_lens_geometry(&ramp, &profile, 0.0).to_rgb16();
    assert!(a.pixels().zip(b.pixels()).all(|(p, q)| p.0 == q.0), "the render must not move");
    // A profile whose CA the user switched OFF stays off: the manual pair
    // stands alone rather than scaling knots nobody asked for.
    let off = EditRecipe {
        lens_profile: LensProfile { ca_on: false, ..profile },
        ca_r: -50.0,
        ..Default::default()
    };
    let composed = geometry_profile(&off);
    assert_eq!(composed.ca_r.len(), 1, "the disabled profile knots must not participate");
    assert!((composed.ca_r[0] - (1.0 + -50.0 * MANUAL_CA_PER_UNIT)).abs() < 1e-9);
    assert_eq!(composed.ca_b, vec![1.0], "the untouched axis is exactly neutral");
}

/// **Every tier that renders nothing renders nothing** — re-derived from
/// the registry so a row that changes tier without gaining an engine stage
/// fails here.
///
/// R25 B3 wrote this over `Tier::CarriedOnly` alone, when that tier had
/// fifteen members. v1.5.0 emptied it — Track F: everything this app can
/// set now renders — so the sieve is the PROPERTY, `!tier.renders()`,
/// rather than the one tier that happened to have the members, and
/// `passthrough` is what keeps it from being a test over nothing. The
/// opposite direction, that the twenty-four promoted rows really do render,
/// is `advisor::catalogue::tests::
/// nothing_is_carried_any_more_and_every_row_that_left_renders` plus each
/// operator's own module test.
#[test]
fn a_tier_that_renders_nothing_renders_nothing() {
    use crate::advisor::catalogue::{global_value, Shape, RECIPE_CONTROLS};
    let img = DynamicImage::ImageRgb16(ImageBuffer::from_fn(64, 48, |x, y| {
        Rgb([(x as u16) * 900, (y as u16) * 1200, ((x + y) as u16) * 500])
    }));
    let neutral_recipe = EditRecipe::default();
    let neutral = develop_preview(&img, &neutral_recipe).to_rgb16();
    let mut probed = 0usize;
    for c in RECIPE_CONTROLS.iter().filter(|c| c.tier.is_some_and(|t| !t.renders())) {
        // Through serde, so a renamed field cannot slip past — and BY
        // SHAPE, because B3 put a flag on this tier and B4 a whole map.
        let mut json = serde_json::to_value(&neutral_recipe).expect("recipe serialises");
        json[c.name] = match c.shape {
            Shape::Bool => serde_json::json!(true),
            // `passthrough` is a MAP of crs keys nobody interprets, so the
            // probe has to be a key Lightroom really writes — an invented
            // one would be dropped and the probe would move nothing, which
            // this test would then read as inertness.
            Shape::EngineCarrier => serde_json::json!({ "PerspectiveVertical": "-35" }),
            _ => serde_json::json!(c.range.map(|(_, hi)| hi).unwrap_or(1.0)),
        };
        let mut r: EditRecipe = serde_json::from_value(json)
            .unwrap_or_else(|e| panic!("{}: probe value rejected: {e}", c.name));
        r.clamp();
        assert_ne!(
            global_value(&r, c.name),
            global_value(&neutral_recipe, c.name),
            "{}: the probe must actually move the control",
            c.name
        );
        let out = develop_preview(&img, &r).to_rgb16();
        assert!(
            out.pixels().zip(neutral.pixels()).all(|(p, q)| p.0 == q.0),
            "{}: a non-rendering tier moved a pixel — then it is not carried, it renders",
            c.name
        );
        // The GEOMETRIC stage runs after `develop_preview`, so inertness
        // there needs its own word: a carried control must not reach the
        // composed lens profile either (the manual CA pair is the only
        // thing on that path, and it is `Rendered`).
        assert_eq!(
            *geometry_profile(&r),
            *geometry_profile(&neutral_recipe),
            "{}: a non-rendering tier changed the composed lens geometry",
            c.name
        );
        // …nor the stage AFTER the geometry, which v1.5.0 added: an
        // operator that reached only the post-crop finish would have passed
        // both assertions above and still changed the delivered frame.
        let tail = |r: &EditRecipe| {
            frame_and_finish(
                DynamicImage::ImageRgb16(neutral.clone()),
                r,
                &geometry_profile(r),
                FilmScale::NATIVE,
                CropPolicy::Cut,
            )
            .to_rgb16()
        };
        assert!(
            tail(&r).pixels().zip(tail(&neutral_recipe).pixels()).all(|(p, q)| p.0 == q.0),
            "{}: a non-rendering tier moved a pixel in the finishing pass",
            c.name
        );
        probed += 1;
    }
    // The premise, and it is now a NAMED one: with `CarriedOnly` empty,
    // `passthrough` is the only row this sieve can reach, so an unnamed
    // count would slide to zero the day someone retired it.
    assert_eq!(
        probed, 1,
        "premise: B4's `passthrough` is the last non-rendering row — Track F emptied \
         `CarriedOnly`, so a second one here is a decision and a zero is a hole"
    );
}

/// L04-2: the C2 coordinate contract — the GUI's normalised maps are the
/// render's GREEN map, which CA does not touch (v1.17.0: each channel moves
/// by its own knots), so masks/dropper/clone points stay on the pixels; the
/// forward/inverse pair round-trips; and a CA-only profile maps every point
/// to itself.
#[test]
fn the_gui_map_is_the_renders_green_map() {
    use crate::recipe::LensProfile;
    let profile = LensProfile {
        distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
        ca_r: vec![1.004; 16],
        ca_b: vec![0.997; 16],
        distortion_on: true,
        ca_on: true,
        ..Default::default()
    };
    let dims = (1200.0, 800.0);
    // The normalised map's radial factor at an edge point equals the
    // render's green factor at the same rn — the red knot past 1 adds no
    // zoom to it.
    let (nx, ny) = (0.98, 0.5);
    let (ox, _) = lens_geom_norm(nx, ny, dims, &profile, 0.0);
    let f_norm = (ox - 0.5) / (nx - 0.5);
    let (w, h) = dims;
    let rn = ((nx - 0.5) * w).abs() / (0.5 * (w * w + h * h).sqrt());
    let s_p = profile_fill_scale(&profile.distortion, dims);
    let expect = lens_geom_factor(rn, &profile.distortion, s_p, 0.0, 1.0);
    assert!(
        (f_norm - expect).abs() < 1e-4,
        "the GUI map factor {f_norm} drifted from the render's {expect}"
    );
    for (px, py) in [(0.1, 0.2), (0.9, 0.85), (0.5, 0.05)] {
        let (ax, ay) = lens_geom_norm(px, py, dims, &profile, 12.0);
        let (bx, by) = lens_ungeom_norm(ax, ay, dims, &profile, 12.0);
        assert!(
            (bx - px).abs() < 2e-3 && (by - py).abs() < 2e-3,
            "roundtrip ({px},{py}) → ({bx},{by})"
        );
    }
    // Distortion OFF + a channel past 1: the shared map stays put.
    let ca_only = LensProfile {
        ca_r: vec![1.02; 16],
        ca_b: vec![0.98; 16],
        ca_on: true,
        ..Default::default()
    };
    for (px, py) in [(0.9, 0.5), (0.8, 0.3), (0.02, 0.97)] {
        let (mx, my) = lens_geom_norm(px, py, dims, &ca_only, 0.0);
        let (ux, uy) = lens_ungeom_norm(px, py, dims, &ca_only, 0.0);
        assert!(
            (mx - px).abs() < 1e-6 && (my - py).abs() < 1e-6 && (ux - px).abs() < 1e-6 && (uy - py).abs() < 1e-6,
            "CA alone moved the shared map: ({px},{py}) → ({mx},{my}) / ({ux},{uy})"
        );
    }
}

/// The black is subtracted signed (v1.17.0): a mosaic whose sites sit at the
/// black plus and minus the same read noise develops to a mean of zero, not
/// to the positive bias rawler's clip at 0 leaves (`imgop/raw.rs`,
/// `correct_blacklevel_cfa`). A site above the black keeps rawler's own value
/// to the bit, through the same demosaic.
///
/// MUTATION: clip the subtraction at 0 again, or leave rawler's `Rescale` in.
#[test]
fn the_black_is_subtracted_signed_through_the_demosaic() {
    use rawler::cfa::{PlaneColor, CFA};
    use rawler::decoders::Camera;
    use rawler::imgop::develop::{ProcessingStep, RawDevelop};
    use rawler::rawimage::{BlackLevel, CFAConfig, RawImageData, RawPhotometricInterpretation, WhiteLevel};
    let (w, h) = (16usize, 16usize);
    let mosaic = |values: Vec<u16>| {
        rawler::RawImage::new_with_data(
            Camera::new(),
            RawImageData::Integer(values),
            w,
            h,
            1,
            [1.0, 1.0, 1.0, f32::NAN],
            RawPhotometricInterpretation::Cfa(CFAConfig::new(&CFA::new("RGGB"), &PlaneColor::new("RGB"))),
            Some(BlackLevel::new(&[512u16], 1, 1, 1)),
            Some(WhiteLevel::new(vec![16383])),
            false,
        )
    };
    let mut dev = RawDevelop::default();
    dev.steps.retain(|s| !matches!(s, ProcessingStep::WhiteBalance | ProcessingStep::Calibrate | ProcessingStep::SRgb));
    let develop = |raw: &rawler::RawImage| source::develop_raw_buffer(raw, &dev, None, None, ExportColorSpace::Srgb, None, None, None, false).unwrap().0;
    let mean = |px: &[[f32; 3]]| px.iter().map(|p| f64::from(p[0] + p[1] + p[2]) / 3.0).sum::<f64>() / px.len() as f64;
    // Black ± 40 DN in a checkerboard of 2×2 cells: every channel sees both signs.
    let noisy: Vec<u16> = (0..w * h).map(|i| if ((i % w) / 2 + (i / w) / 2) % 2 == 0 { 552 } else { 472 }).collect();
    let signed = develop(&mosaic(noisy.clone()));
    let clipped = {
        let mut raw = mosaic(noisy);
        raw.apply_scaling().unwrap();
        let mut d = dev.clone();
        d.steps.retain(|s| *s != ProcessingStep::Rescale);
        d.develop_intermediate(&raw).unwrap()
    };
    let rawler::imgop::develop::Intermediate::ThreeColor(clipped) = clipped else { panic!("three-colour develop") };
    let (m_signed, m_clipped) = (mean(&signed), mean(&clipped.data));
    let unit = 40.0 / (16383.0 - 512.0);
    assert!(m_signed.abs() < 0.05 * unit, "signed mean {m_signed:e} (one noise unit {unit:e})");
    assert!(m_clipped > 0.4 * unit, "rawler's clip leaves a positive bias: {m_clipped:e}");
    // Above the black the two paths agree to the bit.
    let bright: Vec<u16> = (0..w * h).map(|i| 1000 + (i as u16 * 37) % 900).collect();
    let ours = develop(&mosaic(bright.clone()));
    let mut raw = mosaic(bright);
    raw.apply_scaling().unwrap();
    let mut d = dev.clone();
    d.steps.retain(|s| *s != ProcessingStep::Rescale);
    let rawler::imgop::develop::Intermediate::ThreeColor(theirs) = d.develop_intermediate(&raw).unwrap() else {
        panic!("three-colour develop")
    };
    assert_eq!(ours, theirs.data, "above the black the signed subtraction is rawler's own");
}
