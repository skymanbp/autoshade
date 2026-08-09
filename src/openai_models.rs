//! List the model ids an OpenAI-compatible account can use (`GET {base}/models`),
//! so the Settings UI can offer a real pick-list instead of a blank text box the
//! user has to guess into. Pure read; never logs the key.

use anyhow::{anyhow, Context, Result};

/// Fetch the sorted, de-duplicated list of model ids from `{base_url}/models`.
/// `base_url` is the OpenAI-compatible API root (e.g. `https://api.openai.com/v1`).
const MODELS_RESPONSE_CAP: u64 = 4 * 1024 * 1024;
const MODEL_ITEM_MAX: usize = 4096;
const MODEL_ID_MAX_BYTES: usize = 256;

fn extract_model_ids(value: &serde_json::Value) -> Result<Vec<String>> {
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("no data[] in /models response"))?;
    if data.len() > MODEL_ITEM_MAX {
        return Err(anyhow!(
            "/models returned {} items; maximum is {MODEL_ITEM_MAX}",
            data.len()
        ));
    }

    let mut ids = Vec::with_capacity(data.len());
    for id in data
        .iter()
        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
    {
        let projected =
            crate::advisor::BoundedUntrustedText::new(id, MODEL_ID_MAX_BYTES, &[]);
        if projected.as_str() != id {
            return Err(anyhow!(
                "/models returned an identifier longer than {MODEL_ID_MAX_BYTES} bytes \
                 or containing control characters"
            ));
        }
        ids.push(projected.into_string());
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

pub fn list_models(base_url: &str, api_key: &str) -> Result<Vec<String>> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("no API key — set one in Settings (save first), then fetch"));
    }
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(20))
        .set("Authorization", &format!("Bearer {api_key}"))
        .call();
    let value: serde_json::Value = match resp {
        Ok(r) => crate::advisor::into_json_capped_at(r, MODELS_RESPONSE_CAP)
            .context("parse bounded /models response")?,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let safe =
                crate::advisor::BoundedUntrustedText::diagnostic(&body, &[api_key]);
            return Err(anyhow!("models API {code}: {safe}"));
        }
        Err(ureq::Error::Transport(t)) => return Err(anyhow!("transport: {t}")),
    };
    extract_model_ids(&value)
}

/// True if `id` looks like an image-generation model (gpt-image-*, *image*).
pub fn is_image_model(id: &str) -> bool {
    id.contains("image")
}

/// True if `id` looks like a text/vision chat model (the proposer/verifier roles),
/// excluding audio/embedding/etc. variants that can't do vision-chat.
pub fn is_chat_model(id: &str) -> bool {
    let bad = ["audio", "realtime", "transcribe", "tts", "whisper", "embedding", "moderation"];
    let family = id.starts_with("gpt")
        || id.starts_with("chatgpt")
        || (id.starts_with('o') && id.as_bytes().get(1).is_some_and(u8::is_ascii_digit));
    family && !is_image_model(id) && !bad.iter().any(|b| id.contains(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_real_account_ids() {
        // Ids verified present in a live /models probe (2026-06).
        assert!(is_image_model("gpt-image-2"));
        assert!(is_image_model("gpt-image-1.5"));
        assert!(is_image_model("chatgpt-image-latest"));
        assert!(!is_chat_model("gpt-image-2")); // an image model is not a chat model

        assert!(is_chat_model("gpt-5.5"));
        assert!(is_chat_model("gpt-4o"));
        assert!(is_chat_model("o3"));
        assert!(is_chat_model("o4-mini"));
        assert!(!is_chat_model("gpt-4o-audio-preview"));
        assert!(!is_chat_model("text-embedding-3-large"));
        assert!(!is_image_model("gpt-5.5"));
    }

        #[test]
        fn model_catalogues_bound_items_and_identifiers_before_sorting() {
            let excessive = serde_json::json!({
                "data": (0..=MODEL_ITEM_MAX)
                    .map(|i| serde_json::json!({"id": format!("gpt-{i}")}))
                    .collect::<Vec<_>>()
            });
            assert!(extract_model_ids(&excessive).is_err());

            let long = serde_json::json!({
                "data": [{"id": "x".repeat(MODEL_ID_MAX_BYTES + 1)}]
            });
            assert!(extract_model_ids(&long).is_err());

            let controlled = serde_json::json!({"data": [{"id": "gpt-safe\nforged"}]});
            assert!(extract_model_ids(&controlled).is_err());
        }

    // FILE: src/config.rs  (append inside the existing `mod tests`)
}
