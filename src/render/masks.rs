//! Local adjustments: the mask pass, the frame a mask is sampled in, Lightroom's stored-frame unwarp, and the engine's activity rule.

use super::*;

/// Apply each local masked adjustment: blend the masked region toward a locally
/// re-adjusted version, weighted by the gradient mask × amount. Mask coords are
/// normalised so this works at any resolution.
///
/// Per mask, in pass order: local **dehaze** → the fused local **WB**
/// (temperature/tint — the same [`wb_gains`] model as the global stage, see
/// [`local_temp_to_kelvin`]) + **tone** (exposure/contrast/highlights/shadows/
/// whites/blacks) + **saturation** + **hue** pass, then the **Color tint** on
/// its result → local **clarity** → local **texture** → local **sharpness** →
/// local **noise reduction**. The last two
/// are the Detail panel's own operators (`render/detail.rs`, v1.5.0) at the
/// mask's amount — signed Sharpness on the recipe's Radius/Detail/Masking,
/// Noise on its Luminance Detail/Contrast — because Lightroom's local sliders
/// have no shaping axes of their own; `film` is the scale those radii are
/// stated in.
///
/// Clarity/dehaze/texture are ENGINE-RENDERED since R22 (they were XMP-only
/// before, so a mask that moved only those three appeared to do nothing in-app
/// — user feedback #15a/#10B; recipes saved before R22 that carry them now
/// re-render with the local effect applied, which the user signed off on).
/// Clarity and texture are unsharp masks at two different radii
/// ([`unsharp_luma_weighted`], weighted by the mask instead of blending against
/// an RGB copy); dehaze reuses the exact global pair
/// [`dehaze_airlight`] + [`dehaze_px`].
///
/// **Pass order vs the global chain** (WB → dehaze → tone → … → clarity →
/// saturation → NR): the local WB/tone/saturation stages are ONE fused
/// single-weight blend, and splitting them apart to interleave the spatial ops
/// would change the output of every existing partial-weight mask (for `0 < w <
/// 1`, one blend of the composed transform ≠ three chained blends). So the two
/// achievable orderings are kept and the residue is documented: dehaze runs
/// BEFORE the fused pass — preserving the two properties the global order
/// exists for (the monotone pinned-white tone LUT cannot blow what dehaze
/// protected, and saturation stays downstream so the user can trim dehaze's
/// chroma restoration) at the cost of local Temp/Tint landing after local
/// dehaze rather than before it; clarity/texture run after the fused pass, so
/// local saturation precedes them (globally clarity precedes saturation).
/// **The colour stage, in order** (one pixel, inside the tone pass): local
/// WB and the zoned recolour gains (one channel LUT) → the master tone LUT →
/// the per-channel curves → saturation → hue rotation → the blend with the
/// pixel by the mask weight → the **Color tint, last**. Last because the
/// swatch names an ABSOLUTE hue (the Hue slider must not turn it, nor the
/// gains tilt it), and outside the blend because its strength is linear in
/// the mask weight ([`tint_px`]): blended toward a tinted copy, a half-covered
/// pixel would move along a chord instead of half the measured shift. Its
/// lightness window reads the pixel it is handed — the one the stages before
/// it produced.
///
/// Both residues are second-order: local Temp/Tint is a relative nudge on a
/// frame that was already globally white-balanced before the airlight was
/// estimated, and clarity/texture scale luma while saturation moves chroma.
///
/// **Memory** (full-resolution export, 61 MP → 244 MB per f32 plane): the
/// spatial passes run SEQUENTIALLY and each drops its planes before the next
/// allocates, so the resident increment is ONE pass's peak, never the sum.
/// That peak is three planes (~732 MB): a luma plane beside `blur_plane`'s two
/// chained ones for clarity, and the three `detail.rs` documents for each of
/// its passes — the spatial-pass row of `decode::PIPELINE_BYTES_PER_PIXEL`.
/// The `!= 0.0` gate on each pass means a mask that does not use an op
/// allocates nothing for it.
#[allow(clippy::too_many_arguments)] // the develop's frame, rasters, film scale and working space, as `apply_develop_with_rasters`
pub(super) fn apply_masks(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
    film: FilmScale,
    space: ExportColorSpace,
) {
    if w == 0 || h == 0 {
        return; // both passes below chunk by w; rayon asserts chunk_size != 0
    }
    // v1.17.0: on a camera-profile start a mask's Exposure, five tone
    // sliders, Dehaze, Clarity's tone and Temp/Tint follow Camera Raw's
    // measured law in the develop's own white-balance and tone stages
    // (`mask_cr`); this pass keeps the rest, and turns the Hue as Lightroom
    // does (`mask_cr::hue_px`).
    let camera_raw = SliderLaw::of(r) == SliderLaw::CameraRaw;
    let hue_space = camera_raw.then(|| (profile::UnderTone::new(space, r.camera_start.unwrap_or_default()), transfer_luts()));
    // The frame adaptation, built ONCE per frame rather than per mask or per
    // pixel (see `MaskUnwarp`). `None` whenever nothing downstream moves these
    // pixels, which is what keeps a photo with no active geometry byte-identical
    // to what this function produced before R29 Batch-3.
    let unwarp = frame.unwarp((w as f32, h as f32));
    let unwarp = unwarp.as_ref();
    // Airlight for every masked dehaze in this frame, estimated ONCE and only
    // when some mask actually asks for dehaze (the estimate is a full-frame
    // histogram — a real cost at 61 MP). Estimating it here, from the frame as
    // the global develop left it, is what makes it independent of MASK STACKING
    // ORDER: reordering or toggling masks cannot re-estimate the haze, exactly
    // as dragging Exposure cannot re-estimate the global one. A per-mask
    // estimate would also make two masks disagree about the same sky.
    let dehaze_a = (!camera_raw
        && r.masks.iter().any(|m| m.enabled && m.amount.clamp(0.0, 1.0) != 0.0 && m.dehaze != 0.0))
        .then(|| dehaze_airlight(data, w));
    for stored_mask in &r.masks {
        // The eye toggle: a disabled mask renders nothing at any Amount —
        // the lossless mute (recipe.rs `LocalAdjustment::enabled`).
        if !stored_mask.enabled {
            continue;
        }
        // LINEAR's only geometry rewrite happens here, once for the base plus
        // once per LINEAR component: its two handles move with the frame
        // (`MaskFrame::prepare_linear_handles`), and the pixel closures below
        // see one straight gradient between them.
        let framed_mask =
            frame.prepare_linear_handles(stored_mask, (w as f32, h as f32));
        let m = framed_mask.as_ref();
        let local = if camera_raw {
            EditRecipe { tone_curve: m.main_curve.clone(), ..EditRecipe::default() }
        } else {
            EditRecipe {
            exposure_ev: m.exposure_ev,
            contrast: m.contrast,
            highlights: m.highlights,
            shadows: m.shadows,
            whites: m.whites,
            blacks: m.blacks,
            // The mask's own master point curve (R25 P6, `crs:MainCurve`)
            // rides in as this synthetic recipe's `tone_curve`: `build_tone_lut`
            // already composes that curve on top of the slider knots, so the
            // local curve costs no new pass, no new LUT and no new curve model
            // — it is the SAME builder the global master curve goes through.
            tone_curve: m.main_curve.clone(),
            ..EditRecipe::default()
            }
        };
        let lut = build_tone_lut(&local);
        // On a camera-profile start the mask's `MainCurve` follows Camera
        // Raw's measured curve rule (`tone::camera_raw_rgb_tone`, middle by
        // its encoded ratio — item 35) instead of the luma ratio below.
        let main_lut = (camera_raw && !m.main_curve.is_empty()).then(|| curve_lut(&m.main_curve));
        // The three per-channel local curves (`crs:{Red,Green,Blue}Curve`),
        // compiled ONCE per mask like `colour_luts` below and applied inside
        // the fused pixel loop right after the master curve. (The global
        // chain puts its own after Saturation since v1.17.0, Camera Raw's
        // measured place; where a mask's sit against its local Saturation is
        // not measured, so this order stays.) `None` when all three are
        // empty, so a curve-free mask pays nothing.
        let curve_space = CurveSpace::new(space);
        let rgb_curve_luts = (!m.red_curve.is_empty()
            || !m.green_curve.is_empty()
            || !m.blue_curve.is_empty())
        .then(|| {
            (
                [
                    curve_lut(&m.red_curve),
                    curve_lut(&m.green_curve),
                    curve_lut(&m.blue_curve),
                ],
                [
                    !m.red_curve.is_empty(),
                    !m.green_curve.is_empty(),
                    !m.blue_curve.is_empty(),
                ],
            )
        });
        let sat = m.saturation / 100.0;
        // Local colour transform, computed ONCE per mask (never inside the
        // pixel loop): compose Temp/Tint WB with the zoned recolour gains,
        // then compile the exact linear-light formula into channel LUTs.
        // None when neutral, so tone-only masks pay no colour-stage cost.
        let local_wb = !camera_raw && (m.temperature != 0.0 || m.tint != 0.0);
        let colour_luts =
            (local_wb || m.color_gains.is_some()).then(|| {
                let g = if local_wb { wb_gains(5500.0, local_temp_to_kelvin(m.temperature), m.tint) } else { [1.0; 3] };
                let cg = m.color_gains.unwrap_or([1.0; 3]);
                colour_gain_luts([g[0] * cg[0], g[1] * cg[1], g[2] * cg[2]])
            });
        // The mask Color tint, compiled ONCE per mask like the LUTs above:
        // the swatch's Oklab push and its amount at full mask weight.
        // `None` at amount 0, so an untinted mask pays nothing and renders to
        // the bit what it rendered before the tint existed.
        let toning = (m.toning_amount > 0.0)
            .then(|| (tint_push(m.toning_hue), m.toning_amount.min(100.0) / 100.0));
        let amount = m.amount.clamp(0.0, 1.0);
        // Amount 0 zeroes every weight below (inverted or not) — skip the
        // full-frame tone scan and a possible NR blur that would all be
        // multiplied away (a real cost at 61 MP for a merely parked mask).
        if amount == 0.0 {
            continue;
        }
        // The coverage every pass below weighs by: the snapshot's rasters
        // and the brush stamps, loaded once per mask per develop ([`Cover`],
        // which also owns the inert skip for an unloadable raster).
        let brushes = Brushes::of(m, w, h);
        let Some(cover) = Cover::new(m, &brushes, rasters, unwarp, w, h) else {
            continue;
        };
        let weight_at = |x: usize, y: usize| cover.at(x, y);

        // --- local dehaze pass (runs FIRST — see the pass-order note on this
        //     function for why, and for the two residues vs the global chain) ---
        // The airlight is the frame-level one estimated above; only the affine
        // map is per-pixel, so this is the same model as the global stage with
        // the mask weight blending the two ends. `|s| < 1e-4` mirrors
        // `apply_dehaze`'s own floor, so a hair-off-zero slider costs no pass.
        let dehaze_s = m.dehaze.clamp(-100.0, 100.0) / 100.0;
        if dehaze_s.abs() >= 1e-4
            && let Some(a) = dehaze_a
        {
            let (dec, enc) = transfer_luts();
            data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, out_px) in row.iter_mut().enumerate() {
                    let mut wgt = weight_at(x, y);
                    if wgt <= 0.001 {
                        continue;
                    }
                    let p = *out_px;
                    // Range Mask intersection, same convention as the tone pass.
                    if let Some(rm) = &m.range {
                        wgt *= range_weight(rm, &p);
                        if wgt <= 0.001 {
                            continue;
                        }
                    }
                    let t = dehaze_px(&p, a, dehaze_s, dec, enc);
                    for c in 0..3 {
                        out_px[c] = p[c] * (1.0 - wgt) + t[c] * wgt;
                    }
                }
            });
        }

        // An adjustment whose tone/sat/colour stages are ALL identity blends
        // each pixel with itself — skip the full-frame scan (a real cost at
        // 61 MP for an NR-only or freshly parked mask); the clarity, texture
        // and NR passes below each still run on their own `!= 0.0` gate, so a
        // mask that moves ONLY one of those reaches it (before R22 a
        // clarity-only mask fell through every gate and rendered nothing).
        let tone_identity = (camera_raw
            || (m.exposure_ev == 0.0
                && m.contrast == 0.0
                && m.highlights == 0.0
                && m.shadows == 0.0
                && m.whites == 0.0
                && m.blacks == 0.0))
            && m.saturation == 0.0
            && m.hue == 0.0
            && toning.is_none()
            && colour_luts.is_none()
            // A mask whose ONLY move is a point curve reaches the pass through
            // these two terms — the same trap a clarity-only mask fell into
            // before R22 (it fell through every gate and rendered nothing).
            && m.main_curve.is_empty()
            && rgb_curve_luts.is_none();
        // ±100 → ±30°, the same scale `apply_hsl` gives the mixer's hue axis —
        // one meaning for "hue 40" wherever the user sets it. No chroma gate
        // here (the mixer needs one because its per-BAND weights are
        // ill-conditioned on near-greys; a uniform rotation has no band to pick
        // and `hsl_to_rgb` returns an achromatic pixel unchanged).
        let hue_turns = m.hue / 100.0 * (30.0 / 360.0);

        // --- tone + saturation pass (rows independent → parallel) ---
        if !tone_identity {
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, out_px) in row.iter_mut().enumerate() {
                let mut wgt = weight_at(x, y);
                if wgt <= 0.001 {
                    continue;
                }
                let p = *out_px;
                // Range Mask refinement: intersect the geometric weight with the
                // per-pixel range weight, evaluated on the pixel as it stands when
                // this mask runs (post-global develop, pre-this-mask — masks stack
                // sequentially, so a later mask's range sees earlier masks' output;
                // documented approximation vs LR's fixed reference image).
                if let Some(rm) = &m.range {
                    wgt *= range_weight(rm, &p);
                    if wgt <= 0.001 {
                        continue;
                    }
                }
                // Local WB/recolour first (the same exact linear-light model
                // as global apply_wb, sampled through its 4096-entry LUT), then
                // luminance-preserving local tone and saturation. The fully-
                // shifted pixel `t` is blended with the original by mask weight.
                let mut t = p;
                if let Some(luts) = &colour_luts {
                    for c in 0..3 {
                        t[c] = sample_lut(&luts[c], t[c]);
                    }
                }
                if let Some(ml) = &main_lut {
                    t = curve_space.tone(t, ml, MiddleRatio::Encoded);
                } else {
                    let l_old = luma601(&t);
                    let l_new = sample_lut(&lut, l_old);
                    scale_chroma(&mut t, l_old, l_new);
                }
                // Per-channel curves right after the master one, before
                // saturation — `apply_develop`'s stage 1 → 1b → 3 order, so a
                // full-coverage mask carrying a curve lands where the same
                // curve set globally would, to within one 8-bit code.
                // APPROXIMATELY, and measured (R25 P8): the two paths compose
                // the same LUTs but not the same arithmetic — this one fuses
                // the stages per pixel and always finishes through
                // `apply_sat_vibrance`, whose factor-1 identity
                // (`l + (c - l)`) is not bit-exact in the deep shadows. Worst
                // observed 1.5e-5 of a code over 2686 of 17751 channels
                // (`mask_curves_at_full_coverage_match_the_global_curves_
                // within_one_code`, which owns the tolerance). The clarity and
                // texture twins ARE bit-exact; this one is not, and the
                // sentence used to claim it was.
                // v1.17.0: on ProPhoto's channels under the sRGB transfer,
                // the global curves' measured law (`tone::apply_rgb_curves`);
                // measured for a mask too — a full-coverage linear mask's
                // `RedCurve` on Camera Raw is predicted by it to 0.09 ΔE, by
                // the working sRGB channels to 9.5.
                if let Some((luts, active)) = &rgb_curve_luts {
                    t = curve_space.channel_curves(t, luts, *active);
                }
                // v1.17.0: a Saturation on Camera Raw's law moves the colour
                // in the reference primaries about V (above 0) or the HSL
                // lightness (below 0), measured (`mask_cr::saturate_px`).
                let mut t = match &hue_space {
                    Some((under, (dec, enc))) if sat != 0.0 => mask_cr::saturate_px(t, sat, under, dec, enc),
                    _ => apply_sat_vibrance(t[0], t[1], t[2], sat, 0.0),
                };
                // Local hue rotation, LAST in the fused transform: it turns the
                // colour this mask's WB/tone/saturation stages produced, which
                // is the order the sliders read in (Temp shift → Saturation →
                // Hue). Blended by the same single weight as the rest.
                if let (true, Some((under, (dec, enc)))) = (m.hue != 0.0, &hue_space) {
                    t = mask_cr::hue_px(t, m.hue, under, dec, enc);
                } else if hue_turns != 0.0 {
                    let (hh, ss, ll) = rgb_to_hsl(t[0], t[1], t[2]);
                    let (r2, g2, b2) = hsl_to_rgb((hh + hue_turns).rem_euclid(1.0), ss, ll);
                    t = [r2, g2, b2];
                }
                for c in 0..3 {
                    out_px[c] = p[c] * (1.0 - wgt) + t[c] * wgt;
                }
                // The Color tint last, on the blended pixel at the mask's
                // own weight (see the colour-stage order on this function).
                if let Some((push, a)) = toning {
                    *out_px = tint_px(*out_px, push, a * wgt);
                }
            }
        });
        }

        // --- local clarity / texture (SPATIAL: full-frame luma plane → blur →
        //     detail weighted by the mask, exactly like the NR pass below) ---
        //     Neither can ride the per-pixel loop above: a spatial operator
        //     needs neighbours, and that loop is a pure per-pixel LUT.
        let spatial_weight = |x: usize, y: usize, px: &[f32; 3]| -> f32 {
            let mut wgt = weight_at(x, y);
            // Range Mask intersection, same convention as the tone pass — the
            // pixel state here already carries this mask's own tone move (the
            // same documented drift the NR pass notes).
            if wgt > 0.001
                && let Some(rm) = &m.range
            {
                wgt *= range_weight(rm, px);
            }
            wgt
        };
        if m.clarity != 0.0 {
            // The global clarity radius model, verbatim (render.rs stage 3):
            // large-radius midtone-masked local contrast, 2% of the short edge
            // floored at 8 px so a preview and a 61 MP export mean the same
            // thing by "Clarity 30".
            let radius = ((0.02 * w.min(h) as f32).round() as usize).max(8);
            unsharp_luma_weighted(data, w, h, radius, m.clarity / 100.0, true, spatial_weight);
        }
        if m.texture != 0.0 {
            // Texture = a SMALL-radius detail operator with no midtone mask, so
            // it works fine detail across the whole tonal range where clarity
            // works midtone volume. The GLOBAL texture stage (R25 B2,
            // `apply_develop` stage 3b) calls the SAME function, so the two are
            // one calibration — 0.5% of the short edge floored at 2 px on the
            // positive half, and since R29 B8-2 a MEASURED two-lowpass mix on
            // the negative one (`texture_negative_pass`). Positive is ours
            // (Adobe's model is proprietary); negative is fitted to controlled
            // Lightroom ladders and carries its own residuals. Same honesty
            // stance as `manual_vignette_lut` either way: the XMP carries the
            // raw slider value, so Lightroom re-renders it with its own model.
            texture_pass(data, w, h, texture_amount(r, m.texture), spatial_weight);
        }
        if m.sharpness != 0.0 {
            // The GLOBAL Detail panel's operator at this mask's own amount
            // (`detail::sharpen`) — not a second calibration. Lightroom's local
            // Sharpness has no radius, detail or masking of its own and rides
            // the global three, so one slider value means one structure
            // globally and inside a mask, at a preview and at 61 MP (radii in
            // film pixels).
            //
            // SIGNED, unlike the global stage (which is 0..150): ACR's local
            // Sharpness band runs -100..100 and the negative half is the point
            // — the operator's negative branch is a blur toward the Radius
            // Gaussian. That is how a background is thrown back without
            // touching the subject.
            let p = detail::SharpenParams::at_amount(r, m.sharpness);
            detail::sharpen(data, w, h, &p, film, spatial_weight);
        }

        // --- local noise reduction (only where the mask covers) ---
        // Lightroom's local Noise on the global luminance operator, with the
        // global Detail / Contrast (`detail::luma_nr`). The weight folds the
        // Range Mask in exactly as the tone pass does; the pixel state here
        // includes this mask's own tone move — acceptable drift, NR being the
        // subtler effect. Below a tenth of a slider step the pass allocates
        // nothing.
        if m.noise_reduction > LOCAL_NR_GATE {
            let p = detail::LumaNrParams::at_amount(r, m.noise_reduction);
            detail::luma_nr(data, w, h, &p, film, spatial_weight);
        }
    }
}

