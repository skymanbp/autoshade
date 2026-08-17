//! GUI data model: view-state types, messages, prefs, constants.

use super::*;

/// How the preview area is laid out. `AfterOnly` gives the edit the whole
/// canvas (hold **B** to flash the source in place — the Lightroom gesture);
/// `SideBySide` keeps the permanent comparison.
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ViewMode {
    SideBySide,
    AfterOnly,
}

/// A transient corner notification. Errors linger twice as long as successes —
/// a one-line status bar alone is too easy to miss for a failed export.
pub(crate) struct Toast {
    pub(crate) text: String,
    pub(crate) kind: ToastKind,
    pub(crate) born: Instant,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ToastKind {
    Success,
    Error,
}

impl Toast {
    /// Display time scales with READING time (阶段5 手感): a per-kind base
    /// plus 35 ms per char beyond the first 40, capped — a two-line path
    /// list used to vanish before anyone finished it, while "Saved" never
    /// needed its full window. Chars, not bytes: a hanzi is one read unit.
    pub(crate) fn ttl(&self) -> Duration {
        let extra = self.text.chars().count().saturating_sub(40) as u64 * 35;
        let (base, cap) = match self.kind {
            ToastKind::Success => (4_000, 10_000),
            ToastKind::Error => (8_000, 14_000),
        };
        Duration::from_millis((base + extra).min(cap))
    }
}

/// The prefs worth remembering across launches (stored via eframe persistence
/// next to the window geometry). Everything here must stay cheap to re-apply.
/// `serde(default)` so prefs saved by an older build (missing newer keys) still
/// load instead of silently resetting everything.
/// The delivery format+depth the Export panel dials in (round-12 阶段4:
/// the old `save_jpeg` bool could not say PNG, nor 8-bit TIFF). Stored in
/// Prefs as a small integer (`pref_code`); `save_jpeg` is still written
/// alongside for downgrade compatibility and consulted on load only when
/// the new code is absent (a pre-阶段4 prefs file).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ExportFormat {
    Tiff16,
    Tiff8,
    Png16,
    Png8,
    Jpeg,
}

impl ExportFormat {
    pub(crate) const ALL: [ExportFormat; 5] = [
        ExportFormat::Tiff16,
        ExportFormat::Tiff8,
        ExportFormat::Png16,
        ExportFormat::Png8,
        ExportFormat::Jpeg,
    ];

    /// English label — the i18n skeleton key (zh via `tr`; the audit
    /// extracts this fn's literals).
    pub(crate) fn label(self) -> &'static str {
        match self {
            ExportFormat::Tiff16 => "16-bit TIFF",
            ExportFormat::Tiff8 => "8-bit TIFF",
            ExportFormat::Png16 => "16-bit PNG",
            ExportFormat::Png8 => "8-bit PNG",
            ExportFormat::Jpeg => "JPEG",
        }
    }

    pub(crate) fn ext(self) -> &'static str {
        match self {
            ExportFormat::Tiff16 | ExportFormat::Tiff8 => "tif",
            ExportFormat::Png16 | ExportFormat::Png8 => "png",
            ExportFormat::Jpeg => "jpg",
        }
    }

    pub(crate) fn eight_bit(self) -> bool {
        matches!(self, ExportFormat::Tiff8 | ExportFormat::Png8 | ExportFormat::Jpeg)
    }

    pub(crate) fn pref_code(self) -> u8 {
        match self {
            ExportFormat::Tiff16 => 0,
            ExportFormat::Tiff8 => 1,
            ExportFormat::Png16 => 2,
            ExportFormat::Png8 => 3,
            ExportFormat::Jpeg => 4,
        }
    }

    /// Prefs migration: an unknown/absent code (0 is also serde's default)
    /// defers to the legacy `save_jpeg` bool, so a pre-阶段4 prefs file
    /// keeps the format the user had chosen.
    pub(crate) fn from_pref(code: u8, legacy_jpeg: bool) -> Self {
        match code {
            1 => ExportFormat::Tiff8,
            2 => ExportFormat::Png16,
            3 => ExportFormat::Png8,
            4 => ExportFormat::Jpeg,
            _ if legacy_jpeg => ExportFormat::Jpeg,
            _ => ExportFormat::Tiff16,
        }
    }
}

/// WHERE an export lands — the delivery target, promoted to a first-class
/// setting (R22-7). Before this it was not a setting at all: "Export" hardcoded
/// a cwd-relative `./out` and "Download…" always opened a save dialog, and the
/// TARGET was the only difference between those two toolbar buttons. One
/// setting + one split button replaces the pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ExportDest {
    /// `./out` beside the working directory — what the CLI, the web surface
    /// and the batch renderer have always used, and still the default.
    #[default]
    OutFolder,
    /// Wherever the last export landed (`last_export_dir`). With nothing
    /// remembered yet this ASKS once and the answer seeds the memory.
    LastUsed,
    /// Ask every time (the old Download… behaviour, now a setting).
    Ask,
}

impl ExportDest {
    pub(crate) const ALL: [ExportDest; 3] =
        [ExportDest::OutFolder, ExportDest::LastUsed, ExportDest::Ask];

