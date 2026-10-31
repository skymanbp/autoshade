// One part of the sidecar's tests (src/xmp/tests.rs includes it): AI masks, nested components, single-quoted attributes and the declared frame scope.

/// THE DOMINANT REFUSAL, closed. Before R27 Batch-5 a correction holding a
/// `Mask/Image` was thrown away entire — 78 corrections across 40 files,
/// 40 % of every file in the reference library that has a mask at all —
/// and it took the gradient standing beside it with it.
///
/// MUTATION-LINED. Verified red by reverting the `"Mask/Image"` arm of
/// `classify_correction` to `unknown_component = true` (transcript in the
/// batch report): the correction disappears and the gradient with it.
#[test]
fn an_ai_mask_imports_beside_the_shapes_it_used_to_take_down() {
    let doc = lr_doc(&lr_correction(
        "Mask 1",
        "",
        &format!("{}{}", lr_gradient("0"), lr_ai_mask("2", "0", "1", LR_AI_PROVENANCE, "")),
    ));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the correction must import: {:?}", r.masks);
    let m = &r.masks[0];
    // The parametric shape is still the BASE — an AI mask does not displace
    // a gradient that was there first (`base_geometry_at`).
    assert!(matches!(m.mask, MaskGeometry::Linear { .. }), "{:?}", m.mask);
    assert_eq!(m.components.len(), 1, "the AI mask rides as a component");
    assert_eq!(m.components[0].mode, MaskCombine::Add, "MaskBlendMode=0 is a union");
    let MaskGeometry::AiMask {
        name,
        subtype,
        ref_x,
        ref_y,
        blend_mode,
        value,
        inverted,
        mask_version,
        provenance,
        gesture,
        raster,
    } = &m.components[0].geometry
    else {
        panic!("expected an AI mask, got {:?}", m.components[0].geometry);
    };
    assert_eq!(name.as_str(), "Sky 1");
    assert_eq!((*subtype, *blend_mode, *value, *inverted, *mask_version), (2, 0, 1.0, false, 1));
    assert_eq!((*ref_x, *ref_y), (0.605469, 0.281525), "the click arrives verbatim");
    assert!(gesture.is_empty(), "no crs:Gesture on this fixture");
    // NOTHING is resolved at parse time: importing a library must not spawn
    // a model run per photo.
    assert!(raster.is_none(), "the alpha is recomputed at DEVELOP time, not here");
    // Every provenance attribute, in document order, carried and untouched.
    let keys: Vec<&str> = provenance.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "InputDigest",
            "InputDigestVersion",
            "MaskDigest",
            "WholeImageArea",
            "Origin",
            "ModelVersion"
        ]
    );
    assert_eq!(provenance[3].1, "0/1,0/1,1920/1,2880/1");

    // And the import SAYS what kind of thing arrived — a RE-DERIVATION,
    // not Adobe's raster. Importing this silently would be worse than the
    // refusal it replaced.
    let losses = import_losses(&doc);
    assert!(
        losses.iter().any(|l| l.reason == MaskImportReason::AiMaskRecomputed),
        "a re-derived AI mask must be disclosed: {losses:?}"
    );
    let line = describe_import_losses(1, &losses).unwrap_or_default();
    assert!(
        line.contains("re-derived") && line.contains("Adobe"),
        "the prose must name the recomputation, not just the count: {line}"
    );
}

/// A correction whose ONLY component is an AI mask imports too, with the
/// AI mask as its base — the 59 corrections in the census that carry
/// nothing else.
#[test]
fn an_ai_only_correction_takes_the_ai_mask_as_its_base() {
    let doc = lr_doc(&lr_correction(
        "Mask 2",
        "",
        &lr_ai_mask("0", "0", "1", LR_AI_PROVENANCE, ""),
    ));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "an AI-only correction imports: {:?}", r.masks);
    assert!(matches!(r.masks[0].mask, MaskGeometry::AiMask { subtype: 0, .. }));
    assert!(r.masks[0].components.is_empty(), "the base is not also a component");
}

/// The `crs:Gesture` child — the photographer's brush refinement of the AI
/// mask (40 of 105 instances) — is carried, so the corrections whose only
/// brush content is a gesture arrive whole.
#[test]
fn an_ai_masks_gesture_strokes_are_carried() {
    let doc = lr_doc(&lr_correction(
        "Mask 3",
        "",
        &lr_ai_mask("2", "0", "1", LR_AI_PROVENANCE, &lr_paint_specimen()),
    ));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "{:?}", r.masks);
    let MaskGeometry::AiMask { gesture, .. } = &r.masks[0].mask else {
        panic!("expected an AI mask, got {:?}", r.masks[0].mask);
    };
    assert_eq!(gesture.len(), 1, "one Mask/Paint per Gesture, as measured");
    assert_eq!(gesture[0].value, 0.439815);
    assert_eq!(gesture[0].radius, 0.582157);
    assert!(gesture[0].dabs.starts_with("r 0.581835\nd 0.000684 0.940004"));
}

