// One part of the sidecar's tests (src/xmp/tests.rs includes it): the parametric curve, calibration and black and white, point colours, edited masks, multi-component corrections, import loss prose, local curves and the lens switch.

/// v1.5.0: the parametric curve reads in as Lightroom states it, writes
/// back as the whole block Lightroom writes, and stays out of a sidecar
/// whose recipe never moved it.
///
/// MUTATIONS THIS CATCHES: the reader's split fallback to 0 (a document
/// with no parametric keys stops importing as a no-op); the writer's
/// all-or-none condition reduced to the regions (a moved split alone never
/// reaches the sidecar); a key left out of `owned_attr_keys` (the merge
/// writes a second copy beside Lightroom's).
#[test]
fn the_parametric_curve_round_trips_as_lightroom_writes_it() {
    let doc = lr_parametric_doc();
    let r = xmp_to_recipe(&doc);
    assert_eq!(
        [r.param_shadows, r.param_darks, r.param_lights, r.param_highlights],
        [0.0, 40.0, -15.0, 0.0]
    );
    assert_eq!(r.parametric_splits(), [25.0, 50.0, 80.0]);

    let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
    for spelling in [
        "crs:ParametricShadows=\"0\"",
        "crs:ParametricDarks=\"+40\"",
        "crs:ParametricLights=\"-15\"",
        "crs:ParametricHighlights=\"0\"",
        "crs:ParametricShadowSplit=\"25\"",
        "crs:ParametricMidtoneSplit=\"50\"",
        "crs:ParametricHighlightSplit=\"80\"",
    ] {
        assert!(out.doc.contains(spelling), "{spelling} missing: {}", out.doc);
        let key = &spelling[..spelling.find('=').expect("a key") + 1];
        assert_eq!(out.doc.matches(key).count(), 1, "{key} must appear exactly once");
    }
    let round = xmp_to_recipe(&out.doc);
    assert_eq!(
        (round.parametric_regions(), round.parametric_splits()),
        (r.parametric_regions(), r.parametric_splits()),
        "…and it reads back as itself"
    );

    // A moved split with every region at rest is still a statement.
    let split_only = EditRecipe { param_midtone_split: 60.0, ..Default::default() };
    let written = recipe_to_xmp(&split_only);
    assert!(written.contains("crs:ParametricMidtoneSplit=\"60\""), "{written}");
    assert!(written.contains("crs:ParametricDarks=\"0\""), "the block goes out whole: {written}");

    // A recipe that never moved the curve writes none of it, and a
    // document that names none of it imports as nothing.
    let neutral = recipe_to_xmp(&EditRecipe::default());
    assert!(!neutral.contains("Parametric"), "{neutral}");
    assert!(xmp_to_recipe(&neutral).is_noop());
}

/// v1.5.0, the era gate on the case it was generalised for: a v1.4
/// `recipe.json` (era 1) beside a Lightroom sidecar with a parametric
/// curve that recipe never saw. Its save must leave the curve standing —
/// and must still publish the R25 keys that recipe DOES own.
///
/// MUTATION THIS CATCHES: `WHOLE_BLOCKS` without `param_` (the gate
/// releases the block key by key and an edited region writes half of it),
/// or the gate's era range starting at era 1 (see the test above).
#[test]
fn a_v1_4_recipe_keeps_the_parametric_curve_it_never_had() {
    let doc = lr_parametric_doc();
    let mut legacy = as_v1_4_recipe(&xmp_to_recipe(&doc));
    assert_eq!(legacy.schema_era, 1);
    assert_eq!(legacy.param_darks, 0.0, "premise: serde filled the absent field");
    legacy.texture = 0.0; // an R25 key the v1.4 recipe owns, cleared here
    let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
    assert!(out.doc.contains("crs:ParametricDarks=\"+40\""), "the curve was deleted: {}", out.doc);
    assert!(out.doc.contains("crs:ParametricHighlightSplit=\"80\""), "{}", out.doc);
    assert_eq!(out.doc.matches("crs:ParametricDarks=").count(), 1);
    assert!(out.doc.contains("crs:Texture=\"0\""), "an era-1 recipe still owns Texture: {}", out.doc);

    // One region moved on the legacy recipe releases the WHOLE block.
    legacy.param_lights = 30.0;
    let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
    for key in ["ParametricShadows", "ParametricDarks", "ParametricLights", "ParametricHighlightSplit"] {
        assert_eq!(out.doc.matches(&format!("crs:{key}=")).count(), 1, "crs:{key}: {}", out.doc);
    }
    assert!(out.doc.contains("crs:ParametricLights=\"+30\""), "{}", out.doc);
    assert!(out.doc.contains("crs:ParametricDarks=\"0\""), "the recipe's own 0 publishes: {}", out.doc);
}

/// A Lightroom sidecar with the v1.5.0 colour blocks in the shape the
/// operator's library carries them: the B&W mixer where Lightroom writes it
/// (two bands moved), the Calibration seven (three moved) and the B&W switch
/// on — beside a Basic-panel exposure.
fn lr_colour_doc() -> String {
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20 <rdf:Description rdf:about=\"\"\n\
         \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
         \x20   crs:Version=\"15.5.1\"\n\
         \x20   crs:ProcessVersion=\"15.4\"\n\
         \x20   crs:Exposure2012=\"+0.35\"\n\
         \x20   crs:GrayMixerRed=\"0\"\n\
         \x20   crs:GrayMixerOrange=\"0\"\n\
         \x20   crs:GrayMixerYellow=\"0\"\n\
         \x20   crs:GrayMixerGreen=\"0\"\n\
         \x20   crs:GrayMixerAqua=\"-1\"\n\
         \x20   crs:GrayMixerBlue=\"-44\"\n\
         \x20   crs:GrayMixerPurple=\"0\"\n\
         \x20   crs:GrayMixerMagenta=\"0\"\n\
         \x20   crs:ShadowTint=\"+3\"\n\
         \x20   crs:RedHue=\"0\"\n\
         \x20   crs:RedSaturation=\"0\"\n\
         \x20   crs:GreenHue=\"0\"\n\
         \x20   crs:GreenSaturation=\"0\"\n\
         \x20   crs:BlueHue=\"-28\"\n\
         \x20   crs:BlueSaturation=\"+83\"\n\
         \x20   crs:ConvertToGrayscale=\"True\"\n\
         \x20   crs:HasSettings=\"True\"/>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n"
        .to_string()
}

