// One part of the sidecar's tests (src/xmp/tests.rs includes it): representable globals, import disclosures, the crs namespace scope and its refusals, crop disclosure and pathological documents.

#[test]
fn atmosphere_xmp_contains_only_representable_global_controls() {
    let recipe = EditRecipe {
        exposure_ev: -0.8,
        temperature_k: Some(9000.0),
        tint: 10.0,
        saturation: 30.0,
        tone_curve: vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 64, output: 48 },
            CurvePoint { input: 128, output: 112 },
            CurvePoint { input: 192, output: 208 },
            CurvePoint { input: 255, output: 255 },
        ],
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "atmosphere-sky.png".into() },
            role: crate::recipe::MaskRole::ZoneSky,
            exposure_ev: -0.5,
            color_gains: Some([1.18, 0.96, 0.85]),
            saturation: -20.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let (doc, losses) = recipe_to_xmp_with_losses(&recipe);
    assert!(doc.contains(r#"crs:Exposure2012="-0.80""#));
    assert!(doc.contains(r#"crs:Temperature="9000""#));
    assert!(doc.contains(r#"crs:Tint="+10""#));
    assert!(doc.contains(r#"crs:Saturation="+30""#));
    assert!(doc.contains("crs:ToneCurvePV2012"));
    assert!(!doc.contains("ToneCurvePV2012Red"));
    assert!(!doc.contains("ToneCurvePV2012Green"));
    assert!(!doc.contains("ToneCurvePV2012Blue"));
    assert!(!doc.contains("MaskGroupBasedCorrections"));
    // TWO verdicts about the one mask (v1.3.1): the projection skips it,
    // and the payload could not embed a raster this test never wrote.
    assert_eq!(
        losses.iter().filter(|l| l.reason == MaskLossReason::Bitmap).count(),
        1,
        "a LEGACY bitmap zone — a recipe.json saved before the zones became \
             Select Sky components — stays engine-only"
    );
    assert_eq!(
        losses.iter().filter(|l| l.reason == MaskLossReason::RasterNotEmbedded).count(),
        1,
        "a raster that does not exist cannot ride in the payload: {losses:?}"
    );
    assert_eq!(losses.len(), 2);

    // The PROJECTION (payload-free): the mask is not in the crs settings.
    let projected = xmp_to_recipe(&bare_document(&recipe, None));
    assert_eq!(projected.exposure_ev, -0.8);
    assert_eq!(projected.temperature_k, Some(9000.0));
    assert_eq!(projected.tint, 10.0);
    assert_eq!(projected.saturation, 30.0);
    assert_eq!(projected.tone_curve, recipe.tone_curve);
    assert!(projected.masks.is_empty());
    // The whole document brings the mask back, by name, raster or not.
    assert_eq!(xmp_to_recipe(&doc).masks.len(), 1);
    assert!(projected.red_curve.is_empty());
    assert!(projected.green_curve.is_empty());
    assert!(projected.blue_curve.is_empty());
}

/// R24-5 M0, EXPORT direction: the sidecar cannot carry the camera base
/// curve or the lens-profile correction, and the user has to hear it —
/// silence there is a photo that renders differently in Lightroom for a
/// reason nothing on screen names.
///
/// Derived from the tier registry, so this test also pins the derivation:
/// a neutral recipe discloses nothing, and the disclosed names are exactly
/// the `RenderedNotExported` rows that are actually set.
#[test]
fn the_export_names_the_globals_the_sidecar_cannot_carry() {
    use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
    assert!(
        global_export_losses(&EditRecipe::default()).is_empty(),
        "a neutral recipe loses nothing — a save that lost nothing must not interrupt"
    );
    // The engine's own measurement of THIS photo: rendered, unexportable.
    let with_base = EditRecipe {
        base_curve: vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]],
        ..Default::default()
    };
    assert_eq!(global_export_losses(&with_base), vec!["base_curve"]);
    let both = EditRecipe {
        lens_profile: crate::recipe::LensProfile {
            vignette_on: true,
            vignette: vec![1.0, 0.1],
            ..Default::default()
        },
        ..with_base.clone()
    };
    assert_eq!(global_export_losses(&both), vec!["base_curve", "lens_profile"]);
    // An ordinary rendered control is NOT a loss (it has its own crs key).
    let exposed = EditRecipe { exposure_ev: 1.5, ..Default::default() };
    assert!(global_export_losses(&exposed).is_empty());
    // Premise: the tier this is derived from is populated. A registry
    // where nobody is RenderedNotExported would make every case above
    // pass for the wrong reason.
    assert_eq!(
        RECIPE_CONTROLS
            .iter()
            .filter(|c| c.tier == Some(Tier::RenderedNotExported))
            .map(|c| c.name)
            .collect::<Vec<_>>(),
        // R33 §G added the third, and the first that is an EDIT rather
        // than the engine's own measurement of the photo; v1.5.0 F6 added
        // `upright_transform`, which is ADOBE'S measurement of it — read
        // from `crs:UprightTransform_N`, rendered, and never written back,
        // because writing it would claim Adobe's key for our own solver's
        // numbers. Registry order, not alphabetical.
        // In registry order, which is the panel's draw order: F7's creative
    // Look sits between the Upright matrices and the base curve because
    // that is where its field is declared.
    // v1.5.0 F9 adds `retouch`, and it is the one row here whose export
    // story has two halves. A MERGE keeps the photographer's own
    // `crs:RetouchAreas` verbatim (this writer does not own the element,
    // so it never strips it) — but a FRESH `recipe_to_xmp` emits nothing
    // for it, and on that path every removal is gone. The tier names the
    // worse half, which is what a disclosure is for. 2026-10-01 adds
    // `pixel_layers`, declared right after it: a generative fill's layer,
    // which no sidecar has an element for on either path.
    vec!["upright_transform", "look", "base_curve", "lens_profile", "retouch", "pixel_layers", "colour_field"],
    );
}

/// R24-5 M0, IMPORT direction: a Lightroom sidecar's globals that AutoShade
/// does not model. The merge keeps them; this is the sentence that says
/// they are there — the global counterpart of the mask-side
/// `unsupported_corrections`, which had no partner until now.
#[test]
fn an_imported_sidecar_names_the_globals_the_engine_does_not_render() {
    // FIXTURE NOTE, FOURTH REVISION. `Texture` / `GrainAmount` were the
    // samples until R25 B2 modelled them; `PerspectiveUpright` took over
    // and B4 has now claimed that too. `PointColor` served until v1.5.0
    // modelled Lightroom's point colours (the `crs:PointColors` element).
    // The samples are `CurveRefineSaturation` — the point curve's Refine
    // Saturation, which no AutoShade control holds — and
    // `CameraProfileDigest`, chosen deliberately: it sits one line from
    // `crs:CameraProfile` in every real sidecar, and B4 owns the profile
    // NAME while the digest stays foreign. The list shrinking under the
    // fixtures, batch after batch, IS the complement definition working.
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                   crs:Exposure2012=\"+1.00\" crs:Texture=\"+30\" \
                   crs:CurveRefineSaturation=\"100\" crs:PerspectiveUpright=\"1\" \
                   crs:CameraProfile=\"Adobe Standard\" \
                   crs:CameraProfileDigest=\"2D1D4700365C3E2831EEAE0D1A8F9CDF\" \
                   crs:RawFileName=\"crs:NotAnAttribute=1.ARW\"/></rdf:RDF></x:xmpmeta>";
    let found = unmodelled_global_crs(doc);
    assert!(found.contains(&"CurveRefineSaturation".to_string()), "{found:?}");
    assert!(found.contains(&"CameraProfileDigest".to_string()), "{found:?}");
    // …and the B4 half of the same claim: the Transform block and the
    // profile NAME are ours now, so they left this list with no edit to it.
    assert!(
        !found.contains(&"PerspectiveUpright".to_string()),
        "PerspectiveUpright is passed through since R25 B4: {found:?}"
    );
    assert!(
        !found.contains(&"CameraProfile".to_string()),
        "the profile NAME is passed through; only its digest is foreign: {found:?}"
    );
    // A control we DO model is not "unmodelled" — the universe is the
    // complement of `owned_attr_keys`, which is what stops this list from
    // needing a catalogue of Adobe property names to keep up to date.
    assert!(!found.contains(&"Exposure2012".to_string()), "{found:?}");
    // …and the B2 half of that claim, which is the whole reason the
    // fixture above had to change: teaching the engine `crs:Texture` took
    // the key OFF this list with no edit to the list itself.
    assert!(
        !found.contains(&"Texture".to_string()),
        "Texture is modelled since R25 B2 and must have left this list: {found:?}"
    );
    // Quote-aware: `crs:` inside an attribute VALUE is text, not a
    // property (a RawFileName or a mask name may contain anything).
    assert!(!found.contains(&"NotAnAttribute".to_string()), "{found:?}");

    // Mask corrections live in a CHILD element and have their own
    // disclosure; reporting every crs:Local* key as an unmodelled global
    // would bury the real ones under sixty names.
    let r = EditRecipe {
        masks: vec![crate::recipe::LocalAdjustment {
            mask: crate::recipe::MaskGeometry::Linear {
                zero_x: 0.0,
                zero_y: 0.0,
                full_x: 0.0,
                full_y: 1.0,
            },
            enabled: true,
            amount: 1.0,
            exposure_ev: 0.5,
            ..Default::default()
        }],
        // Element-form children of OUR OWN making: the four tone curves
        // have no attribute spelling, so `owned_attr_keys` cannot exclude
        // them and only `OWNED_ELEMENT_ONLY` keeps the element arm below
        // from naming our own curves as Lightroom-only properties.
        tone_curve: vec![
            crate::recipe::CurvePoint { input: 0, output: 0 },
            crate::recipe::CurvePoint { input: 128, output: 140 },
            crate::recipe::CurvePoint { input: 255, output: 255 },
        ],
        red_curve: vec![
            crate::recipe::CurvePoint { input: 0, output: 0 },
            crate::recipe::CurvePoint { input: 255, output: 250 },
        ],
        ..Default::default()
    };
    let ours = recipe_to_xmp(&r);
    let found = unmodelled_global_crs(&ours);
    assert!(
        found.is_empty(),
        "a sidecar WE wrote models everything in it by construction: {found:?}"
    );
    // Nothing to read ⇒ nothing to say (never a panic, never a warning).
    assert!(unmodelled_global_crs("").is_empty());
    assert!(unmodelled_global_crs("<not xml").is_empty());
}

/// R24 round-end MED-1: the same disclosure for the PROPERTY-ELEMENT
/// spelling. `crs_str` reads that form "for exactly that reason" and the
/// merge strips it, because Lightroom writes it in plenty of real
/// sidecars — but this scanner walked the Description's open TAG only, so
/// an element-form catalog export disclosed nothing at all and the photo
/// just rendered differently from Lightroom with no sentence on screen.
#[test]
fn the_import_disclosure_reads_property_element_globals_too() {
    let head = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                    xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">";
    let tail = "</rdf:RDF></x:xmpmeta>";
    // (a) Pure element form — the shape that returned EMPTY before.
    // (Same fixture note as the test above, third revision: `Texture` is
    // modelled since B2 and `PerspectiveUpright` is passed through since
    // B4, so the unmodelled samples are `PointColor` and
    // `UprightTransform` — the Upright SOLVER's own opaque blob, which is
    // in six of the seven reference sidecars and stays foreign.)
    let element = format!(
        "{head}<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
             <crs:Exposure2012>+1.00</crs:Exposure2012>\
             <crs:Texture>+30</crs:Texture>\
             <crs:PerspectiveUpright>1</crs:PerspectiveUpright>\
             <crs:PointColor>0</crs:PointColor>\
             <crs:UprightTransform>1.0000000</crs:UprightTransform>\
             </rdf:Description>{tail}"
    );
    let found = unmodelled_global_crs(&element);
    assert_eq!(
        found,
        vec!["PointColor", "UprightTransform"],
        "element-form globals must be named exactly once, and never the modelled \
             Exposure2012 / Texture or the passed-through PerspectiveUpright"
    );

    // (b) MIXED: Lightroom splits the same Description across both forms.
    let mixed = format!(
        "{head}<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Exposure2012=\"+1.00\" crs:PointColor=\"0\">\
             <crs:UprightTransform>1.0000000</crs:UprightTransform>\
             </rdf:Description>{tail}"
    );
    assert_eq!(unmodelled_global_crs(&mixed), vec!["PointColor", "UprightTransform"]);

    // (c) The two exclusions the element walk must keep: a mask block's
    // `crs:Local*` items (their own disclosure) and a creative Look's
    // baked parameters (someone else's settings block, nested inside a
    // child) are NOT this Description's globals. `Look` itself IS one.
    let nested = format!(
        "{head}<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
             <crs:PointColor>0</crs:PointColor>\
             <crs:Look><rdf:Description><crs:Parameters><rdf:Description>\
             <crs:LookClarity2012>+50</crs:LookClarity2012>\
             </rdf:Description></crs:Parameters></rdf:Description></crs:Look>\
             <crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li>\
             <rdf:Description crs:LocalExposure2012=\"0.5\" crs:LocalTexture=\"20\"/>\
             </rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>\
             </rdf:Description>{tail}"
    );
    assert_eq!(unmodelled_global_crs(&nested), vec!["Look", "PointColor"]);

    // (d) NIT-1: `-` and `.` are legal XML name characters. No Adobe key
    // uses either today, so this pins the reading rather than a change:
    // the whole name is reported, not the prefix before the hyphen.
    assert_eq!(
        unmodelled_global_crs(
            "<rdf:Description xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                 crs:Foo-Bar=\"1\" crs:Plain=\"2\"/>"
        ),
        vec!["Foo-Bar", "Plain"]
    );
}

/// `Eq ::= S? '=' S?` — XML lets any whitespace (space, tab, CR, LF) sit
/// between an attribute's name and its `=`, and the attribute reader
/// (`next_xml_attribute`) accepts all four. This scan skipped ASCII spaces
/// only, so a foreign key wrapped as `crs:Foo\n="1"` was kept by the merge
/// and missing from the disclosure that says it is there.
///
/// MUTATION THIS CATCHES: `is_ascii_whitespace()` back to `== ' '` and
/// both keys vanish from the list.
#[test]
fn an_unmodelled_key_is_named_whatever_xml_whitespace_precedes_its_equals_sign() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                   crs:Exposure2012 = \"+1.00\" \
                   crs:CurveRefineSaturation\t=\t\"100\"\n\
                   crs:CameraProfileDigest\r\n=\n\"2D1D4700365C3E2831EEAE0D1A8F9CDF\"/>\
                   </rdf:RDF></x:xmpmeta>";
    assert_eq!(
        unmodelled_global_crs(doc),
        vec!["CameraProfileDigest", "CurveRefineSaturation"],
        "tab, CR and LF before `=` spell the same attribute as a space"
    );
}

/// L03-3: the import gate must MATCH the disclosure sentence — a
/// conflicting crs binding imports nothing, because the scanners would
/// read properties through a prefix the document bound elsewhere.
#[test]
fn a_conflicting_crs_binding_imports_nothing() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" xmlns:crs=\"urn:other\" \
                   crs:Exposure2012=\"+2.50\"/></rdf:RDF></x:xmpmeta>";
    assert!(xmlns_conflict(doc).is_some(), "the binding is a conflict");
    let r = xmp_to_recipe(doc);
    assert_eq!(
        r.exposure_ev, 0.0,
        "settings under a conflicting binding must not import — the disclosure says they were not"
    );
    assert!(
        unparsable_crs_numbers(doc)[0].contains("not imported"),
        "and the disclosure names the refusal"
    );
}

/// L03-3's mask half: the import refuses the WHOLE document under a
/// conflicting binding, and the disclosure sentence says its settings
/// were not imported — so the per-correction loss list, the drop count
/// and the carried-globals list have nothing to add. Each used to be read
/// through the very prefix the gate had just declared unreliable, and
/// reported masks skipped for reasons that were never the reason.
///
/// MUTATION THIS CATCHES: drop `xmlns_conflict` from any one of the three
/// gates and its assertion on the conflicting document fails.
#[test]
fn a_conflicting_crs_binding_discloses_no_mask_or_global_losses() {
    let muted = lr_radial("0", "0").replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"");
    let doc = lr_doc(&lr_correction("Radial 1", "", &muted)).replace(
        "crs:Exposure2012=\"+0.35\"",
        "crs:Exposure2012=\"+0.35\"\n   crs:CurveRefineSaturation=\"100\"",
    );
    // Premise: under the canonical binding all three channels speak.
    assert_eq!(unsupported_corrections(&doc), 1);
    assert!(!import_losses(&doc).is_empty());
    assert!(
        unmodelled_global_crs(&doc).contains(&"CurveRefineSaturation".to_string()),
        "{:?}",
        unmodelled_global_crs(&doc)
    );
    let conflict = doc.replace(CRS_URI, "urn:other");
    assert!(xmlns_conflict(&conflict).is_some(), "premise: the binding is a conflict");
    assert!(xmp_to_recipe(&conflict).masks.is_empty(), "premise: nothing imports");
    assert_eq!(unsupported_corrections(&conflict), 0, "no drop count on a refused document");
    assert!(import_losses(&conflict).is_empty(), "{:?}", import_losses(&conflict));
    assert!(
        unmodelled_global_crs(&conflict).is_empty(),
        "{:?}",
        unmodelled_global_crs(&conflict)
    );
    // …and the photo-aware doors share the gate (it answers before any
    // sibling table is looked for, so the path need not exist).
    let photo = std::path::Path::new("synthetic.arw");
    assert_eq!(unsupported_corrections_for_photo(&conflict, photo), 0);
    assert!(import_losses_for_photo(&conflict, photo).is_empty());
}

/// L03-4: the DEFAULT namespace declaration (bare `xmlns=`) bound to the
/// camera-raw or RDF namespace is the same conflict as a foreign prefix —
/// it hides settings in unprefixed spellings the scanners cannot see.
#[test]
fn a_default_namespace_binding_to_crs_is_a_conflict() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <Exposure2012>+1.00</Exposure2012>\
                   </rdf:Description></rdf:RDF></x:xmpmeta>";
    let why = xmlns_conflict(doc).expect("a default-namespace binding to crs must refuse");
    assert!(why.contains("DEFAULT namespace"), "the reason names the binding: {why}");
    assert_eq!(xmp_to_recipe(doc).exposure_ev, 0.0);
}

/// R12-03: bindings resolve in SCOPE — a nested island that rebinds `crs`
/// around content that never says `crs:` is somebody else's metadata, not
/// a reason to throw away the whole document's settings.
#[test]
fn an_unused_nested_rebind_no_longer_refuses_the_document() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                   crs:Exposure2012=\"+0.50\">\
                   <dc:island xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
                   xmlns:crs=\"urn:other\"><dc:note>hi</dc:note></dc:island>\
                   </rdf:Description></rdf:RDF></x:xmpmeta>";
    assert!(xmlns_conflict(doc).is_none(), "an unused rebind is harmless");
    assert_eq!(xmp_to_recipe(doc).exposure_ev, 0.5, "and the settings import");
}

