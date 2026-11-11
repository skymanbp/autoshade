// One part of the engine's tests (src/render/tests.rs includes it): quarter turns of pixels, points, recipe coordinates and rasters, and the shift of every carrier.

#[test]
fn orient_f32_matches_the_display_orientation_semantics() {
    // A 3×2 buffer whose pixels carry their own index: every one of the
    // EIGHT states must produce the exact hand-derived EXIF mapping
    // (Rotate90 = clockwise, so top-left → top-right; Transpose =
    // main-diagonal mirror; Transverse = anti-diagonal). Only Normal and
    // Rotate90 were pinned before — the other six arms of `oriented`,
    // including the two in-place A7 compositions, had no coverage, and
    // orient_f32 round-trips through `oriented`, so the table below is
    // derived from the EXIF definitions, NOT from the image crate (a
    // crate-op reference would compare the function against itself)
    // (U14).
    let src: Vec<[f32; 3]> = (0..6).map(|i| [i as f32, 0.0, 0.0]).collect();
    let cases: [(Orientation, (usize, usize), [usize; 6]); 8] = [
        (Orientation::Normal, (3, 2), [0, 1, 2, 3, 4, 5]),
        (Orientation::HorizontalFlip, (3, 2), [2, 1, 0, 5, 4, 3]),
        (Orientation::Rotate180, (3, 2), [5, 4, 3, 2, 1, 0]),
        (Orientation::VerticalFlip, (3, 2), [3, 4, 5, 0, 1, 2]),
        (Orientation::Rotate90, (2, 3), [3, 0, 4, 1, 5, 2]),
        (Orientation::Rotate270, (2, 3), [2, 5, 1, 4, 0, 3]),
        (Orientation::Transpose, (2, 3), [0, 3, 1, 4, 2, 5]),
        (Orientation::Transverse, (2, 3), [5, 2, 4, 1, 3, 0]),
    ];
    for (o, dims, map) in cases {
        let (out, w, h) = orient_f32(src.clone(), 3, 2, o);
        assert_eq!((w, h), dims, "{o:?} dims");
        let got: Vec<usize> = out.iter().map(|p| p[0] as usize).collect();
        assert_eq!(&got[..], &map[..], "{o:?} pixel mapping");
    }
}

/// THE SEAM the whole coordinate migration rests on: `orient_point` must
/// be the exact coordinate twin of `oriented`'s PIXEL transform, for all
/// eight states. Derived from the pixel map above (which is itself derived
/// from the EXIF definitions, never from the image crate), so a future
/// edit to either function that drifts from the other fails HERE rather
/// than silently displacing every saved mask.
#[test]
fn orient_point_is_the_coordinate_twin_of_the_pixel_transform() {
    const W: u32 = 5;
    const H: u32 = 3;
    let src = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(W, H, |x, y| {
        image::Rgb([x as u8, y as u8, 0])
    }));
    for o in [
        Orientation::Normal,
        Orientation::HorizontalFlip,
        Orientation::Rotate180,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ] {
        let dst = oriented(src.clone(), o).to_rgb8();
        let (dw, dh) = (dst.width(), dst.height());
        for y in 0..H {
            for x in 0..W {
                // Pixel CENTRES: the only points whose normalised image is
                // unambiguous under a bin edge.
                let (u, v) =
                    orient_point(o, (x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32);
                let (dx, dy) = ((u * dw as f32) as u32, (v * dh as f32) as u32);
                let px = dst.get_pixel(dx.min(dw - 1), dy.min(dh - 1));
                assert_eq!(
                    (px[0] as u32, px[1] as u32),
                    (x, y),
                    "{o:?}: source ({x},{y}) should land at ({dx},{dy})"
                );
            }
        }
    }
}

