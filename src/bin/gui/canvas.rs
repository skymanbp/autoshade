//! After-image canvas: view transform, overlays, drag handlers.

use super::*;

impl AutoshopApp {
    /// The edge a BAKE renders at: the canvas's own resolution, not the
    /// global preference.
    ///
    /// They part company whenever the canvas is a baked raster the preference
    /// could not re-decode — a background retouched / Generated variant keeps
    /// its own resolution because switching variants cannot re-decode a
    /// master, and an undo can restore a superseded one. Baking at the
    /// preference there installed a NEW-edge raster under an OLD-edge canvas
    /// and the frame jumped resolution mid-retouch: the same disagreement the
    /// refused resolution switch exists to prevent, one door along.
    /// `preview_edge` remains the DECODE edge (`open_path`) — a source-based
    /// canvas is developed from a source decoded AT it, so those two agree by
    /// construction.
    /// Glide the zoom one step toward its target — run BEFORE any layout
    /// reads it, so the canvas, the % readout and the pan clamp all see one
    /// value. Extracted as the testable seam (L16-10): tests used to drive
    /// the pure glide_step while the update-loop call could be deleted with
    /// every test green; this method IS the behaviour now, and update() is
    /// one call.
    pub(crate) fn apply_zoom_glide(&mut self, ctx: &egui::Context) {
        if self.zoom != self.zoom_target {
            let dt = ctx.input(|i| i.stable_dt);
            self.zoom = glide_step(self.zoom, self.zoom_target, dt);
            ctx.request_repaint(); // keep stepping while in flight
        }
    }

    pub(crate) fn canvas_edge(&self) -> u32 {
        self.base_preview
            .as_ref()
            .map(|b| b.width().max(b.height()))
            .unwrap_or(self.preview_edge)
            .clamp(640, 8192)
    }

    /// The resolution a canvas must DISCLOSE, when it holds baked pixels the
    /// preview preference cannot reach. `None` when there is nothing to say.
    ///
    /// FOUR doors install such a canvas: the strip click, an undo to a
    /// SUPERSEDED master (the re-decode repoints only the Arc the canvas
    /// held), the navigation stash restore (which reinstalls the raster the
    /// photo was left with while this open decoded the source at the current
    /// preference), and deleting the active variant (which re-anchors onto a
    /// background one).
    pub(crate) fn baked_canvas_edge(&self) -> Option<u32> {
        // A canvas has TWO legitimate resolutions, and measuring against
        // either one alone produced a false claim:
        //  · the PREFERENCE — a recorded master is re-decoded with an
        //    UNGUARDED `thumbnail`, which upscales, so a re-decode lands
        //    exactly there whatever the sensor is;
        //  · what the source DELIVERS — the RAW decode is guarded against
        //    upscaling, so a fresh bake on a sensor smaller than the
        //    preference lands there instead, and is perfectly current.
        // Only a THIRD value is stale: a raster baked under an older
        // preference, or a superseded master an undo brought back. Measuring
        // against the preference alone called every fresh sub-preference bake
        // stale; against the delivered edge alone it called every REOPENED
        // sub-preference master stale ("not the preview preference" while the
        // preference was exactly that). It takes both.
        let delivered = self
            .source_preview
            .as_ref()
            .map(|s| s.width().max(s.height()))
            .unwrap_or(self.preview_edge)
            .clamp(640, 8192);
        let canvas = self.canvas_edge();
        // The baked-raster gate is DEFENCE IN DEPTH, not load-bearing: a
        // source-based canvas IS the source decode (`load_active` falls back
        // to `source_preview`, and every install keeps the pair in lockstep),
        // so `canvas == delivered` already silences it. No test can kill this
        // conjunct; it stays because a future canvas that is neither would
        // otherwise be announced with a word — "bake" — that would not be
        // true of it.
        (canvas != delivered
            && canvas != self.preview_edge
            && self.active_variant().is_some_and(|v| v.base.is_some()))
        .then_some(canvas)
    }

    /// Write the status for a door that just installed a canvas, in BOTH
    /// directions: the disclosure when this canvas disagrees with what the
    /// preference can deliver, and `plain` when it does not.
    ///
    /// The `plain` arm is not decoration. Announcing the disagreement without
    /// retracting it left the previous door's "stays at 1280px — edits and
    /// retouches follow that" standing after a redo had put the canvas back
    /// where the preference could reach: both halves false, on the status
    /// bar, which never expires. A door that can create the disagreement can
    /// also end it, so every door writes on every path.
    pub(crate) fn set_canvas_status(&mut self, plain: &'static str) {
        let lang = self.lang;
        self.status = match self.baked_canvas_edge() {
            Some(px) => trf(
                lang,
                "the canvas pixels stay at {px}px (their own bake) — edits and retouches follow that, not the preview preference",
                &[("px", &px.to_string())],
            ),
            None => tr(lang, plain).to_string(),
        };
    }

    /// The reverse-fit / style-prompt target: a finished rendition of THIS
    /// frame whose look is to be solved back into develop parameters.
    ///
    /// Two entries, in precedence order (R23-6 B, user decision 2026-08-17
    /// ⑤ — "a second develop of the same frame", i.e. the photographer's own
    /// Lightroom export or the camera's JPEG, not only what this app
    /// generated):
    ///
    ///   1. `fit_ref` — a file the user picked on purpose. It wins, because
    ///      an explicit choice must not be shadowed by whichever variant
    ///      happens to be active.
    ///   2. the ./out raster behind the active variant when that variant is
    ///      an AI-generated rendition — the original entry, unchanged.
    ///
    /// The restriction to (2) was never a property of the fit: `fit.rs`'s
    /// own doc has always said "any finished reference of the SAME frame",
    /// and the CLI `match` has always accepted an arbitrary path. It was a
    /// GUI-side `then(...)` that made the generative path the only reachable
    /// one — and, downstream of that, made "the target is not pixel-aligned"
    /// read as an axiom of the method rather than a consequence of the only
    /// target the desktop app could offer.
    pub(crate) fn fit_target(&self) -> Option<PathBuf> {
        if let Some(p) = &self.fit_ref {
            return Some(p.clone());
        }
        let v = self.active_variant()?;
        // Deliberately NOT `!is_parametric()` (R24-1): this is a POLICY about
        // which raster the fit DEFAULTS to, not a claim about the card's
        // parametric-ness — a Fitted card carrying an origin would satisfy
        // the predicate's negation the day one does, and re-fitting a fit is
        // not what the button means.
        (v.kind == VariantKind::Generated).then(|| v.origin.clone()).flatten()
    }

    /// Load the Before texture: `base` with `curve` (a camera-matched base
    /// look) applied. Lightroom's "Before" is the profile-applied default
    /// render, not the linear negative — without the curve, Before sat
    /// 0.6–1.4 EV under After's own starting point and every compare
    /// exaggerated the edit. Baked rasters pass an empty curve (their pixels
    /// already carry the look).
    pub(crate) fn set_before(&mut self, ctx: &egui::Context, base: &image::DynamicImage, curve: &[[f32; 2]]) {
        let img = if curve.is_empty() {
            to_color_image(base)
        } else {
            to_color_image(&autoshop::render::develop_preview(
                base,
                &EditRecipe { base_curve: curve.to_vec(), ..Default::default() },
            ))
        };
        self.before_tex = Some(ctx.load_texture("before", img, egui::TextureOptions::LINEAR));
        self.before_curve = curve.to_vec();
    }

    /// Toggle the clipping layer WITHOUT a full redevelop: the overlay is a
    /// pure function of the last developed pixels (retained from the last
    /// accepted frame), so the J key / ▲ button / histogram triangles respond
    /// instantly instead of paying a whole develop (100-300 ms at 2560/4096).
    /// Falls back to a redevelop only when no frame is retained yet.
    pub(crate) fn toggle_clipping(&mut self, ctx: &egui::Context) {
        self.show_clipping = !self.show_clipping;
        if !self.show_clipping {
            self.clip_tex = None; // OFF is just dropping the layer
            return;
        }
        match &self.last_rgb {
            Some(rgb) => {
                let clip = clipping_overlay_for(rgb, self.recipe.crop);
                if let Some(tex) = &mut self.clip_tex {
                    tex.set(clip, egui::TextureOptions::NEAREST);
                } else {
                    self.clip_tex =
                        Some(ctx.load_texture("clip", clip, egui::TextureOptions::NEAREST));
                }
            }
            None => self.dirty = true, // nothing retained (fresh open) — worker builds it
        }
    }

