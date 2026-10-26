//! Develop paths and sidecars: the develop directory, the recipe and sidecar targets, export beside the photo, raster claims and ownership, and the pixel-source record.

use super::*;

/// This photo's develop directory (not created here).
pub fn develop_dir(src: &Path) -> PathBuf {
    develop_dir_in(&store_root(), src)
}

/// Where pixel masters live — the AI denoise / stack / heal / clone / fill /
/// adjust / reimagine results a card works FROM (`pipeline::unique_out`).
/// Not the delivery folder: that one is the user's to clean, and cleaning it
/// left cards pointing at deleted files (user report 2026-10-18). Flat, with
/// the same `<stem>.<tag>.png` names the delivery folder used to hold, and it
/// moves with the store (`AUTOSHADE_DATA_DIR`). Masters written before v1.12
/// stay where they are: develops link them by absolute path.
pub fn masters_dir() -> PathBuf {
    store_root().join("masters")
}

/// Root-parameterized core of [`develop_dir`] so tests can use a temp root
/// without mutating process-global env (set_var is unsafe + racy in 2024).
pub(super) fn develop_dir_in(root: &Path, src: &Path) -> PathBuf {
    root.join("develops").join(resolve_key_in(root, src))
}

/// The working recipe — the single source of truth for a photo's develop.
pub fn recipe_target(src: &Path) -> PathBuf {
    recipe_target_in(&store_root(), src)
}

pub(super) fn recipe_target_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join("recipe.json")
}

/// How many clockwise quarter turns the photo's SAVED develop asks for
/// (`EditRecipe::quarter_turns`), or 0 when there is no saved develop, it
/// cannot be read, or it predates the field.
///
/// A deliberately CHEAP, side-effect-free reader for the THREE places that need
/// only this one number and must not pay a full restore. R27 wrote "the two
/// places" here and there were four by then; R28 2a counted them and R28
/// Batch-3 removed one:
///   * `gui::thumb_cache_file` (`bin/gui/util.rs:1593`) — the gallery cache
///     key: a rotate has to miss it or the grid keeps serving the sideways
///     thumbnail, exactly the v0.30 staleness the salt beside it documents;
///   * the gallery thumbnail worker (`bin/gui/workers.rs:247`), which decodes
///     in the frame that key was built for;
///   * `style::build_index` (`style.rs:571`) — the aspect feature.
///
/// Fails toward 0 = "no turn", which is what every recipe written before
/// v0.33 means and what a corrupt one is indistinguishable from here.
///
/// **"The cost of being wrong is a stale thumbnail, not a wrong pixel" is true
/// again, and only because the fourth consumer is gone.** R28 2a found
/// `segment::stage_source_frame` reading this number to decode the frame an AI
/// mask is SEGMENTED in — a pixel decision, taken from the store while the
/// renderer used the recipe's own `quarter_turns` (adjudication F1-B). R28
/// Batch-3 gave that function the recipe's turn as a parameter, so nothing here
/// decides pixels any more. The rule this leaves behind: a caller that needs
/// the frame the RENDER will use must take it from the recipe it holds, never
/// from this file.
///
/// Bounded like every other read of a file this app persists but an untrusted
/// photo pack can replace ([`read_text_capped`] / [`MAX_STORE_JSON`], 24 other
/// call sites) — this was the one remaining bypass, and the cap's own doc
/// names exactly this scenario (adjudication F7).
pub fn saved_quarter_turns(src: &Path) -> u8 {
    /// ONE field out of a recipe that may be megabytes: a `serde_json::Value`
    /// tree costs roughly ten times the document in RAM for this one `u8`,
    /// which is why `pixels.json` refuses `Value` too.
    ///
    /// Deliberately NOT `deny_unknown_fields`, and not `EditRecipe`: a recipe
    /// from a NEWER build carries fields the strict form would reject, and a
    /// cache key that hard-fails on a forward file would silently un-rotate
    /// EVERY thumbnail instead of one. A plain derive ignores what it does not
    /// know, so the forward-compat property the `Value` tree provided survives
    /// the switch. `#[serde(default)]` keeps the pre-v0.33 recipes (no such
    /// key at all) on the same 0 the field's own default gives them.
    #[derive(serde::Deserialize)]
    struct SavedTurn {
        #[serde(default)]
        quarter_turns: u8,
    }
    read_text_capped(&recipe_target(src), MAX_STORE_JSON)
        .ok()
        .and_then(|t| serde_json::from_str::<SavedTurn>(&t).ok())
        // `% 4` exactly as `EditRecipe::clamp` normalises the real field, so
        // this reader and the render cannot disagree about a legal recipe.
        .map_or(0, |v| v.quarter_turns % 4)
}

