//! The writer: the recipe rendered as a crs description, the owned attributes and children, the frame declaration, and the recipe_to_xmp entry points.

use super::*;

/// One of a Correction's four LOCAL point curves (R25 P6), or the empty string
/// when that curve is unset — Lightroom writes only the curves that exist.
///
/// **This is deliberately NOT the global `curve_elem`** in [`owned_children`],
/// and the difference is not cosmetic. Three things separate the two spellings,
/// all read off the user's own sidecars (`P51`, `P50`):
///
///   * the KEY is the bare name — `crs:MainCurve` / `RedCurve` / `GreenCurve` /
///     `BlueCurve`, with no `PV2012` suffix, where the globals are
///     `crs:ToneCurvePV2012{,Red,Green,Blue}`;
///   * the POINT payload is `x,y` with **no space after the comma**, where the
///     globals are `x, y` **with** one;
///   * the ELEMENT nests two levels deeper (inside the correction's
///     `rdf:Description`, not the sidecar's), so the indentation differs.
///
/// Sharing one formatter and parameterising the separator would put the two
/// formats one careless "let's make this consistent" edit apart, which is the
/// whole reason this is a second function with this paragraph on it.
/// `local_curve_serialization_has_no_space_after_the_comma` fails the moment
/// they converge.
///
/// The leading spaces follow the reference sidecar's own nesting. They survive
/// into the output where the correction's ATTRIBUTE block's do not — that
/// literal is written with `\` line continuations, which eat the newline and
/// the indent after it. Whitespace is insignificant here either way; this is
/// the spelling Lightroom writes, so it is the one to match.
fn local_curve_elem(tag: &str, points: &[crate::recipe::CurvePoint]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let pts: String = points
        .iter()
        .map(|p| format!("         <rdf:li>{},{}</rdf:li>\n", p.input, p.output))
        .collect();
    format!("       <crs:{tag}>\n        <rdf:Seq>\n{pts}        </rdf:Seq>\n       </crs:{tag}>\n")
}

/// The `crs:CorrectionName` a mask goes out under, and the name every loss
/// verdict about it carries: the user's own label when set; for an unnamed
/// ZONE its role tag (`sky` / `land`), so that a sidecar Lightroom rewrites —
/// which strips the `ash:Role` intent (measured 2026-09-12) — still says which
/// zone this was (`payload::zone_name_role` reads it back, and the reader
/// returns the placeholder to "" exactly as it does `AutoShade <n>`); for any
/// other unnamed mask the `AutoShade <n>` placeholder, as always.
pub(super) fn written_name(i: usize, m: &LocalAdjustment) -> String {
    if !m.name.is_empty() {
        return m.name.clone();
    }
    if m.role.is_zone() {
        return m.role.tag().to_string();
    }
    format!("AutoShade {}", i + 1)
}

