// One part of the sidecar's tests (src/xmp/tests.rs includes it): global and parametric round trips, foreign sidecars, hostile text, attribute-form curves and mask groups, foreign prefixes and bounded input.

#[test]
fn globals_round_trip_through_xmp() {
    // Values are chosen to survive the writer's documented rounding: integer
    // sliders (`signed()`), 2-decimal exposure, integer Kelvin, 1-decimal
    // straighten, %.6f crop — so the reader must land EXACTLY back.
    let r = EditRecipe {
        exposure_ev: 0.32,
        contrast: 14.0,
        highlights: -12.0,
        shadows: 25.0,
        whites: 8.0,
        blacks: -6.0,
        temperature_k: Some(5600.0),
        tint: 3.0,
        vibrance: 18.0,
        saturation: -5.0,
        clarity: 10.0,
        dehaze: 7.0,
        hsl: crate::recipe::Hsl {
            hue: [0.0, 15.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            saturation: [0.0, 0.0, 0.0, -40.0, 0.0, 0.0, 0.0, 0.0],
            ..Default::default()
        },
        color_grade: crate::recipe::ColorGrade {
            shadow_hue: 220.0,
            shadow_sat: 30.0,
            highlight_hue: 45.0,
            highlight_sat: 20.0,
            midtone_lum: -10.0,
            balance: 15.0,
            ..Default::default()
        },
        // → crs 45 → back as 45. Exact for a different reason than before
        // v0.31.1: not "45 happens to survive ×⅔ then ×1.5", but "there is
        // no scale in either direction any more".
        sharpening: 45.0,
        noise_reduction: 20.0,
        lens_vignette: 35.0,
        lens_vignette_mid: 60.0,
        lens_distortion: -24.0,
        straighten_deg: 1.5,
        crop: Some(Crop { left: 0.05, top: 0.0, right: 0.95, bottom: 1.0 }),
        tone_curve: vec![
            CurvePoint { input: 0, output: 8 },
            CurvePoint { input: 255, output: 247 },
        ],
        red_curve: vec![
            CurvePoint { input: 0, output: 10 },
            CurvePoint { input: 255, output: 250 },
        ],
        rationale: "warm & contrasty <test> & \"q\"".into(),
        confidence: 0.82,
        ..Default::default()
    };
    let back = xmp_to_recipe(&recipe_to_xmp(&r));
    assert_eq!(back, r);
}

#[test]
fn as_shot_tint_round_trips_only_for_our_own_sidecars() {
    // Our writer emits a non-neutral Tint even under "As Shot"; the AutoShade
    // marker tells the reader it is a real edit.
    let r = EditRecipe { tint: 3.0, ..Default::default() };
    assert_eq!(xmp_to_recipe(&recipe_to_xmp(&r)).tint, 3.0);
}

#[test]
fn parametric_masks_round_trip_geometry_and_original_inversion_homes() {
    let r = EditRecipe {
        masks: vec![
            LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                range: Some(RangeMask::Luminance { lo_outer: 0.4, lo: 0.5, hi: 1.0, hi_outer: 1.0 }),
                name: "sky & sea".into(),
                amount: 0.75,
                inverted: true,
                exposure_ev: -0.4, // ÷4 → ×4 is a power-of-two rescale: exact
                contrast: 30.0,    // "0.3" ×100 needs the 4-decimal snap: exact
                highlights: -50.0,
                shadows: 60.0,
                whites: 10.0,
                blacks: -20.0,
                clarity: 40.0,
                dehaze: 5.0,
                texture: 15.0,
                // R23-1b: two keys the writer used to emit as a literal
                // "0". They ride the same ÷100 ↔ ×100 pair as their
                // neighbours, and a sidecar carrying them must still import
                // as loss-free — `correction_value_reasons` demanded 0
                // for both until R23-1b, so a non-zero one would have
                // refused the whole correction and this equality would
                // fail on every other field too.
                sharpness: -45.0,
                saturation: 20.0,
                hue: 35.0,
                temperature: 25.0,
                tint: -10.0,
                noise_reduction: 30.0,
                ..Default::default()
            },
            LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                    feather: 0.5, roundness: 0.0, flipped: true, angle: 0.0,
                    midpoint: 50.0, mask_version: 2,
                },
                range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 0.5, px: 0.4, py: 0.7 }),
                name: "subject".into(),
                shadows: 20.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let back = xmp_to_recipe(&recipe_to_xmp(&r));
    // R25 P9: ONE field does not come back where it went in. Lightroom
    // spells a radial's inversion ONCE (`crs:MaskInverted`, with
    // `crs:Flipped` as its complement) where this recipe spells it as the
    // XOR of two flags, so the projection collapses `flipped` into
    // `inverted`. The XOR is the whole of what the pixels see (render.rs
    // `mask_weight` and the weight loop) and it survives exactly; WHICH of
    // our two flags carries it is not a fact about the photograph.
    // Written as an expected value rather than a relaxed comparison so
    // every other field still has to match to the bit.
    let mut expect = r.masks.clone();
    // …and ONE more, from v0.32.0: the radial's box no longer passes
    // through verbatim. It goes out through the inverse of Lightroom's
    // frame affine and comes back through the affine, and the WIRE carries
    // six decimals — Lightroom's own precision, which is the precision any
    // value that has ever been through Lightroom actually has. So the
    // corners settle by up to half a wire step, measured here at
    // 4.6 × 10⁻⁷ of the frame = 0.005 px on a 9504 px export. It is a
    // one-time settle onto the wire grid, not a drift: the second round
    // trip is exact, which the re-emit below is what pins.
    let approx = |a: &MaskGeometry, b: &MaskGeometry| match (a, b) {
        (
            MaskGeometry::Radial { top: t1, left: l1, bottom: b1, right: r1, .. },
            MaskGeometry::Radial { top: t2, left: l2, bottom: b2, right: r2, .. },
        ) => [(t1, t2), (l1, l2), (b1, b2), (r1, r2)]
            .iter()
            .all(|(x, y)| (**x - **y).abs() < 1e-6),
        _ => false,
    };
    assert!(approx(&back.masks[1].mask, &expect[1].mask), "{:?}", back.masks[1].mask);
    expect[1].mask = back.masks[1].mask.clone();
    assert_eq!(back.masks, expect);
    // The wire grid is a FIXED POINT, not a ratchet: once a box has been
    // through the projection its own re-emit reproduces it to the bit.
    assert_eq!(
        xmp_to_recipe(&recipe_to_xmp(&back)).masks[1].mask,
        back.masks[1].mask,
        "the second round trip is exact"
    );
    for (i, (was, now)) in r.masks.iter().zip(&back.masks).enumerate() {
        assert_eq!(
            lr_net_inverted(was),
            lr_net_inverted(now),
            "mask {i}: the net inversion is the part that must survive"
        );
    }
}

