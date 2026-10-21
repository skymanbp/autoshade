//! Masks written as crs XML: geometry, AI and brush masks, the combine spelling, the intent attribute and the range masks.

use super::*;

/// One radial coordinate the way Lightroom spells it: six decimals with the
/// trailing zeros trimmed — `"0.114928"`, `"0.875"`, `"-0.153271"`, `"0"`.
///
/// Six decimals IS Lightroom's precision (every `crs:Top/Left/Bottom/Right/
/// Angle` in the reference sidecars carries at most that many), and the trim is
/// what makes a merged sidecar's untouched radial byte-identical to the one
/// Lightroom wrote instead of merely equal to it — `crs:Bottom="0.875"` must
/// not come back as `"0.875000"`.
pub(super) fn lr_num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    // `-0.0000004` prints as `-0.000000` and trims to `-0`, which is a number
    // no writer should emit.
    if s == "-0" || s.is_empty() { "0".to_string() } else { s.to_string() }
}

/// A stable 32-uppercase-hex GUID derived from `seed` (no external uuid dep).
/// Deterministic so re-emitting the same recipe yields the same sidecar; the
/// per-mask seed includes the index so masks within a file stay unique.
pub(super) fn guid(seed: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h1);
    let a = h1.finish();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    (seed, a).hash(&mut h2);
    let b = h2.finish();
    format!("{a:016X}{b:016X}")
}

/// The ONE inversion bit an XMP radial carries, out of the TWO this recipe
/// spells (R25 P9). `MaskGeometry::Radial::flipped` and `LocalAdjustment::
/// inverted` are separate flags here and `mask_weight` × the weight loop
/// compose them by XOR (render.rs), so their XOR — and only their XOR — is the
/// fact about the photograph. Lightroom has no such pair: it writes the single
/// bit TWICE, as `crs:MaskInverted` on the component and `crs:Flipped` as that
/// value's complement (census of the user's library: 201/201 radials
/// anti-correlated, 0 exceptions; re-derived here on the 7 M-B sidecars, 23/23).
/// So the projection has to collapse, and this is where it collapses.
///
/// Linear gradients get their direction from Zero→Full and Lightroom writes no
/// `crs:Flipped` on one at all (27/27 in the same sidecars), so the `matches!`
/// covers exactly the geometry that has the second flag.
///
/// **All THREE second flags now, not just the radial's.** A brush group's
/// `crs:MaskInverted` and an AI mask's are the same kind of second spelling as
/// `crs:Flipped`, and this function used to cover neither — so
/// `brush_mask_xml` / `ai_mask_xml` wrote the GEOMETRY's bit and silently
/// dropped the correction's, which is how a land zone (or any inverted brush)
/// left here claiming to cover the half it excludes. The composition itself
/// lives in [`LocalAdjustment::net_inverted`], the one place the two bits
/// meet; this stays as the name the writer reads it under.
pub(super) fn lr_net_inverted(m: &LocalAdjustment) -> bool {
    m.net_inverted()
}