/// The Lightroom XMP projection. Keeps the `<stem>.xmp` name so copying it
/// beside the RAW for Lightroom needs no rename.
pub fn xmp_target(src: &Path) -> PathBuf {
    xmp_target_in(&store_root(), src)
}

pub(super) fn xmp_target_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join(format!("{}.xmp", crate::pipeline::stem(src)))
}

/// Where [`export_xmp_beside`] delivers: `<photo folder>/<name>.xmp`, spelled
/// EXACTLY as [`lightroom_sidecar`] and `pipeline::write_xmp` read it
/// (`with_extension`), so the file this produces is the file those two
/// recognise. `None` only for a pathless photo.
pub fn xmp_beside_target(src: &Path) -> Option<PathBuf> {
    src.parent().is_some().then(|| src.with_extension("xmp"))
}

/// How long a staging file must have sat untouched before [`sweep_stale_sidecar_stages`]
/// treats it as an orphan. A real publish stages, fsyncs and renames a few KB of
/// XML in milliseconds, so a minute is far past "another AutoShade is mid-write"
/// while still reclaiming litter on the very next click.
pub(super) const SIDECAR_STAGE_GRACE: std::time::Duration = std::time::Duration::from_secs(60);

/// Reclaim staging files an EARLIER sidecar delivery left in the photo's own
/// folder.
///
/// [`durable_write`] stages beside its target ([`sibling_tmp`]) and removes the
/// stage when the publish fails — but a crash, a kill or a power loss between
/// the two leaves `<name>.xmp.tmp.<pid>.<seq>` behind, and that folder is the
/// user's PHOTO LIBRARY: nothing in this app ever looks there again, so the
/// orphan is permanent litter in the one place we promise to keep clean
/// (everywhere else the store's own recovery sweeps handle it).
///
/// Narrow by construction — this must never touch a file that is not ours:
/// * only names matching this target's own stage pattern, `<file name>.tmp.` +
///   two decimal fields, exactly what [`sibling_tmp`] mints;
/// * only after `grace` has elapsed since the last write, because a younger
///   stage may belong to another AutoShade publishing right now (deleting it
///   would fail that delivery);
/// * every error — unreadable directory, unstat-able entry, refused delete — is
///   ignored: a hand-off must not fail because litter could not be collected.
pub(super) fn sweep_stale_sidecar_stages(to: &Path, grace: std::time::Duration) {
    let (Some(dir), Some(name)) = (to.parent(), to.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let prefix = format!("{name}.tmp.");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let raw = entry.file_name();
        let Some(tail) = raw.to_str().and_then(|n| n.strip_prefix(&prefix)) else { continue };
        let decimal = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let mut fields = tail.split('.');
        let ours = matches!(
            (fields.next(), fields.next(), fields.next()),
            (Some(pid), Some(seq), None) if decimal(pid) && decimal(seq)
        );
        if !ours {
            continue;
        }
        let orphaned = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= grace);
        if orphaned {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Hand this photo's stored Lightroom XMP projection over to Lightroom: copy
/// `<develop dir>/<stem>.xmp` to the photo's OWN folder as `<name>.xmp`, which
/// is the only place Lightroom / Camera Raw look for a sidecar.
///
/// **This is the one deliberate write into the photo's folder.** Everything
/// else refuses (`pipeline::guard_readonly`, "the photo library is read-only")
/// and that guard is intentionally NOT consulted here — it exists to stop
/// RENDERED deliverables and `-o` mistakes from landing in a library, whereas
/// an XMP sidecar is metadata that Lightroom itself writes there by convention,
/// the destination is fixed (never user-typed), and the caller is one explicit
/// per-photo click. It is never reachable from a batch, a paste or the CLI.
///
/// `overwrite = false` refuses an existing sidecar with
/// [`std::io::ErrorKind::AlreadyExists`] — Lightroom's own file may be sitting
/// there, and the caller turns that refusal into a confirmation. The refusal is
/// an ATOMIC `create_new` claim, not an `exists()` probe, so a file that appears
/// between the check and the write cannot be clobbered unseen.
///
/// No develop lock, and the READ half is what makes that safe, not just the
/// write half: the projection is published with [`durable_write`] (stage +
/// atomic rename — `pipeline::write_xmp_doc`), never retire-then-write, so a
/// concurrent save leaves no window in which `xmp_target` is missing or half
/// written. A reader sees the previous complete sidecar or the next one. The
/// write half touches nothing inside the develop directory at all. Taking the
/// lock would therefore only add a way for the hand-off to fail while another
/// AutoShade happens to be saving.
pub fn export_xmp_beside(src: &Path, overwrite: bool) -> std::io::Result<PathBuf> {
    let stored = xmp_target(src);
    // Bounded like every other read in this module (R28 2a): the projection
    // lives in the develop dir, which under the `<temp>/autoshade` fallback
    // root is world-writable. `NotFound` passes through `read_bytes_capped`
    // untouched, so the workflow-state message below keeps its trigger.
    let bytes = read_bytes_capped(&stored, MAX_STORE_JSON).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // "Nothing there" is a WORKFLOW state, not a missing file: the
            // develop has not been saved (or this photo is not a RAW), and the
            // remedy belongs in the message.
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "no stored Lightroom XMP for this photo yet ({}) — save the develop first",
                    stored.display()
                ),
            )
        } else {
            e
        }
    })?;
    let Some(to) = xmp_beside_target(src) else {
        return Err(std::io::Error::other(format!(
            "{} has no containing folder to deliver into",
            src.display()
        )));
    };
    // BEFORE this delivery stages anything of its own: collect the staging files
    // an interrupted earlier hand-off may have left in the photo's folder. This
    // is the only writer that ever stages there, so it is also the only thing
    // that can clean up after itself (R22 L2).
    sweep_stale_sidecar_stages(&to, SIDECAR_STAGE_GRACE);
    let claimed = if overwrite {
        false
    } else {
        // Claim the name atomically; AlreadyExists travels to the caller
        // verbatim so it can ask before replacing someone else's file.
        std::fs::OpenOptions::new().write(true).create_new(true).open(&to)?;
        true
    };
    // ONE write protocol for the payload either way (L03) — and a failed
    // publish must not leave the empty claim behind as a "sidecar".
    //
    // The rollback is NARROW on purpose (R22 L2): only a publish that never
    // reached `to` may delete it. Once the staged bytes are renamed over the
    // claim, that file IS the delivered sidecar, and the only step that can fail
    // afterwards is the parent-directory fsync (`durable_os::finish_parent` —
    // infallible on Windows, `File::open(dir).sync_all()` on unix). The old
    // blanket `if claimed { remove_file }` deleted a correct, complete sidecar
    // for that. The honest degradation is to keep the file and say the directory
    // ENTRY is one notch less durable: the bytes are there, and only a crash in
    // the next moment could lose the name.
    match durable_write_tracked(&to, &bytes) {
        (_, Ok(())) => Ok(to),
        (true, Err(e)) => {
            eprintln!(
                "⚠ the Lightroom sidecar at {} was written, but its folder entry could not be \
                 made durable ({e}) — the file is there; only a crash in the next moment could \
                 lose the name",
                to.display()
            );
            Ok(to)
        }
        (false, Err(e)) => {
            if claimed {
                let _ = std::fs::remove_file(&to);
            }
            Err(e)
        }
    }
}

