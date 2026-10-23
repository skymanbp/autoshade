//! The fit budget and options: the strength-scaled budget, the veto policy, the white-balance search, and the global cast detector.

use super::*;

/// Analysis resolution (long edge). CDFs and band means are stable well below
/// this; keeping it small keeps the closed-loop renders interactive-fast
/// (5 in the common path; up to ~20 if the do-no-harm loop shrinks
/// saturation, each a 384-px develop).
pub(crate) const ANALYZE_EDGE: u32 = 384;
pub(super) const HIST_BINS: usize = 1024;
/// Global structural-divergence threshold. Calibration on same-content pairs:
/// showcase 1/2/3 = 0.075/0.168/0.095, viaduct = 0.070 and sunset = 0.226;
/// the generated-cloud failure is 0.491. `pub` (not `pub(crate)`): the GUI
/// bin crate quotes the same number in the generation-side fidelity
/// disclosure, so the threshold has exactly one definition.
pub const DIVERGENCE_GLOBAL: f32 = 0.35;
/// Per-zone structural-divergence threshold. The same-content top-35% strips
/// peak at 0.532, while the generated-cloud sky is 1.186 (land = 0.436).
pub(crate) const DIVERGENCE_ZONE: f32 = 0.65;
/// A divergent semantic partition covering this source-frame share promotes
/// the global solve to Atmosphere mode. The failing sky covers 44.36%.
pub(crate) const DIVERGENT_COVER_PROMOTES: f32 = 0.35;
/// The low-pass scale of the COARSE structural reading, as a divisor of the
/// analysis raster's long edge: sigma = 384 / 48 = 8 px at [`ANALYZE_EDGE`].
/// Eight pixels is two [`structure_divergence`] bands above the sigma-2
/// gradient pooling and half its widest band.
///
/// WHAT IT IS NOT, measured before it was wired to anything (2026-09-10, the
/// user's `reimagine-2` pair against its own 2048-px neutral develop). The
/// coarse reading was built on the hypothesis that a repaint which keeps the
/// horizon, the ridge and the building where they were would read LOWER at
/// layout scale than at pixel scale, so cell and population statistics could
/// be admitted on it where the fine reading refuses. The pair says the
/// opposite, at every scope:
///
/// | scope | D fine | D coarse |
/// |---|---|---|
/// | frame | 0.275 | 0.609 |
/// | sky zone (44.4% cover) | 0.620 | 0.959 |
/// | land zone | 0.271 | 0.546 |
/// | mean 3x3 cell support | 0.585 | 0.454 |
/// | mean 12x8 cell support | 0.417 | 0.413 |
///
/// The reason is in the instrument: luma is rank-equalised against each
/// image's OWN histogram, so removing the fine texture that both frames still
/// share leaves the broad regional luma the repaint actually moved — a grey
/// sky turned orange re-orders the ranks between sky and land, and that IS
/// the residual the fit exists to close. A low-pass makes the statistic more
/// sensitive to the edit, not less. So the mode line stays on the FINE
/// reading and no evidence gate was moved onto this one; it is measured and
/// DISCLOSED beside the fine number ([`crate::rationale::keys::FIT_NOTE_PAIRING_PIXEL`])
/// so the two scales' disagreement is on the record rather than assumed.
pub(super) const COARSE_SIGMA_DIVISOR: f32 = 48.0;
/// How close to [`DIVERGENCE_GLOBAL`] the fine reading has to be before the
/// report says the mode was a near thing. There is no hysteresis and no dead
/// band on the mode line — D = 0.3499 and D = 0.3501 select different solvers
/// — so the honest substitute for a tie-break is to say when the tie happened.
pub(super) const MODE_MARGIN_DISCLOSED: f32 = 0.05;
/// Independent cap for every residual tone-curve segment. The three showcase
/// curves peak at 1.762/1.905/1.762; the generated-cloud failure reached 4.52.
pub(super) const RESIDUAL_SLOPE_CAP: f32 = 2.0;

