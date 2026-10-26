//! Normal mode's sidebar (design-normal-mode.md §3, approved 2026-10-23): four
//! groups in the order a person asks for them — make it look good (a one-click
//! look), a little brighter or warmer (fine-tune), less noise, save it — and
//! nothing else. Drawn in place of the shared panels when the mode's
//! `Shell::one_click_sidebar` is on (`app.rs` `controls_column`).
//!
//! Every verb here reuses a door the full workspace already has: a one-click
//! look is `produce_recipe` through `spawn_analyze` (a preset is a typed
//! direction, the reference folder is the library — Normal's own,
//! `library::NORMAL_NAME`), the four sliders write the recipe fields the
//! develop panel writes, the noise switch is the Retouch panel's AI denoise
//! (its ◈ card, shown on the canvas and saved as shown; off goes back to the
//! card it was made from), and the Save group is the single export and the
//! paste-then-render batch with a jpg's options, numbered rather than
//! replacing a jpg saved earlier. The words are plain (separate i18n keys,
//! design §6): none of the workspace's trade terms appear here.

use crate::*;
use autoshade::pipeline::{GradeRequest, Guide};

/// The adherence a preset's direction is followed with: above 70 %, a firm
/// brief (design-normal-oneclick.md §2b, 「adherence: 紧」).
pub(crate) const PRESET_ADHERENCE: f32 = 0.8;

/// The Style a reference-folder look is asked at: at or above
/// `library::STYLE_TARGET_MIN` the references are the look to reproduce, not
/// a ceiling (design-normal-oneclick.md §2a).
pub(crate) const REFERENCE_STYLE: f32 = 1.0;

/// What the first AI denoise of a rendered image downloads: the SCUNet weight
/// plus its network file (`python/denoise.py` `WEIGHT_BYTES["color_real_psnr"]` +
/// `NETWORK_BYTES`, 71 994 286 bytes), in MB — pinned by a test against
/// that file.
pub(crate) const DENOISE_FIRST_DOWNLOAD_MB: u32 = 72;

/// What a one-click asks for: one of the fixed looks, or the photos in the
/// chosen reference folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OneClick {
    Preset(&'static str),
    References,
}

/// The one line under the one-click group: what the last one-click did.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum NormalLine {
    #[default]
    Idle,
    Working,
    Done,
    /// No image key: the built-in look stood in (`advisor::HeuristicProposer`).
    BuiltIn,
    /// The look could not be made; the error's own text.
    Failed(String),
}

/// The JPEG quality steps of the Save group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SaveQuality {
    #[default]
    High,
    Medium,
    Small,
}

impl SaveQuality {
    pub(crate) const ALL: [SaveQuality; 3] = [SaveQuality::High, SaveQuality::Medium, SaveQuality::Small];

    pub(crate) fn label(self, lang: Lang) -> &'static str {
        match self {
            SaveQuality::High => tr(lang, "High"),
            SaveQuality::Medium => tr(lang, "Medium"),
            SaveQuality::Small => tr(lang, "Small file"),
        }
    }

    pub(crate) fn jpeg_quality(self) -> u8 {
        match self {
            SaveQuality::High => 95,
            SaveQuality::Medium => 85,
            SaveQuality::Small => 70,
        }
    }
}

/// What the status bar's marker says in Normal (`normal_look_mark`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LookMark {
    /// The look changed since the photo opened or was last saved as a jpg.
    Unsaved,
    /// The look on screen is the one just saved as a jpg.
    Saved,
}

impl LookMark {
    /// The marker's words and its hover, in plain words.
    pub(crate) fn words(self, lang: Lang) -> (&'static str, &'static str) {
        let label = match self {
            LookMark::Unsaved => tr(lang, "unsaved changes"),
            LookMark::Saved => tr(lang, "saved as jpg"),
        };
        (label, tr(lang, "Ctrl+S saves a jpg"))
    }
}

/// The noise switch's two cards for one photo: the card it was turned on
/// from, and the ◈ card the AI denoise made from it (`None` while it runs).
/// Ids, not indices: the strip can change under them in the full workspace.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DenoisePair {
    pub(crate) photo: PathBuf,
    pub(crate) plain: String,
    pub(crate) denoised: Option<String>,
}

/// The four fine-tune values as a one-click left them, so 「back to the
/// one-click result」 can put them back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FineTune {
    pub(crate) exposure_ev: f32,
    pub(crate) contrast: f32,
    pub(crate) temperature_k: Option<f32>,
    pub(crate) saturation: f32,
}

impl FineTune {
    pub(crate) fn of(r: &EditRecipe) -> Self {
        FineTune { exposure_ev: r.exposure_ev, contrast: r.contrast, temperature_k: r.temperature_k, saturation: r.saturation }
    }

    fn apply(self, r: &mut EditRecipe) {
        r.exposure_ev = self.exposure_ev;
        r.contrast = self.contrast;
        r.temperature_k = self.temperature_k;
        r.saturation = self.saturation;
    }
}

