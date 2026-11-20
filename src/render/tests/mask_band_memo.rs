// One part of the engine's tests (src/render/tests.rs includes it): the band read's kept shape planes (`mask_band_memo`) — a develop served them is the develop from nothing bit for bit, and every input they are computed from tells a kept plane apart.

/// The rasters the tests below read: a soft step on the frame's content edge
/// (sensor frame), the same step drawn in the lens-corrected frame of
/// `base`, and an off-centre disc.
fn bm_rasters(dir: &Path, base: &EditRecipe) -> (String, String, String) {
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    save_mask_raster(&memo_step(48, 32, 0.5), Path::new(&path("a.png")), None).unwrap();
    save_mask_raster(&memo_step(48, 32, 0.5), Path::new(&path("b.png")), BakedGeometry::frame_of(base).as_ref()).unwrap();
    save_mask_raster(&off_centre_disc(40, 30), Path::new(&path("c.png")), None).unwrap();
    (path("a.png"), path("b.png"), path("c.png"))
}

/// A brush whose dabs cross the frame's content edge, `x` its first dab's place.
fn bm_brush(x: f32) -> MaskGeometry {
    let dabs = format!("d {:.6} 0.400000\nd {:.6} 0.420000\nd {:.6} 0.460000", x, x + 0.05, x + 0.1);
    probe_brush(&[(1.0, 0.12, 0.8, 0.5, dabs.as_str())])
}

/// Four pictures of one size that are four frames to the mask pass's own
/// memory (`MaskChain` serves nothing across them), so every develop below
/// runs every mask and reads every band.
fn bm_pictures((w, h): (usize, usize), seed: usize) -> Vec<Vec<[f32; 3]>> {
    (0..4).map(|k| memo_frame(w, h, 0.03 * k as f32 + 0.007 * seed as f32)).collect()
}

/// How many band pixels mask `index` of `r` lists on `frame` (`BandShares`).
fn bm_listed(frame: &[[f32; 3]], (w, h): (usize, usize), r: &EditRecipe, lens: MaskFrame<'_>, index: usize) -> usize {
    mask_memo::without_chain(|| {
        let rasters = load_mask_raster_snapshot(r, &crate::diag::dropped()).expect("the fixture's rasters load");
        let unwarp = lens.unwarp((w as f32, h as f32));
        let m = lens.prepare_linear_handles(&r.masks[index], (w as f32, h as f32));
        let m = m.as_ref();
        let brushes = masks::Brushes::of(m, w, h);
        let cover = masks::Cover::new(m, &brushes, &rasters, unwarp.as_ref(), w, h).expect("the mask's rasters load");
        let owed = mask_owed::BandShares::read(&cover, frame);
        (0..h).map(|y| owed.row(y).len()).sum()
    })
}

/// The mask pass on each picture in turn with the band planes kept, against
/// the pass from nothing: equal on every one, the first two computing their
/// planes (the second keeping them) and every later one handed them back.
fn bm_settle(name: &str, pictures: &[Vec<[f32; 3]>], dims: (usize, usize), r: &EditRecipe, lens: MaskFrame<'_>) {
    let fresh: Vec<Vec<u32>> = pictures.iter().map(|p| memo_fresh(p, dims, r, lens)).collect();
    mask_memo::forget_kept_frames();
    for (k, picture) in pictures.iter().enumerate() {
        let served = mask_band_memo::served();
        assert!(memo_pass(picture, dims, r, lens).0 == fresh[k], "{name}: picture {k} is the pass from nothing");
        let now = mask_band_memo::served() - served;
        assert_eq!(now > 0, k >= 2, "{name}: picture {k}: the planes are computed on the first two pictures and handed back from the third ({now} served)");
    }
}