/// `(crs:What value, extra geometry attributes)` for a mask geometry, or
/// `None` for geometries classic ACR XMP cannot express (raster bitmaps —
/// the writer skips those corrections; the render still applies them).
/// Coordinates are written raw (unclamped) — ACR gradients legitimately use
/// values outside [0,1].
///
/// `net_inverted` is [`lr_net_inverted`] for the correction this geometry
/// belongs to — the radial arm needs it to write `crs:Flipped`, and the caller
/// writes the SAME bit into `crs:MaskInverted`, so the pair leaves here in the
/// only shape Lightroom itself ever writes.
///
/// The third element of the tuple is the rotation the projection could NOT
/// write, in degrees — `None` whenever the geometry left here whole. See the
/// radial arm.
pub(super) fn mask_geom_xml(
    g: &MaskGeometry,
    net_inverted: bool,
    frame: Option<FrameAspect>,
) -> Option<(&'static str, String, Option<f64>)> {
    match g {
        // A shaped gradient leaves as the plain gradient on its handles: the
        // curve has no attribute, so Lightroom's own law is the approximation.
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }
        | MaskGeometry::Shaped { zero_x, zero_y, full_x, full_y, .. } => Some((
            "Mask/Gradient",
            format!(
                " crs:ZeroX=\"{zero_x}\" crs:ZeroY=\"{zero_y}\" crs:FullX=\"{full_x}\" crs:FullY=\"{full_y}\""
            ),
            None,
        )),
        // v0.32.0: `angle` IS projected onto `crs:Angle` now, and the box is
        // the ROTATED-CORNER encoding Lightroom actually reads — see
        // `engine_to_lr` and `lr_to_engine`, which carry the measurement this
        // rests on. The old stance ("export the UNROTATED ellipse, name the
        // dropped angle") existed because the sign and the pivot were
        // unverified; both are measured, so it retires. It survives in ONE
        // narrower place: a document that declares no frame gives the
        // pixel→normalised fold no aspect to fold with, and then the writer
        // still emits the unrotated ellipse and hands the angle back for the
        // caller to disclose.
        // `flipped: _` — NOT written straight out any more (R25 P9). See
        // `lr_flipped` below: this recipe's flip is half of an XOR, and the
        // attribute it used to be copied into is Lightroom's complement of
        // `crs:MaskInverted`, so copying it emitted pairs Lightroom never
        // writes and Lightroom rendered them inverted.
        MaskGeometry::Radial {
            top, left, bottom, right, feather, roundness, flipped: _, angle, midpoint,
            mask_version,
        } => {
            let (lr, withheld) = engine_to_lr(
                EngineRadial {
                    top: *top as f64,
                    left: *left as f64,
                    bottom: *bottom as f64,
                    right: *right as f64,
                    angle_deg: *angle as f64,
                },
                frame,
            );
            Some((
                "Mask/CircularGradient",
                {
                    // Lightroom's crs:Feather lives on a 0..100 scale (reference
                    // sidecars carry integers like 50 / 72); the engine's is 0..1.
                    // The old writer emitted the raw 0..1 value, which Lightroom
                    // read as a nearly hard edge — convert on the boundary.
                    let lr_feather = (feather.clamp(0.0, 1.0) * 100.0).round();
                    // `crs:Flipped` is the COMPLEMENT of the correction's
                    // `crs:MaskInverted`, which the caller writes from the same
                    // `net_inverted` — 201/201 radials in the user's library and
                    // 23/23 in the M-B sidecars carry exactly that pair, and
                    // NEITHER of the other two combinations occurs even once.
                    //
                    // Emitting the observed pair is what makes this projection
                    // safe under BOTH readings of the attribute: whether
                    // Lightroom's renderer consults `Flipped` or `MaskInverted`,
                    // an anti-correlated pair says the same thing to it. The old
                    // writer copied `flipped` here while `MaskInverted` came from
                    // `inverted`, so a mask the user had flipped in this app left
                    // as `Flipped="false" MaskInverted="false"` — a combination
                    // Lightroom never writes, and the one it reads as "not
                    // inverted", i.e. the flip was dropped on the way out.
                    let lr_flipped = !net_inverted;
                    // Midpoint / Version ride out exactly as they rode in (R25
                    // P5): both sit on EVERY Lightroom radial, and a sidecar we
                    // rewrite without them is a sidecar that lost two of the
                    // file's own attributes to a reader that could not name
                    // them. Neither is interpreted — see MaskGeometry::Radial.
                    //
                    // The five geometry numbers go out through `lr_num` —
                    // Lightroom's own spelling, and the precision the round
                    // trip is stable to. The projection now runs in f64 while
                    // the recipe stores f32, so printing the stored value's own
                    // Display (as the writer did while the box passed through
                    // verbatim) would publish the f32's decimal tail as data.
                    format!(
                        " crs:Top=\"{top}\" crs:Left=\"{left}\" crs:Bottom=\"{bottom}\" \
crs:Right=\"{right}\" crs:Angle=\"{angle}\" \
crs:Feather=\"{lr_feather}\" crs:Roundness=\"{roundness}\" crs:Flipped=\"{lr_flipped}\" \
crs:Midpoint=\"{midpoint}\" crs:Version=\"{mask_version}\"",
                        top = lr_num(lr.top),
                        left = lr_num(lr.left),
                        bottom = lr_num(lr.bottom),
                        right = lr_num(lr.right),
                        angle = lr_num(lr.angle_deg),
                    )
                },
                withheld,
            ))
        }
        MaskGeometry::Bitmap { .. } => None,
        // A brush group is not an ATTRIBUTE-form component: it carries a
        // `crs:Masks` child holding its strokes, each with a `crs:Dabs` child
        // of its own, so it cannot be expressed as this function's
        // `(what, attributes)` pair. [`brush_mask_xml`] emits the whole
        // element instead and [`masks_xml`] routes to it BEFORE reaching here,
        // so the `None` below is never the answer that decides anything — it
        // exists so this function stays total.
        MaskGeometry::Brush { .. } => None,
        // Same shape of exception as the brush group, one element deeper: an
        // AI mask may carry a `crs:Gesture` child, so it is not an
        // attribute-form component either. [`ai_mask_xml`] emits the whole
        // element and [`masks_xml`] routes to it BEFORE reaching here.
        MaskGeometry::AiMask { .. } => None,
    }
}

