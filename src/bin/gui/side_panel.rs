//! The two side panels' widths — the library and the controls — and what the
//! controls column's content may do with its own.
//!
//! WHO SETS THE WIDTH (2026-10-13, user: 「最大宽度会随着展开的编辑选项变化而
//! 变化」). egui 0.29's `SidePanel` remembers, as next frame's width, the rect
//! its CONTENT ended up covering (`PanelState { rect }` stored after
//! `add_contents`, panel.rs). So one row wider than the panel — a fold opened
//! on a long line, a value box at the far end of a drag — widened the panel,
//! and the panel then stayed that wide: its width was a ratchet over every
//! section ever opened. The fixes before this one (R19, R22-3, R38, #14a)
//! each made one more row fit; this takes the ratchet out. Only the user's
//! drag sets the width, the content never does, and the width lives between
//! [`CONTROLS_W_MIN`] and [`controls_w_max`] — and, for the library panel
//! since 2026-11-17, between [`LIBRARY_W_MIN`] and [`library_w_max`]
//! ([`drag_owned_panel`] is the one host of both).
//!
//! …AND WHERE THE CANVAS STARTS (2026-11-19, user: 「控件栏和右边预览窗口中间
//! 多了一大块黑色」). egui reads the same content rect a second time, in the
//! same frame: the rest of the window is laid beside it (`allocate_left_panel`).
//! Writing the width back after the frame — what the 2026-10-13 fix did —
//! stopped the ratchet and left this: a selected radial mask's feather row
//! stood 97 px past the panel, so the canvas began 97 px to the right of it
//! on every frame, and the strip between the two was painted by nobody. The
//! content's rect now never reaches egui at all ([`drag_owned_panel`]), which
//! answers both readers at once and needs no write-back.
//!
//! WHAT A DRAG MAY MOVE (2026-11-17, user: 「那一栏的宽度一调整，里面的各种
//! 按钮、控件就开始跟着动，很诡异」). Dragging the edge changes how WIDE the
//! column's rows are and nothing else: no row wraps or unwraps, nothing
//! changes its height, so nothing under it moves up or down. Before this, the
//! grading wheels and the curve grew with the panel, the eyedropper jumped up
//! beside its checkbox at one width, radio rows and notes re-wrapped at
//! others, and every such change slid the whole column under it — 150 px of
//! vertical travel over one drag.
//!
//! The rule has one mechanism. Whatever takes its HEIGHT from a width — a
//! wrapping row, a wrapped note, a square — measures against the
//! [`steady_width`]: the width the same `Ui` has with the column at its
//! narrowest. The narrowest is also where the column opens
//! ([`CONTROLS_W_MIN`] is the default width), so the layout a drag keeps is
//! the designed one. Rails, fields, cards and button cells still span the
//! real width: they stretch sideways, which moves nothing vertically.
//! `the_controls_column_keeps_every_row_in_place_at_any_width` lays the whole
//! column out across the range in both languages and pins every drawn row to
//! one y.

use super::*;

/// The id the controls panel stores its width under — egui's `SidePanel`
/// salt, so a width the user dragged before this change is the one restored.
const CONTROLS_ID: &str = "controls";

/// …and the library panel's, for the same reason ("gallery" is the panel's
/// older name, and the one its width has always been stored under).
const LIBRARY_ID: &str = "gallery";

thread_local! {
    /// How much wider the controls column is, this frame, than at
    /// [`CONTROLS_W_MIN`] — set by [`ColumnSlack::enter`] while the column
    /// draws, zero everywhere else (a panel laid by a test or another host
    /// has no narrower self to keep to).
    static COLUMN_SLACK: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// The column's slack for as long as this guard lives.
pub(crate) struct ColumnSlack(f32);

impl ColumnSlack {
    /// `ui` is the controls panel's own `Ui`, before the column draws.
    pub(crate) fn enter(ui: &egui::Ui) -> Self {
        let narrowest = CONTROLS_W_MIN - egui::Frame::side_top_panel(ui.style()).inner_margin.sum().x;
        Self(COLUMN_SLACK.replace((ui.max_rect().width() - narrowest).max(0.0)))
    }
}

impl Drop for ColumnSlack {
    fn drop(&mut self) {
        COLUMN_SLACK.set(self.0);
    }
}

/// The width `ui` has left as it would be with the controls column at its
/// narrowest: what anything that takes its height from a width measures
/// against, so the height is the same at every width of a drag.
pub(crate) fn steady_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() - COLUMN_SLACK.get()).max(0.0)
}

