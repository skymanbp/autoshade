use super::*;
use crate::recipe::MaskRole;

fn linear() -> MaskGeometry {
    MaskGeometry::Linear { zero_x: 0.25, zero_y: 0.25, full_x: 0.75, full_y: 0.75 }
}

fn radial(flipped: bool, angle: f32) -> MaskGeometry {
    MaskGeometry::Radial {
        top: 0.25, left: 0.25, bottom: 0.75, right: 0.75, feather: 0.5,
        roundness: 0.0, flipped, angle, midpoint: 50.0, mask_version: 2,
    }
}

#[test]
fn combine_spelling_matches_all_census_rows_and_both_inversions() {
    // Add: Gradient 163, Circular 148+24 inverted, Image 50+19 inverted,
    // Aggregate 17, Range 11+1 inverted. Subtract: 32/7/33/22/1.
    // Intersect: Gradient 6, Circular 22, Image/Aggregate/Range 1 each.
    for own in [false, true] {
        assert_eq!(combine_spelling(MaskCombine::Add, own), (0, own, 1.0));
        assert_eq!(combine_spelling(MaskCombine::Subtract, own), (1, own, 0.0));
        assert_eq!(combine_spelling(MaskCombine::Intersect, own), (1, !own, 0.0));
    }
}

#[test]
fn native_compositions_round_trip_editor_intent_and_every_coverage_sample() {
    let root = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
        .join("test-rasters");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(format!("composition-sky-{}.png", std::process::id()));
    let alpha = image::GrayImage::from_fn(96, 64, |x, y| image::Luma([((x + 2 * y) % 256) as u8]));
    alpha.save(&path).unwrap();
    let ai = MaskGeometry::select_sky(0.5, 0.25, false, path.to_string_lossy().into_owned());
    let reference = image::DynamicImage::new_rgb8(96, 64);
    for (base, geometry, mode, component_inverted, whole_inverted) in [
        (linear(), radial(false, 0.0), MaskCombine::Intersect, false, false),
        (ai.clone(), linear(), MaskCombine::Subtract, false, false),
        (radial(false, 0.0), linear(), MaskCombine::Add, false, false),
        (linear(), radial(true, 0.0), MaskCombine::Intersect, false, false),
        (ai.clone(), linear(), MaskCombine::Intersect, true, false),
        (linear(), radial(false, 0.0), MaskCombine::Subtract, false, true),
        (linear(), radial(true, 0.0), MaskCombine::Add, false, true),
        (linear(), radial(false, 0.0), MaskCombine::Intersect, false, true),
    ] {
        let original = LocalAdjustment {
            mask: base.clone(), components: vec![MaskComponent { geometry, mode, inverted: component_inverted }],
            role: MaskRole::ZoneSky, inverted: whole_inverted, ..Default::default()
        };
        let recipe = EditRecipe { masks: vec![original.clone()], ..Default::default() };
        let (doc, losses) = recipe_to_xmp_in_frame(&recipe, FrameAspect::from_size(96.0, 64.0));
        assert!(!losses.iter().any(|l| matches!(l.reason, MaskLossReason::ComponentsFlattened | MaskLossReason::Rotation(_))));
        let mut reread = xmp_to_recipe(&doc);
        assert_eq!(reread.masks.len(), 1, "{doc}");
        let got = &mut reread.masks[0];
        assert_eq!(got.role, original.role);
        assert_eq!(got.inverted, original.inverted);
        assert_eq!(got.components, original.components, "{doc}");
        // AI alpha is intentionally recomputed. Supply the SAME measured
        // alpha to isolate composition from that already named export loss.
        if let MaskGeometry::AiMask { raster, .. } = &mut got.mask {
            *raster = Some(path.to_string_lossy().into_owned());
        }
        assert_eq!(got.mask, original.mask);
        let coverage = |m| crate::render::mask_coverage(m, &reference, crate::render::MaskFrame::AsRendered);
        assert_eq!(coverage(got), coverage(&original), "all 6144 samples must agree: {mode:?}");
        let mut native_only = doc.clone();
        while let Some(start) = native_only.find(" ash:") {
            let value = start + native_only[start..].find("=\"").unwrap() + 2;
            let end = value + native_only[value..].find('"').unwrap() + 1;
            native_only.replace_range(start..end, "");
        }
        let mut native = xmp_to_recipe(&native_only);
        if let MaskGeometry::AiMask { raster, .. } = &mut native.masks[0].mask {
            *raster = Some(path.to_string_lossy().into_owned());
        }
        assert_eq!(coverage(&native.masks[0]), coverage(&original),
            "CRS alone carries coverage even when editor intent metadata is absent: {mode:?}");
    }
}