/// The `crs:` attributes a [`MaskGeometry::AiMask`] carries as PROVENANCE — the
/// ones this engine never interprets, listed in the order Lightroom writes
/// them.
///
/// **An allowlist, both ways.** The parser refuses a `Mask/Image` carrying an
/// attribute outside this list plus the modelled ones (the roundness rule: a
/// name we have never seen means a writer we have not measured), and the writer
/// emits only names from this list — so a hand-edited `recipe.json` cannot
/// smuggle a novel attribute, or markup, into a sidecar.
///
/// Current corpus re-derivation: 175 sidecars; 40 Mask/Aggregate; 104
/// Mask/Image; 391 Mask/Paint; 1037 Mask/*; 40 crs:Gesture (recursive `*.xmp`
/// census over the operator-supplied corpus root (env `AUTOSHADE_CENSUS_ROOT`),
/// real XML parser, 0 parse failures; mask counts refreshed at the v1.1
/// release, sidecar total re-derived 2026-09-17 — the operator edited one
/// more photograph in Lightroom, which carries no mask of any kind). Measured
/// over this corpus: 104 `Mask/Image` instances,
/// 21 distinct attribute names, of which 7 are modelled fields
/// ([`MaskGeometry::AiMask`]) plus `crs:What` and `crs:MaskActive` (an
/// invariant, `"true"` on 104/104) and `crs:MaskSyncID` (re-minted by this
/// writer like every other component's). These eleven are the rest.
///
/// The counts above are the current scoped measurements; the invariants are
/// enforced independently by the vocabulary and parser tests.
/// Historical figures elsewhere in this file and in recipe.rs — the F2 era and
/// the 177-sidecar 2026-08 snapshot both — are retained only as provenance;
/// the active counts above are the re-derived values.
/// A commanded XML census over the operator-supplied corpus root
/// (`AUTOSHADE_CENSUS_ROOT`) and its recursive `*.xmp` files
/// found 177 sidecars, 42 `Mask/Aggregate`, 105 `Mask/Image`, 398 `Mask/Paint`,
/// 1081 `Mask/*` and 40 `crs:Gesture` blocks — none of the older totals came
/// back, and the R28 MANIFEST reported the same divergence independently. The
/// active values above are now the scoped, re-derived corpus counts; historical
/// values remain only as provenance. Nothing in the code
/// depends on a count — the allowlists and the refusals depend on the
/// vocabulary and the invariants, which BOTH censuses agree on.
pub(super) const AI_MASK_PROVENANCE_KEYS: [&str; 11] = [
    "MaskSubCategoryID",
    "InputDigest",
    "InputDigestVersion",
    "MaskDigest",
    "LocalInputDigest",
    "LocalInputDigestVersion",
    "WholeImageArea",
    "FullMaskSize",
    "Origin",
    "ModelVersion",
    "ErrorReason",
];