/// v1.5.0: Calibration and the B&W treatment read in as Lightroom states
/// them and write back as the whole blocks Lightroom writes, each key once.
///
/// MUTATIONS THIS CATCHES: a calibration or `GrayMixer*` key left out of
/// `owned_attr_keys` (the merge writes a second copy beside Lightroom's);
/// either block's condition reduced to its moved members; the B&W switch
/// written while off, or read off a creative Look.
#[test]
fn calibration_and_black_and_white_round_trip_as_lightroom_writes_them() {
    let doc = lr_colour_doc();
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.calibration(), Some([3.0, 0.0, 0.0, 0.0, 0.0, -28.0, 83.0]));
    assert!(r.convert_to_grayscale);
    assert_eq!(r.gray_mixer(), [0.0, 0.0, 0.0, 0.0, -1.0, -44.0, 0.0, 0.0]);
    assert!(unparsable_crs_numbers(&doc).is_empty(), "{:?}", unparsable_crs_numbers(&doc));
    assert!(unmodelled_global_crs(&doc).is_empty(), "{:?}", unmodelled_global_crs(&doc));

    let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
    for spelling in [
        "crs:ShadowTint=\"+3\"",
        "crs:RedHue=\"0\"",
        "crs:BlueHue=\"-28\"",
        "crs:BlueSaturation=\"+83\"",
        "crs:GrayMixerRed=\"0\"",
        "crs:GrayMixerAqua=\"-1\"",
        "crs:GrayMixerBlue=\"-44\"",
        "crs:ConvertToGrayscale=\"True\"",
    ] {
        assert!(out.doc.contains(spelling), "{spelling} missing: {}", out.doc);
        let key = &spelling[..spelling.find('=').expect("a key") + 1];
        assert_eq!(out.doc.matches(key).count(), 1, "{key} must appear exactly once");
    }
    let round = xmp_to_recipe(&out.doc);
    assert_eq!(
        (round.calibration(), round.convert_to_grayscale, round.gray_mixer()),
        (r.calibration(), r.convert_to_grayscale, r.gray_mixer()),
        "…and it reads back as itself"
    );

    // One moved slider sends its whole block; B&W with an untouched mix
    // sends the eight at 0, the shape Lightroom writes.
    let one = recipe_to_xmp(&EditRecipe { cal_green_sat: 37.0, ..Default::default() });
    assert!(one.contains("crs:GreenSaturation=\"+37\"") && one.contains("crs:ShadowTint=\"0\""), "{one}");
    assert!(!one.contains("GrayMixer") && !one.contains("ConvertToGrayscale"), "{one}");
    let bw = recipe_to_xmp(&EditRecipe { convert_to_grayscale: true, ..Default::default() });
    assert!(bw.contains("crs:ConvertToGrayscale=\"True\""), "{bw}");
    assert!(bw.contains("crs:GrayMixerRed=\"0\"") && bw.contains("crs:GrayMixerMagenta=\"0\""), "{bw}");
    assert!(!bw.contains("crs:ShadowTint"), "{bw}");

    // A recipe that never moved them writes none of it, and a document
    // that names none of it imports as nothing.
    //
    // Each key SPELT WITH ITS PREFIX, never bare: `crs:DefringeGreenHueHi`
    // — which a neutral recipe does write, at Adobe's own default —
    // contains the bare `GreenHue`, so the bare form asserted that the
    // de-fringe block was absent and failed on a correct writer.
    let neutral = recipe_to_xmp(&EditRecipe::default());
    for key in CALIBRATION_CRS.iter().chain(&["ConvertToGrayscale", "GrayMixer"]) {
        let spelt = format!("crs:{key}");
        assert!(!neutral.contains(&spelt), "{spelt}: {neutral}");
    }
    assert!(xmp_to_recipe(&neutral).is_noop());

    // A switch Lightroom writes OFF is off…
    let off = doc.replace("crs:ConvertToGrayscale=\"True\"", "crs:ConvertToGrayscale=\"False\"");
    assert!(!xmp_to_recipe(&off).convert_to_grayscale);
    // …and a creative Look's own switch and calibration are the Look's.
    let look = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                    xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                    <rdf:Description rdf:about=\"\" \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                    <crs:BlueHue>-7</crs:BlueHue>\
                    <crs:Look><rdf:Description><crs:Parameters><rdf:Description>\
                    <crs:ConvertToGrayscale>True</crs:ConvertToGrayscale>\
                    <crs:BlueHue>+50</crs:BlueHue>\
                    </rdf:Description></crs:Parameters></rdf:Description></crs:Look>\
                    </rdf:Description></rdf:RDF></x:xmpmeta>";
    let r = xmp_to_recipe(look);
    assert!(!r.convert_to_grayscale, "the Look's B&W switch is the Look's");
    assert_eq!(r.cal_blue_hue, -7.0, "the Description's own calibration, never the Look's");
}

/// One Lightroom point-colour item: the sampled swatch MIDI2LR's
/// `LocalPresets.lua` dumps (SrcHue 1.312043 rad, every shift at -1), in
/// the nineteen-number order `PointColor::to_numbers` states.
const LR_SWATCH: &str = "1.312043, 0.473663, 0.739782, -1.000000, -1.000000, -1.000000, 0.500000, \
                             0.000000, 0.330000, 0.670000, 1.000000, 0.000000, 0.290000, 0.650000, 1.000000, \
                             0.150000, 0.700000, 1.000000, 1.000000";

/// The no-swatch placeholder Lightroom writes on 113 of the operator's 175
/// sidecars: one item of nineteen `-1.000000`s.
fn lr_placeholder_item() -> String {
    ["-1.000000"; 19].join(", ")
}

/// A Lightroom sidecar whose `crs:PointColors` holds `items`.
fn lr_point_colors_doc(items: &[&str]) -> String {
    let lis: String = items.iter().map(|i| format!("     <rdf:li>{i}</rdf:li>\n")).collect();
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
             \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             \x20 <rdf:Description rdf:about=\"\"\n\
             \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             \x20   crs:Version=\"15.5.1\"\n\
             \x20   crs:Exposure2012=\"+0.35\"\n\
             \x20   crs:HasSettings=\"True\">\n\
             \x20  <crs:PointColors>\n\
             \x20   <rdf:Seq>\n\
             {lis}\
             \x20   </rdf:Seq>\n\
             \x20  </crs:PointColors>\n\
             \x20 </rdf:Description>\n\
             \x20</rdf:RDF>\n\
             </x:xmpmeta>\n"
    )
}

