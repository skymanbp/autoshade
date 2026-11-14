//! The statement request and reply, from canned text only (no network but a
//! loopback stub).

use super::*;
use crate::advisor::tests::{join_stub, stub_endpoint};

fn look(l0: f32) -> AreaLook {
    AreaLook {
        share: 0.5,
        l: [l0, l0 + 10.0, l0 + 20.0, l0 + 30.0, l0 + 40.0],
        centres: [l0 + 5.0, l0 + 20.0, l0 + 35.0],
        ab: [[1.0, -2.0], [3.0, 4.0], [-5.0, 6.0]],
    }
}

fn two_areas() -> Vec<(AreaName, AreaLook)> {
    vec![(AreaName::Sky, look(40.0)), (AreaName::Ground, look(5.0))]
}

fn ask<'a>(areas: &'a [(AreaName, AreaLook)], shown: &'a [ShownReference<'a>]) -> StatementAsk<'a> {
    StatementAsk {
        frame_jpeg: b"FRAME",
        areas,
        direction: None,
        adherence: DirectionAdherence::new(DirectionAdherence::DEFAULT),
        strength: GradeStrength::default(),
        reference_block: Some("REFERENCE LOOKS — 2 finished photo(s) ...\n  reference 1: warm dusk\n  reference 2: blue night"),
        reference_count: 2,
        shown,
    }
}

const NAMES: [AreaName; 2] = [AreaName::Sky, AreaName::Ground];

fn reply(sky_l: &str) -> String {
    format!(
        r#"{{"look":"a warm dusk","guides":[1],"areas":{{"sky":{{"L":{sky_l},"ab_shadows":[1,2],"ab_mids":[3,4],"ab_highlights":[5,6]}},"ground":{{"L":[2,8,16,24,40],"ab_shadows":[0,0],"ab_mids":[0,0],"ab_highlights":[0,0]}}}}}}"#
    )
}

/// With the reference-image opt-in OFF: the frame is the only picture, the
/// phrase block goes fenced, and no reference NUMBERS go.
///
/// MUTATION: measure and attach the references whatever the opt-in says (fill
/// `shown` regardless) — a second frame and "Finished photograph" numbers
/// appear and this goes red.
#[test]
fn without_the_opt_in_the_request_carries_no_reference_picture_or_numbers() {
    let cfg = crate::pipeline::advise_test_cfg("http://127.0.0.1:1");
    let areas = two_areas();
    let req = statement_request(&cfg, &ask(&areas, &[]));
    assert_eq!(req.frames.len(), 1, "the frame only");
    assert_eq!(req.frames[0].b64, b64(b"FRAME"));
    let t = &req.instruction;
    assert!(t.contains("\"sky\" (50% of the frame): L* at the 2/25/50/75/98% tonal anchors [40.0, 50.0, 60.0, 70.0, 80.0]"), "{t}");
    assert!(t.contains("\"ground\" (50% of the frame)"), "{t}");
    assert!(!t.contains("Finished photograph"), "{t}");
    assert!(!t.contains("IMAGES 2"), "{t}");
    let fence = t.find(crate::advisor::FENCE_STYLE_REFERENCE).expect("the phrase block is fenced");
    assert!(t[fence..].contains("reference 1: warm dusk"), "{t}");
    assert_eq!(req.schema.as_ref().unwrap().0, "look_statement");
    let schema = &req.schema.as_ref().unwrap().1;
    assert_eq!(schema["properties"]["areas"]["required"], serde_json::json!(["sky", "ground"]));
}

/// With the opt-in ON: the pictures follow the frame in order, and each
/// reference's text names its IMAGE number and its measured areas; one whose
/// areas could not be measured goes as picture and words without numbers.
///
/// MUTATION: push the reference frames before the neutral frame, or number
/// the references from IMAGE 1 — the frame order or the IMAGE labels go red.
#[test]
fn with_the_opt_in_the_pictures_follow_the_frame_in_order_with_their_numbers() {
    let cfg = crate::pipeline::advise_test_cfg("http://127.0.0.1:1");
    let areas = two_areas();
    let r1 = vec![(AreaName::Sky, look(60.0)), (AreaName::Ground, look(12.0))];
    let shown = [
        ShownReference { number: 1, areas: Some(&r1), jpeg: b"REF-ONE" },
        ShownReference { number: 2, areas: None, jpeg: b"REF-TWO" },
    ];
    let req = statement_request(&cfg, &ask(&areas, &shown));
    let frames: Vec<&str> = req.frames.iter().map(|f| f.b64.as_str()).collect();
    assert_eq!(frames, vec![b64(b"FRAME"), b64(b"REF-ONE"), b64(b"REF-TWO")]);
    assert!(req.frames.iter().all(|f| f.detail == "high"));
    let t = &req.instruction;
    assert!(t.contains("IMAGES 2-3 are their own FINISHED photographs"), "{t}");
    let one = t.find("Finished photograph 1 (IMAGE 2):").expect("reference 1 is IMAGE 2");
    let two = t.find("Finished photograph 2 (IMAGE 3):").expect("reference 2 is IMAGE 3");
    assert!(one < two);
    assert!(t[one..two].contains("\"sky\" (50% of it): L* at the 2/25/50/75/98% tonal anchors [60.0, 70.0, 80.0, 90.0, 100.0]"), "{t}");
    assert!(t[two..].starts_with("Finished photograph 2 (IMAGE 3): (its areas could not be measured"), "{t}");
    assert!(t.contains("is NOT a guide for hue or for brightness"), "the relevance rule rides with the pictures: {t}");
}

