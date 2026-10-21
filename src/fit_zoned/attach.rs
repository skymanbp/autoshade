//! Attaching zones: the divergence and attachment records, segmentation of both renditions, and attach_zones with its divergence gate.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct ZoneDivergence {
    /// `None` is [`fit::structure_divergence`]'s abstention, never a matched
    /// reading: the region's eroded core was under the instrument's resolvable
    /// size, so nothing was measured either way.
    pub(super) divergence: Option<fit::Divergence>,
    pub(super) share: f32,
}

#[derive(Clone, Copy)]
pub(super) struct ZoneDivergences {
    pub(super) sky: ZoneDivergence,
    pub(super) land: ZoneDivergence,
}

/// One estimator input owns both populations and the adjustment shape it may
/// emit. Estimator dispatch never depends on [`MaskRole`]: a persisted
/// Custom-role luminance band is as safe to re-fit as a semantic bitmap zone.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ZoneAttachment {
    pub(super) source_weights: Vec<f32>,
    pub(super) target_weights: Vec<f32>,
    /// The population the correction MOVES when it differs from the estimator
    /// weights. `None`: the weights are the coverage (a semantic mask, a
    /// luminance ramp). A tile passes its raster: its estimator weights are
    /// evidence-weighted, so asking the evidence vetoes over them would hide
    /// exactly the withheld pixels the raster still moves.
    pub(super) coverage: Option<ZoneCoverage>,
    pub(super) mask: MaskGeometry,
    pub(super) components: Vec<MaskComponent>,
    pub(super) range: Option<RangeMask>,
    pub(super) name: String,
    pub(super) role: MaskRole,
    pub(super) inverted: bool,
    pub(super) label: String,
    pub(super) min_share: f32,
    pub(super) frame_regression_tol: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ZoneCoverage {
    pub(super) source: Vec<f32>,
    pub(super) target: Vec<f32>,
}

#[cfg(test)]
thread_local! {
    pub(super) static SEGMENT_BOTH_OVERRIDE: std::cell::RefCell<Option<(GrayImage, GrayImage)>> =
        const { std::cell::RefCell::new(None) };
}

/// Structural mode evidence is measured before the global solve. The source
/// segmentation weights are deliberately applied to both images; the target
/// segmentation remains exclusively the moment-matching population.
pub(super) fn measure_zone_divergence(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &crate::recipe::EditRecipe,
    src_mask: &GrayImage,
) -> ZoneDivergences {
    let (sp, tp, w, h) = fit::divergence_raster(src, target, base);
    let sky_weights = mask_weights(src_mask, w, h);
    let land_weights: Vec<f32> = sky_weights.iter().map(|v| 1.0 - v).collect();
    let measured = |weights: &[f32]| ZoneDivergence {
        divergence: fit::structure_divergence(&sp, &tp, w, h, weights),
        share: weights.iter().sum::<f32>() / weights.len().max(1) as f32,
    };
    ZoneDivergences { sky: measured(&sky_weights), land: measured(&land_weights) }
}

/// The long edge past which a frame is thumbnailed before segmentation.
const SEGMENTATION_INPUT_EDGE: u32 = 2048;

/// THE input-sizing rule every segmentation bridge hands the sidecar: native
/// pixels through [`SEGMENTATION_INPUT_EDGE`], otherwise the same thumbnail.
/// Segmentation reads scene SEMANTICS, not pixels: a ≤2048 input finds the
/// sky exactly as well as a 61 MP master while skipping a ~180 MB PNG
/// round-trip per side (the CLI fit hands full-res frames here). The
/// persisted mask raster is normalised-coordinate data — the engine resamples
/// it at whatever resolution the develop runs, and the GUI's own reverse-fit
/// already segments preview-res frames.
///
/// One copy on purpose. `image::thumbnail` has no ratio>1 guard, so an
/// unconditional call UPSCALES a smaller frame; the multi-class bridge once
/// carried its own unconditional copy and its sky plane differed from the
/// single-class mask on every ≤2048 frame — which is what the seeded legacy
/// run's byte identity rests on.
pub(super) fn segmentation_input(img: &DynamicImage) -> std::borrow::Cow<'_, DynamicImage> {
    if img.width().max(img.height()) > SEGMENTATION_INPUT_EDGE {
        std::borrow::Cow::Owned(img.thumbnail(SEGMENTATION_INPUT_EDGE, SEGMENTATION_INPUT_EDGE))
    } else {
        std::borrow::Cow::Borrowed(img)
    }
}