/// The nine `Orientation` values under `to_flips`/`from_flips` and the nine
/// under [`orient_point`] are the SAME group, so [`compose_orientation`]'s
/// bit algebra and the geometry it claims to describe cannot drift apart.
///
/// Checked exhaustively: for every (EXIF state, quarter turn) pair and
/// four probe points, `orient_point(compose(e, k), p)` equals
/// `orient_point(R90^k, orient_point(e, p))` — composition IS "apply the
/// EXIF state, then turn the display frame", which is the order the pixels
/// take. The probe set includes an off-frame point, because mask gradients
/// legitimately live outside the unit square.
///
/// MUTATION THIS CATCHES: swap the two arms of `compose_two`'s
/// `if t1 { … } else { … }` (i.e. cross the flips on the wrong side) and
/// every transposing EXIF state composes to its mirror — the 「竖图横躺」
/// failure, one composition step upstream of where it used to live.
#[test]
fn compose_orientation_is_the_composition_of_the_two_coordinate_maps() {
    const STATES: [Orientation; 9] = [
        Orientation::Normal,
        Orientation::HorizontalFlip,
        Orientation::Rotate180,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
        Orientation::Unknown,
    ];
    for e in STATES {
        for k in 0u8..4 {
            let composed = compose_orientation(e, k);
            // The group is CLOSED: nine values in, never `Unknown` out
            // (it is Normal's twin, and a composition that produced it
            // would make `to_u16` write 0 into a sidecar one day).
            assert_ne!(composed, Orientation::Unknown, "{e:?} + {k} quarter turns");
            for (u, v) in [(0.0f32, 0.0f32), (0.13, 0.87), (1.0, 0.25), (-0.4, 1.6)] {
                let (a, b) = orient_point(e, u, v);
                let want = orient_point(quarter_turn_orientation(k), a, b);
                let got = orient_point(composed, u, v);
                assert!(
                    (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
                    "{e:?} then {k} quarter turns = {composed:?}: ({u},{v}) → {got:?}, \
                     but applying the two in order gives {want:?}"
                );
            }
        }
    }
    // …and the identity/period facts the field's 0..3 domain rests on.
    assert_eq!(compose_orientation(Orientation::Rotate90, 0), Orientation::Rotate90);
    assert_eq!(compose_orientation(Orientation::Rotate90, 2), Orientation::Rotate270);
    assert_eq!(compose_orientation(Orientation::Rotate270, 1), Orientation::Normal);
    assert_eq!(compose_orientation(Orientation::Normal, 4), Orientation::Normal);
}

/// [`quarter_turns_between`] IS [`compose_orientation`]'s inverse, over the
/// whole 8 x 8 table of (capture EXIF state, sidecar `tiff:Orientation`)
/// pairs — the sidecar orientation law, checked rather than argued.
///
/// Three properties, exhaustively:
///   * every answer it gives is right: `compose(exif, k) == want`;
///   * it answers exactly when the two states have the same HANDEDNESS,
///     which is the index-two rotation subgroup fact the doc comment rests
///     on — so a refusal is a real impossibility, not a search that gave up;
///   * the me6-2026-09 pack's own row: EXIF 8 over a sidecar declaring 1 is
///     one clockwise quarter turn, which is what makes the delivered frame
///     6240 x 4160 instead of 4160 x 6240.
///
/// MUTATION THIS CATCHES: search `0u8..3` instead of `0u8..4` and the 16
/// pairs needing three quarter turns come back `None` — a refused rotation
/// reported as a mirror.
#[test]
fn quarter_turns_between_is_the_inverse_of_compose_orientation() {
    const STATES: [Orientation; 8] = [
        Orientation::Normal,
        Orientation::HorizontalFlip,
        Orientation::Rotate180,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ];
    let mut solved = 0;
    for exif in STATES {
        for want in STATES {
            let reachable = orientation_mirrors(exif) == orientation_mirrors(want);
            match quarter_turns_between(exif, want) {
                Some(k) => {
                    assert!(k < 4, "{exif:?} -> {want:?}: {k} is not a quarter turn");
                    assert_eq!(
                        compose_orientation(exif, k),
                        want,
                        "{exif:?} + {k} quarter turns is not {want:?}"
                    );
                    assert!(reachable, "{exif:?} -> {want:?} crosses a mirror and cannot be a turn");
                    solved += 1;
                }
                None => assert!(
                    !reachable,
                    "{exif:?} -> {want:?} keeps its handedness, so some quarter turn reaches it"
                ),
            }
        }
    }
    // Half the table is reachable, which IS the index-two statement.
    assert_eq!(solved, 32, "the rotation subgroup has index two in the dihedral group");
    // `Unknown` is `Normal`'s twin on both sides, never a refusal.
    assert_eq!(quarter_turns_between(Orientation::Unknown, Orientation::Normal), Some(0));
    assert_eq!(quarter_turns_between(Orientation::Normal, Orientation::Unknown), Some(0));
    // The pack: IFD0 Orientation = 8 under a sidecar that declares 1.
    assert_eq!(
        quarter_turns_between(Orientation::Rotate270, Orientation::Normal),
        Some(1),
        "the me6-2026-09 pack's own row"
    );
}

/// The 竖图横躺 regression net (R27 A10), stated as the property that
/// actually matters: the composed orientation TRANSPOSES exactly when the
/// rendered frame does.
///
/// Pure code — the pixel side is [`oriented`] on a deliberately
/// non-square frame, which is the same function `orient_f32` round-trips
/// through, so no RAW is needed to pin the chain. The real-file arm lives
/// in `decode::portrait_raw_reaches_the_pipeline_as_rotate270` and the RAW
/// zoo probe.
///
/// MUTATION THIS CATCHES: drop `Rotate270` from `decode`'s
/// `orientation_transposes` list (or add `Rotate180` to it) and the
/// declared dims part company with the pixels for half the states — the
/// exact shape of the v0.30 root fix, now with the user's turn on top.
#[test]
fn a_quarter_turn_on_any_exif_state_transposes_the_dims_iff_it_transposes_the_pixels() {
    const STATES: [Orientation; 8] = [
        Orientation::Normal,
        Orientation::HorizontalFlip,
        Orientation::Rotate180,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ];
    // 7×5: both dims distinct AND distinct from each other's, so a
    // transpose is visible and a square frame cannot hide a bug.
    let src = DynamicImage::ImageRgb8(RgbImage::new(7, 5));
    for e in STATES {
        for k in 0u8..4 {
            let composed = compose_orientation(e, k);
            let (w, h) = oriented(src.clone(), composed).dimensions();
            let transposed = (w, h) == (5, 7);
            assert!(
                transposed || (w, h) == (7, 5),
                "{e:?} + {k}: a rotation/flip produced {w}×{h} from 7×5"
            );
            assert_eq!(
                transposed,
                crate::decode::orientation_transposes(composed),
                "{e:?} + {k} = {composed:?}: pixels {w}×{h} but the dims predicate disagrees"
            );
            // The user's turn applied to the ALREADY-oriented pixels must
            // give the same frame as the composed one — the property that
            // lets `render_to_image_in` keep exactly one orientation stage.
            let two_step = turn_image(oriented(src.clone(), e), k);
            assert_eq!(
                two_step.dimensions(),
                (w, h),
                "{e:?} + {k}: one composed turn and two sequential turns disagree"
            );
        }
    }
}

/// Every state is a BIJECTION of the plane, and the migration is therefore
/// reversible — the property that lets an era-0 recipe be turned exactly
/// once with no accumulated drift.
#[test]
fn orient_point_round_trips_through_its_inverse() {
    // Six of the eight are involutions; the quarter turns are each
    // other's inverse.
    let pairs = [
        (Orientation::Normal, Orientation::Normal),
        (Orientation::Unknown, Orientation::Unknown),
        (Orientation::HorizontalFlip, Orientation::HorizontalFlip),
        (Orientation::VerticalFlip, Orientation::VerticalFlip),
        (Orientation::Rotate180, Orientation::Rotate180),
        (Orientation::Transpose, Orientation::Transpose),
        (Orientation::Transverse, Orientation::Transverse),
        (Orientation::Rotate90, Orientation::Rotate270),
        (Orientation::Rotate270, Orientation::Rotate90),
    ];
    for (o, inv) in pairs {
        // Off-frame points included: mask gradients legitimately live
        // outside [0,1] and must survive the round trip too.
        for (u, v) in [(0.0f32, 0.0f32), (0.13, 0.87), (1.0, 0.0), (-0.4, 1.6)] {
            let (a, b) = orient_point(o, u, v);
            let back = orient_point(inv, a, b);
            assert!(
                (back.0 - u).abs() < 1e-6 && (back.1 - v).abs() < 1e-6,
                "{o:?} then {inv:?} moved ({u},{v}) to {back:?}"
            );
        }
    }
}

/// The A7R IV's own frame — `DefaultCropSize = (9504, 6336)`, aspect
/// exactly 1.5 — as `orient_recipe_coords`' third argument.
///
/// A round number on purpose: `1.5` and its reciprocal are exact in binary,
/// so a radius that survives a four-turn circle in these tests survives it
/// because the algebra is right, not because the aspect happened to cancel
/// its own rounding.
fn probe_frame() -> Option<CoordFrame> {
    CoordFrame::new(9504.0, 6336.0)
}

/// The `angle` half of the radial rule, checked against `mask_weight`
/// itself rather than against the derivation: a turned ELLIPSE must cover
/// exactly the turned PIXELS. This is what makes "rotate the two corners,
/// negate the angle only for mirrors" more than an assertion.
#[test]
fn rotated_radial_mask_covers_the_rotated_pixels() {
    use crate::recipe::LocalAdjustment;
    let base = MaskGeometry::Radial {
        top: 0.15,
        left: 0.30,
        bottom: 0.55,
        right: 0.90,
        feather: 0.4,
        roundness: 0.0,
        flipped: false,
        angle: 37.0,
        midpoint: 50.0,
        mask_version: 2,
    };
    for o in [
        Orientation::HorizontalFlip,
        Orientation::Rotate180,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ] {
        let mut r = EditRecipe {
            masks: vec![LocalAdjustment { mask: base.clone(), ..Default::default() }],
            ..Default::default()
        };
        assert!(orient_recipe_coords(&mut r, o, probe_frame()));
        let turned = &r.masks[0].mask;
        for i in 0..=20 {
            for j in 0..=20 {
                let (u, v) = (i as f32 / 20.0, j as f32 / 20.0);
                let (u2, v2) = orient_point(o, u, v);
                let before = mask_weight(&base, u, v, None);
                let after = mask_weight(turned, u2, v2, None);
                assert!(
                    (before - after).abs() < 1e-4,
                    "{o:?}: weight at ({u},{v}) was {before}, at the turned point ({u2},{v2}) it is {after}"
                );
            }
        }
    }
}

/// The recipe-level migration: crop and every parametric geometry move,
/// the round trip is exact, and a Normal photo is untouched.
/// R33 §G. The field is a RECIPE control now, so the engine has to render
/// it — after the masks, on the frame the masks produced, and only when it
/// is renderable at all.
///
/// The "after the masks" half is the one worth a test: the field's guide
/// is the render's own smoothed luma, so applying it before a mask that
/// moves luma would read a different bin out of the grid's eight and
/// deliver a different correction. The assertion is therefore not "the
/// pixels moved" but "they moved by exactly what `apply_colour_field` does
/// to the MASKED render" — within the one code of slack a second u8
/// quantisation costs.
#[test]
fn the_engine_renders_the_colour_field_after_the_masks_it_reads() {
    use crate::fit::pixels_of;
    use crate::recipe::{ColourField, LocalAdjustment, MaskGeometry};
    let (w, h) = (96u32, 64u32);
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        let v = 0.15 + 0.7 * (x as f32 / (w - 1) as f32) + 0.1 * (y as f32 / (h - 1) as f32);
        let c = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        image::Rgb([c, c, (c as f32 * 0.92) as u8])
    }));
    // A field with a real, spatially varying demand: warmer on the left,
    // cooler on the right, and a different EV per luma bin.
    let (fx, fy, fb) = (4usize, 3usize, 4usize);
    let grid: Vec<[f32; 5]> = (0..fx * fy * fb)
        .map(|v| {
            let (cell, bin) = (v / fb, v % fb);
            let x = (cell % fx) as f32 / (fx - 1) as f32;
            [0.10 * (bin as f32 / (fb - 1) as f32) - 0.05, 0.06 * x, 0.0, -0.06 * x, 0.02]
        })
        .collect();
    let field = ColourField { x: fx, y: fy, b: fb, grid, owners: Vec::new(), amount: 1.0, enabled: true };

    let with_mask = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 1.0, full_y: 0.0 },
            exposure_ev: -0.9,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut both = with_mask.clone();
    both.colour_field = Some(field.clone());

    let masked = pixels_of(&develop_preview(&img, &with_mask));
    let mut expected = masked.clone();
    apply_colour_field(&mut expected, w as usize, h as usize, Some(&field), &crate::render::FieldFrame::default(), None);
    let actual = pixels_of(&develop_preview(&img, &both));
    assert_ne!(actual, masked, "premise: this field is not the identity here");
    let worst = actual
        .iter()
        .zip(&expected)
        .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
        .fold(0.0f32, f32::max);
    assert!(
        worst <= 1.5 / 255.0,
        "the engine's field must be apply_colour_field over the MASKED render; worst {}/255",
        worst * 255.0
    );

    // …and every way of saying "not now" leaves the frame exactly as the
    // masks left it. A mis-shaped grid is in this list on purpose: it is
    // not a field to be guessed at.
    for parked in [
        ColourField { enabled: false, ..field.clone() },
        ColourField { amount: 0.0, ..field.clone() },
        ColourField { x: fx + 1, ..field.clone() },
        ColourField { grid: Vec::new(), ..field.clone() },
    ] {
        let mut off = with_mask.clone();
        off.colour_field = Some(parked);
        assert_eq!(
            pixels_of(&develop_preview(&img, &off)),
            masked,
            "a parked or mis-shaped field must leave the frame untouched"
        );
    }
}

