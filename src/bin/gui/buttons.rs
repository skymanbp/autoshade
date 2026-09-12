//! The button vocabulary (R38). Every button in the app is one of four
//! kinds, and every kind stands exactly one row tall — `interact_size.y`,
//! the floor egui gives a framed button — so a row never mixes heights:
//!
//!  * [`primary`]  — the ONE main verb of its group (Export, Open photo,
//!    AI Analyze, Generate, Reverse-fit, Build, Apply, Save): solid [`PILL`]
//!    gold under near-black text on both themes (8.8:1 on the raw fill;
//!    `both_themes_pass_contrast_checks` pins the pair).
//!  * [`action`]   — every other text verb: the stock frame.
//!  * [`glyph`]    — an icon-only verb (＋ ▣ ✕ 🗑 ⧉ ⬆ ⬇ ↺ ⚙ ⌨): an exact
//!    square of the row height, so it lines up beside text verbs instead of
//!    sitting squat among them (egui's `small_button` zeroes the vertical
//!    padding AND skips the interact-size floor, which is where every
//!    uneven row came from).
//!  * [`toggle`] / [`glyph_toggle`] — an armed or selected state (tools,
//!    the eye 👁, the clipping ▲): the selection frame while on. A text
//!    toggle that needs no cell keeps egui's own `selectable_label`, which
//!    wears the same frame and the same floor.
//!
//! A glyph PREFIX on a text verb is vocabulary, not decoration: it stays
//! only where the same glyph marks the same meaning on at least two
//! buttons — 🤖 an AI verb outside the AI panel, ✓ finish, ✕ cancel, ＋
//! add, ↺/↻ reset & redraw, 🗂 a folder, 🖌 paint, 💧 pick from the image,
//! ⎘ the stamp, 🔄 fetch & rebuild, ✨ generated pixels, and the toolbar's
//! own pairs ↶↷ ⭯⭮ ◫⬛. The one-off glyphs (◌ ⊕ ⊖ ⇱ 🎛 📝 🖼 ⇩ 📷 ▭ ◯ ⌫
//! ⛶ 🎯) are gone; those buttons say their verb.
//!
//! Rows of equal verbs in the side panels lay out on [`columns`]: every
//! button in the row takes the same width, so the row lands in aligned
//! columns instead of a ragged wrapped line; the toolbar wraps only between
//! its [`group`]s, each measured from its labels so egui can place it whole.
//! `every_button_stands_one_row_tall_at_the_default_widths` renders every
//! panel in both languages over three frames and pins the height, the
//! squares, that no label wraps inside its cell, and that neither side
//! panel leaves its default width.

use super::*;

/// Text on the solid gold primary fill — the same near-black on both
/// themes, because the fill is the same gold on both.
pub(crate) const PRIMARY_FG: egui::Color32 = egui::Color32::from_gray(26);

/// One row: the height every button in this vocabulary stands.
pub(crate) fn row_h(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y
}

/// The size each of `n` equal cells takes across the rest of the row, in
/// whole pixels so `n` of them never overrun the row by a rounding error.
pub(crate) fn columns(ui: &egui::Ui, n: usize) -> egui::Vec2 {
    let n = n.max(1);
    let gaps = ui.spacing().item_spacing.x * (n - 1) as f32;
    egui::vec2(((ui.available_width() - gaps) / n as f32).floor().max(0.0), row_h(ui))
}

/// Every button this vocabulary drew on this thread — read by the
/// one-row-tall pin after a frame.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct Drawn {
    pub(crate) kind: &'static str,
    pub(crate) label: String,
    pub(crate) rect: egui::Rect,
    /// Whether the label fit its cell on one line — measured, not assumed.
    pub(crate) fits: bool,
}

