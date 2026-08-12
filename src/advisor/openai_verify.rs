//! OpenAI-compatible **API** verifier — the analysis role when its provider is
//! `api` instead of the OAuth `claude` CLI. Uses the standard Chat Completions
//! endpoint (`/chat/completions`), so it also works with any OpenAI-compatible
//! server (point `analysis_base_url` at it). Text-only: like the Claude verifier,
//! it judges the recipe from data, never the image.

use serde_json::{json, Value};

use crate::config::Config;
use crate::decode::{Histogram, Meta};
use crate::recipe::EditRecipe;

use super::{build_verify_prompt, parse_verdict, Advisor, AdvisorError, Verdict};

pub struct OpenAiVerifier {
    api_key: Option<String>,
    model: String,
    base_url: String,
    /// The analysis role's reasoning-effort tier — see `OpenAiProvider`.
    effort: Option<String>,
}

impl OpenAiVerifier {
    pub fn new(cfg: &Config) -> Self {
        Self {
            api_key: cfg.analysis_api_key.clone(),
            model: cfg.analysis_model.clone(),
            base_url: cfg.analysis_base_url.clone(),
            effort: cfg.analysis_effort.clone(),
        }
    }
}

impl Advisor for OpenAiVerifier {
    fn name(&self) -> &'static str {
        "openai-verify"
    }

    fn verify(
        &self,
        recipe: &EditRecipe,
        meta: &Meta,
        hist: &Histogram,
    ) -> Result<Verdict, AdvisorError> {
        let key = self.api_key.as_ref().ok_or_else(|| {
            AdvisorError::Missing(
                "analysis API key (set it in Settings, or AUTOSHOP_ANALYSIS_API_KEY)".into(),
            )
        })?;
        let prompt = build_verify_prompt(recipe, meta, hist)?;
        let mut body = json!({
            "model": self.model,
            "messages": [{ "role": "user", "content": prompt }],
            "temperature": 0
        });
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        // Text-only chat at temperature 0 — the fastest AI call here, but a
        // reasoning-class verifier still outgrows any fixed overall budget.
        // Streaming-first: token deltas prove liveness, the budget bounds
        // silence, and the reassembled value keeps the blocking shape below.
        // ONE logical verification, ONE negotiation. The temperature pin is
        // negotiated HERE while every other knob is negotiated inside
        // `post_ai_json`, so with the capability knowledge scoped to a single
        // call the retry below started from nothing and re-offered the effort
        // tier and the stream this endpoint had just refused — one extra POST
        // per refusal, to learn the same thing twice (worst case 6 POSTs for
        // one verify, against 4 before the tier existed). `Refused` is owned
        // by the whole verification and passed into both attempts.
        let mut refused = super::Refused::default();
        let value: Value = match super::post_ai_json_with(
            &url,
            key,
            body.clone(),
            super::VERIFY_TIMEOUT_SECS,
            super::SseFamily::Chat,
            self.effort.as_deref(),
            &mut refused,
        ) {
            // Reasoning-class models accept only the DEFAULT temperature and
            // reject the deterministic pin with a 400 blaming it — the pin is
            // a preference, not a requirement, so drop it and retry once
            // (same attribution rule as every negotiated knob).
            Err(AdvisorError::Http { status: 400 | 404 | 422, body: b })
                if super::error_blames_param(&b, "temperature") =>
            {
                eprintln!(
                    "  note: {} rejected temperature=0 — retrying without the pin",
                    self.model
                );
                if let Some(o) = body.as_object_mut() {
                    o.remove("temperature");
                }
                super::post_ai_json_with(
                    &url,
                    key,
                    body,
                    super::VERIFY_TIMEOUT_SECS,
                    super::SseFamily::Chat,
                    self.effort.as_deref(),
                    &mut refused,
                )?
            }
            other => other?,
        };

        let text = value
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AdvisorError::ModelFailure(
                    super::BoundedUntrustedText::diagnostic(
                        &format!("no choices[0].message.content in chat response: {value}"),
                        &[key],
                    )
                    .into_string(),
                )
            })?;

        // The shared parser bounds both recovery work and every returned string.
        parse_verdict(text, &[key])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advisor::tests::{join_stub, stub_endpoint};
    use crate::advisor::Decision;

    fn fixture_meta() -> Meta {
        Meta {
            make: "TestCam".into(),
            model: "T1".into(),
            lens: None,
            iso: Some(100),
            shutter: Some("1/125".into()),
            aperture: Some(4.0),
            focal_length_mm: Some(50.0),
            exposure_bias_ev: Some(0.0),
            date_time: None,
            width: 6000,
            height: 4000,
            as_shot_wb_coeffs: [1.0; 4],
        }
    }

    fn fixture_hist() -> Histogram {
        Histogram {
            luma: vec![0; 256],
            r: vec![0; 256],
            g: vec![0; 256],
            b: vec![0; 256],
            clip_black_pct: 0.0,
            clip_white_pct: 0.0,
            sample_pixels: 1,
        }
    }

    /// ONE verification is ONE negotiation, even though it is negotiated at
    /// two levels: the temperature pin here, every other knob inside
    /// `post_ai_json`. When the capability knowledge lived only inside that
    /// call, the retry below started from nothing and re-offered what this
    /// endpoint had just refused — a POST per refusal, spent to learn the same
    /// answer a second time (6 for one verify at worst, against 4 before the
    /// effort tier existed). Counted over a real transport, because only a
    /// count can say "did not ask twice".
    #[test]
    fn the_temperature_retry_does_not_re_offer_what_the_endpoint_already_refused() {
        const VERDICT: &str =
            r#"{"choices":[{"message":{"content":"{\"decision\":\"accept\",\"reasons\":[]}"}}]}"#;
        let (url, seen, handle) = stub_endpoint(vec![
            (
                400,
                "application/json",
                r#"{"error":{"param":"reasoning_effort","message":"unsupported"}}"#.into(),
            ),
            (
                400,
                "application/json",
                r#"{"error":{"param":"temperature","message":"only the default is supported"}}"#
                    .into(),
            ),
            (200, "application/json", VERDICT.into()),
        ]);
        let verifier = OpenAiVerifier {
            api_key: Some("test-key".into()),
            model: "m".into(),
            base_url: url,
            effort: Some("high".into()),
        };
        let verdict = verifier
            .verify(&EditRecipe::default(), &fixture_meta(), &fixture_hist())
            .expect("both refusals are absorbed and the third POST verifies");
        assert!(matches!(verdict.decision, Decision::Accept));
        join_stub(handle);
        let bodies = seen.lock().unwrap().clone();
        assert_eq!(bodies.len(), 3, "one POST per refusal, then the answer: {bodies:?}");
        let third: Value = serde_json::from_str(&bodies[2]).unwrap();
        assert!(
            third.get("reasoning_effort").is_none(),
            "the tier stays dropped ACROSS the temperature retry: {third}"
        );
        assert!(
            third.get("temperature").is_none(),
            "…and the pin this retry exists to drop is gone: {third}"
        );
    }
}
