//! XMP sidecar writer — render an [`EditRecipe`] as an Adobe Camera Raw /
//! Lightroom `.xmp` sidecar (the `crs:` namespace), so the AI's edit opens as
//! adjustable develop sliders in the user's catalog.
//!
//! Key names, value conventions, and structure were verified against a real ACR
//! sidecar from the user's own library (`DSC08724.xmp`): `ProcessVersion=15.4`,
//! signed-integer sliders, `Sharpness` on 0..100, tone curve as an `rdf:Seq` of
//! `"x, y"` strings (see `docs/M1_PLAN.md` §5 and §9). We emit only the keys we
//! set; Lightroom fills the rest from defaults.

use crate::recipe::{ColorGrade, Crop, CurvePoint, EditRecipe, Hsl, LocalAdjustment, MaskGeometry, RangeMask};

/// Format an integer-valued slider the way ACR writes it: explicit `+` for
/// positives (`"+14"`, `"-12"`, `"0"`).
fn signed(v: f32) -> String {
    let i = v.round() as i64;
    if i > 0 {
        format!("+{i}")
    } else {
        i.to_string()
    }
}

fn xml_escape(s: &str) -> String {
    // Strip XML-1.0-forbidden characters FIRST (an AI rationale or a pasted
    // mask name can carry them; escaped or not, they make the whole sidecar
    // unparsable): control chars except tab/newline/CR, plus the non-character
    // code points U+FFFE/U+FFFF, which are equally illegal in XML 1.0 but are
    // NOT `char::is_control`.
    s.chars()
        .filter(|c| {
            (!c.is_control() || matches!(c, '\t' | '\n' | '\r'))
                && !matches!(c, '\u{FFFE}' | '\u{FFFF}')
        })
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn attr(buf: &mut String, key: &str, val: &str) {
    buf.push_str(&format!("\n    crs:{key}=\"{val}\""));
}

/// Format a LOCAL adjustment value the way ACR writes it: a bare decimal, no
/// forced `+` (e.g. `"-0.075"`, `"0"`). Distinct from the global `signed()`.
fn local_fmt(v: f32) -> String {
    if v == 0.0 {
        "0".to_string()
    } else {
        format!("{v}")
    }
}

/// A stable 32-uppercase-hex GUID derived from `seed` (no external uuid dep).
/// Deterministic so re-emitting the same recipe yields the same sidecar; the
/// per-mask seed includes the index so masks within a file stay unique.
fn guid(seed: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h1);
    let a = h1.finish();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    (seed, a).hash(&mut h2);
    let b = h2.finish();
    format!("{a:016X}{b:016X}")
}

/// `(crs:What value, extra geometry attributes)` for a mask geometry, or
/// `None` for geometries classic ACR XMP cannot express (raster bitmaps —
/// the writer skips those corrections; the render still applies them).
/// Coordinates are written raw (unclamped) — ACR gradients legitimately use
/// values outside [0,1].
fn mask_geom_xml(g: &MaskGeometry) -> Option<(&'static str, String)> {
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => Some((
            "Mask/Gradient",
            format!(
                " crs:ZeroX=\"{zero_x}\" crs:ZeroY=\"{zero_y}\" crs:FullX=\"{full_x}\" crs:FullY=\"{full_y}\""
            ),
        )),
        MaskGeometry::Radial { top, left, bottom, right, feather, roundness, flipped } => Some((
            "Mask/CircularGradient",
            {
                // Lightroom's crs:Feather lives on a 0..100 scale (reference
                // sidecars carry integers like 50 / 72); the engine's is 0..1.
                // The old writer emitted the raw 0..1 value, which Lightroom
                // read as a nearly hard edge — convert on the boundary.
                let lr_feather = (feather.clamp(0.0, 1.0) * 100.0).round();
                format!(
                    " crs:Top=\"{top}\" crs:Left=\"{left}\" crs:Bottom=\"{bottom}\" crs:Right=\"{right}\" \
crs:Feather=\"{lr_feather}\" crs:Roundness=\"{roundness}\" crs:Flipped=\"{flipped}\""
                )
            },
        )),
        MaskGeometry::Bitmap { .. } => None,
    }
}

/// A `Mask/RangeMask` component `<rdf:li>` intersected with the correction's
/// geometric mask (empty string when the adjustment has no range). Component
/// structure and attribute values verified against the user's own Lightroom
/// sidecars (`_DSC9245.xmp` luminance, `_DSC9303.xmp` colour): the intersect
/// encoding is `MaskBlendMode="1" + MaskInverted="true" + MaskValue="0"` —
/// i.e. "paint 0 wherever the range does NOT match", which erases everything
/// outside geometry ∩ range. Luminance uses the attribute form
/// (`crs:LumRange="lo_outer lo hi hi_outer"`); colour uses the child-element
/// form with one `crs:PointModels` entry `"r g b px py 0"` (last three numbers
/// assumed sample-point + reserved; see ROADMAP §A for the verification note).
fn range_mask_xml(range: &Option<RangeMask>, sync_id: &str) -> String {
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

/// Build the `<crs:MaskGroupBasedCorrections>` child element (empty string when
/// there are no masks). Local sliders convert UI scale → ACR local scale:
/// exposure stops ÷4, every other slider ÷100 (verified against the user's real
/// sidecar; see docs/V2_PLAN.md §2a). All 26 `Local*` fields are emitted (unused
/// = 0) as Lightroom expects the full block.
fn masks_xml(r: &EditRecipe) -> String {
    if r.masks.is_empty() {
        return String::new();
    }
    let mut items = String::new();
    for (i, m) in r.masks.iter().enumerate() {
        let name = if m.name.is_empty() { format!("Autoshop {}", i + 1) } else { m.name.clone() };
        let corr_id = guid(&format!("corr-{i}-{name}"));
        let mask_id = guid(&format!("mask-{i}-{name}"));
        // Raster (bitmap) masks have no classic-XMP encoding — skip this
        // correction; the deterministic render still applies it (§A tradeoff).
        let Some((what, geom)) = mask_geom_xml(&m.mask) else { continue };
        items.push_str(&format!(
            "     <rdf:li>\n\
      <rdf:Description\n\
       crs:What=\"Correction\" crs:CorrectionAmount=\"{amount}\" crs:CorrectionActive=\"true\"\n\
       crs:CorrectionName=\"{name}\" crs:CorrectionSyncID=\"{corr_id}\"\n\
       crs:LocalExposure=\"0\" crs:LocalHue=\"0\" crs:LocalSaturation=\"{sat}\"\n\
       crs:LocalContrast=\"0\" crs:LocalClarity=\"0\" crs:LocalSharpness=\"0\"\n\
       crs:LocalBrightness=\"0\" crs:LocalToningHue=\"0\" crs:LocalToningSaturation=\"0\"\n\
       crs:LocalExposure2012=\"{exp}\" crs:LocalContrast2012=\"{con}\"\n\
       crs:LocalHighlights2012=\"{hi}\" crs:LocalShadows2012=\"{sh}\"\n\
       crs:LocalWhites2012=\"{wh}\" crs:LocalBlacks2012=\"{bl}\"\n\
       crs:LocalClarity2012=\"{cl}\" crs:LocalDehaze=\"{dh}\" crs:LocalLuminanceNoise=\"{nr}\"\n\
       crs:LocalMoire=\"0\" crs:LocalDefringe=\"0\" crs:LocalTemperature=\"{temp}\"\n\
       crs:LocalTint=\"{tint}\" crs:LocalTexture=\"{tex}\" crs:LocalGrain=\"0\"\n\
       crs:LocalCurveRefineSaturation=\"100\">\n\
       <crs:CorrectionMasks>\n\
        <rdf:Seq>\n\
         <rdf:li crs:What=\"{what}\" crs:MaskActive=\"true\" crs:MaskName=\"{mname}\"\n\
          crs:MaskBlendMode=\"0\" crs:MaskInverted=\"{inv}\" crs:MaskSyncID=\"{mask_id}\"\n\
          crs:MaskValue=\"1\"{geom}/>\n\
{range}\
        </rdf:Seq>\n\
       </crs:CorrectionMasks>\n\
      </rdf:Description>\n\
     </rdf:li>\n",
            range = range_mask_xml(&m.range, &guid(&format!("range-{i}-{name}"))),
            amount = local_fmt(m.amount),
            name = xml_escape(&name),
            corr_id = corr_id,
            sat = local_fmt(m.saturation / 100.0),
            exp = local_fmt(m.exposure_ev / 4.0),
            con = local_fmt(m.contrast / 100.0),
            hi = local_fmt(m.highlights / 100.0),
            sh = local_fmt(m.shadows / 100.0),
            wh = local_fmt(m.whites / 100.0),
            bl = local_fmt(m.blacks / 100.0),
            cl = local_fmt(m.clarity / 100.0),
            dh = local_fmt(m.dehaze / 100.0),
            temp = local_fmt(m.temperature / 100.0),
            tint = local_fmt(m.tint / 100.0),
            tex = local_fmt(m.texture / 100.0),
            nr = local_fmt(m.noise_reduction / 100.0),
            what = what,
            mname = xml_escape(&format!("{name} mask")),
            inv = m.inverted,
            mask_id = mask_id,
            geom = geom,
        ));
    }
    // All masks may have been raster-skipped — no empty wrapper block then.
    if items.is_empty() {
        return String::new();
    }
    format!(
        "\n   <crs:MaskGroupBasedCorrections>\n    <rdf:Seq>\n{items}    </rdf:Seq>\n   </crs:MaskGroupBasedCorrections>"
    )
}

/// Every crs ATTRIBUTE the writer owns, rendered for `r` — the
/// `\n    crs:K="v"` block. One authority for what Autoshop owns in a
/// sidecar, shared by the fresh-document writer and the merge path
/// ([`merge_recipe_into_xmp`]); the REMOVAL universe lives in
/// [`owned_attr_keys`] and must cover every key this can ever emit.
fn owned_attrs(r: &EditRecipe) -> String {
    let mut a = String::new();

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

    // Per-colour HSL / Color mixer (8 ACR bands). Emit only when non-neutral so
    // a plain global recipe still produces a minimal, v1-compatible sidecar.
    if !r.hsl.is_neutral() {
        for (i, band) in crate::recipe::HSL_BANDS.iter().enumerate() {
            attr(&mut a, &format!("HueAdjustment{band}"), &signed(r.hsl.hue[i]));
            attr(&mut a, &format!("SaturationAdjustment{band}"), &signed(r.hsl.saturation[i]));
            attr(&mut a, &format!("LuminanceAdjustment{band}"), &signed(r.hsl.luminance[i]));
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

    // recipe sharpening is 0..150; crs Sharpness is 0..100 — rescale + clamp.
    let sharp = ((r.sharpening * 2.0 / 3.0).round() as i64).clamp(0, 100);
    attr(&mut a, "Sharpness", &sharp.to_string());
    let nr = (r.noise_reduction.round() as i64).clamp(0, 100);
    attr(&mut a, "LuminanceSmoothing", &nr.to_string());

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

    // Crop (normalised [0,1]); only applied by Lightroom when HasCrop is True.
    // Straighten rides the SAME crop transform in Adobe's model: a nonzero
    // CropAngle under HasCrop="False" is ignored by Lightroom, so a
    // straighten-only recipe must ship HasCrop=True with the full frame (the
    // reader below collapses that full-frame rectangle back to `None`).
    if let Some(c) = &r.crop {
        attr(&mut a, "HasCrop", "True");
        attr(&mut a, "CropTop", &format!("{:.6}", c.top));
        attr(&mut a, "CropLeft", &format!("{:.6}", c.left));
        attr(&mut a, "CropBottom", &format!("{:.6}", c.bottom));
        attr(&mut a, "CropRight", &format!("{:.6}", c.right));
    } else if r.straighten_deg != 0.0 {
        attr(&mut a, "HasCrop", "True");
        attr(&mut a, "CropTop", "0.000000");
        attr(&mut a, "CropLeft", "0.000000");
        attr(&mut a, "CropBottom", "1.000000");
        attr(&mut a, "CropRight", "1.000000");
    } else {
        attr(&mut a, "HasCrop", "False");
    }
    if r.straighten_deg != 0.0 {
        attr(&mut a, "CropAngle", &format!("{:.1}", r.straighten_deg));
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
/// shared by the fresh-document writer and the merge path.
fn owned_children(r: &EditRecipe) -> String {
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
    format!(
        "{}{}{}{}{}",
        curve_elem("ToneCurvePV2012", &r.tone_curve),
        curve_elem("ToneCurvePV2012Red", &r.red_curve),
        curve_elem("ToneCurvePV2012Green", &r.green_curve),
        curve_elem("ToneCurvePV2012Blue", &r.blue_curve),
        masks_xml(r),
    )
}

/// The rationale, made safe for an XML comment. XML comments forbid "--"
/// anywhere inside and "-" as the final char — an AI rationale containing
/// "--" made the WHOLE sidecar unparsable. Swap ASCII hyphens in those
/// positions for U+2011 (display-only text; xml_escape has already run, so
/// no raw markup survives either).
fn safe_rationale(r: &EditRecipe) -> String {
    let s = xml_escape(&r.rationale).replace("--", "‑‑");
    s.strip_suffix('-').map(|p| format!("{p}‑")).unwrap_or(s)
}

/// Render `recipe` as a complete, FRESH `.xmp` sidecar document. When a
/// previous document exists, prefer [`merge_recipe_into_xmp`] —
/// regeneration discards everything Autoshop does not model (A11).
pub fn recipe_to_xmp(r: &EditRecipe) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Autoshop 2\">\n\
 <!-- Generated by Autoshop. AI rationale: {rationale} (confidence {conf:.2}) -->\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"{attrs}>{children}\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n",
        rationale = safe_rationale(r),
        conf = r.confidence,
        attrs = owned_attrs(r),
        children = owned_children(r),
    )
}

/// The crs attribute keys this writer OWNS — the removal universe for the
/// merge. Must cover every key `owned_attrs` can EVER emit, including the
/// conditional ones (a cleared vignette must disappear from a merged
/// document, not linger at its old value).
fn owned_attr_keys() -> Vec<String> {
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
        "VignetteAmount",
        "VignetteMidpoint",
        "LensManualDistortionAmount",
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
    for band in crate::recipe::HSL_BANDS {
        keys.push(format!("HueAdjustment{band}"));
        keys.push(format!("SaturationAdjustment{band}"));
        keys.push(format!("LuminanceAdjustment{band}"));
    }
    keys
}

/// The index of the `>` ending the tag that opens at `start`, plus whether
/// the tag is self-closing. QUOTE-AWARE: attribute values may legally
/// contain `>` (Lightroom mask names do).
fn scan_tag_end(doc: &str, start: usize) -> Option<(usize, bool)> {
    let mut quote: Option<char> = None;
    let mut prev_nonws = ' ';
    for (i, c) in doc[start..].char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '>' => return Some((start + i, prev_nonws == '/')),
                _ => {}
            },
        }
        if !c.is_whitespace() {
            prev_nonws = c;
        }
    }
    None
}

