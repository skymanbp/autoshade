use super::*;

/// A scratch parent directory nobody else shares, for the adoption tests.
/// Never `%LOCALAPPDATA%`: these tests MOVE directories, and a test that
/// can move the real develop store is a test that can lose the user's work.
fn adoption_scratch(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "autoshade-adopt-{tag}-{}-{}",
        std::process::id(),
        next_tmp_seq()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Branch 1 of 3 — the upgrade everyone actually takes: an Autoshop store,
/// no AutoShade store, so the folder is RENAMED and every develop inside it
/// arrives intact.
///
/// The move has to be `rename`, not copy-then-delete: a develop store runs
/// to gigabytes of mask rasters and pixel masters, and a copy that dies
/// half-way leaves two divergent stores with no way to tell which is the
/// real one.
///
/// MUTATION: implement the move as a copy (or a copy-then-remove) and the
/// "the old name is gone" assertion fails.
#[test]
fn a_pre_rename_store_is_adopted_by_moving_it_whole() {
    let parent = adoption_scratch("migrate");
    let legacy = parent.join(LEGACY_STORE_DIR_NAME);
    std::fs::create_dir_all(legacy.join("develops").join("photo-0123456789abcdef")).unwrap();
    std::fs::write(legacy.join("develops/photo-0123456789abcdef/recipe.json"), b"{}").unwrap();
    std::fs::write(legacy.join(crate::config::LEGACY_SETTINGS_FILE), b"{}").unwrap();

    let (root, outcome) = adopt_pre_rename_root(&parent, true, true);

    assert_eq!(outcome, RootAdoption::Migrated);
    assert_eq!(root, parent.join(STORE_DIR_NAME), "the current name is the one in use");
    assert!(!legacy.exists(), "a MOVE leaves no second copy of the user's edits behind");
    assert_eq!(
        std::fs::read(root.join("develops/photo-0123456789abcdef/recipe.json")).unwrap(),
        b"{}",
        "a develop did not survive the move"
    );
    assert!(root.join(crate::config::LEGACY_SETTINGS_FILE).is_file());

    // Idempotent: a second launch finds nothing to adopt and says nothing.
    assert_eq!(adopt_pre_rename_root(&parent, true, true).1, RootAdoption::Nothing);

    let _ = std::fs::remove_dir_all(&parent);
}

/// Branch 2 of 3 — both folders exist. Use the current one, touch neither.
///
/// Merging two stores is a decision about which of two develops of the same
/// photo the user meant to keep, and nothing here is entitled to make it
/// silently.
///
/// MUTATION: let the adoption overwrite or merge into an existing store and
/// the "the old folder is untouched" assertion fails.
#[test]
fn two_stores_side_by_side_are_never_merged() {
    let parent = adoption_scratch("keepboth");
    let legacy = parent.join(LEGACY_STORE_DIR_NAME);
    let current = parent.join(STORE_DIR_NAME);
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(legacy.join("mark.txt"), b"old").unwrap();
    std::fs::write(current.join("mark.txt"), b"new").unwrap();

    let (root, outcome) = adopt_pre_rename_root(&parent, true, true);

    assert_eq!(outcome, RootAdoption::KeptBoth);
    assert_eq!(root, current, "the current store is the one in use");
    assert_eq!(std::fs::read(legacy.join("mark.txt")).unwrap(), b"old", "the old store moved");
    assert_eq!(std::fs::read(current.join("mark.txt")).unwrap(), b"new", "the new store moved");

    let _ = std::fs::remove_dir_all(&parent);
}

/// Branch 3 of 3 — the move could not happen. Keep using the OLD folder.
///
/// Modelled by the same state the process reaches after a failed rename:
/// the pre-rename folder is still there, the current name is not, and the
/// one attempt this process gets has been spent (`act` false). Answering
/// with the current name here would silently start a second, empty store
/// and the user's whole develop history would look deleted.
///
/// MUTATION: return the current name (or delete the old folder) on failure
/// and the "still using the folder that holds the edits" assertion fails.
#[test]
fn a_store_that_could_not_be_moved_keeps_being_used() {
    let parent = adoption_scratch("fellback");
    let legacy = parent.join(LEGACY_STORE_DIR_NAME);
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("mark.txt"), b"the user's edits").unwrap();

    let (root, outcome) = adopt_pre_rename_root(&parent, false, true);

    assert_eq!(outcome, RootAdoption::FellBack);
    assert_eq!(root, legacy, "the store that holds the edits must stay the one in use");
    assert_eq!(std::fs::read(legacy.join("mark.txt")).unwrap(), b"the user's edits");
    assert!(!parent.join(STORE_DIR_NAME).exists(), "a failed move must not mint a new store");

    let _ = std::fs::remove_dir_all(&parent);
}

/// The branch macOS takes: adoption OFF, and the folder left strictly alone.
///
/// `ADOPT_PRE_RENAME` is false there because no Mac ever ran the pre-rename
/// spelling, so an `autoshop` directory in `Library/Application Support`
/// belongs to some other program -- and on a case-insensitive volume the
/// `is_dir()` probe would match `Autoshop` just as readily. This function
/// does not merely read what it finds, it RENAMES it, so the refusal has to
/// be total: no adoption, no fallback to the old name, and above all not one
/// byte moved.
///
/// Reachable on every platform because the decision is a parameter. Before
/// that it was reachable on none: the const made this branch the only one
/// macOS could run while the three tests above described the other three,
/// which is exactly how the Mac lane went red (runs 33437618323 and before)
/// with nothing anywhere exercising the refusal.
///
/// MUTATION: drop the `!adopt` guard, or have either production call site
/// pass `true` instead of the const, and this test fails.
#[test]
fn a_build_that_does_not_adopt_leaves_the_old_folder_strictly_alone() {
    let parent = adoption_scratch("refuse");
    let legacy = parent.join(LEGACY_STORE_DIR_NAME);
    std::fs::create_dir_all(legacy.join("develops")).unwrap();
    std::fs::write(legacy.join("mark.txt"), b"someone else's data").unwrap();

    let (root, outcome) = adopt_pre_rename_root(&parent, true, false);

    assert_eq!(outcome, RootAdoption::Nothing, "a refusal has nothing to disclose");
    assert_eq!(root, parent.join(STORE_DIR_NAME), "the current name, never the old one");
    assert!(legacy.is_dir(), "the folder this build does not own must still be there");
    assert_eq!(
        std::fs::read(legacy.join("mark.txt")).unwrap(),
        b"someone else's data",
        "not one byte of a stranger's directory may move"
    );
    assert!(!root.exists(), "refusing to adopt must not mint a store either");

    // The production call sites pass the const, not `true`. Windows cannot
    // execute the macOS value, so it asserts the WIRING instead -- the same
    // reason the per-platform pin below reads its own source.
    let src = crate::source_before_tests(&crate::store::source_text()).to_string();
    assert_eq!(
        src.matches("adopt_pre_rename_root(&d, latch(), ADOPT_PRE_RENAME)").count()
            + src
                .matches("adopt_pre_rename_root(&std::env::temp_dir(), latch(), ADOPT_PRE_RENAME)")
                .count(),
        2,
        "both production call sites must pass the platform const"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// The fourth case, which is the common one: a clean machine. No folder of
/// either name, so the current name is the answer and nothing is said.
#[test]
fn a_first_run_adopts_nothing_and_says_nothing() {
    let parent = adoption_scratch("nothing");
    let (root, outcome) = adopt_pre_rename_root(&parent, true, true);
    assert_eq!(outcome, RootAdoption::Nothing);
    assert_eq!(root, parent.join(STORE_DIR_NAME));
    assert!(!root.exists(), "resolving a root must not CREATE it");
    let _ = std::fs::remove_dir_all(&parent);
}

/// An explicit `AUTOSHADE_DATA_DIR` names the store outright — adopting
/// some sibling of it would be an ambush, and every test in this binary
/// runs under exactly that override.
#[test]
fn an_explicit_store_root_is_never_second_guessed() {
    let non_test = crate::source_before_tests(&crate::store::source_text()).to_string();
    let body = non_test
        .split("pub fn store_root_with_trust()")
        .nth(1)
        .expect("store_root_with_trust is gone");
    let env_arm = body.split(".or_else(").next().unwrap();
    assert!(
        !env_arm.contains("adopt_pre_rename_root"),
        "the explicit AUTOSHADE_DATA_DIR override went through adoption"
    );
}

// MaskGeometry left the module-level imports when the raster-path walks
// moved to `LocalAdjustment::bitmap_paths_mut`; the fixtures here still
// construct geometries directly.
use crate::recipe::{LocalAdjustment, MaskGeometry};

#[test]
fn owned_raster_siblings_are_atomic_and_independently_releasable() {
    let dir = std::env::temp_dir().join(format!(
        "autoshade-owned-raster-sibling-{}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let home = OwnedRaster::scratch(dir.join("mask-zone-sky.png"));
    let first = home.claim_sibling("mask-zone-tile").unwrap();
    let second = home.claim_sibling("mask-zone-tile").unwrap();
    assert_eq!(first.path().file_name().unwrap(), "mask-zone-tile.png");
    assert_eq!(second.path().file_name().unwrap(), "mask-zone-tile-2.png");
    first.remove();
    assert!(!first.path().exists());
    assert!(second.path().exists(), "rejecting one tile removed its accepted sibling");
    assert!(home.claim_sibling("../escape").is_err());
    second.remove();
    std::fs::remove_dir(&dir).unwrap();
}

/// Every read of a file this module PERSISTS goes through the capped
/// reader. A SOURCE-SCANNING gate, in the shape of the GUI's font gate
/// (`bin/gui/tests.rs`), because "bounded reads only" is a convention no
/// type enforces — and a convention with 24 obedient call sites is exactly
/// the kind that rots without anyone noticing.
///
/// Provenance: R28 2a. FOUR bare bypasses had accumulated beside those 24
/// — `saved_quarter_turns`, both `adopting-from.txt` reads, and
/// `export_xmp_beside` (adjudication F7 and the verification agents'
/// sibling finding). Nothing in the tree would have caught the fifth.
///
/// `segment.rs` is scanned too although it has no such call today: it is
/// the module that turned `saved_quarter_turns` from a cache key into a
/// PIXEL decision (F1-B, undone by R28 Batch-3 — the staged frame now comes
/// from the recipe), so a bare read appearing there is precisely the drift
/// worth hearing about early rather than in the next review.
///
/// Two deliberate narrownesses, both in the SAFE direction:
///   * line comments are stripped, block comments are not — neither file
///     contains a `/* */` today, and a block comment quoting a bare read
///     could only ever raise a false positive, never hide a real one;
///   * `read_to_end` counts as bounded when the same line bounds it with
///     `.take(` — that is not a spelling exemption but the bounded IDIOM
///     itself, and it is how both capped readers are written.
///
/// MUTATION THIS CATCHES: put `std::fs::read_to_string(recipe_target(src))`
/// back into `saved_quarter_turns` and this test fails, naming the line.
#[test]
fn every_store_read_goes_through_the_capped_reader() {
    // Spelled as they appear in a CALL. `fs::read_dir(` is deliberately
    // absent: it enumerates a directory, it never materialises a file.
    const BARE: [&str; 3] = ["read_to_string(", "fs::read(", "read_to_end("];
    // The store's own files come through the split's source-text helper (their
    // test code cut off); segment.rs is read from disk and anchored as before.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let scanned: Vec<(String, String)> = crate::store::SOURCE_FILES
        .iter()
        .map(|(rel, text)| (rel.to_string(), crate::source_before_tests(text).to_string()))
        .chain(std::iter::once((
            "src/segment.rs".to_string(),
            std::fs::read_to_string(root.join("src/segment.rs")).expect("source readable"),
        )))
        .collect();

    let mut offenders: Vec<String> = Vec::new();
    let mut capped = 0usize;
    let mut scanned_lines = 0usize;
    for (rel, text) in scanned {
        // LF-normalised first: this repo has MIXED line endings by design
        // (per-file `.gitattributes` + `core.autocrlf`), and the CRLF
        // spelling of the anchor below is what `scripts/check_docs.py`
        // normalises for the same reason.
        let text = text.replace("\r\n", "\n");
        // Tests read fixture files by the armful and must not be scanned;
        // the anchor is asserted rather than defaulted, so renaming the
        // test module makes this gate fail loudly instead of quietly
        // widening its scope. The store's files carry no test module (cut
        // above), so only segment.rs is anchored.
        let anchor = "\n#[cfg(test)]\nmod tests {";
        let end = if rel == "src/segment.rs" {
            text.find(anchor).unwrap_or_else(|| {
                panic!("{rel}: no `#[cfg(test)] mod tests` anchor — re-anchor this gate")
            })
        } else {
            text.len()
        };
        for (i, line) in text[..end].split('\n').enumerate() {
            scanned_lines += 1;
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains("read_text_capped(") || line.contains("read_bytes_capped(") {
                capped += 1;
            }
            for pat in BARE {
                if line.contains(pat) && !(pat == "read_to_end(" && line.contains(".take(")) {
                    offenders.push(format!("{rel}:{}  {}", i + 1, line.trim()));
                }
            }
        }
    }

    // PREMISE (the font gate's notdef idiom): a scanner that reads the
    // wrong file, or whose comment stripper eats everything, also finds
    // zero offenders and would pass vacuously.
    assert!(
        scanned_lines > 4_000,
        "scanner saw only {scanned_lines} lines of source — it is broken"
    );
    assert!(
        capped >= 15,
        "scanner found only {capped} capped-reader mentions — it is broken"
    );

    assert!(
        offenders.is_empty(),
        "unbounded read(s) outside the capped reader — route through \
             read_text_capped/read_bytes_capped (or bound with `.take(`):\n  {}",
        offenders.join("\n  ")
    );
}

/// One file must never produce two develop directories.
///
/// `photo_key` is `<stem>-<hash>`, and the two halves were derived from
/// DIFFERENT strings: the hash from `std::path::absolute(src)`, the stem
/// from the raw `src`. Any spelling `absolute()` rewrites therefore split
/// the key while the hash stayed identical — visible proof that the two
/// halves disagreed rather than that they described different files.
#[test]
#[cfg(windows)]
fn one_file_spelled_two_ways_is_one_develop_key() {
    // Windows drops a trailing dot when opening, so these name one file.
    let plain = photo_key(Path::new(r"D:\photos\DSC001.NEF"));
    let dotted = photo_key(Path::new(r"D:\photos\DSC001.NEF."));
    assert_eq!(plain, dotted, "a trailing dot forked the develop directory");

    // The case fold and `..` folding that already worked must keep working.
    assert_eq!(plain, photo_key(Path::new(r"d:\PHOTOS\dsc001.nef")), "case fold");
    assert_eq!(plain, photo_key(Path::new(r"D:\photos\sub\..\DSC001.NEF")), "dot-dot fold");

    // …and genuinely different photos must still get different keys.
    assert_ne!(plain, photo_key(Path::new(r"D:\photos\DSC002.NEF")));
    assert_ne!(plain, photo_key(Path::new(r"D:\other\DSC001.NEF")));
}

/// L03: a killed version delete hides the half-version from the list and
/// resumes at the next claim — and the number is BURNED for good (the
/// 2026-08-13 tombstone contract), so a resumed sweep can never eat a
/// fresh snapshot and a recycled label can never impersonate a deleted
/// version.
#[test]
fn a_killed_version_delete_resumes_and_its_number_stays_burned() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-vdel-marker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_vdel.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    // v1 complete; v2 half-deleted: the marker landed, the recipe and
    // one frozen raster still on disk (killed before the sweep ran).
    std::fs::write(version_target(&raw, 1), b"{}").unwrap();
    std::fs::write(version_target(&raw, 2), b"{}").unwrap();
    std::fs::write(dev.join("v2.mask-sky.png"), b"raster").unwrap();
    std::fs::write(dev.join(".deleting.v2"), b"deleting v2\n").unwrap();

    assert_eq!(list_versions(&raw), vec![1], "a half-deleted version is not listed");

    let (n, claimed) = claim_version(&raw).unwrap();
    assert!(!dev.join("v2.mask-sky.png").exists(), "the claim resumed the sweep first");
    assert!(!dev.join(".deleting.v2").exists(), "the finished sweep consumed its marker");
    let reg = read_deleted_versions(&dev).unwrap();
    assert_eq!(reg.hwm, 2, "the resumed delete burned its number in the registry");
    assert!(reg.deleted.iter().any(|e| e.n == 2));
    assert_eq!(
        n, 3,
        "a deleted number is never re-issued — the claim moves past the burned v2"
    );
    assert!(claimed.exists(), "the claim file holds the fresh slot");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// A 🗑 on a version that is already gone (a stale list, or a number that
/// never was one) answers NotFound and leaves NOTHING durable: the delete
/// used to write its transaction marker BEFORE the registry probe raised
/// that NotFound, and the surviving marker made the next recovery burn the
/// number fingerprint-less — lifting the high-water mark to a version that
/// never existed, so the next claim skipped past it.
///
/// MUTATION: drop the existence probe at the top of
/// `delete_version_unlocked` and the marker survives, so the claim below
/// comes back as 8, not 1.
#[test]
fn deleting_a_version_that_does_not_exist_leaves_no_marker_and_burns_nothing() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-vdel-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_vdel_missing.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    let err = delete_version(&raw, 7).expect_err("a version that does not exist cannot be deleted");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "the stale-list NotFound is kept");
    assert!(!dev.join(".deleting.v7").exists(), "a refused delete leaves no transaction marker");
    assert!(!deleted_versions_path(&dev).exists(), "…and no registry entry");

    let (n, _) = claim_version(&raw).unwrap();
    assert_eq!(n, 1, "the refused delete burned no number");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: a killed clear completes on the next locked touch instead of
/// leaving a half-cleared develop whose surviving XMP/variants would
/// resurrect the cleared edits — and the .bak republish never undoes
/// the sweep (the pending marker is checked first).
#[test]
fn a_pending_clear_completes_on_the_next_touch() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-pending-clear-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_pending_clear.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    // The kill point: the marker landed, the recipe fell, everything
    // else survived — including a .bak recovery would republish.
    std::fs::write(dev.join("recipe.json.bak"), b"{}").unwrap();
    std::fs::write(xmp_target(&raw), b"<x:xmpmeta/>").unwrap();
    std::fs::write(variants_path(&raw), b"{}").unwrap();
    std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();
    assert!(!has_develop(&raw), "a pending clear is not a develop");

    recover_orphan_baks(&raw).unwrap();

    assert!(!recipe_target(&raw).exists());
    assert!(!dev.join("recipe.json.bak").exists(), "the .bak fell with the sweep");
    assert!(!xmp_target(&raw).exists());
    assert!(!variants_path(&raw).exists());
    assert!(dev.join("cleared.txt").exists(), "the clear finished with its stamp");
    assert!(!dev.join("clear.pending").exists(), "the transaction closed");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}


/// L04-4: the precondition tag must name what the answer was BUILT
/// from — when a Lightroom sidecar out-ranks the store, its content
/// joins the tag, and a content change re-tags even though the store
/// file never moved.
#[test]
fn the_develop_revision_folds_in_a_ranked_sidecar() {
    use std::time::{Duration, SystemTime};
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-revision-fold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_rev_fold.arw"); // never read — only its neighbours are
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);

    assert_eq!(
        develop_revision(&raw).as_deref(),
        Some("none"),
        "no store file and no sidecar tags exactly like recipe_revision"
    );

    let lr = dir.join("_rev_fold.xmp");
    let lr_v1 =
        crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() });
    std::fs::write(&lr, &lr_v1).unwrap();
    let t1 = develop_revision(&raw).unwrap();
    assert!(t1.starts_with("none+lr"), "a ranked sidecar joins the tag: {t1}");

    let lr_v2 =
        crate::xmp::recipe_to_xmp(&EditRecipe { contrast: -30.0, ..Default::default() });
    std::fs::write(&lr, &lr_v2).unwrap();
    let t2 = develop_revision(&raw).unwrap();
    assert_ne!(t1, t2, "a sidecar content change re-tags while the store never moved");

    // Store newer than the sidecar → the sidecar no longer answers, and
    // the tag is the bare store revision again.
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(
        recipe_target(&raw),
        serde_json::to_string(&EditRecipe { contrast: 10.0, ..Default::default() }).unwrap(),
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&lr)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(7200))
        .unwrap();
    let t3 = develop_revision(&raw).unwrap();
    assert!(t3.starts_with('r') && !t3.contains("+lr"), "store wins: {t3}");
    assert_eq!(t3, recipe_revision(&raw).unwrap());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L02-18: the double-crash window — a crashed publish left the .bak,
