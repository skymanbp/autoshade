//! Central per-user develop store — WHERE a photo's develop state lives.
//!
//! Until v0.12.0 every sidecar (recipe.json / .xmp / v<N> snapshots / mask
//! rasters) was keyed by the photo's bare file STEM inside a cwd-relative
//! `./out`. Two same-named photos in different folders therefore shared one
//! sidecar (silent cross-clobber), and launching the app from a different
//! directory hid every existing edit. This module fixes both by giving each
//! photo its own directory under a per-user root, keyed by the photo's
//! ABSOLUTE path:
//!
//! ```text
//! <root>/develops/<stem>-<fnv1a64(abs path)>/
//!     recipe.json          the working develop (single source of truth)
//!     <stem>.xmp           Lightroom projection (name kept: LR needs <stem>.xmp)
//!     v<N>.recipe.json     numbered snapshots
//!     <kind>.png           mask rasters (mask-sky, mask-zone-sky, …)
//!     pixels.json          baked pixel-master link (retouch/reimagine origin)
//!     variants.json        GUI variant strip (background variants + active kind)
//!     source.txt           breadcrumb: which photo this dir belongs to
//! ```
//!
//! The root resolves at runtime (never hardcoded): `AUTOSHADE_DATA_DIR` env
//! override → `%LOCALAPPDATA%/autoshade` (the thumb cache already lives there)
//! → the system temp dir. EXPORTS (developed/retouch/heal/… images) are user
//! deliverables and deliberately STAY in ./out.
//!
//! Mask rasters are referenced from recipe.json by a path string. Inside the
//! store that string is the BARE file name, resolved against the recipe's own
//! directory at load time ([`resolve_mask_paths`]) and relativized back at
//! write time ([`relativize_mask_paths`]) — so a develop dir is relocatable
//! and legacy cwd-relative "out/…" references keep working unchanged.

