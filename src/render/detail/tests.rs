//! The Detail panel's operators, alone and through the develop.

use super::*;

/// A `w × h` frame whose pixel at `(x, y)` is `f(x, y)`.
fn frame(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Vec<[f32; 3]> {
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect()
}

/// Replayable noise in −1..1 at `(x, y)`: a hash of the position and a
/// seed, so a fixture carries no noise buffer of its own.
fn hashed(seed: u32, x: usize, y: usize) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D);
    for shift in [15u32, 12, 15] {
        v ^= v >> shift;
        v = v.wrapping_mul(0x2C1B_3C6D);
    }
    v as f32 / u32::MAX as f32 * 2.0 - 1.0
}

/// Population variance of luma over the pixels `pick` selects.
fn luma_spread(d: &[[f32; 3]], pick: impl Fn(usize) -> bool) -> f32 {
    let l: Vec<f32> =
        d.iter().enumerate().filter(|(i, _)| pick(*i)).map(|(_, p)| luma601(p)).collect();
    let m = l.iter().sum::<f32>() / l.len() as f32;
    l.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / l.len() as f32
}

fn everywhere(_: usize, _: usize, _: &[f32; 3]) -> f32 {
    1.0
}

fn sharp(amount: f32, detail: f32, masking: f32) -> SharpenParams {
    SharpenParams { amount, radius: 1.0, detail, masking }
}

fn nr(amount: f32, detail: f32, contrast: f32) -> LumaNrParams {
    LumaNrParams { amount, detail, contrast }
}

/// Dark left half, light right half.
fn step(w: usize, h: usize) -> Vec<[f32; 3]> {
    frame(w, h, |x, _| if x < w / 2 { [0.3; 3] } else { [0.6; 3] })
}

#[test]
fn film_scale_is_the_short_edge_ratio_and_never_below_one() {
    assert_eq!(FilmScale::of(Some(6336), 1280, 853).factor(), 6336.0 / 853.0);
    assert_eq!(FilmScale::of(Some(4000), 6000, 4000), FilmScale::NATIVE);
    assert_eq!(FilmScale::of(Some(100), 6000, 4000), FilmScale::NATIVE, "never above its film");
    assert_eq!(FilmScale::of(None, 1280, 853), FilmScale::NATIVE);
    assert_eq!(FilmScale::of(Some(6336), 0, 0), FilmScale::NATIVE);
    assert_eq!(FilmScale::of(Some(4000), 2000, 1000).raster_px(3.0), 0.75);
}

#[test]
fn sharpening_overshoots_a_step_and_leaves_a_flat_field_alone() {
    let mut flat = frame(32, 4, |_, _| [0.4, 0.5, 0.6]);
    let before = flat.clone();
    sharpen(&mut flat, 32, 4, &sharp(1.0, 0.25, 0.0), FilmScale::NATIVE, everywhere);
    // Not bit for bit: the Gaussian's taps are normalised in f64 and quoted to
    // f32, so a constant plane blurs to itself within an ULP or two, and that
    // residue reaches the luma ratio. 1e-6 is the tolerance the pre-v1.5.0
    // sharpening test held its flat field to, a 3700th of an 8-bit code.
    let drift = flat
        .iter()
        .zip(&before)
        .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
        .fold(0.0f32, f32::max);
    assert!(drift < 1e-6, "no detail signal, no change: {drift}");
    let mut s = step(32, 4);
    sharpen(&mut s, 32, 4, &sharp(1.0, 0.25, 0.0), FilmScale::NATIVE, everywhere);
    assert!(s[16][0] > 0.6 && s[15][0] < 0.3, "{:?}", &s[15..17]);
}

#[test]
fn detail_is_the_gain_on_the_finest_signal() {
    // A small step (0.15 / 0.17), well under every cap: what Detail does shows
    // whole. On the oracle Detail 0 took the bright side's gain to 0.36 of
    // Detail 25's and Detail 100 to 2.4 of it; the caps barely moved.
    let overshoot = |detail: f32| {
        let mut s = frame(32, 4, |x, _| if x < 16 { [0.15; 3] } else { [0.17; 3] });
        sharpen(&mut s, 32, 4, &sharp(0.4, detail, 0.0), FilmScale::NATIVE, everywhere);
        s[16][0] - 0.17
    };
    let (low, mid, high) = (overshoot(0.0), overshoot(0.25), overshoot(1.0));
    assert!(low > 0.0 && low < 0.5 * mid, "Detail 0 sharpens less: {low} vs {mid}");
    assert!(high > 2.0 * mid, "Detail 100 sharpens more: {high} vs {mid}");
    // The factor runs linearly in its logarithm between the measured ends.
    let half = detail_factor(SHARPEN.detail0.gain_bright, SHARPEN.detail100.gain_bright, 0.125);
    assert!((half - SHARPEN.detail0.gain_bright.sqrt()).abs() < 1e-6, "{half}");
}

