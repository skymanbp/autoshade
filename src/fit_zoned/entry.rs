//! The zoned fit's entry points: the seeded and multi-region solves, the paired layout, the local sequencer, raster release, feather widening and the refinement notes.

use super::*;

// --------------------------------------------------------------------------
// orchestration
// --------------------------------------------------------------------------

/// The zoned reverse-fit: the global [`fit::fit_recipe`] first, then a
/// sky-to-sky zone correction on top — segment the sky in BOTH images
/// (`seg`, the same sidecar the GUI's mask panel uses), compare zone moments,
/// attach a Bitmap-masked [`LocalAdjustment`] when it measurably helps.
///
/// `mask_path` is where the SOURCE sky mask lands (the recipe references
/// it). Pass a FRESHLY CLAIMED raster name (`store::claim_raster`, prefix
/// `mask-zone-sky`, in the photo's develop dir — what the CLI and the GUI
/// both do), never a shared or pre-existing raster: when every zone is
/// skipped or rejected the file at `mask_path` is REMOVED to release the
/// claim, which would destroy a raster another mask still references (an
/// older version of this doc named the AI-select `out/<stem>.mask-sky.png`
/// convention — exactly such a shared raster).
/// GRACEFUL BY CONTRACT: segmentation missing/failing, a
/// degenerate sky, or a correction that does not improve the look all fall
/// back to the plain global fit with an honest rationale note — never an
/// error, because the global fit in hand is already a valid result.
pub fn fit_recipe_zoned(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
) -> FitReport {
    fit_recipe_zoned_from(src, target, seg, mask_path, &crate::recipe::EditRecipe::default())
}

/// Multi-region entry point shared by the CLI and GUI.  `2` intentionally
/// routes through the historical sky/land implementation so its recipe bytes
/// and rationale remain unchanged. `options` carries the step-11 strength
/// budget and the step-7b provider into BOTH routes.
pub fn fit_recipe_zoned_with_regions(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    regions: usize,
) -> FitReport {
    if regions <= semantic::DEFAULT_SEMANTIC_REGIONS {
        return fit_recipe_zoned_inner_with_options(src, target, seg, mask_path, base, options, SHIPPED_LAYERS);
    }
    fit_recipe_zoned_multi_inner(
        src, target, seg, mask_path, base, options,
        regions.min(semantic::MAX_SEMANTIC_REGIONS),
    )
}

/// [`fit_recipe_zoned`] with a calibration-only base composed into the
/// solve — see [`fit::fit_recipe_from`]. The zone passes need no changes
/// of their own: every zone statistic is measured on a render of the
/// CURRENT recipe (which carries the base from the start), so the zones
/// already live in the canvas's one-pass domain.
pub fn fit_recipe_zoned_from(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
) -> FitReport {
    fit_recipe_zoned_inner(src, target, seg, mask_path, base, None, SHIPPED_LAYERS)
}

pub(super) fn fit_recipe_zoned_inner(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    provider: Option<fit::CorrespondenceProvider>,
    layers: ZonedLayerOpts,
) -> FitReport {
    fit_recipe_zoned_inner_with_options(src, target, seg, mask_path, base, fit::FitOptions { strength: crate::recipe::GradeStrength::default(), provider }, layers)
}

pub(super) fn fit_recipe_zoned_inner_with_options(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    layers: ZonedLayerOpts,
) -> FitReport {
    fit_recipe_zoned_inner_seeded(
        src, target, seg, mask_path, base, options, layers, None,
    )
}

