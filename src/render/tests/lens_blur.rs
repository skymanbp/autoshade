// One part of render's tests (src/render/tests.rs includes it): the AI lens
// blur driven by a depth map (2026-10-13).

/// A 16-bit nearness map that is nearest left of column `edge` and 0 (far)
/// from it on.
fn split_depth(w: u32, h: u32, edge: u32) -> lens_blur::DepthImage {
    image::ImageBuffer::from_fn(w, h, |x, _| image::Luma([if x < edge { u16::MAX } else { 0 }]))
}

/// A grey `w`×`h` frame whose level at (x, y) is `level(x, y)`.
fn frame_of(w: usize, h: usize, level: impl Fn(usize, usize) -> f32) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push([level(x, y); 3]);
        }
    }
    out
}

/// The local contrast left in a column band: mean |difference| between
/// horizontal neighbours.
fn texture(data: &[[f32; 3]], w: usize, cols: std::ops::Range<usize>) -> f32 {
    let rows = data.len() / w;
    let mut total = 0.0;
    for y in 0..rows {
        for x in cols.clone() {
            total += (data[y * w + x + 1][1] - data[y * w + x][1]).abs();
        }
    }
    total / (rows * cols.len()) as f32
}

fn blur_at(focus: [f32; 2]) -> LensBlur {
    LensBlur { depth_map: "depth.png".into(), amount: 100.0, range: 0.0, focus, ..Default::default() }
}

/// The blur follows distance from the focus, in both directions: focused on
/// the near half, the far half loses its texture and the near half keeps it;
/// focused on the far half, the reverse. A blur with no depth map, switched
/// off, or at zero amount changes nothing.
#[test]
fn the_blur_follows_distance_from_the_focus() {
    let (w, h) = (128usize, 32usize);
    let checker = frame_of(w, h, |x, y| if (x + y) % 2 == 0 { 0.8 } else { 0.2 });
    let depth = split_depth(w as u32, h as u32, w as u32 / 2);
    let (near, far) = (2..w / 2 - 8, w / 2 + 8..w - 3);
    let sharp = texture(&checker, w, near.clone());

    let mut on_near = checker.clone();
    lens_blur::apply(&mut on_near, w, h, &blur_at([0.25, 0.5]), &depth, None);
    assert!((texture(&on_near, w, near.clone()) - sharp).abs() < 1e-4, "the focused half keeps its texture");
    assert!(texture(&on_near, w, far.clone()) < sharp * 0.2, "the far half blurs");

    let mut on_far = checker.clone();
    lens_blur::apply(&mut on_far, w, h, &blur_at([0.75, 0.5]), &depth, None);
    assert!(texture(&on_far, w, near) < sharp * 0.2, "the foreground blurs too, by the same law");
    assert!((texture(&on_far, w, far) - sharp).abs() < 1e-4);

    let lb = blur_at([0.25, 0.5]);
    for idle in [
        LensBlur { enabled: false, ..lb.clone() },
        LensBlur { amount: 0.0, ..lb.clone() },
        LensBlur { depth_map: String::new(), ..lb.clone() },
    ] {
        let mut same = checker.clone();
        lens_blur::apply(&mut same, w, h, &idle, &depth, None);
        assert_eq!(same, checker, "{idle:?} must not change a pixel");
    }
}

/// The halo guard: a bright in-focus subject does not bleed into the blurred
/// background beside it, because a blur level averages only pixels that are
/// themselves that blurred.
///
/// MUTATION: give every pixel full weight in the level blur and the dark
/// background beside the subject lights up past the bound.
#[test]
fn an_in_focus_subject_leaves_no_halo_in_the_blurred_background() {
    let (w, h) = (128usize, 16usize);
    let mut out = frame_of(w, h, |x, _| if x < w / 2 { 1.0 } else { 0.0 });
    lens_blur::apply(&mut out, w, h, &blur_at([0.25, 0.5]), &split_depth(w as u32, h as u32, w as u32 / 2), None);
    let row = (h / 2) * w;
    let beside = out[row + w / 2 + 1][0];
    assert!(beside < 0.05, "the background beside the subject took {beside} of its light");
    // sRGB -> linear -> sRGB round trip, so equal to a few ulps.
    let edge = out[row + w / 2 - 1][0];
    assert!((edge - 1.0).abs() < 1e-5, "the subject's own edge stays sharp: {edge}");
}

