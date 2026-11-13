// One part of the engine's tests (src/render/tests.rs includes it): a raster mask's band (J149) through the frame it is read on — a mixed pixel blended in light, the colour field's partition read by content, and the window a noisy frame is read through.

/// A band pixel that is a mixture is rendered as each of its two contents is
/// and the two renderings are mixed IN LIGHT, in the share of its light that
/// is each one's. Here a pixel nine tenths land and one tenth sky beside a
/// land lifted 2.5 EV and a sky left alone: 83 % of its light is the sky's,
/// so it is the pixel as the sky's layer leaves it (untouched) for 83 % of
/// its light and as the land's leaves it (the same value deep in the land,
/// column 55) for 17 %. Blended at one weight in the ENCODED values it took
/// 17 % of an encoded step and stood six codes short.
///
/// MUTATION THIS KILLS: `PixelWeight::blend` blending at the luminance weight
/// alone (66 for 73).
#[test]
fn a_mixed_pixel_is_each_contents_light_through_its_own_treatment() {
    use crate::recipe::MaskGeometry;
    std::fs::create_dir_all("out").ok();
    let (w, h, edge) = (60u32, 24u32, 30u32);
    let (sky, land) = (150.0f32 / 255.0, 20.0f32 / 255.0);
    let mixed = linear_to_srgb(0.1 * srgb_to_linear(sky) + 0.9 * srgb_to_linear(land));
    let base = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
        let v = match x.cmp(&edge) {
            std::cmp::Ordering::Less => sky,
            std::cmp::Ordering::Equal => mixed,
            // The mixed pixel's own value where the land's layer covers it whole.
            std::cmp::Ordering::Greater if x == 55 => mixed,
            std::cmp::Ordering::Greater => land,
        };
        image::Rgb([(v * 255.0).round() as u8; 3])
    }));
    let path = "out/_band_light.png";
    let ramp = |x: u32| ((x as f32 - 27.5) / 5.0).clamp(0.0, 1.0);
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([(ramp(x) * 255.0).round() as u8])).save(path).unwrap();
    let recipe = EditRecipe {
        masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: path.into() }, exposure_ev: 2.5, ..Default::default() }],
        ..Default::default()
    };
    let out = develop_preview(&base, &recipe).to_rgb8();
    let light = |x: u32, y: u32| srgb_to_linear(out.get_pixel(x, y)[0] as f32 / 255.0);
    let own = srgb_to_linear((mixed * 255.0).round() / 255.0);
    let of_sky = 0.1 * srgb_to_linear(sky) / own;
    for y in 0..h {
        assert!(out.get_pixel(55, y)[0] as i32 > out.get_pixel(50, y)[0] as i32 + 20, "premise, row {y}: the land's layer lifts the value");
        let owed = 255.0 * linear_to_srgb(of_sky * own + (1.0 - of_sky) * light(55, y));
        let read = out.get_pixel(edge, y)[0] as f32;
        assert!((read - owed).abs() <= 1.0, "row {y}: the mixed pixel reads {read}, its two renderings' light {owed:.1}");
    }
}

/// The colour field's sky / land partition takes a pixel under its zone
/// raster's step at its CONTENT's coverage — pure sky all sky, pure land none,
/// the pixel that is half of each a half — as the fit solves the field
/// (`preview_field_sky_coverage`), read on the frame as the masks find it:
/// here the land zone lifts the land to the sky's own level, and on the frame
/// it leaves there is no edge to read.
///
/// MUTATIONS THIS KILLS: `preview_field_sky_coverage` handing
/// `field_sky_coverage` no band (the pure sky pixel reads the raster's 0.9);
/// `Owed::by_area` answering the share of the pixel's LIGHT (the half pixel
/// reads 0.85); the band read after the masks (no step in the levels, so the
/// raster's area again).
#[test]
fn the_fields_partition_takes_a_pixel_under_the_rasters_step_at_its_contents_coverage() {
    use crate::recipe::{MaskGeometry, MaskRole};
    std::fs::create_dir_all("out").ok();
    let (w, h, edge) = (60u32, 24u32, 30u32);
    let path = "out/_band_partition_sky.png";
    let ramp = |x: u32| ((x as f32 - 27.5) / 5.0).clamp(0.0, 1.0);
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([255 - (ramp(x) * 255.0).round() as u8])).save(path).unwrap();
    // 110 against 46: 2.5 EV apart in light, so the land zone's lift leaves the two at one level.
    let base = band_scene(w, h, edge, 110, 46);
    let zone = |role: MaskRole, inverted: bool, exposure_ev: f32| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: path.into() },
        role,
        inverted,
        exposure_ev,
        ..Default::default()
    };
    let recipe = EditRecipe { masks: vec![zone(MaskRole::ZoneSky, false, 0.0), zone(MaskRole::ZoneLand, true, 2.5)], ..Default::default() };
    let lifted = develop_preview(&base, &recipe).to_rgb8();
    let (sky_after, land_after) = (lifted.get_pixel(10, 5)[0] as i32, lifted.get_pixel(50, 5)[0] as i32);
    assert!((sky_after - land_after).abs() <= 3, "premise: the masks leave the two at one level: {sky_after} / {land_after}");
    let sky = crate::render::preview_field_sky_coverage(&base, &recipe).expect("the recipe has a sky zone");
    let at = |x: u32, y: u32| sky[(y * w + x) as usize];
    for y in 0..h {
        for x in [10, 28, 29] {
            assert!((at(x, y) - 1.0).abs() <= 1e-3, "row {y}: column {x} is pure sky: {}", at(x, y));
        }
        for x in [31, 32, 50] {
            assert!(at(x, y).abs() <= 1e-3, "row {y}: column {x} is pure land: {}", at(x, y));
        }
        assert!((at(edge, y) - 0.5).abs() <= 0.03, "row {y}: the pixel that is half of each: {}", at(edge, y));
    }
}

