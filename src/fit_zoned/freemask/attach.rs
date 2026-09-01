use image::{imageops::FilterType, DynamicImage};

use super::super::*;
use super::*;

fn compact_numbers(mut numbers: Vec<usize>) -> String {
    numbers.sort_unstable();
    let mut out = Vec::new();
    let (mut start, mut end) = (numbers[0], numbers[0]);
    for n in numbers.into_iter().skip(1) {
        if n == end + 1 { end = n; continue; }
        out.push(if start == end { start.to_string() } else { format!("{start}-{end}") });
        (start, end) = (n, n);
    }
    out.push(if start == end { start.to_string() } else { format!("{start}-{end}") });
    out.join(",")
}

fn push_refusal(report: &mut FitReport, refusals: &[FreeMaskRefusal]) {
    for why in [
        FreeMaskWhy::Share, FreeMaskWhy::Divergence, FreeMaskWhy::Cap,
        FreeMaskWhy::Footprint, FreeMaskWhy::Mass, FreeMaskWhy::RasterClaim,
        FreeMaskWhy::RasterWrite, FreeMaskWhy::ZoneRefused,
        FreeMaskWhy::Frame, FreeMaskWhy::Rim, FreeMaskWhy::Unmeasured,
        FreeMaskWhy::Inert,
    ] {
        let numbers = refusals.iter().filter(|r| r.why == why).map(|r| r.n).collect::<Vec<_>>();
        if numbers.is_empty() { continue; }
        crate::rationale::push_note(
            &mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(crate::rationale::keys::FIELD_MASK_REFUSED, vec![
                ("n", compact_numbers(numbers)), ("why", why.label().into()),
            ]),
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::fit_zoned) fn attach_free_masks(
    src: &DynamicImage, target: &DynamicImage, report: &mut FitReport,
    raster_home: &crate::store::OwnedRaster, field: &LocalField, excluded: &[f32],
    refine: bool, cap: usize,
) -> FreeMaskStage {
    #[cfg(test)]
    {
        FREE_MASK_CALLS.with(|calls| calls.set(calls.get() + 1));
        if FREE_MASK_BYPASS.with(|bypass| bypass.replace(false)) {
            let stage = FreeMaskStage { ran: false, components: 0, disclosed: 0 };
            super::record_stage(stage);
            return stage;
        }
    }
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let target_px = fit::pixels_of(&t_img);
    let corr = report.correspondence.take();
    let search = search_free_masks(
        field, excluded, &report.evidence.source_pixels, &target_px, &report.evidence, cap,
    );
    let components = search.proposals.len()
        + search.refusals.iter().filter(|r| r.why != FreeMaskWhy::NoCandidates).count();
    let mut disclosed = search.refusals.iter()
        .filter(|r| r.why != FreeMaskWhy::NoCandidates).count();
    push_refusal(report, &search.refusals);
    if search.proposals.is_empty() {
        let why = if search.refusals.iter().any(|r| r.why == FreeMaskWhy::NoCandidates) {
            FreeMaskWhy::NoCandidates.label()
        } else { "all-refused" };
        crate::rationale::push_note(
            &mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::FIELD_MASK_NONE, vec![("why", why.into())],
            ),
        );
        report.correspondence = corr;
        let stage = FreeMaskStage { ran: true, components, disclosed };
        super::record_stage(stage);
        return stage;
    }
    for (number, proposal) in &search.proposals {
        crate::rationale::push_note(
            &mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(crate::rationale::keys::FIELD_MASK_PROPOSED, vec![
                ("n", number.to_string()),
                ("sign", if proposal.sign < 0.0 { "-".into() } else { "+".into() }),
                ("mass", format!("{:.6}", proposal.mass)),
                ("share_src", format!("{:.3}", proposal.share.0)),
                ("share_tgt", format!("{:.3}", proposal.share.1)),
                ("d", format!("{:.3}", proposal.divergence.d)),
                ("pixels", proposal.pixels.to_string()),
            ]),
        );
    }
    let mut attached_count = 0usize;
    for (number, proposal) in search.proposals {
        let label = format!("field-zone-{number}");
        let owned = match raster_home.claim_sibling("mask-zone-field") {
            Ok(path) => path,
            Err(_) => {
                push_refusal(report, &[FreeMaskRefusal { n: number, why: FreeMaskWhy::RasterClaim }]);
                disclosed += 1;
                continue;
            }
        };
        let guide = src.thumbnail(spatial::TILE_RASTER_EDGE, spatial::TILE_RASTER_EDGE);
        let raw_mask = image::imageops::resize(
            &proposal.mask, guide.width(), guide.height(), FilterType::Nearest,
        );
        let (mask, refined) = if refine {
            match crate::mask_refine::guided_refine(
                &guide, &raw_mask, 8, (4.0f32 / 255.0).powi(2),
            ) {
                crate::mask_refine::RefineOutcome::Kept { mask, reading } => {
                    spatial::push_refinement_note(report, &label, true, reading);
                    (mask, true)
                }
                crate::mask_refine::RefineOutcome::Abstained { reading } => {
                    spatial::push_refinement_note(report, &label, false, reading);
                    (raw_mask, false)
                }
            }
        } else { (raw_mask, false) };
        // Refinement is a completed producer reading and must survive any
        // later attachment refusal; only tentative attachment notes roll back.
        let rationale_before_attach = report.recipe.rationale.len();
        let notes_before_attach = report.notes.len();
        #[cfg(test)]
        if FREE_MASK_FORCE_ZONE_REFUSAL.with(|force| force.replace(false)) {
            owned.remove();
            report.recipe.rationale.truncate(rationale_before_attach);
            report.notes.truncate(notes_before_attach);
            push_refusal(report, &[FreeMaskRefusal { n: number, why: FreeMaskWhy::ZoneRefused }]);
            disclosed += 1;
            continue;
        }
        if mask.save(owned.path()).is_err() {
            owned.remove();
            report.recipe.rationale.truncate(rationale_before_attach);
            report.notes.truncate(notes_before_attach);
            push_refusal(report, &[FreeMaskRefusal { n: number, why: FreeMaskWhy::RasterWrite }]);
            disclosed += 1;
            continue;
        }
        let coverage = ZoneCoverage {
            source: mask_weights(&mask, s_img.width(), s_img.height()),
            target: mask_weights(&mask, t_img.width(), t_img.height()),
        };
        // The boundary gate reads the mask's own alpha, not the evidence-scoped
        // estimator weights below; kept before `coverage` is moved.
        let boundary_geometry = coverage.source.clone();
        let raw_geometry = proposal.mask.as_raw().iter()
            .map(|v| *v as f32 / 255.0).collect::<Vec<_>>();
        let scoped = spatial::scoped_mask_evidence(&target_px, &report.evidence, &raw_geometry);
        let (source_weights, target_weights) = if refined {
            (
                coverage.source.iter().zip(scoped.source_weights).map(|(m, e)| m * e).collect(),
                coverage.target.iter().zip(scoped.target_weights).map(|(m, e)| m * e).collect(),
            )
        } else { (scoped.source_weights, scoped.target_weights) };
        let attachment = ZoneAttachment {
            source_weights, target_weights, coverage: Some(coverage),
            mask: MaskGeometry::Bitmap { path: owned.path().to_string_lossy().into_owned() },
            range: None, name: label.clone(), role: MaskRole::Custom, inverted: false,
            label, min_share: MIN_ZONE_SHARE,
            frame_regression_tol: spatial::SPATIAL_FRAME_REGRESSION_TOL,
        };
        let before_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let frame_before = fit::look_err_with_evidence(&before_px, &target_px, &report.evidence);
        let mut frame_err = frame_before;
        let first_mask = report.recipe.masks.len();
        let Some(accepted) = attach_one_zone(
            &s_img, &target_px, report, &mut frame_err, &attachment,
            proposal.divergence, corr.as_ref(),
        ) else {
            owned.remove();
            report.recipe.rationale.truncate(rationale_before_attach);
            report.notes.truncate(notes_before_attach);
            report.recipe.masks.truncate(first_mask);
            push_refusal(report, &[FreeMaskRefusal { n: number, why: FreeMaskWhy::ZoneRefused }]);
            disclosed += 1;
            continue;
        };
        let boundary = spatial::enforce_bitmap_boundary(
            &s_img, &target_px, report, first_mask,
            spatial::BitmapBoundaryInput {
                ruler: spatial::BoundaryRuler::CrossBoundaryStep {
                    geometry: &boundary_geometry,
                    reference: &before_px,
                },
                initial_px: accepted.rendered,
                frame_before,
            },
        );
        let boundary = match boundary {
            Ok(value) => value,
            Err(refusal) => {
                owned.remove();
                report.recipe.rationale.truncate(rationale_before_attach);
                report.notes.truncate(notes_before_attach);
                report.recipe.masks.truncate(first_mask);
                let why = match refusal.why {
                    spatial::BitmapBoundaryWhy::Frame => FreeMaskWhy::Frame,
                    spatial::BitmapBoundaryWhy::Rim => FreeMaskWhy::Rim,
                    spatial::BitmapBoundaryWhy::Unmeasured => FreeMaskWhy::Unmeasured,
                    spatial::BitmapBoundaryWhy::Inert => FreeMaskWhy::Inert,
                };
                push_refusal(report, &[FreeMaskRefusal { n: number, why }]);
                disclosed += 1;
                continue;
            }
        };
        let frame_after = fit::look_err_with_evidence(&boundary.pixels, &target_px, &report.evidence);
        report.err_after = frame_after;
        crate::rationale::push_note(
            &mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(crate::rationale::keys::FIELD_MASK_ATTACHED, vec![
                ("n", number.to_string()), ("err_before", format!("{frame_before:.6}")),
                ("err_after", format!("{frame_after:.6}")),
                ("step", format!("{:.5}", boundary.reading.rim)),
            ]),
        );
        disclosed += 1;
        attached_count += 1;
        let _path = owned.into_path();
    }
    report.correspondence = corr;
    if attached_count == 0 {
        crate::rationale::push_note(
            &mut report.recipe.rationale, &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::FIELD_MASK_NONE,
                vec![("why", "all-refused".into())],
            ),
        );
    }
    let final_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    report.err_after = fit::look_err_with_evidence(&final_px, &target_px, &report.evidence);
    // With no accepted mask the previous producer already wrote the finished
    // disclosure for this unchanged recipe; repeating it only spends the
    // bounded rationale budget. An accepted mask needs the fresh terminal
    // reading to describe its composed result.
    if attached_count > 0 {
        fit::append_finished_disclosure(report, &final_px, &target_px);
    }
    let stage = FreeMaskStage { ran: true, components, disclosed };
    super::record_stage(stage);
    stage
}
