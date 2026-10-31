//! The tone model: slider knots and their limiter, the tone and parametric LUTs, the base curve and the point curves.

use super::*;

/// Tone-model knot inputs. 0.66 is an explicit vertex so mid-bright water (≈0.66)
/// stays separated from the midtone (0.50) under a strong −Highlights; 0.82 shapes
/// the highlight shoulder; 0.92 is where whites concentrate. Shared with the
/// reverse-fit (fit.rs), which solves slider values against this same model.
pub(crate) const TONE_KNOTS_X: [f32; 8] = [0.0, 0.10, 0.25, 0.50, 0.66, 0.82, 0.92, 1.0];

/// Per-slider knot-output basis at input `x`: how far a fully-pushed +100 slider
/// moves the knot, in order `[contrast, highlights, shadows, whites, blacks]`.
/// The knot output is `tone_exposure_curve(x, ev) + basis · sliders/100`; keeping
/// this the ONLY definition means render and reverse-fit cannot drift apart.
pub(crate) fn tone_slider_basis(x: f32) -> [f32; 5] {
    // Authority: how far a fully-pushed ±100 slider moves its knot(s).
    const A_SHADOW: f32 = 0.33;
    const A_HIGHLIGHT: f32 = 0.34;
    const A_CONTRAST: f32 = 0.20;
    const A_WB: f32 = 0.32; // whites & blacks share it

    // Region basis functions over knot input x (each ∈ [0,1]).
    let w_shadow = smoothstep(0.0, 0.25, x) * (1.0 - smoothstep(0.25, 0.50, x));
    // highlights: peak 0.82, ZERO at 0.50 and PINNED to 0 at 1.0 — so highlights can
    // never move the white point; specular foam near white is never dragged down.
    let w_high = smoothstep(0.60, 0.82, x) * (1.0 - smoothstep(0.82, 1.0, x));
    // contrast: shoulder lobe minus toe lobe → antisymmetric, 0 at the ends and 0.50.
    let w_contrast = smoothstep(0.50, 0.75, x) * (1.0 - smoothstep(0.75, 1.0, x)) - w_shadow;
    // whites/blacks own the literal end knots (+ a touch of the adjacent knot).
    let w_white = if x >= 0.999 {
        1.0
    } else if (x - 0.92).abs() < 1e-3 {
        0.45
    } else {
        0.0
    };
    let w_black = if x <= 0.001 {
        1.0
    } else if (x - 0.10).abs() < 1e-3 {
        0.45
    } else {
        0.0
    };
    [
        A_CONTRAST * w_contrast,
        A_HIGHLIGHT * w_high,
        A_SHADOW * w_shadow,
        A_WB * w_white,
        A_WB * w_black,
    ]
}

/// The exposure component of a knot output: a linear-light gain of `ev` stops
/// applied under the sRGB transfer curve (the identity curve when ev = 0).
pub(crate) fn tone_exposure_curve(x: f32, ev: f32) -> f32 {
    linear_to_srgb((srgb_to_linear(x) * 2.0_f32.powf(ev)).clamp(0.0, 1.0))
}

/// Per-knot slider AUTHORITY under the current exposure — the tone-model
/// repair for the residual `limit_tone_sliders` documents below.
///
/// The knot model adds `basis(x)·sliders` at knot inputs in the ORIGINAL
/// tonal axis, so the basis does not know when exposure has already collapsed
/// the base curve around a knot: at +1.5 EV every knot from x = 0.66 up sits
/// at base 1.0, `contrast: -100` then writes 1.0 − 0.141 = 0.859 at 0.66 and
/// a DEEPER dip at 0.82, and the monotone backstop flattens the whole
/// [0.66, 0.82] interval at an interior grey — the measured 197-input plateau
/// at code 56304. The λ limiter cannot help: those intervals have
/// `base_gap ≤ 1e-6` and are rightly its skip case (exposure clipped them;
/// that is what exposure means).
///
/// So authority follows the base curve's own local separation: a knot keeps
/// full weight while at least one adjacent base interval is still open
/// (`ramp(0, 0.01, healthiest adjacent gap)`), and fades to zero where
/// exposure has saturated BOTH sides. A muted knot stays on the base curve,
/// the saturated run stays at the ceiling/floor, and a strong slider there
/// now yields honest clipping instead of an interior flat band — Lightroom's
/// own semantics for a slider aimed at a region exposure already clipped.
///
/// Exactness matters twice: at ev = 0 every gap is ≥ 0.08, `ramp` saturates
/// to exactly 1.0, and every render is bit-for-bit unchanged; the boundary
/// knot of a saturated run keeps weight through its healthy side, so the
/// slider's effect on the still-alive interval below is preserved, not
/// chopped at the run's edge. Endpoint knots have one neighbour — the
/// missing side counts as closed, so a fully-clipped end loses authority
/// with the run it belongs to.
///
/// Shared by all three model sites (`build_tone_lut`, `limit_tone_sliders`,
/// `fit.rs::fit_tone_sliders`); weights depend only on `ev`, so the model
/// stays LINEAR in the sliders and the reverse-fit still inverts it
/// analytically.
pub(crate) fn tone_knot_weights(ev: f32) -> [f32; 8] {
    // Full authority once the healthiest adjacent interval separates by 1 %
    // of the axis — comfortably under the 0.08 minimum an ev = 0 grid has,
    // comfortably over the 1e-6 the λ limiter treats as closed.
    const GAP_FULL: f32 = 0.01;
    let base: [f32; 8] = std::array::from_fn(|i| tone_exposure_curve(TONE_KNOTS_X[i], ev));
    std::array::from_fn(|i| {
        let left = if i > 0 { base[i] - base[i - 1] } else { 0.0 };
        let right = if i < 7 { base[i + 1] - base[i] } else { 0.0 };
        ramp(0.0, GAP_FULL, left.max(right))
    })
}

