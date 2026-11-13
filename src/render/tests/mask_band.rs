// One part of the engine's tests (src/render/tests.rs includes it): a raster mask's band on a content edge (J149) — the sides a band lies between, the light share a band pixel is owed between them, how far it is trusted, and the rim the area weight drew.

/// A grey frame of two encoded levels, `left` of column `edge` and `right` of
/// it, the edge column half of each in linear light: one mixed pixel a row.
fn band_frame(w: usize, h: usize, edge: usize, left: f32, right: f32) -> Vec<[f32; 3]> {
    let mixed = linear_to_srgb(0.5 * (srgb_to_linear(left) + srgb_to_linear(right)));
    (0..w * h)
        .map(|i| match (i % w).cmp(&edge) {
            std::cmp::Ordering::Less => [left; 3],
            std::cmp::Ordering::Equal => [mixed; 3],
            std::cmp::Ordering::Greater => [right; 3],
        })
        .collect()
}

/// [`band_frame`] as the 8-bit picture a preview develops.
fn band_scene(w: u32, h: u32, edge: u32, left: u8, right: u8) -> DynamicImage {
    let frame = band_frame(w as usize, h as usize, edge as usize, left as f32 / 255.0, right as f32 / 255.0);
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        image::Rgb(frame[(y * w + x) as usize].map(|c| (c * 255.0).round() as u8))
    }))
}

/// A frame with one grey level a pixel.
fn grey_frame(w: usize, h: usize, level: impl Fn(usize, usize) -> f32) -> Vec<[f32; 3]> {
    (0..w * h).map(|i| [level(i % w, i / w); 3]).collect()
}

/// A layer's band ([`mask_owed::layer_band`]) as the one weight per lane each
/// of its pixels is owed where its two contents are told as one — each
/// content's weight in the share of the pixel's light that is that content's
/// (`Owed::weights`).
fn band_weights(
    data: &[[f32; 3]],
    dims: (usize, usize),
    block: usize,
    budget: usize,
    coverage: &(dyn Fn(usize, usize) -> f32 + Sync),
) -> Vec<Vec<(u32, [f32; 4])>> {
    let rows = mask_band::layer_band(data, dims, block, budget, coverage);
    rows.into_iter().map(|row| row.into_iter().map(|(x, owed)| (x, owed.weights())).collect()).collect()
}

/// The furthest any listed weight stands from the layer's own coverage.
fn largest_move(rows: &[Vec<(u32, [f32; 4])>], coverage: &dyn Fn(usize, usize) -> f32) -> f32 {
    let mut most = 0.0f32;
    for (y, row) in rows.iter().enumerate() {
        for (x, lanes) in row {
            most = lanes.iter().fold(most, |m, v| m.max((v - coverage(*x as usize, y)).abs()));
        }
    }
    most
}

/// The columns a row lists.
fn listed(row: &[(u32, [f32; 4])]) -> Vec<u32> {
    row.iter().map(|entry| entry.0).collect()
}

/// The law itself, on one layer whose two sides stand at 0 and 1: a raster
/// whose step is softer than the frame's edge (as an upsampled raster's is)
/// gives a pure pixel under it none or all of the layer, and the one mixed
/// pixel the layer content's share of its LIGHT. Each band pixel walks the
/// raster down to the plateau at 0 and up to the one at 1, and those are its
/// two sides.
///
/// MUTATIONS THIS KILLS: `Sides::owed` handing back the area share in
/// `light_fraction`'s place — the mixed pixel then reads 0.5; the flat and
/// steep steps set past a quarter — this step is then no band at all;
/// `Blocks::walk` going nowhere — both sides are then the pixel itself.
#[test]
fn a_band_pixel_is_owed_its_contents_share_of_its_light() {
    let (w, h) = (40usize, 16usize);
    // Sky 0.5 on the left, land 0.1 on the right; the LAND layer's coverage
    // steps over columns 18..22.
    let data = band_frame(w, h, 20, 0.5, 0.1);
    let coverage = |x: usize, _: usize| ((x as f32 - 18.0) / 4.0).clamp(0.0, 1.0);
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    let want = light_fraction(0.5, srgb_to_linear(0.1), srgb_to_linear(0.5));
    assert!(want < 0.06, "premise: half the area of a dim content is a small share of the light: {want}");
    for row in &rows {
        assert!((18..=22).all(|x| listed(row).contains(&x)), "the raster's step is on the band: {:?}", listed(row));
        for (x, lanes) in row {
            let owed = match x.cmp(&20) {
                std::cmp::Ordering::Less => 0.0,
                std::cmp::Ordering::Equal => want,
                std::cmp::Ordering::Greater => 1.0,
            };
            let slack = if *x == 20 { 2e-3 } else { 1e-6 };
            assert!(lanes.iter().all(|v| (v - owed).abs() <= slack), "column {x}: {lanes:?} against {owed}");
        }
    }
}

