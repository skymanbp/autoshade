// One part of the engine's tests (src/render/tests.rs includes it): the lens resample in the camera stage (v1.17.0) — the linear resample, the corrected mask frame, the positioned passes on it and the base part that carries the lens.

/// A barrel profile strong enough to move a 96×64 frame's corners by pixels.
fn camera_stage_profile() -> crate::recipe::LensProfile {
    crate::recipe::LensProfile {
        distortion: (0..16).map(|i| 1.0 + 0.004 * i as f32).collect(),
        distortion_on: true,
        ..Default::default()
    }
}

/// The camera-stage resample keeps a sample below the black: a signed pair
/// averages to a signed value, where the encoded resample after the clip could
/// only answer zero or more (the p42 zero-pixel loss, `lens_in_camera`).
#[test]
fn the_camera_stage_resample_keeps_the_signed_black() {
    let (w, h) = (96usize, 64usize);
    let data: Vec<[f32; 3]> = (0..w * h).map(|i| if (i % w + i / w) % 2 == 0 { [-0.004; 3] } else { [0.001; 3] }).collect();
    let out = apply_lens_geometry_linear(data, w, h, &camera_stage_profile());
    let min = out.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let max = out.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
    assert!(min < -0.003, "a below-black sample survives the resample: {min}");
    assert!(max <= 0.001 + 1e-7, "no sample exceeds its sources: {max}");
    assert!(out.iter().any(|p| p[1] < 0.0 && p[1] > -0.004), "mixed windows average signed");
}

/// The linear resample moves every pixel where the encoded tail moved it: the
/// same map and windows, so on a frame inside [0, 1] the two agree to the
/// tail's own 16-bit rounding.
#[test]
fn the_camera_stage_resample_is_the_tails_map() {
    let (w, h) = (96u32, 64u32);
    let v = |x: u32, y: u32, c: u32| ((x * 7 + y * 13 + c * 29) % 97) as f32 / 96.0;
    let data: Vec<[f32; 3]> = (0..w * h).map(|i| [0, 1, 2].map(|c| v(i % w, i / w, c))).collect();
    let img = DynamicImage::ImageRgb16(image::ImageBuffer::from_fn(w, h, |x, y| image::Rgb([0, 1, 2].map(|c| to_u16(v(x, y, c))))));
    let p = crate::recipe::LensProfile {
        ca_r: vec![1.0, 1.002, 1.004],
        ca_b: vec![1.0, 0.998, 0.996],
        ca_on: true,
        ..camera_stage_profile()
    };
    let tail = apply_lens_geometry(&img, &p, 0.0).to_rgb16();
    let lin = apply_lens_geometry_linear(data, w as usize, h as usize, &p);
    let worst = tail
        .pixels()
        .zip(&lin)
        .flat_map(|(t, l)| (0..3).map(move |c| (t.0[c] as f32 / 65535.0 - l[c]).abs()))
        .fold(0.0f32, f32::max);
    assert!(worst < 2.0 / 65535.0, "the two resamples part by {worst}");
}

/// A mask sampled in the corrected frame (`MaskFrame::Corrected`) covers what
/// the same mask covered when the tail resampled the masked frame: the
/// positions the C2 contract stores stay where they were.
#[test]
fn a_mask_in_the_corrected_frame_lands_where_the_tail_carried_it() {
    let p = camera_stage_profile();
    let reference = DynamicImage::new_rgb8(96, 64);
    for geometry in [
        MaskGeometry::Radial {
            top: 0.05,
            left: 0.62,
            bottom: 0.45,
            right: 0.97,
            feather: 0.3,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 2,
        },
        MaskGeometry::Linear { zero_x: 0.1, zero_y: 0.9, full_x: 0.3, full_y: 0.6 },
    ] {
        let m = crate::recipe::LocalAdjustment { mask: geometry, amount: 1.0, exposure_ev: 1.0, ..Default::default() };
        let corrected = mask_coverage(&m, &reference, MaskFrame::Corrected { profile: &p });
        let carried = DynamicImage::ImageLuma8(mask_coverage(&m, &reference, MaskFrame::downstream(&p, 0.0)));
        let carried = apply_lens_geometry(&carried, &p, 0.0).to_luma8();
        let diffs: Vec<f32> =
            corrected.pixels().zip(carried.pixels()).map(|(a, b)| (a.0[0] as f32 - b.0[0] as f32).abs() / 255.0).collect();
        let mean = diffs.iter().sum::<f32>() / diffs.len() as f32;
        let worst = diffs.iter().copied().fold(0.0f32, f32::max);
        assert!(mean < 0.004 && worst < 0.06, "corrected frame vs the carried mask: mean {mean}, worst {worst}");
    }
}