/// THE pin of the band read's kept planes: a develop handed them is the
/// develop from nothing, bit for bit — one raster layer, two, an inverted
/// mask, a brush, a corrected raster and a sensor one under a lens map of
/// either kind.
#[test]
fn a_develop_with_the_band_planes_kept_is_the_develop_from_nothing() {
    use crate::recipe::{MaskCombine, MaskComponent};
    let dir = crate::test_dir("band-memo-warm");
    let base = marked_recipe();
    let (a, b, c) = bm_rasters(&dir, &base);
    let geom = geometry_profile(&base).into_owned();
    let bitmap = |path: &str| MaskGeometry::Bitmap { path: path.into() };
    let dims = (96usize, 64usize);
    let cases: Vec<(&str, Vec<LocalAdjustment>, MaskFrame<'_>)> = vec![
        ("one raster layer", vec![LocalAdjustment { mask: bitmap(&a), exposure_ev: 0.6, ..Default::default() }], MaskFrame::AsRendered),
        (
            "two raster layers",
            vec![LocalAdjustment {
                mask: bitmap(&a),
                components: vec![MaskComponent { geometry: bitmap(&c), mode: MaskCombine::Subtract, inverted: false }],
                shadows: 50.0,
                ..Default::default()
            }],
            MaskFrame::AsRendered,
        ),
        ("an inverted mask", vec![LocalAdjustment { mask: bitmap(&a), inverted: true, exposure_ev: 0.5, ..Default::default() }], MaskFrame::AsRendered),
        ("a brush", vec![LocalAdjustment { mask: bm_brush(0.42), exposure_ev: 0.7, ..Default::default() }], MaskFrame::AsRendered),
        (
            "under the corrected frame's lens map",
            vec![
                LocalAdjustment { mask: MaskGeometry::select_sky(0.2, 0.2, false, b.clone()), exposure_ev: 0.6, ..Default::default() },
                LocalAdjustment { mask: bitmap(&a), highlights: -40.0, ..Default::default() },
            ],
            MaskFrame::Corrected { profile: &geom },
        ),
        (
            "under the downstream lens map",
            vec![
                LocalAdjustment { mask: MaskGeometry::select_sky(0.2, 0.2, false, b.clone()), exposure_ev: 0.6, ..Default::default() },
                LocalAdjustment { mask: bitmap(&a), highlights: -40.0, ..Default::default() },
            ],
            MaskFrame::WarpedDownstream { profile: &geom, amount: 0.0 },
        ),
    ];
    for (seed, (name, masks, lens)) in cases.into_iter().enumerate() {
        let r = EditRecipe { masks, ..base.clone() };
        let pictures = bm_pictures(dims, seed);
        assert!(bm_listed(&pictures[0], dims, &r, lens, 0) > 0, "premise, {name}: the first mask has a band on a content edge");
        bm_settle(name, &pictures, dims, &r, lens);
    }
}

/// The colour field's band (`masks::field_sky_band`), the read's other
/// caller, handed its kept planes: read alone and through a whole preview,
/// it is the read from nothing.
#[test]
fn the_colour_fields_band_with_its_planes_kept_is_the_band_from_nothing() {
    use crate::recipe::{ColourField, MaskRole};
    let dir = crate::test_dir("band-memo-field");
    let (w, h, edge) = (120u32, 72u32, 60u32);
    let path = dir.join("sky.png");
    save_mask_raster(&image::GrayImage::from_fn(w, h, |x, _| image::Luma([255 - (((x as f32 - 57.5) / 5.0).clamp(0.0, 1.0) * 255.0).round() as u8])), &path, None).unwrap();
    let (fx, fy, fb) = (3usize, 2usize, 2usize);
    let lifted = |v: usize| (v / fb).is_multiple_of(fx);
    let grid: Vec<[f32; 5]> = (0..fx * fy * fb).map(|v| if lifted(v) { [0.5, 0.0, 0.0, 0.0, 0.0] } else { [0.0; 5] }).collect();
    let owners = (0..fx * fy * fb).map(|v| if lifted(v) { ColourField::OWNER_SKY } else { ColourField::OWNER_LAND }).collect();
    let field = ColourField { x: fx, y: fy, b: fb, grid, owners, amount: 1.0, enabled: true };
    let zone = LocalAdjustment { mask: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() }, role: MaskRole::ZoneSky, exposure_ev: 0.2, ..Default::default() };
    let r = EditRecipe { masks: vec![zone], colour_field: Some(field), ..Default::default() };

    // The band alone, on three pictures.
    let rasters = load_mask_raster_snapshot(&r, &crate::diag::dropped()).expect("the zone's raster loads");
    let (fw, fh) = (w as usize, h as usize);
    let band = |data: &[[f32; 3]]| -> Vec<(u32, u32)> {
        masks::field_sky_band(&r, &rasters, MaskFrame::AsRendered, fw, fh, data).into_iter().map(|(i, c)| (i, c.to_bits())).collect()
    };
    let frames: Vec<Vec<[f32; 3]>> = (0..4).map(|k| band_frame(fw, fh, edge as usize, 0.55 + 0.05 * k as f32, 0.08)).collect();
    let fresh: Vec<_> = frames.iter().map(|f| mask_memo::without_chain(|| band(f))).collect();
    assert!(!fresh[0].is_empty(), "premise: the zone's raster has a band on the content edge");
    mask_memo::forget_kept_frames();
    for (k, frame) in frames.iter().enumerate() {
        let served = mask_band_memo::served();
        assert!(band(frame) == fresh[k], "frame {k}: the band is the band from nothing");
        assert_eq!(mask_band_memo::served() > served, k >= 2, "frame {k}: computed on the first two frames, handed back from the third");
    }

    // Through the preview: three pictures, so the pass's own memory serves no
    // band. On the first, the zone mask's own read in the mask pass is the
    // second computation of the key the field's band read just noted — one
    // layer, one raster, one frame — and keeps its planes; from the second
    // picture on both reads are handed them.
    let pictures: Vec<DynamicImage> = (0..3).map(|k| band_scene(w, h, edge, 150 + 10 * k, 20)).collect();
    let fresh: Vec<_> = pictures.iter().map(|p| mask_memo::without_chain(|| develop_preview(p, &r).to_rgb8().into_raw())).collect();
    assert!(fresh[0] != mask_memo::without_chain(|| develop_preview(&pictures[0], &EditRecipe { colour_field: None, ..r.clone() }).to_rgb8().into_raw()), "premise: the field shows");
    mask_memo::forget_kept_frames();
    for (k, picture) in pictures.iter().enumerate() {
        let served = mask_band_memo::served();
        assert!(develop_preview(picture, &r).to_rgb8().into_raw() == fresh[k], "preview {k} is the preview from nothing");
        assert_eq!(mask_band_memo::served() > served, k > 0, "preview {k}: kept on the first preview, handed back after");
    }
}