/// A raster's sides are where it is FLAT, whatever coverage they stand at: a
/// layer at 0.10 over the sky and 0.75 over the land (the shape the fit's own
/// zone rasters have along a silhouette) owes a band pixel of pure sky the
/// sky side's 0.10 — what its flat neighbours get, no rim and no patch — one
/// of pure land the land side's 0.75, and the mixed pixel its light's share
/// of the way between. Read by "coverage near 0 or 1" this raster has no side
/// at all.
///
/// MUTATION THIS KILLS: the owed weight taken between 0 and 1 in the two
/// sides' own coverages' place.
#[test]
fn a_band_between_two_plateaus_is_owed_each_sides_own_coverage() {
    let (w, h) = (40usize, 16usize);
    let data = band_frame(w, h, 20, 0.5, 0.1);
    let coverage = |x: usize, _: usize| 0.10 + 0.65 * ((x as f32 - 18.0) / 4.0).clamp(0.0, 1.0);
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    let want = 0.10 + 0.65 * light_fraction(0.5, srgb_to_linear(0.1), srgb_to_linear(0.5));
    for row in &rows {
        assert!((18..=22).all(|x| listed(row).contains(&x)), "the raster's step is on the band: {:?}", listed(row));
        assert!(listed(row).iter().all(|x| (17..=23).contains(x)), "the flat pixels of either plateau are not listed: {:?}", listed(row));
        for (x, lanes) in row {
            let owed = match x.cmp(&20) {
                std::cmp::Ordering::Less => 0.10,
                std::cmp::Ordering::Equal => want,
                std::cmp::Ordering::Greater => 0.75,
            };
            let slack = if *x == 20 { 2e-3 } else { 1e-4 };
            assert!(lanes.iter().all(|v| (v - owed).abs() <= slack), "column {x}: {lanes:?} against {owed}");
        }
    }
}

/// A side's coverage is read WHERE THE WALK ENDS: a plateau that slopes
/// toward the step (0.10 where it meets it, two hundredths more each column
/// away) owes a pure pixel under the step the 0.10 it stands at beside the
/// step (0.108 as read), not the plain mean of the blocks that stand near
/// that (0.127).
///
/// MUTATION THIS KILLS: a side's plain mean coverage in its nearby one's
/// place.
#[test]
fn a_sloping_plateau_is_read_where_it_meets_the_step() {
    let (w, h) = (40usize, 16usize);
    let data = band_frame(w, h, 20, 0.5, 0.1);
    let coverage = |x: usize, _: usize| {
        if x <= 17 { 0.10 + 0.02 * (17 - x) as f32 } else { 0.10 + 0.65 * ((x as f32 - 18.0) / 4.0).clamp(0.0, 1.0) }
    };
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    for row in &rows {
        let (_, lanes) = row.iter().find(|entry| entry.0 == 19).expect("column 19 is under the step");
        assert!(lanes.iter().all(|v| (v - 0.10).abs() <= 0.01), "pure sky a column into the step: {lanes:?}");
    }
}

/// The ruler's ramp: two sides whose levels are within one code cannot be
/// told apart and the band keeps its area weight — nothing is listed.
///
/// MUTATION THIS KILLS: `told` fixed at 1 in `Blocks::sides`.
#[test]
fn a_band_over_levels_within_one_code_keeps_its_area_weight() {
    let (w, h) = (40usize, 16usize);
    let coverage = |x: usize, _: usize| ((x as f32 - 18.0) / 4.0).clamp(0.0, 1.0);
    let read = |codes: f32| {
        band_weights(&band_frame(w, h, 20, 0.5, 0.5 + codes / 255.0), (w, h), 1, w * h, &coverage)
    };
    assert!(read(0.0).is_empty(), "one level");
    assert!(read(0.5).is_empty(), "half a code apart");
    assert!(!read(3.0).is_empty(), "three codes apart is a mixture that can be read");
}

