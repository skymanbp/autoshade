// One part of the GUI's tests (src/bin/gui/tests.rs includes it): a CPU
// rasteriser for egui's tessellated output, so a panel can be LOOKED AT as a
// PNG without opening a window (the app is never launched while debugging),
// and the scrollbar-gutter pin the 2026-10-12 report asked for.

    /// The controls panel as the app lays it (`controls_side_panel` around
    /// `controls_column`), as if the user had dragged it to `width` px (the
    /// app's range still clamps it); its rect.
    fn controls_panel(ctx: &egui::Context, app: &mut AutoShadeApp, width: f32) -> egui::Rect {
        let id = egui::Id::new("controls");
        if egui::containers::panel::PanelState::load(ctx, id).is_none() {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 100.0));
            ctx.data_mut(|d| d.insert_persisted(id, egui::containers::panel::PanelState { rect }));
        }
        crate::side_panel::controls_side_panel(ctx, |ui| app.controls_column(ui)).response.rect
    }

    /// Every texture egui handed out so far, as premultiplied sRGBA rows.
    #[derive(Default)]
    struct ShotTextures(std::collections::HashMap<egui::TextureId, (usize, usize, Vec<egui::Color32>)>);

    impl ShotTextures {
        fn apply(&mut self, delta: &egui::TexturesDelta) {
            for (id, d) in &delta.set {
                let (w, h) = (d.image.width(), d.image.height());
                let px: Vec<egui::Color32> = match &d.image {
                    egui::ImageData::Color(c) => c.pixels.clone(),
                    egui::ImageData::Font(f) => f.srgba_pixels(None).collect(),
                };
                match d.pos {
                    None => {
                        self.0.insert(*id, (w, h, px));
                    }
                    Some([x0, y0]) => {
                        let (tw, _, tex) = self.0.get_mut(id).expect("patch onto a known texture");
                        for y in 0..h {
                            for x in 0..w {
                                tex[(y0 + y) * *tw + x0 + x] = px[y * w + x];
                            }
                        }
                    }
                }
            }
        }
    }

    /// Draw `prims` over a `bg`-filled canvas of `size` points at `ppp`, the
    /// way egui's own GL backend blends: premultiplied colour × texture,
    /// source-over in gamma space, scissored to each primitive's clip rect.
    fn rasterise(
        prims: &[egui::ClippedPrimitive],
        tex: &ShotTextures,
        size: egui::Vec2,
        ppp: f32,
        bg: egui::Color32,
    ) -> image::RgbaImage {
        let (w, h) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
        let mut buf = vec![[bg.r() as f32, bg.g() as f32, bg.b() as f32]; w * h];
        for p in prims {
            let egui::epaint::Primitive::Mesh(mesh) = &p.primitive else { continue };
            let Some((tw, th, texels)) = tex.0.get(&mesh.texture_id) else { continue };
            let clip = p.clip_rect;
            let (cx0, cy0) = ((clip.min.x * ppp).max(0.0), (clip.min.y * ppp).max(0.0));
            let (cx1, cy1) = ((clip.max.x * ppp).ceil().min(w as f32), (clip.max.y * ppp).ceil().min(h as f32));
            for t in mesh.indices.chunks_exact(3) {
                let v = [&mesh.vertices[t[0] as usize], &mesh.vertices[t[1] as usize], &mesh.vertices[t[2] as usize]];
                let pt = |i: usize| (v[i].pos.x * ppp, v[i].pos.y * ppp);
                let (a, b, c) = (pt(0), pt(1), pt(2));
                let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                if area.abs() < 1e-6 {
                    continue;
                }
                let x0 = a.0.min(b.0).min(c.0).floor().max(cx0) as usize;
                let x1 = a.0.max(b.0).max(c.0).ceil().min(cx1).max(0.0) as usize;
                let y0 = a.1.min(b.1).min(c.1).floor().max(cy0) as usize;
                let y1 = a.1.max(b.1).max(c.1).ceil().min(cy1).max(0.0) as usize;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / area;
                        let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let mix = |f: fn(&egui::epaint::Vertex) -> f32| w0 * f(v[0]) + w1 * f(v[1]) + w2 * f(v[2]);
                        let tx = ((mix(|q| q.uv.x) * *tw as f32) as usize).min(tw - 1);
                        let ty = ((mix(|q| q.uv.y) * *th as f32) as usize).min(th - 1);
                        let texel = texels[ty * tw + tx].to_array();
                        let colour = [
                            mix(|q| q.color.r() as f32),
                            mix(|q| q.color.g() as f32),
                            mix(|q| q.color.b() as f32),
                            mix(|q| q.color.a() as f32),
                        ];
                        let src: [f32; 4] = std::array::from_fn(|k| colour[k] * texel[k] as f32 / 255.0);
                        let dst = &mut buf[y * w + x];
                        for k in 0..3 {
                            dst[k] = src[k] + dst[k] * (1.0 - src[3] / 255.0);
                        }
                    }
                }
            }
        }
        image::RgbaImage::from_fn(w as u32, h as u32, |x, y| {
            let p = buf[y as usize * w + x as usize];
            image::Rgba([p[0].round() as u8, p[1].round() as u8, p[2].round() as u8, 255])
        })
    }

    /// [`controls_panel`] run `frames` times over a `size`-point screen at
    /// `ppp`; the last frame's output, its rasterised picture and the
    /// panel's rect. Every fold is open — the AI folds by
    /// `fold_open_in_tests`, the rest by egui's own test hook.
    fn controls_shot(
        app: &mut AutoShadeApp,
        theme: crate::theme::ThemePref,
        size: egui::Vec2,
        panel_w: f32,
        ppp: f32,
        frames: usize,
    ) -> (image::RgbaImage, egui::FullOutput, egui::Rect) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        let mut tex = ShotTextures::default();
        let mut last = None;
        let mut panel = egui::Rect::NOTHING;
        for i in 0..frames {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 60.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
            let out = ctx.run(input, |ctx| {
                ctx.memory_mut(|m| m.set_everything_is_visible(true));
                panel = controls_panel(ctx, app, panel_w);
            });
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("at least one frame");
        // The panel's own layer only: egui's show-everything hook also pops
        // every tooltip, which the picture is not about.
        let shapes: Vec<_> = out.shapes.iter().filter(|c| c.clip_rect.right() <= panel.right() + 1.0).cloned().collect();
        let prims = ctx.tessellate(shapes, out.pixels_per_point);
        let bg = ctx.style().visuals.panel_fill;
        (rasterise(&prims, &tex, size, out.pixels_per_point, bg), out, panel)
    }

    /// User report 2026-10-12 (screenshot: the Generative Fill hint running
    /// under the panel's right edge): egui 0.29's floating scrollbar reserves
    /// NO width, so every row filled the panel to its last pixel and the bar
    /// was drawn over the text. With the bar shown — the panel is shorter than
    /// its content — no text may reach the band the bar is drawn in, and the
    /// panel keeps its width while the bar fades in.
    ///
    /// MUTATION THIS CATCHES: `controls_column`'s right gutter set to 0.
    #[test]
    fn no_panel_text_runs_under_the_scrollbar() {
        fn walk(s: &egui::Shape, clip: egui::Rect, bar_left: f32, past: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => {
                    let r = t.visual_bounding_rect().intersect(clip);
                    if r.is_positive() && r.right() > bar_left + 0.5 && !t.galley.text().trim().is_empty() {
                        let head: String = t.galley.text().chars().take(40).collect();
                        past.push(format!("{head:?} right {:.1}", r.right()));
                    }
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, bar_left, past)),
                _ => {}
            }
        }
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = app_with_every_button(lang);
            let theme = crate::theme::ThemePref::Dark;
            let (_, out, panel) = controls_shot(&mut app, theme, egui::vec2(1400.0, 900.0), 320.0, 1.0, 12);
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, theme);
            let style = ctx.style();
            // The panel frame's own right margin, then the bar inside it.
            let content_right = panel.right() - egui::Frame::side_top_panel(&style).inner_margin.right;
            let bar_left = content_right - style.spacing.scroll.bar_width;
            let mut past = Vec::new();
            for c in out.shapes.iter().filter(|c| c.clip_rect.right() <= panel.right() + 0.5) {
                walk(&c.shape, c.clip_rect, bar_left, &mut past);
            }
            assert!(past.is_empty(), "{lang:?}: text reaches the scrollbar band (x > {bar_left:.1}): {past:#?}");
            // The response rect is the CONTENT's, which an invisible
            // allocation may overrun by a few px with every fold open; the
            // panel's own width is pinned by
            // `the_controls_panel_width_follows_the_drag_not_the_content`.
            // What must hold here is that nothing DRAWN passes the content edge.
            let mut wide = Vec::new();
            for c in out.shapes.iter().filter(|c| c.clip_rect.right() <= panel.right() + 0.5) {
                // What reaches the screen: the shape cut by its own clip (a
                // prompt's hint runs on inside its field's clip), and not the
                // panel's own frame fill, which spans the whole panel.
                let r = c.shape.visual_bounding_rect().intersect(c.clip_rect);
                if r.is_positive() && r.left() > panel.left() + 0.5 && r.right() > content_right + 0.5 {
                    wide.push(format!("{r:?}"));
                }
            }
            assert!(wide.is_empty(), "{lang:?}: drawn past the panel's content edge {content_right:.1}: {wide:#?}");
        }
    }

    /// Not a check — the camera. `AUTOSHADE_SHOT_DIR=<dir> cargo test …
    /// panel_screenshots -- --ignored` writes the controls panel in both
    /// languages and both themes at the default and a wide width.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn panel_screenshots() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            for theme in crate::theme::ThemePref::ALL {
                for w in [320.0f32, 560.0] {
                    let mut app = app_with_every_button(lang);
                    with_long_analysis(&mut app);
                    let (img, _, panel) = controls_shot(&mut app, theme, egui::vec2(w + 40.0, 16_000.0), w, 2.0, 6);
                    let crop = image::imageops::crop_imm(&img, 0, 0, (panel.right() * 2.0) as u32, img.height());
                    crop.to_image().save(dir.join(format!("controls-{lang:?}-{theme:?}-{w}.png"))).unwrap();
                }
                let mut app = app_with_every_button(lang);
                with_long_analysis(&mut app);
                app.show_analysis_full = true;
                window_shot(&mut app, theme).save(dir.join(format!("analysis-window-{lang:?}-{theme:?}.png"))).unwrap();
            }
        }
    }

    /// An analysis long enough to overflow its box: a verdict, a rationale
    /// sentence with a deterministic note, and the four deep-thinking notes.
    fn with_long_analysis(app: &mut AutoShadeApp) {
        use autoshade::rationale::{keys, render_one, Note};
        let notes = vec![
            Note::new(keys::JUDGE_SCORE, vec![("score", "84".into()), ("critique", "the sky still reads a touch flat".into())]),
            Note::new(keys::THINK_SCENE, vec![("scene", "a red-rock canyon at golden hour, the far wall in shade".into())]),
            Note::new(keys::THINK_LOOK, vec![("look", "warm rock against a clean blue sky, shadows kept open".into())]),
            Note::new(keys::THINK_CRITIQUE, vec![("critique", "close to the target; the highlights could take one more step down".into())]),
            Note::new(keys::PIXEL_TOOLS, vec![("tools", "denoise (grain in the shaded wall)".into())]),
        ];
        let prose = concat!(
            "Exposure lifted half a stop for the shaded wall, highlights pulled to hold the sunlit rim, ",
            "whites eased so the sky keeps its gradient, vibrance up and saturation held so the red rock ",
            "stays believable, a cooler white balance on the sky band only, and a gentle S curve for depth. ",
        );
        app.rationale = std::iter::once(prose.to_string()).chain(notes.iter().map(render_one)).collect();
        app.recipe.rationale = app.rationale.clone();
        app.rationale_notes = notes;
        app.verdict = Some((autoshade::advisor::Decision::Accept, vec!["balanced".into(), "on target".into()]));
    }

    /// The analysis full-text window alone, on a 720 × 560 screen at 2×.
    fn window_shot(app: &mut AutoShadeApp, theme: crate::theme::ThemePref) -> image::RgbaImage {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        let size = egui::vec2(720.0, 560.0);
        let mut tex = ShotTextures::default();
        let mut last = None;
        for i in 0..6 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 2.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(2.0);
            let out = ctx.run(input, |ctx| app.analysis_full_window(ctx));
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("frames ran");
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        rasterise(&prims, &tex, size, out.pixels_per_point, theme.colors().canvas_bed)
    }

    /// The whole window as the app lays it — toolbar, status bar, variant
    /// strip, both side panels and the canvas — over a synthetic photo, at
    /// `size` points and 1.5×. The update phases the shell runs, minus the
    /// ones that need a live window (drops, title, sheets).
    fn full_window_shot(app: &mut AutoShadeApp, theme: crate::theme::ThemePref, size: egui::Vec2) -> image::RgbaImage {
        // The app reads its own theme field for the bars and the canvas bed.
        app.theme = theme;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        // A warm-rock / blue-sky gradient standing in for a photo.
        let (w, h) = (600usize, 400usize);
        let px: Vec<egui::Color32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                if y < 0.45 {
                    egui::Color32::from_rgb((70.0 + 60.0 * y) as u8, (120.0 + 50.0 * y) as u8, (200.0 - 30.0 * x) as u8)
                } else {
                    egui::Color32::from_rgb((170.0 + 50.0 * x) as u8, (80.0 + 30.0 * y) as u8, (45.0 + 20.0 * x) as u8)
                }
            })
            .collect();
        let photo = egui::ColorImage { size: [w, h], pixels: px };
        app.after_tex = Some(ctx.load_texture("shot-photo", photo, egui::TextureOptions::LINEAR));
        let ppp = 1.5;
        let mut tex = ShotTextures::default();
        let mut last = None;
        for i in 0..6 {
            // The pointer rests on the canvas, so an armed brush shows its ring.
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 2.0),
                events: vec![egui::Event::PointerMoved(egui::pos2(size.x * 0.7, size.y * 0.3))],
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
            let out = ctx.run(input, |ctx| {
                app.upd_top_bar(ctx);
                app.upd_status_bar(ctx);
                app.upd_strips_and_side_panels(ctx);
                app.upd_central(ctx);
            });
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("frames ran");
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        rasterise(&prims, &tex, size, out.pixels_per_point, egui::Color32::BLACK)
    }

    /// Not a check — the camera for the whole window, both themes:
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … window_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn window_screenshots() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            for theme in crate::theme::ThemePref::ALL {
                let mut app = app_with_every_button(lang);
                with_long_analysis(&mut app);
                full_window_shot(&mut app, theme, egui::vec2(1400.0, 880.0))
                    .save(dir.join(format!("window-{lang:?}-{theme:?}.png")))
                    .unwrap();
            }
        }
    }