/// The radius is a share of the long edge, so a larger render of the same
/// picture blurs the same picture: averaged back down, a 2× render matches
/// the 1× one closely.
#[test]
fn a_larger_render_blurs_the_same_picture() {
    let render = |w: usize, h: usize| {
        let mut out = frame_of(w, h, |x, _| if (x * 16 / w).is_multiple_of(2) { 0.7 } else { 0.3 });
        lens_blur::apply(&mut out, w, h, &blur_at([0.1, 0.5]), &split_depth(64, 32, 24), None);
        out
    };
    let (w, h) = (512usize, 256usize);
    let small = render(w, h);
    let big = render(w * 2, h * 2);
    let mut diff = 0.0;
    for y in 0..h {
        for x in 0..w {
            let at = |dx: usize, dy: usize| big[(2 * y + dy) * 2 * w + 2 * x + dx][0];
            diff += ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4.0 - small[y * w + x][0]).abs();
        }
    }
    let mean = diff / (w * h) as f32;
    assert!(mean < 0.02, "a 2x render differs from the 1x one by {mean} on average");
}

/// Through the real develop: the recipe's depth map is loaded and blurs the
/// frame. An export (the strict loader) refuses a map it cannot read rather
/// than leaving the blur out; the preview skips it.
#[test]
fn the_develop_blurs_by_its_depth_map_and_an_export_refuses_a_missing_one() {
    let dir = crate::test_dir("lens-blur");
    let file = dir.join("depth.png");
    split_depth(64, 32, 32).save(&file).expect("write the depth fixture");
    let frame = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 32, |x, y| Rgb([if (x + y) % 2 == 0 { 200 } else { 50 }; 3])));
    let lb = LensBlur { depth_map: file.display().to_string(), ..blur_at([0.25, 0.5]) };
    let blurred = develop_preview(&frame, &EditRecipe { lens_blur: Some(lb.clone()), ..Default::default() }).to_rgb8();
    let plain = develop_preview(&frame, &EditRecipe::default()).to_rgb8();
    assert_eq!(blurred.get_pixel(8, 8), plain.get_pixel(8, 8), "the focused side is untouched");
    assert_ne!(blurred.get_pixel(56, 8), plain.get_pixel(56, 8), "the far side is blurred");

    let gone = EditRecipe {
        lens_blur: Some(LensBlur { depth_map: dir.join("gone.png").display().to_string(), ..lb }),
        ..Default::default()
    };
    let why = load_mask_raster_snapshot(&gone, &crate::diag::pixels())
        .expect_err("an export must refuse a depth map it cannot read");
    assert!(format!("{why:#}").contains("depth map"), "the refusal names the depth map: {why:#}");
    assert_eq!(develop_preview(&frame, &gone).as_bytes(), plain.as_raw().as_slice(), "the preview skips it");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Absent, the blur costs a recipe no bytes (old recipes serialise as they
/// did); present, it round-trips; a key from a future build is refused; the
/// clamp keeps its numbers in range and drops a blur with no depth map.
#[test]
fn the_lens_blur_round_trips_and_its_absence_costs_no_bytes() {
    assert!(!serde_json::to_string(&EditRecipe::default()).unwrap().contains("lens_blur"));
    let saved = LensBlur { depth_map: "d.png".into(), focus: [0.2, 0.7], amount: 35.0, range: 60.0, enabled: false, protect: None };
    let r = EditRecipe { lens_blur: Some(saved), ..Default::default() };
    assert_eq!(serde_json::from_str::<EditRecipe>(&serde_json::to_string(&r).unwrap()).unwrap(), r);
    assert!(serde_json::from_str::<EditRecipe>(r#"{"lens_blur":{"depth_map":"d.png","bokeh":"cats-eye"}}"#).is_err());

    let wild = LensBlur { depth_map: "d.png".into(), focus: [f32::NAN, 2.0], amount: 400.0, range: -5.0, enabled: true, protect: None };
    let mut clamped = EditRecipe { lens_blur: Some(wild), ..Default::default() };
    clamped.clamp();
    let lb = clamped.lens_blur.expect("a blur with a depth map survives the clamp");
    assert_eq!((lb.focus, lb.amount, lb.range), ([0.5, 1.0], 100.0, 0.0));
    let mut empty = EditRecipe { lens_blur: Some(LensBlur::default()), ..Default::default() };
    empty.clamp();
    assert!(empty.lens_blur.is_none(), "a blur with no depth map has nothing to draw from");
}

/// A small synthetic render through the blur, hashed bit for bit: a ramp of
/// depths (so every level of the ladder is used) under a checker. Pinned on
/// the renderer BEFORE `protect` existed (2026-10-25), so a blur without a
/// protect mask is held to the exact pixels it drew then.
fn pinned_render_hash() -> u64 {
    let (w, h) = (48usize, 24usize);
    let mut out = frame_of(w, h, |x, y| if (x / 2 + y / 3) % 2 == 0 { 0.85 } else { 0.1 + 0.01 * x as f32 });
    let depth: lens_blur::DepthImage =
        image::ImageBuffer::from_fn(w as u32, h as u32, |x, _| image::Luma([(x * 65535 / (w as u32 - 1)) as u16]));
    lens_blur::apply(&mut out, w, h, &blur_at([0.1, 0.5]), &depth, None);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in out.iter().flatten().flat_map(|v| v.to_bits().to_le_bytes()) {
        hash = (hash ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[test]
fn a_blur_without_protection_draws_the_pixels_it_drew_before_protection_existed() {
    assert_eq!(pinned_render_hash(), 6_719_350_490_590_453_548, "the unprotected blur moved a pixel");
}

/// A protect mask over `cols`, at `level` (255 = fully protected).
fn protect_cols(w: u32, h: u32, cols: std::ops::Range<u32>, level: u8) -> image::GrayImage {
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([if cols.contains(&x) { level } else { 0 }]))
}

/// Protection (v1.14.0): inside a fully white protect mask the far half keeps
/// the sharp source although the depth map says blur it; outside the mask the
/// blur is the unprotected one; a half-grey mask halves the radius, so the
/// mask's own soft edge is the transition.
///
/// MUTATION: drop the `1 − alpha` scaling in `radii` and the protected band
/// blurs like the rest of the far half.
#[test]
fn a_protect_mask_keeps_its_white_sharp_and_leaves_the_rest_as_it_was() {
    let (w, h) = (128usize, 32usize);
    let checker = frame_of(w, h, |x, y| if (x + y) % 2 == 0 { 0.8 } else { 0.2 });
    let depth = split_depth(w as u32, h as u32, w as u32 / 2);
    let lb = blur_at([0.25, 0.5]);
    let sharp = texture(&checker, w, 84..108);

    let mut plain = checker.clone();
    lens_blur::apply(&mut plain, w, h, &lb, &depth, None);
    let mut kept = checker.clone();
    let mask = protect_cols(w as u32, h as u32, 80..112, 255);
    lens_blur::apply(&mut kept, w, h, &lb, &depth, Some(&mask));

    assert!(texture(&plain, w, 84..108) < sharp * 0.2, "unprotected, that band blurs");
    assert!((texture(&kept, w, 84..108) - sharp).abs() < 1e-4, "protected, it keeps the source's texture");
    for y in 0..h {
        for x in 86..106 {
            let (k, c) = (kept[y * w + x][0], checker[y * w + x][0]);
            assert!((k - c).abs() < 1e-5, "({x}, {y}) is the sharp source: {k} vs {c}");
        }
    }
    // Away from the mask (beyond the blur's reach) nothing moved.
    for y in 0..h {
        for x in (2..70).chain(118..126) {
            let (k, p) = (kept[y * w + x][0], plain[y * w + x][0]);
            assert!((k - p).abs() < 1e-6, "({x}, {y}) outside the mask changed: {k} vs {p}");
        }
    }

    // Wide stripes on a wider frame, so the two radii are different kernels
    // (on 128 px both round to the same box) and the smaller one visibly
    // keeps more of the stripes.
    let (w, h) = (512usize, 16usize);
    let depth = split_depth(w as u32, h as u32, w as u32 / 2);
    let stripes = frame_of(w, h, |x, _| if (x / 8) % 2 == 0 { 0.8 } else { 0.2 });
    let (mut full, mut half) = (stripes.clone(), stripes.clone());
    lens_blur::apply(&mut full, w, h, &lb, &depth, None);
    lens_blur::apply(&mut half, w, h, &lb, &depth, Some(&protect_cols(w as u32, h as u32, 256..512, 128)));
    let (t_sharp, t_half, t_full) =
        (texture(&stripes, w, 330..440), texture(&half, w, 330..440), texture(&full, w, 330..440));
    assert!(
        t_full + 1e-3 < t_half && t_half + 1e-3 < t_sharp,
        "a half-grey mask blurs part-way: {t_full} < {t_half} < {t_sharp}"
    );
}

/// Through the real develop: the recipe's protect mask is loaded beside its
/// depth map and keeps the far side sharp. A protect mask that cannot be read
/// is not silently dropped: the preview draws no blur at all rather than blur
/// the protected subject, and an export refuses by name.
#[test]
fn the_develop_reads_the_protect_mask_and_never_blurs_past_a_missing_one() {
    let dir = crate::test_dir("lens-blur-protect");
    let depth = dir.join("depth.png");
    split_depth(64, 32, 32).save(&depth).expect("write the depth fixture");
    let mask = dir.join("protect.png");
    protect_cols(64, 32, 40..64, 255).save(&mask).expect("write the protect fixture");
    let frame = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 32, |x, y| Rgb([if (x + y) % 2 == 0 { 200 } else { 50 }; 3])));
    let plain = develop_preview(&frame, &EditRecipe::default()).to_rgb8();
    let lb = LensBlur { depth_map: depth.display().to_string(), ..blur_at([0.25, 0.5]) };
    let blurred = develop_preview(&frame, &EditRecipe { lens_blur: Some(lb.clone()), ..Default::default() }).to_rgb8();
    let guarded = LensBlur { protect: Some(mask.display().to_string()), ..lb.clone() };
    let kept = develop_preview(&frame, &EditRecipe { lens_blur: Some(guarded), ..Default::default() }).to_rgb8();
    assert_ne!(blurred.get_pixel(52, 8), plain.get_pixel(52, 8), "unprotected, the far side blurs");
    assert_eq!(kept.get_pixel(52, 8), plain.get_pixel(52, 8), "protected, it does not");
    assert_eq!(kept.get_pixel(34, 8), blurred.get_pixel(34, 8), "the far side outside the mask still blurs");

    let gone = EditRecipe {
        lens_blur: Some(LensBlur { protect: Some(dir.join("gone.png").display().to_string()), ..lb }),
        ..Default::default()
    };
    assert_eq!(develop_preview(&frame, &gone).as_bytes(), plain.as_raw().as_slice(), "no blur past a missing protect mask");
    let why = load_mask_raster_snapshot(&gone, &crate::diag::pixels())
        .expect_err("an export must refuse a protect mask it cannot read");
    assert!(format!("{why:#}").contains("protect mask"), "the refusal names the protect mask: {why:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `protect` absent costs a recipe no bytes and an older recipe reads as
/// unprotected; present, it round-trips; an empty path is dropped by the clamp.
#[test]
fn the_protect_field_is_absent_unless_used_and_round_trips() {
    let lb = LensBlur { depth_map: "d.png".into(), ..Default::default() };
    let json = serde_json::to_string(&EditRecipe { lens_blur: Some(lb.clone()), ..Default::default() }).unwrap();
    assert!(!json.contains("protect"), "{json}");
    let old = r#"{"lens_blur":{"depth_map":"d.png","focus":[0.5,0.5],"amount":50.0,"range":10.0,"enabled":true}}"#;
    assert_eq!(serde_json::from_str::<EditRecipe>(old).unwrap().lens_blur.unwrap().protect, None);
    let r = EditRecipe { lens_blur: Some(LensBlur { protect: Some("mask-subject.png".into()), ..lb.clone() }), ..Default::default() };
    assert_eq!(serde_json::from_str::<EditRecipe>(&serde_json::to_string(&r).unwrap()).unwrap(), r);
    let mut empty = EditRecipe { lens_blur: Some(LensBlur { protect: Some(String::new()), ..lb }), ..Default::default() };
    empty.clamp();
    assert_eq!(empty.lens_blur.expect("the blur itself survives").protect, None);
}