/// The spread ramp: where the two sides' levels sit inside their own
/// pixel-to-pixel spread, a pixel's own light does not say which side it is
/// on, and the raster's area weight stands.
///
/// MUTATION THIS KILLS: `BAND_STEP_SPREADS` at 0.
#[test]
fn a_band_whose_levels_sit_inside_their_own_spread_keeps_its_area_weight() {
    let (w, h) = (40usize, 16usize);
    let coverage = |x: usize, _: usize| ((x as f32 - 18.0) / 4.0).clamp(0.0, 1.0);
    let read = |spread_codes: f32| {
        let mut data = band_frame(w, h, 20, 0.40, 0.40 + 6.0 / 255.0);
        for (i, p) in data.iter_mut().enumerate() {
            let sign = if (i % w + i / w) % 2 == 0 { 1.0 } else { -1.0 };
            *p = p.map(|c| c + sign * spread_codes / 255.0);
        }
        band_weights(&data, (w, h), 1, w * h, &coverage)
    };
    assert!(read(4.0).is_empty(), "six codes apart under a four-code spread");
    assert!(!read(0.5).is_empty(), "the same six codes under half a code of spread are an edge");
}

/// The spread ramp's waiver: two levels far apart in linear light are read
/// however noisy the frame — there a pixel's own light cannot carry more than
/// twice a zone's own noise into the band, and it is where the area weight
/// draws its rim. Here a chequer of 0.12 over 0.30 against 0.14: the step is
/// well inside twice the spread, the dim level under a third of the bright.
///
/// MUTATION THIS KILLS: the trust taken from the spread ramp alone.
#[test]
fn a_noisy_edge_whose_levels_stand_far_apart_is_still_read() {
    let (w, h) = (40usize, 16usize);
    let coverage = |x: usize, _: usize| ((x as f32 - 18.0) / 4.0).clamp(0.0, 1.0);
    let data = grey_frame(w, h, |x, y| {
        (if x < 20 { 0.30 } else { 0.14 }) + if (x + y) % 2 == 0 { 0.12 } else { -0.12 }
    });
    assert!(!band_weights(&data, (w, h), 1, w * h, &coverage).is_empty());
}

/// A feather is its author's. One that moves less than a twentieth a pixel
/// has no band; one that would take twice the reach to cross from one of its
/// sides to the other — twelve pixels, here — is left as it was drawn, and
/// one of eight across the same edge (off its middle, as a drawn feather's
/// is) is a band.
///
/// MUTATION THIS KILLS: `close` fixed at 1 in `Blocks::sides` (the
/// fourteen-pixel feather's weights then move by 0.30).
#[test]
fn a_feather_is_left_as_it_was_drawn() {
    let (w, h) = (80usize, 16usize);
    let data = band_frame(w, h, 36, 0.5, 0.1);
    let ramp = |width: f32| move |x: usize, _: usize| ((x as f32 - (40.0 - width / 2.0)) / width).clamp(0.0, 1.0);
    let moved = |width: f32| largest_move(&band_weights(&data, (w, h), 1, w * h, &ramp(width)), &ramp(width));
    assert!(band_weights(&data, (w, h), 1, w * h, &ramp(40.0)).is_empty(), "a forty-column feather has no band");
    for width in [20.0, 16.0, 14.0] {
        assert!(moved(width) <= 1e-6, "a {width}-column feather keeps its weights: {}", moved(width));
    }
    assert!(moved(12.0) <= 5e-3, "a twelve-column one keeps its weights: {}", moved(12.0));
    assert!(moved(8.0) > 0.3, "an eight-column step is a band: {}", moved(8.0));
}