#[allow(clippy::too_many_arguments)]
fn fit_recipe_zoned_inner_seeded(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    layers: ZonedLayerOpts,
    segmented: Option<(GrayImage, GrayImage)>,
) -> FitReport {
    let segmented = match segmented {
        Some(pair) => Ok(pair),
        None => segment_both(src, target, seg, mask_path),
    };
    let (mut report, field, first_producer, layout, zones) = match segmented {
        Ok((mut src_mask, mut tgt_mask)) => {
            let refinements = if layers.refine_masks {
                let source = crate::mask_refine::guided_refine(
                    src,
                    &src_mask,
                    MASK_REFINE_RADIUS,
                    MASK_REFINE_EPSILON,
                );
                let target = crate::mask_refine::guided_refine(
                    target,
                    &tgt_mask,
                    MASK_REFINE_RADIUS,
                    MASK_REFINE_EPSILON,
                );
                Some((source, target))
            } else {
                None
            };
            if let Some((source_refinement, target_refinement)) = refinements {
                let mut readings = Vec::new();
                match source_refinement {
                    crate::mask_refine::RefineOutcome::Kept { mask, reading } => {
                        if mask.save(mask_path.path()).is_ok() {
                            src_mask = mask;
                            readings.push(("semantic source", true, reading));
                        } else {
                            readings.push(("semantic source", false, reading));
                        }
                    }
                    crate::mask_refine::RefineOutcome::Abstained { reading } => {
                        readings.push(("semantic source", false, reading));
                    }
                }
                match target_refinement {
                    crate::mask_refine::RefineOutcome::Kept { mask, reading } => {
                        tgt_mask = mask;
                        readings.push(("semantic target", true, reading));
                    }
                    crate::mask_refine::RefineOutcome::Abstained { reading } => {
                        readings.push(("semantic target", false, reading));
                    }
                }
                // The SOURCE feather only: a seam is a property of the frame
                // that gets rendered, and the target raster is read for
                // statistics that a wider ramp would only blur.
                // The matte before the widener, for the reason the
                // multi-region bridge gives; the raster the recipe points at
                // is rewritten so the gate measures the alpha that renders.
                let mattes = vec![matte_source_edge(src, &mut src_mask, "semantic source")];
                if mattes.iter().any(|(_, matted, _, holes)| *matted || holes.is_some())
                    && src_mask.save(mask_path.path()).is_err()
                {
                    src_mask = image::open(mask_path.path()).map(|i| i.to_luma8()).unwrap_or(src_mask);
                }
                let widenings = vec![widen_source_feather(
                    src,
                    &mut src_mask,
                    mask_path,
                    "semantic source",
                )];
                let zone_divergence = measure_zone_divergence(src, target, base, &src_mask);
                let divergent_cover = [zone_divergence.sky, zone_divergence.land]
                    .into_iter()
                    .filter(|zone| zone.divergence.is_some_and(|d| d.d >= fit::DIVERGENCE_ZONE))
                    .map(|zone| zone.share)
                    .sum::<f32>();
                let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(
                    src,
                    target,
                    base,
                    divergent_cover >= fit::DIVERGENT_COVER_PROMOTES,
                    true,
                    options,
                );
                for (label, kept, reading) in readings {
                    crate::rationale::push_note(
                        &mut report.recipe.rationale,
                        &mut report.notes,
                        crate::rationale::Note::new(
                            if kept {
                                crate::rationale::keys::MASK_REFINEMENT_KEPT
                            } else {
                                crate::rationale::keys::MASK_REFINEMENT_ABSTAINED
                            },
                            vec![
                                ("label", label.to_string()),
                                ("coverage", format!("{:.6}", reading.coverage_delta)),
                                ("before", format!("{:.6}", reading.edge_before)),
                                ("after", format!("{:.6}", reading.edge_after)),
                                ("core", reading.core_changed.to_string()),
                            ],
                        ),
                    );
                }
                push_matte_notes(&mut report, &mattes);
                push_widening_notes(&mut report, &widenings);
                let field = layers.field
                    .then(|| field::solve_local_field(src, target, &mut report)).flatten();
                let partitioned = attach_zones_with_divergence(
                    src,
                    target,
                    &mut report,
                    &src_mask,
                    &tgt_mask,
                    mask_path,
                    zone_divergence,
                );
                (
                    report,
                    field,
                    "zones",
                    paired_layout(src, target, &[(&src_mask, &tgt_mask)]),
                    if partitioned { detail::sky_land_zones(&src_mask, &tgt_mask, mask_path) } else { Vec::new() },
                )
            } else {
            let zone_divergence = measure_zone_divergence(src, target, base, &src_mask);
            let divergent_cover = [zone_divergence.sky, zone_divergence.land]
                .into_iter()
                .filter(|zone| zone.divergence.is_some_and(|d| d.d >= fit::DIVERGENCE_ZONE))
                .map(|zone| zone.share)
                .sum::<f32>();
            let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(
                src,
                target,
                base,
                divergent_cover >= fit::DIVERGENT_COVER_PROMOTES,
                true,
                options,
            );
            let field = layers.field
                .then(|| field::solve_local_field(src, target, &mut report)).flatten();
            let partitioned = attach_zones_with_divergence(
                src,
                target,
                &mut report,
                &src_mask,
                &tgt_mask,
                mask_path,
                zone_divergence,
            );
            (
                report,
                field,
                "zones",
                paired_layout(src, target, &[(&src_mask, &tgt_mask)]),
                if partitioned { detail::sky_land_zones(&src_mask, &tgt_mask, mask_path) } else { Vec::new() },
            )
            }
        }
        Err(e) => {
            // The provider still rides the fallback: a failed segmentation
            // must not also cost the global fit its correspondence.
            let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(
                src,
                target,
                base,
                false,
                true,
                options,
            );
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONED_UNAVAILABLE,
                    vec![("e", crate::rationale::error_line(&e))],
                ),
            );
            let field = layers.field
                .then(|| field::solve_local_field(src, target, &mut report)).flatten();
            let proposals = field.as_ref()
                .map(|(_, reading)| reading.proposals.as_slice()).unwrap_or(&[]);
            range::attach_ranges(src, target, &mut report, proposals);
            // No segmentation, no zone whose local contrast could be paired.
            (report, field, "ranges", Vec::new(), Vec::new())
        }
    };
    run_local_sequencer(src, target, &mut report, &field, first_producer, mask_path, layers, &layout, &zones);
    report
}

