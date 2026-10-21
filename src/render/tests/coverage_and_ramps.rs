// One part of the engine's tests (src/render/tests.rs includes it): rotation peaks, thumbnail binning, downscaling, mask coverage, the linear ramps and the byte-stable baselines.

/// Real-machine probe, never run in CI (it allocates gigabytes): the
/// PORTRAIT branch's transient on a 61 MP frame — the one orientation
/// state that was unreachable for an ARW until v0.30.0, so its cost had
/// never actually been paid on a Sony file.
///
/// `orient_f32` casts to and from `Rgb32F` with zero copies
/// (`bytemuck::cast_vec`), so the whole transient is `oriented`'s own
/// `rotate270`: one fresh output frame while the source is still alive.
/// 61 MP x 3 channels x 4 bytes = 732 MB per frame, so the accounting
/// predicts a ~1.46 GB peak and no third copy. Measure with:
///
/// ```text
/// cargo test --lib -- --ignored --exact render::tests::portrait_rotation_peak_on_a_61mp_frame
/// ```
///
/// and read the printed before/after RSS, or wrap the test binary in
/// `Start-Process -PassThru -Wait` and read `PeakWorkingSet64`.
#[test]
#[ignore = "real-machine probe: allocates ~1.5 GB"]
fn portrait_rotation_peak_on_a_61mp_frame() {
    // 9504 x 6336 = the A7R IV sensor, landscape; Rotate270 turns it
    // portrait, which is exactly what an orientation-8 ARW now does.
    let (w, h) = (9504usize, 6336usize);
    let px = w * h;
    let frame_bytes = px * 3 * 4;
    let data: Vec<[f32; 3]> = vec![[0.5, 0.5, 0.5]; px];
    eprintln!(
        "one frame = {:.0} MB ({px} px); predicted peak = {:.0} MB (source + rotated copy)",
        frame_bytes as f64 / 1e6,
        2.0 * frame_bytes as f64 / 1e6
    );
    let (out, ow, oh) = orient_f32(data, w, h, Orientation::Rotate270);
    assert_eq!((ow, oh), (h, w), "the frame must come back portrait");
    assert_eq!(out.len(), px);
    assert_eq!(out.capacity() * 12, frame_bytes, "no third copy was made");
}

#[test]
fn thumbnail_binning_commutes_only_with_non_reversing_orientations() {
    // The probe behind the A7 decision to keep the working-resolution
    // cap AFTER orientation (see `render_to_image_in`) — RE-RUN with the
    // type-exact flip after the eight-state test exposed that the first
    // measurement's Transpose figure (0.48) was entirely the Rgba<u8>
    // flip adapter's quantization (U14). The true shape: `thumbnail`'s
    // integer binning uses forward bin edges, so it commutes EXACTLY
    // with pure axis swaps (Normal, Transpose) and diverges by one
    // source bin (≈1/97 on this gradient) under every orientation with
    // a REVERSAL component — mirrored bin edges of a non-integer ratio
    // don't line up. Cap-before-orientation therefore STAYS forbidden
    // (six of eight states would change preview pixels). The exact arm
    // also pins the type-exact flip: a quantizing Transpose flip shows
    // up here as ~0.5. If the reversing arm ever FAILS, the crate's
    // binning became mirror-symmetric — re-probe all eight states
    // before touching the pipeline order.
    let (w, h) = (97usize, 61usize);
    let data: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            [x as f32 / w as f32, y as f32 / h as f32, 0.0]
        })
        .collect();
    let diff = |o: Orientation| -> f32 {
        let (a, aw, ah) = {
            let (d, dw, dh) = orient_f32(data.clone(), w, h, o);
            downscale_f32(d, dw, dh, 40)
        };
        let (b, bw, bh) = {
            let (d, dw, dh) = downscale_f32(data.clone(), w, h, 40);
            orient_f32(d, dw, dh, o)
        };
        assert_eq!((aw, ah), (bw, bh), "{o:?}: dims always agree");
        a.iter()
            .zip(&b)
            .flat_map(|(p, q)| (0..3).map(move |c| (p[c] - q[c]).abs()))
            .fold(0.0f32, f32::max)
    };
    // Normal is excluded: orient_f32 is a passthrough there, so both
    // sides of the comparison would be the SAME call on equal inputs —
    // an assertion that cannot fail (R12). Transpose is the real
    // content: a pure axis swap that must commute with the binning, and
    // the arm that catches a quantizing flip (~0.5 here).
    let d = diff(Orientation::Transpose);
    assert!(d <= 1e-6, "Transpose must commute exactly, diff {d}");
    for o in [
        Orientation::HorizontalFlip,
        Orientation::VerticalFlip,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
        Orientation::Transverse,
    ] {
        let d = diff(o);
        assert!(
            d > 1e-4,
            "{o:?}: binning became mirror-symmetric (diff {d}) — re-probe \
             all eight states before considering cap-before-orientation"
        );
    }
}

