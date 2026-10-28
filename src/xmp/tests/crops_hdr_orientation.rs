// One part of the sidecar's tests (src/xmp/tests.rs includes it): local hue scale, the measured crops, spot removal, HDR, detail companions, portrait captures and the declared orientation.

/// `crs:LocalHue` rides a 180 scale, not the ÷100 every other local key
/// uses. MEASURED, 2026-08-18: the user's controlled export put the mask
/// Hue slider at **+50** and Lightroom wrote **`crs:LocalHue="0.277778"`**
/// (`P14.xmp`, verbatim). 0.277778 × 180 = 50.00004; ÷100 would read
/// 27.8 and ÷360 would read 100.
///
/// The gate moves with the reader: at the old 100 scale a slider past
/// ±55.6 refused the WHOLE correction as out of model. Its end stop is the
/// file's ±1.0 (v1.17.0): Camera Raw renders `LocalHue="1.0"` as a half
/// turn, so ±180 is inside the model and only past it is refused.
///
/// MUTATION THIS CATCHES: put `q100("LocalHue")` back and the anchor reads
/// 27.7778; leave the domain gate on the 100 scale and the ±100 row
/// refuses; put the old ±100 stop back and the ±1.0 row refuses.
#[test]
fn local_hue_rides_the_measured_180_scale() {
    let doc = lr_doc(&lr_correction("Hue", "", &lr_radial("0", "0")))
        .replace("crs:LocalHue=\"0\"", "crs:LocalHue=\"0.277778\"");
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "the correction imports");
    assert!((r.masks[0].hue - 50.0).abs() < 1e-3, "UI +50 ⇒ {}", r.masks[0].hue);
    // The old ±100 stops (±0.555556, ±100.00008 through the wire's six
    // decimals) and the file's own ±1.0, a half turn, are inside the model.
    for (text, want) in [("0.555556", 100.0), ("-0.555556", -100.0), ("0.7", 126.0), ("1.0", 180.0), ("-1", -180.0)] {
        let doc = lr_doc(&lr_correction("Hue", "", &lr_radial("0", "0")))
            .replace("crs:LocalHue=\"0\"", &format!("crs:LocalHue=\"{text}\""));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "{text} is inside Lightroom's own slider");
        assert!((r.masks[0].hue - want).abs() < 1e-2, "{text} ⇒ {}", r.masks[0].hue);
    }
    // …and a value PAST the half turn is out of model and refused, which is
    // the half the gate exists for.
    let doc = lr_doc(&lr_correction("Hue", "", &lr_radial("0", "0")))
        .replace("crs:LocalHue=\"0\"", "crs:LocalHue=\"1.01\"");
    assert!(xmp_to_recipe(&doc).masks.is_empty(), "a hue past the slider is out of model");
    assert_eq!(
        import_losses(&doc),
        vec![MaskImportLoss { name: "Hue".into(), reason: MaskImportReason::OutOfModel }],
        "…and NAMED"
    );
    // …and the writer is its inverse.
    let mine = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                midpoint: 50.0, mask_version: 2,
            },
            hue: 50.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let out = recipe_to_xmp(&mine);
    assert!(out.contains("crs:LocalHue=\"0.2777778\""), "{out}");
    assert!((xmp_to_recipe(&out).masks[0].hue - 50.0).abs() < 1e-3);
}