pub(super) const ATMOSPHERE_EV_LIMIT: f32 = 1.0;
pub(super) const ATMOSPHERE_SAT_LIMIT: f32 = 30.0;
pub(super) const ATMOSPHERE_WB_GAIN_MIN: f32 = 0.80;
pub(super) const ATMOSPHERE_WB_GAIN_MAX: f32 = 1.25;
pub(super) const ATMOSPHERE_WB_GAIN_RATIO: f32 = 1.40;
pub(super) const ATMOSPHERE_CURVE_SLOPE_MIN: f32 = 0.5;
pub(super) const ATMOSPHERE_CURVE_SLOPE_MAX: f32 = 1.5;
pub(super) const ATMOSPHERE_CONFIDENCE_CAP: f32 = 0.50;
/// Per-band colour-mixer ceiling on the recipe's own +/-100 axis, at Strength
/// 0 / the shipped default / Strength 1. Deliberately far below the global
/// saturation budget: a band move is applied to a SUB-population that the
/// frame statistics can only see through eight coarse bins, so the default is
/// the size of a correction a user would call "the blues are a bit flat"
/// rather than a re-grade. The engine halves the luminance axis on its way in
/// (`render::apply_hsl`), so that axis is the gentler of the two by
/// construction and needs no second number.
pub(super) const HSL_BAND_LIMIT_MIN: f32 = 6.0;
pub(super) const HSL_BAND_LIMIT_DEFAULT: f32 = 18.0;
pub(super) const HSL_BAND_LIMIT_MAX: f32 = 45.0;
/// R34 §D4. Per-channel gain bound of the SHIPPED colour field, at Strength 0
/// / the shipped default / Strength 1.
///
/// The DEFAULT point is `fit_field::BOUNDS_HIGH`'s own 0.35, and that is not a
/// coincidence to be tidied away: the field never ships at or below the
/// default strength, so pinning the default point to the analyzer's constant
/// is what keeps the default recipe byte-identical while giving the dial
/// somewhere to go. Above it the bound opens because the demand is measured:
/// on the desert-dusk reference pair the repainted sky still wants R +0.465 in
/// linear gain after the whole zone ladder has run, and a field capped at 0.35
/// answers that by saturating 15 of the horizon row's vertices — a field that
/// has run out of range is not a measurement of what is left, it is a wall.
/// 0.80 at Strength 1 leaves headroom above the largest demand this corpus has
/// shown rather than being fitted to it.
const FIELD_GAIN_MIN: f32 = 0.35;
const FIELD_GAIN_DEFAULT: f32 = 0.35;
const FIELD_GAIN_MAX: f32 = 0.80;
/// Kelvin domain the WB search walks in log space; landing on either end is
/// disclosed above default strength (`FIT_NOTE_WB_SEARCH_BOUND`).
pub(super) const WB_SEARCH_K: (f32, f32) = (2000.0, 40000.0);

/// Whether unsupported population movement is withheld or disclosed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VetoPolicy {
    Withhold,
    Disclose,
}

/// Strength-governed honesty budget for the global reverse-fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitBudget {
    pub ev: f32,
    pub sat: f32,
    pub wb_gain: (f32, f32),
    pub wb_ratio: f32,
    /// Maximum weighted frame share that a WB correction may re-hue.
    pub wb_rotation_share: f32,
    pub cast_ratio: f32,
    pub slope: (f32, f32),
    pub confidence_cap: f32,
    /// Ceiling for ONE band of the per-band colour mixer, on the recipe's
    /// +/-100 axis. The strength dial has to be able to turn this stage, so
    /// it interpolates like every other budget dimension.
    pub hsl_band: f32,
    /// Window an ATMOSPHERE zone's per-channel recolour is shrunk into
    /// (`fit_zoned::shrink_atmosphere_gains`). It was a pair of fixed
    /// constants, so a user asking for Strength 1.0 on a repainted sky got
    /// the same [0.85, 1.18] a user asking for 0.0 did — the one place in the
    /// solve the strength dial could not reach, and the zone that most needed
    /// it (R33 §F).
    pub zone_gain: (f32, f32),
    /// R34 §D4. Per-channel gain bound of the colour field the recipe SHIPS
    /// (`fit_zoned::field::attach_colour_field`'s support-free pass). The
    /// ANALYSIS field — the ceiling, the shape verdicts, `LOCAL_STOP_MARGIN` —
    /// is never solved against this: it keeps `fit_field::BOUNDS_HIGH`, so the
    /// number the sequencer stops on is the number it has always been.
    pub field_gain: f32,
    pub vetoes: VetoPolicy,
}

