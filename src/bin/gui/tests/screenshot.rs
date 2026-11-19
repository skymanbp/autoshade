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
        rasterise_region(prims, tex, egui::Rect::from_min_size(egui::Pos2::ZERO, size), ppp, bg)
    }

    /// [`rasterise`] of one `region` of the screen only, so a picture of a
    /// few hundred points out of a 16 000-point column does not paint (and
    /// hold in memory) the whole column first.
    fn rasterise_region(
        prims: &[egui::ClippedPrimitive],
        tex: &ShotTextures,
        region: egui::Rect,
        ppp: f32,
        bg: egui::Color32,
    ) -> image::RgbaImage {
        let (w, h) = ((region.width() * ppp) as usize, (region.height() * ppp) as usize);
        let o = region.min;
        let mut buf = vec![[bg.r() as f32, bg.g() as f32, bg.b() as f32]; w * h];
        for p in prims {
            let egui::epaint::Primitive::Mesh(mesh) = &p.primitive else { continue };
            let Some((tw, th, texels)) = tex.0.get(&mesh.texture_id) else { continue };
            let clip = p.clip_rect.translate(-o.to_vec2());
            let (cx0, cy0) = ((clip.min.x * ppp).max(0.0), (clip.min.y * ppp).max(0.0));
            let (cx1, cy1) = ((clip.max.x * ppp).ceil().min(w as f32), (clip.max.y * ppp).ceil().min(h as f32));
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            for t in mesh.indices.chunks_exact(3) {
                let v = [&mesh.vertices[t[0] as usize], &mesh.vertices[t[1] as usize], &mesh.vertices[t[2] as usize]];
                let pt = |i: usize| ((v[i].pos.x - o.x) * ppp, (v[i].pos.y - o.y) * ppp);
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

    /// A Settings form as `load_settings_form` would fill it for a user on
    /// API providers for both roles with keys saved and a custom image-gen
    /// template — filled by hand, because loading it reads the per-user
    /// settings file, which a test must not. Every conditional row shows.
    fn app_with_settings_form(lang: crate::i18n::Lang) -> AutoShadeApp {
        let mut app = AutoShadeApp { lang, show_settings: true, ..Default::default() };
        let f = &mut app.settings;
        f.analysis_provider_api = true;
        f.analysis_model = "gpt-5.5".into();
        f.analysis_base_url = crate::model::OPENAI_DEFAULT_URL.into();
        f.analysis_key_present = true;
        f.analysis_effort = "medium".into();
        f.image_provider_oauth = false;
        f.image_model = "gpt-5.5".into();
        f.image_base_url = crate::model::OPENAI_DEFAULT_URL.into();
        f.image_gen_model = "gpt-image-1.5".into();
        f.image_key_present = true;
        f.image_api_family = "auto".into();
        f.image_gen_custom = true;
        f.image_gen_endpoint = autoshade::image_endpoint::EXAMPLE.into();
        app
    }

    /// The Settings window as the app hosts it (`settings_window`), alone on
    /// an 1100 × 760 screen at 2×, over the canvas bed.
    fn settings_shot(app: &mut AutoShadeApp, theme: crate::theme::ThemePref) -> image::RgbaImage {
        app.theme = theme;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        let size = egui::vec2(1100.0, 760.0);
        let mut tex = ShotTextures::default();
        let mut last = None;
        for i in 0..6 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 2.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(2.0);
            let out = ctx.run(input, |ctx| app.settings_window(ctx));
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("frames ran");
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        rasterise(&prims, &tex, size, out.pixels_per_point, theme.colors().canvas_bed)
    }

    /// Not a check — the camera for the Settings window: every page, both
    /// languages, the Dark and Light themes:
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … settings_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn settings_screenshots() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            for theme in [crate::theme::ThemePref::Dark, crate::theme::ThemePref::Light] {
                for tab in crate::panels::settings::SettingsTab::ALL {
                    let mut app = app_with_settings_form(lang);
                    app.settings_tab = tab;
                    settings_shot(&mut app, theme)
                        .save(dir.join(format!("settings-{tab:?}-{lang:?}-{theme:?}.png")))
                        .unwrap();
                }
            }
        }
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
                let img = full_window_shot(&mut app, theme, egui::vec2(1400.0, 880.0));
                img.save(dir.join(format!("window-{lang:?}-{theme:?}.png"))).unwrap();
                // The top 120 pt alone (the shot is at 1.5×), so the bars of
                // two runs can be laid side by side.
                image::imageops::crop_imm(&img, 0, 0, img.width(), 180)
                    .to_image()
                    .save(dir.join(format!("window-strip-{lang:?}-{theme:?}.png")))
                    .unwrap();
                // A busy app, so the disabled Export split is pictured too.
                if matches!(theme, crate::theme::ThemePref::Dark | crate::theme::ThemePref::Light) {
                    let mut app = app_with_every_button(lang);
                    app.busy = true;
                    let img = full_window_shot(&mut app, theme, egui::vec2(1400.0, 880.0));
                    image::imageops::crop_imm(&img, 0, 0, img.width(), 180)
                        .to_image()
                        .save(dir.join(format!("window-strip-busy-{lang:?}-{theme:?}.png")))
                        .unwrap();
                }
            }
        }
    }

    // ---- The Versions & Export group and the variant strip: their scenes
    // and their camera.

    /// What the Versions section is showing, for its pictures.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum VersionsScene {
        /// One card, nothing saved as a version yet.
        NoVersions,
        /// One card and one version taken from it.
        OneCardOneVersion,
        /// ▣ and ◈ (the ◈ card under the canvas) and three versions: one
        /// named, one auto-archived, one from each card.
        TwoCardsThreeVersions,
    }

    impl VersionsScene {
        const ALL: [Self; 3] = [Self::NoVersions, Self::OneCardOneVersion, Self::TwoCardsThreeVersions];
    }

    /// One strip card for the pictures and the layout pins: no pixels of its
    /// own, a neutral develop.
    fn shot_card(kind: VariantKind, id: &str) -> Variant {
        Variant { kind, id: id.into(), name: None, recipe: EditRecipe::default(), base: None, origin: None, thumb: None }
    }

    /// An app whose Versions section shows `scene` (the store is never read:
    /// the version list and its records are filled by hand).
    fn app_with_versions(lang: crate::i18n::Lang, scene: VersionsScene) -> AutoShadeApp {
        use autoshade::store::{VersionMetaEntry, VERSION_ORIGIN_AUTO, VERSION_ORIGIN_USER};
        let mut app = AutoShadeApp {
            lang,
            src_path: Some(PathBuf::from("D:/library/_versions.ARW")),
            variants: vec![shot_card(VariantKind::Original, ORIGINAL_VARIANT_ID)],
            ..Default::default()
        };
        let from = |n: u32, kind: &str, id: &str| VersionMetaEntry {
            n,
            from_kind: Some(kind.into()),
            from_id: Some(id.into()),
            origin: Some(VERSION_ORIGIN_USER.into()),
            ..Default::default()
        };
        match scene {
            VersionsScene::NoVersions => {}
            VersionsScene::OneCardOneVersion => {
                app.versions = vec![1];
                app.version_meta.insert(1, from(1, "original", ORIGINAL_VARIANT_ID));
            }
            VersionsScene::TwoCardsThreeVersions => {
                app.variants.push(shot_card(VariantKind::Denoised, "dn-1"));
                app.active = 1;
                app.versions = vec![3, 5, 7];
                app.version_meta.insert(3, VersionMetaEntry { n: 3, origin: Some(VERSION_ORIGIN_AUTO.into()), ..Default::default() });
                app.version_meta.insert(5, from(5, "original", ORIGINAL_VARIANT_ID));
                app.version_meta.insert(7, VersionMetaEntry { name: Some("Warm sunset".into()), ..from(7, "denoised", "dn-1") });
            }
        }
        app
    }

    /// The Versions & Export group as the controls panel draws it at `width`
    /// points (every fold open), cut from the group's caption down to the
    /// Export section's last verb, at 2×.
    fn versions_group_shot(app: &mut AutoShadeApp, theme: crate::theme::ThemePref, width: f32) -> image::RgbaImage {
        app.theme = theme;
        let lang = app.lang;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        let size = egui::vec2(width + 40.0, 16_000.0);
        let mut tex = ShotTextures::default();
        let mut last = None;
        let mut panel = egui::Rect::NOTHING;
        for i in 0..6 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 60.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(2.0);
            let out = ctx.run(input, |ctx| {
                ctx.memory_mut(|m| m.set_everything_is_visible(true));
                panel = controls_panel(ctx, app, width);
            });
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("frames ran");
        let items = probe_items(&out.shapes, panel, None);
        let find = |text: &str| {
            items
                .iter()
                .find(|(kind, t, _)| kind == "text" && t == text)
                .map(|(_, _, r)| *r)
                .unwrap_or_else(|| panic!("{text:?} is drawn"))
        };
        let top = find(tr(lang, "Versions & Export")).top() - 8.0;
        let bottom = find(tr(lang, "Export .xmp beside the photo")).bottom() + 16.0;
        // Cut by the width the drag left the panel, not by the rect its
        // content covered: a row that overruns the panel stretches that
        // rect, and then the tooltips egui's show-everything hook pops
        // (clipped to the screen) would pass a cut made by it.
        let shapes: Vec<_> = out.shapes.iter().filter(|c| c.clip_rect.right() <= width + 1.0).cloned().collect();
        let prims = ctx.tessellate(shapes, out.pixels_per_point);
        let region = egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(width, bottom));
        rasterise_region(&prims, &tex, region, out.pixels_per_point, ctx.style().visuals.panel_fill)
    }

    /// The variant strip alone as the app hosts it (the bottom panel), at 2×,
    /// with a two-tone thumbnail on every card.
    fn strip_shot(app: &mut AutoShadeApp, theme: crate::theme::ThemePref) -> image::RgbaImage {
        app.theme = theme;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        for (i, v) in app.variants.iter_mut().enumerate() {
            let (w, h) = (60usize, 40usize);
            let px: Vec<egui::Color32> = (0..w * h)
                .map(|p| {
                    let y = (p / w) as f32 / h as f32;
                    let hue = 0.08 + 0.4 * i as f32;
                    egui::Color32::from(egui::ecolor::Hsva::new((hue + if y < 0.5 { 0.5 } else { 0.0 }) % 1.0, 0.45, 0.8 - 0.3 * y, 1.0))
                })
                .collect();
            v.thumb = Some(ctx.load_texture(format!("strip-thumb-{i}"), egui::ColorImage { size: [w, h], pixels: px }, egui::TextureOptions::LINEAR));
        }
        let size = egui::vec2(560.0, AutoShadeApp::VARIANT_STRIP_H + 16.0);
        let mut tex = ShotTextures::default();
        let mut last = None;
        for i in 0..6 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 60.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(2.0);
            let out = ctx.run(input, |ctx| {
                egui::TopBottomPanel::bottom("variants")
                    .exact_height(AutoShadeApp::VARIANT_STRIP_H)
                    .show(ctx, |ui| app.variant_strip(ui));
            });
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("frames ran");
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        rasterise(&prims, &tex, size, out.pixels_per_point, theme.colors().canvas_bed)
    }

    /// Not a check — the camera for the Versions & Export group (the
    /// Versions section open, in each [`VersionsScene`], at the narrowest, a
    /// middle and the widest panel width) and for the variant strip with
    /// [▣] and [▣, ◈], in both languages and the Dark and Light themes:
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … versions_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn versions_screenshots() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        let style = egui::Context::default().style();
        let widest = crate::theme::controls_w_max(&style);
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            for theme in [crate::theme::ThemePref::Dark, crate::theme::ThemePref::Light] {
                for scene in VersionsScene::ALL {
                    for width in [crate::theme::CONTROLS_W_MIN, 400.0, widest] {
                        let mut app = app_with_versions(lang, scene);
                        versions_group_shot(&mut app, theme, width)
                            .save(dir.join(format!("versions-{scene:?}-{lang:?}-{theme:?}-{width:.0}.png")))
                            .unwrap();
                    }
                }
                for cards in [1usize, 2] {
                    let mut app = app_with_versions(lang, VersionsScene::TwoCardsThreeVersions);
                    app.variants.truncate(cards);
                    app.active = cards - 1;
                    strip_shot(&mut app, theme).save(dir.join(format!("strip-{cards}-cards-{lang:?}-{theme:?}.png"))).unwrap();
                }
            }
        }
    }

    // ---- The library column (the left-most panel): its scenes, its camera
    // and its pin.

    /// What the library column is showing, for its pictures and its pin.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum LibraryScene {
        /// No folder open, no photo open.
        NoFolder,
        /// A folder open and one of its photos on the canvas.
        Open,
        /// …and that photo's recipe copied.
        Copied,
        /// …or three photos picked, nothing copied.
        Picked,
        /// A recipe copied and three photos picked: every batch verb can run.
        Batch,
        /// A recipe copied and 128 photos picked: the widest counts a label
        /// is expected to carry.
        BigPick,
        /// A folder open in Normal, which runs its batch from the sidebar.
        Normal,
    }

    impl LibraryScene {
        /// The scenes the camera takes.
        const PICTURED: [Self; 4] = [Self::NoFolder, Self::Open, Self::Batch, Self::Normal];
        /// The scenes that draw the batch block over a folder — one layout.
        const WITH_BATCH: [Self; 5] = [Self::Open, Self::Copied, Self::Picked, Self::Batch, Self::BigPick];
    }

    /// An app whose library column shows `scene`. The thumbnails are already
    /// resident (textures of `ctx`) and the ● badge is already answered, so
    /// laying the column starts no decode worker and stats no file.
    fn app_with_library(ctx: &egui::Context, lang: crate::i18n::Lang, scene: LibraryScene) -> AutoShadeApp {
        let mut app = AutoShadeApp { lang, ..Default::default() };
        if scene == LibraryScene::NoFolder {
            return app;
        }
        let dir = PathBuf::from("D:/library/2026-11 canyon trip");
        let names = [
            "frame-0412.arw",
            "frame-0413.arw",
            "frame-0414.arw",
            "a-long-name-for-a-stitched-panorama-of-the-canyon-rim.tif",
            "river-bend.jpg",
            "p7.png",
        ];
        app.gallery = (0..14).map(|i| dir.join(format!("{:02}-{}", i, names[i % names.len()]))).collect();
        app.gallery_dir = Some(dir);
        for i in 0..app.gallery.len() {
            // Landscape, portrait and square frames in turn, each a two-tone
            // field standing in for a photo.
            let (w, h) = [(60usize, 40usize), (40, 60), (48, 48)][i % 3];
            let hue = i as f32 / 14.0;
            let px: Vec<egui::Color32> = (0..w * h)
                .map(|p| {
                    let y = (p / w) as f32 / h as f32;
                    let c = egui::ecolor::Hsva::new((hue + if y < 0.5 { 0.55 } else { 0.05 }) % 1.0, 0.45, 0.75 - 0.3 * y, 1.0);
                    egui::Color32::from(c)
                })
                .collect();
            let image = egui::ColorImage { size: [w, h], pixels: px };
            app.thumbs.insert(i, ctx.load_texture(format!("library-thumb-{i}"), image, egui::TextureOptions::LINEAR));
            app.edited_badge.insert(i, i % 3 == 0);
        }
        app.src_path = Some(app.gallery[2].clone());
        app.selected = Some(2);
        let copied = matches!(scene, LibraryScene::Copied | LibraryScene::Batch | LibraryScene::BigPick);
        if copied {
            app.copied = Some(EditRecipe::default());
            app.copied_from = app.src_path.clone();
        }
        match scene {
            LibraryScene::Picked | LibraryScene::Batch => app.multi_sel = [0usize, 3, 4].into_iter().collect(),
            LibraryScene::BigPick => app.multi_sel = (0usize..128).collect(),
            LibraryScene::Normal => app.choose_mode(Mode::Normal),
            LibraryScene::NoFolder | LibraryScene::Open | LibraryScene::Copied => {}
        }
        app
    }

    /// The library column as the app hosts it (`library_side_panel` around
    /// `gallery_panel`), as if the user had dragged it to `width` points (the
    /// app's range still clamps it), over `frames` frames of a `size`-point
    /// screen at `ppp`: the last frame's output, the panel's rect on it, the
    /// panel's width on every frame, and the textures for a picture.
    fn library_frames(
        ctx: &egui::Context,
        app: &mut AutoShadeApp,
        width: f32,
        size: egui::Vec2,
        ppp: f32,
        frames: usize,
    ) -> (egui::FullOutput, egui::Rect, Vec<f32>, ShotTextures) {
        let stored = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 100.0));
        ctx.data_mut(|d| d.insert_persisted(egui::Id::new("gallery"), egui::containers::panel::PanelState { rect: stored }));
        let mut tex = ShotTextures::default();
        let mut last = None;
        let mut panel = egui::Rect::NOTHING;
        let mut widths = Vec::new();
        for i in 0..frames {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 2.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
            // The LAST frame's buttons only.
            crate::buttons::DRAWN.with_borrow_mut(Vec::clear);
            let out = ctx.run(input, |ctx| {
                panel = crate::side_panel::library_side_panel(ctx, |ui| app.gallery_panel(ui)).response.rect;
            });
            widths.push(panel.width());
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        (last.expect("frames ran"), panel, widths, tex)
    }

    /// User report 2026-11-17 (「最左侧那一栏的按钮布局、美观还需要再优化」), on a
    /// column whose title and verb filled their row edge to edge in Chinese
    /// and left it ragged in English, whose three batch verbs were three
    /// full-width slabs with ✕ alone after the last, whose paste option
    /// appeared only after a copy and pushed the thumbnails down a row, and
    /// whose English option label widened the panel by 4 px.
    ///
    /// The column is ONE layout: at every width of its range and in every
    /// state of the clipboard and the pick, in both languages,
    ///  * the panel is exactly as wide as the drag left it, on every frame;
    ///  * nothing is drawn past the column's right edge;
    ///  * every button's label fits its cell — at the floor width too, with a
    ///    three-digit pick;
    ///  * the four batch verbs stand in two rows of two equal cells whose
    ///    right edge is the one 「Open folder…」 ends on;
    ///  * the paste's option reads whole, on one line;
    ///  * the thumbnails start on the column's left edge, where the cells do,
    ///    and the first one stands at one y — nothing above it changes
    ///    height, so the list never moves under the pointer.
    ///
    /// MUTATIONS THIS CATCHES (each run against this pin, 2026-11-17): the
    /// option drawn only `if self.copied.is_some()` (it is missing from Open,
    /// and where it does show the first thumbnail stands a row lower);
    /// `LIBRARY_W_MIN` lowered to 200 (「Copy recipe」 no longer fits its 88 px
    /// cell); the title row's verb laid after the title instead of at the far
    /// end (it ends 40 px short of the edge); the render verb back on a full
    /// row of its own (the row runs past the panel); the row's frame given
    /// back its side margin (the thumbnails start 3 px in); the option's label
    /// made longer than the floor width holds (195 px of its 325 px are
    /// drawn). NOT caught, on purpose: the option's cut taken away — the
    /// label fits whole at every width of the range, so the cut never runs
    /// here (`panels/gallery.rs` says whom it is for).
    #[test]
    fn the_library_column_is_one_layout_in_every_state() {
        use crate::i18n::Lang;
        let size = egui::vec2(700.0, 900.0);
        for lang in [Lang::En, Lang::Zh] {
            let mut first_rows: Vec<(String, f32)> = Vec::new();
            for asked in [crate::theme::LIBRARY_W_MIN, 300.0, 2000.0] {
                for scene in [LibraryScene::NoFolder, LibraryScene::Normal].into_iter().chain(LibraryScene::WITH_BATCH) {
                    let tag = format!("{lang:?} {scene:?} asked {asked}");
                    let ctx = egui::Context::default();
                    crate::theme::install_fonts(&ctx);
                    crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
                    let mut app = app_with_library(&ctx, lang, scene);
                    let (out, panel, widths, _) = library_frames(&ctx, &mut app, asked, size, 1.0, 4);
                    let drawn = crate::buttons::DRAWN.with_borrow(|d| d.clone());

                    // ① The drag owns the width.
                    let expected = asked.clamp(crate::theme::LIBRARY_W_MIN, crate::theme::library_w_max(&ctx.style()));
                    assert!(
                        widths.iter().all(|w| (w - expected).abs() <= 0.5),
                        "{tag}: the panel is not the {expected} px the drag left it: {widths:?}"
                    );

                    // ② Nothing past the column's right edge.
                    let margin = egui::Frame::side_top_panel(&ctx.style()).inner_margin;
                    let (left, right) = (panel.left() + margin.left, panel.right() - margin.right);
                    let items = probe_items(&out.shapes, panel, None);
                    let past: Vec<String> = items
                        .iter()
                        .filter(|(kind, _, r)| kind == "text" && r.right() > right + 0.5)
                        .map(|(_, text, r)| format!("{text:?} ends at {:.1}", r.right()))
                        .chain(drawn.iter().filter(|d| d.rect.right() > right + 0.5).map(|d| format!("button {:?} ends at {:.1}", d.label, d.rect.right())))
                        .collect();
                    assert!(past.is_empty(), "{tag}: drawn past the column's right edge {right:.1}: {past:#?}");

                    // ③ Every label fits its cell on one line.
                    for d in &drawn {
                        assert!(d.fits, "{tag}: {:?} does not fit its {:.1} px cell", d.label, d.rect.width());
                    }

                    let open = drawn.iter().find(|d| d.label == tr(lang, "🗂 Open folder…")).unwrap_or_else(|| panic!("{tag}: open-folder is drawn"));
                    assert!(
                        (open.rect.right() - right).abs() <= 0.5,
                        "{tag}: 「Open folder…」 ends at {:.1}, not on the column's right edge {right:.1}",
                        open.rect.right()
                    );
                    if !LibraryScene::WITH_BATCH.contains(&scene) {
                        assert_eq!(drawn.len(), 1, "{tag}: only open-folder is a button here: {drawn:#?}");
                        continue;
                    }

                    // ④ Two rows of two equal cells, ending where the title
                    // row's verb ends (the cells stop growing at the
                    // readable ceiling, which is where the panel's cap is).
                    let cells: Vec<&crate::buttons::Drawn> = drawn.iter().filter(|d| d.cell).collect();
                    assert_eq!(cells.len(), 4, "{tag}: four batch verbs, each in a cell: {drawn:#?}");
                    for pair in cells.chunks(2) {
                        let (a, b) = (pair[0], pair[1]);
                        assert!((a.rect.top() - b.rect.top()).abs() <= 0.5, "{tag}: {:?} and {:?} share a row", a.label, b.label);
                        assert!((a.rect.left() - left).abs() <= 0.5, "{tag}: {:?} starts at {:.1}, not at the column's left edge {left:.1}", a.label, a.rect.left());
                        assert!((b.rect.right() - right).abs() <= 0.5, "{tag}: {:?} ends at {:.1}, not at the column's right edge {right:.1}", b.label, b.rect.right());
                        assert!((a.rect.width() - b.rect.width()).abs() <= 0.5, "{tag}: {:?} and {:?} are equal cells", a.label, b.label);
                    }
                    assert!(cells[2].rect.top() > cells[0].rect.bottom(), "{tag}: the second pair stands under the first");

                    // The paste's option reads whole, on one line: the column
                    // cuts a label too long for it, and the label is not to
                    // be one of those at any width of the panel's range.
                    // (A cut galley still answers its whole text, so the cut
                    // is told by its width: narrower than the label laid
                    // out with nothing to stop it.)
                    let option = tr(lang, "Paste crop / straighten too");
                    let line = items
                        .iter()
                        .find(|(kind, text, _)| kind == "text" && text == option)
                        .unwrap_or_else(|| panic!("{tag}: the option {option:?} is drawn"));
                    let row = ctx.style().spacing.interact_size.y;
                    assert!(line.2.height() <= row, "{tag}: the option {option:?} stands {:.1} px tall — more than one line", line.2.height());
                    let font = egui::TextStyle::Button.resolve(&ctx.style());
                    let whole = ctx.fonts(|f| f.layout_no_wrap(option.to_owned(), font, egui::Color32::WHITE).size().x);
                    assert!(
                        line.2.width() >= whole - 0.5,
                        "{tag}: the option {option:?} is cut — {:.1} px of its {whole:.1} px are drawn",
                        line.2.width()
                    );

                    // ⑤ The first thumbnail's place: on the column's left
                    // edge, like the cells above it, and at one y.
                    let slot = app.gallery_slot_rect.unwrap_or_else(|| panic!("{tag}: the first thumbnail was drawn"));
                    assert!(
                        (slot.left() - left).abs() <= 0.5,
                        "{tag}: the thumbnails start at {:.1}, not on the column's left edge {left:.1}",
                        slot.left()
                    );
                    first_rows.push((tag, slot.top()));
                }
            }
            let (tag0, y0) = first_rows[0].clone();
            for (tag, y) in &first_rows {
                assert!(
                    (y - y0).abs() <= 0.5,
                    "the first thumbnail stands at {y:.1} in [{tag}] and at {y0:.1} in [{tag0}] — what is above it changed height"
                );
            }
        }
    }

    /// Not a check — the camera for the library column: the pictured scenes,
    /// both languages, Dark and Light, at the floor width and a wide one.
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … library_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn library_screenshots() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            for theme in [crate::theme::ThemePref::Dark, crate::theme::ThemePref::Light] {
                for scene in LibraryScene::PICTURED {
                    for width in [240.0f32, 340.0] {
                        let ctx = egui::Context::default();
                        crate::theme::install_fonts(&ctx);
                        crate::theme::install_theme(&ctx, theme);
                        let mut app = app_with_library(&ctx, lang, scene);
                        app.theme = theme;
                        let size = egui::vec2(width + 60.0, 520.0);
                        let (out, panel, _, tex) = library_frames(&ctx, &mut app, width, size, 2.0, 4);
                        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
                        let img = rasterise(&prims, &tex, size, out.pixels_per_point, theme.colors().canvas_bed);
                        let crop = image::imageops::crop_imm(&img, 0, 0, (panel.right() * 2.0) as u32 + 2, img.height());
                        crop.to_image().save(dir.join(format!("library-{scene:?}-{lang:?}-{theme:?}-{width}.png"))).unwrap();
                    }
                }
            }
        }
    }
