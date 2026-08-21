//! Develop panel: sliders, sections, variant strip.

use crate::*;

// The section ● predicate and the table it reads (R25 P0) — see
// `develop_panel` and `dev_lens`.
use autoshop::advisor::catalogue::{family_is_active, CONTROL_FAMILIES};

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
        Self::slider_hinted(ui, lang, label, value, min, max, default, "")
    }

    /// [`slider`] plus ONE disclosure line at the head of its tooltip — for a
    /// control whose value is SHARED with another panel, where the tooltip is
    /// the only place that fact can be told. (`slider` returns `bool`, not a
    /// `Response`, so a caller cannot chain its own `on_hover_text`; a second
    /// tooltip on a wrapper rect would just stack two bubbles over one
    /// widget.) The brush radius is the case: one number behind the mask
    /// brush AND Fill / Heal / Stamp.
    #[allow(clippy::too_many_arguments)] // `slider` + the one extra tooltip line
    pub(crate) fn slider_hinted(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
        default: f32,
        hint: &str,
    ) -> bool {
        let feel =
            if max - min >= 20.0 { SliderFeel::Int } else { SliderFeel::Frac };
        Self::slider_impl(ui, lang, label, value, min, max, default, feel, hint)
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
        Self::slider_pct_hinted(ui, lang, label, value, max, default, "")
    }

    /// [`slider_pct`] plus ONE disclosure line at the head of its tooltip — the
    /// percent-track twin of [`slider_hinted`], same reason (the helpers return
    /// `bool`, so a caller cannot chain its own `on_hover_text`). For a fraction
    /// slider whose meaning cannot be read off its label: the AI 「Style」
    /// strength, which has to say what it leans on and that 0 means "ignore my
    /// habits" (R22 #16 — it was a bare `egui::Slider` carrying that sentence
    /// itself, which is exactly why it had no reset gesture and no ↑/↓ nudge).
    pub(crate) fn slider_pct_hinted(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        max: f32,
        default: f32,
        hint: &str,
    ) -> bool {
        let mut disp = *value * 100.0;
        let changed = Self::slider_hinted(
            ui,
            lang,
            label,
            &mut disp,
            0.0,
            max * 100.0,
            default * 100.0,
            hint,
        );
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
        Self::slider_impl(ui, lang, label, value, min, max, default, SliderFeel::Fine, "")
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
        Self::slider_impl(ui, lang, label, value, min, max, default, SliderFeel::LogK, "")
    }

    #[allow(clippy::too_many_arguments)] // private impl detail shared by four thin public shapes
    pub(crate) fn slider_impl(
        ui: &mut egui::Ui,
        lang: Lang,
        label: &str,
        value: &mut f32,
        min: f32,
        max: f32,
        default: f32,
        feel: SliderFeel,
        // Prepended to the shared tooltip; "" for every ordinary slider. See
        // `slider_hinted` — the ONE tooltip is the point, not two stacked.
        hint: &str,
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
        let grammar = tr(lang, "double-click / right-click resets · hover + ↑/↓ nudges (Shift ×10)");
        let tip = if hint.is_empty() {
            grammar.to_string()
        } else {
            format!("{hint}\n{grammar}")
        };
        let resp = ui
            .add(
                egui::Slider::new(value, min..=max)
                    .logarithmic(matches!(feel, SliderFeel::LogK))
                    .step_by(snap)
                    .fixed_decimals(decimals)
                    .text(label),
            )
            .on_hover_text(tip);
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
        // OUTSIDE the editable gate below, deliberately (#4 review point): the
        // histogram is a READOUT. Its only interaction is the pair of clipping
        // triangles, whose click reaches `toggle_clipping` — that writes
        // `show_clipping` + the overlay texture and, on a fresh open with no
        // retained frame, `dirty`. None of those is EDIT state: no recipe field
        // is read into a widget or written back, so the L15-2 input-loss class
        // (edit photo A's sliders while B lands) cannot arise here. The worst
        // mid-open outcome is one preview build whose frame `finish_redevelop`
        // then discards by its base+recipe identity check. Greying the tone
        // readout during every open would be the bigger regression.
        self.histogram_ui(ui);
        ui.add_space(SPACE_SM);

        // Mid-open (decode in flight) these controls would edit the STASHED
        // photo A while B lands and replaces the whole recipe — silent input
        // loss (L15-2). Busy alone must NOT gate here: a 600 s analyze keeps
        // the panel live; only the open transition freezes it.
        // The AI area carries its OWN copy of this gate (panels/ai.rs) — it
        // used to ride inside this closure as `dev_ai`.
        let editable = !self.open_in_flight;
        ui.add_enabled_ui(editable, |ui| {
        // Lightroom-style grouping: a wall of 16 sliders scans terribly; four
        // titled sections (tone open, the rest by activity) scan at a glance.
        // A section whose values are non-neutral shows a ● so a collapsed
        // active adjustment is never invisible. Flags are snapshot up front —
        // Copy bools, so no borrow spans the section closures (E0500).
        //
        // The five predicates are DERIVED from the control registry's families
        // (R25 P0): each was a hand-written field tuple that had to be widened
        // by hand whenever its section gained a control, and R22 #16 had
        // already found four of them one field short. `family_is_active` reads
        // the same table the AI's tool plan is built from, so a control that
        // joins a family joins its dot.
        let (presence_active, detail_active, hsl_active, grade_active, curves_active, effects_active) = {
            let r = &self.recipe;
            let fam = |name: &str| {
                CONTROL_FAMILIES
                    .iter()
                    .find(|f| f.name == name)
                    .is_some_and(|f| family_is_active(f, r))
            };
            (
                fam("presence"),
                // R25 B3: the Detail section holds BOTH halves — the two
                // sliders the AI plans with and the eight carried shaping
                // axes — so its ● is the OR of the two families. They are
                // separate families because one is AI-visible and one is not
                // (see `CONTROL_FAMILIES`), not because the panel splits them.
                fam("detail") || fam("detail_effects"),
                fam("hsl"),
                fam("color_grade"),
                fam("curves"),
                // R25 B2: the `effects` family has no AI-visible member, so it
                // reaches the model nowhere — it exists in the table for
                // exactly this, the section's own ●.
                fam("effects"),
            )
        };
        changed |= self.dev_tone_wb(ui);
        changed |= self.dev_presence(ui, presence_active);
        changed |= self.dev_curves(ui, curves_active);
        changed |= self.dev_hsl(ui, hsl_active);
        changed |= self.dev_grading(ui, grade_active);
        changed |= self.dev_detail(ui, detail_active);
        changed |= self.dev_effects(ui, effects_active);
        changed |= self.dev_lens(ui);
        // R25 B4, in Lightroom's own panel order (Transform follows Lens
        // Corrections). Read-only, so it can never set `changed` — and it
        // draws nothing at all unless the photo carried such a block.
        changed |= self.dev_transform(ui);
        changed |= self.dev_crop(ui);
        changed |= self.dev_masks(ui);
        changed |= self.dev_versions(ui);
        changed |= self.dev_export(ui);
        });

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
        // ONE deferred action for the whole strip (R24-5): the five separate
        // `Option`s this replaced were dispatched by an `else if` chain whose
        // exclusivity was a property of the chain; a single value makes it a
        // property of the TYPE, and it is the same value the edit-state list
        // hands to the same owner. Last writer wins where the chain had a
        // fixed priority — reachable only if two widgets were clicked in one
        // frame, which one pointer cannot do.
        let mut act: Option<VariantAction> = None;
        let busy = self.busy; // Copy — read before the card closures borrow self
        // The card column is centered by an EXPLICIT top pad, not by
        // cross-align: Align::Center positioned the row before its content
        // grew — egui seeds a horizontal row at interact_size.y, centered
        // that 26 px stub in the panel, and the 84 px column then grew
        // downward from it (37 px of air above, 21 px of column OVERFLOW
        // below the panel; R14 user report). The column height is knowable
        // up front — thumb + item gap + label row (egui rows are at least
        // interact_size.y tall) — so the pad is exact and the geometry test
        // asserts the air above equals the air below.
        // Named STRIP_ to stay clear of model.rs's gallery THUMB_H (40.0),
        // which this local used to SHADOW inside variant_strip — one glob
        // import away from a silent geometry swap in either direction.
        const STRIP_THUMB_H: f32 = 52.0;
        let card_h = STRIP_THUMB_H + ui.spacing().item_spacing.y + ui.spacing().interact_size.y;
        #[cfg(test)]
        {
            self.strip_row_rect = Some(ui.max_rect());
        }
        ui.add_space(((ui.available_height() - card_h) / 2.0).max(0.0));
        // LAYOUT INVARIANT (probed, R14+R15): egui places the scroll area
        // centered by its 26 px DECLARED seed against the row's height at
        // placement time, then grows content DOWNWARD — (current−26)/2 of
        // offset. Keeping every child before the scroll area at ZERO height
        // (pure horizontal spacing) pins that offset at exactly 0, which the
        // geometry test asserts. (Every cross-align variant re-broke it,
        // measured 37/29/11.8 px — all equal to (row_height−26)/2.) The
        // title and divider therefore do not participate in layout at all
        // (R16): they are PAINTED at rects computed from the same knowns as
        // the pad — the one way to center them on the card column without
        // re-breaking the row.
        let row_top = ui.cursor().top();
        let strip_left = ui.max_rect().left();
        let title_galley = ui.painter().layout_no_wrap(
            tr(lang, "Variants").to_owned(),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().strong_text_color(),
        );
        let title_w = title_galley.size().x;
        let title_pos = egui::pos2(
            strip_left + SPACE_SM,
            row_top + (card_h - title_galley.size().y) / 2.0,
        );
        #[cfg(test)]
        {
            self.strip_title_rect =
                Some(egui::Rect::from_min_size(title_pos, title_galley.size()));
        }
        ui.painter().galley(title_pos, title_galley, ui.visuals().strong_text_color());
        // The divider, at full card height — the stock separator filled the
        // 26 px seed, a stub over the card's top third.
        let sep_x = strip_left + SPACE_SM + title_w + SPACE_SM + 3.0;
        ui.painter().vline(
            sep_x,
            egui::Rangef::new(row_top, row_top + card_h),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.horizontal(|ui| {
            // Horizontal footprint of the PAINTED title + divider: width
            // only, zero height (the invariant above).
            ui.add_space(SPACE_SM + title_w + SPACE_SM + 6.0 + SPACE_SM);
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
                                let h = STRIP_THUMB_H;
                                let w = (s.x / s.y.max(1.0) * h).clamp(30.0, 104.0);
                                let (rect, resp) =
                                    ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
                                let uv = egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                );
                                // Card chrome tokens (L15-1): the glow is
                                // the PILL accent at low alpha and its radius
                                // is DERIVED — the literal 0xc9a14a/7.0 copy
                                // silently detached from any theme change.
                                const CARD_RADIUS: f32 = 4.0;
                                const GLOW_EXPAND: f32 = 3.0;
                                if active {
                                    ui.painter().rect_filled(
                                        rect.expand(GLOW_EXPAND),
                                        CARD_RADIUS + GLOW_EXPAND,
                                        egui::Color32::from_rgba_unmultiplied(
                                            PILL.r(),
                                            PILL.g(),
                                            PILL.b(),
                                            46,
                                        ),
                                    );
                                }
                                ui.painter().image(t.id(), rect, uv, egui::Color32::WHITE);
                                if active {
                                    ui.painter().rect_stroke(
                                        rect,
                                        CARD_RADIUS,
                                        egui::Stroke::new(2.0, PILL),
                                    );
                                }
                                resp
                            } else {
                                ui.add_sized([64.0, STRIP_THUMB_H], egui::Button::new("…"))
                            };
                            #[cfg(test)]
                            if i == 0 {
                                self.strip_thumb_rect = Some(resp.rect);
                            }
                            if resp.on_hover_text(tr(lang, "Click to switch to this variant (lossless)")).clicked() {
                                act = Some(VariantAction::Switch(i));
                            }
                            ui.horizontal(|ui| {
                                let label = egui::RichText::new(tr(lang, kind.label())).small();
                                ui.label(if active { label.strong().color(accent) } else { label });
                                // The card's NAME (R24-3), which until now
                                // had a persistence path (variants.json,
                                // R24-2) and no producer. The ACTIVE card
                                // edits it in place — the version rows'
                                // discipline, keyed by the card's own id so a
                                // background push or delete cannot land the
                                // text on a different card; a background card
                                // only DISPLAYS the name it has (offering a
                                // 「Name…」 placeholder on every card would
                                // widen the whole strip for an affordance
                                // that acts on one of them).
                                let card_id = self.variants[i].id.clone();
                                let named = self.variants[i].name.clone();
                                let editing = active
                                    && self.variant_name_buf.as_ref().is_some_and(|(p, id, ..)| {
                                        *id == card_id
                                            && Some(p.as_path()) == self.src_path.as_deref()
                                    });
                                if editing {
                                    let resp = {
                                        let buf =
                                            &mut self.variant_name_buf.as_mut().expect("checked just above").3;
                                        ui.add(
                                            // Keyed by the CARD's id, like
                                            // the buffer: an index key made
                                            // egui hand the box a new
                                            // identity (and drop the caret)
                                            // the moment an async push
                                            // renumbered the strip.
                                            egui::TextEdit::singleline(buf)
                                                .id(ui.make_persistent_id((
                                                    "variant_name",
                                                    card_id.as_str(),
                                                )))
                                                .desired_width(90.0)
                                                .hint_text(tr(lang, "Name")),
                                        )
                                    };
                                    // The +1 boundary: this box's own
                                    // lost-focus commit (Enter counts as
                                    // losing focus in egui), and the frame it
                                    // appears in has to claim the caret — the
                                    // click that opened it landed on the
                                    // label that used to be here.
                                    #[cfg(test)]
                                    {
                                        self.strip_name_rect = Some(resp.rect);
                                    }
                                    if resp.lost_focus() {
                                        self.commit_variant_name_buf();
                                        self.variant_name_buf = None;
                                    } else if !resp.has_focus() {
                                        resp.request_focus();
                                    }
                                } else if active {
                                    let shown = match &named {
                                        Some(n) => egui::RichText::new(format!("「{n}」")).small(),
                                        None => egui::RichText::new(tr(lang, "Name…").to_string())
                                            .small()
                                            .weak(),
                                    };
                                    let resp = ui
                                        .add(egui::Label::new(shown).sense(egui::Sense::click()))
                                        .on_hover_text(tr(lang, "Name this variant"));
                                    #[cfg(test)]
                                    {
                                        self.strip_name_rect = Some(resp.rect);
                                    }
                                    if resp.clicked() {
                                        act = Some(VariantAction::Rename(card_id.clone()));
                                    }
                                } else if let Some(n) = &named {
                                    ui.label(egui::RichText::new(format!("「{n}」")).small().weak());
                                }
                                if let Some(a) = self.variant_card_buttons(
                                    ui,
                                    i,
                                    active,
                                    busy,
                                    VariantSurface::Strip,
                                ) {
                                    act = Some(a);
                                }
                            });
                            #[cfg(test)]
                            if i == 0 {
                                self.strip_card_rect = Some(ui.min_rect());
                            }
                        });
                        ui.add_space(SPACE_MD);
                    }
                });
            });
        });
        if let Some(a) = act {
            self.dispatch_variant_action(a, ui.ctx());
        }
    }

    /// The card's ACTION buttons — ＋ archive, ▣ apply-to-Original, ✕ delete
    /// — for whichever surface is drawing the card (R24-5).
    ///
    /// Extracted from `variant_strip` verbatim when the Versions section's
    /// edit-state list grew the same three buttons: a card action must mean
    /// one thing and be implemented once, or the two lists answer 「what does
    /// ✕ do here」 differently the first time either is touched. The strip
    /// keeps the rename box (see [`VariantSurface`]).
    ///
    /// Returns the deferred action; the caller performs it through
    /// `dispatch_variant_action` after the layout closure has released `self`.
    // `surface` decides only which headless seam records the row, so it is
    // genuinely unused in a release build — the alternative (two near-copies
    // of this function) is the thing the extraction exists to remove.
    #[cfg_attr(not(test), allow(unused_variables))]
    pub(crate) fn variant_card_buttons(
        &mut self,
        ui: &mut egui::Ui,
        i: usize,
        active: bool,
        busy: bool,
        surface: VariantSurface,
    ) -> Option<VariantAction> {
        let lang = self.lang;
        let kind = self.variants[i].kind;
        let mut act: Option<VariantAction> = None;
        // Which glyphs this row actually laid out — the list surface's test
        // seam, since it has no rects of its own to assert on.
        #[cfg(test)]
        let mut drawn: Vec<&str> = Vec::new();
        // R24-2 (user decision ②, in place of save-as-you-go snapshots): a
        // one-click archive entry point ON the card whose develop it
        // snapshots. Active card only — the button acts on the LIVE canvas,
        // and offering it on a background card would promise a snapshot of
        // something else. A small_button in the existing label row: the row is
        // already at least interact_size.y tall, so the strip's card-height
        // arithmetic (and its geometry test) is untouched.
        // Inert (visibly, not silently) while a photo is still opening — the
        // canvas then still holds the OUTGOING photo's develop, and the
        // snapshot would land it in the incoming photo's store. Same rule the
        // Versions section applies to its own copy.
        if active {
            #[cfg(test)]
            drawn.push("＋");
            if ui
                .add_enabled(!busy, egui::Button::new("＋").small())
                .on_hover_text(tr(lang, "Save all current develop parameters as a numbered snapshot (v<N>.recipe.json in this photo's develop store), reloadable anytime"))
                .clicked()
            {
                act = Some(VariantAction::SaveVersion);
            }
        }
        // R24-3 (#7) 「apply to Original」, on the ACTIVE card for the same
        // reason ＋ is: it copies the LIVE canvas. Two concepts the hover keeps
        // apart — it overwrites the ▣ Original CARD's develop parameters (its
        // pixels stay, this card stays), and Ctrl+S is what afterwards makes
        // that card's develop the photo's SAVED develop.
        // A pixel-state card shows the button DISABLED with the reason rather
        // than hiding it (the R24-2 save_version judgement, same remedy): its
        // look is in its raster, so there is nothing parametric to copy.
        // Hidden entirely when this card IS the negative, or when the strip
        // has no Original card to apply onto.
        if active && kind != VariantKind::Original && self.original_index().is_some() {
            let can = kind.is_parametric();
            let hover = if can {
                tr(lang, "Copy this variant's develop onto the ▣ Original card — its baked pixels and this card both stay. One Ctrl+Z undoes it; Ctrl+S then saves it as this photo's develop")
            } else {
                tr(lang, "A generated variant's look lives in its pixels — there are no develop parameters to copy onto the ▣ Original card; run 「Reverse-fit」 first")
            };
            let resp =
                ui.add_enabled(can && !busy, egui::Button::new("▣").small()).on_hover_text(hover);
            #[cfg(test)]
            drawn.push(if can && !busy { "▣" } else { "▣(off)" });
            #[cfg(test)]
            if surface == VariantSurface::Strip {
                // The STRIP owns this seam: a full frame draws both surfaces,
                // and letting the list overwrite it would silently retarget
                // every existing assertion at the other widget.
                self.strip_apply = Some((resp.rect, can && !busy));
            }
            if resp.clicked() {
                act = Some(VariantAction::Apply(i));
            }
        }
        // Any variant except the sole Original can be dropped.
        // ✕ — the ONE close/cancel glyph app-wide (the old bare × was the
        // fourth variant of it). ARMED (R24-4): the first click asks, the
        // second deletes — the button says which, because a deleted card
        // cannot be brought back (unlike a version, whose number the registry
        // keeps burned rather than the card recoverable).
        let armed = self.variant_delete_confirm == Some(i);
        if self.variants.len() > 1
            && kind != VariantKind::Original
            && ui
                .small_button(if armed { "✕?" } else { "✕" })
                .on_hover_text(if armed {
                    tr(lang, "Click again to delete this variant — it cannot be brought back (Ctrl+Z does not cross variants)")
                } else {
                    tr(lang, "Delete this variant")
                })
                .clicked()
        {
            act = Some(VariantAction::Delete(i));
        }
        #[cfg(test)]
        if self.variants.len() > 1 && kind != VariantKind::Original {
            drawn.push(if armed { "✕?" } else { "✕" });
        }
        #[cfg(test)]
        if surface == VariantSurface::List {
            self.edit_list_actions.push(format!("{i}:{}", drawn.join("")));
        }
        act
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_tone_wb(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let mut changed = false;

        // DERIVED from the registry like the five section dots in
        // `develop_panel` (R25 P8 closed the last two hand-written ones). This
        // section holds BOTH the tone family and the white-balance one — the
        // panel groups them, the registry does not — so its ● is the OR, the
        // same shape the Detail and Lens sections already use for their own
        // pairs.
        let tone_active = CONTROL_FAMILIES
            .iter()
            .filter(|f| f.name == "tone" || f.name == "white_balance")
            .any(|f| family_is_active(f, &self.recipe));

        ui.add_space(SPACE_MD); // same section fence as every sibling
        // #14b: the FIRST of the panel's group captions. The two existing group
        // fences (before Detail, before Local Masks) marked boundaries without
        // ever saying what they divided; the five groups the panel actually has
        // are AI (its own panel above) → tone & colour → detail & lens → local
        // & pixel → versions & export. Captions only: no section moved, no
        // fence rhythm changed.
        group_caption(ui, tr(lang, "Tone & Colour"));
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
                // Lightroom's Basic-panel order inside Presence is Texture →
                // Clarity → Dehaze, and the engine runs them in that order too
                // (R25 B2 put the global texture stage between clarity and
                // saturation). The label reuses the existing per-mask
                // 「Texture / 纹理」 entry — same operator, same word.
                changed |= Self::slider(ui, lang, tr(lang, "Texture"), &mut r.texture, -100.0, 100.0, 0.0);
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
                // 359.9, not 360: the recipe normalizes hue by rem_euclid(360),
                // so a drag to the very end snapped the knob back to 0 — same
                // colour, but the knob teleported under the pointer (L15-3).
                wheel_changed |= Self::slider(ui, lang, tr(lang, "Hue"), &mut hue, 0.0, 359.9, 0.0);
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
        group_caption(ui, tr(lang, "Detail & Lens")); // #14b, group 2 of 4 here
        egui::CollapsingHeader::new(section_title(tr(lang, "Detail"), detail_active))
            .id_salt("sec_detail")
            .default_open(false)
            .show(ui, |ui| {
                {
                    // The SAME disclosure line the Effects section uses (R25
                    // B3): eight of the eleven controls here move a number and
                    // no pixel in this app, and every one of them says so on
                    // the head line of its own tooltip.
                    let carried = tr(lang, "Carried to Lightroom, not rendered here");
                    let r = &mut self.recipe;
                    changed |= Self::slider(ui, lang, tr(lang, "Sharpening"), &mut r.sharpening, 0.0, 150.0, 0.0);
                    // Lightroom's radius band is 0.5..3.0; 0 is our "absent",
                    // so the track starts there. `Fine` (0.1 snap, one shown
                    // decimal), NOT the width rule's `Frac`: the sidecar key
                    // is written to ONE decimal (`+1.0`), so a 0.01 track
                    // would show a number the file cannot carry and the value
                    // would change under the user on the next read.
                    changed |= Self::slider_impl(ui, lang, tr(lang, "Sharpen radius"), &mut r.sharpen_radius, 0.0, 3.0, 0.0, SliderFeel::Fine, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Sharpen detail"), &mut r.sharpen_detail, 0.0, 100.0, 0.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Sharpen masking"), &mut r.sharpen_mask, 0.0, 100.0, 0.0, carried);
                    ui.add_space(SPACE_SM);
                    changed |=
                        Self::slider(ui, lang, tr(lang, "Noise Reduction"), &mut r.noise_reduction, 0.0, 100.0, 0.0);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Noise detail"), &mut r.nr_detail, 0.0, 100.0, 0.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Noise contrast"), &mut r.nr_contrast, 0.0, 100.0, 0.0, carried);
                    ui.add_space(SPACE_SM);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Colour noise reduction"), &mut r.color_nr, 0.0, 100.0, 0.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Colour noise detail"), &mut r.color_nr_detail, 0.0, 100.0, 0.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Colour noise smoothness"), &mut r.color_nr_smooth, 0.0, 100.0, 0.0, carried);
                }
                // AI denoise as an ACTIVE op: run now, see it on canvas —
                // export-time denoise (the Export section toggle) stays for
                // batch/full-res workflows, but nobody should have to export
                // to find out what the denoiser does.
                ui.add_space(SPACE_SM);
                ui.horizontal(|ui| {
                    // CAPABILITY, not just state — the same rule the two
                    // segmentation buttons follow one panel down (R24 batch 2).
                    // Without `python/denoise.py` on disk this click can only
                    // end in `denoise.rs`'s English "denoise sidecar not found
                    // at …" landing in a status line the rest of the window
                    // renders in the user's language, so the button says so up
                    // front instead of spending the click to find out.
                    let has_helper = denoise_helper_available();
                    let ready = self.src_path.is_some() && !self.busy && has_helper;
                    let missing = tr(lang,
                        "this build did not ship the python sidecar — run Autoshop from the project directory, or point AUTOSHOP_DENOISE_SCRIPT at python/denoise.py",
                    );
                    if ui
                        .add_enabled(ready, egui::Button::new(tr(lang, "🤖 AI Denoise now")))
                        // 🤖 + the cross-reference line (#4): this verb stays
                        // beside Noise Reduction on purpose, so its tooltip is
                        // where it says the rest of the AI moved to. On the arm
                        // that CANNOT run, the tooltip is about the missing
                        // sidecar and nothing else — same discipline as the
                        // segmentation pair.
                        .on_hover_text(if has_helper { ai_xref(lang, tr(lang,
                            "Run the SCUNet GPU sidecar on this variant's pixels and show the result on canvas \
                             (undoable — bakes a clean base into the current variant; the develop sliders keep \
                             applying on top; first run downloads the model)",
                        )) } else { missing.to_string() })
                        .clicked()
                    {
                        self.start_ai_denoise();
                    }
                    // Enabled for BAKED sources too (L15-4, the heal-gate
                    // rule): the engine honours the flag on both source
                    // types (denoise_active), and the RAW-only gate left a
                    // high-res baked TIFF no way to opt out of the ≤2048px
                    // working copy.
                    // The label names its VERB (R22 #16): four different
                    // checkboxes in three panels all said just 「Full-res」, so a
                    // support answer ("tick Full-res") pointed at four controls
                    // with different gates and different costs.
                    ui.checkbox(&mut self.denoise_fullres, tr(lang, "Full-res denoise"))
                        .on_hover_text(tr(lang,
                            "Denoise at full resolution (the full-sensor develop for a RAW, the image itself for a \
                             baked source; slow) — off = a ≤2048px working copy for a quick on-canvas result",
                        ));
                });
            });
        changed
    }

    /// 效果 — the nine `Tier::CarriedOnly` globals (R25 B2): Lightroom's
    /// post-crop vignette and film grain.
    ///
    /// Every slider here moves a number and NO pixel in this app, which
    /// `ARCHITECTURE.md` calls the worst kind of bug there is — so each one
    /// says so in its own tooltip, on the head line, before the drag grammar.
    /// The values are real and they are not decoration: they round-trip
    /// through the sidecar, so a Lightroom edit survives an Autoshop save
    /// instead of being stripped by the merge.
    fn dev_effects(&mut self, ui: &mut egui::Ui, effects_active: bool) -> bool {
        let lang = self.lang;
        let mut changed = false;

        ui.add_space(SPACE_MD);
        // ONE disclosure line, shared by all nine (see `slider_hinted`: the
        // helpers return `bool`, so a caller cannot chain its own tooltip, and
        // two stacked bubbles over one widget is not a disclosure).
        let carried = tr(lang, "Carried to Lightroom, not rendered here");
        egui::CollapsingHeader::new(section_title(tr(lang, "Effects"), effects_active))
            .id_salt("sec_effects")
            .default_open(false)
            .show(ui, |ui| {
                let r = &mut self.recipe;
                ui.label(egui::RichText::new(tr(lang, "Post-crop vignetting")).weak().small());
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Vignette amount"), &mut r.post_crop_vignette, -100.0, 100.0, 0.0, carried);
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Midpoint"), &mut r.post_crop_vignette_mid, 0.0, 100.0, 0.0, carried);
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Vignette feather"), &mut r.post_crop_vignette_feather, 0.0, 100.0, 0.0, carried);
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Vignette roundness"), &mut r.post_crop_vignette_round, -100.0, 100.0, 0.0, carried);
                // An operator INDEX (1/2/3), not a band: the ≥20-width rule in
                // `slider_hinted` would give this 0.01 steps and two decimals,
                // which is not a thing Adobe's Style can be.
                changed |= Self::slider_impl(ui, lang, tr(lang, "Vignette style"), &mut r.post_crop_vignette_style, 0.0, 3.0, 0.0, SliderFeel::Int, carried);
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Vignette highlights"), &mut r.post_crop_vignette_hl, 0.0, 100.0, 0.0, carried);
                ui.add_space(SPACE_SM);
                ui.label(egui::RichText::new(tr(lang, "Grain")).weak().small());
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Grain amount"), &mut r.grain, 0.0, 100.0, 0.0, carried);
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Grain size"), &mut r.grain_size, 0.0, 100.0, 0.0, carried);
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Grain roughness"), &mut r.grain_rough, 0.0, 100.0, 0.0, carried);
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
        // Field set: the registry's `lens` family (the section's two manual
        // sliders — `lens_vignette_mid` is exempt, and the reason now lives
        // with the exemption in `catalogue::DOT_EXEMPT`; same rule as
        // `exp_quality` in dev_export) PLUS the in-camera profile's two
        // rendered components, which are not registry rows of their own:
        // `lens_profile` is one engine-only carrier and belongs to no family.
        // R25 B3 added `lens_effects` — the manual CA pair (rendered), the
        // auto-CA switch and the six de-fringe keys (carried). Same OR as the
        // Detail section above, same reason.
        let lens_active = CONTROL_FAMILIES
            .iter()
            .filter(|f| f.name == "lens" || f.name == "lens_effects")
            .any(|f| family_is_active(f, &self.recipe))
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
                // 「Lens vignetting」, not 「Vignette」 (R25 B2): the Effects
                // section above now carries Lightroom's POST-CROP vignette,
                // and two sections labelled 暗角 with different operators
                // behind them is a name collision, not a shorthand. This one
                // is a falloff CORRECTION in linear light before any tonal
                // work; that one is a creative darkening after the crop.
                changed |= Self::slider(ui, lang, tr(lang, "Lens vignetting"), &mut self.recipe.lens_vignette, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Midpoint"), &mut self.recipe.lens_vignette_mid, 0.0, 100.0, 50.0);
                changed |= Self::slider(ui, lang, tr(lang, "Distortion"), &mut self.recipe.lens_distortion, -100.0, 100.0, 0.0);
                // --- manual lateral CA (R25 B3) -------------------------
                // RENDERED, so NO disclosure line: these two fold onto the
                // very per-channel radius LUT the profile CA above uses
                // (`render::geometry_profile`), and the preview, the export
                // and the sidecar all agree. The pair sits under the
                // distortion slider because it is the same kind of thing —
                // an optical defect, not a mood.
                ui.add_space(SPACE_SM);
                ui.label(egui::RichText::new(tr(lang, "Chromatic aberration (manual)")).weak().small());
                {
                    let r = &mut self.recipe;
                    changed |= Self::slider(ui, lang, tr(lang, "Red / cyan"), &mut r.ca_r, -100.0, 100.0, 0.0);
                    changed |= Self::slider(ui, lang, tr(lang, "Blue / yellow"), &mut r.ca_b, -100.0, 100.0, 0.0);
                }
                // …and the CARRIED half of the same panel: Adobe's auto
                // switch and the whole de-fringe block. Every one of these
                // says on its own tooltip that it moves no pixel here.
                let carried = tr(lang, "Carried to Lightroom, not rendered here");
                if ui
                    .checkbox(&mut self.recipe.auto_lateral_ca, tr(lang, "Auto lateral CA"))
                    .on_hover_text(carried)
                    .changed()
                {
                    changed = true;
                }
                ui.add_space(SPACE_SM);
                ui.label(egui::RichText::new(tr(lang, "Defringe")).weak().small());
                {
                    let r = &mut self.recipe;
                    // Adobe's own bands and Adobe's own neutrals: the amounts
                    // rest at 0, the hue windows at 30/70 and 40/60 (this is
                    // the one block in the recipe whose neutral is not zero,
                    // and the reset target has to say so or double-click
                    // would invent a 0..0 window).
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Purple amount"), &mut r.defringe_purple, 0.0, 20.0, 0.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Purple hue low"), &mut r.defringe_purple_lo, 0.0, 100.0, 30.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Purple hue high"), &mut r.defringe_purple_hi, 0.0, 100.0, 70.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Green amount"), &mut r.defringe_green, 0.0, 20.0, 0.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Green hue low"), &mut r.defringe_green_lo, 0.0, 100.0, 40.0, carried);
                    changed |= Self::slider_hinted(ui, lang, tr(lang, "Green hue high"), &mut r.defringe_green_hi, 0.0, 100.0, 60.0, carried);
                }
                ui.add_space(SPACE_SM);
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Vignette: positive brightens the corners (compensates falloff), negative darkens; a radial gain in linear light. Distortion: positive fixes barrel (wide-angle bulge), negative fixes pincushion (tele pinch); auto-scales to fill the frame, and masks / brush still position on the corrected image. Preview / export / XMP match. Manual CA renders here too; the auto-CA switch and de-fringe are carried to Lightroom without being rendered.",
                    ))
                    .weak()
                    .small(),
                );
            });
        changed
    }

    /// 变换 — Lightroom's Transform (Upright / Perspective) and Camera
    /// Calibration blocks, the first members of `Tier::PassThrough` (R25 B4).
    ///
    /// **READ-ONLY, and that is the design, not a shortcut.** Pass-through
    /// means we never interpret these values: no band, no clamp, no neutral,
    /// no idea what they do to a pixel. A slider needs all four. Offering one
    /// would say "this app understands your Upright correction" about sixteen
    /// strings it copies between two files without reading — and the app
    /// already has a rule for a control that moves a number and no pixel
    /// (`ARCHITECTURE.md`: "the worst kind of bug here"). What the section
    /// owes the photographer is the OPPOSITE of a slider: proof the values
    /// are still there, and one sentence saying we do not touch them.
    ///
    /// Absent from the recipe ⇒ absent from the panel: a heading over an
    /// empty list is a promise about a file that never had one.
    ///
    /// The rows show Adobe's own `crs:` property names, not friendly labels —
    /// the honest spelling for something we cannot describe in our own words.
    /// `CameraProfile` is the exception, because its VALUE is a name the
    /// photographer chose in Lightroom's profile browser rather than an
    /// internal of the Upright solver.
    fn dev_transform(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        if self.recipe.passthrough.is_empty() {
            return false;
        }
        ui.add_space(SPACE_MD);
        // BOTH blocks in the heading: the section carries Lightroom's
        // Transform panel AND its Calibration panel, and naming only the first
        // would send someone looking for their camera profile in the wrong
        // place. No ● — the dot means "an adjustment is active but collapsed",
        // and there is no adjustment here to be active.
        egui::CollapsingHeader::new(format!(
            "{} / {}",
            tr(lang, "Transform"),
            tr(lang, "Calibration")
        ))
            .id_salt("sec_transform")
            .default_open(false)
            .show(ui, |ui| {
                // In PASSTHROUGH_CRS order (Adobe's own grouping), which is
                // also the order the writer emits — one order everywhere, so
                // the panel and the sidecar read the same way.
                let rows = |ui: &mut egui::Ui, caption: &str, keys: &[&str]| {
                    let present: Vec<(&str, &str)> = keys
                        .iter()
                        .filter_map(|k| {
                            self.recipe.passthrough.get(*k).map(|v| (*k, v.as_str()))
                        })
                        .collect();
                    if present.is_empty() {
                        return;
                    }
                    ui.label(egui::RichText::new(caption).weak().small());
                    for (key, value) in present {
                        ui.horizontal(|ui| {
                            let label = if key == "CameraProfile" {
                                tr(lang, "Camera profile").to_string()
                            } else {
                                format!("crs:{key}")
                            };
                            ui.label(egui::RichText::new(label).small());
                            ui.label(egui::RichText::new(value).small().strong());
                        });
                    }
                };
                // Partitioned by the KEY, not by a hard-coded midpoint: the
                // two groups are complements, so a seventeenth key lands in
                // one of them instead of falling off the end of a `split_at`.
                let keys = autoshop::xmp::PASSTHROUGH_CRS;
                let perspective: Vec<&str> =
                    keys.iter().copied().filter(|k| k.starts_with("Perspective")).collect();
                let calibration: Vec<&str> =
                    keys.iter().copied().filter(|k| !k.starts_with("Perspective")).collect();
                rows(ui, tr(lang, "Perspective correction"), &perspective);
                rows(ui, tr(lang, "Camera calibration"), &calibration);
                ui.add_space(SPACE_SM);
                ui.label(
                    egui::RichText::new(tr(
                        lang,
                        "Carried through to the sidecar unchanged; Autoshop never interprets these",
                    ))
                    .weak()
                    .small(),
                );
            });
        // Nothing here can change the recipe — that is the whole point.
        false
    }

    /// One develop-panel section — body extracted verbatim from
    /// develop_panel (round-12 decomposition; spacing included).
    fn dev_crop(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let mut changed = false;

        // --- 裁剪 + 拉直: recipe.crop / straighten_deg (export + XMP paths) ---
        ui.add_space(SPACE_MD);
        // The `framing` family IS this section — crop rectangle plus
        // straighten angle — so the dot is read straight off the registry
        // (R25 P8, the same derivation as every other section's).
        let crop_active = CONTROL_FAMILIES
            .iter()
            .filter(|f| f.name == "framing")
            .any(|f| family_is_active(f, &self.recipe));
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
                        self.set_crop_mode(on);
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

    /// Does the Local Masks header earn its ●? Exactly the rule the mask ROWS
    /// below use — [`mask_active`], the engine's own non-neutrality test plus the
    /// eye — applied over the list. The header used to carry `n_masks > 0`
    /// instead (R22 #16), which lit the dot for a list of muted or parked masks
    /// and said nothing the「Local Masks ({n})」count did not already say.
    pub(crate) fn masks_section_active(&self) -> bool {
        self.recipe.masks.iter().any(mask_active)
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
        // #14b, group 3: masks and the pixel-level tools they reach.
        group_caption(ui, tr(lang, "Local & Pixel"));
        // --- 局部调整: manual masks — the SAME recipe.masks the AI writes -----
        let n_masks = self.recipe.masks.len();
        let n_masks_s = n_masks.to_string();
        egui::CollapsingHeader::new(section_title(
            &trf(lang, "Local Masks ({n})", &[("n", &n_masks_s)]),
            self.masks_section_active(),
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
                    .on_hover_text(tr(lang, "Paint a free-form mask (drag the 「Brush size」 slider, or press [ / ]); 「Apply」 bakes it into a new mask"))
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
                // #9: the radius belongs AT THE TOOL. This is the same
                // `self.brush` the Retouch panel's slider drives (and the
                // `[` / `]` keys) — one session block covers both arms, the
                // 「＋ Brush」 above and 「🖌 Edit raster」 down in the mask row.
                // Outside the row closure: that closure holds `&mut self`.
                self.brush_size_slider(ui);
            }
            // --- AI segmentation → bitmap masks (gap batch A②) ---------------
            ui.horizontal(|ui| {
                // CAPABILITY, not just state: without the python sidecar on
                // disk these two can only fail, so the button says so up front
                // instead of spending the click on a "not found at …" toast.
                let has_helper = segment_helper_available();
                let can_seg = !self.busy && self.base_preview.is_some() && has_helper;
                let missing = tr(lang,
                    "this build did not ship the python sidecar — run Autoshop from the project directory, or point AUTOSHOP_SEGMENT_SCRIPT at python/segment.py",
                );
                // 🤖 on BOTH (#4): one prefix marks every AI verb app-wide, and
                // 「☁ AI select sky」 was the lone exception — a weather glyph
                // where its twin one row over already wore the robot. The
                // tooltip carries the AI-panel cross-reference, but only on the
                // arm that CAN run: a missing-sidecar message must stay about
                // the missing sidecar.
                if ui
                    .add_enabled(can_seg, egui::Button::new(tr(lang, "🤖 AI select subject")))
                    // R29 B4: the model, its dependencies and its download size
                    // all changed with the user's ruling of 2026-08-21, and the
                    // FALLBACK is named because a machine without torchvision
                    // gets a materially softer mask and would otherwise have no
                    // way to know which model drew it.
                    .on_hover_text(if has_helper { ai_xref(lang, tr(lang,
                        "BiRefNet salient-subject segmentation → bitmap mask (python sidecar: pip install torchvision timm einops; first run auto-downloads a ~444MB model; without them it falls back to U²-Net / pip install rembg, whose edges are softer)",
                    )) } else { missing.to_string() })
                    .clicked()
                {
                    self.start_segment("subject", "Subject");
                }
                if ui
                    .add_enabled(can_seg, egui::Button::new(tr(lang, "🤖 AI select sky")))
                    .on_hover_text(if has_helper { ai_xref(lang, tr(lang,
                        "OneFormer-ADE20K sky segmentation → bitmap mask (python sidecar: pip install transformers; first run auto-downloads a ~880MB model)",
                    )) } else { missing.to_string() })
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
                            MaskGeometry::Brush { .. } => tr(self.lang, "Brush"),
                            // Named "AI" and never "Sky"/"Subject": the row
                            // must not read as Lightroom's own mask. What it
                            // draws is our segmenter's re-derivation, and the
                            // overlay badge plus the import line carry the
                            // rest of that sentence (R27 Batch-5).
                            MaskGeometry::AiMask { .. } => tr(self.lang, "AI"),
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
                        // (util::mask_active → render::engine_active): a parked
                        // mask looks parked, a working one shows ● — a 64-row
                        // list where the two were indistinguishable was a real
                        // navigation cost. The SECTION header reads the same
                        // helper over the whole list (masks_section_active), so
                        // the two dots cannot drift apart again (R22 #16).
                        let active = mask_active(m);
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
                        // Neither a raster nor a brush group has drag-to-place
                        // geometry — no 重画 for either.
                        // An AI mask has no drag-to-place geometry either:
                        // its reference point is a MODEL PROMPT, so "redraw"
                        // would mean re-running the segmenter, not moving a
                        // shape — a different action from this button's.
                        MaskGeometry::Bitmap { .. }
                        | MaskGeometry::Brush { .. }
                        | MaskGeometry::AiMask { .. } => None,
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
                    // ONE view switch, not a property of this row (R22 #16): it
                    // is the same flag the O key toggles, and
                    // `refresh_mask_overlay` (canvas.rs) always draws whichever
                    // mask is hovered-or-selected. Sitting in the selected-mask
                    // row is right — that IS the mask it draws — but 「Overlay」
                    // beside 「↻ Redraw」 and the ⬆/⬇ order buttons read as
                    // per-mask state that would be remembered per mask. The
                    // label now says what it is, in the same words the F1 sheet
                    // uses for the O key ("Toggle mask overlay").
                    if ui
                        .checkbox(&mut self.show_mask_overlay, tr(lang, "Show mask overlay"))
                        .on_hover_text(tr(lang, "One view switch shared by every mask (shortcut O): shows the hovered-or-selected mask's actual coverage as a red semi-transparent overlay (geometry × range × strength)"))
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
                    // -179.9: clamp_geometry normalizes to (-180, 180], so a
                    // drag to the very left snapped the knob to +180 — the
                    // ellipse never moved, the knob teleported (L15-6; the
                    // hue-slider rule).
                    geo_ch |= Self::slider(
                        ui,
                        lang,
                        tr(lang, "Angle"),
                        angle,
                        -179.9,
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
                                MaskGeometry::Brush { .. } => tr(lang, "Brush"),
                                MaskGeometry::AiMask { .. } => tr(lang, "AI"),
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
                // Lightroom's own three groups for a mask's adjustments (user
                // decision 8 of the 17-feedback triage): Tone / Detail /
                // Color, weak captions in the Color-Grading "All regions"
                // style. Twelve sliders in one unbroken column is what the
                // grouping replaces — the same reason the global panel is
                // sectioned. `Tone` is our own caption for LR's "Light" group:
                // the key "Light" already means the light THEME.
                let group = |ui: &mut egui::Ui, title: &str| {
                    ui.add_space(SPACE_XS);
                    ui.separator();
                    ui.label(egui::RichText::new(title).weak().small());
                };
                group(ui, tr(lang, "Tone"));
                changed |= Self::slider(ui, lang, tr(lang, "Exposure"), &mut m.exposure_ev, -5.0, 5.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Contrast"), &mut m.contrast, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Highlights"), &mut m.highlights, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Shadows"), &mut m.shadows, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Whites"), &mut m.whites, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Blacks"), &mut m.blacks, -100.0, 100.0, 0.0);
                // Texture / Clarity / Dehaze come out of the old
                // 「More (XMP/Lightroom only)」 fold: R22-3 put all three ON the
                // engine (render::apply_masks), so that title became a lie and
                // the fold merely hid three working sliders one level deep.
                group(ui, tr(lang, "Detail"));
                changed |= Self::slider(ui, lang, tr(lang, "Texture"), &mut m.texture, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Clarity"), &mut m.clarity, -100.0, 100.0, 0.0);
                changed |= Self::slider(ui, lang, tr(lang, "Dehaze"), &mut m.dehaze, -100.0, 100.0, 0.0);
                // R23-1b. SIGNED, unlike the global 0..150 Sharpening: this is
                // ACR's local band, and its negative half (soften) is what
                // throws a background back — the tooltip says so, because the
                // two controls share a name and not a range.
                changed |= Self::slider_hinted(
                    ui,
                    lang,
                    tr(lang, "Sharpness"),
                    &mut m.sharpness,
                    -100.0,
                    100.0,
                    0.0,
                    tr(lang, "Sharpens inside the mask when positive and SOFTENS when negative (the global 「Sharpening」 has no negative half). Same radius as the global one."),
                );
                changed |= Self::slider(ui, lang, tr(lang, "Noise Reduction"), &mut m.noise_reduction, 0.0, 100.0, 0.0);
                group(ui, tr(lang, "Color"));
                changed |= Self::slider(ui, lang, tr(lang, "Saturation"), &mut m.saturation, -100.0, 100.0, 0.0);
                // R23-1b: a hue ROTATION of everything the mask covers — the
                // one colour move the global 8-band mixer cannot make, because
                // that one acts on a band everywhere in the frame.
                changed |= Self::slider_hinted(
                    ui,
                    lang,
                    tr(lang, "Hue shift"),
                    &mut m.hue,
                    -100.0,
                    100.0,
                    0.0,
                    tr(lang, "Rotates every colour inside the mask (±100 = ±30°) — unlike the global color mixer, which moves one color band across the whole frame."),
                );
                // Engine-rendered since batch #2-B (render.rs apply_masks
                // mirrors the global WB model inside the mask) — live in the
                // preview like the tone sliders above. The ±100 pair is a
                // RELATIVE gel around a fixed anchor, a different axis from
                // the global 「Temp (K)」 absolute Kelvin — two controls named
                // "temp" that answer different questions, so the tooltip
                // carries the engine's OWN conversion (render::
                // local_temp_to_kelvin, not a number retyped here) for the
                // value currently on the slider.
                let temp_k = format!(
                    "{:.0}",
                    autoshop::render::local_temp_to_kelvin(m.temperature)
                );
                let temp_hint = trf(
                    lang,
                    "A RELATIVE warm/cool shift (±100) around a fixed 5500 K anchor, not absolute Kelvin: this value renders like ≈ {k} K. The global 「Temp (K)」 is absolute and anchored at this photo's as-shot value — a different axis.",
                    &[("k", &temp_k)],
                );
                changed |= Self::slider_hinted(ui, lang, tr(lang, "Temp shift"), &mut m.temperature, -100.0, 100.0, 0.0, &temp_hint);
                changed |= Self::slider_hinted(
                    ui,
                    lang,
                    tr(lang, "Tint shift"),
                    &mut m.tint,
                    -100.0,
                    100.0,
                    0.0,
                    tr(lang, "A RELATIVE green/magenta shift (±100) inside the mask — positive goes magenta. Unlike the global 「Tint」 it is not solved against the photo's as-shot tint."),
                );
                // Reverse-fit recolour gains: per-channel linear gains no
                // classic-ACR key can express (recipe.rs `color_gains`), so
                // they render in-app and vanish from the sidecar. Not a
                // slider — a disclosure plus the one action that makes sense
                // on a value the user never typed.
                if m.color_gains.is_some() {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(tr(lang, "carries reverse-fit recolour (not exported to XMP)"))
                                .weak()
                                .small(),
                        );
                        if ui
                            .small_button(tr(lang, "↺ Clear"))
                            .on_hover_text(tr(lang, "Drop this mask's per-channel recolour gains (one Ctrl+Z to undo)"))
                            .clicked()
                        {
                            m.color_gains = None;
                            changed = true;
                        }
                    });
                }
                // R25 P6 — the FOURTH group under Lightroom's three: this
                // mask's own point curves (`crs:MainCurve` / `RedCurve` /
                // `GreenCurve` / `BlueCurve`). The very editor the global
                // Curves section uses, pointed at this mask, so the plot, the
                // click/drag/drag-out gestures and the engine-faithful line
                // stay ONE implementation — a second copy is where the
                // preview line drifts from `render::curve_lut`.
                //
                // Last in the section on purpose: it is the only control here
                // that is not a slider, and it is the tallest.
                group(ui, tr(lang, "Curve"));
                changed |= self.curve_editor_for(ui, CurveTarget::Mask(i));
            } else if n_masks == 0 {
                ui.label(
                    egui::RichText::new(tr(lang, "Lightroom-style local adjustments: add a gradient to darken the sky, a radial to brighten the subject. AI Analyze also writes to this list."))
                        .weak()
                        .small(),
                );
            } else {
                // Masks exist, none is selected — one click on the selected
                // row (it toggles) lands here, and the section then showed a
                // list with no adjustments under it and no reason why.
                ui.add_space(SPACE_XS);
                ui.label(
                    egui::RichText::new(tr(lang, "Select a mask above to edit its adjustments"))
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
        // #14b, group 4 — DELIVERY (versions + export): what leaves the app,
        // as opposed to what is being edited. This boundary gets the same
        // fence + hairline + breather rhythm the other two group fences use
        // (Detail, Local Masks) — the rhythm is completed here, not changed:
        // Versions previously opened with a bare sibling-section fence while
        // being the head of a different KIND of group.
        ui.add_space(SPACE_MD);
        ui.separator();
        ui.add_space(SPACE_XS);
        group_caption(ui, tr(lang, "Versions & Export"));
        let n_ver = self.versions.len();
        let n_ver_s = n_ver.to_string();
        // Deferred like the strip's, and dispatched through the same owner
        // after the section's closures release `self` (R24-5).
        let mut act: Option<VariantAction> = None;
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
                // R24-4 (#1, phase 2 — minimal): ONE place that answers
                // "what edit states does this photo have". A photo is one
                // negative + N variant CARDS + one version history, so the
                // cards are listed first and the numbered snapshots under
                // them, in the vocabulary the data model now uses: the
                // taxonomy's binary (parametric vs pixel-state) and the
                // orthogonal "hangs off a baked master" attribute are the
                // two notes a row can carry (R24-1).
                //
                // R24-5: the rows are no longer a read-only view — each one
                // carries the card's own ＋ / ▣ / ✕, drawn by the SAME owner
                // the strip draws them with (`variant_card_buttons`), so the
                // two surfaces cannot come to mean different things. Clicking
                // a row switches to that card, exactly as clicking its
                // thumbnail does. Renaming stays on the strip on purpose: two
                // `TextEdit`s over one buffer fight for the caret every frame
                // (see `VariantSurface`).
                //
                // The buttons take a SECOND line rather than joining the
                // label row. A 320 px panel already holds a kind label, a
                // 「name」 and a vocabulary note; three more widgets on that
                // line is how a side panel starts growing per frame, which is
                // a defect this repo has shipped twice (R19, R22-3) and now
                // has width-stability tests for.
                let n_var = self.variants.len();
                let busy = self.busy;
                #[cfg(test)]
                {
                    self.edit_list_rows.clear();
                    self.edit_list_actions.clear();
                }
                if n_var > 0 {
                    let accent = self.theme.colors().accent_text;
                    ui.label(
                        egui::RichText::new(trf(
                            lang,
                            "Variants ({n})",
                            &[("n", &n_var.to_string())],
                        ))
                        .weak()
                        .small(),
                    );
                    let active_i = self.active;
                    for i in 0..n_var {
                        // Read the three facts the row shows BEFORE the layout
                        // closure borrows `self` (Variant is not Clone, and
                        // making it so for a display row would be the wrong
                        // fix — the card's texture handle rides on it).
                        let (kind, name, on_pixels) = {
                            let v = &self.variants[i];
                            (v.kind, v.name.clone(), v.origin.is_some())
                        };
                        #[cfg(test)]
                        self.edit_list_rows.push(format!("variant:{}", kind.store_str()));
                        let hit = ui
                            .horizontal(|ui| {
                                let label = egui::RichText::new(tr(lang, kind.label())).small();
                                ui.label(if i == active_i {
                                    label.strong().color(accent)
                                } else {
                                    label
                                });
                                if let Some(n) = &name {
                                    ui.label(egui::RichText::new(format!("「{n}」")).small());
                                }
                                if i == active_i {
                                    ui.label(
                                        egui::RichText::new(tr(lang, "· current"))
                                            .weak()
                                            .small(),
                                    );
                                }
                                if !kind.is_parametric() {
                                    ui.label(
                                        egui::RichText::new(tr(lang, "· pixel-state (no XMP)"))
                                            .weak()
                                            .small(),
                                    );
                                } else if on_pixels {
                                    ui.label(
                                        egui::RichText::new(tr(lang, "· on baked pixels"))
                                            .weak()
                                            .small(),
                                    );
                                }
                            })
                            .response
                            .interact(egui::Sense::click());
                        // The whole label row is the switch target — the same
                        // gesture the strip's thumbnail offers, on the row that
                        // names the card. A BACKGROUND row only: re-selecting
                        // the active card would run a switch (and its commit)
                        // for no change.
                        if i != active_i
                            && hit
                                .on_hover_text(tr(
                                    lang,
                                    "Click to switch to this variant (lossless)",
                                ))
                                .clicked()
                        {
                            act = Some(VariantAction::Switch(i));
                        }
                        // The card's own actions, one owner with the strip.
                        // Indented under the label so the two lines read as
                        // one row rather than as two entries.
                        let has_actions =
                            i == active_i || (n_var > 1 && kind != VariantKind::Original);
                        if has_actions {
                            ui.horizontal(|ui| {
                                ui.add_space(SPACE_LG);
                                if let Some(a) = self.variant_card_buttons(
                                    ui,
                                    i,
                                    i == active_i,
                                    busy,
                                    VariantSurface::List,
                                ) {
                                    act = Some(a);
                                }
                            });
                        }
                    }
                    ui.add_space(SPACE_XS);
                    ui.separator();
                }
                if ui
                    .button(tr(lang, "＋ Save as version"))
                    .on_hover_text(tr(lang, "Save all current develop parameters as a numbered snapshot (v<N>.recipe.json in this photo's develop store), reloadable anytime"))
                    .clicked()
                {
                    self.save_version();
                }
                // R24-2: a version row is 「v3 「日落偏暖」· 来自 ◭ 反推」 —
                // the number, the user's own name, and which VARIANT the
                // snapshot was taken from. The filter answers "show me only
                // this card's history"; rows it hides are COUNTED below, so
                // a shrinking list is never mistaken for lost versions.
                let active_id = self.active_variant().map(|v| v.id.clone()).unwrap_or_default();
                let active_kind = self.active_variant().map(|v| v.kind);
                if !self.versions.is_empty() {
                    ui.checkbox(&mut self.versions_current_only, tr(lang, "Only this variant"))
                        .on_hover_text(tr(
                            lang,
                            "Show only snapshots taken from the variant you are on. Versions with no recorded source (saved before this) are hidden while it is on.",
                        ));
                }
                let mut load: Option<u32> = None;
                let mut delete: Option<u32> = None;
                let mut rename: Option<u32> = None;
                let mut hidden = 0usize;
                // The list is cloned so the rows may borrow `self` mutably
                // (the rename buffer) — it is a handful of u32.
                let versions = self.versions.clone();
                let photo = self.src_path.clone();
                for n in versions {
                    let meta = self.version_meta.get(&n).cloned();
                    if self.versions_current_only
                        && !Self::version_is_from(meta.as_ref(), &active_id, active_kind)
                    {
                        hidden += 1;
                        continue;
                    }
                    #[cfg(test)]
                    self.edit_list_rows.push(format!("version:{n}"));
                    let named = meta.as_ref().and_then(|m| m.name.clone());
                    let editing = self
                        .version_name_buf
                        .as_ref()
                        .is_some_and(|(p, j, ..)| *j == n && Some(p) == photo.as_ref());
                    ui.horizontal(|ui| {
                        ui.label(format!("v{n}"));
                        let field_id = ui.make_persistent_id(("version_name", n));
                        if editing {
                            let resp = {
                                let buf = &mut self
                                    .version_name_buf
                                    .as_mut()
                                    .expect("checked just above")
                                    .3;
                                ui.add(
                                    egui::TextEdit::singleline(buf)
                                        .id(field_id)
                                        .desired_width(110.0)
                                        .hint_text(tr(lang, "Name")),
                                )
                            };
                            // The +1 boundary: this row's own lost-focus
                            // commit, the twin of the mask panel's (U10).
                            // Enter counts as losing focus in egui. Leaving
                            // the box also leaves EDIT MODE — a row that
                            // stayed a TextEdit forever would keep asking
                            // for focus below and never give it back.
                            if resp.lost_focus() {
                                self.commit_version_name_buf();
                                self.version_name_buf = None;
                            } else if !resp.has_focus() {
                                // The frame the box first appears in: the
                                // click that opened it landed on the LABEL
                                // that used to be here, so the caret has to
                                // be put here explicitly or the user's next
                                // keystroke goes nowhere.
                                resp.request_focus();
                            }
                        } else {
                            let shown = match &named {
                                Some(name) => format!("「{name}」"),
                                None => tr(lang, "Name…").to_string(),
                            };
                            let text = match &named {
                                Some(_) => egui::RichText::new(shown),
                                None => egui::RichText::new(shown).weak(),
                            };
                            if ui
                                .add(egui::Label::new(text).sense(egui::Sense::click()))
                                .on_hover_text(tr(lang, "Name this snapshot"))
                                .clicked()
                            {
                                rename = Some(n);
                            }
                        }
                        // Provenance, only when it was actually recorded —
                        // a version from before this existed says nothing
                        // rather than guessing.
                        if let Some(m) = &meta {
                            let from = m.from_kind.as_deref().and_then(VariantKind::from_store_str);
                            let note = match (from, m.origin.as_deref()) {
                                // `kind` by name, not `k`: the i18n audit's
                                // dynamic-key registry keys on the receiver
                                // spelling (`kind.label()`), which is how a
                                // new VariantKind without a zh pair is caught.
                                (Some(kind), _) => Some(trf(
                                    lang,
                                    "· from {kind}",
                                    &[("kind", tr(lang, kind.label()))],
                                )),
                                (None, Some(autoshop::store::VERSION_ORIGIN_AUTO)) => {
                                    Some(tr(lang, "· auto-archived").to_string())
                                }
                                _ => None,
                            };
                            if let Some(note) = note {
                                ui.label(egui::RichText::new(note).weak().small());
                            }
                        }
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
                if hidden > 0 {
                    ui.label(
                        egui::RichText::new(trf(
                            lang,
                            "{n} hidden — saved from another variant",
                            &[("n", &hidden.to_string())],
                        ))
                        .weak()
                        .small(),
                    );
                }
                if let Some(n) = rename
                    && let Some(p) = photo.clone()
                {
                    // Switching rows commits the previous one to ITS OWN
                    // (photo, number) first — the mask panel's cross-commit
                    // rule, made structural by keying on the number.
                    self.commit_version_name_buf();
                    let cur = self
                        .version_meta
                        .get(&n)
                        .and_then(|m| m.name.clone())
                        .unwrap_or_default();
                    self.version_name_buf = Some((p, n, cur.clone(), cur));
                }
                if let Some(n) = load {
                    self.load_version(n);
                }
                if let Some(n) = delete
                    && let Some(src) = self.src_path.clone()
                {
                    // A rename buffer aimed at the row being deleted dies
                    // WITH it (R24-2): committing it afterwards would write a
                    // name for a number that no longer exists and can never
                    // be re-issued — a record only the next delete would
                    // clean up.
                    if self.version_name_buf.as_ref().is_some_and(|(p, j, ..)| *j == n && *p == src)
                    {
                        self.version_name_buf = None;
                    }
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
        // The edit-state list's card action, performed through the SAME owner
        // the strip uses — after the section's closures have released `self`.
        if let Some(a) = act {
            self.dispatch_variant_action(a, ui.ctx());
        }
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
        // Field set: every setting this section owns EXCEPT `exp_quality` — the
        // deliberate omission (R22 #16 re-checked it after `exp_dest` joined in
        // R22-7). Quality reaches exactly one encoder (`jpeg_quality`,
        // export.rs), which is why `export_summary` prints "q95" only for JPEG;
        // and any format that consumes it is by definition `!= Tiff16`, so it has
        // already lit the dot. Listing quality could therefore only flag a TIFF
        // delivery whose bytes are identical either way. Same rule as
        // `lens_vignette_mid` in dev_lens.
        let export_active = self.exp_format != ExportFormat::Tiff16
            || self.exp_long_edge != 0
            || self.exp_sharpen != 0.0
            || self.exp_space != 0
            || self.exp_dest != ExportDest::OutFolder
            || self.save_denoise;
        egui::CollapsingHeader::new(section_title(tr(lang, "Export"), export_active))
            .id_salt("sec_export")
            .default_open(false)
            .show(ui, |ui| {
                // WHERE first (R22-7): the delivery target is now a setting
                // rather than a property of which toolbar button was pressed,
                // and it is the one whose wrong value sends the file somewhere
                // the user has to hunt for — so it leads the section instead of
                // hiding under the encoder dials.
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Destination"));
                    egui::ComboBox::from_id_salt("exp_dest")
                        .selected_text(tr(lang, self.exp_dest.label()))
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            for d in ExportDest::ALL {
                                ui.selectable_value(&mut self.exp_dest, d, tr(lang, d.label()));
                            }
                        });
                });
                // The RESOLVED target, spelled out: "./out" is relative to
                // whichever directory the app was launched from, and "last used
                // folder" says nothing about which folder that is.
                ui.label(
                    egui::RichText::new(match self.export_dest_dir() {
                        Some(d) => abs_display(&d),
                        None => tr(lang, "a save dialog opens on every export").to_string(),
                    })
                    .weak()
                    .small(),
                );
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
                // 「on export」 completes the pair the 🤖 prefix alone could not
                // separate (R22 #16): this checkbox and the Detail section's
                // 「🤖 AI Denoise now」 are two TIMINGS of one denoiser — this one
                // runs inside every full-resolution delivery and touches no
                // pixel until then, that one bakes a clean base into the current
                // variant immediately. Same-named twins in two panels made the
                // difference invisible until an export took minutes.
                ui.checkbox(&mut self.save_denoise, tr(lang, "🤖 AI Denoise on export")).on_hover_text(
                    ai_xref(lang, tr(lang, "SCUNet AI denoise before developing — high-ISO / astro (slow, GPU; needs the python sidecar). Batch render skips it.")),
                );
                ui.label(
                    egui::RichText::new(tr(lang, "Applied by 「Export」 in the toolbar (Ctrl+Shift+E, or Ctrl+E) and by 「Render selected」 in the library. The ▾ beside Export delivers one file to a path you pick without touching the Destination."))
                        .weak()
                        .small(),
                );
                // --- SF8-A: hand the Lightroom sidecar over ----------------
                // The XMP projection has always been written INSIDE the hashed
                // develop folder, which is the right home for state — but the
                // one thing users do with an XMP is give it to Lightroom, and
                // Lightroom only looks for it beside the photo. That left the
                // hand-off as "browse into %LOCALAPPDATA% yourself".
                ui.add_space(SPACE_MD);
                ui.separator();
                let raw = self.can_export_xmp_beside();
                let label = if self.xmp_beside_confirm {
                    tr(lang, "⚠ Overwrite the .xmp already there")
                } else {
                    tr(lang, "Export .xmp beside the photo")
                };
                let hint = if !raw {
                    // The disabled REASON, not a repeat of the label: only a
                    // camera RAW has the sidecar convention (store::
                    // lightroom_sidecar and pipeline::write_xmp draw the same
                    // line), so a baked PNG/TIFF's neighbouring .xmp belongs to
                    // some other program and must not be written over.
                    tr(lang, "RAW only — a baked PNG/TIFF has no Lightroom sidecar convention, so its neighbouring .xmp belongs to another program")
                } else if self.xmp_beside_confirm {
                    tr(lang, "A .xmp already sits beside this photo (Lightroom's own, or an earlier copy) — clicking again replaces it")
                } else {
                    tr(lang, "Copy this photo's stored Lightroom/ACR sidecar into the photo's own folder, where Lightroom reads it. Save the develop first — this delivers what is stored, not what is unsaved on the canvas.")
                };
                if ui
                    .add_enabled(raw && !self.busy, egui::Button::new(label))
                    .on_hover_text(hint)
                    .clicked()
                {
                    self.export_xmp_beside();
                }
            });
        false
    }
}
