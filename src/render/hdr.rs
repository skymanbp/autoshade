//! Lightroom's HDR edit mode and its SDR rendition, rendered (v1.5.0 F8).
//!
//! # What HDR mode is, and why an SDR engine owes it anything
//!
//! Lightroom's HDR mode does not change the capture. It changes where DIFFUSE
//! WHITE sits in it: the brightest stops stop being "clipped" and become
//! HEADROOM above white, and `crs:HDRMaxValue` says how many stops of it there
//! are. Every slider in the Basic panel then acts on that extended range.
//!
//! Every file this engine writes is SDR — 8- or 16-bit sRGB — so an HDR
//! photograph cannot be published as one. But that is not a reason to render
//! it as if the mode had never been set, because Lightroom has an answer to
//! exactly this problem and writes it into the same sidecar: the SDR
//! RENDITION, tuned by the seven-control panel Lightroom shows only in HDR
//! mode (`crs:SDRBlend`, `…Brightness`, `…Contrast`, `…Highlights`,
//! `…Shadows`, `…Whites`, `…Clarity`). Rendering the rendition is rendering
//! the photograph the photographer actually approved for SDR output.
//!
//! # The model
//!
//! One tone pass at the END of the develop chain, then Clarity:
//!
//! ```text
//!   W        = 2 ^ (HDRMaxValue · (1 + SDRBlend/100))     the headroom, linear
//!   shoulder = Reinhard against W, in linear light
//!   lut      = tone_model_knots(SDRBrightness·EV_PER_100,
//!                               [SDRContrast, SDRHighlights,
//!                                SDRShadows, SDRWhites, 0]) ∘ shoulder
//!   l'       = lut(l)                       (chroma-preserving, as stage 1)
//!   then       clarity(SDRClarity)          (the Basic panel's own operator)
//! ```
//!
//! The headroom gets its OWN curve, taking the stops `crs:HDRMaxValue` states
//! as a tone mapper's white point — the thing a white point already means — so
//! the shoulder has no free parameter to fit. The shoulder runs FIRST and the
//! seven SDR controls tune what it produced, which is both what the panel is
//! for (it exists to correct the SDR rendition) and what keeps
//! `tone_model_knots` inside the SDR-ranged domain it was calibrated in.
//!
//! It was a negative Highlights push until 2026-09-19, on the reasoning that a
//! shoulder IS a highlight give-back and that borrowing the already-calibrated
//! slider beat inventing a private curve. The kit falsified it: see
//! [`shoulder`].
//!
//! Blacks is absent because Lightroom's SDR panel has no Blacks; Brightness
//! stands where an Exposure slider would, because that panel has no Exposure
//! either.
//!
//! # What is measured and what is not
//!
//! **Measured (2026-09-19).** The shoulder, from the kit's `HDR-ON` against the
//! mode-off export of the same frame — one transfer, with every SDR control at
//! 0 and `crs:HDRMaxValue="+2.30"`, so it is the headroom's curve and nothing
//! else. Reinhard against that stated headroom sits at rms 0.0412 of it, and
//! letting the white point float instead of trusting the sidecar improves that
//! to 0.0410 — a 0.5 % gain for a fitted parameter, which is the measurement
//! saying the sidecar's own number is already the right one.
//!
//! **Not measured.** LINEARITY in the headroom: the kit's three HDR cases all
//! carry `+2.30`, so one white point is pinned and the curve's behaviour at
//! other headrooms rests on Reinhard's own form. `SDRBlend`, which is 0 in all
//! three. And `BRIGHTNESS_EV_PER_100`, which stays a first-principles value —
//! see the constant.
//!
//! The reference library (175 sidecars, census 2026-09-17) carries
//! `crs:HDREditMode="0"` on 114 photographs and `"1"` on NONE, which is why
//! this needed a kit at all. The user's standing order for this batch is
//! explicit that a missing ground truth is not a reason to skip the work
//! ("implement every control we can set and do not render; slight deviation
//! from Lightroom is allowed, compatibility is the aim", 2026-09-17) — the same
//! order `detail.rs` was built under.
//!
//! **Measured.** That `crs:HDREditMode` and `crs:HDRMaxValue` are real keys
//! with those spellings and those value forms (`"0"`, `"+1.00"`) — both appear
//! in real sidecars in the library. The seven `SDR*` spellings come from the
//! settings key table inside the installed Camera Raw 18.4 build, located by
//! sibling density around `Exposure2012` (see `docs/ARCHITECTURE.md`).

use rayon::prelude::*;

use super::{
    clarity_radius, hermite_eval, luma601, sample_lut, scale_chroma, tone_model_knots,
    unsharp_luma, LUT_N, TONE_KNOTS_X,
};
use crate::recipe::EditRecipe;