/// Numbered snapshot `v<n>.recipe.json` (GUI versions + programmatic backups).
pub fn version_target(src: &Path, n: u32) -> PathBuf {
    develop_dir(src).join(format!("v{n}.recipe.json"))
}

/// A mask raster (`mask-sky.png`, `mask-zone-sky.png`, …) inside the photo's
/// develop dir. Recipes reference it by bare file name (see module docs).
pub fn raster_target(src: &Path, kind: &str) -> PathBuf {
    develop_dir(src).join(format!("{kind}.png"))
}

/// Claim a FRESH raster name in the photo's develop dir: `<prefix>.png`,
/// `<prefix>-2.png` … `-999`, atomically create_new-claimed so two surfaces
/// can never hand out the same name (the same scheme `pipeline::unique_out`
/// uses for pixel masters). The zoned reverse-fit writes each run's raster
/// under its own claimed name instead of rewriting one fixed file in place —
/// the in-place rewrite left the still-live saved recipe referencing freshly
/// replaced bytes whenever the recipe write AFTER it failed or crashed.
/// Superseded rasters stay on disk (small greyscale PNGs; version snapshots
/// freeze their own copies regardless).
pub fn claim_raster(src: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    for n in 0..=998u32 {
        let kind = if n == 0 { prefix.to_string() } else { format!("{prefix}-{}", n + 1) };
        let cand = raster_target(src, &kind);
        if let Some(par) = cand.parent() {
            std::fs::create_dir_all(par)?;
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&cand) {
            Ok(_) => return Ok(cand),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(format!(
        "over 999 '{prefix}' rasters for this photo — clean up its develop folder first"
    )))
}

/// A raster path whose FILE the holder OWNS and may delete.
///
/// The zoned reverse-fit removes its mask when no zone survives, so that file
/// must be one the same run created. A bare `&Path` cannot carry that: on
/// 2026-08-25 a test passed the user's calibration corpus `sky-mask.png` into
/// the deleting chain, a mutation forced the no-zone branch, and the file --
/// untracked, local-only, referenced by `fitted.recipe.json` by path rather
/// than by bytes -- was gone. The contract had lived in a doc comment,
/// invisible at the call site. It now lives in the type: the only ways to
/// build one are a fresh [`claim_raster`] name and, in tests, an explicit
/// scratch path, so a borrowed user path cannot reach a deletion.
#[derive(Debug)]
pub struct OwnedRaster(PathBuf);

impl OwnedRaster {
    /// Claim a fresh raster name in the photo's develop dir and own it.
    pub fn claim(src: &Path, prefix: &str) -> std::io::Result<Self> {
        claim_raster(src, prefix).map(Self)
    }

    /// Atomically claim another owned raster beside this one.
    ///
    /// Zoned tile attempts use separate ownership so rejecting one candidate
    /// can release only that candidate without touching the semantic raster
    /// or any tile already referenced by the recipe.
    pub(crate) fn claim_sibling(&self, prefix: &str) -> std::io::Result<Self> {
        let parent = self.0.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "owned raster has no parent")
        })?;
        if prefix.is_empty()
            || std::path::Path::new(prefix).components().count() != 1
            || prefix.contains(['/', '\\'])
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "raster prefix must be one file-name component",
            ));
        }
        std::fs::create_dir_all(parent)?;
        for n in 0..=998u32 {
            let name = if n == 0 {
                format!("{prefix}.png")
            } else {
                format!("{prefix}-{}.png", n + 1)
            };
            let candidate = parent.join(name);
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
                Ok(_) => return Ok(Self(candidate)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other(format!(
            "over 999 '{prefix}' rasters beside this mask"
        )))
    }

    /// A scratch file a TEST created and may destroy.
    ///
    /// Refuses the calibration corpus outright: that directory holds the
    /// user's hand-made regression pairs, which no run creates and none may
    /// remove. Refusing at CONSTRUCTION means a corpus path cannot even be
    /// carried to a deletion, let alone reach one.
    #[cfg(test)]
    pub(crate) fn scratch(path: PathBuf) -> Self {
        assert!(
            !is_calibration_corpus(&path),
            "refusing to own a calibration-corpus file: {}",
            path.display()
        );
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn into_path(self) -> PathBuf {
        self.0
    }

    /// Delete the owned file, releasing its claimed name. Best-effort, like
    /// the `remove_file(..).ok()` calls it replaces -- except that a corpus
    /// path is refused instead of removed, loudly in debug builds so the bug
    /// is visible where it happens rather than silently in the user's data.
    pub(crate) fn remove(&self) {
        if is_calibration_corpus(&self.0) {
            debug_assert!(
                false,
                "the calibration corpus must never be deleted: {}",
                self.0.display()
            );
            return;
        }
        std::fs::remove_file(&self.0).ok();
    }
}

