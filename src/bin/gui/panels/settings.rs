//! Settings panel and its form load/save.
//!
//! The window is the conventional two-pane settings window: a navigation
//! column of five pages on the left, the open page on the right (its groups
//! and form rows live in `settings/pages.rs`), and a footer pinned under both
//! that holds 「Save settings」 for the pages whose fields it saves.

use crate::*;

/// The Settings window's pages, in navigation order. Session state on the app
/// (`settings_tab`), never persisted: the window opens where it was left
/// while the app runs, and on General after a restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub(crate) enum SettingsTab {
    /// Language, theme, accent — they apply at once.
    #[default]
    General,
    /// The delivery folder and the develop store.
    Folders,
    /// The analysis model and the image model.
    Models,
    /// The Python interpreter and the segmentation helper.
    Advanced,
    /// The name and the version.
    About,
}

impl SettingsTab {
    pub(crate) const ALL: [SettingsTab; 5] =
        [SettingsTab::General, SettingsTab::Folders, SettingsTab::Models, SettingsTab::Advanced, SettingsTab::About];

    /// The page's name in the navigation and as its title.
    pub(crate) fn label(self, lang: Lang) -> &'static str {
        match self {
            SettingsTab::General => tr(lang, "General"),
            SettingsTab::Folders => tr(lang, "Folders"),
            SettingsTab::Models => tr(lang, "AI models"),
            SettingsTab::Advanced => tr(lang, "Advanced"),
            SettingsTab::About => tr(lang, "About"),
        }
    }

    /// Whether the page holds fields that persist through 「Save settings」
    /// (the others apply at once, and their footer says so).
    fn saves(self) -> bool {
        matches!(self, SettingsTab::Folders | SettingsTab::Models | SettingsTab::Advanced)
    }
}

/// The Settings window's fixed size, before the screen caps it.
const SETTINGS_W: f32 = 780.0;
const SETTINGS_H: f32 = 580.0;
/// The navigation column's width.
const SETTINGS_NAV_W: f32 = 168.0;

// Test seams: every form row's label and the left edge of its control cell
// (`pages::form_row`), and the page column's rect inside the scrollbar
// gutter — as laid this frame, on this thread.
#[cfg(test)]
thread_local! {
    pub(crate) static FORM_CELLS: std::cell::RefCell<Vec<(String, f32)>> = const { std::cell::RefCell::new(Vec::new()) };
    pub(crate) static PAGE_COLUMN: std::cell::Cell<egui::Rect> = const { std::cell::Cell::new(egui::Rect::NOTHING) };
}

mod pages;

/// The Settings window's fixed size on a `screen`: 780 × 580 where it fits,
/// narrower than the screen by a margin and at most 85 % of its height where
/// it does not — the page scrolls inside, so 「Save settings」 never ends up
/// off-screen on a small display.
pub(crate) fn settings_window_size(screen: egui::Rect) -> egui::Vec2 {
    egui::vec2(SETTINGS_W.min(screen.width() - 48.0), SETTINGS_H.min(screen.height() * 0.85))
}

