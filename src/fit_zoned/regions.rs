//! Semantic regions: multi-class segmentation of both renditions and attaching the regions.

use super::*;

/// Run the one-inference-per-frame multi-class sidecar and materialise source
/// planes as owned rasters for the recipe. Temporary manifests and sidecar
/// plane files are removed before returning; the recipe owns only the claimed
/// source rasters it actually receives. The refinement readings ride along so
/// the caller can disclose them once it has a report to disclose into.
type MultiClassSegments = (
    Vec<semantic::SemanticRegion>,
    Vec<crate::store::OwnedRaster>,
    (GrayImage, GrayImage),
    Vec<PlaneRefinement>,
    Vec<FeatherWidening>,
    Vec<EdgeMatte>,
);

pub(super) fn segment_multiclass_both(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    max_regions: usize,
) -> Result<MultiClassSegments> {
    let sibling = |suffix: &str| {
        let mut p = mask_path.path().as_os_str().to_owned();
        p.push(suffix);
        std::path::PathBuf::from(p)
    };
    let src_in = sibling(".multi-src.png");
    let tgt_in = sibling(".multi-tgt.png");
    let src_manifest = sibling(".multi-src.json");
    let tgt_manifest = sibling(".multi-tgt.json");
    let run = (|| -> Result<MultiClassSegments> {
        // The single-class bridge's sizing, through the ONE helper both
        // bridges share: the seeded legacy run is byte-identical to an
        // unseeded one only if the sidecar saw identical inputs.
        let source_input = segmentation_input(src);
        let target_input = segmentation_input(target);
        source_input.to_rgb8().save(&src_in).context("write multi-class source input")?;
        target_input.to_rgb8().save(&tgt_in).context("write multi-class target input")?;
        let sm = crate::segment::segment_multiclass_file(seg, &src_in, &src_manifest, max_regions)?;
        let tm = crate::segment::segment_multiclass_file(seg, &tgt_in, &tgt_manifest, max_regions)?;
        let source_sky = sm
            .planes
            .iter()
            .find(|plane| plane.label.trim().eq_ignore_ascii_case("sky"))
            .map(|plane| plane.mask.clone())
            .context("multi-class source manifest has no sky plane")?;
        let target_sky = tm
            .planes
            .iter()
            .find(|plane| plane.label.trim().eq_ignore_ascii_case("sky"))
            .map(|plane| plane.mask.clone())
            .context("multi-class target manifest has no sky plane")?;
        // Keep the historical anchor valid for the seeded legacy run. The
        // legacy path may abstain from refinement and still reference this
        // raster, so it must be materialised before returning the pair.
        source_sky.save(mask_path.path()).context("save seeded legacy sky mask")?;
        let source_dims = (sm.width, sm.height);
        let mut source = sm.planes.into_iter().map(|p| semantic::ClassPlane {
            class_id: p.class_id, label: p.label, mean_confidence: p.mean_confidence, mask: p.mask,
        }).collect::<Vec<_>>();
        let mut target_planes = tm.planes.into_iter().map(|p| semantic::ClassPlane {
            class_id: p.class_id,
            label: p.label,
            mean_confidence: p.mean_confidence,
            // Segmentation runs independently on each input, so their native
            // raster sizes can differ by a row/column.  The fit's evidence
            // geometry is source-owned; resample the target plane into it
            // before pairing counterparts.
            mask: if p.mask.dimensions() == source_dims {
                p.mask
            } else {
                image::imageops::resize(&p.mask, source_dims.0, source_dims.1, image::imageops::FilterType::Triangle)
            },
        }).collect::<Vec<_>>();
        // The shipped sky/land producer refines both semantic planes before
        // fitting. Apply the same bounded guide to every class before overlap
        // resolution, so the disjoint partition is built from the alphas the
        // renderer will actually persist rather than from a second, rougher
        // semantic path.
        let mut refinements: Vec<PlaneRefinement> = Vec::new();
        let mut widenings: Vec<FeatherWidening> = Vec::new();
        let mut mattes: Vec<EdgeMatte> = Vec::new();
        for (side, frame, planes) in [("source", src, &mut source), ("target", target, &mut target_planes)] {
            for plane in planes.iter_mut() {
                let label = format!("semantic {side} class {} {}", plane.class_id, plane.label);
                match crate::mask_refine::guided_refine(
                    frame,
                    &plane.mask,
                    MASK_REFINE_RADIUS,
                    MASK_REFINE_EPSILON,
                ) {
                    crate::mask_refine::RefineOutcome::Kept { mask, reading } => {
                        plane.mask = mask;
                        refinements.push((label.clone(), true, reading));
                    }
                    crate::mask_refine::RefineOutcome::Abstained { reading } => {
                        refinements.push((label.clone(), false, reading));
                    }
                }
                // SOURCE planes only, and before `resolve_regions`, for the
                // reason the refinement above gives: the disjoint partition
                // must be built from the alphas the renderer will persist.
                // The region raster is claimed and written further down from
                // exactly these bytes, so nothing here needs its own save.
                // The matte first: it settles the ramp where the scene steps,
                // and the widener then broadens it only where the scene does
                // not — the two read complementary halves of one contour.
                if side == "source" {
                    mattes.push(matte_source_edge(frame, &mut plane.mask, &label));
                    match crate::mask_refine::widen_smooth_feather(frame, &plane.mask) {
                        crate::mask_refine::WidenOutcome::Widened { mask, reading } => {
                            plane.mask = mask;
                            widenings.push((label, true, reading));
                        }
                        crate::mask_refine::WidenOutcome::Abstained { reading } => {
                            widenings.push((label, false, reading));
                        }
                    }
                }
            }
        }
        let regions = semantic::resolve_regions(&source, &target_planes, max_regions);
        if let Some(first) = regions.first()
            && !semantic::bitmap_budget_allows(
                0,
                first.source.width(),
                first.source.height(),
                regions.len(),
            )
        {
            anyhow::bail!(
                "semantic region bitmap budget refused {} regions at {}x{}",
                regions.len(), first.source.width(), first.source.height()
            );
        }
        let mut rasters: Vec<crate::store::OwnedRaster> = Vec::with_capacity(regions.len());
        for region in &regions {
            let safe = region.label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>();
            let prefix = format!("mask-region-{}-{}", region.class_id, safe.trim_matches('-'));
            let raster = match mask_path.claim_sibling(&prefix) {
                Ok(raster) => raster,
                Err(e) => {
                    for claimed in &rasters { claimed.remove(); }
                    return Err(e).with_context(|| format!("claim semantic region {}", region.class_id));
                }
            };
            if let Err(e) = region.source.save(raster.path()) {
                rasters.push(raster);
                for claimed in rasters { claimed.remove(); }
                return Err(e).with_context(|| format!("write semantic region {}", region.class_id));
            }
            rasters.push(raster);
        }
        Ok((regions, rasters, (source_sky, target_sky), refinements, widenings, mattes))
    })();
    for p in [&src_in, &tgt_in, &src_manifest, &tgt_manifest] { let _ = std::fs::remove_file(p); }
    run
}

