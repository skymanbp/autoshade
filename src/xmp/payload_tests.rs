//! The AutoShade payload (v1.3.1): what the sidecar carries under this app's
//! own namespace, what a Lightroom rewrite does to it (measured, 2026-09-12,
//! and replayed here from the measured shapes), and what comes back.

use super::*;
use crate::recipe::{ColourField, MaskComponent, MaskRole};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("autoshade-payload-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn raster_file(dir: &Path, name: &str, seed: u8) -> String {
    let bytes: Vec<u8> = (0..4096u32).map(|i| (i as u8).wrapping_mul(seed).wrapping_add(seed)).collect();
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("raster");
    p.to_string_lossy().into_owned()
}

fn linear(zero_x: f32, full_x: f32) -> MaskGeometry {
    MaskGeometry::Linear { zero_x, zero_y: 0.25, full_x, full_y: 0.75 }
}

fn radial() -> MaskGeometry {
    MaskGeometry::Radial {
        top: 0.25, left: 0.25, bottom: 0.75, right: 0.75, feather: 0.5,
        roundness: 0.0, flipped: false, angle: 0.0, midpoint: 50.0, mask_version: 2,
    }
}

/// A develop with everything the `crs:` projection cannot carry: a colour
/// field, zone roles, an unnamed zone, an Intersect component, a bitmap tile,
/// a muted mask, a non-round exposure, the calibration anchor — and recolour
/// gains, which the projection carries only as the mask's Temp/Tint.
fn rich(dir: &Path) -> EditRecipe {
    let zone = raster_file(dir, "mask-zone-sky.png", 7);
    let tile = raster_file(dir, "tile-r3c3.png", 11);
    let band = |i: u32| LocalAdjustment {
        mask: MaskGeometry::select_sky(0.5, 0.2, false, zone.clone()),
        role: MaskRole::ZoneSky,
        name: format!("sky · band {i}/3"),
        components: vec![MaskComponent {
            geometry: linear(0.1 * i as f32, 0.5 + 0.1 * i as f32),
            mode: MaskCombine::Intersect,
            inverted: false,
        }],
        exposure_ev: -0.25 * i as f32,
        ..Default::default()
    };
    let mut r = EditRecipe {
        exposure_ev: -0.8034,
        contrast: 12.0,
        tint: 3.0,
        temperature_k: Some(5600.0),
        as_shot_k: Some(5200.0),
        colour_field: Some(ColourField {
            x: 2, y: 2, b: 2,
            grid: vec![[0.1, 0.0, 0.05, -0.02, 0.0]; 8],
            amount: 0.9,
            enabled: true,
        }),
        rationale: "why".to_string(),
        confidence: 0.8,
        masks: vec![
            LocalAdjustment {
                mask: MaskGeometry::select_sky(0.5, 0.2, false, zone.clone()),
                role: MaskRole::ZoneSky,
                name: String::new(),
                components: vec![MaskComponent {
                    geometry: linear(0.5, 0.5),
                    mode: MaskCombine::Intersect,
                    inverted: false,
                }],
                exposure_ev: -0.35,
                saturation: -12.0,
                ..Default::default()
            },
            band(2),
            LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: tile.clone() },
                name: "Spatial tile r3c3".to_string(),
                exposure_ev: -0.018,
                ..Default::default()
            },
            LocalAdjustment {
                mask: radial(),
                name: "muted".to_string(),
                enabled: false,
                exposure_ev: 0.5,
                ..Default::default()
            },
            LocalAdjustment {
                mask: linear(0.0, 1.0),
                name: "grad".to_string(),
                amount: 0.7,
                contrast: 20.0,
                ..Default::default()
            },
            LocalAdjustment {
                mask: radial(),
                name: "spot".to_string(),
                exposure_ev: 0.25,
                color_gains: Some([1.18, 0.96, 0.85]),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    r.clamp();
    r
}

/// Every attribute `pred` names, removed from every tag — what Lightroom does
/// to the `ash:` intent (0 of them survive a rewrite, measured).
fn strip_attrs(doc: &str, pred: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut prev = 0;
    let mut at = 0;
    while let Some((start, gt, _)) = next_xml_tag(doc, at) {
        out.push_str(&doc[prev..start]);
        let mut tag = doc[start..=gt].to_string();
        loop {
            let mut cursor = 0;
            let mut hit = None;
            while let Some(a) = next_xml_attribute(&tag, &mut cursor) {
                if pred(a.name) {
                    hit = Some(a.span);
                    break;
                }
            }
            let Some(span) = hit else { break };
            let mut left = span.start;
            while left > 0 && tag.as_bytes()[left - 1].is_ascii_whitespace() {
                left -= 1;
            }
            tag.replace_range(left..span.end, "");
        }
        out.push_str(&tag);
        prev = gt + 1;
        at = gt + 1;
    }
    out.push_str(&doc[prev..]);
    out
}

/// A Lightroom 9.4 rewrite, replayed from the measured shapes: the `crs:`
/// settings re-serialised from Lightroom's own model (the EDITED state), every
/// `ash:` attribute dropped, Camera Raw's own defaults materialised for the keys
/// this writer omits at rest, the toolkit stamp and our comment replaced — and
/// the payload of the ORIGINAL preserved byte for byte, in the compact form.
fn lightroom_like(original: &EditRecipe, edited: &EditRecipe) -> String {
    let doc = bare_document(edited, None);
    let mut doc = strip_attrs(&doc, |name| name == "xmlns:ash" || name.starts_with("ash:"));
    let anchor = "\n    crs:HasSettings=\"True\"";
    assert_eq!(doc.matches(anchor).count(), 1, "{doc}");
    doc = doc.replacen(
        anchor,
        &format!(
            "\n    crs:ColorNoiseReductionDetail=\"50\"\n    crs:ColorNoiseReductionSmoothness=\"50\"{}{anchor}",
            payload::root_attrs(original, "asr")
        ),
        1,
    );
    let close = doc.rfind("</rdf:Description>").expect("the settings Description closes");
    let (rasters, losses) = payload::rasters_element(original, "asr", None);
    assert!(losses.is_empty(), "{losses:?}");
    doc.insert_str(close, &format!("{}\n  ", rasters.trim_start_matches('\n')));
    let (c0, c1) = (doc.find("<!--").expect("our comment"), doc.find("-->").expect("closes") + 3);
    doc.replace_range(c0..c1, "");
    doc.replacen(
        "x:xmptk=\"AutoShade 2\"",
        "x:xmptk=\"Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        \"",
        1,
    )
}

fn collector_diag(collector: &crate::diag::Collector) -> crate::diag::Diag<'_> {
    crate::diag::Diag::about(collector, Path::new("payload-test.ARW"))
}

/// The photo a test LOADS a document for — never created: a read only needs
/// its path, which names the develop dir the bare raster names anchor to.
/// That dir is created, as a real develop's is, because the store anchors a
/// name only inside a folder that exists.
fn load_photo() -> PathBuf {
    let photo = std::env::temp_dir().join(format!("payload-load-{}", std::process::id())).join("roll-0001.ARW");
    std::fs::create_dir_all(crate::store::develop_dir(&photo)).unwrap();
    photo
}

/// Read `doc` the way a real load does: for a photo, whose develop dir
/// anchors the payload's bare raster names (a photo-less read disables them).
fn load(doc: &str) -> EditRecipe {
    xmp_to_recipe_for_photo(doc, &load_photo())
}

/// What [`load`] gives back for a document written from `r`: the payload's
/// bare names, anchored in the load photo's develop dir.
fn loaded(r: &EditRecipe) -> EditRecipe {
    let mut p = payload::portable(r);
    crate::store::resolve_mask_paths(&mut p, &crate::store::develop_dir(&load_photo()));
    p
}

