//! The AutoShade PAYLOAD: the whole develop, carried inside the sidecar under
//! this app's own XMP namespace so that a Lightroom rewrite cannot lose it.
//!
//! **What the `crs:` projection alone could never carry back.** A sidecar is
//! Lightroom's document, and the settings this writer owns are re-serialised
//! from Lightroom's own model every time Lightroom touches the file: the
//! `ash:` intent attributes inside `crs:MaskGroupBasedCorrections` are dropped
//! (so a zone's role came back as `Custom`), unknown `crs:` attributes and
//! elements are dropped, and everything classic XMP has no encoding for —
//! bitmap tiles, the 12×8×8 colour field, a muted mask, the alpha the zoned fit
//! measured its numbers against — was never in the file to begin with. A
//! develop that went through Lightroom therefore came back as an
//! approximation of itself, and `recipe.json` beside the store was the only
//! exact record.
//!
//! **What was measured (Lightroom 9.4, 2026-09-12).** One rewrite of a probe
//! sidecar answered which spellings survive. Root-level properties in a
//! foreign namespace survive BYTE-EXACT — a 15 KB attribute, an `rdf:Bag`, a
//! struct, an `rdf:Seq` of structs totalling 250 KB — re-serialised into XMP's
//! compact form: simple properties become attributes on the top-level
//! `rdf:Description`, struct fields become attributes on the struct element
//! (`<rdf:li asr:Name="…" asr:Data="…"/>`), and every `xmlns:` declaration is
//! hoisted to the root. Unknown `crs:` items are dropped; ours are not. The
//! fixtures under `AUTOSHADE_LR_PAYLOAD_FIXTURES` are that rewrite, verbatim.
//!
//! **What this module does with that.** The writer puts the recipe — the
//! develop exactly as the app holds it, in the display frame, raster paths
//! reduced to bare file names — on the root `rdf:Description` as one
//! compressed attribute (`asr:Recipe`, zlib + base64, with `asr:RecipeCrc32`
//! over the JSON bytes), and the rasters nothing can re-derive (bitmap tiles,
//! zone alphas) as an `<asr:Rasters>` sequence of `{Name, Crc32, Data}`
//! structs, written in the compact form Lightroom itself would rewrite them
//! into. The reader, having decoded the `crs:` settings the ordinary way,
//! finds the payload by the URI its prefix is bound to (never by the prefix
//! itself), verifies it, and RECONCILES the two: Lightroom's edits win where
//! Lightroom edited, the payload's exact values are restored everywhere else.
//! The rule is measured, not guessed — see [`restore`].

use super::{
    element_close_start, next_xml_attribute, next_xml_tag, owned_element_body_span, tag_name,
    xml_attr_escape, xml_attribute_raw, xml_unescape, FrameAspect, MaskLoss, MaskLossReason,
};
use crate::recipe::{EditRecipe, MaskGeometry, MaskRole};
use base64::Engine as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

/// The namespace the payload lives in. A URI, not a prefix: XML namespace
/// scoping means the prefix is whatever the document binds to this string,
/// and the reader looks the prefix UP rather than assuming `asr`.
pub(crate) const PAYLOAD_URI: &str = "https://autoshade.dev/ns/recipe/1.0/";
/// The prefix this writer binds, unless a merge base already binds it to some
/// other URI (see [`prefix_for`]).
pub(crate) const PAYLOAD_PREFIX: &str = "asr";
/// The payload's own format version — the `asr:Payload` attribute. A reader
/// that meets a version it does not know imports the `crs:` settings alone and
/// says so, rather than guessing at bytes it cannot interpret.
pub(crate) const PAYLOAD_FORMAT: &str = "1";
/// Raw bytes of raster the writer will embed in ONE sidecar. The zone alpha
/// and the fit's bitmap tiles are 8-bit greys of a few hundred KB together;
/// the cap exists for a user-supplied 61 MP bitmap mask, which would have put
/// the sidecar past [`super::MAX_XMP_BYTES`] and made it unreadable to this
/// very reader. Base64 costs ×4/3 on top, so 6 MiB raw stays well inside the
/// 16 MiB document cap with the rest of the file.
pub(crate) const RASTER_BUDGET: usize = 6 * 1024 * 1024;
/// Inflation cap for the recipe JSON — a hostile sidecar cannot make a 300-byte
/// attribute expand without bound.
const MAX_RECIPE_JSON: u64 = super::MAX_XMP_BYTES as u64;
/// How far a number may drift through Lightroom's rewrite (six decimals) and
/// this reader's own quantisation before it counts as an EDIT. Slider leaves
/// are integers or hundredths; geometry is in frame units, where 2e-4 is a
/// fifth of a pixel on a 1000-pixel frame.
const TOLERANCE: f64 = 2e-4;

// ───────────────────────── writer ─────────────────────────

