// One part of the engine's tests (src/render/tests.rs includes it): what the mask pass keeps from one develop of a frame to the next (`mask_memo`) — the lens map, and the frame the first changed mask finds — each pinned bit for bit against the same thing computed from nothing.

/// A frame every kind of mask has something to act on: a ramp with a lit
/// half, so a raster's step lies on a content edge and a range has levels to
/// tell apart. `shift` lifts it, so two tests' frames are two pictures.
fn memo_frame(w: usize, h: usize, shift: f32) -> Vec<[f32; 3]> {
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let ramp = shift + 0.12 + 0.45 * (x + 2 * y) as f32 / (w + 2 * h) as f32;
            let lit = if x > w / 2 { 0.3 } else { 0.0 };
            [ramp + lit, 0.9 * ramp + lit, 0.8 * ramp + 0.5 * lit]
        })
        .collect()
}

/// A soft step raster: covered left of `edge` (a fraction of the width), a
/// ramp three pixels wide across it.
fn memo_step(w: u32, h: u32, edge: f32) -> image::GrayImage {
    image::GrayImage::from_fn(w, h, |x, _| image::Luma([((edge * w as f32 - x as f32) / 3.0 + 0.5).clamp(0.0, 1.0).mul_add(255.0, 0.5) as u8]))
}

/// A recipe under a lens profile with one mask of every kind the pass reads:
/// a linear and a shaped gradient, radials (feathered, turned, flipped), a
/// bitmap, an AI raster drawn in the corrected frame, a brush, both range
/// kinds, components in each of the three modes with an inverted one,
/// inverted adjustments, and every pass — dehaze, tone and colour, clarity,
/// texture, sharpness, noise. The last mask is the one a test drags.
fn memo_recipe(dir: &Path) -> EditRecipe {
    use crate::recipe::{MaskCombine, MaskComponent, MaskRole, RangeMask};
    let base = marked_recipe();
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    save_mask_raster(&memo_step(48, 32, 0.5), Path::new(&path("a.png")), None).unwrap();
    save_mask_raster(&memo_step(48, 32, 0.35), Path::new(&path("b.png")), BakedGeometry::frame_of(&base).as_ref()).unwrap();
    save_mask_raster(&off_centre_disc(40, 30), Path::new(&path("c.png")), None).unwrap();
    let radial = |left: f32, top: f32, feather: f32, angle: f32, flipped: bool| MaskGeometry::Radial {
        top,
        left,
        bottom: top + 0.45,
        right: left + 0.4,
        feather,
        roundness: 0.0,
        flipped,
        angle,
        midpoint: 50.0,
        mask_version: 2,
    };
    let linear = |zero_x: f32, full_x: f32| MaskGeometry::Linear { zero_x, zero_y: 0.3, full_x, full_y: 0.7 };
    let masks = vec![
        LocalAdjustment { mask: linear(0.7, 0.2), exposure_ev: 0.6, ..Default::default() },
        LocalAdjustment { mask: radial(0.1, 0.2, 0.4, 20.0, false), contrast: 30.0, saturation: 25.0, inverted: true, ..Default::default() },
        LocalAdjustment { mask: MaskGeometry::Bitmap { path: path("a.png") }, shadows: 60.0, temperature: 25.0, tint: -10.0, ..Default::default() },
        LocalAdjustment {
            mask: MaskGeometry::select_sky(0.2, 0.2, false, path("b.png")),
            role: MaskRole::ZoneSky,
            highlights: -40.0,
            components: vec![MaskComponent { geometry: linear(0.9, 0.4), mode: MaskCombine::Intersect, inverted: false }],
            ..Default::default()
        },
        LocalAdjustment {
            mask: probe_brush(&[(1.0, 0.12, 0.8, 0.5, "d 0.300000 0.400000\nd 0.340000 0.420000\nd 0.380000 0.460000")]),
            texture: 60.0,
            clarity: 40.0,
            ..Default::default()
        },
        LocalAdjustment {
            mask: radial(0.5, 0.1, 0.8, 0.0, true),
            range: Some(RangeMask::Luminance { lo_outer: 0.1, lo: 0.25, hi: 0.7, hi_outer: 0.9 }),
            dehaze: 35.0,
            hue: 20.0,
            ..Default::default()
        },
        LocalAdjustment {
            mask: linear(0.1, 0.8),
            range: Some(RangeMask::Color { r: 0.6, g: 0.55, b: 0.4, amount: 60.0, px: 0.7, py: 0.5 }),
            blacks: 30.0,
            toning_hue: 200.0,
            toning_amount: 40.0,
            ..Default::default()
        },
        LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: path("a.png") },
            components: vec![
                MaskComponent { geometry: radial(0.3, 0.3, 0.5, -15.0, false), mode: MaskCombine::Add, inverted: false },
                MaskComponent { geometry: MaskGeometry::Bitmap { path: path("c.png") }, mode: MaskCombine::Subtract, inverted: true },
            ],
            sharpness: 50.0,
            noise_reduction: 40.0,
            amount: 0.8,
            ..Default::default()
        },
        LocalAdjustment {
            mask: MaskGeometry::Shaped { zero_x: 0.2, zero_y: 0.9, full_x: 0.6, full_y: 0.1, falloff: vec![0.0, 0.1, 0.6, 1.0] },
            whites: 25.0,
            inverted: true,
            ..Default::default()
        },
        LocalAdjustment { mask: radial(0.3, 0.3, 0.5, 0.0, false), exposure_ev: 0.2, ..Default::default() },
    ];
    EditRecipe { masks, sharpen_mask: 20.0, ..base }
}

