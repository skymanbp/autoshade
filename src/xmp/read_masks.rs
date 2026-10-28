//! Reading masks: the retouch areas, the mask summaries, unsupported corrections, and the per-correction and per-component import reasons.

use super::*;

/// Local-mask corrections back from `<crs:MaskGroupBasedCorrections>` —
/// exactly what [`masks_xml`] can emit, which since R27 Batch-4 is the
/// parametric geometries PLUS `Mask/Aggregate` brush groups. What still has no
/// classic-XMP encoding is our own `Bitmap` rasters (skipped by the writer, so
/// symmetric) and Lightroom's AI `Mask/Image` masks (skipped by this reader,
/// because the sidecar carries no pixels for them to be read from).
pub(super) fn parse_masks_with_source(
    xmp: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
    diag: Option<&crate::diag::Diag<'_>>,
) -> Vec<LocalAdjustment> {
    // Err (present-but-unterminated) imports no masks — the LOSS half of that
    // outcome is `mask_summary`'s to report, and it does.
    let Ok(Some(block)) = owned_element_body(xmp, "crs:MaskGroupBasedCorrections") else {
        return Vec::new();
    };
    let mut brush_reader = MaskBrushReader::new(photo, diag);
    mask_summary_from_block(block, authored_by_autoshade, intent_namespace_declared(xmp), frame, &mut brush_reader)
        .supported
}

/// Lightroom's spot removal, back from `<crs:RetouchAreas>` — v1.5.0 F9.
///
/// Read from the WHOLE document rather than from [`crs_own_scope`], and the
/// reason is the scope's own rule: it drops every top-level child that nests
/// an `rdf:Description`, which is exactly what this element is. That rule is
/// right for SETTINGS (a nested Description is somebody else's slider values)
/// and this element is the second one it is wrong about — `crs:RetouchAreas`
/// nests Descriptions because its AREAS are Descriptions, the same shape
/// `crs:MaskGroupBasedCorrections` is kept by name for. Reading the document
/// here rather than widening the scope keeps that rule untouched for the
/// eighty-odd scalars that depend on it.
///
/// **The geometry is the authority, not `crs:pm_target_*`.** The two agree —
/// the target rectangle's centre, normalised by `crs:pm_whole_image_*`, lands
/// within 3-4 decimals of the ellipse's own `crs:X`/`crs:Y` on every area that
/// carries both. The ellipse wins because `pm_whole_image` starts at (32, 20)
/// rather than at the origin: it is the model's own padded working area, not
/// the photograph.
pub(super) fn parse_retouch_areas(xmp: &str) -> Vec<crate::retouch::RetouchArea> {
    use crate::retouch::{RetouchArea, RetouchShape, SpotOrigin};

    /// More than an order of magnitude past the largest real file (21 areas)
    /// and still a bound — a hand-written sidecar must not be able to make one
    /// photograph cost an unbounded allocation. The same law as
    /// `MAX_MASKS_FROM_XMP` and `MAX_DAB_TOKENS`.
    const MAX_AREAS: usize = 512;
    const DESCRIPTION_CLOSE: &str = "</rdf:Description>";

    let Ok(Some(block)) = owned_element_body(xmp, "crs:RetouchAreas") else {
        return Vec::new();
    };
    let mut out: Vec<RetouchArea> = Vec::new();
    let mut at = 0usize;
    while let Some((start, gt, self_closing)) = next_xml_tag(block, at) {
        at = gt + 1;
        let tag = &block[start..=gt];
        // An AREA is the `rdf:Description` that carries `crs:SpotType` — 121
        // of 121 do. The Descriptions NESTED inside one (a `Mask/Paint`
        // component) carry none, so this single attribute is the whole test
        // and there is no nesting depth to count.
        if tag.starts_with("</") || tag_name(tag) != "rdf:Description" {
            continue;
        }
        let head = Tag::new(tag);
        let Some(spot_type) = head.crs_str("SpotType") else { continue };
        if out.len() >= MAX_AREAS {
            break;
        }
        // The FILL axis. An unmeasured `crs:SpotType` is refused rather than
        // guessed at: its geometry would render perfectly well, but the label
        // this engine puts under it would be a claim about what Adobe did to
        // those pixels, and there is no measurement behind it.
        let fill = head.crs_str("fill_method");
        let origin = match (spot_type.as_ref(), fill.as_deref()) {
            ("heal", _) => SpotOrigin::LightroomHeal,
            ("heal_patchmatch", Some("firefly")) => SpotOrigin::LightroomGenerative,
            ("heal_patchmatch", None) => SpotOrigin::LightroomContentAware,
            _ => continue,
        };
        if self_closing {
            continue; // no body, so no component, so no geometry to stand on
        }
        let Some(close) = find_matching_close(block, gt + 1) else { continue };
        let seg = &block[start..close + DESCRIPTION_CLOSE.len()];

        // One walk, both shapes. `components_in` is the reader the mask side
        // already uses, so an ellipse written as a bare `<rdf:li crs:What=…/>`
        // (84 of 84) and a Paint written as a nested Description (39 of 39)
        // arrive through one implementation rather than two.
        let comps = components_in(seg);
        let shape = if let Some(e) = comps.iter().find(|c| c.what == "Mask/Ellipse") {
            let t = Tag::new(e.tag);
            let num = |k: &str| t.crs_f32(k).filter(|v| v.is_finite());
            let (Some(cx), Some(cy), Some(size_x), Some(size_y)) =
                (num("X"), num("Y"), num("SizeX"), num("SizeY"))
            else {
                continue; // an ellipse that does not state where it is
            };
            RetouchShape::Ellipse { cx, cy, size_x, size_y }
        } else {
            let strokes: Vec<_> = comps
                .iter()
                .filter(|c| c.what == "Mask/Paint")
                .filter_map(|c| parse_paint_stroke(seg, c).ok())
                .collect();
            if strokes.is_empty() {
                continue; // neither shape: nothing to remove and nowhere to do it
            }
            RetouchShape::Brush(strokes)
        };

        // `crs:Feather` is stated by the 5 `heal` areas (0.5 on every one) and
        // by none of the 116 patchmatch ones. An ABSENT attribute is not a
        // photographer choosing zero, and a hard-edged patch seam is not what
        // Lightroom showed — so the fallback is this engine's own default.
        let feather = head
            .crs_f32("Feather")
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .unwrap_or_else(|| crate::retouch::HealSpot::default().feather);
        // Both halves or neither: a donor with one coordinate is not a donor.
        let donor = match (head.crs_f32("SourceX"), head.crs_f32("OffsetY")) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some([x, y]),
            _ => None,
        };
        out.push(RetouchArea { origin, feather, donor, shape });
    }
    out
}