#[test]
fn a_fresh_document_carries_the_whole_develop_and_reads_it_back_exactly() {
    let dir = scratch("fresh");
    let r = rich(&dir);
    let (doc, losses) = recipe_to_xmp_with_losses(&r);
    assert!(doc.contains("\n    xmlns:asr=\"https://autoshade.dev/ns/recipe/1.0/\""), "{doc}");
    assert!(doc.contains("\n    asr:Payload=\"1\""), "{doc}");
    assert!(doc.contains("\n    asr:RecipeCrc32=\""), "{doc}");
    assert!(doc.contains("\n    asr:Recipe=\""), "{doc}");
    assert_eq!(doc.matches("<asr:Rasters>").count(), 1, "{doc}");
    // Two rasters, each ONCE: the zone alpha is shared by the zone and its
    // band; the tile is the bitmap mask's. An AI cache would not be here.
    assert_eq!(doc.matches("<rdf:li asr:Name=").count(), 2, "{doc}");
    assert!(doc.contains("<rdf:li asr:Name=\"mask-zone-sky.png\" asr:Crc32=\""), "{doc}");
    assert!(doc.contains("<rdf:li asr:Name=\"tile-r3c3.png\" asr:Crc32=\""), "{doc}");
    // The projection's own verdicts are unchanged by the payload: the tile
    // and the muted mask are still not Lightroom's to see.
    assert!(losses.iter().any(|l| l.reason == MaskLossReason::Bitmap && l.name == "Spatial tile r3c3"));
    assert!(losses.iter().any(|l| l.reason == MaskLossReason::Disabled && l.name == "muted"));
    assert!(!losses.iter().any(|l| l.reason == MaskLossReason::RasterNotEmbedded), "{losses:?}");
    // The unnamed zone goes out under its role tag, not the numbered placeholder.
    assert!(doc.contains("crs:CorrectionName=\"sky\""), "{doc}");
    assert!(!doc.contains("crs:CorrectionName=\"AutoShade 1\""), "{doc}");
    // The document says itself what Lightroom will not render: the colour
    // field, beside the develop that does.
    assert!(
        doc.contains("
    asr:NotCarried=\"the Lightroom XMP does not carry: colour field"),
        "{doc}"
    );
    // The spot's gains leave as its Temp/Tint, so a foreign reader of the
    // `crs:` settings alone gets a warm spot and no gains …
    let spot = |doc: &str| {
        let r = xmp_to_recipe(&strip_attrs(doc, |a| a == "xmlns:asr" || a.starts_with("asr:")));
        r.masks.into_iter().find(|m| m.name == "spot").expect("the spot")
    };
    let crs_only = spot(&doc);
    assert!(crs_only.temperature > 0.0 && crs_only.color_gains.is_none(), "{crs_only:?}");

    let found = payload::find(&doc).expect("a payload").expect("decodes");
    assert_eq!(found.recipe, payload::portable(&r), "the payload IS the develop, bare names");
    assert_eq!(found.rasters.len(), 2);
    for raster in &found.rasters {
        let on_disk = std::fs::read(dir.join(&raster.name)).unwrap();
        assert_eq!(raster.bytes, on_disk, "{} round-trips byte for byte", raster.name);
    }
    assert!(found.notes.is_empty(), "{:?}", found.notes);

    // Read back through the ordinary door: everything the `crs:` projection
    // lost is back — the colour field, the roles, the tile, the muted mask,
    // the Intersect spelling, the anchor, the non-round exposure.
    let back = load(&doc);
    assert_eq!(back, loaded(&r));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_lightroom_rewrite_keeps_what_it_did_not_touch_and_yields_what_it_did() {
    let dir = scratch("rewrite");
    let original = rich(&dir);
    // What the user did in Lightroom: exposure, one mask's amount, one mask
    // deleted, one mask added. Everything else Lightroom merely re-serialised.
    let mut edited = original.clone();
    edited.exposure_ev = -0.5;
    edited.masks.retain(|m| m.name != "spot");
    edited.masks.iter_mut().find(|m| m.name == "grad").unwrap().amount = 0.4;
    edited.masks.push(LocalAdjustment {
        mask: linear(0.25, 0.75),
        name: "LR added".to_string(),
        exposure_ev: -1.0,
        ..Default::default()
    });
    let doc = lightroom_like(&original, &edited);
    assert!(!doc.contains("ash:"), "the replay drops every intent attribute: {doc}");
    assert!(doc.contains("crs:ColorNoiseReductionDetail=\"50\""), "{doc}");

    let back = load(&doc);
    let want = loaded(&original);
    // Lightroom's edits win…
    assert_eq!(back.exposure_ev, -0.5, "the edited exposure is Lightroom's");
    let names: Vec<&str> = back.masks.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["", "sky · band 2/3", "Spatial tile r3c3", "muted", "grad", "LR added"],
        "payload order for the survivors, the deleted one gone, the added one last"
    );
    let grad = &back.masks[4];
    assert_eq!(grad.amount, 0.4, "the edited amount is Lightroom's");
    assert_eq!(grad.contrast, 20.0, "…and its untouched neighbour is exact");
    assert_eq!(grad.mask, want.masks[4].mask);
    let added = &back.masks[5];
    assert_eq!(added.role, MaskRole::Custom);
    assert_eq!(added.mask, linear(0.25, 0.75));
    assert!((added.exposure_ev + 1.0).abs() < 1e-3, "{}", added.exposure_ev);
    // …and everything Lightroom did not touch is the payload's, exactly.
    for i in 0..4 {
        assert_eq!(back.masks[i], want.masks[i], "mask {i} ({:?}) is restored exactly", want.masks[i].name);
    }
    assert_eq!(back.masks[0].role, MaskRole::ZoneSky, "role survives without intent");
    assert_eq!(back.masks[0].components[0].mode, MaskCombine::Intersect, "the exact spelling, not Subtract(¬g)");
    assert_eq!(back.colour_field, want.colour_field);
    assert_eq!(back.contrast, 12.0);
    assert_eq!(back.tint, 3.0);
    assert_eq!(back.temperature_k, Some(5600.0));
    assert_eq!(back.as_shot_k, Some(5200.0), "a leaf the projection never carries");
    assert_eq!(back.rationale, "why", "provenance is the payload's even with the comment gone");
    assert_eq!(back.confidence, 0.8);
    assert_eq!(back.color_nr_detail, 0.0, "Camera Raw's materialised 50 is not an edit");
    assert_eq!(back.color_nr_smooth, 0.0);
    assert_eq!(back.color_nr, 0.0, "the zero we wrote came back as the zero we wrote");
    let _ = std::fs::remove_dir_all(&dir);
}