#[test]
fn mixed_component_order_and_whole_inversion_survive_native_export() {
    let image = image::DynamicImage::new_rgb8(127, 73);
    for inverted in [false, true] {
        let original = LocalAdjustment {
            mask: linear(), inverted,
            components: vec![
                MaskComponent { geometry: radial(false, 0.0), mode: MaskCombine::Subtract, inverted: false },
                MaskComponent { geometry: linear(), mode: MaskCombine::Add, inverted: true },
                MaskComponent { geometry: radial(true, 0.0), mode: MaskCombine::Intersect, inverted: false },
            ], ..Default::default()
        };
        let recipe = EditRecipe { masks: vec![original.clone()], ..Default::default() };
        let got = xmp_to_recipe(&recipe_to_xmp(&recipe));
        assert_eq!(got.masks[0].components, original.components);
        let coverage = |m| crate::render::mask_coverage(m, &image, crate::render::MaskFrame::AsRendered);
        assert_eq!(coverage(&got.masks[0]), coverage(&original));
    }
}

#[test]
fn census_shaped_sky_intersect_radial_imports_as_intersect() {
    // Synthesised from the census's vocabulary, never a copied user sidecar.
    let doc = r#"<rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction"
crs:CorrectionName="Sky band" crs:CorrectionAmount="1" crs:CorrectionActive="true">
<crs:CorrectionMasks><rdf:Seq>
<rdf:li><rdf:Description crs:What="Mask/Image" crs:MaskActive="true" crs:MaskName="Sky 1"
crs:MaskBlendMode="0" crs:MaskInverted="false" crs:MaskValue="1" crs:MaskVersion="1"
crs:MaskSubType="2" crs:ReferencePoint="0.5 0.25"/></rdf:li>
<rdf:li crs:What="Mask/CircularGradient" crs:MaskActive="true" crs:MaskBlendMode="1"
crs:MaskInverted="true" crs:MaskValue="0" crs:Top="0.25" crs:Left="0.25" crs:Bottom="0.75"
crs:Right="0.75" crs:Feather="50" crs:Roundness="0" crs:Flipped="false" crs:Angle="0"/>
</rdf:Seq></crs:CorrectionMasks></rdf:Description></rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>
</rdf:Description>"#;
    let recipe = xmp_to_recipe(doc);
    assert_eq!(recipe.masks.len(), 1);
    assert!(matches!(recipe.masks[0].mask, MaskGeometry::AiMask { .. }));
    assert_eq!(recipe.masks[0].components, vec![MaskComponent {
        geometry: radial(false, 0.0), mode: MaskCombine::Intersect, inverted: false,
    }]);
}

#[test]
fn edited_ai_component_mode_overrides_imported_blend_and_add_keeps_plain_zero() {
    let mut ai = MaskGeometry::select_sky(0.5, 0.25, false, String::new());
    if let MaskGeometry::AiMask { blend_mode, value, .. } = &mut ai { *blend_mode = 1; *value = 0.0; }
    for mode in [MaskCombine::Add, MaskCombine::Subtract, MaskCombine::Intersect] {
        let recipe = EditRecipe { masks: vec![LocalAdjustment {
            mask: linear(), components: vec![MaskComponent { geometry: ai.clone(), mode, inverted: false }],
            ..Default::default()
        }], ..Default::default() };
        let doc = recipe_to_xmp(&recipe);
        let got = xmp_to_recipe(&doc);
        assert_eq!(got.masks[0].components[0].mode, mode);
        let MaskGeometry::AiMask { blend_mode, value, .. } = got.masks[0].components[0].geometry else { panic!("AI"); };
        assert_eq!(blend_mode, if mode == MaskCombine::Add { 0 } else { 1 });
        assert_eq!(value, 0.0, "plain zero retains its carried spelling only for Add");
        let doc2 = recipe_to_xmp(&got);
        assert_eq!(doc, doc2, "a pure second round trip is byte-identical");
    }
}

