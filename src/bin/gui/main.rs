// Release builds run WITHOUT a console window — a GUI app shouldn't flash a
// terminal on launch. Debug keeps the console so panics/logs stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! AutoShade — native desktop GUI (egui/eframe).
//!
//! A real native window (no localhost server, no webview): it links the
//! `autoshade` engine library and calls `decode` / `render` / `pipeline` directly
//! in-process. Open a RAW or image, develop it with live before/after, run the
//! AI auto-develop, and export — all from one window.
//!
//! Build/run: `cargo run --release --features gui --bin autoshade-gui`

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

// NOTE: `MaskRole` is addressed only by method here (`m.role.en_name()` in the
// mask row), never named as a type, so it is intentionally NOT imported — the
// enum lives in recipe.rs and is set by the engine, not constructed in the GUI.
use autoshade::recipe::{ColorGrade, CurvePoint, EditRecipe, Hsl, MaskGeometry, RangeMask};
use image::GenericImageView;

// Native-GUI i18n: English is the skeleton/key, Chinese is a single overlay
// table with English fallback. `tr`/`trf` are called with the English literal;
// see i18n.rs. (Private submodule — enabled by `autobins = false` in Cargo.toml.)
mod i18n;
use i18n::{tr, trf, Lang};

mod model;
mod persist;
// The PORTABLE half of the quit guard: its state machine, and the
// process-wide slots the delegate method reads through. Compiled and tested on
// every platform on purpose — the Windows battery is what gates this decision
// — but its non-test caller is `macos.rs`, so off macOS the entry points are
// genuinely uncalled and the compiler is right to say so. The allow is scoped
// to exactly that case: on macOS real dead code here is still caught.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod quit;
// The Objective-C half of the quit guard. `quit.rs` above is its portable
// state machine and is compiled (and tested) on every platform.
#[cfg(target_os = "macos")]
mod macos;
mod theme;
mod util;
mod actions;
mod app;
mod budget;
mod canvas;
mod export;
mod masks;
mod panels;
mod workers;
use model::*;
use persist::*;
use theme::*;
use util::*;
use app::*;



/// Decode the embedded AutoShade icon for the window title bar / taskbar.
/// See main(): the last-crash report lands at `<store root>/panic.log`,
/// and (Windows) a native message box points the user at it.
fn install_panic_reporter() {
    let default = std::panic::take_hook();
    // Captured HERE, where this is provably the main thread: `main()` calls
    // this before anything else. The macOS dialog below is main-thread-only.
    #[cfg(target_os = "macos")]
    let main_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        // The claim must match the outcome (review R12-10): on a first
        // launch the store root may not exist yet, and a failed write must
        // not direct the user at a report that is not there.
        let root = autoshade::store::store_root();
        let _ = std::fs::create_dir_all(&root);
        let log = root.join("panic.log");
        let wrote = std::fs::write(&log, format!("AutoShade crashed: {info}\n").as_bytes()).is_ok();
        // Built once, shown by whichever dialog this platform has. The claim
        // still has to match the outcome, which is what `wrote` decides.
        let msg = if wrote {
            format!(
                "AutoShade hit an internal error and must close.\n\n{info}\n\nA report was written to:\n{}",
                log.display()
            )
        } else {
            format!(
                "AutoShade hit an internal error and must close.\n\n{info}\n\n(a report could NOT be written to {})",
                log.display()
            )
        };
        #[cfg(windows)]
        message_box(&msg);
        // macOS has exactly the problem Windows has — a release build is
        // windowed, so the panic's stderr reaches nobody and the window simply
        // vanishes — and rfd is already in the tree for the file pickers, so
        // this costs no new dependency.
        //
        // MAIN THREAD ONLY. rfd's macOS backend dispatches onto the main queue
        // and blocks for the answer, so raising it from a worker while the main
        // thread is itself unwinding would hang a process that was merely
        // crashing. A worker panic still writes `panic.log` and still reaches
        // the default hook — it just does not get a dialog.
        #[cfg(target_os = "macos")]
        if std::thread::current().id() == main_thread {
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("AutoShade")
                .set_description(&msg)
                .show();
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        let _ = &msg;
        default(info);
    }));
}