impl AutoShadeApp {
    /// Populate the Settings form from the resolved config (keys are shown only as
    /// "present", never revealed). Called when the window opens.
    pub(crate) fn load_settings_form(&mut self) {
        let cfg = autoshade::config::Config::load();
        // Keep any model lists already fetched this session so reopening Settings
        // doesn't force a re-fetch — and keep an in-flight fetch's flag alive:
        // zeroing it mid-fetch stopped the repaint pump AND re-armed the fetch
        // button (duplicate requests, stalled status). The catalogues also
        // carry the per-role auto-fetch guards and generation stamps.
        let image_models = std::mem::take(&mut self.settings.image_models);
        let analysis_models = std::mem::take(&mut self.settings.analysis_models);
        // ONE read of the settings file for the two fields below that show the
        // file's own spelling rather than the resolved value.
        let local = autoshade::config::load_local_settings();
        self.settings = SettingsForm {
            analysis_provider_api: cfg.analysis_is_api(),
            image_provider_oauth: cfg.image_is_oauth(),
            analysis_model: cfg.analysis_model.clone(),
            analysis_base_url: cfg.analysis_base_url.clone(),
            analysis_api_key: String::new(),
            analysis_key_present: cfg.analysis_api_key.is_some(),
            analysis_effort: cfg.analysis_effort.clone().unwrap_or_default(),
            image_model: cfg.openai_model.clone(),
            image_base_url: cfg.openai_base_url.clone(),
            image_gen_model: cfg.openai_image_model.clone(),
            image_api_key: String::new(),
            image_key_present: cfg.openai_api_key.is_some(),
            image_effort: cfg.image_effort.clone().unwrap_or_default(),
            image_api_family: cfg.image_api_family.clone(),
            image_gen_custom: cfg.image_gen_endpoint.is_some(),
            image_gen_endpoint: cfg.image_gen_endpoint.clone().unwrap_or_default(),
            // The FILE's own spelling, not the resolved root: this is the
            // field the user edits, and showing a resolved absolute path in it
            // would silently turn "use the default" into a pinned folder on
            // the next save. `load_local_settings` (not the raw file) so an
            // ambient working-directory copy cannot pre-fill it either.
            out_dir: local.out_dir.unwrap_or_default(),
            // Same rule for the interpreter (M1-3): showing the RESOLVED
            // program here would turn "use the default" into a pinned path on
            // the next save, and `load_local_settings` is what keeps an
            // ambient working-directory copy from pre-filling either field.
            python_bin: local.python_bin.unwrap_or_default(),
            status: String::new(),
            image_models,
            analysis_models,
        };
        // Availability is credential-dependent: another surface (the web
        // Settings) may have replaced a saved key since these lists were
        // fetched, and the URL-keyed self-invalidation on the AI models
        // page (`pages.rs`) cannot see that. Compare each kept catalogue's credential stamp
        // against today's key and drop a stale list here, at the one moment
        // the resolved config is already in hand.
        for role in [ModelRole::Image, ModelRole::Analysis] {
            let fp = key_fingerprint(match role {
                ModelRole::Image => cfg.openai_api_key.as_deref().unwrap_or(""),
                ModelRole::Analysis => cfg.analysis_api_key.as_deref().unwrap_or(""),
            });
            let cat = self.catalogue_mut(role);
            if !cat.is_empty() && cat.from_key != fp {
                cat.clear();
            }
        }
        self.autofetch_models_once(&cfg);
    }

    /// First time Settings opens in a session, fill the pick-lists without
    /// making the user find the button — "which models can I use here" is the
    /// question the panel exists to answer, and a blank dropdown next to a
    /// configured key reads as "none available".
    ///
    /// Once per session PER ROLE, and only for a role that already has a key:
    /// a probe with no credential can only fail, and firing on every open
    /// would put a network call behind "I came here to switch the language".
    ///
    /// The guard is consumed at DISPATCH (`fetch_models` marks the role), not
    /// by the visit itself: one global open-consumed boolean meant a Settings
    /// visit before any key existed spent the whole session's opportunity, so
    /// the role that became eligible five minutes later — key saved, provider
    /// flipped to `api` — was never probed on any later open.
    pub(crate) fn autofetch_models_once(&mut self, cfg: &autoshade::config::Config) {
        if cfg.openai_api_key.is_some() && !self.settings.image_models.autofetched {
            self.fetch_models(ModelRole::Image);
        }
        // The analysis endpoint only has a catalogue to fetch in `api` mode —
        // the OAuth verifier is the `claude` CLI, which serves no /models.
        if cfg.analysis_is_api()
            && cfg.analysis_api_key.is_some()
            && !self.settings.analysis_models.autofetched
        {
            self.fetch_models(ModelRole::Analysis);
        }
    }

