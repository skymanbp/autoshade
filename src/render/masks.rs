//! Local adjustments: the mask pass, the frame a mask is sampled in, Lightroom's stored-frame unwarp, and the engine's activity rule.

use super::*;

/// Apply each local masked adjustment: blend the masked region toward a locally
/// re-adjusted version, weighted by the gradient mask × amount. Mask coords are
/// normalised so this works at any resolution.
///
/// Per mask, in pass order: local **dehaze** → the fused local **WB**
/// (temperature/tint — the same [`wb_gains`] model as the global stage, see
/// [`local_temp_to_kelvin`]) + **tone** (exposure/contrast/highlights/shadows/
/// whites/blacks) + **saturation** + **hue** pass → local **clarity** → local
/// **texture** → local **sharpness** → local **noise reduction**. The last two
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
pub(super) fn apply_masks(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
    film: FilmScale,
) {
    if w == 0 || h == 0 {
        return; // both passes below chunk by w; rayon asserts chunk_size != 0
    }
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
    let dehaze_a = r
        .masks
        .iter()
        .any(|m| m.enabled && m.amount.clamp(0.0, 1.0) != 0.0 && m.dehaze != 0.0)
        .then(|| dehaze_airlight(data, w));
    for stored_mask in &r.masks {
        // The eye toggle: a disabled mask renders nothing at any Amount —
        // the lossless mute (recipe.rs `LocalAdjustment::enabled`).
        if !stored_mask.enabled {
            continue;
        }
        // H2's only geometry rewrite happens here, once for the base plus once
        // per LINEAR component. The pixel closures below see one reconstructed
        // straight raw-frame gradient and never call the camera map.
        let framed_mask =
            frame.linear_handles_to_raw(stored_mask, (w as f32, h as f32));
        let m = framed_mask.as_ref();
        let local = EditRecipe {
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
        };
        let lut = build_tone_lut(&local);
        // The three per-channel local curves (`crs:{Red,Green,Blue}Curve`),
        // compiled ONCE per mask like `colour_luts` below and applied inside
        // the fused pixel loop right after the master curve — the global
        // chain's own order (`apply_develop` stage 1 then 1b). `None` when all
        // three are empty, so a curve-free mask pays nothing.
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
        let colour_luts =
            (m.temperature != 0.0 || m.tint != 0.0 || m.color_gains.is_some()).then(|| {
                let g = wb_gains(5500.0, local_temp_to_kelvin(m.temperature), m.tint);
                let cg = m.color_gains.unwrap_or([1.0; 3]);
                colour_gain_luts([g[0] * cg[0], g[1] * cg[1], g[2] * cg[2]])
            });
        let amount = m.amount.clamp(0.0, 1.0);
        // Amount 0 zeroes every weight below (inverted or not) — skip the
        // full-frame tone scan and a possible NR blur that would all be
        // multiplied away (a real cost at 61 MP for a merely parked mask).
        if amount == 0.0 {
            continue;
        }
        // Bitmap geometry: decode each raster ONCE per mask per develop
        // (never inside the pixel loop); both the tone and the NR pass share
        // them. Components load alongside the base.
        //
        // A BRUSH group arrives through the same slot (R29 Batch-6b). It has
        // no file, so the snapshot has nothing for it; `brush_raster` stamps
        // its dab stream instead — once per mask per develop, memoised across
        // develops, and at THIS frame's size because a dab is a circle in
        // pixels. The two sources cannot collide: `rasters.get` answers only
        // for a geometry with a raster PATH (`geometry_raster_path`), which a
        // brush never has.
        let brush_base = brush_raster(&m.mask, w as u32, h as u32);
        let brush_comps: Vec<Option<std::sync::Arc<image::GrayImage>>> =
            m.components.iter().map(|c| brush_raster(&c.geometry, w as u32, h as u32)).collect();
        let bmp = rasters.get(&m.mask).or(brush_base.as_deref());
        let comp_bmps: Vec<Option<&image::GrayImage>> = m
            .components
            .iter()
            .zip(&brush_comps)
            .map(|(c, brush)| rasters.get(&c.geometry).or(brush.as_deref()))
            .collect();
        // An unloadable raster carries NO coverage, so its weight must never
        // reach the inversion below: 0 with `inverted` would apply this
        // adjustment to the WHOLE frame at full strength. Skipping the whole
        // adjustment is the inert contract (recipe.rs `MaskGeometry::Bitmap`)
        // — and it covers COMPONENTS for the same reason: a lost Subtract
        // raster contributes 0 and silently WIDENS the effect area.
        if (bmp.is_none() && is_raster_backed(&m.mask))
            || m.components
                .iter()
                .zip(&comp_bmps)
                .any(|(c, b)| b.is_none() && is_raster_backed(&c.geometry))
        {
            continue;
        }
        // combined mask coverage × master amount at a pixel (with inversion).
        //
        // PIXEL CENTRES, `(x + 0.5)/w` — see `MASK_SAMPLE_CENTRE` for the
        // measurement, the derivation, and the render-behaviour change.
        let weight_at = |x: usize, y: usize| -> f32 {
            let (nx, ny) = (
                (x as f32 + MASK_SAMPLE_CENTRE) / w as f32,
                (y as f32 + MASK_SAMPLE_CENTRE) / h as f32,
            );
            let mut wgt = combined_mask_weight(
                m,
                nx,
                ny,
                bmp,
                &comp_bmps,
                unwarp,
                (w as f32, h as f32),
            );
            if m.inverted {
                wgt = 1.0 - wgt;
            }
            wgt * amount
        };

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
        let tone_identity = m.exposure_ev == 0.0
            && m.contrast == 0.0
            && m.highlights == 0.0
            && m.shadows == 0.0
            && m.whites == 0.0
            && m.blacks == 0.0
            && m.saturation == 0.0
            && m.hue == 0.0
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
                let l_old = luma601(&t);
                let l_new = sample_lut(&lut, l_old);
                scale_chroma(&mut t, l_old, l_new);
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
                if let Some((luts, active)) = &rgb_curve_luts {
                    for ch in 0..3 {
                        if active[ch] {
                            t[ch] = sample_lut(&luts[ch], t[ch]);
                        }
                    }
                }
                let mut t = apply_sat_vibrance(t[0], t[1], t[2], sat, 0.0);
                // Local hue rotation, LAST in the fused transform: it turns the
                // colour this mask's WB/tone/saturation stages produced, which
                // is the order the sliders read in (Temp shift → Saturation →
                // Hue). Blended by the same single weight as the rest.
                if hue_turns != 0.0 {
                    let (hh, ss, ll) = rgb_to_hsl(t[0], t[1], t[2]);
                    let (r2, g2, b2) = hsl_to_rgb((hh + hue_turns).rem_euclid(1.0), ss, ll);
                    t = [r2, g2, b2];
                }
                for c in 0..3 {
                    out_px[c] = p[c] * (1.0 - wgt) + t[c] * wgt;
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
            texture_pass(data, w, h, m.texture / 100.0, spatial_weight);
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

/// WHERE a mask chain's output will be sampled — the value that keeps
/// parametric mask geometry and the pixel warp travelling together.
///
/// # The defect this closes (R29 Batch-3, user ruling 2026-08-20)
///
/// Lightroom stores mask geometry in different frames and, for LINEAR, uses a
/// different transport topology. Brush dabs live PRE-lens-correction. RADIAL
/// points live POST-correction. LINEAR stores two corrected-frame handles, then
/// either evaluates the reconstructed straight gradient in that corrected frame
/// or transports only those handles to the raw frame and reconstructs it there.
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
///   pointwise forward warp. With downstream geometry it is sampled only at
///   `lens_ungeom_norm(p)`; without downstream geometry its two handles are
///   mapped once by `lr_mask_unwarp_norm` and one straight raw-frame gradient
///   is rebuilt in the raw pixel metric.
///
/// RADIAL maps each sample point first through the inverse description of where
/// engine geometry will put it, then through the inverse Lightroom mask
/// transport. [`MaskUnwarp::at`] is that exact-once composition. LINEAR uses
/// [`MaskUnwarp::engine_at`] for the first half only, or the handle-only path
/// carried by [`MaskFrame::LinearHandlesToRaw`].
///
/// **Precision disclosure (D2 LINEAR, 2026-08-24): this is not 1 px closed.**
/// Against the three wall contours, the active/corrected arm's stored-line RMS
/// residual is 9.748/7.025/6.336 px; the inactive/raw H2 arm's absolute RMS is
/// 12.449/9.943/4.979 px. A fitted anisotropic aspect term is diagnostic only
/// and is deliberately not implemented here.
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
/// [`MaskFrame::without_downstream`], which keeps RADIAL off the engine sampler
/// while still carrying LINEAR's separate handle fact.
///
/// So the invariant is stated as a value: **whoever runs the geometry stage
/// builds this from the same profile and amount it will pass to
/// [`apply_lens_geometry`], and uses [`MaskFrame::warps`] to decide whether to
/// run it at all; whoever omits it passes the profile to
/// [`MaskFrame::without_downstream`].**
#[derive(Clone, Copy, Debug)]
pub enum MaskFrame<'a> {
    /// The caller WILL resample this buffer through [`apply_lens_geometry`]
    /// with exactly this profile and manual amount.
    WarpedDownstream { profile: &'a crate::recipe::LensProfile, amount: f32 },
    /// No geometry resample follows, but a camera map is available for LINEAR's
    /// corrections-off rule. Only its two handles take this map; RADIAL and all
    /// raster geometry remain at stored coordinates.
    LinearHandlesToRaw { profile: &'a crate::recipe::LensProfile },
    /// Nothing downstream moves these pixels: the mask chain's output IS what
    /// will be looked at, so every geometry is evaluated at its stored
    /// coordinates.
    AsRendered,
}

impl<'a> MaskFrame<'a> {
    /// What a caller that runs the geometry stage passes.
    ///
    /// Answers [`MaskFrame::LinearHandlesToRaw`] when no resample follows but a
    /// camera map is available, and [`MaskFrame::AsRendered`] when neither fact
    /// exists. The SAME geometry condition gates `apply_lens_geometry`.
    pub fn downstream(profile: &'a crate::recipe::LensProfile, amount: f32) -> Self {
        if profile.geometry_active() || amount != 0.0 {
            MaskFrame::WarpedDownstream { profile, amount }
        } else {
            Self::without_downstream(profile)
        }
    }

    /// What a caller that deliberately omits the geometry stage passes. RADIAL
    /// and raster geometry stay stored; LINEAR retains its H2 handle transport
    /// whenever the profile carries a solved camera map.
    pub fn without_downstream(profile: &'a crate::recipe::LensProfile) -> Self {
        if profile.linear_handle_warp().len() >= 2 {
            MaskFrame::LinearHandlesToRaw { profile }
        } else {
            MaskFrame::AsRendered
        }
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
            MaskFrame::LinearHandlesToRaw { .. } | MaskFrame::AsRendered => None,
        }
    }

    /// Apply LINEAR's corrections-off H2 rule once per local adjustment. The
    /// returned owned value exists only when at least one base/component LINEAR
    /// geometry was transported; every pixel then evaluates the resulting
    /// straight gradient with no camera map in its sample path.
    pub(super) fn linear_handles_to_raw<'b>(
        self,
        mask: &'b crate::recipe::LocalAdjustment,
        dims: (f32, f32),
    ) -> Cow<'b, crate::recipe::LocalAdjustment> {
        let MaskFrame::LinearHandlesToRaw { profile } = self else {
            return Cow::Borrowed(mask);
        };
        let knots = profile.linear_handle_warp();
        if knots.len() < 2 {
            return Cow::Borrowed(mask);
        }
        let mut out = mask.clone();
        let mut moved = transport_linear_handles(&mut out.mask, dims, profile, knots);
        for component in &mut out.components {
            moved |= transport_linear_handles(&mut component.geometry, dims, profile, knots);
        }
        if moved { Cow::Owned(out) } else { Cow::Borrowed(mask) }
    }
}

/// Map a LINEAR component's two handles in the camera map's forward direction
/// (`D_fwd` / `lr_mask_unwarp_norm`) and nothing else. Returning `true` lets the
/// caller avoid cloning adjustments with no LINEAR geometry.
fn transport_linear_handles(
    geometry: &mut MaskGeometry,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    knots: &[f32],
) -> bool {
    let (MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }
        | MaskGeometry::Shaped { zero_x, zero_y, full_x, full_y, .. }) = geometry
    else {
        return false;
    };
    (*zero_x, *zero_y) = linear_handle_unwarp_norm(*zero_x, *zero_y, dims, profile, knots);
    (*full_x, *full_y) = linear_handle_unwarp_norm(*full_x, *full_y, dims, profile, knots);
    true
}