/// Normal's own state. Nothing here is persisted: the remembered preferences
/// (the look strength among them) are the workspace's own.
#[derive(Default)]
pub(crate) struct NormalState {
    pub(crate) line: NormalLine,
    /// The reference folder picked this session.
    pub(crate) ref_dir: Option<PathBuf>,
    /// The folder is being read into Normal's library.
    pub(crate) building: bool,
    pub(crate) build_progress: Option<(usize, usize)>,
    /// The analysis epoch a one-click started under.
    pub(crate) analyze_epoch: Option<u64>,
    /// The photo a one-click last landed on, and its four fine-tune values.
    pub(crate) result: Option<(PathBuf, FineTune)>,
    /// The noise switch's cards (`set_normal_denoise`); the switch itself
    /// reads the card on screen (`normal_denoise_on`).
    pub(crate) denoise_pair: Option<DenoisePair>,
    /// Whether the rendered-image denoiser's weight is on disk, read on the
    /// first frame the switch is drawn (`None` = not read yet).
    pub(crate) denoise_ready: Option<bool>,
    pub(crate) quality: SaveQuality,
    /// Is an image key set? Read on the first Normal frame and after a
    /// Settings save; `None` = not read yet.
    pub(crate) key_present: Option<bool>,
    /// 「Apply to every photo」 is between its paste and its render: the
    /// folder the jpgs go to, resolved before the paste.
    pub(crate) apply_all: Option<PathBuf>,
    /// A Save-group delivery is running (its landing is worded here).
    pub(crate) delivering: bool,
    /// The photo, card and look a running Save-group delivery is saving.
    pub(crate) saving: Option<(PathBuf, Option<PathBuf>, EditRecipe)>,
    /// The status-bar marker's anchor: the photo, the card and look it is
    /// measured from (as opened, or as last saved as a jpg — the card by the
    /// file it renders from, so the noise switch's ◈ card is another
    /// picture), and whether that was saved (`normal_look_mark`).
    pub(crate) look_mark: Option<(PathBuf, Option<PathBuf>, EditRecipe, bool)>,
    /// Test seam: what the last analysis asked for (request, direction,
    /// library name).
    #[cfg(test)]
    pub(crate) last_request: Option<(GradeRequest, Option<String>, &'static str)>,
    /// Test seam: stands in for the paid chain inside the analyze worker.
    #[cfg(test)]
    pub(crate) runner: Option<AnalyzeRunner>,
    /// Test seam: the batch-folder dialog's answer (`Some(None)` = cancelled).
    #[cfg(test)]
    pub(crate) folder_pick: Option<Option<PathBuf>>,
    /// Test seam: the save window's answer (`Some(None)` = cancelled).
    #[cfg(test)]
    pub(crate) file_pick: Option<Option<PathBuf>>,
}

/// A stand-in for `produce_recipe` (tests only).
#[cfg(test)]
pub(crate) type AnalyzeRunner = fn(
    &std::path::Path,
    &GradeRequest,
) -> anyhow::Result<(EditRecipe, autoshade::advisor::Verdict, Vec<autoshade::rationale::Note>)>;

/// A folder by its own name (its full path is one hover away).
fn folder_name(p: &std::path::Path) -> String {
    p.file_name().map_or_else(|| abs_display(p), |n| n.to_string_lossy().into_owned())
}

/// The warmth slider's reach: ±100 is a factor of 2^±1 on the colour
/// temperature, so the full travel spans one octave either side of the
/// camera's own white balance.
const WARMTH_OCTAVES: f32 = 1.0;

/// The Kelvin the white balance is measured against: the camera's as-shot
/// value when the photo carries one, the develop panel's 5500 K otherwise
/// (the same anchor its Temp slider resets to).
fn warmth_anchor(r: &EditRecipe) -> f32 {
    r.as_shot_k.unwrap_or(5500.0)
}

/// The warmth a recipe shows, −100 (cool) … +100 (warm): its colour
/// temperature as an offset from the anchor, in hundredths of an octave. No
/// custom white balance is 0.
pub(crate) fn warmth_of(r: &EditRecipe) -> f32 {
    match r.temperature_k {
        Some(k) if k > 0.0 => (100.0 / WARMTH_OCTAVES * (k / warmth_anchor(r)).log2()).clamp(-100.0, 100.0),
        _ => 0.0,
    }
}

/// The colour temperature for a warmth: the anchor times 2^(warmth/100),
/// inside the develop panel's 2000–40000 K track. A higher temperature
/// renders warmer, as Lightroom's Temp slider does.
pub(crate) fn kelvin_for_warmth(r: &EditRecipe, warmth: f32) -> f32 {
    (warmth_anchor(r) * (warmth.clamp(-100.0, 100.0) / 100.0 * WARMTH_OCTAVES).exp2()).clamp(2000.0, 40000.0)
}

impl AutoShadeApp {
    /// The request and direction a one-click sends — read on the UI thread,
    /// like `start_analyze`'s. Fixed in Normal (design §3): the style, the
    /// adherence, deep thinking and the guide are not drawn; only the look
    /// strength is the user's.
    pub(crate) fn one_click_request(&self, ask: OneClick) -> (GradeRequest, Option<String>) {
        let strength = self.analysis_strength();
        match ask {
            OneClick::Preset(id) => (
                GradeRequest {
                    guide: Guide::Direction,
                    style: 0.0,
                    send_reference_image: false,
                    strength,
                    think: false,
                    adherence: autoshade::recipe::DirectionAdherence::new(PRESET_ADHERENCE),
                    embed: autoshade::library::EmbeddingSwitch::OFF,
                },
                autoshade::advisor::presets::preset_guidance(id).map(str::to_owned),
            ),
            OneClick::References => (
                GradeRequest {
                    guide: Guide::Library,
                    style: REFERENCE_STYLE,
                    send_reference_image: true,
                    strength,
                    think: false,
                    adherence: autoshade::recipe::DirectionAdherence::default(),
                    // The folder was just read by the same model, so its
                    // download is done; the environment override still wins.
                    embed: autoshade::library::EmbeddingSwitch::resolve(None, true),
                },
                None,
            ),
        }
    }