    /// Persist the Settings form to autoshade.local.json (gitignored). A blank key
    /// keeps the stored one. The next Analyze/Export reloads Config, so it applies.
    pub(crate) fn save_settings_form(&mut self) {
        // The whole load-merge-save runs under the cross-process settings
        // lock (`config::update_local_settings`): this panel and the serve
        // process merge onto the same file, and the old unlocked cycle here
        // let a save that landed between its load and its rename be erased.
        let form = &self.settings;
        let saved = autoshade::config::update_local_settings(|cur| {
            cur.analysis_provider =
                Some(if form.analysis_provider_api { "api" } else { "oauth" }.to_string());
            cur.image_provider =
                Some(if form.image_provider_oauth { "oauth" } else { "api" }.to_string());
            cur.analysis_model = Some(form.analysis_model.trim().to_string());
            cur.analysis_base_url = Some(form.analysis_base_url.trim().to_string());
            cur.image_model = Some(form.image_model.trim().to_string());
            cur.image_base_url = Some(form.image_base_url.trim().to_string());
            cur.image_gen_model = Some(form.image_gen_model.trim().to_string());
            // Effort: an empty field is a real choice — "let the provider
            // decide" — so it is stored as empty rather than skipped, or
            // clearing the field could never take effect.
            cur.analysis_effort = Some(form.analysis_effort.trim().to_string());
            cur.image_effort = Some(form.image_effort.trim().to_string());
            cur.image_api_family = Some(form.image_api_family.trim().to_string());
            // Stored blank for OpenAI's call, so the choice can undo a template.
            cur.image_gen_endpoint = Some(if form.image_gen_custom {
                form.image_gen_endpoint.trim().to_string()
            } else {
                String::new()
            });
            // The delivery root (M8), same explicit-blank rule as the two
            // efforts: an emptied field is a real choice ("the default
            // ./out"), so it is STORED empty rather than skipped, or clearing
            // it could never take effect. `update_local_settings` drops the
            // memo, so the next export claim reads this value.
            cur.out_dir = Some(form.out_dir.trim().to_string());
            // The interpreter (M1-3), same explicit-blank rule: an emptied
            // field is the real choice "use the platform default", and only a
            // STORED blank can undo a path saved earlier.
            cur.python_bin = Some(form.python_bin.trim().to_string());
            // Secrets: only overwrite when a non-empty value was actually
            // typed — and a typed key is FOR the endpoint on screen beside
            // it, so record that home (`config::file_key_for` enforces it at
            // load: a later provider flip or base edit cannot re-route it).
            let ak = form.analysis_api_key.trim().to_string();
            let ik = form.image_api_key.trim().to_string();
            if !ak.is_empty() {
                cur.analysis_api_key = Some(ak);
                cur.analysis_api_key_base =
                    cur.analysis_base_url.clone().filter(|s| !s.trim().is_empty());
            }
            if !ik.is_empty() {
                cur.image_api_key = Some(ik);
                cur.image_api_key_base =
                    cur.image_base_url.clone().filter(|s| !s.trim().is_empty());
            }
        });
        // Did the user type a key on this save? Asked BEFORE the fields are
        // cleared, so the refusal check below can tell "typed and rejected"
        // from "left blank on purpose".
        let typed_image = !self.settings.image_api_key.trim().is_empty();
        let typed_analysis = !self.settings.analysis_api_key.trim().is_empty();
        match saved {
            Ok(p) => {
                self.settings.analysis_api_key.clear();
                self.settings.image_api_key.clear();
                // Presence reflects the RESOLVED config (file merged with env) —
                // deriving it from the file alone told a user whose key lives in
                // OPENAI_API_KEY that no key was set right after saving.
                let cfg = autoshade::config::Config::load();
                self.settings.analysis_key_present = cfg.analysis_api_key.is_some();
                self.settings.image_key_present = cfg.openai_api_key.is_some();
                // Normal's sidebar says whether its looks use the AI.
                self.normal.key_present = Some(cfg.openai_api_key.is_some());
                // A key that cannot appear in an HTTP header is REFUSED by
                // Config::load (`header_safe_key` — a newline or space from a
                // paste is the usual cause). The refusal only prints to
                // stderr, which the windowed GUI does not show, so "saved"
                // beside a still-empty key state was the whole story the user
                // got. Say it here instead.
                let refused = (typed_image && !self.settings.image_key_present)
                    || (typed_analysis && !self.settings.analysis_key_present);
                // The file's HOME decides what `Config::load` reads back:
                // under the shared temp root (no per-user data directory on
                // this machine) it is not trusted with a key or an endpoint,
                // and only the models survive the merge. Said HERE — the
                // loader's warning goes to stderr, which the windowed GUI
                // never shows, so "saved" was the whole story until 2026-09-24.
                let shared = autoshade::config::load_local_settings_from().1
                    == autoshade::config::SettingsOrigin::SharedRoot;
                self.settings.status = if refused {
                    tr(self.lang, "saved, but the key was not accepted — it contains characters that cannot appear in an HTTP header (a stray space or newline from a copy/paste?). Re-copy it and save again.").into()
                } else if shared {
                    trf(self.lang, "saved → {path} — but this file sits in the shared temp folder, which is not trusted with keys: the API key and base URL fields will be ignored (only the models are read). Set AUTOSHADE_DATA_DIR to a folder only you can write and save again.", &[("path", &p.display().to_string())])
                } else {
                    trf(self.lang, "saved → {path}", &[("path", &p.display().to_string())])
                };
                self.status = tr(self.lang, "settings saved — applies to the next AI call (Analyze / Fill / Generate image)").into();
            }
            Err(e) => {
                self.settings.status =
                    trf(self.lang, "save failed: {err}", &[("err", &e.to_string())])
            }
        }
    }

