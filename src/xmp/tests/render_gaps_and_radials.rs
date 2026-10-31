// One part of the sidecar's tests (src/xmp/tests.rs includes it): the render gaps, carried effects, the bitmap mask writer, mask loss reasons and Lightroom radials.

/// **The disclosure that had no other half** (R25 B4, work order 4.7):
/// `global_export_losses` has named "we render it, the sidecar cannot
/// carry it" since R24-5 M0; this names the opposite corner — Lightroom
/// renders it, this engine does not — which R25's B2/B3 batches filled
/// with twenty-four members and B4 with the twenty-fifth.
#[test]
fn the_render_gaps_name_what_lightroom_renders_and_this_engine_does_not() {
    use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
    // NEUTRAL SAYS NOTHING — and against the DEFAULT, not against zero.
    // The de-fringe block's neutral is Adobe's own 30/70/40/60 (R25 B3),
    // so a zero comparison would report a de-fringe gap on every single
    // photo ever opened, and a disclosure that fires always is read never.
    assert!(
        global_render_gaps(&EditRecipe::default()).is_empty(),
        "a default recipe carries no gap: {:?}",
        global_render_gaps(&EditRecipe::default())
    );
    let untouched = xmp_to_recipe(
        "<rdf:Description xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:DefringePurpleAmount=\"0\" crs:DefringePurpleHueLo=\"30\" \
             crs:DefringePurpleHueHi=\"70\" crs:DefringeGreenAmount=\"0\" \
             crs:DefringeGreenHueLo=\"40\" crs:DefringeGreenHueHi=\"60\"/>",
    );
    assert!(
        global_render_gaps(&untouched).is_empty(),
        "a real sidecar's resting de-fringe block is not a gap: {:?}",
        global_render_gaps(&untouched)
    );
    // A RENDERED control is not a gap, however far it is from neutral —
    // including the grain, which was this test's example until v1.5.0
    // gave the engine a stage after the crop (`render/finish.rs`).
    let bright = EditRecipe {
        exposure_ev: 2.0,
        texture: 40.0,
        grain: 30.0,
        post_crop_vignette: -40.0,
        ..Default::default()
    };
    assert!(global_render_gaps(&bright).is_empty(), "{:?}", global_render_gaps(&bright));
    // The B4 row is NOT here, and that is the tier's own definition
    // rather than an omission: we never interpret a pass-through value,
    // so we cannot tell Lightroom's resting `PerspectiveUpright="0"` — on
    // six of the seven reference sidecars, changing nothing anywhere —
    // from a real Upright correction. This sentence would then appear on
    // every Lightroom photo ever opened and drown the members that ARE
    // actionable. Its disclosure is the develop panel's own read-only
    // section, which shows the values instead of guessing at them.
    let upright = EditRecipe {
        passthrough: [("PerspectiveVertical".to_string(), "-35".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    assert!(
        global_render_gaps(&upright).is_empty(),
        "PassThrough has no knowable neutral, so it discloses through its section"
    );
    assert!(
        RECIPE_CONTROLS
            .iter()
            .any(|c| c.name == "passthrough" && c.tier == Some(Tier::PassThrough)),
        "premise: the row exists and renders nothing — the exclusion above is a choice"
    );
    // **v1.5.0's own sentence**: a recipe with EVERY control away from its
    // neutral still carries no gap, because Track F left nothing carried.
    // Built through serde so a renamed field cannot slip past.
    let mut every = serde_json::to_value(EditRecipe::default()).expect("serialises");
    for c in RECIPE_CONTROLS.iter().filter(|c| c.shape.is_scalar()) {
        every[c.name] = match c.shape {
            crate::advisor::catalogue::Shape::Bool => serde_json::json!(true),
            _ => serde_json::json!(c.range.map_or(7.0, |(_, hi)| hi.min(7.0))),
        };
    }
    let all: EditRecipe = serde_json::from_value(every).expect("in range");
    assert!(
        global_render_gaps(&all).is_empty(),
        "nothing is carried any more, so nothing can be a gap: {:?}",
        global_render_gaps(&all)
    );
    assert!(
        RECIPE_CONTROLS.iter().any(|c| c.name == "defringe_purple" && c.tier == Some(Tier::Rendered)),
        "premise: the de-fringe block was the LAST carried row, and it renders now — \
             without this the emptiness above would prove nothing about the batch"
    );
    // THE MECHANISM, which the real registry can no longer exercise: one
    // synthetic CarriedOnly row, over a real serde field so the neutral
    // comparison has something to read. This is the disclosure the day a
    // carried control comes back — and the reason `render_gaps_in` is a
    // function of its registry rather than a closure over the global one.
    let carried = crate::advisor::catalogue::Control {
        name: "defringe_purple",
        shape: crate::advisor::catalogue::Shape::Number,
        range: Some((0.0, 20.0)),
        neutral: "0",
        engine_only: true,
        crs: crate::advisor::catalogue::CrsKey::Attr("DefringePurpleAmount"),
        tier: Some(Tier::CarriedOnly),
        purpose: "a synthetic carried row: the disclosure has to work before it is needed",
    };
    let fringed = EditRecipe { defringe_purple: 3.0, ..Default::default() };
    assert_eq!(
        render_gaps_in(std::slice::from_ref(&carried), &fringed),
        vec!["defringe_purple"],
        "a carried control away from its neutral is named"
    );
    assert!(
        render_gaps_in(std::slice::from_ref(&carried), &EditRecipe::default()).is_empty(),
        "…and at its neutral it is not"
    );
    // The two disclosures are DISJOINT halves of one story, never the same
    // claim twice: a tier renders or it does not. Trivially satisfied while
    // the left half is empty — asserted anyway, because the day it is not
    // empty is the day this matters and nobody will think to add it then.
    for g in render_gaps_in(std::slice::from_ref(&carried), &fringed) {
        assert!(
            !global_export_losses(&fringed).contains(&g),
            "{g} cannot be both a render gap and an export loss"
        );
    }
    let _ = Tier::PassThrough; // the tier this batch populated
}

/// R25 B2 (policy SF4-C): the nine carried effects reach the sidecar and
/// the engine renders nothing from them.
///
/// The write rule is PER-KEY — every one of the nine is neutral at zero,
/// so "write what is non-neutral" needs no group gate, and the three whose
/// ACR default is not zero (Midpoint/Feather 50, Style 1) reach Lightroom
/// by ABSENCE rather than by a value we made up. Verified against the
/// user's own library, where Lightroom writes the companion keys only
/// alongside a non-zero amount.
#[test]
fn carried_effects_round_trip_and_render_nothing() {
    // The shape a real Lightroom sidecar takes (P34 / P05):
    // an amount plus its five companions, grain likewise.
    let r = EditRecipe {
        post_crop_vignette: -17.0,
        post_crop_vignette_mid: 50.0,
        post_crop_vignette_feather: 50.0,
        post_crop_vignette_round: 0.0,
        post_crop_vignette_style: 1.0,
        post_crop_vignette_hl: 0.0,
        grain: 30.0,
        grain_size: 25.0,
        grain_rough: 50.0,
        ..Default::default()
    };
    let xmp = recipe_to_xmp(&r);
    for want in [
        r#"crs:PostCropVignetteAmount="-17""#,
        r#"crs:PostCropVignetteMidpoint="50""#,
        r#"crs:PostCropVignetteFeather="50""#,
        r#"crs:PostCropVignetteStyle="1""#,
        r#"crs:GrainAmount="30""#,
        r#"crs:GrainSize="25""#,
        r#"crs:GrainFrequency="50""#,
    ] {
        assert!(xmp.contains(want), "{want} missing from: {xmp}");
    }
    // The two that are AT their neutral stay out — absence is how
    // Lightroom is told "keep your own default".
    assert!(!xmp.contains("PostCropVignetteRoundness"), "a zero roundness is not written");
    assert!(!xmp.contains("PostCropVignetteHighlightContrast"));
    // Every one comes back as itself — read side and write side in one
    // batch, exactly as for Texture above.
    let back = xmp_to_recipe(&xmp);
    for (name, live, want) in [
        ("post_crop_vignette", back.post_crop_vignette, r.post_crop_vignette),
        ("post_crop_vignette_mid", back.post_crop_vignette_mid, r.post_crop_vignette_mid),
        (
            "post_crop_vignette_feather",
            back.post_crop_vignette_feather,
            r.post_crop_vignette_feather,
        ),
        ("post_crop_vignette_round", back.post_crop_vignette_round, r.post_crop_vignette_round),
        ("post_crop_vignette_style", back.post_crop_vignette_style, r.post_crop_vignette_style),
        ("post_crop_vignette_hl", back.post_crop_vignette_hl, r.post_crop_vignette_hl),
        ("grain", back.grain, r.grain),
        ("grain_size", back.grain_size, r.grain_size),
        ("grain_rough", back.grain_rough, r.grain_rough),
    ] {
        assert_eq!(live, want, "{name} did not survive the round trip");
    }
    // A neutral recipe emits none of the nine (byte-compatible with the
    // pre-B2 writer for every recipe that never touched them).
    let neutral = recipe_to_xmp(&EditRecipe::default());
    for k in ["PostCropVignette", "Grain"] {
        assert!(!neutral.contains(k), "{k} must not appear on a neutral recipe: {neutral}");
    }
    // …and the ENGINE ignores all nine: the developed frame is
    // bit-identical to the neutral one. That is the claim
    // `Tier::CarriedOnly` makes, and it is the half a registry row cannot
    // prove about itself.
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(24, 16, |x, y| {
        image::Rgb([(x * 9) as u8, (y * 13) as u8, (x + y) as u8])
    }));
    assert_eq!(
        crate::render::develop_preview(&img, &r).to_rgb8().into_raw(),
        crate::render::develop_preview(&img, &EditRecipe::default()).to_rgb8().into_raw(),
        "a CarriedOnly control that moved a pixel would be mis-classified"
    );
}

/// One parametric mask + one raster mask — the fixture behind BOTH halves
/// of the raster contract: the writer emits no raster correction, and the
/// reader therefore returns no phantom for one.
fn mixed_parametric_and_raster() -> EditRecipe {
    EditRecipe {
        masks: vec![
            LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                exposure_ev: -1.0,
                ..Default::default()
            },
            LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "out/subject.png".into() },
                exposure_ev: 0.6,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

#[test]
fn bitmap_masks_are_skipped_by_the_xmp_writer() {
    use crate::recipe::MaskGeometry;
    let mixed = mixed_parametric_and_raster();
    let xmp = recipe_to_xmp(&mixed);
    assert!(xmp.contains("Mask/Gradient"), "the parametric mask must survive");
    assert_eq!(xmp.matches("crs:What=\"Correction\"").count(), 1, "raster correction skipped");
    assert!(!xmp.contains("subject.png"), "no raster path may leak into the sidecar");
    // All-raster: the whole corrections block disappears (no empty shell).
    let all_bitmap = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "out/sky.png".into() },
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(!recipe_to_xmp(&all_bitmap).contains("MaskGroupBasedCorrections"));
}

/// M6a: the export direction had FOUR silent losses (raster masks skipped,
/// muted masks skipped, Bitmap components omitted, radial rotation +
/// recolour gains dropped) against four import-side disclosures and zero
/// export-side ones. The writer now names them while it emits, and the
/// assertion is a SET comparison, not a count: a rule that fires on the
/// wrong mask, or twice on one mask, is exactly the bug a count hides.
#[test]
fn the_writer_names_bitmap_components_and_exports_native_components() {
    use crate::recipe::{MaskCombine, MaskComponent};
    let radial = |angle: f32| MaskGeometry::Radial {
        top: 0.3,
        left: 0.35,
        bottom: 0.7,
        right: 0.65,
        feather: 0.5,
        roundness: 0.0,
        flipped: false,
        angle,
        midpoint: 50.0,
        mask_version: 2,
    };
    let component = MaskComponent {
        inverted: false,
        geometry: MaskGeometry::Linear { zero_x: 0.1, zero_y: 0.1, full_x: 0.9, full_y: 0.9 },
        mode: MaskCombine::Subtract,
    };
    let r = EditRecipe {
        masks: vec![
            // Raster geometry: the whole correction goes.
            LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "out/sky.png".into() },
                name: "sky".into(),
                ..Default::default()
            },
            // Muted — and loaded with every degradation as well: the eye
            // is why it is skipped, so it must produce exactly ONE verdict
            // (this arm is what stops the counts from double-billing).
            LocalAdjustment {
                mask: radial(30.0),
                components: vec![component.clone()],
                color_gains: Some([1.4, 1.0, 0.6]),
                enabled: false,
                name: "parked".into(),
                ..Default::default()
            },
            // Base plus native component emit; only the Bitmap extra is lost.
            LocalAdjustment {
                components: vec![component.clone(), MaskComponent {
                    geometry: MaskGeometry::Bitmap { path: "out/component.png".into() },
                    mode: MaskCombine::Intersect, inverted: false,
                }],
                name: "combo".into(),
                ..Default::default()
            },
            // Emitted as an UNROTATED ellipse, its recolour past what the
            // mask's Temp/Tint can carry (`render::gains_as_white`).
            LocalAdjustment {
                mask: radial(-12.0),
                color_gains: Some([3.0, 1.0, 0.2]),
                name: "gold".into(),
                ..Default::default()
            },
            // Nothing lost: an unrotated radial, no components, neutral
            // gains (which are no recolour at all).
            LocalAdjustment {
                mask: radial(0.0),
                color_gains: Some([1.0, 1.0, 1.0]),
                name: "clean".into(),
                ..Default::default()
            },
            // Unnamed masks are identified by the name the SIDECAR would
            // have used, not by "".
            LocalAdjustment { mask: MaskGeometry::Bitmap { path: "out/x.png".into() }, ..Default::default() },
        ],
        ..Default::default()
    };
    let mut got: Vec<(String, MaskLossReason)> =
        mask_export_losses(&r).into_iter().map(|l| (l.name, l.reason)).collect();
    got.sort();
    let mut want = vec![
        ("sky".to_string(), MaskLossReason::Bitmap),
        ("parked".to_string(), MaskLossReason::Disabled),
        ("combo".to_string(), MaskLossReason::ComponentsFlattened),
        // …and the rotation verdict CARRIES the angle it dropped (R25
        // P5): −12°, not merely "some rotation". The disclosure surfaces
        // read the number off this payload, so a writer that raised the
        // reason with the wrong angle fails HERE, at the source, rather
        // than printing a plausible wrong number in the window.
        ("gold".to_string(), MaskLossReason::Rotation(-12)),
        ("gold".to_string(), MaskLossReason::Recolour),
        ("AutoShade 6".to_string(), MaskLossReason::Bitmap),
    ];
    want.sort();
    assert_eq!(got, want, "the loss set must name mask AND reason exactly once each");
    // The prose channel (CLI stderr / web reply) covers every category and
    // counts them; "combo" drops only its one Bitmap component.
    let line = describe_mask_losses(&mask_export_losses(&r)).expect("losses ⇒ a line");
    for expect in [
        "2 bitmap mask(s) skipped (sky, AutoShade 6)",
        "1 muted mask(s) skipped (parked)",
        "1 bitmap component(s) omitted (combo)",
        "1 radial rotation dropped (gold)",
        "1 recolour gains past the reach of Temp/Tint (gold)",
    ] {
        assert!(line.contains(expect), "the line must state {expect:?}: {line}");
    }
    // …and the emitted document agrees with the claim: four masks, one
    // skipped as raster, one as muted, and no leaked raster path.
    let doc = recipe_to_xmp(&r);
    assert_eq!(doc.matches("crs:What=\"Correction\"").count(), 3, "3 of 6 project");
    assert!(!doc.contains("sky.png"), "no raster path in a sidecar");

    // Nothing lossy ⇒ nothing said, on both channels (a faithful save
    // must not be interrupted).
    let faithful = EditRecipe { masks: vec![r.masks[4].clone()], ..Default::default() };
    assert!(mask_export_losses(&faithful).is_empty(), "an exportable mask loses nothing");
    assert!(describe_mask_losses(&[]).is_none(), "an empty list has nothing to say");
    assert!(mask_export_losses(&EditRecipe::default()).is_empty(), "no masks, no losses");
}

