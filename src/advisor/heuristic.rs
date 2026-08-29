//! Heuristic proposer — a no-AI baseline used when `OPENAI_API_KEY` is unset.
//!
//! It is NOT a placeholder stub: it derives a sane recipe from the histogram
//! (exposure toward a midtone target, highlight/shadow recovery on clipping)
//! so the full propose → verify chain runs and produces something reviewable
//! today, before the GPT vision advisor is wired with a key. It ignores the
//! image content (no vision) and the revision hint.

use crate::decode::{Histogram, Meta};
use crate::recipe::EditRecipe;

use super::{Advisor, AdvisorError, Preview, ProposeContext};

#[derive(Default)]
pub struct HeuristicProposer {
    /// Why the heuristic is standing in for the vision proposer, when the cause
    /// is NOT simply "no key configured". Quoted verbatim in the recipe's
    /// rationale because that string is the only fallback explanation the user
    /// ever sees: the windowed GUI has no console for the caller's stderr
    /// warning, so a hard-coded "OPENAI_API_KEY unset" here would send a user
    /// whose key is fine (quota, network, HTTP error) hunting for the wrong bug.
    pub fallback_reason: Option<String>,
}

impl Advisor for HeuristicProposer {
    fn name(&self) -> &'static str {
        "heuristic"
    }

    fn propose(
        &self,
        _img: &Preview,
        _meta: &Meta,
        hist: &Histogram,
        ctx: &ProposeContext,
    ) -> Result<EditRecipe, AdvisorError> {
        self.propose_noted(hist, ctx.strength).map(|(r, _)| r)
    }
}

impl HeuristicProposer {
    /// The midtone the EV-offset estimator aims the frame's mean luma at, on
    /// the histogram's 0..255 scale (≈ 46 %, a touch above where 18 % grey
    /// encodes in sRGB — a camera-rendered preview's mean sits high).
    pub(crate) const MIDTONE_TARGET: f32 = 118.0;

    /// Mean luma of a histogram, 0..255, floored at 1 so the log in
    /// [`Self::ev_offset_estimate`] stays finite on an all-black frame.
    pub(crate) fn mean_luma(hist: &Histogram) -> f32 {
        let total: u64 = hist.luma.iter().map(|&v| v as u64).sum::<u64>().max(1);
        let weighted: u64 = hist
            .luma
            .iter()
            .enumerate()
            .map(|(i, &v)| i as u64 * v as u64)
            .sum();
        (weighted as f32 / total as f32).max(1.0)
    }

    /// **The EV-offset estimator** (M1_PLAN §8 #12), RAW: how many stops the
    /// frame sits from the midtone target — before the ±1.5 clamp, the
    /// 0.15-stop deadband and the 0.1-stop rounding [`Self::propose_noted`]
    /// applies on top of it.
    ///
    /// Split out as an ASSOCIATED function (the type is already re-exported,
    /// so this needs no new module visibility) purely so the estimator can be
    /// MEASURED against real photographs instead of only being observed
    /// through the recipe it feeds — `eval`'s `AUTOSHOP_ESTIMATOR_PROBE`.
    /// `propose_noted` is its only production caller and gets exactly the
    /// number it used to compute inline.
    pub(crate) fn ev_offset_estimate(hist: &Histogram) -> f32 {
        (Self::MIDTONE_TARGET / Self::mean_luma(hist)).log2()
    }