    /// Fetch one role's model ids (`GET /models`) on a worker thread and fill that
    /// role's pick-lists. Uses the key/base typed in the form if present, else the
    /// saved config — so it works whether or not the user has saved a key yet.
    ///
    /// The base and key resolve HERE, on the UI thread, so the catalogue's
    /// `from_base`/`from_key` stamps describe exactly what the worker sends
    /// (the worker used to re-load Config on its own thread, so the stamp and
    /// the request could disagree about which credential was used).
    pub(crate) fn fetch_models(&mut self, role: ModelRole) {
        let (form_key, form_base) = match role {
            ModelRole::Image => (
                self.settings.image_api_key.trim().to_string(),
                self.settings.image_base_url.trim().to_string(),
            ),
            ModelRole::Analysis => (
                self.settings.analysis_api_key.trim().to_string(),
                self.settings.analysis_base_url.trim().to_string(),
            ),
        };
        let cfg = autoshade::config::Config::load();
        let (cfg_base, cfg_key) = match role {
            ModelRole::Image => (cfg.openai_base_url, cfg.openai_api_key),
            ModelRole::Analysis => (cfg.analysis_base_url, cfg.analysis_api_key),
        };
        let base = if form_base.is_empty() { cfg_base.clone() } else { form_base };
        // The SAVED key is only ever sent to the endpoint it is saved beside —
        // `config::file_key_for` is this same rule at load time. Probing a
        // freshly TYPED endpoint takes a typed (or first saved) credential;
        // silently reusing the old endpoint's key against the new URL is the
        // provider-flip misroute in miniature. With no usable key the probe
        // fails immediately with "no API key — set one in Settings", which is
        // the accurate instruction.
        let key = if !form_key.is_empty() {
            form_key
        } else if autoshade::config::same_endpoint(&base, &cfg_base) {
            cfg_key.unwrap_or_default()
        } else {
            String::new()
        };
        let cat = self.catalogue_mut(role);
        if cat.fetching {
            return;
        }
        cat.fetching = true;
        // Any dispatch — auto or manual — spends this role's convenience probe.
        cat.autofetched = true;
        // Drop the previous endpoint's lists NOW (bumping `gen`, so an older
        // in-flight completion can no longer land) and stamp what this fetch
        // targets: a failed fetch then leaves empty lists (grounded fallbacks)
        // rather than ids from the old server under the new URL's name.
        cat.clear();
        cat.from_base = base.clone();
        cat.from_key = key_fingerprint(&key);
        let generation = cat.generation;
        self.settings.status = tr(self.lang, "fetching models…").into();
        // spawn_worker's catch_unwind guarantees the UI's `fetching` flag
        // always clears — a panic still delivers Msg::Models(role, gen, Err)
        // (this site used to hand-roll a Drop guard for exactly that; the
        // helper now covers every worker uniformly).
        self.spawn_worker(
            move || Msg::Models(role, generation, autoshade::openai_models::list_models(&base, &key)),
            move |e| Msg::Models(role, generation, Err(e)),
        );
    }

