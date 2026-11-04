//! Merging a recipe into an existing sidecar: the outcome, the key sets stripped or kept per era, the frame declaration, and the merge entry points.

use super::*;

/// Graft `r`'s owned settings INTO an existing sidecar document, preserving
/// every property AutoShade does not model — Lightroom-only globals
/// (Texture), the camera profile / creative Look, Lightroom's lens-profile
/// block, foreign namespaces, the xpacket wrapper. Save XMP used to
/// REGENERATE the whole document, so copying it beside the RAW wiped all of
/// those from Lightroom's own file (A11).
///
/// `None` means "not safely mergeable" (no crs Description, or markup this
/// scanner cannot splice) — the caller falls back to a fresh document,
/// which is exactly the old behaviour.
///
/// Fully supported masks are replaced wholesale as one block. If any
/// correction is unsupported or partial AND the recipe has no masks of its
/// own, the original block remains byte-for-byte in the existing body and no
/// mask projection is prepended; mixing the two would silently turn an
/// unknown composition into an approximation. When the recipe DOES have
/// masks, the recipe wins — the save in hand IS the newest intent, and
/// keeping the base's foreign block instead published a document whose masks
/// were an older pass's while the develop's own never appeared, with no note
/// (L05#4). The base's block is then dropped from the OUTPUT ONLY (the file
/// it came from is not touched here) and the loss is named in
/// [`MergeOutcome::notes`].
///
/// Owned scalar crs
/// properties are stripped in BOTH forms — attribute and property-element
/// (`<crs:Exposure2012>…</crs:Exposure2012>`, a form Lightroom really
/// writes and the reader really accepts); unowned properties survive in
/// either form. Matching is
/// by the CONVENTIONAL prefixes (`rdf:`, `crs:`) — a document binding either
/// namespace to another prefix (or those prefixes to another URI) is REFUSED
/// here by [`xmlns_conflict`], so the caller regenerates and discloses.
/// Degrading instead spliced a second, contradictory settings block into the
/// user's file behind a clean "saved".
pub struct MergeOutcome {
    pub doc: String,
    /// Losses a SUCCESSFUL merge could not avoid, for the caller's note
    /// channel (the whole-document fallback is disclosed by the caller's own
    /// regeneration note; these are the losses that happen inside a merge
    /// that returns `Some`).
    pub notes: Vec<String>,
    /// The writer's per-mask verdicts for THIS recipe, produced by the same
    /// pass that emitted the mask block ([`owned_children`]) — so a caller that
    /// discloses them does not run the projection a second time (R22 NIT-1).
    /// Identical to [`mask_export_losses`] on the same recipe.
    pub losses: Vec<MaskLoss>,
}

/// What the merge REMOVES from the base document before writing `r`'s own
/// properties back — [`owned_attr_keys`], minus the pass-through keys this
/// particular recipe knows nothing about (R25 B4).
///
/// Owning a key normally means the recipe SPEAKS for it: a grain slider back
/// at 0 means "no grain", the writer omits the key, and the merge's strip is
/// what makes the removal real. A pass-through property has no such state.
/// The map is filled ONLY by reading a document, there is no control that can
/// empty it, and an empty entry therefore never means "the user cleared the
/// camera profile" — it means this recipe came from somewhere that never saw
/// one (a v0.30 `recipe.json` written before the field existed, a paste from
/// another photo, a fresh Analyze). Stripping on that would delete the
/// photographer's Upright correction and camera profile from the file beside
/// their RAW, silently, on an ordinary Ctrl+S — the exact defect class this
/// round is closing, not opening.
///
/// Absent from the map ⇒ not stripped ⇒ the base's own bytes stand. Present
/// ⇒ stripped and rewritten verbatim. Either way exactly one copy survives,
/// which is the duplicate-attribute rule the strip exists for.
fn merge_strip_keys(r: &EditRecipe) -> Vec<String> {
    let era_gated = unspoken_attr_keys(r);
    owned_attr_keys()
        .into_iter()
        .filter(|k| !PASSTHROUGH_CRS.contains(&k.as_str()) || r.passthrough.contains_key(k))
        .filter(|k| !era_gated.contains(k.as_str()))
        .collect()
}