/// Build the `<crs:MaskGroupBasedCorrections>` child element (empty string when
/// there are no masks) PLUS the per-mask loss list the export-side disclosure
/// is built from — one loop, so the XML and the claim about it cannot drift.
/// Local sliders convert UI scale → ACR local scale:
/// exposure stops ÷4, every other slider ÷100 (verified against the user's real
/// sidecar; see docs/V2_PLAN.md §2a). All 26 `Local*` fields are emitted (the
/// ones this engine has no model for as 0) as Lightroom expects the full block
/// — `LocalHue` and `LocalSharpness` joined the carried set in R23-1b, and
/// `LocalToningHue` / `LocalToningSaturation` (the mask Color tint) in v1.17.0
/// when the engine learned to render it. Two keys break the ÷100 rule:
/// `LocalHue` is ÷180 (measured, see below) and `LocalToningHue` is written
/// in degrees as stored, the way Lightroom writes it.
pub(super) fn masks_xml(r: &EditRecipe, frame: Option<FrameAspect>) -> (String, Vec<MaskLoss>) {
    let mut losses: Vec<MaskLoss> = Vec::new();
    if r.masks.is_empty() {
        return (String::new(), losses);
    }
    let mut items = String::new();
    for (i, m) in r.masks.iter().enumerate() {
        // The name goes first: it identifies this mask in the loss list even
        // on the arms that never reach the emit below.
        let name = written_name(i, m);
        // The eye toggle: a disabled mask applies nothing, so projecting it
        // as an active correction would make Lightroom render an edit the
        // app does not. Skipped like a Bitmap mask (lossy projection —
        // recipe.json keeps it; re-enable is one click). The alternative,
        // crs:CorrectionActive="false", is unverified against a real
        // sidecar, and the writer's "true" above is a fixed literal.
        //
        // ONE skip verdict per mask, in control-flow order: a muted bitmap
        // mask is skipped BECAUSE it is muted, and claiming both would
        // inflate every count the disclosure prints.
        if !m.enabled {
            losses.push(MaskLoss { name, reason: MaskLossReason::Disabled });
            continue;
        }
        let corr_id = guid(&format!("corr-{i}-{name}"));
        let mask_id = guid(&format!("mask-{i}-{name}"));
        // The single inversion bit both attributes below carry (R25 P9) — the
        // geometry's `crs:Flipped` is its complement, the correction's
        // `crs:MaskInverted` is it. Computed ONCE so the two can never drift.
        let net_inv = lr_net_inverted(m);
        // THREE shapes for the base component now, not two (R27 Batch-4):
        //  * a brush group emits its whole `Mask/Aggregate` element;
        //  * a parametric shape emits the attribute-form `<rdf:li/>` below;
        //  * a raster (bitmap) mask has no classic-XMP encoding at all — skip
        //    this correction; the deterministic render still applies it
        //    (§A tradeoff).
        let mut withheld: Option<f64> = None;
        let base_li = if matches!(m.mask, MaskGeometry::Brush { .. }) {
            match brush_mask_xml(&m.mask, &guid(&format!("brush-{i}-{name}")), net_inv) {
                Some(li) => li,
                None => unreachable!("brush_mask_xml answers every Brush geometry"),
            }
        // FOUR shapes now (R27 Batch-5): an AI mask emits its whole
        // `Mask/Image` element, carrying the intent Lightroom will recompute
        // from.
        } else if matches!(m.mask, MaskGeometry::AiMask { .. }) {
            match ai_mask_xml(&m.mask, &guid(&format!("ai-{i}-{name}")), net_inv) {
                Some(li) => li,
                None => unreachable!("ai_mask_xml answers every AiMask geometry"),
            }
        } else {
            let Some((what, geom, w)) = mask_geom_xml(&m.mask, net_inv, frame) else {
                losses.push(MaskLoss { name, reason: MaskLossReason::Bitmap });
                continue;
            };
            withheld = w;
            format!(
                "         <rdf:li crs:What=\"{what}\" crs:MaskActive=\"true\" crs:MaskName=\"{mname}\"\n\
          crs:MaskBlendMode=\"0\" crs:MaskInverted=\"{inv}\" crs:MaskSyncID=\"{mask_id}\"\n\
          crs:MaskValue=\"1\"{geom}/>\n",
                mname = xml_attr_escape(&format!("{name} mask")),
                // NOT `m.inverted`: a radial's flip is the other half of the
                // same bit (R25 P9, `lr_net_inverted`). Leaving the flip out
                // here is what silently dropped it on export — Lightroom has
                // one inversion flag per correction and this is it.
                inv = net_inv,
            )
        };
        // Every spellable component leaves in list order. Its editor mode is
        // authoritative; carried Brush/Image blend_mode is import provenance.
        let mut extra_lis = String::new();
        let mut flattened = 0usize;
        for (k, c) in m.components.iter().enumerate() {
            let (mode, own) = projected_combine(c.mode, c.net_inverted(), m.inverted);
            let mut spelling = combine_spelling(mode, own);
            // A plain Add carrying zero keeps that observed resting spelling.
            // It is not permission for an imported subtract to override an edit.
            if c.mode == MaskCombine::Add && mode == MaskCombine::Add
                && let MaskGeometry::Brush { value: 0.0, .. }
                | MaskGeometry::AiMask { value: 0.0, .. } = &c.geometry
            {
                spelling.2 = 0.0;
            }
            let seed = guid(&format!("component-{i}-{name}-{k}"));
            let li = brush_mask_xml_spelled(&c.geometry, &seed, spelling)
                .or_else(|| ai_mask_xml_spelled(&c.geometry, &seed, spelling))
                .or_else(|| {
                    let (what, geom, rotation) = mask_geom_xml(&c.geometry, spelling.1, frame)?;
                    if let Some(deg) = rotation {
                        let reason = MaskLossReason::Rotation(deg.round() as i32);
                        if !losses.iter().any(|l| l.name == name && l.reason == reason) {
                            losses.push(MaskLoss { name: name.clone(), reason });
                        }
                    }
                    Some(format!(
                        "         <rdf:li crs:What=\"{what}\" crs:MaskActive=\"true\" crs:MaskName=\"component\"\n\
crs:MaskBlendMode=\"{}\" crs:MaskInverted=\"{}\" crs:MaskSyncID=\"{seed}\" crs:MaskValue=\"{}\"{geom}/>\n",
                        spelling.0, spelling.1, spelling.2,
                    ))
                });
            match li {
                Some(li) => {
                    let intent = format!(
                        "xmlns:ash=\"{MASK_INTENT_URI}\" ash:Combine=\"{}\" ash:OwnInverted=\"{}\" ash:ComponentInverted=\"{}\" ",
                        combine_name(c.mode), c.geometry.own_inverted(), c.inverted,
                    );
                    extra_lis.push_str(&li.replacen("crs:What=", &format!("{intent}crs:What="), 1));
                }
                None => flattened += 1,
            }
        }
        // DEGRADATIONS (the correction IS emitted, just not whole) — each is
        // read off the same field the emitter above ignores, so adding a
        // projection later deletes its entry here in the same edit.
        if flattened > 0 {
            losses.push(MaskLoss {
                name: name.clone(),
                reason: MaskLossReason::ComponentsFlattened,
            });
        }
        // The other direction of loss: the sidecar gets the brush WHOLE and the
        // pixels this engine showed for it came from OUR rasteriser, working
        // from a measured model of Lightroom's brush (R29 Batch-6b). Raised
        // once per mask that carries one anywhere — base or component —
        // because it is a fact about the mask, not about how many groups it
        // holds.
        if matches!(m.mask, MaskGeometry::Brush { .. })
            || m.components.iter().any(|c| matches!(c.geometry, MaskGeometry::Brush { .. }))
        {
            losses.push(MaskLoss { name: name.clone(), reason: MaskLossReason::BrushRendered });
        }
        // The AI mask's own direction of loss, and it is the sharper one: the
        // sidecar gets the INTENT whole (so Lightroom will reproduce its own
        // mask exactly), while the pixels AutoShade showed came from OUR
        // segmenter. The two will not agree at the edges and can disagree
        // badly on a hard scene. Raised once per mask that carries one
        // anywhere, base or component — a fact about the mask, not a count.
        if matches!(m.mask, MaskGeometry::AiMask { .. })
            || m.components.iter().any(|c| matches!(c.geometry, MaskGeometry::AiMask { .. }))
        {
            losses
                .push(MaskLoss { name: name.clone(), reason: MaskLossReason::AiMaskRecomputed });
        }
        // The rotation verdict now comes FROM THE EMITTER (v0.32.0): it is the
        // one place that knows whether the angle reached the document, and
        // re-deriving it here from `angle != 0.0` is exactly how the writer and
        // its own disclosure would drift once the projection started carrying
        // the tilt. `as i32` saturates, so a corrupt angle degrades to 0 ("no
        // angle worth naming") rather than wrapping into a number that is not
        // the mask's.
        if let Some(deg) = withheld {
            let reason = MaskLossReason::Rotation(deg.round() as i32);
            if !losses.iter().any(|l| l.name == name && l.reason == reason) {
                losses.push(MaskLoss { name: name.clone(), reason });
            }
        }
        // Colour gains have no Lightroom key; the mask's Temp and Tint carry
        // them: the pair (and an Exposure offset) whose render through the
        // develop, Camera Raw's measured mask white law included, comes
        // nearest to the gains on a strip of greys (`render::gains_as_white`). Neutral gains (`None` there) leave
        // the mask's own Temp, Tint and Exposure as they are. A projection
        // that sits at the end of Temp's or Tint's range is a recolour the
        // white balance cannot reach: the sidecar carries the nearest one and
        // the loss is named.
        let white = crate::render::gains_as_white(r, m);
        if white.as_ref().is_some_and(|w| w.clamped) {
            losses.push(MaskLoss { name: name.clone(), reason: MaskLossReason::Recolour });
        }
        let (temp, tint, exposure_ev) = match &white {
            Some(w) => (w.temp, w.tint, m.exposure_ev + w.exposure_ev),
            None => (m.temperature / 100.0, m.tint / 100.0, m.exposure_ev),
        };
        // R25 P6. Position is Lightroom's own and verified on two of the
        // user's sidecars: AFTER the attribute block closes at
        // `crs:LocalCurveRefineSaturation`, BEFORE `<crs:CorrectionMasks>`.
        // Sparse — an unset curve contributes nothing, so a mask that only
        // carries Red writes only `<crs:RedCurve>`. Built HERE rather than as
        // an argument below: a `format!` inside another `format!`'s arguments
        // is what `clippy::format_in_format_args` refuses.
        let curves = format!(
            "{}{}{}{}",
            local_curve_elem("MainCurve", &m.main_curve),
            local_curve_elem("RedCurve", &m.red_curve),
            local_curve_elem("GreenCurve", &m.green_curve),
            local_curve_elem("BlueCurve", &m.blue_curve),
        );
        let intent = format!(
            " xmlns:ash=\"{MASK_INTENT_URI}\" ash:Role=\"{}\" ash:Inverted=\"{}\" ash:BaseInverted=\"{}\"",
            m.role.tag(), m.inverted, m.mask.own_inverted(),
        );
        items.push_str(&format!(
            "     <rdf:li>\n\
      <rdf:Description\n\
       crs:What=\"Correction\" crs:CorrectionAmount=\"{amount}\" crs:CorrectionActive=\"true\"{intent}\n\
       crs:CorrectionName=\"{name}\" crs:CorrectionSyncID=\"{corr_id}\"\n\
       crs:LocalExposure=\"0\" crs:LocalHue=\"{hue}\" crs:LocalSaturation=\"{sat}\"\n\
       crs:LocalContrast=\"0\" crs:LocalClarity=\"0\" crs:LocalSharpness=\"{sharp}\"\n\
       crs:LocalBrightness=\"0\" crs:LocalToningHue=\"{thue}\" crs:LocalToningSaturation=\"{tsat}\"\n\
       crs:LocalExposure2012=\"{exp}\" crs:LocalContrast2012=\"{con}\"\n\
       crs:LocalHighlights2012=\"{hi}\" crs:LocalShadows2012=\"{sh}\"\n\
       crs:LocalWhites2012=\"{wh}\" crs:LocalBlacks2012=\"{bl}\"\n\
       crs:LocalClarity2012=\"{cl}\" crs:LocalDehaze=\"{dh}\" crs:LocalLuminanceNoise=\"{nr}\"\n\
       crs:LocalMoire=\"0\" crs:LocalDefringe=\"0\" crs:LocalTemperature=\"{temp}\"\n\
       crs:LocalTint=\"{tint}\" crs:LocalTexture=\"{tex}\" crs:LocalGrain=\"0\"\n\
       crs:LocalCurveRefineSaturation=\"100\">\n\
{curves}\
       <crs:CorrectionMasks>\n\
        <rdf:Seq>\n\
{base_li}{extra_lis}{range}\
        </rdf:Seq>\n\
       </crs:CorrectionMasks>\n\
      </rdf:Description>\n\
     </rdf:li>\n",
            range = range_mask_xml(&m.range, &guid(&format!("range-{i}-{name}"))),
            curves = curves,
            amount = local_fmt(m.amount),
            name = xml_attr_escape(&name),


            corr_id = corr_id,
            sat = local_fmt(m.saturation / 100.0),
            // R23-1b: two keys the writer emitted as a literal "0" from the
            // first sidecar on. Both of Lightroom's own scales are MEASURED
            // now, and they are NOT the same scale, which is why the two lines
            // below divide by different numbers. `LocalHue` is ÷180: the user's
            // controlled export put the mask Hue slider at +50 and the sidecar
            // came back `0.277778` (v0.32.0). `LocalSharpness` is ÷100, settled
            // 2026-08-19 against two Adobe-anchored pairs no library file
            // supplies (Adobe's own Soften Skin preset, +25 → `0.25`, and
            // MIDI2LR at maximum, +100 → `1`). `parse_one_correction` reads each
            // back through the matching multiplier — `q180` and `q100` — and
            // carries the full adjudication of both, so the round-trip is exact
            // in both directions and neither number rests on a family pattern.
            hue = local_fmt(m.hue / 180.0),
            sharp = local_fmt(m.sharpness / 100.0),
            // The Color swatch: the hue in degrees as Lightroom writes it,
            // the strength /100 like every other 0..1 local key.
            thue = local_fmt(m.toning_hue),
            tsat = local_fmt(m.toning_amount / 100.0),
            exp = local_fmt(exposure_ev / 4.0),
            con = local_fmt(m.contrast / 100.0),
            hi = local_fmt(m.highlights / 100.0),
            sh = local_fmt(m.shadows / 100.0),
            wh = local_fmt(m.whites / 100.0),
            bl = local_fmt(m.blacks / 100.0),
            cl = local_fmt(m.clarity / 100.0),
            dh = local_fmt(m.dehaze / 100.0),
            temp = local_fmt(temp),
            tint = local_fmt(tint),
            tex = local_fmt(m.texture / 100.0),
            nr = local_fmt(m.noise_reduction / 100.0),
            base_li = base_li,
            extra_lis = extra_lis,
        ));
    }
    // All masks may have been raster-skipped — no empty wrapper block then.
    if items.is_empty() {
        return (String::new(), losses);
    }
    (
        format!(
            "\n   <crs:MaskGroupBasedCorrections>\n    <rdf:Seq>\n{items}    </rdf:Seq>\n   </crs:MaskGroupBasedCorrections>"
        ),
        losses,
    )
}

