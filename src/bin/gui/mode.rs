//! The mode-select page and the three modes' shells (design 2026-10-22, the
//! user's rulings of 2026-10-23).
//!
//! Every launch opens on this page, before the workspace: three cards —
//! Landscape, Portraits, Normal — and a click enters the workspace in that
//! mode. There is no "skip next time" switch (ruling 1); the remembered mode
//! only decides which card is highlighted. Inside the workspace the mode's
//! name sits at the left of the toolbar and returns here, with the open photo,
//! its edits and the library untouched.
//!
//! A mode is a GUI-layer preference and nothing more. The library and the CLI
//! never see it, no recipe stores it, and a photo opens the same way in every
//! mode. What a mode changes is which of the SHARED panels are drawn
//! ([`Shell`]): Landscape draws everything exactly as before this page
//! existed; Portraits is the full workspace with its own panel at the top of
//! the sidebar (`panels/portrait.rs`, v1.14.0); Normal draws its own four-group sidebar (`panels/normal.rs`) in place of
//! the shared panels, and hides the workspace's tools (design §4).

use super::*;

/// The three modes. The variant names are the persisted spelling in
/// `gui-prefs.ron` and the i18n keys (ruling 4): Landscape / Portraits /
/// Normal, 风景 / 人像 / 普通. A prefs file written before this field
/// existed decodes to [`Mode::Landscape`] through `Prefs`'s struct-level
/// `#[serde(default)]`, which is what every user had before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum Mode {
    #[default]
    Landscape,
    Portraits,
    Normal,
}

/// What a mode draws and how its verbs default — display switches only. The
/// panel code is shared; each switch is read at the one place it governs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shell {
    /// The reverse-fit's local-area switches sit in a fold of their own,
    /// closed by default (Portraits).
    pub(crate) zoned_folded: bool,
    /// The Open dialog lists rendered images (jpg, png, …) before RAWs.
    pub(crate) baked_first: bool,
    /// The sidebar is Normal's own four groups (`panels/normal.rs`: one-click
    /// look, fine-tune, noise, save) in place of the shared panels.
    pub(crate) one_click_sidebar: bool,
    /// The toolbar's whole row. Off = mode, open, undo, redo, compare and
    /// settings only (Normal): saving lives in the sidebar's Save group.
    pub(crate) full_toolbar: bool,
    /// The library panel's copy / paste / render rows. Off = open-folder and
    /// the thumbnails only; the batch runs from the Save group.
    pub(crate) library_batch_rows: bool,
    /// The canvas's tool marks: armed-tool hints, the selected mask's
    /// outline and badge, the zoom readout and the preview-size box (Fit and
    /// 1:1 stay in every mode).
    pub(crate) canvas_tool_marks: bool,
    /// The variant strip under the canvas.
    pub(crate) variant_strip: bool,
    /// The Portraits panel (`panels/portrait.rs`: people, skin, eyes,
    /// background) at the top of the sidebar, above the shared panels
    /// (Portraits).
    pub(crate) portrait_panel: bool,
}

impl Mode {
    pub(crate) const ALL: [Mode; 3] = [Mode::Landscape, Mode::Portraits, Mode::Normal];

    /// The switch table — the design's §4, one row per mode.
    pub(crate) fn shell(self) -> Shell {
        match self {
            Mode::Landscape => Shell {
                zoned_folded: false,
                baked_first: false,
                one_click_sidebar: false,
                full_toolbar: true,
                library_batch_rows: true,
                canvas_tool_marks: true,
                variant_strip: true,
                portrait_panel: false,
            },
            Mode::Portraits => Shell {
                zoned_folded: true,
                baked_first: false,
                one_click_sidebar: false,
                full_toolbar: true,
                library_batch_rows: true,
                canvas_tool_marks: true,
                variant_strip: true,
                portrait_panel: true,
            },
            Mode::Normal => Shell {
                zoned_folded: false,
                baked_first: true,
                one_click_sidebar: true,
                full_toolbar: false,
                library_batch_rows: false,
                canvas_tool_marks: false,
                variant_strip: false,
                portrait_panel: false,
            },
        }
    }