/// R25 P0-0.6: both disclosure surfaces ITERATE `MaskLossReason::ALL`
/// (here and the GUI's `xmp_loss_line`), so the list is the one place a
/// reason can be forgotten — and the match below is where a new variant
/// stops the build, with `ALL` the next thing it has to satisfy.
#[test]
fn mask_loss_reason_all_covers_every_variant() {
    // Adding a variant makes THIS match non-exhaustive; the arm you write
    // carries the next rank, and the two asserts then fail until `ALL`
    // lists the newcomer in that position.
    fn rank(r: MaskLossReason) -> usize {
        match r {
            MaskLossReason::Bitmap => 0,
            MaskLossReason::Disabled => 1,
            MaskLossReason::ComponentsFlattened => 2,
            MaskLossReason::BrushRendered => 3,
            MaskLossReason::AiMaskRecomputed => 4,
            MaskLossReason::Rotation(_) => 5,
            MaskLossReason::Recolour => 6,
            MaskLossReason::RasterNotEmbedded => 7,
        }
    }
    for (i, r) in MaskLossReason::ALL.into_iter().enumerate() {
        assert_eq!(rank(r), i, "ALL must list every reason once, in rank order");
        assert!(!r.en().trim().is_empty(), "{r:?} has no label for the prose channel");
        assert!(r.same_kind(r), "same_kind must be reflexive for {r:?}");
    }
    // R25 P5: `Rotation` grew a payload, so the grouping key is the
    // DISCRIMINANT — two masks tilted differently are one line in a
    // sentence and two values under `==`, and `ALL`'s placeholder `0`
    // equals neither of them. Same property the import twin relies on.
    assert!(
        MaskLossReason::Rotation(37).same_kind(MaskLossReason::Rotation(-12)),
        "two tilted masks are one line"
    );
    assert!(
        !MaskLossReason::Rotation(0).same_kind(MaskLossReason::Recolour),
        "different variants are different lines"
    );
    // Every reason the WRITER can raise reaches the prose. The mutation
    // this catches: a sixth reason raised by `masks_xml` and left out of
    // `ALL` would be silently invisible in the sentence.
    let losses: Vec<MaskLoss> = MaskLossReason::ALL
        .into_iter()
        .map(|reason| MaskLoss { name: format!("m{}", rank(reason)), reason })
        .collect();
    let line = describe_mask_losses(&losses).expect("five losses ⇒ a line");
    for r in MaskLossReason::ALL {
        assert!(line.contains(r.en()), "{r:?} never reaches the prose: {line}");
        assert!(line.contains(&format!("m{}", rank(r))), "{r:?} loses its mask name: {line}");
    }
}

