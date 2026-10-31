// One part of the engine's tests (src/render/tests.rs includes it): the crop rule, atomic export, mask sampling, zero-size frames, grading, curves, region tones, calibration, black and white, point colours and tonal bands.

#[test]
fn the_crop_rectangle_is_one_rule_for_both_source_paths() {
    use crate::recipe::Crop;
    // `apply_crop` exists BECAUSE the RAW path and the baked path must
    // agree on the rectangle — but nothing pinned that rule, so the shared
    // helper's arithmetic was verified by reading only. Pin it three ways.
    //
    // (a) The exact rectangle. Origin from left/top, SIZE from the width
    // and height — an implementation that computed the size from the
    // right/bottom EDGES instead (a natural slip) yields 80x40 here, not
    // 60x30, and a swapped x/y yields a 30-tall crop starting at x=20.
    let img =
        DynamicImage::ImageRgb8(RgbImage::from_fn(100, 50, |x, y| {
            image::Rgb([x as u8, y as u8, 0])
        }));
    let c = Crop { left: 0.2, top: 0.1, right: 0.8, bottom: 0.7 };
    let out = apply_crop(img.clone(), Some(&c)).to_rgb8();
    assert_eq!(out.dimensions(), (60, 30), "size comes from the crop's extent");
    assert_eq!(out.get_pixel(0, 0).0, [20, 5, 0], "origin = (left, top) of the frame");

    // (b) Degenerate and absent rectangles are no-ops, never a zero-size
    // image (a zero-size frame reaches par_chunks_mut(0) downstream).
    let dims = |i: DynamicImage| (i.width(), i.height());
    assert_eq!(dims(apply_crop(img.clone(), None)), (100, 50));
    let flat = Crop { left: 0.5, top: 0.1, right: 0.5, bottom: 0.9 };
    assert_eq!(dims(apply_crop(img.clone(), Some(&flat))), (100, 50));
    // Out-of-range components clamp instead of overflowing the cast.
    let wild = Crop { left: -1.0, top: -1.0, right: 2.0, bottom: 2.0 };
    assert_eq!(dims(apply_crop(img.clone(), Some(&wild))), (100, 50));

    // (c) End to end through the REAL baked pipeline (`render_to_file`
    // dispatches a non-RAW source to `render_baked_to_image`): the
    // deliverable's dimensions must equal what the helper predicts, or the
    // shared rule is not the rule the export actually applies.
    let dir = std::env::temp_dir().join(format!("autoshade-crop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.png");
    img.save(&src).unwrap();
    let out_p = dir.join("cropped.png");
    let r = EditRecipe { crop: Some(c), ..Default::default() };
    let (w, h) = render_to_file(&src, &r, &out_p, None, None, crate::diag::stderr()).unwrap();
    assert_eq!((w, h), (60, 30), "the baked export applies the SAME rectangle");
    assert_eq!(image::image_dimensions(&out_p).unwrap(), (60, 30));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_publishes_atomically_and_leaves_no_staging_file() {
    let dir = std::env::temp_dir().join(format!("autoshade-export-atomic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.png");
    DynamicImage::ImageRgb8(image::RgbImage::new(4, 3)).save(&src).unwrap();
    let out = dir.join("shot.developed.png");
    let r = EditRecipe::default();
    render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).unwrap();
    assert!(out.exists(), "the deliverable must be published");
    // The staged copy is consumed on EVERY path — a leftover would mean a
    // partial file could survive beside a delivery.
    let residue: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(residue.is_empty(), "staging residue left behind: {residue:?}");
    // A re-export replaces it in place, still with no residue.
    render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).unwrap();
    assert!(out.exists());

    // A PRE-STAGING failure: an unknown extension is rejected at format
    // resolution before any file is created — the target must survive
    // and no staging litter may appear. (The old comment claimed this
    // failed "after staging"; it never did — the REAL post-staging case
    // follows below, R12.)
    let keeper = dir.join("keeper.unknownext");
    std::fs::write(&keeper, b"a previous deliverable").unwrap();
    let err = render_to_file(&src, &r, &keeper, None, None, crate::diag::stderr());
    assert!(err.is_err(), "an unknown extension must fail the export");
    assert_eq!(
        std::fs::read(&keeper).unwrap(),
        b"a previous deliverable",
        "a failed export must not touch the file it was going to replace"
    );
    let residue: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(residue.is_empty(), "a failed export must clean its staging file: {residue:?}");

    // THE ATOMICITY PROPERTY, exercised after staging actually began: a
    // read-only target makes the PUBLISH rename fail on Windows, so the
    // encode has succeeded and the staging file exists at the moment of
    // failure — the previous deliverable must survive byte-for-byte and
    // the staging file must be cleaned (R12; the old failure aborted
    // before any file handle was opened, so atomicity was never tested).
    #[cfg(windows)]
    {
        let ro = dir.join("keeper.png");
        std::fs::write(&ro, b"a previous deliverable").unwrap();
        let mut perm = std::fs::metadata(&ro).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&ro, perm.clone()).unwrap();
        let err = render_to_file(&src, &r, &ro, None, None, crate::diag::stderr());
        assert!(err.is_err(), "publishing over a read-only file must fail");
        assert_eq!(
            std::fs::read(&ro).unwrap(),
            b"a previous deliverable",
            "a failed PUBLISH must leave the previous deliverable untouched"
        );
        let residue: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(residue.is_empty(), "publish failure must clean staging: {residue:?}");
        // This whole block is #[cfg(windows)], so the lint's "world
        // writable on Unix" concern cannot apply — we are only restoring
        // writability so the temp dir can be removed.
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&ro, perm).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bitmap_mask_sampling_matches_the_producers_convention() {
    // Producers normalise at PIXEL CENTRES, `(x + MASK_SAMPLE_CENTRE)/w`;
    // the sampler must read on the matching TEXEL-CENTRE grid, or a
    // frame-sized mask loses its last row/column, its placement drifts
    // with resolution, and the whole family sits half a texel out.
    let mut m = image::GrayImage::new(2, 1);
    m.put_pixel(0, 0, image::Luma([0]));
    m.put_pixel(1, 0, image::Luma([255]));
    // The two pixel positions a 2-wide FRAME produces — 0.5/2 and 1.5/2 —
    // are exactly the two TEXEL centres, so both are exact hits with no
    // interpolation: sx = nx·2 − 0.5 = 0 and 1.
    assert_eq!(sample_gray_norm(&m, 0.5 / 2.0, 0.0), 0.0);
    assert_eq!(sample_gray_norm(&m, 1.5 / 2.0, 0.0), 1.0, "last texel must be reachable");
    // Resolution independence: an 8-wide frame over the same 2-wide mask.
    // Pixel 7 sits at nx = 7.5/8 → sx = 1.875 − 0.5 = 1.375, past the last
    // texel centre → clamp-to-edge → full coverage. Pixel 0 sits at
    // nx = 0.5/8 → sx = −0.375 → clamp → nothing.
    assert_eq!(sample_gray_norm(&m, 7.5 / 8.0, 0.0), 1.0);
    assert_eq!(sample_gray_norm(&m, 0.5 / 8.0, 0.0), 0.0);
    // …and the interpolated interior is SYMMETRIC about the frame centre,
    // which is the half-pixel convention made visible on this arm. Every
    // value here is dyadic, so the arithmetic is exact in f32:
    //   pixel 3 → nx = 3.5/8 = 0.4375 → sx = 0.875 − 0.5 = 0.375 → 0.375
    //   pixel 4 → nx = 4.5/8 = 0.5625 → sx = 1.125 − 0.5 = 0.625 → 0.625
    // Under the refuted `x/w` reading they are 0.75 and 1.0 — a pair that
    // is neither symmetric nor even distinct at the top end.
    let p3 = sample_gray_norm(&m, 3.5 / 8.0, 0.0);
    let p4 = sample_gray_norm(&m, 4.5 / 8.0, 0.0);
    assert_eq!(p3, 0.375);
    assert_eq!(p4, 0.625);
    assert_eq!(p3 + p4, 1.0, "the ramp must be symmetric about the frame centre");
}

/// **The half-pixel convention itself, on every arm of the family that can
/// carry one** — the R29 C2 pin, and the one test built so that reverting
/// [`MASK_SAMPLE_CENTRE`] to 0 fails it four separate ways.
///
/// The measurement is in that constant's doc (two Lightroom captures, both
/// putting a nominally centred radial's centre at pixel-index 3119.5 on a
/// 6240-wide frame). What is pinned HERE is that the engine's own arms all
/// implement it, and each fixture is chosen so the two readings disagree by
/// a whole feature rather than by a rounding:
///
/// | arm | fixture | at pixel centres | at `x/w` |
/// |---|---|---|---|
/// | Radial | ellipse = the middle half of a 4 × 4 frame | a centred 2 × 2 block | ONE off-centre pixel (measured) |
/// | Linear | ramp from row 0's centre to row 3's centre | 0, ⅓, ⅔, 1 | 0, ⅙, ½, ⅚ — never reaches full |
/// | Bitmap | a 2-wide raster over a 4-wide frame | 0, ¼, ¾, 1 (symmetric) | 0, ½, 1, 1 (saturates early) |
/// | Brush | one dab at `d 0.5 0.5` on a 16 × 16 frame | mirror-symmetric alpha | the dab lands ON texel 8 |
///
/// Both frame producers are exercised: [`mask_coverage`]'s loop reads the
/// weights directly, and `apply_masks`' own `weight_at` is pinned through a
/// real develop at the end — they are separate lines of code and a mutation
/// of either one alone must be caught.
#[test]
fn every_mask_family_samples_at_pixel_centres() {
    use crate::recipe::{EditRecipe, LocalAdjustment, MaskGeometry};
    let flat = |n: u32| {
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(n, n, image::Rgb([128, 128, 128])))
    };

    // --- (a) RADIAL -----------------------------------------------------
    // Ellipse centred at (0.5, 0.5) with rx = ry = 0.25, hard edge. On a
    // 4 × 4 frame the pixel centres are nx ∈ {0.125, 0.375, 0.625, 0.875},
    // so (nx − 0.5)/0.25 ∈ {−1.5, −0.5, +0.5, +1.5} and
    // d² ∈ {0.5, 2.5, 4.5}: the four pixels with d = √0.5 = 0.707 are
    // inside and the twelve with d ≥ 1.58 are outside. A centred 2 × 2
    // block, and `radial_falloff(0, d)` is the exact step `d < 1`, so the
    // coverage bytes are exactly 255 and 0 with nothing to round.
    let ell = LocalAdjustment {
        mask: MaskGeometry::Radial {
            top: 0.25,
            left: 0.25,
            bottom: 0.75,
            right: 0.75,
            feather: 0.0,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 2,
        },
        ..Default::default()
    };
    let cov = mask_coverage(&ell, &flat(4), MaskFrame::AsRendered);
    let inside: Vec<(u32, u32)> = (0..4)
        .flat_map(|y| (0..4).map(move |x| (x, y)))
        .filter(|&(x, y)| cov.get_pixel(x, y)[0] == 255)
        .collect();
    // At the refuted `x/w` the offsets are {−2, −1, 0, +1} instead, so
    // d² ∈ {0, 1, 2, …} and the four neighbours land at d = 1 EXACTLY,
    // which the strict `d < 1` hard edge excludes: the whole mask collapses
    // to the single pixel (2, 2). Measured, not predicted — reverting
    // `MASK_SAMPLE_CENTRE` prints `left: [(2, 2)]` here.
    assert_eq!(
        inside,
        vec![(1, 1), (2, 1), (1, 2), (2, 2)],
        "a centred ellipse must cover a CENTRED block, not one corner-anchored pixel"
    );
    for y in 0..4 {
        for x in 0..4 {
            let v = cov.get_pixel(x, y)[0];
            assert!(v == 0 || v == 255, "a hard edge has no partial texel: ({x},{y}) = {v}");
        }
    }

    // --- (b) LINEAR -----------------------------------------------------
    // Zero end on row 0's centre (ny = 0.125), full end on row 3's centre
    // (ny = 0.875). vy = 0.75, len2 = 0.5625, so the weights are
    // (ny − 0.125)·0.75/0.5625 = 0, 1/3, 2/3, 1 — every operand dyadic, and
    // the last one EXACTLY 1. Through the shipped `Measured` profile,
    // smoothstep(t^1.124), they are 0, round(52.17) = 52,
    // round(177.52) = 178, 255.
    let ramp = LocalAdjustment {
        mask: MaskGeometry::Linear {
            zero_x: 0.5, zero_y: 0.125, full_x: 0.5, full_y: 0.875,
        },
        ..Default::default()
    };
    let lcov = mask_coverage(&ramp, &flat(4), MaskFrame::AsRendered);
    let column: Vec<u8> = (0..4).map(|y| lcov.get_pixel(2, y)[0]).collect();
    assert_eq!(column, vec![0, 52, 178, 255], "the shipped ramp must span its ends exactly");

    // --- (c) BITMAP -----------------------------------------------------
    // A 2-wide raster [0, 255] read by a 4-wide frame. Texel centres sit at
    // nx = 0.25 and 0.75; the frame's pixel centres at 0.125/0.375/0.625/
    // 0.875 give sx = nx·2 − 0.5 = −0.25, 0.25, 0.75, 1.25, which clamp to
    // 0, 0.25, 0.75, 1 — symmetric about the frame centre, and reaching
    // BOTH ends. The `Bitmap` arm ignores its path when a raster is handed
    // in, so this needs no file.
    let mut ras = image::GrayImage::new(2, 1);
    ras.put_pixel(0, 0, image::Luma([0]));
    ras.put_pixel(1, 0, image::Luma([255]));
    let bmp = MaskGeometry::Bitmap { path: "unused — the raster is passed in".into() };
    let row: Vec<f32> = (0..4u32)
        .map(|x| {
            mask_weight(&bmp, (x as f32 + MASK_SAMPLE_CENTRE) / 4.0, 0.5, Some(&ras))
        })
        .collect();
    assert_eq!(row, vec![0.0, 0.25, 0.75, 1.0]);
    assert_eq!(row[0] + row[3], 1.0, "the two ends must mirror");
    assert_eq!(row[1] + row[2], 1.0, "…and so must the interior");

    // --- (d) BRUSH ------------------------------------------------------
    // One dab at the exact frame centre of a 16 × 16 frame. `rasterise_
    // brush_group` stamps it at texel coordinate 0.5·16 − 0.5 = 7.5, i.e.
    // BETWEEN texels 7 and 8, so the alpha is mirror-symmetric about the
    // frame centre; the frame then reads texel x exactly (16 is a power of
    // two, so (x + 0.5)/16 · 16 − 0.5 = x with no rounding at all). At
    // `x/w` the dab would land ON texel 8 and the mirror would break —
    // texel 4 falls exactly on ρ = 1 and reads 0 while its partner texel 11
    // is still lit.
    let dab = probe_brush(&[(1.0, 0.25, 1.0, 0.5, "d 0.5 0.5")]);
    let braster = brush_raster(&dab, 16, 16).expect("one dab");
    assert_eq!(braster.dimensions(), (16, 16), "small frame = 1:1 raster");
    let at = |x: u32, y: u32| {
        mask_weight(
            &dab,
            (x as f32 + MASK_SAMPLE_CENTRE) / 16.0,
            (y as f32 + MASK_SAMPLE_CENTRE) / 16.0,
            Some(&braster),
        )
    };
    for x in 0..16u32 {
        assert_eq!(at(x, 7), at(15 - x, 7), "the dab is not mirrored in x at column {x}");
        assert_eq!(at(7, x), at(7, 15 - x), "the dab is not mirrored in y at row {x}");
    }
    // The mirror is only meaningful if the dab is actually THERE and the
    // pair straddling the centre share the peak (a single peak texel is the
    // `x/w` signature).
    assert!(at(7, 7) > 0.9, "premise: the dab covers the centre: {}", at(7, 7));
    assert_eq!(at(7, 7), at(8, 8), "the centre must be shared, not owned by one texel");
    assert_eq!(at(0, 7), 0.0, "…and the dab still ends: {}", at(0, 7));

    // --- both frame producers -------------------------------------------
    // `mask_coverage` above is the OVERLAY's loop. `apply_masks`' own
    // `weight_at` is a separate line, so pin it on the same radial: exactly
    // the centred 2 × 2 may move.
    let r = EditRecipe {
        masks: vec![LocalAdjustment { exposure_ev: -4.0, ..ell }],
        ..Default::default()
    };
    let mut data = vec![[0.6_f32; 3]; 16];
    apply_develop_anon(&mut data, 4, 4, &r);
    let moved: Vec<usize> =
        (0..16).filter(|&i| (data[i][0] - 0.6).abs() > 1e-4).collect();
    assert_eq!(
        moved,
        vec![5, 6, 9, 10],
        "the render's own producer must agree with the overlay's"
    );
}