#[cfg(test)]
thread_local! {
    pub(crate) static DRAWN: std::cell::RefCell<Vec<Drawn>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The one place a button reaches the screen: sized into `cell` when the
/// row is a grid, at its own width otherwise; disabled through egui's own
/// fade, never hidden.
///
/// A cell is ONE allocation of the row (`allocate_ui_with_layout`), so a
/// wrapped row carries the whole cell to its next line. Not a `scope`
/// around `add_sized`: a scope is laid at the cursor with whatever width is
/// left there and never wraps, so a cell past its line's end stood outside
/// the panel and widened the auto-fitting side panel every frame (R19's
/// runaway, met again as +47 px/frame while this vocabulary went in).
fn place(
    ui: &mut egui::Ui,
    kind: &'static str,
    cell: Option<egui::Vec2>,
    enabled: bool,
    text: egui::WidgetText,
    style: impl FnOnce(egui::Button<'static>) -> egui::Button<'static>,
) -> egui::Response {
    // A glyph fills its square edge to edge: a 24 px symbol plus egui's
    // 2 x 4 px of horizontal padding asks for 32 px, which the justified
    // layout cannot centre inside a 26 px square — it overflows it. A text
    // label keeps the padding on both sides.
    let square = kind.starts_with("glyph");
    #[cfg(test)]
    let label = text.text().to_owned();
    #[cfg(test)]
    let fits = cell.is_none_or(|cell| {
        let width = ui.fonts(|f| {
            f.layout_no_wrap(label.clone(), egui::TextStyle::Button.resolve(ui.style()), egui::Color32::WHITE)
                .size()
                .x
        });
        let padding = if square { 0.0 } else { 2.0 * ui.spacing().button_padding.x };
        width + padding <= cell.x
    });
    let button = style(egui::Button::new(text));
    let response = match cell {
        Some(cell) => {
            let layout = egui::Layout::centered_and_justified(ui.layout().main_dir());
            ui.allocate_ui_with_layout(cell, layout, |ui| {
                if square {
                    ui.spacing_mut().button_padding.x = 0.0;
                }
                ui.add_enabled(enabled, button)
            })
            .inner
        }
        None => ui.add_enabled(enabled, button),
    };
    #[cfg(test)]
    DRAWN.with_borrow_mut(|d| d.push(Drawn { kind, label, rect: response.rect, fits }));
    response
}

/// One toolbar group as ONE allocation of the wrapped toolbar row, so a
/// narrow window carries the whole group to the next line instead of
/// splitting Undo from Redo. egui wraps only between allocations, and a
/// `horizontal` child is laid at the cursor at whatever width is left
/// there, so the group's width is measured up front from its own labels —
/// the same galleys its buttons draw, plus their padding — with `glyphs`
/// squares of the row height, the spacing between the items and the group
/// fence after them. Over-measuring by one spacing (the Export split joins
/// two buttons with none) only wraps a hair early: the buttons still lay
/// out at their own widths.
pub(crate) fn group<R>(
    ui: &mut egui::Ui,
    labels: &[&str],
    glyphs: usize,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let padding = 2.0 * ui.spacing().button_padding.x;
    let text: f32 = ui.fonts(|f| {
        labels
            .iter()
            .map(|l| f.layout_no_wrap((*l).to_owned(), font.clone(), egui::Color32::WHITE).size().x + padding)
            .sum()
    });
    let items = labels.len() + glyphs;
    let width = text
        + glyphs as f32 * row_h(ui)
        + ui.spacing().item_spacing.x * items.saturating_sub(1) as f32
        + SPACE_LG;
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(egui::vec2(width.ceil(), row_h(ui)), layout, |ui| {
        let r = add(ui);
        // The fence between groups, INSIDE the allocation: a separator or a
        // space of its own would wrap to a line start and sit there orphaned.
        ui.add_space(SPACE_LG);
        r
    })
    .inner
}

fn primary_text(text: impl Into<String>) -> egui::WidgetText {
    egui::RichText::new(text).color(PRIMARY_FG).into()
}

/// A secondary text verb at its own width.
pub(crate) fn action(ui: &mut egui::Ui, enabled: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    place(ui, "action", None, enabled, text.into(), |b| b)
}

/// A secondary text verb filling one grid cell of the row ([`columns`]).
pub(crate) fn action_in(
    ui: &mut egui::Ui,
    cell: egui::Vec2,
    enabled: bool,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    place(ui, "action", Some(cell), enabled, text.into(), |b| b)
}

/// The group's one main verb at its own width.
pub(crate) fn primary(ui: &mut egui::Ui, enabled: bool, text: impl Into<String>) -> egui::Response {
    place(ui, "primary", None, enabled, primary_text(text), |b| b.fill(PILL))
}

/// The group's one main verb filling one grid cell of the row.
pub(crate) fn primary_in(ui: &mut egui::Ui, cell: egui::Vec2, enabled: bool, text: impl Into<String>) -> egui::Response {
    place(ui, "primary", Some(cell), enabled, primary_text(text), |b| b.fill(PILL))
}

/// The primary verb as a builder, for the one row that sizes its button by
/// hand (the Reimagine prompt row measures it against its field).
pub(crate) fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(primary_text(text)).fill(PILL)
}

/// An armed / selected text state filling one grid cell of the row.
pub(crate) fn toggle_in(ui: &mut egui::Ui, cell: egui::Vec2, on: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    place(ui, "toggle", Some(cell), true, text.into(), |b| b.selected(on))
}

/// An icon-only verb: an exact square of the row height.
pub(crate) fn glyph(ui: &mut egui::Ui, enabled: bool, g: impl Into<egui::WidgetText>) -> egui::Response {
    let h = row_h(ui);
    place(ui, "glyph", Some(egui::vec2(h, h)), enabled, g.into(), |b| b)
}

/// An icon-only armed / selected state: the same square, the selection
/// frame while on.
pub(crate) fn glyph_toggle(ui: &mut egui::Ui, on: bool, g: impl Into<egui::WidgetText>) -> egui::Response {
    let h = row_h(ui);
    place(ui, "glyph_toggle", Some(egui::vec2(h, h)), true, g.into(), |b| b.selected(on))
}
