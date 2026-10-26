//! The reader's entry points (xmp_to_recipe and its clamped, diagnosed and per-photo forms) and the sidecar era markers.

use super::*;

// ───────────────────────── XMP → EditRecipe (reader) ─────────────────────────
//
// The inverse of [`recipe_to_xmp`], so a sidecar written earlier (by us or by
// Lightroom) can be loaded back into the editor. Scan-based like the eval
// harness's parser: classic-ACR values are flat `crs:Key="value"` attributes,
// verified against the user's real LR sidecars, so plain text scanning
// round-trips everything the writer emits without an XML dependency. Fields
// classic XMP cannot carry (bitmap masks, recolour gains, mask roles) simply
// don't come back — the app-internal recipe.json is the lossless sidecar; this
// reader is the recovery path when only an XMP exists.

/// The toolkit strings this app stamps into `x:xmptk`, and the ones it
/// stamped before the AutoShade rename.
///
/// These are ON-DISK FORMAT TOKENS, not display names: every sidecar this app
/// has ever written to a user's library carries one of the pre-rename
/// spellings, and [`is_autoshade_era2`] turns that token into a RENDERING
/// decision (era-1 Temperature is relative to the 5500 K anchor, era-2 is
/// absolute). Teaching the reader only the new spelling would have re-read
/// every existing era-2 sidecar as era-1 and silently shifted its white
/// balance. The writer stamps the current spelling; the readers accept both
/// spellings PERMANENTLY: sidecars on user disks never upgrade themselves, so
/// unlike the environment-name aliases this acceptance has no removal deadline.
const XMPTK_ERA1: &str = "AutoShade";
const XMPTK_ERA2: &str = "AutoShade 2";
const XMPTK_ERA1_PRE_RENAME: &str = "Autoshop";
const XMPTK_ERA2_PRE_RENAME: &str = "Autoshop 2";

/// AutoShade provenance: an ATTRIBUTE-shaped `x:xmptk = "AutoShade"` /
/// `x:xmptk='AutoShade'` match (either quote style, optional whitespace
/// around `=`, and either the current or the pre-rename toolkit name),
/// searched ONLY inside the `<x:xmpmeta …>` start tag — where
/// the attribute actually lives. The old raw-substring test both missed
/// semantically identical XML spellings and matched the literal anywhere in
/// the document (a foreign sidecar's comment could claim our provenance) —
/// and this boolean decides whether an As-Shot tint imports as a real edit.
pub(super) fn is_autoshade_sidecar(xmp: &str) -> bool {
    // COMMENT-AWARE scan for the first real `<x:xmpmeta` start tag: a plain
    // find lost to a forged tag in a LEADING comment, rfind to one in a
    // TRAILING comment. One pass skipping `<!-- … -->` spans settles both
    // (full XML parsing stays out of scope; this only gates the As-Shot
    // tint import).
    // BYTE scanning throughout: `&xmp[i + 1..]` PANICS when i+1 falls inside a
    // multi-byte char, and a file that opens with a UTF-8 BOM (EF BB BF) hits
    // that on the very first step. Every index this loop keeps lands on `<`
    // or just past `-->` — both ASCII — so the one str slice below is safe.
    let bytes = xmp.as_bytes();
    let mut i = 0usize;
    let mut tag_start: Option<usize> = None;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<!--") {
            match bytes[i + 4..].windows(3).position(|w| w == b"-->") {
                Some(end) => i += 4 + end + 3,
                None => break, // unterminated comment: nothing real follows
            }
        } else if bytes[i..].starts_with(b"<x:xmpmeta")
            && bytes
                .get(i + "<x:xmpmeta".len())
                .is_none_or(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/'))
        {
            // Name-boundary check: without it a preceding wrapper whose
            // element name merely STARTS with x:xmpmeta (`<x:xmpmetadata
            // x:xmptk="AutoShade">`) was accepted as the document tag.
            tag_start = Some(i);
            break;
        } else {
            // Advance to the next '<' (or end).
            match bytes[i + 1..].iter().position(|&c| c == b'<') {
                Some(off) => i += 1 + off,
                None => break,
            }
        }
    }
    let Some(tag_start) = tag_start else { return false };
    let tag = &xmp[tag_start..];
    let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
    let mut rest = tag;
    while let Some(i) = rest.find("x:xmptk") {
        let after = rest[i + "x:xmptk".len()..].trim_start();
        if let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let ours = |r: &str, q: char| {
                [XMPTK_ERA1, XMPTK_ERA2, XMPTK_ERA1_PRE_RENAME, XMPTK_ERA2_PRE_RENAME]
                    .iter()
                    .any(|n| r.strip_prefix(n).is_some_and(|t| t.starts_with(q)))
            };
            if v.strip_prefix('"').is_some_and(|r| ours(r, '"'))
                || v.strip_prefix('\'').is_some_and(|r| ours(r, '\''))
            {
                return true;
            }
        }
        rest = &rest[i + "x:xmptk".len()..];
    }
    false
}

