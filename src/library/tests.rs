use super::*;

/// A unit vector along axis `i`.
fn axis(i: usize) -> Vec<f32> {
    let mut v = vec![0.0; crate::embed::EMBED_DIM];
    v[i] = 1.0;
    v
}

/// `cos t · axis(a) + sin t · axis(b)`: unit length, and `cos t` similar to
/// `axis(a)`.
fn toward(a: usize, b: usize, t: f32) -> Vec<f32> {
    let mut v = vec![0.0; crate::embed::EMBED_DIM];
    v[a] = t.cos();
    v[b] = t.sin();
    v
}

/// A photo path the tests name but never create — per process all the same,
/// as every temp-dir name in the tree must be (`test_fixture_dirs_are_process_unique`).
fn photo(name: &str) -> String {
    std::env::temp_dir().join(format!("autoshade-library-tests-{}", std::process::id())).join(name).display().to_string()
}

fn entry(name: &str, image: Vec<f32>, phrases: Option<&str>) -> Entry {
    Entry { id: format!("id-{name}"), path: photo(name), stamp: Stamp { len: 1, modified: 1 }, image, phrases: phrases.map(str::to_string), settings: None, settings_checked: None }
}

fn library(entries: Vec<Entry>) -> Library {
    Library { version: VERSION, dir: photo(""), model: "test".into(), entries }
}