/// Warm the planes with `before` on two pictures, then develop `after` on a
/// third (and `before` is never the answer there): the develop from nothing
/// of `after`.
fn bm_not_served(
    name: &str,
    (before, dims_before, lens_before): (&EditRecipe, (usize, usize), MaskFrame<'_>),
    (after, dims_after, lens_after): (&EditRecipe, (usize, usize), MaskFrame<'_>),
    between: impl FnOnce(),
    seed: usize,
) {
    let warm = bm_pictures(dims_before, seed);
    let third = memo_frame(dims_after.0, dims_after.1, 0.5 + 0.007 * seed as f32);
    let stale = memo_fresh(&third, dims_after, before, lens_before);
    mask_memo::forget_kept_frames();
    for picture in &warm[..2] {
        memo_pass(picture, dims_before, before, lens_before);
    }
    assert!(mask_band_memo::held().1 > 0, "premise, {name}: planes are kept");
    between();
    let fresh = memo_fresh(&third, dims_after, after, lens_after);
    let out = memo_pass(&third, dims_after, after, lens_after).0;
    assert!(out == fresh, "{name}: the develop of the changed input is the develop from nothing");
    if dims_before == dims_after {
        assert!(fresh != stale, "premise, {name}: the change moves the result");
    }
}

/// A raster repainted under its path, at its own size: the loader decodes it
/// anew, the kept planes of the old one are not served.
///
/// MUTATION: `LayerShape::same` without the raster.
#[test]
fn the_kept_planes_are_not_served_for_a_repainted_raster() {
    let dir = crate::test_dir("band-memo-raster");
    let path = dir.join("r.png");
    save_mask_raster(&memo_step(48, 32, 0.5), &path, None).unwrap();
    let r = EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() }, exposure_ev: 0.6, ..Default::default() }], ..Default::default() };
    let dims = (96usize, 64usize);
    let lens = MaskFrame::AsRendered;
    let noisy = image::GrayImage::from_fn(48, 32, |x, y| image::Luma([if (x * 7 + y * 13) % 5 < 2 { 255 } else { (x * 5) as u8 }]));
    bm_not_served(
        "a repainted raster",
        (&r, dims, lens),
        (&r, dims, lens),
        || save_mask_raster(&noisy, &path, None).unwrap(),
        1,
    );
}

