// One part of the engine's tests (src/render/tests.rs includes it): local masks, colour gain, white balance, gamut and profile handling, vignette gain, export options and sizes, kelvin math.

#[test]
fn linear_mask_affects_only_the_full_end() {
    // Linear mask: zero at top (ny=0), full at bottom (ny=1) + strong darken.
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 },
            amount: 1.0,
            exposure_ev: -4.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let (w, h) = (1usize, 4usize);
    let mut data = vec![[0.6_f32, 0.6, 0.6]; w * h];
    apply_develop_anon(&mut data, w, h, &r);
    // The four rows sample at their CENTRES, ny = (y + 0.5)/4, so the
    // weights are exactly 1/8, 3/8, 5/8, 7/8 — the top row is no longer
    // AT the gradient's zero end, it is an eighth of the way past it, and
    // it darkens according to the shipped profile (R29 C2's half-pixel
    // move, visible here). Pinned EXACTLY rather than bounded:
    // a degenerate linear (zero == full) renders weight 1 everywhere, so
    // this comparison carries the identical shipped weight through every
    // stage below it.
    let top_weight = linear_coverage(0.125, LINEAR_FALLOFF);
    let eighth = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
            amount: top_weight,
            exposure_ev: -4.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut eighth_px = vec![[0.6_f32, 0.6, 0.6]; 1];
    apply_develop_anon(&mut eighth_px, 1, 1, &eighth);
    assert_eq!(data[0], eighth_px[0], "top row carries exactly its shipped coverage");
    assert!(data[0][0] < 0.6, "…which is a real darkening: {}", data[0][0]);
    assert!(data[3][0] < 0.5, "bottom should darken: {}", data[3][0]);
    // The interior rows carry the actual gradient — the endpoint checks
    // alone let a "positive weight ⇒ full coverage" mutation render the
    // ramp as a hard step (row 0 has weight EXACTLY 0 and stays pinned;
    // row 3 only darkens further) (U14).
    assert!(
        data[1][0] > data[2][0] + 0.05 && data[2][0] > data[3][0] + 0.05,
        "linear ramp collapsed to a step: {:?}",
        [data[1][0], data[2][0], data[3][0]]
    );
}

#[test]
fn local_noise_reduction_smooths_only_inside_the_mask() {
    // 8x1 strip of alternating luma (= noise). A linear mask covering the
    // RIGHT half with full local NR should flatten the right; left untouched.
    //
    // ±0.03, a real noise amplitude. Since v1.5.0 the local pass is the
    // global Detail operator (`render/detail.rs`), which is EDGE-AWARE: a
    // ±0.2 alternation is structure to it at any amount (local σ 0.2
    // against a noise threshold of 0.054 at Luminance 100), where the
    // pre-v1.5.0 local pass was a plain blur that flattened anything.
    let (w, h) = (8usize, 1usize);
    let mut data: Vec<[f32; 3]> =
        (0..w).map(|x| { let v = if x % 2 == 0 { 0.47 } else { 0.53 }; [v, v, v] }).collect();
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 1.0, full_y: 0.5 },
            amount: 1.0,
            noise_reduction: 100.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let var = |d: &[[f32; 3]], rng: std::ops::Range<usize>| {
        let v: Vec<f32> = rng.map(|i| d[i][0]).collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32
    };
    // Control render (mask-less, same global stages): "untouched" must
    // mean BIT-FOR-BIT equal — the convention the range-mask tests use.
    // The old probe compared the left half's red-channel VARIANCE, which
    // is blind to a constant offset (translation-invariant) and to
    // green/blue-only leaks (it never read those channels) (U14).
    let mut control = data.clone();
    apply_develop_anon(&mut control, w, h, &EditRecipe::default());
    let right0 = var(&data, 4..8);
    apply_develop_anon(&mut data, w, h, &r);
    assert!(var(&data, 4..8) < right0 * 0.8, "right half should smooth");
    assert_eq!(&data[0..4], &control[0..4], "left half untouched, bit for bit");
}

#[test]
fn luminance_range_mask_gates_by_pixel_brightness() {
    // Full-coverage geometry (degenerate linear = weight 1 everywhere) so
    // ONLY the luminance range decides where the −2 EV darken lands. The
    // trapezoid uses a degenerate top edge (hi == hi_outer == 1.0), exactly
    // like the real ACR sidecars' `LumRange="… 1.000000 1.000000"`.
    let full = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 };
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: full,
            range: Some(RangeMask::Luminance { lo_outer: 0.55, lo: 0.7, hi: 1.0, hi_outer: 1.0 }),
            amount: 1.0,
            exposure_ev: -2.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let dark = [0.15_f32; 3];
    let mid = [0.625_f32; 3]; // ramp midpoint between lo_outer and lo
    let bright = [0.85_f32; 3];
    // Control: identical pipeline WITHOUT the mask. The global stages run
    // either way (the neutral tone LUT still costs ~1 ULP of interpolation
    // noise), so "untouched by the mask" means equal to the CONTROL, not to
    // the raw input.
    let mut control = vec![dark, mid, bright];
    apply_develop_anon(&mut control, 3, 1, &EditRecipe::default());
    let mut data = vec![dark, mid, bright];
    apply_develop_anon(&mut data, 3, 1, &r);
    assert_eq!(data[0], control[0], "below the range: the mask must skip it");
    assert!(data[2][0] < 0.6, "bright pixel must darken: {}", data[2][0]);
    // The ramp midpoint moves, but less than the fully-selected pixel.
    let (d_mid, d_bright) = (control[1][0] - data[1][0], control[2][0] - data[2][0]);
    assert!(d_mid > 0.01 && d_mid < d_bright, "feathered ramp: mid {d_mid} vs bright {d_bright}");
}