    /// Run a one-click look on the open photo: the analysis worker, Normal's
    /// library, no base (a fresh look), landing in the sliders like any
    /// analysis (undoable).
    pub(crate) fn start_one_click(&mut self, ask: OneClick) {
        let Some(path) = self.src_path.clone() else { return };
        if self.busy || self.analyze_inflight {
            return;
        }
        let (req, guidance) = self.one_click_request(ask);
        self.busy = true;
        self.status = tr(self.lang, "Making the look…").into();
        self.normal.line = NormalLine::Working;
        let epoch = self.spawn_analyze(path, None, guidance, None, req, autoshade::library::NORMAL_NAME);
        self.normal.analyze_epoch = Some(epoch);
    }

    /// 「Match my reference photos…」 after the folder is picked: read it into
    /// Normal's own library (no look phrases — the photos themselves go to
    /// the model), then hand over to the analysis (`on_one_click_built`).
    pub(crate) fn start_one_click_folder(&mut self, dir: PathBuf) {
        if self.normal.building {
            return;
        }
        self.normal.building = true;
        self.normal.build_progress = None;
        self.normal.ref_dir = Some(dir.clone());
        self.normal.line = NormalLine::Idle;
        self.status = tr(self.lang, "Reading your reference photos…").into();
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        self.spawn_worker(
            move || {
                let progress = |p: autoshade::library::BuildProgress| {
                    let _ = tx.send(Msg::OneClickBuildProgress { done: p.done, total: p.total });
                    ctx.request_repaint();
                };
                let sidecars = autoshade::library::Sidecars::from_config(&autoshade::config::Config::load());
                let built = autoshade::library::rebuild_named(
                    autoshade::library::NORMAL_NAME,
                    &dir,
                    &sidecars,
                    autoshade::library::DescribeSwitch::OFF,
                    None,
                    &progress,
                );
                Msg::OneClickBuilt(built.map(|lib| lib.entries.len()).map_err(|e| format!("{e:#}")))
            },
            |e| Msg::OneClickBuilt(Err(e.to_string())),
        );
    }

