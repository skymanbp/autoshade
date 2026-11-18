//! The Settings window's five pages and the form anatomy they share
//! (2026-11-17, user: the window "looks a bit ugly and non-standard").
//!
//! It was one 480 px column of nine sections fenced by full-width hairlines,
//! each label/control row starting its control wherever its label happened to
//! end, fields of three widths, and walls of grey explanation. A page is now
//! made of GROUPS — a small caption over a card — and every line of a card is
//! a FORM ROW: a label cell of one fixed width, then the control cell. The
//! alignment comes from [`form_row`] alone, never from per-row spacing, so on
//! every page all controls start at the same x. Long explanations fold into
//! [`explainer`] lines or stay as hover text; warnings and status lines stay
//! visible.
//!
//! Layout only: every side effect a control had (the provider-flip model swap,
//! the bridge/OpenAI URL swap, a key edit dropping its role's catalogue, the
//! URL-keyed catalogue invalidation, a fetch) sits beside its control exactly
//! as before, and `save_settings_form` reads only `self.settings`, so a save
//! from any page saves the same values.

use crate::*;

use super::SettingsTab;

/// The label cell of every form row — ONE width, so every control of a page
/// starts at the same x, and a long label wraps inside its cell instead of
/// pushing its control to the right.
pub(crate) const FORM_LABEL_W: f32 = 132.0;

/// One group of a page: a small caption over a card that spans the page.
/// The caption has `headings::group_caption`'s look (small, spaced, weak)
/// without its leading air — the page puts `SPACE_LG` between groups itself,
/// and no hairline.
fn settings_card<R>(ui: &mut egui::Ui, caption: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.label(
        egui::RichText::new(caption)
            .small()
            .strong()
            .extra_letter_spacing(0.8)
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(SPACE_XS);
    let c = theme_of(ui).colors();
    let r = egui::Frame::none()
        .fill(c.card_fill)
        .stroke(egui::Stroke::new(1.0, c.card_stroke))
        .rounding(RADIUS_MD)
        .inner_margin(SPACE_LG)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = SPACE_MD;
            body(ui)
        })
        .inner;
    ui.add_space(SPACE_LG);
    r
}

/// One line of a card: the label cell ([`FORM_LABEL_W`] wide, its text
/// wrapping inside it and set level with the first control line), then the
/// control cell, a column holding the control and any help line under it.
/// An empty `label` leaves the cell blank (a row of buttons that belongs to
/// the rows above it).
pub(crate) fn form_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        let row = row_h(ui);
        let line = ui.text_style_height(&egui::TextStyle::Body);
        ui.allocate_ui_with_layout(egui::vec2(FORM_LABEL_W, row), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(FORM_LABEL_W);
            if !label.is_empty() {
                ui.add_space(((row - line) / 2.0).max(0.0));
                ui.add(egui::Label::new(label).wrap());
            }
        });
        ui.vertical(|ui| {
            #[cfg(test)]
            super::FORM_CELLS.with_borrow_mut(|c| c.push((label.to_owned(), ui.max_rect().left())));
            add(ui)
        })
        .inner
    })
    .inner
}

/// A help line under its control, in the control column: weak, small, and
/// wrapping to the column's width.
fn form_help(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).weak().small()).wrap());
}

/// A warning under its control — visible text in the theme's warning colour.
fn form_warn(ui: &mut egui::Ui, text: &str) {
    let warn = ui.visuals().warn_fg_color;
    ui.add(egui::Label::new(egui::RichText::new(text).color(warn).small()).wrap());
}

/// How wide a field is: the space left in the row after `trailing` (an
/// own-width button beside it), capped at the one readable width every
/// Settings field and dropdown shares.
fn field_w(ui: &egui::Ui, trailing: f32) -> f32 {
    (ui.available_width() - trailing).clamp(FIELD_W_MIN, FIELD_W_MAX).floor()
}

/// The room an own-width text verb takes beside a field: its label, the
/// button's padding, and the gap before it — so a row with a button shortens
/// its field and the button stays on the row.
pub(crate) fn trailing_w(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let label = ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE).size().x);
    (label + 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x).ceil()
}

/// A single-line field `w` wide on the outside, one row tall like the
/// dropdowns and buttons beside it (`util::field_margin`, the one margin of
/// every text field in the app).
fn field_edit<'t>(ui: &egui::Ui, text: &'t mut String, w: f32) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text)
        .desired_width((w - 2.0 * FIELD_MARGIN_X).max(0.0))
        .margin(field_margin(ui))
}