/// The brush rasters one adjustment stamps at a frame size (a BRUSH group
/// arrives through the geometry slot with no file, R29 Batch-6b): stamped
/// once per mask per develop — memoised across develops, and at THIS frame's
/// size because a dab is a circle in pixels — and held here so a [`Cover`]
/// can borrow them.
pub(super) struct Brushes {
    base: Option<std::sync::Arc<image::GrayImage>>,
    comps: Vec<Option<std::sync::Arc<image::GrayImage>>>,
}

impl Brushes {
    pub(super) fn of(m: &crate::recipe::LocalAdjustment, w: usize, h: usize) -> Self {
        Brushes {
            base: brush_raster(&m.mask, w as u32, h as u32),
            comps: m.components.iter().map(|c| brush_raster(&c.geometry, w as u32, h as u32)).collect(),
        }
    }
}

/// One adjustment's coverage on a `w × h` frame: the combined geometry, its
/// inversion and its master amount at a pixel — what every mask pass weighs
/// its move by (the Range Mask is the pass's own business: it reads the pixel
/// as that pass finds it).
pub(super) struct Cover<'a> {
    m: &'a crate::recipe::LocalAdjustment,
    bmp: Option<Bmp<'a>>,
    comp_bmps: Vec<Option<Bmp<'a>>>,
    unwarp: Option<&'a MaskUnwarp>,
    w: usize,
    h: usize,
    amount: f32,
}

