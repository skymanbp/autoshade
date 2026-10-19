//! Fitting one zone: modes and acceptance arms, moments and dials, the saturation step, gain shrinking, local quality, the shrink of a zone's corrections, and the zone error and luma CDF.

use super::*;

/// Per-zone policy selected from the same structural statistic as the global
/// solve, before any within-zone CDF fitting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneMode {
    Full,
    Atmosphere,
}

/// WHICH acceptance arm bought a zone. Only the third one is disclosed as a
/// typed note: the first two are the shipped behaviour and are already
/// described by [`ZONE_ATTACHED`](crate::rationale::keys::ZONE_ATTACHED),
/// while the third is the relaxation R30 batch 1 introduced and a reader has
/// to be able to tell that this correction would have been dropped before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ZoneAcceptArm {
    /// The zone error at most halved ([`ZONE_ACCEPT_RATIO`]).
    Halved,
    /// It landed in matched territory, brightness included, with a real gain
    /// ([`ZONE_MATCHED_ERR`], [`ZONE_MATCHED_EV`], [`ZONE_FLOOR_MIN_GAIN`]).
    MatchedFloor,
    /// It is strictly better by an absolute margin and the FRAME did not pay
    /// for it ([`ZONE_MIN_ABS_GAIN`]).
    StrictlyBetter,
    /// Atmosphere's own arm, which is not one of the three above: a bounded
    /// atmosphere move is judged only on not making its zone worse. Named
    /// rather than folded into [`Self::Halved`] so no reader concludes an
    /// atmosphere zone ever halved anything.
    AtmosphereDoNoHarm,
}

/// The zone-local acceptance predicate: halve the zone error, land it in
/// matched territory — brightness included — with a real gain, or be
/// strictly better by an absolute margin at no cost to the frame (see
/// [`ZONE_ACCEPT_RATIO`], [`ZONE_MATCHED_ERR`], [`ZONE_MATCHED_EV`],
/// [`ZONE_FLOOR_MIN_GAIN`] and [`ZONE_MIN_ABS_GAIN`]). Pure so the regimes
/// are unit-testable without an end-to-end fit.
///
/// The third arm (R30 batch 1) exists because the first two are RATIO
/// yardsticks with nothing absolute in them, so a correction that genuinely
/// works — the calibration land zone, 0.078 -> 0.054 with the frame moving
/// -0.00004 and every quality gate clear — was refused for landing at 69% of
/// its start rather than 50%. `ZONE_ACCEPT_RATIO`'s own doc calls itself a
/// yield gate whose safety is carried by the quality gates, the frame-drift
/// insurance and the rim gate; this arm keeps all three and pays for the
/// relaxation on the frame side, where it demands `frame_after <=
/// frame_before` — STRICTER than the [`ZONE_GLOBAL_REGRESSION_TOL`] the
/// semantic route otherwise allows, and equal to what the spatial, range and
/// free-mask routes already demand.
///
/// `frame_before` / `frame_after` are the caller's OWN running frame reading
/// and the candidate's, both `look_err_with_evidence` under
/// `report.evidence`: the report's single ruler, not a second one.
///
/// The quality gates are NOT a parameter: the caller has already returned on
/// their failure by the time this is asked, so `passes()` holds on every
/// call. One of them, [`ZONE_TEXTURE_MIN`], is known by its own doc comment
/// NOT to separate the saved generated-cloud correction from the accepted
/// zones, so this arm may not be described as standing behind three hard
/// safety gates — it stands behind two hard ones and one that is calibrated
/// but not discriminating.
pub(super) fn zone_accepts(
    zone_before: f32,
    zone_after: f32,
    ev_gap_after: f32,
    frame_before: f32,
    frame_after: f32,
) -> Option<ZoneAcceptArm> {
    if zone_after <= zone_before * ZONE_ACCEPT_RATIO {
        return Some(ZoneAcceptArm::Halved);
    }
    if zone_after <= ZONE_MATCHED_ERR
        && ev_gap_after <= ZONE_MATCHED_EV
        && zone_after <= ZONE_FLOOR_MIN_GAIN * zone_before
    {
        return Some(ZoneAcceptArm::MatchedFloor);
    }
    if zone_after < zone_before - ZONE_MIN_ABS_GAIN && frame_after <= frame_before {
        return Some(ZoneAcceptArm::StrictlyBetter);
    }
    None
}

