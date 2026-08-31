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
use std::path::PathBuf;
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
/// Windows keeps the pre-rename spelling. Renaming it would silently abandon
/// every existing user's window geometry, last library, view mode, export
/// options and theme, with nothing to say so — the same class of loss the
/// develop-store adoption exists to prevent, and the same reason the installer
/// keeps its AppId. Moving those prefs is a follow-up with its own migration;
/// renaming the key on its own is only a reset.
///
/// macOS is the exception, and only because it has nothing to lose: this is its
/// first release, so there are no prefs to abandon — while KEEPING the old
/// spelling there would cost something real. eframe derives
/// `Library/Application Support/<key>/data/app.ron`, so "Autoshop" would put a
/// folder of that name beside the develop store: the very name
/// `store::ADOPT_PRE_RENAME` refuses to touch, and on case-insensitive APFS
/// indistinguishable from the pre-rename store directory. One name, chosen
/// once, before any Mac user has either.
#[cfg(target_os = "macos")]
const STORAGE_KEY: &str = "AutoShade";
#[cfg(not(target_os = "macos"))]
const STORAGE_KEY: &str = "Autoshop";

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
        // See [`STORAGE_KEY`]: a storage key, not a display name, and why it
        // keeps the pre-rename spelling everywhere it has users.
        STORAGE_KEY,
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