/// Does a detail/NR COMPANION key ride out at zero because its group's AMOUNT
/// is set? (R27 T4, `P2-feather-k-closures.md` §4.)
///
/// Lightroom's rule is **amount-gated, not per-key** — over 211 Adobe exports
/// carrying `crs:LuminanceSmoothing`, all 4 with it > 0 carry the Detail
/// companion and none of the 207 with it = 0 does; 133/133 with
/// `ColorNoiseReduction > 0` carry Detail + Smoothness and 0 of 64 with it = 0
/// do; `SharpenEdgeMasking="0"` rides on 159 files whose `Sharpness` is set.
/// This writer gated each companion on ITS OWN value, so a recipe with
/// `LuminanceSmoothing="50"` and a contrast of 0 emitted the amount and the
/// Detail but DROPPED `LuminanceNoiseReductionContrast` — a shape no Lightroom
/// file has.
///
/// **Only the companions whose ACR default is ZERO join**, and that is the
/// whole of the alignment. `LuminanceNoiseReductionContrast` and
/// `SharpenEdgeMasking` default to 0 in ACR, so emitting `"0"` says exactly
/// what their absence said and the change is byte-level only. The others
/// (`SharpenRadius` 1.0, `SharpenDetail` 25, the two Detail/Smoothness 50s)
/// default to a NON-zero value, and this recipe's 0 means "never learned one"
/// — emitting it would tell Lightroom to sharpen at radius 0 or drop detail
/// retention from 50 to 0, which is a render change, not a spelling. Those
/// keep reaching Lightroom by ABSENCE, which is the honest encoding and the
/// rule `owned_attrs`' vignette/grain block states for the same reason —
/// UNLESS the recipe names the control in `explicit_zero` (v1.5.0), where the
/// 0 is a value the photographer set and goes out as one. That is not this
/// function's rule: it is the companion's own, applied beside it.
///
/// **`ColorNoiseReduction` is the one key that goes out at ZERO despite a
/// non-zero ACR default (v1.3.1).** The argument above — "our 0 means never
/// learned, absence keeps Lightroom's default" — holds for a control this
/// engine renders at that default. `color_nr` is not one: its 0 renders NO
/// colour noise reduction here (none at all before v1.5.0, the luma-guided
/// operator's amount-0 no-op since), so the recipe's 0 is the truth of the
/// render, and an absent key made Lightroom apply its RAW default of 25 to a
/// photo the app showed without it. Measured on the Lightroom check of the
/// v1.3.0 sidecars (2026-09-12): Lightroom materialised
/// `ColorNoiseReduction="25"` into every rewritten file. Writing the zero is
/// what makes the two renders describe one photo; the Detail/Smoothness
/// companions stay at-rest-absent (they do nothing at amount 0, and
/// Lightroom writes its 50/50 back regardless — which the payload reader
/// treats as a materialisation, not an edit).
fn amount_carries(key: &str, amount: f32) -> bool {
    key == "ColorNoiseReduction"
        || (matches!(key, "LuminanceNoiseReductionContrast" | "SharpenEdgeMasking")
            && amount != 0.0)
}