/// R12-03: the rebind still refuses wherever a `crs:` name actually
/// RESOLVES through it — here on a descendant deep inside the island.
#[test]
fn a_rebind_refuses_exactly_where_a_name_resolves_through_it() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <dc:island xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
                   xmlns:crs=\"urn:other\">\
                   <dc:inner crs:Shadows2012=\"+10\"/></dc:island>\
                   </rdf:RDF></x:xmpmeta>";
    let why = xmlns_conflict(doc).expect("a name resolving through the rebind refuses");
    assert!(why.contains("urn:other"), "the reason names the binding: {why}");
}

/// R12-03: a foreign alias for the camera-raw URI is inert while no name
/// resolves through it, and a conflict the moment one does — settings
/// spelled through the alias are invisible to the `crs:` scanners.
#[test]
fn a_foreign_alias_for_the_crs_uri_refuses_only_when_used() {
    let head = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                    xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
                    xmlns:zzz=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                    <rdf:Description rdf:about=\"\"";
    let unused = format!("{head}/></rdf:RDF></x:xmpmeta>");
    assert!(xmlns_conflict(&unused).is_none(), "declared but never used");
    let used = format!("{head} zzz:Exposure2012=\"+1.00\"/></rdf:RDF></x:xmpmeta>");
    let why = xmlns_conflict(&used).expect("a name through the alias refuses");
    assert!(why.contains("`zzz:`"), "the reason names the prefix: {why}");
}