// ── R25 P1: the import unlock ────────────────────────────────────────
//
// FIXTURE POLICY. This is a public repository and the reference sidecars
// are the user's own photographs, so no line of them is copied in. Every
// inline fixture below is SYNTHESISED: the attribute set, the value
// shapes, the attribute order and the element nesting are reproduced
// exactly as `P50.xmp` / `P51.xmp` write them, and every
// personal identifier (`crs:CorrectionName`, `crs:MaskName`, the sync
// GUIDs) carries a neutral test value instead. The real files are
// exercised by `real_lightroom_sidecars_import_their_parametric_masks`,
// which reads them from a path given at RUN time and skips when it is
// not set.

/// One `crs:What="Correction"` `<rdf:li>`, structurally verbatim: all 25
/// `crs:Local*` attributes in Lightroom's own order, the sliders on its
/// own 0..1 scale, `crs:CorrectionMasks` last. `locals` is spliced in
/// just before `LocalCurveRefineSaturation` (where an unknown key would
/// really sit); tests that need a DIFFERENT value for an existing key
/// rewrite it on the returned string, so the fixture can never carry the
/// same attribute twice.
fn lr_correction(name: &str, locals: &str, components: &str) -> String {
    lr_correction_with_curves(name, locals, "", components)
}

/// The same fixture with the correction's four LOCAL point curves spliced
/// in (R25 P6) — child ELEMENTS between the attribute block's closing `>`
/// and `<crs:CorrectionMasks>`, which is where the reference sidecars put
/// them. `lr_curve` builds one.
fn lr_correction_with_curves(
    name: &str,
    locals: &str,
    curves: &str,
    components: &str,
) -> String {
    format!(
        "     <rdf:li>\n\
             \x20     <rdf:Description\n\
             \x20      crs:What=\"Correction\"\n\
             \x20      crs:CorrectionAmount=\"1\"\n\
             \x20      crs:CorrectionActive=\"true\"\n\
             \x20      crs:CorrectionName=\"{name}\"\n\
             \x20      crs:CorrectionSyncID=\"0000000000000000000000000000000A\"\n\
             \x20      crs:LocalExposure=\"0\"\n\
             \x20      crs:LocalHue=\"0\"\n\
             \x20      crs:LocalSaturation=\"0\"\n\
             \x20      crs:LocalContrast=\"0\"\n\
             \x20      crs:LocalClarity=\"0\"\n\
             \x20      crs:LocalSharpness=\"0\"\n\
             \x20      crs:LocalBrightness=\"0\"\n\
             \x20      crs:LocalToningHue=\"0\"\n\
             \x20      crs:LocalToningSaturation=\"0\"\n\
             \x20      crs:LocalExposure2012=\"0.1\"\n\
             \x20      crs:LocalContrast2012=\"0.43\"\n\
             \x20      crs:LocalHighlights2012=\"0\"\n\
             \x20      crs:LocalShadows2012=\"0\"\n\
             \x20      crs:LocalWhites2012=\"0\"\n\
             \x20      crs:LocalBlacks2012=\"0\"\n\
             \x20      crs:LocalClarity2012=\"0\"\n\
             \x20      crs:LocalDehaze=\"0\"\n\
             \x20      crs:LocalLuminanceNoise=\"0\"\n\
             \x20      crs:LocalMoire=\"0\"\n\
             \x20      crs:LocalDefringe=\"0\"\n\
             \x20      crs:LocalTemperature=\"0.24\"\n\
             \x20      crs:LocalTint=\"0.44\"\n\
             \x20      crs:LocalTexture=\"0\"\n\
             \x20      crs:LocalGrain=\"0\"\n\
             {locals}\
             \x20      crs:LocalCurveRefineSaturation=\"100\">\n\
             {curves}\
             \x20     <crs:CorrectionMasks>\n\
             \x20      <rdf:Seq>\n\
             {components}\
             \x20      </rdf:Seq>\n\
             \x20     </crs:CorrectionMasks>\n\
             \x20     </rdf:Description>\n\
             \x20    </rdf:li>\n"
    )
}

