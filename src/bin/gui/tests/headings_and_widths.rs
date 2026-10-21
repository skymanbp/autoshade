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
            // Two first-level sections (2026-10-21), each a Heading over its
            // own folds: generation and reverse-fit first, analysis second,
            // and the adjust fold sits inside the analysis section.
            let fold = seen[at(tr(lang, "Analysis · paid API"))].1;
            let titles = [tr(lang, "AI generation & reverse-fit"), tr(lang, "AI analysis & auto develop")];
            for title in titles {
                let size = seen[at(title)].1;
                assert!(size > fold + 2.0, "{lang:?}: the 「{title}」 title ({size}) does not out-rank its folds ({fold})");
            }
            let order = [
                at(tr(lang, "AI generation & reverse-fit")),
                at(tr(lang, "Generate whole image · paid API")),
                at(tr(lang, "AI analysis & auto develop")),
                at(tr(lang, "Analysis · paid API")),
                at(tr(lang, "Adjust the selected variant · paid API")),
            ];
            assert!(order.windows(2).all(|w| w[0] < w[1]), "{lang:?}: section heads out of order: {order:?}");
            // The retired verb, by its two old labels (no longer in the table).
            assert!(!seen.iter().any(|(s, _)| s == "AI Refine" || s == "AI 微调"), "{lang:?}: the retired Refine verb is still drawn");
        }
        let panels = [include_str!("../panels/ai.rs"), include_str!("../panels/develop.rs"), include_str!("../panels/retouch.rs")];
        let fences = panels.iter().map(|src| src.matches(".separator()").count()).sum::<usize>();
        assert_eq!(fences, 0, "a side panel (ai / develop / retouch) draws a hairline separator again");
    }

    /// User report 2026-10-13: 「最大宽度会随着展开的编辑选项变化而变化」. egui's
    /// side panel stores the rect its CONTENT covered as next frame's width,
    /// so one over-wide row — here a 900 px allocation standing in for any
    /// fold opened on a long line — ratcheted the panel wider for good. The
    /// panel must keep the width it was laid at, a width remembered from
    /// before must come back inside the range, and a drag to the far right
    /// stops where the content column is exactly the readable ceiling.
    ///
    /// MUTATION THIS CATCHES: `controls_side_panel`'s write-back removed (the
    /// first arm grows to the 900 px row), or its `width_range` dropped (the
    /// second arm restores 1200 px).
    #[test]
    fn the_controls_panel_width_follows_the_drag_not_the_content() {
        let run = |ctx: &egui::Context, row: f32| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
                ..Default::default()
            };
            // The width the panel keeps for the next frame — the response
            // rect is the CONTENT's (wider when it overflows, narrower when a
            // 10 px row does not fill the panel), not the panel's.
            let _ = ctx.run(input, |ctx| {
                crate::side_panel::controls_side_panel(ctx, |ui| {
                    ui.allocate_exact_size(egui::vec2(row, 20.0), egui::Sense::hover());
                });
            });
            egui::containers::panel::PanelState::load(ctx, egui::Id::new("controls")).expect("stored").rect.width()
        };
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let widths: Vec<f32> = (0..4).map(|_| run(&ctx, 900.0)).collect();
        assert!(
            widths.iter().all(|w| (w - crate::theme::CONTROLS_W_DEFAULT).abs() < 0.5),
            "an over-wide row widened the panel: {widths:?}"
        );

        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let id = egui::Id::new("controls");
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        ctx.data_mut(|d| d.insert_persisted(id, egui::containers::panel::PanelState { rect }));
        let max = crate::theme::controls_w_max(&ctx.style());
        let w = run(&ctx, 10.0);
        assert!((w - max).abs() < 0.5, "a remembered 1200 px came back at {w}, not the {max} cap");
        let style = ctx.style();
        let column = max
            - egui::Frame::side_top_panel(&style).inner_margin.sum().x
            - crate::theme::controls_gutter(&style);
        assert!(
            (column - crate::theme::FIELD_W_MAX).abs() < 0.5,
            "at the cap the content column is {column}, not the {} readable ceiling",
            crate::theme::FIELD_W_MAX
        );
    }

    /// User 2026-10-14: 「"展开折叠"的按钮，放上去给阴影动画」. A fold's header
    /// rises under the pointer — a raised plate in the theme's hover fill fades
    /// in — and settles back once the pointer leaves.
    ///
    /// MUTATION THIS CATCHES: `card_fold` never filling its `lift` slot.
    #[test]
    fn a_fold_header_rises_under_the_pointer_and_settles_back() {
        fn has_fill(shapes: &[egui::epaint::ClippedShape], fill: egui::Color32) -> bool {
            fn walk(s: &egui::Shape, fill: egui::Color32) -> bool {
                match s {
                    egui::Shape::Rect(r) => r.fill == fill,
                    egui::Shape::Vec(v) => v.iter().any(|s| walk(s, fill)),
                    _ => false,
                }
            }
            shapes.iter().any(|c| walk(&c.shape, fill))
        }
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let hover = crate::theme::current_theme().colors().widget_hover;
        let header = std::cell::Cell::new(egui::Rect::NOTHING);
        let frame = |t: f64, pointer: Option<egui::Pos2>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                time: Some(t),
                events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = crate::headings::card_fold(ui, egui::CollapsingHeader::new("Fold").default_open(true), |ui| {
                        ui.add_space(40.0);
                    });
                    header.set(r.header_response.rect);
                });
            })
        };
        let rest = frame(0.0, Some(egui::pos2(390.0, 290.0)));
        assert!(!has_fill(&rest.shapes, hover), "a header the pointer is not on is flat");
        let on = header.get().center();
        let mut risen = None;
        for i in 1..=40 {
            risen = Some(frame(i as f64 / 60.0, Some(on)));
        }
        assert!(has_fill(&risen.unwrap().shapes, hover), "under the pointer the header carries its raised plate");
        let mut settled = None;
        for i in 41..=80 {
            settled = Some(frame(i as f64 / 60.0, Some(egui::pos2(390.0, 290.0))));
        }
        let settled = settled.unwrap();
        assert!(!has_fill(&settled.shapes, hover), "the plate fades out once the pointer leaves");
    }
