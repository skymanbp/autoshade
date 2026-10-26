//! Reading corrections: brush groups, AI masks, dabs and strokes, classification, base and parametric geometry, and the one-correction parsers.

use super::*;

fn parse_brush_group(
    scope: &str,
    agg: &XmlComponent<'_>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> Result<MaskGeometry, ()> {
    let tag = Tag::new(agg.tag);
    // `agg.start` is relative to one correction's component block and can be
    // identical in sibling corrections. The tag's address identifies this
    // Aggregate for the lifetime of the shared classify/build reader.
    let owner_id = agg.tag.as_ptr() as usize;
    // A muted component changes what the mask covers — refused for exactly the
    // reason `component_import_reasons` refuses a muted parametric shape.
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true")) {
        return Err(());
    }
    // The group's three composition attributes, carried VERBATIM. Absent reads
    // as Lightroom's default in each case (it writes all three on 42/42 real
    // Aggregates, so absence is a foreign writer's terseness, not a value).
    let blend_mode = match tag.crs_str("MaskBlendMode") {
        None => 0u32,
        Some(v) => v.trim().parse::<u32>().map_err(|_| ())?,
    };
    let value = match tag.crs_str("MaskValue") {
        None => 1.0f32,
        Some(_) => tag.crs_f32("MaskValue").filter(|v| v.is_finite()).ok_or(())?,
    };
    let inverted = match tag.crs_str("MaskInverted").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(()),
    };
    let name = tag.crs_str("MaskName").map(|v| v.into_owned()).unwrap_or_default();

    let mut strokes = Vec::new();
    let table = tag.crs_str("MaskBrushTable");
    let advertised = tag.crs_str("MaskBrushUncompressedBytes");
    if table.is_some() || advertised.is_some() {
        let reference = match table {
            Some(table) => Some(table),
            None => {
                let error = MaskBrushError::new(
                    MaskBrushTableRefusal::ReferenceMismatch,
                    "MaskBrushUncompressedBytes is present without MaskBrushTable",
                );
                brush_reader.report(owner_id, "<missing>", &error);
                None
            }
        };
        if let Some(table) = reference {
            match advertised.and_then(|v| v.trim().parse::<usize>().ok()) {
                Some(expected) => {
                    if let Ok(table_strokes) = brush_reader.table(owner_id, table.trim(), expected) {
                        strokes.extend(table_strokes);
                    }
                }
                None => {
                    let error = MaskBrushError::new(
                        MaskBrushTableRefusal::LengthMismatch,
                        "MaskBrushUncompressedBytes is missing or unreadable",
                    );
                    brush_reader.report(owner_id, table.trim(), &error);
                }
            }
        }
    }
    if let Some(body) = component_body(scope, agg)? {
        let kids = components_in(body);
        if kids.iter().any(|k| k.depth > 0) {
            return Err(());
        }
        for k in &kids {
            if k.what.as_ref() != "Mask/Paint" {
                return Err(());
            }
            strokes.push(parse_paint_stroke(body, k)?);
        }
    }
    if strokes.is_empty() {
        return Err(());
    }
    Ok(MaskGeometry::Brush { name, blend_mode, value, inverted, strokes })
}

/// One `crs:What="Mask/Image"` component → [`MaskGeometry::AiMask`], or
/// `Err(())` when the file breaks an invariant this parser refuses to guess
/// past. `scope` is the string `img`'s offsets are measured in.
///
/// **What is refused, and why refusing beats guessing** — every gate has zero
/// counter-examples in the 105 current instances:
///
///  * a muted component (`MaskActive` other than `"true"`) — 105/105 are
///    `"true"`, and a muted component changes what the mask covers;
///  * a missing or unreadable `MaskSubType` / `ReferencePoint` — both sit on
///    105/105, and they ARE the mask: without the pair there is nothing to
///    point a segmenter at;
///  * a `MaskSubType` outside `{0, 1, 2}` — three values on 105/105, and each
///    routes to a specific backend. A fourth would have no backend and
///    guessing one would invent a selection;
///  * an attribute name outside the modelled set plus
///    [`AI_MASK_PROVENANCE_KEYS`];
///  * a child element other than `crs:Gesture`, or a Gesture holding anything
///    but `Mask/Paint` (40 Gestures, one Paint each).
fn parse_ai_mask(scope: &str, img: &XmlComponent<'_>) -> Result<MaskGeometry, ()> {
    let tag = Tag::new(img.tag);
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true")) {
        return Err(());
    }
    // The three composition attributes, read exactly as a brush group's are.
    let blend_mode = match tag.crs_str("MaskBlendMode") {
        None => 0u32,
        Some(v) => v.trim().parse::<u32>().map_err(|_| ())?,
    };
    let value = match tag.crs_str("MaskValue") {
        None => 1.0f32,
        Some(_) => tag.crs_f32("MaskValue").filter(|v| v.is_finite()).ok_or(())?,
    };
    let inverted = match tag.crs_str("MaskInverted").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(()),
    };
    let name = tag.crs_str("MaskName").map(|v| v.into_owned()).unwrap_or_default();
    // REQUIRED, not defaulted: an absent subtype is not "object", it is a
    // component this reader has never seen.
    let subtype = tag.crs_str("MaskSubType")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|v| (0..=2).contains(v))
        .ok_or(())?;
    // `"0.517578 0.260997"` — space separated, normalised. STRICT: a malformed
    // token used to be the kind of thing a `filter_map` would drop, leaving the
    // remaining value to shift into the wrong field.
    let pt: Option<Vec<f32>> = tag.crs_str("ReferencePoint")
        .map(|s| s.split_whitespace().map(|x| x.parse::<f32>().ok()).collect::<Option<Vec<_>>>())
        .ok_or(())?;
    let pt = pt.filter(|v| v.len() == 2 && v.iter().all(|x| x.is_finite())).ok_or(())?;
    let mask_version = tag.crs_str("MaskVersion")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(1);

    // Every attribute on the element, in document order, split into "modelled
    // above", "carried as provenance", and "refused".
    let mut provenance: Vec<(String, String)> = Vec::new();
    for (key, raw) in crs_attributes(tag.text()) {
        match key.as_str() {
            // Modelled, or an invariant this writer re-emits as a literal.
            "What" | "MaskActive" | "MaskName" | "MaskBlendMode" | "MaskInverted"
            | "MaskSyncID" | "MaskValue" | "MaskVersion" | "MaskSubType" | "ReferencePoint" => {}
            k if AI_MASK_PROVENANCE_KEYS.contains(&k) => {
                provenance.push((key, xml_unescape(raw).into_owned()));
            }
            _ => return Err(()),
        }
    }

    // The optional `crs:Gesture` — the photographer's brush refinement.
    let mut gesture = Vec::new();
    if let Some(body) = component_body(scope, img)? {
        let Some(seq) = owned_element_body(body, "crs:Gesture")? else {
            // A body that is not a Gesture is markup this reader cannot
            // account for — the same verdict `classify_correction` gives a
            // component nested somewhere unmodelled.
            return Err(());
        };
        let kids = components_in(seq);
        if kids.iter().any(|k| k.depth > 0) {
            return Err(());
        }
        for k in &kids {
            if k.what.as_ref() != "Mask/Paint" {
                return Err(());
            }
            gesture.push(parse_paint_stroke(seq, k)?);
        }
        if gesture.is_empty() {
            return Err(());
        }
    }

    Ok(MaskGeometry::AiMask {
        name,
        subtype,
        ref_x: pt[0],
        ref_y: pt[1],
        blend_mode,
        value,
        inverted,
        mask_version,
        provenance,
        gesture,
        // Nothing is resolved at PARSE time: the segmenter runs at develop
        // time (`segment::resolve_ai_masks`), which is what makes this lazy —
        // importing a library must not spawn a model run per photo.
        raster: None,
    })
}

