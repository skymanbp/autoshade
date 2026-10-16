//! The Lens Blur section (2026-10-13, user: 「AI 景深虚化」 — the AI estimates
//! depth and blurs the background progressively; the user picks the focus
//! point).
//!
//! 🤖 Estimate depth runs `python/depth.py` (MoGe-2) on the original-frame
//! preview — the frame recipe masks and the develop live in — and lands a
//! `LensBlur` on the card pointing at the claimed depth PNG. 🎯 Pick focus
//! arms the canvas: the next click sets the focus point, and the blur follows
//! at once because the render reads only the depth map, the point and the two
//! sliders (`render::lens_blur`).

use crate::*;

impl AutoShadeApp {
    /// The Lens Blur section of the Develop panel.
    pub(crate) fn dev_lens_blur(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let mut changed = false;
        let active = self.recipe.lens_blur.as_ref().is_some_and(|l| l.draws());
        ui.add_space(SPACE_MD);
        card_fold(
            ui,
            egui::CollapsingHeader::new(section_title(tr(lang, "Lens Blur"), active))
                .id_salt("sec_lens_blur")
                .default_open(false),
            |ui| changed = self.lens_blur_body(ui),
        );
        changed
    }

    fn lens_blur_body(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        let has_helper = depth_helper_available();
        let can_run = !self.busy && self.base_preview.is_some() && self.src_path.is_some() && has_helper;
        let verb = if self.recipe.lens_blur.is_some() { tr(lang, "🤖 Re-estimate depth") } else { tr(lang, "🤖 Estimate depth") };
        let tip = if has_helper {
            ai_xref(lang, tr(lang, "MoGe-2 estimates how far away every part of the photo is (Python helper; the first run downloads a ~1.3 GB model). Then click the photo to choose what stays sharp."))
        } else {
            tr(lang, "This feature needs AutoShade's Python helper, which this download does not include. Install the full package, or (advanced) set AUTOSHADE_DEPTH_SCRIPT to the path of depth.py").to_string()
        };
        if action_row(ui, can_run, verb).on_hover_text(tip).clicked() {
            self.start_depth();
        }
        let Some(lb) = self.recipe.lens_blur.as_mut() else {
            let hint = tr(lang, "Estimate the depth first, then click the photo to choose what stays sharp.");
            ui.label(egui::RichText::new(hint).weak().small());
            return false;
        };
        let mut changed = false;
        let mut remove = false;
        let mut flip_pick = false;
        let picking = self.lens_focus_picking;
        ui.horizontal(|ui| {
            let mute = tr(lang, "Show/mute the lens blur without losing its settings");
            if glyph_toggle(ui, lb.enabled, "👁").on_hover_text(mute).clicked() {
                lb.enabled = !lb.enabled;
                changed = true;
            }
            let cell = egui::vec2(ui.available_width() - row_h(ui) - ui.spacing().item_spacing.x, row_h(ui));
            let how = tr(lang, "Click the photo: whatever is at that distance stays sharp; nearer and farther blur by how far they are from it.");
            flip_pick = toggle_in(ui, cell, picking, tr(lang, "🎯 Pick focus")).on_hover_text(how).clicked();
            remove = glyph(ui, true, "🗑").on_hover_text(tr(lang, "Remove the lens blur")).clicked();
        });
        // (label, value, reset) — the two dials of `render::LensBlur`.
        let on = lb.enabled;
        let dials = [(tr(lang, "Blur amount"), &mut lb.amount, 50.0), (tr(lang, "Focus range"), &mut lb.range, 10.0)];
        ui.add_enabled_ui(on, |ui| {
            for (label, value, reset) in dials {
                changed |= Self::slider(ui, lang, label, value, 0.0, 100.0, reset);
            }
        });
        let carried = tr(lang, "Rendered here and in every export from this app; Lightroom's sidecar cannot carry it.");
        ui.label(egui::RichText::new(carried).weak().small());
        if flip_pick {
            // One canvas tool at a time: every arming site disarms the rest.
            self.disarm_tools();
            self.lens_focus_picking = !picking;
        }
        if remove {
            self.recipe.lens_blur = None;
            self.lens_focus_picking = false;
            changed = true;
        }
        changed
    }

