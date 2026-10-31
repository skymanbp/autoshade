// One part of the engine's tests (src/render/tests.rs includes it): the lens geometry a file carries in its own metadata (v1.17.0) — the marker in PNG, TIFF and JPEG, the sensor frame of an unmarked master, and the refusal of a marked one.

/// A recipe whose lens profile moves the frame (a barrel distortion): the
/// geometry a marked file names ([`BakedGeometry::frame_of`]).
fn marked_recipe() -> EditRecipe {
    EditRecipe {
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
    let g = BakedGeometry::frame_of(&marked_recipe()).expect("premise: the recipe's lens moves the frame");
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
/// the export resamples the recipe's lens geometry over it. A MARKED one —
/// lens geometry already in its pixels, which no develop bakes (v1.17.0, the
/// Camera Raw law family removed) — is refused at every door, the export, the
/// master read of a heal or fill and the frame check the previews make,
/// rather than corrected twice or left with a correction the recipe cannot
/// switch off.
///
/// MUTATION: let `SourceFrame::check` pass a marked master, or drop the
/// tail's lens over an unmarked one.
#[test]
fn an_unmarked_master_is_corrected_in_the_tail_and_a_marked_one_is_refused() {
    let dir = crate::test_dir("baked-geometry-apply");
    let r = marked_recipe();
    let g = BakedGeometry::frame_of(&r).unwrap();
    let img = ring_frame(160, 120);
    let (old, new) = (dir.join("old.png"), dir.join("new.png"));
    crate::pipeline::save_master(&old, img.clone(), None).unwrap();
    crate::pipeline::save_master(&new, img.clone(), Some(&g)).unwrap();
    let none = crate::diag::pixels();
    let no_lens = EditRecipe { lens_profile: Default::default(), ..r.clone() };
    let as_is = render_baked_to_image(&img, &no_lens, None, None, None, &none).unwrap().to_rgb16();
    let corrected = render_baked_to_image(&img, &r, None, None, None, &none).unwrap().to_rgb16();
    assert!(as_is.as_raw() != corrected.as_raw(), "premise: the profile moves the rings");
    let out = dir.join("old.tif");
    render_to_file(&old, &r, &out, None, None, crate::diag::stderr()).unwrap();
    assert!(read_back(&out).to_rgb16().as_raw() == corrected.as_raw(), "an unmarked master is corrected in the tail");
    let frame = SourceFrame::of(&new).unwrap();
    for (case, recipe) in [("its own lens geometry", &r), ("no lens geometry", &no_lens)] {
        let e = frame.check(recipe).expect_err(case).to_string();
        assert!(e.contains("bake the master again"), "{case}: {e}");
        let out = dir.join("new.tif");
        assert!(render_to_file(&new, recipe, &out, None, None, crate::diag::stderr()).is_err(), "{case}: the export refuses");
        assert!(master_pixels_for(&new, &BasePart::of(recipe), None).is_err(), "{case}: a heal over it refuses");
    }
}
