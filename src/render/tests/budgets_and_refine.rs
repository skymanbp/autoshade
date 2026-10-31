// One part of the engine's tests (src/render/tests.rs includes it): the preview probe, geometry clamps, refine and raster budgets, staged encodes and tiled guided refine.

/// Manual, machine-relative regression probe for the GUI's engine hot path.
/// Ignored in normal CI because wall-clock budgets are hardware-dependent;
/// run release-only and compare same-machine ratios:
/// `cargo test --release --lib preview_mask_perf_probe -- --ignored --nocapture`.
/// The checksum prevents a future fast path from "winning" by skipping work.
#[test]
#[ignore]
fn preview_mask_perf_probe() {
    use std::time::Instant;

    let (w, h) = (1280u32, 853u32);
    let base = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let fx = x as f32 / (w - 1) as f32;
        let fy = y as f32 / (h - 1) as f32;
        Rgb([
            (255.0 * (0.15 + 0.75 * fx)).round() as u8,
            (255.0 * (0.12 + 0.65 * fy)).round() as u8,
            (255.0 * (0.18 + 0.55 * (1.0 - fx * fy))).round() as u8,
        ])
    }));
    // Process-unique (the ./out fixture race — see fit_zoned's
    // fixture_mask_path): a concurrent `cargo test` deleted this mask
    // mid-run, turning the zone inert and the measurement meaningless.
    let mask_path =
        {
            // Own directory, not the bare temp root (see `fixture_mask_path`).
            let d = std::env::temp_dir()
                .join(format!("autoshade-preview-perf-mask-{}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            d.join("mask.png")
        };
    image::GrayImage::from_fn(w / 4, h / 4, |x, _| {
        image::Luma([((x as f32 / (w / 4 - 1) as f32) * 255.0).round() as u8])
    })
    .save(&mask_path)
    .unwrap();
    let zone = |inverted| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
        inverted,
        exposure_ev: if inverted { -0.35 } else { 0.45 },
        contrast: if inverted { 8.0 } else { -6.0 },
        saturation: if inverted { -4.0 } else { 9.0 },
        color_gains: Some(if inverted { [1.35, 0.88, 0.62] } else { [1.15, 0.96, 0.78] }),
        ..Default::default()
    };
    let no_colour_zone = |inverted| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
        inverted,
        exposure_ev: if inverted { -0.35 } else { 0.45 },
        contrast: if inverted { 8.0 } else { -6.0 },
        saturation: if inverted { -4.0 } else { 9.0 },
        ..Default::default()
    };
    let recipes = [
        ("zero", EditRecipe { exposure_ev: 0.2, saturation: 8.0, ..Default::default() }),
        ("one_no_colour", EditRecipe {
            exposure_ev: 0.2,
            saturation: 8.0,
            masks: vec![no_colour_zone(false)],
            ..Default::default()
        }),
        ("one", EditRecipe {
            exposure_ev: 0.2,
            saturation: 8.0,
            masks: vec![zone(false)],
            ..Default::default()
        }),
        ("shared_pair_no_colour", EditRecipe {
            exposure_ev: 0.2,
            saturation: 8.0,
            masks: vec![no_colour_zone(false), no_colour_zone(true)],
            ..Default::default()
        }),
        ("shared_pair", EditRecipe {
            exposure_ev: 0.2,
            saturation: 8.0,
            masks: vec![zone(false), zone(true)],
            ..Default::default()
        }),
    ];
    for (name, recipe) in recipes {
        let _ = develop_preview(&base, &recipe); // warm bitmap decode cache
        let start = Instant::now();
        let mut checksum = 0u64;
        const N: usize = 5;
        for _ in 0..N {
            let out = develop_preview(&base, &recipe).to_rgb8();
            checksum = out
                .as_raw()
                .iter()
                .step_by(997)
                .fold(checksum, |acc, &v| acc.wrapping_mul(16777619) ^ v as u64);
        }
        eprintln!(
            "PERF preview/{name}: {:.2} ms/frame checksum={checksum}",
            start.elapsed().as_secs_f64() * 1000.0 / N as f64,
        );
    }
    std::fs::remove_file(&mask_path).ok();
}

