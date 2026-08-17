//! The application state, its defaults, and the eframe hooks.

use super::*;

pub(crate) struct AutoshopApp {
    pub(crate) src_path: Option<PathBuf>,
    // The ACTIVE variant's base pixels — shared with the preview worker by Arc,
    // so dispatching a 4096px develop is O(1), not a 50+ MB UI-thread deep copy.
    pub(crate) base_preview: Option<Arc<image::DynamicImage>>,
    // The pristine source neutral (RAW develop / loaded image), decoded once
    // per open. Source-based variants share this same allocation.
    pub(crate) source_preview: Option<Arc<image::DynamicImage>>,
    pub(crate) before_tex: Option<egui::TextureHandle>,
    pub(crate) after_tex: Option<egui::TextureHandle>,
    pub(crate) recipe: EditRecipe,
    pub(crate) dirty: bool, // recipe changed → queue the latest preview state
    // --- asynchronous develop scheduler (single in-flight, latest wins) ---
    pub(crate) develop_inflight: bool,
    pub(crate) develop_count: u64, // accepted frames; regression counter (latest-wins)
    pub(crate) status: String,
    pub(crate) busy: bool, // an analyze/export thread is running
    pub(crate) rx: Option<Receiver<Msg>>,
    pub(crate) tx: Sender<Msg>,
    /// Clone of the runtime egui context, installed at creation
    /// (`actions.rs::new`). Worker spawn and completion request repaints
    /// through it: a plain mpsc send does not wake the event loop, and the
    /// 100 ms pump's gate (`poll_workers`) runs BEFORE any panel can start a
    /// fetch in the same frame. Tests get the headless default.
    pub(crate) egui_ctx: egui::Context,
    // TYPED, not a baked string: the decision word + reasons are rendered
    // (and translated) at draw time, so a language switch re-renders them.
    pub(crate) verdict: Option<(autoshop::advisor::Decision, Vec<String>)>,
    /// Scripts already disclosed as undrawable this session (L12#3) — the
    /// tofu warning fires once per script, not once per folder or frame.
    pub(crate) disclosed_scripts: HashSet<&'static str>,
    pub(crate) rationale: String,
    /// The rationale's deterministic tail as typed notes (L12#2B). They
    /// render, in order, `rationale`'s SUFFIX — the panel strips it and
    /// draws the notes localized; on any mismatch (or when empty — a
    /// develop restored from disk carries none) the raw English rationale
    /// shows instead. Cleared wherever `rationale` is reloaded from disk.
    pub(crate) rationale_notes: Vec<autoshop::rationale::Note>,
    pub(crate) style_strength: f32,
    pub(crate) hsl_tab: usize, // Color Mixer property tab: 0=Hue 1=Saturation 2=Luminance
    pub(crate) grade_region: usize,
    pub(crate) guidance: String, // free-text direction for the AI ("warmer, moodier")
    // Delivery format + depth (阶段4) — replaces the old save_jpeg bool.
    pub(crate) exp_format: ExportFormat,
    // WHERE an export lands (R22-7) — the setting that replaced the
    // Export / Download… button pair. See [`ExportDest`].
    pub(crate) exp_dest: ExportDest,
    // The folder the last export landed in; ExportDest::LastUsed delivers
    // there, and the ask-dialog reopens there.
    pub(crate) last_export_dir: Option<PathBuf>,
    // The 「export .xmp beside the photo」 button is armed for a SECOND click:
    // a sidecar already sits beside this photo (Lightroom's own, or an earlier
    // copy) and the next click overwrites it. Session state, cleared on every
    // photo open and after the copy lands — a two-click confirm rather than a
    // modal, matching how the rest of this panel confirms in place.
    pub(crate) xmp_beside_confirm: bool,
    // --- undo / redo (a drag is one step, committed on release). Each step
    // carries the recipe AND the active variant's pixel identity (base Arc +
    // origin), so a baked pixel retouch (heal / clone / generative fill) is
    // one undoable step instead of a point of no return. Arc clones share the
    // allocation — only a retouch introduces a new raster into history.
    pub(crate) committed: UndoStep,        // current history head (last committed state)
    pub(crate) undo_stack: Vec<UndoStep>,  // prior states, most recent last
    pub(crate) redo_stack: Vec<UndoStep>,  // states undone away (cleared on a new edit)
    // --- unsaved-edit protection ---
    // What the sidecar last held for the open photo (neutral when none): the
    // canvas differing from it is the "● unsaved" condition. Navigation stashes
    // an unsaved canvas per-path for THIS session so switching photos can never
    // silently destroy work; only Ctrl+S writes disk.
    pub(crate) saved_recipe: EditRecipe,
    // Mirror of the open photo's pixels.json (the saved baked-master path, if
    // any) so the per-frame ● indicator can compare pixel identity WITHOUT a
    // disk read per frame. Updated at open, save, analyze-save and clear; the
    // decision points (close guard, quit dialog, navigation stash) still read
    // the disk — the authority.
    pub(crate) pixels_on_disk: Option<PathBuf>,
    /// Mirror of the photo's persisted `variants.json` (resolved form), the
    /// baseline the background-variant dirty test compares the live strip
    /// against. `None` = nothing persisted (the trivial single-card case).
    /// Updated at open and on every strip persist; comparing background
    /// variants against the ACTIVE canvas's `saved_recipe`/`pixels_on_disk`
    /// instead (the pre-v0.22 rule) made any two-origin strip permanently
    /// "unsaved" — the quit dialog then re-armed forever.
    pub(crate) saved_strip: Option<autoshop::store::VariantsRecord>,
    pub(crate) nav_stash: HashMap<PathBuf, StashEntry>,
    /// Master decodes in flight, keyed (photo, origin): repeat clicks into a
    /// still-decoding cold card must coalesce, not stack a fresh full-res
    /// decode thread per click.
    pub(crate) master_loads: std::collections::HashSet<(PathBuf, PathBuf)>,
    /// Decoded cold-variant masters, most-recent last. Keyed by the master's
    /// OWN identity (path + the edge it was shrunk to + its file stamp), the
    /// same rule as `base_cache` — a rewritten ./out master misses, and a px
    /// preference change re-decodes at the new edge instead of serving the
    /// old one.
    pub(crate) master_cache: Vec<((PathBuf, u32, FileStamp), Arc<image::DynamicImage>)>,
    /// The open could not READ the saved develop (damaged file, or busy in
    /// another Autoshop process): the canvas baseline is not that save, so
    /// the next explicit save must version what it overwrites (save_xmp runs
    /// the backup gate while this stands).
    pub(crate) open_unresolved: bool,
    // Paste-to-selected wrote the open photo's store: (path, exact recipe
    // written) so Msg::Pasted can advance the ● baseline on full success.
    pub(crate) pasted_open: Option<(PathBuf, EditRecipe)>,
    // The OPEN photo's fresh camera-matched base-look knots (computed by the
    // open worker, empty for non-RAW / no preview). Reset re-stamps these so
    // "reset" always means the fresh-open look — including on a photo whose
    // legacy save carries no curve and therefore opened dark.
    pub(crate) photo_knots: Vec<[f32; 2]>,
    // This photo's in-camera lens profile (open worker) — the twin of
    // photo_knots: Reset re-stamps it, legacy toggles re-enable from it.
    pub(crate) photo_lens: autoshop::recipe::LensProfile,
    // This photo's as-shot WB anchor (open worker) — the third calibration
    // half: Reset re-stamps it, the Temp slider and eyedropper anchor on it.
    pub(crate) photo_as_shot: Option<(f32, f32)>,
    // The base_curve baked into `before_tex`. The Before pane mirrors the
    // canvas recipe's calibration (Reset / paste / restore can change it), so
    // a mismatch triggers a cheap rebuild in `update`.
    pub(crate) before_curve: Vec<[f32; 2]>,
    // The title-bar ✕ was intercepted with unsaved edits: show the in-app
    // save-all / discard / cancel layer until the user decides.
    pub(crate) confirm_quit: bool,
    // Local buffer for the selected mask's name TextEdit:
    // (mask index, name at seed time, edited text). Committed on focus loss so
    // a rename is ONE undo step, not one per key; the seed-time name lets a
    // pending rename commit safely when the selection jumps rows (commit only
    // while masks[i].name still equals the seed — index shifts can't misfire).
    pub(crate) mask_name_buf: Option<(usize, String, String)>,
    // Arrow-key navigation: gallery index to scroll into view next frame.
    pub(crate) gallery_scroll_to: Option<usize>,
    // --- settings / denoise ---
    pub(crate) save_denoise: bool,     // run SCUNet AI denoise before the full-res render
    pub(crate) zoned_fit: bool,        // 反推 adds a sky-to-sky zoned correction (bitmap mask)
    pub(crate) fit_ai_judge: bool,     // 反推 then asks the vision model to SCORE the match (paid, opt-in)
    pub(crate) show_settings: bool,    // the Settings window is open
    pub(crate) show_shortcuts: bool,   // the keyboard cheat-sheet window is open (F1 / ? / ⌨)
    // Tab hides both side panels (the LR grammar) for an edge-to-edge canvas.
    // Session-only on purpose: relaunching with an invisible library reads as
    // a broken app, so the state never persists.
    pub(crate) panels_hidden: bool,
    // egui derives Tab's focus-traversal direction from raw input BEFORE
    // update() runs, so consuming the key can't stop the first widget from
    // taking focus this frame — which would also kill every focused-none
    // shortcut. Cleared on the NEXT frame instead.
    pub(crate) defocus_next: bool,
    pub(crate) settings: SettingsForm, // editable buffers for that window
    pub(crate) lang: Lang,             // UI language: English skeleton / Chinese overlay (i18n.rs)
    pub(crate) theme: ThemePref,       // dark/light chrome — applied via install_theme, kept in Prefs
    // --- library / gallery ---
    pub(crate) gallery: Vec<PathBuf>,          // sources in the working folder (sorted)
    pub(crate) gallery_dir: Option<PathBuf>,   // the working folder
    pub(crate) gallery_gen: u64,               // bumped on every folder load (thumb invalidation)
    pub(crate) selected: Option<usize>,        // index of the open gallery photo (for highlight)
    pub(crate) thumbs: HashMap<usize, egui::TextureHandle>, // decoded thumbnails by index
    pub(crate) thumb_requested: HashSet<usize>,             // indices already queued/decoded
    /// Decode attempts that FAILED, per gallery index. Bounds the retry: see
    /// `request_thumb`. Not a cache — it holds no pixels, only a count.
    pub(crate) thumb_fail: std::collections::HashMap<usize, u8>,
    pub(crate) thumb_inflight: usize,                       // live thumbnail-decode threads
    pub(crate) edited_badge: HashMap<usize, bool>,          // cached "● edited" sidecar stat per index
    // Decoded-base LRU (recent last): base pixels + the photo's base-look knots.
    pub(crate) base_cache: Vec<(BaseCacheKey, OpenedBase)>,
    // --- region box-select (local-edit target on the After image) ---
    pub(crate) region: Option<[f32; 4]>,                      // normalized [left, top, right, bottom]
    pub(crate) region_drag: Option<(egui::Pos2, egui::Pos2)>, // transient drag (start, current) in screen px
    // What a lone click just cleared, kept for one double-click window: the
    // second release of a Fit↔1:1 double-click restores it — the pair's
    // FIRST release already ran the plain-click clear (M18).
    pub(crate) region_restore: Option<([f32; 4], Instant)>,
    // --- retouch: mask painting + generative fill + heal ---
    pub(crate) paint_mode: bool,                      // brush-paint a mask (pauses box-select)
    pub(crate) brush: f32,                            // brush radius in After-image display px
    pub(crate) mask_paint: Option<image::RgbaImage>,  // painted overlay (red where painted), at preview res
    pub(crate) mask_tex: Option<egui::TextureHandle>, // overlay texture
    pub(crate) mask_dirty: bool,                      // re-upload the overlay
    pub(crate) mask_tex_xform: (f32, f32, (bool, bool)), // (straighten, distortion, (profile distortion, profile CA)) at build
    // Union of the brush segments painted since the last texture upload
    // (canvas px, [x0,y0,x1,y1] half-open). With no geometry active, only
    // this sub-rectangle is uploaded (set_partial) — brushing used to clone
    // + re-upload the WHOLE canvas on every pointer move.
    pub(crate) mask_dirty_rect: Option<[u32; 4]>,
    pub(crate) mask_tex_built: Instant, // last full rebuild — throttles mid-stroke geometry remaps
    pub(crate) paint_last: Option<(f32, f32)>,        // last brush point in mask px (line fill)
    /// A LOCAL-ADJUSTMENT brush session rides the same paint canvas as
    /// fill/heal: `(target, erase)` — target `None` = the strokes become a
    /// NEW Bitmap mask on Apply; `Some(i)` = they edit mask `i`'s raster
    /// (seeded below). While `Some`, `paint_mode` is also true so every
    /// existing paint affordance (dispatch, cursor, Esc, `[`/`]`) works.
    pub(crate) mask_brush: Option<(Option<usize>, bool)>,
    /// The brush session's WEIGHT buffer — greyscale source of truth the
    /// Apply bakes to a raster. The RGBA canvas is only its display: seeding
    /// an existing raster through the canvas' 8-bit alpha and back would
    /// drift soft edges by a rounding step per edit cycle.
    pub(crate) mask_brush_gray: Option<image::GrayImage>,
    pub(crate) fill_prompt: String,                   // generative-fill instruction
    pub(crate) fill_quality: usize,                   // 0=high 1=medium 2=low
    pub(crate) fill_fullres: bool,                    // composite onto the full-res develop
    pub(crate) heal_fullres: bool,                    // heal the full-res develop
    pub(crate) denoise_fullres: bool,                 // AI-denoise the full-sensor develop (slow)
    pub(crate) reimagine_prompt: String,              // whole-image restyle prompt (its own entry)
    // --- production niceties ---
    pub(crate) view_mode: ViewMode,                   // side-by-side vs after-only (hold B = compare)
    pub(crate) toasts: Vec<Toast>,                    // transient corner notifications
    pub(crate) histogram: Option<Vec<[f32; 4]>>,      // live RGB+luma histogram of the After preview
    pub(crate) last_title: String,                    // window title cache (send only on change)
    // --- diagnostic view layers (UX batch) ---
    pub(crate) show_mask_overlay: bool,               // translucent red coverage of the selected mask (O)
    pub(crate) mask_overlay_tex: Option<egui::TextureHandle>,
    pub(crate) overlay_stale: bool,                   // check/rebuild coverage next frame
    pub(crate) overlay_key: Option<OverlayKey>,       // skips work when coverage is unchanged
    pub(crate) hover_mask: Option<usize>,             // mask row under the cursor — previews its coverage
    pub(crate) batch_progress: Option<(usize, usize)>, // (done, total) while a batch render runs
    // Cached masks-cleared develop the coverage's range weights are judged
    // on — reused while the global (non-mask) recipe is unchanged, so a
    // mask-slider drag rebuilds only the coverage map, not a second develop.
    pub(crate) overlay_ref: Option<(EditRecipe, image::DynamicImage)>,
    pub(crate) overlay_build_count: u64,              // actual coverage rebuilds (tests/diagnostics)
    pub(crate) show_clipping: bool,                   // clipping warnings: red blown / blue crushed (J)
    pub(crate) last_rgb: Option<image::RgbImage>,     // last accepted frame's pixels (instant clip toggle)
    pub(crate) clip_tex: Option<egui::TextureHandle>,
    // First-frame OS titlebar sync (see update()'s head): viewport commands
    // sent from the creation closure are dropped before the loop runs.
    pub(crate) os_theme_synced: bool,
    // --- zoom / pan (per-photo, reset on open) ---
    pub(crate) zoom: f32,                             // 1.0 = fit; up to 12×
    pub(crate) pan: egui::Vec2,                       // visible-window centre in crop-window coords
    // The zoom that puts one texel on one PHYSICAL pixel — computed where
    // vis_px/disp/ppp exist (canvas after_view, each frame) so the `1` key
    // can act at frame start, before layout. 1.0 until a first frame lands.
    pub(crate) zoom_one_to_one: f32,
    // Where the zoom is HEADING: discrete jumps (Fit/1:1/keys/double-click)
    // set only this and `zoom` glides there over ~120 ms (step_zoom_glide);
    // continuous inputs (scroll) and photo opens write both, staying
    // instant. Pan is never animated — view_uv's per-frame clamp re-solves
    // it against the gliding zoom, which eases it for free.
    pub(crate) zoom_target: f32,
    // Last frame's controls-panel rect: ↑/↓ walk the library EXCEPT over
    // this panel, where the same keys nudge a hovered slider.
    pub(crate) controls_rect: Option<egui::Rect>,
    // --- crop tool ---
    pub(crate) crop_mode: bool,                       // the crop overlay is active on the After image
    pub(crate) crop_aspect: usize,                    // index into CROP_ASPECTS
    pub(crate) crop_aspect_pending: bool,             // preset changed — re-derive the box in handle_crop
    pub(crate) crop_grid: u8,                         // crop guide: 0 thirds · 1 golden · 2 off (O cycles, LR)
    pub(crate) before_latch: bool,                    // \ latched the Before view (toggled twin of hold-B)
    // (handle, drag start, crop at start, straighten° at start). Handles:
    // 0-3 corners, 4 move-inside, 5-8 edge midpoints (T/B/L/R), 9 = drag
    // OUTSIDE the box → rotate-straighten (the LR gesture).
    pub(crate) crop_drag: Option<(u8, egui::Pos2, [f32; 4], f32)>,
    pub(crate) mask_drag: Option<(u8, (f32, f32))>, // on-image mask knob drag: (handle, last pos in ORIG norm)
    // --- manual local adjustments (masks) ---
    pub(crate) sel_mask: Option<usize>,               // selected recipe.masks index (overlay + sliders)
    pub(crate) placing_mask: Option<(MaskKind, PlaceTarget)>, // next image drag defines a mask / redraw / component
    /// Which of the selected mask's COMPONENTS the canvas tools target:
    /// `None` = the base geometry. Reset whenever the mask selection moves.
    pub(crate) sel_component: Option<usize>,
    /// The combine mode the panel's "add shape" buttons will arm next.
    pub(crate) component_mode: autoshop::recipe::MaskCombine,
    pub(crate) place_start: Option<(f32, f32)>,       // placement drag origin, full-frame normalized
    // --- tone-curve editor ---
    pub(crate) curve_channel: usize,                  // CURVE_CHANNELS index: 0=master 1=R 2=G 3=B
    pub(crate) curve_drag: Option<usize>,             // control point being dragged (active channel)
    #[cfg(test)]
    pub(crate) curve_rect: Option<egui::Rect>,        // test seam: the editor square's last rect
    #[cfg(test)]
    pub(crate) strip_row_rect: Option<egui::Rect>,    // test seam: variant strip's inner row rect
    #[cfg(test)]
    pub(crate) strip_thumb_rect: Option<egui::Rect>,  // test seam: first variant card's thumb rect
    #[cfg(test)]
    pub(crate) strip_card_rect: Option<egui::Rect>,   // test seam: first variant card's column rect
    #[cfg(test)]
    pub(crate) strip_title_rect: Option<egui::Rect>,  // test seam: the painted "Variants" title rect
    #[cfg(test)]
    pub(crate) reimagine_btn_rect: Option<egui::Rect>, // test seam: the ✨ Generate button's rect
    #[cfg(test)]
    pub(crate) gallery_slot_rect: Option<egui::Rect>, // test seam: first drawn gallery thumb's SLOT
    #[cfg(test)]
    pub(crate) gallery_thumb_rect: Option<egui::Rect>, // test seam: … and the image drawn inside it
    #[cfg(test)]
    pub(crate) brush_slider_rect: Option<egui::Rect>, // test seam: the last Brush size slider's rect
    /// Test seam (#14a): every prompt field laid out this frame, in draw order
    /// (Direction, Reimagine, Generative Fill). A Vec, not three Options: the
    /// width cap is ONE rule and the test asserts it over the whole set, so a
    /// fourth prompt field added later is covered without touching the test.
    #[cfg(test)]
    pub(crate) prompt_rects: Vec<egui::Rect>,
    /// Test seam (#4): was the AI section's body ENABLED this frame? The
    /// mid-open editable gate (L15-2) that the AI area used to inherit from
    /// `develop_panel` had to be rebuilt in its new host, and nothing but this
    /// can tell a rebuilt gate from a comment about one.
    #[cfg(test)]
    pub(crate) ai_gate_enabled: Option<bool>,
    // --- batch recipe copy / paste ---
    pub(crate) multi_sel: HashSet<usize>,             // Ctrl+click gallery multi-selection
    pub(crate) copied: Option<EditRecipe>,            // the recipe "clipboard" (in-app only)
    // WHICH photo the clipboard came from: pasting back onto that photo may
    // keep its bitmap masks (the rasters are its own); every other target
    // gets them stripped.
    pub(crate) copied_from: Option<PathBuf>,
    pub(crate) paste_geometry: bool,                  // keep crop/straighten when pasting
    // --- WB eyedropper ---
    pub(crate) wb_picking: bool,                      // next image click samples a neutral point
    // --- colour-range sample (Range Mask) ---
    pub(crate) range_picking: Option<usize>,          // next image click keys masks[i]'s Color range
    // --- clone stamp ---
    pub(crate) clone_mode: bool,                      // brush paints the clone target; Alt+click = source
    pub(crate) clone_src: Option<(f32, f32)>,         // picked source point, original-frame normalized
    pub(crate) clone_fullres: bool,                   // clone on the full-res develop (RAW only)
    // --- cancellable retouch/generative workers ---
    // Cancel bumps `gen_epoch`; a worker's Retouched message carries the epoch
    // it was started under and is discarded on mismatch (the canvas is never
    // mutated by an abandoned task). `gen_cancel` is Some while such a worker
    // is in flight — it arms the status-bar ✕ and, for streaming generative
    // calls, is polled between events to stop the download itself.
    pub(crate) gen_epoch: u64,
    pub(crate) gen_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// An `Analyze` worker is still on the wire, cancelled or not.
    ///
    /// Cancelling an analyze is ABANDON-only — `produce_recipe` takes no
    /// cancel flag, so nothing stops the HTTP call — and ✕ clears `busy`,
    /// whose only other job was to gate `start_analyze`. Without this the ✕
    /// became a "start another one" button: each press spawned a fresh
    /// high-detail vision request that ran to its stall deadline and was
    /// BILLED, N deep. The flag is cleared when the worker's message lands,
    /// whatever its epoch.
    pub(crate) analyze_inflight: bool,
    // --- variants (版本/变体): parallel renditions of the open photo ---
    // Original + any AI-generated / reverse-fitted versions. The active one
    // drives the sliders, histogram and canvas; switching is lossless (each
    // remembers its own base + recipe), so an AI develop no longer "reverts"
    // when you touch a slider — you're editing that variant's own base.
    pub(crate) variants: Vec<Variant>,
    /// The user answered "Discard & quit". The close guard must then stop
    /// re-testing unsaved state: discarding by definition leaves the state
    /// dirty, so without this the guard cancelled the close and re-raised
    /// the dialog every frame and the app could not be quit at all. (The
    /// SAVE path needs no such flag since v0.22 — Save-all persists each
    /// photo's whole strip to variants.json, so the way-out re-check finds
    /// the state genuinely clean.)
    pub(crate) discard_requested: bool,
    pub(crate) active: usize,                         // index into `variants` (always valid once a photo is open)
    pub(crate) keep_recipe: bool,                     // one-shot: next Opened keeps recipe/variants (preview-res re-decode)
    pub(crate) open_in_flight: bool,                  // mid-open window: src_path re-pointed, Opened not yet processed
    pub(crate) open_same_path: bool,                  // the in-flight open targets the photo already open (recorded by open_path — keep_recipe is a REQUEST, not a fact)
    pub(crate) edge_before_flight: Option<u32>,       // armed by the px combo with its re-decode flight; a FAILED keep-flight reverts preview_edge to it (the canvas kept the old edge)
    // --- export pipeline (gap batch F + D2) ---
    pub(crate) exp_long_edge: u32,                    // resize long edge in px; 0 = full resolution
    pub(crate) exp_sharpen: f32,                      // output sharpening 0..100, post-resize
    pub(crate) exp_quality: f32,                      // JPEG quality 1..100 (f32 for the shared slider)
    pub(crate) exp_space: u8,                         // delivery color space: 0 sRGB / 1 Display P3 / 2 Adobe RGB
    // --- preview resolution (gap batch E) ---
    pub(crate) preview_edge: u32,                     // working-preview long edge: 1280 fluid / 2560 / 4096 detail
    // --- recipe versions (gap batch G): ./out/<stem>.v<N>.recipe.json ---
    pub(crate) versions: Vec<u32>,                    // snapshot numbers found for the open photo (sorted)
}