#[test]
fn downscale_f32_is_an_unbiased_average() {
    // The working-resolution cap must not shift LEVELS. A flat field must
    // survive exactly, and a gradient's mean must be preserved: every GUI
    // preview, every retouch base and the camera-base-curve estimation all
    // run through this path, so a per-pixel offset here would wash out the
    // whole application (R12).
    let (w, h) = (97usize, 61usize);
    let flat: Vec<[f32; 3]> = vec![[0.25, 0.5, 0.75]; w * h];
    let (small, sw, sh) = downscale_f32(flat, w, h, 40);
    assert!(sw <= 40 && sh <= 40, "capped to {sw}x{sh}");
    for p in &small {
        for (c, want) in [0.25f32, 0.5, 0.75].iter().enumerate() {
            assert!(
                (p[c] - want).abs() < 1e-4,
                "flat field must survive the cap unchanged: {p:?}"
            );
        }
    }
    let ramp: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let v = (i % w) as f32 / (w - 1) as f32;
            [v; 3]
        })
        .collect();
    let mean_in = ramp.iter().map(|p| p[0] as f64).sum::<f64>() / (w * h) as f64;
    let (small, sw, sh) = downscale_f32(ramp, w, h, 40);
    let mean_out = small.iter().map(|p| p[0] as f64).sum::<f64>() / (sw * sh) as f64;
    assert!(
        (mean_out - mean_in).abs() < 0.02,
        "the cap must preserve the mean level: {mean_in} -> {mean_out}"
    );
    // AVERAGING, not merely level. Both arms above pass for a
    // nearest-neighbour sampler (measured: top-left 0.0023 and
    // bottom-right 0.0125 off the mean, inside the 0.02 budget, and a
    // flat field survives point sampling exactly) — so "optimising" the
    // inner loop into `*px = data[bottom * w + left]` would keep the
    // suite green while every preview started aliasing.
    //
    // One lit source pixel in a dark field: a box filter spreads it over
    // its whole window, so the output lands at 1/window_area — never at 0
    // (a point sampler that missed it) and never at 1 (one that hit it).
    let mut spike = vec![[0.0f32; 3]; w * h];
    spike[(h / 2) * w + w / 2] = [1.0; 3];
    let (small, _sw, _sh) = downscale_f32(spike, w, h, 40);
    let peak = small.iter().map(|p| p[0]).fold(0.0f32, f32::max);
    let lit = small.iter().filter(|p| p[0] > 0.0).count();
    assert_eq!(lit, 1, "exactly one output window contains the lit pixel");
    // Bin windows are ceil-based, so this fixture (97x61 -> 40x25, ratios
    // 2.425 and 2.44) gives each output pixel 2 or 3 source columns and
    // rows: the lit sample is therefore averaged over 4..=9 of them.
    assert!(
        (1.0 / 9.0..=1.0 / 4.0).contains(&peak),
        "one lit pixel must be AVERAGED over its 4..=9 sample window, got {peak}"
    );
}

