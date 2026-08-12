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
    /// Compound surface saves call lower-level store writers (and a locked
    /// settings writer's load can hit the corrupt-file rescue), so
    /// same-thread nesting reuses the outer lock while other threads and
    /// processes still contend through the kernel. Keyed by lock-file path:
    /// develop locks and the settings lock share this set.
    static HELD_FILE_LOCKS: RefCell<HashSet<PathBuf>> = RefCell::new(HashSet::new());
}

struct DevelopLockGuard {
    path: PathBuf,
    file: File,
}

impl Drop for DevelopLockGuard {
    fn drop(&mut self) {
        os_develop_lock::unlock(&self.file);
        HELD_FILE_LOCKS.with(|held| {
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
    with_path_lock(dev.join(".develop.lock"), mode, f)
}

/// The shared engine of [`with_develop_lock`] and [`with_settings_lock`]: run
/// `f` under the kernel lock on `path`. Same-thread nesting on the same lock
/// path re-enters through the thread-local held set; other threads and other
/// processes contend through the kernel.
fn with_path_lock<T, E>(
    path: PathBuf,
    mode: DevelopLockMode,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E>
where
    E: From<io::Error>,
{
    if HELD_FILE_LOCKS.with(|held| held.borrow().contains(&path)) {
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
    HELD_FILE_LOCKS.with(|held| {
        held.borrow_mut().insert(path.clone());
    });
    let _guard = DevelopLockGuard { path, file };
    f()
}

/// Run one settings-file operation — a writer's read-modify-write cycle, or
/// the corrupt-file rescue — under a cross-process kernel lock
/// (`.settings.lock` in the store root). serve's old in-process
/// `SETTINGS_LOCK` Mutex serialized only its own threads: the GUI process and
/// the serve process each load-merge-save the same `autoshop.local.json`, so
/// one process's save landing between the other's load and rename was
/// silently erased — and the file carries the API keys (L01). Kernel-owned
/// like the develop lock: a crash releases it, so no stale-lock cleanup
/// exists or is needed.
pub fn with_settings_lock<T, E>(
    mode: DevelopLockMode,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E>
where
    E: From<io::Error>,
{
    with_settings_lock_in(&store_root(), mode, f)
}

fn with_settings_lock_in<T, E>(
    root: &Path,
    mode: DevelopLockMode,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E>
where
    E: From<io::Error>,
{
    std::fs::create_dir_all(root).map_err(E::from)?;
    with_path_lock(root.join(".settings.lock"), mode, f)
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

/// Whether [`store_root`] resolved to a directory only this account can write,
/// or to the last-resort shared temp fallback.
///
/// This is a TRUST label with teeth, not a breadcrumb. The settings file lives
/// under the root ([`settings_path`]), and a settings file the loader considers
/// CENTRAL may supply an API key *and* the endpoint that key is sent to
/// (`config::SettingsOrigin`). `<temp>/autoshop` is writable by every account
/// on the machine, so a file pre-planted there inherited exactly that
/// authority — the same "extract a shared archive, run Autoshop" attack the
/// ambient guards close for the working directory, through a world-writable
/// directory instead. A shared root therefore carries AMBIENT authority.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RootTrust {
    /// `AUTOSHOP_DATA_DIR` (from the user's own environment), `%LOCALAPPDATA%`,
    /// `$XDG_DATA_HOME`, or `$HOME/.local/share` — per-account.
    PerUser,
    /// `<temp>/autoshop`: nothing better answered. Shared with every account.
    SharedFallback,
}

/// Per-user store root and its trust label. Resolution order:
/// `AUTOSHOP_DATA_DIR` (env override for tests / portable setups) → the
/// platform's own per-account data directory → `<temp>/autoshop`. Absolute, so
/// keys and targets never depend on the process cwd.
///
/// The per-account step used to be `%LOCALAPPDATA%` on EVERY platform — a
/// variable Unix does not set. So every Linux/macOS build fell through to
/// `/tmp/autoshop` and then handed the settings file found there full central
/// authority, keys and base URLs included, on a directory any local account can
/// write first. Each platform now names its own directory, and the shared
/// fallback is LABELLED rather than trusted (the loader downgrades it).
pub fn store_root_with_trust() -> (PathBuf, RootTrust) {
    let (root, trust) = std::env::var_os("AUTOSHOP_DATA_DIR")
        .map(|d| (PathBuf::from(d), RootTrust::PerUser))
        .or_else(|| per_user_data_dir().map(|d| (d.join("autoshop"), RootTrust::PerUser)))
        .unwrap_or_else(|| (std::env::temp_dir().join("autoshop"), RootTrust::SharedFallback));
    (std::path::absolute(&root).unwrap_or(root), trust)
}

pub fn store_root() -> PathBuf {
    store_root_with_trust().0
}

/// The platform's per-account data directory, or `None` when the environment
/// names none (a bare service account, a chroot without `HOME`). Read from the
/// LIVE environment only — nothing in this project writes the process
/// environment, so a `.env` cannot reach these names.
#[cfg(windows)]
fn per_user_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(not(windows))]
fn per_user_data_dir() -> Option<PathBuf> {
    // XDG first (the spec's own override), then the HOME-relative default it
    // documents. A RELATIVE `XDG_DATA_HOME` is ignored per the spec — and
    // would otherwise reintroduce a cwd-relative store root.
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
}

/// FNV-1a 64-bit. Deliberately hand-rolled: `DefaultHasher` is NOT stable
/// across Rust releases, and this hash names PERSISTENT directories — a
/// changed hash would orphan every existing develop on a toolchain bump.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
}

/// Stable per-photo key: `<stem>-<16 hex>` from the photo's IDENTITY
/// spelling — the canonical on-disk form for local volumes (C1/F10 rework,
/// user-decided 2026-08-10), so a symlink/junction/8.3/case alias of one
/// photo yields ONE key and one develop dir (and one develop LOCK — the
/// concurrency cluster's exclusion was void across aliases). Network and
/// device paths, and spellings that cannot be resolved, keep the LEXICAL
/// key (see [`identity_of`]); canonical identity does not see through
/// hardlinks or two mount points of one volume. The stem prefix keeps the
/// store browsable; the hash disambiguates same-named photos in different
/// folders. Windows paths are case-insensitive
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
    key_from_spelling(&identity_of(src))
}

/// Yesterday's key, byte-identical: the spelling handed in, made absolute
/// LEXICALLY (no link resolution). Still real, not legacy-only: it is the
/// fallback identity for network/device paths and unresolvable spellings,
/// and [`resolve_key_in`] consults it to adopt develops saved by pre-C1
/// builds.
pub(crate) fn photo_key_lexical(src: &Path) -> String {
    key_from_spelling(&std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf()))
}

fn key_from_spelling(abs: &Path) -> String {
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
    let mut stem = crate::pipeline::stem(abs).to_string();
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

/// The photo's IDENTITY spelling, memoized for the process lifetime. The
/// memo is LOAD-BEARING for correctness, not a cache: `develop_dir` must
/// answer the SAME directory for a given input path for the whole session
/// (raster re-anchoring via `parent() == dir`, the commit staging base, and
/// the develop-lock path all assume it), and canonicalize is a per-call
/// filesystem probe whose answer can flip transiently (AV holding the file,
/// a link retargeted mid-session). Consequence, documented: a link
/// retargeted while Autoshop runs keeps this session on the identity it
/// opened with.
fn identity_of(src: &Path) -> PathBuf {
    use std::sync::{Mutex, OnceLock};
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    // LEXICAL-FIRST (the F4 rule, user-decided 2026-08-10): a network or
    // device path's identity is its spelling. Canonicalizing one costs a
    // network round trip per photo — a dead mapping would hang every key
    // derivation — and the store has twice decided not to reach off this
    // machine. Note this predicate has a second duty here, distinct from
    // the F4 refusals ("a develop pack may not reach off this machine"):
    // it also means "do not spend a network round trip on identity".
    if remote_identity(&abs) {
        return abs;
    }
    static MEMO: OnceLock<Mutex<std::collections::HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(hit) = memo.lock().unwrap().get(&abs) {
        return hit.clone();
    }
    let (id, hard_fallback) = identity_spelling(&abs);
    if hard_fallback {
        // HARD fallback only (no ancestor resolved — e.g. a disconnected
        // drive): a merely-absent leaf still resolves through its parent
        // and is not disclosed. Once per photo per process via the memo.
        eprintln!(
            "⚠ {} could not be resolved to its on-disk form — its develop is keyed by the \
             path spelling, so a link or short-name alias of this photo would get a \
             separate develop",
            abs.display()
        );
    }
    let mut m = memo.lock().unwrap();
    // Bounded (the memory-boundary idiom). NOT cleared on overflow: existing
    // entries are the stability promise, so they stay; entries past the cap
    // are simply recomputed per call (deterministic for a given disk state).
    const MEMO_CAP: usize = 50_000;
    if m.len() < MEMO_CAP {
        m.entry(abs).or_insert_with(|| id.clone());
    }
    id
}

/// Resolve `abs` to its canonical on-disk spelling: canonicalize the deepest
/// EXISTING ancestor and re-attach the not-yet-created tail (the
/// `pipeline::resolve_existing_pub` shape — an absent photo still keys by
/// the folder that holds it). Returns `(spelling, hard_fallback)`; a hard
/// fallback means NOTHING resolved and the lexical spelling stands.
fn identity_spelling(abs: &Path) -> (PathBuf, bool) {
    let mut cur = abs;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(mut c) = std::fs::canonicalize(cur) {
            for t in tail.iter().rev() {
                c.push(t);
            }
            return (strip_verbatim(&c), false);
        }
        match (cur.parent(), cur.file_name()) {
            (Some(par), Some(name)) if !par.as_os_str().is_empty() => {
                tail.push(name);
                cur = par;
            }
            _ => return (abs.to_path_buf(), true),
        }
    }
}

/// Undo the `\\?\` verbatim prefix `fs::canonicalize` returns on Windows,
/// so the canonical key of a photo already spelled with its true casing on
/// a plain local drive is BYTE-IDENTICAL to its lexical key — the property
/// that keeps the overwhelming majority of existing develop dirs un-rekeyed.
/// Only explicitly-understood prefixes are rewritten; everything else is
/// left UNCHANGED — inventing a DOS spelling for e.g. `\\?\Volume{GUID}\`
/// could collide with a different photo's key, the exact cross-photo
/// clobber this module exists to prevent.
fn strip_verbatim(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut comps = p.components();
        let Some(Component::Prefix(pre)) = comps.next() else { return p.to_path_buf() };
        let root: PathBuf = match pre.kind() {
            Prefix::VerbatimDisk(letter) => PathBuf::from(format!("{}:\\", letter as char)),
            Prefix::VerbatimUNC(server, share) => {
                let mut s = std::ffi::OsString::from(r"\\");
                s.push(server);
                s.push(r"\");
                s.push(share);
                s.push(r"\");
                PathBuf::from(s)
            }
            _ => return p.to_path_buf(),
        };
        let mut out = root;
        for c in comps {
            match c {
                Component::RootDir => {}
                other => out.push(other),
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        p.to_path_buf()
    }
}

/// Network/device identity gate for [`identity_of`]: the F4 lexical
/// prefixes PLUS mapped network drive letters — `GetDriveTypeW` is a local
/// mount-table lookup, so a `Z:` pointing at a NAS is caught without any
/// network I/O.
fn remote_identity(p: &Path) -> bool {
    remote_or_device_path(p) || remote_drive_letter(p)
}

#[cfg(windows)]
fn remote_drive_letter(p: &Path) -> bool {
    use std::path::{Component, Prefix};
    let letter = match p.components().next() {
        Some(Component::Prefix(pre)) => match pre.kind() {
            Prefix::Disk(l) | Prefix::VerbatimDisk(l) => l,
            _ => return false,
        },
        _ => return false,
    };
    const DRIVE_REMOTE: u32 = 4;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "GetDriveTypeW"]
        fn get_drive_type_w(root: *const u16) -> u32;
    }
    let root: [u16; 4] = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: NUL-terminated buffer, live across the synchronous call.
    (unsafe { get_drive_type_w(root.as_ptr()) }) == DRIVE_REMOTE
}

#[cfg(not(windows))]
fn remote_drive_letter(_: &Path) -> bool {
    false
}

/// What one resolved photo told the user about its aliased past — typed, so
/// the GUI renders it in the session language at consumption time (the
/// worker-closure i18n lesson), never a pre-formatted string.
pub enum AliasNote {
    /// Edits saved under an older (lexical) spelling were adopted into the
    /// canonical develop dir.
    Adopted { from: PathBuf },
    /// BOTH spellings hold a real develop: the canonical one is in use, the
    /// other was left untouched at this path (user-decided 2026-08-10:
    /// disclose, never merge — a wrong guess silently destroys a develop).
    SecondDevelop { at: PathBuf },
}

fn alias_notes() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, AliasNote>> {
    static NOTES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, AliasNote>>> =
        std::sync::OnceLock::new();
    NOTES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Pop the alias disclosure for this photo, if its first key resolution this
/// session produced one. The GUI's open path consumes it into a toast.
pub fn take_alias_note(src: &Path) -> Option<AliasNote> {
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    alias_notes().lock().unwrap().remove(&abs)
}

fn stash_alias_note(src_abs: &Path, note: AliasNote) {
    alias_notes().lock().unwrap().insert(src_abs.to_path_buf(), note);
}

/// Which key this photo's develop lives under IN THIS ROOT — the canonical
/// key, after a one-time adoption of anything a pre-canonical build saved
/// under the lexical key. Memoized per (root, photo) for the process
/// lifetime (the same stability contract as [`identity_of`]).
fn resolve_key_in(root: &Path, src: &Path) -> String {
    use std::sync::{Mutex, OnceLock};
    let ck = photo_key(src);
    let lk = photo_key_lexical(src);
    if ck == lk {
        // The common case (plain local path, true casing): zero probes,
        // zero behaviour change.
        return ck;
    }
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    static MEMO: OnceLock<Mutex<std::collections::HashMap<(PathBuf, PathBuf), String>>> =
        OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(hit) = memo.lock().unwrap().get(&(root.to_path_buf(), abs.clone())) {
        return hit.clone();
    }
    match adopt_or_choose(root, &abs, &ck, &lk) {
        Some(key) => {
            let mut m = memo.lock().unwrap();
            const MEMO_CAP: usize = 50_000;
            if m.len() < MEMO_CAP {
                m.entry((root.to_path_buf(), abs)).or_insert_with(|| key.clone());
            }
            key
        }
        // Undecidable RIGHT NOW (another process holds a lock): fall back
        // to the lexical key for THIS call, unmemoized, so the next touch
        // retries the resolution instead of freezing the fallback.
        None => lk,
    }
}

/// Names that are per-dir machinery, never a develop's content — excluded
/// from adoption copies and from the "is there anything here" probes.
fn adoption_skips(name: &str) -> bool {
    name == ".develop.lock"
        || name == "superseded-by.txt"
        || name == "adopting-from.txt"
        || name == "adopted-from.txt"
        || name.contains(".tmp.")
}

/// The one-time adoption decision for a photo whose canonical and lexical
/// keys differ. Returns the key to use, or `None` when a lock could not be
/// taken without waiting (retry next call). All-or-nothing: two dirs that
/// BOTH hold a real develop are never merged file-by-file — a no-clobber
/// union of two generations is a franken-develop.
fn adopt_or_choose(root: &Path, abs: &Path, ck: &str, lk: &str) -> Option<String> {
    let cd = root.join("develops").join(ck);
    let ld = root.join("develops").join(lk);
    let contentful = |d: &Path| -> bool {
        std::fs::read_dir(d).ok().is_some_and(|it| {
            it.flatten().any(|e| !adoption_skips(&e.file_name().to_string_lossy()))
        })
    };
    // Already superseded by an earlier session's adoption — nothing to redo.
    if ld.join("superseded-by.txt").exists() {
        return Some(ck.to_string());
    }
    let resume = cd.join("adopting-from.txt").exists();
    if !resume {
        if !contentful(&ld) {
            // Fresh photo (or only lock litter): the canonical key, no copy.
            return Some(ck.to_string());
        }
        if contentful(&cd) {
            // GENUINE collision: both spellings hold a develop and no
            // adoption was in flight. Canonical wins, the alias stays on
            // disk untouched, and the fact is durable + surfaced (once).
            let marker = cd.join("aliased-develops.txt");
            if !marker.exists() {
                let _ = durable_write(
                    &marker,
                    format!("a second develop for this photo exists at:\n{}\n", ld.display())
                        .as_bytes(),
                );
                eprintln!(
                    "⚠ {} has a second saved develop at {} from an older path spelling — it was NOT merged",
                    abs.display(),
                    ld.display()
                );
                stash_alias_note(abs, AliasNote::SecondDevelop { at: ld.clone() });
            }
            return Some(ck.to_string());
        }
        // A pending transaction in the alias dir means its true content is
        // not settled — and the recovery helpers all derive their paths from
        // the PHOTO, which now resolves canonically, so they cannot be
        // pointed at the alias dir. Defer: this session keys lexically (the
        // pre-upgrade behaviour), the residue settles in place through
        // normal use, and the NEXT session adopts. Self-healing across two
        // sessions instead of a half-adopted transaction.
        let residue = ld.join("clear.pending").exists()
            || ld.join(".commit").exists()
            || std::fs::read_dir(&ld).ok().is_some_and(|it| {
                it.flatten().any(|e| {
                    e.file_name().to_string_lossy().starts_with(".deleting.v")
                })
            })
            || ["recipe.json", "pixels.json", "variants.json"].iter().any(|n| {
                !ld.join(n).exists() && ld.join(format!("{n}.bak")).exists()
            });
        if residue {
            eprintln!(
                "⚠ {} has unsettled develop state under its older path spelling ({}) — adoption \
                 into the canonical develop is postponed until it settles",
                abs.display(),
                ld.display()
            );
            return Some(lk.to_string());
        }
    }
    // Adopt (or resume a crashed adoption), under BOTH dir locks in a fixed
    // lexical→canonical order (total and process-independent, so two
    // processes cannot deadlock; two processes adopting concurrently copy
    // the same source bytes no-clobber and converge). with_path_lock
    // directly — with_develop_lock would re-derive the key and recurse into
    // this very function. NoWait on both: this can run on the UI thread
    // (badge fill), and a held lock postpones, never hangs.
    let adopted: std::io::Result<()> = (|| {
        std::fs::create_dir_all(&cd)?; // the lock file needs its dir
        with_path_lock(ld.join(".develop.lock"), DevelopLockMode::NoWait, || {
            with_path_lock(cd.join(".develop.lock"), DevelopLockMode::NoWait, || {
                adopt_files(&cd, &ld)
            })
        })
    })();
    match adopted {
        Ok(()) => {
            stash_alias_note(abs, AliasNote::Adopted { from: ld.clone() });
            eprintln!(
                "⚠ adopted the develop saved under {} into {} (the photo resolves there)",
                ld.display(),
                cd.display()
            );
            Some(ck.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
        Err(e) => {
            eprintln!(
                "⚠ adopting the develop at {} into {} failed ({e}) — this session keys the \
                 photo by its path spelling and the adoption retries later",
                ld.display(),
                cd.display()
            );
            Some(lk.to_string())
        }
    }
}

/// The copy half of adoption, marker-fenced like every other multi-file
/// mutation in this store: `adopting-from.txt` lands durably FIRST, the
/// files copy no-clobber (idempotent — a crashed adoption resumes, and
/// resumed copies come from the same source bytes), the completion
/// breadcrumbs land, and only then is the in-flight marker consumed. The
/// alias dir is left INTACT as a frozen backup with a `superseded-by.txt`
/// pointer — adoption copies, it never deletes.
fn adopt_files(cd: &Path, ld: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(cd)?;
    durable_write(
        &cd.join("adopting-from.txt"),
        format!("{}\n", ld.display()).as_bytes(),
    )?;
    let copy_dir = |from: &Path, to: &Path| -> std::io::Result<()> {
        for e in std::fs::read_dir(from)?.flatten() {
            let name = e.file_name();
            let name_s = name.to_string_lossy();
            if adoption_skips(&name_s) {
                continue;
            }
            let src = from.join(&name);
            if src.is_dir() {
                if name_s == ".legacy-suppressed" {
                    std::fs::create_dir_all(to.join(&name))?;
                    for c in std::fs::read_dir(&src)?.flatten() {
                        let _ = move_file_no_clobber(&c.path(), &to.join(&name).join(c.file_name()))?;
                    }
                } else {
                    eprintln!(
                        "⚠ adoption skipped unknown directory {} — it stays under the old spelling",
                        src.display()
                    );
                }
                continue;
            }
            let _ = move_file_no_clobber(&src, &to.join(&name))?;
        }
        Ok(())
    };
    copy_dir(ld, cd)?;
    durable_write(&cd.join("adopted-from.txt"), format!("{}\n", ld.display()).as_bytes())?;
    durable_write(
        &ld.join("superseded-by.txt"),
        format!("{}\n", cd.display()).as_bytes(),
    )?;
    let _ = std::fs::remove_file(cd.join("adopting-from.txt"));
    settle_consumed_marker(&cd.join("adopting-from.txt"));
    Ok(())
}

/// This photo's develop directory (not created here).
pub fn develop_dir(src: &Path) -> PathBuf {
    develop_dir_in(&store_root(), src)
}

/// Root-parameterized core of [`develop_dir`] so tests can use a temp root
/// without mutating process-global env (set_var is unsafe + racy in 2024).
fn develop_dir_in(root: &Path, src: &Path) -> PathBuf {
    root.join("develops").join(resolve_key_in(root, src))
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
    refuse_unresolved_strip(src)?;
    publish_json_sidecar(src, "variants.json", variants_record_bytes(src, rec)?)
}

/// The exact variants.json bytes [`write_variants`] publishes for `rec` —
/// exposed so [`commit_develop`] callers can stage the identical record into
/// a single-generation save. The [`refuse_unresolved_strip`] gate does NOT
/// run here: it belongs to the moment of publication, under the lock.
pub fn variants_record_bytes(src: &Path, rec: &VariantsRecord) -> std::io::Result<Vec<u8>> {
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
    serde_json::to_vec_pretty(&stored).map_err(std::io::Error::other)
}

/// What a strip read actually found — the three states are NOT collapsible:
/// an `Unresolved` file records background variants this build cannot parse,
/// and the save primitives refuse to overwrite or clear it (an ordinary
/// Ctrl+S over a single card used to DELETE the unreadable record plus its
/// `.bak`, silently destroying every background variant it held — 16-lane
/// scan L08).
pub enum VariantsRead {
    /// No `variants.json` — the normal single-card case.
    Absent,
    Strip(VariantsRecord),
    /// The file EXISTS but cannot be honoured (unreadable bytes/JSON,
    /// future format, unknown kind, escaping or network origin).
    Unresolved,
}

/// The photo's persisted strip record, if one exists and parses. Origins and
/// Bitmap mask references come back resolved against the develop dir; their
/// EXISTENCE is deliberately not checked here — the GUI restore is the one
/// place that can degrade per-variant honestly (toast + neutral develop)
/// instead of silently dropping a variant's recipe with its raster.
pub fn read_variants(src: &Path) -> Option<VariantsRecord> {
    match read_variants_checked(src) {
        VariantsRead::Strip(rec) => Some(rec),
        _ => None,
    }
}

/// [`read_variants`] with the absent/unresolved distinction preserved. A
/// missing file is silent (the normal single-card case); an existing file
/// that cannot be honoured warns on stderr and comes back `Unresolved`,
/// exactly like [`read_pixel_source`] degrades — except that save paths
/// treat `Unresolved` as a refusal, never as "nothing to keep".
pub fn read_variants_checked(src: &Path) -> VariantsRead {
    let _ = recover_orphan_baks(src);
    let sidecar = variants_path(src);
    let bytes = match read_bytes_capped(&sidecar, MAX_STORE_JSON) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return VariantsRead::Absent,
        Err(e) => {
            eprintln!(
                "⚠ {} exists but cannot be read ({e}) — the variant strip is not restored",
                sidecar.display()
            );
            return VariantsRead::Unresolved;
        }
    };
    let mut rec = match serde_json::from_slice::<VariantsRecord>(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "⚠ {} is unreadable ({e}) — the variant strip is not restored",
                sidecar.display()
            );
            return VariantsRead::Unresolved;
        }
    };
    if rec.v != 1 {
        eprintln!(
            "⚠ {} has format v{} (this build reads v1) — the variant strip is not restored",
            sidecar.display(),
            rec.v
        );
        return VariantsRead::Unresolved;
    }
    if !known_variant_kind(&rec.active_kind)
        || rec.others.iter().any(|entry| !known_variant_kind(&entry.kind))
    {
        eprintln!(
            "⚠ {} contains a variant kind this build does not understand — the variant strip is not restored and the file is left untouched",
            sidecar.display()
        );
        return VariantsRead::Unresolved;
    }
    let dir = develop_dir(src);
    for e in &mut rec.others {
        if let Some(o) = &e.origin {
            // LEXICAL network/device refusal BEFORE any probe, like the
            // pixel-source origin: a crafted UNC origin must not be touched.
            if remote_or_device_path(o) {
                eprintln!(
                    "⚠ {} contains a network/device variant origin — the variant strip is not restored",
                    sidecar.display()
                );
                return VariantsRead::Unresolved;
            }
            if o.is_relative() {
                let Some(origin) = contained_join(&dir, o) else {
                    eprintln!(
                        "⚠ {} contains a variant origin outside its develop directory — the variant strip is not restored",
                        sidecar.display()
                    );
                    return VariantsRead::Unresolved;
                };
                e.origin = Some(origin);
            }
        }
        resolve_mask_paths(&mut e.recipe, &dir);
    }
    VariantsRead::Strip(rec)
}

/// The save-path floor shared by [`write_variants`] and [`clear_variants`]:
/// an existing strip record this build cannot honour is someone's data, not
/// noise — refusing here protects every caller at once (Ctrl+S, save-all).
/// The EXPLICIT user clear (`clear_develop`) removes the file directly and
/// deliberately does not pass through this gate.
fn refuse_unresolved_strip(src: &Path) -> std::io::Result<()> {
    match read_variants_checked(src) {
        VariantsRead::Unresolved => Err(std::io::Error::other(format!(
            "{} exists but cannot be honoured — the background variants it records would be \
             destroyed; fix or delete that file, then save again",
            variants_path(src).display()
        ))),
        _ => Ok(()),
    }
}

/// Forget the persisted strip (the photo went back to a single card). Same
/// two-step as [`clear_pixel_source`], `.bak` first, so a crash between the
/// removals leaves the live record intact instead of resurrection bait.
pub fn clear_variants(src: &Path) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || clear_variants_unlocked(src))
}

fn clear_variants_unlocked(src: &Path) -> std::io::Result<()> {
    refuse_unresolved_strip(src)?;
    let rm = |p: PathBuf| match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    rm(develop_dir(src).join("variants.json.bak"))?;
    rm(variants_path(src))
}

/// One member of a [`DevelopCommit`]: publish these bytes, remove the
/// sidecar (live + retired `.bak`, the detach rule), or leave it exactly as
/// it stands.
pub enum CommitMember {
    Write(Vec<u8>),
    Clear,
    Keep,
}

impl CommitMember {
    fn word(&self) -> &'static str {
        match self {
            CommitMember::Write(_) => "write",
            CommitMember::Clear => "clear",
            CommitMember::Keep => "keep",
        }
    }
}

/// A single-generation write of the develop triple. `recipe` bytes come from
/// [`crate::pipeline::recipe_store_bytes`] (clamped + mask-relativized
/// there), `pixels` from [`pixel_source_record_bytes`], `variants` from
/// [`variants_record_bytes`] — the commit publishes, it does not interpret.
pub struct DevelopCommit {
    pub recipe: Option<Vec<u8>>,
    pub pixels: CommitMember,
    pub variants: CommitMember,
}

fn commit_dir(src: &Path) -> PathBuf {
    develop_dir(src).join(".commit")
}

/// The staged-generation manifest (`.commit/COMMIT`), whose single durable
/// rename is the transaction's linearization point. Sums are hex FNV-1a of
/// each staged member's bytes — strings, because a u64 does not survive a
/// JSON round trip intact.
#[derive(serde::Serialize, serde::Deserialize)]
struct CommitManifest {
    v: u32,
    recipe: String,
    pixels: String,
    variants: String,
    sums: std::collections::BTreeMap<String, String>,
}

/// Publish recipe.json / pixels.json / variants.json as ONE generation (L03).
///
/// Ctrl+S used to run three sequential durable writes under the develop
/// lock; the lock excludes concurrent writers, but every kill point between
/// the renames left a torn generation — a new recipe over an old (or
/// half-cleared) master link, or a new pair under a stale strip — and the
/// per-file `.bak` recovery is generation-blind, so it would republish the
/// OLD pixels link under the NEW recipe. No platform primitive swaps
/// multiple directory entries at once (renameat2 is Linux-only; MoveFileExW
/// cannot replace a directory), so the gap is closed by a MARKER: members
/// stage into `develop_dir/.commit/`, ONE durable rename of `.commit/COMMIT`
/// is the commit point, and recovery rolls FORWARD past it (an unmarked
/// stage is discarded). A crash after COMMIT therefore COMPLETES this save
/// on the photo's next locked touch instead of reverting it.
pub fn commit_develop(src: &Path, commit: DevelopCommit) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || commit_develop_unlocked(src, commit))
}

