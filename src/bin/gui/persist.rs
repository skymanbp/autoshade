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
    /// LR mask corrections (brush/AI/depth) the import cannot represent —
    /// its own channel, NOT `xmp_bad`: appended there it rendered as
    /// "unreadable numeric setting(s)", a warning about something else.
    /// 0 whenever the restore came from recipe.json (no LR-only masks).
    pub(crate) dropped_masks: usize,
    /// What the restore-time clamps DISCARDED (W20: sanitisation must not
    /// be silent loss) — the Opened handler's disclosure channel.
    pub(crate) clamp: autoshop::recipe::ClampSummary,
}

impl Default for RestoredDevelop {
    fn default() -> Self {
        RestoredDevelop {
            saved: SavedDevelop::Nothing,
            xmp_bad: Vec::new(),
            dropped_masks: 0,
            clamp: Default::default(),
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
    let lr = match autoshop::store::lightroom_sidecar(src) {
        autoshop::store::LrSidecar::NewerThanStore(t) => {
            Some((t, "XMP (Lightroom sidecar — newer than the saved develop; Ctrl+S adopts it)"))
        }
        autoshop::store::LrSidecar::Only(t) => {
            Some((t, "XMP (Lightroom sidecar beside the RAW)"))
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
        let mut r = autoshop::xmp::xmp_to_recipe(&text);
        xmp_bad = autoshop::xmp::unparsable_crs_numbers(&text);
        // Neutral / foreign content restores nothing — the store answers
        // exactly as before.
        if !r.is_noop() {
            clamp_dropped.absorb(r.clamp());
            let dropped = autoshop::xmp::unsupported_corrections(&text);
            return RestoredDevelop {
                saved: SavedDevelop::Restored(r, kind),
                xmp_bad,
                dropped_masks: dropped,
                clamp: clamp_dropped,
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
        let text = match std::fs::read_to_string(&rj) {
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
            ..Default::default()
        };
    }
    let mut fallback = None;
    let mut dropped_masks = 0usize;
    for (xp, kind) in [
        (autoshop::pipeline::xmp_target(src), "XMP"),
        (autoshop::store::legacy_xmp(src), "XMP (legacy ./out)"),
    ] {
        // Same rule as the recipe loop: only NotFound falls through — an
        // unreadable central XMP must not let a stale legacy one answer.
        let text = match std::fs::read_to_string(&xp) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                any = true;
                break;
            }
        };
        any = true;
        let mut r = autoshop::xmp::xmp_to_recipe(&text);
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
            clamp_dropped.absorb(r.clamp());
            dropped_masks = autoshop::xmp::unsupported_corrections(&text);
            fallback = Some((r, kind));
        }
        break; // same rule: the file that answered decides
    }
    if let Some(err) = parse_err {
        return RestoredDevelop {
            saved: SavedDevelop::Unreadable { err, fallback },
            xmp_bad,
            dropped_masks,
            clamp: clamp_dropped,
        };
    }
    match (fallback, any) {
        (Some((r, k)), _) => RestoredDevelop {
            saved: SavedDevelop::Restored(r, k),
            xmp_bad,
            dropped_masks,
            clamp: clamp_dropped,
        },
        (None, true) => RestoredDevelop {
            saved: SavedDevelop::NoopOnly,
            xmp_bad,
            clamp: clamp_dropped,
            ..Default::default()
        },
        (None, false) => RestoredDevelop {
            saved: SavedDevelop::Nothing,
            xmp_bad,
            clamp: clamp_dropped,
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
        match std::fs::read_to_string(&p) {
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