#[test]
fn bitmap_masks_come_back_only_through_the_payload() {
    // The writer skips raster corrections (no classic-XMP encoding), so the
    // crs reading must return only the parametric mask — never a phantom.
    let mixed = mixed_parametric_and_raster();
    let back = xmp_to_recipe(&bare_document(&mixed, None));
    assert_eq!(back.masks.len(), 1);
    assert_eq!(back.masks[0].mask, mixed.masks[0].mask);
    assert_eq!(back.masks[0].exposure_ev, -1.0);
    // The payload (v1.3.1) is what brings it back — by its bare name, the
    // raster itself being one this test never wrote, anchored in the develop
    // dir of the photo it is loaded for.
    let photo = std::env::temp_dir().join(format!("bitmap-back-{}", std::process::id())).join("roll-0001.ARW");
    std::fs::create_dir_all(crate::store::develop_dir(&photo)).unwrap();
    let whole = crate::xmp::xmp_to_recipe_for_photo(&recipe_to_xmp(&mixed), &photo);
    assert_eq!(whole.masks.len(), 2);
    let there = crate::store::develop_dir(&photo).join("subject.png").to_string_lossy().into_owned();
    assert_eq!(whole.masks[1].mask, MaskGeometry::Bitmap { path: there });
    let _ = std::fs::remove_dir_all(crate::store::develop_dir(&photo));
    assert_eq!(whole.masks[1].exposure_ev, 0.6);
}