/// The crs ATTRIBUTE keys control-set era `era` gave this writer
/// (`recipe::SCHEMA_ERA_CONTROLS`: era 1 is R25, era 2 v1.5.0), each paired
/// with the registry row that carries it. The CONTROLS are named per era in
/// `recipe.rs`; the key spellings are DERIVED from their registry rows, never
/// hand-copied, because a hand-copied list of spellings is a list that drifts.
/// Era 0, and any era this build does not know, added nothing.
///
/// Membership is by ERA, never by tier. Until v1.5.0 most of R25's list was
/// derived from `Tier::CarriedOnly`, R25's own tier; v1.5.0 renders the
/// carried controls batch by batch, and a control that leaves the tier does
/// not leave R25 — derived from the tier, the eight Detail axes would have
/// dropped out of this gate the day they started rendering, and an ordinary
/// save of a legacy recipe would again have deleted `SharpenRadius` and its
/// neighbours from the photographer's sidecar.
///
/// The B4 PASS-THROUGH keys are deliberately absent from every era: they have
/// a stronger law of their own in [`merge_strip_keys`] (present in the map ⇒
/// ours to rewrite, absent ⇒ never touched), which already answers the
/// question this gate exists for, and answers it for every era.
///
/// Pinned per era by `the_era_gate_is_the_twenty_seven_keys_r25_added` and
/// `the_era_gate_names_the_keys_v1_5_0_added`.
pub(super) fn era_attr_keys(era: u32) -> Vec<(&'static str, &'static str)> {
    use crate::advisor::catalogue::RECIPE_CONTROLS;
    let Some(names) = (era as usize).checked_sub(1).and_then(|i| crate::recipe::SCHEMA_ERA_CONTROLS.get(i))
    else {
        return Vec::new();
    };
    RECIPE_CONTROLS
        .iter()
        .filter(|c| names.contains(&c.name))
        .filter_map(|c| c.crs.attr().map(|k| (c.name, k)))
        .collect()
}

/// The control-set era that introduced registry control `name`
/// (`recipe::SCHEMA_ERA_CONTROLS`), or 0 for a control every recipe has held.
fn control_era(name: &str) -> u32 {
    crate::recipe::SCHEMA_ERA_CONTROLS
        .iter()
        .position(|names| names.contains(&name))
        .map_or(0, |i| i as u32 + 1)
}

/// The control-name prefixes whose keys the writer emits ALL OR NONE — the
/// de-fringe six (R25, `owned_attrs`: a hue window with no amount beside it is
/// a shape no real document has), the parametric seven (v1.5.0, the same
/// argument for a split with no region), and the Calibration seven and the
/// B&W mixer's eight (v1.5.0, whole blocks in every Lightroom file too) — and
/// which [`unspoken_attr_keys`] therefore releases whole.
const WHOLE_BLOCKS: [&str; 4] = ["defringe", "param_", "cal_", "gray_"];