/// v1.18.0. The ENGINE reads a field's zone owners on the recipe's sky zone
/// mask geometry: a field whose sky-owned vertex row carries a lift and whose
/// land-owned rows carry nothing leaves the land under the zone edge exactly
/// as the masks left it, while the sky above it lifts; the same field with no
/// zone mask to read has no partition and reads plain — the land lifts.
#[test]
fn the_engine_reads_a_zone_owned_field_on_the_sky_zones_geometry() {
    use crate::fit::pixels_of;
    use crate::recipe::{ColourField, LocalAdjustment, MaskGeometry, MaskRole};
    let (w, h) = (96u32, 64u32);
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([90, 90, 90])));
    // Three vertex rows at pixel rows 0, 31.5 and 63; the zone edge at row 40,
    // so the middle row's population is mostly sky and the land rows 40..47
    // read it.
    let (fx, fy, fb) = (2usize, 3usize, 2usize);
    let lifted = |v: usize| v / fb / fx < 2;
    let grid: Vec<[f32; 5]> = (0..fx * fy * fb).map(|v| if lifted(v) { [0.5, 0.0, 0.0, 0.0, 0.0] } else { [0.0; 5] }).collect();
    let owners = (0..fx * fy * fb)
        .map(|v| if lifted(v) { ColourField::OWNER_SKY } else { ColourField::OWNER_LAND })
        .collect();
    let field = ColourField { x: fx, y: fy, b: fb, grid, owners, amount: 1.0, enabled: true };
    let edge = 40.0 / h as f32;
    let sky_zone = LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: edge + 0.001, full_x: 0.5, full_y: edge - 0.001 },
        role: MaskRole::ZoneSky,
        ..Default::default()
    };
    let zoned = EditRecipe { masks: vec![sky_zone.clone()], colour_field: Some(field.clone()), ..Default::default() };
    let before = pixels_of(&develop_preview(&img, &EditRecipe { masks: vec![sky_zone], ..Default::default() }));
    let after = pixels_of(&develop_preview(&img, &zoned));
    let row = |px: &[[f32; 3]], y: u32| px[(y * w + w / 2) as usize][1];
    for y in 40..48 {
        assert_eq!(row(&after, y), row(&before, y), "land row {y} reads only land-owned vertices");
    }
    assert!(row(&after, 36) > row(&before, 36), "the sky row above the edge keeps its lift");
    let plain = pixels_of(&develop_preview(&img, &EditRecipe { colour_field: Some(field), ..Default::default() }));
    assert!(row(&plain, 40) > row(&before, 40), "no zone mask, no partition: the land reads the lift");
}