#[test]
fn color_range_mask_selects_chroma_not_brightness() {
    // Desaturate through a colour range keyed to orange: both bright and
    // dark orange collapse to grey (luminance-invariant match), while blue
    // and neutral grey pass through bit-exact.
    let full = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 };
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: full,
            range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 0.5, px: 0.5, py: 0.5 }),
            amount: 1.0,
            saturation: -100.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let orange = [0.9_f32, 0.6, 0.2];
    let dark_orange = [0.45_f32, 0.3, 0.1]; // same chromaticity, half as bright
    let blue = [0.2_f32, 0.3, 0.9];
    let grey = [0.6_f32; 3];
    // Same control-render comparison as the luminance test: out-of-range
    // pixels must match a mask-less render exactly (the mask pass skips them).
    let mut control = vec![orange, dark_orange, blue, grey];
    apply_develop_anon(&mut control, 4, 1, &EditRecipe::default());
    let mut data = vec![orange, dark_orange, blue, grey];
    apply_develop_anon(&mut data, 4, 1, &r);
    let spread = |p: [f32; 3]| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
    assert!(spread(data[0]) < 0.05, "orange must desaturate: {:?}", data[0]);
    assert!(spread(data[1]) < 0.05, "dark orange (same hue) must desaturate: {:?}", data[1]);
    assert_eq!(data[2], control[2], "opposite hue: the mask must skip it");
    assert_eq!(data[3], control[3], "neutral grey: the mask must skip it");
    // Desaturation must land at the pixel's LUMA, not at black — spread
    // alone cannot tell grey from destroyed (a `c * factor` rewrite of
    // the sat formula yields [0,0,0] with spread 0 and passed) (U14).
    let lum = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    assert!(
        (data[0][0] - lum(control[0])).abs() < 0.03,
        "orange must keep its brightness: {:?} vs luma {}",
        data[0],
        lum(control[0])
    );
    assert!(
        (data[1][0] - lum(control[1])).abs() < 0.03,
        "dark orange must keep its brightness: {:?} vs luma {}",
        data[1],
        lum(control[1])
    );
}

#[test]
fn local_temperature_warms_the_masked_region_only() {
    // Feedback batch #2-B prerequisite: LocalAdjustment carried Temp/Tint
    // since v1 and the XMP writer exports them, but the ENGINE ignored
    // them (render.rs listed them as "deferred") — so the GUI's mask
    // Temp/Tint sliders did nothing in-app, and the zoned reverse-fit
    // would have nothing to drive. A warm local temperature must boost
    // red / cut blue inside the mask; a row of weight EXACTLY 0 must stay
    // equal to a mask-less control render (the mask pass skips it).
    //
    // The gradient's zero end sits at `zero_y = 0.125`, which is the TOP
    // ROW'S CENTRE on this 4-row frame (R29 C2: rows sample at
    // ny = (y + 0.5)/4). It has to, for the skip to be exercised at all —
    // under pixel-centre sampling no row of a 0→1 gradient carries weight
    // 0, and the old `zero_y = 0.0` fixture stopped testing the skip the
    // moment the convention was corrected. The weights here are exactly
    // 0, 1/3, 2/3, 1: `(ny − 0.125)·0.75 / 0.5625`, all dyadic.
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear {
                zero_x: 0.5, zero_y: 0.125, full_x: 0.5, full_y: 0.875,
            },
            amount: 1.0,
            temperature: 100.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let grey = [0.5_f32; 3];
    let (w, h) = (1usize, 4usize);
    let mut control = vec![grey; w * h];
    apply_develop_anon(&mut control, w, h, &EditRecipe::default());
    let mut data = vec![grey; w * h];
    apply_develop_anon(&mut data, w, h, &r);
    assert_eq!(data[0], control[0], "zero end of the gradient: the mask must skip it");
    let px = data[3];
    assert!(
        px[0] > grey[0] + 0.02 && px[2] < grey[2] - 0.02,
        "full end must warm (red up, blue down): {px:?}"
    );
    // …and the SAME geometry read on the old `y/h` grid would give the top
    // row weight (0 − 0.125)·0.75/0.5625 < 0 → clamped to 0 as well, so the
    // skip alone cannot separate the two conventions. The bottom row can:
    // at ny = 0.875 the weight is exactly 1, while `y/h` puts row 3 at
    // ny = 0.75 → weight 5/6, short of the full end. Assert the full end is
    // REACHED by comparing against an amount-1 whole-frame mask.
    let full = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
            amount: 1.0,
            temperature: 100.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut full_px = vec![grey; 1];
    apply_develop_anon(&mut full_px, 1, 1, &full);
    assert_eq!(data[3], full_px[0], "the last row's centre IS the gradient's full end");
}

#[test]
fn colour_gain_lut_matches_the_exact_linear_light_formula() {
    // Pin the optimization's fidelity independently of apply_wb: compare
    // LUT interpolation against the old exact formula over dark values
    // (where the sRGB knee is hardest), midtones and highlights, across
    // both sub-unity and strong zoned-fit gains. The tolerance is below
    // one 16-bit code value (1/65535 ≈ 1.53e-5).
    for gains in [[0.41f32, 0.91, 1.45], [1.65, 0.76, 0.38], [1.0, 1.0, 1.0]] {
        let luts = colour_gain_luts(gains);
        for x in [0.0f32, 0.001, 0.003, 0.01, 0.04, 0.1, 0.25, 0.5, 0.8, 0.99, 1.0] {
            for ch in 0..3 {
                let exact =
                    linear_to_srgb((srgb_to_linear(x) * gains[ch]).clamp(0.0, 1.0));
                let fast = sample_lut(&luts[ch], x);
                assert!(
                    (fast - exact).abs() < 1.5e-5,
                    "gain {} x {x}: LUT {fast} vs exact {exact}",
                    gains[ch]
                );
            }
        }
    }
}