/// R42. The membership every analysis pixel has in a region the segmenter
/// found in BOTH frames: per pair of rasters the product of the source and
/// target memberships at analysis size, so a pixel counts only where the same
/// class sits in the same place on both sides, and the union over the pairs.
/// The two-region route pairs its sky plane alone — its land is the sky's
/// complement, "not sky" on both sides, which is not a class the segmenter
/// found; the multi-region route pairs every class it resolved; the range
/// fallback has none. The colour field reads it
/// (`field::attach_colour_field`) for the cells the pixel-scale evidence
/// cannot read at all.
fn paired_layout(
    src: &DynamicImage,
    target: &DynamicImage,
    pairs: &[(&GrayImage, &GrayImage)],
) -> Vec<f32> {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let (w, h) = (s_img.width(), s_img.height());
    let presence = fit::presence_of(&t_img);
    let mut out = vec![0.0f32; w as usize * h as usize];
    for (source, tgt) in pairs {
        let sw = mask_weights(source, w, h);
        let tw = mask_weights(tgt, t_img.width(), t_img.height());
        let tw = fit::with_presence(&tw, presence.as_deref()).into_owned();
        for ((o, s), t) in out.iter_mut().zip(&sw).zip(&tw) {
            *o = o.max(s * t);
        }
    }
    out
}