impl<'a> Cover<'a> {
    /// `None` for an adjustment with an unloadable raster. Such a raster
    /// carries NO coverage, so its weight must never reach the inversion: 0
    /// with `inverted` would apply the adjustment to the WHOLE frame at full
    /// strength. Skipping the whole adjustment is the inert contract
    /// (recipe.rs `MaskGeometry::Bitmap`) — and it covers COMPONENTS for the
    /// same reason: a lost Subtract raster contributes 0 and silently WIDENS
    /// the effect area. Bitmap geometry is decoded once per mask per develop
    /// (the snapshot), never inside the pixel loop.
    pub(super) fn new(
        m: &'a crate::recipe::LocalAdjustment,
        brushes: &'a Brushes,
        rasters: &'a MaskRasterSnapshot,
        unwarp: Option<&'a MaskUnwarp>,
        w: usize,
        h: usize,
    ) -> Option<Self> {
        // The two sources cannot collide: `rasters.get` answers only for a
        // geometry with a raster PATH (`geometry_raster_path`), which a brush
        // never has.
        let bmp = rasters.get(&m.mask).or(brushes.base.as_deref().map(Bmp::sensor));
        let comp_bmps: Vec<Option<Bmp<'_>>> = m
            .components
            .iter()
            .zip(&brushes.comps)
            .map(|(c, brush)| rasters.get(&c.geometry).or(brush.as_deref().map(Bmp::sensor)))
            .collect();
        if (bmp.is_none() && is_raster_backed(&m.mask))
            || m.components.iter().zip(&comp_bmps).any(|(c, b)| b.is_none() && is_raster_backed(&c.geometry))
        {
            return None;
        }
        Some(Cover { m, bmp, comp_bmps, unwarp, w, h, amount: m.amount.clamp(0.0, 1.0) })
    }