#[test]
fn foreign_as_shot_sidecar_imports_no_wb_and_drops_identity_curves() {
    // A Lightroom-style sidecar (no AutoShade marker): "As Shot" Temperature
    // and Tint are the CAMERA's values, not edits — they must NOT import.
    // LR also always writes the master curve; the 2-point identity means
    // "no curve" and must collapse to empty.
    let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="As Shot"
    crs:Temperature="5150"
    crs:Tint="+10"
    crs:Exposure2012="+0.65"
    crs:Contrast2012="+22"
    crs:Sharpness="40"
    crs:HasSettings="True">
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
    let r = xmp_to_recipe(lr);
    assert_eq!(r.temperature_k, None, "as-shot Kelvin is not an edit");
    assert_eq!(r.tint, 0.0, "as-shot tint is not an edit");
    assert_eq!(r.exposure_ev, 0.65);
    assert_eq!(r.contrast, 22.0);
    // 1:1 since v0.31.1. This is the value the user's own seven reference
    // sidecars carry, and it used to import as 60.
    assert_eq!(r.sharpening, 40.0);
    assert!(r.tone_curve.is_empty(), "identity curve must collapse");
    // A Custom-WB foreign sidecar DOES import its Kelvin + tint.
    let custom = lr.replace("As Shot", "Custom");
    let rc = xmp_to_recipe(&custom);
    assert_eq!(rc.temperature_k, Some(5150.0));
    assert_eq!(rc.tint, 10.0);
}