/// A pole is a side though every mean round it is moving: the walk down ends
/// on it — nothing stands lower — and the side is the blocks that stand at
/// the pole's own coverage, read on the raster itself. Four pixels wide or
/// two, the half-covered column either side of it — pure sky — is owed all of
/// the sky's layer and the pole none.
///
/// MUTATION THIS KILLS: a side taken as every block round the walk's end,
/// whatever coverage it stands at (the two sides then read alike, and nothing
/// is listed).
#[test]
fn a_thin_pole_is_a_side() {
    let (w, h) = (44usize, 16usize);
    for pole in [20..=23usize, 21..=22] {
        let data = grey_frame(w, h, |x, _| if pole.contains(&x) { 0.1 } else { 0.5 });
        let coverage = |x: usize, _: usize| match x {
            x if pole.contains(&x) => 0.0,
            x if x + 1 == *pole.start() || x == *pole.end() + 1 => 0.5,
            _ => 1.0,
        };
        let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
        assert_eq!(rows.len(), h);
        for row in &rows {
            let band = (*pole.start() as u32 - 3..=*pole.end() as u32 + 3).collect::<Vec<u32>>();
            assert_eq!(listed(row), band, "the pole, its half column and two more either side");
            for (x, lanes) in row {
                let owed = if pole.contains(&(*x as usize)) { 0.0 } else { 1.0 };
                assert!(lanes.iter().all(|v| (v - owed).abs() <= 1e-4), "pole {pole:?}, column {x}: {lanes:?} against {owed}");
            }
        }
    }
}

/// A thin spire's flank walks DOWN THE SPIRE: the raster stands at 0.30 over
/// the spire's top and falls to nothing where it meets the butte, with no
/// flat floor anywhere along it, and the spire's own pixels are owed what the
/// butte's are — none of the sky's layer where the butte is in sight (eight
/// rows up), 0.015 fourteen rows up — while the half-covered column either
/// side of it, pure sky, is owed all of it.
///
/// MUTATION THIS KILLS: `BAND_SIGHT` at 2 (eight rows up, the spire is then
/// read against itself two rows down and keeps 0.07 of the sky's layer).
#[test]
fn a_thin_spire_is_read_against_the_butte_it_rises_from() {
    let (w, h) = (44usize, 40usize);
    let on_spire = |x: usize, y: usize| (21..=22).contains(&x) && (6..28).contains(&y);
    let data = grey_frame(w, h, |x, y| if y >= 28 || on_spire(x, y) { 0.1 } else { 0.5 });
    let coverage = |x: usize, y: usize| {
        let spire = 0.30 * (27 - y.min(27)) as f32 / 21.0;
        let at = match x {
            _ if y >= 28 => 0.0,
            _ if y < 6 => 1.0,
            21 | 22 => spire,
            20 | 23 => 0.5 * (1.0 + spire),
            _ => 1.0,
        };
        if y == 27 { at.min(0.5) } else { at }
    };
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    let lanes_at = |x: u32, y: usize| rows[y].iter().find(|entry| entry.0 == x).unwrap_or_else(|| panic!("column {x} of row {y} is on the band")).1;
    for (y, most) in [(20usize, 1e-3f32), (14, 0.02)] {
        for x in [21, 22] {
            assert!(lanes_at(x, y).iter().all(|v| *v <= most), "row {y}, the spire: {:?} over {most}", lanes_at(x, y));
        }
        for x in [20, 23] {
            assert!(lanes_at(x, y).iter().all(|v| (v - 1.0).abs() <= 1e-3), "row {y}, the sky beside it: {:?}", lanes_at(x, y));
        }
    }
}

/// Where three levels meet, a band block has a THIRD side. A staircase — the
/// land layer at 0.1 over the sky, 0.7 over a far ridge below row 12 and 1.0
/// over the butte right of column 30, each step two pixels wide — walks every
/// band block by the corner down to the sky and up to the butte, the raster's
/// slope leading past the ridge both ways; read between those two alone, the
/// ridge's own pixels by the corner kept 0.27 of the layer for their 0.7.
/// With the ridge heeded as the middle of a chain of three levels, every
/// pixel on the band is owed its own content's coverage: the sky's 0.1, the
/// ridge's 0.7, the butte's 1.0.
///
/// MUTATIONS THIS KILLS: `Blocks::third` answering `None` (the ridge by the
/// corner reads 0.27); `Blocks::walk` stepping by the bare difference, a
/// diagonal step counted as long as a straight one (the butte's column three
/// and four rows below the corner is then walked up into the sky, its band
/// taken for one nine blocks across, and keeps 0.925).
#[test]
fn a_pixel_where_three_levels_meet_is_read_between_the_two_it_stands_between() {
    let (w, h) = (44usize, 30usize);
    let data = grey_frame(w, h, |x, y| match (x, y) {
        (30.., _) => 0.08,
        (_, 12..) => 0.20,
        _ => 0.5,
    });
    let coverage = |x: usize, y: usize| {
        let ridge = 0.1 + 0.6 * ((y as f32 - 11.0) / 2.0).clamp(0.0, 1.0);
        ridge + (1.0 - ridge) * ((x as f32 - 29.0) / 2.0).clamp(0.0, 1.0)
    };
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    for (y, row) in rows.iter().enumerate() {
        for (x, lanes) in row {
            let owed = match (*x, y) {
                (30.., _) => 1.0,
                (_, 12..) => 0.7,
                _ => 0.1,
            };
            assert!(lanes.iter().all(|v| (v - owed).abs() <= 1e-3), "row {y}, column {x}: {lanes:?} against {owed}");
        }
    }
    // The corner is on the band: the ridge under the sky's step, and the butte's column down from it.
    for (x, y) in [(28u32, 12usize), (29, 12), (30, 12), (30, 15), (30, 16), (30, 20)] {
        assert!(listed(&rows[y]).contains(&x), "row {y} lists {:?}", listed(&rows[y]));
    }
}