/// Every crs ATTRIBUTE the writer owns, rendered for `r` — the
/// `\n    crs:K="v"` block. One authority for what AutoShade owns in a
/// sidecar, shared by the fresh-document writer and the merge path
/// ([`merge_recipe_into_xmp`]); the REMOVAL universe lives in
/// [`owned_attr_keys`] and must cover every key this can ever emit.
pub(super) fn owned_attrs(r: &EditRecipe, frame: Option<FrameAspect>) -> String {
    let mut a = String::new();
    // The SCHEMA-ERA gate's emission half (R25 P8). It has to sit beside the
    // strip half in [`merge_strip_keys`] and agree with it exactly: strip
    // without emit deletes the base's value, emit without strip leaves two
    // copies of the same attribute in one tag. `unspoken_attr_keys`
    // returns the ONE set both consult — empty for every current-era recipe,
    // so this costs nothing on the ordinary path.
    let era_gated = unspoken_attr_keys(r);
    let ungated = |key: &str| !era_gated.contains(key);
    // Does a stored value STATE something? Non-zero always does; a zero does
    // when the recipe names the control in `explicit_zero` (v1.5.0) — the
    // COMPANIONS' real 0, which absence cannot say.
    let states = |name: &str, v: f32| v != 0.0 || r.explicit_zero.iter().any(|n| n == name);

    // ProcessVersion 15.4 / Version 15.5.1 are the verified current values from
    // the user's real sidecar (not the research's guessed 11.0/15.0).
    attr(&mut a, "Version", "15.5.1");
    attr(&mut a, "ProcessVersion", "15.4");

    // White balance: an explicit temperature means Custom WB — and
    // Temperature is ABSOLUTE Kelvin in both models now that the engine
    // anchors at the stamped as-shot (`as_shot_k`), so the number finally
    // means the same thing to Lightroom. A tint-only edit on a STAMPED photo
    // emits Custom pinned AT the as-shot Kelvin (exactly what Lightroom
    // itself writes for a tint-only move): under "As Shot" Lightroom may
    // re-read camera metadata and ignore the Tint attribute entirely — the
    // old documented lossy edge, closed wherever the engine knows the K.
    // A LEGACY recipe (no stamp) keeps the old honest fallback: "As Shot"
    // plus the tint, still disclosed as lossy; recipe.json carries the tint
    // losslessly either way.
    let wb_kelvin = r.temperature_k.or(if r.tint != 0.0 { r.as_shot_k } else { None });
    if let Some(k) = wb_kelvin {
        attr(&mut a, "WhiteBalance", "Custom");
        attr(&mut a, "Temperature", &(k.round() as i64).to_string());
        attr(&mut a, "Tint", &signed(r.tint));
    } else {
        attr(&mut a, "WhiteBalance", "As Shot");
        if r.tint != 0.0 {
            attr(&mut a, "Tint", &signed(r.tint));
        }
    }

    // Exposure as a plain decimal (Lightroom parses signed or unsigned).
    attr(&mut a, "Exposure2012", &format!("{:.2}", r.exposure_ev));
    attr(&mut a, "Contrast2012", &signed(r.contrast));
    attr(&mut a, "Highlights2012", &signed(r.highlights));
    attr(&mut a, "Shadows2012", &signed(r.shadows));
    attr(&mut a, "Whites2012", &signed(r.whites));
    attr(&mut a, "Blacks2012", &signed(r.blacks));
    attr(&mut a, "Clarity2012", &signed(r.clarity));
    attr(&mut a, "Dehaze", &signed(r.dehaze));
    attr(&mut a, "Vibrance", &signed(r.vibrance));
    attr(&mut a, "Saturation", &signed(r.saturation));
    // Global Texture (R25 B2) — the last Basic-panel slider that had no key.
    // UNCONDITIONAL like its four neighbours above: Lightroom writes
    // `crs:Texture="+26"` in the signed form (verified in the user's library),
    // and a recipe that says 0 is stating a value, not omitting one — UNLESS
    // it is an era-0 recipe that never had the field at all, which is the one
    // case where 0 states nothing (see the gate above).
    if ungated("Texture") {
        attr(&mut a, "Texture", &signed(r.texture));
    }
    // The parametric tone curve (v1.5.0), right after the Basic-panel keys,
    // where Lightroom writes it. ALL SEVEN OR NONE: Lightroom writes the whole
    // block on every sidecar, and a split with no region beside it is a shape
    // no real document has. Emitted when anything in the block differs from
    // Adobe's defaults, so a plain recipe still produces a minimal sidecar —
    // and a merge onto a Lightroom file carrying the defaults loses nothing by
    // stripping them, the defaults being what an absent key means. The regions
    // are SIGNED like the 2012 sliders; the splits are unsigned integers. One
    // key answers for the block at the era gate, which releases it whole.
    let splits = [r.param_shadow_split, r.param_midtone_split, r.param_highlight_split];
    if (r.parametric_regions().is_some() || splits != crate::recipe::PARAMETRIC_SPLITS)
        && ungated("ParametricShadows")
    {
        attr(&mut a, "ParametricShadows", &signed(r.param_shadows));
        attr(&mut a, "ParametricDarks", &signed(r.param_darks));
        attr(&mut a, "ParametricLights", &signed(r.param_lights));
        attr(&mut a, "ParametricHighlights", &signed(r.param_highlights));
        attr(&mut a, "ParametricShadowSplit", &(splits[0].round() as i64).to_string());
        attr(&mut a, "ParametricMidtoneSplit", &(splits[1].round() as i64).to_string());
        attr(&mut a, "ParametricHighlightSplit", &(splits[2].round() as i64).to_string());
    }

    // Per-colour HSL / Color mixer (8 ACR bands). Emit only when non-neutral so
    // a plain global recipe still produces a minimal, v1-compatible sidecar.
    if !r.hsl.is_neutral() {
        for (i, band) in crate::recipe::HSL_BANDS.iter().enumerate() {
            attr(&mut a, &format!("HueAdjustment{band}"), &signed(r.hsl.hue[i]));
            attr(&mut a, &format!("SaturationAdjustment{band}"), &signed(r.hsl.saturation[i]));
            attr(&mut a, &format!("LuminanceAdjustment{band}"), &signed(r.hsl.luminance[i]));
        }
    }
    // The B&W mixer (v1.5.0), where Lightroom writes it — the HSL block's
    // place — ALL EIGHT OR NONE, signed like the HSL cells. Lightroom writes
    // the eight on every black-and-white photo (at 0 until one moves) and none
    // on a colour one; a mix moved on a colour photo still goes out, being a
    // value the recipe holds and a later B&W switch renders. One key answers
    // for the block at the era gate, which releases it whole.
    let gray = r.gray_mixer();
    if (r.convert_to_grayscale || gray.iter().any(|v| *v != 0.0)) && ungated("GrayMixerRed") {
        for (band, v) in crate::recipe::HSL_BANDS.iter().zip(gray) {
            attr(&mut a, &format!("GrayMixer{band}"), &signed(v));
        }
    }

    // Colour grading (3-wheel + global). ACR convention VERIFIED against the
    // user's own sidecar: shadow/highlight hue+sat round-trip via the legacy
    // SplitToning* keys; lum, midtone, global, blending via ColorGrade*; balance
    // via SplitToningBalance. Hue/sat/blending are unsigned, lum/balance signed.
    if !r.color_grade.is_neutral() {
        let cg = &r.color_grade;
        let uns = |v: f32| (v.round() as i64).to_string();
        attr(&mut a, "SplitToningShadowHue", &uns(cg.shadow_hue));
        attr(&mut a, "SplitToningShadowSaturation", &uns(cg.shadow_sat));
        attr(&mut a, "SplitToningHighlightHue", &uns(cg.highlight_hue));
        attr(&mut a, "SplitToningHighlightSaturation", &uns(cg.highlight_sat));
        attr(&mut a, "SplitToningBalance", &signed(cg.balance));
        attr(&mut a, "ColorGradeShadowLum", &signed(cg.shadow_lum));
        attr(&mut a, "ColorGradeMidtoneHue", &uns(cg.midtone_hue));
        attr(&mut a, "ColorGradeMidtoneSat", &uns(cg.midtone_sat));
        attr(&mut a, "ColorGradeMidtoneLum", &signed(cg.midtone_lum));
        attr(&mut a, "ColorGradeHighlightLum", &signed(cg.highlight_lum));
        attr(&mut a, "ColorGradeGlobalHue", &uns(cg.global_hue));
        attr(&mut a, "ColorGradeGlobalSat", &uns(cg.global_sat));
        attr(&mut a, "ColorGradeGlobalLum", &signed(cg.global_lum));
        attr(&mut a, "ColorGradeBlending", &uns(cg.blending));
    }

    // 1:1 with the Detail > Sharpening "Amount" slider, whose UI maximum IS
    // 150 — see the reader for the evidence that retired the old ×⅔ rescale.
    // Round + clamp to the same band the recipe row states, no scale change.
    // A COMPANION since v1.6.0: an absent amount leaves the key out, and
    // Lightroom applies its own default for the kind of file — 40 on a RAW,
    // 0 on a JPEG — which is what the engine renders
    // (`EditRecipe::capture_sharpening`); a real 0 is stated like any other.
    if states("sharpening", r.sharpening) {
        let sharp = (r.sharpening.round() as i64).clamp(0, 150);
        attr(&mut a, "Sharpness", &sharp.to_string());
    }
    // SharpenRadius is the one DECIMAL key in the detail block, and the one
    // Lightroom writes with an explicit `+`: `crs:SharpenRadius="+1.0"` in all
    // seven of the user's sidecars (the integer neighbours are bare —
    // `SharpenDetail="25"`, `SharpenEdgeMasking="0"`). Emitted only when it
    // states something, so an absent radius stays absent and Lightroom keeps
    // its own 1.0.
    if states("sharpen_radius", r.sharpen_radius) && ungated("SharpenRadius") {
        attr(&mut a, "SharpenRadius", &format!("{:+.1}", r.sharpen_radius));
    }
    for (key, name, v, amount) in [
        ("SharpenDetail", "sharpen_detail", r.sharpen_detail, r.sharpening),
        ("SharpenEdgeMasking", "sharpen_mask", r.sharpen_mask, r.sharpening),
    ] {
        if (states(name, v) || amount_carries(key, amount)) && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }
    let nr = (r.noise_reduction.round() as i64).clamp(0, 100);
    attr(&mut a, "LuminanceSmoothing", &nr.to_string());
    // The rest of the R25 B3 detail axes, in Lightroom's own key order
    // (verified against the user's sidecars: Sharpness, SharpenRadius,
    // SharpenDetail, SharpenEdgeMasking, LuminanceSmoothing, then the colour
    // NR trio). Same per-key "only when it states something" rule as the B2
    // effects — and the same reason: an absent zero here means "the sidecar
    // said nothing", and the companions Lightroom itself omits when the
    // amount is zero (ColorNoiseReductionDetail / Smoothness are absent from
    // the two files whose ColorNoiseReduction is 0) must stay absent from ours
    // too.
    for (key, name, v, amount) in [
        ("LuminanceNoiseReductionDetail", "nr_detail", r.nr_detail, r.noise_reduction),
        ("LuminanceNoiseReductionContrast", "nr_contrast", r.nr_contrast, r.noise_reduction),
        ("ColorNoiseReduction", "color_nr", r.color_nr, 0.0),
        ("ColorNoiseReductionDetail", "color_nr_detail", r.color_nr_detail, r.color_nr),
        ("ColorNoiseReductionSmoothness", "color_nr_smooth", r.color_nr_smooth, r.color_nr),
    ] {
        if (states(name, v) || amount_carries(key, amount)) && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }

    // Manual lens-vignette correction. `VignetteAmount` name verified against
    // the user's sidecars (present, =0, in 140 of them); the Midpoint companion
    // key follows the documented ACR pair and is only emitted when the amount
    // is set — a zero-amount recipe stays byte-identical to the old writer.
    if r.lens_vignette != 0.0 {
        attr(&mut a, "VignetteAmount", &signed(r.lens_vignette));
        attr(&mut a, "VignetteMidpoint", &(r.lens_vignette_mid.round() as i64).to_string());
    }

    // Manual distortion correction — key name verified against the user's
    // sidecars (`LensManualDistortionAmount="0"` in 148 of them). Same
    // only-when-set policy as the vignette pair. NB: our render's amount→curve
    // gain is our own calibration; Adobe's is unpublished, so LR's slider at
    // the same number may correct a somewhat different physical strength.
    if r.lens_distortion != 0.0 {
        attr(&mut a, "LensManualDistortionAmount", &signed(r.lens_distortion));
    }

    // The profile switch the develop rendered under
    // ([`EditRecipe::lightroom_lens_profile_enable`]): the frame Lightroom
    // places every mask in. Owned only while stated (`unspoken_attr_keys`).
    if let Some(on) = r.lightroom_lens_profile_enable() {
        attr(&mut a, "LensProfileEnable", on);
    }

    // The profile a strength needs NAMED (2026-10-28): Camera Raw honours the
    // two strengths below only for a profile crs:LensProfileName names, and
    // renders them at 100 otherwise, so the export names it whenever a
    // strength is off 100 ([`EditRecipe::lightroom_named_lens_profile`], which
    // with the setup [`EditRecipe::lightroom_lens_setup`]). In Lightroom's own key order. The
    // three keys are owned only while written (`unspoken_attr_keys`): a
    // Lightroom file at 100 keeps its own setup, name, file name and digest.
    if let Some(id) = r.lightroom_named_lens_profile() {
        attr(&mut a, "LensProfileSetup", r.lightroom_lens_setup());
        attr(&mut a, "LensProfileName", &id.name);
        attr(&mut a, "LensProfileFilename", &id.filename);
    }

    // The PROFILE's two strengths (v1.5.0). Written when they say something —
    // which here means "away from 100", not "away from 0": 100 is Lightroom's
    // own neutral and 0 is a real value meaning "switch this component off".
    // The era gate applies to both, so an era-1 recipe that never saw them
    // leaves whatever the base document says untouched.
    for (key, v) in [
        ("LensProfileDistortionScale", r.lens_profile_distortion_scale),
        ("LensProfileVignettingScale", r.lens_profile_vignetting_scale),
    ] {
        if v != 100.0 && ungated(key) {
            attr(&mut a, key, &format!("{}", v.round() as i32));
        }
    }

    // Lightroom's TRANSFORM panel (v1.5.0 F6). Only-when-set, and each key in
    // the SPELLING measured in this operator's 175 sidecars rather than
    // guessed — the three formats really do differ:
    //
    //   PerspectiveVertical/Horizontal/Aspect/Scale   plain integers ("0", "-22", "-35", "100")
    //   PerspectiveRotate                             one decimal WITH an explicit sign ("0.0", "-0.6", "+0.9")
    //   PerspectiveX/Y                                two decimals ("0.00")
    //
    // Lightroom writes all eight on every photo, at neutral or not; this writer
    // only adds what is off neutral, and the merge leaves the document's own
    // neutral bytes alone — so a Lightroom file keeps the block it had.
    //
    // ONE SPELLING IS EXTRAPOLATED, not measured: whether a non-zero X or Y
    // carries an explicit `+`. No photograph in the 175-sidecar library has a
    // non-zero offset, so the library cannot say — and the one non-zero
    // fractional key it DOES carry, `PerspectiveRotate="+0.9"`, is signed, so
    // the two offsets are written the same way. A future kit export with the
    // X slider moved settles it; until then this is the honest guess and it is
    // marked as one.
    for (key, v) in [
        ("PerspectiveVertical", r.perspective_vertical),
        ("PerspectiveHorizontal", r.perspective_horizontal),
        ("PerspectiveAspect", r.perspective_aspect),
    ] {
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }
    if r.perspective_scale != 100.0 && ungated("PerspectiveScale") {
        attr(&mut a, "PerspectiveScale", &(r.perspective_scale.round() as i64).to_string());
    }
    if r.perspective_rotate != 0.0 && ungated("PerspectiveRotate") {
        attr(&mut a, "PerspectiveRotate", &format!("{:+.1}", r.perspective_rotate));
    }
    for (key, v) in [("PerspectiveX", r.perspective_x), ("PerspectiveY", r.perspective_y)] {
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &format!("{v:+.2}"));
        }
    }
    // The Upright MODE, an integer. Written only when a mode is chosen, for
    // `AutoLateralCA`'s reason: a recipe that never met the key must not start
    // asserting "off" into someone's document.
    if r.perspective_upright != 0.0 && ungated("PerspectiveUpright") {
        attr(&mut a, "PerspectiveUpright", &(r.perspective_upright.round() as i64).to_string());
    }
    // Constrain Crop, the 0/1 flag Lightroom writes — `"0"` on all 52 of this
    // library's sidecars that carry it, so ON is the only thing worth saying.
    if r.crop_constrain_to_warp && ungated("CropConstrainToWarp") {
        attr(&mut a, "CropConstrainToWarp", "1");
    }
    // `crs:UprightTransform_0…N` are deliberately NOT written: they are
    // Adobe's own solution for this photograph, read and rendered
    // (`recipe::EditRecipe::upright_transform`, tier `RenderedNotExported`), and
    // the merge preserves the document's own bytes for us. Writing them would
    // mean claiming Adobe's key for numbers our own solver produced.

    // Manual lateral CA (R25 B3), same only-when-set policy. UNSIGNED: these
    // are the legacy PV2010 integer keys, so they belong to the
    // `Sharpness="40"` family rather than the `Contrast2012="+22"` one. No
    // sidecar in the user's library carries either key (Lightroom's PV2012
    // panel replaced the pair with de-fringe and the auto switch), so the
    // spelling follows the family convention rather than a measurement — and
    // an absent key is what every one of those files already has.
    for (key, v) in [("ChromaticAberrationR", r.ca_r), ("ChromaticAberrationB", r.ca_b)] {
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }
    // Adobe's auto-CA switch, written as the 0/1 flag Lightroom writes
    // (`crs:AutoLateralCA="0"` on six of the user's sidecars, `="1"` on the
    // seventh) — and only when ON, so a recipe that never met the key does
    // not start asserting "off" into someone's document.
    if r.auto_lateral_ca && ungated("AutoLateralCA") {
        attr(&mut a, "AutoLateralCA", "1");
    }

    // De-fringe (R25 B3): all six keys, UNCONDITIONALLY, which is the shape
    // Lightroom itself writes — 7 of 7 of the user's sidecars carry the whole
    // block with the amounts at 0 and the hue windows at Adobe's 30/70 and
    // 40/60. Writing only the non-default ones would emit a hue window with
    // no amount beside it (or the reverse), a shape no real document has.
    // Unsigned integers: `DefringePurpleAmount="3"`, never `"+3"` — the
    // `Sharpness` family again.
    for (key, v) in [
        ("DefringePurpleAmount", r.defringe_purple),
        ("DefringePurpleHueLo", r.defringe_purple_lo),
        ("DefringePurpleHueHi", r.defringe_purple_hi),
        ("DefringeGreenAmount", r.defringe_green),
        ("DefringeGreenHueLo", r.defringe_green_lo),
        ("DefringeGreenHueHi", r.defringe_green_hi),
    ] {
        // The era gate releases these six together or not at all (see
        // `unspoken_attr_keys`), so this per-key test can never split
        // the block the paragraph above insists on writing whole.
        if ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }

    // The nine CARRIED effects (R25 B2): Lightroom renders them, we do not,
    // and the sidecar is the whole point of modelling them at all.
    //
    // PER-KEY conditional, not the vignette pair's group gate. The pair above
    // needs one because `lens_vignette_mid`'s neutral is 50 and it has no
    // "absent" spelling; every field here is neutral at ZERO, so writing only
    // the non-zero ones IS "write what is non-neutral" — and the three whose
    // ACR default is not zero (Midpoint/Feather 50, Style 1) then reach
    // Lightroom by ABSENCE, which is the honest encoding of "the recipe never
    // learned one" and cannot invent a Midpoint of 0. Verified round-trip
    // against the user's own sidecars: Lightroom writes the companions only
    // when the amount is non-zero, so an imported file comes back the same
    // shape it went in.
    for (key, v) in [
        ("PostCropVignetteAmount", r.post_crop_vignette),
        ("PostCropVignetteRoundness", r.post_crop_vignette_round),
    ] {
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &signed(v));
        }
    }
    for (key, name, v) in [
        ("PostCropVignetteMidpoint", "post_crop_vignette_mid", r.post_crop_vignette_mid),
        ("PostCropVignetteFeather", "post_crop_vignette_feather", r.post_crop_vignette_feather),
        ("PostCropVignetteStyle", "post_crop_vignette_style", r.post_crop_vignette_style),
        ("PostCropVignetteHighlightContrast", "post_crop_vignette_hl", r.post_crop_vignette_hl),
        ("GrainAmount", "grain", r.grain),
        ("GrainSize", "grain_size", r.grain_size),
        ("GrainFrequency", "grain_rough", r.grain_rough),
    ] {
        // `ungated` is redundant for every value that states something — it
        // has already left the default the gate keys on (an explicit zero
        // counts as moved, `unspoken_attr_keys`) — and it is written
        // all the same, on both loops and on every R25 key below and above:
        // the gate is the LAW for these twenty-seven, and a law spelled at
        // only the sites that need it today is a law the next default change
        // quietly repeals.
        if states(name, v) && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }

    // Camera Calibration (v1.5.0), after the effects as Lightroom orders it.
    // ALL SEVEN OR NONE, signed (`crs:BlueHue="-28"`, `crs:ShadowTint="+3"` in
    // the user's library), and only when a slider moved: Lightroom writes the
    // block at 0 on nearly every file, and 0 is what an absent key means, so a
    // plain recipe still produces a minimal sidecar. One key answers for the
    // block at the era gate, which releases it whole.
    if let Some(cal) = r.calibration()
        && ungated(CALIBRATION_CRS[0])
    {
        for (key, v) in CALIBRATION_CRS.into_iter().zip(cal) {
            attr(&mut a, key, &signed(v));
        }
    }
    // The B&W switch (v1.5.0), after the calibration block as Lightroom orders
    // it, and only when ON: `"False"` is what absence means, and a recipe that
    // never met the switch must not start asserting it into someone's file.
    if r.convert_to_grayscale && ungated("ConvertToGrayscale") {
        attr(&mut a, "ConvertToGrayscale", "True");
    }
    // HDR edit mode and its SDR rendition (v1.5.0 F8). Each key on its own
    // condition, the B&W switch's rule: absent IS the neutral for all nine, so
    // a photograph that never entered HDR mode produces the same bytes it did
    // before this batch, and a merge onto a Lightroom file carrying
    // `crs:HDREditMode="0"` loses nothing by stripping it.
    //
    // `HDREditMode` is Lightroom's "0"/"1" and NOT its "True"/"False" — the
    // two boolean spellings really do sit in one sidecar, three attributes
    // apart, and writing the wrong one produces a file Lightroom reads as OFF.
    // `HDRMaxValue` is a DECIMAL like `SharpenRadius`, at two places
    // (`crs:HDRMaxValue="+1.00"` in the user's library).
    if r.hdr_edit && ungated("HDREditMode") {
        attr(&mut a, "HDREditMode", "1");
    }
    if r.hdr_max_ev != 0.0 && ungated("HDRMaxValue") {
        attr(&mut a, "HDRMaxValue", &format!("{:+.2}", r.hdr_max_ev));
    }
    for (key, v) in SDR_CRS.into_iter().zip(r.sdr_controls()) {
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &signed(v));
        }
    }

    // The PASS-THROUGH block (R25 B4): Lightroom's own Upright bookkeeping,
    // written back as the exact strings it arrived as.
    //
    // In [`PASSTHROUGH_CRS`] order, NOT the map's — a `BTreeMap` iterates
    // alphabetically, which is an order no Lightroom file uses: legal XML,
    // unreadable diffs. The declared order is Adobe's own grouping.
    //
    // A key absent from the map was absent from the document, and stays
    // absent: we do not invent a Transform block for a file that never had one.
    for key in PASSTHROUGH_CRS {
        if let Some(v) = r.passthrough.get(key) {
            attr(&mut a, key, v);
        }
    }
    // The camera profile's NAME (v1.5.0 F7), AFTER the Upright block because
    // that is the order Lightroom's own files use — `…UprightFocalLength35mm`
    // then `CameraProfile` then `CameraProfileDigest`. It is an owned key now
    // (the render obeys it through [`crate::dcp`]) but it is still written as
    // the exact string it arrived as: a profile name is an identifier, and
    // reformatting one would stop it matching the file it names. `attr`
    // XML-escapes, which is transport rather than interpretation, and is why a
    // name with an `&` in it survives.
    if !r.camera_profile.is_empty() && ungated("CameraProfile") {
        attr(&mut a, "CameraProfile", &r.camera_profile);
    }

    // Crop + straighten, as ONE rotated-corner encoding ([`engine_to_lr_crop`],
    // R27). Only applied by Lightroom when HasCrop is True — a non-zero
    // CropAngle under HasCrop="False" is ignored — so a straighten-only recipe
    // still ships HasCrop=True, and what it ships is the STRAIGHTENED frame's
    // own four corners (which are `0,0,1,1` exactly when there is no tilt, so
    // every un-straightened document this writer has ever produced is
    // unchanged to the byte).
    match engine_to_lr_crop(r.crop.as_ref(), r.straighten_deg as f64, frame) {
        Some(c) => {
            attr(&mut a, "HasCrop", "True");
            attr(&mut a, "CropTop", &format!("{:.6}", c.top));
            attr(&mut a, "CropLeft", &format!("{:.6}", c.left));
            attr(&mut a, "CropBottom", &format!("{:.6}", c.bottom));
            attr(&mut a, "CropRight", &format!("{:.6}", c.right));
            // SIX decimals, which is Lightroom's own precision for this key
            // (`-3.274380`) and the four beside it. The `{:.1}` this replaces
            // (R27, `P3-cropangle-model.md` §6.4) turned that specimen into
            // `-3.3` on every re-save: 0.0256° of drift = 4.3 px of
            // edge-to-edge tilt across a 9504 px frame, in a carrier whose
            // whole purpose is a lossless round trip.
            if c.angle_deg != 0.0 {
                attr(&mut a, "CropAngle", &format!("{:.6}", c.angle_deg));
            }
        }
        None => attr(&mut a, "HasCrop", "False"),
    }

    attr(
        &mut a,
        "ToneCurveName2012",
        if r.tone_curve.is_empty() { "Linear" } else { "Custom" },
    );
    // Last, so the fresh-document skeleton stays byte-identical to the
    // pre-merge writer (which hardcoded this right after {attrs}).
    attr(&mut a, "HasSettings", "True");
    a
}