/// Scale a slider vector `[contrast, highlights, shadows, whites, blacks]`
/// (each already in −1..1) down to the strongest version of ITSELF that no
/// longer collapses a tonal band. A slider must SATURATE, never annihilate.
///
/// The knots sit only 0.08–0.25 apart and nothing used to check that a
/// slider's offset fitted in the gap. Past a threshold a knot overtook its
/// neighbour and the repair in [`build_tone_lut`] — snap to `prev + 1e-4` —
/// turned that whole interval FLAT; Fritsch–Carlson then zeroed both tangents,
/// making it exactly flat, so every input tone in the interval rendered to one
/// output value. Clamping to 1.0 did the same at the top end. These are
/// ORDINARY edits, not abuse — measured on the pre-fix engine through the real
/// export path on a 16-bit ramp:
///
///   * `whites: -50` mapped input 0.9568–0.9731 to a single code and cut the
///     top decade from 411 distinct codes to 75.
///   * `highlights: +60` mapped everything above 0.8195 to pure white — 18 %
///     of the range, 740 of 4096 sampled inputs.
///
/// Detail destroyed here is not recoverable by any later stage. Four rounds of
/// tests missed it because a flat band is still monotone and still pins its
/// endpoints, which is all they asserted.
///
/// Returning scaled SLIDERS rather than a repaired curve is deliberate: the
/// knot model stays linear in the sliders, which is what lets `fit.rs` invert
/// it analytically. ONE site applies this — `tone_model_knots`, which is where
/// the sliders become knots, so every consumer of the knot model inherits it:
/// `build_tone_lut` for the develop chain and `hdr::SdrRendition` for the SDR
/// rendition's own four controls. That inheritance is not free, and it is worth
/// naming: it is what capped the HDR shoulder at 55 % of Lightroom's while the
/// shoulder was expressed as a negative Highlights, because a rendering
/// transform was being governed by a taste control's guard (fixed 2026-09-19 —
/// see `render::hdr::shoulder`). The reverse
/// fit deliberately does not (see the note at the end of `fit_tone_sliders`):
/// it scores candidates by rendering them, so it already measures whatever
/// the engine does, and pre-applying the limiter perturbed a solve that was
/// tuned against a knife-edge acceptance test.
///
/// λ = 1 whenever nothing binds, so every edit inside the thresholds renders
/// bit-for-bit as before; only the region that was being destroyed changes.
///
/// Both of this design's measured gaps are CLOSED, by different halves of
/// the model (grid = 18 recipes × 15 exposures on the real engine, counting
/// only INTERIOR plateaus — a run at 0 or 65535 is clipping, which is what a
/// strong slider on a bright frame is meant to do):
///
///   * The high-exposure residual (worst cell `contrast: -100` at `+1.5 EV`,
///     a 197-input plateau at code 56304; grid 13 cells > 96) was the knot
///     BASIS not knowing exposure had saturated the base curve around a knot
///     — fixed in the model by [`tone_knot_weights`], not here. Four
///     knot-level repairs measured before it (including a pre-weights
///     per-slider λ: 13 cells → 9 but worst 197 → 317) all traded the tail
///     for a worse one; the model fix took the grid to 6 cells > 96.
///   * The collateral gap — ONE λ scaled the whole vector, so pinning
///     shadows +50 while dragging whites −45 → −100 rendered the shadows at
///     22.5 — is closed by the per-slider iteration below: only the sliders
///     that CLOSE the worst-violated interval shrink, the single-λ pass
///     stays as the unconditional backstop, and the grid worst is now 100 —
///     the same level the ev = 0 design holds
///     (`a_slider_that_binds_an_interval_no_longer_drags_the_innocent_ones`).
pub(crate) fn limit_tone_sliders(ev: f32, s: [f32; 5]) -> [f32; 5] {
    // The share of an interval's EXISTING separation the sliders must leave
    // behind. Two calibration notes, both learned the hard way:
    //
    //   * Phrased against the base curve's own gap, NOT against the identity
    //     slope. An absolute floor has a cliff wherever exposure has already
    //     narrowed an interval to just above it; there λ collapses to ~0 and
    //     silently zeroes every slider at once.
    //   * Small on purpose. This exists to stop a band COLLAPSING, not to
    //     reserve a fixed share of every interval. At 0.05 it bound on an
    //     ordinary reverse-fit result (contrast +40.4 with shadows −42.8 puts
    //     λ at 0.990 for the 0.10–0.25 interval), which perturbed the solve
    //     that fit.rs had already tuned to that fixture. At 0.01 the same
    //     recipe is unconstrained (λ = 1.03) while `highlights: +60` is still
    //     cut to ~47 and `whites: -50` to ~45 — the settings that were
    //     destroying detail. 1 % of a 0.147-wide interval is still 96 distinct
    //     16-bit codes, which is a gradient, not a flat patch.
    const KEEP: f32 = 0.01;
    let weights = tone_knot_weights(ev);
    // Per-slider differential contribution to each interval: how much slider
    // k (at full value s[k]) changes the separation of interval i. Weighted —
    // an unweighted λ would limit against offsets the engine no longer adds.
    let mut base = [0.0f32; 8];
    let mut wb = [[0.0f32; 5]; 8];
    for (i, &x) in TONE_KNOTS_X.iter().enumerate() {
        let b = tone_slider_basis(x);
        base[i] = tone_exposure_curve(x, ev);
        for k in 0..5 {
            wb[i][k] = weights[i] * b[k] * s[k];
        }
    }

    // PER-SLIDER λ, iteratively: for the worst-violated interval, shrink only
    // the sliders whose contribution CLOSES it, by exactly the factor that
    // interval needs; repeat. Sliders that open the interval — or act
    // elsewhere — keep their authority (the old single λ scaled the whole
    // vector, so pinning shadows +50 while dragging whites −45 → −100 pulled
    // the rendered shadows down to 22.5 with it). A shrink here can deepen a
    // violation in ANOTHER interval that the shrunk slider was helping to
    // hold open (contrast is antisymmetric), so this iterates to a fixpoint —
    // and the single-λ pass below remains as the unconditional backstop, so
    // the hard guarantee never rests on convergence.
    let mut lam = [1.0f32; 5];
    for _ in 0..8 {
        // Worst violation under the CURRENT per-slider scales.
        let mut worst: Option<(usize, f32)> = None; // (interval, allowed/actual)
        for i in 1..8 {
            let gap = base[i] - base[i - 1];
            if gap <= 1e-6 {
                continue; // exposure's own clipping — see tone_knot_weights
            }
            let d: f32 = (0..5).map(|k| (wb[i][k] - wb[i - 1][k]) * lam[k]).sum();
            let allowed = -(1.0 - KEEP) * gap;
            if d < allowed {
                let ratio = allowed / d; // in (0,1): fraction of d that fits
                if worst.is_none_or(|(_, r)| ratio < r) {
                    worst = Some((i, ratio));
                }
            }
        }
        let Some((i, _)) = worst else { break };
        let gap = base[i] - base[i - 1];
        let allowed = -(1.0 - KEEP) * gap;
        let (mut open, mut close) = (0.0f32, 0.0f32);
        for k in 0..5 {
            let c = (wb[i][k] - wb[i - 1][k]) * lam[k];
            if c < 0.0 { close += c } else { open += c }
        }
        // f·close + open ≥ allowed  ⇒  f = (allowed − open) / close, in [0,1):
        // `close < allowed − open ≤ 0` here, since the interval is violated
        // and `allowed − open ≤ allowed < 0`.
        let f = ((allowed - open) / close).clamp(0.0, 1.0);
        for k in 0..5 {
            if (wb[i][k] - wb[i - 1][k]) * lam[k] < 0.0 {
                lam[k] *= f;
            }
        }
    }

    // Unconditional single-λ backstop over whatever the iteration left: the
    // band-collapse guarantee is enforced HERE, not by convergence above.
    // λ = 1 whenever nothing binds, and then every knot — and so every
    // rendered pixel — is bit-for-bit what the per-slider scales produced.
    let mut lambda = 1.0f32;
    for i in 1..8 {
        let gap = base[i] - base[i - 1];
        if gap <= 1e-6 {
            continue;
        }
        let d: f32 = (0..5).map(|k| (wb[i][k] - wb[i - 1][k]) * lam[k]).sum();
        if d < 0.0 {
            lambda = lambda.min((1.0 - KEEP) * gap / -d);
        }
    }
    let lambda = lambda.clamp(0.0, 1.0);
    std::array::from_fn(|k| s[k] * lam[k] * lambda)
}

