// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the
// workspace's top bar — its two zones, the dividers, the mode chip, the view
// switch as one segmented control, the Export split as one pill — and the
// library's folder line under it (2026-11-17).

    /// The top bar alone on a `width` × 800 screen, two frames (the second
    /// one's registry); the buttons it drew.
    fn bar_frame(app: &mut AutoShadeApp, width: f32, events: Vec<egui::Event>) -> Vec<crate::buttons::Drawn> {
        use crate::buttons::DRAWN;
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input(Vec::new()), |ctx| app.upd_top_bar(ctx));
        DRAWN.with_borrow_mut(Vec::clear);
        let _ = ctx.run(input(events), |ctx| app.upd_top_bar(ctx));
        DRAWN.with_borrow(|d| d.clone())
    }

    fn drawn_rect(drawn: &[crate::buttons::Drawn], label: &str) -> egui::Rect {
        drawn
            .iter()
            .find(|d| d.label == label)
            .unwrap_or_else(|| panic!("{label:?} was not drawn: {:?}", drawn.iter().map(|d| &d.label).collect::<Vec<_>>()))
            .rect
    }

    /// On a wide window the right zone (view switch, save + export, the two
    /// squares) ends at the row's right limit — just left of the window
    /// buttons — in both languages and both toolbars (full and Normal's), and
    /// nothing of the bar reaches the window-button strip.
    ///
    /// MUTATION THIS CATCHES: the flexible gap before the right zone removed
    /// (the zone then follows the left one and ends hundreds of px short).
    #[test]
    fn the_right_zone_of_the_top_bar_stands_flush_right() {
        let width = 1400.0;
        let limit = width - crate::window_chrome::controls_width() - SPACE_LG;
        for lang in [Lang::En, Lang::Zh] {
            for mode in [Mode::Landscape, Mode::Normal] {
                let mut app = app_with_every_button(lang);
                app.mode = mode;
                let drawn = bar_frame(&mut app, width, Vec::new());
                let last = drawn.iter().map(|d| d.rect.right()).fold(f32::MIN, f32::max);
                assert!(
                    (limit - last).abs() <= SPACE_LG + 1.0,
                    "{lang:?} {mode:?}: the right zone ends at {last:.1}, not near the row's limit {limit:.1}"
                );
                for d in &drawn {
                    assert!(
                        d.rect.right() <= width - crate::window_chrome::controls_width() + 0.5,
                        "{lang:?} {mode:?}: {:?} ends under the window buttons",
                        d.label
                    );
                }
                // The left zone still starts at the left: the chip first.
                let chip = drawn_rect(&drawn, mode.name(lang));
                assert!(chip.left() < 60.0, "{lang:?} {mode:?}: the mode chip leads the bar at {chip:?}");
            }
        }
    }

    /// At the narrowest window the app allows (980 px), in both languages,
    /// the bar wraps only between its groups — no group is split across two
    /// lines — and nothing reaches the window buttons, now with the right
    /// zone present.
    #[test]
    fn the_top_bar_wraps_between_groups_at_the_narrowest_window() {
        let width = 980.0;
        for lang in [Lang::En, Lang::Zh] {
            let mut app = app_with_every_button(lang);
            let drawn = bar_frame(&mut app, width, Vec::new());
            let pairs = [
                (tr(lang, "Open photo…"), tr(lang, "Reset")),
                (tr(lang, "↶ Undo"), tr(lang, "↷ Redo")),
                (tr(lang, "⭯ Turn left"), tr(lang, "⭮ Turn right")),
                (tr(lang, "◫ Compare"), tr(lang, "⬛ Single")),
                (tr(lang, "Save develop"), crate::buttons::bar_label(tr(lang, "Export"))),
                ("⚙", "⌨"),
            ];
            for (a, b) in pairs {
                let (ra, rb) = (drawn_rect(&drawn, a), drawn_rect(&drawn, b));
                assert_eq!(ra.top(), rb.top(), "{lang:?}: {a:?} and {b:?} split across lines");
            }
            for d in &drawn {
                assert!(
                    d.rect.right() <= width - crate::window_chrome::controls_width() + 0.5,
                    "{lang:?}: {:?} ends under the window buttons",
                    d.label
                );
            }
        }
    }

    /// The Export split is one pill, enabled and disabled: its two halves
    /// share one top and one bottom and touch, and in Chinese the main half
    /// says 「导出」 alone — the bar has no room for the second language the
    /// Export fold's heading keeps.
    #[test]
    fn the_export_split_reads_as_one_pill_in_both_states() {
        for lang in [Lang::En, Lang::Zh] {
            for enabled in [true, false] {
                let mut app = app_with_every_button(lang);
                app.busy = !enabled;
                let drawn = bar_frame(&mut app, 1400.0, Vec::new());
                let label = crate::buttons::bar_label(tr(lang, "Export"));
                let main = drawn.iter().find(|d| d.kind == "primary" && d.label == label).map(|d| d.rect);
                let main = main.unwrap_or_else(|| panic!("{lang:?}: no Export half labelled {label:?}"));
                let chevron = drawn_rect(&drawn, "▾");
                assert_eq!((main.top(), main.bottom()), (chevron.top(), chevron.bottom()), "{lang:?} enabled={enabled}: one height");
                assert!((main.right() - chevron.left()).abs() <= 0.5, "{lang:?} enabled={enabled}: the halves touch: {main:?} {chevron:?}");
            }
        }
        assert_eq!(crate::buttons::bar_label(tr(Lang::Zh, "Export")), "导出");
    }

    /// The plate the bar puts under the mode chip and a disabled Export split
    /// stands off the bar on EVERY theme. Measured against `bar_fill`
    /// (WCAG ratio, `theme::contrast`): widget_fill gives Dark 1.333,
    /// Warm ink 1.353, Cool blue 1.363 but Light 1.018 (the bar's own grey);
    /// card_fill gives 1.180–1.195 everywhere. The floor 1.15 sits under the
    /// weakest plate that reads (Light's card_fill, 1.180) and far above
    /// Light's widget_fill.
    ///
    /// MUTATION THIS CATCHES: `bar_plate` returning `widget_fill` always —
    /// Light then measures 1.018 and fails.
    #[test]
    fn the_bar_plate_stands_off_the_bar_on_every_theme() {
        for theme in crate::theme::ThemePref::ALL {
            let c = theme.colors();
            let ratio = crate::theme::contrast(c.bar_plate(), c.bar_fill);
            assert!(ratio >= 1.15, "{theme:?}: the bar plate's contrast against the bar is {ratio:.3}, under 1.15");
        }
    }

    /// The view switch is ONE segmented control: both segments inside one
    /// container, side by side, and a click on the other segment changes
    /// the view.
    #[test]
    fn the_view_switch_is_one_segmented_control() {
        let mut app = app_with_every_button(Lang::En);
        app.view_mode = ViewMode::SideBySide;
        let drawn = bar_frame(&mut app, 1400.0, Vec::new());
        let container = drawn.iter().find(|d| d.kind == "segmented").expect("the container is registered").rect;
        let (a, b) = (drawn_rect(&drawn, "◫ Compare"), drawn_rect(&drawn, "⬛ Single"));
        for s in [a, b] {
            assert!(container.contains_rect(s), "segment {s:?} outside its container {container:?}");
        }
        assert!((a.right() - b.left()).abs() <= 0.5, "the segments sit side by side");
        let at = b.center();
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        // Press and release on one context: `bar_frame` is fresh per call, so
        // drive the two frames here.
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        for (frame, events) in [vec![], vec![egui::Event::PointerMoved(at), press(true)], vec![press(false)]].into_iter().enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
                time: Some(frame as f64 / 10.0),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.upd_top_bar(ctx));
        }
        assert!(app.view_mode == ViewMode::AfterOnly, "the click chose the other segment");
    }

    /// The library's folder line under the bar is ONE row whatever the
    /// path's length: the folder's own name and the count, never the parent
    /// folders (they are in the hover text).
    #[test]
    fn the_library_line_names_the_folder_on_one_row() {
        for lang in [Lang::En, Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.gallery_dir = Some(PathBuf::from("D:/library/a-rather-long-parent-folder/and-another-one-below-it/2026-06-trip-raw"));
            app.gallery = (0..124).map(|i| PathBuf::from(format!("D:/library/p{i}.png"))).collect();
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut out = None;
            for _ in 0..2 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                    ..Default::default()
                };
                out = Some(ctx.run(input, |ctx| {
                    egui::SidePanel::left("gallery").default_width(240.0).show(ctx, |ui| app.gallery_panel(ui));
                }));
            }
            let shapes = out.unwrap().shapes;
            let line = drawn_texts(&shapes).into_iter().find(|t| t.contains("124")).expect("the count line is drawn");
            assert!(!line.contains("a-rather-long-parent-folder"), "{lang:?}: the parent folders are in the line: {line:?}");
            assert!(line.starts_with("2026-06-trip"), "{lang:?}: the line leads with the folder's name: {line:?}");
            let r = text_rects(&shapes, &line)[0];
            let row = ctx.style().text_styles[&egui::TextStyle::Small].size * 1.6;
            assert!(r.height() <= row, "{lang:?}: the line wraps ({:.1} px tall): {line:?}", r.height());
        }
    }