/// R12-03: a scope ends at its element's close tag — the island's rebind
/// must not leak forward onto a following sibling whose `crs:` names
/// resolve through the document-level canonical binding.
#[test]
fn a_closed_scope_releases_its_binding() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <dc:island xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
                   xmlns:crs=\"urn:other\"><dc:note>hi</dc:note></dc:island>\
                   <rdf:Description rdf:about=\"\" crs:Exposure2012=\"+0.50\"/>\
                   </rdf:RDF></x:xmpmeta>";
    assert!(
        xmlns_conflict(doc).is_none(),
        "the sibling's crs resolves through the canonical ancestor binding"
    );
}

/// R12-03 coordination: the scoped gate now clears a Description whose
/// foreign `xmlns:crs` is unused — so the merge's target finder must not
/// key on the attribute NAME alone, or it would splice canonical-intent
/// `crs:` settings into a scope where `crs` means something else.
#[test]
fn the_merge_skips_a_description_whose_crs_binding_is_foreign() {
    let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" xmlns:crs=\"urn:other\"/>\
                   </rdf:RDF></x:xmpmeta>";
    assert!(xmlns_conflict(doc).is_none(), "unused foreign binding is cleared");
    assert_eq!(
        find_crs_description(doc),
        None,
        "and the merge must not adopt that Description as its settings target"
    );
}