/// The skip decision, pure for the same reason: a zone at/below the
/// observed matched domain — brightness included — is left alone with the
/// honest note instead of being dialled.
pub(super) fn zone_skips(zone_before: f32, ev_gap: f32) -> bool {
    zone_before <= ZONE_SKIP_ERR && ev_gap <= ZONE_MATCHED_EV
}

/// Mask-weighted first moments of one zone.
pub(crate) struct ZoneMoments {
    /// Weighted mean Rec.601 luma of the LINEAR-light channels (EV math needs
    /// linear; the engine's exact transfer curve via `srgb_to_linear`).
    pub luma_lin: f32,
    /// Weighted mean per-channel LINEAR values (colour gains act here).
    pub mean_lin: [f32; 3],
    /// Weighted mean HSV-style chroma (max−min) in sRGB — the same definition
    /// the global fit's `mean_chroma` uses, so the two stages agree on what
    /// "saturated" means.
    pub chroma: f32,
    /// Weighted zone share of the frame, Σw / n.
    pub share: f32,
}

/// Moments of the zone selected by `weights` (one weight per pixel, [0,1] —
/// a decoded segmentation mask, or anything else). Zero-weight pixels cost
/// nothing. A degenerate mask (Σw ≈ 0) returns `share == 0.0` and neutral
/// moments — callers gate on [`MIN_ZONE_SHARE`] anyway.
pub(crate) fn zone_moments(px: &[[f32; 3]], weights: &[f32]) -> ZoneMoments {
    debug_assert_eq!(px.len(), weights.len());
    let mut w_total = 0.0f64;
    let mut luma = 0.0f64;
    let mut mean = [0.0f64; 3];
    let mut chroma = 0.0f64;
    for (p, &w) in px.iter().zip(weights) {
        if w <= 0.0 {
            continue;
        }
        let w = w as f64;
        w_total += w;
        let lin = [
            render::srgb_to_linear(p[0]),
            render::srgb_to_linear(p[1]),
            render::srgb_to_linear(p[2]),
        ];
        luma += w * (0.299 * lin[0] + 0.587 * lin[1] + 0.114 * lin[2]) as f64;
        for c in 0..3 {
            mean[c] += w * lin[c] as f64;
        }
        chroma += w * (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) as f64;
    }
    if w_total <= 0.0 {
        return ZoneMoments { luma_lin: 0.0, mean_lin: [0.0; 3], chroma: 0.0, share: 0.0 };
    }
    ZoneMoments {
        luma_lin: (luma / w_total) as f32,
        mean_lin: [
            (mean[0] / w_total) as f32,
            (mean[1] / w_total) as f32,
            (mean[2] / w_total) as f32,
        ],
        chroma: (chroma / w_total) as f32,
        share: (w_total / px.len().max(1) as f64) as f32,
    }
}

/// The coarse zone correction: local exposure + the exact per-channel linear
/// gains the engine's mask stage renders. Saturation is deliberately NOT
/// here — it is closed-loop by construction (see [`zone_sat_step`]).
pub(crate) struct ZoneDials {
    pub exposure_ev: f32,
    pub color_gains: [f32; 3],
}

/// Solve the dials that move the source zone's moments onto the target
/// zone's. Pure moment math, no renders, and EXACT for the moments by
/// construction:
///
/// * **exposure** — linear-luma ratio in EV. Brightness rides the local tone
///   LUT (soft shoulder) instead of a raw gain, so bright zone texture rolls
///   off instead of clipping.
/// * **color_gains** — the remaining brightness-normalised per-channel
///   demand `(tgt/src) / 2^EV` in linear light: exactly the ratios the
///   engine multiplies in (`apply_masks`), exactly what a WB dial cannot
///   express (see the module doc).
pub(crate) fn fit_zone_dials(src: &ZoneMoments, tgt: &ZoneMoments) -> ZoneDials {
    let exposure_ev = (tgt.luma_lin.max(1e-5) / src.luma_lin.max(1e-5))
        .log2()
        .clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT);
    let bright = 2.0f32.powf(exposure_ev);
    let mut color_gains = [1.0f32; 3];
    for (c, gain) in color_gains.iter_mut().enumerate() {
        let want = tgt.mean_lin[c].max(1e-5) / src.mean_lin[c].max(1e-5);
        // Same legal range recipe::clamp enforces (0 would kill a channel).
        *gain = (want / bright).clamp(0.05, 8.0);
    }
    ZoneDials { exposure_ev, color_gains }
}