// ── R27: the crop rectangle is the SAME rotated-corner encoding ─────────
//
// The seven rows below are the user's own library, transcribed verbatim
// from `p3-scratch/final_table.py:10-18` in the R27 materials ledger, outside the tree
// (the script `P3-cropangle-model.md` §8 lists as its reproduction). Each
// is a self-consistent pair: the crop block and the pixels come out of the
// same file, so a stale sidecar cannot contaminate it. NO photograph and
// NO export is in this repository — these are the metadata numbers and the
// exported dimensions they predict.
//
// `(name, W, H, L, T, R, B, CropAngle, exported W × H in SENSOR
//  orientation, px of width and height this engine's composition clamps
//  away)`.
type P3Crop = (&'static str, f64, f64, f64, f64, f64, f64, f64, f64, f64, f64, f64);
const P3_CROPS: [P3Crop; 7] = [
    ("P11", 9504.0, 6336.0, 0.119389, 0.003478, 0.994441, 0.991209, -0.132584, 8302.0, 6277.0, 0.000, 0.000),
    ("P30_1", 9504.0, 6336.0, 0.00219, 0.007883, 0.99781, 0.992117, -0.303486, 9429.0, 6286.0, 0.000, 0.216),
    ("P45", 9504.0, 6336.0, 0.013441, 0.0, 0.986559, 1.0, 0.724343, 9328.0, 6219.0, 0.000, 1.243),
    ("P40_hdr", 9438.0, 6265.0, 0.005237, 0.018724, 0.994763, 0.981276, -0.725680, 9262.0, 6148.0, 0.000, 1.251),
    ("P41", 9504.0, 6336.0, 0.031274, 0.0, 0.968726, 1.0, 1.728388, 9097.0, 6065.0, 0.000, 6.909),
    ("P44_1", 9504.0, 6336.0, 0.094153, 0.135618, 0.978448, 0.955195, -1.979145, 8220.0, 5480.0, 0.000, 30.260),
    ("P08", 9504.0, 6336.0, 0.09115, 0.100566, 1.0, 0.900897, -3.274380, 8334.0, 5556.0, 5.895, 0.000),
];

/// §0 OF R27 BATCH-3, the crop half. `crs:Crop{Left,Top,Right,Bottom}` are
/// the two opposite ROTATED CORNERS of the crop rectangle in the un-rotated
/// source frame — the identical encoding `BBOX-DECODE.md` found for
/// `Mask/CircularGradient`, one family — and the exported dimensions are
/// that rectangle's own side lengths, `2p × 2q`.
///
/// **7/7 pixel-exact, zero free parameters**, across two signs of
/// `CropAngle`, two `tiff:Orientation` states and two source aspect ratios
/// (`P3-cropangle-model.md` §3.2). The rivals, on the same seven rows:
/// naive AABB (what this build read until R27) 485 px, 0/7; opposite sign
/// 987 px; rotation in normalised space 165 px; AABB of the rotated rect
/// 957 px; fractions of the rotated bbox 619 px; of the inscribed rect
/// 878 px; side length ÷cos θ 477 px. The first two are asserted below, so
/// restoring either reading fails here rather than in a photograph.
///
/// The last two columns are what this engine's own composition
/// (rotate → auto-crop to the inscribed rectangle → crop) cannot express:
/// a Lightroom rectangle pushed against the edge of the rotated frame can
/// reach outside the CENTRED inscribed rectangle. Measured, per specimen,
/// and it is 0 on the first row, ≤ 1.3 px on three more, and worst at
/// 30.3 px = 0.55 % of one edge.
///
/// MUTATION THIS CATCHES: drop the `sin`/`cos` mixing in
/// `lr_to_engine_crop` (i.e. go back to `W·(R−L) × H·(B−T)`) and every row
/// but the sign-free ones misses its exported size by tens to hundreds of
/// pixels; flip the straighten's sign and the predicted sizes swap into the
/// opposite-sign column.
#[test]
fn the_seven_measured_lightroom_crops_reproduce_their_exported_dimensions() {
    for (name, w, h, l, t, r, b, angle, ow, oh, lost_w, lost_h) in P3_CROPS {
        let frame = FrameAspect::from_size(w, h);
        let lr = LrCrop { left: l, top: t, right: r, bottom: b, angle_deg: angle };
        let CropDecode::Read { crop: Some(c), straighten_deg, .. } =
            lr_to_engine_crop(lr, frame, false)
        else {
            panic!("{name}: a real Lightroom crop must decode");
        };
        assert!(
            (straighten_deg + angle).abs() < 1e-12,
            "{name}: the engine's clockwise straighten is −CropAngle"
        );
        // The frame this engine's straighten leaves behind, in pixels.
        let (wi, hi) = inscribed_norm(w / h, straighten_deg);
        let (wi, hi) = (wi * h, hi * h);
        // …and the rectangle's own side lengths inside it, with the
        // clamped edge added back: that IS the model's `2p × 2q`.
        let out_w = (c.right - c.left) as f64 * wi + lost_w;
        let out_h = (c.bottom - c.top) as f64 * hi + lost_h;
        // Half a pixel — `W_out = round(2p)` per axis — on the six
        // specimens whose crop block and pixels come out of the SAME file.
        // `P41` is the one paired to a SIDECAR, so its block may have
        // moved since the export; `P3-cropangle-model.md` §6.5 registers
        // its −0.61 px height as exactly that (and notes 9097/1.5 =
        // 6064.67, which is what an aspect lock would give).
        let tol = if name == "P41" { 0.65 } else { 0.5 };
        assert!(
            (out_w - ow).abs() < tol && (out_h - oh).abs() < tol,
            "{name}: model says {out_w:.1} × {out_h:.1}, Lightroom exported {ow} × {oh}"
        );
        // The two headline rivals, refuted on this row's own numbers.
        let naive = ((r - l) * w, (b - t) * h);
        let flipped = {
            let (sin, cos) = angle.to_radians().sin_cos();
            let (x, y) = ((r - l) / 2.0 * w, (b - t) / 2.0 * h);
            (2.0 * (x * cos - y * sin), 2.0 * (x * sin + y * cos))
        };
        if angle.abs() > 0.2 {
            assert!(
                (naive.0 - ow).abs() > 1.0 || (naive.1 - oh).abs() > 1.0,
                "{name}: the naive AABB must NOT reproduce {ow} × {oh}"
            );
            assert!(
                (flipped.0 - ow).abs() > 1.0 || (flipped.1 - oh).abs() > 1.0,
                "{name}: the opposite sign must NOT reproduce {ow} × {oh}"
            );
        }
    }
}

/// The crop codec is an ALGEBRAIC inverse: whatever Lightroom wrote comes
/// back, to the last decimal it spelled. Run on the four P3 specimens whose
/// rectangle fits this engine's inscribed frame outright, plus the two
/// arrangements §6.3 says are legal.
///
/// MUTATION THIS CATCHES: swap `p*cos − q*sin` for `p*cos + q*sin` in
/// `engine_to_lr_crop` (the natural sign slip) and every rotated row comes
/// back with different corners.
#[test]
fn the_crop_corner_encoding_round_trips_what_lightroom_wrote() {
    for (name, w, h, l, t, r, b, angle, ..) in P3_CROPS {
        let frame = FrameAspect::from_size(w, h);
        let lr = LrCrop { left: l, top: t, right: r, bottom: b, angle_deg: angle };
        let CropDecode::Read { crop: Some(c), straighten_deg, overshoot_frac } =
            lr_to_engine_crop(lr, frame, false)
        else {
            panic!("{name}: must decode");
        };
        let back = engine_to_lr_crop(Some(&c), straighten_deg, frame).expect("a crop");
        // The clamp is the only lossy edge, and it moves an edge by at
        // most its own overshoot — in units of the source height, which
        // bounds every corner coordinate (Top/Bottom are fractions of
        // that height, Left/Right of the wider width) — so that is the
        // tolerance, and it is ZERO on the rows that needed no clamp.
        let tol = overshoot_frac + 1e-6;
        for (got, want, key) in [
            (back.left, l, "Left"),
            (back.top, t, "Top"),
            (back.right, r, "Right"),
            (back.bottom, b, "Bottom"),
        ] {
            assert!(
                (got - want).abs() <= tol,
                "{name}: crs:Crop{key} {got:.6} vs {want:.6} (tol {tol:.6})"
            );
        }
        assert!((back.angle_deg - angle).abs() < 1e-12, "{name}: the angle is exact");
    }
}

/// R27 `P3-cropangle-model.md` §6.3: `Left > Right` is a legal Lightroom
/// arrangement — under the corner encoding it means `X < 0`, i.e.
/// `tan θ > p/q`, which a 2:3 crop straightened past +33.69° produces, and
/// `crs:CropAngle`'s own documented range is ±45°. The pre-R27 reader
/// required `left < right && top < bottom` and threw such a crop away in
/// silence.
///
/// The fixture is CONSTRUCTED, not copied: no file in the user's library
/// reaches the inverted region (the nine non-zero `CropAngle` sidecars all
/// sit ≥ 30.4° from their own wall), so the encoder builds the corners a
/// 35° straighten of a tall rectangle would produce and the decoder has to
/// read them back.
///
/// MUTATION THIS CATCHES: restore either half of the ordering guard and
/// the inverted arrangement decodes to `None` — a crop silently gone.
#[test]
fn an_inverted_crop_arrangement_is_read_rather_than_discarded() {
    // A tall (2:3) rectangle inside a 3:2 frame, straightened 35°.
    let frame = FrameAspect::from_size(9504.0, 6336.0);
    let engine = Crop { left: 0.30, top: 0.10, right: 0.62, bottom: 0.90 };
    let lr = engine_to_lr_crop(Some(&engine), -35.0, frame).expect("corners");
    assert!(
        lr.left > lr.right,
        "the fixture must actually reach the inverted region: {lr:?}"
    );
    let CropDecode::Read { crop: Some(back), straighten_deg, .. } =
        lr_to_engine_crop(lr, frame, false)
    else {
        panic!("an inverted arrangement must still decode: {lr:?}");
    };
    assert!((straighten_deg + 35.0).abs() < 1e-12);
    for (got, want) in [
        (back.left, engine.left),
        (back.top, engine.top),
        (back.right, engine.right),
        (back.bottom, engine.bottom),
    ] {
        assert!((got - want).abs() < 1e-5, "{got} != {want} ({back:?})");
    }
}

/// The whole document round trip on a tilted crop, through the reader and
/// the writer the app actually calls — including the `{:.6}` `CropAngle`
/// that replaced `{:.1}` (`P3-cropangle-model.md` §6.4: importing
/// `P08` and saving it back emitted `-3.3`, a 0.0256° drift = 4.3 px
/// of edge-to-edge tilt across a 9504 px frame).
///
/// MUTATION THIS CATCHES: put `{:.1}` back and the angle assertion fails
/// by the exact drift the report measured; drop the negation at either end
/// and the round trip returns `+3.274380`.
#[test]
fn a_tilted_crop_survives_a_whole_document_round_trip() {
    let doc = in_frame(
        &lr_doc(""),
        9504,
        6336,
    )
    .replace(
        "crs:Version=\"15.5.1\"",
        "crs:Version=\"15.5.1\"\n   crs:HasCrop=\"True\"\n   crs:CropLeft=\"0.09115\"\n   \
             crs:CropTop=\"0.100566\"\n   crs:CropRight=\"1\"\n   crs:CropBottom=\"0.900897\"\n   \
             crs:CropAngle=\"-3.274380\"",
    );
    let r = xmp_to_recipe(&doc);
    assert!((r.straighten_deg - 3.27438).abs() < 1e-5, "{}", r.straighten_deg);
    let c = r.crop.expect("the tilted rectangle imports");
    let out = recipe_to_xmp_in_frame(&r, FrameAspect::from_size(9504.0, 6336.0)).0;
    assert!(out.contains("crs:CropAngle=\"-3.274380\""), "{out}");
    // …and the rectangle comes back within the clamp this composition
    // costs on this specimen (5.9 px of 9186 = 0.00064).
    let back = xmp_to_recipe(&out).crop.expect("and again");
    for (got, want) in [
        (back.left, c.left),
        (back.top, c.top),
        (back.right, c.right),
        (back.bottom, c.bottom),
    ] {
        assert!((got - want).abs() < 1e-5, "{got} != {want}");
    }
}

/// A tilted crop in a FOREIGN document that declares no frame cannot be
/// placed — and is dropped and disclosed rather than read as the
/// axis-aligned rectangle it is not. Ours is read verbatim, because that
/// is precisely what this writer's frameless arm emits.
///
/// MUTATION THIS CATCHES: read the foreign one verbatim too and the note
/// disappears while a rectangle appears out of corners that mean something
/// else.
#[test]
fn a_frameless_tilted_crop_is_dropped_for_a_foreign_document_and_kept_for_ours() {
    let crop = "crs:HasCrop=\"True\" crs:CropLeft=\"0.05\" crs:CropTop=\"0\" \
                    crs:CropRight=\"0.95\" crs:CropBottom=\"1\" crs:CropAngle=\"-1.5\"";
    let foreign = format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
             xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description \
             rdf:about=\"\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             {crop}/></rdf:RDF></x:xmpmeta>"
    );
    let r = xmp_to_recipe(&foreign);
    assert_eq!(r.crop, None, "a rectangle we cannot place is not invented");
    assert!((r.straighten_deg - 1.5).abs() < 1e-6, "the tilt needs no aspect and rides");
    assert!(
        crop_import_note(&foreign).is_some_and(|n| n.contains("could not be placed")),
        "{:?}",
        crop_import_note(&foreign)
    );
    // Ours: the writer's own frameless arm wrote the rectangle in the
    // straightened frame, so the reader takes it back.
    let mine = EditRecipe {
        crop: Some(Crop { left: 0.05, top: 0.0, right: 0.95, bottom: 1.0 }),
        straighten_deg: 1.5,
        ..Default::default()
    };
    let back = xmp_to_recipe(&recipe_to_xmp(&mine));
    assert_eq!(back.crop, mine.crop, "our own frameless round trip is lossless");
    assert!(crop_import_note(&recipe_to_xmp(&mine)).is_none());
}

