use super::*;
use crate::render::{blur_plane, texture_radius};
use image::RgbImage;

/// A frame with structure at both of the engine's scales: a slow horizontal
/// gradient, a mid-scale bar pattern and a fine checker, grey so chroma plays
/// no part. `fine` and `mid` scale the two structures; `seed` draws the
/// fine component from an independent pseudo-random field instead and
/// shifts the bars by half a period, which is how a "re-synthesised" frame
/// is built — the same energy at both scales, none of it where the
/// original's is.
fn textured(w: u32, h: u32, fine: f32, mid: f32, seed: Option<u32>) -> DynamicImage {
    let img = RgbImage::from_fn(w, h, |x, y| {
        let base = 0.35 + 0.2 * x as f32 / w as f32;
        let phase = if seed.is_some() { 6 } else { 0 };
        let bars = if ((y + phase) / 12) % 2 == 0 { mid * 0.05 } else { -mid * 0.05 };
        let checker = match seed {
            None => if (x + y) % 2 == 0 { fine * 0.04 } else { -fine * 0.04 },
            Some(s) => {
                let mut v = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ s;
                v ^= v >> 13;
                v = v.wrapping_mul(0x5bd1_e995);
                v ^= v >> 15;
                // Uniform on [-1, 1] has a third of the checker's variance;
                // sqrt(3) puts the two at the same energy.
                fine * 0.04 * 1.732 * ((v % 2000) as f32 / 1000.0 - 1.0)
            }
        };
        let l = (base + bars + checker).clamp(0.0, 1.0);
        image::Rgb([(l * 255.0).round() as u8; 3])
    });
    DynamicImage::ImageRgb8(img)
}

/// The engine's own unsharp pass at `radius`, applied to a frame: the band
/// above that radius multiplied by `1 + amount` (no midtone mask), chroma-free
/// here. At the texture radius this is the positive texture pass; at the
/// clarity radius, clarity without its midtone weighting.
fn unsharpened(img: &DynamicImage, radius: usize, amount: f32) -> Vec<[f32; 3]> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = fit::pixels_of(img);
    let luma: Vec<f32> = px.iter().map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]).collect();
    let blurred = blur_plane(&luma, w, h, radius);
    luma.iter().zip(&blurred).map(|(l, b)| [(l + amount * (l - b)).clamp(0.0, 1.0); 3]).collect()
}

fn textured_by_engine(img: &DynamicImage, amount: f32) -> Vec<[f32; 3]> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    unsharpened(img, texture_radius(w, h), amount)
}

fn image_of(px: &[[f32; 3]], w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        image::Rgb(px[(y * w + x) as usize].map(|c| (c * 255.0).round() as u8))
    }))
}

#[test]
fn band_readings_pair_identical_structure_at_unit_gain() {
    let img = textured(384, 256, 1.0, 1.0, None);
    let px = fit::pixels_of(&img);
    let weights = vec![1.0f32; px.len()];
    for band in band_readings(&px, &px, &weights, 384, 256) {
        assert!(band.r > 0.999, "{band:?}");
        assert!((band.paired_gain() - 1.0).abs() < 1e-3, "{band:?}");
        assert!((band.render - band.target).abs() < 1e-6 && band.render > DETAIL_MIN_CONTRAST, "{band:?}");
    }
}

/// The reading is the engine's own operator inverted: a target made by the
/// texture pass at `amount` reads a gain of `1 + amount` on the fine band,
/// whichever way the amount points — to within a few percent (box^3 is not
/// a projection: the pass at +0.6 reads 1.577), which is why the solve is
/// closed loop and not this number. The mid band is left where it was.
#[test]
fn band_readings_read_the_engines_own_gain_back() {
    let img = textured(384, 256, 1.0, 1.0, None);
    let px = fit::pixels_of(&img);
    let weights = vec![1.0f32; px.len()];
    for amount in [0.6f32, -0.4] {
        let target = textured_by_engine(&img, amount);
        let [fine, mid] = band_readings(&px, &target, &weights, 384, 256);
        assert!(fine.r > 0.99, "{fine:?}");
        assert!((fine.paired_gain() - (1.0 + amount)).abs() < 0.05, "amount {amount}: {fine:?}");
        // The 2-px checker is not fully inside a 2-px box^3, so a little of
        // the fine move reaches the mid band; most of it does not.
        assert!((mid.paired_gain() - 1.0).abs() < 0.4 * amount.abs(), "the fine pass mostly leaves the mid band alone: {mid:?}");
    }
}