/// A layer whose own parameters change while its raster stays the same
/// object — an AI raster's own inversion — and a brush whose strokes move:
/// the kept planes of the one before are not served.
///
/// MUTATION: `LayerShape::same` without the text.
#[test]
fn the_kept_planes_are_not_served_for_a_layer_with_other_parameters() {
    let dir = crate::test_dir("band-memo-params");
    let (a, _, _) = bm_rasters(&dir, &marked_recipe());
    let dims = (96usize, 64usize);
    let lens = MaskFrame::AsRendered;
    let with = |mask: MaskGeometry| EditRecipe { masks: vec![LocalAdjustment { mask, exposure_ev: 0.6, ..Default::default() }], ..Default::default() };
    let (sky, flipped) = (with(MaskGeometry::select_sky(0.2, 0.2, false, a.clone())), with(MaskGeometry::select_sky(0.2, 0.2, true, a)));
    bm_not_served("an AI raster's own inversion", (&sky, dims, lens), (&flipped, dims, lens), || {}, 2);
    let (brush, moved) = (with(bm_brush(0.42)), with(bm_brush(0.30)));
    bm_not_served("a brush's strokes", (&brush, dims, lens), (&moved, dims, lens), || {}, 3);
}

/// Another frame size — one column fewer, one row fewer, the blocks the same
/// count: the kept planes of the other size are not served.
///
/// MUTATION: the frame's `w` and `h` out of the strip's key.
#[test]
fn the_kept_planes_are_not_served_for_another_frame_size() {
    let dir = crate::test_dir("band-memo-size");
    let (a, _, _) = bm_rasters(&dir, &marked_recipe());
    let r = EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: a }, exposure_ev: 0.6, ..Default::default() }], ..Default::default() };
    let lens = MaskFrame::AsRendered;
    let block = |(w, h): (usize, usize)| (w as f32 / 48.0).max(h as f32 / 32.0).ceil() as usize;
    for (seed, other) in [(95usize, 64usize), (96, 63)].into_iter().enumerate() {
        assert_eq!(block(other), block((96, 64)), "premise: the same block edge");
        assert_eq!((other.0.div_ceil(2), other.1.div_ceil(2)), (48, 32), "premise: the same count of blocks");
        bm_not_served(&format!("{other:?} after (96, 64)"), (&r, (96, 64), lens), (&r, other, lens), || {}, 4 + seed);
    }
}

/// Another lens map — another profile, and none against one: the kept
/// planes read through the other are not served.
///
/// MUTATION: `LayerShape::same` without the lens map.
#[test]
fn the_kept_planes_are_not_served_under_another_lens_map() {
    let dir = crate::test_dir("band-memo-lens");
    let base = marked_recipe();
    let (a, _, _) = bm_rasters(&dir, &base);
    let geom = geometry_profile(&base).into_owned();
    let mut other = base.clone();
    other.lens_profile_distortion_scale = 50.0;
    let other_geom = geometry_profile(&other).into_owned();
    let r = EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: a }, exposure_ev: 0.6, ..Default::default() }], ..base };
    let dims = (96usize, 64usize);
    let (lens, other_lens) = (MaskFrame::Corrected { profile: &geom }, MaskFrame::Corrected { profile: &other_geom });
    bm_not_served("another profile", (&r, dims, lens), (&r, dims, other_lens), || {}, 6);
    bm_not_served("a lens map after none", (&r, dims, MaskFrame::AsRendered), (&r, dims, lens), || {}, 7);
    bm_not_served("no lens map after one", (&r, dims, lens), (&r, dims, MaskFrame::AsRendered), || {}, 8);
}

/// A layer's band rows as bits.
fn bm_rows(rows: &[mask_owed::BandRow]) -> Vec<(usize, u32, [u32; 7])> {
    let mut out = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        for (x, owed) in row {
            let [c0, c1] = owed.cover;
            let [s0, s1, s2, s3] = owed.share;
            out.push((y, *x, [c0, c1, owed.area, s0, s1, s2, s3].map(f32::to_bits)));
        }
    }
    out
}

