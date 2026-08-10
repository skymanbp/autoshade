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
use egui::load::SizedTexture;

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
use model::*;
use persist::*;
use theme::*;
use util::*;

// the user crop AFTER the geometric chain, so the crop tool needs no mapping.
// All maps are the identity when both controls are zero.

struct AutoshopApp {
    src_path: Option<PathBuf>,
    // The ACTIVE variant's base pixels — shared with the preview worker by Arc,
    // so dispatching a 4096px develop is O(1), not a 50+ MB UI-thread deep copy.
    base_preview: Option<Arc<image::DynamicImage>>,
    // The pristine source neutral (RAW develop / loaded image), decoded once
    // per open. Source-based variants share this same allocation.
    source_preview: Option<Arc<image::DynamicImage>>,
    before_tex: Option<egui::TextureHandle>,
    after_tex: Option<egui::TextureHandle>,
    recipe: EditRecipe,
    dirty: bool, // recipe changed → queue the latest preview state
    // --- asynchronous develop scheduler (single in-flight, latest wins) ---
    develop_inflight: bool,
    develop_count: u64, // accepted frames; regression counter (latest-wins)
    status: String,
    busy: bool, // an analyze/export thread is running
    rx: Option<Receiver<Msg>>,
    tx: Sender<Msg>,
    verdict: Option<String>,
    rationale: String,
    style_strength: f32,
    hsl_tab: usize, // Color Mixer property tab: 0=Hue 1=Saturation 2=Luminance
    grade_region: usize,
    guidance: String, // free-text direction for the AI ("warmer, moodier")
    save_jpeg: bool,  // export/download as JPEG instead of 16-bit TIFF
    // --- undo / redo (a drag is one step, committed on release). Each step
    // carries the recipe AND the active variant's pixel identity (base Arc +
    // origin), so a baked pixel retouch (heal / clone / generative fill) is
    // one undoable step instead of a point of no return. Arc clones share the
    // allocation — only a retouch introduces a new raster into history.
    committed: UndoStep,        // current history head (last committed state)
    undo_stack: Vec<UndoStep>,  // prior states, most recent last
    redo_stack: Vec<UndoStep>,  // states undone away (cleared on a new edit)
    // --- unsaved-edit protection ---
    // What the sidecar last held for the open photo (neutral when none): the
    // canvas differing from it is the "● unsaved" condition. Navigation stashes
    // an unsaved canvas per-path for THIS session so switching photos can never
    // silently destroy work; only Ctrl+S writes disk.
    saved_recipe: EditRecipe,
    // Mirror of the open photo's pixels.json (the saved baked-master path, if
    // any) so the per-frame ● indicator can compare pixel identity WITHOUT a
    // disk read per frame. Updated at open, save, analyze-save and clear; the
    // decision points (close guard, quit dialog, navigation stash) still read
    // the disk — the authority.
    pixels_on_disk: Option<PathBuf>,
    /// Mirror of the photo's persisted `variants.json` (resolved form), the
    /// baseline the background-variant dirty test compares the live strip
    /// against. `None` = nothing persisted (the trivial single-card case).
    /// Updated at open and on every strip persist; comparing background
    /// variants against the ACTIVE canvas's `saved_recipe`/`pixels_on_disk`
    /// instead (the pre-v0.22 rule) made any two-origin strip permanently
    /// "unsaved" — the quit dialog then re-armed forever.
    saved_strip: Option<autoshop::store::VariantsRecord>,
    nav_stash: HashMap<PathBuf, StashEntry>,
    /// Master decodes in flight, keyed (photo, origin): repeat clicks into a
    /// still-decoding cold card must coalesce, not stack a fresh full-res
    /// decode thread per click.
    master_loads: std::collections::HashSet<(PathBuf, PathBuf)>,
    /// The open could not READ the saved develop (damaged file, or busy in
    /// another Autoshop process): the canvas baseline is not that save, so
    /// the next explicit save must version what it overwrites (save_xmp runs
    /// the backup gate while this stands).
    open_unresolved: bool,
    // Paste-to-selected wrote the open photo's store: (path, exact recipe
    // written) so Msg::Pasted can advance the ● baseline on full success.
    pasted_open: Option<(PathBuf, EditRecipe)>,
    // The OPEN photo's fresh camera-matched base-look knots (computed by the
    // open worker, empty for non-RAW / no preview). Reset re-stamps these so
    // "reset" always means the fresh-open look — including on a photo whose
    // legacy save carries no curve and therefore opened dark.
    photo_knots: Vec<[f32; 2]>,
    // This photo's in-camera lens profile (open worker) — the twin of
    // photo_knots: Reset re-stamps it, legacy toggles re-enable from it.
    photo_lens: autoshop::recipe::LensProfile,
    // This photo's as-shot WB anchor (open worker) — the third calibration
    // half: Reset re-stamps it, the Temp slider and eyedropper anchor on it.
    photo_as_shot: Option<(f32, f32)>,
    // The base_curve baked into `before_tex`. The Before pane mirrors the
    // canvas recipe's calibration (Reset / paste / restore can change it), so
    // a mismatch triggers a cheap rebuild in `update`.
    before_curve: Vec<[f32; 2]>,
    // The title-bar ✕ was intercepted with unsaved edits: show the in-app
    // save-all / discard / cancel layer until the user decides.
    confirm_quit: bool,
    // Local buffer for the selected mask's name TextEdit:
    // (mask index, name at seed time, edited text). Committed on focus loss so
    // a rename is ONE undo step, not one per key; the seed-time name lets a
    // pending rename commit safely when the selection jumps rows (commit only
    // while masks[i].name still equals the seed — index shifts can't misfire).
    mask_name_buf: Option<(usize, String, String)>,
    // Arrow-key navigation: gallery index to scroll into view next frame.
    gallery_scroll_to: Option<usize>,
    // --- settings / denoise ---
    save_denoise: bool,     // run SCUNet AI denoise before the full-res render
    zoned_fit: bool,        // 反推 adds a sky-to-sky zoned correction (bitmap mask)
    show_settings: bool,    // the Settings window is open
    show_shortcuts: bool,   // the keyboard cheat-sheet window is open (F1 / ? / ⌨)
    // Tab hides both side panels (the LR grammar) for an edge-to-edge canvas.
    // Session-only on purpose: relaunching with an invisible library reads as
    // a broken app, so the state never persists.
    panels_hidden: bool,
    // egui derives Tab's focus-traversal direction from raw input BEFORE
    // update() runs, so consuming the key can't stop the first widget from
    // taking focus this frame — which would also kill every focused-none
    // shortcut. Cleared on the NEXT frame instead.
    defocus_next: bool,
    settings: SettingsForm, // editable buffers for that window
    lang: Lang,             // UI language: English skeleton / Chinese overlay (i18n.rs)
    theme: ThemePref,       // dark/light chrome — applied via install_theme, kept in Prefs
    // --- library / gallery ---
    gallery: Vec<PathBuf>,          // sources in the working folder (sorted)
    gallery_dir: Option<PathBuf>,   // the working folder
    gallery_gen: u64,               // bumped on every folder load (thumb invalidation)
    selected: Option<usize>,        // index of the open gallery photo (for highlight)
    thumbs: HashMap<usize, egui::TextureHandle>, // decoded thumbnails by index
    thumb_requested: HashSet<usize>,             // indices already queued/decoded
    /// Decode attempts that FAILED, per gallery index. Bounds the retry: see
    /// `request_thumb`. Not a cache — it holds no pixels, only a count.
    thumb_fail: std::collections::HashMap<usize, u8>,
    thumb_inflight: usize,                       // live thumbnail-decode threads
    edited_badge: HashMap<usize, bool>,          // cached "● edited" sidecar stat per index
    // Decoded-base LRU (recent last): base pixels + the photo's base-look knots.
    base_cache: Vec<(BaseCacheKey, OpenedBase)>,
    // --- region box-select (local-edit target on the After image) ---
    region: Option<[f32; 4]>,                      // normalized [left, top, right, bottom]
    region_drag: Option<(egui::Pos2, egui::Pos2)>, // transient drag (start, current) in screen px
    // What a lone click just cleared, kept for one double-click window: the
    // second release of a Fit↔1:1 double-click restores it — the pair's
    // FIRST release already ran the plain-click clear (M18).
    region_restore: Option<([f32; 4], Instant)>,
    // --- retouch: mask painting + generative fill + heal ---
    paint_mode: bool,                      // brush-paint a mask (pauses box-select)
    brush: f32,                            // brush radius in After-image display px
    mask_paint: Option<image::RgbaImage>,  // painted overlay (red where painted), at preview res
    mask_tex: Option<egui::TextureHandle>, // overlay texture
    mask_dirty: bool,                      // re-upload the overlay
    mask_tex_xform: (f32, f32, bool), // (straighten, distortion, profile geometry on) at build
    // Union of the brush segments painted since the last texture upload
    // (canvas px, [x0,y0,x1,y1] half-open). With no geometry active, only
    // this sub-rectangle is uploaded (set_partial) — brushing used to clone
    // + re-upload the WHOLE canvas on every pointer move.
    mask_dirty_rect: Option<[u32; 4]>,
    mask_tex_built: Instant, // last full rebuild — throttles mid-stroke geometry remaps
    paint_last: Option<(f32, f32)>,        // last brush point in mask px (line fill)
    /// A LOCAL-ADJUSTMENT brush session rides the same paint canvas as
    /// fill/heal: `(target, erase)` — target `None` = the strokes become a
    /// NEW Bitmap mask on Apply; `Some(i)` = they edit mask `i`'s raster
    /// (seeded below). While `Some`, `paint_mode` is also true so every
    /// existing paint affordance (dispatch, cursor, Esc, `[`/`]`) works.
    mask_brush: Option<(Option<usize>, bool)>,
    /// The brush session's WEIGHT buffer — greyscale source of truth the
    /// Apply bakes to a raster. The RGBA canvas is only its display: seeding
    /// an existing raster through the canvas' 8-bit alpha and back would
    /// drift soft edges by a rounding step per edit cycle.
    mask_brush_gray: Option<image::GrayImage>,
    fill_prompt: String,                   // generative-fill instruction
    fill_quality: usize,                   // 0=high 1=medium 2=low
    fill_fullres: bool,                    // composite onto the full-res develop
    heal_fullres: bool,                    // heal the full-res develop
    denoise_fullres: bool,                 // AI-denoise the full-sensor develop (slow)
    reimagine_prompt: String,              // whole-image restyle prompt (its own entry)
    // --- production niceties ---
    view_mode: ViewMode,                   // side-by-side vs after-only (hold B = compare)
    toasts: Vec<Toast>,                    // transient corner notifications
    histogram: Option<Vec<[f32; 4]>>,      // live RGB+luma histogram of the After preview
    last_title: String,                    // window title cache (send only on change)
    // --- diagnostic view layers (UX batch) ---
    show_mask_overlay: bool,               // translucent red coverage of the selected mask (O)
    mask_overlay_tex: Option<egui::TextureHandle>,
    overlay_stale: bool,                   // check/rebuild coverage next frame
    overlay_key: Option<OverlayKey>,       // skips work when coverage is unchanged
    hover_mask: Option<usize>,             // mask row under the cursor — previews its coverage
    batch_progress: Option<(usize, usize)>, // (done, total) while a batch render runs
    // Cached masks-cleared develop the coverage's range weights are judged
    // on — reused while the global (non-mask) recipe is unchanged, so a
    // mask-slider drag rebuilds only the coverage map, not a second develop.
    overlay_ref: Option<(EditRecipe, image::DynamicImage)>,
    overlay_build_count: u64,              // actual coverage rebuilds (tests/diagnostics)
    show_clipping: bool,                   // clipping warnings: red blown / blue crushed (J)
    last_rgb: Option<image::RgbImage>,     // last accepted frame's pixels (instant clip toggle)
    clip_tex: Option<egui::TextureHandle>,
    // --- zoom / pan (per-photo, reset on open) ---
    zoom: f32,                             // 1.0 = fit; up to 12×
    pan: egui::Vec2,                       // visible-window centre in crop-window coords
    // --- crop tool ---
    crop_mode: bool,                       // the crop overlay is active on the After image
    crop_aspect: usize,                    // index into CROP_ASPECTS
    crop_aspect_pending: bool,             // preset changed — re-derive the box in handle_crop
    crop_grid: u8,                         // crop guide: 0 thirds · 1 golden · 2 off (O cycles, LR)
    before_latch: bool,                    // \ latched the Before view (toggled twin of hold-B)
    // (handle, drag start, crop at start, straighten° at start). Handles:
    // 0-3 corners, 4 move-inside, 5-8 edge midpoints (T/B/L/R), 9 = drag
    // OUTSIDE the box → rotate-straighten (the LR gesture).
    crop_drag: Option<(u8, egui::Pos2, [f32; 4], f32)>,
    mask_drag: Option<(u8, (f32, f32))>, // on-image mask knob drag: (handle, last pos in ORIG norm)
    // --- manual local adjustments (masks) ---
    sel_mask: Option<usize>,               // selected recipe.masks index (overlay + sliders)
    placing_mask: Option<(MaskKind, PlaceTarget)>, // next image drag defines a mask / redraw / component
    /// Which of the selected mask's COMPONENTS the canvas tools target:
    /// `None` = the base geometry. Reset whenever the mask selection moves.
    sel_component: Option<usize>,
    /// The combine mode the panel's "add shape" buttons will arm next.
    component_mode: autoshop::recipe::MaskCombine,
    place_start: Option<(f32, f32)>,       // placement drag origin, full-frame normalized
    // --- tone-curve editor ---
    curve_channel: usize,                  // CURVE_CHANNELS index: 0=master 1=R 2=G 3=B
    curve_drag: Option<usize>,             // control point being dragged (active channel)
    #[cfg(test)]
    curve_rect: Option<egui::Rect>,        // test seam: the editor square's last rect
    // --- batch recipe copy / paste ---
    multi_sel: HashSet<usize>,             // Ctrl+click gallery multi-selection
    copied: Option<EditRecipe>,            // the recipe "clipboard" (in-app only)
    // WHICH photo the clipboard came from: pasting back onto that photo may
    // keep its bitmap masks (the rasters are its own); every other target
    // gets them stripped.
    copied_from: Option<PathBuf>,
    paste_geometry: bool,                  // keep crop/straighten when pasting
    // --- WB eyedropper ---
    wb_picking: bool,                      // next image click samples a neutral point
    // --- colour-range sample (Range Mask) ---
    range_picking: Option<usize>,          // next image click keys masks[i]'s Color range
    // --- clone stamp ---
    clone_mode: bool,                      // brush paints the clone target; Alt+click = source
    clone_src: Option<(f32, f32)>,         // picked source point, original-frame normalized
    clone_fullres: bool,                   // clone on the full-res develop (RAW only)
    // --- cancellable retouch/generative workers ---
    // Cancel bumps `gen_epoch`; a worker's Retouched message carries the epoch
    // it was started under and is discarded on mismatch (the canvas is never
    // mutated by an abandoned task). `gen_cancel` is Some while such a worker
    // is in flight — it arms the status-bar ✕ and, for streaming generative
    // calls, is polled between events to stop the download itself.
    gen_epoch: u64,
    gen_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// An `Analyze` worker is still on the wire, cancelled or not.
    ///
    /// Cancelling an analyze is ABANDON-only — `produce_recipe` takes no
    /// cancel flag, so nothing stops the HTTP call — and ✕ clears `busy`,
    /// whose only other job was to gate `start_analyze`. Without this the ✕
    /// became a "start another one" button: each press spawned a fresh
    /// high-detail vision request that ran to its stall deadline and was
    /// BILLED, N deep. The flag is cleared when the worker's message lands,
    /// whatever its epoch.
    analyze_inflight: bool,
    // --- variants (版本/变体): parallel renditions of the open photo ---
    // Original + any AI-generated / reverse-fitted versions. The active one
    // drives the sliders, histogram and canvas; switching is lossless (each
    // remembers its own base + recipe), so an AI develop no longer "reverts"
    // when you touch a slider — you're editing that variant's own base.
    variants: Vec<Variant>,
    /// The user answered "Discard & quit". The close guard must then stop
    /// re-testing unsaved state: discarding by definition leaves the state
    /// dirty, so without this the guard cancelled the close and re-raised
    /// the dialog every frame and the app could not be quit at all. (The
    /// SAVE path needs no such flag since v0.22 — Save-all persists each
    /// photo's whole strip to variants.json, so the way-out re-check finds
    /// the state genuinely clean.)
    discard_requested: bool,
    active: usize,                         // index into `variants` (always valid once a photo is open)
    keep_recipe: bool,                     // one-shot: next Opened keeps recipe/variants (preview-res re-decode)
    open_in_flight: bool,                  // mid-open window: src_path re-pointed, Opened not yet processed
    open_same_path: bool,                  // the in-flight open targets the photo already open (recorded by open_path — keep_recipe is a REQUEST, not a fact)
    edge_before_flight: Option<u32>,       // armed by the px combo with its re-decode flight; a FAILED keep-flight reverts preview_edge to it (the canvas kept the old edge)
    // --- export pipeline (gap batch F + D2) ---
    exp_long_edge: u32,                    // resize long edge in px; 0 = full resolution
    exp_sharpen: f32,                      // output sharpening 0..100, post-resize
    exp_quality: f32,                      // JPEG quality 1..100 (f32 for the shared slider)
    exp_space: u8,                         // delivery color space: 0 sRGB / 1 Display P3 / 2 Adobe RGB
    // --- preview resolution (gap batch E) ---
    preview_edge: u32,                     // working-preview long edge: 1280 fluid / 2560 / 4096 detail
    // --- recipe versions (gap batch G): ./out/<stem>.v<N>.recipe.json ---
    versions: Vec<u32>,                    // snapshot numbers found for the open photo (sorted)
}

impl Default for AutoshopApp {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            src_path: None,
            base_preview: None,
            source_preview: None,
            before_tex: None,
            after_tex: None,
            recipe: EditRecipe::default(),
            dirty: false,
            develop_inflight: false,
            develop_count: 0,
            status: "Open a photo, or open a folder to browse your library.".into(),
            busy: false,
            rx: Some(rx),
            tx,
            verdict: None,
            rationale: String::new(),
            style_strength: 0.30,
            hsl_tab: 0,
            grade_region: 0,
            guidance: String::new(),
            save_jpeg: false,
            committed: UndoStep::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            save_denoise: false,
            zoned_fit: true,
            show_settings: false,
            show_shortcuts: false,
            panels_hidden: false,
            defocus_next: false,
            settings: SettingsForm::default(),
            // English is the default / skeleton language; a persisted pref
            // (restored in `new`) overrides this on launch.
            lang: Lang::En,
            theme: ThemePref::Dark,
            gallery: Vec::new(),
            gallery_dir: None,
            gallery_gen: 0,
            selected: None,
            thumbs: HashMap::new(),
            thumb_requested: HashSet::new(),
            thumb_fail: std::collections::HashMap::new(),
            thumb_inflight: 0,
            edited_badge: HashMap::new(),
            base_cache: Vec::new(),
            saved_recipe: EditRecipe::default(),
            pixels_on_disk: None,
            saved_strip: None,
            nav_stash: HashMap::new(),
            master_loads: std::collections::HashSet::new(),
            open_unresolved: false,
            pasted_open: None,
            photo_knots: Vec::new(),
            photo_lens: Default::default(),
            photo_as_shot: None,
            before_curve: Vec::new(),
            confirm_quit: false,
            mask_name_buf: None,
            gallery_scroll_to: None,
            region: None,
            region_drag: None,
            region_restore: None,
            paint_mode: false,
            brush: 30.0,
            mask_paint: None,
            mask_tex: None,
            mask_dirty_rect: None,
            mask_tex_built: Instant::now(),
            mask_dirty: false,
            mask_tex_xform: (0.0, 0.0, false),
            paint_last: None,
            fill_prompt: String::new(),
            fill_quality: 0,
            fill_fullres: false,
            heal_fullres: false,
            denoise_fullres: false,
            reimagine_prompt: String::new(),
            view_mode: ViewMode::SideBySide,
            toasts: Vec::new(),
            histogram: None,
            last_title: String::new(),
            zoom: 1.0,
            pan: egui::vec2(0.5, 0.5),
            crop_mode: false,
            crop_aspect: 0,
            crop_aspect_pending: false,
            crop_grid: 0,
            before_latch: false,
            crop_drag: None,
            mask_drag: None,
            sel_mask: None,
            placing_mask: None,
            sel_component: None,
            component_mode: autoshop::recipe::MaskCombine::Add,
            mask_brush: None,
            mask_brush_gray: None,
            place_start: None,
            curve_channel: 0,
            curve_drag: None,
            #[cfg(test)]
            curve_rect: None,
            multi_sel: HashSet::new(),
            copied: None,
            copied_from: None,
            paste_geometry: false,
            wb_picking: false,
            range_picking: None,
            clone_mode: false,
            clone_src: None,
            clone_fullres: false,
            gen_epoch: 0,
            gen_cancel: None,
            analyze_inflight: false,
            variants: Vec::new(),
            discard_requested: false,
            active: 0,
            keep_recipe: false,
            open_in_flight: false,
            open_same_path: false,
            edge_before_flight: None,
            exp_long_edge: 0,
            exp_sharpen: 0.0,
            exp_quality: 95.0,
            exp_space: 0,
            preview_edge: PREVIEW_EDGE,
            versions: Vec::new(),
            show_mask_overlay: true,
            mask_overlay_tex: None,
            overlay_stale: false,
            overlay_key: None,
            overlay_ref: None,
            overlay_build_count: 0,
            hover_mask: None,
            batch_progress: None,
            show_clipping: false,
            last_rgb: None,
            clip_tex: None,
        }
    }
}

impl AutoshopApp {
    /// Restore persisted prefs (last folder, view mode, export options) and
    /// re-open the library the user was browsing. Window geometry itself is
    /// restored by eframe's own persistence layer.
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default();
        if let Some(prefs) =
            cc.storage.and_then(|s| eframe::get_value::<Prefs>(s, eframe::APP_KEY))
        {
            app.style_strength = prefs.style_strength.clamp(0.0, 1.0);
            app.save_jpeg = prefs.save_jpeg;
            app.save_denoise = prefs.save_denoise;
            app.zoned_fit = prefs.zoned_fit;
            app.view_mode = prefs.view_mode;
            app.exp_long_edge = prefs.exp_long_edge;
            app.exp_sharpen = prefs.exp_sharpen.clamp(0.0, 100.0);
            app.exp_quality = prefs.exp_quality.clamp(1.0, 100.0);
            app.show_clipping = prefs.show_clipping;
            // Restore the UI language (an older save without this key decoded to
            // `Lang::En` via `#[serde(default)]`, so this is always valid).
            app.lang = prefs.lang;
            // Restore the theme and re-apply it — main() installed the Dark
            // default before prefs were readable (same shape as the greeting).
            app.theme = prefs.theme;
            install_theme(&cc.egui_ctx, app.theme);
            // The greeting was set by default() BEFORE the language was known —
            // re-issue it, or a Chinese install opens with one English line.
            app.status =
                tr(app.lang, "Open a photo, or open a folder to browse your library.").into();
            // Only known color spaces — an out-of-range pref falls back to sRGB.
            if prefs.exp_space <= 2 {
                app.exp_space = prefs.exp_space;
            }
            // Only the known steps — a corrupt pref must not produce a 1-px
            // or 100-MP working preview.
            if [1280, 2560, 4096].contains(&prefs.preview_edge) {
                app.preview_edge = prefs.preview_edge;
            }
            if let Some(dir) = prefs.gallery_dir.filter(|d| d.is_dir()) {
                app.open_folder(dir);
            }
        }
        app
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        // An identical LIVE toast refreshes instead of duplicating: undoing
        // through a washed history region fires the same repair disclosure
        // once per healed step, and five copies evicted still-live ERROR
        // toasts from the ring. The refresh MOVES it to the back — it is
        // the newest content, and refreshing in place left it first in
        // line for eviction, the exact loss the dedup exists to prevent.
        if let Some(i) = self.toasts.iter().position(|t| t.text == text && t.kind == kind) {
            self.toasts.remove(i);
        }
        self.toasts.push(Toast { text, kind, born: Instant::now() });
        if self.toasts.len() > 5 {
            self.toasts.remove(0); // keep the stack readable
        }
    }

    /// A worker finished successfully: status line + a success toast, unbusy.
    fn done(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text.clone();
        self.toast(ToastKind::Success, text);
        self.busy = false;
    }

    /// A worker failed: status line + a lingering error toast, unbusy. A single
    /// status line is too easy to miss for a failed export or API call.
    fn fail(&mut self, what: &str, e: impl std::fmt::Display) {
        let text = format!("{what}: {e}");
        self.status = text.clone();
        self.toast(ToastKind::Error, text);
        self.busy = false;
    }

    /// Arm the cancellable-worker state for a retouch/generative task: a fresh
    /// cancel flag (this shows the status-bar ✕) and the epoch its result must
    /// present to be applied. Streaming generative workers also hand the flag
    /// to the lib so the download itself stops at the next event; local
    /// compute is abandon-only (its late result is discarded by epoch).
    fn arm_cancel(&mut self) -> (u64, Arc<std::sync::atomic::AtomicBool>) {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.gen_cancel = Some(flag.clone());
        (self.gen_epoch, flag)
    }

    /// The status-bar ✕: stop the running retouch/generative task. The UI
    /// unblocks NOW; the worker sees the flag at its next checkpoint and its
    /// late result arrives under a stale epoch — discarded, never applied.
    fn cancel_generative(&mut self) {
        if let Some(flag) = self.gen_cancel.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            self.gen_epoch = self.gen_epoch.wrapping_add(1);
            self.busy = false;
            // Honest about BOTH shapes. A generative worker polls the flag
            // between stream events and really does stop; `produce_recipe`
            // takes no flag at all, so an Analyze keeps running — and keeps
            // billing — until it answers or hits its deadline. Claiming a
            // checkpoint that does not exist for the path the user is most
            // likely cancelling is the kind of false status this project
            // treats as a defect.
            self.status = tr(
                self.lang,
                "cancelled — the app is free again and the late result is discarded; a generative call stops at its next checkpoint, while an AI analyze keeps running (and billing) until it finishes or times out",
            )
            .into();
        }
    }

    /// Spawn a worker whose PANIC still delivers a terminal message. Every
    /// worker's last act is sending the `Msg` that clears `busy` (or an
    /// inflight counter); a panic before that send unwinds the thread, the
    /// message never arrives, and the whole app soft-locks — every action
    /// gates on `!busy`, and only killing the process recovers. One decode
    /// panic inside `rawler`/`image` on a malformed file is enough. So: run
    /// the body under `catch_unwind` and synthesize the site's failure `Msg`
    /// from the panic payload. `AssertUnwindSafe` is sound here — all
    /// captured state is moved in and dropped on unwind; the UI only ever
    /// observes the channel message.
    fn spawn_worker(
        &self,
        body: impl FnOnce() -> Msg + Send + 'static,
        on_panic: impl FnOnce(anyhow::Error) -> Msg + Send + 'static,
    ) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let msg = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body))
                .unwrap_or_else(|p| {
                    let s = p
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into());
                    on_panic(anyhow::anyhow!("worker panicked: {s}"))
                });
            let _ = tx.send(msg);
        });
    }
}

impl AutoshopApp {
    fn open_path(&mut self, path: PathBuf) {
        if self.busy {
            // Defence-in-depth: since the px combo refuses BEFORE arming, no
            // live caller reaches this guard with keep_recipe set — but a
            // stale request here once misclassified a genuine cross-photo
            // open as a same-path keep-flight, and any future caller that
            // arms-then-calls gets the same protection instead of relying on
            // its own busy check. Said so, instead of claiming a live
            // scenario.
            self.keep_recipe = false;
            return;
        }
        // Flush a typed-but-uncommitted mask rename before the stash below
        // snapshots the recipe — a thumbnail click / Ctrl+O / arrow-key nav
        // with the name box focused silently stashed the OLD name (U10).
        self.commit_mask_name_buf();
        // …and the buffer itself dies with the photo: carried across, a
        // same-index mask on the NEXT photo skipped the reseed and the box
        // showed the previous photo's name text (M15).
        self.mask_name_buf = None;
        // The mid-open window marker (see confirm_quit_layer): src_path is
        // re-pointed below while recipe/saved_recipe still describe the old
        // photo; cleared in BOTH Msg::Opened arms. `open_same_path` is the
        // FACT the repair gates key on — keep_recipe is only a request, and
        // a stale request must never classify a flight.
        self.open_in_flight = true;
        self.open_same_path = self.src_path.as_ref() == Some(&path);
        // Leaving a photo with unsaved edits: stash the canvas for this
        // session so navigation can never silently destroy work (arrow keys /
        // thumbnail clicks used to drop the whole develop). Ctrl+S still owns
        // disk; returning to the photo restores the stash over the sidecar.
        // "Unsaved" covers PIXELS too: a baked retouch whose master isn't the
        // one recorded in the store's pixels.json would otherwise vanish from
        // the canvas on the way back even though its raster is on disk.
        // SAME-path reopens stash too (no `old != path` gate): clicking the
        // current thumbnail / re-picking the current file takes the fresh
        // branch below, which resets per-photo state and restores stash-then-
        // disk — without the entry, a dirty canvas was silently replaced by
        // the disk state. The stash entry is consumed by that same restore,
        // so a same-path reopen becomes "reload, preserving unsaved work".
        if let Some(old) = self.src_path.clone() {
            let origin = self.active_variant().and_then(|v| v.origin.clone());
            // Compared BOTH ways: a canvas that dropped its baked master
            // (undo back to source) is as unsaved as one that gained it —
            // without the stash, reopening would resurrect the disk pixels.
            // The WHOLE identity counts, generated flag included: the
            // classification decides whether calibration is stripped on
            // restore, so a flag drift is as unsaved as a path drift.
            let recorded = autoshop::store::read_pixel_source(&old);
            let live_generated = self.active_is_generated();
            let pixels_unsaved = !(same_master_opt(
                recorded.as_ref().map(|(p, _)| p.as_path()),
                origin.as_deref(),
            ) && recorded.as_ref().map(|(_, g)| *g)
                == origin.as_ref().map(|_| live_generated));
            // Background variants count too (H4): their unsaved work has no
            // sidecar to survive in, and the strip used to collapse to the
            // active canvas alone on navigation. THIS photo's strip only:
            // the cross-photo sum (inactive_dirty_variants) belongs to the
            // quit dialog, and using it here chain-stashed clean photos.
            let background_dirty = self.open_dirty_variants() > 0;
            if dirty_vs(&self.recipe, &self.saved_recipe) || pixels_unsaved || background_dirty
            {
                let others: Vec<StashedVariant> = self
                    .variants
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != self.active)
                    .map(|(_, v)| StashedVariant {
                        kind: v.kind,
                        recipe: v.recipe.clone(),
                        base: v.base.clone(),
                        origin: v.origin.clone(),
                    })
                    .collect();
                let active_pos = self.active.min(others.len());
                self.nav_stash.insert(
                    old,
                    StashEntry {
                        recipe: self.recipe.clone(),
                        base: self.active_variant().and_then(|v| v.base.clone()),
                        origin,
                        kind: self.active_variant().map_or(VariantKind::Original, |v| v.kind),
                        others,
                        active_pos,
                    },
                );
            }
        }
        let lang = self.lang;
        self.busy = true;
        // Every open starts resolved-until-proven-otherwise; the Opened
        // handler's Unreadable arm re-arms it for THIS photo when the read
        // fails.
        self.open_unresolved = false;
        self.src_path = Some(path.clone());
        // The version list describes the photo `src_path` NAMES, so it dies
        // with the old value of that field — not later, when the decode lands.
        // `refresh_versions` only runs from the completed `Msg::Opened`, and a
        // RAW decode is seconds; for that whole window the Versions panel
        // showed photo A's v1/v2/v3 with live buttons while every action
        // resolved `self.src_path`, which already pointed at photo B. Clicking
        // 🗑 on the stale list called `store::delete_version(B, n)` and
        // PERMANENTLY removed a snapshot of B — recipe and frozen mask
        // rasters, no backup, no undo — then refreshed the list from B so the
        // rows visibly changed and the status line confirmed a delete, hiding
        // what had happened. "＋ Save as version" was the mirror image: it
        // wrote A's canvas into B's develop as B's next version.
        self.versions.clear();
        self.status = trf(lang, "decoding {path} …", &[("path", &path.display().to_string())]);
        // Working-preview size is a user choice now (gap batch E): 1280 keeps
        // sliders fluid; 2560/4096 trade tick latency for real 1:1 detail when
        // checking focus / noise.
        let edge = self.preview_edge.clamp(640, 8192);
        // Decoded-base LRU: gallery culling flips between neighbours constantly,
        // and a hit skips the multi-second full demosaic entirely. Delivered
        // through the ordinary Msg::Opened channel so the handler stays the one
        // place that resets per-photo state.
        if let Some(hit) = self.cached_base(&path, edge) {
            let _ = self.tx.send(Msg::Opened(Box::new(Ok(hit))));
            return;
        }
        self.spawn_worker(
            move || {
                // Build a CLEAN preview base by developing the RAW sensor data
                // (downscaled), NOT the camera's already-baked 8-bit JPEG preview:
                // re-developing that double-processes it and amplifies its grain when
                // you push tone/clarity. Baked images (PNG/TIFF/JPEG) are their own
                // source. Demosaic is slow, so this runs off the UI thread.
                let res = (|| -> anyhow::Result<OpenedBase> {
                    // The LRU key, captured BEFORE any read (the curve-memo
                    // rule): filing the decode under completion-time state
                    // handed a mid-open replacement the previous content's
                    // pixels and knots under its own key, and a popup-driven
                    // px change mid-flight mislabelled the edge.
                    let src_ident = (edge, file_stamp(&path), pixel_identity(&path));
                    let (thumb, knots, lens, as_shot) = if autoshop::decode::is_raw(&path) {
                        // Identity BEFORE the read: the primed answer below
                        // is filed under the content it was computed FROM —
                        // stat'ing after the multi-second develop filed a
                        // mid-open replacement's NEW identity with the OLD
                        // content's answer.
                        let ident = autoshop::pipeline::curve_ident(&path);
                        // Developed AT the working edge (the cap runs before
                        // tone/geometry): opening a 61 MP RAW no longer keeps
                        // a full-resolution develop resident just to be
                        // thumbnailed — the knots estimate below is CDF
                        // statistics and reads the same at working res.
                        let full = autoshop::render::render_to_image(
                            &path,
                            &EditRecipe::default(),
                            None,
                            Some(edge),
                        )?;
                        // In-camera lens profile (Sony 0x70xx): one cheap TIFF
                        // parse, stamped all-available-on (the user default).
                        let lens = autoshop::pipeline::fresh_lens_profile(&path);
                        // As-shot WB anchor: one metadata-only decode (no
                        // demosaic) — the Temp slider then speaks absolute
                        // Kelvin for this photo.
                        let as_shot = autoshop::render::as_shot_wb(&path);
                        // Camera-matched base look: CDF-match the neutral
                        // develop against the camera's own rendition — with
                        // the profile VIGNETTE applied first: the camera JPEG
                        // already contains that correction, and matching the
                        // uncorrected neutral would bake the corner lift into
                        // the global curve a second time. Geometry moves
                        // pixels, not their histogram — skipped on purpose.
                        // A RAW with no embedded preview (or a decode hiccup)
                        // just opens without a base look — never a failure.
                        let knots = match autoshop::decode::embedded_preview(&path) {
                            Ok(Some(cam)) => {
                                let est = autoshop::render::estimation_base(&full, &lens);
                                match autoshop::render::camera_base_knots(&est, &cam) {
                                    // The ANSWER primes the repair memo: the
                                    // open already paid this develop, and
                                    // discarding the result made every later
                                    // repair site (Ctrl+Z, variant switch,
                                    // Ctrl+S) pay a second full decode on
                                    // the UI thread. Answers only — an
                                    // inability stays uncached so the next
                                    // reader retries.
                                    Some(k) => {
                                        autoshop::pipeline::prime_curve_memo(
                                            &path,
                                            ident,
                                            k.clone(),
                                        );
                                        k
                                    }
                                    // None = could not judge. For an OPEN
                                    // that is the same as no base look — the
                                    // repair path is where the distinction
                                    // decides whether a SAVED curve may be
                                    // replaced.
                                    None => Vec::new(),
                                }
                            }
                            _ => Vec::new(),
                        };
                        // render_to_image already developed AT the working edge
                        // — `full` IS the thumb. The unconditional
                        // `full.thumbnail(edge, edge)` here duplicated a
                        // ~45 MP frame at the 8192 edge just to copy it; the
                        // resize only runs in the defensive off-spec case.
                        let thumb = if full.width().max(full.height()) > edge {
                            full.thumbnail(edge, edge)
                        } else {
                            full
                        };
                        (thumb, knots, lens, as_shot)
                    } else {
                        (
                            autoshop::decode::load_image(&path)?.thumbnail(edge, edge),
                            Vec::new(),
                            Default::default(),
                            None,
                        )
                    };
                    // The saved develop's baked pixel master (in-place heal /
                    // clone / fill / denoise), when the store records one and
                    // it still resolves — decoded HERE so the UI thread never
                    // loads a 61 MP PNG. A missing/unreadable master degrades
                    // to the plain source open, never a failure.
                    let baked = autoshop::store::read_pixel_source(&path).and_then(
                        |(origin, generated)| {
                            let img = match autoshop::decode::load_image(&origin) {
                                Ok(i) => i,
                                Err(e) => {
                                    // Disclosed, not silent: "the canvas came
                                    // back un-retouched" must have a traceable
                                    // cause in the log.
                                    eprintln!(
                                        "⚠ baked master {} failed to decode ({e}) — opening the un-retouched source",
                                        origin.display()
                                    );
                                    return None;
                                }
                            };
                            Some((Arc::new(img.thumbnail(edge, edge)), origin, generated))
                        },
                    );
                    // Arc once here so every downstream sharer (variants, the
                    // preview worker) is an O(1) refcount bump, not a deep copy.
                    Ok((Arc::new(thumb), knots, lens, as_shot, baked, src_ident))
                })();
                Msg::Opened(Box::new(res))
            },
            |e| Msg::Opened(Box::new(Err(e))),
        );
    }

    /// Decoded-base LRU lookup (most-recent entry kept last). Missing metadata
    /// (mtime `None`) matches itself, so the cache still works where mtime is
    /// unavailable — it just loses the staleness guard there.
    fn cached_base(&mut self, path: &std::path::Path, edge: u32) -> Option<OpenedBase> {
        let mtime = file_stamp(path);
        let pixels = pixel_identity(path);
        let pos = self.base_cache.iter().position(|((p, e, t, px), _)| {
            p == path && *e == edge && *t == mtime && *px == pixels
        })?;
        let entry = self.base_cache.remove(pos);
        let hit = entry.1.clone();
        self.base_cache.push(entry);
        Some(hit)
    }

    /// Remember a freshly decoded base. The KEY comes from the worker's
    /// pre-read capture (`opened.5`), never from a stat or `preview_edge`
    /// here: the decode takes seconds, an egui dropdown popup outlives
    /// add_enabled_ui(!busy) (so the px combo CAN change mid-flight), and
    /// filing the pixels under completion-time state handed a mid-open
    /// replacement the previous content's pixels AND knots under its own
    /// key — the curve memo's identity-before-read rule, applied to its
    /// missed twin.
    fn remember_base(&mut self, path: &std::path::Path, opened: &OpenedBase) {
        let (edge, mtime, pixels) = opened.5.clone();
        self.base_cache.retain(|((p, e, _, _), _)| !(p == path && *e == edge));
        self.base_cache.push(((path.to_path_buf(), edge, mtime, pixels), opened.clone()));
        if self.base_cache.len() > BASE_CACHE_CAP {
            self.base_cache.remove(0); // least-recent first
        }
    }

    /// Drop the open photo's decoded-base cache entries. Called whenever its
    /// BAKED pixel identity changes (a retouch lands, a save writes/clears
    /// pixels.json) — the source mtime doesn't move then, so the mtime guard
    /// alone would happily serve a hit with stale baked pixels.
    fn forget_open_base(&mut self) {
        if let Some(p) = self.src_path.clone() {
            self.base_cache.retain(|((q, _, _, _), _)| q != &p);
        }
    }

    /// How many BACKGROUND variants hold work that quitting would discard.
    ///
    /// Dirty means: the live strip minus the active card differs from the
    /// photo's persisted `variants.json` mirror ([`Self::saved_strip`]) — in
    /// kind, recipe, raster origin, count, or in the active card's own
    /// identity (kind/position, which the record also persists). A `None`
    /// mirror means nothing is persisted, so every background card is
    /// unsaved work. The OPEN photo's strip only — the per-photo half of
    /// [`Self::inactive_dirty_variants`]; the stash decision in `open_path`
    /// keys on THIS (summing other photos' stashed variants into the gate
    /// chain-stashed every clean photo the user merely visited).
    ///
    /// The pre-v0.22 rule compared each background variant against the
    /// ACTIVE canvas's `saved_recipe`/`pixels_on_disk`. Those describe a
    /// DIFFERENT variant, so any strip whose cards disagree in origin (every
    /// generate→fit flow) counted dirty forever, no save could clear it, and
    /// the quit dialog re-armed until 「Discard」.
    fn open_dirty_variants(&self) -> usize {
        let live: Vec<&Variant> = self
            .variants
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active)
            .map(|(_, v)| v)
            .collect();
        let Some(rec) = &self.saved_strip else {
            return live.len();
        };
        let mut n = 0usize;
        for (i, v) in live.iter().enumerate() {
            let matches = rec.others.get(i).is_some_and(|e| {
                VariantKind::from_store_str(&e.kind) == Some(v.kind)
                    && !dirty_vs(&v.recipe, &e.recipe)
                    && same_master_opt(v.origin.as_deref(), e.origin.as_deref())
            });
            if !matches {
                n += 1;
            }
        }
        // Cards persisted but no longer live: the deletion is unsaved too —
        // quitting now would resurrect them on the next open.
        n += rec.others.len().saturating_sub(live.len());
        // Active-card identity drift: a changed kind or position reopens as
        // a different strip even when every background card matches.
        let ak = self.active_variant().map_or(VariantKind::Original, |v| v.kind);
        if VariantKind::from_store_str(&rec.active_kind) != Some(ak)
            || rec.active_pos != self.active
        {
            n = n.max(1);
        }
        n
    }

    fn inactive_dirty_variants(&self) -> usize {
        // Photos NAVIGATED AWAY FROM count too. Since batch 47 a photo is
        // stashed when only a BACKGROUND variant is dirty, so the quit dialog
        // lists it. Save-all persists each stashed photo's strip record along
        // with its active canvas (v0.22), so these are savable — the count
        // deliberately stays conservative (every stashed background variant,
        // not only those differing from their photo's on-disk record: that
        // comparison would cost a disk read per stash entry per check).
        let stashed: usize = self.nav_stash.values().map(|st| st.others.len()).sum();
        self.open_dirty_variants() + stashed
    }

    /// The live strip as a persistable [`store::VariantsRecord`]: `None` when
    /// the strip is trivial (one Original card — recipe.json + pixels.json
    /// already say everything, and a record would be pure sidecar noise).
    fn current_strip_record(&self) -> Option<autoshop::store::VariantsRecord> {
        let ak = self.active_variant().map_or(VariantKind::Original, |v| v.kind);
        if self.variants.len() <= 1 && ak == VariantKind::Original {
            return None;
        }
        Some(autoshop::store::VariantsRecord {
            v: 1,
            active_kind: ak.store_str().to_string(),
            active_pos: self.active,
            others: self
                .variants
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != self.active)
                .map(|(_, v)| autoshop::store::VariantEntry {
                    kind: v.kind.store_str().to_string(),
                    recipe: v.recipe.clone(),
                    origin: v.origin.clone(),
                })
                .collect(),
        })
    }

    /// Persist the open photo's strip beside recipe.json/pixels.json and
    /// advance the mirror. Trivial strip ⇒ clear (no noise file). An error
    /// leaves the mirror untouched, so the unsaved protection stays armed —
    /// callers surface the error, never swallow it.
    fn persist_strip(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        match self.current_strip_record() {
            Some(rec) => {
                autoshop::store::write_variants(path, &rec)?;
                self.saved_strip = Some(rec);
            }
            None => {
                autoshop::store::clear_variants(path)?;
                self.saved_strip = None;
            }
        }
        Ok(())
    }

    /// The active variant, if a photo is open.
    fn active_variant(&self) -> Option<&Variant> {
        self.variants.get(self.active)
    }

    /// The edge a BAKE renders at: the canvas's own resolution, not the
    /// global preference.
    ///
    /// They part company whenever the canvas is a baked raster the preference
    /// could not re-decode — a background retouched / Generated variant keeps
    /// its own resolution because switching variants cannot re-decode a
    /// master, and an undo can restore a superseded one. Baking at the
    /// preference there installed a NEW-edge raster under an OLD-edge canvas
    /// and the frame jumped resolution mid-retouch: the same disagreement the
    /// refused resolution switch exists to prevent, one door along.
    /// `preview_edge` remains the DECODE edge (`open_path`) — a source-based
    /// canvas is developed from a source decoded AT it, so those two agree by
    /// construction.
    fn canvas_edge(&self) -> u32 {
        self.base_preview
            .as_ref()
            .map(|b| b.width().max(b.height()))
            .unwrap_or(self.preview_edge)
            .clamp(640, 8192)
    }

    /// The resolution a canvas must DISCLOSE, when it holds baked pixels the
    /// preview preference cannot reach. `None` when there is nothing to say.
    ///
    /// FOUR doors install such a canvas: the strip click, an undo to a
    /// SUPERSEDED master (the re-decode repoints only the Arc the canvas
    /// held), the navigation stash restore (which reinstalls the raster the
    /// photo was left with while this open decoded the source at the current
    /// preference), and deleting the active variant (which re-anchors onto a
    /// background one).
    fn baked_canvas_edge(&self) -> Option<u32> {
        // A canvas has TWO legitimate resolutions, and measuring against
        // either one alone produced a false claim:
        //  · the PREFERENCE — a recorded master is re-decoded with an
        //    UNGUARDED `thumbnail`, which upscales, so a re-decode lands
        //    exactly there whatever the sensor is;
        //  · what the source DELIVERS — the RAW decode is guarded against
        //    upscaling, so a fresh bake on a sensor smaller than the
        //    preference lands there instead, and is perfectly current.
        // Only a THIRD value is stale: a raster baked under an older
        // preference, or a superseded master an undo brought back. Measuring
        // against the preference alone called every fresh sub-preference bake
        // stale; against the delivered edge alone it called every REOPENED
        // sub-preference master stale ("not the preview preference" while the
        // preference was exactly that). It takes both.
        let delivered = self
            .source_preview
            .as_ref()
            .map(|s| s.width().max(s.height()))
            .unwrap_or(self.preview_edge)
            .clamp(640, 8192);
        let canvas = self.canvas_edge();
        // The baked-raster gate is DEFENCE IN DEPTH, not load-bearing: a
        // source-based canvas IS the source decode (`load_active` falls back
        // to `source_preview`, and every install keeps the pair in lockstep),
        // so `canvas == delivered` already silences it. No test can kill this
        // conjunct; it stays because a future canvas that is neither would
        // otherwise be announced with a word — "bake" — that would not be
        // true of it.
        (canvas != delivered
            && canvas != self.preview_edge
            && self.active_variant().is_some_and(|v| v.base.is_some()))
        .then_some(canvas)
    }

    /// Write the status for a door that just installed a canvas, in BOTH
    /// directions: the disclosure when this canvas disagrees with what the
    /// preference can deliver, and `plain` when it does not.
    ///
    /// The `plain` arm is not decoration. Announcing the disagreement without
    /// retracting it left the previous door's "stays at 1280px — edits and
    /// retouches follow that" standing after a redo had put the canvas back
    /// where the preference could reach: both halves false, on the status
    /// bar, which never expires. A door that can create the disagreement can
    /// also end it, so every door writes on every path.
    fn set_canvas_status(&mut self, plain: &'static str) {
        let lang = self.lang;
        self.status = match self.baked_canvas_edge() {
            Some(px) => trf(
                lang,
                "the canvas pixels stay at {px}px (their own bake) — edits and retouches follow that, not the preview preference",
                &[("px", &px.to_string())],
            ),
            None => tr(lang, plain).to_string(),
        };
    }

    /// The reverse-fit / style-prompt target: the ./out PNG behind the active
    /// variant when it is an AI-generated rendition (nothing to fit otherwise).
    /// Reverse-fit maps the SOURCE neutral onto this rendition, so it only
    /// makes sense when the look lives in a generated raster.
    fn fit_target(&self) -> Option<PathBuf> {
        let v = self.active_variant()?;
        (v.kind == VariantKind::Generated).then(|| v.origin.clone()).flatten()
    }

    /// The on-disk PIXEL SOURCE the active variant renders / retouches /
    /// exports FROM. Any variant whose pixels are baked into a ./out raster — a
    /// reimagine (Generated), OR an in-place fill/heal/clone on ANY variant —
    /// carries that full-resolution artifact in `origin` and renders from it;
    /// a pristine source-based variant (原片 / 反推 with no pixel retouch) has
    /// `origin = None` and renders from `src_path` (the RAW / loaded image)
    /// developed by the recipe. Retouch and export key off THIS — not raw
    /// `src_path` — so what exports matches what's on screen (WYSIWYG), never
    /// the untouched negative underneath a generated / retouched rendition.
    fn active_source_path(&self) -> Option<PathBuf> {
        match self.active_variant() {
            Some(v) => v.origin.clone().or_else(|| self.src_path.clone()),
            None => self.src_path.clone(),
        }
    }

    /// Is the active variant an AI-generated raster (look baked into pixels,
    /// not the recipe)? Such a variant has no parametric XMP representation —
    /// exporting a sidecar for it would be a lie; steer the user to 反推 first.
    fn active_is_generated(&self) -> bool {
        self.active_variant().is_some_and(|v| v.kind == VariantKind::Generated)
    }

    /// Load the Before texture: `base` with `curve` (a camera-matched base
    /// look) applied. Lightroom's "Before" is the profile-applied default
    /// render, not the linear negative — without the curve, Before sat
    /// 0.6–1.4 EV under After's own starting point and every compare
    /// exaggerated the edit. Baked rasters pass an empty curve (their pixels
    /// already carry the look).
    fn set_before(&mut self, ctx: &egui::Context, base: &image::DynamicImage, curve: &[[f32; 2]]) {
        let img = if curve.is_empty() {
            to_color_image(base)
        } else {
            to_color_image(&autoshop::render::develop_preview(
                base,
                &EditRecipe { base_curve: curve.to_vec(), ..Default::default() },
            ))
        };
        self.before_tex = Some(ctx.load_texture("before", img, egui::TextureOptions::LINEAR));
        self.before_curve = curve.to_vec();
    }

    /// Uniform failure disclosure for a foreground persist compound — the
    /// single mapper the NoWait develop-lock wrappers report through
    /// (arch item b: five hand-rolled copies had already drifted into
    /// wording bugs). TYPED: `WouldBlock` really is "another Autoshop
    /// process owns this develop right now", so it gets the caller's busy
    /// wording with its retry hint; any other error is real I/O and must
    /// not wear the busy costume — a full disk used to read as "another
    /// process is working on this photo".
    fn persist_postponed(
        &mut self,
        e: &std::io::Error,
        busy_key: &'static str,
        args: &[(&str, &str)],
    ) {
        let es = e.to_string();
        let mut a: Vec<(&str, &str)> = args.to_vec();
        a.push(("err", &es));
        let t = if e.kind() == std::io::ErrorKind::WouldBlock {
            trf(self.lang, busy_key, &a)
        } else {
            trf(self.lang, "not saved — a develop-store write failed: {err}", &a)
        };
        self.status = t.clone();
        self.toast(ToastKind::Error, t);
    }

    /// Decode a variant's retouched master off-thread. FULL resolution — a
    /// 61 MP TIFF takes seconds — so the UI thread never does this inline.
    /// One in-flight decode per (photo, origin): repeat entries (re-clicks
    /// into a still-decoding card, a stash restore) coalesce instead of
    /// stacking a fresh full-resolution decode thread each time. And
    /// `decode::load_image`, not `image::open`: the master rides the same
    /// 4 GiB decode budget and orientation handling as every other source.
    fn spawn_master_load(&mut self, photo: PathBuf, origin: PathBuf) {
        if !self.master_loads.insert((photo.clone(), origin.clone())) {
            return;
        }
        self.status = tr(
            self.lang,
            "loading this variant's retouched master… (showing the source develop meanwhile)",
        )
        .into();
        let (p2, o2) = (photo.clone(), origin.clone());
        self.spawn_worker(
            move || {
                let img = autoshop::decode::load_image(&origin);
                Msg::MasterLoaded { photo, origin, img: Box::new(img) }
            },
            move |e| Msg::MasterLoaded { photo: p2, origin: o2, img: Box::new(Err(e)) },
        );
    }

    /// Make `self.active`'s recipe + base pixels the live working state and
    /// rebuild the before texture. Per-variant transient state (undo history,
    /// local selection, view) restarts — like a soft re-open; what persists is
    /// each variant's recipe + pixels. Shared by switch / push / delete.
    fn load_active(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let Some(v) = self.variants.get(self.active) else { return };
        self.recipe = v.recipe.clone();
        let vkind = v.kind;
        let vbase = v.base.clone();
        let vorigin = v.origin.clone();
        self.rationale = self.recipe.rationale.clone();
        // A DISK-restored baked variant (cold strip restore) carries its
        // origin but no decoded base. The master is FULL resolution — a
        // 61 MP TIFF takes seconds — and this runs on the UI thread, so the
        // old inline decode froze the window on the first click into a
        // cold-restored card. Decode on a worker instead: until the pixels
        // land the canvas shows the source develop UNDER A DISCLOSURE (a
        // silent stand-in is the wrong image wearing the right card label),
        // and Msg::MasterLoaded installs by (photo, origin) identity, so a
        // delete/reorder mid-decode discards the late pixels. A re-click
        // while decoding just spawns a redundant decode whose install finds
        // `base` already filled — wasteful once, never wrong.
        if vbase.is_none()
            && let Some(o) = &vorigin
            && let Some(photo) = self.src_path.clone()
        {
            self.spawn_master_load(photo, o.clone());
        }
        // The strip's READER joins the repair rule: push_variant and
        // switch_variant sync the OUTGOING canvas into the strip, so a
        // washed Original displaced by an AI push comes back through HERE
        // when its card is clicked — no navigation, no stash, no save
        // involved. Memo-bounded; era-2 recipes short-circuit; a Generated
        // entry is skipped like every other ordering site (its curve is
        // empty by invariant).
        // Synchronous first-click cost: the same accepted class as the open
        // gate — one estimate per photo per process when it succeeds, and a
        // transient inability early-exits on the failed probe and retries on
        // the next read.
        if vkind != VariantKind::Generated
            // ...and never while a CROSS-PHOTO open is in flight (src_path
            // already points at the INCOMING photo — the apply_step /
            // live-canvas override hazard); a same-path keep-flight stays
            // admitted. Unreachable today through this fn's callers — the
            // busy interlocks exclude an in-flight open during switch /
            // push / delete — so this is defence-in-depth, like the
            // apply_step gate beside it (whose own callers became
            // busy-gated later), and says so instead of claiming a live
            // scenario.
            && (!self.open_in_flight || (self.open_same_path && self.keep_recipe))
            && let Some(p) = self.src_path.clone()
            && autoshop::pipeline::repair_pre_era_base_curve(&p, &mut self.recipe).is_some()
        {
            // The strip entry follows the healed canvas (the Ctrl+S rule),
            // and the user is told BY TOAST: the dominant callers
            // (switch_variant, push_variant) overwrite self.status in the
            // same frame, so a status write here died before a single paint
            // — the exact defect the load_version site documented.
            if let Some(v) = self.variants.get_mut(self.active) {
                v.recipe = self.recipe.clone();
            }
            self.toast(
                ToastKind::Success,
                tr(
                    lang,
                    "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                )
                .to_string(),
            );
        }
        // Base pixels: the variant's own baked raster, else the shared source
        // neutral (Original / Fitted re-develop the same negative).
        let base = vbase.or_else(|| self.source_preview.clone());
        if let Some(base) = base {
            let (mw, mh) = base.dimensions();
            // Before compares under the canvas recipe's own base calibration
            // (empty for legacy / fit recipes — deliberate). This covers
            // BAKED variants too: an InPlace master is a NEUTRAL develop whose
            // recipe keeps its curve (Before must show the camera look on the
            // healed pixels, not the dark neutral), while a Generated
            // variant's recipe carries an empty curve by construction — the
            // same expression serves both.
            let curve = self.recipe.base_curve.clone();
            self.set_before(ctx, &base, &curve);
            self.base_preview = Some(base);
            // A fresh transparent paint mask sized to THIS base (a generated
            // raster and the source neutral can differ in dimensions).
            self.mask_paint = Some(image::RgbaImage::new(mw, mh));
            self.mask_tex = None;
            self.mask_dirty = false;
            self.paint_last = None;
        }
        self.reset_history(); // you can't undo across variants
        self.region = None;
        self.region_drag = None;
        self.sel_mask = None;
        self.sel_component = None;
        // Same boundary rule as open_path: the rename buffer belongs to the
        // variant it was typed on (M15).
        self.mask_name_buf = None;
        self.overlay_ref = None;
        self.overlay_stale = true;
        self.last_rgb = None; // the retained frame belongs to the OLD variant
        self.disarm_tools();
        self.clone_src = None; // unlike a mere disarm, a variant switch drops the sample
        self.zoom = 1.0;
        self.pan = egui::vec2(0.5, 0.5);
        self.verdict = None;
        self.dirty = true; // re-develop the newly active variant
    }

    /// Switch the active variant losslessly (strip click): in-flight slider
    /// edits are saved back into the variant you leave, then the target's
    /// recipe + pixels become current.
    fn switch_variant(&mut self, idx: usize, ctx: &egui::Context) {
        if idx == self.active || idx >= self.variants.len() {
            return;
        }
        if self.busy {
            // Refusal must be visible — the card stays clickable and a silent
            // no-op reads as a dead UI.
            let t = tr(self.lang, "busy — variants unlock when the current task finishes");
            self.toast(ToastKind::Error, t);
            return;
        }
        if let Some(cur) = self.variants.get_mut(self.active) {
            cur.recipe = self.recipe.clone(); // don't lose the edits in progress
        }
        self.active = idx;
        self.load_active(ctx);
        let lang = self.lang;
        let name = tr(lang, self.variants[self.active].kind.label());
        // A baked variant keeps the resolution it was baked at: switching
        // cannot re-decode a master, so the preference and the canvas can
        // legitimately disagree from here on. Said at the moment the
        // disagreement is created — the combo alone would imply the canvas
        // followed it.
        // (Both arms, like every canvas door — see `set_canvas_status`; this
        // one names the variant, so it writes its own pair.)
        self.status = match self.baked_canvas_edge() {
            None => trf(
                lang,
                "Switched to variant「{name}」 — variants are independent, switching is lossless",
                &[("name", name)],
            ),
            Some(px) => trf(
                lang,
                "Switched to variant「{name}」 — its baked pixels stay at {px}px (their own bake); edits and retouches follow that, not the preview preference",
                &[("name", name), ("px", &px.to_string())],
            ),
        };
    }

    /// Append a variant and switch to it (its recipe/pixels become live).
    /// Saves the outgoing variant's edits first so nothing is lost.
    fn push_variant(&mut self, v: Variant, ctx: &egui::Context) {
        // "Nothing is lost" includes a rename still sitting in its TextEdit:
        // both callers are ASYNC completions (reverse-fit, generative edit)
        // that switch variants while the user may be mid-typing, and the
        // switch's M15 boundary clear then discarded the typed name. A
        // USER-initiated switch keeps the deliberate M15 drop — this flush
        // is the async thief's, committed into the recipe snapshotted below.
        self.commit_mask_name_buf();
        if let Some(cur) = self.variants.get_mut(self.active) {
            cur.recipe = self.recipe.clone();
        }
        self.variants.push(v);
        self.active = self.variants.len() - 1;
        self.load_active(ctx);
    }

    /// Remove variant `idx` (never the last one — the strip stays non-empty).
    /// Only reloads the working state when the ACTIVE variant's identity moves,
    /// so deleting a background variant can't clobber live edits. Refuses while
    /// busy: an in-flight retouch worker resolves its target by `self.active` at
    /// COMPLETION time, so re-anchoring `active` mid-flight would bake its result
    /// onto the wrong variant.
    fn delete_variant(&mut self, idx: usize, ctx: &egui::Context) {
        if self.busy {
            let t = tr(self.lang, "busy — variants unlock when the current task finishes");
            self.toast(ToastKind::Error, t);
            return;
        }
        if self.variants.len() <= 1 || idx >= self.variants.len() {
            return;
        }
        let active_removed = idx == self.active;
        self.variants.remove(idx);
        if self.active > idx {
            self.active -= 1; // same variant, shifted left
        }
        if self.active >= self.variants.len() {
            self.active = self.variants.len() - 1; // removed the tail
        }
        if active_removed {
            self.load_active(ctx); // the active variant changed identity
            // …onto a canvas the user did not choose: deleting the active
            // variant can land on a background BAKED one, whose raster the
            // preference cannot re-decode. Silent before — and it carried the
            // deleted canvas's own disclosure forward as if it still applied.
            self.set_canvas_status("variant removed");
        }
    }

    /// Recipe snapshot path for version `n` — `v<n>.recipe.json` in the photo's
    /// central develop dir (gap batch G, ≈ Lightroom virtual copies: cheap
    /// parametric versions, never touching the library or the working
    /// `recipe.json`).
    fn version_path(src: &std::path::Path, n: u32) -> PathBuf {
        autoshop::store::version_target(src, n)
    }

    /// Re-point every stored mask index (selection, colour-range sampler,
    /// redraw target) after the mask list changed shape — delete, drag-drop
    /// reorder, ⬆/⬇ swap. One place, so an index-carrying tool cannot be
    /// forgotten: a dangling index used to make the range sampler write into
    /// whatever mask slid under it.
    fn remap_mask_indices(&mut self, f: impl Fn(usize) -> Option<usize>) {
        let sel_before = self.sel_mask;
        self.sel_mask = self.sel_mask.and_then(&f);
        // Component selection is meaningful only while the SAME mask stays
        // selected under the same index-space; a moved/vanished mask drops it
        // (its component list travelled with the mask, but every canvas tool
        // re-derives from sel_mask — a stale pair here was the same class of
        // bug as the dangling range sampler).
        if self.sel_mask != sel_before || self.sel_mask.is_none() {
            self.sel_component = None;
        }
        self.range_picking = self.range_picking.and_then(&f);
        self.placing_mask = self.placing_mask.take().and_then(|(k, t)| match t {
            PlaceTarget::NewMask => Some((k, PlaceTarget::NewMask)),
            // A vanished redraw target degrades to a NEW mask placement (the
            // pre-v0.22 `Option<usize>` behaviour, kept); a vanished
            // component target must NOT — appending a component to nothing
            // has no sensible fallback, so the arm disarms.
            PlaceTarget::Redraw(i) => {
                Some((k, f(i).map_or(PlaceTarget::NewMask, PlaceTarget::Redraw)))
            }
            PlaceTarget::Component(i, mode) => {
                f(i).map(|j| (k, PlaceTarget::Component(j, mode)))
            }
        });
        // The pending-rename buffer is index-carrying too: after a delete /
        // reorder its seed-guard (name-at-seed must still match) correctly
        // refused to cross-commit — but the SAME mask at its new index then
        // failed the match as well, and the typed rename silently died.
        self.mask_name_buf =
            self.mask_name_buf.take().and_then(|(j, o, b)| Some((f(j)?, o, b)));
        // The mask-brush session carries its TARGET index: 「Apply」 bakes the
        // stroke into `mask_brush.0`, so after a delete/reorder it painted
        // whatever mask slid under the stale slot — the dangling-range-sampler
        // class again, writing pixel weights instead of range bounds. A moved
        // target follows its mask; a vanished one ends the session exactly
        // like Esc (gray buffer AND paint canvas — see disarm_tools: fill/heal
        // would inherit the strokes as a phantom retouch selection). Not a
        // new-mask fallback: that would resurrect a just-deleted mask.
        if let Some((target, erase)) = self.mask_brush.take() {
            match target.map(&f) {
                None => self.mask_brush = Some((None, erase)),
                Some(Some(j)) => self.mask_brush = Some((Some(j), erase)),
                Some(None) => {
                    self.mask_brush_gray = None;
                    self.paint_mode = false;
                    self.paint_last = None;
                    self.clear_mask();
                }
            }
        }
    }

    /// Rescan the photo's develop dir for version snapshots (cached in
    /// `self.versions`; called on photo open and after saving a version — NOT
    /// every frame).
    fn refresh_versions(&mut self) {
        self.versions.clear();
        let Some(src) = self.src_path.as_deref() else { return };
        self.versions = autoshop::store::list_versions(src);
    }

    /// Save the CURRENT develop as the next numbered version snapshot.
    fn save_version(&mut self) {
        // A typed-but-uncommitted mask rename belongs in the snapshot —
        // every persistence entry point flushes it (the U10 rule; L27).
        self.commit_mask_name_buf();
        let lang = self.lang;
        let Some(src) = self.src_path.clone() else { return };
        // ONE NoWait lock across claim → raster freeze → recipe write:
        // another process's delete_version sweeping the just-claimed slot
        // mid-snapshot is a real interleave — and this is a foreground
        // button, so busy must report, not hang the UI.
        let locked = autoshop::store::with_develop_lock(
            &src,
            autoshop::store::DevelopLockMode::NoWait,
            || -> std::io::Result<()> {
        // ATOMICALLY reserved number (store::claim_version): disk-list+1
        // raced the auto-backup (and the web/CLI) to the same N — the later
        // publish silently replaced the earlier snapshot.
        let (n, vpath) = match autoshop::store::claim_version(&src) {
            Ok(v) => v,
            Err(e) => {
                let t = trf(lang, "Save version failed: {err}", &[("err", &e.to_string())]);
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
                return Ok(());
            }
        };
        // Freeze referenced rasters under THIS version (the backup gate's
        // pair): a version whose recipe pointed at another version's frozen
        // raster (load v3 → save as v4) broke the moment v3 was deleted.
        let dev = autoshop::store::develop_dir(&src);
        let mut snap = self.recipe.clone();
        if let Err(e) = autoshop::store::snapshot_rasters(&mut snap, &dev, n) {
            let _ = std::fs::remove_file(&vpath); // release the claim
            let t = trf(lang, "Save version failed: {err}", &[("err", &e.to_string())]);
            self.status = t.clone();
            self.toast(ToastKind::Error, t);
            return Ok(());
        }
        let res = autoshop::pipeline::write_recipe(&src, &snap, Some(vpath.clone()));
        if res.is_err() {
            // Release the claimed slot AND this call's frozen rasters — an
            // empty version must not pollute the list after a failed write.
            autoshop::store::rollback_frozen_rasters(&dev, n);
            let _ = std::fs::remove_file(&vpath);
        }
        match res {
            Ok(p) => {
                self.refresh_versions();
                self.status = trf(
                    lang,
                    "Version v{n} saved → {path}",
                    &[("n", &n.to_string()), ("path", &p.display().to_string())],
                );
            }
            Err(e) => {
                let t = trf(lang, "Save version failed: {err}", &[("err", &e.to_string())]);
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
            }
        }
                Ok(())
            },
        );
        if let Err(e) = locked {
            self.persist_postponed(&e, "Save version failed: {err}", &[]);
        }
    }

    /// Load version `n` as the working recipe (one undo step, like AI Analyze).
    fn load_version(&mut self, n: u32) {
        let lang = self.lang;
        let Some(src) = self.src_path.clone() else { return };
        let p = Self::version_path(&src, n);
        match std::fs::read_to_string(&p)
            .map_err(anyhow::Error::from)
            .and_then(|s| Ok(serde_json::from_str::<EditRecipe>(&s)?))
        {
            Ok(mut r) => {
                let dropped = r.clamp();
                if !dropped.is_empty() {
                    // Same W20 disclosure as the open restore: a snapshot
                    // past the caps loads minus edits, never silently —
                    // and all four loss kinds, like the open toast.
                    let t = trf(
                        lang,
                        "recipe limits discarded {n} mask(s), {m} component(s), {c} curve point(s) and {s} string byte(s) on restore — the saved file exceeds the app's caps",
                        &[
                            ("n", &dropped.dropped_masks.to_string()),
                            ("m", &dropped.dropped_components.to_string()),
                            ("c", &dropped.truncated_curve_points.to_string()),
                            ("s", &dropped.truncated_string_bytes.to_string()),
                        ],
                    );
                    self.toast(ToastKind::Error, t);
                }
                // Snapshots name their rasters by bare file name, like the
                // working recipe — re-anchor them to the develop dir.
                if let Some(base) = p.parent() {
                    autoshop::store::resolve_mask_paths(&mut r, base);
                }
                // Then DETACH from the snapshot: the frozen v<N>.*.png files
                // belong to that version and `delete_version` sweeps them, so
                // a canvas pointing at them lost its masks the moment the user
                // deleted the version it was loaded from — and the next save
                // persisted the dangling path. The loaded state gets its own
                // claimed copies.
                autoshop::store::detach_rasters(&src, &mut r, "mask-restored");
                // A Generated variant's pixels already carry the camera look
                // AND the lens corrections — a source-based snapshot's
                // calibration would cook both twice (same strip rule as the
                // open-restore and Analyze paths).
                if self.active_is_generated() {
                    r.base_curve = Vec::new();
                    r.lens_profile = Default::default();
                    // The anchor too: its pixels carry a BAKED white balance,
                    // so an absolute-Kelvin claim over them would be false —
                    // generated canvases keep the relative 5500 model.
                    r.as_shot_k = None;
                    r.as_shot_tint = None;
                }
                // A version snapshot is a saved recipe like any other, and
                // it predates the era stamp by definition — loading one put
                // an unrepaired washed curve straight onto the canvas and
                // into everything exported from it. AFTER the generated
                // strip: a generated canvas keeps no base curve, so there is
                // nothing to repair (paying the estimate just to discard it,
                // and disclosing a re-estimate the strip then deleted, were
                // both wrong). The note rides the SAME status write as the
                // load announcement — a separate earlier write was replaced
                // one statement later, so the promised disclosure never
                // survived to a single rendered frame.
                let relook =
                    autoshop::pipeline::repair_pre_era_base_curve(&src, &mut r).is_some();
                self.recipe = r;
                self.resync_recipe_display();
                self.dirty = true;
                let loaded = trf(lang, "Loaded version v{n} — Ctrl+Z returns to before the load", &[("n", &n.to_string())]);
                self.status = if relook {
                    // The GUI's OWN sentence, localized: the engine note is
                    // English prose meant for the CLI/HTTP surfaces, and
                    // embedding it verbatim put raw English into a localized
                    // status line.
                    format!(
                        "{loaded} — {}",
                        tr(
                            lang,
                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                        )
                    )
                } else {
                    loaded
                };
            }
            Err(e) => {
                let t = trf(
                    lang,
                    "Load v{n} failed: {err}",
                    &[("n", &n.to_string()), ("err", &e.to_string())],
                );
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Open one of the gallery photos by index (keeps the thumbnail highlighted).
    fn open_gallery_index(&mut self, idx: usize) {
        if self.busy {
            return;
        }
        let Some(path) = self.gallery.get(idx).cloned() else { return };
        self.selected = Some(idx);
        self.open_path(path);
    }

    /// Scan `dir` (recursively) for sources off the UI thread and replace the
    /// gallery — folders can hold thousands of RAWs, so this never blocks paint.
    fn open_folder(&mut self, dir: PathBuf) {
        if self.busy {
            return;
        }
        let lang = self.lang;
        self.busy = true;
        self.status = trf(lang, "scanning {path} …", &[("path", &dir.display().to_string())]);
        self.spawn_worker(
            move || {
                let res = autoshop::pipeline::find_sources(&dir).map(|list| (dir, list));
                Msg::Folder(Box::new(res))
            },
            |e| Msg::Folder(Box::new(Err(e))),
        );
    }

    /// The live working state as an [`UndoStep`]: current recipe + the active
    /// variant's pixel identity. Arc/path clones only — never pixel copies.
    fn current_step(&self) -> UndoStep {
        UndoStep {
            recipe: self.recipe.clone(),
            base: self.active_variant().and_then(|v| v.base.clone()),
            origin: self.active_variant().and_then(|v| v.origin.clone()),
        }
    }

    /// Reset undo history — call when a brand-new photo opens, when the active
    /// variant changes (you can't undo across either), or when a photo GOES
    /// AWAY: a failed fresh open leaves no canvas for its steps to apply to,
    /// and an undo there restored one that no longer existed. `committed`
    /// becomes the current head — with no photo open that is the empty step,
    /// which is exactly what stops the next settled frame pushing a phantom.
    fn reset_history(&mut self) {
        self.committed = self.current_step();
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Commit the current state as one undo step RIGHT NOW (when it differs
    /// from the head). Used directly by programmatic pixel swaps (the retouch
    /// bake-in): waiting for `commit_if_settled` left a one-frame window where
    /// a same-frame Ctrl+Z saw only the older history and undid a recipe step
    /// while keeping the freshly retouched pixels installed.
    fn commit_now(&mut self) {
        let cur = self.current_step();
        if cur.recipe != self.committed.recipe || !cur.same_pixels(&self.committed) {
            self.undo_stack.push(std::mem::replace(&mut self.committed, cur));
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0); // cap history memory
            }
            self.redo_stack.clear();
        }
    }

    /// Commit the current state as ONE undo step once the edit gesture settles
    /// (pointer released) — dragging a slider is one step, not one per frame.
    /// Programmatic edits (Analyze, Reset) land here on the next frame.
    fn commit_if_settled(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_down()) {
            self.commit_now();
        }
    }

    /// Recipe replaced wholesale (undo/redo/Reset/version load/AI apply): the
    /// derived display state must follow, or the panel keeps showing an AI
    /// rationale / verdict that describes a recipe no longer on screen — and
    /// an ARMED index-carrying tool (range sampler, ↻ Redraw) would fire into
    /// whatever mask now happens to sit at its remembered index.
    /// One tool at a time on the canvas — the SINGLE owner of the disarm
    /// list. Ten arming sites used to hand-copy these assignments, and the
    /// comments at two of them record real bugs born from copies drifting;
    /// every arming site now calls this before setting its own flag.
    /// Transient gesture anchors die with the tools: a stale place_start or
    /// crop_drag used to hijack the next drag. clone_src (the sampled source
    /// pin) deliberately survives — samples stay for resuming, like paint.
    fn disarm_tools(&mut self) {
        self.crop_mode = false;
        self.paint_mode = false;
        self.clone_mode = false;
        self.wb_picking = false;
        self.range_picking = None;
        self.placing_mask = None;
        self.place_start = None;
        self.crop_drag = None;
        self.mask_drag = None;
        self.paint_last = None;
        // The mask-brush session dies with its paint mode (Esc = cancel):
        // strokes live in the canvas + gray buffer only until 「Apply」bakes
        // them into a claimed raster, so dropping both IS the cancel. The
        // canvas is cleared too — fill/heal would otherwise inherit the
        // brush-mask strokes as a phantom retouch selection.
        if self.mask_brush.take().is_some() {
            self.mask_brush_gray = None;
            self.clear_mask();
        }
        // A preset change armed for the crop tool must die with it — the
        // flag survived Esc / Done / a hold-B detour and rewrote the box as
        // a surprise on the next crop entry (CX5-4).
        self.crop_aspect_pending = false;
    }

    /// Any canvas tool armed? (the once-hand-written OR list)
    fn tool_armed(&self) -> bool {
        self.crop_mode
            || self.paint_mode
            || self.clone_mode
            || self.wb_picking
            || self.range_picking.is_some()
            || self.placing_mask.is_some()
    }

    /// Commit a pending mask-rename buffer to its own mask (the same
    /// seed-guard the panel's row-switch commit uses: only while that mask's
    /// name still equals the seed-time snapshot). Focus-independent Ctrl+S
    /// runs while the TextEdit still holds focus — without this it saved the
    /// old name and the advanced baseline dropped the rename.
    fn commit_mask_name_buf(&mut self) {
        if let Some((j, orig, buf)) = self.mask_name_buf.clone()
            && let Some(prev) = self.recipe.masks.get_mut(j)
            && prev.name == orig
            && buf != orig
        {
            prev.name = buf.clone();
            // Re-seed so the panel's own lost-focus commit stays a no-op.
            self.mask_name_buf = Some((j, buf.clone(), buf));
        }
    }

    fn resync_recipe_display(&mut self) {
        self.rationale = self.recipe.rationale.clone();
        self.verdict = None;
        // A wholesale recipe swap (undo/redo/Reset/version load/AI apply)
        // disarms EVERY canvas tool — an armed index-carrying tool would fire
        // into whatever now sits at its remembered index, and a live
        // crop/rotate drag would re-apply its stale start angle over the
        // freshly restored recipe on the very next drag frame.
        self.disarm_tools();
        self.mask_name_buf = None; // stale (index, text) must not cross-commit
        // A curve-point drag in flight when the recipe is swapped (Ctrl+Z
        // mid-drag) kept mutating point i of the RESTORED curve on the next
        // drag frame (M17).
        self.curve_drag = None;
        // A wholesale recipe swap can shrink the mask list — an out-of-range
        // selection must not linger (every consumer bounds-checks, but the
        // panel would silently deselect anyway; do it deterministically).
        // Same-index-different-mask after undo is a known cosmetic residue —
        // mask identity isn't tracked across history steps.
        self.sel_mask = self.sel_mask.filter(|&i| i < self.recipe.masks.len());
        // The component selection rides on the mask's OWN list, which the
        // swap can also reshape — same deterministic bound.
        self.sel_component = self.sel_component.filter(|&c| {
            self.sel_mask
                .and_then(|i| self.recipe.masks.get(i))
                .is_some_and(|m| c < m.components.len())
        });
    }

    /// Apply a history step: recipe always; the active variant's pixels only
    /// when the step's identity differs (so plain slider undos stay pixel-free
    /// and O(1)). A restored `base: None` reverts a retouched source variant
    /// to the shared source neutral.
    fn apply_step(&mut self, step: UndoStep, ctx: &egui::Context) {
        let pixels_differ = !step.same_pixels(&self.committed);
        self.committed = step.clone();
        self.recipe = step.recipe;
        // The history is a READER too: a step recorded while the canvas was
        // washed reinstalls the pre-era pair on undo/redo — and the actor
        // that later promoted the canvas (Reset, Analyze, load_version,
        // Ctrl+S) cannot know which stack entries still hold it (the
        // save-time restamp covers only the pair IT replaced). Repairing on
        // the way back keeps every door to the canvas behind one rule.
        // Cost honesty: when the OPEN succeeded, the worker primed the
        // memo and this is a lookup. But washed steps enter history
        // precisely when the open-time estimate was an INABILITY — uncached
        // by design — so the first traversal here IS the retry point and
        // pays one estimate (the accepted class shared with load_active and
        // the open gate); its answer memoizes and the rest of the washed
        // history heals by lookup. Era-2 installs short-circuit before any
        // I/O; the Generated guard matches every other ordering site; and a
        // CROSS-PHOTO in-flight open is refused — src_path already points
        // at the INCOMING photo while this history belongs to the outgoing
        // one (the live-canvas override records the same hazard), so
        // repairing would estimate the WRONG photo onto this canvas. A
        // same-path preview-resolution re-decode (request AND fact: the
        // keep_recipe request verified against the recorded open_same_path)
        // keeps the pair coherent AND its keep arm preserves history —
        // refusing there made a washed install DURABLE, so it is admitted.
        // A same-path FRESH reopen is NOT: its rebuild discards this repair,
        // so admitting it bought a concurrent decode and a false success
        // toast. `committed` follows the heal, or the next commit_now would
        // push the washed head straight back onto the stack.
        //
        // DEFENCE IN DEPTH since the keyboard undo took the buttons' busy
        // gate: `open_in_flight` implies `busy` (open_path early-returns while
        // busy, sets both, and clears both in one handler), and undo/redo are
        // this fn's only callers — so no live caller reaches here mid-flight
        // any more. The gate stays because ungating the keyboard would
        // resurrect every scenario above, and the test drives it directly.
        if (!self.open_in_flight || (self.open_same_path && self.keep_recipe))
            && !self
                .variants
                .get(self.active)
                .is_some_and(|v| v.kind == VariantKind::Generated)
            && let Some(p) = self.src_path.clone()
            && autoshop::pipeline::repair_pre_era_base_curve(&p, &mut self.recipe).is_some()
        {
            self.committed.recipe = self.recipe.clone();
            if let Some(v) = self.variants.get_mut(self.active) {
                v.recipe = self.recipe.clone();
            }
            let lang = self.lang;
            self.toast(
                ToastKind::Success,
                tr(
                    lang,
                    "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                )
                .to_string(),
            );
        }
        if pixels_differ {
            if let Some(v) = self.variants.get_mut(self.active) {
                v.base = step.base;
                v.origin = step.origin;
            }
            self.refresh_active_pixels(ctx);
            // The quietest door: undoing to a SUPERSEDED master installs a
            // raster the preference cannot re-decode (the resolution switch
            // repoints only the Arc the canvas held), and undo/redo wrote no
            // status at all — so the combo was left to imply the canvas had
            // followed it. BOTH directions: the redo back to a reachable
            // canvas has to retract the claim, or it stands there false.
            self.set_canvas_status("restored the canvas pixels");
        }
        self.dirty = true;
        self.resync_recipe_display();
    }

    /// Rebuild the pixel-derived per-variant state after the active variant's
    /// base changed under the same variant (retouch undo/redo): before pane,
    /// paint canvas, overlay reference and the retained frame all described
    /// the old raster. History is deliberately NOT touched here.
    fn refresh_active_pixels(&mut self, ctx: &egui::Context) {
        let Some(v) = self.variants.get(self.active) else { return };
        let base = v.base.clone().or_else(|| self.source_preview.clone());
        if let Some(base) = base {
            let (mw, mh) = base.dimensions();
            // Same rule as load_active: the canvas recipe's own curve serves
            // every variant (a Generated recipe's curve is empty by
            // construction; an InPlace master is neutral and needs it).
            let curve = self.recipe.base_curve.clone();
            self.set_before(ctx, &base, &curve);
            self.base_preview = Some(base);
            self.mask_paint = Some(image::RgbaImage::new(mw, mh));
            self.mask_tex = None;
            self.mask_dirty = false;
            self.paint_last = None;
        }
        self.overlay_ref = None;
        self.overlay_stale = true;
        self.last_rgb = None;
    }

    fn undo(&mut self, ctx: &egui::Context) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.committed.clone());
            self.apply_step(prev, ctx);
        }
    }

    fn redo(&mut self, ctx: &egui::Context) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.committed.clone());
            self.apply_step(next, ctx);
        }
    }

    /// Populate the Settings form from the resolved config (keys are shown only as
    /// "present", never revealed). Called when the window opens.
    fn load_settings_form(&mut self) {
        let cfg = autoshop::config::Config::load();
        // Keep any model lists already fetched this session so reopening Settings
        // doesn't force a re-fetch — and keep an in-flight fetch's flag alive:
        // zeroing it mid-fetch stopped the repaint pump AND re-armed the fetch
        // button (duplicate requests, stalled status).
        let chat_choices = std::mem::take(&mut self.settings.chat_choices);
        let image_gen_choices = std::mem::take(&mut self.settings.image_gen_choices);
        let fetching_models = self.settings.fetching_models;
        let models_from_base = std::mem::take(&mut self.settings.models_from_base);
        self.settings = SettingsForm {
            analysis_provider_api: cfg.analysis_is_api(),
            image_provider_oauth: cfg.image_is_oauth(),
            analysis_model: cfg.analysis_model.clone(),
            analysis_base_url: cfg.analysis_base_url.clone(),
            analysis_api_key: String::new(),
            analysis_key_present: cfg.analysis_api_key.is_some(),
            image_model: cfg.openai_model.clone(),
            image_base_url: cfg.openai_base_url.clone(),
            image_gen_model: cfg.openai_image_model.clone(),
            image_api_key: String::new(),
            image_key_present: cfg.openai_api_key.is_some(),
            status: String::new(),
            chat_choices,
            image_gen_choices,
            fetching_models,
            models_from_base,
        };
    }

    /// Persist the Settings form to autoshop.local.json (gitignored). A blank key
    /// keeps the stored one. The next Analyze/Export reloads Config, so it applies.
    fn save_settings_form(&mut self) {
        let mut cur = autoshop::config::load_local_settings();
        cur.analysis_provider =
            Some(if self.settings.analysis_provider_api { "api" } else { "oauth" }.to_string());
        cur.image_provider =
            Some(if self.settings.image_provider_oauth { "oauth" } else { "api" }.to_string());
        cur.analysis_model = Some(self.settings.analysis_model.trim().to_string());
        cur.analysis_base_url = Some(self.settings.analysis_base_url.trim().to_string());
        cur.image_model = Some(self.settings.image_model.trim().to_string());
        cur.image_base_url = Some(self.settings.image_base_url.trim().to_string());
        cur.image_gen_model = Some(self.settings.image_gen_model.trim().to_string());
        // Secrets: only overwrite when a non-empty value was actually typed.
        let ak = self.settings.analysis_api_key.trim().to_string();
        let ik = self.settings.image_api_key.trim().to_string();
        if !ak.is_empty() {
            cur.analysis_api_key = Some(ak);
        }
        if !ik.is_empty() {
            cur.image_api_key = Some(ik);
        }
        match autoshop::config::save_local_settings(&cur) {
            Ok(p) => {
                self.settings.analysis_api_key.clear();
                self.settings.image_api_key.clear();
                // Presence reflects the RESOLVED config (file merged with env) —
                // deriving it from the file alone told a user whose key lives in
                // OPENAI_API_KEY that no key was set right after saving.
                let cfg = autoshop::config::Config::load();
                self.settings.analysis_key_present = cfg.analysis_api_key.is_some();
                self.settings.image_key_present = cfg.openai_api_key.is_some();
                self.settings.status =
                    trf(self.lang, "saved → {path}", &[("path", &p.display().to_string())]);
                self.status = tr(self.lang, "settings saved — applies to the next AI call (Analyze / Fill / Reimagine)").into();
            }
            Err(e) => {
                self.settings.status =
                    trf(self.lang, "save failed: {err}", &[("err", &e.to_string())])
            }
        }
    }

    /// Fetch the account's model ids (`GET /models`) on a worker thread and fill the
    /// Settings pick-lists. Uses the key/base typed in the form if present, else the
    /// saved config — so it works whether or not the user has saved a key yet.
    fn fetch_models(&mut self) {
        if self.settings.fetching_models {
            return;
        }
        self.settings.fetching_models = true;
        self.settings.status = tr(self.lang, "fetching models…").into();
        let form_key = self.settings.image_api_key.trim().to_string();
        let form_base = self.settings.image_base_url.trim().to_string();
        // Drop the previous endpoint's lists NOW and stamp the base this fetch
        // targets: a failed fetch then leaves empty lists (grounded fallbacks)
        // rather than ids from the old server under the new URL's name.
        self.settings.chat_choices.clear();
        self.settings.image_gen_choices.clear();
        self.settings.models_from_base = if form_base.is_empty() {
            autoshop::config::Config::load().openai_base_url.clone()
        } else {
            form_base.clone()
        };
        // spawn_worker's catch_unwind guarantees the UI's `fetching_models`
        // flag always clears — a panic still delivers Msg::Models(Err) (this
        // site used to hand-roll a Drop guard for exactly that; the helper
        // now covers every worker uniformly).
        self.spawn_worker(
            move || {
                let cfg = autoshop::config::Config::load();
                let base =
                    if form_base.is_empty() { cfg.openai_base_url.clone() } else { form_base };
                let key = if form_key.is_empty() {
                    cfg.openai_api_key.clone().unwrap_or_default()
                } else {
                    form_key
                };
                Msg::Models(autoshop::openai_models::list_models(&base, &key))
            },
            |e| Msg::Models(Err(e)),
        );
    }

    /// The quit-confirm layer (in-app egui window). Lists what quitting would
    /// lose; 「Save all & quit」 writes each pending develop exactly like
    /// Ctrl+S (explicit user save → no backup gate), 「Discard & quit」 makes
    /// the state clean so the close guard lets the next close through.
    fn confirm_quit_layer(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let accent = self.theme.colors().accent_text; // Copy — safe in closures
        // Everything quitting would lose: the stash + the open photo's canvas
        // (the live canvas outranks its own stale stash entry). Each entry
        // carries its pixel identity so 「Save all」 persists a baked retouch's
        // master link exactly like Ctrl+S would.
        let mut pending: Vec<PendingSave> = self
            .nav_stash
            .iter()
            .map(|(p, st)| {
                (
                    p.clone(),
                    st.recipe.clone(),
                    st.origin.clone().map(|o| (o, st.kind == VariantKind::Generated)),
                    stash_strip_record(st),
                )
            })
            .collect();
        // Live-canvas override — skipped while a photo OPEN is in flight:
        // src_path already points at the new photo but recipe/saved_recipe
        // still describe the old one, so acting on the pair would either drop
        // the new photo's stash entry or save the old recipe under the new
        // path. The stash (written by open_path just before the flight) is
        // the correct authority for that window. ONLY the open transition —
        // gating on plain `busy` let any background worker (Export…) empty
        // `pending`, and the empty-pending branch below then CLOSED the app,
        // losing a sole live unsaved canvas (background work still lands
        // under the dialog — the scrim below blocks only pointer input).
        if !self.open_in_flight && let Some(p) = self.src_path.clone() {
            let origin = self.active_variant().and_then(|v| v.origin.clone());
            // The live canvas outranks its own stash entry WHOLESALE: displace
            // it even when the canvas is clean (the user reverted) — the loop
            // below would otherwise write the stale stashed edits to disk.
            pending.retain(|(q, ..)| q != &p);
            // Both directions count (gained OR dropped master) — see open_path.
            let recorded = autoshop::store::read_pixel_source(&p);
            let live_generated = self.active_is_generated();
            let pixels_unsaved = !(same_master_opt(
                recorded.as_ref().map(|(q, _)| q.as_path()),
                origin.as_deref(),
            ) && recorded.as_ref().map(|(_, g)| *g)
                == origin.as_ref().map(|_| live_generated));
            // The strip counts as unsaved work of the OPEN photo too — a
            // clean canvas over an unpersisted strip must still be listed,
            // or Save-all had nothing to write and the guard's re-check
            // bounced the close forever (the v0.21 dead-button livelock).
            if dirty_vs(&self.recipe, &self.saved_recipe)
                || pixels_unsaved
                || self.open_dirty_variants() > 0
            {
                let pix = origin.map(|o| (o, self.active_is_generated()));
                pending.push((p, self.recipe.clone(), pix, self.current_strip_record()));
            }
        }
        // Background variants ride IN `pending` since v0.22: each entry
        // carries its photo's strip record, and Save-all persists it to
        // variants.json beside the develop. The count still gates the
        // empty-pending close below (defence-in-depth — a strip-dirty photo
        // is also pushed into `pending` above).
        let orphan_variants = self.inactive_dirty_variants();
        if pending.is_empty() && orphan_variants == 0 {
            // Saved (or discarded) since the guard fired — nothing to protect.
            self.confirm_quit = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let mut open = true;
        let mut save_quit = false;
        let mut discard_quit = false;
        let mut cancel = false;
        // The global shortcut block is gated off while this layer is up, so
        // the promised keyboard grammar is honoured HERE: Esc = the safe
        // escape (always), Enter = the safe default (save everything) — but
        // only while NO button holds keyboard focus, or Enter on a focused
        // Cancel/Discard would fire Save instead of the focused control.
        let widget_focused = ctx.memory(|m| m.focused()).is_some();
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                cancel = true;
            }
            if !widget_focused && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                save_quit = true;
            }
        });
        // A modal owns the pointer as well as the keyboard (D13): the scrim
        // swallows every click behind the dialog — sliders were still live
        // during the save-or-discard decision — and dims the stakes into
        // view. Background workers are unaffected (see open_in_flight above).
        egui::Area::new(egui::Id::new("confirm_quit_scrim"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                let r = ctx.screen_rect();
                ui.allocate_response(r.size(), egui::Sense::click_and_drag());
                ui.painter().rect_filled(r, 0.0, egui::Color32::from_black_alpha(96));
            });
        egui::Window::new(tr(lang, "● Unsaved edits"))
            .id(egui::Id::new("confirm_quit"))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                if !pending.is_empty() {
                    ui.label(trf(
                        lang,
                        "{n} photo(s) have edits that were never saved:",
                        &[("n", &pending.len().to_string())],
                    ));
                }
                // Parent folder + stem: two DSC_0431 from two shoots must be
                // distinguishable in the one dialog where the stakes are real.
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    for (p, ..) in &pending {
                        let folder = p
                            .parent()
                            .and_then(|d| d.file_name())
                            .and_then(|s| s.to_str())
                            .unwrap_or("");
                        ui.label(
                            egui::RichText::new(format!(
                                "{folder}/{}",
                                autoshop::pipeline::stem(p)
                            ))
                            .small()
                            .weak(),
                        );
                    }
                });
                if orphan_variants > 0 {
                    // Informational since v0.22: Save-all persists each
                    // photo's whole variant strip (variants.json), so these
                    // are rescued WITH their photo, not lost.
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(trf(
                            lang,
                            "{n} other variant(s) hold edits — 「Save all」 saves each photo's whole variant strip along with its develop.",
                            &[("n", &orphan_variants.to_string())],
                        ))
                        .small()
                        .weak(),
                    );
                }
                ui.add_space(6.0);
                // Safe escape far left, the destructive action fenced off by
                // space, the primary (save) tinted and last — a 4px slip from
                // Save must not land on an unrecoverable Discard.
                ui.horizontal(|ui| {
                    cancel |= ui.button(tr(lang, "Cancel")).on_hover_text("Esc").clicked();
                    ui.add_space(18.0);
                    discard_quit = ui
                        .button(
                            egui::RichText::new(tr(lang, "Discard & quit"))
                                .color(ui.visuals().warn_fg_color),
                        )
                        .on_hover_text(tr(lang, "Quit WITHOUT saving — these edits are gone for good"))
                        .clicked();
                    save_quit |= ui
                        .button(egui::RichText::new(tr(lang, "Save all & quit")).color(accent))
                        .on_hover_text(tr(lang, "Enter · save every listed develop, then quit"))
                        .clicked();
                });
            });
        if save_quit {
            let mut failed: Option<String> = None;
            // Collected, not fatal — reported on the way out (see below).
            let mut xmp_warns: Vec<String> = Vec::new();
            let mut clear_warns: Vec<String> = Vec::new();
            for (p, r, pix, strip) in &pending {
                // ONE NoWait lock per photo across its whole persist compound
                // (the Ctrl+S pairing). A develop busy in another process
                // fails THIS photo — the quit bounces and reports below —
                // instead of freezing the dialog behind an unbounded Wait.
                let locked: std::io::Result<()> = autoshop::store::with_develop_lock(
                    p,
                    autoshop::store::DevelopLockMode::NoWait,
                    || {
                // Neutral + no pixel identity + trivial strip = Ctrl+S's
                // "clear my edits": WRITING neutral files here pinned the
                // existence-keyed ● badge that a direct Ctrl+S removes. A
                // non-trivial strip takes the save path below instead —
                // clear_develop would destroy the OTHER cards' work.
                if r.is_noop() && pix.is_none() && strip.is_none() {
                    // The store's ONE clear primitive (both surfaces): it takes
                    // the retired pixels.json.bak with it, which a bare unlink
                    // left behind for the next open to republish.
                    match autoshop::store::clear_develop(p) {
                        Ok(o) => {
                            // clear_develop took variants.json with it — the
                            // open photo's mirror must follow, or the guard's
                            // way-out re-check compares live-trivial against
                            // a stale record and bounces the close.
                            if self.src_path.as_deref() == Some(p.as_path()) {
                                self.saved_strip = None;
                            }
                            // Cleared, but not MARKED — same channel as the XMP
                            // half: quitting silently would let a projection
                            // copied beside the RAW undo this clear unannounced.
                            if let Some(w) = o.marker_warning {
                                clear_warns.push(format!("{}: {w}", autoshop::pipeline::stem(p)));
                            }
                        }
                        Err(e) => {
                            failed = Some(format!("{}: {e}", autoshop::pipeline::stem(p)));
                        }
                    }
                    return Ok(());
                }
                // A GENERATED entry's recipe is the STRIPPED canvas form —
                // persisting it verbatim erased the RAW's calibration from
                // disk (the Analyze saver writes the calibrated form; a
                // master that later fails to decode then falls back to a
                // dark, uncorrected develop). Re-stamp from the same
                // saved-first source produce_recipe uses.
                let mut disk = r.clone();
                if pix.as_ref().is_some_and(|(_, g)| *g) {
                    // ONE snapshot for every half — independent reads could
                    // pair an OLD curve with a NEW profile if another surface
                    // published between them. The era stamp rides WITH the
                    // curve (the paste rule): this WRITES recipe.json, and a
                    // canvas-era stamp over a saved era-1 curve would launder
                    // it past every repair.
                    let cal = autoshop::pipeline::photo_calibration(p);
                    disk.version = cal.version;
                    disk.base_curve = cal.base_curve;
                    disk.lens_profile = cal.lens_profile;
                    disk.as_shot_k = cal.as_shot_k;
                    disk.as_shot_tint = cal.as_shot_tint;
                }
                // The WRITER's rule (Ctrl+S, api_xmp): a stashed canvas whose
                // open-time estimate failed still carries the washed pre-era
                // curve — repaired before it is persisted. Memo-cheap when
                // already repaired, and it also covers the generated arm
                // above, whose calibration snapshot may itself have adopted
                // an unrepaired curve when the store read hit an inability.
                // SAID on the loop's existing quitting-time channel — gated
                // on the RECIPE write, not the compound result below: the
                // pixel-link half can fail AFTER recipe.json already
                // published the re-estimated curve, and dropping the note
                // then hid a mutation that IS on disk (the Ctrl+S rule: the
                // disclosure travels with the mutation).
                let relook_note = autoshop::pipeline::repair_pre_era_base_curve(p, &mut disk);
                let generated = pix.as_ref().is_some_and(|(_, g)| *g);
                let recipe_written = autoshop::pipeline::write_recipe(p, &disk, None);
                let recipe_ok = recipe_written.is_ok();
                let res = recipe_written.and_then(|_| {
                    // The baked-pixels link saves/clears with the recipe —
                    // the same pairing Ctrl+S writes.
                    match pix {
                        Some((o, g)) => autoshop::store::write_pixel_source(p, o, *g),
                        None => autoshop::store::clear_pixel_source(p),
                    }
                    .map_err(anyhow::Error::from)
                });
                if recipe_ok
                    && let Some(note) = relook_note.as_deref()
                {
                    eprintln!("⚠ {}: {note}", autoshop::pipeline::stem(p));
                }
                if let Err(e) = res {
                    failed = Some(format!("{}: {e}", autoshop::pipeline::stem(p)));
                    return Ok(());
                }
                // The strip record saves/clears with the recipe (the Ctrl+S
                // pairing): without it the background variants this dialog
                // just listed die with the quit anyway — a fatal result, not
                // a warning.
                let strip_res = match strip {
                    Some(rec) => autoshop::store::write_variants(p, rec),
                    None => autoshop::store::clear_variants(p),
                };
                match strip_res {
                    Ok(()) => {
                        if self.src_path.as_deref() == Some(p.as_path()) {
                            self.saved_strip = strip.clone();
                        }
                    }
                    Err(e) => {
                        failed = Some(format!("{}: {e}", autoshop::pipeline::stem(p)));
                        return Ok(());
                    }
                }
                // NO XMP for a generated entry — Ctrl+S refuses those for the
                // same reason: the look lives in baked pixels no parametric
                // sidecar can reproduce, and writing one here overwrote a real
                // Lightroom sidecar with a lie. The recipe + pixels pair alone
                // restores faithfully.
                //
                // OUTSIDE the fatal result: the recipe write alone decides the
                // saved state (cross-surface rule), so a failed XMP projection
                // must not abort the remaining photos or refuse to quit over a
                // develop that IS durably saved.
                if autoshop::decode::is_raw(p)
                    && !generated
                    && let Err(e) = autoshop::pipeline::write_xmp(p, &disk)
                {
                    xmp_warns.push(format!("{}: {e}", autoshop::pipeline::stem(p)));
                }
                        Ok(())
                    },
                );
                if let Err(e) = locked {
                    failed = Some(format!(
                        "{}: the photo is being changed by another Autoshop process ({e})",
                        autoshop::pipeline::stem(p)
                    ));
                }
                if failed.is_some() {
                    break;
                }
            }
            // ALL said regardless of a later photo's failure (the
            // travels-with-the-mutation rule the repair note above follows),
            // COUNTED (a single-slot accumulator reported only the last of
            // several same-kind failures), and worded per-photo so the
            // sentence stays true on the abort path — the old "develops
            // saved" claimed a completed batch even after a break.
            // stderr keeps the full list for logs; the USER-facing copy is
            // below — an eprintln alone vanished with the closing window,
            // which is exactly the silence W3 exists to end.
            if !xmp_warns.is_empty() {
                eprintln!(
                    "⚠ {} Lightroom XMP projection(s) failed for develop(s) that ARE saved: {}",
                    xmp_warns.len(),
                    xmp_warns.join("; ")
                );
            }
            if !clear_warns.is_empty() {
                eprintln!(
                    "⚠ {} clear(s) succeeded but could not be marked: {} — a sidecar \
                     beside the RAW may restore those edits on the next open",
                    clear_warns.len(),
                    clear_warns.join("; ")
                );
            }
            let mut quit_warns: Vec<String> = Vec::new();
            if !xmp_warns.is_empty() {
                quit_warns.push(trf(
                    lang,
                    "{n} Lightroom XMP projection(s) failed (those develops ARE saved): {detail}",
                    &[("n", &xmp_warns.len().to_string()), ("detail", &brief_list(&xmp_warns))],
                ));
            }
            if !clear_warns.is_empty() {
                quit_warns.push(trf(
                    lang,
                    "{n} clear(s) could not be marked: {detail} — a sidecar beside the RAW may restore those edits on the next open",
                    &[("n", &clear_warns.len().to_string()), ("detail", &brief_list(&clear_warns))],
                ));
            }
            match failed {
                None => {
                    self.nav_stash.clear();
                    self.saved_recipe = self.recipe.clone();
                    self.confirm_quit = false;
                    // The close guard re-checks on the way out. Everything
                    // above just persisted, so the re-check passes — but if
                    // anything is STILL dirty (logic drift, a future writer
                    // this loop misses), the bounce must be SAID: a silently
                    // re-armed dialog is the v0.21 dead-button livelock.
                    let residue = self.inactive_dirty_variants();
                    if residue > 0 {
                        let t = trf(
                            lang,
                            "saved, but {n} variant(s) still count as unsaved — the window stays open; please report this",
                            &[("n", &residue.to_string())],
                        );
                        self.toast(ToastKind::Error, t);
                    } else if !quit_warns.is_empty() {
                        // Everything IS saved (recipe-write-decides), so
                        // refusing to quit would be wrong — but a toast on a
                        // closing window dies unread. Bounce ONCE with the
                        // warnings visible; nothing is dirty any more, so the
                        // next quit passes straight through without this
                        // dialog. One extra click, only in the failure case.
                        let t = trf(
                            lang,
                            "saved with warnings — the window stays open so they can be read; quit again to close: {detail}",
                            &[("detail", &quit_warns.join(" · "))],
                        );
                        self.status = t.clone();
                        self.toast(ToastKind::Error, t);
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Some(e) => {
                    // Stay open: a failed save on quit must never quietly quit.
                    let t = trf(lang, "save failed: {err}", &[("err", &e)]);
                    self.status = t.clone();
                    self.toast(ToastKind::Error, t);
                    self.confirm_quit = false;
                }
            }
        } else if discard_quit {
            self.nav_stash.clear();
            self.saved_recipe = self.recipe.clone();
            // The answer to the guard's question, recorded: these two lines
            // clean the ACTIVE canvas, but a background variant's edits have
            // nowhere to be saved, so nothing can make them "clean" — only
            // the user's decision can end the question.
            self.discard_requested = true;
            self.confirm_quit = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if cancel || !open {
            self.confirm_quit = false;
        }
    }

    /// Migrate every gallery photo's sidecars from a user-picked pre-store
    /// ./out folder (Settings button). Worker-side: one read_dir per photo
    /// over a possibly huge folder is I/O the UI thread must not pay.
    fn start_import_legacy(&mut self, dir: PathBuf) {
        if self.busy {
            return;
        }
        let lang = self.lang;
        let photos = self.gallery.clone();
        if photos.is_empty() {
            self.status = tr(
                lang,
                "Open a folder first — import migrates the photos currently in the gallery",
            )
            .into();
            return;
        }
        self.busy = true;
        self.status = trf(
            lang,
            "Importing develops from {path} …",
            &[("path", &dir.display().to_string())],
        );
        self.spawn_worker(
            move || {
                // One directory scan for the whole gallery — the per-photo
                // migrate_legacy_from re-scanned the (possibly huge) legacy
                // folder once per photo, making import O(photos × entries).
                let n = autoshop::store::migrate_legacy_from_many(&dir, &photos);
                Msg::LegacyImported(Ok(trf(
                    lang,
                    "Imported saved develops for {n} photo(s) from {path}",
                    &[("n", &n.to_string()), ("path", &dir.display().to_string())],
                )))
            },
            |e| Msg::LegacyImported(Err(e)),
        );
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut do_save = false;
        let mut do_fetch = false;
        // `lang` is a Copy snapshot so `tr`/`trf` never borrow `self` — the
        // `let f = &mut self.settings` block below holds a partial borrow of self.
        let lang = self.lang;
        ui.label(
            egui::RichText::new(tr(
                lang,
                "Language & Reverse-fit apply immediately. The provider sections below persist via 「Save settings」 to autoshop.local.json in your per-user Autoshop folder (never in a repo) and apply to the next AI call (Analyze / Fill / Reimagine).",
            ))
            .weak()
            .small(),
        );
        ui.separator();
        ui.heading(tr(lang, "Language"));
        // English is the skeleton; Chinese is an overlay. Switching takes effect
        // next frame (every label re-reads `self.lang`), no restart/save needed.
        // `from_id_salt` is egui 0.29's name for the old `from_id_source`.
        egui::ComboBox::from_id_salt("lang_picker")
            .selected_text(self.lang.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.lang, Lang::En, Lang::En.label());
                ui.selectable_value(&mut self.lang, Lang::Zh, Lang::Zh.label());
            });
        ui.separator();
        ui.heading(tr(lang, "Theme"));
        // Two complete looks (see ThemeColors) — switching re-installs the
        // egui style this frame and persists with the other prefs.
        let before_theme = self.theme;
        egui::ComboBox::from_id_salt("theme_picker")
            .selected_text(tr(lang, self.theme.label()))
            .show_ui(ui, |ui| {
                for t in [ThemePref::Dark, ThemePref::Light] {
                    ui.selectable_value(&mut self.theme, t, tr(lang, t.label()));
                }
            });
        if self.theme != before_theme {
            install_theme(ui.ctx(), self.theme);
        }
        ui.separator();
        ui.heading(tr(lang, "Reverse-fit"));
        ui.checkbox(&mut self.zoned_fit, tr(lang, "Zoned fit (sky)")).on_hover_text(tr(
            lang,
            "On reverse-fit, auto-split the sky on both sides and colour-correct sky↔sky separately (exposure / recolour gains / saturation, bitmap mask). Masks are rendered by the local engine; the LR sidecar carries only the global part. Needs the python segmentation deps (transformers + torch); falls back to pure global reverse-fit when unavailable, noting it in the rationale.",
        ));
        ui.separator();
        // The develop store is otherwise invisible (hashed AppData folders) —
        // this is the one place that names it and rescues pre-store saves.
        ui.heading(tr(lang, "Develop store"));
        ui.label(
            egui::RichText::new(autoshop::store::store_root().display().to_string())
                .small()
                .weak(),
        )
        .on_hover_text(tr(
            lang,
            "Where saved develops live: recipes, Lightroom XMP, version snapshots and mask rasters — one folder per photo, keyed by its absolute path. Override the location with the AUTOSHOP_DATA_DIR environment variable.",
        ));
        if ui
            .button(tr(lang, "Import develops from an old ./out folder…"))
            .on_hover_text(tr(
                lang,
                "Saves made before v0.13 lived in a ./out folder next to wherever the app was launched. If your old edits are missing, point this at that folder — its recipes / XMP / versions migrate into the develop store.",
            ))
            .clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            self.start_import_legacy(dir);
        }
        {
            let f = &mut self.settings;
            // Fetched ids belong to the endpoint recorded at fetch time; once
            // the Base/Bridge URL stops matching (typed edit, provider
            // auto-swap), they describe a DIFFERENT server — self-invalidate
            // so the pickers fall back to grounded defaults, not a stale menu.
            if (!f.chat_choices.is_empty() || !f.image_gen_choices.is_empty())
                && !same_base(&f.image_base_url, &f.models_from_base)
            {
                f.chat_choices.clear();
                f.image_gen_choices.clear();
            }
            ui.separator();
            ui.heading(tr(lang, "Analysis — the verifier"));
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Provider"));
                let r1 = ui.radio_value(&mut f.analysis_provider_api, false, tr(lang, "OAuth (Claude CLI)"));
                let r2 = ui.radio_value(&mut f.analysis_provider_api, true, tr(lang, "API (OpenAI-compatible)"));
                if r1.changed() || r2.changed() {
                    // Without this the OTHER provider's model id stays in the
                    // field and the picker presents it as the current choice
                    // (a claude alias sent to an OpenAI endpoint, or vice
                    // versa). Swap to this provider's default on a flip.
                    let claude_alias =
                        matches!(f.analysis_model.as_str(), "opus" | "sonnet" | "haiku");
                    if f.analysis_provider_api && claude_alias {
                        f.analysis_model = "gpt-5.5".into();
                    } else if !f.analysis_provider_api && !claude_alias {
                        f.analysis_model = "opus".into();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Model"));
                // OAuth uses Claude CLI aliases; API uses the fetched OpenAI chat ids,
                // but only when the analysis endpoint matches the one we fetched from
                // (the image key/base) — otherwise those ids may not exist there.
                let opts = if f.analysis_provider_api {
                    let fetched = if same_base(&f.analysis_base_url, &f.image_base_url) {
                        f.chat_choices.as_slice()
                    } else {
                        &[]
                    };
                    model_opts(fetched, &["gpt-5.5", "gpt-4o"], &f.analysis_model)
                } else {
                    model_opts(&[], &["opus", "sonnet", "haiku"], &f.analysis_model)
                };
                model_picker(ui, "set_analysis_model", &mut f.analysis_model, &opts, lang);
            });
            if f.analysis_provider_api {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Base URL"));
                    ui.text_edit_singleline(&mut f.analysis_base_url);
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "API Key"));
                    let hint = if f.analysis_key_present { tr(lang, "key set — blank keeps it") } else { tr(lang, "no key set") };
                    ui.add(egui::TextEdit::singleline(&mut f.analysis_api_key).password(true).hint_text(hint));
                });
            }
            ui.separator();
            ui.heading(tr(lang, "Image — the vision proposer + generative edits"));
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Provider"));
                ui.radio_value(&mut f.image_provider_oauth, false, tr(lang, "API (OpenAI-compatible)"));
                ui.radio_value(&mut f.image_provider_oauth, true, tr(lang, "OAuth (Codex bridge / ChatGPT sub)"));
            });
            // Flipping into OAuth while the endpoint is still empty or the stock
            // OpenAI host means the field is wrong for a subscription bridge —
            // swap in the loopback bridge default so it works without retyping.
            // Idempotent: stops once the user sets any other (custom) value.
            if f.image_provider_oauth {
                let b = f.image_base_url.trim();
                if b.is_empty() || b.trim_end_matches('/') == OPENAI_DEFAULT_URL {
                    f.image_base_url = CODEX_BRIDGE_URL.to_string();
                }
            } else {
                // Mirror image of the swap above: flipping BACK to API mode
                // with the auto-installed loopback bridge URL still in the
                // field would send real-API calls at a local bridge that may
                // not even be running. Idempotent, stops at custom values.
                let b = f.image_base_url.trim();
                if b.is_empty() || b.trim_end_matches('/') == CODEX_BRIDGE_URL.trim_end_matches('/') {
                    f.image_base_url = OPENAI_DEFAULT_URL.to_string();
                }
            }
            ui.horizontal(|ui| {
                let label = if f.fetching_models { tr(lang, "fetching…") } else { tr(lang, "🔄 Fetch models") };
                let clicked = ui
                    .add_enabled(!f.fetching_models, egui::Button::new(label))
                    .on_hover_text(tr(
                        lang,
                        "List the models this endpoint serves (GET /models) so you can pick instead of guess — and a live reachability check for the bridge/API. Uses the key/token typed below, or the saved one if blank.",
                    ))
                    .clicked();
                if clicked {
                    do_fetch = true;
                }
                if !f.chat_choices.is_empty() || !f.image_gen_choices.is_empty() {
                    let cn = f.chat_choices.len().to_string();
                    let im = f.image_gen_choices.len().to_string();
                    ui.label(
                        egui::RichText::new(trf(lang, "{chat} chat · {image} image", &[("chat", &cn), ("image", &im)]))
                            .weak()
                            .small(),
                    );
                }
            });
            ui.horizontal(|ui| {
                ui.label(if f.image_provider_oauth { tr(lang, "Bridge URL") } else { tr(lang, "Base URL") });
                ui.text_edit_singleline(&mut f.image_base_url);
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Vision model"));
                let opts = model_opts(&f.chat_choices, &["gpt-5.5", "gpt-4o"], &f.image_model);
                model_picker(ui, "set_vision_model", &mut f.image_model, &opts, lang);
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Image-gen model"));
                // OAuth (subscription) exposes gpt-image-2 first; API keys often
                // still prefer gpt-image-1.5 for its input_fidelity lock.
                let fallbacks: &[&str] = if f.image_provider_oauth {
                    &["gpt-image-2", "gpt-image-1.5"]
                } else {
                    &["gpt-image-1.5", "gpt-image-2", "gpt-image-1", "gpt-image-1-mini", "chatgpt-image-latest"]
                };
                let opts = model_opts(&f.image_gen_choices, fallbacks, &f.image_gen_model);
                model_picker(ui, "set_imagegen_model", &mut f.image_gen_model, &opts, lang);
            });
            ui.horizontal(|ui| {
                ui.label(if f.image_provider_oauth { tr(lang, "Gate token") } else { tr(lang, "API Key") });
                let hint = if f.image_key_present {
                    tr(lang, "set — blank keeps it")
                } else if f.image_provider_oauth {
                    tr(lang, "the bridge's own api-keys token (loopback, not a cloud key)")
                } else {
                    tr(lang, "no key set")
                };
                if ui
                    .add(egui::TextEdit::singleline(&mut f.image_api_key).password(true).hint_text(hint))
                    .changed()
                {
                    // Model availability is CREDENTIAL-dependent, not just
                    // URL-dependent: a different key at the same endpoint can
                    // serve a different catalogue, and the URL-keyed
                    // self-invalidation above can't see that. Typing a new
                    // key drops the fetched lists (the pickers fall back to
                    // grounded defaults until the next fetch).
                    f.chat_choices.clear();
                    f.image_gen_choices.clear();
                }
            });
            let note = if f.image_provider_oauth {
                tr(lang, "OAuth rides your ChatGPT subscription via the local Codex bridge — no OpenAI key. Start the bridge first (else edits fail to connect). Generative output is capped at ~1.5 MP by the subscription image tier; for full-resolution edits switch to API mode with a real key.")
            } else {
                tr(lang, "Tip: gpt-image-1.5 keeps the photo most faithful (input_fidelity); newer models like gpt-image-2 ignore that lock and edit more freely.")
            };
            ui.label(egui::RichText::new(note).weak().small());
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Save settings")).clicked() {
                    do_save = true;
                }
                if !f.status.is_empty() {
                    ui.label(egui::RichText::new(&f.status).weak().small());
                }
            });
        }
        if do_save {
            self.save_settings_form();
        }
        if do_fetch {
            self.fetch_models();
        }
    }

    /// Queue a thumbnail decode for `idx` if it isn't cached/queued and we're
    /// under the concurrency cap. Uses the camera's embedded preview (fast) — the
    /// double-processing concern only applies to the develop base, not a 56px chip.
    /// A persistent disk cache (keyed on path+mtime+size) makes every later
    /// session — and every scroll-back after texture eviction — a ~1 ms JPEG
    /// read instead of a full decode.
    fn request_thumb(&mut self, idx: usize) {
        if self.thumbs.contains_key(&idx) || self.thumb_requested.contains(&idx) {
            return;
        }
        // A BOUNDED retry, because both extremes are wrong. Dropping the
        // marker on failure re-requested every frame — six decode threads per
        // 100 ms for as long as the row was visible. Keeping it forever
        // blanked the row for the session, and the LRU that was supposed to
        // provide the retry only prunes above THUMB_TEX_CAP, so a folder
        // under that size never retried at all. Three attempts covers the
        // transient causes (AV lock, a slow share) and stops dead on a file
        // that simply cannot be decoded.
        if self.thumb_fail.get(&idx).is_some_and(|n| *n >= 3) {
            return;
        }
        if self.thumb_inflight >= MAX_THUMB_INFLIGHT {
            return;
        }
        let Some(path) = self.gallery.get(idx).cloned() else { return };
        self.thumb_requested.insert(idx);
        self.thumb_inflight += 1;
        let generation = self.gallery_gen;
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<image::DynamicImage> {
                    let cache = thumb_cache_file(&path);
                    if let Some(p) = &cache
                        && let Ok(img) = image::open(p)
                    {
                        return Ok(img);
                    }
                    // Large baked rasters (the app's own 60 MP TIFF/PNG exports)
                    // decode whole before the 160px shrink — ~360 MB each, and
                    // MAX_THUMB_INFLIGHT of them at once spiked to ~2 GB. Gate
                    // decodes above 24 MP behind one permit; the header read is
                    // cheap and RAWs (embedded-JPEG previews) stay concurrent.
                    let _big_permit = if !autoshop::decode::is_raw(&path)
                        && image::ImageReader::open(&path)
                            .and_then(|r| r.into_dimensions().map_err(std::io::Error::other))
                            .is_ok_and(|(w, h)| w as u64 * h as u64 > 24_000_000)
                    {
                        Some(big_decode_gate().lock().unwrap_or_else(|p| p.into_inner()))
                    } else {
                        None
                    };
                    let thumb =
                        autoshop::decode::preview_only(&path)?.thumbnail(THUMB_EDGE, THUMB_EDGE);
                    if let Some(p) = &cache {
                        save_thumb_cache(p, &thumb); // best-effort write-through
                    }
                    Ok(thumb)
                })();
                Msg::Thumb { generation, idx, img: Box::new(res) }
            },
            // The Err handler decrements thumb_inflight like any decode failure.
            move |e| Msg::Thumb { generation, idx, img: Box::new(Err(e)) },
        );
    }

    /// Queue the latest preview state on the single CPU worker. Dispatch is O(1):
    /// base pixels are Arc-shared and the recipe is small. While a frame is in
    /// flight, edits only set `dirty`; no parallel render storm is possible.
    fn start_redevelop(&mut self) {
        if self.develop_inflight {
            return;
        }
        let Some(base) = self.base_preview.clone() else {
            self.dirty = false;
            return;
        };
        let recipe = self.recipe.clone();
        let show_clipping = self.show_clipping;
        self.develop_inflight = true;
        self.dirty = false;
        self.spawn_worker(
            move || Msg::Developed(Box::new(Ok(build_preview(base, recipe, show_clipping)))),
            |e| Msg::Developed(Box::new(Err(e))),
        );
    }

    /// Accept one worker-built frame if it still describes the active base +
    /// recipe. Old frames are dropped without touching textures; `dirty` remains
    /// set by the newer edit and starts next. Texture handles are UPDATED in
    /// place, avoiding a manager allocate/free cycle on every slider tick.
    fn finish_redevelop(&mut self, ctx: &egui::Context, done: anyhow::Result<PreviewDone>) {
        self.develop_inflight = false;
        let frame = match done {
            Ok(frame) => frame,
            // Pure preview work has no ordinary error path; only a caught panic
            // lands here. Keep the last good frame and surface the failure —
            // WITHOUT fail(): that helper also clears the global `busy` flag,
            // which the preview path never set, and a panicking preview worker
            // mid-export used to unlock every busy-gated action.
            Err(e) => {
                let text = format!("{}: {e}", tr(self.lang, "preview develop failed"));
                self.status = text.clone();
                self.toast(ToastKind::Error, text);
                return;
            }
        };
        let current = self
            .base_preview
            .as_ref()
            .is_some_and(|base| Arc::ptr_eq(base, &frame.base))
            && self.recipe == frame.recipe;
        if !current {
            // The frame was solved for a recipe that changed mid-flight. The
            // redispatch gate is `dirty && !develop_inflight`, so a caller
            // that mutated the recipe WITHOUT setting dirty (historically the
            // crop path) would freeze the preview here forever — always
            // re-arm, the gate coalesces to a single fresh develop.
            self.dirty = true;
            ctx.request_repaint();
            return;
        }
        self.develop_count += 1;
        self.histogram = Some(frame.histogram);

        // The ColorImage arrives worker-built (see PreviewDone.after) — the UI
        // thread only uploads. The raw RGB is retained so the clipping toggle
        // can rebuild its overlay later without a redevelop.
        if let Some(tex) = &mut self.after_tex {
            tex.set(frame.after, egui::TextureOptions::LINEAR);
        } else {
            self.after_tex =
                Some(ctx.load_texture("after", frame.after, egui::TextureOptions::LINEAR));
        }
        // The clipping layer follows the CURRENT toggle, not the one baked at
        // dispatch: pressing J changes no recipe field, so an in-flight frame
        // is still accepted — its stale decision used to blank the layer under
        // a lit ▲ (or resurrect a ghost layer after J-off).
        let clip = if self.show_clipping {
            // Crop-blanked like the worker path — the J-mid-flight fallback
            // used to warn outside the crop while the worker's layer did not.
            Some(
                frame
                    .clipping
                    .unwrap_or_else(|| clipping_overlay_for(&frame.rgb, frame.recipe.crop)),
            )
        } else {
            None
        };
        self.last_rgb = Some(frame.rgb);
        match clip {
            Some(clip) => {
                if let Some(tex) = &mut self.clip_tex {
                    tex.set(clip, egui::TextureOptions::NEAREST);
                } else {
                    self.clip_tex =
                        Some(ctx.load_texture("clip", clip, egui::TextureOptions::NEAREST));
                }
            }
            None => self.clip_tex = None,
        }
        if let Some(v) = self.variants.get_mut(self.active) {
            if let Some(tex) = &mut v.thumb {
                tex.set(frame.thumb, egui::TextureOptions::LINEAR);
            } else {
                v.thumb = Some(ctx.load_texture("vthumb", frame.thumb, egui::TextureOptions::LINEAR));
            }
        }
        // A global change can alter a Range Mask's coverage reference. The
        // coverage-aware key makes this a cheap no-op for ordinary local effect
        // sliders, whose coverage is independent of their pixel adjustment.
        self.overlay_stale = true;
        ctx.request_repaint();
    }

    /// Toggle the clipping layer WITHOUT a full redevelop: the overlay is a
    /// pure function of the last developed pixels (retained from the last
    /// accepted frame), so the J key / ▲ button / histogram triangles respond
    /// instantly instead of paying a whole develop (100-300 ms at 2560/4096).
    /// Falls back to a redevelop only when no frame is retained yet.
    fn toggle_clipping(&mut self, ctx: &egui::Context) {
        self.show_clipping = !self.show_clipping;
        if !self.show_clipping {
            self.clip_tex = None; // OFF is just dropping the layer
            return;
        }
        match &self.last_rgb {
            Some(rgb) => {
                let clip = clipping_overlay_for(rgb, self.recipe.crop);
                if let Some(tex) = &mut self.clip_tex {
                    tex.set(clip, egui::TextureOptions::NEAREST);
                } else {
                    self.clip_tex =
                        Some(ctx.load_texture("clip", clip, egui::TextureOptions::NEAREST));
                }
            }
            None => self.dirty = true, // nothing retained (fresh open) — worker builds it
        }
    }

    /// (Re)build the translucent red coverage layer for the active mask. A
    /// coverage key prevents local effect sliders from rebuilding this full-
    /// frame raster: Exposure/Temp/Saturation change WHAT happens inside the
    /// mask, not WHERE it applies. Range masks include their PREFIX
    /// reference recipe (earlier masks applied — the engine's own stacking
    /// rule) because their coverage genuinely depends on pixels.
    fn refresh_mask_overlay(&mut self, ctx: &egui::Context) {
        if !self.show_mask_overlay {
            self.mask_overlay_tex = None;
            self.overlay_key = None;
            return;
        }
        let Some(base) = self.base_preview.as_ref() else {
            self.mask_overlay_tex = None;
            self.overlay_key = None;
            return;
        };
        // A hovered row previews its coverage; otherwise use the selection.
        let target = self
            .hover_mask
            .filter(|&i| i < self.recipe.masks.len())
            .or_else(|| self.sel_mask.filter(|&i| i < self.recipe.masks.len()));
        let Some(i) = target else {
            self.mask_overlay_tex = None;
            self.overlay_key = None;
            return;
        };
        let mask = self.recipe.masks[i].clone();
        let mut pre = self.recipe.clone();
        // The PREFIX (masks before this one), not masks-cleared: the engine
        // evaluates a Range Mask on the pixel as it stands when THIS mask
        // runs — masks stack sequentially, so a later mask's range sees
        // earlier masks' output (render.rs apply_masks). A cleared reference
        // judged mask 2's range on pixels mask 0/1 had already moved (CX5-6).
        pre.masks.truncate(i);
        // Geometry runs after develop, so it is not part of a Range Mask's
        // masks-cleared pixel reference. Keep it separately in OverlayKey.
        pre.straighten_deg = 0.0;
        pre.lens_distortion = 0.0;
        pre.crop = None;
        let lp = &self.recipe.lens_profile;
        let key = OverlayKey {
            base: Arc::as_ptr(base) as usize,
            target: i,
            mask: mask.mask.clone(),
            components: mask.components.clone(),
            enabled: mask.enabled,
            range: mask.range,
            amount: mask.amount,
            inverted: mask.inverted,
            reference_recipe: mask.range.is_some().then(|| pre.clone()),
            straighten_deg: self.recipe.straighten_deg,
            lens_distortion: self.recipe.lens_distortion,
            profile_dist_on: lp.distortion_on && !lp.distortion.is_empty(),
            profile_ca_on: lp.ca_on && !lp.ca_r.is_empty() && !lp.ca_b.is_empty(),
        };
        if self.overlay_key.as_ref() == Some(&key) && self.mask_overlay_tex.is_some() {
            return;
        }

        // A geometry-only mask never reads reference pixels. Avoid the old
        // second develop entirely; only Range Masks need the masks-cleared
        // developed reference and its recipe-keyed cache.
        // KNOWN COST (C21): this is a ONE-slot cache compared by full-recipe
        // equality, and the reference is now the mask PREFIX rather than a
        // masks-cleared develop — so sweeping the pointer down a list of
        // Range masks alternates prefixes and misses the cache on every row,
        // paying a synchronous preview develop each time. Correctness first:
        // the prefix IS what the engine evaluates a range against, and a
        // wrong overlay is worse than a slow one. Keyed multi-slot caching is
        // the fix and is recorded in the roadmap, not smuggled in here.
        let reference: &image::DynamicImage = if mask.range.is_some() {
            if !matches!(&self.overlay_ref, Some((r, _)) if *r == pre) {
                // Rebuilding the masks-cleared reference is a full develop on
                // the UI thread — the 100-300 ms class (2560/4096) the async
                // preview path exists to hide. Mid-gesture (global-slider
                // drag), keep showing the previous coverage and re-arm the
                // stale flag; the rebuild lands on the frame after the pointer
                // settles. Geometry-only masks (the else arm) stay instant.
                if ctx.input(|i| i.pointer.any_down()) {
                    self.overlay_stale = true;
                    return;
                }
                let img = autoshop::render::develop_preview(base, &pre);
                self.overlay_ref = Some((pre, img));
            }
            &self.overlay_ref.as_ref().expect("range reference cached").1
        } else {
            base.as_ref()
        };
        // Coverage is a display-only translucent layer (LINEAR-filtered when
        // painted), so it never needs more than ~1k resolution — at 2560/4096
        // preview edges the full-frame raster + RGBA pass dominated the
        // rebuild. Range weights are judged on the downscaled reference: the
        // same pixels box-filtered, indistinguishable for an overlay.
        let small;
        let cov_ref: &image::DynamicImage = if reference.width().max(reference.height()) > 1024 {
            small = reference.thumbnail(1024, 1024);
            &small
        } else {
            reference
        };
        let mut cov = image::DynamicImage::ImageLuma8(autoshop::render::mask_coverage(&mask, cov_ref));
        if self.recipe.lens_profile.geometry_active() || self.recipe.lens_distortion != 0.0 {
            // The coverage overlay must follow the SAME geometric chain as the
            // rendered pixels — profile distortion included, or the red wash
            // drifts off its mask near the frame edges.
            cov = autoshop::render::apply_lens_geometry(
                &cov,
                &self.recipe.lens_profile,
                self.recipe.lens_distortion,
            );
        }
        if self.recipe.straighten_deg != 0.0 {
            cov = autoshop::render::rotate_straighten(&cov, self.recipe.straighten_deg);
        }
        let g = cov.to_luma8();
        let (w, h) = (g.width() as usize, g.height() as usize);
        let mut rgba = vec![0u8; w * h * 4];
        for (i, p) in g.pixels().enumerate() {
            rgba[i * 4..i * 4 + 4]
                .copy_from_slice(&[255, 40, 40, (p[0] as u16 * 140 / 255) as u8]);
        }
        let colour = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        if let Some(tex) = &mut self.mask_overlay_tex {
            tex.set(colour, egui::TextureOptions::LINEAR);
        } else {
            self.mask_overlay_tex =
                Some(ctx.load_texture("mask_overlay", colour, egui::TextureOptions::LINEAR));
        }
        self.overlay_key = Some(key);
        self.overlay_build_count += 1;
    }

    /// One-line echo of the current delivery settings for the Export /
    /// Download hover — e.g. "JPEG · 2560 px · q95 · sRGB (universal)" — so
    /// the state stays glanceable now that the settings live in the Export
    /// section instead of a toolbar row.
    fn export_summary(&self, lang: Lang) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(if self.save_jpeg { "JPEG".into() } else { tr(lang, "16-bit TIFF").to_string() });
        parts.push(if self.exp_long_edge == 0 {
            tr(lang, "Original size").to_string()
        } else {
            format!("{} px", self.exp_long_edge)
        });
        if self.save_jpeg {
            parts.push(format!("q{:.0}", self.exp_quality));
        }
        if self.exp_sharpen > 0.0 {
            parts.push(format!("{} {:.0}", tr(lang, "Output sharpening"), self.exp_sharpen));
        }
        parts.push(tr(lang, EXPORT_SPACES[(self.exp_space as usize).min(2)]).to_string());
        if self.save_denoise {
            parts.push(tr(lang, "AI Denoise").to_string());
        }
        parts.join(" · ")
    }

    /// The geometric mapping context every interaction boundary needs:
    /// original preview pixel dims + the current straighten angle + the lens
    /// geometry (in-camera profile + manual distortion amount).
    fn geom_ctx(&self) -> ((f32, f32), f32, LensArg) {
        let dims = self
            .base_preview
            .as_ref()
            .map(|b| {
                let (w, h) = b.dimensions();
                (w as f32, h as f32)
            })
            .unwrap_or((1.0, 1.0));
        (
            dims,
            self.recipe.straighten_deg,
            LensArg {
                profile: self.recipe.lens_profile.clone(),
                amount: self.recipe.lens_distortion,
            },
        )
    }

    /// Draw the live histogram (R/G/B filled, luma outline; one bin per 8-bit
    /// code value on ONE shared vertical scale) — the tone readout a photo
    /// editor is expected to have. Sqrt-scaled so shadow detail reads.
    /// The corner triangles are LR's clipping indicators: lit when pixels sit
    /// at the J-overlay thresholds (≤1 / ≥254); clicking toggles that overlay.
    fn histogram_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let Some(hist) = &self.histogram else { return };
        let h = 72.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), h),
            egui::Sense::hover(),
        );
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 3.0, egui::Color32::from_gray(16));
        let n = hist.len().max(1);
        let bar_w = rect.width() / n as f32;
        // Additive-ish RGB: draw each channel as translucent filled bars.
        let colors = [
            egui::Color32::from_rgba_unmultiplied(220, 70, 70, 110),
            egui::Color32::from_rgba_unmultiplied(90, 200, 90, 110),
            egui::Color32::from_rgba_unmultiplied(90, 130, 240, 110),
        ];
        for (ch, color) in colors.iter().enumerate() {
            for (i, bins) in hist.iter().enumerate() {
                let v = bins[ch].sqrt(); // sqrt: make shadow counts visible
                if v <= 0.0 {
                    continue;
                }
                let x0 = rect.min.x + i as f32 * bar_w;
                let y0 = rect.max.y - v * (h - 2.0);
                p.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x0 + bar_w, rect.max.y)),
                    0.0,
                    *color,
                );
            }
        }
        // Luma as a thin outline on top for the overall tone shape.
        let pts: Vec<egui::Pos2> = hist
            .iter()
            .enumerate()
            .map(|(i, bins)| {
                egui::pos2(
                    rect.min.x + (i as f32 + 0.5) * bar_w,
                    rect.max.y - bins[3].sqrt() * (h - 2.0),
                )
            })
            .collect();
        p.add(egui::Shape::line(pts, egui::Stroke::new(1.0, egui::Color32::from_gray(210))));

        // Clipping triangles, per-channel (the LR convention): the colour
        // names WHICH channels sit in the extreme bin — one channel reads as
        // that primary, two mix to yellow/magenta/cyan, all three to white
        // (a neutral crush/blow-out vs. a colour cast at a glance). Shadows
        // top-left, highlights top-right; grey when clean; click = the same
        // toggle as ▲ / J.
        let tri_color = |bins: &[f32; 4]| -> Option<egui::Color32> {
            let (r, g, b) = (bins[0] > 0.0, bins[1] > 0.0, bins[2] > 0.0);
            (r || g || b).then(|| {
                let c = |on: bool| if on { 255u8 } else { 45 };
                egui::Color32::from_rgb(c(r), c(g), c(b))
            })
        };
        let chan_names = |bins: &[f32; 4]| -> String {
            ["R", "G", "B"]
                .iter()
                .zip(bins)
                .filter(|(_, v)| **v > 0.0)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join("+")
        };
        let mut toggle = false;
        // The triangles judge the SAME thresholds as the J overlay (≤1 / ≥254):
        // with one bin per code value that is the two extreme bins summed. The
        // old 64-bin "extreme bin" spanned code values 0-3 / 252-255, so the
        // warning lit on near-clipped pixels the overlay never marked.
        let edge2 = |a: &[f32; 4], b: &[f32; 4]| -> [f32; 4] {
            std::array::from_fn(|ch| a[ch] + b[ch])
        };
        let second = hist.get(1).unwrap_or(&hist[0]);
        let shadow = edge2(&hist[0], second);
        let highlight = edge2(
            &hist[hist.len() - 1],
            hist.get(hist.len().wrapping_sub(2)).unwrap_or(&hist[hist.len() - 1]),
        );
        for (right, bins, what) in [
            (false, &shadow, "shadow crush"),
            (true, &highlight, "highlight clip"),
        ] {
            let s = 10.0;
            let x0 = if right { rect.max.x - s - 4.0 } else { rect.min.x + 4.0 };
            let tri = egui::Rect::from_min_size(egui::pos2(x0, rect.min.y + 4.0), egui::vec2(s, s));
            let lit = tri_color(bins);
            let tip = match lit {
                Some(_) => trf(
                    lang,
                    "{what}: {chan} channel(s) — click to toggle clipping warning (J)",
                    &[("what", tr(lang, what)), ("chan", &chan_names(bins))],
                ),
                None => trf(
                    lang,
                    "{what} indicator (clean) — click to toggle clipping warning (J)",
                    &[("what", tr(lang, what))],
                ),
            };
            let resp = ui
                .interact(tri, ui.id().with(("clip_tri", right)), egui::Sense::click())
                .on_hover_text(tip);
            let color = lit.unwrap_or(if self.show_clipping {
                self.theme.colors().clip_tri_on
            } else {
                self.theme.colors().clip_tri_off
            });
            p.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(tri.center().x, tri.min.y),
                    egui::pos2(tri.max.x, tri.max.y),
                    egui::pos2(tri.min.x, tri.max.y),
                ],
                color,
                egui::Stroke::NONE,
            ));
            if resp.clicked() {
                toggle = true;
            }
        }
        if toggle {
            let ctx = ui.ctx().clone();
            self.toggle_clipping(&ctx); // instant — rebuilt from the retained frame
        }
    }

    /// The interactive tone-curve editor: a channel picker (master / R / G / B)
    /// over a painted square — histogram backdrop, quarter grid, and the curve
    /// drawn straight from `render::curve_lut`, the SAME sampler the engine
    /// applies (so the preview line can never drift from the render). Click
    /// adds a point ON the curve, dragging moves it (inputs stay strictly
    /// increasing), dragging well outside the box deletes it — the Lightroom
    /// gestures. Returns true when the recipe changed this frame.
    fn curve_editor(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let label_colors = self.theme.colors().curve_labels;
        let mut changed = false;
        ui.horizontal(|ui| {
            for (i, (name, _)) in CURVE_CHANNELS.iter().enumerate() {
                if ui
                    .selectable_label(
                        self.curve_channel == i,
                        egui::RichText::new(tr(lang, name)).color(label_colors[i]).small(),
                    )
                    .clicked()
                {
                    self.curve_channel = i;
                    self.curve_drag = None;
                }
            }
            if ui.small_button("↺").on_hover_text(tr(lang, "Clear the current channel's curve")).clicked() {
                let pts = curve_points_mut(&mut self.recipe, self.curve_channel);
                if !pts.is_empty() {
                    pts.clear();
                    changed = true;
                }
                self.curve_drag = None;
            }
        });

        let side = ui.available_width().clamp(160.0, 240.0);
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click_and_drag());
        #[cfg(test)]
        {
            // Test seam (the same convention as develop_count /
            // overlay_build_count): the headless driven test aims its
            // synthetic click with the square's screen rect.
            self.curve_rect = Some(rect);
        }
        let p = ui.painter_at(rect);
        let accent = CURVE_CHANNELS[self.curve_channel].1;
        p.rect_filled(rect, 3.0, egui::Color32::from_gray(16));

        // Value space: x = input 0..1 (left→right), y = output 0..1 (bottom→top).
        let to_screen = |x: f32, y: f32| {
            egui::pos2(rect.min.x + x * rect.width(), rect.max.y - y * rect.height())
        };
        let to_val = |q: egui::Pos2| {
            (
                ((q.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0),
                ((rect.max.y - q.y) / rect.height().max(1.0)).clamp(0.0, 1.0),
            )
        };

        // Histogram backdrop for the active channel (luma behind the master curve)
        // — same data as the panel histogram, sqrt-scaled the same way.
        if let Some(hist) = &self.histogram {
            let ch = [3usize, 0, 1, 2][self.curve_channel];
            let bar_w = rect.width() / hist.len().max(1) as f32;
            let fill =
                egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 34);
            for (i, bins) in hist.iter().enumerate() {
                let v = bins[ch].sqrt();
                if v <= 0.0 {
                    continue;
                }
                let x0 = rect.min.x + i as f32 * bar_w;
                p.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x0, rect.max.y - v * (rect.height() - 2.0)),
                        egui::pos2(x0 + bar_w, rect.max.y),
                    ),
                    0.0,
                    fill,
                );
            }
        }

        // Quarter grid + the identity diagonal for reference.
        let grid = egui::Stroke::new(1.0, egui::Color32::from_gray(38));
        for i in 1..4 {
            let t = i as f32 / 4.0;
            p.line_segment([to_screen(t, 0.0), to_screen(t, 1.0)], grid);
            p.line_segment([to_screen(0.0, t), to_screen(1.0, t)], grid);
        }
        p.line_segment(
            [to_screen(0.0, 0.0), to_screen(1.0, 1.0)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(56)),
        );

        // --- interaction (mutates the active channel's control points) --------
        const HIT: f32 = 10.0; // grab radius around a point, screen px
        let lut_before =
            autoshop::render::curve_lut(curve_points(&self.recipe, self.curve_channel));
        let pts = curve_points_mut(&mut self.recipe, self.curve_channel);
        if (resp.drag_started() || resp.clicked())
            && let Some(q) = resp.interact_pointer_pos()
        {
            let near = pts.iter().position(|c| {
                to_screen(c.input as f32 / 255.0, c.output as f32 / 255.0).distance(q) <= HIT
            });
            let idx = match near {
                Some(i) => i,
                None => {
                    // Add ON the current curve at the clicked input — the shape
                    // doesn't jump; the user then drags the new point away.
                    let (vx, _) = to_val(q);
                    let input = (vx * 255.0).round() as u8;
                    let output = (lut_before[input as usize] * 255.0).round() as u8;
                    changed = true;
                    insert_curve_point(pts, input, output)
                }
            };
            if resp.drag_started() {
                self.curve_drag = Some(idx);
            }
        }
        if let Some(i) = self.curve_drag.filter(|&i| i < pts.len()) {
            if resp.dragged()
                && let Some(q) = resp.interact_pointer_pos()
            {
                if rect.expand(28.0).contains(q) {
                    let (vx, vy) = to_val(q);
                    drag_curve_point(
                        pts,
                        i,
                        (vx * 255.0).round() as u8,
                        (vy * 255.0).round() as u8,
                    );
                } else {
                    pts.remove(i); // dragged well outside → delete (LR gesture)
                    self.curve_drag = None;
                }
                changed = true;
            }
            if resp.drag_stopped() {
                self.curve_drag = None;
            }
        }

        // Engine-faithful curve line: all 256 samples straight from the shared LUT.
        let lut = autoshop::render::curve_lut(pts);
        let line: Vec<egui::Pos2> = lut
            .iter()
            .enumerate()
            .map(|(i, &y)| to_screen(i as f32 / 255.0, y))
            .collect();
        p.add(egui::Shape::line(line, egui::Stroke::new(1.6, accent)));

        // Control-point handles (the dragged one filled with the channel colour).
        for (i, c) in pts.iter().enumerate() {
            let q = to_screen(c.input as f32 / 255.0, c.output as f32 / 255.0);
            if self.curve_drag == Some(i) {
                p.circle_filled(q, 5.0, accent);
            } else {
                p.circle_filled(q, 3.5, egui::Color32::from_gray(230));
                p.circle_stroke(q, 3.5, egui::Stroke::new(1.0, egui::Color32::from_gray(60)));
            }
        }

        ui.label(
            egui::RichText::new(tr(
                self.lang,
                "Click to add a point · drag to move · drag outside the box to delete — preview and export match (XMP carries the closest Lightroom form)",
            ))
            .weak()
            .small(),
        );
        changed
    }

    fn start_analyze(&mut self, refine: bool) {
        let Some(path) = self.src_path.clone() else { return };
        // `busy` alone stopped guarding this the moment Analyze became
        // cancellable: ✕ clears it while the request is still on the wire and
        // still billing. See `analyze_inflight`.
        if self.busy || self.analyze_inflight {
            return;
        }
        let lang = self.lang;
        self.busy = true;
        self.status = if refine {
            tr(lang, "refining your current edit with AI…").into()
        } else {
            tr(lang, "analyzing with AI (GPT + Claude)…").into()
        };
        let style = self.style_strength;
        // Free-text direction ("warmer, moodier") steers the proposal; with
        // `refine` (its own button now — no pre-armed checkbox), the AI
        // ADJUSTS the current recipe instead of starting from scratch. A
        // box-selected region (if any) folds into the direction so the AI
        // masks exactly there — same prompt the web UI sends.
        let guidance = {
            let g = self.guidance.trim();
            match self.region {
                Some([l, t, r, b]) => Some(format!(
                    "The user SELECTED a target region (normalized 0..1 frame coords): \
                     left={l:.3} top={t:.3} right={r:.3} bottom={b:.3}. Apply the direction ONLY to \
                     that region — emit a mask covering it (a radial mask with those exact \
                     left/top/right/bottom bounds and feather ~0.4 is ideal, or a linear gradient \
                     for a thin edge band). Direction: {}",
                    if g.is_empty() { "make a tasteful local improvement" } else { g }
                )),
                None => (!g.is_empty()).then(|| g.to_string()),
            }
        };
        let base = refine.then(|| {
            // UNSTRIPPED: produce_recipe strips the base look + lens profile
            // from its PROMPT copy itself now, and carry_over_unrepresentable
            // needs the user's REAL profile to preserve unsaved lens toggles
            // — the pre-strip here made Refine revert them to the saved
            // profile (the same defect the web caller had, fixed in B48).
            self.recipe.clone()
        });
        // Every other network starter arms this; Analyze — the one a stalled
        // endpoint holds the longest — did not, so a hung stream left no ✕
        // and only killing the process recovered. Abandon semantics (like the
        // local-compute starters): the advisor has no cancel checkpoints, so
        // ✕ frees the UI immediately, the call dies at its stall/progress
        // deadline, and the stale-epoch result is discarded on arrival.
        let (epoch, _flag) = self.arm_cancel();
        self.analyze_inflight = true;
        self.spawn_worker(
            move || {
                // Config is reloaded in-thread (cheap) so we don't need it to be Clone.
                let cfg = autoshop::config::Config::load();
                let res = autoshop::pipeline::produce_recipe(
                    &path,
                    &cfg,
                    false,
                    guidance.as_deref(),
                    base.as_ref(),
                    style,
                );
                Msg::Analyzed(epoch, Box::new(res))
            },
            move |e| Msg::Analyzed(epoch, Box::new(Err(e))),
        );
    }

    /// `./out/<stem>.developed.{tif|jpg}` — the default export target. The stem
    /// follows the ACTIVE variant's pixel source, so a Generated variant exports
    /// under its reimagine stem (and its AI pixels), not the original's.
    fn default_out(&self) -> PathBuf {
        let src = self.active_source_path();
        let stem = src
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("out")
            .to_string();
        let ext = if self.save_jpeg { "jpg" } else { "tif" };
        PathBuf::from("out").join(format!("{stem}.developed.{ext}"))
    }

    /// Render the full-resolution develop to `out` on a worker thread (16-bit
    /// TIFF, or 8-bit JPEG when the path ends in .jpg). Renders the ACTIVE
    /// variant's pixel source (a Generated variant → its full-res reimagine PNG,
    /// developed by the recipe), so what exports matches what's on screen.
    fn start_render_to(&mut self, out: PathBuf) {
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang;
        // Same library-read-only gate every CLI export runs: without it,
        // Download…'s save dialog was the one door that could drop a
        // developed.tif beside the source RAW. Guard BOTH anchors: `path` is
        // the render source, which for a Generated variant is the ./out PNG —
        // guarding only that would leave the ORIGINAL photo's library folder
        // unprotected exactly when a generated variant is active.
        let mut anchors = vec![path.clone()];
        if let Some(orig) = self.src_path.clone()
            && orig != path
        {
            anchors.push(orig);
        }
        for a in &anchors {
            if let Err(e) = autoshop::pipeline::guard_readonly(&out, a) {
                let t = e.to_string();
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
                return;
            }
        }
        self.busy = true;
        self.status = if self.save_denoise {
            trf(
                lang,
                "rendering + AI denoise → {path} … (GPU sidecar, can take minutes)",
                &[("path", &out.display().to_string())],
            )
        } else {
            trf(lang, "rendering full-resolution → {path} …", &[("path", &out.display().to_string())])
        };
        let recipe = self.recipe.clone();
        let denoise = self.save_denoise;
        let export = self.export_opts();
        let src_photo = self.src_path.clone();
        self.spawn_worker(
            move || {
                let res = (|| {
                    if let Some(p) = out.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    // Every deliverable repairs (the batch worker's rule):
                    // the canvas normally arrives repaired at open, but when
                    // that repair was an INABILITY (a then-locked file) the
                    // canvas holds the washed curve — and this export shipped
                    // it while a batch export of the SAME canvas repaired it.
                    // Anchored on the PHOTO, like every sibling site: `path`
                    // is the RENDER SOURCE, which for an in-place heal/clone
                    // is the baked master .png — never a RAW — so anchoring
                    // there left the repair permanently dead for exactly the
                    // canvases whose curve still renders (a generated canvas
                    // is stripped and no-ops either way). Off the UI thread,
                    // memo-bounded.
                    let mut recipe = recipe;
                    let relook = src_photo
                        .as_deref()
                        .and_then(|p| {
                            autoshop::pipeline::repair_pre_era_base_curve(p, &mut recipe)
                        })
                        .is_some();
                    // SCUNet AI denoise (python sidecar) runs before the develop when on.
                    let opts = denoise.then(|| {
                        autoshop::denoise::DenoiseOpts::from_config(&autoshop::config::Config::load(), None, 1.0)
                    });
                    autoshop::render::render_to_file(&path, &recipe, &out, opts.as_ref(), Some(&export))?;
                    Ok::<String, anyhow::Error>(if relook {
                        format!(
                            "{} — {}",
                            out.display(),
                            tr(
                                lang,
                                "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                            )
                        )
                    } else {
                        out.display().to_string()
                    })
                })();
                Msg::Exported(res)
            },
            |e| Msg::Exported(Err(e)),
        );
    }

    /// Run the AI segmentation sidecar on the ORIGINAL-frame preview and attach
    /// the resulting raster as a Bitmap local mask (gap batch A②). The AI only
    /// picks WHERE — every actual edit stays a deterministic recipe slider.
    fn start_segment(&mut self, target: &'static str, label: &'static str) {
        if self.busy {
            return;
        }
        let lang = self.lang;
        // The STATUS shows a localised name; the mask's persisted `name` stays
        // the stable English label — recipe.json / XMP are data and must
        // survive a language switch (a translated name baked into the sidecar
        // can never be matched or re-keyed again).
        let disp = tr(lang, label).to_string();
        let Some(base) = self.base_preview.clone() else { return };
        let Some(src) = self.src_path.clone() else { return };
        self.busy = true;
        self.status = trf(
            lang,
            "AI segmenting {what}… (first run auto-downloads the model; failures are reported here)",
            &[("what", &disp)],
        );
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<(String, PathBuf)> {
                    let cfg = autoshop::config::Config::load();
                    let opts = autoshop::segment::SegmentOpts::from_config(&cfg, target);
                    // The sidecar sees the ORIGINAL-frame preview — the space recipe
                    // masks live in. Preview resolution is enough: the engine samples
                    // the raster bilinearly in normalised coords at any render size.
                    let mut tmp = std::env::temp_dir();
                    tmp.push(format!("autoshop_seg_{}_{target}.png", std::process::id()));
                    base.to_rgb8()
                        .save(&tmp)
                        .map_err(|e| anyhow::anyhow!("write segmentation input {}: {e}", tmp.display()))?;
                    // A FRESH claimed name per run (mask-sky.png, -2, …):
                    // rewriting one fixed file replaced bytes a SAVED recipe
                    // still referenced before any save — the same corruption
                    // class the zoned-fit raster had. The Segmented handler
                    // re-points the existing mask entry by name FAMILY, so a
                    // rerun still refreshes instead of stacking.
                    let mask = autoshop::store::claim_raster(&src, &format!("mask-{target}"))?;
                    let run = autoshop::segment::segment_file(&opts, &tmp, &mask);
                    let _ = std::fs::remove_file(&tmp);
                    if let Err(e) = run {
                        // Release the claimed name: a failed run leaves its
                        // create_new 0-byte slot (segment.py publishes
                        // atomically, so an error means nothing real landed).
                        let _ = std::fs::remove_file(&mask);
                        return Err(e);
                    }
                    // English label into the recipe (stable data), not `disp`.
                    Ok((label.to_string(), mask))
                })();
                Msg::Segmented(res)
            },
            |e| Msg::Segmented(Err(e)),
        );
    }

    /// The delivery options the export UI currently dials in (gap batch F) —
    /// shared by single export, Download… and batch render.
    fn export_opts(&self) -> autoshop::render::ExportOpts {
        autoshop::render::ExportOpts {
            long_edge: (self.exp_long_edge > 0).then_some(self.exp_long_edge),
            sharpen: self.exp_sharpen.clamp(0.0, 100.0),
            jpeg_quality: self.exp_quality.round().clamp(1.0, 100.0) as u8,
            color_space: match self.exp_space {
                1 => autoshop::render::ExportColorSpace::DisplayP3,
                2 => autoshop::render::ExportColorSpace::AdobeRgb,
                _ => autoshop::render::ExportColorSpace::Srgb,
            },
        }
    }

    /// Batch-render every Ctrl+click-selected photo through its own saved
    /// recipe.json (central store, else legacy ./out; neutral develop when
    /// none exists) with the current export options — "export selected".
    /// Sequential on one worker: each full-res develop is already multi-second
    /// and memory-heavy (61 MP frames), so parallelism would thrash, not speed
    /// up. AI denoise is deliberately excluded (minutes per photo via the GPU
    /// sidecar — run it per-photo from the export panel instead).
    fn start_batch_render(&mut self) {
        if self.busy {
            return;
        }
        let targets: Vec<PathBuf> = {
            let mut idx: Vec<usize> = self.multi_sel.iter().copied().collect();
            idx.sort_unstable(); // report in gallery order, not hash order
            idx.into_iter().filter_map(|i| self.gallery.get(i).cloned()).collect()
        };
        if targets.is_empty() {
            return;
        }
        let ext = if self.save_jpeg { "jpg" } else { "tif" };
        let export = self.export_opts();
        let lang = self.lang; // localise the UI status AND the worker's result strings
        self.busy = true;
        self.status = trf(
            lang,
            "Batch-rendering {n} photos → ./out …",
            &[("n", &targets.len().to_string())],
        );
        self.batch_progress = Some((0, targets.len())); // the top-bar progress bar
        // Interim BatchProgress ticks flow through this extra clone; the
        // TERMINAL Msg::Exported is owned by spawn_worker (panic-safe).
        let tx = self.tx.clone();
        let ext = ext.to_string();
        // In-memory state outranks disk WHOLESALE — recipes AND pixel
        // identities, for every photo that has any: the nav stash holds work
        // navigated away from, and the open photo's live canvas outranks even
        // its own stash entry. Without this, batching a photo you edited (or
        // are looking at) renders the stale (or absent) recipe.json / stale
        // saved pixels, visibly diverging from the screen.
        // path → (recipe, Some((master, generated)) | None = source pixels).
        type BatchOverride = (EditRecipe, Option<(PathBuf, bool)>);
        let mut overrides: std::collections::HashMap<PathBuf, BatchOverride> = self
            .nav_stash
            .iter()
            .map(|(p, st)| {
                (
                    p.clone(),
                    (
                        st.recipe.clone(),
                        st.origin.clone().map(|o| (o, st.kind == VariantKind::Generated)),
                    ),
                )
            })
            .collect();
        if let Some(p) = self.src_path.clone() {
            let pix = self
                .active_variant()
                .and_then(|v| v.origin.clone())
                .map(|o| (o, self.active_is_generated()));
            overrides.insert(p, (self.recipe.clone(), pix));
        }
        self.spawn_worker(
            move || {
                let res = (|| {
                    let total = targets.len();
                    let (mut okn, mut errs) = (0usize, Vec::<String>::new());
                    // A selection can hold two same-stem photos (different
                    // folders). Batch-scope claims keep their deliverables
                    // apart; the summary disloses which photo took which
                    // name. Single exports stay stem-keyed (re-export
                    // replaces in place) — the dedup is batch-level by
                    // decision.
                    let mut names = autoshop::pipeline::BatchNames::default();
                    // Disclosure counter, like `names.renamed`: this worker
                    // was the one reader that repaired with nobody watching.
                    let mut relooked = 0usize;
                    for p in &targets {
                        let one = (|| -> anyhow::Result<()> {
                            let over = overrides.get(p);
                            let recipe = if let Some((lr, _)) = over {
                                lr.clone()
                            } else {
                                // Central store first, then a not-yet-migrated
                                // legacy ./out sidecar; rasters re-anchor to
                                // whichever dir the recipe was read from.
                                let mut found = None;
                                for rj in [
                                    autoshop::store::recipe_target(p),
                                    autoshop::store::legacy_recipe(p),
                                ] {
                                    if rj.exists() {
                                        let mut r = serde_json::from_str::<EditRecipe>(
                                            &std::fs::read_to_string(&rj)?,
                                        )?;
                                        // The one restore path that never went
                                        // through clamp: a stored recipe with
                                        // extreme-but-finite geometry rendered
                                        // NaN weights into a published export.
                                        // render_to_file now clamps too — this
                                        // keeps the batch recipe equal to what
                                        // OPENING the photo would show.
                                        r.clamp();
                                        if let Some(base) = rj.parent() {
                                            autoshop::store::resolve_mask_paths(&mut r, base);
                                        }
                                        found = Some(r);
                                        break;
                                    }
                                }
                                // No saved develop → export what the canvas
                                // WOULD show: neutral + the photo's camera-
                                // matched base look (one extra develop per
                                // photo; without it the batch export of an
                                // unedited RAW comes out on the dark base
                                // while its open canvas shows the bright one).
                                found.unwrap_or_else(|| EditRecipe {
                                    base_curve: autoshop::pipeline::photo_base_knots(p),
                                    lens_profile: autoshop::pipeline::fresh_lens_profile(p),
                                    ..Default::default()
                                })
                            };
                            let out = names.claim(p, "developed", &ext);
                            autoshop::pipeline::ensure_parent(&out)?;
                            // A develop whose pixels are a baked retouch
                            // master renders FROM that master (the recipe
                            // composes on top — the same InPlace contract the
                            // canvas uses); exporting the un-healed source
                            // would silently drop the retouch from the batch.
                            // The override's pixel identity wins over disk —
                            // an unsaved retouch/denoise must export exactly
                            // like its canvas.
                            let pix = match over {
                                Some((_, pix)) => pix.clone(),
                                None => {
                                    let pix = autoshop::store::read_pixel_source(p);
                                    // A recorded-but-unhonourable master:
                                    // exporting would silently drop the
                                    // retouch — fail THIS photo with the
                                    // cause instead (the summary lists it).
                                    if pix.is_none() && autoshop::store::has_pixel_source(p) {
                                        anyhow::bail!(
                                            "the saved retouch master could not be loaded — the export would silently drop the retouch (open the photo for the cause, then re-save or clear it)"
                                        );
                                    }
                                    pix
                                }
                            };
                            let mut recipe = recipe;
                            if pix.as_ref().is_some_and(|(_, generated)| *generated) {
                                // A GENERATED master's look (camera curve AND
                                // lens corrections) already lives in its
                                // pixels — the stamped disk recipe keeps them
                                // for the RAW's own sake, but rendering the
                                // master through them would cook both twice
                                // (the same strip rule the canvas applies).
                                recipe.base_curve = Vec::new();
                                recipe.lens_profile = Default::default();
                                // The anchor follows the strip rule: baked
                                // pixels carry their WB (relative model).
                                recipe.as_shot_k = None;
                                recipe.as_shot_tint = None;
                            }
                            // Repair AFTER the strip, BEFORE the render — the
                            // load_version ordering, for the same reason: a
                            // generated master's curve is deleted either way,
                            // so repairing first paid a decode for an estimate
                            // nothing used, and the summary then claimed a
                            // correction that never reached a pixel. Counted
                            // only when the export LANDS, so a failure summary
                            // cannot claim corrections it did not ship.
                            // (render_to_file does not go through
                            // store::render_source_checked — batch 60 put the
                            // repair there and this path never saw it.)
                            let repaired = autoshop::pipeline::repair_pre_era_base_curve(
                                p, &mut recipe,
                            )
                            .is_some();
                            let src = pix.map(|(m, _)| m).unwrap_or_else(|| p.clone());
                            autoshop::render::render_to_file(&src, &recipe, &out, None, Some(&export))?;
                            if repaired {
                                relooked += 1;
                            }
                            Ok(())
                        })();
                        match one {
                            Ok(()) => okn += 1,
                            Err(e) => errs.push(format!("{}: {e}", autoshop::pipeline::stem(p))),
                        }
                        let _ = tx.send(Msg::BatchProgress { done: okn + errs.len(), total });
                    }
                    // Same-stem photos were kept apart — disclose WHICH
                    // photo took WHICH name, or the user hunts for an export
                    // that "vanished" (it was never under the bare name).
                    let renames = if names.renamed.is_empty() {
                        String::new()
                    } else {
                        trf(
                            lang,
                            " · same-name photos kept apart: {list}",
                            &[("list", &names.renamed.join(", "))],
                        )
                    };
                    let relook = if relooked == 0 {
                        String::new()
                    } else {
                        trf(
                            lang,
                            " · {n} base look(s) re-estimated (a pre-era save rendered too dark)",
                            &[("n", &relooked.to_string())],
                        )
                    };
                    if errs.is_empty() {
                        Ok(format!(
                            "{}{renames}{relook}",
                            trf(lang, "./out — batch {n} done", &[("n", &okn.to_string())])
                        ))
                    } else {
                        anyhow::bail!(
                            "{}{renames}{relook}",
                            trf(
                                lang,
                                "Batch: {ok} succeeded, {fail} failed: {detail}",
                                &[
                                    ("ok", &okn.to_string()),
                                    ("fail", &errs.len().to_string()),
                                    ("detail", &errs.join("; ")),
                                ],
                            )
                        )
                    }
                })();
                Msg::Exported(res)
            },
            |e| Msg::Exported(Err(e)),
        );
    }

    fn start_export(&mut self) {
        let out = self.default_out();
        self.start_render_to(out);
    }

    /// Save this photo's develop to the central store: recipe.json for every
    /// source type + the Lightroom / Camera-Raw XMP projection for RAW.
    /// An XMP reproduces a look via develop PARAMETERS; a Generated variant's
    /// look lives in its pixels, not the recipe, so there's nothing faithful to
    /// write — steer the user to 反推 (which produces a Fitted variant whose XMP
    /// IS the look). Always keyed to the original RAW `src_path`.
    fn save_xmp(&mut self) {
        // Flush a typed-but-uncommitted mask rename FIRST (U10): every save
        // entry point (Ctrl+S, the Save develop button) must write the name
        // the user sees in the box, not the pre-focus snapshot.
        self.commit_mask_name_buf();
        let lang = self.lang;
        if self.active_is_generated() {
            // A keyboard Ctrl+S refusal must be SEEN — the status line alone
            // scrolls away under the very next message.
            let t = tr(
                lang,
                "A generated variant's look lives in its pixels — there's no parametric recipe to export; run 「Reverse-fit」 first to get an exportable XMP",
            );
            self.status = t.into();
            self.toast(ToastKind::Error, t);
            return;
        }
        let Some(path) = self.src_path.clone() else { return };
        // Reset-then-save means "clear my edits": writing a neutral pair would
        // only pin a misleading ● badge with no in-app way to remove it —
        // delete the working sidecars instead (version snapshots are kept).
        // BOTH homes are cleared: the central store AND any legacy ./out file
        // a pre-store build left behind — a surviving legacy sidecar would
        // resurrect the "cleared" edits through the read fallbacks.
        // A baked pixel retouch IS an edit even under a neutral recipe — a
        // heal on an untouched photo must take the SAVE path (recipe.json +
        // pixels.json), not report "nothing to save" while deleting the very
        // linkage the retouch needs to survive a reopen.
        // A NON-TRIVIAL STRIP is an edit too: clear_develop removes
        // variants.json with everything else, so a neutral canvas sitting in
        // a multi-variant strip must take the save path (noop recipe + strip
        // record) — "clear my edits" must not silently destroy the OTHER
        // cards.
        if self.recipe.is_noop()
            && self.active_variant().is_none_or(|v| v.origin.is_none())
            && self.current_strip_record().is_none()
        {
            // NoWait wrapper: clear_develop locks internally, but with Wait —
            // on this UI thread a develop held by another process must fail
            // into the error arm below, not freeze the window.
            match autoshop::store::with_develop_lock(
                &path,
                autoshop::store::DevelopLockMode::NoWait,
                || autoshop::store::clear_develop(&path),
            ) {
                Ok(o) => {
                    self.edited_badge.clear();
                    // The CANVAS is the baseline, not default(): Reset leaves
                    // the stamped lens profile (and knots) on the canvas, and
                    // a default baseline compared dirty on lens_profile
                    // forever — a permanent ● with quit prompts (R4-8).
                    self.saved_recipe = self.recipe.clone();
                    self.pixels_on_disk = None;
                    self.saved_strip = None;
                    self.nav_stash.remove(&path);
                    self.forget_open_base();
                    match o.marker_warning {
                        // NEVER a plain success: the store copies are gone, but
                        // without the marker a projection copied beside the RAW
                        // out-ranks this clear and restores the edits on reopen.
                        Some(w) => {
                            let t = trf(
                                lang,
                                "saved edits cleared, but the clear could not be marked ({err}) — a sidecar beside the RAW may restore them",
                                &[("err", &w)],
                            );
                            self.status = t.clone();
                            self.toast(ToastKind::Error, t);
                        }
                        None => {
                            self.status = if o.removed {
                                tr(lang, "neutral recipe — saved edits cleared (saved files removed)")
                                    .into()
                            } else {
                                tr(lang, "neutral recipe — nothing to save").into()
                            };
                        }
                    }
                }
                Err(e) => {
                    self.persist_postponed(&e, "could not clear the saved edits: {err}", &[]);
                }
            }
            return;
        }
        // recipe.json is the source of truth for EVERY source type (the badge,
        // batch render and reopening all read it; the XMP alone is lossy — no
        // bitmap masks / recolour gains). The XMP is the RAW-only Lightroom
        // projection on top. The recipe write ALONE decides the saved state:
        // once it lands, reopening restores it regardless of the XMP — so the
        // ● baseline must follow it even when the XMP half fails.
        let raw = autoshop::decode::is_raw(&path);
        // The WRITER's rule (api_xmp, produce_recipe, match, Save-all): a
        // washed pre-era curve must not be re-persisted verbatim. The canvas
        // normally arrives repaired at open, but an open-time INABILITY (a
        // then-locked file) leaves it washed — and Ctrl+S then froze the
        // defect on disk while a single export from the SAME canvas repaired
        // and disclosed. The canvas itself heals here: what is written is
        // what is shown (a generated canvas was refused above, and its curve
        // is empty by invariant).
        // ...and the HISTORY follows it — the Arc-repoint rule (see the
        // preview-resolution repoint below in this file): calibration is not
        // an edit, so every step holding the exact pre-repair pair is
        // re-stamped in place. Without this, commit_if_settled pushed the
        // washed step onto the undo stack and ONE Ctrl+Z after Ctrl+S handed
        // the washed curve back to the canvas invisibly (dirty_vs neutralises
        // both fields, so no ● would have said so) — while disk and every
        // deliverable stayed repaired.
        let before = (self.recipe.version, self.recipe.base_curve.clone());
        let relooked =
            autoshop::pipeline::repair_pre_era_base_curve(&path, &mut self.recipe).is_some();
        if relooked {
            let (after_ver, after_curve) =
                (self.recipe.version, self.recipe.base_curve.clone());
            let restamp = |r: &mut EditRecipe| {
                if r.version == before.0 && r.base_curve == before.1 {
                    r.version = after_ver;
                    r.base_curve = after_curve.clone();
                }
            };
            restamp(&mut self.committed.recipe);
            self.undo_stack.iter_mut().for_each(|s| restamp(&mut s.recipe));
            self.redo_stack.iter_mut().for_each(|s| restamp(&mut s.recipe));
            // The strip entry that IS the canvas follows it, and the preview
            // re-develops under the healed curve.
            if let Some(v) = self.variants.get_mut(self.active) {
                v.recipe = self.recipe.clone();
            }
            // ...and the SIBLINGS follow too (the stash-retry rule): the
            // estimate is already paid, so healing the rest of the strip is
            // a memo hit apiece — a save must not leave the photo split
            // into a bright canvas over washed sibling RECIPES. A healed
            // sibling also drops its CARD: the thumb is a stale render of
            // the washed recipe, and keeping it showed a stops-dark
            // thumbnail beside a bright canvas until the next rebuild.
            let active = self.active;
            for (i, v) in self.variants.iter_mut().enumerate() {
                if i != active
                    && v.kind != VariantKind::Generated
                    && autoshop::pipeline::repair_pre_era_base_curve(&path, &mut v.recipe)
                        .is_some()
                {
                    v.thumb = None;
                }
            }
            self.dirty = true;
        }
        // ONE lock across the whole compound save: recipe.json, the
        // pixels.json link, the strip record and the XMP projection publish
        // as a unit, so another process cannot interleave between the halves.
        // NoWait — a busy develop postpones the save, ● stays lit, the canvas
        // loses nothing, and Ctrl+S retries.
        let locked_save = autoshop::store::with_develop_lock(
            &path,
            autoshop::store::DevelopLockMode::NoWait,
            || -> std::io::Result<()> {
        if self.open_unresolved {
            // The baseline came from an open that could not READ the saved
            // develop — snapshot whatever is on disk before overwriting it
            // (v<N> copy). Refusing on a failed snapshot beats destroying a
            // save nobody ever looked at.
            if let Err(e) = autoshop::store::backup_saved_develop(&path, Some(&self.recipe)) {
                return Err(std::io::Error::other(format!(
                    "the unread develop could not be backed up ({e}) — nothing was overwritten"
                )));
            }
        }
        match autoshop::pipeline::write_recipe(&path, &self.recipe, None) {
            Ok(rp) => {
                self.open_unresolved = false;
                // Pixel identity FIRST — before the badge/baseline/stash are
                // advanced — so a failed pixels.json write leaves the stash
                // protection armed instead of declaring everything saved: an
                // in-place heal/clone/fill bakes pixels into the variant's
                // origin raster, parametric recipe/XMP cannot carry them, and
                // the store records the master's path so reopening restores
                // the retouched canvas. A parametric-only save CLEARS any
                // stale record (a silent clear failure would resurrect an
                // obsolete retouched canvas on the next open — equally loud).
                let mut pixel_note: Option<String> = None;
                let pixels_ok = match self.active_variant().and_then(|v| v.origin.clone()) {
                    Some(origin) => {
                        let generated = self.active_is_generated();
                        match autoshop::store::write_pixel_source(&path, &origin, generated) {
                            Ok(()) => {
                                pixel_note = Some(
                                    tr(
                                        lang,
                                        " · retouched pixels: master linked — reopening restores them (the Lightroom XMP stays parametric-only)",
                                    )
                                    .to_string(),
                                );
                                true
                            }
                            Err(e) => {
                                let t = trf(
                                    lang,
                                    "could not record the retouched master ({err}) — reopening shows the un-retouched source; Export keeps the pixels",
                                    &[("err", &e.to_string())],
                                );
                                self.toast(ToastKind::Error, t.clone());
                                pixel_note = Some(format!(" · {t}"));
                                false
                            }
                        }
                    }
                    None => match autoshop::store::clear_pixel_source(&path) {
                        Ok(()) => true,
                        Err(e) => {
                            let t = trf(
                                lang,
                                "could not clear the recorded retouched master ({err}) — reopening may resurrect it",
                                &[("err", &e.to_string())],
                            );
                            self.toast(ToastKind::Error, t.clone());
                            pixel_note = Some(format!(" · {t}"));
                            false
                        }
                    },
                };
                // The strip record saves/clears WITH the recipe (same pairing
                // as the pixel link): a failed write keeps the mirror — and
                // with it the background-variant unsaved protection — armed.
                let strip_err = self.persist_strip(&path).err();
                if let Some(e) = &strip_err {
                    let t = trf(
                        lang,
                        "could not save the variant strip ({err}) — background variants will not survive a reopen",
                        &[("err", &e.to_string())],
                    );
                    self.toast(ToastKind::Error, t.clone());
                    pixel_note = Some(match pixel_note {
                        Some(n) => format!("{n} · {t}"),
                        None => format!(" · {t}"),
                    });
                }
                self.edited_badge.clear(); // the open photo just gained its badge
                self.saved_recipe = self.recipe.clone();
                if pixels_ok {
                    self.nav_stash.remove(&path);
                    self.pixels_on_disk = self.active_variant().and_then(|v| v.origin.clone());
                }
                let mut s = if raw {
                    match autoshop::pipeline::write_xmp_disclosed(&path, &self.recipe) {
                        Ok((p, merge_note)) => {
                            // A sidecar we could not MERGE was regenerated, and
                            // that drops the user's Lightroom-only properties.
                            // Saying only "saved" is how that loss stayed
                            // invisible until they reopened the catalog.
                            if let Some(m) = &merge_note {
                                self.toast(ToastKind::Error, m.clone());
                            }
                            let base = trf(
                                lang,
                                "XMP + recipe saved → {path}",
                                &[("path", &p.display().to_string())],
                            );
                            match merge_note {
                                Some(m) => format!("{base} — ⚠ {m}"),
                                None => base,
                            }
                        }
                        Err(e) => {
                            let t = trf(
                                lang,
                                "recipe saved — but the Lightroom XMP failed: {err}",
                                &[("err", &e.to_string())],
                            );
                            self.toast(ToastKind::Error, t.clone());
                            t
                        }
                    }
                } else {
                    trf(
                        lang,
                        "recipe saved → {path} (XMP applies to RAW only)",
                        &[("path", &rp.display().to_string())],
                    )
                };
                if let Some(n) = pixel_note {
                    s.push_str(&n);
                }
                if relooked {
                    s.push_str(&format!(
                        " · {}",
                        tr(
                            lang,
                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                        )
                    ));
                }
                self.forget_open_base();
                self.status = s;
            }
            Err(e) => {
                let mut t = trf(lang, "save failed: {err}", &[("err", &e.to_string())]);
                if relooked {
                    // The disclosure travels with the MUTATION, not the
                    // success: the canvas healed above even though the write
                    // failed, and a silently brighter photo under an
                    // unrelated error reads as a glitch.
                    t.push_str(&format!(
                        " · {}",
                        tr(
                            lang,
                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                        )
                    ));
                }
                self.status = t.clone();
                self.toast(ToastKind::Error, t); // a failed save must be seen
            }
        }
                Ok(())
            },
        );
        if let Err(e) = locked_save {
            self.persist_postponed(
                &e,
                "save postponed: this photo is being changed by another Autoshop process ({err}); your canvas remains unsaved — retry",
                &[],
            );
        }
    }

    /// Paste the copied recipe onto every Ctrl+click-selected photo on a worker
    /// thread — Lightroom's "sync settings", without rendering anything: a
    /// recipe.json per photo (central store) plus an XMP sidecar for RAWs.
    /// Geometry (crop/straighten) is stripped unless `paste_geometry` is on,
    /// because composition rarely transfers between frames. Library files are
    /// never touched (write_recipe / write_xmp land in the store, never beside
    /// the photo).
    fn start_paste(&mut self) {
        let Some(src) = self.copied.clone() else { return };
        if self.busy {
            return;
        }
        let targets: Vec<PathBuf> = {
            let mut idx: Vec<usize> = self.multi_sel.iter().copied().collect();
            idx.sort_unstable(); // report in gallery order, not hash order
            idx.into_iter().filter_map(|i| self.gallery.get(i).cloned()).collect()
        };
        if targets.is_empty() {
            return;
        }
        let mut recipe = src;
        if !self.paste_geometry {
            recipe.crop = None;
            recipe.straighten_deg = 0.0;
        }
        // Bitmap masks reference per-photo rasters keyed to the SOURCE stem —
        // pasted onto ANOTHER photo they point at the wrong file (and classic
        // XMP cannot carry them, so recipe.json and .xmp would disagree).
        // Strip them for foreign targets, visibly — but the photo the
        // clipboard CAME FROM keeps its own masks (pasting back onto itself
        // used to silently destroy valid AI selections).
        let recipe_full = recipe.clone();
        let copied_from = self.copied_from.clone();
        let n_bitmap = recipe
            .masks
            .iter()
            .filter(|m| matches!(m.mask, autoshop::recipe::MaskGeometry::Bitmap { .. }))
            .count();
        let has_foreign_target = targets.iter().any(|t| Some(t) != copied_from.as_ref());
        if n_bitmap > 0 {
            recipe
                .masks
                .retain(|m| !matches!(m.mask, autoshop::recipe::MaskGeometry::Bitmap { .. }));
            if has_foreign_target {
                self.toast(
                    ToastKind::Error,
                    trf(
                        self.lang,
                        "{n} bitmap mask(s) not pasted — their rasters belong to the source photo (re-run AI select on each target)",
                        &[("n", &n_bitmap.to_string())],
                    ),
                );
            }
        }
        // If the open photo is one of the targets, take the paste live in the
        // editor too (undo-able through the usual committed-snapshot step).
        // Remember the pair so Msg::Pasted can advance the ● baseline: the
        // worker writes this exact recipe to the open photo's store, so
        // leaving `saved_recipe` behind kept a false "● unsaved" lit for
        // edits that were already on disk.
        self.pasted_open = None;
        if let Some(open) = self.src_path.clone()
            && targets.iter().any(|t| t == &open)
        {
            // Same base_curve rule the worker applies to every target below,
            // mirrored here so `pasted_open` equals the bytes the worker
            // writes and the ● baseline can advance on success.
            let chosen = if Some(&open) == copied_from.as_ref() { &recipe_full } else { &recipe };
            let mut live = paste_recipe_for(&open, chosen);
            // The DISK form keeps its calibration (the worker writes it
            // below), but an active GENERATED canvas must not render it —
            // its pixels already carry the look (the load_version /
            // open-restore strip rule). The ● baseline (pasted_open) is
            // normalized the same way: canvas coordinates.
            if self.active_is_generated() {
                live.base_curve = Vec::new();
                live.lens_profile = Default::default();
                // Anchor follows the strip rule (baked WB → relative model).
                live.as_shot_k = None;
                live.as_shot_tint = None;
            }
            self.recipe = live.clone();
            self.dirty = true;
            // Wholesale recipe replacement: disarm index-carrying tools and
            // refresh derived display, like every other whole-swap path.
            self.resync_recipe_display();
            self.pasted_open = Some((open, live));
        }
        let lang = self.lang; // localise the UI status AND the worker's result strings
        self.busy = true;
        self.status = trf(
            lang,
            "Pasting recipe to {n} photos…",
            &[("n", &targets.len().to_string())],
        );
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<String> {
                    let (mut okn, mut xmpn) = (0usize, 0usize);
                    let mut errs: Vec<String> = Vec::new();
                    // XMP-half failures are partial successes (recipe-write-
                    // decides), but their REASON used to reach stderr only —
                    // the status said "n XMP" and left the user to notice the
                    // shortfall by subtraction.
                    let mut xmp_fails: Vec<String> = Vec::new();
                    for path in &targets {
                        let mut step = || -> anyhow::Result<bool> {
                            // Worker thread ⇒ Wait: queue behind whichever
                            // process holds this target's develop, exactly
                            // like the CLI. ONE lock across gate → recipe →
                            // XMP so the pasted pair publishes as a unit.
                            autoshop::store::with_develop_lock(
                                path,
                                autoshop::store::DevelopLockMode::Wait,
                                || {
                            // Per-target base-look resolution (paste_recipe_for):
                            // paste copies the user edit, never the source
                            // photo's camera calibration over a saved one. The
                            // clipboard's own photo keeps its bitmap masks.
                            let chosen =
                                if Some(path) == copied_from.as_ref() { &recipe_full } else { &recipe };
                            let r = paste_recipe_for(path, chosen);
                            // The programmatic-overwrite gate, like Analyze /
                            // serve / the CLI: one click overwrites MANY saved
                            // develops the user is not even looking at — a
                            // failed snapshot refuses that target.
                            autoshop::store::backup_saved_develop(path, Some(&r))
                                .map_err(|e| anyhow::anyhow!(
                                    "refusing to overwrite the saved develop: backing it up failed ({e})"
                                ))?;
                            autoshop::pipeline::write_recipe(path, &r, None)?;
                            if autoshop::decode::is_raw(path) {
                                // Recipe-write-decides: an XMP failure after
                                // the recipe landed is a partial success —
                                // reopening restores the paste regardless.
                                if let Err(e) = autoshop::pipeline::write_xmp(path, &r) {
                                    eprintln!(
                                        "⚠ {}: recipe pasted, but the Lightroom XMP failed: {e}",
                                        autoshop::pipeline::stem(path)
                                    );
                                    xmp_fails.push(format!(
                                        "{}: {e}",
                                        autoshop::pipeline::stem(path)
                                    ));
                                    return Ok(false);
                                }
                                return Ok(true);
                            }
                            Ok(false)
                                },
                            )
                        };
                        match step() {
                            Ok(wrote_xmp) => {
                                okn += 1;
                                xmpn += usize::from(wrote_xmp);
                            }
                            Err(e) => errs.push(format!("{}: {e}", autoshop::pipeline::stem(path))),
                        }
                    }
                    // Any failure surfaces as an error toast WITH the success count —
                    // a partial failure must never read as a clean success.
                    if errs.is_empty() {
                        let mut s = trf(
                            lang,
                            "Recipe pasted to {ok} photos ({xmp} XMP) → develop store",
                            &[("ok", &okn.to_string()), ("xmp", &xmpn.to_string())],
                        );
                        if !xmp_fails.is_empty() {
                            let d = brief_list(&xmp_fails);
                            s.push_str(&trf(
                                lang,
                                " — ⚠ {n} XMP projection(s) failed (those pastes ARE saved): {detail}",
                                &[("n", &xmp_fails.len().to_string()), ("detail", &d)],
                            ));
                        }
                        Ok(s)
                    } else {
                        anyhow::bail!(
                            "{}",
                            trf(
                                lang,
                                "{ok} succeeded, {fail} failed: {detail}",
                                &[
                                    ("ok", &okn.to_string()),
                                    ("fail", &errs.len().to_string()),
                                    ("detail", &errs.join(" · ")),
                                ],
                            )
                        )
                    }
                })();
                Msg::Pasted(res)
            },
            |e| Msg::Pasted(Err(e)),
        );
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        // UI-thread language for status/toast strings built here. Worker RESULT
        // strings (msg / note / s / label) were already localised inside their
        // spawn closures before the thread started, so they arrive ready to show.
        let lang = self.lang;
        // Drain a bounded batch each frame so a burst of thumbnails doesn't take
        // one-per-frame to land (try_recv borrow is released before we mutate).
        for _ in 0..64 {
            let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) else { break };
            match msg {
                Msg::Opened(boxed) => {
                    // `keep` distinguishes a fresh open from a preview-resolution
                    // re-decode (the px combo): consumed whether the open
                    // succeeds or fails so a failure can't leak it into a later
                    // open.
                    // The KEEP request is honoured only when the recorded
                    // FACT agrees the flight was same-path: a stale request
                    // must never graft the outgoing photo's whole canvas,
                    // strip and history onto an incoming photo.
                    let keep = std::mem::take(&mut self.keep_recipe) && self.open_same_path;
                    let edge_revert = self.edge_before_flight.take();
                    self.open_in_flight = false; // both arms: the transition ended
                    match *boxed {
                    Ok((base, knots, lens, as_shot, baked, src_ident)) => {
                        self.busy = false;
                        if let Some(p) = self.src_path.clone() {
                            self.remember_base(
                                &p,
                                &(
                                    base.clone(),
                                    knots.clone(),
                                    lens.clone(),
                                    as_shot,
                                    baked.clone(),
                                    src_ident.clone(),
                                ),
                            );
                        }
                        // Kept for Reset: "reset" = the fresh-open look, so it
                        // needs this photo's knots even when the restored
                        // recipe (legacy save) deliberately carries none.
                        // Deliberately LAST-wins across preview-edge switches
                        // (Reset means the fresh-open look at the CURRENT
                        // edge) while the repair memo pins its FIRST answer
                        // for persistence determinism — the two agree within
                        // the estimator's documented tolerance.
                        self.photo_knots = knots.clone();
                        self.photo_lens = lens.clone();
                        self.photo_as_shot = as_shot;
                        if keep {
                            // Preview-resolution re-decode: the SOURCE pixels just
                            // changed resolution — keep the whole variant set,
                            // recipe, undo history and view (zoom included: you
                            // switched to 4096px to inspect 1:1, losing the zoom
                            // would defeat the point). Refresh the base a
                            // source-based active variant develops from; hand a
                            // baked-raster variant its own master re-decoded —
                            // and when neither is possible, REFUSE the switch
                            // instead of half-applying it.
                            let (mw, mh) = base.dimensions();
                            // Consume the stash entry the same-path open_path
                            // call just wrote: the KEEP branch preserves the
                            // live canvas in place, so the entry is a stale
                            // duplicate — after an undo-to-clean it would
                            // resurrect the pre-undo edits on the next return.
                            // (Both outcomes below keep the canvas in place, so
                            // this is unconditional.)
                            if let Some(p) = self.src_path.clone() {
                                self.nav_stash.remove(&p);
                            }
                            let active_source =
                                self.active_variant().is_none_or(|v| v.base.is_none());
                            // ONE source of truth for "the worker brought the
                            // master this canvas renders": the branch below
                            // destructures the same `baked`, and a guard that
                            // could drift from it is how a canvas ends up at an
                            // edge nobody recorded.
                            let master_hit = baked.as_ref().is_some_and(|(_, borigin, _)| {
                                self.active_variant().is_some_and(|v| {
                                    v.origin.as_deref().is_some_and(|o| same_master(o, borigin))
                                })
                            });
                            if !active_source && !master_hit && let Some(e) = edge_revert {
                                // The switch CANNOT materialize on THIS canvas:
                                // the active variant renders baked pixels
                                // (heal / clone / fill / reimagine) whose master
                                // the worker had nothing to re-decode from —
                                // `pixels.json` is written by SAVE, so an
                                // unsaved retouch records no master at all.
                                // Adopting the new edge anyway was the busy
                                // refusal's residue through the third door: the
                                // status announced a re-decode the canvas never
                                // took, the value persisted into Prefs, and
                                // every bake site would have installed a
                                // new-edge raster under an old-edge canvas. The
                                // re-decoded source is dropped with it, so
                                // canvas and edge cannot disagree in either
                                // direction (a later switch after saving
                                // re-decodes the master properly).
                                self.preview_edge = e;
                                // The remedy is read off THIS CANVAS, never
                                // off the photo's record: a stale broken
                                // record (an ./out cleanup) plus a NEW unsaved
                                // retouch used to be told its situation was
                                // unfixable and denied the one cure that
                                // works. Three causes, three answers:
                                //  · a GENERATED canvas has no save a user can
                                //    be sent to press: Ctrl+S refuses it
                                //    outright (the Analyze auto-save and
                                //    Save-all DO record generated masters, so
                                //    "cannot be recorded" would be false), and
                                //    the message names the route that always
                                //    works instead;
                                //  · a master no longer ON DISK cannot be
                                //    re-decoded however often it is recorded
                                //    (saving would re-record the same broken
                                //    link and the refusal would repeat);
                                //  · otherwise the master is there and merely
                                //    unrecorded (or superseded since the last
                                //    save), which saving fixes.
                                // Residual, bounded and stderr-visible: a
                                // master that exists but will not DECODE takes
                                // the third arm, and `read_pixel_source` /
                                // the worker already name it on stderr.
                                let canvas_master =
                                    self.active_variant().and_then(|v| v.origin.clone());
                                let t = if self.active_is_generated() {
                                    tr(
                                        lang,
                                        "preview resolution kept — a generated variant's pixels come from its own render; switch to a source-based variant to work at another resolution",
                                    )
                                } else if !canvas_master.as_deref().is_some_and(|o| o.exists()) {
                                    tr(
                                        lang,
                                        "preview resolution kept — this canvas's retouch master is no longer on disk, so it cannot be re-decoded at the new size",
                                    )
                                } else {
                                    tr(
                                        lang,
                                        "preview resolution kept — this retouched canvas has no saved master to re-decode at the new size; save the photo, then switch",
                                    )
                                };
                                // The bar still reads "decoding … " (open_path
                                // wrote it and `busy` is already false): every
                                // other outcome of this landing replaces it, so
                                // the refusal must too — a finished decode
                                // announcing itself forever is the same lie one
                                // door along, on the surface that does not
                                // expire.
                                self.status = t.to_string();
                                self.toast(ToastKind::Error, t);
                            } else {
                                self.source_preview = Some(base.clone());
                                if active_source {
                                    let curve = self.recipe.base_curve.clone();
                                    self.set_before(ctx, &base, &curve);
                                    self.mask_paint = Some(image::RgbaImage::new(mw, mh));
                                    self.mask_tex = None;
                                    self.mask_dirty = false;
                                    self.paint_last = None;
                                    // The range-mask reference develop was computed
                                    // from the OLD base — recipe-only keying would
                                    // serve it stale against these new pixels.
                                    self.overlay_ref = None;
                                    self.overlay_stale = true;
                                    self.base_preview = Some(base);
                                } else if master_hit
                                    && let Some((bimg, _, _)) = &baked
                                {
                                    // The active variant renders a RECORDED baked
                                    // master: hand it the master re-decoded at the
                                    // new preview edge too, or 1:1 inspection stays
                                    // at the old resolution. History steps holding
                                    // the old Arc are repointed — same master, new
                                    // resolution — so Ctrl+Z can't flip the canvas
                                    // back to the stale-res pixels.
                                    let old = self
                                        .variants
                                        .get(self.active)
                                        .and_then(|v| v.base.clone());
                                    if let Some(v) = self.variants.get_mut(self.active) {
                                        v.base = Some(bimg.clone());
                                    }
                                    // Bound, stated instead of implied: only
                                    // the Arc the canvas holds NOW is
                                    // re-pointed. A step on a SUPERSEDED master
                                    // (an earlier heal, replaced by this one)
                                    // has no re-decoded twin — the worker
                                    // resolves exactly one master — so undoing
                                    // to it restores that raster at ITS own
                                    // edge. That is why a bake follows the
                                    // canvas (`canvas_edge`), not the
                                    // preference.
                                    if let Some(old) = old {
                                        let repoint = |s: &mut UndoStep| {
                                            let hit = s
                                                .base
                                                .as_ref()
                                                .is_some_and(|b| Arc::ptr_eq(b, &old));
                                            if hit {
                                                s.base = Some(bimg.clone());
                                            }
                                        };
                                        repoint(&mut self.committed);
                                        self.undo_stack.iter_mut().for_each(repoint);
                                        self.redo_stack.iter_mut().for_each(repoint);
                                    }
                                    let (bw, bh) = bimg.dimensions();
                                    // Canvas-curve Before, same rule as every
                                    // restore path: an InPlace master is NEUTRAL
                                    // (needs the camera curve); a Generated
                                    // canvas recipe carries an empty curve.
                                    let curve = self.recipe.base_curve.clone();
                                    self.set_before(ctx, bimg, &curve);
                                    self.mask_paint = Some(image::RgbaImage::new(bw, bh));
                                    self.mask_tex = None;
                                    self.mask_dirty = false;
                                    self.paint_last = None;
                                    self.overlay_ref = None;
                                    self.overlay_stale = true;
                                    self.base_preview = Some(bimg.clone());
                                }
                                self.last_rgb = None; // resolution changed under the frame
                                self.dirty = true;
                                self.status = trf(
                                    lang,
                                    "Preview resolution {px}px — re-decoded",
                                    &[("px", &self.preview_edge.to_string())],
                                );
                            }
                        } else {
                            // Fresh open: a single Original variant, all
                            // per-photo state reset. The recipe starts from the
                            // photo's SAVED develop when a ./out sidecar exists
                            // — the gallery badge already promises "● edited",
                            // so opening must honour it — neutral otherwise.
                            let (mw, mh) = base.dimensions();
                            // before_tex is built AFTER the recipe is settled
                            // below — the Before pane carries the canvas
                            // recipe's own base_curve.
                            self.source_preview = Some(base.clone());
                            self.base_preview = Some(base);
                            let (saved, xmp_bad, dropped_masks, clamp_dropped) = self
                                .src_path
                                .as_deref()
                                .map(read_saved_develop)
                                .unwrap_or((SavedDevelop::Nothing, Vec::new(), 0, Default::default()));
                            // W20: restore-time sanitisation is not allowed to
                            // be silent loss — a stored recipe past the caps
                            // opens minus edits, and the user must hear it.
                            if !clamp_dropped.is_empty() {
                                // All four loss kinds — a curve-only or
                                // string-only truncation used to toast
                                // "0 mask(s) and 0 component(s)" (16-lane
                                // scan L14/L16).
                                let t = trf(
                                    lang,
                                    "recipe limits discarded {n} mask(s), {m} component(s), {c} curve point(s) and {s} string byte(s) on restore — the saved file exceeds the app's caps",
                                    &[
                                        ("n", &clamp_dropped.dropped_masks.to_string()),
                                        ("m", &clamp_dropped.dropped_components.to_string()),
                                        ("c", &clamp_dropped.truncated_curve_points.to_string()),
                                        ("s", &clamp_dropped.truncated_string_bytes.to_string()),
                                    ],
                                );
                                self.toast(ToastKind::Error, t);
                            }
                            // LR brush / AI / depth masks have no engine
                            // equivalent — the import skips them BY DESIGN
                            // (the writer skips symmetrically); what was
                            // missing is telling the user their Lightroom
                            // work arrived incomplete. The sidecar keeps
                            // them; only the in-app render lacks them.
                            if dropped_masks > 0 {
                                let t = trf(
                                    lang,
                                    "{n} Lightroom mask(s) (brush/AI/depth) have no engine equivalent and were not imported — they stay in the sidecar untouched",
                                    &[("n", &dropped_masks.to_string())],
                                );
                                self.toast(ToastKind::Error, t);
                            }
                            let mut restored: Option<&'static str> = None;
                            let mut open_note: Option<String> = None;
                            let mut recipe = EditRecipe::default();
                            // Whether to stamp this photo's camera-matched base
                            // look onto the recipe: yes for fresh opens and
                            // XMP-only restores (Lightroom tuned those against
                            // its own camera-profile base — the bright base is
                            // the closer rendering); NO for recipe.json, which
                            // keeps its saved curve verbatim so legacy develops
                            // render exactly as they were tuned.
                            let mut stamp = true;
                            match saved {
                                SavedDevelop::Restored(r, kind) => {
                                    recipe = r;
                                    restored = Some(kind);
                                    stamp = kind.starts_with("XMP");
                                    // Only the recipe.json path can carry a
                                    // washed-frame base curve to the canvas: an
                                    // XMP restore re-stamps the curve anyway
                                    // (stamp == true). Its own sentence, in the
                                    // open-note channel — appended to `xmp_bad`
                                    // it was rendered as "unreadable XMP
                                    // numeric setting(s) … restored as
                                    // neutral", a warning about something else
                                    // entirely.
                                    if !stamp
                                        // The fourth ordering site: a
                                        // GENERATED canvas strips its
                                        // calibration below, so repairing
                                        // first paid a synchronous RAW
                                        // decode + develop ON THE UI THREAD
                                        // for a curve the strip then
                                        // deleted — and the note disclosed
                                        // a re-estimate that never reached
                                        // a pixel (load_version, the render
                                        // funnel and the batch export were
                                        // the other three).
                                        && !baked.as_ref().is_some_and(|(_, _, g)| *g)
                                        // ...nor when THIS session's nav
                                        // stash is about to override the
                                        // canvas wholesale (below): the
                                        // toast would disclose a repair the
                                        // stash discards, and the decode
                                        // would be paid for nothing. (● is
                                        // safe in BOTH directions now —
                                        // dirty_vs neutralises the era
                                        // stamp with the curve.)
                                        // Deliverables repair for
                                        // themselves.
                                        && !self
                                            .src_path
                                            .as_ref()
                                            .is_some_and(|p| self.nav_stash.contains_key(p))
                                        && self.src_path.clone().is_some_and(|p| {
                                            autoshop::pipeline::repair_pre_era_base_curve(
                                                &p, &mut recipe,
                                            )
                                            .is_some()
                                        })
                                    {
                                        // The GUI's OWN sentence, localized —
                                        // every other open-note here goes
                                        // through tr/trf; the engine note is
                                        // for the CLI/HTTP surfaces.
                                        open_note = Some(
                                            tr(
                                                lang,
                                                "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                                            )
                                            .into(),
                                        );
                                    }
                                }
                                SavedDevelop::Unreadable { err, fallback } => {
                                    // A damaged / newer-build recipe.json must
                                    // never degrade to the lossy XMP silently:
                                    // bitmap masks and recolour gains would be
                                    // gone and the next Ctrl+S would overwrite
                                    // the still-good file.
                                    if let Some((r, kind)) = fallback {
                                        recipe = r;
                                        restored = Some(kind);
                                        stamp = kind.starts_with("XMP");
                                    }
                                    // An explicit save over a develop we never
                                    // successfully READ (damaged file, or busy
                                    // in another process at open) must version
                                    // what it overwrites — save_xmp runs the
                                    // backup gate while this flag stands, so
                                    // the unread save survives as v<N> instead
                                    // of being destroyed by a blank baseline.
                                    self.open_unresolved = true;
                                    open_note = Some(trf(
                                        lang,
                                        "recipe.json is unreadable ({err}) — edits NOT fully restored; Ctrl+S would overwrite it (the unread save is backed up as a version first)",
                                        &[("err", &err)],
                                    ));
                                }
                                SavedDevelop::NoopOnly => {
                                    open_note = Some(
                                        tr(lang, "a saved develop exists but holds no effective edits")
                                            .into(),
                                    );
                                }
                                SavedDevelop::Nothing => {}
                            }
                            // A6 disclosure: numeric settings in the restored
                            // XMP that do not parse imported as SILENT
                            // neutrals — and the next save would write those
                            // neutrals back over the sidecar. Say so on open.
                            if !xmp_bad.is_empty() {
                                let w = trf(
                                    lang,
                                    "{n} XMP numeric setting(s) unreadable ({list}) — restored as neutral; saving would overwrite the sidecar with those neutrals",
                                    &[
                                        ("n", &xmp_bad.len().to_string()),
                                        ("list", &xmp_bad.join(", ")),
                                    ],
                                );
                                open_note = Some(match open_note {
                                    Some(o) => format!("{o} · {w}"),
                                    None => w,
                                });
                            }
                            if stamp {
                                if !knots.is_empty() {
                                    recipe.base_curve = knots.clone();
                                }
                                // Same rule as the curve: fresh opens and
                                // XMP-only restores get the photo's in-camera
                                // lens profile (all-available-on); a saved
                                // recipe.json keeps its own verbatim.
                                recipe.lens_profile = lens.clone();
                                // Third calibration half, same rule — plus
                                // stamp-if-None: an old-era Autoshop XMP
                                // restore arrives with the 5500 anchor PINNED
                                // by xmp_to_recipe (its Kelvin was tuned
                                // relative); overwriting the pin would
                                // reinterpret the saved look.
                                if recipe.as_shot_k.is_none()
                                    && let Some((k, t)) = as_shot
                                {
                                    recipe.as_shot_k = Some(k);
                                    recipe.as_shot_tint = Some(t);
                                }
                            } else if !stamp
                                && recipe.base_curve.is_empty()
                                && !knots.is_empty()
                                && open_note.is_none()
                            {
                                // A legacy save renders exactly as it was
                                // tuned — on the old dark base. Say so, and
                                // point at the way out, or the photo just
                                // looks broken next to fresh opens.
                                open_note = Some(
                                    tr(
                                        lang,
                                        "saved before the camera base look — renders as originally tuned; Reset switches to the camera-matched base",
                                    )
                                    .into(),
                                );
                            }
                            // The saved develop is the ● baseline; a canvas
                            // stashed when the user navigated away THIS session
                            // outranks it — that stash is their newest work.
                            self.saved_recipe = recipe.clone();
                            let mut from_stash = false;
                            // The stash (this session's newest work) outranks
                            // disk WHOLESALE — recipe and pixel identity both:
                            // a stashed source-based canvas (e.g. the retouch
                            // was undone) must not resurrect disk pixels.
                            // Disk truth for the per-frame ● comparison —
                            // captured BEFORE the stash override below.
                            self.pixels_on_disk = baked.as_ref().map(|(_, o, _)| o.clone());
                            // A RECORDED master that arrived as None failed to
                            // decode/resolve in the worker (it only eprintln'd
                            // there) — a desktop-launched GUI would otherwise
                            // show an apparently normal un-retouched open with
                            // no in-app trace.
                            // has_pixel_source, NOT read_pixel_source: the
                            // latter answers None for "nothing recorded" AND
                            // for "recorded but broken", so asking it again
                            // here made the toast unreachable for the two
                            // commonest causes — a deleted/moved master and a
                            // corrupt pixels.json. Those are precisely the
                            // cases the user must be told about.
                            if baked.is_none()
                                && let Some(p) = self.src_path.as_deref()
                                && autoshop::store::has_pixel_source(p)
                            {
                                let t = tr(
                                    lang,
                                    "the saved retouch master could not be loaded — opened the un-retouched source (Ctrl+S would overwrite the master link)",
                                );
                                self.toast(ToastKind::Error, t);
                            }
                            let mut pixels: Option<BakedBase> = baked;
                            // The stashed active card's master when its decode
                            // was still IN FLIGHT at navigate-away: origin
                            // known, pixels not yet — the identity must
                            // survive the reopen even though the image is not
                            // here yet.
                            let mut pending_master: Option<PathBuf> = None;
                            let mut stash_others: Vec<StashedVariant> = Vec::new();
                            let mut stash_active_pos = 0usize;
                            // The ACTIVE card's three-valued kind, whichever
                            // authority supplies it: the session stash below,
                            // or the persisted strip record read further down
                            // (`Fitted` is source-based, so it has no pixels
                            // arm to ride back on — without this the card
                            // reopened renamed 「▣ 原片」).
                            let mut active_kind = VariantKind::Original;
                            if let Some(st) =
                                self.src_path.as_ref().and_then(|p| self.nav_stash.remove(p))
                            {
                                recipe = st.recipe;
                                active_kind = st.kind;
                                pixels = match (st.base, st.origin) {
                                    (Some(b), Some(o)) => {
                                        Some((b, o, st.kind == VariantKind::Generated))
                                    }
                                    // Navigated away while the cold master was
                                    // still decoding: the ORIGIN is the pixel
                                    // link's identity — dropping it here let
                                    // the next Ctrl+S clear pixels.json and
                                    // persist a generated recipe as a source-
                                    // based develop. The base re-decodes
                                    // below; the link and the kind survive.
                                    (None, Some(o)) => {
                                        pending_master = Some(o);
                                        None
                                    }
                                    _ => None,
                                };
                                stash_others = st.others;
                                stash_active_pos = st.active_pos;
                                from_stash = true;
                                // The stash carries the strip VERBATIM — and
                                // when the ORIGINAL open's repair was an
                                // inability, those recipes still hold the
                                // washed era-1 curve. Inabilities exist to
                                // be retried by the next reader, and the
                                // gate above deliberately skipped the
                                // disk-recipe repair in deference to this
                                // override — so the retry happens HERE, on
                                // every recipe that can land on the canvas:
                                // the active one AND each sibling (a strip
                                // entry is one click from BEING the canvas).
                                // The washed-sibling population is ORDINARY,
                                // not residual: push_variant/switch_variant
                                // sync the OUTGOING canvas into the strip,
                                // so a washed Original displaced by an AI
                                // push is a washed sibling (the freshly
                                // PUSHED variants themselves are era-2 and
                                // empty by construction). load_active
                                // repairs on switch as the last line of
                                // defence; this eager pass heals the whole
                                // strip the moment it is restored. ONE
                                // estimate total — the memo keys on the
                                // photo — and already-repaired recipes
                                // short-circuit on their era stamp.
                                // A failed estimate retries on the next
                                // return; the deterministic-inability
                                // population is empty for estimator-written
                                // curves (the too-few-pixels guard is
                                // coeval with base_curve itself). Generated
                                // entries are skipped like the other four
                                // ordering sites — their curves are empty
                                // by invariant, and the explicit guard
                                // beats leaning on six unrelated call
                                // sites.
                                if let Some(p) = self.src_path.clone() {
                                    let mut relooked = active_kind != VariantKind::Generated
                                        && autoshop::pipeline::repair_pre_era_base_curve(
                                            &p, &mut recipe,
                                        )
                                        .is_some();
                                    for sv in &mut stash_others {
                                        if sv.kind != VariantKind::Generated {
                                            relooked |=
                                                autoshop::pipeline::repair_pre_era_base_curve(
                                                    &p,
                                                    &mut sv.recipe,
                                                )
                                                .is_some();
                                        }
                                    }
                                    if relooked {
                                        let w = tr(
                                            lang,
                                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                                        )
                                        .to_string();
                                        open_note = Some(match open_note {
                                            Some(o) => format!("{o} · {w}"),
                                            None => w,
                                        });
                                    }
                                }
                            }
                            // The persisted strip record — the disk half of
                            // the session stash. Read even when the stash
                            // outranks it wholesale: it is also the mirror
                            // the background-variant dirty test compares
                            // against (see `open_dirty_variants`).
                            let disk_strip = match self
                                .src_path
                                .as_deref()
                                .map(autoshop::store::read_variants_checked)
                            {
                                Some(autoshop::store::VariantsRead::Strip(rec)) => Some(rec),
                                Some(autoshop::store::VariantsRead::Unresolved) => {
                                    // The store's save floor refuses to touch
                                    // an unresolved strip — say so at OPEN,
                                    // not first at the failing save.
                                    let t = tr(
                                        lang,
                                        "this photo's variant strip (variants.json) cannot be read — background variants stay hidden and saving refuses until the file is fixed or deleted",
                                    )
                                    .to_string();
                                    self.toast(ToastKind::Error, t);
                                    None
                                }
                                _ => None,
                            };
                            if !from_stash
                                && let Some(rec) = &disk_strip
                                && rec.active_kind == "fitted"
                            {
                                // Fitted is source-based — it has no pixels
                                // arm to ride back on; without this the card
                                // cold-reopened renamed 「▣ 原片」. A recorded
                                // "generated" needs no hand here: the baked
                                // pixels arm below upgrades the card exactly
                                // when the master really decoded.
                                active_kind = VariantKind::Fitted;
                            }
                            self.recipe = recipe.clone();
                            self.rationale = recipe.rationale.clone();
                            self.variants = vec![Variant {
                                // A PENDING master keeps its stashed kind: the
                                // Fitted-or-Original collapse renamed a still-
                                // decoding Generated card to 「▣ 原片」, and a
                                // save then persisted that lie.
                                kind: if pending_master.is_some() {
                                    active_kind
                                } else if active_kind == VariantKind::Fitted {
                                    VariantKind::Fitted
                                } else {
                                    VariantKind::Original
                                },
                                recipe,
                                base: None,
                                origin: None,
                                thumb: None,
                            }];
                            self.active = 0;
                            // Before = this photo's starting point under the
                            // canvas recipe's own base calibration: stamped
                            // fresh opens compare bright, legacy saves compare
                            // as originally tuned.
                            if let Some(b) = self.base_preview.clone() {
                                let curve = self.recipe.base_curve.clone();
                                self.set_before(ctx, &b, &curve);
                            }
                            // A fresh, fully-transparent paint mask sized to the preview.
                            self.mask_paint = Some(image::RgbaImage::new(mw, mh));
                            self.mask_tex = None;
                            self.mask_dirty = false;
                            self.paint_last = None;
                            self.paint_mode = false;
                            if let Some((bimg, borigin, generated)) = pixels {
                                // Restore the baked pixel master (persisted
                                // pixels.json, or this session's stash): the
                                // canvas sits on the retouched pixels again
                                // with the recipe rendering on top. An
                                // in-place master is a NEUTRAL develop — the
                                // recipe's base_curve legitimately renders the
                                // camera look on top of it; Before compares
                                // curve-less, exactly like the InPlace flow
                                // that made it. A GENERATED master's look
                                // already lives in its pixels, so calibration
                                // must be STRIPPED from the canvas recipe
                                // (same rule as analyzing a baked variant) or
                                // curve + lens geometry would cook it twice.
                                let (bw, bh) = bimg.dimensions();
                                if let Some(v) = self.variants.get_mut(0) {
                                    v.base = Some(bimg.clone());
                                    v.origin = Some(borigin);
                                    if generated {
                                        v.kind = VariantKind::Generated;
                                    }
                                }
                                if generated {
                                    self.recipe.base_curve = Vec::new();
                                    self.recipe.lens_profile = Default::default();
                                    self.recipe.as_shot_k = None;
                                    self.recipe.as_shot_tint = None;
                                    if let Some(v) = self.variants.get_mut(0) {
                                        v.recipe = self.recipe.clone();
                                    }
                                    // The ● baseline lives in CANVAS
                                    // coordinates (same rule as the Analyze
                                    // saver): the disk recipe keeps its
                                    // calibration, but comparing the stripped
                                    // canvas against it lit ● on lens_profile
                                    // the instant a clean generated save
                                    // opened.
                                    self.saved_recipe.base_curve = Vec::new();
                                    self.saved_recipe.lens_profile = Default::default();
                                    self.saved_recipe.as_shot_k = None;
                                    self.saved_recipe.as_shot_tint = None;
                                }
                                // Before under the canvas recipe's own curve:
                                // empty for a Generated master (stripped just
                                // above — its pixels carry the look); the
                                // camera curve for an InPlace master, whose
                                // pixels are a NEUTRAL develop (an empty curve
                                // there compared 0.6–1.4 EV dark).
                                let curve = self.recipe.base_curve.clone();
                                self.set_before(ctx, &bimg, &curve);
                                self.base_preview = Some(bimg);
                                self.mask_paint = Some(image::RgbaImage::new(bw, bh));
                            }
                            if let Some(o) = pending_master.clone() {
                                // Re-attach the surviving identity and re-arm
                                // the worker decode (coalesced if the earlier
                                // one is somehow still in flight). Until it
                                // lands the canvas shows the source develop
                                // under spawn_master_load's disclosure.
                                if let Some(v) = self.variants.get_mut(0) {
                                    v.origin = Some(o.clone());
                                }
                                if let Some(photo) = self.src_path.clone() {
                                    self.spawn_master_load(photo, o);
                                }
                            }
                            // Restore the BACKGROUND variants the stash
                            // carried (H4): the strip used to collapse to
                            // the active canvas alone, silently dropping
                            // every other variant's unsaved work.
                            if from_stash && !stash_others.is_empty() {
                                let active_v = self.variants.remove(0);
                                let mut strip: Vec<Variant> = stash_others
                                    .drain(..)
                                    .map(|sv| Variant {
                                        kind: sv.kind,
                                        recipe: sv.recipe,
                                        base: sv.base,
                                        origin: sv.origin,
                                        thumb: None,
                                    })
                                    .collect();
                                let pos = stash_active_pos.min(strip.len());
                                strip.insert(pos, active_v);
                                self.variants = strip;
                                self.active = pos;
                            } else if !from_stash
                                && let Some(rec) = &disk_strip
                                && !rec.others.is_empty()
                            {
                                // Cold restore of the persisted strip: the
                                // background variants come back with their
                                // recipes and raster origins; base pixels
                                // re-decode lazily on first switch
                                // (`load_active`), the same deal a stash
                                // restore gets from its held Arcs. Pre-era
                                // curves heal exactly like the stash arm
                                // above — a strip entry is one click from
                                // BEING the canvas. (dirty_vs neutralises
                                // version/base_curve, so healing the live
                                // strip never lights ● against the mirror.)
                                let active_v = self.variants.remove(0);
                                let mut strip: Vec<Variant> = rec
                                    .others
                                    .iter()
                                    .filter_map(|e| {
                                        let Some(kind) =
                                            VariantKind::from_store_str(&e.kind)
                                        else {
                                            eprintln!(
                                                "⚠ variants.json entry with unknown kind {:?} — that variant is not restored",
                                                e.kind
                                            );
                                            return None;
                                        };
                                        Some(Variant {
                                            kind,
                                            recipe: e.recipe.clone(),
                                            base: None,
                                            origin: e.origin.clone(),
                                            thumb: None,
                                        })
                                    })
                                    .collect();
                                if let Some(p) = self.src_path.clone() {
                                    for v in strip.iter_mut() {
                                        if v.kind != VariantKind::Generated {
                                            let _ = autoshop::pipeline::repair_pre_era_base_curve(
                                                &p,
                                                &mut v.recipe,
                                            );
                                        }
                                    }
                                }
                                let pos = rec.active_pos.min(strip.len());
                                strip.insert(pos, active_v);
                                self.variants = strip;
                                self.active = pos;
                            }
                            self.saved_strip = disk_strip;
                            self.last_rgb = None; // retained frame was the old photo's
                            self.reset_history(); // a new photo starts a fresh undo history
                            self.region = None; // and a fresh local-edit selection
                            self.region_drag = None;
                            // View + tool state is per-photo.
                            self.zoom = 1.0;
                            self.pan = egui::vec2(0.5, 0.5);
                            self.disarm_tools();
                            // The \-latch is per-photo transient state:
                            // carried across an open, the new photo arrived
                            // with every tool locked out (M21).
                            self.before_latch = false;
                            self.sel_mask = None;
                            self.sel_component = None;
                            self.overlay_ref = None; // the reference develop belongs to ONE base
                            self.overlay_stale = true;
                            self.curve_drag = None; // curve_channel is a UI pref, keep it
                            self.wb_picking = false;
                            self.range_picking = None;
                            self.clone_mode = false;
                            self.clone_src = None;
                            self.verdict = None;
                            // (rationale already restored alongside the recipe)
                            self.refresh_versions(); // version snapshots are per-photo
                            self.dirty = true; // render the (restored or neutral) after
                            self.status = if from_stash {
                                tr(
                                    lang,
                                    "ready — restored this session's unsaved edits (● not saved yet; Ctrl+S)",
                                )
                                .into()
                            } else {
                                match restored {
                                    Some(kind) => trf(
                                        lang,
                                        "ready — restored saved edits ({kind}); Reset returns to neutral",
                                        &[("kind", kind)],
                                    ),
                                    None => {
                                        tr(lang, "ready — adjust sliders or run AI Analyze").into()
                                    }
                                }
                            };
                            // The third door: a stash restore reinstalls the
                            // raster this photo was LEFT with, at its own
                            // edge, while this open decoded the source at the
                            // current preference — which may have moved while
                            // the user was on another photo.
                            if let Some(px) = self.baked_canvas_edge() {
                                let extra = trf(
                                    lang,
                                    " · restored pixels stay at {px}px (their own bake)",
                                    &[("px", &px.to_string())],
                                );
                                self.status.push_str(&extra);
                            }
                            if let Some(note) = open_note {
                                // Sidecar anomalies must be seen, not scrolled
                                // away in the status line.
                                self.toast(ToastKind::Error, note);
                            }
                        }
                    }
                    Err(e) => {
                        self.fail(tr(lang, "could not open"), e);
                        if !keep {
                            // A FRESH open failed: open_path already re-pointed
                            // src_path to the file that wouldn't decode, but the
                            // previous photo's variants / pixels are still live —
                            // a mismatch that would misdirect a later fit / heal /
                            // XMP (they key off src_path). Drop to a clean
                            // no-photo state so src_path and the variants can never
                            // disagree. (A preview-res re-decode failure — keep —
                            // leaves the still-open photo untouched.)
                            self.src_path = None;
                            self.variants.clear();
                            self.active = 0;
                            self.base_preview = None;
                            self.source_preview = None;
                            self.before_tex = None;
                            self.after_tex = None;
                            self.selected = None;
                            // …and its HISTORY, which this arm used to leave
                            // standing: the steps hold the dead photo's
                            // rasters and its recipe, so an undo here restored
                            // a canvas that no longer exists (the keyboard
                            // path reached it because it checked only `busy`).
                            // Gating the key is the guard; clearing the stack
                            // is the reason there is nothing to guard. The
                            // photo's unsaved work is NOT lost — open_path
                            // stashed it before the flight.
                            self.reset_history();
                        } else if let Some(p) = self.src_path.clone() {
                            // A preview-res re-decode failed: the photo (and
                            // its live canvas) is untouched — consume the
                            // stash entry the same-path open_path call just
                            // wrote, exactly like the successful keep branch.
                            // Left behind, an undo-to-clean before the next
                            // navigation resurrected these pre-undo edits on
                            // the way back (CX4-4).
                            self.nav_stash.remove(&p);
                            // ...and the switch never materialized: the
                            // canvas kept the OLD edge, so preview_edge
                            // reverts with it (the busy-refusal rule) —
                            // leaving the new value displayed an unreachable
                            // resolution and fed it to every bake site under
                            // an old-edge canvas.
                            if let Some(e) = edge_revert {
                                self.preview_edge = e;
                            }
                        }
                    }
                }},
                Msg::Developed(boxed) => self.finish_redevelop(ctx, *boxed),
                Msg::Analyzed(epoch, boxed) => {
                    // Cleared whatever the epoch: the wire is free either way,
                    // and this is what re-arms the Analyze button.
                    self.analyze_inflight = false;
                    if epoch != self.gen_epoch {
                        // A cancelled Analyze's late result: the user already
                        // moved on. The Ok arm below also PERSISTS (backup +
                        // recipe.json + XMP) — a stale install would not just
                        // repaint the canvas, it would save over the photo's
                        // develop. Discard entirely (Err is just as silent).
                        continue;
                    }
                    self.gen_cancel = None;
                    match *boxed {
                    Ok((recipe, verdict)) => {
                        // Sliders stay live while Analyze runs (10-30 s):
                        // flush a rename typed during the wait (the resync
                        // below clears the buffer — unflushed, the rename
                        // died with it, M16), then commit any mid-flight
                        // edit as its own undo step NOW, or the wholesale
                        // install below folds it into the pre-analyze step
                        // and Ctrl+Z skips it.
                        self.commit_mask_name_buf();
                        self.commit_now();
                        let accepted =
                            verdict.decision == autoshop::advisor::Decision::Accept;
                        // The recipe arrives with its base_curve already
                        // stamped by produce_recipe (saved-curve-first, else a
                        // fresh estimate) — the single authority all three
                        // surfaces share; the canvas must not override it.
                        // One exception, canvas-side only: a GENERATED variant
                        // already carries the camera look AND the lens
                        // corrections in its pixels, so developing it through
                        // the RAW's curve would cook both twice — the canvas
                        // copy drops them; the persist below still writes the
                        // full stamped recipe (it is the RAW's saved develop,
                        // and reopening lands on the source). An InPlace
                        // retouch master is a NEUTRAL develop (see the
                        // RetouchKind::InPlace arm): its recipe legitimately
                        // keeps the calibration — stripping there rendered the
                        // canvas neutral-dark while the disk recipe kept the
                        // look.
                        let stamped = recipe;
                        let mut canvas = stamped.clone();
                        if self.active_is_generated() {
                            canvas.base_curve = Vec::new();
                            canvas.lens_profile = Default::default();
                            // Anchor follows the strip rule (baked WB).
                            canvas.as_shot_k = None;
                            canvas.as_shot_tint = None;
                        }
                        self.recipe = canvas;
                        // Wholesale replacement: disarm index-carrying tools +
                        // refresh rationale, THEN install the fresh verdict.
                        self.resync_recipe_display();
                        self.verdict = Some(format!("{:?} — {}", verdict.decision, verdict.reasons.join("; ")));
                        self.dirty = true;
                        self.busy = false;
                        // Persist like the CLI and the web do — the badge,
                        // batch render and reopening all read recipe.json, so
                        // an unpersisted analyze silently diverges from every
                        // one of them. An existing explicit save is snapshotted
                        // to v<N> first, never destroyed.
                        self.status = if !accepted {
                            // The verifier itself judged this result not
                            // ready, and a non-Accept verdict may not
                            // auto-save (user decision). The result stays on
                            // the canvas as an UNSAVED edit — ● lit, the nav
                            // stash and the close guard protect it — and the
                            // user decides: Ctrl+S keeps it, Ctrl+Z steps
                            // back to the pre-analyze edit.
                            let t = trf(
                                lang,
                                "AI develop applied — verdict {v}: NOT saved (Ctrl+S keeps it, Ctrl+Z steps back)",
                                &[("v", &format!("{:?}", verdict.decision))],
                            );
                            self.toast(ToastKind::Success, t.clone());
                            t
                        } else { match self.src_path.clone() {
                            // The backup gate comes FIRST: if the existing save
                            // cannot be snapshotted, it is not overwritten —
                            // the analyze result stays on the canvas only.
                            // Persist `stamped`, not the canvas copy: the
                            // sidecar is the RAW's develop and keeps its curve.
                            // ONE NoWait lock across gate → recipe → pixel
                            // link → XMP (the Ctrl+S pairing): busy must
                            // degrade to "applied but NOT saved", never hang
                            // the UI thread behind another process.
                            Some(p) => match autoshop::store::with_develop_lock(
                                &p,
                                autoshop::store::DevelopLockMode::NoWait,
                                || -> std::io::Result<String> {
                                    Ok(match autoshop::store::backup_saved_develop(&p, Some(&stamped)) {
                                Err(e) => {
                                    let t = trf(
                                        lang,
                                        "AI develop applied — but NOT saved: backing up your existing save failed ({err}); Ctrl+S overwrites explicitly",
                                        &[("err", &e.to_string())],
                                    );
                                    self.toast(ToastKind::Error, t.clone());
                                    t
                                }
                                Ok(backed) => {
                                    // The recipe write ALONE decides the saved
                                    // state (same rule as save_xmp): once it
                                    // lands, reopening restores it regardless
                                    // of the XMP — so the ● baseline follows
                                    // it even when the XMP half fails.
                                    match autoshop::pipeline::write_recipe(&p, &stamped, None) {
                                        Ok(_) => {
                                            // Analyze is a SAVER (same rule as
                                            // Ctrl+S) — and the same ORDER:
                                            // pixel identity FIRST, then the
                                            // badge/baseline, stash removal
                                            // GATED on it. A failed
                                            // pixels.json write must leave
                                            // the stash protection armed, not
                                            // declare everything saved.
                                            let sync = match self
                                                .active_variant()
                                                .and_then(|v| v.origin.clone())
                                            {
                                                Some(o) => autoshop::store::write_pixel_source(
                                                    &p,
                                                    &o,
                                                    self.active_is_generated(),
                                                ),
                                                None => autoshop::store::clear_pixel_source(&p),
                                            };
                                            let pixels_ok = match sync {
                                                Ok(()) => true,
                                                Err(e) => {
                                                    let t = trf(
                                                        lang,
                                                        "could not record the retouched master ({err}) — reopening shows the un-retouched source; Export keeps the pixels",
                                                        &[("err", &e.to_string())],
                                                    );
                                                    self.toast(ToastKind::Error, t);
                                                    false
                                                }
                                            };
                                            self.edited_badge.clear();
                                            // The ● baseline lives in CANVAS
                                            // coordinates: on a baked variant
                                            // the canvas copy dropped the
                                            // curve + lens profile, and a
                                            // baseline keeping them (the
                                            // disk form) lit ● the instant a
                                            // successful analyze landed.
                                            self.saved_recipe = self.recipe.clone();
                                            if pixels_ok {
                                                self.nav_stash.remove(&p);
                                                self.pixels_on_disk = self
                                                    .active_variant()
                                                    .and_then(|v| v.origin.clone());
                                            }
                                            self.forget_open_base();
                                            if backed.is_some() {
                                                self.refresh_versions();
                                            }
                                            let mut s = match backed {
                                                Some(n) => trf(
                                                    lang,
                                                    "AI develop applied · saved (previous save backed up as v{n})",
                                                    &[("n", &n.to_string())],
                                                ),
                                                None => tr(lang, "AI develop applied · saved to recipe.json").to_string(),
                                            };
                                            if autoshop::decode::is_raw(&p)
                                                && let Err(e) =
                                                    autoshop::pipeline::write_xmp(&p, &stamped)
                                            {
                                                let t = trf(
                                                    lang,
                                                    "recipe saved — but the Lightroom XMP failed: {err}",
                                                    &[("err", &e.to_string())],
                                                );
                                                self.toast(ToastKind::Error, t.clone());
                                                s = t;
                                            }
                                            s
                                        }
                                        Err(e) => {
                                            // The old save was already COPIED
                                            // to v<N>, so nothing is lost —
                                            // but this photo's working save
                                            // failed, and a status line alone
                                            // scrolls away.
                                            let t = trf(
                                                lang,
                                                "AI develop applied — but saving the sidecar failed: {err}",
                                                &[("err", &e.to_string())],
                                            );
                                            self.toast(ToastKind::Error, t.clone());
                                            self.refresh_versions();
                                            t
                                        }
                                    }
                                }
                                    })
                                },
                            ) {
                                Ok(status) => status,
                                Err(e) => {
                                    self.persist_postponed(
                                        &e,
                                        "AI develop applied — but NOT saved: this photo is being changed by another Autoshop process ({err}); Ctrl+S retries",
                                        &[],
                                    );
                                    self.status.clone()
                                }
                            },
                            None => tr(lang, "AI develop applied").into(),
                        } };
                    }
                    Err(e) => {
                        self.fail(tr(lang, "analyze failed"), e);
                    }
                }}
                Msg::Exported(Ok(p)) => {
                    self.batch_progress = None; // the bar belongs to ONE batch run
                    self.done(trf(lang, "exported → {path}", &[("path", p.as_str())]));
                }
                Msg::Exported(Err(e)) => {
                    self.batch_progress = None;
                    self.fail(tr(lang, "export failed"), e);
                }
                Msg::BatchProgress { done, total } => {
                    self.batch_progress = Some((done, total));
                    self.status = trf(
                        lang,
                        "Batch-rendering {done}/{total} → ./out …",
                        &[("done", &done.to_string()), ("total", &total.to_string())],
                    );
                }
                Msg::Segmented(res) => match res {
                    Ok((label, path)) => {
                        let path_s = path.to_string_lossy().into_owned();
                        // One raster per (photo, target): a re-run refreshed the
                        // SAME file, so re-select the mask that already
                        // references it instead of stacking a duplicate (whose
                        // sliders would all be 0 — an inert-looking copy).
                        // Compare by RASTER NAME, not full string: the stored
                        // reference may be bare ("mask-sky.png"), legacy
                        // ("out/<stem>.mask-sky.png") or absolute, while
                        // `path` is the absolute target — the kind-name is
                        // unique per photo per target by construction. On a
                        // hit, re-point the stored path at the freshly written
                        // raster so a stale reference is revived instead of
                        // stranding the user's sliders on a dead mask.
                        let new_name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                            .to_string();
                        let stem_prefix = self
                            .src_path
                            .as_deref()
                            .map(|p| format!("{}.", autoshop::pipeline::stem(p)));
                        let existing = self.recipe.masks.iter_mut().enumerate().find_map(
                            |(i, m)| match &mut m.mask {
                                autoshop::recipe::MaskGeometry::Bitmap { path: p } => {
                                    let stored = std::path::Path::new(p.as_str())
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or_default();
                                    let bare = stem_prefix
                                        .as_deref()
                                        .and_then(|sp| stored.strip_prefix(sp))
                                        .unwrap_or(stored);
                                    // Strip the NEW name symmetrically: a
                                    // legacy-out target carries the stem
                                    // prefix that a central bare name lacks —
                                    // one-sided stripping missed the match
                                    // and stacked an inert duplicate.
                                    let new_bare = stem_prefix
                                        .as_deref()
                                        .and_then(|sp| new_name.strip_prefix(sp))
                                        .unwrap_or(&new_name);
                                    // Compare by name FAMILY ("mask-sky-3.png"
                                    // → "mask-sky"): rasters are claim_raster
                                    // uniques now, so a rerun's new name must
                                    // still REPOINT the existing entry.
                                    (mask_family(bare) == mask_family(new_bare)).then(|| {
                                        *p = path_s.clone();
                                        i
                                    })
                                }
                                _ => None,
                            },
                        );
                        let reused = existing.is_some();
                        match existing {
                            Some(i) => self.sel_mask = Some(i),
                            None => {
                                self.recipe.masks.push(autoshop::recipe::LocalAdjustment {
                                    mask: autoshop::recipe::MaskGeometry::Bitmap {
                                        path: path_s,
                                    },
                                    name: label.clone(),
                                    ..Default::default()
                                });
                                self.sel_mask = Some(self.recipe.masks.len() - 1);
                            }
                        }
                        self.overlay_stale = true; // fresh raster ⇒ fresh coverage
                        self.dirty = true; // committed-snapshot makes this one undo step
                        self.busy = false;
                        // A rerun REFRESHES the existing mask's raster — its
                        // sliders may already apply; claiming "added, adjust
                        // sliders" there was false on both counts.
                        self.status = if reused {
                            trf(
                                lang,
                                "AI「{what}」mask refreshed — the existing mask now uses the new selection (its sliders still apply)",
                                &[("what", &label)],
                            )
                        } else {
                            trf(
                                lang,
                                "AI「{what}」mask added — adjust its sliders (exposure / contrast / saturation…) to take effect",
                                &[("what", &label)],
                            )
                        };
                    }
                    Err(e) => {
                        self.fail(tr(lang, "AI segmentation failed"), e);
                    }
                },
                Msg::MaskRefined(res) => match res {
                    Ok((idx, stored_ref, out)) => {
                        // Index + stored reference validated TOGETHER: the
                        // strip may have been edited while the worker decoded
                        // the full-res source, and a bare path search could
                        // repoint the wrong mask when two masks share one
                        // raster. Index-with-matching-path wins; else the
                        // unique path match; else say so — never guess.
                        let out_s = out.to_string_lossy().into_owned();
                        let at = |m: &autoshop::recipe::LocalAdjustment| {
                            matches!(&m.mask, MaskGeometry::Bitmap { path } if *path == stored_ref)
                        };
                        let hit = match self.recipe.masks.get(idx) {
                            Some(m) if at(m) => Some(idx),
                            _ => {
                                let matches: Vec<usize> = self
                                    .recipe
                                    .masks
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, m)| at(m))
                                    .map(|(i, _)| i)
                                    .collect();
                                // Only an UNAMBIGUOUS survivor is adopted.
                                (matches.len() == 1).then(|| matches[0])
                            }
                        };
                        match hit {
                            Some(i) => {
                                self.recipe.masks[i].mask = MaskGeometry::Bitmap { path: out_s };
                                self.sel_mask = Some(i);
                                self.dirty = true;
                                self.overlay_stale = true;
                                self.busy = false;
                                self.status = tr(
                                    lang,
                                    "mask refined to full resolution — boundaries now follow the source's own edges",
                                )
                                .into();
                            }
                            None => {
                                self.busy = false;
                                let t = trf(
                                    lang,
                                    "the mask changed while refining — the result was saved at {path} but not applied",
                                    &[("path", &out.display().to_string())],
                                );
                                self.toast(ToastKind::Error, t.clone());
                                self.status = t;
                            }
                        }
                    }
                    Err(e) => {
                        self.fail(tr(lang, "mask refine failed"), e);
                    }
                },
                Msg::Folder(boxed) => match *boxed {
                    Ok((dir, list)) => {
                        let n = list.len();
                        self.gallery = list;
                        self.gallery_dir = Some(dir);
                        self.gallery_gen += 1; // invalidate any in-flight old thumbs
                        self.thumbs.clear();
                        self.thumb_requested.clear();
                        // Keyed by gallery INDEX, so it is meaningless across
                        // folders: without this, index 0 failing three times
                        // here silently denied index 0 a thumbnail in every
                        // folder opened afterwards.
                        self.thumb_fail.clear();
                        self.thumb_inflight = 0;
                        self.edited_badge.clear(); // badge stats belong to the old folder
                        self.selected = None;
                        self.multi_sel.clear(); // indices belong to the old folder
                        self.busy = false;
                        self.status = if n == 0 {
                            // The plural line says "click a thumbnail" — with
                            // zero rows there is none to click.
                            tr(lang, "no photos found in this folder").to_string()
                        } else if n == 1 {
                            tr(lang, "1 photo — click a thumbnail to open").to_string()
                        } else {
                            trf(lang, "{n} photos — click a thumbnail to open", &[("n", &n.to_string())])
                        };
                    }
                    Err(e) => {
                        self.fail(tr(lang, "scan failed"), e);
                    }
                },
                Msg::Thumb { generation, idx, img } => {
                    // Ignore thumbnails from a previous folder generation (their
                    // inflight count was already discarded when the folder changed).
                    if generation == self.gallery_gen {
                        self.thumb_inflight = self.thumb_inflight.saturating_sub(1);
                        match *img {
                            Ok(im) => {
                                let tex = ctx.load_texture(
                                    format!("thumb{idx}"),
                                    to_color_image(&im),
                                    egui::TextureOptions::LINEAR,
                                );
                                self.thumbs.insert(idx, tex);
                            }
                            Err(_) => {
                                // Count the failure and release the queue slot:
                                // `request_thumb` allows three attempts, so a
                                // transient decode failure (AV lock, a slow
                                // share) still recovers on a later frame while
                                // an undecodable file stops for good instead of
                                // respawning decode threads every frame.
                                *self.thumb_fail.entry(idx).or_insert(0) += 1;
                                self.thumb_requested.remove(&idx);
                            }
                        }
                    }
                }
                Msg::MasterLoaded { photo, origin, img } => {
                    // The in-flight marker clears on EVERY outcome — photo
                    // mismatch included — or one failed decode would block
                    // all retries for the rest of the session.
                    self.master_loads.remove(&(photo.clone(), origin.clone()));
                    // Install by identity, not index: only while the SAME
                    // photo is open, and only into strip entries that still
                    // reference this exact master and still await pixels.
                    if self.src_path.as_ref() == Some(&photo) {
                        match *img {
                            Ok(im) => {
                                let arc = Arc::new(im);
                                let mut hit_active = false;
                                for (i, v) in self.variants.iter_mut().enumerate() {
                                    if v.base.is_none() && v.origin.as_ref() == Some(&origin) {
                                        v.base = Some(arc.clone());
                                        hit_active |= i == self.active;
                                    }
                                }
                                if hit_active {
                                    // The canvas was showing the disclosed
                                    // source stand-in — swap in the real
                                    // pixels now that they exist.
                                    self.refresh_active_pixels(ctx);
                                    self.set_canvas_status("restored the canvas pixels");
                                }
                            }
                            Err(e) => {
                                let t = trf(
                                    lang,
                                    "this variant's saved master could not be loaded ({err}) — showing the un-retouched source develop instead",
                                    &[("err", &e.to_string())],
                                );
                                self.toast(ToastKind::Error, t);
                            }
                        }
                    }
                }
                Msg::Progress(epoch, m) => {
                    // Liveness lines from a running generative worker. Epoch-
                    // gated: after Cancel + an immediate re-run, the abandoned
                    // worker's heartbeats must not overwrite the new task's
                    // status line.
                    if self.busy && self.gen_cancel.is_some() && epoch == self.gen_epoch {
                        self.status = m;
                    }
                }
                Msg::Retouched(epoch, boxed) => {
                    if epoch != self.gen_epoch {
                        // A cancelled task's late result: the user already
                        // moved on — never let it mutate the canvas. Its ./out
                        // artifact stays on disk (harmless, and Err is just as
                        // silent).
                        continue;
                    }
                    self.gen_cancel = None;
                    match *boxed {
                    Ok((img, msg, saved, kind)) => {
                        self.clear_mask();
                        match kind {
                            RetouchKind::NewGenerated => {
                                // Whole-frame reimagine → a NEW「AI 生成」variant
                                // whose base IS this raster. Auto-switch to it, so
                                // editing works on the generated pixels — a slider
                                // no longer reverts to the source develop. Its path
                                // is the reverse-fit / full-res export source.
                                self.push_variant(
                                    Variant {
                                        kind: VariantKind::Generated,
                                        recipe: EditRecipe::default(),
                                        base: Some(Arc::new(img)),
                                        origin: Some(saved),
                                        thumb: None,
                                    },
                                    ctx,
                                );
                            }
                            RetouchKind::InPlace => {
                                // fill/heal/clone/denoise: a pixel touch-up of the
                                // NEUTRAL-DEVELOP base (never the developed
                                // rendition — the recipe keeps rendering ON TOP,
                                // so baking developed pixels would cook the tone
                                // twice). Bake it into the active variant's base
                                // AND repoint its origin at the saved full-res
                                // artifact, so export / reverse-fit / a further
                                // retouch all follow the retouched pixels.
                                let img = Arc::new(img);
                                let (mw, mh) = img.dimensions();
                                if let Some(v) = self.variants.get_mut(self.active) {
                                    v.base = Some(img.clone());
                                    v.origin = Some(saved);
                                }
                                // The InPlace master is a NEUTRAL develop (see
                                // above) — Before shows it under the recipe's
                                // own camera curve, exactly like the source
                                // compare; an empty curve here sat 0.6–1.4 EV
                                // under After's starting point.
                                let curve = self.recipe.base_curve.clone();
                                self.set_before(ctx, &img, &curve);
                                // New baked base ⇒ the cached masks-cleared
                                // reference develop no longer matches it.
                                self.overlay_ref = None;
                                self.overlay_stale = true;
                                self.base_preview = Some(img);
                                // Keep the paint canvas sized to the new base (the
                                // retouch result can differ in dimensions — e.g. a
                                // non-square reimagine origin).
                                self.mask_paint = Some(image::RgbaImage::new(mw, mh));
                                self.mask_tex = None;
                                self.mask_dirty = false;
                                self.dirty = true;
                                // The pixel swap must enter history THIS frame
                                // — a same-frame Ctrl+Z otherwise undoes an
                                // older recipe step over the new pixels.
                                self.commit_now();
                                // The decoded-base cache now describes pixels
                                // this photo no longer shows.
                                self.forget_open_base();
                            }
                        }
                        self.done(msg);
                    }
                    Err(e) => {
                        self.fail(tr(lang, "retouch failed"), e);
                    }
                    }
                }
                Msg::Fitted(boxed) => match *boxed {
                    // Either way the worker may have persisted a recipe.json
                    // (an Err can land after that write) — recompute badges.
                    Ok((recipe, note, persisted)) => {
                        self.edited_badge.clear();
                        // Advance the ● baseline ONLY when the store actually
                        // holds the fit. A backup-gate refusal means nothing
                        // was written: leaving the baseline alone keeps
                        // ● unsaved lit and lets nav_stash protect the fit —
                        // the ordinary unsaved-edit path takes over.
                        if persisted {
                            self.saved_recipe = recipe.clone();
                            self.nav_stash.remove(
                                &self.src_path.clone().unwrap_or_default(),
                            );
                            // The worker cleared pixels.json alongside the
                            // recipe (a fit is source-based) — keep the
                            // per-frame ● pixel comparison's disk mirror in
                            // step with it.
                            self.pixels_on_disk = None;
                        }
                        self.refresh_versions();
                        // The generated look, solved back into an editable recipe,
                        // becomes a NEW「反推」variant: base = the source neutral
                        // (same negative as Original), look carried by the recipe —
                        // so it is fully editable, exports XMP and renders at full
                        // resolution. Auto-switch to it.
                        self.push_variant(
                            Variant {
                                kind: VariantKind::Fitted,
                                recipe,
                                base: None,
                                origin: None,
                                thumb: None,
                            },
                            ctx,
                        );
                        self.done(note);
                    }
                    Err(e) => {
                        self.edited_badge.clear();
                        // The pre-fit snapshot may exist even when the fit
                        // errored — keep the Versions list truthful.
                        self.refresh_versions();
                        self.fail(tr(lang, "Reverse-fit failed"), e);
                    }
                },
                Msg::Styled(boxed) => match *boxed {
                    Ok((prompt, note)) => {
                        // Into the Reimagine prompt: ready to restyle OTHER photos.
                        self.reimagine_prompt = prompt;
                        self.done(note);
                    }
                    Err(e) => {
                        self.fail(tr(lang, "Style extraction failed"), e);
                    }
                },
                Msg::Pasted(res) => {
                    // Sidecars were written (possibly partially on error) —
                    // recompute the gallery badges either way.
                    self.edited_badge.clear();
                    match res {
                        Ok(s) => {
                            // The open photo's paste is on disk: advance the
                            // ● baseline so the marker doesn't cry wolf. Only
                            // on FULL success — a partial failure could be
                            // exactly the open photo, which must keep warning.
                            // Guard on the photo still being open, and compare
                            // nothing: edits made while the worker ran leave
                            // recipe != saved_recipe, correctly re-lighting ●.
                            if let Some((p, r)) = self.pasted_open.take()
                                && self.src_path.as_ref() == Some(&p)
                            {
                                self.saved_recipe = r;
                                self.nav_stash.remove(&p);
                            }
                            self.done(s);
                        }
                        Err(e) => {
                            self.pasted_open = None;
                            self.fail(tr(lang, "batch paste"), e);
                        }
                    }
                }
                Msg::LegacyImported(res) => {
                    self.edited_badge.clear(); // imported sidecars light ● badges
                    match res {
                        Ok(s) => self.done(s),
                        Err(e) => self.fail(tr(lang, "import failed"), e),
                    }
                }
                Msg::Models(res) => match res {
                    Ok(ids) => {
                        let chat: Vec<String> = ids
                            .iter()
                            .filter(|s| autoshop::openai_models::is_chat_model(s))
                            .cloned()
                            .collect();
                        let imgs: Vec<String> = ids
                            .iter()
                            .filter(|s| autoshop::openai_models::is_image_model(s))
                            .cloned()
                            .collect();
                        self.settings.status = trf(
                            lang,
                            "fetched {n} models ({chat} chat · {img} image)",
                            &[
                                ("n", &ids.len().to_string()),
                                ("chat", &chat.len().to_string()),
                                ("img", &imgs.len().to_string()),
                            ],
                        );
                        self.settings.chat_choices = chat;
                        self.settings.image_gen_choices = imgs;
                        self.settings.fetching_models = false;
                    }
                    Err(e) => {
                        self.settings.fetching_models = false;
                        self.settings.status =
                            trf(lang, "fetch failed: {err}", &[("err", &e.to_string())]);
                    }
                },
            }
        }
        // Keep the frame loop alive while any worker (analyze/export/thumbs/models/
        // master decodes) runs — but at a 100 ms poll, not frame rate: worker
        // completion only surfaces through the mpsc poll above, and a full-rate
        // repaint burned CPU for the whole life of a stalled 600 s AI call. Input
        // still repaints immediately; 100 ms only bounds COMPLETION latency.
        // `master_loads` is in the gate because a cold-variant master decode is
        // NOT `busy`: without it, MasterLoaded sat unread until the next input
        // and the canvas kept showing the disclosed stand-in (16-lane scan L06).
        if self.busy
            || self.thumb_inflight > 0
            || self.settings.fetching_models
            || !self.master_loads.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// One labelled slider; double-click resets to `default` (the Lightroom
    /// gesture), hover + ↑/↓ nudges by a domain-appropriate step (Shift ×10 —
    /// LR's arrow grammar; ←/→ stay library navigation). Returns true if the
    /// value changed this frame. Callers pass an already-translated `label`.
    /// Whole-number domains (range ≥ 20: the ±100 family, 0..150, hue °) snap
    /// to integers while dragging, like Lightroom — the web UI already had to
    /// work around raw floats ("13.4849996…" overflowing its value box).
    fn slider(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
        default: f32,
    ) -> bool {
        let feel =
            if max - min >= 20.0 { SliderFeel::Int } else { SliderFeel::Frac };
        Self::slider_impl(ui, lang, label, value, min, max, default, feel)
    }

    /// A 0..=1-stored fraction shown on Lightroom's 0..100 track (Amount,
    /// feathers, range tolerance): the panel used to mix "Amount 0.65" with
    /// "Shadows 40" in one column, which read as two unit systems. Storage
    /// stays the fraction (the XMP contract); only the display scales. The
    /// ×100 track crosses the ≥ 20 width rule above, so these snap to whole
    /// numbers exactly like LR's.
    fn slider_pct(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        max: f32,
        default: f32,
    ) -> bool {
        let mut disp = *value * 100.0;
        let changed =
            Self::slider(ui, lang, label, &mut disp, 0.0, max * 100.0, default * 100.0);
        if changed {
            *value = disp / 100.0;
        }
        changed
    }

    /// Sub-unit precision variant (Straighten °): the whole-number snap of the
    /// wide-range class would destroy 0.1° levelling on a ±45 track.
    fn slider_fine(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
        default: f32,
    ) -> bool {
        Self::slider_impl(ui, lang, label, value, min, max, default, SliderFeel::Fine)
    }

    /// Log-scaled variant for values whose useful band is a small fraction of
    /// the range — Temp (K): 2000–40000 linear puts 4000–8000 K (where nearly
    /// every photo lives) on ~40 px of a 320 px track, making a routine 100 K
    /// nudge sub-pixel.
    fn slider_log(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
        default: f32,
    ) -> bool {
        Self::slider_impl(ui, lang, label, value, min, max, default, SliderFeel::LogK)
    }

    #[allow(clippy::too_many_arguments)] // private impl detail shared by three thin public shapes
    fn slider_impl(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
        default: f32,
        feel: SliderFeel,
    ) -> bool {
        let range = max - min;
        // (drag snap, ↑/↓ nudge, shown decimals) per feel class. Frac's snap
        // matches its 2 shown decimals exactly — egui's fixed_decimals ALSO
        // rounds the stored value (slider.rs), so a finer snap would silently
        // lose to the display rounding. The Frac NUDGE is floored at that
        // same 0.01 grid: a finer step (range < 1) rounds back onto the
        // current value below and the `next != *value` guard drops it —
        // ArrowDown on the 0–0.5 Feather was a permanent no-op (D13).
        // EV: 0.01-drag / 0.1-arrow. LogK's
        // nudge is ~1% of the current Kelvin (≈55 K at 5500) — a fixed step
        // would be sub-pixel at one end of a log track and huge at the other.
        let (snap, nudge, decimals) = match feel {
            SliderFeel::Int => (1.0, 1.0, 0usize),
            SliderFeel::Fine => (0.1, 0.1, 1),
            SliderFeel::Frac => (0.01, (range / 100.0).max(0.01), 2),
            SliderFeel::LogK => (1.0, ((*value).abs() * 0.01).max(1.0).round(), 0),
        };
        let resp = ui
            .add(
                egui::Slider::new(value, min..=max)
                    .logarithmic(matches!(feel, SliderFeel::LogK))
                    .step_by(snap)
                    .fixed_decimals(decimals)
                    .text(label),
            )
            .on_hover_text(tr(lang, "double-click resets · hover + ↑/↓ nudges (Shift ×10)"));
        if resp.double_clicked() && *value != default {
            *value = default;
            return true;
        }
        let mut changed = resp.changed();
        // The nudge is gated on "no widget owns the keyboard", so arrows typed
        // into a text field can never double-apply here.
        if resp.hovered() && ui.memory(|m| m.focused().is_none()) {
            let shift = ui.input(|i| i.modifiers.shift);
            let (up, down) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                        || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                        || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown),
                )
            });
            if up != down {
                let step = nudge * if shift { 10.0 } else { 1.0 };
                // Round to the class's shown decimals — direct assignment
                // bypasses egui's set_value rounding, and an inherited
                // 13.485 must nudge to 14, not to a hidden 14.485.
                let f = 10f32.powi(decimals as i32);
                let next = ((*value + if up { step } else { -step }) * f).round() / f;
                let next = next.clamp(min, max);
                if next != *value {
                    *value = next;
                    changed = true;
                }
            }
        }
        changed
    }

    /// Left-most panel: the working-folder thumbnail gallery. Only visible rows
    /// are laid out (show_rows) and only their thumbnails are queued to decode.
    fn gallery_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            ui.heading(tr(lang, "Library"));
            if ui.button(tr(lang, "Open folder…")).clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.open_folder(dir);
            }
        });
        if let Some(d) = &self.gallery_dir {
            let dir = d.display().to_string();
            let cnt = self.gallery.len().to_string();
            ui.label(
                egui::RichText::new(trf(lang, "{dir} · {count} photos", &[("dir", &dir), ("count", &cnt)]))
                    .weak()
                    .small(),
            );
        }
        // Batch: copy the open photo's recipe → Ctrl+click a selection → paste.
        // Lightroom's "sync settings" for the whole working folder. Wrapped:
        // four dynamically sized controls in a fixed row clipped at narrow
        // panel widths.
        ui.horizontal_wrapped(|ui| {
            // `!busy` like both siblings below. `open_path` re-points
            // `src_path` immediately but `recipe` only refreshes when the
            // decode lands, so during an open this pairs photo A's recipe
            // (below) with photo B's path — and `copied_from` is what
            // suppresses the "bitmap mask(s) not pasted" warning, so the
            // mismatch INVERTS that guard and writes A's raster masks into
            // B's saved develop.
            ui.add_enabled_ui(self.src_path.is_some() && !self.busy, |ui| {
                if ui
                    .small_button(tr(lang, "⎘ Copy recipe"))
                    .on_hover_text(tr(lang, "Copy every develop setting from the current photo"))
                    .clicked()
                {
                    // Flush a pending mask rename first — the clipboard must
                    // carry the name the user sees in the box (U10; CX5-9).
                    self.commit_mask_name_buf();
                    self.copied = Some(self.recipe.clone());
                    self.copied_from = self.src_path.clone();
                    self.status = tr(lang, "Recipe copied — Ctrl/⌘+click to pick several, then “Paste to selected”").to_string();
                }
            });
            let n = self.multi_sel.len();
            ui.add_enabled_ui(self.copied.is_some() && n > 0 && !self.busy, |ui| {
                let n_s = n.to_string();
                if ui
                    .small_button(trf(lang, "⇩ Paste to selected ({n})", &[("n", &n_s)]))
                    .on_hover_text(tr(lang, "Writes each photo's develop into your develop store (recipe JSON; RAW also gets a Lightroom XMP). Leaves library files untouched, renders nothing."))
                    .clicked()
                {
                    self.start_paste();
                }
            });
            ui.add_enabled_ui(n > 0 && !self.busy, |ui| {
                let n_s = n.to_string();
                if ui
                    .small_button(trf(lang, "🖼 Render selected ({n})", &[("n", &n_s)]))
                    .on_hover_text(tr(
                        lang,
                        "Each renders by its own saved develop from the store (neutral develop if none) → ./out/<name>.developed.*, using the current format / long-edge / sharpening / quality; AI Denoise sits out the batch.",
                    ))
                    .clicked()
                {
                    self.start_batch_render();
                }
            });
            if n > 0 && ui.small_button("✕").on_hover_text(tr(lang, "Clear selection")).clicked() {
                self.multi_sel.clear();
            }
        });
        if self.copied.is_some() {
            ui.checkbox(&mut self.paste_geometry, tr(lang, "Include crop / straighten when pasting"))
                .on_hover_text(tr(lang, "Off by default — composition rarely transfers between photos"));
        }
        ui.separator();
        if self.gallery.is_empty() {
            ui.label(egui::RichText::new(tr(lang, "Open a folder to browse your photos here.")).weak());
            return;
        }

        let count = self.gallery.len();
        // Borrow only the fields the row closure reads; collect actions to apply
        // after (request_thumb / open_gallery_index both need &mut self).
        let thumbs = &self.thumbs;
        let edited_badge = &mut self.edited_badge;
        let gallery = &self.gallery;
        let selected = self.selected;
        let multi_sel = &self.multi_sel;
        let colors = self.theme.colors();
        let mut to_open: Option<usize> = None;
        let mut to_toggle: Option<usize> = None;
        let mut to_request: Vec<usize> = Vec::new();
        let mut visible: std::ops::Range<usize> = 0..0;

        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(i) = self.gallery_scroll_to.take() {
            // Keyboard navigation moved the selection: jump the viewport so the
            // highlighted row sits roughly centred. show_rows only lays out
            // visible rows, so scroll_to_me can never reach an off-screen row —
            // the offset (rows are fixed-height) is the reliable route.
            let target = (i as f32 * GALLERY_ROW_H - ui.available_height() * 0.4).max(0.0);
            scroll = scroll.vertical_scroll_offset(target);
        }
        scroll
            .show_rows(ui, GALLERY_ROW_H, count, |ui, range| {
                visible = range.clone();
                for i in range {
                    let path = &gallery[i];
                    let is_sel = selected == Some(i);
                    let is_multi = multi_sel.contains(&i);
                    let fill = if is_sel {
                        colors.sel_bg
                    } else if is_multi {
                        colors.sel_bg_dim
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    let resp = egui::Frame::none()
                        .fill(fill)
                        .inner_margin(egui::Margin::same(3.0))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if let Some(t) = thumbs.get(&i) {
                                    ui.add(
                                        egui::Image::new(SizedTexture::new(
                                            t.id(),
                                            egui::vec2(THUMB_W, THUMB_H),
                                        ))
                                        .rounding(3.0),
                                    );
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(THUMB_W, THUMB_H),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(rect, 3.0, colors.thumb_placeholder);
                                    to_request.push(i);
                                }
                                ui.vertical(|ui| {
                                    let mut name = egui::RichText::new(autoshop::pipeline::stem(path)).small();
                                    if is_sel {
                                        name = name.strong().color(colors.accent_text);
                                    }
                                    // Truncate, never wrap: show_rows assumes
                                    // every row is exactly GALLERY_ROW_H, and a
                                    // wrapped long filename would misalign the
                                    // whole virtualized scroll.
                                    ui.add(egui::Label::new(name).truncate());
                                    // Cached: 2 filesystem stats per visible row
                                    // per repaint (continuous while thumbnails
                                    // decode) added up to thousands of syscalls
                                    // per second. Dropped whenever this app
                                    // writes a sidecar or the folder changes.
                                    let edited = *edited_badge.entry(i).or_insert_with(|| {
                                        // Central store OR legacy ./out — a
                                        // pre-migration library keeps its ●.
                                        autoshop::store::has_develop(path)
                                    });
                                    let baked = !autoshop::decode::is_raw(path);
                                    ui.horizontal(|ui| {
                                        if is_multi {
                                            ui.label(egui::RichText::new(tr(lang, "✓ selected")).color(colors.accent_text).small());
                                        }
                                        if baked {
                                            ui.label(egui::RichText::new("PNG/TIFF").color(colors.accent_text).small());
                                        }
                                        if edited {
                                            ui.label(egui::RichText::new(tr(lang, "● edited")).color(colors.accent_text).small());
                                        }
                                    });
                                });
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        // Ctrl+click toggles the batch selection; plain click opens.
                        if ui.input(|inp| inp.modifiers.command) {
                            to_toggle = Some(i);
                        } else {
                            to_open = Some(i);
                        }
                    }
                }
            });

        for i in to_request {
            self.request_thumb(i);
        }
        // Texture LRU (bounded GPU memory): every scrolled-past row otherwise
        // pins its 160px texture until the folder changes (~68 KB each — a
        // 10k-photo folder reached ~0.7 GB). Keep a generous window around the
        // viewport; evicted indices drop their `requested` marker so a
        // scroll-back re-materialises from the disk cache in ~1 ms.
        const THUMB_TEX_CAP: usize = 1500;
        if self.thumbs.len() > THUMB_TEX_CAP {
            let keep = visible.start.saturating_sub(600)..visible.end + 600;
            self.thumbs.retain(|i, _| keep.contains(i));
            self.thumb_requested.retain(|i| keep.contains(i));
        }
        if let Some(i) = to_toggle
            && !self.multi_sel.remove(&i)
        {
            self.multi_sel.insert(i);
        }
        if let Some(i) = to_open {
            self.open_gallery_index(i);
        }
    }

    fn develop_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang; // Copy — never borrows self, safe inside egui closures.
        let mut changed = false;
        ui.heading(tr(lang, "Develop"));
        self.histogram_ui(ui);
        ui.add_space(4.0);

        // The AI area: everything one Analyze run reads or writes, in ONE
        // place (UX batch — Direction/Refine/Style used to be scattered
        // across two toolbar rows with Undo/Redo in between). Open whenever
        // there's a verdict to show; the inputs are always present.
        let ai_active = self.verdict.is_some() || !self.guidance.is_empty();
        egui::CollapsingHeader::new(section_title(tr(lang, "AI"), ai_active))
            .id_salt("sec_verdict")
            .default_open(true)
            .show(ui, |ui| {
                if let Some(v) = &self.verdict {
                    // Accept reads calm; anything else (Revise/Reject)
                    // gets the warn colour so it can't be skimmed past.
                    let col = if v.starts_with("Accept") {
                        ui.visuals().strong_text_color()
                    } else {
                        ui.visuals().warn_fg_color
                    };
                    ui.label(egui::RichText::new(v).color(col));
                }
                if !self.rationale.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("“{}”", self.rationale))
                            .italics()
                            .weak(),
                    );
                }
                ui.label(tr(lang, "Direction"))
                    .on_hover_text(tr(lang, "Free-text direction for AI Analyze — e.g. warmer and moodier"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.guidance)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr(lang, "e.g. warmer and moodier, lift the shadows")),
                );
                // The prompt's triggers sit DIRECTLY under it (user feedback:
                // the toolbar Analyze button sat nowhere near the text it
                // consumes). TWO explicit verbs replace the old pre-armed
                // 「Refine」 checkbox — a mode you had to remember to tick
                // (and untick) before clicking is exactly the kind of hidden
                // state a button-per-intent design removes. Wrapped so the
                // Style slider never strands off-panel at narrow widths.
                ui.horizontal_wrapped(|ui| {
                    // `analyze_inflight` too, or ✕ leaves these ENABLED while
                    // `start_analyze` silently refuses (it must refuse — the
                    // cancelled call is still on the wire and still billing).
                    // A button that looks live and does nothing, for up to the
                    // 600 s stall budget, with the status line saying the app
                    // is free, is worse than one that says why it is greyed.
                    let ready =
                        self.src_path.is_some() && !self.busy && !self.analyze_inflight;
                    let waiting = self.analyze_inflight && !self.busy;
                    let why = |ui: egui::Response| {
                        if waiting {
                            ui.on_hover_text(tr(
                                lang,
                                "the cancelled AI call is still running (and still billed) — this re-arms when it finishes or times out",
                            ))
                        } else {
                            ui
                        }
                    };
                    if why(ui
                        .add_enabled(ready, egui::Button::new(tr(lang, "AI Analyze")))
                        .on_hover_text(tr(lang,
                            "AI proposes a recipe from scratch (GPT proposal + validation), written into \
                             the sliders — undoable. Uses the Direction above; Style steers it.",
                        )))
                        .clicked()
                    {
                        self.start_analyze(false);
                    }
                    // Refining a neutral edit IS analyzing — disable the verb
                    // until there is an edit to refine.
                    let has_edit = ready && !self.recipe.is_noop();
                    if why(ui
                        .add_enabled(has_edit, egui::Button::new(tr(lang, "AI Refine")))
                        .on_hover_text(tr(lang,
                            "Adjust the CURRENT edit instead of proposing from scratch — your sliders are \
                             the starting point (enabled once the edit is non-neutral).",
                        )))
                        .clicked()
                    {
                        self.start_analyze(true);
                    }
                    ui.separator();
                    ui.label(tr(lang, "Style")).on_hover_text(
                        tr(lang, "Personal style strength: how far AI proposals lean toward your past XMP editing habits (0 = ignore)"),
                    );
                    ui.add(egui::Slider::new(&mut self.style_strength, 0.0..=1.0).show_value(false))
                        .on_hover_text(
                            tr(lang, "Personal style strength: how far AI proposals lean toward your past XMP editing habits (0 = ignore)"),
                        );
                    ui.label(format!("{:.0}%", self.style_strength * 100.0));
                });
            });

        // Lightroom-style grouping: a wall of 16 sliders scans terribly; four
        // titled sections (tone open, the rest by activity) scan at a glance.
        // A section whose values are non-neutral shows a ● so a collapsed
        // active adjustment is never invisible. Flags are snapshot up front —
        // Copy bools, so no borrow spans the section closures (E0500).
        let (presence_active, detail_active, hsl_active, grade_active, curves_active) = {
            let r = &self.recipe;
            (
                r.clarity != 0.0 || r.dehaze != 0.0 || r.vibrance != 0.0 || r.saturation != 0.0,
                r.sharpening != 0.0 || r.noise_reduction != 0.0,
                !r.hsl.is_neutral(),
                !r.color_grade.is_neutral(),
                !r.tone_curve.is_empty()
                    || !r.red_curve.is_empty()
                    || !r.green_curve.is_empty()
                    || !r.blue_curve.is_empty(),
            )
        };
        let tone_active = {
            let r = &self.recipe;
            r.temperature_k.is_some()
                || r.tint != 0.0
                || r.exposure_ev != 0.0
                || r.contrast != 0.0
                || r.highlights != 0.0
                || r.shadows != 0.0
                || r.whites != 0.0
                || r.blacks != 0.0
        };

        egui::CollapsingHeader::new(section_title(tr(lang, "Tone & WB"), tone_active))
            .id_salt("sec_tone")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // A nonzero tint IS a WB edit (recipes saved before the
                    // uncheck-zeroes-tint fix can carry one with no Temp) —
                    // showing "off = as-shot" over an active tint, with its
                    // slider disabled, was a lie with no way out.
                    let mut custom_wb =
                        self.recipe.temperature_k.is_some() || self.recipe.tint != 0.0;
                    if ui.checkbox(&mut custom_wb, tr(lang, "Custom white balance (off = as-shot)")).changed() {
                        // Arm AT the as-shot anchor when stamped: checking
                        // the box must not shift the image — the slider then
                        // starts from the camera's real Kelvin, not 5500.
                        self.recipe.temperature_k =
                            if custom_wb { Some(self.recipe.as_shot_k.unwrap_or(5500.0)) } else { None };
                        if !custom_wb {
                            // "As-shot" is the WHOLE white balance: a custom
                            // Tint surviving the un-check kept a WB edit
                            // active behind a label claiming there was none.
                            self.recipe.tint = 0.0;
                        }
                        changed = true;
                    }
                    let label = if self.wb_picking { tr(lang, "💧 Click in image…") } else { tr(lang, "💧 Eyedropper") };
                    if ui
                        .small_button(label)
                        .on_hover_text(tr(lang,
                            "Click a spot in the image that should be neutral grey/white to auto-solve Temp/Tint (same forward model as the engine). Click again to cancel.",
                        ))
                        .clicked()
                    {
                        let on = !self.wb_picking;
                        self.disarm_tools();
                        self.wb_picking = on;
                        if on {
                            self.status = tr(lang, "WB eyedropper: click a spot that should be neutral grey/white").into();
                        }
                    }
                });
                if let (Some(k), Some(t)) = (self.recipe.as_shot_k, self.recipe.as_shot_tint) {
                    // The camera's own WB in absolute terms — the reference
                    // the Temp slider is anchored on for this photo. BOTH
                    // halves required: the era pin (anchor 5500, tint None)
                    // knows the anchor but NOT the camera, and showing it as
                    // "as shot" would be a false claim.
                    ui.weak(trf(
                        lang,
                        "as shot ≈ {k} K · tint {t}",
                        &[("k", &format!("{k:.0}")), ("t", &format!("{t:+.0}"))],
                    ));
                }
                // Double-click reset lands on the as-shot Kelvin when stamped
                // (the honest "no shift" position), 5500 for legacy recipes.
                let anchor_k = self.recipe.as_shot_k.unwrap_or(5500.0);
                if let Some(mut k) = self.recipe.temperature_k
                    && Self::slider_log(ui, lang, tr(lang, "Temp (K)"), &mut k, 2000.0, 40000.0, anchor_k)
                {
                    self.recipe.temperature_k = Some(k);
                    changed = true;
                }
                let custom_wb = self.recipe.temperature_k.is_some() || self.recipe.tint != 0.0;
                let r = &mut self.recipe;
                // Tint is HALF of the white balance: editable only under
                // Custom WB, or a tint move silently contradicted the
                // checkbox's "off = as-shot" promise. A legacy tint-only
                // recipe counts as Custom (see the checkbox above) so its
                // slider stays reachable.
                ui.add_enabled_ui(custom_wb, |ui| {
                    changed |= Self::slider(ui, lang, tr(lang, "Tint"), &mut r.tint, -100.0, 100.0, 0.0);
                });
                changed |= Self::slider(ui, lang, tr(lang, "Exposure"), &mut r.exposure_ev, -5.0, 5.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Contrast"), &mut r.contrast, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Highlights"), &mut r.highlights, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Shadows"), &mut r.shadows, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Whites"), &mut r.whites, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Blacks"), &mut r.blacks, -100.0, 100.0, 0.0);
            });

        // LR-Basic order (UX batch): Presence sits directly under Tone & WB —
        // the two halves of Lightroom's Basic panel — then Curves, then the
        // two colour sections; Detail moved BELOW them so colour work isn't
        // interrupted by sharpening. add_space fences each header so the ten
        // sections read as groups, not one undivided list.
        ui.add_space(6.0);
        egui::CollapsingHeader::new(section_title(tr(lang, "Presence"), presence_active))
            .id_salt("sec_presence")
            .default_open(true)
            .show(ui, |ui| {
                let r = &mut self.recipe;
                changed |= Self::slider(ui, lang, tr(lang, "Clarity"), &mut r.clarity, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Dehaze"), &mut r.dehaze, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Vibrance"), &mut r.vibrance, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Saturation"), &mut r.saturation, -100.0, 100.0, 0.0);
            });

        ui.add_space(6.0);
        // --- 曲线: master + RGB tone curves (engine + XMP already apply them,
        // this is purely the editing surface — Lightroom's panel order) --------
        egui::CollapsingHeader::new(section_title(tr(lang, "Curves"), curves_active))
            .id_salt("sec_curves")
            .default_open(false)
            .show(ui, |ui| {
                changed |= self.curve_editor(ui);
            });
        ui.add_space(6.0);

        ui.add_space(6.0);
        egui::CollapsingHeader::new(section_title(tr(lang, "Color Mixer (HSL)"), hsl_active))
            .id_salt("sec_hsl")
            .default_open(false)
            .show(ui, |ui| {
                // LR's mixer layout: pick ONE property (Hue / Saturation /
                // Luminance) and see all 8 bands at once. The old band picker
                // showed one band at a time — every other band's state was
                // invisible, the opposite of what a mixer is for.
                ui.horizontal(|ui| {
                    for (i, name) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
                        if ui.selectable_label(self.hsl_tab == i, tr(lang, name)).clicked() {
                            self.hsl_tab = i;
                        }
                    }
                    if ui.small_button(tr(lang, "↺ reset all")).clicked() {
                        self.recipe.hsl = Hsl::default();
                        changed = true;
                    }
                });
                let tab = self.hsl_tab;
                for (b, name) in HSL_BANDS.iter().enumerate() {
                    let v = match tab {
                        0 => &mut self.recipe.hsl.hue[b],
                        1 => &mut self.recipe.hsl.saturation[b],
                        _ => &mut self.recipe.hsl.luminance[b],
                    };
                    ui.horizontal(|ui| {
                        // Band swatch so rows scan like LR's mixer.
                        let (rect, _) = ui
                            .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, HSL_SWATCH[b]);
                        changed |= Self::slider(ui, lang, tr(lang, name), v, -100.0, 100.0, 0.0);
                    });
                }
            });

        ui.add_space(6.0);
        egui::CollapsingHeader::new(section_title(tr(lang, "Color Grading"), grade_active))
            .id_salt("sec_grade")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("grade_region")
                        .selected_text(tr(lang, GRADE_REGIONS[self.grade_region]))
                        .show_ui(ui, |ui| {
                            for (i, name) in GRADE_REGIONS.iter().enumerate() {
                                ui.selectable_value(&mut self.grade_region, i, tr(lang, name));
                            }
                        });
                    if ui.small_button(tr(lang, "↺ reset all")).clicked() {
                        self.recipe.color_grade = ColorGrade::default();
                        changed = true;
                    }
                });
                let cg = &mut self.recipe.color_grade;
                let (mut hue, mut sat, mut lum) = match self.grade_region {
                    0 => (cg.shadow_hue, cg.shadow_sat, cg.shadow_lum),
                    1 => (cg.midtone_hue, cg.midtone_sat, cg.midtone_lum),
                    2 => (cg.highlight_hue, cg.highlight_sat, cg.highlight_lum),
                    _ => (cg.global_hue, cg.global_sat, cg.global_lum),
                };
                let mut wheel_changed = false;
                wheel_changed |= Self::slider(ui, lang, tr(lang, "Hue"), &mut hue, 0.0, 360.0, 0.0);
                wheel_changed |= Self::slider(ui, lang, tr(lang, "Saturation"), &mut sat, 0.0, 100.0, 0.0);
                wheel_changed |= Self::slider(ui, lang, tr(lang, "Luminance"), &mut lum, -100.0, 100.0, 0.0);
                if wheel_changed {
                    match self.grade_region {
                        0 => { cg.shadow_hue = hue; cg.shadow_sat = sat; cg.shadow_lum = lum; }
                        1 => { cg.midtone_hue = hue; cg.midtone_sat = sat; cg.midtone_lum = lum; }
                        2 => { cg.highlight_hue = hue; cg.highlight_sat = sat; cg.highlight_lum = lum; }
                        _ => { cg.global_hue = hue; cg.global_sat = sat; cg.global_lum = lum; }
                    }
                    changed = true;
                }
                // Blending/Balance shape the WHOLE grade, not the region the
                // combo shows — a scope caption keeps them from reading as
                // "shadow Blending".
                ui.separator();
                ui.label(egui::RichText::new(tr(lang, "All regions")).weak().small());
                changed |= Self::slider(ui, lang, tr(lang, "Blending"), &mut cg.blending, 0.0, 100.0, 50.0);
                changed |= Self::slider(ui, lang, tr(lang, "Balance"), &mut cg.balance, -100.0, 100.0, 0.0);
            });

        // Look ends here — detail, then geometry (lens BEFORE crop: the lens
        // profile / manual distortion redefine the frame the crop sits in).
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(2.0);
        egui::CollapsingHeader::new(section_title(tr(lang, "Detail"), detail_active))
            .id_salt("sec_detail")
            .default_open(false)
            .show(ui, |ui| {
                {
                    let r = &mut self.recipe;
                    changed |= Self::slider(ui, lang, tr(lang, "Sharpening"), &mut r.sharpening, 0.0, 150.0, 0.0);
                    changed |=
                        Self::slider(ui, lang, tr(lang, "Noise Reduction"), &mut r.noise_reduction, 0.0, 100.0, 0.0);
                }
                // AI denoise as an ACTIVE op: run now, see it on canvas —
                // export-time denoise (the Export section toggle) stays for
                // batch/full-res workflows, but nobody should have to export
                // to find out what the denoiser does.
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ready = self.src_path.is_some() && !self.busy;
                    if ui
                        .add_enabled(ready, egui::Button::new(tr(lang, "🤖 AI Denoise now")))
                        .on_hover_text(tr(lang,
                            "Run the SCUNet GPU sidecar on this variant's pixels and show the result on canvas \
                             (undoable — bakes a clean base into the current variant; the develop sliders keep \
                             applying on top; first run downloads the model)",
                        ))
                        .clicked()
                    {
                        self.start_ai_denoise();
                    }
                    // RAW-only, exactly as the hover says — an enabled toggle
                    // on a baked source promised a mode that changes nothing.
                    let src_is_raw = self
                        .active_source_path()
                        .is_some_and(|p| autoshop::decode::is_raw(&p));
                    ui.add_enabled(src_is_raw, egui::Checkbox::new(&mut self.denoise_fullres, tr(lang, "Full-res")))
                        .on_hover_text(tr(lang,
                            "Denoise the full-sensor develop (slow, minutes on GPU); off = a ≤2048px working copy \
                             for a quick on-canvas result",
                        ));
                });
            });

        // --- 镜头校正: in-camera profile + manual corrections -----------------
        ui.add_space(6.0);
        let lens_active = self.recipe.lens_vignette != 0.0
            || self.recipe.lens_distortion != 0.0
            || self.recipe.lens_profile.vignette_active()
            || self.recipe.lens_profile.geometry_active();
        egui::CollapsingHeader::new(section_title(tr(lang, "Lens"), lens_active))
            .id_salt("sec_lens")
            .default_open(false)
            .show(ui, |ui| {
                // In-camera profile corrections (lensmeta): the manufacturer's
                // exact per-shot data read from the RAW itself — no lens
                // database. Toggling a component ON whose data a legacy
                // restore dropped re-fills it from this photo's metadata.
                let photo = self.photo_lens.clone();
                let lp = &mut self.recipe.lens_profile;
                let has_v = !lp.vignette.is_empty() || !photo.vignette.is_empty();
                let has_d = !lp.distortion.is_empty() || !photo.distortion.is_empty();
                let has_c = (!lp.ca_r.is_empty() || !photo.ca_r.is_empty())
                    && (!lp.ca_b.is_empty() || !photo.ca_b.is_empty());
                if has_v || has_d || has_c {
                    ui.label(tr(lang, "Profile corrections (from camera metadata)"));
                    let mut pch = false;
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(has_v, egui::Checkbox::new(&mut lp.vignette_on, tr(lang, "Vignetting")))
                            .on_hover_text(tr(lang, "The camera's own falloff map for this shot, applied in linear light"))
                            .changed()
                        {
                            if lp.vignette_on && lp.vignette.is_empty() {
                                lp.vignette = photo.vignette.clone();
                            }
                            pch = true;
                        }
                        if ui
                            .add_enabled(has_d, egui::Checkbox::new(&mut lp.distortion_on, tr(lang, "Distortion")))
                            .on_hover_text(tr(lang, "The camera's own geometric correction; masks and crop follow the corrected frame"))
                            .changed()
                        {
                            if lp.distortion_on && lp.distortion.is_empty() {
                                lp.distortion = photo.distortion.clone();
                            }
                            pch = true;
                        }
                        if ui
                            .add_enabled(has_c, egui::Checkbox::new(&mut lp.ca_on, tr(lang, "Chromatic aberration")))
                            .on_hover_text(tr(lang, "Per-channel radius correction: removes red/blue colour fringing at edges"))
                            .changed()
                        {
                            if lp.ca_on && (lp.ca_r.is_empty() || lp.ca_b.is_empty()) {
                                // Refill only the MISSING channel(s): `has_c`
                                // accepts the union of saved + photo data, so
                                // replacing BOTH from the photo could erase a
                                // saved channel the photo can't supply and
                                // leave the pair incomplete (CA that renders
                                // nothing — geometry_active needs both).
                                if lp.ca_r.is_empty() {
                                    lp.ca_r = photo.ca_r.clone();
                                }
                                if lp.ca_b.is_empty() {
                                    lp.ca_b = photo.ca_b.clone();
                                }
                            }
                            pch = true;
                        }
                    });
                    if pch {
                        self.overlay_stale = true;
                        changed = true;
                    }
                    ui.separator();
                } else if self.src_path.as_deref().is_some_and(autoshop::decode::is_raw) {
                    ui.label(
                        egui::RichText::new(tr(lang, "No in-camera lens correction data in this file"))
                            .weak()
                            .small(),
                    );
                    ui.separator();
                }
                changed |= Self::slider(ui, lang, tr(lang, "Vignette"), &mut self.recipe.lens_vignette, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Midpoint"), &mut self.recipe.lens_vignette_mid, 0.0, 100.0, 50.0);
                changed |= Self::slider(ui, lang, tr(lang, "Distortion"), &mut self.recipe.lens_distortion, -100.0, 100.0, 0.0);
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Vignette: positive brightens the corners (compensates falloff), negative darkens; a radial gain in linear light. Distortion: positive fixes barrel (wide-angle bulge), negative fixes pincushion (tele pinch); auto-scales to fill the frame, and masks / brush still position on the corrected image. Preview / export / XMP match. De-fringe in a later batch.",
                    ))
                    .weak()
                    .small(),
                );
            });

        // --- 裁剪 + 拉直: recipe.crop / straighten_deg (export + XMP paths) ---
        ui.add_space(6.0);
        let crop_active = self.recipe.crop.is_some() || self.recipe.straighten_deg != 0.0;
        egui::CollapsingHeader::new(section_title(tr(lang, "Crop"), crop_active))
            .id_salt("sec_crop")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let label = if self.crop_mode { tr(lang, "✅ Done") } else { tr(lang, "⛶ Enter crop") };
                    if ui.button(label).clicked() {
                        let on = !self.crop_mode;
                        self.disarm_tools();
                        self.crop_mode = on;
                    }
                    let prev_aspect = self.crop_aspect;
                    egui::ComboBox::from_id_salt("crop_aspect")
                        .selected_text(tr(lang, CROP_ASPECTS[self.crop_aspect].0))
                        .width(70.0)
                        .show_ui(ui, |ui| {
                            for (i, (name, _)) in CROP_ASPECTS.iter().enumerate() {
                                ui.selectable_value(&mut self.crop_aspect, i, tr(lang, name));
                            }
                        });
                    // LR applies a picked preset to the box IMMEDIATELY — the
                    // combo used to only arm the ratio for the NEXT handle
                    // drag, so the UI said 4:5 while preview/export kept the
                    // old shape (D13). Consumed in handle_crop, where the
                    // view dims live — and armed ONLY while the crop tool is
                    // up: with the tool away the flag would survive until
                    // some later session's crop entry and rewrite the box as
                    // a surprise (Codex batch-38); tool-off keeps the old
                    // arm-for-the-next-drag meaning.
                    if self.crop_aspect != prev_aspect && self.crop_mode {
                        self.crop_aspect_pending = true;
                    }
                    if ui.button(tr(lang, "Clear crop")).clicked()
                        && self.recipe.crop.take().is_some()
                    {
                        // Through the panel's own change path: clamp + dirty,
                        // so the crop-restricted histogram/clipping refresh.
                        changed = true;
                    }
                });
                // Straighten: rotate + auto-crop (engine rotate_straighten);
                // the preview shows exactly the export geometry. Fine class:
                // levelling needs 0.1°, not the wide-range integer snap.
                changed |= Self::slider_fine(
                    ui,
                    lang,
                    tr(lang, "Straighten (°)"),
                    &mut self.recipe.straighten_deg,
                    -45.0,
                    45.0,
                    0.0,
                );
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Once in (R): drag corner/edge handles to resize, drag inside to move, drag OUTSIDE the box (or the canvas border while the box is full-frame) to rotate-straighten; arrows nudge the box, Enter commits; preview, export and XMP all match. Straighten auto-crops the black corners.",
                    ))
                    .weak()
                    .small(),
                );
            });

        // Geometry ends here — local adjustments and management below.
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(2.0);
        // --- 局部调整: manual masks — the SAME recipe.masks the AI writes -----
        let n_masks = self.recipe.masks.len();
        let n_masks_s = n_masks.to_string();
        egui::CollapsingHeader::new(section_title(
            &trf(lang, "Local Masks ({n})", &[("n", &n_masks_s)]),
            n_masks > 0,
        ))
        .id_salt("sec_local")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let lin_armed = matches!(self.placing_mask, Some((MaskKind::Linear, PlaceTarget::NewMask)));
                if ui.selectable_label(lin_armed, tr(lang, "＋ Linear gradient")).on_hover_text(tr(lang, "Drag on the image: start = fully-applied side, end = unaffected side (Shift = horizontal/vertical)")).clicked() {
                    self.disarm_tools();
                    if !lin_armed {
                        self.placing_mask = Some((MaskKind::Linear, PlaceTarget::NewMask));
                        self.status = tr(lang, "Drag on the image to draw a linear gradient (start fully applied → end unaffected; Shift = axis lock)").into();
                    }
                }
                let rad_armed = matches!(self.placing_mask, Some((MaskKind::Radial, PlaceTarget::NewMask)));
                if ui.selectable_label(rad_armed, tr(lang, "＋ Radial gradient")).on_hover_text(tr(lang, "Drag on the image to draw an elliptical area")).clicked() {
                    self.disarm_tools();
                    if !rad_armed {
                        self.placing_mask = Some((MaskKind::Radial, PlaceTarget::NewMask));
                        self.status = tr(lang, "Drag on the image to draw a radial (elliptical) area").into();
                    }
                }
                // LR's most-used local tool, finally: free-form brush →
                // Bitmap mask (the raster carrier + bilinear sampler + cache
                // all pre-existed; this wires the paint canvas into
                // recipe.masks — see start_mask_brush / commit_mask_brush).
                let brush_armed = matches!(self.mask_brush, Some((None, _)));
                if ui
                    .selectable_label(brush_armed, tr(lang, "🖌 Brush"))
                    .on_hover_text(tr(lang, "Paint a free-form mask ([ ] = brush size); 「Apply」 bakes it into a new mask"))
                    .clicked()
                {
                    if brush_armed {
                        self.disarm_tools();
                    } else if self.base_preview.is_some() {
                        self.start_mask_brush(None);
                    }
                }
            });
            // Brush-session controls (create OR raster-edit): erase toggle +
            // the bake/cancel pair. Lives up here so it is visible whichever
            // row armed the session.
            if let Some((target, erase)) = self.mask_brush {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(erase, tr(lang, "⌫ Erase"))
                        .on_hover_text(tr(lang, "Strokes remove from the selection instead of adding"))
                        .clicked()
                    {
                        self.mask_brush = Some((target, !erase));
                    }
                    if ui.button(tr(lang, "✓ Apply")).clicked() {
                        self.commit_mask_brush();
                    }
                    if ui.button(tr(lang, "✕ Cancel")).clicked() {
                        self.disarm_tools();
                    }
                });
            }
            // --- AI segmentation → bitmap masks (gap batch A②) ---------------
            ui.horizontal(|ui| {
                let can_seg = !self.busy && self.base_preview.is_some();
                if ui
                    .add_enabled(can_seg, egui::Button::new(tr(lang, "🤖 AI select subject")))
                    .on_hover_text(tr(lang,
                        "U²-Net salient-subject segmentation → bitmap mask (python sidecar: pip install rembg; first run auto-downloads the model to ~/.u2net)",
                    ))
                    .clicked()
                {
                    self.start_segment("subject", "Subject");
                }
                if ui
                    .add_enabled(can_seg, egui::Button::new(tr(lang, "☁ AI select sky")))
                    .on_hover_text(tr(lang,
                        "SegFormer-ADE20K sky segmentation → bitmap mask (python sidecar: pip install transformers; first run auto-downloads a ~14MB model)",
                    ))
                    .clicked()
                {
                    self.start_segment("sky", "Sky");
                }
            });
            // Mask list: click to select (shows overlay + sliders), 🗑 deletes;
            // HOVERING a row previews that mask's coverage without selecting.
            // hover_mask lives ONE frame: update() takes it each frame and this
            // list re-sets it while the cursor is on a row — so leaving the
            // panel, collapsing this section or switching photos all fall back
            // to the selection with no stale index to chase.
            // Reorder is a dedicated ☰ grip per row (the ONLY drag source),
            // NOT the row itself: `dnd_drag_source` overlays its contents with
            // a drag-sensing interact (grab cursor), so wrapping the whole row
            // made the slightest press-move float the row instead of selecting
            // it (user-reported "click moves instead of selects"). Grip drags,
            // row body clicks — the drop TARGET stays the whole row. egui
            // clears the payload on release/Esc itself, and while a drag is in
            // flight `hovered()` is false everywhere, so the hover preview
            // pauses instead of churning the coverage overlay.
            let mut delete: Option<usize> = None;
            let mut toggle_eye: Option<usize> = None;
            let mut dropped: Option<(usize, usize)> = None; // (from, insert-before)
            for i in 0..n_masks {
                let row_resp = ui
                    .horizontal(|ui| {
                        let m = &self.recipe.masks[i];
                        let kind = match m.mask {
                            MaskGeometry::Linear { .. } => tr(self.lang, "Linear"),
                            MaskGeometry::Radial { .. } => tr(self.lang, "Radial"),
                            MaskGeometry::Bitmap { .. } => tr(self.lang, "Bitmap"),
                        };
                        // A user-given name wins; else a reverse-fit zone shows
                        // its localised role label; else the generic placeholder.
                        let base: &str = if !m.name.is_empty() {
                            m.name.as_str()
                        } else if let Some(en) = m.role.en_name() {
                            tr(self.lang, en)
                        } else {
                            tr(self.lang, "mask")
                        };
                        // The activity dot IS the engine's own rule
                        // (render::engine_active): a parked mask looks parked,
                        // a working one shows ● — a 64-row list where the two
                        // were indistinguishable was a real navigation cost.
                        let active = m.enabled && autoshop::render::engine_active(m);
                        let enabled = m.enabled;
                        let label = format!(
                            "{base} · {kind}{}",
                            if active { "  ●" } else { "" }
                        );
                        let label = if enabled {
                            egui::RichText::new(label)
                        } else {
                            egui::RichText::new(label).weak()
                        };
                        ui.dnd_drag_source(ui.id().with(("mask_row", i)), i, |ui| {
                            ui.label("☰");
                        })
                        .response
                        .on_hover_text(tr(self.lang, "Drag to reorder"));
                        // The eye: Lightroom's lossless mute. Amount-to-0 as a
                        // mute destroyed the tuned value; this keeps it.
                        if ui
                            .selectable_label(enabled, "👁")
                            .on_hover_text(tr(
                                self.lang,
                                "Show/mute this mask without losing its settings",
                            ))
                            .clicked()
                        {
                            toggle_eye = Some(i);
                        }
                        let row = ui.selectable_label(self.sel_mask == Some(i), label);
                        if row.hovered() {
                            self.hover_mask = Some(i);
                        }
                        if row.clicked() {
                            self.sel_mask =
                                if self.sel_mask == Some(i) { None } else { Some(i) };
                            // Component selection belongs to ONE mask's list.
                            self.sel_component = None;
                            self.overlay_stale = true; // coverage follows the selection
                            // The colour sampler is INDEX-armed: left live
                            // across a selection change, the next canvas click
                            // wrote a range into the OLD mask with no visible
                            // feedback (its row's 🎯 label was gone).
                            self.range_picking = None;
                        }
                        if ui
                            .small_button("🗑")
                            .on_hover_text(tr(self.lang, "Delete this mask (its stack order shifts the ones below)"))
                            .clicked()
                        {
                            delete = Some(i);
                        }
                    })
                    .response;
                // A grip being dragged over this row: mark the insertion edge
                // (above/below the row midline) and take the drop. The whole
                // row rect is the drop zone, so aiming needs no precision.
                if let (Some(from), Some(p)) =
                    (row_resp.dnd_hover_payload::<usize>(), ui.ctx().pointer_interact_pos())
                {
                    let below = p.y > row_resp.rect.center().y;
                    let y = if below { row_resp.rect.max.y } else { row_resp.rect.min.y };
                    ui.painter().hline(
                        row_resp.rect.x_range(),
                        y,
                        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                    );
                    let insert = if below { i + 1 } else { i };
                    if row_resp.dnd_release_payload::<usize>().is_some() {
                        dropped = Some((*from, insert));
                    }
                }
            }
            if let Some((from, insert)) = dropped
                // Bounds re-checked at APPLY time: the payload index was
                // captured at drag start, and the list can shrink mid-drag
                // (Ctrl+Z, an async AI result) — a stale `from` would panic
                // in Vec::remove.
                && from < self.recipe.masks.len()
                && insert <= self.recipe.masks.len()
                && insert != from
                && insert != from + 1
            {
                let (to, remap) = reorder_move(from, insert);
                let m = self.recipe.masks.remove(from);
                self.recipe.masks.insert(to, m);
                self.remap_mask_indices(|s| Some(remap(s)));
                self.overlay_stale = true;
                changed = true;
            }
            if let Some(i) = toggle_eye {
                // A recipe mutation like any slider: develop + overlay follow,
                // and commit_if_settled makes it one undo step.
                self.recipe.masks[i].enabled = !self.recipe.masks[i].enabled;
                self.overlay_stale = true;
                changed = true;
            }
            if let Some(i) = delete {
                self.recipe.masks.remove(i);
                self.remap_mask_indices(|s| {
                    if s == i {
                        None
                    } else if s > i {
                        Some(s - 1)
                    } else {
                        Some(s)
                    }
                });
                self.overlay_stale = true;
                changed = true;
            }
            // Selected mask: its full slider set.
            if let Some(i) = self.sel_mask.filter(|&i| i < self.recipe.masks.len()) {
                ui.separator();
                ui.horizontal(|ui| {
                    // The name edits a LOCAL buffer and commits one recipe
                    // mutation on focus loss (Enter included) — binding the
                    // TextEdit straight to the recipe pushed one undo step per
                    // keystroke, so "sky gradient" burned 12 history slots and
                    // Ctrl+Z walked the name back letter by letter. When the
                    // selection jumps to another row mid-edit, the pending
                    // rename commits to ITS OWN mask first — but only while
                    // that mask's name still equals the seed-time snapshot, so
                    // index shifts (delete/reorder) can never cross-commit.
                    if self.mask_name_buf.as_ref().is_none_or(|(j, ..)| *j != i) {
                        if let Some((j, orig, buf)) = self.mask_name_buf.take()
                            && let Some(prev) = self.recipe.masks.get_mut(j)
                            && prev.name == orig
                            && buf != orig
                        {
                            prev.name = buf;
                            changed = true;
                        }
                        let cur = self.recipe.masks[i].name.clone();
                        self.mask_name_buf = Some((i, cur.clone(), cur));
                    }
                    let name_resp = {
                        let buf = &mut self.mask_name_buf.as_mut().expect("seeded above").2;
                        ui.add(
                            egui::TextEdit::singleline(buf)
                                .desired_width(110.0)
                                .hint_text(tr(lang, "Name")),
                        )
                    };
                    if name_resp.lost_focus()
                        && let Some((_, orig, buf)) = self.mask_name_buf.as_mut()
                        && *buf != self.recipe.masks[i].name
                    {
                        self.recipe.masks[i].name = buf.clone();
                        // Re-baseline the seed: a SECOND rename in this row
                        // must still cross-commit correctly on a row switch.
                        *orig = buf.clone();
                        changed = true;
                    }
                    let m = &mut self.recipe.masks[i];
                    // Raster masks have no drag-to-place geometry — no 重画.
                    let kind = match m.mask {
                        MaskGeometry::Linear { .. } => Some(MaskKind::Linear),
                        MaskGeometry::Radial { .. } => Some(MaskKind::Radial),
                        MaskGeometry::Bitmap { .. } => None,
                    };
                    if let Some(kind) = kind {
                        let redraw_armed = matches!(
                            self.placing_mask,
                            Some((k, PlaceTarget::Redraw(j))) if k == kind && j == i
                        );
                        if ui
                            .selectable_label(redraw_armed, tr(lang, "↻ Redraw"))
                            .on_hover_text(tr(lang, "Re-drag this mask's area on the image"))
                            .clicked()
                        {
                            self.disarm_tools();
                            if !redraw_armed {
                                self.placing_mask = Some((kind, PlaceTarget::Redraw(i)));
                                self.status =
                                    tr(lang, "Re-drag this mask's area on the image").into();
                            }
                        }
                    }
                    if ui
                        .checkbox(&mut self.show_mask_overlay, tr(lang, "Overlay"))
                        .on_hover_text(tr(lang, "Show this mask's actual coverage as a red semi-transparent overlay (geometry × range × strength, shortcut O)"))
                        .changed()
                    {
                        self.overlay_stale = true;
                    }
                    // Mask ORDER is render semantics (masks stack sequentially;
                    // a later mask's range sees earlier masks' output) — so the
                    // list order is editable, not just cosmetic.
                    if ui
                        .add_enabled(i > 0, egui::Button::new("⬆").small())
                        .on_hover_text(tr(lang, "Move up (renders earlier)"))
                        .clicked()
                    {
                        self.recipe.masks.swap(i, i - 1);
                        self.remap_mask_indices(|s| {
                            Some(if s == i { i - 1 } else if s == i - 1 { i } else { s })
                        });
                        self.overlay_stale = true;
                        changed = true;
                    }
                    if ui
                        .add_enabled(i + 1 < self.recipe.masks.len(), egui::Button::new("⬇").small())
                        .on_hover_text(tr(lang, "Move down (renders later)"))
                        .clicked()
                    {
                        self.recipe.masks.swap(i, i + 1);
                        self.remap_mask_indices(|s| {
                            Some(if s == i { i + 1 } else if s == i + 1 { i } else { s })
                        });
                        self.overlay_stale = true;
                        changed = true;
                    }
                    // Inversion flips the mask's coverage — its Response.changed()
                    // must drive the develop + overlay like every other mask
                    // control (was silently discarded: the toggle mutated the
                    // recipe but never re-rendered until an unrelated edit).
                    if ui.checkbox(&mut self.recipe.masks[i].inverted, tr(lang, "Invert")).changed() {
                        self.overlay_stale = true;
                        changed = true;
                    }
                    // Duplicate: a second gradient with the same tuned ten
                    // sliders used to mean re-dragging and re-typing every
                    // value. Rasters are DETACHED copies (the version-load
                    // rule), so the twins never share a mutable file.
                    if ui
                        .small_button("⧉")
                        .on_hover_text(tr(lang, "Duplicate this mask (bitmap rasters are copied, so the copies stay independent)"))
                        .clicked()
                    {
                        if self.recipe.masks.len() >= 64 {
                            let t = tr(lang, "mask limit reached (64) — delete one first");
                            self.toast(ToastKind::Error, t.to_string());
                        } else {
                            let mut clone = self.recipe.masks[i].clone();
                            if let Some(p) = self.src_path.as_deref() {
                                let mut tmp = EditRecipe { masks: vec![clone], ..Default::default() };
                                autoshop::store::detach_rasters(p, &mut tmp, "mask-dup");
                                clone = tmp.masks.remove(0);
                            }
                            if !clone.name.is_empty() {
                                clone.name = format!("{} 2", clone.name);
                            }
                            self.recipe.masks.insert(i + 1, clone);
                            self.remap_mask_indices(|s| Some(if s > i { s + 1 } else { s }));
                            self.sel_mask = Some(i + 1);
                            self.sel_component = None;
                            self.overlay_stale = true;
                            changed = true;
                        }
                    }
                });
                // Radial geometry: edge feather (shown 0..100, LR's track) +
                // rotation + inside/outside flip — all engine-rendered.
                if let MaskGeometry::Radial { feather, flipped, angle, .. } =
                    &mut self.recipe.masks[i].mask
                {
                    let mut geo_ch = false;
                    ui.horizontal(|ui| {
                        geo_ch |= Self::slider_pct(
                            ui,
                            lang,
                            tr(lang, "Edge feather"),
                            feather,
                            1.0,
                            0.5,
                        );
                        geo_ch |= ui
                            .checkbox(flipped, tr(lang, "Flip"))
                            .on_hover_text(tr(
                                lang,
                                "Swap which side of the ellipse the adjustment affects (composes with Invert)",
                            ))
                            .changed();
                    });
                    // Rotation: the on-image grip (the knob parked above the
                    // ellipse) drags it too; the slider gives numeric entry.
                    geo_ch |= Self::slider(
                        ui,
                        lang,
                        tr(lang, "Angle"),
                        angle,
                        -180.0,
                        180.0,
                        0.0,
                    );
                    if geo_ch {
                        self.overlay_stale = true;
                        changed = true;
                    }
                }
                // Bitmap (AI / brush) raster tools: before these, a raster
                // mask was completely un-editable after creation — a clipped
                // treeline's only recourse was re-running the model to get
                // the same raster. Every op BAKES a freshly claimed raster
                // and repoints (never mutates the input file).
                if matches!(self.recipe.masks[i].mask, MaskGeometry::Bitmap { .. }) {
                    ui.horizontal_wrapped(|ui| {
                        let edit_armed = matches!(self.mask_brush, Some((Some(j), _)) if j == i);
                        if ui
                            .selectable_label(edit_armed, tr(lang, "🖌 Edit raster"))
                            .on_hover_text(tr(lang, "Brush-edit this mask: paint adds, 「Erase」 removes, 「Apply」 bakes"))
                            .clicked()
                        {
                            if edit_armed {
                                self.disarm_tools();
                            } else {
                                self.start_mask_brush(Some(i));
                            }
                        }
                        if ui
                            .button(tr(lang, "◌ Feather"))
                            .on_hover_text(tr(lang, "Soften the mask boundary one step (bakes a new raster; repeat for more)"))
                            .clicked()
                        {
                            self.bake_mask_raster(
                                i,
                                |g| {
                                    let s = (g.width().max(g.height()) as f32 / 400.0).max(1.0);
                                    autoshop::render::feather_mask(g, s)
                                },
                                "mask-edit",
                            );
                        }
                        if ui
                            .button(tr(lang, "⊕ Expand"))
                            .on_hover_text(tr(lang, "Grow the selection one step (bakes a new raster)"))
                            .clicked()
                        {
                            self.bake_mask_raster(
                                i,
                                |g| {
                                    let r = (g.width().max(g.height()) / 500).max(2) as i32;
                                    autoshop::render::morph_mask(g, r)
                                },
                                "mask-edit",
                            );
                        }
                        if ui
                            .button(tr(lang, "⊖ Contract"))
                            .on_hover_text(tr(lang, "Shrink the selection one step (bakes a new raster)"))
                            .clicked()
                        {
                            self.bake_mask_raster(
                                i,
                                |g| {
                                    let r = (g.width().max(g.height()) / 500).max(2) as i32;
                                    autoshop::render::morph_mask(g, -r)
                                },
                                "mask-edit",
                            );
                        }
                        if ui
                            .add_enabled(!self.busy, egui::Button::new(tr(lang, "⇱ Full-res refine")))
                            .on_hover_text(tr(lang, "Re-cut this mask against the FULL-resolution source (guided filter). Preview-res AI masks smear their boundary at export — this snaps it to real edges. Decodes the full-size source; takes a few seconds."))
                            .clicked()
                        {
                            self.start_mask_refine(i);
                        }
                    });
                }
                // --- Shapes（组件）: compose extra geometry onto this mask —
                // Lightroom's Add / Subtract / Intersect grammar. ENGINE-ONLY
                // (recipe.rs MaskComponent): the XMP projection carries the
                // base shape alone.
                {
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Shapes"));
                        let mode_label = |m: autoshop::recipe::MaskCombine| match m {
                            autoshop::recipe::MaskCombine::Add => tr(lang, "＋ Add"),
                            autoshop::recipe::MaskCombine::Subtract => tr(lang, "－ Subtract"),
                            autoshop::recipe::MaskCombine::Intersect => tr(lang, "∩ Intersect"),
                        };
                        egui::ComboBox::from_id_salt("comp_mode")
                            .selected_text(mode_label(self.component_mode))
                            .width(96.0)
                            .show_ui(ui, |ui| {
                                for m in [
                                    autoshop::recipe::MaskCombine::Add,
                                    autoshop::recipe::MaskCombine::Subtract,
                                    autoshop::recipe::MaskCombine::Intersect,
                                ] {
                                    ui.selectable_value(&mut self.component_mode, m, mode_label(m));
                                }
                            });
                        let mode = self.component_mode;
                        let lin_armed = matches!(
                            self.placing_mask,
                            Some((MaskKind::Linear, PlaceTarget::Component(j, _))) if j == i
                        );
                        if ui
                            .selectable_label(lin_armed, tr(lang, "▭ Linear"))
                            .on_hover_text(tr(lang, "Drag on the image to add a linear shape to THIS mask"))
                            .clicked()
                        {
                            self.disarm_tools();
                            if !lin_armed {
                                self.placing_mask =
                                    Some((MaskKind::Linear, PlaceTarget::Component(i, mode)));
                                self.status = tr(lang, "Drag on the image: the new shape composes onto this mask").into();
                            }
                        }
                        let rad_armed = matches!(
                            self.placing_mask,
                            Some((MaskKind::Radial, PlaceTarget::Component(j, _))) if j == i
                        );
                        if ui
                            .selectable_label(rad_armed, tr(lang, "◯ Radial"))
                            .on_hover_text(tr(lang, "Drag on the image to add an elliptical shape to THIS mask"))
                            .clicked()
                        {
                            self.disarm_tools();
                            if !rad_armed {
                                self.placing_mask =
                                    Some((MaskKind::Radial, PlaceTarget::Component(i, mode)));
                                self.status = tr(lang, "Drag on the image: the new shape composes onto this mask").into();
                            }
                        }
                    });
                    let n_comp = self.recipe.masks[i].components.len();
                    let mut del_comp: Option<usize> = None;
                    for c in 0..n_comp {
                        ui.horizontal(|ui| {
                            let comp = &self.recipe.masks[i].components[c];
                            let icon = match comp.mode {
                                autoshop::recipe::MaskCombine::Add => "＋",
                                autoshop::recipe::MaskCombine::Subtract => "－",
                                autoshop::recipe::MaskCombine::Intersect => "∩",
                            };
                            let kind = match comp.geometry {
                                MaskGeometry::Linear { .. } => tr(lang, "Linear"),
                                MaskGeometry::Radial { .. } => tr(lang, "Radial"),
                                MaskGeometry::Bitmap { .. } => tr(lang, "Bitmap"),
                            };
                            let selected = self.sel_component == Some(c);
                            if ui
                                .selectable_label(selected, format!("{icon} {kind}"))
                                .on_hover_text(tr(lang, "Select to drag this shape's knobs on the image (the base mask's knobs come back when deselected)"))
                                .clicked()
                            {
                                self.sel_component = if selected { None } else { Some(c) };
                            }
                            let mut mode = comp.mode;
                            egui::ComboBox::from_id_salt(("comp_row_mode", c))
                                .selected_text(match mode {
                                    autoshop::recipe::MaskCombine::Add => tr(lang, "Add"),
                                    autoshop::recipe::MaskCombine::Subtract => tr(lang, "Subtract"),
                                    autoshop::recipe::MaskCombine::Intersect => tr(lang, "Intersect"),
                                })
                                .width(88.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut mode, autoshop::recipe::MaskCombine::Add, tr(lang, "Add"));
                                    ui.selectable_value(&mut mode, autoshop::recipe::MaskCombine::Subtract, tr(lang, "Subtract"));
                                    ui.selectable_value(&mut mode, autoshop::recipe::MaskCombine::Intersect, tr(lang, "Intersect"));
                                });
                            if mode != comp.mode {
                                self.recipe.masks[i].components[c].mode = mode;
                                self.overlay_stale = true;
                                changed = true;
                            }
                            if ui.small_button("🗑").clicked() {
                                del_comp = Some(c);
                            }
                        });
                    }
                    if let Some(c) = del_comp {
                        self.recipe.masks[i].components.remove(c);
                        self.sel_component = match self.sel_component {
                            Some(s) if s == c => None,
                            Some(s) if s > c => Some(s - 1),
                            other => other,
                        };
                        self.overlay_stale = true;
                        changed = true;
                    }
                    if n_comp > 0 {
                        ui.label(
                            egui::RichText::new(tr(lang, "Shapes compose in order onto the base mask. In-app render + export only — the Lightroom XMP carries the base shape alone."))
                                .weak()
                                .small(),
                        );
                    }
                }
                // --- Range Mask（LR 范围蒙版）: refines WHERE the geometry applies —
                // final weight = geometry × range, live in preview + export + XMP.
                // Coverage invalidation happens ONCE below (after Amount): any
                // edit up to there can move the wash, and the overlay key
                // compare dedupes rebuilds — the old "range-section-only"
                // tracker went blind whenever an earlier control had already
                // set `changed` in the same frame.
                {
                    let cur = match &self.recipe.masks[i].range {
                        None => 0usize,
                        Some(RangeMask::Luminance { .. }) => 1,
                        Some(RangeMask::Color { .. }) => 2,
                    };
                    let mut sel = cur;
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Range mask"));
                        egui::ComboBox::from_id_salt("range_kind")
                            .selected_text([tr(lang, "None"), tr(lang, "Luminance"), tr(lang, "Color")][sel])
                            .width(70.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut sel, 0, tr(lang, "None"));
                                ui.selectable_value(&mut sel, 1, tr(lang, "Luminance"));
                                ui.selectable_value(&mut sel, 2, tr(lang, "Color"));
                            });
                    });
                    if sel != cur {
                        // Leaving Color: the armed sampler goes too, or the
                        // canvas keeps advertising (and dispatching) a colour
                        // pick for a range that no longer exists.
                        if cur == 2 && self.range_picking == Some(i) {
                            self.range_picking = None;
                        }
                        self.recipe.masks[i].range = match sel {
                            // Full range = neutral start; narrow from there.
                            1 => Some(RangeMask::Luminance { lo_outer: 0.0, lo: 0.0, hi: 1.0, hi_outer: 1.0 }),
                            2 => Some(RangeMask::Color { r: 0.5, g: 0.5, b: 0.5, amount: 0.5, px: 0.5, py: 0.5 }),
                            _ => None,
                        };
                        if sel == 2 {
                            // Jump straight into sampling — a colour range without
                            // a picked colour selects nothing useful.
                            self.disarm_tools();
                            self.range_picking = Some(i);
                            self.status = tr(lang, "Color range: click the color to pick in the image").into();
                        }
                        changed = true;
                    }
                    let picking_this = self.range_picking == Some(i);
                    let mut want_pick = false;
                    match &mut self.recipe.masks[i].range {
                        Some(RangeMask::Luminance { lo_outer, lo, hi, hi_outer }) => {
                            // GUI shows lo/hi + one symmetric feather; the recipe keeps
                            // ACR's 4-number trapezoid (asymmetric AI trapezoids show
                            // their averaged feather until a slider is touched).
                            // All three on LR's 0..100 display track (storage
                            // stays the 0..1 fraction — see slider_pct).
                            let mut f = ((*lo - *lo_outer) + (*hi_outer - *hi)) * 0.5;
                            let ch_lo = Self::slider_pct(ui, lang, tr(lang, "Lum. low"), lo, 1.0, 0.0);
                            let ch_hi = Self::slider_pct(ui, lang, tr(lang, "Lum. high"), hi, 1.0, 1.0);
                            let ch_f = Self::slider_pct(ui, lang, tr(lang, "Feather"), &mut f, 0.5, 0.1);
                            if ch_lo || ch_hi || ch_f {
                                // CLAMP the edited endpoint at the other, never
                                // swap: the active slider stays bound to its
                                // field across drag frames, so swapping made
                                // both bounds follow the pointer and converge.
                                if *lo > *hi {
                                    if ch_lo { *lo = *hi } else { *hi = *lo }
                                }
                                *lo_outer = (*lo - f).max(0.0);
                                *hi_outer = (*hi + f).min(1.0);
                                changed = true;
                            }
                        }
                        Some(RangeMask::Color { r, g, b, amount, .. }) => {
                            ui.horizontal(|ui| {
                                let mut c = [*r, *g, *b];
                                if ui.color_edit_button_rgb(&mut c).changed() {
                                    [*r, *g, *b] = [c[0], c[1], c[2]];
                                    changed = true;
                                }
                                let label = if picking_this { tr(lang, "🎯 Click in image…") } else { tr(lang, "🎯 Sample") };
                                if ui.small_button(label).on_hover_text(tr(lang, "Click the color to pick in the image (the same color at other brightnesses is also selected; clicking this button again cancels sampling)")).clicked() {
                                    want_pick = true;
                                }
                            });
                            changed |= Self::slider_pct(ui, lang, tr(lang, "Tolerance"), amount, 1.0, 0.5);
                        }
                        None => {}
                    }
                    if want_pick {
                        self.disarm_tools();
                        if !picking_this {
                            self.range_picking = Some(i);
                            self.status = tr(lang, "Color range: click the color to pick in the image").into();
                        }
                    }
                }
                let m = &mut self.recipe.masks[i];
                // LR's 0..100 display track (storage stays 0..1 — slider_pct).
                changed |= Self::slider_pct(ui, lang, tr(lang, "Amount"), &mut m.amount, 1.0, 1.0);
                // Any edit up to here — geometry, range, Amount — can move the
                // coverage wash (the overlay key carries them all); the key
                // compare inside refresh_mask_overlay dedupes rebuilds, so
                // over-flagging is free. The tone sliders below are not
                // coverage and deliberately stay out.
                if changed {
                    self.overlay_stale = true;
                }
                changed |= Self::slider(ui, lang, tr(lang, "Exposure"), &mut m.exposure_ev, -5.0, 5.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Contrast"), &mut m.contrast, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Highlights"), &mut m.highlights, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Shadows"), &mut m.shadows, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Whites"), &mut m.whites, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Blacks"), &mut m.blacks, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Saturation"), &mut m.saturation, -100.0, 100.0, 0.0);
                // Engine-rendered since batch #2-B (render.rs apply_masks
                // mirrors the global WB model inside the mask) — live in the
                // preview like the tone sliders above.
                changed |= Self::slider(ui, lang, tr(lang, "Temp shift"), &mut m.temperature, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Tint shift"), &mut m.tint, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Noise Reduction"), &mut m.noise_reduction, 0.0, 100.0, 0.0);
                // These serialise to the XMP but the in-app preview doesn't
                // render them yet (documented engine scope) — honest label.
                egui::CollapsingHeader::new(tr(lang, "More (XMP/Lightroom only)"))
                    .id_salt("sec_local_xmp")
                    .default_open(false)
                    .show(ui, |ui| {
                        let m = &mut self.recipe.masks[i];
                        changed |= Self::slider(ui, lang, tr(lang, "Clarity"), &mut m.clarity, -100.0, 100.0, 0.0);
                        changed |= Self::slider(ui, lang, tr(lang, "Dehaze"), &mut m.dehaze, -100.0, 100.0, 0.0);
                        changed |= Self::slider(ui, lang, tr(lang, "Texture"), &mut m.texture, -100.0, 100.0, 0.0);
                    });
            } else if n_masks == 0 {
                ui.label(
                    egui::RichText::new(tr(lang, "Lightroom-style local adjustments: add a gradient to darken the sky, a radial to brighten the subject. AI Analyze also writes to this list."))
                        .weak()
                        .small(),
                );
            }
        });

        // --- 版本: recipe snapshots ≈ LR virtual copies (gap batch G) --------
        ui.add_space(6.0);
        let n_ver = self.versions.len();
        let n_ver_s = n_ver.to_string();
        egui::CollapsingHeader::new(section_title(&trf(lang, "Versions ({n})", &[("n", &n_ver_s)]), n_ver > 0))
            .id_salt("sec_versions")
            .default_open(false)
            .show(ui, |ui| {
                // Clearing the list on open (see `open_path`) already stops a
                // STALE row from acting on the incoming photo. This is the
                // second half: while a photo is still decoding, "Save as
                // version" would snapshot the OUTGOING photo's canvas — the
                // fresh recipe has not landed yet — into the incoming photo's
                // develop. Nothing here is meaningful until the open settles,
                // so the whole section goes inert rather than each button
                // carrying its own guard.
                ui.add_enabled_ui(!self.busy, |ui| {
                if ui
                    .button(tr(lang, "＋ Save as version"))
                    .on_hover_text(tr(lang, "Save all current develop parameters as a numbered snapshot (v<N>.recipe.json in this photo's develop store), reloadable anytime"))
                    .clicked()
                {
                    self.save_version();
                }
                let mut load: Option<u32> = None;
                let mut delete: Option<u32> = None;
                for &n in &self.versions {
                    ui.horizontal(|ui| {
                        ui.label(format!("v{n}"));
                        if ui.small_button(tr(lang, "Load")).on_hover_text(tr(lang, "Replace current parameters (one Ctrl+Z to undo)")).clicked() {
                            load = Some(n);
                        }
                        if ui
                            .small_button("🗑")
                            .on_hover_text(tr(lang, "Delete this snapshot (its frozen mask rasters go with it)"))
                            .clicked()
                        {
                            delete = Some(n);
                        }
                    });
                }
                if let Some(n) = load {
                    self.load_version(n);
                }
                if let Some(n) = delete
                    && let Some(src) = self.src_path.clone()
                {
                    // NoWait wrapper: delete_version locks internally with
                    // Wait — a foreground 🗑 click must fail into the error
                    // arm when another process holds the develop, not hang.
                    match autoshop::store::with_develop_lock(
                        &src,
                        autoshop::store::DevelopLockMode::NoWait,
                        || autoshop::store::delete_version(&src, n),
                    ) {
                        Ok(()) => {
                            self.refresh_versions();
                            self.status =
                                trf(lang, "Version v{n} deleted", &[("n", &n.to_string())]);
                        }
                        Err(e) => {
                            self.persist_postponed(
                                &e,
                                "Delete v{n} failed: {err}",
                                &[("n", &n.to_string())],
                            );
                        }
                    }
                }
                if n_ver == 0 {
                    ui.label(
                        egui::RichText::new(tr(lang, "Like LR virtual copies: store multiple parameter sets for one photo (B&W, cropped…) without overwriting."))
                            .weak()
                            .small(),
                    );
                }
                }); // add_enabled_ui: inert while a photo is still opening
            });

        // --- 导出设置 (UX batch): moved out of the toolbar — touched once per
        // delivery, these are Export-dialog contents, not toolbar chrome. The
        // toolbar keeps the ACTIONS; their hover echoes this section's state.
        ui.add_space(6.0);
        let export_active = self.save_jpeg
            || self.exp_long_edge != 0
            || self.exp_sharpen != 0.0
            || self.exp_space != 0
            || self.save_denoise;
        egui::CollapsingHeader::new(section_title(tr(lang, "Export"), export_active))
            .id_salt("sec_export")
            .default_open(false)
            .show(ui, |ui| {
                // One setting per row: two label+combo pairs in a non-wrapping
                // horizontal overflowed the 320px panel in Chinese.
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Format"));
                    egui::ComboBox::from_id_salt("save_fmt")
                        .selected_text(if self.save_jpeg { "JPEG" } else { tr(lang, "16-bit TIFF") })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.save_jpeg, false, tr(lang, "16-bit TIFF"));
                            ui.selectable_value(&mut self.save_jpeg, true, "JPEG");
                        });
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Long edge"));
                    egui::ComboBox::from_id_salt("exp_long_edge")
                        .selected_text(if self.exp_long_edge == 0 {
                            tr(lang, "Original size").to_string()
                        } else {
                            format!("{} px", self.exp_long_edge)
                        })
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.exp_long_edge, 0, tr(lang, "Original size"));
                            for px in [1600u32, 2048, 2560, 3840, 5120] {
                                ui.selectable_value(&mut self.exp_long_edge, px, format!("{px} px"));
                            }
                        });
                });
                Self::slider(ui, lang, tr(lang, "Output sharpening"), &mut self.exp_sharpen, 0.0, 100.0, 0.0);
                // Always allocated, merely disabled for TIFF — the old
                // appear/disappear reflowed every control to its right.
                ui.add_enabled_ui(self.save_jpeg, |ui| {
                    Self::slider(ui, lang, tr(lang, "JPEG quality"), &mut self.exp_quality, 60.0, 100.0, 95.0);
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Colour space"));
                    egui::ComboBox::from_id_salt("exp_space")
                        .selected_text(tr(lang, EXPORT_SPACES[(self.exp_space as usize).min(2)]))
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            for (i, name) in EXPORT_SPACES.iter().enumerate() {
                                ui.selectable_value(&mut self.exp_space, i as u8, tr(lang, name));
                            }
                        });
                });
                ui.checkbox(&mut self.save_denoise, tr(lang, "AI Denoise")).on_hover_text(
                    tr(lang, "SCUNet AI denoise before developing — high-ISO / astro (slow, GPU; needs the python sidecar). Batch render skips it."),
                );
                ui.label(
                    egui::RichText::new(tr(lang, "Applied by Export / Download… in the toolbar (Ctrl+E). Files land in ./out unless Download picks a path."))
                        .weak()
                        .small(),
                );
            });

        if changed {
            self.recipe.clamp();
            self.dirty = true;
        }
    }

    /// The visible full-frame uv window: committed crop (shown cropped, like
    /// Lightroom — except while the crop tool is open, which needs the full
    /// frame) narrowed by zoom/pan. `pan` is stored in crop-window coords and
    /// re-clamped here so edge panning never accumulates out of range.
    fn view_uv(&mut self) -> egui::Rect {
        let win = match (&self.recipe.crop, self.crop_mode) {
            (Some(c), false) => egui::Rect::from_min_max(
                egui::pos2(c.left.min(c.right), c.top.min(c.bottom)),
                egui::pos2(c.right.max(c.left), c.bottom.max(c.top)),
            ),
            _ => egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        };
        let half = 0.5 / self.zoom.clamp(1.0, 12.0);
        self.pan = egui::vec2(
            self.pan.x.clamp(half, 1.0 - half),
            self.pan.y.clamp(half, 1.0 - half),
        );
        egui::Rect::from_min_max(
            egui::pos2(
                win.min.x + (self.pan.x - half) * win.width(),
                win.min.y + (self.pan.y - half) * win.height(),
            ),
            egui::pos2(
                win.min.x + (self.pan.x + half) * win.width(),
                win.min.y + (self.pan.y + half) * win.height(),
            ),
        )
    }

    /// The After image with its interaction layers — crop tool, mask placement,
    /// paint canvas, box-select — or the SOURCE flashed in the same rect while
    /// `comparing` (B held). Scroll zooms to the cursor; middle-drag or
    /// Space+drag pans; all image-space handlers map through [`ViewXform`].
    fn after_view(&mut self, ui: &mut egui::Ui, max_w: f32, avail_y: f32, comparing: bool) {
        let lang = self.lang;
        let tex = if comparing { self.before_tex.as_ref() } else { self.after_tex.as_ref() };
        let Some((id, tex_size)) = tex.map(|t| (t.id(), t.size_vec2())) else {
            ui.label(egui::RichText::new("…").weak());
            return;
        };

        let uv = self.view_uv();
        // Display size fits the VISIBLE window's aspect (in image pixels).
        let vis_px = egui::vec2(uv.width() * tex_size.x, uv.height() * tex_size.y);
        let disp = fit_in(vis_px, max_w, avail_y);
        // PHYSICAL pixels per image pixel: disp is measured in egui logical
        // points — without pixels_per_point a "100%" readout on a 2× display
        // spanned two physical pixels per texel, and "1:1" wasn't.
        let ppp = ui.ctx().pixels_per_point();
        let scale = disp.x * ppp / vis_px.x.max(1.0); // display (physical) px per image px

        // Caption row: mode hint left, zoom readout + Fit / 1:1 right. Every
        // armed-tool hint names its exit (Esc), and the placement hint speaks
        // the armed KIND's language instead of calling a radial a gradient.
        let hint = if comparing {
            if self.before_latch {
                // The latch came from \ — "release B" was a lie there (M21).
                tr(lang, "Before (source) — press \\ again (or Esc) to return to editing")
            } else {
                tr(lang, "Before (source) — release B to return to editing")
            }
        } else if self.crop_mode {
            tr(lang, "Crop — drag corners/edges to resize, inside to move, outside to rotate · Esc to exit")
        } else if let Some((kind, _)) = self.placing_mask {
            match kind {
                MaskKind::Linear => tr(lang, "Linear gradient — drag from the fully-applied side to the unaffected side (Shift = axis lock) · Esc to exit"),
                MaskKind::Radial => tr(lang, "Radial gradient — drag to draw an elliptical area · Esc to exit"),
            }
        } else if self.wb_picking {
            tr(lang, "WB eyedropper — click a spot that should be neutral grey/white · Esc to exit")
        } else if self.range_picking.is_some() {
            tr(lang, "Color range — click the color to pick in the image · Esc to exit")
        } else if self.clone_mode {
            tr(lang, "Stamp — Alt+click to set the source · drag to brush the area to cover · Esc to exit")
        } else if self.paint_mode {
            if self.mask_brush.is_some() {
                tr(lang, "Mask brush — paint to select · 「Erase」 removes · 「Apply」 bakes · Esc cancels")
            } else {
                tr(lang, "Brush — paint over the area to fill / heal · Esc to exit")
            }
        } else {
            tr(lang, "After — drag a box = local AI · scroll to zoom · space/middle-drag to pan · hold B to compare")
        };
        // An armed tool must be visible at NORMAL contrast — .weak().small()
        // is the passive-help style, and it was the only always-on indicator
        // when the arming control sat in a collapsed section.
        let armed = !comparing && self.tool_armed();
        ui.horizontal(|ui| {
            if armed {
                ui.label(egui::RichText::new(hint).color(self.theme.colors().armed_hint).small());
            } else {
                ui.label(egui::RichText::new(hint).weak().small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("1:1").on_hover_text(tr(lang, "Preview pixels 1:1 (double-click the image to toggle)")).clicked() {
                    // Same ceiling as the render path (view_uv clamps at 12) —
                    // an unclamped value desynced zoom/pan math from the view.
                    // ppp: 1:1 means one texel per PHYSICAL pixel (see `scale`).
                    self.zoom = (vis_px.x * self.zoom / (disp.x * ppp)).clamp(1.0, 12.0);
                }
                // "Fit" is natural language, unlike its "1:1" sibling — it
                // must route through `tr` like every user-facing literal
                // (the i18n module contract; the audit now flags bypasses).
                if ui.small_button(tr(lang, "Fit")).on_hover_text(tr(lang, "Fit the whole image to the canvas (double-click the image to toggle)")).clicked() {
                    self.zoom = 1.0;
                    self.pan = egui::vec2(0.5, 0.5);
                }
                if ui
                    .selectable_label(self.show_clipping, "▲")
                    .on_hover_text(tr(lang, "Clipping warning (J): red = highlight clip, blue = shadow crush (judged on export pixels)"))
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.toggle_clipping(&ctx); // instant — no redevelop
                }
                ui.label(egui::RichText::new(format!("{:.0}%", scale * 100.0)).weak().small());
                // --- preview resolution (gap batch E): 1:1 that actually
                // resolves detail. Switching re-decodes the current photo with
                // the recipe KEPT (the keep_recipe path from batch B).
                let before = self.preview_edge;
                ui.add_enabled_ui(!self.busy, |ui| {
                    egui::ComboBox::from_id_salt("preview_edge")
                        .selected_text(format!("{}px", self.preview_edge))
                        .width(64.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.preview_edge, 1280, tr(lang, "1280px · fluid"));
                            ui.selectable_value(&mut self.preview_edge, 2560, "2560px");
                            ui.selectable_value(&mut self.preview_edge, 4096, tr(lang, "4096px · inspect"));
                        })
                        .response
                        .on_hover_text(tr(lang, "Working preview resolution: 1280 is smoothest on the sliders; 2560/4096 for 1:1 focus/noise checks (slower on every adjustment)"));
                });
                if self.preview_edge != before
                    && let Some(p) = self.src_path.clone()
                {
                    if self.busy {
                        // An already-open popup outlives the disabled combo
                        // (its own egui Area), so this is reachable while
                        // busy — and the combo has ALREADY mutated
                        // preview_edge. Keeping the new value displayed a
                        // resolution the working preview does not have,
                        // unreachable to boot (the != trigger above is a
                        // one-shot), persisted it into Prefs, and fed it to
                        // every bake site. The switch reverts with the
                        // refusal, and the status says so instead of
                        // dropping it silently.
                        self.preview_edge = before;
                        // BY TOAST (the switch_variant / Ctrl+S refusal
                        // rule): this runs during the draw phase, and a
                        // status line dies under the very next worker
                        // message — one frame, then the combo just snaps
                        // back with no visible reason.
                        let t = tr(
                            lang,
                            "busy — the preview-resolution switch was not applied; pick it again when the current task finishes",
                        )
                        .to_string();
                        self.toast(ToastKind::Error, t);
                    } else {
                        // The pre-switch edge rides the flight: a FAILED
                        // keep-flight leaves the canvas at the OLD edge, and
                        // preview_edge must not keep displaying (and feeding
                        // every bake site) a resolution the preview never
                        // reached — the same residue the busy refusal above
                        // is cured of.
                        self.edge_before_flight = Some(before);
                        self.keep_recipe = true; // re-decode, keep the edit
                        self.open_path(p);
                    }
                }
            });
        });

        let (rect, resp) = ui.allocate_exact_size(disp, egui::Sense::click_and_drag());
        ui.painter_at(rect).image(id, rect, uv, egui::Color32::WHITE);
        // Diagnostic layers, uv-synced with the image (hidden while comparing):
        // clipping warnings, then the selected mask's coverage on top.
        if !comparing {
            if let Some(t) = &self.clip_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
            if let Some(t) = &self.mask_overlay_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
        }
        let xf = ViewXform { rect, uv };

        // --- zoom to cursor (scroll) -----------------------------------------
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.1
                && let Some(p) = resp.hover_pos()
            {
                let half = 0.5 / self.zoom;
                let (fx, fy) = (
                    ((p.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0),
                    ((p.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0),
                );
                // Cursor's point in crop-window coords, kept stationary across the zoom.
                let q = egui::vec2(
                    self.pan.x - half + fx * 2.0 * half,
                    self.pan.y - half + fy * 2.0 * half,
                );
                self.zoom = (self.zoom * (scroll * 0.003).exp()).clamp(1.0, 12.0);
                let nh = 0.5 / self.zoom;
                self.pan = q - egui::vec2((fx - 0.5) * 2.0 * nh, (fy - 0.5) * 2.0 * nh);
            }
        }
        let tool_active = self.tool_armed();
        // Double-click toggles fit ↔ 1:1 (preview pixels) — but never while a
        // canvas tool is armed: a quick second tap inside brush/crop/pick used
        // to teleport the view instead of reaching the tool.
        if resp.double_clicked() && !tool_active {
            if self.zoom > 1.01 {
                self.zoom = 1.0;
                self.pan = egui::vec2(0.5, 0.5);
            } else {
                // Same physical-pixel 1:1 target as the button above.
                self.zoom = (vis_px.x / (disp.x * ppp)).clamp(1.0, 12.0);
            }
            // The pair's FIRST release cleared the AI region through the
            // plain-click path — a double-click means "toggle zoom", so put
            // it back. Bounded window: a much later double-click elsewhere
            // must not resurrect a long-cleared region (M18).
            if let Some((r, t)) = self.region_restore.take()
                && t.elapsed() < Duration::from_millis(600)
            {
                self.region = Some(r);
            }
        }

        // --- pan: middle-drag, Space + left-drag, or (the LR gesture) a plain
        // left-drag while ZOOMED IN — a zoomed-in drag means "pan" in every
        // photo editor, and requiring Space for it was a top "feels off".
        // Box-select stays reachable while zoomed via Ctrl+drag; an active
        // tool, a mask-knob hover/drag or a box drag in flight all take
        // priority over the implicit pan.
        // Focus-gated like B-compare: Space in a text field is a space.
        let space = ui.ctx().memory(|m| m.focused()).is_none()
            && ui.input(|i| i.key_down(egui::Key::Space));
        let ctrl = ui.input(|i| i.modifiers.command);
        let over_knob = self.mask_drag.is_some()
            || self.sel_target_geometry().is_some_and(|g| {
                let (dims, deg, dist) = self.geom_ctx();
                // Probe BOTH positions: egui reports drag_started only
                // after ~6 px of travel, so a fast flick's CURRENT pointer
                // has already left the knob while its press origin is still
                // on it — `or_else` (origin only when hover was None) still
                // let pan steal an in-canvas flick. The knobs probed are the
                // ones actually shown: the selected COMPONENT's when one is
                // selected (sel_target_geometry), matching handle_mask_edit.
                let handles = mask_handle_points(&geom_to_view(g, dims, deg, &dist), xf);
                let hits = |p: egui::Pos2| handles.iter().any(|(_, hp)| hp.distance(p) <= HANDLE_HIT);
                resp.hover_pos().is_some_and(hits)
                    || ui.input(|i| i.pointer.press_origin()).is_some_and(hits)
            });
        let zoom_pan = self.zoom > 1.001
            && !tool_active
            && !ctrl
            && !over_knob
            && self.region_drag.is_none();
        let panning = resp.dragged_by(egui::PointerButton::Middle)
            || ((space || zoom_pan) && resp.dragged_by(egui::PointerButton::Primary));
        if panning {
            let d = resp.drag_delta();
            let ext = 1.0 / self.zoom; // visible extent in crop-window coords
            self.pan -= egui::vec2(
                d.x / rect.width().max(1.0) * ext,
                d.y / rect.height().max(1.0) * ext,
            );
        }

        // Cursor language: say what a click/drag would do right now. The pick
        // tools (WB / range / clone-source) set their own crosshair in their
        // handlers; this covers the hand for panning and the drawing tools.
        if panning {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if (space || zoom_pan) && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if resp.hovered()
            && (self.paint_mode || self.placing_mask.is_some() || self.crop_mode)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }

        if comparing || panning {
            return; // tools pause while comparing / panning
        }

        // --- tool dispatch (one active interaction at a time) -----------------
        if self.crop_mode {
            self.handle_crop(ui, &resp, xf, tex_size);
        } else if self.placing_mask.is_some() {
            self.handle_place_mask(ui, &resp, xf);
        } else if self.wb_picking {
            self.handle_wb_pick(ui, &resp, xf);
        } else if self.range_picking.is_some() {
            self.handle_range_pick(ui, &resp, xf);
        } else if self.clone_mode {
            self.handle_clone(ui, &resp, xf);
            self.ensure_mask_tex(ui.ctx());
            if let Some(t) = &self.mask_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
        } else if self.paint_mode {
            self.handle_paint(&resp, xf);
            self.ensure_mask_tex(ui.ctx());
            if let Some(t) = &self.mask_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
        } else {
            // The selected mask's on-image knobs take priority over box-select
            // (a knob hit means "edit the mask", never "start a region").
            if !self.handle_mask_edit(ui, &resp, xf) {
                self.handle_region_select(ui, &resp, xf);
            }
        }

        // The committed AI region (and a drag in flight) paints EVERY frame it
        // exists — it still feeds the analyze prompt while a tool or a knob
        // hover owns the pointer, so hiding it there misrepresented what the
        // AI would see.
        {
            let stroke = egui::Stroke::new(2.0, ACCENT);
            let fill =
                egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 40);
            let draw = |r: egui::Rect| {
                ui.painter().rect_filled(r, 0.0, fill);
                ui.painter().rect_stroke(r, 0.0, stroke);
            };
            if let Some((s, e)) = self.region_drag {
                draw(egui::Rect::from_two_pos(s, e).intersect(rect));
            } else if let Some([l, t, rr, bb]) = self.region {
                // Stored in the original frame → back into the transformed view.
                let ((bw, bh), deg, dist) = self.geom_ctx();
                let a = orig_norm_to_view(l, t, (bw, bh), deg, &dist);
                let b2 = orig_norm_to_view(rr, bb, (bw, bh), deg, &dist);
                draw(
                    egui::Rect::from_min_max(xf.to_screen(a.0, a.1), xf.to_screen(b2.0, b2.1))
                        .intersect(rect),
                );
            }
        }

        // Selected mask stays visualised so its sliders have visual feedback
        // (geometry is stored in the original frame → map into the view),
        // with the editing knobs on top (drag = reshape/move, handle_mask_edit).
        // When one of the mask's COMPONENTS is selected, the outline and
        // knobs target THAT shape — the same geometry the edit handler
        // mutates (sel_target_geometry).
        if !self.crop_mode
            && self.placing_mask.is_none()
            && let Some(m) = self.sel_mask.and_then(|i| self.recipe.masks.get(i))
        {
            let target = match self.sel_component {
                Some(c) if c < m.components.len() => &m.components[c].geometry,
                _ => &m.mask,
            };
            let (dims, deg, dist) = self.geom_ctx();
            let vg = geom_to_view(target, dims, deg, &dist);
            let geom_active = dist.amount != 0.0
                || (dist.profile.distortion_on && !dist.profile.distortion.is_empty());
            if let MaskGeometry::Radial { top, left, bottom, right, flipped, angle, .. } = target
                && (deg != 0.0 || geom_active)
            {
                // The engine evaluates the ellipse in the ORIGINAL frame; under
                // straighten/distortion its image is a rotated/warped curve —
                // not the axis-aligned ellipse the bbox mapping yields. Sample
                // it parametrically — the mask's OWN rotation folded in first,
                // the same order the engine applies — and draw the true
                // outline. (Knobs keep the bbox positions; HANDLE_HIT absorbs
                // the difference.)
                let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
                let (rx, ry) = ((right - left) / 2.0, (bottom - top) / 2.0);
                let (s_a, c_a) = angle.to_radians().sin_cos();
                let pts: Vec<egui::Pos2> = (0..48)
                    .map(|k| {
                        let th = k as f32 / 48.0 * std::f32::consts::TAU;
                        let (ex, ey) = (rx * th.cos(), ry * th.sin());
                        let (vx, vy) = orig_norm_to_view(
                            cx + ex * c_a - ey * s_a,
                            cy + ex * s_a + ey * c_a,
                            dims,
                            deg,
                            &dist,
                        );
                        xf.to_screen(vx, vy)
                    })
                    .collect();
                let p = ui.painter_at(xf.rect);
                p.add(egui::Shape::closed_line(pts, egui::Stroke::new(2.0, ACCENT)));
                // Same effect-side marker semantics as draw_mask_overlay.
                let (ccx, ccy) = orig_norm_to_view(cx, cy, dims, deg, &dist);
                let cs = xf.to_screen(ccx, ccy);
                if flipped ^ m.inverted {
                    p.circle_stroke(cs, 3.0, egui::Stroke::new(2.0, ACCENT));
                } else {
                    p.circle_filled(cs, 3.0, ACCENT);
                }
            } else {
                draw_mask_overlay(ui, xf, &vg, m.inverted, self.lang);
            }
            let p = ui.painter_at(xf.rect);
            for (h, pos) in mask_handle_points(&vg, xf) {
                let r = if h == 0 { 5.5 } else { 4.5 }; // centre knob reads bigger
                p.circle_filled(pos, r, egui::Color32::WHITE);
                p.circle_stroke(pos, r, egui::Stroke::new(1.5, ACCENT));
            }
        }
    }

    /// WB eyedropper: click a pixel that SHOULD be neutral and the engine's
    /// inverse solver (`render::solve_wb_from_neutral` — the same forward
    /// model the render applies, anchored at this photo's stamped as-shot
    /// Kelvin, or the legacy 5500 K) turns it into Temp + Tint.
    /// Samples a 5×5 mean of the SOURCE preview: WB runs before develop, so
    /// the solve must see pre-develop pixels, not the current edit.
    fn handle_wb_pick(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if !resp.clicked() {
            return;
        }
        let Some(q) = resp.interact_pointer_pos() else { return };
        let (nx, ny) = xf.to_norm(q);
        // base_preview is the ORIGINAL frame — map out of the transformed view.
        let ((bw, bh), deg, dist) = self.geom_ctx();
        let (nx, ny) = view_norm_to_orig(nx, ny, (bw, bh), deg, &dist);
        let px = {
            let Some(base) = &self.base_preview else { return };
            let Some(px) = sample_5x5_mean(base, nx, ny) else { return };
            px
        };
        let (k, tint) = autoshop::render::solve_wb_from_neutral(
            px,
            self.recipe.as_shot_k.unwrap_or(5500.0),
        );
        self.recipe.temperature_k = Some(k);
        self.recipe.tint = tint;
        self.wb_picking = false;
        self.dirty = true;
        self.status = trf(
            self.lang,
            "WB eyedropper: {k} K · tint {tint} — fine-tune in the Tone section",
            &[("k", &format!("{k:.0}")), ("tint", &format!("{tint:+.0}"))],
        );
    }

    /// Colour-range sample: click keys the pending mask's Color range to that
    /// spot. Samples a 5×5 mean of a PRE-MASK develop (this recipe with masks
    /// stripped) — the exact pixel state `apply_masks` evaluates range weights
    /// against, so the picked colour is what the engine will match. One extra
    /// preview-sized develop per click ≈ the cost of one slider tick.
    fn handle_range_pick(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if !resp.clicked() {
            return;
        }
        let Some(q) = resp.interact_pointer_pos() else { return };
        let Some(mi) = self.range_picking.filter(|&i| i < self.recipe.masks.len()) else {
            self.range_picking = None; // stale index (mask deleted mid-pick)
            return;
        };
        let (nx, ny) = xf.to_norm(q);
        // develop_preview works in the ORIGINAL frame — map out of the view.
        let ((bw, bh), deg, dist) = self.geom_ctx();
        let (nx, ny) = view_norm_to_orig(nx, ny, (bw, bh), deg, &dist);
        let smp = {
            let Some(base) = self.base_preview.clone() else { return };
            // The PREFIX reference (masks before this one — the engine
            // evaluates a range on the pixel as it stands when THIS mask
            // runs), built EXACTLY like the coverage overlay's (geometry
            // fields stripped — develop_preview has no geometry stage,
            // they'd only break the cache key) and SHARING its cache: with
            // the overlay on, a click costs a 5×5 read instead of a full
            // preview develop on the UI thread — and a miss here pre-warms
            // the overlay's next rebuild. A masks-CLEARED reference judged
            // mask 2's range on pixels mask 0/1 had already moved (CX5-6).
            let mut pre = self.recipe.clone();
            pre.masks.truncate(mi);
            pre.straighten_deg = 0.0;
            pre.lens_distortion = 0.0;
            pre.crop = None;
            if !matches!(&self.overlay_ref, Some((r, _)) if *r == pre) {
                let img = autoshop::render::develop_preview(&base, &pre);
                self.overlay_ref = Some((pre, img));
            }
            let reference = &self.overlay_ref.as_ref().expect("range reference cached").1;
            let Some(smp) = sample_5x5_mean(reference, nx, ny) else { return };
            smp
        };
        // Keep the tolerance the user already dialled in; only re-key the colour.
        let amount = match self.recipe.masks[mi].range {
            Some(RangeMask::Color { amount, .. }) => amount,
            _ => 0.5,
        };
        self.recipe.masks[mi].range =
            Some(RangeMask::Color { r: smp[0], g: smp[1], b: smp[2], amount, px: nx, py: ny });
        self.range_picking = None;
        self.dirty = true;
        self.status = tr(
            self.lang,
            "Color range: sampled — the 「Tolerance」 slider adjusts the selection width",
        )
        .into();
    }

    /// Box-select on the After image: drag a rectangle to target a local edit;
    /// the normalized box is folded into the AI direction so it masks exactly
    /// there (mirrors the web region→mask prompt). A plain click — or a tiny
    /// drag — clears the selection. Coordinates are full-frame normalized (the
    /// AI mask space), mapped through the view transform.
    /// On-image editing of the SELECTED mask's geometry (the LR gesture):
    /// drag an end/edge knob to reshape, the centre knob to move the whole
    /// mask — no more redraw-from-scratch via 重画. Geometry lives in the
    /// ORIGINAL frame, so every write maps the pointer back through
    /// view_norm_to_orig — the same chain placement uses. Returns true while
    /// it owns the pointer (hovering a knob or mid-drag) so box-select
    /// doesn't also react; a box-select drag already in flight keeps
    /// priority (else its live rectangle would freeze mid-air whenever the
    /// pointer crossed a knob).
    /// The geometry the canvas tools currently target: the selected mask's
    /// selected COMPONENT when one is selected (and still in bounds), else
    /// the selected mask's base geometry.
    fn sel_target_geometry(&self) -> Option<&MaskGeometry> {
        let i = self.sel_mask.filter(|&i| i < self.recipe.masks.len())?;
        let m = &self.recipe.masks[i];
        match self.sel_component {
            Some(c) if c < m.components.len() => Some(&m.components[c].geometry),
            _ => Some(&m.mask),
        }
    }

    fn sel_target_geometry_mut(&mut self) -> Option<&mut MaskGeometry> {
        let i = self.sel_mask.filter(|&i| i < self.recipe.masks.len())?;
        let m = &mut self.recipe.masks[i];
        match self.sel_component {
            Some(c) if c < m.components.len() => Some(&mut m.components[c].geometry),
            _ => Some(&mut m.mask),
        }
    }

    fn handle_mask_edit(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) -> bool {
        if self.region_drag.is_some() {
            return false;
        }
        let Some(target_geom) = self.sel_target_geometry().cloned() else {
            self.mask_drag = None;
            return false;
        };
        let (dims, deg, dist) = self.geom_ctx();
        let view_geom = geom_to_view(&target_geom, dims, deg, &dist);
        let handles = mask_handle_points(&view_geom, xf);
        if handles.is_empty() {
            self.mask_drag = None; // bitmap: nothing parametric to drag
            return false;
        }
        // A stranded grab (Esc / hold-B / pan swallowed the drag_stopped frame)
        // must not turn the NEXT unrelated drag into a mask reshape: no drag in
        // flight ⇒ no grab.
        if !resp.dragged() && !resp.drag_started() {
            self.mask_drag = None;
        }
        // Nearest handle wins, ties broken toward the edge/end knobs — the
        // centre-move knob is emitted first and used to shadow them whenever a
        // small mask put everything within HANDLE_HIT.
        let pick = |p: egui::Pos2| {
            handles
                .iter()
                .filter(|(_, hp)| hp.distance(p) <= HANDLE_HIT)
                .min_by(|a, b| {
                    a.1.distance(p).total_cmp(&b.1.distance(p)).then(b.0.cmp(&a.0))
                })
                .map(|(h, _)| *h)
        };
        let hover_h = resp.hover_pos().and_then(pick);
        let orig_at = |p: egui::Pos2| {
            let (nx, ny) = xf.to_norm(p);
            view_norm_to_orig(nx, ny, dims, deg, &dist)
        };
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // egui reports drag_started only after ~6 px of travel — a fast
            // flick has left the knob by then. Hit-test the PRESS ORIGIN so
            // quick grabs don't fall through to box-select — and ANCHOR the
            // delta there too: anchoring at the current pointer discarded
            // the lead-in motion (a short drag left the edge unmoved).
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if let Some(h) = pick(origin) {
                self.mask_drag = Some((h, orig_at(origin)));
            }
        }
        if resp.dragged()
            && let (Some((h, last)), Some(p)) = (self.mask_drag, resp.interact_pointer_pos())
        {
            let cur = orig_at(p);
            let (dx, dy) = (cur.0 - last.0, cur.1 - last.1);
            // LR allows geometry to start off-canvas; a generous band keeps
            // knobs recoverable instead of letting them fly to infinity.
            let cl = |v: f32| v.clamp(-0.5, 1.5);
            match self.sel_target_geometry_mut() {
                Some(MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }) => match h {
                    1 => (*zero_x, *zero_y) = (cl(cur.0), cl(cur.1)),
                    2 => (*full_x, *full_y) = (cl(cur.0), cl(cur.1)),
                    _ => {
                        *zero_x = cl(*zero_x + dx);
                        *zero_y = cl(*zero_y + dy);
                        *full_x = cl(*full_x + dx);
                        *full_y = cl(*full_y + dy);
                    }
                },
                Some(MaskGeometry::Radial { top, left, bottom, right, angle, .. }) => {
                    const MIN_SIZE: f32 = 0.01;
                    // Edges move by the pointer's original-space DELTA, not to
                    // its absolute position: the displayed handles sit on the
                    // transformed BOUNDING BOX (≠ the true transformed ellipse
                    // under straighten/distortion), so an absolute assign made
                    // the edge jump to close that gap on the first drag frame.
                    // Under neutral geometry the two are identical.
                    //
                    // With a rotated mask the four knobs sit on the ROTATED
                    // axis endpoints, so the delta is projected onto the
                    // mask's own axes first — at angle 0 (du, dv) ≡ (dx, dy).
                    let (s_a, c_a) = angle.to_radians().sin_cos();
                    let du = dx * c_a + dy * s_a;
                    let dv = -dx * s_a + dy * c_a;
                    match h {
                        1 => *left = cl(*left + du).min(*right - MIN_SIZE),
                        2 => *top = cl(*top + dv).min(*bottom - MIN_SIZE),
                        3 => *right = cl(*right + du).max(*left + MIN_SIZE),
                        4 => *bottom = cl(*bottom + dv).max(*top + MIN_SIZE),
                        5 => {
                            // Rotation grip: it is parked toward the TOP axis
                            // endpoint, so the ellipse angle is the pointer's
                            // bearing from the centre + 90° (same rotation
                            // convention as the engine — recipe.rs Radial).
                            let (cx, cy) = ((*left + *right) / 2.0, (*top + *bottom) / 2.0);
                            let th =
                                (cur.1 - cy).atan2(cur.0 - cx).to_degrees() + 90.0;
                            *angle = th.rem_euclid(360.0);
                            if *angle > 180.0 {
                                *angle -= 360.0;
                            }
                        }
                        _ => {
                            // Clamp the SHIFT, not each edge — independent
                            // clamps at the band boundary squashed the
                            // ellipse toward zero size instead of parking it.
                            let (lx, hx) = (-0.5 - *left, 1.5 - *right);
                            let (ly, hy) = (-0.5 - *top, 1.5 - *bottom);
                            let dx = dx.clamp(lx.min(hx), lx.max(hx));
                            let dy = dy.clamp(ly.min(hy), ly.max(hy));
                            *left += dx;
                            *right += dx;
                            *top += dy;
                            *bottom += dy;
                        }
                    }
                }
                _ => {}
            }
            self.mask_drag = Some((h, cur));
            self.dirty = true; // masks are develop stages — live preview
            self.overlay_stale = true;
        }
        if resp.drag_stopped() {
            self.mask_drag = None; // commit_if_settled turns the drag into ONE undo step
        }
        if self.mask_drag.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hover_h.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        self.mask_drag.is_some() || hover_h.is_some()
    }

    fn handle_region_select(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                // Anchor at the PRESS ORIGIN: egui reports drag_started only
                // after ~6 px of travel, so anchoring at the current pointer
                // shaved that lead-in off every box (small fast selections
                // could then die to the 0.02 minimum below).
                let s = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
                self.region_drag = Some((s, p));
            }
        } else if resp.dragged() {
            if let (Some(p), Some((s, _))) = (resp.interact_pointer_pos(), self.region_drag) {
                self.region_drag = Some((s, p));
            }
        } else if resp.drag_stopped() {
            if let Some((s, e)) = self.region_drag.take() {
                // The region feeds the AI's mask prompt — ORIGINAL frame space.
                // ALL FOUR corners map (the same policy as serve's
                // region_to_original): under rotation/distortion the two
                // diagonal corners alone describe a different — sometimes
                // near-degenerate — box than the rectangle the user drew.
                let ((bw, bh), deg, dist) = self.geom_ctx();
                let map = |p: egui::Pos2| {
                    let (nx, ny) = xf.to_norm(p);
                    view_norm_to_orig(nx, ny, (bw, bh), deg, &dist)
                };
                let corners = [
                    map(s),
                    map(e),
                    map(egui::pos2(s.x, e.y)),
                    map(egui::pos2(e.x, s.y)),
                ];
                let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for (x, y) in corners {
                    l = l.min(x);
                    t = t.min(y);
                    r = r.max(x);
                    b = b.max(y);
                }
                if r - l > 0.02 && b - t > 0.02 {
                    self.region = Some([l, t, r, b]);
                    self.status = trf(
                        self.lang,
                        "region {w}×{h}% — type a direction, then AI Analyze (click to clear)",
                        &[
                            ("w", &(((r - l) * 100.0).round() as i32).to_string()),
                            ("h", &(((b - t) * 100.0).round() as i32).to_string()),
                        ],
                    );
                } else {
                    self.region = None; // a tiny drag clears the selection
                }
            }
        } else if resp.clicked() && !resp.double_clicked() {
            // Remember what this click cleared: if it turns out to be the
            // first half of a Fit↔1:1 double-click, after_view restores it
            // (M18 — zooming must not eat the AI region).
            //
            // `&& !resp.double_clicked()`: egui sets `clicked` on the release
            // that COMPLETES the pair too, and after_view restores the region
            // earlier in this same frame — so without this the restore was
            // taken straight back out and the double-click still ate the
            // region. The fix was inert until this line existed.
            self.region_restore = self.region.take().map(|r| (r, Instant::now()));
        }
        // (Drawing lives in after_view so the box shows under every tool —
        // this handler only owns the gesture.)
    }

    /// The interactive crop overlay: darkened surround, thirds grid, corner +
    /// edge handles (aspect-constrained), move-inside, and drag OUTSIDE the
    /// box to rotate-straighten (the LR gesture set). The crop lives in
    /// `recipe.crop` (full-frame normalized) — exactly what the export render
    /// and the XMP already apply, so the tool adds no new data path.
    fn handle_crop(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        xf: ViewXform,
        tex_size: egui::Vec2,
    ) {
        use autoshop::recipe::Crop;
        // Pixel aspect ratio (w/h) requested by the preset; "原始" resolves here.
        // `tex_size` is the CURRENT after texture: right after a straighten
        // change with the develop still in flight it can be one frame stale,
        // and a preset applied in that instant derives against the previous
        // frame's dims (recorded residual, CX5-5 — recomputing inscribed
        // dims here would duplicate the engine's geometry; the next
        // interaction re-derives correctly).
        let aspect = CROP_ASPECTS[self.crop_aspect.min(CROP_ASPECTS.len() - 1)]
            .1
            .map(|r| if r == 0.0 { tex_size.x / tex_size.y.max(1.0) } else { r });
        // A preset picked in the panel re-derives the box NOW (LR applies the
        // aspect immediately, D13): largest same-centre fit at the new
        // ratio, shrunk to stay in frame. "Free" keeps the box untouched.
        if std::mem::take(&mut self.crop_aspect_pending)
            && let Some(r_px) = aspect
        {
            let rn = r_px * tex_size.y.max(1.0) / tex_size.x.max(1.0);
            let c0 = self
                .recipe
                .crop
                .map(|c| [c.left, c.top, c.right, c.bottom])
                .unwrap_or([0.0, 0.0, 1.0, 1.0]);
            let (cx, cy) = ((c0[0] + c0[2]) / 2.0, (c0[1] + c0[3]) / 2.0);
            let (mut w, mut h) = (c0[2] - c0[0], c0[3] - c0[1]);
            if w / h.max(1e-6) > rn {
                w = h * rn;
            } else {
                h = w / rn.max(1e-6);
            }
            let s = ((2.0 * cx.min(1.0 - cx)) / w.max(1e-6))
                .min((2.0 * cy.min(1.0 - cy)) / h.max(1e-6))
                .min(1.0);
            let (w, h) = (w * s, h * s);
            // The drag path refuses boxes under 0.05 a side — the preset
            // path must not create what the handles could then never
            // reproduce (Codex batch-38). Growing can shift the box off its
            // centre at the frame edge, so the position re-clamps after.
            let grow = (0.05_f32 / w.max(1e-6)).max(0.05_f32 / h.max(1e-6)).max(1.0);
            let (w, h) = ((w * grow).min(1.0), (h * grow).min(1.0));
            let left = (cx - w / 2.0).clamp(0.0, 1.0 - w);
            let top = (cy - h / 2.0).clamp(0.0, 1.0 - h);
            let next = Some(Crop { left, top, right: left + w, bottom: top + h });
            if self.recipe.crop != next {
                self.recipe.crop = next;
                self.dirty = true; // histogram/clipping follow the crop
            }
        }
        let cur = self
            .recipe
            .crop
            .map(|c| [c.left, c.top, c.right, c.bottom])
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);

        // Handle order: 0=TL 1=TR 2=BL 3=BR, 4=move (inside), 5=T 6=B 7=L 8=R
        // edge midpoints, 9=rotate (anywhere outside the box).
        const HIT: f32 = 12.0; // handle pick radius, px — shared by drag + cursor
        let handle_pos = |c: &[f32; 4], k: u8| match k {
            0 => xf.to_screen(c[0], c[1]),
            1 => xf.to_screen(c[2], c[1]),
            2 => xf.to_screen(c[0], c[3]),
            3 => xf.to_screen(c[2], c[3]),
            5 => xf.to_screen((c[0] + c[2]) / 2.0, c[1]),
            6 => xf.to_screen((c[0] + c[2]) / 2.0, c[3]),
            7 => xf.to_screen(c[0], (c[1] + c[3]) / 2.0),
            _ => xf.to_screen(c[2], (c[1] + c[3]) / 2.0),
        };
        // Corners win over edges on a tiny box (checked first); inside = move;
        // anywhere else on the canvas = rotate-straighten, so the pick is
        // total — implicit pan is already suppressed while crop is armed.
        let pick_handle = |c: &[f32; 4], p: egui::Pos2| {
            (0u8..4)
                .chain(5u8..9)
                .find(|&k| handle_pos(c, k).distance(p) <= HIT)
                .or_else(|| {
                    let r = egui::Rect::from_min_max(
                        xf.to_screen(c[0], c[1]),
                        xf.to_screen(c[2], c[3]),
                    );
                    // A box flush with the canvas (a fresh full-frame crop)
                    // has no outside — the promised rotate-straighten gesture
                    // was unreachable. Only a FULLY flush box donates inner
                    // bands (moving it is a no-op anyway, D13); a box merely
                    // touching ONE canvas edge still has real outside to
                    // rotate from, and per-edge donation turned a move near
                    // that edge into a surprise rotate (M20).
                    const BAND: f32 = 16.0;
                    let mut inner = r.intersect(xf.rect);
                    let full_frame = r.min.x <= xf.rect.min.x + 1.0
                        && r.min.y <= xf.rect.min.y + 1.0
                        && r.max.x >= xf.rect.max.x - 1.0
                        && r.max.y >= xf.rect.max.y - 1.0;
                    if full_frame {
                        inner.min.x += BAND;
                        inner.min.y += BAND;
                        inner.max.x -= BAND;
                        inner.max.y -= BAND;
                    }
                    Some(if inner.contains(p) { 4 } else { 9 })
                })
        };
        // A stranded grab (Esc mid-drag, or leaving/re-entering crop mode)
        // must not resume against a stale anchor: no drag in flight ⇒ no grab.
        if !resp.dragged() && !resp.drag_started() {
            self.crop_drag = None;
        }
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Hit-test the PRESS ORIGIN — drag_started fires only after ~6 px
            // of travel, and a fast corner grab has already left the handle.
            // The drag ANCHOR is the origin too: anchoring at the current
            // pointer discarded the lead-in motion (and let the dominant-axis
            // pick follow a small residual instead of the user's gesture).
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if let Some(h) = pick_handle(&cur, origin) {
                self.crop_drag = Some((h, origin, cur, self.recipe.straighten_deg));
            }
        }
        if resp.dragged()
            && let (Some((h, start, orig, deg0)), Some(p)) =
                (self.crop_drag, resp.interact_pointer_pos())
        {
            if h == 9 {
                // Rotate-straighten: drag outside the box (LR). Angle around
                // the box centre, screen space — y-down atan2 makes clockwise
                // positive, matching the engine's clockwise straighten °; the
                // image turns WITH the drag. The box mapping is unaffected
                // mid-drag (recipe.crop lives in the post-rotation view frame).
                let a = xf.to_screen(orig[0], orig[1]);
                let b = xf.to_screen(orig[2], orig[3]);
                let centre = egui::pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                let ang = |q: egui::Pos2| (q.y - centre.y).atan2(q.x - centre.x);
                // Wrap the delta into (-180°, 180°]: crossing atan2's ±180°
                // branch cut otherwise reads a 2° clockwise move near the
                // leftward ray as −358° and slams the slider to the clamp.
                let delta = (ang(p) - ang(start)).to_degrees();
                let delta = (delta + 540.0) % 360.0 - 180.0;
                let next = (deg0 + delta).clamp(-45.0, 45.0);
                if self.recipe.straighten_deg != next {
                    self.recipe.straighten_deg = next;
                    self.dirty = true;
                }
            } else {
                let (sn, pn) = (xf.to_norm(start), xf.to_norm(p));
                let (dx, dy) = (pn.0 - sn.0, pn.1 - sn.1);
                // The preset is a PIXEL w/h ratio; geometry below works in
                // normalised view space, so convert once.
                let ratio_n =
                    aspect.map(|r_px| r_px * tex_size.y.max(1.0) / tex_size.x.max(1.0));
                let c: [f32; 4] = if h == 4 {
                    // Move: shift, clamped so the rect stays inside the frame.
                    let (w, hg) = (orig[2] - orig[0], orig[3] - orig[1]);
                    let nl = (orig[0] + dx).clamp(0.0, 1.0 - w);
                    let nt = (orig[1] + dy).clamp(0.0, 1.0 - hg);
                    [nl, nt, nl + w, nt + hg]
                } else if h == 5 || h == 6 {
                    // Top/bottom edge: anchored at the opposite edge. Free
                    // aspect moves only this edge; a fixed aspect rederives
                    // the width centred on the box's vertical axis (the LR
                    // edge behaviour), shrinking to stay in frame.
                    let ay = if h == 5 { orig[3] } else { orig[1] };
                    let y = (if h == 5 { orig[1] } else { orig[3] } + dy).clamp(0.0, 1.0);
                    let mut h_n = (y - ay).abs();
                    let (mut l, mut r) = (orig[0], orig[2]);
                    if let Some(rn) = ratio_n {
                        let cx = (orig[0] + orig[2]) / 2.0;
                        let mut w_n = h_n * rn;
                        let room = 2.0 * cx.min(1.0 - cx);
                        if w_n > room {
                            w_n = room;
                            h_n = w_n / rn;
                        }
                        l = cx - w_n / 2.0;
                        r = cx + w_n / 2.0;
                    }
                    let y = if h == 5 { ay - h_n } else { ay + h_n };
                    [l, y.min(ay), r, y.max(ay)]
                } else if h == 7 || h == 8 {
                    // Left/right edge — the transpose of the above.
                    let ax = if h == 7 { orig[2] } else { orig[0] };
                    let x = (if h == 7 { orig[0] } else { orig[2] } + dx).clamp(0.0, 1.0);
                    let mut w_n = (x - ax).abs();
                    let (mut t, mut b) = (orig[1], orig[3]);
                    if let Some(rn) = ratio_n {
                        let cy = (orig[1] + orig[3]) / 2.0;
                        let mut h_n = w_n / rn;
                        let room = 2.0 * cy.min(1.0 - cy);
                        if h_n > room {
                            h_n = room;
                            w_n = h_n * rn;
                        }
                        t = cy - h_n / 2.0;
                        b = cy + h_n / 2.0;
                    }
                    let x = if h == 7 { ax - w_n } else { ax + w_n };
                    [x.min(ax), t, x.max(ax), b]
                } else {
                    // Corner: drag it, anchored at the opposite corner.
                    let (ax, ay) = match h {
                        0 => (orig[2], orig[3]),
                        1 => (orig[0], orig[3]),
                        2 => (orig[2], orig[1]),
                        _ => (orig[0], orig[1]),
                    };
                    let mut x = (match h {
                        0 | 2 => orig[0] + dx,
                        _ => orig[2] + dx,
                    })
                    .clamp(0.0, 1.0);
                    let mut y = (match h {
                        0 | 1 => orig[1] + dy,
                        _ => orig[3] + dy,
                    })
                    .clamp(0.0, 1.0);
                    if let Some(rn) = ratio_n {
                        // The DOMINANT drag axis drives and the other follows
                        // the ratio. Dominance comes from the DRAG DELTAS (in
                        // width units), not max(extents): max() can only grow
                        // past the stale axis, so an inward pull dominated by
                        // one axis left the corner pinned at the old size.
                        let top_corner = h == 0 || h == 1;
                        let mut w_n = if dx.abs() >= dy.abs() * rn {
                            (x - ax).abs()
                        } else {
                            (y - ay).abs() * rn
                        };
                        let mut h_n = w_n / rn;
                        let room = if top_corner { ay } else { 1.0 - ay };
                        if h_n > room {
                            h_n = room;
                            w_n = h_n * rn;
                        }
                        // Horizontal room too: a dominantly VERTICAL pull can
                        // derive a width wider than the frame leaves on the
                        // driven side — x would land outside 0..1 and the
                        // final min/max can't repair that.
                        let x_room = if x >= ax { 1.0 - ax } else { ax };
                        if w_n > x_room {
                            w_n = x_room;
                            h_n = w_n / rn;
                        }
                        x = if x >= ax { ax + w_n } else { ax - w_n };
                        y = if top_corner { ay - h_n } else { ay + h_n };
                    }
                    [x.min(ax), y.min(ay), x.max(ax), y.max(ay)]
                };
                if c[2] - c[0] >= 0.05 && c[3] - c[1] >= 0.05 {
                    let next = Some(Crop { left: c[0], top: c[1], right: c[2], bottom: c[3] });
                    // The crop IS a develop input: build_preview restricts the
                    // histogram + clipping to it, so a crop change that never
                    // sets `dirty` leaves both describing the OLD crop until
                    // some unrelated slider is touched.
                    if self.recipe.crop != next {
                        self.recipe.crop = next;
                        self.dirty = true;
                    }
                }
            }
        }
        if resp.drag_stopped() {
            self.crop_drag = None;
        }

        // Cursor affordance: name the resize direction of the handle under
        // the pointer — or of the one being DRAGGED, since the pointer can
        // lag off a corner mid-drag. Runs after show_image's generic
        // crosshair set, and cursor_icon is last-write-wins, so this
        // overrides it exactly when a handle would take the drag.
        let c = self
            .recipe
            .crop
            .map(|c| [c.left, c.top, c.right, c.bottom])
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let hover_handle = self
            .crop_drag
            .map(|(h, ..)| h)
            .or_else(|| resp.hover_pos().and_then(|p| pick_handle(&c, p)));
        if let Some(h) = hover_handle {
            ui.ctx().set_cursor_icon(match h {
                0 | 3 => egui::CursorIcon::ResizeNwSe, // TL/BR diagonal
                1 | 2 => egui::CursorIcon::ResizeNeSw, // TR/BL diagonal
                5 | 6 => egui::CursorIcon::ResizeVertical, // top/bottom edge
                7 | 8 => egui::CursorIcon::ResizeHorizontal, // left/right edge
                4 => egui::CursorIcon::Move,           // inside: move the window
                _ => egui::CursorIcon::Grab,           // outside: rotate-straighten
            });
        }

        // --- overlay: darkened surround + thirds + handles --------------------
        let p = ui.painter_at(xf.rect);
        // The TRUE box for guides + border (the painter clips): computing
        // the thirds on the viewport-intersected rect drew them at thirds of
        // the VISIBLE PORTION whenever zoom pushed part of the box
        // off-screen (CX5-8). The shading keeps the intersected rect — its
        // job is exactly the visible surround.
        let r_full =
            egui::Rect::from_min_max(xf.to_screen(c[0], c[1]), xf.to_screen(c[2], c[3]));
        let r = r_full.intersect(xf.rect);
        let dark = egui::Color32::from_black_alpha(140);
        let full = xf.rect;
        for shade in [
            egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, r.min.y)), // top
            egui::Rect::from_min_max(egui::pos2(full.min.x, r.max.y), full.max), // bottom
            egui::Rect::from_min_max(egui::pos2(full.min.x, r.min.y), egui::pos2(r.min.x, r.max.y)),
            egui::Rect::from_min_max(egui::pos2(r.max.x, r.min.y), egui::pos2(full.max.x, r.max.y)),
        ] {
            if shade.width() > 0.0 && shade.height() > 0.0 {
                p.rect_filled(shade, 0.0, dark);
            }
        }
        // O cycles the guide while cropping (LR): thirds → golden → off.
        let guide: &[f32] = match self.crop_grid {
            0 => &[1.0 / 3.0, 2.0 / 3.0],
            1 => &[0.381_966, 0.618_034], // 1−1/φ and 1/φ
            _ => &[],
        };
        let grid = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70));
        for &t in guide {
            p.line_segment(
                [
                    egui::pos2(r_full.min.x + t * r_full.width(), r_full.min.y),
                    egui::pos2(r_full.min.x + t * r_full.width(), r_full.max.y),
                ],
                grid,
            );
            p.line_segment(
                [
                    egui::pos2(r_full.min.x, r_full.min.y + t * r_full.height()),
                    egui::pos2(r_full.max.x, r_full.min.y + t * r_full.height()),
                ],
                grid,
            );
        }
        p.rect_stroke(r_full, 0.0, egui::Stroke::new(1.5, egui::Color32::WHITE));
        for k in 0..4u8 {
            p.rect_filled(
                egui::Rect::from_center_size(handle_pos(&c, k), egui::vec2(9.0, 9.0)),
                1.0,
                egui::Color32::WHITE,
            );
        }
        // Edge midpoints as slim pills, so the affordance reads as "this edge"
        // rather than a fifth corner.
        for k in [5u8, 6, 7, 8] {
            let sz = if k <= 6 { egui::vec2(15.0, 5.0) } else { egui::vec2(5.0, 15.0) };
            p.rect_filled(
                egui::Rect::from_center_size(handle_pos(&c, k), sz),
                1.0,
                egui::Color32::WHITE,
            );
        }
    }

    /// Place (or re-draw) a manual local-adjustment mask by dragging: the drag
    /// vector defines a linear gradient (press = full-strength side, LR's
    /// direction; Shift = axis lock) or the
    /// bounding box of a radial. Commits into `recipe.masks` — the SAME field
    /// the AI writes, so render + XMP need nothing new.
    fn handle_place_mask(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        let Some((kind, target)) = self.placing_mask else { return };
        // Mask geometry lives in the ORIGINAL frame (the engine composites
        // masks before the geometric remap) — map pointer positions out of the view.
        let (dims, deg, dist) = self.geom_ctx();
        // place_start holds VIEW-normalized coords (not original-frame): the
        // radial bbox below must map ALL FOUR view-space corners — under
        // rotation/distortion the two dragged diagonals alone can describe a
        // thin or zero-area original-space box (the region handler maps all
        // four for exactly this reason).
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Anchor at the PRESS ORIGIN: drag_started fires ~6 px in, and
            // that lead-in shortened every gradient / shifted tiny radials.
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            self.place_start = Some(xf.to_norm(start));
        }
        let Some(sv) = self.place_start else { return };
        let Some(p) = resp.interact_pointer_pos() else { return };
        let mut ev = xf.to_norm(p);
        // LR: Shift locks a linear gradient to horizontal / vertical.
        // Snapped in VIEW space (what the user sees) — under straighten the
        // original-frame vector is legitimately off-axis (D13).
        if kind == MaskKind::Linear && ui.input(|i| i.modifiers.shift) {
            if (ev.0 - sv.0).abs() >= (ev.1 - sv.1).abs() {
                ev.1 = sv.1;
            } else {
                ev.0 = sv.0;
            }
        }
        let geom = match kind {
            MaskKind::Linear => {
                // Endpoints map exactly — two points suffice for a gradient.
                // LR anchors the FULL effect at the press and fades toward
                // the release (D13): press = full side, release = zero side.
                let s = view_norm_to_orig(sv.0, sv.1, dims, deg, &dist);
                let e = view_norm_to_orig(ev.0, ev.1, dims, deg, &dist);
                autoshop::recipe::MaskGeometry::Linear {
                    zero_x: e.0,
                    zero_y: e.1,
                    full_x: s.0,
                    full_y: s.1,
                }
            }
            MaskKind::Radial => {
                // Under straighten/distortion the dragged view-rect has NO
                // exact preimage in the model — Radial carries no rotation —
                // so the committed area is the bbox of the mapped corners: a
                // SUPERSET of the drag (the effect covers everything pointed
                // at; the alternative, axis-preserving, would miss corners).
                // The selected-mask outline then shows the TRUE transformed
                // ellipse (parametric sampler in draw_masks) and the edge
                // handles move by deltas, so the approximation never
                // compounds. Identity when geometry is neutral.
                let corners = [(sv.0, sv.1), (ev.0, ev.1), (sv.0, ev.1), (ev.0, sv.1)]
                    .map(|(x, y)| view_norm_to_orig(x, y, dims, deg, &dist));
                let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for (x, y) in corners {
                    l = l.min(x);
                    t = t.min(y);
                    r = r.max(x);
                    b = b.max(y);
                }
                // A degenerate box is useless and invisible — pad to the same
                // MIN_SIZE floor the edge handles enforce.
                const MIN_SIZE: f32 = 0.01;
                if r - l < MIN_SIZE {
                    let c = (l + r) * 0.5;
                    l = c - MIN_SIZE * 0.5;
                    r = c + MIN_SIZE * 0.5;
                }
                if b - t < MIN_SIZE {
                    let c = (t + b) * 0.5;
                    t = c - MIN_SIZE * 0.5;
                    b = c + MIN_SIZE * 0.5;
                }
                autoshop::recipe::MaskGeometry::Radial {
                    top: t,
                    left: l,
                    bottom: b,
                    right: r,
                    feather: 0.5,
                    roundness: 0.0,
                    flipped: false,
                    angle: 0.0,
                }
            }
        };
        // Live placement preview: no owning adjustment yet → markers upright.
        draw_mask_overlay(ui, xf, &geom_to_view(&geom, dims, deg, &dist), false, self.lang);
        if resp.drag_stopped() {
            let status: String = match target {
                PlaceTarget::Redraw(i) if i < self.recipe.masks.len() => {
                    // Redraw replaces the AREA only — a radial's tuned feather /
                    // roundness / flipped / angle are slider+handle state, and
                    // silently resetting them made ↻ Redraw destructive.
                    let mut geom = geom;
                    if let (
                        autoshop::recipe::MaskGeometry::Radial {
                            feather, roundness, flipped, angle, ..
                        },
                        autoshop::recipe::MaskGeometry::Radial {
                            feather: kept_f,
                            roundness: kept_r,
                            flipped: kept_fl,
                            angle: kept_a,
                            ..
                        },
                    ) = (&mut geom, &self.recipe.masks[i].mask)
                    {
                        *feather = *kept_f;
                        *roundness = *kept_r;
                        *flipped = *kept_fl;
                        *angle = *kept_a;
                    }
                    self.recipe.masks[i].mask = geom;
                    // A REDRAW kept the mask's existing (possibly nonzero)
                    // sliders — the "all 0 now" line below is only true for a
                    // brand-new mask.
                    tr(self.lang, "mask area redrawn — its existing adjustments now apply to the new area").into()
                }
                PlaceTarget::Component(i, mode) if i < self.recipe.masks.len() => {
                    let m = &mut self.recipe.masks[i];
                    m.components.push(autoshop::recipe::MaskComponent { geometry: geom, mode });
                    self.sel_mask = Some(i);
                    self.sel_component = Some(m.components.len() - 1);
                    tr(self.lang, "shape added to this mask — drag its knobs to adjust; the shape list is under the mask's row").into()
                }
                // NewMask — and the two index-carrying targets whose mask
                // vanished mid-drag (an async recipe replace): a redraw
                // degrades to a fresh mask (pre-v0.22 behaviour); a component
                // with no owner joins it rather than vanishing silently.
                _ => {
                    let n = self.recipe.masks.len();
                    let name = trf(self.lang, "Manual {n}", &[("n", &(n + 1).to_string())]);
                    self.recipe.masks.push(autoshop::recipe::LocalAdjustment {
                        mask: geom,
                        name,
                        ..Default::default()
                    });
                    self.sel_mask = Some(n);
                    self.sel_component = None;
                    tr(self.lang, "mask placed — pull its sliders in 「Local Masks」 at left (all 0 now, no visible effect yet)").into()
                }
            };
            self.placing_mask = None;
            self.place_start = None;
            self.dirty = true;
            self.overlay_stale = true;
            self.status = status;
        }
    }

    /// PNG bytes of the EXPORT mask: painted → transparent (regenerate / heal
    /// here), unpainted → opaque. None if nothing is painted — mirrors the web.
    fn export_mask_png(&self) -> Option<Vec<u8>> {
        let m = self.mask_paint.as_ref()?;
        let (w, h) = (m.width(), m.height());
        let mut out = image::RgbaImage::new(w, h);
        let mut any = false;
        for (x, y, p) in m.enumerate_pixels() {
            let painted = p.0[3] > 10;
            any |= painted;
            out.put_pixel(x, y, image::Rgba([0, 0, 0, if painted { 0 } else { 255 }]));
        }
        if !any {
            return None;
        }
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(out)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .ok()?;
        Some(buf)
    }

    fn clear_mask(&mut self) {
        if let Some(m) = &mut self.mask_paint {
            for p in m.pixels_mut() {
                *p = image::Rgba([0, 0, 0, 0]);
            }
            self.mask_dirty = true;
            // The WHOLE canvas changed: a pending brush sub-rect from an
            // uncommitted stroke would make ensure_mask_tex's partial-upload
            // fast path re-upload only that rectangle, leaving previously
            // painted pixels alive in the GPU texture indefinitely.
            self.mask_dirty_rect = None;
        }
        self.paint_last = None;
    }

    /// Arm a mask-brush session: `target = None` paints a NEW Bitmap mask on
    /// Apply; `Some(i)` edits mask `i`'s raster — seeded into the canvas (as
    /// the red display wash) and the greyscale weight buffer, at canvas
    /// resolution, so the session bakes exactly what it shows.
    fn start_mask_brush(&mut self, target: Option<usize>) {
        self.disarm_tools();
        let Some(base) = self.base_preview.as_ref() else { return };
        let (mw, mh) = base.dimensions();
        let mut gray = image::GrayImage::new(mw, mh);
        let mut canvas = image::RgbaImage::new(mw, mh);
        if let Some(i) = target
            && let Some(MaskGeometry::Bitmap { path }) = self.recipe.masks.get(i).map(|m| &m.mask)
        {
            match image::open(path) {
                Ok(img) => {
                    let g = image::imageops::resize(
                        &img.to_luma8(),
                        mw,
                        mh,
                        image::imageops::FilterType::Triangle,
                    );
                    for (x, y, p) in g.enumerate_pixels() {
                        if p[0] > 0 {
                            let a = (p[0] as u16 * 160 / 255) as u8;
                            canvas.put_pixel(x, y, image::Rgba([255, 64, 64, a]));
                        }
                    }
                    gray = g;
                }
                Err(e) => {
                    let t = trf(
                        self.lang,
                        "could not load this mask's raster ({err}) — starting from an empty brush canvas",
                        &[("err", &e.to_string())],
                    );
                    self.toast(ToastKind::Error, t);
                }
            }
        }
        self.mask_paint = Some(canvas);
        self.mask_dirty = true;
        self.mask_dirty_rect = None;
        self.mask_brush_gray = Some(gray);
        self.mask_brush = Some((target, false));
        self.paint_mode = true;
        self.status = tr(
            self.lang,
            "Brush mask — paint to select; 「Erase」 removes; 「Apply」 bakes it · Esc cancels",
        )
        .into();
    }

    /// Bake the brush session: the greyscale weight buffer becomes a freshly
    /// CLAIMED raster (the input file is never mutated — saved recipes and
    /// version snapshots keep rendering what they rendered), then the target
    /// mask repoints at it, or a new Bitmap mask is pushed.
    fn commit_mask_brush(&mut self) {
        let Some((target, _)) = self.mask_brush else { return };
        let Some(gray) = self.mask_brush_gray.clone() else { return };
        let Some(src) = self.src_path.clone() else { return };
        let lang = self.lang;
        if target.is_none() {
            if gray.pixels().all(|p| p[0] == 0) {
                let t = tr(lang, "nothing painted yet — drag on the image first");
                self.toast(ToastKind::Error, t.to_string());
                return;
            }
            if self.recipe.masks.len() >= 64 {
                let t = tr(lang, "mask limit reached (64) — delete one first");
                self.toast(ToastKind::Error, t.to_string());
                return;
            }
        }
        let written = autoshop::store::claim_raster(&src, "mask-brush")
            .map_err(anyhow::Error::from)
            .and_then(|p| gray.save(&p).map(|()| p).map_err(anyhow::Error::from));
        match written {
            Ok(path) => {
                let path_s = path.to_string_lossy().into_owned();
                match target {
                    Some(i) if i < self.recipe.masks.len() => {
                        self.recipe.masks[i].mask = MaskGeometry::Bitmap { path: path_s };
                        self.sel_mask = Some(i);
                        self.status = tr(lang, "mask raster updated — its adjustments now apply to the edited area").into();
                    }
                    _ => {
                        let n = self.recipe.masks.len();
                        self.recipe.masks.push(autoshop::recipe::LocalAdjustment {
                            mask: MaskGeometry::Bitmap { path: path_s },
                            name: trf(lang, "Brush {n}", &[("n", &(n + 1).to_string())]),
                            ..Default::default()
                        });
                        self.sel_mask = Some(n);
                        self.status = tr(lang, "brush mask created — pull its sliders in 「Local Masks」 (all 0 now, no visible effect yet)").into();
                    }
                }
                self.sel_component = None;
                self.dirty = true;
                self.overlay_stale = true;
                self.disarm_tools(); // ends the session and clears the canvas
            }
            Err(e) => {
                let t = trf(
                    lang,
                    "could not save the brush mask ({err})",
                    &[("err", &e.to_string())],
                );
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Bake one raster op (feather / expand / contract) on mask `i`'s bitmap:
    /// load → transform → claim a fresh raster → repoint (immutability rule —
    /// see commit_mask_brush). Synchronous at the raster's own resolution.
    fn bake_mask_raster(
        &mut self,
        i: usize,
        op: impl FnOnce(&image::GrayImage) -> image::GrayImage,
        tag: &str,
    ) {
        let Some(src) = self.src_path.clone() else { return };
        let Some(MaskGeometry::Bitmap { path }) =
            self.recipe.masks.get(i).map(|m| m.mask.clone())
        else {
            return;
        };
        let lang = self.lang;
        let done = image::open(&path)
            .map_err(anyhow::Error::from)
            .map(|img| op(&img.to_luma8()))
            .and_then(|g| {
                let p = autoshop::store::claim_raster(&src, tag).map_err(anyhow::Error::from)?;
                g.save(&p).map_err(anyhow::Error::from)?;
                Ok(p)
            });
        match done {
            Ok(p) => {
                self.recipe.masks[i].mask =
                    MaskGeometry::Bitmap { path: p.to_string_lossy().into_owned() };
                self.dirty = true;
                self.overlay_stale = true;
                self.status =
                    tr(lang, "mask raster updated — its adjustments now apply to the edited area")
                        .into();
            }
            Err(e) => {
                let t = trf(
                    lang,
                    "could not edit this mask's raster ({err})",
                    &[("err", &e.to_string())],
                );
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Refine mask `i`'s raster to FULL resolution: decode the full-res
    /// source in a worker, guided-filter the raster against it
    /// (render::refine_mask_guided), save under a fresh claim. The result is
    /// matched back by the stored raster reference — the strip may have
    /// changed while the worker ran.
    fn start_mask_refine(&mut self, i: usize) {
        if self.busy {
            return;
        }
        let Some(src) = self.src_path.clone() else { return };
        let Some(MaskGeometry::Bitmap { path }) =
            self.recipe.masks.get(i).map(|m| m.mask.clone())
        else {
            return;
        };
        let lang = self.lang;
        self.busy = true;
        self.status = tr(lang, "Refining the mask to full resolution (decoding the full-size source) …").into();
        let stored_ref = path.clone();
        self.spawn_worker(
            move || {
                let res = (|| {
                    let mask = image::open(&path)
                        .map_err(|e| anyhow::anyhow!("load mask raster {path}: {e}"))?
                        .to_luma8();
                    let full = autoshop::decode::load_image(&src)?;
                    let long = full.width().max(full.height());
                    let refined = autoshop::render::refine_mask_guided(
                        &mask,
                        &full,
                        (long / 500).max(4) as usize,
                        1e-4,
                    );
                    let out = autoshop::store::claim_raster(&src, "mask-refined")?;
                    refined.save(&out)?;
                    Ok((i, stored_ref, out))
                })();
                Msg::MaskRefined(res)
            },
            |e| Msg::MaskRefined(Err(e)),
        );
    }

    fn ensure_mask_tex(&mut self, ctx: &egui::Context) {
        // The canvas lives in the ORIGINAL frame (strokes are mapped there so
        // fill/heal edit source pixels), but the texture is blitted over the
        // After image, which has been through distortion + straighten — the
        // overlay must go through the SAME remap or every stroke displays
        // offset under a straightened view. Rebuild when the transform moved,
        // not only when the canvas was painted.
        // The overlay's remap depends on the PROFILE DISTORTION toggle (CA
        // never moves an overlay), so it joins the transform key: switching
        // it must re-blit the canvas (knot data only changes per photo, and
        // opening a photo resets the texture anyway).
        let profile_dist = self.recipe.lens_profile.distortion_on
            && !self.recipe.lens_profile.distortion.is_empty();
        let xform_now = (self.recipe.straighten_deg, self.recipe.lens_distortion, profile_dist);
        let stale_xform = self.mask_tex.is_some() && self.mask_tex_xform != xform_now;
        if self.mask_dirty || stale_xform {
            // Fast path (the common no-geometry case): the change since the
            // last upload is a known brush rect — upload ONLY that
            // sub-rectangle. Brushing used to clone and re-upload the WHOLE
            // canvas on every pointer move (at an 8192 working preview that
            // is a ~270 MB round trip per frame).
            if xform_now == (0.0, 0.0, false) && !stale_xform && self.mask_dirty {
                let rect = self.mask_dirty_rect;
                if let (Some(m), Some(tex), Some([x0, y0, x1, y1])) =
                    (&self.mask_paint, &mut self.mask_tex, rect)
                    && tex.size() == [m.width() as usize, m.height() as usize]
                    && x1 > x0
                    && y1 > y0
                    && x1 <= m.width()
                    && y1 <= m.height()
                {
                    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
                    let mut sub = Vec::with_capacity(w * h * 4);
                    for y in y0..y1 {
                        let start = ((y * m.width() + x0) * 4) as usize;
                        sub.extend_from_slice(&m.as_raw()[start..start + w * 4]);
                    }
                    tex.set_partial(
                        [x0 as usize, y0 as usize],
                        egui::ColorImage::from_rgba_unmultiplied([w, h], &sub),
                        egui::TextureOptions::LINEAR,
                    );
                    self.mask_dirty_rect = None;
                    self.mask_dirty = false;
                    return;
                }
            }
            // Geometry active: the remap is whole-frame work — mid-stroke,
            // rebuild at a bounded cadence instead of on every pointer move
            // (the stroke's final shape lands on the release frame, when the
            // pointer is no longer down).
            if !stale_xform
                && xform_now != (0.0, 0.0, false)
                && self.mask_tex.is_some()
                && ctx.input(|i| i.pointer.any_down())
                && self.mask_tex_built.elapsed() < std::time::Duration::from_millis(120)
            {
                return; // mask_dirty stays armed; retried next frame
            }
            if let Some(m) = &self.mask_paint {
                let ci = if xform_now != (0.0, 0.0, false) {
                    // Alpha-preserving RGBA twins: the RGB16 photo paths
                    // flatten transparency to opaque, which turned the whole
                    // canvas into a red wash under any active geometry.
                    let mut img = m.clone();
                    if xform_now.1 != 0.0 || profile_dist {
                        img = autoshop::render::apply_lens_geometry_rgba(
                            &img,
                            &self.recipe.lens_profile,
                            xform_now.1,
                        );
                    }
                    if xform_now.0 != 0.0 {
                        img = autoshop::render::rotate_straighten_rgba(&img, xform_now.0);
                    }
                    let rgba = img;
                    egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize],
                        rgba.as_raw(),
                    )
                } else {
                    // Common case: no geometry — zero-copy view of the canvas.
                    egui::ColorImage::from_rgba_unmultiplied(
                        [m.width() as usize, m.height() as usize],
                        m.as_raw(),
                    )
                };
                // Update in place: brushing sets mask_dirty on every pointer
                // move, and a fresh texture per frame is the allocate/free
                // churn finish_redevelop documents avoiding (set() re-uploads,
                // size changes included).
                if let Some(tex) = &mut self.mask_tex {
                    tex.set(ci, egui::TextureOptions::LINEAR);
                } else {
                    self.mask_tex =
                        Some(ctx.load_texture("paintmask", ci, egui::TextureOptions::LINEAR));
                }
                self.mask_tex_xform = xform_now;
                self.mask_tex_built = Instant::now();
            }
            self.mask_dirty_rect = None; // full upload covered everything pending
            self.mask_dirty = false;
        }
    }

    /// Brush-paint into the mask while dragging on the After image. The canvas
    /// is full-frame at preview resolution; pointer→canvas goes through the
    /// view transform so painting stays accurate at any zoom, and the brush
    /// radius converts display px → canvas px by the current pixel scale.
    fn handle_paint(&mut self, resp: &egui::Response, xf: ViewXform) {
        let brush = self.brush;
        let (dims, deg, dist) = self.geom_ctx();
        // Mask-brush session: strokes write the display canvas AND the
        // greyscale weight buffer; erase clears both.
        let erase = self.mask_brush.is_some_and(|(_, e)| e);
        let display_px = if erase {
            image::Rgba([0, 0, 0, 0])
        } else {
            image::Rgba([255, 64, 64, 160])
        };
        let gray_v: u8 = if erase { 0 } else { 255 };
        let mut gray = self.mask_brush_gray.take();
        let Some(m) = self.mask_paint.as_mut() else {
            self.mask_brush_gray = gray;
            return;
        };
        let (mw, mh) = (m.width() as f32, m.height() as f32);
        // The canvas lives in the ORIGINAL frame (fill/heal edit source
        // pixels), so pointer positions map out of the transformed view.
        let to_mask = |p: egui::Pos2| {
            let (nx, ny) = xf.to_norm(p);
            let (ox, oy) = view_norm_to_orig(nx, ny, dims, deg, &dist);
            (ox * mw, oy * mh)
        };
        // Brush radius in MASK pixels, measured through the SAME nonlinear
        // view→original chain the centers use: map the pointer and a point one
        // display-radius to its right, take their mask-space distance. Under
        // straighten/distortion a fixed display brush covers a position-
        // dependent original-frame extent — the old constant horizontal-UV
        // conversion mismatched the painted footprint there. Neutral geometry
        // degenerates to exactly the old constant.
        let brush_at = |p: egui::Pos2| {
            let a = to_mask(p);
            // Probe BOTH directions and keep the larger. `ViewXform::to_norm`
            // CLAMPS to the viewport, so a rightward-only probe mapped to the
            // same point as the centre once the pointer reached the right
            // edge: the distance went to zero, the `.max(1.0)` floor took
            // over, and a 30 px brush stamped a single mask pixel exactly
            // where the user was painting along that edge. The chain is
            // smooth, so the direction with room measures the same radius.
            let probe = |dx: f32| {
                let b = to_mask(p + egui::vec2(dx, 0.0));
                ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
            };
            probe(brush).max(probe(-brush)).max(1.0)
        };
        // Union a brush segment's bounding box (± radius, canvas px) into the
        // pending dirty rect — ensure_mask_tex's partial-upload path.
        let grow = |rect: &mut Option<[u32; 4]>, a: (f32, f32), b: (f32, f32), r: f32| {
            let r = r + 1.0;
            let n = [
                (a.0.min(b.0) - r).floor().clamp(0.0, mw) as u32,
                (a.1.min(b.1) - r).floor().clamp(0.0, mh) as u32,
                (a.0.max(b.0) + r).ceil().clamp(0.0, mw) as u32,
                (a.1.max(b.1) + r).ceil().clamp(0.0, mh) as u32,
            ];
            *rect = Some(match rect {
                Some(o) => [o[0].min(n[0]), o[1].min(n[1]), o[2].max(n[2]), o[3].max(n[3])],
                None => n,
            });
        };
        // PRIMARY button only: `dragged()` is button-agnostic, so a
        // secondary-button drag — which the caller intercepts for panning —
        // was also reported here and painted into the fill/heal/clone mask.
        if resp.dragged_by(egui::PointerButton::Primary)
            || resp.drag_started_by(egui::PointerButton::Primary)
        {
            if let Some(p) = resp.interact_pointer_pos() {
                let cur = to_mask(p);
                let r = brush_at(p);
                match self.paint_last {
                    Some(prev) => {
                        stamp_line_px(m, prev, cur, r, display_px);
                        if let Some(g) = gray.as_mut() {
                            stamp_line_gray(g, prev, cur, r, gray_v);
                        }
                    }
                    // First stroke event: connect from the PRESS ORIGIN —
                    // drag_started fires ~6 px in, and a lone dot at the
                    // current pointer dropped the stroke's lead-in.
                    None => {
                        let start = ui_press_origin(resp).map(&to_mask).unwrap_or(cur);
                        stamp_line_px(m, start, cur, r, display_px);
                        if let Some(g) = gray.as_mut() {
                            stamp_line_gray(g, start, cur, r, gray_v);
                        }
                        grow(&mut self.mask_dirty_rect, start, cur, r);
                    }
                }
                grow(&mut self.mask_dirty_rect, self.paint_last.unwrap_or(cur), cur, r);
                self.paint_last = Some(cur);
                self.mask_dirty = true;
            }
        } else {
            // A quick tap is a CLICK to egui (<6 px / <0.8 s — never a drag):
            // stamp a single dot so marking dust spots doesn't require a smear.
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let cur = to_mask(p);
                let r = brush_at(p);
                stamp_dot_px(m, cur, r, display_px);
                if let Some(g) = gray.as_mut() {
                    stamp_dot_gray(g, cur, r, gray_v);
                }
                grow(&mut self.mask_dirty_rect, cur, cur, r);
                self.mask_dirty = true;
            }
            // No stroke is in flight on any non-drag frame — clearing here
            // (not just on drag_stopped) also covers strokes interrupted by
            // Esc / hold-B / an Alt source-pick, whose drag_stopped frame
            // never reaches this handler and used to leave a stale anchor
            // that drew a full-width connecting streak into the next stroke.
            self.paint_last = None;
        }
        self.mask_brush_gray = gray;
    }

    /// Clone-stamp interaction: Alt+click picks the SOURCE point (stored in
    /// the original frame like every pixel-path coordinate); plain drags paint
    /// the target with the shared brush. The picked source stays marked with
    /// a crosshair ring so the offset is always visible.
    fn handle_clone(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        let alt = ui.input(|i| i.modifiers.alt);
        if alt {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.clicked()
                && let Some(q) = resp.interact_pointer_pos()
            {
                let (dims, deg, dist) = self.geom_ctx();
                let (nx, ny) = xf.to_norm(q);
                self.clone_src = Some(view_norm_to_orig(nx, ny, dims, deg, &dist));
                self.status = tr(
                    self.lang,
                    "Clone source sampled — brush the area to cover, then 「⎘ Clone painted area」",
                )
                .into();
            }
        } else {
            self.handle_paint(resp, xf);
        }
        if let Some((sx, sy)) = self.clone_src {
            let (dims, deg, dist) = self.geom_ctx();
            let (vx, vy) = orig_norm_to_view(sx, sy, dims, deg, &dist);
            let q = xf.to_screen(vx, vy);
            let p = ui.painter_at(xf.rect);
            // ACCENT, per the two-tier colour rule: on-canvas tool overlays
            // never wear the PILL chrome gold — a source pin sampled on skin,
            // sand or a sunset sky simply vanished in gold.
            p.circle_stroke(q, 9.0, egui::Stroke::new(2.0, ACCENT));
            p.line_segment(
                [q - egui::vec2(13.0, 0.0), q + egui::vec2(13.0, 0.0)],
                egui::Stroke::new(1.0, ACCENT),
            );
            p.line_segment(
                [q - egui::vec2(0.0, 13.0), q + egui::vec2(0.0, 13.0)],
                egui::Stroke::new(1.0, ACCENT),
            );
        }
    }

    /// Generative fill: regenerate the painted area (gpt-image), composite onto
    /// the source, save to ./out. Runs on a worker thread.
    fn start_fill(&mut self) {
        // Retouch the ACTIVE variant's pixels (a Generated variant → its origin
        // PNG), not the raw negative — otherwise a fill on the AI image would
        // splice in original pixels.
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // localise UI statuses AND the worker's result string
        let prompt = self.fill_prompt.trim().to_string();
        if prompt.is_empty() {
            self.status = tr(lang, "write what should fill the painted area").into();
            return;
        }
        let Some(mask_png) = self.export_mask_png() else {
            self.status = tr(lang, "paint the area to remove/fill first (tick Paint mask)").into();
            return;
        };
        let Some(out) = unique_out(&path, "retouch") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = if self.fill_fullres {
            tr(lang, "generative fill (full-res render)… (slow, minutes)").into()
        } else {
            tr(lang, "generative fill via gpt-image… (high quality can run minutes — progress in the status bar; ✕ Cancel to stop)").into()
        };
        let quality = ["high", "medium", "low"][self.fill_quality.min(2)].to_string();
        let full_res = self.fill_fullres;
        let edge = self.canvas_edge(); // bake at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, flag) = self.arm_cancel();
        let ptx = self.tx.clone();
        self.spawn_worker(
            move || {
                // Progress heartbeats → status bar; the cancel flag stops the
                // stream between events. Installed on THIS worker thread only.
                autoshop::generative::set_worker_hooks(Some(autoshop::generative::WorkerHooks {
                    progress: Box::new(move |m| {
                        let _ = ptx.send(Msg::Progress(epoch, m));
                    }),
                    cancel: flag,
                }));
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let mask_tmp = gui_tmp_png("fill");
                    std::fs::write(&mask_tmp, &mask_png)?;
                    let r = autoshop::generative::retouch(&cfg, &path, &mask_tmp, &prompt, &quality, full_res, &out);
                    let _ = std::fs::remove_file(&mask_tmp);
                    r?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // InPlace: refine the current rendition — bake into the active
                    // variant's base AND repoint its origin at this saved artifact
                    // so export / reverse-fit / next retouch follow the fill.
                    Ok((
                        img,
                        trf(
                            lang,
                            "filled → {path} (updated current variant)",
                            &[("path", &out.display().to_string())],
                        ),
                        out,
                        RetouchKind::InPlace,
                    ))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Heal: AI auto-detect (use_mask=false) or the painted mask (use_mask=true).
    /// Pixel retouch from surrounding real pixels; saves to ./out.
    fn start_heal(&mut self, use_mask: bool) {
        // Heal the ACTIVE variant's pixels (Generated → its origin PNG).
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // localise UI statuses AND the worker's result string
        let mask_png = if use_mask {
            match self.export_mask_png() {
                Some(b) => Some(b),
                None => {
                    self.status =
                        tr(lang, "tick Paint mask and paint the spots, then Heal painted area").into();
                    return;
                }
            }
        } else {
            None
        };
        let Some(out) = unique_out(&path, "heal") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = if use_mask {
            tr(lang, "healing painted area…").into()
        } else {
            tr(lang, "AI healing… (~10-30s)").into()
        };
        let full_res = self.heal_fullres;
        let edge = self.canvas_edge(); // bake at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, _flag) = self.arm_cancel(); // local compute: Cancel = abandon (epoch discard)
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let mask_tmp = match mask_png {
                        Some(bytes) => {
                            let t = gui_tmp_png("heal");
                            std::fs::write(&t, &bytes)?;
                            Some(t)
                        }
                        None => None,
                    };
                    let rep = autoshop::retouch::heal(&cfg, &path, mask_tmp.as_deref(), !use_mask, full_res, &out);
                    if let Some(t) = &mask_tmp {
                        let _ = std::fs::remove_file(t);
                    }
                    let rep = rep?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // InPlace: bake into the active variant's base + repoint origin.
                    Ok((
                        img,
                        trf(
                            lang,
                            "healed {n} spot(s) → {path}",
                            &[("n", &rep.spots.to_string()), ("path", &out.display().to_string())],
                        ),
                        out,
                        RetouchKind::InPlace,
                    ))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// AI denoise (SCUNet, GPU sidecar) as an ACTIVE canvas operation: run on
    /// the current variant's pixels NOW and bake the clean base in-place, so
    /// the user sees the result immediately instead of only in an export.
    /// Mirrors heal's plumbing: RetouchKind::InPlace swaps the variant base
    /// (undoable) and the develop chain keeps rendering on top.
    fn start_ai_denoise(&mut self) {
        // Denoise the ACTIVE variant's pixels (a Generated variant → its
        // origin PNG), same source rule as heal/clone.
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // localise UI statuses AND the worker's result string
        let Some(out) = unique_out(&path, "denoise") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = if self.denoise_fullres {
            tr(lang, "AI denoise (full-res)… (GPU sidecar, can take minutes; first run downloads the model)").into()
        } else {
            tr(lang, "AI denoise… (GPU sidecar on a ≤2048px working copy; first run downloads the model)").into()
        };
        let full_res = self.denoise_fullres;
        let edge = self.canvas_edge(); // show at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, _flag) = self.arm_cancel(); // sidecar compute: Cancel = abandon (epoch discard)
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let opts =
                        autoshop::denoise::DenoiseOpts::from_config(&cfg, None, 1.0);
                    autoshop::denoise::denoise_active(&opts, &path, full_res, &out)?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // InPlace: bake into the active variant's base + repoint origin.
                    Ok((
                        img,
                        trf(
                            lang,
                            "AI denoised → {path} (updated current variant)",
                            &[("path", &out.display().to_string())],
                        ),
                        out,
                        RetouchKind::InPlace,
                    ))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Run the clone stamp on a worker: painted target mask + the Alt+picked
    /// source point → `retouch::clone_stamp` (deterministic, no AI) → ./out
    /// pixel master shown in the After pane, exactly like heal.
    fn start_clone(&mut self) {
        // Clone within the ACTIVE variant's pixels (Generated → its origin PNG).
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // localise UI statuses AND the worker's result string
        let Some(src_pt) = self.clone_src else {
            self.status = tr(lang, "Alt+click to set the clone source first").into();
            return;
        };
        let Some(mask_png) = self.export_mask_png() else {
            self.status = tr(lang, "Brush the area to clone over first").into();
            return;
        };
        let Some(out) = unique_out(&path, "clone") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = tr(lang, "Cloning… (local pixel compute)").into();
        let full_res = self.clone_fullres;
        let edge = self.canvas_edge(); // bake at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, _flag) = self.arm_cancel(); // local compute: Cancel = abandon (epoch discard)
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let mask_tmp = gui_tmp_png("clone");
                    std::fs::write(&mask_tmp, &mask_png)?;
                    let rep = autoshop::retouch::clone_stamp(&path, &mask_tmp, src_pt, full_res, &out);
                    let _ = std::fs::remove_file(&mask_tmp);
                    let rep = rep?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // InPlace: a pixel transplant of the current rendition — bake it
                    // into the active variant's base + repoint origin at the artifact.
                    Ok((
                        img,
                        trf(
                            lang,
                            "Cloned {n} spot(s) → {path}",
                            &[("n", &rep.spots.to_string()), ("path", &out.display().to_string())],
                        ),
                        out,
                        RetouchKind::InPlace,
                    ))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Full-frame generative re-render via gpt-image — the OPTIONAL "let GPT
    /// directly make the picture" path. Uses the Direction text as the look
    /// prompt. Unlike Analyze (a faithful parametric recipe), this REGENERATES
    /// pixels — a creative restyle (up to ~8 MP on flexible-size models, ~1.5K
    /// on older ones). The result enters the strip as a new「AI 生成」variant
    /// (`origin = Some(out)`); its saved path is the reverse-fit ("反推配方")
    /// target that turns the look back into sliders + XMP at full resolution.
    fn start_reimagine(&mut self) {
        // Always reimagine the ORIGINAL negative (src_path), never a generated
        // variant's pixels — regenerating a rendition is the double-cook path
        // the variant model exists to avoid. Each call gets a UNIQUE ./out PNG
        // so two Generated variants never alias the same origin (which would
        // cross-wire their export / reverse-fit).
        let Some(path) = self.src_path.clone() else { return };
        if self.busy {
            return;
        }
        // First FREE ./out name (shared `unique_out` probe) so
        // delete-then-reimagine can't reuse a number whose PNG a surviving
        // variant still points at. Refuse past the cap rather than alias:
        // reusing an existing origin would cross-wire two variants'
        // export / reverse-fit.
        let Some(out) = unique_out(&path, "reimagine") else {
            self.status = tr(self.lang, "over 999 generated variants for this photo — clean up ./out first").into();
            return;
        };
        let prompt = {
            // The Reimagine section's OWN prompt field (no longer the shared
            // Direction — each prompt entry pairs with its own trigger).
            let g = self.reimagine_prompt.trim();
            if g.is_empty() {
                "Develop this photo into a finished, natural-looking edit: balanced exposure and \
                 contrast, pleasing realistic colour; keep the scene true to the original."
                    .to_string()
            } else {
                g.to_string()
            }
        };
        self.busy = true;
        let lang = self.lang;
        self.status =
            tr(lang, "AI generating… (gpt-image; high quality can run minutes — progress in the status bar; ✕ Cancel to stop; hi-res input needs a full-frame develop first)").into();
        // The PREFERENCE here, not `canvas_edge`: this is the one bake that
        // renders from src_path (never the canvas — see above) and lands as a
        // NEW variant, so nothing is composited under it and there is no
        // raster to match. Following a stale canvas — a background 1280 bake
        // clicked while the preference sat at 4096 — pinned the new variant
        // below the resolution the user asked for, behind a refusal whose own
        // remedy (save) Ctrl+S REFUSES for a generated variant.
        let edge = self.preview_edge.clamp(640, 8192);
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, flag) = self.arm_cancel();
        let ptx = self.tx.clone();
        self.spawn_worker(
            move || {
                // Progress heartbeats → status bar; the cancel flag stops the
                // stream between events. Installed on THIS worker thread only.
                autoshop::generative::set_worker_hooks(Some(autoshop::generative::WorkerHooks {
                    progress: Box::new(move |m| {
                        let _ = ptx.send(Msg::Progress(epoch, m));
                    }),
                    cancel: flag,
                }));
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    // fidelity "high" keeps it recognisably the same photo.
                    autoshop::generative::reimagine(&cfg, &path, &prompt, "high", &cfg.openai_image_quality, &out)?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    let msg = trf(
                        lang,
                        "「AI generated」variant created → {path} · keep tweaking or 「Reverse-fit」",
                        &[("path", &out.display().to_string())],
                    );
                    // NewGenerated: a whole-frame rendition → a new Generated variant.
                    Ok((img, msg, out, RetouchKind::NewGenerated))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Reverse-fit ("match"): statistically solve the develop parameters that map
    /// the SOURCE neutral onto the active「AI 生成」variant — the result lands as
    /// a new「反推」variant (base = source neutral, look in the recipe), and for a
    /// RAW the XMP sidecar is written immediately. Deterministic, no API call.
    ///
    /// The base is `source_preview`, NOT `base_preview`: after a reimagine the
    /// active variant's base IS the generated raster, and fitting a rendition
    /// onto itself would recover ~neutral. Fit must map the negative → the look.
    fn start_fit(&mut self) {
        let (Some(base), Some(tgt)) = (self.source_preview.clone(), self.fit_target())
        else {
            return;
        };
        if self.busy {
            return;
        }
        let src_path = self.src_path.clone();
        let zoned = self.zoned_fit;
        let lang = self.lang;
        self.busy = true;
        self.status = if zoned {
            tr(lang, "Reverse-fitting… (statistical fit + sky segmentation; first run downloads the model)").into()
        } else {
            tr(lang, "Reverse-fitting… (statistical fit, local compute)").into()
        };
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<(EditRecipe, String, bool)> {
                    let target = autoshop::decode::load_image(&tgt)?;
                    // The gate runs at PERSIST time (below), not up front:
                    // a pre-fit snapshot left the whole multi-minute
                    // segmentation as a race window — an explicit save
                    // landing during the fit was then overwritten
                    // unversioned. (Claimed unique rasters already made the
                    // fit itself harmless to the saved develop.)
                    // Zoned sky pass only when enabled AND the photo has a
                    // real path (the mask raster needs a home). The raster
                    // gets a FRESH unique name per fit (mask-zone-sky.png,
                    // -2, -3, …, create_new-claimed like every master name):
                    // the old fixed name was rewritten IN PLACE before the
                    // recipe landed, so a crash or a failed recipe write left
                    // the still-live saved recipe pointing at the NEW bytes —
                    // the same shared-file corruption class that once made
                    // the fit's cleanup delete 「AI select sky」's raster.
                    // With unique names the saved develop's raster is never
                    // touched, so the pass no longer needs the backup gate;
                    // superseded rasters stay on disk (tiny greyscale PNGs —
                    // v<N> snapshots freeze their own copies regardless).
                    let rep = match (zoned, &src_path) {
                        (true, Some(p)) => {
                            let cfg = autoshop::config::Config::load();
                            let seg =
                                autoshop::segment::SegmentOpts::from_config(&cfg, "sky");
                            let mask = autoshop::store::claim_raster(p, "mask-zone-sky")?;
                            autoshop::fit_zoned::fit_recipe_zoned(&base, &target, &seg, &mask)
                        }
                        _ => autoshop::fit::fit_recipe(&base, &target),
                    };
                    let mut note = trf(
                        lang,
                        "Reverse-fit done: look residual {before}→{after} · created a「Reverse-fit」variant (editable / XMP / full-res)",
                        &[
                            ("before", &format!("{:.3}", rep.err_before)),
                            ("after", &format!("{:.3}", rep.err_after)),
                        ],
                    );
                    if !rep.recipe.masks.is_empty() {
                        note.push_str(tr(
                            lang,
                            " · includes sky-zone correction (adjustable in the mask panel; XMP carries the global part only)",
                        ));
                    }
                    let mut persisted = false;
                    if let Some(p) = &src_path {
                        // Worker thread ⇒ Wait, ONE lock across gate →
                        // recipe → pixel-link clear → XMP: still the
                        // narrowest gate window, now with no other process
                        // interleaving between the halves. The
                        // identical-content skip keeps the gate from
                        // spamming versions on repeated fits.
                        let persist = autoshop::store::with_develop_lock(
                            p,
                            autoshop::store::DevelopLockMode::Wait,
                            || -> std::io::Result<()> {
                        let backed = autoshop::store::backup_saved_develop(p, Some(&rep.recipe));
                        // Persist the fit losslessly: recipe.json carries the
                        // bitmap zone masks + recolour gains the XMP cannot,
                        // so reopening the photo restores the whole fit. The
                        // snapshot above is the gate — if it FAILED, skip the
                        // persist entirely (the fit stays on the canvas with
                        // ● unsaved; Ctrl+S overwrites explicitly).
                        match &backed {
                            Ok(backed) => match autoshop::pipeline::write_recipe(p, &rep.recipe, None) {
                              Err(e) => {
                                // A failed store write must NOT discard the
                                // minutes of segmentation + fitting that
                                // already succeeded — same degrade shape as
                                // the backup-gate branch below: the fit lands
                                // on the canvas unsaved.
                                note.push_str(&trf(
                                    lang,
                                    " · NOT persisted: writing recipe.json failed ({err}) — Ctrl+S to save explicitly",
                                    &[("err", &e.to_string())],
                                ));
                              }
                              Ok(_) => {
                                persisted = true;
                                // The persisted fit is a SOURCE-based develop
                                // by construction (it maps the source neutral
                                // onto the rendition) — a stale pixels.json
                                // master link would make a reopen render the
                                // fit on baked pixels it was never computed
                                // against. Pair the write like Ctrl+S does.
                                if let Err(e) = autoshop::store::clear_pixel_source(p) {
                                    // NOT fully persisted: the pair is what a
                                    // reopen reads. Claiming success here
                                    // advanced the baseline and dropped the
                                    // stash, so the ● vanished while disk
                                    // still linked the stale master — the user
                                    // was told the work was safe when a cold
                                    // reopen would render the wrong pixels.
                                    persisted = false;
                                    note.push_str(&trf(
                                        lang,
                                        " · could not clear the old retouch-master link ({err}) — reopening may show the fit on retouched pixels; Ctrl+S repairs it",
                                        &[("err", &e.to_string())],
                                    ));
                                }
                                if autoshop::decode::is_raw(p) {
                                    // The recipe write ALONE decides the saved
                                    // state (same rule as Ctrl+S / Analyze):
                                    // an XMP failure must not collapse an
                                    // already-persisted fit into an error —
                                    // reopening WOULD restore it, so the UI
                                    // must agree that it saved.
                                    match autoshop::pipeline::write_xmp(p, &rep.recipe) {
                                        Ok(x) => {
                                            note.push_str(&format!(" · XMP → {}", x.display()));
                                        }
                                        Err(e) => {
                                            note.push_str(" · ");
                                            note.push_str(&trf(
                                                lang,
                                                "recipe saved — but the Lightroom XMP failed: {err}",
                                                &[("err", &e.to_string())],
                                            ));
                                        }
                                    }
                                }
                                if let Some(n) = backed {
                                    note.push_str(&trf(
                                        lang,
                                        " · previous save backed up as v{n}",
                                        &[("n", &n.to_string())],
                                    ));
                                }
                              }
                            },
                            Err(e) => {
                                note.push_str(&trf(
                                    lang,
                                    " · NOT persisted: backing up your existing save failed ({err}) — Ctrl+S to save explicitly",
                                    &[("err", &e.to_string())],
                                ));
                            }
                        }
                                Ok(())
                            },
                        );
                        if let Err(e) = persist {
                            note.push_str(&trf(
                                lang,
                                " · NOT persisted: the develop store could not be locked ({err}) — Ctrl+S to save explicitly",
                                &[("err", &e.to_string())],
                            ));
                        }
                    }
                    Ok((rep.recipe, note, persisted))
                })();
                Msg::Fitted(Box::new(res))
            },
            |e| Msg::Fitted(Box::new(Err(e))),
        );
    }

    /// Extract a reusable STYLE PROMPT from the before/after pair via the vision
    /// model. The result lands in the Direction box (ready to reimagine OTHER
    /// photos with the same look) and is saved to ./out/<stem>.style.txt.
    fn start_style_prompt(&mut self) {
        let (Some(base), Some(tgt)) = (self.source_preview.clone(), self.fit_target())
        else {
            return;
        };
        if self.busy {
            return;
        }
        let src_path = self.src_path.clone();
        let lang = self.lang; // localise the worker's result note
        self.busy = true;
        self.status = tr(self.lang, "Extracting style prompt… (vision, ~5-20s)").into();
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<(String, String)> {
                    let cfg = autoshop::config::Config::load();
                    let jpg = |img: &image::DynamicImage| -> anyhow::Result<Vec<u8>> {
                        let mut buf = Vec::new();
                        image::DynamicImage::ImageRgb8(img.thumbnail(768, 768).to_rgb8())
                            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)?;
                        Ok(buf)
                    };
                    let target = autoshop::decode::load_image(&tgt)?;
                    let prompt = autoshop::advisor::describe_style(&cfg, &jpg(&base)?, &jpg(&target)?)?;
                    // The side-file is a convenience copy — its write failing
                    // must not discard the vision result the user paid for.
                    // The note tells the truth either way (the old fixed
                    // message claimed the file was saved unconditionally).
                    let note = match &src_path {
                        Some(p) => {
                            let out = autoshop::pipeline::default_out(p, "style", "txt");
                            // Same protocol as every deliverable (W33): the
                            // library guard, then stage + rename — a direct
                            // fs::write truncated the previous prompt in
                            // place and followed a leaf symlink.
                            match autoshop::pipeline::guard_readonly(&out, p)
                                .and_then(|()| autoshop::pipeline::ensure_parent(&out))
                                .and_then(|()| {
                                    autoshop::render::stage_and_publish(&out, |staged| {
                                        Ok(std::fs::write(staged, &prompt)?)
                                    })
                                })
                            {
                                Ok(()) => tr(
                                    lang,
                                    "Style prompt extracted → filled into the Reimagine prompt (also saved ./out/<stem>.style.txt)",
                                )
                                .to_string(),
                                Err(e) => trf(
                                    lang,
                                    "Style prompt extracted → filled into the Reimagine prompt (saving ./out/<stem>.style.txt failed: {err})",
                                    &[("err", &e.to_string())],
                                ),
                            }
                        }
                        None => tr(
                            lang,
                            "Style prompt extracted → filled into the Reimagine prompt",
                        )
                        .to_string(),
                    };
                    Ok((prompt, note))
                })();
                Msg::Styled(Box::new(res))
            },
            |e| Msg::Styled(Box::new(Err(e))),
        );
    }

    /// The variant strip (版本条): one card per rendition — 原片 / AI 生成 /
    /// 反推 — with a live developed thumbnail. Click a card to switch (lossless;
    /// each variant keeps its own base + recipe), × to drop one. This is the
    /// selector that makes an AI develop a first-class, non-reverting version.
    fn variant_strip(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let accent = self.theme.colors().accent_text; // Copy — safe in closures
        let mut switch_to: Option<usize> = None;
        let mut delete: Option<usize> = None;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tr(lang, "Variants")).strong());
            ui.separator();
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..self.variants.len() {
                        let active = i == self.active;
                        let kind = self.variants[i].kind;
                        ui.vertical(|ui| {
                            // Developed thumbnail (or a placeholder until the
                            // variant has been developed once).
                            let resp = if let Some(t) = &self.variants[i].thumb {
                                let s = t.size_vec2();
                                let h = 52.0;
                                let w = (s.x / s.y.max(1.0) * h).clamp(30.0, 104.0);
                                let (rect, resp) =
                                    ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
                                let uv = egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                );
                                if active {
                                    ui.painter().rect_filled(
                                        rect.expand(3.0),
                                        5.0,
                                        egui::Color32::from_rgba_unmultiplied(0xc9, 0xa1, 0x4a, 46),
                                    );
                                }
                                ui.painter().image(t.id(), rect, uv, egui::Color32::WHITE);
                                if active {
                                    ui.painter().rect_stroke(
                                        rect,
                                        4.0,
                                        egui::Stroke::new(2.0, PILL),
                                    );
                                }
                                resp
                            } else {
                                ui.add_sized([64.0, 52.0], egui::Button::new("…"))
                            };
                            if resp.on_hover_text(tr(lang, "Click to switch to this variant (lossless)")).clicked() {
                                switch_to = Some(i);
                            }
                            ui.horizontal(|ui| {
                                let label = egui::RichText::new(tr(lang, kind.label())).small();
                                ui.label(if active { label.strong().color(accent) } else { label });
                                // Any variant except the sole Original can be dropped.
                                if self.variants.len() > 1
                                    && kind != VariantKind::Original
                                    && ui.small_button("×").on_hover_text(tr(lang, "Delete this variant")).clicked()
                                {
                                    delete = Some(i);
                                }
                            });
                        });
                        ui.add_space(6.0);
                    }
                });
            });
        });
        if let Some(i) = switch_to {
            self.switch_variant(i, ui.ctx());
        } else if let Some(i) = delete {
            self.delete_variant(i, ui.ctx());
        }
    }

    fn retouch_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang; // Copy — never borrows self, safe inside egui closures.
        ui.separator();
        ui.heading(tr(lang, "Retouch"));

        // Whole-image generative re-render: let gpt-image DIRECTLY produce the
        // picture (the optional "GPT makes the image" path). Distinct from
        // AI Analyze, which emits a faithful parametric recipe. The result
        // becomes a new「AI 生成」variant in the strip below; the reverse-fit
        // button then closes the loop, adding a「反推」variant whose look lives
        // in an editable recipe (full-res + XMP). No more "continue from
        // master" button — each result is its own selectable variant, so a
        // slider edit can never revert or double-cook it.
        egui::CollapsingHeader::new(tr(lang, "Reimagine (whole image)"))
            .id_salt("sec_reimagine")
            .default_open(true)
            .show(ui, |ui| {
                // This entry's OWN style prompt, right next to its trigger
                // (it used to silently borrow the Direction field at the top
                // of the panel — a prompt and its button belong together).
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.reimagine_prompt)
                            .desired_width((ui.available_width() - 130.0).max(80.0))
                            .hint_text(tr(lang, "style to repaint toward — e.g. golden-hour glow, moody film look")),
                    );
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui
                            .button(tr(lang, "✨ Generate image"))
                            .on_hover_text(tr(lang,
                                "Repaint the whole image with gpt-image, styled by the prompt on the left \
                                 (empty = a neutral finished develop). Repainted pixels = not faithful; the \
                                 result is added as an 「AI generated」 variant at the bottom and switched to, \
                                 so you can keep tweaking without reverting. Models that accept any size \
                                 (gpt-image-2) reach ~8MP, others ~1.5K. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
                            ))
                            .clicked()
                        {
                            self.start_reimagine();
                        }
                    });
                });
                // Reverse-fit the active generated variant's look back into an
                // editable recipe — how the low-res experiment becomes a
                // full-res, XMP-able「反推」variant.
                let can_fit = self.fit_target().is_some() && self.source_preview.is_some();
                if !can_fit {
                    ui.label(
                        egui::RichText::new(tr(lang,
                            "Generate an image first and stay on that variant to reverse-fit its recipe."))
                            .weak()
                            .small(),
                    );
                }
                ui.add_enabled_ui(!self.busy && can_fit, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(tr(lang, "🎛 Reverse-fit recipe → sliders/XMP"))
                            .on_hover_text(tr(lang,
                                "Statistical fit: reverse the freshly generated look into editable develop params \
                                 (local, no API cost). Sliders update (undoable), and for RAW a Lightroom XMP goes \
                                 into this photo's develop store; hit Export to render the full-resolution result.",
                            ))
                            .clicked()
                        {
                            self.start_fit();
                        }
                        if ui
                            .button(tr(lang, "📝 Extract style prompt"))
                            .on_hover_text(tr(lang,
                                "Compare the original / generated images and have the vision model write a reusable \
                                 style prompt: auto-fills the Reimagine prompt (ready to restyle other photos) and \
                                 saves ./out/<stem>.style.txt.",
                            ))
                            .clicked()
                        {
                            self.start_style_prompt();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "After generating, use 「Reverse-fit recipe」 to turn the look into sliders + XMP \
                         (the full-resolution way).",
                    ))
                    .weak()
                    .small(),
                );
            });

        // Mask tools shared by Fill AND Heal — one brush, two consumers.
        ui.horizontal(|ui| {
            let r = ui
                .checkbox(&mut self.paint_mode, tr(lang, "Paint mask"))
                .on_hover_text(tr(lang, "Brush over the area; box-select is paused while on. Shared by Fill and Heal."));
            if r.changed() && self.paint_mode {
                // Mutual exclusion lives in ONE place now (disarm_tools) —
                // this site's own hand copy once drifted and made a ticked
                // brush completely inert (dispatch tries the others first).
                self.disarm_tools();
                self.paint_mode = true; // re-arm after the sweep
            }
            if ui
                .button(tr(lang, "Clear brush"))
                .on_hover_text(tr(lang, "Wipe the painted area (shared by Fill, Heal and Stamp)"))
                .clicked()
            {
                self.clear_mask();
            }
        });
        Self::slider(ui, lang, tr(lang, "Brush size"), &mut self.brush, 4.0, 80.0, 30.0);

        egui::CollapsingHeader::new(tr(lang, "Generative Fill"))
            .id_salt("sec_fill")
            .default_open(false)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.fill_prompt)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr(lang, "what belongs there, e.g. remove the trash can, extend the sky")),
                );
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("fill_quality")
                        .selected_text(tr(lang, ["high", "medium", "low"][self.fill_quality.min(2)]))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.fill_quality, 0, tr(lang, "high"));
                            ui.selectable_value(&mut self.fill_quality, 1, tr(lang, "medium"));
                            ui.selectable_value(&mut self.fill_quality, 2, tr(lang, "low"));
                        })
                        .response
                        .on_hover_text(tr(lang, "gpt-image render quality — higher looks better and costs more per image"));
                    let src_is_raw = self
                        .active_source_path()
                        .is_some_and(|p| autoshop::decode::is_raw(&p));
                    ui.add_enabled(src_is_raw, egui::Checkbox::new(&mut self.fill_fullres, tr(lang, "Full-res")))
                        .on_hover_text(tr(lang, "Composite onto the full-sensor develop (slow, RAW only)"));
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui
                            .button(tr(lang, "Remove / Fill"))
                            .on_hover_text(tr(lang, "Regenerate ONLY the painted area from your prompt (gpt-image API call — costs per image); the rest keeps the engine's own develop"))
                            .clicked()
                        {
                            self.start_fill();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Paint the area, write what belongs there, then Remove/Fill. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
                    ))
                    .weak()
                    .small(),
                );
            });

        egui::CollapsingHeader::new(tr(lang, "Heal (pixel)"))
            .id_salt("sec_heal")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui
                            .button(tr(lang, "✦ AI heal (auto)"))
                            .on_hover_text(tr(lang, "A vision model finds small dust spots / blemishes (API call), then each is healed from surrounding REAL pixels — never generated"))
                            .clicked()
                        {
                            self.start_heal(false);
                        }
                        if ui
                            .button(tr(lang, "Heal painted area"))
                            .on_hover_text(tr(lang, "Heal the brushed area from surrounding real pixels — local compute, no API"))
                            .clicked()
                        {
                            self.start_heal(true);
                        }
                    });
                    let src_is_raw = self
                        .active_source_path()
                        .is_some_and(|p| autoshop::decode::is_raw(&p));
                    ui.add_enabled(src_is_raw, egui::Checkbox::new(&mut self.heal_fullres, tr(lang, "Full-res")))
                        .on_hover_text(tr(lang, "Heal the full-resolution develop (slow, RAW only)"));
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "AI auto-detects dust / blemishes, or paint a mask and Heal it. Pixel retouch from surrounding pixels; saved to ./out.",
                    ))
                    .weak()
                    .small(),
                );
            });

        egui::CollapsingHeader::new(tr(lang, "Clone Stamp"))
            .id_salt("sec_clone")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let label = if self.clone_mode { tr(lang, "✅ Done") } else { tr(lang, "⎘ Enter stamp") };
                    if ui
                        .button(label)
                        .on_hover_text(tr(lang, "Arm the stamp: Alt+click samples a source, the brush paints the target; your painted mask survives"))
                        .clicked()
                    {
                        let on = !self.clone_mode;
                        self.disarm_tools();
                        self.clone_mode = on;
                        // The painted canvas SURVIVES arming — it used to be
                        // wiped here with no undo, so a mask painted for
                        // Fill/Heal died the moment the user peeked at the
                        // stamp. The explicit Clear button owns wiping.
                        if on {
                            self.status =
                                tr(lang, "Stamp: Alt+click to set the source → brush the target area → 「⎘ Clone painted area」").into();
                        }
                    }
                    let src_is_raw = self
                        .active_source_path()
                        .is_some_and(|p| autoshop::decode::is_raw(&p));
                    ui.add_enabled(src_is_raw, egui::Checkbox::new(&mut self.clone_fullres, tr(lang, "Full-res")))
                        .on_hover_text(tr(lang, "Clone on the full-resolution develop (slow, RAW only)"));
                    ui.add_enabled_ui(!self.busy && self.clone_mode, |ui| {
                        if ui
                            .button(tr(lang, "⎘ Clone painted area"))
                            .on_hover_text(tr(lang, "Copy the sampled source over the brushed area verbatim (feathered edges, no tone matching) — local compute"))
                            .clicked()
                        {
                            self.start_clone();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Photoshop-style clone stamp: Alt+click to sample a source (cross marker), brush the area to \
                         cover, and pixels are carried over as-is from the source (feathered edges, no tone matching). \
                         Local compute, saves a ./out pixel master.",
                    ))
                    .weak()
                    .small(),
                );
            });
    }
}

impl eframe::App for AutoshopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_workers(ctx);
        // Window-close guard: the unsaved-edit protection (● + nav_stash)
        // used to stop at photo switching — the title-bar ✕ dropped the open
        // photo's uncommitted develop AND every stashed one with no prompt.
        // An in-app confirm layer (egui Window, never an OS dialog) offers
        // save-all / discard / cancel; the guard re-checks on the way out, so
        // both quit buttons work by making the state genuinely clean.
        if ctx.input(|i| i.viewport().close_requested()) {
            // A typed-but-uncommitted mask rename must COUNT as unsaved
            // work: commit it before the dirty check below — an otherwise
            // clean recipe used to close without a prompt and the rename
            // died with the window (U10).
            self.commit_mask_name_buf();
            // Unsaved covers PIXELS too: a baked retouch whose master isn't
            // recorded in the store yet dies with the window exactly like an
            // unsaved slider move (the master PNG survives, its linkage not).
            let pixels_unsaved = self.src_path.as_deref().is_some_and(|p| {
                let origin = self.active_variant().and_then(|v| v.origin.clone());
                // Both directions count (gained OR dropped master).
                let recorded = autoshop::store::read_pixel_source(p).map(|(q, _)| q);
                !same_master_opt(recorded.as_deref(), origin.as_deref())
            });
            let unsaved_open = self.src_path.is_some()
                && (dirty_vs(&self.recipe, &self.saved_recipe) || pixels_unsaved);
            if self.busy {
                // A running export / retouch / paid AI generation dies with
                // the process — the ✕ used to bypass every guard mid-flight.
                // Block and say why; close again once the worker lands.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                let t = tr(self.lang, "An operation is still running — wait for it to finish, then close").to_string();
                self.toast(ToastKind::Error, t);
            } else if !self.discard_requested
                && (unsaved_open
                    || !self.nav_stash.is_empty()
                    || self.inactive_dirty_variants() > 0)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_quit = true;
                // Surrender any widget focus NOW: the layer's Enter-saves
                // default is gated on "no widget focused", and a text field
                // that kept focus from before the close request silently
                // disabled the promised keyboard default. (Tab inside the
                // dialog can still focus its buttons — the gate then
                // correctly routes Enter to the focused control.)
                if let Some(id) = ctx.memory(|m| m.focused()) {
                    ctx.memory_mut(|m| m.surrender_focus(id));
                }
            }
        }
        if self.confirm_quit {
            self.confirm_quit_layer(ctx);
        }
        // Hover-to-preview is frame-scoped: take last frame's target; the mask
        // list re-sets it below if the cursor is still on a row. The diff is
        // checked right before the overlay refresh at the end of update().
        let hover_prev = self.hover_mask.take();

        // Tab's egui focus traversal fired last frame despite the consume (see
        // `defocus_next`): drop that focus now so the panel toggle doesn't
        // leave a surprise-focused widget eating every later shortcut.
        if std::mem::take(&mut self.defocus_next)
            && let Some(id) = ctx.memory(|m| m.focused())
        {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        // Global shortcuts, in THREE tiers (all skipped while the
        // quit-confirm layer is up — no key must mutate the very state the
        // user is deciding whether to save; the Settings / ⌨ windows are
        // keyboard-modal too, D13):
        //  * Esc while a transient window is up dismisses it.
        //  * Ctrl+O/E/S have no text-editing meaning, so they work even while
        //    a widget holds focus — focusing a prompt used to silently kill
        //    Save/Open/Export until the user clicked elsewhere.
        //  * Everything else (plain letters, arrows, Ctrl+Z/Y) stays gated on
        //    "no widget focused": typing must type, and a text field's own
        //    undo owns Ctrl+Z there.
        // Ctrl+Z/Y = undo/redo, Ctrl+O = open, Ctrl(+Shift)+E = export,
        // Ctrl+S = save XMP, ←/→ = walk the gallery (crop armed: nudge the
        // box), R = crop, Enter = commit crop, W/Q/K/M = LR tool keys,
        // \ / Y = compare, [ ] = brush size, Tab = hide the side panels —
        // the keyboard grammar of every desktop photo editor.
        let transient_open = self.show_settings || self.show_shortcuts;
        if !self.confirm_quit && transient_open {
            let sheet_open = self.show_shortcuts;
            let (esc, sheet_key) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                    // F1 / ? stay live while the sheet is open — they are its
                    // advertised toggle, and the focused-none tier below is
                    // skipped entirely while a transient is up (L19).
                    sheet_open
                        && (i.consume_key(egui::Modifiers::NONE, egui::Key::F1)
                            || i.consume_key(egui::Modifiers::NONE, egui::Key::Questionmark)
                            || i.consume_key(egui::Modifiers::SHIFT, egui::Key::Questionmark)),
                )
            });
            if esc || sheet_key {
                // Cheat-sheet first — it is the topmost transient.
                if self.show_shortcuts {
                    self.show_shortcuts = false;
                } else {
                    self.show_settings = false;
                }
            }
        }
        if !self.confirm_quit && !transient_open {
            let (mut do_open, mut do_export, mut do_xmp) = (false, false, false);
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::O) { do_open = true; }
                // LR's export key is Ctrl+Shift+E — consumed FIRST (the
                // exact-match consume would leave it dead behind plain
                // Ctrl+E, which stays as the historical alias, D13).
                if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::E) { do_export = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::E) { do_export = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S) { do_xmp = true; }
            });
            if do_open && !self.busy
                && let Some(path) = photo_file_dialog()
            {
                self.selected = None;
                self.open_path(path);
            }
            if do_export && self.src_path.is_some() && !self.busy {
                self.start_export();
            }
            if do_xmp && self.src_path.is_some() && !self.busy {
                // The pending-rename flush lives at save_xmp's head now (it
                // fires while a TextEdit holds focus either way) — the Save
                // develop BUTTON takes the same entry point (U10).
                self.save_xmp();
            }
        }
        if !self.confirm_quit && !transient_open && ctx.memory(|m| m.focused()).is_none() {
            let (mut do_undo, mut do_redo) = (false, false);
            let (mut do_escape, mut do_overlay, mut do_clip) = (false, false, false);
            let (mut do_cheatsheet, mut do_crop, mut do_panels) = (false, false, false);
            let (mut do_crop_commit, mut do_crop_grid) = (false, false);
            let (mut do_wb, mut do_heal, mut do_brush) = (false, false, false);
            let (mut do_linear, mut do_radial) = (false, false);
            let (mut do_before, mut do_compare) = (false, false);
            let mut crop_nudge = (0.0f32, 0.0f32);
            let mut nav: i32 = 0;
            let mut brush_delta: f32 = 0.0;
            // [ / ] belong to the active brush tool; when none is armed the
            // keys stay unconsumed (free for egui / future bindings).
            let brush_tool = self.paint_mode || self.clone_mode;
            let crop_tool = self.crop_mode;
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) { do_redo = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) { do_redo = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) { do_undo = true; }
                if crop_tool {
                    // Crop owns the arrows while armed (keyboard-only
                    // geometry, D13); gallery walking resumes on exit.
                    let s = if i.modifiers.shift { 10.0 } else { 1.0 };
                    for (key, dx, dy) in [
                        (egui::Key::ArrowRight, 1.0, 0.0),
                        (egui::Key::ArrowLeft, -1.0, 0.0),
                        (egui::Key::ArrowDown, 0.0, 1.0),
                        (egui::Key::ArrowUp, 0.0, -1.0),
                    ] {
                        if i.consume_key(egui::Modifiers::NONE, key)
                            || i.consume_key(egui::Modifiers::SHIFT, key)
                        {
                            crop_nudge.0 += dx * s;
                            crop_nudge.1 += dy * s;
                        }
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) { do_crop_commit = true; }
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) { nav = 1; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) { nav = -1; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) { do_escape = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::O) {
                    if crop_tool { do_crop_grid = true; } else { do_overlay = true; }
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::J) { do_clip = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::R) { do_crop = true; }
                // LR tool keys (D13): W / Q / K and M / Shift+M (the Shift
                // variant consumed first — exact match leaves it dead after).
                if i.consume_key(egui::Modifiers::NONE, egui::Key::W) { do_wb = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Q) { do_heal = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::K) { do_brush = true; }
                if i.consume_key(egui::Modifiers::SHIFT, egui::Key::M) { do_radial = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::M) { do_linear = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Backslash) { do_before = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Y) { do_compare = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) { do_panels = true; }
                if brush_tool {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket) { brush_delta = -4.0; }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket) { brush_delta = 4.0; }
                }
                // F1 / ? — the cheat-sheet (Shift+/ produces ? on most layouts).
                if i.consume_key(egui::Modifiers::NONE, egui::Key::F1)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Questionmark)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::Questionmark)
                {
                    do_cheatsheet = true;
                }
            });
            // Mirror the ↶/↷ BUTTONS exactly — `add_enabled(ready &&
            // !stack.is_empty())` with `ready = src_path.is_some() && !busy`.
            // Un-gated, Ctrl+Z during a retouch swapped the base the landing
            // result is applied over and, since every pixel undo now writes a
            // status, wiped the only liveness the bar was showing for a
            // worker that sends no progress. But the BUSY half alone made the
            // refusal lie: Ctrl+Z during a folder scan with no photo open
            // drew "busy — …unlock when the current task finishes" for an
            // unlock that would never come (no photo, no history). Only a
            // press that WOULD have acted earns a refusal; the rest stay the
            // silent no-ops the disabled buttons are.
            let armed_undo = do_undo && self.src_path.is_some() && !self.undo_stack.is_empty();
            let armed_redo = do_redo && self.src_path.is_some() && !self.redo_stack.is_empty();
            if self.busy {
                if armed_undo || armed_redo {
                    let t = tr(
                        self.lang,
                        "busy — undo and redo unlock when the current task finishes",
                    );
                    self.toast(ToastKind::Error, t);
                }
            } else {
                if armed_undo { self.undo(ctx); }
                if armed_redo { self.redo(ctx); }
            }
            // R mirrors the crop button exactly (incl. the one-tool-at-a-time
            // disarms); no photo → nothing to crop.
            if do_crop && self.src_path.is_some() {
                let on = !self.crop_mode;
                self.disarm_tools();
                self.crop_mode = on;
            }
            // Enter commits the crop (LR, D13): the box is already live in
            // recipe.crop — committing is dropping the grab and disarming.
            if do_crop_commit {
                self.crop_drag = None;
                self.crop_mode = false;
            }
            if crop_nudge != (0.0, 0.0)
                && let Some(c) = self.recipe.crop
            {
                // Keyboard twin of the move drag (D13): 0.5% of the frame per
                // press (Shift ×10 via the multiplier above), clamped
                // in-frame; a full-frame box has no room and stays put — and
                // NO box at all is a no-op: materialising an identity crop
                // from None lit a false ● and stamped HasCrop into the next
                // XMP (M19).
                let c0 = [c.left, c.top, c.right, c.bottom];
                let (w, h) = (c0[2] - c0[0], c0[3] - c0[1]);
                let nl = (c0[0] + crop_nudge.0 * 0.005).clamp(0.0, 1.0 - w);
                let nt = (c0[1] + crop_nudge.1 * 0.005).clamp(0.0, 1.0 - h);
                let next = Some(autoshop::recipe::Crop {
                    left: nl,
                    top: nt,
                    right: nl + w,
                    bottom: nt + h,
                });
                if self.recipe.crop != next {
                    self.recipe.crop = next;
                    self.dirty = true; // histogram/clipping follow the crop
                }
            }
            if do_crop_grid {
                self.crop_grid = (self.crop_grid + 1) % 3;
            }
            // LR tool keys: each mirrors its own button exactly
            // (disarm-then-arm; a second press exits the tool).
            if do_wb && self.src_path.is_some() {
                let on = !self.wb_picking;
                self.disarm_tools();
                self.wb_picking = on;
                if on {
                    self.status = tr(self.lang, "WB eyedropper: click a spot that should be neutral grey/white").into();
                }
            }
            if do_heal && self.src_path.is_some() {
                let on = !self.clone_mode;
                self.disarm_tools();
                self.clone_mode = on;
            }
            if do_brush && self.src_path.is_some() {
                let on = !self.paint_mode;
                self.disarm_tools();
                self.paint_mode = on;
            }
            if do_linear && self.src_path.is_some() {
                let armed = matches!(self.placing_mask, Some((MaskKind::Linear, PlaceTarget::NewMask)));
                self.disarm_tools();
                if !armed {
                    self.placing_mask = Some((MaskKind::Linear, PlaceTarget::NewMask));
                    self.status = tr(self.lang, "Drag on the image to draw a linear gradient (start fully applied → end unaffected; Shift = axis lock)").into();
                }
            }
            if do_radial && self.src_path.is_some() {
                let armed = matches!(self.placing_mask, Some((MaskKind::Radial, PlaceTarget::NewMask)));
                self.disarm_tools();
                if !armed {
                    self.placing_mask = Some((MaskKind::Radial, PlaceTarget::NewMask));
                    self.status = tr(self.lang, "Drag on the image to draw a radial (elliptical) area").into();
                }
            }
            // \ latches the Before view, Y flips the comparison layout (LR).
            if do_before {
                self.before_latch = !self.before_latch;
            }
            if do_compare {
                self.view_mode = if self.view_mode == ViewMode::SideBySide {
                    ViewMode::AfterOnly
                } else {
                    ViewMode::SideBySide
                };
            }
            if do_panels {
                self.panels_hidden = !self.panels_hidden;
                self.defocus_next = true; // undo egui's Tab focus traversal next frame
            }
            if brush_delta != 0.0 {
                self.brush = (self.brush + brush_delta).clamp(4.0, 80.0);
            }
            if do_cheatsheet {
                self.show_shortcuts = !self.show_shortcuts;
            }
            // Esc leaves whatever on-image tool is active (the universal
            // editor exit) — the transient windows own their Esc in the
            // pre-tier above; painted canvases/samples stay for resuming.
            if do_escape
                && (self.tool_armed() || self.region_drag.is_some() || self.before_latch)
            {
                // disarm_tools also kills the transient gesture anchors — a
                // grab or stroke surviving Esc used to hijack the next drag.
                // The \-latched Before view exits here too: it locks every
                // tool out, and Esc is the universal way back (M21).
                self.disarm_tools();
                self.region_drag = None;
                self.before_latch = false;
                self.status = tr(self.lang, "Exited the current tool (Esc)").into();
            }
            if do_overlay {
                self.show_mask_overlay = !self.show_mask_overlay;
                self.overlay_stale = true;
            }
            if do_clip {
                // Instant: rebuilt from the retained frame, no redevelop (J).
                self.toggle_clipping(ctx);
            }
            if nav != 0 && !self.busy && !self.gallery.is_empty() {
                // Nothing selected yet: either arrow ENTERS the gallery at its
                // first photo (the old Right-arrow default skipped index 0).
                let next = match self.selected {
                    Some(i) => (i as i32 + nav).clamp(0, self.gallery.len() as i32 - 1),
                    None => 0,
                };
                if Some(next as usize) != self.selected {
                    // Keyboard walking must keep the highlight on screen — the
                    // gallery scrolls to it next frame (clicks don't set this:
                    // a clicked row is visible by definition).
                    self.gallery_scroll_to = Some(next as usize);
                    self.open_gallery_index(next as usize);
                }
            }
        }

        // Drag & drop: dropping a photo opens it, a folder opens the library.
        // Ignored entirely while the quit-confirm layer is up — a drop must
        // not mutate the very state the user is deciding whether to save
        // (the shortcut block is gated on the same condition).
        let dropped: Vec<PathBuf> = if self.confirm_quit {
            Vec::new()
        } else {
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect())
        };
        let n_dropped = dropped.len();
        if let Some(p) = dropped.into_iter().next() {
            if self.busy {
                self.toast(
                    ToastKind::Error,
                    tr(self.lang, "busy — wait for the current task to finish before opening"),
                );
            } else if p.is_dir() {
                self.open_folder(p);
                if n_dropped > 1 {
                    // EVERY branch discloses the leftovers — silently eating
                    // them read as "it handled them all" (the photo branch
                    // alone used to say so).
                    self.toast(
                        ToastKind::Error,
                        trf(
                            self.lang,
                            "opened the first folder — {n} more dropped item(s) ignored",
                            &[("n", &(n_dropped - 1).to_string())],
                        ),
                    );
                }
            } else if is_photo_path(&p) {
                self.selected = None;
                self.open_path(p);
                if n_dropped > 1 {
                    // Silently eating the rest read as "it opened them all".
                    self.toast(
                        ToastKind::Error,
                        trf(
                            self.lang,
                            "opened the first photo — {n} more ignored (drop their folder to browse them all)",
                            &[("n", &(n_dropped - 1).to_string())],
                        ),
                    );
                }
            } else {
                let msg = if n_dropped > 1 {
                    trf(
                        self.lang,
                        "unsupported file type: {path} — {n} more dropped item(s) ignored",
                        &[
                            ("path", &p.display().to_string()),
                            ("n", &(n_dropped - 1).to_string()),
                        ],
                    )
                } else {
                    trf(self.lang, "unsupported file type: {path}", &[("path", &p.display().to_string())])
                };
                self.toast(ToastKind::Error, msg);
            }
        }

        // Window title mirrors the open photo (send only on change).
        let title = match &self.src_path {
            Some(p) => format!(
                "{} — Autoshop",
                p.file_name().and_then(|s| s.to_str()).unwrap_or("photo")
            ),
            None => "Autoshop".to_string(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            let lang = self.lang;
            // ONE wrapped row of ACTIONS only (UX batch): settings that used
            // to live here moved to where they are used — export options into
            // the Develop panel's Export section, the AI direction/refine/
            // style trio into the AI section. The row wraps, never clips (the
            // "shrink the window and lose the buttons" bug); wrapping only
            // works between ATOMIC widget allocations, so the enabled gating
            // stays per-widget (ui.add_enabled), and groups are fenced with
            // add_space (a separator that wraps to a line start orphans).
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new(tr(lang, "Open photo…")))
                    .on_hover_text(tr(lang, "Ctrl+O · or drag a file into the window"))
                    .clicked()
                    && let Some(path) = photo_file_dialog()
                {
                    self.selected = None; // a one-off file isn't a gallery selection
                    self.open_path(path);
                }
                // AI Analyze moved INTO the AI section, onto the same row as
                // the Direction prompt it consumes (user feedback: a trigger
                // stranded in the toolbar, far from its input, reads as
                // unrelated — every prompt entry now carries its own button).
                let ready = self.src_path.is_some() && !self.busy;
                if ui
                    .add_enabled(ready, egui::Button::new(tr(lang, "Reset")))
                    .on_hover_text(tr(lang, "Back to this photo's fresh-open look: sliders neutral on the camera-matched base (one undo brings it back)"))
                    .clicked()
                {
                    // Reset = the fresh-open look: sliders neutral on this
                    // photo's camera-matched base. RE-STAMP the open knots
                    // rather than keeping the canvas curve — a legacy save
                    // deliberately carries none, and preserving that emptiness
                    // made Reset stay dark on every previously-edited photo.
                    // Only a GENERATED variant keeps no curve (its pixels
                    // carry the look); an InPlace retouch master is a NEUTRAL
                    // develop that still needs the calibration on top —
                    // stripping there turned Reset into "go dark".
                    let generated = self.active_is_generated();
                    let base_curve =
                        if generated { Vec::new() } else { self.photo_knots.clone() };
                    // The lens profile is the same kind of calibration: Reset
                    // re-stamps the photo's own (a Generated master's pixels
                    // already carry the corrections — none re-applied).
                    let lens_profile =
                        if generated { Default::default() } else { self.photo_lens.clone() };
                    // The as-shot anchor follows the SAME generated strip:
                    // baked pixels carry their WB, so an absolute-Kelvin
                    // claim over them would be false.
                    let (as_shot_k, as_shot_tint) = match self.photo_as_shot {
                        Some((k, t)) if !generated => (Some(k), Some(t)),
                        _ => (None, None),
                    };
                    self.recipe = EditRecipe {
                        base_curve,
                        lens_profile,
                        as_shot_k,
                        as_shot_tint,
                        ..EditRecipe::default()
                    };
                    self.region = None;
                    self.dirty = true;
                    // The old AI rationale/verdict describe the recipe Reset
                    // just discarded — showing them under neutral sliders lies.
                    self.resync_recipe_display();
                }
                ui.add_space(12.0);
                if ui
                    .add_enabled(ready && !self.undo_stack.is_empty(), egui::Button::new(tr(lang, "↶ Undo")))
                    .on_hover_text(tr(lang, "Ctrl+Z · undo the last edit"))
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.undo(&ctx);
                }
                if ui
                    .add_enabled(ready && !self.redo_stack.is_empty(), egui::Button::new(tr(lang, "↷ Redo")))
                    .on_hover_text(tr(lang, "Ctrl+Y · redo the undone edit"))
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.redo(&ctx);
                }
                ui.add_space(12.0);
                // View mode: side-by-side vs a full-width edit (hold B =
                // compare). ◫ lives in egui's bundled fonts — the old ⿲
                // (U+2FF2) only rendered when the OPTIONAL CJK fallback font
                // loaded, tofu otherwise.
                ui.selectable_value(&mut self.view_mode, ViewMode::SideBySide, tr(lang, "◫ Compare"))
                    .on_hover_text(tr(lang, "Before/After side by side"));
                ui.selectable_value(&mut self.view_mode, ViewMode::AfterOnly, tr(lang, "⬛ Single"))
                    .on_hover_text(tr(lang, "The edit fills the canvas; hold B to quickly compare the original"));
                ui.add_space(12.0);
                // Delivery ACTIONS (their settings live in the Develop panel's
                // Export section; the hover echoes the current delivery state
                // so it stays glanceable without a toolbar row of combos).
                let summary = self.export_summary(lang);
                if ui
                    .add_enabled(ready, egui::Button::new(tr(lang, "Export")))
                    .on_hover_text(format!(
                        "{}\n{summary}",
                        tr(lang, "Ctrl+Shift+E · full-resolution render to ./out (follows the current variant's pixels); settings in the Export section")
                    ))
                    .clicked()
                {
                    self.start_export();
                }
                if ui
                    .add_enabled(ready, egui::Button::new(tr(lang, "Download…")))
                    .on_hover_text(format!(
                        "{}\n{summary}",
                        tr(lang, "Download… = save the full-resolution export to a path you choose")
                    ))
                    .clicked()
                {
                    let ext = if self.save_jpeg { "jpg" } else { "tif" };
                    // Suggest a name from the ACTIVE variant's pixel source (a
                    // Generated variant → its reimagine stem), matching what
                    // Export writes; the rendered pixels already follow it.
                    let src = self.active_source_path();
                    let stem = src
                        .as_deref()
                        .and_then(|p| p.file_stem())
                        .and_then(|s| s.to_str())
                        .unwrap_or("photo")
                        .to_string();
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter(ext, &[ext])
                        .set_file_name(format!("{stem}.developed.{ext}"))
                        .save_file()
                    {
                        self.start_render_to(p);
                    }
                }
                if ui
                    .add_enabled(ready, egui::Button::new(tr(lang, "Save develop")))
                    .on_hover_text(tr(lang, "Ctrl+S · save this photo's develop (recipe + a Lightroom/ACR XMP for RAW; a baked retouch master is linked so reopening restores it) to your develop store"))
                    .clicked()
                {
                    self.save_xmp();
                }
                ui.add_space(12.0);
                if ui.button(tr(lang, "⚙ Settings")).on_hover_text(tr(lang, "AI provider / model / API key")).clicked() {
                    // Reload the form only on the closed→open edge — reloading
                    // while open wiped everything already typed (incl. keys).
                    // Toggle semantics, matching ⌨ next door.
                    if !self.show_settings {
                        self.load_settings_form();
                    }
                    self.show_settings = !self.show_settings;
                }
                if ui.button("⌨").on_hover_text(tr(lang, "Keyboard shortcuts (F1 / ?)")).clicked() {
                    self.show_shortcuts = !self.show_shortcuts;
                }
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                    // Armed by every cancellable long-run: retouch/generative
                    // workers (which poll the flag and really stop), local
                    // compute, and AI analyze — the last two are ABANDON-only,
                    // which the hover text says rather than promising a
                    // checkpoint they do not have.
                    if self.gen_cancel.is_some()
                        && ui
                            .small_button(tr(self.lang, "✕ Cancel"))
                            .on_hover_text(tr(
                                self.lang,
                                "Stop waiting: the app unblocks now and the late result is discarded. A generative call halts at its next checkpoint; an AI analyze keeps running (and billing) until it finishes or times out",
                            ))
                            .clicked()
                    {
                        self.cancel_generative();
                    }
                }
                // The unsaved marker: canvas differs from the saved develop
                // (base_curve excluded — calibration is not an edit, dirty_vs)
                // OR the canvas pixels differ from the recorded baked master
                // (`pixels_on_disk` mirrors pixels.json — an unsaved heal is
                // unsaved work even under an untouched recipe). It sits FIRST
                // — before the batch bar and the status text — so neither can
                // ever push it out of view at narrow widths.
                let pixels_dirty = self.src_path.is_some()
                    && !same_master_opt(
                        self.active_variant().and_then(|v| v.origin.as_deref()),
                        self.pixels_on_disk.as_deref(),
                    );
                if self.src_path.is_some()
                    && (dirty_vs(&self.recipe, &self.saved_recipe) || pixels_dirty)
                {
                    ui.label(
                        egui::RichText::new(tr(self.lang, "● unsaved"))
                            .color(ui.visuals().warn_fg_color),
                    )
                    .on_hover_text(tr(
                        self.lang,
                        "Edits (or a baked retouch) differ from your saved develop — Ctrl+S saves; switching photos keeps them for this session only",
                    ));
                }
                // Live batch-render progress, beside the spinner (its old home
                // at the START of the toolbar shifted every control right by
                // ~160px exactly while the user was waiting).
                if let Some((done, total)) = self.batch_progress {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_width(150.0)
                            .text(trf(self.lang, "Batch {done}/{total}",
                                &[("done", &done.to_string()), ("total", &total.to_string())])),
                    );
                }
                // Long messages (paths, batch reports) must clip, not blow the
                // panel wide; the full text is one hover away.
                ui.add(egui::Label::new(&self.status).truncate())
                    .on_hover_text(&self.status);
            });
        });

        // Variant strip — sits directly above the status bar (registered after
        // it so it stacks on top), only when a photo is open. The selector for
        // 原片 / AI 生成 / 反推 renditions.
        if self.src_path.is_some() {
            egui::TopBottomPanel::bottom("variants")
                .exact_height(96.0)
                .show(ctx, |ui| {
                    self.variant_strip(ui);
                });
        }

        // Left-most: the library gallery (folder browse + thumbnails), then
        // the develop controls. Tab collapses both for an edge-to-edge canvas
        // (LR's panel-hiding grammar); all state lives on, only layout skips.
        if !self.panels_hidden {
            egui::SidePanel::left("gallery").default_width(240.0).show(ctx, |ui| {
                self.gallery_panel(ui);
            });

            egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.src_path.is_some() {
                        self.develop_panel(ui);
                        self.retouch_panel(ui);
                        // Verdict + rationale moved to the TOP of develop_panel
                        // ("AI verdict" section) — the output of the headline
                        // feature no longer hides below every adjustment section.
                    } else {
                        ui.label(tr(self.lang, "No photo open."));
                    }
                });
            });
        }

        // Re-develop AFTER the controls are read (so this frame reflects edits).
        // The preview build runs on a SINGLE background worker (latest wins), so
        // egui's update loop never blocks on it — the old synchronous path froze
        // the UI for the whole develop (100-300 ms at 2560/4096, and 0.6-1.2 s
        // once a v0.8 zoned colour mask was present). While a frame is in flight,
        // further edits only set `dirty`; the completion handler discards a stale
        // frame and this block re-dispatches the newest recipe next tick, so
        // fast drags coalesce to the worker's throughput without a render storm.
        // The Before pane mirrors the canvas recipe's base_curve (calibration,
        // not an edit — the ● logic ignores it, but the compare must not):
        // Reset re-stamping a legacy photo, a paste, or a restore would
        // otherwise leave the compare against a stale starting point. One
        // cheap LUT-only develop, and only when the curve actually changed.
        if self.src_path.is_some()
            // NOT base-present: an InPlace master legitimately renders Before
            // under the canvas curve, and gating on v.base froze its Before
            // against Reset / undo / paste forever. A Generated canvas
            // recipe's curve is empty by construction, so this compare is
            // simply quiet there.
            && !self.active_is_generated()
            && self.recipe.base_curve != self.before_curve
            && let Some(b) = self.base_preview.clone()
        {
            let curve = self.recipe.base_curve.clone();
            self.set_before(ctx, &b, &curve);
        }
        // A held-still pointer still needs a repaint to receive the result.
        if self.dirty && !self.develop_inflight {
            self.start_redevelop();
        }
        if self.develop_inflight {
            ctx.request_repaint();
        }
        // The mask coverage overlay follows develop / selection / toggle /
        // hover (a changed hover target includes "left the list entirely").
        if self.hover_mask != hover_prev {
            self.overlay_stale = true;
        }
        if std::mem::take(&mut self.overlay_stale) {
            self.refresh_mask_overlay(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // Empty state: a real landing surface instead of a blank canvas.
            if self.src_path.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.32);
                    ui.heading("Autoshop");
                    ui.label(egui::RichText::new(tr(self.lang, "AI auto-develop · RAW develop")).weak());
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        // Center the button pair by padding half the leftover width.
                        let w = 300.0;
                        ui.add_space((ui.available_width() - w).max(0.0) * 0.5);
                        if ui.button(tr(self.lang, "📷 Open photo…  (Ctrl+O)")).clicked()
                            && let Some(p) = photo_file_dialog()
                        {
                            self.open_path(p);
                        }
                        if ui.button(tr(self.lang, "🗂 Open folder…")).clicked()
                            && let Some(d) = rfd::FileDialog::new().pick_folder()
                        {
                            self.open_folder(d);
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(tr(self.lang, "or drag a RAW / image straight into the window · drag & drop anywhere"))
                            .weak()
                            .small(),
                    );
                });
                return;
            }

            // Fit BOTH dimensions (max_width alone lets a portrait overflow the
            // panel). The displayed rect is what paint/box-select map against,
            // so sizing here never changes their coordinate math.
            let avail = ui.available_size() - egui::vec2(0.0, 22.0); // room for the caption row
            // Hold B to flash the source in place — the Lightroom compare
            // gesture (\ latches it until pressed again, D13). Focus-gated
            // like every other shortcut: typing a "b" into Direction must
            // not flash the Before mid-word.
            let comparing = self.before_latch
                || (ctx.memory(|m| m.focused()).is_none()
                    && ctx.input(|i| i.key_down(egui::Key::B)));

            match self.view_mode {
                ViewMode::SideBySide => {
                    let half = (avail.x - 16.0) * 0.5;
                    let uv = self.view_uv(); // same window for both panes (synced zoom)
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(tr(self.lang, "Before (source)"))
                                    .weak()
                                    .small(),
                            );
                            if let Some(t) = &self.before_tex {
                                let size = t.size_vec2();
                                let vis = egui::vec2(uv.width() * size.x, uv.height() * size.y);
                                let disp = fit_in(vis, half, avail.y);
                                let (rect, _) =
                                    ui.allocate_exact_size(disp, egui::Sense::hover());
                                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
                            }
                        });
                        ui.separator();
                        ui.vertical(|ui| self.after_view(ui, half, avail.y, comparing));
                    });
                }
                ViewMode::AfterOnly => {
                    ui.vertical(|ui| self.after_view(ui, avail.x, avail.y, comparing));
                }
            }
        });

        // Settings window (provider / model / API keys). A local `open` avoids a
        // double &mut self borrow (Window::open vs the closure that reads self).
        if self.show_settings {
            let mut open = true;
            // Scroll inside the window, capped below the screen height: the
            // provider sections outgrow a small display, and without a scroll
            // area the 保存 button ends up unreachable off-screen.
            let max_h = ctx.screen_rect().height() * 0.85;
            egui::Window::new(tr(self.lang, "⚙ Settings"))
                // Fixed id: the TITLE now varies with the language, and egui
                // keys window state (position, size) off the id.
                .id(egui::Id::new("settings_window"))
                .collapsible(false)
                .resizable(false)
                .default_width(480.0)
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(max_h)
                        .show(ui, |ui| self.settings_ui(ui));
                });
            if !open {
                self.show_settings = false;
            }
        }

        // Keyboard cheat-sheet (F1 / ? / the ⌨ toolbar button) — the full
        // shortcut + gesture map lived only in tooltips and a code comment;
        // O (mask overlay) had no visible control at all.
        if self.show_shortcuts {
            let mut open = true;
            let lang = self.lang;
            let accent = self.theme.colors().accent_text;
            egui::Window::new(tr(lang, "⌨ Shortcuts"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    // Runtime table (not `const`): the ZH column is resolved by
                    // `tr` at draw time. ASCII key combos carry no
                    // natural-language words, so they stay literal.
                    let rows: [(&str, &str); 30] = [
                        ("Ctrl/⌘+O", tr(lang, "Open photo")),
                        ("Ctrl/⌘+Shift+E / Ctrl/⌘+E", tr(lang, "Export (settings in the Export section)")),
                        ("Ctrl/⌘+S", tr(lang, "Save develop (recipe + XMP for RAW)")),
                        ("Ctrl/⌘+Z / +Shift+Z / +Y", tr(lang, "Undo / Redo")),
                        ("← / →", tr(lang, "Step through the library")),
                        ("R", tr(lang, "Enter / exit crop")),
                        ("Enter", tr(lang, "Commit the crop (exit the tool)")),
                        ("W", tr(lang, "WB eyedropper")),
                        ("Q", tr(lang, "Retouch stamp")),
                        ("K", tr(lang, "Paint mask brush")),
                        ("M / Shift+M", tr(lang, "Linear / radial gradient")),
                        (tr(lang, "Shift (while drawing a gradient)"), tr(lang, "Lock to horizontal / vertical")),
                        ("[ / ]", tr(lang, "Brush size (paint / clone armed)")),
                        ("Tab", tr(lang, "Hide / show the side panels")),
                        (tr(lang, "Hover a slider + ↑/↓"), tr(lang, "Nudge its value (Shift ×10)")),
                        (tr(lang, "B (hold)"), tr(lang, "Compare original")),
                        ("\\", tr(lang, "Before / after (toggle)")),
                        ("Y", tr(lang, "Side-by-side ↔ single view")),
                        ("O", tr(lang, "Toggle mask overlay (crop: cycle grid)")),
                        ("J", tr(lang, "Toggle clipping warning")),
                        ("Esc", tr(lang, "Exit tool / close this window")),
                        ("F1 / ?", tr(lang, "This cheat-sheet")),
                        (tr(lang, "Scroll"), tr(lang, "Zoom (toward cursor)")),
                        (tr(lang, "Double-click canvas"), tr(lang, "Fit ↔ 1:1")),
                        (tr(lang, "Space+drag / middle-drag"), tr(lang, "Pan")),
                        (tr(lang, "Drag when zoomed"), tr(lang, "Pan (Ctrl+drag = box-select)")),
                        (tr(lang, "Alt+click"), tr(lang, "Sample clone source")),
                        (tr(lang, "Slider double-click"), tr(lang, "Reset to its default")),
                        (tr(lang, "Curve: click / drag / drag-out"), tr(lang, "Add / move / delete point")),
                        (tr(lang, "Drag a mask handle"), tr(lang, "Reshape / move the selected mask")),
                    ];
                    // Bounded + scrollable: the 30-row grid clipped its lower
                    // entries off short displays (the window is non-resizable
                    // and had no height limit, unlike the Settings window).
                    let max_h = (ctx.screen_rect().height() * 0.7).max(200.0);
                    egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                        egui::Grid::new("shortcut_grid").num_columns(2).striped(true).show(
                            ui,
                            |ui| {
                                for (keys, what) in rows {
                                    ui.label(egui::RichText::new(keys).monospace().color(accent));
                                    ui.label(what);
                                    ui.end_row();
                                }
                            },
                        );
                    });
                });
            if !open {
                self.show_shortcuts = false;
            }
        }

        // Drag & drop affordance: show a full-window overlay while files hover.
        // Gated like drop PROCESSING is: while the quit-confirm layer is up,
        // drops are deliberately discarded — promising "Drop to open" there
        // was a lie.
        if !self.confirm_quit && ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            let rect = ctx.screen_rect();
            // Chrome-side veil → the PILL gold family (see the colour rule at ACCENT).
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(58, 47, 20, 150));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                tr(self.lang, "Drop to open"),
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }

        // Transient toasts (bottom-right). Errors linger longer than successes.
        self.toasts.retain(|t| t.born.elapsed() < t.ttl());
        if !self.toasts.is_empty() {
            egui::Area::new(egui::Id::new("toasts"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -40.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let c = self.theme.colors();
                    for t in &self.toasts {
                        let (bg, fg) = match t.kind {
                            ToastKind::Success => (c.toast_ok_bg, c.toast_ok_fg),
                            ToastKind::Error => (c.toast_err_bg, c.toast_err_fg),
                        };
                        egui::Frame::none()
                            .fill(bg)
                            .rounding(6.0)
                            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.label(egui::RichText::new(&t.text).color(fg));
                            });
                        ui.add_space(6.0);
                    }
                });
            // Keep repainting so expiry doesn't wait for the next input event.
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        // Land a finished edit gesture (slider release, AI Analyze, Reset) into
        // the undo history — once per gesture, after all controls are read.
        self.commit_if_settled(ctx);
    }

    /// Persist the prefs (last folder, view mode, export options) — restored by
    /// [`AutoshopApp::new`]. Window geometry is saved by eframe itself.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            eframe::APP_KEY,
            &Prefs {
                gallery_dir: self.gallery_dir.clone(),
                style_strength: self.style_strength,
                save_jpeg: self.save_jpeg,
                save_denoise: self.save_denoise,
                zoned_fit: self.zoned_fit,
                view_mode: self.view_mode,
                exp_long_edge: self.exp_long_edge,
                exp_sharpen: self.exp_sharpen,
                exp_quality: self.exp_quality,
                exp_space: self.exp_space,
                preview_edge: self.preview_edge,
                show_clipping: self.show_clipping,
                lang: self.lang,
                theme: self.theme,
            },
        );
    }
}

/// Decode the embedded Autoshop icon for the window title bar / taskbar.
fn app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../../../assets/icon_256.png"))
        .expect("embedded icon decodes")
        .to_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData { rgba: img.into_raw(), width, height }
}

fn main() -> eframe::Result<()> {
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
mod tests {
    use super::*;

    /// Both themes must keep every text-on-chrome pairing at WCAG AA (4.5:1
    /// for text, 3:1 for the armed indicator glyph). This is the contract the
    /// Light scheme was tuned against, and the guard that keeps a future
    /// colour tweak from shipping unreadable text on one theme. Pairings
    /// tested = pairings actually rendered (see each line's comment);
    /// `clip_tri_off` is deliberately dim (disarmed state) and exempt.
    #[test]
    fn both_themes_pass_contrast_checks() {
        // WCAG 2.x relative luminance + contrast ratio.
        fn lum(c: egui::Color32) -> f64 {
            let ch = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        }
        fn ratio(a: egui::Color32, b: egui::Color32) -> f64 {
            let (hi, lo) = (lum(a).max(lum(b)), lum(a).min(lum(b)));
            (hi + 0.05) / (lo + 0.05)
        }
        for theme in [ThemePref::Dark, ThemePref::Light] {
            let c = theme.colors();
            let visuals = match theme {
                ThemePref::Dark => egui::Visuals::dark(),
                ThemePref::Light => egui::Visuals::light(),
            };
            let panel = visuals.panel_fill;
            let text = visuals.widgets.noninteractive.fg_stroke.color;
            let checks: [(&str, egui::Color32, egui::Color32, f64); 7] = [
                // (what, fg, bg, minimum). The selected row carries ONLY
                // accent-coloured text (name + badges — see the gallery row),
                // so text-on-sel_bg is not a rendered pairing; multi-select
                // rows DO render default-colour names on sel_bg_dim.
                ("default text on panels", text, panel, 4.5),
                ("gold accent text on panels", c.accent_text, panel, 4.5),
                ("gold accent text on the selected row", c.accent_text, c.sel_bg, 4.5),
                ("default text on a multi-select row", text, c.sel_bg_dim, 4.5),
                ("success toast", c.toast_ok_fg, c.toast_ok_bg, 4.5),
                ("error toast", c.toast_err_fg, c.toast_err_bg, 4.5),
                // Non-text indicator: WCAG AA for UI components is 3:1.
                ("armed clipping triangle", c.clip_tri_on, panel, 3.0),
            ];
            for (what, fg, bg, min) in checks {
                let r = ratio(fg, bg);
                assert!(
                    r >= min,
                    "{theme:?} theme: {what} is {r:.2}:1, needs {min}:1"
                );
            }
            // Coloured text that sits on panel chrome (not on the dark plot).
            assert!(
                ratio(c.armed_hint, panel) >= 4.5,
                "{theme:?} theme: armed-tool hint is {:.2}:1",
                ratio(c.armed_hint, panel)
            );
            // selection_fill is PREMULTIPLIED-alpha: what actually renders is
            // its composite over the surface beneath, not the raw constant —
            // checking the raw colour validated a pairing no pixel ever shows.
            // Selected text / combo rows draw DEFAULT text over that
            // composite, on panels and on the text-edit well alike.
            let over = |fgp: egui::Color32, bg: egui::Color32| {
                let a = f64::from(fgp.a()) / 255.0;
                let ch = |f: u8, b: u8| {
                    (f64::from(f) + f64::from(b) * (1.0 - a)).round().clamp(0.0, 255.0) as u8
                };
                egui::Color32::from_rgb(
                    ch(fgp.r(), bg.r()),
                    ch(fgp.g(), bg.g()),
                    ch(fgp.b(), bg.b()),
                )
            };
            for (surface, bg) in
                [("panels", panel), ("the text-edit well", visuals.extreme_bg_color)]
            {
                let composite = over(c.selection_fill, bg);
                // Default text over a TRANSIENT text-selection highlight: the
                // 3:1 component bar, not the 4.5:1 text bar — the user is
                // mid-manipulation on text they just produced, and holding
                // 4.5:1 here would force the highlight to near-invisibility
                // on the dark panel's tiny luminance headroom.
                let r = ratio(text, composite);
                assert!(
                    r >= 3.0,
                    "{theme:?} theme: default text on selection over {surface} is {r:.2}:1 \
                     (fill composites to {composite:?}), needs 3:1"
                );
                // Selected COMBO rows draw their text with selection_stroke —
                // persistent state, full AA text bar.
                let r = ratio(c.selection_stroke, composite);
                assert!(
                    r >= 4.5,
                    "{theme:?} theme: selection-stroke text on selection over {surface} is \
                     {r:.2}:1 (fill composites to {composite:?}), needs 4.5:1"
                );
            }
            // The selection outline is a non-text UI component: 3:1 (same
            // class as the armed clipping triangle above).
            assert!(
                ratio(c.selection_stroke, panel) >= 3.0,
                "{theme:?} theme: selection stroke on panels is {:.2}:1, needs 3:1",
                ratio(c.selection_stroke, panel)
            );
            for (i, col) in c.curve_labels.iter().enumerate() {
                let r = ratio(*col, panel);
                assert!(
                    r >= 4.5,
                    "{theme:?} theme: curve label {i} is {r:.2}:1, needs 4.5:1"
                );
            }
        }
    }

    /// Every symbol the GUI renders must have a glyph in the guaranteed font
    /// chain (egui's bundle + the embedded subsets) — system fonts vary, and
    /// this is exactly how the v0.22 tofu boxes (⧉ ⊖ ◭ ▭ ◯ ◌ ✓ ✕ 🖌 …)
    /// shipped: those glyphs existed only on SOME machines. Scans STRING
    /// LITERALS of gui.rs + i18n.rs (comments never render); CJK ranges are
    /// exempt — ideographs come from the runtime system-font fallback by
    /// design. Fails ⇒ re-run scripts/subset_gui_fonts.py and commit the
    /// refreshed assets/fonts/.
    #[test]
    fn embedded_fonts_cover_every_ui_symbol() {
        use ab_glyph::Font as _;
        // Non-ASCII chars inside Rust string/char literals only. Mirrors the
        // extractor in scripts/subset_gui_fonts.py — keep the two in sync.
        fn literal_chars(src: &str, out: &mut std::collections::BTreeSet<char>) {
            let b: Vec<char> = src.chars().collect();
            let n = b.len();
            let mut i = 0;
            while i < n {
                match b[i] {
                    '/' if i + 1 < n && b[i + 1] == '/' => {
                        while i < n && b[i] != '\n' {
                            i += 1;
                        }
                    }
                    '/' if i + 1 < n && b[i + 1] == '*' => {
                        let mut depth = 1;
                        i += 2;
                        while i < n && depth > 0 {
                            if i + 1 < n && b[i] == '/' && b[i + 1] == '*' {
                                depth += 1;
                                i += 2;
                            } else if i + 1 < n && b[i] == '*' && b[i + 1] == '/' {
                                depth -= 1;
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                    }
                    'r' if i + 1 < n && (b[i + 1] == '#' || b[i + 1] == '"') => {
                        let mut j = i + 1;
                        let mut hashes = 0;
                        while j < n && b[j] == '#' {
                            hashes += 1;
                            j += 1;
                        }
                        if j < n && b[j] == '"' {
                            j += 1;
                            while j < n {
                                if b[j] == '"' {
                                    let mut k = 0;
                                    while k < hashes && j + 1 + k < n && b[j + 1 + k] == '#' {
                                        k += 1;
                                    }
                                    if k == hashes {
                                        j += 1 + hashes;
                                        break;
                                    }
                                }
                                if !b[j].is_ascii() {
                                    out.insert(b[j]);
                                }
                                j += 1;
                            }
                            i = j;
                        } else {
                            i += 1;
                        }
                    }
                    '"' => {
                        i += 1;
                        while i < n {
                            if b[i] == '\\' {
                                // `\u{...}` renders exactly like the literal
                                // char, so it needs the same glyph. STRING
                                // literals only: the one char-literal escape
                                // in this tree is the notdef sentinel below,
                                // which must stay out of the needed set.
                                if i + 2 < n && b[i + 1] == 'u' && b[i + 2] == '{' {
                                    let mut j = i + 3;
                                    let mut hex = String::new();
                                    while j < n && b[j] != '}' {
                                        hex.push(b[j]);
                                        j += 1;
                                    }
                                    if j < n {
                                        if let Some(c) = u32::from_str_radix(&hex, 16)
                                            .ok()
                                            .and_then(char::from_u32)
                                            && !c.is_ascii()
                                        {
                                            out.insert(c);
                                        }
                                        i = j + 1;
                                        continue;
                                    }
                                }
                                i += 2;
                            } else if b[i] == '"' {
                                i += 1;
                                break;
                            } else {
                                if !b[i].is_ascii() {
                                    out.insert(b[i]);
                                }
                                i += 1;
                            }
                        }
                    }
                    '\'' => {
                        // 'x' char literal (possibly non-ASCII); '\n'-style
                        // escapes; anything else is a lifetime — skip the quote.
                        if i + 2 < n && b[i + 1] != '\\' && b[i + 2] == '\'' {
                            if !b[i + 1].is_ascii() {
                                out.insert(b[i + 1]);
                            }
                            i += 3;
                        } else if i + 3 < n && b[i + 1] == '\\' && b[i + 3] == '\'' {
                            i += 4;
                        } else {
                            i += 1;
                        }
                    }
                    _ => i += 1,
                }
            }
        }

        // Walk the whole GUI module tree at runtime (round-12 split): an
        // include_str! list would silently lose coverage the moment a new
        // module file is added — the exact drift this gate exists to catch.
        fn walk_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("gui source dir listable") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    walk_rs(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let gui_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("bin")
            .join("gui");
        let mut sources = Vec::new();
        walk_rs(&gui_src, &mut sources);
        assert!(
            sources.len() >= 6,
            "font gate expected the split module tree, found {} files",
            sources.len()
        );
        let mut syms = std::collections::BTreeSet::new();
        for p in &sources {
            let text = std::fs::read_to_string(p).expect("gui source readable");
            literal_chars(&text, &mut syms);
        }
        let is_cjk = |c: char| {
            matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
        };
        // Premise: the GUI uses dozens of symbols. Finding almost none means
        // the extractor broke, and the assertions below would pass vacuously.
        assert!(
            syms.iter().filter(|&&c| !is_cjk(c)).count() >= 40,
            "literal extractor found too few symbols — it is broken"
        );

        let defaults = egui::FontDefinitions::default();
        let mut faces: Vec<ab_glyph::FontVec> = Vec::new();
        for data in defaults.font_data.values() {
            faces.push(
                ab_glyph::FontVec::try_from_vec_and_index(data.font.to_vec(), data.index)
                    .expect("egui bundled font parses"),
            );
        }
        for (name, bytes) in EMBEDDED_SYMBOL_FONTS {
            faces.push(
                ab_glyph::FontVec::try_from_vec_and_index(bytes.to_vec(), 0)
                    .unwrap_or_else(|_| panic!("embedded font {name} parses")),
            );
        }
        let covered = |c: char| faces.iter().any(|f| f.glyph_id(c).0 != 0);
        // Negative control: an unassigned codepoint must read as uncovered —
        // this is what makes `glyph_id().0 == 0` trustworthy as "no glyph".
        assert!(
            !covered('\u{0378}'),
            "notdef sentinel covered?! the coverage probe is meaningless"
        );

        let missing: Vec<String> = syms
            .iter()
            .filter(|&&c| !is_cjk(c) && !covered(c))
            .map(|&c| format!("U+{:04X} {c}", c as u32))
            .collect();
        assert!(
            missing.is_empty(),
            "UI symbols with no glyph in the guaranteed font chain \
             (re-run scripts/subset_gui_fonts.py): {missing:?}"
        );

        // CJK is embedded too (W18): choosing 中文 must not depend on the
        // machine owning a system CJK font — without one the whole window
        // rendered as tofu. Asserted separately so a regression names the
        // hanzi, and premise-checked so a broken extractor cannot pass here
        // by finding nothing.
        let cjk: Vec<char> = syms.iter().copied().filter(|&c| is_cjk(c)).collect();
        assert!(
            cjk.len() >= 300,
            "only {} CJK codepoints extracted — the translations or the \
             extractor are broken",
            cjk.len()
        );
        let missing_cjk: Vec<String> = cjk
            .iter()
            .filter(|&&c| !covered(c))
            .map(|&c| format!("U+{:04X} {c}", c as u32))
            .collect();
        assert!(
            missing_cjk.is_empty(),
            "translated text with no embedded glyph — the Chinese UI would \
             show tofu on a machine with no system CJK font \
             (re-run scripts/subset_gui_fonts.py): {missing_cjk:?}"
        );
    }

    #[test]
    fn the_pickers_average_their_window_instead_of_point_sampling() {
        // Shared by the WB eyedropper and the colour-range sample. A point
        // sampler passes any "the picked colour is about right" probe on a
        // smooth image, so pin the AVERAGING directly: one lit pixel in a dark
        // field must come back at 1/25, never 0 (missed it) and never 1 (hit
        // it). This is what makes a click on noisy pixels usable at all.
        let mut img = image::RgbImage::new(9, 9);
        img.put_pixel(4, 4, image::Rgb([255, 255, 255]));
        let img = image::DynamicImage::ImageRgb8(img);
        let centre = sample_5x5_mean(&img, 0.5, 0.5).expect("centre is in bounds");
        assert!(
            (centre[0] - 1.0 / 25.0).abs() < 1e-4,
            "one lit pixel must be averaged over the whole 5x5 window: {centre:?}"
        );
        // A CORNER click keeps only the in-bounds samples (3x3 = 9 here) and
        // divides by that count — dividing by a fixed 25 would darken every
        // edge pick, and reading out of bounds would panic.
        let mut edge = image::RgbImage::new(9, 9);
        for (_, _, p) in edge.enumerate_pixels_mut() {
            *p = image::Rgb([200, 200, 200]);
        }
        let edge = image::DynamicImage::ImageRgb8(edge);
        let corner = sample_5x5_mean(&edge, 0.0, 0.0).expect("a corner is still sampleable");
        for c in corner {
            assert!((c - 200.0 / 255.0).abs() < 1e-4, "a flat field reads flat at the corner: {corner:?}");
        }
        // A zero-size image has no samples at all — None, not a divide by zero.
        let empty = image::DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
        assert!(sample_5x5_mean(&empty, 0.5, 0.5).is_none());
    }

    #[test]
    fn releasing_a_claim_spares_anything_that_actually_landed() {
        // Five retouch workers call this on two failure paths each, and the
        // dangerous mutant is the loosest one: dropping the length check turns
        // "give the reserved NAME back" into "delete the user's partial
        // result". A non-empty file is evidence and must survive.
        let dir = std::env::temp_dir().join(format!("autoshop-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("claimed.png");
        std::fs::write(&empty, b"").unwrap();
        release_empty_claim(&empty);
        assert!(!empty.exists(), "a 0-byte claim is a reservation — give it back");
        let partial = dir.join("partial.png");
        std::fs::write(&partial, b"half an image").unwrap();
        release_empty_claim(&partial);
        assert_eq!(
            std::fs::read(&partial).unwrap(),
            b"half an image",
            "a non-empty partial is the user's result — never delete it"
        );
        // A path that was never claimed (or already swept) is not an error.
        release_empty_claim(&dir.join("never-existed.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dirty_vs_ignores_calibration_provenance() {
        // A repaired canvas (era 2, fresh curve) against an unrepaired
        // baseline (era 1, washed curve) — same user edits — is NOT dirty:
        // the curve is calibration and `version` is its provenance stamp.
        // BOTH directions are live: the stash gate produces baseline-v1 vs
        // canvas-v2, and load_version produces the reverse — a pre-era
        // snapshot whose LIFTING curve the fingerprint rightly declines
        // lands on the canvas at era 1 under an era-2 baseline. Neither may
        // ever read as an edit.
        let baseline = EditRecipe {
            version: 1,
            base_curve: vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0]],
            contrast: 20.0,
            ..Default::default()
        };
        let canvas = EditRecipe {
            version: 2,
            base_curve: vec![[0.0, 0.0], [0.2, 0.4], [1.0, 1.0]],
            contrast: 20.0,
            ..Default::default()
        };
        assert!(!dirty_vs(&canvas, &baseline), "repair is not an edit");
        assert!(!dirty_vs(&baseline, &canvas), "in either direction");
        // ...while a real edit on the same pair still is.
        let edited = EditRecipe { exposure_ev: 0.3, ..canvas.clone() };
        assert!(dirty_vs(&edited, &baseline));
    }

    #[test]
    fn insert_curve_point_keeps_inputs_sorted_and_unique() {
        let mut pts = Vec::new();
        insert_curve_point(&mut pts, 128, 140);
        insert_curve_point(&mut pts, 32, 20);
        insert_curve_point(&mut pts, 200, 210);
        assert_eq!(pts.iter().map(|p| p.input).collect::<Vec<_>>(), vec![32, 128, 200]);
        // Same input again → overwrite in place, never a duplicate input.
        let i = insert_curve_point(&mut pts, 128, 100);
        assert_eq!(i, 1);
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[1].output, 100);
    }

    #[test]
    fn drag_curve_point_clamps_strictly_between_neighbours() {
        let mut pts = vec![
            CurvePoint { input: 30, output: 30 },
            CurvePoint { input: 128, output: 128 },
            CurvePoint { input: 200, output: 200 },
        ];
        // Dragging the middle point past its right neighbour stops 1 short.
        drag_curve_point(&mut pts, 1, 240, 250);
        assert_eq!(pts[1].input, 199);
        assert_eq!(pts[1].output, 250);
        // …and past its left neighbour stops 1 above it.
        drag_curve_point(&mut pts, 1, 0, 10);
        assert_eq!(pts[1].input, 31);
        // Endpoints reach the full 0 / 255 range.
        drag_curve_point(&mut pts, 0, 0, 0);
        drag_curve_point(&mut pts, 2, 255, 255);
        assert_eq!((pts[0].input, pts[2].input), (0, 255));
        // Invariant after any sequence: inputs strictly increasing.
        assert!(pts.windows(2).all(|w| w[0].input < w[1].input));
    }

    #[test]
    fn geometric_view_mapping_roundtrips() {
        // The two boundary maps must be exact inverses (they share the
        // engine's inscribed_dims / lens_geom_norm formulas), and all-zero
        // controls must be the identity so no existing flow changes.
        // Interior points only: originals near the frame edge can be
        // legitimately cropped away by a strong barrel fix (no preimage).
        let dims = (1280.0, 853.0);
        let off = LensArg::default();
        assert_eq!(view_norm_to_orig(0.31, 0.77, dims, 0.0, &off), (0.31, 0.77));
        // A real-shaped in-camera profile joins the sweep: the composed map
        // must round-trip exactly like the manual-only one.
        let profile = autoshop::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            ..Default::default()
        };
        for deg in [0.0f32, 2.5, -7.0, 12.0] {
            for dist in [0.0f32, 60.0, -60.0, 100.0] {
                for lens in [
                    LensArg { profile: Default::default(), amount: dist },
                    LensArg { profile: profile.clone(), amount: dist },
                ] {
                    for (nx, ny) in [(0.5, 0.5), (0.35, 0.65), (0.6, 0.42)] {
                        let (vx, vy) = orig_norm_to_view(nx, ny, dims, deg, &lens);
                        // Active geometry must MOVE an off-centre point —
                        // identity-regressed wrappers round-trip perfectly
                        // and hid exactly that. This is also the suite's
                        // only inertness pin for lens_geom_norm's PROFILE
                        // branch (the render-side batch-20 twin runs
                        // profile-off) (U10).
                        if (deg != 0.0 || dist != 0.0 || lens.profile.distortion_on)
                            && (nx, ny) != (0.5, 0.5)
                        {
                            assert!(
                                (vx - nx).abs() > 1e-4 || (vy - ny).abs() > 1e-4,
                                "deg {deg} dist {dist} profile {}: ({nx},{ny}) unmoved — inert map",
                                lens.profile.distortion_on
                            );
                        }
                        let (ox, oy) = view_norm_to_orig(vx, vy, dims, deg, &lens);
                        assert!(
                            (ox - nx).abs() < 2e-3 && (oy - ny).abs() < 2e-3,
                            "deg {deg} dist {dist}: ({nx},{ny}) → view ({vx},{vy}) → back ({ox},{oy})"
                        );
                    }
                    // The centre is a fixed point at any angle + geometry.
                    let (cx, cy) = view_norm_to_orig(0.5, 0.5, dims, deg, &lens);
                    assert!((cx - 0.5).abs() < 1e-4 && (cy - 0.5).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn curve_lut_helpers_match_the_engine() {
        // The LUT + point helpers the editor builds on — renamed from
        // "curve_editor_edits_render_identically_to_the_engine": this test
        // never enters curve_editor (the driven test below does, U10).
        // Empty = identity; an anchored lift keeps the ends and raises the
        // anchored midpoint.
        let id = autoshop::render::curve_lut(&[]);
        assert!(id[0].abs() < 1e-6 && (id[255] - 1.0).abs() < 1e-6);
        assert!((id[128] - 128.0 / 255.0).abs() < 1e-3);

        let mut r = EditRecipe::default();
        let pts = curve_points_mut(&mut r, 0);
        insert_curve_point(pts, 0, 0);
        insert_curve_point(pts, 255, 255);
        insert_curve_point(pts, 64, 96); // classic shadow lift between pinned ends
        let lut = autoshop::render::curve_lut(pts);
        assert!(lut[0].abs() < 1e-6 && (lut[255] - 1.0).abs() < 1e-6);
        assert!((lut[64] - 96.0 / 255.0).abs() < 1e-3, "anchored point maps exactly");
        // The channel selector reaches the right recipe field (master only here).
        for ch in 0..4 {
            assert_eq!(curve_points(&r, ch).len(), if ch == 0 { 3 } else { 0 });
        }
    }

    #[test]
    fn curve_editor_click_adds_a_point_to_the_selected_channel() {
        // Drive the REAL editor with synthetic pointer events — a hard-coded
        // channel in the write path, a dead click branch, or a lost `changed`
        // report all stayed green under the LUT-only test above (U10).
        // Pass 1 lays out and records the square (test seam); pass 2
        // presses; pass 3 releases → egui reports the click (which pass
        // reports the change depends on egui's drag-vs-click bookkeeping,
        // so the two are OR-ed).
        let mut app = AutoshopApp { curve_channel: 2, ..Default::default() };
        let ctx = egui::Context::default();
        let run_pass = |app: &mut AutoshopApp, events: Vec<egui::Event>| -> bool {
            let mut changed = false;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    changed |= app.curve_editor(ui);
                });
            });
            changed
        };
        let _ = run_pass(&mut app, vec![]);
        let rect = app.curve_rect.expect("the editor records its square (test seam)");
        let q = egui::pos2(rect.min.x + rect.width() * 0.25, rect.min.y + rect.height() * 0.5);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: q,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let changed_press = run_pass(&mut app, vec![egui::Event::PointerMoved(q), button(true)]);
        let changed_release = run_pass(&mut app, vec![button(false)]);
        assert!(changed_press || changed_release, "the editor must report the edit");
        assert_eq!(
            curve_points(&app.recipe, 2).len(),
            1,
            "the click adds one point to the SELECTED channel"
        );
        for ch in [0usize, 1, 3] {
            assert!(curve_points(&app.recipe, ch).is_empty(), "no cross-channel write (ch {ch})");
        }
        let p = &curve_points(&app.recipe, 2)[0];
        assert!(
            (p.input as f32 - 255.0 * 0.25).abs() <= 2.0,
            "the point lands at the clicked input: {}",
            p.input
        );
        assert!(
            (p.output as i16 - p.input as i16).abs() <= 2,
            "seeded ON the identity curve: {p:?}"
        );
    }

    /// A tiny synthetic base + a bitmap mask on disk, for the async-develop
    /// and overlay regression tests — returned WITH its `Scrub`, never a path
    /// for the caller to clean up: the ./out fixture is removed on DROP, so a failing
    /// assert cannot leave it behind. Handing the guard back (rather than
    /// trusting a trailing `remove_file`) is what makes it impossible for the
    /// next caller to forget — five of them had.
    fn app_with_masked_photo(tag: &str) -> (AutoshopApp, Scrub) {
        let (w, h) = (24u32, 16u32);
        let base = Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            w,
            h,
            |x, y| image::Rgb([(x * 8 % 256) as u8, (y * 12 % 256) as u8, 120]),
        )));
        std::fs::create_dir_all("out").ok();
        let mask_path = std::path::PathBuf::from(format!("out/_gui_perf_{tag}.png"));
        image::GrayImage::from_fn(w, h, |x, _| image::Luma([if x < w / 2 { 255 } else { 0 }]))
            .save(&mask_path)
            .unwrap();
        let recipe = EditRecipe {
            masks: vec![autoshop::recipe::LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
                exposure_ev: 0.4,
                color_gains: Some([1.2, 0.95, 0.7]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let scrub = Scrub(vec![mask_path.clone()]);
        let app = AutoshopApp {
            source_preview: Some(base.clone()),
            base_preview: Some(base),
            variants: vec![Variant {
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: None,
                origin: None,
                thumb: None,
            }],
            recipe,
            sel_mask: Some(0),
            ..AutoshopApp::default()
        };
        (app, scrub)
    }

    #[test]
    fn async_develop_discards_stale_frames_latest_wins() {
        // The whole point of the async scheduler: a frame built for an OLD
        // recipe must be dropped when the live recipe has moved on, and an
        // in-flight guard must prevent a second dispatch. Drives the pure
        // pieces (build_preview + finish_redevelop) with a headless egui ctx —
        // never run_native.
        let (mut app, _scrub) = app_with_masked_photo("latest");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();

        // Dispatch itself (U10): arming must set the in-flight flag and
        // consume the pending edit, and the guard must swallow a SECOND
        // dispatch while one is armed — this is also what makes the
        // "inflight cleared" assertion below non-vacuous (develop_inflight
        // used to be false from construction to the end).
        app.dirty = true;
        app.start_redevelop();
        assert!(app.develop_inflight, "dispatch arms the in-flight flag");
        assert!(!app.dirty, "dispatch consumes the pending edit");
        app.dirty = true;
        app.start_redevelop();
        assert!(app.dirty, "the in-flight guard swallows a second dispatch (edit stays armed)");

        // A matching frame is accepted and bumps the counter + sets the texture.
        let good = build_preview(base.clone(), app.recipe.clone(), false);
        app.finish_redevelop(&ctx, Ok(good));
        assert_eq!(app.develop_count, 1, "matching frame accepted");
        assert!(app.after_tex.is_some(), "after texture set");
        assert!(!app.develop_inflight, "inflight cleared on completion");

        // Build a frame for the CURRENT recipe, then move the recipe on before
        // it "arrives": the stale frame must be discarded (counter unchanged)
        // AND the pending edit re-armed. `dirty` is cleared first — it was
        // still true from the swallowed dispatch above, so the re-arm
        // assertion used to be vacuously satisfiable (L26).
        let stale = build_preview(base, app.recipe.clone(), false);
        app.recipe.masks[0].exposure_ev = 1.9; // user kept dragging
        app.dirty = false;
        app.finish_redevelop(&ctx, Ok(stale));
        assert_eq!(app.develop_count, 1, "stale frame (recipe moved) discarded");
        assert!(app.dirty, "a discarded stale frame re-arms the pending edit");

    }

    #[test]
    fn mask_rename_flushes_at_entry_points() {
        // A typed-but-uncommitted mask rename (TextEdit still focused, so
        // the panel's lost-focus commit has not run) used to be silently
        // dropped by every entry point except Ctrl+S (U10). Driven here:
        // open_path (flush before the stash) and save_xmp (flush at its
        // head). The close guard's own call site can't be driven headless
        // (it needs a viewport close event through update()); its
        // precondition — a flushed rename reads as UNSAVED — is pinned
        // instead (Codex batch 41).
        let (mut app, _scrub) = app_with_masked_photo("rename");
        app.recipe.masks[0].name = "sky".into();
        app.saved_recipe = app.recipe.clone();
        let old = std::path::PathBuf::from("out/_rename_old.arw");
        app.src_path = Some(old.clone());
        app.mask_name_buf = Some((0, "sky".into(), "sky gradient".into()));

        app.open_path(std::path::PathBuf::from("out/_rename_new.arw"));
        assert_eq!(
            app.recipe.masks[0].name, "sky gradient",
            "open_path itself must flush the pending rename"
        );
        assert!(
            dirty_vs(&app.recipe, &app.saved_recipe),
            "the flushed rename must read as unsaved (the close guard's gate)"
        );
        let stash = app.nav_stash.get(&old).expect("a dirty canvas must be stashed");
        assert_eq!(
            stash.recipe.masks[0].name, "sky gradient",
            "the stash carries the TYPED name, not the pre-focus one"
        );

        // save_xmp's head flush, driven WITHOUT disk side effects: on a
        // generated variant save_xmp refuses with a toast BEFORE touching
        // any file, but the flush at its head has already run — removing
        // that flush previously survived this test (Codex batch 41).
        app.variants[0].kind = VariantKind::Generated;
        app.mask_name_buf = Some((0, "sky gradient".into(), "sky gradient 2".into()));
        app.save_xmp();
        assert_eq!(
            app.recipe.masks[0].name, "sky gradient 2",
            "save_xmp itself must flush the pending rename"
        );

    }

    #[test]
    fn a_clean_photo_is_not_stashed_for_another_photos_background_work() {
        // Batch 47's stash gate keyed on a per-photo count; batch 56 widened
        // the count for the quit dialog and the gate silently inherited it —
        // one photo with a dirty background variant then chain-stashed every
        // clean photo the user merely visited, the quit dialog listed them
        // as unsaved, and Save-all wrote sidecars for zero user edits.
        let (mut app, _scrub) = app_with_masked_photo("chainstash");
        app.saved_recipe = app.recipe.clone(); // this canvas is clean
        app.variants[0].origin = None;
        app.pixels_on_disk = None;
        let clean = PathBuf::from("D:/__autoshop_chain__/clean.ARW");
        app.src_path = Some(clean.clone());
        // ANOTHER photo's stash holds a dirty background variant.
        app.nav_stash.insert(
            PathBuf::from("D:/__autoshop_chain__/other.ARW"),
            StashEntry {
                recipe: app.recipe.clone(),
                base: None,
                origin: None,
                kind: VariantKind::Original,
                others: vec![StashedVariant {
                    kind: VariantKind::Generated,
                    recipe: EditRecipe { contrast: 33.0, ..Default::default() },
                    base: None,
                    origin: Some(PathBuf::from("out/_chain_gen.png")),
                }],
                active_pos: 0,
            },
        );
        assert_eq!(app.open_dirty_variants(), 0, "this photo's strip is clean");
        assert!(
            app.inactive_dirty_variants() > 0,
            "the quit surfaces still see the other photo's work"
        );
        app.open_path(PathBuf::from("D:/__autoshop_chain__/next.ARW"));
        assert!(
            !app.nav_stash.contains_key(&clean),
            "a clean photo must not be stashed for another photo's background work"
        );

    }

    #[test]
    fn navigation_stash_restores_background_variants() {
        // H4: a dirty BACKGROUND variant must survive nav-away-and-back —
        // the stash used to carry only the active canvas, so the strip
        // collapsed and the background variant's unsaved work died.
        let (mut app, _scrub) = app_with_masked_photo("h4stash");
        let ctx = egui::Context::default();
        let gen_base = Arc::new(image::DynamicImage::new_rgb8(8, 6));
        app.variants.push(Variant {
            kind: VariantKind::Generated,
            recipe: EditRecipe { contrast: 33.0, ..Default::default() },
            base: Some(gen_base),
            origin: Some(PathBuf::from("out/_h4_gen.png")),
            thumb: None,
        });
        app.saved_recipe = app.recipe.clone(); // active canvas clean
        // Unique stem, load-bearing for THIS path: the synthetic Opened
        // below lands on the fresh arm, whose read_saved_develop migrates
        // cwd ./out legacy sidecars by stem (see the keep-fact test). The
        // nav target's own rename is defence-in-depth — its decode fails
        // into the Err arm, which never reads a sidecar.
        let old = PathBuf::from("D:/__autoshop_h4__/__autoshop_h4_a__.ARW");
        app.src_path = Some(old.clone());
        assert_eq!(app.inactive_dirty_variants(), 1, "premise: background dirty");
        // Navigate away — the stash snapshot is written synchronously.
        app.open_path(PathBuf::from("D:/__autoshop_h4__/__autoshop_h4_b__.ARW"));
        {
            let st = app.nav_stash.get(&old).expect("background work must stash the strip");
            assert_eq!(st.others.len(), 1);
            assert!(matches!(st.others[0].kind, VariantKind::Generated));
        }
        // Drain the (failing) decode of the nav target so its Err cannot
        // interleave with the synthetic return below.
        for _ in 0..200 {
            app.poll_workers(&ctx);
            if !app.busy {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!app.busy, "the failed decode of the nav target must land");
        // Return: simulate the Opened result for the original photo.
        app.src_path = Some(old.clone());
        let base = Arc::new(image::DynamicImage::new_rgb8(8, 6));
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                base,
                Vec::new(),
                Default::default(),
                None,
                None,
                (1280, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants.len(), 2, "the background variant survives the round trip");
        assert!(
            app.variants
                .iter()
                .any(|v| v.kind == VariantKind::Generated && v.recipe.contrast == 33.0),
            "…with its unsaved recipe intact"
        );
        // The commit claimed "active position included", and nothing pinned
        // it: with the fixture's active == 0 the whole restore could append
        // the active variant at the END and leave self.active at 0 — canvas
        // and index disagreeing — and every assertion above still passed.
        assert_eq!(app.active, 0, "the active position comes back with the strip");
        assert!(
            matches!(app.variants[app.active].kind, VariantKind::Original),
            "…pointing at the variant that WAS active, not merely at a valid index"
        );
        assert_eq!(
            app.variants[app.active].recipe, app.recipe,
            "the canvas and the active slot must agree after a restore"
        );

    }

    #[test]
    fn overlay_skips_rebuild_for_local_effect_sliders() {
        // The coverage-aware key: dragging a mask's Exposure/Temp/color_gains
        // changes WHAT it does, not WHERE — so the full-frame coverage raster
        // must NOT rebuild. Geometry / amount / inversion MUST rebuild.
        let (mut app, _scrub) = app_with_masked_photo("overlay");
        let ctx = egui::Context::default();

        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 1, "first coverage build");
        assert!(app.mask_overlay_tex.is_some());

        // Local effect sliders: no rebuild.
        app.recipe.masks[0].exposure_ev = -2.0;
        app.refresh_mask_overlay(&ctx);
        app.recipe.masks[0].temperature = 55.0;
        app.refresh_mask_overlay(&ctx);
        app.recipe.masks[0].color_gains = Some([1.6, 0.8, 0.5]);
        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 1, "local effect sliders must not rebuild coverage");

        // Amount is coverage-relevant: rebuild.
        app.recipe.masks[0].amount = 0.5;
        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 2, "amount change rebuilds coverage");

        // Inversion is coverage-relevant: rebuild.
        app.recipe.masks[0].inverted = true;
        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 3, "inversion rebuilds coverage");

    }

    #[test]
    fn reorder_move_remap_matches_actual_remove_insert() {
        // The remap returned by reorder_move must agree with what physically
        // happens to a vec under remove(from) + insert(to) — for EVERY
        // element, every (from, insert) pair, including the append slot
        // (insert == len). The two no-op slots are the caller's guard.
        for len in 1..=5usize {
            for from in 0..len {
                for insert in 0..=len {
                    if insert == from || insert == from + 1 {
                        continue; // no-op drop slots, skipped by the GUI
                    }
                    let mut v: Vec<usize> = (0..len).collect();
                    let (to, remap) = reorder_move(from, insert);
                    let m = v.remove(from);
                    v.insert(to, m);
                    for orig in 0..len {
                        let now = v.iter().position(|&x| x == orig).unwrap();
                        assert_eq!(
                            remap(orig),
                            now,
                            "len {len} from {from} insert {insert}: element {orig}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn read_saved_develop_prefers_recipe_json_then_xmp() {
        // The open-path restore contract: no sidecar → Nothing; a NEUTRAL
        // sidecar → NoopOnly (never announced as a restore); XMP-only → the
        // reverse crs import; recipe.json present → preferred (lossless); a
        // DAMAGED recipe.json → Unreadable with the XMP fallback attached
        // (loud degradation, never a silent fall-through); a LEGACY ./out
        // sidecar → migrated into the central store on first read. Unique stem
        // so parallel tests can't race; the whole develop dir is scrubbed
        // before and after (its key is derived from this fake path only).
        let src = std::path::Path::new("D:/library/_sidecar_prio_test.ARW"); // never touched
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev); // a crashed earlier run may have left files
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all("out").unwrap();
        let rj = autoshop::store::recipe_target(src);
        let xp = autoshop::pipeline::xmp_target(src);
        let legacy_rj = autoshop::store::legacy_recipe(src);
        let _ = std::fs::remove_file(&legacy_rj);
        // Scrub on DROP: a failing assert is exactly the regression case, and
        // the tail cleanup never runs then — leaving fixtures in the real
        // central store, in ./out and beside the fake library path.
        let _scrub = Scrub(vec![dev.clone(), xp.clone(), legacy_rj.clone()]);

        assert!(
            matches!(read_saved_develop(src).0, SavedDevelop::Nothing),
            "no sidecar → Nothing"
        );

        // A NEUTRAL XMP (foreign file, or ours with nothing set) restores nothing.
        std::fs::write(&xp, autoshop::xmp::recipe_to_xmp(&EditRecipe::default())).unwrap();
        assert!(
            matches!(read_saved_develop(src).0, SavedDevelop::NoopOnly),
            "a no-op XMP must not claim a restore"
        );

        // A sidecar whose ONLY edit is CORRUPT imports as a no-op — the
        // disclosure list must still surface, or a later save silently
        // overwrites the corrupt original (Codex batch-32 #1).
        let doc = autoshop::xmp::recipe_to_xmp(&EditRecipe::default())
            .replace("crs:Exposure2012=\"0.00\"", "crs:Exposure2012=\"broken\"");
        assert!(doc.contains("broken"), "fixture: the corrupt attribute must exist");
        std::fs::write(&xp, &doc).unwrap();
        let (saved, warn, _, _) = read_saved_develop(src);
        assert!(matches!(saved, SavedDevelop::NoopOnly), "corrupt-only still restores nothing");
        assert!(warn.contains(&"Exposure2012".to_string()), "{warn:?}");

        // XMP with real edits → imported through the reverse crs mapping.
        let edited = EditRecipe { contrast: 22.0, ..Default::default() };
        std::fs::write(&xp, autoshop::xmp::recipe_to_xmp(&edited)).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src).0 else {
            panic!("an edited XMP restores");
        };
        assert_eq!((r.contrast, kind), (22.0, "XMP"));

        // recipe.json appears → preferred over the XMP.
        let full = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(&rj, serde_json::to_string(&full).unwrap()).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src).0 else {
            panic!("recipe.json restores");
        };
        assert_eq!((r.exposure_ev, kind), (0.5, "recipe.json"));

        // A damaged recipe.json degrades LOUDLY, XMP fallback attached.
        std::fs::write(&rj, "{ not json").unwrap();
        let SavedDevelop::Unreadable { fallback, .. } = read_saved_develop(src).0 else {
            panic!("a damaged recipe.json must be reported, not skipped");
        };
        assert_eq!(fallback.expect("XMP fallback rides along").1, "XMP");

        // A pre-store legacy ./out sidecar is migrated in on FIRST read and
        // then restores from the CENTRAL copy (kind says recipe.json, not the
        // legacy fallback). Migration COPIES and then suppresses the legacy
        // fallback with a tombstone rather than unlinking it: a `./out` name is
        // keyed by stem alone, so two photos with the same stem in different
        // folders share it and deleting one photo's legacy bytes destroyed the
        // other's. A SEPARATE photo path: migrate_legacy memoizes per photo per
        // process, so `src` (already touched above with nothing legacy) would
        // correctly skip the scan — the production contract, not a test bug.
        let src2 = std::path::Path::new("D:/library/_sidecar_prio_test_legacy.ARW");
        let dev2 = autoshop::store::develop_dir(src2);
        let _ = std::fs::remove_dir_all(&dev2);
        let legacy_rj2 =
            PathBuf::from("out").join("_sidecar_prio_test_legacy.recipe.json");
        let _scrub2 = Scrub(vec![dev2.clone(), legacy_rj2.clone()]);
        let legacy = EditRecipe { contrast: -11.0, ..Default::default() };
        std::fs::write(&legacy_rj2, serde_json::to_string(&legacy).unwrap()).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src2).0 else {
            panic!("a legacy ./out recipe restores");
        };
        assert_eq!((r.contrast, kind), (-11.0, "recipe.json"));
        assert!(
            legacy_rj2.exists(),
            "the stem-keyed legacy file is COPIED, never unlinked — another \
             photo with the same stem may still need it"
        );
        assert!(
            autoshop::store::recipe_target(src2).exists(),
            "…and now lives in the central store"
        );
    }

    #[test]
    fn read_saved_develop_lets_a_newer_lightroom_sidecar_win() {
        let dir = std::env::temp_dir().join("autoshop-gui-lr-sidecar");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_lr_probe.ARW");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshop::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]); // …even if an assert fires
        // The stored develop (older).
        let saved = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(
            autoshop::store::recipe_target(&src),
            serde_json::to_string(&saved).unwrap(),
        )
        .unwrap();
        // Lightroom's own sidecar beside the RAW, stamped NEWER (set, not
        // slept for).
        let lr = dir.join("_gui_lr_probe.xmp");
        std::fs::write(
            &lr,
            autoshop::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(&src).0 else {
            panic!("a newer Lightroom sidecar must restore");
        };
        assert_eq!(r.contrast, 33.0, "the newer Lightroom edit wins over the stored develop");
        assert!(kind.starts_with("XMP"), "stamped like every XMP restore: {kind}");
        assert!(kind.contains("Lightroom"), "the source is disclosed: {kind}");
        // The store copy is untouched — only an explicit save adopts it.
        let kept: EditRecipe = serde_json::from_str(
            &std::fs::read_to_string(autoshop::store::recipe_target(&src)).unwrap(),
        )
        .unwrap();
        assert_eq!(kept.exposure_ev, 0.5);
    }

    #[test]
    fn a_stale_keep_request_is_refused_without_the_same_path_fact() {
        // The KEEP arm honours the request only when open_path's recorded
        // FACT agrees the flight was same-path: honouring a stale request
        // grafted the outgoing photo's whole strip onto the incoming one.
        // Deleting `&& self.open_same_path` at the KEEP arm turns phase 1
        // into a graft and fails its assert.
        let mut app = AutoshopApp::default();
        let ctx = egui::Context::default();
        let mk = || Variant {
            kind: VariantKind::Generated,
            recipe: EditRecipe::default(),
            base: None,
            origin: Some(PathBuf::from("out/_keepfact_gen.png")),
            thumb: None,
        };
        app.variants = vec![mk(), mk()];
        // Stem must be globally improbable: the fresh arm's
        // read_saved_develop runs store::migrate_legacy, which scans the
        // cwd ./out for {stem}.recipe.json / {stem}.xmp and MOVES any hit
        // into the develop dir keyed to this fake path — a generic stem
        // ("b") could relocate a real user's legacy sidecars into an
        // unreachable key.
        app.src_path =
            Some(PathBuf::from("D:/__autoshop_keepfact__/__autoshop_keepfact__.ARW"));
        app.keep_recipe = true;
        app.open_same_path = false; // stale request, cross-photo fact
        let base = Arc::new(image::DynamicImage::new_rgb8(8, 6));
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                base.clone(),
                Vec::new(),
                Default::default(),
                None,
                None,
                (1280, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants.len(), 1, "fresh arm: the strip is rebuilt, never grafted");
        // ...and an HONOURED request (fact agrees) preserves the strip.
        app.variants = vec![mk(), mk()];
        app.keep_recipe = true;
        app.open_same_path = true;
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                base,
                Vec::new(),
                Default::default(),
                None,
                None,
                (1280, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants.len(), 2, "keep arm: the strip survives a same-path re-decode");
        // ...and the REQUEST half is load-bearing too: fact without request
        // (a same-path FRESH reopen) reloads fresh. Reducing the KEEP arm to
        // `let keep = self.open_same_path;` turns this into a graft — the
        // reload never happens and the one-shot request goes sticky — and
        // fails the assert.
        app.variants = vec![mk(), mk()];
        app.keep_recipe = false;
        app.open_same_path = true;
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(8, 6)),
                Vec::new(),
                Default::default(),
                None,
                None,
                (1280, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants.len(), 1, "fresh arm: a reopen without the request reloads");
    }

    #[test]
    fn the_history_gate_takes_request_and_fact_before_repairing() {
        // apply_step is history's one exit, and its repair gate takes
        // request AND fact: during a same-path FRESH reopen the rebuild
        // discards the repair, so the gate refuses; a same-path keep-flight
        // is admitted. (The keyboard undo is busy-gated now, so this test
        // drives the states directly — the gate is defence-in-depth, and
        // this is what keeps it honest.) The memo seam keeps this
        // decodable-RAW-free — the repair consults the memo before
        // estimating, so a primed answer makes the gate's verdict visible
        // as the era stamp. Deleting `&& self.keep_recipe` at the
        // apply_step gate repairs phase 1's install and fails its assert.
        let mut app = AutoshopApp::default();
        let ctx = egui::Context::default();
        let p = PathBuf::from("D:/__autoshop_histgate__/__autoshop_histgate__.ARW");
        // Pre-era fingerprint: version 1, interior x >= 0.5, darkens > 0.05.
        let washy = vec![[0.0, 0.0], [0.6, 0.4], [1.0, 1.0]];
        let primed = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
        autoshop::pipeline::prime_curve_memo(
            &p,
            autoshop::pipeline::curve_ident(&p),
            primed.clone(),
        );
        let pre_era =
            EditRecipe { version: 1, base_curve: washy.clone(), ..Default::default() };
        app.src_path = Some(p);
        app.variants = vec![Variant {
            kind: VariantKind::Original,
            recipe: pre_era.clone(),
            base: None,
            origin: None,
            thumb: None,
        }];
        app.recipe = pre_era.clone();
        app.committed = UndoStep { recipe: pre_era.clone(), base: None, origin: None };
        app.undo_stack.push(UndoStep { recipe: pre_era.clone(), base: None, origin: None });
        // Same-path FRESH reopen in flight: fact without request — refused.
        app.open_in_flight = true;
        app.open_same_path = true;
        app.keep_recipe = false;
        app.undo(&ctx);
        assert_eq!(app.recipe.version, 1, "fresh-reopen flight: the install is NOT repaired");
        assert_eq!(app.recipe.base_curve, washy, "…and the washed curve is untouched");
        // Same-path keep-flight: request AND fact — admitted, and the memo's
        // primed answer is adopted with its era stamp.
        app.keep_recipe = true;
        app.redo(&ctx);
        assert_eq!(
            app.recipe.version,
            autoshop::recipe::CALIB_ERA,
            "keep-flight: the reinstalled pre-era pair is repaired"
        );
        assert_eq!(app.recipe.base_curve, primed, "…with the primed answer, not a re-estimate");
        assert_eq!(
            app.variants[0].recipe.base_curve, primed,
            "…and the strip entry follows the healed canvas"
        );
        // ...and the FACT half is load-bearing too: the redo above pushed the
        // still-washed `committed` back onto the undo stack, so one more undo
        // reinstalls it — this time with the request but a CROSS-PHOTO fact,
        // where src_path already points at the incoming photo and repairing
        // would estimate the wrong RAW onto this canvas. Deleting
        // `&& self.open_same_path` from the apply_step gate repairs it and
        // fails these asserts.
        app.open_same_path = false;
        app.undo(&ctx);
        assert_eq!(app.recipe.version, 1, "cross-photo flight: the install is NOT repaired");
        assert_eq!(app.recipe.base_curve, washy, "…and the washed curve is untouched");
    }

    #[test]
    fn a_landing_that_cannot_move_the_canvas_puts_the_switch_back() {
        // The px combo mutates preview_edge BEFORE the flight starts, so
        // every landing that leaves the canvas where it was must put the
        // switch back — otherwise the displayed resolution is one the
        // preview never reached, it persists into Prefs, and five bake sites
        // read it (a heal in that window installs a new-edge raster under an
        // old-edge canvas). Three outcomes, three asserts.
        let ctx = egui::Context::default();
        let armed = |base: Option<Arc<image::DynamicImage>>, origin: Option<PathBuf>| AutoshopApp {
            src_path: Some(PathBuf::from(
                "D:/__autoshop_edgeflight__/__autoshop_edgeflight__.ARW",
            )),
            variants: vec![Variant {
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base,
                origin,
                thumb: None,
            }],
            preview_edge: 4096,             // the combo already switched...
            edge_before_flight: Some(1280), // ...and armed the flight
            keep_recipe: true,
            open_same_path: true,
            open_in_flight: true,
            // What open_path leaves on the bar for the whole flight.
            status: "decoding … ".to_string(),
            ..Default::default()
        };
        let fresh = || Arc::new(image::DynamicImage::new_rgb8(8, 6));

        // (1) The re-decode FAILED: the photo and its canvas are untouched.
        let mut app = armed(None, None);
        app.tx.send(Msg::Opened(Box::new(Err(anyhow::anyhow!("decode failed"))))).unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 1280, "a failed keep-flight puts the switch back");
        assert!(app.src_path.is_some(), "…and leaves the still-open photo alone");

        // (2) It landed, but the canvas renders an UNSAVED retouch — no
        // pixels.json, so the worker brought no master and base_preview
        // would have kept its old-edge raster under a 4096 preview_edge.
        let old_raster = Arc::new(image::DynamicImage::new_rgb8(4, 3));
        let old_source = Arc::new(image::DynamicImage::new_rgb8(6, 4));
        // The canvas master EXISTS: this is the arm where saving is the real
        // remedy, and without a live file the missing-master arm would answer
        // instead — leaving the record-less arm unpinned (a mutant that always
        // emits the other message used to survive the whole suite).
        let master = PathBuf::from("out/__autoshop_edgeflight__.heal.png");
        std::fs::create_dir_all("out").unwrap();
        let _scrub = Scrub(vec![master.clone()]);
        std::fs::write(&master, b"png").unwrap();
        let mut app = armed(Some(old_raster.clone()), Some(master.clone()));
        app.base_preview = Some(old_raster.clone());
        app.source_preview = Some(old_source.clone()); // the OLD-edge decode
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                fresh(),
                Vec::new(),
                Default::default(),
                None,
                None,
                (4096, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 1280, "a switch the canvas cannot take is not recorded");
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &old_raster)),
            "…the retouched canvas is untouched"
        );
        assert!(
            app.source_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &old_source)),
            "…and the OLD-edge source survives instead of being replaced by the new one, \
             so a later undo-to-source cannot disagree the other way"
        );
        assert!(
            app.status.starts_with("preview resolution kept"),
            "…and the refusal replaces the flight's 'decoding …', which never expires: {}",
            app.status
        );
        assert!(
            app.status.contains("save the photo"),
            "…prescribing the remedy that DOES work when the master is on disk: {}",
            app.status
        );

        // (3) The SAME master, recorded: the switch applies. The two
        // spellings differ — an in-session retouch records ./out relative,
        // store::write_pixel_source absolutizes — and comparing them
        // literally left 1:1 inspection at the old resolution.
        let rel = PathBuf::from("out/_edgeflight.png");
        let abs = std::path::absolute(&rel).unwrap();
        assert_ne!(rel, abs, "premise: the two spellings are not equal as paths");
        let mut app = armed(Some(old_raster.clone()), Some(rel));
        app.base_preview = Some(old_raster.clone());
        let master = fresh();
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                fresh(),
                Vec::new(),
                Default::default(),
                None,
                Some((master.clone(), abs, false)),
                (4096, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 4096, "a canvas that CAN be re-pointed keeps the switch");
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &master)),
            "…and renders the master re-decoded at the new edge"
        );
        assert!(
            app.variants[0].base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &master)),
            "…with the variant re-pointed at it"
        );
    }

    /// Removes its paths on DROP, so a failing assert cannot leave fixtures
    /// behind in the user's real central store or in ./out — a panic skips
    /// trailing cleanup lines, and the failing case is exactly the one a
    /// regression hits.
    struct Scrub(Vec<PathBuf>);
    impl Drop for Scrub {
        fn drop(&mut self) {
            for p in &self.0 {
                let _ = std::fs::remove_file(p);
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }

    #[test]
    fn a_saved_retouch_is_not_re_reported_as_unsaved() {
        // The store records the master ABSOLUTIZED while an in-session
        // retouch holds the same file's ./out path RELATIVE, and nothing
        // rewrites the in-memory origin at save time. Comparing the two
        // literally reported a fully saved photo as unsaved: the stash gate
        // armed on the way out, its restore re-lit the ● on the way back, and
        // the quit dialog listed a photo with nothing to save.
        // Unique stem + full scrub: this writes into the real central store.
        let src = std::path::Path::new("D:/library/__autoshop_masterid__.ARW");
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all("out").unwrap();
        let master = PathBuf::from("out/__autoshop_masterid__.heal.png");
        let _scrub = Scrub(vec![master.clone(), dev.clone()]);
        std::fs::write(&master, b"png").unwrap();
        autoshop::store::write_pixel_source(src, &master, false).unwrap();
        let mk = |origin: PathBuf| Variant {
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
            origin: Some(origin),
            thumb: None,
        };
        let mut app = AutoshopApp {
            src_path: Some(src.to_path_buf()),
            // The RELATIVE spelling, exactly as the retouch handler records it.
            variants: vec![mk(master.clone()), mk(master.clone())],
            pixels_on_disk: Some(std::path::absolute(&master).unwrap()),
            ..Default::default()
        };
        // Not persisted yet (no mirror): the background card IS unsaved work.
        assert_eq!(app.open_dirty_variants(), 1, "premise: unsaved until the strip persists");
        // The persisted record spells the master ABSOLUTIZED (read_variants
        // resolves to the dev dir / absolute), the live variant holds the
        // relative ./out spelling — same_master_opt must bridge them, or a
        // fully saved strip re-reports as unsaved (the original regression,
        // re-expressed against the v0.22 mirror).
        app.saved_strip = Some(autoshop::store::VariantsRecord {
            v: 1,
            active_kind: VariantKind::Original.store_str().to_string(),
            active_pos: 0,
            others: vec![autoshop::store::VariantEntry {
                kind: VariantKind::Original.store_str().to_string(),
                recipe: EditRecipe::default(),
                origin: Some(std::path::absolute(&master).unwrap()),
            }],
        });
        assert_eq!(
            app.open_dirty_variants(),
            0,
            "a background variant matching the persisted strip record is not unsaved work"
        );
        app.variants.truncate(1);
        // Mirror follows the trivial strip (as a save would persist it) —
        // this half of the test pins the PIXELS spelling comparison in the
        // stash gate, not strip dirtiness.
        app.saved_strip = None;
        app.open_path(PathBuf::from("D:/library/__autoshop_masterid_next__.ARW"));
        assert!(
            !app.nav_stash.contains_key(src),
            "a saved retouch must not stash as unsaved just because the store spells its master absolutely"
        );
    }

    #[test]
    fn fitted_kind_survives_the_navigation_stash() {
        // Round-9 issue 1 (variant "renames itself"): StashEntry carried the
        // active card only as `generated: bool`, so a 「◭ Reverse-fit」 card
        // came back from navigation as 「▣ 原片」. The three-valued kind must
        // round-trip through the stash write verbatim.
        let (mut app, _scrub) = app_with_masked_photo("stashkind");
        let src = PathBuf::from("D:/library/__autoshop_stashkind__.ARW");
        app.src_path = Some(src.clone());
        app.variants.push(Variant {
            kind: VariantKind::Fitted,
            recipe: EditRecipe { contrast: 21.0, ..Default::default() },
            base: None,
            origin: None,
            thumb: None,
        });
        app.active = 1;
        app.recipe = EditRecipe { contrast: 21.0, ..Default::default() };
        app.open_path(PathBuf::from("D:/library/__autoshop_stashkind_next__.ARW"));
        let st = app.nav_stash.get(&src).expect("a dirty strip stashes on nav-away");
        assert_eq!(
            st.kind,
            VariantKind::Fitted,
            "the stash records the ACTIVE card's real kind, not a generated bool"
        );
    }

    #[test]
    fn a_persisted_strip_makes_background_variants_saved_work() {
        // Round-9 issue 3 (quit livelock): after generate→fit the inactive
        // Generated card's origin can NEVER equal the photo's single recorded
        // master, so the old dirty rule held the quit guard armed forever and
        // 「Save all & quit」 bounced closed→cancelled every frame. The fix:
        // dirtiness compares the strip against the persisted variants.json
        // mirror, and persisting the strip is what Ctrl+S / Save-all now do —
        // after it, the guard's `inactive_dirty_variants() > 0` term is 0 and
        // the close goes through.
        let src = std::path::Path::new("D:/library/__autoshop_striprec__.ARW");
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let master = dev.join("reimagine-1.png");
        std::fs::write(&master, b"png").unwrap();
        let mut app = AutoshopApp {
            src_path: Some(src.to_path_buf()),
            variants: vec![
                Variant {
                    kind: VariantKind::Original,
                    recipe: EditRecipe::default(),
                    base: None,
                    origin: None,
                    thumb: None,
                },
                Variant {
                    kind: VariantKind::Generated,
                    recipe: EditRecipe::default(),
                    base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
                    origin: Some(master.clone()),
                    thumb: None,
                },
                Variant {
                    kind: VariantKind::Fitted,
                    recipe: EditRecipe { contrast: 40.0, ..Default::default() },
                    base: None,
                    origin: None,
                    thumb: None,
                },
            ],
            active: 2,
            pixels_on_disk: None,
            ..Default::default()
        };
        // The exact pre-fix trap: unsaved strip, guard armed…
        assert!(app.inactive_dirty_variants() > 0, "premise: the strip is unsaved work");
        // …then the save persists the strip (what Ctrl+S / Save-all do now)…
        app.persist_strip(src).expect("strip persists");
        // …and the guard's orphan term must be genuinely clean: this is the
        // assertion whose absence was the livelock.
        assert_eq!(
            app.inactive_dirty_variants(),
            0,
            "after a save the close guard must let the window go"
        );
        // The record really is on disk and restores the three-valued kinds.
        let rec = autoshop::store::read_variants(src).expect("record on disk");
        assert_eq!(rec.active_kind, "fitted");
        assert_eq!(rec.others.len(), 2);
        assert_eq!(rec.others[0].kind, "original");
        assert_eq!(rec.others[1].kind, "generated");
        assert_eq!(rec.others[1].origin.as_deref(), Some(master.as_path()));
        // Any strip mutation re-arms the protection: deleting the Generated
        // card is unsaved again until the next persist.
        app.variants.remove(1);
        app.active = 1;
        assert!(
            app.inactive_dirty_variants() > 0,
            "a deleted background variant is unsaved work against the mirror"
        );
        // …and switching the active card (identity drift) counts too.
        app.persist_strip(src).expect("re-persist");
        assert_eq!(app.inactive_dirty_variants(), 0);
        app.active = 0;
        app.variants.swap(0, 1);
        assert!(
            app.open_dirty_variants() > 0,
            "active-card identity drift reopens as a different strip"
        );
    }

    #[test]
    fn a_bake_follows_the_canvas_resolution_not_the_preference() {
        // A background baked variant keeps its own resolution while
        // preview_edge moves on (switching variants cannot re-decode a
        // master), so baking at the preference installed a new-edge raster
        // under an old-edge canvas — the frame jumped mid-retouch.
        //
        // COVERAGE BOUND, stated instead of implied: this pins the RULER, not
        // the five call sites that use it — the bake starters spawn network
        // workers and cannot run headless, so reverting one of those
        // single-expression uses would not fail here. Reviewed by reading.
        let mut app = AutoshopApp { preview_edge: 4096, ..Default::default() };
        assert_eq!(app.canvas_edge(), 4096, "no canvas yet: the preference is all there is");
        app.base_preview = Some(Arc::new(image::DynamicImage::new_rgb8(1280, 853)));
        assert_eq!(app.canvas_edge(), 1280, "a baked canvas bakes at its OWN resolution");
        app.base_preview = Some(Arc::new(image::DynamicImage::new_rgb8(853, 1280)));
        assert_eq!(app.canvas_edge(), 1280, "…measured on the long edge, portrait included");
    }

    #[test]
    fn a_refusal_never_prescribes_a_remedy_that_cannot_work() {
        // A master that WAS recorded but no longer resolves (moved, deleted,
        // unreadable) is not cured by saving — saving re-records the same
        // broken link and the refusal repeats forever. The two causes must
        // get different words.
        let src = std::path::Path::new("D:/library/__autoshop_gonemaster__.ARW");
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let gone = dev.join("gone-master.png");
        std::fs::write(&gone, b"png").unwrap();
        autoshop::store::write_pixel_source(src, &gone, false).unwrap();
        std::fs::remove_file(&gone).unwrap(); // recorded, and now unresolvable
        assert!(autoshop::store::has_pixel_source(src), "premise: a record survives");
        let ctx = egui::Context::default();
        let mut app = AutoshopApp {
            src_path: Some(src.to_path_buf()),
            variants: vec![Variant {
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
                origin: Some(gone.clone()),
                thumb: None,
            }],
            preview_edge: 4096,
            edge_before_flight: Some(1280),
            keep_recipe: true,
            open_same_path: true,
            open_in_flight: true,
            ..Default::default()
        };
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(8, 6)),
                Vec::new(),
                Default::default(),
                None,
                None,
                (4096, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 1280, "premise: the switch is refused");
        assert!(
            app.status.contains("no longer on disk"),
            "the missing-master case names its own cause: {}",
            app.status
        );
        assert!(
            !app.status.contains("save the photo"),
            "…and never prescribes a save that cannot restore a master that is gone: {}",
            app.status
        );
        // ...and a GENERATED canvas is not told to save either: Ctrl+S refuses
        // a generated variant outright, so that remedy is unreachable for it.
        let present = dev.join("present-master.png");
        std::fs::write(&present, b"png").unwrap();
        let mut app = AutoshopApp {
            src_path: Some(src.to_path_buf()),
            variants: vec![Variant {
                kind: VariantKind::Generated,
                recipe: EditRecipe::default(),
                base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
                origin: Some(present.clone()),
                thumb: None,
            }],
            preview_edge: 4096,
            edge_before_flight: Some(1280),
            keep_recipe: true,
            open_same_path: true,
            open_in_flight: true,
            ..Default::default()
        };
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(8, 6)),
                Vec::new(),
                Default::default(),
                None,
                None,
                (4096, None, None),
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(
            app.status.contains("generated variant"),
            "a generated canvas names its own cause: {}",
            app.status
        );
        assert!(
            !app.status.contains("save the photo"),
            "…and is not told to save, which Ctrl+S refuses for it: {}",
            app.status
        );
    }

    #[test]
    fn only_a_baked_canvas_discloses_its_own_resolution() {
        // The disclosure must key on the canvas HOLDING baked pixels, not on
        // the sizes differing: a RAW smaller than the preference decodes
        // un-upscaled, and calling that source canvas a stale bake is a lie on
        // the surface that never expires.
        let ctx = egui::Context::default();
        let src = |base: Option<Arc<image::DynamicImage>>| Variant {
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base,
            origin: None,
            thumb: None,
        };
        // (1) source-based canvas, sensor below the preference: no disclosure.
        let mut app = AutoshopApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![src(None), src(None)],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert_eq!(app.canvas_edge(), 800, "premise: the canvas is below the preference");
        assert!(
            !app.status.contains("baked"),
            "a source-based canvas is not a bake: {}",
            app.status
        );
        // (2) the same size gap, but the canvas really is a baked raster.
        let mut app = AutoshopApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![
                src(None),
                src(Some(Arc::new(image::DynamicImage::new_rgb8(640, 480)))),
            ],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert!(
            app.status.contains("640px"),
            "a baked canvas says which resolution it kept: {}",
            app.status
        );
        // (3) a baked canvas at the resolution the source DELIVERS — a
        // sub-preference sensor, freshly healed. Measured against the raw
        // preference this fresh bake was called a stale one.
        let mut app = AutoshopApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![
                src(None),
                src(Some(Arc::new(image::DynamicImage::new_rgb8(800, 600)))),
            ],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert!(
            !app.status.contains("800px"),
            "a bake at the delivered resolution is not a stale bake: {}",
            app.status
        );
        // (4) …and a baked canvas at the PREFERENCE on that same photo — a
        // recorded master re-decoded on open, which `thumbnail` UPSCALES to
        // exactly the preference. Measured against the delivered edge alone,
        // this said "stays at 1280px … not the preview preference" while the
        // preference was 1280.
        let mut app = AutoshopApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![
                src(None),
                src(Some(Arc::new(image::DynamicImage::new_rgb8(1280, 960)))),
            ],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert!(
            !app.status.contains("1280px"),
            "a canvas at the preference is not a stale bake either: {}",
            app.status
        );
        // (A third value — neither the preference nor what the source
        // delivers — is case (2) above: 640 under a 1280 preference on an 800
        // source. A separate case here would have been that fixture again.)
    }

    #[test]
    fn an_undo_to_a_superseded_master_discloses_its_resolution() {
        // Undo/redo wrote no status at all, so the quietest door onto the same
        // canvas/preference disagreement said nothing.
        let ctx = egui::Context::default();
        let superseded = Arc::new(image::DynamicImage::new_rgb8(640, 480));
        // 800 is what the SOURCE delivers here, and it is NOT the preference
        // (1280): the retraction therefore comes from the delivered-edge arm
        // specifically, and the exact-string assert below is what turns that
        // into a pin. With the two equal — and with a substring assert — this
        // test passed under either arm and pinned neither.
        let current = Arc::new(image::DynamicImage::new_rgb8(800, 600));
        let step = |base: &Arc<image::DynamicImage>, tag: &str| UndoStep {
            recipe: EditRecipe::default(),
            base: Some(base.clone()),
            origin: Some(PathBuf::from(format!("out/_{tag}.png"))),
        };
        let mut app = AutoshopApp {
            preview_edge: 1280,
            // A REAL source: without it the ruler took its `unwrap_or`
            // fallback and never the delivered-edge path this test covers.
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            base_preview: Some(current.clone()),
            variants: vec![Variant {
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: Some(current.clone()),
                origin: Some(PathBuf::from("out/_current.png")),
                thumb: None,
            }],
            committed: step(&current, "current"),
            ..Default::default()
        };
        app.undo_stack.push(step(&superseded, "superseded"));
        app.undo(&ctx);
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &superseded)),
            "premise: the superseded raster is back on the canvas"
        );
        assert!(
            app.status.contains("640px"),
            "the undo door discloses the resolution it restored: {}",
            app.status
        );
        // ...and REDOING back to a reachable canvas must RETRACT it: a
        // disclosure left standing is false in both halves.
        app.redo(&ctx);
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &current)),
            "premise: the matching-edge raster is back"
        );
        // EXACT, not `!contains("640px")`: a negative substring cannot tell
        // "no claim" from "a different claim", so it survived deleting the
        // delivered-edge conjunct (the redo then claims 800px, which contains
        // no "640px"). This is what makes the fixture pin that arm.
        assert_eq!(
            app.status, "restored the canvas pixels",
            "the claim is retracted when the disagreement ends, not merely reworded"
        );
    }

    #[test]
    fn deleting_the_active_variant_discloses_the_canvas_it_lands_on() {
        // The fourth door: deleting the active variant re-anchors onto a
        // BACKGROUND variant, whose baked raster the preference cannot
        // re-decode — silently, and carrying the deleted canvas's own
        // disclosure forward.
        let ctx = egui::Context::default();
        let baked = |w: u32, h: u32, tag: &str| Variant {
            kind: VariantKind::Generated,
            recipe: EditRecipe::default(),
            base: Some(Arc::new(image::DynamicImage::new_rgb8(w, h))),
            origin: Some(PathBuf::from(format!("out/_{tag}.png"))),
            thumb: None,
        };
        let mut app = AutoshopApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(1280, 853))),
            variants: vec![baked(640, 480, "keep"), baked(1280, 853, "drop")],
            active: 1,
            ..Default::default()
        };
        app.delete_variant(1, &ctx);
        assert_eq!(app.active, 0, "premise: the strip re-anchored");
        assert!(
            app.status.contains("640px"),
            "the landing canvas's resolution is disclosed: {}",
            app.status
        );
        // ...and landing on a canvas the preference DOES reach says so plainly.
        let mut app = AutoshopApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(1280, 853))),
            variants: vec![
                Variant {
                    kind: VariantKind::Original,
                    recipe: EditRecipe::default(),
                    base: None,
                    origin: None,
                    thumb: None,
                },
                baked(1280, 853, "drop2"),
            ],
            active: 1,
            ..Default::default()
        };
        app.delete_variant(1, &ctx);
        assert!(
            app.status.contains("variant removed"),
            "no disagreement, no claim: {}",
            app.status
        );
    }

    #[test]
    fn history_pixel_identity_compares_the_master_not_its_spelling() {
        // The last comparison the batch-80 sweep left on `==`.
        let base = Arc::new(image::DynamicImage::new_rgb8(4, 3));
        let rel = UndoStep {
            recipe: EditRecipe::default(),
            base: Some(base.clone()),
            origin: Some(PathBuf::from("out/_spelling.png")),
        };
        let abs = UndoStep {
            recipe: EditRecipe::default(),
            base: Some(base),
            origin: Some(std::path::absolute("out/_spelling.png").unwrap()),
        };
        assert!(rel.same_pixels(&abs), "one master, two spellings, one pixel identity");
        let other = UndoStep {
            recipe: EditRecipe::default(),
            base: rel.base.clone(),
            origin: Some(PathBuf::from("out/_spelling-2.png")),
        };
        assert!(!rel.same_pixels(&other), "…but a DIFFERENT master still differs");
    }

    #[test]
    fn a_failed_open_takes_the_dead_photos_history_with_it() {
        // The Err arm dropped src_path, the strip and the canvas but left the
        // undo stack standing, so an undo afterwards restored a canvas that no
        // longer existed — the keyboard path reached it because it checked
        // only `busy`. Gating the key is the guard; clearing the stack is the
        // reason there is nothing to guard. (The photo's unsaved work is not
        // lost: open_path stashed it before the flight.)
        let mut app = AutoshopApp::default();
        let ctx = egui::Context::default();
        app.src_path = Some(PathBuf::from("D:/__autoshop_deadopen__/__autoshop_deadopen__.ARW"));
        app.variants = vec![Variant {
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
            origin: Some(PathBuf::from("out/_deadopen.png")),
            thumb: None,
        }];
        let step = || UndoStep {
            recipe: EditRecipe { contrast: 11.0, ..Default::default() },
            base: None,
            origin: None,
        };
        app.undo_stack.push(step());
        app.redo_stack.push(step());
        app.tx
            .send(Msg::Opened(Box::new(Err(anyhow::anyhow!("decode failed")))))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(app.src_path.is_none(), "premise: a FRESH open failed into the no-photo state");
        assert!(
            app.undo_stack.is_empty() && app.redo_stack.is_empty(),
            "the dead photo's history goes with it"
        );
        // …so an undo here cannot reinstate the recipe of a photo that is gone.
        app.undo(&ctx);
        assert_eq!(app.recipe.contrast, 0.0, "nothing left to restore");
        assert!(app.variants.is_empty(), "…and no canvas to restore it onto");
    }

    #[test]
    fn decoded_base_lru_hits_by_key_and_evicts_least_recent() {
        // Nonexistent paths give mtime None on both sides, which must match
        // itself (the cache still works where metadata is unavailable).
        let mut app = AutoshopApp::default();
        let base = Arc::new(image::DynamicImage::new_rgb8(4, 3));
        let knots: Vec<[f32; 2]> = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let lens = autoshop::recipe::LensProfile {
            vignette: vec![1.0, 1.2],
            vignette_on: true,
            ..Default::default()
        };
        let p = std::path::Path::new("D:/__autoshop_nonexistent__/x.ARW");
        // The entry's edge (2560) deliberately DIFFERS from app.preview_edge
        // (1280): the key must come from the worker's pre-read ident riding
        // OpenedBase — a body that re-derives the EDGE from app state files
        // under 1280 and both edge asserts flip. (The stamp halves are
        // None-on-None for these nonexistent fixtures, so only the edge
        // component is pinned here; identity-before-read for the stamps is
        // review-enforced and recorded in the remember_base doc.)
        app.remember_base(
            p,
            &(
                base.clone(),
                knots.clone(),
                lens.clone(),
                Some((4830.0, 6.0)),
                None,
                (2560, None, None),
            ),
        );
        let hit = app.cached_base(p, 2560);
        assert!(hit.is_some(), "same path + the IDENT's edge hits");
        let hit = hit.unwrap();
        assert_eq!(hit.1, knots, "base-look knots ride the cache entry");
        assert_eq!(hit.2, lens, "the lens profile rides the cache entry too");
        assert_eq!(hit.3, Some((4830.0, 6.0)), "the as-shot anchor rides the cache entry too");
        assert!(
            app.cached_base(p, 1280).is_none(),
            "the key's edge comes from the pre-read ident, not preview_edge"
        );
        // Filling past the cap evicts the least-recently-used entry.
        let others: Vec<std::path::PathBuf> = (0..BASE_CACHE_CAP)
            .map(|i| std::path::PathBuf::from(format!("D:/__autoshop_nonexistent__/{i}.ARW")))
            .collect();
        for o in &others {
            app.remember_base(
                o,
                &(base.clone(), Vec::new(), Default::default(), None, None, (1280, None, None)),
            );
        }
        assert!(app.cached_base(p, 2560).is_none(), "least-recent evicted at cap");
        assert!(app.cached_base(&others[1], 1280).is_some(), "newer entries survive");
        assert!(app.base_cache.len() <= BASE_CACHE_CAP, "cap holds");
    }

    #[test]
    fn the_mask_brush_session_follows_index_remaps() {
        let mut app = AutoshopApp::default();
        app.recipe.masks =
            vec![Default::default(), Default::default(), Default::default()];
        // Session open on mask 2, deleting mask 0 shifts it to 1 — the stroke
        // must keep committing into the SAME mask, not whatever slid under
        // index 2.
        app.mask_brush = Some((Some(2), false));
        app.mask_brush_gray = Some(image::GrayImage::new(4, 4));
        app.paint_mode = true;
        app.recipe.masks.remove(0);
        app.remap_mask_indices(|s| match s {
            0 => None,
            s => Some(s - 1),
        });
        assert_eq!(app.mask_brush, Some((Some(1), false)), "target follows its mask");
        assert!(app.mask_brush_gray.is_some(), "a surviving session keeps its buffer");

        // Deleting the session's OWN mask ends it like Esc: buffer gone,
        // paint mode disarmed — never a new-mask fallback that would
        // resurrect the deleted mask under a fresh slot.
        app.recipe.masks.remove(1);
        app.remap_mask_indices(|s| match s {
            1 => None,
            s => Some(s),
        });
        assert_eq!(app.mask_brush, None, "a vanished target ends the session");
        assert!(app.mask_brush_gray.is_none(), "the weight buffer dies with it");
        assert!(!app.paint_mode, "paint mode disarms with the dead session");

        // A NEW-mask session carries no index and survives any remap.
        app.mask_brush = Some((None, true));
        app.remap_mask_indices(|_| None);
        assert_eq!(app.mask_brush, Some((None, true)));
    }

    #[test]
    fn an_async_variant_push_commits_a_typed_mask_rename_first() {
        let mut app = AutoshopApp::default();
        app.recipe.masks = vec![autoshop::recipe::LocalAdjustment {
            name: "old".into(),
            ..Default::default()
        }];
        // A photo is open: its strip holds the outgoing variant that
        // push_variant snapshots the live canvas into.
        app.variants = vec![Variant {
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        }];
        app.active = 0;
        // The user is mid-typing "sky gradient" when a reverse-fit lands and
        // push_variant auto-switches: the switch's M15 boundary clear used to
        // discard the buffer, and the outgoing variant snapshotted "old".
        app.mask_name_buf = Some((0, "old".into(), "sky gradient".into()));
        app.push_variant(
            Variant {
                kind: VariantKind::Fitted,
                recipe: EditRecipe::default(),
                base: None,
                origin: None,
                thumb: None,
            },
            &egui::Context::default(),
        );
        assert_eq!(
            app.variants[0].recipe.masks[0].name, "sky gradient",
            "the typed rename survives into the outgoing variant's snapshot"
        );
    }
}