#[test]
fn mask_coverage_reports_the_engine_weight() {
    use crate::recipe::{LocalAdjustment, MaskGeometry, RangeMask};
    // (a) A top→bottom linear gradient over a flat grey reference: zero at
    // the top row, ~full at the bottom, ~half in the middle.
    let grey = DynamicImage::ImageRgb8(RgbImage::from_pixel(20, 20, image::Rgb([120, 120, 120])));
    let grad = LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 },
        ..Default::default()
    };
    let cov = mask_coverage(&grad, &grey, MaskFrame::AsRendered);
    // Rows sample at their CENTRES, ny = (y + 0.5)/20, so this 0→1
    // gradient reads t = 0.025 / 0.525 / 0.975 at rows 0 / 10 / 19; the
    // shipped `Measured` profile maps those to 0.00075 / 0.47703 / 0.99768
    // and quantises to `round(w · 255)` = 0 / 122 / 254 (Clamped gave
    // 6 / 134 / 249; the unwarped smoothstep gave 0 / 137 / 255). Pinned
    // EXACTLY, both because the arithmetic is exact and because rows 10
    // and 19 still separate the conventions: `y/h` would give
    // t = 0.0 / 0.5 / 0.95 → 0 / 112 / 249. Under Clamped the old
    // `assert_eq!(…, 0)` on row 0 was the whole reason this test caught
    // R29 C2 rather than sleeping through it; under any eased profile row
    // 0 rounds to 0 either way, so rows 10 and 19 carry that duty now. Row
    // 19 reading 254 rather than 255 IS the warp: the last sample sits at
    // t = 0.975, and smoothstep(0.975^1.124) = 0.99768 is under 254.5/255.
    assert_eq!(cov.get_pixel(10, 0)[0], 0, "the zero end is flat at the handle");
    assert_eq!(cov.get_pixel(10, 19)[0], 254, "the full end reaches its plateau");
    assert_eq!(cov.get_pixel(10, 10)[0], 122, "the midpoint sits off the linear centre");

    // (b) amount halves the whole map; inversion flips its direction.
    let half = LocalAdjustment { amount: 0.5, ..grad.clone() };
    assert!((mask_coverage(&half, &grey, MaskFrame::AsRendered).get_pixel(10, 19)[0] as i32 - 128).abs() < 15);
    let inv = LocalAdjustment { inverted: true, ..grad.clone() };
    let icov = mask_coverage(&inv, &grey, MaskFrame::AsRendered);
    assert!(icov.get_pixel(10, 0)[0] > 235 && icov.get_pixel(10, 19)[0] < 20);

    // (c) A luminance range gates the map by the REFERENCE pixels: with a
    // bright-only range, the dark half of the reference reads 0 even where
    // the geometry is at full strength.
    let split = DynamicImage::ImageRgb8(RgbImage::from_fn(20, 20, |x, _| {
        if x < 10 { image::Rgb([30, 30, 30]) } else { image::Rgb([220, 220, 220]) }
    }));
    let ranged = LocalAdjustment {
        // Degenerate linear (zero == full) = weight 1 everywhere.
        mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
        range: Some(RangeMask::Luminance { lo_outer: 0.5, lo: 0.6, hi: 1.0, hi_outer: 1.0 }),
        ..Default::default()
    };
    let rcov = mask_coverage(&ranged, &split, MaskFrame::AsRendered);
    assert_eq!(rcov.get_pixel(3, 10)[0], 0, "dark side gated out");
    assert!(rcov.get_pixel(16, 10)[0] > 235, "bright side kept: {}", rcov.get_pixel(16, 10)[0]);
}

#[test]
fn angled_linear_mask_matches_the_pixel_metric_closed_form() {
    let g = MaskGeometry::Linear {
        zero_x: 0.15,
        zero_y: 0.20,
        full_x: 0.85,
        full_y: 0.80,
    };
    let (w, h) = (300.0f32, 200.0f32);
    let (zx, zy, fx, fy) = match &g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => (*zero_x, *zero_y, *full_x, *full_y),
        _ => unreachable!(),
    };
    let (vx, vy) = (fx - zx, fy - zy);
    let (px, py) = (vx * w, vy * h);
    let den = px * px + py * py;
    for (nx, ny) in [(0.30, 0.25), (0.55, 0.50), (0.75, 0.65)] {
        let dx = (nx - zx) * w;
        let dy = (ny - zy) * h;
        let want = linear_coverage((dx * px + dy * py) / den, LINEAR_FALLOFF);
        let got = mask_weight_in(&g, nx, ny, None, None, (w, h));
        assert!((got - want).abs() < 1e-6, "({nx},{ny}): got {got}, want {want}");
        let normalized = mask_weight(&g, nx, ny, None);
        assert!((got - normalized).abs() > 1e-3, "({nx},{ny}) did not expose aspect skew");
    }
}

