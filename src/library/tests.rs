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

fn photo(name: &str) -> String {
    std::env::temp_dir().join("autoshade-library-tests").join(name).display().to_string()
}

fn entry(name: &str, image: Vec<f32>, phrases: Option<(&str, Vec<f32>)>) -> Entry {
    Entry {
        id: format!("id-{name}"),
        path: photo(name),
        stamp: Stamp { len: 1, modified: 1 },
        image,
        phrases: phrases.as_ref().map(|(p, _)| p.to_string()),
        phrases_vec: phrases.map(|(_, v)| v),
    }
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
    lib.search(&Query { image: &axis(0), prompt: None, exclude: None }, 3)
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

    let lib = library(vec![entry("a.jpg", axis(0), Some(("warm, faded", axis(1))))]);
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
        library(vec![entry("d.jpg", axis(0), Some(("x", vec![f32::NAN; crate::embed::EMBED_DIM])))]),
        library(vec![entry("e.jpg", axis(0), Some((&"long ".repeat(60), axis(1))))]),
    ];
    for b in &bad {
        std::fs::write(&file, serde_json::to_vec(b).unwrap()).unwrap();
        assert!(matches!(load_from(&file), Loaded::Unusable(_)), "must refuse {b:?}");
        assert!(b.save(&file).is_err(), "and must not write it either");
    }
    std::fs::write(&file, b"not json").unwrap();
    let Loaded::Unusable(why) = load_from(&file) else { panic!("garbage must be unusable") };
    assert!(why.contains("parse"), "{why}");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn without_a_prompt_the_scene_alone_ranks() {
    let lib = library(vec![
        entry("far.jpg", toward(0, 2, 1.2), Some(("cool, crisp", axis(5)))),
        entry("near.jpg", toward(0, 2, 0.1), None),
        entry("mid.jpg", toward(0, 2, 0.6), Some(("warm, soft", axis(6)))),
    ]);
    let hits = search_plain(&lib);
    assert_eq!(names(&hits), ["near.jpg", "mid.jpg", "far.jpg"]);
    assert!(hits.iter().all(|h| h.prompt.is_none()), "no prompt, no prompt score");
    assert!(hits[0].scene > hits[1].scene);
    assert_eq!(lib.search(&Query { image: &axis(0), prompt: None, exclude: None }, 2).len(), 2, "k bounds the answer");
}

#[test]
fn a_prompt_lifts_the_look_it_names_over_a_closer_scene() {
    // Two scenes nearly as close; the prompt asks for the second's look.
    let lib = library(vec![
        entry("closest.jpg", toward(0, 2, 0.30), Some(("cool, crisp", toward(5, 6, 1.4)))),
        entry("asked.jpg", toward(0, 2, 0.35), Some(("warm, soft", toward(5, 6, 0.1)))),
        entry("other.jpg", toward(0, 2, 1.3), Some(("neutral", toward(5, 6, 0.8)))),
    ]);
    assert_eq!(names(&search_plain(&lib))[0], "closest.jpg");
    let warm = axis(5);
    let asked = lib.search(&Query { image: &axis(0), prompt: Some(&warm), exclude: None }, 3);
    assert_eq!(names(&asked)[0], "asked.jpg", "the direction must be able to choose the look");
    assert!(asked[0].prompt.is_some());
}

#[test]
fn an_entry_without_phrases_neither_gains_nor_loses_on_the_prompt() {
    let lib = library(vec![
        entry("plain.jpg", toward(0, 2, 0.2), None),
        entry("wrong.jpg", toward(0, 3, 0.2), Some(("cool", toward(5, 6, 1.5)))),
        entry("right.jpg", toward(0, 4, 0.2), Some(("warm", axis(5)))),
    ]);
    let warm = axis(5);
    let hits = lib.search(&Query { image: &axis(0), prompt: Some(&warm), exclude: None }, 3);
    assert_eq!(names(&hits), ["right.jpg", "plain.jpg", "wrong.jpg"], "absent sits at the mean");
    assert!(hits[1].prompt.is_none());
}

#[test]
fn the_photo_being_developed_is_never_its_own_reference() {
    let lib = library(vec![entry("self.jpg", axis(0), None), entry("other.jpg", toward(0, 2, 0.5), None)]);
    let me = PathBuf::from(photo("self.jpg"));
    let hits = lib.search(&Query { image: &axis(0), prompt: None, exclude: Some(&me) }, 3);
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
        entry("a.jpg", toward(0, 2, 0.1), Some(("warm, faded,\nlow contrast", axis(5)))),
        entry("b.jpg", toward(0, 3, 0.4), None),
    ]);
    let hits = search_plain(&lib);
    assert!(reference_block(&[], &[], StyleVoice::Ceiling, false).is_none(), "nothing found, no block");

    let block = reference_block(&hits, &[Some(2), None], StyleVoice::Target, false).unwrap();
    assert!(block.starts_with("REFERENCE LOOKS — 2 finished photo(s)"), "{block}");
    has_all(&block, &[
        "closest to this photo.",
        "Reproduce their look",
        "\n  reference 1 (IMAGE 2): warm, faded, low contrast",
        "\n  reference 2: (no phrases)",
    ]);
    assert_eq!(block.lines().count(), 3, "a phrase must not forge a line: {block}");
    assert!(!block.contains(".jpg"), "file names stay out of the paid prompt: {block}");

    let led = reference_block(&hits, &[], StyleVoice::Background, true).unwrap();
    has_all(&led, &["closest to this photo and to the direction", "the direction overrides them"]);
    assert!(led.len() <= crate::advisor::REFERENCE_BUDGET_BYTES);
}

#[test]
fn the_look_summary_is_the_first_reference_that_has_phrases() {
    let lib = library(vec![
        entry("a.jpg", toward(0, 2, 0.1), None),
        entry("b.jpg", toward(0, 3, 0.4), Some(("cool,\thazy", axis(5)))),
    ]);
    let hits = search_plain(&lib);
    assert_eq!(look_summary(&hits).as_deref(), Some("cool, hazy"));
    assert_eq!(look_summary(&hits[..1]), None);
}

#[test]
fn only_a_real_direction_at_direct_or_brief_takes_the_lead() {
    use crate::recipe::DirectionAdherence as A;
    let cases = [
        (0.3, Some("moody"), A::default(), StyleVoice::Background),
        (0.3, Some("moody"), A::new(0.9), StyleVoice::Background),
        (0.3, Some("moody"), A::new(0.2), StyleVoice::Ceiling),
        (0.9, Some("   "), A::default(), StyleVoice::Target),
        (STYLE_TARGET_MIN, None, A::default(), StyleVoice::Target),
    ];
    for (style, direction, adherence, want) in cases {
        assert_eq!(StyleVoice::choose(style, direction, adherence), want, "{style} {direction:?} {adherence:?}");
    }
    assert_eq!(StyleVoice::default(), StyleVoice::Ceiling);
}
