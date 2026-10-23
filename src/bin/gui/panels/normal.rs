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
//! develop panel writes, the noise switch is the export-time AI denoise at its
//! default strength, and the Save group is the single export and the
//! paste-then-render batch with a jpg's options. The words are plain (separate
//! i18n keys, design §6): none of the workspace's trade terms appear here.

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
/// plus its network file (`python/denoise.py` `WEIGHT_BYTES["color_real_psnr"]`
/// + `NETWORK_BYTES`, 71 994 286 bytes), in MB — pinned by a test against
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
    /// The noise switch.
    pub(crate) denoise: bool,
    /// Whether the rendered-image denoiser's weight is on disk, read when the
    /// switch is turned on (`None` = not read yet).
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
    /// The photo and look a running Save-group delivery is saving.
    pub(crate) saving: Option<(PathBuf, EditRecipe)>,
    /// The status-bar marker's anchor: the photo, the look it is measured
    /// from (as opened, or as last saved as a jpg), and whether that look
    /// was saved (`normal_look_mark`).
    pub(crate) look_mark: Option<(PathBuf, EditRecipe, bool)>,
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

    /// 「Save as jpg」: the single export, as a jpg at the quality step, to the
    /// export destination (asked when the destination is "ask"), with the AI
    /// denoise at its default strength when the noise switch is on.
    pub(crate) fn start_normal_save(&mut self) {
        let Some(src) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let name = self.export_file_name_as("jpg");
        let out = match self.export_dest_dir() {
            Some(dir) => dir.join(name),
            None => {
                let mut dlg = rfd::FileDialog::new().add_filter("jpg", &["jpg"]).set_file_name(name);
                if let Some(d) = self.last_export_dir.clone().filter(|d| d.is_dir()) {
                    dlg = dlg.set_directory(d);
                }
                let Some(p) = dlg.save_file() else { return };
                normalize_export_target(p, "jpg")
            }
        };
        let denoise = self.normal.denoise.then(|| autoshade::denoise::default_strength_for(&src));
        let export = self.normal_export_opts();
        if self.start_render_with(out, export, denoise, false) {
            self.normal.delivering = true;
            self.normal.saving = self.src_path.clone().map(|p| (p, self.recipe.clone()));
        }
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
        self.start_batch_render_for(targets, "jpg".into(), export, dest);
        if self.busy {
            self.normal.delivering = true;
            // The open photo is one of them: its look counts as saved too.
            self.normal.saving = self.src_path.clone().map(|p| (p, self.recipe.clone()));
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
            ExportOutcome::Single { out, .. } => {
                let dir = out.parent().map(folder_name).unwrap_or_default();
                (Some(trf(lang, "Saved to {folder}", &[("folder", &dir)])), true)
            }
            ExportOutcome::Batch { ok, errs, dest, .. } if errs.is_empty() => (
                Some(trf(lang, "{n} photos saved to {folder}", &[("n", &ok.to_string()), ("folder", &folder_name(dest))])),
                true,
            ),
            // Every failure was the refusal to replace earlier photos
            // (counted by type in the worker): the plain sentence.
            ExportOutcome::Batch { errs, refused, .. } if *refused == errs.len() => {
                (Some(tr(lang, "That folder already holds photos saved earlier. Pick another folder, or move the old ones first.").to_string()), false)
            }
            ExportOutcome::Batch { .. } => (None, false),
        };
        // Delivered whole: the look that went out is the photo's saved jpg
        // (a batch with failures names no photo, so it marks none).
        if whole && let Some((photo, look)) = saving {
            self.normal.look_mark = Some((photo, look, true));
        }
        words
    }

    /// Normal's words for `pipeline::OverwriteRefused` — matched by type
    /// (anywhere in the chain), never by its English; `None` for any other
    /// error, or outside Normal, where the library's own line stands.
    pub(crate) fn plain_refusal(&self, lang: Lang, e: &anyhow::Error) -> Option<String> {
        (self.mode.shell().one_click_sidebar && e.chain().any(|c| c.is::<autoshade::pipeline::OverwriteRefused>()))
            .then(|| tr(lang, "That folder already holds photos saved earlier. Pick another folder, or move the old ones first.").to_string())
    }

    /// The status bar's marker in Normal: the open photo's look against the
    /// look it opened with or was last saved as a jpg with (`look_mark`,
    /// re-anchored whenever another photo is open). `None` = nothing to say:
    /// unchanged since it opened and not saved yet.
    pub(crate) fn normal_look_mark(&mut self) -> Option<LookMark> {
        let photo = self.src_path.clone()?;
        if self.normal.look_mark.as_ref().is_none_or(|(p, _, _)| *p != photo) {
            self.normal.look_mark = Some((photo, self.recipe.clone(), false));
        }
        let (_, look, saved) = self.normal.look_mark.as_ref()?;
        if *look != self.recipe {
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

    /// Group 3 — noise: one switch, the export-time AI denoise at its default
    /// strength.
    fn normal_noise(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_LG);
        panel_heading(ui, tr(lang, "Noise"), false, |_| {});
        let helper = denoise_helper_available();
        let toggled = ui
            .add_enabled(helper, egui::Checkbox::new(&mut self.normal.denoise, tr(lang, "AI denoise")))
            .on_hover_text(tr(lang, "Cleans the grain out of the photo when you save it — it helps night and dark phone photos most; slow, and it uses the graphics card when there is one."))
            .changed();
        if toggled && self.normal.denoise && self.normal.denoise_ready.is_none() {
            let cfg = autoshade::config::Config::load();
            let weight = std::path::Path::new(&cfg.weights_dir).join(format!("scunet_{}.pth", cfg.denoise_model));
            self.normal.denoise_ready = Some(weight.is_file());
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
        // The switch is the export-time denoise: the canvas never shows it.
        let mut lines = vec![tr(lang, "Applied when you save; not shown in the preview.").to_string()];
        if self.normal.denoise && self.normal.denoise_ready == Some(false) && !self.source_is_raw() {
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