/// Every child ELEMENT the writer owns (tone curves + mask corrections),
/// shared by the fresh-document writer and the merge path — PLUS the per-mask
/// loss verdicts that fall out of emitting them.
///
/// The mask pass runs even when `include_masks` is false (the merge is
/// preserving the base's own block): the caller has to disclose the recipe's
/// losses either way, and running it here is what stops a save from building the
/// mask XML twice (R22 NIT-1).
pub(super) fn owned_children(
    r: &EditRecipe,
    include_masks: bool,
    frame: Option<FrameAspect>,
) -> (String, Vec<MaskLoss>) {
    let (masks, losses) = masks_xml(r, frame);

    // Tone curves are child elements (rdf:Seq of "x, y" strings), not attributes.
    // One builder for the master + the three per-channel curves (verified key
    // names against the user's sidecar: ToneCurvePV2012Red/Green/Blue).
    let curve_elem = |tag: &str, points: &[crate::recipe::CurvePoint]| -> String {
        if points.is_empty() {
            return String::new();
        }
        let pts: String = points
            .iter()
            .map(|p| format!("     <rdf:li>{}, {}</rdf:li>\n", p.input, p.output))
            .collect();
        format!("\n   <crs:{tag}>\n    <rdf:Seq>\n{pts}    </rdf:Seq>\n   </crs:{tag}>")
    };
    // Camera Raw IGNORES the three channel curves of a sidecar that has no
    // `crs:ToneCurvePV2012` element (measured v1.17.0: a fitted recipe's
    // R/G/B curves rendered byte-identical to none at all, and the same
    // sidecar with an identity composite block moved 11.9 ΔE — the move
    // the engine renders). Lightroom always writes the composite, so a
    // recipe with channel curves and no composite one writes it as the
    // identity, which reads back empty (`parse_curve_checked`).
    let identity = [
        crate::recipe::CurvePoint { input: 0, output: 0 },
        crate::recipe::CurvePoint { input: 255, output: 255 },
    ];
    let has_channel_curve = [&r.red_curve, &r.green_curve, &r.blue_curve].iter().any(|c| !c.is_empty());
    let composite: &[crate::recipe::CurvePoint] =
        if r.tone_curve.is_empty() && has_channel_curve { &identity } else { &r.tone_curve };
    let children = format!(
        "{}{}{}{}{}{}",
        curve_elem("ToneCurvePV2012", composite),
        curve_elem("ToneCurvePV2012Red", &r.red_curve),
        curve_elem("ToneCurvePV2012Green", &r.green_curve),
        curve_elem("ToneCurvePV2012Blue", &r.blue_curve),
        point_colors_elem(&r.point_colors),
        if include_masks { masks.as_str() } else { "" },
    );
    (children, losses)
}