/// The prefix a document being written should bind to [`PAYLOAD_URI`]:
/// `asr`, unless the merge base's opening tag already binds that prefix to a
/// FOREIGN URI (a binding to ours has been stripped by then, see
/// [`strip_root_attrs`]). Re-binding a prefix an element already binds is a
/// duplicate attribute, and a duplicate attribute is not XML — Lightroom would
/// refuse the whole file.
pub(super) fn prefix_for(tag: Option<&str>) -> String {
    let Some(tag) = tag else { return PAYLOAD_PREFIX.to_string() };
    let mut n = 0u32;
    loop {
        let candidate =
            if n == 0 { PAYLOAD_PREFIX.to_string() } else { format!("{PAYLOAD_PREFIX}{n}") };
        match xml_attribute_raw(tag, &format!("xmlns:{candidate}")) {
            Some((_, uri)) if xml_unescape(uri) != PAYLOAD_URI => n += 1,
            _ => return candidate,
        }
    }
}

/// The recipe as the payload carries it: every raster path reduced to its bare
/// file name. The sidecar travels with the photo; a machine-local absolute
/// path would be both useless elsewhere and a leak of this machine's layout,
/// while a bare name is exactly the store's own convention
/// (`store::relativize_mask_paths`) and what [`place_rasters`] resolves again.
pub(super) fn portable(r: &EditRecipe) -> EditRecipe {
    let mut p = r.clone();
    for m in &mut p.masks {
        for path in m.bitmap_paths_mut() {
            *path = bare_name(path);
        }
    }
    p
}

fn bare_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

/// The recipe's JSON bytes exactly as the payload carries them — one
/// producer, so the CRC the writer stamps and the CRC the reader checks are
/// over the same bytes.
fn recipe_json(r: &EditRecipe) -> Vec<u8> {
    serde_json::to_vec(&portable(r)).unwrap_or_default()
}

/// The root-tag half of the payload, in the writer's `\n    key="value"`
/// attribute spelling: the namespace binding, the format version, the writer,
/// the CRC and the recipe itself.
pub(super) fn root_attrs(r: &EditRecipe, prefix: &str) -> String {
    let json = recipe_json(r);
    if json.is_empty() {
        return String::new();
    }
    let crc = crc32fast::hash(&json);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    // Writing into a Vec cannot fail; an empty payload on the impossible arm
    // is still a document the reader refuses cleanly (CRC mismatch).
    let packed = z.write_all(&json).and_then(|()| z.finish()).unwrap_or_default();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&packed);
    format!(
        "\n    xmlns:{prefix}=\"{PAYLOAD_URI}\"\
         \n    {prefix}:Payload=\"{PAYLOAD_FORMAT}\"\
         \n    {prefix}:Writer=\"AutoShade {}\"\
         \n    {prefix}:RecipeCrc32=\"{crc:08x}\"\
         \n    {prefix}:Recipe=\"{b64}\"",
        env!("CARGO_PKG_VERSION"),
    )
}

/// The `<asr:Rasters>` child element: every raster the recipe cannot
/// re-derive — [`crate::recipe::LocalAdjustment::turnable_raster_paths_mut`]
/// is that set, the bitmap tiles and the zone alphas, and NOT an AI mask's
/// cached re-derivation — each once, as `{Name, Crc32, Data}` in the compact
/// struct form. A raster that cannot be read, or that would push the document
/// past [`RASTER_BUDGET`], is left out and named in the losses as
/// [`MaskLossReason::RasterNotEmbedded`]: the recipe still references it by
/// name, so a store that has the file keeps rendering, and a store that does
/// not is told why the mask is inert.
///
/// `photo` anchors a RELATIVE path to the photo's develop dir, the way the
/// store resolves one on load; an absolute path is read as given.
pub(super) fn rasters_element(
    r: &EditRecipe,
    prefix: &str,
    photo: Option<&Path>,
) -> (String, Vec<MaskLoss>) {
    let mut losses: Vec<MaskLoss> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut items = String::new();
    let mut used = 0usize;
    let develop = photo.map(crate::store::develop_dir);
    for (i, m) in r.masks.iter().enumerate() {
        let mask_name = super::written_name(i, m);
        let mut walk = m.clone();
        for path in walk.turnable_raster_paths_mut() {
            let name = bare_name(path);
            if seen.contains(&name) {
                continue;
            }
            let p = Path::new(path.as_str());
            let at: PathBuf = if p.is_relative() && let Some(d) = &develop {
                d.join(p)
            } else {
                p.to_path_buf()
            };
            let embedded = match std::fs::read(&at) {
                Ok(bytes) if bytes.len() <= RASTER_BUDGET - used && valid_name(&name) => {
                    used += bytes.len();
                    let crc = crc32fast::hash(&bytes);
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    items.push_str(&format!(
                        "     <rdf:li {prefix}:Name=\"{}\" {prefix}:Crc32=\"{crc:08x}\" {prefix}:Data=\"{b64}\"/>\n",
                        xml_attr_escape(&name),
                    ));
                    true
                }
                _ => false,
            };
            seen.push(name);
            if !embedded {
                losses.push(MaskLoss {
                    name: mask_name.clone(),
                    reason: MaskLossReason::RasterNotEmbedded,
                });
            }
        }
    }
    if items.is_empty() {
        return (String::new(), losses);
    }
    (
        format!("\n   <{prefix}:Rasters>\n    <rdf:Seq>\n{items}    </rdf:Seq>\n   </{prefix}:Rasters>"),
        losses,
    )
}

