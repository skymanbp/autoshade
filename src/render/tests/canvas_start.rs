// One part of the engine's tests (src/render/tests.rs includes it): the canvas's base develop (v1.17.0) — the base part, the fresh start, and the invariant that every surface showing a fresh canvas shows the export.

/// The base part keeps exactly what the profile stage reads — the camera
/// profile and the Look's identity and table — and nothing of the Look's
/// baked half, which the preview develop renders from the full recipe; a
/// recipe that names no profile has the neutral part, whose base is the
/// neutral develop every base was before v1.17.0, unsharpened.
///
/// MUTATION: keep the Look's tone curve in the part (it would render twice),
/// give a profile-less Look a non-neutral part (a needless re-develop), or
/// let a base take the RAW default sharpening (it would be sharpened twice).
#[test]
fn the_base_part_is_the_profile_stage_and_nothing_after_it() {
    let canvas = canvas_recipe();
    let part = BasePart::of(&canvas);
    assert!(!part.is_neutral());
    let base = part.recipe();
    assert_eq!(base.camera_profile, CANVAS_PROFILE);
    let look = base.look.as_ref().expect("the Look rides the part");
    let full = canvas.look.as_ref().expect("the canvas carries Adobe Color");
    assert_eq!((&look.name, &look.table, &look.base_profile), (&full.name, &full.table, &full.base_profile));
    assert_eq!(look.amount, full.amount);
    assert!(!full.tone_curve.is_empty(), "Adobe Color bakes a curve");
    assert!(look.tone_curve.is_empty(), "…which the base does not");
    assert!(base.baked_look().is_none() && !base.has_tone_work(), "the base develops no tone");
    // Sliders and the camera curve are not the part's business.
    let edited = EditRecipe { exposure_ev: 0.7, base_curve: vec![[0.0, 0.0], [1.0, 1.0]], ..canvas.clone() };
    assert_eq!(BasePart::of(&edited), part);
    // No profile named: neutral, whatever else the Look carries.
    let unnamed = EditRecipe {
        look: Some(crate::recipe::CreativeLook { base_profile: String::new(), ..full.clone() }),
        ..Default::default()
    };
    assert!(BasePart::of(&unnamed).is_neutral());
    let mut unsharpened = EditRecipe::default();
    unsharpened.set_resolved("sharpening", 0.0);
    assert_eq!(BasePart::default().recipe(), unsharpened, "the neutral develop's recipe, unsharpened");
    assert_eq!(base.capture_sharpening(true), 0.0, "no base is sharpened: the display and the export sharpen once");
}

/// A recipe saved before v1.17.0 — a stamped camera base curve, no profile —
/// renders BYTE-IDENTICALLY to the build before the canvas switch (the era
/// rule: a saved develop keeps the start it was tuned on). The digest was
/// read off this same render (`render_to_image`, 16-bit, 1000 px) at c30a6b5,
/// the v1.17.0 step-3 commit, before any of the canvas work, and re-read once
/// on the merge with the v1.17.0 Detail law (47c2674): that law re-tunes the
/// default capture sharpening every RAW gets, so it moves this render too —
/// with sharpening explicitly off, the c30a6b5-line build and the merge render
/// the recipe byte-identically. Re-read once more on the raw white level
/// (v1.17.0: a measured Sony body now scales its raw by Camera Raw's level,
/// 16383 not 15360, so every render of it is 0.096 EV darker): the era rule
/// is about what a saved recipe means, not about the raw's scale, so the
/// digest follows the decode. Re-read a third time, on the merge of that
/// white level with the signed black level (`develop_cfa_signed`, v1.17.0),
/// which moves this render where the sensor sits near its black and only
/// there: against the build before it (CLI, 1000 px, 16-bit) 1.77 % of the
/// pixels change, 70 % of those encoded below 0.02 and 14 % of those in
/// 0.02–0.05, against 0.002 % above 0.5 (the demosaic's neighbourhood of a
/// site below the black). Unchanged by the merge of rel1170-vigca (the
/// measured exposure transfer, `CameraStart`, `UnderTone`): this recipe names
/// no profile, so no profile stage runs. Env-gated on the corpus (`p39.arw`).
///
/// MUTATION: let `starting_curve` drop a curve on a recipe that names no
/// profile, or develop a neutral part through anything but the neutral recipe.
#[test]
fn an_old_recipe_with_a_stamped_base_curve_renders_as_it_did() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("old recipe identity", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p39.arw");
    if !raw.is_file() {
        crate::test_skipped("old recipe identity", "the corpus lacks its RAW");
        return;
    }
    let old: EditRecipe = serde_json::from_str(OLD_RECIPE_JSON).expect("the old recipe parses");
    assert!(!old.names_profile() && BasePart::of(&old).is_neutral(), "an old recipe names no profile");
    assert_eq!(old.starting_curve(), old.base_curve.as_slice(), "its curve is its start");
    let img = render_to_image(&raw, &old, None, Some(1000)).expect("render").to_rgb16();
    let digest = fnv1a64_u16(img.as_raw());
    println!("old recipe @1000: {}x{} digest {digest:016x}", img.width(), img.height());
    assert_eq!(digest, OLD_RECIPE_P39_1000_DIGEST, "the old recipe no longer renders as it did");
}

