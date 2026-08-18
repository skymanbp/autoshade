//! Histogram panel and the tone-curve editor.

use crate::*;

impl AutoshopApp {
    /// Draw the live histogram (R/G/B filled, luma outline; one bin per 8-bit
    /// code value on ONE shared vertical scale) — the tone readout a photo
    /// editor is expected to have. Sqrt-scaled so shadow detail reads.
    /// The corner triangles are LR's clipping indicators: lit when pixels sit
    /// at the J-overlay thresholds (≤1 / ≥254); clicking toggles that overlay.
    pub(crate) fn histogram_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let Some(hist) = &self.histogram else { return };
        let h = 72.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), h),
            egui::Sense::hover(),
        );
        let p = ui.painter_at(rect);
        p.rect_filled(rect, RADIUS_SM, egui::Color32::from_gray(16));
        let n = hist.len().max(1);
        let bar_w = rect.width() / n as f32;
        // Additive-ish RGB: draw each channel as translucent filled bars.
        let colors = [
            egui::Color32::from_rgba_unmultiplied(220, 70, 70, 110),
            egui::Color32::from_rgba_unmultiplied(90, 200, 90, 110),
            egui::Color32::from_rgba_unmultiplied(90, 130, 240, 110),
        ];
        for (ch, color) in colors.iter().enumerate() {
            for (i, bins) in hist.iter().enumerate() {
                let v = bins[ch].sqrt(); // sqrt: make shadow counts visible
                if v <= 0.0 {
                    continue;
                }
                let x0 = rect.min.x + i as f32 * bar_w;
                let y0 = rect.max.y - v * (h - 2.0);
                p.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x0 + bar_w, rect.max.y)),
                    0.0,
                    *color,
                );
            }
        }
        // Luma as a thin outline on top for the overall tone shape.
        let pts: Vec<egui::Pos2> = hist
            .iter()
            .enumerate()
            .map(|(i, bins)| {
                egui::pos2(
                    rect.min.x + (i as f32 + 0.5) * bar_w,
                    rect.max.y - bins[3].sqrt() * (h - 2.0),
                )
            })
            .collect();
        p.add(egui::Shape::line(pts, egui::Stroke::new(1.0, egui::Color32::from_gray(210))));

        // Clipping triangles, per-channel (the LR convention): the colour
        // names WHICH channels sit in the extreme bin — one channel reads as
        // that primary, two mix to yellow/magenta/cyan, all three to white
        // (a neutral crush/blow-out vs. a colour cast at a glance). Shadows
        // top-left, highlights top-right; grey when clean; click = the same
        // toggle as ▲ / J.
        let tri_color = |bins: &[f32; 4]| -> Option<egui::Color32> {
            let (r, g, b) = (bins[0] > 0.0, bins[1] > 0.0, bins[2] > 0.0);
            (r || g || b).then(|| {
                let c = |on: bool| if on { 255u8 } else { 45 };
                egui::Color32::from_rgb(c(r), c(g), c(b))
            })
        };
        let chan_names = |bins: &[f32; 4]| -> String {
            ["R", "G", "B"]
                .iter()
                .zip(bins)
                .filter(|(_, v)| **v > 0.0)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join("+")
        };
        let mut toggle = false;
        // The triangles judge the SAME thresholds as the J overlay (≤1 / ≥254):
        // with one bin per code value that is the two extreme bins summed. The
        // old 64-bin "extreme bin" spanned code values 0-3 / 252-255, so the
        // warning lit on near-clipped pixels the overlay never marked.
        let edge2 = |a: &[f32; 4], b: &[f32; 4]| -> [f32; 4] {
            std::array::from_fn(|ch| a[ch] + b[ch])
        };
        let second = hist.get(1).unwrap_or(&hist[0]);
        let shadow = edge2(&hist[0], second);
        let highlight = edge2(
            &hist[hist.len() - 1],
            hist.get(hist.len().wrapping_sub(2)).unwrap_or(&hist[hist.len() - 1]),
        );
        for (right, bins, what) in [
            (false, &shadow, "shadow crush"),
            (true, &highlight, "highlight clip"),
        ] {
            let s = 10.0;
            let x0 = if right { rect.max.x - s - 4.0 } else { rect.min.x + 4.0 };
            let tri = egui::Rect::from_min_size(egui::pos2(x0, rect.min.y + 4.0), egui::vec2(s, s));
            let lit = tri_color(bins);
            let tip = match lit {
                Some(_) => trf(
                    lang,
                    "{what}: {chan} channel(s) — click to toggle clipping warning (J)",
                    &[("what", tr(lang, what)), ("chan", &chan_names(bins))],
                ),
                None => trf(
                    lang,
                    "{what} indicator (clean) — click to toggle clipping warning (J)",
                    &[("what", tr(lang, what))],
                ),
            };
            let resp = ui
                .interact(tri, ui.id().with(("clip_tri", right)), egui::Sense::click())
                .on_hover_text(tip);
            let color = lit.unwrap_or(if self.show_clipping {
                self.theme.colors().clip_tri_on
            } else {
                self.theme.colors().clip_tri_off
            });
            p.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(tri.center().x, tri.min.y),
                    egui::pos2(tri.max.x, tri.max.y),
                    egui::pos2(tri.min.x, tri.max.y),
                ],
                color,
                egui::Stroke::NONE,
            ));
            if resp.clicked() {
                toggle = true;
            }
        }
        if toggle {
            let ctx = ui.ctx().clone();
            self.toggle_clipping(&ctx); // instant — rebuilt from the retained frame
        }
    }

    /// The interactive tone-curve editor: a channel picker (master / R / G / B)
    /// over a painted square — histogram backdrop, quarter grid, and the curve
    /// drawn straight from `render::curve_lut`, the SAME sampler the engine
    /// applies (so the preview line can never drift from the render). Click
    /// adds a point ON the curve, dragging moves it (inputs stay strictly
    /// increasing), dragging well outside the box deletes it — the Lightroom
    /// gestures. Returns true when the recipe changed this frame.
    pub(crate) fn curve_editor(&mut self, ui: &mut egui::Ui) -> bool {
        self.curve_editor_for(ui, CurveTarget::Global)
    }

    /// The same editor pointed at one set of four curves — the recipe's own
    /// (`CurveTarget::Global`) or a mask's (R25 P6). Everything below is the
    /// original body with `self.recipe.<curve>` replaced by a lookup through
    /// `target`; splitting the widget in two instead would have given the mask
    /// curves a second plot, a second gesture model and a second place for the
    /// engine-faithful line to drift.
    pub(crate) fn curve_editor_for(
        &mut self,
        ui: &mut egui::Ui,
        target: CurveTarget,
    ) -> bool {
        // A stale mask index addresses no curve (see `curve_points`): draw
        // nothing rather than fall through to somebody else's four vectors.
        // Checked ONCE here, so every `expect` below is this guard's.
        if curve_points(&self.recipe, target, self.curve_channel).is_none() {
            return false;
        }
        let lang = self.lang;
        let label_colors = self.theme.colors().curve_labels;
        let mut changed = false;
        ui.horizontal(|ui| {
            for (i, (name, _)) in CURVE_CHANNELS.iter().enumerate() {
                if ui
                    .selectable_label(
                        self.curve_channel == i,
                        egui::RichText::new(tr(lang, name)).color(label_colors[i]).small(),
                    )
                    .clicked()
                {
                    self.curve_channel = i;
                    self.curve_drag = None;
                }
            }
            if ui.small_button("↺").on_hover_text(tr(lang, "Clear the current channel's curve")).clicked() {
                let pts = curve_points_mut(&mut self.recipe, target, self.curve_channel)
                    .expect("the target was validated on entry");
                if !pts.is_empty() {
                    pts.clear();
                    changed = true;
                }
                self.curve_drag = None;
            }
        });

        let side = ui.available_width().clamp(160.0, 240.0);
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click_and_drag());
        #[cfg(test)]
        {
            // Test seam (the same convention as develop_count /
            // overlay_build_count): the headless driven test aims its
            // synthetic click with the square's screen rect.
            self.curve_rect = Some(rect);
        }
        let p = ui.painter_at(rect);
        let accent = CURVE_CHANNELS[self.curve_channel].1;
        p.rect_filled(rect, RADIUS_SM, egui::Color32::from_gray(16));

        // Value space: x = input 0..1 (left→right), y = output 0..1 (bottom→top).
        let to_screen = |x: f32, y: f32| {
            egui::pos2(rect.min.x + x * rect.width(), rect.max.y - y * rect.height())
        };
        let to_val = |q: egui::Pos2| {
            (
                ((q.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0),
                ((rect.max.y - q.y) / rect.height().max(1.0)).clamp(0.0, 1.0),
            )
        };

        // Histogram backdrop for the active channel (luma behind the master curve)
        // — same data as the panel histogram, sqrt-scaled the same way.
        if let Some(hist) = &self.histogram {
            let ch = [3usize, 0, 1, 2][self.curve_channel];
            let bar_w = rect.width() / hist.len().max(1) as f32;
            let fill =
                egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 34);
            for (i, bins) in hist.iter().enumerate() {
                let v = bins[ch].sqrt();
                if v <= 0.0 {
                    continue;
                }
                let x0 = rect.min.x + i as f32 * bar_w;
                p.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x0, rect.max.y - v * (rect.height() - 2.0)),
                        egui::pos2(x0 + bar_w, rect.max.y),
                    ),
                    0.0,
                    fill,
                );
            }
        }

        // Quarter grid + the identity diagonal for reference.
        let grid = egui::Stroke::new(1.0, egui::Color32::from_gray(38));
        for i in 1..4 {
            let t = i as f32 / 4.0;
            p.line_segment([to_screen(t, 0.0), to_screen(t, 1.0)], grid);
            p.line_segment([to_screen(0.0, t), to_screen(1.0, t)], grid);
        }
        p.line_segment(
            [to_screen(0.0, 0.0), to_screen(1.0, 1.0)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(56)),
        );

        // --- interaction (mutates the active channel's control points) --------
        const HIT: f32 = 10.0; // grab radius around a point, screen px
        let lut_before = autoshop::render::curve_lut(
            curve_points(&self.recipe, target, self.curve_channel)
                .expect("the target was validated on entry"),
        );
        // The drag belongs to the target it STARTED on: two editors can be
        // laid out in one frame (the global Curves section and a selected
        // mask's), and a bare index would let one editor's drag move — and
        // highlight — the other's point. Read fresh at each use, never cached,
        // because the block below both sets and clears it within one frame.
        let drag_here = |d: Option<(CurveTarget, usize)>| -> Option<usize> {
            d.and_then(|(t, i)| (t == target).then_some(i))
        };
        let pts = curve_points_mut(&mut self.recipe, target, self.curve_channel)
            .expect("the target was validated on entry");
        if (resp.drag_started() || resp.clicked())
            && let Some(q) = resp.interact_pointer_pos()
        {
            let near = pts.iter().position(|c| {
                to_screen(c.input as f32 / 255.0, c.output as f32 / 255.0).distance(q) <= HIT
            });
            let idx = match near {
                Some(i) => i,
                None => {
                    // Add ON the current curve at the clicked input — the shape
                    // doesn't jump; the user then drags the new point away.
                    let (vx, _) = to_val(q);
                    let input = (vx * 255.0).round() as u8;
                    let output = (lut_before[input as usize] * 255.0).round() as u8;
                    changed = true;
                    insert_curve_point(pts, input, output)
                }
            };
            if resp.drag_started() {
                self.curve_drag = Some((target, idx));
            }
        }
        if let Some(i) = drag_here(self.curve_drag).filter(|&i| i < pts.len()) {
            if resp.dragged()
                && let Some(q) = resp.interact_pointer_pos()
            {
                if rect.expand(28.0).contains(q) {
                    let (vx, vy) = to_val(q);
                    drag_curve_point(
                        pts,
                        i,
                        (vx * 255.0).round() as u8,
                        (vy * 255.0).round() as u8,
                    );
                } else {
                    pts.remove(i); // dragged well outside → delete (LR gesture)
                    self.curve_drag = None;
                }
                changed = true;
            }
            if resp.drag_stopped() {
                self.curve_drag = None;
            }
        }

        // Engine-faithful curve line: all 256 samples straight from the shared LUT.
        let lut = autoshop::render::curve_lut(pts);
        let line: Vec<egui::Pos2> = lut
            .iter()
            .enumerate()
            .map(|(i, &y)| to_screen(i as f32 / 255.0, y))
            .collect();
        p.add(egui::Shape::line(line, egui::Stroke::new(1.6, accent)));

        // Control-point handles (the dragged one filled with the channel colour).
        for (i, c) in pts.iter().enumerate() {
            let q = to_screen(c.input as f32 / 255.0, c.output as f32 / 255.0);
            if drag_here(self.curve_drag) == Some(i) {
                p.circle_filled(q, 5.0, accent);
            } else {
                p.circle_filled(q, 3.5, egui::Color32::from_gray(230));
                p.circle_stroke(q, 3.5, egui::Stroke::new(1.0, egui::Color32::from_gray(60)));
            }
        }

        ui.label(
            egui::RichText::new(tr(
                self.lang,
                "Click to add a point · drag to move · drag outside the box to delete — preview and export match (XMP carries the closest Lightroom form)",
            ))
            .weak()
            .small(),
        );
        changed
    }
}
