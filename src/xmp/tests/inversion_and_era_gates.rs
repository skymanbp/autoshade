// One part of the sidecar's tests (src/xmp/tests.rs includes it): the probe sidecars, rotation loss, the inversion bit, gradients, subtract components, local keys, preserved mask blocks and the era gates.

/// The REAL probe sidecars, when they are on the machine. Twelve controlled
/// Lightroom exports live at
/// `lr-experiment/` in the R25 materials ledger, outside the tree; point
/// `AUTOSHADE_LR_PROBE_FIXTURES` at that directory and this walks every
/// `.xmp` in it (and its `probe*/` subdirectories), asserting that every
/// radial imports and round-trips its corners byte-for-byte.
///
/// SILENTLY SKIPPED when the variable is unset — the files are the user's
/// photographs' metadata and are deliberately not in the repository, so
/// this cannot be a CI gate. The synthetic fixtures above carry the same
/// numbers; this is what proves the transcription.
#[test]
fn the_probe_sidecars_decode_to_their_measured_ellipses() {
    let Some(dir) = crate::config::live_env("AUTOSHADE_LR_PROBE_FIXTURES") else { return };
    let mut checked = 0usize;
    let mut roots = vec![std::path::PathBuf::from(dir)];
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    while let Some(root) = roots.pop() {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                roots.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")) {
                files.push(p);
            }
        }
    }
    files.sort();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if !text.contains("Mask/CircularGradient") {
            continue;
        }
        let frame = FrameAspect::from_xmp(&text);
        assert!(frame.is_some(), "{}: a Lightroom sidecar declares its frame", path.display());
        let r = xmp_to_recipe(&text);
        assert!(!r.masks.is_empty(), "{}: its radial must import", path.display());
        let out = recipe_to_xmp_in_frame(&r, frame).0;
        let read = |doc: &str, key: &str| -> Option<String> {
            let name = format!("crs:{key}=\"");
            let at = doc.find(&name)? + name.len();
            Some(doc[at..][..doc[at..].find('"')?].to_string())
        };
        // Every corner the file wrote comes back BYTE-identical — the
        // decode/encode pair is an algebraic inverse and `lr_num` is
        // Lightroom's own spelling.
        for key in ["Top", "Left", "Bottom", "Right"] {
            let Some(want) = read(&text, key) else { continue };
            assert_eq!(
                read(&out, key).as_deref(),
                Some(want.as_str()),
                "{}: crs:{key} did not survive the round trip",
                path.display()
            );
        }
        // The angle to 10⁻⁴ °, for the `f32` reason the synthetic
        // round-trip test spells out.
        if let (Some(w), Some(g)) = (read(&text, "Angle"), read(&out, "Angle")) {
            let (w, g): (f64, f64) = (w.parse().unwrap(), g.parse().unwrap());
            assert!((w - g).abs() < 1e-4, "{}: crs:Angle {g} vs {w}", path.display());
        }
        // R27: the CROP block travels the same road, and every one of
        // these files carries `crs:HasCrop="False"` — so the crop codec
        // must leave them exactly that, not invent a full-frame carrier.
        for key in ["HasCrop", "CropTop", "CropLeft", "CropBottom", "CropRight", "CropAngle"] {
            assert_eq!(
                read(&out, key),
                read(&text, key),
                "{}: crs:{key} did not survive the round trip",
                path.display()
            );
        }
        // …and the frame this writer declares is the frame the file
        // declared (A8), which is what makes the round trip re-readable.
        // Both sides are read through the RESOLVED frame scope (R29-2), so
        // this compares what the reader will actually see, not a
        // first-occurrence sweep of two whole documents.
        for key in ["tiff:ImageWidth", "tiff:ImageLength", "tiff:Orientation"] {
            let want = FrameScope::resolve(&text).declared_number(key);
            let got = FrameScope::resolve(&out).declared_number(key);
            assert_eq!(want, got, "{}: {key} {got:?} vs {want:?}", path.display());
        }
        checked += 1;
    }
    assert!(checked > 0, "AUTOSHADE_LR_PROBE_FIXTURES held no radial sidecar");
    eprintln!("AUTOSHADE_LR_PROBE_FIXTURES: {checked} radial sidecar(s) round-tripped");
}