/// The recipe [`an_old_recipe_with_a_stamped_base_curve_renders_as_it_did`]
/// renders: a camera curve of the pre-v1.17.0 kind plus a few sliders.
const OLD_RECIPE_JSON: &str = r#"{"version":3,"base_curve":[[0.0,0.0],[0.1,0.13],[0.25,0.33],[0.5,0.6],[0.75,0.83],[1.0,1.0]],"exposure_ev":0.3,"contrast":12.0,"saturation":8.0}"#;

/// Dehaze ALONE on a recipe that names a camera profile renders through
/// Camera Raw's measured law (`tone_cr`, v1.17.0): with no other tone work
/// the develop still runs the slider pass, a hazy pale sky blue darkens and
/// gains colour, and the engine law's airlight stage does not run as well (on
/// a uniform frame that stage keeps the darkest channel, the frame's own
/// airlight, where it is).
///
/// MUTATION: gate the slider pass on `has_tone_work` alone, or run
/// `apply_dehaze` on a camera-profile recipe too.
#[test]
fn dehaze_alone_on_a_camera_profile_follows_camera_raw() {
    let px = image::Rgb([200u8, 215, 240]);
    let frame = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, px));
    let named = EditRecipe { camera_profile: "Adobe Standard".into(), dehaze: 40.0, ..Default::default() };
    assert!(!named.has_tone_work(), "Dehaze is not tone work on its own");
    let got = develop_preview(&frame, &named).to_rgb8().get_pixel(1, 1).0;
    let luma = |p: [u8; 3]| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
    let spread = |p: [u8; 3]| p[2] as f32 - p[0] as f32;
    assert!(luma(got) < luma(px.0) - 2.0, "darker: {got:?} from {:?}", px.0);
    assert!(spread(got) > spread(px.0), "more colourful: {got:?} from {:?}", px.0);
    // The engine law's airlight stage on the same frame keeps the darkest
    // channel, its own airlight, where it was; the law above moves it.
    let unnamed = EditRecipe { dehaze: 40.0, ..Default::default() };
    let engine = develop_preview(&frame, &unnamed).to_rgb8().get_pixel(1, 1).0;
    assert!(engine[0].abs_diff(px.0[0]) <= 1, "the airlight stage: {engine:?}");
    assert!(got[0] + 3 < px.0[0], "Camera Raw's law: {got:?}");
}

