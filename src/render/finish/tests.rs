//! The Effects panel's two operators, and the tail that puts them after the
//! crop.

use image::GenericImageView;

use super::*;
use crate::recipe::Crop;

/// A flat 16-bit frame at `v`.
fn grey(w: u32, h: u32, v: f32) -> DynamicImage {
    let s = to_u16(v);
    DynamicImage::ImageRgb16(
        image::ImageBuffer::from_raw(w, h, vec![s; (w * h * 3) as usize]).expect("size matches"),
    )
}

/// A flat 16-bit frame whose every pixel is `c`.
fn flat(w: u32, h: u32, c: [f32; 3]) -> DynamicImage {
    DynamicImage::ImageRgb16(image::ImageBuffer::from_fn(w, h, |_, _| {
        image::Rgb([to_u16(c[0]), to_u16(c[1]), to_u16(c[2])])
    }))
}

/// One pixel at its OWN sample depth. `DynamicImage::get_pixel` answers in
/// 8-bit RGBA whatever the buffer holds, which reads a 16-bit render at 1/257
/// of its value — the finishing pass is judged at 1e-4, so the depth matters.
fn at(img: &DynamicImage, x: u32, y: u32) -> [f32; 3] {
    match img {
        DynamicImage::ImageRgb16(b) => {
            let p = b.get_pixel(x, y).0;
            [p[0] as f32 / 65535.0, p[1] as f32 / 65535.0, p[2] as f32 / 65535.0]
        }
        DynamicImage::ImageRgb8(b) => {
            let p = b.get_pixel(x, y).0;
            [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]
        }
        other => panic!("the finishing pass answered in {other:?}"),
    }
}

fn luma(img: &DynamicImage, x: u32, y: u32) -> f32 {
    luma601(&at(img, x, y))
}

/// The tail with no lens profile and no straighten — every test here is about
/// the crop and the two finishing operators.
fn finished(img: DynamicImage, r: &EditRecipe, policy: CropPolicy) -> DynamicImage {
    frame_and_finish(img, r, &crate::recipe::LensProfile::default(), FilmScale::NATIVE, policy)
}

/// A flat frame with one bright square at its centre, so a geometric stage's
/// effect is legible as WHERE the square went.
fn marked(side: u32) -> DynamicImage {
    let mut img = grey(side, side, 0.25);
    let DynamicImage::ImageRgb16(b) = &mut img else { panic!("grey answers 16-bit") };
    let c = side / 2;
    for y in c - 2..=c + 2 {
        for x in c - 2..=c + 2 {
            b.put_pixel(x, y, image::Rgb([to_u16(0.9); 3]));
        }
    }
    img
}

/// Raw samples, for a byte comparison between two renders of the same frame.
fn samples(img: &DynamicImage) -> (u32, u32, Vec<u16>) {
    let DynamicImage::ImageRgb16(b) = img else { panic!("the stage answered {img:?}") };
    (b.width(), b.height(), b.as_raw().clone())
}

fn vignetted(amount: f32) -> EditRecipe {
    EditRecipe { post_crop_vignette: amount, ..Default::default() }
}

#[test]
fn the_vignette_is_centred_on_the_crop_and_not_on_the_frame() {
    // 200 × 100, cropped to its LEFT half: the vignette belongs to the
    // 100 × 100 rectangle, which is the whole point of the control's name.
    let r = EditRecipe {
        crop: Some(Crop { left: 0.0, top: 0.0, right: 0.5, bottom: 1.0 }),
        ..vignetted(-100.0)
    };
    let keep = finished(grey(200, 100, 0.5), &r, CropPolicy::Keep);
    assert_eq!(keep.dimensions(), (200, 100), "a preview keeps the whole frame");
    let crop_centre = luma(&keep, 50, 50);
    assert!(
        (crop_centre - 0.5).abs() < 1e-3,
        "the crop's own centre is untouched, not {crop_centre}"
    );
    assert!(luma(&keep, 1, 1) < 0.2, "the crop's corner is darkened");
    assert!(
        luma(&keep, 150, 50) < crop_centre - 0.1,
        "outside the crop the same field continues — it is not a second vignette"
    );
    // The frame's own centre sits on the crop's right EDGE, so it must be
    // darkened; a vignette centred on the frame would leave it alone.
    assert!(luma(&keep, 100, 50) < crop_centre - 0.05, "the frame's centre is not the crop's");

    // And the export, which cuts, must agree with the preview pixel for pixel
    // inside the rectangle — that agreement is what `CropPolicy` exists for.
    let cut = finished(grey(200, 100, 0.5), &r, CropPolicy::Cut);
    assert_eq!(cut.dimensions(), (100, 100));
    for (x, y) in [(1, 1), (50, 50), (99, 99), (20, 80)] {
        assert!(
            (luma(&cut, x, y) - luma(&keep, x, y)).abs() < 2e-4,
            "({x},{y}): the preview promises what the export delivers"
        );
    }
}