    /// English label — the i18n skeleton key (zh via `tr`; the audit extracts
    /// this fn's literals).
    pub(crate) fn label(self) -> &'static str {
        match self {
            ExportDest::OutFolder => "./out folder",
            ExportDest::LastUsed => "Last used folder",
            ExportDest::Ask => "Ask every time",
        }
    }

    pub(crate) fn pref_code(self) -> u8 {
        match self {
            ExportDest::OutFolder => 0,
            ExportDest::LastUsed => 1,
            ExportDest::Ask => 2,
        }
    }

    /// Prefs migration: 0 is also serde's default, so a prefs file written
    /// before R22-7 restores the historical `./out` behaviour, and an unknown
    /// code from a NEWER build degrades to it too rather than to "ask".
    pub(crate) fn from_pref(code: u8) -> Self {
        match code {
            1 => ExportDest::LastUsed,
            2 => ExportDest::Ask,
            _ => ExportDest::OutFolder,
        }
    }
}

/// What the Export button does THIS click, decided before anything is written
/// (R22-7). A separate type rather than an `Option<PathBuf>` so the asking
/// arm is a state a headless test can assert on — the dialog itself cannot be
/// opened from a test, but the decision that leads to it can.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum ExportRoute {
    /// The destination setting resolved to this concrete file.
    Render(PathBuf),
    /// The setting has to ask (Ask, or LastUsed with nothing remembered).
    Ask,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Prefs {
    pub(crate) gallery_dir: Option<PathBuf>,
    pub(crate) style_strength: f32,
    pub(crate) save_jpeg: bool,
    /// [`ExportFormat::pref_code`]; 0 defers to `save_jpeg` (migration).
    pub(crate) exp_format: u8,
    /// [`ExportDest::pref_code`]; 0 (serde's default too) = `./out`.
    pub(crate) exp_dest: u8,
    /// The folder the last export landed in — the target of
    /// [`ExportDest::LastUsed`], and where the ask-dialog reopens.
    pub(crate) last_export_dir: Option<PathBuf>,
    pub(crate) save_denoise: bool,
    pub(crate) zoned_fit: bool,
    pub(crate) fit_ai_judge: bool,
    pub(crate) view_mode: ViewMode,
    pub(crate) exp_long_edge: u32,
    pub(crate) exp_sharpen: f32,
    pub(crate) exp_quality: f32,
    pub(crate) exp_space: u8,
    pub(crate) preview_edge: u32,
    pub(crate) show_clipping: bool,
    pub(crate) lang: Lang,
    pub(crate) theme: ThemePref,
}

impl Default for Prefs {
    fn default() -> Self {
        // Mirror AutoshopApp's own defaults (see its Default impl) so a pref
        // key missing from an older save degrades to exactly the app default.
        Self {
            gallery_dir: None,
            style_strength: STYLE_STRENGTH_DEFAULT,
            save_jpeg: false,
            exp_format: 0,
            exp_dest: 0, // ./out — the CLI/batch shape, unchanged for old prefs
            last_export_dir: None,
            save_denoise: false,
            // Zoned sky reverse-fit ON by default: it degrades gracefully to
            // the plain global fit when segmentation is unavailable.
            zoned_fit: true,
            // AI review of the fit OFF by default: it is a PAID vision call
            // per fit — spending is the user's opt-in, never a default.
            fit_ai_judge: false,
            view_mode: ViewMode::SideBySide,
            exp_long_edge: 0,
            exp_sharpen: 0.0,
            exp_quality: 95.0,
            exp_space: 0,
            preview_edge: PREVIEW_EDGE,
            show_clipping: false,
            lang: Lang::En, // English is the default / skeleton language
            theme: ThemePref::Dark, // the historical look
        }
    }
}

/// Where the personal-style strength starts, what its slider's double-click /
/// right-click reset lands on, and the baseline the AI section's ● compares
/// against ([`AutoshopApp::ai_section_active`]). ONE definition on purpose
/// (R22 #16): the number was a bare `0.30` in BOTH `Default` impls, so
/// "the user moved it" was a statement no predicate could make without adding a
/// third copy to drift from the other two.
pub(crate) const STYLE_STRENGTH_DEFAULT: f32 = 0.30;

pub(crate) const PREVIEW_EDGE: u32 = 1280; // working preview size for fast live develop

pub(crate) const THUMB_EDGE: u32 = 160; // decoded gallery-thumbnail long edge

// The GALLERY's thumbnail geometry: decode budget (THUMB_EDGE) → drawn slot
// (THUMB_W × THUMB_H) → row height, one pipeline, which is why these four sit
// together rather than in theme.rs beside the cross-surface spacing/width
// SCALES. The variant strip's own thumb height is a different surface with its
// own arithmetic and lives at its one consumer (`STRIP_THUMB_H` in
// panels/develop.rs::variant_strip, which names this constant back).
pub(crate) const THUMB_W: f32 = 56.0; // displayed thumbnail size in the gallery

pub(crate) const THUMB_H: f32 = 40.0;

pub(crate) const GALLERY_ROW_H: f32 = 50.0; // fixed row height for ScrollArea::show_rows

pub(crate) const MAX_THUMB_INFLIGHT: usize = 6; // cap concurrent thumbnail decodes

// ONE band table (L16-7): the GUI indexes the recipe's HSL arrays with
// this order, so it must BE the recipe's order — a literal copy could be
// reordered on either side and silently mislabel every band.
pub(crate) use autoshop::recipe::HSL_BANDS;

// Representative swatch per HSL band (row markers in the mixer — approximate
// band-centre hues, display-only; the engine's band maths lives in render.rs).
pub(crate) const HSL_SWATCH: [egui::Color32; 8] = [
    egui::Color32::from_rgb(224, 72, 60),  // Red
    egui::Color32::from_rgb(224, 141, 60), // Orange
    egui::Color32::from_rgb(224, 211, 60), // Yellow
    egui::Color32::from_rgb(76, 191, 90),  // Green
    egui::Color32::from_rgb(60, 200, 200), // Aqua
    egui::Color32::from_rgb(76, 120, 224), // Blue
    egui::Color32::from_rgb(142, 90, 217), // Purple
    egui::Color32::from_rgb(208, 82, 184), // Magenta
];