#[test]
fn axis_aligned_linear_coverage_is_byte_stable() {
    let (w, h) = (13u32, 9u32);
    let g = MaskGeometry::Linear {
        zero_x: 0.4,
        zero_y: 0.1,
        full_x: 0.4,
        full_y: 0.9,
    };
    let m = LocalAdjustment { mask: g.clone(), ..Default::default() };
    let reference = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([120, 120, 120])));
    let got = mask_coverage(&m, &reference, MaskFrame::AsRendered);
    let mut want = image::GrayImage::new(w, h);
    for (x, y, px) in want.enumerate_pixels_mut() {
        let weight = mask_weight(
            &g,
            (x as f32 + MASK_SAMPLE_CENTRE) / w as f32,
            (y as f32 + MASK_SAMPLE_CENTRE) / h as f32,
            None,
        );
        *px = image::Luma([(weight * 255.0).round() as u8]);
    }
    assert_eq!(got.as_raw(), want.as_raw(), "axis-aligned coverage changed byte-for-byte");
}

#[test]
fn linear_coverage_clamped_is_byte_identical_to_head() {
    // Keep the pre-refactor ramp expression in the test itself. This is a
    // code snapshot, rather than a file fixture that could be regenerated
    // with the new implementation by mistake.
    for i in 0..=4096u32 {
        let t = (i as f32 - 512.0) / 3072.0;
        let head = t.clamp(0.0, 1.0);
        let got = linear_coverage(t, LinearFalloff::Clamped);
        // Exact f32 identity on purpose: the 16-bit form of this check
        // went green under hand mutation M-L2 (2026-08-28, the [0,1] clamp
        // removed) because a saturating integer cast turns negative and
        // above-one coverage into the same 0 / 65535 the head ramp gives.
        assert_eq!(got.to_bits(), head.to_bits(), "clamped coverage changed at t={t}");
    }
}

#[test]
fn linear_coverage_eased_is_c1_at_both_ends() {
    let n = 2000usize;
    let values: Vec<u16> = (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            (linear_coverage(t, LinearFalloff::Eased) * 65535.0).round() as u16
        })
        .collect();
    let slopes: Vec<i32> = values.windows(2).map(|p| p[1] as i32 - p[0] as i32).collect();
    let through: f32 = slopes[400..1600].iter().map(|&v| v as f32).sum::<f32>() / 1200.0;
    assert!(through > 1.0, "the 16-bit ramp must have a measurable slope");
    let turnover = |part: &[i32]| {
        part.iter()
            .take(80)
            .take_while(|&&s| (s as f32 - through).abs() >= 0.25 * through)
            .count()
    };
    let max_edge_jump = |part: &[i32]| {
        part.windows(2).take(80).map(|p| (p[1] - p[0]).abs()).max().unwrap_or(0)
    };
    assert!(max_edge_jump(&slopes) <= 2, "eased full-end first difference is discontinuous");
    assert!(max_edge_jump(&slopes[slopes.len() - 81..]) <= 2, "eased zero-end first difference is discontinuous");
    assert!(turnover(&slopes) > 2, "eased full end turns over in one row");
    assert!(turnover(&slopes[slopes.len() - 80..]) > 2, "eased zero end turns over in one row");
    assert_eq!(linear_coverage(0.0, LinearFalloff::Eased), 0.0);
    assert_eq!(linear_coverage(1.0, LinearFalloff::Eased), 1.0);
}