/// Native blocking message box (user32, the store.rs raw-link pattern —
/// windows-sys is in the tree but without the WindowsAndMessaging gate).
#[cfg(windows)]
fn message_box(text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        #[link_name = "MessageBoxW"]
        fn message_box_w(hwnd: usize, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    const MB_OK_ICONERROR: u32 = 0x0000_0010;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "AutoShade".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated buffers, live across the synchronous call.
    unsafe { message_box_w(0, wide.as_ptr(), title.as_ptr(), MB_OK_ICONERROR) };
}

/// The eframe STORAGE KEY — not a display name; the window title is set on the
/// viewport. With `persistence_path` unset (every real launch) eframe derives
/// the prefs file from THIS string: `%APPDATA%\<key>\data\app.ron` on Windows
/// (eframe 0.29 glow_integration.rs:200 -> file_storage.rs:37).
///
/// One spelling everywhere since the C2 ruling (2026-08-31: migrate, do not
/// accept the old name). Renaming the key ALONE would silently abandon every
/// existing user's window geometry, last library, view mode, export options
/// and theme — so [`adopt_pre_rename_prefs`] moves the pre-rename directory
/// first, on the develop store's own rename-or-fall-back doctrine
/// (`store::adopt_pre_rename_root`).
const STORAGE_KEY: &str = "AutoShade";
/// The spelling every pre-rename install wrote its prefs under; read (and
/// renamed away) by [`adopt_pre_rename_prefs`], never written again.
const LEGACY_STORAGE_KEY: &str = "Autoshop";
/// macOS opts out for `store::ADOPT_PRE_RENAME`'s reason: `Application
/// Support` holds every program's folders under DISPLAY names, no Mac ever ran
/// the pre-rename spelling, and on case-insensitive APFS an adoption could
/// only rename a stranger's directory.
const ADOPT_PRE_RENAME_PREFS: bool = !cfg!(target_os = "macos");

/// Outcome of [`adopt_prefs_between`] — `store::RootAdoption`'s shape, for the
/// prefs directory.
#[derive(Debug, PartialEq, Eq)]
enum PrefsAdoption {
    /// No pre-rename directory (or the platform opts out): nothing to move.
    Nothing,
    /// Both spellings exist: the new one wins and the old is not touched.
    KeptBoth,
    /// The pre-rename directory was renamed to the new spelling, prefs inside.
    Migrated,
    /// The rename failed. The caller keeps the LEGACY key for this session —
    /// the prefs keep working, nothing is reset — and the next launch retries.
    FellBack,
}

/// The migration core, on explicit paths so the battery drives it with temp
/// directories: rename `legacy` to `current` when only `legacy` exists,
/// making `current`'s parent first because on Windows it is not there yet.
fn adopt_prefs_between(current: &Path, legacy: &Path, adopt: bool) -> PrefsAdoption {
    if !adopt || !legacy.is_dir() {
        return PrefsAdoption::Nothing;
    }
    if current.exists() {
        return PrefsAdoption::KeptBoth;
    }
    // `eframe::storage_dir` is TWO levels deep on Windows —
    // `%APPDATA%\<key>\data` — so `current`'s parent is the app's own folder
    // under the NEW spelling, which a machine that never ran the new name does
    // not have. Renaming into it fails ERROR_PATH_NOT_FOUND and the retry the
    // FellBack arm promises fails identically on every later launch. The two
    // sibling adoptions (`store::adopt_pre_rename_root`,
    // `serve::adopted_export_registry_root`) rename WITHIN a directory that
    // already exists, which is why only this one needs the parent made first.
    if let Some(parent) = current.parent()
        && !parent.as_os_str().is_empty()
        && std::fs::create_dir_all(parent).is_err()
    {
        return PrefsAdoption::FellBack;
    }
    match std::fs::rename(legacy, current) {
        Ok(()) => PrefsAdoption::Migrated,
        Err(_) => PrefsAdoption::FellBack,
    }
}

/// Resolve the storage key for THIS launch, adopting the pre-rename prefs
/// directory on the way. Sandboxed runs (`AUTOSHADE_DATA_DIR`) never touch the
/// real per-user directories — `persistence_path` overrides storage wholesale,
/// so there is nothing to migrate and nothing real to disturb.
fn adopt_pre_rename_prefs() -> &'static str {
    if autoshade::config::live_env_os("AUTOSHADE_DATA_DIR").is_some() {
        return STORAGE_KEY;
    }
    let (Some(current), Some(legacy)) =
        (eframe::storage_dir(STORAGE_KEY), eframe::storage_dir(LEGACY_STORAGE_KEY))
    else {
        return STORAGE_KEY;
    };
    match adopt_prefs_between(&current, &legacy, ADOPT_PRE_RENAME_PREFS) {
        PrefsAdoption::Nothing => STORAGE_KEY,
        PrefsAdoption::Migrated => {
            eprintln!(
                "note: your window/library preferences moved from {LEGACY_STORAGE_KEY} to \
                 {STORAGE_KEY} (the app was renamed). Nothing was copied or duplicated."
            );
            STORAGE_KEY
        }
        PrefsAdoption::KeptBoth => {
            eprintln!(
                "note: a pre-rename {LEGACY_STORAGE_KEY} preferences folder sits beside the \
                 {STORAGE_KEY} one this version uses. Nothing was moved or merged — \
                 {STORAGE_KEY} is in use."
            );
            STORAGE_KEY
        }
        PrefsAdoption::FellBack => {
            eprintln!(
                "warning: could not move your preferences from {LEGACY_STORAGE_KEY} to \
                 {STORAGE_KEY}. Still using the old folder, so nothing is lost; the next \
                 launch tries again."
            );
            LEGACY_STORAGE_KEY
        }
    }
}