/// The ENGINE reads the field on that partition: a field whose sky-owned
/// vertices carry a lift and whose land-owned ones carry none lifts the pure
/// sky pixels under the zone raster's step as it lifts the sky beside them.
/// Read by the raster's area, the one under 0.7 of the step took 0.7 of it.
///
/// MUTATION THIS KILLS: the develop handing `field_sky_coverage` no band.
#[test]
fn the_engine_reads_a_zone_owned_field_at_each_pixels_content() {
    use crate::recipe::{ColourField, MaskGeometry, MaskRole};
    std::fs::create_dir_all("out").ok();
    let (w, h, edge) = (60u32, 24u32, 30u32);
    let path = "out/_band_field_sky.png";
    let ramp = |x: u32| ((x as f32 - 27.5) / 5.0).clamp(0.0, 1.0);
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([255 - (ramp(x) * 255.0).round() as u8])).save(path).unwrap();
    let base = band_scene(w, h, edge, 150, 20);
    // Vertex columns at pixel columns 0, 29.5 and 59: the first is the sky's, the other two the land's.
    let (fx, fy, fb) = (3usize, 2usize, 2usize);
    let lifted = |v: usize| (v / fb).is_multiple_of(fx);
    let grid: Vec<[f32; 5]> = (0..fx * fy * fb).map(|v| if lifted(v) { [0.5, 0.0, 0.0, 0.0, 0.0] } else { [0.0; 5] }).collect();
    let owners = (0..fx * fy * fb).map(|v| if lifted(v) { ColourField::OWNER_SKY } else { ColourField::OWNER_LAND }).collect();
    let field = ColourField { x: fx, y: fy, b: fb, grid, owners, amount: 1.0, enabled: true };
    let zone = LocalAdjustment { mask: MaskGeometry::Bitmap { path: path.into() }, role: MaskRole::ZoneSky, ..Default::default() };
    let before = develop_preview(&base, &EditRecipe { masks: vec![zone.clone()], ..Default::default() }).to_rgb8();
    let after = develop_preview(&base, &EditRecipe { masks: vec![zone], colour_field: Some(field), ..Default::default() }).to_rgb8();
    let at = |img: &RgbImage, x: u32, y: u32| img.get_pixel(x, y)[0] as i32;
    for y in 0..h {
        assert!(at(&after, 10, y) > at(&before, 10, y) + 10, "premise, row {y}: the sky is lifted");
        for x in [28, 29] {
            assert_eq!(at(&after, x, y), at(&after, 10, y), "row {y}: column {x} is pure sky and is lifted as the sky is");
        }
        for x in [31, 32] {
            assert_eq!(at(&after, x, y), at(&before, x, y), "row {y}: column {x} is pure land and reads no sky lift");
        }
    }
}

/// A frame of two levels with a sensor's noise on both: what stands off its
/// level by `spread` (three uniform draws, summed), a draw a pixel.
fn noisy_frame(w: usize, h: usize, edge: usize, levels: [f32; 2], spreads: [f32; 2]) -> Vec<[f32; 3]> {
    let draw = |i: usize, k: u64| {
        let mut s = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ k.wrapping_mul(0xD1B5_4A32_D192_ED03);
        s ^= s >> 29;
        s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        s ^= s >> 32;
        (s >> 40) as f32 / (1u64 << 24) as f32
    };
    (0..w * h)
        .map(|i| {
            let side = usize::from(i % w >= edge);
            let off = 2.0 * spreads[side] * (draw(i, 1) + draw(i, 2) + draw(i, 3) - 1.5);
            [linear_to_srgb((levels[side] + off).max(0.0)); 3]
        })
        .collect()
}

