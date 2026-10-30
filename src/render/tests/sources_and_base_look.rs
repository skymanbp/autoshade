// One part of the engine's tests (src/render/tests.rs includes it): the source dispatch, the baked develop, vignette and base-curve composition, creative profiles, retouch areas, HDR and the camera base look.

/// [`source_pixels`] is the ONE raw-vs-baked dispatch, so both arms and the
/// cap contract are pinned here.
///
/// The RAW arm cannot be exercised end-to-end without a real sensor file
/// (the repo carries no RAW fixture), but the DISPATCH can: a .ARW must
/// reach the develop engine — proven by the failure it produces being the
/// raw decoder's (`decoder_for`'s "does not read as any RAW format"),
/// never `load_image`'s "is a camera RAW, and this step reads finished
/// images" refusal. That refusal appearing here would mean the gate had
/// sent a RAW down the baked arm, which is exactly the v0.22 mask-refine
/// bug. The needle is the refusal's own clause, not the bare "is a camera
/// RAW": the decoder's message says "if it really is a camera RAW" too.
#[test]
fn the_source_dispatch_sends_each_kind_down_its_own_arm() {
    let dir = std::env::temp_dir().join(format!("autoshade-source-px-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let raw = dir.join("fake.arw");
    std::fs::write(&raw, b"not really a raw").unwrap();
    let e = format!("{:#}", source_pixels(&raw, None).unwrap_err());
    assert!(
        !e.contains("is a camera RAW, and this step reads finished images"),
        "a RAW must be DEVELOPED, not sent to the baked decoder: {e}"
    );
    assert!(
        e.contains("does not read as any RAW format"),
        "the failure is the RAW decoder's own: {e}"
    );

    // Baked arm: full resolution when uncapped...
    let big = dir.join("big.png");
    image::RgbImage::from_fn(400, 200, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, 7]))
        .save(&big)
        .unwrap();
    assert_eq!(source_pixels(&big, None).unwrap().dimensions(), (400, 200));
    // ...bounded by the cap's LONG edge, aspect kept...
    assert_eq!(source_pixels(&big, Some(100)).unwrap().dimensions(), (100, 50));
    // ...and NEVER upsampled: `thumbnail` alone inflates a small source,
    // which would hand a heal/denoise/refine consumer invented pixels (and
    // save them as the delivered master).
    let small = dir.join("small.png");
    image::RgbImage::from_pixel(64, 48, image::Rgb([3, 4, 5])).save(&small).unwrap();
    assert_eq!(source_pixels(&small, Some(2048)).unwrap().dimensions(), (64, 48));
    let _ = std::fs::remove_dir_all(&dir);
}

/// R27 P3 — the baked develop takes a working-resolution cap, and obeys
/// the RAW arm's two rules for it: bound the LONG edge, and only ever go
/// DOWN.
///
/// Before this the baked arm had no cap while the RAW arm had one (the
/// format map's B2), so a 60 MP TIFF developed at full size wherever it
/// was developed at all. The downscale-only half matters as much as the
/// bound: plain `thumbnail` UPSCALES a source smaller than the cap, which
/// would invent pixels and then hand them on as a developed master.
///
/// MUTATION THIS CATCHES: use `resize` instead of `thumbnail`'s
/// downscale-only guard and the third assertion inflates the 64×48 source
/// to 2048×1536; cap AFTER the develop instead of before and the first
/// assertion still passes while the memory saving — the entire point —
/// silently disappears, which is why the fourth assertion pins that the
/// tone stage saw the SMALL frame.
#[test]
fn the_baked_develop_caps_its_working_resolution_downward_only() {
    let big = DynamicImage::ImageRgb8(image::RgbImage::from_fn(400, 200, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 7])
    }));
    let r = EditRecipe::default();

    // Uncapped = the source's own resolution (what the export path asks).
    assert_eq!(
        render_baked_to_image(&big, &r, None, None, None, &crate::diag::pixels()).unwrap().dimensions(),
        (400, 200)
    );
    // Capped on the LONG edge, aspect kept.
    assert_eq!(
        render_baked_to_image(&big, &r, None, Some(100), None, &crate::diag::pixels()).unwrap().dimensions(),
        (100, 50)
    );
    // Never upsampled.
    let small = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 48, image::Rgb([3, 4, 5])));
    assert_eq!(
        render_baked_to_image(&small, &r, None, Some(2048), None, &crate::diag::pixels()).unwrap().dimensions(),
        (64, 48)
    );

    // The cap runs BEFORE the develop, not after: a crop is expressed in
    // normalised coordinates on the developed frame, so if the shrink
    // happened last the crop would be taken from the 400-wide frame and
    // then shrunk, landing on 100×50 either way. Ask instead for a frame
    // whose SIZE reveals the order — a half-width crop of a capped
    // develop is 50 px; of an uncapped one shrunk afterwards it would be
    // 200 px before the shrink and the function returns the crop, not the
    // cap, so the two disagree.
    let cropped = EditRecipe {
        crop: Some(crate::recipe::Crop { left: 0.0, top: 0.0, right: 0.5, bottom: 1.0 }),
        ..Default::default()
    };
    assert_eq!(
        render_baked_to_image(&big, &cropped, None, Some(100), None, &crate::diag::pixels()).unwrap().dimensions(),
        (50, 50),
        "the crop must be taken from the CAPPED frame — i.e. the cap ran first"
    );
}

/// Patrol (the `find_raws_accepts_every_raw_format_the_app_can_decode`
/// pattern: ONE predicate, app-wide): every line in the tree that names the
/// baked decoder must say, ON THE LINE, why it is allowed to. A new consumer
/// of *source* pixels must go through [`source_pixels`] instead — the whole
/// point of having one dispatch.
///
/// Per CALL SITE, not per file (R22 M2). The old form asserted a sorted FILE
/// allow-list, so any file already on it could grow a new hand-rolled decode
/// of source pixels and stay green — including `bin/gui/workers.rs`, the
/// exact site of the v0.22 "AI mask refine failed" accident this patrol was
/// written for. Two markers, either on the line or on the line above it:
///
/// * `// baked-by-construction: <why this path can never be a camera RAW>`
///   — a consumer's call.
/// * `// not-a-consumer-call: <why the line is not a consumer at all>` — the
///   gate's own declaration / dispatch / unit tests, and this patrol's own
///   extractor literal. It exists so those lines stay HONEST instead of
///   claiming a baked path they do not have.
///
/// Lexical on purpose: the drift this catches is someone typing
/// `load_image` in a new worker, which no type can prevent (both arms
/// return `DynamicImage`). Known and accepted limit, unchanged from the
/// file-granular form: a `//`-prefixed line is skipped (that is where the
/// doc references live), and a mention inside a string literal is scanned
/// like a call — the marker is then the honest answer.
#[test]
fn every_baked_decode_line_says_why_it_is_allowed() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("source dir listable") {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    files.sort();
    assert!(files.len() >= 20, "only {} source files walked — the patrol is broken", files.len());

    const MARKERS: [&str; 2] = ["baked-by-construction:", "not-a-consumer-call:"];
    let mut scanned = 0usize;
    let mut unmarked: Vec<String> = Vec::new();
    for p in &files {
        let text = std::fs::read_to_string(p).expect("source readable");
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            // Comment lines carry the DOC references, which are exactly what
            // this patrol must not count.
            // not-a-consumer-call: the patrol's own extractor literal.
            if line.trim_start().starts_with("//") || !line.contains("load_image") {
                continue;
            }
            scanned += 1;
            let above = i.checked_sub(1).map(|j| lines[j]).unwrap_or("");
            if MARKERS.iter().any(|m| line.contains(m) || above.contains(m)) {
                continue;
            }
            unmarked.push(format!(
                "{}:{}: {}",
                p.strip_prefix(&src).unwrap_or(p).to_string_lossy().replace('\\', "/"),
                i + 1,
                line.trim()
            ));
        }
    }
    assert!(scanned >= 20, "only {scanned} lines scanned — the extractor is broken");
    assert!(
        unmarked.is_empty(),
        "these lines name the baked decoder with no reason on them — route the source through \
         render::source_pixels, or mark the line `// baked-by-construction: <why>` (a path that \
         cannot be a RAW) / `// not-a-consumer-call: <why>`:\n{}",
        unmarked.join("\n")
    );
}