/// A target whose fine structure is drawn from an independent field carries
/// the same band energy and no pairing: the gate line separates it from a
/// paired zone by a wide margin on both sides.
#[test]
fn band_readings_refuse_a_re_synthesised_texture() {
    let img = textured(384, 256, 1.0, 1.0, None);
    let drawn = textured(384, 256, 1.0, 1.0, Some(7));
    let (px, tp) = (fit::pixels_of(&img), fit::pixels_of(&drawn));
    let weights = vec![1.0f32; px.len()];
    let [fine, mid] = band_readings(&px, &tp, &weights, 384, 256);
    assert!(fine.r < DETAIL_PAIRED_MIN - 0.2, "an independent texture must not pair: {fine:?}");
    assert!(mid.r < DETAIL_PAIRED_MIN - 0.2, "half-period bars do not pair either: {mid:?}");
    assert!((fine.target / fine.render - 1.0).abs() < 0.3, "the energies were meant to match: {fine:?}");
    // The pairing line is calibrated against both populations (module doc).
    assert!((0.5..=0.8).contains(&DETAIL_PAIRED_MIN));
}

/// Weights select the zone: structure outside it is not read.
#[test]
fn band_readings_read_only_the_weighted_zone() {
    let (w, h) = (384u32, 256u32);
    let img = textured(w, h, 1.0, 1.0, None);
    let px = fit::pixels_of(&img);
    // The target's lower half is the engine's pass at +0.5; the upper half
    // is untouched.
    let mut target = textured_by_engine(&img, 0.5);
    for (i, p) in target.iter_mut().enumerate() {
        if (i as u32) / w < h / 2 {
            *p = px[i];
        }
    }
    let lower: Vec<f32> = (0..px.len()).map(|i| if (i as u32) / w >= h / 2 { 1.0 } else { 0.0 }).collect();
    let upper: Vec<f32> = lower.iter().map(|v| 1.0 - v).collect();
    let [lo, _] = band_readings(&px, &target, &lower, w, h);
    let [up, _] = band_readings(&px, &target, &upper, w, h);
    assert!((lo.paired_gain() - 1.5).abs() < 0.05, "{lo:?}");
    assert!((up.paired_gain() - 1.0).abs() < 0.02, "{up:?}");
}

/// One zone over a whole frame, on a raster the test owns.
fn whole_frame_zone(name: &str, w: u32, h: u32) -> (DetailZone, crate::store::OwnedRaster) {
    let raster = super::super::tests::fixture_mask_path(name);
    let mask = GrayImage::from_pixel(w, h, image::Luma([255u8]));
    mask.save(raster.path()).unwrap();
    let zone = DetailZone {
        label: "sky".to_string(),
        name: String::new(),
        role: MaskRole::ZoneSky,
        mask: MaskGeometry::Bitmap { path: raster.path().to_string_lossy().into_owned() },
        components: Vec::new(),
        inverted: false,
        source: mask.clone(),
        target: mask,
    };
    (zone, raster)
}

fn detail_mask(report: &FitReport) -> Option<&LocalAdjustment> {
    report.recipe.masks.iter().find(|m| m.texture != 0.0 || m.clarity != 0.0)
}

fn has_note(report: &FitReport, key: &str) -> bool {
    report.notes.iter().any(|n| n.key == key)
}