#[test]
fn full_frame_local_wb_matches_the_global_wb_stage() {
    // The local Temp/Tint must MIRROR apply_recipe_wb's semantics — same
    // wb_gains model, same 5500 K anchor, WB→tone→sat order — so a
    // weight-1 full-frame mask must land within LUT-quantization of a
    // global render whose absolute Kelvin is the mired-mapped target.
    // This pins local_temp_to_kelvin AND the tint sign end to end.
    let full = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 };
    let (t_rel, tint) = (60.0_f32, -25.0_f32);
    let src = [[0.7_f32, 0.55, 0.35], [0.2, 0.35, 0.6], [0.5, 0.5, 0.5]];
    let mut global = src.to_vec();
    apply_recipe_wb(
        &mut global,
        &EditRecipe {
            temperature_k: Some(local_temp_to_kelvin(t_rel)),
            tint,
            ..Default::default()
        },
    );
    apply_develop_anon(&mut global, 3, 1, &EditRecipe::default());
    let mut local = src.to_vec();
    apply_develop_anon(
        &mut local,
        3,
        1,
        &EditRecipe {
            masks: vec![LocalAdjustment {
                mask: full,
                amount: 1.0,
                temperature: t_rel,
                tint,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    for (a, b) in global.iter().zip(&local) {
        for c in 0..3 {
            assert!(
                (a[c] - b[c]).abs() < 3e-3,
                "full-frame local WB drifted from the global stage: {a:?} vs {b:?}"
            );
        }
    }
}

#[test]
fn exports_are_tagged_srgb_in_all_three_formats() {
    // Every export format must carry the sRGB profile: JPEG in an APP2
    // "ICC_PROFILE" segment, PNG in an iCCP chunk, TIFF as the raw profile
    // (tag 34675) whose header signature is "acsp".
    std::fs::create_dir_all("out").ok();
    let src_p = std::path::Path::new("out/_icc_src.png");
    RgbImage::from_fn(32, 16, |x, y| Rgb([(x * 8) as u8, (y * 16) as u8, 128]))
        .save(src_p)
        .unwrap();
    let neutral = EditRecipe::default();
    for (name, needle) in [
        ("out/_icc.jpg", &b"ICC_PROFILE"[..]),
        ("out/_icc.png", &b"iCCP"[..]),
        ("out/_icc.tif", &b"acsp"[..]),
    ] {
        render_to_file(src_p, &neutral, std::path::Path::new(name), None, None, crate::diag::stderr()).unwrap();
        let bytes = std::fs::read(name).unwrap();
        assert!(
            bytes.windows(needle.len()).any(|win| win == needle),
            "{name} must carry the sRGB ICC marker"
        );
        // The markers above match ANY ICC payload — pin the PROFILE:
        // tag_icc's Srgb arm shipping the P3/Adobe bytes passed every
        // marker check (U14). JPEG/TIFF store the profile verbatim; PNG
        // deflate-compresses it, so compare the DECODED profile.
        if name.ends_with(".png") {
            let mut d = image::codecs::png::PngDecoder::new(std::io::BufReader::new(
                std::fs::File::open(name).unwrap(),
            ))
            .unwrap();
            assert_eq!(
                image::ImageDecoder::icc_profile(&mut d).unwrap().unwrap(),
                SRGB_ICC.to_vec(),
                "{name} must embed the sRGB profile (decompressed iCCP)"
            );
        } else {
            assert!(
                bytes.windows(SRGB_ICC.len()).any(|win| win == SRGB_ICC),
                "{name} must embed the sRGB profile bytes"
            );
        }
    }
}

#[test]
fn gamut_transform_is_colorimetric_not_a_tag_swap() {
    // (a) White preservation pins the whole matrix derivation: every row of
    // sRGB→target must sum to 1 (R=G=B=1 stays exactly white — all three
    // spaces share the D65 white point, so no adaptation term may appear).
    for space in [ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
        let m = srgb_to_space_matrix(space).unwrap();
        for (i, row) in m.iter().enumerate() {
            let s: f32 = row.iter().sum();
            assert!((s - 1.0).abs() < 1e-3, "{space:?} row {i} sums to {s}");
        }
        // (b) Invertibility: a color grid survives forward → inverse.
        let inv = inv3(&m);
        for c in [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.7, 0.2, 0.55]] {
            let back = mat_vec3(&inv, &mat_vec3(&m, &c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-3, "{space:?} roundtrip {c:?} → {back:?}");
            }
        }
    }

    // (c) Full pixel path on a mid-grey: P3 shares sRGB's TRC, so a neutral
    // pixel is numerically UNCHANGED; Adobe RGB's pure gamma encodes the
    // same grey darker — while staying exactly neutral. That difference is
    // the transform actually running (a tag swap would leave both equal).
    let grey = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(2, 2, Rgb([32896u16, 32896, 32896])));
    let p3 = convert_export_color_space(grey.clone(), ExportColorSpace::DisplayP3).to_rgb16();
    let (pr, pg, pb) = (p3.get_pixel(0, 0)[0], p3.get_pixel(0, 0)[1], p3.get_pixel(0, 0)[2]);
    assert!(pr == pg && pg == pb, "P3 grey must stay neutral: {pr},{pg},{pb}");
    assert!((pr as i32 - 32896).abs() <= 4, "P3 grey must keep its value: {pr}");
    let ad = convert_export_color_space(grey.clone(), ExportColorSpace::AdobeRgb).to_rgb16();
    let (ar, ag, ab) = (ad.get_pixel(0, 0)[0], ad.get_pixel(0, 0)[1], ad.get_pixel(0, 0)[2]);
    assert!(ar == ag && ag == ab, "AdobeRGB grey must stay neutral: {ar},{ag},{ab}");
    assert!((ar as i32) < pr as i32 - 64, "AdobeRGB gamma must encode grey darker: {ar} vs {pr}");

    // (d) Saturated sRGB red. P3's red primary sits further out, so sRGB
    // red lands strictly INSIDE (dominant red, positive green/blue).
    // Adobe RGB shares sRGB's red CHROMATICITY, so sRGB red stays a pure
    // red there — just rescaled (Adobe's red carries a larger luminance
    // share): g = b = 0 with red below full scale. Both derive from the
    // primaries table, so both directions pin the matrix.
    let red = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([65535u16, 0, 0])));
    let p3r = convert_export_color_space(red.clone(), ExportColorSpace::DisplayP3).to_rgb16();
    let p = p3r.get_pixel(0, 0);
    assert!(
        p[0] > 55000 && p[1] > 0 && p[2] > 0 && p[1] < p[0] && p[2] < p[0],
        "DisplayP3: sRGB red must land inside the gamut, got {p:?}"
    );
    let adr = convert_export_color_space(red, ExportColorSpace::AdobeRgb).to_rgb16();
    let q = adr.get_pixel(0, 0);
    assert!(
        q[0] > 50000 && q[0] < 62000 && q[1] <= 300 && q[2] <= 300,
        "AdobeRGB: sRGB red must stay a rescaled pure red, got {q:?}"
    );

    // (d2) Green and blue primaries pin the REMAINING columns: the red
    // probe alone survives a green/blue column swap of the matrix — row
    // sums, invertibility, grey and red are all invariant under it, and
    // so is the calibration cross-check, which derives from the same
    // primaries table (U14). All three spaces share the same blue
    // CHROMATICITY, so sRGB blue stays a pure blue in both targets;
    // under the swap each primary would receive the other's column.
    let green = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([0u16, 65535, 0])));
    let blue = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([0u16, 0, 65535])));
    for space in [ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
        let g = convert_export_color_space(green.clone(), space).to_rgb16();
        let p = g.get_pixel(0, 0);
        assert!(
            p[1] > 50000 && p[0] < p[1] && p[2] < p[1],
            "{space:?}: sRGB green must stay dominant-green, got {p:?}"
        );
        let b = convert_export_color_space(blue.clone(), space).to_rgb16();
        let p = b.get_pixel(0, 0);
        assert!(
            p[2] > 55000 && p[0] < 1000 && p[1] < 1000,
            "{space:?}: sRGB blue shares the target's blue primary — must stay pure, got {p:?}"
        );
    }

    // (e) sRGB is the identity (now a MOVE, not a clone).
    let same = convert_export_color_space(grey, ExportColorSpace::Srgb).to_rgb16();
    assert_eq!(same.get_pixel(1, 1)[0], 32896);
}