/// R25 P5: both rotation verdicts CARRY the angle, so a disclosure can
/// say how much tilt it set aside instead of only that some existed.
/// `0` is the payload's word for "no angle to name" — an unreadable
/// `crs:Angle`, or a tilt that rounds away — and the prose channels fall
/// back to their plain phrasing on it.
#[test]
fn the_rotation_loss_names_the_angle() {
    let radial = |angle: f32| MaskGeometry::Radial {
        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
        feather: 0.5, roundness: 0.0, flipped: false, angle,
        midpoint: 50.0, mask_version: 2,
    };
    let with = |angle: f32| EditRecipe {
        masks: vec![LocalAdjustment {
            mask: radial(angle),
            name: "tilted".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(
        mask_export_losses(&with(37.412506)),
        vec![MaskLoss { name: "tilted".into(), reason: MaskLossReason::Rotation(37) }],
        "the export verdict names the engine's own angle"
    );
    assert_eq!(
        mask_export_losses(&with(-12.0)),
        vec![MaskLoss { name: "tilted".into(), reason: MaskLossReason::Rotation(-12) }],
        "including its sign"
    );
    assert_eq!(
        mask_export_losses(&with(0.4)),
        vec![MaskLoss { name: "tilted".into(), reason: MaskLossReason::Rotation(0) }],
        "a tilt under half a degree is still a loss, with no angle worth naming"
    );

    // The import twin, on the sidecar's own value — the measured negative
    // end of the reference library's range.
    let imported = |angle: &str| {
        import_losses(&lr_doc(&lr_correction("Radial 1", "", &lr_radial(angle, "0"))))
    };
    assert_eq!(
        imported("-43.945287"),
        vec![MaskImportLoss {
            name: "Radial 1".into(),
            reason: MaskImportReason::Rotation(-44)
        }],
        "the import verdict names Lightroom's angle"
    );
    assert_eq!(
        imported("oblique"),
        vec![MaskImportLoss {
            name: "Radial 1".into(),
            reason: MaskImportReason::Rotation(0)
        }],
        "an unreadable angle still counts as rotated — we cannot say it is zero"
    );
    assert!(imported("0").is_empty(), "and an unrotated radial loses nothing at all");
}

/// FORENSIC CONCLUSION, REVISED IN R25 P9 — read the revision, it is the
/// interesting part.
///
/// R24 observed that the reference sidecars carry `crs:Flipped="true"`
/// BESIDE `crs:MaskInverted="false"` on the same component, concluded the
/// two were INDEPENDENT (different concepts, both real), and this test
/// pinned all four combinations importing to distinct flags. The
/// observation was right and is unchanged; the CONCLUSION was wrong, and
/// what falsified it was sample size. R24 saw one cell of a 2×2. The R25
/// census walked the user's whole library — 201 radials — and found
/// `Flipped` and `MaskInverted` PERFECTLY ANTI-CORRELATED: 155 `(true,
/// false)`, 46 `(false, true)`, and ZERO of either matching pair. The two
/// M-B batches agree on the raw bytes and are not in conflict: "the fields
/// are not mirror copies of each other" (R24) and "their values are always
/// opposite" (R25) are both true of the same files. Only the reading
/// "therefore they mean different things" does not survive.
///
/// So Lightroom writes ONE inversion bit TWICE. This engine has TWO flags
/// composed by XOR (`Radial::flipped` in `mask_weight`, `inverted` in the
/// weight loop, render.rs), and importing both halves of Lightroom's
/// redundant pair XORed a value with its own complement: the net came out
/// `true` for EVERY imported Lightroom radial whatever the file said.
/// Measured on `P34` against the real Lightroom export, tone-matched
/// RMS 0.1099 → 0.0751 (blue 0.1901 → 0.0869) once the flip was dropped
/// (E1-verdict §6 defect 2).
///
/// MUTATION THIS CATCHES: putting `crs:Flipped` back into the geometry
/// flag re-inverts every Lightroom radial (rows 1 and 2 below), and
/// writing our own `flipped` straight back out re-emits a pair Lightroom
/// never writes (the last assertion).
#[test]
fn lightroom_spells_one_inversion_bit_twice() {
    // The two pairs Lightroom actually writes, then the two it never does.
    // For the observed pairs the net is `MaskInverted`; for the impossible
    // ones the tie is broken in favour of `MaskInverted`, which is the
    // attribute this reader trusts (see the importer's comment).
    for (flipped, inverted, net) in [
        (true, false, false), // 155 of 201 in the library
        (false, true, true),  //  46 of 201
        (true, true, true),   //   0 of 201 — resolved, not guessed
        (false, false, false), //  0 of 201
    ] {
        let comp = lr_radial("0", "0")
            .replace("crs:Flipped=\"true\"", &format!("crs:Flipped=\"{flipped}\""))
            .replace("crs:MaskInverted=\"false\"", &format!("crs:MaskInverted=\"{inverted}\""));
        let doc = lr_doc(&lr_correction("Radial 1", "", &comp));
        let r = xmp_to_recipe(&doc);
        let MaskGeometry::Radial { flipped: got_f, .. } = r.masks[0].mask else {
            panic!("expected a radial, got {:?}", r.masks[0].mask);
        };
        assert!(
            !got_f,
            "Flipped={flipped} Inverted={inverted}: crs:Flipped must not reach the render flag"
        );
        assert_eq!(
            r.masks[0].inverted, inverted,
            "Flipped={flipped} Inverted={inverted}: MaskInverted is the inversion"
        );
        assert_eq!(
            lr_net_inverted(&r.masks[0]),
            net,
            "Flipped={flipped} Inverted={inverted}: net inversion"
        );
        // …and OUR writer re-emits the pair Lightroom would have written
        // for that net, so the two rows Lightroom really uses round-trip
        // byte-for-byte and the two it never writes are normalised onto
        // the nearest row it does.
        let xmp = recipe_to_xmp(&r);
        assert!(
            xmp.contains(&format!("crs:MaskInverted=\"{net}\"")),
            "Flipped={flipped} Inverted={inverted}: MaskInverted must carry the net"
        );
        assert!(
            xmp.contains(&format!("crs:Flipped=\"{}\"", !net)),
            "Flipped={flipped} Inverted={inverted}: crs:Flipped is its complement"
        );
        assert_eq!(
            lr_net_inverted(&xmp_to_recipe(&xmp).masks[0]),
            net,
            "Flipped={flipped} Inverted={inverted}: the net survives the round trip"
        );
    }
}

/// R25 P9, the other direction: a mask THIS APP flipped (the GUI's Flip
/// checkbox — `flipped: true`, `inverted: false`) used to export as
/// `crs:Flipped="false" crs:MaskInverted="false"`, a combination Lightroom
/// never writes and reads as NOT inverted. The flip was dropped at the
/// border, silently, in the one direction the user cannot check from
/// inside this app.
///
/// MUTATION THIS CATCHES: any writer that copies `flipped` into
/// `crs:Flipped` instead of deriving both attributes from the net.
#[test]
fn our_own_flip_leaves_as_lightrooms_own_inversion() {
    for (flipped, inverted) in [(true, false), (false, true), (true, true), (false, false)] {
        let m = LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                feather: 0.5, roundness: 0.0, flipped, angle: 0.0,
                midpoint: 50.0, mask_version: 2,
            },
            inverted,
            exposure_ev: -0.5,
            ..Default::default()
        };
        let net = flipped ^ inverted;
        let xmp = recipe_to_xmp(&EditRecipe { masks: vec![m], ..Default::default() });
        // Both attributes read off the SAME component tag — the pair is
        // the claim, and two whole-document `contains` could each be
        // satisfied by a different component.
        let p = Scope::new(xmp.as_str())
            .find_value_at("What", "Mask/CircularGradient")
            .expect("the radial must be emitted");
        let (s, e, _) = next_xml_tag(&xmp, p).expect("its component tag");
        let tag = Tag::new(&xmp[s..=e]);
        assert_eq!(
            tag.crs_str("MaskInverted").as_deref(),
            Some(if net { "true" } else { "false" }),
            "flipped={flipped} inverted={inverted}: the net must reach crs:MaskInverted"
        );
        assert_eq!(
            tag.crs_str("Flipped").as_deref(),
            Some(if net { "false" } else { "true" }),
            "flipped={flipped} inverted={inverted}: and its complement crs:Flipped — a \
                 matching pair is one Lightroom never writes, and it is what makes this \
                 projection safe under BOTH readings of which attribute Lightroom consults"
        );
        assert_eq!(
            lr_net_inverted(&xmp_to_recipe(&xmp).masks[0]),
            net,
            "flipped={flipped} inverted={inverted}: the rendered result must survive"
        );
    }
}

/// The other half of §0: `crs:MaskBlendMode` is on every component
/// Lightroom writes, and the import refused it unless WE had written the
/// file. Its default value is the plain composition the engine already
/// does, so accepting it costs nothing — and a lossless import must
/// report NO loss, or the disclosure cries wolf on every photo.
#[test]
fn a_lightroom_gradient_with_blend_mode_zero_imports_losslessly() {
    let doc = lr_doc(&lr_correction("Gradient 1", "", &lr_gradient("0")));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "a plain Lightroom gradient must import: {:?}", r.masks);
    assert!(import_losses(&doc).is_empty(), "a faithful import says nothing");
    assert_eq!(unsupported_corrections(&doc), 0);
    // …and a mode we cannot reproduce is a NOTE on an imported mask, not
    // a refusal: the shape is still exactly what the file draws.
    // (`lr_gradient` pairs the mode with `MaskValue="1"`; that pair does
    // not occur in the wild — see the next test, which uses the one that
    // does — but it isolates the blend-mode arm on its own.)
    let subtract = lr_doc(&lr_correction("Gradient 1", "", &lr_gradient("1")));
    let r2 = xmp_to_recipe(&subtract);
    assert_eq!(r2.masks.len(), 1, "a Subtract component still has a shape");
    assert_eq!(
        import_losses(&subtract),
        vec![MaskImportLoss {
            name: "Gradient 1".into(),
            reason: MaskImportReason::BlendMode
        }]
    );
}