/// The stage ships a POSITIVE texture when the target's fine structure is
/// stronger, a NEGATIVE clarity when the target is softer above the clarity
/// radius, and lands the asked band within the matched window of the
/// target's — on real renders, through the engine, with the sliders on
/// Lightroom's integer scale. The softer target is built with the clarity
/// radius because the texture slider has no negative half here (module doc).
#[test]
fn the_detail_stage_solves_both_directions_on_real_renders() {
    let (w, h) = (384u32, 256u32);
    let src = textured(w, h, 1.0, 1.0, None);
    let r_cla = crate::render::clarity_radius(w as usize, h as usize);
    for (band, amount, radius, name) in [
        (0usize, 0.5f32, texture_radius(w as usize, h as usize), "detail-up"),
        (1usize, -0.3f32, r_cla, "detail-down"),
    ] {
        let tgt = image_of(&unsharpened(&src, radius, amount), w, h);
        let (zone, raster) = whole_frame_zone(name, w, h);
        let mut report = super::super::tests::neutral_report(&src, &tgt);
        let masks_before = report.recipe.masks.len();
        attach_zone_detail(&src, &tgt, &mut report, &[zone]);
        let mask = detail_mask(&report).unwrap_or_else(|| panic!("no detail mask shipped for {amount}: {}", report.recipe.rationale));
        assert_eq!(report.recipe.masks.len(), masks_before + 1);
        assert!(has_note(&report, crate::rationale::keys::DETAIL_ATTACHED), "{}", report.recipe.rationale);
        assert_eq!(mask.texture, mask.texture.round(), "integer slider");
        assert_eq!(mask.clarity, mask.clarity.round(), "integer slider");
        assert!(mask.sharpness == 0.0 && mask.exposure_ev == 0.0, "only the two detail sliders move: {mask:?}");
        if amount > 0.0 {
            assert!(mask.texture > 0.0, "{mask:?}");
        } else {
            // The negative clarity takes the fine band down with the mid
            // band (its operand contains it), past the target's 0.7; the
            // loop then gives some of it back with a positive texture — the
            // coupled solve, not a stray slider.
            assert!(mask.clarity < 0.0 && mask.texture >= 0.0, "{mask:?}");
        }
        // Re-read what shipped: the asked band sits at the target's within
        // the matched window, and no band was pushed past the target.
        let big = src.thumbnail(DETAIL_EDGE, DETAIL_EDGE);
        let big_t = tgt.thumbnail_exact(big.width(), big.height());
        let px = fit::pixels_of(&render::develop_preview(&big, &report.recipe));
        let weights = vec![1.0f32; px.len()];
        let after = band_readings(&px, &fit::pixels_of(&big_t), &weights, big.width(), big.height());
        assert!((after[band].paired_gain() - 1.0).abs() <= 0.12, "amount {amount}: landed at {after:?}, mask {mask:?}");
        if amount < 0.0 {
            assert!((after[0].paired_gain() - 1.0).abs() <= 0.15, "the fine band rides along: {after:?}, mask {mask:?}");
        }
        if amount > 0.0 {
            assert!(after[0].render <= after[0].target * (1.0 + DETAIL_ENERGY_TOL), "{after:?}");
        }
        raster.remove();
    }
}

/// A target whose texture was drawn, not developed, gets no slider and the
/// sentence that says why.
#[test]
fn the_detail_stage_refuses_a_re_synthesised_zone() {
    let src = textured(384, 256, 1.0, 1.0, None);
    let tgt = textured(384, 256, 1.6, 1.0, Some(11));
    let (zone, raster) = whole_frame_zone("detail-drawn", 384, 256);
    let mut report = super::super::tests::neutral_report(&src, &tgt);
    let masks_before = report.recipe.masks.len();
    attach_zone_detail(&src, &tgt, &mut report, &[zone]);
    assert_eq!(report.recipe.masks.len(), masks_before, "{}", report.recipe.rationale);
    assert!(has_note(&report, crate::rationale::keys::DETAIL_UNPAIRED), "{}", report.recipe.rationale);
    raster.remove();
}

/// The asked gain is the slope on the shared structure, and only when the
/// whole band agrees on the direction.
#[test]
fn the_asked_gain_is_the_slope_the_ratio_corroborates() {
    let reading = |r: f32, ratio: f32| BandReading { r, target: ratio * 0.01, render: 0.01 };
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    // Well paired, busier target: the slope, nearly the ratio.
    assert!(close(asked_gain(reading(0.95, 1.3)), 0.95 * 1.3));
    // Well paired, softer target: the slope below one.
    assert!(close(asked_gain(reading(0.9, 0.8)), 0.72));
    // Half paired, busier target, corresponding structure softer: no ask
    // (the busy street's ground, 2026-10-21).
    assert!(close(asked_gain(reading(0.65, 1.37)), 1.0));
    // Nothing on the render side: unity.
    assert!(close(asked_gain(BandReading { r: 0.0, target: 0.01, render: 0.0 }), 1.0));
}

/// A paired band may rise to the target's energy; an unpaired band may not
/// rise past where it stood; neither is a push when it was not raised.
#[test]
fn the_energy_ceiling_follows_the_evidence() {
    let at = |render: f32, target: f32| BandReading { r: 0.0, target, render };
    // Paired: under the target is fine, over it is a push.
    assert!(!pushed_past_evidence(true, at(0.010, 0.020), at(0.019, 0.020)));
    assert!(pushed_past_evidence(true, at(0.010, 0.020), at(0.022, 0.020)));
    // Unpaired: the target's energy is no ceiling — the same rise is a push.
    assert!(pushed_past_evidence(false, at(0.010, 0.020), at(0.013, 0.020)));
    // Unpaired: within the tolerance of where it stood is not a push.
    assert!(!pushed_past_evidence(false, at(0.010, 0.020), at(0.0104, 0.020)));
    // Standing above the ceiling without being raised is not a push.
    assert!(!pushed_past_evidence(true, at(0.030, 0.020), at(0.028, 0.020)));
    assert!(!pushed_past_evidence(false, at(0.030, 0.020), at(0.030, 0.020)));
}