#[test]
fn wide_develop_calibration_agrees_with_the_export_matrix() {
    // A synthetic camera whose native space IS sRGB: xyz2cam =
    // inv(sRGB→XYZ). The DNG calibration into a target space must then
    // equal the sRGB→target export matrix — the two derivations meet.
    let xyz2cam = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
    for space in [ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
        let cam2space = camera_to_space_matrix(&xyz2cam, space);
        let reference = srgb_to_space_matrix(space).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (cam2space[i][j] - reference[i][j]).abs() < 2e-3,
                    "{space:?} [{i}][{j}]: {} vs {}",
                    cam2space[i][j],
                    reference[i][j]
                );
            }
        }
    }
}

#[test]
fn wide_develop_keeps_out_of_srgb_camera_colors_and_neutral_parity() {
    // A camera whose native space IS Display P3: its pure red lies
    // OUTSIDE sRGB. Developed INTO DisplayP3 it must survive as ~[1,0,0]
    // — the colour the old clip-at-sRGB pipeline destroyed.
    let xyz2cam = inv3(&rgb_to_xyz(P3_PRIM, D65_XY));
    let mut px = [[1.0f32, 0.0, 0.0]];
    calibrate_camera_buffer(&mut px, &xyz2cam, [1.0, 1.0, 1.0], ExportColorSpace::DisplayP3, None);
    let p = px[0];
    assert!(
        p[0] > 0.99 && p[1].abs() < 0.02 && p[2].abs() < 0.02,
        "P3-native red must survive a P3 develop, got {p:?}"
    );
    // The same HUE at 80% (an unblown saturated colour — full-scale 1.0
    // is a clipped sensor reading and legitimately takes the highlight
    // desaturation, same as rawler) into sRGB goes out of gamut: a
    // NEGATIVE component must reach the caller (the final pack clips it
    // — not the decode).
    let mut px = [[0.8f32, 0.0, 0.0]];
    calibrate_camera_buffer(&mut px, &xyz2cam, [1.0, 1.0, 1.0], ExportColorSpace::Srgb, None);
    assert!(
        px[0][1] < -0.001 || px[0][2] < -0.001,
        "out-of-gamut components must SURVIVE to the pack, got {:?}",
        px[0]
    );
    // Neutral parity: a white-balanced grey encodes to the SAME value in
    // every space (shared D65 white + shared working transfer) — the
    // whole reason the wide develop may share the sRGB tone pipeline.
    let wb = [2.0f32, 1.0, 1.5];
    let grey_cam = [[0.4 / 2.0, 0.4, 0.4 / 1.5]];
    let mut out = [[0.0f32; 3]; 3];
    for (i, space) in
        [ExportColorSpace::Srgb, ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb]
            .into_iter()
            .enumerate()
    {
        let mut px = grey_cam;
        calibrate_camera_buffer(&mut px, &xyz2cam, wb, space, None);
        out[i] = px[0];
    }
    let want = linear_to_srgb(0.4);
    for (i, o) in out.iter().enumerate() {
        for c in o {
            assert!((c - want).abs() < 2e-3, "space {i}: grey drifted to {o:?} (want {want})");
        }
    }
}

#[test]
fn adobe_trc_transcode_matches_the_conversion_path_on_neutrals() {
    // A grey through the native-wide path (primaries already Adobe,
    // transfer swap only) must land where the matrix path lands it —
    // on neutrals the matrix is a no-op, isolating the TRC.
    let grey = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([32896u16, 32896, 32896])));
    let via_matrix =
        convert_export_color_space(grey.clone(), ExportColorSpace::AdobeRgb).to_rgb16();
    let via_transcode = transcode_srgb_trc_to_adobe(grey).to_rgb16();
    let (a, b) = (via_matrix.get_pixel(0, 0), via_transcode.get_pixel(0, 0));
    for c in 0..3 {
        assert!(
            (a[c] as i32 - b[c] as i32).abs() <= 1,
            "TRC transcode must agree with the conversion path: {a:?} vs {b:?}"
        );
    }
}