/// The overshoot sentence counts PIXELS of the frame the straighten leaves
/// behind, on the axis the rectangle actually crossed. The decoder
/// measures the overshoot in units of the source height; the note used to
/// scale a per-axis fraction by the source's LONGER side, so a bottom-edge
/// overshoot on a 3:2 frame read 50 % too many pixels (and a width-axis
/// one was a fraction of the inscribed width scaled by the source width —
/// a different frame again).
///
/// The fixture is built through the writer: an engine crop whose bottom
/// edge sits 10 % of the straightened height BELOW that frame, folded into
/// Lightroom corners that all lie inside the source, so the reader clamps
/// it back by exactly that much.
///
/// MUTATION THIS CATCHES: `f.h` → `f.w` or `f.w.max(f.h)` in
/// `crop_import_note` (264 px), or the bare per-axis maximum back in
/// `lr_to_engine_crop` (200 px) — the sentence says 176.
#[test]
fn the_crop_overshoot_note_counts_pixels_of_the_straightened_frame() {
    let (w, h, straighten) = (3000.0, 2000.0, 5.0);
    let frame = FrameAspect::from_size(w, h);
    let engine = Crop { left: 0.2, top: 0.2, right: 0.8, bottom: 1.1 };
    let lr = engine_to_lr_crop(Some(&engine), straighten, frame).expect("corners");
    for v in [lr.left, lr.top, lr.right, lr.bottom] {
        assert!((0.0..=1.0).contains(&v), "premise: corners inside the source: {lr:?}");
    }
    let doc = in_frame(&lr_doc(""), 3000, 2000).replace(
        "crs:Version=\"15.5.1\"",
        &format!(
            "crs:Version=\"15.5.1\"\n   crs:HasCrop=\"True\"\n   crs:CropLeft=\"{}\"\n   \
                 crs:CropTop=\"{}\"\n   crs:CropRight=\"{}\"\n   crs:CropBottom=\"{}\"\n   \
                 crs:CropAngle=\"{}\"",
            lr_num(lr.left),
            lr_num(lr.top),
            lr_num(lr.right),
            lr_num(lr.bottom),
            lr_num(lr.angle_deg),
        ),
    );
    let c = xmp_to_recipe(&doc).crop.expect("the crop imports, clamped");
    assert!((c.bottom - 1.0).abs() < 1e-6, "premise: the bottom edge was clamped: {c:?}");
    // 0.1 of the straightened height, in pixels of the same scale.
    let (_, hi) = inscribed_norm(w / h, straighten);
    let want = format!("{:.0} px", 0.1 * hi * h);
    let note = crop_import_note(&doc).expect("an overshoot is disclosed");
    assert!(note.contains(&want), "want {want:?} in {note:?}");
}

/// R27 T3, the second half of `P5-cropped-mask-frame.md` §8's parser
/// lesson: `crs:RetouchAreas` carries its OWN `Mask/Ellipse` (and
/// `Mask/Paint`) components, which are healing brushes rather than local
/// adjustments — a flat `crs:What="Mask/…"` count over the packet
/// disagrees with the correction-scoped parse on **83 of 166** images in
/// the user's library. This reader has always scoped its scan to
/// `crs:MaskGroupBasedCorrections` (`mask_summary`); the point of this
/// test is that it STAYS scoped.
///
/// MUTATION THIS CATCHES: point `mask_summary` at the whole crs scope
/// instead of the correction block and the retouch ellipse arrives as a
/// phantom mask (or a phantom loss).
#[test]
fn a_retouch_area_is_not_counted_as_a_local_adjustment() {
    // RE-BASED for v1.5.0 F9. This fixture used to spell the ellipse
    // `crs:Top/Left/Bottom/Right`, which is `Mask/CircularGradient`'s
    // encoding and not this one: all 84 retouch ellipses in the reference
    // library write `crs:X/Y/SizeX/SizeY`. The old spelling did not make
    // the test wrong — it asserts that the block is not counted as a
    // correction, which holds either way — but it did mean the fixture was
    // a shape no Lightroom writes, and F9 now reads these attributes for
    // real, so it has to be the real one.
    let retouch = "  <crs:RetouchAreas>\n   <rdf:Seq>\n    <rdf:li>\n     \
             <rdf:Description crs:SpotType=\"heal\" crs:SourceState=\"sourceSetAutomatically\">\n\
             \x20    <crs:Masks>\n      <rdf:Seq>\n       <rdf:li crs:What=\"Mask/Ellipse\" \
             crs:MaskValue=\"1\" crs:X=\"0.15\" crs:Y=\"0.15\" crs:SizeX=\"0.05\" \
             crs:SizeY=\"0.05\"/>\n      </rdf:Seq>\n     </crs:Masks>\n     \
             </rdf:Description>\n    </rdf:li>\n   </rdf:Seq>\n  </crs:RetouchAreas>\n";
    let doc = lr_doc(&lr_correction("R", "", &lr_radial("0", "0")))
        .replace("  <crs:MaskGroupBasedCorrections>", &format!("{retouch}  <crs:MaskGroupBasedCorrections>"));
    assert!(doc.contains("Mask/Ellipse"), "the fixture must carry the retouch component");
    let r = xmp_to_recipe(&doc);
    assert_eq!(r.masks.len(), 1, "one correction, one mask — the retouch is not one");
    assert_eq!(unsupported_corrections(&doc), 0, "nor is it a LOSS");
    assert!(import_losses(&doc).is_empty(), "{:?}", import_losses(&doc));
    // …and the other half of the same scoping fact, which is new in F9:
    // it is not a mask, and it IS a retouch area. Before this batch the
    // block read as nothing at all.
    assert_eq!(r.retouch.len(), 1, "the retouch area itself imports");
}