/// The mask pass on a copy of `frame`, its rasters loaded as a develop loads
/// them, and what the pass's memory did.
fn memo_pass(frame: &[[f32; 3]], (w, h): (usize, usize), r: &EditRecipe, lens: MaskFrame<'_>) -> (Vec<u32>, mask_memo::ChainTrace) {
    let rasters = load_mask_raster_snapshot(r, &crate::diag::dropped()).expect("the fixture's rasters load");
    let mut out = frame.to_vec();
    apply_masks(&mut out, w, h, r, &rasters, lens, FilmScale::NATIVE);
    (out.iter().flatten().map(|v| v.to_bits()).collect(), mask_memo::chain_trace())
}

/// The pass with nothing kept and nothing served.
fn memo_fresh(frame: &[[f32; 3]], dims: (usize, usize), r: &EditRecipe, lens: MaskFrame<'_>) -> Vec<u32> {
    mask_memo::without_chain(|| memo_pass(frame, dims, r, lens).0)
}

/// A develop of the same frame under the same masks until the whole pass is
/// served from what is kept: three at most (seen, kept, served).
fn memo_settle(frame: &[[f32; 3]], dims: (usize, usize), r: &EditRecipe, lens: MaskFrame<'_>) {
    let fresh = memo_fresh(frame, dims, r, lens);
    for _ in 0..3 {
        assert!(memo_pass(frame, dims, r, lens).0 == fresh, "{lens:?}: a settled pass is the pass from nothing");
    }
    assert_eq!(mask_memo::chain_trace().resumed, r.masks.len(), "{lens:?}: the third develop of one frame applies no mask again");
}