/// v0.31.1: Lightroom's SUBTRACT is the PAIR `crs:MaskBlendMode="1"` +
/// `crs:MaskValue="0"`, and the second half is an ENCODING, not an opacity.
///
/// EVIDENCE (complete census of the GitHub-code-search-indexed population
/// of `.xmp` files containing `crs:MaskBlendMode` — 157 files, 479
/// attribute instances, verified twice, by regex and by
/// `xml.etree.ElementTree`, 0 parse failures, 2026-08-18):
/// `MaskBlendMode="1"` co-occurs with `MaskValue="0"` in 26 of 26
/// instances, and `MaskBlendMode="1"` with `MaskValue="1"` in 0 of 479.
/// The attribute never sits on the `crs:What="Correction"` element — it is
/// always a per-component value, which is where this reader looks.
///
/// The importer read that zero as "muted", refused the component, and the
/// geometry arm turned the refusal into `OutOfModel` — the user's whole
/// correction, thrown away to avoid a composition we could simply have
/// disclosed. Now the base shape imports and `BlendMode` names what did
/// not. The zero is never multiplied into anything; strength comes from
/// `crs:CorrectionAmount`, which this test also pins.
///
/// MUTATION THIS CATCHES: reading `MaskValue` as an opacity (the mask
/// arrives at strength 0), and dropping the `subtracted` guard (the whole
/// correction disappears again).
#[test]
fn a_real_lightroom_subtract_component_keeps_its_geometry() {
    // The real pair, on a component this reader has a model for.
    let subtract = lr_radial("0", "1").replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"");
    let doc = lr_doc(&lr_correction("Radial 1", "", &subtract));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the shape the file draws must survive: {:?}", r.masks);
    assert_eq!(unsupported_corrections(&doc), 0, "and the correction is not counted lost");
    assert_eq!(
        import_losses(&doc),
        vec![MaskImportLoss {
            name: "Radial 1".into(),
            reason: MaskImportReason::BlendMode
        }],
        "exactly one note: the composition, not the geometry"
    );
    // The geometry is the file's own, and the zero did NOT become a
    // strength: `CorrectionAmount="1"` is still the master opacity.
    let MaskGeometry::Radial { top, left, .. } = r.masks[0].mask else {
        panic!("expected a radial, got {:?}", r.masks[0].mask);
    };
    // The file's own `(0.114928, 0.590368)`, VERBATIM: the frame affine is
    // the identity since the 2026-08-19 `LR_MASK_FRAME_SCALE = 1.0` ruling
    // (see `a_lightroom_radial_with_angle_imports`).
    assert!(
        (top as f64 - 0.114928).abs() < 1e-7 && (left as f64 - 0.590368).abs() < 1e-7,
        "the real coordinates arrived: {top} {left}"
    );
    assert_eq!(r.masks[0].amount, 1.0, "MaskValue=0 is an encoding, never a pre-multiplier");
    assert_eq!(r.masks[0].contrast, 43.0, "and the sliders are untouched by it");

    // The guard is a PAIR. A zero MaskValue with the DEFAULT blend mode is
    // the muted component it always was, and still refuses.
    let muted = lr_radial("0", "0").replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"");
    let muted_doc = lr_doc(&lr_correction("Radial 1", "", &muted));
    assert!(xmp_to_recipe(&muted_doc).masks.is_empty(), "a muted component still refuses");
    assert_eq!(unsupported_corrections(&muted_doc), 1);

    // …and so does a genuinely PARTIAL value, blend mode or not — that is
    // coverage we can read and have no model for. (0.662178 is the one
    // non-0/1 MaskValue in the whole 479-instance census.)
    for blend in ["0", "1"] {
        let partial = lr_radial("0", blend)
            .replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0.662178\"");
        let partial_doc = lr_doc(&lr_correction("Radial 1", "", &partial));
        assert!(
            xmp_to_recipe(&partial_doc).masks.is_empty(),
            "MaskBlendMode={blend}: a partial MaskValue is not the subtract encoding"
        );
    }
}

