// One part of the engine's tests (src/render/tests.rs includes it): the canvas's base develop (v1.17.0) — the base part, and the invariant that every surface showing a develop shows the export.

/// The base part keeps exactly what the profile stage reads — the camera
/// profile and the Look's identity and table — and nothing of the Look's
/// baked half (a profile and a Look are colour data, v1.17.0); a recipe that
/// names no profile has the neutral part, whose base is the neutral develop
/// every base was before v1.17.0, unsharpened.
///
/// MUTATION: keep the Look's tone curve in the part, give a profile-less Look
/// a non-neutral part (a needless re-develop), or let a base take the RAW
/// default sharpening (it would be sharpened twice).
#[test]
fn the_base_part_is_the_profile_stage_and_nothing_after_it() {
    let look = crate::recipe::CreativeLook {
        name: "A Look".into(),
        amount: 1.0,
        base_profile: "A Profile".into(),
        table: "00000000000000000000000000000000".into(),
        tone_curve: vec![
            crate::recipe::CurvePoint { input: 0, output: 0 },
            crate::recipe::CurvePoint { input: 64, output: 50 },
            crate::recipe::CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    let named = EditRecipe { camera_profile: "A Profile".into(), look: Some(look.clone()), ..Default::default() };
    let part = BasePart::of(&named);
    assert!(!part.is_neutral());
    let base = part.recipe();
    assert_eq!(base.camera_profile, "A Profile");
    let kept = base.look.as_ref().expect("the Look rides the part");
    assert_eq!((&kept.name, &kept.table, &kept.base_profile), (&look.name, &look.table, &look.base_profile));
    assert_eq!(kept.amount, look.amount);
    assert!(kept.tone_curve.is_empty(), "the Look's baked curve is not the base's");
    assert!(!base.has_tone_work(), "the base develops no tone");
    // Sliders and the camera curve are not the part's business.
    let edited = EditRecipe { exposure_ev: 0.7, base_curve: vec![[0.0, 0.0], [1.0, 1.0]], ..named.clone() };
    assert_eq!(BasePart::of(&edited), part);
    // No profile named: neutral, whatever else the Look carries.
    let unnamed = EditRecipe {
        look: Some(crate::recipe::CreativeLook { base_profile: String::new(), ..look }),
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
/// no profile, so no profile stage runs. Re-read a fourth time on
/// rel1170-final, where the v1.17.0 capture-sharpening and luminance-NR
/// re-measure was reverted and the raw white level went back to the file's
/// own: this recipe is back on v1.16.0's sharpening and white level, so it
/// renders as v1.16.0 did up to
/// the signed black level — the CLI at 1000 px against the installed v1.16.0
/// differs on 0.034 % of the pixels, by at most one 8-bit code, 82 % of them
/// encoded below 0.05 and none above 0.5 (the merged tree before the gate: 98.7 %, up to 4.3 %).
/// Unchanged by the removal of the Camera Raw law family (v1.17.0): this
/// recipe names no profile, so none of it ever ran here.
/// Env-gated on the corpus (`p39.arw`).
///
/// MUTATION: drop the camera curve, or develop a neutral part through
/// anything but the neutral recipe.
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
    let img = render_to_image(&raw, &old, None, Some(1000)).expect("render").to_rgb16();
    let digest = fnv1a64_u16(img.as_raw());
    println!("old recipe @1000: {}x{} digest {digest:016x}", img.width(), img.height());
    assert_eq!(digest, OLD_RECIPE_P39_1000_DIGEST, "the old recipe no longer renders as it did");
}

/// The recipe [`an_old_recipe_with_a_stamped_base_curve_renders_as_it_did`]
/// renders: a camera curve of the pre-v1.17.0 kind plus a few sliders.
const OLD_RECIPE_JSON: &str = r#"{"version":3,"base_curve":[[0.0,0.0],[0.1,0.13],[0.25,0.33],[0.5,0.6],[0.75,0.83],[1.0,1.0]],"exposure_ev":0.3,"contrast":12.0,"saturation":8.0}"#;

/// The develop of `raw` imported from an all-zero sidecar that names a camera
/// profile (「Adobe Standard」), with the import's calibration halves — the
/// photo's camera base curve, lens profile and as-shot anchor
/// (`pipeline::stamp_import_start`). The profile is colour data: where this
/// machine carries it, the base develop applies its tables; where it does
/// not, nothing runs, and the invariants below hold either way.
fn profile_named(raw: &std::path::Path) -> EditRecipe {
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Version="18.4" crs:ProcessVersion="15.4" crs:CameraProfile="Adobe Standard" crs:HasSettings="True">
</rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let mut r = crate::xmp::xmp_to_recipe(xmp);
    let fresh = crate::pipeline::calibration_recipe(crate::pipeline::fresh_photo_calibration(raw));
    (r.lens_profile, r.as_shot_k, r.as_shot_tint, r.version) =
        (fresh.lens_profile, fresh.as_shot_k, fresh.as_shot_tint, fresh.version);
    crate::pipeline::stamp_import_start(raw, &mut r, &crate::diag::photo(raw));
    r
}

/// The reverse fit's source of `raw` when its calibration is `cal`
/// (`pipeline::fit_source` for a photo whose saved develop is `cal`).
fn canvas_fit_source(raw: &std::path::Path, cal: &EditRecipe) -> (Plate, EditRecipe) {
    let plate = source_plate(raw, cal, crate::pipeline::FIT_SOURCE_EDGE).expect("fit source");
    (plate, cal.clone())
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
const OLD_RECIPE_P39_1000_DIGEST: u64 = 0xc5db_fee1_fdbe_f2dd;

/// v1.17.0's INVARIANT: a develop is one picture on every surface. An
/// imported develop naming a camera profile ([`profile_named`]), at 1000 px,
/// against its own export:
///
/// * the GUI and web canvas — the preview develop over the base develop
///   ([`base_develop`], as the GUI holds it, then the engine's finishing tail) — ΔE76 ≤ 0.5;
/// * a MASTER baked from that base (a heal / denoise / stack writes the base
///   develop's pixels) rendered under the recipe — ΔE76 ≤ 0.5;
/// * the reverse fit's SOURCE ([`crate::pipeline::fit_source`], the base
///   develop at the fit's 2048 edge) developed under its calibration through
///   the same tail — ΔE76 ≤ 0.5 against the export at that same edge, both
///   brought to the 1000 px grid by one filter.
///
/// Env-gated on the corpus; it prints every number it bounds.
///
/// MUTATION: develop any of the three bases neutrally while the export
/// applies the profile, or let the preview develop drop the camera curve
/// under a named profile.
#[test]
fn every_surface_shows_a_profile_naming_develop_as_the_export_renders_it() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("surface invariant", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p39.arw");
    if !raw.is_file() {
        crate::test_skipped("surface invariant", "the corpus lacks its RAW");
        return;
    }
    const EDGE: u32 = 1000;
    let named = profile_named(&raw);
    assert!(named.names_profile(), "premise: the sidecar names a camera profile");
    assert!(!named.base_curve.is_empty(), "an import starts from the photo's camera curve");
    let reference = render_to_image(&raw, &named, None, Some(EDGE)).expect("export");

    // 1. The canvas: the preview develop over the base develop, then the
    // engine's tail — `bin/gui/util.rs`'s `build_preview` and the web pane.
    let part = BasePart::of(&named);
    let base = base_develop(&raw, &part, Some(EDGE)).expect("base");
    let film = crate::decode::film_short_edge(&raw);
    let preview = |base: &DynamicImage, r: &EditRecipe, source: &SourceFrame| {
        let after = develop_preview_film(base, r, &crate::diag::pixels(), film, source);
        let geom = geometry_profile(r);
        let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
        frame_and_finish(after, r, tail_lens(&geom, source.corrected()), fs, CropPolicy::Cut)
    };
    let canvas = preview(&base, &named, &SourceFrame::of(&raw).expect("frame"));
    let de_canvas = mean_de76(&canvas, &reference);

    // 2. A master baked from the base and rendered under the recipe — the
    // baked door every master export takes.
    let master = master_develop(&raw, &part, Some(EDGE)).expect("master");
    let from_master =
        render_baked_to_image(&master, &named, None, None, None, false, &crate::diag::pixels()).expect("master render");
    let de_master = mean_de76(&from_master, &reference);

    // 3. The reverse fit's source under its calibration, at the fit's edge,
    // through the canvas's own tail.
    let (source, cal) = canvas_fit_source(&raw, &named);
    assert!(!source.corrected(), "a RAW's fit source is in the sensor frame");
    let fitted = preview(&DynamicImage::ImageRgb8(source.to_rgb8()), &cal, &SourceFrame::of(&raw).expect("frame"));
    let at_fit_edge = render_to_image(&raw, &named, None, Some(crate::pipeline::FIT_SOURCE_EDGE))
        .expect("export at the fit's edge");
    let to_grid = |im: &DynamicImage| {
        im.resize_exact(reference.width(), reference.height(), image::imageops::FilterType::Triangle)
    };
    let de_fit = mean_de76(&to_grid(&fitted), &to_grid(&at_fit_edge));
    println!(
        "surface invariant @{EDGE}: canvas ΔE76 {de_canvas:.3}; master ΔE76 {de_master:.3}; fit source \
         ΔE76 {de_fit:.3}"
    );
    assert!(de_canvas <= 0.5, "canvas ΔE76 {de_canvas:.3} (bound 0.5)");
    assert!(de_master <= 0.5, "master ΔE76 {de_master:.3} (bound 0.5)");
    assert!(de_fit <= 0.5, "fit source ΔE76 {de_fit:.3} (bound 0.5)");
}

/// A float frame (one that keeps its over-range light) thumbnails to the same
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

/// A camera base curve of the lantern pair's shape (v1.18.0, J151): it lifts a
/// mid-bright luma by a quarter, so a channel a fifth above its pixel's luma
/// passes white.
const LIFTING_BASE_JSON: &str =
    r#"{"version":3,"base_curve":[[0.0,0.0],[0.15,0.29],[0.33,0.58],[0.57,0.83],[0.81,0.96],[1.0,1.0]]}"#;

/// The fit's plate holds the pixels the export develops BEFORE its base's
/// pointwise stages — also where the base's curve lifts a channel past white
/// (v1.18.0, J151). The export's develop of the base clamps that channel, and
/// a plate decoded from the clamped frame alone holds less of it than the
/// photograph does, so a candidate that pulls the channel down (a warmer
/// white balance over an overcast sky) is rendered on less light than the
/// export has. Here a sky whose blue the curve cuts stands beside a grey it
/// cuts nothing of: the plate made from the develop's frame and the same
/// develop before the stages is the pixels themselves in both halves, and so
/// is its analysis thumbnail, the resample every candidate is rendered on.
///
/// MUTATION: let `BaseEncoding::encode` report no cut.
#[test]
fn a_fit_plate_keeps_the_light_its_base_curve_lifts_past_white() {
    let base: EditRecipe = serde_json::from_str(LIFTING_BASE_JSON).expect("the base parses");
    let encoding = super::canvas::BaseEncoding::of(&base).expect("a base curve is a pointwise stage");
    const SKY: [f32; 3] = [0.62, 0.72, 0.90];
    const GREY: [f32; 3] = [0.40, 0.40, 0.40];
    let (w, h) = (64u32, 48u32);
    let bare: Vec<[f32; 3]> = (0..w * h).map(|i| if i % w < w / 2 { SKY } else { GREY }).collect();
    // The export's develop of the base on those pixels, packed as the export packs it.
    let mut developed = bare.clone();
    apply_develop_anon(&mut developed, w as usize, h as usize, &base);
    let (sky, grey) = (developed[0], developed[(w - 1) as usize]);
    assert!(sky[2] >= 1.0 && sky[0] < 1.0 && sky[1] < 1.0, "premise: the base curve cuts the sky's blue, and only it: {sky:?}");
    assert!(grey.iter().all(|v| *v < 1.0), "premise: it cuts nothing of the grey: {grey:?}");
    let pack = |px: &[[f32; 3]]| {
        DynamicImage::ImageRgb16(ImageBuffer::from_fn(w, h, |x, y| Rgb(px[(y * w + x) as usize].map(to_u16))))
    };
    let plate = encoding.restore(&pack(&developed), &pack(&bare));
    let off = |img: &DynamicImage, x: u32, want: [f32; 3]| {
        let got = img.to_rgb32f().get_pixel(x, img.height() / 2).0;
        (0..3).map(|c| (got[c] - want[c]).abs()).fold(0.0f32, f32::max)
    };
    // A quarter of an 8-bit code.
    const NEAR: f32 = 1e-3;
    assert!(off(&plate, 4, SKY) < NEAR, "the plate's sky is off the pixels by {}", off(&plate, 4, SKY));
    assert!(off(&plate, w - 4, GREY) < NEAR, "the plate's grey is off the pixels by {}", off(&plate, w - 4, GREY));
    let small = Plate::thumbnail_of(&Plate::fit_plate(plate, false, &base), 16);
    assert_eq!((small.width(), small.height()), (16, 12));
    assert!(off(&small, 1, SKY) < NEAR, "the thumbnail's sky is off the pixels by {}", off(&small, 1, SKY));
    assert!(off(&small, 14, GREY) < NEAR, "the thumbnail's grey is off the pixels by {}", off(&small, 14, GREY));
}

/// The open forms of the plate encoding's two operators are the develop's own
/// wherever its white clamp cuts nothing, bit for bit — so what a clamp cut is
/// their difference, and exactly nothing elsewhere — and each is undone by its
/// inverse where the clamped form is not (v1.18.0, J151).
///
/// MUTATION: scale the open chroma in another order of operations
/// (`v / l_old * l_new`).
#[test]
fn the_open_operators_are_the_develops_own_inside_white() {
    let (w, h) = (33usize, 21usize);
    let gain: Vec<f32> = (0..64).map(|i| 1.0 + 0.6 * (i as f32 / 63.0).powi(2)).collect();
    let frame: Vec<[f32; 3]> =
        (0..w * h).map(|i| [0usize, 1, 2].map(|c| ((i * 7 + c * 13) % 101) as f32 / 100.0)).collect();
    let (mut clamped, mut open) = (frame.clone(), frame.clone());
    apply_radial_gain(&mut clamped, w, h, &gain);
    super::develop::apply_radial_gain_open(&mut open, w, h, &gain);
    let mut cut = 0;
    for (c, o) in clamped.iter().flatten().zip(open.iter().flatten()) {
        if o > c {
            cut += 1;
            assert!(*c > 0.9999, "a channel the open gain leaves above the develop's was cut at white: {c}");
        } else {
            assert_eq!(c.to_bits(), o.to_bits(), "inside white the two gains are one value");
        }
    }
    assert!(cut > 0, "premise: the gain lifts some channel past white");
    let inverse: Vec<f32> = gain.iter().map(|g| 1.0 / g).collect();
    super::develop::apply_radial_gain_open(&mut open, w, h, &inverse);
    let worst = open.iter().flatten().zip(frame.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1.0 / 255.0, "the inverse gain undoes the open pass: off by {worst}");

    let mut cut = 0;
    for (px, l_new) in [([0.62f32, 0.72, 0.90], 0.9f32), ([0.2, 0.5, 0.1], 0.6), ([0.9, 0.95, 1.0], 0.99), ([0.3, 0.3, 0.3], 0.1)] {
        let l_old = luma601(&px);
        let (mut c, mut o) = (px, px);
        scale_chroma(&mut c, l_old, l_new);
        super::luma::scale_chroma_open(&mut o, l_old, l_new);
        for ch in 0..3 {
            if o[ch] > 1.0 {
                cut += 1;
                assert_eq!(c[ch], 1.0, "a channel the open scale leaves above white was cut there");
            } else {
                assert_eq!(c[ch].to_bits(), o[ch].to_bits(), "inside white the two scales are one value");
            }
        }
        let l_open = luma601(&o);
        super::luma::scale_chroma_open(&mut o, l_open, l_old);
        let worst = (0..3).map(|ch| (o[ch] - px[ch]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "the inverse ratio undoes the open scale: off by {worst}");
    }
    assert!(cut > 0, "premise: a ratio lifts some channel past white");
}

/// The lantern street (p38), the photograph J151 was found on: its camera
/// base curve cuts the blue of the overcast sky, and the fit made the white
/// balance warmer, which pulls that blue down. On the pixels whose blue the
/// export of the base holds at white, the fit's preview of a warmer white
/// balance (as-shot x 1.5, the size of the move the fit made) reads the blue
/// the export of that recipe renders: 0.12 codes apart in the
/// mean, where a plate decoded from the clamped frame alone reads
/// 12.52 codes under it (the mutation's reading, 2026-11-13). Both at the fit's
/// source edge with the lens geometry left for later, the frame the plate is
/// in. Env-gated on the corpus; it prints what it bounds.
///
/// MUTATION: let `BaseEncoding::encode` report no cut (the plate is then
/// decoded from the develop's frame alone).
#[test]
fn the_fit_previews_a_warmer_white_balance_as_the_export_renders_it_where_the_base_cuts_a_channel() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from)
    else {
        crate::test_skipped("plate white clamp", "AUTOSHADE_FIT_CALIBRATION_DIR is unset");
        return;
    };
    let raw = dir.join("p38.arw");
    if !raw.is_file() {
        crate::test_skipped("plate white clamp", "the corpus lacks its RAW");
        return;
    }
    let (plate, cal) = crate::pipeline::fit_source(&raw).expect("fit source");
    let mut warm = cal.clone();
    warm.temperature_k = Some(cal.as_shot_k.expect("a RAW's calibration carries its as-shot white") * 1.5);
    let export = |r: &EditRecipe| {
        let mut r = r.clone();
        (r.lens_profile.distortion_on, r.lens_profile.ca_on) = (false, false);
        render_to_image(&raw, &r, None, Some(crate::pipeline::FIT_SOURCE_EDGE)).expect("export").to_rgb8()
    };
    let (at_base, exported) = (export(&cal), export(&warm));
    let previewed = develop_preview(&plate, &warm).to_rgb8();
    assert_eq!(previewed.dimensions(), exported.dimensions(), "the plate is in the export's frame");
    let cut: Vec<bool> = at_base.pixels().map(|p| p[2] == 255).collect();
    let share = cut.iter().filter(|c| **c).count() as f64 / cut.len() as f64;
    let mean_blue = |img: &RgbImage, on: bool| {
        let (sum, n) = img
            .pixels()
            .zip(&cut)
            .filter(|(_, c)| **c == on)
            .fold((0.0f64, 0usize), |(s, n), (p, _)| (s + f64::from(p[2]), n + 1));
        sum / n.max(1) as f64
    };
    let gap = mean_blue(&previewed, true) - mean_blue(&exported, true);
    let rest = mean_blue(&previewed, false) - mean_blue(&exported, false);
    println!(
        "plate white clamp (p38): the base holds the blue of {:.2} % of the frame at white; there the preview's \
         mean blue is {gap:+.2} codes off the export's ({:.1} against {:.1}), elsewhere {rest:+.2}",
        100.0 * share,
        mean_blue(&previewed, true),
        mean_blue(&exported, true)
    );
    assert!(share > 0.01, "premise: the base's develop cuts the sky's blue ({:.2} % of the frame)", 100.0 * share);
    assert!(gap.abs() <= 1.0, "the preview's blue is {gap:+.2} codes off the export's where the base cut it (bound 1.0)");
}
