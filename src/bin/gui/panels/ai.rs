//! AI panel: every AI *develop* verb and its output in ONE first-level area.
//!
//! R22 #4. Before this the AI surface was scattered across three panels: the
//! verdict + Direction + Analyze/Refine + Style lived at the top of Develop,
//! whole-image Reimagine and reverse-fit sat in the middle of Retouch (between
//! the brush tools), and reverse-fit's own 「Zoned fit (sky)」 switch was in
//! Settings, two panels away from the button it changes. One AI area —
//! analysis / reference libraries / whole-image generation / generated-image
//! adjustment / reverse-fit — is the whole idea; the panel sits at the TOP of the side
//! panel because it is the headline feature, above the sliders it writes into.
//!
//! What deliberately did NOT move: the PIXEL-level AI verbs (AI select
//! subject / sky in Local Masks, AI Denoise now in Detail, AI heal in Retouch,
//! export-time AI Denoise). Those belong beside the thing they act on; they
//! carry a 🤖 prefix and a cross-reference tooltip (`util::ai_xref`) instead,
//! and the index line at the foot of this panel says where each one is.
//!
//! R23 landed three more controls HERE (locked plan, docs/ROADMAP.md
//! 「第二十二至二十四轮计划」), all analysis-side and so all inside
//! `ai_analysis`, beside the Direction they steer: the style-library entry with
//! its reference-image switch (R23-2), the grade STRENGTH slider (R23-3, beside
//! Style — they are one pair of taste axes, and the reported defect was that
//! only the style half existed), and the 「Deep thinking」 switch (R23-4,
//! feedback #13), which sits under both dials because it reads them: the visual
//! judge's round budget comes off the Strength band directly above it.
//!
//! R30 cut the panel along the line the USER PAYS ON. Each sub-area is its
//! own `CollapsingHeader` whose title carries a COST TAG (`· paid API` /
//! `· local`). The reference library — local disk, CPU and one model download,
//! never an API call — is its own sub-area ([`AutoShadeApp::ai_libraries`]).
//! A library is read only while Style > 0 (`pipeline.rs`'s
//! `(req.style > 0.0).then(library::load)`), so the controls that only feed an
//! analysis go dark at Style 0 while the build side stays live: building a
//! library is not reading one, and greying the build at Style 0 would make the
//! library nobody has yet the one nobody can make.
use crate::*;

impl AutoShadeApp {
    /// Does the AI area carry state worth a ● on its collapsed header?
    ///
    /// Written next to the panel it describes, and enumerating that panel's OWN
    /// field set in reading order: the verdict `ai_analysis` prints and the
    /// Direction it consumes.
    /// R22 #16 added the taste dials on the ground that a non-default dial is
    /// state. On 2026-09-20 they were removed because they are persisted
    /// preferences, so the dot never went out.
    ///
    /// Deliberately NOT in the set: `style_strength`, `grade_strength`, and
    /// `fit_strength` are remembered dial preferences, not this photo's state;
    /// `reimagine_prompt`, `fit_ai_judge`, `zoned_fit`, and (R23-2)
    /// `send_style_ref_image` / `style_src_dir` are persisted preferences of
    /// paid verbs or library bookkeeping. The dot means "this PHOTO's AI
    /// inputs carry state", and a remembered dial or folder says nothing
    /// about this photo.
    pub(crate) fn ai_section_active(&self) -> bool {
        self.verdict.is_some() || !self.guidance.is_empty()
    }

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
    pub(crate) fn ai_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang; // Copy — never borrows self, safe inside egui closures.
        // The header's ● (`section_title`): a verdict or a Direction is this
        // photo's AI state. The section itself always opens (`default_open`).
        let ai_active = self.ai_section_active();
        let editable = !self.open_in_flight;
        // The gate wraps the WHOLE area, header included — exactly how
        // develop_panel wraps its sections, so the two read the same mid-open
        // (greyed) instead of one panel looking live beside a dead one.
        ui.add_enabled_ui(editable, |ui| {
            // A panel title like Develop's and Retouch's (`panel_heading`),
            // only collapsible. `CollapsingHeader` indents its body, which set
            // every AI row 18 px right of the other two panels' rows and cut
            // its cells by 9 px (2026-09-27): the body is laid UNINDENTED, and
            // the folds inside keep their own indent, so their rows start
            // where a Develop fold's do. Same persistent id as the header it
            // replaces, so an open state a user already has carries over.
            // 2026-10-01: the title used to be drawn at fold size, so the
            // five folds under it read as its siblings; it is a Heading now,
            // and the folds sit under two group heads, as Develop's do.
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                ui.make_persistent_id("sec_ai"),
                true,
            );
            let header = panel_heading(ui, tr(lang, "AI"), ai_active, |ui| {
                state.show_toggle_button(ui, paint_chevron);
            })
            .on_hover_cursor(egui::CursorIcon::PointingHand);
            if header.clicked() {
                state.toggle(ui);
            }
            state.show_body_unindented(ui, |ui| {
                #[cfg(test)]
                {
                    // The gate's own witness: a comment cannot keep it here.
                    self.ai_gate_enabled = Some(ui.is_enabled());
                }
                group_caption(ui, tr(lang, "Analysis & References"));
                self.ai_analysis(ui);
                self.ai_libraries(ui);
                group_caption(ui, tr(lang, "Generate & Reverse-fit"));
                self.ai_generate(ui);
                self.ai_adjust(ui);
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
            state.store(ui.ctx());
        });
        ui.add_space(SPACE_MD); // fence to the Develop heading below
    }