/// L01-6: two active vignette stages compose into ONE clamped pass — the
/// per-pass clamp made mathematically inverse corrections irreversible on
/// bright pixels.
#[test]
fn opposing_vignette_stages_compose_into_one_clamped_pass() {
    let knots = vec![2.0f32; 4];
    let p_lut = profile_vignette_lut(&knots, 100.0);
    let m_lut = manual_vignette_lut(-50.0, 50.0);
    let r = EditRecipe {
        lens_vignette: -50.0,
        lens_profile: crate::recipe::LensProfile {
            vignette: knots,
            vignette_on: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let lut = vignette_gain_lut(&r).expect("both stages active");
    let last = *lut.last().unwrap();
    assert!(
        (last - p_lut.last().unwrap() * m_lut.last().unwrap()).abs() < 1e-6,
        "the composed LUT is the product of the stage gains"
    );
    assert!((last - 1.0).abs() < 1e-3, "2.0 × 0.5 cancels at the corner: {last}");

    // A bright corner pixel survives the composed pass untouched, where
    // the clamped two-pass order permanently darkened it. 9×9: the
    // radial geometry floors rmax at one PIXEL, so a frame this small
    // is needed for the corner to actually reach rn = 1.0.
    let mut composed = vec![[0.9f32; 3]; 81];
    apply_radial_gain(&mut composed, 9, 9, &lut);
    let mut two_pass = vec![[0.9f32; 3]; 81];
    apply_radial_gain(&mut two_pass, 9, 9, &p_lut);
    apply_radial_gain(&mut two_pass, 9, 9, &m_lut);
    assert!((composed[0][0] - 0.9).abs() < 1e-3, "composed: {}", composed[0][0]);
    assert!(two_pass[0][0] < 0.85, "the clamp loses the highlight: {}", two_pass[0][0]);
}

#[test]
fn base_curve_composes_under_user_tone_and_is_not_skipped() {
    // Mid-grey through a lifting base curve must brighten even with a
    // fully-neutral user recipe — the neutral short-circuit must treat the
    // base look as tone work — and user exposure must act ON TOP of it.
    let base =
        DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([100, 100, 100])));
    let mut lifted = EditRecipe {
        base_curve: vec![[0.0, 0.0], [0.4, 0.62], [1.0, 1.0]],
        ..Default::default()
    };
    let neutral_px = develop_preview(&base, &EditRecipe::default()).to_rgb8()[(0, 0)][0];
    assert_eq!(neutral_px, 100, "a truly neutral recipe is the identity");
    let lifted_px = develop_preview(&base, &lifted).to_rgb8()[(0, 0)][0];
    assert!(lifted_px > 130, "base curve must lift mid-grey: {lifted_px}");
    lifted.exposure_ev = 1.0;
    let more_px = develop_preview(&base, &lifted).to_rgb8()[(0, 0)][0];
    assert!(more_px > lifted_px, "exposure composes on top of the base look: {more_px}");
    // Endpoints stay pinned through the composition: black in → black out.
    let black = DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 0])));
    lifted.exposure_ev = 0.0;
    assert_eq!(develop_preview(&black, &lifted).to_rgb8()[(0, 0)][0], 0);
}

/// v1.5.0 F7, END TO END: a creative profile's baked half reaches the
/// PIXELS, in the direction the profile states.
///
/// This is the claim the whole batch rests on and the one the unit tests
/// underneath it cannot make. `render::build_tone_lut`,
/// `render::apply_rgb_curves` and `EditRecipe::with_baked` each read the
/// Look, and every one of them could have been written correctly while
/// nothing called them — a develop panel that shows a profile's name over
/// an unchanged photograph is exactly the 「a slider that moves a number
/// and no pixel」 failure this file's rules call the worst kind here.
///
/// The curve is `Adobe Color`'s own, measured off the sidecars: 161 of the
/// 175 in the reference library carry a Look and 152 of those are this one,
/// so this S is what almost every Lightroom photograph is developed
/// through. It darkens the low end (22→16, 40→35) and lifts the high end
/// (224→230, 240→246) about a pinned midpoint (127→127).
///
/// MUTATION: drop any of the four `baked_look`/`with_baked` reads in this
/// file, or let `amount` scale nothing.
#[test]
fn a_baked_creative_profile_reaches_the_pixels_in_its_own_direction() {
    use crate::recipe::{CreativeLook, CurvePoint};
    let knot = |input, output| CurvePoint { input, output };
    let adobe_color = || CreativeLook {
        name: "Adobe Color".to_string(),
        amount: 1.0,
        tone_curve: vec![
            knot(0, 0),
            knot(22, 16),
            knot(40, 35),
            knot(127, 127),
            knot(224, 230),
            knot(240, 246),
            knot(255, 255),
        ],
        ..Default::default()
    };
    let patch = |v: u8| DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([v; 3])));
    let shot = |img: &DynamicImage, r: &EditRecipe| develop_preview(img, r).to_rgb8()[(0, 0)][0];

    let bare = EditRecipe::default();
    let profiled = EditRecipe { look: Some(adobe_color()), ..Default::default() };

    // The two ends move the way the curve says, and the pinned middle does
    // not move at all — which is what makes this the curve's shape rather
    // than a brightness offset that happens to be signed correctly.
    let (dark_bare, dark_profiled) = (shot(&patch(40), &bare), shot(&patch(40), &profiled));
    let (lit_bare, lit_profiled) = (shot(&patch(224), &bare), shot(&patch(224), &profiled));
    let (mid_bare, mid_profiled) = (shot(&patch(127), &bare), shot(&patch(127), &profiled));
    assert!(dark_profiled < dark_bare, "40→35 must darken: {dark_bare} → {dark_profiled}");
    assert!(lit_profiled > lit_bare, "224→230 must lift: {lit_bare} → {lit_profiled}");
    assert!(
        mid_profiled.abs_diff(mid_bare) <= 1,
        "127→127 is pinned: {mid_bare} → {mid_profiled}"
    );

    // `crs:Amount` scales the baked half, so half of it lands strictly
    // between doing nothing and doing all of it.
    let half = EditRecipe {
        look: Some(CreativeLook { amount: 0.5, ..adobe_color() }),
        ..Default::default()
    };
    let dark_half = shot(&patch(40), &half);
    assert!(
        dark_profiled < dark_half && dark_half < dark_bare,
        "amount 0.5 sits between: {dark_bare} / {dark_half} / {dark_profiled}"
    );

    // The baked SLIDERS add to the photographer's own rather than replacing
    // them (`EditRecipe::with_baked`): the same −40 shadows pulls further
    // down when the profile bakes −40 too.
    let img = patch(90);
    let own = EditRecipe { shadows: -40.0, ..Default::default() };
    let both = EditRecipe {
        shadows: -40.0,
        look: Some(CreativeLook { shadows: -40.0, ..adobe_color() }),
        ..Default::default()
    };
    assert!(
        shot(&img, &both) < shot(&img, &own),
        "a baked shadow move ADDS: {} vs {}",
        shot(&img, &both),
        shot(&img, &own)
    );

    // …and a monochrome profile develops to grey with the photographer's
    // own Black & White switch still off.
    let mono = EditRecipe {
        look: Some(CreativeLook {
            name: "Adobe Monochrome".to_string(),
            grayscale: true,
            ..adobe_color()
        }),
        ..Default::default()
    };
    assert!(!mono.convert_to_grayscale, "the photographer never touched the switch");
    let colourful =
        DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([200, 60, 40])));
    let px = develop_preview(&colourful, &mono).to_rgb8()[(0, 0)];
    let (hi, lo) = (px[0].max(px[1]).max(px[2]), px[0].min(px[1]).min(px[2]));
    assert!(hi - lo <= 1, "a monochrome profile must develop grey: {px:?}");
    // The control: the very same frame through the colour profile is not.
    let colour_px = develop_preview(&colourful, &profiled).to_rgb8()[(0, 0)];
    assert!(
        colour_px[0] > colour_px[2] + 20,
        "…and the colour profile leaves it red: {colour_px:?}"
    );
}

/// v1.5.0 F9, end to end: an area the photographer removed in Lightroom
/// comes off the photograph here too.
///
/// This test exists because F7 taught the lesson at cost — two shipped
/// defects that each masked the other, invisible to every unit test,
/// because the only thing that could see them was a photograph going in
/// one end and coming out the other. Both retouch geometries go through
/// the public develop entry point here, for the same reason.
///
/// `feather: 0.0` on purpose: it makes the healed disc's edge crisp, so
/// "was the radius converted" is a question about which pixels changed
/// rather than about how far a ramp reached.
///
/// MUTATION: drop the `heal_planar` call; drop `to_short` from the radius
/// (the 7.5 px disc leaves the blob's rim behind); rasterise the brush at
/// the wrong frame size.
#[test]
fn a_lightroom_retouch_area_takes_the_object_off_the_photograph() {
    use crate::retouch::{RetouchArea, RetouchShape, SpotOrigin};
    // 160x120 flat grey with a black disc of radius 8 px at (40, 60).
    let (w, h) = (160u32, 120u32);
    let mut src = RgbImage::from_pixel(w, h, image::Rgb([140u8; 3]));
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - 40.0, y as f32 - 60.0);
            if dx * dx + dy * dy <= 64.0 {
                src.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
    }
    let img = DynamicImage::ImageRgb8(src);
    // The darkest pixel anywhere in the blob, and a far-field control.
    // Stated against the SAME rendition's own background rather than
    // against an absolute code value, so the assertion does not depend on
    // what the rest of the develop chain does to a flat grey.
    let darkest = |d: &DynamicImage| {
        let rgb = d.to_rgb8();
        let mut m = 255u8;
        for y in 51..70u32 {
            for x in 31..50u32 {
                let (dx, dy) = (x as f32 - 40.0, y as f32 - 60.0);
                if dx * dx + dy * dy <= 64.0 {
                    m = m.min(rgb[(x, y)][0]);
                }
            }
        }
        m
    };
    let far = |d: &DynamicImage| d.to_rgb8()[(130, 25)][0];

    // The control: an empty recipe leaves the object exactly where it is.
    let bare = develop_preview(&img, &EditRecipe::default());
    assert!(
        darkest(&bare) + 80 < far(&bare),
        "the blob must survive an empty recipe: {} vs {}",
        darkest(&bare),
        far(&bare)
    );

    // ELLIPSE. `size_x` is a half-extent in WIDTH units — 10 px of 160 —
    // which is 10 px of the 120 px short side once converted, and so
    // covers the 8 px blob. Without the conversion it is 7.5 px and the
    // rim survives.
    let ellipse = RetouchArea {
        origin: SpotOrigin::LightroomContentAware,
        feather: 0.0,
        donor: None,
        shape: RetouchShape::Ellipse {
            cx: 40.0 / 160.0,
            cy: 60.0 / 120.0,
            size_x: 10.0 / 160.0,
            size_y: 10.0 / 160.0,
        },
    };
    let healed = develop_preview(
        &img,
        &EditRecipe { retouch: vec![ellipse.clone()], ..Default::default() },
    );
    assert!(
        darkest(&healed) + 20 > far(&healed),
        "every pixel of the area must heal to its surroundings: {} vs {}",
        darkest(&healed),
        far(&healed)
    );

    // BRUSH — the other 37 of the library's 121 areas. One dab at the same
    // place, the same width-unit radius, through the mask side's own
    // rasteriser and the retouch side's own planner.
    let brush = RetouchArea {
        shape: RetouchShape::Brush(vec![crate::recipe::BrushStroke {
            value: 1.0,
            radius: 10.0 / 160.0,
            flow: 1.0,
            center_weight: 1.0,
            sync_id: String::new(),
            dabs: "d 0.250000 0.500000".to_string(),
        }]),
        ..ellipse.clone()
    };
    let brushed = develop_preview(
        &img,
        &EditRecipe { retouch: vec![brush], ..Default::default() },
    );
    assert!(
        darkest(&brushed) + 20 > far(&brushed),
        "a brush-shaped area heals too: {} vs {}",
        darkest(&brushed),
        far(&brushed)
    );
}