/// How many corrections in this sidecar produced NO mask at all — AI / depth
/// geometry Lightroom recomputes from its own model, plus the ones whose values
/// land outside this engine's model. What was missing before R25 P1 was the
/// DISCLOSURE: a user importing their own Lightroom work lost every one of
/// these with no indication anything had been dropped.
///
/// BRUSH corrections stopped being counted here in R27 Batch-4: a
/// `Mask/Aggregate` group imports, so it is no longer a refusal at all. It
/// carries a `BrushRendered` note instead, which [`import_losses`] reports and
/// this counter deliberately does not (see below).
///
/// COUNTS DROPS ONLY, not every import defect (R25 P1) — a correction that
/// imports carrying a note is not a refusal, and `eval` reads
/// `imported + refused` as the size of the user's local work. The named list
/// of everything, notes included, is [`import_losses`].
pub fn unsupported_corrections(xmp: &str) -> usize {
    // Gated like the reader and [`import_losses`]: a document refused whole
    // for its namespace binding has no per-correction drops to count.
    if xmp.len() > MAX_XMP_BYTES || xmlns_conflict(xmp).is_some() {
        return 0;
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    mask_summary(scope.as_ref(), authored_by_autoshade, FrameAspect::from_xmp(xmp)).dropped
}

pub fn unsupported_corrections_for_photo(xmp: &str, photo: &std::path::Path) -> usize {
    if xmp.len() > MAX_XMP_BYTES || xmlns_conflict(xmp).is_some() {
        return 0;
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    mask_summary_with_source(
        scope.as_ref(),
        authored_by_autoshade,
        FrameAspect::from_xmp(xmp),
        Some(photo),
        None,
    )
    .dropped
}

/// One correction's verdict. R25 P1 retired the third state ("Partial", which
/// meant DISCARDED): a correction we can read the geometry of imports, and
/// whatever it carried that we do not model rides along as a named reason.
pub(super) enum MaskCorrectionParse {
    /// BOXED, and not for style: `LocalAdjustment` passed 320 bytes when R25
    /// P6 gave every mask four point-curve vectors, while the refusal arm is
    /// one small enum — `clippy::large_enum_variant` refuses that spread, and
    /// the box is the indirection it asks for. Only `mask_summary_from_block`
    /// destructures this, and it moves the adjustment straight into a `Vec`.
    Supported(Box<LocalAdjustment>, Vec<MaskImportReason>),
    /// The one verdict that costs the whole correction — always a
    /// [`MaskImportReason::is_drop`] reason.
    Unsupported(MaskImportReason),
}

#[derive(Default)]
pub(super) struct MaskSummary {
    supported: Vec<LocalAdjustment>,
    /// Every defect, NAMED — the one list every disclosure surface iterates.
    /// CAPPED: a document's corrections are unbounded and a disclosure is a
    /// sentence, not a log. The two counters beside it are exact.
    pub(super) losses: Vec<MaskImportLoss>,
    /// Corrections that produced no mask, EXACTLY (past the display cap too):
    /// [`unsupported_corrections`]' answer.
    dropped: usize,
    /// Every defect, exactly — `losses.len()` before the cap. Also the
    /// answer to "was this import LOSSY", which a separate `preserve_original`
    /// boolean used to carry: it was set on exactly this condition, so the two
    /// could only ever agree, and the merge keyed on the boolean until R25 P8
    /// found it was standing in for a different question entirely (see
    /// `corrections` below). One fact, one field.
    pub(super) defects: usize,
    /// How many `crs:What="Correction"` entries the BASE document's mask block
    /// holds, whatever became of them. The merge's preserve arm and its
    /// disclosure both key off THIS — "does the photographer have a mask block
    /// here", which is what the old boolean was mistaken for; see
    /// [`merge_recipe_into_xmp`] for how the two came apart in R25 P1. A block
    /// that opens and never closes counts as one: there IS a block, we simply
    /// cannot count what is in it.
    pub(super) corrections: usize,
}

impl MaskSummary {
    /// The ONE door a defect enters by, so the list and the two counters
    /// cannot drift: named, and counted — drop or note.
    ///
    /// It used to raise a `preserve_original` flag here as well, which the
    /// merge read as "keep the base's own mask block". That was true only
    /// while a Lightroom block ALWAYS produced a defect; R25 P1 made those
    /// blocks import cleanly and the flag started answering "no block to
    /// keep" for a file full of them. The merge asks its own question now
    /// (`MaskSummary::corrections`), and a defect is just a defect.
    fn record(&mut self, name: &str, reason: MaskImportReason) {
        /// A sentence, not a log.
        const MAX_IMPORT_LOSSES: usize = 256;
        /// A correction name is untrusted text straight out of the sidecar
        /// (the recipe's own names are capped by `EditRecipe::clamp`; these
        /// never went through it). Truncated on a char boundary by `take`.
        const MAX_NAME_CHARS: usize = 64;
        if reason.is_drop() {
            self.dropped = self.dropped.saturating_add(1);
        }
        self.defects = self.defects.saturating_add(1);
        if self.losses.len() < MAX_IMPORT_LOSSES {
            self.losses.push(MaskImportLoss {
                name: name.chars().take(MAX_NAME_CHARS).collect(),
                reason,
            });
        }
    }
}

pub(super) fn mask_summary(
    xmp: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
) -> MaskSummary {
    mask_summary_with_source(xmp, authored_by_autoshade, frame, None, None)
}

pub(super) fn mask_summary_with_source(
    xmp: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
    diag: Option<&crate::diag::Diag<'_>>,
) -> MaskSummary {
    match owned_element_body(xmp, "crs:MaskGroupBasedCorrections") {
        Ok(Some(block)) => {
            let mut brush_reader = MaskBrushReader::new(photo, diag);
            mask_summary_from_block(
                block,
                authored_by_autoshade,
                intent_namespace_declared(xmp),
                frame,
                &mut brush_reader,
            )
        }
        Ok(None) => MaskSummary::default(),
        // The group OPENS but never closes: whatever corrections it holds
        // cannot be counted, so the one honest summary is "a loss, and there
        // is a block here" — the old literal finder reported this exact
        // document as zero losses and no block at all, which both hid the
        // drop from the GUI toast and told the merge it was free to delete
        // the block from the user's own sidecar.
        Err(()) => {
            let mut summary = MaskSummary::default();
            summary.record("Correction 1", MaskImportReason::OutOfModel);
            // There IS a block — that is exactly what we just failed to read
            // the end of — so the merge must keep the user's bytes rather
            // than replace an unreadable block with nothing.
            summary.corrections = 1;
            summary
        }
    }
}

/// The label this correction wears in every disclosure: its own
/// `crs:CorrectionName`, else its position in the block. Blank names fall to
/// the positional form — an empty slot in a comma list reads as a bug.
///
/// `own` is the correction's OWN scope ([`correction_own_scope`]), not the
/// whole segment: a nested component carrying a `crs:CorrectionName` would
/// otherwise be able to name the correction it sits inside.
fn correction_name(own: Scope<'_>, position: usize) -> String {
    own.crs_str("CorrectionName")
        .map(|v| v.into_owned())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("Correction {position}"))
}

fn mask_summary_from_block(
    block: &str,
    authored_by_autoshade: bool,
    intent_declared: bool,
    frame: Option<FrameAspect>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> MaskSummary {
    const MAX_MASKS_FROM_XMP: usize = 64;
    const DESCRIPTION_CLOSE: &str = "</rdf:Description>";

    let mut summary = MaskSummary::default();
    let mut at = 0;
    let mut seen = 0usize;
    while let Some((start, gt, self_closing)) = next_xml_tag(block, at) {
        let tag = &block[start..=gt];
        let correction = tag_name(tag) == "rdf:Description"
            && !tag.starts_with("</")
            && xml_attribute_raw(tag, "crs:What")
                .is_some_and(|(_, raw)| xml_unescape(raw).as_ref() == "Correction");
        if !correction {
            at = gt + 1;
            continue;
        }

        seen += 1;
        if self_closing {
            // No body at all: nothing to read a geometry out of.
            summary.record(&format!("Correction {seen}"), MaskImportReason::Unrepresentable);
            at = gt + 1;
            continue;
        }
        let Some(close) = find_matching_close(block, gt + 1) else {
            summary.record(&format!("Correction {seen}"), MaskImportReason::OutOfModel);
            break;
        };
        let end = close + DESCRIPTION_CLOSE.len();
        let seg = &block[start..end];
        // Built ONCE per correction and handed to both readers below, so the
        // label a disclosure prints and the values the import takes cannot come
        // from two different scopes (R28 Batch-5 5d).
        let own = correction_own_scope(seg);
        let own = Scope::new(own.as_ref());
        let name = correction_name(own, seen);
        match classify_correction(seg, own, authored_by_autoshade, intent_declared, frame, brush_reader) {
            MaskCorrectionParse::Supported(mask, reasons)
                if summary.supported.len() < MAX_MASKS_FROM_XMP =>
            {
                summary.supported.push(*mask);
                for reason in reasons {
                    summary.record(&name, reason);
                }
            }
            // Past the recipe's own mask cap the correction does not import,
            // which is a drop however well it parsed.
            MaskCorrectionParse::Supported(..) => {
                summary.record(&name, MaskImportReason::OutOfModel);
            }
            MaskCorrectionParse::Unsupported(reason) => summary.record(&name, reason),
        }
        at = end;
    }
    if seen == 0 && Scope::new(block).find_value_at("What", "Correction").is_some() {
        summary.record("Correction 1", MaskImportReason::OutOfModel);
        // An attribute-form group is a group: the scanner above walked past
        // it, but the document really does carry corrections and the merge's
        // preserve arm has to know.
        seen = 1;
    }
    // The fact the merge keys off, counted the same way every disclosure in
    // this module is: by the ONE pass that read the block.
    summary.corrections = seen;
    summary
}

/// Generic over the SOURCE (R28 Batch-5 5d): the correction-wide gates ask a
/// [`Scope`] and the per-component gates ask a [`Tag`], and both spellings of
/// "absent is fine, present must be in band" are the same three lines. Being
/// generic is also what stops the two from drifting into two copies with two
/// different scope habits — the shape the untyped `&str` version had.
fn optional_scaled_number_in<'a>(
    src: impl CrsSource<'a>,
    key: &str,
    scale: f32,
    lo: f32,
    hi: f32,
) -> bool {
    match src.crs_str(key) {
        None => true,
        Some(_) => src
            .crs_f32(key)
            .map(|v| v * scale)
            .is_some_and(|v| (lo..=hi).contains(&v)),
    }
}

fn optional_number_is<'a>(src: impl CrsSource<'a>, key: &str, expected: f32) -> bool {
    match src.crs_str(key) {
        None => true,
        Some(_) => src.crs_f32(key).is_some_and(|v| (v - expected).abs() <= 1e-6),
    }
}