/// v1.5.0: point colours read in from Lightroom's nineteen numbers, write
/// back where and as Lightroom writes them, and leave Lightroom's own
/// no-swatch placeholder alone.
///
/// MUTATIONS THIS CATCHES: the placeholder read as a swatch, or stripped by
/// an ordinary save (the merge owning the element unconditionally); a
/// legacy recipe's save deleting swatches it never saw (the era read
/// dropped); an item written in another order or precision; the element
/// left off `OWNED_ELEMENT_ONLY` (a second `crs:PointColors` beside
/// Lightroom's, and the import disclosure naming a property this engine
/// renders).
#[test]
fn point_colors_round_trip_and_leave_the_placeholder_alone() {
    // Lightroom's placeholder: nothing to import, nothing to disclose, and
    // an ordinary save keeps it where it stands.
    let placeholder = lr_point_colors_doc(&[lr_placeholder_item().as_str()]);
    let r = xmp_to_recipe(&placeholder);
    assert!(r.point_colors.is_empty(), "{:?}", r.point_colors);
    assert!(unparsable_crs_numbers(&placeholder).is_empty(), "{:?}", unparsable_crs_numbers(&placeholder));
    assert!(unmodelled_global_crs(&placeholder).is_empty(), "{:?}", unmodelled_global_crs(&placeholder));
    let kept = merge_recipe_into_xmp(&placeholder, &r).expect("mergeable");
    assert_eq!(kept.doc.matches("<crs:PointColors>").count(), 1, "{}", kept.doc);
    assert!(kept.doc.contains(&lr_placeholder_item()), "the placeholder stands: {}", kept.doc);

    // A real swatch reads in number for number…
    let doc = lr_point_colors_doc(&[LR_SWATCH]);
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.point_colors.len(), 1);
    let p = &r.point_colors[0];
    assert_eq!((p.src_hue, p.src_sat, p.src_lum), (1.312043, 0.473663, 0.739782));
    assert_eq!((p.hue_shift, p.sat_scale, p.lum_scale, p.range_amount), (-1.0, -1.0, -1.0, 0.5));
    assert_eq!((p.hue_range, p.sat_range, p.lum_range), (
        [0.0, 0.33, 0.67, 1.0],
        [0.0, 0.29, 0.65, 1.0],
        [0.15, 0.7, 1.0, 1.0]
    ));
    // …and merges back as the very item it arrived as, once.
    let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
    assert_eq!(out.doc.matches("<crs:PointColors>").count(), 1, "{}", out.doc);
    assert!(out.doc.contains(&format!("<rdf:li>{LR_SWATCH}</rdf:li>")), "{}", out.doc);
    assert_eq!(xmp_to_recipe(&out.doc).point_colors, r.point_colors);
    // Where Lightroom writes it: after the curves, before the masks.
    let fresh = recipe_to_xmp(&EditRecipe {
        tone_curve: vec![
            crate::recipe::CurvePoint { input: 0, output: 0 },
            crate::recipe::CurvePoint { input: 128, output: 140 },
            crate::recipe::CurvePoint { input: 255, output: 255 },
        ],
        masks: vec![crate::recipe::LocalAdjustment {
            mask: crate::recipe::MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 0.0, full_y: 1.0 },
            enabled: true,
            amount: 1.0,
            exposure_ev: 0.5,
            ..Default::default()
        }],
        point_colors: r.point_colors.clone(),
        ..Default::default()
    });
    let at = |needle: &str| fresh.find(needle).unwrap_or_else(|| panic!("{needle} missing: {fresh}"));
    assert!(at("</crs:ToneCurvePV2012>") < at("<crs:PointColors>"), "{fresh}");
    assert!(at("</crs:PointColors>") < at("<crs:MaskGroupBasedCorrections>"), "{fresh}");

    // The develop DELETED its swatch: a current-era recipe speaks for the
    // element, and the base's goes.
    let cleared = EditRecipe { point_colors: Vec::new(), ..r.clone() };
    let out = merge_recipe_into_xmp(&doc, &cleared).expect("mergeable");
    assert!(!out.doc.contains("<crs:PointColors>"), "{}", out.doc);

    // A recipe from before v1.5.0 never saw the element: its save keeps it.
    let legacy = as_v1_4_recipe(&r);
    assert!(legacy.point_colors.is_empty() && legacy.schema_era == 1, "premise");
    let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
    assert_eq!(out.doc.matches("<crs:PointColors>").count(), 1, "{}", out.doc);
    assert!(out.doc.contains(&format!("<rdf:li>{LR_SWATCH}</rdf:li>")), "{}", out.doc);

    // A recipe with a swatch replaces the placeholder: one element, ours.
    let out = merge_recipe_into_xmp(&placeholder, &r).expect("mergeable");
    assert_eq!(out.doc.matches("<crs:PointColors>").count(), 1, "{}", out.doc);
    assert!(!out.doc.contains(&lr_placeholder_item()), "{}", out.doc);
    assert!(out.notes.is_empty(), "{:?}", out.notes);
}

/// A `crs:PointColors` this reader refuses is NAMED, kept by a save that
/// has nothing to put in its place, and replaced out loud by one that has.
///
/// MUTATIONS THIS CATCHES: `from_numbers` repairing a record it does not
/// understand; the reader keeping the first sixteen of a longer list; the
/// merge owning an unreadable base block for a recipe with no swatch (a
/// list in some future Lightroom layout deleted on Ctrl+S); the
/// replacement note dropped.
#[test]
fn an_unreadable_point_color_block_is_named_kept_and_replaced_out_loud() {
    let eighteen = LR_SWATCH.rsplit_once(", ").expect("nineteen numbers").0;
    let twenty = format!("{LR_SWATCH}, 0.000000");
    let src_sat_above_one = LR_SWATCH.replacen("0.473663", "1.473663", 1);
    let backwards_window = LR_SWATCH.replacen("0.290000, 0.650000", "0.650000, 0.290000", 1);
    let seventeen = [LR_SWATCH; crate::recipe::MAX_POINT_COLORS + 1];
    for doc in [
        lr_point_colors_doc(&[eighteen]),
        lr_point_colors_doc(&[twenty.as_str()]),
        lr_point_colors_doc(&[src_sat_above_one.as_str()]),
        lr_point_colors_doc(&[backwards_window.as_str()]),
        lr_point_colors_doc(&[LR_SWATCH, "not, a, number"]),
        lr_point_colors_doc(&seventeen),
    ] {
        assert_eq!(unparsable_crs_numbers(&doc), vec!["PointColors"], "{doc}");
        let r = xmp_to_recipe(&doc);
        assert!(r.point_colors.is_empty(), "refused, never repaired: {doc}");
        // Nothing to put in its place: the base's own bytes stand.
        let body = owned_element_body(&doc, "crs:PointColors").expect("closed").expect("present");
        let kept = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
        assert!(kept.doc.contains(body), "{}", kept.doc);
        assert!(kept.notes.is_empty(), "{:?}", kept.notes);
        // A develop with a swatch of its own replaces it — and says so.
        let mine = EditRecipe { point_colors: vec![crate::recipe::PointColor::sampled(1.0, 0.5, 0.5)], ..r };
        let replaced = merge_recipe_into_xmp(&doc, &mine).expect("mergeable");
        assert_eq!(replaced.doc.matches("<crs:PointColors>").count(), 1, "{}", replaced.doc);
        assert!(!replaced.doc.contains(body), "{}", replaced.doc);
        assert_eq!(replaced.notes.len(), 1, "{:?}", replaced.notes);
        assert!(replaced.notes[0].contains("point colours could not be read"), "{}", replaced.notes[0]);
    }
}

