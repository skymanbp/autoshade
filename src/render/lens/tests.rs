//! De-fringing: what it removes, and everything it must leave alone.

use super::*;

/// A `w × 8` frame with a luminance step down the middle and a two-pixel
/// fringe of `hue` straddling it — the shape the operator exists for.
fn fringed(w: usize, fringe: [f32; 3]) -> Vec<[f32; 3]> {
    (0..8)
        .flat_map(|_| 0..w)
        .map(|x| {
            let mid = w / 2;
            if x + 1 >= mid && x <= mid {
                fringe
            } else if x < mid {
                [0.12, 0.12, 0.12]
            } else {
                [0.86, 0.86, 0.86]
            }
        })
        .collect()
}

fn purple_defringe(amount: f32) -> EditRecipe {
    EditRecipe {
        defringe_purple: amount,
        defringe_purple_lo: 30.0,
        defringe_purple_hi: 70.0,
        ..Default::default()
    }
}

/// The fringe colours: a violet-magenta and a green, both well inside Adobe's
/// own default windows once they are read on this module's scales.
const PURPLE: [f32; 3] = [0.62, 0.30, 0.86];
const GREEN: [f32; 3] = [0.30, 0.80, 0.36];

#[test]
fn a_fringe_on_an_edge_loses_its_colour_and_keeps_its_brightness() {
    let (w, h) = (16, 8);
    let mut data = fringed(w, PURPLE);
    let before = data.clone();
    defringe(&mut data, w, h, &purple_defringe(20.0));
    let (i, j) = (4 * w + w / 2 - 1, 4 * w + w / 2);
    for k in [i, j] {
        assert!(
            chroma(&data[k]) < chroma(&before[k]) * 0.1,
            "the fringe kept its colour: {:?} → {:?}",
            before[k],
            data[k]
        );
        assert!(
            (luma601(&data[k]) - luma601(&before[k])).abs() < 1e-3,
            "a de-fringe must not change brightness: {:?} → {:?}",
            before[k],
            data[k]
        );
    }
    // The step itself is untouched: it carries no chroma to remove.
    for k in [4 * w, 4 * w + w - 1] {
        assert_eq!(data[k], before[k], "the picture either side of the edge is not the fringe");
    }
}

#[test]
fn a_saturated_subject_away_from_an_edge_is_never_touched() {
    let (w, h) = (16, 8);
    let mut flat: Vec<[f32; 3]> = vec![PURPLE; w * h];
    let before = flat.clone();
    defringe(&mut flat, w, h, &purple_defringe(20.0));
    assert_eq!(flat, before, "a flat purple field is a photograph of something purple");
}

#[test]
fn each_window_only_answers_for_its_own_hue() {
    let (w, h) = (16, 8);
    let mut green_under_purple = fringed(w, GREEN);
    let before = green_under_purple.clone();
    defringe(&mut green_under_purple, w, h, &purple_defringe(20.0));
    assert_eq!(green_under_purple, before, "the purple sliders must not touch a green fringe");

    let mut green = fringed(w, GREEN);
    let r = EditRecipe {
        defringe_green: 20.0,
        defringe_green_lo: 40.0,
        defringe_green_hi: 60.0,
        ..Default::default()
    };
    defringe(&mut green, w, h, &r);
    assert!(
        chroma(&green[4 * w + w / 2]) < chroma(&before[4 * w + w / 2]) * 0.2,
        "the green sliders must answer for it: {:?}",
        green[4 * w + w / 2]
    );
}

#[test]
fn the_window_sliders_move_the_window() {
    let (w, h) = (16, 8);
    let k = 4 * w + w / 2;
    let moved_off = EditRecipe { defringe_purple_lo: 85.0, ..purple_defringe(20.0) };
    let mut data = fringed(w, PURPLE);
    let before = data.clone();
    defringe(&mut data, w, h, &moved_off);
    assert_eq!(data, before, "a window that no longer covers the hue corrects nothing");
    // And a wider window than Adobe's default still covers it.
    let mut wide = fringed(w, PURPLE);
    defringe(&mut wide, w, h, &EditRecipe { defringe_purple_hi: 100.0, ..purple_defringe(20.0) });
    assert!(chroma(&wide[k]) < chroma(&before[k]) * 0.2);
}

#[test]
fn the_amount_is_proportional_and_zero_is_a_no_op() {
    let (w, h) = (16, 8);
    let k = 4 * w + w / 2;
    let run = |amount: f32| {
        let mut d = fringed(w, PURPLE);
        defringe(&mut d, w, h, &purple_defringe(amount));
        chroma(&d[k])
    };
    let full = run(20.0);
    let half = run(10.0);
    let none = run(0.0);
    assert!(full < half && half < none, "amount must be a dial: {full} {half} {none}");
    assert_eq!(none, chroma(&PURPLE), "0 is Lightroom's own default and moves nothing");
}

// ── the automatic lateral-CA solver ──────────────────────────────────────────

/// Concentric rings, with the red channel magnified about the centre by
/// `scale_r` and the blue by `scale_b` — lateral CA, synthesised: a feature at
/// radius `r` in green appears at `r · scale` in that channel.
fn rings(w: usize, h: usize, scale_r: f32, scale_b: f32) -> Vec<[f32; 3]> {
    let (cx, cy) = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
    let rmax = (cx * cx + cy * cy).sqrt();
    let ring = |rn: f32| 0.5 + 0.34 * (rn * 28.0).sin();
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let rn = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / rmax;
            [ring(rn / scale_r), ring(rn), ring(rn / scale_b)]
        })
        .collect()
}