/// v1.18.0. A zone with no controls of its own is still the zone: a land zone
/// on a raster (the inverted sky) that moves no pixel — the develop's raster
/// snapshot holds only masks that act — still partitions the frame for the
/// field's zone-owned read, so the land under the edge reads no sky lift.
#[test]
fn a_zone_that_moves_no_pixel_still_partitions_the_fields_read() {
    use crate::fit::pixels_of;
    use crate::recipe::{ColourField, LocalAdjustment, MaskGeometry, MaskRole};
    let (w, h) = (96u32, 64u32);
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([90, 90, 90])));
    let dir = std::env::temp_dir().join(format!("autoshade-field-partition-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sky.png");
    image::GrayImage::from_fn(w, h, |_, y| image::Luma([if y < 40 { 255 } else { 0 }])).save(&path).unwrap();
    let (fx, fy, fb) = (2usize, 3usize, 2usize);
    let lifted = |v: usize| v / fb / fx < 2;
    let grid: Vec<[f32; 5]> = (0..fx * fy * fb).map(|v| if lifted(v) { [0.5, 0.0, 0.0, 0.0, 0.0] } else { [0.0; 5] }).collect();
    let owners = (0..fx * fy * fb)
        .map(|v| if lifted(v) { ColourField::OWNER_SKY } else { ColourField::OWNER_LAND })
        .collect();
    let field = ColourField { x: fx, y: fy, b: fb, grid, owners, amount: 1.0, enabled: true };
    let land_zone = LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() },
        role: MaskRole::ZoneLand,
        inverted: true,
        ..Default::default()
    };
    assert!(!crate::render::engine_active(&land_zone), "premise: the zone moves no pixel");
    let before = pixels_of(&develop_preview(&img, &EditRecipe::default()));
    let after = pixels_of(&develop_preview(
        &img,
        &EditRecipe { masks: vec![land_zone], colour_field: Some(field), ..Default::default() },
    ));
    let row = |px: &[[f32; 3]], y: u32| px[(y * w + w / 2) as usize][1];
    for y in 40..48 {
        assert_eq!(row(&after, y), row(&before, y), "land row {y} reads only land-owned vertices");
    }
    assert!(row(&after, 36) > row(&before, 36), "the sky row above the edge keeps its lift");
    let _ = std::fs::remove_dir_all(&dir);
}