    /// The concrete entry `produce_recipe` calls (it holds this type, not a
    /// `dyn Advisor`): the recipe PLUS its rationale as a typed note, so the
    /// GUI can render the baseline explanation in the session language. The
    /// recipe's `rationale` string is `render_one(&note)` byte-for-byte —
    /// the L12#2B suffix contract with an empty prose prefix.
    pub fn propose_noted(
        &self,
        hist: &Histogram,
        strength: crate::recipe::GradeStrength,
    ) -> Result<(EditRecipe, crate::rationale::Note), AdvisorError> {
        let mean = Self::mean_luma(hist); // 0..255

        let mut r = EditRecipe::default();

        // Nudge exposure toward a midtone target of ~118/255, capped to ±1.5 EV.
        // Deadband: leave exposure untouched for sub-0.15-stop corrections — a
        // near-neutral frame doesn't need a trivial (and visually pointless)
        // nudge whose sign looks arbitrary.
        let ev_raw = Self::ev_offset_estimate(hist).clamp(-1.5, 1.5);
        r.exposure_ev = if ev_raw.abs() < 0.15 {
            0.0
        } else {
            (ev_raw * 10.0).round() / 10.0
        };

        // Recover blown highlights / lifted-but-clipped blacks proportionally.
        if hist.clip_white_pct > 0.5 {
            r.highlights = -(hist.clip_white_pct * 8.0).min(70.0);
            r.whites = -(hist.clip_white_pct * 4.0).min(40.0);
        }
        if hist.clip_black_pct > 1.0 {
            r.shadows = (hist.clip_black_pct * 6.0).min(60.0);
        }

        // Mild, conservative default "presence".
        r.contrast = 8.0;
        r.vibrance = 8.0;
        r.clarity = 4.0;

        r.confidence = 0.4;
        r.clamp();
        // GATE 6 of the strength axis (R23-3): the same guardrail as the AI path,
        // now on the same dial as the AI path. The baseline's own presence values
        // (contrast 8 / vibrance 8 / clarity 4) sit far below every soft-cap knee,
        // so the dial reaches this recipe only through the histogram-driven
        // recovery above — a heavily clipped frame drives Highlights to −70 and
        // Shadows to +60, where the knee decides the final number. Threading it
        // anyway is the point: a fallback that tastes different from the AI path
        // at the same dial setting is its own reported bug.
        r.temper(strength);

        // Rationale formatted AFTER clamp+temper: both can move the very
        // numbers it quotes (temper soft-caps recovery strength), and a
        // rationale that contradicts the recipe's own values reads as a bug —
        // a real verifier run flagged exactly that mismatch.
        use crate::rationale::{keys, render_one, Note};
        let mut args = vec![
            ("mean", format!("{mean:.0}")),
            ("clip_b", format!("{:.1}", hist.clip_black_pct)),
            ("clip_w", format!("{:.1}", hist.clip_white_pct)),
            ("ev", format!("{:+.1}", r.exposure_ev)),
            ("hl", format!("{:.0}", r.highlights)),
            ("sh", format!("{:.0}", r.shadows)),
        ];
        let note = match &self.fallback_reason {
            Some(e) => {
                args.push(("e", crate::rationale::error_line(&anyhow::anyhow!("{}", e))));
                Note::new(keys::HEURISTIC_UNAVAILABLE, args)
            }
            None => Note::new(keys::HEURISTIC_NO_KEY, args),
        };
        r.rationale = render_one(&note);
        Ok((r, note))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat histogram whose every pixel sits at `mean`.
    fn hist_at(mean: u8) -> Histogram {
        let mut luma = vec![0u32; 256];
        luma[mean as usize] = 1000;
        Histogram {
            luma,
            r: vec![0; 256],
            g: vec![0; 256],
            b: vec![0; 256],
            clip_black_pct: 0.0,
            clip_white_pct: 0.0,
            sample_pixels: 1000,
        }
    }

    /// The EV-offset estimator (M1_PLAN §8 #12) is a signed number of STOPS
    /// from the midtone target, and the proposer's `exposure_ev` is that
    /// number clamped/deadbanded/rounded — nothing else. Pinned because the
    /// estimator is now measured against the photographer's own
    /// `crs:Exposure2012` by `eval`'s probe, and a report is worthless if the
    /// quantity it correlates is not the quantity the product uses.
    ///
    /// MUTATION THIS CATCHES: invert the ratio (`mean / MIDTONE_TARGET`) and
    /// every sign below flips — a dark frame would ask for LESS exposure, and
    /// the probe's slope against the user's slider would come back negative
    /// for the right-looking reason. Drop the `Self::` extraction back inline
    /// and the last assertion (estimator ⇒ recipe) stops holding the two in
    /// step.
    #[test]
    fn ev_offset_estimator_is_signed_stops_and_feeds_the_recipe() {
        let est = HeuristicProposer::ev_offset_estimate;
        // On target ⇒ no correction at all.
        assert!(est(&hist_at(118)).abs() < 0.01, "{}", est(&hist_at(118)));
        // Half the target luma ⇒ exactly one stop UP (positive = brighten).
        assert!((est(&hist_at(59)) - 1.0).abs() < 0.01, "{}", est(&hist_at(59)));
        // Double it ⇒ exactly one stop DOWN.
        assert!((est(&hist_at(236)) + 1.0).abs() < 0.01, "{}", est(&hist_at(236)));
        // …and the RAW estimate runs past the proposer's ±1.5 clamp, which is
        // the whole reason the probe reads it here instead of off the recipe.
        assert!(est(&hist_at(2)) > 5.0, "a near-black frame is many stops down");

        // The recipe carries the estimate, rounded to 0.1 stop.
        let recipe = HeuristicProposer::default()
            .propose_noted(&hist_at(59), crate::recipe::GradeStrength::calibrated())
            .unwrap()
            .0;
        assert_eq!(recipe.exposure_ev, 1.0);
        // Deadband: a sub-0.15-stop estimate reaches the recipe as exactly 0.
        let near = hist_at(126);
        assert!(est(&near).abs() < 0.15 && est(&near).abs() > 0.0);
        let flat = HeuristicProposer::default()
            .propose_noted(&near, crate::recipe::GradeStrength::calibrated())
            .unwrap()
            .0;
        assert_eq!(flat.exposure_ev, 0.0);
    }

    #[test]
    fn rationale_quotes_the_tempered_numbers() {
        // 12% white clip drives highlights to the -70 recovery cap; temper's
        // soft-cap then reshapes that to exactly -60 (knee 50, ceil 70,
        // excess 20 → 50 + 20·20/40). The rationale must quote the FINAL
        // value — a real verifier flagged a rationale/recipe mismatch on
        // exactly this stale-rationale path.
        let mut luma = vec![0u32; 256];
        luma[128] = 1000;
        let hist = Histogram {
            luma,
            r: vec![0; 256],
            g: vec![0; 256],
            b: vec![0; 256],
            clip_black_pct: 0.0,
            clip_white_pct: 12.0,
            sample_pixels: 1000,
        };
        let meta = Meta {
            make: String::new(),
            model: String::new(),
            lens: None,
            iso: None,
            shutter: None,
            aperture: None,
            focal_length_mm: None,
            exposure_bias_ev: None,
            date_time: None,
            width: 0,
            height: 0,
            as_shot_wb_coeffs: [1.0; 4],
        };
        let at = |s: crate::recipe::GradeStrength| {
            HeuristicProposer::default()
                .propose(
                    &Preview { jpeg: Vec::new() },
                    &meta,
                    &hist,
                    &ProposeContext { strength: s, ..Default::default() },
                )
                .unwrap()
        };
        let calib = at(crate::recipe::GradeStrength::calibrated());
        assert_eq!(calib.highlights, -60.0, "temper soft-cap expectation drifted");
        assert!(
            calib.rationale.contains("highlights -60"),
            "rationale must quote the tempered value: {}",
            calib.rationale
        );

        // GATE 6 of the strength axis (R23-3): the no-AI fallback rides the SAME
        // dial as the AI path, so the same histogram must produce a bolder
        // recovery at a higher strength — and the rationale must still quote the
        // number the recipe actually carries, which is the property above.
        let bold = at(crate::recipe::GradeStrength::new(0.9));
        assert!(
            bold.highlights < calib.highlights,
            "gate 6 is not wired: strength 0.9 recovered no harder than 0.5 ({} vs {})",
            bold.highlights, calib.highlights
        );
        assert!(
            bold.rationale.contains(&format!("highlights {:.0}", bold.highlights)),
            "rationale must quote the tempered value at every strength: {}",
            bold.rationale
        );
        // …and the DEFAULT (0.65) sits strictly between them: the shipped
        // behaviour is braver than the calibration point by construction.
        let def = at(crate::recipe::GradeStrength::default());
        assert!(
            bold.highlights < def.highlights && def.highlights < calib.highlights,
            "the default must sit between calibrated and bold: {} < {} < {}",
            bold.highlights, def.highlights, calib.highlights
        );
    }
}