#[test]
fn masking_spares_flat_noise_but_still_sharpens_the_edge() {
    let (w, h) = (48, 24);
    let base = frame(w, h, |x, y| [if x < w / 2 { 0.2 } else { 0.7 } + 0.004 * hashed(7, x, y); 3]);
    let flat_left = |i: usize| (4..20).contains(&(i / w)) && (4..16).contains(&(i % w));
    let after = |masking: f32| {
        let mut d = base.clone();
        sharpen(&mut d, w, h, &sharp(1.0, 1.0, masking), FilmScale::NATIVE, everywhere);
        (luma_spread(&d, flat_left), d[12 * w + w / 2][0])
    };
    let original = luma_spread(&base, flat_left);
    let (open, _) = after(0.0);
    let (masked, edge) = after(1.0);
    assert!(open > 1.5 * original, "Masking 0 sharpens the noise too");
    assert!(masked < 1.05 * original, "Masking 100 leaves flat noise alone");
    assert!(edge > 0.7, "…while the edge is still sharpened: {edge}");
}

#[test]
fn a_downscaled_raster_sharpens_by_what_survives_the_downscale() {
    // A 1280 px preview of a 6336 px short edge: the export's Radius 1.0 (a
    // 0.75 px σ) lifts a few percent of what the sampled σ would at Nyquist.
    let film = FilmScale::of(Some(6336), 1280, 853);
    let fade = usm_transfer(film.raster_px(SHARPEN.sigma_per_radius)) / usm_transfer(SHARPEN_MIN_SIGMA_PX);
    assert!(fade > 0.02 && fade < 0.2, "{fade}");
    let (mut full, mut preview) = (step(32, 4), step(32, 4));
    sharpen(&mut full, 32, 4, &sharp(1.0, 1.0, 0.0), FilmScale::NATIVE, everywhere);
    sharpen(&mut preview, 32, 4, &sharp(1.0, 1.0, 0.0), film, everywhere);
    assert!(preview[16][0] - 0.6 < 0.5 * (full[16][0] - 0.6));
}

#[test]
fn negative_local_sharpness_softens() {
    let mut s = step(32, 4);
    sharpen(&mut s, 32, 4, &sharp(-1.0, 0.25, 0.0), FilmScale::NATIVE, everywhere);
    assert!(s[16][0] < 0.6 && s[15][0] > 0.3, "{:?}", &s[15..17]);
}

#[test]
fn the_dark_halo_digs_with_the_pixels_own_luminance() {
    // Two steps of one height (0.30) at two levels, at Lightroom's RAW default
    // of 40. The law's dark cap is a table over the pixel's OWN luma (0.0045
    // at 0.10, 0.14 at 0.40), so the dark side of the dim step (0.10 → 0.40)
    // hardly dips while the dark side of the bright step (0.40 → 0.70) dips by
    // two orders more — the ring Camera Raw digs around a bright star and not
    // around a faint one. The light side of both steps lifts alike.
    let edge = |lo: f32, hi: f32| {
        let mut s = frame(32, 4, |x, _| if x < 16 { [lo; 3] } else { [hi; 3] });
        sharpen(&mut s, 32, 4, &sharp(0.4, 0.25, 0.0), FilmScale::NATIVE, everywhere);
        (lo - s[15][0], s[16][0] - hi)
    };
    let (dim_dip, dim_lift) = edge(0.10, 0.40);
    let (bright_dip, bright_lift) = edge(0.40, 0.70);
    assert!(dim_dip > 0.0 && dim_dip < 0.01, "a dim edge's dark side is held: {dim_dip}");
    assert!(bright_dip > 0.05 && bright_dip > 20.0 * dim_dip, "a bright edge's digs: {bright_dip} vs {dim_dip}");
    assert!(dim_lift > 0.05 && (bright_lift - dim_lift).abs() < 0.02, "both light sides lift alike: {dim_lift} {bright_lift}");
}