pub(crate) const GRADE_REGIONS: [&str; 4] = ["Shadows", "Midtones", "Highlights", "Global"];

/// How a finished retouch enters the variant strip.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RetouchKind {
    /// A whole-frame REIMAGINE rendition → a NEW「AI 生成」variant (its look
    /// lives in the pixels).
    NewGenerated,
    /// A fill/heal/clone touch-up of the CURRENT rendition → bake into the
    /// active variant's base AND repoint its `origin` at the saved artifact, so
    /// export / reverse-fit / a further retouch all follow the retouched pixels
    /// (WYSIWYG) instead of the pre-retouch source.
    InPlace,
}

/// What one finished retouch DID (L12#4): facts, not prose — the landing
/// renders them in the landing-time language (`render_retouch_note`).
pub(crate) enum RetouchNote {
    /// Generative fill landed at this ./out artifact.
    Filled(PathBuf),
    /// Heal: spot count + artifact + the heal report's rationale split per
    /// the L12#2B suffix contract (AI prose prefix + typed notes).
    Healed {
        n: usize,
        out: PathBuf,
        ai_prose: String,
        notes: Vec<autoshop::rationale::Note>,
    },
    /// AI denoise baked into the active variant.
    Denoised(PathBuf),
    /// Clone stamp: spot count + artifact.
    Cloned { n: usize, out: PathBuf },
    /// Whole-frame reimagine → a new Generated variant.
    Reimagined(PathBuf),
}

/// A finished retouch from any of the five pixel paths (fill/heal/denoise/
/// clone/reimagine): `(preview of the ./out result, typed note, the saved
/// full-resolution ./out artifact, kind)`. The saved path becomes the affected
/// variant's `origin` — its export / reverse-fit / next-retouch source.
pub(crate) type RetouchDone =
    anyhow::Result<(image::DynamicImage, RetouchNote, PathBuf, RetouchKind)>;

/// Style-prompt extraction facts (L12#4): whether the ./out copy landed.
pub(crate) enum StyleNote {
    SavedCopy,
    SaveFailed(String),
    NotSaved,
}

/// Export result facts (L12#4) — one message enum for both export shapes.
pub(crate) enum ExportOutcome {
    /// A single render: the deliverable + whether the pre-era base look was
    /// re-estimated on the way.
    Single { out: PathBuf, relooked: bool },
    /// A batch: counts + per-photo errors (library English, shown verbatim
    /// as today) + the same-stem rename disclosures + relook count + the
    /// per-photo develop warnings the OPEN path would have surfaced (L16-2),
    /// plus the delivery ROOT this run actually used.
    ///
    /// That root is a FACT the worker carries (L12#4), not something the
    /// landing re-derives: the Destination combo stays reachable for the
    /// minutes a batch runs, and a landing that read the CURRENT setting
    /// would name a folder the files are not in.
    Batch {
        ok: usize,
        errs: Vec<String>,
        renamed: Vec<String>,
        relooked: usize,
        warns: Vec<String>,
        dest: PathBuf,
    },
}

/// Batch recipe-paste facts (L12#4). `errs` non-empty = partial failure —
/// the landing must keep routing that through the error channel.
pub(crate) struct PasteOutcome {
    pub(crate) ok: usize,
    pub(crate) xmp: usize,
    pub(crate) errs: Vec<String>,
    pub(crate) xmp_fails: Vec<String>,
    pub(crate) xmp_notes: Vec<String>,
}

/// CPU-built preview frame. Everything expensive is worker-side: engine
/// develop, geometry, the one RGB8 conversion, histogram, clipping pixels and
/// the 96px variant thumbnail. The UI thread only submits the prepared images
/// to egui textures. `base` + `recipe` are identity tags: if either differs
/// when the result arrives, the frame is stale and is discarded (latest wins).
pub(crate) struct PreviewDone {
    pub(crate) base: Arc<image::DynamicImage>,
    pub(crate) recipe: EditRecipe,
    /// Kept AFTER display too (as `last_rgb`) so toggling the clipping layer
    /// can rebuild its overlay without a full redevelop.
    pub(crate) rgb: image::RgbImage,
    /// The texture-ready conversion of `rgb`, built worker-side — the UI
    /// thread's only job is `tex.set` (the RGB→RGBA expansion of a 4-11 MP
    /// frame at 2560/4096 was a 5-15 ms stall per accepted frame).
    pub(crate) after: egui::ColorImage,
    pub(crate) histogram: Vec<[f32; 4]>,
    pub(crate) clipping: Option<egui::ColorImage>,
    pub(crate) thumb: egui::ColorImage,
}