    /// `Msg::OneClickBuilt` landing: the folder chain's hand-off — a built
    /// library goes straight into the reference-folder analysis.
    pub(crate) fn on_one_click_built(&mut self, lang: Lang, res: Result<usize, String>) {
        self.normal.building = false;
        self.normal.build_progress = None;
        match res {
            Ok(_) => self.start_one_click(OneClick::References),
            Err(err) => {
                let t = trf(lang, "Could not read your reference photos: {err}", &[("err", &err)]);
                self.normal.line = NormalLine::Failed(t.clone());
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Before `on_analyzed` consumes a result: is it a one-click's (`Some`),
    /// and did it land — `Ok(true)` when the built-in look stood in for the
    /// AI.
    pub(crate) fn one_click_landing(
        &self,
        epoch: u64,
        res: &anyhow::Result<(EditRecipe, autoshade::advisor::Verdict, Vec<autoshade::rationale::Note>)>,
    ) -> Option<Result<bool, ()>> {
        (self.normal.analyze_epoch == Some(epoch) && epoch == self.gen_epoch).then(|| {
            res.as_ref().map(|(_, _, notes)| {
                notes.iter().any(|n| {
                    matches!(
                        n.key,
                        autoshade::rationale::keys::HEURISTIC_NO_KEY | autoshade::rationale::keys::HEURISTIC_UNAVAILABLE
                    )
                })
            })
            .map_err(|_| ())
        })
    }

    /// After a one-click landed through `on_analyzed`: the plain line, and
    /// the four fine-tune values remembered for 「back to the one-click
    /// result」.
    pub(crate) fn after_one_click(&mut self, lang: Lang, landed: Result<bool, ()>) {
        self.normal.analyze_epoch = None;
        match landed {
            Ok(built_in) => {
                if let Some(p) = self.src_path.clone() {
                    self.normal.result = Some((p, FineTune::of(&self.recipe)));
                }
                self.normal.line = if built_in { NormalLine::BuiltIn } else { NormalLine::Done };
                if built_in {
                    self.normal.key_present = Some(false);
                }
                let plain = self.normal_line_text(lang).to_string();
                self.reword_landing(plain);
            }
            Err(()) => self.normal.line = NormalLine::Failed(self.status.clone()),
        }
    }

    /// Put a plain sentence in place of the status a shared landing just
    /// wrote — and in its toast, when it raised one.
    pub(crate) fn reword_landing(&mut self, plain: String) {
        let old = std::mem::replace(&mut self.status, plain.clone());
        if let Some(t) = self.toasts.iter_mut().rev().find(|t| t.text == old) {
            t.text = plain;
        }
    }

    /// A mode without canvas tools (`Shell::canvas_tool_marks` off: Normal)
    /// holds none armed. `choose_mode` puts them down on the way in; this is
    /// the frame's own check, so no other route (a mode set without the page,
    /// a later arming path) can leave a brush ring on the canvas or its hint
    /// in the status bar. Run before the status bar and the canvas draw.
    pub(crate) fn put_down_hidden_tools(&mut self) {
        if self.mode.shell().canvas_tool_marks || !(self.tool_armed() || self.lens_focus_picking) {
            return;
        }
        self.disarm_tools();
        self.status = self.normal_line_text(self.lang);
    }

    /// The jpg the Save group delivers.
    fn normal_export_opts(&self) -> autoshade::render::ExportOpts {
        autoshade::render::ExportOpts { jpeg_quality: self.normal.quality.jpeg_quality(), ..Default::default() }
    }

    /// 「Save as jpg」: the single export of the card on screen (the ◈ card
    /// when the noise switch is on), as a jpg at the quality step, to the
    /// export destination (asked when the destination is "ask"). A name the
    /// app made is numbered (`-2`, `-3`, …) when a jpg of that name is
    /// already there; a file picked in the save window is replaced, as that
    /// window confirmed.
    pub(crate) fn start_normal_save(&mut self) {
        if self.active_source_path().is_none() || self.busy {
            return;
        }
        let name = self.export_file_name_as("jpg");
        let (out, how) = match self.export_dest_dir() {
            Some(dir) => (dir.join(name), OutName::Numbered),
            None => {
                let Some(p) = self.pick_save_file(name) else { return };
                (normalize_export_target(p, "jpg"), OutName::Chosen)
            }
        };
        let export = self.normal_export_opts();
        if self.start_render_with(out, export, None, false, how) {
            self.normal.delivering = true;
            self.normal.saving = self.saving_now();
        }
    }

    /// The save window's answer (`None` = cancelled).
    fn pick_save_file(&mut self, name: String) -> Option<PathBuf> {
        #[cfg(test)]
        if let Some(pick) = self.normal.file_pick.take() {
            return pick;
        }
        let mut dlg = rfd::FileDialog::new().add_filter("jpg", &["jpg"]).set_file_name(name);
        if let Some(d) = self.last_export_dir.clone().filter(|d| d.is_dir()) {
            dlg = dlg.set_directory(d);
        }
        dlg.save_file()
    }

    /// What a Save-group delivery starting now saves: the photo, the card on
    /// screen (the file it renders from) and its look.
    fn saving_now(&self) -> Option<(PathBuf, Option<PathBuf>, EditRecipe)> {
        self.src_path.clone().map(|p| (p, self.active_source_path(), self.recipe.clone()))
    }

    /// 「Apply this look to every photo in the folder」: the open photo's look
    /// pasted onto every photo of the library folder (crop left out), then —
    /// once the paste has landed whole (`after_apply_all_paste`) — every one
    /// rendered as a jpg. The library panel's own two verbs, as one. The
    /// jpgs' folder is settled FIRST: a folder dialog cancelled after the
    /// paste would have left every photo's look changed and nothing saved.
    pub(crate) fn start_apply_to_folder(&mut self) {
        if self.busy || self.gallery.is_empty() || self.src_path.is_none() {
            return;
        }
        let Some(dest) = self.apply_all_dest() else {
            // Cancelled: nothing was changed, and the line says nothing.
            self.status.clear();
            return;
        };
        self.commit_pending_names();
        let targets = self.gallery.clone();
        self.normal.apply_all = Some(dest);
        self.start_paste_to(self.recipe.clone(), self.src_path.clone(), targets, false);
        if !self.busy {
            self.normal.apply_all = None;
        }
    }

    /// The destination setting's folder, or the folder dialog's answer when
    /// it asks (`None` = cancelled).
    fn apply_all_dest(&mut self) -> Option<PathBuf> {
        if let Some(d) = self.export_dest_dir() {
            return Some(d);
        }
        #[cfg(test)]
        if let Some(pick) = self.normal.folder_pick.take() {
            return pick;
        }
        self.pick_batch_folder()
    }

    /// The paste half of 「apply to every photo」 landed: render them all into
    /// `dest`, or leave the paste's own error standing.
    pub(crate) fn after_apply_all_paste(&mut self, lang: Lang, dest: PathBuf, pasted: bool) {
        if !pasted {
            return;
        }
        // The paste's own sentence is the workspace's; the render's status
        // replaces it below.
        let old = self.status.clone();
        self.toasts.retain(|t| t.text != old);
        let targets = self.gallery.clone();
        let export = self.normal_export_opts();
        self.start_batch_render_for(targets, "jpg".into(), export, dest, true);
        if self.busy {
            self.normal.delivering = true;
            // The open photo is one of them: its look counts as saved too.
            self.normal.saving = self.saving_now();
            self.status = tr(lang, "Saving every photo in the folder…").into();
        }
    }

    /// The Save group's landing in plain words — `None` when no Save-group
    /// delivery is running, or a batch had failures (their error stands).
    pub(crate) fn normal_delivery_words(&mut self, lang: Lang, outcome: &ExportOutcome) -> Option<String> {
        if !std::mem::take(&mut self.normal.delivering) {
            return None;
        }
        let saving = self.normal.saving.take();
        let (words, whole) = match outcome {
            // The file actually written: a numbered save names its number.
            ExportOutcome::Single { out, .. } => {
                let dir = out.parent().map(folder_name).unwrap_or_default();
                let file = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                (Some(trf(lang, "Saved {file} to {folder}", &[("file", &file), ("folder", &dir)])), true)
            }
            ExportOutcome::Batch { ok, errs, dest, renamed, .. } if errs.is_empty() => {
                let saved = trf(lang, "{n} photos saved to {folder}", &[("n", &ok.to_string()), ("folder", &folder_name(dest))]);
                let words = if renamed.is_empty() {
                    saved
                } else {
                    let k = renamed.len().to_string();
                    let numbered = trf(lang, "{k} of them have a number added to the name, so no earlier photo was replaced.", &[("k", &k)]);
                    format!("{saved}. {numbered}")
                };
                (Some(words), true)
            }
            // Every failure was the refusal to save into the photos' own
            // folder (counted by type in the worker): the plain sentence.
            ExportOutcome::Batch { errs, refused, .. } if *refused == errs.len() => {
                (Some(tr(lang, "AutoShade never saves into the folder your photos are in. Pick another folder.").to_string()), false)
            }
            ExportOutcome::Batch { .. } => (None, false),
        };
        // Delivered whole: the look that went out is the photo's saved jpg
        // (a batch with failures names no photo, so it marks none).
        if whole && let Some((photo, card, look)) = saving {
            self.normal.look_mark = Some((photo, card, look, true));
        }
        words
    }

    /// Normal's words for `pipeline::SourceFolderRefused` — the refusal
    /// Normal's numbered saves can still meet — matched by type (anywhere in
    /// the chain), never by its English; `None` for any other error, or
    /// outside Normal, where the library's own line stands.
    pub(crate) fn plain_refusal(&self, lang: Lang, e: &anyhow::Error) -> Option<String> {
        (self.mode.shell().one_click_sidebar && e.chain().any(|c| c.is::<autoshade::pipeline::SourceFolderRefused>()))
            .then(|| tr(lang, "AutoShade never saves into the folder your photos are in. Pick another folder.").to_string())
    }

    /// Is the noise switch on: the card on screen is a ◈ card, or the
    /// switch's own AI denoise is running.
    pub(crate) fn normal_denoise_on(&self) -> bool {
        self.active_variant().is_some_and(|v| v.kind == VariantKind::Denoised)
            || (self.busy
                && self
                    .normal
                    .denoise_pair
                    .as_ref()
                    .is_some_and(|d| d.denoised.is_none() && self.src_path.as_ref() == Some(&d.photo)))
    }

    /// The strip index of the card with `id` (an empty id matches nothing).
    fn card_index(&self, id: &str) -> Option<usize> {
        (!id.is_empty()).then(|| self.variants.iter().position(|v| v.id == id)).flatten()
    }

    /// Put card `idx` under the canvas with the look on screen: the switch
    /// changes the grain, never the look the sliders made.
    fn show_card_keeping_look(&mut self, idx: usize) {
        let look = self.recipe.clone();
        let ctx = self.egui_ctx.clone();
        self.switch_variant(idx, &ctx);
        if self.active == idx {
            self.recipe = look;
            self.dirty = true;
        }
    }

    /// The noise switch. On: the Retouch panel's own AI denoise
    /// (`start_ai_denoise` — the same worker, model and strength; its ◈ card
    /// lands on the canvas), or — when this card was denoised already — that
    /// ◈ card again, with no second run. Off: back to the card it was made
    /// from (the first card that is not a ◈ card when the switch did not
    /// make it).
    pub(crate) fn set_normal_denoise(&mut self, on: bool) {
        let Some(photo) = self.src_path.clone() else { return };
        if self.busy || on == self.normal_denoise_on() {
            return;
        }
        let Some(active) = self.active_variant().map(|v| v.id.clone()) else { return };
        let pair = self.normal.denoise_pair.clone().filter(|d| d.photo == photo);
        let lang = self.lang;
        if on {
            let made = pair.filter(|d| d.plain == active).and_then(|d| d.denoised).and_then(|id| self.card_index(&id));
            if let Some(idx) = made {
                self.show_card_keeping_look(idx);
                self.status = tr(lang, "AI denoise on").into();
                return;
            }
            self.normal.denoise_pair = Some(DenoisePair { photo, plain: active, denoised: None });
            self.start_ai_denoise();
            if self.busy {
                self.status = tr(lang, "Removing the grain… this takes a moment.").into();
            } else {
                // Refused before it ran: its own line stands.
                self.normal.denoise_pair = None;
            }
        } else {
            let back = pair
                .filter(|d| d.denoised.as_deref() == Some(active.as_str()))
                .and_then(|d| self.card_index(&d.plain))
                .or_else(|| self.variants.iter().position(|v| v.kind != VariantKind::Denoised));
            let Some(idx) = back else { return };
            // Remembered, so turning it on again shows this ◈ card.
            self.normal.denoise_pair = Some(DenoisePair { photo, plain: self.variants[idx].id.clone(), denoised: Some(active) });
            self.show_card_keeping_look(idx);
            self.status = tr(lang, "AI denoise off").into();
        }
    }

    /// After a retouch landed (`Msg::Retouched`): when it was the noise
    /// switch's AI denoise, remember its ◈ card and say so plainly.
    pub(crate) fn after_normal_denoise(&mut self, lang: Lang) {
        let Some(d) = self.normal.denoise_pair.as_mut() else { return };
        if d.denoised.is_some() || self.src_path.as_ref() != Some(&d.photo) {
            return;
        }
        let Some(v) = self.variants.get(self.active).filter(|v| v.kind == VariantKind::Denoised && v.id != d.plain) else {
            return;
        };
        d.denoised = Some(v.id.clone());
        if self.mode.shell().one_click_sidebar {
            self.reword_landing(tr(lang, "AI denoise done. Turn the switch off to see the photo without it.").into());
        }
    }

    /// The status bar's marker in Normal: the open photo's card and look
    /// against the card and look it opened with or was last saved as a jpg
    /// with (`look_mark`, re-anchored whenever another photo is open) —
    /// 「saved」 only while the card on screen, with its current look, is what
    /// was last saved. `None` = nothing to say: unchanged since it opened and
    /// not saved yet.
    pub(crate) fn normal_look_mark(&mut self) -> Option<LookMark> {
        let photo = self.src_path.clone()?;
        let card = self.active_source_path();
        if self.normal.look_mark.as_ref().is_none_or(|(p, ..)| *p != photo) {
            self.normal.look_mark = Some((photo, card.clone(), self.recipe.clone(), false));
        }
        let (_, at, look, saved) = self.normal.look_mark.as_ref()?;
        if *look != self.recipe || *at != card {
            Some(LookMark::Unsaved)
        } else if *saved {
            Some(LookMark::Saved)
        } else {
            None
        }
    }

    /// The one-click group's line, in the session language.
    pub(crate) fn normal_line_text(&self, lang: Lang) -> String {
        if self.normal.building {
            return match self.normal.build_progress {
                Some((done, total)) => trf(
                    lang,
                    "Reading your reference photos… {done} / {total}",
                    &[("done", &done.to_string()), ("total", &total.to_string())],
                ),
                None => tr(lang, "Reading your reference photos…").into(),
            };
        }
        match &self.normal.line {
            // A cancelled one-click leaves no worker behind to say so.
            NormalLine::Working if self.analyze_inflight => tr(lang, "Making the look…").into(),
            NormalLine::Idle | NormalLine::Working => tr(lang, "Pick a look, or match your own reference photos.").into(),
            NormalLine::Done => tr(lang, "Done — Ctrl+Z goes back").into(),
            NormalLine::BuiltIn => tr(lang, "No AI key set — built-in look used").into(),
            NormalLine::Failed(why) => why.clone(),
        }
    }

    /// The sidebar: the four groups, behind the same mid-open gate as the
    /// shared panels (L15-2 — these controls write the open photo's recipe).
    pub(crate) fn normal_panel(&mut self, ui: &mut egui::Ui) {
        if self.normal.key_present.is_none() {
            self.normal.key_present = Some(autoshade::config::Config::load().openai_api_key.is_some());
        }
        let editable = !self.open_in_flight;
        ui.add_enabled_ui(editable, |ui| {
            self.normal_one_click(ui);
            self.normal_fine_tune(ui);
            self.normal_noise(ui);
            self.normal_save(ui);
        });
    }

    /// Group 1 — the one-click look: six fixed looks, or the reference folder.
    fn normal_one_click(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_LG);
        panel_heading(ui, tr(lang, "One-click look"), false, |_| {});
        let ready = self.src_path.is_some() && !self.busy && !self.analyze_inflight && !self.normal.building;
        let names: [&str; 6] = [
            tr(lang, "Natural"),
            tr(lang, "Bright & clean"),
            tr(lang, "Warm"),
            tr(lang, "Cool night"),
            tr(lang, "Film"),
            tr(lang, "Black & white"),
        ];
        let presets = &autoshade::advisor::presets::PRESETS;
        let mut asked: Option<OneClick> = None;
        // Two to a row: at three, "Bright & clean" and "Black & white" are cut short.
        for row in 0..3 {
            ui.horizontal(|ui| {
                let cell = columns(ui, 2);
                for col in 0..2 {
                    let i = row * 2 + col;
                    if action_in(ui, cell, ready, names[i])
                        .on_hover_text(tr(lang, "The AI looks at this photo and gives it this look — Ctrl+Z goes back"))
                        .clicked()
                    {
                        asked = Some(OneClick::Preset(presets[i].id));
                    }
                }
            });
        }
        ui.vertical_centered(|ui| ui.label(egui::RichText::new(tr(lang, "or")).weak().small()));
        let pick = action_row(ui, ready, tr(lang, "Match my reference photos…"))
            .on_hover_text(tr(
                lang,
                "Pick a folder of photos whose look you like. They are read on this computer (the first time, a 1.5 GB model is downloaded for it); then the AI looks at this photo and up to 3 of them, each billed as one more picture.",
            ))
            .clicked();
        if let Some(dir) = &self.normal.ref_dir {
            let name = dir.file_name().map_or_else(|| abs_display(dir), |n| n.to_string_lossy().into_owned());
            ui.label(egui::RichText::new(trf(lang, "Chosen: {name}", &[("name", &name)])).weak().small());
        }
        Self::slider_pct_hinted(
            ui,
            lang,
            tr(lang, "Look strength"),
            &mut self.grade_strength,
            1.0,
            GRADE_STRENGTH_DEFAULT,
            tr(lang, "How strong the look is. Double-click to reset."),
        );
        let line = self.normal_line_text(lang);
        ui.horizontal_wrapped(|ui| {
            if self.normal.building || (self.normal.line == NormalLine::Working && self.analyze_inflight) {
                ui.spinner();
            }
            ui.label(egui::RichText::new(line).small());
        });
        if self.normal.key_present == Some(false) {
            ui.label(
                egui::RichText::new(tr(lang, "To make looks with AI, add a service key in Settings (⚙)."))
                    .weak()
                    .small(),
            );
        }
        if let Some(ask) = asked {
            self.start_one_click(ask);
        }
        // The dialog after the group is laid out: it is modal and blocks.
        if pick {
            let mut dialog = rfd::FileDialog::new();
            if let Some(d) = self.normal.ref_dir.clone().filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(d);
            }
            if let Some(dir) = dialog.pick_folder() {
                self.start_one_click_folder(dir);
            }
        }
    }

    /// Group 2 — fine-tune: four of the develop panel's fields under plain
    /// names, on top of the one-click result.
    fn normal_fine_tune(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_LG);
        panel_heading(ui, tr(lang, "Fine-tune"), false, |_| {});
        let mut changed = false;
        let r = &mut self.recipe;
        changed |= Self::slider(ui, lang, tr(lang, "Brightness"), &mut r.exposure_ev, -5.0, 5.0, 0.0);
        changed |= Self::slider(ui, lang, tr(lang, "Contrast"), &mut r.contrast, -100.0, 100.0, 0.0);
        let mut warmth = warmth_of(r);
        if Self::slider_hinted(
            ui,
            lang,
            tr(lang, "Warmth"),
            &mut warmth,
            -100.0,
            100.0,
            0.0,
            tr(lang, "Cooler to the left, warmer to the right; 0 is the camera's own colour."),
        ) {
            r.temperature_k = Some(kelvin_for_warmth(r, warmth));
            changed = true;
        }
        changed |= Self::slider(ui, lang, tr(lang, "Vividness"), &mut r.saturation, -100.0, 100.0, 0.0);
        let back = self
            .normal
            .result
            .as_ref()
            .filter(|(p, f)| self.src_path.as_ref() == Some(p) && *f != FineTune::of(&self.recipe))
            .map(|(_, f)| *f);
        if action_row(ui, back.is_some(), tr(lang, "Back to the one-click result")).clicked()
            && let Some(f) = back
        {
            f.apply(&mut self.recipe);
            changed = true;
        }
        if changed {
            self.recipe.clamp();
            self.dirty = true;
        }
    }