/// One local point curve as Lightroom writes it inside a Correction: a
/// BARE key (`MainCurve`, not `ToneCurvePV2012`) and points spelled `x,y`
/// with NO space after the comma — structurally verbatim from
/// `P51.xmp`'s `<crs:RedCurve>` block, with test values.
fn lr_curve(tag: &str, points: &[(u8, u8)]) -> String {
    let pts: String = points
        .iter()
        .map(|(x, y)| format!("       <rdf:li>{x},{y}</rdf:li>\n"))
        .collect();
    format!(
        "      <crs:{tag}>\n       <rdf:Seq>\n{pts}       </rdf:Seq>\n      </crs:{tag}>\n"
    )
}

/// A radial component the way Lightroom writes one — `crs:Angle` and
/// `crs:MaskBlendMode` included, because it writes them on EVERY radial.
fn lr_radial(angle: &str, blend: &str) -> String {
    format!(
        "        <rdf:li\n\
             \x20        crs:What=\"Mask/CircularGradient\"\n\
             \x20        crs:MaskActive=\"true\"\n\
             \x20        crs:MaskName=\"Radial Gradient 1\"\n\
             \x20        crs:MaskBlendMode=\"{blend}\"\n\
             \x20        crs:MaskInverted=\"false\"\n\
             \x20        crs:MaskSyncID=\"0000000000000000000000000000000B\"\n\
             \x20        crs:MaskValue=\"1\"\n\
             \x20        crs:Top=\"0.114928\"\n\
             \x20        crs:Left=\"0.590368\"\n\
             \x20        crs:Bottom=\"0.802847\"\n\
             \x20        crs:Right=\"0.921381\"\n\
             \x20        crs:Angle=\"{angle}\"\n\
             \x20        crs:Midpoint=\"50\"\n\
             \x20        crs:Roundness=\"0\"\n\
             \x20        crs:Feather=\"100\"\n\
             \x20        crs:Flipped=\"true\"\n\
             \x20        crs:Version=\"2\"/>\n"
    )
}

/// A linear gradient component, same provenance.
fn lr_gradient(blend: &str) -> String {
    format!(
        "        <rdf:li\n\
             \x20        crs:What=\"Mask/Gradient\"\n\
             \x20        crs:MaskActive=\"true\"\n\
             \x20        crs:MaskName=\"Linear Gradient 1\"\n\
             \x20        crs:MaskBlendMode=\"{blend}\"\n\
             \x20        crs:MaskInverted=\"false\"\n\
             \x20        crs:MaskSyncID=\"0000000000000000000000000000000C\"\n\
             \x20        crs:MaskValue=\"1\"\n\
             \x20        crs:ZeroX=\"0.5\"\n\
             \x20        crs:ZeroY=\"0.8\"\n\
             \x20        crs:FullX=\"0.5\"\n\
             \x20        crs:FullY=\"0.2\"/>\n"
    )
}

/// The surrounding document: a Lightroom catalog export, NOT one of ours
/// (`x:xmptk` is Adobe's), which is the whole point — every gate this
/// batch reopened keyed on our own provenance.
fn lr_doc(corrections: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
             \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             \x20 <rdf:Description rdf:about=\"\"\n\
             \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             \x20   crs:Version=\"15.5.1\"\n\
             \x20   crs:ProcessVersion=\"15.4\"\n\
             \x20   crs:Exposure2012=\"+0.35\"\n\
             \x20   crs:HasSettings=\"True\">\n\
             \x20  <crs:MaskGroupBasedCorrections>\n\
             \x20   <rdf:Seq>\n\
             {corrections}\
             \x20   </rdf:Seq>\n\
             \x20  </crs:MaskGroupBasedCorrections>\n\
             \x20 </rdf:Description>\n\
             \x20</rdf:RDF>\n\
             </x:xmpmeta>\n"
    )
}