/// v1.5.0 F9: a retouch area turns with the frame, and comes home.
///
/// Lightroom states these coordinates in the UN-ROTATED SENSOR frame, like
/// a crop rectangle and like a brush dab, and this is not a corner case —
/// 7 of the 25 retouched photographs in the reference library are
/// `tiff:Orientation="8"`. An area left un-turned is repaired a quarter
/// turn from the object: a clean patch of sky rewritten and the power line
/// still there.
///
/// A round trip, plus the assertion that makes a round trip mean
/// something — that the first turn MOVED it. Without that, deleting the
/// whole arm passes this test twice over.
///
/// MUTATION: drop either arm of the `r.retouch` loop; turn the centre but
/// not the half-extents; hand the second turn the un-swapped frame.
#[test]
fn a_retouch_area_turns_with_the_frame_and_comes_home() {
    use crate::recipe::BrushStroke;
    use crate::retouch::{RetouchArea, RetouchShape, SpotOrigin};
    use rawler::Orientation;
    // 3:2, so a quarter turn really changes which edge "width" names and a
    // half-extent that rode through unscaled would come back 1.5x wrong.
    let area = |shape| RetouchArea {
        origin: SpotOrigin::LightroomContentAware,
        feather: 0.5,
        donor: Some([0.6, 0.4]),
        shape,
    };
    let mut r = EditRecipe {
        retouch: vec![
            area(RetouchShape::Ellipse { cx: 0.25, cy: 0.5, size_x: 0.05, size_y: 0.05 }),
            area(RetouchShape::Brush(vec![BrushStroke {
                value: 1.0,
                radius: 0.05,
                flow: 1.0,
                center_weight: 1.0,
                sync_id: String::new(),
                dabs: "d 0.250000 0.500000".to_string(),
            }])),
        ],
        ..Default::default()
    };
    let before = r.retouch.clone();

    orient_recipe_coords(&mut r, Orientation::Rotate90, CoordFrame::new(9504.0, 6336.0));
    assert_ne!(r.retouch, before, "a quarter turn must MOVE a retouch area");
    // Both arms moved, not just the one the loop happens to reach first.
    match (&before[0].shape, &r.retouch[0].shape) {
        (
            RetouchShape::Ellipse { cx: c0, size_x: s0, .. },
            RetouchShape::Ellipse { cx: c1, size_x: s1, .. },
        ) => {
            assert_ne!(c0, c1, "the ellipse's centre turns");
            assert_ne!(s0, s1, "…and its half-extent rescales with the new width");
        }
        ref other => panic!("two ellipses, got {other:?}"),
    }
    match (&before[1].shape, &r.retouch[1].shape) {
        (RetouchShape::Brush(b0), RetouchShape::Brush(b1)) => {
            assert_ne!(b0[0].dabs, b1[0].dabs, "the brush's dabs turn too");
        }
        ref other => panic!("two brushes, got {other:?}"),
    }
    assert_ne!(before[0].donor, r.retouch[0].donor, "and so does the donor point");

    // Home again, through the TURNED frame — the quarter turns are each
    // other's inverse, so the second one is handed 6336x9504.
    orient_recipe_coords(&mut r, Orientation::Rotate270, CoordFrame::new(6336.0, 9504.0));
    // A tolerance, not equality: the half-extent is multiplied by 1.5 and
    // then by its reciprocal in f32, which is exact for the dab grid the
    // writer rounds to and within a millionth here.
    for (b, a) in before.iter().zip(&r.retouch) {
        match (&b.shape, &a.shape) {
            (
                RetouchShape::Ellipse { cx: x0, cy: y0, size_x: s0, size_y: t0 },
                RetouchShape::Ellipse { cx: x1, cy: y1, size_x: s1, size_y: t1 },
            ) => {
                for (was, now, what) in
                    [(x0, x1, "cx"), (y0, y1, "cy"), (s0, s1, "size_x"), (t0, t1, "size_y")]
                {
                    assert!(
                        (was - now).abs() < 1e-6,
                        "{what} must come home: {was} → {now}"
                    );
                }
            }
            (RetouchShape::Brush(b0), RetouchShape::Brush(b1)) => {
                assert_eq!(b0[0].dabs, b1[0].dabs, "the dab stream comes home verbatim");
                assert!((b0[0].radius - b1[0].radius).abs() < 1e-6, "so does its radius");
            }
            ref other => panic!("same shapes both ways, got {other:?}"),
        }
        let (Some(d0), Some(d1)) = (b.donor, a.donor) else { panic!("both donors survive") };
        assert!(
            (d0[0] - d1[0]).abs() < 1e-6 && (d0[1] - d1[1]).abs() < 1e-6,
            "the donor comes home: {d0:?} → {d1:?}"
        );
    }
}

/// v1.5.0 F9: the two unit conversions `retouch_spots` owns, pinned as
/// numbers rather than inferred from a rendition.
///
/// Both are places where Lightroom and this engine mean different things
/// by the same word, and both are silent when wrong — a radius off by the
/// aspect ratio still heals SOMETHING, and a donor read as an offset still
/// copies SOME pixels.
///
/// MUTATION: normalise the radius by the long side or by width; read the
/// donor without subtracting the spot centre.
#[test]
fn a_retouch_areas_units_become_this_engines_at_exactly_one_place() {
    use crate::retouch::{RetouchArea, RetouchShape, SpotOrigin};
    let r = EditRecipe {
        retouch: vec![
            RetouchArea {
                origin: SpotOrigin::LightroomHeal,
                feather: 0.5,
                // ABSOLUTE, whatever `crs:OffsetY` sounds like.
                donor: Some([0.60, 0.40]),
                shape: RetouchShape::Ellipse {
                    cx: 0.25,
                    cy: 0.50,
                    size_x: 0.05,
                    size_y: 0.05,
                },
            },
            // A BRUSH area, which reaches the spots by the other road —
            // rasterised, then planned by connected components. The sweep
            // is what put it here: the planner takes everything except the
            // geometry from a `HealSpot` template, and with only an
            // ellipse in this test that template could be replaced by
            // `Default::default()` with nothing going red.
            RetouchArea {
                origin: SpotOrigin::LightroomGenerative,
                feather: 0.25,
                donor: None,
                shape: RetouchShape::Brush(vec![crate::recipe::BrushStroke {
                    value: 1.0,
                    radius: 0.05,
                    flow: 1.0,
                    center_weight: 1.0,
                    sync_id: String::new(),
                    dabs: "d 0.750000 0.500000".to_string(),
                }]),
            },
        ],
        ..Default::default()
    };
    // A 160x120 frame: width 160, short side 120, so a width-unit
    // half-extent of 0.05 (8 px) is 8/120 of the short side.
    let spots = super::retouch_spots(&r, 160, 120);
    assert_eq!(spots.len(), 2, "one spot from the ellipse, one from the stroke");
    // The brush's spot carries what the RASTER cannot say.
    assert_eq!(
        spots[1].origin,
        SpotOrigin::LightroomGenerative,
        "a planned spot keeps its area's provenance"
    );
    assert_eq!(spots[1].feather, 0.25, "…and its area's feather");
    assert!(spots[1].coverage.is_some(), "…and the stroke's exact shape");
    assert!(
        (spots[0].radius - 8.0 / 120.0).abs() < 1e-6,
        "0.05 of WIDTH is 8 px is {} of the short side, got {}",
        8.0 / 120.0,
        spots[0].radius
    );
    let source = spots[0].source.expect("a stated donor survives");
    assert!(
        (source[0] - 0.35).abs() < 1e-6 && (source[1] + 0.10).abs() < 1e-6,
        "an absolute donor becomes an offset from the spot centre: got {source:?}"
    );
    assert_eq!(spots[0].feather, 0.5, "the area's own feather rides through");
    assert_eq!(spots[0].origin, SpotOrigin::LightroomHeal, "so does its provenance");
}