/// v0.31.1: `crs:Roundness` is Lightroom's ±100 integer slider, so a user
/// who moved it no longer loses the mask.
///
/// EVIDENCE (direct observation of the harvested real-sidecar corpus,
/// 2026-08-18): all 24 `Mask/CircularGradient` components write Roundness
/// as a bare signed integer, every one at its default `0`, beside
/// `Feather="+100"` and `Midpoint="+50"` — integers on a 0..100-style
/// footing, not 0..1 reals. The importer's old `(0.0..=1.0)` gate was the
/// "bbox aspect ratio" reading of the field, and being a GEOMETRY check it
/// refused the entire correction.
///
/// The value is CARRIED, not converted (`mask_weight` never reads it), so
/// the ambiguous `1` needs no ruling: whatever scale it was written on, it
/// is written back as `1`. That is the difference from `feather`, which is
/// rendered and therefore must guess.
#[test]
fn a_lightroom_roundness_slider_is_carried_not_refused() {
    let with = |v: &str| {
        lr_doc(&lr_correction(
            "Radial 1",
            "",
            &lr_radial("0", "0").replace("crs:Roundness=\"0\"", &format!("crs:Roundness=\"{v}\"")),
        ))
    };
    // Both signs of the real slider, the ambiguous 1, and a value only our
    // own legacy writer could have produced.
    for (text, want) in [("-30", -30.0), ("+45", 45.0), ("1", 1.0), ("0.25", 0.25)] {
        let doc = with(text);
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "Roundness={text} must not cost the mask: {:?}", r.masks);
        assert!(import_losses(&doc).is_empty(), "Roundness={text}: carrying it loses nothing");
        let MaskGeometry::Radial { roundness, .. } = r.masks[0].mask else {
            panic!("expected a radial, got {:?}", r.masks[0].mask);
        };
        assert_eq!(roundness, want, "Roundness={text}: carried verbatim, never rescaled");
        // …and it goes back out as the same number, through the clamp.
        let back = xmp_to_recipe(&recipe_to_xmp(&r));
        let MaskGeometry::Radial { roundness: round2, .. } = back.masks[0].mask else {
            panic!("expected a radial, got {:?}", back.masks[0].mask);
        };
        assert_eq!(round2, want, "Roundness={text} did not survive our own writer");
    }
    // The gate still has ends: past the slider's own range is unreadable
    // geometry, and that IS a refusal.
    let wild = with("101");
    assert!(xmp_to_recipe(&wild).masks.is_empty(), "101 is off the Lightroom slider");
    assert_eq!(unsupported_corrections(&wild), 1);
}

/// A `crs:Local*` slider this engine has no model for used to cost the
/// user the whole mask. It is a knob, not a coverage change: the shape
/// and the fifteen sliders we DO model are still exactly the file's.
#[test]
fn a_nonzero_inert_local_no_longer_drops_the_whole_correction() {
    let doc = lr_doc(
        &lr_correction("Radial 1", "", &lr_radial("0", "0"))
            .replace("crs:LocalDefringe=\"0\"", "crs:LocalDefringe=\"0.3\""),
    );
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the mask imports: {:?}", r.masks);
    assert_eq!(r.masks[0].contrast, 43.0, "the modelled sliders came through");
    assert_eq!(
        import_losses(&doc),
        vec![MaskImportLoss {
            name: "Radial 1".into(),
            reason: MaskImportReason::InertLocal("LocalDefringe")
        }],
        "the slider that did not come through is named"
    );
    assert_eq!(unsupported_corrections(&doc), 0);
}

/// A `crs:Local*` attribute this build has never seen: same rule, and the
/// loss carries the CORRECTION's name so the sentence is actionable.
/// Also pins the two notes stacking on one correction.
#[test]
fn unknown_local_key_names_itself() {
    let doc = lr_doc(&lr_correction(
        "Sky",
        "       crs:LocalWhatsit=\"0.5\"\n",
        &lr_radial("0", "0"),
    ))
    .replace("crs:LocalCurveRefineSaturation=\"100\"", "crs:LocalCurveRefineSaturation=\"80\"");
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "an unknown knob is not a reason to lose the mask");
    let losses = import_losses(&doc);
    assert_eq!(losses.len(), 2, "both notes are raised: {losses:?}");
    assert!(losses.iter().all(|l| l.name == "Sky"), "each names the correction: {losses:?}");
    assert!(
        losses.iter().any(|l| l.reason == MaskImportReason::UnknownLocalKey),
        "{losses:?}"
    );
    assert!(
        losses.iter().any(|l| l.reason == MaskImportReason::CurveRefineSaturation),
        "{losses:?}"
    );
}

/// R25 P1 round-end. `crs:LocalCorrectedDepth`, `crs:LocalInputDigest`
/// and `crs:LocalInputDigestVersion` are Lightroom's own BOOKKEEPING —
/// a numeric flag and a recompute ledger (32-hex digest + schema version)
/// — and they rode into `UnknownLocalKey`, whose label reads "unmodelled
/// slider". That sentence was wrong about the thing AND wrong about how
/// often: 12 notes across the reference library's 31 importable
/// corrections, on files whose sliders all came through intact.
///
/// Knowing a key and not modelling it is the honest answer for all three.
/// The numeric one keeps the inert-key law all the same — silent at its
/// observed 0, NAMED at anything else — while the two string keys stay out
/// of `INERT_LOCAL`, because `optional_number_is` cannot parse a hex
/// digest and would raise a false note on every file that carries one.
#[test]
fn lightroom_bookkeeping_keys_are_known_not_unmodelled_sliders() {
    // The shapes are Lightroom's, the digest is a neutral test value: a
    // real digest is a hash OF the user's own file (fixture policy).
    let ledger = "       crs:LocalCorrectedDepth=\"0\"\n\
                      \x20      crs:LocalInputDigest=\"0000000000000000000000000000002A\"\n\
                      \x20      crs:LocalInputDigestVersion=\"1\"\n";
    let doc = lr_doc(&lr_correction("Sky", ledger, &lr_radial("0", "0")));
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "premise: the correction imports at all");
    assert!(
        import_losses(&doc).is_empty(),
        "Lightroom's own bookkeeping is not a loss: {:?}",
        import_losses(&doc)
    );

    // The inert-key law still holds for the numeric one. MUTATION THIS
    // CATCHES: adding it to `KNOWN_LOCAL` and NOT to `INERT_LOCAL` makes
    // a non-zero value silent, which is the opposite failure.
    let moved = doc.replace("crs:LocalCorrectedDepth=\"0\"", "crs:LocalCorrectedDepth=\"0.5\"");
    assert_eq!(
        import_losses(&moved),
        vec![MaskImportLoss {
            name: "Sky".into(),
            reason: MaskImportReason::InertLocal("LocalCorrectedDepth")
        }],
        "a bookkeeping flag off its observed value is named like any other inert key"
    );
    assert_eq!(xmp_to_recipe(&moved).masks.len(), 1, "and it still costs no mask");

    // The two STRING keys can never be read as numbers — if either were
    // in `INERT_LOCAL`, this document would raise a note for a value that
    // is exactly what Lightroom writes.
    assert!(
        import_losses(&doc.replace(
            "crs:LocalInputDigestVersion=\"1\"",
            "crs:LocalInputDigestVersion=\"2\""
        ))
        .is_empty(),
        "a digest ledger is not a slider at any value"
    );
}

