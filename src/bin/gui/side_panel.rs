//! The controls side panel's width (2026-10-13, user: 「最大宽度会随着展开的
//! 编辑选项变化而变化」).
//!
//! egui 0.29's `SidePanel` remembers, as next frame's width, the rect its
//! CONTENT ended up covering (`PanelState { rect }` stored after
//! `add_contents`, panel.rs). So one row wider than the panel — a fold opened
//! on a long line, a value box at the far end of a drag — widened the panel,
//! and the panel then stayed that wide: its width was a ratchet over every
//! section ever opened. The fixes before this one (R19, R22-3, R38, #14a)
//! each made one more row fit; this takes the ratchet out. Only the user's
//! drag sets the width, the content never does, and the width lives between
//! [`CONTROLS_W_MIN`] and [`controls_w_max`].

use super::*;

/// The id the controls panel stores its width under — egui's `SidePanel`
/// salt, so a width the user dragged before this change is the one restored.
const CONTROLS_ID: &str = "controls";

/// Show the controls panel with its width owned by the drag alone.
///
/// The width the panel is laid at this frame is egui's own computation —
/// last frame's stored width, or the pointer while the edge is dragged, then
/// clamped into the range and to the screen. After the content has drawn,
/// that width is written back over whatever the content covered, so an
/// overflowing row is clipped by the panel (egui clips it already) instead of
/// becoming next frame's width.
pub(crate) fn controls_side_panel<R>(
    ctx: &egui::Context,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let id = egui::Id::new(CONTROLS_ID);
    let range = CONTROLS_W_MIN..=controls_w_max(&ctx.style());
    let stored = egui::containers::panel::PanelState::load(ctx, id).map(|s| s.rect.width());
    let response = egui::SidePanel::left(id)
        .default_width(CONTROLS_W_DEFAULT)
        .width_range(range.clone())
        .show(ctx, add_contents);
    let rect = response.response.rect;
    let drag = ctx.read_response(id.with("__resize")).filter(egui::Response::dragged);
    let laid = match drag.and_then(|d| d.interact_pointer_pos()) {
        Some(pointer) => pointer.x - rect.left(),
        None => stored.unwrap_or(CONTROLS_W_DEFAULT),
    };
    let laid = laid.clamp(*range.start(), *range.end()).min(ctx.screen_rect().width());
    if (rect.width() - laid).abs() > 0.5 {
        let rect = egui::Rect::from_min_size(rect.min, egui::vec2(laid, rect.height()));
        ctx.data_mut(|d| d.insert_persisted(id, egui::containers::panel::PanelState { rect }));
    }
    response
}
