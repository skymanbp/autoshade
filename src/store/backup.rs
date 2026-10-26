//! Version backups: claiming a version, the frozen rasters, deletion and its register.

use super::*;

/// Before a PROGRAMMATIC writer (AI Analyze, reverse-fit — any surface)
/// replaces an existing saved develop, snapshot it to the next
/// `v<N>.recipe.json` so an explicit save can never be silently destroyed.
/// Explicit user saves overwrite without backup — the user asked for that one.
///
/// The snapshot is a COPY (the working recipe.json stays in place), so callers
/// may take it BEFORE running the operation that will overwrite — required for
/// the zoned reverse-fit, whose segmentation rewrites `mask-zone-sky.png`
/// before any recipe write happens. Rasters referenced by the snapshot are
/// versioned along (`v<N>.<name>.png`) for the same reason: a shared raster
/// mutated later must not silently change what an old snapshot renders.
///
/// `incoming = Some(r)`: skip when the existing save equals `r` (no snapshot
/// spam on identical rewrites). `incoming = None`: snapshot unconditionally
/// (callers that run before their result exists, e.g. the fits).
///
/// `Ok(Some(n))` = snapshotted as v<n>; `Ok(None)` = nothing to snapshot;
/// `Err` = an existing save COULD NOT be snapshotted — the caller must then
/// leave it untouched, because overwriting without the promised backup is
/// exactly the silent destruction this function exists to prevent.
///
/// ONE deliberate exception to that promise (user decision 2026-08-13):
/// content whose version snapshot the user explicitly DELETED answers
/// `Ok(None)` too — the caller's write then replaces it with no surviving
/// copy. Deleting a snapshot means "stop preserving this"; without the
/// exception the very next gated write re-created the deleted version
/// verbatim. The match is fingerprint-exact (bytes, or structure + raster
/// bytes — see [`DeletedVersions`]), and an explicit 「＋ Save as version」
/// remains ungated.
pub fn backup_saved_develop(
    src: &Path,
    incoming: Option<&EditRecipe>,
) -> std::io::Result<Option<u32>> {
    with_develop_lock(src, DevelopLockMode::Wait, || {
        backup_saved_develop_unlocked(src, incoming)
    })
}

fn backup_saved_develop_unlocked(
    src: &Path,
    incoming: Option<&EditRecipe>,
) -> std::io::Result<Option<u32>> {
    // A crashed publish's survivor is a save like any other — restore it
    // before deciding what to snapshot, or the write below destroys it. An
    // orphan we could NOT restore is an existing save we cannot see: refusing
    // beats overwriting it unversioned, the same stance as an unreadable
    // recipe below.
    recover_orphan_baks(src)?;
    // L13#3: the newest intent may be LIGHTROOM'S OWN sidecar beside the
    // RAW — the one develop every restore surface prefers when it out-ranks
    // the store, and the one this gate never snapshotted: the programmatic
    // write it gates is paired with an XMP write that destroys it. Read the
    // ranking ONCE; snapshot the store develop FIRST and the sidecar SECOND,
    // so version numbers encode intent order (higher n = newer intent).
    let lr_intent = match lightroom_sidecar(src) {
        LrSidecar::Only(t) | LrSidecar::NewerThanStore(t)
            if !crate::xmp::xmp_to_recipe_for_photo(&t, src).is_noop() =>
        {
            Some(t)
        }
        LrSidecar::Unreadable(why) => {
            // Unreadable is not absent — but a text we cannot read cannot be
            // snapshotted either. Disclosed; the sidecar file itself stays
            // untouched beside the RAW (this gate never writes there).
            eprintln!(
                "⚠ {}: a Lightroom sidecar sits beside this photo but could not be read ({why}) — it is NOT snapshotted; the file itself stays untouched beside the RAW",
                crate::pipeline::stem(src)
            );
            None
        }
        _ => None,
    };
    let store_n = backup_store_half_unlocked(src, incoming)?;
    match lr_intent {
        // The is_noop-guarded sidecar snapshot: dedup against the latest
        // version's xmp bytes lives inside snapshot_xmp_text, so repeated
        // programmatic writes do not spam versions.
        Some(t) => Ok(snapshot_xmp_text(src, t)?.or(store_n)),
        None => Ok(store_n),
    }
}