use std::{
    cell::RefCell,
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

// Bitmap raster paths are walked through `LocalAdjustment::bitmap_paths_mut`
// (base geometry + components in one place), so this module no longer
// pattern-matches `MaskGeometry` directly.
use crate::recipe::EditRecipe;
mod backup;
mod clear;
mod commit;
mod durable;
mod key;
mod lock;
mod migrate;
mod paths;
mod root;
mod sidecar;
mod snapshot;
mod variants;
mod versions;

pub use backup::{
    backup_saved_develop, claim_version, delete_version, rollback_frozen_rasters, snapshot_rasters,
};
use backup::{deleting_marker, read_deleted_versions, register_deleted_version};
#[cfg(test)]
use backup::{DeletedVersion, DeletedVersions, deleted_versions_path};
pub use clear::{
    ClearOutcome, clear_develop, list_versions, mark_develop_cleared, relativize_mask_paths,
    resolve_mask_paths,
};
use clear::{clear_sweep, contained_join, remote_or_device_path};
#[cfg(test)]
use clear::{clear_develop_in, unc_or_device_spelling};
pub use commit::{DevelopCommit, commit_develop};
use commit::{commit_dir_in, resolve_pending_commit_unlocked};
pub use durable::{durable_adopt};
pub(crate) use durable::{
    durable_rename, durable_replace, durable_retire_and_write, durable_write, next_tmp_seq,
};
use durable::{
    copy_atomic, durable_os, durable_write_tracked, move_file_no_clobber, publish_no_clobber,
    sibling_tmp, sync_staged, write_staged,
};
#[cfg(test)]
use durable::{publish_no_clobber_with};
pub use key::{AliasNote, photo_key, take_alias_note, volume_folds_case};
pub(crate) use key::{photo_key_lexical};
use key::{forget_resolved_key, resolve_key_in, resume_marked_adoption};
#[cfg(test)]
use key::{
    DEFAULT_CASE_FOLD, adopt_or_choose, adoption_skips, dir_folds_case, key_from_spelling_folded,
    resolved_keys, strip_verbatim,
};
pub use lock::{DevelopLockMode, with_develop_lock, with_settings_lock};
use lock::{with_develop_lock_in, with_path_lock};
#[cfg(test)]
use lock::{with_settings_lock_in};
pub use migrate::{migrate_legacy, migrate_legacy_from_many};
#[cfg(test)]
use migrate::{migrate_legacy_in};
pub use paths::{
    OwnedRaster, claim_raster, clear_pixel_source, detach_rasters, develop_dir, export_xmp_beside,
    pixel_source_path, pixel_source_record_bytes, raster_target, recipe_target, saved_quarter_turns,
    variants_path, version_target, write_pixel_source, xmp_beside_target, xmp_target,
};
use paths::{
    clear_pixel_source_unlocked_in, develop_dir_in, pixel_source_path_in, publish_json_sidecar,
    recipe_target_in, variants_path_in, xmp_target_in,
};
#[cfg(test)]
use paths::{SIDECAR_STAGE_GRACE, is_within, sweep_stale_sidecar_stages};
pub use root::{RootAdoption, RootTrust, store_root, store_root_with_trust};
use root::{FNV_OFFSET, fnv1a64, fnv1a64_update};
#[cfg(test)]
pub(crate) use root::{LEGACY_STORE_DIR_NAME, STORE_DIR_NAME, adopt_pre_rename_root};
#[cfg(test)]
use root::{ADOPT_PRE_RENAME};
pub use sidecar::{
    LrSidecar, MAX_STORE_JSON, SidecarRead, SidecarStamp, embedded_packet_for_restore, has_develop,
    has_develop_or_sidecar, legacy_recipe, legacy_xmp, lightroom_sidecar, read_bytes_capped,
    read_sidecar, read_sidecar_checked, read_sidecar_stamped, read_text_capped, settings_path,
};
use sidecar::{
    clear_pending, clear_pending_in, legacy_out_roots, legacy_suppressed, legacy_suppressed_in,
    suppress_legacy_in,
};
#[cfg(test)]
use sidecar::{
    LEGACY_TOMBSTONE, legacy_recipe_in, rank_lightroom_sidecar, suppressed_legacy_path_in,
};
pub use snapshot::{
    DevelopSnapshot, develop_revision, develop_revision_of, has_pixel_source, read_develop_snapshot,
    read_pixel_source, recipe_revision, recorded_pixel_source, recover_orphan_baks,
    render_source_checked,
};
use snapshot::{
    has_pixel_source_in, recover_orphan_baks_unlocked, resolve_pending_clear_unlocked,
    settle_consumed_marker,
};
#[cfg(test)]
use snapshot::{revision_of};
pub use variants::{
    ActiveWrite, CommitMember, MAX_STORE_NAME, VariantEntry, VariantsRead, VariantsRecord,
    clear_variants, read_variants, read_variants_checked, variants_member, variants_record_bytes,
    write_variants,
};
use variants::{capped_name, refuse_unresolved_strip};
pub use versions::{
    EditState, EditStateKind, VERSION_ORIGIN_AUTO, VERSION_ORIGIN_USER, VersionMetaEntry,
    list_edits, note_source, read_version_meta, record_version_meta, set_version_name,
};
use versions::{
    delete_version_unlocked, record_version_meta_unlocked, recover_pending_version_deletes_unlocked,
};
#[cfg(test)]
use versions::{version_meta_path};

/// The store's source as ONE text, for the source-text gates that read what
/// `store.rs` alone held before it was split into files: the modules in their
/// declaration order, the root last (so `source_before_tests` cuts at the root's
/// own test modules and nowhere else). Test-only: nothing here is compiled into a
/// shipped binary.
#[cfg(test)]
pub(crate) const SOURCE_FILES: [(&str, &str); 14] = [
    ("src/store/backup.rs", include_str!("store/backup.rs")),
    ("src/store/clear.rs", include_str!("store/clear.rs")),
    ("src/store/commit.rs", include_str!("store/commit.rs")),
    ("src/store/durable.rs", include_str!("store/durable.rs")),
    ("src/store/key.rs", include_str!("store/key.rs")),
    ("src/store/lock.rs", include_str!("store/lock.rs")),
    ("src/store/migrate.rs", include_str!("store/migrate.rs")),
    ("src/store/paths.rs", include_str!("store/paths.rs")),
    ("src/store/root.rs", include_str!("store/root.rs")),
    ("src/store/sidecar.rs", include_str!("store/sidecar.rs")),
    ("src/store/snapshot.rs", include_str!("store/snapshot.rs")),
    ("src/store/variants.rs", include_str!("store/variants.rs")),
    ("src/store/versions.rs", include_str!("store/versions.rs")),
    ("src/store.rs", include_str!("store.rs")),
];

#[cfg(test)]
pub(crate) fn source_text() -> String {
    SOURCE_FILES.iter().map(|(_, text)| *text).collect()
}
/// Two spellings of the same photo must land in the same develop dir on a
/// case-insensitive volume — and the folded name must actually RESOLVE to any
/// directory an earlier build created.
///
/// The FOLD ITSELF is checked on every host, both ways round, by driving
/// [`key_from_spelling_folded`] directly: a case-insensitive volume owes one
/// key for two spellings, a case-sensitive one owes two. Only the WIRING —
/// which of the two `photo_key` is actually given — is `cfg`-gated, and that
/// gate is two asserts wide. This test used to assert the folding half
/// UNCONDITIONALLY and so failed on both Unix runners of CI run 32398395462;
/// the fix is not to weaken it but to stop calling one platform's rule
/// universal.
#[cfg(test)]
#[test]
fn the_stem_fold_never_invents_a_name_ntfs_cannot_resolve() {
    let (upper, lower) = (Path::new("D:/p/DSC001.ARW"), Path::new("D:/p/dsc001.arw"));
    // ASCII case folds (the reason the fold exists): on a case-insensitive
    // volume these two spellings open ONE file, so they owe one develop dir.
    assert_eq!(
        key_from_spelling_folded(upper, true),
        key_from_spelling_folded(lower, true),
        "a folding host owes one key for two spellings of one file"
    );
    // And where paths are case-SENSITIVE the same two spellings are two
    // FILES. Folding them would hand two different photos one develop dir and
    // one develop lock, so each save would overwrite the other's recipe —
    // distinct keys is the correct answer there, not a missing feature.
    assert_ne!(
        key_from_spelling_folded(upper, false),
        key_from_spelling_folded(lower, false),
        "a case-sensitive host must not merge two photos into one develop"
    );
    // The wiring: `photo_key` folds exactly where the platform guarantees
    // case-insensitive paths for every local volume, and NOT per volume — see
    // its doc comment for why a key may not depend on a probe that can fail.
    if cfg!(windows) {
        assert_eq!(photo_key(upper), photo_key(lower), "NTFS: one file, one key");
    } else {
        assert_ne!(photo_key(upper), photo_key(lower), "case-sensitive host: two files, two keys");
    }
    // Non-ASCII stems are left ALONE even on the folding branch: Rust's full
    // lowercase maps these to names NTFS does not consider equal to the
    // original, so folding them would point at a directory that does not
    // exist. Driven through `folded = true` so the check runs everywhere.
    for stem in ["\u{130}MG_001", "\u{3a3}\u{391}\u{3a3}", "\u{1e9e}"] {
        let p = format!("D:/p/{stem}.ARW");
        let key = key_from_spelling_folded(Path::new(&p), true);
        let folded = key.rsplit_once('-').expect("key is <stem>-<hash>").0;
        // Each fixture is a case where Rust's full lowercase and NTFS's own
        // folding disagree — that is what makes it a fixture at all.
        assert_ne!(
            stem.to_lowercase(),
            stem.to_ascii_lowercase(),
            "fixture must be a divergence case"
        );
        // The guarantee: no character EXPANDS (Rust maps U+0130 to two chars,
        // which can never name the same NTFS directory), and only ASCII
        // letters change — the subset NTFS folds identically.
        assert_eq!(folded.chars().count(), stem.chars().count(), "no expansion: {folded}");
        assert_eq!(folded, stem.to_ascii_lowercase(), "only ASCII letters fold: {folded}");
    }
}

/// [`volume_folds_case`] answers what the VOLUME does, not what the operating
/// system usually does.
///
/// The ground truth is taken from the filesystem in the same breath — write one
/// file, then ask whether the case-flipped spelling opens — so the test proves
/// the probe on whatever volume the temp directory happens to live on, which is
/// the entire point of replacing `cfg!(windows)` with a probe.
///
/// MUTATION: make `dir_folds_case` return `Some(false)` and this fails on every
/// case-insensitive volume (each Windows runner, and a default-APFS Mac).
#[cfg(test)]
#[test]
fn the_case_rule_is_read_off_the_volume_not_off_the_platform() {
    let dir = std::env::temp_dir()
        .join(format!("autoshade-case-probe-{}-{}", std::process::id(), next_tmp_seq()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Ab.txt"), b"x").unwrap();
    let truth = std::fs::metadata(dir.join("aB.txt")).is_ok();
    assert_eq!(
        volume_folds_case(&dir.join("Ab.txt")),
        truth,
        "the probe disagreed with the volume it was probing"
    );
    // The leaf a caller actually hands in usually does NOT exist yet (a first
    // `-o`, a recipe not written), so the probe is taken at the deepest
    // EXISTING ancestor and must answer the same.
    assert_eq!(volume_folds_case(&dir.join("not-written-yet.json")), truth);
    // Nothing to probe: the platform rule, stated once and only once.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        volume_folds_case(&empty.join("photo.arw")),
        DEFAULT_CASE_FOLD,
        "an unprobeable directory must fall back to the platform rule"
    );
    assert_eq!(
        DEFAULT_CASE_FOLD,
        cfg!(windows) || cfg!(target_os = "macos"),
        "the platform rule: Windows and default APFS fold, ordinary Linux does not"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The probe may never call a case-SENSITIVE volume a folding one.
///
/// It asks whether the flipped spelling names the SAME OBJECT, not whether
/// something answers to it, and that distinction is the one that costs data: a
/// case-sensitive directory can hold both spellings as two separate files, and
/// calling it "folds" would hand two photos one develop dir and one develop
/// lock. Both outcomes are asserted from the same fixture — where the second
/// spelling can be CREATED there are two files and the answer must be "does not
/// fold", and where creating it collides there is one file and the answer must
/// be "folds".
///
/// MUTATION: replace `same_object` with a bare `b.exists()` and a
/// case-sensitive runner reports folding for a directory holding two files.
#[cfg(test)]
#[test]
fn two_spellings_are_only_one_name_when_they_are_one_object() {
    let dir = std::env::temp_dir()
        .join(format!("autoshade-case-object-{}-{}", std::process::id(), next_tmp_seq()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cd.txt"), b"first").unwrap();
    let second = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join("cD.txt"));
    match second {
        Ok(_) => assert_eq!(
            dir_folds_case(&dir),
            Some(false),
            "two files were created, so this volume tells them apart"
        ),
        Err(e) => {
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::AlreadyExists,
                "the fixture must fail by collision, not by anything else: {e}"
            );
            assert_eq!(
                dir_folds_case(&dir),
                Some(true),
                "the second spelling collided with the first, so this volume folds"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
#[cfg(test)]
mod ownership_tests {
    use super::*;

    /// The guard that turns the 2026-08-25 corpus loss from unlikely into
    /// impossible: the zoned fit's deleting call sites are reachable only
    /// through [`OwnedRaster`], and no corpus path can become one.
    #[test]
    fn a_calibration_corpus_path_can_never_become_an_owned_raster() {
        let dir = std::env::temp_dir().join(format!("autoshade-ownership-guard-{}", std::process::id()));
        assert!(is_within(&dir.join("sky-mask.png"), &dir), "a file inside the corpus is inside it");
        assert!(is_within(&dir.join("sub").join("m.png"), &dir), "containment is not depth-limited");
        let sibling = dir.with_file_name("autoshade-ownership-guard-other");
        assert!(
            !is_within(&sibling.join("sky-mask.png"), &dir),
            "a sibling directory sharing a name PREFIX is not inside the corpus"
        );

        // And against the real corpus, when one is configured for this run.
        let Some(root) = crate::config::live_env("AUTOSHADE_FIT_CALIBRATION_DIR") else { return };
        let mask = PathBuf::from(&root).join("sky-mask.png");
        if !mask.exists() {
            return;
        }
        let outcome = std::panic::catch_unwind(|| OwnedRaster::scratch(mask.clone()));
        assert!(outcome.is_err(), "the corpus mask was accepted as an owned raster");
        assert!(mask.exists(), "the refusal must leave the corpus untouched");
    }

    /// `remove` is the only deletion path, and it refuses the corpus too --
    /// belt as well as braces, because a future constructor could reintroduce
    /// what `scratch` currently rejects.
    #[test]
    fn owned_raster_remove_deletes_only_what_it_owns() {
        let scratch = std::env::temp_dir()
            .join(format!("autoshade-owned-remove-{}.png", std::process::id()));
        std::fs::write(&scratch, b"x").unwrap();
        let owned = OwnedRaster::scratch(scratch.clone());
        assert_eq!(owned.path(), scratch.as_path());
        owned.remove();
        assert!(!scratch.exists(), "remove must delete the file it owns");
    }
}

#[cfg(test)]
mod tests;