/// Absolute-Kelvin era marker (`x:xmptk="AutoShade 2"`): documents whose
/// Temperature is ABSOLUTE (written by the anchored engine). We only ever
/// serialise the fixed form below, so an exact scan suffices; a hand-edited
/// whitespace variant merely misses the marker and falls back to the
/// old-era 5500 pin — the fail-safe direction (renders as the old engine
/// did) — never to a wrong absolute reinterpretation.
pub(super) fn is_autoshade_era2(xmp: &str) -> bool {
    [XMPTK_ERA2, XMPTK_ERA2_PRE_RENAME].iter().any(|n| {
        xmp.contains(&format!(r#"x:xmptk="{n}""#)) || xmp.contains(&format!("x:xmptk='{n}'"))
    })
}

/// Upgrade an old AutoShade era marker on a MERGED document: the merge just
/// rewrote every owned WB attribute in absolute-Kelvin semantics, so leaving
/// `x:xmptk="AutoShade"` in place would make the next import pin the 5500
/// anchor onto absolute values. Foreign (Adobe) markers are untouched —
/// foreign import semantics are already absolute.
pub(super) fn upgrade_era_marker(doc: String) -> String {
    // The closing quote is part of the pattern, so an already-upgraded
    // "AutoShade 2" value can never prefix-match and double-upgrade. Both
    // era-1 spellings upgrade to the CURRENT era-2 name: a pre-rename era-1
    // document left un-upgraded would pin the 5500 anchor onto the absolute
    // values the merge just wrote.
    [XMPTK_ERA1, XMPTK_ERA1_PRE_RENAME].iter().fold(doc, |d, n| {
        d.replacen(&format!(r#"x:xmptk="{n}""#), &format!(r#"x:xmptk="{XMPTK_ERA2}""#), 1)
            .replacen(&format!("x:xmptk='{n}'"), &format!("x:xmptk='{XMPTK_ERA2}'"), 1)
    })
}

/// Parse an ACR / Lightroom `.xmp` sidecar into an [`EditRecipe`] — the inverse
/// of [`recipe_to_xmp`] over every field classic XMP can carry. Absent keys stay
/// neutral, so a foreign XML parses to (nearly) a default recipe rather than
/// erroring. Two provenance rules keep a FOREIGN sidecar honest:
///   * `Temperature` counts only under `WhiteBalance="Custom"` — an "As Shot"
///     sidecar records the CAMERA's Kelvin, which is not an edit, and importing
///     it would visibly shift the render.
///   * Same for `Tint`, except sidecars we wrote ourselves (marked
///     `x:xmptk="AutoShade"`), whose Tint is always a real edit.
///
/// The returned recipe is clamped before it crosses the parser boundary, using
/// the same ranges and size caps as every other untrusted recipe input.
pub fn xmp_to_recipe(xmp: &str) -> EditRecipe {
    xmp_to_recipe_clamped(xmp).0
}

/// Path-aware import for an XMP sidecar that may reference a sibling `.acr`.
/// The diagnostic's photo is the single source of both discovery identity and
/// attribution, matching the rest of the injected diagnostic discipline.
pub fn xmp_to_recipe_with_diag(xmp: &str, diag: &crate::diag::Diag<'_>) -> EditRecipe {
    xmp_to_recipe_clamped_with_diag(xmp, diag).0
}

/// Silent path-aware import used for equality/probe work whose caller owns a
/// separate disclosure channel.
pub fn xmp_to_recipe_for_photo(xmp: &str, photo: &std::path::Path) -> EditRecipe {
    xmp_to_recipe_clamped_impl(xmp, Some(photo), None).0
}

/// [`xmp_to_recipe`] plus WHAT THE CLAMP COST — the door for every surface
/// that DISCLOSES import loss.
///
/// The clamp result used to be dropped on the floor here (`r.clamp();`), and
/// that single discarded value made the whole import-side truncation channel
/// silent: the GUI's own second clamp (`bin/gui/persist.rs`,
/// `bin/gui/export.rs`) ran on the already-cut recipe and correctly reported
/// nothing, so a sidecar that lost 131 KB of brush dabs and a mask component
/// on the way in looked exactly like one that arrived whole (R28 2b,
/// adjudication F5). The summary is a property of the READ, so it is produced
/// where the read is and returned rather than re-derived by anyone.
///
/// Callers that only want the recipe keep using [`xmp_to_recipe`] — the
/// unclamped-summary form stays the exception, not the default, so no caller
/// is obliged to handle a value it has no surface for.
pub fn xmp_to_recipe_clamped(xmp: &str) -> (EditRecipe, crate::recipe::ClampSummary) {
    xmp_to_recipe_clamped_impl(xmp, None, None)
}

pub fn xmp_to_recipe_clamped_with_diag(
    xmp: &str,
    diag: &crate::diag::Diag<'_>,
) -> (EditRecipe, crate::recipe::ClampSummary) {
    xmp_to_recipe_clamped_impl(xmp, diag.photo(), Some(diag))
}

pub(super) fn xmp_to_recipe_clamped_impl(
    xmp: &str,
    photo: Option<&std::path::Path>,
    diag: Option<&crate::diag::Diag<'_>>,
) -> (EditRecipe, crate::recipe::ClampSummary) {
    let (mut r, dropped) = read_document(xmp, photo, diag);
    // A reading with no photo has no develop dir, so nothing can say which
    // folder a relative or bare raster name means, and a later save would
    // anchor it to whichever develop the recipe lands in — a different
    // photo's file of the same name. Such a reading never hands one out,
    // silent or disclosing. Done HERE, once the whole reading is finished,
    // and not inside it: the payload restore's own re-read of the projection
    // (`read_document`) must keep comparing bare names against bare names.
    // The line reaches a reader that has a channel; a silent read has none.
    if photo.is_none() {
        let n = crate::store::disable_unanchored_mask_paths(&mut r);
        if n > 0
            && let Some(d) = diag
        {
            d.warn(format!(
                "the sidecar's {n} raster reference(s) were disabled — it was read without its \
                 photo, so there is no develop to anchor them to; the masks and layers that use \
                 them render inert"
            ));
        }
    }
    (r, dropped)
}

/// The reading itself, raster names as the document gives them. Only the
/// payload restore calls it directly, for its re-read of the projection;
/// every other reader goes through [`xmp_to_recipe_clamped_impl`].
pub(super) fn read_document(
    xmp: &str,
    photo: Option<&std::path::Path>,
    diag: Option<&crate::diag::Diag<'_>>,
) -> (EditRecipe, crate::recipe::ClampSummary) {
    if xmp.len() > MAX_XMP_BYTES {
        return (EditRecipe::default(), crate::recipe::ClampSummary::default());
    }
    // The disclosure scan answers a namespace conflict with "its camera-raw
    // settings were not imported" — and every restore surface pairs the two
    // calls. This reader kept importing anyway, reading properties through
    // the very prefixes the gate just declared unreliable: the two faces of
    // one document contradicted each other. Neutral is the only import the
    // disclosure sentence keeps honest.
    if xmlns_conflict(xmp).is_some() {
        return (EditRecipe::default(), crate::recipe::ClampSummary::default());
    }
    let ours = is_autoshade_sidecar(xmp);


    // EVERY setting below is read from this Description's OWN scope, never
    // the raw document: a nested creative Look carries owned-LOOKING crs
    // properties, and the flat scanners answered from them whenever the top
    // level omitted the key (see `crs_own_scope`). The provenance reads
    // (`is_autoshade_sidecar` above, the rationale comment below) deliberately
    // stay on the whole document — they live OUTSIDE the Description.
    let scope = crs_own_scope(xmp);
    // …and that scope is a TYPE now (R28 Batch-5 5d): `Scope` says "subtree,
    // first match wins", which is what a whole-document read means and what a
    // per-element read must never be handed.
    let scope = Scope::new(scope.as_ref());
    // Any EXPLICIT white balance is a user decision, not the camera's: ACR
    // writes Daylight / Cloudy / Shade / Tungsten / Fluorescent / Flash — each
    // with its own Temperature+Tint — and accepting only "Custom" imported all
    // of them as as-shot, dropping a WB the photographer had chosen. (Absent
    // is treated as explicit for the same reason `eval`/`style` do: a sidecar
    // carrying Temperature without the mode is still a stated value.)
    let custom_wb = scope.crs_str("WhiteBalance").as_deref() != Some("As Shot");
    let f = |k: &str| scope.crs_f32(k).unwrap_or(0.0);
    // The engine's own neutrals, for the ONE block whose neutral is not zero
    // (de-fringe, R25 B3) — `f`'s zero fallback would invent a hue window.
    let dflt = EditRecipe::default();
    // The SOURCE frame this document's coordinates are measured against, and
    // the turn that carries them into the frame this engine displays — read
    // ONCE and served to both geometry decodes (crop and masks), because they
    // are the same encoding in the same frame (`FrameAspect`).
    let frame = FrameAspect::from_xmp(xmp);
    // …and WHICH WAY IS UP, which is a separate read (`declared_orientation`
    // says why) and needs the photograph as well as the document. The
    // photograph's half is its un-rotated rectangle plus its own EXIF turn,
    // through the memo `photo_frame_aspect` and `migrate_recipe_coord_frame`
    // already share — one header walk per photo per process, not one per
    // consumer. Skipped entirely when the document declares no orientation,
    // because then the law has nothing to solve and the read would buy
    // nothing.
    let declared = declared_orientation(xmp);
    let source = declared
        .and(photo)
        .filter(|p| crate::decode::is_raw(p))
        .and_then(crate::pipeline::source_frame_memo);
    let turn = honour_declared_orientation(declared, source.map(|(_, exif)| exif));
    if let (Some(refused), Some(diag)) = (turn.refused, diag) {
        // NAMED, never silently mapped to the nearest rotation: the value, the
        // reason it cannot be delivered, and what was kept instead.
        diag.warn(format!(
            "tiff:Orientation=\"{}\" mirrors the frame and no quarter turn can deliver it — \
             the sidecar's orientation was refused and the photo's own EXIF orientation kept",
            refused.to_u16()
        ));
    }
    // A document that declares only an orientation — 154 of the 175 sidecars
    // in the operator's library — had its geometry folded through the
    // PHOTOGRAPH's rectangle on the way out (`merge_frame`'s Keep and
    // Orientation arms), so that rectangle is the frame it must be read in.
    // Without it (until 2026-09-24) the crop's verbatim arm read folded
    // corners as unfolded and a rotated radial decoded as unrotated, with a
    // rotation loss disclosed that had not happened.
    let frame = frame.or_else(|| frame_fallback(source, &turn));
    // Adobe applies `CropAngle` only under `HasCrop="True"` — importing a
    // stale angle from a DISABLED crop activated a straighten Adobe itself
    // does not render.
    let crop_read = read_crop(scope, frame, ours);

    let mut hsl = Hsl::default();
    for (i, band) in crate::recipe::HSL_BANDS.iter().enumerate() {
        hsl.hue[i] = f(&format!("HueAdjustment{band}"));
        hsl.saturation[i] = f(&format!("SaturationAdjustment{band}"));
        hsl.luminance[i] = f(&format!("LuminanceAdjustment{band}"));
    }
    // A wheel whose HUE is present but unreadable must not keep its paired
    // saturation: the generic zero fallback turned a corrupt hue into finite
    // 0 (= red), so `ShadowHue="bogus"` + a valid Saturation of 50 imported
    // as a STRONG RED grade while the disclosure said "restored as neutral"
    // (16-lane scan L05). The hue itself is already named by
    // `unparsable_crs_numbers`; zeroing the sat makes the wheel colourless.
    let wheel_sat = |hue_key: &str, sat_key: &str| -> f32 {
        if scope.crs_str(hue_key).is_some() && scope.crs_f32(hue_key).is_none() {
            0.0
        } else {
            f(sat_key)
        }
    };
    let color_grade = ColorGrade {
        shadow_hue: f("SplitToningShadowHue"),
        shadow_sat: wheel_sat("SplitToningShadowHue", "SplitToningShadowSaturation"),
        shadow_lum: f("ColorGradeShadowLum"),
        midtone_hue: f("ColorGradeMidtoneHue"),
        midtone_sat: wheel_sat("ColorGradeMidtoneHue", "ColorGradeMidtoneSat"),
        midtone_lum: f("ColorGradeMidtoneLum"),
        highlight_hue: f("SplitToningHighlightHue"),
        highlight_sat: wheel_sat("SplitToningHighlightHue", "SplitToningHighlightSaturation"),
        highlight_lum: f("ColorGradeHighlightLum"),
        global_hue: f("ColorGradeGlobalHue"),
        global_sat: wheel_sat("ColorGradeGlobalHue", "ColorGradeGlobalSat"),
        global_lum: f("ColorGradeGlobalLum"),
        blending: scope.crs_f32("ColorGradeBlending").unwrap_or(ColorGrade::default().blending),
        balance: f("SplitToningBalance"),
    };
    // Our own comment header carries the AI provenance back (best-effort; the
    // escaped rationale cannot contain a raw "-->", so the scan is unambiguous).
    let (rationale, confidence) = block_between(xmp, "AI rationale: ", " -->")
        .and_then(|body| {
            let cut = body.rfind(" (confidence ")?;
            let conf =
                body[cut + " (confidence ".len()..].trim_end_matches(')').parse::<f32>().ok()?;
            Some((xml_unescape(&body[..cut]).into_owned(), conf))
        })
        .unwrap_or_default();

    // Camera Calibration (v1.5.0): Lightroom's unprefixed seven, whose absent
    // keys are its zeros, in `EditRecipe::calibration` order.
    let cal = CALIBRATION_CRS.map(&f);
    let mut r = EditRecipe {
        temperature_k: custom_wb.then(|| scope.crs_f32("Temperature")).flatten(),
        tint: if custom_wb || ours { f("Tint") } else { 0.0 },
        exposure_ev: f("Exposure2012"),
        contrast: f("Contrast2012"),
        highlights: f("Highlights2012"),
        shadows: f("Shadows2012"),
        whites: f("Whites2012"),
        blacks: f("Blacks2012"),
        clarity: f("Clarity2012"),
        dehaze: f("Dehaze"),
        vibrance: f("Vibrance"),
        saturation: f("Saturation"),
        texture: f("Texture"),
        // The parametric tone curve (v1.5.0). The regions fall back to 0 like
        // every Basic slider; the splits to ADOBE'S DEFAULTS, the de-fringe
        // rule — `f` would import a split at 0 from a document that never
        // named one, and the next save would write that invented split into
        // the sidecar beside the RAW.
        param_shadows: f("ParametricShadows"),
        param_darks: f("ParametricDarks"),
        param_lights: f("ParametricLights"),
        param_highlights: f("ParametricHighlights"),
        param_shadow_split: scope.crs_f32("ParametricShadowSplit").unwrap_or(dflt.param_shadow_split),
        param_midtone_split: scope.crs_f32("ParametricMidtoneSplit").unwrap_or(dflt.param_midtone_split),
        param_highlight_split: scope
            .crs_f32("ParametricHighlightSplit")
            .unwrap_or(dflt.param_highlight_split),
        // The nine CARRIED effects (R25 B2). `f` answers 0 for an absent key,
        // which is exactly this batch's neutral — so a sidecar that names none
        // of them still imports as a no-op, and one that names a real vignette
        // brings all six values with it.
        post_crop_vignette: f("PostCropVignetteAmount"),
        post_crop_vignette_mid: f("PostCropVignetteMidpoint"),
        post_crop_vignette_feather: f("PostCropVignetteFeather"),
        post_crop_vignette_round: f("PostCropVignetteRoundness"),
        post_crop_vignette_style: f("PostCropVignetteStyle"),
        post_crop_vignette_hl: f("PostCropVignetteHighlightContrast"),
        grain: f("GrainAmount"),
        grain_size: f("GrainSize"),
        grain_rough: f("GrainFrequency"),
        // F8 (v1.5.0): HDR edit mode, its headroom and the seven SDR-rendition
        // controls. `f` answers 0 for an absent key, which is the neutral for
        // all eight numbers; the mode is Lightroom's "0"/"1" spelling, so
        // anything that is not "1" — including a missing key — is OFF.
        hdr_edit: scope.crs_str("HDREditMode").as_deref().map(str::trim) == Some("1"),
        hdr_max_ev: f("HDRMaxValue"),
        sdr_blend: f("SDRBlend"),
        sdr_brightness: f("SDRBrightness"),
        sdr_contrast: f("SDRContrast"),
        sdr_highlights: f("SDRHighlights"),
        sdr_shadows: f("SDRShadows"),
        sdr_whites: f("SDRWhites"),
        sdr_clarity: f("SDRClarity"),
        cal_shadow_tint: cal[0],
        cal_red_hue: cal[1],
        cal_red_sat: cal[2],
        cal_green_hue: cal[3],
        cal_green_sat: cal[4],
        cal_blue_hue: cal[5],
        cal_blue_sat: cal[6],
        hsl,
        // The B&W switch (v1.5.0): Lightroom's `"True"`, and the `true` / `1`
        // spellings a crs boolean takes in the wild, on THIS Description — a
        // creative Look's own switch belongs to the Look. The eight-band mixer
        // is read just below.
        convert_to_grayscale: matches!(
            scope.crs_str("ConvertToGrayscale").as_deref().map(str::trim),
            Some("True") | Some("true") | Some("1")
        ),
        color_grade,
        // 1:1. Lightroom's Detail > Sharpening "Amount" slider runs 0..150 and
        // `crs:Sharpness` stores that UI number unscaled — 15 real sidecars in
        // 2 unrelated repositories, 2 camera bodies and 2 Lightroom generations
        // carry `crs:Sharpness="150"` (`crs:Version` 15.3/17.2), the maximum in
        // 566 observed occurrences (web survey, 2026-08-18). The reader's old
        // ×1.5 and the writer's ×⅔ were both built on a 0..100 ceiling that
        // does not exist: `Sharpness="40"` used to import as 60, `"150"` came
        // in as 225 and was clamped back to 150 while ALSO being reported as
        // an unparsable number, and a rendered 60 was written back as 40.
        sharpening: f("Sharpness"),
        noise_reduction: f("LuminanceSmoothing"),
        // The eight detail axes (R25 B3). `f` answers 0 for an absent key,
        // which IS this block's neutral — an untouched sidecar still imports
        // as a no-op, and one that names a real sharpening radius brings the
        // whole triple with it. A companion stated AT 0 is told apart from an
        // absent one just below (`explicit_zero`).
        sharpen_radius: f("SharpenRadius"),
        sharpen_detail: f("SharpenDetail"),
        sharpen_mask: f("SharpenEdgeMasking"),
        nr_detail: f("LuminanceNoiseReductionDetail"),
        nr_contrast: f("LuminanceNoiseReductionContrast"),
        color_nr: f("ColorNoiseReduction"),
        color_nr_detail: f("ColorNoiseReductionDetail"),
        color_nr_smooth: f("ColorNoiseReductionSmoothness"),
        lens_vignette: f("VignetteAmount"),
        lens_vignette_mid: scope.crs_f32("VignetteMidpoint").unwrap_or(50.0),
        lens_distortion: f("LensManualDistortionAmount"),
        // Absent is NOT zero for these two — 100 is Lightroom's neutral, and
        // reading an absent key as 0 would import "profile correction off"
        // from a document that never mentioned the profile at all. Same shape
        // as the de-fringe windows below, and for the same reason.
        lens_profile_distortion_scale: scope
            .crs_f32("LensProfileDistortionScale")
            .unwrap_or(dflt.lens_profile_distortion_scale),
        lens_profile_vignetting_scale: scope
            .crs_f32("LensProfileVignettingScale")
            .unwrap_or(dflt.lens_profile_vignetting_scale),
        perspective_vertical: f("PerspectiveVertical"),
        perspective_horizontal: f("PerspectiveHorizontal"),
        perspective_rotate: f("PerspectiveRotate"),
        // 100 is the neutral, so an absent key means "the frame as it is" and
        // NOT "scale to nothing" — the lens-profile strengths' rule again.
        perspective_scale: scope
            .crs_f32("PerspectiveScale")
            .unwrap_or(dflt.perspective_scale),
        perspective_aspect: f("PerspectiveAspect"),
        perspective_x: f("PerspectiveX"),
        perspective_y: f("PerspectiveY"),
        perspective_upright: f("PerspectiveUpright"),
        upright_transform: upright_matrices(scope),
        crop_constrain_to_warp: matches!(
            scope.crs_str("CropConstrainToWarp").as_deref().map(str::trim),
            Some("1") | Some("true") | Some("True")
        ),
        ca_r: f("ChromaticAberrationR"),
        ca_b: f("ChromaticAberrationB"),
        // A FLAG: Lightroom writes 0/1, and "true" is the other spelling in
        // the wild for a crs boolean — both are accepted, anything else (and
        // absence) is off.
        auto_lateral_ca: matches!(
            scope.crs_str("AutoLateralCA").as_deref().map(str::trim),
            Some("1") | Some("true") | Some("True")
        ),
        // De-fringe, the ONE block that falls back to ADOBE'S DEFAULT rather
        // than to zero. `f` answers 0 for an absent key, and taking that here
        // would import a hue window of 0..0 from a document that never
        // mentioned one — the photo would stop being a no-op, and the next
        // save would write that invented window into the sidecar beside the
        // RAW. `EditRecipe::default()` holds Adobe's 30/70 and 40/60, so a
        // document with no de-fringe block comes back exactly neutral.
        defringe_purple: scope.crs_f32("DefringePurpleAmount").unwrap_or(dflt.defringe_purple),
        defringe_purple_lo: scope.crs_f32("DefringePurpleHueLo").unwrap_or(dflt.defringe_purple_lo),
        defringe_purple_hi: scope.crs_f32("DefringePurpleHueHi").unwrap_or(dflt.defringe_purple_hi),
        defringe_green: scope.crs_f32("DefringeGreenAmount").unwrap_or(dflt.defringe_green),
        defringe_green_lo: scope.crs_f32("DefringeGreenHueLo").unwrap_or(dflt.defringe_green_lo),
        defringe_green_hi: scope.crs_f32("DefringeGreenHueHi").unwrap_or(dflt.defringe_green_hi),
        // Both halves come out of ONE decode (R27, `lr_to_engine_crop`): the
        // rectangle and the tilt are two faces of one rotated-corner encoding,
        // and reading either without the other is what made every straightened
        // import wrong by 2 × CropAngle on top of a frame error.
        straighten_deg: crop_read.straighten_deg() as f32,
        crop: crop_read.crop(),


        tone_curve: parse_curve(scope.text(), "ToneCurvePV2012"),
        red_curve: parse_curve(scope.text(), "ToneCurvePV2012Red"),
        green_curve: parse_curve(scope.text(), "ToneCurvePV2012Green"),
        blue_curve: parse_curve(scope.text(), "ToneCurvePV2012Blue"),
        point_colors: parse_point_colors(scope.text()),
        masks: parse_masks_with_source(scope.text(), ours, frame, photo, diag),
        // v1.5.0 F9. The whole document, not `scope` — see the reader.
        retouch: parse_retouch_areas(xmp),

        // The PASS-THROUGH blocks (R25 B4), read as STRINGS and stored
        // verbatim. `crs_str` already reads BOTH spellings — the
        // Description's own attribute and the property-element child — so
        // this loop covers the element form with no second scanner (the R24
        // round-end MED-1 lesson: a third scan arm is how the two forms drift
        // apart). An absent key stays absent from the map, which is how the
        // writer knows not to invent it.
        //
        // Read from `scope`, like every other setting: a creative Look nests
        // its own baked `crs:CameraProfile`, and the flat scan would have
        // imported the PROFILE's name whenever the top level omitted one.
        passthrough: PASSTHROUGH_CRS
            .iter()
            .filter_map(|k| scope.crs_str(k).map(|v| ((*k).to_string(), v.into_owned())))
            .collect(),

        // v1.5.0 F7. `scope`, not the whole document, for the reason the
        // comment above gives twice over: a creative Look nests its OWN
        // `crs:CameraProfile`, and a flat scan would import the Look's base
        // profile whenever the top level omitted one.
        camera_profile: scope.crs_str("CameraProfile").unwrap_or_default().into_owned(),
        look: read_creative_look(xmp),

        rationale,
        confidence,
        ..Default::default()
    };
    // The B&W mixer's eight bands (v1.5.0), through the one door the panel's
    // rows share, in the band order the writer spells them; an absent key is
    // the band's zero.
    for (i, band) in crate::recipe::HSL_BANDS.iter().enumerate() {
        if let Some(slot) = r.gray_mixer_mut(i) {
            *slot = f(&format!("GrayMixer{band}"));
        }
    }
    // A COMPANION key present at 0 (`recipe::LR_COMPANION_DEFAULTS`) is a
    // VALUE the document states — none of those controls defaults to 0 on a
    // RAW (Sharpness does on a JPEG, where the stated 0 and the default
    // agree), so Lightroom writes `SharpenDetail="0"` only for a Detail set
    // to 0 — while
    // `f` above folds a missing key into the same number. Keep the difference
    // (`EditRecipe::explicit_zero`): the engine renders the absent one at
    // Lightroom's default and the stated one at zero, as Lightroom does. Each
    // key's spelling comes from its registry row, never a second copy; an
    // unparsable value stays "absent", as `f` already has it.
    r.explicit_zero = crate::recipe::LR_COMPANION_DEFAULTS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| {
            crate::advisor::catalogue::global_control(name)
                .and_then(|c| c.crs.attr())
                .and_then(|key| scope.crs_f32(key))
                .is_some_and(|v| v == 0.0)
        })
        .map(str::to_string)
        .collect();
    r.explicit_zero.sort();
    // PROVENANCE RULE 3 (WB-anchor era): a sidecar WE wrote before the
    // absolute-Kelvin engine (x:xmptk="AutoShade", no era-2 marker) carries a
    // Temperature that was tuned RELATIVE to the historical 5500 K anchor.
    // Pin the engine anchor there — the honest encoding of that provenance —
    // so every stamp-if-None call site leaves it alone and the develop
    // renders exactly as it was tuned. Foreign sidecars (Lightroom's
    // Temperature is absolute) and era-2 documents stay unpinned: the caller
    // stamps the camera's real anchor. The pin deliberately leaves
    // as_shot_tint None — "anchor known, camera unknown" — which is also what
    // gates the as-shot caption off for these photos.
    if ours && !is_autoshade_era2(xmp) && r.temperature_k.is_some() {
        r.as_shot_k = Some(5500.0);
    }
    // SOURCE FRAME → DISPLAY FRAME (R27 A7, `P1-portrait-mask-frame.md` §5).
    // Every `crs:` coordinate above — crop rectangle and mask geometry alike —
    // was decoded in the frame the document declares, which for a portrait
    // capture is the UN-ROTATED sensor array (9504 × 6336) while this engine
    // renders the turned one. `orient_recipe_coords` is the algebra that moves
    // a whole recipe between those two frames; it already exists for the
    // `coord_era` migration, and running it HERE is what makes an imported
    // recipe's `coord_era` stamp true instead of a label on sensor-frame
    // numbers. It moves the crop, every mask box, the range-mask sample point
    // and — under a mirror — the straighten's sign, all in one pass.
    //
    // A document whose declared orientation already IS the photograph's own
    // turns nothing, so this stays inert for every frame the twelve-export
    // experiment, the 16 reference sidecars and the 169 pairs of the reference
    // library contain (that census found 0 sidecars declaring an orientation
    // their RAW's EXIF disagrees with; the me6-2026-09 pack is the measured
    // exception, and it is 46 for 46).
    //
    // The turn is `HonouredTurn`'s, not `frame`'s: they are the same value
    // whenever both exist, and the two come apart exactly where the law has
    // something to say — a refused mirror, and a document that declares an
    // orientation but no rectangle.
    //
    // The frame handed along is the SOURCE rectangle, because that is the one
    // these coordinates are still in (R29 C1, `render::CoordFrame`): a brush's
    // radii are in width units, so the rewrite has to divide by the width the
    // document declares — or, when it declares none, by the photograph's own,
    // which is the rectangle Lightroom measured against and the one
    // `merge_frame`'s third arm already declares on the way back out.
    crate::render::orient_recipe_coords(
        &mut r,
        turn.turn,
        frame.and_then(|f| crate::render::CoordFrame::new(f.w, f.h)).or_else(|| {
            source.and_then(|((w, h), _)| crate::render::CoordFrame::new(w as f64, h as f64))
        }),
    );
    // …and the turn that makes the render DELIVER the frame those coordinates
    // are now in. Zero for every document whose declaration agrees with the
    // capture, which is what keeps an un-rotated recipe's JSON byte-identical
    // (`quarter_turns` is the one `skip_serializing_if` field).
    r.quarter_turns = turn.quarter_turns;
    // Independent scalar controls saturate at the recipe contract and are
    // named by `unparsable_crs_numbers` when that changes a foreign value.
    // Compound crop and mask data are rejected earlier because clamping only
    // part of their geometry would silently change coverage.
    //
    // The summary RIDES OUT rather than being discarded (R28 2b): what the
    // SIZE caps cut — dab bytes, strokes, curve knots — is loss no other
    // channel here can see, because `import_losses` reads the document and
    // this reads the recipe the document produced.
    //
    // THE PAYLOAD (v1.3.1). Everything above is what the `crs:` settings say,
    // which is where Lightroom's edits live and the only thing a document
    // this app never wrote can say. A document this app DID write also
    // carries the develop itself, exactly, under its own namespace — and a
    // Lightroom rewrite preserves that (measured 2026-09-12). The two are
    // reconciled leaf by leaf: Lightroom's value where Lightroom edited, the
    // exact one everywhere else (`payload::restore`). A payload this build
    // cannot read is disclosed and the `crs:` reading above stands alone.
    // Rasters are placed beside the develop only on a DISCLOSING read: the
    // silent probe readers compare and never write.
    if let Some(found) = payload::find(xmp) {
        match found {
            Ok(p) => {
                let develop_dir = photo.map(crate::store::develop_dir);
                r = payload::restore(
                    r,
                    p,
                    frame,
                    photo,
                    develop_dir.as_deref(),
                    diag.is_some(),
                    diag,
                );
            }
            Err(why) => {
                if let Some(d) = diag {
                    d.warn(format!(
                        "the sidecar carries an AutoShade payload this build could not read \
                         ({why}) — the develop was imported from its camera-raw settings alone"
                    ));
                }
            }
        }
    }
    let dropped = r.clamp();
    (r, dropped)
}