/// R33 §G. A quarter turn moves the field's cells with every other
/// coordinate in the recipe, and swaps its two axis LENGTHS with them.
///
/// Round-tripped through the inverse orientation, because a permutation
/// that is off by one cell still looks plausible in isolation: only going
/// back proves each cell landed where exactly one cell came from.
#[test]
fn a_quarter_turn_transposes_the_colour_field_and_comes_back() {
    use crate::recipe::ColourField;
    let (fx, fy, fb) = (4usize, 3usize, 2usize);
    let seed = |i: usize| [i as f32, 0.0, 0.0, 0.0, 0.0];
    // The zone owners (v1.18.0) are a per-vertex fact: each rides with its
    // vertex, read back below off the value it was seeded beside.
    let owner = |i: usize| (i % 3) as u8;
    let field = ColourField {
        x: fx,
        y: fy,
        b: fb,
        grid: (0..fx * fy * fb).map(seed).collect(),
        owners: (0..fx * fy * fb).map(owner).collect(),
        amount: 1.0,
        enabled: true,
    };
    for (there, back) in [
        (Orientation::Rotate90, Orientation::Rotate270),
        (Orientation::Rotate270, Orientation::Rotate90),
        (Orientation::Transpose, Orientation::Transpose),
        (Orientation::Rotate180, Orientation::Rotate180),
    ] {
        let mut r = EditRecipe { colour_field: Some(field.clone()), ..Default::default() };
        orient_recipe_coords(&mut r, there, None);
        let turned = r.colour_field.clone().expect("the field survives the turn");
        let swaps = crate::decode::orientation_transposes(there);
        assert_eq!(
            (turned.x, turned.y),
            if swaps { (fy, fx) } else { (fx, fy) },
            "{there:?}: the axis lengths follow the frame"
        );
        assert_eq!(turned.grid.len(), fx * fy * fb, "no vertex was lost or invented");
        for (vertex, &o) in turned.grid.iter().zip(&turned.owners) {
            assert_eq!(o, owner(vertex[0] as usize), "{there:?}: an owner travels with its vertex");
        }
        orient_recipe_coords(&mut r, back, None);
        assert_eq!(
            r.colour_field.as_ref().expect("still there"),
            &field,
            "{there:?} then {back:?} must be the identity on the grid"
        );
    }
}

#[test]
fn orient_recipe_coords_moves_geometry_and_round_trips() {
    use crate::recipe::{LocalAdjustment, MaskComponent};
    let seed = || EditRecipe {
        crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Linear {
                zero_x: 0.5,
                zero_y: 0.0,
                full_x: 0.5,
                full_y: 0.45,
            },
            components: vec![MaskComponent {
                geometry: MaskGeometry::Radial {
                    top: 0.1,
                    left: 0.2,
                    bottom: 0.6,
                    right: 0.7,
                    feather: 0.5,
                    roundness: 0.0,
                    flipped: false,
                    angle: 0.0,
                    midpoint: 50.0,
                    mask_version: 2,
                },
                ..Default::default()
            }],
            range: Some(RangeMask::Color {
                r: 0.2,
                g: 0.4,
                b: 0.9,
                amount: 0.5,
                px: 0.25,
                py: 0.75,
            }),
            ..Default::default()
        }],
        ..Default::default()
    };
    // Normal / Unknown: not a single field moves, and the caller is told
    // nothing happened.
    for o in [Orientation::Normal, Orientation::Unknown] {
        let mut r = seed();
        assert!(!orient_recipe_coords(&mut r, o, probe_frame()), "{o:?} must report no move");
        assert_eq!(r, seed(), "{o:?} must not touch a single coordinate");
    }
    // The portrait ARW case, and back.
    let mut r = seed();
    assert!(orient_recipe_coords(&mut r, Orientation::Rotate270, probe_frame()));
    assert_ne!(r, seed(), "a quarter turn must actually move the geometry");
    // Hand-derived: Rotate270 maps (u,v) -> (v, 1-u), so the crop's
    // left/right come from top/bottom and its top/bottom from 1-right,
    // 1-left.
    let t = r.crop.expect("crop survives");
    for (got, want, what) in [
        (t.left, 0.2, "left"),
        (t.right, 0.9, "right"),
        (t.top, 1.0 - 0.8, "top"),
        (t.bottom, 1.0 - 0.1, "bottom"),
    ] {
        assert!((got - want).abs() < 1e-6, "crop {what}: {got} != {want} ({t:?})");
    }
    assert!(orient_recipe_coords(&mut r, Orientation::Rotate90, probe_frame()));
    let back = seed();
    let (c, c0) = (r.crop.unwrap(), back.crop.unwrap());
    assert!(
        (c.left - c0.left).abs() < 1e-6
            && (c.top - c0.top).abs() < 1e-6
            && (c.right - c0.right).abs() < 1e-6
            && (c.bottom - c0.bottom).abs() < 1e-6,
        "crop round trip: {c:?} vs {c0:?}"
    );
    let (
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y },
        MaskGeometry::Linear { zero_x: a, zero_y: b, full_x: cx, full_y: cy },
    ) = (&r.masks[0].mask, &back.masks[0].mask)
    else {
        panic!("linear geometry survives")
    };
    assert!(
        (zero_x - a).abs() < 1e-6
            && (zero_y - b).abs() < 1e-6
            && (full_x - cx).abs() < 1e-6
            && (full_y - cy).abs() < 1e-6,
        "linear round trip"
    );
    let Some(RangeMask::Color { px, py, .. }) = r.masks[0].range else {
        panic!("range survives")
    };
    assert!((px - 0.25).abs() < 1e-6 && (py - 0.75).abs() < 1e-6, "range point round trip");
}