/// The correction's own `crs:Local*` settings. `Err` refuses the WHOLE
/// correction; `Ok` imports it carrying one note per knob we do not model.
///
/// The line between the two (R25 P1): a value we can READ but that lands
/// outside the model is a refusal — importing it would render something the
/// file does not say. A knob we simply have no model FOR is not: dropping the
/// user's whole mask because it also carries a Moiré slider loses far more
/// than it protects, and the note says exactly what did not come through.
///
/// `own` is the correction's OWN scope (R28 Batch-5 5d, adjudication F4
/// symptom A) — the correction tag plus its property children, with the
/// component list cut out. It used to be the whole segment, so a nested
/// component carrying any of the names below answered for the correction
/// whenever the correction itself omitted the key: an unknown
/// `crs:LocalExposure2012` on a `Mask/Paint` was read as the correction's
/// exposure, and the very same scan then reported the correction CLEAN, because
/// the unknown-key walk below (correctly) looks only at the correction's own
/// tag. Gate and reader now share one scope, which is the invariant that was
/// missing.
pub(super) fn correction_value_reasons(own: Scope<'_>) -> Result<Vec<MaskImportReason>, MaskImportReason> {
    const KNOWN_LOCAL: [&str; 28] = [
        "LocalExposure",
        "LocalHue",
        "LocalSaturation",
        "LocalContrast",
        "LocalClarity",
        "LocalSharpness",
        "LocalBrightness",
        "LocalToningHue",
        "LocalToningSaturation",
        "LocalExposure2012",
        "LocalContrast2012",
        "LocalHighlights2012",
        "LocalShadows2012",
        "LocalWhites2012",
        "LocalBlacks2012",
        "LocalClarity2012",
        "LocalDehaze",
        "LocalLuminanceNoise",
        "LocalMoire",
        "LocalDefringe",
        "LocalTemperature",
        "LocalTint",
        "LocalTexture",
        "LocalGrain",
        "LocalCurveRefineSaturation",
        // Lightroom BOOKKEEPING, not sliders — they carry no user intent, so
        // the `UnknownLocalKey` note they used to raise printed "unmodelled
        // slider" about something that is not one, on 12 of the reference
        // set's 31 importable corrections (R25 P1 round-end). Measured in the
        // user's own library: `LocalCorrectedDepth` appears 23 times and is
        // "0" every time (a numeric flag), and the two Digest keys are
        // Lightroom's own recompute ledger — a 32-hex string plus its schema
        // version ("1"). Knowing a key and not modelling it is the honest
        // answer for all three; only the numeric one can also carry a value,
        // so only it joins `INERT_LOCAL` below. The two STRING keys must stay
        // out of that list: `optional_number_is` cannot parse a hex digest, so
        // membership there would raise a false note on every file that has one.
        "LocalCorrectedDepth",
        "LocalInputDigest",
        "LocalInputDigestVersion",
    ];
    // The pre-2012 process-version twins (`LocalExposure` beside
    // `LocalExposure2012`, …) plus the bands this engine has no model for.
    // `LocalHue`/`LocalSharpness` LEFT this list in R23-1b: they have no
    // `*2012` twin, they are read back by `parse_one_correction`, and the
    // writer now emits the recipe's own values — a correction carrying them
    // is fully supported, not a partial import. `LocalToningHue` /
    // `LocalToningSaturation` (the mask Color tint) left it on the same
    // terms once the engine rendered the tint.
    const INERT_LOCAL: [&str; 8] = [
        "LocalExposure",
        "LocalContrast",
        "LocalClarity",
        "LocalBrightness",
        "LocalMoire",
        "LocalDefringe",
        "LocalGrain",
        // Silent at its observed "0", NAMED if a file ever carries another
        // value — the same law every other inert key follows. Being
        // bookkeeping is a reason not to call it an unknown key, not a reason
        // to stop looking at it.
        "LocalCorrectedDepth",
    ];

    if !matches!(
        own.crs_str("CorrectionActive").as_deref(),
        None | Some("true")
    ) || !optional_scaled_number_in(own, "CorrectionAmount", 1.0, 0.0, 1.0)
        || !optional_scaled_number_in(own, "LocalExposure2012", 4.0, -5.0, 5.0)
        || !optional_scaled_number_in(own, "LocalLuminanceNoise", 100.0, 0.0, 100.0)
    {
        return Err(MaskImportReason::OutOfModel);
    }

    for key in [
        "LocalContrast2012",
        "LocalHighlights2012",
        "LocalShadows2012",
        "LocalWhites2012",
        "LocalBlacks2012",
        "LocalClarity2012",
        "LocalDehaze",
        "LocalTexture",
        "LocalSharpness",
        "LocalSaturation",
        "LocalTemperature",
        "LocalTint",
    ] {
        if !optional_scaled_number_in(own, key, 100.0, -100.0, 100.0) {
            return Err(MaskImportReason::OutOfModel);
        }
    }
    // `LocalHue` left the loop above in v0.32.0: its file scale is 180, not
    // 100 (`parse_one_correction`'s `q180`). The gate moves WITH the reader.
    // Its end stop is the file's ±1.0, a half turn (v1.17.0, measured):
    // Camera Raw renders 0.25 / 0.5 / 0.75 / 1.0 as turns that grow with the
    // value and ±1.0 as the same 180° turn, so ±1.0 is a hue Lightroom's
    // engine draws, and the old ±100 (±0.555556) stop refused it. Plus one
    // wire step: the key is written at six decimals.
    let hue_stop = crate::recipe::LOCAL_HUE_MAX_DEG + 0.001;
    if !optional_scaled_number_in(own, "LocalHue", 180.0, -hue_stop, hue_stop) {
        return Err(MaskImportReason::OutOfModel);
    }
    // The Color swatch's own ranges: a hue in degrees around the wheel and a
    // strength on the 0..1 scale. Off either end is not a swatch Lightroom
    // can produce, so it is the same refusal as any other off-slider value.
    if !optional_scaled_number_in(own, "LocalToningHue", 1.0, 0.0, 360.0)
        || !optional_scaled_number_in(own, "LocalToningSaturation", 100.0, 0.0, 100.0)
    {
        return Err(MaskImportReason::OutOfModel);
    }
    // Everything below this line is a NOTE, not a refusal.
    let mut reasons: Vec<MaskImportReason> = Vec::new();
    for key in INERT_LOCAL {
        if !optional_number_is(own, key, 0.0) {
            reasons.push(MaskImportReason::InertLocal(key));
        }
    }
    if !optional_number_is(own, "LocalCurveRefineSaturation", 100.0) {
        reasons.push(MaskImportReason::CurveRefineSaturation);
    }
    // The four per-channel local curves are child ELEMENTS, so the attribute
    // scan below never sees them. R25 P1 raised a note for every correction
    // that carried one, because none of them was modelled; R25 P6 models all
    // four, so the note narrowed to the case that is still a real loss: a
    // curve that is PRESENT and cannot be READ.
    //
    // The distinction is the module's own — "a knob we do not model is not the
    // same thing as a value we cannot read" (see `MaskImportReason`) — and the
    // narrowing is what stops the disclosure from claiming a loss that no
    // longer happens. `parse_one_correction` reads the same four keys through
    // the unchecked `parse_curve`, whose `Err` half becomes an empty curve;
    // this loop is what keeps that from being silent. Unreadable costs the
    // CURVE, not the correction — the geometry is still exactly what the file
    // draws, the same verdict `ForeignRangeMask` gets.
    if ["MainCurve", "RedCurve", "GreenCurve", "BlueCurve"]
        .iter()
        .any(|k| parse_curve_checked(own.text(), k).is_err())
    {
        reasons.push(MaskImportReason::LocalCurve);
    }

    // The own scope BEGINS with the correction's start tag by construction
    // (`element_own_scope` copies it first), so this walk sees exactly the
    // attributes it always did.
    let Some((_, gt, _)) = next_xml_tag(own.text(), 0) else {
        return Err(MaskImportReason::OutOfModel);
    };
    let mut cursor = 0;
    while let Some(a) = next_xml_attribute(&own.text()[..=gt], &mut cursor) {
        if let Some(local) = a.name.strip_prefix("crs:")
            && local.starts_with("Local")
            && !KNOWN_LOCAL.contains(&local)
        {
            reasons.push(MaskImportReason::UnknownLocalKey);
            break;
        }
    }
    Ok(reasons)
}

