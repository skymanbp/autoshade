//! Reading a develop: the render source, revisions, the snapshot, orphan recovery, and the pixel source.

use super::*;

/// THE develop's render source, shared by every surface (CLI, GUI export and
/// the web) so they can never disagree about what a recipe is applied TO.
///
/// A saved `pixels.json` master IS the develop's source: heal/clone/generative
/// results are pixels no recipe can reproduce. The calibration rule rides
/// along because it is a property of the SOURCE, not of the surface:
/// * `inplace` master (heal/clone) = a NEUTRAL develop, so `base_curve` and
///   `lens_profile` still render on top of it;
/// * `generated` master (AI reimagine/fill) already carries the look in its
///   pixels, so both fields are stripped from the copy being rendered — the
///   recipe ON DISK keeps them (a master that later fails to decode must
///   still restore a calibrated develop).
///
/// A master that WAS recorded but cannot be honoured is an `Err` naming the
/// remedy, not a silent fallback — exporting the un-retouched source while
/// reporting success was the A6 defect. Preview surfaces degrade explicitly
/// at their call site (a canvas must still open; the web rides an
/// X-Preview-Warning); the silently-degrading `render_source` wrapper had no
/// callers left and hid exactly that decision, so it is gone. On success the
/// pre-era repair's disclosure rides along for the caller to surface.
/// The `Err` message is ASCII-only: it travels in an HTTP header.
pub fn render_source_checked(
    raw: &Path,
    recipe: &mut EditRecipe,
) -> Result<(PathBuf, Option<String>), String> {
    // The pre-era base-curve repair belongs HERE, not only on the surfaces
    // that stamp calibration. Batch 52 put it in `saved_recipe_snapshot` and
    // claimed "no surface can render a washed curve by forgetting to ask" —
    // but that funnel is the programmatic WRITER's path. Every deliverable
    // reads recipe.json for itself (GUI batch export, CLI apply, load_version)
    // or receives it over HTTP, and each of them rendered the washed curve at
    // full resolution while the GUI canvas showed the repaired one: two
    // surfaces of the same build disagreeing about the same file.
    //
    // This function is what deliverables DO share, and it already takes the
    // recipe by &mut for exactly this class of source-dependent correction.
    // The repair is a no-op for era-2 recipes and for any curve without the
    // fingerprint, so the cost lands only on the photos that need it — and it
    // runs AFTER the generated strip below (the load_version ordering): a
    // generated master's curve is deleted either way, so repairing first paid
    // a RAW decode + develop for an estimate nothing could use, on every
    // caller of this funnel. And it runs ONLY when a source is handed back:
    // every deliverable caller ABORTS on the Err arm, and funding a full RAW
    // decode for a render that never runs was the tax two earlier fixes
    // existed to avoid (api_export pays it holding the HEAVY lock). The one
    // caller that renders anyway after a refusal — the web preview's
    // degraded fallback — runs the repair itself at that decision, where the
    // cost buys pixels the user actually sees.
    let source = match read_pixel_source(raw) {
        Some((master, generated)) => {
            if generated {
                recipe.strip_baked_start();
                // Batch-30 rule, applied where every surface renders: baked
                // pixels carry their white balance, so the absolute anchor is
                // stripped — a CLI/web render of a generated master must
                // match the GUI canvas, which strips it too.
                recipe.as_shot_k = None;
                recipe.as_shot_tint = None;
            }
            Ok(master)
        }
        // STATIC ASCII text, no stem: the message travels in an HTTP header,
        // and a non-ASCII file name made Header::from_bytes fail — silently
        // dropping the very warning this path exists to deliver. Callers
        // that batch photos add their own per-photo prefix.
        None if has_pixel_source(raw) => Err(
            "the saved retouch master could not be loaded - rendering would silently drop \
             the retouch; open the photo for the cause, then re-save or clear the link with \
             a parametric-only save"
                .to_string(),
        ),
        None => Ok(raw.to_path_buf()),
    };
    source.map(|p| (p, crate::pipeline::repair_pre_era_base_curve(raw, recipe)))
}