#[test]
fn xml_values_round_trip_hostile_text_and_foreign_references_exactly_once() {
    let hostile = r#"& < > " ' literal &lt; masks\Bob's "sky".xmp"#;
    let r = EditRecipe {
        rationale: hostile.into(),
        masks: vec![LocalAdjustment { name: hostile.into(), ..Default::default() }],
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    assert!(xmp.contains("&quot;sky&quot;"), "attribute quotes are escaped: {xmp}");
    let back = xmp_to_recipe(&xmp);
    assert_eq!(back.rationale, hostile);
    assert_eq!(back.masks[0].name, hostile);

    let foreign = r#"<rdf:Description crs:CorrectionName = "Bob&apos;s &#x3C;sky&#62; &#38; &quot;sea&quot;"/>"#;
    assert_eq!(
        Tag::new(foreign).crs_str("CorrectionName").as_deref(),
        Some(r#"Bob's <sky> & "sea""#)
    );
}

#[test]
fn comments_and_whitespace_cannot_hijack_the_crs_description_or_merge() {
    let fake = r#"<!-- <rdf:Description xmlns:crs="urn:fake" crs:Exposure2012="9"/> -->"#;
    let doc = format!(
        "{fake}\n<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Exposure2012 = \"+0.65\" crs:HasSettings=\"True\">\
             </rdf:Description>"
    );
    assert_eq!(xmp_to_recipe(&doc).exposure_ev, 0.65);
    let merged = merged_doc(
        &doc,
        &EditRecipe { exposure_ev: 0.25, ..Default::default() },
    )
    .expect("the real description is mergeable");
    assert!(merged.contains(fake), "the foreign comment survives verbatim");
    assert_eq!(xmp_to_recipe(&merged).exposure_ev, 0.25);
}

/// R25 P1 rewrote this test's premise. It used to read
/// `partial_and_unsupported_masks_are_not_rendered…` and pin the old
/// all-or-nothing rule: five corrections in, five losses, zero masks. Two
/// of those five carry nothing worse than a rotation angle and a DEFAULT
/// blend mode, which is what every Lightroom radial and every Lightroom
/// component look like — so the rule refused the user's whole catalog.
/// Now the readable ones import with a named note, the genuinely
/// unreadable ones still do not, and the base's block is still preserved
/// byte-for-byte while the develop has not touched it.
#[test]
fn readable_corrections_import_with_a_note_and_their_group_is_still_preserved() {
    let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:MaskGroupBasedCorrections>
        <rdf:Seq>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="true">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Gradient" crs:ZeroX="0.5" crs:ZeroY="0.8" crs:FullX="0.5" crs:FullY="0.2"/>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="false">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Gradient" crs:ZeroX="0.4" crs:ZeroY="0.8" crs:FullX="0.4" crs:FullY="0.2"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/CircularGradient" crs:Top="0.2" crs:Left="0.2" crs:Bottom="0.8" crs:Right="0.8" crs:Feather="50" crs:Roundness="0" crs:Flipped="false" crs:Angle="12"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Gradient" crs:MaskBlendMode="0" crs:ZeroX="0.3" crs:ZeroY="0.8" crs:FullX="0.3" crs:FullY="0.2"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
        </rdf:Seq>
       </crs:MaskGroupBasedCorrections>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;

    let parsed = xmp_to_recipe(doc);
    assert_eq!(
        parsed.masks.len(),
        2,
        "the rotated radial and the default-blend-mode gradient are readable: {:?}",
        parsed.masks
    );
    assert_eq!(
        unsupported_corrections(doc),
        3,
        "the brush pair and the muted correction are the only refusals"
    );
    let losses = import_losses(doc);
    assert_eq!(losses.len(), 4, "three refusals plus the rotation note: {losses:?}");
    assert_eq!(
        losses.iter().filter(|l| l.reason == MaskImportReason::Rotation(12)).count(),
        1,
        "crs:Angle=\"12\" is named WITH ITS ANGLE, not silently discarded: {losses:?}"
    );
    assert!(
        !losses.iter().any(|l| l.reason == MaskImportReason::BlendMode),
        "the DEFAULT blend mode costs nothing and must raise nothing: {losses:?}"
    );
    // The rotated radial imports UNROTATED — the reading is honest about
    // being approximate, which is the whole point of naming the loss.
    assert!(
        parsed
            .masks
            .iter()
            .any(|m| matches!(m.mask, MaskGeometry::Radial { angle, .. } if angle == 0.0)),
        "crs:Angle is not mapped onto the engine angle in this batch"
    );

    let start = doc.find("<crs:MaskGroupBasedCorrections>").unwrap();
    let end = doc.find("</crs:MaskGroupBasedCorrections>").unwrap()
        + "</crs:MaskGroupBasedCorrections>".len();
    let original = &doc[start..end];
    let merged = merged_doc(
        doc,
        &EditRecipe { exposure_ev: 0.25, ..Default::default() },
    )
    .expect("the surrounding document remains mergeable");
    assert!(merged.contains(original), "the original mask group is retained verbatim");
    assert!(merged.contains("Mask/Brush"));
    assert!(merged.contains(r#"crs:CorrectionActive="false""#));
    assert!(merged.contains(r#"crs:Angle="12""#));
    assert!(merged.contains(r#"crs:MaskBlendMode="0""#));
}

/// L05#4: the preserve rule yields to the recipe's own masks — the save
/// in hand is the newest intent, so the published document carries THIS
/// develop's masks, the foreign block goes, and the loss is a note
/// rather than a silence (before: the output showed an older pass's
/// masks and none of the develop's, reported as plain success).
#[test]
fn a_recipe_with_masks_outranks_the_bases_foreign_mask_block() {
    let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:MaskGroupBasedCorrections>
        <rdf:Seq>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="true">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
        </rdf:Seq>
       </crs:MaskGroupBasedCorrections>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
    let mut r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Radial {
            top: 0.2,
            left: 0.2,
            bottom: 0.8,
            right: 0.8,
            feather: 0.5,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 2,
        },
        name: "face".into(),
        exposure_ev: 0.4,
        ..Default::default()
    });
    let out = merge_recipe_into_xmp(doc, &r).expect("mergeable");
    assert!(
        out.doc.contains("Mask/CircularGradient"),
        "the develop's own mask is published: {}",
        out.doc
    );
    assert!(!out.doc.contains("Mask/Brush"), "the foreign block is not resurrected");
    assert_eq!(out.notes.len(), 1, "the replacement is disclosed: {:?}", out.notes);
    assert!(
        out.notes[0].contains("1 thing(s)") && out.notes[0].contains("1 edited mask(s)"),
        "the note names both counts: {}",
        out.notes[0]
    );
    // The mirror case stays preserved-without-note: nothing of the user's
    // is suppressed when the recipe has no masks.
    let out2 = merge_recipe_into_xmp(
        doc,
        &EditRecipe { exposure_ev: 0.25, ..Default::default() },
    )
    .expect("mergeable");
    assert!(out2.doc.contains("Mask/Brush"), "no recipe masks → the block is preserved");
    assert!(out2.notes.is_empty(), "a pure preserve has no loss to note: {:?}", out2.notes);
}

/// L05#1: the attribute-carrying spelling of an owned element is the SAME
/// property (legal XML; the writer's strip already matched it by name) —
/// the literal reader missed it, imported "no curve", and the merge then
/// deleted the element from the user's own sidecar with nothing written
/// in its place.
#[test]
fn an_attribute_form_curve_is_read_not_deleted() {
    let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:ToneCurvePV2012 xml:lang="x-default"><rdf:Seq>
        <rdf:li>0, 20</rdf:li>
        <rdf:li>255, 240</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012>
      </rdf:Description>"#;
    let r = xmp_to_recipe(doc);
    assert_eq!(
        r.tone_curve,
        vec![CurvePoint { input: 0, output: 20 }, CurvePoint { input: 255, output: 240 }],
        "the attribute-form curve is read"
    );
    // Merging a NEW curve over it must not leave two curves behind: the
    // attribute-form element is stripped (by name) and ours replaces it.
    let merged = merged_doc(
        doc,
        &EditRecipe {
            tone_curve: vec![
                CurvePoint { input: 0, output: 5 },
                CurvePoint { input: 255, output: 250 },
            ],
            ..Default::default()
        },
    )
    .expect("mergeable");
    assert!(!merged.contains("0, 20"), "the old spelling is stripped: {merged}");
    assert_eq!(xmp_to_recipe(&merged).tone_curve[0].output, 5, "the new curve answers");
}

/// L05#1: an attribute-form mask GROUP is a real group — reading it as
/// "absent" reported zero unsupported corrections AND told the merge it
/// was free to replace the block.
#[test]
fn an_attribute_form_mask_group_counts_as_a_loss_and_survives_the_merge() {
    let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:MaskGroupBasedCorrections rdf:parseType="Resource">
        <rdf:Seq>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="true">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
        </rdf:Seq>
       </crs:MaskGroupBasedCorrections>
      </rdf:Description>"#;
    assert_eq!(unsupported_corrections(doc), 1, "the brush correction is a counted loss");
    let merged = merged_doc(
        doc,
        &EditRecipe { exposure_ev: 0.25, ..Default::default() },
    )
    .expect("mergeable");
    assert!(merged.contains("Mask/Brush"), "the group survives the merge: {merged}");
}

/// A whitespace-carrying close tag (`</crs:Key >`) is the same close in
/// XML; the literal close scan ran past it.
#[test]
fn a_close_tag_with_trailing_space_still_ends_a_property_element() {
    let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
       <crs:Exposure2012>+0.65</crs:Exposure2012 >
      </rdf:Description>"#;
    assert_eq!(Scope::new(doc).crs_f32("Exposure2012"), Some(0.65));
}

/// Present-but-unreadable is a DISCLOSED loss, not "no curve": the
/// attribute-form spelling used to make the element invisible to the
/// disclosure as well, so bad points imported as a silent neutral.
#[test]
fn an_unreadable_attribute_form_curve_is_named_by_unparsable_crs_numbers() {
    let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
       <crs:ToneCurvePV2012 xml:lang="x-default"><rdf:Seq>
        <rdf:li>999, -5</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012>
      </rdf:Description>"#;
    let bad = unparsable_crs_numbers(doc);
    assert!(bad.iter().any(|v| v == "ToneCurvePV2012"), "disclosed: {bad:?}");
    // An element that never closes is the same disclosed loss.
    let unterminated = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
       <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li></rdf:Seq>
      </rdf:Description>"#;
    let bad = unparsable_crs_numbers(unterminated);
    assert!(bad.iter().any(|v| v == "ToneCurvePV2012"), "disclosed: {bad:?}");
}

/// L05#7: a document binding the camera-raw namespace to another prefix
/// (or `crs` to another URI) is one every scanner here misreads — the
/// merge REFUSES (the caller regenerates and discloses) instead of
/// splicing a second, contradictory settings block beside the foreign
/// one, and the import discloses instead of coming back silently neutral.
#[test]
fn a_foreign_camera_raw_prefix_refuses_the_merge_and_is_disclosed() {
    let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:cr="http://ns.adobe.com/camera-raw-settings/1.0/"
        cr:Exposure2012="+1.00" cr:HasSettings="True">
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
    assert!(
        merge_recipe_into_xmp(doc, &EditRecipe::default()).is_none(),
        "a foreign camera-raw prefix is refused, never duplicated"
    );
    let bad = unparsable_crs_numbers(doc);
    assert_eq!(bad.len(), 1, "one entry naming the binding: {bad:?}");
    assert!(bad[0].contains("`cr:`"), "the prefix is named: {}", bad[0]);

    let crooked = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://example.invalid/ns" crs:Exposure2012="+1.00">
      </rdf:Description>"#;
    assert!(
        merge_recipe_into_xmp(crooked, &EditRecipe::default()).is_none(),
        "a crs prefix bound to a foreign URI is not camera raw"
    );
    assert!(!unparsable_crs_numbers(crooked).is_empty());
}

/// L05#7 sub-item 4: `xmlns:crs` may legally live on an ANCESTOR
/// (`rdf:RDF`) with every setting in property-element form — the
/// attribute-only test missed that Description, and the merge spliced a
/// SECOND settings Description into the same document.
#[test]
fn a_description_whose_crs_children_declare_the_namespace_upstream_is_still_found() {
    let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
       xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
      <rdf:Description rdf:about="">
       <crs:Exposure2012>+0.80</crs:Exposure2012>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
    assert_eq!(xmp_to_recipe(doc).exposure_ev, 0.8, "element-form settings are found");
    let merged = merged_doc(
        doc,
        &EditRecipe { exposure_ev: 0.25, ..Default::default() },
    )
    .expect("mergeable");
    assert_eq!(
        merged.matches("<rdf:Description").count(),
        1,
        "spliced in place, not duplicated: {merged}"
    );
    assert_eq!(xmp_to_recipe(&merged).exposure_ev, 0.25);
    assert!(!merged.contains("+0.80"), "the old element spelling is stripped");
}

/// The guard the refusal gate must not break: a genuinely settings-free
/// ratings sidecar still takes the INSERT path (that path exists because
/// regenerating over one reported an unfixable loss on every save).
#[test]
fn a_ratings_only_sidecar_still_takes_the_insert_path() {
    let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
        xmp:Rating="4">
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
    let out = merge_recipe_into_xmp(
        doc,
        &EditRecipe { exposure_ev: 0.25, ..Default::default() },
    )
    .expect("insertable");
    assert!(out.doc.contains(r#"xmp:Rating="4""#), "the rating survives verbatim");
    assert_eq!(xmp_to_recipe(&out.doc).exposure_ev, 0.25, "our settings are added");
    assert!(out.notes.is_empty(), "a clean insert has no loss: {:?}", out.notes);
}

#[test]
fn xmp_input_is_bounded_and_numeric_groups_follow_recipe_boundaries() {
    let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:WhiteBalance="Custom"
        crs:Temperature="90000"
        crs:Exposure2012="99"
        crs:Contrast2012="-500"
        crs:Sharpness="200"
        crs:HasCrop="True"
        crs:CropLeft="-1"
        crs:CropTop="0"
        crs:CropRight="1"
        crs:CropBottom="1"
        crs:HasSettings="True">
       <crs:ToneCurvePV2012><rdf:Seq>
        <rdf:li>999, -5</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012>
       <crs:ToneCurvePV2012Red><rdf:Seq>
        <rdf:li>broken</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012Red>
       <crs:MaskGroupBasedCorrections><rdf:Seq>
        <rdf:li><rdf:Description crs:What="Correction" crs:LocalExposure2012="9">
         <crs:CorrectionMasks><rdf:Seq>
          <rdf:li crs:What="Mask/Gradient" crs:ZeroX="0.5" crs:ZeroY="0.8" crs:FullX="0.5" crs:FullY="0.2"/>
         </rdf:Seq></crs:CorrectionMasks>
        </rdf:Description></rdf:li>
       </rdf:Seq></crs:MaskGroupBasedCorrections>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;

    let r = xmp_to_recipe(doc);
    assert_eq!(r.temperature_k, Some(40000.0));
    assert_eq!(r.exposure_ev, 5.0);
    assert_eq!(r.contrast, -100.0);
    assert_eq!(r.sharpening, 150.0);
    assert_eq!(r.crop, None, "invalid compound crop geometry is rejected");
    assert!(
        r.tone_curve.is_empty(),
        "out-of-domain curve coordinates are rejected as a group — the old \
             saturation policy imported '999, -5' as a near-black one-point curve"
    );
    assert!(r.red_curve.is_empty(), "a malformed curve is rejected as a group");
    assert!(r.masks.is_empty(), "an out-of-range local correction is rejected as partial");
    assert_eq!(unsupported_corrections(doc), 1);

    let bad = unparsable_crs_numbers(doc);
    for key in [
        "Temperature",
        "Exposure2012",
        "Contrast2012",
        "Sharpness",
        "CropLeft",
        "ToneCurvePV2012Red",
    ] {
        assert!(bad.iter().any(|v| v == key), "{key} must be disclosed: {bad:?}");
    }

    let oversized = "x".repeat(MAX_XMP_BYTES + 1);
    assert!(crs_own_scope(&oversized).is_empty());
    assert_eq!(xmp_to_recipe(&oversized), EditRecipe::default());
    assert!(merged_doc(&oversized, &EditRecipe::default()).is_none());
    assert_eq!(
        unparsable_crs_numbers(&oversized),
        vec!["XMP document exceeds the 16 MiB limit".to_string()]
    );
}

// --- R27 Batch-5: the `Mask/Image` AI arm (L-08 Arm C) -------------------
//
// Fixtures below reproduce the shape measured across 105 real `Mask/Image`
// instances in the user's library on 2026-08-19: 21 distinct attribute
// names, `MaskActive="true"` on all of them, `MaskVersion="1"` on all of
// them, `MaskSubType` in {0, 1, 2}, `ReferencePoint` on all of them, and
// exactly one optional child element (`crs:Gesture`, on 40).

/// One `Mask/Image` component. `extra` splices additional attributes;
/// `gesture` splices a `crs:Gesture` child (empty = self-closing, which is
/// what 65 of the 105 real instances are).
fn lr_ai_mask(subtype: &str, blend: &str, value: &str, extra: &str, gesture: &str) -> String {
    let head = format!(
        "        <rdf:Description\n\
             \x20        crs:What=\"Mask/Image\"\n\
             \x20        crs:MaskActive=\"true\"\n\
             \x20        crs:MaskName=\"Sky 1\"\n\
             \x20        crs:MaskBlendMode=\"{blend}\"\n\
             \x20        crs:MaskInverted=\"false\"\n\
             \x20        crs:MaskSyncID=\"440777CD3CB8E24BB8E16028893B45DC\"\n\
             \x20        crs:MaskValue=\"{value}\"\n\
             \x20        crs:MaskVersion=\"1\"\n\
             \x20        crs:MaskSubType=\"{subtype}\"\n\
             \x20        crs:ReferencePoint=\"0.605469 0.281525\"{extra}"
    );
    if gesture.is_empty() {
        format!("       <rdf:li>\n{head}/>\n       </rdf:li>\n")
    } else {
        format!(
            "       <rdf:li>\n{head}>\n\
                 \x20       <crs:Gesture>\n\
                 \x20        <rdf:Seq>\n\
                 {gesture}\
                 \x20        </rdf:Seq>\n\
                 \x20       </crs:Gesture>\n\
                 \x20       </rdf:Description>\n\
                 \x20      </rdf:li>\n"
        )
    }
}

/// The provenance block Lightroom writes on a real sky mask — every
/// attribute this engine carries and never interprets.
const LR_AI_PROVENANCE: &str = "\n         crs:InputDigest=\"D0DAC04EB58F013F49D93EF47D22794E\"\
\n         crs:InputDigestVersion=\"2\"\
\n         crs:MaskDigest=\"00D1A1B68591DF41F6CA3F8F805D0F1B\"\
\n         crs:WholeImageArea=\"0/1,0/1,1920/1,2880/1\"\
\n         crs:Origin=\"0,0\"\
\n         crs:ModelVersion=\"234881976\"";