/// The STORE half of [`backup_saved_develop`]: central recipe.json first,
/// then a not-yet-migrated LEGACY recipe — the read fallbacks restore
/// either, so overwriting the central slot unversioned while a legacy
/// develop still answered was silent destruction too.
fn backup_store_half_unlocked(
    src: &Path,
    incoming: Option<&EditRecipe>,
) -> std::io::Result<Option<u32>> {
    let mut found: Option<(PathBuf, String)> = None;
    for rj in [recipe_target(src), legacy_recipe(src)] {
        match read_text_capped(&rj, MAX_STORE_JSON) {
            Ok(t) => {
                found = Some((rj, t));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Unreadable ≠ absent (lock/permissions): refusing beats
            // overwriting a save we could not even look at.
            Err(e) => return Err(e),
        }
    }
    let Some((rj, text)) = found else {
        // No recipe.json anywhere — but an XMP-ONLY develop (a Lightroom-
        // authored sidecar, or a pre-recipe-era save) is STILL a save every
        // reader honours (has_develop, the GUI/web restore fallbacks), and
        // the programmatic write this call gates is paired with an XMP write
        // that would destroy it. Snapshot it too.
        return backup_xmp_only(src);
    };
    let parsed = serde_json::from_str::<EditRecipe>(&text).ok();
    // A NEUTRAL recipe.json IS what the readers restore since 2026-09-13
    // (the GUI's `SavedDevelop::NoopOnly`, the web's `api_recipe` and the
    // batch resolver all end their walk at it): the projection beside it is
    // derived from it — written or cleared in the same commit generation —
    // so a projection holding edits can only be stale, and preserving that
    // as a version would resurrect a develop the user had already replaced.
    // The neutral snapshot below is the whole of what such a save holds.
    if let (Some(existing), Some(inc)) = (&parsed, incoming) {
        // The on-disk copy names rasters by bare file name while `incoming`
        // carries absolute paths — resolve before comparing, or every
        // raster-bearing rewrite would look "different" and snapshot itself.
        let mut existing = existing.clone();
        if let Some(base) = rj.parent() {
            resolve_mask_paths(&mut existing, base);
        }
        if existing == *inc {
            return Ok(None); // rewriting the same content needs no snapshot
        }
    }
    let dev = develop_dir(src);
    // Content the user explicitly DISCARDED is not re-preserved (the
    // deleted-version registry, 2026-08-13): the gate's only "already
    // preserved" witness used to be the deletable snapshot itself, so a
    // gate run whose existing save matched a just-deleted version
    // re-created it — same content, freshly claimed number — on the very
    // next programmatic write. NB this Ok(None) means the caller's write
    // then replaces the save with NO surviving copy — the delete said so
    // (see the [`DeletedVersions`] contract and the gate doc above).
    if discarded_recipe_matches(&dev, &text, parsed.as_ref()) {
        return Ok(None);
    }
    // Atomically RESERVED number (see claim_version): the old list+1 pick
    // let two processes select the same n and silently replace each other's
    // snapshot; the claim also embeds the vMAX refusal.
    let (n, dst) = claim_version(src)?;
    // Per-process AND per-call tmp name from the ONE shared counter (see
    // next_tmp_seq): GUI + web server are separate PROCESSES backing up the
    // same photo, and per-SITE counters let two same-process writers mint
    // the identical name.
    let tmp = dst.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        next_tmp_seq()
    ));
    match parsed {
        Some(mut r) => {
            // A raster that cannot be frozen fails the WHOLE backup: returning
            // Ok would let the caller overwrite that raster believing a
            // faithful snapshot exists — the exact lie this gate prevents.
            if let Err(e) = snapshot_rasters(&mut r, &dev, n) {
                let _ = std::fs::remove_file(&dst); // release the claim
                return Err(e);
            }
            let publish = (|| {
                let json = serde_json::to_string_pretty(&r).map_err(std::io::Error::other)?;
                write_staged(&tmp, json.as_bytes())?;
                durable_os::replace(&tmp, &dst)?;
                durable_os::finish_parent(&dst)
            })();
            if let Err(e) = publish {
                let _ = std::fs::remove_file(&tmp);
                rollback_frozen_rasters(&dev, n);
                let _ = std::fs::remove_file(&dst); // release the claim
                return Err(e);
            }
        }
        // Unparsable: snapshot the bytes as-is — still recoverable by hand.
        // Staged + renamed so a failed copy can never leave a PARTIAL file
        // wearing the final v<n>.recipe.json name (list_versions would then
        // count a corrupt snapshot as real).
        None => {
            if let Err(e) = std::fs::copy(&rj, &tmp)
                .and_then(|_| sync_staged(&tmp))
                .and_then(|_| durable_os::replace(&tmp, &dst))
                .and_then(|_| durable_os::finish_parent(&dst))
            {
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_file(&dst); // release the claim
                return Err(e);
            }
        }
    }
    // Provenance, best-effort and AFTER the snapshot is really published
    // (R24-2): this arm is the AUTOMATIC one — the gate preserving a save a
    // programmatic write is about to replace. It cannot name the variant
    // (the store has no strip cursor); the explicit 「＋ Save as version」
    // adds `from_kind`/`from_id` at its own call site.
    let _ = record_version_meta_unlocked(
        &dev,
        &VersionMetaEntry {
            n,
            origin: Some(VERSION_ORIGIN_AUTO.to_string()),
            ..Default::default()
        },
    );
    Ok(Some(n))
}