#[test]
fn render_to_file_clamps_extreme_finite_mask_geometry_before_pixel_work() {
    use crate::recipe::MaskGeometry;

    let dir = std::env::temp_dir().join(format!(
        "autoshade-render-clamp-{}-{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.png");
    DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 4, image::Rgb([120, 120, 120])))
        .save(&src)
        .unwrap();

    let wild = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear {
                zero_x: 1e30,
                zero_y: 1e30,
                full_x: -1e30,
                full_y: -1e30,
            },
            exposure_ev: 1.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut clamped = wild.clone();
    clamped.clamp();
    let wild_out = dir.join("wild.png");
    let clamped_out = dir.join("clamped.png");
    render_to_file(&src, &wild, &wild_out, None, None, crate::diag::stderr()).unwrap();
    render_to_file(&src, &clamped, &clamped_out, None, None, crate::diag::stderr()).unwrap();

    let got = image::open(&wild_out).unwrap().to_rgb16();
    let expected = image::open(&clamped_out).unwrap().to_rgb16();
    assert_eq!(got.as_raw(), expected.as_raw());
    assert!(got.as_raw().iter().all(|&channel| channel != 0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A PNG that carries ONLY its header (signature + IHDR + an empty IDAT),
/// so a fixture can claim 61 MP dimensions in 45 bytes.
///
/// Legitimate for the budget projection under test because that projection
/// is header-only BY DESIGN (`raster_projected_bytes` never decodes), and
/// necessary because the honest alternative is not free: encoding a real
/// 9504×6336 grayscale PNG measured 3.16 s and a 60 MB allocation per
/// fixture (probed on this machine), twice over in the scenario below.
fn write_header_only_png(path: &std::path::Path, w: u32, h: u32) {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_over = kind.to_vec();
        crc_over.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_over).to_be_bytes());
        out
    }
    // IHDR: width, height, 8-bit, colour type 0 (grayscale), deflate,
    // adaptive filtering, no interlace — the same shape a mask raster has.
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut file = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    file.extend_from_slice(&chunk(b"IHDR", &ihdr));
    // The dimension reader stops at the first image-data chunk HEADER, so
    // an empty IDAT is enough to make the header complete and parsable.
    file.extend_from_slice(&chunk(b"IDAT", &[]));
    std::fs::write(path, &file).expect("fixture written");
}