fn commit_develop_unlocked(src: &Path, commit: DevelopCommit) -> std::io::Result<()> {
    // The batch-S trap, same shape: a PENDING explicit clear must complete
    // before a new generation stages — its recovery would otherwise eat the
    // very save it predates. A pending commit resolves too: gen(n+1) cannot
    // stage over an unresolved gen(n).
    resolve_pending_clear_unlocked(src)?;
    resolve_pending_commit_unlocked(src)?;
    // The save-path floor, at the moment of publication: an unresolved strip
    // refuses BOTH the write and the clear, exactly as the member writers do.
    if !matches!(commit.variants, CommitMember::Keep) {
        refuse_unresolved_strip(src)?;
    }
    if commit.recipe.is_none()
        && matches!(commit.pixels, CommitMember::Keep)
        && matches!(commit.variants, CommitMember::Keep)
    {
        return Ok(());
    }
    let dev = develop_dir(src);
    std::fs::create_dir_all(&dev)?;
    let cdir = commit_dir(src);
    let staged = (|| -> std::io::Result<()> {
        // create_dir, not create_dir_all: a leftover `.commit` was resolved
        // above, so an existing directory here is a real error, not residue.
        std::fs::create_dir(&cdir)?;
        let mut sums = std::collections::BTreeMap::new();
        let mut stage = |name: &str, bytes: &[u8]| -> std::io::Result<()> {
            write_staged(&cdir.join(name), bytes)?;
            sums.insert(name.to_string(), format!("{:016x}", fnv1a64(bytes)));
            Ok(())
        };
        if let Some(b) = &commit.recipe {
            stage("recipe.json", b)?;
        }
        if let CommitMember::Write(b) = &commit.pixels {
            stage("pixels.json", b)?;
        }
        if let CommitMember::Write(b) = &commit.variants {
            stage("variants.json", b)?;
        }
        // The staged entries' directory records go down BEFORE the marker
        // can exist (unix: dir fsync; Windows: finish_parent is a no-op and
        // durability rests on the write-through rename below plus journaled
        // metadata, with the sums as the belt).
        durable_os::finish_parent(&cdir.join("COMMIT"))?;
        let manifest = CommitManifest {
            v: 1,
            recipe: if commit.recipe.is_some() { "write" } else { "keep" }.to_string(),
            pixels: commit.pixels.word().to_string(),
            variants: commit.variants.word().to_string(),
            sums,
        };
        // THE commit point: one durable rename. Before it, no live file has
        // been touched; after it, the generation is promised.
        durable_write(
            &cdir.join("COMMIT"),
            &serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?,
        )
    })();
    if let Err(e) = staged {
        // Unmarked stage: nothing was promised and nothing was applied — the
        // save simply failed, whole.
        let _ = std::fs::remove_dir_all(&cdir);
        return Err(e);
    }
    if let Err(e) = apply_commit_members(
        src,
        commit.recipe.as_deref().map_or(MemberRef::Keep, MemberRef::Write),
        (&commit.pixels).into(),
        (&commit.variants).into(),
    ) {
        // BEYOND the commit point: the generation is promised and `.commit`
        // stays — recovery completes the apply on the photo's next locked
        // touch, so the caller must not read this Err as "nothing happened".
        return Err(std::io::Error::new(
            e.kind(),
            format!("{e} — the save IS committed and completes on this photo's next open or save"),
        ));
    }
    // Consumed. A failure here only means the (idempotent) replay runs once
    // more on the next touch and consumes it then.
    let _ = std::fs::remove_dir_all(&cdir);
    settle_consumed_marker(&cdir);
    Ok(())
}

