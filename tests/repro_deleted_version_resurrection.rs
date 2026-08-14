//! Regression contract for the 2026-08-13 report (deleted version snapshots
//! came back after a restart). Root cause, one class, two arms: the backup
//! gate's "already preserved" dedup witness WAS the deletable snapshot
//! itself, and `claim_version` re-issued a freed number (max+1) — so the
//! next gated programmatic write (analyze persist, reverse-fit, paste,
//! batch, serve, CLI — and the legacy ./out migration, the third arm) re-
//! created the version the user had just deleted, wearing the very `v<n>`
//! label the GUI lists it by. The fix (user decision 2026-08-13): a delete
//! registers the number + content fingerprints in the develop's permanent
//! `.deleted-versions.json` — the number is never re-issued, and content
//! the user explicitly discarded is never AUTO-preserved again (an
//! explicit 「＋ Save as version」 still saves anything).
//!
//! Isolation: run with AUTOSHOP_DATA_DIR pointing at a scratch store; the
//! fake source paths also hash to their own develop dirs (the store-test
//! pattern), scrubbed before and after.

use autoshop::recipe::EditRecipe;

/// Arm 1 — the Lightroom-sidecar half (`snapshot_xmp_text`). Before the fix
/// this resurrected the deleted snapshot BYTE-FOR-BYTE under the recycled
/// number on the next gated write.
#[test]
fn a_deleted_sidecar_snapshot_stays_deleted() {
    let dir = std::env::temp_dir().join("autoshop-repro-xmp-resurrection");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_repro_resurrection.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = autoshop::store::develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    let lr_text =
        autoshop::xmp::recipe_to_xmp(&EditRecipe { contrast: 12.0, ..Default::default() });
    std::fs::write(raw.with_extension("xmp"), &lr_text).unwrap();

    // Any gated programmatic write preserves the sidecar as v1, and the
    // dedup holds while the snapshot lives.
    assert_eq!(autoshop::store::backup_saved_develop(&raw, None).unwrap(), Some(1));
    assert_eq!(autoshop::store::backup_saved_develop(&raw, None).unwrap(), None);

    // The GUI 🗑: the version is gone, its registry entry stays.
    autoshop::store::delete_version(&raw, 1).unwrap();
    assert!(autoshop::store::list_versions(&raw).is_empty());
    let stem = autoshop::pipeline::stem(&raw).to_string();
    assert!(!dev.join(format!("v1.{stem}.xmp")).exists());
    assert!(dev.join(".deleted-versions.json").exists());

    // The next gated write no longer resurrects the discarded bytes...
    assert_eq!(
        autoshop::store::backup_saved_develop(&raw, None).unwrap(),
        None,
        "explicitly discarded sidecar content stays discarded"
    );
    assert!(autoshop::store::list_versions(&raw).is_empty());

    // ...while GENUINELY NEW sidecar content is still preserved — under a
    // fresh number, never the deleted label.
    let lr2 =
        autoshop::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() });
    std::fs::write(raw.with_extension("xmp"), &lr2).unwrap();
    assert_eq!(autoshop::store::backup_saved_develop(&raw, None).unwrap(), Some(2));
    assert_eq!(autoshop::store::list_versions(&raw), vec![2]);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// Arm 2 — the store half (`backup_store_half_unlocked`). Before the fix the
/// next gated overwrite re-claimed the freed number for its new snapshot;
/// the versions panel lists bare numbers, so "v1" read as the delete never
/// having worked.
#[test]
fn a_deleted_version_number_is_never_reissued() {
    let dir = std::env::temp_dir().join("autoshop-repro-number-recycling");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("_repro_recycle.arw");
    std::fs::write(&raw, b"raw").unwrap();
    let dev = autoshop::store::develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();

    let a = EditRecipe { exposure_ev: 0.1, ..Default::default() };
    let b = EditRecipe { exposure_ev: 0.2, ..Default::default() };
    let c = EditRecipe { exposure_ev: 0.3, ..Default::default() };
    let write_save = |r: &EditRecipe| {
        std::fs::write(
            autoshop::store::recipe_target(&raw),
            serde_json::to_string_pretty(r).unwrap(),
        )
        .unwrap();
    };
    write_save(&a);

    // First programmatic overwrite: the existing save A is preserved as v1,
    // then the caller lands its own write (B).
    assert_eq!(autoshop::store::backup_saved_develop(&raw, Some(&b)).unwrap(), Some(1));
    write_save(&b);

    // The GUI 🗑 on v1: the next gate snapshot takes a FRESH number.
    autoshop::store::delete_version(&raw, 1).unwrap();
    assert!(autoshop::store::list_versions(&raw).is_empty());
    assert_eq!(
        autoshop::store::backup_saved_develop(&raw, Some(&c)).unwrap(),
        Some(2),
        "a new snapshot never wears a deleted label"
    );

    // Deleting THAT snapshot registers its content (the save B) as
    // discarded: neither gate arm auto-preserves it again.
    autoshop::store::delete_version(&raw, 2).unwrap();
    assert_eq!(autoshop::store::backup_saved_develop(&raw, None).unwrap(), None);
    assert_eq!(autoshop::store::backup_saved_develop(&raw, Some(&c)).unwrap(), None);
    assert!(autoshop::store::list_versions(&raw).is_empty());

    // The registry survives clear_develop exactly like versions do, so
    // even a cleared develop never re-issues a burned number.
    autoshop::store::clear_develop(&raw).unwrap();
    assert!(dev.join(".deleted-versions.json").exists());
    let (n, _) = autoshop::store::claim_version(&raw).unwrap();
    assert_eq!(n, 3);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}