/// R12-03: past the scope-tracking bound the gate cannot prove a binding
/// harmless, so it refuses conservatively — never silently accepts.
#[test]
fn deeper_xmlns_nesting_than_tracked_refuses_conservatively() {
    let mut doc = String::new();
    for _ in 0..1025 {
        doc.push_str("<t xmlns:q=\"urn:x\">");
    }
    let why = xmlns_conflict(&doc).expect("beyond the bound is a refusal");
    assert!(why.contains("more xmlns declarations"), "{why}");
}

/// R13-01 (round-13 Codex review): a SURPLUS or MISNAMED close tag must
/// not release a foreign binding early — pops are paired by name, not by
/// arithmetic alone, so malformed nesting degrades toward refusal.
#[test]
fn a_mismatched_close_does_not_release_a_foreign_binding() {
    let doc = "<rdf:Description \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <island xmlns:crs=\"urn:foreign\"></bogus>\
                   <crs:Exposure2012>+2.0</crs:Exposure2012>\
                   </island></rdf:Description>";
    let why = xmlns_conflict(doc)
        .expect("the crs name still resolves through the un-closed island's rebind");
    assert!(why.contains("urn:foreign"), "the reason names the live binding: {why}");
}

/// R13-02 (round-13 Codex review): the tracking bound counts LIVE
/// DECLARATIONS, not frames — a single tag carrying a declaration flood
/// is past what the gate can resolve affordably, so it refuses.
#[test]
fn a_flat_declaration_flood_refuses_conservatively() {
    let mut doc = String::from("<t");
    for i in 0..257 {
        doc.push_str(&format!(" xmlns:q{i}=\"urn:x\""));
    }
    doc.push('>');
    let why = xmlns_conflict(&doc).expect("a declaration flood is a refusal");
    assert!(why.contains("more xmlns declarations"), "{why}");
}