/// A borrowed [`CommitMember`], shared by the live apply and the recovery
/// replay (which owns its bytes read back from the stage).
#[derive(Clone, Copy)]
enum MemberRef<'a> {
    Write(&'a [u8]),
    Clear,
    Keep,
}

impl<'a> From<&'a CommitMember> for MemberRef<'a> {
    fn from(m: &'a CommitMember) -> Self {
        match m {
            CommitMember::Write(b) => MemberRef::Write(b),
            CommitMember::Clear => MemberRef::Clear,
            CommitMember::Keep => MemberRef::Keep,
        }
    }
}

/// Apply one committed generation onto the live triple. IDEMPOTENT —
/// replay-safe: a Write member whose live bytes already equal the staged
/// bytes is skipped, so a resumed apply neither re-retires the new
/// generation onto its own `.bak` (destroying the previous generation's
/// crash-recovery copy) nor repeats completed work.
fn apply_commit_members(
    src: &Path,
    recipe: MemberRef<'_>,
    pixels: MemberRef<'_>,
    variants: MemberRef<'_>,
) -> std::io::Result<()> {
    let dev = develop_dir(src);
    let apply = |live: PathBuf, bak: PathBuf, member: MemberRef<'_>| -> std::io::Result<()> {
        let rm = |p: PathBuf| match std::fs::remove_file(p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        match member {
            MemberRef::Write(bytes) => {
                if read_bytes_capped(&live, MAX_STORE_JSON).is_ok_and(|cur| cur == bytes) {
                    return Ok(()); // already applied — keep the retired previous generation
                }
                durable_retire_and_write(&live, &bak, bytes)
            }
            // The clear pair, `.bak` first (the clear_pixel_source rule: a
            // crash between the removals leaves the live record, never the
            // resurrection bait). RAW removal, not the public clear helpers
            // — a replay must not re-run publication gates against the
            // half-cleared state it exists to finish.
            MemberRef::Clear => {
                rm(bak)?;
                rm(live)
            }
            MemberRef::Keep => Ok(()),
        }
    };
    apply(recipe_target(src), dev.join("recipe.json.bak"), recipe)?;
    if matches!(recipe, MemberRef::Write(_)) {
        note_source(src); // the write_recipe breadcrumb rides the same member
    }
    apply(pixel_source_path(src), dev.join("pixels.json.bak"), pixels)?;
    apply(variants_path(src), dev.join("variants.json.bak"), variants)
}

/// Resolve a `.commit` stage left by a killed [`commit_develop`]. COMMIT
/// present with intact sums ⇒ replay the (idempotent) apply and consume the
/// stage; COMMIT absent ⇒ nothing was promised — discard the stage. COMMIT
/// present but unreadable, unknown, or sum-mismatched is `Err` and the
/// evidence STAYS: the marker was durably renamed in only after every staged
/// member was fsynced, so a mismatch means a spec-broken disk or tampering —
/// rolling back could freeze a half-applied generation as final, and rolling
/// forward would publish bytes nobody wrote. The error names the remedy.
fn resolve_pending_commit_unlocked(src: &Path) -> std::io::Result<()> {
    let cdir = commit_dir(src);
    if !cdir.exists() {
        return Ok(());
    }
    let refuse = |why: String| {
        std::io::Error::other(format!(
            "{} holds a crashed develop save that cannot be resolved ({why}) — delete that \
             directory to discard the crashed save and keep the photo's current develop",
            cdir.display()
        ))
    };
    let marker = cdir.join("COMMIT");
    let manifest_bytes = match read_bytes_capped(&marker, MAX_STORE_JSON) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Unmarked stage: the commit point was never reached, nothing
            // was applied — discarding is a true rollback.
            std::fs::remove_dir_all(&cdir)?;
            return Ok(());
        }
        Err(e) => return Err(refuse(format!("unreadable COMMIT marker: {e}"))),
    };
    let m: CommitManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| refuse(format!("corrupt COMMIT marker: {e}")))?;
    if m.v != 1 {
        return Err(refuse(format!("future commit format v{}", m.v)));
    }
    // The recipe member never clears through a commit — only `clear_develop`
    // removes recipe.json, through its own marker.
    if m.recipe == "clear" {
        return Err(refuse("a commit cannot clear the recipe".into()));
    }
    let member = |name: &str, action: &str| -> std::io::Result<Option<Vec<u8>>> {
        match action {
            "keep" | "clear" => Ok(None),
            "write" => {
                let staged = read_bytes_capped(&cdir.join(name), MAX_STORE_JSON)
                    .map_err(|e| refuse(format!("staged {name} unreadable: {e}")))?;
                match m.sums.get(name) {
                    Some(want) if *want == format!("{:016x}", fnv1a64(&staged)) => Ok(Some(staged)),
                    Some(_) => Err(refuse(format!("staged {name} does not match its recorded sum"))),
                    None => Err(refuse(format!("staged {name} has no recorded sum"))),
                }
            }
            other => Err(refuse(format!("unknown {name} action {other:?}"))),
        }
    };
    let recipe = member("recipe.json", &m.recipe)?;
    let pixels = member("pixels.json", &m.pixels)?;
    let variants = member("variants.json", &m.variants)?;
    let recipe_ref = recipe.as_deref().map_or(MemberRef::Keep, MemberRef::Write);
    let pixels_ref = match (m.pixels.as_str(), &pixels) {
        ("write", Some(b)) => MemberRef::Write(b),
        ("clear", _) => MemberRef::Clear,
        _ => MemberRef::Keep,
    };
    let variants_ref = match (m.variants.as_str(), &variants) {
        ("write", Some(b)) => MemberRef::Write(b),
        ("clear", _) => MemberRef::Clear,
        _ => MemberRef::Keep,
    };
    apply_commit_members(src, recipe_ref, pixels_ref, variants_ref)?;
    std::fs::remove_dir_all(&cdir)?;
    settle_consumed_marker(&cdir);
    Ok(())
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

