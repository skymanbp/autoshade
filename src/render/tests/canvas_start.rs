// One part of the engine's tests (src/render/tests.rs includes it): the canvas's base develop (v1.17.0) — the base part, the fresh start, and the invariant that every surface showing a fresh canvas shows the export.

/// The base part keeps exactly what the profile stage reads — the camera
/// profile and the Look's identity and table — and nothing of the Look's
/// baked half, which the preview develop renders from the full recipe; a
/// recipe that names no profile has the neutral part, whose base is the
/// neutral develop every base was before v1.17.0.
///
/// MUTATION: keep the Look's tone curve in the part (it would render twice),
/// or give a profile-less Look a non-neutral part (a needless re-develop).
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
    assert_eq!(BasePart::default().recipe(), EditRecipe::default(), "the neutral develop's recipe");
}

/// A recipe saved before v1.17.0 — a stamped camera base curve, no profile —
/// renders BYTE-IDENTICALLY to the build before the canvas switch (the era
/// rule: a saved develop keeps the start it was tuned on). The digest was
/// read off this same render (`render_to_image`, 16-bit, 1000 px) at c30a6b5,
/// the v1.17.0 step-3 commit, before any of the canvas work; this build must
/// reproduce it. Env-gated on the corpus (`p39.arw`).
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
const OLD_RECIPE_P39_1000_DIGEST: u64 = 0x6e65_09a7_6132_1a09;

/// v1.17.0 step 2's INVARIANT: a fresh canvas is one picture on every surface.
/// The all-zero fresh recipe (the calibration a fresh open stamps) against the
/// engine's render of the all-zero Lightroom sidecar naming 「Adobe Standard」
/// with 「Adobe Color」, at 1000 px:
///
/// * the EXPORT of the fresh recipe — the same code path — byte-identical;
/// * the GUI and web canvas — the preview develop over the base develop
///   ([`base_develop`], 8-bit, then the engine's finishing tail) — ΔE76 ≤ 0.5;
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
    let base = DynamicImage::ImageRgb8(base_develop(&raw, &part, Some(EDGE)).expect("base").to_rgb8());
    let film = crate::decode::film_short_edge(&raw);
    let preview = |base: &DynamicImage, r: &EditRecipe| {
        let after = develop_preview_film(base, r, &crate::diag::pixels(), film, true);
        let geom = geometry_profile(r);
        let fs = FilmScale::of(film, after.width() as usize, after.height() as usize);
        frame_and_finish(after, r, &geom, fs, CropPolicy::Cut)
    };
    let canvas = preview(&base, &fresh);
    let de_canvas = mean_de76(&canvas, &reference);

    // 3. A master baked from the base and rendered under the recipe — the
    // baked door every master export takes.
    let master = base_develop(&raw, &part, Some(EDGE)).expect("master");
    let from_master =
        render_baked_to_image(&master, &fresh, None, None, &crate::diag::pixels()).expect("master render");
    let de_master = mean_de76(&from_master, &reference);

    // 4. The reverse fit's source under its calibration, at the fit's edge,
    // through the canvas's own tail (the fit source is the uncorrected
    // sensor frame; the lens geometry is the tail's).
    let (source, cal) = crate::pipeline::fit_source(&raw).expect("fit source");
    let fit_has_canvas = BasePart::of(&cal) == part;
    let fitted = preview(&DynamicImage::ImageRgb8(source.to_rgb8()), &cal);
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
