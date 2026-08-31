//! Settings panel and its form load/save.

use crate::*;

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

    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut do_save = false;
        let mut fetch: Option<ModelRole> = None;
        // `lang` is a Copy snapshot so `tr`/`trf` never borrow `self` — the
        // `let f = &mut self.settings` block below holds a partial borrow of self.
        let lang = self.lang;
        ui.label(
            // "& Theme", not "& Reverse-fit": the reverse-fit switch this
            // sentence used to point at now lives in the AI panel (R22 #4), and
            // Theme is the other control here that applies without a save.
            egui::RichText::new(tr(
                lang,
                "Language & Theme apply immediately. The provider sections below persist via 「Save settings」 to autoshade.local.json in your per-user AutoShade folder (never in a repo) and apply to the next AI call (Analyze / Fill / Reimagine).",
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
        // 「Zoned fit (sky)」 MOVED to the AI panel's reverse-fit sub-area (R22
        // #4): it is a setting for ONE button, and it sat two panels away from
        // that button — you changed it here and then went hunting for 「🎛
        // Reverse-fit」. The `zoned_fit` FIELD is untouched (same Prefs key,
        // same persistence, same `start_fit` read); only its UI row moved.
        ui.separator();
        // The develop store is otherwise invisible (hashed AppData folders) —
        // this is the one place that names it and rescues pre-store saves.
        ui.heading(tr(lang, "Develop store"));
        // THIS photo's folder, not just the root (R22-8): the root is one line
        // above a hash-named subdirectory the user then has to identify by
        // guessing, and "where is my XMP" is the question this row exists to
        // answer. Falls back to the root when no photo is open, which is all
        // there is to say then.
        let shown = match self.src_path.as_deref() {
            Some(p) => autoshade::store::develop_dir(p),
            None => autoshade::store::store_root(),
        };
        ui.label(egui::RichText::new(abs_display(&shown)).small().weak()).on_hover_text(tr(
            lang,
            "Where saved develops live: recipes, Lightroom XMP, version snapshots and mask rasters — one folder per photo, keyed by its absolute path. Override the location with the AUTOSHADE_DATA_DIR environment variable.",
        ));
        // Enabled only when the folder EXISTS: an unsaved photo has no develop
        // directory yet, and a file manager pointed at a missing path silently
        // opens somewhere else entirely (Explorer lands in Documents). One stat
        // per frame, but only while this window is open — a cached answer would
        // be wrong for exactly the case that matters (the user saves, then
        // reaches for this button). `develop_dir` above costs nothing repeated:
        // its identity resolution is memoized for the process (store::
        // identity_of), and `store_root` was already read here every frame.
        let exists = shown.is_dir();
        if ui
            .add_enabled(exists, egui::Button::new(tr(lang, "🗂 Show in file manager")))
            .on_hover_text(if exists {
                tr(lang, "Open this folder in your file manager")
            } else {
                tr(lang, "Nothing saved for this photo yet — the folder appears with the first save")
            })
            .clicked()
            && let Err(e) = reveal_folder(&shown)
        {
            let t = trf(lang, "could not open the folder: {err}", &[("err", &e.to_string())]);
            self.status = t.clone();
            self.toast(ToastKind::Error, t);
        }
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
        // --- the DELIVERY ROOT (R24-5 M8) ------------------------------------
        // The counterpart to the develop store above: that folder holds the
        // recipes, this one holds the finished files. It used to be the
        // hardcoded cwd-relative `./out` in five places at once (the CLI, the
        // batch renderer, the web download route, the GUI's destination
        // setting, the style-prompt writer), which is why "where did my export
        // go" had no answer but "wherever you launched the app from".
        ui.separator();
        ui.heading(tr(lang, "Delivery folder"));
        let resolved = autoshade::config::delivery_root();
        {
            let f = &mut self.settings;
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut f.out_dir)
                        .desired_width(FIELD_W_MAX.min(ui.available_width() - 90.0).max(80.0))
                        .hint_text(autoshade::config::DEFAULT_DELIVERY_ROOT),
                )
                .on_hover_text(format!(
                    "{}\n\n{}",
                    tr(
                        lang,
                        "Where finished files land: exports, AI/retouch pixel masters and the extracted style prompt — for this window, the CLI, the web surface and batch renders alike. Blank = the default ./out beside the working directory. Saved develops are NOT here (see 「Develop store」 above).",
                    ),
                    // R24 round-end LOW-3: choosing a folder inside the photo
                    // library RETIRES that folder's read-only protection
                    // (`guard_readonly` allows the delivery root before it
                    // refuses the RAW's own folder), and nothing said so.
                    tr(
                        lang,
                        "Pointing it inside your photo library removes that folder's read-only protection: AutoShade refuses to write beside your originals, but never into its own delivery folder.",
                    ),
                ));
                if ui
                    .button(tr(lang, "Browse…"))
                    .on_hover_text(tr(lang, "Pick the delivery folder"))
                    .clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    f.out_dir = dir.display().to_string();
                }
            });
        }
        // The RESOLVED root, absolute: a relative setting (the default
        // included) means nothing without saying which directory it is
        // relative to — the same reason the export summary spells its target
        // out in full. Reflects the SAVED value, so it only moves after
        // 「Save settings」.
        ui.label(egui::RichText::new(abs_display(&resolved)).small().weak());
        // The DYNAMIC half of the warning above: this window knows which photo
        // is open, so it can say that THIS root and THAT photo's folder
        // overlap rather than leaving the user to notice. Reads the typed
        // field (the choice being made now); blank falls back to the saved
        // resolution, which is what a blank field will resolve to.
        let typed = self.settings.out_dir.trim();
        let chosen = if typed.is_empty() { resolved } else { std::path::PathBuf::from(typed) };
        if delivery_root_shadows_photo(&chosen, self.src_path.as_deref()) {
            ui.label(
                egui::RichText::new(tr(
                    lang,
                    "⚠ This folder and the open photo's folder are inside one another — the photo's folder is no longer protected as read-only, so a render can land beside your originals.",
                ))
                .small()
                .weak(),
            );
        }
        // --- the PYTHON INTERPRETER (M1-3) -----------------------------------
        // WRITABLE here, unlike the sidecar path below, and that is not the
        // same ruling softened. `AUTOSHADE_PYTHON` stays `Trust::Destination`
        // — no `.env`, no working-directory `autoshade.local.json` beside
        // someone's photos may supply it — and this field writes ONLY the
        // trusted per-user file, which is the authority the environment
        // variable already had. What changed is REACHABILITY: a
        // Finder-launched `.app` inherits launchd's environment, not a
        // shell's, so on macOS an env-only setting is one no user of the
        // shipped app can set at all.
        //
        // The button probes FIXED ABSOLUTE candidates
        // (`config::PYTHON_CANDIDATES`) — never a file dialog, never a `PATH`
        // scan — so a value the user did not type by hand can still only be an
        // installer-owned location.
        ui.separator();
        // One resolved config for this row and the sidecar row below; the
        // panel used to load it once per row.
        let cfg_now = autoshade::config::Config::load();
        ui.heading(tr(lang, "Python interpreter"));
        {
            let f = &mut self.settings;
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut f.python_bin)
                        .desired_width(FIELD_W_MAX.min(ui.available_width() - 90.0).max(80.0))
                        .hint_text(autoshade::config::default_python_bin()),
                )
                .on_hover_text(tr(
                    lang,
                    "Which Python runs the AI sidecars (segmentation, denoise, style). Blank = the platform default. It can only be set here or by the AUTOSHADE_PYTHON environment variable — never by a file that arrives beside your photos.",
                ));
                if ui
                    .button(tr(lang, "Detect"))
                    .on_hover_text(tr(
                        lang,
                        "Look in the standard install locations for a working Python 3",
                    ))
                    .clicked()
                {
                    // `probe_python_bin` RUNS each candidate (`--version`)
                    // before offering it: on a Mac without developer tools
                    // `/usr/bin/python3` exists and does not work, so an
                    // existence check alone would fill this field with a stub
                    // whose only behaviour is an install prompt.
                    match autoshade::config::probe_python_bin() {
                        Some(bin) => {
                            f.status = trf(lang, "found {bin}", &[("bin", &bin)]);
                            f.python_bin = bin;
                        }
                        // A real answer, said as one: "we looked in the
                        // standard places and found nothing" is actionable,
                        // a silently unchanged field is a button that did
                        // nothing.
                        None => {
                            f.status = tr(
                                lang,
                                "no Python found in the standard install locations — type the full path above",
                            )
                            .into();
                        }
                    }
                }
            });
        }
        // What the app will ACTUALLY run. The field above holds the FILE's
        // spelling and an `AUTOSHADE_PYTHON` in the environment outranks it,
        // so a user whose environment already answers would otherwise be
        // looking at an empty box.
        ui.label(egui::RichText::new(cfg_now.python_bin.clone()).small().weak());
        // --- the SEGMENTATION SIDECAR path (R25, closing R22-1) --------------
        // R22 left "a settings row for the sidecar path" to R23; R23 and R24
        // did not do it, and this is why. `AUTOSHADE_SEGMENT_SCRIPT` names a
        // file that goes straight into `Command::new`, so `config.rs`
        // registers it `env_only(..., Trust::Destination)`: neither a `.env`
        // nor an ambient `autoshade.local.json` beside someone's photos may
        // supply it. Adding a picker here would write it into the trusted
        // settings file and hand every later launch a program chosen in a
        // dialog — the opposite of that ruling. The interpreter row ABOVE is
        // not a counter-example to it: that one offers a fixed candidate list
        // rather than a dialog, and it exists because a Finder-launched app
        // has no environment to read it from. This path is different on the
        // fact that decides it — it already has a working default inside the
        // app's own tree, so nobody needs a picker to make the app run.
        //
        // So this is READ-ONLY on purpose, and it says why. The two facts a
        // user actually needs are which file is resolved and whether it is
        // there; the existing 「did not ship the python sidecar」 line (the one
        // the two 🤖 buttons already show) is the answer for the missing case,
        // so the missing arm is not a second wording of the same thing.
        ui.separator();
        ui.heading(tr(lang, "Segmentation sidecar"));
        let seg = cfg_now.segment_script;
        let seg_path = std::path::PathBuf::from(&seg);
        ui.label(egui::RichText::new(abs_display(&seg_path)).small().weak()).on_hover_text(tr(
            lang,
            "This path can only be set by environment variable, because it is executed",
        ));
        // `segment_helper_available` is the SAME predicate the two AI-select
        // buttons gate on (one `exists()` per process), so this row and those
        // buttons can never disagree about whether the helper is there.
        if !segment_helper_available() {
            ui.label(
                egui::RichText::new(tr(lang,
                    "this build did not ship the python sidecar — run AutoShade from the project directory, or point AUTOSHADE_SEGMENT_SCRIPT at python/segment.py",
                ))
                .small()
                .weak(),
            );
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
        // The icon's fine print (round-13 easter egg).
        ui.add_space(SPACE_SM);
        ui.label(
            egui::RichText::new(tr(
                lang,
                "skymanbp's AS — the “As” stands for AutoShade, not an Adobe subscription. Rent paid to date: $0.00.",
            ))
            .weak()
            .small(),
        );
        if do_save {
            self.save_settings_form();
        }
        if let Some(role) = fetch {
            self.fetch_models(role);
        }
    }
}