/// One file-name component and nothing else: what [`place_rasters`] will
/// join onto the develop dir, so a name that could climb out of it is refused
/// on BOTH sides — never written, never honoured.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.contains(['/', '\\', '\0'])
        && name != "."
        && name != ".."
        && Path::new(name).components().count() == 1
}

/// The prefix a document's OPENING TAG binds to [`PAYLOAD_URI`], if any.
fn bound_prefix_in_tag(tag: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(a) = next_xml_attribute(tag, &mut cursor) {
        if let Some(p) = a.name.strip_prefix("xmlns:")
            && xml_unescape(a.value) == PAYLOAD_URI
        {
            return Some(p.to_string());
        }
    }
    None
}

/// Strip a PREVIOUS payload's attributes from a merge base's opening tag —
/// the binding and every attribute under the prefix it bound — and answer
/// that old prefix, so the merge can strip the base's `<prefix:Rasters>`
/// element by its real name. The strip-then-append discipline of the `crs:`
/// keys, for the same reason: a leftover would be a second, stale answer in
/// the same tag.
pub(super) fn strip_root_attrs(tag: &mut String) -> Option<String> {
    let old = bound_prefix_in_tag(tag)?;
    let attr_prefix = format!("{old}:");
    let binding = format!("xmlns:{old}");
    loop {
        let mut cursor = 0;
        let mut span = None;
        while let Some(a) = next_xml_attribute(tag, &mut cursor) {
            if a.name == binding || a.name.starts_with(&attr_prefix) {
                span = Some(a.span);
                break;
            }
        }
        let Some(span) = span else { break };
        let mut left = span.start;
        while left > 0 && tag.as_bytes()[left - 1].is_ascii_whitespace() {
            left -= 1;
        }
        tag.replace_range(left..span.end, "");
    }
    Some(old)
}

// ───────────────────────── reader ─────────────────────────

/// One raster the payload carries, verified against its own CRC.
pub(crate) struct Raster {
    pub(crate) name: String,
    pub(crate) crc: u32,
    pub(crate) bytes: Vec<u8>,
}

/// A decoded payload: the recipe exactly as the app saved it, the rasters
/// that verified, and what did not (disclosed at restore time).
pub(crate) struct Payload {
    pub(crate) recipe: EditRecipe,
    pub(crate) rasters: Vec<Raster>,
    pub(crate) notes: Vec<String>,
}

/// The prefix ANY element of `xmp` binds to [`PAYLOAD_URI`]. Every producer
/// that hoists declarations hoists them to the root, so "any element" and "an
/// ancestor of the payload" differ only for a document built to make them
/// differ (the same simplification `intent_namespace_declared` takes).
pub(super) fn bound_prefix(xmp: &str) -> Option<String> {
    let mut at = 0;
    while let Some((start, gt, _)) = next_xml_tag(xmp, at) {
        if let Some(p) = bound_prefix_in_tag(&xmp[start..=gt]) {
            return Some(p);
        }
        at = gt + 1;
    }
    None
}

/// A simple property `{prefix}:{local}`, in either XMP spelling: an attribute
/// on some element (Lightroom's compact form, and this writer's) or a text
/// element (the form the preservation probe was written in). The crs
/// Description's own tag is consulted FIRST: a tool that splits namespaces
/// into separate Descriptions (exiftool) could leave a stale copy on another
/// one, and the one the merge rewrites is the one that is current.
pub(super) fn simple_property(xmp: &str, prefix: &str, local: &str) -> Option<String> {
    let key = format!("{prefix}:{local}");
    if let Some(start) = super::find_crs_description(xmp)
        && let Some((gt, _)) = super::scan_tag_end(xmp, start)
        && let Some((_, v)) = xml_attribute_raw(&xmp[start..=gt], &key)
    {
        return Some(xml_unescape(v).into_owned());
    }
    let mut at = 0;
    while let Some((start, gt, _)) = next_xml_tag(xmp, at) {
        if let Some((_, v)) = xml_attribute_raw(&xmp[start..=gt], &key) {
            return Some(xml_unescape(v).into_owned());
        }
        at = gt + 1;
    }
    match owned_element_body_span(xmp, &key) {
        Ok(Some((s, e))) => Some(xml_unescape(xmp[s..e].trim()).into_owned()),
        _ => None,
    }
}

/// The payload of `xmp`, if the document carries one: `None` when no element
/// binds [`PAYLOAD_URI`] or nothing under it names a recipe (every foreign
/// sidecar, and every sidecar this app wrote before the payload existed);
/// `Some(Err)` when there IS one and it cannot be trusted — a version this
/// build does not read, bytes that do not inflate, a CRC that does not match,
/// a recipe with a field this build does not know. The error is prose for the
/// disclosure line; the caller imports the `crs:` settings alone.
pub(crate) fn find(xmp: &str) -> Option<Result<Payload, String>> {
    let prefix = bound_prefix(xmp)?;
    let packed = simple_property(xmp, &prefix, "Recipe")?;
    Some(decode(xmp, &prefix, &packed))
}

