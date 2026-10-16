//! List the models an OpenAI-compatible account can use (`GET {base}/models`),
//! so the Settings UI can offer a real pick-list instead of a blank text box the
//! user has to guess into — and, where the endpoint says so, which reasoning
//! tiers each model takes and whether it accepts images. Pure read; never
//! logs the key.

use anyhow::{anyhow, Context, Result};

const MODELS_RESPONSE_CAP: u64 = 4 * 1024 * 1024;
const MODEL_ITEM_MAX: usize = 4096;
const MODEL_ID_MAX_BYTES: usize = 256;
/// The most effort tiers one model may declare; a longer list is cut, since
/// the picker shows them one per row.
const EFFORT_OPTIONS_MAX: usize = 16;

/// One model as `/models` described it.
///
/// The two capabilities are `Option` because the plain OpenAI listing says
/// nothing about either — `None` is "the endpoint did not say", and the
/// pickers then offer their fixed suggestions as before. `Some` comes from
/// the extended listing some providers serve (Venice's
/// `model_spec.capabilities`, OpenRouter's `architecture` and
/// `supported_parameters`): `effort_options` is the exact tier list the
/// model takes (empty: it takes none, the request must carry no tier) and
/// `vision` whether it accepts an image part.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ModelInfo {
    pub id: String,
    pub effort_options: Option<Vec<String>>,
    pub vision: Option<bool>,
}

impl From<&str> for ModelInfo {
    /// An id with nothing known about it — the plain listing's shape.
    fn from(id: &str) -> Self {
        Self { id: id.to_string(), ..Default::default() }
    }
}

/// What one `/models` item says about its reasoning tiers, when it says
/// anything (see [`ModelInfo::effort_options`]).
///
/// Venice: `model_spec.capabilities.supportsReasoning` false ⇒ no tier;
/// `reasoningEffortOptions` ⇒ that list (through `config::effort`'s shape
/// rule, so a stray value cannot reach a request). OpenRouter:
/// `supported_parameters` without `reasoning` ⇒ no tier; with it the tiers
/// are the three common ones. Anything else ⇒ unknown.
fn effort_options_of(model: &serde_json::Value) -> Option<Vec<String>> {
    let tiers = |v: &serde_json::Value| -> Option<Vec<String>> {
        let list: Vec<String> = v
            .as_array()?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter_map(|s| crate::config::effort(Some(s.to_string())))
            .take(EFFORT_OPTIONS_MAX)
            .collect();
        Some(list)
    };
    if let Some(caps) = model.get("model_spec").and_then(|s| s.get("capabilities")) {
        let reasons = caps.get("supportsReasoning").and_then(serde_json::Value::as_bool);
        let effort = caps.get("supportsReasoningEffort").and_then(serde_json::Value::as_bool);
        if reasons == Some(false) || effort == Some(false) {
            return Some(Vec::new());
        }
        if let Some(list) = caps.get("reasoningEffortOptions").and_then(tiers) {
            return Some(list);
        }
        return None;
    }
    let params = model.get("supported_parameters")?.as_array()?;
    let names: Vec<&str> = params.iter().filter_map(serde_json::Value::as_str).collect();
    if names.iter().any(|n| *n == "reasoning" || *n == "reasoning_effort") {
        Some(["low", "medium", "high"].map(String::from).to_vec())
    } else {
        Some(Vec::new())
    }
}

/// Whether one `/models` item accepts an image part, when it says: Venice's
/// `model_spec.capabilities.supportsVision`, OpenRouter's
/// `architecture.input_modalities` (or `modality`, "text+image->text").
fn vision_of(model: &serde_json::Value) -> Option<bool> {
    if let Some(v) = model
        .get("model_spec")
        .and_then(|s| s.get("capabilities"))
        .and_then(|c| c.get("supportsVision"))
        .and_then(serde_json::Value::as_bool)
    {
        return Some(v);
    }
    let arch = model.get("architecture")?;
    if let Some(inputs) = arch.get("input_modalities").and_then(serde_json::Value::as_array) {
        return Some(inputs.iter().any(|m| m.as_str() == Some("image")));
    }
    let modality = arch.get("modality")?.as_str()?;
    let (inputs, _) = modality.split_once("->")?;
    Some(inputs.split('+').any(|m| m.trim() == "image"))
}