/// The first `rdf:Description` opening tag that carries camera-raw settings
/// (declares `xmlns:crs` or holds a `crs:` attribute).
fn find_crs_description(doc: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = doc[from..].find("<rdf:Description") {
        let start = from + rel;
        let (end, _) = scan_tag_end(doc, start)?;
        let tag = &doc[start..=end];
        if tag.contains("xmlns:crs=") || tag.contains(" crs:") {
            return Some(start);
        }
        from = end + 1;
    }
    None
}

/// The `</rdf:Description>` closing the element whose opening tag ended just
/// before `from` — DEPTH-COUNTED, because Lightroom nests `rdf:Description`
/// elements inside mask corrections (the batch-3 lesson: naive scans shred
/// nested structures).
fn find_matching_close(doc: &str, mut from: usize) -> Option<usize> {
    let mut depth = 0usize;
    loop {
        let close_rel = doc[from..].find("</rdf:Description>")?;
        match doc[from..from + close_rel].find("<rdf:Description") {
            Some(open_rel) => {
                let abs = from + open_rel;
                let (end, self_closing) = scan_tag_end(doc, abs)?;
                if !self_closing {
                    depth += 1;
                }
                from = end + 1;
            }
            None => {
                let abs = from + close_rel;
                if depth == 0 {
                    return Some(abs);
                }
                depth -= 1;
                from = abs + "</rdf:Description>".len();
            }
        }
    }
}

/// Re-stamp the Autoshop rationale comment (older saves embedded it) so a
/// merged document never carries a STALE rationale for a new recipe.
fn refresh_rationale_comment(doc: String, r: &EditRecipe) -> String {
    const MARK: &str = "<!-- Generated by Autoshop. AI rationale: ";
    let Some(start) = doc.find(MARK) else { return doc };
    let Some(end) = doc[start..].find("-->") else { return doc };
    format!(
        "{}{MARK}{} (confidence {:.2}) {}",
        &doc[..start],
        safe_rationale(r),
        r.confidence,
        &doc[start + end..]
    )
}

/// The element name in a start or end tag: `<crs:Exposure2012 xml:lang="…">`
/// and `</crs:Exposure2012>` both give `crs:Exposure2012`.
fn tag_name(tag: &str) -> &str {
    let t = tag.trim_start_matches('<').trim_start_matches('/');
    let end = t.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(t.len());
    &t[..end]
}

/// Byte spans of the body's TOP-LEVEL owned property elements, in reverse
/// document order so the caller can splice them out without re-indexing.
///
/// DEPTH-AWARE, and matched by tag NAME. A flat `<crs:Name>` literal scan
/// reached INSIDE the creative Look this merge exists to preserve: Adobe
/// writes a profile's baked parameters as owned-LOOKING children of a nested
/// `rdf:Description` (`<crs:Look><rdf:Description><crs:Parameters>
/// <rdf:Description><crs:Exposure2012>…`), and stripping those gutted the
/// Look — verified by a probe on that exact shape. An owned property belongs
/// to THIS Description; anything deeper belongs to its container. Matching by
/// name also catches the attribute-carrying spelling
/// (`<crs:Exposure2012 xml:lang="x-default">`), which the literal missed —
/// leaving behind the very duplicate the element strip exists to prevent.
///
/// `None` = markup this scanner cannot account for (an unterminated tag, a
/// close with no open, an owned element that never closes). The merge then
/// bails and the caller regenerates the document, which is the pre-merge
/// behaviour.
fn top_level_owned_spans(
    body: &str,
    owned: &std::collections::HashSet<String>,
) -> Option<Vec<(usize, usize)>> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut depth = 0usize;
    // The owned element currently open AT TOP LEVEL: (span start, tag name).
    let mut open: Option<(usize, String)> = None;
    let mut i = 0usize;
    while let Some(rel) = body[i..].find('<') {
        let p = i + rel;
        let rest = &body[p..];
        if let Some(after) = rest.strip_prefix("<!--") {
            i = p + 4 + after.find("-->")? + 3;
            continue;
        }
        if let Some(after) = rest.strip_prefix("<?") {
            i = p + 2 + after.find("?>")? + 2;
            continue;
        }
        // CDATA is TEXT, not markup: its `<`/`>` must not be counted as tags.
        // Counting them left `depth` unbalanced, which bails the whole merge
        // into a full regenerate — and that path replaces the user's sidecar
        // with our own document, taking every foreign property with it. Legal
        // XML must never reach the bail.
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            i = p + "<![CDATA[".len() + after.find("]]>")? + 3;
            continue;
        }
        let (gt, self_closing) = scan_tag_end(body, p)?;
        let name = tag_name(&body[p..=gt]).to_string();
        if rest.starts_with("</") {
            // A close with no open: not markup this scanner can splice.
            depth = depth.checked_sub(1)?;
            if depth == 0
                && let Some((start, open_name)) = open.take()
            {
                if name != open_name {
                    return None;
                }
                spans.push((start, gt + 1));
            }
            i = gt + 1;
            continue;
        }
        if depth == 0
            && let Some(bare) = name.strip_prefix("crs:")
            && owned.contains(bare)
        {
            // The leading indentation (and the newline before it) goes with
            // the property — the same whitespace hygiene the attribute strip
            // applies, so an untouched document's formatting is preserved.
            let mut start = p;
            while start > 0 && matches!(body.as_bytes()[start - 1], b' ' | b'\t') {
                start -= 1;
            }
            if start > 0 && body.as_bytes()[start - 1] == b'\n' {
                start -= 1;
            }
            if self_closing {
                spans.push((start, gt + 1));
            } else {
                open = Some((start, name.clone()));
            }
        }
        if !self_closing {
            depth += 1;
        }
        i = gt + 1;
    }
    if open.is_some() || depth != 0 {
        return None; // unbalanced: regenerate rather than splice blind
    }
    spans.reverse();
    Some(spans)
}

