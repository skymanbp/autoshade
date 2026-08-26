//! Saved-develop restore precedence, paste projection, dirty check.

use super::*;

/// What ./out holds for a photo — the open path and the status line key off
/// this, and every outcome is DISTINGUISHED so nothing degrades silently
/// (an unparsable recipe.json used to fall through to the lossy XMP without
/// a word, and a neutral sidecar claimed "restored saved edits").
pub(crate) enum SavedDevelop {
    /// A sidecar with real edits (kind = which file; technical name, shown
    /// untranslated in the status line).
    Restored(EditRecipe, &'static str),
    /// recipe.json exists but does not parse (interrupted write, or written
    /// by a newer build — EditRecipe is deny_unknown_fields). The XMP
    /// fallback, when it held real edits, comes along.
    Unreadable { err: String, fallback: Option<(EditRecipe, &'static str)> },
    /// Sidecars exist but describe no effective edit (Reset-then-save
    /// leftovers, or a foreign neutral XMP).
    NoopOnly,
    /// No sidecar at all.
    Nothing,
}

/// The photo's saved develop from the central store (see `autoshop::store`):
/// the app-internal `recipe.json` first (lossless — bitmap masks, recolour
/// gains, roles all round-trip), else the Lightroom-compatible `<stem>.xmp`
/// re-imported through the reverse crs mapping (globals, curves, crop,
/// parametric masks — everything classic XMP can carry). Pre-store sidecars in
/// the legacy cwd-relative ./out are migrated in on first touch; a file the
/// migration could not move keeps being read in place, and the `kind` string
/// says which file actually answered.
///
/// The disclosure channels ride along in [`RestoredDevelop`].
/// Everything a saved-develop restore hands back: the develop itself plus
/// the three restore-time disclosure channels (W20 — restore loss must never
/// be silent). Was a positional 4-tuple; the channels kept being threaded
/// by position through the fresh-open path (round-12 结构 cluster).
pub(crate) struct RestoredDevelop {
    pub(crate) saved: SavedDevelop,
    /// Unreadable numeric XMP settings, phrased for the open-note channel.
    pub(crate) xmp_bad: Vec<String>,
    /// What the sidecar's mask corrections cost on the way in, NAMED (R25
    /// P1) — its own channel, NOT `xmp_bad`: appended there it rendered as
    /// "unreadable numeric setting(s)", a warning about something else.
    /// Empty whenever the restore came from recipe.json (nothing was
    /// imported from a sidecar) or the import was faithful.
    pub(crate) mask_import: Vec<autoshop::xmp::MaskImportLoss>,
    /// How many masks that same import DID bring in — the other half of the
    /// sentence, and until R25 P1 it was always zero on a Lightroom file.
    pub(crate) imported_masks: usize,
    /// R24-5 M0, the GLOBAL counterpart of `mask_import`: `crs:` properties
    /// the sidecar carries on its own Description that this engine does not
    /// model (LR's global Texture, Grain, the Transform/Calibration blocks).
    /// The merge PRESERVES them — this is the sentence saying they are there,
    /// which no surface used to say, so a photo that renders differently in
    /// Lightroom had no explanation on screen. Its own channel, like
    /// `mask_import`: appended to `xmp_bad` it would read as "unreadable",
    /// which is the opposite of what happened.
    pub(crate) carried_globals: Vec<String>,
    /// What the restore-time clamps DISCARDED (W20: sanitisation must not
    /// be silent loss) — the Opened handler's disclosure channel.
    pub(crate) clamp: autoshop::recipe::ClampSummary,
    /// A Lightroom sidecar sits beside the RAW but could not be read (the
    /// reason, verbatim from [`autoshop::store::SidecarRead`]) — whatever it
    /// holds is NOT in `saved`, and the user must hear that (L08).
    pub(crate) lr_unreadable: Option<&'static str>,
    /// The RAW carries an embedded XMP packet that could not be read
    /// (`autoshop::store::embedded_packet_for_restore`'s `Err`) — same
    /// unreadable-is-not-absent rule as `lr_unreadable`, its own channel.
    pub(crate) packet_unreadable: Option<String>,
}

impl Default for RestoredDevelop {
    fn default() -> Self {
        RestoredDevelop {
            saved: SavedDevelop::Nothing,
            xmp_bad: Vec::new(),
            mask_import: Vec::new(),
            imported_masks: 0,
            carried_globals: Vec::new(),
            clamp: Default::default(),
            lr_unreadable: None,
            packet_unreadable: None,
        }
    }
}

pub(crate) fn read_saved_develop(src: &std::path::Path) -> RestoredDevelop {
    // The whole restore — legacy migration plus the multi-file precedence
    // walk — runs under ONE develop lock, so another process's mid-compound
    // save cannot hand back half-old, half-new answers. NoWait: this runs on
    // photo open (UI thread); a busy develop must report, not freeze the
    // window behind a lock with no cancel.
    match autoshop::store::with_develop_lock(
        src,
        autoshop::store::DevelopLockMode::NoWait,
        || Ok::<_, std::io::Error>(read_saved_develop_locked(src)),
    ) {
        Ok(answer) => answer,
        Err(e) => RestoredDevelop {
            saved: SavedDevelop::Unreadable {
                err: format!(
                    "the develop is busy in another Autoshop process ({e}); retry opening the photo"
                ),
                fallback: None,
            },
            ..Default::default()
        },
    }
}

pub(crate) fn read_saved_develop_locked(src: &std::path::Path) -> RestoredDevelop {
    let mut clamp_dropped = autoshop::recipe::ClampSummary::default();
    autoshop::store::migrate_legacy(src);
    // The sidecar BESIDE the RAW is the one file Lightroom itself writes.
    // Newest intent wins (store::lightroom_sidecar): a sidecar Lightroom
    // touched after our last save outranks the stored develop — it used to
    // be ignored entirely, so Lightroom edits silently lost to older
    // Autoshop saves. Our own copied projection is recognised and skipped;
    // the store copy stays untouched until the user actually saves.
    let mut lr_unreadable: Option<&'static str> = None;
    let lr = match autoshop::store::lightroom_sidecar(src) {
        autoshop::store::LrSidecar::NewerThanStore(t) => {
            Some((t, "XMP (Lightroom sidecar — newer than the saved develop; Ctrl+S adopts it)"))
        }
        autoshop::store::LrSidecar::Only(t) => {
            Some((t, "XMP (Lightroom sidecar beside the RAW)"))
        }
        // The file EXISTS but cannot answer — the old fold into "absent"
        // opened neutral in silence while Lightroom work might sit right
        // there (L08). The store still answers below; the note rides along.
        autoshop::store::LrSidecar::Unreadable(why) => {
            lr_unreadable = Some(why);
            None
        }
        _ => None,
    };
    // A6 disclosure accumulator: unreadable numbers in ANY consulted XMP —
    // including one whose ONLY edit is corrupt and therefore imports as a
    // NO-OP. That file restores nothing, so a per-branch warning was
    // discarded with it, and the next save overwrote the corrupt original
    // in silence (Codex 32-#1).
    let mut xmp_bad: Vec<String> = Vec::new();
    if let Some((text, kind)) = lr {
        // CLAMPED door (R28 2b): the reader clamps on its way out, so the
        // `r.clamp()` below saw an already-cut recipe and reported an empty
        // summary — a sidecar whose brush dabs were truncated on import
        // toasted nothing. Both halves are absorbed, and they cannot double
        // count: the second clamp is idempotent over the first.
        let diag = autoshop::diag::photo(src);
        let (mut r, imported_clamp) =
            autoshop::xmp::xmp_to_recipe_clamped_with_diag(&text, &diag);
        xmp_bad = autoshop::xmp::unparsable_crs_numbers(&text);
        // Neutral / foreign content restores nothing — the store answers
        // exactly as before.
        if !r.is_noop() {
            clamp_dropped.absorb(imported_clamp);
            clamp_dropped.absorb(r.clamp());
            let mask_import = autoshop::xmp::import_losses_for_photo(&text, src);
            let imported_masks = r.masks.len();
            return RestoredDevelop {
                saved: SavedDevelop::Restored(r, kind),
                xmp_bad,
                mask_import,
                imported_masks,
                carried_globals: autoshop::xmp::unmodelled_global_crs(&text),
                clamp: clamp_dropped,
                lr_unreadable,
                packet_unreadable: None,
            };
        }
    }
    let mut any = false;
    let mut parse_err: Option<String> = None;
    let mut restored: Option<(EditRecipe, &'static str)> = None;
    for (rj, kind) in [
        (autoshop::store::recipe_target(src), "recipe.json"),
        (autoshop::store::legacy_recipe(src), "recipe.json (legacy ./out)"),
    ] {
        // Only ABSENCE falls through to the next source. Any other read error
        // (permissions, transient I/O, invalid UTF-8) means the central file
        // EXISTS but can't answer — reporting it as Unreadable keeps the
        // stated precedence: a stale legacy recipe (or the lossy XMP) must
        // never silently replace an unreadable central one.
        let text = match autoshop::store::read_text_capped(&rj, autoshop::store::MAX_STORE_JSON) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                any = true;
                parse_err = Some(format!("read {}: {e}", rj.display()));
                break;
            }
        };
        any = true;
        match serde_json::from_str::<EditRecipe>(&text) {
            Ok(mut r) => {
                if !r.is_noop() {
                    clamp_dropped.absorb(r.clamp());
                    // Store recipes reference rasters by bare file name —
                    // anchor them to the file they were loaded beside.
                    if let Some(base) = rj.parent() {
                        autoshop::store::resolve_mask_paths(&mut r, base);
                    }
                    restored = Some((r, kind));
                }
                // Neutral recipe.json: fall through — an XMP with real edits
                // may still exist beside it.
            }
            Err(e) => parse_err = Some(e.to_string()),
        }
        // Whatever the central file said (restored / neutral / damaged) is the
        // answer — a stale legacy file must never shadow it.
        break;
    }
    if let Some((r, kind)) = restored {
        // recipe.json restored: lossless truth — a corrupt number in the
        // subordinate XMP is REPLACED by the next save's owned-attr merge,
        // so there is nothing to disclose on this path.
        return RestoredDevelop {
            saved: SavedDevelop::Restored(r, kind),
            clamp: clamp_dropped,
            lr_unreadable,
            ..Default::default()
        };
    }
    let mut fallback = None;
    let mut mask_import: Vec<autoshop::xmp::MaskImportLoss> = Vec::new();
    let mut imported_masks = 0usize;
    let mut carried_globals: Vec<String> = Vec::new();
    for (xp, kind) in [
        (autoshop::pipeline::xmp_target(src), "XMP"),
        (autoshop::store::legacy_xmp(src), "XMP (legacy ./out)"),
    ] {
        // Same rule as the recipe loop: only NotFound falls through — an
        // unreadable central XMP must not let a stale legacy one answer.
        let text = match autoshop::store::read_text_capped(&xp, autoshop::store::MAX_STORE_JSON) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                any = true;
                break;
            }
        };
        any = true;
        // Clamped door, same reason as the Lightroom branch above (R28 2b).
        let diag = autoshop::diag::photo(src);
        let (mut r, imported_clamp) =
            autoshop::xmp::xmp_to_recipe_clamped_with_diag(&text, &diag);
        // Accumulated regardless of noop-ness (dedup vs the LR sidecar's
        // list) — see the accumulator's contract above.
        for k in autoshop::xmp::unparsable_crs_numbers(&text) {
            if !xmp_bad.contains(&k) {
                xmp_bad.push(k);
            }
        }
        // A foreign / neutral sidecar parses to a no-op recipe — "restoring"
        // it would only produce a misleading status line.
        if !r.is_noop() {
            clamp_dropped.absorb(imported_clamp);
            clamp_dropped.absorb(r.clamp());
            mask_import = autoshop::xmp::import_losses_for_photo(&text, src);
            imported_masks = r.masks.len();
            carried_globals = autoshop::xmp::unmodelled_global_crs(&text);
            fallback = Some((r, kind));
        }
        break; // same rule: the file that answered decides
    }
    // The RAW's own embedded XMP packet — the develop Lightroom bakes INTO a
    // DNG — as the strictly LOWEST-priority source, probed only when NO store
    // file exists at all (`!any`, not merely "nothing restored": a NEUTRAL
    // recipe.json is a store file expressing neutral intent, and probing past
    // it resurrected the baked develop that a web-side neutral save had just
    // walked away from — Codex L05 EMBED-01; the clear-marker gate lives in
    // `embedded_packet_for_restore`). The common developed-photo open
    // therefore pays no TIFF walk.
    let mut packet_unreadable: Option<String> = None;
    if fallback.is_none() && !any {
        match autoshop::store::embedded_packet_for_restore(src) {
            Ok(Some(text)) => {
                // Clamped door (R28 2b), like both branches above.
                let diag = autoshop::diag::photo(src);
                let (mut r, imported_clamp) =
                    autoshop::xmp::xmp_to_recipe_clamped_with_diag(&text, &diag);
                for k in autoshop::xmp::unparsable_crs_numbers(&text) {
                    if !xmp_bad.contains(&k) {
                        xmp_bad.push(k);
                    }
                }
                if !r.is_noop() {
                    clamp_dropped.absorb(imported_clamp);
                    clamp_dropped.absorb(r.clamp());
                    mask_import = autoshop::xmp::import_losses_for_photo(&text, src);
                    imported_masks = r.masks.len();
                    carried_globals = autoshop::xmp::unmodelled_global_crs(&text);
                    fallback = Some((r, "XMP (embedded in the RAW)"));
                }
            }
            Ok(None) => {}
            Err(why) => packet_unreadable = Some(why),
        }
    }
    if let Some(err) = parse_err {
        return RestoredDevelop {
            saved: SavedDevelop::Unreadable { err, fallback },
            xmp_bad,
            mask_import,
            imported_masks,
            carried_globals: carried_globals.clone(),
            clamp: clamp_dropped,
            lr_unreadable,
            packet_unreadable,
        };
    }
    match (fallback, any) {
        (Some((r, k)), _) => RestoredDevelop {
            saved: SavedDevelop::Restored(r, k),
            xmp_bad,
            mask_import,
            imported_masks,
            carried_globals: carried_globals.clone(),
            clamp: clamp_dropped,
            lr_unreadable,
            packet_unreadable,
        },
        (None, true) => RestoredDevelop {
            saved: SavedDevelop::NoopOnly,
            xmp_bad,
            clamp: clamp_dropped,
            lr_unreadable,
            packet_unreadable,
            ..Default::default()
        },
        (None, false) => RestoredDevelop {
            saved: SavedDevelop::Nothing,
            xmp_bad,
            clamp: clamp_dropped,
            lr_unreadable,
            packet_unreadable,
            ..Default::default()
        },
    }
}