/// The falloff is symmetric about the crop's continuous centre: on a 100 px
/// wide crop that centre is x = 50.0, and the pixels 5 and 94 — centres
/// 5.5 and 94.5, each 44.5 px from it — must darken alike. Sampled at the
/// pixel CORNER (until 2026-09-24) they sat 45 and 44 px from it and the
/// whole vignette rode half a pixel up-left. (Pixels 25 / 74 would sit
/// inside the core a Midpoint-50 falloff leaves untouched.)
///
/// MUTATION: sample `vg.weight` at `(fx, fy)` again.
#[test]
fn the_vignette_is_sampled_at_the_pixel_centre() {
    let img = finished(grey(100, 100, 0.5), &vignetted(-100.0), CropPolicy::Keep);
    for (a, b) in [((5, 50), (94, 50)), ((50, 5), (50, 94)), ((10, 10), (89, 89))] {
        let (la, lb) = (luma(&img, a.0, a.1), luma(&img, b.0, b.1));
        assert!(la < 0.48, "premise: the vignette reaches ({}, {}): {la}", a.0, a.1);
        assert!((la - lb).abs() < 2e-4, "{a:?} {la} against {b:?} {lb}: not symmetric about the centre");
    }
}

#[test]
fn the_companions_render_lightrooms_own_defaults_when_the_sidecar_is_silent() {
    // A Lightroom sidecar writes Midpoint / Feather / Style only when they are
    // not its defaults, so a recipe that holds none of them must render the
    // same picture as one that spells all three out. Reading the stored 0
    // instead would put the falloff at the centre of the frame.
    let silent = vignetted(-80.0);
    let mut spelt = vignetted(-80.0);
    spelt.set_resolved("post_crop_vignette_mid", 50.0);
    spelt.set_resolved("post_crop_vignette_feather", 50.0);
    spelt.set_resolved("post_crop_vignette_style", 1.0);
    let a = finished(grey(64, 64, 0.5), &silent, CropPolicy::Cut);
    let b = finished(grey(64, 64, 0.5), &spelt, CropPolicy::Cut);
    assert_eq!(a.as_bytes(), b.as_bytes(), "absent companions are Lightroom's defaults");
    assert!(luma(&a, 0, 0) < 0.3, "and they really do render a vignette");
    assert!((luma(&a, 32, 32) - 0.5).abs() < 1e-3, "whose centre is clean");
}