    /// Sub-area ① continued — the deep-thinking WORKING as a block of its own.
    ///
    /// User feedback 2026-09-11: listed inline in the rationale sentence, the
    /// three R23-4 sentences and the R23-1b pixel-tool line ran the fold to a
    /// wall of italic text. Here they are a captioned block under the
    /// sentence, one row per note with the note's SUBJECT as a bold lead and
    /// the model's own sentence selectable beside it — the arg text rides
    /// verbatim, exactly as it did inside the sentence (the rationale
    /// contract: model prose is never reworded on a surface). Its height is
    /// bounded by the analysis box it sits in (2026-10-13), not by a scroll
    /// of its own: a scroll inside that box would steal the wheel.
    ///
    /// Drawn only when the caller's split actually took these notes OUT of
    /// the sentence; on the fallback path they are still in it, and drawing
    /// them too would print the working twice.
    fn thinking_notes(&self, ui: &mut egui::Ui, working: &[&autoshade::rationale::Note]) {
        use autoshade::rationale::keys;
        let lang = self.lang;
        ui.add_space(SPACE_SM);
        ui.label(egui::RichText::new(tr(lang, "Deep thinking · its working")).small().strong());
        for n in working {
            // The lead is OURS (one catalogue literal per key, so the i18n
            // audit sees each); the body is the note's single arg, the
            // model's sentence.
            let lead = match n.key {
                keys::THINK_SCENE => tr(lang, "What it saw:"),
                keys::THINK_LOOK => tr(lang, "The look it aimed for:"),
                keys::THINK_CRITIQUE => tr(lang, "Its own critique against your strength target:"),
                keys::PIXEL_TOOLS => tr(lang, "Pixel tools it suggests (nothing was run):"),
                _ => continue,
            };
            let Some((_, body)) = n.args.first() else { continue };
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(lead).strong());
                ui.add(egui::Label::new(body.as_str()).selectable(true));
            });
        }
    }

    /// Whether the last analysis left anything to read.
    fn has_analysis_output(&self) -> bool {
        self.verdict.is_some() || !self.rationale.is_empty()
    }

    /// What one Analyze run wrote — the verdict, the rationale sentence and
    /// the deep-thinking working — drawn by both the Analysis fold's bounded
    /// box and the full-text window, so the two can never disagree.
    fn analysis_output(&self, ui: &mut egui::Ui) {
        let lang = self.lang;
        if let Some((d, reasons)) = &self.verdict {
            // Accept reads calm; anything else (Revise/Reject)
            // gets the warn colour so it can't be skimmed past.
            // Matched on the TYPED decision, never its rendered
            // spelling — the old `starts_with("Accept")` sniff
            // would have flipped every verdict to warn the moment
            // the word was translated.
            let col = if matches!(d, autoshade::advisor::Decision::Accept) {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().warn_fg_color
            };
            let text = trf(
                lang,
                "{decision} — {reasons}",
                &[
                    ("decision", tr(lang, autoshade::advisor::decision_key(d))),
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
        //
        // The typed tail splits in two at draw time (user feedback
        // 2026-09-11): the deterministic notes stay in the sentence, the
        // deep-thinking WORKING — the notes only a thinking analysis
        // produces, see `is_working_note` — draws in a box of its own
        // under it. The persisted string is untouched (one suffix, five
        // surfaces); the split is a display decision, and on the fallback
        // path the working is still inside `shown`, so no box is drawn.
        let (working, tail): (Vec<_>, Vec<_>) =
            self.rationale_notes.iter().partition(|n| is_working_note(n.key));
        let localized = (!self.rationale_notes.is_empty())
            .then(|| {
                let det: String = self
                    .rationale_notes
                    .iter()
                    .map(autoshade::rationale::render_one)
                    .collect();
                self.rationale.strip_suffix(det.as_str()).map(|prose| {
                    let mut s = String::from(prose);
                    for n in &tail {
                        s.push_str(&trf(lang, n.key, &note_args(n)));
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
        if localized.is_some() && !working.is_empty() {
            self.thinking_notes(ui, &working);
        }
    }

    /// The analysis output's full text in a window of its own: movable,
    /// resizable, scrolling, its text selectable. Not a transient sheet — it
    /// is a reading pane, so the editor's keys stay live beside it. Closes
    /// itself once there is nothing to show (a new photo, a cleared result).
    pub(crate) fn analysis_full_window(&mut self, ctx: &egui::Context) {
        if !self.show_analysis_full {
            return;
        }
        let mut open = self.has_analysis_output();
        egui::Window::new(tr(self.lang, "AI analysis · full text"))
            // Fixed id: the title varies with the language, and egui keys
            // the window's position and size off the id.
            .id(egui::Id::new("analysis_full_window"))
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(520.0, 420.0))
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.analysis_output(ui));
            });
        self.show_analysis_full = open;
    }

    /// Sub-area ① — ANALYSIS (paid API): the verdict and rationale one Analyze
    /// run wrote, the Direction prompt it reads, its two verbs, and the two
    /// taste dials that steer them. Body migrated verbatim from
    /// `develop::dev_ai` (R22 #4); R30 moved the LIBRARY half out of it into
    /// [`AutoShadeApp::ai_libraries`], so this fold is now exactly the controls
    /// that spend money — which is what its 「· paid API」 tag claims while it
    /// is collapsed.
    ///
    /// Default OPEN: it is the panel's first read.
    fn ai_analysis(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        card_fold(ui, egui::CollapsingHeader::new(tr(lang, "Analysis · paid API"))
            .id_salt("sec_ai_analysis")
            .default_open(true)
            // Headless layout tests must see these rows — a test frame never
            // clicks a header, and the Direction prompt carries the #14a width
            // rule. Only `cfg(test)` forces it; a real run keeps the user's
            // own fold state.
            .open(fold_open_in_tests())
            , |ui| {
            // The analysis output lives in ONE bounded box (2026-10-13, user:
            // 「思考过程输出有时候太长了会占满屏幕」): the verdict, the rationale
            // sentence and the deep-thinking working scroll inside
            // ANALYSIS_BOX_MAX_H, and once they overflow it a verb opens the
            // whole text in a window of its own (`analysis_full_window`).
            if self.has_analysis_output() {
                let out = egui::Frame::group(ui.style())
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("ai_analysis_box")
                            .max_height(ANALYSIS_BOX_MAX_H)
                            .auto_shrink([false, true])
                            .show(ui, |ui| self.analysis_output(ui))
                    })
                    .inner;
                #[cfg(test)]
                {
                    self.analysis_box_rect = Some(out.inner_rect);
                }
                if out.content_size.y > out.inner_rect.height() + 0.5
                    && action_row(ui, true, tr(lang, "View full text…")).clicked()
                {
                    self.show_analysis_full = true;
                }
            }
            // Library OR direction (user ruling 2026-10-14): the switch heads
            // the controls it chooses between, and greys the other side.
            use autoshade::pipeline::Guide;
            ui.horizontal_wrapped(|ui| {
                ui.label(tr(lang, "Guided by"));
                ui.radio_value(&mut self.guide, Guide::Library, tr(lang, "Reference library")).on_hover_text(tr(
                    lang,
                    "The AI follows the finished photos in your reference library closest to this one; the Direction text is not sent.",
                ));
                ui.radio_value(&mut self.guide, Guide::Direction, tr(lang, "Direction")).on_hover_text(tr(
                    lang,
                    "The AI follows the Direction you type; the reference library is not read.",
                ));
            });
            let by_direction = self.guide == Guide::Direction;
            ui.add_enabled_ui(by_direction, |ui| {
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
            });
            // The prompt's triggers sit DIRECTLY under it (user feedback:
            // the toolbar Analyze button sat nowhere near the text it
            // consumes). TWO explicit verbs replace the old pre-armed
            // 「Refine」 checkbox — a mode you had to remember to tick
            // (and untick) before clicking is exactly the kind of hidden
            // state a button-per-intent design removes.
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
                let cell = columns(ui, 2);
                if why(primary_in(ui, cell, ready, tr(lang, "AI Analyze"))
                    .on_hover_text(tr(lang,
                        "AI proposes a recipe from scratch (GPT proposal + validation + a visual \
                         review: the result is RENDERED and judged by the vision model, which may \
                         buy one guided revision), written into the sliders — undoable. Uses the \
                         reference library or the Direction above (the switch picks one); Strength \
                         steers it. COST, worst case: 11 API \
                         calls, 6 of them carrying images (8 high-detail frames). Ticking 「Deep \
                         thinking」 below OR pushing Strength above 70% raises that ceiling — \
                         either one alone does it; the Deep thinking tooltip has the numbers.",
                    )))
                    .clicked()
                {
                    self.start_analyze(false);
                }
                // Refining a neutral edit IS analyzing — disable the verb
                // until there is an edit to refine.
                let has_edit = ready && !self.recipe.is_noop();
                if why(action_in(ui, cell, has_edit, tr(lang, "AI Refine"))
                    .on_hover_text(tr(lang,
                        "Adjust the CURRENT edit instead of proposing from scratch — your sliders are \
                         the starting point (enabled once the edit is non-neutral).",
                    )))
                    .clicked()
                {
                    self.start_analyze(true);
                }
            });
            // The two TASTE DIALS get their own lines, below the verbs (R23-3).
            //
            // They used to share the verbs' row, with a note claiming the wrap kept
            // Style on-panel at narrow widths. It did not: `egui::Slider` lays its
            // value box, track and LABEL out in a nested `ui.horizontal`, and a
            // nested row never wraps in a wrapping parent — it overflows and is
            // clipped. At the default 320 px side panel the row drew only Style's
            // value ("30"), with the word "Style" itself off-panel, and a headless
            // frame confirms it. Adding a second dial to that row hid it outright.
            // One dial per line is also what every other slider in this app does.
            ui.add_enabled_ui(!by_direction, |ui| ui.horizontal_wrapped(|ui| {
                #[cfg(test)]
                {
                    self.guide_gate_enabled = Some((by_direction, ui.is_enabled()));
                }
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
                    tr(lang, "How far AI proposals follow the looks in your reference library: below 85% they stay within its grading, from 85% they reproduce its look (0 = ignore it). Greyed while the Direction guides."),
                );
                // R23-2: a slider that provably cannot do anything must not read as
                // live. The old one showed 30% and a tooltip about "your past XMP
                // editing habits" on a fresh install with no library at all — a
                // control that was permanently inert with nothing saying so. Only
                // when the status is KNOWN and negative (never while it is still
                // being read, which would flash a false warning every launch).
                if matches!(
                    self.style_info,
                    Some(autoshade::library::Summary::Absent | autoshade::library::Summary::Unusable(_))
                ) {
                    ui.label(
                        egui::RichText::new(tr(lang, "⚠ no library"))
                            .color(ui.visuals().warn_fg_color)
                            .small(),
                    )
                    .on_hover_text(tr(lang,
                        "This slider does nothing until a reference library is built — the 「Reference library」 section below builds one.",
                    ));
                }
            }));
            // R23-3 (feedback #5): the SECOND taste axis, directly under the first —
            // they are a PAIR, and the whole reported problem was that only the style
            // half existed, so "lean on my habits" was the only dial there was and it
            // bought MORE restraint. Same helper as Style, so it gets the same 0..100
            // track, double-click reset and ↑/↓ nudge.
            Self::slider_pct_hinted(
                ui,
                lang,
                tr(lang, "Strength"),
                &mut self.grade_strength,
                1.0,
                GRADE_STRENGTH_DEFAULT,
                tr(lang,
                    "How hard the AI pushes the grade — a different axis from Style: Style asks how close to your reference looks, Strength asks how committed the result should be. 50% is where every AI guardrail NUMBER was calibrated: the ±50/±35 pair and the soft caps are bit-for-bit the ones earlier releases used, but the restraint WORDING those releases sent is now the 40%-and-below prose, so no single setting brings an old release back whole. From 41% up the AI must decide EACH colour control explicitly instead of leaving it neutral by default; the default 65% (double-click to reset) leans a little further than the calibration point. Above 70% it is additionally told to use the controls it wants at a strength a viewer can see, and the visual review may then run up to 3 rounds — the same ceiling 「Deep thinking」 raises it to, and either one ALONE is enough to make the worst case 17 API calls (10 carrying images). The clipping and white-point safeguards never widen with it.",
                ),
            );
            let has_direction = by_direction && !self.guidance.trim().is_empty();
            ui.add_enabled_ui(has_direction, |ui| {
                #[cfg(test)]
                {
                    // The gate's own witness (same seam as `ai_gate_enabled`): a
                    // comment cannot keep the wrapper here.
                    self.adherence_gate_enabled = Some(ui.is_enabled());
                }
                Self::slider_pct_hinted(
                    ui,
                    lang,
                    tr(lang, "Adherence"),
                    &mut self.direction_adherence,
                    1.0,
                    autoshade::recipe::DirectionAdherence::DEFAULT,
                    tr(lang, "How closely the AI follows your direction; disabled until the Direction guides and has text: <=40% Hint, 40-70% Direct, above 70% Brief. Prompt intent only - it never moves a render limit."),
                );
            });
            // R23-4 (feedback #13): the THIRD analysis-side control, under the two
            // dials it modifies — the target score and round budget it unlocks are
            // read off the Strength band directly above it.
            ui.checkbox(&mut self.deep_think, tr(lang, "Deep thinking"))
                .on_hover_text(tr(lang,
                    "Make the AI show its work and let it iterate. The proposal must first name what it sees, decide EACH tool family (tone / white balance / presence / HSL / colour grading / curves / detail / framing / masks) with a reason, state the look it is going for, and end by critiquing its own answer — those three sentences land in the 「Deep thinking」 box under the rationale. It also asks the image model for one step more reasoning effort (only when a tier other than 「provider default」 is set in Settings), and lets the visual judge keep going until it scores well enough: 2 rounds at a balanced Strength, 3 above 70%. COST: a normal analyze is at worst 11 API calls (6 with images, 8 high-detail frames); with this box ticked OR Strength above 70% — either one alone is enough — it is at worst 17 calls (10 with images, 14 high-detail), plus roughly 10-20% more output tokens per proposal. Batch and the eval harness never do this.",
                ));
            });
    }

    /// Sub-area ② — the REFERENCE LIBRARY (local): finished photos whose look
    /// an analysis may follow — which library, how to build it, and the two
    /// switches an analysis reads.
    ///
    /// COLLAPSED by default because it is a setup-time surface: built once,
    /// then left alone. The status line is half the feature (R23-2: "I have no
    /// idea which library it is referencing"), so it heads the fold.
    fn ai_libraries(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        // Read the status ONCE per session (cached in `style_info`). OUTSIDE
        // the fold on purpose: the Style slider's own 「⚠ no library」 flag one
        // sub-area up reads this same answer, so a collapsed fold must not
        // starve it. Not in tests: a headless frame would spawn a worker
        // reading the developer's own store; tests set `style_info` directly.
        if !cfg!(test) && self.style_info.is_none() && !self.style_info_loading {
            self.start_style_info();
        }
        ui.add_space(SPACE_XS);
        // Read off the SAME comparison the pipeline makes
        // (`GradeRequest::library_style`), so the two can never disagree
        // about what "reads a library" means.
        let by_direction = self.guide == autoshade::pipeline::Guide::Direction;
        let reads_a_library = !by_direction && self.style_strength > 0.0;
        card_fold(ui, egui::CollapsingHeader::new(tr(lang, "Reference library · local"))
            .id_salt("sec_ai_libraries")
            .default_open(false)
            // Headless layout tests must see these rows: a test frame never
            // clicks a header, and an unopened fold makes an assertion about
            // them silently vacuous instead of red.
            .open(fold_open_in_tests())
            , |ui| {
                self.ai_library_status(ui);
                self.ai_library_build(ui);
                if !reads_a_library {
                    // Outside the disabled scope below: the sentence that
                    // explains a greyed control must not itself be greyed.
                    let why = if by_direction {
                        tr(lang, "Guided by the Direction: the analysis reads no library")
                    } else {
                        tr(lang, "Style is at 0%: the analysis reads no library")
                    };
                    ui.label(egui::RichText::new(why).weak().small());
                }
                ui.add_enabled_ui(reads_a_library, |ui| {
                    #[cfg(test)]
                    {
                        self.ai_library_read_enabled = Some(ui.is_enabled());
                    }
                    self.ai_library_use(ui);
                });
            });
    }

    /// Which library, how big, how old, and where — or why there is none.
    fn ai_library_status(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let line = match &self.style_info {
            Some(autoshade::library::Summary::Built { photos, phrased, dir, age }) => {
                let mut t = trf(
                    lang,
                    "{n} finished photos · from {path} · {m} with look phrases",
                    &[("n", &photos.to_string()), ("path", dir), ("m", &phrased.to_string())],
                );
                if let Some(age) = age {
                    // Coarse buckets, no calendar arithmetic: "how stale is my
                    // library" is answered by hours or days, and both spellings
                    // avoid a plural rule.
                    const HOUR: u64 = 3600;
                    let secs = age.as_secs();
                    t.push_str(" · ");
                    t.push_str(&if secs < 48 * HOUR {
                        trf(lang, "built {hours}h ago", &[("hours", &(secs / HOUR).to_string())])
                    } else {
                        trf(lang, "built {days}d ago", &[("days", &(secs / (24 * HOUR)).to_string())])
                    });
                }
                egui::RichText::new(t).small()
            }
            Some(autoshade::library::Summary::Unusable(err)) => egui::RichText::new(trf(
                lang,
                "The reference library could not be read ({err}) — rebuild it below.",
                &[("err", err)],
            ))
            .color(ui.visuals().warn_fg_color)
            .small(),
            Some(autoshade::library::Summary::Absent) => egui::RichText::new(tr(lang,
                "No reference library yet — the Style slider above has nothing to follow. Pick a folder of finished photos whose look you like (JPEG, TIFF or PNG exports).",
            ))
            .small(),
            None => egui::RichText::new(tr(lang, "reading the reference library…")).weak().small(),
        };
        ui.label(line).on_hover_text(trf(
            lang,
            "Library file: {path}",
            &[("path", &abs_display(&autoshade::library::path()))],
        ));
    }

    /// The folder picker, the build button, the phrase switch and progress —
    /// the BUILD side, live at every Style value; only a running build greys it.
    fn ai_library_build(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let building = self.style_build_inflight;
        let mut pick = false;
        let mut build: Option<PathBuf> = None;
        ui.add_enabled_ui(!building, |ui| {
            #[cfg(test)]
            {
                self.ai_library_build_enabled = Some(ui.is_enabled());
            }
            // 🗂, the same glyph Settings uses for a folder action: the
            // embedded font subset covers it already (📂 is not in it).
            let cell = columns(ui, 2);
            if action_in(ui, cell, true, tr(lang, "🗂 Pick folder…"))
                .on_hover_text(tr(lang,
                    "Choose a folder of finished photos whose look you like — your own exports or anyone's. Nothing reads their .xmp: each photo is measured from its pixels. Building starts as soon as you choose.",
                ))
                .clicked()
            {
                pick = true;
            }
            // Rebuilding needs a folder: the remembered one, or one picked now.
            let have = self.style_src_dir.clone().filter(|d| d.is_dir());
            let label = if building { tr(lang, "building…") } else { tr(lang, "🔄 Build / rebuild") };
            let resp = primary_in(ui, cell, have.is_some(), label).on_hover_text(if have.is_some() {
                tr(lang,
                    "Measure every finished photo in that folder (local compute, no API cost). A rebuild measures only new or changed photos. It cannot be cancelled, and a failed build leaves your existing library untouched.",
                )
            } else {
                tr(lang, "Pick a folder first")
            });
            if resp.clicked() {
                build = have;
            }
            ui.checkbox(&mut self.style_describe, tr(lang, "Also write each look as short phrases (local vision model; downloads 4.3 GB once, slower builds)"))
                .on_hover_text(tr(lang,
                    "A local model (Qwen3-VL-2B) writes a few phrases per photo about its GRADE — \"warm, faded, low contrast\" — which go to the AI beside each reference. Nothing leaves this machine and nothing is billed. Off = the references go without words.",
                ));
        });
        if let Some((stage, done, total)) = self.style_build_progress {
            ui.label(
                egui::RichText::new(trf(
                    lang,
                    "{stage}: {done} / {total} photos",
                    &[("stage", tr(lang, stage.label())), ("done", &done.to_string()), ("total", &total.to_string())],
                ))
                .weak()
                .small(),
            );
        }
        // rfd after the row is laid out: the dialog is modal and blocks.
        if pick {
            let mut dialog = rfd::FileDialog::new();
            // Reopen where the last build ran, when that folder still exists.
            if let Some(d) = self.style_src_dir.clone().filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(d);
            }
            if let Some(dir) = dialog.pick_folder() {
                // Remembered on the PICK: a build that then fails must not
                // lose the folder the rebuild button needs.
                self.style_src_dir = Some(dir.clone());
                build = Some(dir);
            }
        }
        if let Some(dir) = build {
            self.start_style_build(dir);
        }
    }

    /// The READ side: whether an analysis searches the library, and whether
    /// the references also go to the model as pictures.
    fn ai_library_use(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let built = matches!(self.style_info, Some(autoshade::library::Summary::Built { .. }));
        ui.checkbox(&mut self.style_embed, tr(lang, "Search the library when analysing (SigLIP 2, local; downloads 1.5 GB once)"))
            .on_hover_text(tr(lang,
                "Each analysis measures this photo with a local model and picks the closest references. Building a library turns this on, since the build has already downloaded the model.",
            ));
        if built && !self.style_embed {
            ui.label(
                egui::RichText::new(tr(lang, "built, but not searched: the switch above is off"))
                    .color(ui.visuals().warn_fg_color)
                    .small(),
            );
        }
        ui.add_enabled_ui(built && self.style_embed, |ui| {
            ui.checkbox(&mut self.send_style_ref_image, tr(lang, "Also show the model the reference photos"))
                .on_hover_text(if built {
                    tr(lang,
                        "WILL UPLOAD UP TO 3 MORE IMAGES per analysis call: the closest reference photos, so the model can match their look by eye as well as by words. COST: each is billed as one more image — and a revision round sends them again. They are never stored by the provider (store:false), and the rationale says how many went. Off = the references go as words only.",
                    )
                } else {
                    tr(lang, "Build a reference library first — there is no reference photo to send")
                });
        });
    }

    /// Sub-area ③ — WHOLE-IMAGE GENERATION (paid API): let gpt-image DIRECTLY produce the
    /// picture (the optional "GPT makes the image" path). Distinct from AI
    /// Analyze, which emits a faithful parametric recipe. The result becomes a
    /// new「AI 生成」variant in the strip below; the reverse-fit sub-area under
    /// this one then closes the loop, adding a「反推」variant whose look lives
    /// in an editable recipe (full-res + XMP). No more "continue from master"
    /// button — each result is its own selectable variant, so a slider edit can
    /// never revert or double-cook it.
    ///
    /// Collapsed by default: this is the OPTIONAL path (and the paid one), so
    /// the analysis sub-area at the top stays the panel's first read. R30 put
    /// the cost in the header text as well — a collapsed fold that says
    /// 「· paid API」 is the only warning a user gets before opening it.
    fn ai_generate(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_XS);
        card_fold(ui, egui::CollapsingHeader::new(tr(lang, "Reimagine (whole image) · paid API"))
            .id_salt("sec_ai_reimagine")
            .default_open(false)
            // Headless layout tests must see this row: it carries the R19
            // width arithmetic, and a test frame never clicks a header. Only
            // `cfg(test)` forces it; a real run keeps the user's fold state.
            .open(fold_open_in_tests())
            , |ui| {
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
                        let resp = ui.add(primary_button(btn_label).wrap_mode(egui::TextWrapMode::Extend));
                        #[cfg(test)]
                        {
                            self.reimagine_btn_rect = Some(resp.rect);
                        }
                        if resp
                            .on_hover_text(tr(lang,
                                "Repaint the whole image with gpt-image, styled by the prompt on the left \
                                 (empty = a neutral finished develop). Repainted pixels = not faithful; the \
                                 result is added as an 「AI generated」 variant at the bottom and switched to; \
                                 it stays as generated, and your tweaks continue on an ✎ card beside it. Models that accept any size \
                                 (gpt-image-2) reach ~8MP, others ~1.5K. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
                            ))
                            .clicked()
                        {
                            self.start_reimagine();
                        }
                    });
                });
                // Its OWN row, deliberately: a fourth item in the wrapped
                // prompt+button row re-opens the R19 width arithmetic.
                ui.checkbox(
                    &mut self.reimagine_retry,
                    tr(lang, "auto-retry once if the result diverges"),
                )
                .on_hover_text(trf(
                    lang,
                    "After generating, the structural divergence D vs the original is measured. \
                     If D ≥ {limit} (the reverse-fit's atmosphere threshold), buy ONE more \
                     generation — a second paid image — and keep the closer result. \
                     Off = never spend extra.",
                    &[("limit", &format!("{:.2}", autoshade::fit::DIVERGENCE_GLOBAL))],
                ));
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

    /// Adjust owns every input, its painted area included (`BrushOwner::Adjust`).
    /// Generated and Edited both read AI pixels; the negative never does.
    pub(crate) fn can_adjust(&self) -> bool {
        !self.busy && self.active_on_ai_pixels()
            && (self.area_painted(BrushOwner::Adjust) || !self.adjust_prompt.trim().is_empty())
    }

    fn ai_adjust(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_XS);
        card_fold(ui, egui::CollapsingHeader::new(tr(lang, "Adjust generated image · paid API"))
            .id_salt("sec_ai_adjust")
            .default_open(false)
            .open(fold_open_in_tests())
            , |ui| {
                let _field = prompt_field(
                    ui,
                    &mut self.adjust_prompt,
                    tr(lang, "what to change — e.g. make the sky bluer, remove the wires on the left; blank with a painted area = remove"),
                );
                #[cfg(test)]
                {
                    self.prompt_rects.push(_field.rect);
                }
                egui::ComboBox::from_id_salt("adjust_quality")
                    .selected_text(tr(lang, ["high", "medium", "low"][self.adjust_quality.min(2)]))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.adjust_quality, 0, tr(lang, "high"));
                        ui.selectable_value(&mut self.adjust_quality, 1, tr(lang, "medium"));
                        ui.selectable_value(&mut self.adjust_quality, 2, tr(lang, "low"));
                    })
                    .response
                    .on_hover_text(tr(lang, "gpt-image render quality — higher looks better and costs more per image"));
                // This fold's own brush and painted area (2026-09-27): it used
                // to read the Retouch panel's shared mask with no control here.
                self.brush_row(ui, BrushOwner::Adjust);
                let note = if !self.active_on_ai_pixels() {
                    tr(lang, "select a ✨ AI generated card (or its ✎ edit) first")
                } else if self.area_painted(BrushOwner::Adjust) {
                    tr(lang, "painted area only")
                } else {
                    tr(lang, "whole image (paint an area to limit it)")
                };
                ui.label(egui::RichText::new(note).weak().small());
                let resp = primary_row(ui, self.can_adjust(), tr(lang, "✨ Adjust"))
                    .on_hover_text(tr(lang,
                        "Change the generated picture with the prompt: the whole image, or only the painted area when this fold's brush has strokes (blank = remove there). One gpt-image call per adjust — costs per image. The result is a new ✨ AI generated card; this card stays as it is; crop / straighten are not carried (set them on the new card). Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
                    ));
                #[cfg(test)]
                {
                    self.adjust_btn_enabled = Some(resp.enabled());
                }
                if resp.clicked() {
                    self.start_adjust();
                }
            });
    }

    /// Sub-area ⑤ — REVERSE-FIT (local; the AI review is paid): turn the
    /// freshly generated look back into an
    /// editable recipe — how the low-res experiment becomes a full-res, XMP-able
    ///「反推」variant. A PEER of the generation sub-area, not a row inside it:
    /// its own switches (the optional AI review, and zoned fit — which used to
    /// live in Settings, two panels from the button it changes) are settings for
    /// this verb, and burying them one fold deeper than the verb was the
    /// scattering this panel exists to end.
    ///
    /// R30 gave it a header of its own (it was a `group_caption` under a
    /// separator) and left it OPEN by default: the fit and the extract are
    /// local and free, and the one paid switch inside — 「AI review」 — is
    /// what the header's 「AI 打分付费」 half names.
    fn ai_reverse_fit(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.add_space(SPACE_XS);
        card_fold(ui, egui::CollapsingHeader::new(tr(lang, "Reverse-fit · local; AI review is paid"))
            .id_salt("sec_ai_reverse_fit")
            .default_open(true)
            // Same reason as the folds above: a headless frame never clicks a
            // header, and this body carries the row whose width runaway
            // `the_generate_button_stays_one_line_and_the_panel_stays_put`
            // exists to catch.
            .open(fold_open_in_tests())
            , |ui| {
            let can_fit = self.fit_target().is_some() && self.source_preview.is_some();
            if !can_fit {
                // An ✎ card is not a fit target (`fit_target`): the fit reads
                // pixels, and the card's edits are sliders the user already
                // has — the line says which card to select instead of
                // pretending nothing was generated.
                let on_edited =
                    self.active_variant().is_some_and(|v| v.kind == VariantKind::Edited);
                let note = if on_edited {
                    tr(lang,
                        "Reverse-fit reads the ✨ AI generated card's pixels — select that card; your ✎ edits are sliders over the same pixels and stay where they are.")
                } else {
                    tr(lang,
                        "Pick a reference below, or generate an image and stay on that variant, to reverse-fit a recipe.")
                };
                ui.label(egui::RichText::new(note).weak().small());
            }
            // ── the SOURCE row (2026-09-30, user decision): which negative the
            // fit solves FROM — the ▣ card or any ◈ / ▦ master — named and
            // choosable. Until then the fold showed the target only, and the
            // source was a rule nobody could see (`negative_index`).
            let card = |i: usize| format!("{} #{}", self.variant_label(i), i + 1);
            let sources: Vec<(String, String)> = (0..self.variants.len())
                .filter(|&i| Self::is_fit_source(&self.variants[i]))
                .map(|i| (self.variants[i].id.clone(), card(i)))
                .collect();
            let to = (self.fit_ref.is_none() && self.fit_target().is_some())
                .then(|| trf(lang, "To: {card}", &[("card", &card(self.active))]));
            if !sources.is_empty() {
                let auto = self.negative_index().map(card).unwrap_or_default();
                let auto = trf(lang, "Automatic · {card}", &[("card", &auto)]);
                let shown = match self.fit_from.as_deref() {
                    Some(id) => sources.iter().find(|(s, _)| s == id).map(|(_, l)| l.clone()),
                    None => None,
                }
                .unwrap_or_else(|| auto.clone());
                let mut pick: Option<Option<String>> = None;
                let busy = self.busy;
                ui.horizontal(|ui| {
                    // The row's width is measured at its start, before the
                    // label, and capped at FIELD_W_MAX (`columns`), so the menu
                    // ends on the same right edge as the buttons under it;
                    // `available_width()` alone stretched it across a wide
                    // panel (2026-10-01 user report).
                    let row = columns(ui, 1).x;
                    let label = ui.label(tr(lang, "From:")).rect.width();
                    let menu = (row - label - ui.spacing().item_spacing.x).max(FIELD_W_MIN);
                    let _menu = ui.add_enabled_ui(!busy, |ui| {
                        egui::ComboBox::from_id_salt("fit_from")
                            .width(menu)
                            .truncate()
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.fit_from.is_none(), &auto).clicked() {
                                    pick = Some(None);
                                }
                                for (id, label) in &sources {
                                    let on = self.fit_from.as_deref() == Some(id.as_str());
                                    if ui.selectable_label(on, label).clicked() {
                                        pick = Some(Some(id.clone()));
                                    }
                                }
                            })
                            .response
                            .on_hover_text(tr(lang,
                                "The negative the reverse-fit starts FROM: the ▣ original, or a ◈ denoised / ▦ stacked master. Automatic = the ◈ / ▦ card you stand on, else the first one in the strip, else ▣.",
                            ))
                    });
                    #[cfg(test)]
                    {
                        self.fit_from_rect = Some(_menu.inner.rect);
                    }
                });
                if let Some(p) = pick {
                    self.fit_from = p;
                }
            }
            // ── the TARGET, named when it is the active card (a picked
            // reference file names itself in the row below).
            if let Some(to) = to {
                ui.label(egui::RichText::new(to).weak().small());
            }
            // ── the REFERENCE row (R23-6 B): any finished rendition of this same
            // frame — your own Lightroom export, the camera's JPEG, another RAW
            // developed elsewhere. The generated-variant entry below still works
            // untouched; this one simply stops it from being the only one.
            let mut pick_ref = false;
            let mut clear_ref = false;
            ui.horizontal_wrapped(|ui| {
                // One cell of the grid, measured at the row's start; the file
                // name and its ✕ follow, wrapping under when short (2026-09-27).
                let cell = columns(ui, 2);
                if action_in(ui, cell, !self.busy, tr(lang, "Choose reference…"))
                    .on_hover_text(tr(lang,
                        "Reverse-fit toward ANY finished version of THIS SAME photo — your own \
                         Lightroom/Capture One export, the camera's JPEG, a TIFF, or another RAW \
                         (developed neutrally first). The fit solves the develop parameters that \
                         reproduce that file's look and leaves your pixels untouched. It must be \
                         the same frame: a different picture is warned about, not refused, and \
                         its result means nothing.",
                    ))
                    .clicked()
                {
                    pick_ref = true;
                }
                if let Some(p) = self.fit_ref.clone() {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string());
                    // Truncated, never wrapped: a file name is one unbreakable
                    // word, and egui's inline text layout in a wrapped row ran
                    // it past the panel edge (11 px at the 320 px default) and
                    // widened the auto-fitting panel; the full path is on
                    // hover. Truncation keeps it one atomic widget that wraps
                    // under the button when the line is short.
                    ui.add(egui::Label::new(egui::RichText::new(name).weak().small()).truncate())
                        .on_hover_text(p.display().to_string());
                    // Bare glyph, not `tr` — the symbol IS the label in every
                    // language (the same rule the variant-strip and gallery ✕
                    // buttons follow); only the tooltip is translated.
                    if glyph(ui, true, "✕")
                        .on_hover_text(tr(lang,
                            "Forget this reference and go back to reverse-fitting the active generated variant",
                        ))
                        .clicked()
                    {
                        clear_ref = true;
                    }
                }
            });
            // rfd OUTSIDE the closure above (it borrows `ui`) and after the row is
            // laid out: the dialog is modal and blocks this thread.
            if pick_ref && let Some(p) = util::photo_file_dialog() {
                self.fit_ref = Some(p);
            }
            if clear_ref {
                self.fit_ref = None;
            }
            // The fit's OWN Strength, directly above the verb that reads it (user
            // decision 2026-09-12: a control belongs to the section whose function
            // it serves). Through v1.3.1 the fit silently read the Analysis dial two
            // folds up — the last place anyone looking at this row would look. Same
            // helper as the Analysis dials: 0..100 track, double-click reset, hover
            // nudge, the explanation as the tooltip.
            Self::slider_pct_hinted(
                ui,
                lang,
                tr(lang, "Reverse-fit strength"),
                &mut self.fit_strength,
                1.0,
                autoshade::recipe::GradeStrength::DEFAULT,
                tr(lang,
                    "The reverse-fit's honesty budget — its own dial: the Analysis Strength above does not reach the fit. At or below the default 65% the fit is byte-identical to the calibrated path. Above it the Atmosphere budget widens, and a white balance outside the budget shrinks along its fitted move instead of staying as-shot. From 85% unsupported movement is DISCLOSED (confidence capped) instead of withheld. Double-click to reset.",
                ),
            );
            ui.horizontal_wrapped(|ui| {
                let cell = columns(ui, 2);
                let can = !self.busy && can_fit;
                if primary_in(ui, cell, can, tr(lang, "Reverse-fit recipe"))
                    .on_hover_text(tr(lang,
                        "Statistical fit: reverse the freshly generated look into editable develop params \
                         (local, no API cost). Sliders update (undoable), and for RAW a Lightroom XMP goes \
                         into this photo's develop store; hit Export to render the full-resolution result. \
                         Uses the Reverse-fit strength dial above as its honesty budget.",
                    ))
                    .clicked()
                {
                    self.start_fit();
                }
                if action_in(ui, cell, can, tr(lang, "Extract style"))
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
            // disables it. R38: on a plain row of its own with 「deep」, never
            // in the wrapped verb row above — its two cells take the whole
            // line, and a checkbox scoped after them was laid past the line's
            // end (a scope never wraps), which widened the auto-fitting side
            // panel by 47 px every frame
            // (`the_generate_button_stays_one_line_and_the_panel_stays_put`
            // is the witness — that test exists because this panel's width
            // has run away once before).
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.busy, |ui| {
                    ui.checkbox(&mut self.fit_ai_judge, tr(lang, "AI review"))
                        .on_hover_text(tr(lang,
                            "After the fit, show the target and the fitted render to the vision model and \
                             have it SCORE the match (0-100) with a short critique — LLM as a judge. One \
                             paid vision call per fit (needs the image API key); the fit itself stays \
                             local and free. The score, its critique AND its suggestion land in the \
                             status line below — nothing is changed for you. No cancel: like the fit \
                             itself, the app stays busy until the review returns.",
                        ));
                });
                // R23-6 D: the review can now also ACT, when asked. Beside
                //「AI review」 and gated on it, because a deep fit IS that review
                // plus a loop — a checkbox that silently switched the other one
                // on would be two settings pretending to be one.
                ui.add_enabled_ui(!self.busy && self.fit_ai_judge, |ui| {
                    ui.checkbox(&mut self.fit_deep, tr(lang, "deep"))
                        .on_hover_text(if self.fit_ai_judge {
                            tr(lang,
                                "DEEP REVERSE-FIT: run the review BEFORE saving and let it buy one \
                                 guided retry — the reviewer's suggestion picks the next ACTION \
                                 (add the zoned pass, pull the chroma chase back), never the \
                                 numbers, and the retry is kept only if it re-scores at least as \
                                 high. COST: up to two paid vision calls instead of one, and the \
                                 save waits for them; there is NO cancel, exactly as for the \
                                 review itself. Off = the reviewed fit is saved first and the \
                                 score is a note (the behaviour of every release since v0.26.0).",
                            )
                        } else {
                            tr(lang, "Turn on 「AI review」 first — the deep fit is that review, iterated")
                        });
                });
            });
            // Migrated from Settings (#4): a switch that only ever changes what
            // the button above does. Same rule as 「AI review」 — a persisted
            // preference, so no can_fit gate (and, as in Settings, no busy gate:
            // `start_fit` reads the flag when it runs).
            ui.checkbox(&mut self.zoned_fit, tr(lang, "Zoned fit (sky)")).on_hover_text(tr(
                lang,
                "On reverse-fit, fit globally first. Sky segmentation and native luminance-range fallback stay exclusive; then frozen-evidence spatial tiles are tried automatically on a 4x4 grid with a four-tile cap and zero frame regression. Conservative guided refinement may keep or abstain before fitting semantic/tile masks, and never changes luminance ranges. The sky and land zones ride out as Lightroom's own Select Sky mask (Lightroom rebuilds its own sky alpha; the raster shown here is ours). Residual-earned bands add intersecting gradients. Tiles also export as intersecting gradients when that replacement passes the shared gates and fits the photo no worse than the refined raster; retained refined tiles and free-form masks keep a named bitmap loss. Native ranges are written to the Lightroom sidecar. Segmentation needs the python dependencies (transformers + torch), and every fallback or abstention is noted in the rationale.",
            ));
            ui.checkbox(&mut self.zoned_four_regions, tr(lang, "Up to four semantic regions"))
                .on_hover_text(tr(lang, "Opt in to semantic regions beyond the historical sky/land pass; this costs one OneFormer pass per frame and may take longer."));
            });
    }
}

/// Bound on the analysis box's height before it scrolls: about eight body
/// lines — the verdict, a few lines of rationale and the first of the
/// working. The rationale's deterministic tail grows with every note a run
/// raises, so the box used to take whatever the text asked for and an
/// over-long analysis filled the screen (2026-10-13); the rest is one
/// 「View full text…」 away.
pub(crate) const ANALYSIS_BOX_MAX_H: f32 = 160.0;

/// The notes only a DEEP-THINKING analysis produces (`pipeline.rs`, R23-4's
/// three sentences + R23-1b's pixel-tool line): the model's working, as
/// opposed to the deterministic tail every analysis carries. ONE list, so the
/// box and the sentence agree on who takes which note.
fn is_working_note(key: &str) -> bool {
    use autoshade::rationale::keys as k;
    [k::THINK_SCENE, k::THINK_LOOK, k::THINK_CRITIQUE, k::PIXEL_TOOLS].contains(&key)
}

/// A note's args as the `(key, value)` slice `trf` takes.
fn note_args(n: &autoshade::rationale::Note) -> Vec<(&str, &str)> {
    n.args.iter().map(|(k, v)| (*k, v.as_str())).collect()
}