/// True when `p` lies inside `dir`. Canonicalizes both sides so a relative
/// or `..`-bearing path cannot slip past, and falls back to a lexical
/// component-wise prefix test when either side cannot be canonicalized -- the
/// path may already be gone, which is exactly when the answer still matters.
///
/// Split out of [`is_calibration_corpus`] so it can be pinned by a test that
/// does not mutate a process-global environment variable: a guard whose only
/// test rewrites `AUTOSHADE_FIT_CALIBRATION_DIR` races every other test in the
/// binary and passes for reasons that have nothing to do with the guard.
pub(super) fn is_within(p: &Path, dir: &Path) -> bool {
    match (p.canonicalize(), dir.canonicalize()) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => p.starts_with(dir),
    }
}

/// True when `p` lies inside the corpus named by `AUTOSHADE_FIT_CALIBRATION_DIR`.
fn is_calibration_corpus(p: &Path) -> bool {
    crate::config::live_env("AUTOSHADE_FIT_CALIBRATION_DIR")
        .is_some_and(|dir| is_within(p, Path::new(&dir)))
}

/// Give every bitmap mask in `r` its own LIVE raster copy, claimed under
/// `prefix`, and repoint the recipe at the copies.
///
/// Used when a VERSION snapshot becomes the working recipe. A snapshot's
/// rasters are frozen under that version's own names (`v3.mask-sky.png`) and
/// `delete_version` sweeps them with the snapshot, so a canvas still pointing
/// at them lost its masks the moment the user deleted the version it came
/// from — and the next save wrote the dangling path to disk. Copies make the
/// loaded state independent of the snapshot's lifetime.
///
/// Best-effort per mask: a raster that cannot be copied keeps its existing
/// reference (the engine's missing-raster contract still reports it) rather
/// than failing the whole load.
pub fn detach_rasters(src: &Path, r: &mut EditRecipe, prefix: &str) {
    for path in r.raster_paths_mut() {
        let from = PathBuf::from(path.as_str());
        if !from.exists() {
            continue;
        }
        let Ok(dst) = claim_raster(src, prefix) else { continue };
        if copy_atomic(&from, &dst).is_ok() {
            *path = dst.to_string_lossy().into_owned();
        } else {
            // Release the claim we could not fill.
            let _ = std::fs::remove_file(&dst);
        }
    }
}