/// The point colours (v1.5.0) as Lightroom writes them — between the tone
/// curves and the mask block, one `rdf:li` of nineteen comma-separated
/// six-decimal numbers per swatch (`PointColor::to_numbers`) — or nothing for
/// a recipe with none. Lightroom's no-swatch placeholder (one item of nineteen
/// `-1.000000`s) is never written: it says what absence says, and the merge
/// leaves a base's placeholder where it stands.
fn point_colors_elem(swatches: &[crate::recipe::PointColor]) -> String {
    if swatches.is_empty() {
        return String::new();
    }
    let items: String = swatches
        .iter()
        .map(|p| {
            // `+ 0.0` folds a negative zero, which prints as `-0.000000`.
            let numbers: Vec<String> = p.to_numbers().iter().map(|v| format!("{:.6}", v + 0.0)).collect();
            format!("     <rdf:li>{}</rdf:li>\n", numbers.join(", "))
        })
        .collect();
    format!("\n   <crs:PointColors>\n    <rdf:Seq>\n{items}    </rdf:Seq>\n   </crs:PointColors>")
}

/// The creative profile (`<crs:Look>`, v1.17.0) as Lightroom writes it: the
/// name and amount on the Look's own Description, the baked half inside
/// `crs:Parameters` — the camera profile it sits on, its colour table's id,
/// the grayscale switch, any baked Clarity/Highlights/Shadows and its four
/// curves. Nothing for a recipe with no Look.
///
/// Written by the FRESH document and by a merge whose base carries no Look of
/// its own; a base that has one keeps it verbatim (`merge_recipe_into_xmp`),
/// which is stricter than anything re-serialised here. Without it a recipe
/// that develops through 「Adobe Color」 reached Lightroom as bare 「Adobe
/// Standard」 whenever no Lightroom document stood beside the photo.
pub(super) fn look_elem(r: &EditRecipe) -> String {
    let Some(l) = r.look.as_ref() else { return String::new() };
    let curve = |tag: &str, points: &[crate::recipe::CurvePoint]| -> String {
        if points.is_empty() {
            return String::new();
        }
        let pts: String = points
            .iter()
            .map(|p| format!("       <rdf:li>{}, {}</rdf:li>\n", p.input, p.output))
            .collect();
        format!("\n     <crs:{tag}>\n      <rdf:Seq>\n{pts}      </rdf:Seq>\n     </crs:{tag}>")
    };
    let mut params =
        format!("\n      crs:ConvertToGrayscale=\"{}\"", if l.grayscale { "True" } else { "False" });
    for (key, value) in [("CameraProfile", &l.base_profile), ("LookTable", &l.table)] {
        if !value.is_empty() {
            params.push_str(&format!("\n      crs:{key}=\"{}\"", xml_attr_escape(value)));
        }
    }
    for (key, value) in
        [("Clarity2012", l.clarity), ("Highlights2012", l.highlights), ("Shadows2012", l.shadows)]
    {
        if value != 0.0 {
            params.push_str(&format!("\n      crs:{key}=\"{}\"", signed(value)));
        }
    }
    let curves = [
        curve("ToneCurvePV2012", &l.tone_curve),
        curve("ToneCurvePV2012Red", &l.red_curve),
        curve("ToneCurvePV2012Green", &l.green_curve),
        curve("ToneCurvePV2012Blue", &l.blue_curve),
    ]
    .concat();
    format!(
        "\n   <crs:Look>\n    <rdf:Description\n     crs:Name=\"{}\"\n     crs:Amount=\"{}\">\n    \
         <crs:Parameters>\n     <rdf:Description{params}>{curves}\n     </rdf:Description>\n    \
         </crs:Parameters>\n    </rdf:Description>\n   </crs:Look>",
        xml_attr_escape(&l.name),
        l.amount,
    )
}