/// R25 P8, the READ / WRITE asymmetry: a document whose top-level child
/// opens and closes under DIFFERENT names balances its tag counts but
/// crosses its names. `top_level_owned_spans` (the writer's strip) does
/// not even notice — it tracks OWNED children only — so the merge went
/// ahead, while `crs_scope_inner` bailed, and a bailed scope hands every
/// scanner the WHOLE document: the creative Look's baked `crs:Clarity2012`
/// was then read as the photographer's own slider, which is the one thing
/// the scope function exists to prevent.
#[test]
fn a_crossed_name_look_is_dropped_from_the_scope_not_promoted_by_it() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core\">\n\
             \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             \x20 <rdf:Description rdf:about=\"\"\n\
             \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             \x20   crs:Exposure2012=\"+0.35\"\n\
             \x20   crs:HasSettings=\"True\">\n\
             \x20  <crs:Look>\n\
             \x20   <rdf:Description>\n\
             \x20    <crs:Clarity2012>+50</crs:Clarity2012>\n\
             \x20   </rdf:Description>\n\
             \x20  </crs:Foo>\n\
             \x20 </rdf:Description>\n\
             \x20</rdf:RDF>\n\
             </x:xmpmeta>\n";
    let r = xmp_to_recipe(doc);
    assert_eq!(r.exposure_ev, 0.35, "the top level's own settings still import");
    assert_eq!(
        r.clarity, 0.0,
        "the Look's baked clarity is NOT this photographer's slider: {doc}"
    );
    // The writer really does go ahead on this document — which is what
    // made the reader's bail an asymmetry rather than a shared refusal.
    assert!(
        merge_recipe_into_xmp(doc, &EditRecipe::default()).is_some(),
        "premise: the merge does not refuse this shape"
    );
}

/// The other side of the same rule: the moment the user edits a mask, the
/// develop in hand IS the newest intent, so it publishes — and the base's
/// block going is a disclosed note, never a silence.
#[test]
fn an_edited_mask_overwrites_and_says_so() {
    let doc = lr_doc(&lr_correction("Radial 1", "", &lr_radial("37.412506", "0")));
    let mut r = xmp_to_recipe(&doc);
    r.masks[0].exposure_ev = 1.25;
    let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
    assert!(
        !out.doc.contains("crs:Angle=\"37.412506\""),
        "the develop's own projection replaced the block: {}",
        out.doc
    );
    assert_eq!(out.notes.len(), 1, "the replacement is disclosed: {:?}", out.notes);
    assert!(
        out.notes[0].contains("1 thing(s)") && out.notes[0].contains("1 edited mask(s)"),
        "the note names both counts: {}",
        out.notes[0]
    );
    assert_eq!(
        xmp_to_recipe(&out.doc).masks[0].exposure_ev,
        1.25,
        "and the edit is what the file now says"
    );
}

/// R35 preserves every ordered component. The base selector still owes
/// the R25 P9 rule: composition must start at the BASE — the
/// first component in the file, which is what Lightroom's Add/Subtract
/// stack composes onto. It used to be decided by KIND instead: the reader
/// tried `Mask/Gradient` before `Mask/CircularGradient`, so a trailing
/// linear beat a leading radial and the imported mask was a shape the
/// correction merely happened to also contain.
///
/// Measured on the user's library, not invented: `P29` 蒙版 5 is
/// `[CircularGradient, CircularGradient, Gradient]` and imported as that
/// trailing LINEAR with both radials gone; `P12` Mask 9 is
/// `[CircularGradient, Gradient]` and did the same. The loss was DISCLOSED
/// throughout (`MultiComponent`, and the dropped radials' own
/// `Rotation(…)` notes) — so this was never the silent drop it looked
/// like from the recipe alone — but the surviving shape was the wrong one.
///
/// MUTATION THIS CATCHES: restoring the kind-ordered `if let` chain flips
/// row 1 back to a linear; ignoring `crs:MaskBlendMode` in the selector
/// flips rows 3 and 4, the two that are inversions of intent rather than
/// truncations of it.
#[test]
fn a_multi_component_correction_imports_its_base_and_every_ordered_shape() {
    // A subtract component, spelled the way Lightroom spells it (the pair
    // v0.31.1 taught this reader to read: mode "1" WITH MaskValue "0").
    let subtract = |c: String| {
        c.replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"")
    };
    let radial = || lr_radial("0", "0");
    for (label, comps, want_radial) in [
        // Kind order used to decide: a TRAILING linear beat a leading
        // radial. `P29` 蒙版 5's real structure, all three at the
        // default blend mode — a plain union.
        ("radial, radial, linear (union)", vec![radial(), radial(), lr_gradient("0")], true),
        ("linear, radial (union)", vec![lr_gradient("0"), radial()], false),
        // Blend mode decides over document order: `P29` 蒙版 3 and
        // `P12` Mask 9 are both a base radial + a SUBTRACT linear, and
        // the importer kept the shape Lightroom carves away WITH.
        ("radial base, linear subtract", vec![radial(), subtract(lr_gradient("1"))], true),
        ("linear subtract, radial base", vec![subtract(lr_gradient("1")), radial()], true),
        // Nothing but subtractions has no base to find, so the first
        // component stands in — some shape beats no shape.
        (
            "all subtract",
            vec![subtract(lr_gradient("1")), subtract(lr_radial("0", "1"))],
            false,
        ),
    ] {
        let doc = lr_doc(&lr_correction("Stacked 1", "", &comps.concat()));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "{label}: one correction imports one composed mask");
        let got_radial = matches!(r.masks[0].mask, MaskGeometry::Radial { .. });
        assert_eq!(
            got_radial, want_radial,
            "{label}: wrong base — got {:?}",
            r.masks[0].mask
        );
        assert_eq!(r.masks[0].components.len(), comps.len() - 1, "{label}");
        let losses = import_losses(&doc);
        assert!(!losses.iter().any(|l| l.reason == MaskImportReason::MultiComponent), "{label}: {losses:?}");
    }
}

