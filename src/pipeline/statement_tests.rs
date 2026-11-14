//! The statement path, end to end with fakes for the outside world, and ONE
//! run of the real reverse fit on a small synthetic pair.

use super::*;
use crate::advisor::tests::{join_stub, stub_endpoint};
use std::cell::RefCell;

/// A 96x64 frame: a blue-grey sky over a dark ground.
fn frame_image() -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(96, 64, |x, y| {
        if y < 32 {
            image::Rgb([90 + (x / 3) as u8, 110 + (y / 2) as u8, 150])
        } else {
            image::Rgb([20 + (x / 4) as u8, 18 + ((x + y) % 9) as u8, 12])
        }
    }))
}

fn sky_mask(w: u32, h: u32) -> image::GrayImage {
    image::GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]))
}

fn statement_reply(areas: &str) -> String {
    let inner = format!(r#"{{"look":"a warm dusk","guides":[1],"areas":{{{areas}}}}}"#);
    serde_json::json!({ "output": [{ "content": [{ "type": "output_text", "text": inner }] }] }).to_string()
}

const SKY_GROUND: &str = r#""sky":{"L":[50,60,70,80,90],"ab_shadows":[5,10],"ab_mids":[8,14],"ab_highlights":[10,20]},"ground":{"L":[5,10,15,20,30],"ab_shadows":[0,0],"ab_mids":[0,0],"ab_highlights":[0,0]}"#;

/// Fakes: the frame above as the plate under a default calibration, the sky
/// fake given, the fit recording the target it was handed and answering a
/// recipe with exposure 0.5 and one note of its own.
fn fakes<'a>(
    sky: SkyFn<'a>,
    seen_target: &'a RefCell<Option<image::DynamicImage>>,
) -> StatementTools<'a> {
    StatementTools {
        is_raw: Box::new(|_| true),
        source: Box::new(|_| Ok((crate::render::Plate::baked(frame_image()), EditRecipe::default()))),
        sky,
        reference: Box::new(|_| {
            let mut j = Vec::new();
            frame_image().write_to(&mut std::io::Cursor::new(&mut j), image::ImageFormat::Jpeg)?;
            Ok(j)
        }),
        fit: Box::new(move |_, _, _, target| {
            *seen_target.borrow_mut() = Some(target.clone());
            let mut recipe = EditRecipe { exposure_ev: 0.5, ..Default::default() };
            let mut notes = Vec::new();
            crate::rationale::push_note(&mut recipe.rationale, &mut notes, Note::new(keys::JUDGE_SCORE, vec![("score", "1".into()), ("critique", "fit".into())]));
            Ok(LookFitResult { recipe, notes, err_before: 0.1, err_after: 0.01 })
        }),
        render: Box::new(|_, _| Ok(frame_image())),
        dump: None,
    }
}

fn mean_l(img: &image::DynamicImage, rows: std::ops::Range<u32>) -> f64 {
    let rgb = img.to_rgb8();
    let mut s = 0.0;
    let mut n = 0.0;
    for y in rows {
        for x in 0..rgb.width() {
            s += f64::from(crate::fit::cie_lab(&rgb.get_pixel(x, y).0.map(|c| f32::from(c) / 255.0))[0]);
            n += 1.0;
        }
    }
    s / n
}

/// The path end to end: ONE statement call naming sky and ground with their
/// measured numbers, the fit handed the frame MOVED to the stated numbers (the
/// sky brighter, as stated), and the fit's recipe returned with the stated
/// look and the guide on its rationale.
///
/// MUTATION: hand the fit the neutral frame instead of the moved one — the
/// target's sky is not brighter and this goes red; drop the STATEMENT_LOOK note
/// — the rationale assertion goes red.
#[test]
fn the_statement_path_fits_the_raw_onto_the_frame_moved_to_the_stated_look() {
    let (url, seen, handle) = stub_endpoint(vec![(200, "application/json", statement_reply(SKY_GROUND))]);
    let cfg = crate::pipeline::advise_test_cfg(&url);
    let target = RefCell::new(None);
    let tools = fakes(Box::new(|f| Ok(sky_mask(f.width() / 2, f.height() / 2))), &target);
    let out = statement_recipe(Path::new("photo.arw"), &cfg, &super::super::GradeRequest::default(), None, &[], &tools, crate::diag::silent())
        .expect("the canned statement path completes");
    join_stub(handle);
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    let text = body["input"][0]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("- \"sky\" (50% of the frame)") && text.contains("- \"ground\" (50% of the frame)"), "{text}");
    let target = target.borrow().clone().expect("the fit ran");
    assert_eq!((target.width(), target.height()), (96, 64), "the target is the plate's own size");
    let (before, after) = (mean_l(&frame_image(), 0..32), mean_l(&target, 0..32));
    assert!(after > before + 5.0, "the stated sky (median L* 70) is brighter than the frame's: {before:.1} -> {after:.1}");
    assert_eq!(out.recipe.exposure_ev, 0.5, "the fit's recipe");
    assert_eq!(out.look, "a warm dusk");
    assert!(out.recipe.rationale.contains(" [Stated look: a warm dusk]"), "{}", out.recipe.rationale);
    assert!(out.notes.iter().any(|n| n.key == keys::STATEMENT_LOOK));
    assert!(out.notes.iter().any(|n| n.key == keys::JUDGE_SCORE), "the fit's own notes stay first");
    assert!(out.guides.is_empty(), "no reference was carried, so no guide survives");
}