#[test]
fn exports_embed_the_selected_wide_gamut_profile() {
    // JPEG (APP2, one segment at 736 B) and TIFF (tag 34675) store the raw
    // profile — the ENTIRE profile bytes must appear in the file. PNG
    // deflate-compresses inside iCCP, so its profile is compared
    // DECOMPRESSED via the decoder — the old claim that the sRGB test's
    // chunk check covered it was FALSE: that check only proves an iCCP
    // chunk exists, so a PNG arm shipping sRGB bytes under a P3
    // selection passed everything (U14).
    std::fs::create_dir_all("out").ok();
    let src_p = std::path::Path::new("out/_gamut_src.png");
    RgbImage::from_fn(24, 12, |x, y| Rgb([(x * 10) as u8, (y * 20) as u8, 90]))
        .save(src_p)
        .unwrap();
    let neutral = EditRecipe::default();
    for (space, profile) in [
        (ExportColorSpace::DisplayP3, DISPLAY_P3_ICC),
        (ExportColorSpace::AdobeRgb, ADOBE_RGB_ICC),
    ] {
        let opts = ExportOpts { color_space: space, ..Default::default() };
        for name in ["out/_gamut.jpg", "out/_gamut.tif"] {
            render_to_file(src_p, &neutral, std::path::Path::new(name), None, Some(&opts), crate::diag::stderr()).unwrap();
            let bytes = std::fs::read(name).unwrap();
            assert!(
                bytes.windows(profile.len()).any(|win| win == profile),
                "{name} must embed the full {space:?} profile ({} B)",
                profile.len()
            );
        }
        let png_name = "out/_gamut.png";
        render_to_file(src_p, &neutral, std::path::Path::new(png_name), None, Some(&opts), crate::diag::stderr()).unwrap();
        let mut d = image::codecs::png::PngDecoder::new(std::io::BufReader::new(
            std::fs::File::open(png_name).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            image::ImageDecoder::icc_profile(&mut d).unwrap().unwrap(),
            profile.to_vec(),
            "{png_name} must embed the full {space:?} profile"
        );
    }
}

#[test]
fn vignette_gain_is_radial_and_linear_light() {
    // A flat mid-grey field: +60 compensation must leave the exact centre
    // untouched, brighten the corner the most, and increase monotonically
    // with radius. Negative amount darkens the corner instead.
    let (w, h) = (9usize, 9usize);
    let flat = vec![[0.5_f32; 3]; w * h];
    let mut up = flat.clone();
    apply_radial_gain(&mut up, w, h, &manual_vignette_lut(60.0, 50.0));
    let centre = up[4 * w + 4][0];
    let mid = up[2 * w + 2][0]; // halfway toward the corner
    let corner = up[0][0];
    assert!((centre - 0.5).abs() < 1e-4, "centre must not move: {centre}");
    assert!(corner > mid && mid > centre, "radial monotone: {centre} < {mid} < {corner}");
    assert!(corner > 0.62, "corner must clearly brighten: {corner}");
    // Pin the NAMED linear-light formula at the exact corner (rn = 1 →
    // gain = 1.6): decode → gain → encode. Gamma-space multiplication
    // (0.5·1.6 = 0.8) clears every shape assertion here yet misses this
    // by 0.18 (U14).
    let expect = linear_to_srgb((srgb_to_linear(0.5) * 1.6).min(1.0));
    assert!(
        (corner - expect).abs() < 2e-3,
        "corner must follow the linear-light gain: {corner} vs {expect}"
    );
    // Grey in, grey out: the gain applies to all three channels — every
    // assertion above reads channel 0 only, so a red-only regression of
    // the per-channel loop passed with a strong colour cast (R12).
    for p in [&up[0], &up[2 * w + 2]] {
        assert!(
            (p[0] - p[1]).abs() < 1e-6 && (p[1] - p[2]).abs() < 1e-6,
            "vignette must stay neutral on grey: {p:?}"
        );
    }

    let mut down = flat.clone();
    apply_radial_gain(&mut down, w, h, &manual_vignette_lut(-60.0, 50.0));
    assert!(down[0][0] < 0.38, "negative amount darkens the corner: {}", down[0][0]);

    // Higher midpoint confines the effect to the corners: the halfway
    // pixel moves LESS than with the default midpoint.
    let mut tight = flat.clone();
    apply_radial_gain(&mut tight, w, h, &manual_vignette_lut(60.0, 100.0));
    assert!(tight[2 * w + 2][0] < mid, "midpoint 100 must spare the mid-field");
}

#[test]
fn export_opts_resize_sharpen_quality() {
    // Synthetic 200×100 gradient source (baked path), rendered through the
    // delivery pipeline. Long edge 50 → 50×25 saved AND reported; a long
    // edge larger than the source never upscales; lower JPEG quality
    // produces a smaller file than higher quality.
    std::fs::create_dir_all("out").ok();
    let src_p = std::path::Path::new("out/_export_src.png");
    let img = RgbImage::from_fn(200, 100, |x, y| {
        Rgb([(x % 256) as u8, (y * 2 % 256) as u8, ((x + y) % 256) as u8])
    });
    img.save(src_p).unwrap();
    let neutral = EditRecipe::default();

    let small = ExportOpts { long_edge: Some(50), sharpen: 25.0, ..Default::default() };
    let (w, h) =
        render_to_file(src_p, &neutral, std::path::Path::new("out/_export_le50.png"), None, Some(&small), crate::diag::stderr())
            .unwrap();
    assert_eq!((w, h), (50, 25), "long edge 50 must fit 200×100 to 50×25");
    let saved = image::image_dimensions("out/_export_le50.png").unwrap();
    assert_eq!(saved, (50, 25), "saved file dims must match the report");

    let big = ExportOpts { long_edge: Some(400), ..Default::default() };
    let (w, h) =
        render_to_file(src_p, &neutral, std::path::Path::new("out/_export_le400.png"), None, Some(&big), crate::diag::stderr())
            .unwrap();
    assert_eq!((w, h), (200, 100), "long edge beyond source must NOT upscale");

    for (q, name) in [(30u8, "out/_export_q30.jpg"), (95u8, "out/_export_q95.jpg")] {
        let opts = ExportOpts { jpeg_quality: q, ..Default::default() };
        render_to_file(src_p, &neutral, std::path::Path::new(name), None, Some(&opts), crate::diag::stderr()).unwrap();
    }
    let (s30, s95) = (
        std::fs::metadata("out/_export_q30.jpg").unwrap().len(),
        std::fs::metadata("out/_export_q95.jpg").unwrap().len(),
    );
    assert!(s30 < s95, "q30 ({s30} B) must be smaller than q95 ({s95} B)");

    // Output sharpening must be OBSERVABLE: same source, same size, only
    // `sharpen` differs — the sharpened file needs strictly more edge
    // energy. Deleting the whole post-resize stage changed no previously
    // asserted quantity (dims and JPEG sizes are blind to it) (U14).
    let edge_p = std::path::Path::new("out/_export_edge.png");
    RgbImage::from_fn(200, 100, |x, _| Rgb([if x < 100 { 40 } else { 215 }; 3]))
        .save(edge_p)
        .unwrap();
    let export = |sharpen: f32, name: &str| {
        let opts = ExportOpts { long_edge: Some(100), sharpen, ..Default::default() };
        render_to_file(edge_p, &neutral, std::path::Path::new(name), None, Some(&opts), crate::diag::stderr()).unwrap();
        let px = image::open(name).unwrap().to_rgb8();
        let (w, h) = px.dimensions();
        // Per channel: a red-only sharpen raised the summed energy too,
        // so each channel gets its own comparison (R12).
        let mut energy = [0u64; 3];
        for y in 0..h {
            for x in 1..w {
                for (c, e) in energy.iter_mut().enumerate() {
                    *e += (px[(x, y)][c] as i64 - px[(x - 1, y)][c] as i64).unsigned_abs();
                }
            }
        }
        ((w, h), energy)
    };
    let (dim_flat, e_flat) = export(0.0, "out/_export_sharp0.png");
    let (dim_sharp, e_sharp) = export(100.0, "out/_export_sharp100.png");
    assert_eq!(dim_flat, dim_sharp, "only the sharpen knob differs");
    for c in 0..3 {
        assert!(
            e_sharp[c] > e_flat[c],
            "output sharpening must raise edge energy on channel {c}: {e_sharp:?} vs {e_flat:?}"
        );
    }
}

/// R29 Batch-2 acceptance ①: WHAT `--long-edge` is, stated as an equality.
///
/// The CLI's new `--long-edge N` reaches [`ExportOpts::long_edge`], which
/// `render_to_file` applies as its LAST pixel stage — after the whole
/// develop, after output sharpening's own input is chosen, before the
/// encode. So the delivered file is exactly the full-resolution render
/// resampled, and the equality below says so with no tolerance at all:
/// `long_edge = k` is Lanczos3 downscale of the k-less render, to the
/// 16-bit code, at every k.
///
/// That is a REAL claim and not a tautology, because three plausible
/// implementations break it and one of them was on the table:
///
/// * resampling with a different kernel (`serve`'s preview arm uses
///   `Triangle`, `src/serve.rs:860`, while `decode::preview_resized` uses
///   `Lanczos3`, `src/decode.rs:2146` — the two spellings already in this
///   tree). The CLI flag inherits the EXPORT path's `Lanczos3`
///   (`src/render.rs:1564`) because it is that path; no third choice was
///   added.
/// * developing at the capped resolution instead of capping the developed
///   pixels — see acceptance ② below for how far apart those two are.
/// * doing the resize before output sharpening's measurement, or after the
///   colour-space transform.
///
/// MEASURED, on the 800×600 fixture below with sharpening 60 / clarity 40 /
/// texture 50: mean |Δ| = 0.000000 and worst |Δ| = 0 sixteen-bit codes at
/// both k = 400 and k = 200. Pinned at 0, since anything else means one of
/// the three above happened.
#[test]
fn export_at_size_is_exactly_a_downscale_of_the_full_render() {
    std::fs::create_dir_all("out").ok();
    let src_p = std::path::Path::new("out/_le_src.png");
    // Detail at all three scales the normalised operators work on: a
    // deterministic hash for pixel-scale grain (texture/sharpening), the
    // sinusoids for mid-band structure, the ramp for the tonal range
    // clarity's midtone mask needs. A flat fixture would satisfy this test
    // with every stage deleted.
    RgbImage::from_fn(800, 600, |x, y| {
        let hash = x.wrapping_mul(2_654_435_761u32).wrapping_add(y.wrapping_mul(40_503)) >> 13;
        let grain = (hash & 31) as f32 - 15.5;
        let mid = 40.0 * ((x as f32 / 9.0).sin() + (y as f32 / 7.0).cos());
        let ramp = 90.0 + 100.0 * x as f32 / 800.0;
        let v = |off: f32| (ramp + mid + grain + off).clamp(0.0, 255.0) as u8;
        Rgb([v(0.0), v(-8.0), v(12.0)])
    })
    .save(src_p)
    .unwrap();
    let recipe =
        EditRecipe { sharpening: 60.0, clarity: 40.0, texture: 50.0, ..Default::default() };

    let full_p = std::path::Path::new("out/_le_full.png");
    let (fw, fh) =
        render_to_file(src_p, &recipe, full_p, None, None, crate::diag::stderr()).unwrap();
    assert_eq!((fw, fh), (800, 600), "no export opts = the source's own resolution");
    // 16-bit PNG, so the round trip through the file is lossless and the
    // comparison below measures the RESIZE and nothing else.
    let full = image::open(full_p).unwrap();

    for k in [400u32, 200] {
        let opts = ExportOpts { long_edge: Some(k), ..Default::default() };
        let p = format!("out/_le_{k}.png");
        let (w, h) = render_to_file(
            src_p,
            &recipe,
            std::path::Path::new(&p),
            None,
            Some(&opts),
            crate::diag::stderr(),
        )
        .unwrap();
        assert_eq!(w.max(h), k, "the LONG edge is what the flag bounds");
        assert_eq!((w, h), (k, k * 3 / 4), "…and the aspect ratio is kept");
        let got = image::open(&p).unwrap().to_rgb16();
        let want = full.resize(k, k, image::imageops::FilterType::Lanczos3).to_rgb16();
        assert_eq!(got.dimensions(), want.dimensions());
        let worst = got
            .as_raw()
            .iter()
            .zip(want.as_raw())
            .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
            .max()
            .unwrap();
        assert_eq!(
            worst, 0,
            "long_edge {k} must BE the Lanczos3 downscale of the full render — \
             worst channel difference {worst} sixteen-bit codes"
        );
    }

    // `Some(0)` is FULL RESOLUTION, not "resize to nothing": the guard is
    // `le > 0` (this file, the `opts.long_edge` block), and the CLI folds
    // its own `--long-edge 0` to `None` on top of that so the two surfaces
    // cannot disagree. Pinned here because a `saturating`-flavoured
    // rewrite of that guard would produce a 1×1 deliverable in silence.
    let zero = ExportOpts { long_edge: Some(0), ..Default::default() };
    let z_p = std::path::Path::new("out/_le_zero.png");
    let (zw, zh) =
        render_to_file(src_p, &recipe, z_p, None, Some(&zero), crate::diag::stderr()).unwrap();
    assert_eq!((zw, zh), (800, 600), "long_edge 0 = full resolution");
    assert_eq!(
        image::open(z_p).unwrap().to_rgb16().as_raw(),
        full.to_rgb16().as_raw(),
        "long_edge 0 must not touch a single pixel"
    );
}

/// R29 Batch-2 acceptance ②: the R25 B2 RESOLUTION-NORMALISATION promise,
/// measured — and the boundary of what acceptance ① above buys.
///
/// The promise (`apply_develop` stages 3/3b/5, and the `texture_pass` doc)
/// is that clarity, texture and sharpening have radii expressed as a
/// FRACTION of the frame, so one slider value means the same structure on a
/// 1280 px preview and on a 61 MP export. Nothing in the tree measured it;
/// the promise lived in four comments.
///
/// This measures it where it is checkable: clarity's radius is 2 % of the
/// short edge (`src/render.rs:1833`), and a three-pass box blur of radius r
/// reaches 3r, so a step edge's halo must be 3 × 0.02 × short-edge px wide —
/// i.e. the SAME fraction of the frame at both resolutions. Doubling the
/// working resolution must double the halo in pixels.
///
/// MUTATION THIS KILLS: replacing the radius with any constant (the shape
/// the code had before R25 B2 — `unsharp_luma(data, w, h, 8, …)`), which
/// makes the ratio 1.0 instead of 2.0 while every other clarity test in
/// this file still passes, because they all work at ONE resolution.
///
/// **What this does NOT say, and it matters for `--long-edge`.** Acceptance
/// ① shows the export resize happens AFTER the develop, so `--long-edge`
/// never exercises this promise at all: the develop runs at full sensor
/// resolution and the resampler then averages its halos down with
/// everything else. Developing at the capped resolution instead is a
/// visibly different picture, and it is worth knowing by how much.
///
/// The five figures below are PROVENANCED but not gate-checked: they come
/// from a throwaway harness run once during R29 Batch-2 over the
/// acceptance-① fixture and functions named here, and no test re-derives
/// them, so treat them as a recorded observation rather than a pinned
/// quantity. `render_baked_to_image(max_edge = k)` differs from
/// the same-resampler downscale of the full develop by a mean 5.7 codes8 at
/// k = 400 and 19.8 codes8 at k = 200, against 19.1 / 30.0 codes8 for the
/// entire effect of that recipe (neutral vs graded at the same size), with
/// a resampler-only control of 0.25 codes8. That is not a defect in either
/// path — normalised operators are SUPPOSED to place their halos at the
/// working resolution's scale, so the two disagree by construction — but it
/// is exactly why the delivery flag resizes finished pixels instead of
/// quietly reusing the preview path.
#[test]
fn the_develop_radius_is_a_fraction_of_the_frame_not_a_pixel_count() {
    // Clarity alone: its radius is the largest of the three, so the halo is
    // the easiest to measure, and its midtone weight is ~0.75 at both
    // plateau levels below (m = 1 − (2l − 1)²), so neither side is starved.
    let recipe = EditRecipe { clarity: 60.0, ..Default::default() };
    // Halo width in PIXELS: walk left from the step and count how far the
    // overshoot is still visible against the far-field plateau.
    let halo = |w: usize, h: usize| -> usize {
        let mut data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| if i % w < w / 2 { [0.25; 3] } else { [0.75; 3] })
            .collect();
        apply_develop_anon(&mut data, w, h, &recipe);
        let row = h / 2;
        // The plateau as DEVELOPED (x = 0 is beyond any halo at these
        // radii), not the literal 0.25 — the tone stage is free to move it.
        let plateau = data[row * w][0];
        (0..w / 2)
            .rev()
            .take_while(|x| (data[row * w + x][0] - plateau).abs() > 1e-3)
            .count()
    };
    // 2 % of the short edge: 24 px at 1200, 12 px at 600 — both clear of
    // the 8 px floor, which would otherwise flatten the ratio by itself.
    let big = halo(1600, 1200);
    let small = halo(800, 600);
    assert!(big > 0 && small > 0, "clarity must produce a halo at all: {big} / {small}");
    // MEASURED: 59 px at 1600×1200, 30 px at 800×600 — ratio 1.967.
    let ratio = big as f64 / small as f64;
    assert!(
        (ratio - 2.0).abs() < 0.15,
        "clarity's radius must scale with the frame: halo {big} px at 1600×1200 vs \
         {small} px at 800×600 (ratio {ratio:.3}, expected 2.0 ± 0.15)"
    );
    // …and each halo really is the reach of a 2 %-of-short-edge blur, not
    // some other quantity that happens to double. Three box passes of
    // radius r reach 3r, so the ceiling is exact; the floor is 70 % of it
    // because the 1e-3 detection threshold above cuts the blur's thin outer
    // tail short (measured 59 of a possible 72, and 30 of 36 — the same
    // fraction at both sizes, which is itself the shape being preserved).
    // A constant radius fails this at 1600×1200 whatever it is set to.
    for (short, measured) in [(1200usize, big), (600, small)] {
        let radius = (0.02 * short as f64).round() as usize;
        let reach = 3 * radius;
        assert!(
            measured <= reach && measured * 10 >= reach * 7,
            "a {short} px short edge gives clarity a {radius} px radius, so the halo must \
             land in {}..={reach} px — measured {measured}",
            reach * 7 / 10
        );
    }
}