/// v1.5.0 F9: `crs:RetouchAreas` reads back as what Lightroom wrote — the
/// three fills and the two geometries, in the two markup forms real files
/// use for them.
///
/// Every shape here is the measured one. An ellipse is an attribute-only
/// `<rdf:li/>` (84 of 84 in the reference library) and a brush is a nested
/// `<rdf:Description>` with its own `<crs:Dabs>` (39 of 39), so this
/// fixture exercises both of the parser's paths rather than the tidier one
/// twice. The counts behind the fill names: 5 `heal`, 99 `heal_patchmatch`
/// with no `crs:fill_method`, 17 with `fill_method="firefly"`.
///
/// MUTATION THIS CATCHES: read the areas from `crs_own_scope` (which drops
/// the whole element, so nothing imports); accept an unmeasured
/// `crs:SpotType`; default `crs:Feather` to 0 instead of this engine's own;
/// treat `crs:SourceX`/`crs:OffsetY` as one-of-two-is-enough.
#[test]
fn lightrooms_spot_removal_reads_back_with_its_fill_and_its_shape() {
    use crate::retouch::{RetouchShape, SpotOrigin};
    let ellipse = |x: &str, y: &str| {
        format!(
            "<crs:Masks><rdf:Seq><rdf:li crs:What=\"Mask/Ellipse\" \
                 crs:MaskActive=\"true\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" \
                 crs:MaskValue=\"1\" crs:X=\"{x}\" crs:Y=\"{y}\" crs:SizeX=\"0.05\" \
                 crs:SizeY=\"0.05\" crs:Alpha=\"0\" crs:CenterValue=\"1\" \
                 crs:PerimeterValue=\"0\"/></rdf:Seq></crs:Masks>"
        )
    };
    let paint = "<crs:Masks><rdf:Seq><rdf:Description crs:What=\"Mask/Paint\" \
             crs:MaskActive=\"true\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" \
             crs:MaskSyncID=\"0000000000000000000000000000000A\" crs:MaskValue=\"1\" \
             crs:Radius=\"0.08\" crs:Flow=\"1\" crs:CenterWeight=\"0.5\">\
             <crs:Dabs><rdf:Seq><rdf:li>d 0.700000 0.300000</rdf:li></rdf:Seq></crs:Dabs>\
             </rdf:Description></rdf:Seq></crs:Masks>";
    let area = |attrs: &str, body: &str| {
        format!("<rdf:li><rdf:Description {attrs}>{body}</rdf:Description></rdf:li>")
    };
    let block = format!(
        "  <crs:RetouchAreas><rdf:Seq>{}{}{}{}{}</rdf:Seq></crs:RetouchAreas>\n",
        area(
            "crs:SpotType=\"heal\" crs:SourceState=\"sourceAutoComputed\" \
                 crs:Method=\"gaussian\" crs:HealVersion=\"2\" crs:SourceX=\"0.6\" \
                 crs:OffsetY=\"0.4\" crs:Opacity=\"1\" crs:Feather=\"0.5\" crs:Seed=\"2\"",
            &ellipse("0.25", "0.5")
        ),
        area("crs:SpotType=\"heal_patchmatch\" crs:Opacity=\"1\"", &ellipse("0.4", "0.6")),
        area(
            "crs:SpotType=\"heal_patchmatch\" crs:fill_method=\"firefly\" \
                 crs:pm_clio_model_version=\"clio-erase-2.0#test\"",
            paint
        ),
        // HALF a donor, which is not a donor. No real file does this —
        // all 5 `heal` areas state both — so this is the malformed-input
        // guard, and the 17-mutation sweep is what said it needed a case:
        // relaxing the pattern to `(Some(x), y)` changed nothing that any
        // test could see.
        area(
            "crs:SpotType=\"heal\" crs:SourceX=\"0.6\" crs:Feather=\"0.5\"",
            &ellipse("0.7", "0.7"),
        ),
        // Refused: a spelling nothing has measured. Its geometry would
        // render perfectly well; the label under it would be a claim.
        area("crs:SpotType=\"some_future_adobe_fill\"", &ellipse("0.8", "0.8")),
    );
    let doc = lr_doc(&lr_correction("R", "", &lr_radial("0", "0")))
        .replace("  <crs:MaskGroupBasedCorrections>", &format!("{block}  <crs:MaskGroupBasedCorrections>"));
    let r = xmp_to_recipe(&doc);

    assert_eq!(r.retouch.len(), 4, "four measured fills import, the unmeasured one does not");
    assert_eq!(
        r.retouch.iter().map(|a| a.origin).collect::<Vec<_>>(),
        vec![
            SpotOrigin::LightroomHeal,
            SpotOrigin::LightroomContentAware,
            SpotOrigin::LightroomGenerative,
            SpotOrigin::LightroomHeal
        ],
        "the fill axis is read from SpotType crossed with fill_method"
    );
    // Both halves of the donor or neither: the area imports, its geometry
    // stands, and the half-stated donor is simply not one.
    assert_eq!(
        r.retouch[3].donor, None,
        "a donor with one coordinate is not a donor"
    );

    // The `heal` area is the only kind that states a donor and a feather.
    let heal = &r.retouch[0];
    assert_eq!(heal.donor, Some([0.6, 0.4]), "both halves of the donor or neither");
    assert_eq!(heal.feather, 0.5);
    match heal.shape {
        RetouchShape::Ellipse { cx, cy, size_x, size_y } => {
            assert_eq!((cx, cy, size_x, size_y), (0.25, 0.5, 0.05, 0.05));
        }
        ref other => panic!("an ellipse, got {other:?}"),
    }

    // A patchmatch area states neither, so the feather is this engine's
    // own — an ABSENT attribute is not a photographer choosing zero.
    let pm = &r.retouch[1];
    assert_eq!(pm.donor, None, "Adobe states a search window, not a donor");
    assert_eq!(pm.feather, crate::retouch::HealSpot::default().feather);

    // The generative one here is a BRUSH, which is the common pairing:
    // 14 of the library's 17 firefly areas are brushes, not ellipses.
    match r.retouch[2].shape {
        RetouchShape::Brush(ref strokes) => {
            assert_eq!(strokes.len(), 1);
            assert_eq!(strokes[0].radius, 0.08);
            assert_eq!(strokes[0].dabs, "d 0.700000 0.300000");
        }
        ref other => panic!("a brush, got {other:?}"),
    }

    // And none of it reached the local adjustments.
    assert_eq!(r.masks.len(), 1, "still one correction, one mask");
}

/// v1.5.0 F8: HDR edit mode and the seven SDR controls survive a round
/// trip, in Lightroom's own spellings.
///
/// Three spellings are pinned here because getting any of them wrong
/// produces a file Lightroom reads silently and WRONGLY:
///
/// * `crs:HDREditMode` is Lightroom's `"0"`/`"1"` and not its
///   `"True"`/`"False"` — both boolean spellings live in one sidecar, and
///   `"True"` here reads as OFF;
/// * `crs:HDRMaxValue` is a two-place decimal (`"+1.00"`), the
///   `SharpenRadius` shape, not an integer;
/// * the seven `crs:SDR*` are signed integers like every Basic slider.
///
/// Every read here goes through [`bare_document`], and that is the whole
/// difference between a test and a tautology. `recipe_to_xmp` embeds the
/// recipe as an `ash` PAYLOAD and `xmp_to_recipe` prefers it, so a round
/// trip through the public pair proves the JSON survived and says nothing
/// about the `crs:` attributes — relaxing the reader to `!= Some("0")`,
/// which turns HDR mode on for every photograph in the archive, was GREEN
/// against the payload form (falsification case F8-M10).
///
/// MUTATION: write `"True"` for the mode; drop the `+.2` from the
/// headroom; read the mode with `!= Some("0")` (an absent key turns HDR
/// mode ON for every photograph in the archive).
#[test]
fn hdr_edit_mode_and_its_sdr_rendition_round_trip_in_lightrooms_spellings() {
    let r = EditRecipe {
        hdr_edit: true,
        hdr_max_ev: 1.5,
        sdr_blend: -30.0,
        sdr_brightness: 40.0,
        sdr_contrast: 15.0,
        sdr_highlights: -50.0,
        sdr_shadows: 25.0,
        sdr_whites: -20.0,
        sdr_clarity: 10.0,
        ..Default::default()
    };
    let doc = recipe_to_xmp(&r);
    for want in [
        "crs:HDREditMode=\"1\"",
        "crs:HDRMaxValue=\"+1.50\"",
        "crs:SDRBlend=\"-30\"",
        "crs:SDRBrightness=\"+40\"",
        "crs:SDRContrast=\"+15\"",
        "crs:SDRHighlights=\"-50\"",
        "crs:SDRShadows=\"+25\"",
        "crs:SDRWhites=\"-20\"",
        "crs:SDRClarity=\"+10\"",
    ] {
        assert!(doc.contains(want), "missing {want} in\n{doc}");
    }
    // Read back from the PAYLOAD-FREE projection, so what comes home is
    // the nine attributes above and not the embedded JSON.
    let back = xmp_to_recipe(&bare_document(&r, None));
    assert!(back.hdr_edit, "the mode comes home");
    assert_eq!(back.hdr_max_ev, 1.5, "…and the headroom, to its two places");
    assert_eq!(back.sdr_controls(), r.sdr_controls(), "…and all seven controls");

    // The mode is read STRICTLY: absent is off, "0" is off, and so is
    // Lightroom's other boolean spelling, which this key never uses.
    let off = EditRecipe::default();
    let bare = bare_document(&off, None);
    assert!(!bare.contains("HDREditMode") && !bare.contains("SDR"), "{bare}");
    assert!(!xmp_to_recipe(&bare).hdr_edit, "an absent key is not HDR mode");
    let inject = |spelling: &str| {
        bare.replace("crs:Version=", &format!("crs:HDREditMode=\"{spelling}\" crs:Version="))
    };
    // THE POSITIVE CONTROL, and the reason it is here: without it the four
    // negatives below are satisfied by an injection that never reached the
    // reader at all, and relaxing the read to `!= Some("0")` — which turns
    // HDR mode ON for every photograph in the archive — stayed green
    // (falsification case F8-M10). Prove the door opens before testing
    // that it is shut.
    assert!(
        xmp_to_recipe(&inject("1")).hdr_edit,
        "premise: an injected crs:HDREditMode must reach the reader"
    );
    for spelling in ["0", "True", "true", ""] {
        assert!(
            !xmp_to_recipe(&inject(spelling)).hdr_edit,
            "crs:HDREditMode=\"{spelling}\" is not HDR mode"
        );
    }
}

/// The same nine keys read from LIGHTROOM's bytes:
/// `src/fixtures/hdr-on-lightroom-9.4.xmp`, which Lightroom 9.4 (Camera
/// Raw 18.4) wrote on 2026-09-19 with the mode on.
///
/// The round trip above proves this writer and this reader agree with
/// each other, which is a weaker claim than agreeing with Lightroom, and
/// no library file can make the stronger one: the reference library's
/// 175 sidecars carry `crs:HDREditMode="0"` on 114 photographs and `"1"`
/// on NONE (census 2026-09-17, [`crate::render::hdr`]). This one file is
/// also the measurement's own input --- the headroom shoulder in
/// `render::hdr` was fitted against its `+2.30` (rms 0.0412 of
/// Lightroom's own transfer), so a tree without it cannot re-derive that
/// number. It is 8,165 bytes and names no file, place or person; what it
/// carries beyond the develop is the camera and the lens.
///
/// MUTATION THIS CATCHES: read the headroom with a parser that stops at
/// the sign (`+2.30` -> 0.0, a photograph with no headroom at all), or
/// let an SDR control fall back to its default instead of reading the
/// zero Lightroom wrote.
#[test]
fn lightrooms_own_hdr_sidecar_reads_as_the_mode_and_headroom_it_states() {
    let doc = include_str!("../../fixtures/hdr-on-lightroom-9.4.xmp");
    // The premise, and what makes this test different from the one
    // above: Lightroom wrote this file, so there is no payload of ours
    // to read the develop out of. Everything below is `crs:` attributes.
    assert!(
        !doc.contains("xmlns:asr"),
        "a Lightroom-written sidecar carries no payload of ours"
    );
    assert!(
        doc.contains("crs:HDREditMode=\"1\""),
        "premise: the mode is on in the bytes themselves"
    );

    let r = xmp_to_recipe(doc);
    assert!(r.hdr_edit, "the mode Lightroom wrote comes home");
    assert_eq!(r.hdr_max_ev, 2.30, "...and the headroom, sign and two places");
    assert_eq!(
        r.sdr_controls(),
        [0.0; 7],
        "...and all seven SDR controls, at the zero Lightroom wrote"
    );
}