    /// The mode's name, in the session language.
    pub(crate) fn name(self, lang: Lang) -> &'static str {
        match self {
            Mode::Landscape => tr(lang, "Landscape"),
            Mode::Portraits => tr(lang, "Portraits"),
            Mode::Normal => tr(lang, "Normal"),
        }
    }

    /// The card's one line (the design's §2 table).
    fn blurb(self, lang: Lang) -> &'static str {
        match self {
            Mode::Landscape => tr(lang, "RAW first, the full reverse-fit: zones, sky, denoise, the Lightroom sidecar"),
            Mode::Portraits => tr(lang, "Faces first: skin tones, eyes, background blur, skin kept natural"),
            Mode::Normal => tr(lang, "Phone or jpg snapshots, a one-click reference look, no jargon"),
        }
    }

    /// Who the card is for (the design's §2 table).
    fn audience(self, lang: Lang) -> &'static str {
        match self {
            Mode::Landscape => tr(lang, "For: current users"),
            Mode::Portraits => tr(lang, "For: people who photograph people"),
            Mode::Normal => tr(lang, "For: anyone who does not shoot RAW"),
        }
    }

    /// What this version actually delivers, where the card's line promises
    /// more than the shell has yet. None since v1.14.0: Portraits has its
    /// face tools (`panels/portrait.rs`), and its v1.13.0 note — "the face
    /// tools come later" — would now be false.
    fn note(self, _lang: Lang) -> Option<&'static str> {
        None
    }
}

/// One card's text, laid before any card is drawn: the mode, its title, line,
/// audience and the optional note.
type LaidCard = (
    Mode,
    std::sync::Arc<egui::Galley>,
    std::sync::Arc<egui::Galley>,
    std::sync::Arc<egui::Galley>,
    Option<std::sync::Arc<egui::Galley>>,
);

/// The narrowest a card is laid side by side; below three of these the cards
/// stack.
pub(crate) const MODE_CARD_W_MIN: f32 = 240.0;
/// The widest a card grows on a wide window.
const MODE_CARD_W_MAX: f32 = 360.0;
/// The schematic's height inside a card.
const SCHEMATIC_H: f32 = 120.0;
/// The air between two cards.
const CARD_GAP: f32 = 24.0;