    /// (Re)build the translucent red coverage layer for the active mask. A
    /// coverage key prevents local effect sliders from rebuilding this full-
    /// frame raster: Exposure/Temp/Saturation change WHAT happens inside the
    /// mask, not WHERE it applies. Range masks include their PREFIX
    /// reference recipe (earlier masks applied — the engine's own stacking
    /// rule) because their coverage genuinely depends on pixels.
    pub(crate) fn refresh_mask_overlay(&mut self, ctx: &egui::Context) {
        if !self.show_mask_overlay {
            self.mask_overlay_tex = None;
            self.overlay_key = None;
            return;
        }
        let Some(base) = self.base_preview.as_ref() else {
            self.mask_overlay_tex = None;
            self.overlay_key = None;
            return;
        };
        // A hovered row previews its coverage; otherwise use the selection.
        let target = self
            .hover_mask
            .filter(|&i| i < self.recipe.masks.len())
            .or_else(|| self.sel_mask.filter(|&i| i < self.recipe.masks.len()));
        let Some(i) = target else {
            self.mask_overlay_tex = None;
            self.overlay_key = None;
            return;
        };
        let mask = self.recipe.masks[i].clone();
        let mut pre = self.recipe.clone();
        // The PREFIX (masks before this one), not masks-cleared: the engine
        // evaluates a Range Mask on the pixel as it stands when THIS mask
        // runs — masks stack sequentially, so a later mask's range sees
        // earlier masks' output (render.rs apply_masks). A cleared reference
        // judged mask 2's range on pixels mask 0/1 had already moved (CX5-6).
        pre.masks.truncate(i);
        // Geometry runs after develop, so it is not part of a Range Mask's
        // masks-cleared pixel reference. Keep it separately in OverlayKey.
        pre.straighten_deg = 0.0;
        pre.lens_distortion = 0.0;
        pre.ca_r = 0.0;
        pre.ca_b = 0.0;
        pre.crop = None;
        let lp = &self.recipe.lens_profile;
        let key = OverlayKey {
            base: Arc::as_ptr(base) as usize,
            target: i,
            mask: mask.mask.clone(),
            components: mask.components.clone(),
            enabled: mask.enabled,
            range: mask.range,
            amount: mask.amount,
            inverted: mask.inverted,
            reference_recipe: mask.range.is_some().then(|| pre.clone()),
            straighten_deg: self.recipe.straighten_deg,
            lens_distortion: self.recipe.lens_distortion,
            profile_dist_on: lp.distortion_on && !lp.distortion.is_empty(),
            profile_ca_on: lp.ca_on && !lp.ca_r.is_empty() && !lp.ca_b.is_empty(),
            manual_ca: (self.recipe.ca_r, self.recipe.ca_b),
        };
        if self.overlay_key.as_ref() == Some(&key) && self.mask_overlay_tex.is_some() {
            return;
        }

        // A geometry-only mask never reads reference pixels. Avoid the old
        // second develop entirely; only Range Masks need the masks-cleared
        // developed reference and its recipe-keyed cache.
        // KNOWN COST (C21): this is a ONE-slot cache compared by full-recipe
        // equality, and the reference is now the mask PREFIX rather than a
        // masks-cleared develop — so sweeping the pointer down a list of
        // Range masks alternates prefixes and misses the cache on every row,
        // paying a synchronous preview develop each time. Correctness first:
        // the prefix IS what the engine evaluates a range against, and a
        // wrong overlay is worse than a slow one. Keyed multi-slot caching is
        // the fix and is recorded in the roadmap, not smuggled in here.
        let reference: &image::DynamicImage = if mask.range.is_some() {
            if !matches!(&self.overlay_ref, Some((r, _)) if *r == pre) {
                // Rebuilding the masks-cleared reference is a full develop on
                // the UI thread — the 100-300 ms class (2560/4096) the async
                // preview path exists to hide. Mid-gesture (global-slider
                // drag), keep showing the previous coverage and re-arm the
                // stale flag; the rebuild lands on the frame after the pointer
                // settles. Geometry-only masks (the else arm) stay instant.
                if ctx.input(|i| i.pointer.any_down()) {
                    self.overlay_stale = true;
                    return;
                }
                let img = autoshop::render::develop_preview(base, &pre);
                self.overlay_ref = Some((pre, img));
            }
            &self.overlay_ref.as_ref().expect("range reference cached").1
        } else {
            base.as_ref()
        };
        // Coverage is a display-only translucent layer (LINEAR-filtered when
        // painted), so it never needs more than ~1k resolution — at 2560/4096
        // preview edges the full-frame raster + RGBA pass dominated the
        // rebuild. Range weights are judged on the downscaled reference: the
        // same pixels box-filtered, indistinguishable for an overlay.
        let small;
        let cov_ref: &image::DynamicImage = if reference.width().max(reference.height()) > 1024 {
            small = reference.thumbnail(1024, 1024);
            &small
        } else {
            reference
        };
        let mut cov = image::DynamicImage::ImageLuma8(autoshop::render::mask_coverage(&mask, cov_ref));
        // The COMPOSED profile (R25 B3) — manual CA rides the same knots.
        let cov_geom = autoshop::render::geometry_profile(&self.recipe);
        if cov_geom.geometry_active() || self.recipe.lens_distortion != 0.0 {
            // The coverage overlay must follow the SAME geometric chain as the
            // rendered pixels — profile distortion included, or the red wash
            // drifts off its mask near the frame edges.
            cov = autoshop::render::apply_lens_geometry(
                &cov,
                &cov_geom,
                self.recipe.lens_distortion,
            );
        }
        if self.recipe.straighten_deg != 0.0 {
            cov = autoshop::render::rotate_straighten(&cov, self.recipe.straighten_deg);
        }
        let g = cov.to_luma8();
        let (w, h) = (g.width() as usize, g.height() as usize);
        let mut rgba = vec![0u8; w * h * 4];
        for (i, p) in g.pixels().enumerate() {
            rgba[i * 4..i * 4 + 4]
                .copy_from_slice(&[255, 40, 40, (p[0] as u16 * 140 / 255) as u8]);
        }
        let colour = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        if let Some(tex) = &mut self.mask_overlay_tex {
            tex.set(colour, egui::TextureOptions::LINEAR);
        } else {
            self.mask_overlay_tex =
                Some(ctx.load_texture("mask_overlay", colour, egui::TextureOptions::LINEAR));
        }
        self.overlay_key = Some(key);
        self.overlay_build_count += 1;
    }

    /// The geometric mapping context every interaction boundary needs:
    /// original preview pixel dims + the current straighten angle + the lens
    /// geometry (in-camera profile + manual distortion amount).
    pub(crate) fn geom_ctx(&self) -> ((f32, f32), f32, LensArg) {
        let dims = self
            .base_preview
            .as_ref()
            .map(|b| {
                let (w, h) = b.dimensions();
                (w as f32, h as f32)
            })
            .unwrap_or((1.0, 1.0));
        (
            dims,
            self.recipe.straighten_deg,
            LensArg {
                // COMPOSED (R25 B3): every interaction map downstream of this
                // bundle must see the same profile the pixels went through,
                // manual CA included.
                profile: autoshop::render::geometry_profile(&self.recipe).into_owned(),
                amount: self.recipe.lens_distortion,
            },
        )
    }

    /// The visible full-frame uv window: committed crop (shown cropped, like
    /// Lightroom — except while the crop tool is open, which needs the full
    /// frame) narrowed by zoom/pan. `pan` is stored in crop-window coords and
    /// re-clamped here so edge panning never accumulates out of range.
    pub(crate) fn view_uv(&mut self) -> egui::Rect {
        let win = match (&self.recipe.crop, self.crop_mode) {
            (Some(c), false) => egui::Rect::from_min_max(
                egui::pos2(c.left.min(c.right), c.top.min(c.bottom)),
                egui::pos2(c.right.max(c.left), c.bottom.max(c.top)),
            ),
            _ => egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        };
        let half = 0.5 / self.zoom.clamp(1.0, 12.0);
        self.pan = egui::vec2(
            self.pan.x.clamp(half, 1.0 - half),
            self.pan.y.clamp(half, 1.0 - half),
        );
        egui::Rect::from_min_max(
            egui::pos2(
                win.min.x + (self.pan.x - half) * win.width(),
                win.min.y + (self.pan.y - half) * win.height(),
            ),
            egui::pos2(
                win.min.x + (self.pan.x + half) * win.width(),
                win.min.y + (self.pan.y + half) * win.height(),
            ),
        )
    }