#[test]
fn straighten_survives_a_zero_size_frame() {
    let img = DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
    let out = rotate_straighten(&img, 5.0);
    assert_eq!((out.width(), out.height()), (0, 0));
}

#[test]
fn develop_survives_a_zero_size_frame() {
    // rayon asserts chunk_size != 0 even on an EMPTY slice, so every
    // `par_chunks_mut(w)` in the develop needs a zero-dim guard. Batch 40
    // guarded the two vignette passes and claimed they were the last of
    // the family; `apply_masks` in fact had two more (its tone pass and
    // its local-NR pass), so this recipe carries a MASK as well — the
    // case that still panicked (R12).
    let img = DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
    let r = EditRecipe {
        lens_vignette: 60.0,
        lens_profile: crate::recipe::LensProfile {
            vignette: vec![1.0, 1.2, 1.4, 1.6],
            vignette_on: true,
            ..Default::default()
        },
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 1.0, full_y: 1.0 },
            amount: 1.0,
            exposure_ev: 1.0,
            noise_reduction: 50.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let out = develop_preview(&img, &r);
    assert_eq!((out.width(), out.height()), (0, 0));
}

#[test]
fn grading_blending_controls_overlap_not_amplitude() {
    use crate::recipe::ColorGrade;
    // A legal Blending of 0 used to zero every regional wheel. It must
    // now still grade — only the region split gets tighter.
    let mut cg = ColorGrade { blending: 0.0, shadow_sat: 100.0, shadow_hue: 210.0, ..Default::default() };
    let mut dark = [[0.08f32, 0.08, 0.08]];
    apply_color_grade(&mut dark, &cg);
    assert!(
        (dark[0][2] - dark[0][0]).abs() > 0.01,
        "blending=0 must still apply the shadow wheel, got {:?}",
        dark[0]
    );
    // And 100 must keep the shadow wheel OFF the midtone: at l = 0.5,
    // mid = 0.5, w_sh = 1 − smoothstep(0, 0.5, 0.5) = 0 exactly, and the
    // midtone/global wheels carry no sat/lum here — the pixel must come
    // back UNTOUCHED. (The old form compared two applications with
    // field-for-field identical ColorGrade values — f(x) == f(x) could
    // never fail, so a shadow-into-midtone leak passed, U14.)
    cg.blending = 100.0;
    let mut a = [[0.5f32, 0.5, 0.5]];
    apply_color_grade(&mut a, &cg);
    assert_eq!(
        a[0],
        [0.5f32, 0.5, 0.5],
        "blending=100: the shadow wheel must not reach the midtone"
    );
    // AMPLITUDE INVARIANCE on a fully-owned deep shadow: blending shapes
    // the region SPLIT only, so a pixel every split assigns to the
    // shadow wheel must grade the same at 0 / 50 / 100. An engine that
    // additionally scaled the regional weights by any blending factor
    // passes the two probes above (the deep probe only ran at 0, the
    // midpoint at 100) but fails this sweep (R12). l = 0.02 sits below
    // every sh_start; the b=100 ramp contributes only ~5e-3 of weight
    // there, far under the mutation's 2x amplitude swing.
    let mut sweep = Vec::new();
    for b in [0.0f32, 50.0, 100.0] {
        let mut px = [[0.02f32, 0.02, 0.02]];
        apply_color_grade(
            &mut px,
            &ColorGrade { blending: b, shadow_sat: 100.0, shadow_hue: 210.0, ..Default::default() },
        );
        sweep.push(px[0]);
    }
    // ENDPOINTS, not consecutive pairs. An amplitude multiplier moves
    // this probe monotonically across the sweep, so each STEP is only
    // ~4e-3 — under a 5e-3 tolerance — while end to end it moves ~8e-3.
    // The consecutive form therefore passed on the exact mutation this
    // block names (measured against the real engine, which stays within
    // 3.7e-5 end to end, so the endpoint form keeps 100x+ of headroom).
    for (a, b) in sweep[0].iter().zip(&sweep[sweep.len() - 1]) {
        assert!(
            (a - b).abs() < 5e-3,
            "blending changed the shadow AMPLITUDE: {sweep:?}"
        );
    }
}