/// One closed-loop saturation step: the same mean-chroma chase as the global
/// fit's stage 3 (per-step ±40; the caller clamps the accumulated value with
/// [`clamp_zone_sat`]), fed with the zone chroma MEASURED on a real render of
/// the current recipe — open-loop chroma math after a recolour is not
/// trustworthy (the gains change chroma by themselves). Returns the step to
/// ADD to the current local saturation; `None` when converged (< 1 point) or
/// when the zone carries no chroma evidence.
pub(crate) fn zone_sat_step(cur_chroma: f32, tgt_chroma: f32) -> Option<f32> {
    if cur_chroma < 1e-4 {
        return None;
    }
    let step = ((tgt_chroma / cur_chroma - 1.0) * 100.0).clamp(-40.0, 40.0);
    if step.abs() < 1.0 {
        return None;
    }
    Some(step)
}

/// Clamp an accumulated local saturation to the zone model cap.
pub(crate) fn clamp_zone_sat(v: f32) -> f32 {
    v.clamp(-ZONE_SAT_LIMIT, ZONE_SAT_LIMIT)
}

pub(super) fn clamp_zone_sat_for_mode(v: f32, mode: ZoneMode) -> f32 {
    match mode {
        ZoneMode::Full => clamp_zone_sat(v),
        ZoneMode::Atmosphere => v.clamp(-ZONE_ATMOS_SAT_LIMIT, ZONE_ATMOS_SAT_LIMIT),
    }
}

/// Shrink one atmosphere-zone recolour toward unity with a single scalar, so
/// every channel keeps the fitted direction and the ratios are not independently
/// clipped into a different hue.
///
/// The window is the strength budget's, not a constant: a repainted sky is
/// precisely the zone a user raising Strength is asking about, and it was the
/// one control in the solve the dial could not reach.
pub(super) fn shrink_atmosphere_gains_in(gains: [f32; 3], window: (f32, f32)) -> [f32; 3] {
    let mut k = 1.0f32;
    for gain in gains {
        if gain > 1.0 {
            k = k.min((window.1 - 1.0) / (gain - 1.0));
        } else if gain < 1.0 {
            k = k.min((1.0 - window.0) / (1.0 - gain));
        }
    }
    gains.map(|gain| 1.0 + k.clamp(0.0, 1.0) * (gain - 1.0))
}

/// R36. Bisection steps of the vouched-share search: 1/64 of the solved move
/// is the resolution, six analysis renders the cost.
pub(super) const ZONE_VOUCH_SHRINK_STEPS: usize = 6;

/// R36. A share `k` of a zone's solved COLOUR move, direction kept: every
/// gain shrinks toward unity the way `shrink_atmosphere_gains_in` shrinks it
/// (a linear share of each channel's deviation, so the ratios are not clipped
/// into a different hue) and the saturation step shrinks with it, both rounded
/// the way the solve rounds them so the probe IS what would ship.
pub(super) fn scale_zone_colour(m: &mut LocalAdjustment, k: f32, mode: ZoneMode) {
    if let Some(gains) = m.color_gains {
        m.color_gains = Some(gains.map(|gain| ((1.0 + k * (gain - 1.0)) * 100.0).round() / 100.0));
    }
    m.saturation = clamp_zone_sat_for_mode((m.saturation * k).round(), mode);
}

