//! Choosing between the two answers, over canned replies on a loopback stub
//! and fakes for the segmenter, the fit and the renders.

use super::*;
use crate::advisor::tests::{join_stub, stub_endpoint};
use crate::pipeline::advise::tests::{fixture_png, propose_reply, stub_cfg, verdict_reply};
use crate::pipeline::statement::LookFitResult;
use serde_json::{Value, json};

const SKY_GROUND: &str = r#""sky":{"L":[50,60,70,80,90],"ab_shadows":[5,10],"ab_mids":[8,14],"ab_highlights":[10,20]},"ground":{"L":[5,10,15,20,30],"ab_shadows":[0,0],"ab_mids":[0,0],"ab_highlights":[0,0]}"#;

fn statement_reply() -> String {
    let inner = format!(r#"{{"look":"a warm dusk","guides":[],"areas":{{{SKY_GROUND}}}}}"#);
    json!({ "output": [{ "content": [{ "type": "output_text", "text": inner }] }] }).to_string()
}

fn judgement(decision: &str, score: u32, hint: Option<&str>) -> String {
    propose_reply(json!({ "score": score, "decision": decision, "critique": "seen", "hint": hint }))
}

fn choice(preferred: &str) -> String {
    propose_reply(json!({ "preferred": preferred, "score": 88, "decision": "accept", "critique": "the warmer one", "hint": null }))
}

/// A solid frame whose red channel tells which recipe it renders:
/// 200 × exposure (the calibration-only reference 0, today's 0.3 → 60, the
/// statement's 0.5 → 100).
fn tagged(r: &EditRecipe) -> image::DynamicImage {
    let red = (r.exposure_ev * 200.0).round().clamp(0.0, 255.0) as u8;
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(32, 24, image::Rgb([red, 90, 90])))
}

fn frame_image() -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(96, 64, |x, y| {
        if y < 32 { image::Rgb([90 + (x / 3) as u8, 110, 150]) } else { image::Rgb([20 + (x / 4) as u8, 18, 12]) }
    }))
}

/// Fakes: a RAW (by the seam's word), a sky over a ground, a fit answering
/// exposure 0.5, renders tagged by exposure.
fn tools(is_raw: bool) -> StatementTools<'static> {
    StatementTools {
        is_raw: Box::new(move |_| is_raw),
        source: Box::new(|_| Ok((crate::render::Plate::baked(frame_image()), EditRecipe::default()))),
        sky: Box::new(|f| Ok(image::GrayImage::from_fn(f.width(), f.height(), |_, y| image::Luma([if y < f.height() / 2 { 255 } else { 0 }])))),
        reference: Box::new(|_| anyhow::bail!("no references in these tests")),
        fit: Box::new(|_, _, _, _| {
            Ok(LookFitResult { recipe: EditRecipe { exposure_ev: 0.5, ..Default::default() }, notes: Vec::new(), err_before: 0.1, err_after: 0.01 })
        }),
        render: Box::new(|_, r| Ok(tagged(r))),
        dump: None,
    }
}

fn run(approach: AnalysisApproach, script: Vec<String>, judge: bool, is_raw: bool) -> (EditRecipe, Verdict, Vec<Note>, Vec<String>) {
    let (url, seen, handle) = stub_endpoint(script.into_iter().map(|b| (200, "application/json", b)).collect());
    let (dir, png) = fixture_png(&format!("e4-{}", approach.as_str()));
    let req = GradeRequest { approach, ..GradeRequest::default() };
    let t = tools(is_raw);
    let (recipe, verdict, notes) =
        produce_develop_with(&png, None, &stub_cfg(&url), false, None, None, req, judge, crate::diag::silent(), &t)
            .expect("the canned chain completes");
    join_stub(handle);
    let bodies = seen.lock().unwrap().clone();
    let _ = std::fs::remove_dir_all(dir);
    (recipe, verdict, notes, bodies)
}

/// Today's chain with the judge's first look accepting: propose, verify, judge.
fn controls_script() -> Vec<String> {
    vec![
        propose_reply(json!({ "exposure_ev": 0.3, "rationale": "first" })),
        verdict_reply("accept", None),
        judgement("accept", 90, None),
    ]
}

/// `Controls` is today's flow, byte for byte: the same requests in the same
/// order and the same recipe, verdict and notes as `produce_recipe` itself.
///
/// MUTATION: route `Controls` through the statement path (or add a call) — the
/// stub's script is not consumed exactly, or the bodies differ, and this goes red.
#[test]
fn controls_is_byte_for_byte_todays_flow() {
    let (url, seen, handle) = stub_endpoint(controls_script().into_iter().map(|b| (200, "application/json", b)).collect());
    let (dir, png) = fixture_png("e4-today");
    let today = crate::pipeline::produce_recipe(&png, None, &stub_cfg(&url), false, None, None, GradeRequest::default(), true, crate::diag::silent())
        .expect("today's chain");
    join_stub(handle);
    let today_bodies = seen.lock().unwrap().clone();
    let _ = std::fs::remove_dir_all(dir);
    let (recipe, verdict, notes, bodies) = run(AnalysisApproach::Controls, controls_script(), true, true);
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies, today_bodies, "the same requests, byte for byte");
    assert_eq!((recipe, verdict, notes.len()), (today.0, today.1, today.2.len()));
}

fn both_script(statement: String, choice_reply: String) -> Vec<String> {
    let mut s = controls_script();
    s.push(statement);
    s.push(choice_reply);
    s
}

