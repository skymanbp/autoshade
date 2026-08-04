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
//!     source.txt           breadcrumb: which photo this dir belongs to
//! ```
//!
//! The root resolves at runtime (never hardcoded): `AUTOSHOP_DATA_DIR` env
//! override → `%LOCALAPPDATA%/autoshop` (the thumb cache already lives there)
//! → the system temp dir. EXPORTS (developed/retouch/heal/… images) are user
//! deliverables and deliberately STAY in ./out.
//!
//! Mask rasters are referenced from recipe.json by a path string. Inside the
//! store that string is the BARE file name, resolved against the recipe's own
//! directory at load time ([`resolve_mask_paths`]) and relativized back at
//! write time ([`relativize_mask_paths`]) — so a develop dir is relocatable
//! and legacy cwd-relative "out/…" references keep working unchanged.

use std::path::{Path, PathBuf};

use crate::recipe::{EditRecipe, MaskGeometry};

/// Per-user store root: `AUTOSHOP_DATA_DIR` env override (tests, portable
/// setups) → `%LOCALAPPDATA%/autoshop` → `<temp>/autoshop`. Absolute, so keys
/// and targets never depend on the process cwd.
pub fn store_root() -> PathBuf {
    let root = std::env::var_os("AUTOSHOP_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(|d| PathBuf::from(d).join("autoshop"))
            .unwrap_or_else(|| std::env::temp_dir().join("autoshop"))
    });
    std::path::absolute(&root).unwrap_or(root)
}

/// FNV-1a 64-bit. Deliberately hand-rolled: `DefaultHasher` is NOT stable
/// across Rust releases, and this hash names PERSISTENT directories — a
/// changed hash would orphan every existing develop on a toolchain bump.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
}

/// Stable per-photo key: `<stem>-<16 hex>` from the photo's absolute path.
/// The stem prefix keeps the store browsable; the hash disambiguates
/// same-named photos in different folders. Windows paths are case-insensitive
/// (NTFS), so the key hashes the lowercased path there — one file must never
/// produce two keys just because it was opened as `D:\` once and `d:\` once.
pub fn photo_key(src: &Path) -> String {
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let mut s = abs.to_string_lossy().into_owned();
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    format!("{}-{:016x}", crate::pipeline::stem(src), fnv1a64(s.as_bytes()))
}

/// This photo's develop directory (not created here).
pub fn develop_dir(src: &Path) -> PathBuf {
    develop_dir_in(&store_root(), src)
}

/// Root-parameterized core of [`develop_dir`] so tests can use a temp root
/// without mutating process-global env (set_var is unsafe + racy in 2024).
fn develop_dir_in(root: &Path, src: &Path) -> PathBuf {
    root.join("develops").join(photo_key(src))
}

/// The working recipe — the single source of truth for a photo's develop.
pub fn recipe_target(src: &Path) -> PathBuf {
    develop_dir(src).join("recipe.json")
}