/// Sidecar recording the saved develop's PIXEL SOURCE when it is a baked
/// raster — an in-place heal/clone/fill/denoise master, or a reimagine
/// rendition. The recipe/XMP are parametric and cannot carry baked pixels;
/// without this record, reopening a photo silently reverted the canvas to the
/// un-retouched source (the "variant linkage lost on navigation" boundary).
pub fn pixel_source_path(src: &Path) -> PathBuf {
    pixel_source_path_in(&store_root(), src)
}

pub(super) fn pixel_source_path_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join("pixels.json")
}

/// Record `origin` as the photo's baked pixel source. Stored by bare name when
/// it already lives inside the develop dir (relocatable, like mask rasters);
/// otherwise ABSOLUTIZED — an `out/`-relative master would silently stop
/// resolving the moment the app is launched from a different directory.
pub fn write_pixel_source(src: &Path, origin: &Path, generated: bool) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || {
        write_pixel_source_unlocked(src, origin, generated)
    })
}

fn write_pixel_source_unlocked(
    src: &Path,
    origin: &Path,
    generated: bool,
) -> std::io::Result<()> {
    publish_json_sidecar(src, "pixels.json", pixel_source_record_bytes(src, origin, generated)?)
}

/// The exact pixels.json bytes [`write_pixel_source`] publishes for `origin`
/// — exposed so [`commit_develop`] callers can stage the identical record
/// into a single-generation save.
pub fn pixel_source_record_bytes(
    src: &Path,
    origin: &Path,
    generated: bool,
) -> std::io::Result<Vec<u8>> {
    let dir = develop_dir(src);
    let stored: PathBuf = if origin.parent() == Some(dir.as_path()) {
        origin.file_name().map(PathBuf::from).unwrap_or_else(|| origin.to_path_buf())
    } else {
        std::path::absolute(origin)?
    };
    let doc = serde_json::json!({
        "origin": stored.to_string_lossy(),
        "kind": if generated { "generated" } else { "inplace" },
    });
    serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other)
}

