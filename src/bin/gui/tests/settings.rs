// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the
// Settings window's pages (panels/settings.rs, panels/settings/pages.rs) and
// the mode page's language switch (mode.rs), 2026-11-17.

    /// The screen the Settings tests and camera use: wide and tall enough
    /// that the window takes its fixed 780 × 580.
    const SETTINGS_SCREEN: egui::Vec2 = egui::vec2(1100.0, 760.0);

    /// One headless frame of the Settings window as the app hosts it
    /// (`settings_window`), with `events`.
    fn settings_frame(ctx: &egui::Context, app: &mut AutoShadeApp, frame: usize, events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SETTINGS_SCREEN)),
            time: Some(frame as f64 / 2.0),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| app.settings_window(ctx))
    }

    /// Where each text shape reading exactly `text` was drawn, left to right.
    fn text_rects(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<egui::Rect> {
        fn walk(s: &egui::Shape, text: &str, out: &mut Vec<egui::Rect>) {
            match s {
                egui::Shape::Text(t) if t.galley.text() == text => out.push(t.visual_bounding_rect()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, text, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|c| walk(&c.shape, text, &mut out));
        out.sort_by(|a, b| a.left().total_cmp(&b.left()));
        out
    }

    /// The Settings window, `frames` frames on a fresh context; the last
    /// frame's output. `app.settings_tab` picks the page.
    fn settings_frames(app: &mut AutoShadeApp, frames: usize) -> egui::FullOutput {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut out = None;
        for i in 0..frames {
            out = Some(settings_frame(&ctx, app, i, Vec::new()));
        }
        out.expect("frames ran")
    }

    /// One page drawn whole through `settings_page`, at the width the window
    /// gives its page column, on a screen tall enough that nothing is culled:
    /// the page's texts, and every text drawn past that width.
    fn whole_page(app: &mut AutoShadeApp, tab: crate::panels::settings::SettingsTab) -> (Vec<String>, Vec<String>) {
        // The column's width, as the window lays it.
        app.settings_tab = tab;
        settings_frames(app, 3);
        let width = crate::panels::settings::PAGE_COLUMN.get().width();
        assert!(width > 300.0, "premise: the window laid its page column ({width:.1} px)");
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut out = None;
        for i in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width + 400.0, 20_000.0))),
                time: Some(i as f64 / 2.0),
                ..Default::default()
            };
            out = Some(ctx.run(input, |ctx| {
                egui::Area::new(egui::Id::new("whole_page")).fixed_pos(egui::Pos2::ZERO).show(ctx, |ui| {
                    ui.set_width(width);
                    app.settings_page(ui, tab);
                });
            }));
        }
        let out = out.unwrap();
        fn walk(s: &egui::Shape, clip: egui::Rect, edge: f32, past: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => {
                    let r = t.visual_bounding_rect().intersect(clip);
                    if r.is_positive() && r.right() > edge + 0.5 && !t.galley.text().trim().is_empty() {
                        let head: String = t.galley.text().chars().take(40).collect();
                        past.push(format!("{head:?} right {:.1}", r.right()));
                    }
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, edge, past)),
                _ => {}
            }
        }
        let mut past = Vec::new();
        for c in &out.shapes {
            walk(&c.shape, c.clip_rect, width, &mut past);
        }
        (drawn_texts(&out.shapes), past)
    }

    /// The navigation names the five pages in both languages, and a click on
    /// an entry — a real pointer press and release on the drawn row, through
    /// the window host — opens that page.
    #[test]
    fn the_settings_nav_names_five_pages_and_a_click_opens_one() {
        use crate::panels::settings::SettingsTab;
        for lang in [Lang::En, Lang::Zh] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = app_with_settings_form(lang);
            assert_eq!(app.settings_tab, SettingsTab::General, "the window opens on General");
            let mut frame = 0;
            let mut out = None;
            for _ in 0..3 {
                out = Some(settings_frame(&ctx, &mut app, frame, Vec::new()));
                frame += 1;
            }
            let texts = drawn_texts(&out.unwrap().shapes);
            for tab in SettingsTab::ALL {
                assert!(texts.iter().any(|t| t == tab.label(lang)), "{lang:?}: the nav names {tab:?}: {texts:?}");
            }
            // A text only the page itself draws, per page.
            let marker = |tab: SettingsTab| match tab {
                SettingsTab::General => tr(lang, "Accent colour"),
                SettingsTab::Folders => tr(lang, "Delivery folder"),
                SettingsTab::Models => tr(lang, "Vision model"),
                SettingsTab::Advanced => tr(lang, "Python interpreter"),
                SettingsTab::About => tr(lang, "Version"),
            };
            for tab in [SettingsTab::Models, SettingsTab::Advanced, SettingsTab::About, SettingsTab::Folders, SettingsTab::General] {
                let shapes = settings_frame(&ctx, &mut app, frame, Vec::new()).shapes;
                frame += 1;
                // The nav row is the leftmost drawing of the name (the page
                // title repeats it on the open page).
                let at = text_rects(&shapes, tab.label(lang))[0].center();
                let button = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                for events in [vec![egui::Event::PointerMoved(at), button(true)], vec![button(false)]] {
                    settings_frame(&ctx, &mut app, frame, events);
                    frame += 1;
                }
                assert_eq!(app.settings_tab, tab, "{lang:?}: the click opened {tab:?}");
                let texts = drawn_texts(&settings_frame(&ctx, &mut app, frame, Vec::new()).shapes);
                frame += 1;
                for other in SettingsTab::ALL {
                    assert_eq!(
                        texts.iter().any(|t| t == marker(other)),
                        other == tab,
                        "{lang:?}: on {tab:?}, {other:?}'s page is {} drawn",
                        if other == tab { "not" } else { "also" }
                    );
                }
            }
        }
    }

    /// The form anatomy: on every page, in both languages, every form row's
    /// control cell starts at the same x, and every row this pin names is a
    /// form row at all.
    ///
    /// MUTATION THIS CATCHES: one row built with a plain `ui.horizontal`
    /// instead of `form_row` — its label never reaches `FORM_CELLS`, and its
    /// control would start wherever its label ended.
    #[test]
    fn every_settings_form_row_starts_its_control_at_one_x() {
        use crate::panels::settings::{SettingsTab, FORM_CELLS};
        for lang in [Lang::En, Lang::Zh] {
            // Each label through a literal `tr` call: the i18n audit reads
            // call sites lexically and refuses a key it cannot see.
            let rows: [(SettingsTab, Vec<&str>); 5] = [
                (SettingsTab::General, vec![tr(lang, "Language"), tr(lang, "Theme"), tr(lang, "Accent colour")]),
                (SettingsTab::Folders, vec![tr(lang, "Folder"), tr(lang, "All photos"), tr(lang, "Pixel masters")]),
                (
                    SettingsTab::Models,
                    vec![
                        tr(lang, "Provider"),
                        tr(lang, "Base URL"),
                        tr(lang, "API Key"),
                        tr(lang, "Model"),
                        tr(lang, "Reasoning effort"),
                        tr(lang, "Vision model"),
                        tr(lang, "API style"),
                        tr(lang, "Image-gen model"),
                        tr(lang, "Image-gen API"),
                    ],
                ),
                (SettingsTab::Advanced, vec![tr(lang, "Path")]),
                (SettingsTab::About, vec![tr(lang, "Version")]),
            ];
            for (tab, labels) in rows {
                let mut app = app_with_settings_form(lang);
                app.settings_tab = tab;
                let ctx = egui::Context::default();
                crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
                for i in 0..3 {
                    FORM_CELLS.with_borrow_mut(Vec::clear); // the LAST frame's rows only
                    settings_frame(&ctx, &mut app, i, Vec::new());
                }
                let cells = FORM_CELLS.with_borrow(|c| c.clone());
                for l in labels {
                    assert!(
                        cells.iter().any(|(c, _)| c == l),
                        "{lang:?} {tab:?}: {l:?} is not a form row: {cells:?}"
                    );
                }
                let x0 = cells[0].1;
                for (label, x) in &cells {
                    assert!((x - x0).abs() <= 0.5, "{lang:?} {tab:?}: {label:?}'s control starts at {x:.1}, not {x0:.1}");
                }
            }
        }
    }

    /// At the window's fixed width, in both languages, nothing a page draws
    /// runs past the page column's right edge (the band the scrollbar floats
    /// in) — the whole page, not only its visible part, and every control of
    /// the old single-column panel is still on its page.
    #[test]
    fn every_settings_page_fits_its_column_and_keeps_its_controls() {
        use crate::panels::settings::SettingsTab;
        for lang in [Lang::En, Lang::Zh] {
            // Literal `tr` calls, for the i18n audit (see the pin above).
            let keys: [(SettingsTab, Vec<&str>); 5] = [
                (SettingsTab::General, vec![tr(lang, "Language"), tr(lang, "Theme"), tr(lang, "Accent colour")]),
                (
                    SettingsTab::Folders,
                    vec![
                        tr(lang, "Delivery folder"),
                        tr(lang, "Browse…"),
                        tr(lang, "Develop store"),
                        tr(lang, "Pixel masters"),
                        tr(lang, "🗂 Show in file manager"),
                        tr(lang, "🗂 Show masters folder"),
                        tr(lang, "Import develops from an old ./out folder…"),
                    ],
                ),
                (
                    SettingsTab::Models,
                    vec![
                        tr(lang, "Analysis model — used by AI Analyze, AI adjust and AI review"),
                        tr(lang, "Image model — looks at your photo, and generates or edits images"),
                        tr(lang, "Provider"),
                        tr(lang, "Model"),
                        tr(lang, "Reasoning effort"),
                        tr(lang, "Vision model"),
                        tr(lang, "API style"),
                        tr(lang, "Image-gen model"),
                        tr(lang, "Image-gen API"),
                        tr(lang, "🔄 Fetch models"),
                        tr(lang, "Custom"),
                    ],
                ),
                (SettingsTab::Advanced, vec![tr(lang, "Python interpreter"), tr(lang, "Detect"), tr(lang, "Segmentation helper")]),
                (SettingsTab::About, vec![tr(lang, "Version")]),
            ];
            for (tab, labels) in keys {
                let mut app = app_with_settings_form(lang);
                let (texts, past) = whole_page(&mut app, tab);
                assert!(past.is_empty(), "{lang:?} {tab:?}: drawn past the page column: {past:#?}");
                for l in labels {
                    assert!(texts.iter().any(|t| t == l), "{lang:?} {tab:?}: {l:?} is not drawn: {texts:?}");
                }
            }
        }
    }

    /// 「Save settings」 stands in the footer of the three pages whose fields
    /// it saves and nowhere else: General and About apply at once, and say so.
    #[test]
    fn the_save_button_stands_only_under_the_pages_it_saves() {
        use crate::buttons::DRAWN;
        use crate::panels::settings::SettingsTab;
        for lang in [Lang::En, Lang::Zh] {
            for tab in SettingsTab::ALL {
                let mut app = app_with_settings_form(lang);
                app.settings_tab = tab;
                DRAWN.with_borrow_mut(Vec::clear);
                let out = settings_frames(&mut app, 3);
                let saves = DRAWN.with_borrow(|d| d.iter().any(|b| b.kind == "primary" && b.label == tr(lang, "Save settings")));
                let wants = matches!(tab, SettingsTab::Folders | SettingsTab::Models | SettingsTab::Advanced);
                assert_eq!(saves, wants, "{lang:?} {tab:?}: the Save button");
                let texts = drawn_texts(&out.shapes);
                let at_once = texts.iter().any(|t| t == tr(lang, "Changes on this page apply at once"));
                assert_eq!(at_once, !wants, "{lang:?} {tab:?}: the apply-at-once hint");
            }
        }
    }

    /// Both endonyms stand in the mode page's title bar in both languages —
    /// each in its own script, English left of 中文 — inside the window and
    /// clear of the window-button strip, wide and narrow.
    #[test]
    fn the_mode_page_shows_both_language_names_in_their_own_script() {
        for lang in [Lang::En, Lang::Zh] {
            for size in [egui::vec2(1400.0, 880.0), egui::vec2(600.0, 1400.0)] {
                let mut app = AutoShadeApp { lang, ..Default::default() };
                let ctx = egui::Context::default();
                crate::theme::install_fonts(&ctx);
                crate::theme::install_theme(&ctx, app.theme);
                let mut out = None;
                for i in 0..3 {
                    out = Some(app_frame(&ctx, &mut app, size, i, Vec::new()));
                }
                let shapes = out.unwrap().shapes;
                let strip = size.x - window_chrome::controls_width();
                let mut seen = Vec::new();
                for name in [Lang::En.label(), Lang::Zh.label()] {
                    let r = *text_rects(&shapes, name).first().unwrap_or_else(|| panic!("{lang:?} {size:?}: {name} is not drawn"));
                    assert!(r.left() >= 0.0 && r.top() >= 0.0 && r.bottom() <= size.y, "{lang:?} {size:?}: {name} {r:?} inside the window");
                    assert!(r.right() <= strip, "{lang:?} {size:?}: {name} {r:?} runs into the window buttons (from x {strip})");
                    seen.push(r);
                }
                assert!(seen[0].right() < seen[1].left(), "{lang:?} {size:?}: English stands left of 中文: {seen:?}");
            }
        }
    }

    /// A click on the other language's name switches the language through
    /// `set_language` — the prefs are due to be written — and the very next
    /// frame draws the cards in it; a click back returns.
    #[test]
    fn a_click_on_the_mode_page_language_switch_changes_the_language() {
        let size = egui::vec2(1400.0, 880.0);
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut app = AutoShadeApp::default();
        let mut frame = 0;
        for (to, from) in [(Lang::Zh, Lang::En), (Lang::En, Lang::Zh)] {
            let shapes = app_frame(&ctx, &mut app, size, frame, Vec::new()).shapes;
            frame += 1;
            assert_eq!(app.lang, from);
            let at = text_rects(&shapes, to.label())[0].center();
            click_at(&ctx, &mut app, size, &mut frame, at);
            assert_eq!(app.lang, to, "the click switched the language");
            assert!(std::mem::take(&mut app.prefs_save_due), "{to:?}: the prefs are written at once");
            let texts = drawn_texts(&app_frame(&ctx, &mut app, size, frame, Vec::new()).shapes);
            frame += 1;
            for m in Mode::ALL {
                assert!(texts.iter().any(|t| t == m.name(to)), "{to:?}: card {m:?} titled in the new language: {texts:?}");
            }
            assert!(texts.iter().any(|t| t == tr(to, "Choose a mode")), "{to:?}: the heading too");
            assert!(!app.mode_chosen_this_launch, "{to:?}: switching the language is not choosing a mode");
        }
    }

    /// `set_language` re-says the two RESTING status lines — the greeting and
    /// the folder-scan count — in the new language, and leaves every other
    /// line as it was: a count carrying the ⚠ skipped suffix, an error.
    ///
    /// MUTATION THIS CATCHES: re-saying unconditionally (the error line would
    /// become the greeting), or comparing against the NEW language's line.
    #[test]
    fn set_language_re_says_only_the_resting_status_lines() {
        let greeting = |l| tr(l, "Open a photo, or open a folder to browse your library.");
        let mut app = AutoShadeApp::default();
        assert_eq!(app.status, greeting(Lang::En), "premise: a fresh app greets");
        app.set_language(Lang::Zh);
        assert_eq!((app.lang, app.status.as_str()), (Lang::Zh, greeting(Lang::Zh)));
        assert!(std::mem::take(&mut app.prefs_save_due));
        app.set_language(Lang::En);
        assert_eq!(app.status, greeting(Lang::En));
        for n in [0usize, 1, 5] {
            app.gallery = (0..n).map(|i| PathBuf::from(format!("D:/library/p{i}.png"))).collect();
            app.status = crate::workers::folder_count_line(Lang::En, n);
            app.set_language(Lang::Zh);
            assert_eq!(app.status, crate::workers::folder_count_line(Lang::Zh, n), "{n} photos");
            app.set_language(Lang::En);
            assert_eq!(app.status, crate::workers::folder_count_line(Lang::En, n), "{n} photos, back");
        }
        for other in [
            format!("{} — ⚠ {}", crate::workers::folder_count_line(Lang::En, 5), "2 unreadable item(s) skipped during the folder scan"),
            "scan failed: the folder is gone".to_string(),
        ] {
            app.status = other.clone();
            app.set_language(Lang::Zh);
            assert_eq!(app.status, other, "left alone");
            app.set_language(Lang::En);
        }
    }