/// Every `crs:`-namespaced attribute on one element tag, as
/// `(local name, RAW value)` in document order.
///
/// Exists so [`parse_ai_mask`] can assert the CLOSED vocabulary its refusal
/// gate depends on. Asking `crs_str` for each known name could only ever tell
/// us which of the names we already knew were present — never that the document
/// carried a name we have not measured, which is the case that means "not
/// Lightroom's writer".
///
/// **R28 Batch-5 5d (F4 root 2 / symptom C): built on [`next_xml_attribute`]
/// instead of a second hand-rolled lexer.** The old one searched for `crs:`,
/// then for `=`, then for a DOUBLE QUOTE — so on a document written with
/// single-quoted attributes (`crs:MaskSubType='0'`, legal XML that every parser
/// but this one accepts) the first `find('"')` ran past the end of the tag or
/// into an unrelated value. What that cost was not academic: the closed-
/// vocabulary loop above saw an empty (or wrongly paired) attribute list, so it
/// could neither refuse an unmeasured name nor CARRY the eleven provenance /
/// digest keys — they were dropped on the way back out, silently, from a file
/// this reader had just accepted.
///
/// `next_xml_attribute` is quote-complete (it takes whichever quote opens the
/// value as the one that closes it), stops at the tag's own `/` or `>` rather
/// than running into a body, and is the SAME reader
/// `correction_value_reasons`' unknown-key walk already used — so the two scans
/// of "what attributes does this element carry" are now one implementation
/// rather than two with different ideas about XML.
fn crs_attributes(tag: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(a) = next_xml_attribute(tag, &mut cursor) {
        if let Some(local) = a.name.strip_prefix("crs:") {
            out.push((local.to_string(), a.value));
        }
    }
    out
}

/// One `crs:What="Mask/Paint"` child of a brush group → [`BrushStroke`].
///
/// The three attribute INVARIANTS are gates, not fields: `MaskActive="true"`,
/// `MaskBlendMode="0"` and `MaskInverted="false"` hold on 398/398 real Paints,
/// so a Paint that says otherwise is asserting a composition inside a group
/// that has never been observed to have one. The four NUMBERS are required
/// rather than defaulted for the same reason — all nine attributes sit on
/// 398/398 in-correction instances, with no optional fields and no variation,
/// so a missing one means this is not the encoding we measured.
pub(super) fn parse_paint_stroke(scope: &str, p: &XmlComponent<'_>) -> Result<BrushStroke, ()> {
    let tag = Tag::new(p.tag);
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true"))
        || !matches!(tag.crs_str("MaskBlendMode").as_deref(), None | Some("0"))
        || !matches!(tag.crs_str("MaskInverted").as_deref(), None | Some("false"))
    {
        return Err(());
    }
    let num = |k: &str| tag.crs_f32(k).filter(|v| v.is_finite()).ok_or(());
    Ok(BrushStroke {
        value: num("MaskValue")?,
        radius: num("Radius")?,
        flow: num("Flow")?,
        center_weight: num("CenterWeight")?,
        sync_id: tag.crs_str("MaskSyncID").map(|v| v.into_owned()).unwrap_or_default(),
        dabs: parse_dabs(scope, p)?,
    })
}

/// The `crs:Dabs` token stream of one Paint, VERBATIM — one token per line, in
/// the order the file lists its `<rdf:li>` items.
///
/// Each token is VALIDATED against the measured grammar (§[`BrushStroke::dabs`])
/// and then stored unchanged. Validating without parsing is the whole point:
/// the stream is the one payload whose MEANING waits on a measurement the
/// sidecar cannot supply, so this proves it is a stream we recognise and
/// refuses to impose a structure on it.
fn parse_dabs(scope: &str, p: &XmlComponent<'_>) -> Result<String, ()> {
    /// Far past the largest real stroke (645 dabs / 1,267 tokens) and far
    /// past the whole reference library (15,964 dabs), but still a bound — a
    /// hand-written sidecar must not be able to make one correction cost an
    /// unbounded allocation.
    const MAX_DAB_TOKENS: usize = 65_536;
    let Some(body) = component_body(scope, p)? else {
        return Err(());
    };
    let Some(seq) = owned_element_body(body, "crs:Dabs")? else {
        // Present on 398/398. A Paint without one is not a stroke.
        return Err(());
    };
    let mut out = String::new();
    let mut tokens = 0usize;
    let mut from = 0;
    while let Some((start, end, self_closing)) = next_xml_tag(seq, from) {
        let tag = &seq[start..=end];
        if tag.starts_with("</") || tag_name(tag) != "rdf:li" {
            from = end + 1;
            continue;
        }
        if self_closing || tokens >= MAX_DAB_TOKENS {
            return Err(()); // an empty <rdf:li/> holds no token
        }
        let close = element_close_start(seq, "rdf:li", end).ok_or(())?;
        let token = xml_unescape(&seq[end + 1..close]);
        dab_token_is_known(token.as_ref())?;
        if tokens > 0 {
            out.push('\n');
        }
        out.push_str(token.as_ref());
        tokens += 1;
        from = close + 1;
    }
    if tokens == 0 {
        return Err(());
    }
    Ok(out)
}

