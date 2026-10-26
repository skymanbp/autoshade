//! Clearing a develop, listing versions, contained joins, and mask-path resolution.

use super::*;

/// Stamp "this develop was explicitly CLEARED now": the newest-intent rule
/// ranks the marker's mtime against the Lightroom sidecar beside the RAW
/// (see [`lightroom_sidecar`]), so a projection the user once copied there
/// cannot resurrect edits that were just cleared. Never counted by
/// [`has_develop`]; rewritten in place — only the mtime matters.
pub fn mark_develop_cleared(src: &Path) -> std::io::Result<()> {
    mark_develop_cleared_in(&store_root(), src)
}

fn mark_develop_cleared_in(root: &Path, src: &Path) -> std::io::Result<()> {
    let dir = develop_dir_in(root, src);
    std::fs::create_dir_all(&dir)?;
    durable_write(
        &dir.join("cleared.txt"),
        b"develop cleared by an explicit neutral save\n",
    )
}

/// What an explicit clear actually achieved. The marker is best-effort by
/// nature (it only decides anything when a projection sits beside the RAW),
/// but a clear that reports plain success while that resurrection route stays
/// open is the same lie the deliverable paths already outlawed — so the
/// failure rides back to the surface, which shows it.
pub struct ClearOutcome {
    /// Did any saved file actually go away (else: "nothing to save").
    pub removed: bool,
    /// The develop IS cleared, but [`mark_develop_cleared`] failed: a sidecar
    /// beside the RAW can still out-rank the clear and restore the edits.
    pub marker_warning: Option<String>,
}

/// Delete a photo's saved develop — the "clear my edits" semantics of a
/// neutral Reset-then-Save — from EVERY home: the central store, any legacy
/// ./out sidecar a pre-store build left behind, and the baked-pixels link.
/// Version snapshots are kept.
///
/// ONE primitive for every surface. The GUI and the web each carried their own
/// copy of this list and drifted apart twice: the web missed the cleared
/// marker, and BOTH unlinked `pixels.json` directly instead of going through
/// [`clear_pixel_source`] — which leaves the retired `pixels.json.bak` behind
/// for [`recover_orphan_baks`] to republish, handing the next open the very
/// retouch the user just cleared.
///
/// A file already missing IS the desired end state; any OTHER removal failure
/// reaches the caller — a surviving sidecar resurrects the edits on reopen.
pub fn clear_develop(src: &Path) -> std::io::Result<ClearOutcome> {
    let root = store_root();
    let legacy_roots = legacy_out_roots();
    clear_develop_in(&root, &legacy_roots, src)
}

pub(super) fn clear_develop_in(
    root: &Path,
    legacy_roots: &[PathBuf],
    src: &Path,
) -> std::io::Result<ClearOutcome> {
    with_develop_lock_in(root, src, DevelopLockMode::Wait, || {
        clear_develop_unlocked_in(root, legacy_roots, src)
    })
}

fn clear_develop_unlocked_in(
    root: &Path,
    legacy_roots: &[PathBuf],
    src: &Path,
) -> std::io::Result<ClearOutcome> {
    // TRANSACTION MARKER FIRST (L03): a kill mid-sweep used to leave a
    // half-cleared develop — recipe gone, XMP or variants alive — that
    // every reader took for a real partial develop, resurrecting edits the
    // user explicitly cleared (and recover_orphan_baks republished the very
    // .baks the clear was deleting). The marker records the intent durably;
    // recovery completes the sweep on the next locked touch.
    durable_write(&clear_pending_in(root, src), b"develop clear in progress\n")?;
    let (removed, first_err) = clear_sweep_in(root, legacy_roots, src);
    if let Some(e) = first_err {
        // The marker STAYS — recovery retries the sweep.
        return Err(e);
    }
    let marker_warning = mark_develop_cleared_in(root, src).err().map(|e| e.to_string());
    if marker_warning.is_none() {
        // Only after the durable cleared stamp exists — between the two
        // writes BOTH markers exist, so the clear's intent is never
        // invisible. A failed stamp keeps the pending marker: recovery
        // retries it, and the ranking treats the marker itself as newest
        // intent meanwhile.
        let _ = std::fs::remove_file(clear_pending_in(root, src));
        settle_consumed_marker(&clear_pending_in(root, src));
    }
    Ok(ClearOutcome { removed, marker_warning })
}

/// The sweep half of [`clear_develop_in`], IDEMPOTENT so recovery may
/// repeat it: legacy tombstone, the central files (retired `.bak`s
/// included) and the pixel-source pair. Returns what it removed and the
/// first failure (later removals still run — retrying costs nothing and a
/// partial sweep is smaller than the one it retries).
pub(super) fn clear_sweep(src: &Path) -> (bool, Option<std::io::Error>) {
    let root = store_root();
    let legacy_roots = legacy_out_roots();
    clear_sweep_in(&root, &legacy_roots, src)
}

