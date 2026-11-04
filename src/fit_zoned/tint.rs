//! The zone colour tint: read from a zone's paired pixels, and the transactional trial that lets it replace the plain colour solve.

use super::*;
use super::one_zone::{ZoneColour, attach_one_zone_as};

/// What a zone's paired pixels say about ONE mask Color tint
/// (`render::tint_px`): the Lightroom hue it pushes toward, its amount on
/// Lightroom's 0..1 scale, and the population it was read from.
#[derive(Clone, Copy, Debug)]
pub(super) struct ZoneTint {
    pub(super) hue: f32,
    pub(super) amount: f32,
    pub(super) pairs: usize,
    pub(super) neutral: usize,
}

/// One slider unit of `toning_amount` (0.01 of Lightroom's 0..1): at the
/// window's centre that is 0.0011 of Oklab chroma, under one 8-bit code on
/// any pixel, so a read below it has no tint to ship.
const TINT_MIN_AMOUNT: f32 = 0.01;

/// The disclosure's line between a neutral and a coloured pixel, HSV
/// saturation — the line the cloud block of the sky measurement reads.
const TINT_NEUTRAL_SAT: f32 = 0.12;

/// The estimator reads at most this many pairs (a deterministic stride over
/// the zone): the solve is two parameters, and a preview-sized zone holds
/// ten times as many pixels as it needs to pin them.
const TINT_MAX_PAIRS: usize = 40_000;

/// Read one tint from EVERY paired pixel of a zone, neutral and coloured
/// alike — the one colour move whose shape is the same on both, which is
/// why it is read from both rather than from the zone's mean.
///
/// The render model is linear in the tint: at a pixel it moves Oklab (a, b)
/// by `k · ramp · u`, `ramp` the lightness window at the candidate's own luma
/// times the mask's coverage there (a pixel the mask half covers moves half
/// as far), `u` the swatch's unit direction and `k` its strength. So the
/// pairs' Oklab (a, b) gaps `d = target − candidate` answer it in closed
/// form: the least-squares vector `v = Σ ρ·ramp·d / Σ ρ·ramp²` has the
/// direction `u` and the length `k` — the amount along the ramp, with the
/// pixels the window opens widest counting most. `weights` is the zone's own
/// robust pairing weight per pixel (ρ).
///
/// Iteratively reweighted on top: a pair the tint cannot explain — a
/// soft-edge blend of the zone with whatever lies past it, a highlight that
/// clipped on one side, a pixel the tone solve left far off — has a gap of
/// another shape, and at full weight that leverage turns the hue. Huber's
/// weight at his own 1.345 on the MAD scale of the per-pair misfit keeps
/// every pair that fits the model at full weight; three passes settle it.
///
/// The vector goes back to Lightroom's hue and amount through
/// `render::tint_from_push`, the measured wheel inverted.
pub(super) fn read_zone_tint(
    rendered: &[[f32; 3]],
    target: &[[f32; 3]],
    weights: &dyn Fn(usize) -> f32,
    coverage: &dyn Fn(usize) -> f32,
) -> Option<ZoneTint> {
    let ok = |p: &[f32; 3]| render::linear_to_oklab(p.map(render::srgb_to_linear));
    let paired: Vec<usize> = (0..rendered.len().min(target.len()))
        .filter(|&i| weights(i) > 0.0)
        .collect();
    let stride = paired.len().div_ceil(TINT_MAX_PAIRS).max(1);
    // (gap in Oklab (a, b), ramp, pairing weight)
    let mut samples: Vec<([f32; 2], f32, f32)> = Vec::new();
    let mut neutral = 0usize;
    for &i in paired.iter().step_by(stride) {
        let px = rendered[i];
        let ramp = render::tint_window(render::tint_luma(&px)) * coverage(i).clamp(0.0, 1.0);
        if ramp <= 1e-3 {
            continue;
        }
        let (mx, mn) = (px[0].max(px[1]).max(px[2]), px[0].min(px[1]).min(px[2]));
        if (mx - mn) / mx.max(1e-6) < TINT_NEUTRAL_SAT {
            neutral += 1;
        }
        let (c, t) = (ok(&px), ok(&target[i]));
        samples.push(([t[1] - c[1], t[2] - c[2]], ramp, weights(i)));
    }
    if samples.is_empty() {
        return None;
    }
    let solve = |rw: &[f32]| -> [f32; 2] {
        let (mut num, mut den) = ([0.0f64; 2], 0.0f64);
        for ((d, ramp, rho), h) in samples.iter().zip(rw) {
            let w = (*rho * *h * *ramp) as f64;
            num[0] += w * d[0] as f64;
            num[1] += w * d[1] as f64;
            den += w * *ramp as f64;
        }
        if den <= 0.0 { [0.0; 2] } else { [(num[0] / den) as f32, (num[1] / den) as f32] }
    };
    let mut rw = vec![1.0f32; samples.len()];
    let mut v = solve(&rw);
    for _ in 0..3 {
        let misfit: Vec<f32> = samples
            .iter()
            .map(|(d, ramp, _)| (d[0] - ramp * v[0]).hypot(d[1] - ramp * v[1]))
            .collect();
        let mut sorted = misfit.clone();
        sorted.sort_by(f32::total_cmp);
        let scale = 1.4826 * sorted[sorted.len() / 2];
        if scale <= 1e-9 {
            break;
        }
        for (w, e) in rw.iter_mut().zip(&misfit) {
            *w = if *e <= 1.345 * scale { 1.0 } else { 1.345 * scale / e };
        }
        v = solve(&rw);
    }
    let (hue, amount) = render::tint_from_push(v);
    let amount = amount.min(1.0);
    (amount >= TINT_MIN_AMOUNT).then_some(ZoneTint {
        hue,
        amount,
        pairs: samples.len() * stride,
        neutral: neutral * stride,
    })
}