/// THE pin of the mask pass's memory: on every lens frame, with every kind
/// of mask, the pass is the pass computed from nothing bit for bit — cold,
/// served from what is kept, a slider in hand move after move, and after each
/// kind of change that must not be served: a moved handle, a changed feather,
/// an added and a removed component, an inversion, an earlier mask's own
/// slider, a range, a field of the recipe outside its masks, a raster
/// repainted under its path, another frame size, another lens fact.
///
/// MUTATIONS THIS KILLS (each run, 2026-11-19): `MaskKey::eq` without the
/// text — the moved handle and its fellows are served the kept frame;
/// `MaskKey::eq` without the rasters — the repainted file is; the slot looked
/// up without the frame's size — the turned frame is; `same_bits` answering
/// `true` — the other picture is; the context compared without the lens
/// frame — the other profile is.
#[test]
fn a_pass_resumed_from_a_kept_frame_is_the_pass_from_nothing_bit_for_bit() {
    let dir = crate::test_dir("mask-memo-chain");
    let base = memo_recipe(&dir);
    let geom = geometry_profile(&base).into_owned();
    let mut other_lens = base.clone();
    other_lens.lens_profile_distortion_scale = 50.0;
    let other_geom = geometry_profile(&other_lens).into_owned();
    let dims = (96usize, 64usize);
    let (count, last) = (base.masks.len(), base.masks.len() - 1);
    let none = mask_memo::ChainTrace::NONE;
    let lenses = [MaskFrame::AsRendered, MaskFrame::Corrected { profile: &geom }, MaskFrame::WarpedDownstream { profile: &geom, amount: 0.0 }];
    for (nth, lens) in lenses.into_iter().enumerate() {
        // Each lens frame its own picture: its first sight is a first sight.
        let frame = memo_frame(dims.0, dims.1, 0.01 * nth as f32);
        let fresh = |r: &EditRecipe| memo_fresh(&frame, dims, r, lens);
        let kept = |r: &EditRecipe| memo_pass(&frame, dims, r, lens);
        // Cold: a first sight, which copies no pixel.
        let plain = fresh(&base);
        assert!(plain != frame.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>(), "premise: the masks move the frame");
        assert_eq!(mask_memo::chain_trace(), none, "the pass from nothing keeps nothing");
        let (cold, trace) = kept(&base);
        assert!(cold == plain && trace == none, "{lens:?}: a first sight is computed whole and keeps no pixel: {trace:?}");
        // Seen again: kept. A third time: every mask served.
        let (again, trace) = kept(&base);
        assert!(again == plain, "{lens:?}: the second develop");
        assert_eq!(trace, mask_memo::ChainTrace { kept_start: true, kept_at: Some(count), ..none }, "{lens:?}: the second sight keeps the frame");
        let (served, trace) = kept(&base);
        assert!(served == plain && trace.resumed == count, "{lens:?}: the third develop is served whole: {trace:?}");

        // A slider in hand on the last mask: from the second move on, only that mask runs.
        for (step, ev) in [0.35f32, 0.5, 0.65].into_iter().enumerate() {
            let mut moved = base.clone();
            moved.masks[last].exposure_ev = ev;
            let (out, trace) = kept(&moved);
            assert!(out == fresh(&moved), "{lens:?}: move {step} is the pass from nothing");
            assert!(out != plain, "premise: the move shows");
            assert_eq!(trace.resumed, if step == 0 { 0 } else { last }, "{lens:?}: move {step}: {trace:?}");
            assert_eq!(trace.kept_at, Some(last), "{lens:?}: the frame the mask in hand finds is the one kept");
        }

        // Each change that must not be served, on a mask before the kept one.
        type Change = (&'static str, usize, fn(&mut EditRecipe));
        let changes: [Change; 9] = [
            ("a moved handle", 0, |r| {
                if let MaskGeometry::Linear { zero_x, .. } = &mut r.masks[0].mask {
                    *zero_x += 0.08;
                }
            }),
            ("a changed feather", 1, |r| {
                if let MaskGeometry::Radial { feather, .. } = &mut r.masks[1].mask {
                    *feather = 0.9;
                }
            }),
            ("an inversion", 1, |r| r.masks[1].inverted = false),
            ("an added component", 2, |r| {
                let geometry = MaskGeometry::Linear { zero_x: 0.2, zero_y: 0.5, full_x: 0.6, full_y: 0.5 };
                r.masks[2].components.push(crate::recipe::MaskComponent { geometry, mode: crate::recipe::MaskCombine::Subtract, inverted: false });
            }),
            ("a removed component", 7, |r| {
                r.masks[7].components.pop();
            }),
            ("a component's mode", 3, |r| r.masks[3].components[0].mode = crate::recipe::MaskCombine::Add),
            ("an earlier mask's own slider", 3, |r| r.masks[3].highlights = -80.0),
            ("a range", 5, |r| r.masks[5].range = Some(crate::recipe::RangeMask::Luminance { lo_outer: 0.3, lo: 0.4, hi: 0.6, hi_outer: 0.7 })),
            ("a field of the recipe outside its masks", 0, |r| r.sharpen_mask = 70.0),
        ];
        for (what, index, change) in changes {
            memo_settle(&frame, dims, &base, lens);
            let mut varied = base.clone();
            change(&mut varied);
            let (out, trace) = kept(&varied);
            assert!(out != plain, "premise: {what} moves the result");
            assert!(out == fresh(&varied), "{lens:?}: after {what} the pass is the pass from nothing");
            assert!(trace.resumed <= index, "{lens:?}: {what} at mask {index}, the pass resumed at {}", trace.resumed);
        }

        // Another frame size: the same pixels read as a frame turned on its side.
        memo_settle(&frame, dims, &base, lens);
        let turned = (dims.1, dims.0);
        let (out, trace) = memo_pass(&frame, turned, &base, lens);
        assert!(out == memo_fresh(&frame, turned, &base, lens) && out != plain, "{lens:?}: another frame size");
        assert_eq!(trace, none, "{lens:?}: another size is another frame, seen for the first time");
        assert_eq!(memo_pass(&frame, turned, &base, lens).1.resumed, 0, "{lens:?}: and nothing of the other size is served to it");

        // Two sizes side by side — a draft and its canvas — each keep their own.
        memo_settle(&frame, dims, &base, lens);
        let (large, wide) = (memo_frame(2 * dims.0, 2 * dims.1, 0.01 * nth as f32), (2 * dims.0, 2 * dims.1));
        memo_settle(&large, wide, &base, lens);
        assert_eq!(kept(&base).1.resumed, count, "{lens:?}: the small frame is still served after the large one");

        // Another picture of the same size under the same masks, and one the
        // hint cannot tell from the kept one: the large frame is hinted by
        // every sixth pixel, and this one differs at a pixel between two.
        let mut other = large.clone();
        let odd = wide.0 * wide.1 / 2 + 7;
        assert!(wide.0 * wide.1 / 4096 == 6 && !odd.is_multiple_of(6), "premise: the changed pixel is not one the hint reads");
        other[odd][1] += 0.25;
        let (out, trace) = memo_pass(&other, wide, &base, lens);
        assert!(out == memo_fresh(&other, wide, &base, lens), "{lens:?}: another picture is the pass from nothing");
        assert!(out != memo_fresh(&large, wide, &base, lens), "premise: the other picture shows");
        assert!(trace.kept_start && trace.resumed == 0, "{lens:?}: found by its hint, told apart by its pixels: {trace:?}");
    }

    // Another lens fact: the same frame, masks and recipe read through another profile.
    let frame = memo_frame(dims.0, dims.1, 0.05);
    let (lens, other) = (MaskFrame::Corrected { profile: &geom }, MaskFrame::Corrected { profile: &other_geom });
    memo_settle(&frame, dims, &base, lens);
    let (out, trace) = memo_pass(&frame, dims, &base, other);
    assert!(out == memo_fresh(&frame, dims, &base, other), "another lens fact is the pass from nothing");
    assert!(out != memo_fresh(&frame, dims, &base, lens), "premise: the other profile moves the masks");
    assert_eq!(trace.resumed, 0, "another lens fact: no mask is served");

    // A raster repainted under its path: the loader decodes it anew, and the
    // new object is not the kept one.
    let lens = MaskFrame::AsRendered;
    memo_settle(&frame, dims, &base, lens);
    let before = memo_fresh(&frame, dims, &base, lens);
    let noisy = image::GrayImage::from_fn(48, 32, |x, y| image::Luma([if (x * 7 + y * 13) % 5 < 2 { 255 } else { (x * 5) as u8 }]));
    save_mask_raster(&noisy, &dir.join("a.png"), None).unwrap();
    let (out, trace) = memo_pass(&frame, dims, &base, lens);
    assert!(out != before, "premise: the repainted raster moves the result");
    assert!(out == memo_fresh(&frame, dims, &base, lens), "a repainted raster is read, not the kept frame");
    assert!(trace.resumed <= 2, "the first mask that reads the file is mask 2; the pass resumed at {}", trace.resumed);
}

/// The lens map is built once for a profile, an amount and a size, handed
/// out as built, and built again for any other.
///
/// MUTATION: drop the frame's size, the amount or the builder from
/// `unwarp_of`'s key.
#[test]
fn the_lens_map_is_built_once_and_again_for_another_frame() {
    let r = marked_recipe();
    let geom = geometry_profile(&r).into_owned();
    let mut other = r.clone();
    other.lens_profile_distortion_scale = 50.0;
    let other_geom = geometry_profile(&other).into_owned();
    let probe = |u: &MaskUnwarp| -> Vec<u32> {
        let at = [(0.1f32, 0.2f32), (0.5, 0.5), (0.93, 0.07), (0.3, 0.88)];
        at.iter().flat_map(|&(x, y)| [u.engine_at(x, y), u.source_point(x, y), u.radial_at(x, y, 0.4, 0.6)]).flat_map(|(a, b)| [a.to_bits(), b.to_bits()]).collect()
    };
    let dims = (300.0f32, 200.0f32);
    let cases = [
        (MaskFrame::Corrected { profile: &geom }, dims),
        (MaskFrame::Corrected { profile: &geom }, (200.0, 300.0)),
        (MaskFrame::Corrected { profile: &other_geom }, dims),
        (MaskFrame::WarpedDownstream { profile: &geom, amount: 0.0 }, dims),
        (MaskFrame::WarpedDownstream { profile: &geom, amount: 12.0 }, dims),
    ];
    let mut seen: Vec<Vec<u32>> = Vec::new();
    for (frame, dims) in cases {
        let built = mask_memo::unwarp_built();
        let first = frame.unwarp(dims).expect("the profile moves the frame");
        assert_eq!(mask_memo::unwarp_built(), built + 1, "{frame:?} {dims:?}: another frame's map is built");
        let again = frame.unwarp(dims).expect("kept");
        assert_eq!(mask_memo::unwarp_built(), built + 1, "{frame:?} {dims:?}: asked again, it is not built again");
        let direct = match frame {
            MaskFrame::Corrected { profile } => MaskUnwarp::corrected(profile, dims),
            MaskFrame::WarpedDownstream { profile, amount } => MaskUnwarp::new(profile, amount, dims),
            MaskFrame::AsRendered => None,
        }
        .expect("built directly");
        assert!(probe(&first) == probe(&direct) && probe(&again) == probe(&direct), "{frame:?} {dims:?}: the kept map is the built one");
        assert!(!seen.contains(&probe(&direct)), "premise: {frame:?} {dims:?} is a map of its own");
        seen.push(probe(&direct));
    }
    assert!(MaskFrame::AsRendered.unwarp(dims).is_none());
    let flat = crate::recipe::LensProfile::default();
    for sight in 0..2 {
        assert!(MaskFrame::Corrected { profile: &flat }.unwarp(dims).is_none(), "sight {sight}: a profile that moves nothing has no map, kept or not");
    }
}

/// The develop a preview runs — the frame the masks find computed from the
/// picture — gives the same bytes developed once, again, and a third time.
#[test]
fn a_preview_developed_again_is_the_preview_developed_once() {
    let dir = crate::test_dir("mask-memo-preview");
    let r = memo_recipe(&dir);
    let base = DynamicImage::ImageRgb8(RgbImage::from_fn(96, 64, |x, y| image::Rgb([(40 + 2 * x) as u8, (30 + 3 * y) as u8, if x > 48 { 200 } else { 90 }])));
    let once = mask_memo::without_chain(|| develop_preview(&base, &r).to_rgb8().into_raw());
    for sight in 0..3 {
        assert!(develop_preview(&base, &r).to_rgb8().into_raw() == once, "sight {sight}");
    }
    assert_eq!(mask_memo::chain_trace().resumed, r.masks.len(), "the third preview applies no mask again");
    let mut moved = r.clone();
    moved.masks[9].exposure_ev = 0.9;
    let fresh = mask_memo::without_chain(|| develop_preview(&base, &moved).to_rgb8().into_raw());
    assert!(fresh != once, "premise: the move shows");
    for sight in 0..2 {
        assert!(develop_preview(&base, &moved).to_rgb8().into_raw() == fresh, "moved, sight {sight}");
    }
    assert_eq!(mask_memo::chain_trace().resumed, 9, "the second move starts at the mask in hand");
}

/// The colour field's band is read once for a frame and handed out again
/// while the frame the masks find and its zone mask are the same — a slider
/// in hand on another mask — and read again when the zone's raster is
/// repainted, when the zone's own slider moves, for another picture, and
/// every time for a zone whose raster the develop's snapshot does not hold.
///
/// MUTATIONS THIS KILLS (each run, 2026-11-19): `MaskChain::field_band`
/// serving without comparing the zone mask — the zone's own slider and the
/// repainted raster are handed the kept band; `MaskChain::open` carrying the
/// band over whatever the pixels — the other picture is.
#[test]
fn the_colour_fields_band_is_kept_with_the_frame_and_its_zone_mask() {
    use crate::recipe::{ColourField, MaskRole};
    let dir = crate::test_dir("mask-memo-field");
    let (w, h, edge) = (120u32, 72u32, 60u32);
    let path = dir.join("sky.png");
    let step = |from: f32| image::GrayImage::from_fn(w, h, |x, _| image::Luma([255 - (((x as f32 - from) / 5.0).clamp(0.0, 1.0) * 255.0).round() as u8]));
    save_mask_raster(&step(57.5), &path, None).unwrap();
    let base = band_scene(w, h, edge, 150, 20);
    let (fx, fy, fb) = (3usize, 2usize, 2usize);
    let lifted = |v: usize| (v / fb).is_multiple_of(fx);
    let grid: Vec<[f32; 5]> = (0..fx * fy * fb).map(|v| if lifted(v) { [0.5, 0.0, 0.0, 0.0, 0.0] } else { [0.0; 5] }).collect();
    let owners = (0..fx * fy * fb).map(|v| if lifted(v) { ColourField::OWNER_SKY } else { ColourField::OWNER_LAND }).collect();
    let field = ColourField { x: fx, y: fy, b: fb, grid, owners, amount: 1.0, enabled: true };
    let zone = |exposure_ev: f32| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: path.to_string_lossy().into_owned() },
        role: MaskRole::ZoneSky,
        exposure_ev,
        ..Default::default()
    };
    let hand = |exposure_ev: f32| LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.9, zero_y: 0.5, full_x: 0.1, full_y: 0.5 },
        exposure_ev,
        ..Default::default()
    };
    let recipe = |zone_ev: f32, hand_ev: f32| EditRecipe { masks: vec![zone(zone_ev), hand(hand_ev)], colour_field: Some(field.clone()), ..Default::default() };
    let fresh = |r: &EditRecipe| mask_memo::without_chain(|| develop_preview(&base, r).to_rgb8().into_raw());
    let kept = |r: &EditRecipe| (develop_preview(&base, r).to_rgb8().into_raw(), mask_memo::chain_trace());

    let r = recipe(0.2, 0.3);
    let plain = fresh(&r);
    assert!(plain != fresh(&EditRecipe { colour_field: None, ..r.clone() }), "premise: the field shows");
    for (sight, served) in [false, false, true].into_iter().enumerate() {
        let (out, trace) = kept(&r);
        assert!(out == plain, "sight {sight}");
        assert_eq!(trace.field_served, served, "sight {sight}: read on the first two, handed out from the third");
    }
    // A slider in hand on the other mask: the band is the kept one, move after move.
    for hand_ev in [0.5f32, 0.7] {
        let moved = recipe(0.2, hand_ev);
        let (out, trace) = kept(&moved);
        assert!(out == fresh(&moved) && out != plain, "the other mask moved to {hand_ev}");
        assert!(trace.field_served, "the zone and the frame the masks find are the same: {trace:?}");
    }
    // The zone's own slider: the band does not read it, and is read again all the same.
    let own = recipe(0.4, 0.7);
    let (out, trace) = kept(&own);
    assert!(out == fresh(&own) && !trace.field_served, "the zone's own slider: {trace:?}");
    // Another picture the hint cannot tell from this one: the frame is hinted
    // by every second pixel, and the column right of the edge is an odd one.
    assert!((w * h) as usize / 4096 == 2 && (edge + 1) % 2 == 1 && w % 2 == 0, "premise: the changed pixels are not ones the hint reads");
    let mut lit = base.to_rgb8();
    for y in 0..h {
        lit.put_pixel(edge + 1, y, image::Rgb([150; 3]));
    }
    let lit = DynamicImage::ImageRgb8(lit);
    let other = mask_memo::without_chain(|| develop_preview(&lit, &own).to_rgb8().into_raw());
    let out = develop_preview(&lit, &own).to_rgb8().into_raw();
    let trace = mask_memo::chain_trace();
    assert!(out == other, "another picture reads its own band");
    assert!(trace.kept_start && !trace.field_served && trace.resumed == 0, "found by its hint, told apart by its pixels: {trace:?}");
    assert!(!kept(&own).1.field_served && kept(&own).1.field_served, "back on the first picture: read once, then handed out");
    // The zone's raster repainted under its path, its step six pixels along.
    assert!(kept(&own).1.field_served, "premise: settled on the old raster");
    let before = fresh(&own);
    save_mask_raster(&step(63.5), &path, None).unwrap();
    let (out, trace) = kept(&own);
    assert!(out == fresh(&own) && out != before, "the repainted zone is read");
    assert!(!trace.field_served, "a repainted zone's band is read again: {trace:?}");
    // A zone with no controls of its own: its raster is loaded apart, and nothing read from it is kept.
    let idle = recipe(0.0, 0.7);
    let plain = fresh(&idle);
    for sight in 0..3 {
        let (out, trace) = kept(&idle);
        assert!(out == plain && !trace.field_served, "sight {sight}: an idle zone's band is read every time: {trace:?}");
    }
}