/// Build the develop tone curve as a [`LUT_N`]-entry LUT over input gamma [0,1].
///
/// It is an 8-knot control-point curve fit by a MONOTONE cubic Hermite spline
/// (Fritsch–Carlson), so it is monotone *by construction* (no post-hoc clamp) and
/// the endpoints are pinned. Exposure is a linear-light gain applied before the
/// curve; contrast is an antisymmetric S; shadows/highlights shape the toe/shoulder
/// WITHOUT reaching the midtones or the white point (so a strong −Highlights can't
/// drag specular foam to grey — that is the white point's job, owned by whites);
/// whites/blacks move the end knots. The parametric curve (v1.5.0) and then the
/// recipe's own `tone_curve` are composed on top, Lightroom's Tone Curve panel
/// order. This replaces a summed-region-hump model that could go non-monotonic
/// and crush mid-bright water / near-white foam (which had needed ad-hoc
/// patches).
pub(crate) fn build_tone_lut(r: &EditRecipe) -> Vec<f32> {
    tone_lut_at(r, r.exposure_ev)
}

/// The tone of a Camera Raw recipe after its exposure, as the develop applies
/// it, in Lightroom's order: Contrast, Whites, Blacks' and Dehaze's LUT
/// (Highlights and Shadows run before Exposure, `highlights_shadows`), which the develop puts
/// through Camera Raw's curve rule ([`CameraRawTone::sliders_px`], the middle by
/// its encoded ratio; with Clarity or Dehaze `tone_cr::camera_raw_slider_pixel`); the
/// parametric and point curves, by the same rule; the Look's baked curve, by
/// it with the middle at its linear ratio. `None` for a pass with nothing to do.
/// With exposure (`tone_cr::Exposure`) the passes compose to
/// [`build_tone_lut`] up to the colour rule.
pub(crate) struct CameraRawTone {
    pub(crate) sliders: Option<SliderPass>,
    /// A NEGATIVE Blacks alone, as its own LUT: the develop puts it through
    /// each ProPhoto channel on its own, after `sliders` (which then carries
    /// no Blacks). Measured against Camera Raw on a synthetic colour grid
    /// (408 patches, Blacks −80): each channel through the grey curve leaves
    /// a mean channel-multiplier error of 0.001, the luminance gain 0.207, the
    /// brightest channel's gain 0.277, the curve rule (encoded middle) dE
    /// 1.34 against 0.77; on p39 Blacks −40 / −80 (coloured |Δab|) 1.18 /
    /// 2.41 against the curve rule's 1.19 / 2.54. A positive Blacks stays in
    /// `sliders`: no rule measured for it.
    pub(crate) blacks_down: Option<Vec<f32>>,
    /// Highlights and Shadows in −1..1, a creative profile's baked moves
    /// included: the pass before Exposure (`tone_cr::apply_camera_raw_hs`).
    pub(crate) highlights_shadows: (f32, f32),
    pub(crate) look: Option<Vec<f32>>,
    pub(crate) curves: Option<Vec<f32>>,
}

pub(crate) fn camera_raw_tone_passes(r: &EditRecipe) -> CameraRawTone {
    let parametric = parametric_lut(r);
    let curves = (parametric.is_some() || !curve_is_identity(&r.tone_curve))
        .then(|| camera_raw_after_lut(None, parametric.as_deref(), &curve_lut(&r.tone_curve)));
    let bl = tone_sliders(r)[4];
    CameraRawTone {
        sliders: camera_raw_slider_lut(
            {
                // Highlights and Shadows run before Exposure, as a pass of
                // their own (`tone_cr::apply_camera_raw_hs`); a negative
                // Blacks runs as its own pass (`blacks_down`).
                let [c, _, _, wh, _] = tone_sliders(r);
                [c, 0.0, 0.0, wh, bl.max(0.0)]
            },
            Local { clarity: (r.clarity / 100.0).clamp(-1.0, 1.0) },
        ),
        blacks_down: if bl < 0.0 {
            camera_raw_slider_lut([0.0, 0.0, 0.0, 0.0, bl], Local::default()).map(|p| p.lut)
        } else {
            None
        },
        look: r.baked_tone_curve().map(|l| look_curve_lut(&l.tone_curve, l.amount)),
        highlights_shadows: {
            let s = tone_sliders(r);
            (s[1], s[2])
        },
        curves,
    }
}

impl CameraRawTone {
    /// One pixel (sRGB-encoded, working space) through the passes after the
    /// exposures and before a mask's table sliders, as the develop puts them
    /// on: Contrast, Whites and a positive Blacks by Camera Raw's curve rule
    /// ([`CurveSpace::tone`], the middle at its encoded ratio), or by
    /// `tone_cr::camera_raw_slider_pixel` when the pass carries Clarity's
    /// tone (`clarity`); then a negative Blacks through each ProPhoto channel
    /// on its own ([`CurveSpace::each_channel`]). The develop's loop and the
    /// zone fit's law (`mask_cr::LocalGreyLaw`) both call this.
    pub(super) fn sliders_px(&self, px: [f32; 3], cs: &CurveSpace, clarity: bool) -> [f32; 3] {
        let mut px = px;
        if let Some(s) = &self.sliders {
            px = if clarity { camera_raw_slider_pixel(px, s) } else { cs.tone(px, &s.lut, MiddleRatio::Encoded) };
        }
        if let Some(lut) = &self.blacks_down {
            px = cs.each_channel(px, lut);
        }
        px
    }

    /// One pixel through the passes after a mask's table sliders: the
    /// photographer's parametric and point curves, then the Look's baked
    /// curve, both by Camera Raw's rule ([`CurveSpace::tone`]; the middle at
    /// its encoded ratio, the Look's at its linear one). Shared as
    /// [`CameraRawTone::sliders_px`] is.
    pub(super) fn curves_px(&self, px: [f32; 3], cs: &CurveSpace) -> [f32; 3] {
        let mut px = px;
        if let Some(c) = &self.curves {
            px = cs.tone(px, c, MiddleRatio::Encoded);
        }
        if let Some(look) = &self.look {
            px = cs.tone(px, look, MiddleRatio::Linear);
        }
        px
    }
}

/// A point curve that leaves every value where it was: empty, or every knot
/// on the diagonal.
fn curve_is_identity(points: &[crate::recipe::CurvePoint]) -> bool {
    points.iter().all(|p| p.input == p.output)
}

/// The five tone sliders of `r` in −1..1, a creative profile's baked moves
/// included.
fn tone_sliders(r: &EditRecipe) -> [f32; 5] {
    let contrast = (r.contrast / 100.0).clamp(-1.0, 1.0);
    // v1.5.0 F7 — a creative profile's BAKED tone moves ride with the
    // photographer's own. "Adobe Landscape" bakes Highlights −12 and Shadows
    // +12; adding them here is exact, because the knot model below is linear in
    // the slider vector (see the comment under it).
    let highlights = (r.with_baked(r.highlights, |l| l.highlights) / 100.0).clamp(-1.0, 1.0);
    let shadows = (r.with_baked(r.shadows, |l| l.shadows) / 100.0).clamp(-1.0, 1.0);
    let whites = (r.whites / 100.0).clamp(-1.0, 1.0);
    let blacks = (r.blacks / 100.0).clamp(-1.0, 1.0);
    [contrast, highlights, shadows, whites, blacks]
}