/// Without a sky mask the frame is ONE area named "frame", and a note says why.
///
/// MUTATION: fail the whole path when the segmenter fails — this goes red on
/// the `expect`.
#[test]
fn without_a_sky_mask_the_frame_is_stated_as_one_area() {
    let reply = statement_reply(r#""frame":{"L":[5,20,40,60,85],"ab_shadows":[0,0],"ab_mids":[0,0],"ab_highlights":[0,0]}"#);
    let (url, seen, handle) = stub_endpoint(vec![(200, "application/json", reply)]);
    let cfg = crate::pipeline::advise_test_cfg(&url);
    let target = RefCell::new(None);
    let tools = fakes(Box::new(|_| Err(anyhow::anyhow!("segmenter unavailable"))), &target);
    let out = statement_recipe(Path::new("photo.arw"), &cfg, &super::super::GradeRequest::default(), None, &[], &tools, crate::diag::silent())
        .expect("the path runs on one area");
    join_stub(handle);
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    let text = body["input"][0]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("- \"frame\" (100% of the frame)") && !text.contains("\"sky\""), "{text}");
    assert!(out.recipe.rationale.contains("segmenter unavailable"), "{}", out.recipe.rationale);
    assert!(out.notes.iter().any(|n| n.key == keys::STATEMENT_NO_SKY));
}

/// The dump switch: with a directory in `StatementTools::dump` the path
/// writes the frame it measured, the sky mask at its size, the target and the
/// validated statement beside the measured numbers — and the target file is
/// exactly the frame the fit was handed.
///
/// MUTATION: write the neutral frame as `statement-target.png` (or skip the
/// dump) — the pixel comparison or the existence checks go red.
#[test]
fn the_dump_switch_writes_what_one_statement_produced() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("autoshade-statement-dump-{}-{nanos}", std::process::id()));
    let (url, _seen, handle) = stub_endpoint(vec![(200, "application/json", statement_reply(SKY_GROUND))]);
    let cfg = crate::pipeline::advise_test_cfg(&url);
    let target = RefCell::new(None);
    let mut tools = fakes(Box::new(|f| Ok(sky_mask(f.width() / 2, f.height() / 2))), &target);
    tools.dump = Some(dir.clone());
    statement_recipe(Path::new("photo.arw"), &cfg, &super::super::GradeRequest::default(), None, &[], &tools, crate::diag::silent())
        .expect("the canned statement path completes");
    join_stub(handle);
    for f in ["statement-frame.png", "statement-sky.png", "statement-target.png", "statement-reply.json"] {
        assert!(dir.join(f).is_file(), "{f} was not written");
    }
    let written = image::open(dir.join("statement-target.png")).unwrap().to_rgb8();
    assert_eq!(written, target.borrow().as_ref().expect("the fit ran").to_rgb8(), "the target the fit received");
    let sky = image::open(dir.join("statement-sky.png")).unwrap();
    assert_eq!((sky.width(), sky.height()), (96, 64), "the mask at the frame's size");
    let reply: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("statement-reply.json")).unwrap()).unwrap();
    assert_eq!(reply["look"], "a warm dusk");
    assert_eq!(reply["areas"][0]["area"], "sky");
    assert!(reply["areas"][0]["stated"]["l"].is_array() && reply["areas"][0]["measured"]["l"].is_array(), "{reply}");
    let _ = std::fs::remove_dir_all(dir);
}

fn entry(id: &str, path: &Path) -> crate::library::Entry {
    crate::library::Entry {
        id: id.to_string(),
        path: path.display().to_string(),
        stamp: crate::library::Stamp { len: 1, modified: 2 },
        image: vec![0.0; 4],
        phrases: Some("warm dusk, open shadows".into()),
        settings: None,
        settings_checked: None,
    }
}

