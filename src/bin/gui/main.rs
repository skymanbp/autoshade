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
mod actions;
mod canvas;
mod export;
mod masks;
mod panels;
mod workers;
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

}

impl AutoshopApp {

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