/// One `<rdf:li crs:What="Mask/…">` component. `Err(())` = the component is
/// UNUSABLE, and the caller decides what that costs: a geometry takes the
/// whole correction with it, a range component only takes itself.
pub(super) fn component_import_reasons(
    tag: Tag<'_>,
    what: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
) -> Result<Vec<MaskImportReason>, ()> {
    let expected_mode = if what == "Mask/RangeMask" { "1" } else { "0" };
    let expected_value = if what == "Mask/RangeMask" { 0.0 } else { 1.0 };

    // A muted component changes what the mask covers — a value we can read but
    // have no model for, so it still refuses. (Compare the knobs below, which
    // are composition, not coverage.)
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true")) {
        return Err(());
    }
    // Lightroom's SUBTRACT is encoded as a PAIR, and `MaskValue="0"` is HALF of
    // it — not an opacity of zero. Census of every GitHub-indexed `.xmp`
    // carrying `crs:MaskBlendMode` (157 files, 479 attribute instances,
    // 2026-08-18; re-derived with a real XML parser, 0 parse failures):
    //
    //   MaskBlendMode="1"  ⇒  MaskValue="0"    26 / 26, no exceptions
    //   MaskBlendMode="0"  ⇒  MaskValue="1"   436 of 453 (16 are 0, one 0.662178)
    //   MaskBlendMode="1"  with MaskValue="1"   0 / 479
    //
    // That older corpus has inversion only with mode 0. The R35 library
    // census also verifies mode 1 + inverted true: subtracting the inverted
    // shape is Lightroom's spelling of Intersect. Both use the same value 0.
    //
    // So a zero MaskValue UNDER a non-default blend mode says "this shape is
    // subtracted", where the same zero on its own says "this shape is muted".
    // Reading the pair as a mute took the whole correction down (the geometry
    // arm turns `Err` into `OutOfModel`), which threw away a mask the file
    // draws perfectly well. R35 imports and projects each supported shape
    // in order through the common composition spelling. What we must NEVER
    // do is treat the 0 as a strength and multiply it
    // in: that would silently neutralise a mask the file says is fully
    // painted. Nothing here reads `MaskValue` as a magnitude; the adjustment's
    // strength comes from `crs:CorrectionAmount` alone (see
    // `parse_one_correction`).
    //
    // `expected_value != 0.0` keeps our own `Mask/RangeMask` encoding
    // (`MaskBlendMode="1"` + `MaskValue="0"`, which IS this app's intersect
    // spelling) out of the branch — it is already the expected pair there.
    let subtracted = expected_value != 0.0
        && tag.crs_str("MaskBlendMode").is_some_and(|mode| mode.as_ref() != expected_mode)
        && tag.crs_f32("MaskValue").is_some_and(|v| v.abs() <= 1e-6);
    if !subtracted && !optional_number_is(tag, "MaskValue", expected_value) {
        return Err(());
    }
    let mut reasons: Vec<MaskImportReason> = Vec::new();
    // `crs:Angle` sits on EVERY Lightroom radial, written as "0" when the
    // shape was never rotated (13 of the 24 radials in the reference set). A
    // zero angle loses NOTHING, so flagging its mere presence would raise a
    // false loss on half the catalog — the same "alarm on every save is alarm
    // the user learns to ignore" rule R24 applied to the export line. Present
    // but unreadable counts as rotated: we cannot say it is zero.
    //
    // v0.32.0 NARROWED this to what it now costs. The rotation used to be
    // dropped from EVERY radial, because the sign and pivot were unverified;
    // both are measured and `lr_to_engine` carries the tilt through. What is
    // left is one case — a document that declares no `tiff:ImageWidth /
    // ImageLength`, so the pixel→normalised fold has no aspect to fold with
    // (`FrameAspect`). Then, and only then, the ellipse arrives axis-aligned
    // and this says so. Asked of the DECODER rather than re-derived here, so
    // the sentence the user reads and the geometry the render draws cannot
    // disagree.
    if tag.crs_str("Angle").is_some() && tag.crs_f32("Angle").is_none_or(|v| v != 0.0) {
        // The angle rides along so the disclosure can NAME it; an unreadable
        // one is a rotation we cannot measure, and `0` is this payload's word
        // for that (see the variant's doc). `as i32` saturates rather than
        // wrapping.
        let readable = tag.crs_f32("Angle");
        // Two ways the tilt fails to arrive, and the frame narrows only ONE of
        // them: a value we cannot PARSE is a rotation nobody can apply however
        // well the frame is known, and `parse_one_correction` reads it as 0.
        if readable.is_none() || frame.is_none() {
            reasons.push(MaskImportReason::Rotation(readable.map_or(0, |v| v.round() as i32)));
        }
    }
    // `crs:MaskBlendMode` sits on every component Lightroom writes, and the
    // overwhelming majority carry the DEFAULT — the plain composition this
    // engine already does, so accepting it costs nothing. Only a different
    // mode is a loss, and it costs the composition, not the shape. Authorship
    // is deliberately NOT part of this test any more: the value says what it
    // says whoever wrote it, and requiring our own provenance is what refused
    // every Lightroom mask in the first place.
    if tag.crs_str("MaskBlendMode").is_some_and(|mode| !matches!(mode.as_ref(), "0" | "1")) {
        reasons.push(MaskImportReason::BlendMode);
    }

    match what {
        "Mask/Gradient" => {
            if matches!(
                tag.crs_str("MaskInverted").as_deref(),
                None | Some("true") | Some("false")
            ) && ["ZeroX", "ZeroY", "FullX", "FullY"]
                .iter()
                .all(|key| tag.crs_f32(key).is_some_and(|v| (-8.0..=8.0).contains(&v)))
            {
                Ok(reasons)
            } else {
                Err(())
            }
        }
        "Mask/CircularGradient" => {
            // `crs:Roundness` is a ±100 INTEGER slider, not a 0..1 aspect
            // ratio. Direct observation, 2026-08-18: every one of the 24
            // radials in the harvested real-sidecar corpus writes it as a bare
            // signed integer, all of them `"0"` (its default), alongside
            // `Feather="+100"` and `Midpoint="+50"` on the same 0..100-style
            // integer footing — while the ONE attribute in those components
            // that really is a 0..1 real, `Feather` inside `crs:RetouchAreas`,
            // is a different field entirely (`0.388672`). ExifTool types the
            // whole `%sCorrectionMask` family as `real`, so the type says
            // nothing; the VALUES say ±100. The old `(0.0..=1.0)` gate was the
            // "bbox aspect" reading, and it refused the WHOLE correction — a
            // Lightroom user who touched this slider lost the mask, silently.
            //
            // Widened to the observed slider domain. No CONVERSION is applied
            // and none is needed: `roundness` is carried, never rendered (see
            // `MaskGeometry::Radial`), so the number rides through the recipe
            // and back into the sidecar unchanged. That is also what settles
            // the one value both readings could claim — `1`. Feather HAS to
            // disambiguate 0..1 from 0..100 because feather is rendered and a
            // wrong guess reshapes the photo; roundness has nothing to
            // disambiguate FOR, so a legacy `"0.25"` we once wrote comes back
            // as 0.25 and a Lightroom `"1"` comes back as 1, each written out
            // exactly as it arrived. Whichever scale either meant, the file
            // gets its own number back.
            if !matches!(
                tag.crs_str("MaskInverted").as_deref(),
                None | Some("true") | Some("false")
            ) || !matches!(
                tag.crs_str("Flipped").as_deref(),
                None | Some("true") | Some("false")
            ) || !["Top", "Left", "Bottom", "Right"]
                .iter()
                .all(|key| tag.crs_f32(key).is_some_and(|v| (-8.0..=8.0).contains(&v)))
                || !tag.crs_f32("Roundness").is_some_and(|v| (-100.0..=100.0).contains(&v))
            {
                return Err(());
            }
            let Some(raw) = tag.crs_f32("Feather") else {
                return Err(());
            };
            let feather = if raw > 1.0 || raw == raw.trunc() { raw / 100.0 } else { raw };
            if !(0.0..=1.0).contains(&feather) {
                return Err(());
            }
            // v0.32.0 — the corner decode's OWN gate, run here so a component
            // that cannot be decoded is refused by the same pass that refuses
            // every other unreadable value, with the geometry arm's cost
            // (`OutOfModel` takes the whole correction). Real Lightroom data
            // clears it 80/80 (`BBOX-DECODE.md` §2.1); what does not is a box
            // that folds to a semi-axis of ZERO at the declared angle, i.e.
            // not an ellipse at all. `OutOfModel` is the honest existing
            // reason — "a value we can read that lands outside this engine's
            // model" — and needs no new word in any UI language. A NEGATIVE
            // fold left this gate in v1.2.4: see [`RadialDecode::Refused`].
            if matches!(
                lr_to_engine(
                    LrRadial {
                        top: tag.crs_f32("Top").unwrap_or(0.0) as f64,
                        left: tag.crs_f32("Left").unwrap_or(0.0) as f64,
                        bottom: tag.crs_f32("Bottom").unwrap_or(0.0) as f64,
                        right: tag.crs_f32("Right").unwrap_or(0.0) as f64,
                        angle_deg: tag.crs_f32("Angle").unwrap_or(0.0) as f64,
                    },
                    frame,
                ),
                RadialDecode::Refused
            ) {
                return Err(());
            }
            Ok(reasons)
        }
        // Someone else's range encoding is not ours to interpret — but that
        // is a reason to leave the RANGE behind, not the mask (R25 P1). The
        // caller turns this `Err` into a `ForeignRangeMask` note.
        "Mask/RangeMask" => {
            if authored_by_autoshade
                && matches!(tag.crs_str("MaskInverted").as_deref(), None | Some("true"))
            {
                Ok(reasons)
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// One `crs:What="Mask/Aggregate"` component → [`MaskGeometry::Brush`], or
/// `Err(())` when the file breaks an invariant this parser refuses to guess
/// past. `scope` is the string `agg`'s offsets are measured in.
///
/// **What is refused, and why refusing beats guessing.** Every gate below has
/// ZERO counter-examples in the 177-sidecar current corpus, so a document
/// that trips one was written by something other than Lightroom and this
/// module has no measurement to model it with. The caller turns the `Err` into
/// [`MaskImportReason::OutOfModel`] — "a value we can read that lands outside
/// this engine's model" — which costs the correction and says so, rather than
/// importing a shape the file does not describe.
///
///  * a child that is not `Mask/Paint` (measured: 398/398 children are Paint,
///    never a Gradient, Radial, RangeMask, Image or another Aggregate);
///  * anything nested BELOW the Paints (measured: maximum component nesting
///    depth in the whole library is exactly 2);
///  * a group with no strokes at all — not a measured shape, and re-emitting
///    an empty `<crs:Masks>` would put a construct into a sidecar that
///    Lightroom never writes;
///  * the per-stroke gates in [`parse_paint_stroke`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskBrushTableRefusal {
    MaskBrushTableUnavailable,
    ContainerInvalid,
    ReferenceMismatch,
    DigestMismatch,
    EncodingUnsupported,
    Corrupt,
    LengthMismatch,
    PayloadUnsupported,
    PayloadInvalid,
}

impl MaskBrushTableRefusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::MaskBrushTableUnavailable => "MaskBrushTableUnavailable",
            Self::ContainerInvalid => "ContainerInvalid",
            Self::ReferenceMismatch => "ReferenceMismatch",
            Self::DigestMismatch => "DigestMismatch",
            Self::EncodingUnsupported => "EncodingUnsupported",
            Self::Corrupt => "Corrupt",
            Self::LengthMismatch => "LengthMismatch",
            Self::PayloadUnsupported => "PayloadUnsupported",
            Self::PayloadInvalid => "PayloadInvalid",
        }
    }
}