    pub(crate) fn catalogue_mut(&mut self, role: ModelRole) -> &mut ModelCatalogue {
        match role {
            ModelRole::Image => &mut self.settings.image_models,
            ModelRole::Analysis => &mut self.settings.analysis_models,
        }
    }

    /// The whole Settings window content: the navigation on the left, the
    /// footer pinned at the bottom, the open page in a scroll column between.
    /// The window host stays one call (`AutoShadeApp::settings_window`).
    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        // The footer first, so it spans both columns under them (a panel
        // takes its side of whatever room the panels before it left).
        let footer_tab = self.settings_tab;
        let save = egui::TopBottomPanel::bottom("settings_footer")
            .frame(egui::Frame::none().inner_margin(egui::Margin { top: SPACE_MD, ..Default::default() }))
            .show_inside(ui, |ui| self.settings_footer(ui, footer_tab))
            .inner;
        egui::SidePanel::left("settings_nav")
            .exact_width(SETTINGS_NAV_W)
            .resizable(false)
            .frame(egui::Frame::none().inner_margin(egui::Margin { right: SPACE_LG, ..Default::default() }))
            .show_inside(ui, |ui| self.settings_nav(ui));
        // Read after the navigation, so a click lands on its page this frame
        // (the footer follows on the next).
        let tab = self.settings_tab;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin { left: SPACE_LG, ..Default::default() }))
            .show_inside(ui, |ui| {
                // The same right gutter the controls column keeps: egui's
                // scrollbar floats over the content and reserves no width.
                let gutter = controls_gutter(ui.style());
                egui::ScrollArea::vertical()
                    .id_salt(("settings_page", tab))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Frame::none()
                            .inner_margin(egui::Margin { right: gutter, ..Default::default() })
                            .show(ui, |ui| {
                                #[cfg(test)]
                                PAGE_COLUMN.set(ui.max_rect());
                                self.settings_page(ui, tab);
                            });
                    });
            });
        if save {
            self.save_settings_form();
        }
    }

    /// The five pages, one row each, the open one filled like the gallery's
    /// selected row.
    fn settings_nav(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let c = self.theme.colors();
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
            let strong = ui.visuals().strong_text_color();
            let v = ui.visuals_mut();
            v.selection.bg_fill = c.sel_bg;
            v.selection.stroke.color = strong;
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
                w.rounding = RADIUS_MD.into();
            }
            for tab in SettingsTab::ALL {
                if ui.selectable_label(self.settings_tab == tab, tab.label(lang)).clicked() {
                    self.settings_tab = tab;
                }
            }
        });
    }

    /// The footer under both columns: on the pages whose fields persist, the
    /// status line (or what saving does) and 「Save settings」 at the right;
    /// on the others, that their changes apply at once. True when Save was
    /// clicked.
    fn settings_footer(&mut self, ui: &mut egui::Ui, tab: SettingsTab) -> bool {
        let lang = self.lang;
        let mut save = false;
        ui.horizontal(|ui| {
            if !tab.saves() {
                ui.label(egui::RichText::new(tr(lang, "Changes on this page apply at once")).weak().small());
                return;
            }
            let verb = tr(lang, "Save settings");
            let left = (ui.available_width() - pages::trailing_w(ui, verb)).max(0.0).floor();
            let hint = if self.settings.status.is_empty() {
                tr(lang, "Changes on this page take effect after 「Save settings」").to_owned()
            } else {
                self.settings.status.clone()
            };
            ui.allocate_ui_with_layout(egui::vec2(left, row_h(ui)), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_width(left);
                ui.add(egui::Label::new(egui::RichText::new(hint).weak().small()).truncate());
            });
            save = primary(ui, true, verb)
                .on_hover_text(tr(lang, "Saves to autoshade.local.json in your per-user AutoShade folder (never in a repo)"))
                .clicked();
        });
        save
    }
}