fn extract_models(value: &serde_json::Value) -> Result<Vec<ModelInfo>> {
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

    let mut models = Vec::with_capacity(data.len());
    for model in data {
        let Some(id) = model.get("id").and_then(serde_json::Value::as_str) else { continue };
        let projected =
            crate::advisor::BoundedUntrustedText::new(id, MODEL_ID_MAX_BYTES, &[]);
        if projected.as_str() != id {
            return Err(anyhow!(
                "/models returned an identifier longer than {MODEL_ID_MAX_BYTES} bytes \
                 or containing control characters"
            ));
        }
        models.push(ModelInfo {
            id: projected.into_string(),
            effort_options: effort_options_of(model),
            vision: vision_of(model),
        });
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

/// Fetch the models from `{base_url}/models`, sorted and de-duplicated by
/// id, with what the listing says about each ([`ModelInfo`]).
/// `base_url` is the OpenAI-compatible API root (e.g. `https://api.openai.com/v1`).
pub fn list_models(base_url: &str, api_key: &str) -> Result<Vec<ModelInfo>> {
    // Trim BEFORE the header is built, and refuse what cannot ride one. ureq
    // quotes the whole rejected header line back — `Authorization: Bearer
    // <the real key>` (2.12.1 `header.rs:147`) — and this call's errors are
    // shown verbatim in the Settings panel, so a key with a trailing newline
    // from a copy/paste used to print itself into the UI. Same boundary rule
    // as `config::header_safe_key`; the Transport arm below is the second
    // layer.
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(anyhow!("no API key — set one in Settings (save first), then fetch"));
    }
    if !crate::config::key_is_header_safe(api_key) {
        return Err(anyhow!(
            "the API key contains characters that cannot appear in an HTTP header \
             (a stray space or newline from a copy/paste?) — re-copy it and save again"
        ));
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
        // Redacted like the Status arm above: a transport diagnostic can carry
        // the header line (see the boundary check at the top) or a base URL
        // with embedded credentials, and this string is displayed in Settings.
        Err(ureq::Error::Transport(t)) => {
            let safe =
                crate::advisor::BoundedUntrustedText::diagnostic(&t.to_string(), &[api_key]);
            return Err(anyhow!("transport: {safe}"));
        }
    };
    extract_models(&value)
}

/// True if `id` looks like an image-generation model (gpt-image-*, *image*).
/// Case-FOLDED, like every other id test here: `Qwen/Qwen-Image` names the
/// same model as `qwen-image`.
pub fn is_image_model(id: &str) -> bool {
    id.to_ascii_lowercase().contains("image")
}

/// True if `id` may be OFFERED as a chat model — the proposer/verifier roles.
///
/// EXCLUSION, not an allowlist. This used to require an OpenAI-branded prefix
/// (`gpt*`, `chatgpt*`, `o<digit>`), which quietly made "fetch the models this
/// endpoint serves" mean "fetch the models api.openai.com serves": every id on
/// any other OpenAI-compatible endpoint — a Codex bridge exposing `claude-*`,
/// LM Studio, vLLM, Qwen, DeepSeek — was filtered to nothing and the picker
/// silently fell back to two hardcoded ids. The base URL is configurable
/// precisely so those endpoints work, so the classifier must not re-hardcode
/// the vendor.
///
/// What a name CAN and CANNOT decide, since an exclusion list only removes
/// what it recognises:
/// - It must actually recognise it. The match is case-FOLDED because the ids
///   this relaxation exists to admit are vendor-formatted, and the canonical
///   `Qwen/Qwen2-Audio-7B-Instruct` sailed straight past a lowercase `audio`
///   test — the exclusions claimed to work vendor-wide and did not.
/// - "Offered" is not "vision-capable", and nothing in an id proves it is:
///   the picker is a suggestion list beside a free-text field, and a wrong
///   pick fails loudly — and unbilled, on a 400-class refusal — at the first
///   call. Filtering down to vision models would also be wrong for the OTHER
///   role: the analysis verifier never sends the image (`openai_verify.rs`),
///   so a text-only chat model is a correct pick there.
pub fn is_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    // A different MODALITY: these serve another kind of call entirely.
    let other_modality = [
        "audio", "realtime", "transcribe", "tts", "whisper", "embedding", "moderation", "rerank",
        "dall-e", "sora", "video", "speech", "guard",
    ];
    // A different ENDPOINT FAMILY: OpenAI's two surviving legacy base models
    // answer `/completions` only, so a chat or responses call is refused
    // outright. Exact ids — a substring test on `davinci` would also strike
    // third-party ids that merely mention it.
    let completion_only = ["babbage-002", "davinci-002"];
    !id.is_empty()
        && !is_image_model(&id)
        && !other_modality.iter().any(|b| id.contains(b))
        && !completion_only.contains(&id.as_str())
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

        // The base URL is configurable, so the classifier must not be. Ids
        // served by other OpenAI-compatible endpoints used to be filtered to
        // nothing by an OpenAI-branded prefix test, which left the picker
        // offering two hardcoded ids on every third-party endpoint.
        for id in [
            "claude-opus-4-6",          // a Codex/Claude bridge
            "qwen2.5-vl-72b-instruct",  // vLLM / LM Studio
            "deepseek-chat",
            // Text-only, and correct to offer: the ANALYSIS verifier judges
            // the recipe from numbers and never sends the image.
            "llama-3.3-70b-instruct",
            "mistral-large-latest",
        ] {
            assert!(is_chat_model(id), "{id} disappeared from the picker");
        }
        // …while the exclusions keep doing their job vendor-wide — which they
        // only do when the match is case-FOLDED. These are the canonical
        // upstream spellings, not the lowercase ones a test author reaches
        // for; the first exclusion list matched raw bytes and let every one
        // of them through.
        for id in [
            "Qwen/Qwen2-Audio-7B-Instruct",
            "openai/whisper-large-v3",
            "BAAI/bge-reranker-v2-m3",
            "Meta-Llama-Guard-2-8B",
        ] {
            assert!(!is_chat_model(id), "{id} must not be offered as a chat model");
        }
        assert!(is_image_model("Qwen/Qwen-Image"), "…and the image side folds case too");
        for id in ["qwen-audio-turbo", "bge-reranker-v2", "dall-e-3", "sora-2", "llama-guard-3"] {
            assert!(!is_chat_model(id), "{id} must not be offered as a chat model");
        }
        // Not a modality — a different ENDPOINT: the two legacy base models
        // answer /completions only, so a chat call to them is refused.
        for id in ["babbage-002", "davinci-002"] {
            assert!(!is_chat_model(id), "{id} is a completion-only model");
        }
        assert!(!is_chat_model(""), "an empty id is not a model");
    }

    #[test]
    fn model_catalogues_bound_items_and_identifiers_before_sorting() {
        let excessive = serde_json::json!({
            "data": (0..=MODEL_ITEM_MAX)
                .map(|i| serde_json::json!({"id": format!("gpt-{i}")}))
                .collect::<Vec<_>>()
        });
        assert!(extract_models(&excessive).is_err());

        let long = serde_json::json!({
            "data": [{"id": "x".repeat(MODEL_ID_MAX_BYTES + 1)}]
        });
        assert!(extract_models(&long).is_err());

        let controlled = serde_json::json!({"data": [{"id": "gpt-safe\nforged"}]});
        assert!(extract_models(&controlled).is_err());
    }

    /// The capabilities, where a listing carries them: Venice's
    /// `model_spec.capabilities` and OpenRouter's `architecture` /
    /// `supported_parameters`; the plain OpenAI listing says nothing, and
    /// "nothing" must stay distinct from "none" — the pickers keep their
    /// fixed suggestions on `None` and offer no tier on `Some([])`.
    #[test]
    fn the_listing_says_which_tiers_and_images_a_model_takes_when_it_says_anything() {
        let listing = serde_json::json!({ "data": [
            { "id": "plain" },
            { "id": "venice-reasoner", "model_spec": { "capabilities": {
                "supportsReasoning": true, "supportsReasoningEffort": true,
                "reasoningEffortOptions": ["low", "medium", "high", "Bad Tier!"],
                "supportsVision": true } } },
            { "id": "venice-plain", "model_spec": { "capabilities": {
                "supportsReasoning": false, "supportsVision": false } } },
            { "id": "router-vision", "architecture": { "input_modalities": ["text", "image"] },
              "supported_parameters": ["temperature", "reasoning"] },
            { "id": "router-text", "architecture": { "modality": "text->text" },
              "supported_parameters": ["temperature"] }
        ] });
        let models = extract_models(&listing).unwrap();
        let by_id = |id: &str| models.iter().find(|m| m.id == id).unwrap().clone();
        assert_eq!(by_id("plain"), ModelInfo::from("plain"), "the plain listing knows nothing");
        let venice = by_id("venice-reasoner");
        assert_eq!(
            venice.effort_options.as_deref(),
            Some(&["low".to_string(), "medium".into(), "high".into()][..]),
            "the tiers as listed, a malformed one dropped by config::effort's rule"
        );
        assert_eq!(venice.vision, Some(true));
        let plain = by_id("venice-plain");
        assert_eq!(plain.effort_options.as_deref(), Some(&[][..]), "no tier at all");
        assert_eq!(plain.vision, Some(false));
        let router = by_id("router-vision");
        assert_eq!(router.effort_options.as_deref().map(<[String]>::len), Some(3));
        assert_eq!(router.vision, Some(true));
        let text = by_id("router-text");
        assert_eq!(text.effort_options.as_deref(), Some(&[][..]));
        assert_eq!(text.vision, Some(false));
    }
}