#[test]
fn the_three_vignette_styles_differ_the_way_adobe_describes_them() {
    // A corner pixel with one channel at the clip and Highlights at 100.
    let colour = [1.0, 0.45, 0.45];
    let style = |n: f32, hl: f32| {
        let mut r = vignetted(-100.0);
        r.post_crop_vignette_hl = hl;
        r.set_resolved("post_crop_vignette_style", n);
        at(&finished(flat(48, 48, colour), &r, CropPolicy::Cut), 0, 0)
    };
    let (hp, cp, po) = (style(1.0, 100.0), style(2.0, 100.0), style(3.0, 100.0));
    // The gain is a LINEAR-light multiply, so "the ratios are kept" is a claim
    // about linear values — the encoded ones move even under a flat gain.
    let lin = |v: f32| sample_lut(&transfer_luts().0, v);
    let ratio = |p: [f32; 3]| lin(p[0]) / lin(p[1]).max(1e-6);
    // Highlight Priority recovers per channel, so the clipped one is spared
    // further than its neighbours — Adobe's own warning about colour shifts.
    assert!(hp[0] > cp[0] + 0.05, "Highlight Priority recovers the clipped channel: {hp:?}");
    assert!(
        ratio(hp) > ratio(colour) * 1.5,
        "and the ratio moves, which is the colour shift it is named for: {} vs {}",
        ratio(hp),
        ratio(colour)
    );
    // Colour Priority reads ONE number for the pixel, so the ratios cannot move.
    assert!(
        (ratio(cp) / ratio(colour) - 1.0).abs() < 0.02,
        "Colour Priority keeps the hue: {} vs {}",
        ratio(cp),
        ratio(colour)
    );
    // Paint Overlay mixes toward black in the encoded domain and recovers
    // nothing, so it is the darkest of the three at the same Amount.
    assert!(po[0] < cp[0] && po[0] < 0.05, "Paint Overlay flattens toward black: {po:?}");
    // A flat mix is not a gain, and a deep shadow is where the two part
    // company: halving an encoded 0.05 lands at 0.025, while two stops of
    // LINEAR gain on the same pixel lands at half of that again.
    let shadow = |n: f32| {
        let mut r = vignetted(-50.0);
        r.set_resolved("post_crop_vignette_style", n);
        luma(&finished(grey(48, 48, 0.05), &r, CropPolicy::Cut), 0, 0)
    };
    assert!(
        shadow(3.0) > shadow(2.0) * 1.5,
        "a flat mix is not a gain: paint {} vs colour {}",
        shadow(3.0),
        shadow(2.0)
    );
}

#[test]
fn highlights_only_spare_a_vignette_that_darkens() {
    // ONE tone, ONE magnitude, two signs. The tone is the whole difficulty:
    // `HL_OPEN` opens on the LINEAR level, so a probe that sits below it passes
    // BOTH halves of this test for the wrong reason — encoded 0.4 is linear
    // 0.13 and never opens the window at all. So the darkening half comes
    // first: it is what proves the window is open at this tone, and only then
    // does the brightening half mean anything.
    const TONE: f32 = 0.80; // linear ≈ 0.60, well inside HL_OPEN
    const AMOUNT: f32 = 15.0; // small enough that +AMOUNT cannot clip at 0.60
    let run = |amount: f32, hl: f32| {
        let mut r = vignetted(amount);
        r.post_crop_vignette_hl = hl;
        finished(grey(48, 48, TONE), &r, CropPolicy::Cut)
    };
    let darken = |hl: f32| luma(&run(-AMOUNT, hl), 0, 0);
    assert!(
        darken(100.0) > darken(0.0) + 0.01,
        "the window must be OPEN at this tone or the rest of this test is vacuous: {} vs {}",
        darken(100.0),
        darken(0.0)
    );
    assert_eq!(
        run(AMOUNT, 0.0).as_bytes(),
        run(AMOUNT, 100.0).as_bytes(),
        "and there is nothing to spare in a vignette that brightens"
    );
    // The other end of the same slider's name: a shadow is not a highlight, so
    // the deepest darkening a vignette can do is not spared at all.
    let deep = |hl: f32| {
        let mut r = vignetted(-100.0);
        r.post_crop_vignette_hl = hl;
        luma(&finished(grey(48, 48, 0.12), &r, CropPolicy::Cut), 0, 0)
    };
    assert!(
        (deep(100.0) - deep(0.0)).abs() < 1e-3,
        "Highlights spared a shadow: {} vs {}",
        deep(100.0),
        deep(0.0)
    );
}