/// The single local producer sequencer shared by the historical two-region
/// route, the semantic multi-region route, and the range fallback.  Keep the
/// order and disclosures stable: first producer -> stop -> tiles -> stop ->
/// free masks -> colour field. `layout` is [`paired_layout`]'s reading for
/// the colour field.
///
/// A stop ends the MASK producers only. The colour field is still asked, and
/// on a stopped fit it withholds itself on the same verdict and says so
/// (`FIELD_WITHHELD`): until 2026-10-19 the early return skipped it in
/// silence, which no fixture reached until zones stopped being capped at
/// ~1 EV by the texture gate and began ending the sequencer on their own.
///
/// The detail stage ([`detail::attach_zone_detail`]) runs LAST, after the
/// colour field, and is not ended by a stop either: a stop is a verdict on
/// the tone residual, and a zone's local contrast is a separate question
/// the tone producers never asked (2026-10-21). `zones` are the segmented
/// regions the stage may read — the sky and land of the two-region route,
/// every resolved class of the multi-region route, none on the range
/// fallback.
#[allow(clippy::too_many_arguments)]
fn run_local_sequencer(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    field: &Option<(crate::fit_field::LocalField, field::ShapeReading)>,
    first_producer: &str,
    mask_path: &crate::store::OwnedRaster,
    layers: ZonedLayerOpts,
    layout: &[f32],
    zones: &[detail::DetailZone],
) {
    run_mask_producers(src, target, report, field, first_producer, mask_path, layers);
    // R33 §G: the TERMINAL producer, and the only one that is not a mask. It
    // runs at the shipped default strength and above (R41 moved the gate down
    // from "above the default"); every result below the default is
    // byte-identical to the build before R33 §G.
    if field.is_some() {
        field::attach_colour_field(
            src,
            target,
            report,
            fit::carried_strength_from_notes(&report.notes),
            layout,
        );
    }
    if layers.detail {
        detail::attach_zone_detail(src, target, report, zones);
    }
}

/// The mask half of [`run_local_sequencer`]; a stop verdict returns from here.
fn run_mask_producers(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    field: &Option<(crate::fit_field::LocalField, field::ShapeReading)>,
    first_producer: &str,
    mask_path: &crate::store::OwnedRaster,
    layers: ZonedLayerOpts,
) {
    if let Some((local, _)) = field {
        field::push_realized(report, local, first_producer);
        if layers.free_masks && field::stop_verdict(local, report.err_after) {
            let skipped = match (layers.spatial, layers.free_masks) {
                (true, true) => "tiles, free masks",
                (true, false) => "tiles",
                (false, true) => "free masks",
                (false, false) => "none",
            };
            field::push_stop(report, first_producer, skipped);
            return;
        }
    }
    let mut excluded = field
        .as_ref()
        .map(|(local, _)| vec![0.0f32; local.remainder.len()])
        .unwrap_or_default();
    if layers.spatial {
        let cap = field
            .as_ref()
            .map(|(_, reading)| reading.effective_tile_cap)
            .unwrap_or(spatial::SPATIAL_MAX_ATTACHMENTS);
        excluded = spatial::attach_tiles(src, target, report, mask_path, layers.refine_masks, cap);
        if let Some((local, _)) = field {
            field::push_realized(report, local, "tiles");
            if layers.free_masks && field::stop_verdict(local, report.err_after) {
                field::push_stop(report, "tiles", "free masks");
                return;
            }
        }
    }
    if layers.free_masks && let Some((local, _)) = field {
        let stage = freemask::attach_free_masks(
            src,
            target,
            report,
            mask_path,
            local,
            &excluded,
            layers.refine_masks,
            freemask::FREE_MASK_MAX_ATTACHMENTS,
        );
        debug_assert_eq!(stage.components, stage.disclosed);
        if stage.ran { field::push_realized(report, local, "free masks"); }
    }
}