/// THE TRAP OF THIS BATCH (data-corruption class). Once a lossy sidecar
/// imports, `r.masks.is_empty()` stops meaning "the user has not touched
/// these" — and the merge used that emptiness to decide whether to keep
/// the base's own mask block. Left alone, an ordinary Ctrl+S would have
/// written our DEGRADED reading (rotation read as 0, blend mode ignored,
/// `crs:Midpoint` / `crs:Version` not even read) over the user's own
/// Lightroom block, silently.
///
/// The whole round trip the app really takes is exercised, not just the
/// merge: import → serde_json → back (recipe.json is a file, and f32 that
/// does not survive the text round trip would make the equality fail on
/// the user's second launch, not in a unit test) → merge → the base's
/// mask block must come out byte-for-byte.
#[test]
fn preserve_masks_survives_a_lossy_import_the_user_did_not_touch() {
    let doc = lr_doc(&format!(
        "{}{}",
        lr_correction("Radial 1", "", &lr_radial("37.412506", "0")),
        lr_correction("Gradient 1", "", &lr_gradient("1")),
    ));
    let imported = xmp_to_recipe(&doc);
    assert_eq!(imported.masks.len(), 2, "premise: the masks really did import");
    assert_eq!(import_losses(&doc).len(), 2, "premise: the import really was lossy");

    // recipe.json in the middle, exactly as the app stores it.
    let json = serde_json::to_string(&imported).expect("serialise");
    let reloaded: EditRecipe = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(
        reloaded.masks, imported.masks,
        "an f32 that does not survive recipe.json would silently arm the overwrite"
    );

    let start = doc.find("<crs:MaskGroupBasedCorrections>").expect("block");
    let end = doc.find("</crs:MaskGroupBasedCorrections>").expect("block close")
        + "</crs:MaskGroupBasedCorrections>".len();
    let original = &doc[start..end];
    let out = merge_recipe_into_xmp(&doc, &reloaded).expect("mergeable");
    assert!(
        out.doc.contains(original),
        "the user's own mask block must survive an untouched save VERBATIM:\n{}",
        out.doc
    );
    assert!(
        out.doc.contains("crs:Angle=\"37.412506\"") && out.doc.contains("crs:Midpoint=\"50\""),
        "the parts we cannot express are exactly the parts this rule protects"
    );
    assert!(
        out.notes.is_empty(),
        "nothing was replaced, so nothing to disclose: {:?}",
        out.notes
    );
    // Our own projection is NOT prepended beside the base's block — one
    // document, two mask groups was the other way to get this wrong.
    assert_eq!(
        out.doc.matches("<crs:MaskGroupBasedCorrections>").count(),
        1,
        "exactly one mask group in the output"
    );
}

/// A `recipe.json` in the shape v0.30 wrote them: today's serialisation
/// MINUS `schema_era` and minus every field R25 added a `crs:` key for.
/// Built by deletion rather than by hand so the fixture cannot quietly
/// stop being a subset of what the app really writes — and read back
/// through the real serde path, because the whole point is what the FIELD
/// DEFAULTS do with an absent key.
fn as_v0_30_recipe(r: &EditRecipe) -> EditRecipe {
    let mut v = serde_json::to_value(r).expect("serialise");
    let obj = v.as_object_mut().expect("a recipe is an object");
    assert!(obj.remove("schema_era").is_some(), "the era stamp must have been there to remove");
    for (name, _) in era_attr_keys(1) {
        assert!(obj.remove(name).is_some(), "{name} is a recipe field");
    }
    // v1.5.0's fields are skipped at their defaults, so one is there to
    // remove only when the recipe moved it. By CONTROL, not by attribute:
    // the point colours are an element and have no attribute key.
    for name in crate::recipe::V150_CONTROLS {
        obj.remove(name);
    }
    serde_json::from_value(v).expect("deserialise")
}

/// The same deletion for a `recipe.json` in the shape v1.4 wrote: stamped
/// era 1, and without the keys v1.5.0 added.
fn as_v1_4_recipe(r: &EditRecipe) -> EditRecipe {
    let mut v = serde_json::to_value(r).expect("serialise");
    let obj = v.as_object_mut().expect("a recipe is an object");
    obj.insert("schema_era".to_string(), serde_json::json!(1));
    for name in crate::recipe::V150_CONTROLS {
        obj.remove(name);
    }
    serde_json::from_value(v).expect("deserialise")
}

/// A Lightroom sidecar carrying the R25 globals and no masks — the shape
/// the B2 / B3 keys actually arrive in (values from the reference
/// library: a negative Texture, the one signed decimal in the detail
/// block, a real post-crop vignette, a grain triple, and the de-fringe
/// block at Adobe's own defaults with one non-zero amount).
fn lr_globals_doc() -> String {
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20 <rdf:Description rdf:about=\"\"\n\
         \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
         \x20   crs:Version=\"15.5.1\"\n\
         \x20   crs:ProcessVersion=\"15.4\"\n\
         \x20   crs:Exposure2012=\"+0.35\"\n\
         \x20   crs:Texture=\"-20\"\n\
         \x20   crs:SharpenRadius=\"+1.0\"\n\
         \x20   crs:SharpenDetail=\"25\"\n\
         \x20   crs:PostCropVignetteAmount=\"-17\"\n\
         \x20   crs:PostCropVignetteMidpoint=\"50\"\n\
         \x20   crs:GrainAmount=\"30\"\n\
         \x20   crs:GrainSize=\"25\"\n\
         \x20   crs:GrainFrequency=\"50\"\n\
         \x20   crs:DefringePurpleAmount=\"3\"\n\
         \x20   crs:DefringePurpleHueLo=\"30\"\n\
         \x20   crs:DefringePurpleHueHi=\"70\"\n\
         \x20   crs:DefringeGreenAmount=\"0\"\n\
         \x20   crs:DefringeGreenHueLo=\"40\"\n\
         \x20   crs:DefringeGreenHueHi=\"60\"\n\
         \x20   crs:HasSettings=\"True\"/>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n"
        .to_string()
}

