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
//!     pixels.json          baked pixel-master link (retouch/reimagine origin)
//!     variants.json        GUI variant strip (background variants + active kind)
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

use std::{
    cell::RefCell,
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

// Bitmap raster paths are walked through `LocalAdjustment::bitmap_paths_mut`
// (base geometry + components in one place), so this module no longer
// pattern-matches `MaskGeometry` directly.
use crate::recipe::EditRecipe;

/// How a surface responds when another process is mutating this photo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DevelopLockMode {
    /// CLI, server, and worker threads queue behind the current mutation.
    Wait,
    /// Foreground GUI actions return `WouldBlock` and leave the canvas dirty.
    NoWait,
}

thread_local! {
    /// OS locks are not recursively lockable through independent handles.
    /// Compound surface saves call lower-level store writers, so same-thread
    /// nesting reuses the outer lock while other threads and processes still
    /// contend through the kernel.
    static HELD_DEVELOP_LOCKS: RefCell<HashSet<PathBuf>> = RefCell::new(HashSet::new());
}

struct DevelopLockGuard {
    path: PathBuf,
    file: File,
}

impl Drop for DevelopLockGuard {
    fn drop(&mut self) {
        os_develop_lock::unlock(&self.file);
        HELD_DEVELOP_LOCKS.with(|held| {
            held.borrow_mut().remove(&self.path);
        });
    }
}

/// Run one coherent per-photo store operation under a kernel-owned file lock.
/// The `.develop.lock` directory entry is persistent, but lock ownership is
/// not: the OS releases it when a process exits or is killed, so a crash
/// cannot leave a stale PID file or a permanent wait.
pub fn with_develop_lock<T, E>(
    src: &Path,
    mode: DevelopLockMode,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E>
where
    E: From<io::Error>,
{
    with_develop_lock_in(&store_root(), src, mode, f)
}

fn with_develop_lock_in<T, E>(
    root: &Path,
    src: &Path,
    mode: DevelopLockMode,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E>
where
    E: From<io::Error>,
{
    let dev = develop_dir_in(root, src);
    std::fs::create_dir_all(&dev).map_err(E::from)?;
    let path = dev.join(".develop.lock");
    if HELD_DEVELOP_LOCKS.with(|held| held.borrow().contains(&path)) {
        return f();
    }

    // truncate(false) is the INTENT, not a default: the lock lives in the
    // kernel, not in the bytes, so this file's content is irrelevant and
    // truncating it would be a pointless write against a path another process
    // may hold open. `write(true)` is required because Windows needs write
    // access on the handle it locks.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(E::from)?;
    os_develop_lock::lock(&file, mode).map_err(E::from)?;
    HELD_DEVELOP_LOCKS.with(|held| {
        held.borrow_mut().insert(path.clone());
    });
    let _guard = DevelopLockGuard { path, file };
    f()
}

#[cfg(unix)]
mod os_develop_lock {
    use super::{DevelopLockMode, File};
    use std::{
        io,
        os::fd::{AsRawFd as _, RawFd},
    };

    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    const LOCK_UN: i32 = 8;

    #[link(name = "c")]
    unsafe extern "C" {
        #[link_name = "flock"]
        fn libc_flock(fd: RawFd, operation: i32) -> i32;
    }

    pub(super) fn lock(file: &File, mode: DevelopLockMode) -> io::Result<()> {
        let operation =
            LOCK_EX | if mode == DevelopLockMode::NoWait { LOCK_NB } else { 0 };
        loop {
            // SAFETY: the descriptor remains owned by `DevelopLockGuard` for
            // the complete lock lifetime and `flock` does not retain pointers.
            if unsafe { libc_flock(file.as_raw_fd(), operation) } == 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    pub(super) fn unlock(file: &File) {
        // SAFETY: this is the same live descriptor successfully locked above.
        let _ = unsafe { libc_flock(file.as_raw_fd(), LOCK_UN) };
    }
}

#[cfg(windows)]
mod os_develop_lock {
    use super::{DevelopLockMode, File};
    use std::{
        ffi::c_void,
        io,
        os::windows::io::AsRawHandle as _,
        ptr,
    };

    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 1;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 2;
    const ERROR_LOCK_VIOLATION: i32 = 33;

    #[repr(C)]
    struct Overlapped {
        _internal: usize,
        _internal_high: usize,
        _offset: u32,
        _offset_high: u32,
        _event: *mut c_void,
    }

    impl Overlapped {
        fn zeroed() -> Self {
            Self {
                _internal: 0,
                _internal_high: 0,
                _offset: 0,
                _offset_high: 0,
                _event: ptr::null_mut(),
            }
        }
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "LockFileEx"]
        fn lock_file_ex(
            file: *mut c_void,
            flags: u32,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        #[link_name = "UnlockFileEx"]
        fn unlock_file_ex(
            file: *mut c_void,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }

    pub(super) fn lock(file: &File, mode: DevelopLockMode) -> io::Result<()> {
        let mut overlapped = Overlapped::zeroed();
        let flags = LOCKFILE_EXCLUSIVE_LOCK
            | if mode == DevelopLockMode::NoWait {
                LOCKFILE_FAIL_IMMEDIATELY
            } else {
                0
            };
        // SAFETY: the handle and OVERLAPPED remain valid for the synchronous
        // call; the guard keeps the handle open until the matching unlock.
        let ok = unsafe {
            lock_file_ex(
                file.as_raw_handle(),
                flags,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        };
        if ok != 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if mode == DevelopLockMode::NoWait && e.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "this photo is being saved by another Autoshop process",
            ));
        }
        Err(e)
    }

    pub(super) fn unlock(file: &File) {
        let mut overlapped = Overlapped::zeroed();
        // SAFETY: this is the same live handle and byte range locked above.
        let _ = unsafe {
            unlock_file_ex(
                file.as_raw_handle(),
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        };
    }
}

#[cfg(not(any(unix, windows)))]
mod os_develop_lock {
    use super::{DevelopLockMode, File};
    use std::io;

    pub(super) fn lock(_: &File, _: DevelopLockMode) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "develop locking is unsupported on this platform",
        ))
    }

    pub(super) fn unlock(_: &File) {}
}

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
/// (NTFS), so BOTH halves fold case there — one file must never produce two
/// keys just because it was opened as `D:\DSC001.ARW` once and
/// `d:\dsc001.arw` once. (The hash folded from the start; the stem prefix
/// did not, so the promise held only because NTFS resolves the two spellings
/// to one directory anyway. That same resolution keeps every pre-fold store
/// dir reachable under the folded key on the default root. A deliberately
/// case-SENSITIVE data dir — an opt-in fsutil/WSL configuration — never had
/// the one-key guarantee to begin with, and keeps any old mixed-case dirs
/// as orphans; the store's stem-cased artifact names assume a
/// case-insensitive root on Windows throughout.)
pub fn photo_key(src: &Path) -> String {
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let mut s = abs.to_string_lossy().into_owned();
    // BOTH halves normalise, from the SAME string. The hash was taken from
    // `abs` while the stem was taken from the raw `src`, so a spelling that
    // `absolute()` rewrites produced one hash and two directory names: Windows
    // drops a trailing dot when opening, so `…\DSC001.NEF` and `…\DSC001.NEF.`
    // are one file, yet `file_stem()` answered "DSC001" for the first and
    // "DSC001.NEF" for the second. The develop saved under one spelling was
    // then invisible under the other, and the next save built a fresh empty
    // develop beside it. For every ordinary path the two agree, so this
    // re-keys nothing that exists.
    let mut stem = crate::pipeline::stem(&abs).to_string();
    if cfg!(windows) {
        s = s.to_lowercase();
        // ASCII-only for the DIRECTORY NAME half. Rust's full Unicode
        // lowercase and NTFS's $UpCase table disagree — and where they do,
        // the folded name is a directory that does not exist: measured on
        // this machine, "İMG_001" folds to "i\u{307}mg_001" (one char becomes
        // two), "ΣΑΣ" to "σας" (final sigma) and "ẞ" to "ß", and NONE of the
        // three resolve back to the pre-fold directory. The develop dir would
        // silently orphan — recipe, XMP, every version snapshot, every mask
        // raster and the master link — and the next save would create a fresh
        // empty one beside it. ASCII folding is the subset NTFS always agrees
        // with, and it is the case the fold was added for.
        //
        // No migration is needed for the full-Unicode spelling this replaces:
        // it existed only between two unreleased commits of this same fix
        // wave, so no build that ever shipped could have created a directory
        // under it.
        stem = stem.to_ascii_lowercase();
    }
    let suffix = format!("-{:016x}", fnv1a64(s.as_bytes()));
    // Leave headroom for filesystem metadata and keep the same key for every
    // existing ordinary stem. Enforce both byte-oriented Unix limits and
    // UTF-16-oriented Windows limits without splitting a scalar value.
    const MAX_COMPONENT_UNITS: usize = 240;
    let mut prefix = String::new();
    let mut bytes = 0usize;
    let mut wide = 0usize;
    for ch in stem.chars() {
        let next_bytes = bytes + ch.len_utf8();
        let next_wide = wide + ch.len_utf16();
        if next_bytes + suffix.len() > MAX_COMPONENT_UNITS
            || next_wide + suffix.encode_utf16().count() > MAX_COMPONENT_UNITS
        {
            break;
        }
        prefix.push(ch);
        bytes = next_bytes;
        wide = next_wide;
    }
    format!("{prefix}{suffix}")
}

/// Two spellings of the same photo must land in the same develop dir on a
/// case-insensitive volume — and the folded name must actually RESOLVE to any
/// directory an earlier build created.
#[cfg(test)]
#[test]
fn the_stem_fold_never_invents_a_name_ntfs_cannot_resolve() {
    // ASCII case folds (the reason the fold exists).
    assert_eq!(photo_key(Path::new("D:/p/DSC001.ARW")), photo_key(Path::new("D:/p/dsc001.arw")));
    // Non-ASCII stems are left ALONE: Rust's full lowercase maps these to
    // names NTFS does not consider equal to the original, so folding them
    // would point at a directory that does not exist.
    for stem in ["\u{130}MG_001", "\u{3a3}\u{391}\u{3a3}", "\u{1e9e}"] {
        let p = format!("D:/p/{stem}.ARW");
        let key = photo_key(Path::new(&p));
        let folded = key.rsplit_once('-').expect("key is <stem>-<hash>").0;
        // Each fixture is a case where Rust's full lowercase and NTFS's own
        // folding disagree — that is what makes it a fixture at all.
        assert_ne!(
            stem.to_lowercase(),
            stem.to_ascii_lowercase(),
            "fixture must be a divergence case"
        );
        // The guarantee: no character EXPANDS (Rust maps U+0130 to two chars,
        // which can never name the same NTFS directory), and only ASCII
        // letters change — the subset NTFS folds identically.
        assert_eq!(folded.chars().count(), stem.chars().count(), "no expansion: {folded}");
        assert_eq!(folded, stem.to_ascii_lowercase(), "only ASCII letters fold: {folded}");
    }
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
    for m in r.masks.iter_mut() {
        for path in m.bitmap_paths_mut() {
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
    with_develop_lock(src, DevelopLockMode::Wait, || {
        write_pixel_source_unlocked(src, origin, generated)
    })
}

fn write_pixel_source_unlocked(
    src: &Path,
    origin: &Path,
    generated: bool,
) -> std::io::Result<()> {
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
    publish_json_sidecar(
        src,
        "pixels.json",
        serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other)?,
    )
}

/// The ONE retire-and-publish primitive for the small per-photo JSON sidecars
/// (`pixels.json`, `variants.json`). Same publish discipline as recipe.json:
/// per-process AND per-call tmp name (the web server threads requests), and
/// the old file is RETIRED to `<name>.bak` rather than deleted before the
/// rename — a crash in the window then leaves the previous record recoverable
/// beside the photo, never nothing at all. Both consumers must keep a
/// matching entry in [`recover_orphan_baks`]'s pair list, or the crash-window
/// `.bak` this leaves behind is never republished.
fn publish_json_sidecar(src: &Path, name: &str, bytes: Vec<u8>) -> std::io::Result<()> {
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
    let rm = |p: PathBuf| match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    rm(develop_dir(src).join("pixels.json.bak"))?;
    rm(pixel_source_path(src))
}

/// GUI-only sidecar persisting the photo's VARIANT STRIP beyond the single
/// saved develop: `variants.json`. recipe.json + pixels.json stay the
/// cross-surface authority for the ACTIVE develop (CLI, web and export never
/// read this file); what THEY cannot carry is the rest of the strip — the
/// three-valued per-variant kind and each background variant's own recipe +
/// baked-raster origin. Before this file existed, every reopen collapsed the
/// strip to one card whose kind was guessed from the 2-valued pixels.json
/// flag (a `Fitted` card silently reopened as `Original`), and background
/// variants counted as permanently-unsavable work that pinned the quit
/// guard's dialog forever.
pub fn variants_path(src: &Path) -> PathBuf {
    develop_dir(src).join("variants.json")
}

/// One BACKGROUND variant in a [`VariantsRecord`]: three-valued kind
/// ("original" | "generated" | "fitted"), the variant's full develop recipe,
/// and its baked raster origin when the variant is pixel-based. Base pixels
/// are NOT stored — they re-decode from `origin`; source-based variants
/// re-develop the shared source.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct VariantEntry {
    pub kind: String,
    pub recipe: EditRecipe,
    #[serde(default)]
    pub origin: Option<PathBuf>,
}

/// The strip minus the active variant (whose develop recipe.json owns),
/// mirroring the GUI's navigation-stash shape: `others` + where the active
/// card sits + the active card's kind (the ONE fact about the active variant
/// recipe.json cannot express).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct VariantsRecord {
    /// Format version; readers refuse a future major they cannot honour.
    pub v: u32,
    pub active_kind: String,
    pub active_pos: usize,
    pub others: Vec<VariantEntry>,
}