    /// Group 3 — noise: one switch, the Retouch panel's AI denoise
    /// (`set_normal_denoise`).
    fn normal_noise(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_LG);
        panel_heading(ui, tr(lang, "Noise"), false, |_| {});
        // A run that ended without its card (failed, cancelled) leaves no
        // pair behind.
        if !self.busy && self.normal.denoise_pair.as_ref().is_some_and(|d| d.denoised.is_none()) {
            self.normal.denoise_pair = None;
        }
        let helper = denoise_helper_available();
        let mut on = self.normal_denoise_on();
        let toggled = ui
            .add_enabled(helper && !self.busy, egui::Checkbox::new(&mut on, tr(lang, "AI denoise")))
            .on_hover_text(tr(lang, "Cleans the grain out of the photo, and the preview shows the result — it helps night and dark phone photos most; it uses the graphics card when there is one. Turn it off to see the photo without it."))
            .changed();
        if helper && self.normal.denoise_ready.is_none() {
            let cfg = autoshade::config::Config::load();
            let weight = std::path::Path::new(&cfg.weights_dir).join(format!("scunet_{}.pth", cfg.denoise_model));
            self.normal.denoise_ready = Some(weight.is_file());
        }
        if toggled {
            self.set_normal_denoise(on);
        }
        for line in self.noise_lines(lang, helper) {
            ui.label(egui::RichText::new(line).weak().small());
        }
    }