/// One [`MaskGeometry::AiMask`] as a complete `crs:CorrectionMasks` member: the
/// `Mask/Image` element, its carried provenance attributes, and the
/// `crs:Gesture` list when the photographer refined it with a stroke.
///
/// **What this is and is not.** It is the INTENT riding back out verbatim, so a
/// sidecar this app rewrites still tells Lightroom which segmentation the
/// photographer asked for, at which click, from which model — and Lightroom
/// recomputes its own alpha from that, exactly as it does for its own files.
/// It is NOT a claim that our render matched Adobe's: those pixels came from a
/// different segmenter and the disclosure channels say so
/// ([`MaskLossReason::AiMaskRecomputed`]).
///
/// The provenance digests ride out UNCHANGED on purpose. They describe the
/// intent (which input, which model version), not our raster; re-minting them
/// would assert a provenance we did not have, and dropping them would lose the
/// photographer's own. `crs:MaskSyncID` is the one identity this writer does
/// mint, because that is what it does for every component it emits.
pub(super) fn ai_mask_xml(g: &MaskGeometry, sync_seed: &str, net_inverted: bool) -> Option<String> {
    let (blend, value) = match g {
        MaskGeometry::Brush { blend_mode, value, .. }
        | MaskGeometry::AiMask { blend_mode, value, .. } => (*blend_mode, *value),
        _ => return None,
    };
    ai_mask_xml_spelled(g, sync_seed, (blend, net_inverted, value))
}

pub(super) fn ai_mask_xml_spelled(
    g: &MaskGeometry,
    sync_seed: &str,
    (blend_mode, net_inverted, value): (u32, bool, f32),
) -> Option<String> {
    let MaskGeometry::AiMask {
        name,
        subtype,
        ref_x,
        ref_y,
        blend_mode: _,
        value: _,
        // NOT read here: `net_inverted` is this bit composed with the
        // correction's own through `LocalAdjustment::net_inverted`, which is
        // the only inversion Lightroom has a place for. Writing the raw field
        // dropped `LocalAdjustment::inverted` on the floor.
        inverted: _,
        mask_version,
        provenance,
        gesture,
        ..
    } = g
    else {
        return None;
    };
    let mut extra = String::new();
    for (k, v) in provenance {
        // The allowlist, enforced at the LAST moment before the bytes exist.
        // `recipe.json` is disk input and this string becomes XML someone
        // else's parser reads.
        if AI_MASK_PROVENANCE_KEYS.contains(&k.as_str()) {
            extra.push_str(&format!(" crs:{k}=\"{}\"", xml_attr_escape(v)));
        }
    }
    // The gesture's strokes reuse the Paint spelling exactly — same element,
    // same nine attributes, same three literals-because-they-are-invariants.
    let mut painted = String::new();
    for (k, s) in gesture.iter().enumerate() {
        let dabs: String = s
            .dabs
            .split('\n')
            .map(|t| format!("               <rdf:li>{}</rdf:li>\n", xml_attr_escape(t)))
            .collect();
        painted.push_str(&format!(
            "            <rdf:li>\n\
             <rdf:Description\n\
              crs:What=\"Mask/Paint\" crs:MaskActive=\"true\" crs:MaskBlendMode=\"0\"\n\
              crs:MaskInverted=\"false\" crs:MaskSyncID=\"{id}\" crs:MaskValue=\"{v}\"\n\
              crs:Radius=\"{r}\" crs:Flow=\"{f}\" crs:CenterWeight=\"{cw}\">\n\
             <crs:Dabs>\n\
              <rdf:Seq>\n\
{dabs}              </rdf:Seq>\n\
             </crs:Dabs>\n\
             </rdf:Description>\n\
            </rdf:li>\n",
            id = guid(&format!("{sync_seed}-gesture-{k}")),
            v = s.value,
            r = s.radius,
            f = s.flow,
            cw = s.center_weight,
        ));
    }
    let head = format!(
        "          <rdf:Description\n\
           crs:What=\"Mask/Image\" crs:MaskActive=\"true\" crs:MaskName=\"{mname}\"\n\
           crs:MaskBlendMode=\"{blend_mode}\" crs:MaskInverted=\"{net_inverted}\" \
crs:MaskSyncID=\"{id}\"\n\
           crs:MaskValue=\"{value}\" crs:MaskVersion=\"{mask_version}\" \
crs:MaskSubType=\"{subtype}\"\n\
           crs:ReferencePoint=\"{ref_x} {ref_y}\"{extra}",
        mname = xml_attr_escape(name),
        id = guid(sync_seed),
    );
    Some(if painted.is_empty() {
        // Self-closing when there is no gesture — 64 of 104 current instances.
        format!("         <rdf:li>\n{head}/>\n         </rdf:li>\n")
    } else {
        format!(
            "         <rdf:li>\n{head}>\n\
          <crs:Gesture>\n\
           <rdf:Seq>\n\
{painted}           </rdf:Seq>\n\
          </crs:Gesture>\n\
          </rdf:Description>\n\
         </rdf:li>\n"
        )
    })
}