fn decode(xmp: &str, prefix: &str, packed: &str) -> Result<Payload, String> {
    let format = simple_property(xmp, prefix, "Payload").unwrap_or_default();
    if format != PAYLOAD_FORMAT {
        return Err(format!(
            "payload format {format:?} is not the {PAYLOAD_FORMAT:?} this build reads"
        ));
    }
    let deflated = base64::engine::general_purpose::STANDARD
        .decode(packed.trim().as_bytes())
        .map_err(|e| format!("the recipe is not base64: {e}"))?;
    let mut json = Vec::new();
    flate2::read::ZlibDecoder::new(&deflated[..])
        .take(MAX_RECIPE_JSON)
        .read_to_end(&mut json)
        .map_err(|e| format!("the recipe does not inflate: {e}"))?;
    let want = simple_property(xmp, prefix, "RecipeCrc32")
        .and_then(|s| u32::from_str_radix(s.trim(), 16).ok());
    let got = crc32fast::hash(&json);
    if want != Some(got) {
        return Err(format!(
            "the recipe checksum does not match (stamped {}, computed {got:08x})",
            want.map_or_else(|| "nothing".to_string(), |w| format!("{w:08x}"))
        ));
    }
    let recipe: EditRecipe =
        serde_json::from_slice(&json).map_err(|e| format!("the recipe JSON: {e}"))?;
    let (rasters, notes) = read_rasters(xmp, prefix);
    Ok(Payload { recipe, rasters, notes })
}

/// Every `rdf:li` of `<prefix:Rasters>` as the `{prefix}:` fields it carries,
/// in either struct spelling — fields as attributes of the `<rdf:li>` (the
/// compact form Lightroom rewrites into, and this writer's) or as child
/// elements of it (`rdf:parseType="Resource"`, or a nested `rdf:Description`).
/// `Err` names an entry that never closes.
pub(super) fn raster_entries(
    xmp: &str,
    prefix: &str,
) -> Result<Vec<Vec<(String, String)>>, String> {
    let mut out = Vec::new();
    let Ok(Some((s, e))) = owned_element_body_span(xmp, &format!("{prefix}:Rasters")) else {
        return Ok(out);
    };
    let body = &xmp[s..e];
    let attr_prefix = format!("{prefix}:");
    let mut at = 0;
    while let Some((start, gt, self_closing)) = next_xml_tag(body, at) {
        let tag = &body[start..=gt];
        if tag.starts_with("</") || tag_name(tag) != "rdf:li" {
            at = gt + 1;
            continue;
        }
        let (inner, next) = if self_closing {
            ("", gt + 1)
        } else {
            match element_close_start(body, "rdf:li", gt) {
                Some(close) => (&body[gt + 1..close], close + "</rdf:li>".len()),
                None => return Err("an embedded raster entry never closes".to_string()),
            }
        };
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut cursor = 0;
        while let Some(a) = next_xml_attribute(tag, &mut cursor) {
            if let Some(local) = a.name.strip_prefix(&attr_prefix) {
                fields.push((local.to_string(), xml_unescape(a.value).into_owned()));
            }
        }
        // Element form: every `{prefix}:` child of the li is a field and its
        // text the value; a nested `rdf:Description` may carry the fields as
        // its attributes instead. Document order either way.
        let mut inner_at = 0;
        while let Some((s2, g2, sc2)) = next_xml_tag(inner, inner_at) {
            let t2 = &inner[s2..=g2];
            if t2.starts_with("</") {
                inner_at = g2 + 1;
                continue;
            }
            let n2 = tag_name(t2);
            if let Some(local) = n2.strip_prefix(&attr_prefix) {
                let (text, after) = if sc2 {
                    ("", g2 + 1)
                } else {
                    match element_close_start(inner, n2, g2) {
                        Some(c) => (&inner[g2 + 1..c], c + n2.len() + 3),
                        None => return Err(format!("embedded raster field {n2} never closes")),
                    }
                };
                fields.push((local.to_string(), xml_unescape(text.trim()).into_owned()));
                inner_at = after;
            } else {
                let mut c2 = 0;
                while let Some(a) = next_xml_attribute(t2, &mut c2) {
                    if let Some(local) = a.name.strip_prefix(&attr_prefix) {
                        fields.push((local.to_string(), xml_unescape(a.value).into_owned()));
                    }
                }
                inner_at = g2 + 1;
            }
        }
        out.push(fields);
        at = next;
    }
    Ok(out)
}