/// R22 H1: the mask-refine precheck must ask the question the LOADER asks —
/// the aggregate one. The refined raster is charged next to the recipe's
/// other active rasters, so the SECOND full-resolution refine on a 61 MP
/// photo is refused instead of publishing a raster that `render_to_file`
/// then bails on and `develop_preview` silently drops.
///
/// The arithmetic, spelled out (61 MP Sony A7R: 9504×6336 = 60,217,344 px):
/// one raster projects 60,217,344 × 4 = 240,869,376 B, which fits the
/// 268,435,456 B (256 MiB) budget; two project 481,738,752 B, which does
/// not. The old single-raster judgement said yes to both.
#[test]
fn a_second_full_resolution_refine_is_refused_by_the_aggregate_budget() {
    use crate::recipe::MaskGeometry;

    let dir = std::env::temp_dir().join(format!(
        "autoshade-refine-budget-{}-{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // Two masks, both active, each with its own segmentation raster; `sky`
    // is the one that gets refined to full resolution first.
    let sky_full = dir.join("mask-sky-refined.png");
    let sky_small = dir.join("mask-sky.png");
    let ground_small = dir.join("mask-ground.png");
    write_header_only_png(&sky_full, 9504, 6336);
    for p in [&sky_small, &ground_small] {
        image::GrayImage::from_pixel(64, 48, image::Luma([255])).save(p).unwrap();
    }
    let mask = |name: &str, path: &std::path::Path| LocalAdjustment {
        name: name.into(),
        mask: MaskGeometry::Bitmap { path: path.display().to_string() },
        exposure_ev: 1.0,
        ..Default::default()
    };

    // Leg 1 — refining `sky`: its own small raster is the one being
    // REPLACED, so the sum is `ground`'s 12,288 B plus the incoming
    // 240,869,376 B. Fits.
    let pre_refine = EditRecipe {
        masks: vec![mask("sky", &sky_small), mask("ground", &ground_small)],
        ..Default::default()
    };
    assert!(
        mask_raster_write_fits_budget(
            &pre_refine,
            Some(&sky_small.display().to_string()),
            9504,
            6336
        ),
        "the FIRST full-resolution refine fits: 240,881,664 B ≤ {MASK_RASTER_BUDGET_BYTES} B"
    );

    // Leg 2 — now refining `ground` while `sky` holds its 61 MP raster:
    // 240,869,376 B already committed + 240,869,376 B incoming. Refused.
    let recipe = EditRecipe {
        masks: vec![mask("sky", &sky_full), mask("ground", &ground_small)],
        ..Default::default()
    };
    assert!(
        !mask_raster_write_fits_budget(
            &recipe,
            Some(&ground_small.display().to_string()),
            9504,
            6336
        ),
        "the SECOND full-resolution refine must be refused: 481,738,752 B > \
         {MASK_RASTER_BUDGET_BYTES} B"
    );
    // And the refusal is the AGGREGATE's, not this file's: the same raster
    // alone is still fine (which is exactly why the single-raster judgement
    // said yes).
    assert!(
        mask_raster_write_fits_budget(&EditRecipe::default(), None, 9504, 6336),
        "one 61 MP raster on its own fits — the aggregate is what refuses"
    );
    // A mask the engine will not render is not charged (the loader's own
    // filter): muting `sky` frees its raster's share.
    let mut muted = recipe.clone();
    muted.masks[0].enabled = false;
    assert!(
        mask_raster_write_fits_budget(
            &muted,
            Some(&ground_small.display().to_string()),
            9504,
            6336
        ),
        "an inactive mask's raster is never loaded, so it must not be charged"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_active_raster_set_over_its_budget_is_refused_before_pixel_work() {
    use crate::recipe::MaskGeometry;

    let dir = std::env::temp_dir().join(format!(
        "autoshade-raster-budget-{}-{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let first = dir.join("first.png");
    let second = dir.join("second.png");
    image::GrayImage::from_pixel(1, 1, image::Luma([255])).save(&first).unwrap();
    image::GrayImage::from_pixel(1, 1, image::Luma([255])).save(&second).unwrap();
    let recipe = EditRecipe {
        masks: vec![
            LocalAdjustment {
                name: "first".into(),
                mask: MaskGeometry::Bitmap { path: first.display().to_string() },
                exposure_ev: 1.0,
                ..Default::default()
            },
            LocalAdjustment {
                name: "second".into(),
                mask: MaskGeometry::Bitmap { path: second.display().to_string() },
                exposure_ev: 1.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let Err(error) = load_mask_raster_snapshot_with_budget(&recipe, 1, true, &crate::diag::pixels()) else {
        panic!("two decoded bytes must exceed a one-byte snapshot budget");
    };
    assert!(
        error.to_string().contains("1-byte aggregate budget"),
        "{error:#}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_a_raster_after_snapshot_construction_does_not_change_the_render() {
    use crate::recipe::MaskGeometry;

    let dir = std::env::temp_dir().join(format!(
        "autoshade-raster-snapshot-{}-{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mask = dir.join("mask.png");
    image::GrayImage::from_pixel(2, 2, image::Luma([255])).save(&mask).unwrap();
    let recipe = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: mask.display().to_string() },
            exposure_ev: 1.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let snapshot = load_mask_raster_snapshot(&recipe, &crate::diag::pixels()).unwrap();
    let untouched = vec![[0.25, 0.25, 0.25]; 4];
    let mut before_delete = untouched.clone();
    apply_develop_with_rasters(&mut before_delete, 2, 2, &recipe, &snapshot, MaskFrame::AsRendered, FilmScale::NATIVE, false);
    std::fs::remove_file(&mask).unwrap();
    let mut after_delete = untouched.clone();
    apply_develop_with_rasters(&mut after_delete, 2, 2, &recipe, &snapshot, MaskFrame::AsRendered, FilmScale::NATIVE, false);
    assert_eq!(after_delete, before_delete);
    assert_ne!(after_delete, untouched, "the retained white mask must still apply");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_staged_encode_leaves_the_existing_target_intact() {
    let dir = std::env::temp_dir().join(format!(
        "autoshade-staged-failure-{}-{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("preview.jpg");
    std::fs::write(&target, b"previous deliverable").unwrap();

    let result = stage_and_publish(&target, |staged| {
        std::fs::write(staged, b"partial new bytes")?;
        Err(anyhow::anyhow!("encoder failed"))
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"previous deliverable");
    assert!(
        std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp."))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tiled_guided_refine_matches_the_whole_frame_result_across_every_seam() {
    let (w, h) = (53u32, 47u32);
    let tile_edge = 19usize;
    assert!(
        w as usize > 2 * tile_edge && h as usize > 2 * tile_edge,
        "the fixture must cross two tile seams in each axis"
    );

    let guide = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let base: u8 = if x + y / 3 < w / 2 { 28 } else { 210 };
        image::Rgb([
            base,
            base.saturating_add(((x * 5 + y * 3) % 17) as u8),
            base.saturating_sub(((x * 2 + y * 7) % 13) as u8),
        ])
    }));
    let small = image::GrayImage::from_fn(9, 7, |x, y| {
        image::Luma([((x * 37 + y * 53 + (x * y % 7) * 19) % 256) as u8])
    });

    let tiled = refine_mask_guided_tiled(&small, &guide, 2, 1e-2, tile_edge);
    let reference =
        refine_mask_guided_tiled(&small, &guide, 2, 1e-2, w.max(h) as usize);

    for (k, (&got, &want)) in tiled.as_raw().iter().zip(reference.as_raw().iter()).enumerate() {
        let x = k % w as usize;
        let y = k / w as usize;
        let delta = (got as i16 - want as i16).abs();
        let on_seam = (x != 0 && x.is_multiple_of(tile_edge))
            || (y != 0 && y.is_multiple_of(tile_edge));
        assert!(
            delta <= 1,
            "pixel ({x}, {y}), seam={on_seam}: tiled {got}, whole-frame {want}"
        );
    }
}

/// L14#7: the PRODUCTION tile geometry. The public entry hard-wires
/// GUIDED_REFINE_TILE_EDGE and every real caller crosses it (the GUI
/// refines at full decode resolution), yet the seam oracle above only
/// drives the parameterised internal — from its point of view the
/// shipped constant is dead code. The guide here exceeds the edge in
/// ONE axis (two real seams for ~100 K pixels; growing both axes would
/// square the cost for no added coverage), and the width is DERIVED
/// from the constant so the test keeps crossing two seams if the edge
/// ever changes. Same |delta| ≤ 1 tolerance: tiled and whole-frame
/// differ in f32 summation order, and bit-equality would be flaky
/// across targets.
#[test]
fn the_public_guided_refine_crosses_its_production_tile_seams_cleanly() {
    let w = (GUIDED_REFINE_TILE_EDGE * 2 + 52) as u32;
    let h = 48u32;
    // Slanted high-contrast edge sweeping through the first seam column,
    // plus per-channel dither — same generator family as the oracle test.
    let guide = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let base: u8 = if x + y * 11 < w / 2 { 28 } else { 210 };
        image::Rgb([
            base,
            base.saturating_add(((x * 5 + y * 3) % 17) as u8),
            base.saturating_sub(((x * 2 + y * 7) % 13) as u8),
        ])
    }));
    let small = image::GrayImage::from_fn(17, 5, |x, y| {
        image::Luma([((x * 37 + y * 53 + (x * y % 7) * 19) % 256) as u8])
    });
    // eps matches the sole production caller (gui/masks.rs).
    let public = refine_mask_guided(&small, &guide, 2, 1e-4);
    let whole = refine_mask_guided_tiled(&small, &guide, 2, 1e-4, w.max(h) as usize);
    for (k, (&got, &want)) in public.as_raw().iter().zip(whole.as_raw().iter()).enumerate() {
        let x = k % w as usize;
        let y = k / w as usize;
        let delta = (got as i16 - want as i16).abs();
        let on_seam = x != 0 && x.is_multiple_of(GUIDED_REFINE_TILE_EDGE);
        assert!(
            delta <= 1,
            "pixel ({x}, {y}), production-seam-column={on_seam}: public {got}, \
             whole-frame {want}"
        );
    }
}

/// The public entry's OWN guards, unreachable through the internal fn
/// the seam tests drive: a hostile eps (NaN / negative / zero) is
/// floored to 1e-6 — not divided by (near-)zero variance and quantised
/// to black — and a 0×0 guide returns the mask unchanged.
#[test]
fn the_public_guided_refine_floors_a_hostile_eps() {
    let (w, h) = (64u32, 32u32);
    let guide = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
        if x < w / 2 { image::Rgb([20, 20, 20]) } else { image::Rgb([220, 220, 220]) }
    }));
    let small = image::GrayImage::from_fn(9, 5, |x, y| {
        image::Luma([((x * 41 + y * 59) % 256) as u8])
    });
    let floored = refine_mask_guided(&small, &guide, 2, 1e-6);
    assert!(
        floored.as_raw().iter().any(|&p| p > 0),
        "premise: the floored reference is not all black"
    );
    for bad in [f32::NAN, -1.0, 0.0] {
        let got = refine_mask_guided(&small, &guide, 2, bad);
        assert_eq!(
            got.as_raw(),
            floored.as_raw(),
            "eps {bad} must be floored to 1e-6, not quantise NaN to black"
        );
    }
    let empty = refine_mask_guided(&small, &DynamicImage::ImageRgb8(RgbImage::new(0, 0)), 2, 1e-4);
    assert_eq!(empty.as_raw(), small.as_raw(), "a 0x0 guide returns the mask unchanged");
}