/// The crs Description's OWN property scope — the text every whole-document
/// READ below must be restricted to: its opening tag plus the top-level
/// children that carry ITS settings.
///
/// The mirror of [`top_level_owned_spans`], for the reader. Adobe writes a
/// creative profile's baked parameters as owned-LOOKING children of a NESTED
/// `rdf:Description` (`<crs:Look><rdf:Description><crs:Parameters>
/// <rdf:Description><crs:Clarity2012>+50…`), and the scanners here
/// ([`crs_str`], [`parse_curve`], [`parse_masks`]) match by name anywhere in
/// the string they are given. Whenever the top-level Description OMITS a key
/// the Look nests, the flat scan therefore answered from the profile — the
/// import turned a camera profile's baked look into user slider values, and
/// the next save persisted them. The WRITER's depth-aware strip exists for
/// exactly this shape; the reader now shares the rule.
///
/// A top-level child that nests an `rdf:Description` is a CONTAINER of
/// someone else's settings and is dropped. `crs:MaskGroupBasedCorrections`
/// nests them too but IS this Description's own property (its nested
/// Descriptions are its mask items), so it is kept by name — dropping it
/// would blind [`parse_masks`].
///
/// `None` = markup this scanner cannot account for; [`crs_own_scope`] then
/// hands back the whole document, which is exactly the pre-fix behaviour.
fn crs_scope_inner(doc: &str) -> Option<String> {
    /// Owned containers that legitimately nest `rdf:Description`.
    const KEEP_NESTED: [&str; 1] = ["crs:MaskGroupBasedCorrections"];
    let start = find_crs_description(doc)?;
    let (gt, self_closing) = scan_tag_end(doc, start)?;
    let mut out = doc[start..=gt].to_string();
    if self_closing {
        return Some(out); // every setting is an attribute — no children at all
    }
    let close = find_matching_close(doc, gt + 1)?;
    let body = &doc[gt + 1..close];
    let mut depth = 0usize;
    // The top-level child currently open: (span start, tag name, nests an
    // rdf:Description).
    let mut open: Option<(usize, String, bool)> = None;
    let mut i = 0usize;
    while let Some(rel) = body[i..].find('<') {
        let p = i + rel;
        let rest = &body[p..];
        // Comments / PIs / CDATA are TEXT — their `<`/`>` are not tags (the
        // same three skips top_level_owned_spans makes, for the same reason:
        // counting them unbalances `depth` and bails the whole scope).
        if let Some(after) = rest.strip_prefix("<!--") {
            i = p + 4 + after.find("-->")? + 3;
            continue;
        }
        if let Some(after) = rest.strip_prefix("<?") {
            i = p + 2 + after.find("?>")? + 2;
            continue;
        }
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            i = p + "<![CDATA[".len() + after.find("]]>")? + 3;
            continue;
        }
        let (gt2, self_closing) = scan_tag_end(body, p)?;
        let name = tag_name(&body[p..=gt2]).to_string();
        if rest.starts_with("</") {
            depth = depth.checked_sub(1)?; // a close with no open: unaccountable
            if depth == 0
                && let Some((s, open_name, nested)) = open.take()
            {
                if name != open_name {
                    return None;
                }
                if !nested || KEEP_NESTED.contains(&open_name.as_str()) {
                    out.push('\n');
                    out.push_str(&body[s..=gt2]);
                }
            }
            i = gt2 + 1;
            continue;
        }
        if depth == 0 {
            if self_closing {
                // No children to inspect — a bare property element is ours.
                out.push('\n');
                out.push_str(&body[p..=gt2]);
            } else {
                open = Some((p, name.clone(), false));
            }
        } else if name == "rdf:Description"
            && let Some(o) = open.as_mut()
        {
            o.2 = true; // this top-level child nests a foreign settings block
        }
        if !self_closing {
            depth += 1;
        }
        i = gt2 + 1;
    }
    if open.is_some() || depth != 0 {
        return None;
    }
    Some(out)
}

/// [`crs_scope_inner`] with the whole-document fallback — what every reader
/// that is handed a complete sidecar must pass to the scanners. Borrowed on
/// the fallback path, so an unmergeable/unparseable document costs nothing.
pub(crate) fn crs_own_scope(xmp: &str) -> std::borrow::Cow<'_, str> {
    match crs_scope_inner(xmp) {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed(xmp),
    }
}

/// Graft `r`'s owned settings INTO an existing sidecar document, preserving
/// every property Autoshop does not model — Lightroom-only globals
/// (Texture), the camera profile / creative Look, Lightroom's lens-profile
/// block, foreign namespaces, the xpacket wrapper. Save XMP used to
/// REGENERATE the whole document, so copying it beside the RAW wiped all of
/// those from Lightroom's own file (A11).
///
/// `None` means "not safely mergeable" (no crs Description, or markup this
/// scanner cannot splice) — the caller falls back to a fresh document,
/// which is exactly the old behaviour.
///
/// Boundary, disclosed: MASKS are replaced wholesale as one block.
/// Lightroom extras INSIDE individual mask items (LocalHue / LocalMoire /
/// range-mask Invert flags, …) do not survive a re-save; parametric mask
/// geometry does, via the normal recipe round-trip. Owned scalar crs
/// properties are stripped in BOTH forms — attribute and property-element
/// (`<crs:Exposure2012>…</crs:Exposure2012>`, a form Lightroom really
/// writes and the reader really accepts); unowned properties survive in
/// either form. Matching is
/// by the CONVENTIONAL prefixes (`rdf:`, `crs:`) — a document binding the
/// camera-raw namespace to another prefix (no known producer; Adobe and
/// every interop tool use these) is judged unmergeable and regenerated,
/// which is exactly the pre-merge behaviour.
pub fn merge_recipe_into_xmp(existing: &str, r: &EditRecipe) -> Option<String> {
    let desc_start = find_crs_description(existing)?;
    let (gt, self_closing) = scan_tag_end(existing, desc_start)?;

    // The opening tag: strip every owned crs attribute, then append ours.
    // BOTH quote styles: single-quoted attributes are legal XML, and leaving
    // one behind would duplicate the attribute we append.
    let mut tag = existing[desc_start..=gt].to_string();
    for key in owned_attr_keys() {
        for q in ['"', '\''] {
            let pat = format!("crs:{key}={q}");
            while let Some(p) = tag.find(&pat) {
                let vstart = p + pat.len();
                let vend = vstart + tag[vstart..].find(q)?;
                let mut left = p;
                while left > 0 && tag.as_bytes()[left - 1].is_ascii_whitespace() {
                    left -= 1;
                }
                tag.replace_range(left..=vend, "");
            }
        }
    }
    let closing_len = if self_closing { 2 } else { 1 };
    let head = tag[..tag.len() - closing_len].trim_end().to_string();
    let new_tag = format!("{head}{}>", owned_attrs(r));

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
    let owned_elements: std::collections::HashSet<String> = [
        "ToneCurvePV2012",
        "ToneCurvePV2012Red",
        "ToneCurvePV2012Green",
        "ToneCurvePV2012Blue",
        "MaskGroupBasedCorrections",
    ]
    .into_iter()
    .map(str::to_string)
    .chain(owned_attr_keys())
    .collect();
    // TOP LEVEL ONLY (see `top_level_owned_spans`): the previous flat scan
    // also stripped identically-named children out of the nested Look this
    // merge exists to preserve. Reverse document order — earlier spans keep
    // their indices while later ones are spliced out.
    for (start, end) in top_level_owned_spans(&body, &owned_elements)? {
        body.replace_range(start..end, "");
    }

    let mut out = String::with_capacity(existing.len() + 256);
    out.push_str(&existing[..desc_start]);
    out.push_str(&new_tag);
    out.push_str(&owned_children(r));
    out.push_str(body.trim_end());
    out.push_str("\n  </rdf:Description>");
    out.push_str(&existing[tail_start..]);
    Some(upgrade_era_marker(refresh_rationale_comment(out, r)))
}

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

/// Autoshop provenance: an ATTRIBUTE-shaped `x:xmptk = "Autoshop"` /
/// `x:xmptk='Autoshop'` match (either quote style, optional whitespace
/// around `=`), searched ONLY inside the `<x:xmpmeta …>` start tag — where
/// the attribute actually lives. The old raw-substring test both missed
/// semantically identical XML spellings and matched the literal anywhere in
/// the document (a foreign sidecar's comment could claim our provenance) —
/// and this boolean decides whether an As-Shot tint imports as a real edit.
fn is_autoshop_sidecar(xmp: &str) -> bool {
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
            // x:xmptk="Autoshop">`) was accepted as the document tag.
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
            if v.strip_prefix('"')
                .is_some_and(|r| r.starts_with("Autoshop\"") || r.starts_with("Autoshop 2\""))
                || v.strip_prefix('\'')
                    .is_some_and(|r| r.starts_with("Autoshop'") || r.starts_with("Autoshop 2'"))
            {
                return true;
            }
        }
        rest = &rest[i + "x:xmptk".len()..];
    }
    false
}

/// Absolute-Kelvin era marker (`x:xmptk="Autoshop 2"`): documents whose
/// Temperature is ABSOLUTE (written by the anchored engine). We only ever
/// serialise the fixed form below, so an exact scan suffices; a hand-edited
/// whitespace variant merely misses the marker and falls back to the
/// old-era 5500 pin — the fail-safe direction (renders as the old engine
/// did) — never to a wrong absolute reinterpretation.
fn is_autoshop_era2(xmp: &str) -> bool {
    xmp.contains(r#"x:xmptk="Autoshop 2""#) || xmp.contains("x:xmptk='Autoshop 2'")
}

/// Upgrade an old Autoshop era marker on a MERGED document: the merge just
/// rewrote every owned WB attribute in absolute-Kelvin semantics, so leaving
/// `x:xmptk="Autoshop"` in place would make the next import pin the 5500
/// anchor onto absolute values. Foreign (Adobe) markers are untouched —
/// foreign import semantics are already absolute.
fn upgrade_era_marker(doc: String) -> String {
    // The closing quote is part of the pattern, so an already-upgraded
    // "Autoshop 2" value can never prefix-match and double-upgrade.
    doc.replacen(r#"x:xmptk="Autoshop""#, r#"x:xmptk="Autoshop 2""#, 1)
        .replacen(r#"x:xmptk='Autoshop'"#, r#"x:xmptk='Autoshop 2'"#, 1)
}

/// Raw string value of a `crs:<key>="…"` attribute (first occurrence). The
/// `crs:` anchor makes prefixed cousins unambiguous (`crs:Tint` can never match
/// inside `crs:LocalTint`). pub(crate): the style index shares the
/// WhiteBalance="Custom" provenance rule (as-shot Temperature/Tint are camera
/// values, not user edits) — see `style::read_settings`.
pub(crate) fn crs_str<'a>(xmp: &'a str, key: &str) -> Option<&'a str> {
    for quote in ['"', '\''] {
        let needle = format!("crs:{key}={quote}");
        if let Some(at) = xmp.find(&needle) {
            let rest = &xmp[at + needle.len()..];
            return Some(&rest[..rest.find(quote)?]);
        }
    }
    // PROPERTY-ELEMENT form: Lightroom serialises the very same settings as
    // `<crs:Exposure2012>+0.65</crs:Exposure2012>` in plenty of real
    // sidecars, and reading only the attribute form imported those files as
    // UNEDITED — which then let an explicit save overwrite them.
    let open = format!("<crs:{key}>");
    let close = format!("</crs:{key}>");
    let at = xmp.find(&open)? + open.len();
    let rest = &xmp[at..];
    Some(rest[..rest.find(&close)?].trim())
}

/// Owned crs settings PRESENT in a document whose value does not parse as a
/// number under [`crs_f32`]'s exact rule. Each of these imports as a SILENT
/// neutral in [`xmp_to_recipe`], and the next save then overwrites the
/// sidecar with those neutrals — so restore surfaces disclose them (GUI open
/// note, web X-Recipe-Warning, store derived-snapshot trace). String-typed
/// owned keys are exempt.
pub fn unparsable_crs_numbers(xmp: &str) -> Vec<String> {
    const STRINGY: [&str; 6] = [
        "Version",
        "ProcessVersion",
        "WhiteBalance",
        "HasCrop",
        "ToneCurveName2012",
        "HasSettings",
    ];
    // The import's OWN scope (see `crs_own_scope`): a corrupt number inside a
    // nested creative Look is not a setting this document restores, so
    // reporting it would promise a loss that never happens — and the contract
    // below is that this list is decided by what the import actually reads.
    let scope = crs_own_scope(xmp);
    owned_attr_keys()
        .into_iter()
        .filter(|k| !STRINGY.contains(&k.as_str()))
        .filter(|k| {
            // Present-but-unreadable, decided BY the import's own reader
            // (crs_f32, including its finiteness filter) so the disclosure
            // can never drift from what the import actually does: anything
            // read as None-while-present becomes a silent neutral.
            crs_str(&scope, k).is_some() && crs_f32(&scope, k).is_none()
        })
        .collect()
}

/// Numeric `crs:` attribute, tolerating ACR's explicit `+` (`"+22"`). `None`
/// if the key is absent, unparsable, or NON-FINITE: Rust's f32 parser accepts
/// "NaN"/"inf", no real sidecar writer emits them, and letting one through
/// imported a value the recipe clamp then silently neutralised WITHOUT the
/// unparsable-number disclosure ever firing. Shared with the eval harness +
/// style index (re-exported through `eval`).
pub(crate) fn crs_f32(xmp: &str, key: &str) -> Option<f32> {
    crs_str(xmp, key)?
        .trim()
        .trim_start_matches('+')
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}

/// Undo [`xml_escape`]. `&amp;` is decoded LAST so an escaped `&lt;` cannot
/// cascade into a second decode.
fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&gt;", ">").replace("&lt;", "<").replace("&amp;", "&")
}