impl AutoshopApp {
    /// One update() phase — body extracted verbatim from the eframe
    /// update loop (round-12 decomposition).
    // pub(crate): the zoom-keys test drives a real headless frame through it.
    pub(crate) fn upd_shortcuts(&mut self, ctx: &egui::Context) {
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
            let (mut do_fit, mut do_one) = (false, false);
            let (mut do_copy, mut do_paste) = (false, false);
            let mut zoom_key: f32 = 0.0;
            let mut crop_nudge = (0.0f32, 0.0f32);
            let mut nav: i32 = 0;
            let mut brush_delta: f32 = 0.0;
            // [ / ] belong to the active brush tool; when none is armed the
            // keys stay unconsumed (free for egui / future bindings).
            let brush_tool = self.paint_mode || self.clone_mode;
            let crop_tool = self.crop_mode;
            // ↑/↓ walk the library ONLY while the pointer is off the controls
            // panel: over it, the same keys nudge a hovered slider — and this
            // handler runs FIRST each frame, so consuming here would eat the
            // nudge before the slider ever saw the key. Last frame's rect
            // (panels draw after the shortcuts) — one frame stale, harmless.
            let over_controls = ctx
                .input(|i| i.pointer.latest_pos())
                .zip(self.controls_rect)
                .is_some_and(|(p, r)| r.contains(p));
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
                // ↑/↓: the vertical twins (a vertical list wants vertical
                // keys) — gated on `over_controls` above.
                if !over_controls {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) { nav = 1; }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) { nav = -1; }
                }
                // Zoom keys — the canvas Fit / 1:1 buttons' twins. `=` is
                // unshifted `+` on ANSI layouts; Shift++ covers the rest.
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Plus)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::Plus)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Equals)
                {
                    zoom_key = 1.0;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Minus) { zoom_key = -1.0; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Num0) { do_fit = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Num1) { do_one = true; }
                // Ctrl+Shift+C/V (LR's Copy/Paste Develop Settings):
                // keyboard twins of ⎘ Copy recipe / ⇩ Paste to selected.
                // EVENT-level, not consume_key: egui-winit swallows every
                // Ctrl(+Shift)+C into Event::Copy and emits NO Key event
                // (egui-winit lib.rs is_copy_command — the Codex 阶段5
                // review caught the Key-based version as unreachable on
                // real input). Shift is the discriminator: a plain Ctrl+C
                // keeps egui's own selected-text copy, the Shift chord is
                // ours and the event is removed so nothing double-fires.
                if i.modifiers.command && i.modifiers.shift {
                    let n = i.events.len();
                    i.events.retain(|e| !matches!(e, egui::Event::Copy));
                    do_copy |= i.events.len() < n;
                    let n = i.events.len();
                    i.events.retain(|e| !matches!(e, egui::Event::Paste(_)));
                    do_paste |= i.events.len() < n;
                }
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
                self.set_crop_mode(on);
            }
            // Enter commits the crop (LR, D13): the box is already live in
            // recipe.crop — committing is dropping the grab and disarming.
            if do_crop_commit {
                self.crop_drag = None;
                self.set_crop_mode(false);
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
            // Zoom keys act on the open photo's canvas. Fit/1:1 mirror their
            // buttons exactly (1:1 via the per-frame zoom_one_to_one twin);
            // +/− step about the view centre — pan holds, view_uv reclamps.
            // All three set the TARGET only: the glide in update() carries
            // `zoom` there, and +/− compound on the target so a key roll
            // accumulates instead of fighting the animation.
            if self.src_path.is_some() {
                if do_fit {
                    self.zoom_target = 1.0;
                    self.pan = egui::vec2(0.5, 0.5);
                }
                if do_one {
                    self.zoom_target = self.zoom_one_to_one.clamp(1.0, 12.0);
                }
                if zoom_key != 0.0 {
                    self.zoom_target =
                        (self.zoom_target * 1.25f32.powi(zoom_key as i32)).clamp(1.0, 12.0);
                }
            }
            // Ctrl+C mirrors ⎘ Copy recipe (same guards + the pending-rename
            // flush); Ctrl+V mirrors ⇩ Paste to selected — and like the
            // disabled buttons, a press that would not act stays silent.
            if do_copy && self.src_path.is_some() && !self.busy {
                self.commit_mask_name_buf();
                self.copied = Some(self.recipe.clone());
                self.copied_from = self.src_path.clone();
                self.status = tr(
                    self.lang,
                    "Recipe copied — Ctrl/⌘+click to pick several, then “Paste to selected”",
                )
                .to_string();
            }
            if do_paste && !self.busy && self.copied.is_some() && !self.multi_sel.is_empty() {
                self.start_paste();
            }
        }

        // Drag & drop: dropping a photo opens it, a folder opens the library.
        // Ignored entirely while the quit-confirm layer is up — a drop must
        // not mutate the very state the user is deciding whether to save
        // (the shortcut block is gated on the same condition).
    }

    /// One update() phase — body extracted verbatim from the eframe
    /// update loop (round-12 decomposition).
    fn upd_dropped_and_title(&mut self, ctx: &egui::Context) {
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
    }

    /// One update() phase — body extracted verbatim from the eframe
    /// update loop (round-12 decomposition).
    fn upd_top_bar(&mut self, ctx: &egui::Context) {
        // egui's side_top_panel frame keeps only 2px of vertical margin —
        // at that spacing the action row visually touches the strip edges.
        let frame = egui::Frame::side_top_panel(&ctx.style())
            .inner_margin(egui::Margin::symmetric(8.0, SPACE_MD));
        egui::TopBottomPanel::top("top").frame(frame).show(ctx, |ui| {
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
                ui.add_space(SPACE_LG);
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
                ui.add_space(SPACE_LG);
                // View mode: side-by-side vs a full-width edit (hold B =
                // compare). ◫ lives in egui's bundled fonts — the old ⿲
                // (U+2FF2) only rendered when the OPTIONAL CJK fallback font
                // loaded, tofu otherwise.
                ui.selectable_value(&mut self.view_mode, ViewMode::SideBySide, tr(lang, "◫ Compare"))
                    .on_hover_text(tr(lang, "Before/After side by side"));
                ui.selectable_value(&mut self.view_mode, ViewMode::AfterOnly, tr(lang, "⬛ Single"))
                    .on_hover_text(tr(lang, "The edit fills the canvas; hold B to quickly compare the original"));
                ui.add_space(SPACE_LG);
                // Delivery ACTIONS (their settings live in the Develop panel's
                // Export section; the hover echoes the current delivery state
                // so it stays glanceable without a toolbar row of combos).
                //
                // ONE split button (R22-7). 「Export」 and 「Download…」 were the
                // same code path — start_render_to — differing only in where
                // the target came from, which made "where do my files go" a
                // property of WHICH BUTTON you pressed instead of a setting.
                // The main half now follows the Destination setting; the ▾ half
                // is the one-off escape hatch that leaves the setting alone.
                // Zero item_spacing joins the two halves visually into one
                // control (egui has no split-button widget).
                let summary = self.export_summary(lang);
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if ui
                        .add_enabled(ready, egui::Button::new(tr(lang, "Export")))
                        .on_hover_text(format!(
                            "{}\n{summary}",
                            tr(lang, "Ctrl+Shift+E (or Ctrl+E) · full-resolution render to the Destination below (follows the current variant's pixels); Destination + settings live in the Export section")
                        ))
                        .clicked()
                    {
                        self.start_export();
                    }
                    // `add_enabled_ui`, not a disabled Button: menu_button
                    // returns an InnerResponse and takes no enabled flag.
                    ui.add_enabled_ui(ready, |ui| {
                        ui.menu_button("▾", |ui| {
                            if ui
                                .button(tr(lang, "Export to…"))
                                .on_hover_text(tr(lang, "Pick a path for THIS export only — the Destination setting is left as it is"))
                                .clicked()
                            {
                                ui.close_menu();
                                self.export_to_chosen_path();
                            }
                        })
                        .response
                        .on_hover_text(tr(lang, "Export to a one-off path…"));
                    });
                });
                if ui
                    .add_enabled(ready, egui::Button::new(tr(lang, "Save develop")))
                    .on_hover_text(tr(lang, "Ctrl+S · save this photo's develop (recipe + a Lightroom/ACR XMP for RAW; a baked retouch master is linked so reopening restores it) to your develop store"))
                    .clicked()
                {
                    self.save_xmp();
                }
                ui.add_space(SPACE_LG);
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
    }

    /// One update() phase — body extracted verbatim from the eframe
    /// update loop (round-12 decomposition).
    fn upd_status_bar(&mut self, ctx: &egui::Context) {
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
    }

    /// Variant-strip panel height. Derivation: an 84 px card column (52 px
    /// thumb, 6 px item_spacing, and a 26 px label row — egui rows are at
    /// least interact_size.y tall), 8 px of air above and below the column
    /// (variant_strip top-pads it into center), and the panel frame's
    /// 2+2 px vertical margins. One authority — the strip geometry test
    /// lays the real panel out at this same constant and asserts the two
    /// airs are EQUAL.
    pub(crate) const VARIANT_STRIP_H: f32 = 104.0;

    /// One update() phase — body extracted verbatim from the eframe
    /// update loop (round-12 decomposition).
    fn upd_strips_and_side_panels(&mut self, ctx: &egui::Context) {
        // Variant strip — sits directly above the status bar (registered after
        // it so it stacks on top), only when a photo is open. The selector for
        // 原片 / AI 生成 / 反推 renditions.
        if self.src_path.is_some() {
            egui::TopBottomPanel::bottom("variants")
                // At the old 96 the cards' slack all pooled below the labels
                // — height and its derivation live on VARIANT_STRIP_H.
                .exact_height(Self::VARIANT_STRIP_H)
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

            let controls = egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.src_path.is_some() {
                        // The AI area is FIRST (R22 #4): one first-level
                        // section holding every AI develop verb — analysis,
                        // whole-image generation, reverse-fit — above the
                        // sliders it writes into. It used to be three
                        // fragments: a section inside Develop, a section in
                        // the middle of Retouch, and a switch in Settings.
                        self.ai_panel(ui);
                        self.develop_panel(ui);
                        self.retouch_panel(ui);
                    } else {
                        // Empty state with the same voice as the canvas hero:
                        // centered, quiet, and it says what to do next — a
                        // bare left-aligned label read as a stuck error.
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("🖼").size(26.0).weak());
                            ui.add_space(SPACE_SM);
                            ui.label(egui::RichText::new(tr(self.lang, "No photo open.")).weak());
                            ui.label(
                                egui::RichText::new(tr(
                                    self.lang,
                                    "Click a thumbnail in the Library, or press Ctrl+O.",
                                ))
                                .weak()
                                .small(),
                            );
                            ui.add_space(SPACE_MD);
                            // Round-13 easter egg — quiet enough to miss,
                            // which is the point of an easter egg.
                            ui.label(
                                egui::RichText::new(tr(
                                    self.lang,
                                    "Fun fact: this empty state finished loading while Photoshop's splash screen would still be painting its clouds.",
                                ))
                                .weak()
                                .small()
                                .italics(),
                            );
                        });
                    }
                });
            });
            // ↑/↓ routing (see upd_shortcuts): over this panel the keys
            // belong to a hovered slider; anywhere else they walk the
            // library. One frame stale — panels draw after the shortcuts.
            self.controls_rect = Some(controls.response.rect);
        } else {
            self.controls_rect = None;
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
    }

    /// One update() phase — body extracted verbatim from the eframe
    /// update loop (round-12 decomposition).
    fn upd_central(&mut self, ctx: &egui::Context) {
        // The canvas sits on a bed one step below the panel fill (recessed
        // stage / raised chrome) — keep the stock margins, swap the fill.
        let bed = egui::Frame::central_panel(&ctx.style())
            .fill(self.theme.colors().canvas_bed);
        egui::CentralPanel::default().frame(bed).show(ctx, |ui| {
            // Empty state: a real landing surface instead of a blank canvas.
            if self.src_path.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.32);
                    ui.heading("Autoshop");
                    ui.label(egui::RichText::new(tr(self.lang, "AI auto-develop · RAW develop")).weak());
                    ui.add_space(SPACE_LG);
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
    }

    /// One update() phase — body extracted verbatim from the eframe
    /// update loop (round-12 decomposition).
    fn upd_sheets(&mut self, ctx: &egui::Context) {
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
                    let rows: [(&str, &str); 33] = [
                        ("Ctrl/⌘+O", tr(lang, "Open photo")),
                        ("Ctrl/⌘+Shift+E / Ctrl/⌘+E", tr(lang, "Export to the Destination (Destination + settings in the Export section)")),
                        ("Ctrl/⌘+S", tr(lang, "Save develop (recipe + XMP for RAW)")),
                        ("Ctrl/⌘+Z / +Shift+Z / +Y", tr(lang, "Undo / Redo")),
                        ("Ctrl/⌘+Shift+C / +Shift+V", tr(lang, "Copy recipe / paste to selected")),
                        ("← / →", tr(lang, "Step through the library")),
                        ("↑ / ↓", tr(lang, "Step through the library (outside the controls panel)")),
                        ("+ / − / 0 / 1", tr(lang, "Zoom in / out / fit / 1:1")),
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
                        (tr(lang, "Slider double-click / right-click"), tr(lang, "Reset to its default")),
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
                    ui.add_space(SPACE_SM);
                    // Round-13 easter egg: the pricing model, disclosed.
                    ui.label(
                        egui::RichText::new(tr(
                            lang,
                            "Every shortcut above ships free — no Creative tier, no Cloud, no monthly ransom.",
                        ))
                        .weak()
                        .small(),
                    );
                });
            if !open {
                self.show_shortcuts = false;
            }
        }

        // Drag & drop affordance: show a full-window overlay while files hover.
    }
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
            egui_ctx: egui::Context::default(),
            verdict: None,
            disclosed_scripts: HashSet::new(),
            rationale: String::new(),
            rationale_notes: Vec::new(),
            style_strength: 0.30,
            hsl_tab: 0,
            grade_region: 0,
            guidance: String::new(),
            exp_format: ExportFormat::Tiff16,
            // ./out — the CLI's and the batch renderer's own root; mirror
            // Prefs::default so a missing pref key means exactly this.
            exp_dest: ExportDest::OutFolder,
            last_export_dir: None,
            xmp_beside_confirm: false,
            committed: UndoStep::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            save_denoise: false,
            zoned_fit: true,
            // Paid opt-in (a vision call per fit) — mirror Prefs::default.
            fit_ai_judge: false,
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
            master_cache: Vec::new(),
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
            mask_tex_xform: (0.0, 0.0, (false, false)),
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
            zoom_one_to_one: 1.0,
            zoom_target: 1.0,
            controls_rect: None,
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
            #[cfg(test)]
            strip_row_rect: None,
            #[cfg(test)]
            strip_thumb_rect: None,
            #[cfg(test)]
            strip_card_rect: None,
            #[cfg(test)]
            strip_title_rect: None,
            #[cfg(test)]
            reimagine_btn_rect: None,
            #[cfg(test)]
            gallery_slot_rect: None,
            #[cfg(test)]
            gallery_thumb_rect: None,
            #[cfg(test)]
            brush_slider_rect: None,
            #[cfg(test)]
            prompt_rects: Vec::new(),
            #[cfg(test)]
            ai_gate_enabled: None,
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
            os_theme_synced: false,
        }
    }
}