#[test]
fn roundness_moves_the_contour_between_the_crops_edge_and_a_circle() {
    // A wide frame: at +100 the contour is a circle in pixel space, so the
    // SHORT edge's midpoint is darkened far more than at 0, where the contour
    // is the ellipse inscribed in the crop and every edge midpoint matches.
    let shape = |round: f32| {
        let mut r = vignetted(-100.0);
        r.post_crop_vignette_round = round;
        let img = finished(grey(200, 100, 0.5), &r, CropPolicy::Cut);
        (luma(&img, 100, 0), luma(&img, 0, 50)) // top edge midpoint, left edge midpoint
    };
    let (top0, left0) = shape(0.0);
    assert!((top0 - left0).abs() < 0.02, "at 0 the contour follows the crop's own aspect");
    let (top_round, left_round) = shape(100.0);
    assert!(
        left_round < left0 - 0.05,
        "+100 is a circle inscribed in the SHORT edge, so the long sides fall inside the \
         falloff: {left_round} vs {left0}"
    );
    assert!((top_round - top0).abs() < 0.02, "while the short edge keeps its own contour");
    let (top_rect, _) = shape(-100.0);
    assert!(
        top_rect < top0 - 0.02,
        "−100 is the rounded RECTANGLE that hugs the crop's edges, so an edge midpoint is \
         darkened nearly as far as a corner: {top_rect} vs {top0}"
    );
}

#[test]
fn grain_is_deterministic_sized_in_film_pixels_and_fades_out_of_both_ends() {
    let r = EditRecipe { grain: 100.0, ..Default::default() };
    let once = finished(grey(96, 96, 0.5), &r, CropPolicy::Cut);
    let again = finished(grey(96, 96, 0.5), &r, CropPolicy::Cut);
    assert_eq!(once.as_bytes(), again.as_bytes(), "the same frame grains the same way, always");
    assert!(
        (0..96).map(|x| luma(&once, x, 48)).any(|l| (l - 0.5).abs() > 0.01),
        "and it does something"
    );

    // Sign changes along a row measure how fine the lattice is IN RASTER
    // PIXELS. A preview whose every pixel stands for four film pixels shows
    // the export's grain four times smaller, which is what the export looks
    // like once it is downscaled to that preview — so the same recipe grains
    // that raster FINER, not coarser.
    let runs = |img: &DynamicImage| {
        (1..96)
            .filter(|&x| (luma(img, x, 48) - 0.5).signum() != (luma(img, x - 1, 48) - 0.5).signum())
            .count()
    };
    let mut downscaled = grey(96, 96, 0.5);
    finish(&mut downscaled, &r, CropRect::whole(96, 96), FilmScale::of(Some(384), 96, 96));
    assert!(
        runs(&downscaled) > runs(&once) * 2,
        "film pixels, not raster pixels: {} vs {}",
        runs(&downscaled),
        runs(&once)
    );

    for tone in [0.0, 1.0] {
        let ends = grey(48, 48, tone);
        let out = finished(ends.clone(), &r, CropPolicy::Cut);
        assert_eq!(out.as_bytes(), ends.as_bytes(), "no grain in pure black or pure white");
    }
}

/// The web preview reaches its frame through the ENGINE's tail, with the
/// policy that keeps the frame whole. Pinned in the source, the house pattern
/// for a choice inside a request handler no offline test drives (the GUI
/// canvas's half of this pin lives in `bin/gui/tests.rs`).
///
/// MUTATION: `CropPolicy::Cut` there, or the old geometry-only chain back, and
/// this names it — a web pane that cropped would answer a different photograph
/// from the one the sliders are moving.
#[test]
fn the_web_preview_finishes_through_the_engines_own_tail() {
    let src = include_str!("../../serve.rs");
    let head = src.find("let mut after = render::develop_preview_film(").expect("preview moved");
    let body = &src[head..head + 1200];
    assert!(
        body.contains("render::frame_and_finish("),
        "the web preview no longer runs the engine's tail: {body}"
    );
    assert!(
        body.contains("render::CropPolicy::Keep"),
        "the web preview must keep the whole frame and only POSITION the finish: {body}"
    );
    assert!(
        !body.contains("render::apply_lens_geometry("),
        "a second copy of the geometry chain is exactly what the tail replaced"
    );
}