/// R27 T4, `P2-feather-k-closures.md` §4.3. Lightroom's rule for the
/// detail/NR companions is **amount-gated**: 4/4 exports with
/// `LuminanceSmoothing > 0` carry the Detail companion and 0 of 207 with
/// it at 0 do; `SharpenEdgeMasking="0"` rides on 159 files whose
/// `Sharpness` is set. This writer gated each companion on its OWN value,
/// so `P33.JPG`'s shape — `LuminanceSmoothing="50"`,
/// `…Detail="50"`, `…Contrast="0"` — came out with the Contrast key
/// missing, which no Lightroom file has.
///
/// Behaviourally neutral by construction: only the companions whose ACR
/// default is ZERO join the rule (see `amount_carries`), so an emitted
/// `"0"` says exactly what its absence said.
///
/// MUTATION THIS CATCHES: revert `amount_carries` to `false` and the two
/// zero-valued companions vanish from a document whose amounts are set;
/// widen it to every companion and `SharpenRadius="+0.0"` starts asserting
/// a radius Lightroom would render at.
#[test]
fn the_detail_companions_ride_on_their_amount_the_way_lightroom_writes_them() {
    // `P33.JPG`, verbatim (P2 §4.1).
    let r = EditRecipe {
        noise_reduction: 50.0,
        nr_detail: 50.0,
        nr_contrast: 0.0,
        color_nr: 100.0,
        color_nr_detail: 50.0,
        color_nr_smooth: 50.0,
        sharpening: 0.0,
        ..Default::default()
    };
    let x = recipe_to_xmp(&r);
    for want in [
        "crs:LuminanceSmoothing=\"50\"",
        "crs:LuminanceNoiseReductionDetail=\"50\"",
        "crs:LuminanceNoiseReductionContrast=\"0\"",
        "crs:ColorNoiseReduction=\"100\"",
        "crs:ColorNoiseReductionDetail=\"50\"",
        "crs:ColorNoiseReductionSmoothness=\"50\"",
    ] {
        assert!(x.contains(want), "the LR-shaped NR block is missing {want}: {x}");
    }
    assert!(!x.contains("SharpenEdgeMasking"), "no sharpening amount, no companion: {x}");
    assert!(!x.contains("SharpenRadius"), "and never an invented radius: {x}");
    // Sharpening set, masking at rest: Lightroom writes the zero.
    let sharp = EditRecipe { sharpening: 45.0, ..Default::default() };
    let x = recipe_to_xmp(&sharp);
    assert!(x.contains("crs:SharpenEdgeMasking=\"0\""), "{x}");
    assert!(!x.contains("SharpenDetail"), "a 25-default companion stays absent: {x}");
    assert!(!x.contains("SharpenRadius"), "so does the 1.0-default radius: {x}");
    // A recipe with NO noise reduction keeps the whole block absent — the
    // half of the rule the old per-key gate got right.
    let none = recipe_to_xmp(&EditRecipe::default());
    assert!(!none.contains("LuminanceNoiseReductionContrast"), "{none}");
    assert!(!none.contains("SharpenEdgeMasking"), "{none}");
}

/// R27 A7. A PORTRAIT capture's `crs:` coordinates are fractions of the
/// UN-ROTATED SENSOR ARRAY, and the export is already upright
/// (`P1-portrait-mask-frame.md` §1, HIGH; 7/7 files pick their true frame
/// by `dSS`, four landscape controls recover the known answer).
///
/// The fixture is `P46-已增强-NR.JPG`'s Mask 8 (P1 §4.2), verbatim:
/// a single `Mask/CircularGradient` at `Angle="0"` declaring +1.6 EV, whose
/// `|b|` far exceeds the frame so it renders as a BAND — and the two
/// readings put that band on perpendicular axes, 12 628 px apart. P1
/// measures its fitted gain at **+1.647 ± 0.270** under the sensor frame
/// against a declared +1.60, and **−1.982** — the wrong SIGN — under the
/// display one.
///
/// The two numbers asserted here are P1 §4.2's own: the band sits about
/// display `y = 4748` with a half-extent of `4315` px along that axis.
/// Under the rejected reading it would be a VERTICAL band about
/// `x = 3171` with half-width `4453`.
///
/// MUTATION THIS CATCHES: drop the `orient_recipe_coords` call at the end
/// of `xmp_to_recipe` (or read `tiff:Orientation` as `Normal`) and the band
/// comes back vertical — the v0.33 defect P1 was written to close.
#[test]
fn a_portrait_captures_mask_is_decoded_in_the_sensor_frame() {
    // sensor 9504 × 6336, tiff:Orientation="8" → display 6336 × 9504.
    let doc = in_frame(
        &lr_doc(&lr_correction(
            "Mask 8",
            "",
            &lr_radial_at("-3.21675", "0.060424", "2.073933", "0.940417", "0"),
        )),
        9504,
        6336,
    )
    .replace("tiff:ImageWidth", "tiff:Orientation=\"8\"\n   tiff:ImageWidth");
    let r = xmp_to_recipe(&doc);
    let MaskGeometry::Radial { top, left, bottom, right, .. } = r.masks[0].mask else {
        panic!("a radial");
    };
    // The engine's frame is the DISPLAY one: 6336 × 9504.
    let (cy, ry) = (
        (top as f64 + bottom as f64) / 2.0 * 9504.0,
        (bottom as f64 - top as f64) / 2.0 * 9504.0,
    );
    assert!((cy - 4748.0).abs() < 1.5, "the band's display centre is y = {cy:.0}, not 4748");
    // P1 §4.2's own pixel number was 4315 — measured on the EXPORT, i.e.
    // the k-image (4315 = 1.032 × 4182). Since the 2026-08-19 ruling set
    // `LR_MASK_FRAME_SCALE = 1.0` the recipe carries the STORED extent,
    // 0.4399965 × 9504 = 4181.7; the residual vs the export is that
    // frame's lens-profile warp, unmodelled by decision (batch10 §7.5).
    assert!((ry - 4182.0).abs() < 1.5, "its display half-extent is {ry:.0}, not 4182");
    // …and the OTHER axis is the one that overflows the frame, which is
    // what makes it a band rather than an ellipse.
    let rx = (right as f64 - left as f64) / 2.0 * 6336.0;
    assert!(rx > 6336.0, "the perpendicular half-extent {rx:.0} must exceed the frame");
    // The writer un-turns it: the file's own four numbers come back.
    let out = recipe_to_xmp_in_frame(
        &r,
        FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::Rotate270),
    )
    .0;
    for (key, want) in [
        ("Top", "-3.21675"),
        ("Left", "0.060424"),
        ("Bottom", "2.073933"),
        ("Right", "0.940417"),
    ] {
        assert!(
            out.contains(&format!("crs:{key}=\"{want}\"")),
            "crs:{key} did not survive the portrait round trip: {out}"
        );
    }
}