    /// The combined mask coverage × master amount at pixel `(x, y)`, with
    /// inversion. PIXEL CENTRES, `(x + 0.5)/w` — see `MASK_SAMPLE_CENTRE` for
    /// the measurement, the derivation, and the render-behaviour change.
    pub(super) fn at(&self, x: usize, y: usize) -> f32 {
        let (w, h) = (self.w as f32, self.h as f32);
        let (nx, ny) = ((x as f32 + MASK_SAMPLE_CENTRE) / w, (y as f32 + MASK_SAMPLE_CENTRE) / h);
        let mut wgt = combined_mask_weight(self.m, nx, ny, self.bmp, &self.comp_bmps, self.unwarp, (w, h));
        if self.m.inverted {
            wgt = 1.0 - wgt;
        }
        wgt * self.amount
    }

    /// [`Cover::at`] with the Range Mask read on `px`, the pixel as the
    /// calling pass finds it (masks stack, so a later mask's range sees the
    /// earlier masks' output — the documented approximation vs Lightroom's
    /// fixed reference image).
    pub(super) fn at_px(&self, x: usize, y: usize, px: &[f32; 3]) -> f32 {
        let wgt = self.at(x, y);
        match &self.m.range {
            Some(rm) if wgt > 0.001 => wgt * range_weight(rm, px),
            _ => wgt,
        }
    }
}

/// WHERE a mask chain's output will be sampled — the value that keeps
/// parametric mask geometry and the pixel warp travelling together.
///
/// # The defect this closes (R29 Batch-3, user ruling 2026-08-20)
///
/// Lightroom stores mask geometry in different frames. Brush dabs live
/// PRE-lens-correction. RADIAL points live POST-correction. LINEAR stores two
/// sensor-frame handles that move with the photograph, and draws a straight
/// gradient between them in whatever frame the photograph is shown in.
/// The `D` adjudication measured the RADIAL frame at pixel level on the 105 mm pair: the
/// PIXELS move +87.5 px at r ≈ 3250 (the `.lcp` model at 2.69 px rms over 30
/// NCC points, tangential rms 1.22 px) while the radial mask itself measures a
/// similarity of 0.99956 — the identity to 0.05 %, and **88.7 px away from the
/// pixel field**.
///
/// This engine evaluates every mask BEFORE its geometry stage (`apply_masks`
/// runs inside `apply_develop`, `apply_lens_geometry` after it) and then
/// resamples the whole frame. Let `T_engine` map an original pixel to the
/// corrected output and `m_lr` map Lightroom's stored parametric geometry to
/// its exported point. The sample adapter therefore has this table:
///
/// * a BRUSH is then already right — Lightroom rasterises it in that same
///   pre-correction frame, so the two agree with nothing applied. Warping a
///   dab here would apply the field twice.
/// * a RADIAL was **wrong by the whole field** — up to 186 px at 24 mm and
///   88 px at 105 mm — because Lightroom does not move it and this engine did.
/// * a LINEAR needs neither RADIAL's pointwise Lightroom inverse nor a
///   pointwise forward warp. With downstream geometry its two handles are
///   carried once by `lens_ungeom_norm` (with the photograph) and it is
///   sampled at `lens_ungeom_norm(p)`, a straight line in the corrected frame
///   (v1.17.0, Camera Raw); without downstream geometry nothing moves the
///   photograph, so nothing moves its handles: it is evaluated at its stored
///   coordinates.
///
/// RADIAL maps each sample point through the inverse description of where
/// engine geometry will put it, then translates it back by the displacement
/// the same map gives its ellipse centre ([`MaskUnwarp::radial_at`]). LINEAR
/// samples at [`MaskUnwarp::engine_at`] with both handles carried by it
/// ([`MaskFrame::prepare_linear_handles`]).
///
/// **The 2026-08 D2 "H2" LINEAR rule is superseded (v1.17.0).** It read the
/// handles as corrected-frame points with correction on, and with correction
/// off sent them through the in-camera map (`D_fwd`), fitted to three
/// Lightroom wall exports at 4.979–12.449 px rms that are not in the fixtures
/// and cannot be re-checked. Camera Raw renders of two frames (p38, p39) at
/// 0.25 EV now measure both arms: correction off, the stored handles score
/// IoU½ 0.9983 / 0.9905 / 0.9967 against `D_fwd`'s 0.9956 / 0.9810 / 0.9944;
/// correction on, see [`MaskFrame::prepare_linear_handles`].
///
/// # Why this is a parameter and not derived from the recipe
///
/// Because "does the geometry stage run?" is the CALLER's fact, not the
/// recipe's. Five surfaces compose develop-then-geometry (`render_to_file`'s
/// two arms, the GUI preview, the GUI coverage overlay, the web preview) and
/// all five gate on the same expression — but the GUI's range REFERENCE builds
/// (`canvas.rs`) deliberately develop a recipe that still carries a lens
/// profile and then apply NO geometry, because they exist to sample pixel
/// values, not to be looked at. They state that with
/// [`MaskFrame::AsRendered`], which keeps every geometry at its stored
/// coordinates.
///
/// So the invariant is stated as a value: **whoever runs the geometry stage
/// builds this from the same profile and amount it will pass to
/// [`apply_lens_geometry`], and uses [`MaskFrame::warps`] to decide whether to
/// run it at all; whoever omits it passes [`MaskFrame::AsRendered`].**
#[derive(Clone, Copy, Debug)]
pub enum MaskFrame<'a> {
    /// The caller WILL resample this buffer through [`apply_lens_geometry`]
    /// with exactly this profile and manual amount.
    WarpedDownstream { profile: &'a crate::recipe::LensProfile, amount: f32 },
    /// Nothing downstream moves these pixels: the mask chain's output IS what
    /// will be looked at, so every geometry is evaluated at its stored
    /// coordinates.
    AsRendered,
    /// The pixels are ALREADY lens-corrected through exactly this profile (the
    /// camera stage resampled them, v1.17.0, `lens_in_camera`) and nothing
    /// downstream moves them. Every geometry is stored in the original frame,
    /// so each corrected pixel first finds its original-frame point through the
    /// resample's own map (the green channel's) and is then evaluated there
    /// exactly as [`MaskFrame::WarpedDownstream`] evaluates it: the same mask
    /// at the same place as when the tail resampled the masked frame.
    Corrected { profile: &'a crate::recipe::LensProfile },
}

impl<'a> MaskFrame<'a> {
    /// What a caller that runs the geometry stage passes.
    ///
    /// Answers [`MaskFrame::AsRendered`] when no resample follows. The SAME
    /// geometry condition gates `apply_lens_geometry`.
    pub fn downstream(profile: &'a crate::recipe::LensProfile, amount: f32) -> Self {
        if profile.geometry_active() || amount != 0.0 {
            MaskFrame::WarpedDownstream { profile, amount }
        } else {
            MaskFrame::AsRendered
        }
    }

    /// The frame for pixels developed from a base that is (`corrected`) or is
    /// not already lens-corrected: [`MaskFrame::Corrected`], or what
    /// [`MaskFrame::downstream`] answers for a tail that will run the geometry.
    pub fn of_base(profile: &'a crate::recipe::LensProfile, amount: f32, corrected: bool) -> Self {
        if corrected { MaskFrame::Corrected { profile } } else { Self::downstream(profile, amount) }
    }

    /// The frame for a SAMPLED develop (no geometry stage follows) of a base
    /// that is (`corrected`) or is not already lens-corrected:
    /// [`MaskFrame::Corrected`], or [`MaskFrame::AsRendered`].
    pub fn sampled(profile: &'a crate::recipe::LensProfile, corrected: bool) -> Self {
        if corrected { MaskFrame::Corrected { profile } } else { MaskFrame::AsRendered }
    }

    /// Will the geometry stage run? The caller's gate, so that the gate and the
    /// mask map are one decision.
    pub fn warps(self) -> bool {
        matches!(self, MaskFrame::WarpedDownstream { .. })
    }

    /// The inverse map for this frame, or `None` when nothing moves.
    pub(super) fn unwarp(self, dims: (f32, f32)) -> Option<MaskUnwarp> {
        match self {
            MaskFrame::WarpedDownstream { profile, amount } => {
                MaskUnwarp::new(profile, amount, dims)
            }
            MaskFrame::Corrected { profile } => MaskUnwarp::corrected(profile, dims),
            MaskFrame::AsRendered => None,
        }
    }

    /// Move LINEAR's two handles once per local adjustment, for the frame the
    /// pixels are evaluated in. The returned owned value exists only when at
    /// least one base/component LINEAR geometry moved; every pixel then
    /// evaluates one straight gradient between the moved handles.
    ///
    /// * Lens geometry in the frame ([`MaskFrame::WarpedDownstream`],
    ///   [`MaskFrame::Corrected`]): each handle is a sensor-frame point carried
    ///   with the photograph by the engine's own map ([`MaskUnwarp::engine_at`],
    ///   the carry [`MaskUnwarp::radial_at`] gives a RADIAL centre), and the
    ///   pixel loop draws the line straight in the corrected frame between the
    ///   carried handles. **Measured against Camera Raw (v1.17.0)**, 0.25 EV
    ///   local exposure, weight maps against Camera Raw's own at IoU½: p39 one
    ///   gradient 0.9891 → 0.9962 (rms 0.093 → 0.032) predicted from Camera
    ///   Raw's content displacement at the handles; p38 two gradients in
    ///   opposite corners 0.9900 → 0.9980 and 0.9820 → 0.9902. The handles
    ///   uncarried (the 2026-08 H2 "corrected-frame handles" reading) scored
    ///   0.9918 / 0.9815 on p38.
    /// * No lens geometry ([`MaskFrame::AsRendered`]): the stored handles,
    ///   untouched. A disabled profile used to send them through the in-camera
    ///   map (H2's `D_fwd`); Camera Raw does not (see [`MaskFrame`]).
    pub(super) fn prepare_linear_handles<'b>(
        self,
        mask: &'b crate::recipe::LocalAdjustment,
        dims: (f32, f32),
    ) -> Cow<'b, crate::recipe::LocalAdjustment> {
        let Some(u) = self.unwarp(dims) else { return Cow::Borrowed(mask) };
        let mut out = mask.clone();
        let mut moved = carry_linear_handles(&mut out.mask, &u);
        for component in &mut out.components {
            moved |= carry_linear_handles(&mut component.geometry, &u);
        }
        if moved { Cow::Owned(out) } else { Cow::Borrowed(mask) }
    }
}