/// v1.5.0: a companion's EXPLICIT zero (`EditRecipe::explicit_zero`) through
/// a Lightroom rewrite. Untouched, the zero Lightroom re-serialises is the one
/// we wrote, so the payload's list stands; edited away from it in Lightroom,
/// Lightroom's number wins and the zero's mark leaves with the zero — the
/// list is reconciled as one leaf, never kept beside a value it no longer
/// describes.
/// A mask's gains leave as its Temp, Tint and Exposure; a Lightroom edit of
/// that colour is the colour from then on, and the payload's gains do not
/// join it (they are already in it).
///
/// MUTATION: drop `recolour_carrier` from `reconcile_mask`, and the edited
/// Temp comes back beside the payload's gains.
#[test]
fn an_edited_recolour_carrier_is_lightrooms_and_the_gains_go() {
    let dir = scratch("carrier");
    let original = rich(&dir);
    let mut edited = original.clone();
    let spot = edited.masks.iter_mut().find(|m| m.name == "spot").unwrap();
    spot.color_gains = None;
    spot.temperature = 50.0;
    let back = load(&lightroom_like(&original, &edited));
    let spot = back.masks.iter().find(|m| m.name == "spot").expect("the spot");
    assert_eq!(spot.color_gains, None, "the gains are in the edited colour: {spot:?}");
    assert!((spot.temperature - 50.0).abs() < 1e-3, "Lightroom's Temp: {}", spot.temperature);
    assert!((spot.exposure_ev - 0.25).abs() < 1e-3, "…and its Exposure: {}", spot.exposure_ev);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_explicit_companion_zero_survives_a_rewrite_and_yields_to_an_edit() {
    let mut original = EditRecipe { sharpening: 40.0, ..Default::default() };
    original.set_resolved("sharpen_detail", 0.0);
    let doc = recipe_to_xmp(&original);
    assert!(doc.contains("crs:SharpenDetail=\"0\""), "the real zero goes out as one: {doc}");

    let back = xmp_to_recipe(&lightroom_like(&original, &original));
    assert_eq!(back.explicit_zero, vec!["sharpen_detail".to_string()]);
    assert_eq!(back.resolved("sharpen_detail"), 0.0);

    let mut edited = original.clone();
    edited.set_resolved("sharpen_detail", 40.0);
    let back = xmp_to_recipe(&lightroom_like(&original, &edited));
    assert!(back.explicit_zero.is_empty(), "{:?}", back.explicit_zero);
    assert_eq!(back.resolved("sharpen_detail"), 40.0, "Lightroom's Detail wins");
}

#[test]
fn rasters_are_placed_beside_the_develop_on_a_disclosing_read_and_never_on_a_silent_one() {
    let dir = scratch("place-src");
    let dev = scratch("place-dev");
    let r = rich(&dir);
    let doc = recipe_to_xmp(&r);
    let crs_only = xmp_to_recipe(&bare_document(&r, None));

    // Silent: nothing written, bare names anchored to the dir it was given.
    let p = payload::find(&doc).unwrap().unwrap();
    let silent = payload::restore(crs_only.clone(), p, None, None, Some(&dev), false, None);
    assert!(!dev.join("mask-zone-sky.png").exists(), "a silent read never writes");
    let MaskGeometry::AiMask { raster: Some(path), .. } = &silent.masks[0].mask else {
        panic!("zone mask geometry");
    };
    assert_eq!(Path::new(path), dev.join("mask-zone-sky.png"), "anchored, not written");

    // Disclosing: written, byte for byte; a second read finds them and is quiet.
    let collector = crate::diag::Collector::new();
    let diag = collector_diag(&collector);
    let p = payload::find(&doc).unwrap().unwrap();
    let placed = payload::restore(crs_only.clone(), p, None, None, Some(&dev), true, Some(&diag));
    for name in ["mask-zone-sky.png", "tile-r3c3.png"] {
        assert_eq!(std::fs::read(dev.join(name)).unwrap(), std::fs::read(dir.join(name)).unwrap(), "{name}");
    }
    let MaskGeometry::Bitmap { path } = &placed.masks[2].mask else { panic!("tile geometry") };
    assert_eq!(Path::new(path), dev.join("tile-r3c3.png"));
    assert!(collector.take().is_empty(), "a clean placement says nothing");
    let p = payload::find(&doc).unwrap().unwrap();
    let _ = payload::restore(crs_only.clone(), p, None, None, Some(&dev), true, Some(&diag));
    assert!(collector.take().is_empty(), "an identical file already there is kept in silence");

    // A DIFFERENT file under the name: the sidecar's copy takes a fresh name,
    // the recipe follows it, and the line says so.
    std::fs::write(dev.join("tile-r3c3.png"), b"someone else's tile").unwrap();
    let p = payload::find(&doc).unwrap().unwrap();
    let moved = payload::restore(crs_only, p, None, None, Some(&dev), true, Some(&diag));
    let MaskGeometry::Bitmap { path } = &moved.masks[2].mask else { panic!("tile geometry") };
    assert_eq!(Path::new(path), dev.join("tile-r3c3-2.png"), "the reference follows the placement");
    assert_eq!(std::fs::read(dev.join("tile-r3c3-2.png")).unwrap(), std::fs::read(dir.join("tile-r3c3.png")).unwrap());
    assert_eq!(std::fs::read(dev.join("tile-r3c3.png")).unwrap(), b"someone else's tile", "the other file is untouched");
    let lines = collector.take();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].text.contains("tile-r3c3-2.png"), "{}", lines[0].text);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// `doc` with its payload recipe swapped for `r`'s JSON exactly as written —
/// raster paths included, which the writer itself would have made bare. How a
/// crafted or foreign sidecar puts a relative or share path in front of the
/// reader.
fn with_payload_recipe(doc: &str, r: &EditRecipe) -> String {
    use base64::Engine as _;
    use std::io::Write as _;
    let json = serde_json::to_vec(r).unwrap();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&json).unwrap();
    let packed = base64::engine::general_purpose::STANDARD.encode(z.finish().unwrap());
    let recipe = payload::simple_property(doc, "asr", "Recipe").unwrap();
    let crc = payload::simple_property(doc, "asr", "RecipeCrc32").unwrap();
    let out = doc
        .replacen(&format!("asr:Recipe=\"{recipe}\""), &format!("asr:Recipe=\"{packed}\""), 1)
        .replacen(
            &format!("asr:RecipeCrc32=\"{crc}\""),
            &format!("asr:RecipeCrc32=\"{:08x}\"", crc32fast::hash(&json)),
            1,
        );
    assert_ne!(out, doc, "the swap landed");
    out
}

/// A reading with NO photo has no develop dir, so nothing can say which
/// folder a relative or bare raster name means — and a later save would anchor
/// it to whichever develop the recipe lands in, where a different photo's
/// file may carry the name. Silent or disclosing, such a reading never hands
/// one out: every relative, bare and share path is disabled.
#[test]
fn a_read_without_a_photo_never_keeps_an_unanchored_raster_path() {
    let bitmap = |path: &str| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: path.into() },
        exposure_ev: 0.5,
        ..Default::default()
    };
    let r = EditRecipe {
        exposure_ev: 0.3,
        masks: vec![bitmap("mask-bare.png"), bitmap("sub/rel.png"), bitmap("//host/share/m.png")],
        lens_blur: Some(crate::render::LensBlur {
            depth_map: "../depth.png".into(),
            protect: Some(r"\\host\share\p.png".into()),
            ..Default::default()
        }),
        pixel_layers: vec![crate::render::PixelLayer { path: "layer.png".into(), ..Default::default() }],
        ..Default::default()
    };
    let doc = with_payload_recipe(&recipe_to_xmp(&r), &r);
    let found = payload::find(&doc).unwrap().unwrap();
    assert_eq!(found.recipe.clone().raster_paths_mut().len(), 6, "premise: the payload carries all six as written");
    let sentinel = crate::store::store_root().join(".invalid-mask-reference");
    let check = |label: &str, mut back: EditRecipe| {
        let paths: Vec<String> = back.raster_paths_mut().into_iter().map(|p| p.clone()).collect();
        assert_eq!(paths.len(), 6, "{label}: every reference survives as a reference: {paths:?}");
        for p in &paths {
            assert_eq!(Path::new(p), sentinel.as_path(), "{label}: an unanchored raster path came out: {p}");
        }
    };
    check("silent", xmp_to_recipe(&doc));
    check("silent, clamped", xmp_to_recipe_clamped(&doc).0);
    let collector = crate::diag::Collector::new();
    let diag = crate::diag::Diag::pixels_only(&collector);
    check("disclosing", xmp_to_recipe_with_diag(&doc, &diag));
    let lines = collector.take();
    assert!(lines.iter().any(|l| l.text.contains("6 raster reference")), "{lines:?}");
}