/// Every radial now ARRIVES, so every withheld radial rotation must be
/// disclosed. Native component blend modes do not lose their composition.
/// Restoring the old base-only filter would hide the imported component's
/// withheld angle; restoring the old blend warning would invent a loss.
#[test]
fn every_imported_radials_withheld_rotation_is_disclosed_and_composition_is_preserved() {
    // Base = an UNROTATED linear; the imported component carries the angle.
    let comps = format!("{}{}", lr_gradient("0"), lr_radial("37.412506", "0"));
    let doc = lr_doc(&lr_correction("Stacked 1", "", &comps));
    let reasons: Vec<_> = import_losses(&doc).into_iter().map(|l| l.reason).collect();
    assert!(
        matches!(r_kind(&doc), MaskKindForTest::Linear),
        "the unrotated linear is the base here"
    );
    assert!(
        reasons.contains(&MaskImportReason::Rotation(37)),
        "the imported component's withheld rotation must be named: {reasons:?}"
    );
    assert!(
        !reasons.contains(&MaskImportReason::MultiComponent),
        "no shape is dropped: {reasons:?}"
    );
    // The mirror: when the ROTATED radial is the base, the note is true and
    // must still fire.
    let comps = format!("{}{}", lr_radial("37.412506", "0"), lr_gradient("0"));
    let doc = lr_doc(&lr_correction("Stacked 1", "", &comps));
    let reasons: Vec<_> = import_losses(&doc).into_iter().map(|l| l.reason).collect();
    assert!(
        reasons.contains(&MaskImportReason::Rotation(37)),
        "the imported radial's own rotation is a real loss: {reasons:?}"
    );
    // And a dropped SUBTRACT component keeps its own note (v0.31.1).
    let comps = format!(
        "{}{}",
        lr_radial("0", "0"),
        lr_gradient("1").replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"")
    );
    let doc = lr_doc(&lr_correction("Stacked 1", "", &comps));
    let reasons: Vec<_> = import_losses(&doc).into_iter().map(|l| l.reason).collect();
    assert!(
        !reasons.contains(&MaskImportReason::BlendMode),
        "the subtract component is now composed: {reasons:?}"
    );
}

enum MaskKindForTest {
    Linear,
    Other,
}

fn r_kind(doc: &str) -> MaskKindForTest {
    match xmp_to_recipe(doc).masks.first().map(|m| &m.mask) {
        Some(MaskGeometry::Linear { .. }) => MaskKindForTest::Linear,
        _ => MaskKindForTest::Other,
    }
}

/// R25 P1, the import twin of `mask_loss_reason_all_covers_every_variant`:
/// both disclosure surfaces ITERATE `MaskImportReason::ALL` (here and the
/// GUI's `xmp_import_line`), so the list is the one place a reason can be
/// forgotten — and the match below is where a new variant stops the build.
#[test]
fn import_loss_reasons_all_reach_the_prose() {
    // Adding a variant makes THIS match non-exhaustive; the arm you write
    // carries the next rank, and the asserts fail until `ALL` lists the
    // newcomer in that position.
    fn rank(r: MaskImportReason) -> usize {
        match r {
            MaskImportReason::Unrepresentable => 0,
            MaskImportReason::OutOfModel => 1,
            MaskImportReason::Rotation(_) => 2,
            MaskImportReason::BlendMode => 3,
            MaskImportReason::MultiComponent => 4,
            MaskImportReason::BrushRendered => 5,
            MaskImportReason::AiMaskRecomputed => 6,
            MaskImportReason::AiMaskUnresolved => 7,
            MaskImportReason::ForeignRangeMask => 8,
            MaskImportReason::LocalCurve => 9,
            MaskImportReason::CurveRefineSaturation => 10,
            MaskImportReason::InertLocal(_) => 11,
            MaskImportReason::UnknownLocalKey => 12,
        }
    }
    for (i, r) in MaskImportReason::ALL.into_iter().enumerate() {
        assert_eq!(rank(r), i, "ALL must list every reason once, in rank order");
        assert!(!r.en().trim().is_empty(), "{r:?} has no label for the prose channel");
        assert!(r.same_kind(r), "same_kind must be reflexive for {r:?}");
    }
    // The payload variantS group by KIND, not by value — the property the
    // prose channels rely on to print one line for two sliders, and (since
    // R25 P5) one line for two differently-tilted radials.
    assert!(
        MaskImportReason::InertLocal("LocalGrain")
            .same_kind(MaskImportReason::InertLocal("LocalMoire")),
        "two unmodelled sliders are one line"
    );
    assert!(
        MaskImportReason::Rotation(37).same_kind(MaskImportReason::Rotation(-44)),
        "two tilted radials are one line"
    );
    assert!(
        !MaskImportReason::Rotation(0).same_kind(MaskImportReason::BlendMode),
        "different variants are different lines"
    );
    // Exactly the two drop verdicts, and they are the ones
    // `unsupported_corrections` counts.
    assert_eq!(
        MaskImportReason::ALL.into_iter().filter(|r| r.is_drop()).count(),
        2,
        "a third drop reason needs `unsupported_corrections`' doc revisited"
    );
    let losses: Vec<MaskImportLoss> = MaskImportReason::ALL
        .into_iter()
        .map(|reason| MaskImportLoss { name: format!("m{}", rank(reason)), reason })
        .collect();
    let line = describe_import_losses(3, &losses).expect("ten losses ⇒ a line");
    assert!(line.contains("imported 3 Lightroom mask(s)"), "{line}");
    for r in MaskImportReason::ALL {
        assert!(line.contains(r.en()), "{r:?} never reaches the prose: {line}");
        assert!(
            line.contains(&format!("m{}", rank(r))),
            "{r:?} loses its correction name: {line}"
        );
    }
}

// ── R25 P6: the four LOCAL point curves ──────────────────────────────

/// The round trip, in both directions, over all four keys and their
/// SPARSENESS. The fixture reproduces `P51.xmp`'s own shape: Red and
/// Green present, Main and Blue absent.
#[test]
fn local_curves_round_trip() {
    let curves = format!(
        "{}{}",
        lr_curve("RedCurve", &[(0, 0), (239, 255)]),
        lr_curve("GreenCurve", &[(0, 12), (128, 140), (255, 255)]),
    );
    let doc = lr_doc(&lr_correction_with_curves(
        "Radial 1",
        "",
        &curves,
        &lr_radial("0", "0"),
    ));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the correction must import: {:?}", r.masks);
    let m = &r.masks[0];
    assert_eq!(
        m.red_curve,
        vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 239, output: 255 },
        ],
        "crs:RedCurve did not reach the recipe"
    );
    assert_eq!(m.green_curve.len(), 3, "crs:GreenCurve: {:?}", m.green_curve);
    assert!(m.main_curve.is_empty() && m.blue_curve.is_empty(), "absent means absent");

    // …and back out through OUR writer, then in again: the curves survive
    // byte-for-byte as VALUES (the writer's own spelling is pinned by
    // `local_curve_serialization_has_no_space_after_the_comma`).
    let back = xmp_to_recipe(&recipe_to_xmp(&r));
    assert_eq!(back.masks.len(), 1, "the mask survives our own projection");
    assert_eq!(back.masks[0].red_curve, m.red_curve, "red curve lost in the round trip");
    assert_eq!(back.masks[0].green_curve, m.green_curve, "green curve lost");
    assert!(
        back.masks[0].main_curve.is_empty() && back.masks[0].blue_curve.is_empty(),
        "the writer invented a curve the recipe does not hold"
    );
}