/// The saved recipe's revision tag: the FNV-1a of recipe.json's bytes,
/// `"none"` when no file exists — absence is a REAL revision, two tabs
/// racing the FIRST save of a fresh photo must still collide — and `None`
/// when the file exists but cannot be read (untaggable; a conditional
/// writer refuses rather than overwrite what it cannot name). Content, not
/// mtime: stamp granularity is too coarse to gate a write.
pub fn recipe_revision(src: &Path) -> Option<String> {
    revision_of(&recipe_target(src))
}

fn revision_of(p: &Path) -> Option<String> {
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
    /// recipe-absent and neutral-recipe fallthroughs the open path takes.
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
        // recipe is absent or parses neutral. Same one-file precedence walk
        // as the recipe: only NotFound falls through, and an unreadable
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
fn settle_consumed_marker(path: &Path) {
    let _ = durable_os::finish_parent(path);
}

fn resolve_pending_clear_unlocked(src: &Path) -> std::io::Result<()> {
    if !clear_pending(src).exists() {
        return Ok(());
    }
    let (_, err) = clear_sweep(src);
    if let Some(e) = err {
        // The recover contract: Err is "state we cannot resolve" — the
        // backup gates refuse rather than overwrite it.
        return Err(e);
    }
    if mark_develop_cleared(src).is_ok() {
        let _ = std::fs::remove_file(clear_pending(src));
        settle_consumed_marker(&clear_pending(src));
    }
    Ok(())
}

fn recover_orphan_baks_unlocked(src: &Path) -> std::io::Result<()> {
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
    // env_or_dotenv (L16#3): a .env-set legacy root kept working when the
    // dotenv stopped writing the process environment.
    if let Some(o) = crate::config::env_or_dotenv("AUTOSHOP_LEGACY_OUT") {
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
/// The explicit-clear transaction marker (L03): written durably BEFORE
/// the sweep begins, consumed only after the cleared stamp lands. While it
/// exists the develop is "being removed": [`has_develop`] answers false,
/// the newest-intent ranking treats it like the cleared stamp, and
/// recovery completes the sweep on the next locked touch.
fn clear_pending(src: &Path) -> PathBuf {
    develop_dir(src).join("clear.pending")
}

pub fn has_develop(src: &Path) -> bool {
    // A PENDING explicit clear means "this develop is being removed" — the
    // surviving files are sweep leftovers, not a save (L03).
    if clear_pending(src).exists() {
        return false;
    }
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

/// [`has_develop`] PLUS the sidecar Lightroom itself writes beside the RAW
/// (L13#2). The badge/resume predicate: a photo edited only in Lightroom
/// showed no ● badge yet clicking it restored the Lightroom develop, and
/// the CLI batch resume filter spent a PAID analyze on it and then wrote
/// recipe.json over the user's Lightroom work. Existence-only and
/// deliberately cheap (called per photo per refresh) — reading and RANKING
/// the file is [`lightroom_sidecar`]'s job. Conservative by design: a
/// foreign or neutral sidecar counts as "a develop may exist", because a
/// false SKIP costs one manual analyze while a false ANALYZE costs money
/// and supersedes the user's work. A SIBLING of `has_develop`, never a
/// replacement: [`rank_lightroom_sidecar`] calls `has_develop` to mean "is
/// there a STORE develop to rank against" — folding the sidecar in there
/// would make `LrSidecar::Only` unreachable whenever a sidecar exists.
pub fn has_develop_or_sidecar(src: &Path) -> bool {
    // The pending-clear marker outranks the sidecar probe exactly as it
    // outranks the store files (L03): while the develop is being removed, a
    // projection the user once copied beside the RAW must not resurrect
    // the badge.
    if clear_pending(src).exists() {
        return false;
    }
    has_develop(src)
        || (crate::decode::is_raw(src) && src.with_extension("xmp").exists())
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
    /// A sidecar file IS beside the RAW but cannot be read, and this is why
    /// (see [`SidecarRead::Unreadable`]). Folding this into `None` opened the
    /// photo neutral in silence while the user's Lightroom work might sit
    /// right there (L08) — callers disclose it and fall back to the store.
    Unreadable(&'static str),
}

/// What a bounded sidecar read found. `Missing` and `Unreadable` are NOT the
/// same thing to a caller choosing a merge base: a missing file carries
/// nothing, while an unreadable one may carry the user's Lightroom work — and
/// collapsing both into one "absent" let the XMP writer fall back to
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

/// The bounded ceiling for the app's own JSON/text sidecars (recipe.json,
/// variants.json, pixels.json, version snapshots, settings). Far above any
/// legitimate file this app writes (recipe strings are clamp-capped; a real
/// variants.json is kilobytes), yet it stops a photo pack's 2 GB
/// "recipe.json" from materialising in RAM on open (L02).
pub const MAX_STORE_JSON: u64 = 16 * 1024 * 1024;

/// Bounded replacement for `std::fs::read_to_string` on files the app itself
/// persists but an untrusted photo pack can replace wholesale. Over the cap →
/// `InvalidData` naming the limit; `NotFound` passes through untouched, so
/// every caller's existing "unreadable ≠ absent" branching keeps its shape.
/// Bytes first, text second — same rationale as [`read_sidecar_checked`]:
/// `read_to_string` on a `Take` that cuts through a multi-byte character
/// reports the wrong reason.
pub fn read_text_capped(path: &Path, cap: u64) -> std::io::Result<String> {
    let buf = read_bytes_capped(path, cap)?;
    String::from_utf8(buf).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not readable UTF-8 text", path.display()),
        )
    })
}

/// [`read_text_capped`] for binary payloads (`variants.json`/`pixels.json`
/// parse from slices).
pub fn read_bytes_capped(path: &Path, cap: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    let n = f.take(cap + 1).read_to_end(&mut buf)?;
    if n as u64 > cap {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is larger than the {cap}-byte limit", path.display()),
        ));
    }
    Ok(buf)
}

/// Every read of an XMP sidecar, bounded. A sidecar is metadata a user
/// RECEIVES — from Lightroom, from a shared shoot, from a stranger's delivery —
/// so its size is not ours to trust: a plain `read_to_string` on a 2 GB file
/// named `DSC0001.xmp` materialises 2 GB in a request thread just to be handed
/// to a scanner. Real ones are kilobytes; the biggest Lightroom masks documents
/// are single-digit megabytes. Over the cap the content is refused — restore
/// callers treat that as "no develop", and the XMP-writing path DISCLOSES it
/// (see [`crate::pipeline::write_xmp`]) instead of silently merging
/// against a different base.
///
/// `Read::take` rather than a `metadata()` size check: the length is bounded by
/// what was actually read, so a file that grows between the two syscalls cannot
/// widen the allocation.
/// A sidecar's identity as its open HANDLE reports it: (mtime, length). A
/// stage+rename publish re-points the PATH at a different file, but never the
/// handle — so this is the identity the content actually read is bound to,
/// and the only mtime a newest-intent ranking of that content may use.
pub type SidecarStamp = (std::time::SystemTime, u64);

fn handle_stamp(f: &std::fs::File) -> Option<SidecarStamp> {
    f.metadata().ok().and_then(|m| m.modified().ok().map(|t| (t, m.len())))
}

pub fn read_sidecar_checked(path: &Path) -> SidecarRead {
    read_sidecar_stamped(path).0
}

