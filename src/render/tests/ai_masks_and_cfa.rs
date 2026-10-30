// One part of the engine's tests (src/render/tests.rs includes it): export depth, AI masks under turns and the CFA-geometry demosaic.

/// 阶段4: the export DEPTH is an ExportOpts setting, not an extension
/// property — a .png/.tif carries 16 bits by default and 8 on request,
/// and the staged publish keeps the contract for both.
#[test]
fn export_depth_follows_the_option_not_the_extension() {
    let dir = std::env::temp_dir().join(format!(
        "autoshade-export-depth-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let src_dir = dir.join("library");
    let out_dir = dir.join("exports");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&out_dir).unwrap();
    let src = src_dir.join("in.png");
    RgbImage::from_fn(12, 8, |x, y| image::Rgb([x as u8 * 20, y as u8 * 30, 90]))
        .save(&src)
        .unwrap();
    let recipe = EditRecipe::default();
    for (name, eight, want8) in [
        ("d16.png", false, false),
        ("d8.png", true, true),
        ("d16.tif", false, false),
        ("d8.tif", true, true),
    ] {
        let out = out_dir.join(name);
        let opts = ExportOpts { eight_bit: eight, ..Default::default() };
        render_to_file(&src, &recipe, &out, None, Some(&opts), crate::diag::stderr()).unwrap();
        let img = image::open(&out).unwrap();
        let got8 = matches!(img.color(), image::ColorType::Rgb8 | image::ColorType::Rgba8);
        assert_eq!(
            got8, want8,
            "{name}: eight_bit={eight} must decide the stored depth, got {:?}",
            img.color()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An AI mask with no recomputed alpha must SKIP its adjustment, not
/// render it at weight 0.
///
/// The distinction is not academic — it is the difference between "this
/// mask does nothing" and "this mask does everything". `LocalAdjustment`
/// composes coverage with `inverted` as `1 - w`, so a zero-coverage mask
/// under `inverted: true` applies the edit to the ENTIRE frame. That is
/// exactly the silent-zero failure the AI arm is not allowed to have, and
/// `is_raster_backed` is what separates "needs pixels and has none" from
/// "needs no pixels".
///
/// MUTATION-LINED. Verified red by reverting the two `is_raster_backed`
/// call sites to `matches!(…, MaskGeometry::Bitmap { .. })` (transcript in
/// the batch report): the inverted arm's assertion fails because the
/// unresolved AI mask brightens the whole frame.
#[test]
fn an_unresolved_ai_mask_skips_its_adjustment_instead_of_covering_the_frame() {
    let ai = |inverted: bool| crate::recipe::LocalAdjustment {
        name: "Sky".into(),
        inverted,
        exposure_ev: 3.0,
        mask: MaskGeometry::AiMask {
            name: "Sky 1".into(),
            subtype: 2,
            ref_x: 0.5,
            ref_y: 0.3,
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            mask_version: 1,
            provenance: Vec::new(),
            gesture: Vec::new(),
            // NOT resolved: the segmenter has not run, or declined.
            raster: None,
        },
        ..Default::default()
    };
    // The two questions the weight loop asks, answered directly — the same
    // pair `apply_masks` and `mask_coverage` both consult.
    let g = &ai(false).mask;
    assert!(is_raster_backed(g), "an AI mask draws from a raster");
    assert!(geometry_raster_path(g).is_none(), "…and it has none yet");
    // With no bitmap, the weight is 0 everywhere — which is exactly why the
    // caller must skip rather than invert it.
    assert_eq!(mask_weight(g, 0.5, 0.3, None), 0.0);
    assert_eq!(mask_weight(g, 0.1, 0.9, None), 0.0);

    // The visible consequence, through the public coverage preview: an
    // unresolved AI mask advertises NO coverage in either polarity. Under
    // the old Bitmap-only test the inverted one would have advertised the
    // whole frame.
    let reference = DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
    for inverted in [false, true] {
        let cov = mask_coverage(&ai(inverted), &reference, MaskFrame::AsRendered);
        let lit = cov.pixels().filter(|p| p.0[0] > 0).count();
        assert_eq!(
            lit, 0,
            "an unresolved AI mask must advertise nothing (inverted={inverted})"
        );
    }

    // And once an alpha EXISTS the geometry samples it like any raster.
    let dir = std::env::temp_dir().join(format!("autoshade-ai-render-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("alpha.png");
    let mut img = image::GrayImage::new(4, 4);
    for px in img.pixels_mut() {
        px.0[0] = 255;
    }
    img.save(&p).unwrap();
    let mut resolved = ai(false);
    if let MaskGeometry::AiMask { raster, .. } = &mut resolved.mask {
        *raster = Some(p.to_string_lossy().into_owned());
    }
    assert_eq!(
        geometry_raster_path(&resolved.mask),
        Some(p.to_string_lossy().as_ref()),
        "the resolved alpha is the geometry's raster"
    );
    let bmp = load_mask_bitmap(&resolved.mask, &crate::diag::pixels()).expect("the alpha must load");
    assert_eq!(bmp.frame, None, "a segmentation alpha is drawn in the sensor frame");
    assert_eq!(mask_weight(&resolved.mask, 0.5, 0.5, Some(&bmp.gray)), 1.0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A reverse-fit ZONE renders through its Select Sky component EXACTLY as
/// it rendered through the `MaskGeometry::Bitmap` that carrier replaced —
/// byte for byte, upright and inverted.
///
/// The change was made for the SIDECAR (classic XMP has no encoding for a
/// raster mask, so both zone corrections used to be skipped with a named
/// `MaskLossReason::Bitmap`), and the one thing it must not do is move a
/// pixel of what the photographer already sees. Both carriers hold the
/// same claimed PNG and `mask_weight` samples them through the same
/// `sample_gray_norm`, so equality here is the proof that nothing about
/// the correction changed except the form it is written down in.
///
/// The INVERTED arm is THE TRIPWIRE FOR THE NET BEING APPLIED EXACTLY
/// ONCE. A zone spells its inversion in ONE home — `LocalAdjustment::
/// inverted`, the flag the weight loop reads — and the Select Sky
/// component's own bit stays `false`, so
/// [`crate::recipe::LocalAdjustment::net_inverted`] is `true` once and the
/// render inverts once. The AI arm of `mask_weight` honours the geometry's
/// bit now (it read no inversion at all before this change); if the zone
/// ever went back to spelling the fact in both homes, the land zone would
/// invert TWICE, cover the sky it excludes, and this assertion is what
/// says so.
///
/// MUTATION: give `select_sky` below the same `inverted` the adjustment
/// carries — the two-home spelling this replaced — and the `inverted` pass
/// fails.
#[test]
fn a_zone_ai_mask_renders_exactly_like_the_bitmap_it_replaced() {
    let dir =
        std::env::temp_dir().join(format!("autoshade-zone-ai-render-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("mask-zone-sky.png");
    // A real partition with a soft edge, not a flat mask: a flat one
    // renders identically under any sampling and could not witness a
    // coverage difference between the two carriers.
    image::GrayImage::from_fn(16, 16, |_, y| {
        image::Luma([match y {
            0..=5 => 255,
            6 => 160,
            7 => 64,
            _ => 0,
        }])
    })
    .save(&p)
    .unwrap();
    let path = p.to_string_lossy().into_owned();
    let src = DynamicImage::ImageRgb8(image::RgbImage::from_fn(24, 16, |x, y| {
        image::Rgb([40 + (x * 7 % 180) as u8, 60 + (y * 5 % 160) as u8, 120])
    }));
    let plain = develop_preview(&src, &EditRecipe::default()).to_rgb8().into_raw();
    for (inverted, role) in [
        (false, crate::recipe::MaskRole::ZoneSky),
        (true, crate::recipe::MaskRole::ZoneLand),
    ] {
        let zone = |mask: MaskGeometry| EditRecipe {
            masks: vec![crate::recipe::LocalAdjustment {
                mask,
                role,
                inverted,
                exposure_ev: -0.7,
                saturation: 18.0,
                color_gains: Some([1.12, 0.98, 0.87]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let was = develop_preview(&src, &zone(MaskGeometry::Bitmap { path: path.clone() }))
            .to_rgb8()
            .into_raw();
        let now = develop_preview(
            &src,
            // `false`, not `inverted`: the correction's flag above is the
            // zone's ONE home for it (`fit_zoned`'s `land_attachment`).
            &zone(MaskGeometry::select_sky(0.5, 0.2, false, path.clone())),
        )
        .to_rgb8()
        .into_raw();
        assert_eq!(
            was, now,
            "inverted={inverted}: the Select Sky carrier must render the bitmap's own pixels"
        );
        // Premise: the correction is not a no-op, so the equality above is
        // not two identical copies of the untouched frame.
        assert_ne!(
            plain, now,
            "inverted={inverted}: the zone must actually change the render"
        );
    }
    // …and the two polarities are not each other, which is what makes the
    // inverted arm a real second case.
    let arm = |inverted: bool| {
        develop_preview(
            &src,
            &EditRecipe {
                masks: vec![crate::recipe::LocalAdjustment {
                    mask: MaskGeometry::select_sky(0.5, 0.2, false, path.clone()),
                    role: crate::recipe::MaskRole::ZoneSky,
                    inverted,
                    exposure_ev: -0.7,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .to_rgb8()
        .into_raw()
    };
    assert_ne!(arm(false), arm(true), "the inversion must reach the pixels");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Native Lightroom inversion remains a component-owned bit, and must
/// render the complement of a graded alpha. AutoShade's optional editing
/// metadata instead restores the authored inversion home. A native edit
/// that contradicts that metadata wins; neither path may invert twice.
///
/// The native-only pair still differs in exactly MaskInverted. The
/// authored pair also differs in ash:Inverted, deliberately, and both
/// inconsistent pairs test that stale editing intent cannot veto CRS.
/// Dropping the AI geometry's own bit or applying either bit twice must
/// still fail the exact coverage assertions below.
#[test]
fn ai_mask_import_preserves_authored_inversion_and_native_edits_win() {
    let dir =
        std::env::temp_dir().join(format!("autoshade-ai-import-inv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("alpha.png");
    // A GRADED alpha: a flat one cannot tell `1 - w` from a constant.
    image::GrayImage::from_fn(8, 8, |x, _| image::Luma([(x * 32) as u8])).save(&p).unwrap();
    let path = p.to_string_lossy().into_owned();

    // The export, from the zone's own one-home shape: the correction
    // carries the flag, the component carries `false`, and the sidecar
    // gets their net on the component — which is the only place Lightroom
    // has for it.
    let recipe = |inverted: bool| EditRecipe {
        masks: vec![crate::recipe::LocalAdjustment {
            mask: MaskGeometry::select_sky(0.5, 0.25, false, path.clone()),
            role: crate::recipe::MaskRole::ZoneLand,
            inverted,
            exposure_ev: -0.5,
            ..Default::default()
        }],
        ..Default::default()
    };
    // The PROJECTION (payload-free, v1.3.1): the native bit and the `ash`
    // intent are the two channels this test is about. The payload is a
    // third, and the whole documents below put it through the same rows.
    let exported = |inverted: bool| crate::xmp::bare_document(&recipe(inverted), None);
    let authored_inv = exported(true);
    let authored_up = exported(false);
    assert!(
        authored_inv.contains(r#"crs:What="Mask/Image""#)
            && authored_inv.contains(r#"crs:MaskInverted="true""#),
        "premise: the fixture must BE an inverted Lightroom AI mask:\n{authored_inv}"
    );
    let without_intent = |mut text: String| {
        while let Some(start) = text.find(" ash:") {
            let value = start + text[start..].find("=\"").unwrap() + 2;
            let end = value + text[value..].find('"').unwrap() + 1;
            text.replace_range(start..end, "");
        }
        text
    };
    let inv_doc = without_intent(authored_inv.clone());
    let up_doc = inv_doc.replace(r#"crs:MaskInverted="true""#, r#"crs:MaskInverted="false""#);
    assert_eq!(up_doc, without_intent(authored_up.clone()),
        "the native-only exports differ in the inversion attribute and nothing else");
    assert_eq!(
        authored_inv.replace(r#"crs:MaskInverted="true""#, r#"crs:MaskInverted="false""#)
            .replace(r#"ash:Inverted="true""#, r#"ash:Inverted="false""#),
        authored_up,
        "the authored exports also record the inversion home"
    );
    let edited_up = authored_inv.replace(r#"crs:MaskInverted="true""#, r#"crs:MaskInverted="false""#);
    let edited_inv = authored_up.replace(r#"crs:MaskInverted="false""#, r#"crs:MaskInverted="true""#);
    // The whole documents (v1.3.1): the payload restores the authored home
    // when the net Lightroom hands back is the net that was written — the
    // intent still there, or stripped the way Lightroom strips it — and
    // yields to a native edit of the net, in Lightroom's home.
    let whole = |inverted: bool| crate::xmp::recipe_to_xmp(&recipe(inverted));
    let (whole_inv, whole_up) = (whole(true), whole(false));
    let (rewritten_inv, rewritten_up) =
        (without_intent(whole_inv.clone()), without_intent(whole_up.clone()));
    let rewritten_edited_up =
        rewritten_inv.replace(r#"crs:MaskInverted="true""#, r#"crs:MaskInverted="false""#);
    let rewritten_edited_inv =
        rewritten_up.replace(r#"crs:MaskInverted="false""#, r#"crs:MaskInverted="true""#);
    let bmp = image::open(&p).unwrap().to_luma8();
    for (attr, text, want_inverted, want_whole) in [
        ("native inverted", &inv_doc, true, false),
        ("native upright", &up_doc, false, false),
        ("authored inverted", &authored_inv, true, true),
        ("authored upright", &authored_up, false, false),
        ("native edit upright", &edited_up, false, false),
        ("native edit inverted", &edited_inv, true, false),
        ("payload inverted", &whole_inv, true, true),
        ("payload upright", &whole_up, false, false),
        ("payload, intent stripped", &rewritten_inv, true, true),
        ("payload, native edit upright", &rewritten_edited_up, false, false),
        ("payload, native edit inverted", &rewritten_edited_inv, true, false),
    ]
    {
        let back = crate::xmp::xmp_to_recipe(text);
        assert_eq!(back.masks.len(), 1, "{attr}: {:?}", back.masks);
        let m = &back.masks[0];
        let want_component = want_inverted ^ want_whole;
        assert_eq!(m.inverted, want_whole, "{attr}: preserve only consistent authored intent");
        assert_eq!(m.mask.own_inverted(), want_component, "{attr}: the native fallback owns its bit");
        assert_eq!(m.net_inverted(), want_inverted, "{attr}: the native net wins");
        // THE RENDER. `raster` is `None` on import — the sidecar carries
        // the intent, never Adobe's pixels — so point it at the alpha this
        // engine would recompute and sample the geometry directly.
        let mut g = m.mask.clone();
        // The SLOT, not `geometry_raster_path_mut`: that helper repoints a
        // raster a mask already has, and an imported AI mask has none —
        // which is the state this is standing in for the segmenter to fill.
        let MaskGeometry::AiMask { raster, .. } = &mut g else {
            panic!("{attr}: expected the Select Sky component back, got {g:?}");
        };
        *raster = Some(path.clone());
        for (nx, ny) in [(0.1f32, 0.5f32), (0.4, 0.5), (0.9, 0.5)] {
            let upright = sample_gray_norm(&bmp, nx, ny);
            let want = if want_inverted { 1.0 - upright } else { upright };
            assert_eq!(mask_weight(&g, nx, ny, Some(&bmp)),
                if want_component { 1.0 - upright } else { upright },
                "{attr}: the geometry honours its own bit exactly once");
            let composed = combined_mask_weight(m, nx, ny, Some(Bmp::sensor(&bmp)), &[], None, (8.0, 8.0));
            assert_eq!(
                if m.inverted { 1.0 - composed } else { composed },
                want,
                "{attr}: at ({nx}, {ny}) the weight must be the {} alpha",
                if want_inverted { "complement of the" } else { "upright" }
            );
        }
        // Premise for the pair: the alpha is not its own complement here.
        assert_ne!(
            sample_gray_norm(&bmp, 0.1, 0.5),
            1.0 - sample_gray_norm(&bmp, 0.1, 0.5),
            "the fixture must be able to witness an inversion"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `coord_era` migration turns an AI mask's reference point and DROPS
/// its cached alpha — the raster was segmented in the old frame, so
/// rotating it is a re-render, not a coordinate migration.
///
/// MUTATION: leave `*raster` alone in `orient_recipe_coords`' AiMask arm
/// and the second assert fails — the mask would then render the OLD
/// frame's selection over the turned pixels.
#[test]
fn turning_the_frame_moves_the_ai_click_and_invalidates_its_cached_alpha() {
    let mut r = EditRecipe {
        coord_era: 0, // the LEGACY era — what the migration acts on
        masks: vec![crate::recipe::LocalAdjustment {
            exposure_ev: 1.0,
            mask: MaskGeometry::AiMask {
                name: "Sky 1".into(),
                subtype: 2,
                ref_x: 0.25,
                ref_y: 0.10,
                blend_mode: 0,
                value: 1.0,
                inverted: false,
                mask_version: 1,
                provenance: Vec::new(),
                gesture: Vec::new(),
                raster: Some("stale.png".into()),
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        recipe_has_frame_coords(&r),
        "an AI mask's reference point IS a frame coordinate the migration moves"
    );
    let turned = orient_recipe_coords(&mut r, rawler::Orientation::Rotate90, probe_frame());
    assert!(turned, "the migration ran");
    let MaskGeometry::AiMask { ref_x, ref_y, raster, .. } = &r.masks[0].mask else {
        panic!("the geometry must survive the migration");
    };
    assert_ne!((*ref_x, *ref_y), (0.25, 0.10), "the click moved with the frame");
    assert!(raster.is_none(), "the alpha from the OLD frame must not be reused");
}

/// …and the ZONE masks are the one exception, because their alpha is not a
/// cache of anything.
///
/// The zoned reverse-fit renders that PNG itself, claims it under a unique
/// name and measures the zone's exposure, gains and saturation against it;
/// no re-segmentation reproduces it from the recipe. Dropping it on a
/// rotate would leave both zone corrections inert until a model run — and
/// inert forever on a machine with no segmentation sidecar, i.e. the
/// photographer's edit silently gone. So it is KEPT here and turned by
/// `pipeline::rotate_recipe` phase 1 with the `Bitmap` rasters
/// (`LocalAdjustment::turnable_raster_paths_mut`), and by the `coord_era`
/// migration since v1.6.0 (`pipeline::migrate_raster_file`).
///
/// MUTATION: drop the `owned_alpha` guard in `orient_recipe_coords`' AiMask
/// arm and the `raster` assertion fails.
#[test]
fn turning_the_frame_keeps_a_zone_masks_own_alpha_and_turns_it() {
    let mut r = EditRecipe {
        coord_era: 0,
        masks: vec![crate::recipe::LocalAdjustment {
            exposure_ev: 1.0,
            role: crate::recipe::MaskRole::ZoneSky,
            mask: MaskGeometry::select_sky(0.25, 0.10, false, "mask-zone-sky.png".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(
        r.masks[0].turnable_raster_paths_mut(),
        vec![&mut "mask-zone-sky.png".to_string()],
        "and it is in the walk `rotate_recipe` turns"
    );
    assert!(orient_recipe_coords(&mut r, rawler::Orientation::Rotate90, probe_frame()));
    let MaskGeometry::AiMask { ref_x, ref_y, raster, .. } = &r.masks[0].mask else {
        panic!("the geometry must survive the migration");
    };
    assert_ne!((*ref_x, *ref_y), (0.25, 0.10), "the click still moves with the frame");
    assert_eq!(
        raster.as_deref(),
        Some("mask-zone-sky.png"),
        "the fit's own alpha is kept for phase 1 to turn, never dropped"
    );
}

// ---- R28 Batch-1 1a: the CFA-geometry demosaic ------------------------

/// The X-S10's 6×6 X-Trans tile, read back from the zoo RAF's OWN
/// `XTransLayout` (0x0131) record rather than from rawler's camera DB —
/// the RAF decoder prefers the file's copy (`decoders/raf.rs:257-263`),
/// and for this body the two turned out identical (measured 2026-08-20,
/// `AUTOSHADE_RAW_ZOO` probe, alongside `active_area = 6252×4176 @ (0,5)`
/// and `crop_area = 6240×4160 @ (6,13)`).
const XTRANS_XS10: &str = "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG";

/// 20 green / 8 red / 8 blue in four 2×2 all-green blocks plus four
/// isolated greens — the structure every claim below rests on.
#[test]
fn the_x_trans_tile_is_four_green_blocks_and_four_isolated_greens() {
    use rawler::cfa::{CFA, CFA_COLOR_B, CFA_COLOR_G, CFA_COLOR_R};
    let cfa = CFA::new(XTRANS_XS10);
    assert_eq!((cfa.width, cfa.height), (6, 6));
    let count = |ch: usize| (0..36).filter(|i| cfa.color_at(i / 6, i % 6) == ch).count();
    assert_eq!((count(CFA_COLOR_R), count(CFA_COLOR_G), count(CFA_COLOR_B)), (8, 20, 8));
    // A green whose right AND down neighbours are both green is the
    // top-left corner of a 2×2 block; there are four of them.
    let blocks = (0..36)
        .filter(|i| {
            let (r, c) = (i / 6, i % 6);
            cfa.color_at(r, c) == CFA_COLOR_G
                && cfa.color_at(r, c + 1) == CFA_COLOR_G
                && cfa.color_at(r + 1, c) == CFA_COLOR_G
        })
        .count();
    assert_eq!(blocks, 4, "X-Trans is defined by its four 2×2 green blocks per tile");
}

/// WHY this file has its own demosaic, pinned from the pattern alone.
///
/// rawler's `interpolate_rb_at_green` (`imgop/sensor/bayer/ppg.rs:185-203`)
/// fills a green photosite's two missing channels from exactly
/// `(row, col+1)` and `(row+1, col)`, on the Bayer axiom that those two
/// carry the two DIFFERENT chroma colours. Every time a neighbour is green
/// instead, the write lands back on the green channel and that chroma
/// value is never written by any pass — `interpolate_rb_at_non_green`
/// (`ppg.rs:220-252`) is gated on `color_at != G` and never revisits a
/// green site.
///
/// The four 2×2 blocks contribute 2 + 1 + 1 failures each (top-left corner
/// both ways, top-right down, bottom-left right), so **16 chroma values
/// per 36-pixel tile are lost**, split 8 R / 8 B by the tile's R↔B duality.
/// Confirmed on the real X-S10 RAF before the fix: binning the 6252×4176
/// camera-native demosaic by `(row mod 6, col mod 6)` gave R = 0.0 at
/// 99.8 % of the pixels in 8 of the 36 phases and B = 0.0 in a different 8,
/// green in none — the residual 0.2 % being the 3-px ring that upstream's
/// CFA-correct border pass (`ppg.rs:74-110`) does reach.
///
/// Kept as a PATTERN assertion rather than a call into `PPGDemosaic`
/// deliberately: on a 6×6 CFA those passes read the buffer they are
/// concurrently writing through `Color2DPtr` (`pixarray.rs:500-529`), so
/// running them here would put a genuine data race in the suite to assert
/// on its output.
///
/// MUTATION THIS CATCHES: a future rawler that fixes X-Trans does not make
/// this red — it makes [`demosaic_over_cfa_geometry`] redundant, which is
/// a decision, not a regression. What it pins is the arithmetic behind the
/// 16, so nobody re-derives it from memory.
#[test]
fn the_bayer_chroma_axiom_fails_sixteen_times_per_x_trans_tile() {
    use rawler::cfa::{CFA, CFA_COLOR_G};
    let cfa = CFA::new(XTRANS_XS10);
    let lost: usize = (0..36)
        .map(|i| {
            let (r, c) = (i / 6, i % 6);
            if cfa.color_at(r, c) != CFA_COLOR_G {
                return 0;
            }
            usize::from(cfa.color_at(r, c + 1) == CFA_COLOR_G)
                + usize::from(cfa.color_at(r + 1, c) == CFA_COLOR_G)
        })
        .sum();
    assert_eq!(lost, 16, "the Bayer chroma axiom must fail 16 times per X-Trans tile");
}

/// The fix, on the case the defect destroyed: a flat colour must survive
/// EVERY CFA phase exactly. On the pre-fix path 16 of every 36 pixels came
/// out with a channel at 0.0 or half its value.
///
/// The ROI deliberately starts at neither the frame origin nor a tile
/// boundary — the X-S10's active area starts at `y = 5` (measured) — so a
/// demosaic that forgot to shift the pattern by the ROI origin writes the
/// measured photosite into the wrong channel and this goes red at the
/// first pixel.
///
/// MUTATION THIS CATCHES: drop `cfa.shift(roi.x(), roi.y())`; swap the R
/// and B tap sets; index the source plane from the frame origin instead of
/// the ROI's; mirror instead of fold at the border (the outer ring then
/// samples the wrong colour).
#[test]
fn the_cfa_geometry_demosaic_is_exact_on_flat_colour_at_every_phase() {
    use rawler::cfa::CFA;
    use rawler::imgop::{Dim2, Point, Rect};
    let cfa = CFA::new(XTRANS_XS10);
    let truth = [0.2f32, 0.5, 0.3];
    let (pw, ph) = (72usize, 72usize);
    let roi = Rect::new(Point::new(3, 5), Dim2::new(60, 60));
    let plane: Vec<f32> =
        (0..pw * ph).map(|i| truth[cfa.color_at(i / pw, i % pw)]).collect();
    let out = demosaic_over_cfa_geometry(&plane, Dim2::new(pw, ph), &cfa, roi);
    assert_eq!(out.len(), roi.width() * roi.height());
    let mut sums = [[0.0f64; 3]; 36];
    for (i, px) in out.iter().enumerate() {
        let (row, col) = (i / roi.width(), i % roi.width());
        for ch in 0..3 {
            assert!(
                (px[ch] - truth[ch]).abs() < 1e-5,
                "phase ({}, {}) channel {ch} came out {} instead of {}",
                row % 6,
                col % 6,
                px[ch],
                truth[ch]
            );
            sums[(row % 6) * 6 + col % 6][ch] += px[ch] as f64;
        }
    }
    // The whole-frame statement of the same thing, and the shape the
    // defect was originally measured in: 36 phase means per channel, which
    // must not spread at all on a flat field.
    let n = (out.len() / 36) as f64;
    for ch in 0..3 {
        let means: Vec<f64> = sums.iter().map(|s| s[ch] / n).collect();
        let spread = means.iter().cloned().fold(f64::MIN, f64::max)
            - means.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread < 1e-5, "channel {ch} spreads {spread} across the 36 CFA phases");
    }
}

/// …and on a LINEAR gradient, which is what makes the taps a plane fit
/// rather than a distance-weighted mean. Measured on this same synthetic
/// (60×60, interior): a mean's per-phase R/G ratio spreads by 7.2e-3 — a
/// 1.8 % chroma modulation at the tile period, i.e. visible fixed-pattern
/// chroma on a sky — where the plane fit spreads by 3.3e-16.
///
/// Interior only: outside `CfaTaps::radius` of the ROI edge the window
/// folds back into the frame by whole CFA repeats, which is colour-correct
/// but not linear.
#[test]
fn the_cfa_geometry_demosaic_is_exact_on_a_linear_gradient() {
    use rawler::cfa::CFA;
    use rawler::imgop::{Dim2, Point, Rect};
    let cfa = CFA::new(XTRANS_XS10);
    let (pw, ph) = (72usize, 72usize);
    let roi = Rect::new(Point::new(3, 5), Dim2::new(60, 60));
    let truth = |row: usize, col: usize| {
        let t = 0.1 + 0.6 * col as f32 / 71.0 + 0.2 * row as f32 / 71.0;
        [0.4 * t, t, 0.6 * t]
    };
    let plane: Vec<f32> = (0..pw * ph)
        .map(|i| truth(i / pw, i % pw)[cfa.color_at(i / pw, i % pw)])
        .collect();
    let out = demosaic_over_cfa_geometry(&plane, Dim2::new(pw, ph), &cfa, roi);
    let guard = cfa.width.max(cfa.height);
    for (i, px) in out.iter().enumerate() {
        let (row, col) = (i / roi.width(), i % roi.width());
        if row < guard || col < guard || row + guard >= roi.height() || col + guard >= roi.width()
        {
            continue;
        }
        let want = truth(roi.y() + row, roi.x() + col);
        for ch in 0..3 {
            assert!(
                (px[ch] - want[ch]).abs() < 2e-5,
                "({row}, {col}) channel {ch}: {} vs {}",
                px[ch],
                want[ch]
            );
        }
    }
}

/// Every tap set is a normalised, NON-NEGATIVE combination — the two
/// properties the doc comment's quality claims rest on. Sum = 1 is what
/// makes flat colour exact; non-negativity makes each estimate a convex
/// combination of real samples of that colour, so it cannot leave their
/// range and cannot ring. The second is a MEASURED property of this tile
/// at radius 2 (worst tap +0.056 over all 108 sets), not a theorem about
/// least squares — a different geometry may well need the guarantee
/// dropped, and this is where that would be noticed.
#[test]
fn every_cfa_tap_set_is_a_normalised_convex_combination() {
    use rawler::cfa::CFA;
    let cfa = CFA::new(XTRANS_XS10);
    let taps = cfa_taps(&cfa);
    assert_eq!(taps.per_phase.len(), 6 * 6 * 3);
    assert!(taps.radius >= 6, "the fallback window must span a whole repeat");
    for (i, set) in taps.per_phase.iter().enumerate() {
        let (phase, ch) = (i / 3, i % 3);
        assert!(!set.is_empty(), "phase {phase} channel {ch} has no sample at all");
        let sum: f32 = set.iter().map(|t| t.2).sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "phase {phase} channel {ch} weights sum to {sum}"
        );
        let worst = set.iter().map(|t| t.2).fold(f32::MAX, f32::min);
        assert!(worst >= 0.0, "phase {phase} channel {ch} has a negative tap {worst}");
    }
}

/// Out-of-frame taps fold by whole CFA repeats, so an offset keeps its
/// COLOUR. A mirror or a clamp puts a different colour under it and
/// reintroduces the channel error the whole path exists to remove.
#[test]
fn out_of_frame_taps_fold_by_whole_cfa_repeats() {
    let t = wrap_table(60, 6, 6);
    assert_eq!(t.len(), 60 + 12);
    for (i, &src) in t.iter().enumerate() {
        let logical = i as isize - 6;
        assert!(src < 60, "index {logical} folded to {src}, outside the frame");
        assert_eq!(
            src % 6,
            logical.rem_euclid(6) as usize,
            "index {logical} folded to {src} and changed colour"
        );
    }
    // A frame that is not a whole number of repeats folds by the largest
    // whole span inside it (54 of 57 rows here), never by the frame size.
    let t = wrap_table(57, 6, 6);
    for (i, &src) in t.iter().enumerate() {
        let logical = i as isize - 6;
        assert!(src < 57);
        assert_eq!(src % 6, logical.rem_euclid(6) as usize, "{logical} -> {src}");
    }
}

/// The dispatch is GEOMETRIC. Every Bayer spelling stays on rawler's own
/// develop (byte-identical: all eight non-Fuji zoo renders hashed the same
/// before and after this change), a 4-colour array is not ours to touch,
/// and only a non-2×2 RGB repeat takes the new path.
#[test]
fn only_a_non_two_by_two_rgb_cfa_takes_the_geometry_path() {
    use rawler::cfa::CFA;
    for bayer in ["RGGB", "BGGR", "GRBG", "GBRG"] {
        assert!(
            !cfa_needs_geometry_demosaic(&CFA::new(bayer)),
            "{bayer} is a 2×2 quincunx and must keep rawler's PPG"
        );
    }
    assert!(!cfa_needs_geometry_demosaic(&CFA::new("RGBE")), "4-colour is refused earlier");
    assert!(cfa_needs_geometry_demosaic(&CFA::new(XTRANS_XS10)));
}