/// A path: weak and small, cut to the width it is given from the FRONT — the
/// file and its folder are the informative end — so a long path never wraps
/// its row taller or widens the window. The whole path opens in the hover
/// text, in front of `why` when there is one.
fn path_label(ui: &mut egui::Ui, path: &str, why: Option<&str>) -> egui::Response {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let shown = elide_front(ui, path, &font, ui.available_width());
    let tip = match why {
        Some(why) => format!("{path}\n\n{why}"),
        None => path.to_owned(),
    };
    ui.add(egui::Label::new(egui::RichText::new(shown).weak().small()).wrap_mode(egui::TextWrapMode::Extend))
        .on_hover_text(tip)
}

/// [`path_label`] in a cell exactly `w` wide, level with the row's controls —
/// the cell a field would take, so the button after it lines up with the
/// buttons after the fields.
fn path_cell(ui: &mut egui::Ui, w: f32, path: &str, why: Option<&str>) {
    ui.allocate_ui_with_layout(egui::vec2(w, row_h(ui)), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(w);
        path_label(ui, path, why);
    });
}

/// `path` as it fits in `max_w`: whole, or "…" and the longest tail that fits
/// (a binary search over the char boundaries — a handful of layouts, which
/// egui caches, not one per character).
fn elide_front(ui: &egui::Ui, path: &str, font: &egui::FontId, max_w: f32) -> String {
    let width = |s: &str| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE).size().x);
    if width(path) <= max_w {
        return path.to_owned();
    }
    let starts: Vec<usize> = path.char_indices().map(|(i, _)| i).collect();
    let tail = |k: usize| starts.get(k).map_or("", |&i| &path[i..]);
    // The fewest leading characters to drop: the empty tail always fits.
    let (mut lo, mut hi) = (0, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if width(&format!("…{}", tail(mid))) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    format!("…{}", tail(lo))
}

/// Image-gen API: OpenAI's `/images/edits`, or a call the user describes as a
/// JSON template (`autoshade::image_endpoint`). Two choices and no provider
/// presets (user ruling 2026-10-17): any provider is one user's template, and
/// the empty box shows the shape to start from.
fn image_gen_api_ui(ui: &mut egui::Ui, f: &mut SettingsForm, lang: Lang) {
    use autoshade::image_endpoint::{ImageEndpoint, EXAMPLE};
    form_row(ui, tr(lang, "Image-gen API"), |ui| {
        ui.radio_value(&mut f.image_gen_custom, false, tr(lang, "OpenAI standard (/images/edits)"));
        ui.radio_value(&mut f.image_gen_custom, true, tr(lang, "Custom"));
        if !f.image_gen_custom {
            return;
        }
        ui.add(
            egui::TextEdit::multiline(&mut f.image_gen_endpoint)
                .code_editor()
                .desired_rows(8)
                .desired_width(f32::INFINITY)
                .hint_text(EXAMPLE),
        );
        // A blank template saves as OpenAI's call (`config::Config::load` reads
        // blank as unset), so say that rather than flag it as broken.
        if f.image_gen_endpoint.trim().is_empty() {
            form_help(ui, tr(lang, "empty — saved as OpenAI standard"));
        } else {
            match ImageEndpoint::parse(&f.image_gen_endpoint) {
                Ok(_) => form_help(ui, tr(lang, "✓ template is valid")),
                Err(e) => form_warn(ui, &format!("⚠ {e:#}")),
            }
        }
        explainer(
            ui,
            lang,
            tr(
                lang,
                "Describe the provider's image-edit call as JSON, sent to the Base URL above with the API key above: \"path\" is added to the Base URL, \"body\" is what is sent, and \"reply\" is \"image\" when the answer is the picture itself, or a JSON pointer to its base64 (default /data/0/b64_json). In the body, {model} {prompt} {image} {image_data_url} {mask} {mask_data_url} {marked} {marked_data_url} {marked_note} {width} {height} {size} {quality} are filled in; {mask} is white where the picture should change. For a provider with no mask parameter, send {marked} (the picture with the painted area tinted red) as another image and put {marked_note} after {prompt}: it tells the model to change only the red area. All three are left out when nothing is painted.",
            ),
        );
    });
}