#[test]
fn edited_brush_component_uses_the_same_spelling_and_preserves_plain_add_zero() {
    let brush = MaskGeometry::Brush {
        name: "Brush 1".into(), blend_mode: 1, value: 0.0, inverted: false,
        strokes: vec![BrushStroke { dabs: "d 0.5 0.5".into(), radius: 0.1, ..Default::default() }],
    };
    for mode in [MaskCombine::Add, MaskCombine::Subtract, MaskCombine::Intersect] {
        let recipe = EditRecipe { masks: vec![LocalAdjustment {
            mask: linear(), components: vec![MaskComponent { geometry: brush.clone(), mode, inverted: false }],
            ..Default::default()
        }], ..Default::default() };
        let doc = recipe_to_xmp(&recipe);
        let got = xmp_to_recipe(&doc);
        assert_eq!(got.masks.len(), 1, "{doc}");
        assert_eq!(got.masks[0].components[0].mode, mode);
        let MaskGeometry::Brush { blend_mode, value, .. } = got.masks[0].components[0].geometry else { panic!("Brush"); };
        assert_eq!(blend_mode, if mode == MaskCombine::Add { 0 } else { 1 });
        assert_eq!(value, 0.0);
        assert_eq!(doc, recipe_to_xmp(&got));
    }
}

#[test]
fn a_complemented_subtract_writes_add_one_even_when_the_import_carried_zero() {
    let mut ai = MaskGeometry::select_sky(0.5, 0.25, false, String::new());
    if let MaskGeometry::AiMask { blend_mode, value, .. } = &mut ai { *blend_mode = 1; *value = 0.0; }
    for mode in [MaskCombine::Subtract, MaskCombine::Intersect] {
        let recipe = EditRecipe { masks: vec![LocalAdjustment {
            mask: linear(), inverted: true,
            components: vec![MaskComponent { geometry: ai.clone(), mode, inverted: false }],
            ..Default::default()
        }], ..Default::default() };
        let got = xmp_to_recipe(&recipe_to_xmp(&recipe));
        assert_eq!(got.masks[0].components[0].mode, mode);
        let MaskGeometry::AiMask { blend_mode, value, .. } = got.masks[0].components[0].geometry else { panic!("AI"); };
        assert_eq!((blend_mode, value), (0, 1.0), "De Morgan creates a real Add, not the plain Add zero fallback");
    }
}

#[test]
fn stale_radial_inversion_metadata_cannot_reverse_a_linear_feather() {
    let original = LocalAdjustment { mask: linear(), inverted: true, ..Default::default() };
    let recipe = EditRecipe { masks: vec![original.clone()], ..Default::default() };
    // The native inverted flag is unchanged, but metadata still names the
    // geometry-owned inversion home of a radial that has been replaced.
    let doc = recipe_to_xmp(&recipe)
        .replace("ash:Inverted=\"true\"", "ash:Inverted=\"false\"")
        .replace("ash:BaseInverted=\"false\"", "ash:BaseInverted=\"true\"");
    let got = xmp_to_recipe(&doc);
    assert_eq!(got.masks[0].mask, original.mask);
    assert_eq!(got.masks[0].inverted, original.inverted);
    let image = image::DynamicImage::new_rgb8(127, 73);
    let coverage = |m| crate::render::mask_coverage(m, &image, crate::render::MaskFrame::AsRendered);
    assert_eq!(coverage(&got.masks[0]), coverage(&original));
}

#[test]
fn component_inversion_is_sparse_and_legacy_components_keep_their_bytes() {
    let component = MaskComponent { geometry: linear(), mode: MaskCombine::Intersect, inverted: false };
    let json = serde_json::to_string(&component).unwrap();
    assert!(!json.contains("inverted"));
    assert_eq!(serde_json::from_str::<MaskComponent>(&json).unwrap(), component);
    let inverted = MaskComponent { inverted: true, ..component };
    let json = serde_json::to_string(&inverted).unwrap();
    assert!(json.contains("\"inverted\":true"));
    assert_eq!(serde_json::from_str::<MaskComponent>(&json).unwrap(), inverted);
    assert_eq!(EditRecipe::default().schema_era, crate::recipe::SCHEMA_ERA);
}