/// One [`MaskGeometry::Brush`] group as a complete `crs:CorrectionMasks`
/// member: the `Mask/Aggregate` element, its `crs:Masks` list, one
/// `Mask/Paint` per stroke and each stroke's `crs:Dabs` token stream.
///
/// **Byte-faithful to the measured shape** (F2 §1.1 / §2, verified against
/// `P12` Mask 7 → Brush 1): the Aggregate's seven attributes in
/// Lightroom's own order, the Paint's nine in Lightroom's own order, one
/// `<rdf:li>` per dab token. Three of those attributes are LITERALS because
/// they are invariants rather than data — `MaskActive="true"` on both,
/// `MaskBlendMode="0"` and `MaskInverted="false"` on the Paint (398/398), and
/// the reader refuses anything else rather than storing it.
///
/// The numbers go out through plain `Display`, NOT through `local_fmt` or
/// `lr_num`. `f32`'s `Display` prints the shortest decimal that round-trips —
/// so `crs:Radius="0.582157"` comes back as `0.582157`, exactly the string the
/// file used, and `crs:Flow="1"` stays `1` rather than becoming `1.000000`. A
/// rounding formatter would republish a value the photographer never chose.
///
/// That rests on the numbers not being computed on, which R29 C1 narrowed:
/// a TURN rescales `Radius` and rewrites the dab stream
/// (`render::orient_recipe_coords`). The rewrite quantises back onto
/// Lightroom's own six-decimal grid, so a portrait capture's round trip still
/// lands on the file's digits (`a_portrait_captures_brush_turns_on_the_way_in_
/// and_comes_home_on_the_way_out`); what it does NOT promise any more is
/// byte-identity for a frame whose aspect does not close that grid.
///
/// `sync_seed` is hashed into fresh `crs:MaskSyncID`s by the writer's own
/// [`guid`] rule, like every other component this module emits. The IDs the
/// file used are carried in `recipe.json` ([`BrushStroke::sync_id`]) but not
/// re-emitted: a sidecar we rewrite is OUR document, and minting IDs is what
/// the rest of this writer already does.
pub(super) fn brush_mask_xml(g: &MaskGeometry, sync_seed: &str, net_inverted: bool) -> Option<String> {
    let (blend, value) = match g {
        MaskGeometry::Brush { blend_mode, value, .. }
        | MaskGeometry::AiMask { blend_mode, value, .. } => (*blend_mode, *value),
        _ => return None,
    };
    brush_mask_xml_spelled(g, sync_seed, (blend, net_inverted, value))
}