/// Repair a CRASHED publish. `write_recipe` and `write_pixel_source` retire
/// the live file to `<name>.bak` and then rename the staged copy over it; a
/// crash between those two renames leaves the develop ONLY in the `.bak`.
/// Nothing used to look there at read time: every reader reported "no
/// develop", the GUI/web silently fell back to the lossy XMP, and the next
/// programmatic save then snapshotted THAT and overwrote the survivor —
/// recipe-only work (bitmap masks, colour gains, the baked-master linkage)
/// was gone. Restoring here makes every reader see the real last-known-good.
/// Best-effort and idempotent: a live file always wins (the restore
/// publishes no-clobber, so even a save landing CONCURRENTLY is never
/// replaced), and a failed restore leaves the survivor untouched.
/// Returns `Ok(())` when nothing needed recovering or everything recovered,
/// and `Err` naming the survivor when an orphan could NOT be restored — a
/// locked or permission-denied `.bak` is an EXISTING save we cannot see, and
/// callers that decide whether a save exists must refuse rather than treat it
/// as absence (the same "unreadable is not absent" rule the backup gate
/// applies to the recipe itself).
pub fn recover_orphan_baks(src: &Path) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || recover_orphan_baks_unlocked(src))
}

/// The saved recipe's revision tag: the FNV-1a of recipe.json's bytes,
/// `"none"` when no file exists — absence is a REAL revision, two tabs
/// racing the FIRST save of a fresh photo must still collide — and `None`
/// when the file exists but cannot be read (untaggable; a conditional
/// writer refuses rather than overwrite what it cannot name). Content, not
/// mtime: stamp granularity is too coarse to gate a write.
pub fn recipe_revision(src: &Path) -> Option<String> {
    revision_of(&recipe_target(src))
}

/// The saved develop's revision tag for HTTP preconditions: the recipe.json
/// revision, with the Lightroom sidecar's CONTENT identity folded in
/// whenever that sidecar currently out-ranks the store — because it is then
/// the very body `/api/recipe` serves. Tagging only the store file let a
/// stale tab pass If-Match while the sidecar its answer was built from had
/// changed underneath it (L04-4): the lost update the precondition exists
/// to refuse. `None` (untaggable) when either side exists but cannot be
/// read, the same stance as [`recipe_revision`].
pub fn develop_revision(src: &Path) -> Option<String> {
    develop_revision_of(src, &lightroom_sidecar(src))
}

/// The tag for a sidecar ranking the CALLER already holds (review R12-04):
/// the GET handler's body and its ETag must come from ONE sidecar read —
/// Lightroom holds none of our locks, so a second read could describe a
/// file the body was not built from (body A shipped under ETag B, and the
/// client's later If-Match: B silently overwrote B with stale A).
pub fn develop_revision_of(src: &Path, ranked: &LrSidecar) -> Option<String> {
    let base = recipe_revision(src)?;
    match ranked {
        LrSidecar::NewerThanStore(text) | LrSidecar::Only(text) => {
            Some(format!("{base}+lr{:016x}", fnv1a64(text.as_bytes())))
        }
        LrSidecar::Unreadable(_) => None,
        _ => Some(base),
    }
}