/// Coverage-cache identity. Local effect sliders (Exposure/Temp/Saturation/
/// color_gains) are intentionally absent: they change pixels INSIDE a mask,
/// never its coverage. A Range Mask includes its PREFIX reference recipe
/// (earlier masks applied — the engine's stacking rule) because its weight
/// is judged on those developed pixels.
#[derive(Clone, PartialEq)]
pub(crate) struct OverlayKey {
    pub(crate) base: usize,
    pub(crate) target: usize,
    pub(crate) mask: MaskGeometry,
    // Coverage inputs the combined weight adds (v0.22): the component list
    // (each geometry + mode) and the eye toggle — without them an added /
    // re-moded / muted shape reused the stale wash.
    pub(crate) components: Vec<autoshop::recipe::MaskComponent>,
    pub(crate) enabled: bool,
    pub(crate) range: Option<RangeMask>,
    pub(crate) amount: f32,
    pub(crate) inverted: bool,
    pub(crate) reference_recipe: Option<EditRecipe>,
    pub(crate) straighten_deg: f32,
    pub(crate) lens_distortion: f32,
    // The coverage warp also depends on the PROFILE geometry toggles —
    // without them in the key, flipping profile Distortion/CA reused a wash
    // warped under the previous state (knot data only changes per photo,
    // which `base` already keys).
    pub(crate) profile_dist_on: bool,
    pub(crate) profile_ca_on: bool,
}

/// Messages from worker threads back to the UI. The large payloads are boxed so
/// the channel message stays small (clippy::large_enum_variant).
pub(crate) enum Msg {
    /// Decoded base + the photo's camera-matched base-look knots (empty for
    /// non-RAW sources, missing embedded previews, or a near-identity map —
    /// see `render::camera_base_knots`).
    Opened(Box<anyhow::Result<OpenedBase>>),
    // (MasterLoaded carries the SPAWN-time edge and file stamp: the landing
    // used to stat and read preferences at completion time, filing the
    // pixels under an identity they never had — L12-2/L12-6.)
    /// A synchronous GUI develop used to block egui's update loop. Preview work
    /// now returns here from a single latest-wins worker.
    Developed(Box<anyhow::Result<PreviewDone>>),
    /// Carries the gen_epoch it was started under: Analyze is cancellable
    /// (abandon-only — the advisor has no cancel checkpoints, so ✕ unblocks
    /// the UI now and the network call dies at its stall/progress deadline),
    /// and a cancelled call's late result must be discarded, not installed
    /// and PERSISTED over whatever the user has since done. The third tuple
    /// element is the deterministic-rationale notes (L12#2B) the landing
    /// installs for draw-time localization.
    Analyzed(
        u64,
        Box<
            anyhow::Result<(
                EditRecipe,
                autoshop::advisor::Verdict,
                Vec<autoshop::rationale::Note>,
            )>,
        >,
    ),
    Exported(anyhow::Result<ExportOutcome>),
    /// A folder scan finished: (folder, sorted source paths).
    Folder(Box<anyhow::Result<(PathBuf, Vec<PathBuf>, usize)>>),
    /// A gallery thumbnail decoded. `generation` tags the folder generation so a
    /// folder switch can't insert a stale thumbnail under a reused index.
    Thumb { generation: u64, idx: usize, img: Box<anyhow::Result<image::DynamicImage>> },
    /// A cold-restored variant's retouched MASTER decoded off-thread. The
    /// (photo, origin) pair is the install identity — index-free, so a card
    /// deleted or reordered while the decode ran discards the late pixels
    /// instead of installing them into whatever now sits at the old index.
    MasterLoaded {
        photo: PathBuf,
        origin: PathBuf,
        /// SPAWN-time identity (L12-2/L12-6): the landing files the pixels
        /// under the edge + stamp the decode actually ran with, never under
        /// completion-time preferences or a completion-time stat.
        edge: u32,
        stamp: FileStamp,
        img: Box<anyhow::Result<image::DynamicImage>>,
    },
    /// A generative-fill / heal / clone / reimagine result — see [`RetouchDone`].
    /// The `u64` is the cancel epoch the task was started under: Cancel bumps
    /// the app's epoch, so a cancelled worker's late result arrives with a
    /// stale epoch and is discarded instead of mutating the canvas.
    Retouched(u64, Box<RetouchDone>),
    /// A liveness line from a running generative worker (partial-image
    /// heartbeats, negotiation notes) — mirrored into the status bar. Carries
    /// the worker's cancel epoch: an abandoned task's heartbeat must not
    /// overwrite the status of the task the user started after cancelling.
    Progress(u64, String),
    /// AI segmentation finished: (mask display name, grayscale raster path)
    /// — attached to the recipe as a `MaskGeometry::Bitmap` local mask.
    Segmented(anyhow::Result<(String, PathBuf)>),
    /// Full-res mask refine finished — see [`MaskRefineOutcome`].
    MaskRefined(anyhow::Result<MaskRefineOutcome>),
    /// Batch render advanced: `done` of `total` photos finished (ok or err).
    BatchProgress { done: usize, total: usize },
    /// Reverse-fit finished — see [`FitOutcome`]: FACTS, not prose, so the
    /// landing renders the status in the language live at LANDING time
    /// (L12#4 — the old String was translated inside the spawn closure with
    /// the language captured at spawn, minutes stale by the time it landed).
    Fitted(Box<anyhow::Result<FitOutcome>>),
    /// Settings → "Import develops from an old ./out folder":
    /// (photo count, the folder) — rendered at landing (L12#4).
    LegacyImported(anyhow::Result<(usize, PathBuf)>),
    /// Style-prompt extraction finished: (the reusable prompt text, the
    /// typed fact of whether the .style.txt copy landed).
    Styled(Box<anyhow::Result<(String, StyleNote)>>),
    /// A `GET /models` fetch finished: the account's model ids (Settings
    /// pick-list). Tagged with the ROLE that asked, because the two roles can
    /// point at different endpoints with different catalogues — the analysis
    /// picker used to borrow the image endpoint's ids and offer nothing at all
    /// whenever the two base URLs differed. The `u64` is the catalogue
    /// generation captured at launch ([`ModelCatalogue::gen`]): a completion
    /// is installed only if nothing invalidated the catalogue while it flew.
    Models(ModelRole, u64, anyhow::Result<Vec<String>>),
    /// A batch recipe paste finished — counts and per-photo details,
    /// rendered at landing (L12#4).
    Pasted(anyhow::Result<PasteOutcome>),
}