#[test]
fn duplicate_curve_points_do_not_cliff() {
    use crate::recipe::CurvePoint;
    // Two outputs at ONE input is not a function; the documented rule is
    // first-point-wins, which must hold at the code AND just after it.
    let lut = curve_lut(&[
        CurvePoint { input: 0, output: 0 },
        CurvePoint { input: 128, output: 200 },
        CurvePoint { input: 128, output: 50 },
        CurvePoint { input: 255, output: 255 },
    ]);
    let step = (lut[129] - lut[128]).abs();
    assert!(step < 0.05, "one-bin cliff at the duplicate: {step} ({} -> {})", lut[128], lut[129]);
    assert!(lut[128] > 0.7, "first point must win at the duplicate code: {}", lut[128]);
}

#[test]
fn white_point_is_invariant_to_highlights_and_bright_stays_bright() {
    // The engine renders faithfully: Highlights shapes the shoulder but must NOT
    // move the white point, so the brightest tone stays pinned at white. (Keeping
    // bright FOAM bright under an over-cooked recipe is the recipe layer's job —
    // EditRecipe::temper — not an engine override.)
    for h in [-100.0, -78.81, -30.0, 30.0, 100.0] {
        let lut = build_tone_lut(&EditRecipe { highlights: h, ..Default::default() });
        assert!(
            (sample_lut(&lut, 1.0) - 1.0).abs() < 1e-3,
            "highlights {h} moved the white point: {}",
            sample_lut(&lut, 1.0)
        );
    }
    // A neutral recipe must leave bright near-white foam bright.
    let mut foam = vec![[0.90_f32, 0.93, 0.96]];
    apply_develop_anon(&mut foam, 1, 1, &EditRecipe::default());
    let lum = 0.299 * foam[0][0] + 0.587 * foam[0][1] + 0.114 * foam[0][2];
    assert!(lum > 0.90, "neutral recipe dimmed bright foam: {lum}");
}