/// The keys a merge must neither STRIP nor EMIT for `r`, because `r` has
/// nothing to say about them.
///
/// TWO reasons a recipe can be silent about a key, and they are different
/// things. The first is the [`crate::recipe::SCHEMA_ERA`] gate below: the
/// recipe predates the control, so serde filled it from the default. The second
/// arrived with v1.5.0 F7 and is not about eras at all — `camera_profile` is a
/// NAME, and an empty name is not "no profile", it is "we were not told".
/// Stripping `crs:CameraProfile` out of a Lightroom document because our own
/// recipe carries no name would delete the photographer's profile choice on an
/// ordinary save, which is the very defect the era gate was built to stop,
/// arriving through a different door.
///
/// **The defect this closes** (R25 P8, one root cause with the mask-block arm
/// in [`merge_recipe_into_xmp`]): a `recipe.json` written by v0.30 has no key
/// for any of R25's twenty-seven, so serde fills them from
/// [`EditRecipe::default`] and the recipe "says" texture 0, no grain, no
/// sharpening radius. Owning a key means the merge STRIPS it before writing
/// ours back, and the writer omits a slider at rest — so an ordinary Ctrl+S on
/// such a recipe deleted `crs:Texture="-20"`, the whole Grain block and the
/// PostCrop/SharpenRadius keys out of the photographer's own Lightroom sidecar,
/// silently, with nothing on screen. Measured on the reference library: three
/// of seven files lost nine keys each. "This file has never held that key" is
/// not "the photographer cleared it", and only the era stamp can tell them
/// apart.
///
/// PER KEY, not per recipe, and that is not a softening — it is what keeps the
/// gate from becoming the next silent loss. An era-0 recipe whose Texture the
/// user has just dragged to +20 differs from the untouched default, and that
/// value IS a statement: suppressing it would mean a legacy photo could never
/// write Texture to its sidecar again, permanently, because nothing ever
/// re-stamps the era of a file. So the gate covers only keys still sitting
/// exactly where serde left them.
///
/// PER ERA as well: a recipe is gated only on the eras newer than its own
/// stamp. An era-1 (v1.4) recipe has held every R25 key all along and owns
/// them; it has never seen v1.5.0's parametric curve, and a Lightroom
/// `ParametricDarks` beside it must survive its save exactly as `Texture`
/// survived a v0.30 one.
///
/// The [`WHOLE_BLOCKS`] move as ONE BLOCK each: the writer emits them all or
/// none, so a gate that released part of one would publish exactly the shape
/// the writer refuses to.
pub(super) fn unspoken_attr_keys(r: &EditRecipe) -> std::collections::BTreeSet<&'static str> {
    // A name we do not hold is a name we must not delete — whatever the era.
    let mut out: std::collections::BTreeSet<&'static str> = Default::default();
    if r.camera_profile.is_empty() {
        out.insert("CameraProfile");
    }
    // The lens profile's identity is written only with a strength that needs
    // it (`owned_attrs`); otherwise the document's own stays.
    if r.lightroom_named_lens_profile().is_none() {
        out.extend(["LensProfileSetup", "LensProfileName", "LensProfileFilename"]);
    }
    // The switch, only when the recipe carries no profile to switch.
    if r.lightroom_lens_profile_enable().is_none() {
        out.insert("LensProfileEnable");
    }
    out.extend(unspoken_era_controls(r).into_iter().map(|(_, k)| k));
    out
}

/// The era half of [`unspoken_attr_keys`]: every control `r`'s schema era
/// predates and `r` leaves at serde's default fill, as `(recipe field, crs
/// key)`. A recipe CARRIES no value for these — and so neither does its
/// payload, which is why `payload::restore` takes the document's.
pub(super) fn unspoken_era_controls(r: &EditRecipe) -> Vec<(&'static str, &'static str)> {
    use crate::advisor::catalogue::global_value;
    if r.schema_era >= crate::recipe::SCHEMA_ERA {
        return Vec::new();
    }
    let neutral = EditRecipe::default();
    let keys: Vec<_> = (r.schema_era + 1..=crate::recipe::SCHEMA_ERA).flat_map(era_attr_keys).collect();
    // An explicit zero (v1.5.0) is a MOVE, though its number is the default's:
    // it is the photographer setting a companion to 0, not serde filling one.
    let untouched = |name: &str| {
        global_value(r, name) == global_value(&neutral, name)
            && !r.explicit_zero.iter().any(|n| n == name)
    };
    let block_untouched =
        |block: &str| keys.iter().filter(|(n, _)| n.starts_with(block)).all(|(n, _)| untouched(n));
    keys.iter()
        .filter(|(n, _)| match WHOLE_BLOCKS.iter().find(|b| n.starts_with(**b)) {
            Some(block) => block_untouched(block),
            None => untouched(n),
        })
        .copied()
        .collect()
}