#[test]
fn luminance_nr_flattens_noise_and_keeps_a_strong_edge() {
    let mut d = frame(40, 40, |x, y| [0.5 + 0.02 * hashed(3, x, y); 3]);
    let before = luma_spread(&d, |_| true);
    luma_nr(&mut d, 40, 40, &nr(0.5, 0.5, 0.0), FilmScale::NATIVE, everywhere);
    assert!(luma_spread(&d, |_| true) < 0.5 * before);
    // At full strength (a 4 px neighbourhood) a 0.3 step keeps most of its
    // height across its two boundary pixels, where a plain box of the same
    // radius keeps a ninth. It does not keep all of it: the self-guided
    // filter blends a boundary pixel toward its neighbourhood by `1 − ā`,
    // measured 0.856 of the step here — how far Lightroom's Luminance 100
    // softens an edge is what the `NR-100` kit export pins. Past the reach of
    // the two box passes both levels stand.
    let mut s = step(40, 40);
    luma_nr(&mut s, 40, 40, &nr(1.0, 0.5, 0.0), FilmScale::NATIVE, everywhere);
    let row = 5 * 40;
    let kept = (s[row + 20][0] - s[row + 19][0]) / 0.3;
    assert!(kept > 0.8, "the boundary keeps {kept} of the step");
    let (dark, light) = (s[row + 5][0], s[row + 34][0]);
    assert!((dark - 0.3).abs() < 1e-4 && (light - 0.6).abs() < 1e-4, "{dark} {light}");
}

#[test]
fn higher_nr_detail_keeps_more_texture() {
    let kept = |detail: f32| {
        let mut d = frame(40, 40, |x, y| [0.5 + 0.02 * hashed(11, x, y); 3]);
        luma_nr(&mut d, 40, 40, &nr(0.5, detail, 0.0), FilmScale::NATIVE, everywhere);
        luma_spread(&d, |_| true)
    };
    assert!(kept(1.0) > kept(0.0));
}

#[test]
fn nr_contrast_restores_low_frequency_structure() {
    // A gentle ripple under the noise: Contrast puts more of its local
    // variation back than Contrast 0 does.
    let kept = |contrast: f32| {
        let mut d =
            frame(64, 8, |x, y| [0.4 + 0.01 * (x as f32 / 8.0).sin() + 0.02 * hashed(5, x, y); 3]);
        luma_nr(&mut d, 64, 8, &nr(1.0, 0.0, contrast), FilmScale::NATIVE, everywhere);
        luma_spread(&d, |_| true)
    };
    assert!(kept(1.0) > kept(0.0));
}

#[test]
fn colour_nr_removes_colour_noise_and_leaves_luma() {
    let mut d =
        frame(64, 64, |x, y| [0.5 + 0.05 * hashed(21, x, y), 0.5, 0.5 + 0.05 * hashed(22, x, y)]);
    let luma_before: Vec<f32> = d.iter().map(luma601).collect();
    let red_minus_luma = |d: &[[f32; 3]]| {
        let c: Vec<[f32; 3]> = d.iter().map(|p| [p[0] - luma601(p); 3]).collect();
        luma_spread(&c, |_| true)
    };
    let before = red_minus_luma(&d);
    let p = ChromaNrParams { amount: 0.5, detail: 0.5, smoothness: 0.5 };
    chroma_nr(&mut d, 64, 64, &p, FilmScale::NATIVE);
    assert!(red_minus_luma(&d) < 0.2 * before);
    let drift =
        d.iter().zip(&luma_before).map(|(p, l)| (luma601(p) - l).abs()).fold(0.0, f32::max);
    assert!(drift < 1e-4, "luma is not colour noise: {drift}");
}

/// Colour NR keeps a pure black black and every pixel's luma, at Lightroom's
/// default Color 25 and at 100: half the frame exact zeros among dark,
/// strongly coloured pixels (their neighbours lend a zero colour, which needs
/// a negative channel) and a band of bright saturated ones (which needs a
/// channel above 1).
///
/// MUTATION: clamp each rebuilt channel to `[0, 1]` again.
#[test]
fn colour_nr_keeps_pure_black_and_luma_out_of_range() {
    for amount in [0.25, 1.0] {
        let mut d = frame(64, 64, |x, y| {
            if y >= 48 {
                [0.98, 0.9 + 0.05 * hashed(41, x, y), 0.2]
            } else if hashed(31, x, y) > 0.0 {
                [0.0; 3]
            } else {
                [0.06 + 0.03 * hashed(32, x, y), 0.004, 0.08]
            }
        });
        let luma_before: Vec<f32> = d.iter().map(luma601).collect();
        let zeros = |d: &[[f32; 3]]| d.iter().filter(|p| **p == [0.0; 3]).count();
        let before = zeros(&d);
        assert!(before > 64 * 48 / 4, "the fixture holds zeros: {before}");
        let p = ChromaNrParams { amount, detail: 0.5, smoothness: 0.5 };
        chroma_nr(&mut d, 64, 64, &p, FilmScale::NATIVE);
        assert_eq!(zeros(&d), before, "Color {}: a pure black left 0", amount * 100.0);
        let drift =
            d.iter().zip(&luma_before).map(|(p, l)| (luma601(p) - l).abs()).fold(0.0, f32::max);
        assert!(drift < 1e-5, "Color {}: luma moved {drift}", amount * 100.0);
    }
}