/// A wrapping row that wraps where it would with the column at its narrowest
/// — so it holds the same number of lines at every width — for own-width
/// items (radios, a checkbox beside a dropdown, a spinner and its line).
/// Rows of `columns` cells need none of this: equal cells fill their row at
/// any width.
pub(crate) fn steady_wrapped<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let w = steady_width(ui);
    ui.allocate_ui_with_layout(egui::vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_max_width(w);
        ui.horizontal_wrapped(add).inner
    })
    .inner
}

/// A label wrapped at the steady width, so a drag never changes how many
/// lines it takes. The text is laid out HERE, at that width, and handed to
/// egui as a finished galley: a `Label` given text wraps it at whatever the
/// `Ui` has left, which is the real width. No child `Ui` — the label stands
/// in a row or a column exactly as `ui.label` would.
pub(crate) fn steady_label(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    let galley = text.into().into_galley(ui, Some(egui::TextWrapMode::Wrap), steady_width(ui), egui::TextStyle::Body);
    ui.label(galley)
}

/// A note under a control — weak, small — as a [`steady_label`]. Every
/// free-standing note of the column is one of these.
pub(crate) fn steady_note(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    steady_label(ui, egui::RichText::new(text).weak().small())
}

/// Show the controls panel with its width owned by the drag alone.
pub(crate) fn controls_side_panel<R>(
    ctx: &egui::Context,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let range = CONTROLS_W_MIN..=controls_w_max(&ctx.style());
    drag_owned_panel(ctx, CONTROLS_ID, CONTROLS_W_DEFAULT, range, add_contents)
}

/// Show the library panel — the left-most one — with its width owned by the
/// drag alone (2026-11-17). It was a plain `SidePanel`, so it had the ratchet
/// this module's head describes: copying a recipe drew a checkbox whose
/// English label was 4 px wider than the panel, and the panel stayed 4 px
/// wider; and it had no floor, so its narrowest was wherever the title row
/// stopped it, a different width in each language.
pub(crate) fn library_side_panel<R>(
    ctx: &egui::Context,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let range = LIBRARY_W_MIN..=library_w_max(&ctx.style());
    drag_owned_panel(ctx, LIBRARY_ID, LIBRARY_W_DEFAULT, range, add_contents)
}

#[cfg(test)]
thread_local! {
    /// Each side panel this thread laid, at its latest frame: its id, the
    /// rect its column was given and the rect the column's rows covered.
    /// The layout tests read it — a row the panel clips away is drawn
    /// nowhere, so only the layout itself can say it was too wide.
    static LAID: std::cell::RefCell<Vec<(egui::Id, egui::Rect, egui::Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The rect the side panel `id` gave its column at this thread's latest
/// frame, and the rect the column's rows covered.
#[cfg(test)]
pub(crate) fn laid_column(id: &str) -> Option<(egui::Rect, egui::Rect)> {
    let id = egui::Id::new(id);
    LAID.with_borrow(|l| l.iter().find(|(i, ..)| *i == id).map(|(_, given, covered)| (*given, *covered)))
}

/// A left side panel whose width only the user's drag sets.
///
/// The width the panel is laid at this frame is egui's own computation —
/// last frame's stored width, or the pointer while the edge is dragged, then
/// clamped into the range and to the screen. The column draws in a CHILD of
/// the panel's `Ui`, and the panel's `Ui` is then told only of the rect it
/// handed the child: the rect egui reads back from a panel — to store as next
/// frame's width, and to lay the rest of the window beside it — is the rect
/// the panel's own `Ui` covered, so it is the panel's laid rect whatever the
/// rows did. An overflowing row is clipped by the panel (egui clips to the
/// laid rect already) and moves nothing outside it.
fn drag_owned_panel<R>(
    ctx: &egui::Context,
    id: &str,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let id = egui::Id::new(id);
    egui::SidePanel::left(id).default_width(default).width_range(range).show(ctx, |ui| {
        let given = ui.max_rect();
        let mut column = ui.new_child(egui::UiBuilder::new().id_salt("column").max_rect(given).layout(*ui.layout()));
        let out = add_contents(&mut column);
        #[cfg(test)]
        LAID.with_borrow_mut(|l| {
            l.retain(|(i, ..)| *i != id);
            l.push((id, given, column.min_rect()));
        });
        // No widget of its own: the rect is taken, nothing is sensed, so
        // nothing lies over the column's controls.
        ui.advance_cursor_after_rect(given);
        out
    })
}