impl FitBudget {
    pub fn for_strength(s: crate::recipe::GradeStrength) -> Self {
        let s = s.get().clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let between = |at_zero: f32, at_default: f32, at_full: f32| {
            if s <= crate::recipe::GradeStrength::DEFAULT {
                lerp(at_zero, at_default, s / crate::recipe::GradeStrength::DEFAULT)
            } else {
                lerp(
                    at_default,
                    at_full,
                    (s - crate::recipe::GradeStrength::DEFAULT)
                        / (1.0 - crate::recipe::GradeStrength::DEFAULT),
                )
            }
        };
        let wb_rotation_share = if s <= crate::recipe::GradeStrength::DEFAULT {
            ROT_SHARE
        } else {
            lerp(
                ROT_SHARE,
                1.0,
                (s - crate::recipe::GradeStrength::DEFAULT)
                    / (1.0 - crate::recipe::GradeStrength::DEFAULT),
            )
        };
        Self {
            ev: between(0.5, ATMOSPHERE_EV_LIMIT, 2.5),
            sat: between(15.0, ATMOSPHERE_SAT_LIMIT, 60.0),
            wb_gain: (between(0.90, ATMOSPHERE_WB_GAIN_MIN, 0.50), between(1.12, ATMOSPHERE_WB_GAIN_MAX, 2.0)),
            wb_ratio: between(1.20, ATMOSPHERE_WB_GAIN_RATIO, 3.0),
            wb_rotation_share,
            cast_ratio: between(1.5, CAST_ACCEPT_RATIO, 3.0),
            slope: (between(0.7, ATMOSPHERE_CURVE_SLOPE_MIN, 0.25), between(1.3, ATMOSPHERE_CURVE_SLOPE_MAX, 3.0)),
            confidence_cap: between(0.50, ATMOSPHERE_CONFIDENCE_CAP, 0.35),
            hsl_band: between(HSL_BAND_LIMIT_MIN, HSL_BAND_LIMIT_DEFAULT, HSL_BAND_LIMIT_MAX),
            zone_gain: (
                between(0.92, crate::fit_zoned::ZONE_ATMOS_GAIN_MIN, 0.50),
                between(1.08, crate::fit_zoned::ZONE_ATMOS_GAIN_MAX, 2.00),
            ),
            field_gain: between(FIELD_GAIN_MIN, FIELD_GAIN_DEFAULT, FIELD_GAIN_MAX),
            vetoes: if s >= 0.85 { VetoPolicy::Disclose } else { VetoPolicy::Withhold },
        }
    }
}

/// Options shared by global and zoned reverse-fit entry points.
#[derive(Clone, Copy, Default)]
pub struct FitOptions<'a> {
    pub strength: crate::recipe::GradeStrength,
    pub provider: Option<CorrespondenceProvider<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalCast {
    pub rotation_deg: f32,
    pub chroma_ratio: f32,
}

pub(super) fn wb_gain_ratio(gains: [f32; 3]) -> f32 {
    gains.iter().copied().fold(0.0f32, f32::max)
        / gains
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min)
            .max(1e-6)
}

pub(super) fn wb_gains_fit_budget(gains: [f32; 3], budget: FitBudget) -> bool {
    gains
        .iter()
        .all(|gain| (budget.wb_gain.0..=budget.wb_gain.1).contains(gain))
        && wb_gain_ratio(gains) <= budget.wb_ratio
}

pub(super) fn wb_path_candidate(anchor: f32, wb_k: f32, wb_tint: f32, lambda: f32) -> (f32, f32) {
    wb_path_between(anchor, (anchor, 0.0), (wb_k, wb_tint), lambda)
}

/// The point `lambda` of the way from one white balance to another, walked
/// in GAIN space (2026-10-22): the log-gain segment between the two ends,
/// inverted onto the engine's (Kelvin, tint) manifold for its own Kelvin by
/// the estimator's inversion. The ends themselves are returned as given
/// (the inversion's 401-step Kelvin grid would otherwise move a free fit by
/// a step). Walking Kelvin in log space with the tint scaled linearly, as
/// this did until then, carried the far end's tint — a compensation for the
/// far end's Kelvin — to every point between: the night sky's demand hit the
/// 2000 K bound with tint −100, shrank to 4850 K / −26.8 and the horizon
/// went green.
pub(super) fn wb_path_between(anchor: f32, from: (f32, f32), to: (f32, f32), lambda: f32) -> (f32, f32) {
    if lambda <= 1e-6 {
        return ((from.0 / 50.0).round() * 50.0, round1(from.1));
    }
    if lambda >= 1.0 - 1e-6 {
        return ((to.0 / 50.0).round() * 50.0, round1(to.1));
    }
    let log = |(k, tint): (f32, f32)| render::wb_gains(anchor, k, tint).map(|v| v.max(1e-5).ln());
    let (a, b) = (log(from), log(to));
    let want: [f32; 3] = std::array::from_fn(|c| a[c] + (b[c] - a[c]) * lambda);
    super::evidence::invert_wb_log_gains(anchor, want)
}