#[test]
fn kelvin_to_rgb_warm_is_redder_than_cool() {
    let warm = kelvin_to_rgb(3000.0);
    let cool = kelvin_to_rgb(9000.0);
    // STRICT: a kelvin_to_rgb that ignored its argument satisfied the
    // old >= / <= forms (R12).
    assert!(warm[0] > cool[0], "warm red {} > cool red {}", warm[0], cool[0]);
    assert!(warm[2] < cool[2], "warm blue {} < cool blue {}", warm[2], cool[2]);
}

/// The published Tanner-Helland constants have a cliff at the 6600 K
/// branch seam (green +1.31 %, blue +0.96 %) and a red plateau to 6688 K
/// where the r/b ratio — the eyedropper's temperature signal — does not
/// move at all. The recalibrated branches must be continuous at the seam
/// and alive inside the formerly dead band.
#[test]
fn the_6600k_branch_seam_is_continuous_and_carries_a_temperature_signal() {
    // C0 at the seam: 2 K apart may differ by slope (≈2e-4 per channel),
    // never by a branch cliff (the old green cliff alone was 1.3e-2).
    let below = kelvin_to_rgb(6599.0);
    let above = kelvin_to_rgb(6601.0);
    for c in 0..3 {
        assert!(
            (below[c] - above[c]).abs() < 2e-3,
            "channel {c} jumps across the seam: {} vs {}",
            below[c],
            above[c]
        );
    }
    // Inside 6600–6688 K the old red branch sat clamped at 255 while blue
    // was 255 by definition — r/b pinned at 1.0, so every temperature in
    // the band solved identically. The ratio must move now.
    let a = kelvin_to_rgb(6610.0);
    let b = kelvin_to_rgb(6680.0);
    let (ra, rb) = (a[0] / a[2], b[0] / b[2]);
    assert!(
        (ra - rb).abs() > 1e-3,
        "the r/b temperature signal is still dead in-band: {ra} vs {rb}"
    );
}