/// v1.5.0 F8: the HDR switch is a GATE, and the SDR rendition really acts.
///
/// Both halves in one test, because each is worthless without the other.
/// A gate that is always shut renders nothing and passes "the seven do not
/// act while the mode is off"; a rendition with no gate passes "the seven
/// act" and re-tones every SDR photograph in the archive. So: the same
/// seven values, twice, and the ONLY difference is the switch.
///
/// MUTATION: drop the `!r.hdr_edit` early return in `SdrRendition::global`
/// (the gated arm stops matching the bare one); drop the `hdr::apply` call
/// at the end of the chain (the HDR arm stops differing).
#[test]
fn hdr_edit_mode_gates_the_sdr_rendition_and_the_rendition_renders() {
    // A vertical ramp: every code value present, so a tone move anywhere
    // in the range has somewhere to show.
    let (w, h) = (64u32, 256u32);
    let mut src = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = y as u8;
            src.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    let img = DynamicImage::ImageRgb8(src);
    let sdr = EditRecipe {
        sdr_brightness: 40.0,
        sdr_contrast: 30.0,
        sdr_highlights: -50.0,
        sdr_shadows: 25.0,
        sdr_whites: -20.0,
        sdr_clarity: 20.0,
        ..Default::default()
    };
    let bare = develop_preview(&img, &EditRecipe::default());
    let gated = develop_preview(&img, &sdr);
    assert_eq!(
        bare.to_rgb8().as_raw(),
        gated.to_rgb8().as_raw(),
        "seven SDR controls outside HDR edit mode must not move one pixel"
    );
    let on = develop_preview(&img, &EditRecipe { hdr_edit: true, ..sdr.clone() });
    assert_ne!(
        bare.to_rgb8().as_raw(),
        on.to_rgb8().as_raw(),
        "…and inside it they must move the photograph"
    );
    // The direction, so a rendition that merely perturbs the frame cannot
    // pass: +40 Brightness on a mid ramp value lifts it.
    let mid = |d: &DynamicImage| d.to_rgb8()[(32, 96)][0];
    assert!(
        mid(&on) > mid(&bare),
        "+40 SDR Brightness lifts the midtone: {} → {}",
        mid(&bare),
        mid(&on)
    );
    // …and Brightness ALONE lifts it. The arm above moves five sliders at
    // once, so it cannot see which one did the lifting: deleting
    // Brightness entirely left Contrast and Shadows holding the midtone up
    // and the assertion green (falsification case F8-M6). One slider, one
    // probe.
    let bright = develop_preview(
        &img,
        &EditRecipe { hdr_edit: true, sdr_brightness: 40.0, ..Default::default() },
    );
    assert!(
        mid(&bright) > mid(&bare),
        "SDR Brightness is this rendition's exposure and must act by itself: {} → {}",
        mid(&bare),
        mid(&bright)
    );
}

/// v1.5.0 F8: the HEADROOM alone renders, with every SDR slider at 0, and
/// it renders as a shoulder — the highlights come down, the shadows do not.
///
/// A headroom that reached the picture as an exposure cut, or as contrast,
/// would fail the second assertion; one that reached it as a highlights
/// LIFT would fail the first. Those four properties are the shoulder's
/// contract and they held across the 2026-09-19 change of mechanism, from a
/// negative Highlights push to the headroom's own curve — which is what a
/// contract is for.
///
/// MUTATION: flip the shoulder's sign; drop `hdr_max_ev` from
/// `hdr::shoulder`'s white point; drop the blend term (the two headroom
/// arms converge).
#[test]
fn the_hdr_headroom_reaches_the_picture_as_a_shoulder() {
    let (w, h) = (64u32, 256u32);
    let mut src = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = y as u8;
            src.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    let img = DynamicImage::ImageRgb8(src);
    let at = |d: &DynamicImage, y: u32| d.to_rgb8()[(32, y)][0] as i32;
    let bare = develop_preview(&img, &EditRecipe::default());
    let one = develop_preview(
        &img,
        &EditRecipe { hdr_edit: true, hdr_max_ev: 1.0, ..Default::default() },
    );
    assert!(
        at(&one, 230) < at(&bare, 230),
        "one stop of headroom pulls the highlights down: {} → {}",
        at(&bare, 230),
        at(&one, 230)
    );
    assert!(
        (at(&one, 20) - at(&bare, 20)).abs() <= 1,
        "…and leaves the shadows where they were: {} → {}",
        at(&bare, 20),
        at(&one, 20)
    );
    // Blend scales it, and in the stated direction: −100 is no shoulder at
    // all, +100 is twice one.
    let off = develop_preview(
        &img,
        &EditRecipe {
            hdr_edit: true,
            hdr_max_ev: 1.0,
            sdr_blend: -100.0,
            ..Default::default()
        },
    );
    assert_eq!(
        bare.to_rgb8().as_raw(),
        off.to_rgb8().as_raw(),
        "SDRBlend −100 means the headroom does not reach the rendition at all"
    );
    let twice = develop_preview(
        &img,
        &EditRecipe {
            hdr_edit: true,
            hdr_max_ev: 1.0,
            sdr_blend: 100.0,
            ..Default::default()
        },
    );
    assert!(
        at(&twice, 230) < at(&one, 230),
        "…and +100 reaches further than 0: {} vs {}",
        at(&one, 230),
        at(&twice, 230)
    );
}

/// v1.5.0 F8: the headroom keeps reaching at every headroom the sidecar can
/// state — it is not governed by a taste control's limiter.
///
/// The defect this pins, measured off the kit's `HDR-ON` on 2026-09-19:
/// the shoulder used to be a negative Highlights push, so it inherited
/// [`limit_tone_sliders`], whose rule is that a slider must SATURATE and
/// never annihilate a tonal band. Correct for a slider a photographer
/// drags; wrong for a rendering transform. The shoulder stopped growing
/// partway up the headroom range and reached 55 % of Lightroom's, and no
/// larger `crs:HDRMaxValue` could recover the rest.
///
/// So this asserts the DEPTH at the headroom the kit itself states, which
/// is the quantity that separated the two models. At `crs:HDRMaxValue` of
/// +2.30 the old model reached 0.104 below the diagonal at input 0.90 where
/// Lightroom reaches 0.189; the curve reaches 0.193. On this ramp that is
/// about 27 code values against about 50, so a floor of 40 sits clear of
/// both. Direction alone does not catch this — the capped model came down
/// too, just not far enough, which is how it passed its tests for a whole
/// batch.
///
/// Each further stop still reaches further, but by LESS, and that is the
/// curve's own form rather than a cap: extended Reinhard tends to
/// `L/(1+L)` as its white point grows, so headroom has diminishing returns
/// on an input that is itself bounded. This test asserted the opposite when
/// it was first written and the measurement corrected it.
///
/// MUTATION: clamp `stops` in `SdrRendition::global` to 1.0, or route the
/// shoulder back through the highlights slot — the depth falls under 40.
#[test]
fn the_headroom_keeps_reaching_past_where_a_slider_would_saturate() {
    let (w, h) = (64u32, 256u32);
    let mut src = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = y as u8;
            src.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    let img = DynamicImage::ImageRgb8(src);
    // y = 230 of a 0..255 ramp: high enough to be the shoulder's business,
    // far enough from 255 that clipping is not doing the work.
    let top = |ev: f32| {
        develop_preview(
            &img,
            &EditRecipe { hdr_edit: true, hdr_max_ev: ev, ..Default::default() },
        )
        .to_rgb8()[(32, 230)][0] as i32
    };
    let bare = top(0.0);
    let (d1, d2, d4) = (bare - top(1.0), bare - top(2.0), bare - top(4.0));
    assert!(
        0 < d1 && d1 < d2 && d2 < d4,
        "every extra stop of headroom must pull the top end further: \
         {bare} → −{d1} at 1 EV, −{d2} at 2 EV, −{d4} at 4 EV"
    );
    // The kit's own headroom, and the depth Lightroom was measured to reach
    // there. The limiter held the old model to about 27 on this ramp.
    let stated = bare - top(2.30);
    assert!(
        stated >= 40,
        "at the +2.30 EV the kit states, the shoulder must reach Lightroom's \
         own depth and not a slider limiter's: {bare} → −{stated}"
    );
}