/// §0 OF THE ROUND. Lightroom writes `crs:Angle` on every radial — `"0"`
/// when the shape was never rotated — and the import refused the WHOLE
/// correction on its mere presence. Every radial mask in the user's
/// catalog therefore arrived as nothing, and the only thing said about it
/// was an integer count.
///
/// MUTATION THIS CATCHES: put `tag.crs_str("Angle").is_some()` back into
/// the refusal and this test goes to zero masks — which is exactly the
/// state the round opened in.
#[test]
fn a_lightroom_radial_with_angle_imports() {
    let doc = lr_doc(&lr_correction("Radial 1", "", &lr_radial("37.412506", "0")));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "a rotated Lightroom radial must import: {:?}", r.masks);
    // The angle itself is NOT mapped here — not because the sign or pivot
    // are unknown (v0.32.0 measured both), but because `lr_doc` declares
    // no frame, which is the one case left. The mask arrives as its
    // axis-aligned ellipse, and the note says so.
    let MaskGeometry::Radial { angle, top, feather, flipped, .. } = r.masks[0].mask else {
        panic!("expected a radial, got {:?}", r.masks[0].mask);
    };
    assert_eq!(angle, 0.0, "crs:Angle is disclosed, not guessed at");
    // v0.32.0: `lr_doc` declares no `tiff:ImageWidth/ImageLength`, so the
    // pixel→normalised fold has no aspect and the tilt is disclosed rather
    // than applied (the assert above, and the `Rotation` note below). The
    // FRAME AFFINE is applied to every radial — and since the 2026-08-19
    // ruling set `LR_MASK_FRAME_SCALE = 1.0` (Batch-10: the sidecar's
    // geometry lives in the PLAIN frame; the old 1.032 was one frame's
    // lens-profile warp), the affine is the identity and the corner is
    // the STORED number. Hand-checked: `cy = 0.4588875`,
    // `ry = 0.3439595`, top = cy − ry = 0.1149280.
    assert!(
        (top as f64 - 0.1149280).abs() < 1e-7,
        "the geometry is the file's, in the plain frame: {top}"
    );
    assert_eq!(feather, 1.0, "crs:Feather=100 is Lightroom's 0..100 scale");
    // R25 P9: `crs:Flipped="true"` beside `crs:MaskInverted="false"` is
    // Lightroom's NOT-inverted spelling — one bit written twice — so the
    // mask must arrive with neither flag set. It used to arrive flipped,
    // which inverted it.
    assert!(!flipped, "crs:Flipped is not a second inversion flag");
    assert!(!r.masks[0].inverted, "and MaskInverted=false is the file's actual verdict");
    assert_eq!(r.masks[0].exposure_ev, 0.4, "0.1 × 4 stops");
    assert_eq!(unsupported_corrections(&doc), 0, "nothing was refused");
    let losses = import_losses(&doc);
    assert_eq!(
        losses,
        vec![MaskImportLoss {
            name: "Radial 1".into(),
            // R25 P5: the verdict carries the sidecar's own angle, rounded
            // to whole degrees for the sentence that prints it. 37.412506
            // → 37; the recipe keeps nothing of it at all (the ellipse
            // imports axis-aligned), which is exactly why the disclosure
            // has to be able to say how much was set aside.
            reason: MaskImportReason::Rotation(37)
        }],
        "the rotation is named, with its angle, and it is the ONLY loss"
    );
}

// ── R25 P5: the geometry write-back (B5-B1) ──────────────────────────
//
// Same fixture policy as the block above — `lr_radial` reproduces
// Lightroom's own attribute set, order and value shapes for a radial
// component, with neutral identifiers. The two numbers that had to be
// REAL to be worth pinning (the out-of-frame corners) are the measured
// ones from the reference library.

/// `crs:Midpoint` and `crs:Version` sit on EVERY Lightroom radial, and
/// until this batch on neither side of this engine: not read, therefore
/// not kept, therefore deleted from the photographer's own sidecar the
/// first time AutoShade rewrote it. Both directions in one test, because
/// a read without a write is the worse half — it looks like it works.
///
/// The values are deliberately NON-default (37 / 3): 50 / 2 are what a
/// reader that ignores both attributes also produces.
///
/// MUTATION THIS CATCHES: default either field on the way in, or drop
/// either attribute on the way out, and 37 / 3 vanish.
#[test]
fn radial_midpoint_and_version_round_trip() {
    let comp = lr_radial("0", "0")
        .replace("crs:Midpoint=\"50\"", "crs:Midpoint=\"37\"")
        .replace("crs:Version=\"2\"", "crs:Version=\"3\"");
    let doc = lr_doc(&lr_correction("Radial 1", "", &comp));
    let r = xmp_to_recipe(&doc);
    let MaskGeometry::Radial { midpoint, mask_version, .. } = r.masks[0].mask else {
        panic!("expected a radial, got {:?}", r.masks[0].mask);
    };
    assert_eq!(midpoint, 37.0, "crs:Midpoint is read, not defaulted");
    assert_eq!(mask_version, 3, "crs:Version is read, not defaulted");
    assert!(import_losses(&doc).is_empty(), "carrying a value is not losing it");

    // …and out again, adjacent and in the writer's own order.
    let out = recipe_to_xmp(&r);
    assert!(
        out.contains("crs:Midpoint=\"37\" crs:Version=\"3\""),
        "both ride back out of the writer: {out}"
    );
    assert_eq!(
        xmp_to_recipe(&out).masks[0].mask,
        r.masks[0].mask,
        "our own document re-reads to the same geometry"
    );

    // The collision the bounded read exists for: `range_mask_xml` writes
    // `crs:Version="3"` on the RANGE component, which sits after the
    // geometry inside the same correction. An unbounded scan reads that
    // as the ellipse's schema stamp and quietly promotes every ranged
    // radial from 2 to 3.
    let ranged = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                midpoint: 50.0, mask_version: 2,
            },
            range: Some(RangeMask::Luminance {
                lo_outer: 0.1,
                lo: 0.2,
                hi: 0.8,
                hi_outer: 0.9,
            }),
            name: "ranged".into(),
            exposure_ev: 0.5,
            ..Default::default()
        }],
        ..Default::default()
    };
    let back = xmp_to_recipe(&recipe_to_xmp(&ranged));
    let MaskGeometry::Radial { mask_version, .. } = back.masks[0].mask else {
        panic!("expected a radial, got {:?}", back.masks[0].mask);
    };
    assert_eq!(mask_version, 2, "the range component's own Version is not the ellipse's");
}

