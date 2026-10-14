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
/// panel can toggle on it. `active` adds the section's activity dot — a small
/// gold disc, as on the folds (`util::section_title`); the drawn text is still
/// [`section_label`]'s, so a test reading it sees the ●.
pub(crate) fn panel_heading(
    ui: &mut egui::Ui,
    title: &str,
    active: bool,
    lead: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let accent = ui.visuals().hyperlink_color;
    let text: egui::WidgetText = if active {
        let font = egui::TextStyle::Heading.resolve(ui.style());
        let label = section_label(title, true);
        let mut job = egui::text::LayoutJob::default();
        job.append(title, 0.0, egui::TextFormat::simple(font.clone(), ui.visuals().strong_text_color()));
        job.append(
            &label[title.len()..],
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::new(font.size * 0.5, font.family),
                color: accent,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
        job.into()
    } else {
        egui::RichText::new(title).heading().strong().into()
    };
    let row = ui.horizontal(|ui| {
        lead(ui);
        ui.add(egui::Label::new(text).sense(egui::Sense::click()))
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
///
/// Set as the quiet, spaced small caption Lightroom and RapidRaw put over a
/// band (2026-10-13): the gold tick and gold text made every band head shout
/// as loud as the panel title above it, so the gold is now the title's alone.
pub(crate) fn group_caption(ui: &mut egui::Ui, title: &str) {
    ui.add_space(SPACE_LG + SPACE_SM);
    ui.label(
        egui::RichText::new(title)
            .small()
            .strong()
            .extra_letter_spacing(0.8)
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(SPACE_XS);
}

/// The fold disclosure: a thin chevron that turns from › to ⌄ as the fold
/// opens, in place of egui's filled triangle (2026-10-13 — the triangle is
/// the one glyph that read as a 2005 tree view).
pub(crate) fn paint_chevron(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let colour = ui.style().interact(response).fg_stroke.color;
    let rect = response.rect;
    let s = rect.width().min(rect.height()) * 0.2;
    let turn = egui::emath::Rot2::from_angle(openness * std::f32::consts::FRAC_PI_2);
    let points = [egui::vec2(-0.5 * s, -s), egui::vec2(0.5 * s, 0.0), egui::vec2(-0.5 * s, s)]
        .map(|v| rect.center() + turn * v)
        .to_vec();
    ui.painter().add(egui::Shape::line(points, egui::Stroke::new(1.5, colour)));
}

/// A section's how-it-works paragraph, folded to one quiet line (2026-10-14,
/// user: 「文字需要精简，不要过于 overwhelming」): the paragraph opens as the
/// line's tooltip, so nothing it says is lost, and the panel shows one row
/// where it used to show three to six. Status lines and warnings stay plain
/// labels — only explanation goes here.
pub(crate) fn explainer(ui: &mut egui::Ui, lang: Lang, text: &str) -> egui::Response {
    ui.label(egui::RichText::new(tr(lang, "ⓘ How this works")).small().weak()).on_hover_ui(|ui| {
        ui.set_max_width(EXPLAINER_W);
        ui.label(text);
    })
}

/// How wide an [`explainer`]'s paragraph opens.
pub(crate) const EXPLAINER_W: f32 = 320.0;

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
    let lift = ui.painter().add(egui::Shape::Noop);
    let (left, right) = (ui.max_rect().left(), ui.max_rect().right());
    let r = header.icon(paint_chevron).show(ui, body);
    // The header is the fold's button, and it rises under the pointer
    // (2026-10-14, user: 「"展开折叠"的按钮，放上去给阴影动画」): a raised plate
    // with a soft downward shadow fades in over MOTION_FADE_S and out again,
    // painted beneath the header's text, open fold or closed.
    let h = r.header_response.clone();
    let rise = ui.ctx().animate_bool_with_time(h.id.with("lift"), h.hovered(), MOTION_FADE_S);
    if rise > 0.0 {
        let c = theme_of(ui).colors();
        let row = egui::Rect::from_x_y_ranges(left - FOLD_CARD_PAD + 2.0..=right + FOLD_CARD_PAD - 2.0, h.rect.y_range()).expand2(egui::vec2(0.0, 2.0));
        let shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 2.0 * rise),
            blur: 8.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha((f32::from(c.shadow_alpha) * rise) as u8),
        };
        ui.painter().set(
            lift,
            egui::Shape::Vec(vec![
                shadow.as_shape(row, RADIUS_MD).into(),
                egui::Shape::rect_filled(row, RADIUS_MD, c.widget_hover.gamma_multiply(rise)),
            ]),
        );
    }
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
