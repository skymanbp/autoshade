//! Color Grading's wheels (2026-10-13, user: 「可以加个像 LR 那种三色调色盘之类
//! 的面板」).
//!
//! Lightroom grades on colour wheels: the angle is the hue, the distance from
//! the centre the saturation, and a luminance slider sits under each wheel.
//! The section used to show one region at a time as three plain sliders
//! picked from a combo box, so the three regions could not be seen together
//! and a hue had to be found by number. The wheels write the same
//! `ColorGrade` fields the sliders did — the recipe, the render and the
//! sidecar are unchanged.

use crate::*;

/// The section's two views, Lightroom's 3-way and Global.
pub(crate) const GRADE_VIEWS: [&str; 2] = ["3-way", "Global"];

/// The display colour of `hue` (degrees) at saturation `s` (0..=1), at one
/// muted brightness so the wheel reads as hue and chroma only — a
/// full-brightness rainbow was the loudest object in a dark panel, where
/// Lightroom's wheels sit a step above mid-grey.
fn wheel_colour(hue: f32, s: f32) -> egui::Color32 {
    egui::ecolor::Hsva::new(hue.rem_euclid(360.0) / 360.0, 0.8 * s.clamp(0.0, 1.0), 0.62, 1.0).into()
}

/// The offset of a wheel point at `hue` (degrees, counter-clockwise from
/// 3 o'clock, red there) and `sat` (0..=100) for a wheel of radius `r`.
pub(crate) fn wheel_offset(hue: f32, sat: f32, r: f32) -> egui::Vec2 {
    let a = hue.to_radians();
    egui::vec2(a.cos(), -a.sin()) * (sat / 100.0).clamp(0.0, 1.0) * r
}

/// The hue / saturation a wheel point `d` (offset from the centre) stands
/// for, in the recipe's whole-number steps (hue below 360, the recipe wraps
/// it; a centred puck has no hue).
pub(crate) fn wheel_value(d: egui::Vec2, r: f32) -> (f32, f32) {
    let sat = (d.length() / r * 100.0).clamp(0.0, 100.0).round();
    if sat == 0.0 {
        return (0.0, 0.0);
    }
    let hue = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0).round();
    (if hue >= 360.0 { 0.0 } else { hue }, sat)
}

/// One colour wheel of `diameter` with its title above and its numbers
/// under it: drag anywhere on the disc to set hue and saturation, Shift for
/// a quarter of the motion, double-click to return to neutral. Returns
/// whether hue or saturation changed.
pub(crate) fn grade_wheel(
    ui: &mut egui::Ui,
    lang: Lang,
    title: &str,
    diameter: f32,
    hue: &mut f32,
    sat: &mut f32,
) -> bool {
    let mut changed = false;
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(title).small().color(ui.visuals().weak_text_color()));
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::click_and_drag());
        let resp = resp.on_hover_text(tr(
            lang,
            "Drag: angle = hue, distance from the centre = saturation · Shift for fine control · double-click to reset",
        ));
        let c = rect.center();
        let r = diameter * 0.5 - 2.0;
        if resp.double_clicked() {
            if *hue != 0.0 || *sat != 0.0 {
                *hue = 0.0;
                *sat = 0.0;
                changed = true;
            }
        } else if resp.dragged() || resp.drag_started() {
            let fine = ui.input(|i| i.modifiers.shift);
            let target = if fine {
                // Fine: the puck follows a quarter of the pointer's motion.
                Some(c + wheel_offset(*hue, *sat, r) + resp.drag_delta() * 0.25)
            } else {
                resp.interact_pointer_pos()
            };
            if let Some(p) = target {
                let (h, s) = wheel_value(p - c, r);
                if (h, s) != (*hue, *sat) {
                    *hue = h;
                    *sat = s;
                    changed = true;
                }
            }
        }
        if ui.is_rect_visible(rect) {
            paint_wheel(ui, c, r, *hue, *sat, resp.hovered() || resp.dragged());
        }
        ui.label(
            egui::RichText::new(format!("{} {:.0}°  ·  {} {:.0}", tr(lang, "Hue"), hue, tr(lang, "Saturation"), sat))
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    });
    changed
}

/// The disc (hue round, saturation out), its rim and centre cross, and the
/// puck.
fn paint_wheel(ui: &egui::Ui, c: egui::Pos2, r: f32, hue: f32, sat: f32, hot: bool) {
    const SEGMENTS: u32 = 96;
    const RINGS: u32 = 8;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, wheel_colour(0.0, 0.0));
    for ring in 1..=RINGS {
        let t = ring as f32 / RINGS as f32;
        for seg in 0..SEGMENTS {
            let h = seg as f32 / SEGMENTS as f32 * 360.0;
            mesh.colored_vertex(c + wheel_offset(h, t * 100.0, r), wheel_colour(h, t));
        }
    }
    let at = |ring: u32, seg: u32| 1 + (ring - 1) * SEGMENTS + seg % SEGMENTS;
    for seg in 0..SEGMENTS {
        mesh.add_triangle(0, at(1, seg), at(1, seg + 1));
    }
    for ring in 1..RINGS {
        for seg in 0..SEGMENTS {
            let (a, b, inner_next, outer_next) = (at(ring, seg), at(ring, seg + 1), at(ring + 1, seg), at(ring + 1, seg + 1));
            mesh.add_triangle(a, b, inner_next);
            mesh.add_triangle(b, outer_next, inner_next);
        }
    }
    let p = ui.painter();
    p.add(egui::Shape::mesh(mesh));
    p.circle_stroke(c, r, ui.visuals().widgets.noninteractive.bg_stroke);
    let cross = egui::Stroke::new(1.0, egui::Color32::from_black_alpha(70));
    p.line_segment([c - egui::vec2(4.0, 0.0), c + egui::vec2(4.0, 0.0)], cross);
    p.line_segment([c - egui::vec2(0.0, 4.0), c + egui::vec2(0.0, 4.0)], cross);
    let puck = c + wheel_offset(hue, sat, r);
    let pr = if hot { 6.5 } else { 5.5 };
    p.circle_filled(puck, pr + 1.5, egui::Color32::from_black_alpha(140));
    p.circle_filled(puck, pr, wheel_colour(hue, sat / 100.0));
    p.circle_stroke(puck, pr, egui::Stroke::new(1.5, egui::Color32::WHITE));
}
