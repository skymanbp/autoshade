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
    /// by a newer build — EditRecipe is deny_unknown_fields), or a central
    /// store file (recipe.json or XMP) exists but cannot be read. The XMP
    /// fallback, when it held real edits, comes along.
    Unreadable { err: String, fallback: Option<(EditRecipe, &'static str)> },
    /// Sidecars exist but describe no effective edit (Reset-then-save
    /// leftovers, or a foreign neutral XMP).
    NoopOnly,
    /// No sidecar at all.
    Nothing,
}

/// The photo's saved develop from the central store (see `autoshade::store`):
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
    pub(crate) mask_import: Vec<autoshade::xmp::MaskImportLoss>,
    /// How many masks that same import DID bring in — the other half of the
    /// sentence, and until R25 P1 it was always zero on a Lightroom file.
    pub(crate) imported_masks: usize,
    /// R24-5 M0, the GLOBAL counterpart of `mask_import`: `crs:` properties
    /// the sidecar carries on its own Description that this engine does not
    /// model (LR's creative Look, its `CameraProfileDigest`, a key from a
    /// newer Camera Raw than this reader knows).
    /// The merge PRESERVES them — this is the sentence saying they are there,
    /// which no surface used to say, so a photo that renders differently in
    /// Lightroom had no explanation on screen. Its own channel, like
    /// `mask_import`: appended to `xmp_bad` it would read as "unreadable",
    /// which is the opposite of what happened.
    pub(crate) carried_globals: Vec<String>,
    /// What the restore-time clamps DISCARDED (W20: sanitisation must not
    /// be silent loss) — the Opened handler's disclosure channel.
    pub(crate) clamp: autoshade::recipe::ClampSummary,
    /// A Lightroom sidecar sits beside the RAW but could not be read (the
    /// reason, verbatim from [`autoshade::store::SidecarRead`]) — whatever it
    /// holds is NOT in `saved`, and the user must hear that (L08).
    pub(crate) lr_unreadable: Option<&'static str>,
    /// The RAW carries an embedded XMP packet that could not be read
    /// (`autoshade::store::embedded_packet_for_restore`'s `Err`) — same
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
    match autoshade::store::with_develop_lock(
        src,
        autoshade::store::DevelopLockMode::NoWait,
        || Ok::<_, std::io::Error>(read_saved_develop_locked(src)),
    ) {
        Ok(answer) => answer,
        Err(e) => RestoredDevelop {
            saved: SavedDevelop::Unreadable {
                err: format!(
                    "the develop is busy in another AutoShade process ({e}); retry opening the photo"
                ),
                fallback: None,
            },
            ..Default::default()
        },
    }
}