/// A raster drawn in the ORIGINAL frame (a brush, a face the detector found on
/// the sensor-frame picture), read into the lens-CORRECTED frame a base is
/// developed in when the camera stage resamples the lens (v1.17.0,
/// [`super::lens_in_camera`]): every corrected pixel takes the original-frame
/// point it shows, through the map [`MaskFrame::Corrected`] samples masks
/// with. A copy when the profile moves nothing.
pub fn sensor_raster_to_corrected(mask: &image::GrayImage, profile: &crate::recipe::LensProfile) -> image::GrayImage {
    let dims = (mask.width() as f32, mask.height() as f32);
    match MaskUnwarp::corrected(profile, dims) {
        Some(u) => raster_through(mask, &u),
        None => mask.clone(),
    }
}

/// [`sensor_raster_to_corrected`]'s inverse, for a raster drawn in the
/// lens-CORRECTED frame of `profile` and `amount` (a reverse-fit zone, marked
/// so, `render::MaskRaster`) read into the sensor frame: every sensor pixel
/// takes the corrected point the engine's geometry puts it at
/// ([`MaskUnwarp::engine_at`], the map a LINEAR mask is read through). A copy
/// when the profile moves nothing.
pub fn corrected_raster_to_sensor(mask: &image::GrayImage, profile: &crate::recipe::LensProfile, amount: f32) -> image::GrayImage {
    let (w, h) = (mask.width() as f32, mask.height() as f32);
    let Some(u) = MaskUnwarp::new(profile, amount, (w, h)) else { return mask.clone() };
    image::GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        let (qx, qy) = u.engine_at((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
        image::Luma([(sample_gray_norm(mask, qx, qy) * 255.0).round().clamp(0.0, 255.0) as u8])
    })
}