/// A pixel within its side's own spread IS that side: a land whose pixels
/// chequer between 0.10 and 0.14 beside a sky at 0.5 is the land on its
/// brighter pixels as on its darker, and every one of them under the raster's
/// step is owed all of the land's layer. Placed by its own light alone, a
/// brighter one read as part sky and kept 0.77.
///
/// MUTATION THIS KILLS: the two sides' own edges left at their levels in
/// `Blocks::sides`.
#[test]
fn a_pixel_within_its_sides_own_spread_is_that_side() {
    let (w, h) = (40usize, 16usize);
    let data = grey_frame(w, h, |x, y| match x {
        x if x < 20 => 0.5,
        x if (x + y) % 2 == 0 => 0.14,
        _ => 0.10,
    });
    let coverage = |x: usize, _: usize| ((x as f32 - 18.0) / 4.0).clamp(0.0, 1.0);
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    for row in &rows {
        assert!((18..=22).all(|x| listed(row).contains(&x)), "{:?}", listed(row));
        for (x, lanes) in row {
            let owed = if *x < 20 { 0.0 } else { 1.0 };
            assert!(lanes.iter().all(|v| (v - owed).abs() <= 1e-3), "column {x}: {lanes:?} against {owed}");
        }
    }
}

/// The band is read on the raster's 3 x 3 mean, which a noisy raster does not
/// set moving: a plateau that chequers four hundredths either side of 0.5 —
/// each pixel eight hundredths from its neighbour — is no band, and only the
/// step at its end is.
///
/// MUTATION THIS KILLS: the band read on the raster itself (the plateau's
/// own pixels are then listed, two columns short of the step).
#[test]
fn a_noisy_plateau_sets_no_band() {
    let (w, h) = (40usize, 16usize);
    let data = grey_frame(w, h, |x, _| if x < 20 { 0.5 } else { 0.1 });
    let coverage = |x: usize, y: usize| match x {
        x if x < 20 => if (x + y).is_multiple_of(2) { 0.54 } else { 0.46 },
        20 => 0.25,
        _ => 0.0,
    };
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    for row in &rows {
        assert_eq!(listed(row), (18..=22).collect::<Vec<u32>>());
    }
}

/// Both sides must be IN SIGHT: a side of fewer pixels than one line of the
/// window it is read in is trusted by its share of one. A square three pixels
/// on a side is nine — nine thirteenths — so the column of it that is the
/// other content keeps four thirteenths of the square's weight, and the
/// column outside it that is the square's content takes nine.
///
/// MUTATIONS THIS KILLS: `seen` fixed at 1; `MASK_BAND_REACH` at 30 (a line
/// is then the frame's 24 rows).
#[test]
fn a_side_seen_in_a_few_pixels_is_trusted_by_its_share_of_a_line() {
    let (w, h) = (44usize, 24usize);
    let coverage = |x: usize, y: usize| if (20..=22).contains(&x) && (8..=10).contains(&y) { 1.0 } else { 0.0 };
    // The content's square stands one column to the right of the raster's.
    let data = grey_frame(w, h, |x, y| if (21..=23).contains(&x) && (8..=10).contains(&y) { 0.5 } else { 0.1 });
    let rows = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert_eq!(rows.len(), h);
    let lanes_at = |x: u32| rows[9].iter().find(|entry| entry.0 == x).unwrap_or_else(|| panic!("column {x} is on the band")).1;
    let seen = 9.0 / 13.0;
    assert!(lanes_at(20).iter().all(|v| (v - (1.0 - seen)).abs() <= 1e-3), "{:?}", lanes_at(20));
    assert!(lanes_at(23).iter().all(|v| (v - seen).abs() <= 1e-3), "{:?}", lanes_at(23));
    assert!(lanes_at(21).iter().all(|v| (v - 1.0).abs() <= 1e-4), "the square's content inside the square: {:?}", lanes_at(21));
}