/// v1.5.0 F8: the SDR rendition's Clarity is the Basic panel's Clarity —
/// one operator at one radius, not a second copy.
///
/// The radius rule used to be written twice (`0.02 · min(w,h)`), which is
/// how one of the two would have been tuned and the other left behind.
/// `clarity_radius` is now the single definition, and this is the probe
/// that keeps it single: the same amount through either door has to make
/// the same picture.
///
/// MUTATION: give `hdr::apply` its own radius expression again (any
/// constant but `clarity_radius`'s) and the two renditions diverge.
#[test]
fn the_sdr_renditions_clarity_is_the_basic_panels_clarity() {
    // 512 px, and that size is load-bearing. `clarity_radius` is
    // `0.02·min(w,h)` with a FLOOR of 8, so on the 128 px frame this test
    // first used the rule never bound — 0.02·128 = 2.56 and 0.03·128 =
    // 3.84 both clamp to 8, and a mutated coefficient was invisible
    // (falsification case F8-M7). At 512 the floor is behind us: 10 px
    // against 15.
    let (w, h) = (512u32, 512u32);
    let mut src = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = if (x / 32 + y / 32) % 2 == 0 { 90u8 } else { 165u8 };
            src.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    let img = DynamicImage::ImageRgb8(src);
    let basic = develop_preview(&img, &EditRecipe { clarity: 60.0, ..Default::default() });
    let through_sdr = develop_preview(
        &img,
        &EditRecipe { hdr_edit: true, sdr_clarity: 60.0, ..Default::default() },
    );
    assert_eq!(
        basic.to_rgb8().as_raw(),
        through_sdr.to_rgb8().as_raw(),
        "one Clarity operator at one radius, whichever panel asks for it"
    );
    // …and it is not the identity, so the equality above is not two
    // untouched frames agreeing with each other.
    let bare = develop_preview(&img, &EditRecipe::default());
    assert_ne!(
        bare.to_rgb8().as_raw(),
        basic.to_rgb8().as_raw(),
        "Clarity +60 must actually do something to a checkerboard"
    );
}

/// v1.5.0 F7, the class sweep's SECOND site: a creative profile's
/// per-channel curves reach the pixels, and they compose UNDER the
/// photographer's own.
///
/// This is the arm no real photograph exercises. Adobe writes all three
/// channels of every one of the 161 measured Looks as identity
/// (`0,0 … 255,255`), which `parse_curve_checked` returns EMPTY, so
/// `apply_rgb_curves`'s composition branch has never run on a file. The
/// branch was still wrong — it indexed a 256-entry `curve_lut` with this
/// table's `LUT_N` = 4096 counter, the same out-of-bounds the master
/// curve's arm had — and the 29-mutation sweep is what said so: M27 put
/// the defect back and every one of the 1,593 tests stayed green. A fix
/// nothing can fail is a fix nobody can keep.
///
/// MUTATION: index `base[i]` instead of `sample_lut(&base, x)` (the arm
/// panics); compose the two curves in the other order, or drop either one
/// (the three-way ordering below stops holding).
#[test]
fn a_baked_per_channel_curve_reaches_the_pixels_under_the_photographers_own() {
    use crate::recipe::{CreativeLook, CurvePoint};
    let knot = |input, output| CurvePoint { input, output };
    // RED only, so green is an in-frame control for "did this touch the
    // channels it was not given?".
    //
    // The two curves move the SAME input in opposite directions, and they
    // are deliberately NOT each other's inverse. The first draft made them
    // inverses (128→64 against 64→128) and the mutation that swaps the
    // composition order stayed GREEN: where two curves undo each other,
    // `own(baked(x))` and `baked(own(x))` both land back on x, so the
    // probe was blind to the very order it claimed to pin. These two share
    // the knot at 128 instead, which makes the two orders land on opposite
    // sides of where the pixel started.
    let darker = vec![knot(0, 0), knot(128, 64), knot(255, 255)];
    let lighter = vec![knot(0, 0), knot(128, 192), knot(255, 255)];
    let baked = |c: &[CurvePoint]| CreativeLook {
        name: "per-channel".to_string(),
        amount: 1.0,
        red_curve: c.to_vec(),
        ..Default::default()
    };
    let patch = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([128; 3])));
    let shot = |r: &EditRecipe| {
        let p = develop_preview(&patch, r).to_rgb8()[(0, 0)];
        (p[0], p[1])
    };

    let (bare_r, bare_g) = shot(&EditRecipe::default());

    // 1) The PROFILE's curve lands at all, on red alone — the half that
    //    was unreachable before F7 read a Look's per-channel curves.
    let (prof_r, prof_g) =
        shot(&EditRecipe { look: Some(baked(&darker)), ..Default::default() });
    assert!(prof_r < bare_r, "a baked 128→64 must darken red: {bare_r} → {prof_r}");
    assert_eq!(prof_g, bare_g, "…and must leave green alone: {bare_g} → {prof_g}");

    // 2) The PHOTOGRAPHER's own curve, on its own, the other way.
    let (own_r, _) =
        shot(&EditRecipe { red_curve: lighter.clone(), ..Default::default() });
    assert!(own_r > bare_r, "a photographer's 64→128 must lift red: {bare_r} → {own_r}");

    // 3) COMPOSITION, in the ORDER the engine claims: the photographer's
    //    curve reads the PROFILE's output, not the raw pixel.
    //
    //    The profile darkens 128 to ~64; the photographer's curve then
    //    reads 64, which it lifts only part of the way, so the composition
    //    lands BELOW where the pixel started. Swap the two and the
    //    photographer's curve reads 128 and lifts it to ~192, which the
    //    profile then darkens to ~160 — ABOVE where it started. One
    //    ordering assertion separates them, and it is stated against
    //    `bare` rather than as an exact value so it does not rest on where
    //    in the encoding this stage happens to sit.
    let (both_r, both_g) = shot(&EditRecipe {
        look: Some(baked(&darker)),
        red_curve: lighter.clone(),
        ..Default::default()
    });
    assert!(
        prof_r < both_r && both_r < bare_r,
        "the photographer's curve reads the profile's OUTPUT, so the pair lands \
         BELOW bare: darker={prof_r}, composed={both_r}, bare={bare_r}, lighter={own_r}"
    );
    assert_eq!(both_g, bare_g, "green is still untouched: {bare_g} → {both_g}");
}

#[test]
fn camera_base_knots_recovers_the_map_and_identity_is_empty() {
    // neutral = a luma gradient; camera = the SAME frame through a known
    // pointwise lift (x^0.6). The CDF match must recover that map at the
    // interior knots, and a self-match must collapse to empty (no-op).
    let (w, h) = (512u32, 64u32);
    let mut n = RgbImage::new(w, h);
    let mut c = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / (w - 1) as f32;
            let v = (t * 255.0).round() as u8;
            n.put_pixel(x, y, image::Rgb([v, v, v]));
            let l = (t.powf(0.6) * 255.0).round() as u8;
            c.put_pixel(x, y, image::Rgb([l, l, l]));
        }
    }
    let n = DynamicImage::ImageRgb8(n);
    let c = DynamicImage::ImageRgb8(c);
    let knots =
        camera_base_knots(&n, &c).expect("512x64 holds 512 blocks of the 64-column grid");
    assert!(!knots.is_empty(), "a real lift must be detected");
    assert_eq!(knots.first(), Some(&[0.0, 0.0]), "black endpoint pinned");
    assert_eq!(knots.last(), Some(&[1.0, 1.0]), "white endpoint pinned");
    for p in &knots {
        if p[0] > 0.05 && p[0] < 0.95 {
            assert!(
                (p[1] - p[0].powf(0.6)).abs() < 0.04,
                "knot {p:?} vs expected {}",
                p[0].powf(0.6)
            );
        }
    }
    assert!(
        camera_base_knots(&n, &n).expect("same pair — still judgeable").is_empty(),
        "identity map → Some(empty) (no base look)"
    );
    // Degenerate input is an INABILITY, not a verdict: the pre-era repair
    // keys on exactly this distinction (None retries later; Some(empty)
    // clears a saved curve and stamps the era).
    let tiny =
        DynamicImage::ImageRgb8(RgbImage::from_pixel(50, 50, image::Rgb([128, 128, 128])));
    assert!(
        camera_base_knots(&tiny, &tiny).is_none(),
        "a picture smaller than the block grid is an inability, not an identity verdict"
    );
}