    /// The lines under the noise switch, given whether the denoise helper is
    /// installed (a process-wide probe, so it is passed in).
    pub(crate) fn noise_lines(&self, lang: Lang, helper: bool) -> Vec<String> {
        if !helper {
            return vec![tr(lang, "This download does not include the noise tool.").into()];
        }
        // The switch is the Retouch panel's denoise: the preview shows it.
        let mut lines = vec![tr(lang, "It takes a moment; the preview then shows the result, and that is what gets saved.").to_string()];
        if self.normal.denoise_ready != Some(true) && !self.denoise_uses_raw() {
            lines.push(trf(
                lang,
                "The first time, the noise model is downloaded (about {mb} MB).",
                &[("mb", &DENOISE_FIRST_DOWNLOAD_MB.to_string())],
            ));
        }
        lines
    }

    /// Group 4 — save: a jpg of this photo, or the look on every photo of the
    /// folder.
    fn normal_save(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_LG);
        panel_heading(ui, tr(lang, "Save"), false, |_| {});
        let ready = self.src_path.is_some() && !self.busy;
        let mut save = false;
        ui.horizontal(|ui| {
            let cell = columns(ui, 2);
            save = primary_in(ui, cell, ready, tr(lang, "Save as jpg"))
                .on_hover_text(tr(lang, "Saves the photo with this look as a jpg; the original file is never changed."))
                .clicked();
            egui::ComboBox::from_id_salt("normal_quality")
                .selected_text(format!("{} · {}", tr(lang, "Quality"), self.normal.quality.label(lang)))
                .width(cell.x)
                .show_ui(ui, |ui| {
                    for q in SaveQuality::ALL {
                        ui.selectable_value(&mut self.normal.quality, q, q.label(lang));
                    }
                });
        });
        // Never beside the photo (the library is read-only): the app's output
        // folder from Settings, or the folder last saved to, or a dialog —
        // the one destination setting the workspace's Export reads.
        let place = match (self.exp_dest, self.export_dest_dir()) {
            (_, None) => tr(lang, "Asks where to save each time").to_string(),
            (ExportDest::LastUsed, Some(d)) => {
                trf(lang, "Saves to the folder you last saved to: {folder}", &[("folder", &folder_name(&d))])
            }
            (_, Some(d)) => trf(
                lang,
                "Saves to the app's output folder {folder} (change it in Settings ⚙)",
                &[("folder", &folder_name(&d))],
            ),
        };
        let full = self.export_dest_dir().map(|d| abs_display(&d)).unwrap_or_default();
        ui.label(egui::RichText::new(place).weak().small()).on_hover_text(full);
        let all = action_row(ui, ready && !self.gallery.is_empty(), tr(lang, "Apply this look to every photo in the folder"))
            .on_hover_text(tr(lang, "Gives every photo in the open folder this look and saves each one as a jpg."))
            .on_disabled_hover_text(tr(lang, "Open a folder first — the library on the left."))
            .clicked();
        if save {
            self.start_normal_save();
        }
        if all {
            self.start_apply_to_folder();
        }
    }
}