/// R27 L-16c, half one. The `coord_era` migration's crop arm is exact ONLY
/// if the frame the crop is normalised against turns with the frame — and
/// that frame is `inscribed_dims`'s output, not the sensor rectangle,
/// because `render_pipeline` straightens before it crops.
///
/// Swapping `w` and `h` must swap the two answers and change nothing else.
/// The general branch shows it by inspection; this pins the branch
/// boundary too, which is where the `if w >= h` inside the thin case could
/// have made the claim false.
///
/// MUTATION THIS CATCHES: collapse the thin branch's
/// `if w >= h { (x/s, x/c) } else { (x/c, x/s) }` to either arm alone and
/// the sliver rows go red (verified). Note what does NOT catch it, because
/// it looks like it should: SWAPPING the general branch's two expressions
/// keeps the property, since exchanging both the inputs and the outputs of
/// a swap-equivariant pair is still swap-equivariant. This test pins the
/// symmetry, not the formula — `the_straighten_angle_reverses_only_under_
/// a_mirror` and the existing crop round trip pin the values.
#[test]
fn the_straightened_frame_turns_with_the_photo() {
    // Real ARW dims, their transpose, a square, and a sliver.
    for (w, h) in
        [(9504.0f32, 6336.0f32), (6336.0, 9504.0), (4000.0, 4000.0), (100.0, 3000.0)]
    {
        for deg in [0.5f32, 2.5, -2.5, 30.0, -44.0, 44.0, 45.0, -45.0] {
            let (a, b) = inscribed_dims(w, h, deg);
            let (c, d) = inscribed_dims(h, w, deg);
            let close = |x: f32, y: f32| (x - y).abs() <= 1e-3 * x.abs().max(1.0);
            assert!(
                close(a, d) && close(b, c),
                "inscribed_dims({w},{h},{deg}) = ({a},{b}) but ({h},{w}) = ({c},{d}) \
                 — the turned frame must be the turn of the frame"
            );
        }
    }
}

/// R27 L-16c, half two. R24 registered 「`straighten≠0` 时 crop 迁移一阶
/// 近似」 without a code site. The residue is the SIGN: rotations commute
/// with a quarter turn, so the four pure rotations were already exact, but
/// `rot(deg) ∘ mirror == mirror ∘ rot(−deg)`, so a mirrored photo was
/// straightened the wrong way by `2·deg` and every crop coordinate then
/// indexed content that had moved out from under it.
///
/// Both angles the registration is quoted against: a routine horizon
/// (2.5°) and the extreme end of the ±45 clamp (−44°).
///
/// MUTATION THIS CATCHES: delete the `if mirrors { r.straighten_deg = … }`
/// block in `orient_recipe_coords` and the four mirror rows go red; negate
/// on EVERY orientation instead and the three rotation rows go red.
#[test]
fn the_straighten_angle_reverses_only_under_a_mirror() {
    let seed = |deg: f32| EditRecipe {
        straighten_deg: deg,
        crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
        ..Default::default()
    };
    for deg in [2.5f32, -44.0] {
        // A quarter or half turn carries the tilt unchanged: the content
        // and the frame turned together.
        for o in [Orientation::Rotate90, Orientation::Rotate180, Orientation::Rotate270] {
            let mut r = seed(deg);
            assert!(orient_recipe_coords(&mut r, o, probe_frame()));
            assert_eq!(r.straighten_deg, deg, "{o:?} must not touch the tilt");
        }
        // A reflection reverses it — and these four are involutions, so
        // applying the same one twice is the identity (the round trip the
        // migration's bijectivity claim rests on).
        for o in [
            Orientation::HorizontalFlip,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Transverse,
        ] {
            let mut r = seed(deg);
            assert!(orient_recipe_coords(&mut r, o, probe_frame()));
            assert_eq!(r.straighten_deg, -deg, "{o:?} must reverse the tilt");
            assert!(orient_recipe_coords(&mut r, o, probe_frame()));
            assert_eq!(r.straighten_deg, deg, "{o:?} twice is the identity");
            // …and the crop came home with it (float tolerance, not `==`:
            // `1 − (1 − 0.1)` is 0.10000002 in f32).
            let (c, c0) = (r.crop.unwrap(), seed(deg).crop.unwrap());
            assert!(
                (c.left - c0.left).abs() < 1e-6
                    && (c.top - c0.top).abs() < 1e-6
                    && (c.right - c0.right).abs() < 1e-6
                    && (c.bottom - c0.bottom).abs() < 1e-6,
                "{o:?}: crop round trip {c:?} vs {c0:?}"
            );
        }
    }
    // And a photo with no tilt is untouched whatever the state.
    for o in [Orientation::Transverse, Orientation::Rotate90] {
        let mut r = seed(0.0);
        assert!(orient_recipe_coords(&mut r, o, probe_frame()));
        assert_eq!(r.straighten_deg, 0.0);
    }
}