/// Another block edge, with every other input the same: the planes counted
/// in the other are not served. (Through a develop the block edge follows
/// from the raster and the frame's size, which are in the key already;
/// `layer_band` is handed it, so its key holds it too.)
///
/// MUTATION: the block edge out of the strip's key.
#[test]
fn the_kept_planes_are_not_served_for_another_block_edge() {
    let (w, h) = (64usize, 32usize);
    let data = band_frame(w, h, 32, 0.5, 0.1);
    let coverage = |x: usize, _: usize| ((x as f32 - 29.0) / 6.0).clamp(0.0, 1.0);
    let shape = mask_band_memo::LayerShape::named("a layer");
    mask_band_memo::using(std::sync::Arc::new(mask_band_memo::BandMemo::new(1 << 30)), || {
        let fresh = |block: usize| bm_rows(&mask_band::layer_band(&data, (w, h), block, w * h, &coverage, None));
        let kept = |block: usize| bm_rows(&mask_band::layer_band(&data, (w, h), block, w * h, &coverage, Some(&shape)));
        assert!(!fresh(1).is_empty() && !fresh(2).is_empty() && fresh(1) != fresh(2), "premise: both block edges list a band, each its own");
        for round in 0..3 {
            assert!(kept(1) == fresh(1), "block 1, round {round}");
        }
        for round in 0..3 {
            assert!(kept(2) == fresh(2), "block 2 after block 1, round {round}");
        }
        assert!(kept(1) == fresh(1), "block 1 again");
    });
}

/// A frame read in three strips: each strip's planes are its own, and the
/// read handed them back is the read from nothing.
///
/// MUTATION: the strip's first block row out of its key.
#[test]
fn each_strip_of_a_tall_frame_keeps_its_own_planes() {
    let (w, h) = (4096usize, 800usize);
    let data = band_frame(w, h, 2048, 0.5, 0.1);
    let coverage = |x: usize, _: usize| ((x as f32 - 2045.0) / 6.0).clamp(0.0, 1.0);
    let shape = mask_band_memo::LayerShape::named("a tall layer");
    let memo = std::sync::Arc::new(mask_band_memo::BandMemo::new(1 << 30));
    mask_band_memo::using(memo.clone(), || {
        let fresh = bm_rows(&mask_band::layer_band(&data, (w, h), 1, w * h, &coverage, None));
        let listed: Vec<usize> = [0usize, 400, 799].iter().map(|&y| fresh.iter().filter(|e| e.0 == y).count()).collect();
        assert!(listed.iter().all(|&n| n > 0), "premise: every strip has a band: {listed:?}");
        for round in 0..2 {
            let read = bm_rows(&mask_band::layer_band(&data, (w, h), 1, w * h, &coverage, Some(&shape)));
            assert!(read == fresh, "computed, round {round}: the read from nothing");
        }
        assert!(memo.held().1 > 1, "premise: the frame is read in more than one strip ({} kept)", memo.held().1);
        let served = mask_band_memo::served();
        let again = bm_rows(&mask_band::layer_band(&data, (w, h), 1, w * h, &coverage, Some(&shape)));
        assert!(again == fresh, "handed back: the read from nothing");
        assert_eq!(mask_band_memo::served() - served, memo.held().1, "every strip handed back its own planes");
    });
}

/// A budget too small for everything, and one too small for anything: what
/// is kept stays within it, and every develop is the develop from nothing.
///
/// MUTATION: `BandMemo::put` without the byte walk.
#[test]
fn the_kept_planes_hold_their_budget_and_develops_stay_exact() {
    use crate::recipe::{MaskCombine, MaskComponent};
    let dir = crate::test_dir("band-memo-budget");
    let (a, _, c) = bm_rasters(&dir, &marked_recipe());
    let dims = (96usize, 64usize);
    let lens = MaskFrame::AsRendered;
    let two = EditRecipe {
        masks: vec![
            LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: a.clone() },
                components: vec![MaskComponent { geometry: MaskGeometry::Bitmap { path: c.clone() }, mode: MaskCombine::Add, inverted: false }],
                exposure_ev: 0.6,
                ..Default::default()
            },
            LocalAdjustment { mask: MaskGeometry::Bitmap { path: c }, inverted: true, shadows: 40.0, ..Default::default() },
        ],
        ..Default::default()
    };
    // What one layer's strip holds.
    let one = EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: a }, exposure_ev: 0.6, ..Default::default() }], ..Default::default() };
    let probe = std::sync::Arc::new(mask_band_memo::BandMemo::new(1 << 30));
    mask_band_memo::using(probe.clone(), || {
        for shift in [0.9f32, 0.95] {
            memo_pass(&memo_frame(dims.0, dims.1, shift), dims, &one, lens);
        }
    });
    let (strip, count) = probe.held();
    assert_eq!(count, 1, "premise: one layer, one strip");
    for (budget, most) in [(strip, 1usize), (100, 0)] {
        let memo = std::sync::Arc::new(mask_band_memo::BandMemo::new(budget));
        let pictures = bm_pictures(dims, 9 + most);
        let fresh: Vec<_> = pictures.iter().map(|p| memo_fresh(p, dims, &two, lens)).collect();
        mask_band_memo::using(memo.clone(), || {
            for (k, picture) in pictures.iter().enumerate() {
                assert!(memo_pass(picture, dims, &two, lens).0 == fresh[k], "budget {budget}: picture {k}");
                let (bytes, kept) = memo.held();
                assert!(bytes <= budget && kept <= most, "budget {budget}: picture {k}: {bytes} bytes in {kept} strips");
            }
        });
    }
}