/// [`sensor_raster_to_corrected`] for an RGBA raster (a painted heal or fill
/// mask): every channel read the same way, so a pixel past the original
/// frame's edge takes the edge's paint, as a grey mask's does — never
/// "unpainted", which a fill mask spells as transparent.
pub fn sensor_rgba_to_corrected(raster: &image::RgbaImage, profile: &crate::recipe::LensProfile) -> image::RgbaImage {
    let (w, h) = (raster.width() as f32, raster.height() as f32);
    let Some(u) = MaskUnwarp::corrected(profile, (w, h)) else { return raster.clone() };
    image::RgbaImage::from_fn(raster.width(), raster.height(), |x, y| {
        let (sx, sy) = u.source_point((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
        image::Rgba(std::array::from_fn(|c| {
            let v = super::mask_raster::sample_texels_norm(raster.width(), raster.height(), sx, sy, |tx, ty| {
                raster.get_pixel(tx, ty)[c] as f32
            });
            v.round().clamp(0.0, 255.0) as u8
        }))
    })
}

/// `alpha` read through `u`'s corrected-frame map ([`MaskUnwarp::source_point`]),
/// bilinearly at texel centres, at `alpha`'s own size.
pub(super) fn raster_through(alpha: &image::GrayImage, u: &MaskUnwarp) -> image::GrayImage {
    let (w, h) = (alpha.width() as f32, alpha.height() as f32);
    image::GrayImage::from_fn(alpha.width(), alpha.height(), |x, y| {
        let (sx, sy) = u.source_point((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
        image::Luma([(sample_gray_norm(alpha, sx, sy) * 255.0).round().clamp(0.0, 255.0) as u8])
    })
}

/// Carry a LINEAR component's two handles with the photograph: each stored
/// (sensor-frame) handle goes where the engine's lens map puts that point.
/// Returning `true` lets the caller avoid cloning adjustments with no LINEAR
/// geometry.
fn carry_linear_handles(geometry: &mut MaskGeometry, u: &MaskUnwarp) -> bool {
    let (MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }
        | MaskGeometry::Shaped { zero_x, zero_y, full_x, full_y, .. }) = geometry
    else {
        return false;
    };
    (*zero_x, *zero_y) = u.engine_at(*zero_x, *zero_y);
    (*full_x, *full_y) = u.engine_at(*full_x, *full_y);
    true
}

/// RADIAL-only: ORIGINAL-frame point → the Lightroom-stored sample point
/// whose effect will occupy the right Lightroom export point after this
/// engine's geometry stage.
///
/// This is `T_engine(p) − (T_engine(c) − c)`, `c` the ellipse's stored
/// centre: ask where original pixel `p` will land under the exact
/// downstream engine resample, then shift that output coordinate back by
/// how far the same resample carries the CENTRE. `T_engine` is built by
/// calling [`lens_ungeom_norm`]; the downstream resample supplies the one
/// `T_engine` application after mask rasterisation.
///
/// **Why a translation, and why the engine's own map (v1.17.0, Camera
/// Raw).** The 2026-08 law sampled the whole ellipse through Lightroom's
/// mask-warp spline `m_lr⁻¹` pointwise, which dilates it by the local
/// magnification: a centred feather-0 ellipse on a 105 mm frame had its
/// edge at 1.045 of the stored radius, where Camera Raw draws it at 1.005
/// (lens off: both 1.000). Off-centre ellipses showed Camera Raw moving the
/// centre exactly as the photograph moves there, at an unchanged size — and
/// the `m_lr` spline moving it 8–22 % further than the pixels on the four
/// frames measured (`radial_at`). The 2026-08 41-vector fixture on another
/// body still pins `lr_mask_warp_norm` itself; it no longer routes a
/// radial. The manual `lens_distortion` amount is covered with no residue.
/// LINEAR, brush, bitmap and AI never call this adapter; the explicit
/// match in `mask_weight_in` is the type boundary.
///
/// A LUT because `lens_ungeom_norm` costs a 256-step peak scan plus 40
/// bisection steps per call, and this is a per-pixel question on frames up to
/// 61 MP. The map is radial, so [`LUT_N`] nodes over the normalised radius
/// carry it exactly as the resampler's own per-channel LUTs carry the forward
/// map, at the same node density.
pub(super) struct MaskUnwarp {
    /// Factor `r_corrected / r_original` at node `i` = radius `i/(LUT_N−1)` of
    /// the half-diagonal.
    lut: Vec<f32>,
    /// [`MaskFrame::Corrected`] only: the resample's own green-channel factor
    /// `r_original / r_corrected` over the corrected radius
    /// ([`super::channel_luts`]), which takes a corrected pixel to the
    /// original-frame point it was resampled from ([`MaskUnwarp::source_point`]).
    pre: Option<Vec<f32>>,
    rr: f32,
    w: f32,
    h: f32,
}

impl MaskUnwarp {
    pub(super) fn new(profile: &crate::recipe::LensProfile, amount: f32, dims: (f32, f32)) -> Option<Self> {
        let (w, h) = dims;
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
        // Sampled along the +x axis: the map is radial, so one ray carries it,
        // and going through the public entry point keeps this honest.
        let lut: Vec<f32> = (0..LUT_N)
            .map(|i| {
                let rho = i as f32 / (LUT_N - 1) as f32;
                let dx = rho * rr;
                if dx <= 1e-6 {
                    // The centre is a fixed point of every radial map; the
                    // ratio there is 0/0 and the limit is the next node's.
                    return f32::NAN;
                }
                let (ox, _) = lens_ungeom_norm(dx / w + 0.5, 0.5, dims, profile, amount);
                (ox - 0.5) * w / dx
            })
            .collect();
        let mut lut = lut;
        if lut.len() > 1 {
            lut[0] = lut[1];
        }
        // Identity map = nothing to do, and saying so here is what keeps a
        // distortion-free profile bit-identical rather than merely close.
        //
        // The threshold is in PIXELS, not in factor units, because that is the
        // question: a factor of 1 ± 1e-7 on a 9504 px frame moves a mask by
        // 6e-4 px, which is not a displacement, it is the bisection's own
        // residue (`lens_ungeom_norm` does not short-circuit for an active
        // profile whose knots are all 1.0 — it solves, and lands a few ulps
        // off). Anything a real lens produces is four orders larger: the
        // gentlest frame measured in this batch moves 0.6 % at the centre.
        //
        // MUTATION THIS KILLS: dropping this guard makes every coordinate on a
        // distortion-free profile take a float round trip through `at`, and
        // `with_the_geometry_stage_inactive_the_mask_chain_is_untouched` goes red.
        if lut.iter().all(|f| (f - 1.0).abs() * rr < 0.01) {
            return None;
        }
        Some(MaskUnwarp { lut, pre: None, rr, w, h })
    }

    /// [`MaskFrame::Corrected`]'s map: [`MaskUnwarp::new`]'s, with the camera
    /// stage's resample in front of it. `None` when the profile moves nothing.
    pub(super) fn corrected(profile: &crate::recipe::LensProfile, dims: (f32, f32)) -> Option<Self> {
        let mut u = Self::new(profile, 0.0, dims)?;
        let [_, green, _] = super::channel_luts(profile, 0.0, dims);
        u.pre = Some(green);
        Some(u)
    }

    /// The ORIGINAL-frame point the pixel at `(nx, ny)` shows: itself, or, for
    /// a corrected frame, where the camera stage's resample read it from — the
    /// same radius, centre and factor table `apply_lens_geometry_linear` uses
    /// (pixel centres: `(nx − 0.5)·w` is its `x − (w − 1)/2`).
    pub(super) fn source_point(&self, nx: f32, ny: f32) -> (f32, f32) {
        let Some(pre) = &self.pre else { return (nx, ny) };
        let (dx, dy) = ((nx - 0.5) * self.w, (ny - 0.5) * self.h);
        let rn = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, 1.0);
        let f = super::sample_lut(pre, rn);
        ((dx * f) / self.w + 0.5, (dy * f) / self.h + 0.5)
    }

    /// Exact inverse of the downstream engine geometry, without Lightroom's
    /// point-transport half: where the photograph carries a sensor-frame
    /// point. LINEAR samples through it and carries its two handles by it, so
    /// its straight line lands in the corrected output frame without
    /// acquiring RADIAL's map.
    pub(super) fn engine_at(&self, nx: f32, ny: f32) -> (f32, f32) {
        let (dx, dy) = ((nx - 0.5) * self.w, (ny - 0.5) * self.h);
        let rho = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, 1.0);
        let t = rho * (LUT_N - 1) as f32;
        let i = (t.floor() as usize).min(LUT_N - 2);
        let f = t - i as f32;
        let k = self.lut[i] * (1.0 - f) + self.lut[i + 1] * f;
        ((dx * k) / self.w + 0.5, (dy * k) / self.h + 0.5)
    }

    /// RADIAL: the stored-frame point whose mask value pixel `(nx, ny)`
    /// carries, for an ellipse centred at the stored `(cx, cy)`. Camera Raw
    /// carries the ellipse's CENTRE with the photograph — a sensor-frame
    /// point, moved by the same lens field as the pixels — and draws its
    /// radii in the corrected frame. So the sample is the pixel's corrected
    /// position less the centre's displacement under the SAME map: a
    /// translation, not a dilation. LINEAR calls the engine-only half above.
    ///
    /// Measured against Camera Raw (v1.17.0), feather-0 ellipses with lens
    /// correction on, on four frames (24–105 mm at 45, 73 and 105 mm; a
    /// 14–24 mm at 20 mm): a centred one's edge at 1.0025–1.0055 of its
    /// stored radius where the old pointwise transport put it at 1.045;
    /// two corner ones' centres displaced by what the frame's own content
    /// moves there (0.0286 / 0.0298 of the centre's offset against 0.029),
    /// and the in-camera mask-warp spline moving them 8–22 % further.
    pub(super) fn radial_at(&self, nx: f32, ny: f32, cx: f32, cy: f32) -> (f32, f32) {
        let (qx, qy) = self.engine_at(nx, ny);
        let (lx, ly) = self.engine_at(cx, cy);
        (qx - (lx - cx), qy - (ly - cy))
    }
}

/// Historical classification assertion for the unchanged regression below.
///
/// Production routing deliberately does NOT use this union: H2 requires RADIAL
/// point transport and LINEAR handle transport to take separate match arms.
/// The test-only helper keeps the older classification test byte-for-byte while
/// the reasons for the non-parametric types remain registered beside it:
///
/// * `Radial` / `Linear` — YES. Measured post-correction (`MaskFrame`).
/// * `Brush` — no. Measured PRE-correction, which is the frame this engine
///   already evaluates in; the geometry stage carries it correctly untouched.
/// * `Bitmap` / `AiMask` — no, and not for want of measuring. These rasters are
///   ENGINE-generated (our segmenter, the GUI's own paint), so there is no
///   Lightroom rendering for them to agree with; they are authored in the frame
///   the engine draws them in and stay there.
/// * A colour / luminance `RangeMask` is not here at all because it selects by
///   pixel VALUE, not position, and a pixel keeps its value through a resample
///   — approximately frame-invariant, so nothing to map. (Approximately: the
///   resampler interpolates, so a value on a steep edge shifts slightly. That
///   residue is sub-pixel and is registered here rather than modelled.)
#[cfg(test)]
pub(super) fn is_lr_post_correction_geometry(g: &MaskGeometry) -> bool {
    matches!(g, MaskGeometry::Radial { .. } | MaskGeometry::Linear { .. } | MaskGeometry::Shaped { .. })
}

/// The local Noise Reduction at or below which the render's NR pass allocates
/// nothing — a tenth of a slider step. [`engine_active`] reads the SAME line,
/// because the GUI's ● marker and the raster budget are promises about what
/// the render will do: one constant, two readers, so a mutation of either is a
/// mutation of both. (They were two literals once, and before that `!= 0.0` on
/// one side against `> 0.1` on the other.)
pub(super) const LOCAL_NR_GATE: f32 = 0.1;

/// The ENGINE's own activity rule for one local adjustment: does
/// [`apply_masks`] have anything to do for it? Every `!= 0.0` gate inside that
/// function is mirrored here — identity tone/sat + no local WB/recolour + no
/// local dehaze/clarity/texture/NR renders nothing even with a healthy raster.
///
/// Two consumers depend on it and must never disagree with the render: the
/// GUI's mask-list activity marker (so the ● the user sees IS the rule the
/// render applies) and `load_mask_raster_snapshot_with_budget`, which spends
/// the raster budget only on masks that will actually render. Adding the
/// clarity/dehaze/texture terms in R22 fixed both at once: a clarity-only
/// bitmap mask used to read "parked" AND have its raster left unloaded.
pub fn engine_active(m: &crate::recipe::LocalAdjustment) -> bool {
    m.exposure_ev != 0.0
        || m.contrast != 0.0
        || m.highlights != 0.0
        || m.shadows != 0.0
        || m.whites != 0.0
        || m.blacks != 0.0
        || m.clarity != 0.0
        || m.dehaze != 0.0
        || m.texture != 0.0
        || m.sharpness != 0.0
        || m.saturation != 0.0
        || m.hue != 0.0
        || m.temperature != 0.0
        || m.tint != 0.0
        // The tint's hue alone is inert: the pass is gated on its amount.
        || m.toning_amount > 0.0
        // The render's NR pass allocates nothing at or below `LOCAL_NR_GATE`.
        || m.noise_reduction > LOCAL_NR_GATE
        // The four local point curves (R25 P6). Empty = identity, exactly as
        // `apply_develop`'s own `tone_neutral` reads the global curves — a
        // non-empty curve is an edit even if its points happen to trace the
        // diagonal, which is the same latitude the global stage takes.
        || !m.main_curve.is_empty()
        || !m.red_curve.is_empty()
        || !m.green_curve.is_empty()
        || !m.blue_curve.is_empty()
        || m.color_gains.is_some_and(|g| g != [1.0, 1.0, 1.0])
}


// The mask Color tint (`crs:LocalToningHue` / `LocalToningSaturation`).
//
// The model is fitted, not guessed. First on the seaside street exported from
// Lightroom (cloud 9.4) with nothing moved, then with ONE whole-frame mask
// carrying only the Color swatch — hue 210, amount 0.782609 and 0.391304, as
// the XMP packet each export embeds records. Per pixel, tinted − flat says
// what the control is: the shift points the same way on neutral, mid-chroma
// and warm pixels (an ADDITIVE chroma shift, not a blend toward a colour — a
// blend bends it on every pixel that already has a colour); the two exports'
// shifts stand 1.94–2.05 in every bin (linear in the amount); it peaks in the
// midtones and fades in the shadows and highlights (a lightness window); and
// lightness hardly moves (≤ 2.2 L where the shift is 23 units). Of the six
// models fitted to both exports at once, the additive shift in Oklab with a
// Gaussian window lands the sky at mean ΔE 2.62 (1.82 on the neutral pixels,
// the clouds), where a blend toward the hue at the pixel's own luminance read
// 7.87 on the sky.
//
// Then across the wheel, on Camera Raw renders of the user's own mask packet
// (2026-10-27): the swatch at hues 0, 60, 120, 180, 240 and 300 at amount
// 0.5, and hue 210 at 0.25, 0.391304, 0.782609 and 1.0, each against the
// same packet at amount 0. Camera Raw reproduces the user's Lightroom
// exports of that packet to mean ΔE 0.32 / 0.33 (0.31 untinted), so the
// renders stand for Lightroom. Neither the direction nor the strength of the
// push is uniform around the wheel in Oklab: read one hue at a time it
// pushes along Oklab 12.0°, 100.0°, 155.3°, 190.5°, 217.7°, 276.6° and
// 336.1° for hues 0, 60, 120, 180, 210, 240 and 300, at 0.086 to 0.158 per
// unit amount. Both follow Lightroom's own working space: the step from
// linear mid grey toward the fully saturated linear-ProPhoto colour of the
// hue ([`prophoto_push`]) points within 5.4° of every measured push, and its
// length tracks the measured strength at r = 0.97. What it misses, the
// nodes keep ([`TINT_NODES`]). With one shared window over all twelve pairs
// (fitted on a 30k-pixel sample at 1200 px) the mean ΔE is 1.66; the map this
// replaced (Oklab hue = Lightroom hue + 8.1°, one strength) read 2.42–6.14 at
// the six hues.

/// Centre of the tint's lightness window, in the gamma-encoded Rec.709 luma
/// of the pixel the tint receives ([`tint_luma`]), fitted jointly with
/// [`TINT_WINDOW_SIGMA`] and the seven nodes over all twelve pairs.
pub(crate) const TINT_WINDOW_CENTRE: f32 = 0.469;

/// Width (σ) of the tint's lightness window, same quantity and same fit as
/// [`TINT_WINDOW_CENTRE`]. (The two Lightroom exports alone read 0.470 / 0.272.)
pub(crate) const TINT_WINDOW_SIGMA: f32 = 0.283;

/// The measured wheel: per Lightroom hue, the Oklab angle (degrees) the
/// swatch pushes along and its Oklab chroma shift per unit of the 0..1
/// amount at the window's centre and full mask weight, each read from that
/// hue's pairs alone under the window above (one joint fit, 2026-10-27).
/// Hue 210 pools its six pairs (four Camera Raw amounts, two Lightroom
/// exports); read one at a time its amounts give 0.1011, 0.1051, 0.1074 and
/// 0.1029 per unit at 0.25, 0.39, 0.78 and 1.0 — linear to 6 %. Between nodes
/// the push is the ProPhoto step turned and scaled by the two neighbouring
/// nodes' corrections, interpolated linearly in hue; those hues are not
/// measured.
const TINT_NODES: [(f32, f32, f32); 7] = [
    (0.0, 12.0, 0.1276),
    (60.0, 100.0, 0.0926),
    (120.0, 155.3, 0.1576),
    (180.0, 190.5, 0.1422),
    (210.0, 217.7, 0.1004),
    (240.0, 276.6, 0.0861),
    (300.0, 336.1, 0.1531),
];

/// Linear ProPhoto (ROMM) RGB → linear sRGB: ProPhoto → XYZ (D50), Bradford
/// D50 → D65, XYZ → sRGB (Lindbloom's published matrices, multiplied out).
const PROPHOTO_TO_LINEAR_SRGB: [[f64; 3]; 3] = [
    [2.034_076_1, -0.727_334_2, -0.306_741_9],
    [-0.228_813_4, 1.231_730_1, -0.002_916_8],
    [-0.008_569_7, -0.153_286_6, 1.161_856_4],
];

/// The gamma-encoded Rec.709 luma `0.2126 R' + 0.7152 G' + 0.0722 B'` of a
/// display-encoded pixel — the quantity the window was fitted in. Not
/// Oklab L and not `luma601`: the constants above are this quantity's.
pub(crate) fn tint_luma(px: &[f32; 3]) -> f32 {
    0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
}

/// The tint's lightness window at gamma luma `lp`: 1 at the centre.
pub(crate) fn tint_window(lp: f32) -> f32 {
    let z = (lp - TINT_WINDOW_CENTRE) / TINT_WINDOW_SIGMA;
    (-0.5 * z * z).exp()
}

/// Linear sRGB → Oklab (Björn Ottosson, 2020: his M1, the cube root, his M2).
pub(crate) fn linear_to_oklab(c: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_47 * c[0] + 0.536_332_55 * c[1] + 0.051_445_995 * c[2]).max(0.0).cbrt();
    let m = (0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2]).max(0.0).cbrt();
    let s = (0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2]).max(0.0).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab → linear sRGB, the exact inverse of [`linear_to_oklab`] (his own
/// published inverse matrices). Out-of-gamut results come back as they are;
/// the caller decides what to do with them.
pub(crate) fn oklab_to_linear(lab: [f32; 3]) -> [f32; 3] {
    let l = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// The base wheel: the Oklab (a, b) step, per unit of step, from linear
/// ProPhoto mid grey toward the fully saturated ProPhoto colour of HSV hue
/// `hue_deg`. It is the differential, so neither the grey level nor the
/// encoding matters (measured: grey 0.18, 0.3 and 0.5, linear or gamma 1.8,
/// all agree). In f64 because it is a difference of nearly equal cube roots.
fn prophoto_push(hue_deg: f32) -> [f64; 2] {
    const EPS: f64 = 1e-3;
    const GREY: f64 = 0.5;
    let h = f64::from(hue_deg).rem_euclid(360.0) / 60.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let rgb = match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    };
    let ab = |p: [f64; 3]| {
        let m = PROPHOTO_TO_LINEAR_SRGB;
        let c = [0, 1, 2].map(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2]);
        let l = (0.412_221_470_8 * c[0] + 0.536_332_536_3 * c[1] + 0.051_445_992_9 * c[2]).cbrt();
        let mm = (0.211_903_498_2 * c[0] + 0.680_699_545_1 * c[1] + 0.107_396_956_6 * c[2]).cbrt();
        let s = (0.088_302_461_9 * c[0] + 0.281_718_837_6 * c[1] + 0.629_978_700_5 * c[2]).cbrt();
        [
            1.977_998_495_1 * l - 2.428_592_205 * mm + 0.450_593_709_9 * s,
            0.025_904_037_1 * l + 0.782_771_766_2 * mm - 0.808_675_766 * s,
        ]
    };
    let (o0, o1) = (ab([GREY; 3]), ab(rgb.map(|v| GREY + EPS * (v - GREY))));
    [(o1[0] - o0[0]) / EPS, (o1[1] - o0[1]) / EPS]
}