/// A raster mask is an image FILE: the coordinate rewrites must leave its
/// path alone — its owner re-writes the file and re-points the path
/// afterwards (`pipeline::rotate_recipe`, `pipeline::migrate_raster_file`).
#[test]
fn raster_masks_are_left_for_their_owner_to_rewrite() {
    use crate::recipe::LocalAdjustment;
    let mut r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "sky.png".into() },
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(!recipe_has_frame_coords(&r), "a raster-only recipe has no turnable coordinate");
    let before = r.clone();
    orient_recipe_coords(&mut r, Orientation::Rotate270, probe_frame());
    assert_eq!(r, before, "the raster path must survive byte-for-byte under a turn");
    assert!(shift_recipe_coords(&mut r, 0.01, 0.02));
    assert_eq!(r, before, "and under a translation");
}

/// [`orient_vector`] is exactly the difference of two [`orient_point`]
/// images for every state — the affine map's linear part, spelled per
/// state so a small vector is not read off a subtraction near 1.
#[test]
fn orient_vector_is_the_linear_part_of_orient_point() {
    let states = [
        Orientation::Normal,
        Orientation::Unknown,
        Orientation::HorizontalFlip,
        Orientation::Rotate180,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ];
    let (dx, dy) = (-0.003367f32, 0.003157f32);
    for o in states {
        for (px, py) in [(0.0f32, 0.0f32), (0.3, 0.7), (1.0, 0.25)] {
            let (ax, ay) = orient_point(o, px + dx, py + dy);
            let (bx, by) = orient_point(o, px, py);
            let (vx, vy) = orient_vector(o, dx, dy);
            assert!(
                (ax - bx - vx).abs() < 1e-6 && (ay - by - vy).abs() < 1e-6,
                "{o:?} at ({px}, {py}): ({}, {}) vs ({vx}, {vy})",
                ax - bx,
                ay - by
            );
        }
    }
}

/// The era-2 translation moves every coordinate carrier by the same
/// vector, leaves every length alone, clamps only the crop, and undoes
/// itself.
#[test]
fn shift_recipe_coords_moves_every_carrier_and_round_trips() {
    use crate::recipe::{BrushStroke, ColourField, LocalAdjustment, MaskComponent};
    use crate::retouch::{RetouchArea, RetouchShape};
    let stroke = || BrushStroke {
        radius: 0.05,
        dabs: "r 0.050000\nd 0.500000 0.500000\nf 1.000000\nd 0.600000 0.400000".into(),
        ..Default::default()
    };
    let seed = || EditRecipe {
        crop: Some(Crop { left: 0.0, top: 0.2, right: 0.8, bottom: 1.0 }),
        straighten_deg: 3.0,
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.1,
                left: 0.2,
                bottom: 0.6,
                right: 0.7,
                feather: 0.5,
                roundness: 0.0,
                flipped: false,
                angle: 15.0,
                midpoint: 50.0,
                mask_version: 2,
            },
            components: vec![
                MaskComponent {
                    geometry: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 0.45 },
                    ..Default::default()
                },
                MaskComponent {
                    geometry: MaskGeometry::Brush {
                        name: "Brush 1".into(),
                        blend_mode: 0,
                        value: 1.0,
                        inverted: false,
                        strokes: vec![stroke()],
                    },
                    ..Default::default()
                },
                MaskComponent {
                    geometry: MaskGeometry::AiMask {
                        name: "Sky 1".into(),
                        subtype: 2,
                        ref_x: 0.25,
                        ref_y: 0.10,
                        blend_mode: 0,
                        value: 1.0,
                        inverted: false,
                        mask_version: 1,
                        provenance: Vec::new(),
                        gesture: vec![stroke()],
                        raster: Some("alpha.png".into()),
                    },
                    ..Default::default()
                },
            ],
            range: Some(RangeMask::Color { r: 0.2, g: 0.4, b: 0.9, amount: 0.5, px: 0.25, py: 0.75 }),
            ..Default::default()
        }],
        retouch: vec![
            RetouchArea {
                donor: Some([0.3, 0.3]),
                shape: RetouchShape::Ellipse { cx: 0.5, cy: 0.5, size_x: 0.02, size_y: 0.02 },
                ..Default::default()
            },
            RetouchArea { donor: None, shape: RetouchShape::Brush(vec![stroke()]), ..Default::default() },
        ],
        colour_field: Some(ColourField {
            x: 2,
            y: 1,
            b: 1,
            grid: vec![[0.0; 5], [1.0; 5]],
            owners: Vec::new(),
            amount: 1.0,
            enabled: true,
        }),
        ..Default::default()
    };
    let mut r = seed();
    assert!(!shift_recipe_coords(&mut r, 0.0, 0.0), "the zero vector is no move");
    assert_eq!(r, seed());
    let (du, dv) = (0.01f32, -0.02f32);
    assert!(shift_recipe_coords(&mut r, du, dv));
    let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
    let c = r.crop.unwrap();
    assert!(near(c.left, 0.01) && near(c.top, 0.18) && near(c.right, 0.81), "{c:?}");
    assert!(near(c.bottom, 0.98), "bottom moved up, off the edge it sat on: {c:?}");
    assert_eq!(r.straighten_deg, 3.0, "a translation turns nothing");
    let MaskGeometry::Radial { top, left, bottom, right, angle, feather, .. } = r.masks[0].mask else {
        panic!("radial")
    };
    assert!(near(left, 0.21) && near(right, 0.71) && near(top, 0.08) && near(bottom, 0.58));
    assert!(near(right - left, 0.5) && near(bottom - top, 0.5), "the radii are lengths");
    assert_eq!((angle, feather), (15.0, 0.5));
    let MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } = r.masks[0].components[0].geometry else {
        panic!("linear")
    };
    assert!(near(zero_x, 0.51) && near(zero_y, -0.02) && near(full_x, 0.51) && near(full_y, 0.43));
    let MaskGeometry::Brush { strokes, .. } = &r.masks[0].components[1].geometry else { panic!("brush") };
    assert_eq!(strokes[0].dabs, "r 0.050000\nd 0.510000 0.480000\nf 1.000000\nd 0.610000 0.380000");
    assert_eq!(strokes[0].radius, 0.05, "a radius is a length");
    let MaskGeometry::AiMask { ref_x, ref_y, gesture, raster, .. } = &r.masks[0].components[2].geometry
    else {
        panic!("ai mask")
    };
    assert!(near(*ref_x, 0.26) && near(*ref_y, 0.08));
    assert_eq!(gesture[0].dabs, "r 0.050000\nd 0.510000 0.480000\nf 1.000000\nd 0.610000 0.380000");
    assert_eq!(raster.as_deref(), Some("alpha.png"), "the cache is kept for the file rewrite");
    let Some(RangeMask::Color { px, py, .. }) = r.masks[0].range else { panic!("range") };
    assert!(near(px, 0.26) && near(py, 0.73));
    let RetouchShape::Ellipse { cx, cy, size_x, size_y } = r.retouch[0].shape else { panic!("ellipse") };
    assert!(near(cx, 0.51) && near(cy, 0.48) && size_x == 0.02 && size_y == 0.02);
    let d = r.retouch[0].donor.unwrap();
    assert!(near(d[0], 0.31) && near(d[1], 0.28));
    let RetouchShape::Brush(strokes) = &r.retouch[1].shape else { panic!("retouch brush") };
    assert_eq!(strokes[0].dabs, "r 0.050000\nd 0.510000 0.480000\nf 1.000000\nd 0.610000 0.380000");
    // The colour field: a 2 x 1 grid holding 0 and 1 moved right by a
    // fiftieth of the frame = 0.02 of a cell, so the right cell reads 0.98
    // of the way from 0 to 1 and the left cell clamps to the edge.
    let f = r.colour_field.as_ref().unwrap();
    assert!(near(f.grid[0][0], 0.0) && near(f.grid[1][0], 0.98), "{:?}", f.grid);
    // And back, on everything that was not clamped or resampled.
    assert!(shift_recipe_coords(&mut r, -du, -dv));
    let back = seed();
    let (
        MaskGeometry::Radial { top, left, bottom, right, .. },
        MaskGeometry::Radial { top: t0, left: l0, bottom: b0, right: r0, .. },
    ) = (&r.masks[0].mask, &back.masks[0].mask)
    else {
        panic!("radial")
    };
    assert!(near(*top, *t0) && near(*left, *l0) && near(*bottom, *b0) && near(*right, *r0), "radial round trip");
    let (MaskGeometry::Brush { strokes: a, .. }, MaskGeometry::Brush { strokes: b, .. }) =
        (&r.masks[0].components[1].geometry, &back.masks[0].components[1].geometry)
    else {
        panic!("brush")
    };
    assert_eq!(a[0].dabs, b[0].dabs, "the dab stream round-trips on Lightroom's six-decimal grid");
    let c = r.crop.unwrap();
    assert!(near(c.left, 0.0) && near(c.top, 0.2) && near(c.right, 0.8) && near(c.bottom, 1.0), "{c:?}");
}