/// The headroom's own curve: Reinhard against a white point of `2^stops`.
///
/// Zero free parameters. `stops` is what `crs:HDRMaxValue` states, and a tone
/// mapper's white point is already "the value that maps to white", so the
/// sidecar's number enters as itself rather than through a fitted coefficient.
/// Linear light, because a headroom is a multiplier there and nowhere else.
///
/// MEASURED against the kit's `HDR-ON` (2026-09-19): rms 0.0412 of Lightroom's
/// own transfer, and at the top of the range (input 0.90) it lands within
/// −0.006 of Lightroom's −0.189.
///
/// That kit case is IN THE TREE, at `src/fixtures/hdr-on-lightroom-9.4.xmp` —
/// the `+2.30` headroom above was read from it, and it is the only sidecar
/// anywhere on hand with the mode on, so a tree without it could restate this
/// number but never re-derive it. `xmp::tests::lightrooms_own_hdr_sidecar_
/// reads_as_the_mode_and_headroom_it_states` holds the reader to those bytes.
///
/// This REPLACED a negative Highlights push, and the reason is worth keeping
/// because the old reasoning was good. Folding the shoulder into the slider
/// meant it inherited [`render::limit_tone_sliders`](super::limit_tone_sliders),
/// a deliberate limiter whose rule is that a slider must saturate and never
/// annihilate a tonal band. That is right for a slider a photographer drags and
/// wrong for a rendering transform, and it capped the shoulder at about 55 % of
/// Lightroom's: the old model could not get past −0.104 at input 0.90 no matter
/// what headroom the sidecar stated. A headroom is not a taste control and must
/// not be governed by a taste control's guard.
///
/// Monotone for every input this can be handed, including `SDRBlend` at its
/// ends: `stops <= 0` gives `W = 1`, for which the expression collapses to
/// `L·(1+L)/(1+L) = L` — the identity, exactly, with no special case.
fn shoulder(x: f32, stops: f32) -> f32 {
    if stops <= 0.0 {
        return x;
    }
    let w = 2.0f32.powf(stops);
    let l = super::srgb_to_linear(x);
    super::linear_to_srgb(l * (1.0 + l / (w * w)) / (1.0 + l))
}

/// Stops of exposure at SDRBrightness ±100.
///
/// One, because the SDR panel has no Exposure slider and Brightness is
/// therefore that rendition's exposure control, and because ±1 EV is the
/// range over which a rendition is TUNED rather than re-exposed.
///
/// REPLACE IT from `HDR-EXP+1-SDR` against `HDR-EXP+1`: the two differ only in
/// the SDR block, so their exported pixels give the whole four-control move
/// (Brightness +30, Contrast +20, Highlights −40, Shadows +20) in one
/// difference. That is a JOINT constraint, not this constant alone — fitting
/// it means solving the four together against the knot model, and no kit case
/// moves Brightness by itself.
const BRIGHTNESS_EV_PER_100: f32 = 1.0;

/// The SDR rendition as one tone LUT plus a Clarity amount — what
/// [`apply`](self::apply) needs and nothing else.
pub(crate) struct SdrRendition {
    /// `None` when every tonal control and the shoulder are neutral, so a
    /// rendition that is only a Clarity move does not pay for an identity LUT.
    lut: Option<Vec<f32>>,
    /// Clarity, already in the operator's -1..=1 units.
    clarity: f32,
}

impl SdrRendition {
    /// The SDR rendition this recipe asks for, or `None` when it asks for
    /// none.
    ///
    /// **`hdr_edit` is a hard gate**, exactly as in Lightroom, where the panel
    /// exists only in HDR mode. A sidecar can easily carry `crs:SDRBrightness`
    /// from an HDR session the photographer later left; honouring it on an SDR
    /// photograph would re-tone a picture whose owner can no longer see the
    /// control that did it.
    pub(crate) fn global(r: &EditRecipe) -> Option<Self> {
        if !r.hdr_edit {
            return None;
        }
        // Blend scales the HEADROOM rather than the shoulder's output, so the
        // shoulder stays a Reinhard curve — and therefore monotone — at every
        // blend value, where mixing its output against the identity would not be.
        let stops = (r.hdr_max_ev * (1.0 + r.sdr_blend / 100.0)).max(0.0);
        let ev = r.sdr_brightness / 100.0 * BRIGHTNESS_EV_PER_100;
        let sliders = [
            (r.sdr_contrast / 100.0).clamp(-1.0, 1.0),
            (r.sdr_highlights / 100.0).clamp(-1.0, 1.0),
            (r.sdr_shadows / 100.0).clamp(-1.0, 1.0),
            (r.sdr_whites / 100.0).clamp(-1.0, 1.0),
            // Blacks: the SDR panel has none, so neither does the rendition.
            0.0,
        ];
        let tonal = stops > 0.0 || ev != 0.0 || sliders.iter().any(|v| *v != 0.0);
        let clarity = (r.sdr_clarity / 100.0).clamp(-1.0, 1.0);
        if !tonal && clarity == 0.0 {
            return None;
        }
        let lut = tonal.then(|| {
            let (ys, m) = tone_model_knots(ev, sliders);
            (0..LUT_N)
                .map(|i| {
                    let x = i as f32 / (LUT_N - 1) as f32;
                    // Shoulder first: the SDR controls tune what it produced,
                    // which keeps the knot model in its calibrated domain.
                    hermite_eval(&TONE_KNOTS_X, &ys, &m, shoulder(x, stops))
                })
                .collect()
        });
        Some(SdrRendition { lut, clarity })
    }
}

/// Render the SDR rendition in place, over the finished develop.
///
/// Tone first and Clarity second — the Basic panel's own order, and the order
/// the global chain uses, so a Clarity of +30 lifts the local contrast of the
/// tones the rendition actually publishes rather than the ones it was about to
/// compress away.
pub(crate) fn apply(data: &mut [[f32; 3]], w: usize, h: usize, p: &SdrRendition) {
    if let Some(lut) = &p.lut {
        data.par_iter_mut().for_each(|px| {
            let l_old = luma601(px);
            let l_new = sample_lut(lut, l_old);
            scale_chroma(px, l_old, l_new);
        });
    }
    if p.clarity != 0.0 {
        unsharp_luma(data, w, h, clarity_radius(w, h), p.clarity, true);
    }
}