/// A semantic zone's colour, solved three ways and kept the best.
///
/// The PLAIN solve is [`attach_one_zone`]'s per-channel gains and saturation
/// chase. Two TINT trials read one mask Color tint from every paired pixel
/// after the tone solve ([`read_zone_tint`]) and differ only in what the
/// zone-mean gains are asked:
///
/// * [`ZoneColour::Tint`] reads the tint FIRST, on the zone as its tone left
///   it, and then solves the gains and the saturation on the TINTED
///   candidate — the order for a gap that is a tint and nothing else, where
///   gains solved first would take the mean of it and leave the tint only
///   the difference between the two populations;
/// * [`ZoneColour::TintOnGains`] keeps the plain solve's gains, read before
///   the tone as always, and reads the tint on what THEY leave — the order
///   for a gap that is a cast plus a tint. Measured on the seaside street
///   (2026-10-27): the fit's own global stage leaves that sky 35 b* warmer
///   than its target, read first the tint swallowed the cast and stopped at
///   Lightroom's end stop (amount 1.00) with weaker gains behind it, and the
///   finished sky and clouds both landed further from the target than the
///   plain solve's; read after the gains it came out at 0.50 and the frame,
///   the sky and the clouds all landed closer. It runs ONLY when the
///   tint-first read is at that end stop, which is what says one tint cannot
///   explain the gap; below it, a tint read on what plain gains leave is a
///   residual chase that beat the margin with small tints on gains-shaped
///   zones (three of the pinned zone fixtures, 2026-10-27).
///
/// Each trial goes through the very same function as the plain solve, so the
/// same probes, voucher, quality, acceptance and frame gates judge it. A
/// trial counts only when it shipped a tint and passes the subzone bands'
/// own replacement rule against the plain solve: the zone's mean ΔE to its
/// target lower by their margin, and the frame within the zone's own
/// regression tolerance of what the plain solve left. Of the trials that
/// count, the one with the lower zone ΔE replaces the plain solve; when none
/// does, the plain solve stands exactly as it would have without them — its
/// recipe, notes and running frame error are restored to the byte. The tint
/// cannot take what the gains explain: tint-first re-solves the gains under
/// it and on-gains keeps them, and in both the band veto still judges the
/// gains exactly as on the plain solve. What a trial can add on a
/// gains-shaped gap is a residual tint on top of the same gains, and only
/// where it lands the zone closer: measured on the pinned warm-recolour zone
/// (2026-10-27), on-gains shipped the plain gains plus a 0.03 tint, zone ΔE
/// 4.42 → 4.17.
pub(super) fn attach_zone_tint_or_plain(
    s_img: &DynamicImage,
    tgt_px: &[[f32; 3]],
    report: &mut FitReport,
    frame_err: &mut f32,
    attachment: &ZoneAttachment,
    divergence: Option<fit::Divergence>,
    corr: Option<&fit::PairCorrespondence>,
) -> Option<AcceptedZone> {
    let entry = (report.recipe.clone(), report.notes.clone(), *frame_err);
    let entry_px = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    let plain = attach_one_zone(s_img, tgt_px, report, frame_err, attachment, divergence, corr);
    let plain_exit = (report.recipe.clone(), report.notes.clone(), *frame_err);
    // The zone's own pixels as `attach_one_zone` reads them: its weights
    // where the target can pair, nowhere else.
    let sw = report.evidence.present_weights(&attachment.source_weights).into_owned();
    let plain_de = subzones::delta_e(plain.as_ref().map_or(&entry_px, |z| &z.rendered), tgt_px, &sw);
    type Trial = (f32, Option<AcceptedZone>, (crate::recipe::EditRecipe, Vec<crate::rationale::Note>, f32));
    let mut best: Option<Trial> = None;
    // On-gains runs only when the tint-first read hit Lightroom's end stop:
    // a tint alone that cannot explain the gap within the slider's travel is
    // a cast plus a tint, and the cast is the gains' to take. Below the stop
    // the tint-first read already explains the gap, and an on-gains read of
    // what plain gains leave is a residual chase — small tints that beat the
    // margin on gains-shaped zones and replace a solve that was right.
    let mut tint_first_read = None;
    for colour in [ZoneColour::Tint, ZoneColour::TintOnGains] {
        if colour == ZoneColour::TintOnGains && tint_first_read.is_none_or(|a: f32| a < 1.0) {
            break;
        }
        (report.recipe, report.notes, *frame_err) = entry.clone();
        let read_out = if colour == ZoneColour::Tint { &mut tint_first_read } else { &mut None };
        let tinted = attach_one_zone_as(s_img, tgt_px, report, frame_err, attachment, divergence, corr, colour, read_out);
        let Some(zone) = tinted.as_ref() else { continue };
        let de = subzones::delta_e(&zone.rendered, tgt_px, &sw);
        let counts = report.recipe.masks[zone.mask_index].toning_amount > 0.0
            && de < plain_de - ZONE_DELTA_E_MARGIN
            // S-MODE (v1.18.0): this is a LOCAL correction's do-no-harm, so it reads
            // E_paired + H_ab (`report.evidence.local`): S is read where the question
            // is structure, a local correction cannot change which pixels pair, its
            // seams stay with the boundary / ramp rulers and its structural gate is
            // the existing D threshold only.
            && *frame_err <= plain_exit.2 + attachment.frame_regression_tol;
        if counts && best.as_ref().is_none_or(|b| de < b.0) {
            best = Some((de, tinted, (report.recipe.clone(), report.notes.clone(), *frame_err)));
        }
    }
    if let Some((_, zone, state)) = best {
        (report.recipe, report.notes, *frame_err) = state;
        return zone;
    }
    (report.recipe, report.notes, *frame_err) = plain_exit;
    plain
}
