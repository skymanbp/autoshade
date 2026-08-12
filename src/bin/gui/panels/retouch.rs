//! Retouch panel: paint/clone/fill/heal/AI tools.

use crate::*;

impl AutoshopApp {
    /// Brush-paint into the mask while dragging on the After image. The canvas
    /// is full-frame at preview resolution; pointer→canvas goes through the
    /// view transform so painting stays accurate at any zoom, and the brush
    /// radius converts display px → canvas px by the current pixel scale.
    pub(crate) fn handle_paint(&mut self, resp: &egui::Response, xf: ViewXform) {
        let brush = self.brush;
        let (dims, deg, dist) = self.geom_ctx();
        // Mask-brush session: strokes write the display canvas AND the
        // greyscale weight buffer; erase clears both.
        let erase = self.mask_brush.is_some_and(|(_, e)| e);
        let display_px = if erase {
            image::Rgba([0, 0, 0, 0])
        } else {
            image::Rgba([255, 64, 64, 160])
        };
        let gray_v: u8 = if erase { 0 } else { 255 };
        let mut gray = self.mask_brush_gray.take();
        let Some(m) = self.mask_paint.as_mut() else {
            self.mask_brush_gray = gray;
            return;
        };
        let (mw, mh) = (m.width() as f32, m.height() as f32);
        // The canvas lives in the ORIGINAL frame (fill/heal edit source
        // pixels), so pointer positions map out of the transformed view.
        let to_mask = |p: egui::Pos2| {
            let (nx, ny) = xf.to_norm(p);
            let (ox, oy) = view_norm_to_orig(nx, ny, dims, deg, &dist);
            (ox * mw, oy * mh)
        };
        // Brush radius in MASK pixels, measured through the SAME nonlinear
        // view→original chain the centers use: map the pointer and a point one
        // display-radius to its right, take their mask-space distance. Under
        // straighten/distortion a fixed display brush covers a position-
        // dependent original-frame extent — the old constant horizontal-UV
        // conversion mismatched the painted footprint there. Neutral geometry
        // degenerates to exactly the old constant.
        let brush_at = |p: egui::Pos2| {
            let a = to_mask(p);
            // Probe BOTH directions and keep the larger. `ViewXform::to_norm`
            // CLAMPS to the viewport, so a rightward-only probe mapped to the
            // same point as the centre once the pointer reached the right
            // edge: the distance went to zero, the `.max(1.0)` floor took
            // over, and a 30 px brush stamped a single mask pixel exactly
            // where the user was painting along that edge. The chain is
            // smooth, so the direction with room measures the same radius.
            let probe = |dx: f32| {
                let b = to_mask(p + egui::vec2(dx, 0.0));
                ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
            };
            probe(brush).max(probe(-brush)).max(1.0)
        };
        // Union a brush segment's bounding box (± radius, canvas px) into the
        // pending dirty rect — ensure_mask_tex's partial-upload path.
        let grow = |rect: &mut Option<[u32; 4]>, a: (f32, f32), b: (f32, f32), r: f32| {
            let r = r + 1.0;
            let n = [
                (a.0.min(b.0) - r).floor().clamp(0.0, mw) as u32,
                (a.1.min(b.1) - r).floor().clamp(0.0, mh) as u32,
                (a.0.max(b.0) + r).ceil().clamp(0.0, mw) as u32,
                (a.1.max(b.1) + r).ceil().clamp(0.0, mh) as u32,
            ];
            *rect = Some(match rect {
                Some(o) => [o[0].min(n[0]), o[1].min(n[1]), o[2].max(n[2]), o[3].max(n[3])],
                None => n,
            });
        };
        // PRIMARY button only: `dragged()` is button-agnostic, so a
        // secondary-button drag — which the caller intercepts for panning —
        // was also reported here and painted into the fill/heal/clone mask.
        if resp.dragged_by(egui::PointerButton::Primary)
            || resp.drag_started_by(egui::PointerButton::Primary)
        {
            if let Some(p) = resp.interact_pointer_pos() {
                let cur = to_mask(p);
                let r = brush_at(p);
                match self.paint_last {
                    Some(prev) => {
                        stamp_line_px(m, prev, cur, r, display_px);
                        if let Some(g) = gray.as_mut() {
                            stamp_line_gray(g, prev, cur, r, gray_v);
                        }
                    }
                    // First stroke event: connect from the PRESS ORIGIN —
                    // drag_started fires ~6 px in, and a lone dot at the
                    // current pointer dropped the stroke's lead-in.
                    None => {
                        let start = ui_press_origin(resp).map(&to_mask).unwrap_or(cur);
                        stamp_line_px(m, start, cur, r, display_px);
                        if let Some(g) = gray.as_mut() {
                            stamp_line_gray(g, start, cur, r, gray_v);
                        }
                        grow(&mut self.mask_dirty_rect, start, cur, r);
                    }
                }
                grow(&mut self.mask_dirty_rect, self.paint_last.unwrap_or(cur), cur, r);
                self.paint_last = Some(cur);
                self.mask_dirty = true;
            }
        } else {
            // A quick tap is a CLICK to egui (<6 px / <0.8 s — never a drag):
            // stamp a single dot so marking dust spots doesn't require a smear.
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let cur = to_mask(p);
                let r = brush_at(p);
                stamp_dot_px(m, cur, r, display_px);
                if let Some(g) = gray.as_mut() {
                    stamp_dot_gray(g, cur, r, gray_v);
                }
                grow(&mut self.mask_dirty_rect, cur, cur, r);
                self.mask_dirty = true;
            }
            // No stroke is in flight on any non-drag frame — clearing here
            // (not just on drag_stopped) also covers strokes interrupted by
            // Esc / hold-B / an Alt source-pick, whose drag_stopped frame
            // never reaches this handler and used to leave a stale anchor
            // that drew a full-width connecting streak into the next stroke.
            self.paint_last = None;
        }
        self.mask_brush_gray = gray;
    }

    /// Clone-stamp interaction: Alt+click picks the SOURCE point (stored in
    /// the original frame like every pixel-path coordinate); plain drags paint
    /// the target with the shared brush. The picked source stays marked with
    /// a crosshair ring so the offset is always visible.
    pub(crate) fn handle_clone(&mut self, ui: &egui::Ui, resp: &egui::Response, xf: ViewXform) {
        let alt = ui.input(|i| i.modifiers.alt);
        if alt {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.clicked()
                && let Some(q) = resp.interact_pointer_pos()
            {
                let (dims, deg, dist) = self.geom_ctx();
                let (nx, ny) = xf.to_norm(q);
                self.clone_src = Some(view_norm_to_orig(nx, ny, dims, deg, &dist));
                self.status = tr(
                    self.lang,
                    "Clone source sampled — brush the area to cover, then 「⎘ Clone painted area」",
                )
                .into();
            }
        } else {
            self.handle_paint(resp, xf);
        }
        if let Some((sx, sy)) = self.clone_src {
            let (dims, deg, dist) = self.geom_ctx();
            let (vx, vy) = orig_norm_to_view(sx, sy, dims, deg, &dist);
            let q = xf.to_screen(vx, vy);
            let p = ui.painter_at(xf.rect);
            // ACCENT, per the two-tier colour rule: on-canvas tool overlays
            // never wear the PILL chrome gold — a source pin sampled on skin,
            // sand or a sunset sky simply vanished in gold.
            p.circle_stroke(q, 9.0, egui::Stroke::new(2.0, ACCENT));
            p.line_segment(
                [q - egui::vec2(13.0, 0.0), q + egui::vec2(13.0, 0.0)],
                egui::Stroke::new(1.0, ACCENT),
            );
            p.line_segment(
                [q - egui::vec2(0.0, 13.0), q + egui::vec2(0.0, 13.0)],
                egui::Stroke::new(1.0, ACCENT),
            );
        }
    }

    /// Generative fill: regenerate the painted area (gpt-image), composite onto
    /// the source, save to ./out. Runs on a worker thread.
    pub(crate) fn start_fill(&mut self) {
        // Retouch the ACTIVE variant's pixels (a Generated variant → its origin
        // PNG), not the raw negative — otherwise a fill on the AI image would
        // splice in original pixels.
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // pre-spawn UI statuses only; results land as FACTS (L12#4)
        let prompt = self.fill_prompt.trim().to_string();
        if prompt.is_empty() {
            self.status = tr(lang, "write what should fill the painted area").into();
            return;
        }
        let Some(mask_png) = self.export_mask_png() else {
            self.status = tr(lang, "paint the area to remove/fill first (tick Paint mask)").into();
            return;
        };
        let Some(out) = unique_out(&path, "retouch") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = if self.fill_fullres {
            tr(lang, "generative fill (full-res render)… (slow, minutes)").into()
        } else {
            tr(lang, "generative fill via gpt-image… (high quality can run minutes — progress in the status bar; ✕ Cancel to stop)").into()
        };
        let quality = ["high", "medium", "low"][self.fill_quality.min(2)].to_string();
        let full_res = self.fill_fullres;
        let edge = self.canvas_edge(); // bake at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, flag) = self.arm_cancel();
        let ptx = self.tx.clone();
        self.spawn_worker(
            move || {
                // Progress heartbeats → status bar; the cancel flag stops the
                // stream between events. Installed on THIS worker thread only.
                autoshop::generative::set_worker_hooks(Some(autoshop::generative::WorkerHooks {
                    progress: Box::new(move |m| {
                        let _ = ptx.send(Msg::Progress(epoch, m));
                    }),
                    cancel: flag,
                }));
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let mask_tmp = gui_tmp_png("fill");
                    std::fs::write(&mask_tmp, &mask_png)?;
                    let r = autoshop::generative::retouch(&cfg, &path, &mask_tmp, &prompt, &quality, full_res, &out);
                    let _ = std::fs::remove_file(&mask_tmp);
                    r?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // InPlace: refine the current rendition — bake into the active
                    // variant's base AND repoint its origin at this saved artifact
                    // so export / reverse-fit / next retouch follow the fill.
                    // FACTS, not prose (L12#4): the landing renders these
                    // with the language live when the result lands.
                    Ok((img, RetouchNote::Filled(out.clone()), out, RetouchKind::InPlace))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Heal: AI auto-detect (use_mask=false) or the painted mask (use_mask=true).
    /// Pixel retouch from surrounding real pixels; saves to ./out.
    pub(crate) fn start_heal(&mut self, use_mask: bool) {
        // Heal the ACTIVE variant's pixels (Generated → its origin PNG).
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // pre-spawn UI statuses only; results land as FACTS (L12#4)
        let mask_png = if use_mask {
            match self.export_mask_png() {
                Some(b) => Some(b),
                None => {
                    self.status =
                        tr(lang, "tick Paint mask and paint the spots, then Heal painted area").into();
                    return;
                }
            }
        } else {
            None
        };
        let Some(out) = unique_out(&path, "heal") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = if use_mask {
            tr(lang, "healing painted area…").into()
        } else {
            tr(lang, "AI healing… (~10-30s)").into()
        };
        let full_res = self.heal_fullres;
        let edge = self.canvas_edge(); // bake at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, _flag) = self.arm_cancel(); // local compute: Cancel = abandon (epoch discard)
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let mask_tmp = match mask_png {
                        Some(bytes) => {
                            let t = gui_tmp_png("heal");
                            std::fs::write(&t, &bytes)?;
                            Some(t)
                        }
                        None => None,
                    };
                    let rep = autoshop::retouch::heal(&cfg, &path, mask_tmp.as_deref(), !use_mask, full_res, &out);
                    if let Some(t) = &mask_tmp {
                        let _ = std::fs::remove_file(t);
                    }
                    let rep = rep?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // The report's rationale (AI-detect fallback, budget
                    // skips) must reach the status — stderr is invisible in
                    // the windowed GUI (L08 rule). Split per the L12#2B
                    // suffix contract: typed notes render localized at the
                    // landing; on a mismatch the whole string rides as prose.
                    let det = autoshop::rationale::render_en(&rep.notes);
                    let (ai_prose, notes) = match rep.rationale.strip_suffix(det.as_str()) {
                        Some(p) => (p.to_string(), rep.notes),
                        None => (rep.rationale.clone(), Vec::new()),
                    };
                    // InPlace: bake into the active variant's base + repoint origin.
                    Ok((
                        img,
                        RetouchNote::Healed { n: rep.spots, out: out.clone(), ai_prose, notes },
                        out,
                        RetouchKind::InPlace,
                    ))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// AI denoise (SCUNet, GPU sidecar) as an ACTIVE canvas operation: run on
    /// the current variant's pixels NOW and bake the clean base in-place, so
    /// the user sees the result immediately instead of only in an export.
    /// Mirrors heal's plumbing: RetouchKind::InPlace swaps the variant base
    /// (undoable) and the develop chain keeps rendering on top.
    pub(crate) fn start_ai_denoise(&mut self) {
        // Denoise the ACTIVE variant's pixels (a Generated variant → its
        // origin PNG), same source rule as heal/clone.
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // pre-spawn UI statuses only; results land as FACTS (L12#4)
        let Some(out) = unique_out(&path, "denoise") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = if self.denoise_fullres {
            tr(lang, "AI denoise (full-res)… (GPU sidecar, can take minutes; first run downloads the model)").into()
        } else {
            tr(lang, "AI denoise… (GPU sidecar on a ≤2048px working copy; first run downloads the model)").into()
        };
        let full_res = self.denoise_fullres;
        let edge = self.canvas_edge(); // show at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, _flag) = self.arm_cancel(); // sidecar compute: Cancel = abandon (epoch discard)
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    let opts =
                        autoshop::denoise::DenoiseOpts::from_config(&cfg, None, 1.0);
                    autoshop::denoise::denoise_active(&opts, &path, full_res, &out)?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // InPlace: bake into the active variant's base + repoint origin.
                    Ok((img, RetouchNote::Denoised(out.clone()), out, RetouchKind::InPlace))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Run the clone stamp on a worker: painted target mask + the Alt+picked
    /// source point → `retouch::clone_stamp` (deterministic, no AI) → ./out
    /// pixel master shown in the After pane, exactly like heal.
    pub(crate) fn start_clone(&mut self) {
        // Clone within the ACTIVE variant's pixels (Generated → its origin PNG).
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang; // pre-spawn UI statuses only; results land as FACTS (L12#4)
        let Some(src_pt) = self.clone_src else {
            self.status = tr(lang, "Alt+click to set the clone source first").into();
            return;
        };
        let Some(mask_png) = self.export_mask_png() else {
            self.status = tr(lang, "Brush the area to clone over first").into();
            return;
        };
        let Some(out) = unique_out(&path, "clone") else {
            self.status = tr(lang, "over 999 retouch masters for this photo — clean up ./out first").into();
            return;
        };
        self.busy = true;
        self.status = tr(lang, "Cloning… (local pixel compute)").into();
        let full_res = self.clone_fullres;
        let edge = self.canvas_edge(); // bake at the CANVAS's res (canvas_edge)
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, _flag) = self.arm_cancel(); // local compute: Cancel = abandon (epoch discard)
        self.spawn_worker(
            move || {
                let res = (|| -> RetouchDone {
                    let mask_tmp = gui_tmp_png("clone");
                    std::fs::write(&mask_tmp, &mask_png)?;
                    let rep = autoshop::retouch::clone_stamp(&path, &mask_tmp, src_pt, full_res, &out);
                    let _ = std::fs::remove_file(&mask_tmp);
                    let rep = rep?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // InPlace: a pixel transplant of the current rendition — bake it
                    // into the active variant's base + repoint origin at the artifact.
                    Ok((
                        img,
                        RetouchNote::Cloned { n: rep.spots, out: out.clone() },
                        out,
                        RetouchKind::InPlace,
                    ))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Full-frame generative re-render via gpt-image — the OPTIONAL "let GPT
    /// directly make the picture" path. Uses the Direction text as the look
    /// prompt. Unlike Analyze (a faithful parametric recipe), this REGENERATES
    /// pixels — a creative restyle (up to ~8 MP on flexible-size models, ~1.5K
    /// on older ones). The result enters the strip as a new「AI 生成」variant
    /// (`origin = Some(out)`); its saved path is the reverse-fit ("反推配方")
    /// target that turns the look back into sliders + XMP at full resolution.
    pub(crate) fn start_reimagine(&mut self) {
        // Always reimagine the ORIGINAL negative (src_path), never a generated
        // variant's pixels — regenerating a rendition is the double-cook path
        // the variant model exists to avoid. Each call gets a UNIQUE ./out PNG
        // so two Generated variants never alias the same origin (which would
        // cross-wire their export / reverse-fit).
        let Some(path) = self.src_path.clone() else { return };
        if self.busy {
            return;
        }
        // First FREE ./out name (shared `unique_out` probe) so
        // delete-then-reimagine can't reuse a number whose PNG a surviving
        // variant still points at. Refuse past the cap rather than alias:
        // reusing an existing origin would cross-wire two variants'
        // export / reverse-fit.
        let Some(out) = unique_out(&path, "reimagine") else {
            self.status = tr(self.lang, "over 999 generated variants for this photo — clean up ./out first").into();
            return;
        };
        let prompt = {
            // The Reimagine section's OWN prompt field (no longer the shared
            // Direction — each prompt entry pairs with its own trigger).
            let g = self.reimagine_prompt.trim();
            if g.is_empty() {
                "Develop this photo into a finished, natural-looking edit: balanced exposure and \
                 contrast, pleasing realistic colour; keep the scene true to the original."
                    .to_string()
            } else {
                g.to_string()
            }
        };
        self.busy = true;
        let lang = self.lang;
        self.status =
            tr(lang, "AI generating… (gpt-image; high quality can run minutes — progress in the status bar; ✕ Cancel to stop; hi-res input needs a full-frame develop first)").into();
        // The PREFERENCE here, not `canvas_edge`: this is the one bake that
        // renders from src_path (never the canvas — see above) and lands as a
        // NEW variant, so nothing is composited under it and there is no
        // raster to match. Following a stale canvas — a background 1280 bake
        // clicked while the preference sat at 4096 — pinned the new variant
        // below the resolution the user asked for, behind a refusal whose own
        // remedy (save) Ctrl+S REFUSES for a generated variant.
        let edge = self.preview_edge.clamp(640, 8192);
        let out_claim = out.clone(); // release the claim on failure (worker tail)
        let out_panic = out.clone(); // …and on a worker panic (see the error closure)
        let (epoch, flag) = self.arm_cancel();
        let ptx = self.tx.clone();
        self.spawn_worker(
            move || {
                // Progress heartbeats → status bar; the cancel flag stops the
                // stream between events. Installed on THIS worker thread only.
                autoshop::generative::set_worker_hooks(Some(autoshop::generative::WorkerHooks {
                    progress: Box::new(move |m| {
                        let _ = ptx.send(Msg::Progress(epoch, m));
                    }),
                    cancel: flag,
                }));
                let res = (|| -> RetouchDone {
                    let cfg = autoshop::config::Config::load();
                    // fidelity "high" keeps it recognisably the same photo.
                    autoshop::generative::reimagine(&cfg, &path, &prompt, "high", &cfg.openai_image_quality, &out)?;
                    let img = autoshop::decode::load_image(&out)?.thumbnail(edge, edge);
                    // NewGenerated: a whole-frame rendition → a new Generated variant.
                    Ok((img, RetouchNote::Reimagined(out.clone()), out, RetouchKind::NewGenerated))
                })();
                if res.is_err() {
                    release_empty_claim(&out_claim);
                }
                Msg::Retouched(epoch, Box::new(res))
            },
            move |e| {
                // The worker PANICKED (caught by spawn_worker), so the tail
                // above never ran — release here too.
                release_empty_claim(&out_panic);
                Msg::Retouched(epoch, Box::new(Err(e)))
            },
        );
    }

    /// Extract a reusable STYLE PROMPT from the before/after pair via the vision
    /// model. The result lands in the Direction box (ready to reimagine OTHER
    /// photos with the same look) and is saved to ./out/<stem>.style.txt.
    pub(crate) fn start_style_prompt(&mut self) {
        let (Some(base), Some(tgt)) = (self.source_preview.clone(), self.fit_target())
        else {
            return;
        };
        if self.busy {
            return;
        }
        let src_path = self.src_path.clone();
        // No lang capture: the worker returns FACTS; the landing localises
        // them with the language live at landing time (L12#4).
        self.busy = true;
        self.status = tr(self.lang, "Extracting style prompt… (vision, ~5-20s)").into();
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<(String, StyleNote)> {
                    let cfg = autoshop::config::Config::load();
                    let jpg = |img: &image::DynamicImage| -> anyhow::Result<Vec<u8>> {
                        let mut buf = Vec::new();
                        image::DynamicImage::ImageRgb8(img.thumbnail(768, 768).to_rgb8())
                            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)?;
                        Ok(buf)
                    };
                    let target = autoshop::decode::load_image(&tgt)?;
                    let prompt = autoshop::advisor::describe_style(&cfg, &jpg(&base)?, &jpg(&target)?)?;
                    // The side-file is a convenience copy — its write failing
                    // must not discard the vision result the user paid for.
                    // The note tells the truth either way (the old fixed
                    // message claimed the file was saved unconditionally).
                    let note = match &src_path {
                        Some(p) => {
                            let out = autoshop::pipeline::default_out(p, "style", "txt");
                            // Same protocol as every deliverable (W33): the
                            // library guard, then stage + rename — a direct
                            // fs::write truncated the previous prompt in
                            // place and followed a leaf symlink.
                            match autoshop::pipeline::guard_readonly(&out, p)
                                .and_then(|()| autoshop::pipeline::ensure_parent(&out))
                                .and_then(|()| {
                                    autoshop::render::stage_and_publish(&out, |staged| {
                                        Ok(std::fs::write(staged, &prompt)?)
                                    })
                                })
                            {
                                Ok(()) => StyleNote::SavedCopy,
                                Err(e) => StyleNote::SaveFailed(e.to_string()),
                            }
                        }
                        None => StyleNote::NotSaved,
                    };
                    Ok((prompt, note))
                })();
                Msg::Styled(Box::new(res))
            },
            |e| Msg::Styled(Box::new(Err(e))),
        );
    }

    pub(crate) fn retouch_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang; // Copy — never borrows self, safe inside egui closures.
        ui.separator();
        ui.heading(tr(lang, "Retouch"));

        // Whole-image generative re-render: let gpt-image DIRECTLY produce the
        // picture (the optional "GPT makes the image" path). Distinct from
        // AI Analyze, which emits a faithful parametric recipe. The result
        // becomes a new「AI 生成」variant in the strip below; the reverse-fit
        // button then closes the loop, adding a「反推」variant whose look lives
        // in an editable recipe (full-res + XMP). No more "continue from
        // master" button — each result is its own selectable variant, so a
        // slider edit can never revert or double-cook it.
        egui::CollapsingHeader::new(tr(lang, "Reimagine (whole image)"))
            .id_salt("sec_reimagine")
            .default_open(true)
            .show(ui, |ui| {
                // This entry's OWN style prompt, right next to its trigger
                // (it used to silently borrow the Direction field at the top
                // of the panel — a prompt and its button belong together).
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.reimagine_prompt)
                            .desired_width((ui.available_width() - 130.0).max(80.0))
                            .hint_text(tr(lang, "style to repaint toward — e.g. golden-hour glow, moody film look")),
                    );
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui
                            .button(tr(lang, "✨ Generate image"))
                            .on_hover_text(tr(lang,
                                "Repaint the whole image with gpt-image, styled by the prompt on the left \
                                 (empty = a neutral finished develop). Repainted pixels = not faithful; the \
                                 result is added as an 「AI generated」 variant at the bottom and switched to, \
                                 so you can keep tweaking without reverting. Models that accept any size \
                                 (gpt-image-2) reach ~8MP, others ~1.5K. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
                            ))
                            .clicked()
                        {
                            self.start_reimagine();
                        }
                    });
                });
                // Reverse-fit the active generated variant's look back into an
                // editable recipe — how the low-res experiment becomes a
                // full-res, XMP-able「反推」variant.
                let can_fit = self.fit_target().is_some() && self.source_preview.is_some();
                if !can_fit {
                    ui.label(
                        egui::RichText::new(tr(lang,
                            "Generate an image first and stay on that variant to reverse-fit its recipe."))
                            .weak()
                            .small(),
                    );
                }
                ui.add_enabled_ui(!self.busy && can_fit, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(tr(lang, "🎛 Reverse-fit recipe → sliders/XMP"))
                            .on_hover_text(tr(lang,
                                "Statistical fit: reverse the freshly generated look into editable develop params \
                                 (local, no API cost). Sliders update (undoable), and for RAW a Lightroom XMP goes \
                                 into this photo's develop store; hit Export to render the full-resolution result.",
                            ))
                            .clicked()
                        {
                            self.start_fit();
                        }
                        if ui
                            .button(tr(lang, "📝 Extract style prompt"))
                            .on_hover_text(tr(lang,
                                "Compare the original / generated images and have the vision model write a reusable \
                                 style prompt: auto-fills the Reimagine prompt (ready to restyle other photos) and \
                                 saves ./out/<stem>.style.txt.",
                            ))
                            .clicked()
                        {
                            self.start_style_prompt();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "After generating, use 「Reverse-fit recipe」 to turn the look into sliders + XMP \
                         (the full-resolution way).",
                    ))
                    .weak()
                    .small(),
                );
            });

        // Mask tools shared by Fill AND Heal — one brush, two consumers.
        ui.horizontal(|ui| {
            let r = ui
                .checkbox(&mut self.paint_mode, tr(lang, "Paint mask"))
                .on_hover_text(tr(lang, "Brush over the area; box-select is paused while on. Shared by Fill and Heal."));
            if r.changed() && self.paint_mode {
                // Mutual exclusion lives in ONE place now (disarm_tools) —
                // this site's own hand copy once drifted and made a ticked
                // brush completely inert (dispatch tries the others first).
                self.disarm_tools();
                self.paint_mode = true; // re-arm after the sweep
            }
            if ui
                .button(tr(lang, "Clear brush"))
                .on_hover_text(tr(lang, "Wipe the painted area (shared by Fill, Heal and Stamp)"))
                .clicked()
            {
                self.clear_mask();
            }
        });
        Self::slider(ui, lang, tr(lang, "Brush size"), &mut self.brush, 4.0, 80.0, 30.0);

        egui::CollapsingHeader::new(tr(lang, "Generative Fill"))
            .id_salt("sec_fill")
            .default_open(false)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.fill_prompt)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr(lang, "what belongs there, e.g. remove the trash can, extend the sky")),
                );
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("fill_quality")
                        .selected_text(tr(lang, ["high", "medium", "low"][self.fill_quality.min(2)]))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.fill_quality, 0, tr(lang, "high"));
                            ui.selectable_value(&mut self.fill_quality, 1, tr(lang, "medium"));
                            ui.selectable_value(&mut self.fill_quality, 2, tr(lang, "low"));
                        })
                        .response
                        .on_hover_text(tr(lang, "gpt-image render quality — higher looks better and costs more per image"));
                    let src_is_raw = self
                        .active_source_path()
                        .is_some_and(|p| autoshop::decode::is_raw(&p));
                    ui.add_enabled(src_is_raw, egui::Checkbox::new(&mut self.fill_fullres, tr(lang, "Full-res")))
                        .on_hover_text(tr(lang, "Composite onto the full-sensor develop (slow, RAW only)"));
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui
                            .button(tr(lang, "Remove / Fill"))
                            .on_hover_text(tr(lang, "Regenerate ONLY the painted area from your prompt (gpt-image API call — costs per image); the rest keeps the engine's own develop"))
                            .clicked()
                        {
                            self.start_fill();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Paint the area, write what belongs there, then Remove/Fill. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
                    ))
                    .weak()
                    .small(),
                );
            });

        egui::CollapsingHeader::new(tr(lang, "Heal (pixel)"))
            .id_salt("sec_heal")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui
                            .button(tr(lang, "✦ AI heal (auto)"))
                            .on_hover_text(tr(lang, "A vision model finds small dust spots / blemishes (API call), then each is healed from surrounding REAL pixels — never generated"))
                            .clicked()
                        {
                            self.start_heal(false);
                        }
                        if ui
                            .button(tr(lang, "Heal painted area"))
                            .on_hover_text(tr(lang, "Heal the brushed area from surrounding real pixels — local compute, no API"))
                            .clicked()
                        {
                            self.start_heal(true);
                        }
                    });
                    // Enabled for BAKED sources too (L09#4, user decision
                    // 2026-08-11): heal honours the flag on both source
                    // types since b4c6c30, but the RAW-only gate left a
                    // 61 MP baked TIFF no way to opt out of the 2048px
                    // downsample the unchecked path bakes into the master.
                    ui.checkbox(&mut self.heal_fullres, tr(lang, "Full-res"))
                        .on_hover_text(tr(lang, "Heal at full resolution (slow; without it a baked image is saved at 2048px)"));
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "AI auto-detects dust / blemishes, or paint a mask and Heal it. Pixel retouch from surrounding pixels; saved to ./out.",
                    ))
                    .weak()
                    .small(),
                );
            });

        egui::CollapsingHeader::new(tr(lang, "Clone Stamp"))
            .id_salt("sec_clone")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let label = if self.clone_mode { tr(lang, "✓ Done") } else { tr(lang, "⎘ Enter stamp") };
                    if ui
                        .button(label)
                        .on_hover_text(tr(lang, "Arm the stamp: Alt+click samples a source, the brush paints the target; your painted mask survives"))
                        .clicked()
                    {
                        let on = !self.clone_mode;
                        self.disarm_tools();
                        self.clone_mode = on;
                        // The painted canvas SURVIVES arming — it used to be
                        // wiped here with no undo, so a mask painted for
                        // Fill/Heal died the moment the user peeked at the
                        // stamp. The explicit Clear button owns wiping.
                        if on {
                            self.status =
                                tr(lang, "Stamp: Alt+click to set the source → brush the target area → 「⎘ Clone painted area」").into();
                        }
                    }
                    // Enabled for BAKED sources too — clone_stamp honours
                    // the flag on both source types (retouch.rs), the same
                    // rule as Heal beside it (L15-7).
                    ui.checkbox(&mut self.clone_fullres, tr(lang, "Full-res"))
                        .on_hover_text(tr(lang, "Clone at full resolution (slow; without it a baked image is saved at 2048px)"));
                    ui.add_enabled_ui(!self.busy && self.clone_mode, |ui| {
                        if ui
                            .button(tr(lang, "⎘ Clone painted area"))
                            .on_hover_text(tr(lang, "Copy the sampled source over the brushed area verbatim (feathered edges, no tone matching) — local compute"))
                            .clicked()
                        {
                            self.start_clone();
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(tr(lang,
                        "Photoshop-style clone stamp: Alt+click to sample a source (cross marker), brush the area to \
                         cover, and pixels are carried over as-is from the source (feathered edges, no tone matching). \
                         Local compute, saves a ./out pixel master.",
                    ))
                    .weak()
                    .small(),
                );
            });
    }
}