/// What the full-resolution mask refine did — FACTS, not prose (L12#4, the
/// same rule [`FitOutcome`] follows): the worker runs for as long as a 61 MP
/// develop takes, so the sentence is composed at LANDING in the language live
/// then, never with the language captured at spawn.
pub(crate) enum MaskRefineOutcome {
    /// (the initiating mask index, the stored raster reference the job started
    /// from, the refined raster's path). Landing validates index AND reference
    /// together — the list may have changed mid-flight, and a bare path search
    /// could repoint the WRONG mask when two masks legitimately reference one
    /// raster (Codex R9-1).
    Refined(usize, String, PathBuf),
    /// The guide's own resolution would make a raster past the mask-raster
    /// budget (`render::mask_raster_fits_budget`) — one no later open or export
    /// could ever load. Refused with NOTHING written; the dimensions ride along
    /// because the refusal has to say which source it is talking about.
    OverBudget { w: u32, h: u32 },
}

/// One reverse-fit landing fact (L12#4): the worker records WHAT happened
/// on the persist path; `render_fit_note` (workers.rs) translates it when
/// the result lands. Owned args only — stringified errors, paths, counts.
pub(crate) enum FitNote {
    /// The fit attached at least one zone mask.
    IncludesSkyZone,
    /// `commit_develop` failed — the fit stays on the canvas unsaved.
    NotPersistedCommit(String),
    /// The Lightroom XMP sidecar landed at this path.
    XmpWritten(std::path::PathBuf),
    /// write_xmp's regenerated-not-merged disclosure (library English).
    XmpMergeNote(String),
    /// recipe.json persisted but the XMP write failed.
    XmpFailed(String),
    /// The previous explicit save was snapshotted as v{n} first.
    BackedUpAs(u32),
    /// The backup gate refused — nothing was written.
    NotPersistedBackup(String),
    /// The develop store lock could not be taken.
    NotPersistedLock(String),
    /// R20 opt-in AI review: the vision judge scored how faithfully the
    /// fitted render matches the target look. Critique is model English
    /// (the rationale contract — args ride verbatim, like error text).
    /// The judgement's accept/revise decision is deliberately NOT carried:
    /// the prompt pins its thresholds to the score (accept = 85+), so the
    /// score already says it — a second word would just need translating.
    AiReview { score: f32, critique: String },
    /// The AI review call failed — the fit itself already landed; this is
    /// the informational layer degrading, never the fit erroring.
    AiReviewFailed(String),
}

/// The reverse-fit result: recipe + errors + typed notes. `persisted` is
/// false when the backup gate refused the store write — the ● baseline
/// must NOT be advanced then. `rationale_notes` are the fit rationale's
/// typed copy (L12#2B), installed for draw-time localization.
pub(crate) struct FitOutcome {
    pub(crate) recipe: EditRecipe,
    pub(crate) err_before: f32,
    pub(crate) err_after: f32,
    pub(crate) rationale_notes: Vec<autoshop::rationale::Note>,
    pub(crate) status: Vec<FitNote>,
    pub(crate) persisted: bool,
}

/// Default endpoint for the image-role OAuth preset: a local Codex bridge
/// (e.g. CLIProxyAPI) that replays a ChatGPT-subscription OAuth token as an
/// OpenAI-compatible API on loopback. Only a default — the field stays editable
/// so a custom host/port works too.
pub(crate) const CODEX_BRIDGE_URL: &str = "http://127.0.0.1:8317/v1";

/// The stock OpenAI endpoint. Used to recognise a stale default when flipping the
/// image role into OAuth mode, so we can swap in the bridge URL automatically.
pub(crate) const OPENAI_DEFAULT_URL: &str = "https://api.openai.com/v1";

/// Which role a `GET /models` probe was run for. The two roles have their own
/// endpoint, their own key, and therefore their own catalogue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ModelRole {
    /// The vision proposer + generative edits (`image_base_url`).
    Image,
    /// The verifier, in `api` mode (`analysis_base_url`).
    Analysis,
}

/// One endpoint's live pick-lists, as last fetched.
#[derive(Default)]
pub(crate) struct ModelCatalogue {
    /// Text/vision chat ids — the proposer and the API verifier.
    pub(crate) chat: Vec<String>,
    /// Image-generation ids. Only the image role has a use for these; the
    /// analysis catalogue leaves it empty.
    pub(crate) image_gen: Vec<String>,
    /// A `GET /models` worker is in flight for this role.
    pub(crate) fetching: bool,
    /// The endpoint these ids came FROM. Editing the Base/Bridge URL (or the
    /// provider auto-swap rewriting it) makes them ids of a different server —
    /// the pickers self-invalidate instead of offering stale ids.
    pub(crate) from_base: String,
    /// Validity stamp for in-flight fetches: bumped by every [`clear`], and
    /// captured by `fetch_models` into [`Msg::Models`]. A completion whose
    /// stamp no longer matches was fetched for a world (key, endpoint) that a
    /// mid-flight edit invalidated — installing it would resurrect the OLD
    /// credential's ids under the new one's name. Same claim/installed shape
    /// as serve.rs's folder generations.
    ///
    /// [`clear`]: ModelCatalogue::clear
    pub(crate) generation: u64,
    /// Fingerprint of the credential the ids were fetched WITH (availability
    /// is credential-dependent, not just URL-dependent). Compared on a
    /// Settings reopen, where another surface (the web panel) may have
    /// replaced the saved key since this list was fetched. See
    /// `util::key_fingerprint` — an in-memory staleness stamp, never
    /// persisted, never displayed.
    pub(crate) from_key: u64,
    /// This role's once-per-session convenience probe has been DISPATCHED.
    /// Consumed at dispatch, not at Settings-open: a role that only becomes
    /// eligible later in the session (key added, provider flipped to `api`)
    /// still gets its one auto-fetch, while an already-probed role never
    /// re-probes a metered endpoint on every reopen.
    pub(crate) autofetched: bool,
}