/// The reference library holds radial corners on BOTH sides of the frame
/// — `crs:Bottom="1.802847"` in one file, `crs:Top="-0.153271"` in
/// another. ACR geometry is a centre+radii carrier, so this is ordinary,
/// not corruption: the reader already widened to ±8, and the WRITER must
/// not quietly pull them back to 0..1 on the way out (a clamp there
/// shortens the falloff of every off-frame gradient the user placed).
#[test]
fn out_of_frame_radial_corners_survive_a_round_trip() {
    let comp = lr_radial("0", "0")
        .replace("crs:Top=\"0.114928\"", "crs:Top=\"-0.153271\"")
        .replace("crs:Bottom=\"0.802847\"", "crs:Bottom=\"1.802847\"");
    let doc = lr_doc(&lr_correction("Radial 1", "", &comp));
    let r = xmp_to_recipe(&doc);
    let MaskGeometry::Radial { top, bottom, .. } = r.masks[0].mask else {
        panic!("expected a radial, got {:?}", r.masks[0].mask);
    };
    // The corners arrive VERBATIM (the frame affine is the identity since
    // the 2026-08-19 `LR_MASK_FRAME_SCALE = 1.0` ruling) — the point of
    // the test is that nothing pulls an off-frame corner back to 0..1,
    // and that holds on both boundaries.
    assert!(
        (top as f64 - -0.153271).abs() < 1e-7,
        "a corner above the frame is a real value: {top}"
    );
    assert!(
        (bottom as f64 - 1.802847).abs() < 1e-7,
        "and so is one below it: {bottom}"
    );
    let out = recipe_to_xmp(&r);
    // …and the WRITER hands the file its own numbers back, to the byte.
    assert!(out.contains("crs:Top=\"-0.153271\""), "written raw, not clamped: {out}");
    assert!(out.contains("crs:Bottom=\"1.802847\""), "written raw, not clamped: {out}");
    assert_eq!(xmp_to_recipe(&out).masks[0].mask, r.masks[0].mask, "and it is stable");
}

// ── v0.32.0: the radial geometry projection ─────────────────────────────
//
// The fixtures below are the SIDECAR NUMBERS of the user's own twelve-frame
// controlled Lightroom experiment, transcribed from
// `lr-experiment/probe2/probe2-extract.txt` in the R25 materials ledger, outside the tree
// and `.../lr-experiment/extract.txt`. No photograph and no export is in
// the repository — the RENDERED measurements those exports produced are
// quoted in the assertions, and the fixtures that reproduce them from the
// real files live behind `AUTOSHADE_LR_PROBE_FIXTURES` (see
// `the_probe_sidecars_decode_to_their_measured_ellipses`).

/// A radial component with arbitrary corners, otherwise byte-shaped like
/// [`lr_radial`] (which is transcribed from a real Lightroom write).
fn lr_radial_at(t: &str, l: &str, b: &str, r: &str, angle: &str) -> String {
    lr_radial(angle, "0")
        .replace("crs:Top=\"0.114928\"", &format!("crs:Top=\"{t}\""))
        .replace("crs:Left=\"0.590368\"", &format!("crs:Left=\"{l}\""))
        .replace("crs:Bottom=\"0.802847\"", &format!("crs:Bottom=\"{b}\""))
        .replace("crs:Right=\"0.921381\"", &format!("crs:Right=\"{r}\""))
}

/// Declare the frame on a document that had none. [`lr_doc`] deliberately
/// does NOT — that keeps every older test on the no-frame arm, which is a
/// real arm and has to stay covered — so the tests that need the aspect
/// add it here. Every real Lightroom sidecar carries these two
/// (`P19.xmp`: `tiff:ImageWidth="9504" tiff:ImageLength="6336"`,
/// which is the ARW's own `DefaultCropSize`).
fn in_frame(doc: &str, w: u32, h: u32) -> String {
    doc.replace(
        "crs:Version=\"15.5.1\"",
        &format!("tiff:ImageWidth=\"{w}\"\n   tiff:ImageLength=\"{h}\"\n   crs:Version=\"15.5.1\""),
    )
}

/// The engine's stored radial re-expressed as the PIXEL ellipse it draws:
/// `(semi-axis along the tilt, the other one, tilt in degrees)`, both axes
/// in pixels of a `w × h` frame. This is the quantity the probes measured,
/// so it is the quantity the assertions can quote.
fn engine_pixel_ellipse(m: &MaskGeometry, w: f64, h: f64) -> (f64, f64, f64) {
    let MaskGeometry::Radial { top, left, bottom, right, angle, .. } = m else {
        panic!("expected a radial, got {m:?}");
    };
    let (rx, ry) = (
        ((*right as f64 - *left as f64) / 2.0).abs() * w,
        ((*bottom as f64 - *top as f64) / 2.0).abs() * h,
    );
    // The engine rotates in the NORMALISED frame, so the pixel ellipse is
    // `diag(w, h)·R(angle)·diag(rx/w, ry/h)` — written out with the `w`/`h`
    // already folded into `rx`/`ry` above.
    let (sin, cos) = (*angle as f64).to_radians().sin_cos();
    let (s1, s2, tu) = svd2([cos * rx, -sin * ry * w / h, sin * rx * h / w, cos * ry]);
    (s1.abs(), s2.abs(), tu.to_degrees())
}

/// §0 OF v0.32.0. `crs:Top/Left/Bottom/Right` are the ROTATED CORNERS of
/// the ellipse's box in pixel space, and reading them as a bounding box —
/// which every build up to v0.31.2 did — gets the SHAPE wrong by factors,
/// not percentages.
///
/// Both rotated probes are here, and they are the two that discriminate:
///
/// | probe | file | `crs:Angle` | naive `a/b` | corner `a/b` | measured |
/// |---|---|---|---|---|---|
/// | #4 | `P24` | +24.348422 | **1.652** | **8.332** | 6.3–9.8, scan optimum 7.92 |
/// | #8 | `P22` | +29.513785 | **−0.032** (impossible) | **0.524** | 1.84–2.09, scan optimum 1.907 |
///
/// and `#8`'s decoded MAJOR axis is `b`, so its predicted screen tilt is
/// `θ + 90 = −60.486°` against a measured **−60.5° ± 0.9** (mean over the
/// τ ≥ 0.8 level sets). `PROBE2-VERDICT.md` §1, §5.
///
/// MUTATION THIS CATCHES: take `abs()` on `X`/`Y` in `lr_to_engine` and
/// `#8` — whose `Left > Right` — decodes to the naive sliver again; drop
/// the `sin`/`cos` mixing and both ratios collapse onto the naive column.
#[test]
fn a_rotated_lightroom_radial_decodes_to_the_measured_ellipse() {
    let (w, h) = (9504.0, 6336.0);
    // probe #4 — `P24`, an 8:1 sliver rotated +24.35°.
    let doc = in_frame(
        &lr_doc(&lr_correction(
            "R",
            "",
            &lr_radial_at("-0.045582", "-0.056408", "0.9708", "1.062771", "24.348422"),
        )),
        9504,
        6336,
    );
    let m = &xmp_to_recipe(&doc).masks[0].mask;
    // The RATIO is the model's zero-free-parameter prediction and carries
    // no `k`; the axes themselves are the decode × the frame scale, so they
    // are quoted against `k · a` and `k · b`.
    let k = LR_MASK_FRAME_SCALE;
    let (a, b, tilt) = engine_pixel_ellipse(m, w, h);
    assert!((a - k * 6172.8).abs() < 0.5, "semi-major {a} px, decoded 6172.8 × k");
    assert!((b - k * 740.8).abs() < 0.5, "semi-minor {b} px, decoded 740.8 × k");
    assert!((a / b - 8.332).abs() < 0.01, "axis ratio {} — the naive read is 1.652", a / b);
    assert!((tilt - 24.348422).abs() < 1e-3, "screen tilt {tilt}°, declared +24.348422");

    // probe #8 — `P22`, TALL (1:2) and rotated past the inversion
    // point, so Lightroom wrote `Left > Right`. The naive read makes
    // `X = −122 px` and the shape a sliver on the wrong axis.
    let doc = in_frame(
        &lr_doc(&lr_correction(
            "R",
            "",
            &lr_radial_at("-0.088191", "0.520214", "1.113528", "0.494492", "29.513785"),
        )),
        9504,
        6336,
    );
    let m = &xmp_to_recipe(&doc).masks[0].mask;
    let (a, b, tilt) = engine_pixel_ellipse(m, w, h);
    // The SVD reports the MAJOR axis first, and here that is the decoded
    // `b = 3373.2` at `θ + 90`.
    assert!((a - k * 3373.2).abs() < 0.5, "semi-major {a} px, decoded 3373.2 × k");
    assert!((b - k * 1769.1).abs() < 0.5, "semi-minor {b} px, decoded 1769.1 × k");
    assert!((b / a - 0.524).abs() < 0.001, "axis ratio {} — the naive read is −0.032", b / a);
    assert!((tilt - -60.486).abs() < 1e-2, "major-axis tilt {tilt}°, measured −60.5 ± 0.9");
}