/// The Oklab (a, b) shift the swatch at Lightroom hue `hue_deg` makes per
/// unit of amount at the window's centre and full mask weight. At a node of
/// [`TINT_NODES`] it is that node's measured push exactly; between nodes it
/// is [`prophoto_push`] turned by, and scaled to, the two neighbouring
/// nodes' corrections interpolated linearly in hue.
pub(crate) fn tint_push(hue_deg: f32) -> [f32; 2] {
    let h = hue_deg.rem_euclid(360.0);
    let correction = |i: usize| {
        let (nh, angle, k) = TINT_NODES[i];
        let u = prophoto_push(nh);
        let turn = (f64::from(angle).to_radians() - u[1].atan2(u[0])).to_degrees();
        ((turn + 180.0).rem_euclid(360.0) - 180.0, f64::from(k) / u[0].hypot(u[1]))
    };
    let n = TINT_NODES.len();
    let i = TINT_NODES.iter().rposition(|node| node.0 <= h).unwrap_or(0);
    let (h0, h1) = (TINT_NODES[i].0, if i + 1 < n { TINT_NODES[i + 1].0 } else { 360.0 });
    let t = f64::from((h - h0) / (h1 - h0));
    let ((turn0, scale0), (turn1, scale1)) = (correction(i), correction((i + 1) % n));
    let (turn, scale) = (turn0 + t * (turn1 - turn0), scale0 + t * (scale1 - scale0));
    let u = prophoto_push(h);
    let (sin, cos) = turn.to_radians().sin_cos();
    [((u[0] * cos - u[1] * sin) * scale) as f32, ((u[0] * sin + u[1] * cos) * scale) as f32]
}

