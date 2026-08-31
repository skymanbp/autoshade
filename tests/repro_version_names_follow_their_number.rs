//! Contract for R24-2's version NAMES (`.version-meta.json`).
//!
//! A name belongs to a NUMBER, not to content: `v3` is 「日落偏暖」 because
//! the user said so about *that snapshot*. Two properties follow, and both
//! are load-bearing enough to pin here rather than in a unit test:
//!
//!   1. deleting v3 drops the name WITH it, and — because the number is
//!      burned in `.deleted-versions.json` and never re-issued (R21) — no
//!      later snapshot can ever inherit it. The failure mode this forbids is
//!      the version-resurrection bug's cosmetic twin: a fresh snapshot
//!      wearing a deleted version's name;
//!   2. the sidecar follows the NON-GENERATIONAL discipline
//!      `.deleted-versions.json` established: no `.bak`, no commit
//!      membership, no pairing table, and NOT swept by `clear_develop` —
//!      which leaves the versions themselves standing, so taking their names
//!      would leave every surviving snapshot anonymous.
//!
//! Isolation follows the store-test pattern of
//! `repro_deleted_version_resurrection.rs`: fake source paths hash to their
//! own develop dirs, scrubbed before and after.

use autoshade::recipe::EditRecipe;

fn scratch(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("autoshade-repro-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join(format!("_repro_{tag}.arw"));
    std::fs::write(&raw, b"raw").unwrap();
    let dev = autoshade::store::develop_dir(&raw);
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    (dir, raw, dev)
}

fn write_save(raw: &std::path::Path, r: &EditRecipe) {
    std::fs::write(autoshade::store::recipe_target(raw), serde_json::to_string_pretty(r).unwrap())
        .unwrap();
}

fn name_of(raw: &std::path::Path, n: u32) -> Option<String> {
    autoshade::store::read_version_meta(raw).into_iter().find(|e| e.n == n).and_then(|e| e.name)
}

/// Property 1 — the name dies with the number, and the burned number can
/// never bring it back.
#[test]
fn a_version_name_dies_with_its_number_and_is_never_inherited() {
    let (dir, raw, dev) = scratch("version-name-lifetime");

    let a = EditRecipe { exposure_ev: 0.1, ..Default::default() };
    let b = EditRecipe { exposure_ev: 0.2, ..Default::default() };
    let c = EditRecipe { exposure_ev: 0.3, ..Default::default() };
    write_save(&raw, &a);

    // The AUTOMATIC arm preserves A as v1 and stamps its own provenance —
    // "auto" is what makes the「· 自动存档」 row honest about who took it.
    assert_eq!(autoshade::store::backup_saved_develop(&raw, Some(&b)).unwrap(), Some(1));
    write_save(&raw, &b);
    let v1 = autoshade::store::read_version_meta(&raw)
        .into_iter()
        .find(|e| e.n == 1)
        .expect("the gate records the snapshot it took");
    assert_eq!(v1.origin.as_deref(), Some(autoshade::store::VERSION_ORIGIN_AUTO));
    assert_eq!(v1.name, None, "an automatic snapshot is unnamed until the user names it");

    // Naming MERGES: the provenance the gate stamped must survive a rename.
    autoshade::store::set_version_name(&raw, 1, Some("日落偏暖")).unwrap();
    assert_eq!(name_of(&raw, 1).as_deref(), Some("日落偏暖"));
    assert_eq!(
        autoshade::store::read_version_meta(&raw)[0].origin.as_deref(),
        Some(autoshade::store::VERSION_ORIGIN_AUTO),
        "renaming a version must not erase where it came from"
    );

    // The 🗑: the snapshot goes, and the name goes with it.
    autoshade::store::delete_version(&raw, 1).unwrap();
    assert!(autoshade::store::list_versions(&raw).is_empty());
    assert!(
        autoshade::store::read_version_meta(&raw).is_empty(),
        "a deleted version's name must not outlive it"
    );

    // The next snapshot takes a FRESH number (R21's burned high-water mark)
    // and arrives ANONYMOUS — the resurrection bug's cosmetic twin would be
    // v2 (or a recycled v1) wearing 「日落偏暖」.
    assert_eq!(autoshade::store::backup_saved_develop(&raw, Some(&c)).unwrap(), Some(2));
    assert_eq!(autoshade::store::list_versions(&raw), vec![2]);
    assert_eq!(name_of(&raw, 2), None, "a new snapshot never inherits a deleted version's name");

    // An explicit record for the same number carries the variant it came
    // from; a later rename still merges onto it rather than replacing it.
    autoshade::store::record_version_meta(
        &raw,
        &autoshade::store::VersionMetaEntry {
            n: 2,
            from_kind: Some("fitted".into()),
            from_id: Some("card-7".into()),
            origin: Some(autoshade::store::VERSION_ORIGIN_USER.into()),
            ..Default::default()
        },
    )
    .unwrap();
    autoshade::store::set_version_name(&raw, 2, Some("  暖调  ")).unwrap();
    let v2 = autoshade::store::read_version_meta(&raw).into_iter().find(|e| e.n == 2).unwrap();
    assert_eq!(v2.name.as_deref(), Some("暖调"), "a name is trimmed, not stored with its padding");
    assert_eq!(v2.from_kind.as_deref(), Some("fitted"));
    assert_eq!(v2.from_id.as_deref(), Some("card-7"));
    assert_eq!(v2.origin.as_deref(), Some(autoshade::store::VERSION_ORIGIN_USER));

    // Clearing a name leaves the provenance standing (the two are separate
    // facts, and only one of them is the user's to type).
    autoshade::store::set_version_name(&raw, 2, None).unwrap();
    let v2 = autoshade::store::read_version_meta(&raw).into_iter().find(|e| e.n == 2).unwrap();
    assert_eq!(v2.name, None);
    assert_eq!(v2.from_id.as_deref(), Some("card-7"));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}

/// Property 2 — the sidecar is advisory and NON-GENERATIONAL: no retired
/// `.bak` beside it, and `clear_develop` leaves it standing exactly like the
/// version snapshots it names.
#[test]
fn the_version_name_sidecar_is_advisory_and_non_generational() {
    let (dir, raw, dev) = scratch("version-name-discipline");
    let meta = dev.join(".version-meta.json");

    write_save(&raw, &EditRecipe { exposure_ev: 0.4, ..Default::default() });
    assert_eq!(
        autoshade::store::backup_saved_develop(
            &raw,
            Some(&EditRecipe { exposure_ev: 0.5, ..Default::default() })
        )
        .unwrap(),
        Some(1)
    );
    autoshade::store::set_version_name(&raw, 1, Some("keep me")).unwrap();
    assert!(meta.exists());

    // NO retired generation: `publish_json_sidecar` (recipe.json,
    // pixels.json, variants.json) retires the previous file to `<name>.bak`
    // and `recover_orphan_baks` republishes it whenever the live file is
    // missing. An advisory file must not join that pair list — a republished
    // `.bak` would resurrect names the live file has since dropped.
    autoshade::store::set_version_name(&raw, 1, Some("renamed once")).unwrap();
    assert!(
        !dev.join(".version-meta.json.bak").exists(),
        "the advisory sidecar must never leave a retired generation behind"
    );
    autoshade::store::recover_orphan_baks(&raw).unwrap();
    assert_eq!(name_of(&raw, 1).as_deref(), Some("renamed once"));

    // clear_develop wipes the WORKING develop (recipe/xmp/pixels/variants)
    // but deliberately leaves the version snapshots — so it must leave their
    // names too, or every surviving version comes back anonymous.
    autoshade::store::clear_develop(&raw).unwrap();
    assert_eq!(
        autoshade::store::list_versions(&raw),
        vec![1],
        "clear_develop leaves version snapshots standing (the premise of this assertion)"
    );
    assert!(meta.exists(), "the names must survive a clear exactly like the versions do");
    assert_eq!(name_of(&raw, 1).as_deref(), Some("renamed once"));

    // Corrupt bytes degrade to "no names" — never to a refusal: a develop
    // whose LABELS are unreadable must still save, load and delete.
    std::fs::write(&meta, b"not json at all").unwrap();
    assert!(autoshade::store::read_version_meta(&raw).is_empty());
    autoshade::store::set_version_name(&raw, 1, Some("after the corruption")).unwrap();
    assert_eq!(name_of(&raw, 1).as_deref(), Some("after the corruption"));
    autoshade::store::delete_version(&raw, 1).unwrap();
    assert!(autoshade::store::list_versions(&raw).is_empty());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dev);
}