/// The photographer's typed direction rides the request under the proposer's
/// heading, and the strength dial adds its one line.
///
/// MUTATION: drop the direction from `statement_request` — the heading goes
/// missing and this goes red.
#[test]
fn the_direction_and_the_strength_ride_the_request() {
    let cfg = crate::pipeline::advise_test_cfg("http://127.0.0.1:1");
    let areas = two_areas();
    let mut a = ask(&areas, &[]);
    a.direction = Some("moodier, colder sky");
    a.strength = GradeStrength::new(0.9);
    let t = statement_request(&cfg, &a).instruction;
    assert!(t.contains("USER DIRECTION (a specific request from the photographer — follow it closely): moodier, colder sky"), "{t}");
    assert!(t.contains("STRENGTH: the photographer set this develop's strength dial to 90% of full"), "{t}");
    assert!(t.contains("BOLD"), "{t}");
}

/// A decreasing L list is repaired to non-decreasing, not refused.
///
/// MUTATION: drop the running maximum in `parse_statement` — the sky's L
/// comes back decreasing and this goes red.
#[test]
fn a_decreasing_l_list_is_repaired() {
    let s = parse_statement(&reply("[30,50,45,70,65]"), &NAMES, 2, &[]).expect("repaired, not refused");
    assert_eq!(s.areas[0], (AreaName::Sky, AreaStatement { l: [30.0, 50.0, 50.0, 70.0, 70.0], ab: [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0]] }));
    assert_eq!(s.look, "a warm dusk");
    assert_eq!(s.guides, vec![1]);
}

/// A reply that leaves out an area, or names one that was not asked for, or
/// gives the wrong count or an off-scale L*, is a typed error.
///
/// MUTATION: default a missing area to the measured numbers (or skip it) — the
/// first assertion goes red.
#[test]
fn a_reply_missing_an_area_or_off_its_scale_is_refused() {
    let missing = r#"{"look":"x","guides":[],"areas":{"sky":{"L":[1,2,3,4,5],"ab_shadows":[0,0],"ab_mids":[0,0],"ab_highlights":[0,0]}}}"#;
    let e = parse_statement(missing, &NAMES, 0, &[]).unwrap_err().to_string();
    assert!(e.contains("no numbers for the area \"ground\""), "{e}");
    let extra = reply("[1,2,3,4,5]").replace("\"ground\"", "\"subject\"");
    assert!(parse_statement(&extra, &NAMES, 0, &[]).is_err());
    assert!(parse_statement(&reply("[1,2,3,4]"), &NAMES, 0, &[]).is_err(), "four anchors");
    assert!(parse_statement(&reply("[1,2,3,4,140]"), &NAMES, 0, &[]).is_err(), "L* over 100");
    assert!(parse_statement("not json", &NAMES, 0, &[]).is_err());
}

/// a*, b* are clamped to ±100 and `guides` keeps only references the request
/// carried, each once.
///
/// MUTATION: keep every guide number — the 7 survives and this goes red.
#[test]
fn colours_are_clamped_and_guides_keep_only_shown_references() {
    let text = reply("[1,2,3,4,5]").replace("[1,2]", "[-180,240]").replace("\"guides\":[1]", "\"guides\":[2,7,2,0]");
    let s = parse_statement(&text, &NAMES, 2, &[]).unwrap();
    assert_eq!(s.areas[0].1.ab[0], [-100.0, 100.0]);
    assert_eq!(s.guides, vec![2]);
    let none = parse_statement(&reply("[1,2,3,4,5]"), &NAMES, 0, &[]).unwrap();
    assert!(none.guides.is_empty(), "no references were carried");
}

/// A reply wrapped in a markdown code fence parses.
///
/// MUTATION: parse the raw text without `strip_code_fence` — this goes red.
#[test]
fn a_fenced_reply_parses() {
    let fenced = format!("```json\n{}\n```", reply("[10,20,30,40,50]"));
    let s = parse_statement(&fenced, &NAMES, 2, &[]).expect("the fence is stripped");
    assert_eq!(s.areas.len(), 2);
}

/// The wire: ONE image-role POST carrying the strict `look_statement` schema
/// and `store:false`, its reply parsed into a statement.
///
/// MUTATION: send the request on the analysis role (or twice) — the stub's
/// script is not consumed exactly and this goes red.
#[test]
fn one_statement_call_posts_once_and_parses_the_reply() {
    let body = serde_json::json!({ "output": [{ "content": [{ "type": "output_text", "text": reply("[10,20,30,40,50]") }] }] })
        .to_string();
    let (url, seen, handle) = stub_endpoint(vec![(200, "application/json", body)]);
    let cfg = crate::pipeline::advise_test_cfg(&url);
    let areas = two_areas();
    let s = ask_statement(&cfg, &ask(&areas, &[])).expect("the statement parses");
    join_stub(handle);
    assert_eq!(s.look, "a warm dusk");
    let sent: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(sent["store"], false);
    assert_eq!(sent["text"]["format"]["name"], "look_statement");
    assert_eq!(sent["text"]["format"]["strict"], true);
}
