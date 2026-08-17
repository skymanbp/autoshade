//! AI panel: every AI *develop* verb and its output in ONE first-level area.
//!
//! R22 #4. Before this the AI surface was scattered across three panels: the
//! verdict + Direction + Analyze/Refine + Style lived at the top of Develop,
//! whole-image Reimagine and reverse-fit sat in the middle of Retouch (between
//! the brush tools), and reverse-fit's own 「Zoned fit (sky)」 switch was in
//! Settings, two panels away from the button it changes. One AI area, three
//! sub-areas — analysis / whole-image generation / reverse-fit — is the whole
//! idea; the panel sits at the TOP of the side panel because it is the headline
//! feature, above the sliders it writes into.
//!
//! What deliberately did NOT move: the PIXEL-level AI verbs (AI select
//! subject / sky in Local Masks, AI Denoise now in Detail, AI heal in Retouch,
//! export-time AI Denoise). Those belong beside the thing they act on; they
//! carry a 🤖 prefix and a cross-reference tooltip (`util::ai_xref`) instead,
//! and the index line at the foot of this panel says where each one is.
//!
//! R23 lands three more controls HERE (locked plan, docs/ROADMAP.md
//! 「第二十二至二十四轮计划」): the grade STRENGTH slider (default 0.65), the
//! thinking-depth control, and the style-library entry with its reference-image
//! switch. They are analysis-side, so they belong in `ai_analysis` beside
//! Direction / Style.
//!
//! R23-2 landed the third of those: [`AutoshopApp::ai_style_library`] — the
//! desktop app's FIRST production-side entry for the style reference library
//! (building existed only on the CLI and in the web panel, so the Style slider
//! shipped permanently inert for anyone who double-clicks the exe), plus the
//! transparency half of the same feedback item: which library, how big, how
//! old, and which shots the last analysis actually leaned on. It sits INSIDE
//! `ai_analysis`, directly under the slider it explains, rather than in a
//! fold or in Settings — the status IS half the feature.

use crate::*;

impl AutoshopApp {
    /// The AI area — one first-level section, three sub-areas.
    ///
    /// The editable gate is REBUILT here (L15-2). While a decode is in flight
    /// (`open_in_flight`) these controls would read and write the STASHED photo
    /// A while B lands and replaces the whole recipe — silent input loss. The
    /// analysis half used to inherit `develop_panel`'s `add_enabled_ui`
    /// wrapper; moving it out of that closure without restoring the same gate
    /// here would have re-opened exactly that hole. `busy` alone must NOT gate:
    /// a 600 s analyze keeps the panel live (each verb has its own `ready`
    /// gate) — only the open transition freezes it.
    /// Does the AI area carry state worth a ● on its collapsed header?
    ///
    /// Written next to the panel it describes, and enumerating that panel's OWN
    /// field set in reading order: the verdict `ai_analysis` prints, the
    /// Direction it consumes, and the Style strength that steers both verbs.
    /// Style was the drift this predicate exists to close (R22 #16) — it is the
    /// one AI input with a NON-ZERO default, so the honest test is "moved off
    /// [`STYLE_STRENGTH_DEFAULT`]", and comparing against the same constant the
    /// slider resets to means the dot and the reset can never disagree.
    ///
    /// Deliberately NOT in the set: `reimagine_prompt`, `fit_ai_judge`,
    /// `zoned_fit`, and (R23-2) `send_style_ref_image` / `style_src_dir`. Those
    /// are persisted PREFERENCES of paid verbs, or library bookkeeping — the
    /// same rule as `fit_ai_judge`, whose default-off state must not light the
    /// dot either. The dot means "this PHOTO's AI inputs carry state", and a
    /// remembered folder says nothing about this photo.
    pub(crate) fn ai_section_active(&self) -> bool {
        self.verdict.is_some()
            || !self.guidance.is_empty()
            || self.style_strength != STYLE_STRENGTH_DEFAULT
    }