/// The merge's mask-preserve check re-reads its base the way the develop in
/// hand was read. With no photo both sides are read photo-less and disabled
/// alike, so an untouched import still compares equal and the base's mask
/// block is kept — no "not in the new file" note.
#[test]
fn a_photo_less_merge_still_preserves_an_untouched_import_with_a_bare_raster() {
    let mut r = EditRecipe { exposure_ev: 0.3, ..Default::default() };
    r.masks.push(LocalAdjustment {
        mask: radial(),
        exposure_ev: 0.4,
        ..Default::default()
    });
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: "subject.png".into() },
        exposure_ev: 0.6,
        ..Default::default()
    });
    let base = recipe_to_xmp(&r);
    let develop = xmp_to_recipe(&base);
    assert_eq!(develop.masks.len(), 2, "premise: the bitmap mask comes back through the payload");
    let merged = merge_recipe_into_xmp(&base, &develop).expect("our own document merges");
    assert!(
        !merged.notes.iter().any(|n| n.contains("merge base's mask block")),
        "an untouched import keeps the base's block: {:?}",
        merged.notes
    );
}

#[test]
fn a_payload_this_build_cannot_trust_is_disclosed_and_the_crs_reading_stands() {
    let dir = scratch("corrupt");
    let r = rich(&dir);
    let doc = recipe_to_xmp(&r);
    let crs_only = xmp_to_recipe(&bare_document(&r, None));
    assert_ne!(crs_only, payload::portable(&r), "premise: the crs reading is the approximation");
    let crc = payload::simple_property(&doc, "asr", "RecipeCrc32").unwrap();
    for (label, tampered, needle) in [
        ("checksum", doc.replacen(&format!("asr:RecipeCrc32=\"{crc}\""), "asr:RecipeCrc32=\"00000000\"", 1), "checksum"),
        ("format", doc.replacen("asr:Payload=\"1\"", "asr:Payload=\"2\"", 1), "format"),
        ("base64", doc.replacen("asr:Recipe=\"", "asr:Recipe=\"!!!", 1), "base64"),
    ] {
        assert_ne!(tampered, doc, "{label}: the tamper landed");
        let collector = crate::diag::Collector::new();
        let diag = collector_diag(&collector);
        let back = xmp_to_recipe_with_diag(&tampered, &diag);
        assert_eq!(back, crs_only, "{label}: the crs reading, nothing invented");
        let lines = collector.take();
        assert!(
            lines.iter().any(|l| l.text.contains("could not read") && l.text.contains(needle)),
            "{label}: {lines:?}"
        );
        // The silent door says the same thing, silently.
        assert_eq!(xmp_to_recipe(&tampered), crs_only, "{label}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_merge_replaces_the_previous_payload_and_steps_around_a_foreign_prefix() {
    let dir = scratch("merge");
    let r1 = rich(&dir);
    let mut r2 = r1.clone();
    r2.exposure_ev = 0.3;
    r2.masks.truncate(3); // zone, band, tile — the rasters stay
    let base = recipe_to_xmp(&r1);
    let merged = merge_recipe_into_xmp(&base, &r2).expect("our own document merges").doc;
    assert_eq!(merged.matches("xmlns:asr=").count(), 1, "{merged}");
    assert_eq!(merged.matches("asr:Recipe=").count(), 1, "{merged}");
    assert_eq!(merged.matches("asr:RecipeCrc32=").count(), 1, "{merged}");
    assert_eq!(merged.matches("<asr:Rasters>").count(), 1, "the base's element is gone: {merged}");
    assert_eq!(load(&merged), loaded(&r2), "the merged file carries the NEW develop");

    // A base that binds `asr` to somebody else's URI keeps it; ours goes out
    // under the next free prefix and the reader finds it by URI, not by name.
    let foreign = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                   <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                   <rdf:Description rdf:about=\"\"\n    \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                   xmlns:asr=\"urn:somebody-else\"\n    \
                   asr:Thing=\"kept\" crs:Exposure2012=\"+0.10\" crs:HasSettings=\"True\">\n  \
                   </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
    let merged = merge_recipe_into_xmp(foreign, &r2).expect("mergeable").doc;
    assert!(merged.contains("xmlns:asr=\"urn:somebody-else\""), "{merged}");
    assert!(merged.contains("asr:Thing=\"kept\""), "{merged}");
    assert!(merged.contains("xmlns:asr1=\"https://autoshade.dev/ns/recipe/1.0/\""), "{merged}");
    assert!(merged.contains("asr1:Recipe=\""), "{merged}");
    assert!(merged.contains("<asr1:Rasters>"), "{merged}");
    assert_eq!(payload::bound_prefix(&merged).as_deref(), Some("asr1"));
    assert_eq!(load(&merged), loaded(&r2));
    // …and merging AGAIN over that file replaces the asr1 payload, not the foreign asr.
    let again = merge_recipe_into_xmp(&merged, &r1).expect("mergeable").doc;
    assert_eq!(again.matches("asr1:Recipe=").count(), 1, "{again}");
    assert_eq!(again.matches("<asr1:Rasters>").count(), 1, "{again}");
    assert!(again.contains("asr:Thing=\"kept\""), "{again}");
    assert_eq!(load(&again), loaded(&r1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_raster_over_the_budget_is_left_out_and_named() {
    let dir = scratch("budget");
    let mut r = rich(&dir);
    let huge = dir.join("huge.png");
    std::fs::write(&huge, vec![0x42u8; payload::RASTER_BUDGET + 1]).unwrap();
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: huge.to_string_lossy().into_owned() },
        name: "huge".to_string(),
        exposure_ev: 0.1,
        ..Default::default()
    });
    let (doc, losses) = recipe_to_xmp_with_losses(&r);
    assert!(
        losses.iter().any(|l| l.reason == MaskLossReason::RasterNotEmbedded && l.name == "huge"),
        "{losses:?}"
    );
    assert_eq!(doc.matches("<rdf:li asr:Name=").count(), 2, "the two small rasters still ride: {doc}");
    assert!(!doc.contains("asr:Name=\"huge.png\""), "{}", doc.len());
    assert!(doc.len() < 2 * 1024 * 1024, "the document stayed small: {}", doc.len());
    // The recipe itself is whole: the mask is there, naming a raster the
    // store may or may not have.
    let back = load(&doc);
    assert_eq!(back.masks.len(), 7);
    let huge_there = crate::store::develop_dir(&load_photo()).join("huge.png").to_string_lossy().into_owned();
    assert_eq!(back.masks[6].mask, MaskGeometry::Bitmap { path: huge_there });
    assert!(describe_mask_losses(&losses).unwrap().contains("not embedded"), "{losses:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_zone_role_comes_back_from_the_name_when_no_intent_survives() {
    let dir = scratch("role");
    let mut r = rich(&dir);
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::select_sky(0.4, 0.3, false, String::new()),
        name: "Sky 1".to_string(),
        ..Default::default()
    });
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::select_sky(0.4, 0.3, false, String::new()),
        name: "sky".to_string(),
        role: MaskRole::Custom,
        ..Default::default()
    });
    // A v1.3.0-shaped document after Lightroom: no payload, no intent.
    let doc = strip_attrs(&bare_document(&r, None), |n| n == "xmlns:ash" || n.starts_with("ash:"));
    assert!(payload::find(&doc).is_none(), "premise: nothing to restore from");
    let back = xmp_to_recipe(&doc);
    let by_name = |n: &str| {
        back.masks.iter().find(|m| m.name == n).unwrap_or_else(|| {
            panic!("{n}: {:?}", back.masks.iter().map(|m| &m.name).collect::<Vec<_>>())
        })
    };
    assert_eq!(by_name("sky · band 2/3").role, MaskRole::ZoneSky, "the band label says which zone");
    assert_eq!(by_name("grad").role, MaskRole::Custom);
    assert_eq!(by_name("Sky 1").role, MaskRole::Custom, "Lightroom's own Select Sky name is not a zone");
    // The unnamed zone went out as `sky` and comes back unnamed, a zone; the
    // user-named `sky` Select Sky is indistinguishable from it without intent
    // and is read the same way (the payload, when present, restores its name).
    let unnamed: Vec<&LocalAdjustment> = back.masks.iter().filter(|m| m.name.is_empty()).collect();
    assert_eq!(unnamed.len(), 2, "{:?}", back.masks.iter().map(|m| &m.name).collect::<Vec<_>>());
    assert!(unnamed.iter().all(|m| m.role == MaskRole::ZoneSky));
    // With intent present the intent rules, even against the name: the four
    // written Custom masks (grad, spot, `Sky 1`, the user's `sky`) stay Custom.
    let with_intent = xmp_to_recipe(&bare_document(&r, None));
    assert_eq!(with_intent.masks.iter().filter(|m| m.role == MaskRole::Custom).count(), 4);
    assert_eq!(with_intent.masks.iter().filter(|m| m.role == MaskRole::ZoneSky).count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The two REAL Lightroom 9.4 round trips (2026-09-12), verbatim: the
/// preservation probe (the payload forms, written in element form, back in
/// Lightroom's compact attribute form) and the v1.3.0 band sidecar (no
/// payload, intent stripped — the name fallback's real case). Skipped with a
/// line when the fixture root is not named, the calibration-corpus convention.
#[test]
fn the_real_lightroom_rewrites_read_as_measured() {
    let Some(root) = crate::config::live_env_os("AUTOSHADE_LR_PAYLOAD_FIXTURES") else {
        println!("skipped: AUTOSHADE_LR_PAYLOAD_FIXTURES is unset (the lr-payload-2026-09 fixtures)");
        return;
    };
    let root = PathBuf::from(root);
    let read = |name: &str| {
        std::fs::read_to_string(root.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    let (orig, lr) = (read("probe-original.xmp"), read("probe-lightroom-9.4-rewrite.xmp"));
    assert_ne!(orig, lr, "premise: Lightroom rewrote the probe");
    assert_eq!(payload::bound_prefix(&orig).as_deref(), Some("asr"));
    assert_eq!(payload::bound_prefix(&lr).as_deref(), Some("asr"), "the binding survives, hoisted");
    // The 15 KB recipe, element form → attribute form, byte for byte.
    let (a, b) = (
        payload::simple_property(&orig, "asr", "Recipe").expect("probe recipe"),
        payload::simple_property(&lr, "asr", "Recipe").expect("rewritten recipe"),
    );
    assert!(a.len() > 8_000, "{}", a.len());
    assert_eq!(a, b, "the recipe payload survived the rewrite byte for byte");
    assert_eq!(
        payload::simple_property(&orig, "asr", "RecipeSha256"),
        payload::simple_property(&lr, "asr", "RecipeSha256")
    );
    // The five rasters (~250 KB), a Seq of structs, both forms.
    let (ra, rb) = (
        payload::raster_entries(&orig, "asr").expect("probe rasters"),
        payload::raster_entries(&lr, "asr").expect("rewritten rasters"),
    );
    assert_eq!(ra.len(), 5);
    assert_eq!(ra, rb, "every raster struct survived, field for field, in order");
    assert!(ra.iter().all(|f| f.iter().map(|(k, _)| k.as_str()).eq(["Name", "Sha256", "Data"])), "{:?}", ra[0].iter().map(|(k, _)| k).collect::<Vec<_>>());
    assert!(rb.iter().all(|f| f.iter().any(|(k, v)| k == "Data" && v.len() > 1000)));
    // The probe is not the v1.3.1 format (no `asr:Payload`, SHA-256 not CRC-32),
    // and is refused as such rather than misread.
    match payload::find(&lr) {
        Some(Err(why)) => assert!(why.contains("format"), "{why}"),
        Some(Ok(_)) => panic!("a probe-format payload must be refused, not decoded"),
        None => panic!("the probe's payload must be found"),
    }
    // The v1.3.0 band sidecar: intent gone after Lightroom, roles back from the names.
    let (orig, lr) = (read("band-sidecar-original.xmp"), read("band-sidecar-lightroom-9.4-rewrite.xmp"));
    assert!(orig.contains("ash:Role=\"sky\""), "premise: the original carried intent");
    assert!(!lr.contains("ash:"), "premise: Lightroom stripped every intent attribute");
    assert!(payload::find(&lr).is_none(), "premise: a v1.3.0 sidecar has no payload");
    let (before, after) = (xmp_to_recipe(&orig), xmp_to_recipe(&lr));
    assert_eq!(before.masks.len(), 2);
    assert_eq!(after.masks.len(), 2);
    for (b, a) in before.masks.iter().zip(&after.masks) {
        assert_eq!(b.role, MaskRole::ZoneSky, "{}", b.name);
        assert_eq!(a.role, MaskRole::ZoneSky, "{}: the name gives the role back", a.name);
        assert_eq!(a.name, b.name);
        assert!(a.name.starts_with("sky · band "), "{}", a.name);
    }
    assert_eq!(after.color_nr, 25.0, "with no payload, Lightroom's materialised 25 is what the file says");
    assert_eq!(before.color_nr, 0.0, "the v1.3.0 writer omitted the key");

    // The third pair (2026-09-12, after the release): a v1.3.1 sidecar ITSELF —
    // the 0.85 reference-pair develop, 285,172 bytes with the payload and five
    // rasters — rewritten by Lightroom 9.4 after one mask toggle. The whole
    // develop comes back; what differs is exactly what Lightroom wrote: its own
    // Select Sky reference point and provenance on the two zone masks, and the
    // nine unmodelled keys it materialised (kept as passthrough).
    let (orig, lr) = (read("payload-131-original.xmp"), read("payload-131-lightroom-9.4-rewrite.xmp"));
    assert_ne!(orig, lr, "premise: Lightroom rewrote the v1.3.1 sidecar");
    assert!(orig.contains("asr:Writer=\"AutoShade 1.3.1\""), "premise: written by v1.3.1");
    assert!(!lr.contains(" ash:"), "the intent is stripped, as before");
    assert_eq!(
        payload::simple_property(&orig, "asr", "Recipe"),
        payload::simple_property(&lr, "asr", "Recipe"),
        "the recipe attribute survived byte for byte"
    );
    let (po, pl) = (
        payload::find(&orig).expect("payload").expect("decodes"),
        payload::find(&lr).expect("payload after Lightroom").expect("decodes after Lightroom"),
    );
    assert!(pl.notes.is_empty(), "{:?}", pl.notes);
    assert_eq!(pl.rasters.len(), 5);
    assert!(
        po.rasters.iter().zip(&pl.rasters).all(|(a, b)| a.name == b.name && a.bytes == b.bytes),
        "every raster survived byte for byte, in order"
    );
    let want = po.recipe;
    let back = xmp_to_recipe(&lr);
    let mut diffs = Vec::new();
    leaf_diffs("", &serde_json::to_value(&want).unwrap(), &serde_json::to_value(&back).unwrap(), &mut diffs);
    diffs.sort();
    let lightroom_wrote = [
        "/masks[0]/mask/provenance", "/masks[0]/mask/ref_x", "/masks[0]/mask/ref_y",
        "/masks[1]/mask/provenance", "/masks[1]/mask/ref_x", "/masks[1]/mask/ref_y",
        "/passthrough/CameraProfile", "/passthrough/PerspectiveAspect",
        "/passthrough/PerspectiveHorizontal", "/passthrough/PerspectiveRotate",
        "/passthrough/PerspectiveScale", "/passthrough/PerspectiveUpright",
        "/passthrough/PerspectiveVertical", "/passthrough/PerspectiveX", "/passthrough/PerspectiveY",
    ];
    assert_eq!(diffs, lightroom_wrote, "only what Lightroom wrote differs from the develop");
    assert_eq!(back.colour_field, want.colour_field, "the colour field is back");
    assert_eq!(back.masks.len(), 6);
    for (b, w) in back.masks.iter().zip(&want.masks) {
        assert_eq!((&b.name, b.role, b.enabled, b.inverted), (&w.name, w.role, w.enabled, w.inverted));
        if matches!(w.mask, MaskGeometry::Bitmap { .. }) {
            assert_eq!(b.mask, w.mask, "{}: the tile is back, bare name and all", w.name);
        }
    }
    assert_eq!(back.masks.iter().filter(|m| m.role == MaskRole::ZoneSky).count(), 2);
    assert_eq!((back.exposure_ev, back.temperature_k, back.as_shot_k), (want.exposure_ev, want.temperature_k, want.as_shot_k));
    assert_eq!(back.color_nr, 0.0, "written at zero, and Lightroom wrote the zero back");
}

/// Every leaf where `a` and `b` differ, as JSON-pointer-like paths (absent = differs).
fn leaf_diffs(path: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&str> = x.keys().chain(y.keys()).map(String::as_str).collect();
            for k in keys {
                match (x.get(k), y.get(k)) {
                    (Some(u), Some(v)) => leaf_diffs(&format!("{path}/{k}"), u, v, out),
                    _ => out.push(format!("{path}/{k}")),
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                leaf_diffs(&format!("{path}[{i}]"), u, v, out);
            }
        }
        _ => {
            if a != b {
                out.push(path.to_string());
            }
        }
    }
}

/// The inversion is one bit in two homes (the correction's `inverted`, the
/// geometry's own), and only their XOR reaches the sidecar. Lightroom's
/// rewrite drops the intent that says which home held it; the payload puts
/// it back — unless Lightroom changed the net, in which case Lightroom's net
/// is what renders, in Lightroom's own home.
#[test]
fn an_inversion_keeps_its_authored_home_through_a_rewrite_and_follows_a_lightroom_flip() {
    let dir = scratch("inversion");
    let zone = raster_file(&dir, "mask-zone-sky.png", 7);
    let mut original = EditRecipe {
        masks: vec![
            LocalAdjustment {
                mask: MaskGeometry::select_sky(0.5, 0.2, false, zone),
                role: MaskRole::ZoneLand,
                inverted: true, // the home is the CORRECTION; the component is upright
                exposure_ev: -0.5,
                ..Default::default()
            },
            LocalAdjustment {
                mask: radial(),
                name: "spot".to_string(),
                inverted: true, // likewise; a radial's own bit is `flipped`
                exposure_ev: 0.25,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    original.clamp();
    let want = loaded(&original);
    // Untouched: the intent is gone, the net is the net that was written, and
    // the payload's home comes back — not the native fallback's.
    let back = load(&lightroom_like(&original, &original));
    assert_eq!(back.masks.len(), 2);
    for (b, w) in back.masks.iter().zip(&want.masks) {
        assert_eq!((b.inverted, b.mask.own_inverted()), (true, false), "{:?}: the authored home", w.name);
        assert_eq!(b, w, "{:?}: exact", w.name);
    }
    // Flipped in Lightroom: the net is the edit, and the edit wins.
    let mut flipped = original.clone();
    for m in &mut flipped.masks {
        m.inverted = false;
    }
    let back = xmp_to_recipe(&lightroom_like(&original, &flipped));
    for (b, w) in back.masks.iter().zip(&want.masks) {
        assert!(!b.net_inverted(), "{:?}: Lightroom's net", w.name);
        assert_eq!(b.exposure_ev, w.exposure_ev, "{:?}: the rest is still the payload's", w.name);
    }
    // …and a flip the OTHER way, from an upright original: Lightroom's net, in
    // the home the native reading gives it (the AI mask's own bit; a radial's
    // correction flag), since the authored home no longer describes the file.
    let mut upright = original.clone();
    for m in &mut upright.masks {
        m.inverted = false;
    }
    let back = xmp_to_recipe(&lightroom_like(&upright, &original));
    assert!(back.masks.iter().all(LocalAdjustment::net_inverted), "{:?}", back.masks);
    assert_eq!((back.masks[0].inverted, back.masks[0].mask.own_inverted()), (false, true));
    assert_eq!((back.masks[1].inverted, back.masks[1].mask.own_inverted()), (true, false));
    let _ = std::fs::remove_dir_all(&dir);
}

/// F4 (2026-09-24 audit). A raster two masks share is embedded once — right —
/// and, when it could NOT be embedded, was named as lost once: the second mask
/// stepped over the `seen` name and was told nothing. The losses say which
/// masks render inert, so every mask that names the raster is in them.
#[test]
fn a_shared_raster_that_cannot_be_embedded_is_a_loss_for_every_mask_that_names_it() {
    let dir = scratch("shared-loss");
    let missing = dir.join("mask-zone-sky.png").to_string_lossy().into_owned();
    let present = raster_file(&dir, "tile-r1c1.png", 5);
    let zone = |name: &str| LocalAdjustment {
        mask: MaskGeometry::select_sky(0.5, 0.2, false, missing.clone()),
        role: MaskRole::ZoneSky,
        name: name.to_string(),
        ..Default::default()
    };
    let tile = |name: &str| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: present.clone() },
        name: name.to_string(),
        ..Default::default()
    };
    let r = EditRecipe {
        masks: vec![zone("sky"), tile("tile"), zone("sky · band 1/3"), tile("tile again")],
        ..Default::default()
    };
    let (element, losses) = payload::rasters_element(&r, "asr", None);
    let lost: Vec<&str> = losses
        .iter()
        .filter(|l| l.reason == MaskLossReason::RasterNotEmbedded)
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(lost, ["sky", "sky · band 1/3"], "{losses:?}");
    assert_eq!(losses.len(), 2, "the embedded tile is no loss to either mask: {losses:?}");
    assert_eq!(element.matches("<rdf:li asr:Name=").count(), 1, "the shared tile still rides once: {element}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// F5 (2026-09-24 audit). A recipe that inflated past `MAX_RECIPE_JSON` was
/// CUT there and then failed its CRC, so the disclosure said "checksum does
/// not match" about a document whose checksum was fine. Too large is the fact.
#[test]
fn a_recipe_that_inflates_past_the_ceiling_is_refused_as_too_large_not_as_corrupt() {
    use base64::Engine as _;
    use std::io::Write as _;
    let doc = recipe_to_xmp(&EditRecipe { exposure_ev: 0.5, ..Default::default() });
    assert!(payload::find(&doc).is_some_and(|p| p.is_ok()), "premise: a sound payload");
    let oversize = vec![b' '; payload::MAX_RECIPE_JSON as usize + 1];
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&oversize).unwrap();
    let packed = base64::engine::general_purpose::STANDARD.encode(z.finish().unwrap());
    let recipe = payload::simple_property(&doc, "asr", "Recipe").unwrap();
    let crc = payload::simple_property(&doc, "asr", "RecipeCrc32").unwrap();
    let tampered = doc
        .replacen(&format!("asr:Recipe=\"{recipe}\""), &format!("asr:Recipe=\"{packed}\""), 1)
        .replacen(
            &format!("asr:RecipeCrc32=\"{crc}\""),
            &format!("asr:RecipeCrc32=\"{:08x}\"", crc32fast::hash(&oversize)),
            1,
        );
    assert_ne!(tampered, doc, "the swap landed");
    let why = match payload::find(&tampered) {
        Some(Err(why)) => why,
        Some(Ok(_)) => panic!("an oversize recipe must be refused"),
        None => panic!("the payload must still be found"),
    };
    assert!(why.contains("too large"), "{why}");
    assert!(!why.contains("checksum"), "{why}");
    // …and the crs reading stands, as for every payload this build cannot trust.
    assert_eq!(xmp_to_recipe(&tampered).exposure_ev, 0.5);
}

/// F6 (2026-09-24 audit). `claim_beside` claimed `<stem>-2.<ext>` with
/// `create_new` and, when the write or the sync then failed, returned the
/// error and LEFT the half-written file under the claimed name — where the
/// next restore's CRC check would find a DIFFERENT file and step on to `-3`.
/// A claim that could not be filled is released.
#[test]
fn a_claimed_name_whose_write_fails_is_released_not_left_half_written() {
    let dir = scratch("claim-release");
    let at = dir.join("tile-r3c3-2.png");
    // The whole write path: a claimed handle takes the bytes.
    let claimed = std::fs::OpenOptions::new().write(true).create_new(true).open(&at).unwrap();
    payload::fill_claimed(&at, claimed, b"a raster").expect("a writable claim fills");
    assert_eq!(std::fs::read(&at).unwrap(), b"a raster");
    // A handle the bytes cannot go through (opened read-only) is the failing
    // write — and the name it held is free again.
    let unwritable = std::fs::File::open(&at).unwrap();
    payload::fill_claimed(&at, unwritable, b"a raster").expect_err("a read-only handle cannot be filled");
    assert!(!at.exists(), "the claim is released along with the error");
    let _ = std::fs::remove_dir_all(&dir);
}

/// GROUP A (2026-10-26): the payload carries the depth map, protect matte and
/// pixel-layer paths as bare names, like every mask raster — the GUI holds them
/// absolute, and an absolute path in a sidecar leaks this machine's layout —
/// and a received payload naming a network path for any of them is refused on
/// restore exactly as a mask path is.
#[test]
fn the_payload_carries_depth_protect_and_layer_paths_bare_and_refuses_a_share() {
    let dir = scratch("rasters-bare");
    let dev = scratch("rasters-bare-dev");
    let abs = |n: &str| dir.join(n).to_string_lossy().into_owned();
    let r = EditRecipe {
        exposure_ev: 0.3,
        lens_blur: Some(crate::render::LensBlur {
            depth_map: abs("depth.png"),
            protect: Some(abs("protect.png")),
            ..Default::default()
        }),
        pixel_layers: vec![crate::render::PixelLayer { path: abs("layer.png"), ..Default::default() }],
        ..Default::default()
    };
    let p = payload::portable(&r);
    let lb = p.lens_blur.as_ref().unwrap();
    assert_eq!(
        (lb.depth_map.as_str(), lb.protect.as_deref(), p.pixel_layers[0].path.as_str()),
        ("depth.png", Some("protect.png"), "layer.png"),
        "no directory component goes into the sidecar"
    );

    let doc = recipe_to_xmp(&r);
    let mut found = payload::find(&doc).unwrap().unwrap();
    let lb = found.recipe.lens_blur.as_mut().unwrap();
    lb.depth_map = r"\\host\share\d.png".into();
    lb.protect = Some("//host/share/p.png".into());
    found.recipe.pixel_layers[0].path = "../../outside.png".into();
    let crs_only = xmp_to_recipe(&bare_document(&r, None));
    let back = payload::restore(crs_only, found, None, None, Some(&dev), false, None);
    let sentinel = dev.join(".invalid-mask-reference");
    let lb = back.lens_blur.as_ref().expect("the lens blur survives the restore");
    assert_eq!(Path::new(&lb.depth_map), sentinel.as_path());
    assert_eq!(lb.protect.as_deref().map(Path::new), Some(sentinel.as_path()));
    assert_eq!(Path::new(&back.pixel_layers[0].path), sentinel.as_path());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// A Lightroom sidecar as it sits beside a RAW before AutoShade writes over
/// it: the settings this writer does not own (the camera profile, a creative
/// Look, the lens-profile switch) are what a merge keeps from it.
const LIGHTROOM_BASE: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
     xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
     <rdf:Description rdf:about=\"\" \
     xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
     crs:ProcessVersion=\"15.4\" crs:LensProfileEnable=\"0\" crs:HasSettings=\"True\" \
     crs:CameraProfile=\"Adobe Standard\">\
     <crs:Look>\
      <rdf:Description crs:Name=\"Adobe Color\" crs:Amount=\"1\" \
       crs:UUID=\"B952C231111CD8E0ECCF14B86BAA7077\" crs:SupportsMonochrome=\"false\">\
      <crs:Parameters>\
       <rdf:Description crs:Version=\"18.0\" crs:ProcessVersion=\"15.4\" \
        crs:ConvertToGrayscale=\"False\" crs:CameraProfile=\"Adobe Standard\" \
        crs:LookTable=\"E1095149FDB39D7A057BAB208837E2E1\">\
       <crs:ToneCurvePV2012><rdf:Seq>\
        <rdf:li>0, 0</rdf:li><rdf:li>22, 16</rdf:li><rdf:li>40, 35</rdf:li>\
        <rdf:li>127, 127</rdf:li><rdf:li>224, 230</rdf:li><rdf:li>240, 246</rdf:li>\
        <rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>\
       </rdf:Description>\
      </crs:Parameters>\
      </rdf:Description>\
     </crs:Look>\
     </rdf:Description></rdf:RDF></x:xmpmeta>";

/// Our own sidecar, merged over a Lightroom base and read back untouched, is
/// the develop that was written — not the base's camera profile, Look and
/// lens switch, which the merge keeps for Lightroom (v1.17.1). Until then the
/// reader measured Lightroom's edits against a bare projection of the
/// develop, which never had those keys, so all three read as Lightroom edits:
/// the re-import rendered through the base's Adobe Color Look, and the import
/// stamp switched the camera's lens correction off.
///
/// Render → write → read → render: the two renders are the same bytes.
#[test]
fn our_own_sidecar_over_a_lightroom_base_reads_back_as_the_develop_it_carries() {
    let dir = scratch("own-over-base");
    let mut r = rich(&dir);
    r.lens_profile = crate::recipe::LensProfile {
        vignette_on: true,
        distortion_on: true,
        ca_on: true,
        ..Default::default()
    };
    let merged = merge_recipe_into_xmp(LIGHTROOM_BASE, &r).expect("a Lightroom base merges").doc;
    for kept in ["crs:CameraProfile=\"Adobe Standard\"", "<crs:Look>", "crs:LensProfileEnable=\"0\""] {
        assert!(merged.contains(kept), "premise: the merge keeps the base's {kept}: {merged}");
    }
    assert!(merged.contains("\n    asr:Written=\""), "the payload records the document as written");
    let found = payload::find(&merged).expect("a payload").expect("decodes");
    assert!(found.notes.is_empty(), "{:?}", found.notes);
    assert!(found.written.as_deref().is_some_and(|w| !w.contains("asr:")), "the record holds no payload of its own");

    // Field by field: everything the develop carries comes back as written.
    let back = load(&merged);
    assert_eq!(back.camera_profile, r.camera_profile, "the base's profile is not an edit");
    assert_eq!(back.look, r.look, "the base's Look is not an edit");
    assert_eq!(back, loaded(&r));
    // The lens profile the import stamp hands every surface is the develop's.
    assert_eq!(crate::xmp::written_lens_profile(&merged), Some(r.lens_profile.clone()));

    // The render round trip, on a recipe the preview renders whole (the rich
    // recipe's rasters are not images): the same bytes both ways.
    let plain = EditRecipe {
        exposure_ev: 0.35,
        contrast: 12.0,
        saturation: -8.0,
        lens_profile: r.lens_profile.clone(),
        ..Default::default()
    };
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(24, 16, |x, y| {
        image::Rgb([(x * 10) as u8, (y * 15) as u8, ((x + y) * 6) as u8])
    }));
    let doc = merge_recipe_into_xmp(LIGHTROOM_BASE, &plain).expect("merges").doc;
    let render = |r: &EditRecipe| crate::render::develop_preview(&img, r).to_rgb8().into_raw();
    assert_eq!(render(&load(&doc)), render(&plain), "write -> read must not move a pixel");
    let _ = std::fs::remove_dir_all(&dir);
}

/// …and Lightroom's edits still win: a camera profile or lens switch changed
/// AFTER the write differs from the recorded document, so it is Lightroom's.
#[test]
fn a_lightroom_edit_to_a_key_the_base_carried_still_wins() {
    let dir = scratch("base-key-edit");
    let mut r = rich(&dir);
    r.lens_profile = crate::recipe::LensProfile { distortion_on: true, ..Default::default() };
    let merged = merge_recipe_into_xmp(LIGHTROOM_BASE, &r).expect("merges").doc;
    // The ROOT tag's profile comes first in the document; the Look's own
    // baked one stays.
    let edited = merged.replacen("crs:CameraProfile=\"Adobe Standard\"", "crs:CameraProfile=\"Adobe Portrait\"", 1);
    assert_eq!(load(&edited).camera_profile, "Adobe Portrait", "the photographer's profile in Lightroom");
    let switched = merged.replacen("crs:LensProfileEnable=\"0\"", "crs:LensProfileEnable=\"1\"", 1);
    assert_eq!(crate::xmp::written_lens_profile(&switched), None, "a flipped switch is Lightroom's to obey");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The record of the written document is verified like the recipe: one
/// that does not match its checksum is disclosed and dropped, and the reader
/// measures against the bare projection as before the record existed.
#[test]
fn a_written_record_that_does_not_verify_is_disclosed_and_dropped() {
    let dir = scratch("written-crc");
    let r = rich(&dir);
    let merged = merge_recipe_into_xmp(LIGHTROOM_BASE, &r).expect("merges").doc;
    let key = "asr:WrittenCrc32=\"";
    let start = merged.find(key).expect("a record checksum") + key.len();
    let mut tampered = merged.clone();
    let digits = start..start + 8;
    let flipped = if &merged[digits.clone()] == "00000000" { "00000001" } else { "00000000" };
    tampered.replace_range(digits, flipped);
    let found = payload::find(&tampered).expect("a payload").expect("the recipe itself still decodes");
    assert!(found.written.is_none());
    assert!(found.notes.iter().any(|n| n.contains("record of the document AutoShade wrote")), "{:?}", found.notes);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A develop carrying the camera's lens profile, as a RAW's stamp fills it:
/// knots for both components, the Adobe profile Camera Raw takes for the photo.
fn stamped_lens(adobe: bool) -> crate::recipe::LensProfile {
    crate::recipe::LensProfile {
        vignette: vec![1.0, 1.1, 1.3],
        distortion: vec![1.0, 1.0, 1.01],
        vignette_on: true,
        distortion_on: true,
        ca_on: true,
        adobe: adobe.then(|| crate::recipe::AdobeLensProfile {
            name: "Adobe (Test 24mm)".into(),
            filename: "TEST (Test 24mm) - RAW.lcp".into(),
        }),
        ..Default::default()
    }
}

/// The lens switch is the develop's statement (v1.18.0): a develop that
/// rendered the camera's lens correction says `LensProfileEnable="1"` over a
/// Lightroom base that said `"0"`, so Lightroom draws the photo — and places
/// every mask — in the frame the develop drew them in (Camera Raw's geometry
/// against the export: 0 px on 63 of 63 tiles on both pairs measured, against
/// up to 33 px under the base's "0"). Our own document still reads back as the
/// develop it carries, byte for byte through the render (eeb0b99's law), and
/// what the switch carries leaves `asr:NotCarried`: with an Adobe profile for
/// the photo only this engine's vignette stays declared; with none, the whole
/// correction does.
#[test]
fn a_develop_with_the_cameras_lens_correction_states_the_switch_over_a_base_that_said_off() {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(24, 16, |x, y| {
        image::Rgb([(x * 10) as u8, (y * 15) as u8, ((x + y) * 6) as u8])
    }));
    let render = |r: &EditRecipe| crate::render::develop_preview(&img, r).to_rgb8().into_raw();
    for adobe in [true, false] {
        let r = EditRecipe { exposure_ev: 0.35, contrast: 12.0, lens_profile: stamped_lens(adobe), ..Default::default() };
        assert_eq!(r.lightroom_lens_profile_enable(), Some("1"));
        let doc = merge_recipe_into_xmp(LIGHTROOM_BASE, &r).expect("a Lightroom base merges").doc;
        assert_eq!(doc.matches("crs:LensProfileEnable=").count(), 1, "one switch, ours, not the base's beside it: {doc}");
        assert!(doc.contains("crs:LensProfileEnable=\"1\""), "{doc}");
        assert!(!doc.contains("crs:LensProfileSetup="), "the setup stays the document's (none here): {doc}");
        assert_eq!(crate::xmp::written_lens_profile(&doc), Some(r.lens_profile.clone()), "our own switch is not an edit");
        assert_eq!(load(&doc), loaded(&r), "adobe {adobe}: the read-back is the develop");
        assert_eq!(render(&load(&doc)), render(&r), "adobe {adobe}: write -> read must not move a pixel");
        let declared = crate::xmp::describe_global_losses(&crate::xmp::global_export_losses(&r)).unwrap_or_default();
        let vignette = "lens vignetting (this engine's; Lightroom applies its own profile's)";
        if adobe {
            assert!(declared.contains(vignette) && !declared.contains("lens profile correction"), "{declared}");
        } else {
            assert!(declared.contains("lens profile correction") && !declared.contains(vignette), "{declared}");
        }
        assert!(doc.contains(&xml_attr_escape(&declared)), "the document declares it: {doc}");
        // Lightroom switching it off after the write is Lightroom's to obey.
        let off = doc.replacen("crs:LensProfileEnable=\"1\"", "crs:LensProfileEnable=\"0\"", 1);
        assert_eq!(crate::xmp::written_lens_profile(&off), None);
    }
    // A fresh document states it too, and reads back the same way.
    let r = EditRecipe { lens_profile: stamped_lens(true), ..Default::default() };
    let fresh = recipe_to_xmp(&r);
    assert!(fresh.contains("crs:LensProfileEnable=\"1\""), "{fresh}");
    assert_eq!(crate::xmp::written_lens_profile(&fresh), Some(r.lens_profile.clone()));
    assert_eq!(render(&load(&fresh)), render(&r));
}

/// The other two states of the switch: a profile carried with both components
/// off says `"0"` (nothing is drawn on either side, so nothing is declared);
/// a recipe carrying no profile says nothing and the base's switch stays.
#[test]
fn a_switched_off_profile_says_off_and_no_profile_leaves_the_documents_switch() {
    let mut off = stamped_lens(true);
    off.vignette_on = false;
    off.distortion_on = false;
    let r = EditRecipe { lens_profile: off, ..Default::default() };
    assert_eq!(r.lightroom_lens_profile_enable(), Some("0"));
    let doc = merge_recipe_into_xmp(&LIGHTROOM_BASE.replace("crs:LensProfileEnable=\"0\"", "crs:LensProfileEnable=\"1\""), &r)
        .expect("merges")
        .doc;
    assert_eq!(doc.matches("crs:LensProfileEnable=").count(), 1, "{doc}");
    assert!(doc.contains("crs:LensProfileEnable=\"0\""), "{doc}");
    assert!(!crate::xmp::global_export_losses(&r).contains(&"lens_profile"));
    let bare = EditRecipe { exposure_ev: 0.5, ..Default::default() };
    assert_eq!(bare.lightroom_lens_profile_enable(), None);
    let on_base = LIGHTROOM_BASE.replace("crs:LensProfileEnable=\"0\"", "crs:LensProfileEnable=\"1\"");
    let kept = merge_recipe_into_xmp(&on_base, &bare).expect("merges").doc;
    assert!(kept.contains("crs:LensProfileEnable=\"1\""), "a recipe with no profile does not touch the switch: {kept}");
    assert!(!recipe_to_xmp(&bare).contains("LensProfileEnable"));
}