impl ModelCatalogue {
    pub(crate) fn is_empty(&self) -> bool {
        self.chat.is_empty() && self.image_gen.is_empty()
    }

    /// Forget everything fetched. Used when the endpoint or the credential
    /// changes — both decide which ids exist. Bumps `gen`: whatever is still
    /// in flight was fetched for the world this clear just discarded.
    pub(crate) fn clear(&mut self) {
        self.chat.clear();
        self.image_gen.clear();
        self.generation += 1;
    }
}

/// Reasoning-effort tiers offered by the pickers. Suggestions, not a closed
/// set: both pickers sit beside a free-text field, `config::effort` only
/// BOUNDS the value, and an endpoint that rejects a tier is negotiated down
/// (`advisor::post_ai_json`, `claude::cli_rejected_effort`).
///
/// The Claude CLI's list is quoted from `claude --help` on this machine
/// (`--effort <level>  Effort level for the current session (low, medium,
/// high, xhigh, max)`, measured 2026-08-11). OpenAI-compatible endpoints
/// share the first three; anything else goes in the text field.
pub(crate) const EFFORT_TIERS_CLI: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub(crate) const EFFORT_TIERS_API: [&str; 3] = ["low", "medium", "high"];

/// The `claude` CLI's model ALIASES, from the same `--help` (`--model
/// <model>  … Provide an alias for the latest model (e.g. 'fable', 'opus', or
/// 'sonnet') …`). Used both as the OAuth picker's list and as the test for
/// "is this id a Claude alias" when the provider radio flips — a hardcoded
/// trio here used to rewrite a legitimate `fable` back to `opus`.
pub(crate) const CLAUDE_ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// Editable buffers for the in-app Settings window. Key fields stay blank on
/// load and only overwrite the stored key when non-empty (the form never shows
/// an existing secret) — mirroring the web `/api/settings` contract.
#[derive(Default)]
pub(crate) struct SettingsForm {
    pub(crate) analysis_provider_api: bool, // false = OAuth (claude CLI), true = OpenAI-compatible API
    pub(crate) image_provider_oauth: bool,  // true = Codex bridge (ChatGPT sub), false = OpenAI-compatible API
    pub(crate) analysis_model: String,
    pub(crate) analysis_base_url: String,
    pub(crate) analysis_api_key: String,
    pub(crate) analysis_key_present: bool,
    pub(crate) analysis_effort: String,
    pub(crate) image_model: String,
    pub(crate) image_base_url: String,
    pub(crate) image_gen_model: String,
    pub(crate) image_api_key: String,
    pub(crate) image_key_present: bool,
    pub(crate) image_effort: String,
    pub(crate) status: String,
    /// Live pick-lists, one per endpoint (see [`ModelCatalogue`]). Each
    /// carries its own once-per-session auto-fetch guard: one global boolean
    /// here was consumed by the first Settings open even when NO role was
    /// eligible yet, so a key saved later in the session never got its probe.
    pub(crate) image_models: ModelCatalogue,
    pub(crate) analysis_models: ModelCatalogue,
}

/// Pick radius (px) for on-image mask knobs — matches the crop handles' feel.
pub(crate) const HANDLE_HIT: f32 = 12.0;

/// What the next image drag defines (armed by the mask panel's buttons):
/// a brand-new mask, a redraw of mask `i`'s base area, or a new COMPONENT
/// appended to mask `i` with the given combine mode.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum PlaceTarget {
    NewMask,
    Redraw(usize),
    Component(usize, autoshop::recipe::MaskCombine),
}

/// Curve-editor channels: label + PLOT draw colour, indexed by `curve_channel`
/// (0 = master, then R/G/B — the recipe's tone/red/green/blue_curve fields).
/// The plot colours are theme-independent — the curve square stays dark on
/// both themes (see [`ThemeColors`]); the PICKER LABELS above the plot sit on
/// panel chrome and take their colours from `ThemeColors::curve_labels`.
// Tone-curve channel picker labels — skeleton keys, localized with `tr` at the
// render site (curve_editor). Colours are the on-curve accent, not localized.
pub(crate) const CURVE_CHANNELS: [(&str, egui::Color32); 4] = [
    ("Master", egui::Color32::from_gray(225)),
    ("Red", egui::Color32::from_rgb(235, 90, 90)),
    ("Green", egui::Color32::from_rgb(90, 205, 90)),
    ("Blue", egui::Color32::from_rgb(90, 130, 240)),
];

/// The lens-geometry half of the view mapping: the in-camera profile (the
/// composed map needs its distortion knots) plus the manual amount. Cloned
/// out of `geom_ctx` per gesture — a few 16-float Vecs, noise next to any
/// develop tick.
#[derive(Clone, Default)]
pub(crate) struct LensArg {
    pub(crate) profile: autoshop::recipe::LensProfile,
    pub(crate) amount: f32,
}