/// Multi-class semantic path.  It intentionally shares the global solve and
/// downstream field/tile stages with the legacy path, while replacing only the
/// sky/land producer with one attachment per disjoint class region.
fn fit_recipe_zoned_multi_inner(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
    options: fit::FitOptions<'_>,
    max_regions: usize,
) -> FitReport {
    let semantic = segment_multiclass_both(src, target, seg, mask_path, max_regions);
    let (regions, rasters, sky_pair, refinements, widenings, mattes) = match semantic {
        Ok(pair) => pair,
        Err(e) => {
            // A multi-manifest failure is a semantic-layer failure, not a
            // reason to switch producers. Re-enter the historical two-region
            // route; it owns its own range fallback and sequencer, and its
            // result remains the byte-identity reference. Disclosed under its
            // OWN key: `ZONED_UNAVAILABLE` narrates a luminance-range fallback,
            // and the route that ran here is the sky/land pass.
            let mut report = fit_recipe_zoned_inner_with_options(
                src, target, seg, mask_path, base, options, SHIPPED_LAYERS,
            );
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::SEMANTIC_REGIONS_UNAVAILABLE,
                    // THE door, like every other `{e}` note: one line, absolute
                    // paths reduced to a basename (`[path]` inside a home
                    // directory), 160 characters. `{e:#}` flattened the whole
                    // anyhow chain and carried the sidecar's paths into a
                    // rationale that travels into XMP and into bug reports.
                    vec![("e", crate::rationale::error_line(&e))],
                ),
            );
            return report;
        }
    };
    if regions.is_empty() {
        // No class cleared the shared support floor on both frames. The
        // historical route is the reference result here too: it judges the
        // sky partition on its own numbers (and drops its anchor when that
        // fails) and runs the same sequencer — a typed hand-off, not a fourth
        // exit that would render bare placeholders and strand the anchor.
        let mut report = fit_recipe_zoned_inner_seeded(
            src, target, seg, mask_path, base, options, SHIPPED_LAYERS, Some(sky_pair),
        );
        push_refinement_notes(&mut report, &refinements);
        push_matte_notes(&mut report, &mattes);
        push_widening_notes(&mut report, &widenings);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::SEMANTIC_REGIONS_NONE,
                vec![("n", max_regions.to_string())],
            ),
        );
        return report;
    }
    // The five things the sequencer needs from this route: the report the
    // global solve and the region producer built, the colour field, the
    // producer's name for the disclosures, the paired layout for the field,
    // and the regions as the detail stage reads them (every resolved class
    // on its own raster, source and target membership both).
    let (mut report, field, first_producer, layout, zones) = {
        let fit::DivergenceRaster { source: sp, target: tp, width: w, height: h, presence } = fit::divergence_raster(src, target, base);
        let divergences = regions.iter().map(|r| {
            let weights = mask_weights(&r.source, w, h);
            ZoneDivergence { divergence: fit::structure_divergence(&sp, &tp, w, h, &fit::with_presence(&weights, presence.as_deref())), share: weights.iter().sum::<f32>() / weights.len().max(1) as f32 }
        }).collect::<Vec<_>>();
        // Promotion is the claim that the structure was REPLACED over this much
        // of the frame, so a region the instrument could not read contributes
        // nothing to the cover: `is_some_and` refuses on an absent reading where
        // the old field read a matched 0.0 and refused for the wrong reason.
        let divergent_cover = divergences.iter().filter(|d| d.divergence.is_some_and(|r| r.d >= fit::DIVERGENCE_ZONE)).map(|d| d.share).sum::<f32>();
        let mut report = fit::fit_recipe_from_promoted_with_disclosure_opts(src, target, base,
            divergent_cover >= fit::DIVERGENT_COVER_PROMOTES, true, options);
        // The same disclosure the sky/land route makes for ITS refinement.
        push_refinement_notes(&mut report, &refinements);
        push_matte_notes(&mut report, &mattes);
        push_widening_notes(&mut report, &widenings);
        let field = SHIPPED_LAYERS.field.then(|| field::solve_local_field(src, target, &mut report)).flatten();
        attach_semantic_regions(src, target, &mut report, &regions, &rasters, &divergences);
        let pairs: Vec<(&GrayImage, &GrayImage)> =
            regions.iter().map(|region| (&region.source, &region.target)).collect();
        (
            report,
            field,
            "semantic regions",
            paired_layout(src, target, &pairs),
            detail::region_zones(&regions, &rasters),
        )
    };
    run_local_sequencer(src, target, &mut report, &field, first_producer, mask_path, SHIPPED_LAYERS, &layout, &zones);
    // The four-region producer may leave pixels to the global fit by design;
    // the historical two-region producer owns the inverse-sky complement.
    // Compare against the unchanged legacy sequencer and keep the multi-class
    // candidate only when it is no worse. This is a strict selection gate,
    // not extra segmentation and not a tolerance. The multi-class sky plane
    // remains available for the region producer, but the legacy path is the
    // byte-identity reference and runs through the seeded sky bridge.
    let two = fit_recipe_zoned_inner_seeded(
        src,
        target,
        seg,
        mask_path,
        base,
        options,
        SHIPPED_LAYERS,
        Some(sky_pair),
    );
    let remove_unselected = |candidate: &FitReport, kept: &FitReport| {
        release_unselected_rasters(candidate, kept, mask_path)
    };
    // One ruler for the comparison. Each report's `err_after` was measured
    // under ITS OWN evidence model, and the two global solves can land in
    // different modes — a Full ruler is structural, an Atmosphere ruler
    // structure-blind — so those numbers are not comparable across reports.
    // Both finished renders are re-measured under the reference's ruler, and
    // those are the numbers the refusal discloses.
    let multi_error = frame_err_under(src, target, &report, &two.evidence);
    let two_error = frame_err_under(src, target, &two, &two.evidence);
    if multi_error >= two_error {
        // The two key FAMILIES first (R34 §D2): one outcome each, two
        // carriers each, and `is_colour_refusal`/`is_tone_refusal` are the one
        // place that knows it — a match arm per key would be a second place.
        let verdict_name = |key: &str| {
            if is_colour_refusal(key) {
                return "ZONE_EVIDENCE_WITHHELD_COLOUR";
            }
            if is_tone_refusal(key) {
                return "ZONE_EVIDENCE_WITHHELD_TONE";
            }
            match key {
            crate::rationale::keys::ZONE_ATTACHED => "ZONE_ATTACHED",
            crate::rationale::keys::ZONE_ALREADY_MATCHED => "ZONE_ALREADY_MATCHED",
            crate::rationale::keys::ZONE_NO_MOVEMENT_SURVIVED => "ZONE_NO_MOVEMENT_SURVIVED",
            crate::rationale::keys::ZONE_SHARE_NO_CORRECTION => "ZONE_NO_CORRECTION",
            crate::rationale::keys::ZONE_TOO_SMALL => "ZONE_TOO_SMALL",
            crate::rationale::keys::ZONE_SHARE_MISMATCH => "ZONE_SHARE_MISMATCH",
            crate::rationale::keys::ZONE_BOUNDARY_PASSED => "ZONE_BOUNDARY_PASSED",
            crate::rationale::keys::ZONE_BOUNDARY_INERT => "ZONE_BOUNDARY_INERT",
            crate::rationale::keys::REGION_BOUNDARY_REFUSED => "REGION_BOUNDARY_REFUSED",
            crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED => "ZONE_QUALITY_TEXTURE_FAILED",
            crate::rationale::keys::ZONE_QUALITY_CLIPPING_FAILED => "ZONE_QUALITY_CLIPPING_FAILED",
            crate::rationale::keys::ZONE_DROPPED => "ZONE_DROPPED",
            crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED => "ZONE_ATMOSPHERE_DROPPED",
            crate::rationale::keys::ZONE_MODE_FULL => "ZONE_MODE_FULL",
            crate::rationale::keys::ZONE_MODE_ATMOSPHERE => "ZONE_MODE_ATMOSPHERE",
            crate::rationale::keys::ZONE_QUALITY_PASSED => "ZONE_QUALITY_PASSED",
            // An honest "something else", never a verdict the zone did not get.
            _ => "ZONE_OTHER",
            }
        };
        let regions_text = regions.iter().map(|region| {
            let label = format!("region-{}-{}", region.class_id, region.label);
            let verdict = report.notes.iter().rev()
                .find(|note| note.args.iter().any(|(name, value)| *name == "label" && value == &label))
                .map(|note| note.key)
                .unwrap_or(crate::rationale::keys::ZONE_SHARE_NO_CORRECTION);
            format!("{} {}: {}", region.class_id, region.label, verdict_name(verdict))
        }).collect::<Vec<_>>().join("; ");
        remove_unselected(&report, &two);
        let mut chosen = two;
        let refusal = crate::rationale::Note::new(
            crate::rationale::keys::REGION_FRAME_REFUSED,
            vec![
                ("multi", format!("{multi_error:.6}")),
                ("two", format!("{two_error:.6}")),
                ("regions", regions_text),
            ],
        );
        chosen.recipe.rationale.push_str(&crate::rationale::render_one(&refusal));
        // Keep the truncation sentinel in place.  Appending the arbitration
        // verdict after it preserves the consumer's raw-English fallback
        // while still exposing the typed decision to the GUI/test callers.
        chosen.notes.push(refusal);
        return chosen;
    }
    // Keep the selected semantic candidate and release only the seeded legacy
    // candidate's unreferenced raster claims.
    remove_unselected(&two, &report);
    report
}

