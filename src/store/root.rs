//! The store root: trust, the store directory names, the pre-rename adoption, the per-user data directory, and the FNV hash.

use super::*;

/// Whether [`store_root`] resolved to a directory only this account can write,
/// or to the last-resort shared temp fallback.
///
/// This is a TRUST label with teeth, not a breadcrumb. The settings file lives
/// under the root ([`settings_path`]), and a settings file the loader considers
/// CENTRAL may supply an API key *and* the endpoint that key is sent to
/// (`config::SettingsOrigin`). `<temp>/autoshade` is writable by every account
/// on the machine, so a file pre-planted there inherited exactly that
/// authority — the same "extract a shared archive, run AutoShade" attack the
/// ambient guards close for the working directory, through a world-writable
/// directory instead. A shared root therefore carries AMBIENT authority.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RootTrust {
    /// `AUTOSHADE_DATA_DIR` (from the user's own environment), `%LOCALAPPDATA%`,
    /// `$XDG_DATA_HOME`, or `$HOME/.local/share` — per-account.
    PerUser,
    /// `<temp>/autoshade`: nothing better answered. Shared with every account.
    SharedFallback,
}

/// The directory this app keeps its develop store in, and the one it kept it
/// in up to v1.1.0, when it was called Autoshop.
///
/// Cased per platform, because the parent's convention differs.
/// `%LOCALAPPDATA%` and the XDG data directory hold lowercase folder names;
/// `Library/Application Support` holds applications' DISPLAY names, and a
/// lowercase `autoshade` sitting among them reads as debris. This is the FIRST
/// macOS release, so there is no existing spelling to preserve there — and it
/// must be settled before one ships, since the directory under it is named by
/// a hash of an absolute path and a later change orphans every develop.
#[cfg(target_os = "macos")]
pub(crate) const STORE_DIR_NAME: &str = "AutoShade";
#[cfg(not(target_os = "macos"))]
pub(crate) const STORE_DIR_NAME: &str = "autoshade";
/// The store directory's pre-rename name.
///
/// KEPT, and this is the settled answer rather than another grace period:
/// v1.2.4 retired the `AUTOSHOP_*` environment door and the pre-rename settings
/// FILE name, both of which stood in configuration the app rewrites on the next
/// save. This one stands in front of the user's develops — every edit they have
/// ever made — and there is no release after which a v1.1 install stops having
/// them under the old folder name. Someone jumping from v1.1 straight to a much
/// later version must still find their store, so [`adopt_pre_rename_root`]
/// keeps looking; it renames once, discloses what it did, and a store that has
/// been adopted never meets this constant again.
pub(crate) const LEGACY_STORE_DIR_NAME: &str = "autoshop";

/// Whether a pre-rename directory sitting beside the current one is OURS.
///
/// False on macOS, and that is a data-safety rule rather than tidiness. This
/// app's first macOS release is already called AutoShade — there has never
/// been an Autoshop on a Mac — so a folder of that name in the Mac's
/// application-support directory belongs to someone else. APFS is also
/// case-insensitive by default, so the `is_dir()` probe would match
/// `Autoshop` as readily as `autoshop`, and [`adopt_pre_rename_root`] does not
/// merely read what it finds: it RENAMES it. Nothing to gain, a stranger's
/// directory to lose.
///
/// A plain `const` so Windows and Linux constant-fold it away and keep the
/// adoption path byte-for-byte as it was.
pub(super) const ADOPT_PRE_RENAME: bool = !cfg!(target_os = "macos");

/// What became of a pre-rename store directory sitting beside the new one.
///
/// Every outcome is disclosed, including the boring one — a develop store is
/// where every edit the user has ever made lives, so "which folder am I
/// actually writing to" is never allowed to be a guess.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RootAdoption {
    /// No pre-rename directory: nothing to adopt, nothing to say.
    Nothing,
    /// It was renamed onto the current name. One `rename`, same volume, so
    /// every develop, snapshot, mask raster and lock moved together or not at
    /// all — a copy could half-succeed and leave two divergent stores.
    Migrated,
    /// BOTH names exist. The current one is used and the old one is left
    /// exactly where it is: merging two stores is a decision about the user's
    /// edits, and this code does not get to make it silently.
    KeptBoth,
    /// The rename failed (a file open in another process, a permission, a
    /// junction across volumes). The PRE-RENAME directory stays in use, so the
    /// user's existing edits keep working; the next launch tries again.
    FellBack,
}