/// The Lightroom hue (0..360, to a tenth of a degree) and the 0..1 amount
/// whose swatch makes the Oklab (a, b) shift `push` at the window's centre:
/// [`tint_push`] inverted by the nearest direction on the tenth-degree
/// wheel (the measured wheel turns monotonically, so the nearest is the one).
pub(crate) fn tint_from_push(push: [f32; 2]) -> (f32, f32) {
    use std::f32::consts::{PI, TAU};
    let theta = push[1].atan2(push[0]);
    let (mut best, mut best_gap) = (0.0f32, f32::INFINITY);
    for tenth in 0..3600u16 {
        let hue = f32::from(tenth) / 10.0;
        let p = tint_push(hue);
        let gap = ((p[1].atan2(p[0]) - theta + PI).rem_euclid(TAU) - PI).abs();
        if gap < best_gap {
            (best, best_gap) = (hue, gap);
        }
    }
    let unit = tint_push(best);
    (best, push[0].hypot(push[1]) / unit[0].hypot(unit[1]))
}

/// One display-encoded pixel under the Color tint: its Oklab (a, b) moved by
/// `push · amount · window(luma)`, its Oklab L untouched, back to linear
/// light clipped at 0 and re-encoded. `push` is [`tint_push`] of the hue and
/// `amount` the 0..1 amount × the mask weight at this pixel — the shift is
/// linear in both, which is what the renders measured — so a soft mask edge
/// tints half as far rather than blending half way toward a tinted copy. No
/// gamut mapping past the clip: Lightroom's exports clip too, and the warm
/// pixels' residual is where that shows. A pixel the window has all but
/// closed on (a highlight past 1.0 in an HDR develop) is returned as it
/// came, so the transfer LUTs' clamp cannot touch it.
pub(crate) fn tint_px(px: [f32; 3], push: [f32; 2], amount: f32) -> [f32; 3] {
    let shift = amount * tint_window(tint_luma(&px));
    if (shift * push[0].hypot(push[1])).abs() < 1e-7 {
        return px;
    }
    let (dec, enc) = transfer_luts();
    let lin = px.map(|v| if (0.0..=1.0).contains(&v) { sample_lut(dec, v) } else { srgb_to_linear(v) });
    let mut lab = linear_to_oklab(lin);
    lab[1] += shift * push[0];
    lab[2] += shift * push[1];
    oklab_to_linear(lab).map(|v| {
        let v = v.max(0.0);
        if v <= 1.0 { sample_lut(enc, v) } else { linear_to_srgb(v) }
    })
}
