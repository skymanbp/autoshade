// One part of the engine's tests (src/render/tests.rs includes it): the frame a mask raster carries in its own file (v1.17.0) — the marker a reverse-fit zone raster drawn in the lens-corrected frame carries, the sensor frame of an unmarked raster, the refusal of a raster read under another lens geometry, sampling that lands a corrected raster where its sensor-frame equivalent lands, and the sidecar that cannot tell the two apart.

/// A raster any radial resample visibly moves: a soft disc off the centre,
/// where the barrel profile of [`marked_recipe`] moves the frame most.
fn off_centre_disc(w: u32, h: u32) -> image::GrayImage {
    image::GrayImage::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - 0.78 * w as f32, y as f32 + 0.5 - 0.3 * h as f32);
        let r = (dx * dx + dy * dy).sqrt() / (0.12 * w as f32);
        image::Luma([((1.0 - (r - 0.8) / 0.4).clamp(0.0, 1.0) * 255.0).round() as u8])
    })
}

/// Mean absolute difference of two equal-sized grey rasters, in codes.
fn mean_gray_diff(a: &image::GrayImage, b: &image::GrayImage) -> f32 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (*x as f32 - *y as f32).abs()).sum::<f32>() / a.as_raw().len() as f32
}

/// A corrected-frame raster is the sensor-frame one as the camera stage's
/// resample would show it.
fn corrected_twin(sensor: &image::GrayImage, r: &EditRecipe) -> image::GrayImage {
    sensor_raster_to_corrected(sensor, &geometry_profile(r))
}

/// The marker a corrected raster is written with reads back through THE entry,
/// the pixels untouched; a raster written without one reads as the sensor
/// frame; and the frame names only what moves a mask — chromatic aberration
/// and vignetting do not.
///
/// MUTATION: skip the stamp in `save_mask_raster`, or let CA into `frame_of`.
#[test]
fn a_mask_raster_carries_its_frame_and_an_unmarked_one_is_the_sensor_frame() {
    let dir = crate::test_dir("mask-raster-frame");
    let r = marked_recipe();
    let frame = RasterFrame::of(&r).expect("premise: the recipe moves the frame");
    let g = frame.marker().clone();
    assert!(g.distortion && !g.ca && !g.vignette && g.corrects_frame(), "{g}");
    let mask = off_centre_disc(96, 64);
    let (marked, plain) = (dir.join("marked.png"), dir.join("plain.png"));
    save_mask_raster(&mask, &marked, Some(&g)).unwrap();
    save_mask_raster(&mask, &plain, None).unwrap();
    let (m, p) = (open_mask_raster(&marked).unwrap(), open_mask_raster(&plain).unwrap());
    assert_eq!(m.frame.as_ref(), Some(&g), "the marked raster names its frame");
    assert_eq!(p.frame, None, "an unmarked raster is the sensor frame");
    assert!(m.image.to_luma8().as_raw() == mask.as_raw() && p.image.to_luma8().as_raw() == mask.as_raw());
    let mut with_ca = r.clone();
    with_ca.ca_r = 40.0;
    with_ca.lens_profile.vignette = vec![1.0, 1.3];
    with_ca.lens_profile.vignette_on = true;
    assert_eq!(BakedGeometry::frame_of(&with_ca).as_ref(), Some(&g), "CA and vignetting move no mask");
    assert_eq!(BakedGeometry::frame_of(&EditRecipe::default()), None, "no geometry, no frame");
}

/// A raster drawn under one lens geometry is refused under another, with both
/// named — by the check, by the develop's strict loader — and a sensor-frame
/// raster suits every recipe.
///
/// MUTATION: return `Ok` from `check_mask_frame`.
#[test]
fn a_marked_raster_under_another_geometry_is_refused_naming_both() {
    let dir = crate::test_dir("mask-raster-mismatch");
    let r = marked_recipe();
    let g = BakedGeometry::frame_of(&r).unwrap();
    let path = dir.join("zone.png");
    save_mask_raster(&off_centre_disc(64, 48), &path, Some(&g)).unwrap();
    let raster = open_mask_raster(&path).unwrap();
    raster.check(&r, &path).expect("its own geometry reads it");
    let mut other = r.clone();
    other.lens_profile_distortion_scale = 50.0;
    let other_g = BakedGeometry::frame_of(&other).unwrap();
    let e = raster.check(&other, &path).unwrap_err().to_string();
    assert!(e.contains(&g.knots) && e.contains(&other_g.knots), "both geometries named: {e}");
    let e = raster.check(&EditRecipe::default(), &path).unwrap_err().to_string();
    assert!(e.contains(&g.knots) && e.contains("no lens geometry"), "{e}");
    let plain = dir.join("plain.png");
    save_mask_raster(&off_centre_disc(64, 48), &plain, None).unwrap();
    for rr in [&r, &other, &EditRecipe::default()] {
        open_mask_raster(&plain).unwrap().check(rr, &plain).expect("a sensor-frame raster suits every recipe");
    }
    let zone = |rr: &EditRecipe| EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() },
            exposure_ev: 1.0,
            ..Default::default()
        }],
        ..rr.clone()
    };
    let refused = load_mask_raster_snapshot(&zone(&other), &crate::diag::dropped()).unwrap_err();
    assert!(format!("{refused:#}").contains(&other_g.knots), "{refused:#}");
    assert!(load_mask_raster_snapshot(&zone(&r), &crate::diag::dropped()).is_ok());
}