/// Claim hygiene after arbitration: every mask raster the losing
/// `candidate` references and the `kept` report does not is released, and so
/// is the shared anchor when nothing kept references it. Only files inside
/// the anchor's own directory (the develop store) are touched, so a recipe
/// that names a raster elsewhere can never make the fit delete it.
///
/// **The question is "which FILE does this mask render from", not "is this
/// mask a `Bitmap`"** — `render::geometry_raster_path`, the one place that is
/// answered. Asking the geometry's variant instead was safe only while every
/// zone was a raster mask: the sky/land zones are Select Sky components now
/// (`MaskGeometry::select_sky`), and a variant test would have found the kept
/// recipe referencing nothing, so the `anchor.remove()` below would have
/// deleted the very alpha the winning report renders both of its zones from.
pub(super) fn release_unselected_rasters(
    candidate: &FitReport,
    kept: &FitReport,
    anchor: &crate::store::OwnedRaster,
) {
    let rasters = |report: &FitReport| {
        report
            .recipe
            .masks
            .iter()
            .filter_map(|mask| render::geometry_raster_path(&mask.mask).map(str::to_string))
            .collect::<Vec<_>>()
    };
    let keep = rasters(kept).into_iter().collect::<std::collections::HashSet<_>>();
    let parent = anchor.path().parent();
    for path in rasters(candidate) {
        let file = std::path::Path::new(&path);
        if !keep.contains(&path) && file.parent() == parent {
            let _ = std::fs::remove_file(file);
        }
    }
    let anchor_name = anchor.path().to_string_lossy().into_owned();
    if !keep.contains(&anchor_name) {
        anchor.remove();
    }
}