pub(crate) fn read_saved_develop_locked(src: &std::path::Path) -> RestoredDevelop {
    let mut clamp_dropped = autoshade::recipe::ClampSummary::default();
    autoshade::store::migrate_legacy(src);
    // The sidecar BESIDE the RAW is the one file Lightroom itself writes.
    // Newest intent wins (store::lightroom_sidecar): a sidecar Lightroom
    // touched after our last save outranks the stored develop — it used to
    // be ignored entirely, so Lightroom edits silently lost to older
    // AutoShade saves. Our own copied projection is recognised and skipped;
    // the store copy stays untouched until the user actually saves.
    let mut lr_unreadable: Option<&'static str> = None;
    let lr = match autoshade::store::lightroom_sidecar(src) {
        autoshade::store::LrSidecar::NewerThanStore(t) => {
            Some((t, "XMP (Lightroom sidecar — newer than the saved develop; Ctrl+S adopts it)"))
        }
        autoshade::store::LrSidecar::Only(t) => {
            Some((t, "XMP (Lightroom sidecar beside the RAW)"))
        }
        // The file EXISTS but cannot answer — the old fold into "absent"
        // opened neutral in silence while Lightroom work might sit right
        // there (L08). The store still answers below; the note rides along.
        autoshade::store::LrSidecar::Unreadable(why) => {
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
        let diag = autoshade::diag::photo(src);
        let (mut r, imported_clamp) =
            autoshade::xmp::xmp_to_recipe_clamped_with_diag(&text, &diag);
        xmp_bad = autoshade::xmp::unparsable_crs_numbers(&text);
        // Neutral / foreign content restores nothing — the store answers
        // exactly as before.
        if !r.is_noop() {
            clamp_dropped.absorb(imported_clamp);
            clamp_dropped.absorb(r.clamp());
            let mask_import = autoshade::xmp::import_losses_for_photo(&text, src);
            let imported_masks = r.masks.len();
            return RestoredDevelop {
                saved: SavedDevelop::Restored(r, kind),
                xmp_bad,
                mask_import,
                imported_masks,
                carried_globals: autoshade::xmp::unmodelled_global_crs(&text),
                clamp: clamp_dropped,
                lr_unreadable,
                packet_unreadable: None,
            };
        }
    }
    let mut any = false;
    let mut neutral = false;
    let mut parse_err: Option<String> = None;
    let mut restored: Option<(EditRecipe, &'static str)> = None;
    for (rj, kind) in [
        (autoshade::store::recipe_target(src), "recipe.json"),
        (autoshade::store::legacy_recipe(src), "recipe.json (legacy ./out)"),
    ] {
        // Only ABSENCE falls through to the next source. Any other read error
        // (permissions, transient I/O, invalid UTF-8) means the central file
        // EXISTS but can't answer — reporting it as Unreadable keeps the
        // stated precedence: a stale legacy recipe (or the lossy XMP) must
        // never silently replace an unreadable central one.
        let text = match autoshade::store::read_text_capped(&rj, autoshade::store::MAX_STORE_JSON) {
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
                        autoshade::store::resolve_mask_paths(&mut r, base);
                    }
                    restored = Some((r, kind));
                } else {
                    // A NEUTRAL recipe.json is the store's answer, and the
                    // answer is "neutral" (2026-09-13). The projection beside
                    // it is DERIVED from it — written or cleared in the same
                    // commit generation — so a projection that disagrees can
                    // only be stale, and walking on into it is how a sidecar
                    // left by a deleted reverse-fit card restored its +90
                    // saturation over a pristine AI-generated card on open.
                    neutral = true;
                }
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
    if neutral {
        // The projection is not consulted (see above); the Lightroom
        // sidecar's own disclosures, gathered before the store walk, ride.
        return RestoredDevelop {
            saved: SavedDevelop::NoopOnly,
            xmp_bad,
            clamp: clamp_dropped,
            lr_unreadable,
            ..Default::default()
        };
    }
    let mut fallback = None;
    let mut mask_import: Vec<autoshade::xmp::MaskImportLoss> = Vec::new();
    let mut imported_masks = 0usize;
    let mut carried_globals: Vec<String> = Vec::new();
    for (xp, kind) in [
        (autoshade::pipeline::xmp_target(src), "XMP"),
        (autoshade::store::legacy_xmp(src), "XMP (legacy ./out)"),
    ] {
        // Same rule as the recipe loop: only NotFound falls through — an
        // unreadable central XMP must not let a stale legacy one answer, and
        // it is REPORTED like an unreadable recipe.json. `any` alone folded it
        // into NoopOnly: the open note said nothing and the next Ctrl+S
        // overwrote the file without the version backup `Unreadable` gates.
        // A recipe.json error already in hand stays the headline.
        let text = match autoshade::store::read_text_capped(&xp, autoshade::store::MAX_STORE_JSON) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                any = true;
                if parse_err.is_none() {
                    parse_err = Some(format!("read {}: {e}", xp.display()));
                }
                break;
            }
        };
        any = true;
        // Clamped door, same reason as the Lightroom branch above (R28 2b).
        let diag = autoshade::diag::photo(src);
        let (mut r, imported_clamp) =
            autoshade::xmp::xmp_to_recipe_clamped_with_diag(&text, &diag);
        // Accumulated regardless of noop-ness (dedup vs the LR sidecar's
        // list) — see the accumulator's contract above.
        for k in autoshade::xmp::unparsable_crs_numbers(&text) {
            if !xmp_bad.contains(&k) {
                xmp_bad.push(k);
            }
        }
        // A foreign / neutral sidecar parses to a no-op recipe — "restoring"
        // it would only produce a misleading status line.
        if !r.is_noop() {
            clamp_dropped.absorb(imported_clamp);
            clamp_dropped.absorb(r.clamp());
            mask_import = autoshade::xmp::import_losses_for_photo(&text, src);
            imported_masks = r.masks.len();
            carried_globals = autoshade::xmp::unmodelled_global_crs(&text);
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
        match autoshade::store::embedded_packet_for_restore(src) {
            Ok(Some(text)) => {
                // Clamped door (R28 2b), like both branches above.
                let diag = autoshade::diag::photo(src);
                let (mut r, imported_clamp) =
                    autoshade::xmp::xmp_to_recipe_clamped_with_diag(&text, &diag);
                for k in autoshade::xmp::unparsable_crs_numbers(&text) {
                    if !xmp_bad.contains(&k) {
                        xmp_bad.push(k);
                    }
                }
                if !r.is_noop() {
                    clamp_dropped.absorb(imported_clamp);
                    clamp_dropped.absorb(r.clamp());
                    mask_import = autoshade::xmp::import_losses_for_photo(&text, src);
                    imported_masks = r.masks.len();
                    carried_globals = autoshade::xmp::unmodelled_global_crs(&text);
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
// (autoshade::store::backup_saved_develop) so the web and CLI enforce the same
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
/// Transform block and camera profile name read off ONE document, and the
/// answer for a
/// foreign target is "we know nothing", not "look up what the target's last
/// save happened to record". An empty map is what makes that literal — the
/// merge strips only keys the recipe carries, so the target's own block stays
/// in the target's own sidecar, byte for byte. Reading the target's saved
/// recipe.json instead would have been strictly worse: that copy can be STALE
/// against the sidecar the photographer has edited in Lightroom since, and a
/// paste would then rewrite the old values back over the new ones.
pub(crate) fn paste_recipe_for(target: &std::path::Path, pasted: &EditRecipe) -> EditRecipe {
    let mut r = pasted.clone();
    if !autoshade::decode::is_raw(target) {
        r.base_curve = Vec::new();
        r.lens_profile = Default::default();
        // A baked image carries no camera WB metadata to anchor on.
        r.as_shot_k = None;
        r.as_shot_tint = None;
        return r;
    }
    for p in [
        autoshade::store::recipe_target(target),
        autoshade::store::legacy_recipe(target),
    ] {
        match autoshade::store::read_text_capped(&p, autoshade::store::MAX_STORE_JSON) {
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
                    let _ = autoshade::pipeline::repair_pre_era_base_curve(target, &mut saved);
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
    r.lens_profile = autoshade::pipeline::fresh_lens_profile(target);
    let (ask, ast) = autoshade::pipeline::fresh_as_shot_wb(target);
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
/// photo through [`autoshade::pipeline::repair_pre_era_base_curve`].
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
                    autoshade::pipeline::repair_pre_era_base_curve(p, &mut recipe).is_some()
                })
            {
                // The GUI's OWN sentence, localized — every other open-note
                // here goes through tr/trf; the engine note is for the
                // CLI/HTTP surfaces.
                open_note = Some(
                    tr(
                        lang,
                        "camera base look re-estimated — this photo was saved by an earlier version, whose estimate of the camera's tone this version replaces",
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
                "the saved develop is unreadable ({err}) — edits NOT fully restored; Ctrl+S would overwrite it (the unread save is backed up as a version first)",
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
    match src.map(|p| autoshade::pipeline::migrate_recipe_coord_frame(p, &mut recipe)) {
        Some(Ok(Some(c))) => open_note = merge_note(open_note, coord_migration_sentence(lang, c)),
        // A raster that could not be re-written: nothing moved, and the photo
        // opens with its saved geometry off the picture — said in the same
        // note, so the photographer knows what they are looking at.
        Some(Err(e)) => {
            open_note = merge_note(open_note, coord_migration_failure_sentence(lang, &e.to_string()))
        }
        _ => {}
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
    if src.is_some_and(autoshade::store::has_pixel_source) {
        return None;
    }
    match src.map(autoshade::store::read_variants_checked) {
        Some(autoshade::store::VariantsRead::Strip(rec)) => {
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
        Some(autoshade::store::VariantsRead::Unresolved) => return None,
        _ => {}
    }
    Some(tr(lang, "a saved develop exists but holds no effective edits").into())
}

/// The GUI's OWN localized sentences for a coordinate-frame migration — the
/// engine's [`autoshade::pipeline::coord_migration_note`] is for the CLI/HTTP
/// surfaces, exactly like the base-curve pair. One sentence per fact, never
/// one with conditional clauses: the turn, the translation and the raster
/// rewrite are three different things that happened.
pub(crate) fn coord_migration_sentence(
    lang: Lang,
    c: autoshade::pipeline::CoordMigration,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.turned() {
        parts.push(
            tr(
                lang,
                "this photo's saved crop and masks were rotated to match the RAW's EXIF orientation — earlier versions displayed rotated RAWs sideways, so their coordinates were stored against the sideways frame",
            )
            .into(),
        );
    }
    if c.shift != (0, 0) {
        parts.push(trf(
            lang,
            "this photo's saved crop, masks and retouches were moved by ({dx}, {dy}) px — earlier versions developed this camera's RAWs from the sensor's corner instead of the crop the camera declares, so everything drawn on them sat that far off the picture",
            &[("dx", &c.shift.0.to_string()), ("dy", &c.shift.1.to_string())],
        ));
    }
    if c.rasters > 0 {
        parts.push(trf(
            lang,
            "{n} raster mask file(s) were re-written into the corrected frame; the originals are kept",
            &[("n", &c.rasters.to_string())],
        ));
    }
    parts.join(" · ")
}

/// The GUI's sentence for a coordinate-frame migration that could NOT run —
/// a raster mask unreadable or unwritable. Nothing moved, and the photo is
/// showing its saved geometry off the picture until the cause is fixed.
pub(crate) fn coord_migration_failure_sentence(lang: Lang, err: &str) -> String {
    trf(
        lang,
        "this photo's saved geometry could NOT be brought into the corrected frame ({err}) — it is left as saved and will be tried again on the next open",
        &[("err", err)],
    )
}

/// Stamp the photo's camera-matched calibration onto the canvas recipe:
/// base curve, in-camera lens profile, and — only when unpinned — the
/// as-shot WB anchor (an old-era AutoShade XMP restore arrives with the
/// 5500 anchor PINNED by xmp_to_recipe; overwriting the pin would
/// reinterpret the saved look).
pub(crate) fn stamp_calibration(
    recipe: &mut EditRecipe,
    knots: &[[f32; 2]],
    lens: &autoshade::recipe::LensProfile,
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
    calibration: impl FnOnce() -> (Vec<[f32; 2]>, autoshade::recipe::LensProfile, Option<(f32, f32)>),
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
        r.version = autoshade::recipe::CALIB_ERA;
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
/// ([`VariantKind::is_source_based`]).
/// …and, beside the strip, what the restore-time clamps DISCARDED from its
/// cards (summed over them), for the same W20 channel the active card's
/// losses reach: a stored card past the caps restores minus edits, and
/// until 2026-09-24 the strip's losses had no channel at all.
pub(crate) fn strip_from_record(
    rec: &autoshade::store::VariantsRecord,
    src: Option<&std::path::Path>,
) -> (Vec<Variant>, autoshade::recipe::ClampSummary) {
    let mut dropped = autoshade::recipe::ClampSummary::default();
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
            // Untrusted input like the active card's recipe.json, which
            // `read_saved_develop_locked` clamps on the way in — a card is one
            // click from BEING the canvas, and an unclamped crop (wider than
            // the frame, or NaN) reached `upd_shortcuts`' nudge maths and
            // panicked there. The summary has no per-card channel here.
            let mut recipe = e.recipe.clone();
            dropped.absorb(recipe.clamp());
            Some(Variant {
                kind,
                // Hop 2 of 6 (R24-2): disk → live strip.
                id: variant_id_or_mint(e.id.as_ref()),
                name: e.name.clone(),
                recipe,
                base: None,
                origin: e.origin.clone(),
                thumb: None,
            })
        })
        .collect();
    if let Some(p) = src {
        for v in strip.iter_mut() {
            if v.kind.is_source_based() {
                let _ = autoshade::pipeline::repair_pre_era_base_curve(p, &mut v.recipe);
            }
            // The coordinate frame, for EVERY card including the pixel-state
            // ones: a generated variant carries no base curve (hence the
            // parametric gate above) but it does carry the crop and the masks
            // the user drew, and `variants.json` is a FILE that can predate
            // the orientation fix like any other. NOT disclosed per card —
            // the frame is a property of the PHOTO, and the open path's own
            // migration says it once for all of them (N identical toasts for
            // one fact was the noise the strip's other corrections avoid).
            let _ = autoshade::pipeline::migrate_recipe_coord_frame(p, &mut v.recipe);
        }
    }
    (strip, dropped)
}

// ── AI masks imported from Lightroom: re-derived on this computer ───────────
//
// A Lightroom sidecar's AI masks (select sky / subject / object) carry the
// intent only — `xmp` reads them with `raster = None` — and the render SKIPS
// an AI mask that has no alpha (`render::mask_raster`). The CLI re-derives
// them at develop time (`segment::resolve_ai_masks`); the app does the same:
// once the open landed (a sidecar call, so on a worker, with the status
// line), and on the export's own copy of the recipe (`export.rs`), so a
// photo never opened this session exports with its masks too. A face / eyes
// / skin mask that names no raster yet (`face::is_face_intent`) is the same
// case, drawn by the face finder: both go through the library's one
// resolve-before-develop step, `pipeline::resolve_develop_masks`.

/// What `face_outcomes` hands the landing.
pub(crate) type FaceOutcomes = Vec<Option<autoshade::recipe::LocalAdjustment>>;

/// What re-derives a recipe's unresolved AI masks and face intents: the
/// photo (the cache's key and home), the pixels the render uses, the recipe.
pub(crate) type AiMaskRunner =
    fn(&std::path::Path, &std::path::Path, &mut EditRecipe) -> autoshade::pipeline::DevelopMasks;

/// The production runner: `pipeline::resolve_develop_masks`, as `apply` calls it.
pub(crate) fn resolve_ai_masks_locally(
    photo: &std::path::Path,
    pixels: &std::path::Path,
    recipe: &mut EditRecipe,
) -> autoshade::pipeline::DevelopMasks {
    autoshade::pipeline::resolve_develop_masks(&autoshade::config::Config::load(), photo, pixels, recipe)
}

/// Every geometry of `r` — each mask's own and its components'.
fn geometries(r: &EditRecipe) -> impl Iterator<Item = &autoshade::recipe::MaskGeometry> {
    r.masks.iter().flat_map(|m| std::iter::once(&m.mask).chain(m.components.iter().map(|c| &c.geometry)))
}

/// How many AI masks of `r` have no alpha yet.
pub(crate) fn unresolved_ai_masks(r: &EditRecipe) -> usize {
    geometries(r)
        .filter(|g| matches!(g, autoshade::recipe::MaskGeometry::AiMask { raster: None, .. }))
        .count()
}

/// How many masks of `r` the resolve step has work for: AI masks without an
/// alpha and face intents without a raster.
pub(crate) fn unresolved_develop_masks(r: &EditRecipe) -> usize {
    unresolved_ai_masks(r) + r.masks.iter().filter(|m| autoshade::face::is_face_intent(m)).count()
}

/// What the resolve step made of each face intent of `before`, in recipe
/// order: the mask it became (`Some`), or `None` when it was taken out.
/// `after` is `before` resolved — `face::resolve_face_masks` keeps every
/// other mask, in order, and only re-points or drops the intents.
pub(crate) fn face_outcomes(before: &EditRecipe, after: &EditRecipe) -> FaceOutcomes {
    let mut rest = after.masks.iter().peekable();
    let mut out = Vec::new();
    for m in &before.masks {
        if !autoshade::face::is_face_intent(m) {
            rest.next();
            continue;
        }
        let became = rest.peek().is_some_and(|a| {
            !autoshade::face::is_face_intent(a)
                && autoshade::recipe::LocalAdjustment { mask: m.mask.clone(), range: m.range, ..(*a).clone() } == *m
        });
        out.push(if became { rest.next().cloned() } else { None });
    }
    out
}

/// Give `dst`'s face intents what the resolve step made of the same intents
/// (`face_outcomes`): the raster and seeded range, or — for one taken out —
/// its removal. Matched by order, and only when `dst` still has exactly as
/// many intents (an edit made while the face finder ran leaves it alone).
/// Returns how many were filled.
pub(crate) fn fill_face_intents(dst: &mut EditRecipe, outcomes: &[Option<autoshade::recipe::LocalAdjustment>]) -> usize {
    let at: Vec<usize> =
        dst.masks.iter().enumerate().filter(|(_, m)| autoshade::face::is_face_intent(m)).map(|(i, _)| i).collect();
    if at.len() != outcomes.len() {
        return 0;
    }
    let mut n = 0;
    for (&i, o) in at.iter().zip(outcomes).rev() {
        match o {
            Some(r) => {
                dst.masks[i].mask = r.mask.clone();
                if dst.masks[i].range.is_none() {
                    dst.masks[i].range = r.range;
                }
                n += 1;
            }
            None => {
                dst.masks.remove(i);
            }
        }
    }
    n
}

/// Give `dst`'s alpha-less AI masks the alpha `from` found for the SAME mask
/// (the same intent in the same frame; only the raster differs). Matched by
/// geometry, not by position, so an edit made while the segmenter ran keeps
/// its masks where it put them. Returns how many were filled.
pub(crate) fn fill_ai_rasters(dst: &mut EditRecipe, from: &EditRecipe) -> usize {
    use autoshade::recipe::MaskGeometry;
    if dst.quarter_turns % 4 != from.quarter_turns % 4 {
        return 0; // an alpha is cut in one frame
    }
    let found: Vec<(MaskGeometry, String)> = geometries(from)
        .filter_map(|g| match g {
            MaskGeometry::AiMask { raster: Some(r), .. } => {
                let mut bare = g.clone();
                if let MaskGeometry::AiMask { raster, .. } = &mut bare {
                    *raster = None;
                }
                Some((bare, r.clone()))
            }
            _ => None,
        })
        .collect();
    let mut n = 0;
    for m in &mut dst.masks {
        for g in std::iter::once(&mut m.mask).chain(m.components.iter_mut().map(|c| &mut c.geometry)) {
            if !matches!(g, MaskGeometry::AiMask { raster: None, .. }) {
                continue;
            }
            if let Some((_, r)) = found.iter().find(|(bare, _)| bare == &*g)
                && let MaskGeometry::AiMask { raster, .. } = g
            {
                *raster = Some(r.clone());
                n += 1;
            }
        }
    }
    n
}

impl AutoShadeApp {
    /// The runner the workers call (a test's stand-in when one is set).
    pub(crate) fn ai_mask_runner(&self) -> AiMaskRunner {
        #[cfg(test)]
        if let Some(stub) = self.ai_mask_stub {
            return stub;
        }
        resolve_ai_masks_locally
    }

    /// After an open landed: re-derive the open card's AI masks that came
    /// without an alpha (a Lightroom sidecar's), on a worker — the segmenter
    /// is a sidecar run, seconds of GPU the first time, cached after.
    pub(crate) fn start_ai_mask_resolve(&mut self, lang: Lang) {
        if self.busy || self.open_in_flight || unresolved_develop_masks(&self.recipe) == 0 {
            return;
        }
        let (Some(photo), Some(pixels)) = (self.src_path.clone(), self.active_source_path()) else { return };
        let card = self.active_variant().map(|v| v.id.clone()).unwrap_or_default();
        let mut recipe = self.recipe.clone();
        let run = self.ai_mask_runner();
        self.busy = true;
        self.status = if unresolved_ai_masks(&recipe) > 0 {
            tr(lang, "Re-deriving the AI masks from Lightroom on this computer…")
        } else {
            tr(lang, "Finding the faces… (the first run downloads the models)")
        }
        .into();
        let (panic_photo, panic_card) = (photo.clone(), card.clone());
        self.spawn_worker(
            move || {
                let before = recipe.clone();
                let res = run(&photo, &pixels, &mut recipe);
                let faces = face_outcomes(&before, &recipe);
                Msg::AiMasksResolved(Box::new((photo, card, recipe, res, faces)))
            },
            move |e| {
                let res = autoshade::pipeline::DevelopMasks {
                    ai: autoshade::segment::AiMaskResolution { unresolved: vec![(String::new(), e.to_string())], ..Default::default() },
                    faces: Default::default(),
                };
                Msg::AiMasksResolved(Box::new((panic_photo, panic_card, EditRecipe::default(), res, Vec::new())))
            },
        );
    }

    /// `Msg::AiMasksResolved`: the alphas go into the open card — only when
    /// it is still the photo and card they were cut for — and into its saved
    /// mark and undo history too: an alpha is derived from the intent, not
    /// an edit, so it neither lights ● nor goes away on Ctrl+Z.
    pub(crate) fn on_ai_masks_resolved(
        &mut self,
        lang: Lang,
        (photo, card, resolved, res, faces): (PathBuf, String, EditRecipe, autoshade::pipeline::DevelopMasks, FaceOutcomes),
    ) {
        self.busy = false;
        let here = self.active_variant().map(|v| v.id.clone()).unwrap_or_default();
        if self.src_path.as_ref() != Some(&photo) || here != card {
            return;
        }
        let fill = |r: &mut EditRecipe| {
            fill_ai_rasters(r, &resolved);
            fill_face_intents(r, &faces);
        };
        let n = fill_ai_rasters(&mut self.recipe, &resolved);
        let nf = fill_face_intents(&mut self.recipe, &faces);
        fill(&mut self.saved_recipe);
        fill(&mut self.committed.recipe);
        for s in self.undo_stack.iter_mut().chain(self.redo_stack.iter_mut()) {
            fill(&mut s.recipe);
        }
        if let Some(v) = self.variants.get_mut(self.active) {
            fill(&mut v.recipe);
        }
        if n + nf > 0 || faces.iter().any(Option::is_none) {
            self.dirty = true;
            self.overlay_stale = true;
        }
        if let Some(why) = res.warning() {
            let t = trf(lang, "Some masks could not be drawn, so they are not applied: {why}", &[("why", &why)]);
            self.status = t.clone();
            self.toast(ToastKind::Error, t);
        } else {
            let mut said: Vec<String> = Vec::new();
            if n > 0 || nf == 0 {
                said.push(trf(lang, "AI masks ×{n} re-derived locally — not Adobe's raster", &[("n", &n.to_string())]));
            }
            if nf > 0 {
                said.push(trf(lang, "Face masks ×{n} outlined on this computer", &[("n", &nf.to_string())]));
            }
            self.done(said.join(" · "));
        }
    }
}