/// One mask, one verdict: a mask that names the same missing raster
/// through its base geometry AND a component is one mask that lost one
/// raster, not two losses with the same name.
#[test]
fn a_mask_naming_one_missing_raster_twice_loses_it_once() {
    use crate::recipe::{MaskCombine, MaskComponent};
    let recipe = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "gone-raster.png".into() },
            components: vec![MaskComponent {
                geometry: MaskGeometry::Bitmap { path: "gone-raster.png".into() },
                mode: MaskCombine::Add,
                inverted: false,
            }],
            exposure_ev: 0.3,
            ..Default::default()
        }],
        ..Default::default()
    };
    let (_, losses) = recipe_to_xmp_with_losses(&recipe);
    assert_eq!(
        losses.iter().filter(|l| l.reason == MaskLossReason::RasterNotEmbedded).count(),
        1,
        "one raster, one verdict: {losses:?}"
    );
}

/// The corner encoding is a bijection, and the WRITER is its other half.
/// Both of Lightroom's legal corner arrangements go out byte-identical to
/// the way they came in — `Left < Right` (probe #4) and `Left > Right`
/// (probe #8, which the model REQUIRES to carry `Angle > 0`, 6/6 in the
/// user's library, `BBOX-DECODE.md` §2.1). Sorting or clamping the box on
/// the way out destroys the second one.
///
/// MUTATION THIS CATCHES: `min`/`max` the corners in `engine_to_lr`, or
/// drop the `|θ| ≤ 45°` canonicalisation, and probe #8 comes back as a
/// different ellipse.
#[test]
fn the_radial_corner_encoding_round_trips_both_lightroom_arrangements() {
    let frame = FrameAspect::from_size(9504.0, 6336.0);
    for (t, l, b, r, angle) in [
        ("-0.045582", "-0.056408", "0.9708", "1.062771", "24.348422"),
        ("-0.088191", "0.520214", "1.113528", "0.494492", "29.513785"),
        // `P19`, the subject the whole angle model was measured on.
        ("-0.082402", "-0.008723", "1.109604", "1.090228", "28.229232"),
        // …and an UNROTATED one, which must not acquire an angle.
        ("0.069396", "-0.059577", "0.855822", "1.06594", "0"),
    ] {
        let doc = in_frame(
            &lr_doc(&lr_correction("R", "", &lr_radial_at(t, l, b, r, angle))),
            9504,
            6336,
        );
        let recipe = xmp_to_recipe(&doc);
        let out = recipe_to_xmp_in_frame(&recipe, frame).0;
        let shown = &out[out.find("CircularGradient").unwrap_or(0)..];
        let shown = &shown[..shown.len().min(400)];
        // The four CORNERS come back to the byte.
        for (key, want) in [("Top", t), ("Left", l), ("Bottom", b), ("Right", r)] {
            assert!(
                out.contains(&format!("crs:{key}=\"{want}\"")),
                "crs:{key} must come back as {want}: {shown}"
            );
        }
        // The ANGLE cannot, and the reason is arithmetic rather than
        // geometric: `MaskGeometry::Radial::angle` is an `f32`, whose
        // ~6 × 10⁻⁸ relative precision is 2 × 10⁻⁶ of a 34° engine angle
        // and 7.6 × 10⁻⁶ of a 73° one — a unit or two in the sixth decimal
        // Lightroom writes. Measured here: 2 × 10⁻⁶ ° on `#4`, the worst
        // 1.4 × 10⁻⁵ ° on `#8` (whose engine angle is −73.198° and whose
        // decode goes through the ±45° axis swap). Bounded at 10⁻⁴ °, four
        // orders under the +0.33° systematic the tilt measurement that
        // calibrated this carries.
        let at = out.find("crs:Angle=\"").expect("an angle is written") + 11;
        let got: f64 = out[at..][..out[at..].find('"').unwrap()].parse().expect("a number");
        let want: f64 = angle.parse().unwrap();
        assert!((got - want).abs() < 1e-4, "crs:Angle {got} vs {want}: {shown}");
    }
}

