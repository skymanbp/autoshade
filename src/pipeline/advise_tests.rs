//! The analysis chain's revision rounds, over canned provider replies on a
//! loopback stub (no network, no key, no paid call).

use super::*;
use crate::advisor::tests::{join_stub, stub_endpoint};
use serde_json::Value;

/// A config whose image role AND analysis role both talk to `url`.
pub(crate) fn stub_cfg(url: &str) -> crate::config::Config {
    crate::config::Config {
        // Fixture placeholders, not credentials — the stub only reads bodies.
        openai_api_key: Some("test-key".into()),
        openai_model: "test-vision".into(),
        openai_base_url: url.to_string(),
        openai_image_model: "test-image".into(),
        openai_image_quality: "auto".into(),
        openai_image_max_px: 4_000_000,
        image_provider: "api".into(),
        image_effort: None,
        image_api_family: "auto".into(),
        image_gen_endpoint: None,
        analysis_provider: "api".into(),
        analysis_model: "test-chat".into(),
        analysis_effort: None,
        claude_bin: "claude".into(),
        analysis_api_key: Some("test-key".into()),
        analysis_base_url: url.to_string(),
        python_bin: "python".into(),
        denoise_model: "scunet_color_real_psnr".into(),
        denoise_script: String::new(),
        denoise_raw_script: String::new(),
        weights_dir: String::new(),
        segment_script: String::new(),
        depth_script: String::new(),
        face_script: String::new(),
        embed_script: String::new(),
        correspond_script: String::new(),
        describe_script: String::new(),
        style_strength: 0.5,
        send_reference_image: false,
    }
}

/// A small baked source (a PNG with a gradient and a bright patch) in a fresh
/// directory under the temp root; returns the directory and the file.
pub(crate) fn fixture_png(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("autoshade-advise-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let img = image::RgbImage::from_fn(96, 64, |x, y| {
        if (40..56).contains(&x) && (20..36).contains(&y) {
            image::Rgb([235, 230, 220])
        } else {
            let v = (40 + x + y) as u8;
            image::Rgb([v, v.saturating_sub(10), v.saturating_sub(25)])
        }
    });
    let path = dir.join("photo.png");
    img.save(&path).expect("write the fixture PNG");
    (dir, path)
}

/// A Responses-API reply carrying `recipe` as the structured output.
pub(crate) fn propose_reply(recipe: Value) -> String {
    serde_json::json!({
        "output": [{ "content": [{ "type": "output_text", "text": recipe.to_string() }] }]
    })
    .to_string()
}

/// A Chat Completions reply carrying a verdict.
pub(crate) fn verdict_reply(decision: &str, hint: Option<&str>) -> String {
    let verdict = serde_json::json!({ "decision": decision, "reasons": ["r"], "revised_hint": hint });
    serde_json::json!({
        "choices": [{ "finish_reason": "stop", "message": { "content": verdict.to_string() } }]
    })
    .to_string()
}

/// The image and the instruction one recorded propose request carried.
pub(crate) fn propose_parts(body: &str) -> (String, String) {
    let v: Value = serde_json::from_str(body).expect("the stub records raw JSON");
    let content = v["input"][0]["content"].as_array().expect("a Responses propose body");
    let image = content
        .iter()
        .find(|c| c["type"] == "input_image")
        .and_then(|c| c["image_url"].as_str())
        .expect("one input image")
        .to_string();
    let text = content
        .iter()
        .find(|c| c["type"] == "input_text")
        .and_then(|c| c["text"].as_str())
        .expect("the instruction")
        .to_string();
    (image, text)
}

pub(crate) fn analyze(png: &std::path::Path, url: &str) -> (EditRecipe, crate::advisor::Verdict) {
    let (recipe, verdict, _) = super::super::produce_recipe(
        png,
        None,
        &stub_cfg(url),
        false,
        None,
        None,
        super::super::GradeRequest::default(),
        false,
        crate::diag::stderr(),
    )
    .expect("the canned chain completes");
    (recipe, verdict)
}

