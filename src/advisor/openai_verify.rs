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
        let value: Value = match super::post_ai_json(
            &url,
            key,
            body.clone(),
            super::VERIFY_TIMEOUT_SECS,
            super::SseFamily::Chat,
            self.effort.as_deref(),
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
                super::post_ai_json(
                    &url,
                    key,
                    body,
                    super::VERIFY_TIMEOUT_SECS,
                    super::SseFamily::Chat,
                    self.effort.as_deref(),
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