/// L03-7: curve items are matched by tag name — a whitespace-spelled
/// `<rdf:li >` is a real item, not an invisible one that empties the
/// curve (and lets the next save delete it).
#[test]
fn a_whitespace_spelled_curve_item_is_still_a_curve_point() {
    let scope = "<crs:ToneCurvePV2012><rdf:Seq>\
                     <rdf:li >128, 64</rdf:li >\
                     <rdf:li>255, 255</rdf:li>\
                     </rdf:Seq></crs:ToneCurvePV2012>";
    assert_eq!(
        parse_curve_checked(scope, "ToneCurvePV2012"),
        Ok(vec![
            CurvePoint { input: 128, output: 64 },
            CurvePoint { input: 255, output: 255 },
        ]),
        "both spellings are legal XML for the same element"
    );
}

/// L03-9: HasCrop="True" whose coordinates are missing or inverted still
/// imports as no-crop (clamping half a geometry would change coverage),
/// but the drop is DISCLOSED — the next save persists HasCrop="False",
/// and silence made that a deletion nobody asked for.
#[test]
fn an_inconsistent_crop_is_disclosed_not_silently_dropped() {
    let head = "<rdf:Description rdf:about=\"\" \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                    crs:HasCrop=\"True\" crs:CropLeft=\"0.1\" crs:CropTop=\"0.1\" \
                    crs:CropRight=\"0.9\"/>";
    assert!(xmp_to_recipe(head).crop.is_none(), "a missing coordinate cannot crop");
    assert!(
        unparsable_crs_numbers(head).iter().any(|k| k.starts_with("Crop")),
        "the missing coordinate is disclosed: {:?}",
        unparsable_crs_numbers(head)
    );

    let inverted = "<rdf:Description rdf:about=\"\" \
                        xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                        crs:HasCrop=\"True\" crs:CropLeft=\"0.8\" crs:CropTop=\"0.1\" \
                        crs:CropRight=\"0.2\" crs:CropBottom=\"0.9\"/>";
    assert!(xmp_to_recipe(inverted).crop.is_none());
    assert!(
        unparsable_crs_numbers(inverted).iter().any(|k| k.starts_with("Crop")),
        "inverted ordering is disclosed"
    );

    let fine = "<rdf:Description rdf:about=\"\" \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                    crs:HasCrop=\"True\" crs:CropLeft=\"0.1\" crs:CropTop=\"0.1\" \
                    crs:CropRight=\"0.9\" crs:CropBottom=\"0.9\"/>";
    assert!(xmp_to_recipe(fine).crop.is_some());
    assert!(
        unparsable_crs_numbers(fine).is_empty(),
        "a consistent crop discloses nothing"
    );
}

