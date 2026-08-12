//! Settings panel and its form load/save.

use crate::*;

impl AutoshopApp {
    /// Populate the Settings form from the resolved config (keys are shown only as
    /// "present", never revealed). Called when the window opens.
    pub(crate) fn load_settings_form(&mut self) {
        let cfg = autoshop::config::Config::load();
        // Keep any model lists already fetched this session so reopening Settings
        // doesn't force a re-fetch — and keep an in-flight fetch's flag alive:
        // zeroing it mid-fetch stopped the repaint pump AND re-armed the fetch
        // button (duplicate requests, stalled status). The catalogues also
        // carry the per-role auto-fetch guards and generation stamps.
        let image_models = std::mem::take(&mut self.settings.image_models);
        let analysis_models = std::mem::take(&mut self.settings.analysis_models);
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
            status: String::new(),
            image_models,
            analysis_models,
        };
        // Availability is credential-dependent: another surface (the web
        // Settings) may have replaced a saved key since these lists were
        // fetched, and the URL-keyed self-invalidation in `settings_ui`
        // cannot see that. Compare each kept catalogue's credential stamp
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
    pub(crate) fn autofetch_models_once(&mut self, cfg: &autoshop::config::Config) {
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

    /// Persist the Settings form to autoshop.local.json (gitignored). A blank key
    /// keeps the stored one. The next Analyze/Export reloads Config, so it applies.
    pub(crate) fn save_settings_form(&mut self) {
        // The whole load-merge-save runs under the cross-process settings
        // lock (`config::update_local_settings`): this panel and the serve
        // process merge onto the same file, and the old unlocked cycle here
        // let a save that landed between its load and its rename be erased.
        let form = &self.settings;
        let saved = autoshop::config::update_local_settings(|cur| {
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
                let cfg = autoshop::config::Config::load();
                self.settings.analysis_key_present = cfg.analysis_api_key.is_some();
                self.settings.image_key_present = cfg.openai_api_key.is_some();
                // A key that cannot appear in an HTTP header is REFUSED by
                // Config::load (`header_safe_key` — a newline or space from a
                // paste is the usual cause). The refusal only prints to
                // stderr, which the windowed GUI does not show, so "saved"
                // beside a still-empty key state was the whole story the user
                // got. Say it here instead.
                let refused = (typed_image && !self.settings.image_key_present)
                    || (typed_analysis && !self.settings.analysis_key_present);
                self.settings.status = if refused {
                    tr(self.lang, "saved, but the key was not accepted — it contains characters that cannot appear in an HTTP header (a stray space or newline from a copy/paste?). Re-copy it and save again.").into()
                } else {
                    trf(self.lang, "saved → {path}", &[("path", &p.display().to_string())])
                };
                self.status = tr(self.lang, "settings saved — applies to the next AI call (Analyze / Fill / Reimagine)").into();
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
        let cfg = autoshop::config::Config::load();
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
        } else if autoshop::config::same_endpoint(&base, &cfg_base) {
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
            move || Msg::Models(role, generation, autoshop::openai_models::list_models(&base, &key)),
            move |e| Msg::Models(role, generation, Err(e)),
        );
    }

    pub(crate) fn catalogue_mut(&mut self, role: ModelRole) -> &mut ModelCatalogue {
        match role {
            ModelRole::Image => &mut self.settings.image_models,
            ModelRole::Analysis => &mut self.settings.analysis_models,
        }
    }

    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut do_save = false;
        let mut fetch: Option<ModelRole> = None;
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
            // that role's URL stops matching (typed edit, provider auto-swap),
            // they describe a DIFFERENT server — self-invalidate so the
            // pickers fall back to grounded defaults, not a stale menu.
            if !f.image_models.is_empty() && !same_base(&f.image_base_url, &f.image_models.from_base)
            {
                f.image_models.clear();
            }
            if !f.analysis_models.is_empty()
                && !same_base(&f.analysis_base_url, &f.analysis_models.from_base)
            {
                f.analysis_models.clear();
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
                    // versa). Swap to this provider's default on a flip —
                    // but ONLY what provably belongs to the other provider:
                    // see `analysis_model_on_flip` for what a name can and
                    // cannot decide here.
                    if let Some(m) =
                        analysis_model_on_flip(&f.analysis_model, f.analysis_provider_api)
                    {
                        f.analysis_model = m.into();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Model"));
                // OAuth uses Claude CLI aliases; API uses the ids fetched from
                // the ANALYSIS endpoint — its own probe, not the image role's.
                // Borrowing the image list meant a separate analysis endpoint
                // offered nothing but two hardcoded ids.
                let opts = if f.analysis_provider_api {
                    model_opts(&f.analysis_models.chat, &["gpt-5.5", "gpt-4o"], &f.analysis_model)
                } else {
                    model_opts(&[], &CLAUDE_ALIASES, &f.analysis_model)
                };
                model_picker(ui, "set_analysis_model", &mut f.analysis_model, &opts, lang);
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Reasoning effort"));
                // The CLI and the API endpoints accept different tier names —
                // offer the right list, and keep the free-text field for
                // anything a given endpoint adds later.
                let tiers: &[&str] =
                    if f.analysis_provider_api { &EFFORT_TIERS_API } else { &EFFORT_TIERS_CLI };
                effort_picker(ui, "set_analysis_effort", &mut f.analysis_effort, tiers, lang);
            });
            if f.analysis_provider_api {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Base URL"));
                    ui.text_edit_singleline(&mut f.analysis_base_url);
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "API Key"));
                    let hint = if f.analysis_key_present { tr(lang, "key set — blank keeps it") } else { tr(lang, "no key set") };
                    if ui
                        .add(egui::TextEdit::singleline(&mut f.analysis_api_key).password(true).hint_text(hint))
                        .changed()
                    {
                        // Availability is CREDENTIAL-dependent as well as
                        // URL-dependent — see the image role's key field.
                        f.analysis_models.clear();
                    }
                });
                ui.horizontal(|ui| {
                    let label = if f.analysis_models.fetching {
                        tr(lang, "fetching…")
                    } else {
                        tr(lang, "🔄 Fetch models")
                    };
                    if ui
                        .add_enabled(!f.analysis_models.fetching, egui::Button::new(label))
                        .on_hover_text(tr(
                            lang,
                            "List the models THIS endpoint serves (GET /models). The analysis role has its own endpoint and key, so it gets its own list.",
                        ))
                        .clicked()
                    {
                        fetch = Some(ModelRole::Analysis);
                    }
                    if !f.analysis_models.chat.is_empty() {
                        ui.label(
                            egui::RichText::new(trf(
                                lang,
                                "{chat} chat",
                                &[("chat", &f.analysis_models.chat.len().to_string())],
                            ))
                            .weak()
                            .small(),
                        );
                    }
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
                let label = if f.image_models.fetching { tr(lang, "fetching…") } else { tr(lang, "🔄 Fetch models") };
                let clicked = ui
                    .add_enabled(!f.image_models.fetching, egui::Button::new(label))
                    .on_hover_text(tr(
                        lang,
                        "List the models this endpoint serves (GET /models) so you can pick instead of guess — and a live reachability check for the bridge/API. Uses the key/token typed below; a saved key is only used at the endpoint it was saved for.",
                    ))
                    .clicked();
                if clicked {
                    fetch = Some(ModelRole::Image);
                }
                if !f.image_models.is_empty() {
                    let cn = f.image_models.chat.len().to_string();
                    let im = f.image_models.image_gen.len().to_string();
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
                let opts = model_opts(&f.image_models.chat, &["gpt-5.5", "gpt-4o"], &f.image_model);
                model_picker(ui, "set_vision_model", &mut f.image_model, &opts, lang);
            });
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Reasoning effort"));
                effort_picker(ui, "set_image_effort", &mut f.image_effort, &EFFORT_TIERS_API, lang);
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
                let opts = model_opts(&f.image_models.image_gen, fallbacks, &f.image_gen_model);
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
                    f.image_models.clear();
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
        if let Some(role) = fetch {
            self.fetch_models(role);
        }
    }
}