/// A report's finished render measured under a GIVEN evidence ruler — the
/// only way two reports that may have solved in different modes can be
/// compared. Analysis geometry, like every zone gate.
pub(super) fn frame_err_under(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &FitReport,
    evidence: &fit::EvidenceModel,
) -> f32 {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    fit::look_err_with_evidence(&px, &tgt_px, evidence)
}

/// One guided-refinement reading per class plane, as the bridge took it.
pub(super) type PlaneRefinement = (String, bool, crate::mask_refine::RefineReading);

/// One feather-widening reading, as the caller needs to disclose it.
pub(super) type FeatherWidening = (String, bool, crate::mask_refine::WidenReading);

/// One content-matte reading, as the caller needs to disclose it: the label,
/// whether the keying changed the mask, its reading, and the confidence-hole
/// fill before it as `(share of partial pixels moved, reach)` when it moved any.
pub(super) type EdgeMatte = (String, bool, crate::mask_refine::MatteReading, Option<(f32, u32)>);

/// Settle a SOURCE semantic raster's transition onto the scene's own edge
/// ([`crate::mask_refine::content_matte`]) in place. The caller owns the
/// bytes on disk: the two-region route rewrites the raster it already claimed,
/// and the multi-region route claims its region rasters from these planes.
pub(super) fn matte_source_edge(guide: &DynamicImage, mask: &mut GrayImage, label: &str) -> EdgeMatte {
    // The holes first, so the keying's side references are the region's own
    // level and not the model's hesitation.
    let holes = crate::mask_refine::fill_confidence_holes(mask).map(|(filled, share, reach)| {
        *mask = filled;
        (share, reach)
    });
    match crate::mask_refine::content_matte(guide, mask) {
        crate::mask_refine::MatteOutcome::Matted { mask: matted, reading } => {
            *mask = matted;
            (label.to_string(), true, reading, holes)
        }
        crate::mask_refine::MatteOutcome::Abstained { reading } => (label.to_string(), false, reading, holes),
    }
}