/// The rationale, made safe for an XML comment. XML comments forbid "--"
/// anywhere inside and "-" as the final char — an AI rationale containing
/// "--" made the WHOLE sidecar unparsable. Swap ASCII hyphens in those
/// positions for U+2011 (display-only text; xml_escape has already run, so
/// no raw markup survives either).
pub(super) fn safe_rationale(r: &EditRecipe) -> String {
    let s = xml_text_escape(&r.rationale).replace("--", "‑‑");


    s.strip_suffix('-').map(|p| format!("{p}‑")).unwrap_or(s)
}

/// Render `recipe` as a complete, FRESH `.xmp` sidecar document. When a
/// previous document exists, prefer [`merge_recipe_into_xmp`] —
/// regeneration discards everything AutoShade does not model (A11).
///
/// A WRITER that also has to disclose what the projection cost should take
/// [`recipe_to_xmp_with_losses`] instead: the verdicts fall out of the same pass
/// that emits the XML, so asking for them separately builds the mask block a
/// second time.
pub fn recipe_to_xmp(r: &EditRecipe) -> String {
    recipe_to_xmp_with_losses(r).0
}

/// [`recipe_to_xmp_with_losses`] told what frame the photo is — the aspect the
/// radial projection needs to write `crs:Angle` (see [`FrameAspect`]). A fresh
/// document declares no `tiff:ImageWidth/ImageLength` of its own, so without
/// this a rotated radial can only be written as its unrotated ellipse and
/// disclosed; with it the tilt goes out.
pub fn recipe_to_xmp_in_frame(
    r: &EditRecipe,
    frame: Option<FrameAspect>,
) -> (String, Vec<MaskLoss>) {
    recipe_to_xmp_in_frame_for_photo(r, frame, None)
}

/// [`recipe_to_xmp_in_frame`] told which PHOTOGRAPH the develop belongs to,
/// so the payload can anchor a relative raster path to that photo's develop
/// dir when it embeds the raster (`payload::rasters_element`). Without it an
/// absolute path still embeds; a bare name cannot, and is disclosed.
pub fn recipe_to_xmp_in_frame_for_photo(
    r: &EditRecipe,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
) -> (String, Vec<MaskLoss>) {
    let (desc, losses) = crs_description(r, frame, photo, true);
    (xmp_document(r, &desc), losses)
}

/// The document the payload reader measures a payload AGAINST: the recipe
/// projected through this writer with NO payload of its own — what a
/// Lightroom that touched nothing would hand back, once `payload::restore`
/// has also dropped the `ash:` intent the way Lightroom does. It carries no
/// payload precisely so that reading it cannot recurse into one. Crate-wide
/// for the tests outside this module that assert the PROJECTION rather than
/// the whole document.
pub(crate) fn bare_document(r: &EditRecipe, frame: Option<FrameAspect>) -> String {
    xmp_document(r, &crs_description(r, frame, None, false).0)
}

/// [`recipe_to_xmp`] and the writer's own per-mask loss verdicts, from ONE pass
/// over the masks. `write_xmp_doc` used to build the document and then call
/// [`mask_export_losses`], i.e. run `masks_xml` twice per save for a `Vec` the
/// first pass had already produced and thrown away (R22 NIT-1).
pub fn recipe_to_xmp_with_losses(r: &EditRecipe) -> (String, Vec<MaskLoss>) {
    recipe_to_xmp_in_frame(r, None)
}

/// The document skeleton around one `rdf:Description`.
fn xmp_document(r: &EditRecipe, desc: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"AutoShade 2\">\n\
 <!-- Generated by AutoShade. AI rationale: {rationale} (confidence {conf:.2}) -->\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  {desc}\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n",
        rationale = safe_rationale(r),
        conf = r.confidence,
    )
}

/// The `rdf:Description` carrying everything this writer owns — the ONE
/// definition, so a fresh document and a spliced-in one cannot drift. Carries
/// the mask-loss verdicts out with it (see [`owned_children`]).
///
/// A FRESH document declares its own frame (R27 A8) and therefore writes its
/// geometry in that frame — see [`frame_declaration`] and [`in_source_frame`].
///
/// `embed` says whether the AutoShade payload rides along (`payload`): every
/// document that leaves this app carries it; the ONE exception is the bare
/// projection the payload reader compares a document against
/// ([`bare_document`]). `photo` anchors the payload's relative raster paths.
fn crs_description(
    r: &EditRecipe,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
    embed: bool,
) -> (String, Vec<MaskLoss>) {
    // The payload's subject is the recipe AS THE APP HOLDS IT — display frame,
    // the very value `recipe.json` gets — not the source-frame projection
    // below, which exists for Lightroom's coordinates alone.
    let app = r;
    let r = in_source_frame(r, frame);
    let r = r.as_ref();
    let (children, mut losses) = owned_children(r, true, frame);
    let (payload_attrs, payload_children) = if embed {
        let (rasters, mut raster_losses) =
            payload::rasters_element(app, payload::PAYLOAD_PREFIX, photo);
        losses.append(&mut raster_losses);
        (payload::root_attrs(app, payload::PAYLOAD_PREFIX), rasters)
    } else {
        (String::new(), String::new())
    };
    let desc = format!(
        "<rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"{tiff}{attrs}{payload_attrs}>\
{children}{look}{payload_children}\n\
  </rdf:Description>",
        tiff = frame_declaration(frame),
        attrs = owned_attrs(r, frame),
        look = look_elem(r),
    );
    (desc, losses)
}

/// The `tiff:` block that says which frame the `crs:` coordinates in this
/// document are measured against — `ImageWidth` / `ImageLength` (the SOURCE
/// frame, un-rotated, i.e. the RAW's `DefaultCropSize`) and `Orientation` (the
/// turn that carries it to the frame the photographer sees, EXIF composed with
/// their own quarter turns).
///
/// Empty when the frame is unknown, which is also when nothing above needed it.
///
/// **Why a writer declares this at all** (R27, closing `C-rotation-skeleton.md`'s
/// round-trip hole). Until now AutoShade's own fresh sidecars declared no frame,
/// so re-importing one could not decode its own rotated radial — the reader
/// needs `W/H` to fold a pixel-frame tilt into the engine's normalised one, and
/// a document with no declaration hands it nothing. Every real Lightroom
/// sidecar carries these (`P19.xmp`: `tiff:ImageWidth="9504"
/// tiff:ImageLength="6336"`); ours now do too, and a portrait sidecar's
/// `tiff:Orientation` is what tells the reader the numbers are sensor-frame.
pub(super) fn frame_declaration(frame: Option<FrameAspect>) -> String {
    let Some(f) = frame else { return String::new() };
    format!(
        "\n    xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\
         \n    tiff:ImageWidth=\"{w}\"\n    tiff:ImageLength=\"{h}\"\n    tiff:Orientation=\"{o}\"",
        w = f.w.round() as i64,
        h = f.h.round() as i64,
        o = f.turn().to_u16().max(1),
    )
}

/// The recipe as the frame it will be WRITTEN in sees it: the inverse of the
/// turn [`xmp_to_recipe`] applies on the way in.
///
/// Borrowed and untouched for every frame whose source and display coincide —
/// every landscape capture, every baked image, and every document that
/// declares no frame at all.
pub(super) fn in_source_frame<'a>(
    r: &'a EditRecipe,
    frame: Option<FrameAspect>,
) -> std::borrow::Cow<'a, EditRecipe> {
    use rawler::Orientation as O;
    let Some(f) = frame else { return std::borrow::Cow::Borrowed(r) };
    let turn = f.turn();
    if matches!(turn, O::Normal | O::Unknown) {
        return std::borrow::Cow::Borrowed(r);
    }
    // The dihedral group of the square: the two quarter turns are each other's
    // inverse and everything else is an involution.
    let back = match turn {
        O::Rotate90 => O::Rotate270,
        O::Rotate270 => O::Rotate90,
        other => other,
    };
    // The recipe's coordinates are in the DISPLAY frame here — this is the
    // inverse projection — so the aspect a brush rewrite rescales out of is the
    // displayed rectangle, `FrameAspect`'s own `displayed()` (R29 C1,
    // `render::CoordFrame`). Passing the SOURCE aspect would rescale every
    // radius by `H/W` where `W/H` was owed, i.e. by the square of the error.
    let d = f.displayed();
    let mut owned = r.clone();
    crate::render::orient_recipe_coords(
        &mut owned,
        back,
        crate::render::CoordFrame::new(d.w, d.h),
    );
    std::borrow::Cow::Owned(owned)
}