/// **THE R25 P8 TRAP, mask half** (data-destruction class), and the exact
/// scenario measured on the reference library: P31 lost four
/// corrections and P51 lost eight, with an empty note list.
///
/// P1 made Lightroom's own masks import CLEANLY, and the merge's preserve
/// arm was keyed on a `MaskSummary::preserve_original` flag that only a
/// LOSSY import ever set (it is gone now; `defects > 0` is what it meant). A v0.30 `recipe.json` is maskless by construction (that
/// build could not import one), so the pair "clean base + maskless
/// recipe" answered "nothing to preserve", and an ordinary Ctrl+S stripped
/// the block and wrote nothing in its place.
///
/// MUTATION THIS CATCHES: put the old flag's condition —
/// `summary.defects > 0`, which is exactly when `preserve_original` was
/// raised — back into either the preserve or the note test, and this goes
/// red on both assertions at once: the block vanishes AND nothing says so.
#[test]
fn a_clean_lightroom_mask_block_survives_a_recipe_that_never_saw_it() {
    let doc = lr_doc(&format!(
        "{}{}",
        lr_correction("Radial 1", "", &lr_radial("0", "0")),
        lr_correction("Gradient 1", "", &lr_gradient("0")),
    ));
    let imported = xmp_to_recipe(&doc);
    assert_eq!(imported.masks.len(), 2, "premise: the masks import");
    assert!(
        import_losses(&doc).is_empty(),
        "premise: and they import CLEANLY — that is what broke the old flag: {:?}",
        import_losses(&doc)
    );

    // The v0.30 recipe.json beside that sidecar: no masks, no era stamp.
    let legacy = EditRecipe { masks: Vec::new(), ..as_v0_30_recipe(&imported) };
    assert_eq!(legacy.schema_era, 0, "an absent key is what makes it legacy");

    let start = doc.find("<crs:MaskGroupBasedCorrections>").expect("block");
    let end = doc.find("</crs:MaskGroupBasedCorrections>").expect("block close")
        + "</crs:MaskGroupBasedCorrections>".len();
    let original = &doc[start..end];
    let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
    assert!(
        out.doc.contains(original),
        "the photographer's own mask block must survive VERBATIM:\n{}",
        out.doc
    );
    assert_eq!(
        out.doc.matches("<crs:MaskGroupBasedCorrections>").count(),
        1,
        "exactly one mask group in the output"
    );
    assert!(out.notes.is_empty(), "nothing was replaced: {:?}", out.notes);
}

/// The disclosure half of the same rule, on a base the importer
/// understands COMPLETELY. Before P8 this arm could not be reached at all
/// (the note was gated on that same lossy-import flag), so replacing perfectly
/// readable Lightroom corrections was a silent success — and the sentence
/// itself had to change, because "carries 0 thing(s) this build cannot
/// represent" is what the old wording says about a clean block.
#[test]
fn replacing_a_clean_mask_block_names_what_it_replaced() {
    let doc = lr_doc(&lr_correction("Radial 1", "", &lr_radial("0", "0")));
    assert!(import_losses(&doc).is_empty(), "premise: a clean base");
    let mut r = xmp_to_recipe(&doc);
    r.masks[0].exposure_ev = 1.25; // the user moved it: the develop is newest
    let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
    assert_eq!(out.notes.len(), 1, "the replacement is disclosed: {:?}", out.notes);
    assert!(
        out.notes[0].contains("1 correction(s)")
            && !out.notes[0].contains("0 thing(s)")
            && out.notes[0].contains("1 edited mask(s)"),
        "the note counts what was there, not a defect count of zero: {}",
        out.notes[0]
    );
}

/// **THE R25 P8 TRAP, globals half** (data-destruction class): three of the
/// seven reference sidecars lost nine keys each to this, silently.
///
/// Owning a key means the merge STRIPS it and the writer puts ours back —
/// and the writer omits a slider at rest. A v0.30 `recipe.json` has no
/// field for any of the twenty-seven keys R25 added, so serde fills them
/// from `EditRecipe::default()` and the recipe "says" texture 0, no grain,
/// no radius. Stripping on that reading deleted `crs:Texture="-20"`, the
/// whole Grain block and the PostCrop / SharpenRadius keys out of the
/// photographer's Lightroom file on an ordinary Ctrl+S.
///
/// MUTATION THIS CATCHES: return an empty set from
/// `unspoken_attr_keys` (or drop its `schema_era` test) and every
/// value below goes to the writer's default.
#[test]
fn a_v0_30_recipe_does_not_strip_the_keys_it_never_had() {
    let doc = lr_globals_doc();
    let imported = xmp_to_recipe(&doc);
    assert_eq!(imported.texture, -20.0, "premise: the fixture really carries them");
    assert_eq!((imported.grain, imported.sharpen_radius), (30.0, 1.0));

    let legacy = as_v0_30_recipe(&imported);
    assert_eq!(legacy.schema_era, 0);
    assert_eq!(legacy.texture, 0.0, "premise: serde filled the absent field with the default");

    let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
    for spelling in [
        "crs:Texture=\"-20\"",
        "crs:SharpenRadius=\"+1.0\"",
        "crs:SharpenDetail=\"25\"",
        "crs:PostCropVignetteAmount=\"-17\"",
        "crs:PostCropVignetteMidpoint=\"50\"",
        "crs:GrainAmount=\"30\"",
        "crs:GrainSize=\"25\"",
        "crs:GrainFrequency=\"50\"",
        "crs:DefringePurpleAmount=\"3\"",
    ] {
        assert!(out.doc.contains(spelling), "{spelling} was deleted from the user's file");
    }
    // Suppressing the strip WITHOUT suppressing the write is the other way
    // to get this wrong: one tag, two answers.
    for key in ["Texture", "GrainAmount", "DefringePurpleAmount", "DefringePurpleHueLo"] {
        assert_eq!(
            out.doc.matches(&format!("crs:{key}=")).count(),
            1,
            "crs:{key} must appear exactly once"
        );
    }
    assert_eq!(xmp_to_recipe(&out.doc).texture, -20.0, "…and it reads back as itself");
    assert!(out.notes.is_empty(), "nothing was lost, so nothing to disclose: {:?}", out.notes);
}