/// [`read_sidecar_checked`] plus the handle identity of what was read. The
/// stamp is fstat'd from the SAME handle before AND after the read: a writer
/// mutating the file in place mid-read used to hand back silently torn text —
/// that now reports as unreadable with the true reason, and the stamp is
/// returned only when both fstats agree. (A same-length in-place rewrite
/// inside one mtime tick of a coarse-granularity filesystem can still slip
/// through; the develop lock covers the app's own writers — this guard is
/// for foreign ones, best-effort by nature.)
pub fn read_sidecar_stamped(path: &Path) -> (SidecarRead, Option<SidecarStamp>) {
    use std::io::Read as _;
    const MAX_SIDECAR: u64 = 16 * 1024 * 1024;
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (SidecarRead::Missing, None),
        Err(_) => return (SidecarRead::Unreadable("it could not be opened"), None),
    };
    let before = handle_stamp(&f);
    // BYTES first, text second. `read_to_string` on a `Take` that cuts through
    // a multi-byte character fails with InvalidData BEFORE any size check can
    // run, so an over-cap sidecar carrying CJK captions or typographic quotes
    // was reported to the user as "not readable UTF-8 text" — a false reason,
    // in a note this round exists to make truthful.
    let mut buf = Vec::new();
    let read = match (&f).take(MAX_SIDECAR + 1).read_to_end(&mut buf) {
        Err(_) => SidecarRead::Unreadable("it could not be read"),
        Ok(n) if n as u64 > MAX_SIDECAR => {
            SidecarRead::Unreadable("it is larger than the 16 MiB sidecar limit")
        }
        Ok(_) => match String::from_utf8(buf) {
            Ok(s) => SidecarRead::Ok(s),
            Err(_) => SidecarRead::Unreadable("it is not readable UTF-8 text"),
        },
    };
    let after = handle_stamp(&f);
    match (before, after) {
        (Some(b), Some(a)) if b == a => (read, Some(a)),
        (Some(_), Some(_)) => {
            (SidecarRead::Unreadable("it changed while it was being read"), None)
        }
        // No handle identity available (exotic filesystem): the text is
        // still the text — callers just get no stamp to verify against.
        _ => (read, None),
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

/// The RAW's embedded XMP packet AS A RESTORE SOURCE — strictly the
/// LOWEST-priority answer (a develop Lightroom baked INTO a DNG must not
/// outrank anything the user did since), and gated by the explicit-clear
/// markers: unlike every other source, the packet lives inside a file this
/// app never writes, so `clear_develop` cannot delete it — without the gate,
/// Reset+Save would resurrect the baked develop on the very next open (the
/// exact bug [`lightroom_sidecar`]'s cleared-marker contest was built to
/// close). Known, accepted consequence of "strictly lowest": once any store
/// file exists the packet is never consulted again — re-baking the DNG in
/// Lightroom does not win (that would need an mtime rank a file we never
/// write cannot express).
///
/// `Ok(None)` = no packet, or none a restore may consult. `Err(reason)` = a
/// packet that EXISTS but cannot be read — disclosed by every caller, never
/// folded into absence.
pub fn embedded_packet_for_restore(src: &Path) -> Result<Option<String>, String> {
    if !crate::decode::is_raw(src) {
        return Ok(None);
    }
    if develop_dir(src).join("cleared.txt").exists() || clear_pending(src).exists() {
        return Ok(None);
    }
    crate::decode::embedded_xmp(src).map_err(|e| e.to_string())
}

pub fn lightroom_sidecar(src: &Path) -> LrSidecar {
    // Only camera RAWs have a Lightroom-sidecar convention; a baked
    // PNG/TIFF's neighbouring .xmp (if any) is not ours to interpret.
    if !crate::decode::is_raw(src) {
        return LrSidecar::None;
    }
    let lr = src.with_extension("xmp");
    let (read, stamp) = read_sidecar_stamped(&lr);
    let text = match read {
        SidecarRead::Ok(t) => t,
        SidecarRead::Missing => return LrSidecar::None,
        // A file IS there but cannot answer — never fold that into "absent".
        SidecarRead::Unreadable(why) => return LrSidecar::Unreadable(why),
    };
    rank_lightroom_sidecar(src, &lr, text, stamp)
}

/// The ranking half of [`lightroom_sidecar`], split so a test can hand it a
/// deliberately mismatched identity. `stamp` is the HANDLE identity of the
/// text actually read; the mtime the newest-intent contest uses comes from it
/// — never from a fresh path stat, which after a swap describes a DIFFERENT
/// file than the text in hand (generation-N text ranked under
/// generation-N+1's mtime resurrected stale edits as "newer than the store",
/// and the mirror swap silently discarded fresh Lightroom work as older).
fn rank_lightroom_sidecar(
    src: &Path,
    lr: &Path,
    text: String,
    stamp: Option<SidecarStamp>,
) -> LrSidecar {
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
    // A PENDING clear is newest intent too (L03): between the marker and
    // the completed sweep, a projection beside the RAW must not out-rank
    // the clear and resurrect what is being removed.
    let cleared_t = [develop_dir(src).join("cleared.txt"), clear_pending(src)]
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max();
    let lr_t = match stamp {
        Some(s) => {
            // ONE re-verification after the read: the path must still
            // resolve to the very file the handle read. A swap — or an
            // unlink — after the read means the text in hand no longer
            // describes what sits beside the photo: disclosed, never ranked.
            let now = std::fs::metadata(lr)
                .ok()
                .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
            if now != Some(s) {
                return LrSidecar::Unreadable("it was replaced while it was being read");
            }
            Some(s.0)
        }
        // No handle identity was available: the old path stat is all there
        // is — strictly no worse than before this guard existed.
        None => std::fs::metadata(lr).and_then(|m| m.modified()).ok(),
    };
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
    // TRANSACTION MARKER FIRST (L03): a kill mid-sweep used to leave a
    // half-cleared develop — recipe gone, XMP or variants alive — that
    // every reader took for a real partial develop, resurrecting edits the
    // user explicitly cleared (and recover_orphan_baks republished the very
    // .baks the clear was deleting). The marker records the intent durably;
    // recovery completes the sweep on the next locked touch.
    durable_write(&clear_pending(src), b"develop clear in progress\n")?;
    let (removed, first_err) = clear_sweep(src);
    if let Some(e) = first_err {
        // The marker STAYS — recovery retries the sweep.
        return Err(e);
    }
    let marker_warning = mark_develop_cleared(src).err().map(|e| e.to_string());
    if marker_warning.is_none() {
        // Only after the durable cleared stamp exists — between the two
        // writes BOTH markers exist, so the clear's intent is never
        // invisible. A failed stamp keeps the pending marker: recovery
        // retries it, and the ranking treats the marker itself as newest
        // intent meanwhile.
        let _ = std::fs::remove_file(clear_pending(src));
        settle_consumed_marker(&clear_pending(src));
    }
    Ok(ClearOutcome { removed, marker_warning })
}

/// The sweep half of [`clear_develop_unlocked`], IDEMPOTENT so recovery may
/// repeat it: legacy tombstone, the central files (retired `.bak`s
/// included) and the pixel-source pair. Returns what it removed and the
/// first failure (later removals still run — retrying costs nothing and a
/// partial sweep is smaller than the one it retries).
fn clear_sweep(src: &Path) -> (bool, Option<std::io::Error>) {
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
    // The tombstone lands BEFORE central files are removed. A crash can
    // therefore leave the old central develop visible, or leave it cleared
    // with legacy already suppressed, but cannot expose the ambiguous
    // fallback in between.
    if let Err(e) = suppress_legacy(src) {
        return (removed, Some(e));
    }
    removed |= legacy_was_visible;

    // A pending single-generation commit dies with the clear: the clear is
    // the newer intent (commit_develop completes a pending clear before it
    // stages), and a surviving `.commit` would REPLAY the very develop this
    // sweep removes.
    match std::fs::remove_dir_all(commit_dir(src)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            first_err.get_or_insert(e);
        }
    }

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
/// True for UNC / verbatim-UNC / device-namespace prefixes — the classes
/// whose mere `exists()` probe leaves this machine (an SMB touch sends
/// NetNTLM credentials). Checked LEXICALLY, before any filesystem call, for
/// exactly that reason. Local drive-letter absolutes stay honoured: the
/// pixel-source writer legitimately records them for masters outside the
/// develop dir.
fn remote_or_device_path(p: &Path) -> bool {
    use std::path::{Component, Prefix};
    match p.components().next() {
        Some(Component::Prefix(pre)) => matches!(
            pre.kind(),
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(_)
        ),
        _ => false,
    }
}

pub fn resolve_mask_paths(r: &mut EditRecipe, base: &Path) {
    for m in &mut r.masks {
        for path in m.bitmap_paths_mut() {
            let p = Path::new(path.as_str());
            if remote_or_device_path(p) {
                // A develop store is not trusted to reach OFF this machine:
                // a crafted `\\attacker\share\mask.png` turned "open the
                // photo" into an outbound SMB authentication.
                eprintln!(
                    "⚠ bitmap mask reference {path:?} names a network/device path — it is disabled"
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
            if !crate::xmp::xmp_to_recipe(&t).is_noop() =>
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
    // Complete any KILLED delete before claiming (L03): a half-deleted
    // version whose recipe already fell is invisible to list_versions, so
    // max+1 would RECYCLE its number — and the surviving marker would then
    // have recovery delete the brand-new snapshot. Best-effort: a number
    // whose marker cannot be cleared is skipped below, never claimed.
    let _ = recover_pending_version_deletes_unlocked(src);
    let mut last = list_versions(src).last().copied();
    loop {
        if last == Some(u32::MAX) {
            return Err(std::io::Error::other(
                "version namespace exhausted (a v4294967295 snapshot exists)",
            ));
        }
        let n = last.unwrap_or(0).saturating_add(1);
        let dst = version_target(src, n);
        if deleting_marker(src, n).exists() {
            // A marker that survived a failed resume owns this number.
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

/// The version-delete transaction marker (L03). A leading dot keeps it
/// outside the `v<N>.` namespace the sweep removes and `list_versions`
/// parses; while it exists the version is "being removed" — unlisted, its
/// number unclaimable — and recovery resumes the sweep.
fn deleting_marker(src: &Path, n: u32) -> PathBuf {
    develop_dir(src).join(format!(".deleting.v{n}"))
}

fn delete_version_unlocked(src: &Path, n: u32) -> std::io::Result<()> {
    // TRANSACTION MARKER FIRST (L03): a kill mid-sweep used to leave a
    // half-version — recipe alive with rasters gone (listed, loadable,
    // rendering dead masks the next save persists as dangling paths) or
    // rasters alive with the recipe gone (orphan blobs forever, and the
    // number silently recyclable). The marker records the intent durably;
    // the sweep resumes at the next claim or locked recovery touch.
    durable_write(&deleting_marker(src, n), format!("deleting v{n}\n").as_bytes())?;
    sweep_version_unlocked(src, n, true)?;
    let _ = std::fs::remove_file(deleting_marker(src, n));
    settle_consumed_marker(&deleting_marker(src, n));
    Ok(())
}

/// The sweep half of [`delete_version_unlocked`]: rasters first, recipe
/// last. `must_exist` keeps a fresh delete's NotFound error for a
/// stale-list 🗑 while a RESUME tolerates the recipe already being gone.
fn sweep_version_unlocked(src: &Path, n: u32, must_exist: bool) -> std::io::Result<()> {
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
    match std::fs::remove_file(version_target(src, n)) {
        Ok(()) => Ok(()),
        Err(e) if !must_exist && e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Resume every KILLED version delete whose marker survives. Best-effort
/// per version — a locked raster keeps its marker (the version stays
/// unlisted and its number unclaimable) and the next touch retries.
fn recover_pending_version_deletes_unlocked(src: &Path) -> std::io::Result<()> {
    let mut pending = Vec::new();
    match std::fs::read_dir(develop_dir(src)) {
        Ok(dir) => {
            for e in dir.flatten() {
                if let Some(n) = e
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix(".deleting.v"))
                    .and_then(|rest| rest.parse::<u32>().ok())
                {
                    pending.push(n);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    let mut failure: Option<std::io::Error> = None;
    for n in pending {
        match sweep_version_unlocked(src, n, false) {
            Ok(()) => {
                let _ = std::fs::remove_file(deleting_marker(src, n));
                settle_consumed_marker(&deleting_marker(src, n));
            }
            Err(e) => {
                failure.get_or_insert(e);
            }
        }
    }
    match failure {
        None => Ok(()),
        Some(e) => Err(e),
    }
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

/// Durability tail for a staged publish performed OUTSIDE this module (the
/// settings file, the style index): fsync the staged bytes, rename them over
/// the live name, fsync the parent directory. tmp+rename ALONE leaves a
/// post-crash window where the live name points at bytes the disk never
/// received (L03) — and modes are untouched, so a 0600-claimed staging
/// carries its mode to the live name.
pub(crate) fn durable_replace(staged: &Path, live: &Path) -> std::io::Result<()> {
    sync_staged(staged)?;
    durable_os::replace(staged, live)?;
    durable_os::finish_parent(live)
}

/// Durably MOVE an existing file to a new name. The bytes are already
/// whatever the disk holds — nothing was staged — so only the directory
/// entry needs to survive. NOT [`durable_replace`]: that fsyncs its source
/// through a WRITE handle, which a read-only or exclusively-held file
/// refuses — turning a move that used to succeed into a failure (L03).
pub(crate) fn durable_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    durable_os::replace(from, to)?;
    durable_os::finish_parent(to)
}

/// Make an already-written payload durable where it stands: fsync its
/// bytes and its parent directory. For binary payloads (mask rasters,
/// sidecar masks) written straight onto their claimed name and about to be
/// REFERENCED by a durably-committed JSON — the JSON's own fsync is
/// meaningless if the payload it names can still vanish with the page
/// cache (L03). `pub`: the GUI binary's mask writers consume it.
pub fn durable_adopt(path: &Path) -> std::io::Result<()> {
    sync_staged(path)?;
    durable_os::finish_parent(path)
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
    // AFTER the recovery, never before the lock (L03): every explicit clear
    // writes the tombstone, so the suppressed early-return used to skip the
    // recovery hook for exactly the photos a killed clear leaves behind —
    // and the web's api_recipe reaches recovery only through this call.
    if legacy_suppressed(src) {
        return false;
    }
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
    let Ok(text) = read_text_capped(from, MAX_STORE_JSON) else { return false };
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

    /// L03: a killed version delete hides the half-version from the list,
    /// resumes at the next claim, and releases the number only once the
    /// sweep truly finished — so a resumed sweep can never eat a fresh
    /// snapshot that took a recycled number.
    #[test]
    fn a_killed_version_delete_resumes_before_its_number_is_reused() {
        let dir = std::env::temp_dir().join("autoshop-store-test-vdel-marker");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_vdel.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();

        // v1 complete; v2 half-deleted: the marker landed, the recipe and
        // one frozen raster still on disk (killed before the sweep ran).
        std::fs::write(version_target(&raw, 1), b"{}").unwrap();
        std::fs::write(version_target(&raw, 2), b"{}").unwrap();
        std::fs::write(dev.join("v2.mask-sky.png"), b"raster").unwrap();
        std::fs::write(dev.join(".deleting.v2"), b"deleting v2\n").unwrap();

        assert_eq!(list_versions(&raw), vec![1], "a half-deleted version is not listed");

        let (n, claimed) = claim_version(&raw).unwrap();
        assert!(!dev.join("v2.mask-sky.png").exists(), "the claim resumed the sweep first");
        assert!(!dev.join(".deleting.v2").exists(), "the finished sweep consumed its marker");
        assert_eq!(
            n, 2,
            "a number is recycled only AFTER its delete truly finished — with the marker gone that is safe"
        );
        assert!(claimed.exists(), "the claim file holds the recycled slot");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: a killed clear completes on the next locked touch instead of
    /// leaving a half-cleared develop whose surviving XMP/variants would
    /// resurrect the cleared edits — and the .bak republish never undoes
    /// the sweep (the pending marker is checked first).
    #[test]
    fn a_pending_clear_completes_on_the_next_touch() {
        let dir = std::env::temp_dir().join("autoshop-store-test-pending-clear");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_pending_clear.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();

        // The kill point: the marker landed, the recipe fell, everything
        // else survived — including a .bak recovery would republish.
        std::fs::write(dev.join("recipe.json.bak"), b"{}").unwrap();
        std::fs::write(xmp_target(&raw), b"<x:xmpmeta/>").unwrap();
        std::fs::write(variants_path(&raw), b"{}").unwrap();
        std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();
        assert!(!has_develop(&raw), "a pending clear is not a develop");

        recover_orphan_baks(&raw).unwrap();

        assert!(!recipe_target(&raw).exists());
        assert!(!dev.join("recipe.json.bak").exists(), "the .bak fell with the sweep");
        assert!(!xmp_target(&raw).exists());
        assert!(!variants_path(&raw).exists());
        assert!(dev.join("cleared.txt").exists(), "the clear finished with its stamp");
        assert!(!dev.join("clear.pending").exists(), "the transaction closed");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// One temp-dir photo + develop-dir fixture for the commit tests, with a
    /// non-empty master raster and helpers to build staged generations.
    fn commit_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("autoshop-store-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join(format!("_store_{}.arw", tag.replace('-', "_")));
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        (dir, raw, dev)
    }

    fn commit_sum(bytes: &[u8]) -> String {
        format!("{:016x}", fnv1a64(bytes))
    }

    fn strip_record(kind: &str) -> VariantsRecord {
        VariantsRecord {
            v: 1,
            active_kind: kind.to_string(),
            active_pos: 0,
            others: Vec::new(),
        }
    }

    /// L03: the kill point IMMEDIATELY after the COMMIT marker's rename — all
    /// three live files still hold the previous generation. The next locked
    /// touch replays the whole staged generation and consumes the stage: a
    /// marked commit is a promise, not a suggestion.
    #[test]
    fn a_committed_generation_replays_over_a_stale_triple() {
        let (dir, raw, dev) = commit_fixture("commit-replay");
        let m1 = dev.join("m1.png");
        let m2 = dev.join("m2.png");
        std::fs::write(&m1, b"px-one").unwrap();
        std::fs::write(&m2, b"px-two").unwrap();
        std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
        std::fs::write(
            pixel_source_path(&raw),
            pixel_source_record_bytes(&raw, &m1, false).unwrap(),
        )
        .unwrap();
        std::fs::write(
            variants_path(&raw),
            variants_record_bytes(&raw, &strip_record("fitted")).unwrap(),
        )
        .unwrap();

        let cdir = dev.join(".commit");
        std::fs::create_dir_all(&cdir).unwrap();
        let r2 = b"gen2-recipe".to_vec();
        let p2 = pixel_source_record_bytes(&raw, &m2, false).unwrap();
        let v2 = variants_record_bytes(&raw, &strip_record("generated")).unwrap();
        std::fs::write(cdir.join("recipe.json"), &r2).unwrap();
        std::fs::write(cdir.join("pixels.json"), &p2).unwrap();
        std::fs::write(cdir.join("variants.json"), &v2).unwrap();
        let manifest = serde_json::json!({
            "v": 1, "recipe": "write", "pixels": "write", "variants": "write",
            "sums": {
                "recipe.json": commit_sum(&r2),
                "pixels.json": commit_sum(&p2),
                "variants.json": commit_sum(&v2),
            },
        });
        std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

        recover_orphan_baks(&raw).unwrap();

        assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), r2, "recipe rolled forward");
        assert_eq!(
            std::fs::read(dev.join("recipe.json.bak")).unwrap(),
            b"gen1-recipe",
            "the previous generation retired to its .bak"
        );
        let (master, generated) = read_pixel_source(&raw).expect("gen2 master restored");
        assert_eq!(master.file_name().unwrap().to_str().unwrap(), "m2.png");
        assert!(!generated);
        match read_variants_checked(&raw) {
            VariantsRead::Strip(rec) => assert_eq!(rec.active_kind, "generated"),
            _ => panic!("gen2 strip must be readable"),
        }
        assert!(!cdir.exists(), "the stage is consumed");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: the kill point MID-apply — recipe already published, pixels and
    /// strip still stale. The replay converges the remaining members and
    /// SKIPS the already-applied one, so the previous generation's retired
    /// `.bak` is not destroyed by a re-retire of the new bytes.
    #[test]
    fn a_half_applied_commit_finishes_instead_of_tearing() {
        let (dir, raw, dev) = commit_fixture("commit-half");
        let m1 = dev.join("m1.png");
        std::fs::write(&m1, b"px-one").unwrap();
        let r2 = b"gen2-recipe".to_vec();
        // Recipe member already applied: live = staged bytes, .bak = gen1.
        std::fs::write(recipe_target(&raw), &r2).unwrap();
        std::fs::write(dev.join("recipe.json.bak"), b"gen1-recipe").unwrap();
        std::fs::write(
            pixel_source_path(&raw),
            pixel_source_record_bytes(&raw, &m1, false).unwrap(),
        )
        .unwrap();

        let cdir = dev.join(".commit");
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("recipe.json"), &r2).unwrap();
        let manifest = serde_json::json!({
            "v": 1, "recipe": "write", "pixels": "clear", "variants": "keep",
            "sums": { "recipe.json": commit_sum(&r2) },
        });
        std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

        recover_orphan_baks(&raw).unwrap();

        assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), r2);
        assert_eq!(
            std::fs::read(dev.join("recipe.json.bak")).unwrap(),
            b"gen1-recipe",
            "the idempotent replay must not re-retire the new bytes over gen1's .bak"
        );
        assert!(!pixel_source_path(&raw).exists(), "the pixels clear finished");
        assert!(!cdir.exists(), "the stage is consumed");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: a stage the crash caught BEFORE the marker is no commitment at
    /// all — it rolls back wholesale and the live generation stands.
    #[test]
    fn an_unmarked_stage_rolls_back() {
        let (dir, raw, dev) = commit_fixture("commit-unmarked");
        std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
        let cdir = dev.join(".commit");
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("recipe.json"), b"gen2-recipe").unwrap();

        recover_orphan_baks(&raw).unwrap();

        assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen1-recipe");
        assert!(!cdir.exists(), "the unmarked stage is discarded");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: a COMMIT whose staged bytes fail their recorded sum is a state
    /// this store cannot legally reach (members are fsynced BEFORE the marker
    /// rename) — so it refuses loudly and KEEPS the evidence: rolling back
    /// could freeze a half-applied generation as final, rolling forward would
    /// publish bytes nobody wrote.
    #[test]
    fn a_corrupt_commit_refuses_loudly_and_keeps_the_evidence() {
        let (dir, raw, dev) = commit_fixture("commit-corrupt");
        std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
        let cdir = dev.join(".commit");
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("recipe.json"), b"gen2-recipe").unwrap();
        let manifest = serde_json::json!({
            "v": 1, "recipe": "write", "pixels": "keep", "variants": "keep",
            "sums": { "recipe.json": commit_sum(b"different bytes entirely") },
        });
        std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

        let err = recover_orphan_baks(&raw).expect_err("a sum mismatch must refuse");
        assert!(
            err.to_string().contains("delete that directory"),
            "the refusal names the remedy: {err}"
        );
        assert_eq!(
            std::fs::read(recipe_target(&raw)).unwrap(),
            b"gen1-recipe",
            "nothing was published from the suspect stage"
        );
        assert!(cdir.exists(), "the evidence stays for the user to inspect");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: a committed CLEAR removes the live member AND its retired `.bak`
    /// — the generation-blind pair sweep must not republish (resurrect) a
    /// member the committed generation deletes.
    #[test]
    fn a_committed_clear_takes_the_retired_bak_with_it() {
        let (dir, raw, dev) = commit_fixture("commit-clear-bak");
        let m1 = dev.join("m1.png");
        std::fs::write(&m1, b"px-one").unwrap();
        std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
        // The resurrection bait: live pixels.json already fell, its .bak
        // survives — exactly the state the pair sweep exists to republish.
        std::fs::write(
            dev.join("pixels.json.bak"),
            pixel_source_record_bytes(&raw, &m1, false).unwrap(),
        )
        .unwrap();
        let cdir = dev.join(".commit");
        std::fs::create_dir_all(&cdir).unwrap();
        let manifest = serde_json::json!({
            "v": 1, "recipe": "keep", "pixels": "clear", "variants": "keep",
            "sums": {},
        });
        std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();

        recover_orphan_baks(&raw).unwrap();

        assert!(!pixel_source_path(&raw).exists(), "the cleared member stays cleared");
        assert!(!dev.join("pixels.json.bak").exists(), "no resurrection bait survives");
        assert!(!has_pixel_source(&raw));
        assert!(!cdir.exists());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: a pending explicit CLEAR is the newer intent by construction
    /// (commit_develop completes one before it stages), so recovery lets the
    /// clear take the crashed commit with it — never replays a develop the
    /// user asked to remove.
    #[test]
    fn a_cleared_develop_takes_its_pending_commit_with_it() {
        let (dir, raw, dev) = commit_fixture("commit-vs-clear");
        std::fs::write(recipe_target(&raw), b"gen1-recipe").unwrap();
        let cdir = dev.join(".commit");
        std::fs::create_dir_all(&cdir).unwrap();
        let r2 = b"gen2-recipe".to_vec();
        std::fs::write(cdir.join("recipe.json"), &r2).unwrap();
        let manifest = serde_json::json!({
            "v": 1, "recipe": "write", "pixels": "keep", "variants": "keep",
            "sums": { "recipe.json": commit_sum(&r2) },
        });
        std::fs::write(cdir.join("COMMIT"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();

        recover_orphan_baks(&raw).unwrap();

        assert!(!recipe_target(&raw).exists(), "the clear won — no replay");
        assert!(!cdir.exists(), "the crashed commit died with the clear");
        assert!(dev.join("cleared.txt").exists());
        assert!(!dev.join("clear.pending").exists());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// The public saver end to end: a full triple lands as one generation
    /// with no staging residue, a second generation writes + clears + keeps
    /// per member, and an unresolved strip refuses the WHOLE save before
    /// anything stages (the all-or-nothing face).
    #[test]
    fn a_develop_commit_lands_all_three_or_nothing() {
        let (dir, raw, dev) = commit_fixture("commit-public");
        let m1 = dev.join("m1.png");
        std::fs::write(&m1, b"px-one").unwrap();

        commit_develop(
            &raw,
            DevelopCommit {
                recipe: Some(b"gen1-recipe".to_vec()),
                pixels: CommitMember::Write(pixel_source_record_bytes(&raw, &m1, true).unwrap()),
                variants: CommitMember::Write(
                    variants_record_bytes(&raw, &strip_record("fitted")).unwrap(),
                ),
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen1-recipe");
        let (master, generated) = read_pixel_source(&raw).expect("master linked");
        assert_eq!(master.file_name().unwrap().to_str().unwrap(), "m1.png");
        assert!(generated);
        assert!(matches!(read_variants_checked(&raw), VariantsRead::Strip(_)));
        assert!(!dev.join(".commit").exists(), "the stage is consumed");

        commit_develop(
            &raw,
            DevelopCommit {
                recipe: Some(b"gen2-recipe".to_vec()),
                pixels: CommitMember::Clear,
                variants: CommitMember::Keep,
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(recipe_target(&raw)).unwrap(), b"gen2-recipe");
        assert!(!has_pixel_source(&raw), "cleared, retired .bak included");
        assert!(matches!(read_variants_checked(&raw), VariantsRead::Strip(_)), "Keep kept it");

        // No `.tmp.` staging litter anywhere in the develop dir.
        for e in std::fs::read_dir(&dev).unwrap().flatten() {
            let name = e.file_name();
            assert!(
                !name.to_string_lossy().contains(".tmp."),
                "staging residue: {name:?}"
            );
        }

        // The all-or-nothing face: an unresolved strip refuses the save
        // BEFORE anything stages — the recipe member does not land either.
        std::fs::write(variants_path(&raw), b"not json at all").unwrap();
        let err = commit_develop(
            &raw,
            DevelopCommit {
                recipe: Some(b"gen3-recipe".to_vec()),
                pixels: CommitMember::Keep,
                variants: CommitMember::Clear,
            },
        )
        .expect_err("an unresolved strip refuses the whole save");
        assert!(err.to_string().contains("cannot be honoured"), "{err}");
        assert_eq!(
            std::fs::read(recipe_target(&raw)).unwrap(),
            b"gen2-recipe",
            "all-or-nothing: the refused save left no member behind"
        );
        assert!(!dev.join(".commit").exists(), "nothing staged");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: between the marker and the completed sweep, a projection copied
    /// beside the RAW must not out-rank the clear and resurrect what is
    /// being removed — the pending marker ranks like the cleared stamp.
    #[test]
    fn a_pending_clear_outranks_the_sidecar_beside_the_photo() {
        use std::time::{Duration, SystemTime};
        let dir = std::env::temp_dir().join("autoshop-store-test-pending-rank");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_pending_rank.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let lr = raw.with_extension("xmp");
        std::fs::write(
            &lr,
            crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 30.0, ..Default::default() }),
        )
        .unwrap();
        std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();
        // The sidecar is OLDER than the pending clear → the clear wins.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(7200))
            .unwrap();
        assert!(
            matches!(lightroom_sidecar(&raw), LrSidecar::None),
            "a pending clear must outrank the older sidecar"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L03: a 0-byte file at the recorded master path is the crash-
    /// between-claim-and-publish state — the reader refuses it with the
    /// cause while the record itself still counts (callers refuse
    /// deliverables instead of silently rendering the un-retouched source).
    #[test]
    fn an_empty_master_claim_is_refused_not_restored() {
        let dir = std::env::temp_dir().join("autoshop-store-test-empty-master");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_empty_master.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let master = dev.join("retouch-master.png");
        std::fs::write(&master, b"").unwrap();
        write_pixel_source(&raw, &master, false).unwrap();

        assert!(read_pixel_source(&raw).is_none(), "an empty claim is not a master");
        assert!(
            has_pixel_source(&raw),
            "the record still exists — deliverable callers refuse, not degrade"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L01: the revision tag names the BYTES. Absence is a real tag; a
    /// byte-identical republish keeps its tag (it must never 412); different
    /// bytes change it. (The untaggable arm — an existing file the bounded
    /// reader refuses — is a two-line error map covered by
    /// read_text_capped_enforces_its_limit + the serve-side gate test.)
    #[test]
    fn a_recipe_revision_names_the_bytes_and_absence_is_a_real_tag() {
        let dir = std::env::temp_dir().join("autoshop-store-test-revision");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("recipe.json");

        assert_eq!(revision_of(&p).as_deref(), Some("none"), "absence is a real tag");
        std::fs::write(&p, b"{\"contrast\":1.0}").unwrap();
        let a = revision_of(&p).expect("readable bytes must tag");
        assert_ne!(a, "none");
        std::fs::write(&p, b"{\"contrast\":1.0}").unwrap();
        assert_eq!(revision_of(&p).as_deref(), Some(a.as_str()), "same bytes, same tag");
        std::fs::write(&p, b"{\"contrast\":2.0}").unwrap();
        assert_ne!(revision_of(&p).expect("readable"), a, "changed bytes change the tag");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L01: the snapshot's .bak recovery PRECEDES recipe selection, and
    /// recipe + pixel source come from one lock acquisition. A crashed
    /// publish leaves the recipe only in recipe.json.bak — the old batch
    /// order (exists → read, recovery buried in the later pixel read)
    /// exported a neutral develop while opening the same photo showed the
    /// recovered one.
    #[test]
    fn a_develop_snapshot_recovers_the_bak_before_choosing_a_recipe() {
        let dir = std::env::temp_dir().join("autoshop-store-test-snapshot-bak");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_snapshot_probe.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();

        // A crashed publish: the recipe lives ONLY in the .bak.
        let saved =
            serde_json::to_string(&EditRecipe { contrast: 17.0, ..Default::default() })
                .unwrap();
        std::fs::write(dev.join("recipe.json.bak"), &saved).unwrap();

        let snap = read_develop_snapshot(&raw).unwrap();
        let (text, from) = snap.recipe.expect("the .bak must be recovered, not skipped");
        assert_eq!(text, saved, "the recovered recipe is the crashed publish's bytes");
        assert_eq!(from, recipe_target(&raw), "recovered into the central slot");
        assert!(snap.recipe_err.is_none(), "a recovered read is not an error");
        assert!(!snap.pixel_recorded, "no pixel link exists in this fixture");
        assert!(snap.pixel_source.is_none());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
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

    /// L08: an unreadable sidecar beside the RAW is DISCLOSED, not folded
    /// into "no sidecar" — the old fold opened the photo neutral in silence.
    #[test]
    fn a_sidecar_swapped_after_the_read_is_disclosed_not_ranked() {
        // L01: the newest-intent contest may only rank the text it actually
        // read. An identity that cannot match what is on disk models a
        // sidecar swapped right after the read.
        let dir = std::env::temp_dir().join(format!(
            "autoshop-sidecar-swap-{}-{}",
            std::process::id(),
            next_tmp_seq()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("SWAP.ARW");
        std::fs::write(&raw, b"raw").unwrap();
        let lr = raw.with_extension("xmp");
        std::fs::write(&lr, b"<x:xmpmeta/>").unwrap();

        let swapped = rank_lightroom_sidecar(
            &raw,
            &lr,
            "<x:xmpmeta/>".to_string(),
            // No real file carries the epoch mtime with a u64::MAX length.
            Some((std::time::SystemTime::UNIX_EPOCH, u64::MAX)),
        );
        assert!(
            matches!(swapped, LrSidecar::Unreadable(why) if why.contains("replaced")),
            "an impossible identity must disclose the swap"
        );

        let real = std::fs::File::open(&lr)
            .ok()
            .and_then(|f| f.metadata().ok())
            .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
        assert!(real.is_some(), "the fixture filesystem must report mtimes");
        let ranked = rank_lightroom_sidecar(&raw, &lr, "<x:xmpmeta/>".to_string(), real);
        assert!(
            matches!(ranked, LrSidecar::Only(_)),
            "the true identity must rank exactly as before this guard"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lightroom_sidecar_unreadable_is_disclosed_not_absent() {
        let dir = std::env::temp_dir().join("autoshop-store-test-lr-unreadable");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_probe.arw"); // never read — only its neighbour is
        std::fs::write(dir.join("_probe.xmp"), [0xFFu8, 0xFE, 0xC0, 0x00]).unwrap();
        let LrSidecar::Unreadable(why) = lightroom_sidecar(&raw) else {
            panic!("an unreadable sidecar must be disclosed, not treated as absent");
        };
        assert!(why.contains("UTF-8"), "the reason names the cause: {why}");
    }

    /// L02: the bounded reader — over the cap is InvalidData naming the
    /// limit, at the cap passes, NotFound passes through untouched.
    #[test]
    fn read_text_capped_enforces_its_limit() {
        let dir = std::env::temp_dir().join("autoshop-store-test-capped");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("capped.json");
        std::fs::write(&p, b"12345678").unwrap();
        assert_eq!(read_text_capped(&p, 8).unwrap(), "12345678");
        let err = read_text_capped(&p, 7).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("limit"), "{err}");
        let missing = read_text_capped(&dir.join("absent.json"), 8).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        std::fs::write(&p, [0xFFu8, 0xFE]).unwrap();
        assert_eq!(
            read_text_capped(&p, 8).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
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

    /// 16-lane scan L08: an unreadable/foreign variants.json opened as a
    /// single card, and the next ordinary save DELETED it (with its .bak) —
    /// every background variant it recorded died to a Ctrl+S. The save
    /// primitives now refuse while the record is unresolved.
    #[test]
    fn an_unresolved_strip_refuses_save_and_clear_but_not_reads() {
        let base = std::env::temp_dir().join("autoshop-store-test-varunres");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("DSC_VARUNRES.ARW");
        std::fs::write(&photo, b"raw").unwrap();
        let dev = develop_dir(&photo);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();

        std::fs::write(variants_path(&photo), b"not json at all").unwrap();
        assert!(
            matches!(read_variants_checked(&photo), VariantsRead::Unresolved),
            "garbage bytes are Unresolved, never Absent"
        );
        assert!(read_variants(&photo).is_none(), "the plain reader still degrades");

        clear_variants(&photo).expect_err("clearing over an unresolved strip must refuse");
        let rec = VariantsRecord {
            v: 1,
            active_kind: "original".into(),
            active_pos: 0,
            others: Vec::new(),
        };
        write_variants(&photo, &rec).expect_err("overwriting an unresolved strip must refuse");
        assert_eq!(
            std::fs::read(variants_path(&photo)).unwrap(),
            b"not json at all",
            "the refused save left the record byte-identical"
        );

        // Removing the bad record ends the refusal — the normal flow resumes.
        std::fs::remove_file(variants_path(&photo)).unwrap();
        assert!(matches!(read_variants_checked(&photo), VariantsRead::Absent));
        write_variants(&photo, &rec).expect("a resolved store accepts saves again");
        clear_variants(&photo).expect("and clears");
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 16-lane scan L07: a develop pack naming `\\attacker\share\…` had its
    /// origin PROBED on open (`exists()` = an outbound SMB authentication).
    /// The refusal is lexical — no filesystem call may touch the path.
    #[test]
    #[cfg(windows)]
    fn network_and_device_paths_are_refused_lexically() {
        assert!(remote_or_device_path(Path::new(r"\\attacker\share\master.png")));
        assert!(remote_or_device_path(Path::new(r"\\?\UNC\attacker\share\m.png")));
        assert!(remote_or_device_path(Path::new(r"\\.\PhysicalDrive0")));
        assert!(!remote_or_device_path(Path::new(r"C:\photos\master.png")));
        assert!(!remote_or_device_path(Path::new("master.png")));

        let base = std::env::temp_dir().join("autoshop-store-test-uncorigin");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let photo = base.join("DSC_UNC.ARW");
        std::fs::write(&photo, b"raw").unwrap();
        let dev = develop_dir(&photo);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(
            pixel_source_path(&photo),
            br#"{"origin": "\\\\attacker\\share\\master.png", "kind": "inplace"}"#,
        )
        .unwrap();
        assert!(
            read_pixel_source(&photo).is_none(),
            "a UNC origin must not be restored (nor probed)"
        );

        // A recipe's bitmap ref pointing at a share is disabled, not probed.
        let mut r = EditRecipe {
            masks: vec![crate::recipe::LocalAdjustment {
                mask: crate::recipe::MaskGeometry::Bitmap {
                    path: r"\\attacker\share\mask.png".into(),
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        resolve_mask_paths(&mut r, &dev);
        let crate::recipe::MaskGeometry::Bitmap { path } = &r.masks[0].mask else {
            panic!("geometry kind must survive");
        };
        assert!(
            path.ends_with(".invalid-mask-reference"),
            "the share ref must be repointed at the never-existing sentinel: {path}"
        );
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

    /// A temp-dir fixture base spelled CANONICALLY (env vars can carry an
    /// off-case or 8.3 spelling of the temp dir, which would make lexical
    /// and canonical keys differ for reasons unrelated to the test).
    fn canonical_temp(tag: &str) -> PathBuf {
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let base = strip_verbatim(&base).join(format!("autoshop-store-test-{tag}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// A directory junction (no privilege needed, unlike symlinks). The
    /// fixture must build LOUDLY or the test is vacuous (the L14#5 lesson:
    /// a fixture that cannot build must never silently pass green).
    #[cfg(windows)]
    fn make_junction(link: &Path, target: &Path) {
        let ok = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(ok, "junction fixture could not be built: {} -> {}", link.display(), target.display());
    }

    /// C1/F10: for a photo already spelled canonically on a plain local
    /// drive, the canonical key is BYTE-IDENTICAL to the lexical key — the
    /// property that keeps existing develop dirs un-rekeyed.
    #[test]
    fn a_plain_local_path_keeps_the_key_it_had_before_canonical_identity() {
        let dir = canonical_temp("c1-plain");
        let photo = dir.join("_c1_plain.arw");
        std::fs::write(&photo, b"raw").unwrap();
        assert_eq!(photo_key(&photo), photo_key_lexical(&photo));
        // An ABSENT photo under a real folder resolves through its parent
        // and keeps the same key too (no disclosure, no divergence).
        let absent = dir.join("_c1_never_existed.arw");
        assert_eq!(photo_key(&absent), photo_key_lexical(&absent));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C1/F10 (user-decided 2026-08-10): network paths keep the lexical key
    /// — identity never spends a network round trip.
    #[test]
    fn a_network_path_is_keyed_lexically_without_probing() {
        let unc = Path::new(r"\\nas-that-does-not-exist\share\photos\DSC001.ARW");
        assert_eq!(photo_key(unc), photo_key_lexical(unc));
    }

    /// C1/F10: a junction alias and its target are ONE photo — one key, one
    /// develop dir, one develop lock.
    #[cfg(windows)]
    #[test]
    fn a_directory_junction_and_its_target_share_one_develop() {
        let dir = canonical_temp("c1-junction");
        let target = dir.join("real");
        std::fs::create_dir_all(&target).unwrap();
        let link = dir.join("alias");
        make_junction(&link, &target);
        let photo = target.join("_c1_junction.arw");
        std::fs::write(&photo, b"raw").unwrap();
        let via_alias = link.join("_c1_junction.arw");
        assert_ne!(
            photo_key_lexical(&via_alias),
            photo_key_lexical(&photo),
            "premise: the two spellings differ lexically"
        );
        assert_eq!(photo_key(&via_alias), photo_key(&photo), "one photo, one key");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C1/F10: a develop saved by a pre-canonical build under the alias
    /// spelling is ADOPTED into the canonical dir — copied no-clobber, the
    /// alias dir left intact with a superseded pointer, the note surfaced.
    #[cfg(windows)]
    #[test]
    fn an_alias_develop_dir_is_adopted_without_clobbering() {
        let dir = canonical_temp("c1-adopt");
        let target = dir.join("real");
        std::fs::create_dir_all(&target).unwrap();
        let link = dir.join("alias");
        make_junction(&link, &target);
        let photo = target.join("_c1_adopt.arw");
        std::fs::write(&photo, b"raw").unwrap();
        let via_alias = link.join("_c1_adopt.arw");

        let root = store_root();
        let ld = root.join("develops").join(photo_key_lexical(&via_alias));
        let cd_expect = root.join("develops").join(photo_key(&via_alias));
        let _ = std::fs::remove_dir_all(&ld);
        let _ = std::fs::remove_dir_all(&cd_expect);
        std::fs::create_dir_all(&ld).unwrap();
        std::fs::write(ld.join("recipe.json"), b"{\"exposure_ev\":0.5}").unwrap();
        std::fs::write(ld.join("mask-sky.png"), b"raster").unwrap();
        std::fs::write(ld.join("source.txt"), b"breadcrumb").unwrap();

        let dev = develop_dir(&via_alias);
        assert_eq!(dev, cd_expect, "the photo now lives under its canonical key");
        assert_eq!(std::fs::read(dev.join("recipe.json")).unwrap(), b"{\"exposure_ev\":0.5}");
        assert_eq!(std::fs::read(dev.join("mask-sky.png")).unwrap(), b"raster");
        assert!(dev.join("adopted-from.txt").exists(), "the adoption breadcrumb landed");
        assert!(!dev.join("adopting-from.txt").exists(), "the in-flight marker was consumed");
        assert!(
            ld.join("superseded-by.txt").exists(),
            "the alias dir points at its successor"
        );
        assert_eq!(
            std::fs::read(ld.join("recipe.json")).unwrap(),
            b"{\"exposure_ev\":0.5}",
            "adoption copies — the alias dir stays intact as a frozen backup"
        );
        assert!(
            matches!(take_alias_note(&via_alias), Some(AliasNote::Adopted { .. })),
            "the adoption is surfaced to the GUI once"
        );

        let _ = std::fs::remove_dir_all(&ld);
        let _ = std::fs::remove_dir_all(&cd_expect);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C1/F10 (user-decided 2026-08-10): when BOTH spellings hold a real
    /// develop, the canonical one wins, the alias is left untouched, and
    /// the fact is disclosed durably + to the GUI — never merged, never
    /// guessed by mtime.
    #[cfg(windows)]
    #[test]
    fn two_spellings_with_two_develops_keep_both_and_disclose() {
        let dir = canonical_temp("c1-collide");
        let target = dir.join("real");
        std::fs::create_dir_all(&target).unwrap();
        let link = dir.join("alias");
        make_junction(&link, &target);
        let photo = target.join("_c1_collide.arw");
        std::fs::write(&photo, b"raw").unwrap();
        let via_alias = link.join("_c1_collide.arw");

        let root = store_root();
        let ld = root.join("develops").join(photo_key_lexical(&via_alias));
        let cd = root.join("develops").join(photo_key(&via_alias));
        let _ = std::fs::remove_dir_all(&ld);
        let _ = std::fs::remove_dir_all(&cd);
        std::fs::create_dir_all(&ld).unwrap();
        std::fs::create_dir_all(&cd).unwrap();
        std::fs::write(ld.join("recipe.json"), b"alias-develop").unwrap();
        std::fs::write(cd.join("recipe.json"), b"canonical-develop").unwrap();

        let dev = develop_dir(&via_alias);
        assert_eq!(dev, cd);
        assert_eq!(
            std::fs::read(cd.join("recipe.json")).unwrap(),
            b"canonical-develop",
            "the canonical develop is untouched"
        );
        assert_eq!(
            std::fs::read(ld.join("recipe.json")).unwrap(),
            b"alias-develop",
            "the alias develop is untouched — no merge, no delete"
        );
        assert!(!ld.join("superseded-by.txt").exists(), "a collision never supersedes");
        assert!(cd.join("aliased-develops.txt").exists(), "the fact is durable");
        assert!(
            matches!(take_alias_note(&via_alias), Some(AliasNote::SecondDevelop { .. })),
            "the collision is surfaced to the GUI once"
        );

        let _ = std::fs::remove_dir_all(&ld);
        let _ = std::fs::remove_dir_all(&cd);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C1/F10: unsettled transaction state under the alias spelling defers
    /// adoption — this session keys lexically (the pre-upgrade behaviour),
    /// the residue settles in place, the NEXT session adopts.
    #[cfg(windows)]
    #[test]
    fn a_pending_transaction_defers_adoption() {
        let dir = canonical_temp("c1-defer");
        let target = dir.join("real");
        std::fs::create_dir_all(&target).unwrap();
        let link = dir.join("alias");
        make_junction(&link, &target);
        let photo = target.join("_c1_defer.arw");
        std::fs::write(&photo, b"raw").unwrap();
        let via_alias = link.join("_c1_defer.arw");

        let root = store_root();
        let ld = root.join("develops").join(photo_key_lexical(&via_alias));
        let cd = root.join("develops").join(photo_key(&via_alias));
        let _ = std::fs::remove_dir_all(&ld);
        let _ = std::fs::remove_dir_all(&cd);
        std::fs::create_dir_all(&ld).unwrap();
        std::fs::write(ld.join("recipe.json"), b"{}").unwrap();
        std::fs::write(ld.join("clear.pending"), b"develop clear in progress\n").unwrap();

        let dev = develop_dir(&via_alias);
        assert_eq!(dev, ld, "unsettled state keeps the pre-upgrade lexical key");
        assert!(!ld.join("superseded-by.txt").exists());

        let _ = std::fs::remove_dir_all(&ld);
        let _ = std::fs::remove_dir_all(&cd);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L13#2: the badge/resume predicate counts the sidecar Lightroom
    /// itself writes beside the RAW — a photo edited only in Lightroom
    /// showed no ● yet opened with its LR develop, and CLI batch spent a
    /// paid analyze on it.
    #[test]
    fn has_develop_or_sidecar_counts_a_lightroom_sidecar_beside_the_raw() {
        let dir = std::env::temp_dir().join("autoshop-store-test-lr-badge");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_lr_badge.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::write(raw.with_extension("xmp"), b"<x:xmpmeta/>").unwrap();

        assert!(!has_develop(&raw), "no store develop");
        assert!(has_develop_or_sidecar(&raw), "the LR sidecar counts");

        // The is_raw guard: a baked photo's neighbouring .xmp is not ours.
        let png = dir.join("_store_lr_badge.png");
        std::fs::write(&png, b"png").unwrap();
        std::fs::write(png.with_extension("xmp"), b"<x:xmpmeta/>").unwrap();
        let png_dev = develop_dir(&png);
        let _ = std::fs::remove_dir_all(&png_dev);
        assert!(!has_develop_or_sidecar(&png), "a baked photo's sidecar does not count");

        // A pending clear still outranks the sidecar probe (L03).
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("clear.pending"), b"develop clear in progress\n").unwrap();
        assert!(!has_develop_or_sidecar(&raw), "a pending clear masks the badge");

        // Regression guard: the SIBLING did not leak into has_develop —
        // rank_lightroom_sidecar still classifies a sidecar-only photo as
        // LrSidecar::Only (has_develop false), not a store contest.
        let _ = std::fs::remove_dir_all(&dev);
        match lightroom_sidecar(&raw) {
            LrSidecar::Only(_) => {}
            _ => panic!("a sidecar-only photo must rank as LrSidecar::Only"),
        }

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&png_dev);
    }

    /// L13#1: the one-lock snapshot carries the Lightroom sidecar and the
    /// store's XMP projection with the open path's precedence, so the batch
    /// renderer can answer exactly what opening the photo would show.
    #[test]
    fn a_develop_snapshot_carries_the_xmp_layers_the_open_path_reads() {
        let dir = std::env::temp_dir().join("autoshop-store-test-snap-xmp");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_snap_xmp.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();

        // XMP-only develop: the store projection answers, no recipe.
        let xmp = crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 21.0, ..Default::default() });
        std::fs::write(xmp_target(&raw), &xmp).unwrap();
        let snap = read_develop_snapshot(&raw).unwrap();
        assert!(snap.recipe.is_none());
        assert!(snap.lr_xmp.is_none());
        let (text, kind) = snap.store_xmp.expect("the projection rides the snapshot");
        assert_eq!(text, xmp);
        assert_eq!(kind, "XMP");

        // A NEWER Lightroom sidecar beside the RAW outranks the recipe.
        std::fs::write(recipe_target(&raw), b"{\"exposure_ev\":0.5}").unwrap();
        let lr = raw.with_extension("xmp");
        std::fs::write(
            &lr,
            crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        let snap = read_develop_snapshot(&raw).unwrap();
        assert!(snap.recipe.is_some(), "the recipe still rides along");
        let (_, kind) = snap.lr_xmp.expect("the newer Lightroom sidecar rides the snapshot");
        assert!(kind.contains("Lightroom"), "{kind}");

        // An unreadable sidecar is disclosed, never folded into absence.
        std::fs::write(&lr, [0xFF, 0xFE, 0x00, 0xDC, 0x00]).unwrap();
        let snap = read_develop_snapshot(&raw).unwrap();
        assert!(snap.lr_xmp.is_none());
        assert!(snap.lr_unreadable.is_some(), "unreadable is not absent");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L13#3: the backup gate snapshots the Lightroom sidecar when it is
    /// the newest intent — store develop FIRST, sidecar SECOND, so version
    /// numbers encode intent order; repeats dedup; a neutral sidecar is not
    /// a save.
    #[test]
    fn a_newer_lightroom_sidecar_is_snapshotted_before_a_programmatic_write() {
        let dir = std::env::temp_dir().join("autoshop-store-test-lr-backup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_lr_backup.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let stored = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(recipe_target(&raw), serde_json::to_string(&stored).unwrap()).unwrap();
        let lr = raw.with_extension("xmp");
        let lr_text = crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() });
        std::fs::write(&lr, &lr_text).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();

        let incoming = EditRecipe { exposure_ev: 1.5, ..Default::default() };
        let n = backup_saved_develop(&raw, Some(&incoming)).unwrap();
        assert_eq!(n, Some(2), "two snapshots: the store develop, then the newer sidecar");
        assert_eq!(list_versions(&raw), vec![1, 2]);
        let v1: EditRecipe =
            serde_json::from_str(&std::fs::read_to_string(version_target(&raw, 1)).unwrap())
                .unwrap();
        assert_eq!(v1.exposure_ev, 0.5, "v1 is the store develop (older intent)");
        let stem = crate::pipeline::stem(&raw);
        assert_eq!(
            std::fs::read_to_string(dev.join(format!("v2.{stem}.xmp"))).unwrap(),
            lr_text,
            "v2 carries the sidecar's lossless bytes (newer intent)"
        );

        // The real caller flow: the gated write lands, then a LATER gated
        // write with the same content — the sidecar unchanged. Both halves
        // dedup: no new version.
        std::fs::write(recipe_target(&raw), serde_json::to_string(&incoming).unwrap()).unwrap();
        let versions_before = list_versions(&raw);
        let again = backup_saved_develop(&raw, Some(&incoming)).unwrap();
        assert_eq!(again, None, "nothing new to snapshot");
        assert_eq!(list_versions(&raw), versions_before);

        // The sidecar file itself is never touched.
        assert_eq!(std::fs::read_to_string(&lr).unwrap(), lr_text);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L13#3: a sidecar-ONLY develop (no store files at all) is snapshotted
    /// instead of reported as "nothing to snapshot" — and a NEUTRAL sidecar
    /// still is not a save.
    #[test]
    fn a_sidecar_only_develop_is_snapshotted_instead_of_reported_as_nothing() {
        let dir = std::env::temp_dir().join("autoshop-store-test-lr-only-backup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_store_lr_only.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let lr = raw.with_extension("xmp");
        let lr_text = crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 12.0, ..Default::default() });
        std::fs::write(&lr, &lr_text).unwrap();

        let n = backup_saved_develop(&raw, None).unwrap();
        assert_eq!(n, Some(1), "the sidecar IS the develop and is preserved");
        let stem = crate::pipeline::stem(&raw);
        assert_eq!(
            std::fs::read_to_string(dev.join(format!("v1.{stem}.xmp"))).unwrap(),
            lr_text
        );

        // Neutral sidecar: not a save, nothing snapshotted.
        let raw2 = dir.join("_store_lr_neutral.arw");
        std::fs::write(&raw2, b"raw").unwrap();
        let dev2 = develop_dir(&raw2);
        let _ = std::fs::remove_dir_all(&dev2);
        std::fs::write(raw2.with_extension("xmp"), b"<x:xmpmeta/>").unwrap();
        assert_eq!(backup_saved_develop(&raw2, None).unwrap(), None);
        assert!(list_versions(&raw2).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&dev2);
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

        /// L01: the settings lock is the develop lock's machinery pointed at
        /// ONE store-root-wide file — kernel-owned (it reaches the GUI and
        /// serve PROCESSES; threads model them here), reentrant on its own
        /// thread (a writer's cycle contains the loader's rescue), and
        /// NoWait-refusing while held.
        #[test]
        fn a_settings_lock_serializes_writers_and_reenters_on_its_thread() {
            let root = std::env::temp_dir().join("autoshop-store-test-settings-lock");
            let _ = std::fs::remove_dir_all(&root);

            let nested = with_settings_lock_in(&root, DevelopLockMode::Wait, || {
                // The rescue inside a locked writer's own load re-enters
                // instead of deadlocking against its own cycle.
                with_settings_lock_in(&root, DevelopLockMode::NoWait, || {
                    Ok::<_, std::io::Error>(7)
                })
            });
            assert_eq!(nested.unwrap(), 7);

            with_settings_lock_in(&root, DevelopLockMode::Wait, || {
                let root2 = root.clone();
                let busy = std::thread::spawn(move || {
                    with_settings_lock_in(&root2, DevelopLockMode::NoWait, || {
                        Ok::<_, std::io::Error>(())
                    })
                })
                .join()
                .unwrap();
                assert_eq!(busy.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
                Ok::<_, std::io::Error>(())
            })
            .unwrap();

            with_settings_lock_in(&root, DevelopLockMode::NoWait, || {
                Ok::<_, std::io::Error>(())
            })
            .expect("dropping the owner releases the settings lock");
            let _ = std::fs::remove_dir_all(&root);
        }


        /// L03: the out-of-module publish tail — staged bytes land intact
        /// under the live name, the stage is consumed, and the staged
        /// file's mode travels with the rename (the 0600 settings claim).
        #[test]
        fn durable_replace_lands_complete_bytes_and_consumes_its_stage() {
            let dir = std::env::temp_dir().join("autoshop-store-test-durable-replace");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let live = dir.join("settings.json");
            std::fs::write(&live, b"old").unwrap();
            let staged = dir.join("settings.json.stage");
            std::fs::write(&staged, b"complete new bytes").unwrap();

            durable_replace(&staged, &live).unwrap();

            assert_eq!(std::fs::read(&live).unwrap(), b"complete new bytes");
            assert!(!staged.exists(), "a completed publish consumes its stage");
            let _ = std::fs::remove_dir_all(&dir);
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