#[test]
fn component_radial_rotation_uses_the_base_projection_and_loss_gate() {
    let mut ellipse = radial(false, 24.0);
    if let MaskGeometry::Radial { left, right, .. } = &mut ellipse { *left = 0.15; *right = 0.85; }
    let recipe = EditRecipe { masks: vec![LocalAdjustment {
        mask: linear(), components: vec![MaskComponent {
            geometry: ellipse, mode: MaskCombine::Intersect, inverted: false,
        }], ..Default::default()
    }], ..Default::default() };
    let (_, losses) = recipe_to_xmp_with_losses(&recipe);
    assert_eq!(losses, vec![MaskLoss { name: "AutoShade 1".into(), reason: MaskLossReason::Rotation(24) }]);
    let (doc, losses) = recipe_to_xmp_in_frame(&recipe, FrameAspect::from_size(600.0, 400.0));
    assert!(losses.is_empty());
    let reread = xmp_to_recipe(&doc);
    let MaskGeometry::Radial { angle, .. } = reread.masks[0].components[0].geometry else { panic!("radial"); };
    assert!((angle - 24.0).abs() < 1e-4, "decoded angle {angle}");
}

#[test]
#[ignore = "lane measurement: AUTOSHADE_R35_RECIPE must name a scratch recipe inside this worktree"]
fn r35_export_truth_for_a_scratch_recipe_names_every_remaining_loss() {
    let path = std::path::PathBuf::from(std::env::var("AUTOSHADE_R35_RECIPE").expect("scratch recipe"));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap();
    assert!(path.canonicalize().unwrap().starts_with(&root));
    let recipe: EditRecipe = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let (doc, losses) = recipe_to_xmp_in_frame(&recipe, FrameAspect::from_size(2048.0, 1365.0));
    std::fs::write(path.with_extension("xmp"), &doc).unwrap();
    let reread = xmp_to_recipe(&doc);
    let signature = |m: &LocalAdjustment| {
        (m.name.clone(), m.role, m.inverted,
            m.components.iter().map(|c| (c.mode, c.net_inverted())).collect::<Vec<_>>())
    };
    let expected: Vec<_> = recipe.masks.iter().filter(|m|
        m.enabled && !matches!(m.mask, MaskGeometry::Bitmap { .. })
    ).map(signature).collect();
    let got: Vec<_> = reread.masks.iter().map(signature).collect();
    eprintln!("source spellable mask set: {expected:#?}");
    eprintln!("parsed mask set: {got:#?}");
    eprintln!("remaining named losses: {}", describe_mask_losses(&losses).unwrap_or_default());
    assert_eq!(got, expected, "roles, component order, modes and inversion must survive");
    let expected_bitmaps: Vec<_> = recipe.masks.iter().filter(|m|
        m.enabled && matches!(m.mask, MaskGeometry::Bitmap { .. })
    ).map(|m| m.name.as_str()).collect();
    let named_bitmaps: Vec<_> = losses.iter().filter(|l| l.reason == MaskLossReason::Bitmap)
        .map(|l| l.name.as_str()).collect();
    assert_eq!(named_bitmaps, expected_bitmaps, "every retained raster, and only a raster, has its named loss");
    for mask in recipe.masks.iter().filter(|m| m.name.starts_with("Spatial tile") && !m.components.is_empty()) {
        assert!(!losses.iter().any(|l| l.name == mask.name && matches!(l.reason,
            MaskLossReason::Bitmap | MaskLossReason::ComponentsFlattened | MaskLossReason::Rotation(_)
        )), "a native tile has no geometry export loss; local gains keep their separate disclosure");
    }
    eprintln!("complete mask-set diff: omitted {expected_bitmaps:?}");
    eprintln!("spellable mask-set diff: [] (roles, components, modes and inversion agree)");
}