/// The sampling half: a raster drawn in the corrected frame covers the pixels
/// its sensor-frame equivalent covers — on a lens-corrected base
/// (`MaskFrame::Corrected`) and on a sensor-frame one the tail will resample
/// (`MaskFrame::WarpedDownstream`) — because it is read where a LINEAR's
/// handles are, through the engine's own map. Read as if it were a sensor
/// raster it lands a lens field away.
///
/// MUTATION: drop the corrected-raster arm in `mask_weight_in`.
#[test]
fn a_corrected_raster_covers_what_its_sensor_twin_covers() {
    let dir = crate::test_dir("mask-raster-sampling");
    let r = marked_recipe();
    let geom = geometry_profile(&r).into_owned();
    let (w, h) = (192u32, 128u32);
    let sensor = off_centre_disc(w, h);
    let corrected = corrected_twin(&sensor, &r);
    assert!(mean_gray_diff(&sensor, &corrected) > 1.0, "premise: the profile moves the disc");
    let (sp, cp, up) = (dir.join("s.png"), dir.join("c.png"), dir.join("u.png"));
    save_mask_raster(&sensor, &sp, None).unwrap();
    save_mask_raster(&corrected, &cp, BakedGeometry::frame_of(&r).as_ref()).unwrap();
    save_mask_raster(&corrected, &up, None).unwrap();
    let adj = |p: &Path| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: p.to_string_lossy().into_owned() },
        ..Default::default()
    };
    let reference = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([128, 128, 128])));
    for frame in [MaskFrame::Corrected { profile: &geom }, MaskFrame::WarpedDownstream { profile: &geom, amount: 0.0 }] {
        let want = mask_coverage(&adj(&sp), &reference, frame);
        let got = mask_coverage(&adj(&cp), &reference, frame);
        let unmarked = mask_coverage(&adj(&up), &reference, frame);
        let (d, du) = (mean_gray_diff(&want, &got), mean_gray_diff(&want, &unmarked));
        assert!(d < 0.6, "{frame:?}: the corrected raster lands on its sensor twin (mean {d:.3} codes)");
        assert!(du > 3.0 * d.max(0.1), "{frame:?}: read as a sensor raster it would not ({du:.3} vs {d:.3})");
    }
}

/// THE read for a surface that draws in the sensor frame (the GUI's raster
/// edits): a corrected raster comes back as its sensor twin, an unmarked one
/// as it is.
#[test]
fn a_corrected_raster_reads_back_into_the_sensor_frame() {
    let dir = crate::test_dir("mask-raster-to-sensor");
    let r = marked_recipe();
    let sensor = off_centre_disc(192, 128);
    let (cp, sp) = (dir.join("c.png"), dir.join("s.png"));
    save_mask_raster(&corrected_twin(&sensor, &r), &cp, BakedGeometry::frame_of(&r).as_ref()).unwrap();
    save_mask_raster(&sensor, &sp, None).unwrap();
    let back = open_mask_raster(&cp).unwrap().into_sensor_gray(&r, &cp).unwrap();
    let d = mean_gray_diff(&back, &sensor);
    assert!(d < 0.6, "the sensor twin, to resampling (mean {d:.3} codes)");
    assert!(open_mask_raster(&sp).unwrap().into_sensor_gray(&r, &sp).unwrap().as_raw() == sensor.as_raw());
    assert!(open_mask_raster(&cp).unwrap().into_sensor_gray(&EditRecipe::default(), &cp).is_err());
}

/// Constraint (a) of the v1.17.0 raster-frame ruling: Lightroom's mask
/// coordinates have one convention, so the sidecar a mask exports is the same
/// whether its raster is drawn in the sensor frame or in the corrected one —
/// byte for byte.
///
/// MUTATION: let the mask XML read the raster or its marker.
#[test]
fn a_corrected_raster_and_its_sensor_twin_export_the_same_mask_xml() {
    let dir = crate::test_dir("mask-raster-xmp");
    let r = marked_recipe();
    let sensor = off_centre_disc(192, 128);
    let path = dir.join("mask-zone-sky.png");
    let recipe = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::select_sky(0.62, 0.31, false, path.to_string_lossy().into_owned()),
            exposure_ev: -0.4,
            components: vec![crate::recipe::MaskComponent {
                geometry: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() },
                mode: crate::recipe::MaskCombine::Intersect,
                inverted: false,
            }],
            ..Default::default()
        }],
        ..r.clone()
    };
    save_mask_raster(&sensor, &path, None).unwrap();
    let from_sensor = crate::xmp::bare_document(&recipe, None);
    save_mask_raster(&corrected_twin(&sensor, &r), &path, BakedGeometry::frame_of(&r).as_ref()).unwrap();
    assert!(open_mask_raster(&path).unwrap().frame.is_some(), "premise: the raster is now the corrected one");
    let from_corrected = crate::xmp::bare_document(&recipe, None);
    assert!(from_sensor.contains("crs:MaskSubType=\"2\""), "premise: the Select Sky mask is exported");
    assert_eq!(from_sensor, from_corrected, "one sidecar for one mask, whatever frame its raster is drawn in");
}