#[test]
fn linear_coverage_profiles_agree_at_the_handles() {
    for &t in &[0.0, 1.0] {
        assert_eq!(linear_coverage(t, LinearFalloff::Clamped), linear_coverage(t, LinearFalloff::Eased));
    }
}

#[test]
fn shipped_linear_falloff_is_eased() {
    // The shipped arm is `Measured` — Hermite smoothstep on the measured
    // abscissa t^LINEAR_FALLOFF_WARP (me6-2026-09 group B, 12 gradients).
    assert_eq!(LINEAR_FALLOFF, LinearFalloff::Measured);
    // …and it is still an EASING, which is what this test's name buys:
    // C1 at both handles, strictly monotone in between, and equal to the
    // other two profiles exactly at t = 0 and t = 1. A mutation pointing
    // LINEAR_FALLOFF at Clamped fails the slope pair as well as the
    // assert_eq above.
    for &t in &[0.0f32, 1.0] {
        assert_eq!(linear_coverage(t, LINEAR_FALLOFF), linear_coverage(t, LinearFalloff::Clamped));
    }
    let d = 1e-3;
    let slope = |t: f32| (linear_coverage(t + d, LINEAR_FALLOFF) - linear_coverage(t, LINEAR_FALLOFF)) / d;
    assert!(slope(0.0) < 0.02, "zero handle is not C1: {}", slope(0.0));
    assert!(slope(1.0 - d) < 0.02, "full handle is not C1: {}", slope(1.0 - d));
    let mut prev = -1.0f32;
    for i in 0..=1000 {
        let got = linear_coverage(i as f32 / 1000.0, LINEAR_FALLOFF);
        assert!(got > prev, "not monotone at t = {}", i as f32 / 1000.0);
        prev = got;
    }
}

#[test]
fn a_shaped_gradient_reads_its_own_falloff_along_the_handles() {
    let falloff = vec![0.0, 0.1, 0.5, 1.0];
    let g = MaskGeometry::Shaped { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0, falloff: falloff.clone() };
    // At the samples the table is read exactly; between them, linearly.
    assert_eq!(mask_weight(&g, 0.2, 0.0, None), 0.0);
    assert!((mask_weight(&g, 0.2, 1.0 / 3.0, None) - 0.1).abs() < 1e-5);
    assert!((mask_weight(&g, 0.2, 0.5, None) - 0.3).abs() < 1e-5);
    assert_eq!(mask_weight(&g, 0.2, 1.0, None), 1.0);
    assert_eq!(mask_weight(&g, 0.2, 1.5, None), 1.0, "past the Full handle the last entry holds");
    assert_eq!(mask_weight(&g, 0.2, -0.5, None), 0.0, "before the Zero handle the first entry holds");
    // An empty table is the measured linear law, and the metric form agrees
    // with the plain one on an axis-aligned gradient as it does for Linear.
    let plain = MaskGeometry::Shaped { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0, falloff: Vec::new() };
    let linear = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 };
    for i in 0..=10 {
        let y = i as f32 / 10.0;
        assert_eq!(mask_weight(&plain, 0.5, y, None), mask_weight(&linear, 0.5, y, None));
        assert_eq!(mask_weight_in(&g, 0.5, y, None, None, (300.0, 200.0)), mask_weight(&g, 0.5, y, None));
    }
    // It survives the recipe's JSON round trip, and an entry outside 0..=1
    // is clamped to a coverage.
    let json = serde_json::to_string(&g).unwrap();
    assert!(json.contains("\"kind\":\"shaped\""), "{json}");
    assert_eq!(serde_json::from_str::<MaskGeometry>(&json).unwrap(), g);
    let mut r = EditRecipe::default();
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Shaped { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0, falloff: vec![-1.0, 0.5, 2.0] },
        ..Default::default()
    });
    let clean = crate::recipe::ValidatedRecipe::new(&r);
    let MaskGeometry::Shaped { falloff, .. } = &clean.masks[0].mask else { panic!("the geometry survives") };
    assert_eq!(falloff, &vec![0.0, 0.5, 1.0]);
}

