// One part of the engine's tests (src/render/tests.rs includes it): the lens geometry a master carries in its own file (v1.17.0) — the marker in PNG, TIFF and JPEG, the sensor frame of an unmarked master, no second resample over a marked one, and the refusal of a recipe whose lens geometry is not the one baked in.

/// A recipe under Camera Raw's law whose camera stage resamples a barrel
/// profile ([`lens_in_camera`]): its masters are lens-corrected.
fn marked_recipe() -> EditRecipe {
    EditRecipe {
        camera_profile: CANVAS_PROFILE.to_string(),
        lens_profile: crate::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0 + 0.004 * i as f32).collect(),
            distortion_on: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A written file's pixels, read back.
fn read_back(p: &Path) -> DynamicImage {
    // not-a-consumer-call: the tests' own round trip of files they wrote.
    crate::decode::load_image(p).unwrap()
}

/// Concentric rings: a picture any radial resample visibly moves.
fn ring_frame(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb16(ImageBuffer::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
        let r = (dx * dx + dy * dy).sqrt();
        let v = (((r * 0.45).sin() * 0.4 + 0.5) * 65535.0) as u16;
        Rgb([v, v / 2 + 9000, 65535 - v])
    }))
}

/// The marker rides PNG, TIFF and JPEG masters through `save_master`, reads
/// back as written, leaves the pixels and the colour profile alone, is
/// replaced rather than doubled by a second stamp, and is absent from a master
/// saved without one.
///
/// MUTATION: skip the stamp in `save_master`, or append a second marker
/// instead of replacing the first.
#[test]
fn a_master_carries_its_lens_geometry_in_png_tiff_and_jpeg() {
    let dir = crate::test_dir("baked-geometry-formats");
    let g = BakedGeometry::wanted(&marked_recipe()).expect("premise: the recipe bakes a lens");
    assert!(g.distortion && !g.ca && !g.vignette && g.corrects_frame(), "{g}");
    let other = BakedGeometry { knots: "fnv1a64:0000000000000001".into(), ..g.clone() };
    let img = ring_frame(64, 48);
    for name in ["m.png", "m.tif", "m.jpg"] {
        let out = dir.join(name);
        crate::pipeline::save_master(&out, img.clone(), Some(&g)).unwrap();
        assert_eq!(read_baked_geometry(&out).unwrap().as_ref(), Some(&g), "{name} reads back its marker");
        assert_eq!(SourceFrame::of(&out).unwrap(), SourceFrame::Baked(Some(g.clone())), "{name}");
        let back = read_back(&out);
        if !name.ends_with(".jpg") {
            assert!(back.as_bytes() == img.as_bytes(), "{name}: the marker leaves the pixels alone");
        }
        assert_eq!((back.width(), back.height()), (64, 48));
        stamp_baked_geometry(&out, &other).unwrap();
        assert_eq!(read_baked_geometry(&out).unwrap(), Some(other.clone()), "{name}: a second stamp replaces the first");
        let back2 = read_back(&out);
        assert!(back2.as_bytes() == back.as_bytes(), "{name}: restamping leaves the pixels alone");
        let bare = dir.join(format!("bare-{name}"));
        crate::pipeline::save_master(&bare, img.clone(), None).unwrap();
        assert_eq!(read_baked_geometry(&bare).unwrap(), None, "{name}: no marker, no geometry");
    }
    // The TIFF keeps its colour profile through the appended directory.
    let file = std::io::BufReader::new(std::fs::File::open(dir.join("m.tif")).unwrap());
    let mut dec = tiff::decoder::Decoder::new(file).unwrap();
    assert_eq!(dec.get_tag_u8_vec(tiff::tags::Tag::IccProfile).ok().as_deref(), Some(crate::render::SRGB_ICC));
    // PNG: exactly one marker chunk after the restamp.
    let png = std::fs::read(dir.join("m.png")).unwrap();
    let key = BAKED_GEOMETRY_KEY.as_bytes();
    assert_eq!(png.windows(key.len()).filter(|w| *w == key).count(), 1, "one marker chunk");
}