#[test]
fn base_curve_bridges_histogram_gaps_without_plateaus_or_white_pinning() {
    // Night-street-like pair: luma mass in two bands (0..0.30 and
    // 0.62..0.95) with NOTHING between, both sides the same frame through
    // a known lift (x^0.75). The first estimator planted equal-y knots
    // inside the empty band (→ ~30-level posterised plateaus) and latched
    // its top knots to 1.0 on frames darker than its fixed grid (→ whole
    // upper bands pinned to pure white). Quantile-anchored knots must
    // bridge the gap monotonically and keep the tail off white.
    // 512×64 keeps the sample count above the degenerate-input guard.
    let (w, h) = (512u32, 64u32);
    let mut n = RgbImage::new(w, h);
    let mut c = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / (w - 1) as f32;
            let nv = if x % 2 == 0 { 0.30 * t } else { 0.62 + 0.33 * t };
            let cv = nv.powf(0.75).min(1.0);
            n.put_pixel(x, y, image::Rgb([(nv * 255.0).round() as u8; 3]));
            c.put_pixel(x, y, image::Rgb([(cv * 255.0).round() as u8; 3]));
        }
    }
    let knots = camera_base_knots(&DynamicImage::ImageRgb8(n), &DynamicImage::ImageRgb8(c))
        .expect("512x64 holds 512 blocks of the 64-column grid");
    assert!(!knots.is_empty(), "a real lift must be detected");
    // Apply through the real render path over a full ramp and measure.
    let mut ramp = RgbImage::new(256, 1);
    for x in 0..256 {
        ramp.put_pixel(x, 0, image::Rgb([x as u8; 3]));
    }
    let r = EditRecipe { base_curve: knots, ..Default::default() };
    let out = develop_preview(&DynamicImage::ImageRgb8(ramp), &r).to_rgb8();
    let vals: Vec<u8> = (0..256u32).map(|x| out[(x, 0)][0]).collect();
    let (mut longest, mut run) = (1usize, 1usize);
    for i in 1..256 {
        if vals[i] == vals[i - 1] {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 1;
        }
    }
    assert!(longest <= 12, "posterised plateau of {longest} identical output levels");
    assert!(vals[229] < 250, "input 0.9 must not latch to white: {}", vals[229]);
    assert!(vals[250] > vals[235], "the tail keeps rising toward the (1,1) pin");
}

/// v1.2.2: the base-look estimator pairs the rendition against the frame
/// it SHOWS. A 300x200 neutral with dark side strips and a "camera"
/// rendition that is its centred 4:3 crop, pixel for pixel: paired whole,
/// the strips' mass sits on one side of the CDF match only and a curve
/// appears where there is none; paired on the camera's frame the pair is
/// the identity it is. A same-frame pair passes through untouched.
#[test]
fn the_base_look_is_estimated_on_the_frame_the_camera_shows() {
    let neutral = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(300, 200, |x, _| {
        if !(16..283).contains(&x) {
            image::Rgb([20, 20, 20])
        } else {
            let v = (x * 255 / 300) as u8;
            image::Rgb([v, v, v])
        }
    }));
    let camera = neutral.crop_imm(16, 0, 267, 200);
    assert!(
        !camera_base_knots(&neutral, &camera).expect("judgeable").is_empty(),
        "paired whole, the edge strips read as a camera curve"
    );
    let paired = camera_frame_of(&neutral, &camera);
    assert_eq!((paired.width(), paired.height()), (267, 200));
    assert!(
        camera_base_knots(&paired, &camera).expect("judgeable").is_empty(),
        "paired on the frame the camera shows, an identical crop is the identity"
    );
    let same = camera_frame_of(&neutral, &super::thumbnail(&neutral, 150, 100));
    assert_eq!((same.width(), same.height()), (300, 200), "a same-frame pair is untouched");
}