/// On a NOISY frame a band pixel is read by the median of its window. Sky at
/// 0.4 of linear light with a spread of 0.08 against land at 0.1 with 0.04 —
/// the photon law's two to one — under a sky raster stepping over five
/// columns: every pure land pixel under the step is owed none of the sky's
/// layer and every pure sky pixel all of it, to the mean of a column. Read by
/// its own light a land pixel as bright as a mixture was owed a mixture's
/// share, one in ten of them, and the land's columns stood at 0.05.
///
/// MUTATION THIS KILLS: `calm` fixed at 0 in `Blocks::sides`.
#[test]
fn a_noisy_frame_is_read_through_a_window() {
    let (w, h, edge) = (64usize, 48usize, 30usize);
    let data = noisy_frame(w, h, edge, [0.4, 0.1], [0.08, 0.04]);
    let coverage = |x: usize, _: usize| 1.0 - ((x as f32 - 27.5) / 5.0).clamp(0.0, 1.0);
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    let mean = |x: u32| {
        let owed: Vec<f32> = rows.iter().filter_map(|row| row.iter().find(|entry| entry.0 == x)).map(|entry| entry.1[0]).collect();
        assert!(owed.len() >= h - 2, "premise: column {x} is on the band on {} of {h} rows", owed.len());
        owed.iter().sum::<f32>() / owed.len() as f32
    };
    for x in 30..=32 {
        assert!(mean(x) <= 0.005, "column {x} is land: {}", mean(x));
    }
    for x in 28..=29 {
        assert!(mean(x) >= 0.99, "column {x} is sky: {}", mean(x));
    }
}

/// A side that is rough on a frame with no noise is not a noisy frame: a
/// sensor's noise stands on both sides of an edge, the dimmer no noisier than
/// the brighter. A chequered sky beside a flat pole two pixels wide is read
/// pixel by pixel, and the pole is owed none of the sky's layer. Taken for
/// noise, the sky's chequer had the pole read through a window five pixels
/// wide, where two columns in five are the pole and the median is sky.
///
/// MUTATION THIS KILLS: `frame_noise` answering each side's own grain.
#[test]
fn a_rough_side_on_a_clean_frame_is_read_pixel_by_pixel() {
    let (w, h) = (44usize, 16usize);
    let pole = 21..=22usize;
    let data = grey_frame(w, h, |x, y| if pole.contains(&x) { 0.1 } else if (x + y) % 2 == 0 { 0.58 } else { 0.42 });
    let coverage = |x: usize, _: usize| match x {
        x if pole.contains(&x) => 0.0,
        20 | 23 => 0.5,
        _ => 1.0,
    };
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    for (y, row) in rows.iter().enumerate() {
        for x in [20u32, 21, 22, 23] {
            let lanes = row.iter().find(|entry| entry.0 == x).unwrap_or_else(|| panic!("row {y}: column {x} is on the band")).1;
            let owed = if pole.contains(&(x as usize)) { 0.0 } else { 1.0 };
            assert!(lanes.iter().all(|v| (v - owed).abs() <= 1e-3), "row {y}, column {x}: {lanes:?} against {owed}");
        }
    }
}

/// A band is a few blocks wide however small the frame, so on a thumbnail it
/// is a large share of the pixels — here the raster's step lies over five
/// columns of thirty-two. The list holds it all the same
/// (`mask_owed::BAND_LIST_FLOOR`): the pure sky pixels under the step wear what
/// the sky's flat pixels wear, as they do at full size.
///
/// MUTATION THIS KILLS: the list's budget a share of the frame alone (the
/// thumbnail's layer keeps its area weight, and the sky's last two columns
/// take a tenth and three tenths of the land's lift).
#[test]
fn a_small_frames_band_is_read_whole() {
    use crate::recipe::MaskGeometry;
    std::fs::create_dir_all("out").ok();
    let (w, h, edge) = (32u32, 24u32, 16u32);
    let base = band_scene(w, h, edge, 150, 20);
    let path = "out/_band_thumbnail.png";
    let ramp = |x: u32| ((x as f32 - 13.5) / 5.0).clamp(0.0, 1.0);
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([(ramp(x) * 255.0).round() as u8])).save(path).unwrap();
    let recipe = EditRecipe {
        masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: path.into() }, exposure_ev: 2.5, ..Default::default() }],
        ..Default::default()
    };
    let out = develop_preview(&base, &recipe).to_rgb8();
    for y in 0..h {
        let at = |x: u32| out.get_pixel(x, y)[0] as i32;
        assert!(at(26) > 20 + 20, "premise, row {y}: the land's layer lifts the land");
        for x in [14, 15] {
            assert_eq!(at(x), at(4), "row {y}, column {x}: a pure sky pixel under the raster's step wears the sky's weight");
        }
    }
}

/// The develop's memory account (`decode::PIPELINE_BYTES_PER_PIXEL`) charges a
/// band entry 32 bytes.
#[test]
fn a_band_entry_is_the_bytes_the_memory_account_charges() {
    assert_eq!(std::mem::size_of::<(u32, mask_owed::Owed)>(), 32);
}