fn read_rasters(xmp: &str, prefix: &str) -> (Vec<Raster>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let entries = match raster_entries(xmp, prefix) {
        Ok(e) => e,
        Err(why) => {
            notes.push(why);
            return (out, notes);
        }
    };
    for fields in entries {
        let field = |local: &str| fields.iter().find(|(k, _)| k == local).map(|(_, v)| v.as_str());
        match (field("Name"), field("Crc32"), field("Data")) {
            (Some(name), Some(crc), Some(data)) => match decode_raster(name, crc, data) {
                Ok(r) => out.push(r),
                Err(why) => notes.push(why),
            },
            _ => notes.push("an embedded raster entry lacks its Name, Crc32 or Data".to_string()),
        }
    }
    (out, notes)
}

fn decode_raster(name: &str, crc: &str, data: &str) -> Result<Raster, String> {
    if !valid_name(name) {
        return Err(format!("embedded raster {name:?} is not a plain file name — refused"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim().as_bytes())
        .map_err(|e| format!("embedded raster {name:?} is not base64: {e}"))?;
    if bytes.len() > RASTER_BUDGET {
        return Err(format!("embedded raster {name:?} is larger than this build embeds — refused"));
    }
    let want = u32::from_str_radix(crc.trim(), 16)
        .map_err(|_| format!("embedded raster {name:?} carries an unreadable checksum"))?;
    let got = crc32fast::hash(&bytes);
    if want != got {
        return Err(format!(
            "embedded raster {name:?} does not match its checksum (stamped {want:08x}, computed {got:08x}) — not extracted"
        ));
    }
    Ok(Raster { name: name.to_string(), crc: got, bytes })
}

// ───────────────────────── restore ─────────────────────────

/// The develop a sidecar with a payload restores to.
///
/// Three recipes meet here. **P** is the payload — the develop exactly as the
/// app saved it. **C** is what the reader just decoded from the document's
/// `crs:` settings, which is where Lightroom's edits live. **W** is what the
/// same reader decodes from P projected through this writer with NO payload
/// (`bare_document`) and NO `ash:` intent ([`strip_mask_intent`]): what
/// Lightroom would have handed back had it rewritten the file without
/// touching anything — rounding included, and the intent gone, which is the
/// measured half of the rewrite (0 of 12 attributes survived). The
/// reconciliation walks the three as JSON trees, leaf by leaf:
///
/// * `W ≈ C` (within [`TOLERANCE`]) — the projection round-tripped
///   unchanged, so Lightroom did not edit this leaf: **P's exact value**. This
///   is also, automatically, every leaf the projection cannot carry at all —
///   a colour field, a bitmap mask's path, a zone alpha's name, an Intersect
///   spelled as Subtract-of-the-inverse — because both W and C are silent on it
///   in the same way.
/// * otherwise — the document says something different from what was written
///   into it, which is what an edit in Lightroom looks like: **C's value**.
///   A leaf Lightroom MATERIALISES rather than edits (Camera Raw's own default
///   for a key this writer omits at rest, `ColorNoiseReductionDetail="50"` and
///   its kin) is the one measured exception: absent-then-default is not an
///   edit, and P stands.
/// * a mask's `role` is intent with no `crs:` spelling — it is P's whether or
///   not a rename in Lightroom changed what the name-fallback would guess.
/// * a mask's INVERSION is one bit in two homes, and only their net reaches
///   the `crs:` spelling — the pair is reconciled as a unit by that net, see
///   [`reconcile_mask`].
///
/// Masks are matched, not zipped: the payload's masks that the projection
/// never wrote (a muted mask, a bitmap-based one) are restored as they were;
/// a written mask is paired with its read-back by order and name, then with
/// the document's by name; a pair Lightroom removed is dropped, a correction
/// Lightroom added is appended, and everything paired is reconciled leaf by
/// leaf like the globals.
///
/// Rasters: with a develop dir to hand, every verified raster is placed there
/// under its bare name — kept when the file is already there byte for byte,
/// placed under a fresh `-2`… name when a DIFFERENT file holds the name (the
/// recipe's references follow), written when absent — but only when
/// `materialize` says the caller is a restore surface: the silent probe
/// readers (`xmp_to_recipe_for_photo`) compare and never write. The bare
/// names are then anchored to the develop dir exactly as the store anchors a
/// loaded `recipe.json`.
pub(crate) fn restore(
    crs: EditRecipe,
    payload: Payload,
    frame: Option<FrameAspect>,
    photo: Option<&Path>,
    develop_dir: Option<&Path>,
    materialize: bool,
    diag: Option<&crate::diag::Diag<'_>>,
) -> EditRecipe {
    let warn = |text: String| {
        if let Some(d) = diag {
            d.warn(text);
        }
    };
    for note in &payload.notes {
        warn(note.clone());
    }
    let written = strip_mask_intent(&super::bare_document(&payload.recipe, frame));
    let written_then = super::xmp_to_recipe_clamped_impl(&written, photo, None).0;
    let mut r = match reconcile_recipes(&payload.recipe, &written_then, &crs) {
        Ok(r) => r,
        Err(why) => {
            warn(format!(
                "the sidecar's AutoShade payload could not be reconciled with its camera-raw \
                 settings ({why}) — the develop was imported from the camera-raw settings alone"
            ));
            return crs;
        }
    };
    if let Some(dir) = develop_dir {
        if materialize {
            place_rasters(&mut r, &payload.rasters, dir, &warn);
        }
        crate::store::resolve_mask_paths(&mut r, dir);
    }
    r
}

/// `doc` with every `ash:` intent attribute (and the prefix binding) removed
/// from every tag — what Lightroom's rewrite does to a document of ours, so
/// that the projection a payload is measured against is the one Lightroom
/// hands back and not the one this writer emitted. Measured against the two
/// real rewrites: the `crs:` settings came back, the intent did not.
fn strip_mask_intent(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut at = 0;
    while let Some((start, gt, _)) = next_xml_tag(doc, at) {
        out.push_str(&doc[at..start]);
        let mut tag = doc[start..=gt].to_string();
        loop {
            let mut cursor = 0;
            let mut span = None;
            while let Some(a) = next_xml_attribute(&tag, &mut cursor) {
                if a.name == "xmlns:ash" || a.name.starts_with("ash:") {
                    span = Some(a.span);
                    break;
                }
            }
            let Some(span) = span else { break };
            let mut left = span.start;
            while left > 0 && tag.as_bytes()[left - 1].is_ascii_whitespace() {
                left -= 1;
            }
            tag.replace_range(left..span.end, "");
        }
        out.push_str(&tag);
        at = gt + 1;
    }
    out.push_str(&doc[at..]);
    out
}

fn reconcile_recipes(
    p: &EditRecipe,
    w: &EditRecipe,
    c: &EditRecipe,
) -> Result<EditRecipe, String> {
    let to = |r: &EditRecipe| serde_json::to_value(r).map_err(|e| e.to_string());
    let (p, w, c) = (to(p)?, to(w)?, to(c)?);
    serde_json::from_value(reconcile(&p, &w, &c, None)).map_err(|e| e.to_string())
}

fn reconcile(
    p: &serde_json::Value,
    w: &serde_json::Value,
    c: &serde_json::Value,
    key: Option<&str>,
) -> serde_json::Value {
    use serde_json::Value;
    // Intent and provenance with no `crs:` spelling — see the `role` paragraph
    // of [`restore`]. The rationale rides in a comment the merge does not put
    // into a foreign base at all, so its absence from the document is not
    // Lightroom's doing either.
    if matches!(key, Some("role" | "rationale" | "confidence")) {
        return p.clone();
    }
    match (p, w, c) {
        (Value::Object(po), Value::Object(wo), Value::Object(co)) => {
            // An internally tagged enum whose tag moved is ONE value, not a
            // field set to merge: a Linear's fields have no meaning on a Radial.
            let tag = |o: &serde_json::Map<String, Value>| {
                o.get("kind").and_then(Value::as_str).map(str::to_string)
            };
            if tag(po) != tag(wo) || tag(wo) != tag(co) {
                return choose(p, w, c, key);
            }
            let keys: std::collections::BTreeSet<&str> =
                po.keys().chain(wo.keys()).chain(co.keys()).map(String::as_str).collect();
            let mut out = serde_json::Map::new();
            for k in keys {
                let get = |o: &'_ serde_json::Map<String, Value>| o.get(k).cloned().unwrap_or(Value::Null);
                let v = reconcile(&get(po), &get(wo), &get(co), Some(k));
                // `Null` is "absent" for a `skip_serializing_if` field, which
                // is how a payload written without it stays without it.
                if !v.is_null() {
                    out.insert(k.to_string(), v);
                }
            }
            Value::Object(out)
        }
        (Value::Array(pa), Value::Array(wa), Value::Array(ca)) => {
            if key == Some("masks") {
                return reconcile_masks(pa, wa, ca);
            }
            if pa.len() == wa.len() && wa.len() == ca.len() {
                Value::Array(
                    pa.iter()
                        .zip(wa)
                        .zip(ca)
                        .map(|((x, y), z)| reconcile(x, y, z, key))
                        .collect(),
                )
            } else {
                choose(p, w, c, key)
            }
        }
        _ => choose(p, w, c, key),
    }
}

/// The leaf rule of [`restore`].
fn choose(
    p: &serde_json::Value,
    w: &serde_json::Value,
    c: &serde_json::Value,
    key: Option<&str>,
) -> serde_json::Value {
    if approx_eq(w, c) {
        return p.clone();
    }
    if let Some(k) = key
        && let Some(default) = lightroom_materialised(k)
        && (w.is_null() || w.as_f64().is_some_and(|x| x.abs() <= TOLERANCE))
        && c.as_f64().is_some_and(|x| (x - default).abs() <= TOLERANCE)
    {
        return p.clone();
    }
    c.clone()
}

/// Camera Raw's own default for a key this writer OMITS at rest (see
/// `amount_carries` in the parent module): Lightroom writes the number back
/// into the file the first time it rewrites it, which is a materialisation,
/// not an edit. Keyed by the recipe's JSON field, valued by the number
/// Lightroom writes.
fn lightroom_materialised(key: &str) -> Option<f64> {
    match key {
        // Lightroom's RAW default (v1.6.0). Into a JPEG's it materialises 0,
        // which is the number a payload that stored nothing already holds.
        "sharpening" => Some(40.0),
        "sharpen_radius" => Some(1.0),
        "sharpen_detail" => Some(25.0),
        "nr_detail" | "color_nr_detail" | "color_nr_smooth" => Some(50.0),
        _ => None,
    }
}

fn approx_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= TOLERANCE + 1e-6 * x.abs().max(y.abs()),
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(u, v)| approx_eq(u, v))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, u)| y.get(k).is_some_and(|v| approx_eq(u, v)))
        }
        _ => a == b,
    }
}