/// and the RESTORE of that .bak crashed between its create_new claim and
/// its write-through replace, leaving a zero-byte live file beside the
/// only real survivor.
#[test]
fn a_zero_byte_live_claim_yields_to_the_surviving_bak() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-zerobyte-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_zerobyte.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    std::fs::write(dev.join("recipe.json.bak"), b"{\"survivor\":true}").unwrap();
    std::fs::write(recipe_target(&raw), b"").unwrap();

    recover_orphan_baks(&raw).unwrap();

    assert_eq!(
        std::fs::read(recipe_target(&raw)).unwrap(),
        b"{\"survivor\":true}",
        "a zero-byte claim is a dead publish, not a save — the .bak must come back live"
    );
    assert!(!dev.join("recipe.json.bak").exists(), "the restored survivor consumed its .bak");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L02-6 + L02-8: a resume copies from the source the MARKER records
/// (not the current session's spelling), and while the marker fences the
/// destination its bytes have no authority — a frozen half-copy from the
/// crashed attempt is REPLACED, not kept by no-clobber.
#[test]
fn a_resumed_adoption_copies_from_the_marker_recorded_source() {
    let root = std::env::temp_dir().join(format!("autoshade-store-test-adopt-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cd = root.join("develops").join("photo-ck");
    let ld = root.join("develops").join("photo-lk");
    let old = root.join("develops").join("photo-old");
    for d in [&cd, &ld, &old] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(cd.join("adopting-from.txt"), format!("{}\n", old.display())).unwrap();
    std::fs::write(cd.join("recipe.json"), b"stale-frozen-copy").unwrap();
    // A member the SOURCE no longer has (cleared under the alias between
    // attempts) — synchronization must drop it (review R12-02).
    std::fs::write(cd.join("variants.json"), b"stale-destination-only").unwrap();
    std::fs::write(old.join("recipe.json"), b"newest-under-old").unwrap();
    std::fs::write(old.join("pixels.json"), b"pixels-under-old").unwrap();
    // The CURRENT spelling holds a decoy: a resume that copies "from the
    // current spelling" would import it and franken-merge generations.
    std::fs::write(ld.join("recipe.json"), b"decoy-current-spelling").unwrap();

    let key = adopt_or_choose(&root, Path::new("unused-abs"), "photo-ck", "photo-lk");
    assert_eq!(key.as_deref(), Some("photo-ck"));
    assert_eq!(
        std::fs::read(cd.join("recipe.json")).unwrap(),
        b"newest-under-old",
        "the resume copies from the marker's source and REPLACES the frozen half-copy"
    );
    assert_eq!(std::fs::read(cd.join("pixels.json")).unwrap(), b"pixels-under-old");
    assert!(
        !cd.join("variants.json").exists(),
        "a destination-only member is REMOVED, not resurrected (sync, not overlay)"
    );
    assert!(!cd.join("adopting-from.txt").exists(), "the finished resume consumed its marker");
    assert!(old.join("superseded-by.txt").exists(), "the true source is marked superseded");
    assert!(
        !ld.join("superseded-by.txt").exists(),
        "the current spelling was NOT the source and keeps its develop untouched"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// L02-4: a canonical-spelling session (ck == lk) finds the marker an
/// alias session's crashed adoption left behind, and finishes the copy —
/// before the fix the fast path returned with zero probes and served the
/// half-copied dir as if complete.
#[test]
fn a_canonical_session_finishes_an_alias_sessions_crashed_adoption() {
    let root = canonical_temp("orphan-resume");
    // `canonical_temp`, not `env::temp_dir()`: the ck == lk precondition
    // below is a statement about a canonically-spelled photo, and a temp
    // dir reached through a symlink (macOS `/var`) or an 8.3/off-case env
    // var breaks it before the code under test runs. See the helper.
    let dir = canonical_temp("orphan-resume-photos");
    let raw = dir.join("_orphan_resume.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let ck = photo_key(&raw);
    assert_eq!(ck, photo_key_lexical(&raw), "a plain true-cased path keys canonically");
    let cd = root.join("develops").join(&ck);
    let old = root.join("develops").join("old-spelling");
    std::fs::create_dir_all(&cd).unwrap();
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("recipe.json"), b"alias-session-bytes").unwrap();
    std::fs::write(cd.join("adopting-from.txt"), format!("{}\n", old.display())).unwrap();

    let key = resolve_key_in(&root, &raw);
    assert_eq!(key, ck);
    assert_eq!(
        std::fs::read(cd.join("recipe.json")).unwrap(),
        b"alias-session-bytes",
        "the fast path probed, found the crashed adoption and finished the copy"
    );
    assert!(!cd.join("adopting-from.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

/// L02-5: the superseded marker is probed INSIDE the develop lock — a
/// touch of a dir a concurrent adoption froze must refuse (and
/// re-resolve) rather than run its body against the frozen backup.
#[test]
fn a_develop_dir_marked_superseded_refuses_the_locked_touch() {
    let root = std::env::temp_dir().join(format!("autoshade-store-test-superseded-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-superseded-photos-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_superseded_probe.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir_in(&root, &raw);
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("superseded-by.txt"), b"elsewhere\n").unwrap();

    let mut ran = false;
    let out = with_develop_lock_in(&root, &raw, DevelopLockMode::Wait, || {
        ran = true;
        Ok::<_, std::io::Error>(())
    });
    assert!(out.is_err(), "a superseded dir must refuse the touch, not absorb the write");
    assert!(!ran, "no body may run against a frozen alias backup");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

/// R12-01: the develop LOCK is the fence's authority gate — a session
/// that probed "no marker" before a concurrent adoption started (and
/// memoized it) must still finish the adoption before its locked touch
/// runs, instead of serving the half-copied dir.
#[test]
fn a_locked_touch_finishes_a_pending_adoption_first() {
    let root = std::env::temp_dir().join(format!("autoshade-store-test-fence-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-fence-gate-photos-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_fence_gate.arw");
    std::fs::write(&raw, b"raw").unwrap();

    // The pre-adoption probe: memoizes "no marker" for this photo.
    let ck = resolve_key_in(&root, &raw);
    let cd = root.join("develops").join(&ck);
    // NOW a concurrent alias session starts an adoption and crashes
    // half-copied: marker present, member missing.
    let old = root.join("develops").join("crashed-alias");
    std::fs::create_dir_all(&cd).unwrap();
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("recipe.json"), b"the-real-develop").unwrap();
    std::fs::write(cd.join("adopting-from.txt"), format!("{}\n", old.display())).unwrap();

    let mut seen = None;
    with_develop_lock_in(&root, &raw, DevelopLockMode::Wait, || {
        seen = Some(std::fs::read(cd.join("recipe.json")).map(|b| b.to_vec()));
        Ok::<_, std::io::Error>(())
    })
    .unwrap();
    assert_eq!(
        seen.unwrap().ok().as_deref(),
        Some(b"the-real-develop".as_slice()),
        "the locked body ran only AFTER the fence was resolved"
    );
    assert!(!cd.join("adopting-from.txt").exists(), "the gate consumed the fence");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fenced dir with the marker naming `old`, and a second thread holding
/// `old`'s develop lock the way a resume in another process would — the
/// fixture both Wait-mode resume tests share.
fn fenced_dir_with_held_source(
    tag: &str,
    hold: impl FnOnce(&Path, &Path) + Send + 'static,
) -> (PathBuf, PathBuf, PathBuf, PathBuf, std::thread::JoinHandle<()>) {
    let root = std::env::temp_dir().join(format!("autoshade-store-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-{tag}-photos-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join(format!("_{tag}.arw"));
    std::fs::write(&raw, b"raw").unwrap();
    let cd = root.join("develops").join(resolve_key_in(&root, &raw));
    let old = root.join("develops").join("crashed-alias-held");
    std::fs::create_dir_all(&cd).unwrap();
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("recipe.json"), b"the-real-develop").unwrap();
    std::fs::write(cd.join("adopting-from.txt"), format!("{}\n", old.display())).unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = {
        let (old, cd) = (old.clone(), cd.clone());
        std::thread::spawn(move || {
            with_path_lock(old.join(".develop.lock"), DevelopLockMode::Wait, || {
                held_tx.send(()).unwrap();
                // Long enough for the touch under test to read the marker
                // and reach the source lock — the real resume holds it
                // for a whole directory copy.
                std::thread::sleep(std::time::Duration::from_millis(500));
                hold(&cd, &old);
                Ok::<_, std::io::Error>(())
            })
            .unwrap();
        })
    };
    held_rx.recv().expect("the holder took the source lock");
    (root, dir, raw, cd, holder)
}

/// A Wait-mode touch of a fenced dir QUEUES behind the process holding the
/// adoption's locks — the stated `DevelopLockMode::Wait` contract, the same
/// way it queues behind any other mutation — instead of failing the CLI /
/// server / worker call with the WouldBlock a NoWait resume produced.
///
/// MUTATION: hand `DevelopLockMode::NoWait` to `resume_marked_adoption` from
/// the locked-touch gate and the touch errs instead of waiting.
#[test]
fn a_wait_mode_touch_queues_behind_a_held_adoption_lock_instead_of_failing() {
    let (root, dir, raw, cd, holder) = fenced_dir_with_held_source("fence-wait", |_, _| {});
    let mut seen = None;
    with_develop_lock_in(&root, &raw, DevelopLockMode::Wait, || {
        seen = Some(std::fs::read(cd.join("recipe.json")).ok());
        Ok::<_, std::io::Error>(())
    })
    .expect("a Wait touch queues behind the held adoption lock");
    holder.join().unwrap();
    assert_eq!(
        seen.flatten().as_deref(),
        Some(b"the-real-develop".as_slice()),
        "the body ran only after the resume it waited for had finished"
    );
    assert!(!cd.join("adopting-from.txt").exists(), "the resumed adoption consumed its fence");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The resume re-reads the fence UNDER both locks: one that waited while a
/// concurrent resume finished copies NOTHING — `adopt_files` synchronizes
/// the destination to the source, so a second run over the completed
/// adoption would replace a save that landed in between with the
/// superseded source's bytes.
///
/// MUTATION: call `adopt_files` unconditionally once the locks are held
/// (drop the under-lock marker re-read) and the newer save below reads as
/// the stale source bytes.
#[test]
fn a_resume_that_lost_the_race_copies_nothing_over_the_finished_adoption() {
    let (root, dir, raw, cd, holder) = fenced_dir_with_held_source("fence-lost-race", |cd, _| {
        // The concurrent resume finishes, and a NEWER save lands in the
        // now-unfenced dir, before the source lock is released.
        std::fs::remove_file(cd.join("adopting-from.txt")).unwrap();
        std::fs::write(cd.join("recipe.json"), b"a-newer-save").unwrap();
    });
    let mut seen = None;
    with_develop_lock_in(&root, &raw, DevelopLockMode::Wait, || {
        seen = Some(std::fs::read(cd.join("recipe.json")).ok());
        Ok::<_, std::io::Error>(())
    })
    .expect("a finished adoption is an ordinary unfenced dir");
    holder.join().unwrap();
    assert_eq!(
        seen.flatten().as_deref(),
        Some(b"a-newer-save".as_slice()),
        "a resume that lost the race must not copy the superseded source over a newer save"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

/// L02-5, the healing half: a session whose MEMOIZED key points at a dir
/// a concurrent process adopted away drops the stale entry inside the
/// lock and re-resolves onto the canonical dir — instead of writing the
/// whole session's edits into the frozen backup.
#[test]
fn a_superseded_memo_entry_is_dropped_and_the_touch_lands_canonically() {
    let dir = canonical_temp("superseded-heal");
    let target = dir.join("real");
    std::fs::create_dir_all(&target).unwrap();
    let link = dir.join("alias");
    make_junction(&link, &target);
    let photo = target.join("_superseded_heal.arw");
    std::fs::write(&photo, b"raw").unwrap();
    let via_alias = link.join("_superseded_heal.arw");

    let root = store_root();
    let lk = photo_key_lexical(&via_alias);
    let ck = photo_key(&via_alias);
    assert_ne!(lk, ck, "the junction spelling must key lexically");
    let ld = root.join("develops").join(&lk);
    let cd = root.join("develops").join(&ck);
    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd);
    std::fs::create_dir_all(&ld).unwrap();
    std::fs::create_dir_all(&cd).unwrap();
    // A concurrent process adopted the lexical dir away…
    std::fs::write(ld.join("superseded-by.txt"), format!("{}\n", cd.display())).unwrap();
    // …while THIS session still holds the lexical key memoized (the
    // stable fallback of an earlier failed adoption).
    let abs = std::path::absolute(&via_alias).unwrap();
    resolved_keys().lock().unwrap().insert((root.clone(), abs.clone()), lk.clone());

    let mut saw = None;
    with_develop_lock(&via_alias, DevelopLockMode::Wait, || {
        saw = Some(develop_dir(&via_alias));
        Ok::<_, std::io::Error>(())
    })
    .unwrap();
    assert_eq!(
        saw.as_deref(),
        Some(cd.as_path()),
        "the locked touch re-resolved onto the canonical dir, not the frozen backup"
    );

    resolved_keys().lock().unwrap().remove(&(root.clone(), abs));
    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd);
    let _ = std::fs::remove_dir_all(&dir);
}

/// L02-12: the commit stage refuses any member the recovery reader could
/// not read back — committing it would work today and brick the photo on
/// the first crash recovery.
#[test]
fn a_staged_member_above_the_recovery_cap_refuses_the_save_whole() {
    let (dir, raw, dev) = commit_fixture("stage-cap");
    let big = vec![b' '; MAX_STORE_JSON as usize + 1];
    let err = commit_develop(
        &raw,
        DevelopCommit {
            recipe: Some(big),
            pixels: CommitMember::Keep,
            variants: CommitMember::Keep,
            xmp: CommitMember::Keep,
        },
    )
    .expect_err("a member the recovery reader cannot read back must not commit");
    assert!(err.to_string().contains("store limit"), "the refusal names the cap: {err}");
    assert!(!dev.join(".commit").exists(), "the refused stage was discarded whole");
    assert!(!recipe_target(&raw).exists(), "nothing was applied");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L02-15: a clear marker that cannot be consumed must FAIL the
/// resolution — reported success with the marker still on disk replays
/// the clear over whatever save lands next (the batch-S trap through the
/// failure path).
#[test]
#[cfg(windows)]
fn a_clear_marker_that_cannot_be_consumed_fails_the_resolution() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-clear-consume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_clear_consume.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();

    // An exclusive handle denies the marker's deletion (share_mode 0).
    let hold = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dev.join("clear.pending"))
        .unwrap();
    recover_orphan_baks(&raw)
        .expect_err("a marker that survives resolution must fail it, not report success");
    drop(hold);

    // With the handle gone the same touch completes and consumes it.
    recover_orphan_baks(&raw).unwrap();
    assert!(!dev.join("clear.pending").exists(), "the retried resolution consumed the marker");
    assert!(dev.join("cleared.txt").exists(), "the clear finished with its stamp");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// One temp-dir photo + develop-dir fixture for the commit tests, with a
/// non-empty master raster and helpers to build staged generations.
fn commit_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join(format!("_store_{}.arw", tag.replace('-', "_")));
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    (dir, raw, dev)
}

fn commit_sum(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

fn strip_record(kind: &str) -> VariantsRecord {
    VariantsRecord {
        extra: Default::default(),
        active_id: None,
        active_name: None,
        v: 1,
        active_kind: kind.to_string(),
        active_pos: 0,
        others: Vec::new(),
    }
}

/// R24-4: a writer that publishes a NEW active develop but authors no
/// strip must leave `variants.json`'s active half describing what it just
/// wrote. Left `Keep` (four production writers did), a CLI `match` over a
/// photo whose record said 「original」 reopened as 「▣ 原片」 holding a
/// reverse-fit. The round trip is the point: everything the record holds
/// BESIDES the kind — the card's identity, its name, the background cards
/// with their own ids/names/origins — must come back untouched.
#[test]
fn a_foreign_develop_write_restates_the_strips_active_card() {
    let (dir, raw, dev) = commit_fixture("active-restate");
    let master = dev.join("card-b.png");
    std::fs::write(&master, b"png").unwrap();
    let rec = VariantsRecord {
        extra: Default::default(),
        v: 1,
        active_kind: "original".into(),
        active_pos: 1,
        active_id: Some("card-a".into()),
        active_name: Some("我的底片".into()),
        others: vec![VariantEntry {
            extra: Default::default(),
            kind: "generated".into(),
            recipe: EditRecipe { contrast: 12.0, ..Default::default() },
            origin: Some(master.clone()),
            id: Some("card-b".into()),
            name: Some("warm".into()),
        }],
    };
    write_variants(&raw, &rec).unwrap();

    // The CLI's own commit, member for member.
    commit_develop(
        &raw,
        DevelopCommit {
            recipe: Some(b"{}".to_vec()),
            pixels: CommitMember::Clear,
            variants: variants_member(&raw, ActiveWrite::Kind("fitted")).unwrap(),
            xmp: CommitMember::Keep,
        },
    )
    .unwrap();

    let VariantsRead::Strip(back) = read_variants_checked(&raw) else {
        panic!("the strip must still read back");
    };
    assert_eq!(back.active_kind, "fitted", "the active half names what was written");
    assert_eq!(back.active_id.as_deref(), Some("card-a"), "the CARD kept its identity");
    assert_eq!(back.active_name.as_deref(), Some("我的底片"), "…and its name");
    assert_eq!(back.active_pos, 1, "…and its place in the strip");
    assert_eq!(back.others.len(), 1, "background cards are none of this write's business");
    assert_eq!(back.others[0].id.as_deref(), Some("card-b"));
    assert_eq!(back.others[0].name.as_deref(), Some("warm"));
    assert_eq!(back.others[0].origin.as_deref(), Some(master.as_path()));
    assert_eq!(back.others[0].recipe.contrast, 12.0);

    // Idempotent: the same write again now finds the record already true
    // and stages nothing (no `.bak` churn for a no-op).
    assert!(
        matches!(variants_member(&raw, ActiveWrite::Kind("fitted")).unwrap(), CommitMember::Keep),
        "a record that already says so is left alone"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// R24 round-end MED-3: the additive contract in the WRITE direction.
///
/// `ActiveWrite::Kind` is a READ-MODIFY-WRITE of someone else's file. Read
/// tolerance alone ("not `deny_unknown_fields`, so an old build ignores a
/// new field") only protects the reader: without a capture-all this arm
/// parsed a newer strip, dropped every member it did not know, and
/// published the truncated record back — the CLI `match` on a photo whose
/// strip a newer GUI wrote would silently delete that GUI's data. Costs
/// nothing today (no such field exists yet); the day one does, this test
/// is what says it survives.
#[test]
fn a_read_modify_write_of_the_strip_keeps_fields_this_build_never_heard_of() {
    let (dir, raw, dev) = commit_fixture("active-additive");
    // A record from a hypothetical FUTURE build: unknown members at BOTH
    // levels (the record and an entry), beside the ones we do know.
    std::fs::write(
        variants_path(&raw),
        br#"{
              "v": 1,
              "active_kind": "original",
              "active_pos": 0,
              "active_id": "card-a",
              "active_rating": 5,
              "active_stack": {"of": 3, "pick": 1},
              "others": [
                {"kind":"generated","recipe":{},"id":"card-b","flagged":true}
              ]
            }"#,
    )
    .unwrap();
    let CommitMember::Write(bytes) =
        variants_member(&raw, ActiveWrite::Kind("fitted")).unwrap()
    else {
        panic!("a strip whose active kind changed must be rewritten");
    };
    let back: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back["active_kind"], "fitted", "premise: the write did its own job");
    assert_eq!(back["active_id"], "card-a", "…without disturbing the known members");
    // The whole point: unknown members, both levels, byte-for-byte.
    assert_eq!(back["active_rating"], 5, "record-level unknown member survived: {back}");
    assert_eq!(back["active_stack"], serde_json::json!({"of": 3, "pick": 1}), "{back}");
    assert_eq!(back["others"][0]["flagged"], true, "entry-level unknown member: {back}");
    assert_eq!(back["others"][0]["id"], "card-b");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// The other three answers, each for its own reason (R24-4): no record to
/// keep truthful, a record this build cannot honour, and the GUI handing
/// over a whole strip (or clearing it).
#[test]
fn the_strip_member_keeps_its_hands_off_what_it_must() {
    let (dir, raw, dev) = commit_fixture("active-keep");
    // No strip record: a trivial one-card photo writes none, and minting
    // one here would publish a strip nobody has.
    assert!(matches!(
        variants_member(&raw, ActiveWrite::Kind("fitted")).unwrap(),
        CommitMember::Keep
    ));
    // A record this build cannot honour is someone's data — and a
    // non-Keep member would make `refuse_unresolved_strip` fail the whole
    // save, so a foreign sidecar must not block a write that never reads
    // it.
    std::fs::write(variants_path(&raw), b"{ not json").unwrap();
    assert!(matches!(read_variants_checked(&raw), VariantsRead::Unresolved), "premise");
    assert!(matches!(
        variants_member(&raw, ActiveWrite::Kind("fitted")).unwrap(),
        CommitMember::Keep
    ));
    // The strip's OWNER hands the record over verbatim — and `None` (the
    // strip went trivial) still clears it, exactly as Ctrl+S always has.
    let rec = strip_record("generated");
    let bytes = variants_record_bytes(&raw, &rec).unwrap();
    match variants_member(&raw, ActiveWrite::Strip(Some(&rec))).unwrap() {
        CommitMember::Write(b) => assert_eq!(b, bytes, "verbatim, not re-derived"),
        _ => panic!("an owned strip publishes"),
    }
    assert!(matches!(
        variants_member(&raw, ActiveWrite::Strip(None)).unwrap(),
        CommitMember::Clear
    ));
    // …and a writer that learned nothing about the card leaves it be.
    assert!(matches!(
        variants_member(&raw, ActiveWrite::Unknown).unwrap(),
        CommitMember::Keep
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// R24-4: one query for a photo's whole edit-state list — the strip's
/// cards in strip order (the active one back at its recorded position),
/// then the numbered snapshots with their advisory metadata, restricted
/// to numbers that are actually listed.
#[test]
fn list_edits_joins_the_strip_with_the_version_history() {
    let (dir, raw, dev) = commit_fixture("list-edits");
    write_variants(
        &raw,
        &VariantsRecord {
            extra: Default::default(),
            v: 1,
            active_kind: "fitted".into(),
            active_pos: 1,
            active_id: Some("card-fit".into()),
            active_name: Some("dusk".into()),
            others: vec![VariantEntry {
                extra: Default::default(),
                kind: "original".into(),
                recipe: EditRecipe::default(),
                origin: None,
                id: Some("original".into()),
                name: None,
            }],
        },
    )
    .unwrap();
    std::fs::write(version_target(&raw, 1), b"{}").unwrap();
    record_version_meta(
        &raw,
        &VersionMetaEntry {
            n: 1,
            name: Some("before".into()),
            from_kind: Some("original".into()),
            from_id: Some("original".into()),
            origin: Some(VERSION_ORIGIN_USER.to_string()),
        },
    )
    .unwrap();
    // A record for a number nothing lists (a kill between a delete's
    // sweep and its metadata drop) must never surface as a phantom row.
    record_version_meta(&raw, &VersionMetaEntry { n: 9, ..Default::default() }).unwrap();

    let edits = list_edits(&raw);
    assert_eq!(edits.len(), 3, "two cards + one snapshot, no phantom: {}", edits.len());
    match &edits[0].state {
        EditStateKind::Variant { kind, active, .. } => {
            assert_eq!(kind, "original");
            assert!(!active, "the base negative sits at position 0 and is not active here");
        }
        _ => panic!("cards come first, in strip order"),
    }
    assert_eq!(edits[1].id, "card-fit");
    assert_eq!(edits[1].name.as_deref(), Some("dusk"));
    match &edits[1].state {
        EditStateKind::Variant { kind, active, .. } => {
            assert_eq!(kind, "fitted");
            assert!(active, "the active card returns at its recorded position");
        }
        _ => panic!("the active card is a card"),
    }
    assert_eq!(edits[2].id, "v1");
    assert_eq!(edits[2].name.as_deref(), Some("before"));
    match &edits[2].state {
        EditStateKind::Version { n, from_id, source, .. } => {
            assert_eq!(*n, 1);
            assert_eq!(from_id.as_deref(), Some("original"), "provenance rides along");
            assert_eq!(source.as_deref(), Some(VERSION_ORIGIN_USER));
        }
        _ => panic!("snapshots come after the cards"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: the kill point IMMEDIATELY after the COMMIT marker's rename — all
/// three live files still hold the previous generation. The next locked
/// touch replays the whole staged generation and consumes the stage: a
/// marked commit is a promise, not a suggestion.
#[test]
fn a_committed_generation_replays_over_a_stale_triple() {
    let (dir, raw, dev) = commit_fixture("commit-replay");
    let m1 = dev.join("m1.png");
    let m2 = dev.join("m2.png");
    std::fs::write(&m1, b"px-one").unwrap();
    std::fs::write(&m2, b"px-two").unwrap();
    std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
    std::fs::write(
        pixel_source_path(&raw),
        pixel_source_record_bytes(&raw, &m1, false).unwrap(),
    )
    .unwrap();
    std::fs::write(
        variants_path(&raw),
        variants_record_bytes(&raw, &strip_record("fitted")).unwrap(),
    )
    .unwrap();

    let cdir = dev.join(".commit");
    std::fs::create_dir_all(&cdir).unwrap();
    let r2 = b"gen2-recipe".to_vec();
    let p2 = pixel_source_record_bytes(&raw, &m2, false).unwrap();
    let v2 = variants_record_bytes(&raw, &strip_record("generated")).unwrap();
    std::fs::write(cdir.join("recipe.json"), &r2).unwrap();
    std::fs::write(cdir.join("pixels.json"), &p2).unwrap();
    std::fs::write(cdir.join("variants.json"), &v2).unwrap();
    let manifest = serde_json::json!({
        "v": 1, "recipe": "write", "pixels": "write", "variants": "write",
        "sums": {
            "recipe.json": commit_sum(&r2),
            "pixels.json": commit_sum(&p2),
            "variants.json": commit_sum(&v2),
        },
    });
    std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

    recover_orphan_baks(&raw).unwrap();

    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), r2, "recipe rolled forward");
    assert_eq!(
        std::fs::read(dev.join("recipe.json.bak")).unwrap(),
        b"gen1-recipe",
        "the previous generation retired to its .bak"
    );
    let (master, generated) = read_pixel_source(&raw).expect("gen2 master restored");
    assert_eq!(master.file_name().unwrap().to_str().unwrap(), "m2.png");
    assert!(!generated);
    match read_variants_checked(&raw) {
        VariantsRead::Strip(rec) => assert_eq!(rec.active_kind, "generated"),
        _ => panic!("gen2 strip must be readable"),
    }
    assert!(!cdir.exists(), "the stage is consumed");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// The projection member replays like the other three: a marked stage
/// with `xmp: write` lands the staged sidecar beside its recipe, a
/// marked `clear` removes the standing one, and a marker written before
/// the member existed (no `xmp` field — an older build's crash) reads as
/// `keep`. Also the RECORD-level question the member is decided by on
/// the web and the batch paste: `recorded_pixel_source` answers for a
/// recorded generated master whether or not the PNG resolves.
#[test]
fn the_projection_replays_with_its_generation() {
    let (dir, raw, dev) = commit_fixture("commit-projection");
    std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
    std::fs::write(xmp_target(&raw), b"<x:xmpmeta>gen1</x:xmpmeta>").unwrap();
    let cdir = dev.join(".commit");
    let stage = |recipe: &[u8], xmp: Option<&[u8]>, word: &str| {
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("recipe.json"), recipe).unwrap();
        let mut sums = serde_json::Map::new();
        sums.insert("recipe.json".into(), commit_sum(recipe).into());
        if let Some(x) = xmp {
            std::fs::write(cdir.join("projection.xmp"), x).unwrap();
            sums.insert("projection.xmp".into(), commit_sum(x).into());
        }
        let mut manifest = serde_json::json!({
            "v": 1, "recipe": "write", "pixels": "keep", "variants": "keep", "sums": sums,
        });
        if !word.is_empty() {
            manifest["xmp"] = word.into();
        }
        std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    };

    stage(b"gen2-recipe", Some(b"<x:xmpmeta>gen2</x:xmpmeta>"), "write");
    recover_orphan_baks(&raw).unwrap();
    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen2-recipe");
    assert_eq!(
        std::fs::read(xmp_target(&raw)).unwrap(),
        b"<x:xmpmeta>gen2</x:xmpmeta>",
        "the projection rolled forward with its recipe"
    );
    assert!(!cdir.exists(), "the stage is consumed");

    stage(b"gen3-recipe", None, "clear");
    recover_orphan_baks(&raw).unwrap();
    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen3-recipe");
    assert!(!xmp_target(&raw).exists(), "a stale projection dies with the generation that has none");

    std::fs::write(xmp_target(&raw), b"<x:xmpmeta>standing</x:xmpmeta>").unwrap();
    stage(b"gen4-recipe", None, "");
    recover_orphan_baks(&raw).unwrap();
    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen4-recipe");
    assert_eq!(
        std::fs::read(xmp_target(&raw)).unwrap(),
        b"<x:xmpmeta>standing</x:xmpmeta>",
        "a marker from before the member existed keeps the projection"
    );

    assert!(recorded_pixel_source(&raw).is_none(), "no record at all");
    let gone = dev.join("nowhere.png");
    std::fs::write(pixel_source_path(&raw), pixel_source_record_bytes(&raw, &gone, true).unwrap())
        .unwrap();
    assert!(read_pixel_source(&raw).is_none(), "premise: the recorded master does not resolve");
    let (recorded, generated) =
        recorded_pixel_source(&raw).expect("the RECORD says generated, resolvable or not");
    assert!(generated);
    assert_eq!(recorded.file_name().and_then(|n| n.to_str()), Some("nowhere.png"));
    std::fs::write(pixel_source_path(&raw), pixel_source_record_bytes(&raw, &gone, false).unwrap())
        .unwrap();
    assert_eq!(
        recorded_pixel_source(&raw).map(|(_, g)| g),
        Some(false),
        "an in-place master is a source develop"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// The immutability rule for writers without a live strip
/// (`ActiveWrite::DevelopOnAiPixels`, 2026-09-13): a non-neutral develop
/// published over a generated master takes the active slot as an
/// "edited" card and keeps the pristine generated card beside it; a
/// neutral one is the pristine card itself; an edited card whose
/// develop is replaced over its own master stands.
#[test]
fn a_develop_over_ai_pixels_forks_the_record_and_keeps_the_pristine_card() {
    let (dir, raw, dev) = commit_fixture("commit-ai-fork");
    let master = dev.join("reimagine.png");
    std::fs::write(&master, b"png").unwrap();
    std::fs::write(pixel_source_path(&raw), pixel_source_record_bytes(&raw, &master, true).unwrap())
        .unwrap();
    let edits = EditRecipe { contrast: 7.0, ..Default::default() };
    let neutral = EditRecipe::default();
    let member = |recipe: &EditRecipe, master: &Path| {
        variants_member(&raw, ActiveWrite::DevelopOnAiPixels { recipe, master }).unwrap()
    };
    let record = |m: CommitMember| -> VariantsRecord {
        let CommitMember::Write(b) = m else { panic!("expected a written record") };
        serde_json::from_slice(&b).unwrap()
    };

    // No record: a trivial strip has nothing to keep truthful.
    assert!(matches!(member(&edits, &master), CommitMember::Keep));

    // The pristine card is active, identified and named.
    write_variants(
        &raw,
        &VariantsRecord {
            extra: Default::default(),
            v: 1,
            active_kind: "generated".into(),
            active_pos: 1,
            active_id: Some("g1".into()),
            active_name: Some("sky".into()),
            others: vec![VariantEntry {
                extra: Default::default(),
                kind: "original".into(),
                recipe: EditRecipe { exposure_ev: 0.4, ..Default::default() },
                origin: None,
                id: Some("original".into()),
                name: None,
            }],
        },
    )
    .unwrap();
    assert!(
        matches!(member(&neutral, &master), CommitMember::Keep),
        "a neutral develop over its own master is the pristine card"
    );
    let forked = record(member(&edits, &master));
    assert_eq!(forked.active_kind, "edited");
    assert_eq!(forked.active_pos, 2);
    assert_eq!(forked.active_id, None, "an edited card is born without an identity to inherit");
    assert_eq!(forked.active_name, None);
    assert_eq!(forked.others.len(), 2);
    assert_eq!(forked.others[0].kind, "original", "the negative keeps its place");
    assert_eq!(forked.others[1].kind, "generated");
    assert_eq!(forked.others[1].id.as_deref(), Some("g1"), "the pristine card keeps its identity");
    assert_eq!(forked.others[1].name.as_deref(), Some("sky"), "…and its name");
    assert!(forked.others[1].recipe.is_noop(), "…and stays neutral");
    assert_eq!(
        forked.others[1].origin.as_deref(),
        Some(Path::new("reimagine.png")),
        "…on its raster, relativized by the record writer"
    );

    // Publish the fork; the edited card now owns the slot.
    write_variants(&raw, &forked).unwrap();
    assert!(
        matches!(member(&edits, &master), CommitMember::Keep),
        "the develop inside the edited card is replaced, the card stands"
    );
    assert!(
        matches!(member(&neutral, &master), CommitMember::Keep),
        "an edited card back at neutral keeps its word"
    );

    // A NEW master under edits (a web reimagine while the edited card
    // was active): the slot keeps its card, the new raster gets its
    // pristine card.
    let master2 = dev.join("reimagine-2.png");
    std::fs::write(&master2, b"png").unwrap();
    let regenerated = record(member(&edits, &master2));
    assert_eq!(regenerated.active_kind, "edited");
    assert_eq!(regenerated.active_pos, 3);
    assert_eq!(regenerated.others.len(), 3);
    assert_eq!(regenerated.others[2].kind, "generated");
    assert_eq!(regenerated.others[2].origin.as_deref(), Some(Path::new("reimagine-2.png")));

    // A source card's slot taken by edits over a generated master (a web
    // reimagine under edits while ▣ was active): the new raster's
    // pristine card is inserted, the slot's identity is not inherited.
    write_variants(&raw, &strip_record("original")).unwrap();
    let taken = record(member(&edits, &master));
    assert_eq!(taken.active_kind, "edited");
    assert_eq!(taken.active_pos, 1);
    assert_eq!(taken.active_id, None);
    assert_eq!(taken.others.len(), 1);
    assert_eq!(taken.others[0].kind, "generated");
    assert_eq!(taken.others[0].origin.as_deref(), Some(Path::new("reimagine.png")));
    // …and neutral over that slot is the pristine card, as before.
    assert_eq!(record(member(&neutral, &master)).active_kind, "generated");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: the kill point MID-apply — recipe already published, pixels and
/// strip still stale. The replay converges the remaining members and
/// SKIPS the already-applied one, so the previous generation's retired
/// `.bak` is not destroyed by a re-retire of the new bytes.
#[test]
fn a_half_applied_commit_finishes_instead_of_tearing() {
    let (dir, raw, dev) = commit_fixture("commit-half");
    let m1 = dev.join("m1.png");
    std::fs::write(&m1, b"px-one").unwrap();
    let r2 = b"gen2-recipe".to_vec();
    // Recipe member already applied: live = staged bytes, .bak = gen1.
    std::fs::write(recipe_target(&raw), &r2).unwrap();
    std::fs::write(dev.join("recipe.json.bak"), b"gen1-recipe").unwrap();
    std::fs::write(
        pixel_source_path(&raw),
        pixel_source_record_bytes(&raw, &m1, false).unwrap(),
    )
    .unwrap();

    let cdir = dev.join(".commit");
    std::fs::create_dir_all(&cdir).unwrap();
    std::fs::write(cdir.join("recipe.json"), &r2).unwrap();
    let manifest = serde_json::json!({
        "v": 1, "recipe": "write", "pixels": "clear", "variants": "keep",
        "sums": { "recipe.json": commit_sum(&r2) },
    });
    std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

    recover_orphan_baks(&raw).unwrap();

    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), r2);
    assert_eq!(
        std::fs::read(dev.join("recipe.json.bak")).unwrap(),
        b"gen1-recipe",
        "the idempotent replay must not re-retire the new bytes over gen1's .bak"
    );
    assert!(!pixel_source_path(&raw).exists(), "the pixels clear finished");
    assert!(!cdir.exists(), "the stage is consumed");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: a stage the crash caught BEFORE the marker is no commitment at
/// all — it rolls back wholesale and the live generation stands.
#[test]
fn an_unmarked_stage_rolls_back() {
    let (dir, raw, dev) = commit_fixture("commit-unmarked");
    std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
    let cdir = dev.join(".commit");
    std::fs::create_dir_all(&cdir).unwrap();
    std::fs::write(cdir.join("recipe.json"), b"gen2-recipe").unwrap();

    recover_orphan_baks(&raw).unwrap();

    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen1-recipe");
    assert!(!cdir.exists(), "the unmarked stage is discarded");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: a COMMIT whose staged bytes fail their recorded sum is a state
/// this store cannot legally reach (members are fsynced BEFORE the marker
/// rename) — so it refuses loudly and KEEPS the evidence: rolling back
/// could freeze a half-applied generation as final, rolling forward would
/// publish bytes nobody wrote.
#[test]
fn a_corrupt_commit_refuses_loudly_and_keeps_the_evidence() {
    let (dir, raw, dev) = commit_fixture("commit-corrupt");
    std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
    let cdir = dev.join(".commit");
    std::fs::create_dir_all(&cdir).unwrap();
    std::fs::write(cdir.join("recipe.json"), b"gen2-recipe").unwrap();
    let manifest = serde_json::json!({
        "v": 1, "recipe": "write", "pixels": "keep", "variants": "keep",
        "sums": { "recipe.json": commit_sum(b"different bytes entirely") },
    });
    std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

    let err = recover_orphan_baks(&raw).expect_err("a sum mismatch must refuse");
    assert!(
        err.to_string().contains("delete that directory"),
        "the refusal names the remedy: {err}"
    );
    assert_eq!(
        std::fs::read(recipe_target(&raw)).unwrap(),
        b"gen1-recipe",
        "nothing was published from the suspect stage"
    );
    assert!(cdir.exists(), "the evidence stays for the user to inspect");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: a committed CLEAR removes the live member AND its retired `.bak`
/// — the generation-blind pair sweep must not republish (resurrect) a
/// member the committed generation deletes.
#[test]
fn a_committed_clear_takes_the_retired_bak_with_it() {
    let (dir, raw, dev) = commit_fixture("commit-clear-bak");
    let m1 = dev.join("m1.png");
    std::fs::write(&m1, b"px-one").unwrap();
    std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
    // The resurrection bait: live pixels.json already fell, its .bak
    // survives — exactly the state the pair sweep exists to republish.
    std::fs::write(
        dev.join("pixels.json.bak"),
        pixel_source_record_bytes(&raw, &m1, false).unwrap(),
    )
    .unwrap();
    let cdir = dev.join(".commit");
    std::fs::create_dir_all(&cdir).unwrap();
    let manifest = serde_json::json!({
        "v": 1, "recipe": "keep", "pixels": "clear", "variants": "keep",
        "sums": {},
    });
    std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

    recover_orphan_baks(&raw).unwrap();

    assert!(!pixel_source_path(&raw).exists(), "the cleared member stays cleared");
    assert!(!dev.join("pixels.json.bak").exists(), "no resurrection bait survives");
    assert!(!has_pixel_source(&raw));
    assert!(!cdir.exists());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: a pending explicit CLEAR is the newer intent by construction
/// (commit_develop completes one before it stages), so recovery lets the
/// clear take the crashed commit with it — never replays a develop the
/// user asked to remove.
#[test]
fn a_cleared_develop_takes_its_pending_commit_with_it() {
    let (dir, raw, dev) = commit_fixture("commit-vs-clear");
    std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
    let cdir = dev.join(".commit");
    std::fs::create_dir_all(&cdir).unwrap();
    let r2 = b"gen2-recipe".to_vec();
    std::fs::write(cdir.join("recipe.json"), &r2).unwrap();
    let manifest = serde_json::json!({
        "v": 1, "recipe": "write", "pixels": "keep", "variants": "keep",
        "sums": { "recipe.json": commit_sum(&r2) },
    });
    std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();

    recover_orphan_baks(&raw).unwrap();

    assert!(!recipe_target(&raw).exists(), "the clear won — no replay");
    assert!(!cdir.exists(), "the crashed commit died with the clear");
    assert!(dev.join("cleared.txt").exists());
    assert!(!dev.join("clear.pending").exists());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// The public saver end to end: all four files land as one generation
/// with no staging residue, a second generation writes + clears + keeps
/// per member (the projection cleared WITH the recipe that has no
/// sidecar to project — the 2026-09-13 stale-projection fix), and an
/// unresolved strip refuses the WHOLE save before anything stages (the
/// all-or-nothing face).
#[test]
fn a_develop_commit_lands_all_four_or_nothing() {
    let (dir, raw, dev) = commit_fixture("commit-public");
    let m1 = dev.join("m1.png");
    std::fs::write(&m1, b"px-one").unwrap();

    commit_develop(
        &raw,
        DevelopCommit {
            recipe: Some(b"gen1-recipe".to_vec()),
            pixels: CommitMember::Write(pixel_source_record_bytes(&raw, &m1, true).unwrap()),
            variants: CommitMember::Write(
                variants_record_bytes(&raw, &strip_record("fitted")).unwrap(),
            ),
            xmp: CommitMember::Write(b"<x:xmpmeta>gen1</x:xmpmeta>".to_vec()),
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen1-recipe");
    let (master, generated) = read_pixel_source(&raw).expect("master linked");
    assert_eq!(master.file_name().unwrap().to_str().unwrap(), "m1.png");
    assert!(generated);
    assert!(matches!(read_variants_checked(&raw), VariantsRead::Strip(_)));
    assert_eq!(
        std::fs::read(xmp_target(&raw)).unwrap(),
        b"<x:xmpmeta>gen1</x:xmpmeta>",
        "the projection landed in the recipe's generation"
    );
    assert!(!dev.join(".commit").exists(), "the stage is consumed");

    commit_develop(
        &raw,
        DevelopCommit {
            recipe: Some(b"gen2-recipe".to_vec()),
            pixels: CommitMember::Clear,
            variants: CommitMember::Keep,
            xmp: CommitMember::Clear,
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen2-recipe");
    assert!(!has_pixel_source(&raw), "cleared, retired .bak included");
    assert!(matches!(read_variants_checked(&raw), VariantsRead::Strip(_)), "Keep kept it");
    assert!(!xmp_target(&raw).exists(), "the projection cleared with its generation");

    // No `.tmp.` staging litter anywhere in the develop dir.
    for e in std::fs::read_dir(&dev).unwrap().flatten() {
        let name = e.file_name();
        assert!(
            !name.to_string_lossy().contains(".tmp."),
            "staging residue: {name:?}"
        );
    }

    // The all-or-nothing face: an unresolved strip refuses the save
    // BEFORE anything stages — the recipe member does not land either.
    std::fs::write(variants_path(&raw), b"not json at all").unwrap();
    let err = commit_develop(
        &raw,
        DevelopCommit {
            recipe: Some(b"gen3-recipe".to_vec()),
            pixels: CommitMember::Keep,
            variants: CommitMember::Clear,
            xmp: CommitMember::Write(b"<x:xmpmeta>gen3</x:xmpmeta>".to_vec()),
        },
    )
    .expect_err("an unresolved strip refuses the whole save");
    assert!(!xmp_target(&raw).exists(), "all-or-nothing: the projection did not land either");
    assert!(err.to_string().contains("cannot be honoured"), "{err}");
    assert_eq!(
        std::fs::read(recipe_target(&raw)).unwrap(),
        b"gen2-recipe",
        "all-or-nothing: the refused save left no member behind"
    );
    assert!(!dev.join(".commit").exists(), "nothing staged");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: between the marker and the completed sweep, a projection copied
/// beside the RAW must not out-rank the clear and resurrect what is
/// being removed — the pending marker ranks like the cleared stamp.
#[test]
fn a_pending_clear_outranks_the_sidecar_beside_the_photo() {
    use std::time::{Duration, SystemTime};
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-pending-rank-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_pending_rank.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    let lr = raw.with_extension("xmp");
    std::fs::write(
        &lr,
        crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() }),
    )
    .unwrap();
    std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();
    // The sidecar is OLDER than the pending clear → the clear wins.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&lr)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(7200))
        .unwrap();
    assert!(
        matches!(lightroom_sidecar(&raw), LrSidecar::None),
        "a pending clear must outrank the older sidecar"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L03: a 0-byte file at the recorded master path is the crash-
/// between-claim-and-publish state — the reader refuses it with the
/// cause while the record itself still counts (callers refuse
/// deliverables instead of silently rendering the un-retouched source).
#[test]
fn an_empty_master_claim_is_refused_not_restored() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-empty-master-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_empty_master.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    let master = dev.join("retouch-master.png");
    std::fs::write(&master, b"").unwrap();
    write_pixel_source(&raw, &master, false).unwrap();

    assert!(read_pixel_source(&raw).is_none(), "an empty claim is not a master");
    assert!(
        has_pixel_source(&raw),
        "the record still exists — deliverable callers refuse, not degrade"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L01: the revision tag names the BYTES. Absence is a real tag; a
/// byte-identical republish keeps its tag (it must never 412); different
/// bytes change it. (The untaggable arm — an existing file the bounded
/// reader refuses — is a two-line error map covered by
/// read_text_capped_enforces_its_limit + the serve-side gate test.)
#[test]
fn a_recipe_revision_names_the_bytes_and_absence_is_a_real_tag() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-revision-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("recipe.json");

    assert_eq!(revision_of(&p).as_deref(), Some("none"), "absence is a real tag");
    std::fs::write(&p, b"{\"contrast\":1.0}").unwrap();
    let a = revision_of(&p).expect("readable bytes must tag");
    assert_ne!(a, "none");
    std::fs::write(&p, b"{\"contrast\":1.0}").unwrap();
    assert_eq!(revision_of(&p).as_deref(), Some(a.as_str()), "same bytes, same tag");
    std::fs::write(&p, b"{\"contrast\":2.0}").unwrap();
    assert_ne!(revision_of(&p).expect("readable"), a, "changed bytes change the tag");

    let _ = std::fs::remove_dir_all(&dir);
}

/// L01: the snapshot's .bak recovery PRECEDES recipe selection, and
/// recipe + pixel source come from one lock acquisition. A crashed
/// publish leaves the recipe only in recipe.json.bak — the old batch
/// order (exists → read, recovery buried in the later pixel read)
/// exported a neutral develop while opening the same photo showed the
/// recovered one.
#[test]
fn a_develop_snapshot_recovers_the_bak_before_choosing_a_recipe() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-snapshot-bak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_snapshot_probe.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    // A crashed publish: the recipe lives ONLY in the .bak.
    let saved =
        serde_json::to_string(&EditRecipe { contrast: 17.0, ..Default::default() })
            .unwrap();
    std::fs::write(dev.join("recipe.json.bak"), &saved).unwrap();

    let snap = read_develop_snapshot(&raw).unwrap();
    let (text, from) = snap.recipe.expect("the .bak must be recovered, not skipped");
    assert_eq!(text, saved, "the recovered recipe is the crashed publish's bytes");
    assert_eq!(from, recipe_target(&raw), "recovered into the central slot");
    assert!(snap.recipe_err.is_none(), "a recovered read is not an error");
    assert!(!snap.pixel_recorded, "no pixel link exists in this fixture");
    assert!(snap.pixel_source.is_none());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

#[test]
fn lightroom_sidecar_newest_intent_wins() {
    use std::time::{Duration, SystemTime};
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-lr-sidecar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_lr_probe.arw"); // never read — only its neighbours are
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);

    // No sidecar at all → None.
    assert!(matches!(lightroom_sidecar(&raw), LrSidecar::None), "no file");

    // A sidecar and NO stored develop → the sidecar IS the develop.
    let lr = dir.join("_store_lr_probe.xmp");
    let lr_v1 =
        crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() });
    std::fs::write(&lr, &lr_v1).unwrap();
    assert!(matches!(lightroom_sidecar(&raw), LrSidecar::Only(_)), "only");

    // Our own projection copied beside the RAW is NOT a Lightroom edit.
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(xmp_target(&raw), &lr_v1).unwrap();
    assert!(matches!(lightroom_sidecar(&raw), LrSidecar::None), "our own copy");

    // A DIFFERENT sidecar, OLDER than the store → the store wins. Times
    // are SET, not slept for — deterministic on any filesystem.
    std::fs::write(
        recipe_target(&raw),
        serde_json::to_string(&EditRecipe::default()).unwrap(),
    )
    .unwrap();
    let lr_v2 =
        crate::xmp::recipe_to_xmp(&EditRecipe { contrast: -30.0, ..Default::default() });
    std::fs::write(&lr, &lr_v2).unwrap();
    let set = |p: &Path, t: SystemTime| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    };
    let now = SystemTime::now();
    set(&lr, now - Duration::from_secs(7200));
    assert!(
        matches!(lightroom_sidecar(&raw), LrSidecar::OlderThanStore),
        "the store is newer — AutoShade work outranks the older Lightroom pass"
    );

    // …and NEWER than the store → Lightroom's edit is the newest intent.
    set(&lr, now + Duration::from_secs(7200));
    let LrSidecar::NewerThanStore(text) = lightroom_sidecar(&raw) else {
        panic!("a newer Lightroom sidecar must win the restore");
    };
    assert_eq!(text, lr_v2);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L08: an unreadable sidecar beside the RAW is DISCLOSED, not folded
/// into "no sidecar" — the old fold opened the photo neutral in silence.
#[test]
fn a_sidecar_swapped_after_the_read_is_disclosed_not_ranked() {
    // L01: the newest-intent contest may only rank the text it actually
    // read. An identity that cannot match what is on disk models a
    // sidecar swapped right after the read.
    let dir = std::env::temp_dir().join(format!(
        "autoshade-sidecar-swap-{}-{}",
        std::process::id(),
        next_tmp_seq()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("SWAP.ARW");
    std::fs::write(&raw, b"raw").unwrap();
    let lr = raw.with_extension("xmp");
    std::fs::write(&lr, b"<x:xmpmeta/>").unwrap();

    let swapped = rank_lightroom_sidecar(
        &raw,
        &lr,
        "<x:xmpmeta/>".to_string(),
        // No real file carries the epoch mtime with a u64::MAX length.
        Some((std::time::SystemTime::UNIX_EPOCH, u64::MAX)),
    );
    assert!(
        matches!(swapped, LrSidecar::Unreadable(why) if why.contains("replaced")),
        "an impossible identity must disclose the swap"
    );

    let real = std::fs::File::open(&lr)
        .ok()
        .and_then(|f| f.metadata().ok())
        .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
    assert!(real.is_some(), "the fixture filesystem must report mtimes");
    let ranked = rank_lightroom_sidecar(&raw, &lr, "<x:xmpmeta/>".to_string(), real);
    assert!(
        matches!(ranked, LrSidecar::Only(_)),
        "the true identity must rank exactly as before this guard"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lightroom_sidecar_unreadable_is_disclosed_not_absent() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-lr-unreadable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_probe.arw"); // never read — only its neighbour is
    std::fs::write(dir.join("_probe.xmp"), [0xFFu8, 0xFE, 0xC0, 0x00]).unwrap();
    let LrSidecar::Unreadable(why) = lightroom_sidecar(&raw) else {
        panic!("an unreadable sidecar must be disclosed, not treated as absent");
    };
    assert!(why.contains("UTF-8"), "the reason names the cause: {why}");
}

/// R22-8 / SF8-A: the hand-off to Lightroom. The projection lands beside the
/// photo with its bytes intact; a sidecar ALREADY there (Lightroom's own) is
/// refused rather than silently replaced; and the explicit second attempt
/// replaces it. Every arm is a separate failure mode — the middle one is the
/// whole point of the feature having a confirmation at all.
#[test]
fn the_lightroom_hand_off_lands_beside_the_photo_and_never_clobbers_in_silence() {
    let dir = std::env::temp_dir()
        .join(format!("autoshade-store-test-xmp-beside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_r22_beside.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);

    // Nothing saved yet: the refusal is NotFound and says what to do.
    let err = export_xmp_beside(&raw, false).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    assert!(
        err.to_string().contains("save the develop first"),
        "the workflow remedy travels with the refusal: {err}"
    );
    assert!(
        !raw.with_extension("xmp").exists(),
        "a refusal writes NOTHING beside the photo"
    );

    // A stored projection → it lands beside the photo, byte for byte.
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(xmp_target(&raw), b"<x:xmpmeta>ours</x:xmpmeta>").unwrap();
    let to = export_xmp_beside(&raw, false).unwrap();
    assert_eq!(to, raw.with_extension("xmp"), "the name Lightroom looks for");
    assert_eq!(std::fs::read(&to).unwrap(), b"<x:xmpmeta>ours</x:xmpmeta>");

    // Someone else's sidecar is now in the way (this is also what a SECOND
    // click sees). Refused, and their bytes survive untouched.
    std::fs::write(&to, b"<x:xmpmeta>LIGHTROOM</x:xmpmeta>").unwrap();
    let err = export_xmp_beside(&raw, false).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read(&to).unwrap(),
        b"<x:xmpmeta>LIGHTROOM</x:xmpmeta>",
        "a refused hand-off leaves the existing sidecar EXACTLY as it was"
    );

    // …and the explicit overwrite does replace it.
    export_xmp_beside(&raw, true).unwrap();
    assert_eq!(std::fs::read(&to).unwrap(), b"<x:xmpmeta>ours</x:xmpmeta>");

    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&dir);
}

/// L02: the bounded reader — over the cap is InvalidData naming the
/// limit, at the cap passes, NotFound passes through untouched.
#[test]
fn read_text_capped_enforces_its_limit() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-capped-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("capped.json");
    std::fs::write(&p, b"12345678").unwrap();
    assert_eq!(read_text_capped(&p, 8).unwrap(), "12345678");
    let err = read_text_capped(&p, 7).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("limit"), "{err}");
    let missing = read_text_capped(&dir.join("absent.json"), 8).unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    std::fs::write(&p, [0xFFu8, 0xFE]).unwrap();
    assert_eq!(
        read_text_capped(&p, 8).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn detach_rasters_frees_a_loaded_version_from_its_snapshot() {
    let base = std::env::temp_dir().join(format!("autoshade-store-test-detach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_DETACH.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    std::fs::create_dir_all(&dev).unwrap();
    // A version-frozen raster, exactly as save_version writes it.
    let frozen = dev.join("v3.mask-sky.png");
    std::fs::write(&frozen, b"raster bytes").unwrap();
    let mut r = EditRecipe::default();
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: frozen.to_string_lossy().into_owned() },
        ..Default::default()
    });
    detach_rasters(&photo, &mut r, "mask-restored");
    let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
    assert_ne!(Path::new(path), frozen, "must no longer point at the snapshot's file");
    assert!(Path::new(path).exists(), "the copy must exist");
    assert_eq!(std::fs::read(path).unwrap(), b"raster bytes", "content preserved");
    // Deleting the version's raster now leaves the live mask intact.
    std::fs::remove_file(&frozen).unwrap();
    assert!(Path::new(path).exists(), "live mask survives the version delete");
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

/// The gallery import (`migrate_legacy_from_many`) runs the crashed-publish
/// recovery FIRST, like the per-photo path: a `recipe.json.bak` whose live
/// file never landed is the NEWEST save, and the no-clobber legacy publish
/// used to fill the empty slot with the OLDER ./out recipe — after which
/// the recovery saw a live file and left the survivor retired for good.
///
/// MUTATION: drop the `recover_orphan_baks_unlocked` call from the import
/// closure and the legacy bytes become the develop.
#[test]
fn a_gallery_import_restores_the_bak_survivor_before_publishing_legacy_bytes() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-import-bak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let legacy_out = dir.join("out");
    std::fs::create_dir_all(&legacy_out).unwrap();
    let raw = dir.join("_import_bak.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    // The crashed-publish window: the newer save survives only as .bak…
    let survivor = EditRecipe { exposure_ev: 0.9, ..Default::default() };
    std::fs::write(dev.join("recipe.json.bak"), serde_json::to_string_pretty(&survivor).unwrap())
        .unwrap();
    // …while an OLDER pre-store sidecar waits in the legacy folder.
    let older = EditRecipe { exposure_ev: 0.1, ..Default::default() };
    std::fs::write(
        legacy_out.join(format!("{}.recipe.json", crate::pipeline::stem(&raw))),
        serde_json::to_string_pretty(&older).unwrap(),
    )
    .unwrap();

    migrate_legacy_from_many(&legacy_out, std::slice::from_ref(&raw));

    let live: EditRecipe =
        serde_json::from_str(&std::fs::read_to_string(recipe_target(&raw)).unwrap()).unwrap();
    assert!(
        (live.exposure_ev - 0.9).abs() < 1e-6,
        "the .bak survivor is the develop, not the older legacy bytes: {}",
        live.exposure_ev
    );
    assert!(!dev.join("recipe.json.bak").exists(), "the restored survivor consumed its .bak");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// The gallery import honours the explicit-clear tombstone the per-photo
/// path honours: a cleared develop stays cleared. MUTATION: drop the
/// `legacy_suppressed_in` check from the import closure.
#[test]
fn a_gallery_import_leaves_a_cleared_develop_alone() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-import-tomb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let legacy_out = dir.join("out");
    std::fs::create_dir_all(&legacy_out).unwrap();
    let raw = dir.join("_import_tomb.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    let legacy = legacy_out.join(format!("{}.recipe.json", crate::pipeline::stem(&raw)));
    std::fs::write(&legacy, serde_json::to_string_pretty(&EditRecipe { exposure_ev: 0.1, ..Default::default() }).unwrap()).unwrap();
    suppress_legacy_in(&store_root(), &raw).unwrap();

    assert_eq!(migrate_legacy_from_many(&legacy_out, std::slice::from_ref(&raw)), 0);
    assert!(!recipe_target(&raw).exists(), "a cleared develop is not re-imported");
    assert!(legacy.exists(), "…and the legacy file is left where it was");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// A pristine card with no pixel record to mint its ghost from hands its
/// identity and name to the fresh master card instead of losing them —
/// the version snapshots taken from it (R24-2) keep something to point
/// at. MUTATION: put `id: None, name: None` back on the fresh card.
#[test]
fn a_pristine_card_without_a_pixel_record_hands_its_identity_to_the_fresh_master_card() {
    let (dir, raw, dev) = commit_fixture("commit-ai-fork-no-record");
    let master = dev.join("reimagine.png");
    std::fs::write(&master, b"png").unwrap();
    // No pixels.json: the record-level answer is "no master".
    write_variants(
        &raw,
        &VariantsRecord {
            extra: Default::default(),
            v: 1,
            active_kind: "generated".into(),
            active_pos: 0,
            active_id: Some("g1".into()),
            active_name: Some("sky".into()),
            others: Vec::new(),
        },
    )
    .unwrap();
    let edits = EditRecipe { contrast: 7.0, ..Default::default() };
    let CommitMember::Write(bytes) =
        variants_member(&raw, ActiveWrite::DevelopOnAiPixels { recipe: &edits, master: &master }).unwrap()
    else {
        panic!("a non-neutral develop over a pristine card writes the record")
    };
    let forked: VariantsRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(forked.active_kind, "edited");
    assert_eq!(forked.active_id, None, "the edited card is still born without an identity");
    assert_eq!(forked.others.len(), 1, "one fresh master card, no ghost");
    assert_eq!(forked.others[0].kind, "generated");
    assert_eq!(forked.others[0].id.as_deref(), Some("g1"), "the identity moves onto the master card");
    assert_eq!(forked.others[0].name.as_deref(), Some("sky"), "…with its name");
    assert_eq!(forked.others[0].origin.as_deref(), Some(Path::new("reimagine.png")));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

#[test]
fn orphan_bak_is_restored_on_read() {
    // The crashed-publish window: recipe.json gone, recipe.json.bak holds
    // the develop. A reader must see the develop, not "unedited".
    let base = std::env::temp_dir().join(format!("autoshade-store-test-orphanbak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let photo = base.join("DSC_ORPHAN.ARW");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(&photo, b"not a real raw").unwrap();
    // develop_dir is keyed by absolute path; build it through the API.
    let dev = develop_dir(&photo);
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("recipe.json.bak"), b"{\"exposure\":1.0}").unwrap();
    assert!(!recipe_target(&photo).exists(), "precondition: live file gone");
    recover_orphan_baks(&photo).expect("a restorable orphan must report success");
    assert!(recipe_target(&photo).exists(), "the survivor must be restored");
    assert!(!dev.join("recipe.json.bak").exists(), "and consumed");
    // A live file always wins: a second .bak must NOT clobber it.
    std::fs::write(dev.join("recipe.json.bak"), b"{\"exposure\":9.0}").unwrap();
    recover_orphan_baks(&photo).expect("a live file present is not a failure");
    assert!(
        dev.join("recipe.json.bak").exists(),
        "adoption keeps the retired .bak — live + .bak is the normal post-publish state"
    );
    // A RECORDED master is visible even when unusable (the predicate the
    // GUI warning needs — read_pixel_source cannot answer this).
    assert!(!has_pixel_source(&photo), "no pixels sidecar yet");
    std::fs::write(dev.join("pixels.json"), b"{ not json").unwrap();
    assert!(has_pixel_source(&photo), "a corrupt sidecar still means RECORDED");
    assert!(read_pixel_source(&photo).is_none(), "...while the reader honestly fails");
    let live = std::fs::read_to_string(recipe_target(&photo)).unwrap();
    assert!(live.contains("1.0"), "live file must win: {live}");
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_cleared_develop_outranks_the_stale_copied_projection() {
    use std::time::{Duration, SystemTime};
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-cleared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_cleared_probe.arw");
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    // The user's stale copied projection beside the RAW; the store holds
    // NOTHING (the clear deleted it) — this used to read as a foreign
    // Lightroom edit and resurrect the cleared develop.
    let lr = dir.join("_store_cleared_probe.xmp");
    std::fs::write(
        &lr,
        crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() }),
    )
    .unwrap();
    mark_develop_cleared(&raw).unwrap();
    let set = |p: &Path, t: SystemTime| {
        std::fs::OpenOptions::new().write(true).open(p).unwrap().set_modified(t).unwrap();
    };
    let now = SystemTime::now();
    set(&lr, now - Duration::from_secs(3600)); // sidecar predates the clear
    assert!(
        matches!(lightroom_sidecar(&raw), LrSidecar::None),
        "a sidecar older than the clear must NOT resurrect the develop"
    );
    // A Lightroom edit made AFTER the clear is newest intent — it wins.
    set(&lr, now + Duration::from_secs(3600));
    assert!(
        matches!(lightroom_sidecar(&raw), LrSidecar::Only(_)),
        "a post-clear Lightroom edit still restores"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

#[test]
fn a_live_file_survives_recovery_and_keeps_its_retired_bak() {
    // What this pins: recovery must not touch a develop that already has
    // a live file, and the retired `.bak` beside it stays (live + .bak is
    // the normal post-publish state, not an orphan to consume).
    //
    // What it does NOT pin, stated plainly because the earlier version of
    // this comment claimed otherwise: it never reaches
    // `publish_no_clobber`, because recovery skips the row as soon as the
    // live file exists. The window the no-clobber publish exists for —
    // another process landing the live file BETWEEN that check and the
    // publish — cannot be staged single-threaded. The primitive itself is
    // covered directly by `publish_no_clobber_never_replaces_an_owner`
    // and `publish_no_clobber_lands_on_a_fresh_destination`.
    let base = std::env::temp_dir().join(format!("autoshade-store-test-noclobber-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_NOCLOBBER.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    // The crashed publish's survivor AND a concurrent save's live file.
    std::fs::write(dev.join("recipe.json.bak"), b"{\"contrast\":11.0}").unwrap();
    std::fs::write(recipe_target(&photo), b"{\"contrast\":22.0}").unwrap();
    recover_orphan_baks(&photo).expect("a live file present is not a failure");
    let live = std::fs::read_to_string(recipe_target(&photo)).unwrap();
    assert!(
        live.contains("22.0"),
        "the CONCURRENT save owns the live file — the .bak must not replace it: {live}"
    );
    assert!(
        dev.join("recipe.json.bak").exists(),
        "and the retired .bak stays put; live + .bak is the normal state"
    );
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn clear_develop_leaves_no_resurrection_route() {
    // The whole contract of an explicit clear, in the one place both
    // surfaces now go through: every home gone, the retired master gone
    // with it (a `.bak` the next open would have republished), and the
    // newest-intent marker stamped.
    let base = std::env::temp_dir().join(format!("autoshade-store-test-cleardev-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_CLEARDEV.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    let master_a = dev.join("master-a.png");
    let master_b = dev.join("master-b.png");
    std::fs::write(&master_a, b"A").unwrap();
    std::fs::write(&master_b, b"B").unwrap();
    write_pixel_source(&photo, &master_a, false).unwrap();
    write_pixel_source(&photo, &master_b, false).unwrap();
    // EVERY removal target the primitive owns — including the retired
    // recipe.json.bak, whose own resurrection route this gate previously
    // asserted about without ever creating one.
    std::fs::write(recipe_target(&photo), b"{}").unwrap();
    std::fs::write(dev.join("recipe.json.bak"), b"{\"contrast\":30.0}").unwrap();
    std::fs::write(xmp_target(&photo), b"<x:xmpmeta/>").unwrap();
    // The strip record is a removal target too — live AND retired: a
    // surviving variants.json(.bak) resurrects background variants over
    // a develop the user explicitly cleared.
    std::fs::write(variants_path(&photo), b"{\"v\":1}").unwrap();
    std::fs::write(dev.join("variants.json.bak"), b"{\"v\":1}").unwrap();
    let _ = std::fs::create_dir_all("out");
    std::fs::write(legacy_recipe(&photo), b"{}").unwrap();
    std::fs::write(legacy_xmp(&photo), b"<x:xmpmeta/>").unwrap();
    assert!(dev.join("pixels.json.bak").exists(), "precondition: a retired master exists");
    assert!(has_develop(&photo), "precondition: a develop exists");
    let out = clear_develop(&photo).expect("the clear must succeed");
    assert!(out.removed, "files really went away");
    assert!(out.marker_warning.is_none(), "the marker was stamped");
    assert!(!has_develop(&photo), "no sidecar survives");
    assert!(!has_pixel_source(&photo), "no master survives — the .bak included");
    for p in [xmp_target(&photo), legacy_recipe(&photo), legacy_xmp(&photo)] {
        assert!(!p.exists(), "every home is cleared, including {}", p.display());
    }
    assert!(
        !dev.join("recipe.json.bak").exists(),
        "the retired recipe goes too — recover_orphan_baks would republish it"
    );
    assert!(dev.join("cleared.txt").exists(), "the newest-intent marker is stamped");
    // The resurrection probe itself: every reader recovers orphans first.
    recover_orphan_baks(&photo).unwrap();
    assert!(read_pixel_source(&photo).is_none(), "nothing resurrects the cleared retouch");
    // Non-vacuous now: a recipe.json.bak DID exist before the clear, so
    // this really exercises the recovery's recipe row.
    assert!(!has_develop(&photo), "and nothing resurrects the cleared recipe");
    assert!(
        !variants_path(&photo).exists() && !dev.join("variants.json.bak").exists(),
        "the strip record goes with the develop — live and retired"
    );
    // Clearing an already-clean develop is a no-op, not an error.
    let again = clear_develop(&photo).expect("idempotent");
    assert!(!again.removed, "nothing left to remove");
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn variants_record_round_trips_relocatable_and_recovers_its_bak() {
    let base = std::env::temp_dir().join(format!("autoshade-store-test-variants-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_VARS.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    // Nothing persisted yet — the normal single-card case is silent None.
    assert!(read_variants(&photo).is_none(), "no record, no strip");
    // A generated background variant: raster origin inside the develop
    // dir, recipe carrying a Bitmap mask that also lives there.
    let master = dev.join("reimagine-1.png");
    std::fs::write(&master, b"png").unwrap();
    std::fs::write(dev.join("mask-sky.png"), b"png").unwrap();
    let mut gen_recipe = EditRecipe { exposure_ev: 0.25, ..Default::default() };
    gen_recipe.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: dev.join("mask-sky.png").to_string_lossy().into_owned() },
        ..Default::default()
    });
    let rec = VariantsRecord {
        extra: Default::default(),
        active_id: None,
        active_name: None,
        v: 1,
        active_kind: "fitted".into(),
        active_pos: 2,
        others: vec![
            VariantEntry {
                extra: Default::default(),
                id: None,
                name: None,
                kind: "original".into(),
                recipe: EditRecipe { contrast: 12.0, ..Default::default() },
                origin: None,
            },
            VariantEntry {
                extra: Default::default(),
                id: None,
                name: None,
                kind: "generated".into(),
                recipe: gen_recipe,
                origin: Some(master.clone()),
            },
        ],
    };
    write_variants(&photo, &rec).unwrap();
    // RELOCATABLE on disk: in-dev origin and mask stored by bare name.
    let raw = std::fs::read_to_string(variants_path(&photo)).unwrap();
    assert!(raw.contains("\"reimagine-1.png\""), "origin stored bare, got:\n{raw}");
    assert!(raw.contains("\"mask-sky.png\""), "mask raster stored bare, got:\n{raw}");
    assert!(!raw.contains(&dev.to_string_lossy().replace('\\', "\\\\")), "no absolute dev paths leak");
    // Reader resolves both back to absolute in-dev paths.
    let back = read_variants(&photo).expect("record parses");
    assert_eq!(back.active_kind, "fitted");
    assert_eq!(back.active_pos, 2);
    assert_eq!(back.others.len(), 2);
    assert_eq!(back.others[0].kind, "original");
    assert_eq!(back.others[0].recipe.contrast, 12.0);
    assert_eq!(back.others[0].origin, None);
    assert_eq!(back.others[1].origin.as_deref(), Some(master.as_path()));
    let MaskGeometry::Bitmap { path } = &back.others[1].recipe.masks[0].mask else { panic!() };
    assert_eq!(Path::new(path), dev.join("mask-sky.png"), "mask ref resolved to the dev dir");
    // Crash-window recovery: a second write retires the first to .bak;
    // losing the live file must republish it (the recover_orphan_baks
    // pair registered for variants.json).
    write_variants(&photo, &rec).unwrap();
    assert!(dev.join("variants.json.bak").exists(), "precondition: a retired record exists");
    std::fs::remove_file(variants_path(&photo)).unwrap();
    assert!(read_variants(&photo).is_some(), "the .bak republishes through the reader");
    // Explicit clear detaches BOTH copies and stays silent-idempotent.
    clear_variants(&photo).unwrap();
    assert!(read_variants(&photo).is_none(), "cleared means gone");
    assert!(!dev.join("variants.json.bak").exists(), "no resurrection bait left");
    clear_variants(&photo).unwrap();
    // A future format version is refused, not misread.
    std::fs::write(variants_path(&photo), b"{\"v\":9,\"active_kind\":\"original\",\"active_pos\":0,\"others\":[]}").unwrap();
    assert!(read_variants(&photo).is_none(), "future major refused");
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

/// 16-lane scan L08: an unreadable/foreign variants.json opened as a
/// single card, and the next ordinary save DELETED it (with its .bak) —
/// every background variant it recorded died to a Ctrl+S. The save
/// primitives now refuse while the record is unresolved.
#[test]
fn an_unresolved_strip_refuses_save_and_clear_but_not_reads() {
    let base = std::env::temp_dir().join(format!("autoshade-store-test-varunres-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_VARUNRES.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    std::fs::write(variants_path(&photo), b"not json at all").unwrap();
    assert!(
        matches!(read_variants_checked(&photo), VariantsRead::Unresolved),
        "garbage bytes are Unresolved, never Absent"
    );
    assert!(read_variants(&photo).is_none(), "the plain reader still degrades");

    clear_variants(&photo).expect_err("clearing over an unresolved strip must refuse");
    let rec = VariantsRecord {
        extra: Default::default(),
        active_id: None,
        active_name: None,
        v: 1,
        active_kind: "original".into(),
        active_pos: 0,
        others: Vec::new(),
    };
    write_variants(&photo, &rec).expect_err("overwriting an unresolved strip must refuse");
    assert_eq!(
        std::fs::read(variants_path(&photo)).unwrap(),
        b"not json at all",
        "the refused save left the record byte-identical"
    );

    // Removing the bad record ends the refusal — the normal flow resumes.
    std::fs::remove_file(variants_path(&photo)).unwrap();
    assert!(matches!(read_variants_checked(&photo), VariantsRead::Absent));
    write_variants(&photo, &rec).expect("a resolved store accepts saves again");
    clear_variants(&photo).expect("and clears");
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

/// R24-2: the version-name sidecar joins `.deleted-versions.json` in the
/// NON-GENERATIONAL club, and the two facts that class hangs on are
/// name-level — cheap to state here, and easy to break silently by
/// editing a list elsewhere in this file.
#[test]
fn the_version_meta_sidecar_keeps_the_non_generational_discipline() {
    // Adoption's list is an EXCLUSION list: being ABSENT from it is what
    // makes a file travel with an adopted develop. Putting the sidecar in
    // it would strand every name behind the adoption.
    assert!(!adoption_skips(".version-meta.json"));
    assert!(!adoption_skips(".deleted-versions.json"));
    // The generational pair list (`recover_orphan_baks`) covers exactly
    // the three files `publish_json_sidecar` retires. The advisory
    // sidecar must never join it: a republished `.bak` would resurrect
    // names the live file has since dropped.
    let src = std::path::Path::new("C:/nowhere/DSC_META.ARW");
    let dev = develop_dir(src);
    let pairs = [
        (recipe_target(src), dev.join("recipe.json.bak")),
        (pixel_source_path(src), dev.join("pixels.json.bak")),
        (variants_path(src), dev.join("variants.json.bak")),
    ];
    assert!(
        !pairs.iter().any(|(live, _)| *live == version_meta_path(&dev)),
        "the advisory sidecar must stay out of the generational pair list"
    );
    assert_eq!(version_meta_path(&dev).file_name().unwrap(), ".version-meta.json");
}

/// R24-2: `id` / `name` are ADDITIVE at v=1, in both directions.
///
/// FORWARD (the one that had to be verified before the field was added,
/// not after): what does a build that predates these fields do with a
/// record that carries them? `VariantEntry` / `VariantsRecord` are NOT
/// `deny_unknown_fields` — unlike `EditRecipe`, whose forward semantics
/// really is a hard refusal — so serde IGNORES anything it does not know
/// and the old build restores the strip complete, minus the names. This
/// test pins that by parsing a record with fields NO build has, which is
/// exactly the shape today's build is to a future one. The day someone
/// adds `deny_unknown_fields` here, a strip written by a newer AutoShade
/// would go `Unresolved` and every background variant would vanish from
/// the older one — this red test is the warning.
///
/// BACKWARD: a record written before the fields existed reads back with
/// both `None`, and re-serializes byte-identically to what the older
/// build wrote (`skip_serializing_if`) — adding the pair changed nothing
/// on disk for strips that use neither.
#[test]
fn variant_ids_and_names_are_additive_in_both_directions() {
    let base = std::env::temp_dir().join(format!("autoshade-store-test-varnames-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_VARNAMES.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    // FORWARD: unknown members (a hypothetical future build's) are
    // ignored, and the known ones still land.
    std::fs::write(
        variants_path(&photo),
        br#"{
              "v": 1,
              "active_kind": "original",
              "active_pos": 0,
              "active_id": "card-a",
              "active_name": "base",
              "active_rating": 5,
              "others": [
                {"kind":"fitted","recipe":{},"id":"card-b","name":"sunset","flagged":true}
              ]
            }"#,
    )
    .unwrap();
    let rec = match read_variants_checked(&photo) {
        VariantsRead::Strip(r) => r,
        _ => panic!(
            "a record carrying fields this build does not know must still restore — \
                 VariantEntry/VariantsRecord must never become deny_unknown_fields"
        ),
    };
    assert_eq!(rec.active_id.as_deref(), Some("card-a"));
    assert_eq!(rec.active_name.as_deref(), Some("base"));
    assert_eq!(rec.others[0].id.as_deref(), Some("card-b"));
    assert_eq!(rec.others[0].name.as_deref(), Some("sunset"));

    // A hand-edited name is CAPPED, never a reason to refuse the strip
    // (that would cost the user every background variant over a label).
    let long = "名".repeat(400);
    write_variants(
        &photo,
        &VariantsRecord {
            extra: Default::default(),
            v: 1,
            active_kind: "original".into(),
            active_pos: 0,
            active_id: Some("card-a".into()),
            active_name: Some(long.clone()),
            others: vec![VariantEntry {
                extra: Default::default(),
                kind: "generated".into(),
                recipe: EditRecipe::default(),
                origin: None,
                id: Some("card-b".into()),
                name: Some(long),
            }],
        },
    )
    .unwrap();
    let back = read_variants(&photo).expect("a long name is capped, not refused");
    assert!(back.active_name.as_deref().unwrap().len() <= MAX_STORE_NAME);
    assert!(back.others[0].name.as_deref().unwrap().len() <= MAX_STORE_NAME);
    assert!(
        back.active_name.as_deref().unwrap().ends_with('名'),
        "the cap must land on a char boundary, never mid-codepoint"
    );

    // BACKWARD: a strip that uses neither field serializes exactly as an
    // older build wrote it — no `"id": null` noise, no diff on disk.
    let bare = VariantsRecord {
        extra: Default::default(),
        v: 1,
        active_kind: "original".into(),
        active_pos: 0,
        active_id: None,
        active_name: None,
        others: vec![VariantEntry {
            extra: Default::default(),
            kind: "fitted".into(),
            recipe: EditRecipe::default(),
            origin: None,
            id: None,
            name: None,
        }],
    };
    let bytes = variants_record_bytes(&photo, &bare).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("\"id\""), "an id-less strip must not grow an id key: {text}");
    assert!(!text.contains("\"name\""), "a nameless strip must not grow a name key: {text}");
    assert!(!text.contains("active_id"), "…nor an active_id key: {text}");

    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

/// 16-lane scan L07: a develop pack naming `\\attacker\share\…` had its
/// origin PROBED on open (`exists()` = an outbound SMB authentication).
/// The refusal is lexical — no filesystem call may touch the path.
#[test]
#[cfg(windows)]
fn network_and_device_paths_are_refused_lexically() {
    assert!(remote_or_device_path(Path::new(r"\\attacker\share\master.png")));
    assert!(remote_or_device_path(Path::new(r"\\?\UNC\attacker\share\m.png")));
    assert!(remote_or_device_path(Path::new(r"\\.\PhysicalDrive0")));
    assert!(!remote_or_device_path(Path::new(r"C:\photos\master.png")));
    assert!(!remote_or_device_path(Path::new("master.png")));

    let base = std::env::temp_dir().join(format!("autoshade-store-test-uncorigin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_UNC.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(
        pixel_source_path(&photo),
        br#"{"origin": "\\\\attacker\\share\\master.png", "kind": "inplace"}"#,
    )
    .unwrap();
    assert!(
        read_pixel_source(&photo).is_none(),
        "a UNC origin must not be restored (nor probed)"
    );

    // A recipe's bitmap ref pointing at a share is disabled, not probed.
    let mut r = EditRecipe {
        masks: vec![crate::recipe::LocalAdjustment {
            mask: crate::recipe::MaskGeometry::Bitmap {
                path: r"\\attacker\share\mask.png".into(),
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    resolve_mask_paths(&mut r, &dev);
    let crate::recipe::MaskGeometry::Bitmap { path } = &r.masks[0].mask else {
        panic!("geometry kind must survive");
    };
    assert!(
        path.ends_with(".invalid-mask-reference"),
        "the share ref must be repointed at the never-existing sentinel: {path}"
    );
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn clear_pixel_source_detaches_the_retired_bak_too() {
    // Detach sequence: master A saved, master B saved (A retired to
    // .bak), then a parametric-only save clears the linkage. Without the
    // .bak removal, the next open's recover_orphan_baks resurrected
    // master A — a state the user explicitly left.
    let base = std::env::temp_dir().join(format!("autoshade-store-test-clearbak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let photo = base.join("DSC_CLEAR.ARW");
    std::fs::write(&photo, b"raw").unwrap();
    let dev = develop_dir(&photo);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    let master_a = dev.join("master-a.png");
    let master_b = dev.join("master-b.png");
    std::fs::write(&master_a, b"A").unwrap();
    std::fs::write(&master_b, b"B").unwrap();
    write_pixel_source(&photo, &master_a, false).unwrap();
    write_pixel_source(&photo, &master_b, false).unwrap();
    assert!(dev.join("pixels.json.bak").exists(), "precondition: A retired to .bak");
    clear_pixel_source(&photo).unwrap();
    assert!(!dev.join("pixels.json.bak").exists(), "detach must take the .bak too");
    assert!(!has_pixel_source(&photo), "nothing recorded anymore");
    assert!(read_pixel_source(&photo).is_none(), "and nothing resurrects on read");
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn tmp_names_are_process_unique_across_minters() {
    // The collision class R8 closed: every minter shares next_tmp_seq,
    // so no two tmp names for the same target can ever coincide.
    let t = Path::new("D:/x/v1.recipe.json");
    assert_ne!(sibling_tmp(t), sibling_tmp(t));
    let a = next_tmp_seq();
    let b = next_tmp_seq();
    assert!(b > a, "strictly monotone within the process");
}

#[test]
fn publish_no_clobber_never_replaces_an_owner() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-pnc-owner-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let to = dir.join("recipe.json");
    std::fs::write(&to, b"newer save").unwrap();
    let tmp = sibling_tmp(&to);
    std::fs::write(&tmp, b"older legacy").unwrap();
    assert!(!publish_no_clobber(&tmp, &to).unwrap(), "owner must win");
    assert_eq!(std::fs::read(&to).unwrap(), b"newer save");
    assert!(!tmp.exists(), "tmp must be consumed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn publish_no_clobber_lands_on_a_fresh_destination() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-pnc-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let to = dir.join("recipe.json");
    let tmp = sibling_tmp(&to);
    std::fs::write(&tmp, b"payload").unwrap();
    assert!(publish_no_clobber(&tmp, &to).unwrap());
    assert_eq!(std::fs::read(&to).unwrap(), b"payload");
    assert!(!tmp.exists(), "tmp must be consumed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn move_no_clobber_adopts_owner_and_keeps_the_source() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-mnc-adopt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let from = dir.join("legacy.xmp");
    let to = dir.join("central.xmp");
    std::fs::write(&from, b"legacy").unwrap();
    std::fs::write(&to, b"newer").unwrap();
    assert!(!move_file_no_clobber(&from, &to).unwrap());
    assert_eq!(std::fs::read(&to).unwrap(), b"newer", "owner intact");
    assert!(from.exists(), "adoption must not consume the source");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn migrate_gate_treats_a_nondirectory_as_empty_not_failure() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let not_a_dir = dir.join("out"); // exists, but is a FILE
    std::fs::write(&not_a_dir, b"x").unwrap();
    let (moved, failed) =
        migrate_legacy_in(&dir.join("root"), &not_a_dir, Path::new("D:/x/photo.arw"));
    assert!(!moved);
    assert!(!failed, "a non-directory is 'nothing legacy', not a retryable failure");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn photo_key_disambiguates_same_stem_different_folder() {
    // The exact bug this module exists to fix: DSC001.ARW in two folders
    // must never share one develop dir.
    let a = photo_key(Path::new("D:/trip-a/DSC001.ARW"));
    let b = photo_key(Path::new("D:/trip-b/DSC001.ARW"));
    // Windows folds BOTH halves of the key (NTFS is case-insensitive);
    // elsewhere the spelling is identity-relevant and stays.
    let want = if cfg!(windows) { "dsc001-" } else { "DSC001-" };
    assert!(a.starts_with(want), "{a}");
    assert!(b.starts_with(want), "{b}");
    assert_ne!(a, b);
    if cfg!(windows) {
        assert_eq!(
            a,
            photo_key(Path::new("d:/trip-a/dsc001.arw")),
            "case-variant spellings of ONE file must produce ONE key"
        );
    }
    // Stable across calls (persistent directory names).
    assert_eq!(a, photo_key(Path::new("D:/trip-a/DSC001.ARW")));
}

#[test]
fn fnv1a64_is_the_reference_function() {
    // Pin the published FNV-1a test vectors: this hash names directories
    // on disk, so it must never drift.
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
}

#[test]
fn resolve_and_relativize_round_trip() {
    let base = std::env::temp_dir().join(format!("autoshade-store-test-roundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let raster = base.join("mask-sky.png");
    std::fs::write(&raster, b"png").unwrap();

    let mut r = EditRecipe::default();
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: "mask-sky.png".into() },
        ..Default::default()
    });
    r.masks.push(LocalAdjustment {
        // Not under base and not existing → must stay untouched.
        mask: MaskGeometry::Bitmap { path: "out/other.mask.png".into() },
        ..Default::default()
    });
    resolve_mask_paths(&mut r, &base);
    let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
    assert_eq!(Path::new(path), raster.as_path(), "existing bare name resolves to base");
    let MaskGeometry::Bitmap { path } = &r.masks[1].mask else { panic!() };
    assert_eq!(path, "out/other.mask.png", "missing reference left alone");

    relativize_mask_paths(&mut r, &base);
    let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
    assert_eq!(path, "mask-sky.png", "absolute-under-base collapses back to bare");
    let _ = std::fs::remove_file(&raster);
}

#[test]
fn migrate_legacy_moves_recipe_xmp_versions_and_rasters() {
    // Runs against the real cwd ./out (like the render/fit tests) with
    // unique _store_mig_* names, and a temp store root via the _in APIs —
    // no process-global env mutation.
    let stem = "_store_mig_photo";
    let src = PathBuf::from(format!("D:/nowhere/{stem}.ARW"));
    let root = std::env::temp_dir().join(format!("autoshade-store-test-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all("out").unwrap();

    let raster = PathBuf::from(format!("out/{stem}.mask-sky.png"));
    std::fs::write(&raster, b"png").unwrap();
    let mut r = EditRecipe { exposure_ev: 0.5, ..Default::default() };
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: raster.to_string_lossy().into_owned() },
        ..Default::default()
    });
    let rj = PathBuf::from(format!("out/{stem}.recipe.json"));
    std::fs::write(&rj, serde_json::to_string_pretty(&r).unwrap()).unwrap();
    let xmp = PathBuf::from(format!("out/{stem}.xmp"));
    std::fs::write(&xmp, "<x:xmpmeta/>").unwrap();
    let v2 = PathBuf::from(format!("out/{stem}.v2.recipe.json"));
    std::fs::write(&v2, serde_json::to_string_pretty(&EditRecipe::default()).unwrap()).unwrap();

    assert_eq!(
        migrate_legacy_in(&root, Path::new("out"), &src),
        (true, false),
        "moved everything, no failures"
    );
    let dev = develop_dir_in(&root, &src);
    assert!(dev.join("recipe.json").exists());
    assert!(dev.join(format!("{stem}.xmp")).exists());
    assert!(dev.join("v2.recipe.json").exists());
    assert!(dev.join("mask-sky.png").exists(), "raster moved along");
    assert!(dev.join("source.txt").exists(), "breadcrumb written");
    assert!(
        rj.exists() && xmp.exists() && v2.exists() && raster.exists(),
        "ambiguous stem-keyed legacy bytes are copied, never consumed"
    );

    // The migrated recipe references the raster by bare name.
    let back: EditRecipe =
        serde_json::from_str(&std::fs::read_to_string(dev.join("recipe.json")).unwrap()).unwrap();
    let MaskGeometry::Bitmap { path } = &back.masks[0].mask else { panic!() };
    assert_eq!(path, "mask-sky.png");

    // Idempotent: a second call finds nothing legacy and reports
    // (nothing moved, nothing failed).
    assert_eq!(migrate_legacy_in(&root, Path::new("out"), &src), (false, false));

    // HIGH-1 (the third resurrection arm): a version the user DELETED
    // stays deleted across the next migrate scan — the retained legacy
    // v2 file must not re-publish it. Simulated at file level (the
    // delete API keys off the global store root, this fixture off a
    // temp root): central v2 swept, its delete registered.
    std::fs::remove_file(dev.join("v2.recipe.json")).unwrap();
    let reg = DeletedVersions {
        hwm: 2,
        deleted: vec![DeletedVersion {
            n: 2,
            recipe_raw: None,
            recipe_norm: None,
            rasters: None,
            xmp: None,
        }],
    };
    std::fs::write(
        deleted_versions_path(&dev),
        serde_json::to_string_pretty(&reg).unwrap(),
    )
    .unwrap();
    assert_eq!(migrate_legacy_in(&root, Path::new("out"), &src), (false, false));
    assert!(
        !dev.join("v2.recipe.json").exists(),
        "a deleted version is not re-migrated from the retained legacy file"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A temp-dir fixture base spelled CANONICALLY (env vars can carry an
/// off-case or 8.3 spelling of the temp dir, which would make lexical
/// and canonical keys differ for reasons unrelated to the test).
///
/// Not a Windows nicety: on macOS `env::temp_dir()` answers
/// `/var/folders/…`, and `/var` is a SYMLINK to `/private/var`, so
/// `identity_of` resolves every fixture photo to a spelling the lexical
/// key never sees. Any test whose subject is "canonical == lexical here"
/// must build its photos under THIS base, or it is measuring the runner's
/// temp dir — that failed `a_canonical_session_finishes_an_alias_sessions_crashed_adoption`
/// on the macOS CI leg (run 32398395462; ubuntu's `/tmp` has no symlink). Per-PROCESS (`-{pid}`)
/// like this file's other fixture roots: two batteries side by side raced ONE path (S1-fix §4).
fn canonical_temp(tag: &str) -> PathBuf {
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let base = strip_verbatim(&base).join(format!("autoshade-store-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// The network/device refusal must not be Windows-only.
///
/// `Component::Prefix` is produced by the WINDOWS path parser and nothing
/// else, so off Windows the match had nothing to match: a recipe written
/// on Windows and opened on a Mac had its network mask reference taken for
/// an ordinary relative name and joined to the develop dir.
#[test]
fn a_network_or_device_mask_reference_is_refused_on_every_platform() {
    for p in [r"\\nas\share\mask.png", "//nas/share/mask.png", r"\\.\COM1", r"\\?\UNC\nas\s\m.png"] {
        assert!(remote_or_device_path(Path::new(p)), "{p} must be refused everywhere");
    }
    for p in ["mask.png", "sub/mask.png", "/pictures/mask.png", "/tmp/m.png"] {
        assert!(!remote_or_device_path(Path::new(p)), "{p} is an ordinary path");
    }
    // A LOCAL drive-letter absolute stays honoured: the pixel-source writer
    // records them for masters that live outside the develop dir.
    assert!(!remote_or_device_path(Path::new(r"C:\photos\mask.png")));
}

/// The spelling test and the Windows parser are NOT one predicate, and the
/// difference is deliberate rather than an oversight.
///
/// The verbatim-LOCAL form starts with two separators but names a local
/// file, and `std::fs::canonicalize` hands those out by the dozen — so on
/// Windows the parser has to win, or a real raster would be disabled. Off
/// Windows that spelling cannot name a local file at all, so refusing it
/// there is the right answer rather than a compromise.
#[test]
fn the_verbatim_local_spelling_is_local_on_windows_and_refused_elsewhere() {
    let verbatim = Path::new(r"\\?\C:\photos\mask.png");
    assert!(unc_or_device_spelling(verbatim), "it does start with two separators");
    assert_eq!(
        remote_or_device_path(verbatim),
        !cfg!(windows),
        "Windows keeps it (canonicalize produces it); elsewhere it names nothing local"
    );
    // On Windows every path in these tests HAS a prefix, so the parser arm
    // answers them all and the non-Windows arm is unreachable here. Assert
    // it exists instead: deleting it is invisible on this platform and is
    // the whole refusal on the other.
    assert!(
        crate::source_before_tests(&crate::store::source_text())
            .contains("_ => unc_or_device_spelling(p),"),
        "the arm that carries the refusal off Windows"
    );
}

/// The store's directory NAME and the pre-rename adoption are one decision
/// per platform, and both had to be settled before the first Mac build
/// shipped: the directory under this name is keyed by a hash of an
/// absolute path, so changing it afterwards orphans every develop.
#[test]
fn the_store_directory_name_and_the_rename_adoption_agree_per_platform() {
    // One assertion over the PAIR, not four over the halves: the name and
    // the adoption are a single decision per platform, and saying it that
    // way also keeps `assert!` off a constant (clippy's
    // `assertions_on_constants`, which a `#[cfg]`-selected const trips).
    assert_eq!(
        (STORE_DIR_NAME, ADOPT_PRE_RENAME),
        if cfg!(target_os = "macos") {
            // That directory holds applications' DISPLAY names, and no Mac
            // ever ran the pre-rename spelling, so there is nothing to adopt
            // — on a case-insensitive volume an adoption could only rename
            // some other program's folder.
            ("AutoShade", false)
        } else {
            // Lowercase, and still adopting: these roots hold existing
            // users' develops, keyed by a hash of an absolute path.
            ("autoshade", true)
        },
        "the store directory name and the rename adoption are one decision"
    );
    // Windows cannot execute the macOS arms, so it asserts they EXIST:
    // deleting one is a silent no-op here, and on a Mac it is either an
    // orphaned store or the rename of a stranger's directory. The needles
    // below are spelled in THIS test, so the test module has to come off
    // first or every assertion matches its own text (it did).
    let src = crate::source_before_tests(&crate::store::source_text()).to_string();
    assert!(src.contains(r#"STORE_DIR_NAME: &str = "AutoShade""#), "the macOS store name");
    assert!(src.contains(r#".join("Library").join("Application Support")"#), "the macOS root");
    assert!(
        src.contains(r#"const ADOPT_PRE_RENAME: bool = !cfg!(target_os = "macos");"#),
        "the adoption opt-out"
    );
}

/// A directory ALIAS: a junction on Windows (no privilege needed, unlike
/// a Windows symlink), a symlink everywhere else. The fixture must build
/// LOUDLY or the test is vacuous (the L14#5 lesson: a fixture that cannot
/// build must never silently pass green).
///
/// The alias family used to be Windows-only, which left the canonical
/// identity rules — one photo through two spellings is one develop —
/// untested on the platform where an aliased library is ordinary (an
/// external drive under `/Volumes`, moved between machines).
fn make_junction(link: &Path, target: &Path) {
    #[cfg(windows)]
    let ok = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|o| o.status.success());
    #[cfg(unix)]
    let ok = std::os::unix::fs::symlink(target, link).is_ok();
    assert!(ok, "directory-alias fixture could not be built: {} -> {}", link.display(), target.display());
}

/// C1/F10: for a photo already spelled canonically on a plain local
/// drive, the canonical key is BYTE-IDENTICAL to the lexical key — the
/// property that keeps existing develop dirs un-rekeyed.
#[test]
fn a_plain_local_path_keeps_the_key_it_had_before_canonical_identity() {
    let dir = canonical_temp("c1-plain");
    let photo = dir.join("_c1_plain.arw");
    std::fs::write(&photo, b"raw").unwrap();
    assert_eq!(photo_key(&photo), photo_key_lexical(&photo));
    // An ABSENT photo under a real folder resolves through its parent
    // and keeps the same key too (no disclosure, no divergence).
    let absent = dir.join("_c1_never_existed.arw");
    assert_eq!(photo_key(&absent), photo_key_lexical(&absent));
    let _ = std::fs::remove_dir_all(&dir);
}

/// C1/F10 (user-decided 2026-08-10): network paths keep the lexical key
/// — identity never spends a network round trip.
#[test]
fn a_network_path_is_keyed_lexically_without_probing() {
    let unc = Path::new(r"\\nas-that-does-not-exist\share\photos\DSC001.ARW");
    assert_eq!(photo_key(unc), photo_key_lexical(unc));
}

/// C1/F10: a junction alias and its target are ONE photo — one key, one
/// develop dir, one develop lock.
#[test]
fn a_directory_junction_and_its_target_share_one_develop() {
    let dir = canonical_temp("c1-junction");
    let target = dir.join("real");
    std::fs::create_dir_all(&target).unwrap();
    let link = dir.join("alias");
    make_junction(&link, &target);
    let photo = target.join("_c1_junction.arw");
    std::fs::write(&photo, b"raw").unwrap();
    let via_alias = link.join("_c1_junction.arw");
    assert_ne!(
        photo_key_lexical(&via_alias),
        photo_key_lexical(&photo),
        "premise: the two spellings differ lexically"
    );
    assert_eq!(photo_key(&via_alias), photo_key(&photo), "one photo, one key");
    let _ = std::fs::remove_dir_all(&dir);
}

/// C1/F10: a develop saved by a pre-canonical build under the alias
/// spelling is ADOPTED into the canonical dir — copied source-wins into
/// the fresh canonical dir, the alias dir left intact with a superseded
/// pointer, the note surfaced.
#[test]
fn an_alias_develop_dir_is_adopted_without_clobbering() {
    let dir = canonical_temp("c1-adopt");
    let target = dir.join("real");
    std::fs::create_dir_all(&target).unwrap();
    let link = dir.join("alias");
    make_junction(&link, &target);
    let photo = target.join("_c1_adopt.arw");
    std::fs::write(&photo, b"raw").unwrap();
    let via_alias = link.join("_c1_adopt.arw");

    let root = store_root();
    let ld = root.join("develops").join(photo_key_lexical(&via_alias));
    let cd_expect = root.join("develops").join(photo_key(&via_alias));
    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd_expect);
    std::fs::create_dir_all(&ld).unwrap();
    std::fs::write(ld.join("recipe.json"), b"{\"exposure_ev\":0.5}").unwrap();
    std::fs::write(ld.join("mask-sky.png"), b"raster").unwrap();
    std::fs::write(ld.join("source.txt"), b"breadcrumb").unwrap();

    let dev = develop_dir(&via_alias);
    assert_eq!(dev, cd_expect, "the photo now lives under its canonical key");
    assert_eq!(std::fs::read(dev.join("recipe.json")).unwrap(), b"{\"exposure_ev\":0.5}");
    assert_eq!(std::fs::read(dev.join("mask-sky.png")).unwrap(), b"raster");
    assert!(dev.join("adopted-from.txt").exists(), "the adoption breadcrumb landed");
    assert!(!dev.join("adopting-from.txt").exists(), "the in-flight marker was consumed");
    assert!(
        ld.join("superseded-by.txt").exists(),
        "the alias dir points at its successor"
    );
    assert_eq!(
        std::fs::read(ld.join("recipe.json")).unwrap(),
        b"{\"exposure_ev\":0.5}",
        "adoption copies — the alias dir stays intact as a frozen backup"
    );
    assert!(
        matches!(take_alias_note(&via_alias), Some(AliasNote::Adopted { .. })),
        "the adoption is surfaced to the GUI once"
    );

    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd_expect);
    let _ = std::fs::remove_dir_all(&dir);
}

/// C1/F10 (user-decided 2026-08-10): when BOTH spellings hold a real
/// develop, the canonical one wins, the alias is left untouched, and
/// the fact is disclosed durably + to the GUI — never merged, never
/// guessed by mtime.
#[test]
fn two_spellings_with_two_develops_keep_both_and_disclose() {
    let dir = canonical_temp("c1-collide");
    let target = dir.join("real");
    std::fs::create_dir_all(&target).unwrap();
    let link = dir.join("alias");
    make_junction(&link, &target);
    let photo = target.join("_c1_collide.arw");
    std::fs::write(&photo, b"raw").unwrap();
    let via_alias = link.join("_c1_collide.arw");

    let root = store_root();
    let ld = root.join("develops").join(photo_key_lexical(&via_alias));
    let cd = root.join("develops").join(photo_key(&via_alias));
    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd);
    std::fs::create_dir_all(&ld).unwrap();
    std::fs::create_dir_all(&cd).unwrap();
    std::fs::write(ld.join("recipe.json"), b"alias-develop").unwrap();
    std::fs::write(cd.join("recipe.json"), b"canonical-develop").unwrap();

    let dev = develop_dir(&via_alias);
    assert_eq!(dev, cd);
    assert_eq!(
        std::fs::read(cd.join("recipe.json")).unwrap(),
        b"canonical-develop",
        "the canonical develop is untouched"
    );
    assert_eq!(
        std::fs::read(ld.join("recipe.json")).unwrap(),
        b"alias-develop",
        "the alias develop is untouched — no merge, no delete"
    );
    assert!(!ld.join("superseded-by.txt").exists(), "a collision never supersedes");
    assert!(cd.join("aliased-develops.txt").exists(), "the fact is durable");
    assert!(
        matches!(take_alias_note(&via_alias), Some(AliasNote::SecondDevelop { .. })),
        "the collision is surfaced to the GUI once"
    );

    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd);
    let _ = std::fs::remove_dir_all(&dir);
}

/// C1/F10: unsettled transaction state under the alias spelling defers
/// adoption — this session keys lexically (the pre-upgrade behaviour),
/// the residue settles in place, the NEXT session adopts.
#[test]
fn a_pending_transaction_defers_adoption() {
    let dir = canonical_temp("c1-defer");
    let target = dir.join("real");
    std::fs::create_dir_all(&target).unwrap();
    let link = dir.join("alias");
    make_junction(&link, &target);
    let photo = target.join("_c1_defer.arw");
    std::fs::write(&photo, b"raw").unwrap();
    let via_alias = link.join("_c1_defer.arw");

    let root = store_root();
    let ld = root.join("develops").join(photo_key_lexical(&via_alias));
    let cd = root.join("develops").join(photo_key(&via_alias));
    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd);
    std::fs::create_dir_all(&ld).unwrap();
    std::fs::write(ld.join("recipe.json"), b"{}").unwrap();
    std::fs::write(ld.join("clear.pending"), b"develop clear in progress\n").unwrap();

    let dev = develop_dir(&via_alias);
    assert_eq!(dev, ld, "unsettled state keeps the pre-upgrade lexical key");
    assert!(!ld.join("superseded-by.txt").exists());

    let _ = std::fs::remove_dir_all(&ld);
    let _ = std::fs::remove_dir_all(&cd);
    let _ = std::fs::remove_dir_all(&dir);
}

/// L13#2: the badge/resume predicate counts the sidecar Lightroom
/// itself writes beside the RAW — a photo edited only in Lightroom
/// showed no ● yet opened with its LR develop, and CLI batch spent a
/// paid analyze on it.
#[test]
fn has_develop_or_sidecar_counts_a_lightroom_sidecar_beside_the_raw() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-lr-badge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_lr_badge.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::write(raw.with_extension("xmp"), b"<x:xmpmeta/>").unwrap();

    assert!(!has_develop(&raw), "no store develop");
    assert!(has_develop_or_sidecar(&raw), "the LR sidecar counts");

    // The is_raw guard: a baked photo's neighbouring .xmp is not ours.
    let png = dir.join("_store_lr_badge.png");
    std::fs::write(&png, b"png").unwrap();
    std::fs::write(png.with_extension("xmp"), b"<x:xmpmeta/>").unwrap();
    let png_dev = develop_dir(&png);
    let _ = std::fs::remove_dir_all(&png_dev);
    assert!(!has_develop_or_sidecar(&png), "a baked photo's sidecar does not count");

    // A pending clear still outranks the sidecar probe (L03).
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();
    assert!(!has_develop_or_sidecar(&raw), "a pending clear masks the badge");

    // Regression guard: the SIBLING did not leak into has_develop —
    // rank_lightroom_sidecar still classifies a sidecar-only photo as
    // LrSidecar::Only (has_develop false), not a store contest.
    let _ = std::fs::remove_dir_all(&dev);
    match lightroom_sidecar(&raw) {
        LrSidecar::Only(_) => {}
        _ => panic!("a sidecar-only photo must rank as LrSidecar::Only"),
    }

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&png_dev);
}

/// L13#1: the one-lock snapshot carries the Lightroom sidecar and the
/// store's XMP projection with the open path's precedence, so the batch
/// renderer can answer exactly what opening the photo would show.
#[test]
fn a_develop_snapshot_carries_the_xmp_layers_the_open_path_reads() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-snap-xmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_snap_xmp.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    // XMP-only develop: the store projection answers, no recipe.
    let xmp = crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 21.0, ..Default::default() });
    std::fs::write(xmp_target(&raw), &xmp).unwrap();
    let snap = read_develop_snapshot(&raw).unwrap();
    assert!(snap.recipe.is_none());
    assert!(snap.lr_xmp.is_none());
    let (text, kind) = snap.store_xmp.expect("the projection rides the snapshot");
    assert_eq!(text, xmp);
    assert_eq!(kind, "XMP");

    // A NEWER Lightroom sidecar beside the RAW outranks the recipe.
    std::fs::write(recipe_target(&raw), b"{\"exposure_ev\":0.5}").unwrap();
    let lr = raw.with_extension("xmp");
    std::fs::write(
        &lr,
        crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&lr)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
        .unwrap();
    let snap = read_develop_snapshot(&raw).unwrap();
    assert!(snap.recipe.is_some(), "the recipe still rides along");
    let (_, kind) = snap.lr_xmp.expect("the newer Lightroom sidecar rides the snapshot");
    assert!(kind.contains("Lightroom"), "{kind}");

    // An unreadable sidecar is disclosed, never folded into absence.
    std::fs::write(&lr, [0xFF, 0xFE, 0x00, 0xDC, 0x00]).unwrap();
    let snap = read_develop_snapshot(&raw).unwrap();
    assert!(snap.lr_xmp.is_none());
    assert!(snap.lr_unreadable.is_some(), "unreadable is not absent");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L13#3: the backup gate snapshots the Lightroom sidecar when it is
/// the newest intent — store develop FIRST, sidecar SECOND, so version
/// numbers encode intent order; repeats dedup; a neutral sidecar is not
/// a save.
#[test]
fn a_newer_lightroom_sidecar_is_snapshotted_before_a_programmatic_write() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-lr-backup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_lr_backup.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    let stored = EditRecipe { exposure_ev: 0.5, ..Default::default() };
    std::fs::write(recipe_target(&raw), serde_json::to_string(&stored).unwrap()).unwrap();
    let lr = raw.with_extension("xmp");
    let lr_text = crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() });
    std::fs::write(&lr, &lr_text).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&lr)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
        .unwrap();

    let incoming = EditRecipe { exposure_ev: 1.5, ..Default::default() };
    let n = backup_saved_develop(&raw, Some(&incoming)).unwrap();
    assert_eq!(n, Some(2), "two snapshots: the store develop, then the newer sidecar");
    assert_eq!(list_versions(&raw), vec![1, 2]);
    let v1: EditRecipe =
        serde_json::from_str(&std::fs::read_to_string(version_target(&raw, 1)).unwrap())
            .unwrap();
    assert_eq!(v1.exposure_ev, 0.5, "v1 is the store develop (older intent)");
    let stem = crate::pipeline::stem(&raw);
    assert_eq!(
        std::fs::read_to_string(dev.join(format!("v2.{stem}.xmp"))).unwrap(),
        lr_text,
        "v2 carries the sidecar's lossless bytes (newer intent)"
    );

    // The real caller flow: the gated write lands, then a LATER gated
    // write with the same content — the sidecar unchanged. Both halves
    // dedup: no new version.
    std::fs::write(recipe_target(&raw), serde_json::to_string(&incoming).unwrap()).unwrap();
    let versions_before = list_versions(&raw);
    let again = backup_saved_develop(&raw, Some(&incoming)).unwrap();
    assert_eq!(again, None, "nothing new to snapshot");
    assert_eq!(list_versions(&raw), versions_before);

    // The sidecar file itself is never touched.
    assert_eq!(std::fs::read_to_string(&lr).unwrap(), lr_text);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// L13#3: a sidecar-ONLY develop (no store files at all) is snapshotted
/// instead of reported as "nothing to snapshot" — and a NEUTRAL sidecar
/// still is not a save.
#[test]
fn a_sidecar_only_develop_is_snapshotted_instead_of_reported_as_nothing() {
    let dir = std::env::temp_dir().join(format!("autoshade-store-test-lr-only-backup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_store_lr_only.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    let lr = raw.with_extension("xmp");
    let lr_text = crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 12.0, ..Default::default() });
    std::fs::write(&lr, &lr_text).unwrap();

    let n = backup_saved_develop(&raw, None).unwrap();
    assert_eq!(n, Some(1), "the sidecar IS the develop and is preserved");
    let stem = crate::pipeline::stem(&raw);
    assert_eq!(
        std::fs::read_to_string(dev.join(format!("v1.{stem}.xmp"))).unwrap(),
        lr_text
    );

    // Neutral sidecar: not a save, nothing snapshotted.
    let raw2 = dir.join("_store_lr_neutral.arw");
    std::fs::write(&raw2, b"raw").unwrap();
    let dev2 = develop_dir(&raw2);
    let _ = std::fs::remove_dir_all(&dev2);
    std::fs::write(raw2.with_extension("xmp"), b"<x:xmpmeta/>").unwrap();
    assert_eq!(backup_saved_develop(&raw2, None).unwrap(), None);
    assert!(list_versions(&raw2).is_empty());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
    let _ = std::fs::remove_dir_all(&dev2);
}

#[test]
fn backup_snapshot_is_a_copy_and_freezes_rasters() {
    // Unique fake path → its own hashed develop dir under the real store
    // root (same isolation pattern as the GUI sidecar test); scrubbed
    // before and after.
    let src = PathBuf::from("D:/nowhere/_store_backup_test.ARW");
    let dev = develop_dir(&src);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("mask-zone-sky.png"), b"OLD").unwrap();
    let mut r = EditRecipe { exposure_ev: 0.4, ..Default::default() };
    r.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: "mask-zone-sky.png".into() },
        ..Default::default()
    });
    std::fs::write(recipe_target(&src), serde_json::to_string_pretty(&r).unwrap()).unwrap();

    // Identical incoming (after resolve) → no snapshot spam.
    let mut same = r.clone();
    resolve_mask_paths(&mut same, &dev);
    assert_eq!(backup_saved_develop(&src, Some(&same)).unwrap(), None);

    // Unconditional snapshot: v1 appears, the WORKING recipe stays (copy,
    // not move — callers snapshot BEFORE operations that may still fail),
    // and the raster is frozen with the snapshot's reference rewritten.
    assert_eq!(backup_saved_develop(&src, None).unwrap(), Some(1));
    assert!(recipe_target(&src).exists(), "copy semantics: working recipe stays");
    let snap: EditRecipe =
        serde_json::from_str(&std::fs::read_to_string(version_target(&src, 1)).unwrap())
            .unwrap();
    let MaskGeometry::Bitmap { path } = &snap.masks[0].mask else { panic!() };
    assert_eq!(path, "v1.mask-zone-sky.png");
    // Overwrite the live raster (what a re-run zoned fit does) — the
    // frozen copy must keep the OLD bytes, or the snapshot lies.
    std::fs::write(dev.join("mask-zone-sky.png"), b"NEW").unwrap();
    assert_eq!(std::fs::read(dev.join("v1.mask-zone-sky.png")).unwrap(), b"OLD");
    // Frozen rasters never pollute the version list.
    assert_eq!(list_versions(&src), vec![1]);
    // delete_version sweeps the snapshot recipe AND its frozen raster.
    delete_version(&src, 1).unwrap();
    assert!(!version_target(&src, 1).exists());
    assert!(!dev.join("v1.mask-zone-sky.png").exists());
    // The discard fingerprint includes the raster BYTES (NIT-9): the
    // deleted v1 froze the OLD pixels, the live raster now holds NEW —
    // structurally equal is NOT the discarded content, so the gate
    // still preserves it, under a fresh number, never the burned v1.
    assert_eq!(backup_saved_develop(&src, None).unwrap(), Some(2));
    assert_eq!(list_versions(&src), vec![2]);
    // Deleting THAT snapshot (whose frozen raster == the live bytes)
    // tombstones the save for real: no auto re-preservation...
    delete_version(&src, 2).unwrap();
    assert_eq!(backup_saved_develop(&src, None).unwrap(), None);
    assert_eq!(list_versions(&src), Vec::<u32>::new());
    // ...while the EXPLICIT save path stays ungated — the claim works
    // and moves past both burned numbers.
    let (n, _) = claim_version(&src).unwrap();
    assert_eq!(n, 3, "explicit 「＋ Save as version」 is never blocked by the registry");
    let _ = std::fs::remove_dir_all(&dev);
}


    #[test]
    fn the_no_hard_link_publish_fallback_never_replaces_an_owner() {
        let dir = std::env::temp_dir().join(format!("autoshade-store-test-pnc-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let refuse_links = |_: &Path, _: &Path| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "forced fallback",
            ))
        };

        let owned = dir.join("owned.json");
        std::fs::write(&owned, b"newer").unwrap();
        let old_tmp = sibling_tmp(&owned);
        std::fs::write(&old_tmp, b"older").unwrap();
        assert!(!publish_no_clobber_with(&old_tmp, &owned, &refuse_links).unwrap());
        assert_eq!(std::fs::read(&owned).unwrap(), b"newer");
        assert!(!old_tmp.exists(), "the rejected stage is consumed");

        let fresh = dir.join("fresh.json");
        let fresh_tmp = sibling_tmp(&fresh);
        std::fs::write(&fresh_tmp, b"payload").unwrap();
        assert!(publish_no_clobber_with(&fresh_tmp, &fresh, &refuse_links).unwrap());
        assert_eq!(std::fs::read(&fresh).unwrap(), b"payload");
        assert!(!fresh_tmp.exists(), "the published stage is consumed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nonblocking_develop_lock_reports_contention_and_recovers_after_drop() {
        let root = std::env::temp_dir().join(format!("autoshade-store-test-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let photo = Path::new("D:/photos/LOCK.ARW");

        with_develop_lock_in(&root, photo, DevelopLockMode::Wait, || {
            let root = root.clone();
            let busy = std::thread::spawn(move || {
                with_develop_lock_in(
                    &root,
                    Path::new("D:/photos/LOCK.ARW"),
                    DevelopLockMode::NoWait,
                    || Ok::<_, std::io::Error>(()),
                )
            })
            .join()
            .unwrap();
            assert_eq!(busy.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
            Ok::<_, std::io::Error>(())
        })
        .unwrap();

        with_develop_lock_in(&root, photo, DevelopLockMode::NoWait, || {
            Ok::<_, std::io::Error>(())
        })
        .expect("dropping the owner releases the kernel lock");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// L01: the settings lock is the develop lock's machinery pointed at
    /// ONE store-root-wide file — kernel-owned (it reaches the GUI and
    /// serve PROCESSES; threads model them here), reentrant on its own
    /// thread (a writer's cycle contains the loader's rescue), and
    /// NoWait-refusing while held.
    #[test]
    fn a_settings_lock_serializes_writers_and_reenters_on_its_thread() {
        let root = std::env::temp_dir().join(format!("autoshade-store-test-settings-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let nested = with_settings_lock_in(&root, DevelopLockMode::Wait, || {
            // The rescue inside a locked writer's own load re-enters
            // instead of deadlocking against its own cycle.
            with_settings_lock_in(&root, DevelopLockMode::NoWait, || {
                Ok::<_, std::io::Error>(7)
            })
        });
        assert_eq!(nested.unwrap(), 7);

        with_settings_lock_in(&root, DevelopLockMode::Wait, || {
            let root2 = root.clone();
            let busy = std::thread::spawn(move || {
                with_settings_lock_in(&root2, DevelopLockMode::NoWait, || {
                    Ok::<_, std::io::Error>(())
                })
            })
            .join()
            .unwrap();
            assert_eq!(busy.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
            Ok::<_, std::io::Error>(())
        })
        .unwrap();

        with_settings_lock_in(&root, DevelopLockMode::NoWait, || {
            Ok::<_, std::io::Error>(())
        })
        .expect("dropping the owner releases the settings lock");
        let _ = std::fs::remove_dir_all(&root);
    }


    /// L03: the out-of-module publish tail — staged bytes land intact
    /// under the live name, the stage is consumed, and the staged
    /// file's mode travels with the rename (the 0600 settings claim).
    #[test]
    fn durable_replace_lands_complete_bytes_and_consumes_its_stage() {
        let dir = std::env::temp_dir().join(format!("autoshade-store-test-durable-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let live = dir.join("settings.json");
        std::fs::write(&live, b"old").unwrap();
        let staged = dir.join("settings.json.stage");
        std::fs::write(&staged, b"complete new bytes").unwrap();

        durable_replace(&staged, &live).unwrap();

        assert_eq!(std::fs::read(&live).unwrap(), b"complete new bytes");
        assert!(!staged.exists(), "a completed publish consumes its stage");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn durable_write_replaces_complete_bytes_and_consumes_its_stage() {
        let dir = std::env::temp_dir().join(format!("autoshade-store-test-durable-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("recipe.json");
        std::fs::write(&target, b"old").unwrap();

        durable_write(&target, b"complete replacement").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"complete replacement");
        let stages = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(stages, 0, "a completed durable publish consumes its stage");
        let _ = std::fs::remove_dir_all(&dir);
    }


    /// R22 L2①: the sidecar hand-off is the only writer that ever stages
    /// inside the user's PHOTO folder, so it is the only thing that can
    /// collect its own orphans — a crash between stage and rename used to
    /// leave `<name>.xmp.tmp.<pid>.<seq>` there forever.
    ///
    /// The sweep has to be surgical: this pins BOTH halves — the orphan goes,
    /// and every neighbour (the live sidecar, another photo's stage, a
    /// look-alike that is not our pattern, the photo itself) stays.
    #[test]
    fn a_stale_sidecar_stage_is_reclaimed_and_nothing_else_is_touched() {
        let dir = std::env::temp_dir().join(format!("autoshade-store-test-sidecar-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("DSC00042.xmp");
        let orphan = dir.join("DSC00042.xmp.tmp.4242.7");
        let keep = [
            to.clone(),                              // the live sidecar
            dir.join("DSC00042.ARW"),                // the photo itself
            dir.join("DSC00099.xmp.tmp.4242.7"),     // ANOTHER photo's stage
            dir.join("DSC00042.xmp.tmp.abc.7"),      // not our <pid>.<seq> shape
            dir.join("DSC00042.xmp.tmp.4242.7.bak"), // …nor this one
            dir.join("DSC00042.xmp.backup"),         // a user's own file
        ];
        for p in std::iter::once(&orphan).chain(keep.iter()) {
            std::fs::write(p, b"x").unwrap();
        }

        // grace = 0: the fixtures were written milliseconds ago, and this is
        // the "long since orphaned" case.
        sweep_stale_sidecar_stages(&to, std::time::Duration::ZERO);
        assert!(!orphan.exists(), "the orphaned stage must be reclaimed");
        for p in &keep {
            assert!(p.exists(), "the sweep must not touch {}", p.display());
        }

        // …and the grace period is what protects a CONCURRENT publish: a
        // stage this fresh belongs to someone still writing it.
        let fresh = dir.join("DSC00042.xmp.tmp.4243.1");
        std::fs::write(&fresh, b"x").unwrap();
        sweep_stale_sidecar_stages(&to, SIDECAR_STAGE_GRACE);
        assert!(fresh.exists(), "a stage younger than the grace period is left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R22 L2②: `durable_write_tracked` reports whether the bytes reached
    /// the live name, which is what lets `export_xmp_beside` keep a
    /// delivered sidecar instead of deleting it when only the durability
    /// tail failed.
    ///
    /// Two of the three states are reachable in a test; the third —
    /// published, then `finish_parent` fails — has NO injection seam on
    /// Windows, where `durable_os::finish_parent` is infallible by
    /// construction (MOVEFILE_WRITE_THROUGH does that work inside the
    /// rename). It is covered by the type instead: the caller's `(true,
    /// Err)` arm cannot delete, because deletion lives only under `(false,
    /// Err)`.
    #[test]
    fn a_durable_write_reports_whether_the_bytes_reached_the_live_name() {
        let dir = std::env::temp_dir().join(format!("autoshade-store-test-durable-tracked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let target = dir.join("sidecar.xmp");
        let (published, result) = durable_write_tracked(&target, b"<x:xmpmeta/>");
        assert!(result.is_ok() && published, "a completed publish reports published");
        assert_eq!(std::fs::read(&target).unwrap(), b"<x:xmpmeta/>");

        // A failure BEFORE the rename must report `false` — that is the only
        // state in which the caller is allowed to remove its claim.
        let blocked = dir.join("wall").join("sidecar.xmp");
        std::fs::write(dir.join("wall"), b"a FILE where the parent dir would go").unwrap();
        let (published, result) = durable_write_tracked(&blocked, b"<x:xmpmeta/>");
        assert!(result.is_err(), "staging under a file must fail");
        assert!(!published, "nothing reached the live name");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Everything this test creates OUTSIDE its own temp dir, removed on
    /// the way out — including the way out through a panic.
    ///
    /// **This is the fixture, not tidiness (R29 C3/C4).** The root-aware
    /// clear twin exercises the production sweep without mutating the
    /// process-global store or cwd-relative legacy state.
    /// The fixture stays in the temporary root and is cleaned by `Drop`
    /// on both success and unwind, with the guard armed before writes.
    struct LegacyTombstoneFixture {
        base: PathBuf,
        legacy: PathBuf,
        develops: Vec<PathBuf>,
    }

    impl Drop for LegacyTombstoneFixture {
        fn drop(&mut self) {
            // Best-effort, exactly as before — a cleanup that PANICKED
            // while unwinding would abort the whole test process and
            // replace the real assertion message with a double-panic.
            let _ = std::fs::remove_file(&self.legacy);
            for d in &self.develops {
                let _ = std::fs::remove_dir_all(d);
            }
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn clearing_one_same_stem_photo_suppresses_but_never_unlinks_legacy_bytes() {
        let base = std::env::temp_dir().join(format!("autoshade-store-test-legacy-tombstone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let a = base.join("trip-a").join("DSC001.ARW");
        let b = base.join("trip-b").join("DSC001.ARW");
        let legacy_root = base.join("out");
        let legacy = legacy_root.join("DSC001.recipe.json");
        // ARMED BEFORE THE FIRST THING THAT CAN FAIL, and before anything
        // is written: `develop_dir` is resolved here, once, so the guard
        // targets the same key the test does even if a later call would
        // resolve differently.
        let _fixture = LegacyTombstoneFixture {
            base: base.clone(),
            legacy: legacy.clone(),
            develops: vec![develop_dir_in(&base, &a), develop_dir_in(&base, &b)],
        };
        if let Some(parent) = legacy.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&legacy, b"{\"contrast\":22.0}").unwrap();

        assert_eq!(legacy_recipe_in(&base, &legacy_root, &a), legacy);
        assert_eq!(legacy_recipe_in(&base, &legacy_root, &b), legacy);
        let outcome = clear_develop_in(&base, std::slice::from_ref(&legacy_root), &a).unwrap();
        assert!(outcome.removed, "suppressing a visible legacy develop is a clear");
        assert!(legacy.exists(), "the ambiguous shared file is never unlinked");
        assert_eq!(legacy_recipe_in(&base, &legacy_root, &a), suppressed_legacy_path_in(
            &base,
            &a,
            "DSC001.recipe.json",
        ));
        assert_eq!(legacy_recipe_in(&base, &legacy_root, &b), legacy, "photo B still sees the old store");
        assert_eq!(
            std::fs::read(&legacy).unwrap(),
            b"{\"contrast\":22.0}",
            "the legacy bytes are preserved verbatim"
        );
    }

    /// The fixture's own contract, because a cleanup that only runs when
    /// nothing went wrong is the bug this pair was written for.
    ///
    /// Drives the guard directly over a stand-in tombstone and a stand-in
    /// legacy file, from inside a closure that PANICS — the shape the real
    /// test takes when an assert fires — and proves the files are gone on
    /// the far side of the unwind. MUTATION-LINED: turning the `Drop` impl
    /// back into an empty body, or moving the guard's construction below
    /// the first `assert`, fails this.
    #[test]
    fn the_tombstone_fixture_cleans_up_through_a_panic() {
        let base = std::env::temp_dir().join(format!("autoshade-store-test-tombstone-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let legacy = base.join("stand-in.recipe.json");
        std::fs::write(&legacy, b"{}").unwrap();
        let dev = base.join("develops").join("stand-in");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("legacy.tombstone"), LEGACY_TOMBSTONE).unwrap();
        assert!(dev.join("legacy.tombstone").exists(), "the probe residue starts present");

        let (b2, l2, d2) = (base.clone(), legacy.clone(), dev.clone());
        let died = std::panic::catch_unwind(move || {
            let _fixture = LegacyTombstoneFixture {
                base: b2,
                legacy: l2,
                develops: vec![d2],
            };
            panic!("the assert that used to skip the cleanup");
        });
        assert!(died.is_err(), "the closure must actually unwind");
        assert!(!dev.join("legacy.tombstone").exists(), "the tombstone is swept by Drop");
        assert!(!legacy.exists(), "the legacy stand-in is swept by Drop");
        assert!(!base.exists(), "and so is the whole fixture root");
    }


    #[test]
    fn unknown_store_kinds_are_refused_without_rewriting_the_newer_record() {
        let base = std::env::temp_dir().join(format!("autoshade-store-test-unknown-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("UNKNOWN.ARW");
        let dev = develop_dir(&photo);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();

        let master = dev.join("future-master.png");
        std::fs::write(&master, b"png").unwrap();
        let pixel_bytes = format!(
            "{{\"origin\":{},\"kind\":\"layered\"}}",
            serde_json::to_string("future-master.png").unwrap()
        );
        std::fs::write(pixel_source_path(&photo), &pixel_bytes).unwrap();
        assert!(read_pixel_source(&photo).is_none());
        assert_eq!(
            std::fs::read_to_string(pixel_source_path(&photo)).unwrap(),
            pixel_bytes
        );

        let variants = serde_json::json!({
            "v": 1,
            "active_kind": "stacked",
            "active_pos": 0,
            "others": [{
                "kind": "original",
                "recipe": EditRecipe::default(),
                "origin": null
            }]
        });
        let variant_bytes = serde_json::to_vec_pretty(&variants).unwrap();
        std::fs::write(variants_path(&photo), &variant_bytes).unwrap();
        assert!(read_variants(&photo).is_none());
        assert_eq!(std::fs::read(variants_path(&photo)).unwrap(), variant_bytes);

        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }


    #[test]
    fn very_long_source_names_produce_portable_but_distinct_develop_keys() {
        let shared = "相片".repeat(180);
        let a = PathBuf::from(format!("D:/roll/{shared}a.ARW"));
        let b = PathBuf::from(format!("D:/roll/{shared}b.ARW"));
        let ka = photo_key(&a);
        let kb = photo_key(&b);

        assert!(ka.len() <= 240, "UTF-8 component is bounded: {}", ka.len());
        assert!(
            ka.encode_utf16().count() <= 240,
            "UTF-16 component is bounded"
        );
        assert_ne!(ka, kb, "the full absolute path still feeds the hash");
        assert_eq!(
            ka.rsplit_once('-').unwrap().1.len(),
            16,
            "the stable identity suffix is retained"
        );
    }


    #[test]
    fn parsed_relative_store_paths_cannot_escape_the_develop_directory() {
        let base = std::env::temp_dir().join(format!("autoshade-store-test-contained-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dev = base.join("develop");
        std::fs::create_dir_all(&dev).unwrap();
        let outside = base.join("outside.png");
        std::fs::write(&outside, b"outside").unwrap();

        assert!(contained_join(&dev, Path::new("../outside.png")).is_none());
        assert_eq!(
            contained_join(&dev, Path::new("inside.png")),
            Some(dev.join("inside.png"))
        );

        let mut recipe = EditRecipe::default();
        recipe.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap {
                path: "../outside.png".into(),
            },
            ..Default::default()
        });
        resolve_mask_paths(&mut recipe, &dev);
        let MaskGeometry::Bitmap { path } = &recipe.masks[0].mask else {
            panic!()
        };
        assert_eq!(Path::new(path), dev.join(".invalid-mask-reference"));
        assert_ne!(Path::new(path), outside);

        let _ = std::fs::remove_dir_all(&base);
    }

/// GROUP A (2026-10-26): the lens blur's depth map and protect matte and a
/// pixel layer's patch are rasters a recipe references exactly like a bitmap
/// mask, and a develop store from someone else is untrusted input for all of
/// them. Before `EditRecipe::raster_paths_mut` the load-time guard walked only
/// the masks, so a `\\host\share\d.png` depth map made a render
/// authenticate to a remote host and a `..` layer path read outside the store.
fn raster_refs(r: &mut EditRecipe) -> (String, String, String) {
    let lb = r.lens_blur.as_ref().expect("lens blur kept");
    (lb.depth_map.clone(), lb.protect.clone().expect("protect kept"), r.pixel_layers[0].path.clone())
}

fn recipe_with_rasters(depth: &str, protect: &str, layer: &str) -> EditRecipe {
    EditRecipe {
        lens_blur: Some(crate::render::LensBlur {
            depth_map: depth.into(),
            protect: Some(protect.into()),
            ..Default::default()
        }),
        pixel_layers: vec![crate::render::PixelLayer { path: layer.into(), ..Default::default() }],
        ..Default::default()
    }
}

#[test]
fn depth_protect_and_layer_paths_are_refused_at_load_like_a_mask_path() {
    let base = std::env::temp_dir().join(format!("autoshade-store-test-rasters-guard-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let sentinel = base.join(".invalid-mask-reference");
    let mut r = recipe_with_rasters(r"\\host\share\d.png", "//host/share/p.png", "../../outside.png");
    resolve_mask_paths(&mut r, &base);
    let (d, p, l) = raster_refs(&mut r);
    for (what, got) in [("depth map", &d), ("protect matte", &p), ("layer", &l)] {
        assert_eq!(Path::new(got), sentinel.as_path(), "the {what} must be disabled like a mask path: {got}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn depth_protect_and_layer_paths_round_trip_relative_like_a_mask_path() {
    let base = std::env::temp_dir().join(format!("autoshade-store-test-rasters-rel-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let abs = |n: &str| base.join(n).to_string_lossy().into_owned();
    let mut r = recipe_with_rasters(&abs("depth.png"), &abs("protect.png"), &abs("layer.png"));
    relativize_mask_paths(&mut r, &base);
    assert_eq!(raster_refs(&mut r), ("depth.png".into(), "protect.png".into(), "layer.png".into()), "stored bare: the develop dir is relocatable");
    resolve_mask_paths(&mut r, &base);
    assert_eq!(raster_refs(&mut r), (abs("depth.png"), abs("protect.png"), abs("layer.png")), "anchored again on load");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_version_snapshot_freezes_depth_protect_and_layer_rasters() {
    let dev = std::env::temp_dir().join(format!("autoshade-store-test-rasters-freeze-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    for n in ["depth.png", "protect.png", "layer.png"] {
        std::fs::write(dev.join(n), n.as_bytes()).unwrap();
    }
    let mut r = recipe_with_rasters("depth.png", "protect.png", "layer.png");
    snapshot_rasters(&mut r, &dev, 7).unwrap();
    assert_eq!(raster_refs(&mut r), ("v7.depth.png".into(), "v7.protect.png".into(), "v7.layer.png".into()));
    for n in ["depth.png", "protect.png", "layer.png"] {
        assert_eq!(std::fs::read(dev.join(format!("v7.{n}"))).unwrap(), n.as_bytes(), "{n} frozen");
    }
    let _ = std::fs::remove_dir_all(&dev);
}