fn tone_lut_at(r: &EditRecipe, exposure_ev: f32) -> Vec<f32> {
    // Knot OUTPUTS: exposure-mapped identity, then the slider offsets — all from
    // the shared basis below so the reverse-fit (fit.rs) solves against the SAME
    // model the engine renders.
    let [contrast, highlights, shadows, whites, blacks] = tone_sliders(r);

    // Saturate the slider vector BEFORE using it, so the knot model below
    // stays exactly what it always was: base + basis·sliders, linear in the
    // sliders. That linearity is load-bearing — `fit_tone_sliders` inverts it
    // analytically — so the limit is applied to the SLIDERS, once, here and in
    // the fit, rather than to the curve afterwards.
    // v1.17.0: on a camera-profile start the sliders follow Camera Raw's
    // measured law, in the input of Adobe's curve (`tone_cr`). All at zero,
    // the start renders exactly as it did: the arm below.
    if SliderLaw::of(r) == SliderLaw::CameraRaw {
        let sliders = [contrast, highlights, shadows, whites, blacks];
        if exposure_ev != 0.0 || sliders.iter().any(|&v| v != 0.0) {
            let baked = r.baked_tone_curve().map(|l| look_curve_lut(&l.tone_curve, l.amount));
            let parametric = parametric_lut(r);
            let curve = curve_lut(&r.tone_curve);
            let start = r.camera_start.unwrap_or_default();
            return camera_raw_tone_lut(start, exposure_ev, sliders, baked.as_deref(), parametric.as_deref(), &curve);
        }
    }
    let (ys, m) = tone_model_knots(
        exposure_ev,
        [contrast, highlights, shadows, whites, blacks],
    );
    let curve = curve_lut_for(r, &r.tone_curve); // the recipe's own tone_curve, composed on top
    let parametric = parametric_lut(r); // …over the parametric curve, over the sliders
    let user: Vec<f32> = (0..LUT_N)
        .map(|i| {
            let x = i as f32 / (LUT_N - 1) as f32;
            let toned = hermite_eval(&TONE_KNOTS_X, &ys, &m, x);
            let shaped = parametric.as_ref().map_or(toned, |p| sample_lut(p, toned));
            sample_lut(&curve, shaped)
        })
        .collect();
    // The BASE RENDITION the photographer's sliders act on, and there is only
    // ever one of it.
    //
    // A creative profile states its own base curve, MEASURED, and the engine
    // estimates one per photograph from the camera's embedded preview when
    // there is no profile to ask. Composing both would put two renditions of
    // the same intent in series and darken the picture twice, so the profile's
    // own curve WINS whenever the sidecar carries one. Its `crs:Amount` blends
    // it toward the identity, as a partial Look means.
    // Exactly one starting tone: `starting_curve` is empty whenever the recipe
    // names a camera profile (v1.17.0), whose own tone is then the start.
    let base = match (r.baked_tone_curve(), r.starting_curve().is_empty()) {
        (Some(l), _) => look_curve_lut(&l.tone_curve, l.amount),
        (None, true) => return user,
        (None, false) => base_curve_lut(r.starting_curve()),
    };
    // Composed UNDER the user controls — sliders act on the camera-like base,
    // the same profile-then-sliders order Lightroom uses.
    // final(x) = user(base(x)); one LUT, still zero extra per-pixel cost.
    (0..LUT_N).map(|i| sample_lut(&user, base[i])).collect()
}

/// A creative Look's baked point curve as a [`LUT_N`] table, blended toward
/// the identity by the Look's `crs:Amount`, as a partial Look means.
///
/// `curve_lut` is the photographer's own point-curve sampler, and a Look's
/// `crs:ToneCurvePV2012` is literally that same spelling — so it is the right
/// sampler, at the wrong RESOLUTION: it returns 256 entries and a base
/// rendition is `LUT_N`. Read it through `sample_lut`, which interpolates
/// whatever length it is handed, rather than indexing it with this loop's own
/// counter.
pub(super) fn look_curve_lut(points: &[crate::recipe::CurvePoint], amount: f32) -> Vec<f32> {
    let c = curve_lut(points);
    let a = amount.clamp(0.0, 1.0);
    (0..LUT_N)
        .map(|i| {
            let x = i as f32 / (LUT_N - 1) as f32;
            x + (sample_lut(&c, x) - x) * a
        })
        .collect()
}

/// How far one parametric region slider at ±100 moves its region's control
/// value, as a fraction of the region's width. ½ is the largest reach that
/// keeps the control values ordered at every slider and split setting (see
/// [`parametric_lut`]), so the extremes may flatten a region — Shadows −100
/// crushes the toe to black — but can never fold the curve back.
/// PROVISIONAL: first-principles, until the Lightroom kit's `PARAM-*` exports
/// measure it.
const PARAMETRIC_REACH: f32 = 0.5;

/// Lightroom's PARAMETRIC tone curve (v1.5.0) as a [`LUT_N`]-entry table over
/// gamma input, or `None` while its four region sliders are all 0.
///
/// A quadratic B-spline on the knot vector `[0, 0, 0, s₁, s₂, s₃, 1, 1, 1]`:
/// the three splits ARE the interior knots, so each region is one polynomial
/// piece and a split moves where two pieces join. Its six control values sit
/// at the Greville abscissae `0, s₁/2, (s₁+s₂)/2, (s₂+s₃)/2, (s₃+1)/2, 1` —
/// the two ends and the four REGION CENTRES, which is why this basis and no
/// other. At rest each control value equals its abscissa and the spline IS the
/// identity (a B-spline reproduces a straight line through its Greville
/// points), so moving a split with every slider at 0 changes nothing, as in
/// Lightroom. A region slider moves its centre's control value by
/// [`PARAMETRIC_REACH`] × slider/100 × the region's width; the response is
/// smooth (C¹) and reaches into both neighbouring regions, the broad overlap
/// Lightroom's panel shades for a hovered region.
///
/// Monotone BY CONSTRUCTION, not by a post-hoc sort: a B-spline whose control
/// values never decrease cannot decrease (variation diminishing), and two
/// consecutive abscissae lie half the two regions' widths apart, so a reach of
/// at most ½ keeps every pair ordered for any sliders and any split layout.
/// The ends stay pinned at 0 and 1.
pub(crate) fn parametric_lut(r: &EditRecipe) -> Option<Vec<f32>> {
    let regions = r.parametric_regions()?;
    let [s1, s2, s3] = r.parametric_splits().map(|s| s / 100.0);
    let knots = [0.0, 0.0, 0.0, s1, s2, s3, 1.0, 1.0, 1.0];
    let widths = [s1, s2 - s1, s3 - s2, 1.0 - s3];
    let centres = [s1 / 2.0, (s1 + s2) / 2.0, (s2 + s3) / 2.0, (s3 + 1.0) / 2.0];
    let mut control = [0.0f32, 0.0, 0.0, 0.0, 0.0, 1.0];
    for i in 0..4 {
        let push = PARAMETRIC_REACH * (regions[i] / 100.0).clamp(-1.0, 1.0) * widths[i];
        control[i + 1] = (centres[i] + push).clamp(0.0, 1.0);
    }
    Some(
        (0..LUT_N)
            .map(|i| {
                let x = i as f32 / (LUT_N - 1) as f32;
                // The knot span holding x: [0,s₁), [s₁,s₂), [s₂,s₃), [s₃,1].
                let span = 2 + knots[3..6].iter().filter(|k| x >= **k).count();
                de_boor_quadratic(&knots, &control, span, x).clamp(0.0, 1.0)
            })
            .collect(),
    )
}