/// The block a layer's band is read in is one pixel of its raster, and never
/// less than one of the frame.
///
/// MUTATION THIS KILLS: `block_edge` answering 1 whatever the raster.
#[test]
fn a_layers_block_is_one_pixel_of_its_own_raster() {
    assert_eq!(mask_owed::block_edge(&image::GrayImage::new(24, 10), 96, 40), 4);
    assert_eq!(mask_owed::block_edge(&image::GrayImage::new(2048, 1365), 9504, 6336), 5, "a 61 MP export over a zone raster");
    assert_eq!(mask_owed::block_edge(&image::GrayImage::new(2048, 1365), 1000, 667), 1, "a raster finer than the frame");
    assert_eq!(mask_owed::block_edge(&image::GrayImage::new(96, 40), 96, 40), 1);
}

/// A raster coarser than the frame: its step is many frame pixels wide over
/// an edge one pixel wide, and it is read in blocks of the raster's own
/// pixels — three of them here, a band. Read in frame pixels (the second
/// half) the same twelve columns are a feather.
///
/// MUTATION THIS KILLS: `Sides::owed` handing back the area share (with
/// `a_band_pixel_is_owed_its_contents_share_of_its_light`).
#[test]
fn a_coarse_rasters_band_is_read_in_the_rasters_own_pixels() {
    let (w, h) = (96usize, 40usize);
    let data = band_frame(w, h, 48, 0.5, 0.1);
    // A raster a quarter of the frame's width: a three-pixel step of its own
    // is twelve frame columns, 42..54.
    let coverage = |x: usize, _: usize| ((x as f32 - 42.0) / 12.0).clamp(0.0, 1.0);
    let want = light_fraction(0.5, srgb_to_linear(0.1), srgb_to_linear(0.5));
    let coarse = band_weights(&data, (w, h), 4, w * h, &coverage);
    assert_eq!(coarse.len(), h);
    for row in &coarse {
        assert_eq!(listed(row), (36..=59).collect::<Vec<u32>>(), "the step's blocks and the block either side");
        for (x, lanes) in row {
            let owed = match x.cmp(&48) {
                std::cmp::Ordering::Less => 0.0,
                std::cmp::Ordering::Equal => want,
                std::cmp::Ordering::Greater => 1.0,
            };
            assert!(lanes.iter().all(|v| (v - owed).abs() <= 5e-3), "column {x}: {lanes:?} against {owed}");
        }
    }
    let fine = band_weights(&data, (w, h), 1, w * h, &coverage);
    assert!(largest_move(&fine, &coverage) <= 5e-3, "read in frame pixels the same columns are a feather");
}

/// The list's share of the frame: a raster that is band over more of the
/// frame than an edge can be keeps its area weight, whole.
///
/// MUTATION THIS KILLS: dropping the `listed > budget` return.
#[test]
fn a_raster_that_is_band_over_too_much_of_the_frame_keeps_its_area_weight() {
    let (w, h) = (48usize, 24usize);
    // Columns in eights — three outside, a step, three inside, a step — over
    // content that follows them: nearly all of the frame is a readable band.
    let data = grey_frame(w, h, |x, _| [0.2, 0.2, 0.2, 0.5, 0.8, 0.8, 0.8, 0.5][x % 8]);
    let coverage = |x: usize, _: usize| [0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 0.5][x % 8];
    let count = |budget: usize| band_weights(&data, (w, h), 1, budget, &coverage).iter().map(Vec::len).sum::<usize>();
    assert!(count(w * h) > w * h / 2, "premise: most of the frame is a band that can be read: {}", count(w * h));
    assert_eq!(count(w * h / 4), 0, "past its share of the frame the layer is left alone");
}