/// `Left > Right` under a non-zero `CropAngle` is a legal Lightroom
/// arrangement (`P3-cropangle-model.md` §6.3;
/// `an_inverted_crop_arrangement_is_read_rather_than_discarded` covers
/// the decoder), and the reader imports it whole. The disclosure restated
/// the reader's ordering rule as `Left < Right && Top < Bottom`
/// unconditionally, so it called this crop "inconsistent" while the
/// recipe carried it — two faces of one document disagreeing.
///
/// MUTATION THIS CATCHES: put the ordering predicate back in place of the
/// `read_crop` verdict and the rotated arrangement is disclosed again.
#[test]
fn a_rotated_left_over_right_crop_the_reader_accepts_is_not_disclosed() {
    let frame = FrameAspect::from_size(9504.0, 6336.0);
    let engine = Crop { left: 0.30, top: 0.10, right: 0.62, bottom: 0.90 };
    let lr = engine_to_lr_crop(Some(&engine), -35.0, frame).expect("corners");
    assert!(lr.left > lr.right, "the fixture must reach the inverted region: {lr:?}");
    let angle = lr_num(lr.angle_deg);
    let doc = in_frame(&lr_doc(""), 9504, 6336).replace(
        "crs:Version=\"15.5.1\"",
        &format!(
            "crs:Version=\"15.5.1\"\n   crs:HasCrop=\"True\"\n   crs:CropLeft=\"{}\"\n   \
                 crs:CropTop=\"{}\"\n   crs:CropRight=\"{}\"\n   crs:CropBottom=\"{}\"\n   \
                 crs:CropAngle=\"{angle}\"",
            lr_num(lr.left),
            lr_num(lr.top),
            lr_num(lr.right),
            lr_num(lr.bottom),
        ),
    );
    let r = xmp_to_recipe(&doc);
    assert!(r.crop.is_some(), "premise: the reader imports the rotated arrangement");
    assert!((r.straighten_deg + 35.0).abs() < 1e-4, "{}", r.straighten_deg);
    assert!(
        unparsable_crs_numbers(&doc).is_empty(),
        "a crop the reader imported whole is not \"inconsistent\": {:?}",
        unparsable_crs_numbers(&doc)
    );
    // …while the SAME corners at θ = 0 are the inverted rectangle they
    // look like: refused by the reader, and disclosed here — the verdict
    // follows the decoder, not the ordering.
    let flat = doc.replace(
        &format!("crs:CropAngle=\"{angle}\""),
        "crs:CropAngle=\"0\"",
    );
    assert!(xmp_to_recipe(&flat).crop.is_none(), "premise: refused at θ = 0");
    assert!(
        unparsable_crs_numbers(&flat).iter().any(|k| k.starts_with("Crop")),
        "{:?}",
        unparsable_crs_numbers(&flat)
    );
}

/// L03-18: raw tab/newline in an attribute value would be folded to
/// spaces by any compliant parser's attribute-value normalization —
/// character references survive it, and our reader decodes them back.
#[test]
fn attribute_control_characters_survive_as_character_references() {
    assert_eq!(xml_attr_escape("a\tb\nc\rd"), "a&#9;b&#10;c&#13;d");
    assert_eq!(xml_unescape("a&#9;b&#10;c&#13;d").as_ref(), "a\tb\nc\rd");
}

/// The merged document alone — most tests assert on the text; the ones
/// about [`MergeOutcome::notes`] call the real function.
fn merged_doc(existing: &str, r: &EditRecipe) -> Option<String> {
    merge_recipe_into_xmp(existing, r).map(|o| o.doc)
}