#[test]
fn colour_nr_detail_keeps_a_colour_edge_that_a_luma_edge_draws() {
    // Left: dark red; right: light blue. The luma step marks the colour edge.
    let bleed = |detail: f32| {
        let mut d = frame(64, 16, |x, _| if x < 32 { [0.35, 0.1, 0.1] } else { [0.5, 0.7, 0.9] });
        let p = ChromaNrParams { amount: 1.0, detail, smoothness: 0.0 };
        chroma_nr(&mut d, 64, 16, &p, FilmScale::NATIVE);
        let px = d[8 * 64 + 30];
        (px[0] - 0.35).abs() + (px[2] - 0.1).abs()
    };
    assert!(bleed(1.0) < bleed(0.0), "{} vs {}", bleed(1.0), bleed(0.0));
}

#[test]
fn every_pass_is_inert_at_amount_zero() {
    let base = frame(16, 16, |x, y| [0.5 + 0.05 * hashed(9, x, y); 3]);
    let mut d = base.clone();
    sharpen(&mut d, 16, 16, &sharp(0.0, 0.5, 0.5), FilmScale::NATIVE, everywhere);
    luma_nr(&mut d, 16, 16, &nr(0.0, 0.5, 0.5), FilmScale::NATIVE, everywhere);
    let p = ChromaNrParams { amount: 0.0, detail: 0.5, smoothness: 0.5 };
    chroma_nr(&mut d, 16, 16, &p, FilmScale::NATIVE);
    assert_eq!(d, base);
}

/// A noisy two-tone RGB8 photograph: the fixture the develop-level tests run.
fn noisy_photo(w: u32, h: u32) -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        let level = if x < w / 2 { 70.0 } else { 170.0 };
        let channel =
            |seed: u32| (level + 18.0 * hashed(seed, x as usize, y as usize)).clamp(0.0, 255.0) as u8;
        image::Rgb([channel(1), channel(2), channel(3)])
    }))
}

/// The registry's claim (`Tier::Rendered` on the eight Detail axes, v1.5.0),
/// made from the ENGINE's side and through the real develop: under its
/// amount, each axis moves pixels.
///
/// The axes and their probes are DERIVED, so a ninth member of the family is
/// probed the day it joins: a COMPANION probes its real zero (through
/// `set_resolved`, away from Lightroom's non-zero default — the value
/// `explicit_zero` exists to hold), every other axis the top of its band.
#[test]
fn every_detail_axis_moves_the_develop_under_its_amount() {
    use crate::advisor::catalogue::{global_control, CONTROL_FAMILIES};
    let img = noisy_photo(96, 64);
    let amounts = EditRecipe { sharpening: 80.0, noise_reduction: 60.0, color_nr: 60.0, ..Default::default() };
    let base = crate::render::develop_preview(&img, &amounts).to_rgb8();
    let family = CONTROL_FAMILIES.iter().find(|f| f.name == "detail_effects").expect("the family");
    assert_eq!(family.members.len(), 8, "premise: the eight Detail axes");
    for name in family.members {
        let band = global_control(name).and_then(|c| c.range).expect("a banded registry row");
        let companion = crate::recipe::LR_COMPANION_DEFAULTS.iter().any(|(n, _)| n == name);
        let value = if companion { 0.0 } else { band.1 };
        let mut json = serde_json::to_value(&amounts).expect("recipe serialises");
        json[*name] = serde_json::json!(value);
        let mut r: EditRecipe = serde_json::from_value(json).expect("an in-range probe");
        r.set_resolved(name, value);
        let out = crate::render::develop_preview(&img, &r).to_rgb8();
        assert_ne!(out.as_raw(), base.as_raw(), "{name} = {value} moved no pixel");
    }
}

/// The film scale reaches the develop: the same sharpening developed as a
/// raster eight times smaller than its film lifts the frame less than when
/// the raster is its own film.
#[test]
fn the_film_scale_reaches_the_develop() {
    let img = noisy_photo(96, 64);
    let neutral = crate::render::develop_preview(&img, &EditRecipe::default()).to_rgb8();
    let r = EditRecipe { sharpening: 150.0, ..Default::default() };
    let moved = |out: &image::RgbImage| -> u64 {
        out.as_raw().iter().zip(neutral.as_raw()).map(|(a, b)| u64::from(a.abs_diff(*b))).sum()
    };
    let native = moved(&crate::render::develop_preview(&img, &r).to_rgb8());
    let diag = crate::diag::pixels();
    let as_preview = moved(&crate::render::develop_preview_film(&img, &r, &diag, Some(64 * 8), &crate::render::SourceFrame::Baked(None)).to_rgb8());
    assert!(native > 0, "premise: sharpening 150 moves the native develop");
    assert!(as_preview < native / 2, "{as_preview} vs {native}");
}