#[test]
fn a_recipe_with_neither_effect_leaves_every_byte_alone() {
    let src = flat(40, 30, [0.2, 0.5, 0.8]);
    let out = finished(src.clone(), &EditRecipe::default(), CropPolicy::Keep);
    assert_eq!(out.as_bytes(), src.as_bytes(), "the finishing pass is a no-op at rest");
    // ... and an 8-bit preview takes the same path without a conversion.
    let preview = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(40, 30, |_, _| {
        image::Rgb([51u8, 128, 204])
    }));
    let out = finished(preview.clone(), &EditRecipe::default(), CropPolicy::Keep);
    assert_eq!(out.as_bytes(), preview.as_bytes());
    assert!(matches!(out, DynamicImage::ImageRgb8(_)), "and it stays 8-bit");
}

#[test]
fn an_eight_bit_preview_gets_the_same_vignette_as_the_sixteen_bit_render() {
    let r = vignetted(-70.0);
    let wide = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(64, 64, |_, _| {
        image::Rgb([to_u8(0.5); 3])
    }));
    let eight = finished(wide, &r, CropPolicy::Cut);
    let sixteen = finished(grey(64, 64, 0.5), &r, CropPolicy::Cut);
    for (x, y) in [(0, 0), (32, 32), (63, 10)] {
        assert!(
            (luma(&eight, x, y) - luma(&sixteen, x, y)).abs() < 0.01,
            "({x},{y}): one operator, two sample depths"
        );
    }
}

/// v1.5.0 F6: the FRAME stage runs lens geometry → Transform → straighten, and
/// this pins that order rather than describing it.
///
/// None of the three commutes with either other one, and the differences are
/// not subtle: a keystone applied before a straighten is a keystone whose axis
/// gets turned with the horizon, and a radial distortion applied after a
/// translation pulls on a radius the photograph never had. The stage was wired
/// into `frame_and_finish` with the order stated only in a comment — which is
/// exactly the shape of claim this project has had to correct before.
///
/// The three orders are composed from the engine's OWN operators, so this is
/// not a second implementation that could drift: the test says which
/// composition `frame_and_finish` equals, and shows that the other two differ.
/// Byte equality, because a geometric stage that lands half a pixel off is
/// still a stage in the wrong place.
///
/// MUTATION: swap the Transform and the straighten in `frame_and_finish`, or
/// move the Transform above the lens geometry, and the named order below is the
/// one that stops matching.
#[test]
fn the_frame_stage_runs_the_lens_then_the_transform_then_the_straighten() {
    use crate::render::{apply_lens_geometry, perspective, rotate_straighten};

    // One of each: a radial distortion (centre-fixing), a pure translation
    // (centre-moving) and a rotation (centre-fixing). Every pair of them is
    // non-commuting, and the marker starts at the centre so the translation is
    // what separates the orders.
    let r = EditRecipe {
        lens_distortion: 80.0,
        perspective_x: 100.0,
        straighten_deg: 30.0,
        ..Default::default()
    };
    let geom = crate::recipe::LensProfile::default();
    let src = marked(96);

    let warp = perspective::transform(&r, &src).expect("a full-strength X offset warps");
    let right = rotate_straighten(
        &perspective::apply(&apply_lens_geometry(&src, &geom, r.lens_distortion), warp),
        r.straighten_deg,
    );
    // The two neighbours it could have been written as, each one stage out of
    // place. `transform` is re-solved per frame because its input frame differs
    // — which is itself part of what the order decides.
    let straighten_first = {
        let s = rotate_straighten(&src, r.straighten_deg);
        let g = apply_lens_geometry(&s, &geom, r.lens_distortion);
        let w = perspective::transform(&r, &g).expect("still warps");
        perspective::apply(&g, w)
    };
    let transform_first = {
        let w = perspective::transform(&r, &src).expect("still warps");
        let t = perspective::apply(&src, w);
        rotate_straighten(&apply_lens_geometry(&t, &geom, r.lens_distortion), r.straighten_deg)
    };

    let got = frame_and_finish(src, &r, &geom, FilmScale::NATIVE, CropPolicy::Keep);
    assert_eq!(
        samples(&got),
        samples(&right),
        "the frame stage is lens → Transform → straighten"
    );
    assert_ne!(samples(&got), samples(&straighten_first), "…not straighten first");
    assert_ne!(samples(&got), samples(&transform_first), "…and not Transform before the lens");

    // And the plainest reading of the same order, in one number: the marker
    // starts at the centre, so only the Transform can move it off-centre, and
    // only a straighten AFTERWARDS can take that displacement off the
    // horizontal. Straighten-first would leave it on the centre row.
    let (w, h, _) = samples(&got);
    let mut best = (0.0f32, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            let v = luma(&got, x, y);
            if v > best.0 {
                best = (v, x, y);
            }
        }
    }
    let dy = best.2 as f32 - (h as f32 - 1.0) * 0.5;
    assert!(
        dy.abs() > 6.0,
        "the marker sits {dy:.1} px off the centre row at ({}, {}) — a straighten that ran \
         BEFORE the offset would have left it on the row",
        best.1,
        best.2
    );
}

