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
    /// The Oklab hue angle (radians) the pairs pushed along — the quantity
    /// the read actually measures; `hue` is the wheel position that renders it.
    pub(super) direction: f32,
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
/// The direction goes back to Lightroom's wheel through
/// [`lightroom_hue_of`], and the length through `render::TINT_OKLAB_PER_UNIT`.
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
    let amount = (v[0].hypot(v[1]) / render::TINT_OKLAB_PER_UNIT).min(1.0);
    let direction = v[1].atan2(v[0]);
    (amount >= TINT_MIN_AMOUNT).then_some(ZoneTint {
        hue: lightroom_hue_of(direction),
        amount,
        direction,
        pairs: samples.len() * stride,
        neutral: neutral * stride,
    })
}

/// The Lightroom hue (0..360, a tenth of a degree) whose swatch pushes along
/// Oklab hue angle `theta` (radians): the render's own wheel
/// (`render::tint_direction`) read backwards on a 3600-entry table, nearest
/// angle wins. A table rather than a bisection because the wheel is not
/// monotone in Oklab — pure blue's neighbourhood folds back by 0.15° between
/// HSV 232° and 240° — and on that plateau (HSV 222–242° within 1°) any
/// entry renders the same push.
pub(super) fn lightroom_hue_of(theta: f32) -> f32 {
    let mut best = (f32::INFINITY, 0.0f32);
    for i in 0..3600 {
        let hue = i as f32 / 10.0;
        let d = render::tint_direction(hue);
        let off = (d[1].atan2(d[0]) - theta + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        if off.abs() < best.0 {
            best = (off.abs(), hue);
        }
    }
    best.1
}

/// A semantic zone's colour, solved two ways and kept the better.
///
/// The PLAIN solve is [`attach_one_zone`]'s per-channel gains and saturation
/// chase. The TINT trial reads one mask Color tint from every paired pixel
/// after the tone solve ([`read_zone_tint`]), then solves the gains and the
/// saturation on the TINTED candidate — what is left after the tint, not
/// the whole gap — and goes through the very same function, so the same
/// probes, voucher, quality, acceptance and frame gates judge it. That order
/// is also why the tint cannot absorb what gains explain: on a gains-shaped
/// gap the tinted solve lands where the plain one does at best, and the
/// margin below keeps the plain one. It REPLACES the plain solve only when it shipped
/// a tint and passes the subzone bands' own replacement rule against the
/// plain solve: the zone's mean ΔE to its target lower by their margin, and
/// the frame within the zone's own regression tolerance of what the plain
/// solve left. Otherwise the plain solve stands exactly as
/// it would have without the trial: its recipe, notes and running frame
/// error are restored to the byte.
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
    (report.recipe, report.notes, *frame_err) = entry;
    let tinted = attach_one_zone_as(
        s_img, tgt_px, report, frame_err, attachment, divergence, corr, ZoneColour::Tint,
    );
    // The zone's own pixels as `attach_one_zone` reads them: its weights
    // where the target can pair (the v1.16.0 presence), nowhere else.
    let sw = report.evidence.present_weights(&attachment.source_weights);
    let sw = &*sw;
    let plain_de = subzones::delta_e(plain.as_ref().map_or(&entry_px, |z| &z.rendered), tgt_px, sw);
    let wins = tinted.as_ref().is_some_and(|zone| {
        report.recipe.masks[zone.mask_index].toning_amount > 0.0
            && subzones::delta_e(&zone.rendered, tgt_px, sw) < plain_de - ZONE_GLOBAL_REGRESSION_TOL
            && *frame_err <= plain_exit.2 + attachment.frame_regression_tol
    });
    if wins {
        return tinted;
    }
    (report.recipe, report.notes, *frame_err) = plain_exit;
    plain
}