fn known_variant_kind(kind: &str) -> bool {
    matches!(kind, "original" | "generated" | "fitted")
}

/// Persist the strip record. Origins inside the develop dir are stored by
/// bare name and each entry's Bitmap mask references are relativized — the
/// same relocatability rules recipe.json and pixels.json follow.
pub fn write_variants(src: &Path, rec: &VariantsRecord) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || write_variants_unlocked(src, rec))
}

fn write_variants_unlocked(src: &Path, rec: &VariantsRecord) -> std::io::Result<()> {
    let dir = develop_dir(src);
    let mut stored = VariantsRecord {
        v: rec.v,
        active_kind: rec.active_kind.clone(),
        active_pos: rec.active_pos,
        others: Vec::with_capacity(rec.others.len()),
    };
    for e in &rec.others {
        let origin = match &e.origin {
            Some(o) if o.parent() == Some(dir.as_path()) => {
                Some(o.file_name().map(PathBuf::from).unwrap_or_else(|| o.clone()))
            }
            Some(o) => Some(std::path::absolute(o)?),
            None => None,
        };
        let mut recipe = e.recipe.clone();
        relativize_mask_paths(&mut recipe, &dir);
        stored.others.push(VariantEntry { kind: e.kind.clone(), recipe, origin });
    }
    publish_json_sidecar(
        src,
        "variants.json",
        serde_json::to_vec_pretty(&stored).map_err(std::io::Error::other)?,
    )
}

/// The photo's persisted strip record, if one exists and parses. Origins and
/// Bitmap mask references come back resolved against the develop dir; their
/// EXISTENCE is deliberately not checked here — the GUI restore is the one
/// place that can degrade per-variant honestly (toast + neutral develop)
/// instead of silently dropping a variant's recipe with its raster.
/// A missing file is silent (the normal single-card case); an existing file
/// that cannot be honoured warns on stderr and degrades to None, exactly like
/// [`read_pixel_source`].
pub fn read_variants(src: &Path) -> Option<VariantsRecord> {
    let _ = recover_orphan_baks(src);
    let sidecar = variants_path(src);
    let bytes = match std::fs::read(&sidecar) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!(
                "⚠ {} exists but cannot be read ({e}) — the variant strip is not restored",
                sidecar.display()
            );
            return None;
        }
    };
    let mut rec = match serde_json::from_slice::<VariantsRecord>(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "⚠ {} is unreadable ({e}) — the variant strip is not restored",
                sidecar.display()
            );
            return None;
        }
    };
    if rec.v != 1 {
        eprintln!(
            "⚠ {} has format v{} (this build reads v1) — the variant strip is not restored",
            sidecar.display(),
            rec.v
        );
        return None;
    }
    if !known_variant_kind(&rec.active_kind)
        || rec.others.iter().any(|entry| !known_variant_kind(&entry.kind))
    {
        eprintln!(
            "⚠ {} contains a variant kind this build does not understand — the variant strip is not restored and the file is left untouched",
            sidecar.display()
        );
        return None;
    }
    let dir = develop_dir(src);
    for e in &mut rec.others {
        if let Some(o) = &e.origin
            && o.is_relative()
        {
            let Some(origin) = contained_join(&dir, o) else {
                eprintln!(
                    "⚠ {} contains a variant origin outside its develop directory — the variant strip is not restored",
                    sidecar.display()
                );
                return None;
            };
            e.origin = Some(origin);
        }
        resolve_mask_paths(&mut e.recipe, &dir);
    }
    Some(rec)
}

/// Forget the persisted strip (the photo went back to a single card). Same
/// two-step as [`clear_pixel_source`], `.bak` first, so a crash between the
/// removals leaves the live record intact instead of resurrection bait.
pub fn clear_variants(src: &Path) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || clear_variants_unlocked(src))
}

fn clear_variants_unlocked(src: &Path) -> std::io::Result<()> {
    let rm = |p: PathBuf| match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    rm(develop_dir(src).join("variants.json.bak"))?;
    rm(variants_path(src))
}

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
                recipe.base_curve = Vec::new();
                recipe.lens_profile = Default::default();
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