/// The tone half of [`scale_zone_colour`]: the six tone dials at a share `k`,
/// rounded as solved.
pub(super) fn scale_zone_tone(m: &mut LocalAdjustment, k: f32) {
    m.exposure_ev = (m.exposure_ev * k * 100.0).round() / 100.0;
    for dial in [&mut m.contrast, &mut m.highlights, &mut m.shadows, &mut m.whites, &mut m.blacks] {
        *dial = (*dial * k * 10.0).round() / 10.0;
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LocalQuality {
    pub(super) texture_ratio: f32,
    pub(super) clipped_before: f32,
    pub(super) clipped_after: f32,
}

impl LocalQuality {
    pub(super) fn texture_passes(self) -> bool {
        (ZONE_TEXTURE_MIN..=ZONE_TEXTURE_MAX).contains(&self.texture_ratio)
    }

    pub(super) fn clipping_passes(self) -> bool {
        self.clipped_after <= self.clipped_before + ZONE_CLIP_GROWTH
    }

    pub(super) fn passes(self) -> bool {
        self.texture_passes() && self.clipping_passes()
    }
}

/// Local-quality reading shared by every zone and mode. Texture is the
/// mask-weighted mean magnitude of the forward Rec.601-luma gradient PER UNIT
/// of the zone's mask-weighted mean luma; clipping is the weighted share at
/// <=1/255 or >=254/255.
///
/// Per unit of brightness because the gate guards CONTRAST, not exposure.
/// Until v1.12.0 the ratio read the absolute gradient, and an exposure move
/// scales every gradient with the pixels it scales: a zone lifted `k` times
/// read `k` however untouched its texture, so the 1.95 ceiling was a hidden
/// ~1 EV cap underneath the zone's own [`ZONE_EV_LIMIT`]. On a night frame
/// whose generated target lit the dark ground, the land zone's solved lift
/// read 2.37 and was dropped, and the colour field — which this gate does not
/// guard — then chased the same lift alone, pinned at its bounds: half the
/// brightness at the source's red cast. Normalised, a pure lift reads 1 and
/// contrast amplification or crushing about an unchanged mean reads exactly
/// what it read before.
pub(super) fn local_quality(
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    weights: &[f32],
    width: u32,
    height: u32,
) -> LocalQuality {
    let (w, h) = (width as usize, height as usize);
    let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    // (mean gradient, mean luma, clipped share), all mask-weighted.
    let reading = |px: &[[f32; 3]]| -> (f64, f64, f32) {
        let mut total = 0.0f64;
        let mut texture = 0.0f64;
        let mut level = 0.0f64;
        let mut clipped = 0.0f64;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let weight = weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
                if weight == 0.0 || i >= px.len() {
                    continue;
                }
                let here = luma(&px[i]);
                let dx = if x + 1 < w && i + 1 < px.len() { luma(&px[i + 1]) - here } else { 0.0 };
                let dy = if y + 1 < h && i + w < px.len() { luma(&px[i + w]) - here } else { 0.0 };
                texture += weight * (dx * dx + dy * dy).sqrt() as f64;
                level += weight * here as f64;
                clipped += weight * (here <= 1.0 / 255.0 || here >= 254.0 / 255.0) as u8 as f64;
                total += weight;
            }
        }
        if total <= 1e-12 {
            (0.0, 0.0, 0.0)
        } else {
            (texture / total, level / total, (clipped / total) as f32)
        }
    };
    let ((texture_before, level_before, clipped_before), (texture_after, level_after, clipped_after)) =
        (reading(before), reading(after));
    // A zone black on either side has no brightness scale to divide out; the
    // pair is then compared as it always was, in absolute gradient.
    let (texture_before, texture_after) = if level_before > 1e-6 && level_after > 1e-6 {
        (texture_before / level_before, texture_after / level_after)
    } else {
        (texture_before, texture_after)
    };
    let texture_ratio = if texture_before <= 1e-8 {
        if texture_after <= 1e-8 { 1.0 } else { f32::INFINITY }
    } else {
        (texture_after / texture_before) as f32
    };
    LocalQuality {
        texture_ratio,
        clipped_before,
        clipped_after,
    }
}