/// Where the store lives under `parent`, adopting a pre-rename directory when
/// there is one.
///
/// `act` gates BOTH the rename attempt and the disclosure, and the caller
/// passes it exactly once per process ([`store_root_with_trust`] holds the
/// latch). That is what keeps a failed rename from being retried on every one
/// of the hundreds of `store_root()` calls a session makes, and keeps the
/// disclosure to one line. With `act` false the answer is still correct: the
/// three exists-checks alone say which directory is in use.
///
/// `adopt` is [`ADOPT_PRE_RENAME`] at both production call sites, and a
/// parameter rather than a direct read of the const for the reason the GUI's
/// `adopt_prefs_between` takes the same decision as an argument: with it wired
/// in, every branch below -- the refusal included -- is reachable from the
/// battery on every platform. Reading the const here instead makes three of
/// the four branches unreachable on macOS, where the tests that describe them
/// then assert something that cannot happen, and leaves the refusal itself
/// executed nowhere at all.
pub(crate) fn adopt_pre_rename_root(
    parent: &Path,
    act: bool,
    adopt: bool,
) -> (PathBuf, RootAdoption) {
    let current = parent.join(STORE_DIR_NAME);
    if !adopt {
        return (current, RootAdoption::Nothing);
    }
    let legacy = parent.join(LEGACY_STORE_DIR_NAME);
    if !legacy.is_dir() {
        return (current, RootAdoption::Nothing);
    }
    if current.exists() {
        if act {
            eprintln!(
                "note: a pre-rename {LEGACY_STORE_DIR_NAME} folder sits beside the \
                 {STORE_DIR_NAME} one this version uses. Nothing was moved or merged — \
                 {STORE_DIR_NAME} is in use, and any develops still in the old folder \
                 are reachable by moving them across yourself."
            );
        }
        return (current, RootAdoption::KeptBoth);
    }
    if !act {
        // A rename was already attempted this process and did not happen —
        // otherwise `legacy` would be gone or `current` would exist.
        return (legacy, RootAdoption::FellBack);
    }
    match std::fs::rename(&legacy, &current) {
        Ok(()) => {
            eprintln!(
                "note: your develop store moved from {LEGACY_STORE_DIR_NAME} to \
                 {STORE_DIR_NAME} (the app was renamed). Every develop, version and \
                 mask came with it; nothing was copied or duplicated."
            );
            (current, RootAdoption::Migrated)
        }
        Err(e) => {
            eprintln!(
                "warning: could not move your develop store from {LEGACY_STORE_DIR_NAME} \
                 to {STORE_DIR_NAME} ({e}). Still using the old folder, so nothing is \
                 lost; the next launch tries again."
            );
            (legacy, RootAdoption::FellBack)
        }
    }
}