/// A night-like frame for the corner question, 600×400: a sky graded left
/// to right over dark ground, as the SENSOR saw it through a lens whose
/// profile lifts the corners by 1.8; the same frame with that lift; and two
/// cameras with one tone (0.85 x), one that drew the sensor's picture and
/// one that drew the lifted one.
fn corner_question() -> (crate::recipe::LensProfile, [DynamicImage; 4]) {
    let (w, h) = (600u32, 400u32);
    let lens = crate::recipe::LensProfile {
        vignette: (0..16).map(|i| 1.0 + 0.8 * i as f32 / 15.0).collect(),
        vignette_on: true,
        ..Default::default()
    };
    let scene = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = if y >= h * 5 / 6 { 0.06 } else { 0.20 + 0.25 * x as f32 / (w - 1) as f32 };
        image::Rgb([(v * 255.0).round() as u8; 3])
    }));
    let falloff = EditRecipe {
        lens_profile: crate::recipe::LensProfile {
            vignette: lens.vignette.iter().map(|g| 1.0 / g).collect(),
            vignette_on: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let neutral = develop_preview(&scene, &falloff);
    let lifted = develop_preview(
        &neutral,
        &EditRecipe { lens_profile: lens.clone(), ..Default::default() },
    );
    let tone = |img: &DynamicImage| {
        let mut out = img.to_rgb8();
        for c in out.iter_mut() {
            *c = (0.85 * *c as f32).round() as u8;
        }
        DynamicImage::ImageRgb8(out)
    };
    let cameras = (tone(&neutral), tone(&lifted));
    (lens, [neutral, lifted, cameras.0, cameras.1])
}

/// 2026-09-21: whether the camera's rendition shows the profile's corner
/// lift is measured on the pair. Both answers, and the reading behind each.
#[test]
fn the_estimation_base_is_the_picture_the_camera_drew() {
    let (lens, [neutral, lifted, drew_the_sensor, drew_the_lift]) = corner_question();
    let bytes = |img: &DynamicImage| img.to_rgb8().into_raw();
    let read = |base: &DynamicImage, camera: &DynamicImage| {
        corner_residual(base, camera).expect("600x400 with an open tone range is judgeable")
    };
    let (as_seen, with_lift) = (read(&neutral, &drew_the_sensor), read(&lifted, &drew_the_sensor));
    assert!(
        as_seen.abs() < 0.002 && with_lift < -0.004,
        "a lift the camera never made predicts brighter corners than it drew: \
         as seen {as_seen}, with the lift {with_lift}"
    );
    assert_eq!(
        bytes(&estimation_base(&neutral, &lens, &drew_the_sensor)),
        bytes(&neutral),
        "the rendition is a tone map of the sensor's picture, so that is the base"
    );
    let (as_seen, with_lift) = (read(&neutral, &drew_the_lift), read(&lifted, &drew_the_lift));
    assert!(
        as_seen > 0.003 && with_lift.abs() < 0.002,
        "a lift the camera did make leaves its corners above the sensor's picture: \
         as seen {as_seen}, with the lift {with_lift}"
    );
    assert_eq!(
        bytes(&estimation_base(&neutral, &lens, &drew_the_lift)),
        bytes(&lifted),
        "the rendition is a tone map of the lifted picture, so that is the base"
    );
    // No profile, no question.
    assert_eq!(
        bytes(&estimation_base(&neutral, &Default::default(), &drew_the_lift)),
        bytes(&neutral)
    );
}

/// What the measurement is FOR. The camera's tone here is 0.85 x and
/// nothing else. Matched against the sensor's picture the estimate says so;
/// matched against the lifted picture — what the estimator did from
/// 2026-08-03 to 2026-09-21 whatever the camera had drawn — the lift comes
/// out as tone, and the control stays in the test so the defect stays
/// visible.
#[test]
fn a_corner_lift_the_camera_never_made_does_not_become_tone() {
    let (lens, [neutral, lifted, drew_the_sensor, _]) = corner_question();
    let miss = |knots: &[[f32; 2]]| {
        knots[1..knots.len() - 1]
            .iter()
            .map(|k| (k[1] - 0.85 * k[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let look = camera_base_look(&neutral, &lens, &drew_the_sensor).expect("judgeable");
    assert!(look.len() > 2, "0.85 x is not the identity: {look:?}");
    assert!(miss(&look) < 0.01, "the camera's tone, to a level or two: {look:?}");
    let control = camera_base_knots(&lifted, &drew_the_sensor).expect("judgeable");
    assert!(
        miss(&control) > 0.03,
        "premise: the unconditional lift misses the camera's tone by levels: {control:?}"
    );
}

/// The profile's gain is a function of the SENSOR'S radius. A body set to
/// an in-camera aspect shows a centred crop, so the lift is made on the
/// whole frame and the camera's frame is cut afterwards — lifted after the
/// cut, the crop's own corners would be taken for the sensor's.
#[test]
fn the_lift_is_made_on_the_sensor_frame_and_cut_to_the_cameras_afterwards() {
    let (lens, [neutral, lifted, ..]) = corner_question();
    let shown = lifted.crop_imm(33, 0, 533, 400); // the centred 4:3 of 600x400
    let mut camera = shown.to_rgb8();
    for c in camera.iter_mut() {
        *c = (0.85 * *c as f32).round() as u8;
    }
    let base = estimation_base(&neutral, &lens, &DynamicImage::ImageRgb8(camera));
    assert_eq!((base.width(), base.height()), (533, 400), "paired on the camera's frame");
    assert_eq!(
        base.to_rgb8().into_raw(),
        shown.to_rgb8().into_raw(),
        "the whole frame's lift, cut to the camera's frame"
    );
}

/// A pair that cannot say has not claimed the lift: the sensor's picture
/// stays the base.
#[test]
fn a_pair_that_cannot_say_keeps_the_picture_the_sensor_saw() {
    let (lens, [neutral, ..]) = corner_question();
    let blown = DynamicImage::ImageRgb8(RgbImage::from_pixel(600, 400, image::Rgb([255; 3])));
    assert!(corner_residual(&neutral, &blown).is_none(), "every block clipped");
    assert_eq!(
        estimation_base(&neutral, &lens, &blown).to_rgb8().into_raw(),
        neutral.to_rgb8().into_raw()
    );
    let tiny = super::thumbnail(&neutral, 48, 32);
    assert!(corner_residual(&tiny, &tiny).is_none(), "smaller than the grid");
}

/// The three open paths enter through `camera_base_look` and nowhere else,
/// so the pairing cannot drift between them again (v1.2.2's frame pairing
/// reached one of the three), and the entry pairs the frame itself.
#[test]
fn the_three_open_paths_share_one_base_look_entry() {
    for (name, text) in [
        ("pipeline.rs", include_str!("../../pipeline.rs")),
        ("serve.rs", include_str!("../../serve.rs")),
        ("bin/gui/actions.rs", include_str!("../../bin/gui/actions.rs")),
    ] {
        assert!(text.contains("camera_base_look("), "{name} estimates through the one entry");
        for by_hand in ["camera_base_knots(", "camera_frame_of(", "estimation_base("] {
            assert!(!text.contains(by_hand), "{name} assembles the pair by hand: {by_hand}");
        }
    }
    // The v1.2.2 construction, through the entry: a rendition that is the
    // centred 4:3 crop of the neutral, pixel for pixel, is the identity.
    let neutral = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(300, 200, |x, _| {
        if !(16..283).contains(&x) {
            image::Rgb([20, 20, 20])
        } else {
            let v = (x * 255 / 300) as u8;
            image::Rgb([v, v, v])
        }
    }));
    let camera = neutral.crop_imm(16, 0, 267, 200);
    assert!(
        camera_base_look(&neutral, &Default::default(), &camera).expect("judgeable").is_empty(),
        "paired on the frame the camera shows, an identical crop is the identity"
    );
}

#[test]
fn camera_base_knots_merges_same_bin_quantiles_with_a_mean() {
    // A posterised neutral (one constant tone) against a camera side whose
    // mass splits across two levels: every probability lands on the same
    // neutral bin, and keeping only the first duplicate would map that
    // tone to the LOWER camera level. The merged knot must sit between.
    let (w, h) = (512u32, 64u32);
    let mut n = RgbImage::new(w, h);
    let mut c = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            n.put_pixel(x, y, image::Rgb([77; 3])); // luma 0.302
            let cv = if x % 2 == 0 { 128 } else { 179 }; // 0.502 / 0.702
            c.put_pixel(x, y, image::Rgb([cv; 3]));
        }
    }
    let knots = camera_base_knots(&DynamicImage::ImageRgb8(n), &DynamicImage::ImageRgb8(c))
        .expect("512x64 clears the degenerate-input guard");
    assert_eq!(knots.len(), 3, "one merged mid knot between the pins: {knots:?}");
    let mid = knots[1];
    assert!((mid[0] - 0.302).abs() < 0.01, "x = the neutral spike: {mid:?}");
    assert!(
        mid[1] > 0.52 && mid[1] < 0.68,
        "y = a mean over the camera split, not its floor: {mid:?}"
    );
}

/// Real-machine probe, never run in CI: point AUTOSHADE_PROBE_RAW at an
/// ARW and run with `--ignored` to check the whole base-look chain on a
/// real photo — estimator knots + the luma median of the base-curved
/// render vs the camera's own preview (they must land close).
#[test]
#[ignore = "real-machine probe: set AUTOSHADE_PROBE_RAW to an ARW path"]
fn probe_real_raw_base_look() {
    let Some(raw) = crate::config::live_env("AUTOSHADE_PROBE_RAW") else {
        panic!("set AUTOSHADE_PROBE_RAW to a RAW path");
    };
    let raw = std::path::PathBuf::from(raw);
    let cam_probe = crate::decode::embedded_preview(&raw);
    println!(
        "embedded_preview: {:?}",
        cam_probe.as_ref().map(|o| o.as_ref().map(|i| (i.width(), i.height())))
    );
    let knots = crate::pipeline::photo_base_knots_checked_in(&raw, &crate::diag::photo(&raw))
        .unwrap_or_default();
    println!("knots: {knots:?}");
    // The corner question on this photo, both readings and both answers.
    if let Ok(Some(cam)) = &cam_probe {
        let lens = crate::pipeline::fresh_lens_profile(&raw);
        let working =
            render_to_image(&raw, &EditRecipe::default(), None, Some(2048)).unwrap();
        let small = super::thumbnail(&working, 1024, 1024);
        let plain = camera_frame_of(&small, cam);
        println!("profile corner gain: {:?}", lens.vignette.last());
        println!("corner residual as the sensor saw it: {:?}", corner_residual(&plain, cam));
        if lens.vignette_active() {
            let lift = EditRecipe {
                lens_profile: crate::recipe::LensProfile {
                    vignette: lens.vignette.clone(),
                    vignette_on: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let lifted = camera_frame_of(&develop_preview(&small, &lift), cam);
            println!("corner residual with the lift: {:?}", corner_residual(&lifted, cam));
            println!("knots as the sensor saw it: {:?}", camera_base_knots(&plain, cam));
            println!("knots with the lift: {:?}", camera_base_knots(&lifted, cam));
            // How far each estimate's render of the SENSOR'S picture sits
            // from the camera's, block by block, in 8-bit levels.
            for (name, base) in [("as seen", &plain), ("with the lift", &lifted)] {
                let Some(knots) = camera_base_knots(base, cam) else { continue };
                let based = develop_preview(
                    &plain,
                    &EditRecipe { base_curve: knots, ..Default::default() },
                );
                let rows = (64.0 * plain.height() as f32 / plain.width() as f32).round() as usize;
                let (ours, theirs) = (
                    block_lumas(&based, 64, rows).unwrap(),
                    block_lumas(cam, 64, rows).unwrap(),
                );
                let mut err: Vec<f32> =
                    ours.iter().zip(&theirs).map(|(a, b)| (a - b) * 255.0).collect();
                let rms = (err.iter().map(|e| e * e).sum::<f32>() / err.len() as f32).sqrt();
                err.sort_by(|a, b| a.total_cmp(b));
                println!(
                    "render with the estimate made {name} vs camera: rms {rms:.2} levels, median {:+.2}",
                    err[err.len() / 2]
                );
            }
        }
    }
    assert!(!knots.is_empty(), "expected a base look on a camera RAW");
    let neutral =
        super::thumbnail(&render_to_image(&raw, &EditRecipe::default(), None, None).unwrap(), 1536, 1536);
    let based = develop_preview(
        &neutral,
        &EditRecipe { base_curve: knots, ..Default::default() },
    );
    let cam = crate::decode::embedded_preview(&raw).unwrap().unwrap();
    let median = |img: &DynamicImage| -> f32 {
        let rgb = img.to_rgb8();
        let mut v: Vec<f32> = rgb
            .as_raw()
            .chunks(3)
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2] / 255.0
    };
    let (mn, mb, mc) = (median(&neutral), median(&based), median(&cam));
    println!("median neutral={mn:.3} based={mb:.3} camera={mc:.3}");
    assert!(
        (mb - mc).abs() < 0.06,
        "base-curved render should sit near the camera preview: based={mb:.3} camera={mc:.3}"
    );
}

/// v1.17.0 — exactly ONE starting tone, decided from the recipe
/// (`EditRecipe::starting_curve`). A recipe that names a camera profile starts
/// from the profile's tone (own curve or Adobe's default), so a camera-matched
/// `base_curve` it still carries does not ALSO apply — in the export and in
/// every preview develop alike; a creative Look's baked curve still composes
/// on top.
///
/// MUTATION: return `base_curve` from `starting_curve` whatever the recipe
/// names, or drop the Look along with the base curve.
#[test]
fn a_named_camera_profile_is_the_only_starting_tone() {
    use crate::recipe::{CreativeLook, CurvePoint};
    let lift = vec![[0.0, 0.0], [0.25, 0.4], [0.5, 0.7], [1.0, 1.0]];
    let stamped = EditRecipe { base_curve: lift.clone(), contrast: 20.0, ..Default::default() };
    let unstamped = EditRecipe { contrast: 20.0, ..Default::default() };
    assert_ne!(build_tone_lut(&stamped), build_tone_lut(&unstamped), "the base curve is tone work");
    let profiled = EditRecipe { camera_profile: "Adobe Standard".into(), ..stamped.clone() };
    assert!(profiled.starting_curve().is_empty(), "a named profile replaces the base curve");
    // The sliders on a named profile follow Camera Raw's law (`tone_cr`), so
    // the same recipe without the stamped curve is the comparison.
    let profiled_unstamped = EditRecipe { camera_profile: "Adobe Standard".into(), ..unstamped.clone() };
    assert_eq!(build_tone_lut(&profiled), build_tone_lut(&profiled_unstamped), "…and nothing else moved");
    // The Look's own base profile names a profile too, and its baked curve is
    // still the start's next step.
    let knot = |input, output| CurvePoint { input, output };
    let look = CreativeLook {
        name: "Adobe Color".to_string(),
        amount: 1.0,
        base_profile: "Adobe Standard".to_string(),
        tone_curve: vec![knot(0, 0), knot(40, 35), knot(224, 230), knot(255, 255)],
        ..Default::default()
    };
    let with_look = EditRecipe { look: Some(look), ..stamped.clone() };
    assert!(with_look.starting_curve().is_empty());
    assert_eq!(
        build_tone_lut(&with_look),
        build_tone_lut(&EditRecipe { base_curve: Vec::new(), ..with_look.clone() }),
    );
    assert_ne!(build_tone_lut(&with_look), build_tone_lut(&unstamped), "the Look's curve renders");
}

/// Without a named profile — an old recipe, and every fresh open on a machine
/// that cannot render Lightroom's canvas — the stamped base curve IS the
/// start, bit for bit as before v1.17.0.
///
/// MUTATION: clear the base curve whatever the recipe names.
#[test]
fn without_a_named_profile_the_base_curve_is_the_start() {
    let lift = vec![[0.0, 0.0], [0.25, 0.4], [0.5, 0.7], [1.0, 1.0]];
    let fresh = EditRecipe { base_curve: lift.clone(), ..Default::default() };
    assert!(!fresh.names_profile());
    assert_eq!(fresh.starting_curve(), lift.as_slice(), "untouched");
    assert!(fresh.has_tone_work(), "the curve is tone work");
    assert_ne!(build_tone_lut(&fresh), build_tone_lut(&EditRecipe::default()));
}

/// CALIBRATION (env-gated, `AUTOSHADE_FIT_CALIBRATION_DIR`): a sidecar that
/// names Adobe Standard and the Adobe Color Look and nothing else renders
/// where Lightroom's export of the same frame with no develop settings sits,
/// band by band in lightness — the v1.17.0 baseline exposure and default
/// curve. Measured before them: midtones ΔL −17.15, highlights −19.68,
/// shadows −1.20. Since the colour spec (ForwardMatrix) and the Adobe Color
/// table, the sky-ish and warm rows' Δb as well (from −5.76 and +7.94).
///
/// Needs the corpus pair (`p39.arw`, `tint/p39-flat.jpg`) and Adobe's
/// installed profiles; says which is missing and passes without them.
#[test]
fn calibration_a_flat_sidecar_renders_where_lightroom_starts() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("flat sidecar calibration", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let (raw, flat) = (dir.join("p39.arw"), dir.join("tint").join("p39-flat.jpg"));
    if !raw.is_file() || !flat.is_file() {
        crate::test_skipped(
            "flat sidecar calibration",
            &format!("the corpus at {} lacks the flat pair", dir.display()),
        );
        return;
    }
    if crate::dcp::find("SONY", "ILCE-7RM4A", "Adobe Standard").is_err() {
        crate::test_skipped("flat sidecar calibration", "Adobe's camera profiles are not installed");
        return;
    }
    let curve = |pts: &[(u8, u8)]| -> String {
        pts.iter().map(|(i, o)| format!("<rdf:li>{i}, {o}</rdf:li>")).collect()
    };
    let flat_curve = curve(&[(0, 0), (255, 255)]);
    let xmp = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Version="15.5.1" crs:ProcessVersion="15.4" crs:CameraProfile="Adobe Standard" crs:HasSettings="True">
<crs:Look><rdf:Description crs:Name="Adobe Color" crs:Amount="1"><crs:Parameters>
<rdf:Description crs:Version="15.5.1" crs:ProcessVersion="15.4" crs:ConvertToGrayscale="False"
 crs:CameraProfile="Adobe Standard" crs:LookTable="E1095149FDB39D7A057BAB208837E2E1">
<crs:ToneCurvePV2012><rdf:Seq>{}</rdf:Seq></crs:ToneCurvePV2012>
<crs:ToneCurvePV2012Red><rdf:Seq>{flat_curve}</rdf:Seq></crs:ToneCurvePV2012Red>
<crs:ToneCurvePV2012Green><rdf:Seq>{flat_curve}</rdf:Seq></crs:ToneCurvePV2012Green>
<crs:ToneCurvePV2012Blue><rdf:Seq>{flat_curve}</rdf:Seq></crs:ToneCurvePV2012Blue>
</rdf:Description></crs:Parameters></rdf:Description></crs:Look>
</rdf:Description></rdf:RDF></x:xmpmeta>"#,
        curve(&[(0, 0), (22, 16), (40, 35), (127, 127), (224, 230), (240, 246), (255, 255)]),
    );
    let mut recipe = crate::xmp::xmp_to_recipe_for_photo(&xmp, &raw);
    assert_eq!(recipe.camera_profile, "Adobe Standard", "the sidecar names the profile");
    recipe.lens_profile = crate::pipeline::fresh_lens_profile_for_sidecar(&raw, Some(&xmp));
    let ours = render_to_image(&raw, &recipe, None, Some(1000)).expect("render").to_rgb8();
    let theirs = image::open(&flat)
        .expect("flat export")
        .resize_exact(ours.width(), ours.height(), image::imageops::FilterType::Triangle)
        .to_rgb8();
    // sRGB → CIE Lab (D65), the measurement script's own conversion.
    #[allow(clippy::excessive_precision)]
    let lab = |p: &image::Rgb<u8>| -> [f64; 3] {
        let lin = |v: u8| {
            let c = v as f64 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (lin(p[0]), lin(p[1]), lin(p[2]));
        let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
        let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
        let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;
        let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
        [116.0 * f(y) - 16.0, 500.0 * (f(x) - f(y)), 200.0 * (f(y) - f(z))]
    };
    // Bands by LIGHTROOM's lightness, ΔL = ours − Lightroom's; and two colour
    // rows by Lightroom's hue (v1.17.0, the ForwardMatrix and the Adobe Color
    // table): sky-ish h 190–280° C > 8 and warm h 20–90° C > 12, Δb.
    let mut bands = [(0.0f64, 0usize); 3];
    let mut colour = [(0.0f64, 0usize); 2];
    for (a, b) in ours.pixels().zip(theirs.pixels()) {
        let (la, lb) = (lab(a), lab(b));
        let band = if lb[0] < 35.0 { 0 } else if lb[0] < 70.0 { 1 } else { 2 };
        bands[band].0 += la[0] - lb[0];
        bands[band].1 += 1;
        let (c, h) = (lb[1].hypot(lb[2]), lb[2].atan2(lb[1]).to_degrees().rem_euclid(360.0));
        let row = if h > 190.0 && h < 280.0 && c > 8.0 {
            Some(0)
        } else if h > 20.0 && h < 90.0 && c > 12.0 {
            Some(1)
        } else {
            None
        };
        if let Some(i) = row {
            colour[i].0 += la[2] - lb[2];
            colour[i].1 += 1;
        }
    }
    let mean = |s: (f64, usize)| s.0 / s.1.max(1) as f64;
    let (dark, mid, light) = (mean(bands[0]), mean(bands[1]), mean(bands[2]));
    let (sky_b, warm_b) = (mean(colour[0]), mean(colour[1]));
    eprintln!("flat sidecar ΔL by band: L<35 {dark:+.2}, L35-70 {mid:+.2}, L>70 {light:+.2}");
    eprintln!("flat sidecar Δb by row: sky-ish {sky_b:+.2}, warm {warm_b:+.2}");
    assert!(dark.abs() <= 2.0, "shadows ΔL {dark:+.2} (bound ±2)");
    assert!(mid.abs() <= 3.0, "midtones ΔL {mid:+.2} (bound ±3)");
    assert!(light.abs() <= 3.0, "highlights ΔL {light:+.2} (bound ±3)");
    // Before v1.17.0's colour spec and look table: −5.76 and +7.94.
    if crate::dcp::find_look_table("E1095149FDB39D7A057BAB208837E2E1").is_ok() {
        assert!(sky_b.abs() <= 1.5, "sky-ish Δb {sky_b:+.2} (bound ±1.5)");
        assert!(warm_b.abs() <= 1.5, "warm Δb {warm_b:+.2} (bound ±1.5)");
    }
}


/// v1.17.0 step 2's contract, ahead of the canvas switch: the canvas start —
/// 「Adobe Standard」 with the 「Adobe Color」 Look, [`crate::xmp::canvas_look`]
/// — is the SAME recipe as an all-zero Lightroom sidecar that names Adobe
/// Color, and renders the same pixels, byte for byte. Env-gated on the corpus
/// (`p39.arw`); passes without it after saying so.
///
/// MUTATION: change any knot of `CANVAS_LOOK_XMP`'s curve, or its table id.
#[test]
fn a_fresh_canvas_renders_like_a_sidecar_that_names_adobe_color() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("canvas start identity", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p39.arw");
    if !raw.is_file() {
        crate::test_skipped("canvas start identity", "the corpus lacks its RAW");
        return;
    }
    let xmp = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Version="18.4" crs:ProcessVersion="15.4" crs:CameraProfile="Adobe Standard" crs:HasSettings="True">
{}
</rdf:Description></rdf:RDF></x:xmpmeta>"#,
        crate::xmp::CANVAS_LOOK_XMP
    );
    let sidecar = crate::xmp::xmp_to_recipe(&xmp);
    let canvas = EditRecipe {
        camera_profile: "Adobe Standard".into(),
        look: Some(crate::xmp::canvas_look()),
        ..Default::default()
    };
    assert_eq!(sidecar.look, canvas.look, "one Look");
    assert_eq!(sidecar.camera_profile, canvas.camera_profile);
    let a = render_to_image(&raw, &canvas, None, Some(600)).expect("canvas").to_rgb16();
    let b = render_to_image(&raw, &sidecar, None, Some(600)).expect("sidecar").to_rgb16();
    assert_eq!(a.dimensions(), b.dimensions());
    assert!(a.as_raw() == b.as_raw(), "the canvas start and the sidecar render different pixels");
}