/// A verifier-driven revision edits the model's OWN answer while looking at
/// it: the second propose request carries the first reply's recipe (its JSON
/// in the envelope), a frame that is the render of that recipe rather than the
/// original, and the render's histogram; a reply that changes ONE slider (and
/// leaves the rest out, as a schema-ignoring endpoint may) keeps every other
/// control as the first reply set it.
///
/// MUTATION: send `proposer_preview` (the original frame) from `revise` and
/// the frame assertion fails; drop `held: Some(held)` from its context and the
/// contrast falls back to neutral 0.
#[test]
fn a_revision_round_shows_the_model_its_own_answer_and_edits_it() {
    let first = serde_json::json!({
        "exposure_ev": 0.3, "contrast": 12.0, "shadows": 5.0, "rationale": "first answer"
    });
    let (url, seen, handle) = stub_endpoint(vec![
        (200, "application/json", propose_reply(first)),
        (200, "application/json", verdict_reply("revise", Some("lift the shadows about +15"))),
        (200, "application/json", propose_reply(serde_json::json!({ "shadows": 20.0, "rationale": "second" }))),
        (200, "application/json", verdict_reply("accept", None)),
    ]);
    let (dir, png) = fixture_png("c1");
    let (recipe, verdict) = analyze(&png, &url);
    join_stub(handle);
    let bodies = seen.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4, "propose, verify, revise, verify");
    let (frame1, text1) = propose_parts(&bodies[0]);
    let (frame2, text2) = propose_parts(&bodies[2]);
    assert!(!text1.contains("REVISION ROUND"), "a first proposal is not a revision");
    assert_ne!(frame1, frame2, "the revision is shown its own render, not the original frame");
    assert!(text2.contains("REVISION ROUND"), "{text2}");
    assert!(
        text2.contains("\"contrast\":12.0") && text2.contains("\"exposure_ev\":0.3"),
        "the first reply's recipe rides the revision request: {text2}"
    );
    assert!(text2.contains("HISTOGRAM OF IMAGE 1"), "the render's own numbers: {text2}");
    assert!(text2.contains("lift the shadows about +15"), "the reviewer's note: {text2}");
    assert_eq!(recipe.shadows, 20.0, "the one slider the revision moved");
    assert_eq!(recipe.contrast, 12.0, "an untouched slider keeps the first answer's value");
    assert_eq!(recipe.exposure_ev, 0.3, "an untouched slider keeps the first answer's value");
    assert_eq!(verdict.decision, crate::advisor::Decision::Accept);
    let _ = std::fs::remove_dir_all(dir);
}

/// The envelope reuses the ADJUST protocol with the held recipe as the
/// CURRENT EDIT, says whose answer IMAGE 1 is, and carries the photographer's
/// own words when there are any.
#[test]
fn the_revision_envelope_is_the_adjust_envelope_around_the_held_answer() {
    let text = revision_envelope("{\"contrast\":12.0}", Some("warmer"), "p50 0.41");
    assert!(text.contains("YOUR OWN previous answer"), "{text}");
    assert!(text.contains("HISTOGRAM OF IMAGE 1 (your previous answer rendered): p50 0.41"), "{text}");
    assert!(text.contains("CURRENT EDIT (EditRecipe JSON): {\"contrast\":12.0}"), "{text}");
    assert!(text.contains("photographer's request (warmer)"), "{text}");
    let bare = revision_envelope("{}", None, "h");
    assert!(!bare.contains("photographer's request"), "{bare}");
    assert!(bare.contains("apply the REVISION NOTE"), "{bare}");
}

/// A revision's null is "the current value stands", and the current value is
/// the held answer's: a nullable control the held answer stated and the
/// revision left at null keeps that value and stays stated; one the held
/// answer never stated stays unstated.
///
/// MUTATION: return `revised_opinions` unchanged from `inherit_held_opinions`
/// and the grain assertions fail.
#[test]
fn a_revision_null_keeps_the_held_answers_stated_value() {
    let held = EditRecipe { grain: 30.0, ..Default::default() };
    let held_op = Opinions::default().with("grain");
    let mut revised = EditRecipe { grain: 0.0, exposure_ev: 0.4, ..Default::default() };
    let op = inherit_held_opinions(&mut revised, Opinions::default(), &held, held_op);
    assert_eq!(revised.grain, 30.0, "the held answer's grain stands");
    assert!(op.stated("grain"), "…and it is still an opinion, not the base's to replace");
    assert_eq!(revised.exposure_ev, 0.4, "the revision's own values are untouched");
    // Stated by the revision: the revision's number wins.
    let mut revised = EditRecipe { grain: 5.0, ..Default::default() };
    let op = inherit_held_opinions(&mut revised, Opinions::default().with("grain"), &held, held_op);
    assert_eq!(revised.grain, 5.0);
    assert!(op.stated("grain"));
    // Neither stated it: still no opinion.
    let mut revised = EditRecipe::default();
    let op = inherit_held_opinions(&mut revised, Opinions::default(), &EditRecipe::default(), Opinions::default());
    assert!(!op.stated("grain"));
}

/// The absent-key fill touches only the schema's own non-nullable keys and
/// never the prose.
///
/// MUTATION: drop the `rationale` exclusion and the held prose is copied.
#[test]
fn only_absent_schema_keys_are_filled_from_the_held_answer() {
    let held = EditRecipe {
        contrast: 12.0,
        grain: 30.0,
        rationale: "held prose".into(),
        ..Default::default()
    };
    let mut reply = serde_json::json!({ "shadows": 20.0 });
    crate::advisor::fill_absent_from_held(&mut reply, &held);
    assert_eq!(reply["contrast"], 12.0, "an absent slider keeps the held value");
    assert_eq!(reply["shadows"], 20.0, "a present one is the reply's");
    assert!(reply.get("grain").is_none(), "an absent nullable stays 'no opinion'");
    assert!(reply.get("rationale").is_none(), "the held prose is not the revision's");
    assert!(reply.get("colour_field").is_none(), "an engine-only field is not the schema's");
}