impl eframe::App for AutoshopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // First frame: re-issue the OS-titlebar theme. install_theme sends it
        // from the creation closure, but viewport commands sent before the
        // event loop runs are dropped — a FIRST-ever launch (no prefs, so no
        // second install) kept the OS-default titlebar over dark chrome.
        if !std::mem::replace(&mut self.os_theme_synced, true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match self.theme {
                ThemePref::Dark => egui::SystemTheme::Dark,
                ThemePref::Light => egui::SystemTheme::Light,
            }));
        }
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

        self.upd_shortcuts(ctx);
        self.apply_zoom_glide(ctx);
        self.upd_dropped_and_title(ctx);

        self.upd_top_bar(ctx);

        self.upd_status_bar(ctx);

        self.upd_strips_and_side_panels(ctx);
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

        self.upd_central(ctx);

        self.upd_sheets(ctx);
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

        // Transient toasts (bottom-right). Errors linger longer than
        // successes, long texts longer than short ones (Toast::ttl), a click
        // dismisses early, and the last 300 ms fade instead of blinking out.
        self.toasts.retain(|t| t.born.elapsed() < t.ttl());
        if !self.toasts.is_empty() {
            let mut dismiss: Option<usize> = None;
            let mut fading = false;
            egui::Area::new(egui::Id::new("toasts"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -40.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let c = self.theme.colors();
                    for (idx, t) in self.toasts.iter().enumerate() {
                        let (bg, fg) = match t.kind {
                            ToastKind::Success => (c.toast_ok_bg, c.toast_ok_fg),
                            ToastKind::Error => (c.toast_err_bg, c.toast_err_fg),
                        };
                        let left = t.ttl().saturating_sub(t.born.elapsed());
                        let fade = (left.as_millis() as f32 / 300.0).min(1.0);
                        fading |= fade < 1.0;
                        // Elevated pill: the theme's popup shadow + a faint
                        // text-tinted hairline lift it off the photo — a flat
                        // fill over arbitrary canvas content had no edge at
                        // all where the photo matched the toast colour.
                        let mut shadow = ui.visuals().popup_shadow;
                        shadow.color = shadow.color.gamma_multiply(fade);
                        let r = egui::Frame::none()
                            .fill(bg.gamma_multiply(fade))
                            .stroke(egui::Stroke::new(1.0, fg.gamma_multiply(0.35 * fade)))
                            .shadow(shadow)
                            .rounding(RADIUS_MD)
                            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                            .show(ui, |ui| {
                                // Named token (#14a): this 420 is a WRAPPED
                                // paragraph's measure, not the prompt-field
                                // ceiling that happens to share the number —
                                // see theme.rs on why they are two constants.
                                ui.set_max_width(TOAST_W_MAX);
                                ui.label(egui::RichText::new(&t.text).color(fg.gamma_multiply(fade)));
                            })
                            .response
                            .interact(egui::Sense::click());
                        if r.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if r.clicked() {
                            dismiss = Some(idx);
                        }
                        ui.add_space(SPACE_MD);
                    }
                });
            if let Some(i) = dismiss {
                self.toasts.remove(i);
            }
            // Keep repainting so expiry doesn't wait for the next input
            // event — per frame while a fade is running, else a coarse poll.
            if fading {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }

        // Land a finished edit gesture (slider release, AI Analyze, Reset) into
        // the undo history — once per gesture, after all controls are read.
        self.commit_if_settled(ctx);
        // LAST, after every panel had its chance to spawn (L12-3): evaluated
        // at the top of update (inside poll_workers) the gate judged a frame
        // whose spawns had not happened yet, so the FIRST batch of workers
        // each frame started with no completion pump armed and their results
        // sat in the channel until the next input event.
        self.pump_repaint_gate(ctx);
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
                // Both written: exp_format is the truth, save_jpeg keeps a
                // pre-阶段4 build reading this prefs file sane.
                save_jpeg: self.exp_format == ExportFormat::Jpeg,
                exp_format: self.exp_format.pref_code(),
                exp_dest: self.exp_dest.pref_code(),
                last_export_dir: self.last_export_dir.clone(),
                save_denoise: self.save_denoise,
                zoned_fit: self.zoned_fit,
                fit_ai_judge: self.fit_ai_judge,
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