/// The text between `open` and `close` (first occurrence of each, in order).
fn block_between<'a>(xmp: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = xmp.find(open)? + open.len();
    let rest = &xmp[start..];
    Some(&rest[..rest.find(close)?])
}

/// Parse one `<crs:ToneCurvePV2012…>` `rdf:Seq` of `"x, y"` points back into
/// curve control points. A 2-point identity (0,0 → 255,255) collapses to empty:
/// Lightroom ALWAYS writes the master curve (even "Linear"), while our writer
/// omits empty curves — collapsing keeps a re-import equal to a recipe that
/// never touched the curve.
fn parse_curve(xmp: &str, tag: &str) -> Vec<CurvePoint> {
    let Some(body) = block_between(xmp, &format!("<crs:{tag}>"), &format!("</crs:{tag}>")) else {
        return Vec::new();
    };
    let mut pts = Vec::new();
    for chunk in body.split("<rdf:li>").skip(1) {
        let Some(end) = chunk.find("</rdf:li>") else { continue };
        let mut it = chunk[..end].split(',');
        if let (Some(x), Some(y)) = (it.next(), it.next())
            && let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>())
        {
            pts.push(CurvePoint {
                input: x.clamp(0.0, 255.0).round() as u8,
                output: y.clamp(0.0, 255.0).round() as u8,
            });
        }
    }
    let identity = [CurvePoint { input: 0, output: 0 }, CurvePoint { input: 255, output: 255 }];
    if pts == identity { Vec::new() } else { pts }
}

/// Local-mask corrections back from `<crs:MaskGroupBasedCorrections>` —
/// parametric geometries only, exactly what [`masks_xml`] can emit (LR brush /
/// AI masks and our own Bitmap rasters have no classic-XMP encoding; those
/// corrections are skipped, matching the writer's own skip rule).
fn parse_masks(xmp: &str) -> Vec<LocalAdjustment> {
    let Some(block) =
        block_between(xmp, "<crs:MaskGroupBasedCorrections>", "</crs:MaskGroupBasedCorrections>")
    else {
        return Vec::new();
    };
    // Corrections are split on their `What="Correction"` attribute. This DOES
    // make attribute order significant (XML says it is not), but the obvious
    // alternative — splitting on `<rdf:li` — is WRONG here: a correction
    // CONTAINS its own `<crs:CorrectionMasks>` rdf:li list, so naive element
    // splitting shreds each correction into pieces (proved by the round-trip
    // tests below). Order-independent splitting needs real XML parsing, which
    // this module deliberately does not do; every serializer seen in the wild
    // (ACR, Lightroom, our writer) puts What first.
    let starts: Vec<usize> =
        block.match_indices("crs:What=\"Correction\"").map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(block.len());
        if let Some(m) = parse_one_correction(&block[s..end]) {
            out.push(m);
        }
    }
    out
}