/// Per-user store root and its trust label. Resolution order:
/// `AUTOSHADE_DATA_DIR` (env override for tests / portable setups) → a TEST
/// executable's own `<target>/<profile>/test-store` → the platform's own
/// per-account data directory → `<temp>/autoshade`. Absolute, so keys and
/// targets never depend on the process cwd.
///
/// The per-account step used to be `%LOCALAPPDATA%` on EVERY platform — a
/// variable Unix does not set. So every Linux/macOS build fell through to
/// `/tmp/autoshade` and then handed the settings file found there full central
/// authority, keys and base URLs included, on a directory any local account can
/// write first. Each platform now names its own directory, and the shared
/// fallback is LABELLED rather than trusted (the loader downgrades it).
///
/// The test step (2026-11-18): a test executable used to resolve the
/// per-account root whenever the variable was unset — the release battery
/// sets it, an ad-hoc test run did not — so such runs wrote fixture develops
/// into the user's real store (hundreds of folders were found there) and
/// could read the user's real settings file, key and endpoint included, from
/// it. A `#[cfg(test)]` default reached only the library's own unit tests:
/// the desktop app's tests and `tests/*.rs` link the library's normal build.
/// What every test executable shares is where cargo puts it
/// ([`test_executable_root`]).
///
/// An explicit `AUTOSHADE_DATA_DIR` names the directory outright and is never
/// second-guessed — it is how tests and portable setups site the store, and
/// adopting some sibling of it would be an ambush. The same holds for the
/// test step. The two per-user DERIVED roots go through
/// [`adopt_pre_rename_root`], which is where an existing Autoshop install
/// becomes an AutoShade one.
pub fn store_root_with_trust() -> (PathBuf, RootTrust) {
    // First caller of the process gets to rename and to disclose; every later
    // one reads the result off the filesystem.
    static ADOPTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let latch = || ADOPTED.set(()).is_ok();
    let (root, trust) = crate::config::live_env_os("AUTOSHADE_DATA_DIR")
        .map(|d| (PathBuf::from(d), RootTrust::PerUser))
        // The developer's own target folder: trusted like the per-user root.
        .or_else(|| test_executable_root().map(|d| (d, RootTrust::PerUser)))
        .or_else(|| {
            per_user_data_dir()
                .map(|d| {
                    (adopt_pre_rename_root(&d, latch(), ADOPT_PRE_RENAME).0, RootTrust::PerUser)
                })
        })
        .unwrap_or_else(|| {
            (
                adopt_pre_rename_root(&std::env::temp_dir(), latch(), ADOPT_PRE_RENAME).0,
                RootTrust::SharedFallback,
            )
        });
    (std::path::absolute(&root).unwrap_or(root), trust)
}

pub fn store_root() -> PathBuf {
    store_root_with_trust().0
}

/// The store a TEST executable uses when no root is named: the folder
/// `test-store` beside its own `deps` folder (`<target>/<profile>/test-store`).
///
/// Cargo puts every test and bench executable — the library's unit tests, the
/// desktop app's tests, `tests/*.rs`, and the children a test re-spawns from
/// `current_exe()` — into `<target>/<profile>/deps/`; a program a person runs
/// (`cargo run`, an installed or trial build) never starts from a folder named
/// `deps`. `None` (the order goes on as before) when the executable cannot be
/// named or its folder is not named exactly `deps`. Asked once per process,
/// and silent: tests read the stderr of the children they spawn.
pub(crate) fn test_executable_root() -> Option<PathBuf> {
    static ROOT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let deps = exe.parent()?;
        if deps.file_name()? != "deps" {
            return None;
        }
        Some(deps.parent()?.join("test-store"))
    })
    .clone()
}

/// [`per_user_data_dir`] for the pin that a test executable never resolves
/// it.
#[cfg(test)]
pub(crate) fn per_user_data_dir_for_tests() -> Option<PathBuf> {
    per_user_data_dir()
}

/// The platform's per-account data directory, or `None` when the environment
/// names none (a bare service account, a chroot without `HOME`). Read from the
/// LIVE environment only — nothing in this project writes the process
/// environment, so a `.env` cannot reach these names.
#[cfg(windows)]
fn per_user_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

/// macOS keeps per-application state under `Library/Application Support` in
/// the user's home.
///
/// XDG is a Linux convention and nothing on a Mac sets `XDG_DATA_HOME`, so the
/// generic Unix arm below fell through to the HOME-relative `.local/share`: a
/// hidden directory no Mac user looks in, and not where eframe already writes
/// this app's window preferences. Sharing one application-support folder with
/// those preferences IS the platform's layout — they occupy different subpaths
/// (`data/app.ron` beside `develops/`), and nothing in this file enumerates or
/// prunes the root's children.
///
/// `XDG_DATA_HOME` is deliberately not consulted: it is not a variable a Mac
/// user sets for this app, and honouring it would let one moved by some
/// unrelated tool relocate a photographer's whole edit history.
#[cfg(target_os = "macos")]
fn per_user_data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|h| h.join("Library").join("Application Support"))
}

#[cfg(not(any(windows, target_os = "macos")))]
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
/// across Rust releases, and this hash names PERSISTENT directories (and
/// fingerprints the deleted-version registry) — a changed hash would orphan
/// every existing develop on a toolchain bump.
pub(super) const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The incremental fold behind [`fnv1a64`], for streaming callers
/// (`file_fnv1a64` hashes rasters chunk-wise in bounded memory).
pub(super) fn fnv1a64_update(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(FNV_PRIME))
}

pub(super) fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_update(FNV_OFFSET, bytes)
}