fn app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../../../assets/icon_256.png"))
        .expect("embedded icon decodes")
        .to_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData { rgba: img.into_raw(), width, height }
}

fn main() -> eframe::Result<()> {
    // A windowed release build has NO console: a panic wrote its report to a
    // stderr nobody can see and the window simply vanished (L14-7). The hook
    // writes the report beside the develop store and says so in a native
    // message box — the one channel that needs no working event loop.
    install_panic_reporter();
    // Before the first render can touch rayon: on an already-tight machine
    // the global pool is built narrower than one-per-logical-core (measured
    // trade-off in budget.rs).
    budget::clamp_global_rayon();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 880.0])
            // Below this the wrapped toolbar rows + two side panels leave no
            // usable canvas; wrapping (not clipping) covers everything above.
            .with_min_inner_size([980.0, 620.0])
            .with_title("AutoShade")
            .with_icon(std::sync::Arc::new(app_icon())),
        // The develop STORE already follows AUTOSHADE_DATA_DIR; the eframe
        // prefs file (last library, theme, window geometry) did not, so a
        // sandboxed E2E launch read — and on exit rewrote — the REAL user's
        // prefs, and its window opened onto their actual photo library. One
        // sandbox variable must mean the whole app is sandboxed.
        persistence_path: autoshade::config::live_env_os("AUTOSHADE_DATA_DIR")
            .map(|d| std::path::PathBuf::from(d).join("gui-prefs.ron")),
        ..Default::default()
    };
    eframe::run_native(
        // See [`STORAGE_KEY`]: a storage key, not a display name. The
        // adoption resolves which spelling answers THIS launch — the new one,
        // unless a failed migration keeps the legacy directory in use.
        adopt_pre_rename_prefs(),
        opts,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx); // embedded symbol subsets + system CJK
            // Dark before prefs are readable; AutoShadeApp::new re-installs the
            // saved choice one call later (same shape as the greeting string).
            install_theme(&cc.egui_ctx, ThemePref::Dark);
            Ok(Box::new(AutoShadeApp::new(cc))) // restores prefs + last library
        }),
    )
}

#[cfg(test)]
mod tests;