    pub(crate) fn ai_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang; // Copy — never borrows self, safe inside egui closures.
        // Open whenever there's a verdict to show; the inputs are always present.
        let ai_active = self.ai_section_active();
        let editable = !self.open_in_flight;
        // The gate wraps the WHOLE area, header included — exactly how
        // develop_panel wraps its sections, so the two read the same mid-open
        // (greyed) instead of one panel looking live beside a dead one.
        ui.add_enabled_ui(editable, |ui| {
            egui::CollapsingHeader::new(section_title(tr(lang, "AI"), ai_active))
                .id_salt("sec_ai")
                .default_open(true)
                .show(ui, |ui| {
                    #[cfg(test)]
                    {
                        // The gate's own witness: a comment cannot keep it here.
                        self.ai_gate_enabled = Some(ui.is_enabled());
                    }
                    self.ai_analysis(ui);
                    self.ai_generate(ui);
                    self.ai_reverse_fit(ui);
                    // Where the AI verbs that did NOT move to this panel live.
                    // A panel called "AI" reads as the complete inventory
                    // otherwise, and the pixel-level tools are deliberately
                    // elsewhere.
                    ui.add_space(SPACE_XS);
                    ui.label(
                        egui::RichText::new(tr(lang,
                            "Pixel-level AI tools stay at their tools: select subject / select sky in Local Masks, denoise in Detail, heal and fill in Retouch.",
                        ))
                        .weak()
                        .small(),
                    );
                });
        });
        ui.add_space(SPACE_MD); // fence to the Develop heading below
    }

    /// Sub-area 1 — ANALYSIS: the verdict and rationale one Analyze run wrote,
    /// the Direction prompt it reads, its two verbs, and the personal-style
    /// strength. Body migrated verbatim from `develop::dev_ai` (R22 #4).
    fn ai_analysis(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        group_caption(ui, tr(lang, "Analysis"));
        if let Some((d, reasons)) = &self.verdict {
            // Accept reads calm; anything else (Revise/Reject)
            // gets the warn colour so it can't be skimmed past.
            // Matched on the TYPED decision, never its rendered
            // spelling — the old `starts_with("Accept")` sniff
            // would have flipped every verdict to warn the moment
            // the word was translated.
            let col = if matches!(d, autoshop::advisor::Decision::Accept) {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().warn_fg_color
            };
            let text = trf(
                lang,
                "{decision} — {reasons}",
                &[
                    ("decision", tr(lang, autoshop::advisor::decision_key(d))),
                    ("reasons", &reasons.join("; ")),
                ],
            );
            ui.label(egui::RichText::new(text).color(col));
        }
        // The deterministic tail renders LOCALIZED when its typed
        // notes ride along and still match the string's suffix
        // (L12#2B); any mismatch — a truncation, a disk-restored
        // develop with no notes — shows the raw English instead
        // (silent-English fallback, user decision 2026-08-11).
        let localized = (!self.rationale_notes.is_empty())
            .then(|| {
                let det: String = self
                    .rationale_notes
                    .iter()
                    .map(autoshop::rationale::render_one)
                    .collect();
                self.rationale.strip_suffix(det.as_str()).map(|prose| {
                    let mut s = String::from(prose);
                    for n in &self.rationale_notes {
                        let args: Vec<(&str, &str)> =
                            n.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                        s.push_str(&trf(lang, n.key, &args));
                    }
                    s
                })
            })
            .flatten();
        let shown = localized.as_deref().unwrap_or(&self.rationale);
        if !shown.is_empty() {
            ui.label(
                egui::RichText::new(format!("“{shown}”"))
                    .italics()
                    .weak(),
            );
        }
        ui.label(tr(lang, "Direction"))
            .on_hover_text(tr(lang, "Free-text direction for AI Analyze — e.g. warmer and moodier"));
        // #14a: was `desired_width(f32::INFINITY)` — a full-panel ribbon on a
        // wide side panel. `prompt_field` caps it, widens while focused and
        // shows the whole text on hover.
        let _field = prompt_field(
            ui,
            &mut self.guidance,
            tr(lang, "e.g. warmer and moodier, lift the shadows"),
        );
        #[cfg(test)]
        {
            self.prompt_rects.push(_field.rect);
        }
        // The prompt's triggers sit DIRECTLY under it (user feedback:
        // the toolbar Analyze button sat nowhere near the text it
        // consumes). TWO explicit verbs replace the old pre-armed
        // 「Refine」 checkbox — a mode you had to remember to tick
        // (and untick) before clicking is exactly the kind of hidden
        // state a button-per-intent design removes. Wrapped so the
        // Style slider never strands off-panel at narrow widths.
        ui.horizontal_wrapped(|ui| {
            // `analyze_inflight` too, or ✕ leaves these ENABLED while
            // `start_analyze` silently refuses (it must refuse — the
            // cancelled call is still on the wire and still billing).
            // A button that looks live and does nothing, for up to the
            // 600 s stall budget, with the status line saying the app
            // is free, is worse than one that says why it is greyed.
            let ready = self.src_path.is_some() && !self.busy && !self.analyze_inflight;
            let waiting = self.analyze_inflight && !self.busy;
            let why = |ui: egui::Response| {
                if waiting {
                    ui.on_hover_text(tr(
                        lang,
                        "the cancelled AI call is still running (and still billed) — this re-arms when it finishes or times out",
                    ))
                } else {
                    ui
                }
            };
            if why(ui
                .add_enabled(ready, egui::Button::new(tr(lang, "AI Analyze")))
                .on_hover_text(tr(lang,
                    "AI proposes a recipe from scratch (GPT proposal + validation + a visual \
                     review: the result is RENDERED and judged by the vision model, which may \
                     buy one guided revision — extra vision cost per run), written into the \
                     sliders — undoable. Uses the Direction above; Style steers it.",
                )))
                .clicked()
            {
                self.start_analyze(false);
            }
            // Refining a neutral edit IS analyzing — disable the verb
            // until there is an edit to refine.
            let has_edit = ready && !self.recipe.is_noop();
            if why(ui
                .add_enabled(has_edit, egui::Button::new(tr(lang, "AI Refine")))
                .on_hover_text(tr(lang,
                    "Adjust the CURRENT edit instead of proposing from scratch — your sliders are \
                     the starting point (enabled once the edit is non-neutral).",
                )))
                .clicked()
            {
                self.start_analyze(true);
            }
            ui.separator();
            // #16: the ONE slider in the app that never went through the
            // panel's own helper — a bare `egui::Slider` with `show_value(false)`
            // and a hand-rolled "30%" label beside it, so it alone had no
            // double-click / right-click reset and no hover ↑/↓ nudge, and its
            // 0..1 storage was shown on a scale nothing else in the UI uses.
            // `slider_pct_hinted` puts it on the same 0..100 track as every
            // other stored fraction (Amount, feathers, tolerance), resets to the
            // shared default, and keeps the explanation as the tooltip's first
            // line instead of on a separate label.
            Self::slider_pct_hinted(
                ui,
                lang,
                tr(lang, "Style"),
                &mut self.style_strength,
                1.0,
                STYLE_STRENGTH_DEFAULT,
                tr(lang, "Personal style strength: how far AI proposals lean toward your past XMP editing habits (0 = ignore)"),
            );
            // R23-2: a slider that provably cannot do anything must not read as
            // live. The old one showed 30% and a tooltip about "your past XMP
            // editing habits" on a fresh install with no library at all — a
            // control that was permanently inert with nothing saying so. Only
            // when the status is KNOWN and negative (never while it is still
            // being read, which would flash a false warning every launch).
            if matches!(
                self.style_info.as_ref().map(|i| &i.state),
                Some(
                    autoshop::style::StyleIndexState::Absent
                        | autoshop::style::StyleIndexState::Unusable { .. }
                )
            ) {
                ui.label(
                    egui::RichText::new(tr(lang, "⚠ no library"))
                        .color(ui.visuals().warn_fg_color)
                        .small(),
                )
                .on_hover_text(tr(lang,
                    "This slider does nothing until a style reference library is built — the section just below builds one.",
                ));
            }
        });
        self.ai_style_library(ui);
    }

    /// Sub-area 1b — the STYLE REFERENCE LIBRARY: what the Style slider above
    /// actually leans on, and the only place in the desktop app that can build
    /// it (R23-2, feedback #6).
    ///
    /// Two defects in one section. The library had no production-side entry
    /// point outside the CLI and the web panel, so the slider above was
    /// permanently inert for anyone who double-clicks the exe; and even with a
    /// library built, nothing ever said WHICH one — the user's own words were
    /// "I have no idea which library it is referencing". So the status line is
    /// not decoration here, it is half the feature, and it sits directly under
    /// the slider it explains rather than behind a fold.
    fn ai_style_library(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        // Read the status ONCE per session (cached in `style_info`) — the file
        // can reach 32 MB, and this draws every frame. Not in tests: a headless
        // frame would spawn a worker reading the developer's own store, and the
        // tests drive `style_info` directly instead.
        if !cfg!(test) && self.style_info.is_none() && !self.style_info_loading {
            self.start_style_info();
        }
        ui.add_space(SPACE_XS);
        group_caption(ui, tr(lang, "Style reference library"));
        // ── the status line: which library, how big, how old, and where.
        match self.style_info.as_ref().map(|i| (i.path.clone(), i.state.clone())) {
            Some((path, autoshop::style::StyleIndexState::Built { total, source_dir, age, .. })) => {
                let from = source_dir.unwrap_or_else(|| tr(lang, "an unrecorded folder").to_string());
                ui.label(
                    egui::RichText::new(trf(
                        lang,
                        "{n} of your own edits · from {path}",
                        &[("n", &total.to_string()), ("path", &from)],
                    ))
                    .small(),
                )
                .on_hover_text(trf(
                    lang,
                    "Library file: {path}",
                    &[("path", &abs_display(&path))],
                ));
                if let Some(age) = age {
                    // Coarse buckets, no calendar arithmetic (this tree carries
                    // no date library): "how stale is my library" is answered by
                    // hours or days, and both spellings avoid a plural rule.
                    const HOUR: u64 = 3600;
                    let secs = age.as_secs();
                    let line = if secs < 48 * HOUR {
                        trf(lang, "built {hours}h ago", &[("hours", &(secs / HOUR).to_string())])
                    } else {
                        trf(lang, "built {days}d ago", &[("days", &(secs / (24 * HOUR)).to_string())])
                    };
                    ui.label(egui::RichText::new(line).weak().small());
                }
            }
            Some((_, autoshop::style::StyleIndexState::Unusable { err })) => {
                ui.label(
                    egui::RichText::new(trf(
                        lang,
                        "The style library could not be read ({err}) — rebuild it below.",
                        &[("err", &err)],
                    ))
                    .color(ui.visuals().warn_fg_color)
                    .small(),
                );
            }
            Some((_, autoshop::style::StyleIndexState::Absent)) => {
                // The shared refusal's own wording (the CLI's `StyleIndex::save`
                // and the web handler say the same): an Autoshop OUTPUT folder
                // always yields nothing, because Autoshop's own .xmp lives in
                // the develop store — that is the mistake this sentence exists
                // to pre-empt, and it must not be reworded per surface.
                ui.label(
                    egui::RichText::new(tr(lang,
                        "No library built yet — the Style slider above has nothing to lean on. Point this at the folder you edit in Lightroom (each RAW with its .xmp sidecar beside it); Autoshop keeps its own .xmp in the develop store, never beside your RAWs, so its output folder always yields nothing.",
                    ))
                    .small(),
                );
            }
            None => {
                ui.label(
                    egui::RichText::new(tr(lang, "reading the style library…")).weak().small(),
                );
            }
        }
        // ── the folder picker + the build button.
        ui.horizontal_wrapped(|ui| {
            let building = self.style_build_inflight;
            let mut pick = false;
            let mut build: Option<PathBuf> = None;
            ui.add_enabled_ui(!building, |ui| {
                if ui
                    // 🗂, the same glyph Settings uses for a folder action: the
                    // embedded font subset covers it already (📂 is not in it).
                    .button(tr(lang, "🗂 Pick folder…"))
                    .on_hover_text(tr(lang,
                        "Choose the folder of your OWN edited RAWs — the ones with a Lightroom .xmp sidecar beside them. Each pair teaches Autoshop one of your finished looks. Indexing starts as soon as you choose.",
                    ))
                    .clicked()
                {
                    pick = true;
                }
                // Rebuilding needs a folder: the remembered one, or one picked
                // now. Enabled only when there IS one, so the button can never
                // launch a build against nothing.
                let have = self.style_src_dir.clone().filter(|d| d.is_dir());
                let label = if building {
                    tr(lang, "building…")
                } else {
                    tr(lang, "🔄 Build / rebuild")
                };
                let resp = ui
                    .add_enabled(have.is_some(), egui::Button::new(label))
                    .on_hover_text(if have.is_some() {
                        tr(lang,
                            "Index every RAW+.xmp pair in that folder (local compute, no API cost). Every RAW is decoded, so a large library takes minutes; the app stays usable and this button re-arms when it finishes. It cannot be cancelled — a build that indexes nothing is refused and leaves your existing library untouched.",
                        )
                    } else {
                        tr(lang, "Pick a folder first")
                    });
                if resp.clicked() {
                    build = have;
                }
            });
            if let Some((done, total)) = self.style_build_progress {
                ui.label(
                    egui::RichText::new(trf(
                        lang,
                        "{done} / {total} photos",
                        &[("done", &done.to_string()), ("total", &total.to_string())],
                    ))
                    .weak()
                    .small(),
                );
            }
            // rfd OUTSIDE the closures above (they borrow `ui`), and after the
            // row is laid out: the dialog is modal and blocks this thread.
            if pick {
                let mut dialog = rfd::FileDialog::new();
                // Reopen where the last build ran, when that folder still
                // exists — a dialog pointed at a missing path lands wherever
                // the OS decides (Explorer drops you in Documents).
                if let Some(d) = self.style_src_dir.clone().filter(|d| d.is_dir()) {
                    dialog = dialog.set_directory(d);
                }
                if let Some(dir) = dialog.pick_folder() {
                    // Remembered on the PICK, not only on a successful build:
                    // the rebuild button needs a folder before it can do
                    // anything, and a build that then fails must not lose it.
                    self.style_src_dir = Some(dir.clone());
                    self.start_style_build(dir);
                }
            }
            if let Some(dir) = build {
                self.start_style_build(dir);
            }
        });
        // ── the opt-in reference PHOTO (the "both" half of feedback #6: the
        // index AND an actual picture).
        let built = matches!(
            self.style_info.as_ref().map(|i| &i.state),
            Some(autoshop::style::StyleIndexState::Built { .. })
        );
        ui.add_enabled_ui(built, |ui| {
            ui.checkbox(
                &mut self.send_style_ref_image,
                tr(lang, "Also give the model a reference photo"),
            )
            .on_hover_text(if built {
                tr(lang,
                    "WILL UPLOAD TWO IMAGES per analysis call: this photo, plus the ONE most similar shot from your style library, so the model can match your look by eye instead of only by numbers. COST: an analysis with two images is billed for two images instead of one — and a revision round sends both again. The reference is never stored by the provider (store:false), and the rationale names the photo that was used. Off = the numeric style reference only.",
                )
            } else {
                tr(lang, "Build a style library first — there is no reference photo to send")
            });
        });
    }

    /// Sub-area 2 — WHOLE-IMAGE GENERATION: let gpt-image DIRECTLY produce the
    /// picture (the optional "GPT makes the image" path). Distinct from AI
    /// Analyze, which emits a faithful parametric recipe. The result becomes a
    /// new「AI 生成」variant in the strip below; the reverse-fit sub-area under
    /// this one then closes the loop, adding a「反推」variant whose look lives
    /// in an editable recipe (full-res + XMP). No more "continue from master"
    /// button — each result is its own selectable variant, so a slider edit can
    /// never revert or double-cook it.
    ///
    /// Collapsed by default: this is the OPTIONAL path (and the paid one), so
    /// the analysis sub-area above it stays the panel's first read.
    fn ai_generate(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_XS);
        egui::CollapsingHeader::new(tr(lang, "Reimagine (whole image)"))
            .id_salt("sec_ai_reimagine")
            .default_open(false)
            // Headless layout tests must see this row: it carries the R19
            // width arithmetic, and a test frame never clicks a header. Only
            // `cfg(test)` forces it; a real run keeps the user's fold state.
            .open(fold_open_in_tests())
            .show(ui, |ui| {
                // This entry's OWN style prompt, right next to its trigger
                // (it used to silently borrow the Direction field at the top
                // of the panel — a prompt and its button belong together).
                ui.horizontal_wrapped(|ui| {
                    // The prompt field's width reserve follows the BUTTON'S
                    // localized label, measured — the old fixed 130 px was
                    // one pixel SHORT of the English "✨ Generate image"
                    // (131 px at Button style + padding; the zh label needs
                    // only 103), so the button's text wrapped into two
                    // lines, twice the height of every neighbour (R19 user
                    // report). Extend-mode is the belt to the measured
                    // braces: the button can never wrap internally again —
                    // which is also why the row math must be EXACT, frame
                    // margins included: with Extend, any surplus stops
                    // being absorbed by a wrap and instead widens the
                    // auto-fitting side panel by that surplus EVERY frame
                    // (probed: +8 px/frame runaway when the TextEdit's own
                    // frame margin was left out of the reserve). The margin
                    // is therefore pinned HERE and handed to the widget, so
                    // the arithmetic and the widget can never disagree.
                    const TE_MARGIN: egui::Margin = egui::Margin::symmetric(4.0, 2.0);
                    let btn_label = tr(lang, "✨ Generate image");
                    let btn_w = ui
                        .painter()
                        .layout_no_wrap(
                            btn_label.to_owned(),
                            egui::TextStyle::Button.resolve(ui.style()),
                            ui.visuals().text_color(),
                        )
                        .size()
                        .x
                        + 2.0 * ui.spacing().button_padding.x;
                    // …and #14a caps the RESULT at FIELD_W_MAX, which only stops
                    // an 800 px panel from drawing an 800 px prompt ribbon —
                    // clamping DOWNWARD cannot feed the runaway above.
                    //
                    // The FLOOR is the honest half: FIELD_W_MIN does ask for
                    // more than a very narrow row has, the shape the runaway
                    // came from. It cannot self-feed the way the runaway did,
                    // because the demand is a CONSTANT 80 px rather than one
                    // defined in terms of `available_width()` — the panel widens
                    // once to fit it and then the surplus is gone. Behaviour is
                    // unchanged from R19 either way: FIELD_W_MIN IS that row's
                    // own `.max(80.0)`, now a named token (theme.rs). Both are
                    // consts with MIN < MAX, so `clamp` cannot hit its panicking
                    // case.
                    let field_w = (ui.available_width()
                        - btn_w
                        - ui.spacing().item_spacing.x
                        - TE_MARGIN.sum().x)
                        .clamp(FIELD_W_MIN, FIELD_W_MAX);
                    let _field = ui.add(
                        egui::TextEdit::singleline(&mut self.reimagine_prompt)
                            .margin(TE_MARGIN)
                            .desired_width(field_w)
                            .hint_text(tr(lang, "style to repaint toward — e.g. golden-hour glow, moody film look")),
                    );
                    #[cfg(test)]
                    {
                        self.prompt_rects.push(_field.rect);
                    }
                    ui.add_enabled_ui(!self.busy, |ui| {
                        let resp = ui.add(
                            egui::Button::new(btn_label).wrap_mode(egui::TextWrapMode::Extend),
                        );
                        #[cfg(test)]
                        {
                            self.reimagine_btn_rect = Some(resp.rect);
                        }
                        if resp
                            .on_hover_text(tr(lang,
                                "Repaint the whole image with gpt-image, styled by the prompt on the left \
                                 (empty = a neutral finished develop). Repainted pixels = not faithful; the \
                                 result is added as an 「AI generated」 variant at the bottom and switched to, \
                                 so you can keep tweaking without reverting. Models that accept any size \
                                 (gpt-image-2) reach ~8MP, others ~1.5K. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
                            ))
                            .clicked()
                        {
                            self.start_reimagine();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "After generating, use 「Reverse-fit recipe」 to turn the look into sliders + XMP \
                         (the full-resolution way).",
                    ))
                    .weak()
                    .small(),
                );
            });
    }

    /// Sub-area 3 — REVERSE-FIT: turn the freshly generated look back into an
    /// editable recipe — how the low-res experiment becomes a full-res, XMP-able
    ///「反推」variant. A PEER of the generation sub-area, not a row inside it:
    /// its own switches (the optional AI review, and zoned fit — which used to
    /// live in Settings, two panels from the button it changes) are settings for
    /// this verb, and burying them one fold deeper than the verb was the
    /// scattering this panel exists to end.
    fn ai_reverse_fit(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_XS);
        ui.separator();
        group_caption(ui, tr(lang, "Reverse-fit"));
        let can_fit = self.fit_target().is_some() && self.source_preview.is_some();
        if !can_fit {
            ui.label(
                egui::RichText::new(tr(lang,
                    "Generate an image first and stay on that variant to reverse-fit its recipe."))
                    .weak()
                    .small(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.busy && can_fit, |ui| {
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
                         style prompt: auto-fills the Reimagine prompt (ready to restyle other photos) and \
                         saves ./out/<stem>.style.txt.",
                    ))
                    .clicked()
                {
                    self.start_style_prompt();
                }
            });
            // R20 opt-in LLM-as-a-judge. OUTSIDE the can_fit gate: the
            // toggle is a persisted PREFERENCE, not a fit-time verb —
            // gating it on can_fit locked a setting behind having a
            // generated variant active (review R20-N1). Only busy
            // disables it.
            ui.add_enabled_ui(!self.busy, |ui| {
                ui.checkbox(&mut self.fit_ai_judge, tr(lang, "AI review"))
                    .on_hover_text(tr(lang,
                        "After the fit, show the target and the fitted render to the vision model and \
                         have it SCORE the match (0-100) with a short critique — LLM as a judge. One \
                         paid vision call per fit (needs the image API key); the fit itself stays \
                         local and free. The score lands in the status line below. No cancel: like \
                         the fit itself, the app stays busy until the review returns.",
                    ));
            });
        });
        // Migrated from Settings (#4): a switch that only ever changes what
        // the button above does. Same rule as 「AI review」 — a persisted
        // preference, so no can_fit gate (and, as in Settings, no busy gate:
        // `start_fit` reads the flag when it runs).
        ui.checkbox(&mut self.zoned_fit, tr(lang, "Zoned fit (sky)")).on_hover_text(tr(
            lang,
            "On reverse-fit, auto-split the sky on both sides and colour-correct sky↔sky separately (exposure / recolour gains / saturation, bitmap mask). Masks are rendered by the local engine; the LR sidecar carries only the global part. Needs the python segmentation deps (transformers + torch); falls back to pure global reverse-fit when unavailable, noting it in the rationale.",
        ));
    }
}