/// Mean |R − G| and |B − G| where the green channel has an edge — the fringe a
/// photographer sees.
fn misalignment(data: &[[f32; 3]], w: usize, h: usize) -> (f32, f32) {
    let (mut sr, mut sb, mut n) = (0.0f32, 0.0f32, 0usize);
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let i = y * w + x;
            if (data[i + 1][1] - data[i - 1][1]).abs() < 0.02 {
                continue;
            }
            sr += (data[i][0] - data[i][1]).abs();
            sb += (data[i][2] - data[i][1]).abs();
            n += 1;
        }
    }
    let n = n.max(1) as f32;
    (sr / n, sb / n)
}

#[test]
fn the_solver_reads_a_magnified_channel_in_the_manual_sliders_own_units() {
    let (w, h) = (129, 129);
    let units = 12.0;
    let data = rings(w, h, 1.0 + units * crate::render::MANUAL_CA_PER_UNIT, 1.0);
    let (dr, db) = solve_lateral_ca(&data, w, h);
    assert!(
        (dr.abs() - units).abs() <= 3.0,
        "a {units}-unit red magnification must read as about {units} units, not {dr}"
    );
    assert!(db.abs() <= 2.0, "and the blue channel, which is aligned, as nothing: {db}");
}

#[test]
fn the_auto_switch_undoes_the_misalignment_it_measured() {
    let (w, h) = (129, 129);
    let scale = 1.0 + 12.0 * crate::render::MANUAL_CA_PER_UNIT;
    let data = rings(w, h, scale, 1.0 / scale);
    let (br, bb) = misalignment(&data, w, h);
    let r = EditRecipe { auto_lateral_ca: true, ..Default::default() };
    let solved = with_auto_lateral_ca(&r, &data, w, h);
    assert!(solved.ca_r != 0.0 && solved.ca_b != 0.0, "the solver said nothing: {solved:?}");
    assert!(
        solved.ca_r.signum() != solved.ca_b.signum(),
        "the two channels are misaligned in opposite directions and the answer must be too:          {} {}",
        solved.ca_r,
        solved.ca_b
    );
    // Through the SAME path the render uses: the composed profile, then the
    // resample. What is left must be smaller than what it started with.
    let img = image::ImageBuffer::from_fn(w as u32, h as u32, |x, y| {
        let px = data[y as usize * w + x as usize];
        image::Rgb([
            crate::render::to_u16(px[0]),
            crate::render::to_u16(px[1]),
            crate::render::to_u16(px[2]),
        ])
    });
    let geom = crate::render::geometry_profile(&solved);
    let out = crate::render::apply_lens_geometry(
        &image::DynamicImage::ImageRgb16(img),
        &geom,
        0.0,
    )
    .to_rgb16();
    let after: Vec<[f32; 3]> = out
        .pixels()
        .map(|p| [p.0[0] as f32 / 65535.0, p.0[1] as f32 / 65535.0, p.0[2] as f32 / 65535.0])
        .collect();
    let (ar, ab) = misalignment(&after, out.width() as usize, out.height() as usize);
    assert!(ar < br * 0.6, "the red fringe survived: {br} → {ar}");
    assert!(ab < bb * 0.6, "the blue fringe survived: {bb} → {ab}");
}

#[test]
fn a_colour_cast_is_not_a_chromatic_aberration() {
    let (w, h) = (129, 129);
    let mut data = rings(w, h, 1.0, 1.0);
    for px in data.iter_mut() {
        px[0] = (px[0] + 0.12).min(1.0); // a warm cast on an aligned frame
        px[2] = (px[2] - 0.08).max(0.0);
    }
    let (dr, db) = solve_lateral_ca(&data, w, h);
    assert!(dr.abs() <= 2.0 && db.abs() <= 2.0, "the intercept must absorb a cast: {dr} {db}");
}

#[test]
fn the_switch_at_rest_borrows_the_recipe_and_a_flat_frame_says_nothing() {
    let (w, h) = (64, 64);
    let flat = vec![[0.5, 0.5, 0.5]; w * h];
    assert_eq!(solve_lateral_ca(&flat, w, h), (0.0, 0.0), "no edges, no estimate");
    let off = EditRecipe::default();
    assert!(
        matches!(with_auto_lateral_ca(&off, &flat, w, h), std::borrow::Cow::Borrowed(_)),
        "a photo that never asked must allocate nothing"
    );
    // ... and the solved value ADDS to a manual pair rather than replacing it.
    //
    // Stated as the EXACT SUM, because "it differs from the manual 5" is true
    // of a replacement too: the solver answers about −12 here, and both
    // `5 + (−12)` and a bare `−12` are far from 5. The invariant is the sum, so
    // the bare answer is measured on the same frame and added here.
    let data = rings(129, 129, 1.0 + 12.0 * crate::render::MANUAL_CA_PER_UNIT, 1.0);
    // Bound, not inline: the returned `Cow` borrows the recipe it was given.
    let on = EditRecipe { auto_lateral_ca: true, ..Default::default() };
    let bare = with_auto_lateral_ca(&on, &data, 129, 129);
    assert!(bare.ca_r != 0.0, "the probe must give the solver something to say: {bare:?}");
    let manual = EditRecipe { auto_lateral_ca: true, ca_r: 5.0, ..Default::default() };
    let solved = with_auto_lateral_ca(&manual, &data, 129, 129);
    assert_eq!(
        solved.ca_r,
        bare.ca_r + 5.0,
        "the manual 5 must still be in there with the solver's {} on top",
        bare.ca_r
    );
    // The blue half of the pair travels the same way, so it is stated too.
    assert_eq!(solved.ca_b, bare.ca_b, "an untouched manual blue keeps the bare answer");
}