/// Reserve the NEXT version number by atomically claiming its recipe file
/// (create_new): `list_versions+1` alone let two processes (GUI + web + CLI)
/// pick the SAME number, and the later rename silently replaced the earlier
/// snapshot. The claimed 0-byte file is immediately overwritten by the
/// caller's tmp+rename publish (a rename onto a file we own) — the caller
/// MUST remove the claim on any later failure, or an empty version pollutes
/// the list. (A crash inside that window leaves a visible 0-byte version —
/// rarer and louder than the silent snapshot loss this replaces.)
pub fn claim_version(src: &Path) -> std::io::Result<(u32, PathBuf)> {
    std::fs::create_dir_all(develop_dir(src))?;
    // Complete any KILLED delete before claiming (L03): a half-deleted
    // version whose recipe already fell is invisible to list_versions, so
    // max+1 would RECYCLE its number — and the surviving marker would then
    // have recovery delete the brand-new snapshot. Best-effort: a number
    // whose marker cannot be cleared is skipped below, never claimed.
    let _ = recover_pending_version_deletes_unlocked(src);
    // The floor counts BURNED numbers too — the registry high-water mark,
    // not just live snapshots: a deleted version's number is never
    // re-issued (user decision 2026-08-13). The GUI lists versions by bare
    // `v<n>`, so a recycled label made the next gate snapshot read as "the
    // delete didn't work". A corrupt registry refuses the claim LOUDLY —
    // silently re-issuing a possibly-burned number is the very bug this
    // floor removes.
    let mut last = list_versions(src).last().copied();
    let reg = read_deleted_versions(&develop_dir(src))?;
    if reg.hwm > 0 && last.is_none_or(|l| l < reg.hwm) {
        last = Some(reg.hwm);
    }
    loop {
        if last == Some(u32::MAX) {
            return Err(std::io::Error::other(
                "version namespace exhausted (a v4294967295 snapshot exists or existed)",
            ));
        }
        let n = last.unwrap_or(0).saturating_add(1);
        let dst = version_target(src, n);
        if deleting_marker(src, n).exists() {
            // A marker that survived a failed resume owns this number (its
            // registry entry lands when the resume finally succeeds).
            last = Some(n);
            continue;
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&dst) {
            Ok(_) => return Ok((n, dst)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last = Some(n);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

/// The XMP-only half of [`backup_saved_develop`]: no recipe.json, but a
/// central (or not-yet-migrated legacy) XMP with real edits exists. Two
/// artifacts per version: `v<n>.<stem>.xmp` — the LOSSLESS bytes — published
/// first, then the `v<n>.recipe.json` CONTENT derived via `xmp_to_recipe`
/// (clamped; XMP carries no bitmap masks, so no rasters to freeze). NB this
/// ordering cannot hide the version number itself: `claim_version` has to
/// run before either publish (the xmp artifact's NAME needs `n`), and its
/// create_new claim IS a `v<n>.recipe.json` — `list_versions` therefore sees
/// the number as a 0-byte entry for the whole window, and a crash leaves
/// that documented loud residue (see `claim_version`). What xmp-first DOES
/// guarantee: any version whose recipe content is readable already has its
/// lossless xmp bytes on disk beside it. A neutral/foreign sidecar is not a
/// save (the NoopOnly rule) and needs no snapshot.
fn backup_xmp_only(src: &Path) -> std::io::Result<Option<u32>> {
    let mut found: Option<String> = None;
    for xp in [xmp_target(src), legacy_xmp(src)] {
        match read_text_capped(&xp, MAX_STORE_JSON) {
            Ok(t) => {
                found = Some(t);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Same refusal as the recipe path: unreadable ≠ absent.
            Err(e) => return Err(e),
        }
    }
    let Some(text) = found else { return Ok(None) };
    snapshot_xmp_text(src, text)
}

/// The publish half shared by [`backup_xmp_only`] and the Lightroom-sidecar
/// arm of [`backup_saved_develop`]: dedup against the latest version's xmp
/// bytes, derive + stamp the recipe content, claim a number, publish both
/// artifacts (lossless xmp first).
fn snapshot_xmp_text(src: &Path, text: String) -> std::io::Result<Option<u32>> {
    let dev = develop_dir(src);
    let stem = crate::pipeline::stem(src);
    // Change-detection instead of an is_noop skip: a derived-noop XMP can
    // still carry edits xmp_to_recipe does not model (Texture, …) — skipping
    // it let analyze/match destroy the only copy. Identical bytes to the
    // NEWEST PRESERVED xmp snapshot mean this save is already preserved (no
    // version spam on repeated programmatic writes). Newest-that-EXISTS,
    // not v<max>: recipe-only snapshots interleave in the same number
    // space (the L13#3 store-then-sidecar order), so v<max> often has no
    // xmp beside it and a v<max>-only probe re-snapshotted an unchanged
    // sidecar on every gated write.
    for n in list_versions(src).into_iter().rev() {
        match read_text_capped(&dev.join(format!("v{n}.{stem}.xmp")), MAX_STORE_JSON) {
            Ok(prev) => {
                if prev == text {
                    return Ok(None);
                }
                break; // the newest preserved xmp differs — a snapshot is due
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // An unreadable snapshot cannot prove preservation — preserve anew.
            Err(_) => break,
        }
    }
    // The deleted-version registry (2026-08-13): sidecar bytes the user
    // explicitly discarded from the version list are not re-preserved. The
    // live witness above IS the deletable `v<n>.<stem>.xmp` — deleting the
    // version deleted the proof, and the next gated write resurrected the
    // snapshot byte-for-byte under a freshly recycled number.
    if discarded_xmp_matches(&dev, &text) {
        return Ok(None);
    }
    let diag = crate::diag::photo(src);
    let mut derived = crate::xmp::xmp_to_recipe_with_diag(&text, &diag);
    // A6 disclosure: numbers the import cannot read become silent neutrals
    // in this derived snapshot. Background path — the trace goes to stderr;
    // the interactive surfaces disclose the same fact at restore time.
    let bad = crate::xmp::unparsable_crs_numbers(&text);
    if !bad.is_empty() {
        eprintln!(
            "⚠ {} numeric XMP setting(s) unreadable ({}) — the derived snapshot treats them as neutral",
            bad.len(),
            bad.join(", ")
        );
    }
    // The CROP half of the same disclosure (R27): a tilted Lightroom rectangle
    // is a rotated-corner encoding in the source frame, and this engine's
    // rotate-then-crop composition cannot always express one — it is trimmed,
    // or (with no declared frame) not placed at all. Same channel, same
    // background-path rule as the numbers above.
    if let Some(note) = crate::xmp::crop_import_note_for_photo(&text, src) {
        eprintln!("⚠ {note}");
    }
    derived.clamp();
    // Stamp like the GUI's XMP-only RESTORE does (fresh camera knots + the
    // in-camera lens profile): a verbatim derived snapshot loaded back would
    // render on the dark base without corrections.
    if derived.base_curve.is_empty() {
        derived.base_curve = crate::pipeline::photo_base_knots(src);
    }
    derived.lens_profile = crate::pipeline::fresh_lens_profile(src);
    // Third calibration half: a foreign (Lightroom) Temperature is ABSOLUTE
    // — anchoring the derived recipe at the camera's real as-shot renders it
    // closer to Lightroom's intent. Stamp-if-None: an old-era AUTOSHADE
    // projection arrives with the 5500 anchor PINNED by xmp_to_recipe (its
    // Kelvin was tuned relative) and must keep rendering as tuned.
    if derived.as_shot_k.is_none() {
        let (ask, ast) = crate::pipeline::fresh_as_shot_wb(src);
        derived.as_shot_k = ask;
        derived.as_shot_tint = ast;
    }
    let (n, dst) = claim_version(src)?;
    let xmp_dst = dev.join(format!("v{n}.{stem}.xmp"));
    if let Err(e) = durable_write(&xmp_dst, text.as_bytes()) {
        let _ = std::fs::remove_file(&dst); // release the claim
        return Err(e);
    }
    let json = serde_json::to_string_pretty(&derived).map_err(std::io::Error::other)?;
    if let Err(e) = durable_write(&dst, json.as_bytes()) {
        let _ = std::fs::remove_file(&xmp_dst);
        let _ = std::fs::remove_file(&dst); // release the claim
        return Err(e);
    }
    // The xmp half of the same automatic gate (R24-2): one gate CALL can
    // burn two numbers — the store snapshot and this sidecar snapshot
    // interleave in one number space — so both stamp their own provenance.
    let _ = record_version_meta_unlocked(
        &dev,
        &VersionMetaEntry {
            n,
            origin: Some(VERSION_ORIGIN_AUTO.to_string()),
            ..Default::default()
        },
    );
    Ok(Some(n))
}

/// Copy each raster the recipe references INSIDE `dev` to a version-frozen
/// name (`v<n>.<name>`) and rewrite the reference. `list_versions` only parses
/// `v<N>.recipe.json`, so the frozen rasters never pollute the version list.
/// A copy failure rolls back this call's earlier copies and errors — a
/// snapshot must never silently keep pointing at a mutable live raster.
pub fn snapshot_rasters(r: &mut EditRecipe, dev: &Path, n: u32) -> std::io::Result<()> {
    for path in r.raster_paths_mut() {
        let p = Path::new(path.as_str());
        // Bare name (the store convention) or absolute path inside dev.
        let name = if p.is_absolute() {
            (p.parent() == Some(dev)).then(|| p.file_name()).flatten()
        } else if p.parent().is_none_or(|x| x.as_os_str().is_empty()) {
            p.file_name()
        } else {
            None
        };
        let Some(name) = name.and_then(|x| x.to_str()) else { continue };
        let frozen_name = format!("v{n}.{name}");
        let live = dev.join(name);
        // metadata triage, not exists(): a permission/transient error on
        // an EXISTING raster read as "missing", the freeze was skipped,
        // and the gate reported a faithful snapshot that never froze the
        // raster the caller then overwrote.
        let live_present = match std::fs::metadata(&live) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                rollback_frozen_rasters(dev, n);
                return Err(e);
            }
        };
        if live_present {
            // `n` is guaranteed FRESH (backup_saved_develop refuses
            // colliding numbers), so an existing v<n>.* file here is a
            // crashed earlier attempt's leftover — possibly PARTIAL.
            // Re-stage it atomically instead of trusting it.
            let frozen = dev.join(&frozen_name);
            if let Err(e) = copy_atomic(&live, &frozen) {
                rollback_frozen_rasters(dev, n);
                return Err(e);
            }
        }
        // The reference moves to the frozen name EVEN when the live
        // raster is already gone: a dead reference frozen as v<n>.<name>
        // stays inert forever (the engine's missing-raster warning),
        // while keeping the live name would silently bind this snapshot
        // to any FUTURE raster recreated under it.
        *path = frozen_name;
    }
    Ok(())
}

/// Remove every `v<n>.*` frozen raster (backup rollback — the snapshot recipe
/// itself is written only after all freezes succeed). pub: the GUI's
/// save_version freezes rasters through the same pair.
pub fn rollback_frozen_rasters(dev: &Path, n: u32) {
    let prefix = format!("v{n}.");
    if let Ok(dir) = std::fs::read_dir(dev) {
        for e in dir.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(&prefix) && !name.ends_with(".recipe.json") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Delete snapshot `n`: its recipe file plus any `v<n>.*` frozen rasters.
/// Registers the number and content fingerprints in the develop's
/// `.deleted-versions.json` first — the number is never re-issued and the
/// backup gate stops auto-preserving the discarded content (see
/// [`DeletedVersions`]).
pub fn delete_version(src: &Path, n: u32) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || delete_version_unlocked(src, n))
}

/// The version-delete transaction marker (L03). A leading dot keeps it
/// outside the `v<N>.` namespace the sweep removes and `list_versions`
/// parses; while it exists the version is "being removed" — unlisted, its
/// number unclaimable — and recovery resumes the sweep.
pub(super) fn deleting_marker(src: &Path, n: u32) -> PathBuf {
    develop_dir(src).join(format!(".deleting.v{n}"))
}

/// The version-delete REGISTRY (user decision 2026-08-13): one
/// `.deleted-versions.json` per develop dir recording every deleted
/// snapshot — its number and its content FINGERPRINTS (fnv1a64 hex), never
/// the payload. Two duties:
///
/// * `hwm` is the claim floor: a deleted number is never re-issued — the
///   GUI lists versions by bare `v<n>`, so a recycled label made the next
///   gate snapshot read as "the delete didn't work";
/// * the fingerprints let the backup gate keep recognising content the
///   user explicitly discarded: its only "already preserved" dedup witness
///   used to be the deletable snapshot itself, so the next gated
///   programmatic write (analyze persist / reverse-fit / paste / CLI /
///   serve / legacy migration) re-preserved the very content the user had
///   just deleted.
///
/// This is a DISCARD record, not a recovery copy: the deleted snapshot's
/// bytes — recipe, frozen rasters, xmp witness (a Lightroom sidecar's only
/// preserved spelling included) — are really gone, and deleting a version
/// frees its space. An explicit 「＋ Save as version」 still saves anything:
/// the fingerprints gate only AUTOMATIC preservation. Every fingerprint
/// fails toward preservation — an unreadable registry, a hash the current
/// schema no longer reproduces, or an unprovable raster set each mean
/// "preserve anew", never "suppress".
///
/// The dot name keeps the file outside the `v<N>.` namespace the sweep
/// removes and `list_versions` parses; `adoption_skips` does not match it,
/// so adoption carries it (and its SYNCHRONIZE pass cannot strand a
/// destination-only copy the source has since grown); `clear_develop`'s
/// explicit file list leaves it standing exactly like the versions
/// themselves.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct DeletedVersions {
    /// Highest number ever burned by a delete (live snapshots are counted
    /// separately at claim time).
    #[serde(default)]
    pub(super) hwm: u32,
    #[serde(default)]
    pub(super) deleted: Vec<DeletedVersion>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct DeletedVersion {
    pub(super) n: u32,
    /// fnv1a64 (hex) of the snapshot recipe's raw bytes — the arm that
    /// catches the gate's unparsable-copy snapshots byte-for-byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) recipe_raw: Option<String>,
    /// fnv1a64 (hex) of the parsed recipe re-serialized compactly with its
    /// `v<n>.` frozen raster prefixes stripped back to bare names — the
    /// structural arm (schema drift changes this hash: fail-open).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) recipe_norm: Option<String>,
    /// Combined fnv1a64 (hex) over the sorted (bare name, raster-bytes
    /// fnv1a64) pairs of the in-dev rasters the snapshot referenced. Equal
    /// recipe structure alone is NOT proof (a released claim can re-mint a
    /// name with different bytes; adoption replaces rasters in place), so
    /// the bytes join the fingerprint. Absent = unprovable at delete time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rasters: Option<String>,
    /// fnv1a64 (hex) of the version's `v<n>.<stem>.xmp` witness bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) xmp: Option<String>,
}

pub(super) fn deleted_versions_path(dev: &Path) -> PathBuf {
    dev.join(".deleted-versions.json")
}

/// NotFound → empty registry; unreadable/corrupt → Err. The WRITER
/// (`register_deleted_version`) and the claim floor propagate that Err —
/// the store's "unreadable ≠ absent" refusal, loud — while the gate and
/// migration PROBES treat it as no-match instead: their fail direction is
/// "preserve / migrate anew", and the claim right behind them reports the
/// same corruption loudly.
pub(super) fn read_deleted_versions(dev: &Path) -> std::io::Result<DeletedVersions> {
    match read_text_capped(&deleted_versions_path(dev), MAX_STORE_JSON) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| {
            std::io::Error::other(format!(
                "the deleted-version registry {} is unreadable ({e}) — repair or remove it by hand",
                deleted_versions_path(dev).display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DeletedVersions::default()),
        Err(e) => Err(e),
    }
}

fn hex64(h: u64) -> String {
    format!("{h:016x}")
}

/// Strip v<n>'s frozen raster prefixes back to the bare live names.
fn strip_frozen_raster_prefix(r: &mut EditRecipe, n: u32) {
    let frozen = format!("v{n}.");
    for path in r.raster_paths_mut() {
        if let Some(bare) = path.strip_prefix(frozen.as_str()) {
            *path = bare.to_string();
        }
    }
}

/// The structural fingerprint: compact re-serialization of the parsed
/// recipe. None = unserializable — fail toward preservation.
fn recipe_struct_hash(r: &EditRecipe) -> Option<String> {
    serde_json::to_string(r).ok().map(|j| hex64(fnv1a64(j.as_bytes())))
}

/// Streaming file hash — bounded memory whatever the raster size. None on
/// any read failure (missing file included).
fn file_fnv1a64(path: &Path) -> Option<u64> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = FNV_OFFSET;
    let mut buf = [0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => return Some(h),
            Ok(k) => h = fnv1a64_update(h, &buf[..k]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
}

/// Fingerprint of the raster BYTES a (bare-named) recipe references inside
/// `dev` — the same bare-or-absolute-inside-dev name rule
/// [`snapshot_rasters`] freezes by; anything else (legacy `out/…`
/// relatives) is identified by its reference string alone, which the
/// struct hash already covers. `frozen_n` reads each name's `v<n>.<name>`
/// frozen copy instead of the live file (the delete side). Some(base) for
/// a recipe with no such references; None the moment ONE referenced raster
/// cannot be read — an unprovable set must fail toward preservation.
fn raster_set_hash(r: &EditRecipe, dev: &Path, frozen_n: Option<u32>) -> Option<String> {
    let mut pairs: Vec<(String, u64)> = Vec::new();
    let mut clone = r.clone();
    for path in clone.raster_paths_mut() {
        let p = Path::new(path.as_str());
        let name = if p.is_absolute() {
            (p.parent() == Some(dev)).then(|| p.file_name()).flatten()
        } else if p.parent().is_none_or(|x| x.as_os_str().is_empty()) {
            p.file_name()
        } else {
            None
        };
        let Some(name) = name.and_then(|x| x.to_str()) else { continue };
        let file = match frozen_n {
            Some(n) => dev.join(format!("v{n}.{name}")),
            None => dev.join(name),
        };
        pairs.push((name.to_string(), file_fnv1a64(&file)?));
    }
    pairs.sort();
    let mut h = FNV_OFFSET;
    for (name, fh) in &pairs {
        h = fnv1a64_update(h, name.as_bytes());
        h = fnv1a64_update(h, &fh.to_le_bytes());
    }
    Some(hex64(h))
}

/// Record v<n> as deleted — burn its number and fingerprint its content —
/// BEFORE the sweep destroys the sources. A read-modify-write of the
/// registry under the develop lock every caller already holds; idempotent
/// by number (a resume keeps the crashed attempt's richer entry).
/// `must_exist` mirrors [`sweep_version_unlocked`]: a fresh delete
/// propagates a missing recipe as its stale-list NotFound, while a RESUME
/// (or a pre-registry `.deleting.v<n>` marker) burns the number with no
/// fingerprints — half-swept content cannot be fingerprinted, only kept
/// unclaimable. An UNREADABLE snapshot burns fingerprint-less the same
/// way: refusing the user's delete over a fingerprint we cannot take
/// would make a corrupt version undeletable.
pub(super) fn register_deleted_version(src: &Path, n: u32, must_exist: bool) -> std::io::Result<()> {
    let dev = develop_dir(src);
    let mut reg = read_deleted_versions(&dev)?;
    let known = reg.deleted.iter().any(|e| e.n == n);
    if known && reg.hwm >= n {
        return Ok(());
    }
    if !known {
        let mut entry = DeletedVersion {
            n,
            recipe_raw: None,
            recipe_norm: None,
            rasters: None,
            xmp: None,
        };
        match read_text_capped(&version_target(src, n), MAX_STORE_JSON) {
            Ok(text) => {
                entry.recipe_raw = Some(hex64(fnv1a64(text.as_bytes())));
                if let Ok(mut bare) = serde_json::from_str::<EditRecipe>(&text) {
                    strip_frozen_raster_prefix(&mut bare, n);
                    entry.recipe_norm = recipe_struct_hash(&bare);
                    entry.rasters = raster_set_hash(&bare, &dev, Some(n));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if must_exist {
                    return Err(e);
                }
            }
            Err(_) => {}
        }
        let stem = crate::pipeline::stem(src);
        entry.xmp = file_fnv1a64(&dev.join(format!("v{n}.{stem}.xmp"))).map(hex64);
        reg.deleted.push(entry);
    }
    reg.hwm = reg.hwm.max(n);
    let json = serde_json::to_string_pretty(&reg).map_err(std::io::Error::other)?;
    durable_write(&deleted_versions_path(&dev), json.as_bytes())
}

/// Was this very save explicitly discarded by a version delete? Raw-bytes
/// arm first (the unparsable-copy snapshots), then the structural arm —
/// which additionally requires the referenced raster BYTES to match: equal
/// references alone are not proof the pixels are the ones the user
/// discarded. Registry unreadable → false (preserve anew; the claim right
/// behind this probe reports the same corruption loudly).
fn discarded_recipe_matches(dev: &Path, text: &str, parsed: Option<&EditRecipe>) -> bool {
    let Ok(reg) = read_deleted_versions(dev) else { return false };
    if reg.deleted.is_empty() {
        return false;
    }
    let raw = hex64(fnv1a64(text.as_bytes()));
    if reg.deleted.iter().any(|e| e.recipe_raw.as_deref() == Some(raw.as_str())) {
        return true;
    }
    let Some(parsed) = parsed else { return false };
    let Some(norm) = recipe_struct_hash(parsed) else { return false };
    // The live raster hash is computed at most ONCE, and only after a
    // structural hit (reading raster bytes on every gated write would tax
    // the common no-match path).
    let mut live_rasters: Option<Option<String>> = None;
    for e in &reg.deleted {
        if e.recipe_norm.as_deref() != Some(norm.as_str()) {
            continue;
        }
        let Some(stone) = e.rasters.as_deref() else { continue };
        let live = live_rasters
            .get_or_insert_with(|| raster_set_hash(parsed, dev, None))
            .as_deref();
        if live == Some(stone) {
            return true;
        }
    }
    false
}

/// The xmp arm of the discard probe: sidecar bytes the user discarded with
/// a version are not re-preserved from a file that still spells them.
fn discarded_xmp_matches(dev: &Path, text: &str) -> bool {
    let Ok(reg) = read_deleted_versions(dev) else { return false };
    let h = hex64(fnv1a64(text.as_bytes()));
    reg.deleted.iter().any(|e| e.xmp.as_deref() == Some(h.as_str()))
}