/// Run the segmentation sidecar on both images. The source mask persists at
/// `mask_path` (the recipe references it); the target's inputs/mask are
/// temporary siblings, removed before returning. Any failure aborts the
/// whole zoned attempt — the caller degrades to the global fit.
pub(super) fn segment_both(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
) -> Result<(GrayImage, GrayImage)> {
    #[cfg(test)]
    if let Some(masks) = SEGMENT_BOTH_OVERRIDE.with(|value| value.borrow_mut().take()) {
        return Ok(masks);
    }

    let sibling = |suffix: &str| -> std::path::PathBuf {
        let mut s = mask_path.path().as_os_str().to_owned();
        s.push(suffix);
        s.into()
    };
    let tmp_src = sibling(".src-in.png");
    let tmp_tgt = sibling(".tgt-in.png");
    let tmp_tgt_mask = sibling(".tgt-mask.png");
    // Sizing lives in `segmentation_input` — shared with the multi-class
    // bridge, which is why the seeded legacy run can be byte-identical.
    let src = segmentation_input(src);
    let target = segmentation_input(target);
    let run = || -> Result<(GrayImage, GrayImage)> {
        src.to_rgb8().save(&tmp_src).context("write segmentation input (source)")?;
        target.to_rgb8().save(&tmp_tgt).context("write segmentation input (target)")?;
        segment_file(seg, &tmp_src, mask_path.path()).context("segment source sky")?;
        segment_file(seg, &tmp_tgt, &tmp_tgt_mask).context("segment target sky")?;
        let sm = crate::render::open_mask_bounded(mask_path.path())
            .context("read source sky mask")?
            .to_luma8();
        let tm = crate::render::open_mask_bounded(&tmp_tgt_mask)
            .context("read target sky mask")?
            .to_luma8();
        Ok((sm, tm))
    };
    let out = run();
    for p in [&tmp_src, &tmp_tgt, &tmp_tgt_mask] {
        std::fs::remove_file(p).ok();
    }
    if out.is_err() {
        // The source segmentation writes mask_path FIRST — a later failure
        // (target segmentation, mask decode) otherwise leaves a partial mask
        // file no zone will ever reference (the caller degrades to the global
        // fit). Removing it also releases a claimed unique raster name.
        mask_path.remove();
    }
    out
}

/// The carrier the sky and land zones ride — Lightroom's own Select Sky
/// component over the claimed raster, referenced at the alpha's centre of
/// mass ([`raster_centroid`]) — built in ONE place so the tone zones and
/// the detail stage's masks are the same geometry to the byte. The land
/// zone is this component with `LocalAdjustment::inverted`, never a second
/// component (see the inversion note in [`attach_zones_with_divergence`]).
pub(super) fn sky_land_geometry(
    src_mask: &GrayImage,
    mask_path: &crate::store::OwnedRaster,
) -> MaskGeometry {
    let zone_raster = mask_path.path().to_string_lossy().into_owned();
    let (ref_x, ref_y) = raster_centroid(src_mask);
    MaskGeometry::select_sky(ref_x, ref_y, false, zone_raster)
}

/// The post-segmentation half (separable so tests drive it with hand-built
/// masks, no python): correct the SKY zone, then the LAND zone — the same
/// raster reused with `inverted = true`, so one segmentation buys the whole
/// frame (the first real-pair render showed why land is not optional: the
/// distant haze-terrain outside the sky mask kept its global-fit blue and
/// clashed against the repainted gold sky as a hard halo). Each zone is
/// gated independently; the raster file is removed only when NO zone kept
/// it. Requires a VALID sky partition first: an empty/degenerate sky mask
/// makes "land" mean "everything", which would just be a weaker-gated
/// re-run of the global fit.
#[cfg(test)]
pub(super) fn attach_zones(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    src_mask: &GrayImage,
    tgt_mask: &GrayImage,
    mask_path: &crate::store::OwnedRaster,
) {
    let divergence = measure_zone_divergence(
        src,
        target,
        &crate::recipe::EditRecipe::default(),
        src_mask,
    );
    attach_zones_with_divergence(
        src,
        target,
        report,
        src_mask,
        tgt_mask,
        mask_path,
        divergence,
    );
}

