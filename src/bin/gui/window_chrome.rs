//! The window's own title bar on Windows (2026-10-13, user: 「UI 现在最上方
//! -□x 条，太丑了」).
//!
//! The system caption was a second, empty bar above the toolbar in the OS's
//! own colours. On Windows the window is now undecorated (`main.rs`) and the
//! toolbar IS the title bar, the way Lightroom, Photoshop and every Electron
//! editor draw it: the toolbar's empty space drags the window (Aero Snap
//! included — winit hands the drag to the OS as a caption drag), a
//! double-click maximizes or restores, the three window buttons sit at its
//! right end, and the window's edges resize it.
//!
//! macOS and Linux keep their native frames: the traffic lights and the
//! desktop's own decorations are what users there expect, and neither was
//! the complaint.

use super::*;

/// Whether this build draws its own title bar.
pub(crate) const CUSTOM_CHROME: bool = cfg!(target_os = "windows");

/// One window button's width — the system caption's own 46 px.
const BUTTON_W: f32 = 46.0;

/// The width the three window buttons take at the toolbar's right end.
pub(crate) fn controls_width() -> f32 {
    if CUSTOM_CHROME { 3.0 * BUTTON_W } else { 0.0 }
}

/// How close to a window edge the pointer must be to resize from it.
const EDGE: f32 = 5.0;

/// The toolbar row with the title-bar duties around it: `add` lays the
/// toolbar's groups in a wrapping row that stops short of the window
/// buttons; the bar's empty space drags and double-click maximizes.
///
/// The drag region is registered BEFORE the toolbar's widgets, so every
/// widget on top of it keeps its own clicks — egui gives the later widget the
/// pointer where two overlap.
pub(crate) fn toolbar_row(ui: &mut egui::Ui, lang: Lang, add: impl FnOnce(&mut egui::Ui)) {
    let bar = ui.clip_rect();
    if CUSTOM_CHROME {
        let drag = ui.interact(bar, egui::Id::new("title_bar_drag"), egui::Sense::click_and_drag());
        if drag.double_clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        } else if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }
    let row_w = (ui.available_width() - controls_width() - SPACE_LG).max(0.0);
    ui.allocate_ui_with_layout(
        egui::vec2(row_w, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
        |ui| {
            // Toolbar verbs read as text until touched (a frame on hover),
            // as in Lightroom's and Photoshop's bars: a row of eleven framed
            // boxes was the bar's loudest feature. The gold primary keeps its
            // own fill.
            let w = &mut ui.visuals_mut().widgets.inactive;
            w.weak_bg_fill = egui::Color32::TRANSPARENT;
            w.bg_fill = egui::Color32::TRANSPARENT;
            w.bg_stroke = egui::Stroke::NONE;
            add(ui);
        },
    );
    if CUSTOM_CHROME {
        window_buttons(ui, bar, lang);
    }
}

/// Minimize, maximize / restore and close, flush with the bar's top-right
/// corner, each the full height of the bar, drawn as thin line glyphs.
fn window_buttons(ui: &mut egui::Ui, bar: egui::Rect, lang: Lang) {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let fg = ui.visuals().text_color();
    for (k, which) in ["min", "max", "close"].into_iter().enumerate() {
        let right = bar.right() - BUTTON_W * (2 - k) as f32;
        let rect = egui::Rect::from_min_max(egui::pos2(right - BUTTON_W, bar.top()), egui::pos2(right, bar.bottom()));
        let resp = ui.interact(rect, egui::Id::new(("window_button", which)), egui::Sense::click());
        let hot = resp.hovered() || resp.is_pointer_button_down_on();
        let (fill, ink) = match (which, hot) {
            ("close", true) => (egui::Color32::from_rgb(196, 43, 28), egui::Color32::WHITE),
            (_, true) => (ui.visuals().widgets.hovered.weak_bg_fill, fg),
            _ => (egui::Color32::TRANSPARENT, fg),
        };
        let p = ui.painter();
        p.rect_filled(rect, 0.0, fill);
        let c = rect.center();
        let s = 5.0;
        let stroke = egui::Stroke::new(1.0, ink);
        match which {
            "min" => {
                p.line_segment([c + egui::vec2(-s, 0.0), c + egui::vec2(s, 0.0)], stroke);
            }
            "max" if maximized => {
                // Restore: two offset squares.
                let back = egui::Rect::from_center_size(c + egui::vec2(1.5, -1.5), egui::vec2(2.0 * s - 2.0, 2.0 * s - 2.0));
                let front = egui::Rect::from_center_size(c + egui::vec2(-1.5, 1.5), egui::vec2(2.0 * s - 2.0, 2.0 * s - 2.0));
                p.rect_stroke(back, 1.0, stroke);
                p.rect_filled(front, 1.0, fill);
                p.rect_stroke(front, 1.0, stroke);
            }
            "max" => {
                p.rect_stroke(egui::Rect::from_center_size(c, egui::vec2(2.0 * s, 2.0 * s)), 1.0, stroke);
            }
            _ => {
                p.line_segment([c + egui::vec2(-s, -s), c + egui::vec2(s, s)], stroke);
                p.line_segment([c + egui::vec2(-s, s), c + egui::vec2(s, -s)], stroke);
            }
        }
        let tip = match which {
            "min" => tr(lang, "Minimize"),
            "max" if maximized => tr(lang, "Restore"),
            "max" => tr(lang, "Maximize"),
            _ => tr(lang, "Close"),
        };
        if resp.on_hover_text(tip).clicked() {
            ctx.send_viewport_cmd(match which {
                "min" => egui::ViewportCommand::Minimized(true),
                "max" => egui::ViewportCommand::Maximized(!maximized),
                _ => egui::ViewportCommand::Close,
            });
        }
    }
}

/// Resize from the window's edges and corners: an undecorated window has no
/// frame for the OS to hit-test, so the edge band is ours — it sets the
/// resize cursor and hands a press to the OS as a resize drag. Not while
/// maximized (there is no edge to pull) and not over an open popup. Runs
/// before the panels, so the band's cursor wins over theirs.
pub(crate) fn edge_resize(ctx: &egui::Context) {
    if !CUSTOM_CHROME || ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    let r = ctx.screen_rect();
    let (w, e, n, s) = (pos.x - r.left() < EDGE, r.right() - pos.x < EDGE, pos.y - r.top() < EDGE, r.bottom() - pos.y < EDGE);
    use egui::viewport::ResizeDirection as D;
    let dir = match (w, e, n, s) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (.., true) => D::South,
        _ => return,
    };
    ctx.set_cursor_icon(match dir {
        D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
        D::West | D::East => egui::CursorIcon::ResizeHorizontal,
        D::North | D::South => egui::CursorIcon::ResizeVertical,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

/// A hairline round the undecorated window, so its edge does not melt into
/// a dark desktop or a neighbouring dark window. None while maximized.
pub(crate) fn window_outline(ctx: &egui::Context) {
    if !CUSTOM_CHROME || ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let stroke = ctx.style().visuals.widgets.noninteractive.bg_stroke;
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window_outline")))
        .rect_stroke(ctx.screen_rect().shrink(0.5), 0.0, stroke);
}