/// The ONE retire-and-publish primitive for the small per-photo JSON sidecars
/// (`pixels.json`, `variants.json`). Same publish discipline as recipe.json:
/// per-process AND per-call tmp name (the web server threads requests), and
/// the old file is RETIRED to `<name>.bak` rather than deleted before the
/// rename — a crash in the window then leaves the previous record recoverable
/// beside the photo, never nothing at all. Both consumers must keep a
/// matching entry in [`recover_orphan_baks`]'s pair list, or the crash-window
/// `.bak` this leaves behind is never republished.
pub(super) fn publish_json_sidecar(src: &Path, name: &str, bytes: Vec<u8>) -> std::io::Result<()> {
    let dir = develop_dir(src);
    std::fs::create_dir_all(&dir)?;
    durable_retire_and_write(
        &dir.join(name),
        &dir.join(format!("{name}.bak")),
        &bytes,
    )
}

/// Forget the baked pixel source (the develop went back to parametric-only).
/// Clearing means DETACH, so the retired `pixels.json.bak` goes too:
/// `write_pixel_source` keeps the previous linkage there for crash recovery,
/// and `recover_orphan_baks` restores a `.bak` whenever the live file is
/// missing — which is exactly the state removing only the live file creates.
/// That combination RESURRECTED the previously superseded master on the next
/// open (and `has_pixel_source` kept counting the develop as retouched).
/// The `.bak` goes FIRST: a crash between the two removals then leaves the
/// live linkage intact (the clear simply has not happened yet) instead of
/// leaving the resurrection bait behind.
/// A file already missing IS the desired end state; any OTHER failure must
/// reach the caller — a surviving pixels.json silently resurrects an obsolete
/// retouched canvas on the next open.
pub fn clear_pixel_source(src: &Path) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || clear_pixel_source_unlocked(src))
}

fn clear_pixel_source_unlocked(src: &Path) -> std::io::Result<()> {
    clear_pixel_source_unlocked_in(&store_root(), src)
}

pub(super) fn clear_pixel_source_unlocked_in(root: &Path, src: &Path) -> std::io::Result<()> {
    let rm = |p: PathBuf| match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    rm(develop_dir_in(root, src).join("pixels.json.bak"))?;
    rm(pixel_source_path_in(root, src))
}

/// GUI-only sidecar persisting the photo's VARIANT STRIP beyond the single
/// saved develop: `variants.json`. recipe.json + pixels.json stay the
/// cross-surface authority for the ACTIVE develop (CLI, web and export never
/// read this file); what THEY cannot carry is the rest of the strip — the
/// per-variant kind and each background variant's own recipe +
/// baked-raster origin. Before this file existed, every reopen collapsed the
/// strip to one card whose kind was guessed from the 2-valued pixels.json
/// flag (a `Fitted` card silently reopened as `Original`), and background
/// variants counted as permanently-unsavable work that pinned the quit
/// guard's dialog forever.
pub fn variants_path(src: &Path) -> PathBuf {
    variants_path_in(&store_root(), src)
}

pub(super) fn variants_path_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join("variants.json")
}