/// The CONTROL for the test above, and the reason the gate is an era stamp
/// rather than a new policy: a CURRENT-era recipe that says texture 0 is
/// STATING a value, and the merge must still publish it over the base's.
/// Whatever else this batch changed, it did not change what a save means.
#[test]
fn a_current_era_recipe_still_owns_every_key_it_states() {
    let doc = lr_globals_doc();
    let mut r = xmp_to_recipe(&doc);
    assert_eq!(r.schema_era, crate::recipe::SCHEMA_ERA, "an import is current-era");
    r.texture = 0.0;
    r.grain = 0.0;
    let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
    assert!(out.doc.contains("crs:Texture=\"0\""), "the cleared slider publishes: {}", out.doc);
    assert!(!out.doc.contains("crs:GrainAmount="), "a zero grain is an omitted key");
    assert_eq!(xmp_to_recipe(&out.doc).texture, 0.0);
}

/// The gate is PER KEY, and this is the case that forces it: a legacy
/// recipe whose Texture the user has just dragged. Nothing ever re-stamps
/// a file's era, so a whole-recipe gate would mean a v0.30 photo could
/// never write Texture to its sidecar again — the same silent divergence
/// class this round is closing, re-introduced by the fix for it.
#[test]
fn an_edited_key_leaves_the_era_gate_even_on_a_legacy_recipe() {
    let doc = lr_globals_doc();
    let mut legacy = as_v0_30_recipe(&xmp_to_recipe(&doc));
    legacy.texture = 20.0; // the user moved THIS slider and nothing else
    let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
    assert!(out.doc.contains("crs:Texture=\"+20\""), "the edit reaches the file: {}", out.doc);
    assert_eq!(out.doc.matches("crs:Texture=").count(), 1, "and only once");
    // Its untouched neighbours are still protected — the gate did not open
    // for the whole recipe.
    assert!(out.doc.contains("crs:GrainAmount=\"30\""), "the grain block stands");
}

/// The de-fringe six move as ONE block or not at all: the writer emits all
/// six unconditionally because a hue window with no amount beside it is a
/// shape no real document has, and a per-key gate that released three of
/// them would publish exactly that.
#[test]
fn the_era_gate_releases_the_de_fringe_block_whole() {
    let doc = lr_globals_doc();
    let mut legacy = as_v0_30_recipe(&xmp_to_recipe(&doc));
    legacy.defringe_purple = 5.0; // one key of the six
    let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
    for key in [
        "DefringePurpleAmount",
        "DefringePurpleHueLo",
        "DefringePurpleHueHi",
        "DefringeGreenAmount",
        "DefringeGreenHueLo",
        "DefringeGreenHueHi",
    ] {
        assert_eq!(
            out.doc.matches(&format!("crs:{key}=")).count(),
            1,
            "crs:{key} must be written exactly once when the block moves"
        );
    }
    assert!(out.doc.contains("crs:DefringePurpleAmount=\"5\""), "{}", out.doc);
}

/// The era gate's universe for era 1, DERIVED and pinned: exactly the
/// twenty-seven attribute keys R25 gave this writer. The controls are
/// named per era in `recipe::SCHEMA_ERA_CONTROLS` and the spellings derived
/// from their registry rows; this asserts the derivation produces the list,
/// whatever tier those rows have since moved to.
#[test]
fn the_era_gate_is_the_twenty_seven_keys_r25_added() {
    let mut keys: Vec<&str> = era_attr_keys(1).into_iter().map(|(_, k)| k).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "AutoLateralCA",
            "ChromaticAberrationB",
            "ChromaticAberrationR",
            "ColorNoiseReduction",
            "ColorNoiseReductionDetail",
            "ColorNoiseReductionSmoothness",
            "DefringeGreenAmount",
            "DefringeGreenHueHi",
            "DefringeGreenHueLo",
            "DefringePurpleAmount",
            "DefringePurpleHueHi",
            "DefringePurpleHueLo",
            "GrainAmount",
            "GrainFrequency",
            "GrainSize",
            "LuminanceNoiseReductionContrast",
            "LuminanceNoiseReductionDetail",
            "PostCropVignetteAmount",
            "PostCropVignetteFeather",
            "PostCropVignetteHighlightContrast",
            "PostCropVignetteMidpoint",
            "PostCropVignetteRoundness",
            "PostCropVignetteStyle",
            "SharpenDetail",
            "SharpenEdgeMasking",
            "SharpenRadius",
            "Texture",
        ]
    );
    // Every one of them is a key this writer OWNS — a gated key the merge
    // never strips anyway would be a rule about nothing.
    let owned = owned_attr_keys();
    for k in &keys {
        assert!(owned.contains(&(*k).to_string()), "{k} is not an owned attribute");
    }
    // And the gate really is EMPTY for a current-era recipe: the ordinary
    // save path pays nothing and changes nothing.
    // A current-era recipe is gated on nothing it has SEEN. The one key it
    // is still silent about is the profile NAME, and for the other reason
    // `unspoken_attr_keys` now carries: an empty name is "not stated", so
    // an ordinary save leaves Lightroom's own profile alone.
    // The lens profile's identity likewise: written only with a strength
    // that needs it named, so an ordinary save leaves Lightroom's own alone.
    assert_eq!(
        unspoken_attr_keys(&EditRecipe::default()).into_iter().collect::<Vec<_>>(),
        vec!["CameraProfile", "LensProfileFilename", "LensProfileName", "LensProfileSetup"]
    );
    assert!(
        unspoken_attr_keys(&EditRecipe {
            camera_profile: "Adobe Standard".to_string(),
            ..Default::default()
        })
        .into_iter()
        .all(|k| k.starts_with("LensProfile")),
        "a recipe that NAMES a profile speaks for the key and owns it"
    );
    let v150 = era_attr_keys(2).len();
    assert_eq!(
        unspoken_attr_keys(&EditRecipe { schema_era: 0, ..Default::default() }).len(),
        27 + v150 + 3,
        "an untouched v0.30 recipe suppresses all twenty-seven and every later era's"
    );
}