fn clear_sweep_in(
    root: &Path,
    legacy_roots: &[PathBuf],
    src: &Path,
) -> (bool, Option<std::io::Error>) {
    let del = |p: &Path| match std::fs::remove_file(p) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    };
    let mut removed = false;
    let mut first_err: Option<std::io::Error> = None;
    // The retired `recipe.json.bak` goes FIRST, for exactly the reason
    // [`clear_pixel_source`] retires its own: [`recover_orphan_baks`]
    // republishes a `.bak` whenever the live file is missing, so a clear that
    // removes only the live recipe hands the next open the develop it just
    // cleared — in the same session, since every read runs the recovery.
    // `write_recipe` drops its `.bak` on a successful publish, so this needs
    // one ignored-unlink fault to arise (the AV-lock case the publisher
    // already documents) — but the clear path is the ONE save path that never
    // runs that publisher's stale-`.bak` hygiene, so nothing else would ever
    // sweep it. `cleared.txt` cannot help: the recovery never reads it.
    // EVERY legacy root, not just the one `legacy_recipe` resolves to.
    // `legacy_file` returns the FIRST existing match, so with two ./out roots
    // in play (the env override and the cwd/exe fallbacks) a clear removed one
    // copy and left the other — which the very next read then restored, so
    // "cleared" did not stay cleared. Duplicates are harmless: a missing file
    // is already the desired end state.
    //
    // Legacy names are STEM-ONLY, so two photos of the same stem in different
    // folders share them — clearing one clears both. That ambiguity is the
    // legacy layout's own, and it is exactly why the central store keys by
    // path; leaving a root unswept does not avoid it, it only makes the clear
    // fail while the same shared file resurrects the edits on the next read.
    let stem = crate::pipeline::stem(src);
    let legacy_was_visible = !legacy_suppressed_in(root, src)
        && legacy_roots.iter().any(|legacy_root| {
            legacy_root.join(format!("{stem}.recipe.json")).exists()
                || legacy_root.join(format!("{stem}.xmp")).exists()
        });
    // The tombstone lands BEFORE central files are removed. A crash can
    // therefore leave the old central develop visible, or leave it cleared
    // with legacy already suppressed, but cannot expose the ambiguous
    // fallback in between.
    if let Err(e) = suppress_legacy_in(root, src) {
        return (removed, Some(e));
    }
    removed |= legacy_was_visible;

    // A pending single-generation commit dies with the clear: the clear is
    // the newer intent (commit_develop completes a pending clear before it
    // stages), and a surviving `.commit` would REPLAY the very develop this
    // sweep removes.
    match std::fs::remove_dir_all(commit_dir_in(root, src)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            first_err.get_or_insert(e);
        }
    }

    for p in [
        develop_dir_in(root, src).join("recipe.json.bak"),
        recipe_target_in(root, src),
        xmp_target_in(root, src),
        // The strip record goes with the develop it describes — a surviving
        // variants.json would resurrect background variants over a develop
        // the user explicitly cleared.
        develop_dir_in(root, src).join("variants.json.bak"),
        variants_path_in(root, src),
    ] {
        match del(&p) {
            Ok(b) => removed |= b,
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    // Asked BEFORE the removal, and of the predicate that counts the `.bak`: a
    // develop whose only surviving trace was the retired master still had
    // something to remove.
    let had_pixels = has_pixel_source_in(root, src);
    match clear_pixel_source_unlocked_in(root, src) {
        Ok(()) => removed |= had_pixels,
        Err(e) => {
            first_err.get_or_insert(e);
        }
    }
    (removed, first_err)
}

/// Snapshot numbers present in the photo's develop dir, sorted ascending.
pub fn list_versions(src: &Path) -> Vec<u32> {
    let mut out = Vec::new();
    let mut deleting = Vec::new();
    if let Ok(dir) = std::fs::read_dir(develop_dir(src)) {
        for e in dir.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(rest) = name.strip_prefix("v")
                && let Some(nums) = rest.strip_suffix(".recipe.json")
                && let Ok(n) = nums.parse::<u32>()
            {
                out.push(n);
            }
            // A half-deleted version is not listed (L03): its recipe may
            // survive the kill, but the user asked for it to go — recovery
            // finishes the sweep at the next claim or locked touch.
            if let Some(n) = name
                .strip_prefix(".deleting.v")
                .and_then(|rest| rest.parse::<u32>().ok())
            {
                deleting.push(n);
            }
        }
    }
    out.retain(|n| !deleting.contains(n));
    out.sort_unstable();
    out
}