/// Is this one `crs:Dabs` item a token of the measured grammar? `r <f>`,
/// `f <f>`, `h <f>` or `d <x> <y>`, and nothing else — 22,966 tokens over 382
/// components, zero malformed, four forms.
///
/// The newline check is not decoration. [`parse_dabs`] joins tokens with
/// `'\n'` and the writer splits them back on it, so a token carrying one would
/// silently become two on the round trip. No real token spans a line; this
/// makes the storage form lossless by construction rather than by luck.
pub(super) fn dab_token_is_known(t: &str) -> Result<(), ()> {
    /// A LENGTH bound as well as a shape one (R28 2b, adjudication F5's
    /// aggravator). Real tokens run ~10-30 bytes over the 22,966-token census
    /// above; 256 is eight times the widest of them, and still comfortably
    /// past the ~100 bytes two full-precision `f32` `Display`s plus the `d `
    /// prefix could occupy, so no token the grammar can legitimately produce
    /// is refused here.
    ///
    /// What it stops: a coordinate written as `0.` plus 300,000 digits parses
    /// to a perfectly finite `f32` (0.111…) and passes every check below,
    /// while the token COUNT gate (`MAX_DAB_TOKENS` = 65,536) never fires —
    /// ONE token then blows the storage-side 256 KiB byte cap by itself.
    /// Refused the way every other malformed token is: the Paint does not
    /// import, which by this parser's all-or-nothing group rule refuses the
    /// Aggregate and discloses it as `OutOfModel`.
    ///
    /// The FRACTIONAL form is the reachable one, and the distinction is not
    /// pedantry: the adjudication's own example — 300,000 integer digits —
    /// overflows to `inf` and was already refused by the finiteness check
    /// below, so only the `0.…` shape ever needed this bound (measured while
    /// writing the mutation test, R28 2b).
    const MAX_TOKEN_BYTES: usize = 256;
    if t.len() > MAX_TOKEN_BYTES {
        return Err(());
    }
    if t.contains('\n') || t.contains('\r') {
        return Err(());
    }
    let mut it = t.split_whitespace();
    let arity = match it.next() {
        Some("r" | "f" | "h") => 1,
        Some("d") => 2,
        _ => return Err(()),
    };
    for _ in 0..arity {
        let v = it.next().ok_or(())?.parse::<f32>().map_err(|_| ())?;
        if !v.is_finite() {
            return Err(());
        }
    }
    if it.next().is_some() {
        return Err(());
    }
    Ok(())
}

/// The inverse of the census spelling, shared by ALL geometry kinds.
/// Without editor metadata, subtract-inverted has the canonical Intersect
/// representation. Its own inversion is removed exactly once.
fn brush_combine(blend_mode: u32, inverted: bool) -> (MaskCombine, bool) {
    match (blend_mode, inverted) {
        (1, true) => (MaskCombine::Intersect, false),
        (1, false) => (MaskCombine::Subtract, false),
        _ => (MaskCombine::Add, inverted),
    }
}

fn range_values_are_supported(range: &RangeMask) -> bool {
    match range {
        RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
            [lo_outer, lo, hi, hi_outer].iter().all(|v| v.is_finite())
                && 0.0 <= *lo_outer
                && *lo_outer <= *lo
                && *lo <= *hi
                && *hi <= *hi_outer
                && *hi_outer <= 1.0
        }
        RangeMask::Color { r, g, b, amount, px, py } => {
            [r, g, b, amount, px, py]
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(*v))
        }
    }
}

