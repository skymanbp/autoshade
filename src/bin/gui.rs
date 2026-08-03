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

/// How the preview area is laid out. `AfterOnly` gives the edit the whole
/// canvas (hold **B** to flash the source in place — the Lightroom gesture);
/// `SideBySide` keeps the permanent comparison.
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
enum ViewMode {
    SideBySide,
    AfterOnly,
}

/// A transient corner notification. Errors linger twice as long as successes —
/// a one-line status bar alone is too easy to miss for a failed export.
struct Toast {
    text: String,
    kind: ToastKind,
    born: Instant,
}

#[derive(Clone, Copy, PartialEq)]
enum ToastKind {
    Success,
    Error,
}

impl Toast {
    fn ttl(&self) -> Duration {
        match self.kind {
            ToastKind::Success => Duration::from_secs(4),
            ToastKind::Error => Duration::from_secs(8),
        }
    }
}

/// The prefs worth remembering across launches (stored via eframe persistence
/// next to the window geometry). Everything here must stay cheap to re-apply.
/// `serde(default)` so prefs saved by an older build (missing newer keys) still
/// load instead of silently resetting everything.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Prefs {
    gallery_dir: Option<PathBuf>,
    style_strength: f32,
    save_jpeg: bool,
    save_denoise: bool,
    zoned_fit: bool,
    view_mode: ViewMode,
    exp_long_edge: u32,
    exp_sharpen: f32,
    exp_quality: f32,
    exp_space: u8,
    preview_edge: u32,
    show_clipping: bool,
    lang: Lang,
}

impl Default for Prefs {
    fn default() -> Self {
        // Mirror AutoshopApp's own defaults (see its Default impl) so a pref
        // key missing from an older save degrades to exactly the app default.
        Self {
            gallery_dir: None,
            style_strength: 0.30,
            save_jpeg: false,
            save_denoise: false,
            // Zoned sky reverse-fit ON by default: it degrades gracefully to
            // the plain global fit when segmentation is unavailable.
            zoned_fit: true,
            view_mode: ViewMode::SideBySide,
            exp_long_edge: 0,
            exp_sharpen: 0.0,
            exp_quality: 95.0,
            exp_space: 0,
            preview_edge: PREVIEW_EDGE,
            show_clipping: false,
            lang: Lang::En, // English is the default / skeleton language
        }
    }
}

const PREVIEW_EDGE: u32 = 1280; // working preview size for fast live develop
const THUMB_EDGE: u32 = 160; // decoded gallery-thumbnail long edge
const THUMB_W: f32 = 56.0; // displayed thumbnail size in the gallery
const THUMB_H: f32 = 40.0;
const GALLERY_ROW_H: f32 = 50.0; // fixed row height for ScrollArea::show_rows
const MAX_THUMB_INFLIGHT: usize = 6; // cap concurrent thumbnail decodes
const HSL_BANDS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];
// Representative swatch per HSL band (row markers in the mixer — approximate
// band-centre hues, display-only; the engine's band maths lives in render.rs).
const HSL_SWATCH: [egui::Color32; 8] = [
    egui::Color32::from_rgb(224, 72, 60),  // Red
    egui::Color32::from_rgb(224, 141, 60), // Orange
    egui::Color32::from_rgb(224, 211, 60), // Yellow
    egui::Color32::from_rgb(76, 191, 90),  // Green
    egui::Color32::from_rgb(60, 200, 200), // Aqua
    egui::Color32::from_rgb(76, 120, 224), // Blue
    egui::Color32::from_rgb(142, 90, 217), // Purple
    egui::Color32::from_rgb(208, 82, 184), // Magenta
];
const GRADE_REGIONS: [&str; 4] = ["Shadows", "Midtones", "Highlights", "Global"];

// Two-tier colour rule (deliberate, not drift):
//  * PILL gold is the ONE chrome accent — panel selections, badges, active
//    variant, theme selection stroke all use the gold family below.
//  * ACCENT blue is for ON-CANVAS tool overlays ONLY (mask knobs, region
//    box): they sit on arbitrary photo content, and gold vanishes on warm
//    frames (a golden canyon) exactly where masks get drawn most — a colour
//    the theme never uses reads as "tool, not photo, not chrome".
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x4c, 0x8b, 0xf5);
const PILL: egui::Color32 = egui::Color32::from_rgb(0xc9, 0xa1, 0x4a);
/// Gallery selected-row fill: the PILL family at panel-background depth.
const SEL_BG: egui::Color32 = egui::Color32::from_rgb(0x45, 0x38, 0x1a);
/// Multi-select row fill — dimmer than [`SEL_BG`], same family.
const SEL_BG_DIM: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x26, 0x12);

/// How a finished retouch enters the variant strip.
#[derive(Clone, Copy, PartialEq)]
enum RetouchKind {
    /// A whole-frame REIMAGINE rendition → a NEW「AI 生成」variant (its look
    /// lives in the pixels).
    NewGenerated,
    /// A fill/heal/clone touch-up of the CURRENT rendition → bake into the
    /// active variant's base AND repoint its `origin` at the saved artifact, so
    /// export / reverse-fit / a further retouch all follow the retouched pixels
    /// (WYSIWYG) instead of the pre-retouch source.
    InPlace,
}

/// A finished retouch from any of the four pixel paths (fill/heal/clone/
/// reimagine): `(preview of the ./out result, status message, the saved
/// full-resolution ./out artifact, kind)`. The saved path becomes the affected
/// variant's `origin` — its export / reverse-fit / next-retouch source.
type RetouchDone = anyhow::Result<(image::DynamicImage, String, PathBuf, RetouchKind)>;

/// CPU-built preview frame. Everything expensive is worker-side: engine
/// develop, geometry, the one RGB8 conversion, histogram, clipping pixels and
/// the 96px variant thumbnail. The UI thread only submits the prepared images
/// to egui textures. `base` + `recipe` are identity tags: if either differs
/// when the result arrives, the frame is stale and is discarded (latest wins).
struct PreviewDone {
    base: Arc<image::DynamicImage>,
    recipe: EditRecipe,
    /// Kept AFTER display too (as `last_rgb`) so toggling the clipping layer
    /// can rebuild its overlay without a full redevelop.
    rgb: image::RgbImage,
    /// The texture-ready conversion of `rgb`, built worker-side — the UI
    /// thread's only job is `tex.set` (the RGB→RGBA expansion of a 4-11 MP
    /// frame at 2560/4096 was a 5-15 ms stall per accepted frame).
    after: egui::ColorImage,
    histogram: Vec<[f32; 4]>,
    clipping: Option<egui::ColorImage>,
    thumb: egui::ColorImage,
}

/// Coverage-cache identity. Local effect sliders (Exposure/Temp/Saturation/
/// color_gains) are intentionally absent: they change pixels INSIDE a mask,
/// never its coverage. A Range Mask includes the masks-cleared reference recipe
/// because its weight is judged on those developed pixels.
#[derive(Clone, PartialEq)]
struct OverlayKey {
    base: usize,
    target: usize,
    mask: MaskGeometry,
    range: Option<RangeMask>,
    amount: f32,
    inverted: bool,
    reference_recipe: Option<EditRecipe>,
    straighten_deg: f32,
    lens_distortion: f32,
    // The coverage warp also depends on the PROFILE geometry toggles —
    // without them in the key, flipping profile Distortion/CA reused a wash
    // warped under the previous state (knot data only changes per photo,
    // which `base` already keys).
    profile_dist_on: bool,
    profile_ca_on: bool,
}

/// Messages from worker threads back to the UI. The large payloads are boxed so
/// the channel message stays small (clippy::large_enum_variant).
enum Msg {
    /// Decoded base + the photo's camera-matched base-look knots (empty for
    /// non-RAW sources, missing embedded previews, or a near-identity map —
    /// see `render::camera_base_knots`).
    Opened(Box<anyhow::Result<OpenedBase>>),
    /// A synchronous GUI develop used to block egui's update loop. Preview work
    /// now returns here from a single latest-wins worker.
    Developed(Box<anyhow::Result<PreviewDone>>),
    Analyzed(Box<anyhow::Result<(EditRecipe, autoshop::advisor::Verdict)>>),
    Exported(anyhow::Result<String>),
    /// A folder scan finished: (folder, sorted source paths).
    Folder(Box<anyhow::Result<(PathBuf, Vec<PathBuf>)>>),
    /// A gallery thumbnail decoded. `generation` tags the folder generation so a
    /// folder switch can't insert a stale thumbnail under a reused index.
    Thumb { generation: u64, idx: usize, img: Box<anyhow::Result<image::DynamicImage>> },
    /// A generative-fill / heal / clone / reimagine result — see [`RetouchDone`].
    Retouched(Box<RetouchDone>),
    /// AI segmentation finished: (mask display name, grayscale raster path)
    /// — attached to the recipe as a `MaskGeometry::Bitmap` local mask.
    Segmented(anyhow::Result<(String, PathBuf)>),
    /// Batch render advanced: `done` of `total` photos finished (ok or err).
    BatchProgress { done: usize, total: usize },
    /// Reverse-fit finished: the fitted recipe + a status note (fit.rs).
    /// (recipe, status note, persisted): `persisted` is false when the backup
    /// gate refused the store write — the ● baseline must NOT be advanced then.
    Fitted(Box<anyhow::Result<(EditRecipe, String, bool)>>),
    /// Settings → "Import develops from an old ./out folder": localized result.
    LegacyImported(anyhow::Result<String>),
    /// Style-prompt extraction finished: the reusable prompt text.
    Styled(Box<anyhow::Result<String>>),
    /// A `GET /models` fetch finished: the account's model ids (Settings pick-list).
    Models(anyhow::Result<Vec<String>>),
    /// A batch recipe paste finished: the human summary (counts; Err on any failure).
    Pasted(anyhow::Result<String>),
}

/// Default endpoint for the image-role OAuth preset: a local Codex bridge
/// (e.g. CLIProxyAPI) that replays a ChatGPT-subscription OAuth token as an
/// OpenAI-compatible API on loopback. Only a default — the field stays editable
/// so a custom host/port works too.
const CODEX_BRIDGE_URL: &str = "http://127.0.0.1:8317/v1";

/// The stock OpenAI endpoint. Used to recognise a stale default when flipping the
/// image role into OAuth mode, so we can swap in the bridge URL automatically.
const OPENAI_DEFAULT_URL: &str = "https://api.openai.com/v1";

/// Editable buffers for the in-app Settings window. Key fields stay blank on
/// load and only overwrite the stored key when non-empty (the form never shows
/// an existing secret) — mirroring the web `/api/settings` contract.
#[derive(Default)]
struct SettingsForm {
    analysis_provider_api: bool, // false = OAuth (claude CLI), true = OpenAI-compatible API
    image_provider_oauth: bool,  // true = Codex bridge (ChatGPT sub), false = OpenAI-compatible API
    analysis_model: String,
    analysis_base_url: String,
    analysis_api_key: String,
    analysis_key_present: bool,
    image_model: String,
    image_base_url: String,
    image_gen_model: String,
    image_api_key: String,
    image_key_present: bool,
    status: String,
    // --- live model pick-lists (populated by the "Fetch models" button) ---
    chat_choices: Vec<String>,      // text/vision chat ids (proposer + api verifier)
    image_gen_choices: Vec<String>, // gpt-image-* ids (generative edits)
    fetching_models: bool,          // a GET /models worker is in flight
}

/// Build the option list for a model ComboBox: the live-fetched ids if we have
/// them, else a grounded fallback; the current value is always included so a
/// custom/manual id is never dropped from the menu.
fn model_opts(fetched: &[String], fallback: &[&str], current: &str) -> Vec<String> {
    let mut v: Vec<String> = if fetched.is_empty() {
        fallback.iter().map(|s| s.to_string()).collect()
    } else {
        fetched.to_vec()
    };
    if !current.trim().is_empty() && !v.iter().any(|x| x == current) {
        v.insert(0, current.to_string());
    }
    v
}

/// Every source type the app opens — ONE list shared by the file dialog, drag &
/// drop, and any future association, so they can't drift apart. Must cover
/// everything `decode::is_raw` accepts (the gallery + decoder already open
/// ORF/RW2/RAW; the dialog refusing them was pure drift).
const PHOTO_EXTS: [&str; 14] = [
    "arw", "dng", "raf", "nef", "cr2", "cr3", "orf", "rw2", "raw", "png", "tif", "tiff", "jpg",
    "jpeg",
];

fn is_photo_path(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PHOTO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn photo_file_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("Photos", &PHOTO_EXTS).pick_file()
}

/// Visualise a mask geometry on the image: linear = the zero→full vector with
/// end bars (solid = full-effect side); radial = the ellipse outline. Clipped
/// to the image rect by the painter.
/// `adj_inverted` is the owning LocalAdjustment's Invert flag: the engine
/// flips the coverage with it, so the directional markers must flip too —
/// otherwise the outline points at exactly the wrong side while the red
/// coverage layer contradicts it in the same frame.
fn draw_mask_overlay(
    ui: &egui::Ui,
    xf: ViewXform,
    geom: &MaskGeometry,
    adj_inverted: bool,
    lang: Lang,
) {
    let p = ui.painter_at(xf.rect);
    let stroke = egui::Stroke::new(2.0, ACCENT);
    match geom {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let a = xf.to_screen(*zero_x, *zero_y);
            let b = xf.to_screen(*full_x, *full_y);
            // Invert swaps which end carries the full effect.
            let (full, zero) = if adj_inverted { (a, b) } else { (b, a) };
            p.line_segment([a, b], stroke);
            let v = b - a;
            let len = v.length().max(1.0);
            let n = egui::vec2(-v.y / len, v.x / len) * 28.0;
            p.line_segment([zero - n, zero + n], egui::Stroke::new(1.0, ACCENT));
            p.line_segment([full - n, full + n], stroke);
            p.circle_filled(full, 4.0, ACCENT); // full-effect end
            p.circle_stroke(zero, 4.0, stroke); // untouched end
        }
        MaskGeometry::Radial { top, left, bottom, right, flipped, .. } => {
            let c = xf.to_screen((left + right) / 2.0, (top + bottom) / 2.0);
            let rx = (xf.to_screen(*right, 0.0).x - xf.to_screen(*left, 0.0).x).abs() / 2.0;
            let ry = (xf.to_screen(0.0, *bottom).y - xf.to_screen(0.0, *top).y).abs() / 2.0;
            p.add(egui::Shape::ellipse_stroke(c, egui::vec2(rx, ry), stroke));
            // Filled centre = the effect fills the INSIDE; hollow = the
            // geometry's own `flipped` and/or the adjustment's Invert put the
            // effect OUTSIDE the ellipse (the two compose, matching the engine).
            if flipped ^ adj_inverted {
                p.circle_stroke(c, 3.0, stroke);
            } else {
                p.circle_filled(c, 3.0, ACCENT);
            }
        }
        // Raster masks have no parametric outline to draw — mark the selection
        // with a badge instead of pretending a shape (rendering the raster as a
        // live translucent overlay is the A② follow-up).
        MaskGeometry::Bitmap { .. } => {
            p.text(
                xf.rect.left_top() + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                tr(lang, "▨ Bitmap mask"),
                egui::FontId::proportional(14.0),
                ACCENT,
            );
        }
    }
}

/// Pick radius (px) for on-image mask knobs — matches the crop handles' feel.
const HANDLE_HIT: f32 = 12.0;