/// THE FORMAT MUTATION GUARD. Lightroom spells a LOCAL curve point
/// `x,y` and a GLOBAL one `x, y`. Nothing in the code enforces that but
/// two separate formatters and this test — and "let's share one helper" /
/// "let's make the spacing consistent" is exactly the tidy-up a later
/// reader would make.
///
/// MUTATION THIS CATCHES: adding a space in `local_curve_elem`, or
/// removing one from `owned_children`'s `curve_elem`. Both halves are
/// asserted in ONE document, so neither can be satisfied by accident.
#[test]
fn local_curve_serialization_has_no_space_after_the_comma() {
    let r = EditRecipe {
        // The global master curve, whose writer uses the SPACED form.
        tone_curve: vec![
            CurvePoint { input: 10, output: 20 },
            CurvePoint { input: 255, output: 255 },
        ],
        masks: vec![LocalAdjustment {
            name: "curved".into(),
            amount: 1.0,
            main_curve: vec![
                CurvePoint { input: 32, output: 48 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    assert!(
        xmp.contains("<rdf:li>32,48</rdf:li>"),
        "a LOCAL curve point is spelled `x,y` with no space: {xmp}"
    );
    assert!(
        !xmp.contains("<rdf:li>32, 48</rdf:li>"),
        "the local writer grew the global writer's space: {xmp}"
    );
    assert!(
        xmp.contains("<rdf:li>10, 20</rdf:li>"),
        "the GLOBAL curve keeps its spaced form: {xmp}"
    );
    assert!(
        !xmp.contains("<rdf:li>10,20</rdf:li>"),
        "the global writer lost its space to the local one: {xmp}"
    );
    // The key is the BARE name and the element sits between the attribute
    // block and the mask list — the two other things the reference
    // sidecars pin and a shared helper would get wrong.
    assert!(xmp.contains("<crs:MainCurve>"), "the local key carries no PV2012 suffix: {xmp}");
    let after_refine = xmp
        .split_once("crs:LocalCurveRefineSaturation=\"100\">")
        .expect("the correction's attribute block closes there")
        .1;
    let curve_at = after_refine.find("<crs:MainCurve>").expect("the curve is emitted");
    let masks_at = after_refine.find("<crs:CorrectionMasks>").expect("the mask list is too");
    assert!(curve_at < masks_at, "the curve must precede <crs:CorrectionMasks>: {xmp}");
}

/// Sparse in, sparse OUT. Lightroom writes only the curves that exist, and
/// a writer that emitted all four (as identities, say) would hand the
/// photographer's sidecar three curves they never drew.
#[test]
fn sparse_curves_stay_sparse() {
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            name: "red only".into(),
            amount: 1.0,
            red_curve: vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 128, output: 160 },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    assert!(xmp.contains("<crs:RedCurve>"), "the curve that exists is written: {xmp}");
    for absent in ["<crs:MainCurve>", "<crs:GreenCurve>", "<crs:BlueCurve>"] {
        assert!(!xmp.contains(absent), "{absent} was invented out of an empty curve: {xmp}");
    }
    // A mask with NO curve at all writes none of the four — the common
    // case, and the one that keeps every pre-P6 sidecar byte-identical.
    let plain = recipe_to_xmp(&EditRecipe {
        masks: vec![LocalAdjustment { name: "plain".into(), amount: 1.0, ..Default::default() }],
        ..Default::default()
    });
    for absent in ["<crs:MainCurve>", "<crs:RedCurve>", "<crs:GreenCurve>", "<crs:BlueCurve>"] {
        assert!(!plain.contains(absent), "{absent} on a curveless mask: {plain}");
    }
}

/// R25 P1 raised `LocalCurve` on every correction that carried one of the
/// four curve elements, because the engine modelled none of them. P6
/// models all four — so a correction whose curves READ must now report
/// NOTHING, or the disclosure cries wolf on 19 of the user's photos.
#[test]
fn a_correction_with_curves_no_longer_reports_a_local_curve_loss() {
    let curves = lr_curve("MainCurve", &[(0, 0), (128, 96), (255, 255)]);
    let doc =
        lr_doc(&lr_correction_with_curves("Radial 1", "", &curves, &lr_radial("0", "0")));
    assert!(
        import_losses(&doc).is_empty(),
        "a correction whose curve imported must report no loss: {:?}",
        import_losses(&doc)
    );
    assert_eq!(unsupported_corrections(&doc), 0);
    assert_eq!(
        xmp_to_recipe(&doc).masks[0].main_curve.len(),
        3,
        "premise: the curve really did import (else the silence is a lie)"
    );
}

/// The other half of the narrowing: a curve that is PRESENT and cannot be
/// read is still a loss, and it is still NAMED. `parse_one_correction`
/// reads the four keys through the unchecked `parse_curve`, whose `Err`
/// half becomes an empty curve — this is what stops that from being
/// silence, which is the module's standing rule (`owned_element_body`).
///
/// Costing the CURVE and not the correction is deliberate: the geometry is
/// still exactly what the file draws, the same verdict a foreign range
/// mask gets.
#[test]
fn an_unreadable_local_curve_is_named_not_swallowed() {
    // "999,-5" is out of the 0..255 domain — the same input
    // `parse_curve_checked` refuses on the global curves (L05).
    let curves = lr_curve("BlueCurve", &[(0, 0), (255, 255)])
        .replace("<rdf:li>255,255</rdf:li>", "<rdf:li>999,-5</rdf:li>");
    let doc =
        lr_doc(&lr_correction_with_curves("Radial 1", "", &curves, &lr_radial("0", "0")));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the mask still imports — the shape is readable");
    assert!(r.masks[0].blue_curve.is_empty(), "an unreadable curve imports as none");
    assert_eq!(
        import_losses(&doc),
        vec![MaskImportLoss {
            name: "Radial 1".into(),
            reason: MaskImportReason::LocalCurve
        }],
        "the loss must be named, not swallowed"
    );
}

// ── R27 Batch-4 (L-08): the brush arm ────────────────────────────────────

/// One `Mask/Paint` stroke. Attribute VALUES are `P12` Mask 7 →
/// Aggregate "Brush 1" verbatim (the F2 anatomy's reference specimen,
/// `P12.xmp` in the user's library, 75,935 B);
/// the indentation is not, because whitespace between attributes is
/// insignificant and a fixture whose mutations depend on counting spaces
/// is a fixture that tests the spaces.
fn lr_paint(sync: &str, value: &str, blend: &str, inverted: &str, dabs: &[&str]) -> String {
    let items: String =
        dabs.iter().map(|t| format!("<rdf:li>{t}</rdf:li>\n")).collect();
    format!(
        "<rdf:li>\n\
             <rdf:Description crs:What=\"Mask/Paint\" crs:MaskActive=\"true\"\n\
             crs:MaskBlendMode=\"{blend}\" crs:MaskInverted=\"{inverted}\"\n\
             crs:MaskSyncID=\"{sync}\" crs:MaskValue=\"{value}\"\n\
             crs:Radius=\"0.582157\" crs:Flow=\"1\" crs:CenterWeight=\"0\">\n\
             <crs:Dabs>\n\
             <rdf:Seq>\n\
             {items}\
             </rdf:Seq>\n\
             </crs:Dabs>\n\
             </rdf:Description>\n\
             </rdf:li>\n"
    )
}

/// Stroke 1 of `P12` Mask 7 → Brush 1: `MaskValue="0.439815"`,
/// `Radius="0.582157"`, and the eight dab tokens §1.1 of the anatomy
/// prints as its worked example.
fn lr_paint_specimen() -> String {
    lr_paint(
        "FA7459A9F5626F4881D7B730C3093F95",
        "0.439815",
        "0",
        "false",
        &[
            "r 0.581835",
            "d 0.000684 0.940004",
            "r 0.581172",
            "d 0.113862 0.987261",
            "r 0.580873",
            "d 0.229292 1.011389",
            "r 0.581205",
            "d 0.112441 1.007149",
        ],
    )
}

/// The `Mask/Aggregate` group itself — two strokes, the second exercising
/// the `f` and `h` state tokens. `(MaskBlendMode, MaskValue) = (0, 1)` is
/// Lightroom's plain ADD, 16 of the 39 real Aggregates; `extra_child` is
/// spliced as a THIRD member of `crs:Masks` for the nesting tests.
fn lr_brush_group(group_inverted: &str, extra_child: &str) -> String {
    let s1 = lr_paint_specimen();
    let s2 = lr_paint(
        "1111111111111111111111111111111A",
        "1",
        "0",
        "false",
        &["f 1", "h 1", "d 0.500000 0.500000"],
    );
    format!(
        "<rdf:li>\n\
             <rdf:Description crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\"\n\
             crs:MaskName=\"Brush 1\" crs:MaskBlendMode=\"0\"\n\
             crs:MaskInverted=\"{group_inverted}\"\n\
             crs:MaskSyncID=\"0000000000000000000000000000000D\" crs:MaskValue=\"1\">\n\
             <crs:Masks>\n\
             <rdf:Seq>\n\
             {extra_child}{s1}{s2}\
             </rdf:Seq>\n\
             </crs:Masks>\n\
             </rdf:Description>\n\
             </rdf:li>\n"
    )
}

/// The `P12` Mask 7 shape — a linear gradient plus the brush group —
/// imports WHOLE, where before R27 Batch-4 the whole correction was thrown
/// away and the gradient with it. That is the L-08 registration's own
/// complaint: 14 already-drawable parametric shapes across the reference
/// library were being discarded because a NEIGHBOURING component was a
/// brush.
///
/// MUTATION-LINED. Verified red three independent ways (transcripts in the
/// batch report): reverting the `"Mask/Aggregate"` arm of
/// `classify_correction` to `unknown_component = true`; deleting
/// `parse_one_correction`'s brush-component collection; never pushing
/// `MaskImportReason::BrushRendered`, which imports the correction SILENTLY
/// — the failure mode this project treats as worse than the refusal it
/// replaced.
/// R29 Batch-3: `crs:LensProfileEnable` is READ, in both spellings, and a
/// document that says nothing gets no opinion put in its mouth.
///
/// This is the fact that separates "no warp because Lightroom drew no
/// correction" (the frames coincide; identity is CORRECT) from "no warp
/// because nobody could solve one" (the frames differ by an unknown
/// amount). `MaskWarpSource` keeps them apart and this reader is what
/// supplies the first one.
#[test]
fn the_sidecar_lens_profile_switch_is_read_in_both_spellings() {
    let with = |v: &str| {
        lr_doc("").replace("crs:Version=", &format!("crs:LensProfileEnable=\"{v}\"\n    crs:Version="))
    };
    // PREMISE: the substitution really landed, or every case below is
    // reading a document with no such key and agreeing by accident.
    assert!(with("0").contains("crs:LensProfileEnable=\"0\""), "{}", with("0"));
    assert_eq!(lens_profile_enabled(&with("0")), Some(false));
    assert_eq!(lens_profile_enabled(&with("1")), Some(true));
    assert_eq!(lens_profile_enabled(&with("False")), Some(false));
    assert_eq!(lens_profile_enabled(&with("true")), Some(true));
    // Says nothing / says something unreadable ⇒ no opinion. Guessing here
    // would decide a coordinate frame from a value nobody wrote.
    assert_eq!(lens_profile_enabled(&lr_doc("")), None);
    assert_eq!(lens_profile_enabled(&with("maybe")), None);
    // `crs:LensProfileName` must not answer for `crs:LensProfileEnable` —
    // MUTATION THIS KILLS: dropping the `crs:` key anchoring in `crs_str`.
    let named = lr_doc("").replace(
        "crs:Version=",
        "crs:LensProfileName=\"Adobe (Sony FE 24-105mm F4 G OSS)\"\n    crs:Version=",
    );
    assert_eq!(lens_profile_enabled(&named), None);
}

/// R29 Batch-3 ACCEPTANCE ④ — the mask warp does NOT touch this boundary.
///
/// `LensProfile::mask_warp` is a RENDER-TIME map. The recipe keeps the
/// coordinates the sidecar stored, verbatim, so `lr_to_engine` and
/// `engine_to_lr` stay exact inverses of one another and a republished
/// sidecar is byte-faithful to what Lightroom wrote — brush dab streams
/// included, which is the payload with the least tolerance for a rewrite.
/// The frame here is LANDSCAPE, which is what keeps that claim whole after
/// R29 C1: a turn does rewrite the dab stream now, and the only thing that
/// must never reach this boundary is the WARP.
///
/// The document here carries a radial, a gradient and a two-stroke brush
/// group, and is exported twice: once from a recipe with no warp and once
/// from the same recipe carrying the full 105 mm warp — the most violent
/// one measured (`m` from 1.0425 at the centre to 0.9976 at the corner,
/// ~88 px at r = 3250). The two documents must be EQUAL, byte for byte.
///
/// MUTATION THIS KILLS: applying `render::lr_mask_warp_norm` inside
/// `masks_xml` / `radial_mask_xml` / `brush_mask_xml`, or anywhere else on
/// the way out. Any of them makes these two strings differ.
#[test]
fn the_mask_warp_never_reaches_the_xmp_boundary() {
    let frame = FrameAspect::from_size(9504.0, 6336.0);
    let doc = in_frame(
        &lr_doc(&format!(
            "{}{}",
            lr_correction("Mask 7", "", &format!("{}{}", lr_gradient("0"), lr_brush_group("false", ""))),
            lr_correction(
                "R",
                "",
                &lr_radial_at("-0.082402", "-0.008723", "1.109604", "1.090228", "28.229232")
            ),
        )),
        9504,
        6336,
    );
    let plain = xmp_to_recipe(&doc);
    // PREMISE: the document really did bring in the geometry whose frame
    // this test is about, or it would prove nothing.
    assert_eq!(plain.masks.len(), 2, "both corrections must import: {:?}", plain.masks);
    assert!(
        plain.masks.iter().any(|m| m
            .components
            .iter()
            .any(|c| matches!(c.geometry, MaskGeometry::Brush { .. }))),
        "the brush group must be in the recipe"
    );
    let mut warped = plain.clone();
    let model = crate::lcp::PerspectiveModel {
        focal_mm: Some(105.0),
        focus_distance: Some(10000.0),
        scale: 0.959207,
        k: [0.961677, 1.182717, -8.218554],
        focal_x: None,
        sensor_format_factor: 1.0,
        vignette: None,
    };
    let legacy_warp = model
        .mask_warp_knots((9504.0, 6336.0), 16)
        .expect("the legacy 105mm table solves");
    let dense_warp = model
        .mask_warp_knots((9504.0, 6336.0), crate::recipe::MASK_WARP_KNOTS)
        .expect("the dense 105mm table solves");
    let half_diag = 0.5f32 * 9504.0f32.hypot(6336.0);
    let radius = 3250.0f32;
    let rho = radius / half_diag;
    let tabulation_delta =
        (crate::render::mask_warp_factor(&dense_warp, rho)
            - crate::render::mask_warp_factor(&legacy_warp, rho))
        .abs()
            * radius;
    eprintln!("105mm mask-warp n=16→64 delta at r=3250: {tabulation_delta:.4}px");
    assert!(tabulation_delta < 0.35, "105mm survival bound: {tabulation_delta}px");
    warped.lens_profile = crate::recipe::LensProfile {
        mask_warp: dense_warp,
        mask_warp_src: crate::recipe::MaskWarpSource::Lcp,
        ..Default::default()
    };
    // PREMISE: the warp really is a warp — an identity table would make
    // the equality below vacuous.
    let w = &warped.lens_profile.mask_warp;
    assert_eq!(w.len(), crate::recipe::MASK_WARP_KNOTS);
    assert!(w[0] > 1.04 && w[w.len() - 1] < 1.0, "the 105mm warp is not the identity: {w:?}");

    // The PROJECTION (payload-free, v1.3.1): the payload carries the whole
    // recipe, lens profile included, so two whole documents differ by
    // design; what must not move is what Lightroom reads.
    let a = bare_document(&plain, frame);
    let b = bare_document(&warped, frame);
    assert_eq!(a, b, "an active mask warp changed the written sidecar");
    // And the ROUND TRIP still lands on the same recipe geometry, so the
    // equality above is not two identically-broken documents.
    //
    // Compared field by field rather than with one `assert_eq!` on the
    // masks, because `crs:MaskSyncID` legitimately differs: the writer
    // MINTS a fresh identity for every component it emits (see
    // `BrushStroke::sync_id`), so a whole-struct comparison would fail on
    // the one field that is supposed to change and say nothing about the
    // frame.
    let back = xmp_to_recipe(&b);
    assert_eq!(back.masks.len(), plain.masks.len());
    for (got, want) in back.masks.iter().zip(&plain.masks) {
        assert_eq!(got.mask, want.mask, "base geometry moved");
        assert_eq!(got.components.len(), want.components.len());
        for (g, w) in got.components.iter().zip(&want.components) {
            match (&g.geometry, &w.geometry) {
                (
                    MaskGeometry::Brush { strokes: gs, name: gn, .. },
                    MaskGeometry::Brush { strokes: ws, name: wn, .. },
                ) => {
                    assert_eq!(gn, wn);
                    assert_eq!(gs.len(), ws.len());
                    for (a, b) in gs.iter().zip(ws) {
                        // The DAB STREAM, token for token — the payload a
                        // coordinate warp would have rewritten.
                        assert_eq!(a.dabs, b.dabs, "a dab stream was rewritten");
                        assert_eq!((a.value, a.radius, a.flow, a.center_weight),
                            (b.value, b.radius, b.flow, b.center_weight));
                    }
                }
                (g, w) => assert_eq!(g, w, "component geometry moved"),
            }
        }
    }
}

/// v1.17.0: a recipe with channel curves and no composite curve writes the
/// composite as the identity. Camera Raw ignores `ToneCurvePV2012Red` /
/// `Green` / `Blue` in a sidecar with no `ToneCurvePV2012` element — measured
/// on a fitted recipe: its three curves rendered byte-identical to none, and
/// with an identity composite block they moved the frame 11.9 ΔE, the move
/// the engine renders. The identity reads back empty, so the recipe
/// round-trips unchanged; a recipe with no curve at all still writes none.
///
/// MUTATION: write `curve_elem("ToneCurvePV2012", &r.tone_curve)` again.
#[test]
fn channel_curves_always_travel_with_a_composite_curve() {
    use crate::recipe::CurvePoint;
    let r = EditRecipe {
        blue_curve: vec![CurvePoint { input: 0, output: 0 }, CurvePoint { input: 128, output: 150 }, CurvePoint { input: 255, output: 255 }],
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    let composite = xmp.find("<crs:ToneCurvePV2012>").expect("the composite curve is written");
    assert!(composite < xmp.find("<crs:ToneCurvePV2012Blue>").expect("the blue curve"), "in Lightroom's order");
    assert!(xmp.contains("<rdf:li>0, 0</rdf:li>\n     <rdf:li>255, 255</rdf:li>"), "as the identity: {xmp}");
    let back = xmp_to_recipe(&xmp);
    assert!(back.tone_curve.is_empty(), "the identity reads back as no curve");
    assert_eq!(back.blue_curve, r.blue_curve);
    assert!(!recipe_to_xmp(&EditRecipe::default()).contains("<crs:ToneCurvePV2012>"), "no curve, no element");
}