pub(super) fn brush_mask_xml_spelled(
    g: &MaskGeometry,
    sync_seed: &str,
    (blend_mode, net_inverted, value): (u32, bool, f32),
) -> Option<String> {
    // `inverted: _` for `ai_mask_xml`'s reason, word for word: the group's own
    // bit reaches the file only through `LocalAdjustment::net_inverted`.
    let MaskGeometry::Brush { name, blend_mode: _, value: _, inverted: _, strokes } = g else {
        return None;
    };
    let mut painted = String::new();
    for (k, s) in strokes.iter().enumerate() {
        let dabs: String = s
            .dabs
            .split('\n')
            // The storage form joins tokens with '\n' and the reader refuses a
            // token that contains one, so this split is the exact inverse.
            .map(|t| format!("               <rdf:li>{}</rdf:li>\n", xml_attr_escape(t)))
            .collect();
        painted.push_str(&format!(
            "            <rdf:li>\n\
             <rdf:Description\n\
              crs:What=\"Mask/Paint\" crs:MaskActive=\"true\" crs:MaskBlendMode=\"0\"\n\
              crs:MaskInverted=\"false\" crs:MaskSyncID=\"{id}\" crs:MaskValue=\"{v}\"\n\
              crs:Radius=\"{r}\" crs:Flow=\"{f}\" crs:CenterWeight=\"{cw}\">\n\
             <crs:Dabs>\n\
              <rdf:Seq>\n\
{dabs}              </rdf:Seq>\n\
             </crs:Dabs>\n\
             </rdf:Description>\n\
            </rdf:li>\n",
            id = guid(&format!("{sync_seed}-stroke-{k}")),
            v = s.value,
            r = s.radius,
            f = s.flow,
            cw = s.center_weight,
        ));
    }
    Some(format!(
        "         <rdf:li>\n\
          <rdf:Description\n\
           crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\" crs:MaskName=\"{mname}\"\n\
           crs:MaskBlendMode=\"{blend_mode}\" crs:MaskInverted=\"{net_inverted}\" \
crs:MaskSyncID=\"{id}\"\n\
           crs:MaskValue=\"{value}\">\n\
          <crs:Masks>\n\
           <rdf:Seq>\n\
{painted}           </rdf:Seq>\n\
          </crs:Masks>\n\
          </rdf:Description>\n\
         </rdf:li>\n",
        mname = xml_attr_escape(name),
        id = guid(sync_seed),
    ))
}

/// R35: spelling verified in 174 sidecars / 399 corrections (102 composed).
/// Intersect has no third blend value: Lightroom subtracts the inverted shape.
/// The census verifies syntax, not Adobe's arithmetic on feathered alphas.
pub(super) fn combine_spelling(mode: MaskCombine, own_inverted: bool) -> (u32, bool, f32) {
    match mode {
        MaskCombine::Add => (0, own_inverted, 1.0),
        MaskCombine::Subtract => (1, own_inverted, 0.0),
        MaskCombine::Intersect => (1, !own_inverted, 0.0),
    }
}

/// Complementing the WHOLE composed mask distributes through every fold.
/// The engine applies LocalAdjustment::inverted after its components, so
/// projecting that bit onto the base alone would change a composed mask.
pub(super) fn projected_combine(mode: MaskCombine, own: bool, inverted: bool) -> (MaskCombine, bool) {
    if !inverted { return (mode, own); }
    match mode {
        MaskCombine::Add => (MaskCombine::Intersect, !own),
        MaskCombine::Subtract => (MaskCombine::Add, own),
        MaskCombine::Intersect => (MaskCombine::Add, !own),
    }
}

pub(super) const MASK_INTENT_URI: &str = "https://autoshade.dev/ns/mask/1.0/";

pub(super) fn combine_name(mode: MaskCombine) -> &'static str {
    match mode {
        MaskCombine::Add => "add",
        MaskCombine::Subtract => "subtract",
        MaskCombine::Intersect => "intersect",
    }
}

/// Non-Adobe metadata keeps the editor's otherwise ambiguous spelling:
/// Subtract(shape) and Intersect(inverted shape) have identical CRS triples.
/// The writer binds the prefix on every element that uses it, so a document
/// of ours is self-contained; `declared` is the prefix bound to
/// [`MASK_INTENT_URI`] by an ANCESTOR — XML namespace scoping, which the XMP
/// toolkit exercises every time it re-serialises a document: every `xmlns:`
/// moves to the top-level `rdf:Description`, and a reader that looked only
/// at the element itself (R35) stopped seeing intent the moment any other
/// writer had touched the file. A binding on the element still wins either
/// way — a prefix re-bound to another URI there shadows the ancestor's.
pub(super) fn mask_intent_attr<'a>(
    tag: &'a str,
    key: &str,
    declared: bool,
) -> Option<std::borrow::Cow<'a, str>> {
    let bound = match xml_attribute_raw(tag, "xmlns:ash") {
        Some((_, uri)) => xml_unescape(uri) == MASK_INTENT_URI,
        None => declared,
    };
    if !bound { return None; }
    xml_attribute_raw(tag, &format!("ash:{key}")).map(|(_, value)| xml_unescape(value))
}

