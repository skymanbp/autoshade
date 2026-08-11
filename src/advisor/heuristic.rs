//! Heuristic proposer — a no-AI baseline used when `OPENAI_API_KEY` is unset.
//!
//! It is NOT a placeholder stub: it derives a sane recipe from the histogram
//! (exposure toward a midtone target, highlight/shadow recovery on clipping)
//! so the full propose → verify chain runs and produces something reviewable
//! today, before the GPT vision advisor is wired with a key. It ignores the
//! image content (no vision) and the revision hint.

use crate::decode::{Histogram, Meta};
use crate::recipe::EditRecipe;

use super::{Advisor, AdvisorError, Preview};

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
        _reference: Option<&str>,
        _guidance: Option<&str>,
        _hint: Option<&str>,
    ) -> Result<EditRecipe, AdvisorError> {
        self.propose_noted(hist).map(|(r, _)| r)
    }
}

impl HeuristicProposer {
    /// The concrete entry `produce_recipe` calls (it holds this type, not a
    /// `dyn Advisor`): the recipe PLUS its rationale as a typed note, so the
    /// GUI can render the baseline explanation in the session language. The
    /// recipe's `rationale` string is `render_one(&note)` byte-for-byte —
    /// the L12#2B suffix contract with an empty prose prefix.
    pub fn propose_noted(
        &self,
        hist: &Histogram,
    ) -> Result<(EditRecipe, crate::rationale::Note), AdvisorError> {
        let total: u64 = hist.luma.iter().map(|&v| v as u64).sum::<u64>().max(1);
        let weighted: u64 = hist
            .luma
            .iter()
            .enumerate()
            .map(|(i, &v)| i as u64 * v as u64)
            .sum();
        let mean = (weighted as f32 / total as f32).max(1.0); // 0..255

        let mut r = EditRecipe::default();

        // Nudge exposure toward a midtone target of ~118/255, capped to ±1.5 EV.
        // Deadband: leave exposure untouched for sub-0.15-stop corrections — a
        // near-neutral frame doesn't need a trivial (and visually pointless)
        // nudge whose sign looks arbitrary.
        let ev_raw = (118.0_f32 / mean).log2().clamp(-1.5, 1.5);
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
        r.temper(); // same taste guardrail as the AI path

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
                let e = super::BoundedUntrustedText::new(e, 512, &[]);
                args.push(("e", e.into_string()));
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
        let r = HeuristicProposer::default()
            .propose(&Preview { jpeg: Vec::new() }, &meta, &hist, None, None, None)
            .unwrap();
        assert_eq!(r.highlights, -60.0, "temper soft-cap expectation drifted");
        assert!(
            r.rationale.contains("highlights -60"),
            "rationale must quote the tempered value: {}",
            r.rationale
        );
    }
}