/// One point of a quadratic B-spline by de Boor's recursion: `span` is the
/// knot interval `knots[span] <= x < knots[span + 1]` holding `x`. A
/// zero-length interval (two coincident knots, which ordered splits never
/// produce) contributes its left value rather than a division by zero.
fn de_boor_quadratic(knots: &[f32; 9], control: &[f32; 6], span: usize, x: f32) -> f32 {
    let mut d = [control[span - 2], control[span - 1], control[span]];
    for r in 1..=2 {
        for j in (r..=2).rev() {
            let lo = knots[j + span - 2];
            let den = knots[j + 1 + span - r] - lo;
            let alpha = if den > 0.0 { (x - lo) / den } else { 0.0 };
            d[j] = (1.0 - alpha) * d[j - 1] + alpha * d[j];
        }
    }
    d[2]
}

/// The knot outputs and spline tangents of the engine's slider-tone model
/// for `(ev, sliders)` — the ONE definition `build_tone_lut` renders and the
/// reverse fit scores candidates against ([`sample_tone_model`]). Limiter,
/// authority weights, monotone snap and Fritsch–Carlson exactly as rendered;
/// no residual tone_curve and no base curve composed.
pub(crate) fn tone_model_knots(ev: f32, sliders: [f32; 5]) -> ([f32; 8], Vec<f32>) {
    let [contrast, highlights, shadows, whites, blacks] = limit_tone_sliders(ev, sliders);
    let mut ys = [0.0f32; 8];
    let weights = tone_knot_weights(ev);
    for (idx, &x) in TONE_KNOTS_X.iter().enumerate() {
        let b = tone_slider_basis(x);
        // Knot authority fades where exposure saturated BOTH adjacent base
        // intervals (see tone_knot_weights): a strong slider aimed at a
        // region exposure already clipped yields honest clipping, not the
        // interior flat band the backstop below used to manufacture.
        ys[idx] = tone_exposure_curve(x, ev)
            + weights[idx]
                * (b[0] * contrast
                    + b[1] * highlights
                    + b[2] * shadows
                    + b[3] * whites
                    + b[4] * blacks);
    }
    // Backstop only. λ above already keeps the SLIDERS from closing an
    // interval, so this now fires just where exposure itself saturated the
    // knots (the `base_gap <= need` skip), which is exposure's prerogative.
    // Kept unchanged and deliberately minimal: with the limiter in place a
    // minimum-slope version of this loop changes nothing a test can see
    // (verified by mutation), and monotonicity is all it owes.
    // Fritsch–Carlson on monotone data ⇒ the whole spline is monotone, so
    // there is NO running-max pass over the sampled LUT — it is structural.
    const EPS: f32 = 1e-4;
    for i in 1..ys.len() {
        if ys[i] < ys[i - 1] + EPS {
            ys[i] = ys[i - 1] + EPS;
        }
    }
    for v in &mut ys {
        *v = v.clamp(0.0, 1.0);
    }
    let m = fc_tangents(&TONE_KNOTS_X, &ys);
    (ys, m)
}

/// The engine's slider-tone response at one input — [`tone_model_knots`]
/// evaluated through the same Hermite the LUT samples.
pub(crate) fn sample_tone_model(knots: &([f32; 8], Vec<f32>), x: f32) -> f32 {
    hermite_eval(&TONE_KNOTS_X, &knots.0, &knots.1, x)
}