#[test]
fn tempered_recipe_renders_foam_light_and_water_saturated() {
    // End-to-end: the over-cooked AI recipe (the one that greyed the foam),
    // after clamp + temper, rendered through the monotone curve. Foam must be
    // LIGHT (not crushed to the muddy ~0.6 grey it was) and water must stay
    // turquoise — the engine + recipe layers compose, no engine override.
    let mut r = EditRecipe {
        highlights: -78.81,
        shadows: 36.56,
        whites: 10.27,
        blacks: -14.59,
        contrast: 4.68,
        exposure_ev: -0.177,
        vibrance: 11.19,
        saturation: 2.9,
        ..Default::default()
    };
    r.clamp();
    r.temper(crate::recipe::GradeStrength::calibrated());
    let lum = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let mut foam = vec![[0.90_f32, 0.93, 0.96]];
    apply_develop_anon(&mut foam, 1, 1, &r);
    assert!(lum(foam[0]) > 0.80, "foam crushed (should stay light): luma {}", lum(foam[0]));
    let mut water = vec![[0.35_f32, 0.62, 0.66]];
    apply_develop_anon(&mut water, 1, 1, &r);
    let [rr, gg, bb] = water[0];
    assert!(gg > rr + 0.10 && bb > rr + 0.10, "water lost its turquoise: [{rr}, {gg}, {bb}]");
}

#[test]
fn region_tones_pin_endpoints_and_stay_monotonic() {
    // Highlights/shadows/contrast must never move the endpoints (only whites/
    // blacks may), and the curve must stay monotone under any extreme combo.
    let recipes = [
        EditRecipe::default(),
        EditRecipe { highlights: -100.0, shadows: 100.0, contrast: 100.0, ..Default::default() },
        EditRecipe { highlights: 100.0, shadows: -100.0, contrast: -100.0, ..Default::default() },
    ];
    for r in recipes {
        let lut = build_tone_lut(&r);
        for i in 1..lut.len() {
            assert!(lut[i] >= lut[i - 1] - 1e-6, "non-monotonic at {i}");
        }
        assert!(sample_lut(&lut, 0.0) < 1e-3, "black point moved by hi/sh/contrast");
        assert!((sample_lut(&lut, 1.0) - 1.0).abs() < 1e-3, "white point moved by hi/sh/contrast");
    }
}