/// Does any element of `doc` bind the `ash` prefix to [`MASK_INTENT_URI`]?
/// The document-level half of [`mask_intent_attr`]'s scoping. "Any element"
/// rather than "an ancestor" because this reader never holds a parsed tree;
/// every producer that hoists declarations hoists them to the root, so the
/// two answers differ only for a document built to make them differ, and the
/// worst that does is read our own attribute names as ours.
pub(super) fn intent_namespace_declared(doc: &str) -> bool {
    let mut at = 0;
    while let Some((start, gt, _)) = next_xml_tag(doc, at) {
        if xml_attribute_raw(&doc[start..=gt], "xmlns:ash")
            .is_some_and(|(_, uri)| xml_unescape(uri) == MASK_INTENT_URI)
        {
            return true;
        }
        at = gt + 1;
    }
    false
}

pub(super) fn geometry_inversion(g: &mut MaskGeometry, inverted: bool) {
    match g {
        MaskGeometry::Radial { flipped, .. } => *flipped = inverted,
        MaskGeometry::Brush { inverted: own, .. }
        | MaskGeometry::AiMask { inverted: own, .. } => *own = inverted,
        _ => {}
    }
}

/// A `Mask/RangeMask` component `<rdf:li>` intersected with the correction's
/// geometric mask (empty string when the adjustment has no range). Component
/// structure and attribute values verified against the user's own Lightroom
/// sidecars (`P07.xmp` luminance, `P09.xmp` colour): the intersect
/// encoding is `MaskBlendMode="1" + MaskInverted="true" + MaskValue="0"` —
/// i.e. "paint 0 wherever the range does NOT match", which erases everything
/// outside geometry ∩ range. Luminance uses the attribute form
/// (`crs:LumRange="lo_outer lo hi hi_outer"`); colour uses the child-element
/// form with one `crs:PointModels` entry `"r g b px py 0"` (last three numbers
/// assumed sample-point + reserved; see ROADMAP §A for the verification note).
pub(super) fn range_mask_xml(range: &Option<RangeMask>, sync_id: &str) -> String {
    let Some(rm) = range else { return String::new() };
    let head = |name: &str| {
        format!(
            "         <rdf:li>\n\
          <rdf:Description\n\
           crs:What=\"Mask/RangeMask\" crs:MaskActive=\"true\" crs:MaskName=\"{name}\"\n\
           crs:MaskBlendMode=\"1\" crs:MaskInverted=\"true\" crs:MaskSyncID=\"{sync_id}\"\n\
           crs:MaskValue=\"0\">\n"
        )
    };
    match rm {
        RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => format!(
            "{}\
           <crs:CorrectionRangeMask\n\
            crs:Version=\"3\"\n\
            crs:Type=\"2\"\n\
            crs:Invert=\"false\"\n\
            crs:SampleType=\"0\"\n\
            crs:LumRange=\"{lo_outer:.6} {lo:.6} {hi:.6} {hi_outer:.6}\"\n\
            crs:LuminanceDepthSampleInfo=\"0 0.500000 0.500000\"/>\n\
          </rdf:Description>\n\
         </rdf:li>\n",
            head("Luminance Range"),
        ),
        RangeMask::Color { r, g, b, amount, px, py } => format!(
            "{}\
           <crs:CorrectionRangeMask>\n\
            <rdf:Description\n\
             crs:Version=\"3\"\n\
             crs:Type=\"1\"\n\
             crs:ColorAmount=\"{amount:.6}\"\n\
             crs:Invert=\"false\"\n\
             crs:SampleType=\"0\">\n\
            <crs:PointModels>\n\
             <rdf:Seq>\n\
              <rdf:li>{r:.6} {g:.6} {b:.6} {px:.6} {py:.6} 0</rdf:li>\n\
             </rdf:Seq>\n\
            </crs:PointModels>\n\
            </rdf:Description>\n\
           </crs:CorrectionRangeMask>\n\
          </rdf:Description>\n\
         </rdf:li>\n",
            head("Color Range"),
        ),
    }
}
