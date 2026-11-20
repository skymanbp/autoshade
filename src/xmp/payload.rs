//! The AutoShade PAYLOAD: the whole develop, carried inside the sidecar under
//! this app's own XMP namespace so that a Lightroom rewrite cannot lose it.
//!
//! **What the `crs:` projection alone could never carry back.** A sidecar is
//! Lightroom's document, and the settings this writer owns are re-serialised
//! from Lightroom's own model every time Lightroom touches the file: the
//! `ash:` intent attributes inside `crs:MaskGroupBasedCorrections` are dropped
//! (so a zone's role came back as `Custom`), unknown `crs:` attributes and
//! elements are dropped, and everything classic XMP has no encoding for —
//! bitmap tiles, the 12×32×8 colour field, a muted mask, the alpha the zoned fit
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
//! over the JSON bytes), on a merge the document as written without the
//! payload (`asr:Written` + `asr:WrittenCrc32`, [`written_attrs`]), and the
//! rasters nothing can re-derive (bitmap tiles,
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
pub(super) const MAX_RECIPE_JSON: u64 = super::MAX_XMP_BYTES as u64;
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
    for path in p.raster_paths_mut() {
        *path = bare_name(path);
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
/// the CRC and the recipe itself — and `NotCarried`, the sentence naming the
/// global controls the `crs:` settings cannot carry
/// ([`super::describe_global_losses`]: a colour field, a lens blur …), so the
/// document itself says what Lightroom will not render, beside the develop
/// that does. Absent when there are none. Under the payload's prefix, so a
/// merge strips the old one with the rest ([`strip_root_attrs`]) and no
/// reader takes it for a setting.
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
    let not_carried = super::describe_global_losses(&super::global_export_losses(r))
        .map(|s| format!("\n    {prefix}:NotCarried=\"{}\"", xml_attr_escape(&s)))
        .unwrap_or_default();
    format!(
        "\n    xmlns:{prefix}=\"{PAYLOAD_URI}\"\
         \n    {prefix}:Payload=\"{PAYLOAD_FORMAT}\"\
         \n    {prefix}:Writer=\"AutoShade {}\"\
         \n    {prefix}:RecipeCrc32=\"{crc:08x}\"\
         \n    {prefix}:Recipe=\"{b64}\"{not_carried}",
        env!("CARGO_PKG_VERSION"),
    )
}

/// The record of the document AS WRITTEN — `written`, the merged document
/// without this payload — under the payload's prefix: `Written` (zlib +
/// base64) and `WrittenCrc32` over its bytes. It is what [`restore`] measures
/// Lightroom's edits against. A merge keeps every key of its base this writer
/// does not own (the camera profile and Look a Lightroom sidecar carries, its
/// lens-profile switch), so the bare projection of the recipe is not the
/// document that was written: read against it, those keys were Lightroom
/// edits and replaced the payload's values on our own unedited sidecar
/// (v1.17.1, measured on the reference pairs' own Lightroom sidecars).
/// Empty when the record would push the document past
/// [`super::MAX_XMP_BYTES`]; the reader then measures against the bare
/// projection, as it did before the record existed.
pub(super) fn written_attrs(written: &str, prefix: &str) -> String {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let packed = z.write_all(written.as_bytes()).and_then(|()| z.finish()).unwrap_or_default();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&packed);
    if packed.is_empty() || 2 * written.len() + b64.len() > super::MAX_XMP_BYTES {
        return String::new();
    }
    format!(
        "\n    {prefix}:WrittenCrc32=\"{:08x}\"\n    {prefix}:Written=\"{b64}\"",
        crc32fast::hash(written.as_bytes()),
    )
}

