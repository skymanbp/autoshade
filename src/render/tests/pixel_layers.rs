// One part of render's tests (src/render/tests.rs includes it): the
// generative-fill layers composited over the finished develop (2026-10-01).

/// A frame-sized layer lands texel for texel: opaque texels replace the
/// develop, transparent ones leave it alone, and the opacity scales the
/// file's own alpha. A layer of another size covers the same fraction of the
/// frame (it is sampled at texel centres, the mask rasters' convention).
#[test]
fn a_layer_replaces_where_it_is_opaque_and_leaves_the_rest() {
    let grey = [0.2f32; 3];
    let layer = image::RgbaImage::from_fn(4, 2, |x, _| {
        if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 0]) }
    });
    let mut data = vec![grey; 8];
    layers::composite(&mut data, 4, 2, &layer, 1.0);
    for (i, px) in data.iter().enumerate() {
        let want = if i % 4 < 2 { [1.0, 0.0, 0.0] } else { grey };
        assert_eq!(*px, want, "pixel {i}");
    }
    let mut half = vec![grey; 8];
    layers::composite(&mut half, 4, 2, &layer, 0.5);
    assert!((half[0][0] - 0.6).abs() < 1e-6 && (half[0][1] - 0.1).abs() < 1e-6, "{:?}", half[0]);
    assert_eq!(half[3], grey, "a transparent texel stays transparent at any opacity");
    // Twice the size: the left half is still the opaque half.
    let mut big = vec![grey; 32];
    layers::composite(&mut big, 8, 4, &layer, 1.0);
    for y in 0..4 {
        assert_eq!(big[y * 8], [1.0, 0.0, 0.0], "row {y}: the left edge is covered");
        assert_eq!(big[y * 8 + 7], grey, "row {y}: the right edge is not");
    }
}

/// Through the real develop: a layer named by the recipe draws over the
/// finished picture; switched off it draws nothing, so the frame is the one
/// with no layer at all, byte for byte. An export (the strict loader)
/// refuses a layer it cannot read instead of leaving the patch out; the
/// preview skips it.
#[test]
fn the_develop_draws_its_layers_and_an_export_refuses_a_missing_one() {
    let dir = crate::test_dir("pixel-layers");
    let file = dir.join("fill.png");
    let patch = |x: u32, y: u32| (x, y) == (1, 1);
    image::RgbaImage::from_fn(6, 4, |x, y| image::Rgba(if patch(x, y) { [10, 200, 30, 255] } else { [0; 4] }))
        .save(&file)
        .expect("write the layer fixture");
    let frame = DynamicImage::ImageRgb8(RgbImage::from_pixel(6, 4, Rgb([90, 90, 90])));
    let layer = PixelLayer { path: file.display().to_string(), ..Default::default() };
    let with = EditRecipe { exposure_ev: 0.3, pixel_layers: vec![layer.clone()], ..Default::default() };
    let without = EditRecipe { pixel_layers: Vec::new(), ..with.clone() };
    let off = EditRecipe { pixel_layers: vec![PixelLayer { enabled: false, ..layer.clone() }], ..with.clone() };
    let drawn = develop_preview(&frame, &with).to_rgb8();
    let plain = develop_preview(&frame, &without).to_rgb8();
    assert_eq!(drawn.get_pixel(1, 1).0, [10, 200, 30], "the layer's texel is what shows");
    assert_eq!(drawn.get_pixel(4, 2), plain.get_pixel(4, 2), "outside the layer the develop shows");
    assert_eq!(develop_preview(&frame, &off).as_bytes(), plain.as_raw().as_slice(), "an off layer draws nothing");

    let missing = PixelLayer { path: dir.join("gone.png").display().to_string(), ..Default::default() };
    let gone = EditRecipe { pixel_layers: vec![missing], ..Default::default() };
    match load_mask_raster_snapshot(&gone, &crate::diag::pixels()) {
        Err(why) => assert!(why.to_string().contains("pixel layer"), "the refusal names the layer: {why}"),
        Ok(_) => panic!("an export must refuse a layer it cannot read"),
    }
    let neutral = develop_preview(&frame, &EditRecipe::default());
    assert_eq!(develop_preview(&frame, &gone).as_bytes(), neutral.as_bytes(), "the preview skips an unreadable layer");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The layer list survives a recipe.json round trip, and a layer from a
/// future build with a key this one does not know is refused rather than
/// silently half-read (the recipe's own `deny_unknown_fields` rule). The
/// clamp keeps the one number sane.
#[test]
fn layers_round_trip_through_the_recipe_and_refuse_unknown_keys() {
    let r = EditRecipe {
        pixel_layers: vec![PixelLayer { path: "a.png".into(), enabled: false, opacity: 0.4 }],
        ..Default::default()
    };
    let text = serde_json::to_string(&r).expect("a recipe serialises");
    assert_eq!(serde_json::from_str::<EditRecipe>(&text).expect("and reads back"), r);
    let future = r#"{"pixel_layers":[{"path":"a.png","blend":"multiply"}]}"#;
    assert!(serde_json::from_str::<EditRecipe>(future).is_err());
    let odd = |opacity| PixelLayer { opacity, ..Default::default() };
    let mut wild = EditRecipe { pixel_layers: vec![odd(f32::NAN), odd(3.0), odd(-1.0)], ..Default::default() };
    wild.clamp();
    let opacities: Vec<f32> = wild.pixel_layers.iter().map(|l| l.opacity).collect();
    assert_eq!(opacities, [1.0, 1.0, 0.0]);
}
