//! The side panel's three heading levels (2026-10-01, user: 「看不出哪些子类别
//! 是属于哪些大类别的」, 「这些分类线太丑了」).
//!
//! Panel title ([`panel_heading`]: AI, Develop, Retouch) → group
//! ([`group_caption`]: Tone & Colour, Detail & Lens, …) → fold
//! (`CollapsingHeader`), on the AI panel as on Develop. Each level is told
//! apart by type and by air, not by full-width hairlines: the old group fence
//! was a separator running edge to edge into the scroll bar over a weak grey
//! caption, and the AI panel's header was drawn at the same size as its own
//! folds, so its children read as its siblings.
//!
//! The gold is `hyperlink_color`, which `install_theme` sets to the theme's
//! `accent_text` — the text-size gold that passes contrast on both themes.

use super::*;

/// A first-level panel title: Heading type with a short gold rule under the
/// text. `lead` is drawn first on the same row (the AI panel's fold toggle);
/// the returned response is the title's own, sensing clicks so a collapsible
/// panel can toggle on it.
pub(crate) fn panel_heading(
    ui: &mut egui::Ui,
    title: &str,
    lead: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let accent = ui.visuals().hyperlink_color;
    let row = ui.horizontal(|ui| {
        lead(ui);
        ui.add(
            egui::Label::new(egui::RichText::new(title).heading().strong())
                .sense(egui::Sense::click()),
        )
    });
    let title_rect = row.inner.rect;
    let y = row.response.rect.bottom() + 2.0;
    ui.painter().line_segment(
        [egui::pos2(title_rect.left(), y), egui::pos2(title_rect.left() + 28.0, y)],
        egui::Stroke::new(2.0, accent),
    );
    ui.add_space(SPACE_SM);
    row.inner
}

/// A GROUP head: the line that names one band of sections (#14b) — air
/// above, a short gold tick, the name in the accent colour. Caption only: the
/// sections keep their own collapsing state, so nothing here can hide a
/// control.
pub(crate) fn group_caption(ui: &mut egui::Ui, title: &str) {
    ui.add_space(SPACE_LG);
    let accent = ui.visuals().hyperlink_color;
    ui.horizontal(|ui| {
        let h = ui.text_style_height(&egui::TextStyle::Small);
        let (tick, _) = ui.allocate_exact_size(egui::vec2(3.0, h), egui::Sense::hover());
        ui.painter().rect_filled(tick, 1.5, accent);
        ui.label(egui::RichText::new(title).small().strong().color(accent));
    });
    ui.add_space(SPACE_XS);
}

/// A caption for one block INSIDE a fold (the colour-grading scope, a mask's
/// slider groups, the library ladder's rungs, the snapshot history): air and
/// a weak small line, no hairline — the fold's own indent already says whose
/// block it is.
pub(crate) fn sub_caption(ui: &mut egui::Ui, title: &str) {
    ui.add_space(SPACE_MD);
    ui.label(egui::RichText::new(title).weak().small());
}

/// A fold drawn as a CARD while it is open (2026-10-12, user: 「UI 美观继续打磨，
/// 现在的还是有点"原始"」): its title and body sit on one rounded, slightly
/// raised plate that spans the column, so an open fold reads as one object
/// instead of a run of indented rows hung off a hairline. A closed fold stays a
/// single row. The plate is painted UNDER what the fold drew (a placeholder
/// shape reserved before the header, filled in once the body's extent is
/// known), so it changes no layout but the breath after an open card.
pub(crate) fn card_fold<R>(
    ui: &mut egui::Ui,
    header: egui::CollapsingHeader,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::CollapsingResponse<R> {
    let plate = ui.painter().add(egui::Shape::Noop);
    let (left, right) = (ui.max_rect().left(), ui.max_rect().right());
    let r = header.show(ui, body);
    if let Some(b) = &r.body_response {
        let c = theme_of(ui).colors();
        let rect = egui::Rect::from_min_max(
            egui::pos2(left - FOLD_CARD_PAD, r.header_response.rect.top() - FOLD_CARD_PAD / 2.0),
            egui::pos2(right + FOLD_CARD_PAD, b.rect.bottom() + FOLD_CARD_PAD),
        );
        ui.painter().set(
            plate,
            egui::Shape::Rect(egui::epaint::RectShape::new(
                rect,
                RADIUS_MD,
                c.card_fill,
                egui::Stroke::new(1.0, c.card_stroke),
            )),
        );
        ui.add_space(FOLD_CARD_PAD + SPACE_XS);
    }
    r
}