#[test]
fn wb_warmer_target_boosts_red_cuts_blue() {
    // Target warmer (higher K) than as-shot ⇒ Lightroom warms: red gain > 1, blue < 1.
    let g = wb_gains(5000.0, 7000.0, 0.0);
    assert!(g[0] > 1.0, "red gain {}", g[0]);
    assert!(g[2] < 1.0, "blue gain {}", g[2]);
    // Neutral (same K, no tint) ⇒ all gains ~1.
    let n = wb_gains(5500.0, 5500.0, 0.0);
    assert!((n[0] - 1.0).abs() < 1e-3 && (n[2] - 1.0).abs() < 1e-3);
}

#[test]
fn wb_eyedropper_neutralizes_a_synthetic_cast() {
    // Build the pixel a grey card shows under a known wrong WB: linear grey
    // L divided by the gains a (k0, tint0) correction WOULD apply — so that
    // correction is exactly what neutralises it. The solver must recover a
    // (k, tint) whose gains bring the pixel back to r≈g≈b, judged by the
    // same forward model (parameter identity is NOT required — nearby K
    // can neutralise equally well; neutrality is the contract).
    for (k0, tint0) in [(3200.0f32, 12.0f32), (7500.0, -18.0), (5500.0, 0.0)] {
        let g0 = wb_gains(5500.0, k0, tint0);
        let l = 0.18f32;
        let cast = [
            linear_to_srgb(l / g0[0]),
            linear_to_srgb(l / g0[1]),
            linear_to_srgb(l / g0[2]),
        ];
        let (k, tint) = solve_wb_from_neutral(cast, 5500.0);
        let g = wb_gains(5500.0, k, tint);
        let out = [
            srgb_to_linear(cast[0]) * g[0],
            srgb_to_linear(cast[1]) * g[1],
            srgb_to_linear(cast[2]) * g[2],
        ];
        let (mx, mn) = (
            out[0].max(out[1]).max(out[2]),
            out[0].min(out[1]).min(out[2]),
        );
        assert!(
            (mx - mn) / mx < 0.02,
            "cast for ({k0},{tint0}) not neutralised: solved ({k:.0},{tint:.1}) → {out:?}"
        );
    }
    // An already-neutral pixel solves to ~as-shot, ~zero tint.
    let (k, tint) = solve_wb_from_neutral([0.5, 0.5, 0.5], 5500.0);
    assert!((k - 5500.0).abs() < 300.0 && tint.abs() < 2.0, "neutral → ({k:.0},{tint:.1})");
}