/// The scope scanner meets a sidecar that is hostile rather than merely
/// unusual. Both halves were real defects: the close search restarted on
/// every nested open (Θ(k²) — this document took MINUTES before, inside
/// SAVE_LOCK and holding a server request permit), and a
/// `</rdf:Description>` inside a COMMENT was read as a real close, which
/// truncated the body and sank the whole merge to a fresh document,
/// dropping the Lightroom-only properties the merge exists to preserve.
#[test]
fn a_pathological_sidecar_neither_hangs_nor_believes_a_comment() {
    // (a) Deep nesting: linear now, quadratic before. 20 000 opens is
    // ~0.4 MB. MEASURED on this box: 0.02 s with the cached close cursor,
    // 13.62 s when the cache is removed — 680x, on a file a user could
    // receive by opening someone else's shoot. The assertion below pins
    // correctness; the wall clock is the pin on the complexity, so keep
    // the size when editing this test.
    let mut doc = String::from(
        r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+0.50">"#,
    );
    let gt = doc.len() - 1;
    for _ in 0..20_000 {
        doc.push_str("<rdf:Description>");
    }
    for _ in 0..20_000 {
        doc.push_str("</rdf:Description>");
    }
    doc.push_str("</rdf:Description>");
    let close = find_matching_close(&doc, gt + 1).expect("the outermost close is found");
    assert_eq!(&doc[close..close + 18], "</rdf:Description>");
    assert_eq!(close, doc.len() - 18, "it is the LAST one, not an inner one");

    // (b) A comment holding the close literal is TEXT, not a close.
    let doc = format!(
        "{}<!-- </rdf:Description> --><crs:Texture>25</crs:Texture></rdf:Description>",
        r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#
    );
    let gt = doc.find('>').unwrap();
    let close = find_matching_close(&doc, gt + 1).expect("the real close is found");
    assert_eq!(close, doc.len() - 18, "the comment's copy is not a close");
    // …and the scope therefore still carries the child that follows it.
    let scope = crs_own_scope(&doc);
    assert!(scope.contains("crs:Texture"), "the body survived the comment: {scope}");

    // (c) CDATA gets the same treatment.
    let doc = format!(
        "{}<![CDATA[ </rdf:Description> ]]><crs:Texture>25</crs:Texture></rdf:Description>",
        r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#
    );
    let gt = doc.find('>').unwrap();
    assert_eq!(find_matching_close(&doc, gt + 1), Some(doc.len() - 18));

    // (d) An UNTERMINATED comment is unaccountable markup: no close at all,
    // so the caller falls back to the whole document rather than guessing.
    let doc = format!(
        "{}<!-- </rdf:Description>",
        r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#
    );
    let gt = doc.find('>').unwrap();
    assert_eq!(find_matching_close(&doc, gt + 1), None);
}

/// The complexity itself, asserted — because the correctness test above
/// passes at ANY speed, which is how the second blowup shipped.
///
/// The scanner has had two separate quadratic shapes. The cached close
/// cursor killed the first (nesting) and the construct skip it shipped
/// alongside introduced the second, on a shape half the size: measured
/// with release-mode replicas of the committed code, 640 KB of
/// back-to-back comments took **8.47 s** and 400 KB of PIs **9.59 s**,
/// against 51 µs and 90 µs for the code that predated the construct skip.
/// Quadratic scaling (4x bytes -> 16x time) put the 16 MiB `read_sidecar`
/// ceiling at roughly an hour and a half — spent inside SAVE_LOCK holding
/// one of the server's eight request permits, reachable by SELECTING a
/// photo that has such a sidecar beside it.
///
/// So both shapes are pinned by wall clock here. The budget is deliberately
/// loose (a debug build on a loaded CI box is not a benchmark); it only has
/// to separate "linear" from "quadratic", and the gap is five orders of
/// magnitude. Keep the SIZES if you edit this test — they are the pin.
#[test]
fn the_scope_scanner_is_linear_on_both_pathological_shapes() {
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
    let head = r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#;

    // Shape 1 — deep nesting (the first blowup). 80 000 opens, ~2.8 MB.
    let mut nested = String::from(head);
    let gt = nested.len() - 1;
    for _ in 0..80_000 {
        nested.push_str("<rdf:Description>");
    }
    for _ in 0..80_000 {
        nested.push_str("</rdf:Description>");
    }
    nested.push_str("</rdf:Description>");

    // Shape 2 — a body of back-to-back comments (the second blowup),
    // and 3 — the same with PIs, which took even longer per byte.
    let commented = format!("{head}{}</rdf:Description>", "<!--x-->".repeat(80_000));
    let pis = format!("{head}{}</rdf:Description>", "<?p?>".repeat(80_000));

    for (name, doc) in [("nested", &nested), ("comments", &commented), ("PIs", &pis)] {
        let started = std::time::Instant::now();
        let close = find_matching_close(doc, gt + 1).expect("the outermost close is found");
        let elapsed = started.elapsed();
        assert_eq!(close, doc.len() - 18, "{name}: it is the LAST close, not an inner one");
        assert!(
            elapsed < BUDGET,
            "{name}: {} bytes scanned in {elapsed:?}, over the {BUDGET:?} budget — \
                 a landmark cursor is being recomputed on every iteration again",
            doc.len()
        );
    }
}

