// One part of the GUI's tests (src/bin/gui/tests.rs includes it): a CPU
// rasteriser for egui's tessellated output, so a panel can be LOOKED AT as a
// PNG without opening a window (the app is never launched while debugging),
// and the scrollbar-gutter pin the 2026-10-12 report asked for.

    /// The controls panel as the app lays it (`controls_column`), `width` px
    /// by default; its rect.
    fn controls_panel(ctx: &egui::Context, app: &mut AutoShadeApp, width: f32) -> egui::Rect {
        egui::SidePanel::left("controls").default_width(width).show(ctx, |ui| app.controls_column(ui)).response.rect
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
    /// panel's rect. Every fold is open (`fold_open_in_tests`).
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
            let out = ctx.run(input, |ctx| panel = controls_panel(ctx, app, panel_w));
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("at least one frame");
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
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
            assert!((panel.width() - 320.0).abs() <= 0.5, "{lang:?}: the panel left its 320 px default: {}", panel.width());
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
            for theme in [crate::theme::ThemePref::Dark, crate::theme::ThemePref::Light] {
                for w in [320.0f32, 560.0] {
                    let mut app = app_with_every_button(lang);
                    let (img, _, panel) = controls_shot(&mut app, theme, egui::vec2(w + 40.0, 6000.0), w, 2.0, 6);
                    let crop = image::imageops::crop_imm(&img, 0, 0, (panel.right() * 2.0) as u32, img.height());
                    crop.to_image().save(dir.join(format!("controls-{lang:?}-{theme:?}-{w}.png"))).unwrap();
                }
            }
        }
    }