/// The mask half of [`restore`]'s rule.
fn reconcile_masks(
    p: &[serde_json::Value],
    w: &[serde_json::Value],
    c: &[serde_json::Value],
) -> serde_json::Value {
    let name_of = |m: &serde_json::Value| -> String {
        m.get("name").and_then(serde_json::Value::as_str).unwrap_or("").to_string()
    };
    // Which payload masks the projection writes at all: the writer skips a
    // muted mask and a bitmap-based one (`masks_xml`), and the reader answers
    // "" for its own placeholder names, so the pairing compares what the reader
    // would have produced.
    let written: Vec<usize> = p
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.get("enabled").and_then(serde_json::Value::as_bool).unwrap_or(true)
                && m.get("mask").and_then(|g| g.get("kind")).and_then(serde_json::Value::as_str)
                    != Some("bitmap")
        })
        .map(|(i, _)| i)
        .collect();
    let mut w_used = vec![false; w.len()];
    let mut c_used = vec![false; c.len()];
    let mut p_to_w: Vec<Option<usize>> = vec![None; p.len()];
    for (j, &i) in written.iter().enumerate() {
        let name = read_back_name(&name_of(&p[i]), &p[i]);
        let cand = if j < w.len() && !w_used[j] && name_of(&w[j]) == name {
            Some(j)
        } else {
            (0..w.len()).find(|&k| !w_used[k] && name_of(&w[k]) == name)
        };
        if let Some(k) = cand {
            w_used[k] = true;
            p_to_w[i] = Some(k);
        }
    }
    let mut w_to_c: Vec<Option<usize>> = vec![None; w.len()];
    for (j, wm) in w.iter().enumerate() {
        let name = name_of(wm);
        let cand = if j < c.len() && !c_used[j] && name_of(&c[j]) == name {
            Some(j)
        } else {
            (0..c.len()).find(|&k| !c_used[k] && name_of(&c[k]) == name)
        };
        if let Some(k) = cand {
            c_used[k] = true;
            w_to_c[j] = Some(k);
        }
    }
    let mut out = Vec::new();
    for (i, pm) in p.iter().enumerate() {
        match p_to_w[i] {
            // Never written (muted, bitmap-based, or beyond a cap the
            // projection applied): the payload is the only record.
            None => out.push(pm.clone()),
            Some(j) => match w_to_c[j] {
                // Written, then removed in Lightroom.
                None => {}
                Some(k) => out.push(reconcile_mask(pm, &w[j], &c[k])),
            },
        }
    }
    for (k, cm) in c.iter().enumerate() {
        if !c_used[k] {
            out.push(cm.clone()); // added in Lightroom
        }
    }
    serde_json::Value::Array(out)
}