    /// Run the depth sidecar on the ORIGINAL-frame preview into a fresh
    /// claimed `depth.png` in the photo's develop folder (a saved recipe may
    /// still reference the previous one, so no map is ever rewritten).
    pub(crate) fn start_depth(&mut self) {
        if self.busy {
            return;
        }
        let Some(base) = self.base_preview.clone() else { return };
        let Some(src) = self.src_path.clone() else { return };
        self.busy = true;
        self.status = tr(self.lang, "AI estimating depth… (the first run downloads the model)").into();
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<PathBuf> {
                    let input = crate::masks::stage_preview_png(&base, "depth")?;
                    let out = autoshade::store::claim_raster(&src, "depth")?;
                    let run = autoshade::depth::estimate_depth(&autoshade::config::Config::load(), &input, &out);
                    let _ = std::fs::remove_file(&input);
                    // A failed run releases its claimed (0-byte) name.
                    run.inspect_err(|_| {
                        let _ = std::fs::remove_file(&out);
                    })?;
                    Ok(out)
                })();
                Msg::DepthEstimated(res)
            },
            |e| Msg::DepthEstimated(Err(e)),
        );
    }

    /// `Msg::DepthEstimated` landing: point the card's blur at the new map,
    /// keeping a focus and dials already chosen, and arm the focus pick for a
    /// first estimate.
    pub(crate) fn on_depth_estimated(&mut self, res: anyhow::Result<PathBuf>) {
        self.busy = false;
        let path = match res {
            Ok(path) => path,
            Err(e) => {
                self.toast(ToastKind::Error, format!("{}: {e:#}", tr(self.lang, "Depth estimate failed")));
                return;
            }
        };
        let first = self.recipe.lens_blur.is_none();
        let lb = self.recipe.lens_blur.get_or_insert_with(Default::default);
        lb.depth_map = path.to_string_lossy().into_owned();
        lb.enabled = true;
        if first {
            self.disarm_tools();
            self.lens_focus_picking = true;
        }
        self.dirty = true;
        let lang = self.lang;
        self.status = if first {
            tr(lang, "Depth estimated — click the photo to choose what stays sharp")
        } else {
            tr(lang, "Depth re-estimated — the focus point and sliders are kept")
        }
        .into();
    }

    /// 🎯 armed: a click sets the focus point in the original frame, the one
    /// the develop and the depth map share.
    pub(crate) fn handle_lens_focus_pick(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        let Some((nx, ny)) = self.clicked_orig_norm(ui, resp, xf) else { return };
        self.lens_focus_picking = false;
        let Some(lb) = self.recipe.lens_blur.as_mut() else { return };
        lb.focus = [nx.clamp(0.0, 1.0), ny.clamp(0.0, 1.0)];
        self.dirty = true;
        self.status = tr(self.lang, "Focus set — the blur follows the distance of what you clicked").into();
    }

    /// The focus point as a small ring while the pick is armed, so the
    /// photographer sees where the current focus sits before moving it.
    pub(crate) fn draw_lens_focus(&self, ui: &egui::Ui, xf: ViewXform) {
        let Some(lb) = self.recipe.lens_blur.as_ref() else { return };
        let ((bw, bh), deg, dist) = self.geom_ctx();
        let (vx, vy) = orig_norm_to_view(lb.focus[0], lb.focus[1], (bw, bh), deg, &dist);
        let c = xf.to_screen(vx, vy);
        let p = ui.painter();
        p.circle_stroke(c, 9.0, egui::Stroke::new(3.0, egui::Color32::from_black_alpha(150)));
        p.circle_stroke(c, 9.0, egui::Stroke::new(1.5, egui::Color32::from_white_alpha(235)));
        p.circle_filled(c, 2.0, egui::Color32::WHITE);
    }
}