/// v1.5.0's era: the keys it added, DERIVED and pinned like R25's, and the
/// gate PER ERA — an untouched v1.4 (era-1) recipe suppresses exactly
/// these and not one R25 key, because it has held those all along.
///
/// MUTATION THIS CATCHES: `unspoken_attr_keys` gating from era 1
/// regardless of the stamp (an era-1 recipe would lose its R25 keys to the
/// gate — a Texture cleared in AutoShade would stop reaching the sidecar),
/// or a v1.5.0 control left off `recipe::V150_CONTROLS`.
#[test]
fn the_era_gate_names_the_keys_v1_5_0_added() {
    let mut keys: Vec<&str> = era_attr_keys(2).into_iter().map(|(_, k)| k).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "BlueHue",
            "BlueSaturation",
            "CameraProfile",
            "ConvertToGrayscale",
            "CropConstrainToWarp",
            "GrayMixerAqua",
            "GrayMixerBlue",
            "GrayMixerGreen",
            "GrayMixerMagenta",
            "GrayMixerOrange",
            "GrayMixerPurple",
            "GrayMixerRed",
            "GrayMixerYellow",
            "GreenHue",
            "GreenSaturation",
            // v1.5.0 F8.
            "HDREditMode",
            "HDRMaxValue",
            "LensProfileDistortionScale",
            "LensProfileVignettingScale",
            "ParametricDarks",
            "ParametricHighlightSplit",
            "ParametricHighlights",
            "ParametricLights",
            "ParametricMidtoneSplit",
            "ParametricShadowSplit",
            "ParametricShadows",
            "PerspectiveAspect",
            "PerspectiveHorizontal",
            "PerspectiveRotate",
            "PerspectiveScale",
            "PerspectiveUpright",
            "PerspectiveVertical",
            "PerspectiveX",
            "PerspectiveY",
            "RedHue",
            "RedSaturation",
            "SDRBlend",
            "SDRBrightness",
            "SDRClarity",
            "SDRContrast",
            "SDRHighlights",
            "SDRShadows",
            "SDRWhites",
            "ShadowTint",
        ]
    );
    let owned = owned_attr_keys();
    for k in &keys {
        assert!(owned.contains(&(*k).to_string()), "{k} is not an owned attribute");
    }
    let mut gated: Vec<&str> =
        unspoken_attr_keys(&EditRecipe { schema_era: 1, ..Default::default() }).into_iter().collect();
    gated.sort_unstable();
    // Plus the lens profile's identity, which is not an era's but the same
    // rule's: unspoken while no strength needs the profile named.
    let mut want = keys.clone();
    want.extend(["LensProfileFilename", "LensProfileName", "LensProfileSetup"]);
    want.sort_unstable();
    assert_eq!(gated, want, "an untouched v1.4 recipe is gated on v1.5.0's keys and nothing else");
    assert!(era_attr_keys(0).is_empty() && era_attr_keys(crate::recipe::SCHEMA_ERA + 1).is_empty());
}

/// A Lightroom sidecar with a real parametric curve — the block in the
/// shape Lightroom 9.4 writes it (all seven keys, signed regions, bare
/// splits) — beside a Basic-panel Texture.
fn lr_parametric_doc() -> String {
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20 <rdf:Description rdf:about=\"\"\n\
         \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
         \x20   crs:Version=\"15.5.1\"\n\
         \x20   crs:ProcessVersion=\"15.4\"\n\
         \x20   crs:Texture=\"-20\"\n\
         \x20   crs:ParametricShadows=\"0\"\n\
         \x20   crs:ParametricDarks=\"+40\"\n\
         \x20   crs:ParametricLights=\"-15\"\n\
         \x20   crs:ParametricHighlights=\"0\"\n\
         \x20   crs:ParametricShadowSplit=\"25\"\n\
         \x20   crs:ParametricMidtoneSplit=\"50\"\n\
         \x20   crs:ParametricHighlightSplit=\"80\"\n\
         \x20   crs:HasSettings=\"True\"/>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n"
        .to_string()
}

/// The mask Color tint is carried both ways: a Lightroom correction holding
/// `LocalToningHue` / `LocalToningSaturation` imports onto the recipe's own
/// tint with no loss raised, the writer emits the recipe's values on the
/// file's own scales, and the round trip returns the same two numbers. A
/// strength off the swatch's 0..1 range is refused like any off-slider value.
#[test]
fn the_mask_colour_tint_imports_exports_and_round_trips() {
    let doc = lr_doc(
        &lr_correction("Sky", "", &lr_radial("0", "0"))
            .replace("crs:LocalToningHue=\"0\"", "crs:LocalToningHue=\"231\"")
            .replace("crs:LocalToningSaturation=\"0\"", "crs:LocalToningSaturation=\"0.73\""),
    );
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the tinted mask imports: {:?}", r.masks);
    assert_eq!((r.masks[0].toning_hue, r.masks[0].toning_amount), (231.0, 73.0));
    assert_eq!(import_losses(&doc), vec![], "a tint the engine renders is not a loss");
    let xmp = recipe_to_xmp(&r);
    assert!(
        xmp.contains("crs:LocalToningHue=\"231\"") && xmp.contains("crs:LocalToningSaturation=\"0.73\""),
        "the writer emits the recipe's tint: {xmp}"
    );
    let back = xmp_to_recipe(&xmp);
    assert_eq!((back.masks[0].toning_hue, back.masks[0].toning_amount), (231.0, 73.0));
    let wild = doc.replace("crs:LocalToningSaturation=\"0.73\"", "crs:LocalToningSaturation=\"1.5\"");
    assert_eq!(unsupported_corrections(&wild), 1, "1.5 is off the swatch");
}