#[test]
fn linear_ramp_has_a_single_definition() {
    let src = include_str!("../mask_weight.rs");
    let metric = &src[src.find("fn mask_weight_metric").unwrap()..src.find("/// Mask coverage").unwrap()];
    let weight = &src[src.find("fn mask_weight(g:").unwrap()..src.find("fn combined_mask_weight").unwrap()];
    let metric_linear = &metric[metric.find("MaskGeometry::Linear").unwrap()..metric.find("_ => mask_weight").unwrap()];
    let weight_linear = &weight[weight.find("MaskGeometry::Linear").unwrap()..weight.find("// `roundness`").unwrap()];
    assert_eq!(metric_linear.matches("linear_coverage(").count(), 2, "metric has a second ramp definition");
    assert_eq!(weight_linear.matches("linear_coverage(").count(), 1, "weight has a second ramp definition");
    assert!(!metric_linear.contains(".clamp(0.0, 1.0)"), "metric keeps an inline linear clamp");
    assert!(!weight_linear.contains(".clamp(0.0, 1.0)"), "weight keeps an inline linear clamp");
}

#[test]
fn radial_mask_renders_byte_identical_to_the_clamped_baseline() {
    let (w, h) = (48u32, 32u32);
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x * 3 + y * 2) as u8, (x * 5) as u8, (y * 7) as u8])
    }));
    let mask = crate::recipe::LocalAdjustment {
        mask: MaskGeometry::Radial { top: 0.1, left: 0.1, bottom: 0.9, right: 0.9, feather: 0.4, roundness: 0.0, flipped: false, angle: 0.0, midpoint: 50.0, mask_version: 2 },
        ..Default::default()
    };
    let got = mask_coverage(&mask, &src, MaskFrame::AsRendered);
    let want = image::GrayImage::from_fn(w, h, |x, y| {
        let nx = (x as f32 + MASK_SAMPLE_CENTRE) / w as f32;
        let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
        let mut weight = mask_weight(&mask.mask, nx, ny, None);
        if mask.inverted {
            weight = 1.0 - weight;
        }
        image::Luma([(weight * 255.0).round() as u8])
    });
    assert_eq!(got.as_raw(), want.as_raw(), "mask coverage changed from the clamped baseline");
}

#[test]
fn bitmap_mask_renders_byte_identical_to_the_clamped_baseline() {
    let (w, h) = (48u32, 32u32);
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x * 3 + y * 2) as u8, (x * 5) as u8, (y * 7) as u8])
    }));
    // Own directory, not the bare temp root (see `fixture_mask_path`).
    let raster_dir =
        std::env::temp_dir().join(format!("autoshade-linear-baseline-{}", std::process::id()));
    std::fs::create_dir_all(&raster_dir).unwrap();
    let raster_path = raster_dir.join("mask.png");
    let raster = image::GrayImage::from_fn(7, 5, |x, y| image::Luma([((x + y) * 20) as u8]));
    raster.save(&raster_path).unwrap();
    let mask = crate::recipe::LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: raster_path.to_string_lossy().into_owned() },
        ..Default::default()
    };
    let got = mask_coverage(&mask, &src, MaskFrame::AsRendered);
    let bmp = load_mask_bitmap(&mask.mask, &crate::diag::dropped());
    let want = image::GrayImage::from_fn(w, h, |x, y| {
        let nx = (x as f32 + MASK_SAMPLE_CENTRE) / w as f32;
        let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
        let mut weight = mask_weight(&mask.mask, nx, ny, bmp.as_deref());
        if mask.inverted {
            weight = 1.0 - weight;
        }
        image::Luma([(weight * 255.0).round() as u8])
    });
    assert_eq!(got.as_raw(), want.as_raw(), "mask coverage changed from the clamped baseline");
    let _ = std::fs::remove_file(raster_path);
}