/// R29 C1, the sidecar boundary — the brush twin of the radial test above.
///
/// A portrait capture's `crs:Dabs` are fractions of the UN-ROTATED SENSOR
/// array, exactly like its `crs:Top/Left`, so the reader has to turn them
/// into the display frame and the writer has to turn them back. Until this
/// batch neither happened: the stream was carried verbatim, which made the
/// round trip byte-exact and the RENDER wrong by a quarter turn — invisible
/// while the brush drew nothing (R27 Batch-4) and visible the moment it did
/// (R29 Batch-6b).
///
/// The rewrite is not free, and this test is where the cost is legible: the
/// stream that comes back out was COMPUTED, not copied. It survives here
/// because Lightroom writes six decimals and this writer re-emits six
/// (`render::LR_DAB_DECIMALS`), so on the pure rotations the decimal grid
/// is closed under the turn — `1 − 0.800000` is `0.200000` both ways, and
/// `0.582157 × 1.5 ÷ 1.5` lands back on `0.582157`. It is arithmetic that
/// happens to be exact, not a carry, and a frame whose aspect is not 3:2
/// would come home within a millionth instead of on it.
///
/// MUTATIONS THIS CATCHES: drop the frame argument at either boundary (the
/// import leaves the dabs sensor-frame, so the export turns them once and
/// they leave the file rotated); hand `in_source_frame` the SOURCE aspect
/// instead of `displayed()` (the radius comes back 1.31, i.e. 1.5² × the
/// error).
#[test]
fn a_portrait_captures_brush_turns_on_the_way_in_and_comes_home_on_the_way_out() {
    // sensor 9504 × 6336, tiff:Orientation="8" = Rotate270 → display
    // 6336 × 9504. `lr_paint` pins crs:Radius="0.582157" (the F2 specimen).
    let paint = lr_paint(
        "FA7459A9F5626F4881D7B730C3093F95",
        "1",
        "0",
        "false",
        &["r 0.200000", "d 0.100000 0.800000"],
    );
    let group = format!(
        "<rdf:li>\n\
             <rdf:Description crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\"\n\
             crs:MaskName=\"Brush 1\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\"\n\
             crs:MaskSyncID=\"0000000000000000000000000000000D\" crs:MaskValue=\"1\">\n\
             <crs:Masks>\n<rdf:Seq>\n{paint}</rdf:Seq>\n</crs:Masks>\n\
             </rdf:Description>\n</rdf:li>\n"
    );
    let doc = in_frame(&lr_doc(&lr_correction("Mask 7", "", &group)), 9504, 6336)
        .replace("tiff:ImageWidth", "tiff:Orientation=\"8\"\n   tiff:ImageWidth");
    let r = xmp_to_recipe(&doc);
    let stroke = |r: &EditRecipe| {
        let g = &r.masks[0].mask;
        let MaskGeometry::Brush { strokes, .. } = g else { panic!("a brush, got {g:?}") };
        (strokes[0].dabs.clone(), strokes[0].radius)
    };
    // IN: Rotate270 maps (u,v) -> (v, 1−u), and the source aspect 1.5
    // rescales every width-unit radius.
    let (dabs, radius) = stroke(&r);
    assert_eq!(dabs, "r 0.300000\nd 0.800000 0.900000", "the stream must reach the display frame");
    assert!((radius - 0.873236).abs() < 1e-6, "crs:Radius became {radius}, not 0.873236");

    // OUT: the inverse, against the DISPLAYED aspect — the file's own
    // digits come back.
    let out = recipe_to_xmp_in_frame(
        &r,
        FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::Rotate270),
    )
    .0;
    for want in [
        "<rdf:li>r 0.200000</rdf:li>",
        "<rdf:li>d 0.100000 0.800000</rdf:li>",
        "crs:Radius=\"0.582157\"",
    ] {
        assert!(out.contains(want), "{want} did not survive the portrait round trip: {out}");
    }
}

/// R27 A8, and it closes `C-rotation-skeleton.md`'s round-trip hole: a
/// document THIS writer produces now declares the frame its coordinates
/// are measured against, so re-importing one recovers the rotated radial
/// it wrote. Before R27 a fresh sidecar declared nothing, and the reader —
/// which needs `W/H` to fold a pixel-frame tilt into the engine's
/// normalised one — could not read our own file back.
///
/// MUTATION THIS CATCHES: return an empty string from `frame_declaration`
/// and the re-import loses the tilt (and says so through
/// `describe_import_losses`), which is exactly the hole.
#[test]
fn a_fresh_document_declares_its_frame_and_can_be_read_back() {
    let r = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.30, left: 0.20, bottom: 0.62, right: 0.75,
                feather: 0.5, roundness: 0.0, flipped: false, angle: 21.5,
                midpoint: 50.0, mask_version: 2,
            },
            exposure_ev: 0.4,
            ..Default::default()
        }],
        crop: Some(Crop { left: 0.08, top: 0.05, right: 0.9, bottom: 0.94 }),
        straighten_deg: 1.25,
        ..Default::default()
    };
    for turn in [rawler::Orientation::Normal, rawler::Orientation::Rotate270] {
        let frame = FrameAspect::from_size_turned(9504.0, 6336.0, turn);
        let doc = recipe_to_xmp_in_frame(&r, frame).0;
        assert!(doc.contains("xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\""), "{doc}");
        assert!(doc.contains("tiff:ImageWidth=\"9504\""), "{doc}");
        assert!(doc.contains("tiff:ImageLength=\"6336\""), "{doc}");
        assert!(
            doc.contains(&format!("tiff:Orientation=\"{}\"", turn.to_u16())),
            "the DISPLAYED orientation, quarter turns included: {doc}"
        );
        let back = xmp_to_recipe(&doc);
        // Compared as the ELLIPSE it draws, in the frame this engine
        // displays — `(box, angle)` is a redundant carrier (a quarter turn
        // of the box with the angle shifted 90° is the SAME ellipse), and
        // Lightroom's own ±45° canonicalisation legitimately picks the
        // other representative on the way through.
        let (dw, dh) = if crate::decode::orientation_transposes(turn) {
            (6336.0, 9504.0)
        } else {
            (9504.0, 6336.0)
        };
        let (a0, b0, t0) = engine_pixel_ellipse(&r.masks[0].mask, dw, dh);
        let (a1, b1, t1) = engine_pixel_ellipse(&back.masks[0].mask, dw, dh);
        assert!(
            (a0 - a1).abs() < 1.0 && (b0 - b1).abs() < 1.0,
            "{turn:?}: semi-axes {a0:.1}/{b0:.1} came back {a1:.1}/{b1:.1}"
        );
        assert!(
            ((t0 - t1).rem_euclid(180.0)).min((t1 - t0).rem_euclid(180.0)) < 1e-2,
            "{turn:?}: tilt {t0} came back as {t1}"
        );
        assert!((back.straighten_deg - 1.25).abs() < 1e-4, "{turn:?}: tilt");
        let c = back.crop.expect("the crop survives");
        assert!(
            (c.left - 0.08).abs() < 2e-3 && (c.bottom - 0.94).abs() < 2e-3,
            "{turn:?}: crop {c:?}"
        );
    }
}

/// **The import half of the sidecar orientation law** (W14), arm by arm.
/// The law itself, its measurement and its sign convention live on
/// [`crate::render::quarter_turns_between`]; this pins what THIS module
/// does with the answer.
///
/// MUTATION THIS CATCHES: hand `honour_declared_orientation` the declared
/// state as its own EXIF (`quarter_turns_between(want, want)`) and every
/// row that needs a turn comes back 0 — the defect the me6-2026-09 pack
/// measured, where 46 landscape exports rendered portrait.
#[test]
fn a_sidecars_declared_orientation_becomes_the_photographers_quarter_turns() {
    use rawler::Orientation as O;
    let turn = |declared, exif| honour_declared_orientation(declared, exif);
    // No declaration: the document says nothing about which way the photo
    // is up, so neither does the recipe. Today's behaviour, byte for byte.
    assert_eq!(
        turn(None, Some(O::Rotate270)),
        HonouredTurn { turn: O::Normal, quarter_turns: 0, refused: None }
    );
    // No photograph to compose against (a baked source, an unreadable
    // RAW): the declaration is the only frame information there is and is
    // taken at face value, which is what every build before W14 did.
    assert_eq!(
        turn(Some(O::Rotate270), None),
        HonouredTurn { turn: O::Rotate270, quarter_turns: 0, refused: None }
    );
    // The me6-2026-09 pack's own row: IFD0 Orientation = 8 under 46
    // sidecars that declare 1. One clockwise quarter turn delivers the
    // landscape frame Lightroom exported.
    assert_eq!(
        turn(Some(O::Normal), Some(O::Rotate270)),
        HonouredTurn { turn: O::Normal, quarter_turns: 1, refused: None }
    );
    // Agreement costs nothing — which is why the 169 sidecar/RAW pairs of
    // the reference library, all of which agree, import unchanged.
    for o in [O::Normal, O::Rotate90, O::Rotate180, O::Rotate270, O::Transpose] {
        assert_eq!(turn(Some(o), Some(o)).quarter_turns, 0, "{o:?} agrees with itself");
    }
    // A MIRROR over an un-mirrored capture has no quarter turn at all, so
    // the tag is refused WHOLE: the photograph's own EXIF decides the
    // frame, exactly as it does for a sidecar that declares nothing, and
    // the refusal is named rather than mapped to the nearest rotation.
    assert_eq!(
        turn(Some(O::HorizontalFlip), Some(O::Normal)),
        HonouredTurn { turn: O::Normal, quarter_turns: 0, refused: Some(O::HorizontalFlip) }
    );
    // …and a mirror over a MIRRORED capture is a plain rotation, so it is
    // honoured. "Mirrored values are refused" would have been the wrong
    // rule: what cannot be delivered is a change of HANDEDNESS.
    assert_eq!(
        turn(Some(O::VerticalFlip), Some(O::HorizontalFlip)),
        HonouredTurn { turn: O::VerticalFlip, quarter_turns: 2, refused: None }
    );
}