fn push_matte_notes(report: &mut FitReport, mattes: &[EdgeMatte]) {
    for (label, matted, reading, holes) in mattes {
        if let Some((share, reach)) = holes {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::MASK_HOLES_FILLED,
                    vec![
                        ("label", label.clone()),
                        ("share", format!("{:.1}", share * 100.0)),
                        ("radius", reach.to_string()),
                    ],
                ),
            );
        }
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                if *matted {
                    crate::rationale::keys::MASK_EDGE_MATTED
                } else {
                    crate::rationale::keys::MASK_EDGE_MATTE_ABSTAINED
                },
                vec![
                    ("label", label.clone()),
                    ("share", format!("{:.1}", reading.matted_share * 100.0)),
                    ("radius", reading.radius.to_string()),
                    ("coverage", format!("{:.6}", reading.coverage_delta)),
                ],
            ),
        );
    }
}

/// Widen a SOURCE semantic raster's feather where the guide is smooth and
/// replace its PNG bytes, exactly the way a kept guided refinement replaces
/// them — same claimed name, same raster, because the widened alpha is this
/// zone's own mask and the recipe already points at that path.
///
/// It runs BEFORE any boundary reading is taken, and the order is load
/// bearing in both directions. The gate must measure the mask that will
/// actually be rendered; and a ramp that persists past a crossing's two
/// baselines earns slope credit against the per-crossing budget
/// ([`crossing_slope`]), so the two halves of the fix compose instead of
/// each taking strength away. Measured on the 64x256 haze fixture at
/// +0.30 EV: a 3-px feather is charged 3.06x its raw rim — the whole
/// ceiling/floor exchange — and keeps k = 0.142; widened first, the raw rim
/// is unchanged, the charge falls to 1.02x and the shrink keeps k = 0.526,
/// 3.7x the strength at the same ceiling. Not k = 1, because the
/// transition-band ruler reads the ramp's HEIGHT and a wider ramp does not
/// reduce that; what widening buys back is the slope credit.
fn widen_source_feather(
    guide: &DynamicImage,
    mask: &mut GrayImage,
    raster: &crate::store::OwnedRaster,
    label: &str,
) -> FeatherWidening {
    match crate::mask_refine::widen_smooth_feather(guide, mask) {
        crate::mask_refine::WidenOutcome::Widened { mask: widened, reading } => {
            if widened.save(raster.path()).is_ok() {
                *mask = widened;
                (label.to_string(), true, reading)
            } else {
                (label.to_string(), false, reading)
            }
        }
        crate::mask_refine::WidenOutcome::Abstained { reading } => {
            (label.to_string(), false, reading)
        }
    }
}

fn push_widening_notes(report: &mut FitReport, widenings: &[FeatherWidening]) {
    for (label, widened, reading) in widenings {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                if *widened {
                    crate::rationale::keys::MASK_FEATHER_WIDENED
                } else {
                    crate::rationale::keys::MASK_FEATHER_ABSTAINED
                },
                vec![
                    ("label", label.clone()),
                    ("share", format!("{:.1}", reading.widened_share * 100.0)),
                    ("radius", reading.max_radius.to_string()),
                    ("coverage", format!("{:.6}", reading.coverage_delta)),
                ],
            ),
        );
    }
}

fn push_refinement_notes(report: &mut FitReport, refinements: &[PlaneRefinement]) {
    for (label, kept, reading) in refinements {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                if *kept {
                    crate::rationale::keys::MASK_REFINEMENT_KEPT
                } else {
                    crate::rationale::keys::MASK_REFINEMENT_ABSTAINED
                },
                vec![
                    ("label", label.clone()),
                    ("coverage", format!("{:.6}", reading.coverage_delta)),
                    ("before", format!("{:.6}", reading.edge_before)),
                    ("after", format!("{:.6}", reading.edge_after)),
                    ("core", reading.core_changed.to_string()),
                ],
            ),
        );
    }
}