// The programmatic-overwrite backup gate lives in the LIB now
// (autoshop::store::backup_saved_develop) so the web and CLI enforce the same
// "never destroy an explicit save without a v<N> snapshot" contract — copy
// semantics + frozen rasters; see store.rs for the full contract.

/// The recipe a paste actually writes to `target`: the copied USER EDIT with
/// the base-look calibration resolved per target. `base_curve` is per-photo
/// (camera-matched at open), so:
///   * a BAKED target (PNG/JPEG/TIFF) gets NO curve — its pixels already
///     carry the camera look, and pasting a RAW's curve would apply the
///     camera tone twice;
///   * a target with a saved recipe (central store OR a not-yet-migrated
///     legacy ./out file) keeps its own curve — including the legacy empty
///     curve, which must keep rendering exactly as it was tuned;
///   * a RAW with no saved develop inherits the source's curve (same-camera
///     S curves — far closer than the dark base).
///
/// `as_shot_k`/`as_shot_tint` follow the lens-profile rule: per-photo
/// calibration (auto-WB varies shot to shot), re-resolved for the target —
/// never inherited from the source photo.
///
/// `passthrough` is per-photo too, but it is resolved BEFORE this function
/// rather than in it (`start_paste`, R25 P8): the map is Lightroom's own
/// Transform / Calibration block read off ONE document, and the answer for a
/// foreign target is "we know nothing", not "look up what the target's last
/// save happened to record". An empty map is what makes that literal — the
/// merge strips only keys the recipe carries, so the target's own block stays
/// in the target's own sidecar, byte for byte. Reading the target's saved
/// recipe.json instead would have been strictly worse: that copy can be STALE
/// against the sidecar the photographer has edited in Lightroom since, and a
/// paste would then rewrite the old values back over the new ones.
pub(crate) fn paste_recipe_for(target: &std::path::Path, pasted: &EditRecipe) -> EditRecipe {
    let mut r = pasted.clone();
    if !autoshop::decode::is_raw(target) {
        r.base_curve = Vec::new();
        r.lens_profile = Default::default();
        // A baked image carries no camera WB metadata to anchor on.
        r.as_shot_k = None;
        r.as_shot_tint = None;
        return r;
    }
    for p in [
        autoshop::store::recipe_target(target),
        autoshop::store::legacy_recipe(target),
    ] {
        match autoshop::store::read_text_capped(&p, autoshop::store::MAX_STORE_JSON) {
            Ok(text) => {
                if let Ok(mut saved) = serde_json::from_str::<EditRecipe>(&text) {
                    // Repair BEFORE adopting, and carry the era stamp WITH the
                    // curve. The stamp describes the curve's provenance, and
                    // the curve comes from the target — keeping the pasted
                    // recipe's version 2 on top of the target's era-1 washed
                    // curve laundered it into a file every repair then
                    // declines forever (the version gate short-circuits), so
                    // the photo rendered several stops dark on every surface,
                    // permanently, with nothing left to detect it by. Pasting
                    // onto the OPEN photo did it immediately: the canvas had
                    // been repaired, this re-read the still-washed file and
                    // overwrote it, and the save baselined that as correct.
                    // Synchronous on the UI thread, accepted: the memo
                    // bounds it to one estimate per photo per process, paste
                    // is an explicit and infrequent action, and the
                    // mask-list hover's reference develop set the precedent
                    // for paying a bounded develop where correctness needs
                    // it.
                    let _ = autoshop::pipeline::repair_pre_era_base_curve(target, &mut saved);
                    r.version = saved.version;
                    r.base_curve = saved.base_curve;
                    // A saved develop owns its profile verbatim — including any
                    // user toggle and the legacy default-off.
                    r.lens_profile = saved.lens_profile;
                    // …and its as-shot anchor (or its legacy None) verbatim:
                    // the SOURCE photo's anchor is a different exposure.
                    r.as_shot_k = saved.as_shot_k;
                    r.as_shot_tint = saved.as_shot_tint;
                    return r;
                }
                // The file that answered is DAMAGED: stop here and fall to the
                // fresh-derive path below — consulting a stale legacy file
                // would violate the central-first precedence, exactly like
                // read_saved_develop's rule.
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => break, // unreadable ≠ absent — same precedence rule
        }
    }
    // No saved develop: the SOURCE photo's correction data is meaningless on
    // a different lens/focal — re-derive the target's own, stamped default-on
    // (paste transfers EDITS; the profile is per-photo calibration).
    r.lens_profile = autoshop::pipeline::fresh_lens_profile(target);
    let (ask, ast) = autoshop::pipeline::fresh_as_shot_wb(target);
    r.as_shot_k = ask;
    r.as_shot_tint = ast;
    r
}

/// "Does the canvas hold real USER edits vs this baseline?" — the ● marker,
/// the quit guard and the nav stash all key off this. `base_curve` is the
/// photo's camera calibration, not an edit: a curve-only difference (a fresh
/// stamp over a neutral-cleared baseline, a Generated variant's empty curve,
/// a paste that resolved the target's own curve) must not light ●, block
/// quit, or stash a canvas — that was exactly the "permanent ● with no way
/// to clear it" regression the adversarial review caught. `version` is the
/// SAME calibration's provenance stamp (the pre-era repair bumps it with the
/// curve), so it is neutralised with it: a repaired canvas against an
/// unrepaired baseline — either direction: the stash gate produces
/// baseline-v1 vs canvas-v2, and load_version's rightly-declined pre-era
/// snapshots produce the reverse — must not read as an edit, or the photo
/// stays "unsaved" all session and Save-all writes sidecars for zero user
/// edits.
pub(crate) fn dirty_vs(canvas: &EditRecipe, baseline: &EditRecipe) -> bool {
    canvas != baseline
        && EditRecipe { version: 0, base_curve: Vec::new(), ..canvas.clone() }
            != EditRecipe { version: 0, base_curve: Vec::new(), ..baseline.clone() }
}

/// Append one more open-note sentence — notes chain with a middle dot.
pub(crate) fn merge_note(note: Option<String>, extra: String) -> Option<String> {
    Some(match note {
        Some(o) => format!("{o} · {extra}"),
        None => extra,
    })
}

/// What resolving the saved develop decided for the canvas — the decision
/// half of the Opened restore (a defect hotspot across rounds), separated
/// from the canvas mutations so it reads and tests on its own.
pub(crate) struct ResolvedSaved {
    pub(crate) recipe: EditRecipe,
    /// Which file answered (`kind`), for the ready-status line.
    pub(crate) restored: Option<&'static str>,
    /// Whether to stamp this photo's camera-matched base look onto the
    /// recipe: yes for fresh opens and XMP-only restores (Lightroom tuned
    /// those against its own camera-profile base — the bright base is the
    /// closer rendering); NO for recipe.json, which keeps its saved curve
    /// verbatim so legacy develops render exactly as they were tuned.
    pub(crate) stamp: bool,
    pub(crate) open_note: Option<String>,
    /// An explicit save over a develop we never successfully READ must
    /// version what it overwrites — save_xmp runs the backup gate while
    /// this stands, so the unread save survives as v<N>.
    pub(crate) unresolved: bool,
}

/// Decide the canvas recipe from the saved develop. `skip_repair` folds the
/// two caller-side reasons not to pay the repair decode (a GENERATED canvas
/// strips its calibration later, and a session nav-stash about to override
/// the canvas wholesale discards the repair); the repair itself reads the
/// photo through [`autoshop::pipeline::repair_pre_era_base_curve`].
pub(crate) fn resolve_saved_develop(
    lang: Lang,
    saved: SavedDevelop,
    skip_repair: bool,
    src: Option<&std::path::Path>,
) -> ResolvedSaved {
    let mut restored: Option<&'static str> = None;
    let mut open_note: Option<String> = None;
    let mut recipe = EditRecipe::default();
    let mut stamp = true;
    let mut unresolved = false;
    match saved {
        SavedDevelop::Restored(r, kind) => {
            recipe = r;
            restored = Some(kind);
            stamp = kind.starts_with("XMP");
            // Only the recipe.json path can carry a washed-frame base curve
            // to the canvas: an XMP restore re-stamps the curve anyway
            // (stamp == true). Its own sentence, in the open-note channel —
            // appended to `xmp_bad` it was rendered as "unreadable XMP
            // numeric setting(s) … restored as neutral", a warning about
            // something else entirely.
            if !stamp
                && !skip_repair
                && src.is_some_and(|p| {
                    autoshop::pipeline::repair_pre_era_base_curve(p, &mut recipe).is_some()
                })
            {
                // The GUI's OWN sentence, localized — every other open-note
                // here goes through tr/trf; the engine note is for the
                // CLI/HTTP surfaces.
                open_note = Some(
                    tr(
                        lang,
                        "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                    )
                    .into(),
                );
            }
        }
        SavedDevelop::Unreadable { err, fallback } => {
            // A damaged / newer-build recipe.json must never degrade to the
            // lossy XMP silently: bitmap masks and recolour gains would be
            // gone and the next Ctrl+S would overwrite the still-good file.
            if let Some((r, kind)) = fallback {
                recipe = r;
                restored = Some(kind);
                stamp = kind.starts_with("XMP");
            }
            unresolved = true;
            open_note = Some(trf(
                lang,
                "recipe.json is unreadable ({err}) — edits NOT fully restored; Ctrl+S would overwrite it (the unread save is backed up as a version first)",
                &[("err", &err)],
            ));
        }
        SavedDevelop::NoopOnly => {
            open_note = noop_only_note(lang, src);
        }
        SavedDevelop::Nothing => {}
    }
    // COORDINATE FRAME, for whichever arm produced a recipe — and NOT gated on
    // `skip_repair` or on `stamp`, unlike the curve repair above:
    //   * `skip_repair` exists to dodge a RAW DECODE + develop; this reads EXIF
    //     metadata only, and the two callers that set it (a generated canvas,
    //     a nav-stash) still render this recipe's crop and masks;
    //   * the XMP arms need no gate of their own — `xmp_to_recipe` builds on
    //     `EditRecipe::default()`, which is era-current, so an imported
    //     Lightroom sidecar (whose coordinates ARE display-frame, because
    //     Lightroom reads tag 0x0112 itself) is a no-op here by construction;
    //   * the `Unreadable` fallback arm carries a real restored recipe too.
    if let Some(c) =
        src.and_then(|p| autoshop::pipeline::migrate_recipe_coord_frame(p, &mut recipe))
    {
        open_note = merge_note(open_note, coord_migration_sentence(lang, c));
    }
    ResolvedSaved { recipe, restored, stamp, open_note, unresolved }
}

/// The open-note for a develop whose ACTIVE card restored as a no-op.
///
/// The sentence it can emit claims something about the WHOLE saved develop,
/// so it must be decided from the whole store — the active card's own files
/// (already judged by the caller), the variant strip, and the baked pixel
/// master. Deciding it from recipe.json alone reported "holds no effective
/// edits" on photos whose saved work lives in a background variant card or
/// in baked pixels (user report, 2026-08-25). The reader widens; recipe.json
/// is NOT rewritten — it stays the active card's sole authority.
fn noop_only_note(lang: Lang, src: Option<&std::path::Path>) -> Option<String> {
    // A recorded pixel master IS saved work, and the canvas shows it —
    // there is no neutral-looking open to explain.
    if src.is_some_and(autoshop::store::has_pixel_source) {
        return None;
    }
    match src.map(autoshop::store::read_variants_checked) {
        Some(autoshop::store::VariantsRead::Strip(rec)) => {
            // A background card holds work when its recipe edits or when it
            // rides its own baked raster (whose recipe may be neutral).
            let with_work =
                rec.others.iter().filter(|v| !v.recipe.is_noop() || v.origin.is_some()).count();
            if with_work > 0 {
                return Some(trf(
                    lang,
                    "the current variant holds no edits; this photo's saved edits live in {n} background variant(s)",
                    &[("n", &with_work.to_string())],
                ));
            }
        }
        // The strip file exists but cannot be read: unknown is not "no
        // edits", and the Opened handler toasts that state in its own words.
        Some(autoshop::store::VariantsRead::Unresolved) => return None,
        _ => {}
    }
    Some(tr(lang, "a saved develop exists but holds no effective edits").into())
}

/// The GUI's OWN localized sentence for a coordinate-frame migration — the
/// engine's [`autoshop::pipeline::coord_migration_note`] is for the CLI/HTTP
/// surfaces, exactly like the base-curve pair. Two sentences, not one with a
/// conditional clause: the raster half is a DIFFERENT fact (work the migration
/// could not do) and must not be buried inside the success line.
pub(crate) fn coord_migration_sentence(
    lang: Lang,
    c: autoshop::pipeline::CoordMigration,
) -> String {
    let mut s: String = tr(
        lang,
        "this photo's saved crop and masks were rotated to match the RAW's EXIF orientation — earlier versions displayed rotated RAWs sideways, so their coordinates were stored against the sideways frame",
    )
    .into();
    if c.rasters_left {
        s.push_str(" · ");
        s.push_str(tr(
            lang,
            "its raster masks are image files, not coordinates, and could NOT be rotated — check them and re-generate if they no longer fit",
        ));
    }
    s
}

/// Stamp the photo's camera-matched calibration onto the canvas recipe:
/// base curve, in-camera lens profile, and — only when unpinned — the
/// as-shot WB anchor (an old-era Autoshop XMP restore arrives with the
/// 5500 anchor PINNED by xmp_to_recipe; overwriting the pin would
/// reinterpret the saved look).
pub(crate) fn stamp_calibration(
    recipe: &mut EditRecipe,
    knots: &[[f32; 2]],
    lens: &autoshop::recipe::LensProfile,
    as_shot: Option<(f32, f32)>,
) {
    if !knots.is_empty() {
        recipe.base_curve = knots.to_vec();
    }
    recipe.lens_profile = lens.clone();
    if recipe.as_shot_k.is_none()
        && let Some((k, t)) = as_shot
    {
        recipe.as_shot_k = Some(k);
        recipe.as_shot_tint = Some(t);
    }
}

/// Reconcile a LOADED SNAPSHOT's calibration with the card it lands on
/// (R24-3) — the two directions of one rule, in one place.
///
/// A version snapshot is a develop taken off SOME card, and calibration
/// (camera base curve, in-camera lens profile, as-shot anchor) belongs to the
/// card's PIXELS, never to the snapshot:
///
/// * onto a PIXEL-STATE card the calibration is STRIPPED — its raster already
///   carries the camera look, the lens geometry and a baked white balance, so
///   rendering them again cooks each twice;
/// * off a pixel-state card the snapshot arrives WITHOUT any (the rule above
///   applied when it was saved) and its era stamp is current by default, so
///   loading it onto a parametric card left the negative rendering with no
///   camera base look at all and the pre-era repair declined it twice over
///   (stamp current, curve shorter than three knots). That direction had no
///   owner until R24-3; this is it.
///
/// `calibration` is a closure so the photo's estimate — a RAW decode — is
/// paid only in the branch that needs it. Returns whether a stamp happened,
/// for the caller's disclosure.
pub(crate) fn reconcile_snapshot_calibration(
    r: &mut EditRecipe,
    onto_pixel_state: bool,
    calibration: impl FnOnce() -> (Vec<[f32; 2]>, autoshop::recipe::LensProfile, Option<(f32, f32)>),
) -> bool {
    if onto_pixel_state {
        r.base_curve = Vec::new();
        r.lens_profile = Default::default();
        // The anchor too: baked pixels carry a baked white balance, so an
        // absolute-Kelvin claim over them would be false — a generated canvas
        // keeps the relative 5500 model.
        r.as_shot_k = None;
        r.as_shot_tint = None;
        return false;
    }
    if !r.base_curve.is_empty() {
        return false; // the snapshot brought its own calibration
    }
    let (knots, lens, as_shot) = calibration();
    let stamped = !knots.is_empty();
    stamp_calibration(r, &knots, &lens, as_shot);
    if stamped {
        // The era stamp rides WITH the curve — the paste rule every other
        // stamper follows (`pipeline::stamp_fit_calibration`,
        // `fresh_photo_calibration`, `produce_recipe`'s fresh arm). These
        // knots are THIS build's own estimate, so the recipe's calibration
        // provenance is this build's era.
        //
        // The OPEN path reaches the same state structurally rather than by
        // assignment: its `stamp_calibration` runs on `EditRecipe::default()`
        // or an XMP restore, and both are `CALIB_ERA` already. A SNAPSHOT is
        // the one input that arrives carrying an older era, and leaving era-1
        // over a freshly estimated curve made the caller's
        // `repair_pre_era_base_curve` re-estimate it (memo-cheap, and it
        // returns the same knots — so no double estimate was ever observed)
        // and then announce a re-estimate that had not happened.
        r.version = autoshop::recipe::CALIB_ERA;
    }
    stamped
}

/// Rebuild the background-variant strip from the persisted record: kind
/// strings parse (an unknown kind drops that variant LOUDLY), identities and
/// names come back with their cards (R24-2 — an id-less legacy entry is
/// minted one here, or the version snapshots taken from it could never be
/// attributed again), and every PARAMETRIC recipe heals its pre-era base
/// curve — a strip entry is one click from BEING the canvas. Pixel-state
/// entries carry empty curves by invariant and are skipped
/// ([`VariantKind::is_parametric`]).
pub(crate) fn strip_from_record(
    rec: &autoshop::store::VariantsRecord,
    src: Option<&std::path::Path>,
) -> Vec<Variant> {
    let mut strip: Vec<Variant> = rec
        .others
        .iter()
        .filter_map(|e| {
            let Some(kind) = VariantKind::from_store_str(&e.kind) else {
                eprintln!(
                    "⚠ variants.json entry with unknown kind {:?} — that variant is not restored",
                    e.kind
                );
                return None;
            };
            Some(Variant {
                kind,
                // Hop 2 of 6 (R24-2): disk → live strip.
                id: variant_id_or_mint(e.id.as_ref()),
                name: e.name.clone(),
                recipe: e.recipe.clone(),
                base: None,
                origin: e.origin.clone(),
                thumb: None,
            })
        })
        .collect();
    if let Some(p) = src {
        for v in strip.iter_mut() {
            if v.kind.is_parametric() {
                let _ = autoshop::pipeline::repair_pre_era_base_curve(p, &mut v.recipe);
            }
            // The coordinate frame, for EVERY card including the pixel-state
            // ones: a generated variant carries no base curve (hence the
            // parametric gate above) but it does carry the crop and the masks
            // the user drew, and `variants.json` is a FILE that can predate
            // the orientation fix like any other. NOT disclosed per card —
            // the frame is a property of the PHOTO, and the open path's own
            // migration says it once for all of them (N identical toasts for
            // one fact was the noise the strip's other corrections avoid).
            let _ = autoshop::pipeline::migrate_recipe_coord_frame(p, &mut v.recipe);
        }
    }
    strip
}