impl AutoShadeApp {
    /// ONE page of the Settings window — no navigation, no footer — at the
    /// width of `ui`. The window draws it inside its scroll column; the tests
    /// draw each page through here.
    pub(crate) fn settings_page(&mut self, ui: &mut egui::Ui, tab: SettingsTab) {
        ui.heading(tab.label(self.lang));
        ui.add_space(SPACE_LG);
        match tab {
            SettingsTab::General => self.settings_general(ui),
            SettingsTab::Folders => self.settings_folders(ui),
            SettingsTab::Models => self.settings_models(ui),
            SettingsTab::Advanced => self.settings_advanced(ui),
            SettingsTab::About => self.settings_about(ui),
        }
    }

    /// Language, theme and accent: they apply at once, no save.
    fn settings_general(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        settings_card(ui, tr(lang, "Appearance"), |ui| {
            form_row(ui, tr(lang, "Language"), |ui| {
                // English is the skeleton; Chinese is an overlay. Switching takes effect
                // next frame (every label re-reads `self.lang`), no restart/save needed.
                // `from_id_salt` is egui 0.29's name for the old `from_id_source`.
                // The pick goes through `set_language`, the one door the mode
                // page's switch uses too.
                let mut picked = self.lang;
                egui::ComboBox::from_id_salt("lang_picker")
                    .selected_text(self.lang.label())
                    .width(field_w(ui, 0.0))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut picked, Lang::En, Lang::En.label());
                        ui.selectable_value(&mut picked, Lang::Zh, Lang::Zh.label());
                    });
                if picked != self.lang {
                    self.set_language(picked);
                }
            });
            // Four surface schemes and an accent picked apart from them (see
            // ThemeColors) — switching either re-installs the egui style this
            // frame and persists with the other prefs.
            let before = (self.theme, self.accent);
            form_row(ui, tr(lang, "Theme"), |ui| {
                egui::ComboBox::from_id_salt("theme_picker")
                    .selected_text(tr(lang, self.theme.label()))
                    .width(field_w(ui, 0.0))
                    .show_ui(ui, |ui| {
                        for t in ThemePref::ALL {
                            ui.selectable_value(&mut self.theme, t, tr(lang, t.label()));
                        }
                    });
            });
            form_row(ui, tr(lang, "Accent colour"), |ui| {
                ui.horizontal(|ui| {
                    for a in AccentPref::ALL {
                        if accent_swatch(ui, a.hue(), self.accent == a).on_hover_text(tr(lang, a.label())).clicked() {
                            self.accent = a;
                        }
                    }
                });
            });
            if (self.theme, self.accent) != before {
                install_look(ui.ctx(), self.theme, self.accent);
            }
        });
        // 「Zoned fit (sky)」 MOVED to the AI panel's reverse-fit sub-area (R22
        // #4): it is a setting for ONE button, and it sat two panels away from
        // that button — you changed it here and then went hunting for 「🎛
        // Reverse-fit」. Since 2026-11-10 it is part of that fold's stepped
        // 「Reverse-fit strength」 (`panels/fit_level.rs`).
    }

    /// Where finished files go and where saved develops live.
    fn settings_folders(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        // --- the DELIVERY ROOT (R24-5 M8) ------------------------------------
        // The counterpart to the develop store below: that folder holds the
        // recipes, this one holds the finished files. It used to be the
        // hardcoded cwd-relative `./out` in five places at once (the CLI, the
        // batch renderer, the web download route, the GUI's destination
        // setting, the style-prompt writer), which is why "where did my export
        // go" had no answer but "wherever you launched the app from".
        let resolved = autoshade::config::delivery_root();
        settings_card(ui, tr(lang, "Delivery folder"), |ui| {
            form_row(ui, tr(lang, "Folder"), |ui| {
                let browse = tr(lang, "Browse…");
                ui.horizontal(|ui| {
                    let w = field_w(ui, trailing_w(ui, browse));
                    ui.add(field_edit(ui, &mut self.settings.out_dir, w).hint_text(autoshade::config::DEFAULT_DELIVERY_ROOT))
                        .on_hover_text(format!(
                            "{}\n\n{}",
                            tr(
                                lang,
                                "Where finished files land: exports and the extracted style prompt — for this window, the CLI, the web surface and batch renders alike. Blank = the default ./out beside the working directory. Saved develops and the pixel masters cards are built on are NOT here (see 「Develop store」 below), so cleaning this folder never breaks a card.",
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
                    if action(ui, true, browse)
                        .on_hover_text(tr(lang, "Pick the delivery folder"))
                        .clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.settings.out_dir = dir.display().to_string();
                    }
                });
                // The RESOLVED root, absolute: a relative setting (the default
                // included) means nothing without saying which directory it is
                // relative to — the same reason the export summary spells its target
                // out in full. Reflects the SAVED value, so it only moves after
                // 「Save settings」.
                path_label(ui, &abs_display(&resolved), None);
                // The DYNAMIC half of the warning above: this window knows which photo
                // is open, so it can say that THIS root and THAT photo's folder
                // overlap rather than leaving the user to notice. Reads the typed
                // field (the choice being made now); blank falls back to the saved
                // resolution, which is what a blank field will resolve to.
                let typed = self.settings.out_dir.trim();
                let chosen = if typed.is_empty() { resolved.clone() } else { std::path::PathBuf::from(typed) };
                if delivery_root_shadows_photo(&chosen, self.src_path.as_deref()) {
                    form_warn(
                        ui,
                        tr(
                            lang,
                            "⚠ This folder and the open photo's folder are inside one another — the photo's folder is no longer protected as read-only, so a render can land beside your originals.",
                        ),
                    );
                }
            });
        });
        // The develop store is otherwise invisible (hash-named data folders) —
        // this is the one place that names it and rescues pre-store saves.
        settings_card(ui, tr(lang, "Develop store"), |ui| {
            // THIS photo's folder, not just the root (R22-8): the root is one line
            // above a hash-named subdirectory the user then has to identify by
            // guessing, and "where is my XMP" is the question this row exists to
            // answer. Falls back to the root when no photo is open, which is all
            // there is to say then.
            let (label, shown) = match self.src_path.as_deref() {
                Some(p) => (tr(lang, "This photo"), autoshade::store::develop_dir(p)),
                None => (tr(lang, "All photos"), autoshade::store::store_root()),
            };
            form_row(ui, label, |ui| {
                let open = tr(lang, "🗂 Show in file manager");
                ui.horizontal(|ui| {
                    let w = field_w(ui, trailing_w(ui, open));
                    path_cell(
                        ui,
                        w,
                        &abs_display(&shown),
                        Some(tr(
                            lang,
                            "Where saved develops live: recipes, Lightroom XMP, version snapshots and mask rasters — one folder per photo, keyed by its absolute path. Override the location with the AUTOSHADE_DATA_DIR environment variable.",
                        )),
                    );
                    // Enabled only when the folder EXISTS: an unsaved photo has no develop
                    // directory yet, and a file manager pointed at a missing path silently
                    // opens somewhere else entirely (Explorer lands in Documents). One stat
                    // per frame, but only while this page is drawn — a cached answer would
                    // be wrong for exactly the case that matters (the user saves, then
                    // reaches for this button). `develop_dir` above costs nothing repeated:
                    // its identity resolution is memoized for the process (store::
                    // identity_of), and `store_root` was already read here every frame.
                    let exists = shown.is_dir();
                    if action(ui, exists, open)
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
                });
            });
            // The pixel masters (2026-10-18): the AI denoise / stack / heal /
            // clone / fill results cards are built FROM. They left the delivery
            // folder so cleaning exports cannot break a card, which also hid
            // them — this row names the folder and opens it.
            let masters = autoshade::store::masters_dir();
            form_row(ui, tr(lang, "Pixel masters"), |ui| {
                let open = tr(lang, "🗂 Show masters folder");
                ui.horizontal(|ui| {
                    let w = field_w(ui, trailing_w(ui, open));
                    path_cell(
                        ui,
                        w,
                        &abs_display(&masters),
                        Some(tr(
                            lang,
                            "The full-resolution images AI denoise, stacking, heal, clone, fill and AI edits produce — the cards built on them read these files. Deleting one breaks its card, so delete the card first. Exports are NOT here (see 「Delivery folder」 above).",
                        )),
                    );
                    let has_masters = masters.is_dir();
                    if action(ui, has_masters, open)
                        .on_hover_text(if has_masters {
                            tr(lang, "Open this folder in your file manager")
                        } else {
                            tr(lang, "No master yet — the folder appears with the first AI denoise, stack or retouch")
                        })
                        .clicked()
                        && let Err(e) = reveal_folder(&masters)
                    {
                        let t = trf(lang, "could not open the folder: {err}", &[("err", &e.to_string())]);
                        self.status = t.clone();
                        self.toast(ToastKind::Error, t);
                    }
                });
            });
            form_row(ui, "", |ui| {
                if action(ui, true, tr(lang, "Import develops from an old ./out folder…"))
                    .on_hover_text(tr(
                        lang,
                        "Saves made before v0.13 lived in a ./out folder next to wherever the app was launched. If your old edits are missing, point this at that folder — its recipes / XMP / versions migrate into the develop store.",
                    ))
                    .clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.start_import_legacy(dir);
                }
            });
        });
    }

    /// The two AI roles: the analysis model and the image model.
    fn settings_models(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let mut fetch: Option<ModelRole> = None;
        let f = &mut self.settings;
        // Fetched ids belong to the endpoint recorded at fetch time; once
        // that role's URL stops matching (typed edit, provider auto-swap),
        // they describe a DIFFERENT server — self-invalidate so the
        // pickers fall back to grounded defaults, not a stale menu.
        if !f.image_models.is_empty() && !same_base(&f.image_base_url, &f.image_models.from_base) {
            f.image_models.clear();
        }
        if !f.analysis_models.is_empty() && !same_base(&f.analysis_base_url, &f.analysis_models.from_base) {
            f.analysis_models.clear();
        }
        settings_card(ui, tr(lang, "Analysis model — used by AI Analyze, AI adjust and AI review"), |ui| {
            form_row(ui, tr(lang, "Provider"), |ui| {
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
                    if let Some(m) = analysis_model_on_flip(&f.analysis_model, f.analysis_provider_api) {
                        f.analysis_model = m.into();
                    }
                }
            });
            if f.analysis_provider_api {
                form_row(ui, tr(lang, "Base URL"), |ui| {
                    let w = field_w(ui, 0.0);
                    ui.add(field_edit(ui, &mut f.analysis_base_url, w));
                });
                form_row(ui, tr(lang, "API Key"), |ui| {
                    let hint = if f.analysis_key_present { tr(lang, "key set — blank keeps it") } else { tr(lang, "no key set") };
                    let w = field_w(ui, 0.0);
                    if ui.add(field_edit(ui, &mut f.analysis_api_key, w).password(true).hint_text(hint)).changed() {
                        // Availability is CREDENTIAL-dependent as well as
                        // URL-dependent — see the image role's key field.
                        f.analysis_models.clear();
                    }
                });
                form_row(ui, "", |ui| {
                    ui.horizontal(|ui| {
                        let label = if f.analysis_models.fetching { tr(lang, "fetching…") } else { tr(lang, "🔄 Fetch models") };
                        if action(ui, !f.analysis_models.fetching, label)
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
                                egui::RichText::new(trf(lang, "{chat} chat", &[("chat", &f.analysis_models.chat.len().to_string())]))
                                    .weak()
                                    .small(),
                            );
                        }
                    });
                });
            }
            form_row(ui, tr(lang, "Model"), |ui| {
                // OAuth uses Claude CLI aliases; API uses the ids fetched from
                // the ANALYSIS endpoint — its own probe, not the image role's.
                // Borrowing the image list meant a separate analysis endpoint
                // offered nothing but two hardcoded ids.
                let opts = if f.analysis_provider_api {
                    model_opts(&f.analysis_models.chat, &["gpt-5.5", "gpt-4o"], &f.analysis_model)
                } else {
                    model_opts(&[], &CLAUDE_ALIASES, &f.analysis_model)
                };
                let w = field_w(ui, 0.0);
                ui.horizontal(|ui| model_picker(ui, "set_analysis_model", &mut f.analysis_model, &opts, lang, w));
            });
            form_row(ui, tr(lang, "Reasoning effort"), |ui| {
                // The CLI and the API endpoints accept different tier names —
                // offer the right list, and keep the free-text field for
                // anything a given endpoint adds later.
                // …and the model's OWN list where the listing said (part D):
                // a `/models` that names the tiers a model takes is the
                // detection the fixed suggestions stand in for.
                let tiers: Vec<&str> = if f.analysis_provider_api {
                    f.analysis_models.tiers_for(&f.analysis_model, &EFFORT_TIERS_API)
                } else {
                    EFFORT_TIERS_CLI.to_vec()
                };
                let w = field_w(ui, 0.0);
                ui.horizontal(|ui| effort_picker(ui, "set_analysis_effort", &mut f.analysis_effort, &tiers, lang, w));
                if f.analysis_provider_api && f.analysis_models.efforts.contains_key(f.analysis_model.trim()) {
                    tiers_note(ui, tiers.is_empty(), lang);
                }
            });
        });
        settings_card(ui, tr(lang, "Image model — looks at your photo, and generates or edits images"), |ui| {
            form_row(ui, tr(lang, "Provider"), |ui| {
                ui.radio_value(&mut f.image_provider_oauth, false, tr(lang, "API (OpenAI-compatible)"));
                ui.radio_value(&mut f.image_provider_oauth, true, tr(lang, "Sign in with ChatGPT (via the local Codex bridge)"));
                if f.image_provider_oauth {
                    explainer(
                        ui,
                        lang,
                        tr(lang, "Uses your ChatGPT subscription through the Codex bridge (CLIProxyAPI) running on this computer — no OpenAI key. To set it up: download CLIProxyAPI from its GitHub releases, start it, sign in with your ChatGPT account when it asks, and paste the api-keys value from its configuration file as the bridge password below. Start the bridge before an edit, or the call cannot connect. Generated images are capped at about 1.5 MP by the subscription; for full-resolution edits switch to API with a real key."),
                    );
                }
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
            form_row(ui, if f.image_provider_oauth { tr(lang, "Bridge URL") } else { tr(lang, "Base URL") }, |ui| {
                let w = field_w(ui, 0.0);
                ui.add(field_edit(ui, &mut f.image_base_url, w));
            });
            form_row(ui, if f.image_provider_oauth { tr(lang, "Bridge password") } else { tr(lang, "API Key") }, |ui| {
                let hint = if f.image_key_present {
                    tr(lang, "set — blank keeps it")
                } else if f.image_provider_oauth {
                    tr(lang, "the password set in the bridge's own config (api-keys) — it stays on this computer and is not an OpenAI key")
                } else {
                    tr(lang, "no key set")
                };
                let w = field_w(ui, 0.0);
                if ui.add(field_edit(ui, &mut f.image_api_key, w).password(true).hint_text(hint)).changed() {
                    // Model availability is CREDENTIAL-dependent, not just
                    // URL-dependent: a different key at the same endpoint can
                    // serve a different catalogue, and the URL-keyed
                    // self-invalidation above can't see that. Typing a new
                    // key drops the fetched lists (the pickers fall back to
                    // grounded defaults until the next fetch).
                    f.image_models.clear();
                }
            });
            form_row(ui, "", |ui| {
                ui.horizontal(|ui| {
                    let label = if f.image_models.fetching { tr(lang, "fetching…") } else { tr(lang, "🔄 Fetch models") };
                    let clicked = action(ui, !f.image_models.fetching, label)
                        .on_hover_text(tr(
                            lang,
                            "List the models this endpoint serves (GET /models) so you can pick instead of guess — and a live reachability check for the bridge/API. Uses the key/token typed above; a saved key is only used at the endpoint it was saved for.",
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
            });
            form_row(ui, tr(lang, "Vision model"), |ui| {
                let opts = model_opts(&f.image_models.chat, &["gpt-5.5", "gpt-4o"], &f.image_model);
                let w = field_w(ui, 0.0);
                ui.horizontal(|ui| model_picker(ui, "set_vision_model", &mut f.image_model, &opts, lang, w));
                // The one thing the listing can say that a wrong pick would
                // otherwise only say at the first paid call.
                if f.image_models.vision_of(&f.image_model) == Some(false) {
                    form_warn(ui, tr(lang, "⚠ takes no images, per /models"));
                }
            });
            form_row(ui, tr(lang, "Reasoning effort"), |ui| {
                let tiers = f.image_models.tiers_for(&f.image_model, &EFFORT_TIERS_API);
                let w = field_w(ui, 0.0);
                ui.horizontal(|ui| effort_picker(ui, "set_image_effort", &mut f.image_effort, &tiers, lang, w));
                if f.image_models.efforts.contains_key(f.image_model.trim()) {
                    tiers_note(ui, tiers.is_empty(), lang);
                }
            });
            form_row(ui, tr(lang, "API style"), |ui| {
                let w = field_w(ui, 0.0);
                api_family_picker(ui, &mut f.image_api_family, lang, w);
            });
            form_row(ui, tr(lang, "Image-gen model"), |ui| {
                // OAuth (subscription) exposes gpt-image-2 first; API keys often
                // still prefer gpt-image-1.5 for its input_fidelity lock.
                let fallbacks: &[&str] = if f.image_provider_oauth {
                    &["gpt-image-2", "gpt-image-1.5"]
                } else {
                    &["gpt-image-1.5", "gpt-image-2", "gpt-image-1", "gpt-image-1-mini", "chatgpt-image-latest"]
                };
                let opts = model_opts(&f.image_models.image_gen, fallbacks, &f.image_gen_model);
                let w = field_w(ui, 0.0);
                ui.horizontal(|ui| model_picker(ui, "set_imagegen_model", &mut f.image_gen_model, &opts, lang, w));
                if !f.image_provider_oauth {
                    form_help(ui, tr(lang, "Tip: gpt-image-1.5 keeps the photo most faithful (input_fidelity); newer models like gpt-image-2 ignore that lock and edit more freely."));
                }
            });
            // The subscription bridge serves OpenAI's call only; a template
            // stays saved and applies again under the API preset.
            if !f.image_provider_oauth {
                image_gen_api_ui(ui, f, lang);
            }
        });
        if let Some(role) = fetch {
            self.fetch_models(role);
        }
    }

    /// The Python the sidecars run under, and the segmentation helper's path.
    fn settings_advanced(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        // One resolved config for both groups of this page — `Config::load`
        // reads the settings file, so once per frame and only here.
        let cfg_now = autoshade::config::Config::load();
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
        settings_card(ui, tr(lang, "Python interpreter"), |ui| {
            form_row(ui, tr(lang, "Path"), |ui| {
                let f = &mut self.settings;
                let detect = tr(lang, "Detect");
                ui.horizontal(|ui| {
                    let w = field_w(ui, trailing_w(ui, detect));
                    ui.add(field_edit(ui, &mut f.python_bin, w).hint_text(autoshade::config::default_python_bin()))
                        .on_hover_text(tr(
                            lang,
                            "Which Python runs the AI sidecars (segmentation, denoise, style). Blank = the platform default. It can only be set here or by the AUTOSHADE_PYTHON environment variable — never by a file that arrives beside your photos.",
                        ));
                    if action(ui, true, detect)
                        .on_hover_text(tr(lang, "Look in the standard install locations for a working Python 3"))
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
                // What the app will ACTUALLY run. The field above holds the FILE's
                // spelling and an `AUTOSHADE_PYTHON` in the environment outranks it,
                // so a user whose environment already answers would otherwise be
                // looking at an empty box.
                path_label(ui, &cfg_now.python_bin, None);
            });
        });
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
        settings_card(ui, tr(lang, "Segmentation helper"), |ui| {
            form_row(ui, tr(lang, "Path"), |ui| {
                let seg_path = std::path::PathBuf::from(&cfg_now.segment_script);
                // In a cell a row tall: the path is this row's only control,
                // so it stands level with the row's label like a field would.
                let w = field_w(ui, 0.0);
                path_cell(
                    ui,
                    w,
                    &abs_display(&seg_path),
                    Some(tr(
                        lang,
                        "Set with the AUTOSHADE_SEGMENT_SCRIPT environment variable (not editable here, because the app runs this file)",
                    )),
                );
                // `segment_helper_available` is the SAME predicate the two AI-select
                // buttons gate on (one `exists()` per process), so this row and those
                // buttons can never disagree about whether the helper is there.
                if !segment_helper_available() {
                    form_help(
                        ui,
                        tr(lang,
                            "This feature needs AutoShade's Python helper, which this download does not include. Install the full package, or (advanced) set AUTOSHADE_SEGMENT_SCRIPT to the path of segment.py",
                        ),
                    );
                }
            });
        });
    }

    /// The name, the version, and the icon's fine print.
    fn settings_about(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        // Bound first: `scripts/audit_i18n.py` reads a literal inside a label
        // call as untranslated UI text, and the version is not text.
        let version = env!("CARGO_PKG_VERSION");
        settings_card(ui, "AutoShade", |ui| {
            form_row(ui, tr(lang, "Version"), |ui| {
                ui.label(version);
                // The icon's fine print (round-13 easter egg).
                form_help(
                    ui,
                    tr(
                        lang,
                        "skymanbp's AS — the “As” stands for AutoShade, not an Adobe subscription. Rent paid to date: $0.00.",
                    ),
                );
            });
        });
    }
}