/// Screen-space knob positions for on-image mask editing (geometry given in
/// VIEW space, i.e. already through geom_to_view). Handle ids: 0 = move
/// (linear midpoint / radial centre); linear 1 = zero end, 2 = full end;
/// radial 1..4 = left/top/right/bottom edge midpoints. Bitmap masks carry no
/// parametric knobs (empty).
fn mask_handle_points(geom: &MaskGeometry, xf: ViewXform) -> Vec<(u8, egui::Pos2)> {
    match *geom {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let a = xf.to_screen(zero_x, zero_y);
            let b = xf.to_screen(full_x, full_y);
            vec![(0, a + (b - a) * 0.5), (1, a), (2, b)]
        }
        MaskGeometry::Radial { top, left, bottom, right, .. } => {
            let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
            vec![
                (0, xf.to_screen(cx, cy)),
                (1, xf.to_screen(left, cy)),
                (2, xf.to_screen(cx, top)),
                (3, xf.to_screen(right, cy)),
                (4, xf.to_screen(cx, bottom)),
            ]
        }
        MaskGeometry::Bitmap { .. } => Vec::new(),
    }
}

/// Scale `tex_size` to fit a `max_w` × `avail_y` box (both dimensions — width
/// alone lets a portrait overflow the panel), never upscaling past 4×.
fn fit_in(tex_size: egui::Vec2, max_w: f32, avail_y: f32) -> egui::Vec2 {
    let s = (max_w / tex_size.x.max(1.0))
        .min(avail_y.max(1.0) / tex_size.y.max(1.0))
        .clamp(0.01, 4.0);
    tex_size * s
}

/// Section header text with an activity dot when the section holds non-neutral
/// values — a collapsed active adjustment must never be invisible.
fn section_title(base: &str, active: bool) -> String {
    if active {
        format!("{base}  ●")
    } else {
        base.to_string()
    }
}

/// Curve-editor channels: label + draw colour, indexed by `curve_channel`
/// (0 = master, then R/G/B — the recipe's tone/red/green/blue_curve fields).
// Tone-curve channel picker labels — skeleton keys, localized with `tr` at the
// render site (curve_editor). Colours are the on-curve accent, not localized.
const CURVE_CHANNELS: [(&str, egui::Color32); 4] = [
    ("Master", egui::Color32::from_gray(225)),
    ("Red", egui::Color32::from_rgb(235, 90, 90)),
    ("Green", egui::Color32::from_rgb(90, 205, 90)),
    ("Blue", egui::Color32::from_rgb(90, 130, 240)),
];

/// The recipe field behind a curve-editor channel index.
fn curve_points(recipe: &EditRecipe, ch: usize) -> &Vec<CurvePoint> {
    match ch {
        0 => &recipe.tone_curve,
        1 => &recipe.red_curve,
        2 => &recipe.green_curve,
        _ => &recipe.blue_curve,
    }
}

fn curve_points_mut(recipe: &mut EditRecipe, ch: usize) -> &mut Vec<CurvePoint> {
    match ch {
        0 => &mut recipe.tone_curve,
        1 => &mut recipe.red_curve,
        2 => &mut recipe.green_curve,
        _ => &mut recipe.blue_curve,
    }
}

/// Insert a curve control point keeping inputs sorted and UNIQUE — a second
/// point at the same input overwrites instead of duplicating (the engine's
/// piecewise-linear interp needs distinct inputs). Returns the point's index.
fn insert_curve_point(pts: &mut Vec<CurvePoint>, input: u8, output: u8) -> usize {
    match pts.binary_search_by_key(&input, |p| p.input) {
        Ok(i) => {
            pts[i].output = output;
            i
        }
        Err(i) => {
            pts.insert(i, CurvePoint { input, output });
            i
        }
    }
}

/// Move point `i` to (input, output), clamping input STRICTLY between its
/// neighbours so the control points always stay sorted with unique inputs.
fn drag_curve_point(pts: &mut [CurvePoint], i: usize, input: u8, output: u8) {
    let lo = if i > 0 { pts[i - 1].input.saturating_add(1) } else { 0 };
    let hi = if i + 1 < pts.len() { pts[i + 1].input.saturating_sub(1) } else { 255 };
    // lo > hi would need two neighbours ≤ 2 apart around an existing point —
    // unreachable via insert/drag above; keep the current input if it happens.
    if lo <= hi {
        pts[i].input = input.clamp(lo, hi);
    }
    pts[i].output = output;
}

/// Drag-reorder bookkeeping: element `from` moves to sit before `insert`
/// (both indices in the PRE-move order; `insert == len` appends). Returns the
/// element's final index plus the remap for every OTHER stored index (e.g.
/// the selection), composed as remove-at-`from` then insert-at-`to`.
/// `insert == from` and `insert == from + 1` are the two no-op drop slots —
/// callers skip the move entirely for those.
fn reorder_move(from: usize, insert: usize) -> (usize, impl Fn(usize) -> usize) {
    let to = if insert > from { insert - 1 } else { insert };
    (to, move |s: usize| {
        if s == from {
            to
        } else {
            let after_rm = if s > from { s - 1 } else { s };
            if after_rm >= to { after_rm + 1 } else { after_rm }
        }
    })
}

// --- geometric coordinate mapping (straighten + distortion) ------------------
// When straighten_deg ≠ 0 or lens_distortion ≠ 0 the After view shows the
// geometrically transformed frame (original → distortion-corrected →
// rotated + auto-cropped, see render.rs's C2 contract), but recipe masks, the
// paint canvas, fill/heal masks and base_preview pixels all live in the
// ORIGINAL frame (the engine applies masks before it remaps; fill/heal edit
// source pixels). These two maps convert between the spaces at the data
// boundaries, sharing the engine's own inscribed_dims / distort_norm formulas
// and rotation convention (clockwise-positive, y-down) so GUI and render can
// never disagree. recipe.crop stays in the view space — the export applies
// the user crop AFTER the geometric chain, so the crop tool needs no mapping.
// All maps are the identity when both controls are zero.

/// The lens-geometry half of the view mapping: the in-camera profile (the
/// composed map needs its distortion knots) plus the manual amount. Cloned
/// out of `geom_ctx` per gesture — a few 16-float Vecs, noise next to any
/// develop tick.
#[derive(Clone, Default)]
struct LensArg {
    profile: autoshop::recipe::LensProfile,
    amount: f32,
}

/// View normalized point → original-frame normalized point:
/// un-rotate (view → corrected), then the engine's forward sampling map
/// (corrected → original). Clamped once, at the end.
fn view_norm_to_orig(nx: f32, ny: f32, dims: (f32, f32), deg: f32, dist: &LensArg) -> (f32, f32) {
    let (w, h) = dims;
    let (cx, cy) = if deg == 0.0 {
        (nx, ny)
    } else {
        let (cw, ch) = autoshop::render::inscribed_dims(w, h, deg);
        let rad = deg.to_radians();
        let (s, c) = (rad.sin(), rad.cos());
        let (dx, dy) = ((nx - 0.5) * cw, (ny - 0.5) * ch);
        // Content was rotated clockwise; undo with the counter-clockwise matrix.
        (((c * dx + s * dy) / w) + 0.5, ((-s * dx + c * dy) / h) + 0.5)
    };
    let (ox, oy) = autoshop::render::lens_geom_norm(cx, cy, dims, &dist.profile, dist.amount);
    (ox.clamp(0.0, 1.0), oy.clamp(0.0, 1.0))
}

/// Original-frame normalized point → view normalized point: the engine's
/// inverse distortion map (original → corrected), then the forward rotation.
/// NOT clamped: an original point can legitimately fall outside the view
/// window (the painter clips overlays to the image rect anyway; original
/// content a barrel fix crops away comes back just outside the unit square).
fn orig_norm_to_view(nx: f32, ny: f32, dims: (f32, f32), deg: f32, dist: &LensArg) -> (f32, f32) {
    let (nx, ny) = autoshop::render::lens_ungeom_norm(nx, ny, dims, &dist.profile, dist.amount);
    if deg == 0.0 {
        return (nx, ny);
    }
    let (w, h) = dims;
    let (cw, ch) = autoshop::render::inscribed_dims(w, h, deg);
    let rad = deg.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rx = c * dx - s * dy; // clockwise forward
    let ry = s * dx + c * dy;
    (rx / cw.max(1e-3) + 0.5, ry / ch.max(1e-3) + 0.5)
}

/// A mask geometry mapped from the ORIGINAL frame into the view for on-screen
/// display (identity when straighten and distortion are both zero). Linear /
/// Radial anchor points map exactly; the radial ellipse is shown as the
/// bounding box of its transformed corners — display-only, and tight at the
/// small tilt angles and gentle distortions the sliders allow.
fn geom_to_view(geom: &MaskGeometry, dims: (f32, f32), deg: f32, dist: &LensArg) -> MaskGeometry {
    // Off = no manual amount AND no active profile distortion.
    let geom_off = dist.amount == 0.0
        && (!dist.profile.distortion_on || dist.profile.distortion.is_empty());
    if deg == 0.0 && geom_off {
        return geom.clone();
    }
    match *geom {
        // Raster masks carry no parametric anchor points to remap; their
        // overlay is a screen-anchored badge (see draw_mask_overlay).
        MaskGeometry::Bitmap { .. } => geom.clone(),
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let a = orig_norm_to_view(zero_x, zero_y, dims, deg, dist);
            let b = orig_norm_to_view(full_x, full_y, dims, deg, dist);
            MaskGeometry::Linear { zero_x: a.0, zero_y: a.1, full_x: b.0, full_y: b.1 }
        }
        MaskGeometry::Radial { top, left, bottom, right, feather, roundness, flipped } => {
            let pts = [
                orig_norm_to_view(left, top, dims, deg, dist),
                orig_norm_to_view(right, top, dims, deg, dist),
                orig_norm_to_view(left, bottom, dims, deg, dist),
                orig_norm_to_view(right, bottom, dims, deg, dist),
            ];
            let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (x, y) in pts {
                l = l.min(x);
                t = t.min(y);
                r = r.max(x);
                b = b.max(y);
            }
            MaskGeometry::Radial { top: t, left: l, bottom: b, right: r, feather, roundness, flipped }
        }
    }
}

/// Two API base URLs are the "same endpoint" ignoring whitespace / a trailing slash.
/// Used so model ids fetched with the image key are only offered to the analysis
/// picker when both roles point at the same server.
fn same_base(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// A model picker: a dropdown of `options` that writes the chosen id into `value`,
/// next to a text field so any custom id can still be typed. Both edit `value`.
fn model_picker(ui: &mut egui::Ui, salt: &str, value: &mut String, options: &[String], lang: Lang) {
    let sel = if value.trim().is_empty() { tr(lang, "Select…").to_owned() } else { value.clone() };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(sel)
        .width(200.0)
        .show_ui(ui, |ui| {
            for opt in options {
                ui.selectable_value(value, opt.clone(), opt.as_str());
            }
        });
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(170.0)
            .hint_text(tr(lang, "or type a custom id")),
    );
}

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
    refine: bool,     // adjust the CURRENT recipe vs propose from scratch
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
    nav_stash: HashMap<PathBuf, EditRecipe>,
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
    // --- library / gallery ---
    gallery: Vec<PathBuf>,          // sources in the working folder (sorted)
    gallery_dir: Option<PathBuf>,   // the working folder
    gallery_gen: u64,               // bumped on every folder load (thumb invalidation)
    selected: Option<usize>,        // index of the open gallery photo (for highlight)
    thumbs: HashMap<usize, egui::TextureHandle>, // decoded thumbnails by index
    thumb_requested: HashSet<usize>,             // indices already queued/decoded
    thumb_inflight: usize,                       // live thumbnail-decode threads
    edited_badge: HashMap<usize, bool>,          // cached "● edited" sidecar stat per index
    // Decoded-base LRU (recent last): base pixels + the photo's base-look knots.
    base_cache: Vec<(BaseCacheKey, OpenedBase)>,
    // --- region box-select (local-edit target on the After image) ---
    region: Option<[f32; 4]>,                      // normalized [left, top, right, bottom]
    region_drag: Option<(egui::Pos2, egui::Pos2)>, // transient drag (start, current) in screen px
    // --- retouch: mask painting + generative fill + heal ---
    paint_mode: bool,                      // brush-paint a mask (pauses box-select)
    brush: f32,                            // brush radius in After-image display px
    mask_paint: Option<image::RgbaImage>,  // painted overlay (red where painted), at preview res
    mask_tex: Option<egui::TextureHandle>, // overlay texture
    mask_dirty: bool,                      // re-upload the overlay
    mask_tex_xform: (f32, f32, bool), // (straighten, distortion, profile geometry on) at build
    paint_last: Option<(f32, f32)>,        // last brush point in mask px (line fill)
    fill_prompt: String,                   // generative-fill instruction
    fill_quality: usize,                   // 0=high 1=medium 2=low
    fill_fullres: bool,                    // composite onto the full-res develop
    heal_fullres: bool,                    // heal the full-res develop
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
    // (handle, drag start, crop at start, straighten° at start). Handles:
    // 0-3 corners, 4 move-inside, 5-8 edge midpoints (T/B/L/R), 9 = drag
    // OUTSIDE the box → rotate-straighten (the LR gesture).
    crop_drag: Option<(u8, egui::Pos2, [f32; 4], f32)>,
    mask_drag: Option<(u8, (f32, f32))>, // on-image mask knob drag: (handle, last pos in ORIG norm)
    // --- manual local adjustments (masks) ---
    sel_mask: Option<usize>,               // selected recipe.masks index (overlay + sliders)
    placing_mask: Option<(MaskKind, Option<usize>)>, // next image drag defines a mask (replace idx)
    place_start: Option<(f32, f32)>,       // placement drag origin, full-frame normalized
    // --- tone-curve editor ---
    curve_channel: usize,                  // CURVE_CHANNELS index: 0=master 1=R 2=G 3=B
    curve_drag: Option<usize>,             // control point being dragged (active channel)
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
    // --- variants (版本/变体): parallel renditions of the open photo ---
    // Original + any AI-generated / reverse-fitted versions. The active one
    // drives the sliders, histogram and canvas; switching is lossless (each
    // remembers its own base + recipe), so an AI develop no longer "reverts"
    // when you touch a slider — you're editing that variant's own base.
    variants: Vec<Variant>,
    active: usize,                         // index into `variants` (always valid once a photo is open)
    keep_recipe: bool,                     // one-shot: next Opened keeps recipe/variants (preview-res re-decode)
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

/// Slider feel classes — Lightroom's stepping grammar. `Int`: whole-number
/// domains (the ±100 family, 0..150, hue °); `Fine`: sub-unit precision
/// (Straighten °); `Frac`: small fractional domains (EV, 0..1 amounts);
/// `LogK`: the log-scaled Temp (K) track.
#[derive(Copy, Clone)]
enum SliderFeel {
    Int,
    Fine,
    Frac,
    LogK,
}

/// One undo/redo step: the recipe plus the active variant's pixel identity.
/// A baked pixel retouch (heal / clone / generative fill) swaps the variant's
/// base + origin outside the recipe — carrying them here makes Ctrl+Z walk
/// back through retouches too. History is per-variant (reset on switch), so a
/// step's pixels always belong to the variant it was recorded on; the base is
/// compared by `Arc::ptr_eq`, never by pixels.
#[derive(Clone, Default)]
struct UndoStep {
    recipe: EditRecipe,
    base: Option<Arc<image::DynamicImage>>,
    origin: Option<PathBuf>,
}

impl UndoStep {
    /// Same pixel identity? (Arc pointer + origin path — cheap, exact.)
    fn same_pixels(&self, other: &UndoStep) -> bool {
        let base_eq = match (&self.base, &other.base) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        base_eq && self.origin == other.origin
    }
}

/// One rendition in the variant strip — a Lightroom-style virtual copy /
/// Capture One variant, NOT a compositing layer (variants never blend; you
/// switch between them losslessly). A variant is fully defined by its base
/// pixels + its develop recipe.
struct Variant {
    kind: VariantKind,
    /// This variant's develop. The ACTIVE variant's recipe is mirrored in
    /// `AutoshopApp::recipe` (the live working copy the sliders edit); it is
    /// saved back here when you switch away.
    recipe: EditRecipe,
    /// Base pixels this variant develops from. Arc makes variant switches and
    /// background preview dispatch O(1); pixels remain immutable.
    /// `None` ⇒ the shared source neutral (`AutoshopApp::source_preview`).
    base: Option<Arc<image::DynamicImage>>,
    /// The ./out artifact behind a raster variant (the reimagine PNG) — the
    /// reverse-fit target and the full-res export source. `None` for
    /// source-based variants.
    origin: Option<PathBuf>,
    /// Small developed thumbnail for the strip (rebuilt for the active variant
    /// on every develop; built once for the others when created / left).
    thumb: Option<egui::TextureHandle>,
}