/// Several threads developing different masks at different sizes at once,
/// on one memory — one large enough for all, one that holds a strip at a
/// time — each get the develop from nothing.
#[test]
fn develops_on_several_threads_at_once_get_the_develop_from_nothing() {
    use crate::recipe::{MaskCombine, MaskComponent};
    let dir = crate::test_dir("band-memo-threads");
    let base = marked_recipe();
    let (a, b, c) = bm_rasters(&dir, &base);
    let geom = geometry_profile(&base).into_owned();
    let recipes = [
        EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: a.clone() }, exposure_ev: 0.6, ..Default::default() }], ..base.clone() },
        EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: c.clone() },
                components: vec![MaskComponent { geometry: MaskGeometry::Bitmap { path: a.clone() }, mode: MaskCombine::Intersect, inverted: true }],
                shadows: 45.0,
                ..Default::default()
            }],
            ..base.clone()
        },
        EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::select_sky(0.2, 0.2, false, b), exposure_ev: 0.4, ..Default::default() }], ..base.clone() },
        EditRecipe { masks: vec![LocalAdjustment { mask: bm_brush(0.40), exposure_ev: 0.5, ..Default::default() }], ..base.clone() },
    ];
    let sizes = [(96usize, 64usize), (80, 56), (112, 72), (96, 64)];
    let lenses = [MaskFrame::AsRendered, MaskFrame::AsRendered, MaskFrame::Corrected { profile: &geom }, MaskFrame::AsRendered];
    for budget in [64usize << 20, 40_000] {
        let memo = std::sync::Arc::new(mask_band_memo::BandMemo::new(budget));
        std::thread::scope(|s| {
            for t in 0..8 {
                let (memo, r, dims, lens) = (memo.clone(), &recipes[t % 4], sizes[t % 4], lenses[t % 4]);
                s.spawn(move || {
                    let pictures: Vec<_> = (0..4).map(|k| memo_frame(dims.0, dims.1, 0.02 * k as f32 + 0.003 * t as f32)).collect();
                    let fresh: Vec<_> = pictures.iter().map(|p| memo_fresh(p, dims, r, lens)).collect();
                    mask_band_memo::using(memo, || {
                        for round in 0..3 {
                            for (k, picture) in pictures.iter().enumerate() {
                                assert!(memo_pass(picture, dims, r, lens).0 == fresh[k], "budget {budget}, thread {t}, round {round}, picture {k}");
                            }
                        }
                    });
                });
            }
        });
        assert!(memo.held().0 <= budget, "budget {budget}: {:?}", memo.held());
    }
}

/// The planes are given back with the kept frames (`forget_kept_frames`,
/// which the editor calls when its canvas changes).
///
/// MUTATION: `forget_kept_frames` without `mask_band_memo::forget`.
#[test]
fn the_kept_planes_are_given_back_with_the_kept_frames() {
    let dir = crate::test_dir("band-memo-forget");
    let (a, _, _) = bm_rasters(&dir, &marked_recipe());
    let r = EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: a }, exposure_ev: 0.6, ..Default::default() }], ..Default::default() };
    let dims = (96usize, 64usize);
    mask_memo::forget_kept_frames();
    for shift in [0.2f32, 0.25] {
        memo_pass(&memo_frame(dims.0, dims.1, shift), dims, &r, MaskFrame::AsRendered);
    }
    assert!(mask_band_memo::held().1 > 0, "premise: planes are kept");
    mask_memo::forget_kept_frames();
    assert_eq!(mask_band_memo::held(), (0, 0), "given back");
}