#[test]
fn linear_mask_renders_the_eased_ramp() {
    let (w, h) = (48u32, 32u32);
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x * 3 + y * 2) as u8, (x * 5) as u8, (y * 7) as u8])
    }));
    let mask = crate::recipe::LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 1.0, full_x: 0.5, full_y: 0.0 },
        ..Default::default()
    };
    let got = mask_coverage(&mask, &src, MaskFrame::AsRendered);
    let want = image::GrayImage::from_fn(w, h, |x, y| {
        let nx = (x as f32 + MASK_SAMPLE_CENTRE) / w as f32;
        let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
        let (zero_x, zero_y, full_x, full_y) = (0.5, 1.0, 0.5, 0.0);
        let t = (((nx - zero_x) * (full_x - zero_x) + (ny - zero_y) * (full_y - zero_y))
            / ((full_x - zero_x).powi(2) + (full_y - zero_y).powi(2)))
            .clamp(0.0, 1.0);
        image::Luma([(linear_coverage(t, LINEAR_FALLOFF) * 255.0).round() as u8])
    });
    assert_eq!(got.as_raw(), want.as_raw(), "linear mask coverage did not use the shipped ramp");

    let byte = |value: f32| (value * 255.0).round() as u8;
    assert_eq!(linear_coverage(0.25, LinearFalloff::Eased), 0.15625);
    assert_eq!(linear_coverage(0.25, LinearFalloff::Clamped), 0.25);
    assert_ne!(byte(linear_coverage(0.25, LinearFalloff::Eased)), byte(linear_coverage(0.25, LinearFalloff::Clamped)));
    for t in [-1.0, 0.0, 1.0, 2.0] {
        assert_eq!(linear_coverage(t, LinearFalloff::Eased), linear_coverage(t, LinearFalloff::Clamped));
    }
}

#[test]
fn shipped_linear_ramp_is_eased_end_to_end() {
    // The probe geometry (vertical gradient, zero at 0.80, full at 0.35,
    // −2 EV) through the public f32 develop path. The tone pass blends
    // `p·(1 − w) + t·w`, so on a flat grey the mask weight is recovered
    // EXACTLY as `(base − row) / (base − full_plateau)` — no assumption
    // about the exposure model, and no 8-bit quantisation. The expected
    // profile is the LITERAL Hermite smoothstep on the LITERAL measured
    // abscissa, not `linear_coverage` and not LINEAR_FALLOFF_WARP, so a
    // mutation of the shipped arm cannot rewrite its own oracle.
    let (w, h) = (4usize, 1000usize);
    let base = 0.5_f32;
    let recipe = EditRecipe {
        masks: vec![crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.80, full_x: 0.5, full_y: 0.35 },
            exposure_ev: -2.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut px = vec![[base; 3]; w * h];
    apply_develop_anon(&mut px, w, h, &recipe);
    let rows: Vec<f32> = (0..h).map(|y| px[y * w + w / 2][1]).collect();
    let full_plateau = rows[100];
    assert!(full_plateau < base - 0.2, "full end must darken by −2 EV: {full_plateau}");
    assert_eq!(rows[950].to_bits(), base.to_bits(), "zero end must be untouched");
    let coverage: Vec<f32> = rows.iter().map(|r| (base - r) / (base - full_plateau)).collect();
    let t_of = |y: usize| {
        let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
        ((0.80 - ny) / (0.80 - 0.35)).clamp(0.0, 1.0)
    };
    // (a) every row IS the literal warped smoothstep of its projected t
    // (the 0.001 work floor and the tone LUT leave ≤ 2e-3 of slack).
    for (y, &got) in coverage.iter().enumerate() {
        let t = t_of(y);
        let u = t.powf(1.124);
        let want = u * u * (3.0 - 2.0 * u);
        assert!((got - want).abs() < 2e-3, "row {y}: t {t:.4} rendered {got:.5}, warped smoothstep {want:.5}");
    }
    // (b) C1 at BOTH handles: the coverage slope over the ten rows just
    // inside each handle is ≤ 0.1 of the mid-ramp slope (Clamped: ≈ 1.0;
    // `t²`: ≈ 0.0 at the zero end but ≈ 2.0 at the full end).
    let full_row = (0.35 * h as f32) as usize;
    let zero_row = (0.80 * h as f32) as usize;
    let mid = (full_row + zero_row) / 2;
    let slope = |a: usize, b: usize| ((coverage[b] - coverage[a]) / (b - a) as f32).abs();
    let mid_slope = slope(mid - 5, mid + 5);
    let full_end = slope(full_row + 1, full_row + 11);
    let zero_end = slope(zero_row - 11, zero_row - 1);
    assert!(full_end < 0.1 * mid_slope, "full-end slope {full_end:.2e} is not eased vs mid {mid_slope:.2e}");
    assert!(zero_end < 0.1 * mid_slope, "zero-end slope {zero_end:.2e} is not eased vs mid {mid_slope:.2e}");
    // (c) the shipped midpoint is 1.535× the linear slope of the same
    // span: 1.5 from the smoothstep, × 1.0314 from d(t^1.124)/dt at t = ½.
    let linear_slope = 1.0 / (zero_row - full_row) as f32;
    let ratio = mid_slope / linear_slope;
    assert!((1.52..=1.55).contains(&ratio), "mid-ramp slope ratio {ratio:.4} is not ~1.535");
}

#[test]
fn probe_fixture_round_trips_through_xmp() {
    let recipe = EditRecipe {
        masks: vec![crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.80, full_x: 0.5, full_y: 0.35 },
            exposure_ev: -2.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let xmp = crate::xmp::recipe_to_xmp(&recipe);
    let back = crate::xmp::xmp_to_recipe(&xmp);
    assert_eq!(back.masks.len(), 1);
    let MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } = back.masks[0].mask else { panic!("probe mask was not linear") };
    assert!((zero_x - 0.5).abs() < 1e-6 && (zero_y - 0.80).abs() < 1e-6);
    assert!((full_x - 0.5).abs() < 1e-6 && (full_y - 0.35).abs() < 1e-6);
    assert_eq!(back.masks[0].exposure_ev, -2.0);
    if crate::config::live_env_os("AUTOSHADE_GENERATE_LINEAR_PROBE").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/linear-falloff/probe");
        std::fs::create_dir_all(&dir).unwrap();
        let encoded = (linear_to_srgb(0.18) * 65535.0).round() as u16;
        let image = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_pixel(3000, 2000, image::Rgb([encoded; 3]));
        image::DynamicImage::ImageRgb16(image).save(dir.join("probe.tif")).unwrap();
        std::fs::write(dir.join("probe.xmp"), xmp.as_bytes()).unwrap();
        std::fs::write(dir.join("probe-recipe.json"), serde_json::to_vec_pretty(&back).unwrap()).unwrap();
    }
}