/// Keep a fitted white balance on the renderer's Kelvin/tint manifold while
/// spending no more than the strength budget. The only degree of freedom is
/// the scalar distance from as-shot `(anchor, 0)` to the free fit, walked in
/// gain space (`wb_path_between`).
pub(super) fn budgeted_wb(
    anchor: f32,
    wb_k: f32,
    wb_tint: f32,
    budget: FitBudget,
) -> (f32, f32, bool, f32, f32, f32) {
    let free_gains = render::wb_gains(anchor, wb_k, wb_tint);
    let ratio_before = wb_gain_ratio(free_gains);
    if wb_gains_fit_budget(free_gains, budget) {
        return (wb_k, wb_tint, false, ratio_before, ratio_before, 1.0);
    }

    // Every point of the path is already a persisted (rounded) pair; the
    // `rounded` flag is kept so the two bisections below read as they did.
    let candidate = |lambda: f32, _rounded: bool| wb_path_candidate(anchor, wb_k, wb_tint, lambda);

    // Zero is as-shot and therefore legal. Maintain a legal lower endpoint
    // and find the largest scalar move admitted by both WB constraints.
    let (mut legal, mut illegal) = (0.0f32, 1.0f32);
    for _ in 0..32 {
        let middle = (legal + illegal) * 0.5;
        let (k, tint) = candidate(middle, false);
        if wb_gains_fit_budget(render::wb_gains(anchor, k, tint), budget) {
            legal = middle;
        } else {
            illegal = middle;
        }
    }

    // Persist with the free path's exact rounding. If quantisation nudges the
    // endpoint over a bound, shrink along the same scalar path until the
    // persisted (rather than merely continuous) WB is legal too.
    let continuous_legal = legal;
    let (mut k, mut tint) = candidate(continuous_legal, true);
    if !wb_gains_fit_budget(render::wb_gains(anchor, k, tint), budget) {
        let (mut rounded_legal, mut rounded_illegal) = (0.0f32, continuous_legal);
        for _ in 0..32 {
            let middle = (rounded_legal + rounded_illegal) * 0.5;
            let (middle_k, middle_tint) = candidate(middle, true);
            if wb_gains_fit_budget(
                render::wb_gains(anchor, middle_k, middle_tint),
                budget,
            ) {
                rounded_legal = middle;
            } else {
                rounded_illegal = middle;
            }
        }
        legal = rounded_legal;
        (k, tint) = candidate(legal, true);
    }

    let ratio_after = wb_gain_ratio(render::wb_gains(anchor, k, tint));
    (k, tint, true, ratio_before, ratio_after, legal)
}

pub(super) fn mean_chroma_vector(px: &[[f32; 3]], weights: &[f32]) -> [f32; 2] {
    let mut out = [0.0; 2];
    let mut total = 0.0;
    for (i, p) in px.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        out[0] += (render::srgb_to_linear(p[0]) - render::srgb_to_linear(p[2])) * w;
        out[1] += (render::srgb_to_linear(p[1]) - (render::srgb_to_linear(p[0]) + render::srgb_to_linear(p[2])) * 0.5) * w;
        total += w;
    }
    if total > 1e-8 { [out[0] / total, out[1] / total] } else { [0.0; 2] }
}

fn hue_degrees(p: &[f32; 3]) -> Option<f32> {
    (evidence_hue_band(p).is_some()).then(|| render::rgb_to_hsl(p[0], p[1], p[2]).0 * 360.0)
}

fn signed_hue_delta(a: f32, b: f32) -> f32 {
    (b - a + 540.0).rem_euclid(360.0) - 180.0
}

pub(super) fn detect_global_cast(sp: &[[f32; 3]], tp: &[[f32; 3]], hue: &[EvidenceRange]) -> Option<GlobalCast> {
    let populated = hue.iter().filter(|r| r.source_populated || r.target_populated).collect::<Vec<_>>();
    if populated.is_empty() || populated.iter().any(|r| r.weight > 0.0 || r.source_populated == r.target_populated) {
        return None;
    }
    let mut deltas = Vec::new();
    for (s, t) in sp.iter().zip(tp) {
        if let (Some(a), Some(b)) = (hue_degrees(s), hue_degrees(t)) { deltas.push(signed_hue_delta(a, b)); }
    }
    if deltas.is_empty() { return None; }
    let sin = deltas.iter().map(|d| d.to_radians().sin()).sum::<f32>();
    let cos = deltas.iter().map(|d| d.to_radians().cos()).sum::<f32>();
    let mean = sin.atan2(cos).to_degrees();
    let coherent = deltas.iter().filter(|d| signed_hue_delta(mean, **d).abs() <= 45.0).count() as f32 / deltas.len() as f32;
    (coherent >= 0.80).then(|| {
        let cs = sp.iter().filter_map(|p| hue_degrees(p).map(|_| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum::<f32>() / sp.len().max(1) as f32;
        let ct = tp.iter().filter_map(|p| hue_degrees(p).map(|_| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum::<f32>() / tp.len().max(1) as f32;
        GlobalCast { rotation_deg: mean, chroma_ratio: ct / cs.max(1e-6) }
    })
}
