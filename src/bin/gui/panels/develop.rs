//! Develop panel: sliders, sections, variant strip.

use crate::*;

impl AutoshopApp {
    /// One labelled slider; double-click resets to `default` (the Lightroom
    /// gesture), hover + ↑/↓ nudges by a domain-appropriate step (Shift ×10 —
    /// LR's arrow grammar; ←/→ stay library navigation). Returns true if the
    /// value changed this frame. Callers pass an already-translated `label`.
    /// Whole-number domains (range ≥ 20: the ±100 family, 0..150, hue °) snap
    /// to integers while dragging, like Lightroom — the web UI already had to
    /// work around raw floats ("13.4849996…" overflowing its value box).
    pub(crate) fn slider(
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

    /// A 0..=1-stored fraction shown on Lightroom's 0..100 track (Amount,
    /// feathers, range tolerance): the panel used to mix "Amount 0.65" with
    /// "Shadows 40" in one column, which read as two unit systems. Storage
    /// stays the fraction (the XMP contract); only the display scales. The
    /// ×100 track crosses the ≥ 20 width rule above, so these snap to whole
    /// numbers exactly like LR's.
    pub(crate) fn slider_pct(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        max: f32,
        default: f32,
    ) -> bool {
        let mut disp = *value * 100.0;
        let changed =
            Self::slider(ui, lang, label, &mut disp, 0.0, max * 100.0, default * 100.0);
        if changed {
            *value = disp / 100.0;
        }
        changed
    }

    /// Sub-unit precision variant (Straighten °): the whole-number snap of the
    /// wide-range class would destroy 0.1° levelling on a ±45 track.
    pub(crate) fn slider_fine(
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
    pub(crate) fn slider_log(
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
    pub(crate) fn slider_impl(
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
        // lose to the display rounding. The Frac NUDGE is floored at that
        // same 0.01 grid: a finer step (range < 1) rounds back onto the
        // current value below and the `next != *value` guard drops it —
        // ArrowDown on the 0–0.5 Feather was a permanent no-op (D13).
        // EV: 0.01-drag / 0.1-arrow. LogK's
        // nudge is ~1% of the current Kelvin (≈55 K at 5500) — a fixed step
        // would be sub-pixel at one end of a log track and huge at the other.
        let (snap, nudge, decimals) = match feel {
            SliderFeel::Int => (1.0, 1.0, 0usize),
            SliderFeel::Fine => (0.1, 0.1, 1),
            SliderFeel::Frac => (0.01, (range / 100.0).max(0.01), 2),
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
            .on_hover_text(tr(lang, "double-click / right-click resets · hover + ↑/↓ nudges (Shift ×10)"));
        // Right-click = reset too (阶段5 手感): the double-click twin — LR
        // muscle memory, and reachable without the precise double timing.
        if (resp.double_clicked() || resp.secondary_clicked()) && *value != default {
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

    pub(crate) fn develop_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang; // Copy — never borrows self, safe inside egui closures.
        let mut changed = false;
        ui.heading(tr(lang, "Develop"));
        self.histogram_ui(ui);
        ui.add_space(SPACE_SM);

        changed |= self.dev_ai(ui);
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
        changed |= self.dev_tone_wb(ui);
        changed |= self.dev_presence(ui, presence_active);
        changed |= self.dev_curves(ui, curves_active);
        changed |= self.dev_hsl(ui, hsl_active);
        changed |= self.dev_grading(ui, grade_active);
        changed |= self.dev_detail(ui, detail_active);
        changed |= self.dev_lens(ui);
        changed |= self.dev_crop(ui);
        changed |= self.dev_masks(ui);
        changed |= self.dev_versions(ui);
        changed |= self.dev_export(ui);

        if changed {
            self.recipe.clamp();
            self.dirty = true;
        }
    }

    /// The variant strip (版本条): one card per rendition — 原片 / AI 生成 /
    /// 反推 — with a live developed thumbnail. Click a card to switch (lossless;
    /// each variant keeps its own base + recipe), × to drop one. This is the
    /// selector that makes an AI develop a first-class, non-reverting version.
    pub(crate) fn variant_strip(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let accent = self.theme.colors().accent_text; // Copy — safe in closures
        let mut switch_to: Option<usize> = None;
        let mut delete: Option<usize> = None;
        ui.horizontal(|ui| {
            ui.add_space(SPACE_SM);
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
                                    // Concentric with the 4.0-radius border
                                    // below: outer radius = inner + expand.
                                    ui.painter().rect_filled(
                                        rect.expand(3.0),
                                        7.0,
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
                                ui.label(if active { label.strong().color(accent) } else { label });
                                // Any variant except the sole Original can be dropped.
                                // ✕ — the ONE close/cancel glyph app-wide (the
                                // old bare × was the fourth variant of it).
                                if self.variants.len() > 1
                                    && kind != VariantKind::Original
                                    && ui.small_button("✕").on_hover_text(tr(lang, "Delete this variant")).clicked()
                                {
                                    delete = Some(i);
                                }
                            });
                        });
                        ui.add_space(SPACE_MD);
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

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_ai(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        // The AI area: everything one Analyze run reads or writes, in ONE
        // place (UX batch — Direction/Refine/Style used to be scattered
        // across two toolbar rows with Undo/Redo in between). Open whenever
        // there's a verdict to show; the inputs are always present.
        let ai_active = self.verdict.is_some() || !self.guidance.is_empty();
        egui::CollapsingHeader::new(section_title(tr(lang, "AI"), ai_active))
            .id_salt("sec_verdict")
            .default_open(true)
            .show(ui, |ui| {
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
                ui.add(
                    egui::TextEdit::singleline(&mut self.guidance)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr(lang, "e.g. warmer and moodier, lift the shadows")),
                );
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
                    let ready =
                        self.src_path.is_some() && !self.busy && !self.analyze_inflight;
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
                            "AI proposes a recipe from scratch (GPT proposal + validation), written into \
                             the sliders — undoable. Uses the Direction above; Style steers it.",
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
        false
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_tone_wb(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let mut changed = false;

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

        ui.add_space(SPACE_MD); // same section fence as every sibling
        egui::CollapsingHeader::new(section_title(tr(lang, "Tone & WB"), tone_active))
            .id_salt("sec_tone")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // A nonzero tint IS a WB edit (recipes saved before the
                    // uncheck-zeroes-tint fix can carry one with no Temp) —
                    // showing "off = as-shot" over an active tint, with its
                    // slider disabled, was a lie with no way out.
                    let mut custom_wb =
                        self.recipe.temperature_k.is_some() || self.recipe.tint != 0.0;
                    if ui.checkbox(&mut custom_wb, tr(lang, "Custom white balance (off = as-shot)")).changed() {
                        // Arm AT the as-shot anchor when stamped: checking
                        // the box must not shift the image — the slider then
                        // starts from the camera's real Kelvin, not 5500.
                        self.recipe.temperature_k =
                            if custom_wb { Some(self.recipe.as_shot_k.unwrap_or(5500.0)) } else { None };
                        if !custom_wb {
                            // "As-shot" is the WHOLE white balance: a custom
                            // Tint surviving the un-check kept a WB edit
                            // active behind a label claiming there was none.
                            self.recipe.tint = 0.0;
                        }
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
                if let (Some(k), Some(t)) = (self.recipe.as_shot_k, self.recipe.as_shot_tint) {
                    // The camera's own WB in absolute terms — the reference
                    // the Temp slider is anchored on for this photo. BOTH
                    // halves required: the era pin (anchor 5500, tint None)
                    // knows the anchor but NOT the camera, and showing it as
                    // "as shot" would be a false claim.
                    ui.weak(trf(
                        lang,
                        "as shot ≈ {k} K · tint {t}",
                        &[("k", &format!("{k:.0}")), ("t", &format!("{t:+.0}"))],
                    ));
                }
                // Double-click reset lands on the as-shot Kelvin when stamped
                // (the honest "no shift" position), 5500 for legacy recipes.
                let anchor_k = self.recipe.as_shot_k.unwrap_or(5500.0);
                if let Some(mut k) = self.recipe.temperature_k
                    && Self::slider_log(ui, lang, tr(lang, "Temp (K)"), &mut k, 2000.0, 40000.0, anchor_k)
                {
                    self.recipe.temperature_k = Some(k);
                    changed = true;
                }
                let custom_wb = self.recipe.temperature_k.is_some() || self.recipe.tint != 0.0;
                let r = &mut self.recipe;
                // Tint is HALF of the white balance: editable only under
                // Custom WB, or a tint move silently contradicted the
                // checkbox's "off = as-shot" promise. A legacy tint-only
                // recipe counts as Custom (see the checkbox above) so its
                // slider stays reachable.
                ui.add_enabled_ui(custom_wb, |ui| {
                    changed |= Self::slider(ui, lang, tr(lang, "Tint"), &mut r.tint, -100.0, 100.0, 0.0);
                });
                changed |= Self::slider(ui, lang, tr(lang, "Exposure"), &mut r.exposure_ev, -5.0, 5.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Contrast"), &mut r.contrast, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Highlights"), &mut r.highlights, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Shadows"), &mut r.shadows, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Whites"), &mut r.whites, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Blacks"), &mut r.blacks, -100.0, 100.0, 0.0);
            });
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_presence(&mut self, ui: &mut egui::Ui, presence_active: bool) -> bool {
        let lang = self.lang;
        let mut changed = false;

        // LR-Basic order (UX batch): Presence sits directly under Tone & WB —
        // the two halves of Lightroom's Basic panel — then Curves, then the
        // two colour sections; Detail moved BELOW them so colour work isn't
        // interrupted by sharpening. add_space fences each header so the ten
        // sections read as groups, not one undivided list.
        ui.add_space(SPACE_MD);
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
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_curves(&mut self, ui: &mut egui::Ui, curves_active: bool) -> bool {
        let lang = self.lang;
        let mut changed = false;

        ui.add_space(SPACE_MD);
        // --- 曲线: master + RGB tone curves (engine + XMP already apply them,
        // this is purely the editing surface — Lightroom's panel order) --------
        egui::CollapsingHeader::new(section_title(tr(lang, "Curves"), curves_active))
            .id_salt("sec_curves")
            .default_open(false)
            .show(ui, |ui| {
                changed |= self.curve_editor(ui);
            });
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_hsl(&mut self, ui: &mut egui::Ui, hsl_active: bool) -> bool {
        let lang = self.lang;
        let mut changed = false;
        ui.add_space(SPACE_MD);
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
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_grading(&mut self, ui: &mut egui::Ui, grade_active: bool) -> bool {
        let lang = self.lang;
        let mut changed = false;

        ui.add_space(SPACE_MD);
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
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_detail(&mut self, ui: &mut egui::Ui, detail_active: bool) -> bool {
        let lang = self.lang;
        let mut changed = false;

        // Look ends here — detail, then geometry (lens BEFORE crop: the lens
        // profile / manual distortion redefine the frame the crop sits in).
        // Group boundary rhythm: fence + hairline + breather (deliberate —
        // stronger than the plain SPACE_MD fence between sibling sections).
        ui.add_space(SPACE_MD);
        ui.separator();
        ui.add_space(SPACE_XS);
        egui::CollapsingHeader::new(section_title(tr(lang, "Detail"), detail_active))
            .id_salt("sec_detail")
            .default_open(false)
            .show(ui, |ui| {
                {
                    let r = &mut self.recipe;
                    changed |= Self::slider(ui, lang, tr(lang, "Sharpening"), &mut r.sharpening, 0.0, 150.0, 0.0);
                    changed |=
                        Self::slider(ui, lang, tr(lang, "Noise Reduction"), &mut r.noise_reduction, 0.0, 100.0, 0.0);
                }
                // AI denoise as an ACTIVE op: run now, see it on canvas —
                // export-time denoise (the Export section toggle) stays for
                // batch/full-res workflows, but nobody should have to export
                // to find out what the denoiser does.
                ui.add_space(SPACE_SM);
                ui.horizontal(|ui| {
                    let ready = self.src_path.is_some() && !self.busy;
                    if ui
                        .add_enabled(ready, egui::Button::new(tr(lang, "🤖 AI Denoise now")))
                        .on_hover_text(tr(lang,
                            "Run the SCUNet GPU sidecar on this variant's pixels and show the result on canvas \
                             (undoable — bakes a clean base into the current variant; the develop sliders keep \
                             applying on top; first run downloads the model)",
                        ))
                        .clicked()
                    {
                        self.start_ai_denoise();
                    }
                    // RAW-only, exactly as the hover says — an enabled toggle
                    // on a baked source promised a mode that changes nothing.
                    let src_is_raw = self
                        .active_source_path()
                        .is_some_and(|p| autoshop::decode::is_raw(&p));
                    ui.add_enabled(src_is_raw, egui::Checkbox::new(&mut self.denoise_fullres, tr(lang, "Full-res")))
                        .on_hover_text(tr(lang,
                            "Denoise the full-sensor develop (slow, minutes on GPU); off = a ≤2048px working copy \
                             for a quick on-canvas result",
                        ));
                });
            });
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_lens(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let mut changed = false;

        // --- 镜头校正: in-camera profile + manual corrections -----------------
        ui.add_space(SPACE_MD);
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
                                // Refill only the MISSING channel(s): `has_c`
                                // accepts the union of saved + photo data, so
                                // replacing BOTH from the photo could erase a
                                // saved channel the photo can't supply and
                                // leave the pair incomplete (CA that renders
                                // nothing — geometry_active needs both).
                                if lp.ca_r.is_empty() {
                                    lp.ca_r = photo.ca_r.clone();
                                }
                                if lp.ca_b.is_empty() {
                                    lp.ca_b = photo.ca_b.clone();
                                }
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
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_crop(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let mut changed = false;

        // --- 裁剪 + 拉直: recipe.crop / straighten_deg (export + XMP paths) ---
        ui.add_space(SPACE_MD);
        let crop_active = self.recipe.crop.is_some() || self.recipe.straighten_deg != 0.0;
        egui::CollapsingHeader::new(section_title(tr(lang, "Crop"), crop_active))
            .id_salt("sec_crop")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // ✓ (geometric) — same finish-glyph as 「✓ Apply」; the
                    // emoji ✅ was the odd one out of the check family.
                    let label = if self.crop_mode { tr(lang, "✓ Done") } else { tr(lang, "⛶ Enter crop") };
                    if ui.button(label).clicked() {
                        let on = !self.crop_mode;
                        self.disarm_tools();
                        self.crop_mode = on;
                    }
                    let prev_aspect = self.crop_aspect;
                    egui::ComboBox::from_id_salt("crop_aspect")
                        .selected_text(tr(lang, CROP_ASPECTS[self.crop_aspect].0))
                        .width(70.0)
                        .show_ui(ui, |ui| {
                            for (i, (name, _)) in CROP_ASPECTS.iter().enumerate() {
                                ui.selectable_value(&mut self.crop_aspect, i, tr(lang, name));
                            }
                        });
                    // LR applies a picked preset to the box IMMEDIATELY — the
                    // combo used to only arm the ratio for the NEXT handle
                    // drag, so the UI said 4:5 while preview/export kept the
                    // old shape (D13). Consumed in handle_crop, where the
                    // view dims live — and armed ONLY while the crop tool is
                    // up: with the tool away the flag would survive until
                    // some later session's crop entry and rewrite the box as
                    // a surprise (Codex batch-38); tool-off keeps the old
                    // arm-for-the-next-drag meaning.
                    if self.crop_aspect != prev_aspect && self.crop_mode {
                        self.crop_aspect_pending = true;
                    }
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
                        "Once in (R): drag corner/edge handles to resize, drag inside to move, drag OUTSIDE the box (or the canvas border while the box is full-frame) to rotate-straighten; arrows nudge the box, Enter commits; preview, export and XMP all match. Straighten auto-crops the black corners.",
                    ))
                    .weak()
                    .small(),
                );
            });
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_masks(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let mut changed = false;

        // Geometry ends here — local adjustments and management below.
        // (Group boundary rhythm — see the Detail section's comment.)
        ui.add_space(SPACE_MD);
        ui.separator();
        ui.add_space(SPACE_XS);
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
                let lin_armed = matches!(self.placing_mask, Some((MaskKind::Linear, PlaceTarget::NewMask)));
                if ui.selectable_label(lin_armed, tr(lang, "＋ Linear gradient")).on_hover_text(tr(lang, "Drag on the image: start = fully-applied side, end = unaffected side (Shift = horizontal/vertical)")).clicked() {
                    self.disarm_tools();
                    if !lin_armed {
                        self.placing_mask = Some((MaskKind::Linear, PlaceTarget::NewMask));
                        self.status = tr(lang, "Drag on the image to draw a linear gradient (start fully applied → end unaffected; Shift = axis lock)").into();
                    }
                }
                let rad_armed = matches!(self.placing_mask, Some((MaskKind::Radial, PlaceTarget::NewMask)));
                if ui.selectable_label(rad_armed, tr(lang, "＋ Radial gradient")).on_hover_text(tr(lang, "Drag on the image to draw an elliptical area")).clicked() {
                    self.disarm_tools();
                    if !rad_armed {
                        self.placing_mask = Some((MaskKind::Radial, PlaceTarget::NewMask));
                        self.status = tr(lang, "Drag on the image to draw a radial (elliptical) area").into();
                    }
                }
                // LR's most-used local tool, finally: free-form brush →
                // Bitmap mask (the raster carrier + bilinear sampler + cache
                // all pre-existed; this wires the paint canvas into
                // recipe.masks — see start_mask_brush / commit_mask_brush).
                let brush_armed = matches!(self.mask_brush, Some((None, _)));
                if ui
                    .selectable_label(brush_armed, tr(lang, "🖌 Brush"))
                    .on_hover_text(tr(lang, "Paint a free-form mask ([ ] = brush size); 「Apply」 bakes it into a new mask"))
                    .clicked()
                {
                    if brush_armed {
                        self.disarm_tools();
                    } else if self.base_preview.is_some() {
                        self.start_mask_brush(None);
                    }
                }
            });
            // Brush-session controls (create OR raster-edit): erase toggle +
            // the bake/cancel pair. Lives up here so it is visible whichever
            // row armed the session.
            if let Some((target, erase)) = self.mask_brush {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(erase, tr(lang, "⌫ Erase"))
                        .on_hover_text(tr(lang, "Strokes remove from the selection instead of adding"))
                        .clicked()
                    {
                        self.mask_brush = Some((target, !erase));
                    }
                    if ui.button(tr(lang, "✓ Apply")).clicked() {
                        self.commit_mask_brush();
                    }
                    if ui.button(tr(lang, "✕ Cancel")).clicked() {
                        self.disarm_tools();
                    }
                });
            }
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
            // Empty state: an open section with zero rows looked like a bug;
            // one quiet line says where masks come from.
            if n_masks == 0 {
                ui.add_space(SPACE_XS);
                ui.label(
                    egui::RichText::new(tr(
                        lang,
                        "No masks yet — draw one with the tools above; AI Analyze adds its own too.",
                    ))
                    .weak()
                    .small(),
                );
            }
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
            let mut toggle_eye: Option<usize> = None;
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
                        // The activity dot IS the engine's own rule
                        // (render::engine_active): a parked mask looks parked,
                        // a working one shows ● — a 64-row list where the two
                        // were indistinguishable was a real navigation cost.
                        let active = m.enabled && autoshop::render::engine_active(m);
                        let enabled = m.enabled;
                        // L08: a dead raster renders INERT (the engine skips
                        // it with a stderr-only warning) while this row still
                        // read "enabled" — the ⚠ puts the fact on the row.
                        // Muted/parked masks skip the probe: nothing renders.
                        let dead = if active {
                            autoshop::render::dead_bitmap_rasters(m)
                        } else {
                            Vec::new()
                        };
                        let label = format!(
                            "{base} · {kind}{}{}",
                            if active { "  ●" } else { "" },
                            if dead.is_empty() { "" } else { "  ⚠" }
                        );
                        let label = if enabled {
                            egui::RichText::new(label)
                        } else {
                            egui::RichText::new(label).weak()
                        };
                        ui.dnd_drag_source(ui.id().with(("mask_row", i)), i, |ui| {
                            ui.label("☰");
                        })
                        .response
                        .on_hover_text(tr(self.lang, "Drag to reorder"));
                        // The eye: Lightroom's lossless mute. Amount-to-0 as a
                        // mute destroyed the tuned value; this keeps it.
                        if ui
                            .selectable_label(enabled, "👁")
                            .on_hover_text(tr(
                                self.lang,
                                "Show/mute this mask without losing its settings",
                            ))
                            .clicked()
                        {
                            toggle_eye = Some(i);
                        }
                        let mut row = ui.selectable_label(self.sel_mask == Some(i), label);
                        if !dead.is_empty() {
                            row = row.on_hover_text(trf(
                                self.lang,
                                "bitmap raster unreadable ({list}) — this mask currently has NO effect",
                                &[("list", &dead.join(", "))],
                            ));
                        }
                        if row.hovered() {
                            self.hover_mask = Some(i);
                        }
                        if row.clicked() {
                            self.sel_mask =
                                if self.sel_mask == Some(i) { None } else { Some(i) };
                            // Component selection belongs to ONE mask's list.
                            self.sel_component = None;
                            self.overlay_stale = true; // coverage follows the selection
                            // EVERY index-armed tool follows the selection —
                            // the colour sampler was fixed first, then the
                            // same class recurred for ↻ Redraw /
                            // add-component / the raster-edit brush, whose
                            // arming indicators all live inside the
                            // selected-mask block (L06#3). The single owner
                            // sits beside remap_mask_indices; the brush
                            // teardown discards user paint, so it is said.
                            if self.disarm_selection_bound_tools(self.sel_mask) {
                                let t = tr(
                                    self.lang,
                                    "the raster-edit brush session ended — you selected another mask; its unbaked strokes were discarded",
                                );
                                self.toast(ToastKind::Error, t.to_string());
                            }
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
                // Bounds re-checked at APPLY time: the payload index was
                // captured at drag start, and the list can shrink mid-drag
                // (Ctrl+Z, an async AI result) — a stale `from` would panic
                // in Vec::remove.
                && from < self.recipe.masks.len()
                && insert <= self.recipe.masks.len()
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
            if let Some(i) = toggle_eye {
                // A recipe mutation like any slider: develop + overlay follow,
                // and commit_if_settled makes it one undo step.
                self.recipe.masks[i].enabled = !self.recipe.masks[i].enabled;
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
                        let redraw_armed = matches!(
                            self.placing_mask,
                            Some((k, PlaceTarget::Redraw(j))) if k == kind && j == i
                        );
                        if ui
                            .selectable_label(redraw_armed, tr(lang, "↻ Redraw"))
                            .on_hover_text(tr(lang, "Re-drag this mask's area on the image"))
                            .clicked()
                        {
                            self.disarm_tools();
                            if !redraw_armed {
                                self.placing_mask = Some((kind, PlaceTarget::Redraw(i)));
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
                    // Duplicate: a second gradient with the same tuned ten
                    // sliders used to mean re-dragging and re-typing every
                    // value. Rasters are DETACHED copies (the version-load
                    // rule), so the twins never share a mutable file.
                    if ui
                        .small_button("⧉")
                        .on_hover_text(tr(lang, "Duplicate this mask (bitmap rasters are copied, so the copies stay independent)"))
                        .clicked()
                    {
                        if self.recipe.masks.len() >= 64 {
                            let t = tr(lang, "mask limit reached (64) — delete one first");
                            self.toast(ToastKind::Error, t.to_string());
                        } else {
                            let mut clone = self.recipe.masks[i].clone();
                            if let Some(p) = self.src_path.as_deref() {
                                let mut tmp = EditRecipe { masks: vec![clone], ..Default::default() };
                                autoshop::store::detach_rasters(p, &mut tmp, "mask-dup");
                                clone = tmp.masks.remove(0);
                            }
                            if !clone.name.is_empty() {
                                clone.name = format!("{} 2", clone.name);
                            }
                            self.recipe.masks.insert(i + 1, clone);
                            self.remap_mask_indices(|s| Some(if s > i { s + 1 } else { s }));
                            self.sel_mask = Some(i + 1);
                            self.sel_component = None;
                            self.overlay_stale = true;
                            // The remap maps an armed Redraw(i)/brush(i) to
                            // ITSELF (s > i is false at s == i) while the
                            // selection jumps to the copy — armed on the
                            // original, pointed at the duplicate (L06#3's
                            // second instance). Selection moved ⇒ same rule.
                            if self.disarm_selection_bound_tools(self.sel_mask) {
                                let t = tr(
                                    self.lang,
                                    "the raster-edit brush session ended — you selected another mask; its unbaked strokes were discarded",
                                );
                                self.toast(ToastKind::Error, t.to_string());
                            }
                            changed = true;
                        }
                    }
                });
                // Radial geometry: edge feather (shown 0..100, LR's track) +
                // rotation + inside/outside flip — all engine-rendered.
                if let MaskGeometry::Radial { feather, flipped, angle, .. } =
                    &mut self.recipe.masks[i].mask
                {
                    let mut geo_ch = false;
                    ui.horizontal(|ui| {
                        geo_ch |= Self::slider_pct(
                            ui,
                            lang,
                            tr(lang, "Edge feather"),
                            feather,
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
                    // Rotation: the on-image grip (the knob parked above the
                    // ellipse) drags it too; the slider gives numeric entry.
                    geo_ch |= Self::slider(
                        ui,
                        lang,
                        tr(lang, "Angle"),
                        angle,
                        -180.0,
                        180.0,
                        0.0,
                    );
                    if geo_ch {
                        self.overlay_stale = true;
                        changed = true;
                    }
                }
                // Bitmap (AI / brush) raster tools: before these, a raster
                // mask was completely un-editable after creation — a clipped
                // treeline's only recourse was re-running the model to get
                // the same raster. Every op BAKES a freshly claimed raster
                // and repoints (never mutates the input file).
                if matches!(self.recipe.masks[i].mask, MaskGeometry::Bitmap { .. }) {
                    ui.horizontal_wrapped(|ui| {
                        let edit_armed = matches!(self.mask_brush, Some((Some(j), _)) if j == i);
                        if ui
                            .selectable_label(edit_armed, tr(lang, "🖌 Edit raster"))
                            .on_hover_text(tr(lang, "Brush-edit this mask: paint adds, 「Erase」 removes, 「Apply」 bakes"))
                            .clicked()
                        {
                            if edit_armed {
                                self.disarm_tools();
                            } else {
                                self.start_mask_brush(Some(i));
                            }
                        }
                        if ui
                            .button(tr(lang, "◌ Feather"))
                            .on_hover_text(tr(lang, "Soften the mask boundary one step (bakes a new raster; repeat for more)"))
                            .clicked()
                        {
                            self.bake_mask_raster(
                                i,
                                |g| {
                                    let s = (g.width().max(g.height()) as f32 / 400.0).max(1.0);
                                    autoshop::render::feather_mask(g, s)
                                },
                                "mask-edit",
                            );
                        }
                        if ui
                            .button(tr(lang, "⊕ Expand"))
                            .on_hover_text(tr(lang, "Grow the selection one step (bakes a new raster)"))
                            .clicked()
                        {
                            self.bake_mask_raster(
                                i,
                                |g| {
                                    let r = (g.width().max(g.height()) / 500).max(2) as i32;
                                    autoshop::render::morph_mask(g, r)
                                },
                                "mask-edit",
                            );
                        }
                        if ui
                            .button(tr(lang, "⊖ Contract"))
                            .on_hover_text(tr(lang, "Shrink the selection one step (bakes a new raster)"))
                            .clicked()
                        {
                            self.bake_mask_raster(
                                i,
                                |g| {
                                    let r = (g.width().max(g.height()) / 500).max(2) as i32;
                                    autoshop::render::morph_mask(g, -r)
                                },
                                "mask-edit",
                            );
                        }
                        if ui
                            .add_enabled(!self.busy, egui::Button::new(tr(lang, "⇱ Full-res refine")))
                            .on_hover_text(tr(lang, "Re-cut this mask against the FULL-resolution source (guided filter). Preview-res AI masks smear their boundary at export — this snaps it to real edges. Decodes the full-size source; takes a few seconds."))
                            .clicked()
                        {
                            self.start_mask_refine(i);
                        }
                    });
                }
                // --- Shapes（组件）: compose extra geometry onto this mask —
                // Lightroom's Add / Subtract / Intersect grammar. ENGINE-ONLY
                // (recipe.rs MaskComponent): the XMP projection carries the
                // base shape alone.
                {
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Shapes"));
                        let mode_label = |m: autoshop::recipe::MaskCombine| match m {
                            autoshop::recipe::MaskCombine::Add => tr(lang, "＋ Add"),
                            autoshop::recipe::MaskCombine::Subtract => tr(lang, "－ Subtract"),
                            autoshop::recipe::MaskCombine::Intersect => tr(lang, "∩ Intersect"),
                        };
                        egui::ComboBox::from_id_salt("comp_mode")
                            .selected_text(mode_label(self.component_mode))
                            .width(96.0)
                            .show_ui(ui, |ui| {
                                for m in [
                                    autoshop::recipe::MaskCombine::Add,
                                    autoshop::recipe::MaskCombine::Subtract,
                                    autoshop::recipe::MaskCombine::Intersect,
                                ] {
                                    ui.selectable_value(&mut self.component_mode, m, mode_label(m));
                                }
                            });
                        let mode = self.component_mode;
                        let lin_armed = matches!(
                            self.placing_mask,
                            Some((MaskKind::Linear, PlaceTarget::Component(j, _))) if j == i
                        );
                        if ui
                            .selectable_label(lin_armed, tr(lang, "▭ Linear"))
                            .on_hover_text(tr(lang, "Drag on the image to add a linear shape to THIS mask"))
                            .clicked()
                        {
                            self.disarm_tools();
                            if !lin_armed {
                                self.placing_mask =
                                    Some((MaskKind::Linear, PlaceTarget::Component(i, mode)));
                                self.status = tr(lang, "Drag on the image: the new shape composes onto this mask").into();
                            }
                        }
                        let rad_armed = matches!(
                            self.placing_mask,
                            Some((MaskKind::Radial, PlaceTarget::Component(j, _))) if j == i
                        );
                        if ui
                            .selectable_label(rad_armed, tr(lang, "◯ Radial"))
                            .on_hover_text(tr(lang, "Drag on the image to add an elliptical shape to THIS mask"))
                            .clicked()
                        {
                            self.disarm_tools();
                            if !rad_armed {
                                self.placing_mask =
                                    Some((MaskKind::Radial, PlaceTarget::Component(i, mode)));
                                self.status = tr(lang, "Drag on the image: the new shape composes onto this mask").into();
                            }
                        }
                    });
                    let n_comp = self.recipe.masks[i].components.len();
                    let mut del_comp: Option<usize> = None;
                    for c in 0..n_comp {
                        ui.horizontal(|ui| {
                            let comp = &self.recipe.masks[i].components[c];
                            let icon = match comp.mode {
                                autoshop::recipe::MaskCombine::Add => "＋",
                                autoshop::recipe::MaskCombine::Subtract => "－",
                                autoshop::recipe::MaskCombine::Intersect => "∩",
                            };
                            let kind = match comp.geometry {
                                MaskGeometry::Linear { .. } => tr(lang, "Linear"),
                                MaskGeometry::Radial { .. } => tr(lang, "Radial"),
                                MaskGeometry::Bitmap { .. } => tr(lang, "Bitmap"),
                            };
                            let selected = self.sel_component == Some(c);
                            if ui
                                .selectable_label(selected, format!("{icon} {kind}"))
                                .on_hover_text(tr(lang, "Select to drag this shape's knobs on the image (the base mask's knobs come back when deselected)"))
                                .clicked()
                            {
                                self.sel_component = if selected { None } else { Some(c) };
                            }
                            let mut mode = comp.mode;
                            egui::ComboBox::from_id_salt(("comp_row_mode", c))
                                .selected_text(match mode {
                                    autoshop::recipe::MaskCombine::Add => tr(lang, "Add"),
                                    autoshop::recipe::MaskCombine::Subtract => tr(lang, "Subtract"),
                                    autoshop::recipe::MaskCombine::Intersect => tr(lang, "Intersect"),
                                })
                                .width(88.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut mode, autoshop::recipe::MaskCombine::Add, tr(lang, "Add"));
                                    ui.selectable_value(&mut mode, autoshop::recipe::MaskCombine::Subtract, tr(lang, "Subtract"));
                                    ui.selectable_value(&mut mode, autoshop::recipe::MaskCombine::Intersect, tr(lang, "Intersect"));
                                });
                            if mode != comp.mode {
                                self.recipe.masks[i].components[c].mode = mode;
                                self.overlay_stale = true;
                                changed = true;
                            }
                            if ui.small_button("🗑").clicked() {
                                del_comp = Some(c);
                            }
                        });
                    }
                    if let Some(c) = del_comp {
                        self.recipe.masks[i].components.remove(c);
                        self.sel_component = match self.sel_component {
                            Some(s) if s == c => None,
                            Some(s) if s > c => Some(s - 1),
                            other => other,
                        };
                        self.overlay_stale = true;
                        changed = true;
                    }
                    if n_comp > 0 {
                        ui.label(
                            egui::RichText::new(tr(lang, "Shapes compose in order onto the base mask. In-app render + export only — the Lightroom XMP carries the base shape alone."))
                                .weak()
                                .small(),
                        );
                    }
                }
                // --- Range Mask（LR 范围蒙版）: refines WHERE the geometry applies —
                // final weight = geometry × range, live in preview + export + XMP.
                // Coverage invalidation happens ONCE below (after Amount): any
                // edit up to there can move the wash, and the overlay key
                // compare dedupes rebuilds — the old "range-section-only"
                // tracker went blind whenever an earlier control had already
                // set `changed` in the same frame.
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
                        // Leaving Color: the armed sampler goes too, or the
                        // canvas keeps advertising (and dispatching) a colour
                        // pick for a range that no longer exists.
                        if cur == 2 && self.range_picking == Some(i) {
                            self.range_picking = None;
                        }
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
                            self.status = tr(lang, "Color range: click the color to pick in the image").into();
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
                            // All three on LR's 0..100 display track (storage
                            // stays the 0..1 fraction — see slider_pct).
                            let mut f = ((*lo - *lo_outer) + (*hi_outer - *hi)) * 0.5;
                            let ch_lo = Self::slider_pct(ui, lang, tr(lang, "Lum. low"), lo, 1.0, 0.0);
                            let ch_hi = Self::slider_pct(ui, lang, tr(lang, "Lum. high"), hi, 1.0, 1.0);
                            let ch_f = Self::slider_pct(ui, lang, tr(lang, "Feather"), &mut f, 0.5, 0.1);
                            if ch_lo || ch_hi || ch_f {
                                // CLAMP the edited endpoint at the other, never
                                // swap: the active slider stays bound to its
                                // field across drag frames, so swapping made
                                // both bounds follow the pointer and converge.
                                if *lo > *hi {
                                    if ch_lo { *lo = *hi } else { *hi = *lo }
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
                                if ui.small_button(label).on_hover_text(tr(lang, "Click the color to pick in the image (the same color at other brightnesses is also selected; clicking this button again cancels sampling)")).clicked() {
                                    want_pick = true;
                                }
                            });
                            changed |= Self::slider_pct(ui, lang, tr(lang, "Tolerance"), amount, 1.0, 0.5);
                        }
                        None => {}
                    }
                    if want_pick {
                        self.disarm_tools();
                        if !picking_this {
                            self.range_picking = Some(i);
                            self.status = tr(lang, "Color range: click the color to pick in the image").into();
                        }
                    }
                }
                let m = &mut self.recipe.masks[i];
                // LR's 0..100 display track (storage stays 0..1 — slider_pct).
                changed |= Self::slider_pct(ui, lang, tr(lang, "Amount"), &mut m.amount, 1.0, 1.0);
                // Any edit up to here — geometry, range, Amount — can move the
                // coverage wash (the overlay key carries them all); the key
                // compare inside refresh_mask_overlay dedupes rebuilds, so
                // over-flagging is free. The tone sliders below are not
                // coverage and deliberately stay out.
                if changed {
                    self.overlay_stale = true;
                }
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
        changed
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_versions(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;

        // --- 版本: recipe snapshots ≈ LR virtual copies (gap batch G) --------
        ui.add_space(SPACE_MD);
        let n_ver = self.versions.len();
        let n_ver_s = n_ver.to_string();
        egui::CollapsingHeader::new(section_title(&trf(lang, "Versions ({n})", &[("n", &n_ver_s)]), n_ver > 0))
            .id_salt("sec_versions")
            .default_open(false)
            .show(ui, |ui| {
                // Clearing the list on open (see `open_path`) already stops a
                // STALE row from acting on the incoming photo. This is the
                // second half: while a photo is still decoding, "Save as
                // version" would snapshot the OUTGOING photo's canvas — the
                // fresh recipe has not landed yet — into the incoming photo's
                // develop. Nothing here is meaningful until the open settles,
                // so the whole section goes inert rather than each button
                // carrying its own guard.
                ui.add_enabled_ui(!self.busy, |ui| {
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
                    // NoWait wrapper: delete_version locks internally with
                    // Wait — a foreground 🗑 click must fail into the error
                    // arm when another process holds the develop, not hang.
                    match autoshop::store::with_develop_lock(
                        &src,
                        autoshop::store::DevelopLockMode::NoWait,
                        || autoshop::store::delete_version(&src, n),
                    ) {
                        Ok(()) => {
                            self.refresh_versions();
                            self.status =
                                trf(lang, "Version v{n} deleted", &[("n", &n.to_string())]);
                        }
                        Err(e) => {
                            self.persist_postponed(
                                &e,
                                "Delete v{n} failed: {err}",
                                &[("n", &n.to_string())],
                            );
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
                }); // add_enabled_ui: inert while a photo is still opening
            });
        false
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_export(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;

        // --- 导出设置 (UX batch): moved out of the toolbar — touched once per
        // delivery, these are Export-dialog contents, not toolbar chrome. The
        // toolbar keeps the ACTIONS; their hover echoes this section's state.
        ui.add_space(SPACE_MD);
        let export_active = self.exp_format != ExportFormat::Tiff16
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
                        .selected_text(tr(lang, self.exp_format.label()))
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for f in ExportFormat::ALL {
                                ui.selectable_value(&mut self.exp_format, f, tr(lang, f.label()));
                            }
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
                ui.add_enabled_ui(self.exp_format == ExportFormat::Jpeg, |ui| {
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
        false
    }
}