pub(super) fn attach_semantic_regions(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    regions: &[semantic::SemanticRegion],
    rasters: &[crate::store::OwnedRaster],
    divergences: &[ZoneDivergence],
) {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let preview = render::develop_preview(&s_img, &report.recipe);
    let (aw, ah) = preview.dimensions();
    // Region j is measured against the render region j-1 left behind, the
    // same rule the tile sweep already follows with its own `current`
    // (`spatial::attach_tiles`). Region 0 reads the pre-region render, which
    // was already being computed for its dimensions and then discarded.
    let mut current = fit::pixels_of(&preview);
    let mut frame_err = report.err_after;
    let corr = report.correspondence.take();
    let mut accepted = Vec::new();
    for ((region, raster), divergence) in regions.iter().zip(rasters).zip(divergences) {
        let sw = mask_weights(&region.source, aw, ah);
        let tw = mask_weights(&region.target, t_img.width(), t_img.height());
        let attachment = ZoneAttachment {
            source_weights: sw,
            target_weights: tw,
            coverage: None,
            mask: MaskGeometry::Bitmap { path: raster.path().to_string_lossy().into_owned() },
            components: Vec::new(),
            range: None,
            name: format!("region-{}-{}", region.class_id, region.label),
            role: MaskRole::Custom,
            inverted: false,
            label: format!("region-{}-{}", region.class_id, region.label),
            min_share: MIN_ZONE_SHARE,
            frame_regression_tol: ZONE_GLOBAL_REGRESSION_TOL,
        };
        let frame_before = frame_err;
        if let Some(mut zone) = attach_one_zone(
            &s_img,
            &tgt_px,
            report,
            &mut frame_err,
            &attachment,
            divergence.divergence,
            corr.as_ref(),
        ) {
            let boundary = spatial::enforce_bitmap_boundary(
                &s_img,
                &tgt_px,
                report,
                zone.mask_index,
                spatial::BitmapBoundaryInput {
                    // Segmentation rasters are feathered, so the transition
                    // band this ruler reads is the real one. Measured against
                    // the cross-boundary step on the calibration corpus and
                    // the viaduct pair (2026-08-30); see the batch report.
                    ruler: spatial::BoundaryRuler::TransitionBand {
                        weights: &zone.mask_weights,
                        reference: &current,
                    },
                    target_boundary: Some(&tgt_px[..]),
                    initial_px: zone.rendered,
                    frame_before,
                    // The tolerance `attach_one_zone` admitted this region
                    // on, so the gate cannot silently override the admission.
                    frame_tol: ZONE_GLOBAL_REGRESSION_TOL,
                },
            );
            match boundary {
                Ok(boundary) => {
                    let target_moments = zone_moments(&tgt_px, &zone.target_weights);
                    zone.after = zone_err(
                        &zone_moments(&boundary.pixels, &zone.source_weights),
                        &target_moments,
                    );
                    zone.rendered = boundary.pixels;
                    current.clone_from(&zone.rendered);
                    frame_err = fit::look_err_with_evidence(
                        &zone.rendered,
                        &tgt_px,
                        &report.evidence,
                    );
                    crate::rationale::push_note(
                        &mut report.recipe.rationale,
                        &mut report.notes,
                        crate::rationale::Note::new(
                            crate::rationale::keys::ZONE_BOUNDARY_PASSED,
                            vec![
                                ("label", attachment.label.clone()),
                                ("n", "1".to_string()),
                                ("before", format!("{:.3}", boundary.initial.rim)),
                                ("after", format!("{:.3}", boundary.reading.rim)),
                                ("k", format!("{:.3}", boundary.k)),
                                ("max", format!("{:.3}", ZONE_BOUNDARY_RIM_MAX)),
                                ("transitions", boundary.reading.transitions.to_string()),
                                ("asked", format!("{:.3}", boundary.reading.asked)),
                                ("charged", format!("{:.3}", boundary.reading.charged)),
                                ("colour", format!("{:.3}", boundary.reading.colour)),
                                (
                                    "colour_charged",
                                    format!("{:.3}", boundary.reading.colour_charged),
                                ),
                            ],
                        ),
                    );
                    accepted.push(zone);
                }
                Err(refusal) => {
                    raster.remove();
                    // The shared gate hands back only what it measured — the
                    // candidate's rim and WHY it refused. Nothing is invented
                    // for a shrink that was never accepted.
                    let why = match refusal.why {
                        spatial::BitmapBoundaryWhy::Rim => "no shared shrink met the rim budget",
                        spatial::BitmapBoundaryWhy::Frame => "the composed frame regressed",
                        // Unreachable on this arm: a feathered region is read
                        // by the transition-band ruler, which never refuses
                        // for want of a measurement. Named, not wildcarded,
                        // so adding a third ruler here has to come back.
                        spatial::BitmapBoundaryWhy::Unmeasured => {
                            "the region boundary could not be sampled"
                        }
                        spatial::BitmapBoundaryWhy::Inert => {
                            "no shrink inside the budget moved a single pixel"
                        }
                    };
                    crate::rationale::push_note(
                        &mut report.recipe.rationale,
                        &mut report.notes,
                        crate::rationale::Note::new(
                            crate::rationale::keys::REGION_BOUNDARY_REFUSED,
                            vec![
                                ("label", attachment.label.clone()),
                                ("why", why.to_string()),
                                ("before", format!("{:.3}", refusal.initial.rim)),
                                ("max", format!("{:.3}", ZONE_BOUNDARY_RIM_MAX)),
                                ("transitions", refusal.initial.transitions.to_string()),
                                ("charged", format!("{:.3}", refusal.initial.charged)),
                                ("colour", format!("{:.3}", refusal.initial.colour)),
                                (
                                    "colour_charged",
                                    format!("{:.3}", refusal.initial.colour_charged),
                                ),
                            ],
                        ),
                    );
                    frame_err = frame_before;
                }
            }
        }
    }
    report.correspondence = corr;
    if accepted.is_empty() {
        for raster in rasters { raster.remove(); }
        let finished = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::append_finished_disclosure(report, &finished, &tgt_px);
        return;
    }
    // A class can pass partition support yet fail its own local quality or
    // acceptance gate.  Release that unreferenced claim; only masks that made
    // it into the recipe survive the run.
    for raster in rasters {
        let path = raster.path().to_string_lossy();
        if !report.recipe.masks.iter().any(|m| render::geometry_raster_path(&m.mask) == Some(path.as_ref())) {
            raster.remove();
        }
    }
    for zone in &accepted { push_zone_attached_note(report, zone); }
    report.err_after = frame_err;
    let worst = semantic::worst_region_residual(&accepted.iter().map(|z| z.after).collect::<Vec<_>>());
    report.recipe.confidence = report.recipe.confidence.min(fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE));
    crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes,
        crate::rationale::Note::new(crate::rationale::keys::ZONE_CONFIDENCE,
            vec![("n", accepted.len().to_string()), ("worst", format!("{worst:.3}")), ("frame", format!("{frame_err:.3}"))]));
    let final_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    fit::append_finished_disclosure(report, &final_px, &tgt_px);
}