/// The decode's own guard, and the one thing it still refuses: a
/// DEGENERATE fold. The sign law — `Left > Right` forces `Angle > 0`,
/// `Top > Bottom` forces `Angle < 0`, both at once impossible, and the
/// library agrees 16/16, p = 2.5 × 10⁻⁵ — is a true statement about what
/// Lightroom WRITES. It was ALSO wired up as a readability gate, and
/// me6-2026-09 group D refuted that half: Lightroom draws the ellipse of
/// the folded MAGNITUDES ([`RadialDecode::Refused`] carries the numbers).
/// So an inverted pair imports, and only a zero fold is refused and NAMED.
///
/// MUTATION THIS CATCHES: delete the guard and a zero-area box imports as
/// a hairline (the renderer's `max(1e-4)`) with no disclosure at all;
/// put the sign half of it back and the group-D shape stops rendering.
#[test]
fn a_radial_whose_corners_do_not_decode_is_refused_and_named() {
    // A zero-AREA box: both corner pairs coincide, so the fold is (0, 0) at
    // every tilt. No ellipse, however it is turned.
    let doc = in_frame(
        &lr_doc(&lr_correction(
            "Impossible",
            "",
            &lr_radial_at("0.500000", "0.500000", "0.500000", "0.500000", "-29.513785"),
        )),
        9504,
        6336,
    );
    assert!(xmp_to_recipe(&doc).masks.is_empty(), "a box that cannot decode must not render");
    assert_eq!(unsupported_corrections(&doc), 1, "and it is counted as a drop");
    assert_eq!(
        import_losses(&doc),
        vec![MaskImportLoss {
            name: "Impossible".into(),
            reason: MaskImportReason::OutOfModel
        }],
        "…and NAMED"
    );
    // A zero-WIDTH box at zero tilt is degenerate for the same reason — at
    // θ = 0 the fold is the box. Under a tilt the SAME corners are not
    // degenerate at all: Lightroom stores the ROTATED corners, so a stored
    // width of zero folds to the honest pair (Y·sinθ, Y·cosθ). Both arms
    // are asserted here so that neither can drift into the other.
    let flat = |angle: &str| {
        in_frame(
            &lr_doc(&lr_correction(
                "Flat",
                "",
                &lr_radial_at("0.300000", "0.500000", "0.700000", "0.500000", angle),
            )),
            9504,
            6336,
        )
    };
    assert!(xmp_to_recipe(&flat("0")).masks.is_empty(), "zero-width at θ = 0 must not render");
    assert_eq!(unsupported_corrections(&flat("0")), 1, "zero-width at θ = 0 is a drop");
    assert_eq!(xmp_to_recipe(&flat("-29.513785")).masks.len(), 1, "its tilted fold is an ellipse");
    // A box the sign law says Lightroom never writes still RENDERS — the
    // refusal is about degeneracy, not about the file being unusual. Both
    // tilts of the same corners import, and each is a real ellipse rather
    // than the hairline a degenerate fold would leave.
    let shape = |angle: &str| {
        let doc = in_frame(
            &lr_doc(&lr_correction(
                "Fine",
                "",
                &lr_radial_at("-0.088191", "0.520214", "1.113528", "0.494492", angle),
            )),
            9504,
            6336,
        );
        let masks = xmp_to_recipe(&doc).masks;
        assert_eq!(masks.len(), 1, "angle {angle} did not import");
        assert_eq!(unsupported_corrections(&doc), 0, "angle {angle} was still counted a drop");
        engine_pixel_ellipse(&masks[0].mask, 9504.0, 6336.0)
    };
    for angle in ["-29.513785", "29.513785"] {
        let (major, minor, _) = shape(angle);
        assert!(major > 300.0 && minor > 300.0, "angle {angle} is a hairline: {major} x {minor}");
    }
}

/// The frame affine is the IDENTITY — the 2026-08-19 ruling set
/// `LR_MASK_FRAME_SCALE = 1.0` after R27 Batches 8+10 proved the old
/// `k = 1.032` was one frame's LENS-PROFILE WARP mistaken for a constant
/// (`batch10-report.md` §5: the `LensProfileEnable` toggle moves the
/// implied scale 0.984 → 0.998, and 11 dabs displace as a radial
/// distortion polynomial, not a scale). The recipe carries the sidecar's
/// STORED geometry verbatim.
///
/// The history this test used to pin, kept legible: `P47`
/// (`Feather="0"`, centre 2799 px off frame-centre) RENDERS its centre at
/// **(2571.0, 5060.0)** px (`PROBE4-FINAL.md` §2, 2880-ray edge fit) —
/// ~88 px from the stored (2638.4, 5002.8), because THAT frame's warp is
/// ≈1.0315. That warp is now UNMODELLED BY DECISION (batch10 §7.5: no
/// `.lcp` reader yet), so the recipe must hold the stored centre and the
/// 88 px is the disclosed residual, not something to bake in.
///
/// MUTATION THIS CATCHES: put any `k ≠ 1` back (1.032, or half-apply it
/// to axes only) and the verbatim assertions fail by 29–88 px.
#[test]
fn the_frame_affine_is_the_identity_since_the_lens_warp_ruling() {
    let (w, h) = (9504.0, 6336.0);
    let doc = in_frame(
        &lr_doc(&lr_correction(
            "R",
            "",
            &lr_radial_at("0.597862", "0.009087", "0.981315", "0.546133", "0"),
        )),
        9504,
        6336,
    );
    let MaskGeometry::Radial { top, left, bottom, right, .. } =
        xmp_to_recipe(&doc).masks[0].mask
    else {
        panic!("expected a radial");
    };
    let (cx, cy) = (
        (left as f64 + right as f64) / 2.0 * w,
        (top as f64 + bottom as f64) / 2.0 * h,
    );
    // The STORED centre, verbatim.
    let stored = ((0.009087 + 0.546133) / 2.0 * w, (0.597862 + 0.981315) / 2.0 * h);
    assert!((cx - stored.0).abs() < 0.01, "centre x {cx} px must be the stored {}", stored.0);
    assert!((cy - stored.1).abs() < 0.01, "centre y {cy} px must be the stored {}", stored.1);
    // …and PROBE4's warped-render measurement stays ~88 px away — the
    // known, disclosed, unmodelled lens warp of that frame.
    assert!(
        (2571.0f64 - cx).hypot(5060.0 - cy) > 80.0,
        "the warp residual on P47 is real and unmodelled: ({cx}, {cy})"
    );
}

/// v0.32.0 narrowed the rotation disclosure to "the document declares no
/// frame" — but there are TWO ways the tilt fails to arrive, and the frame
/// narrows only one. An angle that cannot be PARSED is a rotation nobody
/// can apply however well the frame is known: `parse_one_correction` reads
/// it as 0, so the mask arrives axis-aligned and the file's own intent is
/// gone. That has to keep saying so.
///
/// MUTATION THIS CATCHES: gate the reason on `frame.is_none()` alone and
/// this correction imports rotated-to-zero in silence.
#[test]
fn an_unreadable_angle_is_disclosed_even_when_the_frame_is_known() {
    let doc = in_frame(
        &lr_doc(&lr_correction(
            "Garbled",
            "",
            &lr_radial_at("-0.045582", "-0.056408", "0.9708", "1.062771", "twenty-four"),
        )),
        9504,
        6336,
    );
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the shape is still readable, so the mask arrives");
    let MaskGeometry::Radial { angle, .. } = r.masks[0].mask else { panic!("radial") };
    assert_eq!(angle, 0.0, "an angle we cannot parse is not an angle we can apply");
    assert_eq!(
        import_losses(&doc),
        vec![MaskImportLoss {
            name: "Garbled".into(),
            // `0` is this payload's word for "no angle to name" — see the
            // variant's doc.
            reason: MaskImportReason::Rotation(0)
        }],
        "…and it is NAMED, frame or no frame"
    );
}
