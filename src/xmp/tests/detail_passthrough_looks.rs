// One part of the sidecar's tests (src/xmp/tests.rs includes it): local mask scale, radial feather, texture and detail round trips, defringe, pass-through blocks and creative looks.

#[test]
fn renders_local_masks_with_correct_scale() {
    let r = EditRecipe {
        masks: vec![
            LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                name: "sky".into(),
                exposure_ev: -0.4,  // ÷4 → -0.1
                highlights: -50.0,  // ÷100 → -0.5
                ..Default::default()
            },
            LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                    feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                    midpoint: 50.0, mask_version: 2,
                },
                name: "subject".into(),
                shadows: 20.0,      // ÷100 → 0.2
                inverted: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    // Write it out so well-formedness can be validated by an XML parser
    // while debugging — under the temp dir, never the working directory:
    // leaving `out/_masks_test.xmp` behind on every `cargo test` was a
    // side effect on the tree. Verification aid, not a behavioural
    // assertion; removed again at the end.
    let dir = std::env::temp_dir()
        .join(format!("autoshade-masks-xml-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(dir.join("_masks_test.xmp"), &xmp).ok();
    assert!(xmp.contains("<crs:MaskGroupBasedCorrections>"));
    assert!(xmp.contains(r#"crs:What="Mask/Gradient""#));
    assert!(xmp.contains(r#"crs:What="Mask/CircularGradient""#));
    // local scale conversions
    assert!(xmp.contains(r#"crs:LocalExposure2012="-0.1""#)); // -0.4 / 4
    assert!(xmp.contains(r#"crs:LocalHighlights2012="-0.5""#)); // -50 / 100
    assert!(xmp.contains(r#"crs:LocalShadows2012="0.2""#)); // 20 / 100
    assert!(xmp.contains(r#"crs:MaskInverted="true""#));
    assert!(xmp.contains(r#"crs:ZeroX="0.5""#));
    // Feather crosses the boundary on Lightroom's 0..100 scale (engine 0.5
    // → crs 50) — the old writer's raw "0.5" read in LR as a hard edge.
    assert!(xmp.contains(r#"crs:Feather="50""#));
    // unset masks ⇒ no mask block (v1-compatible)
    assert!(!recipe_to_xmp(&EditRecipe::default()).contains("MaskGroupBasedCorrections"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn radial_feather_converts_both_ways_and_keeps_legacy_own_scale() {
    // LR-style integer feather imports onto the engine's 0..1 scale…
    //
    // R25 P9: wrapped in a real `<rdf:li …/>`. It used to be a bare
    // attribute run, which worked only because the geometry was located by
    // substring; `base_geometry_at` scans TAGS, because choosing the base
    // out of several components means reading each component's own
    // `crs:MaskBlendMode` and that is a per-tag question. Every production
    // caller already hands whole markup — `classify_correction`, the only
    // one, tag-scans the same block itself — so the fixture was the thing
    // that did not look like a sidecar.
    //
    // R27 Batch-4 took the same step again, one level out: the component
    // list is now located by NAME (`crs:CorrectionMasks`) rather than
    // scanned for anywhere in the segment, because "which components are
    // this correction's own" is the question hazards 1 and 2 turned on.
    // `classify_correction` has always required that element — it returns
    // `Unrepresentable` without one — so the two now agree about where a
    // correction's components live, and the fixture wears the wrapper its
    // production caller always supplies.
    let li = r#"<crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/CircularGradient" crs:Top="0.2" crs:Left="0.2" crs:Bottom="0.8" crs:Right="0.8" crs:Feather="72" crs:Roundness="0" crs:Flipped="false"/></rdf:Seq></crs:CorrectionMasks>"#;
    // The correction's own scope is empty here BY CONSTRUCTION: this
    // fixture is a bare component list with no correction tag around it,
    // so there are no sliders to read and the geometry is the whole claim.
    let m = parse_one_correction(li, Scope::new(""), None).expect("radial parses");
    let MaskGeometry::Radial { feather, .. } = m.mask else { panic!("radial") };
    assert!((feather - 0.72).abs() < 1e-6, "LR 72 → 0.72, got {feather}");
    // …while a legacy own-writer value (≤ 1.0) passes through verbatim.
    let legacy = li.replace(r#"crs:Feather="72""#, r#"crs:Feather="0.4""#);
    let m =
        parse_one_correction(&legacy, Scope::new(""), None).expect("legacy radial parses");
    let MaskGeometry::Radial { feather, .. } = m.mask else { panic!("radial") };
    assert!((feather - 0.4).abs() < 1e-6, "legacy 0.4 stays 0.4, got {feather}");
}

#[test]
fn renders_manual_vignette_only_when_set() {
    let r = EditRecipe { lens_vignette: 35.0, lens_vignette_mid: 60.0, ..Default::default() };
    let xmp = recipe_to_xmp(&r);
    assert!(xmp.contains(r#"crs:VignetteAmount="+35""#));
    assert!(xmp.contains(r#"crs:VignetteMidpoint="60""#));
    // A neutral recipe emits neither key (byte-compatible with the old writer).
    let neutral = recipe_to_xmp(&EditRecipe::default());
    assert!(!neutral.contains("VignetteAmount") && !neutral.contains("VignetteMidpoint"));
}

#[test]
fn renders_manual_distortion_only_when_set() {
    let r = EditRecipe { lens_distortion: -24.0, ..Default::default() };
    assert!(recipe_to_xmp(&r).contains(r#"crs:LensManualDistortionAmount="-24""#));
    let pos = EditRecipe { lens_distortion: 80.0, ..Default::default() };
    assert!(recipe_to_xmp(&pos).contains(r#"crs:LensManualDistortionAmount="+80""#));
    // Zero amount emits no key at all (byte-compatible with the old writer).
    assert!(!recipe_to_xmp(&EditRecipe::default()).contains("LensManualDistortionAmount"));
}

/// R25 B2: global Texture round-trips through its own `crs:Texture` key,
/// in Lightroom's own signed form.
///
/// READ AND WRITE IN ONE BATCH is not a nicety here. `owned_attr_keys` is
/// the WRITER's universe and also the merge's STRIP universe, so a
/// read-only Texture would have left Lightroom's value in the document
/// beside ours (two answers for one slider), and a write-only one would
/// have gone on being named by `unmodelled_global_crs` while quietly
/// rendering. Either half alone is a defect; this pins both.
#[test]
fn texture_round_trips_through_xmp() {
    let xmp = recipe_to_xmp(&EditRecipe { texture: 26.0, ..Default::default() });
    assert!(xmp.contains(r#"crs:Texture="+26""#), "the signed form Lightroom writes: {xmp}");
    assert_eq!(xmp_to_recipe(&xmp).texture, 26.0);
    // Negative and neutral, and the key is UNCONDITIONAL like its four
    // Basic-panel neighbours (Clarity2012 / Dehaze / Vibrance / Saturation).
    let neg = recipe_to_xmp(&EditRecipe { texture: -40.0, ..Default::default() });
    assert!(neg.contains(r#"crs:Texture="-40""#));
    assert_eq!(xmp_to_recipe(&neg).texture, -40.0);
    assert!(recipe_to_xmp(&EditRecipe::default()).contains(r#"crs:Texture="0""#));
    // A FOREIGN sidecar's Texture is a real import, not a disclosure line.
    let lr = "<rdf:Description rdf:about=\"\" \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                  crs:Texture=\"+26\"/>";
    assert_eq!(xmp_to_recipe(lr).texture, 26.0, "the LR value must arrive");
}

/// R25 B2, the strip arm: a cleared Texture must DISAPPEAR from a merged
/// document rather than linger at Lightroom's old value.
///
/// This is what owning a key means. Before B2, `crs:Texture` was foreign
/// property and the merge preserved it verbatim (there are four tests
/// above that used it as the example of exactly that). Now the merge
/// strips it and rewrites ours — and if `owned_attr_keys` had gained the
/// key without the writer emitting it, or the writer without the key, this
/// document would answer one slider twice.
#[test]
fn a_cleared_texture_disappears_from_a_merged_document() {
    let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                  <rdf:Description rdf:about=\"\"\n    \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                  crs:Texture=\"+20\" crs:PointColor=\"0\" crs:HasSettings=\"True\">\n  \
                  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
    let merged = merged_doc(lr, &EditRecipe { texture: -8.0, ..Default::default() })
        .expect("a plain LR sidecar is mergeable");
    assert_eq!(merged.matches("crs:Texture=").count(), 1, "one answer only: {merged}");
    assert!(merged.contains(r#"crs:Texture="-8""#), "…and it is OURS: {merged}");
    assert!(merged.contains("crs:PointColor=\"0\""), "an unmodelled global still survives");
    // Cleared to neutral: the old +20 is gone, not resurrected.
    let cleared = merged_doc(lr, &EditRecipe::default()).expect("mergeable");
    assert_eq!(cleared.matches("crs:Texture=").count(), 1);
    assert!(cleared.contains(r#"crs:Texture="0""#), "the stale +20 must not linger: {cleared}");
    assert_eq!(xmp_to_recipe(&cleared).texture, 0.0);
}

/// R25 B3: the eight carried DETAIL axes and the manual CA pair
/// round-trip, each in the spelling Lightroom itself uses.
///
/// The spellings are FIRST-HAND, from all seven sidecars in the user's
/// library: `SharpenRadius="+1.0"` carries an explicit sign and one
/// decimal, while every integer neighbour is bare (`SharpenDetail="25"`,
/// `SharpenEdgeMasking="0"`, `ColorNoiseReduction="25"`). Getting that
/// backwards is not cosmetic — it is the difference between a sidecar
/// Lightroom reads as its own and one it merely tolerates.
#[test]
fn detail_subcontrols_round_trip() {
    let r = EditRecipe {
        sharpen_radius: 1.0,
        sharpen_detail: 25.0,
        sharpen_mask: 12.0,
        nr_detail: 50.0,
        nr_contrast: 8.0,
        color_nr: 25.0,
        color_nr_detail: 50.0,
        color_nr_smooth: 50.0,
        ca_r: 14.0,
        ca_b: -9.0,
        auto_lateral_ca: true,
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    for want in [
        r#"crs:SharpenRadius="+1.0""#,
        r#"crs:SharpenDetail="25""#,
        r#"crs:SharpenEdgeMasking="12""#,
        r#"crs:LuminanceNoiseReductionDetail="50""#,
        r#"crs:LuminanceNoiseReductionContrast="8""#,
        r#"crs:ColorNoiseReduction="25""#,
        r#"crs:ColorNoiseReductionDetail="50""#,
        r#"crs:ColorNoiseReductionSmoothness="50""#,
        r#"crs:ChromaticAberrationR="14""#,
        r#"crs:ChromaticAberrationB="-9""#,
        r#"crs:AutoLateralCA="1""#,
    ] {
        assert!(xmp.contains(want), "{want} missing from: {xmp}");
    }
    let back = xmp_to_recipe(&xmp);
    for (name, live, want) in [
        ("sharpen_radius", back.sharpen_radius, r.sharpen_radius),
        ("sharpen_detail", back.sharpen_detail, r.sharpen_detail),
        ("sharpen_mask", back.sharpen_mask, r.sharpen_mask),
        ("nr_detail", back.nr_detail, r.nr_detail),
        ("nr_contrast", back.nr_contrast, r.nr_contrast),
        ("color_nr", back.color_nr, r.color_nr),
        ("color_nr_detail", back.color_nr_detail, r.color_nr_detail),
        ("color_nr_smooth", back.color_nr_smooth, r.color_nr_smooth),
        ("ca_r", back.ca_r, r.ca_r),
        ("ca_b", back.ca_b, r.ca_b),
    ] {
        assert_eq!(live, want, "{name} did not survive the round trip");
    }
    assert!(back.auto_lateral_ca, "the auto-CA flag must come back on");
    // A neutral recipe writes NONE of them: an absent key is how
    // Lightroom is told to keep its own default (Radius 1.0, Detail 25,
    // Colour NR Detail/Smoothness 50/50), and inventing a zero for each
    // would be a change to the photo, not a faithful silence. The ONE
    // exception is `ColorNoiseReduction` itself (v1.3.1, `amount_carries`):
    // a recipe's zero is colour noise reduction OFF, which is what this
    // engine renders, and an absent key let Lightroom apply its RAW default
    // of 25 to a photo AutoShade showed without it (measured 2026-09-12).
    let neutral = recipe_to_xmp(&EditRecipe::default());
    for key in [
        "SharpenRadius",
        "SharpenDetail",
        "SharpenEdgeMasking",
        "LuminanceNoiseReduction",
        "ColorNoiseReductionDetail",
        "ColorNoiseReductionSmoothness",
        "ChromaticAberration",
        "AutoLateralCA",
    ] {
        assert!(!neutral.contains(key), "{key} must be absent from a neutral sidecar");
    }
    assert!(
        neutral.contains(r#"crs:ColorNoiseReduction="0""#),
        "colour NR goes out at the engine's value even at rest: {neutral}"
    );
    // …and the merge STRIPS them, so a cleared value cannot linger at
    // Lightroom's old number beside ours.
    let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                  <rdf:Description rdf:about=\"\"\n    \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                  crs:SharpenRadius=\"+1.0\" crs:ColorNoiseReduction=\"25\" \
                  crs:AutoLateralCA=\"1\" crs:HasSettings=\"True\">\n  \
                  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
    assert_eq!(xmp_to_recipe(lr).color_nr, 25.0, "premise: the LR values import");
    assert!(xmp_to_recipe(lr).auto_lateral_ca, "premise: so does the flag");
    let cleared = merged_doc(lr, &EditRecipe::default()).expect("a plain LR sidecar merges");
    for key in ["SharpenRadius", "AutoLateralCA"] {
        assert!(!cleared.contains(key), "{key} survived a clear: {cleared}");
    }
    // …and the colour NR is REPLACED, not merely stripped: one answer,
    // and it is the engine's zero, never Lightroom's stale 25.
    assert_eq!(cleared.matches("crs:ColorNoiseReduction=").count(), 1, "one answer: {cleared}");
    assert!(cleared.contains(r#"crs:ColorNoiseReduction="0""#), "ours: {cleared}");
}

/// v1.6.0: the Sharpening amount is a companion — absent from the sidecar
/// while the recipe holds no value, so Lightroom applies its own default
/// for the kind of file (40 on a RAW, 0 on a JPEG), which is what the
/// engine renders (`EditRecipe::capture_sharpening`); a real 0 is stated.
///
/// MUTATIONS THIS CATCHES: the writer emitting `Sharpness="0"` for an
/// absent amount again (Lightroom then renders a RAW unsharpened that the
/// app shows at 40); the reader folding a stated `Sharpness="0"` into
/// "absent" (Lightroom's own 0 imports as 40 on a RAW).
#[test]
fn an_absent_sharpening_amount_leaves_the_sidecar_to_lightroom_and_a_real_zero_is_stated() {
    let absent = recipe_to_xmp(&EditRecipe::default());
    assert!(!absent.contains("crs:Sharpness="), "no amount, no key: {absent}");
    let fresh = xmp_to_recipe(&absent);
    assert!(!fresh.explicit_zero.iter().any(|n| n == "sharpening"), "{:?}", fresh.explicit_zero);
    assert_eq!((fresh.capture_sharpening(true), fresh.capture_sharpening(false)), (40.0, 0.0));

    let mut zero = EditRecipe::default();
    zero.set_resolved("sharpening", 0.0);
    let stated = recipe_to_xmp(&zero);
    assert!(stated.contains(r#"crs:Sharpness="0""#), "{stated}");
    let back = xmp_to_recipe(&stated);
    assert_eq!(back.explicit_zero, vec!["sharpening".to_string()]);
    assert_eq!((back.capture_sharpening(true), back.capture_sharpening(false)), (0.0, 0.0));

    // A stated amount is itself on both kinds, as before.
    let forty = xmp_to_recipe(&recipe_to_xmp(&EditRecipe { sharpening: 40.0, ..Default::default() }));
    assert_eq!((forty.capture_sharpening(true), forty.capture_sharpening(false)), (40.0, 40.0));
}

/// v1.5.0: a COMPANION key Lightroom states AT 0 is a value, an absent one
/// is Lightroom's default — the difference `explicit_zero` holds, in both
/// directions of the sidecar.
///
/// MUTATIONS THIS CATCHES: the reader's explicit-zero pass removed (an LR
/// Detail 0 imports as "absent" and renders at 25); the writer's `states`
/// reduced to `v != 0.0` (our real 0 leaves the sidecar and Lightroom
/// renders its default); the era gate's `untouched` blind to the list (a
/// legacy recipe's real 0 is suppressed on save).
#[test]
fn a_companion_stated_at_zero_is_a_value_and_an_absent_one_is_lightrooms_default() {
    let lr = "<rdf:Description rdf:about=\"\" \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                  crs:Sharpness=\"40\" crs:SharpenRadius=\"+1.0\" crs:SharpenDetail=\"0\" \
                  crs:LuminanceSmoothing=\"30\" crs:LuminanceNoiseReductionDetail=\"50\" \
                  crs:GrainAmount=\"20\" crs:GrainSize=\"0\"/>";
    let r = xmp_to_recipe(lr);
    assert_eq!(
        r.explicit_zero,
        vec!["grain_size".to_string(), "sharpen_detail".to_string()],
        "exactly the companions stated at 0"
    );
    assert_eq!(r.resolved("sharpen_detail"), 0.0, "Lightroom's Detail 0 renders at 0");
    assert_eq!(r.resolved("nr_detail"), 50.0, "a stated default is itself");
    assert_eq!(r.resolved("color_nr_detail"), 50.0, "an absent one renders at the default");

    // Written back: the real zeros go out as zeros, the absent stays absent.
    let xmp = recipe_to_xmp(&r);
    assert!(xmp.contains(r#"crs:SharpenDetail="0""#), "{xmp}");
    assert!(xmp.contains(r#"crs:GrainSize="0""#), "{xmp}");
    assert!(!xmp.contains("ColorNoiseReductionDetail"), "never an invented zero: {xmp}");
    assert_eq!(xmp_to_recipe(&xmp).explicit_zero, r.explicit_zero, "and it reads back the same");

    // A LEGACY recipe (schema era 0) still writes a real zero the
    // photographer set: the era gate suppresses only what serde filled.
    let mut legacy = EditRecipe { schema_era: 0, sharpening: 40.0, ..Default::default() };
    legacy.set_resolved("sharpen_detail", 0.0);
    let written = recipe_to_xmp(&legacy);
    assert!(written.contains(r#"crs:SharpenDetail="0""#), "{written}");
    assert!(!written.contains("SharpenRadius"), "the absent radius stays absent: {written}");
}

/// v0.31.1: `crs:Sharpness` is Lightroom's Detail > Sharpening **Amount**
/// slider stored 1:1, and that slider's UI maximum is **150**, not 100.
///
/// EVIDENCE (web survey of GitHub-hosted `.xmp`, 2026-08-18; 566
/// `crs:Sharpness` occurrences across 636 files, histogram maximum 150):
/// fifteen REAL sidecars carry `crs:Sharpness="150"` — fourteen from
/// `maxbordogna/finger_counting` (`tiff:Model="NIKON Z 6"`,
/// `crs:Version="15.3"` / `ProcessVersion="11.0"`) and one from
/// `ninjahisser/Fotos` (`NIKON Z 30`, `crs:Version="17.2"` /
/// `ProcessVersion="15.4"`), the latter with the value sitting inside the
/// Detail attribute group beside `SharpenRadius="+1.0"` /
/// `SharpenDetail="25"` / `SharpenEdgeMasking="0"` in a file that names its
/// own raw. Two repositories, two camera bodies, two Lightroom
/// generations. The sidecar values are also copied here as TEXT ONLY —
/// no harvested file enters this repository.
///
/// The 0..100 belief was load-bearing in four places, all deleted with it:
/// the reader's ×1.5, the writer's ×⅔, the `Sharpness` special case in
/// `crs_number_is_in_recipe_range`, and an assertion that PINNED
/// `100 × 1.5 == 150` as an invariant. This test is the replacement pin —
/// it fails on every one of those four.
#[test]
fn a_full_lightroom_sharpening_amount_imports_as_itself() {
    // The maximum a real Lightroom writes. Synthetic document, real value.
    let doc = |v: &str| {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF \
                 xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                 <rdf:Description rdf:about=\"\"\n    \
                 xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                 crs:Sharpness=\"{v}\" crs:SharpenRadius=\"+1.0\" crs:SharpenDetail=\"25\"\n    \
                 crs:SharpenEdgeMasking=\"0\" crs:HasSettings=\"True\">\n  \
                 </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n"
        )
    };
    // 1. A full 150 arrives as 150. Under the old reader it was 225,
    //    clamped back to 150 by luck — and under the old BAND it was also
    //    reported as an unparsable number, i.e. the app told the user a
    //    perfectly ordinary Lightroom file was broken.
    let full = xmp_to_recipe(&doc("150"));
    assert_eq!(full.sharpening, 150.0, "a full Lightroom Amount is 150 here too");
    assert!(
        unparsable_crs_numbers(&doc("150")).is_empty(),
        "150 is an ordinary value, not a defect: {:?}",
        unparsable_crs_numbers(&doc("150"))
    );
    // 2. The user's own library value. 40 in, 40 out — it used to be 60.
    assert_eq!(xmp_to_recipe(&doc("40")).sharpening, 40.0);
    // 3. WYSIWYG on the way back out: what the engine renders is what the
    //    sidecar says. A rendered 60 used to be written back as 40.
    let sixty = EditRecipe { sharpening: 60.0, ..Default::default() };
    assert!(
        recipe_to_xmp(&sixty).contains(r#"crs:Sharpness="60""#),
        "{}",
        recipe_to_xmp(&sixty)
    );
    // …and 150 survives a full round trip, which the old ×⅔ writer could
    // not do at all: it had no way to SAY 150.
    assert!(recipe_to_xmp(&full).contains(r#"crs:Sharpness="150""#));
    assert_eq!(xmp_to_recipe(&recipe_to_xmp(&full)).sharpening, 150.0);
    // 4. The band still has a ceiling, and it is the row's: 151 is out.
    assert!(
        unparsable_crs_numbers(&doc("151")).iter().any(|k| k == "Sharpness"),
        "past the slider's own maximum is still disclosed"
    );
}

/// R25 B3 (policy SF4-C): de-fringe round-trips through BOTH spellings
/// Lightroom writes — the `rdf:Description` attribute form and the
/// property-ELEMENT form.
///
/// `crs_str` already reads both (that is why the reader adds no third
/// scanning arm), and this is the test that keeps it that way. Positive
/// amounts are UNSIGNED: `DefringePurpleAmount="3"`, never `"+3"` — the
/// `Sharpness="40"` family, not the `Contrast2012="+22"` one.
#[test]
fn defringe_round_trips_both_serialization_forms() {
    let r = EditRecipe {
        defringe_purple: 3.0,
        defringe_purple_lo: 39.0,
        defringe_purple_hi: 79.0,
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    assert!(xmp.contains(r#"crs:DefringePurpleAmount="3""#), "unsigned, like Sharpness: {xmp}");
    assert!(!xmp.contains(r#"DefringePurpleAmount="+3""#), "no `+` on this family");
    let back = xmp_to_recipe(&xmp);
    assert_eq!(
        (back.defringe_purple, back.defringe_purple_lo, back.defringe_purple_hi),
        (3.0, 39.0, 79.0)
    );
    // The ELEMENT form, in the wild on photoprism's canon_eos_6d fixture
    // family (alphabetical, one child per key).
    let elem = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                    <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                    <rdf:Description rdf:about=\"\"\n    \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\n   \
                    <crs:DefringeGreenAmount>5</crs:DefringeGreenAmount>\n   \
                    <crs:DefringeGreenHueHi>66</crs:DefringeGreenHueHi>\n   \
                    <crs:DefringeGreenHueLo>44</crs:DefringeGreenHueLo>\n   \
                    <crs:DefringePurpleAmount>7</crs:DefringePurpleAmount>\n  \
                    </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
    let e = xmp_to_recipe(elem);
    assert_eq!(
        (e.defringe_green, e.defringe_green_lo, e.defringe_green_hi, e.defringe_purple),
        (5.0, 44.0, 66.0, 7.0),
        "the element form must read exactly like the attribute form"
    );
    // The purple WINDOW was not named by that document — it must fall
    // back to Adobe's default, not to the zero `crs_f32` answers.
    assert_eq!(
        (e.defringe_purple_lo, e.defringe_purple_hi),
        (30.0, 70.0),
        "an unnamed hue window is Adobe's default, never 0..0"
    );
}

/// R25 B3: a NON-default hue window survives — the case the fallback
/// above must not swallow.
///
/// 39/79 is the real shape from a Lightroom preset (`lightA1`): the
/// amount at 3 with the window moved off 30/70. If the reader ever
/// "normalised" a window it did not recognise, this is what would be
/// silently rewritten to Adobe's default on the next save.
#[test]
fn nondefault_hue_bounds_survive() {
    let lr = "<rdf:Description rdf:about=\"\" \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                  crs:DefringePurpleAmount=\"3\" crs:DefringePurpleHueLo=\"39\" \
                  crs:DefringePurpleHueHi=\"79\"/>";
    let r = xmp_to_recipe(lr);
    assert_eq!((r.defringe_purple_lo, r.defringe_purple_hi), (39.0, 79.0));
    let round = xmp_to_recipe(&recipe_to_xmp(&r));
    assert_eq!(
        (round.defringe_purple, round.defringe_purple_lo, round.defringe_purple_hi),
        (3.0, 39.0, 79.0),
        "a moved window must not snap back to 30/70 on a save"
    );
    // The green half of the same document said nothing, so it comes back
    // as Adobe's 40/60 — and that is what gets written, which is the
    // shape every real sidecar has.
    assert_eq!((round.defringe_green_lo, round.defringe_green_hi), (40.0, 60.0));
    assert!(recipe_to_xmp(&r).contains(r#"crs:DefringeGreenHueLo="40""#));
}

/// R25 B3: the whole de-fringe block is written UNCONDITIONALLY, and a
/// document carrying exactly Adobe's defaults imports as a NO-OP.
///
/// Both halves of the non-zero-neutral decision in one place. The first
/// is the shape Lightroom writes — 7 of 7 of the user's sidecars carry
/// all six keys with the amounts at 0 — so a recipe that says nothing
/// still produces a document that looks like one Lightroom made. The
/// second is what stops that from making every photo "edited": the six
/// values are `EditRecipe::default()`'s own, so `is_noop` still answers
/// yes. A reader that took `crs_f32`'s absent-key zero instead would
/// import a 0..0 hue window and fail this, which is exactly the bug the
/// fallback exists to prevent.
#[test]
fn a_real_defringe_block_imports_as_a_noop() {
    let neutral = recipe_to_xmp(&EditRecipe::default());
    for want in [
        r#"crs:DefringePurpleAmount="0""#,
        r#"crs:DefringePurpleHueLo="30""#,
        r#"crs:DefringePurpleHueHi="70""#,
        r#"crs:DefringeGreenAmount="0""#,
        r#"crs:DefringeGreenHueLo="40""#,
        r#"crs:DefringeGreenHueHi="60""#,
    ] {
        assert!(neutral.contains(want), "{want} missing — the block is unconditional: {neutral}");
    }
    // The real shape, verbatim from the user's library (P50 line 139
    // onward), on an otherwise empty document.
    let real = "<rdf:Description rdf:about=\"\" \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                    crs:DefringePurpleAmount=\"0\" crs:DefringePurpleHueLo=\"30\" \
                    crs:DefringePurpleHueHi=\"70\" crs:DefringeGreenAmount=\"0\" \
                    crs:DefringeGreenHueLo=\"40\" crs:DefringeGreenHueHi=\"60\"/>";
    assert!(
        xmp_to_recipe(real).is_noop(),
        "a sidecar carrying only Adobe's own de-fringe defaults is not an edit"
    );
    // …and so is a document that never mentions de-fringe at all — the
    // OTHER direction of the same fallback.
    let silent = "<rdf:Description rdf:about=\"\" \
                      xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"/>";
    assert!(xmp_to_recipe(silent).is_noop(), "no de-fringe keys is no edit either");
    // A REAL de-fringe, by contrast, is one.
    let edited = real.replace("crs:DefringePurpleAmount=\"0\"", "crs:DefringePurpleAmount=\"3\"");
    assert!(!xmp_to_recipe(&edited).is_noop(), "an actual de-fringe IS an edit");
}

/// R25 B3, the complement arm: the detail block, the CA pair and
/// de-fringe have all LEFT `unmodelled_global_crs` — with no edit to that
/// function, because its universe is the complement of `owned_attr_keys`.
///
/// This is the same "the list shrinks by itself" property B2 proved for
/// Texture, and it is the reason the writer and the reader had to land in
/// one commit: a key we read but never write would still be foreign
/// property, named here and duplicated by the merge.
#[test]
fn unmodelled_list_no_longer_names_the_detail_block() {
    let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                  <rdf:Description rdf:about=\"\"\n    \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                  crs:SharpenRadius=\"+1.0\" crs:SharpenDetail=\"25\" \
                  crs:SharpenEdgeMasking=\"0\" crs:LuminanceNoiseReductionDetail=\"50\" \
                  crs:LuminanceNoiseReductionContrast=\"0\" crs:ColorNoiseReduction=\"25\" \
                  crs:ColorNoiseReductionDetail=\"50\" crs:ColorNoiseReductionSmoothness=\"50\" \
                  crs:AutoLateralCA=\"1\" crs:ChromaticAberrationR=\"0\" \
                  crs:ChromaticAberrationB=\"0\" crs:DefringePurpleAmount=\"3\" \
                  crs:DefringePurpleHueLo=\"39\" crs:DefringePurpleHueHi=\"79\" \
                  crs:DefringeGreenAmount=\"0\" crs:DefringeGreenHueLo=\"40\" \
                  crs:DefringeGreenHueHi=\"60\" crs:PointColor=\"0\" \
                  crs:HasSettings=\"True\">\n  \
                  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
    let found = unmodelled_global_crs(lr);
    for gone in [
        "SharpenRadius",
        "SharpenDetail",
        "SharpenEdgeMasking",
        "LuminanceNoiseReductionDetail",
        "LuminanceNoiseReductionContrast",
        "ColorNoiseReduction",
        "ColorNoiseReductionDetail",
        "ColorNoiseReductionSmoothness",
        "AutoLateralCA",
        "ChromaticAberrationR",
        "ChromaticAberrationB",
        "DefringePurpleAmount",
        "DefringePurpleHueLo",
        "DefringePurpleHueHi",
        "DefringeGreenAmount",
        "DefringeGreenHueLo",
        "DefringeGreenHueHi",
    ] {
        assert!(
            !found.contains(&gone.to_string()),
            "{gone} is modelled since R25 B3 and must have left the list: {found:?}"
        );
    }
    // The premise: the scan really did run and still names what we do
    // NOT model, or the seventeen assertions above prove nothing.
    assert!(
        found.contains(&"PointColor".to_string()),
        "an unmodelled global must still be named: {found:?}"
    );
    // …and the values arrived rather than merely stopping being foreign.
    let r = xmp_to_recipe(lr);
    assert_eq!((r.sharpen_radius, r.color_nr, r.defringe_purple), (1.0, 25.0, 3.0));
    assert!(r.auto_lateral_ca);
}

// ───────────────────── R25 B4: the pass-through blocks ──────────────────

/// A Lightroom Transform block, its Upright solver's own bookkeeping, and a
/// camera profile — synthetic, but every value below is copied CHARACTER
/// FOR CHARACTER out of the operator's reference sidecars (a bare `0`, a
/// decimal `0.00`, a signed `+0.9`, a plain `100`, a negative `-35`, two
/// nine-digit normalised fractions, a focal length past the registry's
/// fallback band, and a profile NAME with a space in it: nine different
/// spellings of things a number formatter would flatten into three).
///
/// FIXTURE NOTE, F6 REVISION. The eight `crs:Perspective*` keys are OWNED
/// controls since v1.5.0, so they are no longer this document's
/// pass-through sample — the six `Upright*` keys beside them are, and
/// Lightroom writes those on every photo its Upright panel has touched.
/// Keeping the Perspective keys here is the point: the same document now
/// exercises both sides of the line.
fn lr_transform_doc() -> String {
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
         xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" \
         xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
         crs:Exposure2012=\"+1.00\" \
         crs:PerspectiveUpright=\"0\" crs:PerspectiveVertical=\"-35\" \
         crs:PerspectiveHorizontal=\"0\" crs:PerspectiveRotate=\"+0.9\" \
         crs:PerspectiveScale=\"100\" crs:PerspectiveAspect=\"0\" \
         crs:PerspectiveX=\"0.00\" crs:PerspectiveY=\"0.00\" \
         crs:UprightVersion=\"151388160\" crs:UprightCenterMode=\"0\" \
         crs:UprightCenterNormX=\"0.422764964\" \
         crs:UprightCenterNormY=\"0.46112117\" \
         crs:UprightFocalMode=\"0\" \
         crs:UprightFocalLength35mm=\"104.944461871\" \
         crs:CameraProfile=\"Adobe Standard\" \
         crs:CameraProfileDigest=\"2D1D4700365C3E2831EEAE0D1A8F9CDF\" \
         crs:HasSettings=\"True\"/></rdf:RDF></x:xmpmeta>"
        .to_string()
}

/// **The first real payload `Tier::PassThrough` has ever carried**
/// (`ARCHITECTURE.md` recorded the tier as "none yet" until this batch).
/// The registry's three-sided law only checks that a ROW exists; this
/// checks that the value survives the round trip as ITSELF.
#[test]
fn passthrough_round_trips_verbatim() {
    let r = xmp_to_recipe(&lr_transform_doc());
    assert_eq!(
        r.passthrough.len(),
        6,
        "the six Upright bookkeeping keys, and nothing else: {:?}",
        r.passthrough
    );
    // VERBATIM means the spelling too. Every one of these would have been
    // destroyed by a number round trip: nine significant digits do not
    // survive an f32 format, 104.944461871 is outside the registry's
    // fallback band, and `Adobe Standard` is not a number at all.
    for (key, want) in [
        ("UprightVersion", "151388160"),
        ("UprightCenterNormX", "0.422764964"),
        ("UprightCenterNormY", "0.46112117"),
        ("UprightFocalLength35mm", "104.944461871"),
    ] {
        assert_eq!(r.passthrough.get(key).map(String::as_str), Some(want), "{key}");
    }
    // The profile NAME crossed the same line in F7, and its spelling
    // matters for the same reason: it is an identifier that has to keep
    // matching the file it names.
    assert_eq!(r.camera_profile, "Adobe Standard");
    assert!(!r.passthrough.contains_key("CameraProfile"), "an owned key is not carried");
    // The eight Perspective keys crossed the line in v1.5.0 F6: OWNED, so
    // they are parsed into their own fields and are NOT in this map.
    for owned in ["PerspectiveVertical", "PerspectiveRotate", "PerspectiveX"] {
        assert!(!r.passthrough.contains_key(owned), "{owned} is an owned control now");
    }
    assert_eq!(r.perspective_vertical, -35.0, "…and it arrived as a number");
    assert_eq!(r.perspective_rotate, 0.9, "…including the signed one");
    // The keys OUTSIDE the named nine are untouched by all of this:
    // the digest stays foreign, preserved by the merge and named by the
    // import disclosure. "Named set, not everything unknown."
    assert!(!r.passthrough.contains_key("CameraProfileDigest"));
    assert!(
        unmodelled_global_crs(&lr_transform_doc()).contains(&"CameraProfileDigest".to_string())
    );

    // Out and back, through OUR writer.
    let ours = recipe_to_xmp(&r);
    assert!(ours.contains(r#"crs:UprightFocalLength35mm="104.944461871""#), "{ours}");
    assert!(ours.contains(r#"crs:CameraProfile="Adobe Standard""#), "{ours}");
    assert_eq!(xmp_to_recipe(&ours).passthrough, r.passthrough, "a full verbatim round trip");
    // The owned half round-trips too, in its own measured spellings.
    assert!(ours.contains(r#"crs:PerspectiveRotate="+0.9""#), "{ours}");
    assert!(ours.contains(r#"crs:PerspectiveVertical="-35""#), "{ours}");

    // Written in PASSTHROUGH_CRS order, not the BTreeMap's alphabetical
    // one: Adobe writes its solver's block before the profile name, and a
    // diff against Lightroom's own file has to be readable.
    let at = |k: &str| ours.find(&format!("crs:{k}=")).unwrap_or_else(|| panic!("{k} missing"));
    assert!(at("UprightVersion") < at("UprightFocalLength35mm"), "the block keeps its order");
    assert!(at("UprightFocalLength35mm") < at("CameraProfile"), "block before the profile name");

    // XML transport still applies — escaping is not interpretation, and a
    // profile name really can carry an ampersand.
    let odd = EditRecipe {
        camera_profile: "Sky & Sea <v2>".to_string(),
        ..Default::default()
    };
    let doc = recipe_to_xmp(&odd);
    assert!(doc.contains("Sky &amp; Sea &lt;v2&gt;"), "escaped on the way out: {doc}");
    assert_eq!(
        xmp_to_recipe(&doc).camera_profile,
        "Sky & Sea <v2>",
        "…and unescaped back to the very same string"
    );
}

/// The empty map is the state of every recipe written before this batch,
/// and it must change NOTHING: no invented Transform block in the sidecar,
/// and an older `recipe.json` with no such key still reads.
#[test]
fn an_empty_passthrough_map_leaves_the_sidecar_bytes_unchanged() {
    let doc = recipe_to_xmp(&EditRecipe::default());
    for key in PASSTHROUGH_CRS {
        assert!(
            !doc.contains(&format!("crs:{key}=")),
            "{key}: a recipe that never saw a Transform block must not assert one: {doc}"
        );
    }
    // Forward/backward compatibility of the FIELD, which is the other
    // half of "unchanged": a recipe.json from v0.30 has no `passthrough`
    // key at all and must still load, as an empty map.
    let legacy = r#"{"version":2,"exposure_ev":0.25}"#;
    let r: EditRecipe = serde_json::from_str(legacy).expect("a legacy recipe still parses");
    assert!(r.passthrough.is_empty());
    assert_eq!(r.exposure_ev, 0.25);
}

/// The merge's own trap, and the reason [`merge_strip_keys`] exists: a
/// recipe that never SAW a Transform block must not delete one.
///
/// Owning a key normally licenses the strip — that is how a cleared
/// vignette disappears. Pass-through has no cleared state: nothing in the
/// app can empty the map, so "absent" only ever means "this recipe came
/// from somewhere that never read the document" (a v0.30 recipe.json, a
/// paste from another photo, a fresh Analyze). Stripping on that would
/// delete the photographer's Upright correction and camera profile from
/// the file beside their RAW on an ordinary Ctrl+S.
///
/// v1.5.0 F6 SPLIT THIS TEST IN TWO, because the block it is named after
/// now has two halves with two different protections:
///
/// * the six Upright bookkeeping keys are still carried, and
///   `merge_strip_keys` is still what saves them — case (a). The profile
///   NAME left that half in F7 and is protected differently again: it is
///   owned, and an empty name means "not stated" rather than "cleared"
///   (`unspoken_attr_keys`);
/// * the eight `crs:Perspective*` keys are OWNED, so a build that has them
///   overwrites Lightroom's exactly as it overwrites `crs:Exposure2012`,
///   and what protects a recipe written before they existed is the SCHEMA
///   ERA, not the strip list — case (a2).
///
/// Reading the second half as a regression would have been the easy
/// mistake: an owned key that never overwrites is a control that renders
/// nothing.
#[test]
fn a_recipe_that_never_saw_a_transform_block_does_not_delete_one() {
    let lr = lr_transform_doc();
    // (a) The dangerous case: empty map, real Upright bookkeeping in the
    // base. THIS is what the strip list protects.
    let blind = EditRecipe { exposure_ev: 0.25, ..Default::default() };
    let merged = merged_doc(&lr, &blind).expect("mergeable");
    for want in [
        r#"crs:UprightCenterNormX="0.422764964""#,
        r#"crs:UprightFocalLength35mm="104.944461871""#,
        r#"crs:CameraProfile="Adobe Standard""#,
    ] {
        assert!(merged.contains(want), "the base's own {want} must survive: {merged}");
    }
    assert_eq!(merged.matches("crs:CameraProfile=").count(), 1, "and exactly once");
    assert!(merged.contains(r#"crs:Exposure2012="0.25""#), "…while ours still publish");
    // …and the OWNED half published ours, which is what owning means. The
    // recipe is era 2, so it has an opinion about the keystone: none.
    assert_eq!(blind.schema_era, crate::recipe::SCHEMA_ERA, "premise: a current build");
    assert!(
        !merged.contains(r#"crs:PerspectiveVertical="-35""#),
        "an era-2 recipe states its own Transform, like its own Exposure: {merged}"
    );

    // (a2) The same dangerous case for the owned half: a recipe written
    // BEFORE v1.5.0 has never held a Perspective key, so the era gate
    // neither strips nor re-emits one and Lightroom's own bytes stand.
    let era1 = EditRecipe { exposure_ev: 0.25, schema_era: 1, ..Default::default() };
    let merged = merged_doc(&lr, &era1).expect("mergeable");
    for want in [
        r#"crs:PerspectiveVertical="-35""#,
        r#"crs:PerspectiveRotate="+0.9""#,
        r#"crs:CameraProfile="Adobe Standard""#,
    ] {
        assert!(merged.contains(want), "an era-1 recipe must leave {want} alone: {merged}");
    }
    assert!(merged.contains(r#"crs:Exposure2012="0.25""#), "…while an era-0 key still does");

    // (b) The ordinary case: the recipe DID read the block, so ours are
    // stripped and rewritten — one copy, never two. Probed on a key that
    // is OFF NEUTRAL in the document, because a neutral one has nothing to
    // rewrite and would prove the claim by accident.
    let seen = EditRecipe { exposure_ev: 0.25, ..xmp_to_recipe(&lr) };
    let merged = merged_doc(&lr, &seen).expect("mergeable");
    assert_eq!(merged.matches("crs:CameraProfile=").count(), 1, "stripped, then rewritten");
    assert_eq!(merged.matches("crs:PerspectiveVertical=").count(), 1, "the owned half too");
    assert!(merged.contains(r#"crs:PerspectiveVertical="-35""#), "{merged}");
    assert!(merged.contains(r#"crs:CameraProfile="Adobe Standard""#));
    // …and the neutral one leaves NO key, which is what every owned control
    // at rest does (it is how a cleared vignette disappears). Harmless
    // here, and measured rather than assumed: this recipe's own default is
    // 100 and `xmp_to_recipe` reads an absent `PerspectiveScale` back as
    // 100, so the document renders the same in both apps with the key gone.
    assert_eq!(merged.matches("crs:PerspectiveScale=").count(), 0, "at rest, so not restated");
    assert_eq!(xmp_to_recipe(&merged).perspective_scale, 100.0, "…and absent still reads 100");
    // (c) …and a CHANGED value replaces rather than duplicates. Probed on
    // the OWNED profile name (v1.5.0 F7), because that is now the key with
    // both a strip and a write behind it — the carried half above is copied
    // verbatim and could never duplicate by a formatting difference.
    let mut edited = seen.clone();
    edited.camera_profile = "Adobe Landscape".to_string();
    let merged = merged_doc(&lr, &edited).expect("mergeable");
    assert_eq!(merged.matches("crs:CameraProfile=").count(), 1);
    assert!(merged.contains(r#"crs:CameraProfile="Adobe Landscape""#), "{merged}");
    assert!(!merged.contains("Adobe Standard\""), "the old value is gone: {merged}");
    // …and an UNSTATED name is not a cleared one: a recipe holding no
    // profile leaves Lightroom's own alone rather than deleting it.
    let mut silent = seen.clone();
    silent.camera_profile.clear();
    let kept = merged_doc(&lr, &silent).expect("mergeable");
    assert!(
        kept.contains(r#"crs:CameraProfile="Adobe Standard""#),
        "an empty name must not delete the photographer's profile: {kept}"
    );
}

/// The regenerate path — the one that "carries none of the base's
/// properties". It carries these: the seven live in the RECIPE now, so a
/// document rebuilt from scratch still states them. (`pipeline`'s
/// regeneration note names the creative `Look` instead of the camera
/// profile for exactly this reason.)
///
/// Both halves of the F6 block, because after v1.5.0 they reach a fresh
/// document by two different routes and the same assertion would have
/// passed on either one alone: the carried keys ride the `passthrough`
/// map, the eight `crs:Perspective*` keys ride their own owned fields.
#[test]
fn passthrough_survives_a_regenerate() {
    let r = xmp_to_recipe(&lr_transform_doc());
    let fresh = recipe_to_xmp(&r); // no base document at all
    assert!(fresh.contains(r#"crs:CameraProfile="Adobe Standard""#), "{fresh}");
    assert!(fresh.contains(r#"crs:UprightFocalLength35mm="104.944461871""#), "{fresh}");
    assert_eq!(xmp_to_recipe(&fresh).passthrough, r.passthrough);
    // The owned half, by its own route: a number this time, formatted by
    // the writer rather than copied as a string.
    assert!(fresh.contains(r#"crs:PerspectiveVertical="-35""#), "{fresh}");
    assert_eq!(xmp_to_recipe(&fresh).perspective_rotate, r.perspective_rotate);
}

/// Both spellings, one scanner. `crs_str` already reads the
/// property-ELEMENT form, so the reader needed no second scan arm (R24
/// round-end MED-1: a third arm is how the two forms drift apart) — and
/// the SCOPE rule matters more here than anywhere: a creative Look nests
/// its own baked `crs:CameraProfile`, and a flat scan would import the
/// PROFILE's name as the photographer's choice.
///
/// The profile name is an OWNED control since v1.5.0 F7, so the assertion
/// reads `camera_profile` rather than the carried map — the same question
/// about the same scope, asked of the field that now answers it.
#[test]
fn passthrough_reads_the_element_form_and_never_the_nested_look() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <crs:CameraProfile>Adobe Standard</crs:CameraProfile>\
                   <crs:UprightFocalLength35mm>104.944461871</crs:UprightFocalLength35mm>\
                   <crs:Look><rdf:Description><crs:Parameters><rdf:Description>\
                   <crs:CameraProfile>Camera Landscape</crs:CameraProfile>\
                   <crs:UprightFocalLength35mm>999</crs:UprightFocalLength35mm>\
                   </rdf:Description></crs:Parameters></rdf:Description></crs:Look>\
                   </rdf:Description></rdf:RDF></x:xmpmeta>";
    let r = xmp_to_recipe(doc);
    assert_eq!(
        r.camera_profile,
        "Adobe Standard",
        "the Description's OWN profile, never the Look's baked one"
    );
    assert_eq!(
        r.passthrough.get("UprightFocalLength35mm").map(String::as_str),
        Some("104.944461871")
    );
    // A document with no such block reports none — absence stays absence.
    assert!(xmp_to_recipe("<rdf:Description \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Exposure2012=\"0.00\"/>")
        .passthrough
        .is_empty());
}

/// A creative Look, as Lightroom 9.4 actually writes one.
///
/// Trimmed VERBATIM from the library's "Adobe Landscape" files — the shape
/// with baked sliders, which is the one that exercises every field. The
/// three `ToneCurvePV2012Red/Green/Blue` children Adobe writes beside the
/// master curve are identity on every file measured and are left out here
/// so the master curve's own assertion cannot pass by reading a sibling.
const LOOK_DOC: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
         xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" \
         xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
         crs:Exposure2012=\"+1.00\" crs:Clarity2012=\"+4\" \
         crs:CameraProfile=\"Adobe Standard\">\
         <crs:Look>\
          <rdf:Description crs:Name=\"Adobe Landscape\" crs:Amount=\"1\" \
           crs:UUID=\"6F9C877E84273F4E8271E6B91BEB36A1\" \
           crs:SupportsMonochrome=\"false\">\
          <crs:Group><rdf:Alt><rdf:li xml:lang=\"x-default\">Profiles</rdf:li>\
          </rdf:Alt></crs:Group>\
          <crs:Parameters>\
           <rdf:Description crs:Version=\"18.0\" crs:ProcessVersion=\"15.4\" \
            crs:Highlights2012=\"-12\" crs:Shadows2012=\"+12\" \
            crs:Clarity2012=\"+10\" crs:ConvertToGrayscale=\"False\" \
            crs:CameraProfile=\"Adobe Standard\" \
            crs:LookTable=\"0B3BFB5CFB7DBF7FF175E98F24D316B0\">\
           <crs:ToneCurvePV2012><rdf:Seq>\
            <rdf:li>0, 0</rdf:li><rdf:li>64, 60</rdf:li>\
            <rdf:li>128, 128</rdf:li><rdf:li>192, 196</rdf:li>\
            <rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>\
           </rdf:Description>\
          </crs:Parameters>\
          </rdf:Description>\
         </crs:Look>\
         </rdf:Description></rdf:RDF></x:xmpmeta>";

/// The creative Look is READ — its baked half reaches the recipe, and its
/// baked half only.
///
/// The line this pins is the one the whole F7 model rests on: the Look's
/// `Clarity2012="+10"` is the PROFILE's, the Description's own `"+4"` is
/// the PHOTOGRAPHER's, and they are two different numbers living in two
/// different fields. A reader that flattened the document would hand the
/// profile's value to the slider and show the photographer a clarity they
/// never set.
///
/// MUTATION: read the Look through the Description's scope, drop any field
/// from `read_creative_look`, or let an absent `crs:Amount` mean 0.
#[test]
fn a_creative_look_is_read_for_its_baked_half_and_nothing_else() {
    let r = xmp_to_recipe(LOOK_DOC);
    let look = r.look.as_ref().expect("the document carries a Look");
    assert_eq!(look.name, "Adobe Landscape");
    assert_eq!(look.amount, 1.0);
    assert_eq!(look.base_profile, "Adobe Standard");
    assert_eq!(look.table, "0B3BFB5CFB7DBF7FF175E98F24D316B0");
    assert!(!look.grayscale, "this Look is a colour one");
    assert_eq!((look.highlights, look.shadows, look.clarity), (-12.0, 12.0, 10.0));
    assert_eq!(look.tone_curve.len(), 5, "the baked curve: {:?}", look.tone_curve);
    assert_eq!(look.tone_curve[1], CurvePoint { input: 64, output: 60 });
    assert!(look.red_curve.is_empty() && look.blue_curve.is_empty(), "absent stays absent");

    // …and the two halves do not leak into each other.
    assert_eq!(r.clarity, 4.0, "the photographer's own clarity, not the profile's");
    assert_eq!(r.highlights, 0.0, "the profile's baked -12 is NOT a slider the user set");
    assert!(r.tone_curve.is_empty(), "…and its baked curve is not the user's curve");
    assert_eq!(r.camera_profile, "Adobe Standard");

    // The table is the one thing that cannot be rendered, so it is NAMED.
    assert_eq!(look.unrendered, vec!["LookTable".to_string()], "{:?}", look.unrendered);
    assert!(!look.is_neutral(), "a Look with baked moves is not neutral");
}

/// A document with no Look carries none, and a Look with nothing baked is
/// neutral rather than absent.
///
/// The difference matters downstream: `baked_look` filters on neutrality,
/// so a profile that only names a colour table must not reach the tone
/// composition and silently replace the engine's own base curve with an
/// empty one.
///
/// MUTATION: return `Some(Default)` for a document with no Look, or drop
/// the neutrality filter in `EditRecipe::baked_look`.
#[test]
fn a_look_with_nothing_baked_is_neutral_and_no_look_at_all_is_none() {
    let bare = LOOK_DOC
        .replace("crs:Highlights2012=\\\"-12\\\" ", "")
        .replace("crs:Shadows2012=\\\"+12\\\" ", "")
        .replace("crs:Clarity2012=\\\"+10\\\" ", "");
    let bare = bare
        .replace("crs:Highlights2012=\"-12\" ", "")
        .replace("crs:Shadows2012=\"+12\" ", "")
        .replace("crs:Clarity2012=\"+10\" ", "");
    let stripped = {
        let start = bare.find("<crs:ToneCurvePV2012>").expect("the curve");
        let end = bare.find("</crs:ToneCurvePV2012>").expect("the curve end")
            + "</crs:ToneCurvePV2012>".len();
        format!("{}{}", &bare[..start], &bare[end..])
    };
    let r = xmp_to_recipe(&stripped);
    let look = r.look.as_ref().expect("the Look element is still there");
    assert!(look.is_neutral(), "nothing baked: {look:?}");
    assert!(r.baked_look().is_none(), "a neutral Look does not reach the render");
    assert_eq!(look.table, "0B3BFB5CFB7DBF7FF175E98F24D316B0", "…but its table is still named");

    // An ABSENT `crs:Amount` is the whole Look, not none of it. The two
    // readings are one token apart and they differ by everything: amount 0
    // makes `is_neutral` true, so a profile that bakes a real curve would
    // be switched off silently rather than rendered. Probed here because
    // neutrality is exactly what an amount of 0 would forge.
    let no_amount = LOOK_DOC.replace(" crs:Amount=\"1\"", "");
    assert!(!no_amount.contains("crs:Amount"), "the attribute really went: {no_amount}");
    let silent = xmp_to_recipe(&no_amount);
    let unstated = silent.look.as_ref().expect("a Look with no stated amount is still a Look");
    assert_eq!(unstated.amount, 1.0, "absent means the whole Look");
    assert!(!unstated.is_neutral(), "…so its baked curve still renders");
    assert!(silent.baked_look().is_some(), "…and still reaches the render");

    // No Look element at all.
    let none = xmp_to_recipe(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
             xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Exposure2012=\"0.00\"/></rdf:RDF></x:xmpmeta>",
    );
    assert!(none.look.is_none(), "no element, no Look");
    assert!(!none.renders_grayscale(), "and no black-and-white either");
}

/// A monochrome creative profile renders as black and white.
///
/// This is how Lightroom 9.4 spells a B&W treatment — four of the 175
/// sidecars measured carry `crs:Look` "Adobe Monochrome" whose Parameters
/// hold `ConvertToGrayscale="True"`, while the Description's own switch
/// stays absent. An engine that read only the top-level switch rendered
/// those four photographs in colour.
///
/// MUTATION: read `convert_to_grayscale` alone in `renders_grayscale`.
#[test]
fn a_monochrome_look_renders_black_and_white() {
    let mono = LOOK_DOC
        .replace("crs:ConvertToGrayscale=\"False\"", "crs:ConvertToGrayscale=\"True\"")
        .replace("Adobe Landscape", "Adobe Monochrome");
    let r = xmp_to_recipe(&mono);
    assert!(r.look.as_ref().expect("a Look").grayscale, "the Look's own switch");
    assert!(!r.convert_to_grayscale, "the photographer never touched theirs");
    assert!(r.renders_grayscale(), "…and the render still has to turn grey");
    // The photographer's own switch is still enough on its own.
    let own = EditRecipe { convert_to_grayscale: true, ..Default::default() };
    assert!(own.renders_grayscale(), "either switch, not both");
}

/// A pass-through value is never "unparsable", because it is never parsed.
///
/// The trap this closes: the pass-through keys joined `owned_attr_keys`, and
/// that list IS `unparsable_crs_numbers`' universe — with a ±100 fallback
/// band for any key the registry states no range for. Without the
/// exemption, `crs:UprightFocalLength35mm="104.944461871"` (a real 105 mm
/// prime in this library, well outside ±100) would have been reported as a
/// value that "imports as a silent neutral" — about the one block in the
/// recipe that has no neutral and is never replaced.
///
/// `crs:CameraProfile` is here for the SECOND reason (v1.5.0 F7): it is an
/// owned control now, and a control's exemption comes from its registry
/// SHAPE. A name is not a number whichever list it sits on.
///
/// The sample used to be `crs:PerspectiveX="-140"`; v1.5.0 F6 owns that key
/// with a stated band, so an out-of-band value there IS worth reporting now
/// and the sample moved to a key that is still carried.
#[test]
fn a_passthrough_value_is_never_called_unparsable() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                   crs:CameraProfile=\"Adobe Standard\" \
                   crs:UprightFocalLength35mm=\"104.944461871\" \
                   crs:UprightVersion=\"151388160\" crs:Contrast2012=\"+22\" \
                   crs:HasSettings=\"True\"/></rdf:RDF></x:xmpmeta>";
    assert!(unparsable_crs_numbers(doc).is_empty(), "{:?}", unparsable_crs_numbers(doc));
    // The premise, so the emptiness above is not emptiness for another
    // reason: the same scan still names an OWNED number that is off its
    // band, in the very same document.
    let bad = doc.replace(r#"crs:Contrast2012="+22""#, r#"crs:Contrast2012="+220""#);
    assert_eq!(unparsable_crs_numbers(&bad), vec!["Contrast2012"]);
    // …and the values still arrive, out of band and all.
    let r = xmp_to_recipe(doc);
    assert_eq!(
        r.passthrough.get("UprightFocalLength35mm").map(String::as_str),
        Some("104.944461871")
    );
    assert_eq!(r.passthrough.get("UprightVersion").map(String::as_str), Some("151388160"));
}


/// v1.17.0: a recipe that develops through a creative profile carries it to
/// Lightroom. A FRESH document writes the Look Lightroom's way and reads back
/// as the same Look; a merge into a base WITHOUT a Look adds it; a base WITH
/// one keeps its own, once.
///
/// MUTATION: drop `look_elem` from the fresh document; append it in a merge
/// whatever the base holds.
#[test]
fn a_creative_look_reaches_lightroom_from_a_fresh_document_and_a_lookless_merge() {
    let look = crate::xmp::canvas_look();
    assert_eq!(look.name, "Adobe Color");
    assert_eq!(look.table, "E1095149FDB39D7A057BAB208837E2E1");
    assert_eq!(look.tone_curve.len(), 7, "{:?}", look.tone_curve);
    let r = EditRecipe {
        camera_profile: "Adobe Standard".into(),
        look: Some(look.clone()),
        ..Default::default()
    };
    let doc = recipe_to_xmp(&r);
    assert_eq!(doc.matches("<crs:Look>").count(), 1, "{doc}");
    let back = xmp_to_recipe(&doc);
    let got = back.look.as_ref().expect("the Look reads back");
    assert_eq!((&got.name, got.amount, &got.base_profile, &got.table), (&look.name, look.amount, &look.base_profile, &look.table));
    assert_eq!(got.tone_curve, look.tone_curve);
    assert_eq!((&got.red_curve, &got.green_curve, &got.blue_curve), (&look.red_curve, &look.green_curve, &look.blue_curve));
    assert_eq!(back.camera_profile, "Adobe Standard");
    // A Lightroom base with no Look gains this one…
    let bare = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
         xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" \
         xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
         crs:ProcessVersion=\"15.4\" crs:Exposure2012=\"+0.50\"/>\
         </rdf:RDF></x:xmpmeta>";
    let merged = crate::xmp::merge_recipe_into_xmp(bare, &r).expect("merges").doc;
    assert_eq!(merged.matches("<crs:Look>").count(), 1, "{merged}");
    assert_eq!(xmp_to_recipe(&merged).look.map(|l| l.table), Some(look.table.clone()));
    // …and one that has its own keeps it, verbatim and alone.
    let merged = crate::xmp::merge_recipe_into_xmp(LOOK_DOC, &r).expect("merges").doc;
    assert_eq!(merged.matches("<crs:Look>").count(), 1, "{merged}");
    assert_eq!(xmp_to_recipe(&merged).look.map(|l| l.name), Some("Adobe Landscape".to_string()));
    // No Look, no element.
    assert!(!recipe_to_xmp(&EditRecipe::default()).contains("<crs:Look>"));
}
