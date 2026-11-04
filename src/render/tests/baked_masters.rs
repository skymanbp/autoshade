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
        assert_eq!(SourceFrame::of(&out).unwrap(), SourceFrame::Baked(Some(g.clone()), None), "{name}");
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
    let as_is = render_baked_to_image(&img, &no_lens, None, None, None, false, &none).unwrap().to_rgb16();
    let corrected = render_baked_to_image(&img, &r, None, None, None, false, &none).unwrap().to_rgb16();
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

/// A noisy grey-blue frame: colour noise the display-frame pass visibly takes.
fn chroma_noise_frame(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb16(ImageBuffer::from_fn(w, h, |x, y| {
        let n = |k: u32| ((x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ k.wrapping_mul(1_299_709)) % 4001) as i32 - 2000;
        Rgb([(26000 + 3 * n(1)) as u16, (30000 + 3 * n(2)) as u16, (36000 + 3 * n(3)) as u16])
    }))
}

/// One colour stage per develop (E3 of v1.18.0). Pixels that already carry
/// it — a RAW's base, a marked master — run no display-frame pass, so Colour
/// 25 renders them exactly as Colour 0 does; pixels that carry none — a JPEG,
/// a TIFF, an older master — run it, and Colour 25 changes them.
///
/// MUTATION: run the display pass whatever the pixels carry, or never.
#[test]
fn the_display_pass_runs_only_over_pixels_that_carry_no_colour_stage() {
    let img = chroma_noise_frame(96, 64);
    let at = |nr: f32, in_pixels: bool| {
        let r = EditRecipe { color_nr: nr, ..Default::default() };
        render_baked_to_image(&img, &r, None, None, None, in_pixels, &crate::diag::pixels()).unwrap().to_rgb16()
    };
    assert_eq!(at(25.0, true), at(0.0, true), "the stage is in the pixels: the display pass must be absent");
    assert_ne!(at(25.0, false), at(0.0, false), "no stage in the pixels: the display pass must run");
    // The preview arm reads the same fact off the source.
    let preview = |nr: f32, frame: &SourceFrame| {
        let r = EditRecipe { color_nr: nr, ..Default::default() };
        develop_preview_film(&img, &r, &crate::diag::pixels(), None, frame).to_rgb8()
    };
    let marked = SourceFrame::Baked(None, BakedColourNr::of(25.0));
    let unmarked = SourceFrame::Baked(None, None);
    let (bayer, other) = (SourceFrame::Raw { linear_stage: true }, SourceFrame::Raw { linear_stage: false });
    assert!(bayer.colour_nr_in_pixels() && marked.colour_nr_in_pixels() && !unmarked.colour_nr_in_pixels());
    assert!(!other.colour_nr_in_pixels(), "a mosaic the linear stage cannot read carries no colour stage");
    assert_eq!(preview(25.0, &bayer), preview(0.0, &bayer), "a RAW base: no display pass");
    assert_ne!(preview(25.0, &other), preview(0.0, &other), "a non-Bayer RAW base: the display pass runs");
    assert_eq!(preview(25.0, &marked), preview(0.0, &marked), "a marked master: no display pass");
    assert_ne!(preview(25.0, &unmarked), preview(0.0, &unmarked), "an unmarked baked file: the display pass runs");
}

/// The colour-NR marker rides PNG, TIFF and JPEG beside the lens-geometry
/// marker, each read back as written and neither disturbing the other; a
/// file with none reads `None`, and `SourceFrame::of` carries it.
///
/// MUTATION: stamp both markers under one key, or drop the marker from
/// `SourceFrame::of`.
#[test]
fn a_master_says_its_colour_noise_reduction_is_in_its_pixels() {
    let dir = crate::test_dir("baked-colour-nr");
    let g = BakedGeometry::frame_of(&marked_recipe()).expect("premise: the recipe's lens moves the frame");
    let m = BakedColourNr::of(30.0).expect("on");
    assert_eq!(BakedColourNr::of(0.0), None, "colour NR off bakes no marker");
    for name in ["c.png", "c.tif", "c.jpg"] {
        let out = dir.join(name);
        crate::pipeline::save_master(&out, ring_frame(64, 48), None).unwrap();
        assert_eq!(read_colour_nr(&out).unwrap(), None, "{name}: unmarked");
        stamp_colour_nr(&out, &m).unwrap();
        stamp_baked_geometry(&out, &g).unwrap();
        stamp_colour_nr(&out, &m).unwrap(); // replaced, not doubled
        assert_eq!(read_colour_nr(&out).unwrap().as_ref(), Some(&m), "{name}");
        assert_eq!(read_baked_geometry(&out).unwrap().as_ref(), Some(&g), "{name}: the geometry marker is untouched");
        let carried = dir.join(format!("carried-{name}"));
        crate::pipeline::save_master(&carried, ring_frame(64, 48), None).unwrap();
        carry_baked_geometry(&out, &carried).unwrap();
        assert_eq!(read_colour_nr(&carried).unwrap().as_ref(), Some(&m), "{name}: a denoise product carries it");
    }
    let plain = dir.join("plain.png");
    crate::pipeline::save_master(&plain, ring_frame(64, 48), None).unwrap();
    stamp_colour_nr(&plain, &m).unwrap();
    assert_eq!(SourceFrame::of(&plain).unwrap(), SourceFrame::Baked(None, Some(m)));
}

/// The bake law: the part carries the colour-NR switch by the control's own
/// rule (on exactly when `ChromaNrParams::global` is, at the slider), with
/// or without a profile; the base and master recipes run no colour NR of
/// their own; and a part with the stage on is not the neutral develop.
///
/// MUTATION: take the switch from the base recipe (always 0), or drop it
/// from a part with a profile.
#[test]
fn the_part_carries_the_colour_stage_by_the_controls_own_rule() {
    for nr in [0.0f32, 1.0, 25.0, 100.0] {
        for profile in ["", "Adobe Standard"] {
            let r = EditRecipe { color_nr: nr, camera_profile: profile.into(), ..Default::default() };
            let part = BasePart::of(&r);
            let want = if super::detail::ChromaNrParams::global(&r).is_some() { nr } else { 0.0 };
            assert_eq!(part.colour_nr(), want, "colour NR {nr}, profile {profile:?}");
            assert_eq!(part.recipe().color_nr, 0.0);
            assert_eq!(part.master_recipe().color_nr, 0.0);
            assert_eq!(part.is_neutral(), nr == 0.0 && !r.names_profile());
        }
    }
}