/// Slider feel classes — Lightroom's stepping grammar. `Int`: whole-number
/// domains (the ±100 family, 0..150, hue °); `Fine`: sub-unit precision
/// (Straighten °); `Frac`: small fractional domains (EV, 0..1 amounts);
/// `LogK`: the log-scaled Temp (K) track.
#[derive(Copy, Clone)]
pub(crate) enum SliderFeel {
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
pub(crate) struct UndoStep {
    pub(crate) recipe: EditRecipe,
    pub(crate) base: Option<Arc<image::DynamicImage>>,
    pub(crate) origin: Option<PathBuf>,
}

impl UndoStep {
    /// Same pixel identity? (Arc pointer + origin path — cheap, exact.)
    pub(crate) fn same_pixels(&self, other: &UndoStep) -> bool {
        let base_eq = match (&self.base, &other.base) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        // Master identity, not string identity: this was the one comparison
        // the batch-80 sweep left on `==`, which made its own "EVERY" false.
        // Benign today — a step's base and origin are always replaced
        // together, so no same-Arc / two-spellings pair can reach here — and
        // `same_master` tries plain equality first, so the common path is
        // unchanged.
        base_eq && same_master_opt(self.origin.as_deref(), other.origin.as_deref())
    }
}

/// One photo the quit dialog would save. Was a positional 4-tuple with a
/// nested pair — six values threaded by position (round-12 结构 cluster).
pub(crate) struct PendingSave {
    pub(crate) photo: PathBuf,
    /// The canvas recipe Save-all writes, exactly like Ctrl+S.
    pub(crate) recipe: EditRecipe,
    /// The canvas's baked pixel identity when one exists:
    /// (master origin, is_generated) — persists the master link.
    pub(crate) pixels: Option<(PathBuf, bool)>,
    /// The photo's strip record (background variants + active kind/position)
    /// when the strip is non-trivial — Save-all persists the WHOLE strip.
    pub(crate) strip: Option<autoshop::store::VariantsRecord>,
}

/// The canvas snapshot navigation stashes for a photo with unsaved work:
/// the recipe PLUS the active variant's pixel identity. A baked in-place
/// retouch is unsaved work too — restoring the recipe alone silently dropped
/// the healed pixels when you navigated away and back. `base` is the
/// preview-res Arc (refcount bump, no copy); `origin` the full-res master.
pub(crate) struct StashEntry {
    pub(crate) recipe: EditRecipe,
    pub(crate) base: Option<Arc<image::DynamicImage>>,
    pub(crate) origin: Option<PathBuf>,
    /// The ACTIVE variant's three-valued kind — a plain `generated: bool`
    /// here collapsed `Fitted` onto `Original`, so a 「◭ 反推」 card came back
    /// from navigation renamed 「▣ 原片」 (the strip's only rename bug that
    /// needed no disk at all).
    pub(crate) kind: VariantKind,
    /// The rest of the variant strip (everything but the active variant),
    /// so navigation restores the WHOLE session: a dirty BACKGROUND variant
    /// used to die silently on nav (H4) — the quit dialog discloses that
    /// loss honestly because quitting has nothing to restore into, but
    /// navigation does: this session's stash.
    pub(crate) others: Vec<StashedVariant>,
    /// Where the active variant sat in the strip (`others` = strip minus it).
    pub(crate) active_pos: usize,
}

/// One background variant in a [`StashEntry`] — thumb-less (textures do not
/// survive a photo switch; they rebuild on the first develop after restore).
pub(crate) struct StashedVariant {
    pub(crate) kind: VariantKind,
    pub(crate) recipe: EditRecipe,
    pub(crate) base: Option<Arc<image::DynamicImage>>,
    pub(crate) origin: Option<PathBuf>,
}

/// One rendition in the variant strip — a Lightroom-style virtual copy /
/// Capture One variant, NOT a compositing layer (variants never blend; you
/// switch between them losslessly). A variant is fully defined by its base
/// pixels + its develop recipe.
pub(crate) struct Variant {
    pub(crate) kind: VariantKind,
    /// This variant's develop. The ACTIVE variant's recipe is mirrored in
    /// `AutoshopApp::recipe` (the live working copy the sliders edit); it is
    /// saved back here when you switch away.
    pub(crate) recipe: EditRecipe,
    /// Base pixels this variant develops from. Arc makes variant switches and
    /// background preview dispatch O(1); pixels remain immutable.
    /// `None` ⇒ the shared source neutral (`AutoshopApp::source_preview`).
    pub(crate) base: Option<Arc<image::DynamicImage>>,
    /// The ./out artifact behind a raster variant (the reimagine PNG) — the
    /// reverse-fit target and the full-res export source. `None` for
    /// source-based variants.
    pub(crate) origin: Option<PathBuf>,
    /// Small developed thumbnail for the strip (rebuilt for the active variant
    /// on every develop; built once for the others when created / left).
    pub(crate) thumb: Option<egui::TextureHandle>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum VariantKind {
    Original,  // 原片 — the loaded RAW / image, your develop
    Generated, // AI 生成 — a whole-frame gpt-image restyle (look in the pixels)
    Fitted,    // 反推 — the generated look solved back into an editable recipe
}

impl VariantKind {
    /// Strip label (icon + English key; localised via `tr` at the render site).
    pub(crate) fn label(self) -> &'static str {
        match self {
            VariantKind::Original => "▣ Original",
            VariantKind::Generated => "✨ AI generated",
            VariantKind::Fitted => "◭ Reverse-fit",
        }
    }