pub fn merge_recipe_into_xmp(existing: &str, r: &EditRecipe) -> Option<MergeOutcome> {
    merge_recipe_into_xmp_in_frame(existing, r, None)
}

/// What the merged tag must DECLARE about the frame its geometry is written
/// in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameDecl {
    /// The base already says it; touch nothing. This is what keeps a real
    /// Lightroom sidecar's own bytes intact through a round trip.
    Keep,
    /// Only the ORIENTATION disagrees with the frame we are writing in (or the
    /// base declares none and the frame is turned) — set that one attribute
    /// and leave the base's rectangle, namespace and attribute order alone.
    Orientation,
    /// The base mentions no `tiff:` at all — write the whole block.
    Whole,
}

/// Which frame a MERGE writes its geometry in, and what the output must
/// declare so the two agree (R27 A8; the orientation half is W14).
///
/// The rule is that the coordinates and the declaration can never disagree:
///
/// * the base declares a usable RECTANGLE ⇒ that one wins and needs no help.
///   It is what Lightroom itself measured this file's coordinates against, and
///   a sidecar and its photo can legitimately disagree (a re-crop, a proxy).
/// * the base declares none ⇒ the photo's SOURCE rectangle. Before W14 a base
///   that merely MENTIONED `tiff:` got the DISPLAY rectangle and no
///   declaration instead, on the grounds that attributes could not be added to
///   its tag — but 154 of the 174 sidecars in the reference library are
///   exactly that shape (a `tiff:Orientation` with no dimensions), 17 of them
///   over a portrait capture and carrying mask geometry, and for those the
///   display-frame numbers went out under a declaration saying the frame was
///   turned: a quarter turn wrong in Lightroom AND in our own reader.
///
/// The TURN is the photograph's own composed state whenever we know it
/// (`render::quarter_turns_between` is the law), because that is the frame the
/// recipe's coordinates are in. When it differs from what the base declares,
/// the base's `tiff:Orientation` is CORRECTED to match — that is how a
/// rotation made in AutoShade reaches Lightroom, and it is a no-op for a
/// recipe imported from this very sidecar, which is what keeps the round trip
/// byte for byte.
///
/// `desc` is the crs Description's own opening tag with its offset in
/// `existing` (`None` when the document has none and a fresh Description
/// will be inserted). The declaration is corrected ONLY where this engine's
/// own reader will read it back — [`FrameScope::resolve`] narrows to the one
/// Description that declares both dimensions, so the crs Description must
/// be that one, or the reader must be in its whole-document fallback — and
/// only where the old value cannot survive beside the new one (an attribute
/// of that tag, or no declaration anywhere). Anything else — the
/// property-element spelling, a frame kept in a second Description, as
/// exiftool writes it — is left alone and the geometry is written in the
/// frame the file declares, with the note saying so: a correction the reader
/// cannot see would lose the turn on the next import and put the numbers in
/// a frame nobody recovers.
fn merge_frame(
    existing: &str,
    desc: Option<(usize, &str)>,
    photo: Option<FrameAspect>,
) -> (Option<FrameAspect>, FrameDecl, Option<String>) {
    let mentions_tiff = ["xmlns:tiff", "tiff:ImageWidth", "tiff:ImageLength", "tiff:Orientation"]
        .iter()
        .any(|k| find_outside_constructs(existing, k).is_some());
    let base = FrameAspect::from_xmp(existing);
    // The rectangle: the base's when it declares one, else the photograph's.
    let Some(frame) = base.or(photo) else {
        return (None, FrameDecl::Keep, None);
    };
    if !mentions_tiff {
        // Nothing of ours to disturb — declare the whole block, which is the
        // arm that makes a portrait photo's merged sidecar readable at all.
        return (Some(frame), FrameDecl::Whole, None);
    }
    // The turn the recipe's coordinates are actually in. Without a photograph
    // there is nothing to compose, so the base's own declaration stands.
    let Some(want) = photo.map(|p| p.turn()) else {
        return (Some(frame), FrameDecl::Keep, None);
    };
    let frame = FrameAspect { turn: want, ..frame };
    let (scope, scope_start) = FrameScope::resolve_with_start(existing);
    let declared = scope.declared_orientation();
    if declared.unwrap_or(rawler::Orientation::Normal) == want {
        return (Some(frame), FrameDecl::Keep, None);
    }
    // It disagrees, so it has to be rewritten — and it can be rewritten only
    // where this merge already rewrites (as an attribute of the tag it is
    // splicing; anywhere else the old value would survive beside the new
    // one) AND where the reader will look for it (that tag's Description is
    // the reader's scope, or the reader searches the whole document).
    let at_scope = scope_start.is_none_or(|s| desc.is_some_and(|(start, _)| start == s));
    let rewritable = at_scope
        && desc.is_some_and(|(_, t)| {
            xml_attribute_raw(t, "tiff:Orientation").is_some()
                || find_outside_constructs(existing, "tiff:Orientation").is_none()
        });
    if rewritable {
        return (Some(frame), FrameDecl::Orientation, None);
    }
    let kept = declared.unwrap_or(rawler::Orientation::Normal);
    (
        Some(FrameAspect { turn: kept, ..frame }),
        FrameDecl::Keep,
        Some(format!(
            "this sidecar keeps its tiff: frame where a merge cannot rewrite it, so the \
             photo's own orientation ({}) was not written and the geometry was saved in the \
             frame the file declares ({})",
            want.to_u16().max(1),
            kept.to_u16().max(1)
        )),
    )
}