/// First occurrence of `needle` that is real MARKUP — not text quoted inside a
/// comment, a CDATA section or a processing instruction.
///
/// `xmp.rs` already owns this distinction for the close scanner
/// (`a_pathological_sidecar_neither_hangs_nor_believes_a_comment`), and a
/// plain `str::find` here regressed it: a sidecar whose header quotes
/// `<rdf:RDF …>` in a comment got the settings block spliced INSIDE that
/// comment. The merge then "succeeded", so no loss note fired, and the file
/// Lightroom reads carried none of the user's develop — exactly the silence
/// the disclosure exists to end.
///
/// Forward-only and single-sweep, like [`Landmarks`]: `at` never moves
/// backwards, so the scan is linear in the document.
pub(super) fn find_outside_constructs(doc: &str, needle: &str) -> Option<usize> {
    let mut at = 0;
    loop {
        let hit = at + doc[at..].find(needle)?;
        // The innermost construct that OPENS at or before the hit and has not
        // closed by then swallows it; skip past that construct and resume.
        let swallowing = CONSTRUCTS.iter().filter_map(|(open, close)| {
            let o = doc[at..=hit].rfind(open)? + at;
            // A construct that already closed before the hit does not swallow.
            let end = doc[o + open.len()..].find(close).map(|e| o + open.len() + e);
            match end {
                Some(e) if e > hit => Some(e + close.len()),
                // Unterminated: everything after it is text, so there is no
                // real markup left to find.
                None => Some(doc.len()),
                Some(_) => None,
            }
        });
        match swallowing.max() {
            Some(resume) => at = resume.min(doc.len()),
            None => return Some(hit),
        }
        if at >= doc.len() {
            return None;
        }
    }
}

/// Add our settings to a sidecar that has none, keeping every byte of it.
///
/// The base is a real XMP document — it just carries no camera-raw settings
/// (a ratings/keywords sidecar from exiftool, Bridge or Capture One). Splicing
/// a fresh `rdf:Description` in after the `rdf:RDF` open tag preserves the
/// user's properties AND records ours, so the save is a genuine merge and the
/// caller has no loss to disclose. Returning `None` (no `rdf:RDF`, or a
/// self-closing one) keeps the old regenerate-and-say-so behaviour — the
/// document is then not one we can account for.
pub(super) fn insert_crs_description(
    existing: &str,
    r: &EditRecipe,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
) -> Option<(String, Vec<MaskLoss>)> {
    let at = find_outside_constructs(existing, "<rdf:RDF")?;
    let (gt, self_closing) = scan_tag_end(existing, at)?;
    if self_closing {
        return None;
    }
    let (desc, losses) = crs_description(r, frame, photo, true);
    let mut out = String::with_capacity(existing.len() + 512);
    out.push_str(&existing[..=gt]);
    out.push_str("\n  ");
    out.push_str(&desc);
    out.push_str(&existing[gt + 1..]);
    Some((out, losses))
}

/// The crs attribute keys this writer OWNS — the removal universe for the
/// merge. Must cover every key `owned_attrs` can EVER emit, including the
/// conditional ones (a cleared vignette must disappear from a merged
/// document, not linger at its old value).
///
/// `pub(crate)` since R23-1: the control registry's tests assert that every
/// attribute the AI/eval ruler reads is one this writer can write, so a
/// misspelled key in the ruler cannot silently measure nothing.
pub(crate) fn owned_attr_keys() -> Vec<String> {
    let mut keys: Vec<String> = [
        "Version",
        "ProcessVersion",
        "WhiteBalance",
        "Temperature",
        "Tint",
        "Exposure2012",
        "Contrast2012",
        "Highlights2012",
        "Shadows2012",
        "Whites2012",
        "Blacks2012",
        "Clarity2012",
        "Dehaze",
        "Vibrance",
        "Saturation",
        "Texture",
        // HDR edit mode and its SDR rendition (v1.5.0 F8) — owned, all nine,
        // so a mode the photographer left really does leave the sidecar.
        "HDREditMode",
        "HDRMaxValue",
        "SDRBlend",
        "SDRBrightness",
        "SDRContrast",
        "SDRHighlights",
        "SDRShadows",
        "SDRWhites",
        "SDRClarity",
        // The parametric tone curve (v1.5.0) — owned, so the merge strips
        // Lightroom's copy before writing ours and `unmodelled_global_crs`
        // stops naming a curve this engine renders.
        "ParametricShadows",
        "ParametricDarks",
        "ParametricLights",
        "ParametricHighlights",
        "ParametricShadowSplit",
        "ParametricMidtoneSplit",
        "ParametricHighlightSplit",
        "SplitToningShadowHue",
        "SplitToningShadowSaturation",
        "SplitToningHighlightHue",
        "SplitToningHighlightSaturation",
        "SplitToningBalance",
        "ColorGradeShadowLum",
        "ColorGradeMidtoneHue",
        "ColorGradeMidtoneSat",
        "ColorGradeMidtoneLum",
        "ColorGradeHighlightLum",
        "ColorGradeGlobalHue",
        "ColorGradeGlobalSat",
        "ColorGradeGlobalLum",
        "ColorGradeBlending",
        "Sharpness",
        "LuminanceSmoothing",
        // The R25 B3 carried detail axes + the manual CA pair + the auto-CA
        // switch + the six de-fringe keys. Same reason as the B2 block below:
        // owning a key is what makes the merge STRIP it before rewriting, and
        // it is also what takes the key OUT of `unmodelled_global_crs` (whose
        // universe is the complement of this list).
        "SharpenRadius",
        "SharpenDetail",
        "SharpenEdgeMasking",
        "LuminanceNoiseReductionDetail",
        "LuminanceNoiseReductionContrast",
        "ColorNoiseReduction",
        "ColorNoiseReductionDetail",
        "ColorNoiseReductionSmoothness",
        "VignetteAmount",
        "VignetteMidpoint",
        "LensManualDistortionAmount",
        "LensProfileEnable",
        "LensProfileSetup",
        "LensProfileName",
        "LensProfileFilename",
        "LensProfileDistortionScale",
        "LensProfileVignettingScale",
        // The Transform panel (v1.5.0 F6). Owning them is what moves the eight
        // Perspective keys out of `PASSTHROUGH_CRS` and stops
        // `unmodelled_global_crs` naming a block this engine now renders.
        // `UprightTransform_*` stays UNOWNED on purpose — see the writer.
        "PerspectiveVertical",
        "PerspectiveHorizontal",
        "PerspectiveRotate",
        "PerspectiveScale",
        "PerspectiveAspect",
        "PerspectiveX",
        "PerspectiveY",
        "PerspectiveUpright",
        "CropConstrainToWarp",
        "ChromaticAberrationR",
        "ChromaticAberrationB",
        "AutoLateralCA",
        "DefringePurpleAmount",
        "DefringePurpleHueLo",
        "DefringePurpleHueHi",
        "DefringeGreenAmount",
        "DefringeGreenHueLo",
        "DefringeGreenHueHi",
        // The R25 B2 carried effects. Owning a key is what makes the merge
        // STRIP it before rewriting — without these nine the writer's own
        // values would land beside Lightroom's originals as duplicate
        // attributes, and `unmodelled_global_crs` would go on naming keys we
        // now model.
        "PostCropVignetteAmount",
        "PostCropVignetteMidpoint",
        "PostCropVignetteFeather",
        "PostCropVignetteRoundness",
        "PostCropVignetteStyle",
        "PostCropVignetteHighlightContrast",
        "GrainAmount",
        "GrainSize",
        "GrainFrequency",
        "HasCrop",
        "CropTop",
        "CropLeft",
        "CropBottom",
        "CropRight",
        "CropAngle",
        "ToneCurveName2012",
        "HasSettings",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // The R25 B4 PASS-THROUGH block. Owning it is what makes the merge
    // strip each key before the writer puts it back — without that, our
    // verbatim copy would land beside Lightroom's original as a duplicate
    // attribute. It is also what takes the nine out of
    // `unmodelled_global_crs`, whose universe is this list's complement.
    keys.extend(PASSTHROUGH_CRS.iter().map(|s| (*s).to_string()));
    // v1.5.0 F7: the camera profile's name is OWNED now, so the merge has to
    // strip Lightroom's copy before the writer puts ours back — exactly the
    // duplicate-attribute argument the pass-through block is built on.
    keys.push("CameraProfile".to_string());
    // The v1.5.0 colour keys: the Calibration panel, the B&W switch and — in
    // the band loop below, beside the HSL cells — its eight-band mixer.
    keys.extend(CALIBRATION_CRS.iter().map(|s| (*s).to_string()));
    keys.push("ConvertToGrayscale".to_string());
    for band in crate::recipe::HSL_BANDS {
        keys.push(format!("HueAdjustment{band}"));
        keys.push(format!("SaturationAdjustment{band}"));
        keys.push(format!("LuminanceAdjustment{band}"));
        keys.push(format!("GrayMixer{band}"));
    }
    keys
}