/// The Lightroom XMP projection. Keeps the `<stem>.xmp` name so copying it
/// beside the RAW for Lightroom needs no rename.
pub fn xmp_target(src: &Path) -> PathBuf {
    develop_dir(src).join(format!("{}.xmp", crate::pipeline::stem(src)))
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
/// uses for ./out masters). The zoned reverse-fit writes each run's raster
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

/// Sidecar recording the saved develop's PIXEL SOURCE when it is a baked
/// raster — an in-place heal/clone/fill/denoise master, or a reimagine
/// rendition. The recipe/XMP are parametric and cannot carry baked pixels;
/// without this record, reopening a photo silently reverted the canvas to the
/// un-retouched source (the "variant linkage lost on navigation" boundary).
pub fn pixel_source_path(src: &Path) -> PathBuf {
    develop_dir(src).join("pixels.json")
}

/// Record `origin` as the photo's baked pixel source. Stored by bare name when
/// it already lives inside the develop dir (relocatable, like mask rasters);
/// otherwise ABSOLUTIZED — an `out/`-relative master would silently stop
/// resolving the moment the app is launched from a different directory.
pub fn write_pixel_source(src: &Path, origin: &Path, generated: bool) -> std::io::Result<()> {
    let dir = develop_dir(src);
    std::fs::create_dir_all(&dir)?;
    let stored: PathBuf = if origin.parent() == Some(dir.as_path()) {
        origin.file_name().map(PathBuf::from).unwrap_or_else(|| origin.to_path_buf())
    } else {
        std::path::absolute(origin)?
    };
    let doc = serde_json::json!({
        "origin": stored.to_string_lossy(),
        "kind": if generated { "generated" } else { "inplace" },
    });
    // Same publish discipline as recipe.json: per-process AND per-call tmp
    // name (the web server threads requests), and the old file is RETIRED to
    // .bak rather than deleted before the rename — a crash in the window
    // then leaves the previous linkage recoverable beside the photo, never
    // nothing at all (Windows rename cannot replace an existing target).
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let target = pixel_source_path(src);
    let tmp = dir.join(format!(
        "pixels.json.tmp.{}.{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other)?)?;
    let mut retired = false;
    if target.exists() {
        let bak = dir.join("pixels.json.bak");
        // Make room (Windows rename cannot replace). Losing an OLD .bak to a
        // concurrent writer is last-writer-wins by design — the same stance
        // recipe.json documents for its shared .bak.
        let _ = std::fs::remove_file(&bak);
        match std::fs::rename(&target, &bak) {
            Ok(()) => retired = true,
            // Target vanished mid-race (another writer retired it): nothing
            // left to retire — this write's publish still proceeds.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        // The publish failed AFTER the old file was retired — put it back so
        // the previous linkage stays the live one, not a .bak orphan. A
        // failed restore must be SAID: the error then names where the
        // previous linkage survives instead of pretending it is live.
        let mut restore_note = String::new();
        if retired {
            let bak = dir.join("pixels.json.bak");
            if std::fs::rename(&bak, &target).is_err() {
                restore_note = format!(
                    " (restoring the previous pixels.json ALSO failed — it survives at {})",
                    bak.display()
                );
            }
        }
        let _ = std::fs::remove_file(&tmp);
        if restore_note.is_empty() {
            return Err(e);
        }
        return Err(std::io::Error::new(e.kind(), format!("{e}{restore_note}")));
    }
    Ok(())
}

/// Forget the baked pixel source (the develop went back to parametric-only).
/// A file already missing IS the desired end state; any OTHER failure must
/// reach the caller — a surviving pixels.json silently resurrects an obsolete
/// retouched canvas on the next open.
pub fn clear_pixel_source(src: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(pixel_source_path(src)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The photo's recorded baked pixel source, if it still resolves on disk:
/// `(master_path, is_generated)`. A missing sidecar is silent (the normal
/// parametric-only case); an EXISTING sidecar that cannot be honoured —
/// unreadable JSON or a deleted/moved master — degrades to `None` with a
/// stderr warning, so "the canvas reverted to the un-retouched source" stays
/// traceable instead of looking like data loss with no cause.
pub fn read_pixel_source(src: &Path) -> Option<(PathBuf, bool)> {
    let sidecar = pixel_source_path(src);
    let bytes = match std::fs::read(&sidecar) {
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
    let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        eprintln!(
            "⚠ {} is unreadable — the baked retouch master is not restored",
            sidecar.display()
        );
        return None;
    };
    let Some(origin) = doc.get("origin").and_then(|o| o.as_str()) else {
        eprintln!(
            "⚠ {} has no origin field — the baked retouch master is not restored",
            sidecar.display()
        );
        return None;
    };
    let generated = doc.get("kind").and_then(|k| k.as_str()) == Some("generated");
    let mut path = PathBuf::from(origin);
    if path.is_relative() {
        path = develop_dir(src).join(path);
    }
    if !path.exists() {
        eprintln!(
            "⚠ baked master {} is gone — the retouched canvas cannot be restored (the develop falls back to the source)",
            path.display()
        );
        return None;
    }
    Some((path, generated))
}

/// The style reference index — one per user, not per photo.
pub fn style_index_path() -> PathBuf {
    store_root().join("style-index.json")
}

/// UI-written local settings (used to be a cwd-relative `autoshop.local.json`).
pub fn settings_path() -> PathBuf {
    store_root().join("autoshop.local.json")
}

/// Candidate legacy ./out roots, most-specific first. Pre-store sidecars were
/// CWD-relative, so where they sit depends on how the app used to be launched:
/// a terminal launch put them under the project dir (= today's cwd when
/// launched the same way), a double-click put them beside the exe. An
/// `AUTOSHOP_LEGACY_OUT` env override covers any other history. Without the
/// exe-dir probe, upgrading users who start the exe from a NEW directory would
/// see every pre-store develop silently vanish.
fn legacy_out_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(o) = std::env::var_os("AUTOSHOP_LEGACY_OUT") {
        roots.push(PathBuf::from(o));
    }
    roots.push(PathBuf::from("out"));
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let cand = dir.join("out");
        let dup = roots
            .iter()
            .any(|r| std::path::absolute(r).ok() == std::path::absolute(&cand).ok());
        if !dup {
            roots.push(cand);
        }
    }
    roots
}

/// The photo's legacy recipe path: the first candidate root that actually
/// holds one (else the cwd-relative default, so error messages stay sensible).
pub fn legacy_recipe(src: &Path) -> PathBuf {
    legacy_file(&format!("{}.recipe.json", crate::pipeline::stem(src)))
}

pub fn legacy_xmp(src: &Path) -> PathBuf {
    legacy_file(&format!("{}.xmp", crate::pipeline::stem(src)))
}

fn legacy_file(name: &str) -> PathBuf {
    let mut fallback = None;
    for root in legacy_out_roots() {
        let p = root.join(name);
        if p.exists() {
            return p;
        }
        fallback.get_or_insert(p);
    }
    fallback.unwrap_or_else(|| PathBuf::from("out").join(name))
}

/// Does this photo have ANY saved develop — central or legacy, recipe or XMP?
/// Existence-only (no parse): the gallery badge and the web list call this per
/// photo per refresh.
pub fn has_develop(src: &Path) -> bool {
    recipe_target(src).exists()
        || xmp_target(src).exists()
        || legacy_recipe(src).exists()
        || legacy_xmp(src).exists()
}

/// Snapshot numbers present in the photo's develop dir, sorted ascending.
pub fn list_versions(src: &Path) -> Vec<u32> {
    let mut out = Vec::new();
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
        }
    }
    out.sort_unstable();
    out
}

/// Resolve relative Bitmap mask paths against `base` (the directory the recipe
/// was loaded from). Only rewrites a reference whose file actually EXISTS
/// under `base` — anything else is left untouched, so legacy cwd-relative
/// "out/…" references keep resolving exactly as before this module existed.
pub fn resolve_mask_paths(r: &mut EditRecipe, base: &Path) {
    for m in &mut r.masks {
        if let MaskGeometry::Bitmap { path } = &mut m.mask {
            let p = Path::new(path.as_str());
            if p.is_relative() {
                // A BARE name is the store's own convention and can only mean
                // "this develop dir" — anchor it even when the file is GONE,
                // or a missing raster would stay process-relative and could
                // silently pick up an unrelated same-named file from the
                // launch cwd. Multi-component legacy refs ("out/…") keep the
                // exists-gated rewrite: cwd resolution IS their pre-store
                // contract, and rewriting a missing one would break a launch
                // from the original directory.
                let bare = p.parent().is_none_or(|x| x.as_os_str().is_empty());
                let cand = base.join(p);
                if bare || cand.exists() {
                    *path = cand.to_string_lossy().into_owned();
                }
            }
        }
    }
}

/// Inverse of [`resolve_mask_paths`] at write time: an absolute Bitmap path
/// that lives DIRECTLY inside `base` is stored as its bare file name, keeping
/// the develop dir relocatable. Paths elsewhere are stored as given.
pub fn relativize_mask_paths(r: &mut EditRecipe, base: &Path) {
    for m in &mut r.masks {
        if let MaskGeometry::Bitmap { path } = &mut m.mask {
            let p = Path::new(path.as_str());
            if p.is_absolute()
                && p.parent() == Some(base)
                && let Some(name) = p.file_name().and_then(|n| n.to_str())
            {
                *path = name.to_string();
            }
        }
    }
}

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
pub fn backup_saved_develop(
    src: &Path,
    incoming: Option<&EditRecipe>,
) -> std::io::Result<Option<u32>> {
    // Central first, then a not-yet-migrated LEGACY recipe — the read
    // fallbacks restore either, so overwriting the central slot unversioned
    // while a legacy develop still answered was silent destruction too.
    let mut found: Option<(PathBuf, String)> = None;
    for rj in [recipe_target(src), legacy_recipe(src)] {
        match std::fs::read_to_string(&rj) {
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
    // Atomically RESERVED number (see claim_version): the old list+1 pick
    // let two processes select the same n and silently replace each other's
    // snapshot; the claim also embeds the vMAX refusal.
    let (n, dst) = claim_version(src)?;
    // Per-process AND per-call tmp name: GUI + web server are separate
    // PROCESSES backing up the same photo — a shared fixed tmp path lets the
    // two writes interleave into corrupt JSON before either rename runs.
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = dst.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
                std::fs::write(&tmp, json)?;
                std::fs::rename(&tmp, &dst)
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
            if let Err(e) = std::fs::copy(&rj, &tmp).and_then(|_| std::fs::rename(&tmp, &dst)) {
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_file(&dst); // release the claim
                return Err(e);
            }
        }
    }
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
    let mut last = list_versions(src).last().copied();
    loop {
        if last == Some(u32::MAX) {
            return Err(std::io::Error::other(
                "version namespace exhausted (a v4294967295 snapshot exists)",
            ));
        }
        let n = last.unwrap_or(0).saturating_add(1);
        let dst = version_target(src, n);
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
/// first, then `v<n>.recipe.json` derived via `xmp_to_recipe` (clamped; XMP
/// carries no bitmap masks, so no rasters to freeze) — published LAST because
/// `list_versions` scans only `v<N>.recipe.json`: a crash between the two
/// leaves no half-visible version. A neutral/foreign sidecar is not a save
/// (the NoopOnly rule) and needs no snapshot.
fn backup_xmp_only(src: &Path) -> std::io::Result<Option<u32>> {
    let mut found: Option<String> = None;
    for xp in [xmp_target(src), legacy_xmp(src)] {
        match std::fs::read_to_string(&xp) {
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
    let dev = develop_dir(src);
    let stem = crate::pipeline::stem(src);
    // Change-detection instead of an is_noop skip: a derived-noop XMP can
    // still carry edits xmp_to_recipe does not model (Texture, …) — skipping
    // it let analyze/match destroy the only copy. Identical bytes to the
    // LATEST version's xmp snapshot mean this save is already preserved
    // (no version spam on repeated programmatic writes).
    if let Some(lastn) = list_versions(src).last().copied()
        && let Ok(prev) = std::fs::read_to_string(dev.join(format!("v{lastn}.{stem}.xmp")))
        && prev == text
    {
        return Ok(None);
    }
    let mut derived = crate::xmp::xmp_to_recipe(&text);
    derived.clamp();
    // Stamp like the GUI's XMP-only RESTORE does (fresh camera knots + the
    // in-camera lens profile): a verbatim derived snapshot loaded back would
    // render on the dark base without corrections.
    if derived.base_curve.is_empty() {
        derived.base_curve = crate::pipeline::photo_base_knots(src);
    }
    derived.lens_profile = crate::pipeline::fresh_lens_profile(src);
    let (n, dst) = claim_version(src)?;
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = || TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let publish = |dst: &Path, bytes: &[u8], seq: u64| -> std::io::Result<()> {
        let tmp = dst.with_extension(format!("tmp.{}.{}", std::process::id(), seq));
        if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, dst)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(())
    };
    let xmp_dst = dev.join(format!("v{n}.{stem}.xmp"));
    if let Err(e) = publish(&xmp_dst, text.as_bytes(), seq()) {
        let _ = std::fs::remove_file(&dst); // release the claim
        return Err(e);
    }
    let json = serde_json::to_string_pretty(&derived).map_err(std::io::Error::other)?;
    if let Err(e) = publish(&dst, json.as_bytes(), seq()) {
        let _ = std::fs::remove_file(&xmp_dst);
        let _ = std::fs::remove_file(&dst); // release the claim
        return Err(e);
    }
    Ok(Some(n))
}

/// Copy each raster the recipe references INSIDE `dev` to a version-frozen
/// name (`v<n>.<name>`) and rewrite the reference. `list_versions` only parses
/// `v<N>.recipe.json`, so the frozen rasters never pollute the version list.
/// A copy failure rolls back this call's earlier copies and errors — a
/// snapshot must never silently keep pointing at a mutable live raster.
pub fn snapshot_rasters(r: &mut EditRecipe, dev: &Path, n: u32) -> std::io::Result<()> {
    for m in &mut r.masks {
        if let MaskGeometry::Bitmap { path } = &mut m.mask {
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
            if live.exists() {
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
    }
    Ok(())
}

/// Remove every `v<n>.*` frozen raster (backup rollback — the snapshot recipe
/// itself is written only after all freezes succeed).
fn rollback_frozen_rasters(dev: &Path, n: u32) {
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
pub fn delete_version(src: &Path, n: u32) -> std::io::Result<()> {
    // Sweep the frozen rasters FIRST, recipe LAST: with the old order a
    // raster-removal failure was silently discarded after the recipe was
    // already gone, and a retry returned early on the missing recipe —
    // stranding potentially large frozen bitmaps forever. Now a failed
    // raster removal is reported and the version stays retryable.
    let prefix = format!("v{n}.");
    let recipe_name = version_target(src, n)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut first_err: Option<std::io::Error> = None;
    match std::fs::read_dir(develop_dir(src)) {
        Ok(dir) => {
            for e in dir {
                // A per-ENTRY enumeration error is an enumeration failure
                // too: swallowing it (the old `.flatten()`) let the recipe
                // deletion below proceed past rasters the sweep never saw.
                let e = match e {
                    Ok(e) => e,
                    Err(err) => {
                        if first_err.is_none() {
                            first_err = Some(err);
                        }
                        continue;
                    }
                };
                let name = e.file_name();
                let Some(name) = name.to_str() else { continue };
                // The dot terminator keeps "v3." from matching "v30.recipe.json".
                if name.starts_with(&prefix)
                    && name != recipe_name
                    && let Err(err) = std::fs::remove_file(e.path())
                    && first_err.is_none()
                {
                    first_err = Some(err);
                }
            }
        }
        // A missing dir has nothing to strand; any OTHER enumeration failure
        // must abort BEFORE the recipe deletion, or rasters hidden behind it
        // lose their version entry and become unreachable forever.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    if let Some(err) = first_err {
        return Err(err);
    }
    std::fs::remove_file(version_target(src, n))?;
    Ok(())
}

/// Best-effort breadcrumb so a human browsing the hashed store can tell which
/// photo a develop dir belongs to. Never fails the caller.
pub fn note_source(src: &Path) {
    let dir = develop_dir(src);
    let marker = dir.join("source.txt");
    if marker.exists() {
        return;
    }
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(&marker, format!("{}\n", abs.display()));
    }
}

/// Copy `from` to `to` ATOMICALLY: stage beside the destination, then rename.
/// A bare `fs::copy` can leave a PARTIAL destination on failure — which the
/// migration/backup existence checks would then trust as a completed artifact
/// forever. Clobbers an existing `to` (callers gate on their own semantics).
fn copy_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut name = to.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let tmp = to.with_file_name(name);
    if let Err(e) = std::fs::copy(from, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let _ = std::fs::remove_file(to); // make room (Windows rename cannot replace)
    if let Err(e) = std::fs::rename(&tmp, to) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// `fs::rename`, falling back to atomic copy+delete: ./out and the store root
/// are routinely on DIFFERENT volumes (project on D:, %LOCALAPPDATA% on C:),
/// where rename fails with a cross-device error.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_atomic(from, to)?;
    std::fs::remove_file(from)
}

/// One-time, per-photo migration of legacy ./out sidecars into the central
/// develop dir. Idempotent and best-effort per file: a file that cannot move
/// stays where it was and the legacy read fallbacks keep serving it (nothing
/// is ever deleted without its copy landing first). Returns true when at
/// least one file was migrated.
///
/// Recipe files are parsed so their raster references can move along and be
/// rewritten to bare names; an UNPARSABLE recipe is moved byte-for-byte —
/// the loud `Unreadable` handling at read time stays intact.
///
/// Concurrency: two processes migrating the same photo at once (GUI + `serve`)
/// race benignly — both derive from the same legacy bytes, so whichever write
/// lands carries the same content, and the per-file existence checks plus
/// resumability finish any remainder on the next touch. No lock: the store is
/// single-user by construction, and identical-content races cannot corrupt.
pub fn migrate_legacy(src: &Path) -> bool {
    // Process-wide memo: a photo can only need migrating once per process, and
    // this runs on every photo open (UI thread) — without the memo a library
    // whose ./out holds thousands of exports pays a full directory enumeration
    // on every reopen for nothing.
    use std::sync::{Mutex, OnceLock};
    static DONE: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let key = photo_key(src);
    {
        let mut done = DONE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !done.insert(key) {
            return false;
        }
    }
    let mut moved = false;
    let mut failed = false;
    for legacy_out in legacy_out_roots() {
        let (m, f) = migrate_legacy_in(&store_root(), &legacy_out, src);
        moved |= m;
        failed |= f;
    }
    if failed {
        // A transient failure (AV lock, network volume) must stay RETRYABLE:
        // leaving the memo key in place silenced every later open this
        // process, hiding legacy sidecars for the whole session.
        let mut done = DONE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        done.remove(&photo_key(src));
    }
    moved
}

/// Explicit migration from a user-picked old ./out folder (the GUI Settings
/// 「Import develops」 button) — no memo, the user asked for this scan NOW.
pub fn migrate_legacy_from(legacy_out: &Path, src: &Path) -> bool {
    migrate_legacy_in(&store_root(), legacy_out, src).0
}

/// Gallery-wide variant of [`migrate_legacy_from`]: the legacy folder is
/// scanned ONCE for version snapshots and the result shared across photos —
/// the per-photo call re-ran that `read_dir` for every photo, making a big
/// import O(photos × directory entries). Returns how many photos had
/// anything migrated.
pub fn migrate_legacy_from_many(legacy_out: &Path, photos: &[PathBuf]) -> usize {
    if !legacy_out.is_dir() {
        return 0;
    }
    // stem → its legacy "v<N>" snapshot files. A snapshot is named
    // "<stem>.v<digits>.recipe.json"; stems may contain dots, so split at the
    // LAST ".v<digits>" tail and require the head to be an actual gallery
    // stem (same answer the old per-stem prefix scan produced).
    let stems: std::collections::HashSet<&str> =
        photos.iter().map(|p| crate::pipeline::stem(p)).collect();
    let mut versions: std::collections::HashMap<String, Vec<(PathBuf, String)>> =
        std::collections::HashMap::new();
    if let Ok(dir) = std::fs::read_dir(legacy_out) {
        for e in dir.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(rest) = name.strip_suffix(".recipe.json") else { continue };
            let Some((head, nums)) = rest.rsplit_once(".v") else { continue };
            if nums.parse::<u32>().is_ok() && stems.contains(head) {
                versions
                    .entry(head.to_string())
                    .or_default()
                    .push((e.path(), format!("v{nums}.recipe.json")));
            }
        }
    }
    let root = store_root();
    photos
        .iter()
        .filter(|p| {
            let vjobs = versions
                .get(crate::pipeline::stem(p))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            migrate_legacy_jobs(&root, legacy_out, p, vjobs).0
        })
        .count()
}

/// Returns `(moved_anything, any_attempt_failed)` — the second flag lets the
/// memoized caller keep a FAILED photo retryable instead of silencing it for
/// the process lifetime.
fn migrate_legacy_in(root: &Path, legacy_out: &Path, src: &Path) -> (bool, bool) {
    if !legacy_out.is_dir() {
        return (false, false); // cheap gate — most calls find nothing legacy at all
    }
    let stem = crate::pipeline::stem(src);
    let vprefix = format!("{stem}.v");
    let mut vjobs: Vec<(PathBuf, String)> = Vec::new();
    if let Ok(dir) = std::fs::read_dir(legacy_out) {
        for e in dir.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(rest) = name.strip_prefix(&vprefix)
                && let Some(nums) = rest.strip_suffix(".recipe.json")
                && nums.parse::<u32>().is_ok()
            {
                vjobs.push((e.path(), format!("v{nums}.recipe.json")));
            }
        }
    }
    migrate_legacy_jobs(root, legacy_out, src, &vjobs)
}

/// The per-photo migration body, with the version-snapshot scan factored OUT
/// so gallery imports can share one directory listing (`version_jobs` =
/// `(legacy file, central v<N> name)` pairs for this photo's stem).
fn migrate_legacy_jobs(
    root: &Path,
    legacy_out: &Path,
    src: &Path,
    version_jobs: &[(PathBuf, String)],
) -> (bool, bool) {
    let stem = crate::pipeline::stem(src);
    let dev = develop_dir_in(root, src);

    // Collect (legacy file, new name) pairs.
    let mut jobs: Vec<(PathBuf, String, bool)> = Vec::new(); // (from, to-name, is_recipe)
    let lr = legacy_out.join(format!("{stem}.recipe.json"));
    if lr.exists() {
        jobs.push((lr, "recipe.json".into(), true));
    }
    let lx = legacy_out.join(format!("{stem}.xmp"));
    if lx.exists() {
        jobs.push((lx, format!("{stem}.xmp"), false));
    }
    for (from, to_name) in version_jobs {
        jobs.push((from.clone(), to_name.clone(), true));
    }
    if jobs.is_empty() {
        return (false, false);
    }
    if std::fs::create_dir_all(&dev).is_err() {
        return (false, true);
    }
    let abs_src = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let marker = dev.join("source.txt");
    if !marker.exists() {
        let _ = std::fs::write(&marker, format!("{}\n", abs_src.display()));
    }

    let mut moved = false;
    let mut failed = false;
    for (from, to_name, is_recipe) in jobs {
        let to = dev.join(&to_name);
        if to.exists() {
            // The central copy is the post-migration truth — never clobber it
            // with an older legacy file. The legacy file stays for the read
            // fallbacks (deleting user data it never copied would be worse).
            continue;
        }
        let ok = if is_recipe {
            migrate_one_recipe(&from, &to, stem, &dev, legacy_out)
        } else {
            move_file(&from, &to).is_ok()
        };
        moved |= ok;
        failed |= !ok;
    }
    (moved, failed)
}

/// Move one legacy recipe file, carrying its `./out/<stem>.<kind>.png` raster
/// references into the develop dir (rewritten to bare `<kind>.png` names).
/// Rasters shared by several recipe files move once; later files just rewrite.
///
/// Failure-ordering contract (a migration must never make things WORSE than
/// not migrating): rasters are STAGED as copies first, the rewritten recipe is
/// published next, and the legacy originals are deleted only after the recipe
/// landed. Any earlier failure rolls the staged copies back and leaves every
/// legacy file byte-identical — the read fallbacks keep serving it.
fn migrate_one_recipe(from: &Path, to: &Path, stem: &str, dev: &Path, legacy_out: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(from) else { return false };
    let Ok(mut r) = serde_json::from_str::<EditRecipe>(&text) else {
        // Unparsable (interrupted write / newer schema): move byte-for-byte so
        // the read path can keep reporting it loudly as Unreadable.
        return move_file(from, to).is_ok();
    };
    let raster_prefix = format!("{stem}.");
    // (legacy raster to delete on success — None for a bystander source that
    //  must be preserved, staged central copy to roll back on failure)
    let mut staged: Vec<(Option<PathBuf>, PathBuf)> = Vec::new();
    for m in &mut r.masks {
        if let MaskGeometry::Bitmap { path } = &mut m.mask {
            let mut p = PathBuf::from(path.as_str());
            if p.is_absolute() {
                continue; // foreign reference — not ours to move
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(bare) = name.strip_prefix(&raster_prefix).map(str::to_string) else {
                continue;
            };
            // Legacy refs are relative to the OLD launch cwd ("out/<stem>.<kind>.png").
            // PREFER the raster inside the root being migrated: a same-named
            // file under TODAY'S cwd can belong to a different context, and
            // the staged source is DELETED on success — resolving the cwd
            // file first would destroy a bystander. The recipe's own
            // cwd-relative reading stays as the fallback for a launch from
            // the original directory (where the two spellings coincide).
            let cand = legacy_out.join(name);
            if cand.exists() {
                p = cand;
            }
            let dest = dev.join(&bare);
            if dest.exists() {
                *path = bare; // already migrated by an earlier file
            } else if p.exists() && copy_atomic(&p, &dest).is_ok() {
                // Only a source inside the root BEING MIGRATED may be deleted
                // on success: the cwd-relative fallback can name a bystander
                // file from an UNRELATED context (same stem, different
                // photo) — that one is copied, never deleted.
                let ours = std::path::absolute(&p)
                    .ok()
                    .zip(std::path::absolute(legacy_out).ok())
                    .is_some_and(|(ap, al)| ap.starts_with(al));
                staged.push((ours.then(|| p.clone()), dest));
                *path = bare;
            }
            // Raster missing/uncopyable: keep the old reference — the engine's
            // missing-raster contract (inert + warning) reports it.
        }
    }
    // Publish the rewritten recipe via tmp+rename (`to` is known absent): a
    // crash mid-write must not leave a half-written central recipe shadowing
    // the intact legacy one.
    let publish = serde_json::to_string_pretty(&r).ok().and_then(|json| {
        let tmp = to.with_extension("json.tmp");
        std::fs::write(&tmp, json).ok()?;
        std::fs::rename(&tmp, to).ok()
    });
    if publish.is_none() {
        for (_, dest) in &staged {
            let _ = std::fs::remove_file(dest); // roll back — legacy stays whole
        }
        return false;
    }
    let _ = std::fs::remove_file(from);
    for (legacy, _) in &staged {
        if let Some(legacy) = legacy {
            let _ = std::fs::remove_file(legacy);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::LocalAdjustment;

    #[test]
    fn photo_key_disambiguates_same_stem_different_folder() {
        // The exact bug this module exists to fix: DSC001.ARW in two folders
        // must never share one develop dir.
        let a = photo_key(Path::new("D:/trip-a/DSC001.ARW"));
        let b = photo_key(Path::new("D:/trip-b/DSC001.ARW"));
        assert!(a.starts_with("DSC001-"), "{a}");
        assert!(b.starts_with("DSC001-"), "{b}");
        assert_ne!(a, b);
        // Stable across calls (persistent directory names).
        assert_eq!(a, photo_key(Path::new("D:/trip-a/DSC001.ARW")));
    }

    #[test]
    fn fnv1a64_is_the_reference_function() {
        // Pin the published FNV-1a test vectors: this hash names directories
        // on disk, so it must never drift.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn resolve_and_relativize_round_trip() {
        let base = std::env::temp_dir().join("autoshop-store-test-roundtrip");
        std::fs::create_dir_all(&base).unwrap();
        let raster = base.join("mask-sky.png");
        std::fs::write(&raster, b"png").unwrap();

        let mut r = EditRecipe::default();
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "mask-sky.png".into() },
            ..Default::default()
        });
        r.masks.push(LocalAdjustment {
            // Not under base and not existing → must stay untouched.
            mask: MaskGeometry::Bitmap { path: "out/other.mask.png".into() },
            ..Default::default()
        });
        resolve_mask_paths(&mut r, &base);
        let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
        assert_eq!(Path::new(path), raster.as_path(), "existing bare name resolves to base");
        let MaskGeometry::Bitmap { path } = &r.masks[1].mask else { panic!() };
        assert_eq!(path, "out/other.mask.png", "missing reference left alone");

        relativize_mask_paths(&mut r, &base);
        let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
        assert_eq!(path, "mask-sky.png", "absolute-under-base collapses back to bare");
        let _ = std::fs::remove_file(&raster);
    }

    #[test]
    fn migrate_legacy_moves_recipe_xmp_versions_and_rasters() {
        // Runs against the real cwd ./out (like the render/fit tests) with
        // unique _store_mig_* names, and a temp store root via the _in APIs —
        // no process-global env mutation.
        let stem = "_store_mig_photo";
        let src = PathBuf::from(format!("D:/nowhere/{stem}.ARW"));
        let root = std::env::temp_dir().join("autoshop-store-test-migrate");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all("out").unwrap();

        let raster = PathBuf::from(format!("out/{stem}.mask-sky.png"));
        std::fs::write(&raster, b"png").unwrap();
        let mut r = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: raster.to_string_lossy().into_owned() },
            ..Default::default()
        });
        let rj = PathBuf::from(format!("out/{stem}.recipe.json"));
        std::fs::write(&rj, serde_json::to_string_pretty(&r).unwrap()).unwrap();
        let xmp = PathBuf::from(format!("out/{stem}.xmp"));
        std::fs::write(&xmp, "<x:xmpmeta/>").unwrap();
        let v2 = PathBuf::from(format!("out/{stem}.v2.recipe.json"));
        std::fs::write(&v2, serde_json::to_string_pretty(&EditRecipe::default()).unwrap()).unwrap();

        assert_eq!(
            migrate_legacy_in(&root, Path::new("out"), &src),
            (true, false),
            "moved everything, no failures"
        );
        let dev = develop_dir_in(&root, &src);
        assert!(dev.join("recipe.json").exists());
        assert!(dev.join(format!("{stem}.xmp")).exists());
        assert!(dev.join("v2.recipe.json").exists());
        assert!(dev.join("mask-sky.png").exists(), "raster moved along");
        assert!(dev.join("source.txt").exists(), "breadcrumb written");
        assert!(!rj.exists() && !xmp.exists() && !v2.exists() && !raster.exists(), "legacy consumed");

        // The migrated recipe references the raster by bare name.
        let back: EditRecipe =
            serde_json::from_str(&std::fs::read_to_string(dev.join("recipe.json")).unwrap()).unwrap();
        let MaskGeometry::Bitmap { path } = &back.masks[0].mask else { panic!() };
        assert_eq!(path, "mask-sky.png");

        // Idempotent: a second call finds nothing legacy and reports
        // (nothing moved, nothing failed).
        assert_eq!(migrate_legacy_in(&root, Path::new("out"), &src), (false, false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_snapshot_is_a_copy_and_freezes_rasters() {
        // Unique fake path → its own hashed develop dir under the real store
        // root (same isolation pattern as the GUI sidecar test); scrubbed
        // before and after.
        let src = PathBuf::from("D:/nowhere/_store_backup_test.ARW");
        let dev = develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("mask-zone-sky.png"), b"OLD").unwrap();
        let mut r = EditRecipe { exposure_ev: 0.4, ..Default::default() };
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "mask-zone-sky.png".into() },
            ..Default::default()
        });
        std::fs::write(recipe_target(&src), serde_json::to_string_pretty(&r).unwrap()).unwrap();

        // Identical incoming (after resolve) → no snapshot spam.
        let mut same = r.clone();
        resolve_mask_paths(&mut same, &dev);
        assert_eq!(backup_saved_develop(&src, Some(&same)).unwrap(), None);

        // Unconditional snapshot: v1 appears, the WORKING recipe stays (copy,
        // not move — callers snapshot BEFORE operations that may still fail),
        // and the raster is frozen with the snapshot's reference rewritten.
        assert_eq!(backup_saved_develop(&src, None).unwrap(), Some(1));
        assert!(recipe_target(&src).exists(), "copy semantics: working recipe stays");
        let snap: EditRecipe =
            serde_json::from_str(&std::fs::read_to_string(version_target(&src, 1)).unwrap())
                .unwrap();
        let MaskGeometry::Bitmap { path } = &snap.masks[0].mask else { panic!() };
        assert_eq!(path, "v1.mask-zone-sky.png");
        // Overwrite the live raster (what a re-run zoned fit does) — the
        // frozen copy must keep the OLD bytes, or the snapshot lies.
        std::fs::write(dev.join("mask-zone-sky.png"), b"NEW").unwrap();
        assert_eq!(std::fs::read(dev.join("v1.mask-zone-sky.png")).unwrap(), b"OLD");
        // Frozen rasters never pollute the version list.
        assert_eq!(list_versions(&src), vec![1]);
        // delete_version sweeps the snapshot recipe AND its frozen raster.
        delete_version(&src, 1).unwrap();
        assert!(!version_target(&src, 1).exists());
        assert!(!dev.join("v1.mask-zone-sky.png").exists());
        let _ = std::fs::remove_dir_all(&dev);
    }
}