#[test]
fn gui_coverage_overlay_matches_an_angled_linear_render_weight() {
    let (w, h) = (96u32, 64u32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255])));
    let adj = LocalAdjustment {
        mask: MaskGeometry::Linear {
            zero_x: 0.08,
            zero_y: 0.15,
            full_x: 0.92,
            full_y: 0.85,
        },
        exposure_ev: -4.0,
        ..Default::default()
    };
    let recipe = EditRecipe { masks: vec![adj.clone()], ..Default::default() };
    let coverage = mask_coverage(&adj, &base, MaskFrame::AsRendered);
    let rendered =
        develop_preview_framed(&base, &recipe, &crate::diag::pixels(), MaskFrame::AsRendered, None, false)
            .to_rgb8();
    let (mut claimed, mut agreed, mut clear, mut clean) = (0u32, 0u32, 0u32, 0u32);
    for (x, y, p) in coverage.enumerate_pixels() {
        let lit = rendered.get_pixel(x, y).0[1];
        if p[0] > 200 {
            claimed += 1;
            if lit < 160 {
                agreed += 1;
            }
        } else if p[0] < 20 {
            clear += 1;
            if lit > 200 {
                clean += 1;
            }
        }
    }
    assert!(claimed > 500 && clear > 200, "premise: {claimed} covered / {clear} clear px");
    assert!(agreed * 100 >= claimed * 97, "angled overlay coverage disagrees: {agreed}/{claimed}");
    assert!(clean * 100 >= clear * 97, "angled overlay shows a clean pixel as covered: {clean}/{clear}");
}