/// One composition for the coverage and for a band pixel's shares: a lane
/// handed every layer's own weight is the plain combined coverage to the bit,
/// and a share in a layer's place composes exactly as the coverage it stands
/// for (every mode is linear in each layer).
#[test]
fn the_layer_fold_carries_four_lanes_as_it_carries_one() {
    use crate::recipe::{MaskCombine, MaskComponent, MaskGeometry};
    let linear = |zero_x: f32, full_x: f32| MaskGeometry::Linear { zero_x, zero_y: 0.5, full_x, full_y: 0.5 };
    for mode in [MaskCombine::Add, MaskCombine::Subtract, MaskCombine::Intersect] {
        for inverted in [false, true] {
            let m = LocalAdjustment {
                mask: linear(0.2, 0.8),
                components: vec![MaskComponent { geometry: linear(0.9, 0.1), mode, inverted }],
                ..Default::default()
            };
            let layers = Layers { m: &m, base: None, comps: &[None], unwarp: None, dims: (1.0, 1.0) };
            for nx in [0.15f32, 0.35, 0.5, 0.65, 0.85] {
                let plain = combined_mask_weight(&m, nx, 0.5, None, &[None], None, (1.0, 1.0));
                assert!(layers.fold(nx, 0.5, |_, w| [w; 4]).iter().all(|v| v.to_bits() == plain.to_bits()));
                let (base, comp) = (layers.one(0, nx, 0.5), layers.one(1, nx, 0.5));
                let comp = if inverted { 1.0 - comp } else { comp };
                let shares = [0.0, 0.25, 1.0, base];
                let got = layers.fold(nx, 0.5, |layer, w| if layer == 0 { shares } else { [w; 4] });
                for (got, share) in got.iter().zip(shares) {
                    let want = match mode {
                        MaskCombine::Add => 1.0 - (1.0 - share) * (1.0 - comp),
                        MaskCombine::Subtract => share * (1.0 - comp),
                        MaskCombine::Intersect => share * comp,
                    };
                    assert_eq!(got.to_bits(), want.to_bits(), "{mode:?} inverted {inverted} at {nx}");
                }
            }
        }
    }
}

/// THE RIM, through the develop: a dim land lifted 2.5 EV beside a bright sky
/// left alone, the land's raster stepping over five columns across a one-column
/// edge. By the area weight the pure sky pixel under 30 % of the step took 30 %
/// of the lift and stood some thirty codes over the sky. Drawn three ways — the
/// land's own raster, the sky's raster inverted, and the land's raster inside
/// a gradient down the frame (a zone the fit split in bands, whose combined
/// coverage is settled nowhere along the silhouette): every row falls from the
/// sky to its land without rising, the pure sky pixels are the unmasked
/// render's, and the pure land pixels are their row's settled land.
///
/// MUTATIONS THIS KILLS: `Cover::weight_of` ignoring its entry (the area
/// weight); `BandShares::read` reading the combined coverage in a layer's
/// place (the gradient's rows keep their rim).
#[test]
fn lifting_the_dim_side_of_a_silhouette_draws_no_rim_on_the_bright_side() {
    use crate::recipe::{MaskCombine, MaskComponent, MaskGeometry};
    std::fs::create_dir_all("out").ok();
    let (w, h, edge) = (60u32, 24u32, 30u32);
    let ramp = |x: u32| ((x as f32 - 27.5) / 5.0).clamp(0.0, 1.0);
    let (land_p, sky_p) = ("out/_band_land.png", "out/_band_sky.png");
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([(ramp(x) * 255.0).round() as u8])).save(land_p).unwrap();
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([255 - (ramp(x) * 255.0).round() as u8])).save(sky_p).unwrap();
    let base = band_scene(w, h, edge, 150, 20);
    let control = develop_preview(&base, &EditRecipe::default()).to_rgb8();
    let lift = |mask: LocalAdjustment| {
        let recipe = EditRecipe { masks: vec![LocalAdjustment { exposure_ev: 2.5, ..mask }], ..Default::default() };
        develop_preview(&base, &recipe).to_rgb8()
    };
    let land = MaskGeometry::Bitmap { path: land_p.into() };
    let down = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 };
    let drawn = [
        ("the land's raster", lift(LocalAdjustment { mask: land.clone(), ..Default::default() })),
        (
            "the sky's raster inverted",
            lift(LocalAdjustment { mask: MaskGeometry::Bitmap { path: sky_p.into() }, inverted: true, ..Default::default() }),
        ),
        (
            "the land's raster inside a gradient",
            lift(LocalAdjustment {
                mask: land,
                components: vec![MaskComponent { geometry: down, mode: MaskCombine::Intersect, inverted: false }],
                ..Default::default()
            }),
        ),
    ];
    for (name, out) in &drawn {
        let at = |x: u32, y: u32| out.get_pixel(x, y)[0] as i32;
        let mut lifted_rows = 0;
        for y in 0..h {
            let (sky, settled) = (at(10, y), at(50, y));
            assert_eq!(sky, control.get_pixel(10, y)[0] as i32, "{name}, row {y}: the sky is outside the mask");
            lifted_rows += i32::from(settled > control.get_pixel(50, y)[0] as i32 + 20);
            for x in 26..=34 {
                assert!(at(x, y) <= sky.max(settled) + 1, "{name}, row {y}: column {x} stands over both sides: {} against {sky} / {settled}", at(x, y));
                assert!(at(x + 1, y) <= at(x, y) + 1, "{name}, row {y}: the row rises again at column {}", x + 1);
            }
            for x in [28, 29] {
                assert_eq!(at(x, y), sky, "{name}, row {y}: column {x} is pure sky under the raster's step");
            }
            // Within a code: a side is the blocks that stand at its own coverage, and the step's
            // last column, a tenth short of the land's, counts for a little of it (0.9994 as read).
            for x in [31, 32] {
                assert!((at(x, y) - settled).abs() <= 1, "{name}, row {y}: column {x} is pure land under the raster's step: {} against {settled}", at(x, y));
            }
        }
        assert!(lifted_rows >= 6, "premise, {name}: the land is lifted on {lifted_rows} rows");
    }
}

