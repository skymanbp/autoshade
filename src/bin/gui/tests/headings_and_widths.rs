// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the side
// panel's heading levels and the reverse-fit From menu's width (2026-10-01).

    /// The AI panel with a ▣ and a ◈ card standing on the ◈ one, so the
    /// reverse-fit fold draws its From menu.
    fn app_with_fit_sources(lang: crate::i18n::Lang) -> AutoShadeApp {
        let master: std::path::PathBuf = "out/_headings_test.denoise.png".into();
        let variants = [(VariantKind::Original, ORIGINAL_VARIANT_ID, None), (VariantKind::Denoised, "dn", Some(master))]
            .into_iter()
            .map(|(kind, id, origin)| Variant {
                kind,
                origin,
                id: id.to_owned(),
                name: None,
                recipe: Default::default(),
                base: None,
                thumb: None,
            })
            .collect();
        AutoShadeApp { lang, variants, active: 1, ..Default::default() }
    }

    /// Two frames of the AI panel in a side panel `width` px wide (the panel
    /// settles its width on the first); every text shape of the last frame
    /// with its font size, and `buttons::DRAWN` holding that frame's buttons.
    fn ai_panel_at(app: &mut AutoShadeApp, width: f32) -> Vec<(String, f32)> {
        fn texts(s: &egui::Shape, out: &mut Vec<(String, f32)>) {
            if let egui::Shape::Text(t) = s {
                let size = t.galley.job.sections.first().map_or(0.0, |s| s.format.font_id.size);
                out.push((t.galley.text().to_owned(), size));
            } else if let egui::Shape::Vec(v) = s {
                v.iter().for_each(|s| texts(s, out));
            }
        }
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let screen = egui::Rect::from_x_y_ranges(0.0..=1600.0, 0.0..=20_000.0);
        let mut seen = Vec::new();
        for _ in 0..2 {
            crate::buttons::DRAWN.with_borrow_mut(Vec::clear);
            let out = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
                ctx.memory_mut(|m| m.set_everything_is_visible(true));
                egui::SidePanel::left("controls")
                    .default_width(width)
                    .show(ctx, |ui| egui::ScrollArea::vertical().show(ui, |ui| app.ai_panel(ui)));
            });
            seen.clear();
            out.shapes.iter().for_each(|c| texts(&c.shape, &mut seen));
        }
        seen
    }

    /// User report 2026-10-01 (screenshot: the 「起点」 menu running the whole
    /// width of a dragged-wide panel): the reverse-fit From menu is a
    /// row-filling control and keeps the panel's readable ceiling — it ends on
    /// the same right edge as the two verbs under it, at the default width and
    /// at a wide one.
    ///
    /// MUTATION THIS CATCHES: `.width(ui.available_width())` back on the menu.
    #[test]
    fn the_reverse_fit_from_menu_ends_where_the_verb_row_ends() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            for width in [320.0, 900.0] {
                let mut app = app_with_fit_sources(lang);
                ai_panel_at(&mut app, width);
                let menu = app.fit_from_rect.expect("the From menu was drawn");
                let extract = tr(lang, "Extract style");
                let verb = crate::buttons::DRAWN
                    .with_borrow(|d| d.iter().find(|d| d.label == extract).map(|d| d.rect))
                    .expect("the Extract style verb was drawn");
                assert!(
                    (menu.right() - verb.right()).abs() <= 1.5,
                    "{lang:?} at {width} px: the From menu ends at {:.1}, the verb row at {:.1}",
                    menu.right(),
                    verb.right()
                );
            }
        }
    }

    /// User report 2026-10-01 (「看不出哪些子类别是属于哪些大类别的」): the AI
    /// panel's title out-ranks its folds — it is drawn in the Heading size the
    /// Develop and Retouch titles use, larger than a fold's title — and its
    /// folds sit under two named group heads, as Develop's sit under theirs.
    /// The same frame pins the other user report of that day
    /// (「这些分类线太丑了」) on the source: the three side panels draw no
    /// hairline fences, because a separator is a shape with no text to find.
    ///
    /// MUTATION THIS CATCHES: the AI title back at Button size; a group head
    /// dropped; a `ui.separator()` back in a side panel.
    #[test]
    fn the_ai_title_outranks_its_folds_and_its_folds_sit_under_group_heads() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = app_with_fit_sources(lang);
            let seen = ai_panel_at(&mut app, 320.0);
            let at = |t: &str| {
                seen.iter()
                    .position(|(s, _)| s == t)
                    .unwrap_or_else(|| panic!("{lang:?}: 「{t}」 was not drawn: {seen:?}"))
            };
            let title = seen[at(tr(lang, "AI"))].1;
            let fold = seen[at(tr(lang, "Analysis · paid API"))].1;
            assert!(title > fold + 2.0, "{lang:?}: the AI title ({title}) does not out-rank its folds ({fold})");
            let order = [
                at(tr(lang, "Analysis & References")),
                at(tr(lang, "Analysis · paid API")),
                at(tr(lang, "Generate & Reverse-fit")),
                at(tr(lang, "Reimagine (whole image) · paid API")),
            ];
            assert!(order.windows(2).all(|w| w[0] < w[1]), "{lang:?}: group heads out of order: {order:?}");
        }
        let panels = [include_str!("../panels/ai.rs"), include_str!("../panels/develop.rs"), include_str!("../panels/retouch.rs")];
        let fences = panels.iter().map(|src| src.matches(".separator()").count()).sum::<usize>();
        assert_eq!(fences, 0, "a side panel (ai / develop / retouch) draws a hairline separator again");
    }