/// LUT for the recipe's camera-matched base curve (`EditRecipe::base_curve`).
/// Knot hygiene mirrors `build_tone_lut`: sort by x, drop non-increasing x,
/// force non-decreasing y (a tone curve cannot invert), pin the (0,0)/(1,1)
/// endpoints — a hand-edited recipe must not unpin black/white through the
/// base stage — then the same monotone Fritsch–Carlson Hermite as the tone
/// model, so the base look inherits its no-inversion/no-overshoot guarantees.
fn base_curve_lut(knots: &[[f32; 2]]) -> Vec<f32> {
    const EPS: f32 = 1e-4;
    let mut pts: Vec<(f32, f32)> = knots
        .iter()
        .map(|p| (p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut xs = vec![0.0f32];
    let mut ys = vec![0.0f32];
    for (x, y) in pts {
        if x <= *xs.last().expect("seeded") + EPS || x >= 1.0 - EPS {
            continue; // endpoint pins own x=0 / x=1
        }
        xs.push(x);
        ys.push(y);
    }
    xs.push(1.0);
    ys.push(1.0);
    for i in 1..ys.len() {
        if ys[i] < ys[i - 1] + EPS {
            ys[i] = ys[i - 1] + EPS;
        }
    }
    for v in &mut ys {
        *v = v.clamp(0.0, 1.0);
    }
    let m = fc_tangents(&xs, &ys);
    (0..LUT_N)
        .map(|i| hermite_eval(&xs, &ys, &m, i as f32 / (LUT_N - 1) as f32))
        .collect()
}

/// Monotone cubic Hermite tangents (Fritsch–Carlson). With `xs` strictly increasing
/// and `ys` non-decreasing, the resulting Hermite spline is monotone everywhere.
fn fc_tangents(xs: &[f32], ys: &[f32]) -> Vec<f32> {
    let n = xs.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        if d[i - 1] * d[i] <= 0.0 {
            m[i] = 0.0; // local extremum → flat tangent (keeps monotonicity)
        } else {
            let w1 = 2.0 * (xs[i + 1] - xs[i]) + (xs[i] - xs[i - 1]);
            let w2 = (xs[i + 1] - xs[i]) + 2.0 * (xs[i] - xs[i - 1]);
            m[i] = (w1 + w2) / (w1 / d[i - 1] + w2 / d[i]); // weighted harmonic mean
        }
    }
    // Monotonicity limiter: keep each (α,β) inside the circle α²+β² ≤ 9.
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let a = m[i] / d[i];
            let b = m[i + 1] / d[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    m
}

/// Evaluate the monotone cubic Hermite spline at `x` (clamped to the knot range).
pub(super) fn hermite_eval(xs: &[f32], ys: &[f32], m: &[f32], x: f32) -> f32 {
    let n = xs.len();
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    let mut i = 0;
    while i + 1 < n && x > xs[i + 1] {
        i += 1;
    }
    let h = xs[i + 1] - xs[i];
    let t = (x - xs[i]) / h;
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * ys[i] + h10 * h * m[i] + h01 * ys[i + 1] + h11 * h * m[i + 1]
}

/// Curve control points → a 256-entry [0,1]→[0,1] LUT under the law `r`
/// renders through ([`SliderLaw::of`]): the ONE curve sampler shared by the
/// master tone curve, the per-channel RGB curves, a mask's curves and the GUI
/// curve editor's on-screen preview — public so what the editor draws is
/// exactly what the engine applies. On the engine's own law the knots are
/// joined by straight lines, as every release before v1.17.0 drew them; on
/// Camera Raw's law (a recipe naming a camera profile) by its spline
/// ([`curve_lut`]).
pub fn curve_lut_for(r: &EditRecipe, points: &[crate::recipe::CurvePoint]) -> Vec<f32> {
    curve_lut_under(points, SliderLaw::of(r))
}

/// [`curve_lut_for`] with the law already resolved.
pub fn curve_lut_under(points: &[crate::recipe::CurvePoint], law: SliderLaw) -> Vec<f32> {
    match law {
        SliderLaw::CameraRaw => curve_lut(points),
        SliderLaw::Engine => match curve_knots(points) {
            None => identity_curve_lut(),
            Some(pts) => (0..256).map(|i| interp(&pts, i as f32 / 255.0)).collect(),
        },
    }
}

/// Piecewise-linear interpolation over sorted (x,y) control points, clamped at
/// the ends: the engine law's point curve.
fn interp(pts: &[(f32, f32)], x: f32) -> f32 {
    if pts.is_empty() {
        return x;
    }
    if x <= pts[0].0 {
        return pts[0].1;
    }
    if x >= pts[pts.len() - 1].0 {
        return pts[pts.len() - 1].1;
    }
    for w in pts.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if x >= x0 && x <= x1 {
            let t = if (x1 - x0).abs() < 1e-6 { 0.0 } else { (x - x0) / (x1 - x0) };
            return y0 + (y1 - y0) * t;
        }
    }
    x
}

fn identity_curve_lut() -> Vec<f32> {
    (0..256).map(|i| i as f32 / 255.0).collect()
}

/// Camera Raw's point-curve sampler — the law of a recipe that names a camera
/// profile ([`curve_lut_for`]): a 256-entry [0,1]→[0,1] LUT, identity when
/// empty, the knots joined by Camera Raw's own spline ([`adobe_spline`]).
/// Measured on one frame's flat packet (`ToneCurvePV2012Red` `0,0 / 128,180 /
/// 255,255`, Camera Raw 16-bit against a prediction from the flat render, ΔE
/// mean over 0.58 M pixels): straight lines leave 4.73, the spline 0.08; on a
/// five-knot curve 3.01 against 0.53.
pub fn curve_lut(points: &[crate::recipe::CurvePoint]) -> Vec<f32> {
    match curve_knots(points) {
        None => identity_curve_lut(),
        Some(pts) => {
            let spline = adobe_spline(&pts);
            (0..256).map(|i| spline(i as f32 / 255.0).clamp(0.0, 1.0)).collect()
        }
    }
}

/// A point curve's knots on [0, 1], sorted, twins dropped and the ends
/// pinned; `None` for an empty curve.
fn curve_knots(points: &[crate::recipe::CurvePoint]) -> Option<Vec<(f32, f32)>> {
    if points.is_empty() {
        return None;
    }
    let mut pts: Vec<(f32, f32)> = points
        .iter()
        .map(|p| (p.input as f32 / 255.0, p.output as f32 / 255.0))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Duplicate inputs (possible via a hand-edited / imported recipe; the GUI
    // editor keeps inputs strictly increasing) resolve FIRST-point-wins —
    // now actually, by DROPPING the later twins. Merely relying on the stable
    // sort gave the first twin's output AT that code and the second twin's
    // output one code later: a one-LUT-bin cliff that shows up as a hard
    // contour line across a smooth gradient.
    // Pin the endpoints Lightroom-style: a curve that places no point of its
    // own AT an end keeps (0,0)/(1,1) authoritative there. Without the pins,
    // interp's flat clamp beyond the first/last point turns a single mid-curve
    // click into a constant image and flattens everything past any inner
    // endpoint into crushed/blown bands. A user (or AI) point at exactly
    // x=0 / x=1 still overrides the pin — lifted blacks stay expressible.
    // Drop the later twins so first-point-wins is literally true.
    pts.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-6);
    if pts[0].0 > 0.0 {
        pts.insert(0, (0.0, 0.0));
    }
    if pts[pts.len() - 1].0 < 1.0 {
        pts.push((1.0, 1.0));
    }
    Some(pts)
}

/// The DNG SDK's curve spline (`dng_spline_solver`), which Camera Raw puts
/// through a point curve's knots: a cubic Hermite through every knot whose
/// slopes start as the chord-length-weighted mean of the two neighbouring
/// chords (the end ones extrapolated) and are then solved as a tridiagonal
/// system for a C² curve. Not monotone — a steep knot overshoots between its
/// neighbours, as it does in Lightroom; the caller clamps to [0, 1]. Two knots
/// are the straight line through them. `pts` sorted, inputs distinct.
fn adobe_spline(pts: &[(f32, f32)]) -> impl Fn(f32) -> f32 + '_ {
    let n = pts.len();
    let x: Vec<f64> = pts.iter().map(|p| f64::from(p.0)).collect();
    let y: Vec<f64> = pts.iter().map(|p| f64::from(p.1)).collect();
    let mut s = vec![0.0f64; n];
    if n >= 2 {
        let (mut a, mut b) = (x[1] - x[0], (y[1] - y[0]) / (x[1] - x[0]));
        s[0] = b;
        for j in 2..n {
            let c = x[j] - x[j - 1];
            let d = (y[j] - y[j - 1]) / c;
            s[j - 1] = (b * c + d * a) / (a + c);
            (a, b) = (c, d);
        }
        s[n - 1] = 2.0 * b - s[n - 2];
        s[0] = 2.0 * s[0] - s[1];
        if n > 2 {
            let (mut e, mut f, mut g) = (vec![0.0f64; n], vec![0.0f64; n], vec![0.0f64; n]);
            f[0] = 0.5;
            e[n - 1] = 0.5;
            g[0] = 0.75 * (s[0] + s[1]);
            g[n - 1] = 0.75 * (s[n - 2] + s[n - 1]);
            for j in 1..n - 1 {
                let a = (x[j + 1] - x[j - 1]) * 2.0;
                e[j] = (x[j + 1] - x[j]) / a;
                f[j] = (x[j] - x[j - 1]) / a;
                g[j] = 1.5 * s[j];
            }
            for j in 1..n {
                let a = 1.0 - f[j - 1] * e[j];
                if j != n - 1 {
                    f[j] /= a;
                }
                g[j] = (g[j] - g[j - 1] * e[j]) / a;
            }
            for j in (0..n - 1).rev() {
                g[j] -= f[j] * g[j + 1];
            }
            s = g;
        }
    }
    move |v: f32| {
        let v = f64::from(v);
        if n == 0 {
            return v as f32;
        }
        if v <= x[0] {
            return y[0] as f32;
        }
        if v >= x[n - 1] {
            return y[n - 1] as f32;
        }
        let j = x.partition_point(|&k| k < v).clamp(1, n - 1);
        let a = x[j] - x[j - 1];
        let (b, c) = ((v - x[j - 1]) / a, (x[j] - v) / a);
        (((y[j - 1] * (2.0 - c + b) + s[j - 1] * a * b) * c * c) + ((y[j] * (2.0 - b + c) - s[j] * a * c) * b * b))
            as f32
    }
}