/// The write-back re-emits the component shape Lightroom wrote — the
/// intent verbatim, so Lightroom rebuilds ITS own alpha from it — with a
/// FRESH `crs:MaskSyncID` per this writer's rule.
///
/// MUTATION-LINED. Verified red by dropping the `ai_mask_xml` routing from
/// `masks_xml`'s base arm (the correction then exports as a bitmap-skip
/// and the whole mask disappears from the sidecar) — transcript in the
/// batch report.
#[test]
fn an_ai_mask_round_trips_back_into_the_sidecar() {
    let doc = lr_doc(&lr_correction(
        "Mask 4",
        "",
        &lr_ai_mask("2", "0", "1", LR_AI_PROVENANCE, ""),
    ));
    let r = xmp_to_recipe(&doc);
    let out = recipe_to_xmp(&r);
    assert!(out.contains(r#"crs:What="Mask/Image""#), "the component kind rides out:\n{out}");
    assert!(out.contains(r#"crs:MaskSubType="2""#), "the intent rides out:\n{out}");
    assert!(
        out.contains(r#"crs:ReferencePoint="0.605469 0.281525""#),
        "the click rides out at the file's own precision:\n{out}"
    );
    assert!(out.contains(r#"crs:MaskVersion="1""#), "the schema stamp rides out:\n{out}");
    for (k, v) in [
        ("InputDigest", "D0DAC04EB58F013F49D93EF47D22794E"),
        ("MaskDigest", "00D1A1B68591DF41F6CA3F8F805D0F1B"),
        ("WholeImageArea", "0/1,0/1,1920/1,2880/1"),
        ("ModelVersion", "234881976"),
    ] {
        assert!(
            out.contains(&format!("crs:{k}=\"{v}\"")),
            "provenance {k} must ride out unchanged:\n{out}"
        );
    }
    // FRESH SyncID — a sidecar we rewrite is OUR document, and this writer
    // mints identities for every component it emits.
    assert!(
        !out.contains("440777CD3CB8E24BB8E16028893B45DC"),
        "the file's own MaskSyncID must not be republished:\n{out}"
    );
    // And the EXPORT discloses the other direction of the same gap.
    let losses = mask_export_losses(&r);
    assert!(
        losses.iter().any(|l| l.reason == MaskLossReason::AiMaskRecomputed),
        "the export must say the pixels shown were not Adobe's: {losses:?}"
    );
}

/// The zoned reverse-fit's SKY zone rides out as LIGHTROOM'S OWN Select
/// Sky, and the land zone as that mask inverted.
///
/// What this replaces: both zones were `MaskGeometry::Bitmap`, which
/// classic ACR XMP has no encoding for at all, so `masks_xml` skipped the
/// whole correction with a named `MaskLossReason::Bitmap`. The sidecar
/// carried the global fit and the two edits that actually separate a
/// repainted sky from its ground never reached Lightroom. A `Mask/Image`
/// component with `crs:MaskSubType="2"` carries the same INTENT in a form
/// Lightroom rebuilds its own sky alpha from.
///
/// The honesty is unchanged and is asserted here: the pixels this app
/// showed came from OUR render, never Adobe's raster, which is exactly
/// what `MaskLossReason::AiMaskRecomputed` says — and it REPLACES the
/// `Bitmap` loss rather than joining it, because nothing was left out.
/// Recolour gains ride out on the mask's Temp/Tint; only gains past that
/// reach keep their own loss.
///
/// MUTATION-LINED: put `sky_attachment`/`land_attachment` in `fit_zoned`
/// back on `MaskGeometry::Bitmap` and the first half fails; drop the
/// `AiMaskRecomputed` push from `masks_xml` and the disclosure half does.
#[test]
fn a_reverse_fit_zone_round_trips_select_sky_role_and_inversion_home() {
    // ONE home for the inversion, exactly as `fit_zoned` builds it: the
    // CORRECTION carries the flag and the component carries `false`, so
    // `lr_net_inverted` is what reaches `crs:MaskInverted`.
    let zone = |inverted: bool, gains: Option<[f32; 3]>| EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::select_sky(0.5, 0.25, false, "mask-zone-sky.png".into()),
            role: if inverted {
                crate::recipe::MaskRole::ZoneLand
            } else {
                crate::recipe::MaskRole::ZoneSky
            },
            inverted,
            exposure_ev: -0.4,
            saturation: 12.0,
            color_gains: gains,
            ..Default::default()
        }],
        ..Default::default()
    };

    let r = zone(false, None);
    let (out, losses) = recipe_to_xmp_with_losses(&r);
    assert_eq!(
        out.matches(r#"crs:What="Mask/Image""#).count(),
        1,
        "exactly one Select Sky component reaches the sidecar:\n{out}"
    );
    assert!(out.contains(r#"crs:MaskSubType="2""#), "…and it is the SKY subtype:\n{out}");
    assert!(
        out.contains(r#"crs:ReferencePoint="0.5 0.25""#),
        "…prompted at the alpha's own centre of mass:\n{out}"
    );
    assert!(out.contains(r#"crs:MaskInverted="false""#), "…upright:\n{out}");
    assert!(out.contains(r#"crs:MaskVersion="1""#), "…with Lightroom's schema stamp:\n{out}");
    // The CORRECTION, which is the whole point: the zone's dials are in
    // the file now instead of being skipped along with the mask.
    assert!(
        out.contains(&format!("crs:LocalExposure2012=\"{}\"", local_fmt(-0.4 / 4.0))),
        "the zone's exposure rides out:\n{out}"
    );
    assert!(
        out.contains(&format!("crs:LocalSaturation=\"{}\"", local_fmt(12.0 / 100.0))),
        "…and its saturation:\n{out}"
    );
    // The disclosure: re-derived, NOT a dropped bitmap.
    assert!(
        losses.iter().any(|l| l.reason == MaskLossReason::AiMaskRecomputed),
        "the alpha shown here is ours, and the export must say so: {losses:?}"
    );
    assert!(
        !losses.iter().any(|l| l.reason == MaskLossReason::Bitmap),
        "nothing was left out of the sidecar, so there is no bitmap loss: {losses:?}"
    );

    // Recolour gains the white balance reaches ride out as the mask's
    // Temp/Tint (warm gains, a warm Temp) and lose nothing of their own;
    // gains past its reach keep their named loss beside the AI one.
    let warm = zone(false, Some([1.18, 0.96, 0.85]));
    let (warm_out, recoloured) = recipe_to_xmp_with_losses(&warm);
    let temp = warm_out
        .split("crs:LocalTemperature=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .and_then(|v| v.parse::<f32>().ok())
        .expect("a written Temp");
    assert!(temp > 0.0, "warm gains, warm Temp: {temp}");
    assert!(
        !recoloured.iter().any(|l| l.reason == MaskLossReason::Recolour)
            && recoloured.iter().any(|l| l.reason == MaskLossReason::AiMaskRecomputed),
        "carried, so only the AI sentence: {recoloured:?}"
    );
    let far = mask_export_losses(&zone(false, Some([3.0, 1.0, 0.2])));
    assert!(
        far.iter().any(|l| l.reason == MaskLossReason::Recolour)
            && far.iter().any(|l| l.reason == MaskLossReason::AiMaskRecomputed),
        "both sentences, neither swallowing the other: {far:?}"
    );

    // THE ROUND TRIP, through this app's own reader: the writer must emit
    // only what the parser's closed vocabulary accepts, or a sidecar we
    // wrote would fail to re-import — which is how a photographer loses a
    // mask to their own save.
    for inverted in [false, true] {
        let doc = recipe_to_xmp(&zone(inverted, None));
        // The sidecar carries the NET on the component, which is the only
        // place Lightroom has for it — the land zone's `crs:MaskInverted`
        // is `"true"` even though its geometry's own bit is `false`.
        assert!(
            doc.contains(&format!("crs:MaskInverted=\"{inverted}\"")),
            "inverted={inverted}: the net must reach the component:\n{doc}"
        );
        let back = xmp_to_recipe(&doc);
        assert_eq!(back.masks[0].role, if inverted { crate::recipe::MaskRole::ZoneLand } else { crate::recipe::MaskRole::ZoneSky });
        assert_eq!(back.masks.len(), 1, "inverted={inverted}: {:?}", back.masks);
        let MaskGeometry::AiMask {
            subtype,
            ref_x,
            ref_y,
            inverted: geom_inv,
            mask_version,
            ..
        } = &back.masks[0].mask
        else {
            panic!("inverted={inverted}: expected the Select Sky component back");
        };
        assert_eq!(
            (*subtype, *ref_x, *ref_y, *geom_inv, *mask_version),
            (2, 0.5, 0.25, false, 1),
            "inverted={inverted}: subtype, click and polarity all survive"
        );
        // Native CRS carries the net bit on the base geometry. Consistent
        // editor metadata restores its original home on the correction;
        // it must neither duplicate nor lose that inversion. The engine's
        // net remains the same fact after the trip.
        assert!(
            back.masks[0].inverted == inverted,
            "inverted={inverted}: the authored inversion home survives"
        );
        assert_eq!(
            back.masks[0].net_inverted(),
            inverted,
            "inverted={inverted}: the net is what round-trips"
        );
        assert_eq!(
            back.masks[0].exposure_ev, -0.4,
            "inverted={inverted}: and so does the correction"
        );
    }
}

/// An attribute outside the measured vocabulary is REFUSED, not carried.
/// A name we have never seen means a writer we have not measured (the
/// roundness rule) — and an open-ended attribute bag read off disk and
/// written back into XML is an injection surface besides.
///
/// MUTATION-LINED. Verified red by replacing `parse_ai_mask`'s
/// `_ => return Err(())` arm with `_ => {}` (transcript in the batch
/// report): the unknown attribute is silently dropped and the correction
/// imports as if the file had said nothing surprising.
#[test]
fn an_unmeasured_attribute_on_an_ai_mask_is_refused_not_guessed() {
    let good = lr_doc(&lr_correction("ok", "", &lr_ai_mask("2", "0", "1", "", "")));
    assert_eq!(xmp_to_recipe(&good).masks.len(), 1, "the baseline imports");

    for extra in [
        "\n         crs:SomethingNobodyMeasured=\"1\"",
        // A subtype outside {0,1,2} has no backend, and guessing one would
        // invent a selection.
        "",
    ] {
        let doc = if extra.is_empty() {
            lr_doc(&lr_correction("bad", "", &lr_ai_mask("7", "0", "1", "", "")))
        } else {
            lr_doc(&lr_correction("bad", "", &lr_ai_mask("2", "0", "1", extra, "")))
        };
        assert!(
            xmp_to_recipe(&doc).masks.is_empty(),
            "a Mask/Image outside the measured encoding must not import ({extra:?})"
        );
        let losses = import_losses(&doc);
        assert!(
            losses.iter().any(|l| l.reason.is_drop()),
            "and the refusal must be NAMED: {losses:?}"
        );
    }
}

/// `MaskBlendMode="1"` + `MaskValue="0"` is Lightroom's SUBTRACT pair, not
/// a muted mask — the same reading v0.31.1 taught this parser for
/// parametric components, mapped once through `brush_combine`.
#[test]
fn an_ai_masks_subtract_pair_reads_as_a_subtraction() {
    let doc = lr_doc(&lr_correction(
        "Mask 5",
        "",
        &format!("{}{}", lr_gradient("0"), lr_ai_mask("1", "1", "0", "", "")),
    ));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "{:?}", r.masks);
    assert_eq!(r.masks[0].components.len(), 1);
    assert_eq!(
        r.masks[0].components[0].mode,
        MaskCombine::Subtract,
        "MaskBlendMode=1 carves out"
    );
    let MaskGeometry::AiMask { blend_mode, value, .. } = &r.masks[0].components[0].geometry
    else {
        panic!("expected an AI mask");
    };
    // The pair is CARRIED verbatim for the writer even though the render
    // reads the projected `MaskCombine` — two spellings of one fact.
    assert_eq!((*blend_mode, *value), (1, 0.0));
}

// ================================================================
// R28 Batch-5 5d — THE FOUR ADVERSARIAL SCOPE FIXTURES (F4 A–D)
//
// All four are documents no Lightroom writes; the adjudication rated the
// whole finding "mechanism real, zero sites reachable from real LR". They
// are here because the DEFENCE used to be a coincidence — real Lightroom
// happens not to put these names in these places — and a coincidence is not
// a guard. The typed scope (`Tag` / `Scope`) plus the two narrowed searches
// make them refusals by construction, and these four fixtures say so.
// ================================================================

/// A correction with NO `crs:LocalExposure2012` of its own, one gradient
/// component, and whatever `extra` / `stray` the caller plants.
///
/// `extra` goes on the COMPONENT (inside `crs:CorrectionMasks`); `stray` is
/// spliced in as a child of the correction itself, before the component
/// list. Hand-written rather than built from `lr_correction` deliberately:
/// the point of A is a correction that OMITS a slider, and the Lightroom
/// fixture writes every one of them.
///
/// Authored by US (`x:xmptk="AutoShade 2"`), which matters for exactly one
/// thing: `component_import_reasons` accepts a `Mask/RangeMask` only on our
/// own documents (someone else's range encoding is not ours to interpret).
/// A Lightroom-authored fixture would drop the range for THAT reason and
/// the B control below could not tell the two refusals apart.
fn scope_bleed_doc(extra: &str, stray: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"AutoShade 2\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\"\n\
             \x20 xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             \x20 crs:HasSettings=\"True\">\n\
             \x20<crs:MaskGroupBasedCorrections><rdf:Seq>\n\
             \x20 <rdf:li><rdf:Description crs:What=\"Correction\"\n\
             \x20  crs:CorrectionAmount=\"1\" crs:CorrectionActive=\"true\"\n\
             \x20  crs:CorrectionName=\"Mask 1\" crs:LocalContrast2012=\"0.2\">\n\
             {stray}\
             \x20  <crs:CorrectionMasks><rdf:Seq>\n\
             \x20   <rdf:li crs:What=\"Mask/Gradient\" crs:MaskActive=\"true\"\n\
             \x20    crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" crs:MaskValue=\"1\"\n\
             \x20    crs:ZeroX=\"0.5\" crs:ZeroY=\"0.8\" crs:FullX=\"0.5\" crs:FullY=\"0.2\"{extra}/>\n\
             \x20  </rdf:Seq></crs:CorrectionMasks>\n\
             \x20 </rdf:Description></rdf:li>\n\
             \x20</rdf:Seq></crs:MaskGroupBasedCorrections>\n\
             </rdf:Description></rdf:RDF></x:xmpmeta>\n"
    )
}

/// **F4 SYMPTOM A** — a slider name on a NESTED component must not answer
/// for the correction.
///
/// The correction states no `crs:LocalExposure2012`; its gradient component
/// carries one, at a value (`9`, i.e. +36 EV on the ×4 file scale) far
/// outside Lightroom's own slider. The pre-5d reader scanned the whole
/// correction segment for every slider, so it found the component's number
/// and REFUSED the entire correction as out-of-model — the photographer
/// lost a mask because of an attribute on a shape.
///
/// The nearest real threat this closes: Lightroom really does write
/// `crs:Local*` NAMES on nested components (`LocalInputDigest` and friends,
/// 105 measured instances). They are strings nobody parses as numbers
/// today, which is why nothing has caught fire — a coincidence, now a
/// guard.
///
/// MUTATION: hand `correction_value_reasons` / `parse_one_correction` the
/// whole `seg` again instead of `own`, and the first assertion goes red.
#[test]
fn a_nested_components_slider_name_cannot_answer_for_the_correction() {
    let doc = scope_bleed_doc(" crs:LocalExposure2012=\"9\"", "");
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the correction must still import: {:?}", r.masks);
    assert_eq!(
        r.masks[0].exposure_ev, 0.0,
        "the correction states no exposure; the component's number is not its value"
    );
    // The correction's OWN sliders still arrive — this is a narrowing, not
    // a blindfold.
    assert_eq!(r.masks[0].contrast, 20.0, "crs:LocalContrast2012=0.2 → +20");
    assert_eq!(unsupported_corrections(&doc), 0, "and nothing was dropped");
}

/// **F4 SYMPTOM B** — a `Mask/RangeMask` outside `crs:CorrectionMasks` is
/// not this correction's range.
///
/// The stray element below sits inside the correction but outside its
/// component list, so no component walk ever counts it — which is exactly
/// why attaching it was SILENT: `range_count` stayed 0, so the
/// `ForeignRangeMask` disclosure could not fire either. The old search was
/// a first-occurrence scan over the whole segment, and the reader then ran
/// from that offset to the END of the segment, so even the colour arm's
/// `rdf:li` could come from an unrelated component.
///
/// MUTATION: restore `Scope::new(seg).find_value_at("What",
/// "Mask/RangeMask")` + `&seg[p..]` and the range comes back.
#[test]
fn a_range_mask_outside_the_component_list_is_not_attached() {
    let stray = "\x20 <rdf:li crs:What=\"Mask/RangeMask\" crs:MaskActive=\"true\"\n\
                     \x20  crs:MaskBlendMode=\"1\" crs:MaskValue=\"0\" crs:MaskInverted=\"true\"\n\
                     \x20  crs:LumRange=\"0 0.2 0.8 1\"/>\n";
    let doc = scope_bleed_doc("", stray);
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the correction still imports: {:?}", r.masks);
    assert!(
        r.masks[0].range.is_none(),
        "a range mask outside the component list is not this correction's: {:?}",
        r.masks[0].range
    );
    // …and the control: the SAME range component, INSIDE the list, is.
    let inside = scope_bleed_doc("", "").replace(
        "</rdf:Seq></crs:CorrectionMasks>",
        "<rdf:li crs:What=\"Mask/RangeMask\" crs:MaskActive=\"true\" \
             crs:MaskBlendMode=\"1\" crs:MaskValue=\"0\" crs:MaskInverted=\"true\" \
             crs:LumRange=\"0 0.2 0.8 1\"/></rdf:Seq></crs:CorrectionMasks>",
    );
    let r = xmp_to_recipe(&inside);
    assert!(
        matches!(r.masks[0].range, Some(RangeMask::Luminance { .. })),
        "an in-list range component must still be read, or this test proves nothing: {:?}",
        r.masks[0].range
    );
}

/// **F4 SYMPTOM C** — single-quoted attributes are legal XML, and the AI
/// mask's closed-vocabulary gate has to run on them.
///
/// `crs_attributes` hand-rolled its own lexer and looked for a DOUBLE
/// quote, so on this document it found none and returned an empty list.
/// Two consequences, both silent: the eleven provenance / digest keys were
/// dropped on write-back (Lightroom's own recompute ledger, gone from a
/// file we had just accepted), and the refusal loop that is supposed to
/// reject an unmeasured attribute name never looked at anything.
///
/// MUTATION: restore the `find('"')` lexer — the first half loses the
/// digests, the second half stops refusing.
#[test]
fn single_quoted_attributes_carry_provenance_and_still_refuse_the_unknown() {
    let ai = |extra: &str| {
        format!(
            "       <rdf:li><rdf:Description crs:What='Mask/Image' crs:MaskActive='true'\n\
                 \x20        crs:MaskName='Sky 1' crs:MaskBlendMode='0' crs:MaskInverted='false'\n\
                 \x20        crs:MaskSyncID='440777CD3CB8E24BB8E16028893B45DC' crs:MaskValue='1'\n\
                 \x20        crs:MaskVersion='1' crs:MaskSubType='2'\n\
                 \x20        crs:ReferencePoint='0.605469 0.281525'\n\
                 \x20        crs:InputDigest='D0DAC04EB58F013F49D93EF47D22794E'\n\
                 \x20        crs:ModelVersion='234881976'{extra}/></rdf:li>\n"
        )
    };
    let doc = lr_doc(&lr_correction("Mask 1", "", &ai("")));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "a single-quoted AI mask imports: {:?}", r.masks);
    let MaskGeometry::AiMask { provenance, .. } = &r.masks[0].mask else {
        panic!("expected an AI mask, got {:?}", r.masks[0].mask);
    };
    assert_eq!(
        provenance.len(),
        2,
        "both provenance keys must be CARRIED, not silently dropped: {provenance:?}"
    );
    // …and they come back out, which is the loss the photographer feels.
    let out = recipe_to_xmp(&r);
    assert!(
        out.contains("crs:InputDigest=\"D0DAC04EB58F013F49D93EF47D22794E\"")
            && out.contains("crs:ModelVersion=\"234881976\""),
        "the digests must survive the round trip:\n{out}"
    );
    // The refusal loop runs on legal XML now: an unmeasured name costs the
    // correction, exactly as it does with double quotes.
    let bogus = lr_doc(&lr_correction("Mask 1", "", &ai(" crs:Bogus='1'")));
    assert!(
        xmp_to_recipe(&bogus).masks.is_empty(),
        "an attribute outside the measured vocabulary must still refuse"
    );
}

/// **F4 SYMPTOM D** — the declared frame comes from ONE `rdf:Description`.
///
/// Width, length and orientation used to be three independent
/// first-occurrence searches over the whole document, so a packet carrying
/// a `tiff:ImageWidth` in one element and the real pair in another produced
/// a frame no element declares — and that frame is the coordinate system
/// every mask and crop decode folds pixel geometry with (`lr_to_engine`).
///
/// MUTATION: make [`FrameScope::resolve`] return `FrameScope(doc)`
/// unconditionally — i.e. point the three reads back at the whole document
/// — and the frame becomes 6000 × 6336, which this file never states.
#[test]
fn the_declared_frame_comes_from_one_description() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"6000\"/>\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"9504\" tiff:ImageLength=\"6336\" tiff:Orientation=\"1\"/>\n\
             </rdf:RDF></x:xmpmeta>";
    let frame = FrameAspect::from_xmp(doc).expect("the second Description declares a frame");
    assert_eq!(
        (frame.w, frame.h),
        (9504.0, 6336.0),
        "both dimensions must come from the element that declares both"
    );
    // A document with no `rdf:Description` at all still reads: the
    // narrowing has nothing to protect in a bare fragment, and fixtures in
    // this module that hand `from_xmp` a snippet rely on it.
    let bare = "tiff:ImageWidth=\"800\" tiff:ImageLength=\"600\"";
    let frame = FrameAspect::from_xmp(bare).expect("a bare fragment still declares a frame");
    assert_eq!((frame.w, frame.h), (800.0, 600.0));
}

// ================================================================
// R29-2 — THE FOUR ADVERSARIAL FRAME-SCOPE FIXTURES
//
// R28 Batch-5 5d typed the `crs:` reader's scope and left the `tiff:` frame
// family on a bare-`&str` `declared_number` whose scope was a per-call-site
// CONVENTION. `declared_number` is a method on [`FrameScope`] now, so the
// question can only be asked of a span [`FrameScope::resolve`] produced.
//
// Two of the four pin FALLBACKS, not guarantees. `resolve` deliberately
// widens to the whole document in the two cases its own doc comment names,
// and both are behaviour a photographer's file can reach; they are fixtures
// so that a later narrowing has to face them instead of changing the frame
// — the coordinate system every mask and crop decode folds pixel geometry
// with — by accident. None of these four documents is one Lightroom writes.
// ================================================================

/// **A** — HALF a frame in each of two `rdf:Description`s.
///
/// One element declares only `tiff:ImageWidth`, the next only
/// `tiff:ImageLength`. No element declares both, so `resolve` falls back to
/// the whole document and the pair IS assembled across two elements — the
/// pairing F4 symptom D removes when some element declares both, and which
/// this document gives the reader no way to avoid. PINNED, not endorsed:
/// the alternative is dropping the frame for files nobody has measured.
///
/// MUTATION: drop the fallback (`resolve` returning the last candidate span
/// instead of `doc`) and `from_xmp` returns `None` — the half-declaration
/// the narrowed span sees is not a frame, which the control below states
/// directly.
#[test]
fn half_a_frame_in_each_description_falls_back_to_the_whole_document() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"9504\"/>\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageLength=\"6336\" tiff:Orientation=\"1\"/>\n\
             </rdf:RDF></x:xmpmeta>";
    let frame = FrameAspect::from_xmp(doc).expect("the whole-document fallback still reads");
    assert_eq!(
        (frame.w, frame.h),
        (9504.0, 6336.0),
        "the documented fallback assembles the pair across the two elements"
    );
    // CONTROL: half a declaration is not a frame. Neither element on its
    // own answers, which is what makes the assertion above a statement
    // about the FALLBACK rather than about either span.
    let half = "<rdf:Description rdf:about=\"\" tiff:ImageWidth=\"9504\"/>";
    assert!(
        FrameAspect::from_xmp(half).is_none(),
        "a width with no length declares no rectangle"
    );
}

/// **B** — the two XMP spellings MIXED inside one `rdf:Description`.
///
/// `tiff:ImageWidth` is an attribute on the start tag (what Lightroom
/// writes); `tiff:ImageLength` and `tiff:Orientation` are property elements
/// in the body (the same properties' other legal spelling). The scope runs
/// from the element's `<` to the end of its body precisely so that one
/// element declaring both — in either spelling, or one of each — counts as
/// declaring both.
///
/// The first Description is a DECOY carrying a lone `tiff:ImageWidth="6000"`:
/// if the narrowing stopped working the whole-document fallback would read
/// its 6000 first and the frame would be 6000 × 6336.
///
/// MUTATION: make `resolve` end the span at the start tag's `>` (a `Tag`,
/// not a scope) and the mixed element stops declaring a length, so the
/// decoy's 6000 wins.
#[test]
fn the_frame_scope_sees_both_xmp_spellings_of_one_element() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"6000\"/>\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"9504\">\n\
             \x20<tiff:ImageLength>6336</tiff:ImageLength>\n\
             \x20<tiff:Orientation>8</tiff:Orientation>\n\
             </rdf:Description>\n\
             </rdf:RDF></x:xmpmeta>";
    let frame = FrameAspect::from_xmp(doc).expect("the mixed-spelling element declares both");
    assert_eq!(
        frame,
        FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::from_u16(8))
            .expect("a positive rectangle"),
        "all three come from the ONE element that declares the pair, either spelling"
    );
}

/// **C** — a bare fragment with no `rdf:Description` at all.
///
/// Both shapes fall back to the whole text, and that is load-bearing: this
/// module's own fixtures hand `from_xmp` snippets, and a reader given a
/// fragment cannot be mixing two elements' declarations because there are
/// no elements to mix. The element-form fragment is the sharper of the two
/// — it HAS markup, so it pins that the fallback keys on the absence of an
/// `rdf:Description`, not on the absence of tags.
///
/// MUTATION: make `resolve` return an empty span when no Description
/// matched and both halves of this test lose their frame.
#[test]
fn the_frame_scope_falls_back_to_a_bare_fragment() {
    let attrs = "tiff:ImageWidth=\"800\" tiff:ImageLength=\"600\" tiff:Orientation=\"1\"";
    let frame = FrameAspect::from_xmp(attrs).expect("an attribute fragment declares a frame");
    assert_eq!((frame.w, frame.h), (800.0, 600.0));
    let elems = "<tiff:ImageWidth>800</tiff:ImageWidth>\n\
             <tiff:ImageLength>600</tiff:ImageLength>";
    let frame = FrameAspect::from_xmp(elems).expect("an element fragment declares a frame");
    assert_eq!(
        (frame.w, frame.h),
        (800.0, 600.0),
        "markup without an rdf:Description is still a fragment"
    );
}

/// **D** — `rdf:Description`s present, none of them carrying both
/// dimensions.
///
/// Three elements, three separate properties: an orientation, a width, a
/// length. `resolve` finds no complete pair, falls back to the whole
/// document, and the frame is assembled from all THREE — the exact reading
/// F4 symptom D indicted, kept because refusing would drop the frame for a
/// document class nobody has measured (the aspect is disclosed as degraded
/// downstream either way). This is the residue R29-2 names rather than
/// closes.
///
/// MUTATION: return the FIRST `rdf:Description` seen instead of the
/// whole-document fallback and the frame disappears — that element declares
/// only an orientation.
#[test]
fn descriptions_without_a_complete_pair_read_as_the_whole_document() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:Orientation=\"8\"/>\n\
             <rdf:Description rdf:about=\"\" tiff:ImageWidth=\"9504\"/>\n\
             <rdf:Description rdf:about=\"\" tiff:ImageLength=\"6336\"/>\n\
             </rdf:RDF></x:xmpmeta>";
    let frame = FrameAspect::from_xmp(doc).expect("the whole-document fallback still reads");
    assert_eq!(
        frame,
        FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::from_u16(8))
            .expect("a positive rectangle"),
        "no element declares the pair, so all three properties come from the document"
    );
}

// Authored synthetic MaskBrushTable payloads, Brotli-compressed once with
// Python's `brotli` module. No byte below comes from a user specimen.
const MB_GOOD_A_LEN: usize = 185;
const MB_GOOD_A: &[u8] = &[
    0x1B, 0xB8, 0x00, 0xF8, 0x8F, 0xC2, 0xB6, 0xB5, 0x73, 0x94, 0x79, 0x28, 0xD3,
    0x42, 0xF8, 0xC9, 0x20, 0x88, 0x9B, 0xDF, 0xC6, 0x02, 0xEA, 0x3A, 0x0F, 0x6C,
    0x2C, 0x91, 0x28, 0x0E, 0x3C, 0xF0, 0x31, 0x51, 0xD6, 0x46, 0xAC, 0x01, 0x14,
    0x4E, 0xC4, 0xC3, 0x06, 0x9C, 0xA8, 0x07, 0x1E, 0xA0, 0x47, 0x32, 0xDD, 0x01,
    0x20, 0x10, 0xC7, 0x27, 0x96, 0x08, 0x80, 0x08, 0x00, 0x08, 0x00, 0x00, 0x58,
    0x10, 0xF9, 0xEE, 0xDC, 0x49, 0x6B, 0xC2, 0x58, 0x07, 0x20, 0x02, 0x42, 0x78,
    0x81, 0x98, 0x81, 0x5A, 0xD8, 0xAA, 0xBC, 0x89, 0xFA, 0x9B, 0xAA, 0x71, 0x28,
    0x13, 0x13, 0xC2, 0x58, 0xB7, 0xC6, 0x30, 0x36, 0x54, 0xBF, 0x44, 0x93, 0x3B,
    0x1A,
];
const MB_GOOD_B_LEN: usize = 87;
const MB_GOOD_B: &[u8] = &[
    0x1B, 0x56, 0x00, 0xF8, 0x9F, 0x07, 0x76, 0x0C, 0x99, 0x22, 0x68, 0xF8, 0x02,
    0xE9, 0xA5, 0x10, 0x26, 0xF7, 0x24, 0xE1, 0x08, 0xDB, 0x12, 0x4C, 0x23, 0xA8,
    0x84, 0xA0, 0x93, 0xE0, 0x81, 0xBA, 0x12, 0x66, 0x61, 0x03, 0x4E, 0x38, 0x0D,
    0x14, 0x47, 0x5A, 0x66, 0xBF, 0x1C, 0x20, 0xC1, 0x40, 0x03, 0xB5, 0x8C, 0xFB,
    0xE9, 0xD1, 0x02, 0x81, 0xBC, 0x35, 0x01,
];
const MB_UNKNOWN_OPCODE_LEN: usize = 83;
const MB_UNKNOWN_OPCODE: &[u8] = &[
    0x1B, 0x52, 0x00, 0xF8, 0x07, 0x61, 0x73, 0x13, 0xE9, 0x1A, 0xA2, 0xCD, 0x52,
    0xE5, 0xBC, 0x45, 0xD0, 0x05, 0x99, 0x7A, 0xA4, 0x03, 0x01, 0x80, 0x40, 0xA0,
    0x3C, 0x0C, 0x3E, 0xDD, 0x2B, 0xBD, 0x64, 0x08, 0xE4,
];
const MB_TRAILING_LEN: usize = 88;
const MB_TRAILING: &[u8] = &[
    0x1B, 0x57, 0x00, 0xF8, 0x9F, 0x07, 0x76, 0x0C, 0x99, 0x22, 0x68, 0xF8, 0x02,
    0xE9, 0xA5, 0x10, 0x26, 0xF7, 0x24, 0xE1, 0x08, 0xDB, 0x12, 0x4C, 0x23, 0xA8,
    0x84, 0xA0, 0x93, 0xE0, 0x81, 0xBA, 0x12, 0x66, 0x61, 0x03, 0x4E, 0x38, 0x0D,
    0x18, 0x37, 0x5A, 0x66, 0xBF, 0x1C, 0x20, 0xC1, 0x40, 0x03, 0xB5, 0x8C, 0xFB,
    0xE9, 0xD1, 0x02, 0x81, 0xBC, 0x35, 0x01,
];
const MB_TABLE_WORD_LEN: usize = 8;
const MB_TABLE_WORD: &[u8] =
    &[0x1B, 0x07, 0x00, 0xF8, 0xA7, 0x00, 0x04, 0x82, 0x92, 0x40, 0x20];
const MB_RECORD_BOUND_LEN: usize = 8;
const MB_RECORD_BOUND: &[u8] =
    &[0x8B, 0x03, 0x80, 0x01, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x03];
const MB_DCOUNT_BOUND_LEN: usize = 78;
const MB_DCOUNT_BOUND: &[u8] = &[
    0x1B, 0x4D, 0x00, 0xF8, 0x07, 0xE1, 0x64, 0x17, 0x12, 0x21, 0x6A, 0x4A, 0x35,
    0x1B, 0xD8, 0x80, 0x13, 0x4E, 0x03, 0x87, 0x05, 0x06, 0x5A, 0x4E, 0x07, 0x02,
    0x68, 0xC9, 0xC0, 0x02, 0xFD, 0xBC, 0xDA, 0x4B, 0x35, 0x14, 0x78, 0xAF,
];
const MB_TOKEN_BOUND_LEN: usize = 327_772;
const MB_TOKEN_BOUND: &[u8] = &[
    0x5B, 0x5B, 0x00, 0x85, 0x7F, 0x28, 0xF0, 0x76, 0x5F, 0x54, 0x42, 0xD4, 0x94,
    0x6A, 0x82, 0x76, 0x44, 0x97, 0xAE, 0x7E, 0x93, 0x08, 0x5C, 0xC0, 0x55, 0x49,
    0x08, 0x10, 0x80, 0x8D, 0x4F, 0xF7, 0x4D, 0xEB, 0xD0, 0x7D, 0xEF, 0x09, 0x20,
    0x84, 0xB1, 0x1F, 0x08,
];

fn mb_object(stream: &[u8]) -> Vec<u8> {
    let mut object = Vec::with_capacity(16 + stream.len());
    for word in [4u32, 1, 64_000, stream.len() as u32] {
        object.extend_from_slice(&word.to_le_bytes());
    }
    object.extend_from_slice(stream);
    object
}

fn mb_acr(objects: &[Vec<u8>]) -> (Vec<u8>, Vec<String>) {
    let directory_end = 20 + 32 * objects.len();
    let mut offsets = Vec::with_capacity(objects.len());
    let mut at = directory_end as u64;
    for object in objects {
        offsets.push(at);
        at += object.len() as u64;
        at += (4 - at % 4) % 4;
    }
    let mut acr = Vec::with_capacity(at as usize);
    acr.extend_from_slice(b"ACR\0");
    acr.extend_from_slice(&1u32.to_le_bytes());
    acr.extend_from_slice(b"ARW\0");
    acr.extend_from_slice(&(objects.len() as u32).to_le_bytes());
    acr.extend_from_slice(&0u32.to_le_bytes());
    let mut tokens = Vec::new();
    for (object, offset) in objects.iter().zip(offsets) {
        let digest = md5::compute(object);
        acr.extend_from_slice(&digest.0);
        acr.extend_from_slice(&(object.len() as u64).to_le_bytes());
        acr.extend_from_slice(&offset.to_le_bytes());
        tokens.push(format!("{digest:X}"));
    }
    for object in objects {
        acr.extend_from_slice(object);
        while acr.len() % 4 != 0 {
            acr.push(0);
        }
    }
    (acr, tokens)
}

fn mb_temp(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "autoshade-mask-brush-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("synthetic.arw");
    std::fs::write(&raw, b"synthetic raw identity").unwrap();
    (dir, raw)
}

fn mb_group(name: &str, token: &str, bytes: usize) -> String {
    format!(
        "<rdf:li crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\" \
             crs:MaskName=\"{name}\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" \
             crs:MaskSyncID=\"0000000000000000000000000000000D\" crs:MaskValue=\"1\" \
             crs:MaskBrushTable=\"{token}\" crs:MaskBrushUncompressedBytes=\"{bytes}\"/>\n"
    )
}

fn mb_doc(groups: &[(&str, &str, usize)]) -> String {
    let corrections: String = groups
        .iter()
        .map(|(correction, token, bytes)| {
            lr_correction(correction, "", &mb_group("Brush 1", token, *bytes))
        })
        .collect();
    lr_doc(&corrections)
}

fn mb_parse(
    raw: &std::path::Path,
    doc: &str,
) -> (EditRecipe, Vec<crate::diag::Line>) {
    let collector = crate::diag::Collector::new();
    let diag = crate::diag::Diag::about(&collector, raw);
    let recipe = xmp_to_recipe_with_diag(doc, &diag);
    (recipe, collector.take())
}

fn mb_assert_refusal(
    tag: &str,
    acr: Option<&[u8]>,
    token: &str,
    advertised: usize,
    expected: MaskBrushTableRefusal,
) {
    let (dir, raw) = mb_temp(tag);
    if let Some(acr) = acr {
        std::fs::write(raw.with_extension("acr"), acr).unwrap();
    }
    let doc = mb_doc(&[("Mask 1", token, advertised)]);
    let (recipe, lines) = mb_parse(&raw, &doc);
    assert!(recipe.masks.is_empty(), "a refused table imported partial geometry");
    let matching: Vec<_> = lines
        .iter()
        .filter(|line| line.text.contains(expected.name()))
        .collect();
    assert_eq!(matching.len(), 1, "named refusal must be loud exactly once: {lines:?}");
    let _ = std::fs::remove_dir_all(dir);
}