/// A target with far more mid-scale contrast than clarity +100 can make
/// gets no slider: the solve runs to the stop, does not land, and says so —
/// rather than shipping the stop as a match.
#[test]
fn the_detail_stage_refuses_what_the_slider_cannot_reach() {
    let (w, h) = (384u32, 256u32);
    let src = textured(w, h, 1.0, 1.0, None);
    let r_cla = crate::render::clarity_radius(w as usize, h as usize);
    let tgt = image_of(&unsharpened(&src, r_cla, 4.0), w, h);
    let (zone, raster) = whole_frame_zone("detail-reach", w, h);
    let mut report = super::super::tests::neutral_report(&src, &tgt);
    let masks_before = report.recipe.masks.len();
    attach_zone_detail(&src, &tgt, &mut report, &[zone]);
    assert_eq!(report.recipe.masks.len(), masks_before, "{}", report.recipe.rationale);
    assert!(has_note(&report, crate::rationale::keys::DETAIL_REFUSED_REACH), "{}", report.recipe.rationale);
    raster.remove();
}

/// An already-matched zone is left alone and says so; nothing is dialled
/// for a hairline.
#[test]
fn the_detail_stage_leaves_a_matched_zone_alone() {
    let src = textured(384, 256, 1.0, 1.0, None);
    let (zone, raster) = whole_frame_zone("detail-same", 384, 256);
    let mut report = super::super::tests::neutral_report(&src, &src);
    let masks_before = report.recipe.masks.len();
    attach_zone_detail(&src, &src, &mut report, &[zone]);
    assert_eq!(report.recipe.masks.len(), masks_before, "{}", report.recipe.rationale);
    assert!(has_note(&report, crate::rationale::keys::DETAIL_ALREADY_MATCHED), "{}", report.recipe.rationale);
    raster.remove();
}

/// A zone whose tone was refused has released its raster; the stage puts
/// the alpha back for the sliders it ships, and takes it away again when it
/// ships nothing.
#[test]
fn the_detail_stage_restores_a_released_raster_only_when_it_ships() {
    let src = textured(384, 256, 1.0, 1.0, None);
    let tgt = image_of(&textured_by_engine(&src, 0.5), 384, 256);
    let (zone, raster) = whole_frame_zone("detail-restore", 384, 256);
    std::fs::remove_file(raster.path()).unwrap();
    let mut report = super::super::tests::neutral_report(&src, &tgt);
    attach_zone_detail(&src, &tgt, &mut report, &[zone]);
    assert!(detail_mask(&report).is_some(), "{}", report.recipe.rationale);
    assert!(raster.path().exists(), "the shipped mask's alpha must be on disk");
    raster.remove();

    let (zone, raster) = whole_frame_zone("detail-restore-none", 384, 256);
    std::fs::remove_file(raster.path()).unwrap();
    let mut report = super::super::tests::neutral_report(&src, &src);
    attach_zone_detail(&src, &src, &mut report, &[zone]);
    assert!(detail_mask(&report).is_none());
    assert!(!raster.path().exists(), "nothing shipped, nothing restored");
    raster.remove();
}

/// The constants the module doc derives, pinned where they are read.
#[test]
fn the_detail_constants_are_the_calibrated_ones() {
    assert_eq!(DETAIL_EDGE, 768);
    assert_eq!(texture_radius(768, 512), 3);
    assert_eq!(crate::render::clarity_radius(768, 512), 10);
    // At the analysis size both radii are their floors, not their ratios.
    assert_eq!(texture_radius(384, 256), 2);
    assert_eq!(crate::render::clarity_radius(384, 256), 8);
    assert_eq!(DETAIL_SHRINK_LADDER, [0.75, 0.5, 0.25]);
    assert_eq!((DETAIL_MATCHED_GAIN, DETAIL_ENERGY_TOL, DETAIL_STEPS), (0.05, 0.05, 3));
    assert_eq!(DETAIL_TEXTURE_RANGE, (0.0, 100.0), "texture has no negative half here");
    assert_eq!(DETAIL_CLARITY_RANGE, (-100.0, 100.0));
}