    /// The `variants.json` spelling — persisted, so these strings are a
    /// FORMAT, not display text: never localise or rename them.
    pub(crate) fn store_str(self) -> &'static str {
        match self {
            VariantKind::Original => "original",
            VariantKind::Generated => "generated",
            VariantKind::Fitted => "fitted",
        }
    }

    pub(crate) fn from_store_str(s: &str) -> Option<Self> {
        match s {
            "original" => Some(VariantKind::Original),
            "generated" => Some(VariantKind::Generated),
            "fitted" => Some(VariantKind::Fitted),
            _ => None,
        }
    }
}

/// The two mask geometries a user can place by dragging.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum MaskKind {
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
pub(crate) const EXPORT_SPACES: [&str; 3] =
    ["sRGB (universal)", "Display P3 (wide-gamut screens)", "Adobe RGB (print)"];

pub(crate) const CROP_ASPECTS: [(&str, Option<f32>); 11] = [
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
pub(crate) struct ViewXform {
    pub(crate) rect: egui::Rect, // where the (visible part of the) image is drawn
    pub(crate) uv: egui::Rect,   // which full-frame region is visible (texture uv)
}

impl ViewXform {
    pub(crate) fn to_norm(self, p: egui::Pos2) -> (f32, f32) {
        // .max(1e-6), the same floor to_screen uses (L13-11): max(1.0)
        // REPLACED a sub-point rect dimension with a full point, compressing
        // the inverse map's whole axis — an extreme-panorama edge (>600:1)
        // put every crop/mask/brush gesture at the wrong coordinate.
        let fx = ((p.x - self.rect.min.x) / self.rect.width().max(1e-6)).clamp(0.0, 1.0);
        let fy = ((p.y - self.rect.min.y) / self.rect.height().max(1e-6)).clamp(0.0, 1.0);
        (self.uv.min.x + fx * self.uv.width(), self.uv.min.y + fy * self.uv.height())
    }

    pub(crate) fn to_screen(self, nx: f32, ny: f32) -> egui::Pos2 {
        egui::pos2(
            self.rect.min.x
                + (nx - self.uv.min.x) / self.uv.width().max(1e-6) * self.rect.width(),
            self.rect.min.y
                + (ny - self.uv.min.y) / self.uv.height().max(1e-6) * self.rect.height(),
        )
    }
}

/// A file's identity for cache purposes: mtime + size (size joins mtime like
/// the thumbnail key — a same-mtime replacement is otherwise indistinguishable
/// from the cached copy).
pub(crate) type FileStamp = Option<(std::time::SystemTime, u64)>;

/// The BAKED-PIXEL identity a cached base was decoded FOR: which master the
/// develop resolved to, whether it was AI-generated, and that master's own
/// stamp. `None` = a parametric-only develop (no pixels.json).
pub(crate) type PixelIdentity = Option<(PathBuf, bool, FileStamp)>;

/// Decoded-base LRU key: source path + requested preview edge + the source's
/// stamp + the baked-pixel identity.
///
/// The last component is not optional bookkeeping. A develop's pixels can
/// change while the SOURCE file never moves: reverse-fit clears pixels.json,
/// a retouch writes a new master, a save drops the link. `forget_open_base()`
/// exists to evict eagerly at those moments, but three separate review passes
/// found the same class of bug — one call site that forgot it (reverse-fit),
/// after which navigating away and back within the LRU resurrected the old
/// generated master and rendered the new recipe on pixels it was never
/// computed against. Putting the identity IN the key makes a stale hit
/// impossible instead of merely unlikely.
pub(crate) type BaseCacheKey = (PathBuf, u32, FileStamp, PixelIdentity);

/// A persisted baked master restored at open (store `pixels.json`): its
/// preview-res pixels, the full-res path, and whether it is an AI-generated
/// rendition (restores as a Generated variant — no parametric XMP).
pub(crate) type BakedBase = (Arc<image::DynamicImage>, PathBuf, bool);

/// A decoded preview base + the photo's camera-matched base-look knots
/// (`render::camera_base_knots`; empty = no base look) + its in-camera lens
/// profile (`pipeline::fresh_lens_profile`; default = none available) + its
/// as-shot WB anchor (`render::as_shot_wb`; None = unknown → 5500 K).
/// The sixth element is the decode's IDENTITY, captured in the worker BEFORE
/// any read (working edge + source stamp + baked-pixel identity):
/// `remember_base` files the entry under it, never under completion-time
/// state — the curve-memo rule.
pub(crate) type OpenedBase = (
    Arc<image::DynamicImage>,
    Vec<[f32; 2]>,
    autoshop::recipe::LensProfile,
    Option<(f32, f32)>,
    Option<BakedBase>,
    (u32, FileStamp, PixelIdentity),
    // A baked master that FAILED to decode (the canvas degraded to the
    // un-retouched source): the error detail, surfaced as a landing toast —
    // stderr is invisible in the windowed build (L11-3). Never cached
    // (remember_base stores None), so a revisit does not re-toast.
    Option<String>,
);

/// How many decoded preview bases to keep for instant photo revisits (~3.3 MB
/// each at the default 1280 edge; culling flips between neighbours constantly).
pub(crate) const BASE_CACHE_CAP: usize = 4;

/// How many decoded cold-variant masters to keep, so revisiting a strip card
/// is instant instead of a fresh multi-second decode. Entry-count, not bytes
/// (the same weakness `BASE_CACHE_CAP` carries): ~3.3 MB each at the default
/// 1280 edge but ~33 MB each at 4096, so the resident worst case tracks the
/// px preference.
pub(crate) const MASTER_CACHE_CAP: usize = 3;