impl AutoShadeApp {
    /// The mode-select page: a title bar (on Windows the window has no other),
    /// then the three cards on the canvas bed. A click enters the workspace.
    pub(crate) fn mode_page(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        window_chrome::edge_resize(ctx);
        window_chrome::window_outline(ctx);
        let bar = egui::Frame::side_top_panel(&ctx.style())
            .inner_margin(egui::Margin::symmetric(8.0, SPACE_MD))
            .fill(self.theme.colors().bar_fill);
        egui::TopBottomPanel::top("top").frame(bar).show(ctx, |ui| {
            window_chrome::toolbar_row(ui, lang, |ui| {
                ui.label(egui::RichText::new("AutoShade").strong());
            });
        });
        let bed = egui::Frame::central_panel(&ctx.style()).fill(self.theme.colors().canvas_bed);
        let mut picked = None;
        egui::CentralPanel::default().frame(bed).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                picked = self.mode_cards(ui);
            });
        });
        if let Some(mode) = picked {
            self.choose_mode(mode);
        }
    }

    /// Enter the workspace in `mode`. The open photo, its edits and the
    /// library are not touched; an armed canvas tool is put down when the
    /// mode changes, since its section may not be drawn in the new one.
    pub(crate) fn choose_mode(&mut self, mode: Mode) {
        if mode != self.mode {
            self.disarm_tools();
            // The status line may still describe the other mode's tools
            // (a brush hint, a fit's report); Normal starts on its own words.
            if mode.shell().one_click_sidebar {
                self.status = tr(self.lang, "Pick a look, or match your own reference photos.").into();
            }
        }
        self.mode = mode;
        self.mode_chosen_this_launch = true;
        // Written at once (`update` flushes it), so the remembered card is
        // right even if the app does not get to exit cleanly.
        self.prefs_save_due = true;
    }

    /// Back to the mode page from the workspace — nothing else changes.
    pub(crate) fn return_to_mode_page(&mut self) {
        self.mode_chosen_this_launch = false;
    }

    /// The heading and the three cards; the card clicked this frame.
    fn mode_cards(&mut self, ui: &mut egui::Ui) -> Option<Mode> {
        let lang = self.lang;
        let avail = ui.available_width();
        let side_by_side = avail >= 3.0 * MODE_CARD_W_MIN + 2.0 * CARD_GAP;
        let card_w = if side_by_side {
            ((avail - 2.0 * CARD_GAP) / 3.0).min(MODE_CARD_W_MAX)
        } else {
            avail.min(MODE_CARD_W_MAX)
        }
        .floor();
        let pad = SPACE_LG + SPACE_MD;
        let wrap = (card_w - 2.0 * pad).max(1.0);
        let strong = ui.visuals().strong_text_color();
        let body = ui.visuals().text_color();
        let weak = ui.visuals().weak_text_color();
        let accent = self.theme.colors().accent_text;
        // Lay every card's text first, so the three share one height.
        let laid: Vec<LaidCard> = Mode::ALL
            .iter()
            .map(|&m| {
                let p = ui.painter();
                let title = p.layout(m.name(lang).to_owned(), egui::FontId::proportional(22.0), strong, wrap);
                let blurb = p.layout(m.blurb(lang).to_owned(), egui::FontId::proportional(14.0), body, wrap);
                let who = p.layout(m.audience(lang).to_owned(), egui::FontId::proportional(13.0), weak, wrap);
                let note = m.note(lang).map(|n| p.layout(n.to_owned(), egui::FontId::proportional(12.0), accent, wrap));
                (m, title, blurb, who, note)
            })
            .collect();
        let text_h = laid
            .iter()
            .map(|(_, t, b, w, n)| {
                t.size().y + SPACE_MD + b.size().y + SPACE_MD + w.size().y
                    + n.as_ref().map_or(0.0, |n| SPACE_MD + n.size().y)
            })
            .fold(0.0f32, f32::max);
        let card_h = (pad + SCHEMATIC_H + SPACE_LG + text_h + pad).ceil();

        let mut picked = None;
        #[cfg(test)]
        self.mode_card_rects.clear();
        ui.add_space(SPACE_LG * 3.0);
        ui.vertical_centered(|ui| {
            ui.heading(tr(lang, "Choose a mode"));
            ui.add_space(SPACE_SM);
            ui.label(
                egui::RichText::new(tr(
                    lang,
                    "Switch any time with the mode button at the left of the toolbar — the open photo stays open.",
                ))
                .weak(),
            );
        });
        ui.add_space(SPACE_LG * 2.0);
        let mut draw = |ui: &mut egui::Ui, (m, title, blurb, who, note): LaidCard| {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(card_w, card_h), egui::Sense::click());
            #[cfg(test)]
            self.mode_card_rects.push((m, rect));
            let remembered = m == self.mode;
            let c = self.theme.colors();
            let rise = ui.ctx().animate_bool_with_time(resp.id.with("lift"), resp.hovered(), MOTION_FADE_S);
            let p = ui.painter_at(rect.expand(12.0));
            if rise > 0.0 {
                let shadow = egui::epaint::Shadow {
                    offset: egui::vec2(0.0, 3.0 * rise),
                    blur: 12.0,
                    spread: 0.0,
                    color: egui::Color32::from_black_alpha((f32::from(c.shadow_alpha) * rise) as u8),
                };
                p.add(shadow.as_shape(rect, RADIUS_LG));
            }
            let stroke = if remembered {
                egui::Stroke::new(2.0, c.accent_fill)
            } else {
                egui::Stroke::new(1.0, c.card_stroke)
            };
            p.rect(rect, RADIUS_LG, c.card_fill, stroke);
            let art = egui::Rect::from_min_size(rect.min + egui::vec2(pad, pad), egui::vec2(wrap, SCHEMATIC_H));
            schematic(&p, m, art, c.accent_fill, body);
            let mut y = art.bottom() + SPACE_LG;
            let left = rect.left() + pad;
            if remembered {
                let badge = p.layout_no_wrap(tr(lang, "Last used").to_owned(), egui::FontId::proportional(12.0), accent);
                let at = egui::pos2(rect.right() - pad - badge.size().x, y + (title.size().y - badge.size().y) * 0.5);
                p.galley(at, badge, accent);
            }
            let h = title.size().y;
            p.galley(egui::pos2(left, y), title, strong);
            y += h + SPACE_MD;
            let h = blurb.size().y;
            p.galley(egui::pos2(left, y), blurb, body);
            y += h + SPACE_MD;
            let h = who.size().y;
            p.galley(egui::pos2(left, y), who, weak);
            y += h + SPACE_MD;
            if let Some(n) = note {
                p.galley(egui::pos2(left, y), n, accent);
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                picked = Some(m);
            }
        };
        if side_by_side {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = CARD_GAP;
                let total = 3.0 * card_w + 2.0 * CARD_GAP;
                ui.add_space(((avail - total) * 0.5).max(0.0));
                for l in laid {
                    draw(ui, l);
                }
            });
        } else {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = CARD_GAP;
                for l in laid {
                    draw(ui, l);
                }
            });
        }
        ui.add_space(SPACE_LG * 2.0);
        picked
    }
}