/// A creative profile's baked parameters are the PROFILE's, never the
/// photographer's. Adobe nests them as owned-LOOKING crs children of a
/// second `rdf:Description` (`<crs:Look><rdf:Description><crs:Parameters>
/// <rdf:Description><crs:Clarity2012>…`) — the exact shape the WRITER's
/// depth-aware strip was built for. The reader's flat scan answered from
/// them whenever the top level omitted the key, so opening such a sidecar
/// wrote the profile's look into the user's sliders and the next save
/// persisted it.
#[test]
fn a_nested_look_is_not_a_user_edit() {
    let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:Version="15.5.1"
    crs:Exposure2012="+0.20">
   <crs:Look>
    <rdf:Description crs:Name="Adobe Landscape">
     <crs:Parameters>
      <rdf:Description>
       <crs:Clarity2012>+50</crs:Clarity2012>
       <crs:Vibrance>+35</crs:Vibrance>
       <crs:ToneCurvePV2012>
        <rdf:Seq>
         <rdf:li>0, 30</rdf:li>
         <rdf:li>255, 255</rdf:li>
        </rdf:Seq>
       </crs:ToneCurvePV2012>
      </rdf:Description>
     </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
    let r = xmp_to_recipe(doc);
    assert_eq!(r.exposure_ev, 0.20, "the Description's OWN attribute imports");
    assert_eq!(r.clarity, 0.0, "the Look's Clarity2012 is not a user edit: {}", r.clarity);
    assert_eq!(r.vibrance, 0.0, "the Look's Vibrance is not a user edit: {}", r.vibrance);
    assert!(r.tone_curve.is_empty(), "the Look's baked curve is not a user curve");
    // The disclosure follows the import: a corrupt number the import never
    // reads must not be announced as a setting that will be lost.
    let corrupt = doc.replace("<crs:Clarity2012>+50</crs:Clarity2012>", "<crs:Clarity2012>--</crs:Clarity2012>");
    assert!(
        unparsable_crs_numbers(&corrupt).is_empty(),
        "only settings the import READS may be disclosed: {:?}",
        unparsable_crs_numbers(&corrupt)
    );
    // …and the same key AT top level still imports, in both spellings.
    for own in [
        r#"crs:Exposure2012="+0.20" crs:Clarity2012="+12""#.to_string(),
        r#"crs:Exposure2012="+0.20">
   <crs:Clarity2012>+12</crs:Clarity2012"#
            .to_string(),
    ] {
        let d = doc.replace(r#"crs:Exposure2012="+0.20""#, &own);
        assert_eq!(xmp_to_recipe(&d).clarity, 12.0, "own Clarity2012 must import: {own}");
    }
}

/// The scope keeps what the Description really owns — masks (whose nested
/// Descriptions are its own mask items) and plain property elements — and
/// falls back to the whole document when the markup cannot be accounted
/// for, which is the pre-scope behaviour.
#[test]
fn the_crs_scope_keeps_owned_children_and_degrades_safely() {
    let mut r = EditRecipe {
        exposure_ev: 0.5,
        tone_curve: vec![CurvePoint { input: 0, output: 12 }, CurvePoint { input: 255, output: 250 }],
        ..Default::default()
    };
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.9, full_x: 0.5, full_y: 0.1 },
        name: "sky".into(),
        exposure_ev: -0.4,
        ..Default::default()
    });
    // A full round-trip through the scope: masks and curves are OWNED and
    // must survive it.
    let doc = recipe_to_xmp(&r);
    let back = xmp_to_recipe(&doc);
    assert_eq!(back.tone_curve, r.tone_curve, "the owned tone curve survives the scope");
    assert_eq!(back.masks.len(), 1, "owned mask corrections survive the scope");
    assert_eq!(back.masks[0].name, "sky");
    // Markup the scanner cannot account for (an unclosed element) falls
    // back to the whole document rather than losing every setting.
    let broken = doc.replace("</rdf:Description>", "");
    assert!(crs_scope_inner(&broken).is_none(), "unaccountable markup yields no scope");
    assert_eq!(
        xmp_to_recipe(&broken).exposure_ev,
        0.5,
        "the fallback still reads the document"
    );
}

#[test]
fn straighten_only_activates_crop_and_round_trips_to_no_crop() {
    // Lightroom applies CropAngle only under HasCrop="True" — a
    // straighten-only recipe ships the full frame as its carrier, and the
    // reader collapses that full-frame rectangle back to None.
    let r = EditRecipe { straighten_deg: 2.5, ..Default::default() };
    let x = recipe_to_xmp(&r);
    assert!(x.contains("crs:HasCrop=\"True\""), "straighten must activate the crop state");
    // R27: `crs:CropAngle` is the NEGATION of this engine's clockwise
    // straighten (`P3-cropangle-model.md` §4 — Lightroom turns the content
    // counter-clockwise by +CropAngle, measured on six photographs, 34×
    // margin on the weakest), and it goes out with Lightroom's own six
    // decimals rather than the one this writer used to round to (§6.4).
    assert!(x.contains("crs:CropAngle=\"-2.500000\""), "{x}");
    let back = xmp_to_recipe(&x);
    assert_eq!(back.crop, None, "the full-frame carrier must not become a real crop");
    assert_eq!(back.straighten_deg, 2.5);
    // Control chars in a mask name must not poison the document.
    let dirty = EditRecipe {
        masks: vec![LocalAdjustment { name: "sky\u{0}\u{7}".into(), ..Default::default() }],
        ..Default::default()
    };
    let x = recipe_to_xmp(&dirty);
    assert!(!x.contains('\u{0}') && !x.contains('\u{7}'), "forbidden chars stripped");
}