/// Sample a LUT (any length) at a normalised [0,1] position with linear interp.
pub(crate) fn sample_lut(lut: &[f32], x: f32) -> f32 {
    let n = lut.len();
    if n == 0 {
        return x;
    }
    let pos = x.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = pos.floor() as usize;
    if i >= n - 1 {
        return lut[n - 1];
    }
    let t = pos - i as f32;
    lut[i] * (1.0 - t) + lut[i + 1] * t
}

/// Apply the per-channel RGB curves (red/green/blue) in place — the colour
/// companion to the master tone curve. No-op when all three are empty.
///
/// v1.17.0, Camera Raw's law, measured: each curve acts on one channel of
/// LINEAR ProPhoto RGB (D50, Bradford from the working white —
/// [`profile::curve_space`]) under the sRGB transfer, never on the working
/// space's own channels. On one frame's flat packet, curves `0,0 / 128,180 /
/// 255,255` and `0,0 / 255,200` on each of R, G and B: in that encoding the
/// other two channels move by 0.0000 and the curved one is a pure function
/// of itself (scatter 0.0018–0.0039, the render's noise); predicted there
/// the renders are matched to ΔE 0.01–0.12, where the working sRGB channels
/// (the engine's place before v1.17.0, with straight lines between knots)
/// left 2.3–12.0, and ProPhoto linear, under gamma 1.8 or under gamma 2.2,
/// with the spline, 0.27–8.0.
/// The caller places this pass after Saturation and Vibrance (`develop`):
/// a red curve composed onto Camera Raw's own HSL, Contrast +50, Exposure +1,
/// Saturation +50 and Vibrance +50 renders predicts them to 0.08–0.12.
///
/// All of that is Camera Raw's law, for a recipe that names a camera profile
/// ([`SliderLaw::of`]). On the engine's own law each curve, straight lines
/// between its knots ([`curve_lut_for`]), acts on the working space's own
/// encoded channel, and the develop places the pass right after the tone
/// stage — what every release before v1.17.0 rendered.
pub(super) fn apply_rgb_curves(data: &mut [[f32; 3]], r: &EditRecipe, space: ExportColorSpace) {
    let law = SliderLaw::of(r);
    let curves = [&r.red_curve, &r.green_curve, &r.blue_curve];
    // v1.5.0 F7: a creative profile may bake per-channel curves too, and they
    // compose UNDER the photographer's for the same reason the master curve
    // does. Identity on all 161 Looks in the measured library — Adobe writes
    // the three channels out as `0,0 … 255,255` — so this is the general case
    // being handled rather than an observed one.
    let baked = r.baked_look().map(|l| [&l.red_curve, &l.green_curve, &l.blue_curve]);
    let has_baked = baked.is_some_and(|b| b.iter().any(|c| !c.is_empty()));
    if curves.iter().all(|c| c.is_empty()) && !has_baked {
        return;
    }
    let luts: [Vec<f32>; 3] = std::array::from_fn(|ch| {
        let own = curve_lut_under(curves[ch], law);
        match baked.map(|b| b[ch]).filter(|c| !c.is_empty()) {
            None => own,
            Some(under) => {
                // Both LUTs are `curve_lut`'s 256 entries and this table is
                // `LUT_N`, so BOTH are read through `sample_lut` rather than
                // indexed by this loop's counter — the same defect the master
                // curve's arm had, swept here rather than left for the first
                // Look that bakes a real per-channel curve to find. Every one
                // of the 161 measured writes the three channels as identity,
                // which `parse_curve_checked` returns empty, so this arm has
                // never run on a real file.
                let base = curve_lut_under(under, law);
                (0..LUT_N)
                    .map(|i| {
                        let x = i as f32 / (LUT_N - 1) as f32;
                        sample_lut(&own, sample_lut(&base, x))
                    })
                    .collect()
            }
        }
    });
    let active: [bool; 3] =
        std::array::from_fn(|ch| !curves[ch].is_empty() || baked.is_some_and(|b| !b[ch].is_empty()));
    if law == SliderLaw::Engine {
        data.par_iter_mut().for_each(|px| *px = engine_channel_curves(*px, &luts, active));
        return;
    }
    let space = CurveSpace::new(space);
    data.par_iter_mut().for_each(|px| *px = space.channel_curves(*px, &luts, active));
}

/// The engine law's per-channel curves: each `active` channel of the working
/// space's encoded pixel through its own LUT.
pub(super) fn engine_channel_curves(px: [f32; 3], luts: &[Vec<f32>; 3], active: [bool; 3]) -> [f32; 3] {
    let mut px = px;
    for ch in 0..3 {
        if active[ch] {
            px[ch] = sample_lut(&luts[ch], px[ch]);
        }
    }
    px
}

/// [`profile::curve_space`] and the sRGB transfer, resolved once per pass:
/// what a per-channel curve (global or a mask's) is applied through.
pub(super) struct CurveSpace {
    to: [[f32; 3]; 3],
    from: [[f32; 3]; 3],
    dec: &'static [f32],
    enc: &'static [f32],
}

impl CurveSpace {
    pub(super) fn new(space: ExportColorSpace) -> Self {
        let (to, from) = profile::curve_space(space);
        let (dec, enc) = transfer_luts();
        CurveSpace { to, from, dec, enc }
    }

    /// One working-space pixel (sRGB-encoded) through the `active` channel
    /// curves in ProPhoto under the sRGB transfer, back in the working space.
    pub(super) fn channel_curves(&self, px: [f32; 3], luts: &[Vec<f32>; 3], active: [bool; 3]) -> [f32; 3] {
        let mut pro = mat_vec3(&self.to, &px.map(|v| sample_lut(self.dec, v)));
        for ch in 0..3 {
            if active[ch] {
                let e = sample_lut(self.enc, pro[ch].clamp(0.0, 1.0));
                pro[ch] = sample_lut(self.dec, sample_lut(&luts[ch], e));
            }
        }
        mat_vec3(&self.from, &pro).map(|v| sample_lut(self.enc, v.clamp(0.0, 1.0)))
    }

    /// One working-space pixel with every ProPhoto channel through the same
    /// curve on its own: [`CurveSpace::channel_curves`] with all three
    /// channels on one table.
    pub(super) fn each_channel(&self, px: [f32; 3], lut: &[f32]) -> [f32; 3] {
        let mut pro = mat_vec3(&self.to, &px.map(|v| sample_lut(self.dec, v)));
        for v in &mut pro {
            let e = sample_lut(self.enc, v.clamp(0.0, 1.0));
            *v = sample_lut(self.dec, sample_lut(lut, e));
        }
        mat_vec3(&self.from, &pro).map(|v| sample_lut(self.enc, v.clamp(0.0, 1.0)))
    }