/// A retouch ellipse stored in the original frame heals, on a corrected frame,
/// where the correction put its centre and donor, at the local scale there.
#[test]
fn a_retouch_area_on_a_corrected_frame_moves_with_the_lens() {
    let p = camera_stage_profile();
    let (w, h) = (96usize, 64usize);
    let r = EditRecipe {
        retouch: vec![crate::retouch::RetouchArea {
            donor: Some([0.7, 0.75]),
            shape: crate::retouch::RetouchShape::Ellipse { cx: 0.9, cy: 0.85, size_x: 0.03, size_y: 0.03 },
            ..Default::default()
        }],
        ..Default::default()
    };
    let stored = retouch_spots(&r, w, h);
    let u = MaskUnwarp::corrected(&p, (w as f32, h as f32)).expect("the profile moves the frame");
    let moved = retouch_spots_in(&r, w, h, Some(&u));
    let (cx, cy) = u.engine_at(0.9, 0.85);
    let (dx, dy) = u.engine_at(0.7, 0.75);
    assert_eq!((moved[0].cx, moved[0].cy), (cx, cy));
    assert_eq!(moved[0].source, Some([dx - cx, dy - cy]));
    assert!((cx - 0.9).abs() > 0.002, "premise: the lens moves the corner centre");
    // Barrel knots above 1 sample further out, so content shrinks towards the
    // centre: the radius shrinks with it.
    assert!(moved[0].radius < stored[0].radius && moved[0].radius > 0.9 * stored[0].radius);
}

/// The base part carries the lens geometry its camera stage resamples, and
/// its recipe reads it back unchanged, and so does a master's (v1.17.0: a
/// master is baked in the corrected frame and says so in its file);
/// `without_lens` does not.
#[test]
fn a_base_part_carries_the_lens_its_camera_stage_resamples() {
    let r = EditRecipe {
        lens_profile: crate::recipe::LensProfile { vignette: vec![1.0, 1.2], vignette_on: true, ..camera_stage_profile() },
        ca_r: 10.0,
        ..canvas_recipe()
    };
    assert!(lens_in_camera(&r));
    let part = BasePart::of(&r);
    assert!(part.corrects_lens());
    let base = part.recipe();
    assert!(lens_in_camera(&base));
    let want = crate::recipe::LensProfile { vignette: Vec::new(), vignette_on: false, ..geometry_profile(&r).into_owned() };
    let got = crate::recipe::LensProfile { vignette: Vec::new(), vignette_on: false, ..geometry_profile(&base).into_owned() };
    assert_eq!(got, want, "the base develops the recipe's composed geometry");
    assert!(base.lens_profile.vignette_on, "the vignetting rides along as before");
    let master = part.master_recipe();
    assert!(lens_in_camera(&master), "a master is baked in the corrected frame");
    assert_eq!(geometry_profile(&master).into_owned(), geometry_profile(&base).into_owned(), "…through the base's geometry");
    assert!(part.baked_geometry().is_some_and(|g| g.distortion && g.ca && g.vignette), "and its marker names all three");
    assert!(!part.without_lens().corrects_lens());
    // The Distortion slider keeps the whole geometry in the tail.
    let slider = EditRecipe { lens_distortion: 10.0, ..r.clone() };
    assert!(!lens_in_camera(&slider) && !BasePart::of(&slider).corrects_lens());
    // No profile named: the engine's own law, no camera stage.
    let plain = EditRecipe { lens_profile: camera_stage_profile(), ..Default::default() };
    assert!(!lens_in_camera(&plain));
}