#[test]
fn as_shot_math_lands_on_canonical_illuminants() {
    // Identity camera matrix ⇒ camera space IS XYZ; the WB gains
    // neutralise the illuminant, so wb = 1/XYZ reconstructs it exactly.
    let id = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    // D65 (X 0.95047, Y 1, Z 1.08883): McCamy ⇒ ~6504 K, and D65 sits
    // Duv ≈ +0.0032 ABOVE the Planckian locus ⇒ tint ≈ +10 — ACR's own
    // Daylight-preset tint, which pins the sign convention AND the scale.
    let (k, t) = wb_to_kelvin_tint(&id, [1.0 / 0.95047, 1.0, 1.0 / 1.08883]).unwrap();
    assert!((6400.0..=6600.0).contains(&k), "D65 → {k:.0} K");
    assert!((5.0..=15.0).contains(&t), "D65 → tint {t:+.1}");
    // Illuminant A (X 1.09850, Z 0.35585) lies ON the locus: ~2856 K, ~0.
    let (k, t) = wb_to_kelvin_tint(&id, [1.0 / 1.0985, 1.0, 1.0 / 0.35585]).unwrap();
    assert!((2790.0..=2920.0).contains(&k), "A → {k:.0} K");
    assert!(t.abs() < 6.0, "A → tint {t:+.1}");
    // Damaged coefficients refuse instead of anchoring nonsense.
    assert_eq!(wb_to_kelvin_tint(&id, [f32::NAN, 1.0, 1.0]), None);
    assert_eq!(wb_to_kelvin_tint(&id, [0.0, 1.0, 1.0]), None);
}

#[test]
fn stamped_as_shot_anchor_moves_only_kelvin_edits() {
    // tint-only renders identically stamped or legacy — the tint gain
    // never depends on the anchor — so no old archive can move.
    let grey = [[0.5f32, 0.5, 0.5]; 4];
    let mut legacy_px = grey;
    apply_recipe_wb(&mut legacy_px, &EditRecipe { tint: 20.0, ..Default::default() });
    let mut stamped_px = grey;
    apply_recipe_wb(
        &mut stamped_px,
        &EditRecipe { tint: 20.0, as_shot_k: Some(4000.0), ..Default::default() },
    );
    assert_eq!(legacy_px, stamped_px, "tint-only must ignore the anchor");
    // An ABSOLUTE target equal to the stamped as-shot is a true no-op —
    // the honest semantic the 5500-anchored model could not express…
    let mut at_as_shot = grey;
    apply_recipe_wb(
        &mut at_as_shot,
        &EditRecipe {
            temperature_k: Some(4000.0),
            as_shot_k: Some(4000.0),
            ..Default::default()
        },
    );
    assert_eq!(at_as_shot, grey, "target == as-shot must not shift");
    // …while a LEGACY recipe with the same numbers still takes its tuned
    // 5500-anchored shift, byte-identical to the old engine.
    let mut legacy_shift = grey;
    apply_recipe_wb(
        &mut legacy_shift,
        &EditRecipe { temperature_k: Some(4000.0), ..Default::default() },
    );
    assert_ne!(legacy_shift, grey, "legacy 5500-anchored shift still applies");
}