/// With the reference-image opt-in on, each hit's picture follows the frame
/// with its measured numbers; its measurement is cached in the store per
/// entry, and the guide the model names reaches the rationale by file stem.
/// With the opt-in off, the same hits go as the phrase block only.
///
/// MUTATION: attach the pictures whatever `send_reference_image` says — the
/// opt-in-off request carries two frames and this goes red.
#[test]
fn reference_pictures_go_only_with_the_opt_in_and_their_numbers_are_cached() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let id = format!("lanetest{nanos}");
    let reference = std::env::temp_dir().join(format!("ref-{nanos}.jpg"));
    let e = entry(&id, &reference);
    let hits = [crate::library::Hit { entry: &e, scene: 0.9 }];
    for opt_in in [false, true] {
        let (url, seen, handle) = stub_endpoint(vec![(200, "application/json", statement_reply(SKY_GROUND))]);
        let cfg = crate::pipeline::advise_test_cfg(&url);
        let target = RefCell::new(None);
        let tools = fakes(Box::new(|f| Ok(sky_mask(f.width(), f.height()))), &target);
        let req = super::super::GradeRequest { send_reference_image: opt_in, style: 0.5, ..Default::default() };
        let out = statement_recipe(Path::new("photo.arw"), &cfg, &req, None, &hits, &tools, crate::diag::silent()).expect("runs");
        join_stub(handle);
        let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
        let content = body["input"][0]["content"].as_array().unwrap();
        let frames = content.iter().filter(|c| c["type"] == "input_image").count();
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("warm dusk, open shadows"), "the phrase block always may go: {text}");
        assert_eq!(out.guides, vec![1]);
        assert!(out.recipe.rationale.contains(&format!("ref-{nanos}")), "the guide by stem: {}", out.recipe.rationale);
        if opt_in {
            assert_eq!(frames, 2, "the frame, then the reference picture");
            assert!(text.contains("Finished photograph 1 (IMAGE 2):\n    \"sky\""), "{text}");
            assert!(super::super::statement_cache::load(&e).is_some(), "the measurement is cached by entry id");
        } else {
            assert_eq!(frames, 1, "no picture without the opt-in");
            assert!(!text.contains("Finished photograph"), "{text}");
        }
    }
    let _ = std::fs::remove_file(crate::store::store_root().join("look-cache").join(format!("{id}.json")));
}

/// THE real reverse fit (the production fit tool) on a 192 px synthetic pair:
/// a frame and the frame moved to a stated look. The fitted recipe's render
/// is nearer the target than the frame was.
///
/// MUTATION: hand `run_fit` the neutral frame as its target (or drop the fit
/// and return the calibration recipe) — the render is no nearer and this goes
/// red.
///
/// Ignored by default: the zoned fit with its terminal full-size re-read took
/// about 49 minutes of wall time on this 192 px pair (test profile, a shared
/// machine) — far past a unit test's budget. Run it by name with `--ignored`.
#[test]
#[ignore = "the real zoned reverse fit takes most of an hour even at 192 px; run by name with --ignored"]
fn the_real_reverse_fit_reproduces_a_stated_look_on_a_small_frame() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("autoshade-statement-fit-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("photo.png");
    image::RgbImage::from_fn(192, 128, |x, y| {
        if y < 60 {
            image::Rgb([80 + (x / 3) as u8, 100 + (y / 2) as u8, 140 + (x / 6) as u8])
        } else {
            image::Rgb([30 + (x / 4) as u8, 26 + ((x * 3 + y) % 23) as u8, 18 + (y / 8) as u8])
        }
    })
    .save(&png)
    .unwrap();
    // No segmenter here (`segment_script` is empty): the zoned fit reads
    // luminance ranges, which is its documented fallback.
    let cfg = crate::pipeline::advise_test_cfg("http://127.0.0.1:1");
    let (mut plate, mut base) = super::super::fit_source(&png).expect("a baked source plate");
    let frame = crate::render::develop_preview(&plate, &base);
    let m = measure_frame(&frame, Some(&sky_mask(frame.width(), frame.height())));
    let stated = vec![
        (AreaName::Sky, look::AreaStatement { l: [45.0, 55.0, 62.0, 68.0, 78.0], ab: [[4.0, 8.0], [8.0, 16.0], [12.0, 22.0]] }),
        (AreaName::Ground, look::AreaStatement { l: [12.0, 18.0, 22.0, 26.0, 34.0], ab: [[2.0, 2.0], [3.0, 4.0], [4.0, 6.0]] }),
    ];
    let moved = look::moved(&m.lab, &m.areas, &stated).unwrap();
    let (w, h) = (frame.width(), frame.height());
    let target = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| image::Rgb(look::rgb8_of(moved[(y * w + x) as usize]))));
    let fitted = production_fit(&cfg, &png, &mut plate, &mut base, &target, crate::diag::silent()).expect("the fit runs");
    let render = crate::render::develop_preview(&plate, &fitted.recipe);
    let de = |a: &image::DynamicImage, b: &image::DynamicImage| {
        let (a, b) = (look::lab_of_image(&a.to_rgb8()), look::lab_of_image(&b.to_rgb8()));
        a.iter().zip(&b).map(|(p, q)| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt() as f64).sum::<f64>() / a.len() as f64
    };
    let (start, end) = (de(&frame, &target), de(&render, &target));
    assert!(end < start * 0.7, "the fit's render is nearer the stated look: mean CIE76 {start:.2} -> {end:.2}");
    super::super::discard_rasters(&fitted.recipe, &EditRecipe::default());
    let _ = std::fs::remove_dir_all(dir);
}