/// v1.5.0, the parametric curve's contract (`parametric_lut`): nothing at
/// rest wherever the splits sit, each region slider moving its OWN region
/// the way its sign says and leaving the far end of the range alone, both
/// ends pinned and the curve monotone at every extreme and split layout,
/// composed UNDER the point curve — and reaching the develop.
///
/// MUTATIONS THIS CATCHES: `PARAMETRIC_REACH` above ½ (the extremes fold
/// the curve back), a region's control value moved by the wrong index
/// (the direction table), the composition order swapped (the point-curve
/// case), and `apply_develop`'s `tone_neutral` blind to the curve.
#[test]
fn the_parametric_curve_moves_its_own_region_and_stays_monotone() {
    let base = build_tone_lut(&EditRecipe::default());
    assert!(parametric_lut(&EditRecipe { param_midtone_split: 70.0, ..Default::default() }).is_none());
    for (field, centre, far) in [
        ("param_shadows", 0.125f32, 0.875f32),
        ("param_darks", 0.375, 0.875),
        ("param_lights", 0.625, 0.125),
        ("param_highlights", 0.875, 0.125),
    ] {
        for sign in [1.0f32, -1.0] {
            let mut json = serde_json::to_value(EditRecipe::default()).expect("serialises");
            json[field] = serde_json::json!(100.0 * sign);
            let r: EditRecipe = serde_json::from_value(json).expect("a region value");
            let lut = build_tone_lut(&r);
            let moved = sample_lut(&lut, centre) - sample_lut(&base, centre);
            assert!(moved * sign > 0.03, "{field} {sign:+}: its own centre moved {moved}");
            let stray = (sample_lut(&lut, far) - sample_lut(&base, far)).abs();
            assert!(stray < 0.02, "{field} {sign:+}: the far end of the range moved {stray}");
        }
    }
    // Every extreme, over the split layouts at both walls and the default.
    for splits in [[25.0, 50.0, 75.0], [10.0, 20.0, 30.0], [70.0, 80.0, 90.0], [10.0, 50.0, 90.0]] {
        for signs in 0..16u32 {
            let at = |k: u32| if (signs >> k) & 1 == 1 { 100.0 } else { -100.0 };
            let r = EditRecipe {
                param_shadows: at(0),
                param_darks: at(1),
                param_lights: at(2),
                param_highlights: at(3),
                param_shadow_split: splits[0],
                param_midtone_split: splits[1],
                param_highlight_split: splits[2],
                ..Default::default()
            };
            let lut = parametric_lut(&r).expect("a moved region builds the curve");
            for i in 1..lut.len() {
                assert!(lut[i] >= lut[i - 1] - 1e-6, "{splits:?} signs {signs:04b}: non-monotone at {i}");
            }
            assert!(lut[0].abs() < 1e-6 && (lut[LUT_N - 1] - 1.0).abs() < 1e-6, "{splits:?}: an end moved");
        }
    }
    // Composed UNDER the point curve: the point curve bends what the
    // regions made, and the order is observable on this pair.
    let both = EditRecipe {
        param_darks: 60.0,
        tone_curve: vec![
            crate::recipe::CurvePoint { input: 0, output: 0 },
            crate::recipe::CurvePoint { input: 96, output: 160 },
            crate::recipe::CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    let (p, c, lut) = (parametric_lut(&both).expect("moved"), curve_lut(&both.tone_curve), build_tone_lut(&both));
    let mut observable = 0.0f32;
    for x in [0.2f32, 0.3, 0.4, 0.5] {
        let want = sample_lut(&c, sample_lut(&p, x));
        assert!((sample_lut(&lut, x) - want).abs() < 2e-3, "at {x}: {} vs {want}", sample_lut(&lut, x));
        observable = observable.max((want - sample_lut(&p, sample_lut(&c, x))).abs());
    }
    assert!(observable > 0.02, "premise: the two orders differ here ({observable})");
    // …and the develop runs it: a mid-dark grey brightens under Darks.
    let mut grey = vec![[0.3f32, 0.3, 0.3]];
    apply_develop_anon(&mut grey, 1, 1, &EditRecipe { param_darks: 80.0, ..Default::default() });
    let mut rest = vec![[0.3f32, 0.3, 0.3]];
    apply_develop_anon(&mut rest, 1, 1, &EditRecipe::default());
    assert!(grey[0][1] > rest[0][1] + 0.02, "the develop skipped the curve: {:?} vs {:?}", grey[0], rest[0]);
}

/// v1.5.0, the Calibration panel ([`Calibration`]): a grey is still grey
/// under every primary slider at both extremes, each primary's Hue turns
/// its own colour the way Lightroom's slider track reads (+ red toward
/// yellow, + green toward cyan, + blue toward magenta), Saturation moves
/// that colour's chroma, and the shadows tint pulls a dark grey toward
/// magenta (+) or green (−) while a light grey does not move.
///
/// MUTATIONS THIS CATCHES: the hue rotation's sign flipped, the basis
/// change composed the wrong way round (white stops mapping to white), the
/// tint's luminance weight inverted (highlights tinted), and the stage
/// left out of the develop.
#[test]
fn calibration_turns_its_own_primary_and_keeps_grey_grey() {
    let chroma = |p: [f32; 3]| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
    let with = |name: &str, v: f32| {
        let mut json = serde_json::to_value(EditRecipe::default()).expect("serialises");
        json[name] = serde_json::json!(v);
        serde_json::from_value::<EditRecipe>(json).expect("a calibration value")
    };
    let dev = |px: [f32; 3], r: &EditRecipe| {
        let mut d = vec![px];
        apply_develop_anon(&mut d, 1, 1, r);
        d[0]
    };
    for name in ["cal_red_hue", "cal_red_sat", "cal_green_hue", "cal_green_sat", "cal_blue_hue", "cal_blue_sat"] {
        for v in [100.0f32, -100.0] {
            for g in [0.2f32, 0.5, 0.9] {
                let out = dev([g, g, g], &with(name, v));
                assert!(
                    chroma(out) < 2e-3 && (out[1] - g).abs() < 2e-3,
                    "{name} {v:+}: grey {g} became {out:?}"
                );
            }
        }
    }
    let (red, green, blue) = ([0.8f32, 0.2, 0.2], [0.2f32, 0.8, 0.2], [0.2f32, 0.2, 0.8]);
    let up = dev(red, &with("cal_red_hue", 100.0));
    let down = dev(red, &with("cal_red_hue", -100.0));
    assert!(up[1] > up[2] + 0.02, "red hue + turns red toward yellow: {up:?}");
    assert!(down[2] > down[1] + 0.02, "red hue − turns red toward magenta: {down:?}");
    let up = dev(green, &with("cal_green_hue", 100.0));
    assert!(up[2] > up[0] + 0.02, "green hue + turns green toward cyan: {up:?}");
    let up = dev(blue, &with("cal_blue_hue", 100.0));
    assert!(up[0] > up[1] + 0.02, "blue hue + turns blue toward magenta: {up:?}");
    assert!(chroma(dev(red, &with("cal_red_sat", 100.0))) > chroma(red) + 0.02);
    assert!(chroma(dev(red, &with("cal_red_sat", -100.0))) < chroma(red) - 0.02);
    let dark = dev([0.15; 3], &with("cal_shadow_tint", 100.0));
    assert!(dark[0] > dark[1] + 0.01 && dark[2] > dark[1] + 0.01, "+ tints the shadows magenta: {dark:?}");
    let dark = dev([0.15; 3], &with("cal_shadow_tint", -100.0));
    assert!(dark[1] > dark[0] + 0.01, "− tints the shadows green: {dark:?}");
    let light = dev([0.9; 3], &with("cal_shadow_tint", 100.0));
    assert!(chroma(light) < 2e-3, "a light grey is not a shadow: {light:?}");
    assert_eq!(dev(red, &EditRecipe::default()), red, "no slider, no stage");
}

/// v1.5.0, the B&W treatment ([`apply_gray_mix`]): every pixel turns grey,
/// a mixer band lightens or darkens its own colour and nothing else, the
/// colour mixer has nothing to act on, a mixer without the treatment is
/// not a treatment, and the toning tools — the colour grade and the
/// per-channel curves — still reach the grey.
///
/// MUTATIONS THIS CATCHES: the conversion placed after the RGB curves (the
/// channel-curve toning vanishes), the band weight read from the wrong
/// band, HSL still applied in black and white, and the mixer applied
/// without the treatment.
#[test]
fn black_and_white_mixes_by_band_and_still_takes_toning() {
    use crate::recipe::{ColorGrade, CurvePoint, Hsl};
    let chroma = |p: &[f32; 3]| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
    let frame = vec![[0.8f32, 0.2, 0.2], [0.2, 0.3, 0.8], [0.5, 0.5, 0.5], [0.3, 0.7, 0.2]];
    let run = |r: &EditRecipe| {
        let mut d = frame.clone();
        apply_develop_anon(&mut d, 4, 1, r);
        d
    };
    let bw = EditRecipe { convert_to_grayscale: true, ..Default::default() };
    let flat = run(&bw);
    for p in &flat {
        assert!(chroma(p) < 1e-4, "black and white leaves no colour: {flat:?}");
    }
    assert!((flat[2][0] - 0.5).abs() < 2e-3, "a grey keeps its value: {:?}", flat[2]);
    let lighter = run(&EditRecipe { gray_red: 100.0, ..bw.clone() });
    assert!(lighter[0][0] > flat[0][0] + 0.05, "red +100 lightens the red: {lighter:?} vs {flat:?}");
    for i in 1..4 {
        assert!((lighter[i][0] - flat[i][0]).abs() < 1e-4, "red +100 moved pixel {i}: {lighter:?}");
    }
    let darker = run(&EditRecipe { gray_red: -100.0, ..bw.clone() });
    assert!(darker[0][0] < flat[0][0] - 0.05, "red −100 darkens the red: {darker:?}");
    let blue = run(&EditRecipe { gray_blue: 100.0, ..bw.clone() });
    assert!(blue[1][0] > flat[1][0] + 0.05 && (blue[0][0] - flat[0][0]).abs() < 1e-4);
    let hsl = run(&EditRecipe { hsl: Hsl { luminance: [100.0; 8], ..Default::default() }, ..bw.clone() });
    assert_eq!(hsl, flat, "the colour mixer does not act in black and white");
    assert_eq!(run(&EditRecipe { gray_red: 100.0, ..Default::default() }), frame, "a mixer is not a treatment");
    let graded = run(&EditRecipe {
        color_grade: ColorGrade { shadow_hue: 30.0, shadow_sat: 60.0, ..Default::default() },
        ..bw.clone()
    });
    assert!(chroma(&graded[1]) > 0.01, "the grade tones the grey: {graded:?}");
    let curved = run(&EditRecipe {
        red_curve: vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 128, output: 160 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..bw.clone()
    });
    assert!(curved[2][0] > curved[2][1] + 0.02, "a red-channel curve tones the grey: {curved:?}");
    let saturated = run(&EditRecipe { saturation: 100.0, ..bw });
    assert_eq!(saturated, flat, "Saturation has no colour to move in black and white");
}

/// v1.5.0, Point Color ([`apply_point_colors`]): a swatch sampled from a
/// colour moves that colour, fades out across its hue window, leaves a
/// colour outside its windows and every grey exactly as they were, and
/// moves nothing at all while its shifts are 0.
///
/// MUTATIONS THIS CATCHES: the hue offset not wrapped (a red swatch
/// missing the red at 359°), the trapezoid's falloff inverted, the chroma
/// gate dropped (greys recoloured), and the eyedropper sampling in a
/// different colour model from the one the render matches in.
#[test]
fn a_point_color_moves_its_own_colour_and_nothing_else() {
    use crate::recipe::PointColor;
    let red = [0.8f32, 0.2, 0.2];
    let swatch = point_color_at(red).expect("a red has a colour to hold");
    let (h, s, l) = rgb_to_hsl(red[0], red[1], red[2]);
    assert!((swatch.src_hue - h * std::f32::consts::TAU).abs() < 1e-5);
    assert_eq!((swatch.src_sat, swatch.src_lum), (s, l));
    let chip = point_color_rgb(&swatch);
    assert!(chip.iter().zip(red).all(|(a, b)| (a - b).abs() < 1e-4), "the chip is the sample: {chip:?}");
    assert!(point_color_at([0.5, 0.5, 0.5]).is_none(), "a grey is nobody's colour");
    assert!(point_color_at([0.52, 0.5, 0.5]).is_none(), "nor is a colour the gate gives to nobody");
    // red, blue, grey, orange (partly inside the hue window), and a red on
    // the far side of 0° (hue 355°).
    let frame = vec![red, [0.2, 0.3, 0.8], [0.5, 0.5, 0.5], [0.8, 0.53, 0.2], [0.8, 0.2, 0.25]];
    let run = |p: PointColor| {
        let mut d = frame.clone();
        apply_develop_anon(&mut d, 5, 1, &EditRecipe { point_colors: vec![p], ..Default::default() });
        d
    };
    assert_eq!(run(swatch.clone()), frame, "a swatch with no shift moves nothing");
    let turned = run(PointColor { hue_shift: 1.0, ..swatch.clone() });
    let hue_of = |p: [f32; 3]| rgb_to_hsl(p[0], p[1], p[2]).0;
    let moved = |i: usize| (hue_of(turned[i]) - hue_of(frame[i]) + 0.5).rem_euclid(1.0) - 0.5;
    assert!((moved(0) - POINT_HUE_SHIFT_TURNS).abs() < 0.01, "the sampled red turns fully: {}", moved(0));
    assert!(moved(3) > 0.005 && moved(3) < moved(0) - 0.005, "orange turns partly: {}", moved(3));
    assert!(moved(4) > 0.02, "a red across 0° is still the swatch's red: {}", moved(4));
    assert_eq!(turned[1], frame[1], "blue is outside the window");
    assert_eq!(turned[2], frame[2], "a grey has no colour to match");
    let greyed = run(PointColor { sat_scale: -1.0, ..swatch.clone() });
    assert!(greyed[0][0] - greyed[0][2] < 0.02, "saturation −1 greys the red: {:?}", greyed[0]);
    let lifted = run(PointColor { lum_scale: 1.0, ..swatch });
    assert!(luma601(&lifted[0]) > luma601(&frame[0]) + 0.05, "luminance +1 lifts the red: {:?}", lifted[0]);
}

/// The Point Color eyedropper's reference develop: what the stages AFTER
/// the point colours hold changes nothing in it, and what the stages before
/// them hold still does.
///
/// MUTATION THIS CATCHES: a downstream stage left on in
/// `point_color_sampling_recipe` (the swatch lands on the graded or
/// saturated colour, outside the windows the render then tests), or an
/// upstream one switched off with them (the swatch misses the colour the
/// mixer made).
#[test]
fn the_point_color_eyedropper_samples_the_frame_the_point_colours_see() {
    use crate::recipe::{ColorGrade, Hsl, PointColor};
    let frame: Vec<[f32; 3]> = (0..64)
        .map(|i| {
            let t = i as f32 / 63.0;
            [0.2 + 0.6 * t, 0.5 - 0.3 * t, 0.3 + 0.2 * (t * 7.0).sin().abs()]
        })
        .collect();
    let sample = |r: &EditRecipe| {
        let mut d = frame.clone();
        apply_develop_anon(&mut d, 8, 8, &point_color_sampling_recipe(r));
        d
    };
    let upstream = EditRecipe { exposure_ev: 0.3, hsl: Hsl { hue: [40.0; 8], ..Default::default() }, ..Default::default() };
    let quiet = sample(&upstream);
    let downstream = EditRecipe {
        color_grade: ColorGrade { shadow_hue: 200.0, shadow_sat: 80.0, ..Default::default() },
        clarity: 60.0,
        texture: 40.0,
        saturation: 70.0,
        vibrance: -50.0,
        color_nr: 40.0,
        noise_reduction: 40.0,
        sharpening: 90.0,
        point_colors: vec![PointColor { hue_shift: 1.0, ..PointColor::sampled(1.0, 0.6, 0.5) }],
        ..upstream.clone()
    };
    assert_eq!(sample(&downstream), quiet, "the stages after the point colours are not in the sample");
    assert_ne!(sample(&EditRecipe::default()), quiet, "the stages before them are");
}

/// A slider must run out of authority, never destroy detail.
///
/// Monotonicity and pinned endpoints — the two things the tone tests
/// asserted for four rounds — are both TRUE of a perfectly flat band, so
/// they were blind to the worst thing this curve can do. Measured on the
/// pre-fix engine through the real export path: `whites: -50` mapped input
/// 0.9568–0.9731 to one 16-bit code and left the top decade with 75
/// distinct codes out of 411; `highlights: +60` mapped everything above
/// 0.8195 to pure white. Both are ordinary edits.
///
/// So this pins the property those tests missed: no input band survives
/// the curve as a single output value.
#[test]
fn no_slider_setting_collapses_a_tonal_band() {
    const N: usize = 4096;
    // The onset of the old collapse for each slider, and past it.
    let recipes = [
        ("neutral", EditRecipe::default()),
        ("whites -50", EditRecipe { whites: -50.0, ..Default::default() }),
        ("whites -100", EditRecipe { whites: -100.0, ..Default::default() }),
        ("highlights +60", EditRecipe { highlights: 60.0, ..Default::default() }),
        ("highlights +100", EditRecipe { highlights: 100.0, ..Default::default() }),
        ("blacks +60", EditRecipe { blacks: 60.0, ..Default::default() }),
        ("blacks +100", EditRecipe { blacks: 100.0, ..Default::default() }),
        ("shadows +76", EditRecipe { shadows: 76.0, ..Default::default() }),
        ("shadows +100", EditRecipe { shadows: 100.0, ..Default::default() }),
        ("contrast +100", EditRecipe { contrast: 100.0, ..Default::default() }),
        (
            "the old extreme combo",
            EditRecipe { highlights: -100.0, shadows: 100.0, contrast: 100.0, ..Default::default() },
        ),
        (
            "everything at once",
            EditRecipe {
                whites: -100.0,
                blacks: 100.0,
                highlights: 100.0,
                shadows: 100.0,
                contrast: 100.0,
                ..Default::default()
            },
        ),
    ];
    // A run this long is a visibly flat patch, not quantisation: the worst
    // measured pre-fix run was 740 of 4096 and the neutral curve's is 1.
    const MAX_RUN: usize = 96;
    for (name, r) in recipes {
        let lut = build_tone_lut(&r);
        let out: Vec<u16> = (0..N)
            .map(|i| {
                let x = i as f32 / (N - 1) as f32;
                (sample_lut(&lut, x).clamp(0.0, 1.0) * 65535.0).round() as u16
            })
            .collect();
        let (mut run, mut worst, mut worst_at) = (1usize, 1usize, 0usize);
        for i in 1..out.len() {
            run = if out[i] == out[i - 1] { run + 1 } else { 1 };
            if run > worst {
                worst = run;
                worst_at = i;
            }
        }
        assert!(
            worst <= MAX_RUN,
            "{name}: {worst} consecutive inputs (around x={:.4}) all render to {} — \
             a slider flattened a tonal band instead of saturating",
            worst_at as f32 / (N - 1) as f32,
            out[worst_at]
        );
    }
}

/// The same property with EXPOSURE in play — the dimension the test above
/// never varies, and the one where this design's guarantee actually ends.
///
/// Two corrections to how the band is measured, both from re-deriving the
/// numbers rather than trusting the earlier write-up:
///
///   * A run at 0 or 65535 is CLIPPING, which is what a strong slider on
///     an already-bright frame is supposed to do; a run at an interior
///     code is destroyed detail. Counting both together made
///     `contrast: +100` at `+0.5 EV` look like a 161-input collapse when
///     every one of those inputs renders to pure white — and the same
///     measurement said the pre-fix `highlights: +60` was harmless, which
///     it was not. Only interior runs are counted here.
///   * Measured that way, the v0.18.0 limiter's win is still real and
///     larger than it looked: `whites: -50` goes from a 157-input
///     interior plateau (no limiter) to 43.
///
/// The threshold is what the shipped design HOLDS, not what would be
/// nice — see the measured-grid note ahead of the loop below.
#[test]
fn no_slider_collapses_an_interior_band_at_any_exposure() {
    const N: usize = 4096;
    // What the shipped design HOLDS on this WHOLE grid, with margin: the
    // worst cell measures 100 (weighted-knot model + per-slider λ). Not a
    // wish — measured.
    const MAX_RUN: usize = 128;
    type Case = (&'static str, fn(&mut EditRecipe));
    let sliders: [Case; 18] = [
        ("neutral", |_r| {}),
        ("whites -50", |r| r.whites = -50.0),
        ("whites -100", |r| r.whites = -100.0),
        ("highlights +60", |r| r.highlights = 60.0),
        ("highlights +100", |r| r.highlights = 100.0),
        ("blacks +60", |r| r.blacks = 60.0),
        ("blacks +100", |r| r.blacks = 100.0),
        ("shadows +76", |r| r.shadows = 76.0),
        ("shadows +100", |r| r.shadows = 100.0),
        ("contrast +100", |r| r.contrast = 100.0),
        ("old extreme combo", |r| {
            r.contrast = 100.0;
            r.highlights = -100.0;
            r.shadows = 100.0;
        }),
        ("everything at once", |r| {
            r.contrast = 100.0;
            r.highlights = 100.0;
            r.shadows = 100.0;
            r.whites = -100.0;
            r.blacks = 100.0;
        }),
        ("highlights -100", |r| r.highlights = -100.0),
        ("highlights -60", |r| r.highlights = -60.0),
        ("shadows -100", |r| r.shadows = -100.0),
        ("contrast -100", |r| r.contrast = -100.0),
        ("whites +50", |r| r.whites = 50.0),
        ("blacks -60", |r| r.blacks = -60.0),
    ];
    let interior_run = |r: &EditRecipe| -> (usize, usize, u16) {
        let lut = build_tone_lut(r);
        let out: Vec<u16> = (0..N)
            .map(|i| {
                let x = i as f32 / (N - 1) as f32;
                (sample_lut(&lut, x).clamp(0.0, 1.0) * 65535.0).round() as u16
            })
            .collect();
        let (mut run, mut worst, mut worst_at) = (1usize, 1usize, 0usize);
        for i in 1..out.len() {
            // Clipping is the user's own request; an interior plateau is
            // detail that no later stage can recover.
            run = if out[i] == out[i - 1] && out[i] > 0 && out[i] < u16::MAX {
                run + 1
            } else {
                1
            };
            if run > worst {
                worst = run;
                worst_at = i;
            }
        }
        (worst, worst_at, out[worst_at])
    };

    // ONE bound for the WHOLE grid. Until the weighted-knot model
    // (`tone_knot_weights`, M-T1) this test carried a 220-input carve-out
    // for `ev > 1.0`: the basis added slider offsets at knots whose base
    // intervals exposure had already saturated (`base_gap <= 1e-6`, the λ
    // limiter's rightful skip case), a strong negative slider dipped
    // below the ceiling, and the monotone backstop flattened a whole
    // interval at an interior grey — `contrast: -100` at `+1.5 EV`
    // flattened 197 inputs at code 56304. Four knot-LEVEL repairs were
    // measured and rejected (each traded the tail for a worse one; the
    // pre-weights per-slider λ took the worst from 197 to 317) before the
    // tone-MODEL fix landed: knot authority now follows the base curve's
    // own local separation, a slider aimed at a clipped region yields
    // honest clipping, and the measured grid is 6 cells > 96 with a
    // global worst of 100 — the same level the ev = 0 design holds, three
    // of those six sitting one code below pure white (65534, the
    // quantisation edge of clipping, not a band).
    for ev in [-3.0f32, -2.0, -1.5, -1.0, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 1.28, 1.5, 2.0, 3.0] {
        for (name, apply) in sliders {
            let mut r = EditRecipe { exposure_ev: ev, ..Default::default() };
            apply(&mut r);
            let (worst, at, code) = interior_run(&r);
            assert!(
                worst <= MAX_RUN,
                "{name} at {ev:+} EV: {worst} consecutive inputs (around x={:.4}) all render                      to the interior code {code} — a slider flattened a tonal band instead of                      saturating",
                at as f32 / (N - 1) as f32
            );
        }
    }
}

/// M-T2: the per-slider λ iteration removed (every `lam` left at 1, only
/// the single-λ backstop applied) — the slider that binds an interval
/// must saturate ALONE; the sliders that did not close it keep their
/// authority. This was the model's second known gap: pinning shadows +50
/// while dragging whites −100 rendered the shadows at 22.5.
#[test]
fn a_slider_that_binds_an_interval_no_longer_drags_the_innocent_ones() {
    // whites −100 alone closes the 0.92–1.0 interval and must saturate
    // near −0.45 (the value the interval can absorb); shadows was not
    // involved and keeps exactly what the caller asked.
    let out = limit_tone_sliders(0.0, [0.0, 0.0, 0.5, -1.0, 0.0]);
    assert_eq!(out[2], 0.5, "shadows were scaled for a violation they did not cause");
    assert!(
        (-0.46..=-0.44).contains(&out[3]),
        "whites did not saturate at the interval's own capacity: {}",
        out[3]
    );
    // blacks −100 OPENS the bottom interval (black point down) — no limit
    // applies to it at all, even alongside the binding whites.
    let out = limit_tone_sliders(0.0, [0.0, 0.0, 0.5, -1.0, -1.0]);
    assert_eq!(out[4], -1.0, "blacks bind nothing and must pass through untouched");
    assert_eq!(out[2], 0.5);
    // And a genuinely unconstrained vector is bit-for-bit untouched.
    let s = [0.3, -0.2, 0.4, 0.1, -0.25];
    assert_eq!(limit_tone_sliders(0.0, s), s);
}

#[test]
fn tone_lut_is_monotonic_and_keeps_midtone_separation() {
    // The reported "flat muddy water": strong opposing highlights/shadows made
    // the per-region tone curve non-monotonic and collapsed mid-bright tones
    // into one dark band. The curve must stay monotonic and keep midtones apart.
    let r = EditRecipe {
        highlights: -73.89,
        shadows: 33.28,
        whites: 6.99,
        blacks: -12.94,
        contrast: 4.68,
        ..Default::default()
    };
    let lut = build_tone_lut(&r);
    for i in 1..lut.len() {
        assert!(lut[i] >= lut[i - 1] - 1e-6, "tone LUT inverts at {i}: {} < {}", lut[i], lut[i - 1]);
    }
    // mid-bright water tones (0.50 vs 0.66) must NOT collapse to one value.
    let (a, b) = (sample_lut(&lut, 0.50), sample_lut(&lut, 0.66));
    assert!(b - a > 0.05, "midtone separation crushed flat: {a}..{b}");
    // and a true midtone (0.5) is no longer crushed deep into shadow.
    assert!(a > 0.45, "midtone water still crushed dark: {a}");
}

#[test]
fn aggressive_highlights_keep_saturated_water_from_greying() {
    // Reported bug: strong −highlights + +shadows turned bright turquoise water
    // flat grey, because the tone LUT ran per-channel and the channels converged.
    // Luminance-preserving tone must keep the cyan recognizably cyan (just darker).
    let r = EditRecipe {
        highlights: -73.89,
        shadows: 33.28,
        whites: 6.99,
        blacks: -12.94,
        contrast: 4.68,
        ..Default::default()
    };
    let cyan = [0.35_f32, 0.62, 0.66]; // mid-bright sunlit turquoise
    let mut data = vec![cyan];
    apply_develop_anon(&mut data, 1, 1, &r);
    let [rr, gg, bb] = data[0];
    // green & blue stay clearly above red → still cyan, not neutral grey.
    assert!(gg > rr + 0.08 && bb > rr + 0.08, "water greyed out: [{rr}, {gg}, {bb}]");
    // channel spread preserved (not converged toward equal = grey).
    let spread = rr.max(gg).max(bb) - rr.min(gg).min(bb);
    assert!(spread > 0.12, "channels converged toward grey: spread {spread}");
}

/// Point-curve knots from `(input, output)` pairs.
fn curve_pts(pts: &[(u8, u8)]) -> Vec<crate::recipe::CurvePoint> {
    pts.iter().map(|&(input, output)| crate::recipe::CurvePoint { input, output }).collect()
}

/// A colour in each corner of the working gamut a curve can meet, plus greys.
fn curve_probe_pixels() -> Vec<[f32; 3]> {
    vec![
        [0.85, 0.45, 0.20],
        [0.25, 0.45, 0.80],
        [0.30, 0.70, 0.35],
        [0.70, 0.30, 0.60],
        [0.50, 0.50, 0.50],
        [0.15, 0.12, 0.10],
        [0.95, 0.90, 0.80],
    ]
}

/// One point-curve law for every recipe (v1.17.0, the Camera Raw law family
/// removed): straight lines between the knots, each curve on the working
/// space's own encoded channel, right after the tone stage and before
/// Saturation — what every release before v1.17.0 rendered — and a recipe
/// that names a camera profile renders its curves the same way.
///
/// MUTATION: sample the curves with a spline, apply them in another space,
/// move them after Saturation, or key any of it on the profile.
#[test]
fn a_point_curve_is_one_law_whatever_profile_the_recipe_names() {
    let pts = curve_pts(&[(0, 0), (128, 180), (255, 255)]);
    let engine = EditRecipe { red_curve: pts.clone(), saturation: 40.0, ..EditRecipe::default() };
    let named = EditRecipe { camera_profile: "Adobe Standard".into(), ..engine.clone() };
    // Straight lines between the knots.
    let le = curve_lut(&pts);
    let x = 64.0f32 / 255.0;
    assert!((sample_lut(&le, x) - 90.0 / 255.0).abs() < 1e-3, "the chord at 64 ({})", sample_lut(&le, x));
    // The engine law: the working red alone, before Saturation.
    let (w, h) = (7usize, 1usize);
    let mut got = curve_probe_pixels();
    apply_develop_anon(&mut got, w, h, &engine);
    let mut want = curve_probe_pixels();
    for px in &mut want {
        px[0] = sample_lut(&le, px[0]);
    }
    apply_develop_anon(&mut want, w, h, &EditRecipe { saturation: 40.0, ..EditRecipe::default() });
    for (g, e) in got.iter().zip(&want) {
        for k in 0..3 {
            assert!((g[k] - e[k]).abs() < 1e-5, "engine law: {got:?} vs {want:?}");
        }
    }
    // A named profile changes nothing about the curves.
    let mut named_got = curve_probe_pixels();
    apply_develop_anon(&mut named_got, w, h, &named);
    assert_eq!(named_got, got, "the curves do not follow the profile");
}