/// A small drawing per mode, from shapes alone (no image assets): mountains
/// under a sun with the sky/land zone line for Landscape, a head and
/// shoulders against soft background discs for Portraits, a phone with a
/// picture and a one-click dot for Normal.
fn schematic(p: &egui::Painter, mode: Mode, r: egui::Rect, accent: egui::Color32, ink: egui::Color32) {
    let line = egui::Stroke::new(1.5, ink.gamma_multiply(0.7));
    let soft = accent.gamma_multiply(0.25);
    let at = |x: f32, y: f32| egui::pos2(r.left() + x * r.width(), r.top() + y * r.height());
    p.rect_filled(r, RADIUS_MD, ink.gamma_multiply(0.06));
    match mode {
        Mode::Landscape => {
            p.circle_filled(at(0.78, 0.28), r.height() * 0.11, accent);
            p.add(egui::Shape::convex_polygon(
                vec![at(0.05, 0.92), at(0.35, 0.30), at(0.62, 0.92)],
                soft,
                line,
            ));
            p.add(egui::Shape::convex_polygon(
                vec![at(0.40, 0.92), at(0.66, 0.46), at(0.95, 0.92)],
                soft,
                line,
            ));
            // The sky/land zone line the zoned fit works across.
            p.add(egui::Shape::dashed_line(&[at(0.04, 0.58), at(0.96, 0.58)], line, 6.0, 4.0));
        }
        Mode::Portraits => {
            for (x, y, s) in [(0.18, 0.30, 0.16), (0.84, 0.40, 0.20), (0.70, 0.18, 0.10)] {
                p.circle_filled(at(x, y), r.height() * s, soft);
            }
            let head = at(0.5, 0.36);
            p.circle(head, r.height() * 0.17, ink.gamma_multiply(0.12), line);
            let shoulders: Vec<egui::Pos2> = (0..=16)
                .map(|i| {
                    let t = std::f32::consts::PI * i as f32 / 16.0;
                    at(0.5 - 0.26 * t.cos(), 0.95 - 0.36 * t.sin())
                })
                .collect();
            p.add(egui::Shape::convex_polygon(shoulders, ink.gamma_multiply(0.12), line));
        }
        Mode::Normal => {
            let phone = egui::Rect::from_center_size(r.center(), egui::vec2(r.height() * 0.5, r.height() * 0.86));
            p.rect(phone, RADIUS_LG, ink.gamma_multiply(0.08), line);
            let screen = phone.shrink2(egui::vec2(phone.width() * 0.1, phone.height() * 0.12));
            p.rect_filled(screen, RADIUS_SM, soft);
            p.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(screen.left(), screen.bottom()),
                    egui::pos2(screen.left() + screen.width() * 0.45, screen.top() + screen.height() * 0.45),
                    egui::pos2(screen.right(), screen.bottom()),
                ],
                accent.gamma_multiply(0.6),
                egui::Stroke::NONE,
            ));
            // The one-click dot beside the phone.
            p.circle_filled(egui::pos2(phone.right() + r.height() * 0.22, phone.center().y), r.height() * 0.09, accent);
        }
    }
}