/// A frame with a side past the editor's canvas — an export — keeps no
/// plane and makes no key, however often it is developed; one of exactly the
/// canvas's side keeps its planes.
///
/// MUTATION: `LayerShape::of` without the ceiling.
#[test]
fn a_frame_past_the_canvas_keeps_no_band_planes() {
    let dir = crate::test_dir("band-memo-ceiling");
    let (a, _, _) = bm_rasters(&dir, &marked_recipe());
    let r = EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: a }, exposure_ev: 0.6, ..Default::default() }], ..Default::default() };
    for (w, kept) in [(mask_memo::CANVAS_EDGE + 1, false), (mask_memo::CANVAS_EDGE, true)] {
        mask_memo::forget_kept_frames();
        let dims = (w, 3usize);
        let frame = memo_frame(w, 3, 0.04);
        let fresh = memo_fresh(&frame, dims, &r, MaskFrame::AsRendered);
        for sight in 0..3 {
            assert!(memo_pass(&frame, dims, &r, MaskFrame::AsRendered).0 == fresh, "{w} px, sight {sight}");
            assert_eq!(mask_band_memo::held().1 > 0, kept && sight > 0, "{w} px, sight {sight}: {:?}", mask_band_memo::held());
        }
    }
}

/// A develop that happens once keeps no plane — an export of a photo no
/// larger than the canvas, a thumbnail: the planes are kept from a strip's
/// second computation, as the kept frames are from a frame's second sight.
/// The next develop of the same masks keeps them.
///
/// MUTATION: `BandMemo::put` keeping on the first computation.
#[test]
fn one_develop_alone_keeps_no_band_planes() {
    use crate::recipe::{MaskCombine, MaskComponent};
    let dir = crate::test_dir("band-memo-once");
    let (a, _, c) = bm_rasters(&dir, &marked_recipe());
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: a },
            components: vec![MaskComponent { geometry: MaskGeometry::Bitmap { path: c }, mode: MaskCombine::Subtract, inverted: false }],
            exposure_ev: 0.6,
            ..Default::default()
        }],
        ..Default::default()
    };
    let dims = (96usize, 64usize);
    let pictures = bm_pictures(dims, 12);
    mask_memo::forget_kept_frames();
    assert!(memo_pass(&pictures[0], dims, &r, MaskFrame::AsRendered).0 == memo_fresh(&pictures[0], dims, &r, MaskFrame::AsRendered));
    assert_eq!(mask_band_memo::held(), (0, 0), "one develop: no plane bytes held");
    memo_pass(&pictures[1], dims, &r, MaskFrame::AsRendered);
    assert_eq!(mask_band_memo::held().1, 2, "the second develop keeps both layers' planes");
}

/// Planes kept for a raster object that is gone — its file repainted, the
/// old object decoded no more — are dropped at the next put: they can never
/// be served again.
///
/// MUTATION: `BandMemo::put` without the `alive` check.
#[test]
fn planes_of_a_raster_that_is_gone_are_dropped() {
    let dir = crate::test_dir("band-memo-dead");
    let path = dir.join("r.png");
    save_mask_raster(&memo_step(48, 32, 0.5), &path, None).unwrap();
    let r = EditRecipe { masks: vec![LocalAdjustment { mask: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() }, exposure_ev: 0.6, ..Default::default() }], ..Default::default() };
    let dims = (96usize, 64usize);
    let pictures = bm_pictures(dims, 13);
    mask_memo::forget_kept_frames();
    for picture in &pictures[..2] {
        memo_pass(picture, dims, &r, MaskFrame::AsRendered);
    }
    assert_eq!(mask_band_memo::held().1, 1, "premise: the raster's planes are kept");
    let noisy = image::GrayImage::from_fn(48, 32, |x, y| image::Luma([if (x * 7 + y * 13) % 5 < 2 { 255 } else { (x * 5) as u8 }]));
    save_mask_raster(&noisy, &path, None).unwrap();
    assert!(memo_pass(&pictures[2], dims, &r, MaskFrame::AsRendered).0 == memo_fresh(&pictures[2], dims, &r, MaskFrame::AsRendered));
    assert_eq!(mask_band_memo::held(), (0, 0), "the old raster's planes are gone and the new one's only noted");
}