pub(super) fn classify_correction(
    seg: &str,
    own: Scope<'_>,
    authored_by_autoshade: bool,
    intent_declared: bool,
    frame: Option<FrameAspect>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> MaskCorrectionParse {
    let mut geometry_count = 0usize;
    let mut brush_count = 0usize;
    let mut ai_count = 0usize;
    let mut range_count = 0usize;
    let mut unknown_component = false;
    let mut geometry_unusable = false;
    let mut range_usable = true;
    let mut reasons: Vec<MaskImportReason> = Vec::new();
    // By NAME (attribute-carrying spelling included). No component list at
    // all means no parametric geometry to stand on; an UNTERMINATED one is
    // markup we could not finish reading, which is a different sentence.
    let mask_block = match owned_element_body(seg, "crs:CorrectionMasks") {
        Ok(Some(b)) => b,
        Ok(None) => return MaskCorrectionParse::Unsupported(MaskImportReason::Unrepresentable),
        Err(()) => return MaskCorrectionParse::Unsupported(MaskImportReason::OutOfModel),
    };

    let base_tag = base_geometry_at(seg).and_then(|p| next_xml_tag(seg, p))
        .map(|(s, e, _)| &seg[s..=e]);
    // R27 Batch-4 hazard 1: NESTING-AWARE. This loop used to be a flat
    // `next_xml_tag` walk, which reads the `Mask/Paint` strokes inside a
    // `Mask/Aggregate` as SIBLINGS of it — harmless only while both answers
    // were "refuse", and fatal the moment a Paint means something. Only
    // `depth == 0` components belong to THIS correction's list; everything
    // deeper belongs to the component that contains it, and is validated by
    // that component's own parser (see `parse_brush_group`).
    let components = components_in(mask_block);
    // The extents of the groups that own the nested components, so a nested
    // component sitting somewhere we do NOT model cannot pass unnoticed.
    let mut owned_nesting: Vec<(usize, usize)> = Vec::new();
    for component in components.iter().filter(|c| c.depth == 0) {
        {
            let (tag, what) = (component.tag, &component.what);
            let verdict =
                component_import_reasons(Tag::new(tag), what.as_ref(), authored_by_autoshade, frame);
            match what.as_ref() {
                "Mask/Gradient" | "Mask/CircularGradient" => {
                    geometry_count += 1;
                    if base_tag == Some(tag) && Tag::new(tag).crs_str("MaskBlendMode")
                        .is_some_and(|m| m != "0")
                    {
                        reasons.push(MaskImportReason::BlendMode);
                    }
                    match verdict {
                        Ok(rs) => reasons.extend(rs),
                        Err(()) => geometry_unusable = true,
                    }
                }
                "Mask/RangeMask" => {
                    range_count += 1;
                    if verdict.is_err() {
                        range_usable = false;
                    }
                }
                // BRUSH GROUP — imported since R27 Batch-4 (L-08). The
                // registration that used to stand here said this whole arm was
                // a disclosure-granularity question and not a parser bug; F2's
                // measurement settled it the other way. `Mask/Aggregate` is a
                // one-level container of `Mask/Paint` strokes whose encoding is
                // fully determined by the sidecar (see `MaskGeometry::Brush`),
                // so it is read, carried and written back — and the correction
                // it sits in, plus every parametric shape standing beside it,
                // arrives instead of being thrown away. What is NOT in the
                // sidecar is the alpha kernel — that one was MEASURED rather
                // than guessed (R29 Batch-6), so the render now draws the
                // group from our own model of it and `BrushRendered` says so
                // in both directions.
                //
                // `verdict` is deliberately unused here: the attribute checks
                // it performs are the PARAMETRIC ones (Angle, the subtract
                // pair, `MaskValue == 1`), and a brush group's own attributes
                // mean different things — `MaskValue="0"` on an Aggregate is
                // half of Lightroom's subtract pair and the shared code would
                // read it as a mute. `parse_brush_group` is this component's
                // validator and it is stricter, not looser.
                "Mask/Aggregate" => {
                    brush_count += 1;
                    // The group's EXTENT is recorded whether or not it parses.
                    // A component nested inside a brush group is accounted for
                    // by definition — we know exactly what contains it — so a
                    // group we cannot model must not ALSO make its children
                    // look like markup from nowhere. Getting this backwards
                    // costs the user the accurate sentence: the correction
                    // would be refused as "AI / brush correction(s) skipped"
                    // when what actually happened is "a shape inside the group
                    // that Lightroom has never been observed to write".
                    if let Some(close) =
                        element_close_start(mask_block, tag_name(tag), component.gt)
                    {
                        owned_nesting.push((component.gt, close));
                    }
                    // Same cost as an unreadable parametric component: the
                    // values are legible and the SHAPE is outside the model
                    // this parser measured, which takes the correction.
                    if parse_brush_group(mask_block, component, brush_reader).is_err() {
                        geometry_unusable = true;
                    }
                }
                // AI MASK — imported since R27 Batch-5 (L-08 Arm C), and the
                // dominant refusal until then: 78 corrections across 40 files,
                // 40 % of every file in the reference library that has a mask
                // at all, taking 52 engine-drawable parametric shapes down
                // with them.
                //
                // What arrives is the INTENT (`MaskSubType` + `MaskName` +
                // `ReferencePoint`) and the provenance digests; there is no
                // raster payload and no geometry payload anywhere on one. The
                // alpha is therefore RE-DERIVED by our own segmenter at
                // develop time, not imported, and `AiMaskRecomputed` says so
                // in both directions. That distinction is the whole reason
                // this took a machine-learning work item rather than a parser
                // one — and the reason it can never be silent.
                //
                // `verdict` is deliberately unused, for `Mask/Aggregate`'s
                // reason: the shared checks are the PARAMETRIC ones, and
                // `MaskValue="0"` on an AI component is half of Lightroom's
                // subtract pair, which the shared code would read as a mute.
                // `parse_ai_mask` is this component's validator.
                "Mask/Image" => {
                    ai_count += 1;
                    // The extent is recorded whether or not it parses, exactly
                    // as a brush group's is: a `crs:Gesture` child is nesting
                    // we know the container of, so a component we cannot model
                    // must not ALSO make its own children look like markup from
                    // nowhere.
                    if let Some(close) =
                        element_close_start(mask_block, tag_name(tag), component.gt)
                    {
                        owned_nesting.push((component.gt, close));
                    }
                    if parse_ai_mask(mask_block, component).is_err() {
                        geometry_unusable = true;
                    }
                }
                // Depth / heal / anything else this reader has not measured.
                _ => unknown_component = true,
            }
        }
    }
    // A component nested inside something we did NOT model as a container is
    // markup this reader cannot account for. Refusing it keeps the depth-0
    // filter above honest: without this, a foreign writer could hide a whole
    // second mask inside an element we walked past.
    if components.iter().filter(|c| c.depth > 0).any(|c| {
        !owned_nesting.iter().any(|(open, close)| c.start > *open && c.start < *close)
    }) {
        unknown_component = true;
    }

    if (geometry_count == 0 && brush_count == 0 && ai_count == 0) || unknown_component {
        return MaskCorrectionParse::Unsupported(MaskImportReason::Unrepresentable);
    }
    if geometry_unusable {
        return MaskCorrectionParse::Unsupported(MaskImportReason::OutOfModel);
    }
    if brush_count > 0 {
        reasons.push(MaskImportReason::BrushRendered);
    }
    // Raised on the IMPORT itself, not on whether the segmenter has run: the
    // photographer is being told what kind of thing arrived, and "the alpha
    // will be ours, not Adobe's" is true the moment the component is read.
    // `AiMaskUnresolved` is the separate, later sentence about a mask that has
    // no alpha yet — `segment::resolve_ai_masks` raises that one, because it
    // is the only place that knows.
    if ai_count > 0 {
        reasons.push(MaskImportReason::AiMaskRecomputed);
    }
    match correction_value_reasons(own) {
        Ok(rs) => reasons.extend(rs),
        Err(reason) => return MaskCorrectionParse::Unsupported(reason),
    }

    let Some(mut parsed) =
        parse_one_correction_with_reader(seg, own, intent_declared, frame, brush_reader)
    else {
        return MaskCorrectionParse::Unsupported(MaskImportReason::OutOfModel);
    };
    // A range we cannot honour costs the RANGE, not the mask: the geometry is
    // still exactly what the file draws. Dropping it must be explicit —
    // leaving `parsed.range` in place would import someone else's encoding as
    // if we had understood it.
    if range_count > 0
        && (!range_usable
            || range_count > 1
            || parsed.range.as_ref().is_none_or(|r| !range_values_are_supported(r)))
    {
        parsed.range = None;
        reasons.push(MaskImportReason::ForeignRangeMask);
    }
    // One line per KIND of loss: three components sharing a blend mode is one
    // sentence, not three.
    let mut uniq: Vec<MaskImportReason> = Vec::new();
    for r in reasons {
        if !uniq.contains(&r) {
            uniq.push(r);
        }
    }
    MaskCorrectionParse::Supported(Box::new(parsed), uniq)
}



/// The BASE geometry component of a correction — the byte offset of its tag
/// start within `seg`, or `None` if the correction carries no parametric
/// geometry at all.
///
/// A correction may hold SEVERAL geometry components: Lightroom's Add/Subtract
/// stack, where one shape is the BASE and the rest compose onto it via
/// `crs:MaskBlendMode` (default = the base, `"1"` + `MaskValue="0"` = subtract,
/// the encoding v0.31.1 taught this reader to accept). This engine imports ONE
/// shape and discloses the rest, so which one it takes IS what the photo looks
/// like.
///
/// R25 P9 — it used to take the wrong one, twice over:
///  * the choice was made by KIND. `parse_one_correction` tried
///    `Mask/Gradient` before `Mask/CircularGradient`, so ANY linear anywhere in
///    the correction beat EVERY radial regardless of position. `P29`
///    蒙版 5 is `[CircularGradient, CircularGradient, Gradient]` — a plain
///    3-shape union, every component at the default blend mode — and imported
///    as the TRAILING linear with both radials gone.
///  * and it ignored `crs:MaskBlendMode`, which is the worse half. `P29`
///    蒙版 3 and `P12` Mask 9 are both `[CircularGradient(base),
///    Gradient(MaskBlendMode="1" MaskValue="0")]`: the importer kept the
///    SUBTRACT shape and dropped the base, i.e. it rendered the region
///    Lightroom uses to carve away from the mask as the ENTIRE mask. That is
///    not a truncation of the user's intent, it is an inversion of it.
///
/// So: prefer the first component at the DEFAULT blend mode (the base), and
/// only when every component is subtractive fall back to the first component at
/// all — a correction made of nothing but subtractions has no base to find, and
/// some shape beats no shape.
///
/// R27 Batch-4 hazard 2 — it used to scan the WHOLE correction segment for the
/// first `crs:What="Mask/Gradient"` / `"Mask/CircularGradient"`, nesting-blind.
/// No Aggregate in the current corpus contains a parametric shape (398/398
/// children are Paint), so it could not fire — but nothing in the code enforced
/// that, and a foreign writer that DID nest a gradient inside a brush group
/// would have had it promoted to the correction's base shape. The search is now
/// over this correction's OWN component list, top level only.
fn base_geometry_at(seg: &str) -> Option<usize> {
    let (block_at, _, comps) = correction_mask_components(seg)?;
    let shapes: Vec<_> = comps.iter().filter(|c| c.depth == 0 && matches!(c.what.as_ref(),
        "Mask/Gradient" | "Mask/CircularGradient" | "Mask/Aggregate" | "Mask/Image"
    )).collect();
    let base = shapes.iter().find(|c| Tag::new(c.tag).crs_str("MaskBlendMode")
        .is_none_or(|v| v == "0")).or_else(|| shapes.first())?;
    Some(block_at + base.start)
}

/// One correction's OWN `crs:CorrectionMasks` list: `(offset of the list body
/// inside `seg`, the list body, its components)`.
///
/// The one place the "this correction's components" question is answered, so
/// the base selector and the geometry parser cannot disagree about which
/// components those are.
fn correction_mask_components(seg: &str) -> Option<(usize, &str, Vec<XmlComponent<'_>>)> {
    let (b0, b1) = owned_element_body_span(seg, "crs:CorrectionMasks").ok().flatten()?;
    let block = &seg[b0..b1];
    Some((b0, block, components_in(block)))
}

/// The base component's OWN element, from its `<` to just before its close tag
/// — R27 Batch-4 hazard 3.
///
/// `parse_one_correction` read its geometry keys out of `&seg[p..]`, a slice
/// running from the base component to the END of the correction. That was safe
/// only because Lightroom writes the shared attributes (`MaskValue`,
/// `MaskBlendMode`, `MaskInverted`) on EVERY component, so the first hit was
/// always the base's own — a coincidence, not an invariant, and one that stops
/// holding the moment brush components are legal in an imported correction: an
/// Aggregate at `MaskValue="0"` sitting after a base that omitted the attribute
/// would have donated its subtract half-pair to the base's reads.
///
/// Real Lightroom parametric components are self-closing, so this equals the
/// base TAG on every file in the reference library and the change is invisible
/// there. It is the element-form spelling it makes safe.
fn base_element(seg: &str, p: usize) -> &str {
    let Some((s, e, self_closing)) = next_xml_tag(seg, p) else {
        return &seg[p..];
    };
    if self_closing {
        return &seg[s..=e];
    }
    match element_close_start(seg, tag_name(&seg[s..=e]), e) {
        Some(close) => &seg[s..close],
        // Unterminated markup: fall back to the old unbounded slice rather
        // than losing the geometry entirely — a document this malformed is
        // already refused by `owned_element_body`'s own `Err` upstream.
        None => &seg[s..],
    }
}

/// One `crs:What="Correction"` segment → a [`LocalAdjustment`]. Slider scales
/// invert the writer's: exposure ×4 (a power-of-two rescale, exact in binary
/// FP), every other slider ×100 snapped to 4 decimals so `"0.3" → 30.0` lands
/// back on the UI grid instead of 30.000002.
///
/// `own` is the correction's OWN scope — every SLIDER below is read from it,
/// never from the whole segment (R28 Batch-5 5d, F4 symptom A). The segment
/// itself is still what the GEOMETRY reads walk, because the components live
/// inside it and each of those reads is separately bounded to the component it
/// belongs to.
#[cfg(test)]
pub(super) fn parse_one_correction(
    seg: &str,
    own: Scope<'_>,
    frame: Option<FrameAspect>,
) -> Option<LocalAdjustment> {
    let mut brush_reader = MaskBrushReader::new(None, None);
    parse_one_correction_with_reader(seg, own, intent_namespace_declared(seg), frame, &mut brush_reader)
}

fn parse_parametric_geometry(
    seg: &str,
    p: usize,
    frame: Option<FrameAspect>,
) -> Option<(MaskGeometry, Scope<'_>)> {
    let base_tag = Tag::new(next_xml_tag(seg, p).map_or(&seg[p..], |(s, e, _)| &seg[s..=e]));
    let base_is_linear = xml_attribute_raw(base_tag.text(), "crs:What")
        .is_some_and(|(_, raw)| xml_unescape(raw).as_ref() == "Mask/Gradient");
    // HAZARD 3 (R27 Batch-4): every geometry read below is bounded to the base
    // component's OWN element instead of running to the end of the correction.
    // See `base_element` for the bleed this closes. It stays a SCOPE and not a
    // `Tag` deliberately (R28 Batch-5 5d): the element-form spelling of a
    // component puts these values in a body, and narrowing a working read on no
    // evidence is how a batch grows a regression — the same judgement the
    // `geom_tag` note below records for the two reads that DO need the tag.
    let g = Scope::new(base_element(seg, p));
    Some(if base_is_linear {
        (
            MaskGeometry::Linear {
                zero_x: g.crs_f32("ZeroX")?,
                zero_y: g.crs_f32("ZeroY")?,
                full_x: g.crs_f32("FullX")?,
                full_y: g.crs_f32("FullY")?,
            },
            g,
        )
    } else {
        // The component's OWN TAG — `base_geometry_at` returns a tag START, so
        // this is the `<rdf:li …/>` that carries `crs:What`, and every geometry
        // attribute Lightroom (and this writer) puts on it. Still narrower than
        // `g` after hazard 3: `g` is the base ELEMENT (tag plus any body), and
        // the two reads below ask for names that recur in a body; see there.
        let geom_tag = base_tag;
        // Lightroom's Feather is 0..100 (reference sidecars: 50 / 72 …); the
        // engine's is 0..1. Three writers share this attribute, disambiguated
        // by TEXT SHAPE:
        //  * > 1.0 — unambiguous LR 0..100 scale;
        //  * ≤ 1.0 WITH a decimal point — our LEGACY 0..1 writer (it printed
        //    floats like "0.5"), passed through verbatim;
        //  * ≤ 1.0 integer text ("0"/"1") — LR's 0..100 (a genuine 1% edge)
        //    AND the CURRENT writer (which rounds to integers): both mean
        //    value/100. The old blanket ≤1.0-verbatim rule made our OWN 1%
        //    XMP round-trip back as a 100% feather.
        // (A legacy sidecar holding EXACTLY 1.0 prints as "1" and now reads
        //  as 1% — the current writer's round-trip wins that corner.)
        // Tested on the parsed VALUE (fractional ⇒ legacy 0..1), not the
        // text: "5e-1" carries no '.' yet is 0.5 — a text-shape test sent
        // it through /100.
        let feather_raw = g.crs_f32("Feather")?;
        let feather = if feather_raw > 1.0 || feather_raw == feather_raw.trunc() {
            feather_raw / 100.0
        } else {
            feather_raw
        };
        // v0.32.0: the stored corners are the ROTATED corners of the ellipse's
        // box in PIXEL space, not a bounding box — decoded (with the `k` frame
        // affine and the pixel→normalised rotation fold) by `lr_to_engine`,
        // whose doc carries the evidence. `Refused` is the decode's own
        // degenerate-fold guard: `None` here takes the WHOLE correction, which
        // is what `component_import_reasons` has already independently decided
        // for the same component, so the two arms cannot disagree about what
        // the file says.
        let decoded = lr_to_engine(
            LrRadial {
                top: g.crs_f32("Top")? as f64,
                left: g.crs_f32("Left")? as f64,
                bottom: g.crs_f32("Bottom")? as f64,
                right: g.crs_f32("Right")? as f64,
                // Absent = unrotated. Lightroom writes the attribute on every
                // radial, but an AutoShade sidecar written before v0.32.0 does
                // not, and its box IS the axis-aligned ellipse.
                angle_deg: geom_tag.crs_f32("Angle").unwrap_or(0.0) as f64,
            },
            frame,
        );
        let e = match decoded {
            RadialDecode::Exact(e) | RadialDecode::Unrotated(e) => e,
            RadialDecode::Refused => return None,
        };
        (
            MaskGeometry::Radial {
                top: e.top as f32,
                left: e.left as f32,
                bottom: e.bottom as f32,
                right: e.right as f32,
                feather,
                roundness: g.crs_f32("Roundness")?,
                // NOT `crs:Flipped` (R25 P9 — the defect this batch closed).
                //
                // This engine composes `Radial::flipped` and
                // `LocalAdjustment::inverted` by XOR (render.rs `mask_weight`
                // and the weight loop). Lightroom does not have that pair: it
                // writes ONE inversion bit twice, `crs:MaskInverted` and its
                // complement `crs:Flipped`. Census of the user's library —
                // 201/201 radials anti-correlated, no exceptions; re-derived on
                // the 7 M-B sidecars, 23/23 (16 `Flipped=true MaskInverted=
                // false`, 7 the mirror). Reading BOTH into our two flags XORed
                // a value with its own complement, so the net came out `true`
                // for EVERY imported Lightroom radial regardless of what the
                // file said — AutoShade inverted masks Lightroom does not.
                // Measured cost on `P34`, tone-matched RMS against the
                // real Lightroom export: 0.1099 as imported → 0.0751 with this
                // fixed, and 0.1901 → 0.0869 in blue (E1-verdict §6 defect 2).
                //
                // So the inversion comes from `crs:MaskInverted` ALONE (read
                // into `inverted` below) and this stays false on import. The
                // flag itself is untouched: it is still OUR field, still
                // rendered, still the GUI's Flip checkbox, and a `recipe.json`
                // that carries `flipped: true` renders exactly as before.
                //
                // KNOWN BOUNDARY — a sidecar THIS APP wrote at ≤ v0.31.1 with a
                // flipped radial spelled it `Flipped="true" MaskInverted=
                // "false"`, which now re-imports as not-inverted. That is not a
                // new loss: Lightroom already read that pair as not-inverted,
                // so the old sidecar never carried the flip to Lightroom
                // either. Both directions now agree with Lightroom, which is
                // the whole point. Sidecars written from v0.31.2 on round-trip
                // their net exactly (`mask_geom_xml`).
                flipped: false,
                // v0.32.0: `crs:Angle` IS mapped now. It used to be dropped
                // because its sign and pivot were unverified; both are
                // measured — positive is CLOCKWISE on a y-down screen (three
                // independent determinations, `PROBE2-VERDICT.md` §5's
                // `P22` decoded −60.486° against a measured −60.5°), the
                // pivot is the ellipse centre (25 px against 218 px for the
                // frame centre, `ANGLE-MODEL.md` §3.4), and the rotation
                // happens in PIXEL space (28.554° measured against 19.692°
                // predicted by the normalised-frame reading, §3.2). What lands
                // here is not `crs:Angle` itself but its fold into this
                // engine's normalised-frame convention — see `lr_to_engine`.
                angle: e.angle_deg as f32,
                // R25 P5: the two attributes on every Lightroom radial that
                // this reader could not see until now. OPTIONAL, not `?`:
                // sidecars we wrote before this batch carry neither, and a
                // missing one means "Lightroom's default", not "unreadable
                // mask" (the ACR neutrals 50 / 2 — `crs:Version` is the
                // component's own schema stamp, never our recipe's).
                //
                // Read from `geom_tag`, the ONE component's own tag, not from
                // `g` (which runs to the end of the correction): `crs:Version`
                // is the first geometry attribute whose NAME recurs further
                // down — our own range-mask component carries
                // `crs:Version="3"` (see `range_mask_xml`), so an unbounded
                // scan would read the RANGE's schema stamp as the ellipse's.
                // The older reads above stay on `g`: no attribute they ask for
                // appears twice inside a correction, and re-scoping a working
                // read on no evidence is how a batch grows a regression.
                midpoint: geom_tag.crs_f32("Midpoint").filter(|v| v.is_finite()).unwrap_or(50.0),
                mask_version: geom_tag.crs_str("Version")
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .unwrap_or(2),
            },
            g,
        )
    })
}

fn parse_one_correction_with_reader(
    seg: &str,
    own: Scope<'_>,
    intent_declared: bool,
    frame: Option<FrameAspect>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> Option<LocalAdjustment> {
    let scaled = |k: &str, scale: f32| {
        own.crs_f32(k).map_or(0.0, |v| (v * scale * 10_000.0).round() / 10_000.0)
    };
    let q100 = |k: &str| scaled(k, 100.0);
    let q180 = |k: &str| scaled(k, 180.0);
    let (block_at, block, comps) = correction_mask_components(seg)?;
    let base_at = base_geometry_at(seg)?;
    let base_start = base_at - block_at;
    let correction_tag = next_xml_tag(seg, 0).map(|(s, e, _)| &seg[s..=e]).unwrap_or("");
    let intended_inverted = mask_intent_attr(correction_tag, "Inverted", intent_declared)
        .and_then(|v| v.parse::<bool>().ok());
    let mut parsed = Vec::new();
    for c in comps.iter().filter(|c| c.depth == 0 && c.what != "Mask/RangeMask") {
        let (geometry, inverted) = match c.what.as_ref() {
            "Mask/Aggregate" => {
                let g = parse_brush_group(block, c, brush_reader).ok()?;
                let inverted = g.own_inverted();
                (g, inverted)
            }
            "Mask/Image" => {
                let g = parse_ai_mask(block, c).ok()?;
                let inverted = g.own_inverted();
                (g, inverted)
            }
            _ => {
                let (g, scope) = parse_parametric_geometry(block, c.start, frame)?;
                (g, scope.crs_str("MaskInverted").as_deref() == Some("true"))
            }
        };
        parsed.push((c, geometry, inverted));
    }
    let base_index = parsed.iter().position(|(c, _, _)| c.start == base_start)?;
    let (_, mut mask, base_inv) = parsed.remove(base_index);
    let mut inverted = if matches!(mask, MaskGeometry::Brush { .. } | MaskGeometry::AiMask { .. }) {
        false
    } else { base_inv };
    if let Some(whole) = intended_inverted
        && let Some(own) = mask_intent_attr(correction_tag, "BaseInverted", intent_declared)
            .and_then(|v| v.parse::<bool>().ok())
        && whole ^ own == base_inv
        // A replaced radial may leave old editing metadata on a linear.
        // Linear has no geometry-owned inversion; reversing its asymmetric
        // handles is not a complement. Fall back to the native CRS flag.
        && (!own || !matches!(mask, MaskGeometry::Linear { .. }))
    {
        geometry_inversion(&mut mask, own);
        inverted = whole;
    }
    let components = parsed.into_iter().map(|(c, mut geometry, inv)| {
        let blend = Tag::new(c.tag).crs_str("MaskBlendMode")
            .and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let (native_mode, native_own) = brush_combine(blend, inv);
        let (mut mode, mut own) = projected_combine(native_mode, native_own, inverted);
        let mut component_inverted = false;
        // Trust editor metadata only while it still describes the CRS triple;
        // a Lightroom edit that changes the shape's composition wins over it.
        let intent = mask_intent_attr(c.tag, "Combine", intent_declared).and_then(|s| match s.as_ref() {
            "add" => Some(MaskCombine::Add),
            "subtract" => Some(MaskCombine::Subtract),
            "intersect" => Some(MaskCombine::Intersect),
            _ => None,
        });
        if let Some(wanted) = intent
            && let Some(wanted_own) = mask_intent_attr(c.tag, "OwnInverted", intent_declared)
                .and_then(|v| v.parse::<bool>().ok())
        {
            let wanted_component = mask_intent_attr(c.tag, "ComponentInverted", intent_declared)
                .and_then(|v| v.parse::<bool>().ok()).unwrap_or(false);
            let (p_mode, p_own) = projected_combine(wanted, wanted_own ^ wanted_component, inverted);
            let spelling = combine_spelling(p_mode, p_own);
            if spelling.0 == blend && spelling.1 == inv {
                mode = wanted;
                own = wanted_own;
                component_inverted = wanted_component;
            }
        }
        if matches!(geometry, MaskGeometry::Linear { .. }) {
            component_inverted ^= own;
        } else {
            geometry_inversion(&mut geometry, own);
        }
        MaskComponent { inverted: component_inverted, geometry, mode }
    }).collect();
    // Optional range component. Its head repeats `MaskInverted="true"` as part
    // of the intersect ENCODING (see `range_mask_xml`), so user intent is read
    // from the geometry component only — hence the `base_el`-anchored scan.
    //
    // R28 Batch-5 5d (F4 symptom B) — TWO scope defects, one fix. The search
    // used to run over the WHOLE correction segment for the first tag saying
    // `crs:What="Mask/RangeMask"`, so a Mask/RangeMask sitting anywhere —
    // nested inside a brush group's `crs:Masks`, inside an AI mask's
    // `crs:Gesture`, or even outside `crs:CorrectionMasks` entirely — was
    // attached to this correction as its range. And once found, the reader ran
    // from that offset to the END of the segment, so the `rdf:li` the colour
    // arm parses could come from a later component altogether.
    //
    // Both close by asking the ONE component list this correction owns
    // (`correction_mask_components`, the same answer `base_geometry_at` and the
    // parser above use) for a TOP-LEVEL range component, and reading it from
    // its own element. Real Lightroom is unaffected: it writes range masks as
    // members of `crs:CorrectionMasks` and nowhere else.
    let range = comps
        .iter()
        .find(|c| c.depth == 0 && c.what.as_ref() == "Mask/RangeMask")
        .and_then(|c| {
            let r = Scope::new(base_element(block, c.start));
            // STRICT token parse (`collect::<Option<…>>`), not filter_map: a
            // malformed token used to vanish, letting the remaining values shift
            // one field left and still pass the length check.
            if let Some(lum) = r.crs_str("LumRange") {
                let v: Option<Vec<f32>> =
                    lum.split_whitespace().map(|x| x.parse().ok()).collect();
                let v = v?;
                (v.len() == 4).then(|| RangeMask::Luminance {
                    lo_outer: v[0],
                    lo: v[1],
                    hi: v[2],
                    hi_outer: v[3],
                })
            } else if let Some(amount) = r.crs_f32("ColorAmount") {
                // PointModels entry: "r g b px py 0" (writer + LR convention).
                let li = owned_element_body(r.text(), "rdf:li").ok().flatten()?;
                let v: Option<Vec<f32>> =
                    li.split_whitespace().map(|x| x.parse().ok()).collect();
                let v = v?;
                (v.len() >= 5)
                    .then(|| RangeMask::Color { r: v[0], g: v[1], b: v[2], amount, px: v[3], py: v[4] })
            } else {
                None
            }
        });
    // Our own writer synthesises "AutoShade <n>" for unnamed masks (the
    // block above needs SOME CorrectionName) — importing that back as a
    // user-given name froze the placeholder and hid the localised
    // role/label. Round-trip it back to "unnamed". An unnamed ZONE goes out
    // under its role tag instead (`written_name`), and that placeholder is
    // also the LAST-RESORT role when the intent below is gone: a sidecar
    // Lightroom rewrote carries no `ash:` attribute at all (measured
    // 2026-09-12), and the payload did not exist before v1.3.1
    // (`payload::zone_name_role`).
    let placeholder_free = own
        .crs_str("CorrectionName")
        .map(|v| v.into_owned())
        .filter(|n| n.strip_prefix("AutoShade ").is_none_or(|rest| rest.parse::<u32>().is_err()))
        .unwrap_or_default();
    let (name, name_role) = payload::zone_name_role(placeholder_free, &mask);
    Some(LocalAdjustment {
        mask,
        range,
        name,
        amount: own.crs_f32("CorrectionAmount").unwrap_or(1.0),
        components,
        inverted,
        // Intent first — a document of ours says the role outright, and says
        // `custom` outright too — and the name's verdict only where no intent
        // survives at all.
        // A tag this build does not know (a NEWER build's role) reads as
        // `Custom`, which is how a pre-v1.14.0 build reads the portrait ones.
        role: match mask_intent_attr(correction_tag, "Role", intent_declared).as_deref() {
            Some(tag) => crate::recipe::MaskRole::from_tag(tag).unwrap_or(crate::recipe::MaskRole::Custom),
            None => name_role.unwrap_or(crate::recipe::MaskRole::Custom),
        },
        exposure_ev: own.crs_f32("LocalExposure2012").unwrap_or(0.0) * 4.0,
        contrast: q100("LocalContrast2012"),
        highlights: q100("LocalHighlights2012"),
        shadows: q100("LocalShadows2012"),
        whites: q100("LocalWhites2012"),
        blacks: q100("LocalBlacks2012"),
        clarity: q100("LocalClarity2012"),
        dehaze: q100("LocalDehaze"),
        texture: q100("LocalTexture"),
        // `q100` here is the MEASURED scale as of 2026-08-19, settled by two
        // Adobe-anchored pairs that no library file supplies: Adobe's own
        // shipped Soften Skin local preset (UI Sharpness +25 -> `0.25`) and
        // MIDI2LR's all-sliders-at-maximum dump (+100 -> `1`). The one wobble
        // on record — the controlled session's `P14.xmp` reading
        // `crs:LocalSharpness="0.803738"` against a requested +80, briefly
        // read as a falsification when the user recalled TYPING the value —
        // resolves the other way: the divisor a typed 80 would need (99.535)
        // puts +-100 at +-1.0047, outside the endpoint Adobe sits exactly on,
        // and arbitrary 5-6-decimal values are normal Lightroom output (10 of
        // 18 distinct public non-zero values; no quantisation lattice). So
        // 0.803738 is the slider at 80.37 and the recollection gives way —
        // user-accepted ruling, 2026-08-19. Evidence archive:
        // the R27 materials ledger's F3-web-evidence/ folder, outside the tree.
        // See docs/V2_PLAN.md §7 item 10 for the full adjudication.
        sharpness: q100("LocalSharpness"),
        saturation: q100("LocalSaturation"),
        // NOT `q100` — `crs:LocalHue` is the ONE local key measured off its
        // slider on a different scale (v0.32.0). The user's controlled
        // Lightroom export put the mask Hue slider at +50 and the sidecar came
        // back `crs:LocalHue="0.277778"` (`P14.xmp`, verbatim), and
        // 0.277778 × 180 = 50.00004 — no other simple scale lands on it (÷100
        // would read 27.8, ÷360 would read 100). The recipe's own domain is
        // unchanged at ±100 (`LocalAdjustment::hue`), so this is a boundary
        // conversion exactly like `crs:Feather`'s, not a widening.
        //
        // What is measured is the SCALE, not the meaning: what Lightroom does
        // with a +50 hue is its own model, and this engine keeps rendering the
        // value through `render::apply_masks`'s ±30° rotation — the same
        // honest split `texture` and local `sharpness` already carry.
        hue: q180("LocalHue"),
        temperature: q100("LocalTemperature"),
        tint: q100("LocalTint"),
        noise_reduction: q100("LocalLuminanceNoise"),
        // R25 P6: the four local point curves, read with the machinery the
        // global curves already use — `parse_curve` is
        // `owned_element_body` + `parse_curve_checked`, both name-matched and
        // both already hardened (a whitespace-spelled `<rdf:li >`, an
        // out-of-domain coordinate, an element that never closes). No second
        // parser, and no third scan of the segment.
        //
        // `parse_curve` swallows the `Err` half into an empty curve, which
        // would be silence — so `correction_value_reasons` runs the CHECKED
        // form over the same four keys and raises `LocalCurve` for exactly the
        // curves this line drops. The two must stay in step; the pair is
        // pinned by `an_unreadable_local_curve_is_named_not_swallowed`.
        // The four local point curves are child ELEMENTS of the correction, and
        // they read from its OWN scope for the same reason its sliders do (R28
        // Batch-5 5d): `correction_value_reasons` gates them there through
        // `parse_curve_checked`, and a gate and its reader that disagree about
        // scope are exactly the defect this batch closed one line up. A
        // `<crs:MainCurve>` nested inside a component is that component's, not
        // this correction's.
        main_curve: parse_curve(own.text(), "MainCurve"),
        red_curve: parse_curve(own.text(), "RedCurve"),
        green_curve: parse_curve(own.text(), "GreenCurve"),
        blue_curve: parse_curve(own.text(), "BlueCurve"),
        // color_gains / role are engine-only and never reach a sidecar.
        ..Default::default()
    })
}