/// An UNMARKED master (every master before v1.17.0) is in the sensor frame:
/// the export resamples the recipe's lens geometry over it. A MARKED one is
/// lens-corrected already: the export takes no second resample, and renders
/// exactly what the same pixels render with no lens geometry at all.
///
/// MUTATION: ignore the marker in `render_to_file`'s baked arm, or keep the
/// tail's lens on a marked master.
#[test]
fn a_marked_master_takes_no_second_resample_and_an_unmarked_one_does() {
    let dir = crate::test_dir("baked-geometry-apply");
    let r = marked_recipe();
    let g = BakedGeometry::wanted(&r).unwrap();
    let img = ring_frame(160, 120);
    let (old, new) = (dir.join("old.png"), dir.join("new.png"));
    crate::pipeline::save_master(&old, img.clone(), None).unwrap();
    crate::pipeline::save_master(&new, img.clone(), Some(&g)).unwrap();
    let render = |src: &Path, name: &str| {
        let out = dir.join(name);
        render_to_file(src, &r, &out, None, None, crate::diag::stderr()).unwrap();
        read_back(&out).to_rgb16()
    };
    let none = crate::diag::pixels();
    let no_lens = EditRecipe { lens_profile: Default::default(), ..r.clone() };
    let as_is = render_baked_to_image(&img, &no_lens, None, None, None, &none).unwrap().to_rgb16();
    let corrected = render_baked_to_image(&img, &r, None, None, None, &none).unwrap().to_rgb16();
    assert!(as_is.as_raw() != corrected.as_raw(), "premise: the profile moves the rings");
    assert!(render(&old, "old.tif").as_raw() == corrected.as_raw(), "an unmarked master is corrected in the tail");
    assert!(render(&new, "new.tif").as_raw() == as_is.as_raw(), "a marked master is not corrected again");
}

/// A recipe that develops other lens geometry than the one baked into a
/// marked master is refused at every door — the export, the master read of a
/// heal or fill, the frame check the previews make — rather than corrected
/// twice or left with a correction it switched off. The recipe it was baked
/// with passes.
///
/// MUTATION: compare only `corrects_frame()` in `SourceFrame::check`, or drop
/// the knots digest from `BakedGeometry`.
#[test]
fn a_recipe_whose_lens_geometry_is_not_the_masters_is_refused() {
    let dir = crate::test_dir("baked-geometry-refuse");
    let r = marked_recipe();
    let g = BakedGeometry::wanted(&r).unwrap();
    let master = dir.join("m.png");
    crate::pipeline::save_master(&master, ring_frame(64, 48), Some(&g)).unwrap();
    let frame = SourceFrame::of(&master).unwrap();
    assert!(frame.check(&r).is_ok(), "the recipe it was baked with");
    assert!(master_pixels_for(&master, &BasePart::of(&r), None).is_ok_and(|m| m.corrected()));
    let mut off = r.clone();
    off.lens_profile.distortion_on = false;
    let manual = EditRecipe { lens_distortion: 12.0, ..r.clone() };
    let weaker = EditRecipe { lens_profile_distortion_scale: 50.0, ..r.clone() };
    for (case, other) in [("profile distortion off", &off), ("manual distortion", &manual), ("distortion strength 50", &weaker)] {
        let e = frame.check(other).expect_err(case).to_string();
        assert!(e.contains("bake the master again"), "{case}: {e}");
        let out = dir.join("out.tif");
        assert!(render_to_file(&master, other, &out, None, None, crate::diag::stderr()).is_err(), "{case}: the export refuses");
        assert!(master_pixels_for(&master, &BasePart::of(other), None).is_err(), "{case}: a heal over it refuses");
    }
}

/// The painted masks a heal or fill reads are SENSOR-frame; over a
/// lens-corrected master they are mapped onto its frame, and a painted dot
/// lands where the picked-point map puts its centre.
///
/// MUTATION: return the raster unmapped from `Master::rgba_to_frame`.
#[test]
fn a_sensor_frame_mask_lands_on_a_corrected_master_where_its_point_does() {
    let r = marked_recipe();
    let part = BasePart::of(&r);
    let (w, h) = (240u32, 160u32);
    let master = Master::of(ring_frame(w, h), &SourceFrame::Raw, &part).unwrap();
    assert!(master.corrected() && master.baked == part.baked_geometry());
    let at = (0.88f32, 0.2f32);
    let dot = image::RgbaImage::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - at.0 * w as f32, y as f32 + 0.5 - at.1 * h as f32);
        if dx * dx + dy * dy <= 9.0 { image::Rgba([255, 64, 64, 255]) } else { image::Rgba([0, 0, 0, 0]) }
    });
    let mapped = master.rgba_to_frame(&dot);
    let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y, p) in mapped.enumerate_pixels() {
        let a = p[3] as f32 / 255.0;
        sx += a * (x as f32 + 0.5);
        sy += a * (y as f32 + 0.5);
        n += a;
    }
    let (cx, cy) = (sx / n / w as f32, sy / n / h as f32);
    let (px, py) = master.point_to_frame(at);
    assert!((px - at.0).abs() * w as f32 > 1.0, "premise: the profile moves the point by pixels");
    assert!(
        ((cx - px) * w as f32).abs() < 0.5 && ((cy - py) * h as f32).abs() < 0.5,
        "the mapped dot's centre ({cx}, {cy}) is the mapped point ({px}, {py})"
    );
    let unmarked = Master::of(ring_frame(w, h), &SourceFrame::Baked(None), &part).unwrap();
    assert!(!unmarked.corrected() && unmarked.baked.is_none());
    assert!(unmarked.rgba_to_frame(&dot).as_raw() == dot.as_raw(), "a sensor-frame master takes the mask as painted");
}