/// THE RIM AND THE PATCH, through the develop, on a raster shaped as the
/// fit's zone rasters are: the land's layer at 31 / 255 over the sky and
/// 204 / 255 over the land, a step of five columns between. The sky carries a
/// little of the land's lift all the way to the silhouette — the raster's own
/// plateau — and every pure sky pixel carries the SAME: the ones under the
/// step no more (the rim the area weight drew) and no less (the dark patch
/// the first form of this law drew by taking them to no lift at all beside
/// neighbours left at the plateau's). The land likewise.
///
/// MUTATIONS THIS KILLS: the owed weight taken between 0 and 1 in the two
/// sides' own coverages' place (the pixels under the step lose the plateau's
/// lift); `Cover::weight_of` ignoring its entry (they gain the step's).
#[test]
fn a_silhouette_between_two_plateaus_shows_neither_a_rim_nor_a_patch() {
    use crate::recipe::MaskGeometry;
    std::fs::create_dir_all("out").ok();
    let (w, h, edge) = (60u32, 24u32, 30u32);
    let ramp = |x: u32| ((x as f32 - 27.5) / 5.0).clamp(0.0, 1.0);
    let path = "out/_band_plateaus.png";
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([31 + (173.0 * ramp(x)).round() as u8])).save(path).unwrap();
    let base = band_scene(w, h, edge, 150, 20);
    let control = develop_preview(&base, &EditRecipe::default()).to_rgb8();
    let recipe = EditRecipe {
        masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: path.into() }, exposure_ev: 2.5, ..Default::default() }],
        ..Default::default()
    };
    let out = develop_preview(&base, &recipe).to_rgb8();
    let at = |x: u32, y: u32| out.get_pixel(x, y)[0] as i32;
    for y in 0..h {
        let (sky, land) = (at(10, y), at(50, y));
        assert!(sky > control.get_pixel(10, y)[0] as i32 + 3, "premise, row {y}: the sky's plateau carries some of the lift: {sky}");
        assert!(land > control.get_pixel(50, y)[0] as i32 + 20, "premise, row {y}: the land is lifted: {land}");
        for x in 22..=29 {
            assert_eq!(at(x, y), sky, "row {y}: column {x} is pure sky and reads as the sky beside it does");
        }
        for x in 31..=38 {
            assert_eq!(at(x, y), land, "row {y}: column {x} is pure land and reads as the land beside it does");
        }
        assert!(at(30, y) <= sky.max(land) + 1, "row {y}: the mixed pixel stands over both sides: {}", at(30, y));
    }
}