    /// The After image with its interaction layers — crop tool, mask placement,
    /// paint canvas, box-select — or the SOURCE flashed in the same rect while
    /// `comparing` (B held). Scroll zooms to the cursor; middle-drag or
    /// Space+drag pans; all image-space handlers map through [`ViewXform`].
    pub(crate) fn after_view(&mut self, ui: &mut egui::Ui, max_w: f32, avail_y: f32, comparing: bool) {
        let lang = self.lang;
        let tex = if comparing { self.before_tex.as_ref() } else { self.after_tex.as_ref() };
        let Some((id, tex_size)) = tex.map(|t| (t.id(), t.size_vec2())) else {
            // First develop still in flight: say so — a bare "…" read as a
            // hang, a spinner reads as work.
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(tr(lang, "Preparing preview…")).weak());
            });
            return;
        };

        let uv = self.view_uv();
        // Display size fits the VISIBLE window's aspect (in image pixels).
        let vis_px = egui::vec2(uv.width() * tex_size.x, uv.height() * tex_size.y);
        // Past fit the 4× upscale cap is DROPPED (L13-2): the cap tamed tiny
        // images at fit, but the visible window shrinks as zoom grows, so at
        // deep zoom the cap re-fit the canvas SMALLER — zooming in shrank
        // the picture and broke the cursor anchor. A zoomed view fills the
        // pane box.
        let disp = fit_in_capped(
            vis_px,
            max_w,
            avail_y,
            if self.zoom > 1.0 { f32::INFINITY } else { 4.0 },
        );
        // PHYSICAL pixels per image pixel: disp is measured in egui logical
        // points — without pixels_per_point a "100%" readout on a 2× display
        // spanned two physical pixels per texel, and "1:1" wasn't.
        let ppp = ui.ctx().pixels_per_point();
        let scale = disp.x * ppp / vis_px.x.max(1.0); // display (physical) px per image px
        // The zoom that makes `scale` exactly 1.0, stored for the `1` key
        // which runs at frame start, before vis_px/disp exist; the 1:1
        // button and double-click read it too. Solved at FIT geometry:
        // w_tex (full crop in texels) is zoom-invariant, but the naive
        // vis/disp form was not — fit_in's 4× upscale cap makes disp track
        // vis on small images, so that form varied with live zoom (Codex
        // 阶段5 F2). Pane-bound displays give the identical value; a photo
        // already coarser than 1:1 at fit clamps to 1 (fit is closest).
        let w_tex = vis_px * self.zoom;
        let disp_fit = fit_in(w_tex, max_w, avail_y);
        self.zoom_one_to_one = (w_tex.x / (disp_fit.x * ppp)).clamp(1.0, 12.0);

        // Caption row: mode hint left, zoom readout + Fit / 1:1 right. Every
        // armed-tool hint names its exit (Esc), and the placement hint speaks
        // the armed KIND's language instead of calling a radial a gradient.
        let hint = if comparing {
            if self.before_latch {
                // The latch came from \ — "release B" was a lie there (M21).
                tr(lang, "Before (source) — press \\ again (or Esc) to return to editing")
            } else {
                tr(lang, "Before (source) — release B to return to editing")
            }
        } else if self.crop_mode {
            tr(lang, "Crop — drag corners/edges to resize, inside to move, outside to rotate · Esc to exit")
        } else if let Some((kind, _)) = self.placing_mask {
            match kind {
                MaskKind::Linear => tr(lang, "Linear gradient — drag from the fully-applied side to the unaffected side (Shift = axis lock) · Esc to exit"),
                MaskKind::Radial => tr(lang, "Radial gradient — drag to draw an elliptical area · Esc to exit"),
            }
        } else if self.wb_picking {
            tr(lang, "WB eyedropper — click a spot that should be neutral grey/white · Esc to exit")
        } else if self.range_picking.is_some() {
            tr(lang, "Color range — click the color to pick in the image · Esc to exit")
        } else if self.clone_mode {
            tr(lang, "Stamp — Alt+click to set the source · drag to brush the area to cover · Esc to exit")
        } else if self.paint_mode {
            if self.mask_brush.is_some() {
                tr(lang, "Mask brush — paint to select · 「Erase」 removes · 「Apply」 bakes · Esc cancels")
            } else {
                tr(lang, "Brush — paint over the area to fill / heal · Esc to exit")
            }
        } else {
            tr(lang, "After — drag a box = local AI · scroll to zoom · space/middle-drag to pan · hold B to compare")
        };
        // An armed tool must be visible at NORMAL contrast — .weak().small()
        // is the passive-help style, and it was the only always-on indicator
        // when the arming control sat in a collapsed section.
        let armed = !comparing && self.tool_armed();
        ui.horizontal(|ui| {
            if armed {
                ui.label(egui::RichText::new(hint).color(self.theme.colors().armed_hint).small());
            } else {
                ui.label(egui::RichText::new(hint).weak().small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("1:1").on_hover_text(tr(lang, "Preview pixels 1:1 (double-click the image to toggle; key: 1)")).clicked() {
                    // Same ceiling as the render path (view_uv clamps at 12) —
                    // an unclamped value desynced zoom/pan math from the view.
                    // ppp: 1:1 means one texel per PHYSICAL pixel — the value
                    // computed above, shared with the `1` key. Target only:
                    // the update() glide carries `zoom` there.
                    self.zoom_target = self.zoom_one_to_one;
                }
                // "Fit" is natural language, unlike its "1:1" sibling — it
                // must route through `tr` like every user-facing literal
                // (the i18n module contract; the audit now flags bypasses).
                if ui.small_button(tr(lang, "Fit")).on_hover_text(tr(lang, "Fit the whole image to the canvas (double-click the image to toggle; key: 0)")).clicked() {
                    self.zoom_target = 1.0; // glides; the pan clamp eases the rest
                    self.pan = egui::vec2(0.5, 0.5);
                }
                if ui
                    .selectable_label(self.show_clipping, "▲")
                    .on_hover_text(tr(lang, "Clipping warning (J): red = highlight clip, blue = shadow crush (judged on export pixels)"))
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.toggle_clipping(&ctx); // instant — no redevelop
                }
                ui.label(egui::RichText::new(format!("{:.0}%", scale * 100.0)).weak().small());
                // --- preview resolution (gap batch E): 1:1 that actually
                // resolves detail. Switching re-decodes the current photo with
                // the recipe KEPT (the keep_recipe path from batch B).
                let before = self.preview_edge;
                ui.add_enabled_ui(!self.busy, |ui| {
                    egui::ComboBox::from_id_salt("preview_edge")
                        .selected_text(format!("{}px", self.preview_edge))
                        .width(64.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.preview_edge, 1280, tr(lang, "1280px · fluid"));
                            ui.selectable_value(&mut self.preview_edge, 2560, "2560px");
                            ui.selectable_value(&mut self.preview_edge, 4096, tr(lang, "4096px · inspect"));
                        })
                        .response
                        .on_hover_text(tr(lang, "Working preview resolution: 1280 is smoothest on the sliders; 2560/4096 for 1:1 focus/noise checks (slower on every adjustment)"));
                });
                if self.preview_edge != before
                    && let Some(p) = self.src_path.clone()
                {
                    if self.busy {
                        // An already-open popup outlives the disabled combo
                        // (its own egui Area), so this is reachable while
                        // busy — and the combo has ALREADY mutated
                        // preview_edge. Keeping the new value displayed a
                        // resolution the working preview does not have,
                        // unreachable to boot (the != trigger above is a
                        // one-shot), persisted it into Prefs, and fed it to
                        // every bake site. The switch reverts with the
                        // refusal, and the status says so instead of
                        // dropping it silently.
                        self.preview_edge = before;
                        // BY TOAST (the switch_variant / Ctrl+S refusal
                        // rule): this runs during the draw phase, and a
                        // status line dies under the very next worker
                        // message — one frame, then the combo just snaps
                        // back with no visible reason.
                        let t = tr(
                            lang,
                            "busy — the preview-resolution switch was not applied; pick it again when the current task finishes",
                        )
                        .to_string();
                        self.toast(ToastKind::Error, t);
                    } else {
                        // The pre-switch edge rides the flight: a FAILED
                        // keep-flight leaves the canvas at the OLD edge, and
                        // preview_edge must not keep displaying (and feeding
                        // every bake site) a resolution the preview never
                        // reached — the same residue the busy refusal above
                        // is cured of.
                        self.edge_before_flight = Some(before);
                        self.keep_recipe = true; // re-decode, keep the edit
                        self.open_path(p);
                    }
                }
            });
        });

        let (rect, resp) = ui.allocate_exact_size(disp, egui::Sense::click_and_drag());
        ui.painter_at(rect).image(id, rect, uv, egui::Color32::WHITE);
        // Diagnostic layers, uv-synced with the image (hidden while comparing):
        // clipping warnings, then the selected mask's coverage on top.
        if !comparing {
            if let Some(t) = &self.clip_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
            if let Some(t) = &self.mask_overlay_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
        }
        let xf = ViewXform { rect, uv };

        // --- zoom to cursor (scroll) -----------------------------------------
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.1
                && let Some(p) = resp.hover_pos()
            {
                let half = 0.5 / self.zoom;
                let (fx, fy) = (
                    ((p.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0),
                    ((p.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0),
                );
                // Cursor's point in crop-window coords, kept stationary across the zoom.
                let q = egui::vec2(
                    self.pan.x - half + fx * 2.0 * half,
                    self.pan.y - half + fy * 2.0 * half,
                );
                // Scroll stays INSTANT (it is already incremental, and the
                // cursor-anchored pan re-solve needs the immediate zoom) —
                // the target follows so no stale glide fights the wheel.
                self.zoom = (self.zoom * (scroll * 0.003).exp()).clamp(1.0, 12.0);
                self.zoom_target = self.zoom;
                let nh = 0.5 / self.zoom;
                self.pan = q - egui::vec2((fx - 0.5) * 2.0 * nh, (fy - 0.5) * 2.0 * nh);
            }
        }
        let tool_active = self.tool_armed();
        // Double-click toggles fit ↔ 1:1 (preview pixels) — but never while a
        // canvas tool is armed: a quick second tap inside brush/crop/pick used
        // to teleport the view instead of reaching the tool. Targets only —
        // the update() glide replaces the old teleport with a ~120 ms ease.
        if resp.double_clicked() && !tool_active {
            if self.zoom_target > 1.01 {
                self.zoom_target = 1.0;
                self.pan = egui::vec2(0.5, 0.5);
            } else {
                // Same physical-pixel 1:1 target as the button above — the
                // zoom-invariant form computed at the top of this fn (the
                // old `vis_px.x / (disp.x*ppp)` was only right at zoom≈1,
                // which the pre-glide branch guard used to guarantee).
                self.zoom_target = self.zoom_one_to_one;
            }
            // The pair's FIRST release cleared the AI region through the
            // plain-click path — a double-click means "toggle zoom", so put
            // it back. Bounded window: a much later double-click elsewhere
            // must not resurrect a long-cleared region (M18).
            if let Some((r, t)) = self.region_restore.take()
                && t.elapsed() < Duration::from_millis(600)
            {
                self.region = Some(r);
            }
        }

        // --- pan: middle-drag, Space + left-drag, or (the LR gesture) a plain
        // left-drag while ZOOMED IN — a zoomed-in drag means "pan" in every
        // photo editor, and requiring Space for it was a top "feels off".
        // Box-select stays reachable while zoomed via Ctrl+drag; an active
        // tool, a mask-knob hover/drag or a box drag in flight all take
        // priority over the implicit pan.
        // Focus-gated like B-compare: Space in a text field is a space.
        let space = ui.ctx().memory(|m| m.focused()).is_none()
            && ui.input(|i| i.key_down(egui::Key::Space));
        let ctrl = ui.input(|i| i.modifiers.command);
        let over_knob = self.mask_drag.is_some()
            || self.sel_target_geometry().is_some_and(|g| {
                let (dims, deg, dist) = self.geom_ctx();
                // Probe BOTH positions: egui reports drag_started only
                // after ~6 px of travel, so a fast flick's CURRENT pointer
                // has already left the knob while its press origin is still
                // on it — `or_else` (origin only when hover was None) still
                // let pan steal an in-canvas flick. The knobs probed are the
                // ones actually shown: the selected COMPONENT's when one is
                // selected (sel_target_geometry), matching handle_mask_edit.
                let handles = mask_handle_points(&geom_to_view(g, dims, deg, &dist), xf);
                let hits = |p: egui::Pos2| handles.iter().any(|(_, hp)| hp.distance(p) <= HANDLE_HIT);
                resp.hover_pos().is_some_and(hits)
                    || ui.input(|i| i.pointer.press_origin()).is_some_and(hits)
            });
        let zoom_pan = self.zoom > 1.001
            && !tool_active
            && !ctrl
            && !over_knob
            && self.region_drag.is_none();
        let panning = resp.dragged_by(egui::PointerButton::Middle)
            || ((space || zoom_pan) && resp.dragged_by(egui::PointerButton::Primary));
        if panning {
            let d = resp.drag_delta();
            let ext = 1.0 / self.zoom; // visible extent in crop-window coords
            self.pan -= egui::vec2(
                d.x / rect.width().max(1.0) * ext,
                d.y / rect.height().max(1.0) * ext,
            );
        }

        // Cursor language: say what a click/drag would do right now. The pick
        // tools (WB / range / clone-source) set their own crosshair in their
        // handlers; this covers the hand for panning and the drawing tools.
        if panning {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if (space || zoom_pan) && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if resp.hovered()
            && (self.paint_mode || self.placing_mask.is_some() || self.crop_mode)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }

        if comparing || panning {
            return; // tools pause while comparing / panning
        }

        // --- tool dispatch (one active interaction at a time) -----------------
        if self.crop_mode {
            self.handle_crop(ui, &resp, xf, tex_size);
        } else if self.placing_mask.is_some() {
            self.handle_place_mask(ui, &resp, xf);
        } else if self.wb_picking {
            self.handle_wb_pick(ui, &resp, xf);
        } else if self.range_picking.is_some() {
            self.handle_range_pick(ui, &resp, xf);
        } else if self.clone_mode {
            self.handle_clone(ui, &resp, xf);
            self.ensure_mask_tex(ui.ctx());
            if let Some(t) = &self.mask_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
        } else if self.paint_mode {
            self.handle_paint(&resp, xf);
            self.ensure_mask_tex(ui.ctx());
            if let Some(t) = &self.mask_tex {
                ui.painter_at(rect).image(t.id(), rect, uv, egui::Color32::WHITE);
            }
        } else {
            // The selected mask's on-image knobs take priority over box-select
            // (a knob hit means "edit the mask", never "start a region").
            if !self.handle_mask_edit(ui, &resp, xf) {
                self.handle_region_select(ui, &resp, xf);
            }
        }

        // The committed AI region (and a drag in flight) paints EVERY frame it
        // exists — it still feeds the analyze prompt while a tool or a knob
        // hover owns the pointer, so hiding it there misrepresented what the
        // AI would see.
        {
            let stroke = egui::Stroke::new(2.0, ACCENT);
            let fill =
                egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 40);
            let draw = |r: egui::Rect| {
                ui.painter().rect_filled(r, 0.0, fill);
                ui.painter().rect_stroke(r, 0.0, stroke);
            };
            if let Some((s, e)) = self.region_drag {
                draw(egui::Rect::from_two_pos(s, e).intersect(rect));
            } else if let Some([l, t, rr, bb]) = self.region {
                // Stored in the original frame → back into the transformed view.
                let ((bw, bh), deg, dist) = self.geom_ctx();
                let a = orig_norm_to_view(l, t, (bw, bh), deg, &dist);
                let b2 = orig_norm_to_view(rr, bb, (bw, bh), deg, &dist);
                // from_two_pos, like the drag path above (L13-6): under a
                // straighten the transformed corners can swap order, and
                // from_min_max built a negative rect that was culled — the
                // committed region turned invisible while still feeding AI.
                draw(
                    egui::Rect::from_two_pos(xf.to_screen(a.0, a.1), xf.to_screen(b2.0, b2.1))
                        .intersect(rect),
                );
            }
        }

        // Selected mask stays visualised so its sliders have visual feedback
        // (geometry is stored in the original frame → map into the view),
        // with the editing knobs on top (drag = reshape/move, handle_mask_edit).
        // When one of the mask's COMPONENTS is selected, the outline and
        // knobs target THAT shape — the same geometry the edit handler
        // mutates (sel_target_geometry).
        if !self.crop_mode
            && self.placing_mask.is_none()
            && let Some(m) = self.sel_mask.and_then(|i| self.recipe.masks.get(i))
        {
            let target = match self.sel_component {
                Some(c) if c < m.components.len() => &m.components[c].geometry,
                _ => &m.mask,
            };
            let (dims, deg, dist) = self.geom_ctx();
            let vg = geom_to_view(target, dims, deg, &dist);
            // Includes the CA composite fill (Codex AL F1): the true
            // outline must warp whenever the pixels do.
            let geom_active =
                autoshop::render::geometry_moves_frame(&dist.profile, dist.amount);
            if let MaskGeometry::Radial { top, left, bottom, right, flipped, angle, .. } = target
                && (deg != 0.0 || geom_active)
            {
                // The engine evaluates the ellipse in the ORIGINAL frame; under
                // straighten/distortion its image is a rotated/warped curve —
                // not the axis-aligned ellipse the bbox mapping yields. Sample
                // it parametrically — the mask's OWN rotation folded in first,
                // the same order the engine applies — and draw the true
                // outline. (Knobs keep the bbox positions; HANDLE_HIT absorbs
                // the difference.)
                let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
                let (rx, ry) = ((right - left) / 2.0, (bottom - top) / 2.0);
                let (s_a, c_a) = angle.to_radians().sin_cos();
                let pts: Vec<egui::Pos2> = (0..48)
                    .map(|k| {
                        let th = k as f32 / 48.0 * std::f32::consts::TAU;
                        let (ex, ey) = (rx * th.cos(), ry * th.sin());
                        let (vx, vy) = orig_norm_to_view(
                            cx + ex * c_a - ey * s_a,
                            cy + ex * s_a + ey * c_a,
                            dims,
                            deg,
                            &dist,
                        );
                        xf.to_screen(vx, vy)
                    })
                    .collect();
                let p = ui.painter_at(xf.rect);
                p.add(egui::Shape::closed_line(pts, egui::Stroke::new(2.0, ACCENT)));
                // Same effect-side marker semantics as draw_mask_overlay.
                let (ccx, ccy) = orig_norm_to_view(cx, cy, dims, deg, &dist);
                let cs = xf.to_screen(ccx, ccy);
                if flipped ^ m.inverted {
                    p.circle_stroke(cs, 3.0, egui::Stroke::new(2.0, ACCENT));
                } else {
                    p.circle_filled(cs, 3.0, ACCENT);
                }
            } else {
                draw_mask_overlay(ui, xf, &vg, m.inverted, self.lang);
            }
            let p = ui.painter_at(xf.rect);
            for (h, pos) in mask_handle_points(&vg, xf) {
                let r = if h == 0 { 5.5 } else { 4.5 }; // centre knob reads bigger
                p.circle_filled(pos, r, egui::Color32::WHITE);
                p.circle_stroke(pos, r, egui::Stroke::new(1.5, ACCENT));
            }
        }
    }

    /// WB eyedropper: click a pixel that SHOULD be neutral and the engine's
    /// inverse solver (`render::solve_wb_from_neutral` — the same forward
    /// model the render applies, anchored at this photo's stamped as-shot
    /// Kelvin, or the legacy 5500 K) turns it into Temp + Tint.
    /// Samples a 5×5 mean of the SOURCE preview: WB runs before develop, so
    /// the solve must see pre-develop pixels, not the current edit.
    pub(crate) fn handle_wb_pick(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if !resp.clicked() {
            return;
        }
        let Some(q) = resp.interact_pointer_pos() else { return };
        let (nx, ny) = xf.to_norm(q);
        // base_preview is the ORIGINAL frame — map out of the transformed view.
        let ((bw, bh), deg, dist) = self.geom_ctx();
        let (nx, ny) = view_norm_to_orig(nx, ny, (bw, bh), deg, &dist);
        let px = {
            let Some(base) = &self.base_preview else { return };
            let Some(px) = sample_5x5_mean(base, nx, ny) else { return };
            px
        };
        let (k, tint) = autoshop::render::solve_wb_from_neutral(
            px,
            self.recipe.as_shot_k.unwrap_or(5500.0),
        );
        self.recipe.temperature_k = Some(k);
        self.recipe.tint = tint;
        self.wb_picking = false;
        self.dirty = true;
        self.status = trf(
            self.lang,
            "WB eyedropper: {k} K · tint {tint} — fine-tune in the Tone section",
            &[("k", &format!("{k:.0}")), ("tint", &format!("{tint:+.0}"))],
        );
    }

    /// Colour-range sample: click keys the pending mask's Color range to that
    /// spot. Samples a 5×5 mean of a PRE-MASK develop (this recipe with masks
    /// stripped) — the exact pixel state `apply_masks` evaluates range weights
    /// against, so the picked colour is what the engine will match. One extra
    /// preview-sized develop per click ≈ the cost of one slider tick.
    pub(crate) fn handle_range_pick(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if !resp.clicked() {
            return;
        }
        let Some(q) = resp.interact_pointer_pos() else { return };
        let Some(mi) = self.range_picking.filter(|&i| i < self.recipe.masks.len()) else {
            self.range_picking = None; // stale index (mask deleted mid-pick)
            return;
        };
        let (nx, ny) = xf.to_norm(q);
        // develop_preview works in the ORIGINAL frame — map out of the view.
        let ((bw, bh), deg, dist) = self.geom_ctx();
        let (nx, ny) = view_norm_to_orig(nx, ny, (bw, bh), deg, &dist);
        let smp = {
            let Some(base) = self.base_preview.clone() else { return };
            // The PREFIX reference (masks before this one — the engine
            // evaluates a range on the pixel as it stands when THIS mask
            // runs), built EXACTLY like the coverage overlay's (geometry
            // fields stripped — develop_preview has no geometry stage,
            // they'd only break the cache key) and SHARING its cache: with
            // the overlay on, a click costs a 5×5 read instead of a full
            // preview develop on the UI thread — and a miss here pre-warms
            // the overlay's next rebuild. A masks-CLEARED reference judged
            // mask 2's range on pixels mask 0/1 had already moved (CX5-6).
            let mut pre = self.recipe.clone();
            pre.masks.truncate(mi);
            pre.straighten_deg = 0.0;
            pre.lens_distortion = 0.0;
            pre.crop = None;
            if !matches!(&self.overlay_ref, Some((r, _)) if *r == pre) {
                let img = autoshop::render::develop_preview(&base, &pre);
                self.overlay_ref = Some((pre, img));
            }
            let reference = &self.overlay_ref.as_ref().expect("range reference cached").1;
            let Some(smp) = sample_5x5_mean(reference, nx, ny) else { return };
            smp
        };
        // Keep the tolerance the user already dialled in; only re-key the colour.
        let amount = match self.recipe.masks[mi].range {
            Some(RangeMask::Color { amount, .. }) => amount,
            _ => 0.5,
        };
        self.recipe.masks[mi].range =
            Some(RangeMask::Color { r: smp[0], g: smp[1], b: smp[2], amount, px: nx, py: ny });
        self.range_picking = None;
        self.dirty = true;
        self.status = tr(
            self.lang,
            "Color range: sampled — the 「Tolerance」 slider adjusts the selection width",
        )
        .into();
    }

    /// Box-select on the After image: drag a rectangle to target a local edit;
    /// the normalized box is folded into the AI direction so it masks exactly
    /// there (mirrors the web region→mask prompt). A plain click — or a tiny
    /// drag — clears the selection. Coordinates are full-frame normalized (the
    /// AI mask space), mapped through the view transform.
    /// On-image editing of the SELECTED mask's geometry (the LR gesture):
    /// drag an end/edge knob to reshape, the centre knob to move the whole
    /// mask — no more redraw-from-scratch via 重画. Geometry lives in the
    /// ORIGINAL frame, so every write maps the pointer back through
    /// view_norm_to_orig — the same chain placement uses. Returns true while
    /// it owns the pointer (hovering a knob or mid-drag) so box-select
    /// doesn't also react; a box-select drag already in flight keeps
    /// priority (else its live rectangle would freeze mid-air whenever the
    /// pointer crossed a knob).
    /// The geometry the canvas tools currently target: the selected mask's
    /// selected COMPONENT when one is selected (and still in bounds), else
    /// the selected mask's base geometry.
    pub(crate) fn sel_target_geometry(&self) -> Option<&MaskGeometry> {
        let i = self.sel_mask.filter(|&i| i < self.recipe.masks.len())?;
        let m = &self.recipe.masks[i];
        match self.sel_component {
            Some(c) if c < m.components.len() => Some(&m.components[c].geometry),
            _ => Some(&m.mask),
        }
    }

    pub(crate) fn sel_target_geometry_mut(&mut self) -> Option<&mut MaskGeometry> {
        let i = self.sel_mask.filter(|&i| i < self.recipe.masks.len())?;
        let m = &mut self.recipe.masks[i];
        match self.sel_component {
            Some(c) if c < m.components.len() => Some(&mut m.components[c].geometry),
            _ => Some(&mut m.mask),
        }
    }

    pub(crate) fn handle_mask_edit(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) -> bool {
        if self.region_drag.is_some() {
            return false;
        }
        let Some(target_geom) = self.sel_target_geometry().cloned() else {
            self.mask_drag = None;
            return false;
        };
        let (dims, deg, dist) = self.geom_ctx();
        let view_geom = geom_to_view(&target_geom, dims, deg, &dist);
        let handles = mask_handle_points(&view_geom, xf);
        if handles.is_empty() {
            self.mask_drag = None; // bitmap: nothing parametric to drag
            return false;
        }
        // A stranded grab (Esc / hold-B / pan swallowed the drag_stopped frame)
        // must not turn the NEXT unrelated drag into a mask reshape: no drag in
        // flight ⇒ no grab.
        if !resp.dragged() && !resp.drag_started() {
            self.mask_drag = None;
        }
        // Nearest handle wins, ties broken toward the edge/end knobs — the
        // centre-move knob is emitted first and used to shadow them whenever a
        // small mask put everything within HANDLE_HIT.
        let pick = |p: egui::Pos2| {
            handles
                .iter()
                .filter(|(_, hp)| hp.distance(p) <= HANDLE_HIT)
                .min_by(|a, b| {
                    a.1.distance(p).total_cmp(&b.1.distance(p)).then(b.0.cmp(&a.0))
                })
                .map(|(h, _)| *h)
        };
        let hover_h = resp.hover_pos().and_then(pick);
        let orig_at = |p: egui::Pos2| {
            let (nx, ny) = xf.to_norm(p);
            view_norm_to_orig(nx, ny, dims, deg, &dist)
        };
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // egui reports drag_started only after ~6 px of travel — a fast
            // flick has left the knob by then. Hit-test the PRESS ORIGIN so
            // quick grabs don't fall through to box-select — and ANCHOR the
            // delta there too: anchoring at the current pointer discarded
            // the lead-in motion (a short drag left the edge unmoved).
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if let Some(h) = pick(origin) {
                self.mask_drag = Some((h, orig_at(origin)));
            }
        }
        if resp.dragged()
            && let (Some((h, last)), Some(p)) = (self.mask_drag, resp.interact_pointer_pos())
        {
            let cur = orig_at(p);
            let (dx, dy) = (cur.0 - last.0, cur.1 - last.1);
            // LR allows geometry to start off-canvas; a generous band keeps
            // knobs recoverable instead of letting them fly to infinity.
            let cl = |v: f32| v.clamp(-0.5, 1.5);
            match self.sel_target_geometry_mut() {
                Some(MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }) => match h {
                    1 => (*zero_x, *zero_y) = (cl(cur.0), cl(cur.1)),
                    2 => (*full_x, *full_y) = (cl(cur.0), cl(cur.1)),
                    _ => {
                        *zero_x = cl(*zero_x + dx);
                        *zero_y = cl(*zero_y + dy);
                        *full_x = cl(*full_x + dx);
                        *full_y = cl(*full_y + dy);
                    }
                },
                Some(MaskGeometry::Radial { top, left, bottom, right, angle, .. }) => {
                    const MIN_SIZE: f32 = 0.01;
                    // Edges move by the pointer's original-space DELTA, not to
                    // its absolute position: the displayed handles sit on the
                    // transformed BOUNDING BOX (≠ the true transformed ellipse
                    // under straighten/distortion), so an absolute assign made
                    // the edge jump to close that gap on the first drag frame.
                    // Under neutral geometry the two are identical.
                    //
                    // With a rotated mask the four knobs sit on the ROTATED
                    // axis endpoints, so the delta is projected onto the
                    // mask's own axes first — at angle 0 (du, dv) ≡ (dx, dy).
                    let (s_a, c_a) = angle.to_radians().sin_cos();
                    let du = dx * c_a + dy * s_a;
                    let dv = -dx * s_a + dy * c_a;
                    match h {
                        1 => *left = cl(*left + du).min(*right - MIN_SIZE),
                        2 => *top = cl(*top + dv).min(*bottom - MIN_SIZE),
                        3 => *right = cl(*right + du).max(*left + MIN_SIZE),
                        4 => *bottom = cl(*bottom + dv).max(*top + MIN_SIZE),
                        5 => {
                            // Rotation grip: it is parked toward the TOP axis
                            // endpoint, so the ellipse angle is the pointer's
                            // bearing from the centre + 90° (same rotation
                            // convention as the engine — recipe.rs Radial).
                            let (cx, cy) = ((*left + *right) / 2.0, (*top + *bottom) / 2.0);
                            let th =
                                (cur.1 - cy).atan2(cur.0 - cx).to_degrees() + 90.0;
                            *angle = th.rem_euclid(360.0);
                            if *angle > 180.0 {
                                *angle -= 360.0;
                            }
                        }
                        _ => {
                            // Clamp the SHIFT, not each edge — independent
                            // clamps at the band boundary squashed the
                            // ellipse toward zero size instead of parking it.
                            let (lx, hx) = (-0.5 - *left, 1.5 - *right);
                            let (ly, hy) = (-0.5 - *top, 1.5 - *bottom);
                            let dx = dx.clamp(lx.min(hx), lx.max(hx));
                            let dy = dy.clamp(ly.min(hy), ly.max(hy));
                            *left += dx;
                            *right += dx;
                            *top += dy;
                            *bottom += dy;
                        }
                    }
                }
                _ => {}
            }
            self.mask_drag = Some((h, cur));
            self.dirty = true; // masks are develop stages — live preview
            self.overlay_stale = true;
        }
        if resp.drag_stopped() {
            self.mask_drag = None; // commit_if_settled turns the drag into ONE undo step
        }
        if self.mask_drag.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hover_h.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        self.mask_drag.is_some() || hover_h.is_some()
    }

    pub(crate) fn handle_region_select(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                // Anchor at the PRESS ORIGIN: egui reports drag_started only
                // after ~6 px of travel, so anchoring at the current pointer
                // shaved that lead-in off every box (small fast selections
                // could then die to the 0.02 minimum below).
                let s = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
                self.region_drag = Some((s, p));
            }
        } else if resp.dragged() {
            if let (Some(p), Some((s, _))) = (resp.interact_pointer_pos(), self.region_drag) {
                self.region_drag = Some((s, p));
            }
        } else if resp.drag_stopped() {
            if let Some((s, e)) = self.region_drag.take() {
                // The region feeds the AI's mask prompt — ORIGINAL frame space.
                // ALL FOUR corners map (the same policy as serve's
                // region_to_original): under rotation/distortion the two
                // diagonal corners alone describe a different — sometimes
                // near-degenerate — box than the rectangle the user drew.
                let ((bw, bh), deg, dist) = self.geom_ctx();
                let map = |p: egui::Pos2| {
                    let (nx, ny) = xf.to_norm(p);
                    view_norm_to_orig(nx, ny, (bw, bh), deg, &dist)
                };
                let corners = [
                    map(s),
                    map(e),
                    map(egui::pos2(s.x, e.y)),
                    map(egui::pos2(e.x, s.y)),
                ];
                let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for (x, y) in corners {
                    l = l.min(x);
                    t = t.min(y);
                    r = r.max(x);
                    b = b.max(y);
                }
                if r - l > 0.02 && b - t > 0.02 {
                    self.region = Some([l, t, r, b]);
                    self.status = trf(
                        self.lang,
                        "region {w}×{h}% — type a direction, then AI Analyze (click to clear)",
                        &[
                            ("w", &(((r - l) * 100.0).round() as i32).to_string()),
                            ("h", &(((b - t) * 100.0).round() as i32).to_string()),
                        ],
                    );
                } else {
                    self.region = None; // a tiny drag clears the selection
                }
            }
        } else if resp.clicked() && !resp.double_clicked() {
            // Remember what this click cleared: if it turns out to be the
            // first half of a Fit↔1:1 double-click, after_view restores it
            // (M18 — zooming must not eat the AI region).
            //
            // `&& !resp.double_clicked()`: egui sets `clicked` on the release
            // that COMPLETES the pair too, and after_view restores the region
            // earlier in this same frame — so without this the restore was
            // taken straight back out and the double-click still ate the
            // region. The fix was inert until this line existed.
            self.region_restore = self.region.take().map(|r| (r, Instant::now()));
        }
        // (Drawing lives in after_view so the box shows under every tool —
        // this handler only owns the gesture.)
    }

    /// The interactive crop overlay: darkened surround, thirds grid, corner +
    /// edge handles (aspect-constrained), move-inside, and drag OUTSIDE the
    /// box to rotate-straighten (the LR gesture set). The crop lives in
    /// `recipe.crop` (full-frame normalized) — exactly what the export render
    /// and the XMP already apply, so the tool adds no new data path.
    pub(crate) fn handle_crop(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        xf: ViewXform,
        tex_size: egui::Vec2,
    ) {
        use autoshop::recipe::Crop;
        // Pixel aspect ratio (w/h) requested by the preset; "原始" resolves here.
        // `tex_size` is the CURRENT after texture: right after a straighten
        // change with the develop still in flight it can be one frame stale,
        // and a preset applied in that instant derives against the previous
        // frame's dims (recorded residual, CX5-5 — recomputing inscribed
        // dims here would duplicate the engine's geometry; the next
        // interaction re-derives correctly).
        let aspect = CROP_ASPECTS[self.crop_aspect.min(CROP_ASPECTS.len() - 1)]
            .1
            .map(|r| if r == 0.0 { tex_size.x / tex_size.y.max(1.0) } else { r });
        // A preset picked in the panel re-derives the box NOW (LR applies the
        // aspect immediately, D13): largest same-centre fit at the new
        // ratio, shrunk to stay in frame. "Free" keeps the box untouched.
        if std::mem::take(&mut self.crop_aspect_pending)
            && let Some(r_px) = aspect
        {
            let rn = r_px * tex_size.y.max(1.0) / tex_size.x.max(1.0);
            let c0 = self
                .recipe
                .crop
                .map(|c| [c.left, c.top, c.right, c.bottom])
                .unwrap_or([0.0, 0.0, 1.0, 1.0]);
            let (cx, cy) = ((c0[0] + c0[2]) / 2.0, (c0[1] + c0[3]) / 2.0);
            let (mut w, mut h) = (c0[2] - c0[0], c0[3] - c0[1]);
            if w / h.max(1e-6) > rn {
                w = h * rn;
            } else {
                h = w / rn.max(1e-6);
            }
            let s = ((2.0 * cx.min(1.0 - cx)) / w.max(1e-6))
                .min((2.0 * cy.min(1.0 - cy)) / h.max(1e-6))
                .min(1.0);
            let (w, h) = (w * s, h * s);
            // The drag path refuses boxes under 0.05 a side — the preset
            // path must not create what the handles could then never
            // reproduce (Codex batch-38). Growing can shift the box off its
            // centre at the frame edge, so the position re-clamps after.
            let grow = (0.05_f32 / w.max(1e-6)).max(0.05_f32 / h.max(1e-6)).max(1.0);
            let (w, h) = ((w * grow).min(1.0), (h * grow).min(1.0));
            let left = (cx - w / 2.0).clamp(0.0, 1.0 - w);
            let top = (cy - h / 2.0).clamp(0.0, 1.0 - h);
            let next = Some(Crop { left, top, right: left + w, bottom: top + h });
            if self.recipe.crop != next {
                self.recipe.crop = next;
                self.dirty = true; // histogram/clipping follow the crop
            }
        }
        let cur = self
            .recipe
            .crop
            .map(|c| [c.left, c.top, c.right, c.bottom])
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);

        // Handle order: 0=TL 1=TR 2=BL 3=BR, 4=move (inside), 5=T 6=B 7=L 8=R
        // edge midpoints, 9=rotate (anywhere outside the box).
        const HIT: f32 = 12.0; // handle pick radius, px — shared by drag + cursor
        let handle_pos = |c: &[f32; 4], k: u8| match k {
            0 => xf.to_screen(c[0], c[1]),
            1 => xf.to_screen(c[2], c[1]),
            2 => xf.to_screen(c[0], c[3]),
            3 => xf.to_screen(c[2], c[3]),
            5 => xf.to_screen((c[0] + c[2]) / 2.0, c[1]),
            6 => xf.to_screen((c[0] + c[2]) / 2.0, c[3]),
            7 => xf.to_screen(c[0], (c[1] + c[3]) / 2.0),
            _ => xf.to_screen(c[2], (c[1] + c[3]) / 2.0),
        };
        // Corners win over edges on a tiny box (checked first); inside = move;
        // anywhere else on the canvas = rotate-straighten, so the pick is
        // total — implicit pan is already suppressed while crop is armed.
        let pick_handle = |c: &[f32; 4], p: egui::Pos2| {
            (0u8..4)
                .chain(5u8..9)
                .find(|&k| handle_pos(c, k).distance(p) <= HIT)
                .or_else(|| {
                    let r = egui::Rect::from_min_max(
                        xf.to_screen(c[0], c[1]),
                        xf.to_screen(c[2], c[3]),
                    );
                    // A box flush with the canvas (a fresh full-frame crop)
                    // has no outside — the promised rotate-straighten gesture
                    // was unreachable. Only a FULLY flush box donates inner
                    // bands (moving it is a no-op anyway, D13); a box merely
                    // touching ONE canvas edge still has real outside to
                    // rotate from, and per-edge donation turned a move near
                    // that edge into a surprise rotate (M20).
                    const BAND: f32 = 16.0;
                    let mut inner = r.intersect(xf.rect);
                    let full_frame = r.min.x <= xf.rect.min.x + 1.0
                        && r.min.y <= xf.rect.min.y + 1.0
                        && r.max.x >= xf.rect.max.x - 1.0
                        && r.max.y >= xf.rect.max.y - 1.0;
                    if full_frame {
                        inner.min.x += BAND;
                        inner.min.y += BAND;
                        inner.max.x -= BAND;
                        inner.max.y -= BAND;
                    }
                    Some(if inner.contains(p) { 4 } else { 9 })
                })
        };
        // A stranded grab (Esc mid-drag, or leaving/re-entering crop mode)
        // must not resume against a stale anchor: no drag in flight ⇒ no grab.
        if !resp.dragged() && !resp.drag_started() {
            self.crop_drag = None;
        }
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Hit-test the PRESS ORIGIN — drag_started fires only after ~6 px
            // of travel, and a fast corner grab has already left the handle.
            // The drag ANCHOR is the origin too: anchoring at the current
            // pointer discarded the lead-in motion (and let the dominant-axis
            // pick follow a small residual instead of the user's gesture).
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if let Some(h) = pick_handle(&cur, origin) {
                self.crop_drag = Some((h, origin, cur, self.recipe.straighten_deg));
            }
        }
        if resp.dragged()
            && let (Some((h, start, orig, deg0)), Some(p)) =
                (self.crop_drag, resp.interact_pointer_pos())
        {
            if h == 9 {
                // Rotate-straighten: drag outside the box (LR). Angle around
                // the box centre, screen space — y-down atan2 makes clockwise
                // positive, matching the engine's clockwise straighten °; the
                // image turns WITH the drag. The box mapping is unaffected
                // mid-drag (recipe.crop lives in the post-rotation view frame).
                let a = xf.to_screen(orig[0], orig[1]);
                let b = xf.to_screen(orig[2], orig[3]);
                let centre = egui::pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                let ang = |q: egui::Pos2| (q.y - centre.y).atan2(q.x - centre.x);
                // Wrap the delta into (-180°, 180°]: crossing atan2's ±180°
                // branch cut otherwise reads a 2° clockwise move near the
                // leftward ray as −358° and slams the slider to the clamp.
                let delta = (ang(p) - ang(start)).to_degrees();
                let delta = (delta + 540.0) % 360.0 - 180.0;
                let next = (deg0 + delta).clamp(-45.0, 45.0);
                if self.recipe.straighten_deg != next {
                    self.recipe.straighten_deg = next;
                    self.dirty = true;
                }
            } else {
                let (sn, pn) = (xf.to_norm(start), xf.to_norm(p));
                let (dx, dy) = (pn.0 - sn.0, pn.1 - sn.1);
                // The preset is a PIXEL w/h ratio; geometry below works in
                // normalised view space, so convert once.
                let ratio_n =
                    aspect.map(|r_px| r_px * tex_size.y.max(1.0) / tex_size.x.max(1.0));
                let c: [f32; 4] = if h == 4 {
                    // Move: shift, clamped so the rect stays inside the frame.
                    let (w, hg) = (orig[2] - orig[0], orig[3] - orig[1]);
                    let nl = (orig[0] + dx).clamp(0.0, 1.0 - w);
                    let nt = (orig[1] + dy).clamp(0.0, 1.0 - hg);
                    [nl, nt, nl + w, nt + hg]
                } else if h == 5 || h == 6 {
                    // Top/bottom edge: anchored at the opposite edge. Free
                    // aspect moves only this edge; a fixed aspect rederives
                    // the width centred on the box's vertical axis (the LR
                    // edge behaviour), shrinking to stay in frame.
                    let ay = if h == 5 { orig[3] } else { orig[1] };
                    let y = (if h == 5 { orig[1] } else { orig[3] } + dy).clamp(0.0, 1.0);
                    let mut h_n = (y - ay).abs();
                    let (mut l, mut r) = (orig[0], orig[2]);
                    if let Some(rn) = ratio_n {
                        let cx = (orig[0] + orig[2]) / 2.0;
                        let mut w_n = h_n * rn;
                        let room = 2.0 * cx.min(1.0 - cx);
                        if w_n > room {
                            w_n = room;
                            h_n = w_n / rn;
                        }
                        l = cx - w_n / 2.0;
                        r = cx + w_n / 2.0;
                    }
                    let y = if h == 5 { ay - h_n } else { ay + h_n };
                    [l, y.min(ay), r, y.max(ay)]
                } else if h == 7 || h == 8 {
                    // Left/right edge — the transpose of the above.
                    let ax = if h == 7 { orig[2] } else { orig[0] };
                    let x = (if h == 7 { orig[0] } else { orig[2] } + dx).clamp(0.0, 1.0);
                    let mut w_n = (x - ax).abs();
                    let (mut t, mut b) = (orig[1], orig[3]);
                    if let Some(rn) = ratio_n {
                        let cy = (orig[1] + orig[3]) / 2.0;
                        let mut h_n = w_n / rn;
                        let room = 2.0 * cy.min(1.0 - cy);
                        if h_n > room {
                            h_n = room;
                            w_n = h_n * rn;
                        }
                        t = cy - h_n / 2.0;
                        b = cy + h_n / 2.0;
                    }
                    let x = if h == 7 { ax - w_n } else { ax + w_n };
                    [x.min(ax), t, x.max(ax), b]
                } else {
                    // Corner: drag it, anchored at the opposite corner.
                    let (ax, ay) = match h {
                        0 => (orig[2], orig[3]),
                        1 => (orig[0], orig[3]),
                        2 => (orig[2], orig[1]),
                        _ => (orig[0], orig[1]),
                    };
                    let mut x = (match h {
                        0 | 2 => orig[0] + dx,
                        _ => orig[2] + dx,
                    })
                    .clamp(0.0, 1.0);
                    let mut y = (match h {
                        0 | 1 => orig[1] + dy,
                        _ => orig[3] + dy,
                    })
                    .clamp(0.0, 1.0);
                    if let Some(rn) = ratio_n {
                        // The DOMINANT drag axis drives and the other follows
                        // the ratio. Dominance comes from the DRAG DELTAS (in
                        // width units), not max(extents): max() can only grow
                        // past the stale axis, so an inward pull dominated by
                        // one axis left the corner pinned at the old size.
                        // The Y side follows the POINTER relative to the
                        // anchor, exactly like x below (L13-8): pinning it to
                        // the handle's original identity meant a TL corner
                        // dragged past BR rebuilt the box on the anchor's far
                        // side, away from the pointer — the un-ratio'd path's
                        // min/max already handles crossing, and the asymmetry
                        // was the defect.
                        let above = y < ay;
                        let mut w_n = if dx.abs() >= dy.abs() * rn {
                            (x - ax).abs()
                        } else {
                            (y - ay).abs() * rn
                        };
                        let mut h_n = w_n / rn;
                        let room = if above { ay } else { 1.0 - ay };
                        if h_n > room {
                            h_n = room;
                            w_n = h_n * rn;
                        }
                        // Horizontal room too: a dominantly VERTICAL pull can
                        // derive a width wider than the frame leaves on the
                        // driven side — x would land outside 0..1 and the
                        // final min/max can't repair that.
                        let x_room = if x >= ax { 1.0 - ax } else { ax };
                        if w_n > x_room {
                            w_n = x_room;
                            h_n = w_n / rn;
                        }
                        x = if x >= ax { ax + w_n } else { ax - w_n };
                        y = if above { ay - h_n } else { ay + h_n };
                    }
                    [x.min(ax), y.min(ay), x.max(ax), y.max(ay)]
                };
                if c[2] - c[0] >= 0.05 && c[3] - c[1] >= 0.05 {
                    let next = Some(Crop { left: c[0], top: c[1], right: c[2], bottom: c[3] });
                    // The crop IS a develop input: build_preview restricts the
                    // histogram + clipping to it, so a crop change that never
                    // sets `dirty` leaves both describing the OLD crop until
                    // some unrelated slider is touched.
                    if self.recipe.crop != next {
                        self.recipe.crop = next;
                        self.dirty = true;
                    }
                }
            }
        }
        if resp.drag_stopped() {
            self.crop_drag = None;
        }

        // Cursor affordance: name the resize direction of the handle under
        // the pointer — or of the one being DRAGGED, since the pointer can
        // lag off a corner mid-drag. Runs after show_image's generic
        // crosshair set, and cursor_icon is last-write-wins, so this
        // overrides it exactly when a handle would take the drag.
        let c = self
            .recipe
            .crop
            .map(|c| [c.left, c.top, c.right, c.bottom])
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let hover_handle = self
            .crop_drag
            .map(|(h, ..)| h)
            .or_else(|| resp.hover_pos().and_then(|p| pick_handle(&c, p)));
        if let Some(h) = hover_handle {
            ui.ctx().set_cursor_icon(match h {
                0 | 3 => egui::CursorIcon::ResizeNwSe, // TL/BR diagonal
                1 | 2 => egui::CursorIcon::ResizeNeSw, // TR/BL diagonal
                5 | 6 => egui::CursorIcon::ResizeVertical, // top/bottom edge
                7 | 8 => egui::CursorIcon::ResizeHorizontal, // left/right edge
                4 => egui::CursorIcon::Move,           // inside: move the window
                _ => egui::CursorIcon::Grab,           // outside: rotate-straighten
            });
        }

        // --- overlay: darkened surround + thirds + handles --------------------
        let p = ui.painter_at(xf.rect);
        // The TRUE box for guides + border (the painter clips): computing
        // the thirds on the viewport-intersected rect drew them at thirds of
        // the VISIBLE PORTION whenever zoom pushed part of the box
        // off-screen (CX5-8). The shading keeps the intersected rect — its
        // job is exactly the visible surround.
        let r_full =
            egui::Rect::from_min_max(xf.to_screen(c[0], c[1]), xf.to_screen(c[2], c[3]));
        let r = r_full.intersect(xf.rect);
        let dark = egui::Color32::from_black_alpha(140);
        let full = xf.rect;
        for shade in [
            egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, r.min.y)), // top
            egui::Rect::from_min_max(egui::pos2(full.min.x, r.max.y), full.max), // bottom
            egui::Rect::from_min_max(egui::pos2(full.min.x, r.min.y), egui::pos2(r.min.x, r.max.y)),
            egui::Rect::from_min_max(egui::pos2(r.max.x, r.min.y), egui::pos2(full.max.x, r.max.y)),
        ] {
            if shade.width() > 0.0 && shade.height() > 0.0 {
                p.rect_filled(shade, 0.0, dark);
            }
        }
        // O cycles the guide while cropping (LR): thirds → golden → off.
        let guide: &[f32] = match self.crop_grid {
            0 => &[1.0 / 3.0, 2.0 / 3.0],
            1 => &[0.381_966, 0.618_034], // 1−1/φ and 1/φ
            _ => &[],
        };
        let grid = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70));
        for &t in guide {
            p.line_segment(
                [
                    egui::pos2(r_full.min.x + t * r_full.width(), r_full.min.y),
                    egui::pos2(r_full.min.x + t * r_full.width(), r_full.max.y),
                ],
                grid,
            );
            p.line_segment(
                [
                    egui::pos2(r_full.min.x, r_full.min.y + t * r_full.height()),
                    egui::pos2(r_full.max.x, r_full.min.y + t * r_full.height()),
                ],
                grid,
            );
        }
        p.rect_stroke(r_full, 0.0, egui::Stroke::new(1.5, egui::Color32::WHITE));
        for k in 0..4u8 {
            p.rect_filled(
                egui::Rect::from_center_size(handle_pos(&c, k), egui::vec2(9.0, 9.0)),
                1.0,
                egui::Color32::WHITE,
            );
        }
        // Edge midpoints as slim pills, so the affordance reads as "this edge"
        // rather than a fifth corner.
        for k in [5u8, 6, 7, 8] {
            let sz = if k <= 6 { egui::vec2(15.0, 5.0) } else { egui::vec2(5.0, 15.0) };
            p.rect_filled(
                egui::Rect::from_center_size(handle_pos(&c, k), sz),
                1.0,
                egui::Color32::WHITE,
            );
        }
    }

    /// Place (or re-draw) a manual local-adjustment mask by dragging: the drag
    /// vector defines a linear gradient (press = full-strength side, LR's
    /// direction; Shift = axis lock) or the
    /// bounding box of a radial. Commits into `recipe.masks` — the SAME field
    /// the AI writes, so render + XMP need nothing new.
    pub(crate) fn handle_place_mask(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        let Some((kind, target)) = self.placing_mask else { return };
        // Mask geometry lives in the ORIGINAL frame (the engine composites
        // masks before the geometric remap) — map pointer positions out of the view.
        let (dims, deg, dist) = self.geom_ctx();
        // place_start holds VIEW-normalized coords (not original-frame): the
        // radial bbox below must map ALL FOUR view-space corners — under
        // rotation/distortion the two dragged diagonals alone can describe a
        // thin or zero-area original-space box (the region handler maps all
        // four for exactly this reason).
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Anchor at the PRESS ORIGIN: drag_started fires ~6 px in, and
            // that lead-in shortened every gradient / shifted tiny radials.
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            self.place_start = Some(xf.to_norm(start));
        }
        let Some(sv) = self.place_start else { return };
        let Some(p) = resp.interact_pointer_pos() else { return };
        let mut ev = xf.to_norm(p);
        // LR: Shift locks a linear gradient to horizontal / vertical.
        // Snapped in VIEW space (what the user sees) — under straighten the
        // original-frame vector is legitimately off-axis (D13).
        if kind == MaskKind::Linear && ui.input(|i| i.modifiers.shift) {
            if (ev.0 - sv.0).abs() >= (ev.1 - sv.1).abs() {
                ev.1 = sv.1;
            } else {
                ev.0 = sv.0;
            }
        }
        let geom = match kind {
            MaskKind::Linear => {
                // Endpoints map exactly — two points suffice for a gradient.
                // LR anchors the FULL effect at the press and fades toward
                // the release (D13): press = full side, release = zero side.
                let s = view_norm_to_orig(sv.0, sv.1, dims, deg, &dist);
                let e = view_norm_to_orig(ev.0, ev.1, dims, deg, &dist);
                autoshop::recipe::MaskGeometry::Linear {
                    zero_x: e.0,
                    zero_y: e.1,
                    full_x: s.0,
                    full_y: s.1,
                }
            }
            MaskKind::Radial => {
                // Under straighten/distortion the dragged view-rect has NO
                // exact preimage in the model — Radial carries no rotation —
                // so the committed area is the bbox of the mapped corners: a
                // SUPERSET of the drag (the effect covers everything pointed
                // at; the alternative, axis-preserving, would miss corners).
                // The selected-mask outline then shows the TRUE transformed
                // ellipse (parametric sampler in draw_masks) and the edge
                // handles move by deltas, so the approximation never
                // compounds. Identity when geometry is neutral.
                let corners = [(sv.0, sv.1), (ev.0, ev.1), (sv.0, ev.1), (ev.0, sv.1)]
                    .map(|(x, y)| view_norm_to_orig(x, y, dims, deg, &dist));
                let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for (x, y) in corners {
                    l = l.min(x);
                    t = t.min(y);
                    r = r.max(x);
                    b = b.max(y);
                }
                // A degenerate box is useless and invisible — pad to the same
                // MIN_SIZE floor the edge handles enforce.
                const MIN_SIZE: f32 = 0.01;
                if r - l < MIN_SIZE {
                    let c = (l + r) * 0.5;
                    l = c - MIN_SIZE * 0.5;
                    r = c + MIN_SIZE * 0.5;
                }
                if b - t < MIN_SIZE {
                    let c = (t + b) * 0.5;
                    t = c - MIN_SIZE * 0.5;
                    b = c + MIN_SIZE * 0.5;
                }
                autoshop::recipe::MaskGeometry::Radial {
                    top: t,
                    left: l,
                    bottom: b,
                    right: r,
                    feather: 0.5,
                    roundness: 0.0,
                    flipped: false,
                    angle: 0.0,
                }
            }
        };
        // Live placement preview: no owning adjustment yet → markers upright.
        draw_mask_overlay(ui, xf, &geom_to_view(&geom, dims, deg, &dist), false, self.lang);
        if resp.drag_stopped() {
            let status: String = match target {
                PlaceTarget::Redraw(i) if i < self.recipe.masks.len() => {
                    // Redraw replaces the AREA only — a radial's tuned feather /
                    // roundness / flipped / angle are slider+handle state, and
                    // silently resetting them made ↻ Redraw destructive.
                    let mut geom = geom;
                    if let (
                        autoshop::recipe::MaskGeometry::Radial {
                            feather, roundness, flipped, angle, ..
                        },
                        autoshop::recipe::MaskGeometry::Radial {
                            feather: kept_f,
                            roundness: kept_r,
                            flipped: kept_fl,
                            angle: kept_a,
                            ..
                        },
                    ) = (&mut geom, &self.recipe.masks[i].mask)
                    {
                        *feather = *kept_f;
                        *roundness = *kept_r;
                        *flipped = *kept_fl;
                        *angle = *kept_a;
                    }
                    self.recipe.masks[i].mask = geom;
                    // A REDRAW kept the mask's existing (possibly nonzero)
                    // sliders — the "all 0 now" line below is only true for a
                    // brand-new mask.
                    tr(self.lang, "mask area redrawn — its existing adjustments now apply to the new area").into()
                }
                PlaceTarget::Component(i, mode) if i < self.recipe.masks.len() => {
                    let m = &mut self.recipe.masks[i];
                    m.components.push(autoshop::recipe::MaskComponent { geometry: geom, mode });
                    self.sel_mask = Some(i);
                    self.sel_component = Some(m.components.len() - 1);
                    tr(self.lang, "shape added to this mask — drag its knobs to adjust; the shape list is under the mask's row").into()
                }
                // NewMask — and the two index-carrying targets whose mask
                // vanished mid-drag (an async recipe replace): a redraw
                // degrades to a fresh mask (pre-v0.22 behaviour); a component
                // with no owner joins it rather than vanishing silently.
                _ => {
                    let n = self.recipe.masks.len();
                    let name = trf(self.lang, "Manual {n}", &[("n", &(n + 1).to_string())]);
                    self.recipe.masks.push(autoshop::recipe::LocalAdjustment {
                        mask: geom,
                        name,
                        ..Default::default()
                    });
                    self.sel_mask = Some(n);
                    self.sel_component = None;
                    tr(self.lang, "mask placed — pull its sliders in 「Local Masks」 at left (all 0 now, no visible effect yet)").into()
                }
            };
            self.placing_mask = None;
            self.place_start = None;
            self.dirty = true;
            self.overlay_stale = true;
            self.status = status;
        }
    }

    pub(crate) fn ensure_mask_tex(&mut self, ctx: &egui::Context) {
        // The canvas lives in the ORIGINAL frame (strokes are mapped there so
        // fill/heal edit source pixels), but the texture is blitted over the
        // After image, which has been through distortion + straighten — the
        // overlay must go through the SAME remap or every stroke displays
        // offset under a straightened view. Rebuild when the transform moved,
        // not only when the canvas was painted.
        // The overlay's remap depends on the PROFILE DISTORTION toggle (CA
        // never moves an overlay), so it joins the transform key: switching
        // it must re-blit the canvas (knot data only changes per photo, and
        // opening a photo resets the texture anyway).
        // Profile-side geometry beyond the manual amount: distortion OR
        // the CA composite fill (Codex AL F1) - a CA-only overshoot moves
        // the photo, and the paint overlay must ride the SAME map or
        // strokes drift off the pixels they cover. The bool keys the
        // cache too, so toggling CA re-blits.
        // BOTH profile toggles key the cache (L13-9): collapsed to one
        // bool, "distortion + CA on → turn one off" still compared equal and
        // the overlay kept the OLD warp — strokes floated off their pixels.
        let profile_geom = (
            self.recipe.lens_profile.distortion_on && !self.recipe.lens_profile.distortion.is_empty(),
            self.recipe.lens_profile.ca_on
                && !(self.recipe.lens_profile.ca_r.is_empty()
                    && self.recipe.lens_profile.ca_b.is_empty()),
        );
        let xform_now = (
            self.recipe.straighten_deg,
            self.recipe.lens_distortion,
            profile_geom,
            (self.recipe.ca_r, self.recipe.ca_b),
        );
        // The identity key, spelled ONCE (it grew a fourth member with the
        // manual CA pair in R25 B3, and it is compared in four places).
        const IDENTITY_XFORM: (f32, f32, (bool, bool), (f32, f32)) =
            (0.0, 0.0, (false, false), (0.0, 0.0));
        let stale_xform = self.mask_tex.is_some() && self.mask_tex_xform != xform_now;
        if self.mask_dirty || stale_xform {
            // Fast path (the common no-geometry case): the change since the
            // last upload is a known brush rect — upload ONLY that
            // sub-rectangle. Brushing used to clone and re-upload the WHOLE
            // canvas on every pointer move (at an 8192 working preview that
            // is a ~270 MB round trip per frame).
            if xform_now == IDENTITY_XFORM && !stale_xform && self.mask_dirty {
                let rect = self.mask_dirty_rect;
                if let (Some(m), Some(tex), Some([x0, y0, x1, y1])) =
                    (&self.mask_paint, &mut self.mask_tex, rect)
                    && tex.size() == [m.width() as usize, m.height() as usize]
                    && x1 > x0
                    && y1 > y0
                    && x1 <= m.width()
                    && y1 <= m.height()
                {
                    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
                    let mut sub = Vec::with_capacity(w * h * 4);
                    for y in y0..y1 {
                        let start = ((y * m.width() + x0) * 4) as usize;
                        sub.extend_from_slice(&m.as_raw()[start..start + w * 4]);
                    }
                    tex.set_partial(
                        [x0 as usize, y0 as usize],
                        egui::ColorImage::from_rgba_unmultiplied([w, h], &sub),
                        egui::TextureOptions::LINEAR,
                    );
                    self.mask_dirty_rect = None;
                    self.mask_dirty = false;
                    return;
                }
            }
            // Geometry active: the remap is whole-frame work — mid-stroke,
            // rebuild at a bounded cadence instead of on every pointer move
            // (the stroke's final shape lands on the release frame, when the
            // pointer is no longer down).
            if !stale_xform
                && xform_now != IDENTITY_XFORM
                && self.mask_tex.is_some()
                && ctx.input(|i| i.pointer.any_down())
                && self.mask_tex_built.elapsed() < std::time::Duration::from_millis(120)
            {
                return; // mask_dirty stays armed; retried next frame
            }
            if let Some(m) = &self.mask_paint {
                let ci = if xform_now != IDENTITY_XFORM {
                    // Alpha-preserving RGBA twins: the RGB16 photo paths
                    // flatten transparency to opaque, which turned the whole
                    // canvas into a red wash under any active geometry.
                    let mut img = m.clone();
                    if xform_now.1 != 0.0
                        || profile_geom != (false, false)
                        || xform_now.3 != (0.0, 0.0)
                    {
                        img = autoshop::render::apply_lens_geometry_rgba(
                            &img,
                            &autoshop::render::geometry_profile(&self.recipe),
                            xform_now.1,
                        );
                    }
                    if xform_now.0 != 0.0 {
                        img = autoshop::render::rotate_straighten_rgba(&img, xform_now.0);
                    }
                    let rgba = img;
                    egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize],
                        rgba.as_raw(),
                    )
                } else {
                    // Common case: no geometry — zero-copy view of the canvas.
                    egui::ColorImage::from_rgba_unmultiplied(
                        [m.width() as usize, m.height() as usize],
                        m.as_raw(),
                    )
                };
                // Update in place: brushing sets mask_dirty on every pointer
                // move, and a fresh texture per frame is the allocate/free
                // churn finish_redevelop documents avoiding (set() re-uploads,
                // size changes included).
                if let Some(tex) = &mut self.mask_tex {
                    tex.set(ci, egui::TextureOptions::LINEAR);
                } else {
                    self.mask_tex =
                        Some(ctx.load_texture("paintmask", ci, egui::TextureOptions::LINEAR));
                }
                self.mask_tex_xform = xform_now;
                self.mask_tex_built = Instant::now();
            }
            self.mask_dirty_rect = None; // full upload covered everything pending
            self.mask_dirty = false;
        }
    }
}