/// **The export half**: a merge corrects the base's `tiff:Orientation` when
/// — and only when — the photographer's turn has moved away from it.
///
/// Both halves matter and they pull against each other. A save that never
/// touched the tag could not carry a rotation made here to Lightroom; a
/// save that always rewrote it would move the base's own bytes on every
/// round trip. The composed state (`pipeline::photo_frame_aspect`) decides,
/// so a recipe imported from this very sidecar composes back to the value
/// already in it and nothing is written.
///
/// MUTATION THIS CATCHES: return `FrameDecl::Keep` unconditionally from
/// `merge_frame`'s disagreement arm and the turned save leaves
/// An orientation-only document — the shape 154 of this library's 175
/// sidecars have — is read in the PHOTOGRAPH's frame, which is the frame
/// its geometry was folded through on the way out. With the RAW beside
/// it, a rotated radial comes back rotated and a tilted crop placed; the
/// document-only readers still abstain (nothing to compose against), and
/// the photo-aware disclosure doors agree with the recipe reader instead
/// of reporting a rotation loss that did not happen.
///
/// MUTATION THIS CATCHES: drop the `frame_fallback` line from the reader
/// (the radial decodes `Unrotated`), or the `photo_frame_fallback` from
/// either door (the false loss note returns).
#[test]
fn an_orientation_only_sidecar_is_read_in_the_photographs_frame() {
    let Some(root) = crate::fit::calibration_corpus() else { return };
    let raw = root.join("p36.arw");
    if !raw.is_file() {
        crate::test_skipped("orientation-only sidecar", "p36.arw not in the corpus");
        return;
    }
    let ((w, h), exif) = crate::pipeline::source_frame_memo(&raw).expect("p36's header reads");
    let frame = FrameAspect::from_size_turned(w as f64, h as f64, exif).expect("a rectangle");
    let r = EditRecipe {
        crop: Some(crate::recipe::Crop { left: 0.1, top: 0.1, right: 0.9, bottom: 0.9 }),
        straighten_deg: 3.0,
        masks: vec![LocalAdjustment {
            // Clearly elongated (0.6 w by 0.2 h): a near-circular ellipse
            // reads back with either axis as the major one, and its
            // tilt then a quarter turn off, which says nothing about
            // the frame it was read in.
            mask: MaskGeometry::Radial {
                top: 0.3,
                left: 0.2,
                bottom: 0.5,
                right: 0.8,
                feather: 0.5,
                roundness: 0.0,
                flipped: false,
                angle: 37.0,
                midpoint: 50.0,
                mask_version: 2,
            },
            exposure_ev: 0.5,
            ..Default::default()
        }],
        ..Default::default()
    };
    // The projection through the photograph's frame with no payload to
    // restore from, then the frame block reduced to the orientation alone:
    // exactly what a merge into a Lightroom base that declares only
    // `tiff:Orientation` writes (`merge_frame`'s Keep arm).
    let full = bare_document(&r, Some(frame));
    let doc = full
        .lines()
        .filter(|l| !l.contains("tiff:ImageWidth=") && !l.contains("tiff:ImageLength="))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(doc.contains("tiff:Orientation=") && !doc.contains("tiff:ImageWidth"), "premise: {doc}");
    assert_eq!(FrameAspect::from_xmp(&doc), None, "premise: the document declares no rectangle");
    // The recipe reader, with the photograph: the rotation and the crop come home.
    let with_photo = xmp_to_recipe_for_photo(&doc, &raw);
    let MaskGeometry::Radial { angle, .. } = with_photo.masks[0].mask else { panic!("a radial") };
    assert!((angle - 37.0).abs() < 0.5, "the rotation is read in the photo's frame: {angle}");
    let crop = with_photo.crop.expect("the tilted crop is placed");
    assert!((crop.left - 0.1).abs() < 0.02 && (crop.right - 0.9).abs() < 0.02, "{crop:?}");
    // …and without it the reader abstains exactly as before.
    let MaskGeometry::Radial { angle: blind, .. } = xmp_to_recipe(&doc).masks[0].mask else { panic!() };
    assert_eq!(blind, 0.0, "no photograph, no frame, no rotation");
    // The disclosure doors agree with the reader they describe.
    assert!(
        import_losses(&doc).iter().any(|l| matches!(l.reason, MaskImportReason::Rotation(_))),
        "premise: the document-only door reports the rotation as lost"
    );
    assert!(
        !import_losses_for_photo(&doc, &raw).iter().any(|l| matches!(l.reason, MaskImportReason::Rotation(_))),
        "with the photograph the rotation imported, so it is not a loss"
    );
    // (The crop half of the same door is stated on Lightroom's own
    // documents by `the_census_has_no_false_rotation_loss_beside_a_readable_raw`:
    // an AutoShade-authored document places its rectangle through the
    // verbatim arm with no frame at all.)
    assert!(
        crop_import_note_for_photo(&doc, &raw).is_none_or(|n| !n.contains("could not be placed")),
        "with the photograph the rectangle is placed"
    );
}

/// The library census, read through both doors: on every orientation-only
/// sidecar beside a readable RAW the photo-aware door reports no rotation
/// loss the document-only door invented. Runs only where the census is
/// (`AUTOSHADE_CENSUS_ROOT`), and prints its counts for the ledger.
#[test]
fn the_census_has_no_false_rotation_loss_beside_a_readable_raw() {
    let Some(root) = std::env::var_os("AUTOSHADE_CENSUS_ROOT") else {
        crate::test_skipped("census rotation losses", "AUTOSHADE_CENSUS_ROOT unset");
        return;
    };
    let mut sidecars = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")) {
                sidecars.push(p);
            }
        }
    }
    let (mut orientation_only, mut with_raw, mut doc_rotation, mut photo_rotation, mut doc_crop, mut photo_crop) =
        (0, 0, 0, 0, 0, 0);
    for xmp in &sidecars {
        let Ok(text) = std::fs::read_to_string(xmp) else { continue };
        if declared_orientation(&text).is_none() || FrameAspect::from_xmp(&text).is_some() {
            continue;
        }
        orientation_only += 1;
        let Some(raw) = ["arw", "ARW", "dng", "DNG"]
            .iter()
            .map(|ext| xmp.with_extension(ext))
            .find(|p| p.is_file())
        else {
            continue;
        };
        if crate::pipeline::source_frame_memo(&raw).is_none() {
            continue;
        }
        with_raw += 1;
        let rot = |l: &[MaskImportLoss]| l.iter().any(|l| matches!(l.reason, MaskImportReason::Rotation(_)));
        doc_rotation += usize::from(rot(&import_losses(&text)));
        photo_rotation += usize::from(rot(&import_losses_for_photo(&text, &raw)));
        let unplaced = |n: Option<String>| n.is_some_and(|n| n.contains("could not be placed"));
        doc_crop += usize::from(unplaced(crop_import_note(&text)));
        photo_crop += usize::from(unplaced(crop_import_note_for_photo(&text, &raw)));
    }
    eprintln!(
        "CENSUS orientation-only {orientation_only} of {} sidecars, {with_raw} beside a readable RAW; \
             rotation losses document-only {doc_rotation} / photo-aware {photo_rotation}; \
             unplaced crops document-only {doc_crop} / photo-aware {photo_crop}",
        sidecars.len()
    );
    assert_eq!(photo_rotation, 0, "a rotation the photo-aware reader imports is not a loss");
    assert_eq!(photo_crop, 0, "a rectangle the photo-aware reader places is not unplaceable");
}

/// `tiff:Orientation="8"` on a document whose geometry is now landscape.
#[test]
fn a_merge_corrects_the_declared_orientation_only_when_the_turn_moved() {
    use rawler::Orientation as O;
    // A Lightroom-shaped base: the sensor rectangle plus a portrait turn.
    let base = in_frame(&lr_doc(""), 9504, 6336)
        .replace("tiff:ImageWidth", "tiff:Orientation=\"8\"\n   tiff:ImageWidth");
    let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
    let merged = |turn| {
        merge_recipe_into_xmp_in_frame(
            &base,
            &r,
            FrameAspect::from_size_turned(9504.0, 6336.0, turn),
        )
        .expect("the base is mergeable")
        .doc
    };
    // Unmoved: the base's own frame block survives verbatim, in its own
    // order, with its own whitespace.
    let same = merged(O::Rotate270);
    assert_eq!(same.matches("tiff:Orientation").count(), 1, "{same}");
    assert!(
        same.contains("tiff:Orientation=\"8\"\n   tiff:ImageWidth=\"9504\""),
        "the base's frame block did not survive byte for byte: {same}"
    );
    // Turned here: the declaration follows the composed state, exactly
    // once, and the rectangle it was measured against does not move.
    let turned = merged(O::Normal);
    assert_eq!(
        turned.matches("tiff:Orientation").count(),
        1,
        "one declaration, never two: {turned}"
    );
    assert!(turned.contains("tiff:Orientation=\"1\""), "{turned}");
    assert!(turned.contains("tiff:ImageWidth=\"9504\""), "the rectangle stays: {turned}");
    // …and a reader gets each one back out.
    assert_eq!(declared_orientation(&same), Some(O::Rotate270));
    assert_eq!(declared_orientation(&turned), Some(O::Normal));
    // Without a photograph there is nothing to compose, so the base's own
    // declaration stands — a `recipe_to_xmp`-style save of a document we
    // cannot place must not invent a frame for it.
    let blind = merge_recipe_into_xmp(&base, &r).expect("mergeable").doc;
    assert!(blind.contains("tiff:Orientation=\"8\""), "{blind}");
}