/// Returns whether the sky/land PARTITION held — false on the two exits
/// that judge the split itself invalid (`ZONED_NO_PARTITION`,
/// `ZONE_SHARE_MISMATCH`), which drop the anchor raster and attach no zone;
/// true when the partition was judged and the zones were tried, whatever
/// their tone verdict. The detail stage reads sky/land zones only from a
/// partition that held (a 0% sky once handed it a "land" of the whole frame
/// and the anchor raster outlived the failed partition, 2026-10-21).
#[allow(clippy::too_many_arguments)]
pub(super) fn attach_zones_with_divergence(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    src_mask: &GrayImage,
    tgt_mask: &GrayImage,
    mask_path: &crate::store::OwnedRaster,
    divergence: ZoneDivergences,
) -> bool {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    // This render was already being made and thrown away for its dimensions
    // alone. It is ALSO the boundary gate k=0 baseline: `first_zone` is
    // `masks.len()` as of right here (every rejection path inside
    // `attach_one_zone` pops its own mask again), so nothing between this
    // line and the gate can change the recipe it was rendered from. Binding
    // it is what makes the differential rim cost zero extra renders.
    let base = render::develop_preview(&s_img, &report.recipe);
    let (aw, ah) = (base.width(), base.height());
    let reference_masks = report.recipe.masks.len();
    let reference_px = fit::pixels_of(&base);
    let sw = mask_weights(src_mask, aw, ah);
    let tw = mask_weights(tgt_mask, t_img.width(), t_img.height());
    // Partition validity — judged on the raw mask shares (Σw/n), before any
    // zone-specific gating.
    let share = |w: &[f32]| w.iter().sum::<f32>() / w.len().max(1) as f32;
    let (s_share, t_share) = (share(&sw), share(&tw));
    if !(MIN_ZONE_SHARE..=1.0 - MIN_ZONE_SHARE).contains(&s_share)
        || !(MIN_ZONE_SHARE..=1.0 - MIN_ZONE_SHARE).contains(&t_share)
    {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONED_NO_PARTITION,
                vec![
                    ("s", format!("{:.0}", s_share * 100.0)),
                    ("t", format!("{:.0}", t_share * 100.0)),
                ],
            ),
        );
        mask_path.remove();
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(
            &mut *report,
            &finished,
            &tgt_px,
        );
        return false;
    }
    let (lo, hi) = (s_share.min(t_share), s_share.max(t_share));
    if hi > 2.0 * lo {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_SHARE_MISMATCH,
                vec![
                    ("label", "frame".to_string()),
                    ("s", format!("{:.0}", s_share * 100.0)),
                    ("t", format!("{:.0}", t_share * 100.0)),
                ],
            ),
        );
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::plain(crate::rationale::keys::ZONE_SHARE_NO_CORRECTION),
        );
        mask_path.remove();
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(report, &finished, &tgt_px);
        return false;
    }
    let swl: Vec<f32> = sw.iter().map(|w| 1.0 - w).collect();
    let twl: Vec<f32> = tw.iter().map(|w| 1.0 - w).collect();
    // The running FRAME-GLOBAL error, threaded through the zone passes for
    // the bounded-drift insurance ONLY (R23-6 honesty fix). It used to be
    // `report.err_after` itself, read and overwritten in place — which made
    // the number handed to the user, and the confidence derived from it, the
    // very frame-global metric this module's own [`ZONE_ACCEPT_RATIO`] doc
    // proves cannot judge a zone (0.507 → 0.015 zone-local while the frame
    // moved 0.1768 → 0.1792). Drift still has to be measured incrementally,
    // so the value still has to be carried; it is simply no longer the same
    // variable as the report's verdict.
    let mut frame_err = report.err_after;
    // Taken, not borrowed: each zone needs the field WHILE holding the
    // report mutably; restored below so the report keeps carrying it.
    let corr = report.correspondence.take();
    // THE SKY AND LAND ZONES RIDE OUT AS LIGHTROOM'S OWN SELECT SKY.
    //
    // They used to be `MaskGeometry::Bitmap`, which classic ACR XMP has no
    // encoding for at all: the writer skipped both corrections with a named
    // `MaskLossReason::Bitmap`, so the sidecar carried the global fit and the
    // two edits that actually separate a repainted sky from its ground stayed
    // inside this app. A `Mask/Image` component with `crs:MaskSubType="2"` is
    // the same INTENT in a form Lightroom reads, and it is honest in both
    // directions: Lightroom rebuilds its own sky alpha from the component, and
    // the pixels shown here are the ones AutoShade rendered — never Adobe's
    // raster. `MaskLossReason::AiMaskRecomputed` is the sentence that says so,
    // and it replaces the `Bitmap` loss rather than joining it.
    //
    // Everything else is deliberately unchanged: the same claimed PNG (ONE
    // file for both zones, as before), the same roles, the same gates, the
    // same weights. Only the carrier moved.
    //
    // The reference point is the alpha's own centre of mass. `crs:ReferencePoint`
    // is the photographer's click on 105/105 measured instances and the fit has
    // no click to carry, so it hands over the one point that is inside this sky
    // by construction. Both zones name the SAME point: the land zone is this
    // sky inverted, not a second segmentation.
    let geometry = sky_land_geometry(src_mask, mask_path);
    let sky_attachment = ZoneAttachment {
        source_weights: sw.clone(),
        target_weights: tw.clone(),
        coverage: None,
        mask: geometry.clone(),
        components: Vec::new(),
        range: None,
        name: String::new(),
        role: MaskRole::ZoneSky,
        inverted: false,
        label: MaskRole::ZoneSky.tag().to_string(),
        min_share: MIN_ZONE_SHARE,
        frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
    };
    // ONE HOME FOR THE INVERSION, and it is `LocalAdjustment::inverted` — the
    // flag this engine's weight loop applies and every zone gate already
    // reads. The COMPONENT's own bit stays `false`, so
    // `LocalAdjustment::net_inverted` is `true` exactly once: the render
    // inverts once and `ai_mask_xml` writes `crs:MaskInverted="true"` once.
    // Spelling it in both places would have been two homes for one fact, and
    // the moment the AI arm of `mask_weight` learned to read the geometry's
    // bit (it does now) the land zone would have inverted twice and covered
    // the sky. Re-importing our own sidecar lands the bit in the OTHER home —
    // the parser puts a component's `crs:MaskInverted` inside the geometry —
    // and the net is unchanged, which is what makes the round trip render
    // byte-identically.
    let land_attachment = ZoneAttachment {
        source_weights: swl,
        target_weights: twl,
        coverage: None,
        mask: geometry,
        components: Vec::new(),
        range: None,
        name: String::new(),
        role: MaskRole::ZoneLand,
        inverted: true,
        label: MaskRole::ZoneLand.tag().to_string(),
        min_share: MIN_ZONE_SHARE,
        frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
    };
    let sky = attach_one_zone(
        &s_img,
        &tgt_px,
        report,
        &mut frame_err,
        &sky_attachment,
        divergence.sky.divergence,
        corr.as_ref(),
    );
    let land = attach_one_zone(
        &s_img,
        &tgt_px,
        report,
        &mut frame_err,
        &land_attachment,
        divergence.land.divergence,
        corr.as_ref(),
    );
    report.correspondence = corr;
    let mut accepted: Vec<AcceptedZone> = [sky, land].into_iter().flatten().collect();
    if accepted.is_empty() {
        mask_path.remove();
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(
            report,
            &finished,
            &tgt_px,
        );
        return true;
    }
    let first_zone = report.recipe.masks.len() - accepted.len();
    debug_assert_eq!(
        first_zone, reference_masks,
        "the bound reference is the k=0 baseline only while no other mask joined"
    );
    // R39. The k=1 controls of every accepted zone, kept so that when the
    // SHRUNK set no longer satisfies a zone's own acceptance
    // (`refuse_shrunk_zones`) that zone is dropped and the survivors are
    // gated again from their full-strength controls, not from a shrink that
    // was negotiated for a set they are no longer part of.
    let mut originals = report.recipe.masks[first_zone..].to_vec();
    let entry_frame = fit::look_err_with_evidence(&reference_px, &tgt_px, &report.evidence);
    let mut initial_px = accepted.last().expect("at least one accepted zone").rendered.clone();
    let final_px = loop {
        let correction_shares = accepted
            .iter()
            .map(|zone| {
                zone.mask_weights.iter().sum::<f32>()
                    / zone.mask_weights.len().max(1) as f32
            })
            .collect::<Vec<_>>();
        let (k, pixels) = match enforce_boundary_gate_toward(
            Some(&tgt_px[..]),
            &s_img,
            report,
            &sw,
            &correction_shares,
            first_zone,
            &reference_px,
            initial_px,
        ) {
            BoundaryGateResult::Kept { k, before, after, pixels } => {
                debug_assert!((0.0..=1.0).contains(&k));
                debug_assert!(before.rim.is_finite() && after.rim.is_finite());
                (k, pixels)
            }
            BoundaryGateResult::Dropped => {
                mask_path.remove();
                let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
                fit::append_finished_disclosure(
                    report,
                    &finished,
                    &tgt_px,
                );
                return true;
            }
        };
        let frame_after = fit::look_err_with_evidence(&pixels, &tgt_px, &report.evidence);
        if !refuse_shrunk_zones(
            &s_img, report, &mut accepted, &mut originals, first_zone, &pixels, &tgt_px, k,
            entry_frame, frame_after,
        ) {
            break pixels;
        }
        report.recipe.masks.truncate(first_zone);
        if accepted.is_empty() {
            mask_path.remove();
            let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
            fit::append_finished_disclosure(
                report,
                &finished,
                &tgt_px,
            );
            return true;
        }
        report.recipe.masks.extend(originals.iter().cloned());
        for (j, zone) in accepted.iter_mut().enumerate() {
            zone.mask_index = first_zone + j;
        }
        initial_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    };
    // The boundary shrink changes the actual zone landings, so the attached
    // notes and confidence are measured again from the kept render rather
    // than repeating the pre-gate candidate's dials/residuals.
    for zone in &mut accepted {
        let after = zone_moments(&final_px, &zone.source_weights);
        let target = zone_moments(&tgt_px, &zone.target_weights);
        zone.after = zone_err(&after, &target);
        push_zone_attached_note(report, zone);
    }
    frame_err = fit::look_err_with_evidence(&final_px, &tgt_px, &report.evidence);
    // `err_after` keeps its CONTRACT — the frame-global look distance of the
    // recipe that actually ships, in the same unit as `err_before`, which is
    // what every printout pairs it with. What changes is that it no longer
    // doubles as the zone stage's verdict.
    report.err_after = frame_err;
    // CONFIDENCE, on the other hand, is a verdict, and it now comes from what
    // was actually judged: the WORST accepted zone's own residual, on the
    // zone scale, floored against the global stage's own claim so neither
    // stage can promise what the other did not deliver. Worst, not
    // area-weighted: a perfectly matched sky over a wrecked foreground is not
    // a 70%-confident fit, and the zones are few and large enough that the
    // worst one is never a sliver.
    let worst = semantic::worst_region_residual(&accepted.iter().map(|z| z.after).collect::<Vec<_>>());
    let zone_conf = fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE);
    report.recipe.confidence = report.recipe.confidence.min(zone_conf);
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::ZONE_CONFIDENCE,
            vec![
                ("n", accepted.len().to_string()),
                ("worst", format!("{worst:.3}")),
                ("frame", format!("{frame_err:.3}")),
            ],
        ),
    );
    subzones::replace_zone(&s_img, &tgt_px, report, &sky_attachment, divergence.sky.divergence);
    subzones::replace_zone(&s_img, &tgt_px, report, &land_attachment, divergence.land.divergence);
    let final_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    fit::append_finished_disclosure(
        report,
        &final_px,
        &tgt_px,
    );
    true
}