pub(super) fn revision_of(p: &Path) -> Option<String> {
    match read_bytes_capped(p, MAX_STORE_JSON) {
        Ok(b) => Some(format!("r{:016x}", fnv1a64(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some("none".into()),
        Err(_) => None,
    }
}

/// One coherent view of a photo's saved develop, for a renderer that must
/// not see a mid-save interleave.
pub struct DevelopSnapshot {
    /// Saved recipe text + the file it came from (rasters re-anchor to its
    /// dir) — central store first, else a legacy ./out sidecar.
    pub recipe: Option<(String, PathBuf)>,
    /// A CENTRAL read failure that is not absence (permissions, over-cap):
    /// an existing save the caller must refuse to render over — falling
    /// back to legacy would resurrect stale edits, so the walk stops.
    pub recipe_err: Option<String>,
    /// Lightroom's OWN sidecar when it out-ranks the store (newest intent,
    /// [`lightroom_sidecar`]) — text + the same kind string the GUI open
    /// path shows, so the two surfaces cannot drift (L13#1: the batch
    /// renderer read recipe.json only, exporting neutral for LR-only
    /// photos and preferring an older recipe over a newer LR edit).
    pub lr_xmp: Option<(String, &'static str)>,
    /// The store's XMP projection (central, else legacy ./out) — the
    /// recipe-ABSENT fallthrough the open path takes. A present recipe.json,
    /// neutral or not, is never out-answered by it (2026-09-13): the
    /// projection is derived from the recipe in the same commit generation,
    /// so one that disagrees can only be stale.
    pub store_xmp: Option<(String, &'static str)>,
    /// [`LrSidecar::Unreadable`] — a sidecar that EXISTS but cannot answer,
    /// never folded into absence (the caller discloses it).
    pub lr_unreadable: Option<&'static str>,
    /// The RAW's embedded XMP packet ([`embedded_packet_for_restore`]) —
    /// the open path's LOWEST-priority source, snapshotted so the batch
    /// renderer answers like the open path (the L13 rule).
    pub packet_xmp: Option<String>,
    /// [`embedded_packet_for_restore`]'s `Err`: a packet that exists but
    /// cannot be read — the caller discloses it.
    pub packet_unreadable: Option<String>,
    /// [`read_pixel_source`]'s answer, taken under the same lock.
    pub pixel_source: Option<(PathBuf, bool)>,
    /// [`has_pixel_source`]'s answer — a recorded-but-unloadable master
    /// shows up as `pixel_source: None, pixel_recorded: true`.
    pub pixel_recorded: bool,
}

/// Snapshot a photo's saved develop under ONE develop-lock acquisition
/// (Wait — the worker-thread rule), with the `.bak` recovery FIRST. The GUI
/// batch renderer used to take four independent unlocked store touches per
/// photo: a writer retires recipe.json to .bak for its whole staged publish,
/// so an unlocked exists() read "no develop" mid-save and the batch shipped
/// a neutral render of an edited photo — and the only .bak recovery on that
/// path was buried inside the pixel read, AFTER the recipe had been
/// selected. Render OUTSIDE the lock, on the snapshot (the CLI's documented
/// contract).
pub fn read_develop_snapshot(src: &Path) -> std::io::Result<DevelopSnapshot> {
    with_develop_lock(src, DevelopLockMode::Wait, || {
        // A FAILED recovery is already reported by the helper (the
        // read_pixel_source contract) — the reads below then degrade
        // exactly as they do for any unreadable sidecar.
        let _ = recover_orphan_baks_unlocked(src);
        // The Lightroom sidecar, ranked under the SAME lock (L13#1). The
        // kind strings are byte-identical to the GUI open path's
        // (persist.rs), so the surfaces cannot drift apart in wording.
        let mut lr_xmp = None;
        let mut lr_unreadable = None;
        match lightroom_sidecar(src) {
            LrSidecar::NewerThanStore(t) => {
                lr_xmp = Some((
                    t,
                    "XMP (Lightroom sidecar — newer than the saved develop; Ctrl+S adopts it)",
                ));
            }
            LrSidecar::Only(t) => {
                lr_xmp = Some((t, "XMP (Lightroom sidecar beside the RAW)"));
            }
            LrSidecar::Unreadable(why) => lr_unreadable = Some(why),
            _ => {}
        }
        let mut recipe = None;
        let mut recipe_err = None;
        for rj in [recipe_target(src), legacy_recipe(src)] {
            // Read directly — no exists() probe: absence is the read's own
            // NotFound, decided at the same instant as the content.
            match read_text_capped(&rj, MAX_STORE_JSON) {
                Ok(t) => {
                    recipe = Some((t, rj));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    recipe_err = Some(format!("cannot read {}: {e}", rj.display()));
                    break;
                }
            }
        }
        // The store's XMP projection — what the open path restores when the
        // recipe is ABSENT (a present one, neutral included, ends the walk;
        // the resolver draws that line). Same one-file precedence walk as
        // the recipe: only NotFound falls through, and an unreadable
        // projection with NO recipe to shadow it is an existing save the
        // caller must refuse over (folded into recipe_err).
        let mut store_xmp = None;
        for (xp, kind) in [(xmp_target(src), "XMP"), (legacy_xmp(src), "XMP (legacy ./out)")] {
            match read_text_capped(&xp, MAX_STORE_JSON) {
                Ok(t) => {
                    store_xmp = Some((t, kind));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    if recipe.is_none() && recipe_err.is_none() {
                        recipe_err = Some(format!("cannot read {}: {e}", xp.display()));
                    }
                    break;
                }
            }
        }
        // The embedded packet, under the SAME lock as the markers that gate
        // it (a clear completing between two unlocked reads must not let the
        // packet answer for a develop that is being removed). Filled only
        // when NO store file exists at all — a NEUTRAL recipe.json or
        // projection is a store file expressing neutral intent, and letting
        // the packet answer past it resurrected the baked develop (Codex
        // L05 EMBED-01; the open path draws the same `!any` line).
        let (packet_xmp, packet_unreadable) =
            if recipe.is_none() && recipe_err.is_none() && store_xmp.is_none() {
                match embedded_packet_for_restore(src) {
                    Ok(v) => (v, None),
                    Err(why) => (None, Some(why)),
                }
            } else {
                (None, None)
            };
        Ok(DevelopSnapshot {
            recipe,
            recipe_err,
            lr_xmp,
            store_xmp,
            lr_unreadable,
            packet_xmp,
            packet_unreadable,
            pixel_source: read_pixel_source(src),
            pixel_recorded: has_pixel_source(src),
        })
    })
}

/// Complete a KILLED explicit clear (the `clear.pending` marker): sweep,
/// stamp, and consume the marker — shared by the recovery head and by
/// [`commit_develop`], which must finish a pending clear BEFORE staging a
/// new generation (the batch-S trap: a marker that outlives the save it
/// predates would eat that save on the next recovery).
/// Best-effort durability for a CONSUMED transaction marker (Codex round-12
/// durability review, finding 2): fsync the directory that held it. Without
/// this (unix), the plain unlink can fail to reach the disk and the
/// resurrected marker would replay its transaction over work that POSTDATES
/// it; with it, the exposure collapses to "a not-yet-durable save may be
/// lost" — the store's existing crash contract (any later durable publish
/// in the same directory persists this unlink alongside its own entry).
/// Windows: finish_parent is a documented no-op and the ordering rests on
/// NTFS's sequential metadata journal — a later write-through rename forces
/// every earlier metadata record down with it.
pub(super) fn settle_consumed_marker(path: &Path) {
    let _ = durable_os::finish_parent(path);
}

pub(super) fn resolve_pending_clear_unlocked(src: &Path) -> std::io::Result<()> {
    if !clear_pending(src).exists() {
        return Ok(());
    }
    let (_, err) = clear_sweep(src);
    if let Some(e) = err {
        // The recover contract: Err is "state we cannot resolve" — the
        // backup gates refuse rather than overwrite it.
        return Err(e);
    }
    // The clear marker is DESTRUCTIVE on replay: a marker that survives a
    // "successful" resolution eats whatever save lands after it, on the next
    // recovery (the batch-S trap, reachable through this failure path). So
    // failing to stamp or to consume it must fail the resolution — callers
    // then refuse to stage a new generation on top of it.
    mark_develop_cleared(src)?;
    match std::fs::remove_file(clear_pending(src)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(std::io::Error::new(
                e.kind(),
                format!(
                    "the completed develop clear could not consume its marker ({e}) — saving \
                     is refused until it can, or until the marker is removed by hand"
                ),
            ));
        }
    }
    settle_consumed_marker(&clear_pending(src));
    Ok(())
}

pub(super) fn recover_orphan_baks_unlocked(src: &Path) -> std::io::Result<()> {
    // A PENDING explicit clear outranks every other recovery (L03): the
    // marker says the user's last intent was "remove this develop", so the
    // sweep completes FIRST — the .bak republish below would otherwise
    // resurrect the very files a killed clear was removing. (The sweep takes
    // any pending `.commit` with it: the clear is the newer intent by
    // construction, since `commit_develop` completes a pending clear before
    // it stages.)
    if clear_pending(src).exists() {
        resolve_pending_clear_unlocked(src)?;
        return Ok(());
    }
    // A CRASHED single-generation commit resolves next (L03): a marked stage
    // rolls FORWARD (the generation was promised), an unmarked stage rolls
    // back — and it must happen before the `.bak` pair loop below, which is
    // generation-blind and would otherwise republish members the committed
    // generation replaces or clears.
    resolve_pending_commit_unlocked(src)?;
    // A KILLED version delete resumes here too — best-effort and DISCLOSED,
    // never folded into this fn's Err: that contract means "a save we
    // cannot see, refuse to overwrite", and one locked raster in a dead
    // version must not block every future save of the photo (L03).
    if let Err(e) = recover_pending_version_deletes_unlocked(src) {
        eprintln!("⚠ a crashed version delete could not be completed ({e}) — the version stays hidden and the sweep retries on the next touch");
    }
    let dev = develop_dir(src);
    let mut failure: Option<std::io::Error> = None;
    for (live, bak) in [
        (recipe_target(src), dev.join("recipe.json.bak")),
        (pixel_source_path(src), dev.join("pixels.json.bak")),
        (variants_path(src), dev.join("variants.json.bak")),
    ] {
        if !bak.exists() {
            continue;
        }
        match std::fs::metadata(&live) {
            // A ZERO-BYTE live beside a surviving .bak is a dead CLAIM, not
            // a save: on the no-hard-link path, publish_no_clobber claims
            // the live name with an empty create_new before its
            // write-through replace, and a crash inside that window leaves
            // exactly this state. No JSON member is ever legitimately empty,
            // and no publisher can be mid-claim here — they all hold this
            // lock. Left alone, the claim blocks the restore (AlreadyExists)
            // and the NEXT save retires the empty file onto the .bak,
            // destroying the only survivor. Clear the claim and restore.
            Ok(m) if m.len() == 0 => {
                let _ = std::fs::remove_file(&live);
                let _ = durable_os::finish_parent(&live);
            }
            Ok(_) => continue,
            Err(_) => {} // absent — the publish below decides the rest
        }
        // NOT a direct rename: fs::rename REPLACES an existing destination
        // (verified empirically — see next_tmp_seq), so the old exists-check
        // + rename pair had a window in which a save published by a
        // CONCURRENT process (GUI, web server and CLI share this store) was
        // silently replaced with the older pre-crash bytes. Stage a COPY of
        // the survivor and publish through the no-clobber primitive; the
        // .bak itself is consumed only AFTER its content demonstrably landed
        // — publish_no_clobber deletes its staged input on every path, which
        // must never happen to the only copy of a develop.
        let published = (|| {
            let tmp = sibling_tmp(&live);
            if let Err(e) = std::fs::copy(&bak, &tmp) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            publish_no_clobber(&tmp, &live)
        })();
        match published {
            // Restored: the survivor is live again — consume the .bak.
            Ok(true) => {
                let _ = std::fs::remove_file(&bak);
            }
            // A concurrent save owns the live file — newest intent wins, and
            // the .bak stays behind as the normal retired previous state.
            Ok(false) => {}
            Err(e) => {
                eprintln!(
                    "⚠ {} survives a crashed publish but could not be restored ({e})",
                    bak.display()
                );
                failure.get_or_insert(e);
            }
        }
    }
    match failure {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

/// Was a baked master ever RECORDED for this photo — regardless of whether it
/// can still be honoured?
///
/// [`read_pixel_source`] answers `None` for BOTH "nothing recorded" and
/// "recorded but unusable" (corrupt sidecar, deleted or moved master), so a
/// caller that wants to WARN about a broken linkage cannot ask it twice: the
/// second call returns None again and the warning never fires — which is
/// exactly what happened to the GUI's failed-restore toast for the two
/// commonest causes. This predicate answers the question that caller is
/// really asking.
pub fn has_pixel_source(src: &Path) -> bool {
    has_pixel_source_in(&store_root(), src)
}

pub(super) fn has_pixel_source_in(root: &Path, src: &Path) -> bool {
    let dev = develop_dir_in(root, src);
    // The .bak counts: a crashed publish still means a master WAS recorded.
    pixel_source_path_in(root, src).exists() || dev.join("pixels.json.bak").exists()
}

/// The photo's recorded baked pixel source, if it still resolves on disk:
/// `(master_path, is_generated)`. A missing sidecar is silent (the normal
/// parametric-only case); an EXISTING sidecar that cannot be honoured —
/// unreadable JSON or a deleted/moved master — degrades to `None` with a
/// stderr warning, so "the canvas reverted to the un-retouched source" stays
/// traceable instead of looking like data loss with no cause.
pub fn read_pixel_source(src: &Path) -> Option<(PathBuf, bool)> {
    let (path, generated) = recorded_pixel_source(src)?;
    if !path.exists() {
        eprintln!(
            "⚠ baked master {} is gone — the retouched canvas cannot be restored (the develop falls back to the source)",
            path.display()
        );
        return None;
    }
    // A 0-byte file at the recorded path is the CLAIM, not the master — the
    // same "the claim file is not an artifact" rule sidecar_wrote states. A
    // crash between claim and publish must not hand an empty frame to the
    // renderer as the user's retouch (L03).
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == 0) {
        eprintln!(
            "⚠ baked master {} is empty (an unfinished write) — the retouched canvas cannot be restored (the develop falls back to the source)",
            path.display()
        );
        return None;
    }
    Some((path, generated))
}

/// The RECORD-level half of [`read_pixel_source`]: what `pixels.json` says
/// the develop sits on — the master's resolved path and whether it is an
/// AI-generated raster — without asking whether that file still exists.
/// The writers that decide a develop's projection and its strip word (the
/// web save, the batch paste, [`ActiveWrite::DevelopOnAiPixels`]) need this
/// answer: a develop saved over a reimagine rendition describes those
/// pixels whether or not the PNG opens right now, and a Lightroom sidecar
/// projecting it onto the RAW would be a lie either way. Absent, unreadable
/// or off-machine records answer `None` with the same warnings.
pub fn recorded_pixel_source(src: &Path) -> Option<(PathBuf, bool)> {
    // A crashed publish leaves the linkage only in pixels.json.bak — without
    // this the canvas silently reverts to the un-retouched source. A FAILED
    // recovery is already reported by the helper; this reader then degrades
    // to "no master" exactly as it does for any unreadable sidecar.
    let _ = recover_orphan_baks(src);
    let sidecar = pixel_source_path(src);
    let bytes = match read_bytes_capped(&sidecar, MAX_STORE_JSON) {
        Ok(b) => b,
        // Missing IS the normal parametric-only case — stay silent.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        // An EXISTING sidecar we cannot read (permissions, I/O) must warn —
        // the doc above promises the revert-to-source stays traceable.
        Err(e) => {
            eprintln!(
                "⚠ {} exists but cannot be read ({e}) — the baked retouch master is not restored",
                sidecar.display()
            );
            return None;
        }
    };
    // A narrow struct, not `serde_json::Value`: a hostile pixels.json
    // amplified ~10× into a throwaway tree (L02/L16). Unknown fields still
    // pass — forward compatibility is unchanged.
    #[derive(serde::Deserialize)]
    struct PixelSourceDoc {
        origin: Option<String>,
        kind: Option<String>,
    }
    let Ok(doc) = serde_json::from_slice::<PixelSourceDoc>(&bytes) else {
        eprintln!(
            "⚠ {} is unreadable — the baked retouch master is not restored",
            sidecar.display()
        );
        return None;
    };
    let Some(origin) = doc.origin else {
        eprintln!(
            "⚠ {} has no origin field — the baked retouch master is not restored",
            sidecar.display()
        );
        return None;
    };
    let generated = match doc.kind.as_deref() {
        Some("generated") => true,
        Some("inplace") => false,
        Some(kind) => {
            eprintln!(
                "⚠ {} uses unknown baked-master kind {kind:?} — the master is not restored and the file is left untouched",
                sidecar.display()
            );
            return None;
        }
        None => {
            eprintln!(
                "⚠ {} has no kind field — the baked retouch master is not restored",
                sidecar.display()
            );
            return None;
        }
    };
    let mut path = PathBuf::from(origin);
    // LEXICAL first: probing a `\\attacker\share\…` origin with `exists()`
    // below would already send this machine's credentials outbound. A
    // develop store unzipped from someone else's pack is untrusted input.
    if remote_or_device_path(&path) {
        eprintln!(
            "⚠ {} names a network/device master path — the master is not restored (a develop store may not reach off this machine)",
            sidecar.display()
        );
        return None;
    }
    if path.is_relative() {
        let Some(contained) = contained_join(&develop_dir(src), &path) else {
            eprintln!(
                "⚠ {} contains a baked-master origin outside its develop directory — the master is not restored",
                sidecar.display()
            );
            return None;
        };
        path = contained;
    }
    Some((path, generated))
}