/// Apply one scalar to every correction in the accepted zone set. Each dial
/// is decomposed into its source-share-weighted common component plus its
/// per-zone differential; BOTH terms carry `k`, because `k=0` is required to
/// be no local correction (holding the common term would leave a full-frame
/// masked correction). Thus additive dials land at zero and gains at unity,
/// every zone keeps its fitted direction, and `k=1` is byte-for-byte the
/// candidate. The decomposition makes the common policy explicit even though
/// `k*c + k*(v-c)` deliberately simplifies to `k*v`. The gain form sums that
/// differential BEFORE adding the unity offset, so a channel whose colour was
/// withheld (`v == 0`) stays at exactly unity under every `k`
/// (`a_withheld_channel_is_exactly_unity_under_every_shrink`).
pub(super) fn shrink_zone_corrections(
    masks: &mut [LocalAdjustment],
    originals: &[LocalAdjustment],
    shares: &[f32],
    k: f32,
) {
    debug_assert_eq!(masks.len(), originals.len());
    debug_assert_eq!(masks.len(), shares.len());
    let k = k.clamp(0.0, 1.0);
    let share_total = shares.iter().copied().sum::<f32>().max(1e-6);
    macro_rules! shrink_additive {
        ($field:ident) => {{
            let common = originals
                .iter()
                .zip(shares)
                .map(|(m, share)| m.$field * *share)
                .sum::<f32>()
                / share_total;
            for ((dst, src), _) in masks.iter_mut().zip(originals).zip(shares) {
                dst.$field = k * common + k * (src.$field - common);
            }
        }};
    }
    shrink_additive!(exposure_ev);
    shrink_additive!(contrast);
    shrink_additive!(highlights);
    shrink_additive!(shadows);
    shrink_additive!(whites);
    shrink_additive!(blacks);
    shrink_additive!(saturation);
    for channel in 0..3 {
        let common = originals
            .iter()
            .zip(shares)
            .map(|(m, share)| (m.color_gains.unwrap_or([1.0; 3])[channel] - 1.0) * *share)
            .sum::<f32>()
            / share_total;
        for ((dst, src), _) in masks.iter_mut().zip(originals).zip(shares) {
            let fitted = src.color_gains.unwrap_or([1.0; 3])[channel] - 1.0;
            let gains = dst.color_gains.get_or_insert([1.0; 3]);
            // Differential first, offset last: `k*c` and `k*(0 - c)` are exact
            // negatives, so a withheld channel cancels to 1.0 exactly, where
            // `(1.0 + k*c) + k*(0 - c)` read 0.99999994 at k·c ≈ 0.3 (CI on the
            // 2026-09-24 close-out, the orchestration test's exact pin).
            gains[channel] = 1.0 + (k * common + k * (fitted - common));
        }
    }
    if k == 0.0 {
        for mask in masks {
            mask.color_gains = None;
        }
    }
}

/// Zone-local look distance: mean |Δ| of the linear channel means plus the
/// chroma gap — the moments the fit steers, measured where the mask acts.
/// This is the yardstick the zoned do-no-harm judges by (see
/// [`ZONE_ACCEPT_RATIO`] for why the frame-global `look_err` cannot be).
pub(crate) fn zone_err(a: &ZoneMoments, b: &ZoneMoments) -> f32 {
    let mean: f32 =
        a.mean_lin.iter().zip(&b.mean_lin).map(|(x, y)| (x - y).abs()).sum::<f32>() / 3.0;
    mean + (a.chroma - b.chroma).abs()
}

/// Mask-weighted luma CDF of a zone (sRGB Rec.601 luma, the same domain as
/// the global fit's tone stage). Drives the WITHIN-zone tone solve: a zone
/// can match the target's linear MEAN and still read far darker (the real
/// pair's land: the target holds sunlit mesa tops plus deep canyon shadows —
/// a few bright pixels dominate a linear mean, while perception follows the
/// distribution). Zones correspond semantically, so quantile-to-quantile
/// mapping is identified here — unlike the per-band statistics fit.rs bans
/// on the WHOLE frame, where region correspondence is unknown.
pub(crate) fn zone_luma_cdf(px: &[[f32; 3]], weights: &[f32]) -> Vec<f32> {
    const BINS: usize = 1024;
    let mut hist = vec![0.0f32; BINS];
    let mut total = 0.0f32;
    for (p, &w) in px.iter().zip(weights) {
        if w <= 0.0 {
            continue;
        }
        let l = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        hist[(l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize] += w;
        total += w;
    }
    let total = total.max(1e-6);
    let mut acc = 0.0f32;
    for h in hist.iter_mut() {
        acc += *h;
        *h = acc / total;
    }
    hist
}