/// The raster half reads the picture where it came from, clamps past the
/// edge, copies exactly on an integer move and blends on a fractional one.
#[test]
fn a_raster_shift_reads_the_picture_where_it_came_from() {
    let img = image::GrayImage::from_fn(6, 4, |x, y| image::Luma([(x * 10 + y) as u8]));
    let moved = shift_luma_raster(&img, -2.0, -1.0);
    for y in 0..4u32 {
        for x in 0..6u32 {
            let (sx, sy) = ((x + 2).min(5), (y + 1).min(3));
            assert_eq!(moved.get_pixel(x, y).0[0], (sx * 10 + sy) as u8, "({x}, {y})");
        }
    }
    let half = shift_luma_raster(&img, -0.5, 0.0);
    assert_eq!(half.get_pixel(0, 0).0[0], 5, "half way between 0 and 10");
    assert_eq!(half.get_pixel(5, 0).0[0], 50, "past the right edge the edge continues");
    let same = shift_luma_raster(&img, 0.0, 0.0);
    assert_eq!(same.as_raw(), img.as_raw(), "the zero move is the identity");
}

/// One brush group with `n` strokes built from `(value, radius, flow,
/// hardness, dabs)` — the fixture every brush test below stands on.
fn probe_brush(strokes: &[(f32, f32, f32, f32, &str)]) -> MaskGeometry {
    MaskGeometry::Brush {
        name: "Brush 1".into(),
        blend_mode: 0,
        // The AGGREGATE's MaskValue: the subtract pair's other half, never
        // a strength — `mask_weight`'s `Brush` arm spells out why.
        value: 1.0,
        inverted: false,
        strokes: strokes
            .iter()
            .map(|&(value, radius, flow, center_weight, dabs)| crate::recipe::BrushStroke {
                value,
                radius,
                flow,
                center_weight,
                sync_id: "FA7459A9F5626F4881D7B730C3093F95".into(),
                dabs: dabs.into(),
            })
            .collect(),
    }
}