/// One paired mask: the leaf rule, except for the inversion.
///
/// The inversion is ONE bit spelled in two places — the correction's
/// `inverted` and the geometry's own (`flipped` on a radial, `inverted` on a
/// brush or an AI mask) — whose XOR is the net Lightroom renders
/// (`LocalAdjustment::net_inverted`) and the only thing the `crs:` spelling
/// carries; WHICH of the two holds it is the authored home, intent with no
/// `crs:` spelling of its own (`ash:Inverted`, gone after a rewrite). So the
/// pair is reconciled as a unit by its net: the net Lightroom handed back is
/// the net that was written → the payload's pair, home and all; a different
/// net is an edit → the document's pair, in Lightroom's home. Leaf by leaf
/// the two halves would each lose: a flipped net kept the payload's
/// correction bit beside the document's own bit and undid the edit, and an
/// untouched rewrite moved the home for nothing.
fn reconcile_mask(p: &serde_json::Value, w: &serde_json::Value, c: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    let mut out = reconcile(p, w, c, None);
    fn kind(m: &Value) -> Option<&str> {
        m.get("mask").and_then(|g| g.get("kind")).and_then(Value::as_str)
    }
    let own_key = match kind(p) {
        k if k != kind(w) || k != kind(c) => return out,
        Some("radial") => "flipped",
        Some("brush" | "ai_mask") => "inverted",
        _ => return out,
    };
    let bit = |m: &Value, key: &str| m.get(key).and_then(Value::as_bool).unwrap_or(false);
    let pair = |m: &Value| (bit(m, "inverted"), m.get("mask").is_some_and(|g| bit(g, own_key)));
    let net = |(whole, own): (bool, bool)| whole ^ own;
    let chosen = if net(pair(w)) == net(pair(c)) { pair(p) } else { pair(c) };
    if let Value::Object(o) = &mut out {
        o.insert("inverted".to_string(), Value::Bool(chosen.0));
        if let Some(Value::Object(g)) = o.get_mut("mask") {
            g.insert(own_key.to_string(), Value::Bool(chosen.1));
        }
    }
    out
}