/// The lens profile a sidecar of OURS carries, when Lightroom has not touched
/// the lens keys since it was written: the payload's own, exactly as the
/// develop rendered it. `None` for a document with no payload this build
/// reads, and for one whose `LensProfileEnable` / `LensProfileSetup` /
/// `LensProfileName` / `LensProfileFilename` differ from the document as
/// written ([`written_document`]) — an edit, which
/// `pipeline::fresh_lens_profile_for_sidecar` then obeys.
///
/// Why (v1.17.1): those keys are not this writer's. A merge carries them from
/// the base — the Lightroom sidecar beside the RAW — so our own unedited
/// sidecar said `LensProfileEnable="0"` beside a develop that rendered the
/// camera's distortion and vignetting correction, and the import stamp
/// switched them off: with the profile and Look already restored, that alone
/// left one reference pair's re-import 3.24 dE from its export outside the sky.
pub fn written_lens_profile(xmp: &str) -> Option<crate::recipe::LensProfile> {
    let payload = find(xmp)?.ok()?;
    let written = written_document(&payload, None);
    let keys = |doc: &str| {
        (
            super::lens_profile_enabled(doc),
            super::lens_profile_setup(doc),
            super::lens_profile_name(doc),
            super::lens_profile_filename(doc),
        )
    };
    (keys(xmp) == keys(&written)).then_some(payload.recipe.lens_profile)
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
    // Each raster ONCE, by bare name — and its verdict with it: a raster two
    // masks share that cannot be embedded is a loss to BOTH of them, and the
    // second mask used to step over the name in silence.
    let mut seen: Vec<(String, bool)> = Vec::new();
    let mut items = String::new();
    let mut used = 0usize;
    let develop = photo.map(crate::store::develop_dir);
    for (i, m) in r.masks.iter().enumerate() {
        let mask_name = super::written_name(i, m);
        let mut walk = m.clone();
        for path in walk.turnable_raster_paths_mut() {
            let name = bare_name(path);
            let embedded = if let Some(&(_, embedded)) = seen.iter().find(|(n, _)| *n == name) {
                embedded
            } else {
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
                seen.push((name, embedded));
                embedded
            };
            // ONE verdict per mask: a mask naming the same missing raster
            // through two geometries (an alpha and a component) recorded the
            // same loss twice until 2026-09-24.
            let said = losses
                .iter()
                .any(|l| l.name == mask_name && l.reason == MaskLossReason::RasterNotEmbedded);
            if !embedded && !said {
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
    /// The version of the build that wrote the payload, as its root states it
    /// (`Writer="AutoShade X.Y.Z"`, stamped by every build since the first
    /// payload, v1.3.1): `None` when the attribute is absent or is not that
    /// shape. [`restore`] reads it to know what THAT build wrote into the
    /// `crs:` settings for a mask's colour gains ([`GAINS_PROJECTED_SINCE`]).
    pub(crate) writer: Option<[u32; 3]>,
    /// The document as written, without the payload ([`written_attrs`]):
    /// `None` for a payload written before the record existed, a fresh
    /// document (whose written form IS the bare projection), or a record that
    /// did not verify.
    pub(crate) written: Option<String>,
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
    // One byte past the ceiling, so an overrun is SEEN: `take` AT the ceiling
    // cut the JSON in silence, the cut bytes then failed the CRC, and the line
    // blamed a checksum that was never wrong.
    flate2::read::ZlibDecoder::new(&deflated[..])
        .take(MAX_RECIPE_JSON + 1)
        .read_to_end(&mut json)
        .map_err(|e| format!("the recipe does not inflate: {e}"))?;
    if json.len() as u64 > MAX_RECIPE_JSON {
        return Err(format!(
            "the recipe is too large: it inflates past the {MAX_RECIPE_JSON}-byte ceiling this build reads"
        ));
    }
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
    let (rasters, mut notes) = read_rasters(xmp, prefix);
    let written = match read_written(xmp, prefix) {
        Ok(written) => written,
        Err(why) => {
            notes.push(format!(
                "the sidecar's record of the document AutoShade wrote could not be read ({why}) — Lightroom's edits were told from AutoShade's own values by re-projecting the develop instead"
            ));
            None
        }
    };
    Ok(Payload { recipe, writer: writer_version(xmp, prefix), written, rasters, notes })
}

/// The first build whose writer spells a mask's colour gains as the mask's
/// Temp, Tint and Exposure (`render::gains_as_white`, v1.17.0). Every writer
/// from the first payload (v1.3.1) up to v1.16.x wrote the mask's OWN three
/// there and kept the gains in the payload alone, naming the recolour as a
/// loss Lightroom does not render.
const GAINS_PROJECTED_SINCE: [u32; 3] = [1, 17, 0];

/// [`Payload::writer`]: `X.Y.Z` of `Writer="AutoShade X.Y.Z"`, three plain
/// numbers and nothing after them — the shape `root_attrs` has always
/// stamped. Anything else is `None`: a guess at a version would decide what
/// a build is assumed to have written.
fn writer_version(xmp: &str, prefix: &str) -> Option<[u32; 3]> {
    let stated = simple_property(xmp, prefix, "Writer")?;
    let mut parts = stated.trim().strip_prefix("AutoShade ")?.split('.');
    let mut version = [0u32; 3];
    for part in &mut version {
        *part = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(version)
}

/// The [`written_attrs`] record, verified: `Ok(None)` when the document
/// carries none.
fn read_written(xmp: &str, prefix: &str) -> Result<Option<String>, String> {
    let Some(packed) = simple_property(xmp, prefix, "Written") else { return Ok(None) };
    let deflated = base64::engine::general_purpose::STANDARD
        .decode(packed.trim().as_bytes())
        .map_err(|e| format!("it is not base64: {e}"))?;
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(&deflated[..])
        .take(MAX_RECIPE_JSON + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("it does not inflate: {e}"))?;
    if bytes.len() as u64 > MAX_RECIPE_JSON {
        return Err(format!("it inflates past the {MAX_RECIPE_JSON}-byte ceiling this build reads"));
    }
    let want = simple_property(xmp, prefix, "WrittenCrc32").and_then(|s| u32::from_str_radix(s.trim(), 16).ok());
    if want != Some(crc32fast::hash(&bytes)) {
        return Err("its checksum does not match".to_string());
    }
    String::from_utf8(bytes).map(Some).map_err(|e| format!("it is not UTF-8: {e}"))
}

/// The document a payload's values were written into, as [`restore`] measures
/// edits against it: the recorded one ([`written_attrs`]) when the payload
/// carries it, else the bare projection of the recipe — which IS the written
/// document of a fresh sidecar, and the best a payload from before the record
/// can offer.
pub(super) fn written_document(payload: &Payload, frame: Option<FrameAspect>) -> String {
    payload.written.clone().unwrap_or_else(|| super::bare_document(&payload.recipe, frame))
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
/// same reader decodes from the document P was written into
/// ([`written_document`]: the recorded one, [`written_attrs`], when the
/// payload carries it, else P projected through this writer with NO payload,
/// `bare_document`) with NO `ash:` intent ([`strip_mask_intent`]): what
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
/// * a mask's TEMP and TINT in a payload that names no `mask_wb_era` — one
///   v1.18.3 or an earlier build wrote — are that build's numbers, on its
///   weaker scale, and so are the `crs:LocalTemperature` / `crs:LocalTint`
///   it wrote beside them (the slider's own for a plain mask, its projection
///   of the gains for a recolour carrier). P, W and C are compared as they
///   stand, all three on that scale; then each Temp and Tint is restated
///   leaf by leaf: one the document handed back as written is still that
///   build's number and is carried onto this build's scale, and one that
///   came back different — a number set in Lightroom — stands as it is
///   ([`mask_wb_from_payload`], [`carry_over_mask_wb`]). That holds for a
///   recolour carrier whose three were moved as well: its gains go, the
///   number Lightroom moved stands, and the Temp or Tint beside it that was
///   not moved is carried over. The sidecar imports to the picture it was
///   exported with.
/// * a mask's COLOUR GAINS in a payload whose writer is older than v1.17.0
///   were never spelled into the `crs:` settings: that writer wrote the
///   mask's own Temp, Tint and Exposure. W is today's projection, which
///   spells them, so for such a writer the mask's three in W are restated
///   as the payload's own before the rule runs
///   ([`written_before_the_projection`]): untouched, the gains stay; a leaf
///   moved in Lightroom, the three read as moved like any carrier's.
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
/// loaded `recipe.json`. With no develop dir the names stay as written here;
/// the photo-less READER disables them once the whole reading is done
/// (`read::xmp_to_recipe_clamped_impl`).
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
    // A payload that names no `mask_wb_era` is an older build's (v1.18.3 or
    // earlier; an explicit 0 is the same statement, though no build writes
    // one): its masks' Temp and Tint, and the `crs:` ones that build wrote
    // beside them, are on that build's scale. Read here, off the recipe as
    // the payload spelled it, before anything stamps it. The written
    // document below is then that build's as well — the recorded one is, and
    // the bare projection of a mask's gains (`render::gains_as_white`) is
    // searched with a develop that reads the recipe on the scale it states
    // (`render::mask_wb_gains`), so it ends on the three numbers that build
    // wrote and the gains are recognised as untouched.
    let older_mask_wb = payload.recipe.mask_wb_era < crate::recipe::MASK_WB_ERA;
    // What the writing build put into the `crs:` settings for a mask's colour
    // gains (2026-11-20). A writer older than v1.17.0 did not project them:
    // it wrote the mask's OWN Temp, Tint and Exposure and kept the gains in
    // the payload alone. The bare projection below is today's writer's, which
    // does project them, so measured against it every such mask read as moved
    // in Lightroom and lost its gains — on a sidecar nobody had touched, from
    // v1.17.0 on. Like the scale, the written document is restated as the
    // writing build wrote it (`reconcile_masks`). A RECORDED document needs
    // no restating — it is what was written — and no build that old left one.
    // A payload with no writer this reader can parse was not left that way by
    // any build of this app, so nothing says what its three were written as:
    // it is read by the rule for today's writer, the one that assumes nothing
    // about the past. A wrong assumption either way costs the same — the mask
    // reads as moved, its gains go and the document's numbers stand.
    let own_three = payload.written.is_none() && payload.writer.is_some_and(|v| v < GAINS_PROJECTED_SINCE);
    let wrote = Wrote { older_mask_wb, own_three };
    let written = strip_mask_intent(&written_document(&payload, frame));
    // The raw reading: a photo-less outer read disables raster names only
    // once it is finished, so W keeps the bare names P and C are compared in.
    let written_then = super::read_document(&written, photo, None).0;
    let mut r = match reconcile_recipes(&payload.recipe, &written_then, &crs, wrote) {
        Ok(r) => r,
        Err(why) => {
            warn(format!(
                "the sidecar's AutoShade payload could not be reconciled with its camera-raw \
                 settings ({why}) — the develop was imported from the camera-raw settings alone"
            ));
            return crs;
        }
    };
    // The payload is the authority for every field its recipe CARRIES. A
    // recipe older than a control carries none for it — serde filled the
    // default, and the writer neither strips nor writes that key
    // (`merge::unspoken_attr_keys`) — so the document's value stands there,
    // whatever the payload's JSON spells. Until v1.17.1 the bare projection
    // the payload was measured against lacked those keys, which gave the same
    // answer by accident; measured against the document as written it would
    // not (`written_attrs`).
    let unspoken = super::merge::unspoken_era_controls(&payload.recipe);
    if !unspoken.is_empty()
        && let (Ok(mut restored), Ok(document)) = (serde_json::to_value(&r), serde_json::to_value(&crs))
        && let (Some(out), Some(doc)) = (restored.as_object_mut(), document.as_object())
    {
        for (field, _) in unspoken {
            match doc.get(field) {
                Some(value) => out.insert(field.to_string(), value.clone()),
                None => out.remove(field),
            };
        }
        if let Ok(back) = serde_json::from_value(restored) {
            r = back;
        }
    }
    // Every mask Temp and Tint of `r` is on this build's scale by now: a
    // current payload's and the document's own as they stood, an older
    // payload's carried over leaf by leaf where the rule took them
    // (`carry_over_mask_wb`, 2026-11-20). The stamp says so, once, for the
    // whole recipe. The leaf rule alone would hand an older payload's silence
    // on, and the recipe would be carried over a second time as an older
    // build's save the next time its file was read
    // (`EditRecipe::convert_legacy_mask_wb`) — one import, two pictures.
    r.mask_wb_era = crate::recipe::MASK_WB_ERA;
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

/// What [`restore`] knows about the build that wrote the payload and that the
/// mask half of the rule needs ([`reconcile_masks`]). `NOW` is this build:
/// nothing to restate.
#[derive(Clone, Copy)]
struct Wrote {
    /// The payload names no `mask_wb_era`: its masks' Temp and Tint, and the
    /// `crs:` ones written beside them, are on the scale of v1.18.3.
    older_mask_wb: bool,
    /// The writer is older than v1.17.0 and left no record of the document:
    /// a mask's colour gains were not projected, and the three numbers it
    /// wrote for such a mask are the mask's own ([`GAINS_PROJECTED_SINCE`]).
    own_three: bool,
}

impl Wrote {
    const NOW: Self = Self { older_mask_wb: false, own_three: false };
}

fn reconcile_recipes(
    p: &EditRecipe,
    w: &EditRecipe,
    c: &EditRecipe,
    wrote: Wrote,
) -> Result<EditRecipe, String> {
    let to = |r: &EditRecipe| serde_json::to_value(r).map_err(|e| e.to_string());
    let (p, w, c) = (to(p)?, to(w)?, to(c)?);
    serde_json::from_value(reconcile(&p, &w, &c, None, wrote)).map_err(|e| e.to_string())
}

/// `wrote`: what the writing build did differently from this one — it
/// travels down to [`reconcile_masks`], the one place that knows which recipe
/// each mask leaf came from.
fn reconcile(
    p: &serde_json::Value,
    w: &serde_json::Value,
    c: &serde_json::Value,
    key: Option<&str>,
    wrote: Wrote,
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
                let v = reconcile(&get(po), &get(wo), &get(co), Some(k), wrote);
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
                return reconcile_masks(pa, wa, ca, wrote);
            }
            if pa.len() == wa.len() && wa.len() == ca.len() {
                Value::Array(
                    pa.iter()
                        .zip(wa)
                        .zip(ca)
                        .map(|((x, y), z)| reconcile(x, y, z, key, wrote))
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
///
/// With an older build's payload (`older_mask_wb`) this is also where each
/// mask's Temp and Tint are brought onto this build's scale, by where the
/// rule took them from ([`carry_over_mask_wb`]): a mask the projection never
/// wrote is the payload's alone, both leaves; a paired mask's leaf is that
/// build's number where the document handed it back as written, whether or
/// not the mask carries colour gains ([`mask_wb_from_payload`]); a
/// correction added in Lightroom is the document's and stands.
///
/// And where the writer was older than v1.17.0 (`wrote.own_three`), a paired
/// mask that carries colour gains is measured against what THAT writer wrote
/// for it — the mask's own three, not today's projection of the gains
/// ([`written_before_the_projection`]) — before any rule runs, so the rules
/// themselves are the same for every writer.
fn reconcile_masks(
    p: &[serde_json::Value],
    w: &[serde_json::Value],
    c: &[serde_json::Value],
    wrote: Wrote,
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
        let (mut mask, from_payload) = match p_to_w[i] {
            // Never written (muted, bitmap-based, or beyond a cap the
            // projection applied): the payload is the only record.
            None => (pm.clone(), [true; 2]),
            Some(j) => match w_to_c[j] {
                // Written, then removed in Lightroom.
                None => continue,
                Some(k) => {
                    let written = if wrote.own_three {
                        written_before_the_projection(pm, &w[j])
                    } else {
                        std::borrow::Cow::Borrowed(&w[j])
                    };
                    (reconcile_mask(pm, &written, &c[k]), mask_wb_from_payload(&written, &c[k]))
                }
            },
        };
        if wrote.older_mask_wb {
            carry_over_mask_wb(&mut mask, from_payload);
        }
        out.push(mask);
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
    // No mask holds masks: what an older writer did differently is
    // `reconcile_masks`'s to restate, so the leaves below are read as today's.
    let mut out = recolour_carrier(reconcile(p, w, c, None, Wrote::NOW), p, w, c);
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

/// The other unit of [`reconcile_mask`]: a mask's colour gains, which the
/// writer spells as its Temp, Tint and Exposure (`render::gains_as_white`).
/// Those three leaves of the document are ONE written value derived from the
/// payload's gains, so they are reconciled together: handed back as written,
/// the payload's own Temp, Tint, Exposure and gains stand (the leaf rule has
/// already chosen them); moved — an edit in Lightroom, or a build whose
/// develop now projects the gains differently — the document's three are
/// taken and the gains go, since the document's colour already carries them.
/// Leaf by leaf, a moved Temp would have joined the payload's gains and put
/// the recolour on twice. What is taken here is the document's numbers; on
/// which scale each Temp and Tint among them is read, for an older build's
/// payload, is settled afterwards and leaf by leaf ([`mask_wb_from_payload`]).
fn recolour_carrier(
    mut out: serde_json::Value,
    p: &serde_json::Value,
    w: &serde_json::Value,
    c: &serde_json::Value,
) -> serde_json::Value {
    use serde_json::Value;
    if !carrier_moved(p, w, c) {
        return out;
    }
    if let Value::Object(o) = &mut out {
        for k in CARRIER {
            o.insert(k.to_string(), mask_leaf(c, k));
        }
        o.remove("color_gains");
    }
    out
}

/// The three leaves a mask's colour gains are written as ([`recolour_carrier`]).
const CARRIER: [&str; 3] = ["temperature", "tint", "exposure_ev"];

fn mask_leaf(m: &serde_json::Value, k: &str) -> serde_json::Value {
    m.get(k).cloned().unwrap_or(serde_json::Value::Null)
}

/// Did the document's Temp, Tint and Exposure of a mask whose payload carries
/// colour gains come back different from what was written — the question
/// [`recolour_carrier`] turns on.
fn carrier_moved(p: &serde_json::Value, w: &serde_json::Value, c: &serde_json::Value) -> bool {
    carries_gains(p) && !CARRIER.iter().all(|k| approx_eq(&mask_leaf(w, k), &mask_leaf(c, k)))
}

/// Does the payload's mask carry colour gains that are not neutral?
fn carries_gains(p: &serde_json::Value) -> bool {
    use serde_json::Value;
    let gains = p.get("color_gains").and_then(Value::as_array);
    !gains.is_none_or(|g| g.iter().all(|v| v.as_f64() == Some(1.0)))
}

/// The written mask `w` as a writer older than v1.17.0 wrote it for the
/// payload's mask `p` (2026-11-20).
///
/// `w` is read back from today's bare projection, where a mask's colour gains
/// leave as its Temp, Tint and Exposure (`render::gains_as_white`). Up to
/// v1.16.x the writer did no such thing: `masks_xml` wrote
/// `m.exposure_ev`, `m.temperature` and `m.tint` as they stood and named the
/// recolour as a loss, so what the document held for the three is the
/// payload mask's OWN numbers. They are put back into `w` here, and the rules
/// that follow — the leaf rule, [`recolour_carrier`],
/// [`mask_wb_from_payload`] — then see the document that build wrote:
/// handed back as written, the gains stay and the mask's own Temp and Tint
/// are carried over; a leaf moved in Lightroom makes the three read as moved.
/// A mask without gains was written the same way by every build and is left
/// alone.
fn written_before_the_projection<'a>(
    p: &serde_json::Value,
    w: &'a serde_json::Value,
) -> std::borrow::Cow<'a, serde_json::Value> {
    if !carries_gains(p) {
        return std::borrow::Cow::Borrowed(w);
    }
    let mut own = w.clone();
    if let Some(o) = own.as_object_mut() {
        for k in CARRIER {
            match p.get(k) {
                Some(v) => o.insert(k.to_string(), v.clone()),
                None => o.remove(k),
            };
        }
    }
    std::borrow::Cow::Owned(own)
}

/// Which of a paired mask's Temp and Tint are still the numbers the build
/// that wrote the payload stated (`[temperature, tint]`): each one the
/// document handed back as written, leaf by leaf, whether or not the mask
/// carries colour gains (2026-11-20).
///
/// One rule for the whole reader: a number AutoShade wrote keeps AutoShade's
/// picture, a number set in Lightroom stands as stated. For a plain mask, and
/// for a recolour carrier whose three came back as written, such a leaf is
/// the payload's own ([`choose`]). For a carrier one of whose three was moved
/// in Lightroom the gains go and the document's three are taken
/// ([`recolour_carrier`]) — and a Temp or Tint among them that came back as
/// written is still what that build wrote, so it is carried over like a
/// plain mask's, while the one Lightroom moved stands at the number it
/// states. The Exposure has one scale and is the document's either way.
fn mask_wb_from_payload(w: &serde_json::Value, c: &serde_json::Value) -> [bool; 2] {
    ["temperature", "tint"].map(|k| approx_eq(&mask_leaf(w, k), &mask_leaf(c, k)))
}

/// Carry the leaves of `mask` that are still an older build's numbers
/// (`from_payload`, as [`mask_wb_from_payload`] or the unwritten-mask arm of
/// [`reconcile_masks`] names them) onto this build's scale (2026-11-20).
///
/// A sidecar AutoShade wrote must import to the picture it was exported
/// with, whichever build wrote it: the sidecar is the copy of a develop that
/// travels with the photo. Up to v1.18.3 a mask's Temp and Tint rendered on a
/// weaker scale, and that build states them on it in the payload and in the
/// `crs:` settings alike; carried over by the same exact rule as a saved
/// recipe's (`render::local_wb_from_legacy`), the mask renders what that
/// build rendered. A leaf Lightroom MOVED is not touched: a number set in
/// Lightroom is read as it stands, the rule for every sidecar Lightroom
/// wrote. The two scalings are independent and linear, so the leaves are
/// carried one by one.
fn carry_over_mask_wb(mask: &mut serde_json::Value, from_payload: [bool; 2]) {
    let Some(o) = mask.as_object_mut() else { return };
    let stated = |o: &serde_json::Map<String, serde_json::Value>, k: &str| {
        o.get(k).and_then(serde_json::Value::as_f64).map(|v| v as f32)
    };
    let (temperature, tint) = (stated(o, "temperature"), stated(o, "tint"));
    let carried = crate::render::local_wb_from_legacy(temperature.unwrap_or(0.0), tint.unwrap_or(0.0));
    for (k, had, now, take) in
        [("temperature", temperature, carried.0, from_payload[0]), ("tint", tint, carried.1, from_payload[1])]
    {
        if take && had.is_some() {
            o.insert(k.to_string(), serde_json::Value::from(now));
        }
    }
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
            for path in r.raster_paths_mut() {
                if Path::new(path.as_str()).is_relative() && bare_name(path) == raster.name {
                    *path = placed_as.clone();
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
        let at = dir.join(&fresh);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&at) {
            Ok(f) => {
                fill_claimed(&at, f, bytes)?;
                return Ok(fresh);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(format!("over 999 '{stem}' rasters beside this develop")))
}

/// Fill a name [`claim_beside`] has just created, and RELEASE it when the
/// write or the sync fails: the error used to return with the half-written
/// file still under the claimed name, where the next restore's CRC check
/// found a DIFFERENT file, stepped on to `-3`, and left a truncated raster
/// nothing references beside the develop. A claim that could not be filled
/// is unlinked along with the error.
pub(super) fn fill_claimed(at: &Path, mut f: std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
    if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
        // Closed before it is unlinked: Windows refuses to remove an open file.
        drop(f);
        let _ = std::fs::remove_file(at);
        return Err(e);
    }
    Ok(())
}