#[derive(Clone, Copy, PartialEq)]
enum VariantKind {
    Original,  // 原片 — the loaded RAW / image, your develop
    Generated, // AI 生成 — a whole-frame gpt-image restyle (look in the pixels)
    Fitted,    // 反推 — the generated look solved back into an editable recipe
}

impl VariantKind {
    /// Strip label (icon + English key; localised via `tr` at the render site).
    fn label(self) -> &'static str {
        match self {
            VariantKind::Original => "▣ Original",
            VariantKind::Generated => "✨ AI generated",
            VariantKind::Fitted => "◭ Reverse-fit",
        }
    }
}

/// The two mask geometries a user can place by dragging.
#[derive(Clone, Copy, PartialEq)]
enum MaskKind {
    Linear,
    Radial,
}

/// Crop aspect presets. `None` = free; `Some(r)` = width/height in PIXELS
/// (0.0 is the "original" sentinel, resolved against the photo at drag time).
// Display names are English skeleton keys (localised via `tr` at the render
// site); the ratio values are language-neutral. "1:1"…"9:16" have no ZH entry
// and fall back to themselves in both languages.
/// Export colour-space display names (indices = `exp_space`). Shared by the
/// Export section and the toolbar buttons' delivery-summary hover.
const EXPORT_SPACES: [&str; 3] =
    ["sRGB (universal)", "Display P3 (wide-gamut screens)", "Adobe RGB (print)"];

const CROP_ASPECTS: [(&str, Option<f32>); 11] = [
    ("Free", None),
    ("Original", Some(0.0)),
    ("1:1", Some(1.0)),
    ("3:2", Some(1.5)),
    ("2:3", Some(2.0 / 3.0)),
    ("4:3", Some(4.0 / 3.0)),
    ("3:4", Some(3.0 / 4.0)),
    ("5:4", Some(1.25)),
    ("4:5", Some(0.8)),
    ("16:9", Some(16.0 / 9.0)),
    ("9:16", Some(9.0 / 16.0)),
];

/// Screen ⇄ full-frame-normalized mapping through the visible uv window
/// (committed crop × zoom/pan). ALL image-space state — regions, mask
/// geometry, crop, the paint canvas — lives in full-frame normalized
/// coordinates, so every interaction handler maps through this one struct and
/// zoom/crop can never silently break any of them.
#[derive(Clone, Copy)]
struct ViewXform {
    rect: egui::Rect, // where the (visible part of the) image is drawn
    uv: egui::Rect,   // which full-frame region is visible (texture uv)
}

impl ViewXform {
    fn to_norm(self, p: egui::Pos2) -> (f32, f32) {
        let fx = ((p.x - self.rect.min.x) / self.rect.width().max(1.0)).clamp(0.0, 1.0);
        let fy = ((p.y - self.rect.min.y) / self.rect.height().max(1.0)).clamp(0.0, 1.0);
        (self.uv.min.x + fx * self.uv.width(), self.uv.min.y + fy * self.uv.height())
    }

    fn to_screen(self, nx: f32, ny: f32) -> egui::Pos2 {
        egui::pos2(
            self.rect.min.x
                + (nx - self.uv.min.x) / self.uv.width().max(1e-6) * self.rect.width(),
            self.rect.min.y
                + (ny - self.uv.min.y) / self.uv.height().max(1e-6) * self.rect.height(),
        )
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
            verdict: None,
            rationale: String::new(),
            style_strength: 0.30,
            hsl_tab: 0,
            grade_region: 0,
            guidance: String::new(),
            refine: false,
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
            gallery: Vec::new(),
            gallery_dir: None,
            gallery_gen: 0,
            selected: None,
            thumbs: HashMap::new(),
            thumb_requested: HashSet::new(),
            thumb_inflight: 0,
            edited_badge: HashMap::new(),
            base_cache: Vec::new(),
            saved_recipe: EditRecipe::default(),
            nav_stash: HashMap::new(),
            pasted_open: None,
            photo_knots: Vec::new(),
            photo_lens: Default::default(),
            before_curve: Vec::new(),
            confirm_quit: false,
            mask_name_buf: None,
            gallery_scroll_to: None,
            region: None,
            region_drag: None,
            paint_mode: false,
            brush: 30.0,
            mask_paint: None,
            mask_tex: None,
            mask_dirty: false,
            mask_tex_xform: (0.0, 0.0, false),
            paint_last: None,
            fill_prompt: String::new(),
            fill_quality: 0,
            fill_fullres: false,
            heal_fullres: false,
            view_mode: ViewMode::SideBySide,
            toasts: Vec::new(),
            histogram: None,
            last_title: String::new(),
            zoom: 1.0,
            pan: egui::vec2(0.5, 0.5),
            crop_mode: false,
            crop_aspect: 0,
            crop_drag: None,
            mask_drag: None,
            sel_mask: None,
            placing_mask: None,
            place_start: None,
            curve_channel: 0,
            curve_drag: None,
            multi_sel: HashSet::new(),
            copied: None,
            copied_from: None,
            paste_geometry: false,
            wb_picking: false,
            range_picking: None,
            clone_mode: false,
            clone_src: None,
            clone_fullres: false,
            variants: Vec::new(),
            active: 0,
            keep_recipe: false,
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
        self.toasts.push(Toast { text: text.into(), kind, born: Instant::now() });
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

/// 256-bin RGB+luma histogram of the already-packed preview (one bin per
/// 8-bit code value, Lightroom-grade resolution). One worker-side RGB8 buffer
/// feeds histogram, clipping and texture staging alike.
///
/// All four channels share ONE vertical scale (the global max), the way
/// Lightroom draws it — per-channel normalisation made relative bar heights
/// meaningless (a channel holding 200 pixels drew as tall as one holding two
/// million, so a colour cast read as balanced).
fn compute_histogram_rgb(rgb: &image::RgbImage) -> Vec<[f32; 4]> {
    const BINS: usize = 256;
    let mut counts = vec![[0u32; 4]; BINS];
    for px in rgb.pixels() {
        let (r, g, b) = (px[0] as usize, px[1] as usize, px[2] as usize);
        let luma = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) as usize;
        counts[r][0] += 1;
        counts[g][1] += 1;
        counts[b][2] += 1;
        counts[luma.min(BINS - 1)][3] += 1;
    }
    let mut max = 1u32;
    for bins in &counts {
        for &v in bins {
            max = max.max(v);
        }
    }
    counts
        .iter()
        .map(|bins| std::array::from_fn(|ch| bins[ch] as f32 / max as f32))
        .collect()
}

/// RGB8 → egui texture-ready colour image, without an intermediate RGBA image.
fn rgb_to_color_image(rgb: &image::RgbImage) -> egui::ColorImage {
    egui::ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw())
}

/// `DynamicImage` compatibility helper for cold paths (open / variant switch).
fn to_color_image(img: &image::DynamicImage) -> egui::ColorImage {
    rgb_to_color_image(&img.to_rgb8())
}

/// Clipping-warning layer over the DEVELOPED RGB8 preview (what export clips).
fn clipping_overlay_rgb(rgb: &image::RgbImage) -> egui::ColorImage {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for (i, p) in rgb.pixels().enumerate() {
        let px = &mut rgba[i * 4..i * 4 + 4];
        if p[0] >= 254 || p[1] >= 254 || p[2] >= 254 {
            px.copy_from_slice(&[255, 40, 40, 255]);
        } else if p[0] <= 1 && p[1] <= 1 && p[2] <= 1 {
            px.copy_from_slice(&[70, 110, 255, 255]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// Pure CPU preview build, safe to run off the egui thread. It deliberately
/// excludes texture-manager calls: `TextureHandle::set` stays on the UI thread.
fn build_preview(
    base: Arc<image::DynamicImage>,
    recipe: EditRecipe,
    show_clipping: bool,
) -> PreviewDone {
    let mut after = autoshop::render::develop_preview(&base, &recipe);
    if recipe.lens_profile.geometry_active() || recipe.lens_distortion != 0.0 {
        after = autoshop::render::apply_lens_geometry(
            &after,
            &recipe.lens_profile,
            recipe.lens_distortion,
        );
    }
    if recipe.straighten_deg != 0.0 {
        after = autoshop::render::rotate_straighten(&after, recipe.straighten_deg);
    }
    // into_rgb8 MOVES the buffer in the common no-geometry case (develop_preview
    // returns ImageRgb8) — to_rgb8() deep-copied ~3.3 MB per tick at 1280.
    let rgb = after.into_rgb8();
    // Histogram + clipping share the export's CROP: `recipe.crop` is
    // normalised on this same post-distortion/straighten frame (render.rs —
    // distortion → straighten → crop), so the statistics cover exactly the
    // region that ships. The pixel VALUES, however, are the 8-bit preview at
    // working resolution, not the 16-bit full-res export — downscaling can
    // average a small blown specular below the clip threshold (recorded
    // deferral; export-exact statistics need the full-res pipeline). The
    // preview image itself stays full-frame on purpose (immediate whole-frame
    // slider feedback).
    let crop_px = recipe.crop.map(|c| {
        let (w, h) = (rgb.width() as f32, rgb.height() as f32);
        let x0 = (c.left.clamp(0.0, 1.0) * w) as u32;
        let y0 = (c.top.clamp(0.0, 1.0) * h) as u32;
        let x1 = ((c.right.clamp(0.0, 1.0) * w) as u32).max(x0 + 1).min(rgb.width());
        let y1 = ((c.bottom.clamp(0.0, 1.0) * h) as u32).max(y0 + 1).min(rgb.height());
        (x0, y0, x1 - x0, y1 - y0)
    });
    let histogram = match crop_px {
        Some((x, y, w, h)) => {
            compute_histogram_rgb(&image::imageops::crop_imm(&rgb, x, y, w, h).to_image())
        }
        None => compute_histogram_rgb(&rgb),
    };
    let clipping = show_clipping.then(|| {
        let mut overlay = clipping_overlay_rgb(&rgb);
        // Outside the crop nothing exports — blank those warnings so a blown
        // sky the user already cropped away stops shouting.
        if let Some((x, y, w, h)) = crop_px {
            let transparent = egui::Color32::TRANSPARENT;
            for (i, px) in overlay.pixels.iter_mut().enumerate() {
                let (cx, cy) = (i as u32 % overlay.size[0] as u32, i as u32 / overlay.size[0] as u32);
                if cx < x || cx >= x + w || cy < y || cy >= y + h {
                    *px = transparent;
                }
            }
        }
        overlay
    });
    let thumb_rgb = image::imageops::thumbnail(&rgb, 96, 96);
    let thumb = rgb_to_color_image(&thumb_rgb);
    let after = rgb_to_color_image(&rgb);
    PreviewDone { base, recipe, rgb, after, histogram, clipping, thumb }
}

/// Stamp a filled brush dot into the paint mask (painted = translucent red).
fn stamp_dot(m: &mut image::RgbaImage, c: (f32, f32), r: f32) {
    let (w, h) = (m.width() as i32, m.height() as i32);
    let (cx, cy) = c;
    let r2 = r * r;
    let x0 = (cx - r).floor().max(0.0) as i32;
    let x1 = ((cx + r).ceil() as i32).min(w - 1);
    let y0 = (cy - r).floor().max(0.0) as i32;
    let y1 = ((cy + r).ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if dx * dx + dy * dy <= r2 {
                m.put_pixel(x as u32, y as u32, image::Rgba([255, 64, 64, 160]));
            }
        }
    }
}

/// Stamp a brush stroke between two points (interpolated dots — no gaps).
fn stamp_line(m: &mut image::RgbaImage, a: (f32, f32), b: (f32, f32), r: f32) {
    let dist = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let steps = (dist / (r * 0.5).max(1.0)).ceil().max(1.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        stamp_dot(m, (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t), r);
    }
}

/// One process-wide permit serialising >24 MP baked-raster thumbnail decodes
/// (see request_thumb). RAW thumbs never take it.
fn big_decode_gate() -> &'static std::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(Default::default)
}

/// Where the persistent 160px thumbnail for `src` lives, or `None` when the
/// source can't be stat'ed (no stable key → no caching). Rooted at the same
/// per-user store as develops/settings (`store_root()` honours the
/// AUTOSHOP_DATA_DIR override — a portable setup must not split its data
/// across two roots) and keyed by absolute path + mtime + size, so an
/// edited/replaced source misses. DefaultHasher is fine HERE (unlike the
/// develop keys): a hash that drifts across Rust releases only costs a
/// one-time re-decode.
fn thumb_cache_file(src: &std::path::Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(src).ok()?;
    let dir = autoshop::store::store_root().join("thumbs");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf()).hash(&mut h);
    meta.modified().ok().hash(&mut h);
    meta.len().hash(&mut h);
    Some(dir.join(format!("{:016x}.jpg", h.finish())))
}

/// Write-through for the thumbnail disk cache — best effort: a failed write
/// only costs a re-decode next session, so it warns rather than failing the
/// thumb. Once per session, an oversized cache dir (>10k files) is pruned by
/// age on a background thread.
fn save_thumb_cache(cache: &std::path::Path, thumb: &image::DynamicImage) {
    let Some(dir) = cache.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Err(e) = thumb.to_rgb8().save_with_format(cache, image::ImageFormat::Jpeg) {
        eprintln!("⚠ thumb cache write failed ({e}) — will re-decode next session");
        return;
    }
    static PRUNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let dir = dir.to_path_buf();
    PRUNED.get_or_init(|| {
        std::thread::spawn(move || {
            let Ok(rd) = std::fs::read_dir(&dir) else { return };
            let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let t = e.metadata().and_then(|m| m.modified()).ok()?;
                    Some((t, p))
                })
                .collect();
            if files.len() > 10_000 {
                files.sort_by_key(|(t, _)| *t);
                for (_, p) in files.iter().take(files.len() - 10_000) {
                    let _ = std::fs::remove_file(p);
                }
            }
        });
    });
}

/// Decoded-base LRU key: source path + requested preview edge + file mtime
/// (an on-disk change or a different resolution simply misses).
type BaseCacheKey = (PathBuf, u32, Option<std::time::SystemTime>);
/// A decoded preview base + the photo's camera-matched base-look knots
/// (`render::camera_base_knots`; empty = no base look) + its in-camera lens
/// profile (`pipeline::fresh_lens_profile`; default = none available).
type OpenedBase = (Arc<image::DynamicImage>, Vec<[f32; 2]>, autoshop::recipe::LensProfile);

/// How many decoded preview bases to keep for instant photo revisits (~3.3 MB
/// each at the default 1280 edge; culling flips between neighbours constantly).
const BASE_CACHE_CAP: usize = 4;