fn recover_orphan_baks_unlocked(src: &Path) -> std::io::Result<()> {
    let dev = develop_dir(src);
    let mut failure: Option<std::io::Error> = None;
    for (live, bak) in [
        (recipe_target(src), dev.join("recipe.json.bak")),
        (pixel_source_path(src), dev.join("pixels.json.bak")),
        (variants_path(src), dev.join("variants.json.bak")),
    ] {
        if live.exists() || !bak.exists() {
            continue;
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
    let dev = develop_dir(src);
    // The .bak counts: a crashed publish still means a master WAS recorded.
    pixel_source_path(src).exists() || dev.join("pixels.json.bak").exists()
}

/// The photo's recorded baked pixel source, if it still resolves on disk:
/// `(master_path, is_generated)`. A missing sidecar is silent (the normal
/// parametric-only case); an EXISTING sidecar that cannot be honoured —
/// unreadable JSON or a deleted/moved master — degrades to `None` with a
/// stderr warning, so "the canvas reverted to the un-retouched source" stays
/// traceable instead of looking like data loss with no cause.
pub fn read_pixel_source(src: &Path) -> Option<(PathBuf, bool)> {
    // A crashed publish leaves the linkage only in pixels.json.bak — without
    // this the canvas silently reverts to the un-retouched source. A FAILED
    // recovery is already reported by the helper; this reader then degrades
    // to "no master" exactly as it does for any unreadable sidecar.
    let _ = recover_orphan_baks(src);
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
    let generated = match doc.get("kind").and_then(|k| k.as_str()) {
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
    let name = format!("{}.recipe.json", crate::pipeline::stem(src));
    if legacy_suppressed(src) {
        suppressed_legacy_path(src, &name)
    } else {
        legacy_file(&name)
    }
}

pub fn legacy_xmp(src: &Path) -> PathBuf {
    let name = format!("{}.xmp", crate::pipeline::stem(src));
    if legacy_suppressed(src) {
        suppressed_legacy_path(src, &name)
    } else {
        legacy_file(&name)
    }
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

const LEGACY_TOMBSTONE: &[u8] =
    b"{\"v\":1,\"legacy_fallback\":\"suppressed\",\"reason\":\"explicit_clear\"}\n";

fn legacy_tombstone_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join("legacy.tombstone")
}

fn legacy_tombstone(src: &Path) -> PathBuf {
    legacy_tombstone_in(&store_root(), src)
}

fn legacy_suppressed(src: &Path) -> bool {
    legacy_tombstone(src).exists()
}

fn suppress_legacy(src: &Path) -> std::io::Result<()> {
    durable_write(&legacy_tombstone(src), LEGACY_TOMBSTONE)
}

fn suppressed_legacy_path(src: &Path, name: &str) -> PathBuf {
    develop_dir(src).join(".legacy-suppressed").join(name)
}

/// Does this photo have ANY saved develop — central or legacy, recipe or XMP?
/// Existence-only (no parse): the gallery badge and the web list call this per
/// photo per refresh.
pub fn has_develop(src: &Path) -> bool {
    // recipe.json.bak covers the retire window: a crash between retire and
    // publish leaves ONLY the .bak — recover_orphan_baks republishes it on
    // the next real read — so answering "nothing saved" here was a lie that
    // could send a caller into a second paid analysis over a save the
    // recovery machinery still guarantees.
    let recipe = recipe_target(src);
    recipe.exists()
        || recipe.with_extension("json.bak").exists()
        || xmp_target(src).exists()
        || legacy_recipe(src).exists()
        || legacy_xmp(src).exists()
}

/// What the Lightroom sidecar BESIDE the RAW (`<dir>/<stem>.xmp`) means for
/// this photo's restore. That file is the one place LIGHTROOM itself writes,
/// and until this existed no restore surface looked at it — edits made in
/// Lightroom were silently outranked by an older stored develop. Newest
/// intent wins:
///   * byte-identical to our own projection → it is our copied export, not a
///     Lightroom edit (`None`; the app itself tells users to copy the XMP
///     beside the RAW);
///   * no stored develop at all → the sidecar IS the develop (`Only`);
///   * modified after every stored develop file → Lightroom's edit is the
///     newest intent (`NewerThanStore`);
///   * otherwise the store is newer — the user's Autoshop work outranks the
///     older Lightroom pass (`OlderThanStore`).
///
/// Read-only: the store copy is never touched here — only an explicit save
/// adopts the Lightroom edit as the develop.
pub enum LrSidecar {
    None,
    Only(String),
    NewerThanStore(String),
    OlderThanStore,
}

/// What a bounded sidecar read found. `Missing` and `Unreadable` are NOT the
/// same thing to a caller choosing a merge base: a missing file carries
/// nothing, while an unreadable one may carry the user's Lightroom work — and
/// collapsing both into one "absent" let `write_xmp_disclosed` fall back to
/// our own previous projection with no note, so an over-the-cap Lightroom
/// sidecar lost its properties in exactly the silence the merge note was
/// built to end.
pub enum SidecarRead {
    /// No file at the path.
    Missing,
    /// The file, in full.
    Ok(String),
    /// A file IS there but cannot be used, and this is why (used verbatim in
    /// user-facing notes).
    Unreadable(&'static str),
}

/// Every read of an XMP sidecar, bounded. A sidecar is metadata a user
/// RECEIVES — from Lightroom, from a shared shoot, from a stranger's delivery —
/// so its size is not ours to trust: a plain `read_to_string` on a 2 GB file
/// named `DSC0001.xmp` materialises 2 GB in a request thread just to be handed
/// to a scanner. Real ones are kilobytes; the biggest Lightroom masks documents
/// are single-digit megabytes. Over the cap the content is refused — restore
/// callers treat that as "no develop", and the XMP-writing path DISCLOSES it
/// (see [`crate::pipeline::write_xmp_disclosed`]) instead of silently merging
/// against a different base.
///
/// `Read::take` rather than a `metadata()` size check: the length is bounded by
/// what was actually read, so a file that grows between the two syscalls cannot
/// widen the allocation.
pub fn read_sidecar_checked(path: &Path) -> SidecarRead {
    use std::io::Read as _;
    const MAX_SIDECAR: u64 = 16 * 1024 * 1024;
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SidecarRead::Missing,
        Err(_) => return SidecarRead::Unreadable("it could not be opened"),
    };
    // BYTES first, text second. `read_to_string` on a `Take` that cuts through
    // a multi-byte character fails with InvalidData BEFORE any size check can
    // run, so an over-cap sidecar carrying CJK captions or typographic quotes
    // was reported to the user as "not readable UTF-8 text" — a false reason,
    // in a note this round exists to make truthful.
    let mut buf = Vec::new();
    match f.take(MAX_SIDECAR + 1).read_to_end(&mut buf) {
        Err(_) => SidecarRead::Unreadable("it could not be read"),
        Ok(n) if n as u64 > MAX_SIDECAR => {
            SidecarRead::Unreadable("it is larger than the 16 MiB sidecar limit")
        }
        Ok(_) => match String::from_utf8(buf) {
            Ok(s) => SidecarRead::Ok(s),
            Err(_) => SidecarRead::Unreadable("it is not readable UTF-8 text"),
        },
    }
}

/// [`read_sidecar_checked`] for the callers to whom missing and unreadable
/// really are the same (restore: either way there is no develop to restore).
pub fn read_sidecar(path: &Path) -> Option<String> {
    match read_sidecar_checked(path) {
        SidecarRead::Ok(s) => Some(s),
        SidecarRead::Missing | SidecarRead::Unreadable(_) => None,
    }
}

pub fn lightroom_sidecar(src: &Path) -> LrSidecar {
    // Only camera RAWs have a Lightroom-sidecar convention; a baked
    // PNG/TIFF's neighbouring .xmp (if any) is not ours to interpret.
    if !crate::decode::is_raw(src) {
        return LrSidecar::None;
    }
    let lr = src.with_extension("xmp");
    let Some(text) = read_sidecar(&lr) else {
        return LrSidecar::None;
    };
    for ours in [xmp_target(src), legacy_xmp(src)] {
        if read_sidecar(&ours).is_some_and(|t| t == text) {
            return LrSidecar::None;
        }
    }
    // An explicit CLEAR (Reset + save, in the GUI or the web) is newest
    // intent too: its marker competes by mtime like any store file. Without
    // it, the projection the user once copied beside the RAW — no longer
    // byte-matchable above once the clear deleted our store copies — was
    // reborn as a "foreign Lightroom edit" and resurrected the cleared
    // develop on the very next open. The LIBRARY stays untouched (deleting
    // or rewriting the user's own sidecar would break the read-only
    // contract), and a Lightroom edit made AFTER the clear still wins.
    let cleared_t = std::fs::metadata(develop_dir(src).join("cleared.txt"))
        .and_then(|m| m.modified())
        .ok();
    let lr_t = std::fs::metadata(&lr).and_then(|m| m.modified()).ok();
    if !has_develop(src) {
        return match (lr_t, cleared_t) {
            (Some(l), Some(cl)) if l <= cl => LrSidecar::None,
            _ => LrSidecar::Only(text),
        };
    }
    let store_t = [recipe_target(src), xmp_target(src), legacy_recipe(src), legacy_xmp(src)]
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
        .max(cleared_t);
    match (lr_t, store_t) {
        (Some(l), Some(s)) if l > s => LrSidecar::NewerThanStore(text),
        // has_develop said a store exists, yet none of its files answers a
        // stat — trust the sidecar, the one file that demonstrably can.
        (Some(_), None) => LrSidecar::NewerThanStore(text),
        _ => LrSidecar::OlderThanStore,
    }
}

/// Stamp "this develop was explicitly CLEARED now": the newest-intent rule
/// ranks the marker's mtime against the Lightroom sidecar beside the RAW
/// (see [`lightroom_sidecar`]), so a projection the user once copied there
/// cannot resurrect edits that were just cleared. Never counted by
/// [`has_develop`]; rewritten in place — only the mtime matters.
pub fn mark_develop_cleared(src: &Path) -> std::io::Result<()> {
    let dir = develop_dir(src);
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
    with_develop_lock(src, DevelopLockMode::Wait, || clear_develop_unlocked(src))
}

fn clear_develop_unlocked(src: &Path) -> std::io::Result<ClearOutcome> {
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
    let legacy_was_visible = !legacy_suppressed(src)
        && legacy_out_roots().into_iter().any(|root| {
            root.join(format!("{stem}.recipe.json")).exists()
                || root.join(format!("{stem}.xmp")).exists()
        });
    // The marker lands BEFORE central files are removed. A crash can therefore
    // leave the old central develop visible, or leave it cleared with legacy
    // already suppressed, but cannot expose the ambiguous fallback in between.
    suppress_legacy(src)?;
    removed |= legacy_was_visible;

    for p in [
        develop_dir(src).join("recipe.json.bak"),
        recipe_target(src),
        xmp_target(src),
        // The strip record goes with the develop it describes — a surviving
        // variants.json would resurrect background variants over a develop
        // the user explicitly cleared.
        develop_dir(src).join("variants.json.bak"),
        variants_path(src),
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
    let had_pixels = has_pixel_source(src);
    match clear_pixel_source(src) {
        Ok(()) => removed |= had_pixels,
        Err(e) => {
            first_err.get_or_insert(e);
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    let marker_warning = mark_develop_cleared(src).err().map(|e| e.to_string());
    Ok(ClearOutcome { removed, marker_warning })
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
fn contained_join(base: &Path, relative: &Path) -> Option<PathBuf> {
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
pub fn resolve_mask_paths(r: &mut EditRecipe, base: &Path) {
    for m in &mut r.masks {
        for path in m.bitmap_paths_mut() {
            let p = Path::new(path.as_str());
            if p.is_relative() {
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
                            "⚠ bitmap mask reference {path:?} escapes {} — it is disabled",
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
}

/// Inverse of [`resolve_mask_paths`] at write time: an absolute Bitmap path
/// that lives DIRECTLY inside `base` is stored as its bare file name, keeping
/// the develop dir relocatable. Paths elsewhere are stored as given.
pub fn relativize_mask_paths(r: &mut EditRecipe, base: &Path) {
    for m in &mut r.masks {
        for path in m.bitmap_paths_mut() {
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
    with_develop_lock(src, DevelopLockMode::Wait, || {
        backup_saved_develop_unlocked(src, incoming)
    })
}

fn backup_saved_develop_unlocked(
    src: &Path,
    incoming: Option<&EditRecipe>,
) -> std::io::Result<Option<u32>> {
    // Central first, then a not-yet-migrated LEGACY recipe — the read
    // fallbacks restore either, so overwriting the central slot unversioned
    // while a legacy develop still answered was silent destruction too.
    // A crashed publish's survivor is a save like any other — restore it
    // before deciding what to snapshot, or the write below destroys it. An
    // orphan we could NOT restore is an existing save we cannot see: refusing
    // beats overwriting it unversioned, the same stance as an unreadable
    // recipe below.
    recover_orphan_baks(src)?;
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
    // A NEUTRAL recipe.json is NOT what the readers restore: both the GUI
    // (`SavedDevelop::NoopOnly`) and the web (`api_recipe`) fall THROUGH it
    // to the XMP sidecar. Snapshotting the neutral JSON while the paired XMP
    // write destroys the edits it shadows is exactly the silent loss this
    // gate exists to prevent, so a neutral recipe takes the XMP-only path
    // (which change-detects and answers Ok(None) when there is no XMP
    // develop to lose — the plain neutral snapshot below then still runs).
    if parsed.as_ref().is_some_and(|r| r.is_noop())
        && let Some(n) = backup_xmp_only(src)?
    {
        return Ok(Some(n));
    }
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
    // closer to Lightroom's intent. Stamp-if-None: an old-era AUTOSHOP
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
    Ok(Some(n))
}

/// Copy each raster the recipe references INSIDE `dev` to a version-frozen
/// name (`v<n>.<name>`) and rewrite the reference. `list_versions` only parses
/// `v<N>.recipe.json`, so the frozen rasters never pollute the version list.
/// A copy failure rolls back this call's earlier copies and errors — a
/// snapshot must never silently keep pointing at a mutable live raster.
pub fn snapshot_rasters(r: &mut EditRecipe, dev: &Path, n: u32) -> std::io::Result<()> {
    for m in &mut r.masks {
        for path in m.bitmap_paths_mut() {
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
pub fn delete_version(src: &Path, n: u32) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || delete_version_unlocked(src, n))
}

fn delete_version_unlocked(src: &Path, n: u32) -> std::io::Result<()> {
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
        let _ = durable_write(&marker, format!("{}\n", abs.display()).as_bytes());
    }
}

/// Copy `from` to `to` ATOMICALLY: stage beside the destination, then rename.
/// A bare `fs::copy` can leave a PARTIAL destination on failure — which the
/// migration/backup existence checks would then trust as a completed artifact
/// forever. Clobbers an existing `to` (callers gate on their own semantics).
fn copy_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    let tmp = sibling_tmp(to);
    let result = std::fs::copy(from, &tmp)
        .and_then(|_| sync_staged(&tmp))
        .and_then(|_| durable_os::replace(&tmp, to))
        .and_then(|_| durable_os::finish_parent(to));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|_| ())
}

/// Process-wide sequence for temporary-file names. EVERY tmp minter that
/// emits into the shared `<name>.tmp.<pid>.<seq>` lexical namespace must
/// draw from this ONE counter: independent per-site counters (each starting
/// at 0) let two SAME-process writers — e.g. a version backup racing a
/// legacy-migration publish over the same v<N>.recipe.json — mint the SAME
/// tmp path and truncate each other's staged bytes before rename. (Probe
/// note: fs::rename DOES replace an existing file on Windows — verified
/// empirically — which is exactly why a corrupted tmp gets published.)
pub(crate) fn next_tmp_seq() -> u64 {
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Private staging name beside `to` (pid + the shared process-wide seq — a
/// FIXED name would let two processes truncate each other's staging file).
fn sibling_tmp(to: &Path) -> PathBuf {
    let mut name = to.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".tmp.{}.{}", std::process::id(), next_tmp_seq()));
    to.with_file_name(name)
}

fn write_staged(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flush a staged file we did not write ourselves. The handle must carry
/// WRITE access: Windows backs `sync_all` with `FlushFileBuffers`, which
/// returns ERROR_ACCESS_DENIED on a read-only handle (probed) — unlike
/// `fsync`, which Unix accepts on a read-only fd.
fn sync_staged(path: &Path) -> std::io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

/// Publish complete bytes through the one durable write protocol.
pub(crate) fn durable_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = sibling_tmp(path);
    let result = (|| {
        write_staged(&tmp, bytes)?;
        durable_os::replace(&tmp, path)?;
        durable_os::finish_parent(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Retire the live file to `<name>.bak`, durably publish its replacement, and
/// restore the retired bytes if publication fails.
///
/// The retired copy SURVIVES a successful publish. It is not a rollback buffer
/// that has served its purpose — it is this store's crash-recovery contract:
/// [`recover_orphan_baks`] republishes `<name>.bak` whenever the live file is
/// missing, which is precisely the state a crash mid-publish leaves behind.
/// Removing it on success would reinstate the "a crash leaves nothing at all"
/// case the retire step exists to prevent. Only [`clear_develop`] and
/// [`clear_pixel_source`] delete one, and they must — otherwise a develop the
/// user explicitly cleared is resurrected on the next read. Each retire
/// supersedes the previous `.bak` rather than accumulating.
pub(crate) fn durable_retire_and_write(
    path: &Path,
    bak: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut retired = false;
    if path.exists() {
        if bak.exists() {
            std::fs::remove_file(bak)?;
            durable_os::finish_parent(bak)?;
        }
        durable_os::replace(path, bak)?;
        durable_os::finish_parent(bak)?;
        retired = true;
    } else if bak.exists() {
        // Live missing but a survivor present: adopt it as the retired copy
        // through a round trip so the publish below still has something to
        // restore, and so a crash here cannot leave BOTH names empty.
        durable_os::replace(bak, path)?;
        durable_os::finish_parent(path)?;
        durable_os::replace(path, bak)?;
        durable_os::finish_parent(bak)?;
        retired = true;
    }

    if let Err(e) = durable_write(path, bytes) {
        let mut note = String::new();
        if retired
            && (durable_os::replace(bak, path).is_err()
                || durable_os::finish_parent(path).is_err())
        {
            note = format!(
                " (restoring the previous file ALSO failed — it survives at {})",
                bak.display()
            );
        }
        return Err(std::io::Error::new(e.kind(), format!("{e}{note}")));
    }

    // The retired copy STAYS. It is not a rollback buffer that has served its
    // purpose — it is this store's crash-recovery contract: `recover_orphan_baks`
    // republishes `<name>.bak` whenever the live file is missing, which is
    // exactly the state a crash mid-publish leaves behind. Deleting it here
    // reinstated the "crash leaves nothing at all" case the retire step exists
    // to prevent, and `clear_develop` / `clear_pixel_source` are the only
    // things allowed to remove one (they must, or a cleared develop
    // resurrects). Each retire supersedes the previous `.bak` above.
    let _ = retired;
    Ok(())
}

#[cfg(unix)]
mod durable_os {
    use std::{fs::File, io, path::Path};

    pub(super) fn replace(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub(super) fn finish_parent(path: &Path) -> io::Result<()> {
        let Some(parent) = path.parent() else { return Ok(()) };
        File::open(parent)?.sync_all()
    }
}

#[cfg(windows)]
mod durable_os {
    use std::{
        io,
        os::windows::ffi::OsStrExt as _,
        path::Path,
    };

    const MOVEFILE_REPLACE_EXISTING: u32 = 1;
    const MOVEFILE_WRITE_THROUGH: u32 = 8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "MoveFileExW"]
        fn move_file_ex_w(from: *const u16, to: *const u16, flags: u32) -> i32;
    }

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut out: Vec<u16> = path.as_os_str().encode_wide().collect();
        if out.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains an embedded NUL",
            ));
        }
        out.push(0);
        Ok(out)
    }

    pub(super) fn replace(from: &Path, to: &Path) -> io::Result<()> {
        let from = wide(from)?;
        let to = wide(to)?;
        // SAFETY: both buffers are NUL-terminated and remain live through the
        // synchronous call.
        let ok = unsafe {
            move_file_ex_w(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn finish_parent(_: &Path) -> io::Result<()> {
        // Windows does not expose a useful `File::open(directory).sync_all()`
        // equivalent here. `replace` therefore uses MOVEFILE_WRITE_THROUGH,
        // which waits for the move operation to be flushed before returning.
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
mod durable_os {
    use std::{io, path::Path};

    pub(super) fn replace(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub(super) fn finish_parent(_: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "durable directory publication is unsupported on this platform",
        ))
    }
}

/// Publish `tmp` at `to` WITHOUT ever replacing another writer's file.
/// `fs::rename` REPLACES an existing destination on every platform (verified
/// empirically on Windows), so the previous create_new claim + rename kept a
/// claim→rename window in which a concurrent save landing on `to` was
/// silently overwritten. `fs::hard_link` is the one std primitive with true
/// no-replace semantics (fails `AlreadyExists`); `tmp` sits in `to`'s own
/// directory, so the link never crosses volumes. Filesystems without hard
/// links (exFAT) fall back to the claim + rename dance with that documented
/// microsecond residual. `tmp` is consumed on every path. Ok(true) =
/// published, Ok(false) = someone else owns `to`.
fn publish_no_clobber(tmp: &Path, to: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    let link = |from: &Path, dest: &Path| std::fs::hard_link(from, dest);
    #[cfg(not(unix))]
    let link = |_: &Path, _: &Path| {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "use the write-through claim fallback",
        ))
    };
    publish_no_clobber_with(tmp, to, &link)
}

fn publish_no_clobber_with(
    tmp: &Path,
    to: &Path,
    link: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    if let Err(e) = sync_staged(tmp) {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    match link(tmp, to) {
        Ok(()) => {
            let _ = std::fs::remove_file(tmp);
            durable_os::finish_parent(to)?;
            return Ok(true);
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(tmp);
            return Ok(false);
        }
        Err(_) => {}
    }

    match OpenOptions::new().write(true).create_new(true).open(to) {
        Ok(file) => drop(file),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(tmp);
            return Ok(false);
        }
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            return Err(e);
        }
    }

    match durable_os::replace(tmp, to).and_then(|_| durable_os::finish_parent(to)) {
        Ok(()) => Ok(true),
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            if std::fs::metadata(to).is_ok_and(|m| m.len() == 0) {
                let _ = std::fs::remove_file(to);
                let _ = durable_os::finish_parent(to);
            }
            Err(e)
        }
    }
}

/// Move that never CLOBBERS an existing destination: stage a full copy
/// beside `to` (copying also covers the routine cross-volume case — project
/// on D:, %LOCALAPPDATA% on C:), then [`publish_no_clobber`] it. `Ok(false)`
/// = someone else owns `to` — exactly the "already migrated" outcome. The
/// source is deliberately retained: its stem-only identity is ambiguous, so
/// this photo cannot prove that it owns the legacy bytes.
fn move_file_no_clobber(from: &Path, to: &Path) -> std::io::Result<bool> {
    let tmp = sibling_tmp(to);
    if let Err(e) = std::fs::copy(from, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    publish_no_clobber(&tmp, to)
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
    if legacy_suppressed(src) {
        return false;
    }
    match with_develop_lock(src, DevelopLockMode::Wait, || {
        Ok::<_, std::io::Error>(migrate_legacy_unlocked(src))
    }) {
        Ok(moved) => moved,
        Err(e) => {
            eprintln!(
                "⚠ legacy develops for {} could not be migrated under the develop lock ({e})",
                src.display()
            );
            false
        }
    }
}

fn migrate_legacy_unlocked(src: &Path) -> bool {
    // Ahead of the memo: this is the "touching this photo" hook every reader
    // (GUI read_saved_develop, web api_recipe) already calls, and a crashed
    // publish must be repaired on EVERY touch, not once per process. A failed
    // recovery is reported by the helper and re-decided by the backup gate,
    // which refuses to overwrite what it could not snapshot.
    let _ = recover_orphan_baks(src);
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
    with_develop_lock(src, DevelopLockMode::Wait, || {
        Ok::<_, std::io::Error>(migrate_legacy_in(&store_root(), legacy_out, src).0)
    })
    .unwrap_or(false)
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
            with_develop_lock_in(&root, p, DevelopLockMode::Wait, || {
                Ok::<_, std::io::Error>(migrate_legacy_jobs(&root, legacy_out, p, vjobs).0)
            })
            .unwrap_or(false)
        })
        .count()
}

/// Returns `(moved_anything, any_attempt_failed)` — the second flag lets the
/// memoized caller keep a FAILED photo retryable instead of silencing it for
/// the process lifetime.
fn migrate_legacy_in(root: &Path, legacy_out: &Path, src: &Path) -> (bool, bool) {
    // Cheap gate — most calls find nothing legacy at all. Triage instead of
    // `is_dir()`: that folded an INACCESSIBLE directory (AV lock, offline
    // volume) into "nothing legacy", and the caller's process memo then
    // silenced the photo for the whole session.
    match std::fs::metadata(legacy_out) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return (false, false), // exists but is not a directory
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (false, false),
        Err(_) => return (false, true), // inaccessible — retryable failure
    }
    let stem = crate::pipeline::stem(src);
    let vprefix = format!("{stem}.v");
    let mut vjobs: Vec<(PathBuf, String)> = Vec::new();
    // A failed enumeration is a FAILED attempt, not an empty scan: reporting
    // success here let the process memo mark the photo done and never retry
    // the undiscovered legacy versions this session.
    let mut scan_failed = false;
    match std::fs::read_dir(legacy_out) {
        Ok(dir) => {
            for e in dir {
                let Ok(e) = e else {
                    scan_failed = true;
                    continue;
                };
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
        Err(_) => scan_failed = true,
    }
    let (moved, failed) = migrate_legacy_jobs(root, legacy_out, src, &vjobs);
    (moved, failed || scan_failed)
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
        let _ = durable_write(&marker, format!("{}\n", abs_src.display()).as_bytes());
    }

    let mut moved = false;
    let mut failed = false;
    for (from, to_name, is_recipe) in jobs {
        let to = dev.join(&to_name);
        if to.exists() {
            // The central copy is the post-migration truth — never clobber it
            // with an older legacy file. The legacy file stays for the read
            // fallbacks (deleting user data it never copied would be worse).
            // (The publishes below re-check ATOMICALLY via create_new claims;
            // this early check just skips obvious work.)
            continue;
        }
        let ok = if is_recipe {
            migrate_one_recipe(&from, &to, stem, &dev, legacy_out)
        } else {
            // Ok(false) = destination owned by a newer writer — the desired
            // outcome, not a failure. (The previous `|| to.exists()` also
            // blessed OUR OWN claim residue after a move+cleanup double
            // failure — an empty file then shadowed the intact legacy
            // artifact as a "successfully migrated" one.)
            move_file_no_clobber(&from, &to).is_ok()
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
/// landed. Any earlier failure leaves every legacy file byte-identical — the
/// read fallbacks keep serving it. Staged central copies are NOT rolled back:
/// they are identical-content derivations a concurrent migration may already
/// reference (rolling them back deleted the winner's bitmap).
fn migrate_one_recipe(from: &Path, to: &Path, stem: &str, dev: &Path, legacy_out: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(from) else { return false };
    let Ok(mut r) = serde_json::from_str::<EditRecipe>(&text) else {
        // Unparsable (interrupted write / newer schema): move byte-for-byte so
        // the read path can keep reporting it loudly as Unreadable. Ok(false)
        // = a central owner exists — success, not failure (see the
        // migrate_legacy_jobs caller for why `|| to.exists()` was wrong).
        return move_file_no_clobber(from, to).is_ok();
    };
    let raster_prefix = format!("{stem}.");
    // Legacy rasters (inside the migrated root only) to delete once the
    // recipe publish lands. Deliberately NOT a rollback list: a staged
    // central raster is an identical-content derivation of the legacy bytes,
    // and a concurrent migration may already have ADOPTED it into its own
    // published recipe — rolling it back deleted the winner's bitmap.
    // Unreferenced survivors fall under the accepted superseded-raster
    // boundary.

    for m in &mut r.masks {
        for path in m.bitmap_paths_mut() {
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
                *path = bare; // already migrated by an earlier file / process
                continue;
            }
            if !p.exists() {
                // Keep the old reference — the engine's missing-raster
                // contract reports it.
                continue;
            }
            // Stage a private full copy, then hard-link-publish (see
            // publish_no_clobber): the old create_new claim exposed a 0-byte
            // dest between claim and copy, and its rollback could delete a
            // raster a concurrent migration had already adopted.
            let tmp = sibling_tmp(&dest);
            if std::fs::copy(&p, &tmp).is_err() {
                let _ = std::fs::remove_file(&tmp);
                continue; // keep the old reference
            }
            // On Err the old reference is kept — the engine's missing-raster
            // contract reports it.
            if let Ok(published) = publish_no_clobber(&tmp, &dest) {
                if published {
                    // Only a source inside the root BEING MIGRATED may be
                    // deleted on success: the cwd-relative fallback can
                    // name a bystander file from an UNRELATED context
                    // (same stem, different photo) — that one is copied,
                    // never deleted.

                }
                // Published OR adopted (an identical-content copy landed
                // meanwhile): the central raster is in place either way.
                *path = bare;
            }
        }
    }
    // Publish the rewritten recipe via the hard-link no-clobber publisher: a
    // central recipe created meanwhile (a concurrent save) must never be
    // replaced by older legacy bytes. The previous create_new claim + rename
    // still overwrote a save landing inside the claim→rename window, and its
    // failure cleanup deleted the claim blindly even after a save had
    // replaced it.
    let published = (|| -> Option<()> {
        let json = serde_json::to_string_pretty(&r).ok()?;
        let tmp = sibling_tmp(to);
        if write_staged(&tmp, json.as_bytes()).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
        matches!(publish_no_clobber(&tmp, to), Ok(true)).then_some(())
    })();
    if published.is_none() {
        // Deliberately NO raster rollback (see legacy_rasters above): every
        // legacy file is still byte-identical and keeps serving via the read
        // fallbacks, while the staged central rasters stay as harmless
        // identical-content copies a concurrent migration may already
        // reference.
        return false;
    }
    // The central copies are now authoritative for this photo, but neither the
    // stem-keyed recipe nor its rasters can be proven to belong to this path.
    // Retain all legacy bytes for older builds and same-stem photos.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    // MaskGeometry left the module-level imports when the raster-path walks
    // moved to `LocalAdjustment::bitmap_paths_mut`; the fixtures here still
    // construct geometries directly.
    use crate::recipe::{LocalAdjustment, MaskGeometry};

    /// One file must never produce two develop directories.
    ///
    /// `photo_key` is `<stem>-<hash>`, and the two halves were derived from
    /// DIFFERENT strings: the hash from `std::path::absolute(src)`, the stem
    /// from the raw `src`. Any spelling `absolute()` rewrites therefore split
    /// the key while the hash stayed identical — visible proof that the two
    /// halves disagreed rather than that they described different files.
    #[test]
    #[cfg(windows)]
    fn one_file_spelled_two_ways_is_one_develop_key() {
        // Windows drops a trailing dot when opening, so these name one file.
        let plain = photo_key(Path::new(r"D:\photos\DSC001.NEF"));
        let dotted = photo_key(Path::new(r"D:\photos\DSC001.NEF."));
        assert_eq!(plain, dotted, "a trailing dot forked the develop directory");

        // The case fold and `..` folding that already worked must keep working.
        assert_eq!(plain, photo_key(Path::new(r"d:\PHOTOS\dsc001.nef")), "case fold");
        assert_eq!(plain, photo_key(Path::new(r"D:\photos\sub\..\DSC001.NEF")), "dot-dot fold");

        // …and genuinely different photos must still get different keys.
        assert_ne!(plain, photo_key(Path::new(r"D:\photos\DSC002.NEF")));
        assert_ne!(plain, photo_key(Path::new(r"D:\other\DSC001.NEF")));
    }

    #[test]
    fn lightroom_sidecar_newest_intent_wins() {
        use std::time::{Duration, SystemTime};
        let dir = std::env::temp_dir().join("autoshop-store-test-lr-sidecar");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_lr_probe.arw"); // never read — only its neighbours are
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);

        // No sidecar at all → None.
        assert!(matches!(lightroom_sidecar(&raw), LrSidecar::None), "no file");

        // A sidecar and NO stored develop → the sidecar IS the develop.
        let lr = dir.join("_store_lr_probe.xmp");
        let lr_v1 =
            crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() });
        std::fs::write(&lr, &lr_v1).unwrap();
        assert!(matches!(lightroom_sidecar(&raw), LrSidecar::Only(_)), "only");

        // Our own projection copied beside the RAW is NOT a Lightroom edit.
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(xmp_target(&raw), &lr_v1).unwrap();
        assert!(matches!(lightroom_sidecar(&raw), LrSidecar::None), "our own copy");

        // A DIFFERENT sidecar, OLDER than the store → the store wins. Times
        // are SET, not slept for — deterministic on any filesystem.
        std::fs::write(
            recipe_target(&raw),
            serde_json::to_string(&EditRecipe::default()).unwrap(),
        )
        .unwrap();
        let lr_v2 =
            crate::xmp::recipe_to_xmp(&EditRecipe { contrast: -30.0, ..Default::default() });
        std::fs::write(&lr, &lr_v2).unwrap();
        let set = |p: &Path, t: SystemTime| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        let now = SystemTime::now();
        set(&lr, now - Duration::from_secs(7200));
        assert!(
            matches!(lightroom_sidecar(&raw), LrSidecar::OlderThanStore),
            "the store is newer — Autoshop work outranks the older Lightroom pass"
        );

        // …and NEWER than the store → Lightroom's edit is the newest intent.
        set(&lr, now + Duration::from_secs(7200));
        let LrSidecar::NewerThanStore(text) = lightroom_sidecar(&raw) else {
            panic!("a newer Lightroom sidecar must win the restore");
        };
        assert_eq!(text, lr_v2);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    #[test]
    fn detach_rasters_frees_a_loaded_version_from_its_snapshot() {
        let base = std::env::temp_dir().join("autoshop-store-test-detach");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("DSC_DETACH.ARW");
        std::fs::write(&photo, b"raw").unwrap();
        let dev = develop_dir(&photo);
        std::fs::create_dir_all(&dev).unwrap();
        // A version-frozen raster, exactly as save_version writes it.
        let frozen = dev.join("v3.mask-sky.png");
        std::fs::write(&frozen, b"raster bytes").unwrap();
        let mut r = EditRecipe::default();
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: frozen.to_string_lossy().into_owned() },
            ..Default::default()
        });
        detach_rasters(&photo, &mut r, "mask-restored");
        let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
        assert_ne!(Path::new(path), frozen, "must no longer point at the snapshot's file");
        assert!(Path::new(path).exists(), "the copy must exist");
        assert_eq!(std::fs::read(path).unwrap(), b"raster bytes", "content preserved");
        // Deleting the version's raster now leaves the live mask intact.
        std::fs::remove_file(&frozen).unwrap();
        assert!(Path::new(path).exists(), "live mask survives the version delete");
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn orphan_bak_is_restored_on_read() {
        // The crashed-publish window: recipe.json gone, recipe.json.bak holds
        // the develop. A reader must see the develop, not "unedited".
        let base = std::env::temp_dir().join("autoshop-store-test-orphanbak");
        let _ = std::fs::remove_dir_all(&base);
        let photo = base.join("DSC_ORPHAN.ARW");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&photo, b"not a real raw").unwrap();
        // develop_dir is keyed by absolute path; build it through the API.
        let dev = develop_dir(&photo);
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("recipe.json.bak"), b"{\"exposure\":1.0}").unwrap();
        assert!(!recipe_target(&photo).exists(), "precondition: live file gone");
        recover_orphan_baks(&photo).expect("a restorable orphan must report success");
        assert!(recipe_target(&photo).exists(), "the survivor must be restored");
        assert!(!dev.join("recipe.json.bak").exists(), "and consumed");
        // A live file always wins: a second .bak must NOT clobber it.
        std::fs::write(dev.join("recipe.json.bak"), b"{\"exposure\":9.0}").unwrap();
        recover_orphan_baks(&photo).expect("a live file present is not a failure");
        assert!(
            dev.join("recipe.json.bak").exists(),
            "adoption keeps the retired .bak — live + .bak is the normal post-publish state"
        );
        // A RECORDED master is visible even when unusable (the predicate the
        // GUI warning needs — read_pixel_source cannot answer this).
        assert!(!has_pixel_source(&photo), "no pixels sidecar yet");
        std::fs::write(dev.join("pixels.json"), b"{ not json").unwrap();
        assert!(has_pixel_source(&photo), "a corrupt sidecar still means RECORDED");
        assert!(read_pixel_source(&photo).is_none(), "...while the reader honestly fails");
        let live = std::fs::read_to_string(recipe_target(&photo)).unwrap();
        assert!(live.contains("1.0"), "live file must win: {live}");
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_cleared_develop_outranks_the_stale_copied_projection() {
        use std::time::{Duration, SystemTime};
        let dir = std::env::temp_dir().join("autoshop-store-test-cleared");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_cleared_probe.arw");
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        // The user's stale copied projection beside the RAW; the store holds
        // NOTHING (the clear deleted it) — this used to read as a foreign
        // Lightroom edit and resurrect the cleared develop.
        let lr = dir.join("_store_cleared_probe.xmp");
        std::fs::write(
            &lr,
            crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() }),
        )
        .unwrap();
        mark_develop_cleared(&raw).unwrap();
        let set = |p: &Path, t: SystemTime| {
            std::fs::OpenOptions::new().write(true).open(p).unwrap().set_modified(t).unwrap();
        };
        let now = SystemTime::now();
        set(&lr, now - Duration::from_secs(3600)); // sidecar predates the clear
        assert!(
            matches!(lightroom_sidecar(&raw), LrSidecar::None),
            "a sidecar older than the clear must NOT resurrect the develop"
        );
        // A Lightroom edit made AFTER the clear is newest intent — it wins.
        set(&lr, now + Duration::from_secs(3600));
        assert!(
            matches!(lightroom_sidecar(&raw), LrSidecar::Only(_)),
            "a post-clear Lightroom edit still restores"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    #[test]
    fn a_live_file_survives_recovery_and_keeps_its_retired_bak() {
        // What this pins: recovery must not touch a develop that already has
        // a live file, and the retired `.bak` beside it stays (live + .bak is
        // the normal post-publish state, not an orphan to consume).
        //
        // What it does NOT pin, stated plainly because the earlier version of
        // this comment claimed otherwise: it never reaches
        // `publish_no_clobber`, because recovery skips the row as soon as the
        // live file exists. The window the no-clobber publish exists for —
        // another process landing the live file BETWEEN that check and the
        // publish — cannot be staged single-threaded. The primitive itself is
        // covered directly by `publish_no_clobber_never_replaces_an_owner`
        // and `publish_no_clobber_lands_on_a_fresh_destination`.
        let base = std::env::temp_dir().join("autoshop-store-test-noclobber");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("DSC_NOCLOBBER.ARW");
        std::fs::write(&photo, b"raw").unwrap();
        let dev = develop_dir(&photo);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        // The crashed publish's survivor AND a concurrent save's live file.
        std::fs::write(dev.join("recipe.json.bak"), b"{\"contrast\":11.0}").unwrap();
        std::fs::write(recipe_target(&photo), b"{\"contrast\":22.0}").unwrap();
        recover_orphan_baks(&photo).expect("a live file present is not a failure");
        let live = std::fs::read_to_string(recipe_target(&photo)).unwrap();
        assert!(
            live.contains("22.0"),
            "the CONCURRENT save owns the live file — the .bak must not replace it: {live}"
        );
        assert!(
            dev.join("recipe.json.bak").exists(),
            "and the retired .bak stays put; live + .bak is the normal state"
        );
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clear_develop_leaves_no_resurrection_route() {
        // The whole contract of an explicit clear, in the one place both
        // surfaces now go through: every home gone, the retired master gone
        // with it (a `.bak` the next open would have republished), and the
        // newest-intent marker stamped.
        let base = std::env::temp_dir().join("autoshop-store-test-cleardev");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("DSC_CLEARDEV.ARW");
        std::fs::write(&photo, b"raw").unwrap();
        let dev = develop_dir(&photo);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let master_a = dev.join("master-a.png");
        let master_b = dev.join("master-b.png");
        std::fs::write(&master_a, b"A").unwrap();
        std::fs::write(&master_b, b"B").unwrap();
        write_pixel_source(&photo, &master_a, false).unwrap();
        write_pixel_source(&photo, &master_b, false).unwrap();
        // EVERY removal target the primitive owns — including the retired
        // recipe.json.bak, whose own resurrection route this gate previously
        // asserted about without ever creating one.
        std::fs::write(recipe_target(&photo), b"{}").unwrap();
        std::fs::write(dev.join("recipe.json.bak"), b"{\"contrast\":30.0}").unwrap();
        std::fs::write(xmp_target(&photo), b"<x:xmpmeta/>").unwrap();
        // The strip record is a removal target too — live AND retired: a
        // surviving variants.json(.bak) resurrects background variants over
        // a develop the user explicitly cleared.
        std::fs::write(variants_path(&photo), b"{\"v\":1}").unwrap();
        std::fs::write(dev.join("variants.json.bak"), b"{\"v\":1}").unwrap();
        let _ = std::fs::create_dir_all("out");
        std::fs::write(legacy_recipe(&photo), b"{}").unwrap();
        std::fs::write(legacy_xmp(&photo), b"<x:xmpmeta/>").unwrap();
        assert!(dev.join("pixels.json.bak").exists(), "precondition: a retired master exists");
        assert!(has_develop(&photo), "precondition: a develop exists");
        let out = clear_develop(&photo).expect("the clear must succeed");
        assert!(out.removed, "files really went away");
        assert!(out.marker_warning.is_none(), "the marker was stamped");
        assert!(!has_develop(&photo), "no sidecar survives");
        assert!(!has_pixel_source(&photo), "no master survives — the .bak included");
        for p in [xmp_target(&photo), legacy_recipe(&photo), legacy_xmp(&photo)] {
            assert!(!p.exists(), "every home is cleared, including {}", p.display());
        }
        assert!(
            !dev.join("recipe.json.bak").exists(),
            "the retired recipe goes too — recover_orphan_baks would republish it"
        );
        assert!(dev.join("cleared.txt").exists(), "the newest-intent marker is stamped");
        // The resurrection probe itself: every reader recovers orphans first.
        recover_orphan_baks(&photo).unwrap();
        assert!(read_pixel_source(&photo).is_none(), "nothing resurrects the cleared retouch");
        // Non-vacuous now: a recipe.json.bak DID exist before the clear, so
        // this really exercises the recovery's recipe row.
        assert!(!has_develop(&photo), "and nothing resurrects the cleared recipe");
        assert!(
            !variants_path(&photo).exists() && !dev.join("variants.json.bak").exists(),
            "the strip record goes with the develop — live and retired"
        );
        // Clearing an already-clean develop is a no-op, not an error.
        let again = clear_develop(&photo).expect("idempotent");
        assert!(!again.removed, "nothing left to remove");
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn variants_record_round_trips_relocatable_and_recovers_its_bak() {
        let base = std::env::temp_dir().join("autoshop-store-test-variants");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("DSC_VARS.ARW");
        std::fs::write(&photo, b"raw").unwrap();
        let dev = develop_dir(&photo);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        // Nothing persisted yet — the normal single-card case is silent None.
        assert!(read_variants(&photo).is_none(), "no record, no strip");
        // A generated background variant: raster origin inside the develop
        // dir, recipe carrying a Bitmap mask that also lives there.
        let master = dev.join("reimagine-1.png");
        std::fs::write(&master, b"png").unwrap();
        std::fs::write(dev.join("mask-sky.png"), b"png").unwrap();
        let mut gen_recipe = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        gen_recipe.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: dev.join("mask-sky.png").to_string_lossy().into_owned() },
            ..Default::default()
        });
        let rec = VariantsRecord {
            v: 1,
            active_kind: "fitted".into(),
            active_pos: 2,
            others: vec![
                VariantEntry {
                    kind: "original".into(),
                    recipe: EditRecipe { contrast: 12.0, ..Default::default() },
                    origin: None,
                },
                VariantEntry {
                    kind: "generated".into(),
                    recipe: gen_recipe,
                    origin: Some(master.clone()),
                },
            ],
        };
        write_variants(&photo, &rec).unwrap();
        // RELOCATABLE on disk: in-dev origin and mask stored by bare name.
        let raw = std::fs::read_to_string(variants_path(&photo)).unwrap();
        assert!(raw.contains("\"reimagine-1.png\""), "origin stored bare, got:\n{raw}");
        assert!(raw.contains("\"mask-sky.png\""), "mask raster stored bare, got:\n{raw}");
        assert!(!raw.contains(&dev.to_string_lossy().replace('\\', "\\\\")), "no absolute dev paths leak");
        // Reader resolves both back to absolute in-dev paths.
        let back = read_variants(&photo).expect("record parses");
        assert_eq!(back.active_kind, "fitted");
        assert_eq!(back.active_pos, 2);
        assert_eq!(back.others.len(), 2);
        assert_eq!(back.others[0].kind, "original");
        assert_eq!(back.others[0].recipe.contrast, 12.0);
        assert_eq!(back.others[0].origin, None);
        assert_eq!(back.others[1].origin.as_deref(), Some(master.as_path()));
        let MaskGeometry::Bitmap { path } = &back.others[1].recipe.masks[0].mask else { panic!() };
        assert_eq!(Path::new(path), dev.join("mask-sky.png"), "mask ref resolved to the dev dir");
        // Crash-window recovery: a second write retires the first to .bak;
        // losing the live file must republish it (the recover_orphan_baks
        // pair registered for variants.json).
        write_variants(&photo, &rec).unwrap();
        assert!(dev.join("variants.json.bak").exists(), "precondition: a retired record exists");
        std::fs::remove_file(variants_path(&photo)).unwrap();
        assert!(read_variants(&photo).is_some(), "the .bak republishes through the reader");
        // Explicit clear detaches BOTH copies and stays silent-idempotent.
        clear_variants(&photo).unwrap();
        assert!(read_variants(&photo).is_none(), "cleared means gone");
        assert!(!dev.join("variants.json.bak").exists(), "no resurrection bait left");
        clear_variants(&photo).unwrap();
        // A future format version is refused, not misread.
        std::fs::write(variants_path(&photo), b"{\"v\":9,\"active_kind\":\"original\",\"active_pos\":0,\"others\":[]}").unwrap();
        assert!(read_variants(&photo).is_none(), "future major refused");
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clear_pixel_source_detaches_the_retired_bak_too() {
        // Detach sequence: master A saved, master B saved (A retired to
        // .bak), then a parametric-only save clears the linkage. Without the
        // .bak removal, the next open's recover_orphan_baks resurrected
        // master A — a state the user explicitly left.
        let base = std::env::temp_dir().join("autoshop-store-test-clearbak");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("DSC_CLEAR.ARW");
        std::fs::write(&photo, b"raw").unwrap();
        let dev = develop_dir(&photo);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let master_a = dev.join("master-a.png");
        let master_b = dev.join("master-b.png");
        std::fs::write(&master_a, b"A").unwrap();
        std::fs::write(&master_b, b"B").unwrap();
        write_pixel_source(&photo, &master_a, false).unwrap();
        write_pixel_source(&photo, &master_b, false).unwrap();
        assert!(dev.join("pixels.json.bak").exists(), "precondition: A retired to .bak");
        clear_pixel_source(&photo).unwrap();
        assert!(!dev.join("pixels.json.bak").exists(), "detach must take the .bak too");
        assert!(!has_pixel_source(&photo), "nothing recorded anymore");
        assert!(read_pixel_source(&photo).is_none(), "and nothing resurrects on read");
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn tmp_names_are_process_unique_across_minters() {
        // The collision class R8 closed: every minter shares next_tmp_seq,
        // so no two tmp names for the same target can ever coincide.
        let t = Path::new("D:/x/v1.recipe.json");
        assert_ne!(sibling_tmp(t), sibling_tmp(t));
        let a = next_tmp_seq();
        let b = next_tmp_seq();
        assert!(b > a, "strictly monotone within the process");
    }

    #[test]
    fn publish_no_clobber_never_replaces_an_owner() {
        let dir = std::env::temp_dir().join("autoshop-store-test-pnc-owner");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("recipe.json");
        std::fs::write(&to, b"newer save").unwrap();
        let tmp = sibling_tmp(&to);
        std::fs::write(&tmp, b"older legacy").unwrap();
        assert!(!publish_no_clobber(&tmp, &to).unwrap(), "owner must win");
        assert_eq!(std::fs::read(&to).unwrap(), b"newer save");
        assert!(!tmp.exists(), "tmp must be consumed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn publish_no_clobber_lands_on_a_fresh_destination() {
        let dir = std::env::temp_dir().join("autoshop-store-test-pnc-fresh");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("recipe.json");
        let tmp = sibling_tmp(&to);
        std::fs::write(&tmp, b"payload").unwrap();
        assert!(publish_no_clobber(&tmp, &to).unwrap());
        assert_eq!(std::fs::read(&to).unwrap(), b"payload");
        assert!(!tmp.exists(), "tmp must be consumed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_no_clobber_adopts_owner_and_keeps_the_source() {
        let dir = std::env::temp_dir().join("autoshop-store-test-mnc-adopt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("legacy.xmp");
        let to = dir.join("central.xmp");
        std::fs::write(&from, b"legacy").unwrap();
        std::fs::write(&to, b"newer").unwrap();
        assert!(!move_file_no_clobber(&from, &to).unwrap());
        assert_eq!(std::fs::read(&to).unwrap(), b"newer", "owner intact");
        assert!(from.exists(), "adoption must not consume the source");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_gate_treats_a_nondirectory_as_empty_not_failure() {
        let dir = std::env::temp_dir().join("autoshop-store-test-gate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let not_a_dir = dir.join("out"); // exists, but is a FILE
        std::fs::write(&not_a_dir, b"x").unwrap();
        let (moved, failed) =
            migrate_legacy_in(&dir.join("root"), &not_a_dir, Path::new("D:/x/photo.arw"));
        assert!(!moved);
        assert!(!failed, "a non-directory is 'nothing legacy', not a retryable failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn photo_key_disambiguates_same_stem_different_folder() {
        // The exact bug this module exists to fix: DSC001.ARW in two folders
        // must never share one develop dir.
        let a = photo_key(Path::new("D:/trip-a/DSC001.ARW"));
        let b = photo_key(Path::new("D:/trip-b/DSC001.ARW"));
        // Windows folds BOTH halves of the key (NTFS is case-insensitive);
        // elsewhere the spelling is identity-relevant and stays.
        let want = if cfg!(windows) { "dsc001-" } else { "DSC001-" };
        assert!(a.starts_with(want), "{a}");
        assert!(b.starts_with(want), "{b}");
        assert_ne!(a, b);
        if cfg!(windows) {
            assert_eq!(
                a,
                photo_key(Path::new("d:/trip-a/dsc001.arw")),
                "case-variant spellings of ONE file must produce ONE key"
            );
        }
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
        assert!(
            rj.exists() && xmp.exists() && v2.exists() && raster.exists(),
            "ambiguous stem-keyed legacy bytes are copied, never consumed"
        );

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


        #[test]
        fn the_no_hard_link_publish_fallback_never_replaces_an_owner() {
            let dir = std::env::temp_dir().join("autoshop-store-test-pnc-fallback");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            let refuse_links = |_: &Path, _: &Path| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "forced fallback",
                ))
            };

            let owned = dir.join("owned.json");
            std::fs::write(&owned, b"newer").unwrap();
            let old_tmp = sibling_tmp(&owned);
            std::fs::write(&old_tmp, b"older").unwrap();
            assert!(!publish_no_clobber_with(&old_tmp, &owned, &refuse_links).unwrap());
            assert_eq!(std::fs::read(&owned).unwrap(), b"newer");
            assert!(!old_tmp.exists(), "the rejected stage is consumed");

            let fresh = dir.join("fresh.json");
            let fresh_tmp = sibling_tmp(&fresh);
            std::fs::write(&fresh_tmp, b"payload").unwrap();
            assert!(publish_no_clobber_with(&fresh_tmp, &fresh, &refuse_links).unwrap());
            assert_eq!(std::fs::read(&fresh).unwrap(), b"payload");
            assert!(!fresh_tmp.exists(), "the published stage is consumed");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_nonblocking_develop_lock_reports_contention_and_recovers_after_drop() {
            let root = std::env::temp_dir().join("autoshop-store-test-lock");
            let _ = std::fs::remove_dir_all(&root);
            let photo = Path::new("D:/photos/LOCK.ARW");

            with_develop_lock_in(&root, photo, DevelopLockMode::Wait, || {
                let root = root.clone();
                let busy = std::thread::spawn(move || {
                    with_develop_lock_in(
                        &root,
                        Path::new("D:/photos/LOCK.ARW"),
                        DevelopLockMode::NoWait,
                        || Ok::<_, std::io::Error>(()),
                    )
                })
                .join()
                .unwrap();
                assert_eq!(busy.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
                Ok::<_, std::io::Error>(())
            })
            .unwrap();

            with_develop_lock_in(&root, photo, DevelopLockMode::NoWait, || {
                Ok::<_, std::io::Error>(())
            })
            .expect("dropping the owner releases the kernel lock");
            let _ = std::fs::remove_dir_all(&root);
        }


        #[test]
        fn durable_write_replaces_complete_bytes_and_consumes_its_stage() {
            let dir = std::env::temp_dir().join("autoshop-store-test-durable-write");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let target = dir.join("recipe.json");
            std::fs::write(&target, b"old").unwrap();

            durable_write(&target, b"complete replacement").unwrap();

            assert_eq!(std::fs::read(&target).unwrap(), b"complete replacement");
            let stages = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
                .count();
            assert_eq!(stages, 0, "a completed durable publish consumes its stage");
            let _ = std::fs::remove_dir_all(&dir);
        }


        #[test]
        fn clearing_one_same_stem_photo_suppresses_but_never_unlinks_legacy_bytes() {
            let base = std::env::temp_dir().join("autoshop-store-test-legacy-tombstone");
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();

            let a = base.join("trip-a").join("DSC001.ARW");
            let b = base.join("trip-b").join("DSC001.ARW");
            let legacy = legacy_file("DSC001.recipe.json");
            if let Some(parent) = legacy.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&legacy, b"{\"contrast\":22.0}").unwrap();

            assert_eq!(legacy_recipe(&a), legacy);
            assert_eq!(legacy_recipe(&b), legacy);
            let outcome = clear_develop(&a).unwrap();
            assert!(outcome.removed, "suppressing a visible legacy develop is a clear");
            assert!(legacy.exists(), "the ambiguous shared file is never unlinked");
            assert!(!legacy_recipe(&a).exists(), "photo A's tombstone suppresses fallback");
            assert_eq!(legacy_recipe(&b), legacy, "photo B still sees the old store");
            assert_eq!(
                std::fs::read(&legacy).unwrap(),
                b"{\"contrast\":22.0}",
                "the legacy bytes are preserved verbatim"
            );

            let _ = std::fs::remove_file(&legacy);
            let _ = std::fs::remove_dir_all(develop_dir(&a));
            let _ = std::fs::remove_dir_all(develop_dir(&b));
            let _ = std::fs::remove_dir_all(&base);
        }


        #[test]
        fn unknown_store_kinds_are_refused_without_rewriting_the_newer_record() {
            let base = std::env::temp_dir().join("autoshop-store-test-unknown-kind");
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let photo = base.join("UNKNOWN.ARW");
            let dev = develop_dir(&photo);
            let _ = std::fs::remove_dir_all(&dev);
            std::fs::create_dir_all(&dev).unwrap();

            let master = dev.join("future-master.png");
            std::fs::write(&master, b"png").unwrap();
            let pixel_bytes = format!(
                "{{\"origin\":{},\"kind\":\"layered\"}}",
                serde_json::to_string("future-master.png").unwrap()
            );
            std::fs::write(pixel_source_path(&photo), &pixel_bytes).unwrap();
            assert!(read_pixel_source(&photo).is_none());
            assert_eq!(
                std::fs::read_to_string(pixel_source_path(&photo)).unwrap(),
                pixel_bytes
            );

            let variants = serde_json::json!({
                "v": 1,
                "active_kind": "stacked",
                "active_pos": 0,
                "others": [{
                    "kind": "original",
                    "recipe": EditRecipe::default(),
                    "origin": null
                }]
            });
            let variant_bytes = serde_json::to_vec_pretty(&variants).unwrap();
            std::fs::write(variants_path(&photo), &variant_bytes).unwrap();
            assert!(read_variants(&photo).is_none());
            assert_eq!(std::fs::read(variants_path(&photo)).unwrap(), variant_bytes);

            let _ = std::fs::remove_dir_all(&dev);
            let _ = std::fs::remove_dir_all(&base);
        }


        #[test]
        fn very_long_source_names_produce_portable_but_distinct_develop_keys() {
            let shared = "相片".repeat(180);
            let a = PathBuf::from(format!("D:/roll/{shared}a.ARW"));
            let b = PathBuf::from(format!("D:/roll/{shared}b.ARW"));
            let ka = photo_key(&a);
            let kb = photo_key(&b);

            assert!(ka.len() <= 240, "UTF-8 component is bounded: {}", ka.len());
            assert!(
                ka.encode_utf16().count() <= 240,
                "UTF-16 component is bounded"
            );
            assert_ne!(ka, kb, "the full absolute path still feeds the hash");
            assert_eq!(
                ka.rsplit_once('-').unwrap().1.len(),
                16,
                "the stable identity suffix is retained"
            );
        }


        #[test]
        fn parsed_relative_store_paths_cannot_escape_the_develop_directory() {
            let base = std::env::temp_dir().join("autoshop-store-test-contained-paths");
            let _ = std::fs::remove_dir_all(&base);
            let dev = base.join("develop");
            std::fs::create_dir_all(&dev).unwrap();
            let outside = base.join("outside.png");
            std::fs::write(&outside, b"outside").unwrap();

            assert!(contained_join(&dev, Path::new("../outside.png")).is_none());
            assert_eq!(
                contained_join(&dev, Path::new("inside.png")),
                Some(dev.join("inside.png"))
            );

            let mut recipe = EditRecipe::default();
            recipe.masks.push(LocalAdjustment {
                mask: MaskGeometry::Bitmap {
                    path: "../outside.png".into(),
                },
                ..Default::default()
            });
            resolve_mask_paths(&mut recipe, &dev);
            let MaskGeometry::Bitmap { path } = &recipe.masks[0].mask else {
                panic!()
            };
            assert_eq!(Path::new(path), dev.join(".invalid-mask-reference"));
            assert_ne!(Path::new(path), outside);

            let _ = std::fs::remove_dir_all(&base);
        }
}