/// The ONE `tiff:Orientation` attribute a merge appends when the base's own
/// disagrees with the frame being written ([`FrameDecl::Orientation`]). The
/// rectangle is the base's and stays untouched; only this property is ours
/// to correct — but the prefix must be BOUND on the tag that uses it. A
/// document may bind `tiff:` on an ancestor rather than on the Description
/// itself, and a prefix no enclosing element binds makes the whole sidecar
/// unreadable to an XML parser, Lightroom's included. So the binding rides
/// along exactly when the tag lacks one: re-binding a prefix an ancestor
/// already binds is legal XML, a duplicate attribute on one tag is not.
fn orientation_declaration(frame: Option<FrameAspect>, bind_prefix: bool) -> String {
    let Some(f) = frame else { return String::new() };
    let binding = if bind_prefix { "\n    xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"" } else { "" };
    format!("{binding}\n    tiff:Orientation=\"{}\"", f.turn().to_u16().max(1))
}

/// [`merge_recipe_into_xmp`] with a FALLBACK frame — the photo's own aspect,
/// for the radial projection ([`FrameAspect`]). The base document's own
/// `tiff:ImageWidth/ImageLength` still wins when it has them: those are what
/// Lightroom itself measured this file's mask coordinates against, and a
/// sidecar and its photo can legitimately disagree (a re-crop, a proxy).
pub fn merge_recipe_into_xmp_in_frame(
    existing: &str,
    r: &EditRecipe,
    frame: Option<FrameAspect>,
) -> Option<MergeOutcome> {
    merge_recipe_into_xmp_in_frame_for_photo(existing, r, frame, None)
}

/// Path-aware merge used when the base can carry MaskBrushTable references.
/// An unchanged imported mask compares through the same ACR-backed reader and
/// therefore keeps the base mask block verbatim instead of synthesizing Paints.
pub fn merge_recipe_into_xmp_in_frame_for_photo(
    existing: &str,
    r: &EditRecipe,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
) -> Option<MergeOutcome> {
    if existing.len() > MAX_XMP_BYTES {
        return None;
    }
    if xmlns_conflict(existing).is_some() {
        return None;
    }
    let mut notes: Vec<String> = Vec::new();
    let photo_frame = frame;
    let Some(desc_start) = find_crs_description(existing) else {
        let (frame, _, note) = merge_frame(existing, None, photo_frame);
        notes.extend(note);


        // A well-formed sidecar that simply carries no camera-raw settings —
        // exiftool / Bridge / Capture One ratings and keywords are the common
        // case. Regenerating over it drops those properties, which the loss
        // note then reported truthfully on EVERY save, forever, with no action
        // the user could take. There is nothing of ours to splice INTO, but
        // there is somewhere to put it: adding our own Description to the
        // existing `rdf:RDF` keeps the file verbatim and makes the merge real.
        return insert_crs_description(existing, r, frame, photo)
            .map(|(doc, losses)| MergeOutcome { doc, notes, losses });
    };
    let (gt, self_closing) = scan_tag_end(existing, desc_start)?;

    // The opening tag: strip every owned crs attribute, then append ours.
    // BOTH quote styles: single-quoted attributes are legal XML, and leaving
    // one behind would duplicate the attribute we append.
    let mut tag = existing[desc_start..=gt].to_string();
    // …and the frame decision, which needs that tag: whether the base's own
    // `tiff:Orientation` is somewhere this splice can correct (W14).
    let (frame, declare_frame, frame_note) =
        merge_frame(existing, Some((desc_start, &tag)), photo_frame);
    notes.extend(frame_note);
    // A corrected orientation replaces the base's attribute rather than
    // joining it — the same strip-then-append discipline the crs keys take one
    // loop down, for the same reason.
    if declare_frame == FrameDecl::Orientation {
        while let Some((span, _)) = xml_attribute_raw(&tag, "tiff:Orientation") {
            let mut left = span.start;
            while left > 0 && tag.as_bytes()[left - 1].is_ascii_whitespace() {
                left -= 1;
            }
            tag.replace_range(left..span.end, "");
        }
    }
    for key in merge_strip_keys(r) {
        let name = format!("crs:{key}");
        while let Some((span, _)) = xml_attribute_raw(&tag, &name) {
            let mut left = span.start;
            while left > 0 && tag.as_bytes()[left - 1].is_ascii_whitespace() {
                left -= 1;
            }
            tag.replace_range(left..span.end, "");
        }
    }
    // The PREVIOUS payload goes the same way (strip, then append ours below).
    // The prefix the base bound to it is remembered so its `Rasters` element
    // can be stripped by its real name; the prefix WE bind steps around one
    // the base binds to something else (`payload::prefix_for`).
    let old_payload_prefix = payload::strip_root_attrs(&mut tag);
    let payload_prefix = payload::prefix_for(Some(&tag));


    // The recipe in the frame the OUTPUT document will be read in (R27) — the
    // base's own declaration when it has one, and it is the base's tag we are
    // rewriting, so the turn has to match what that tag says.
    // The payload's subject: the develop as the app holds it, before the turn
    // below (`crs_description` draws the same line).
    let app = r;
    let turned = in_source_frame(r, frame);
    let r = turned.as_ref();

    let closing_len = if self_closing { 2 } else { 1 };
    let head = tag[..tag.len() - closing_len].trim_end().to_string();
    // The opening tag up to where the payload's attributes go; they are
    // appended at assembly (below), once the document they record is known.
    let new_tag = format!(
        "{head}{tiff}{attrs}",
        tiff = match declare_frame {
            FrameDecl::Whole => frame_declaration(frame),
            FrameDecl::Orientation => {
                orientation_declaration(frame, xml_attribute_raw(&tag, "xmlns:tiff").is_none())
            }
            FrameDecl::Keep => String::new(),
        },
        attrs = owned_attrs(r, frame),
    );

    // The element body: drop every owned child block, then prepend ours.
    // Owned blocks never nest themselves, so a whole-span splice is safe —
    // unlike per-item surgery (the reverted batch-3 attempt).
    let (mut body, tail_start) = if self_closing {
        (String::new(), gt + 1)
    } else {
        let close = find_matching_close(existing, gt + 1)?;
        (existing[gt + 1..close].to_string(), close + "</rdf:Description>".len())
    };
    // Curve/mask child blocks AND owned scalars in PROPERTY-ELEMENT form:
    // Lightroom serialises the same settings as
    // `<crs:Exposure2012>+0.65</crs:Exposure2012>` in plenty of real
    // sidecars (crs_str reads that form for exactly that reason), so an
    // attribute-only strip left the old element value in the body beside
    // the attribute we append — one document, two conflicting answers.
    let mask_scope = crs_own_scope(existing);
    let summary = mask_summary_with_source(
        mask_scope.as_ref(),
        is_autoshade_sidecar(existing),
        frame,
        photo,
        None,
    );
    // Preserve the base's own mask block ONLY while this develop has not
    // moved away from it. The recipe in hand is the newest intent by
    // definition — it is what is being saved right now — so once it differs,
    // ours publish and the base's block goes, WITH the note below. (Ranking
    // file mtimes here instead would misfire: every save flow commits
    // recipe.json before projecting the XMP, so the store always looks newer
    // than the sidecar by the time this runs.)
    //
    // R25 P1 — THE trap of the import unlock. `r.masks.is_empty()` was a
    // usable stand-in for "the user has not touched these" only while a lossy
    // sidecar imported NOTHING. Now that it imports, the develop carries a
    // DEGRADED reading of the base's block (a rotation read as 0, a blend
    // mode ignored, a foreign range left behind), and writing that back over
    // an untouched import would delete the parts we cannot express — silently,
    // on an ordinary Ctrl+S, from the user's own Lightroom file. The question
    // is therefore "did anything move since the import", and it is answered by
    // re-reading the base through the SAME importer the develop came through:
    // equal ⇒ we would only be re-emitting our own approximation, so keep the
    // original bytes; different ⇒ the newest intent is the develop in hand, it
    // publishes, and the note below says so.
    //
    // Ordered so the extra parse is only paid where it decides something: a
    // maskless recipe short-circuits (the old arm, unchanged), and a base with
    // nothing to preserve never reaches the comparison at all.
    //
    // R25 P8 — the SECOND half of that trap, and the reason the predicate
    // moved off `summary.preserve_original` entirely. That flag is set by
    // `MaskSummary::record`, i.e. only when the import was LOSSY, and it was
    // never anything more than a stand-in for "the base has a mask block":
    // while every Lightroom block produced defects the two were the same
    // boolean. P1 made LR blocks import cleanly, and the stand-in came apart —
    // a base whose masks import PERFECTLY, merged with a recipe that has none
    // (a v0.30 `recipe.json` predates mask import; every one of them is
    // maskless), reported preserve_original false, stripped the block and
    // published nothing in its place. Measured on the reference library: four
    // corrections destroyed on one file, eight on another, with an empty note
    // list because the disclosure below was gated on the same flag. So the
    // question is asked directly — DOES THE BASE HAVE A BLOCK — and the
    // answer decides both the preserve and the note.
    let preserve_masks = summary.corrections > 0
        && (r.masks.is_empty()
            || r.masks
                == photo.map_or_else(
                    || xmp_to_recipe(existing).masks,
                    |path| xmp_to_recipe_for_photo(existing, path).masks,
                ));
    if summary.corrections > 0 && !preserve_masks {
        // Two shapes, because the trigger now has two shapes. The defect
        // clause was written when only a LOSSY block could reach here and
        // would have read "carries 0 thing(s) this build cannot represent" on
        // a block we understood perfectly — a sentence that says nothing true.
        // What is always true is the count of corrections being replaced.
        let base = if summary.defects > 0 {
            format!(
                "the merge base's mask block carries {} correction(s), {} thing(s) of which this \
                 build cannot represent",
                summary.corrections, summary.defects
            )
        } else {
            format!("the merge base's mask block carries {} correction(s)", summary.corrections)
        };
        notes.push(format!(
            "{base} — it is not in the new file, which carries this develop's {} edited mask(s) \
             instead (the base file itself is not modified)",
            r.masks.len()
        ));
    }
    // The point colours (v1.5.0): the other owned element a recipe can hold
    // none of for two opposite reasons. A recipe with swatches speaks for the
    // element and replaces the base's. One with none speaks for it only when it
    // could have held some — a recipe of the era that brought them, whose empty
    // list is the photographer's own "no point colour" — and only over a base
    // block this reader understands: an older recipe never saw the element (the
    // era gate's case; the gate's list is attributes, so the stamp is read here
    // directly), and a block the reader refuses is not one the recipe can have
    // been imported from. Lightroom's no-swatch placeholder reads as empty, so
    // it stays where it stands.
    let base_point_colors = parse_point_colors_checked(mask_scope.as_ref());
    let own_point_colors = !r.point_colors.is_empty()
        || (r.schema_era >= control_era("point_colors")
            && base_point_colors.as_ref().is_ok_and(|swatches| !swatches.is_empty()));
    if !r.point_colors.is_empty() && base_point_colors.is_err() {
        notes.push(format!(
            "the merge base's point colours could not be read — they are not in the new file, which \
             carries this develop's {} point colour(s) instead (the base file itself is not modified)",
            r.point_colors.len()
        ));
    }
    // [`OWNED_ELEMENT_ONLY`] is the shared list (the import-side disclosure
    // reads the same one), minus the mask block on the arm that keeps the
    // base's foreign masks verbatim and the point colours on the arm above
    // that keeps the base's.
    let owned_elements: std::collections::HashSet<String> = OWNED_ELEMENT_ONLY
        .iter()
        .filter(|k| !(preserve_masks && **k == "MaskGroupBasedCorrections"))
        .filter(|k| own_point_colors || **k != "PointColors")
        .map(|k| (*k).to_string())
        .chain(merge_strip_keys(r))
        // The base's payload rasters, by the FULL name the base gave them —
        // the one owned element outside the crs namespace.
        .chain(old_payload_prefix.iter().map(|p| format!("{p}:Rasters")))
        .collect();


    // TOP LEVEL ONLY (see `top_level_owned_spans`): the previous flat scan
    // also stripped identically-named children out of the nested Look this
    // merge exists to preserve. Reverse document order — earlier spans keep
    // their indices while later ones are spliced out.
    for (start, end) in top_level_owned_spans(&body, &owned_elements)? {
        body.replace_range(start..end, "");
    }

    let mut head = String::with_capacity(existing.len() + 256);
    head.push_str(&existing[..desc_start]);
    head.push_str(&new_tag);
    let mut out = String::from(">");
    let (children, mut losses) = owned_children(r, !preserve_masks, frame);
    out.push_str(&children);
    // The creative profile: a base that carries a Look keeps its own verbatim
    // (the stricter copy); a base without one gets this recipe's, so a Look
    // the render obeys reaches Lightroom too (`write::look_elem`).
    let look_name: std::collections::HashSet<String> = ["Look".to_string()].into_iter().collect();
    if top_level_owned_spans(&body, &look_name).is_some_and(|s| s.is_empty()) {
        out.push_str(&super::write::look_elem(r));
    }
    let (rasters, mut raster_losses) = payload::rasters_element(app, &payload_prefix, photo);
    losses.append(&mut raster_losses);
    let mut tail = body.trim_end().to_string();
    tail.push_str("\n  </rdf:Description>");
    tail.push_str(&existing[tail_start..]);
    // The document is assembled twice: once WITHOUT the payload, which is the
    // document as written that the payload then records (`payload::
    // written_attrs`), and once with it. A merged document keeps the base's
    // keys this writer does not own (a camera profile, a Look, the lens
    // switch), so a bare projection of the recipe is not what was written,
    // and the reader needs the real one to tell Lightroom's edits from the
    // base's own settings (`payload::restore`).
    let assemble = |attrs: &str, rasters: &str| {
        upgrade_era_marker(refresh_rationale_comment(format!("{head}{attrs}{out}{rasters}{tail}"), r))
    };
    let written = assemble("", "");
    let attrs = format!(
        "{}{}",
        payload::root_attrs(app, &payload_prefix),
        payload::written_attrs(&written, &payload_prefix),
    );
    Some(MergeOutcome { doc: assemble(&attrs, &rasters), notes, losses })
}