    /// One working-space pixel through a tone curve by Camera Raw's rule
    /// ([`camera_raw_rgb_tone`]) on these channels, back in the working space.
    pub(super) fn tone(&self, px: [f32; 3], lut: &[f32], middle: MiddleRatio) -> [f32; 3] {
        let pro = mat_vec3(&self.to, &px.map(|v| sample_lut(self.dec, v))).map(|v| v.clamp(0.0, 1.0));
        let c = camera_raw_rgb_tone(pro, lut, middle);
        mat_vec3(&self.from, &c).map(|v| sample_lut(self.enc, v.clamp(0.0, 1.0)))
    }
}

/// The channels a per-channel curve acts on under `law`: ProPhoto under the
/// sRGB transfer on Camera Raw's ([`curve_channels`]), the pixel's own
/// working channels on the engine's ([`apply_rgb_curves`]).
pub(crate) fn curve_channels_under(px: &[f32; 3], law: SliderLaw) -> [f32; 3] {
    match law {
        SliderLaw::CameraRaw => curve_channels(px),
        SliderLaw::Engine => *px,
    }
}

/// sRGB-encoded pixels (sRGB primaries, the analysis renders') → the
/// channels [`apply_rgb_curves`] puts a per-channel curve on under Camera Raw's law: ProPhoto RGB
/// ([`profile::curve_space`]) under the sRGB transfer. What the reverse fit
/// matches a channel curve's distributions in (`fit::curves`), so the curve
/// it fits is a curve of the channel Camera Raw's law applies it to.
pub(crate) fn curve_channels(px: &[f32; 3]) -> [f32; 3] {
    use std::sync::OnceLock;
    static TO: OnceLock<[[f32; 3]; 3]> = OnceLock::new();
    let to = TO.get_or_init(|| profile::curve_space(ExportColorSpace::Srgb).0);
    let (dec, enc) = transfer_luts();
    mat_vec3(to, &px.map(|v| sample_lut(dec, v))).map(|v| sample_lut(enc, v.clamp(0.0, 1.0)))
}

/// Where Camera Raw keeps a colour's MIDDLE channel when a tone curve moves
/// the other two ([`camera_raw_rgb_tone`]). Measured, and the two curves a
/// camera-profile start carries differ here:
///
/// * the creative Look's baked curve keeps it at its LINEAR-light ratio (a
///   bench whose camera space is linear ProPhoto with no tables, item 35:
///   149 colours through Adobe Color's Look curve predicted to ΔE median
///   0.010, max 0.109; by the encoded ratio max 2.21);
/// * the photographer's composite point curve (with the parametric curve),
///   and a mask's `MainCurve`, keep it at its ENCODED ratio (p39's packet with Adobe Color's Look, the
///   four composite cases predicted from Camera Raw's own renders without the
///   curve: `0,0 / 64,40 / 128,170 / 192,200 / 255,255` ΔE 0.308 and warm
///   a* +0.12 by the encoded ratio, 0.325 and −0.43 by the linear one; the
///   other three agree or lean the same way; a full-coverage gradient mask's
///   `MainCurve` `0,0 / 128,180 / 255,255` on the same packet, applied after
///   the Look: ΔE 0.069 by the encoded ratio, 0.075 by the linear one, 0.718
///   by the luma ratio the engine used until item 35).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MiddleRatio {
    Linear,
    Encoded,
}

/// Camera Raw's rule for putting a tone curve on a colour: the channels are
/// linear ProPhoto (D50); the BRIGHTEST and DARKEST go through the curve on
/// their sRGB-ENCODED values, and the middle one keeps its place between
/// them by `middle` — with [`MiddleRatio::Linear`] the DNG SDK's `RGBTone`
/// with the curve read under the sRGB transfer. On the item-35 bench the same
/// rule on sRGB channels instead leaves ΔE median 0.117, max 2.56, and each
/// channel through the curve on its own max 5.44. `lut` is a curve on the
/// encoded range, sampled by [`sample_lut`].
pub(super) fn camera_raw_rgb_tone(pro: [f32; 3], lut: &[f32], middle: MiddleRatio) -> [f32; 3] {
    let (dec, enc) = transfer_luts();
    let (mut hi, mut mid, mut lo) = (0usize, 1usize, 2usize);
    if pro[hi] < pro[mid] {
        std::mem::swap(&mut hi, &mut mid);
    }
    if pro[mid] < pro[lo] {
        std::mem::swap(&mut mid, &mut lo);
    }
    if pro[hi] < pro[mid] {
        std::mem::swap(&mut hi, &mut mid);
    }
    let e = pro.map(|v| sample_lut(enc, v.clamp(0.0, 1.0)));
    let (ea, eb) = (sample_lut(lut, e[hi]), sample_lut(lut, e[lo]));
    let (a, b) = (sample_lut(dec, ea), sample_lut(dec, eb));
    let m = match middle {
        MiddleRatio::Linear => {
            let span = pro[hi] - pro[lo];
            if span > 1e-9 { b + (a - b) * (pro[mid] - pro[lo]) / span } else { b }
        }
        MiddleRatio::Encoded => {
            let span = e[hi] - e[lo];
            sample_lut(dec, if span > 1e-9 { eb + (ea - eb) * (e[mid] - e[lo]) / span } else { eb })
        }
    };
    let mut out = [0.0f32; 3];
    out[hi] = a;
    out[mid] = m;
    out[lo] = b;
    out
}

/// A tone curve on a camera-profile start, by Camera Raw's rule
/// ([`camera_raw_rgb_tone`]) on the channels of [`profile::curve_space`]: the
/// photographer's composite point curve (with the parametric curve,
/// [`MiddleRatio::Encoded`]), and the creative Look's baked curve after it
/// ([`MiddleRatio::Linear`]). The develop puts them on per pixel through
/// [`CameraRawTone::curves_px`]; this whole-frame form is the tests' handle on
/// the same rule ([`CurveSpace::tone`]).
///
/// Order, measured on one frame against predictions from Camera Raw's own
/// renders without the curve (ΔE mean): the photographer's curve runs BEFORE
/// the Look's (after it, the order before v1.17.0, leaves 0.51–1.48, before
/// it 0.13–0.31, from the flat start and from Exposure +1, Contrast +50,
/// Saturation +50 and Vibrance +50 alike), and the Look's curve after the
/// profile's default curve (greys: after 0.011 L, before 0.985 L). With no
/// Look, `0,0 / 128,180 / 255,255` predicted to 0.06, a five-knot curve 0.21,
/// `0,0 / 255,200` 0.02.
#[cfg(test)]
pub(super) fn apply_camera_raw_curve(
    data: &mut [[f32; 3]],
    lut: &[f32],
    space: ExportColorSpace,
    middle: MiddleRatio,
) {
    let cs = CurveSpace::new(space);
    data.par_iter_mut().for_each(|px| *px = cs.tone(*px, lut, middle));
}