/// What ./out holds for a photo — the open path and the status line key off
/// this, and every outcome is DISTINGUISHED so nothing degrades silently
/// (an unparsable recipe.json used to fall through to the lossy XMP without
/// a word, and a neutral sidecar claimed "restored saved edits").
enum SavedDevelop {
    /// A sidecar with real edits (kind = which file; technical name, shown
    /// untranslated in the status line).
    Restored(EditRecipe, &'static str),
    /// recipe.json exists but does not parse (interrupted write, or written
    /// by a newer build — EditRecipe is deny_unknown_fields). The XMP
    /// fallback, when it held real edits, comes along.
    Unreadable { err: String, fallback: Option<(EditRecipe, &'static str)> },
    /// Sidecars exist but describe no effective edit (Reset-then-save
    /// leftovers, or a foreign neutral XMP).
    NoopOnly,
    /// No sidecar at all.
    Nothing,
}

/// The photo's saved develop from the central store (see `autoshop::store`):
/// the app-internal `recipe.json` first (lossless — bitmap masks, recolour
/// gains, roles all round-trip), else the Lightroom-compatible `<stem>.xmp`
/// re-imported through the reverse crs mapping (globals, curves, crop,
/// parametric masks — everything classic XMP can carry). Pre-store sidecars in
/// the legacy cwd-relative ./out are migrated in on first touch; a file the
/// migration could not move keeps being read in place, and the `kind` string
/// says which file actually answered.
fn read_saved_develop(src: &std::path::Path) -> SavedDevelop {
    autoshop::store::migrate_legacy(src);
    let mut any = false;
    let mut parse_err: Option<String> = None;
    let mut restored: Option<(EditRecipe, &'static str)> = None;
    for (rj, kind) in [
        (autoshop::store::recipe_target(src), "recipe.json"),
        (autoshop::store::legacy_recipe(src), "recipe.json (legacy ./out)"),
    ] {
        let Ok(text) = std::fs::read_to_string(&rj) else { continue };
        any = true;
        match serde_json::from_str::<EditRecipe>(&text) {
            Ok(mut r) => {
                if !r.is_noop() {
                    r.clamp();
                    // Store recipes reference rasters by bare file name —
                    // anchor them to the file they were loaded beside.
                    if let Some(base) = rj.parent() {
                        autoshop::store::resolve_mask_paths(&mut r, base);
                    }
                    restored = Some((r, kind));
                }
                // Neutral recipe.json: fall through — an XMP with real edits
                // may still exist beside it.
            }
            Err(e) => parse_err = Some(e.to_string()),
        }
        // Whatever the central file said (restored / neutral / damaged) is the
        // answer — a stale legacy file must never shadow it.
        break;
    }
    if let Some((r, kind)) = restored {
        return SavedDevelop::Restored(r, kind);
    }
    let mut fallback = None;
    for (xp, kind) in [
        (autoshop::pipeline::xmp_target(src), "XMP"),
        (autoshop::store::legacy_xmp(src), "XMP (legacy ./out)"),
    ] {
        let Ok(text) = std::fs::read_to_string(&xp) else { continue };
        any = true;
        let mut r = autoshop::xmp::xmp_to_recipe(&text);
        // A foreign / neutral sidecar parses to a no-op recipe — "restoring"
        // it would only produce a misleading status line.
        if !r.is_noop() {
            r.clamp();
            fallback = Some((r, kind));
        }
        break; // same rule: the file that answered decides
    }
    if let Some(err) = parse_err {
        return SavedDevelop::Unreadable { err, fallback };
    }
    match (fallback, any) {
        (Some((r, k)), _) => SavedDevelop::Restored(r, k),
        (None, true) => SavedDevelop::NoopOnly,
        (None, false) => SavedDevelop::Nothing,
    }
}

// The programmatic-overwrite backup gate lives in the LIB now
// (autoshop::store::backup_saved_develop) so the web and CLI enforce the same
// "never destroy an explicit save without a v<N> snapshot" contract — copy
// semantics + frozen rasters; see store.rs for the full contract.

/// The recipe a paste actually writes to `target`: the copied USER EDIT with
/// the base-look calibration resolved per target. `base_curve` is per-photo
/// (camera-matched at open), so:
///   * a BAKED target (PNG/JPEG/TIFF) gets NO curve — its pixels already
///     carry the camera look, and pasting a RAW's curve would apply the
///     camera tone twice;
///   * a target with a saved recipe (central store OR a not-yet-migrated
///     legacy ./out file) keeps its own curve — including the legacy empty
///     curve, which must keep rendering exactly as it was tuned;
///   * a RAW with no saved develop inherits the source's curve (same-camera
///     S curves — far closer than the dark base).
fn paste_recipe_for(target: &std::path::Path, pasted: &EditRecipe) -> EditRecipe {
    let mut r = pasted.clone();
    if !autoshop::decode::is_raw(target) {
        r.base_curve = Vec::new();
        r.lens_profile = Default::default();
        return r;
    }
    for p in [
        autoshop::store::recipe_target(target),
        autoshop::store::legacy_recipe(target),
    ] {
        if let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(saved) = serde_json::from_str::<EditRecipe>(&text)
        {
            r.base_curve = saved.base_curve;
            // A saved develop owns its profile verbatim — including any user
            // toggle and the legacy default-off.
            r.lens_profile = saved.lens_profile;
            return r;
        }
    }
    // No saved develop: the SOURCE photo's correction data is meaningless on
    // a different lens/focal — re-derive the target's own, stamped default-on
    // (paste transfers EDITS; the profile is per-photo calibration).
    r.lens_profile = autoshop::pipeline::fresh_lens_profile(target);
    r
}

/// "Does the canvas hold real USER edits vs this baseline?" — the ● marker,
/// the quit guard and the nav stash all key off this. `base_curve` is the
/// photo's camera calibration, not an edit: a curve-only difference (a fresh
/// stamp over a neutral-cleared baseline, a Generated variant's empty curve,
/// a paste that resolved the target's own curve) must not light ●, block
/// quit, or stash a canvas — that was exactly the "permanent ● with no way
/// to clear it" regression the adversarial review caught.
fn dirty_vs(canvas: &EditRecipe, baseline: &EditRecipe) -> bool {
    canvas != baseline
        && EditRecipe { base_curve: Vec::new(), ..canvas.clone() }
            != EditRecipe { base_curve: Vec::new(), ..baseline.clone() }
}

impl AutoshopApp {
    fn open_path(&mut self, path: PathBuf) {
        if self.busy {
            return;
        }
        // Leaving a photo with unsaved edits: stash the canvas for this
        // session so navigation can never silently destroy work (arrow keys /
        // thumbnail clicks used to drop the whole develop). Ctrl+S still owns
        // disk; returning to the photo restores the stash over the sidecar.
        if let Some(old) = self.src_path.clone()
            && old != path
            && dirty_vs(&self.recipe, &self.saved_recipe)
        {
            self.nav_stash.insert(old, self.recipe.clone());
        }
        let lang = self.lang;
        self.busy = true;
        self.src_path = Some(path.clone());
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
                    let (thumb, knots, lens) = if autoshop::decode::is_raw(&path) {
                        let full = autoshop::render::render_to_image(
                            &path,
                            &EditRecipe::default(),
                            None,
                        )?;
                        let thumb = full.thumbnail(edge, edge);
                        // In-camera lens profile (Sony 0x70xx): one cheap TIFF
                        // parse, stamped all-available-on (the user default).
                        let lens = autoshop::pipeline::fresh_lens_profile(&path);
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
                                autoshop::render::camera_base_knots(&est, &cam)
                            }
                            _ => Vec::new(),
                        };
                        (thumb, knots, lens)
                    } else {
                        (
                            autoshop::decode::load_image(&path)?.thumbnail(edge, edge),
                            Vec::new(),
                            Default::default(),
                        )
                    };
                    // Arc once here so every downstream sharer (variants, the
                    // preview worker) is an O(1) refcount bump, not a deep copy.
                    Ok((Arc::new(thumb), knots, lens))
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
        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let pos = self
            .base_cache
            .iter()
            .position(|((p, e, t), _)| p == path && *e == edge && *t == mtime)?;
        let entry = self.base_cache.remove(pos);
        let hit = entry.1.clone();
        self.base_cache.push(entry);
        Some(hit)
    }

    /// Remember a freshly decoded base. Called from the Opened handler — the px
    /// combo is disabled while busy, so `preview_edge` still describes the
    /// decode that just completed.
    fn remember_base(
        &mut self,
        path: &std::path::Path,
        edge: u32,
        base: &Arc<image::DynamicImage>,
        knots: &[[f32; 2]],
        lens: &autoshop::recipe::LensProfile,
    ) {
        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        self.base_cache.retain(|((p, e, _), _)| !(p == path && *e == edge));
        self.base_cache.push((
            (path.to_path_buf(), edge, mtime),
            (base.clone(), knots.to_vec(), lens.clone()),
        ));
        if self.base_cache.len() > BASE_CACHE_CAP {
            self.base_cache.remove(0); // least-recent first
        }
    }

    /// The active variant, if a photo is open.
    fn active_variant(&self) -> Option<&Variant> {
        self.variants.get(self.active)
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

    /// Make `self.active`'s recipe + base pixels the live working state and
    /// rebuild the before texture. Per-variant transient state (undo history,
    /// local selection, view) restarts — like a soft re-open; what persists is
    /// each variant's recipe + pixels. Shared by switch / push / delete.
    fn load_active(&mut self, ctx: &egui::Context) {
        let Some(v) = self.variants.get(self.active) else { return };
        self.recipe = v.recipe.clone();
        self.rationale = self.recipe.rationale.clone();
        // Base pixels: the variant's own baked raster, else the shared source
        // neutral (Original / Fitted re-develop the same negative).
        let baked = v.base.is_some();
        let base = v.base.clone().or_else(|| self.source_preview.clone());
        if let Some(base) = base {
            let (mw, mh) = base.dimensions();
            // Source variants compare against the canvas recipe's own base
            // calibration (empty for legacy / fit recipes — deliberate).
            let curve = if baked { Vec::new() } else { self.recipe.base_curve.clone() };
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
        self.status = trf(
            lang,
            "Switched to variant「{name}」 — variants are independent, switching is lossless",
            &[("name", name)],
        );
    }

    /// Append a variant and switch to it (its recipe/pixels become live).
    /// Saves the outgoing variant's edits first so nothing is lost.
    fn push_variant(&mut self, v: Variant, ctx: &egui::Context) {
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
        self.sel_mask = self.sel_mask.and_then(&f);
        self.range_picking = self.range_picking.and_then(&f);
        self.placing_mask = self.placing_mask.take().map(|(k, r)| (k, r.and_then(&f)));
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
        let lang = self.lang;
        let Some(src) = self.src_path.clone() else { return };
        // Number from DISK, not the cached list: the auto-backup writes v<N>
        // through the same counter, and a failure path that skipped
        // refresh_versions used to let this button compute the same N and
        // overwrite the only remaining copy of a backed-up save.
        let n = autoshop::store::list_versions(&src).last().map_or(1, |m| m + 1);
        match autoshop::pipeline::write_recipe(&src, &self.recipe, Some(Self::version_path(&src, n))) {
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
                r.clamp();
                // Snapshots name their rasters by bare file name, like the
                // working recipe — re-anchor them to the develop dir.
                if let Some(base) = p.parent() {
                    autoshop::store::resolve_mask_paths(&mut r, base);
                }
                self.recipe = r;
                self.resync_recipe_display();
                self.dirty = true;
                self.status = trf(lang, "Loaded version v{n} — Ctrl+Z returns to before the load", &[("n", &n.to_string())]);
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

    /// Reset undo history — call when a brand-new photo opens or the active
    /// variant changes (you can't undo across either). `committed` becomes the
    /// current head.
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
        // A wholesale recipe swap can shrink the mask list — an out-of-range
        // selection must not linger (every consumer bounds-checks, but the
        // panel would silently deselect anyway; do it deterministically).
        // Same-index-different-mask after undo is a known cosmetic residue —
        // mask identity isn't tracked across history steps.
        self.sel_mask = self.sel_mask.filter(|&i| i < self.recipe.masks.len());
    }

    /// Apply a history step: recipe always; the active variant's pixels only
    /// when the step's identity differs (so plain slider undos stay pixel-free
    /// and O(1)). A restored `base: None` reverts a retouched source variant
    /// to the shared source neutral.
    fn apply_step(&mut self, step: UndoStep, ctx: &egui::Context) {
        let pixels_differ = !step.same_pixels(&self.committed);
        self.committed = step.clone();
        self.recipe = step.recipe;
        if pixels_differ {
            if let Some(v) = self.variants.get_mut(self.active) {
                v.base = step.base;
                v.origin = step.origin;
            }
            self.refresh_active_pixels(ctx);
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
        let baked = v.base.is_some();
        let base = v.base.clone().or_else(|| self.source_preview.clone());
        if let Some(base) = base {
            let (mw, mh) = base.dimensions();
            let curve = if baked { Vec::new() } else { self.recipe.base_curve.clone() };
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
                self.status = tr(self.lang, "settings saved — applies to the next Analyze").into();
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
        // Everything quitting would lose: the stash + the open photo's canvas
        // (the live canvas outranks its own stale stash entry).
        let mut pending: Vec<(PathBuf, EditRecipe)> =
            self.nav_stash.iter().map(|(p, r)| (p.clone(), r.clone())).collect();
        if let Some(p) = self.src_path.clone()
            && dirty_vs(&self.recipe, &self.saved_recipe)
        {
            pending.retain(|(q, _)| q != &p);
            pending.push((p, self.recipe.clone()));
        }
        if pending.is_empty() {
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
        egui::Window::new(tr(lang, "● Unsaved edits"))
            .id(egui::Id::new("confirm_quit"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(trf(
                    lang,
                    "{n} photo(s) have edits that were never saved:",
                    &[("n", &pending.len().to_string())],
                ));
                // Parent folder + stem: two DSC_0431 from two shoots must be
                // distinguishable in the one dialog where the stakes are real.
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    for (p, _) in &pending {
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
                        .button(egui::RichText::new(tr(lang, "Save all & quit")).color(PILL))
                        .on_hover_text(tr(lang, "Enter · save every listed develop, then quit"))
                        .clicked();
                });
            });
        if save_quit {
            let mut failed: Option<String> = None;
            for (p, r) in &pending {
                let res = autoshop::pipeline::write_recipe(p, r, None).and_then(|_| {
                    if autoshop::decode::is_raw(p) {
                        autoshop::pipeline::write_xmp(p, r).map(|_| ())
                    } else {
                        Ok(())
                    }
                });
                if let Err(e) = res {
                    failed = Some(format!("{}: {e}", autoshop::pipeline::stem(p)));
                    break;
                }
            }
            match failed {
                None => {
                    self.nav_stash.clear();
                    self.saved_recipe = self.recipe.clone();
                    self.confirm_quit = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
                let n = photos
                    .iter()
                    .filter(|p| autoshop::store::migrate_legacy_from(&dir, p))
                    .count();
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
                "Language & Reverse-fit apply immediately. The provider sections below persist via 「Save settings」 to autoshop.local.json in your per-user Autoshop folder (never in a repo) and apply to the next Analyze.",
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
            ui.separator();
            ui.heading(tr(lang, "Analysis — the verifier"));
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Provider"));
                let r1 = ui.radio_value(&mut f.analysis_provider_api, false, "OAuth (Claude CLI)");
                let r2 = ui.radio_value(&mut f.analysis_provider_api, true, "API (OpenAI-compatible)");
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
                ui.radio_value(&mut f.image_provider_oauth, false, "API (OpenAI-compatible)");
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
                ui.add(egui::TextEdit::singleline(&mut f.image_api_key).password(true).hint_text(hint));
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
            Some(frame.clipping.unwrap_or_else(|| clipping_overlay_rgb(&frame.rgb)))
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
                let clip = clipping_overlay_rgb(rgb);
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
    /// mask, not WHERE it applies. Range masks include their masks-cleared
    /// reference recipe because their coverage genuinely depends on pixels.
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
        pre.masks.clear();
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
                egui::Color32::from_gray(130)
            } else {
                egui::Color32::from_gray(60)
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
        let mut changed = false;
        ui.horizontal(|ui| {
            for (i, (name, color)) in CURVE_CHANNELS.iter().enumerate() {
                if ui
                    .selectable_label(
                        self.curve_channel == i,
                        egui::RichText::new(tr(lang, name)).color(*color).small(),
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
                "Click to add a point · drag to move · drag outside the box to delete — preview / export / XMP all match",
            ))
            .weak()
            .small(),
        );
        changed
    }

    fn start_analyze(&mut self) {
        let Some(path) = self.src_path.clone() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang;
        self.busy = true;
        self.status = if self.refine {
            tr(lang, "refining your current edit with AI…").into()
        } else {
            tr(lang, "analyzing with AI (GPT + Claude)…").into()
        };
        let style = self.style_strength;
        // Free-text direction ("warmer, moodier") steers the proposal; when
        // `refine` is on, the AI ADJUSTS the current recipe instead of starting
        // from scratch. A box-selected region (if any) folds into the direction so
        // the AI masks exactly there — same prompt the web UI sends.
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
        let base = self.refine.then(|| {
            let mut r = self.recipe.clone();
            // The refine prompt/verifier work over the camera's embedded
            // preview, where the base look AND the lens corrections are
            // already IN the pixels — leaking either into the prompt would
            // double-apply them. Msg::Analyzed stamps both back afterwards.
            r.base_curve = Vec::new();
            r.lens_profile = Default::default();
            r
        });
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
                Msg::Analyzed(Box::new(res))
            },
            |e| Msg::Analyzed(Box::new(Err(e))),
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
        // developed.tif beside the source RAW.
        if let Err(e) = autoshop::pipeline::guard_readonly(&out, &path) {
            let t = e.to_string();
            self.status = t.clone();
            self.toast(ToastKind::Error, t);
            return;
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
        self.spawn_worker(
            move || {
                let res = (|| {
                    if let Some(p) = out.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    // SCUNet AI denoise (python sidecar) runs before the develop when on.
                    let opts = denoise.then(|| {
                        autoshop::denoise::DenoiseOpts::from_config(&autoshop::config::Config::load(), None, 1.0)
                    });
                    autoshop::render::render_to_file(&path, &recipe, &out, opts.as_ref(), Some(&export))?;
                    Ok::<String, anyhow::Error>(out.display().to_string())
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
                    // One raster per (photo, target): re-running a segmentation
                    // refreshes the same file, and the existing mask follows it.
                    // Lives in the photo's develop dir (path-keyed) so two
                    // same-named photos can no longer swap rasters; segment_file
                    // creates the dir itself (pipeline::ensure_parent).
                    let mask = autoshop::store::raster_target(&src, &format!("mask-{target}"));
                    let run = autoshop::segment::segment_file(&opts, &tmp, &mask);
                    let _ = std::fs::remove_file(&tmp);
                    run?;
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
        // The open photo's LIVE canvas outranks its sidecar — same rule as
        // start_paste. Without this, batching the photo you are looking at
        // renders the stale (or absent) recipe.json, visibly diverging from
        // the screen.
        let live = self.src_path.clone().map(|p| (p, self.recipe.clone()));
        self.spawn_worker(
            move || {
                let res = (|| {
                    let total = targets.len();
                    let (mut okn, mut errs) = (0usize, Vec::<String>::new());
                    for p in &targets {
                        let one = (|| -> anyhow::Result<()> {
                            let recipe = if let Some(lr) =
                                live.as_ref().and_then(|(lp, lr)| (lp == p).then_some(lr))
                            {
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
                            let out = autoshop::pipeline::default_out(p, "developed", &ext);
                            autoshop::pipeline::ensure_parent(&out)?;
                            autoshop::render::render_to_file(p, &recipe, &out, None, Some(&export))?;
                            Ok(())
                        })();
                        match one {
                            Ok(()) => okn += 1,
                            Err(e) => errs.push(format!("{}: {e}", autoshop::pipeline::stem(p))),
                        }
                        let _ = tx.send(Msg::BatchProgress { done: okn + errs.len(), total });
                    }
                    if errs.is_empty() {
                        Ok(trf(lang, "./out — batch {n} done", &[("n", &okn.to_string())]))
                    } else {
                        anyhow::bail!(
                            "{}",
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
        if self.recipe.is_noop() {
            // A file already missing IS the desired end state; any other
            // removal failure (lock, permissions) must not let us claim the
            // edits were cleared — the sidecar would resurrect them on reopen.
            let del = |p: &std::path::Path| match std::fs::remove_file(p) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e),
            };
            let mut removed = false;
            let mut first_err: Option<std::io::Error> = None;
            for p in [
                autoshop::store::recipe_target(&path),
                autoshop::pipeline::xmp_target(&path),
                autoshop::store::legacy_recipe(&path),
                autoshop::store::legacy_xmp(&path),
            ] {
                match del(&p) {
                    Ok(b) => removed |= b,
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    }
                }
            }
            match first_err {
                None => {
                    self.edited_badge.clear();
                    self.saved_recipe = EditRecipe::default();
                    self.nav_stash.remove(&path);
                    self.status = if removed {
                        tr(lang, "neutral recipe — saved edits cleared (saved files removed)").into()
                    } else {
                        tr(lang, "neutral recipe — nothing to save").into()
                    };
                }
                Some(e) => {
                    let t = trf(
                        lang,
                        "could not clear the saved edits: {err}",
                        &[("err", &e.to_string())],
                    );
                    self.status = t.clone();
                    self.toast(ToastKind::Error, t);
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
        match autoshop::pipeline::write_recipe(&path, &self.recipe, None) {
            Ok(rp) => {
                self.edited_badge.clear(); // the open photo just gained its badge
                self.saved_recipe = self.recipe.clone();
                self.nav_stash.remove(&path);
                let mut s = if raw {
                    match autoshop::pipeline::write_xmp(&path, &self.recipe) {
                        Ok(p) => trf(
                            lang,
                            "XMP + recipe saved → {path}",
                            &[("path", &p.display().to_string())],
                        ),
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
                // An in-place heal/clone/fill bakes pixels into the variant's
                // origin raster — the saved develop cannot carry them (recipe/
                // XMP are parametric), so a bare "saved" would overpromise the
                // restore.
                if self.active_variant().is_some_and(|v| v.origin.is_some()) {
                    s.push_str(tr(
                        lang,
                        " · note: retouched pixels live in the baked master, not the saved develop — Export to keep them",
                    ));
                }
                self.status = s;
            }
            Err(e) => {
                let t = trf(lang, "save failed: {err}", &[("err", &e.to_string())]);
                self.status = t.clone();
                self.toast(ToastKind::Error, t); // a failed save must be seen
            }
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
            let live = paste_recipe_for(&open, chosen);
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
                    for path in &targets {
                        let step = || -> anyhow::Result<bool> {
                            // Per-target base-look resolution (paste_recipe_for):
                            // paste copies the user edit, never the source
                            // photo's camera calibration over a saved one. The
                            // clipboard's own photo keeps its bitmap masks.
                            let chosen =
                                if Some(path) == copied_from.as_ref() { &recipe_full } else { &recipe };
                            let r = paste_recipe_for(path, chosen);
                            autoshop::pipeline::write_recipe(path, &r, None)?;
                            if autoshop::decode::is_raw(path) {
                                autoshop::pipeline::write_xmp(path, &r)?;
                                return Ok(true);
                            }
                            Ok(false)
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
                        Ok(trf(
                            lang,
                            "Recipe pasted to {ok} photos ({xmp} XMP) → develop store",
                            &[("ok", &okn.to_string()), ("xmp", &xmpn.to_string())],
                        ))
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
                    let keep = std::mem::take(&mut self.keep_recipe);
                    match *boxed {
                    Ok((base, knots, lens)) => {
                        self.busy = false;
                        if let Some(p) = self.src_path.clone() {
                            let edge = self.preview_edge.clamp(640, 8192);
                            self.remember_base(&p, edge, &base, &knots, &lens);
                        }
                        // Kept for Reset: "reset" = the fresh-open look, so it
                        // needs this photo's knots even when the restored
                        // recipe (legacy save) deliberately carries none.
                        self.photo_knots = knots.clone();
                        self.photo_lens = lens.clone();
                        if keep {
                            // Preview-resolution re-decode: the SOURCE pixels just
                            // changed resolution — keep the whole variant set,
                            // recipe, undo history and view (zoom included: you
                            // switched to 4096px to inspect 1:1, losing the zoom
                            // would defeat the point). Refresh the base a
                            // source-based active variant develops from; a
                            // baked-raster variant keeps its own pixels.
                            let (mw, mh) = base.dimensions();
                            self.source_preview = Some(base.clone());
                            let active_source =
                                self.active_variant().is_none_or(|v| v.base.is_none());
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
                            }
                            self.last_rgb = None; // resolution changed under the frame
                            self.dirty = true;
                            self.status = trf(
                                lang,
                                "Preview resolution {px}px — re-decoded",
                                &[("px", &self.preview_edge.to_string())],
                            );
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
                            let saved = self
                                .src_path
                                .as_deref()
                                .map(read_saved_develop)
                                .unwrap_or(SavedDevelop::Nothing);
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
                                    open_note = Some(trf(
                                        lang,
                                        "recipe.json is unreadable ({err}) — edits NOT fully restored; Ctrl+S would overwrite it",
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
                            if stamp {
                                if !knots.is_empty() {
                                    recipe.base_curve = knots.clone();
                                }
                                // Same rule as the curve: fresh opens and
                                // XMP-only restores get the photo's in-camera
                                // lens profile (all-available-on); a saved
                                // recipe.json keeps its own verbatim.
                                recipe.lens_profile = lens.clone();
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
                            if let Some(st) =
                                self.src_path.as_ref().and_then(|p| self.nav_stash.remove(p))
                            {
                                recipe = st;
                                from_stash = true;
                            }
                            self.recipe = recipe.clone();
                            self.rationale = recipe.rationale.clone();
                            self.variants = vec![Variant {
                                kind: VariantKind::Original,
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
                            self.last_rgb = None; // retained frame was the old photo's
                            self.reset_history(); // a new photo starts a fresh undo history
                            self.region = None; // and a fresh local-edit selection
                            self.region_drag = None;
                            // View + tool state is per-photo.
                            self.zoom = 1.0;
                            self.pan = egui::vec2(0.5, 0.5);
                            self.disarm_tools();
                            self.sel_mask = None;
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
                        }
                    }
                }},
                Msg::Developed(boxed) => self.finish_redevelop(ctx, *boxed),
                Msg::Analyzed(boxed) => match *boxed {
                    Ok((recipe, verdict)) => {
                        // The recipe arrives with its base_curve already
                        // stamped by produce_recipe (saved-curve-first, else a
                        // fresh estimate) — the single authority all three
                        // surfaces share; the canvas must not override it.
                        // One exception, canvas-side only: a BAKED variant
                        // (Generated / retouched base) already carries the
                        // camera look in its pixels, so developing it through
                        // the RAW's curve would cook the tone twice — the
                        // canvas copy drops the curve; the persist below still
                        // writes the full stamped recipe (it is the RAW's
                        // saved develop, and reopening lands on the source).
                        let stamped = recipe;
                        let mut canvas = stamped.clone();
                        if self.active_variant().is_some_and(|v| v.base.is_some()) {
                            canvas.base_curve = Vec::new();
                            canvas.lens_profile = Default::default();
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
                        self.status = match self.src_path.clone() {
                            // The backup gate comes FIRST: if the existing save
                            // cannot be snapshotted, it is not overwritten —
                            // the analyze result stays on the canvas only.
                            // Persist `stamped`, not the canvas copy: the
                            // sidecar is the RAW's develop and keeps its curve.
                            Some(p) => match autoshop::store::backup_saved_develop(&p, Some(&stamped)) {
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
                                            self.edited_badge.clear();
                                            self.saved_recipe = stamped.clone();
                                            self.nav_stash.remove(&p);
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
                            },
                            None => tr(lang, "AI develop applied").into(),
                        };
                    }
                    Err(e) => {
                        self.fail(tr(lang, "analyze failed"), e);
                    }
                },
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
                                    (bare == new_name).then(|| {
                                        *p = path_s.clone();
                                        i
                                    })
                                }
                                _ => None,
                            },
                        );
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
                        self.status = trf(
                            lang,
                            "AI「{what}」mask added — adjust its sliders (exposure / contrast / saturation…) to take effect",
                            &[("what", &label)],
                        );
                    }
                    Err(e) => {
                        self.fail(tr(lang, "AI segmentation failed"), e);
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
                        self.thumb_inflight = 0;
                        self.edited_badge.clear(); // badge stats belong to the old folder
                        self.selected = None;
                        self.multi_sel.clear(); // indices belong to the old folder
                        self.busy = false;
                        self.status = if n == 1 {
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
                        if let Ok(im) = *img {
                            let tex = ctx.load_texture(
                                format!("thumb{idx}"),
                                to_color_image(&im),
                                egui::TextureOptions::LINEAR,
                            );
                            self.thumbs.insert(idx, tex);
                        }
                    }
                }
                Msg::Retouched(boxed) => match *boxed {
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
                                // fill/heal/clone: a pixel touch-up of the CURRENT
                                // rendition — bake it into the active variant's base
                                // (so later slider edits develop OVER the retouched
                                // pixels) AND repoint its origin at the saved
                                // full-res artifact, so export / reverse-fit / a
                                // further retouch all follow the retouched pixels
                                // rather than the pre-retouch source (WYSIWYG).
                                let img = Arc::new(img);
                                let (mw, mh) = img.dimensions();
                                if let Some(v) = self.variants.get_mut(self.active) {
                                    v.base = Some(img.clone());
                                    v.origin = Some(saved);
                                }
                                // A baked (retouched) base already carries the
                                // camera look in its pixels — no curve.
                                self.set_before(ctx, &img, &[]);
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
                            }
                        }
                        self.done(msg);
                    }
                    Err(e) => {
                        self.fail(tr(lang, "retouch failed"), e);
                    }
                },
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
                    Ok(prompt) => {
                        // Into the Direction box: ready to restyle OTHER photos.
                        self.guidance = prompt;
                        self.done(tr(lang, "Style prompt extracted → filled into Direction (also saved ./out/<stem>.style.txt)"));
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
        // Keep the frame loop alive while any worker (analyze/export/thumbs/models) runs.
        if self.busy || self.thumb_inflight > 0 || self.settings.fetching_models {
            ctx.request_repaint();
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
        // lose to the display rounding. EV: 0.01-drag / 0.1-arrow. LogK's
        // nudge is ~1% of the current Kelvin (≈55 K at 5500) — a fixed step
        // would be sub-pixel at one end of a log track and huge at the other.
        let (snap, nudge, decimals) = match feel {
            SliderFeel::Int => (1.0, 1.0, 0usize),
            SliderFeel::Fine => (0.1, 0.1, 1),
            SliderFeel::Frac => (0.01, range / 100.0, 2),
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
        // Lightroom's "sync settings" for the whole working folder.
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.src_path.is_some(), |ui| {
                if ui
                    .small_button(tr(lang, "⎘ Copy recipe"))
                    .on_hover_text(tr(lang, "Copy every develop setting from the current photo"))
                    .clicked()
                {
                    self.copied = Some(self.recipe.clone());
                    self.copied_from = self.src_path.clone();
                    self.status = tr(lang, "Recipe copied — Ctrl+click to pick several, then “Paste to selected”").to_string();
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
                        SEL_BG
                    } else if is_multi {
                        SEL_BG_DIM
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
                                    ui.painter().rect_filled(rect, 3.0, egui::Color32::from_gray(24));
                                    to_request.push(i);
                                }
                                ui.vertical(|ui| {
                                    let mut name = egui::RichText::new(autoshop::pipeline::stem(path)).small();
                                    if is_sel {
                                        name = name.strong().color(PILL);
                                    }
                                    ui.label(name);
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
                                            ui.label(egui::RichText::new(tr(lang, "✓ selected")).color(PILL).small());
                                        }
                                        if baked {
                                            ui.label(egui::RichText::new("PNG/TIFF").color(PILL).small());
                                        }
                                        if edited {
                                            ui.label(egui::RichText::new(tr(lang, "● edited")).color(PILL).small());
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
        let ai_active = self.verdict.is_some() || !self.guidance.is_empty() || self.refine;
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
                    .on_hover_text(tr(lang, "Free-text direction for AI Analyze and Reimagine — e.g. warmer and moodier"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.guidance)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr(lang, "e.g. warmer and moodier, lift the shadows")),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.refine, tr(lang, "Refine"))
                        .on_hover_text(tr(lang, "Adjust the CURRENT edit instead of proposing from scratch"));
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
                    let mut custom_wb = self.recipe.temperature_k.is_some();
                    if ui.checkbox(&mut custom_wb, tr(lang, "Custom white balance (off = as-shot)")).changed() {
                        self.recipe.temperature_k = if custom_wb { Some(5500.0) } else { None };
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
                if let Some(mut k) = self.recipe.temperature_k
                    && Self::slider_log(ui, lang, tr(lang, "Temp (K)"), &mut k, 2000.0, 40000.0, 5500.0)
                {
                    self.recipe.temperature_k = Some(k);
                    changed = true;
                }
                let r = &mut self.recipe;
                changed |= Self::slider(ui, lang, tr(lang, "Tint"), &mut r.tint, -100.0, 100.0, 0.0);
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
                let r = &mut self.recipe;
                changed |= Self::slider(ui, lang, tr(lang, "Sharpening"), &mut r.sharpening, 0.0, 150.0, 0.0);
                changed |=
                    Self::slider(ui, lang, tr(lang, "Noise Reduction"), &mut r.noise_reduction, 0.0, 100.0, 0.0);
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
                                // Refill the PAIR: a saved recipe holding only
                                // ca_r would otherwise enable CA that renders
                                // nothing (geometry_active needs both).
                                lp.ca_r = photo.ca_r.clone();
                                lp.ca_b = photo.ca_b.clone();
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
                    egui::ComboBox::from_id_salt("crop_aspect")
                        .selected_text(tr(lang, CROP_ASPECTS[self.crop_aspect].0))
                        .width(70.0)
                        .show_ui(ui, |ui| {
                            for (i, (name, _)) in CROP_ASPECTS.iter().enumerate() {
                                ui.selectable_value(&mut self.crop_aspect, i, tr(lang, name));
                            }
                        });
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
                        "Once in (R): drag corner/edge handles to resize, drag inside to move, drag OUTSIDE the box to rotate-straighten; preview, export and XMP all match. Straighten auto-crops the black corners.",
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
                let lin_armed = matches!(self.placing_mask, Some((MaskKind::Linear, None)));
                if ui.selectable_label(lin_armed, tr(lang, "＋ Linear gradient")).on_hover_text(tr(lang, "Drag on the image: start = unaffected side, end = fully-applied side")).clicked() {
                    self.disarm_tools();
                    if !lin_armed {
                        self.placing_mask = Some((MaskKind::Linear, None));
                        self.status = tr(lang, "Drag on the image to draw a linear gradient (start unaffected → end fully applied)").into();
                    }
                }
                let rad_armed = matches!(self.placing_mask, Some((MaskKind::Radial, None)));
                if ui.selectable_label(rad_armed, tr(lang, "＋ Radial gradient")).on_hover_text(tr(lang, "Drag on the image to draw an elliptical area")).clicked() {
                    self.disarm_tools();
                    if !rad_armed {
                        self.placing_mask = Some((MaskKind::Radial, None));
                        self.status = tr(lang, "Drag on the image to draw a radial (elliptical) area").into();
                    }
                }
            });
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
                        let label = format!("{base} · {kind}");
                        ui.dnd_drag_source(ui.id().with(("mask_row", i)), i, |ui| {
                            ui.label("☰");
                        })
                        .response
                        .on_hover_text(tr(self.lang, "Drag to reorder"));
                        let row = ui.selectable_label(self.sel_mask == Some(i), label);
                        if row.hovered() {
                            self.hover_mask = Some(i);
                        }
                        if row.clicked() {
                            self.sel_mask =
                                if self.sel_mask == Some(i) { None } else { Some(i) };
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
                        let redraw_armed =
                            matches!(self.placing_mask, Some((k, Some(j))) if k == kind && j == i);
                        if ui
                            .selectable_label(redraw_armed, tr(lang, "↻ Redraw"))
                            .on_hover_text(tr(lang, "Re-drag this mask's area on the image"))
                            .clicked()
                        {
                            self.disarm_tools();
                            if !redraw_armed {
                                self.placing_mask = Some((kind, Some(i)));
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
                });
                // Radial geometry: edge feather + inside/outside flip — both
                // engine-rendered, but neither had a control before (placement
                // hardcodes feather 0.5; only an XMP import or the AI could
                // ever change either).
                if let MaskGeometry::Radial { feather, flipped, .. } =
                    &mut self.recipe.masks[i].mask
                {
                    let mut geo_ch = false;
                    ui.horizontal(|ui| {
                        geo_ch |= Self::slider(
                            ui,
                            lang,
                            tr(lang, "Edge feather"),
                            feather,
                            0.0,
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
                    if geo_ch {
                        self.overlay_stale = true;
                        changed = true;
                    }
                }
                // --- Range Mask（LR 范围蒙版）: refines WHERE the geometry applies —
                // final weight = geometry × range, live in preview + export + XMP.
                // `changed` is watched across the whole block: range edits are
                // COVERAGE edits, so the red overlay must rebuild too — only
                // the geometry controls used to set overlay_stale, leaving the
                // wash stale against the new range until an unrelated toggle.
                let range_changed_before = changed;
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
                            self.status = tr(lang, "Colour range: click the colour to pick in the image").into();
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
                            let mut f = ((*lo - *lo_outer) + (*hi_outer - *hi)) * 0.5;
                            let mut ch = false;
                            ch |= Self::slider(ui, lang, tr(lang, "Lum. low"), lo, 0.0, 1.0, 0.0);
                            ch |= Self::slider(ui, lang, tr(lang, "Lum. high"), hi, 0.0, 1.0, 1.0);
                            ch |= Self::slider(ui, lang, tr(lang, "Feather"), &mut f, 0.0, 0.5, 0.1);
                            if ch {
                                if *lo > *hi {
                                    std::mem::swap(lo, hi);
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
                                if ui.small_button(label).on_hover_text(tr(lang, "Click the colour to pick in the image (the same colour at other brightnesses is also selected)")).clicked() {
                                    want_pick = true;
                                }
                            });
                            changed |= Self::slider(ui, lang, tr(lang, "Tolerance"), amount, 0.0, 1.0, 0.5);
                        }
                        None => {}
                    }
                    if want_pick {
                        self.disarm_tools();
                        if !picking_this {
                            self.range_picking = Some(i);
                            self.status = tr(lang, "Colour range: click the colour to pick in the image").into();
                        }
                    }
                }
                if changed && !range_changed_before {
                    self.overlay_stale = true; // range IS coverage — rebuild the wash
                }
                let m = &mut self.recipe.masks[i];
                changed |= Self::slider(ui, lang, tr(lang, "Amount"), &mut m.amount, 0.0, 1.0, 1.0);
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
                    match autoshop::store::delete_version(&src, n) {
                        Ok(()) => {
                            self.refresh_versions();
                            self.status =
                                trf(lang, "Version v{n} deleted", &[("n", &n.to_string())]);
                        }
                        Err(e) => {
                            let t = trf(
                                lang,
                                "Delete v{n} failed: {err}",
                                &[("n", &n.to_string()), ("err", &e.to_string())],
                            );
                            self.status = t.clone();
                            self.toast(ToastKind::Error, t);
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
        let scale = disp.x / vis_px.x.max(1.0); // display px per image px

        // Caption row: mode hint left, zoom readout + Fit / 1:1 right. Every
        // armed-tool hint names its exit (Esc), and the placement hint speaks
        // the armed KIND's language instead of calling a radial a gradient.
        let hint = if comparing {
            tr(lang, "Before (source) — release B to return to editing")
        } else if self.crop_mode {
            tr(lang, "Crop — drag corners/edges to resize, inside to move, outside to rotate · Esc to exit")
        } else if let Some((kind, _)) = self.placing_mask {
            match kind {
                MaskKind::Linear => tr(lang, "Linear gradient — drag from the unaffected side to the fully-applied side · Esc to exit"),
                MaskKind::Radial => tr(lang, "Radial gradient — drag to draw an elliptical area · Esc to exit"),
            }
        } else if self.wb_picking {
            tr(lang, "WB eyedropper — click a spot that should be neutral grey/white · Esc to exit")
        } else if self.range_picking.is_some() {
            tr(lang, "Colour range — click the colour to pick in the image · Esc to exit")
        } else if self.clone_mode {
            tr(lang, "Stamp — Alt+click to set the source · drag to brush the area to cover · Esc to exit")
        } else if self.paint_mode {
            tr(lang, "Brush — paint over the area to fill / heal · Esc to exit")
        } else {
            tr(lang, "After — drag a box = local AI · scroll to zoom · space/middle-drag to pan · hold B to compare")
        };
        // An armed tool must be visible at NORMAL contrast — .weak().small()
        // is the passive-help style, and it was the only always-on indicator
        // when the arming control sat in a collapsed section.
        let armed = !comparing && self.tool_armed();
        ui.horizontal(|ui| {
            if armed {
                ui.label(egui::RichText::new(hint).color(ACCENT).small());
            } else {
                ui.label(egui::RichText::new(hint).weak().small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("1:1").on_hover_text(tr(lang, "Preview pixels 1:1 (double-click the image to toggle)")).clicked() {
                    self.zoom = (vis_px.x * self.zoom / disp.x).max(1.0);
                }
                if ui.small_button("Fit").on_hover_text(tr(lang, "Fit the whole image to the canvas (double-click the image to toggle)")).clicked() {
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
                    self.keep_recipe = true; // re-decode, keep the edit
                    self.open_path(p);
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
                self.zoom = (vis_px.x / disp.x).max(1.0);
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
            || self.sel_mask.and_then(|i| self.recipe.masks.get(i)).is_some_and(|m| {
                let (dims, deg, dist) = self.geom_ctx();
                resp.hover_pos().is_some_and(|p| {
                    mask_handle_points(&geom_to_view(&m.mask, dims, deg, &dist), xf)
                        .iter()
                        .any(|(_, hp)| hp.distance(p) <= HANDLE_HIT)
                })
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
        if !self.crop_mode
            && self.placing_mask.is_none()
            && let Some(m) = self.sel_mask.and_then(|i| self.recipe.masks.get(i))
        {
            let (dims, deg, dist) = self.geom_ctx();
            let vg = geom_to_view(&m.mask, dims, deg, &dist);
            let geom_active = dist.amount != 0.0
                || (dist.profile.distortion_on && !dist.profile.distortion.is_empty());
            if let MaskGeometry::Radial { top, left, bottom, right, flipped, .. } = &m.mask
                && (deg != 0.0 || geom_active)
            {
                // The engine evaluates the ellipse in the ORIGINAL frame; under
                // straighten/distortion its image is a rotated/warped curve —
                // not the axis-aligned ellipse the bbox mapping yields. Sample
                // it parametrically and draw the true outline. (Knobs keep the
                // bbox positions; HANDLE_HIT absorbs the difference.)
                let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
                let (rx, ry) = ((right - left) / 2.0, (bottom - top) / 2.0);
                let pts: Vec<egui::Pos2> = (0..48)
                    .map(|k| {
                        let th = k as f32 / 48.0 * std::f32::consts::TAU;
                        let (vx, vy) = orig_norm_to_view(
                            cx + rx * th.cos(),
                            cy + ry * th.sin(),
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
    /// inverse solver (`render::solve_wb_from_neutral` — the same 5500 K
    /// anchored forward model the render applies) turns it into Temp + Tint.
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
            let rgb = base.to_rgb8();
            let (w, h) = rgb.dimensions();
            let (cx, cy) = (
                (nx * (w.saturating_sub(1)) as f32).round() as i64,
                (ny * (h.saturating_sub(1)) as f32).round() as i64,
            );
            let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
            for dy in -2..=2i64 {
                for dx in -2..=2i64 {
                    let (x, y) = (cx + dx, cy + dy);
                    if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                        let p = rgb.get_pixel(x as u32, y as u32);
                        for c in 0..3 {
                            acc[c] += p[c] as f32 / 255.0;
                        }
                        n += 1.0;
                    }
                }
            }
            if n == 0.0 {
                return;
            }
            [acc[0] / n, acc[1] / n, acc[2] / n]
        };
        let (k, tint) = autoshop::render::solve_wb_from_neutral(px);
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
            let Some(base) = &self.base_preview else { return };
            let mut pre = self.recipe.clone();
            pre.masks.clear();
            let rgb = autoshop::render::develop_preview(base, &pre).to_rgb8();
            let (w, h) = rgb.dimensions();
            let (cx, cy) = (
                (nx * (w.saturating_sub(1)) as f32).round() as i64,
                (ny * (h.saturating_sub(1)) as f32).round() as i64,
            );
            let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
            for dy in -2..=2i64 {
                for dx in -2..=2i64 {
                    let (x, y) = (cx + dx, cy + dy);
                    if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                        let p = rgb.get_pixel(x as u32, y as u32);
                        for c in 0..3 {
                            acc[c] += p[c] as f32 / 255.0;
                        }
                        n += 1.0;
                    }
                }
            }
            if n == 0.0 {
                return;
            }
            [acc[0] / n, acc[1] / n, acc[2] / n]
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
            "Colour range: sampled — the 「Tolerance」 slider adjusts the selection width",
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
    fn handle_mask_edit(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) -> bool {
        if self.region_drag.is_some() {
            return false;
        }
        let Some(i) = self.sel_mask.filter(|&i| i < self.recipe.masks.len()) else {
            self.mask_drag = None;
            return false;
        };
        let (dims, deg, dist) = self.geom_ctx();
        let view_geom = geom_to_view(&self.recipe.masks[i].mask, dims, deg, &dist);
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
            // quick grabs don't fall through to box-select.
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if let Some(h) = pick(origin) {
                self.mask_drag = Some((h, orig_at(p)));
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
            match &mut self.recipe.masks[i].mask {
                MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => match h {
                    1 => (*zero_x, *zero_y) = (cl(cur.0), cl(cur.1)),
                    2 => (*full_x, *full_y) = (cl(cur.0), cl(cur.1)),
                    _ => {
                        *zero_x = cl(*zero_x + dx);
                        *zero_y = cl(*zero_y + dy);
                        *full_x = cl(*full_x + dx);
                        *full_y = cl(*full_y + dy);
                    }
                },
                MaskGeometry::Radial { top, left, bottom, right, .. } => {
                    const MIN_SIZE: f32 = 0.01;
                    match h {
                        1 => *left = cl(cur.0).min(*right - MIN_SIZE),
                        2 => *top = cl(cur.1).min(*bottom - MIN_SIZE),
                        3 => *right = cl(cur.0).max(*left + MIN_SIZE),
                        4 => *bottom = cl(cur.1).max(*top + MIN_SIZE),
                        _ => {
                            *left = cl(*left + dx);
                            *right = cl(*right + dx);
                            *top = cl(*top + dy);
                            *bottom = cl(*bottom + dy);
                        }
                    }
                }
                MaskGeometry::Bitmap { .. } => {}
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

    fn handle_region_select(&mut self, _ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.region_drag = Some((p, p));
            }
        } else if resp.dragged() {
            if let (Some(p), Some((s, _))) = (resp.interact_pointer_pos(), self.region_drag) {
                self.region_drag = Some((s, p));
            }
        } else if resp.drag_stopped() {
            if let Some((s, e)) = self.region_drag.take() {
                // The region feeds the AI's mask prompt — ORIGINAL frame space.
                let ((bw, bh), deg, dist) = self.geom_ctx();
                let map = |p: egui::Pos2| {
                    let (nx, ny) = xf.to_norm(p);
                    view_norm_to_orig(nx, ny, (bw, bh), deg, &dist)
                };
                let (sn, en) = (map(s), map(e));
                let (l, r) = (sn.0.min(en.0), sn.0.max(en.0));
                let (t, b) = (sn.1.min(en.1), sn.1.max(en.1));
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
        } else if resp.clicked() {
            self.region = None; // a plain click clears the region
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
        let cur = self
            .recipe
            .crop
            .map(|c| [c.left, c.top, c.right, c.bottom])
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);

        // Pixel aspect ratio (w/h) requested by the preset; "原始" resolves here.
        let aspect = CROP_ASPECTS[self.crop_aspect.min(CROP_ASPECTS.len() - 1)]
            .1
            .map(|r| if r == 0.0 { tex_size.x / tex_size.y.max(1.0) } else { r });

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
                    Some(if r.contains(p) { 4 } else { 9 })
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
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if let Some(h) = pick_handle(&cur, origin) {
                self.crop_drag = Some((h, p, cur, self.recipe.straighten_deg));
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
                        // Width drives; height follows the ratio; if the
                        // derived height leaves the frame, shrink both to fit.
                        let top_corner = h == 0 || h == 1;
                        let mut w_n = (x - ax).abs();
                        let mut h_n = w_n / rn;
                        let room = if top_corner { ay } else { 1.0 - ay };
                        if h_n > room {
                            h_n = room;
                            w_n = h_n * rn;
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
        let r = egui::Rect::from_min_max(xf.to_screen(c[0], c[1]), xf.to_screen(c[2], c[3]))
            .intersect(xf.rect);
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
        let grid = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70));
        for i in 1..3 {
            let t = i as f32 / 3.0;
            p.line_segment(
                [egui::pos2(r.min.x + t * r.width(), r.min.y), egui::pos2(r.min.x + t * r.width(), r.max.y)],
                grid,
            );
            p.line_segment(
                [egui::pos2(r.min.x, r.min.y + t * r.height()), egui::pos2(r.max.x, r.min.y + t * r.height())],
                grid,
            );
        }
        p.rect_stroke(r, 0.0, egui::Stroke::new(1.5, egui::Color32::WHITE));
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
    /// vector defines a linear gradient (start = untouched side) or the
    /// bounding box of a radial. Commits into `recipe.masks` — the SAME field
    /// the AI writes, so render + XMP need nothing new.
    fn handle_place_mask(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        let Some((kind, replace)) = self.placing_mask else { return };
        // Mask geometry lives in the ORIGINAL frame (the engine composites
        // masks before the geometric remap) — map pointer positions out of the view.
        let (dims, deg, dist) = self.geom_ctx();
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let (nx, ny) = xf.to_norm(p);
            self.place_start = Some(view_norm_to_orig(nx, ny, dims, deg, &dist));
        }
        let Some(s) = self.place_start else { return };
        let Some(p) = resp.interact_pointer_pos() else { return };
        let e = {
            let (nx, ny) = xf.to_norm(p);
            view_norm_to_orig(nx, ny, dims, deg, &dist)
        };
        let geom = match kind {
            MaskKind::Linear => autoshop::recipe::MaskGeometry::Linear {
                zero_x: s.0,
                zero_y: s.1,
                full_x: e.0,
                full_y: e.1,
            },
            MaskKind::Radial => autoshop::recipe::MaskGeometry::Radial {
                top: s.1.min(e.1),
                left: s.0.min(e.0),
                bottom: s.1.max(e.1),
                right: s.0.max(e.0),
                feather: 0.5,
                roundness: 0.0,
                flipped: false,
            },
        };
        // Live placement preview: no owning adjustment yet → markers upright.
        draw_mask_overlay(ui, xf, &geom_to_view(&geom, dims, deg, &dist), false, self.lang);
        if resp.drag_stopped() {
            match replace {
                Some(i) if i < self.recipe.masks.len() => {
                    // Redraw replaces the AREA only — a radial's tuned feather /
                    // roundness / flipped are slider state, and silently
                    // resetting them made ↻ Redraw destructive.
                    let mut geom = geom;
                    if let (
                        autoshop::recipe::MaskGeometry::Radial {
                            feather, roundness, flipped, ..
                        },
                        autoshop::recipe::MaskGeometry::Radial {
                            feather: kept_f,
                            roundness: kept_r,
                            flipped: kept_fl,
                            ..
                        },
                    ) = (&mut geom, &self.recipe.masks[i].mask)
                    {
                        *feather = *kept_f;
                        *roundness = *kept_r;
                        *flipped = *kept_fl;
                    }
                    self.recipe.masks[i].mask = geom;
                }
                _ => {
                    let n = self.recipe.masks.len();
                    let name = trf(self.lang, "Manual {n}", &[("n", &(n + 1).to_string())]);
                    self.recipe.masks.push(autoshop::recipe::LocalAdjustment {
                        mask: geom,
                        name,
                        ..Default::default()
                    });
                    self.sel_mask = Some(n);
                }
            }
            self.placing_mask = None;
            self.place_start = None;
            self.dirty = true;
            self.status =
                tr(self.lang, "mask placed — pull its sliders in 「Local Masks」 at left (all 0 now, no visible effect yet)").into();
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
        }
        self.paint_last = None;
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
            }
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
        let Some(m) = self.mask_paint.as_mut() else { return };
        let (mw, mh) = (m.width() as f32, m.height() as f32);
        // The canvas lives in the ORIGINAL frame (fill/heal edit source
        // pixels), so pointer positions map out of the transformed view.
        let to_mask = |p: egui::Pos2| {
            let (nx, ny) = xf.to_norm(p);
            let (ox, oy) = view_norm_to_orig(nx, ny, dims, deg, &dist);
            (ox * mw, oy * mh)
        };
        let brush_mask = (brush * (xf.uv.width() * mw) / xf.rect.width().max(1.0)).max(1.0);
        if resp.dragged() || resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let cur = to_mask(p);
                match self.paint_last {
                    Some(prev) => stamp_line(m, prev, cur, brush_mask),
                    None => stamp_dot(m, cur, brush_mask),
                }
                self.paint_last = Some(cur);
                self.mask_dirty = true;
            }
        } else {
            // A quick tap is a CLICK to egui (<6 px / <0.8 s — never a drag):
            // stamp a single dot so marking dust spots doesn't require a smear.
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                stamp_dot(m, to_mask(p), brush_mask);
                self.mask_dirty = true;
            }
            // No stroke is in flight on any non-drag frame — clearing here
            // (not just on drag_stopped) also covers strokes interrupted by
            // Esc / hold-B / an Alt source-pick, whose drag_stopped frame
            // never reaches this handler and used to leave a stale anchor
            // that drew a full-width connecting streak into the next stroke.
            self.paint_last = None;
        }
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
        self.busy = true;
        self.status = if self.fill_fullres {
            tr(lang, "generative fill (full-res render)… (slow, minutes)").into()
        } else {
            tr(lang, "generative fill via gpt-image… (~15-40s)").into()
        };
        let quality = ["high", "medium", "low"][self.fill_quality.min(2)].to_string();
        let full_res = self.fill_fullres;
        let edge = self.preview_edge.clamp(640, 8192); // bake at the working res, not a fixed 1280
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let out = autoshop::pipeline::default_out(&path, "retouch", "png");
                    let mask_tmp = std::env::temp_dir()
                        .join(format!("autoshop_gui_fill_{}.png", std::process::id()));
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
                Msg::Retouched(Box::new(res))
            },
            |e| Msg::Retouched(Box::new(Err(e))),
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
        self.busy = true;
        self.status = if use_mask {
            tr(lang, "healing painted area…").into()
        } else {
            tr(lang, "AI healing… (~10-30s)").into()
        };
        let full_res = self.heal_fullres;
        let edge = self.preview_edge.clamp(640, 8192); // bake at the working res, not a fixed 1280
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let out = autoshop::pipeline::default_out(&path, "heal", "png");
                    let mask_tmp = match mask_png {
                        Some(bytes) => {
                            let t = std::env::temp_dir()
                                .join(format!("autoshop_gui_heal_{}.png", std::process::id()));
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
                Msg::Retouched(Box::new(res))
            },
            |e| Msg::Retouched(Box::new(Err(e))),
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
        self.busy = true;
        self.status = tr(lang, "Cloning… (local pixel compute)").into();
        let full_res = self.clone_fullres;
        let edge = self.preview_edge.clamp(640, 8192); // bake at the working res, not a fixed 1280
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let out = autoshop::pipeline::default_out(&path, "clone", "png");
                    let mask_tmp = std::env::temp_dir()
                        .join(format!("autoshop_gui_clone_{}.png", std::process::id()));
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
                Msg::Retouched(Box::new(res))
            },
            |e| Msg::Retouched(Box::new(Err(e))),
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
        // First FREE ./out name — probing the filesystem (not a live-variant
        // count) so delete-then-reimagine can't reuse a number whose PNG a
        // surviving variant still points at. Bounded by a hard cap.
        let out = {
            let mut n = 0u32;
            loop {
                let tag = if n == 0 { "reimagine".to_string() } else { format!("reimagine-{}", n + 1) };
                let cand = autoshop::pipeline::default_out(&path, &tag, "png");
                if !cand.exists() || n >= 999 {
                    break cand;
                }
                n += 1;
            }
        };
        let prompt = {
            let g = self.guidance.trim();
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
            tr(lang, "AI generating… (gpt-image, ~15–60s; hi-res input needs a full-frame develop first)").into();
        let edge = self.preview_edge.clamp(640, 8192);
        self.spawn_worker(
            move || {
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
                Msg::Retouched(Box::new(res))
            },
            |e| Msg::Retouched(Box::new(Err(e))),
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
                    // Snapshot the existing save BEFORE the fit runs: the
                    // zoned pass rewrites mask-zone-sky.png in place, so a
                    // snapshot taken afterwards would freeze the NEW raster
                    // under the OLD recipe — the v<N> could no longer
                    // reproduce the look it promises. Copy semantics make
                    // this safe even if the fit then fails (the working
                    // recipe.json is untouched; at worst an extra snapshot
                    // exists). `None` = unconditional: the fit result is not
                    // known yet, and a fit is never a byte-identical rewrite.
                    let backed = match &src_path {
                        Some(p) => autoshop::store::backup_saved_develop(p, None),
                        None => Ok(None),
                    };
                    // Zoned sky pass only when enabled AND the photo has a real
                    // path (the mask raster needs a stem) AND the snapshot
                    // above succeeded — the zoned pass REWRITES the live
                    // mask-zone-sky.png, so running it past a failed backup
                    // would mutate the saved develop's raster while we refuse
                    // to touch its recipe (the same contract, one file over).
                    // The raster is named mask-ZONE-sky — distinct from
                    // 「AI select sky」's mask-sky.png on purpose: they used to
                    // share one file, and the fit's cleanup deleted a raster an
                    // existing bitmap mask still referenced. Everything that
                    // can go wrong inside degrades to the global fit with a
                    // rationale note — never an error.
                    let rep = match (zoned && backed.is_ok(), &src_path) {
                        (true, Some(p)) => {
                            let cfg = autoshop::config::Config::load();
                            let seg =
                                autoshop::segment::SegmentOpts::from_config(&cfg, "sky");
                            let mask = autoshop::store::raster_target(p, "mask-zone-sky");
                            autoshop::pipeline::ensure_parent(&mask)?;
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
                        // Persist the fit losslessly: recipe.json carries the
                        // bitmap zone masks + recolour gains the XMP cannot,
                        // so reopening the photo restores the whole fit. The
                        // snapshot above is the gate — if it FAILED, skip the
                        // persist entirely (the fit stays on the canvas with
                        // ● unsaved; Ctrl+S overwrites explicitly).
                        match &backed {
                            Ok(backed) => {
                                autoshop::pipeline::write_recipe(p, &rep.recipe, None)?;
                                persisted = true;
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
                            Err(e) => {
                                note.push_str(&trf(
                                    lang,
                                    " · NOT persisted: backing up your existing save failed ({err}) — Ctrl+S to save explicitly",
                                    &[("err", &e.to_string())],
                                ));
                                if zoned {
                                    note.push_str(tr(
                                        lang,
                                        " · zoned sky pass skipped to protect the saved raster (the fit ran globally)",
                                    ));
                                }
                            }
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
        self.busy = true;
        self.status = tr(self.lang, "Extracting style prompt… (vision, ~5-20s)").into();
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<String> {
                    let cfg = autoshop::config::Config::load();
                    let jpg = |img: &image::DynamicImage| -> anyhow::Result<Vec<u8>> {
                        let mut buf = Vec::new();
                        image::DynamicImage::ImageRgb8(img.thumbnail(768, 768).to_rgb8())
                            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)?;
                        Ok(buf)
                    };
                    let target = autoshop::decode::load_image(&tgt)?;
                    let prompt = autoshop::advisor::describe_style(&cfg, &jpg(&base)?, &jpg(&target)?)?;
                    if let Some(p) = &src_path {
                        let out = autoshop::pipeline::default_out(p, "style", "txt");
                        autoshop::pipeline::ensure_parent(&out)?;
                        std::fs::write(&out, &prompt)?;
                    }
                    Ok(prompt)
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
                                ui.label(if active { label.strong().color(PILL) } else { label });
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
                ui.add_enabled_ui(!self.busy, |ui| {
                    if ui
                        .button(tr(lang, "✨ Generate image"))
                        .on_hover_text(tr(lang,
                            "Repaint the whole image with gpt-image (uses the Direction text above as the style). \
                             Repainted pixels = not faithful; the result is added as an 「AI generated」 variant \
                             at the bottom and switched to, so you can keep tweaking without reverting. Models that \
                             accept any size (gpt-image-2) reach ~8MP, others ~1.5K. Needs OPENAI_API_KEY.",
                        ))
                        .clicked()
                    {
                        self.start_reimagine();
                    }
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
                                 style prompt: auto-fills Direction (ready to Reimagine other photos) and saves \
                                 ./out/<stem>.style.txt.",
                            ))
                            .clicked()
                        {
                            self.start_style_prompt();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Uses the Direction above as the style. After generating, use 「Reverse-fit recipe」 to turn \
                         the look into sliders + XMP (the full-resolution way).",
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
                    ui.checkbox(&mut self.fill_fullres, tr(lang, "Full-res"))
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
                        "Paint the area, write what belongs there, then Remove/Fill. Needs OPENAI_API_KEY.",
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
                    ui.checkbox(&mut self.heal_fullres, tr(lang, "Full-res"))
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
                    ui.checkbox(&mut self.clone_fullres, tr(lang, "Full-res"))
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
            let unsaved_open =
                self.src_path.is_some() && dirty_vs(&self.recipe, &self.saved_recipe);
            if unsaved_open || !self.nav_stash.is_empty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_quit = true;
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
        // Global shortcuts. Skip while a widget is focused so the Direction text
        // field keeps its own text editing / undo, and while the quit-confirm
        // layer is up (R/Tab/[ ] must not mutate the very state the user is
        // deciding whether to save). Ctrl+Z/Y = undo/redo, Ctrl+O = open,
        // Ctrl+E = export, Ctrl+S = save XMP, ←/→ = walk the gallery,
        // R = crop, [ ] = brush size, Tab = hide the side panels — the
        // keyboard grammar of every desktop photo editor.
        if !self.confirm_quit && ctx.memory(|m| m.focused()).is_none() {
            let (mut do_undo, mut do_redo, mut do_open, mut do_export, mut do_xmp) =
                (false, false, false, false, false);
            let (mut do_escape, mut do_overlay, mut do_clip) = (false, false, false);
            let (mut do_cheatsheet, mut do_crop, mut do_panels) = (false, false, false);
            let mut nav: i32 = 0;
            let mut brush_delta: f32 = 0.0;
            // [ / ] belong to the active brush tool; when none is armed the
            // keys stay unconsumed (free for egui / future bindings).
            let brush_tool = self.paint_mode || self.clone_mode;
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) { do_redo = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) { do_redo = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) { do_undo = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::O) { do_open = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::E) { do_export = true; }
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S) { do_xmp = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) { nav = 1; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) { nav = -1; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) { do_escape = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::O) { do_overlay = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::J) { do_clip = true; }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::R) { do_crop = true; }
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
            if do_undo { self.undo(ctx); }
            if do_redo { self.redo(ctx); }
            // R mirrors the crop button exactly (incl. the one-tool-at-a-time
            // disarms); no photo → nothing to crop.
            if do_crop && self.src_path.is_some() {
                let on = !self.crop_mode;
                self.disarm_tools();
                self.crop_mode = on;
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
            // Esc closes an open cheat-sheet FIRST (the topmost transient),
            // else leaves whatever on-image tool is active (the universal
            // editor exit); painted canvases/samples stay for resuming.
            if do_escape && self.show_shortcuts {
                self.show_shortcuts = false;
                do_escape = false;
            }
            // Settings is the next transient in the stack (the cheat-sheet's
            // own hint promises Esc closes windows).
            if do_escape && self.show_settings {
                self.show_settings = false;
                do_escape = false;
            }
            if do_escape && (self.tool_armed() || self.region_drag.is_some()) {
                // disarm_tools also kills the transient gesture anchors — a
                // grab or stroke surviving Esc used to hijack the next drag.
                self.disarm_tools();
                self.region_drag = None;
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
                self.save_xmp();
            }
            if nav != 0 && !self.busy && !self.gallery.is_empty() {
                let cur = self.selected.map(|i| i as i32).unwrap_or(-nav.min(0));
                let next = (cur + nav).clamp(0, self.gallery.len() as i32 - 1);
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
                self.toast(
                    ToastKind::Error,
                    trf(self.lang, "unsupported file type: {path}", &[("path", &p.display().to_string())]),
                );
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
                let ready = self.src_path.is_some() && !self.busy;
                if ui
                    .add_enabled(ready, egui::Button::new(tr(lang, "AI Analyze")))
                    .on_hover_text(tr(lang, "AI proposes a recipe (GPT proposal + validation), written into the sliders — undoable. Direction / Refine / Style live in the AI section of the Develop panel."))
                    .clicked()
                {
                    self.start_analyze();
                }
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
                    // A BAKED variant keeps no curve (its pixels already carry
                    // the camera look).
                    let baked = self.active_variant().is_some_and(|v| v.base.is_some());
                    let base_curve = if baked { Vec::new() } else { self.photo_knots.clone() };
                    // The lens profile is the same kind of calibration: Reset
                    // re-stamps the photo's own (baked variants carry the
                    // corrections in their pixels already — none re-applied).
                    let lens_profile =
                        if baked { Default::default() } else { self.photo_lens.clone() };
                    self.recipe = EditRecipe { base_curve, lens_profile, ..EditRecipe::default() };
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
                        tr(lang, "Ctrl+E · full-resolution render to ./out (follows the current variant's pixels); settings in the Export section")
                    ))
                    .clicked()
                {
                    self.start_export();
                }
                if ui
                    .add_enabled(ready, egui::Button::new(tr(lang, "Download…")))
                    .on_hover_text(format!(
                        "{}\n{summary}",
                        tr(lang, "Save as… (full-resolution export to a path you choose)")
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
                    .on_hover_text(tr(lang, "Ctrl+S · save this photo's develop (recipe + a Lightroom/ACR XMP for RAW) to your develop store"))
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
                }
                // The unsaved marker: canvas differs from the saved develop
                // (base_curve excluded — calibration is not an edit, dirty_vs).
                // It sits FIRST — before the batch bar and the status text —
                // so neither can ever push it out of view at narrow widths.
                if self.src_path.is_some() && dirty_vs(&self.recipe, &self.saved_recipe) {
                    ui.label(
                        egui::RichText::new(tr(self.lang, "● unsaved"))
                            .color(ui.visuals().warn_fg_color),
                    )
                    .on_hover_text(tr(
                        self.lang,
                        "Edits differ from your saved develop — Ctrl+S saves; switching photos keeps them for this session only",
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
            && self.active_variant().is_none_or(|v| v.base.is_none())
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
            // gesture. Focus-gated like every other shortcut: typing a "b"
            // into Direction must not flash the Before mid-word.
            let comparing = ctx.memory(|m| m.focused()).is_none()
                && ctx.input(|i| i.key_down(egui::Key::B));

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
            egui::Window::new(tr(lang, "⌨ Shortcuts"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    // Runtime table (not `const`): the ZH column is resolved by
                    // `tr` at draw time. ASCII key combos + "Fit ↔ 1:1" carry no
                    // natural-language words, so they stay literal.
                    let rows: [(&str, &str); 22] = [
                        ("Ctrl+O", tr(lang, "Open photo")),
                        ("Ctrl+E", tr(lang, "Export (settings in the Export section)")),
                        ("Ctrl+S", tr(lang, "Save develop (recipe + XMP for RAW)")),
                        ("Ctrl+Z / Ctrl+Y", tr(lang, "Undo / Redo")),
                        ("← / →", tr(lang, "Step through the library")),
                        ("R", tr(lang, "Enter / exit crop")),
                        ("[ / ]", tr(lang, "Brush size (paint / clone armed)")),
                        ("Tab", tr(lang, "Hide / show the side panels")),
                        (tr(lang, "Hover a slider + ↑/↓"), tr(lang, "Nudge its value (Shift ×10)")),
                        (tr(lang, "B (hold)"), tr(lang, "Compare original")),
                        ("O", tr(lang, "Toggle mask overlay")),
                        ("J", tr(lang, "Toggle clipping warning")),
                        ("Esc", tr(lang, "Exit tool / close this window")),
                        ("F1 / ?", tr(lang, "This cheat-sheet")),
                        (tr(lang, "Scroll"), tr(lang, "Zoom (toward cursor)")),
                        (tr(lang, "Double-click canvas"), "Fit ↔ 1:1"),
                        (tr(lang, "Space+drag / middle-drag"), tr(lang, "Pan")),
                        (tr(lang, "Drag when zoomed"), tr(lang, "Pan (Ctrl+drag = box-select)")),
                        (tr(lang, "Alt+click"), tr(lang, "Sample clone source")),
                        (tr(lang, "Slider double-click"), tr(lang, "Reset to its default")),
                        (tr(lang, "Curve: click / drag / drag-out"), tr(lang, "Add / move / delete point")),
                        (tr(lang, "Drag a mask handle"), tr(lang, "Reshape / move the selected mask")),
                    ];
                    egui::Grid::new("shortcut_grid").num_columns(2).striped(true).show(
                        ui,
                        |ui| {
                            for (keys, what) in rows {
                                ui.label(egui::RichText::new(keys).monospace().color(PILL));
                                ui.label(what);
                                ui.end_row();
                            }
                        },
                    );
                });
            if !open {
                self.show_shortcuts = false;
            }
        }

        // Drag & drop affordance: show a full-window overlay while files hover.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
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
                    for t in &self.toasts {
                        let (bg, fg) = match t.kind {
                            ToastKind::Success => (
                                egui::Color32::from_rgb(22, 58, 34),
                                egui::Color32::from_rgb(150, 230, 170),
                            ),
                            ToastKind::Error => (
                                egui::Color32::from_rgb(70, 26, 26),
                                egui::Color32::from_rgb(255, 165, 165),
                            ),
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
            },
        );
    }
}

/// Register a system CJK font so Chinese / Japanese UI text renders — egui ships
/// no CJK glyphs, so without this every non-Latin character is a tofu box (□).
///
/// Reads the user's installed font at runtime (no ~16 MB binary bloat) and
/// pre-validates it with `ab_glyph` (egui's own backend) before handing it over —
/// egui PANICS on a font it can't parse, so a missing/odd font must be skipped,
/// not registered. Appended as a FALLBACK so Latin keeps egui's default look.
fn install_cjk_font(ctx: &egui::Context) {
    // Single-face TTFs first (always parse); TTC collections (face 0) last.
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\Deng.ttf",   // DengXian — clean modern UI face
        r"C:\Windows\Fonts\simhei.ttf", // SimHei
        r"C:\Windows\Fonts\msyh.ttc",   // Microsoft YaHei (collection, face 0)
        r"C:\Windows\Fonts\simsun.ttc", // SimSun (collection, face 0)
    ];
    let Some(bytes) = CANDIDATES.iter().find_map(|p| {
        let b = std::fs::read(p).ok()?;
        // Only accept it if egui's backend can actually parse face 0 (no panic).
        ab_glyph::FontVec::try_from_vec_and_index(b.clone(), 0).ok()?;
        Some(b)
    }) else {
        return; // no usable CJK font found — Latin text still renders fine
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".to_owned(), egui::FontData::from_owned(bytes));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// A unified visual theme: warm-gold accent (shared with the variant-strip
/// highlight), rounder widgets, a little more breathing room, and headings a
/// step down from egui's chunky default so section titles group instead of
/// shouting. One tasteful pass over egui's dark base — not a full reskin.
fn install_theme(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, Rounding, Stroke, TextStyle};
    let mut style = (*ctx.style()).clone();
    style.visuals.selection.bg_fill =
        egui::Color32::from_rgba_unmultiplied(0xc9, 0xa1, 0x4a, 90);
    style.visuals.selection.stroke = Stroke::new(1.0, PILL);
    style.visuals.hyperlink_color = PILL;
    let rounding = Rounding::same(5.0);
    for w in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.rounding = rounding;
    }
    style.visuals.window_rounding = Rounding::same(8.0);
    style.visuals.menu_rounding = Rounding::same(6.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 24.0;
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional));
    ctx.set_style(style);
}

/// Decode the embedded Autoshop icon for the window title bar / taskbar.
fn app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../../assets/icon_256.png"))
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
        ..Default::default()
    };
    eframe::run_native(
        "Autoshop",
        opts,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx); // CJK glyphs so Chinese labels aren't tofu
            install_theme(&cc.egui_ctx); // unified accent / spacing / rounding pass
            Ok(Box::new(AutoshopApp::new(cc))) // restores prefs + last library
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn curve_editor_edits_render_identically_to_the_engine() {
        // The editor mutates recipe.*_curve and previews via render::curve_lut —
        // the exact LUT the engine applies. Empty = identity; an anchored lift
        // keeps the ends and raises the anchored midpoint.
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

    /// A tiny synthetic base + a bitmap mask on disk, for the async-develop and
    /// overlay regression tests. Returns (app, mask_path) — caller cleans up.
    fn app_with_masked_photo(tag: &str) -> (AutoshopApp, std::path::PathBuf) {
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
        (app, mask_path)
    }

    #[test]
    fn async_develop_discards_stale_frames_latest_wins() {
        // The whole point of the async scheduler: a frame built for an OLD
        // recipe must be dropped when the live recipe has moved on, and an
        // in-flight guard must prevent a second dispatch. Drives the pure
        // pieces (build_preview + finish_redevelop) with a headless egui ctx —
        // never run_native.
        let (mut app, mask_path) = app_with_masked_photo("latest");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();

        // A matching frame is accepted and bumps the counter + sets the texture.
        let good = build_preview(base.clone(), app.recipe.clone(), false);
        app.finish_redevelop(&ctx, Ok(good));
        assert_eq!(app.develop_count, 1, "matching frame accepted");
        assert!(app.after_tex.is_some(), "after texture set");
        assert!(!app.develop_inflight, "inflight cleared on completion");

        // Build a frame for the CURRENT recipe, then move the recipe on before
        // it "arrives": the stale frame must be discarded (counter unchanged).
        let stale = build_preview(base, app.recipe.clone(), false);
        app.recipe.masks[0].exposure_ev = 1.9; // user kept dragging
        app.finish_redevelop(&ctx, Ok(stale));
        assert_eq!(app.develop_count, 1, "stale frame (recipe moved) discarded");

        std::fs::remove_file(&mask_path).ok();
    }

    #[test]
    fn overlay_skips_rebuild_for_local_effect_sliders() {
        // The coverage-aware key: dragging a mask's Exposure/Temp/color_gains
        // changes WHAT it does, not WHERE — so the full-frame coverage raster
        // must NOT rebuild. Geometry / amount / inversion MUST rebuild.
        let (mut app, mask_path) = app_with_masked_photo("overlay");
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

        std::fs::remove_file(&mask_path).ok();
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

        assert!(
            matches!(read_saved_develop(src), SavedDevelop::Nothing),
            "no sidecar → Nothing"
        );

        // A NEUTRAL XMP (foreign file, or ours with nothing set) restores nothing.
        std::fs::write(&xp, autoshop::xmp::recipe_to_xmp(&EditRecipe::default())).unwrap();
        assert!(
            matches!(read_saved_develop(src), SavedDevelop::NoopOnly),
            "a no-op XMP must not claim a restore"
        );

        // XMP with real edits → imported through the reverse crs mapping.
        let edited = EditRecipe { contrast: 22.0, ..Default::default() };
        std::fs::write(&xp, autoshop::xmp::recipe_to_xmp(&edited)).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src) else {
            panic!("an edited XMP restores");
        };
        assert_eq!((r.contrast, kind), (22.0, "XMP"));

        // recipe.json appears → preferred over the XMP.
        let full = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(&rj, serde_json::to_string(&full).unwrap()).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src) else {
            panic!("recipe.json restores");
        };
        assert_eq!((r.exposure_ev, kind), (0.5, "recipe.json"));

        // A damaged recipe.json degrades LOUDLY, XMP fallback attached.
        std::fs::write(&rj, "{ not json").unwrap();
        let SavedDevelop::Unreadable { fallback, .. } = read_saved_develop(src) else {
            panic!("a damaged recipe.json must be reported, not skipped");
        };
        assert_eq!(fallback.expect("XMP fallback rides along").1, "XMP");

        // A pre-store legacy ./out sidecar is migrated in on FIRST read and
        // then restores from the CENTRAL copy (kind says recipe.json, not the
        // legacy fallback — the file physically moved). A SEPARATE photo path:
        // migrate_legacy memoizes per photo per process, so `src` (already
        // touched above with nothing legacy) would correctly skip the scan —
        // which is the production contract, not a test bug.
        let src2 = std::path::Path::new("D:/library/_sidecar_prio_test_legacy.ARW");
        let dev2 = autoshop::store::develop_dir(src2);
        let _ = std::fs::remove_dir_all(&dev2);
        let legacy_rj2 =
            PathBuf::from("out").join("_sidecar_prio_test_legacy.recipe.json");
        let legacy = EditRecipe { contrast: -11.0, ..Default::default() };
        std::fs::write(&legacy_rj2, serde_json::to_string(&legacy).unwrap()).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src2) else {
            panic!("a legacy ./out recipe restores");
        };
        assert_eq!((r.contrast, kind), (-11.0, "recipe.json"));
        assert!(!legacy_rj2.exists(), "legacy file consumed by the migration");
        assert!(
            autoshop::store::recipe_target(src2).exists(),
            "…and now lives in the central store"
        );

        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&dev2);
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
        app.remember_base(p, 1280, &base, &knots, &lens);
        let hit = app.cached_base(p, 1280);
        assert!(hit.is_some(), "same path+edge hits");
        let hit = hit.unwrap();
        assert_eq!(hit.1, knots, "base-look knots ride the cache entry");
        assert_eq!(hit.2, lens, "the lens profile rides the cache entry too");
        assert!(app.cached_base(p, 2560).is_none(), "edge is part of the key");
        // Filling past the cap evicts the least-recently-used entry.
        let others: Vec<std::path::PathBuf> = (0..BASE_CACHE_CAP)
            .map(|i| std::path::PathBuf::from(format!("D:/__autoshop_nonexistent__/{i}.ARW")))
            .collect();
        for o in &others {
            app.remember_base(o, 1280, &base, &[], &Default::default());
        }
        assert!(app.cached_base(p, 1280).is_none(), "least-recent evicted at cap");
        assert!(app.cached_base(&others[1], 1280).is_some(), "newer entries survive");
        assert!(app.base_cache.len() <= BASE_CACHE_CAP, "cap holds");
    }
}