/// v1.5.0 F6: `crs:CropConstrainToWarp` is a FLAG, and off is what Lightroom
/// actually writes.
///
/// The user ruling (2026-09-17) was to follow Lightroom rather than to pick the
/// prettier default, and the library says 0 on all 52 sidecars that carry the
/// key — so the ordinary photograph keeps its own crop and shows the empty
/// corners a slider left, exactly as those files render in Lightroom. A
/// constraint applied on a flag nobody set would silently re-crop every one of
/// them, which is the failure this pins: the assertion is on the OUTPUT'S OWN
/// SIZE, the one thing a photographer notices immediately.
///
/// `constrain_crop` has its own unit test for the geometry. What could only be
/// tested here is that the flag reaches it at all.
///
/// MUTATION: drop the `.filter(|_| r.crop_constrain_to_warp)` in
/// `frame_and_finish` and the off case starts shrinking too.
#[test]
fn constrain_crop_is_a_flag_and_lightrooms_own_answer_is_off() {
    // The void is WHITE and the picture is mid grey, so counting the fill is
    // counting full-scale pixels. It was counting BLACK until 2026-09-19, which
    // is what `VOID` used to be; the kit measured Lightroom's own fill as white
    // and this test went with it in the same batch. Worth stating because the
    // second count below had quietly become vacuous — nothing in the frame was
    // black any more, so "what it delivers is all picture" was asserting that
    // zero equals zero and would have passed over a frame that was all void.
    const WHITE: [u16; 3] = [u16::MAX; 3];
    // 18, for the slide it produces: `OFFSET` was measured at 0.8121, so this
    // is a 0.146-frame slide. The 60 this used to say now slides the frame by
    // 0.487, which leaves the whole-frame crop's own centre a hair from the void
    // and makes the constrained arm below turn on a rounding error.
    let slid = EditRecipe { perspective_x: 18.0, ..Default::default() };
    let full = finished(grey(96, 96, 0.5), &slid, CropPolicy::Cut);
    assert_eq!(
        (full.width(), full.height()),
        (96, 96),
        "off: the photographer's frame stands and the void shows"
    );
    let void = full.to_rgb16().pixels().filter(|p| p.0 == WHITE).count();
    assert!(void > 96 * 8, "…and the empty corner really is there: {void} px");

    let held = EditRecipe { crop_constrain_to_warp: true, ..slid };
    let cut = finished(grey(96, 96, 0.5), &held, CropPolicy::Cut);
    assert!(
        cut.width() < 96 && cut.height() < 96,
        "on: the crop shrinks until it fits, so the frame comes back smaller: {}x{}",
        cut.width(),
        cut.height()
    );
    assert_eq!(
        cut.to_rgb16().pixels().filter(|p| p.0 == WHITE).count(),
        0,
        "…and what it delivers is all picture"
    );
    // The aspect the photographer set survives the shrink — the whole frame is
    // 1:1 here, so the constrained one has to be too.
    let ratio = cut.width() as f32 / cut.height() as f32;
    assert!((ratio - 1.0).abs() < 0.02, "a square crop stays square: {ratio}");
}