/// One `crs:What="Correction"` segment → a [`LocalAdjustment`]. Slider scales
/// invert the writer's: exposure ×4 (a power-of-two rescale, exact in binary
/// FP), every other slider ×100 snapped to 4 decimals so `"0.3" → 30.0` lands
/// back on the UI grid instead of 30.000002.
fn parse_one_correction(seg: &str) -> Option<LocalAdjustment> {
    let q100 =
        |k: &str| crs_f32(seg, k).map_or(0.0, |v| (v * 100.0 * 10_000.0).round() / 10_000.0);
    // The geometry component decides the mask shape; a correction with no
    // parametric geometry is not representable here.
    let (mask, geom_at) = if let Some(p) = seg.find("crs:What=\"Mask/Gradient\"") {
        let g = &seg[p..];
        (
            MaskGeometry::Linear {
                zero_x: crs_f32(g, "ZeroX")?,
                zero_y: crs_f32(g, "ZeroY")?,
                full_x: crs_f32(g, "FullX")?,
                full_y: crs_f32(g, "FullY")?,
            },
            p,
        )
    } else if let Some(p) = seg.find("crs:What=\"Mask/CircularGradient\"") {
        let g = &seg[p..];
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
        let feather_raw = crs_f32(g, "Feather")?;
        let feather = if feather_raw > 1.0 || feather_raw == feather_raw.trunc() {
            feather_raw / 100.0
        } else {
            feather_raw
        };
        (
            MaskGeometry::Radial {
                top: crs_f32(g, "Top")?,
                left: crs_f32(g, "Left")?,
                bottom: crs_f32(g, "Bottom")?,
                right: crs_f32(g, "Right")?,
                feather,
                roundness: crs_f32(g, "Roundness")?,
                flipped: crs_str(g, "Flipped") == Some("true"),
            },
            p,
        )
    } else {
        return None;
    };
    // Optional range component. Its head repeats `MaskInverted="true"` as part
    // of the intersect ENCODING (see `range_mask_xml`), so user intent is read
    // from the geometry component only — hence the `geom_at`-anchored scan.
    let range = seg.find("crs:What=\"Mask/RangeMask\"").and_then(|p| {
        let r = &seg[p..];
        // STRICT token parse (`collect::<Option<…>>`), not filter_map: a
        // malformed token used to vanish, letting the remaining values shift
        // one field left and still pass the length check.
        if let Some(lum) = crs_str(r, "LumRange") {
            let v: Option<Vec<f32>> =
                lum.split_whitespace().map(|x| x.parse().ok()).collect();
            let v = v?;
            (v.len() == 4).then(|| RangeMask::Luminance {
                lo_outer: v[0],
                lo: v[1],
                hi: v[2],
                hi_outer: v[3],
            })
        } else if let Some(amount) = crs_f32(r, "ColorAmount") {
            // PointModels entry: "r g b px py 0" (writer + LR convention).
            let li = block_between(r, "<rdf:li>", "</rdf:li>")?;
            let v: Option<Vec<f32>> =
                li.split_whitespace().map(|x| x.parse().ok()).collect();
            let v = v?;
            (v.len() >= 5)
                .then(|| RangeMask::Color { r: v[0], g: v[1], b: v[2], amount, px: v[3], py: v[4] })
        } else {
            None
        }
    });
    Some(LocalAdjustment {
        mask,
        range,
        // Our own writer synthesises "Autoshop <n>" for unnamed masks (the
        // block above needs SOME CorrectionName) — importing that back as a
        // user-given name froze the placeholder and hid the localised
        // role/label. Round-trip it back to "unnamed".
        name: crs_str(seg, "CorrectionName")
            .map(xml_unescape)
            .filter(|n| {
                n.strip_prefix("Autoshop ").is_none_or(|rest| rest.parse::<u32>().is_err())
            })
            .unwrap_or_default(),
        amount: crs_f32(seg, "CorrectionAmount").unwrap_or(1.0),
        inverted: crs_str(&seg[geom_at..], "MaskInverted") == Some("true"),
        exposure_ev: crs_f32(seg, "LocalExposure2012").unwrap_or(0.0) * 4.0,
        contrast: q100("LocalContrast2012"),
        highlights: q100("LocalHighlights2012"),
        shadows: q100("LocalShadows2012"),
        whites: q100("LocalWhites2012"),
        blacks: q100("LocalBlacks2012"),
        clarity: q100("LocalClarity2012"),
        dehaze: q100("LocalDehaze"),
        texture: q100("LocalTexture"),
        saturation: q100("LocalSaturation"),
        temperature: q100("LocalTemperature"),
        tint: q100("LocalTint"),
        noise_reduction: q100("LocalLuminanceNoise"),
        // color_gains / role are engine-only and never reach a sidecar.
        ..Default::default()
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
///     `x:xmptk="Autoshop"`), whose Tint is always a real edit.
///
/// Callers should [`EditRecipe::clamp`] the result before use, like any other
/// untrusted recipe input.
pub fn xmp_to_recipe(xmp: &str) -> EditRecipe {
    let ours = is_autoshop_sidecar(xmp);
    // EVERY setting below is read from this Description's OWN scope, never
    // the raw document: a nested creative Look carries owned-LOOKING crs
    // properties, and the flat scanners answered from them whenever the top
    // level omitted the key (see `crs_own_scope`). The provenance reads
    // (`is_autoshop_sidecar` above, the rationale comment below) deliberately
    // stay on the whole document — they live OUTSIDE the Description.
    let scope = crs_own_scope(xmp);
    let scope = scope.as_ref();
    // Any EXPLICIT white balance is a user decision, not the camera's: ACR
    // writes Daylight / Cloudy / Shade / Tungsten / Fluorescent / Flash — each
    // with its own Temperature+Tint — and accepting only "Custom" imported all
    // of them as as-shot, dropping a WB the photographer had chosen. (Absent
    // is treated as explicit for the same reason `eval`/`style` do: a sidecar
    // carrying Temperature without the mode is still a stated value.)
    let custom_wb = crs_str(scope, "WhiteBalance") != Some("As Shot");
    let f = |k: &str| crs_f32(scope, k).unwrap_or(0.0);

    let mut hsl = Hsl::default();
    for (i, band) in crate::recipe::HSL_BANDS.iter().enumerate() {
        hsl.hue[i] = f(&format!("HueAdjustment{band}"));
        hsl.saturation[i] = f(&format!("SaturationAdjustment{band}"));
        hsl.luminance[i] = f(&format!("LuminanceAdjustment{band}"));
    }
    let color_grade = ColorGrade {
        shadow_hue: f("SplitToningShadowHue"),
        shadow_sat: f("SplitToningShadowSaturation"),
        shadow_lum: f("ColorGradeShadowLum"),
        midtone_hue: f("ColorGradeMidtoneHue"),
        midtone_sat: f("ColorGradeMidtoneSat"),
        midtone_lum: f("ColorGradeMidtoneLum"),
        highlight_hue: f("SplitToningHighlightHue"),
        highlight_sat: f("SplitToningHighlightSaturation"),
        highlight_lum: f("ColorGradeHighlightLum"),
        global_hue: f("ColorGradeGlobalHue"),
        global_sat: f("ColorGradeGlobalSat"),
        global_lum: f("ColorGradeGlobalLum"),
        blending: crs_f32(scope, "ColorGradeBlending").unwrap_or(ColorGrade::default().blending),
        balance: f("SplitToningBalance"),
    };
    // Our own comment header carries the AI provenance back (best-effort; the
    // escaped rationale cannot contain a raw "-->", so the scan is unambiguous).
    let (rationale, confidence) = block_between(xmp, "AI rationale: ", " -->")
        .and_then(|body| {
            let cut = body.rfind(" (confidence ")?;
            let conf =
                body[cut + " (confidence ".len()..].trim_end_matches(')').parse::<f32>().ok()?;
            Some((xml_unescape(&body[..cut]), conf))
        })
        .unwrap_or_default();

    let mut r = EditRecipe {
        temperature_k: custom_wb.then(|| crs_f32(scope, "Temperature")).flatten(),
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
        hsl,
        color_grade,
        // crs Sharpness is 0..100, recipe sharpening 0..150 (writer scales ×⅔).
        sharpening: f("Sharpness") * 1.5,
        noise_reduction: f("LuminanceSmoothing"),
        lens_vignette: f("VignetteAmount"),
        lens_vignette_mid: crs_f32(scope, "VignetteMidpoint").unwrap_or(50.0),
        lens_distortion: f("LensManualDistortionAmount"),
        // Adobe applies CropAngle only under HasCrop="True" (see the crop
        // comment below) — importing a stale angle from a DISABLED crop
        // activated a straighten Adobe itself does not render.
        straighten_deg: if crs_str(scope, "HasCrop") == Some("True") { f("CropAngle") } else { 0.0 },
        crop: (crs_str(scope, "HasCrop") == Some("True"))
            .then(|| {
                Some(Crop {
                    left: crs_f32(scope, "CropLeft")?,
                    top: crs_f32(scope, "CropTop")?,
                    right: crs_f32(scope, "CropRight")?,
                    bottom: crs_f32(scope, "CropBottom")?,
                })
            })
            .flatten()
            // A full-frame rectangle is the writer's straighten-only carrier
            // (Adobe activates CropAngle only under HasCrop=True) — collapse
            // it back to "no crop" so the round-trip stays lossless.
            .filter(|c| {
                !(c.left <= 0.0 && c.top <= 0.0 && c.right >= 1.0 && c.bottom >= 1.0)
            }),
        tone_curve: parse_curve(scope, "ToneCurvePV2012"),
        red_curve: parse_curve(scope, "ToneCurvePV2012Red"),
        green_curve: parse_curve(scope, "ToneCurvePV2012Green"),
        blue_curve: parse_curve(scope, "ToneCurvePV2012Blue"),
        masks: parse_masks(scope),
        rationale,
        confidence,
        ..Default::default()
    };
    // PROVENANCE RULE 3 (WB-anchor era): a sidecar WE wrote before the
    // absolute-Kelvin engine (x:xmptk="Autoshop", no era-2 marker) carries a
    // Temperature that was tuned RELATIVE to the historical 5500 K anchor.
    // Pin the engine anchor there — the honest encoding of that provenance —
    // so every stamp-if-None call site leaves it alone and the develop
    // renders exactly as it was tuned. Foreign sidecars (Lightroom's
    // Temperature is absolute) and era-2 documents stay unpinned: the caller
    // stamps the camera's real anchor. The pin deliberately leaves
    // as_shot_tint None — "anchor known, camera unknown" — which is also what
    // gates the as-shot caption off for these photos.
    if ours && !is_autoshop_era2(xmp) && r.temperature_k.is_some() {
        r.as_shot_k = Some(5500.0);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{CurvePoint, EditRecipe, LocalAdjustment};

    /// A creative profile's baked parameters are the PROFILE's, never the
    /// photographer's. Adobe nests them as owned-LOOKING crs children of a
    /// second `rdf:Description` (`<crs:Look><rdf:Description><crs:Parameters>
    /// <rdf:Description><crs:Clarity2012>…`) — the exact shape the WRITER's
    /// depth-aware strip was built for. The reader's flat scan answered from
    /// them whenever the top level omitted the key, so opening such a sidecar
    /// wrote the profile's look into the user's sliders and the next save
    /// persisted it.
    #[test]
    fn a_nested_look_is_not_a_user_edit() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:Version="15.5.1"
    crs:Exposure2012="+0.20">
   <crs:Look>
    <rdf:Description crs:Name="Adobe Landscape">
     <crs:Parameters>
      <rdf:Description>
       <crs:Clarity2012>+50</crs:Clarity2012>
       <crs:Vibrance>+35</crs:Vibrance>
       <crs:ToneCurvePV2012>
        <rdf:Seq>
         <rdf:li>0, 30</rdf:li>
         <rdf:li>255, 255</rdf:li>
        </rdf:Seq>
       </crs:ToneCurvePV2012>
      </rdf:Description>
     </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
        let r = xmp_to_recipe(doc);
        assert_eq!(r.exposure_ev, 0.20, "the Description's OWN attribute imports");
        assert_eq!(r.clarity, 0.0, "the Look's Clarity2012 is not a user edit: {}", r.clarity);
        assert_eq!(r.vibrance, 0.0, "the Look's Vibrance is not a user edit: {}", r.vibrance);
        assert!(r.tone_curve.is_empty(), "the Look's baked curve is not a user curve");
        // The disclosure follows the import: a corrupt number the import never
        // reads must not be announced as a setting that will be lost.
        let corrupt = doc.replace("<crs:Clarity2012>+50</crs:Clarity2012>", "<crs:Clarity2012>--</crs:Clarity2012>");
        assert!(
            unparsable_crs_numbers(&corrupt).is_empty(),
            "only settings the import READS may be disclosed: {:?}",
            unparsable_crs_numbers(&corrupt)
        );
        // …and the same key AT top level still imports, in both spellings.
        for own in [
            r#"crs:Exposure2012="+0.20" crs:Clarity2012="+12""#.to_string(),
            r#"crs:Exposure2012="+0.20">
   <crs:Clarity2012>+12</crs:Clarity2012"#
                .to_string(),
        ] {
            let d = doc.replace(r#"crs:Exposure2012="+0.20""#, &own);
            assert_eq!(xmp_to_recipe(&d).clarity, 12.0, "own Clarity2012 must import: {own}");
        }
    }

    /// The scope keeps what the Description really owns — masks (whose nested
    /// Descriptions are its own mask items) and plain property elements — and
    /// falls back to the whole document when the markup cannot be accounted
    /// for, which is the pre-scope behaviour.
    #[test]
    fn the_crs_scope_keeps_owned_children_and_degrades_safely() {
        let mut r = EditRecipe {
            exposure_ev: 0.5,
            tone_curve: vec![CurvePoint { input: 0, output: 12 }, CurvePoint { input: 255, output: 250 }],
            ..Default::default()
        };
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.9, full_x: 0.5, full_y: 0.1 },
            name: "sky".into(),
            exposure_ev: -0.4,
            ..Default::default()
        });
        // A full round-trip through the scope: masks and curves are OWNED and
        // must survive it.
        let doc = recipe_to_xmp(&r);
        let back = xmp_to_recipe(&doc);
        assert_eq!(back.tone_curve, r.tone_curve, "the owned tone curve survives the scope");
        assert_eq!(back.masks.len(), 1, "owned mask corrections survive the scope");
        assert_eq!(back.masks[0].name, "sky");
        // Markup the scanner cannot account for (an unclosed element) falls
        // back to the whole document rather than losing every setting.
        let broken = doc.replace("</rdf:Description>", "");
        assert!(crs_scope_inner(&broken).is_none(), "unaccountable markup yields no scope");
        assert_eq!(
            xmp_to_recipe(&broken).exposure_ev,
            0.5,
            "the fallback still reads the document"
        );
    }

    #[test]
    fn straighten_only_activates_crop_and_round_trips_to_no_crop() {
        // Lightroom applies CropAngle only under HasCrop="True" — a
        // straighten-only recipe ships the full frame as its carrier, and the
        // reader collapses that full-frame rectangle back to None.
        let r = EditRecipe { straighten_deg: 2.5, ..Default::default() };
        let x = recipe_to_xmp(&r);
        assert!(x.contains("crs:HasCrop=\"True\""), "straighten must activate the crop state");
        assert!(x.contains("crs:CropAngle=\"2.5\""), "{x}");
        let back = xmp_to_recipe(&x);
        assert_eq!(back.crop, None, "the full-frame carrier must not become a real crop");
        assert_eq!(back.straighten_deg, 2.5);
        // Control chars in a mask name must not poison the document.
        let dirty = EditRecipe {
            masks: vec![LocalAdjustment { name: "sky\u{0}\u{7}".into(), ..Default::default() }],
            ..Default::default()
        };
        let x = recipe_to_xmp(&dirty);
        assert!(!x.contains('\u{0}') && !x.contains('\u{7}'), "forbidden chars stripped");
    }

    #[test]
    fn renders_local_masks_with_correct_scale() {
        let r = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    name: "sky".into(),
                    exposure_ev: -0.4,  // ÷4 → -0.1
                    highlights: -50.0,  // ÷100 → -0.5
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                        feather: 0.5, roundness: 0.0, flipped: false,
                    },
                    name: "subject".into(),
                    shadows: 20.0,      // ÷100 → 0.2
                    inverted: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        // Write it out so well-formedness can be validated by an XML parser
        // (out/ is gitignored). Verification aid, not a behavioural assertion.
        std::fs::create_dir_all("out").ok();
        std::fs::write("out/_masks_test.xmp", &xmp).ok();
        assert!(xmp.contains("<crs:MaskGroupBasedCorrections>"));
        assert!(xmp.contains(r#"crs:What="Mask/Gradient""#));
        assert!(xmp.contains(r#"crs:What="Mask/CircularGradient""#));
        // local scale conversions
        assert!(xmp.contains(r#"crs:LocalExposure2012="-0.1""#)); // -0.4 / 4
        assert!(xmp.contains(r#"crs:LocalHighlights2012="-0.5""#)); // -50 / 100
        assert!(xmp.contains(r#"crs:LocalShadows2012="0.2""#)); // 20 / 100
        assert!(xmp.contains(r#"crs:MaskInverted="true""#));
        assert!(xmp.contains(r#"crs:ZeroX="0.5""#));
        // Feather crosses the boundary on Lightroom's 0..100 scale (engine 0.5
        // → crs 50) — the old writer's raw "0.5" read in LR as a hard edge.
        assert!(xmp.contains(r#"crs:Feather="50""#));
        // unset masks ⇒ no mask block (v1-compatible)
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("MaskGroupBasedCorrections"));
    }

    #[test]
    fn radial_feather_converts_both_ways_and_keeps_legacy_own_scale() {
        // LR-style integer feather imports onto the engine's 0..1 scale…
        let li = r#"crs:What="Mask/CircularGradient" crs:Top="0.2" crs:Left="0.2" crs:Bottom="0.8" crs:Right="0.8" crs:Feather="72" crs:Roundness="0" crs:Flipped="false""#;
        let m = parse_one_correction(li).expect("radial parses");
        let MaskGeometry::Radial { feather, .. } = m.mask else { panic!("radial") };
        assert!((feather - 0.72).abs() < 1e-6, "LR 72 → 0.72, got {feather}");
        // …while a legacy own-writer value (≤ 1.0) passes through verbatim.
        let legacy = li.replace(r#"crs:Feather="72""#, r#"crs:Feather="0.4""#);
        let m = parse_one_correction(&legacy).expect("legacy radial parses");
        let MaskGeometry::Radial { feather, .. } = m.mask else { panic!("radial") };
        assert!((feather - 0.4).abs() < 1e-6, "legacy 0.4 stays 0.4, got {feather}");
    }

    #[test]
    fn renders_manual_vignette_only_when_set() {
        let r = EditRecipe { lens_vignette: 35.0, lens_vignette_mid: 60.0, ..Default::default() };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:VignetteAmount="+35""#));
        assert!(xmp.contains(r#"crs:VignetteMidpoint="60""#));
        // A neutral recipe emits neither key (byte-compatible with the old writer).
        let neutral = recipe_to_xmp(&EditRecipe::default());
        assert!(!neutral.contains("VignetteAmount") && !neutral.contains("VignetteMidpoint"));
    }

    #[test]
    fn renders_manual_distortion_only_when_set() {
        let r = EditRecipe { lens_distortion: -24.0, ..Default::default() };
        assert!(recipe_to_xmp(&r).contains(r#"crs:LensManualDistortionAmount="-24""#));
        let pos = EditRecipe { lens_distortion: 80.0, ..Default::default() };
        assert!(recipe_to_xmp(&pos).contains(r#"crs:LensManualDistortionAmount="+80""#));
        // Zero amount emits no key at all (byte-compatible with the old writer).
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("LensManualDistortionAmount"));
    }

    /// One parametric mask + one raster mask — the fixture behind BOTH halves
    /// of the raster contract: the writer emits no raster correction, and the
    /// reader therefore returns no phantom for one.
    fn mixed_parametric_and_raster() -> EditRecipe {
        EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    exposure_ev: -1.0,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: "out/subject.png".into() },
                    exposure_ev: 0.6,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn bitmap_masks_are_skipped_by_the_xmp_writer() {
        use crate::recipe::MaskGeometry;
        let mixed = mixed_parametric_and_raster();
        let xmp = recipe_to_xmp(&mixed);
        assert!(xmp.contains("Mask/Gradient"), "the parametric mask must survive");
        assert_eq!(xmp.matches("crs:What=\"Correction\"").count(), 1, "raster correction skipped");
        assert!(!xmp.contains("subject.png"), "no raster path may leak into the sidecar");
        // All-raster: the whole corrections block disappears (no empty shell).
        let all_bitmap = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "out/sky.png".into() },
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!recipe_to_xmp(&all_bitmap).contains("MaskGroupBasedCorrections"));
    }

    #[test]
    fn renders_range_masks_as_intersected_components() {
        use crate::recipe::RangeMask;
        let r = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    range: Some(RangeMask::Luminance { lo_outer: 0.4, lo: 0.5, hi: 1.0, hi_outer: 1.0 }),
                    name: "sky".into(),
                    highlights: -40.0,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                        feather: 0.5, roundness: 0.0, flipped: false,
                    },
                    range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 0.5, px: 0.4, py: 0.7 }),
                    name: "subject".into(),
                    saturation: 20.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        // Both range components present, encoded as intersections (the decoded
        // ACR algebra: BlendMode 1 + Inverted true + Value 0 = keep only where
        // the range matches).
        assert_eq!(xmp.matches(r#"crs:What="Mask/RangeMask""#).count(), 2);
        assert_eq!(
            xmp.matches(r#"crs:MaskBlendMode="1" crs:MaskInverted="true""#).count(), 2
        );
        // Luminance: attribute form, LumRange in ACR's 4-number trapezoid.
        assert!(xmp.contains(r#"crs:Type="2""#));
        assert!(xmp.contains(r#"crs:LumRange="0.400000 0.500000 1.000000 1.000000""#));
        // Colour: child-element form with one PointModels entry.
        assert!(xmp.contains(r#"crs:Type="1""#));
        assert!(xmp.contains(r#"crs:ColorAmount="0.500000""#));
        assert!(xmp.contains("<rdf:li>0.900000 0.600000 0.200000 0.400000 0.700000 0</rdf:li>"));
        // A mask WITHOUT a range emits no RangeMask component at all.
        let plain = EditRecipe {
            masks: vec![LocalAdjustment { name: "plain".into(), ..Default::default() }],
            ..Default::default()
        };
        assert!(!recipe_to_xmp(&plain).contains("RangeMask"));
    }

    #[test]
    fn renders_expected_crs_keys() {
        let r = EditRecipe {
            exposure_ev: 0.32,
            contrast: 14.0,
            highlights: -12.0,
            temperature_k: Some(5600.0),
            tint: 3.0,
            sharpening: 45.0, // -> Sharpness 30
            tone_curve: vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 255, output: 255 },
            ],
            rationale: "warm & contrasty <test> & \"q\"".into(),
            confidence: 0.82,
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:ProcessVersion="15.4""#));
        assert!(xmp.contains(r#"crs:Exposure2012="0.32""#));
        assert!(xmp.contains(r#"crs:Contrast2012="+14""#));
        assert!(xmp.contains(r#"crs:Highlights2012="-12""#));
        assert!(xmp.contains(r#"crs:WhiteBalance="Custom""#));
        assert!(xmp.contains(r#"crs:Temperature="5600""#));
        assert!(xmp.contains(r#"crs:Sharpness="30""#)); // 45 * 2/3
        assert!(xmp.contains("<crs:ToneCurvePV2012>"));
        assert!(xmp.contains("<rdf:li>0, 0</rdf:li>"));
        // rationale is XML-escaped in the comment
        assert!(xmp.contains("&lt;test&gt;"));
    }

    #[test]
    fn tint_only_edit_on_a_stamped_photo_pins_custom_at_as_shot() {
        // Stamped photo, tint-only: Custom AT the as-shot Kelvin — Lightroom
        // then applies the Tint instead of ignoring it under "As Shot".
        let r = EditRecipe { tint: 15.0, as_shot_k: Some(4820.0), ..Default::default() };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:WhiteBalance="Custom""#), "{xmp}");
        assert!(xmp.contains(r#"crs:Temperature="4820""#), "{xmp}");
        assert!(xmp.contains(r#"crs:Tint="+15""#), "{xmp}");
        // Round trip: the import reads Custom back as an absolute target ==
        // the stamp, which the anchored engine renders as "no Kelvin shift".
        let back = xmp_to_recipe(&xmp);
        assert_eq!(back.temperature_k, Some(4820.0));
        assert_eq!(back.tint, 15.0);
        // A legacy recipe (no stamp) keeps the old honest fallback.
        let legacy = EditRecipe { tint: 15.0, ..Default::default() };
        let xmp = recipe_to_xmp(&legacy);
        assert!(xmp.contains(r#"crs:WhiteBalance="As Shot""#), "{xmp}");
        assert!(xmp.contains(r#"crs:Tint="+15""#), "{xmp}");
        // The engine-only stamp itself NEVER appears in a sidecar.
        assert!(!xmp.contains("as_shot"), "{xmp}");
    }

    #[test]
    fn legacy_autoshop_sidecar_kelvin_stays_relative_via_the_anchor_pin() {
        // A sidecar WE wrote before the absolute-Kelvin engine: its
        // Temperature was tuned against the 5500 K anchor. The import pins
        // the anchor there, so every stamp-if-None caller leaves it alone
        // and the develop renders exactly as tuned.
        let old = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Autoshop">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="Custom"
    crs:Temperature="5000"
    crs:HasSettings="True">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let r = xmp_to_recipe(old);
        assert_eq!(r.temperature_k, Some(5000.0));
        assert_eq!(r.as_shot_k, Some(5500.0), "old-era Kelvin pins the legacy anchor");
        assert_eq!(r.as_shot_tint, None, "the pin claims no camera as-shot");
        // Era-2 documents (what this build writes) stay unpinned — their
        // Temperature is absolute and the caller stamps the real camera K.
        let new =
            recipe_to_xmp(&EditRecipe { temperature_k: Some(5000.0), ..Default::default() });
        assert!(new.contains(r#"x:xmptk="Autoshop 2""#), "{new}");
        let r2 = xmp_to_recipe(&new);
        assert_eq!(r2.temperature_k, Some(5000.0));
        assert_eq!(r2.as_shot_k, None, "era-2 Kelvin is absolute — no pin");
        // Foreign (Lightroom) sidecars are never pinned either.
        let lr = old.replace("Autoshop", "Adobe XMP Core 7.0-c000");
        assert_eq!(xmp_to_recipe(&lr).as_shot_k, None, "foreign Kelvin is absolute");
        // A6 disclosure scanner: corrupt numbers are NAMED; parsable and
        // string-typed keys never flag; our own writer round-trips clean.
        let corrupt = old
            .replace(r#"crs:Temperature="5000""#, r#"crs:Temperature="fivethousand""#)
            .replace(
                r#"crs:HasSettings="True""#,
                "crs:Contrast2012=\"NaNny\"\n    crs:Exposure2012=\"+0.65\"\n    crs:HasSettings=\"True\"",
            );
        let bad = unparsable_crs_numbers(&corrupt);
        assert!(bad.contains(&"Temperature".to_string()), "{bad:?}");
        assert!(bad.contains(&"Contrast2012".to_string()), "{bad:?}");
        assert!(!bad.contains(&"Exposure2012".to_string()), "{bad:?}");
        assert!(!bad.iter().any(|k| k == "WhiteBalance" || k == "HasSettings"), "{bad:?}");
        assert_eq!(xmp_to_recipe(&corrupt).contrast, 0.0, "the silent neutral being disclosed");
        let clean = recipe_to_xmp(&EditRecipe {
            exposure_ev: 0.4,
            temperature_k: Some(5600.0),
            ..Default::default()
        });
        assert!(unparsable_crs_numbers(&clean).is_empty());
        // A MERGE into an old Autoshop document rewrites the WB attributes in
        // absolute semantics — the era marker must upgrade with them.
        let merged = merge_recipe_into_xmp(
            old,
            &EditRecipe { temperature_k: Some(6200.0), ..Default::default() },
        )
        .expect("mergeable");
        assert!(merged.contains(r#"x:xmptk="Autoshop 2""#), "{merged}");
        assert!(!merged.contains(r#"x:xmptk="Autoshop""#) || merged.contains("Autoshop 2"));
        assert_eq!(xmp_to_recipe(&merged).as_shot_k, None, "upgraded doc is not pinned");
    }

    #[test]
    fn non_finite_numbers_import_neutral_and_are_disclosed() {
        // Rust's f32 parser accepts "NaN" and "inf"; no real sidecar writer
        // emits them. They must import as neutral AND be named by the
        // disclosure scanner — the old exact-parse mirror read them as
        // "fine", so the silent neutral was never disclosed.
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="Custom"
    crs:Temperature="NaN"
    crs:Contrast2012="inf"
    crs:Exposure2012="+0.65"
    crs:HasSettings="True">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        assert_eq!(crs_f32(lr, "Temperature"), None, "NaN is not a Kelvin");
        assert_eq!(crs_f32(lr, "Contrast2012"), None, "inf is not a slider");
        let r = xmp_to_recipe(lr);
        assert_eq!(r.temperature_k, None);
        assert_eq!(r.contrast, 0.0);
        assert_eq!(r.exposure_ev, 0.65, "finite neighbours still import");
        let bad = unparsable_crs_numbers(lr);
        assert!(bad.contains(&"Temperature".to_string()), "{bad:?}");
        assert!(bad.contains(&"Contrast2012".to_string()), "{bad:?}");
        assert!(!bad.contains(&"Exposure2012".to_string()), "{bad:?}");
    }

    #[test]
    fn renders_hsl_bands_only_when_set() {
        let r = EditRecipe {
            hsl: crate::recipe::Hsl {
                hue: [0.0, 15.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], // orange +15
                saturation: [0.0, 0.0, 0.0, -40.0, 0.0, 0.0, 0.0, 0.0], // green -40
                ..Default::default()
            },
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:HueAdjustmentOrange="+15""#));
        assert!(xmp.contains(r#"crs:SaturationAdjustmentGreen="-40""#));
        assert!(xmp.contains(r#"crs:LuminanceAdjustmentRed="0""#)); // full 24-key block
        // A neutral recipe emits NO HSL keys (minimal, v1-compatible sidecar).
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("HueAdjustment"));
    }

    #[test]
    fn renders_color_grade_with_verified_split_toning_mapping() {
        let r = EditRecipe {
            color_grade: crate::recipe::ColorGrade {
                shadow_hue: 220.0, shadow_sat: 30.0,
                highlight_hue: 45.0, highlight_sat: 20.0,
                midtone_lum: -10.0, balance: 15.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        // shadow/highlight hue+sat round-trip via the legacy SplitToning* keys
        assert!(xmp.contains(r#"crs:SplitToningShadowHue="220""#));
        assert!(xmp.contains(r#"crs:SplitToningShadowSaturation="30""#));
        assert!(xmp.contains(r#"crs:SplitToningHighlightHue="45""#));
        assert!(xmp.contains(r#"crs:SplitToningBalance="+15""#));
        // lum / midtone / global / blending via ColorGrade*
        assert!(xmp.contains(r#"crs:ColorGradeMidtoneLum="-10""#));
        assert!(xmp.contains(r#"crs:ColorGradeBlending="50""#)); // ACR default
        // A neutral recipe emits NO grading keys at all.
        let neutral = recipe_to_xmp(&EditRecipe::default());
        assert!(!neutral.contains("ColorGrade") && !neutral.contains("SplitToning"));
    }

    #[test]
    fn renders_per_channel_rgb_curves() {
        let r = EditRecipe {
            red_curve: vec![CurvePoint { input: 0, output: 10 }, CurvePoint { input: 255, output: 250 }],
            blue_curve: vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 128, output: 110 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains("<crs:ToneCurvePV2012Red>"));
        assert!(xmp.contains("<rdf:li>0, 10</rdf:li>"));
        assert!(xmp.contains("<crs:ToneCurvePV2012Blue>"));
        assert!(xmp.contains("<rdf:li>128, 110</rdf:li>"));
        // The empty green channel emits no element.
        assert!(!xmp.contains("ToneCurvePV2012Green"));
        // A neutral recipe emits no per-channel curves at all.
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("ToneCurvePV2012Red"));
    }

    // ── merge (merge_recipe_into_xmp) ────────────────────────────────────────

    #[test]
    fn merge_preserves_lightroom_only_properties() {
        let lr = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 7.0-c000\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    crs:Version=\"15.5.1\"\n\
    crs:ProcessVersion='15.4'\n\
    crs:Texture=\"+20\"\n\
    crs:CameraProfile=\"Adobe Color\"\n\
    crs:LensProfileEnable=\"1\"\n\
    crs:LensProfileName=\"Sony FE 24-70 > special\"\n\
    crs:Exposure2012=\"+1.00\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:ToneCurvePV2012>\n\
    <rdf:Seq>\n\
     <rdf:li>0, 0</rdf:li>\n\
     <rdf:li>255, 255</rdf:li>\n\
    </rdf:Seq>\n\
   </crs:ToneCurvePV2012>\n\
   <crs:Look>\n\
    <rdf:Description crs:Name=\"Adobe Color\" crs:Amount=\"1\"/>\n\
   </crs:Look>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe {
            exposure_ev: 0.25,
            contrast: 12.0,
            tone_curve: vec![
                CurvePoint { input: 0, output: 10 },
                CurvePoint { input: 255, output: 250 },
            ],
            ..Default::default()
        };
        let merged = merge_recipe_into_xmp(lr, &r).expect("a plain LR sidecar is mergeable");
        // Everything Autoshop does not model survives.
        assert!(merged.contains("crs:Texture=\"+20\""), "global Texture survives");
        assert!(merged.contains("crs:CameraProfile=\"Adobe Color\""), "camera profile survives");
        assert!(
            merged.contains("crs:LensProfileName=\"Sony FE 24-70 > special\""),
            "LR lens profile survives — even with '>' inside the value"
        );
        assert!(merged.contains("<crs:Look>"), "LR-only child elements survive");
        assert!(merged.contains("xmlns:dc="), "foreign namespaces survive");
        assert!(merged.starts_with("<?xpacket"), "the xpacket wrapper survives");
        // Ours REPLACE, never duplicate — including the single-quoted form
        // (legal XML; leaving it would duplicate the attribute).
        assert_eq!(merged.matches("crs:Exposure2012=").count(), 1);
        assert_eq!(merged.matches("crs:ProcessVersion=").count(), 1);
        assert!(merged.contains("crs:ProcessVersion=\"15.4\""), "replaced in OUR form");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""));
        assert_eq!(merged.matches("<crs:ToneCurvePV2012>").count(), 1);
        assert!(merged.contains("<rdf:li>0, 10</rdf:li>"), "OUR curve, not Lightroom's");
        // The reader sees OUR values in the merged document.
        let back = xmp_to_recipe(&merged);
        assert_eq!((back.exposure_ev, back.contrast), (0.25, 12.0));
        // A second merge over the merged document stays single AND a cleared
        // curve REMOVES the block (a stale slider must not linger).
        let r2 = EditRecipe { exposure_ev: -0.5, ..Default::default() };
        let merged2 = merge_recipe_into_xmp(&merged, &r2).expect("re-mergeable");
        assert_eq!(merged2.matches("crs:Exposure2012=").count(), 1);
        assert!(merged2.contains("crs:Exposure2012=\"-0.50\""));
        assert!(merged2.contains("crs:Texture=\"+20\""), "still there after a second merge");
        assert_eq!(merged2.matches("<crs:ToneCurvePV2012>").count(), 0, "cleared curve gone");
        assert!(merged2.contains("ToneCurveName2012=\"Linear\""));
    }

    #[test]
    fn merge_strips_owned_element_form_properties() {
        // Lightroom serialises the SAME settings as property elements in
        // plenty of real sidecars (crs_str accepts that form). The merge
        // must strip the owned element too, or the document answers one
        // slider with two conflicting values — while unowned elements
        // (Texture) survive untouched.
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 7.0-c000\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:Exposure2012>+1.00</crs:Exposure2012>\n\
   <crs:Contrast2012>+22</crs:Contrast2012>\n\
   <crs:Texture>+20</crs:Texture>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        let merged = merge_recipe_into_xmp(lr, &r).expect("mergeable");
        assert!(!merged.contains("<crs:Exposure2012>"), "owned element stripped: {merged}");
        assert!(!merged.contains("<crs:Contrast2012>"), "owned element stripped");
        assert_eq!(merged.matches("crs:Exposure2012").count(), 1, "ours only: {merged}");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""));
        assert!(
            merged.contains("<crs:Texture>+20</crs:Texture>"),
            "unowned element survives: {merged}"
        );
        let back = xmp_to_recipe(&merged);
        assert_eq!(back.exposure_ev, 0.25);
        assert_eq!(back.contrast, 0.0, "the old element value must not shadow the cleared slider");
    }

    #[test]
    fn merge_strips_only_top_level_owned_elements() {
        // The strip is a property of THIS Description. Adobe writes a creative
        // profile's baked parameters as owned-LOOKING children of a nested
        // rdf:Description inside <crs:Look>, and a flat scan reached in and
        // gutted them — destroying the very Look this merge exists to
        // preserve. Name matching also catches the attribute-carrying
        // spelling, which the `<crs:Name>` literal missed (leaving exactly the
        // duplicate the element strip exists to prevent).
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:Exposure2012 xml:lang=\"x-default\">+1.00</crs:Exposure2012>\n\
   <crs:Look>\n\
    <rdf:Description crs:Name=\"Adobe Landscape\" crs:Amount=\"1\">\n\
     <crs:Parameters>\n\
      <rdf:Description crs:Version=\"15.4\">\n\
       <crs:Exposure2012>+0.35</crs:Exposure2012>\n\
       <crs:ToneCurvePV2012>\n\
        <rdf:Seq><rdf:li>0, 0</rdf:li></rdf:Seq>\n\
       </crs:ToneCurvePV2012>\n\
      </rdf:Description>\n\
     </crs:Parameters>\n\
    </rdf:Description>\n\
   </crs:Look>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        let merged = merge_recipe_into_xmp(lr, &r).expect("mergeable");
        // The Look keeps BOTH of its own baked parameters.
        assert!(
            merged.contains("<crs:Exposure2012>+0.35</crs:Exposure2012>"),
            "the Look's own parameter must survive: {merged}"
        );
        assert!(merged.contains("<rdf:li>0, 0</rdf:li>"), "the Look's own curve must survive");
        assert!(merged.contains("crs:Name=\"Adobe Landscape\""), "and the Look itself");
        // ...while OUR top-level property is stripped in the attribute-carrying
        // spelling too, leaving exactly one answer for the slider.
        assert!(!merged.contains("xml:lang"), "top-level owned element stripped: {merged}");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""), "ours is the attribute");
        assert_eq!(
            merged.matches("crs:Exposure2012").count(),
            3,
            "ours + the Look's open/close, no shadow copy: {merged}"
        );
        assert_eq!(xmp_to_recipe(&merged).exposure_ev, 0.25);
    }

    #[test]
    fn merge_survives_a_cdata_section() {
        // LEGAL XML must never fall back to a full regenerate: that path
        // replaces the user's whole sidecar with our own document and takes
        // every foreign property with it — the data loss the merge exists to
        // prevent. A CDATA section is not a tag; a scanner that counts it as
        // one leaves `depth` unbalanced and bails.
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:Exposure2012>+1.00</crs:Exposure2012>\n\
   <dc:description><![CDATA[client <proof> notes]]></dc:description>\n\
   <crs:Texture>+20</crs:Texture>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        let merged = merge_recipe_into_xmp(lr, &r).expect("a CDATA section must stay mergeable");
        assert!(
            merged.contains("<![CDATA[client <proof> notes]]>"),
            "the foreign CDATA property must survive verbatim: {merged}"
        );
        assert!(merged.contains("<crs:Texture>+20</crs:Texture>"), "unowned element survives");
        assert!(!merged.contains("<crs:Exposure2012>"), "ours is still stripped: {merged}");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""));
    }

    #[test]
    fn merge_replaces_masks_without_shredding_nested_descriptions() {
        // Lightroom nests rdf:Description elements INSIDE mask corrections —
        // the close-tag search must depth-count (the batch-3 lesson), and the
        // mask block is replaced wholesale while everything AFTER it lives.
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    crs:Texture=\"-9\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:MaskGroupBasedCorrections>\n\
    <rdf:Seq>\n\
     <rdf:li>\n\
      <rdf:Description crs:What=\"Correction\" crs:LocalHue=\"0.1\">\n\
       <crs:CorrectionMasks>\n\
        <rdf:Seq>\n\
         <rdf:li>\n\
          <rdf:Description crs:What=\"Mask/Gradient\" crs:ZeroX=\"0.5\"/>\n\
         </rdf:li>\n\
        </rdf:Seq>\n\
       </crs:CorrectionMasks>\n\
      </rdf:Description>\n\
     </rdf:li>\n\
    </rdf:Seq>\n\
   </crs:MaskGroupBasedCorrections>\n\
   <crs:Look>\n\
    <rdf:Description crs:Name=\"Adobe Landscape\"/>\n\
   </crs:Look>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.2,
                    left: 0.2,
                    bottom: 0.8,
                    right: 0.8,
                    feather: 0.5,
                    roundness: 0.0,
                    flipped: false,
                },
                exposure_ev: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let merged = merge_recipe_into_xmp(lr, &r).expect("mergeable");
        assert_eq!(
            merged.matches("<crs:MaskGroupBasedCorrections>").count(),
            1,
            "one mask block — OURS"
        );
        assert!(merged.contains("Mask/CircularGradient"), "our radial mask is in");
        // Our own correction block legitimately emits crs:LocalHue="0" (all 26
        // Local* fields, as Lightroom expects) — what must be GONE is the old
        // block's VALUE.
        assert!(!merged.contains("crs:LocalHue=\"0.1\""), "LR's old mask block fully replaced");
        assert!(!merged.contains("crs:ZeroX=\"0.5\""), "…including its nested gradient");
        assert!(
            merged.contains("crs:Name=\"Adobe Landscape\""),
            "the element AFTER the mask block survives — nesting was not shredded"
        );
        assert!(merged.contains("crs:Texture=\"-9\""), "unowned attribute survives");
        // The whole document still ends properly (splice did not eat the tail).
        assert!(merged.trim_end().ends_with("</x:xmpmeta>"));
    }

    // ── reader (xmp_to_recipe) ───────────────────────────────────────────────

    #[test]
    fn globals_round_trip_through_xmp() {
        // Values are chosen to survive the writer's documented rounding: integer
        // sliders (`signed()`), 2-decimal exposure, integer Kelvin, 1-decimal
        // straighten, %.6f crop — so the reader must land EXACTLY back.
        let r = EditRecipe {
            exposure_ev: 0.32,
            contrast: 14.0,
            highlights: -12.0,
            shadows: 25.0,
            whites: 8.0,
            blacks: -6.0,
            temperature_k: Some(5600.0),
            tint: 3.0,
            vibrance: 18.0,
            saturation: -5.0,
            clarity: 10.0,
            dehaze: 7.0,
            hsl: crate::recipe::Hsl {
                hue: [0.0, 15.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                saturation: [0.0, 0.0, 0.0, -40.0, 0.0, 0.0, 0.0, 0.0],
                ..Default::default()
            },
            color_grade: crate::recipe::ColorGrade {
                shadow_hue: 220.0,
                shadow_sat: 30.0,
                highlight_hue: 45.0,
                highlight_sat: 20.0,
                midtone_lum: -10.0,
                balance: 15.0,
                ..Default::default()
            },
            sharpening: 45.0, // → crs 30 → ×1.5 → 45 exactly
            noise_reduction: 20.0,
            lens_vignette: 35.0,
            lens_vignette_mid: 60.0,
            lens_distortion: -24.0,
            straighten_deg: 1.5,
            crop: Some(Crop { left: 0.05, top: 0.0, right: 0.95, bottom: 1.0 }),
            tone_curve: vec![
                CurvePoint { input: 0, output: 8 },
                CurvePoint { input: 255, output: 247 },
            ],
            red_curve: vec![
                CurvePoint { input: 0, output: 10 },
                CurvePoint { input: 255, output: 250 },
            ],
            rationale: "warm & contrasty <test> & \"q\"".into(),
            confidence: 0.82,
            ..Default::default()
        };
        let back = xmp_to_recipe(&recipe_to_xmp(&r));
        assert_eq!(back, r);
    }

    #[test]
    fn as_shot_tint_round_trips_only_for_our_own_sidecars() {
        // Our writer emits a non-neutral Tint even under "As Shot"; the Autoshop
        // marker tells the reader it is a real edit.
        let r = EditRecipe { tint: 3.0, ..Default::default() };
        assert_eq!(xmp_to_recipe(&recipe_to_xmp(&r)).tint, 3.0);
    }

    #[test]
    fn parametric_masks_round_trip_through_xmp() {
        let r = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    range: Some(RangeMask::Luminance { lo_outer: 0.4, lo: 0.5, hi: 1.0, hi_outer: 1.0 }),
                    name: "sky & sea".into(),
                    amount: 0.75,
                    inverted: true,
                    exposure_ev: -0.4, // ÷4 → ×4 is a power-of-two rescale: exact
                    contrast: 30.0,    // "0.3" ×100 needs the 4-decimal snap: exact
                    highlights: -50.0,
                    shadows: 60.0,
                    whites: 10.0,
                    blacks: -20.0,
                    clarity: 40.0,
                    dehaze: 5.0,
                    texture: 15.0,
                    saturation: 20.0,
                    temperature: 25.0,
                    tint: -10.0,
                    noise_reduction: 30.0,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                        feather: 0.5, roundness: 0.0, flipped: true,
                    },
                    range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 0.5, px: 0.4, py: 0.7 }),
                    name: "subject".into(),
                    shadows: 20.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let back = xmp_to_recipe(&recipe_to_xmp(&r));
        assert_eq!(back.masks, r.masks);
    }

    #[test]
    fn bitmap_masks_do_not_come_back_from_xmp() {
        // The writer skips raster corrections (no classic-XMP encoding), so the
        // reader must return only the parametric mask — never a phantom.
        let mixed = mixed_parametric_and_raster();
        let back = xmp_to_recipe(&recipe_to_xmp(&mixed));
        assert_eq!(back.masks.len(), 1);
        assert_eq!(back.masks[0].mask, mixed.masks[0].mask);
        assert_eq!(back.masks[0].exposure_ev, -1.0);
    }

    #[test]
    fn foreign_as_shot_sidecar_imports_no_wb_and_drops_identity_curves() {
        // A Lightroom-style sidecar (no Autoshop marker): "As Shot" Temperature
        // and Tint are the CAMERA's values, not edits — they must NOT import.
        // LR also always writes the master curve; the 2-point identity means
        // "no curve" and must collapse to empty.
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="As Shot"
    crs:Temperature="5150"
    crs:Tint="+10"
    crs:Exposure2012="+0.65"
    crs:Contrast2012="+22"
    crs:Sharpness="40"
    crs:HasSettings="True">
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
        let r = xmp_to_recipe(lr);
        assert_eq!(r.temperature_k, None, "as-shot Kelvin is not an edit");
        assert_eq!(r.tint, 0.0, "as-shot tint is not an edit");
        assert_eq!(r.exposure_ev, 0.65);
        assert_eq!(r.contrast, 22.0);
        assert_eq!(r.sharpening, 60.0); // crs 40 × 1.5
        assert!(r.tone_curve.is_empty(), "identity curve must collapse");
        // A Custom-WB foreign sidecar DOES import its Kelvin + tint.
        let custom = lr.replace("As Shot", "Custom");
        let rc = xmp_to_recipe(&custom);
        assert_eq!(rc.temperature_k, Some(5150.0));
        assert_eq!(rc.tint, 10.0);
    }
}