/// Resolve relative Bitmap mask paths against `base` (the directory the recipe
/// was loaded from). Only rewrites a reference whose file actually EXISTS
/// under `base` — anything else is left untouched, so legacy cwd-relative
/// "out/…" references keep resolving exactly as before this module existed.
pub(super) fn contained_join(base: &Path, relative: &Path) -> Option<PathBuf> {
    use std::path::Component;

    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }

    let candidate = base.join(relative);
    let mut existing = candidate.as_path();
    while !existing.exists() {
        existing = existing.parent()?;
        if !existing.starts_with(base) {
            return None;
        }
    }

    if base.exists() {
        let canonical_base = std::fs::canonicalize(base).ok()?;
        let canonical_existing = std::fs::canonicalize(existing).ok()?;
        if !canonical_existing.starts_with(canonical_base) {
            return None;
        }
    }
    Some(candidate)
}
/// True for UNC / verbatim-UNC / device-namespace prefixes — the classes
/// whose mere `exists()` probe leaves this machine (an SMB touch sends
/// NetNTLM credentials). Checked LEXICALLY, before any filesystem call, for
/// exactly that reason. Local drive-letter absolutes stay honoured: the
/// pixel-source writer legitimately records them for masters outside the
/// develop dir.
pub(super) fn remote_or_device_path(p: &Path) -> bool {
    use std::path::{Component, Prefix};
    match p.components().next() {
        // Windows: the platform parser classifies, and its judgement is the one
        // this has always used — unchanged to the byte. The verbatim-LOCAL
        // prefix is deliberately absent from this list, because
        // `std::fs::canonicalize` hands those out by the dozen (see
        // [`strip_verbatim`]) and refusing one would disable a real raster.
        Some(Component::Prefix(pre)) => matches!(
            pre.kind(),
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(_)
        ),
        // Everywhere else `Component::Prefix` is never produced at all — it is
        // a Windows-parser construct — so the arm above was dead code and the
        // whole refusal with it: a recipe written on Windows and opened on a
        // Mac had its network mask reference treated as an ordinary relative
        // file name. Same classes, decided on the spelling.
        _ => unc_or_device_spelling(p),
    }
}

/// A UNC or device path by SPELLING: two leading separators, either slash.
///
/// Not equivalent to the prefix match above, and not claimed to be — it also
/// catches the verbatim-local form, which Windows correctly treats as local.
/// That is the right answer off Windows, where such a string cannot name a
/// local file at all, and it never decides anything ON Windows, because the
/// parser answers first for every path that has a prefix.
pub(super) fn unc_or_device_spelling(p: &Path) -> bool {
    let b = p.as_os_str().as_encoded_bytes();
    b.starts_with(br"\\") || b.starts_with(b"//")
}

pub fn resolve_mask_paths(r: &mut EditRecipe, base: &Path) {
    // Every raster reference, not only the masks' (`EditRecipe::
    // raster_paths_mut`): a depth map, protect matte or pixel layer named by
    // an untrusted store reached the render verbatim before.
    for path in r.raster_paths_mut() {
        let p = Path::new(path.as_str());
        if remote_or_device_path(p) {
            // A develop store is not trusted to reach OFF this machine:
            // a crafted `\\attacker\share\mask.png` turned "open the
            // photo" into an outbound SMB authentication.
            eprintln!(
                "⚠ raster reference {path:?} names a network/device path — it is disabled"
            );
            *path = base
                .join(".invalid-mask-reference")
                .to_string_lossy()
                .into_owned();
        } else if p.is_relative() {
            // A BARE name is the store's own convention and can only mean
            // "this develop dir" — anchor it even when the file is GONE.
            // Safe multi-component legacy refs retain their old
            // exists-gated cwd behavior.
            let bare = p.parent().is_none_or(|x| x.as_os_str().is_empty());
            match contained_join(base, p) {
                Some(cand) if bare || cand.exists() => {
                    *path = cand.to_string_lossy().into_owned();
                }
                Some(_) => {}
                None => {
                    eprintln!(
                        "⚠ raster reference {path:?} escapes {} — it is disabled",
                        base.display()
                    );
                    *path = base
                        .join(".invalid-mask-reference")
                        .to_string_lossy()
                        .into_owned();
                }
            }
        }
    }
}

/// Disable every raster reference that cannot be anchored without a develop
/// dir — a relative name, or a network/device path — and return how many.
/// A bare name only means "this develop dir" ([`resolve_mask_paths`]), so a
/// recipe read with no develop dir to hand (a sidecar read without its
/// photo) cannot keep one: a later save would anchor it to whichever develop
/// the recipe lands in, where a different file may carry the name. The
/// sentinel is absolute and under the store root, so no later load re-anchors
/// it and no develop dir's real file can answer for it.
pub fn disable_unanchored_mask_paths(r: &mut EditRecipe) -> usize {
    let sentinel = store_root().join(".invalid-mask-reference").to_string_lossy().into_owned();
    let mut n = 0;
    for path in r.raster_paths_mut() {
        let p = Path::new(path.as_str());
        // An empty name names no file (an AI mask not yet computed), so there
        // is nothing for it to resolve against.
        if !path.is_empty() && (p.is_relative() || remote_or_device_path(p)) {
            *path = sentinel.clone();
            n += 1;
        }
    }
    n
}

/// Inverse of [`resolve_mask_paths`] at write time: an absolute raster path
/// (any of `EditRecipe::raster_paths_mut`) that lives DIRECTLY inside `base` is stored as its bare file name, keeping
/// the develop dir relocatable. Paths elsewhere are stored as given.
pub fn relativize_mask_paths(r: &mut EditRecipe, base: &Path) {
    for path in r.raster_paths_mut() {
        let p = Path::new(path.as_str());
        if p.is_absolute()
            && p.parent() == Some(base)
            && let Some(name) = p.file_name().and_then(|n| n.to_str())
        {
            *path = name.to_string();
        }
    }
}