fn scratch_file(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("autoshade-library-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(FILE_NAME)
}

fn names(hits: &[Hit<'_>]) -> Vec<String> {
    hits.iter().map(|h| Path::new(&h.entry.path).file_name().unwrap().to_string_lossy().into_owned()).collect()
}

fn search_plain(lib: &Library) -> Vec<Hit<'_>> {
    lib.search(&Query { image: &axis(0), exclude: None }, 3)
}

/// Every fragment is in `text`, each named when it is not.
fn has_all(text: &str, fragments: &[&str]) {
    for f in fragments {
        assert!(text.contains(f), "missing {f:?} in: {text}");
    }
}

#[test]
fn a_library_round_trips_and_the_door_refuses_bad_files() {
    let file = scratch_file("door");
    assert!(matches!(load_from(&file), Loaded::Absent), "no file yet");

    let lib = library(vec![entry("a.jpg", axis(0), Some("warm, faded"))]);
    lib.save(&file).unwrap();
    match load_from(&file) {
        Loaded::Ready(back) => assert_eq!(back, lib),
        other => panic!("a saved library must load: {other:?}"),
    }

    // An empty build never replaces a good library.
    assert!(library(Vec::new()).save(&file).is_err());
    assert!(matches!(load_from(&file), Loaded::Ready(_)), "the good file is still there");

    // Every entry is checked at the door, on the way in and on the way out.
    let bad = [
        Library { version: VERSION + 1, ..lib.clone() },
        library(vec![entry("b.jpg", vec![0.5; crate::embed::EMBED_DIM], None)]),
        library(vec![entry("c.jpg", vec![1.0; 3], None)]),
        library(vec![entry("e.jpg", axis(0), Some(&"long ".repeat(60)))]),
    ];
    for b in &bad {
        std::fs::write(&file, serde_json::to_vec(b).unwrap()).unwrap();
        assert!(matches!(load_from(&file), Loaded::Unusable(_)), "must refuse {b:?}");
        assert!(b.save(&file).is_err(), "and must not write it either");
    }
    std::fs::write(&file, b"not json").unwrap();
    let Loaded::Unusable(why) = load_from(&file) else { panic!("garbage must be unusable") };
    assert!(why.contains("parse"), "{why}");

    // A library built while entries still carried a phrase vector loads as
    // it is; the stale field is dropped on the next save, not rebuilt.
    let mut old = serde_json::to_value(&lib).unwrap();
    old["entries"][0]["phrases_vec"] = serde_json::to_value(axis(1)).unwrap();
    std::fs::write(&file, serde_json::to_vec(&old).unwrap()).unwrap();
    assert!(matches!(load_from(&file), Loaded::Ready(back) if back == lib), "an older entry must still load");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn the_scene_alone_ranks() {
    let lib = library(vec![
        entry("far.jpg", toward(0, 2, 1.2), Some("cool, crisp")),
        entry("near.jpg", toward(0, 2, 0.1), None),
        entry("mid.jpg", toward(0, 2, 0.6), Some("warm, soft")),
    ]);
    let hits = search_plain(&lib);
    assert_eq!(names(&hits), ["near.jpg", "mid.jpg", "far.jpg"]);
    assert!(hits[0].scene > hits[1].scene);
    assert_eq!(lib.search(&Query { image: &axis(0), exclude: None }, 2).len(), 2, "k bounds the answer");
}

#[test]
fn the_photo_being_developed_is_never_its_own_reference() {
    let lib = library(vec![entry("self.jpg", axis(0), None), entry("other.jpg", toward(0, 2, 0.5), None)]);
    let me = PathBuf::from(photo("self.jpg"));
    let hits = lib.search(&Query { image: &axis(0), exclude: Some(&me) }, 3);
    assert_eq!(names(&hits), ["other.jpg"]);
}

/// Versions of one shot take one slot; a different frame just under the line
/// keeps its own. MUTATION: drop the version check (the second version comes
/// back) or compare against the query instead of the kept references (the
/// closest photo itself is dropped).
#[test]
fn versions_of_one_shot_take_one_reference_slot() {
    let lib = library(vec![
        entry("shot.jpg", toward(0, 2, 0.10), None),
        // cos 0.15 = 0.989 to shot.jpg: the same shot, re-edited.
        entry("shot_v2.jpg", toward(0, 2, 0.25), None),
        // cos(0.3)·cos(0.1) = 0.951 to shot.jpg: a different frame.
        entry("next_frame.jpg", toward(0, 3, 0.30), None),
        entry("far.jpg", toward(0, 4, 1.2), None),
    ]);
    assert_eq!(names(&search_plain(&lib)), ["shot.jpg", "next_frame.jpg", "far.jpg"]);
}

#[test]
fn the_block_numbers_the_references_and_carries_their_phrases_on_one_line_each() {
    let lib = library(vec![
        entry("a.jpg", toward(0, 2, 0.1), Some("warm, faded,\nlow contrast")),
        entry("b.jpg", toward(0, 3, 0.4), None),
    ]);
    let hits = search_plain(&lib);
    assert!(reference_block(&[], &[], StyleVoice::Ceiling).is_none(), "nothing found, no block");

    let block = reference_block(&hits, &[Some(2), None], StyleVoice::Target).unwrap();
    assert!(block.starts_with("REFERENCE LOOKS — 2 finished photo(s)"), "{block}");
    has_all(&block, &[
        "closest to this photo.",
        "Reproduce their look",
        "\n  reference 1 (IMAGE 2): warm, faded, low contrast",
        "\n  reference 2: (no phrases)",
    ]);
    assert_eq!(block.lines().count(), 3, "a phrase must not forge a line: {block}");
    assert!(!block.contains(".jpg"), "file names stay out of the paid prompt: {block}");
    assert!(block.len() <= crate::advisor::REFERENCE_BUDGET_BYTES);
    let soft = reference_block(&hits, &[], StyleVoice::Ceiling).unwrap();
    has_all(&soft, &["do not go further than they do"]);
}

#[test]
fn the_look_summary_is_the_first_reference_that_has_phrases() {
    let lib = library(vec![
        entry("a.jpg", toward(0, 2, 0.1), None),
        entry("b.jpg", toward(0, 3, 0.4), Some("cool,\thazy")),
    ]);
    let hits = search_plain(&lib);
    assert_eq!(look_summary(&hits).as_deref(), Some("cool, hazy"));
    assert_eq!(look_summary(&hits[..1]), None);
}

#[test]
fn the_style_axis_alone_picks_ceiling_or_target() {
    assert_eq!(StyleVoice::for_style(0.3), StyleVoice::Ceiling);
    assert_eq!(StyleVoice::for_style(STYLE_TARGET_MIN), StyleVoice::Target);
    assert_eq!(StyleVoice::default(), StyleVoice::Ceiling);
}

#[test]
fn a_proven_reference_carries_its_settings_on_its_own_line() {
    let mut with = entry("a.jpg", toward(0, 2, 0.1), Some("warm, faded"));
    with.settings = Some(Settings {
        raw: "a.arw".into(),
        xmp: "a.xmp".into(),
        xmp_stamp: Stamp { len: 1, modified: 1 },
        match_de: 4.0,
        line: "exposure_ev=+0.30,\ncontrast=-12.00".into(),
    });
    let lib = library(vec![with, entry("b.jpg", toward(0, 3, 0.4), Some("cool"))]);
    let block = reference_block(&search_plain(&lib), &[], StyleVoice::Target).unwrap();
    has_all(&block, &["\n  reference 1: warm, faded\n    its settings (", "): exposure_ev=+0.30, contrast=-12.00\n  reference 2: cool"]);
    assert_eq!(block.lines().count(), 4, "a settings line must not forge a line: {block}");
    assert!(!block.contains(".arw") && !block.contains(".xmp"), "paths stay out of the paid prompt: {block}");
}

#[test]
fn a_finished_photo_names_its_raw_by_prefix_but_not_a_longer_number() {
    let index: std::collections::HashMap<String, Vec<PathBuf>> = [
        ("frame0264", vec![PathBuf::from("r/frame0264.ARW")]),
        ("frame0264-hdr", vec![PathBuf::from("r/frame0264-HDR.dng")]),
        ("frame026", vec![PathBuf::from("r/frame026.ARW")]),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let names = |f: &str| settings::candidates(Path::new(f), &index).iter().map(|p| p.display().to_string()).collect::<Vec<_>>();
    assert_eq!(names("frame0264-HDR-Enhanced.jpg"), ["r/frame0264-HDR.dng", "r/frame0264.ARW"], "longest prefix first");
    assert_eq!(names("frame0264-final.jpg"), ["r/frame0264.ARW"]);
    assert!(names("frame02641.jpg").is_empty(), "a longer frame number is a different frame");
    assert!(names("other.jpg").is_empty());
}

#[test]
fn the_settings_line_lists_what_differs_from_neutral_in_recipe_names() {
    assert_eq!(settings_line(&crate::recipe::EditRecipe::default()), "");
    let r = crate::recipe::EditRecipe {
        exposure_ev: 0.3,
        contrast: -12.0,
        tone_curve: [(0, 0), (128, 150), (255, 255)].map(|(input, output)| crate::recipe::CurvePoint { input, output }).to_vec(),
        ..Default::default()
    };
    let line = settings_line(&r);
    assert_eq!(line, "exposure_ev=+0.30, contrast=-12.00, tone_curve=3 points");
    assert!(line.chars().count() <= MAX_SETTINGS_CHARS);
}

#[test]
fn the_match_reads_zero_for_the_same_picture_and_grows_with_a_shift() {
    let a = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(192, 128, |x, y| image::Rgb([(x % 256) as u8, (y * 2) as u8, 90])));
    assert!(settings::mean_de(&a, &a) < 0.01);
    let half = a.resize_exact(96, 64, image::imageops::FilterType::Triangle);
    assert!(settings::mean_de(&a, &half) < 1.0, "the size of the develop does not matter");
    let mut darker = a.to_rgb8();
    darker.pixels_mut().for_each(|p| p.0 = p.0.map(|c| c / 2));
    assert!(settings::mean_de(&a, &image::DynamicImage::ImageRgb8(darker)) > 10.0);
}