/// What this reader answers as the NAME of a written payload mask: the
/// placeholder rules of `parse_one_correction_with_reader` and
/// [`zone_name_role`], applied to what the writer would have emitted.
fn read_back_name(name: &str, mask: &serde_json::Value) -> String {
    if name.strip_prefix("AutoShade ").is_some_and(|rest| rest.parse::<u32>().is_ok()) {
        return String::new();
    }
    let select_sky = mask.get("mask").and_then(|g| g.get("kind")).and_then(serde_json::Value::as_str)
        == Some("ai_mask")
        && mask.get("mask").and_then(|g| g.get("subtype")).and_then(serde_json::Value::as_u64)
            == Some(2);
    if select_sky && matches!(name, "sky" | "land") {
        return String::new();
    }
    name.to_string()
}

/// The role a correction's NAME says it has, when the sidecar carries no
/// intent to say so — the last-resort recovery for a sidecar Lightroom
/// rewrote before the payload existed (v1.3.0 and earlier).
///
/// Only a Select Sky base (`Mask/Image`, subtype 2) can be a zone, and only the
/// fit's own spellings count: the bare tag this writer emits for an unnamed
/// zone (`sky` / `land`, which is then the placeholder and NOT the mask's name,
/// exactly as `AutoShade <n>` is not), and the band label the sub-zone fit
/// gives its members (`sky · band 2/3`, a real name that stays). Anything
/// else — Lightroom's own `Sky 1`, a user's label — is Custom, as before.
pub(super) fn zone_name_role(name: String, base: &MaskGeometry) -> (String, Option<MaskRole>) {
    if !matches!(base, MaskGeometry::AiMask { subtype: 2, .. }) {
        return (name, None);
    }
    let role_of = |tag: &str| match tag {
        "sky" => Some(MaskRole::ZoneSky),
        "land" => Some(MaskRole::ZoneLand),
        _ => None,
    };
    if let Some(role) = role_of(&name) {
        return (String::new(), Some(role));
    }
    if let Some((tag, rest)) = name.split_once(" · band ")
        && let Some((i, n)) = rest.split_once('/')
        && i.parse::<u32>().is_ok()
        && n.parse::<u32>().is_ok()
        && let Some(role) = role_of(tag)
    {
        return (name, Some(role));
    }
    (name, None)
}

/// Put the payload's rasters beside the develop — see [`restore`].
fn place_rasters(r: &mut EditRecipe, rasters: &[Raster], dir: &Path, warn: &dyn Fn(String)) {
    for raster in rasters {
        let target = dir.join(&raster.name);
        let placed_as = match std::fs::read(&target) {
            Ok(existing) if crc32fast::hash(&existing) == raster.crc => raster.name.clone(),
            Ok(_) => match claim_beside(dir, &raster.name, &raster.bytes) {
                Ok(fresh) => {
                    warn(format!(
                        "{} already exists beside this develop with different content — the \
                         sidecar's copy was placed as {fresh} and the restored masks reference that",
                        raster.name
                    ));
                    fresh
                }
                Err(e) => {
                    warn(format!(
                        "the sidecar's copy of {} could not be placed beside the develop ({e}) — \
                         the masks that reference it render from the file already there",
                        raster.name
                    ));
                    continue;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Err(e) = std::fs::create_dir_all(dir)
                    .and_then(|()| crate::store::durable_write(&target, &raster.bytes))
                {
                    warn(format!(
                        "the sidecar's copy of {} could not be written beside the develop ({e}) — \
                         the masks that reference it will render inert",
                        raster.name
                    ));
                    continue;
                }
                raster.name.clone()
            }
            Err(e) => {
                warn(format!(
                    "{} beside this develop could not be read ({e}) — the sidecar's copy was not \
                     placed",
                    raster.name
                ));
                continue;
            }
        };
        if placed_as != raster.name {
            for m in &mut r.masks {
                for path in m.bitmap_paths_mut() {
                    if Path::new(path.as_str()).is_relative() && bare_name(path) == raster.name {
                        *path = placed_as.clone();
                    }
                }
            }
        }
    }
}

/// `<stem>-2.<ext>` … `-999`, `create_new`-claimed so two surfaces can never
/// hand out one name — the scheme `store::claim_raster` uses.
fn claim_beside(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<String> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    for n in 2..=999u32 {
        let fresh = format!("{stem}-{n}{ext}");
        match std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(&fresh)) {
            Ok(mut f) => {
                f.write_all(bytes)?;
                f.sync_all()?;
                return Ok(fresh);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(format!("over 999 '{stem}' rasters beside this develop")))
}