/// **A corrected orientation is written only where this engine's own
/// reader will find it, and arrives bound.** [`FrameScope::resolve`] takes
/// the declaration from the one Description that declares both
/// dimensions, so a correction spliced onto a DIFFERENT Description (the
/// exiftool shape: one Description per namespace) would be invisible to
/// the reader — the round trip would lose the turn while the coordinates
/// went out in a frame nobody recovers. And an attribute whose prefix no
/// enclosing element binds makes the WHOLE sidecar unreadable to an XML
/// parser, Lightroom's included. Three shapes, one rule each:
///
/// * the frame lives in another Description ⇒ the base's declaration is
///   kept, the geometry goes out in the frame the file declares, and the
///   merge NAMES what it could not write;
/// * the crs Description declares the frame and an ancestor binds the
///   prefix ⇒ the correction lands on the crs tag WITH its own binding;
/// * the crs Description binds the prefix itself ⇒ corrected, bound once.
///
/// MUTATION THIS CATCHES: drop the scope check from `merge_frame` and
/// the first shape writes a `tiff:Orientation` the reader never sees;
/// drop the binding from `orientation_declaration` and the second shape
/// carries `tiff:` with no binding in its own start tag.
#[test]
fn a_corrected_orientation_lands_where_the_reader_looks_and_arrives_bound() {
    use rawler::Orientation as O;
    let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
    let turned = |base: &str| {
        merge_recipe_into_xmp_in_frame(
            base,
            &r,
            FrameAspect::from_size_turned(9504.0, 6336.0, O::Rotate90),
        )
        .expect("mergeable")
    };
    let crs_tag = |doc: &str| {
        let start = find_crs_description(doc).expect("the settings Description survives");
        let (gt, _) = scan_tag_end(doc, start).expect("its start tag closes");
        doc[start..=gt].to_string()
    };

    // 1. The exiftool shape: the rectangle in property-element form in a
    //    Description of its own, the settings in Lightroom's attribute form.
    let apart = lr_doc("").replace(
        " <rdf:Description rdf:about=\"\"\n    xmlns:crs=",
        " <rdf:Description rdf:about=\"\"\n    xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\">\n\
             \x20  <tiff:ImageWidth>9504</tiff:ImageWidth>\n\
             \x20  <tiff:ImageLength>6336</tiff:ImageLength>\n\
             \x20 </rdf:Description>\n\
             \x20 <rdf:Description rdf:about=\"\"\n    xmlns:crs=",
    );
    assert!(apart.contains("<tiff:ImageWidth>9504</tiff:ImageWidth>"), "exiftool-shaped: {apart}");
    assert_eq!(declared_orientation(&apart), None);
    let out = turned(&apart);
    assert!(!out.doc.contains("tiff:Orientation"), "nothing the reader cannot see: {}", out.doc);
    assert_eq!(declared_orientation(&out.doc), None);
    assert!(out.doc.contains("<tiff:ImageWidth>9504</tiff:ImageWidth>"), "{}", out.doc);
    assert!(
        out.notes.iter().any(|n| n.contains("where a merge cannot rewrite it") && n.contains("(6)")),
        "the loss is named, with the turn that was not written: {:?}",
        out.notes
    );

    // 2. The frame on the crs Description, the prefix bound on an ancestor.
    let above = in_frame(&lr_doc(""), 9504, 6336).replace(
        "xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">",
        "xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n  xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\">",
    );
    assert_eq!(above.matches("xmlns:tiff=").count(), 1, "{above}");
    let out = turned(&above).doc;
    let tag = crs_tag(&out);
    assert_eq!(tag.matches("tiff:Orientation=\"6\"").count(), 1, "one correction: {tag}");
    assert_eq!(tag.matches("xmlns:tiff=").count(), 1, "bound where it is used: {tag}");
    assert_eq!(declared_orientation(&out), Some(O::Rotate90), "{out}");

    // 3. The Lightroom shape: the crs Description binds the prefix itself.
    let bound = in_frame(&lr_doc(""), 9504, 6336).replace(
        "tiff:ImageWidth",
        "xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n   tiff:ImageWidth",
    );
    let out = turned(&bound).doc;
    assert_eq!(out.matches("xmlns:tiff=").count(), 1, "bound once: {out}");
    assert_eq!(out.matches("tiff:Orientation=\"6\"").count(), 1, "{out}");
    assert_eq!(declared_orientation(&out), Some(O::Rotate90), "{out}");
}

/// The shape 154 of the reference library's 174 sidecars actually have: a
/// `tiff:Orientation` and NO `tiff:ImageWidth`/`tiff:ImageLength` at all,
/// 17 of them over a portrait capture and carrying mask geometry.
///
/// The declaration alone is enough to place the geometry — `orient_point`
/// is aspect-free, and only a brush's radii need the rectangle — so the
/// reader turns on it, and the writer un-turns on it. Before W14 the
/// reader required the rectangle too, so those masks stayed in the sensor
/// frame while the render delivered the turned one (a quarter turn off),
/// and the merge wrote DISPLAY-frame numbers back under a declaration
/// saying they were sensor-frame.
///
/// MUTATION THIS CATCHES: gate the `orient_recipe_coords` call on
/// `frame.is_some()` again and the box comes back where the file put it,
/// i.e. on the wrong axis of the delivered frame.
#[test]
fn an_orientation_declared_without_a_rectangle_still_places_the_mask() {
    use rawler::Orientation as O;
    let doc = lr_doc(&lr_correction(
        "Mask 8",
        "",
        &lr_radial_at("0.100000", "0.200000", "0.300000", "0.600000", "0"),
    ))
    .replace("crs:Version=\"15.5.1\"", "tiff:Orientation=\"8\"\n   crs:Version=\"15.5.1\"");
    assert_eq!(declared_orientation(&doc), Some(O::Rotate270));
    assert!(FrameAspect::from_xmp(&doc).is_none(), "this document declares no rectangle");
    let r = xmp_to_recipe(&doc);
    let MaskGeometry::Radial { top, left, bottom, right, .. } = r.masks[0].mask else {
        panic!("a radial");
    };
    // `Rotate270` maps (u, v) -> (v, 1 - u), so the stored box's x range
    // 0.20..0.60 becomes the y range 0.40..0.80 and its y range 0.10..0.30
    // becomes the x range 0.10..0.30.
    assert!((left - 0.10).abs() < 1e-6 && (right - 0.30).abs() < 1e-6, "x {left}..{right}");
    assert!((top - 0.40).abs() < 1e-6 && (bottom - 0.80).abs() < 1e-6, "y {top}..{bottom}");

    // …and the writer un-turns it against the photograph's own rectangle,
    // which is the only one there is. The numbers that leave are the
    // file's, not the display frame's, and the base's own declaration is
    // untouched because it already says what the geometry now means.
    let out = merge_recipe_into_xmp_in_frame(
        &doc,
        &r,
        FrameAspect::from_size_turned(9504.0, 6336.0, O::Rotate270),
    )
    .expect("mergeable")
    .doc;
    assert_eq!(out.matches("tiff:Orientation").count(), 1, "{out}");
    assert!(out.contains("tiff:Orientation=\"8\""), "{out}");
    assert!(out.contains("crs:Left=\"0.2"), "the stored box is source-frame again: {out}");
    // The round trip is the identity on the engine's own numbers.
    let back = xmp_to_recipe(&out);
    let MaskGeometry::Radial { top: t1, left: l1, bottom: b1, right: r1, .. } =
        back.masks[0].mask
    else {
        panic!("a radial");
    };
    assert!(
        (l1 - left).abs() < 1e-5
            && (r1 - right).abs() < 1e-5
            && (t1 - top).abs() < 1e-5
            && (b1 - bottom).abs() < 1e-5,
        "the round trip moved the box: {l1}..{r1} x {t1}..{b1}"
    );
}