/// FNV-1a over the little-endian bytes of a 16-bit buffer.
fn fnv1a64_u16(px: &[u16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in px {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// The pre-switch digest (see the test above).
const OLD_RECIPE_P39_1000_DIGEST: u64 = 0xcb8b_abcc_4d81_f379;

/// v1.17.0 step 2's INVARIANT: a fresh canvas is one picture on every surface.
/// The all-zero fresh recipe (the calibration a fresh open stamps) against the
/// engine's render of the all-zero Lightroom sidecar naming 「Adobe Standard」
/// with 「Adobe Color」, at 1000 px:
///
/// * the EXPORT of the fresh recipe — the same code path — byte-identical;
/// * the GUI and web canvas — the preview develop over the base develop
///   ([`base_develop`], as the GUI holds it, then the engine's finishing tail) — ΔE76 ≤ 0.5;
/// * a MASTER baked from that base (a heal / denoise / stack writes the base
///   develop's pixels) rendered under the recipe — ΔE76 ≤ 0.5;
/// * the reverse fit's SOURCE ([`crate::pipeline::fit_source`], the base
///   develop at the fit's 2048 edge) developed under its calibration through
///   the same tail — ΔE76 ≤ 0.5 against the sidecar rendered at that same
///   edge, both brought to the 1000 px grid by one filter. Against the
///   1000 px reference itself the edge change alone costs more than the
///   bound (the sidecar's own 2048 render reads 0.65 there on p39: the
///   resolution-dependent stages, not the base), so that number is printed,
///   not bounded.
///
/// Env-gated on the corpus and on Camera Raw's install (the canvas needs
/// both); it prints every number it bounds.
///
/// MUTATION: develop any of the three bases neutrally (the old canvas), or
/// let the preview develop keep the camera curve under a named profile.
#[test]
fn every_surface_shows_a_fresh_canvas_as_the_export_renders_it() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("canvas invariant", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p39.arw");
    if !raw.is_file() {
        crate::test_skipped("canvas invariant", "the corpus lacks its RAW");
        return;
    }
    if let Err(why) = canvas_renders(&raw) {
        crate::test_skipped("canvas invariant", &format!("the canvas cannot render here: {why}"));
        return;
    }
    const EDGE: u32 = 1000;
    // The fresh recipe a fresh open stamps (no saved develop consulted).
    let fresh = crate::pipeline::calibration_recipe(crate::pipeline::fresh_photo_calibration(&raw));
    assert_eq!(fresh.camera_profile, CANVAS_PROFILE, "a fresh open starts from Lightroom's canvas");
    assert!(fresh.base_curve.is_empty(), "…with no camera curve");
    assert!(
        !crate::xmp::global_export_losses(&fresh).contains(&"base_curve"),
        "…so a fresh photo's sidecar no longer names a camera base curve it cannot carry"
    );
    // The all-zero sidecar naming Adobe Color, imported with the photo's own
    // calibration halves (the import's rule) so the two recipes differ only in
    // where they came from.
    let xmp = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Version="18.4" crs:ProcessVersion="15.4" crs:CameraProfile="Adobe Standard" crs:HasSettings="True">
{}
</rdf:Description></rdf:RDF></x:xmpmeta>"#,
        crate::xmp::CANVAS_LOOK_XMP
    );
    let mut sidecar = crate::xmp::xmp_to_recipe(&xmp);
    sidecar.lens_profile = fresh.lens_profile.clone();
    sidecar.as_shot_k = fresh.as_shot_k;
    sidecar.as_shot_tint = fresh.as_shot_tint;
    sidecar.version = fresh.version;
    assert_eq!((&sidecar.camera_profile, &sidecar.look), (&fresh.camera_profile, &fresh.look));
    let reference = render_to_image(&raw, &sidecar, None, Some(EDGE)).expect("sidecar render");

    // 1. The export of the fresh recipe: the same path, byte for byte.
    let export = render_to_image(&raw, &fresh, None, Some(EDGE)).expect("export");
    assert!(
        export.to_rgb16().as_raw() == reference.to_rgb16().as_raw(),
        "the fresh recipe's export and the sidecar's render differ"
    );

    // 2. The canvas: the preview develop over the base develop, then the
    // engine's tail — `bin/gui/util.rs`'s `build_preview` and the web pane.
    let part = BasePart::of(&fresh);
    let base = base_develop(&raw, &part, Some(EDGE)).expect("base");
    let film = crate::decode::film_short_edge(&raw);
    // `source`: the RAW's base carries the lens (v1.17.0, `lens_in_camera`),
    // so the tail resamples none (`tail_lens`); the fit source is a sensor-frame
    // picture.
    let preview = |base: &DynamicImage, r: &EditRecipe, source: &SourceFrame| {
        let after = develop_preview_film(base, r, &crate::diag::pixels(), film, source);
        let geom = geometry_profile(r);
        let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
        frame_and_finish(after, r, tail_lens(&geom, source.corrected(r)), fs, CropPolicy::Cut)
    };
    assert_eq!(SourceFrame::Raw.corrected(&fresh), part.corrects_lens(), "premise: the base's frame is the part's");
    let canvas = preview(&base, &fresh, &SourceFrame::Raw);
    let de_canvas = mean_de76(&canvas, &reference);

    // 3. A master baked from the base and rendered under the recipe — the
    // baked door every master export takes, with the lens geometry its file
    // would carry (v1.17.0, `BasePart::baked_geometry`). Baked in the
    // corrected frame, a capped master is resampled before the cap as the
    // export is, so it is judged pixel for pixel.
    let master = master_develop(&raw, &part, Some(EDGE)).expect("master");
    let baked = part.baked_geometry();
    let from_master = render_baked_to_image(&master, &fresh, None, None, baked.as_ref(), &crate::diag::pixels())
        .expect("master render");
    let de_master = mean_de76(&from_master, &reference);

    // 4. The reverse fit's source under its calibration, at the fit's edge,
    // through the canvas's own tail (the fit source is the uncorrected
    // sensor frame; the lens geometry is the tail's).
    let (source, cal) = crate::pipeline::fit_source(&raw).expect("fit source");
    let fit_has_canvas = BasePart::of(&cal) == part;
    let fitted = preview(&DynamicImage::ImageRgb8(source.to_rgb8()), &cal, &SourceFrame::Baked(None));
    let at_fit_edge = render_to_image(&raw, &sidecar, None, Some(crate::pipeline::FIT_SOURCE_EDGE))
        .expect("sidecar render at the fit's edge");
    let to_grid = |im: &DynamicImage| {
        im.resize_exact(reference.width(), reference.height(), image::imageops::FilterType::Triangle)
    };
    let de_fit = mean_de76(&to_grid(&fitted), &to_grid(&at_fit_edge));
    let de_fit_vs_1000 = mean_de76(&fitted, &reference);
    let de_edge_alone = mean_de76(&at_fit_edge, &reference);
    println!(
        "canvas invariant @{EDGE}: export byte-identical; canvas ΔE76 {de_canvas:.3}; master \
         ΔE76 {de_master:.3}; fit source ΔE76 {de_fit:.3} (fit calibration names the canvas: \
         {fit_has_canvas}; against the {EDGE} px reference {de_fit_vs_1000:.3}, of which the edge \
         change alone {de_edge_alone:.3})"
    );
    assert!(de_canvas <= 0.5, "canvas ΔE76 {de_canvas:.3} (bound 0.5)");
    assert!(de_master <= 0.5, "master ΔE76 {de_master:.3} (bound 0.5)");
    if fit_has_canvas {
        assert!(de_fit <= 0.5, "fit source ΔE76 {de_fit:.3} (bound 0.5)");
    }
}

/// The canvas's float base (v1.17.0): a frame whose camera stage puts warm
/// light above the working space's 1 keeps it in the base, so the canvas's
/// negative Exposure brings it back as the export does. p37 (a warm frame) at
/// its fitted −1.86 EV over the fresh canvas, 1000 px: the canvas over the
/// float base against the export, bounded ΔE76 ≤ 0.5, and the same canvas
/// over that base cut to 8 bits printed beside it.
///
/// A float base (one that keeps its over-range light) thumbnails to the same
/// size and the same windows as `image`'s own thumbnail, with the plain mean:
/// `image` adds `n/2` before its division, so its f32 thumbnail is every
/// channel + 0.5. Downscale (block windows), 1:1 and the fractional upscale
/// cases, against a 16-bit copy of the same frame and against the exact mean.
///
/// MUTATION: send a float frame to `DynamicImage::thumbnail`.
#[test]
fn a_float_base_thumbnails_without_the_half_unit_bias() {
    let (w, h) = (37u32, 23u32);
    let at = |x: u32, y: u32, c: u32| ((x * 7 + y * 13 + c * 5) % 64) as f32 * 1024.0 / 65535.0;
    let f = DynamicImage::ImageRgb32F(ImageBuffer::from_fn(w, h, |x, y| Rgb([at(x, y, 0), at(x, y, 1), at(x, y, 2)])));
    let i = DynamicImage::ImageRgb16(ImageBuffer::from_fn(w, h, |x, y| {
        Rgb([0, 1, 2].map(|c| (at(x, y, c) * 65535.0).round() as u16))
    }));
    for (nw, nh) in [(9u32, 9u32), (37, 23), (50, 50), (20, 40)] {
        let (a, b) = (super::thumbnail(&f, nw, nh), i.thumbnail(nw, nh));
        assert_eq!((a.width(), a.height()), (b.width(), b.height()), "{nw}x{nh}: the size is image's");
        let (a, b) = (a.to_rgb32f(), b.to_rgb32f());
        let worst = a.as_raw().iter().zip(b.as_raw()).map(|(p, q)| (p - q).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1.0 / 65535.0, "{nw}x{nh}: float vs 16-bit thumbnail off by {worst}");
    }
    // A 2x2 block: the plain mean, not the mean + 0.5 image's f32 path gives.
    let tiny = DynamicImage::ImageRgb32F(ImageBuffer::from_fn(2, 2, |x, y| Rgb([(x + 2 * y) as f32 * 0.25, 1.5, -0.25])));
    let one = super::thumbnail_exact(&tiny, 1, 1).to_rgb32f();
    assert_eq!(one.get_pixel(0, 0).0, [0.375, 1.5, -0.25]);
    assert!((tiny.thumbnail_exact(1, 1).to_rgb32f().get_pixel(0, 0)[0] - 0.875).abs() < 1e-6, "premise: image's own is +0.5");
}

/// MUTATION: clip the base at 1 (`OverRange::Clip` in `base_develop`), or
/// read the preview's base through 8 bits.
#[test]
fn the_canvas_brings_back_over_range_light_as_the_export_does() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("canvas over-range", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p37.arw");
    if !raw.is_file() {
        crate::test_skipped("canvas over-range", "the corpus lacks its RAW");
        return;
    }
    if let Err(why) = canvas_renders(&raw) {
        crate::test_skipped("canvas over-range", &format!("the canvas cannot render here: {why}"));
        return;
    }
    const EDGE: u32 = 1000;
    let mut r = crate::pipeline::calibration_recipe(crate::pipeline::fresh_photo_calibration(&raw));
    r.exposure_ev = -1.86;
    let export = render_to_image(&raw, &r, None, Some(EDGE)).expect("export");
    let base = base_develop(&raw, &BasePart::of(&r), Some(EDGE)).expect("base");
    assert!(base.as_rgb32f().is_some(), "a profile part's base is a float frame");
    let over = base.as_rgb32f().map_or(0, |f| f.as_raw().iter().filter(|&&v| v > 1.0).count());
    let film = crate::decode::film_short_edge(&raw);
    let canvas = |base: &DynamicImage| {
        let after = develop_preview_film(base, &r, &crate::diag::pixels(), film, &SourceFrame::Raw);
        let geom = geometry_profile(&r);
        let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
        frame_and_finish(after, &r, tail_lens(&geom, lens_in_camera(&r)), fs, CropPolicy::Cut)
    };
    let de_float = mean_de76(&canvas(&base), &export);
    let de_8bit = mean_de76(&canvas(&DynamicImage::ImageRgb8(base.to_rgb8())), &export);
    println!(
        "canvas over-range @{EDGE}, -1.86 EV: {over} base components above 1; canvas ΔE76 over the float          base {de_float:.3}, over the base cut to 8 bits {de_8bit:.3}"
    );
    assert!(de_float <= 0.5, "canvas ΔE76 {de_float:.3} (bound 0.5)");
}

/// A canvas part names the white only when the recipe moves it on Camera
/// Raw's law; As Shot, the engine's law and the neutral part never carry one,
/// and the part's own recipe names the white it carries.
///
/// MUTATION: let `of_canvas` carry an As Shot white (every canvas re-develops
/// on open), or leave the white out of `recipe()` (the base is developed at
/// As Shot and the preview skips the white: no white at all).
#[test]
fn a_canvas_part_carries_a_white_only_when_the_recipe_moves_it() {
    let mut r = canvas_recipe();
    (r.as_shot_k, r.as_shot_tint) = (Some(5400.0), Some(14.0));
    assert!(!BasePart::of_canvas(&r).carries_white(), "no Temperature, no Tint: As Shot");
    (r.temperature_k, r.tint) = (Some(5400.0), 14.0);
    assert!(!BasePart::of_canvas(&r).carries_white(), "stamped at As Shot");
    assert_eq!(BasePart::of_canvas(&r), BasePart::of(&r));
    r.temperature_k = Some(4000.0);
    let part = BasePart::of_canvas(&r);
    assert!(part.carries_white() && !BasePart::of(&r).carries_white(), "a moved white rides the canvas part only");
    let back = part.recipe();
    assert_eq!(
        (back.temperature_k, back.tint, back.as_shot_k, back.as_shot_tint),
        (r.temperature_k, r.tint, r.as_shot_k, r.as_shot_tint)
    );
    assert_eq!(super::wb::camera_raw_white(&back), super::wb::camera_raw_white(&r));
    let mut engine = r.clone();
    (engine.camera_profile, engine.look) = (String::new(), None);
    assert!(BasePart::of_canvas(&engine).is_neutral(), "the engine's law has no profile stage to carry it");
}

/// v1.17.0: a moved white on every surface is the export's. The export puts a
/// Custom white into the profile stage (`camera::resolve_camera_profile`);
/// the canvas develops its base AT that white from the photo's camera frame
/// ([`CameraBase`], [`BasePart::of_canvas`]) and the preview develop leaves
/// the white out; the reverse fit's plate develops each candidate's white the
/// same way. Before, both adapted an As Shot base after the profile
/// (`wb::camera_raw_wb`), which is not where Camera Raw or the export put it.
///
/// p39 at 1000 px, four whites around its as-shot 5400 K / +14: the canvas
/// over its float base (the base every canvas keeps, `render::canvas`)
/// against the export, and the old route against the export, printed. At As
/// Shot the base is the render's and the canvas reads 0 (bound 0.01); a
/// moved white adds only what the white changes in the camera base's
/// block-average error ([`CameraBase`]), bounded at 0.35. Measured on the
/// merge of the white in the camera stage with the float base and the camera
/// stage's lens (2026-10-30): As Shot 0.000, the four whites 0.224–0.288 (old
/// route 1.084–1.644); over the base cut to 8 bits, as the white's first
/// measurement read it (2026-10-29: As Shot 0.361, whites 0.422–0.457), As
/// Shot 0.316 and the whites 0.403–0.476. Then the fit: a candidate rendered
/// on the fit source's 384 px plate against the export at the fit's edge
/// brought to that plate's grid, bounded at 0.5 (measured 0.217–0.275; old
/// route 1.184–1.747).
///
/// Env-gated on the corpus and on Camera Raw's install.
///
/// MUTATION: skip `plate_pixels` (the fit adapts after the profile again), or
/// let the preview develop apply the white over a canvas that carries it.
#[test]
fn a_moved_white_shows_on_every_surface_as_the_export_renders_it() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("moved-white invariant", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p39.arw");
    if !raw.is_file() {
        crate::test_skipped("moved-white invariant", "the corpus lacks its RAW");
        return;
    }
    if let Err(why) = canvas_renders(&raw) {
        crate::test_skipped("moved-white invariant", &format!("the canvas cannot render here: {why}"));
        return;
    }
    const EDGE: u32 = 1000;
    let fresh = crate::pipeline::calibration_recipe(crate::pipeline::fresh_photo_calibration(&raw));
    let film = crate::decode::film_short_edge(&raw);
    let preview = |base: &DynamicImage, r: &EditRecipe| {
        let after = develop_preview_film(base, r, &crate::diag::pixels(), film, &SourceFrame::Raw);
        let geom = geometry_profile(r);
        let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
        frame_and_finish(after, r, tail_lens(&geom, lens_in_camera(r)), fs, CropPolicy::Cut)
    };
    let as_shot_base = base_develop(&raw, &BasePart::of(&fresh), Some(EDGE)).expect("as-shot base");
    let floor = mean_de76(
        &preview(&as_shot_base, &fresh),
        &render_to_image(&raw, &fresh, None, Some(EDGE)).expect("as-shot export"),
    );
    // The fit's own source and analysis plate (`pipeline::fit_source`,
    // `fit::analysis_pair`), and its calibration with the lens geometry off
    // (the vignetting kept, as the source keeps it): the fit's candidates
    // render no geometry stage.
    let (source, cal) = crate::pipeline::fit_source(&raw).expect("fit source");
    assert!(source.has_camera(), "a corpus RAW's fit source keeps its camera frame");
    let plate = Plate::thumbnail_of(&source, crate::fit::ANALYZE_EDGE);
    assert!(plate.has_camera(), "…and so does its analysis thumbnail");
    let baked = Plate::baked((*source.shared()).clone());
    let old_plate = Plate::thumbnail_of(&baked, crate::fit::ANALYZE_EDGE);
    assert!(!old_plate.has_camera());
    let mut fit_cal = cal.clone();
    (fit_cal.lens_profile.distortion_on, fit_cal.lens_profile.ca_on) = (false, false);
    let mut worst = (0.0f64, 0.0f64);
    for (k, tint) in [(3500.0f32, 14.0f32), (4300.0, 30.0), (7500.0, 14.0), (9000.0, -20.0)] {
        let mut r = fresh.clone();
        (r.temperature_k, r.tint) = (Some(k), tint);
        assert!(BasePart::of_canvas(&r).carries_white());
        let export = render_to_image(&raw, &r, None, Some(EDGE)).expect("export");
        let base = base_develop(&raw, &BasePart::of_canvas(&r), Some(EDGE)).expect("canvas base");
        let canvas = preview(&base, &r);
        // The old route: the As Shot base, the white adapted after the profile.
        // A baked source in the base's own frame keeps the white in this pass;
        // the RAW's sharpening default is the only other thing it decides, so
        // it is written into the recipe.
        let mut old = r.clone();
        old.sharpening = r.capture_sharpening(true);
        let old_canvas = {
            let frame = SourceFrame::Baked(BasePart::of(&r).baked_geometry());
            let after = develop_preview_film(&as_shot_base, &old, &crate::diag::pixels(), film, &frame);
            let geom = geometry_profile(&r);
            let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
            frame_and_finish(after, &r, tail_lens(&geom, lens_in_camera(&r)), fs, CropPolicy::Cut)
        };
        let (de_canvas, de_old) = (mean_de76(&canvas, &export), mean_de76(&old_canvas, &export));
        // The fit's candidate on its plate, against the export at the fit's
        // edge thumbnailed by the plate's own operator.
        let mut c = fit_cal.clone();
        (c.temperature_k, c.tint) = (Some(k), tint);
        let fit_export = render_to_image(&raw, &c, None, Some(crate::pipeline::FIT_SOURCE_EDGE))
            .expect("export at the fit's edge")
            .thumbnail(crate::fit::ANALYZE_EDGE, crate::fit::ANALYZE_EDGE);
        let de_fit = mean_de76(&develop_preview(&plate, &c), &fit_export);
        let de_fit_old = mean_de76(&develop_preview(&old_plate, &c), &fit_export);
        println!(
            "moved white {k} K / {tint:+}: canvas ΔE76 {de_canvas:.3} (old route {de_old:.3}); fit              plate ΔE76 {de_fit:.3} (old route {de_fit_old:.3})"
        );
        worst = (worst.0.max(de_canvas), worst.1.max(de_fit));
    }
    // As Shot on the plate is the plate: no re-develop, no white applied.
    let at_shot = develop_preview(&plate, &fit_cal);
    let at_shot_old = develop_preview(&old_plate, &fit_cal);
    assert!(at_shot.to_rgb8().as_raw() == at_shot_old.to_rgb8().as_raw(), "As Shot renders the plate as it is");
    println!("moved white: the canvas's As Shot reading {floor:.3}");
    assert!(floor <= 0.01, "the canvas at As Shot ΔE76 {floor:.3} (bound 0.01)");
    assert!(worst.0 <= 0.35, "canvas ΔE76 {:.3} (bound 0.35)", worst.0);
    assert!(worst.1 <= 0.5, "fit plate ΔE76 {:.3} (bound 0.5)", worst.1);
}

/// Mean CIE76 ΔE between two renders, `a` resized onto `b`'s grid when their
/// sizes differ (sRGB → Lab D65, the measurement scripts' conversion).
fn mean_de76(a: &DynamicImage, b: &DynamicImage) -> f64 {
    let b8 = b.to_rgb8();
    let a8 = if a.dimensions() == b.dimensions() {
        a.to_rgb8()
    } else {
        a.resize_exact(b8.width(), b8.height(), image::imageops::FilterType::Triangle).to_rgb8()
    };
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
    let n = (a8.width() * a8.height()) as f64;
    a8.pixels()
        .zip(b8.pixels())
        .map(|(p, q)| {
            let (x, y) = (lab(p), lab(q));
            ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
        })
        .sum::<f64>()
        / n
}

/// The camera start (v1.17.0, `EditRecipe::camera_start`) is ONE value for a
/// photograph under its profile: the fresh open stamps it, a saved develop
/// reloaded is stamped with the same, the fit's calibration carries it, and it
/// is the start the profile stage itself renders at — so the canvas at +1 EV
/// (the develop's Exposure over the base, which reads the stamp) shows what
/// the export at +1 EV renders (whose stage knows its own start). The sidecar
/// never carries it.
///
/// Env-gated on the corpus and on Camera Raw's install.
///
/// MUTATION: stamp a start without the headroom (or without the profile's
/// offset), keep a saved recipe's start instead of the photo's, or let the
/// XMP payload carry the field.
#[test]
fn every_surface_sees_one_camera_start_and_the_sidecar_carries_none() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("camera start", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p39.arw");
    if !raw.is_file() {
        crate::test_skipped("camera start", "the corpus lacks its RAW");
        return;
    }
    if let Err(why) = canvas_renders(&raw) {
        crate::test_skipped("camera start", &format!("the canvas cannot render here: {why}"));
        return;
    }
    // The fresh open.
    let fresh = crate::pipeline::calibration_recipe(crate::pipeline::fresh_photo_calibration(&raw));
    let start = fresh.camera_start.expect("a fresh open stamps the start");
    assert!(start.ev > 0.0 && start.headroom > 0.9, "the body's baseline and a daylight neutral's headroom: {start:?}");
    // A saved develop reloaded: the stamp rides the JSON and the reload
    // re-reads the same from the photo, whatever the save held.
    let saved: EditRecipe = serde_json::from_str(&serde_json::to_string(&fresh).expect("json")).expect("reload");
    assert_eq!(saved.camera_start, Some(start), "the save carries it");
    let mut stale = EditRecipe { camera_start: Some(crate::recipe::CameraStart { ev: -2.0, headroom: 0.0, tables: 0 }), ..saved };
    crate::pipeline::stamp_camera_start(&raw, &mut stale);
    assert_eq!(stale.camera_start, Some(start), "a reload re-reads the photo's own");
    // The fit.
    let (_, cal) = crate::pipeline::fit_source(&raw).expect("fit source");
    if BasePart::of(&cal) == BasePart::of(&fresh) {
        assert_eq!(cal.camera_start, Some(start), "the fit's calibration carries the same start");
    }
    // The stage's own: the canvas at +1 EV shows the export at +1 EV.
    const EDGE: u32 = 1000;
    let up = EditRecipe { exposure_ev: 1.0, ..fresh.clone() };
    let export = render_to_image(&raw, &up, None, Some(EDGE)).expect("export");
    let base = DynamicImage::ImageRgb8(base_develop(&raw, &BasePart::of(&up), Some(EDGE)).expect("base").to_rgb8());
    let film = crate::decode::film_short_edge(&raw);
    let canvas = {
        let after = develop_preview_film(&base, &up, &crate::diag::pixels(), film, &SourceFrame::Raw);
        let geom = geometry_profile(&up);
        let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
        frame_and_finish(after, &up, tail_lens(&geom, lens_in_camera(&up)), fs, CropPolicy::Cut)
    };
    let de = mean_de76(&canvas, &export);
    // …and without the stamp the canvas would read the light back under 0 EV
    // and no headroom.
    let unstamped = EditRecipe { camera_start: None, ..up.clone() };
    let wrong = {
        let after = develop_preview_film(&base, &unstamped, &crate::diag::pixels(), film, &SourceFrame::Raw);
        let geom = geometry_profile(&unstamped);
        let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
        frame_and_finish(after, &unstamped, tail_lens(&geom, lens_in_camera(&unstamped)), fs, CropPolicy::Cut)
    };
    let de_wrong = mean_de76(&wrong, &export);
    println!("camera start {start:?}: canvas +1 EV vs export ΔE76 {de:.3}; unstamped {de_wrong:.3}");
    assert!(de <= 0.5, "canvas +1 EV ΔE76 {de:.3} (bound 0.5)");
    assert!(de_wrong > de, "the stamp is what makes the canvas right");
    // The sidecar never carries it.
    let xmp = crate::xmp::recipe_to_xmp(&fresh);
    assert!(!xmp.contains("camera_start"), "the sidecar carries no camera start");
}
