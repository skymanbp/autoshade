// Release builds run WITHOUT a console window — a GUI app shouldn't flash a
// terminal on launch. Debug keeps the console so panics/logs stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Autoshop — native desktop GUI (egui/eframe).
//!
//! A real native window (no localhost server, no webview): it links the
//! `autoshop` engine library and calls `decode` / `render` / `pipeline` directly
//! in-process. Open a RAW or image, develop it with live before/after, run the
//! AI auto-develop, and export — all from one window.
//!
//! Build/run: `cargo run --release --features gui --bin autoshop-gui`

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

// NOTE: `MaskRole` is addressed only by method here (`m.role.en_name()` in the
// mask row), never named as a type, so it is intentionally NOT imported — the
// enum lives in recipe.rs and is set by the engine, not constructed in the GUI.
use autoshop::recipe::{ColorGrade, CurvePoint, EditRecipe, Hsl, MaskGeometry, RangeMask};
use image::GenericImageView;

// Native-GUI i18n: English is the skeleton/key, Chinese is a single overlay
// table with English fallback. `tr`/`trf` are called with the English literal;
// see i18n.rs. (Private submodule — enabled by `autobins = false` in Cargo.toml.)
mod i18n;
use i18n::{tr, trf, Lang};

mod model;
mod persist;
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



/// Decode the embedded Autoshop icon for the window title bar / taskbar.
/// See main(): the last-crash report lands at `<store root>/panic.log`,
/// and (Windows) a native message box points the user at it.
fn install_panic_reporter() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The claim must match the outcome (review R12-10): on a first
        // launch the store root may not exist yet, and a failed write must
        // not direct the user at a report that is not there.
        let root = autoshop::store::store_root();
        let _ = std::fs::create_dir_all(&root);
        let log = root.join("panic.log");
        let wrote = std::fs::write(&log, format!("Autoshop crashed: {info}\n").as_bytes()).is_ok();
        #[cfg(windows)]
        message_box(&if wrote {
            format!(
                "Autoshop hit an internal error and must close.\n\n{info}\n\nA report was written to:\n{}",
                log.display()
            )
        } else {
            format!(
                "Autoshop hit an internal error and must close.\n\n{info}\n\n(a report could NOT be written to {})",
                log.display()
            )
        });
        #[cfg(not(windows))]
        let _ = wrote;
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
    let title: Vec<u16> = "Autoshop".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated buffers, live across the synchronous call.
    unsafe { message_box_w(0, wide.as_ptr(), title.as_ptr(), MB_OK_ICONERROR) };
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
            .with_title("Autoshop")
            .with_icon(std::sync::Arc::new(app_icon())),
        // The develop STORE already follows AUTOSHOP_DATA_DIR; the eframe
        // prefs file (last library, theme, window geometry) did not, so a
        // sandboxed E2E launch read — and on exit rewrote — the REAL user's
        // prefs, and its window opened onto their actual photo library. One
        // sandbox variable must mean the whole app is sandboxed.
        persistence_path: std::env::var_os("AUTOSHOP_DATA_DIR")
            .map(|d| std::path::PathBuf::from(d).join("gui-prefs.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "Autoshop",
        opts,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx); // embedded symbol subsets + system CJK
            // Dark before prefs are readable; AutoshopApp::new re-installs the
            // saved choice one call later (same shape as the greeting string).
            install_theme(&cc.egui_ctx, ThemePref::Dark);
            Ok(Box::new(AutoshopApp::new(cc))) // restores prefs + last library
        }),
    )
}

#[cfg(test)]
mod tests;