/// `Both` scripts propose → verify → judge → statement → choice, and adopts
/// the statement's develop when the judge prefers it ("B").
///
/// MUTATION: adopt the statement whatever the choice says (or never) — one of
/// the two runs comes back with the wrong exposure.
#[test]
fn both_adopts_the_statement_only_when_the_judge_prefers_it() {
    let (recipe, verdict, notes, bodies) = run(AnalysisApproach::Both, both_script(statement_reply(), choice("B")), true, true);
    assert_eq!(bodies.len(), 5, "propose, verify, judge, statement, choice");
    assert_eq!(recipe.exposure_ev, 0.5, "the statement's develop is adopted");
    assert_eq!(verdict.decision, Decision::Accept, "the choice's verdict describes it");
    assert!(recipe.rationale.contains(" [Stated look: a warm dusk]"), "{}", recipe.rationale);
    assert!(notes.iter().any(|n| n.key == keys::STATEMENT_CHOSEN));
    let (recipe, _, notes, _) = run(AnalysisApproach::Both, both_script(statement_reply(), choice("A")), true, true);
    assert_eq!(recipe.exposure_ev, 0.3, "today's develop is kept");
    assert!(notes.iter().any(|n| n.key == keys::STATEMENT_NOT_CHOSEN));
    assert!(!recipe.rationale.contains("Stated look"), "the loser's notes do not ride: {}", recipe.rationale);
}

/// A failed statement call, or a failed choice call, keeps today's develop
/// and says why.
///
/// MUTATION: propagate the statement's error (`?`) instead of keeping today's
/// develop — the first run fails on the `expect`.
#[test]
fn both_keeps_todays_develop_when_the_statement_or_the_choice_fails() {
    let refused = json!({ "output": [{ "content": [{ "type": "output_text", "text": r#"{"look":"x","guides":[],"areas":{}}"# }] }] }).to_string();
    let mut s = controls_script();
    s.push(refused);
    let (recipe, _, notes, bodies) = run(AnalysisApproach::Both, s, true, true);
    assert_eq!(bodies.len(), 4, "no choice without a statement");
    assert_eq!(recipe.exposure_ev, 0.3);
    assert!(notes.iter().any(|n| n.key == keys::STATEMENT_FAILED), "{}", recipe.rationale);
    let garbage = propose_reply(json!({ "nonsense": true }));
    let (recipe, _, notes, _) = run(AnalysisApproach::Both, both_script(statement_reply(), garbage), true, true);
    assert_eq!(recipe.exposure_ev, 0.3);
    assert!(notes.iter().any(|n| n.key == keys::STATEMENT_CHOICE_FAILED), "{}", recipe.rationale);
}

fn red_of(data_url: &str) -> u8 {
    use base64::Engine;
    let b64 = data_url.split_once(',').expect("a data URL").1;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).expect("base64");
    image::load_from_memory(&bytes).expect("a JPEG").to_rgb8().get_pixel(16, 12).0[0]
}

/// The choice request carries three frames in the order reference (the
/// calibration-only develop), today's develop, the statement's — each the
/// deliverable rendered from the RAW.
///
/// MUTATION: swap the held and candidate renders in `ChoiceImages` — the red
/// tags come back 0 / 100 / 60 and this goes red.
#[test]
fn the_choice_shows_the_reference_then_todays_then_the_statements_develop() {
    let (_, _, _, bodies) = run(AnalysisApproach::Both, both_script(statement_reply(), choice("A")), true, true);
    let v: Value = serde_json::from_str(&bodies[4]).unwrap();
    let reds: Vec<u8> = v["input"][0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["type"] == "input_image")
        .map(|c| red_of(c["image_url"].as_str().unwrap()))
        .collect();
    assert_eq!(reds.len(), 3);
    assert!(reds[0] < 10 && (50..70).contains(&reds[1]) && (90..110).contains(&reds[2]), "reference, today's, statement's: {reds:?}");
}

/// `Statement` answers the statement's develop under the judge's own
/// decision, from ONE look at it.
///
/// MUTATION: return Accept for the statement whatever the judge said — the
/// verdict assertion goes red.
#[test]
fn statement_returns_the_judges_decision_as_the_verdict() {
    let script = vec![statement_reply(), judgement("revise", 64, Some("lift the ground"))];
    let (recipe, verdict, notes, bodies) = run(AnalysisApproach::Statement, script, true, true);
    assert_eq!(bodies.len(), 2, "one statement, one judgement");
    assert_eq!(recipe.exposure_ev, 0.5);
    assert_eq!(verdict.decision, Decision::Revise);
    assert_eq!(verdict.revised_hint.as_deref(), Some("lift the ground"));
    assert!(notes.iter().any(|n| n.key == keys::STATEMENT_JUDGE));
}

/// Where the statement path cannot run (here: not a RAW) `Both` is today's
/// analysis, and one note says why.
///
/// MUTATION: try the statement anyway — the script has no reply for it and
/// this goes red.
#[test]
fn where_the_statement_cannot_run_both_is_todays_analysis_with_a_note() {
    let (recipe, _, notes, bodies) = run(AnalysisApproach::Both, controls_script(), true, false);
    assert_eq!(bodies.len(), 3);
    assert_eq!(recipe.exposure_ev, 0.3);
    assert!(recipe.rationale.contains("the photo is not a camera RAW"), "{}", recipe.rationale);
    assert!(notes.iter().any(|n| n.key == keys::STATEMENT_UNAVAILABLE));
}

/// The approach's spellings round-trip, and the struct default is today's.
#[test]
fn the_approach_spellings_round_trip_and_the_default_is_controls() {
    for a in AnalysisApproach::ALL {
        assert_eq!(AnalysisApproach::parse(a.as_str()).unwrap(), a);
    }
    assert!(AnalysisApproach::parse("sliders").is_err());
    assert_eq!(GradeRequest::default().approach, AnalysisApproach::Controls);
}