/// RADIAL-only: ORIGINAL-frame point → the Lightroom-stored sample point
/// whose effect will occupy the right Lightroom export point after this
/// engine's geometry stage.
///
/// This is the composition `m_lr^-1(T_engine(p))`: first ask where original
/// pixel `p` will land under the exact downstream engine resample, then pull
/// that output coordinate back through Lightroom's corrected mask transport.
/// `T_engine` is built by calling [`lens_ungeom_norm`], not by reimplementing
/// it. `m_lr^-1` is built by calling [`lr_mask_unwarp_norm`]. Each appears
/// exactly once; the downstream resample supplies the corresponding one
/// `T_engine` application after mask rasterisation.
///
/// **The manual `lens_distortion` amount is covered with no residue** in the
/// first half. The second half deliberately uses the Lightroom mask map, not
/// the engine map a second time: D2's 41-vector radial fixture closes this
/// point law. LINEAR, brush, bitmap and AI never call this full adapter; the
/// explicit match in `mask_weight_in` is the type boundary.
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
    /// Factor `r_stored / r_exported` for Lightroom's mask transport, sampled
    /// about `lr_cx,lr_cy`. `None` is Lightroom identity.
    lr_lut: Option<Vec<f32>>,
    rr: f32,
    w: f32,
    h: f32,
    lr_cx: f32,
    lr_cy: f32,
    lr_rmax: f32,
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
        let [lr_cx, lr_cy] = lr_mask_center_px(dims, profile);
        let lr_rmax = [
            lr_cx.hypot(lr_cy),
            (w - lr_cx).hypot(lr_cy),
            lr_cx.hypot(h - lr_cy),
            (w - lr_cx).hypot(h - lr_cy),
        ]
        .into_iter()
        .fold(1.0f32, f32::max)
            / rr;
        let lr_lut = if profile.mask_warp.is_empty() {
            None
        } else {
            let mut lr_lut: Vec<f32> = (0..LUT_N)
                .map(|i| {
                    let rho = lr_rmax * i as f32 / (LUT_N - 1) as f32;
                    let dx = rho * rr;
                    if dx <= 1e-6 {
                        return f32::NAN;
                    }
                    let (sx, _) = lr_mask_unwarp_norm(
                        (lr_cx + dx) / w,
                        lr_cy / h,
                        dims,
                        profile,
                    );
                    (sx * w - lr_cx) / dx
                })
                .collect();
            lr_lut[0] = lr_lut[1];
            Some(lr_lut)
        };
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
        let engine_identity = lut.iter().all(|f| (f - 1.0).abs() * rr < 0.01);
        let lr_identity = lr_lut
            .as_ref()
            .is_none_or(|v| v.iter().all(|f| (f - 1.0).abs() * rr < 0.01));
        if engine_identity && lr_identity {
            return None;
        }
        Some(MaskUnwarp { lut, lr_lut, rr, w, h, lr_cx, lr_cy, lr_rmax })
    }

    /// Exact inverse of the downstream engine geometry, without Lightroom's
    /// point-transport half. LINEAR uses this arm so its stored straight line
    /// lands in the corrected output frame without acquiring RADIAL's map.
    pub(super) fn engine_at(&self, nx: f32, ny: f32) -> (f32, f32) {
        let (dx, dy) = ((nx - 0.5) * self.w, (ny - 0.5) * self.h);
        let rho = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, 1.0);
        let t = rho * (LUT_N - 1) as f32;
        let i = (t.floor() as usize).min(LUT_N - 2);
        let f = t - i as f32;
        let k = self.lut[i] * (1.0 - f) + self.lut[i + 1] * f;
        ((dx * k) / self.w + 0.5, (dy * k) / self.h + 0.5)
    }

    /// The point `(nx, ny)` will occupy after the geometry stage. RADIAL's
    /// settled point law remains byte-for-byte on this method; LINEAR calls the
    /// separate engine-only half above.
    pub(super) fn at(&self, nx: f32, ny: f32) -> (f32, f32) {
        let (dx, dy) = ((nx - 0.5) * self.w, (ny - 0.5) * self.h);
        let rho = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, 1.0);
        let t = rho * (LUT_N - 1) as f32;
        let i = (t.floor() as usize).min(LUT_N - 2);
        let f = t - i as f32;
        let k = self.lut[i] * (1.0 - f) + self.lut[i + 1] * f;
        let (nx, ny) = ((dx * k) / self.w + 0.5, (dy * k) / self.h + 0.5);
        let Some(lr_lut) = &self.lr_lut else { return (nx, ny) };
        let (dx, dy) = (nx * self.w - self.lr_cx, ny * self.h - self.lr_cy);
        let rho = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, self.lr_rmax);
        let t = rho / self.lr_rmax * (LUT_N - 1) as f32;
        let i = (t.floor() as usize).min(LUT_N - 2);
        let f = t - i as f32;
        let k = lr_lut[i] * (1.0 - f) + lr_lut[i + 1] * f;
        (
            (dx * k + self.lr_cx) / self.w,
            (dy * k + self.lr_cy) / self.h,
        )
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
