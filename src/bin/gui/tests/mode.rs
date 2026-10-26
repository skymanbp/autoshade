// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the mode
// page and the three modes' shells (mode.rs, 2026-10-23).

    /// One headless frame of the whole app (`run_frame`, i.e. `update`
    /// without the prefs flush) over a `size`-point screen, with `events`.
    fn app_frame(
        ctx: &egui::Context,
        app: &mut AutoShadeApp,
        size: egui::Vec2,
        frame: usize,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(frame as f64 / 2.0),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| app.run_frame(ctx))
    }

    /// A primary click at `at`: press on one frame, release on the next.
    fn click_at(ctx: &egui::Context, app: &mut AutoShadeApp, size: egui::Vec2, frame: &mut usize, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [vec![egui::Event::PointerMoved(at), button(true)], vec![button(false)]] {
            app_frame(ctx, app, size, *frame, events);
            *frame += 1;
        }
    }

    /// The prefs field round-trips through eframe's own storage path (the
    /// app's `save`, `get_value`, `restore_prefs`), is spelled by the variant
    /// name in the file, and a prefs file written before it existed restores
    /// the workspace every user had: Landscape. The page still opens first
    /// whatever was remembered (ruling 1: no skip).
    #[test]
    fn the_mode_round_trips_through_the_prefs_file_and_old_files_mean_landscape() {
        for mode in Mode::ALL {
            let mut storage = PrefsMemory::default();
            let mut app = AutoShadeApp { mode, ..Default::default() };
            eframe::App::save(&mut app, &mut storage);
            let text = eframe::Storage::get_string(&storage, eframe::APP_KEY).unwrap();
            assert!(text.contains(&format!("mode:{mode:?}")), "{mode:?} is stored by its name: {text}");
            let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
            assert_eq!(prefs.mode, mode);
            let mut back = AutoShadeApp::default();
            back.restore_prefs(prefs);
            assert_eq!(back.mode, mode, "the remembered card");
            assert!(!back.mode_chosen_this_launch, "{mode:?}: the page opens first on every launch");
        }
        for old in ["()", "(lang:Zh,theme:Light)"] {
            let mut storage = PrefsMemory::default();
            eframe::Storage::set_string(&mut storage, eframe::APP_KEY, old.into());
            let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).expect("an older prefs file still loads");
            assert_eq!(prefs.mode, Mode::Landscape, "{old}");
        }
    }

    /// The page, wide and narrow: three cards side by side on a wide window
    /// (one row, equal heights, left to right in the design's order), stacked
    /// on a narrow one, the remembered card marked, and nothing of the
    /// workspace drawn behind it. Rendered through the CPU rasteriser, which
    /// writes the pictures when `AUTOSHADE_SHOT_DIR` is set.
    #[test]
    fn the_mode_page_lays_three_cards_wide_and_stacks_them_narrow() {
        let shots = std::env::var_os("AUTOSHADE_SHOT_DIR").map(PathBuf::from);
        for lang in [Lang::En, Lang::Zh] {
            for (size, wide) in [(egui::vec2(1400.0, 880.0), true), (egui::vec2(600.0, 1400.0), false)] {
                let mut app = AutoShadeApp { lang, mode: Mode::Normal, ..Default::default() };
                let ctx = egui::Context::default();
                crate::theme::install_fonts(&ctx);
                crate::theme::install_theme(&ctx, app.theme);
                let mut tex = ShotTextures::default();
                let mut out = None;
                for i in 0..4 {
                    let o = app_frame(&ctx, &mut app, size, i, Vec::new());
                    tex.apply(&o.textures_delta);
                    out = Some(o);
                }
                let out = out.unwrap();
                let rects = app.mode_card_rects.clone();
                assert_eq!(rects.iter().map(|(m, _)| *m).collect::<Vec<_>>(), Mode::ALL.to_vec());
                for w in rects.windows(2) {
                    let (a, b) = (w[0].1, w[1].1);
                    assert_eq!(a.size(), b.size(), "{lang:?}: one card size");
                    if wide {
                        assert_eq!(a.top(), b.top(), "{lang:?}: one row");
                        assert!(b.left() > a.right(), "{lang:?}: side by side, apart");
                    } else {
                        assert_eq!(a.left(), b.left(), "{lang:?}: one column");
                        assert!(b.top() > a.bottom(), "{lang:?}: stacked, apart");
                    }
                }
                for (_, r) in &rects {
                    assert!(r.left() >= 0.0 && r.right() <= size.x, "{lang:?}: {r:?} inside the window");
                }
                let texts = drawn_texts(&out.shapes);
                for m in Mode::ALL {
                    assert!(texts.iter().any(|t| t == m.name(lang)), "{lang:?}: card {m:?} titled");
                }
                assert!(texts.iter().any(|t| t == tr(lang, "Last used")), "{lang:?}: the remembered card is marked");
                assert!(
                    !texts.iter().any(|t| t == tr(lang, "Open photo…") || t == tr(lang, "No photo open.")),
                    "{lang:?}: the workspace is not drawn behind the page"
                );
                let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
                let img = rasterise(&prims, &tex, size, out.pixels_per_point, egui::Color32::BLACK);
                // The remembered card's ring is the accent fill: some pixel of
                // its edge is that colour, and the other cards' edges are not.
                let ring = app.theme.colors().accent_fill;
                let ppp = out.pixels_per_point;
                let edge_has_ring = |r: egui::Rect| {
                    let (x0, x1) = (((r.left() + 20.0) * ppp) as u32, ((r.right() - 20.0) * ppp) as u32);
                    let top = (r.top() * ppp) as u32;
                    (x0..x1).any(|x| {
                        (top.saturating_sub(2)..top + 3).any(|y| {
                            let p = img.get_pixel(x, y);
                            (0..3).all(|k| (i32::from(p[k]) - i32::from(ring.to_array()[k])).abs() <= 32)
                        })
                    })
                };
                assert!(edge_has_ring(rects[2].1), "{lang:?}: Normal, the remembered mode, wears the ring");
                assert!(!edge_has_ring(rects[0].1), "{lang:?}: Landscape does not");
                if let Some(dir) = &shots {
                    std::fs::create_dir_all(dir).unwrap();
                    let kind = if wide { "wide" } else { "narrow" };
                    img.save(dir.join(format!("mode-page-{lang:?}-{kind}.png"))).unwrap();
                }
            }
        }
    }

    /// The design's §4 table as display switches: which sections each mode
    /// draws, on the photo with every button armed. Pass A opens every fold
    /// (egui's show-everything hook), so a section that is absent is absent
    /// because its mode does not draw it; pass B leaves folds at their
    /// defaults, where Portraits' local-area switches are folded away.
    ///
    /// MUTATION THIS CATCHES: any one switch of `Mode::shell` flipped (e.g.
    /// Normal's `one_click_sidebar` to false, Portraits'
    /// `zoned_folded` to false) fails a row below.
    #[test]
    fn each_mode_draws_the_sections_its_row_of_the_table_names() {
        let everything = [
            "Tone & WB",
            "Presence",
            "Curves",
            "Retouch",
            "Generative Fill",
            "Generate whole image · paid API",
            "Reference library · local",
            "Reverse-fit strength",
            "AI review",
            "Export .xmp beside the photo",
            "Highlights",
            "Reverse-fit recipe",
        ];
        // Normal's own four groups (design-normal-mode.md §3, `panels/normal.rs`).
        let normal = [
            "One-click look",
            "Natural",
            "Black & white",
            "Match my reference photos…",
            "Look strength",
            "Fine-tune",
            "Brightness",
            "Contrast",
            "Warmth",
            "Vividness",
            "Noise",
            "AI denoise",
            "Save",
            "Save as jpg",
            "Apply this look to every photo in the folder",
        ];
        let frame = |mode: Mode, show_all: bool| -> Vec<String> {
            let mut app = app_with_every_button(Lang::En);
            app.mode = mode;
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut out = None;
            for i in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 20_000.0))),
                    time: Some(i as f64),
                    ..Default::default()
                };
                out = Some(ctx.run(input, |ctx| {
                    ctx.memory_mut(|m| m.set_everything_is_visible(show_all));
                    controls_panel(ctx, &mut app, 320.0);
                }));
            }
            drawn_texts(&out.unwrap().shapes)
        };
        let has = |texts: &[String], key: &str| texts.iter().any(|t| t.starts_with(key));
        for mode in [Mode::Landscape, Mode::Portraits] {
            let texts = frame(mode, true);
            for key in everything {
                assert!(has(&texts, key), "{mode:?} draws every section: {key:?} missing");
            }
        }
        let texts = frame(Mode::Normal, true);
        for key in normal {
            assert!(has(&texts, key), "Normal draws {key:?}");
        }
        let shared = [
            "Fit local areas too",
            "Up to four semantic regions",
            "Extract style",
            "From:",
            "Develop",
            "Analysis · paid API",
            "Export",
            "Exposure",
            "Saturation",
            "Tint",
        ];
        for key in everything.iter().chain(&shared) {
            assert!(!has(&texts, key), "Normal does not draw {key:?}");
        }
        // Pass B: folds at their defaults.
        let landscape = frame(Mode::Landscape, false);
        assert!(has(&landscape, "Up to four semantic regions"), "Landscape: the switches stand in the fold, as before");
        assert!(!has(&landscape, "Local areas"));
        let portraits = frame(Mode::Portraits, false);
        assert!(has(&portraits, "Local areas"), "Portraits: the switches have a fold of their own");
        assert!(!has(&portraits, "Up to four semantic regions"), "…closed by default");
    }

    /// The Open dialog offers the same set in every mode, rendered images
    /// first in Normal only.
    #[test]
    fn each_mode_sets_its_open_dialog_order() {
        for mode in Mode::ALL {
            let exts = crate::util::photo_exts_for(mode);
            let mut sorted = exts.clone();
            sorted.sort_unstable();
            let mut all = photo_exts();
            all.sort_unstable();
            assert_eq!(sorted, all, "{mode:?}: the same set");
            if mode == Mode::Normal {
                let n = autoshade::pipeline::BAKED_EXTS.len();
                assert!(exts[..n].iter().all(|e| autoshade::pipeline::BAKED_EXTS.contains(e)), "{exts:?}");
            } else {
                assert_eq!(exts, photo_exts(), "{mode:?}: the dialog is unchanged");
            }
        }
    }

    /// The toolbar's mode button returns to the page and the page's card
    /// enters the workspace again, and through the round trip the open
    /// photo, its cards and its develop are the same: switching a mode only
    /// switches the shell. The click also asks for the prefs to be written.
    #[test]
    fn returning_to_the_mode_page_keeps_the_open_photo() {
        use crate::buttons::DRAWN;
        let size = egui::vec2(1400.0, 880.0);
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut app = app_with_every_button(Lang::En);
        let mut frame = 0;
        // Launch: the page, then a click on Landscape's card.
        app_frame(&ctx, &mut app, size, frame, Vec::new());
        frame += 1;
        assert!(!app.mode_chosen_this_launch);
        let landscape = app.mode_card_rects[0].1.center();
        click_at(&ctx, &mut app, size, &mut frame, landscape);
        assert!(app.mode_chosen_this_launch && app.mode == Mode::Landscape);
        assert!(std::mem::take(&mut app.prefs_save_due), "choosing writes the prefs");
        // The workspace: find the mode button the toolbar drew, and click it.
        // The photo as the workspace holds it after a frame (its first frame
        // may fork an edited card — the workspace's own rule, not the mode's).
        DRAWN.with_borrow_mut(Vec::clear);
        app_frame(&ctx, &mut app, size, frame, Vec::new());
        frame += 1;
        let (photo, cards, recipe) = (app.src_path.clone(), app.variants.len(), app.recipe.clone());
        assert!(photo.is_some());
        let button = DRAWN.with_borrow(|d| d.iter().find(|b| b.label == "Landscape").map(|b| b.rect));
        let button = button.expect("the toolbar shows the mode's name");
        click_at(&ctx, &mut app, size, &mut frame, button.center());
        assert!(!app.mode_chosen_this_launch, "the mode button returns to the page");
        assert_eq!(app.src_path, photo);
        // Choose Normal on the page; the photo comes back in the new shell.
        app_frame(&ctx, &mut app, size, frame, Vec::new());
        frame += 1;
        let normal = app.mode_card_rects[2].1.center();
        click_at(&ctx, &mut app, size, &mut frame, normal);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.mode_chosen_this_launch);
        assert_eq!(app.src_path, photo, "the open photo stays open");
        assert_eq!(app.variants.len(), cards, "its cards too");
        assert_eq!(app.recipe, recipe, "and its develop");
        let texts = drawn_texts(&app_frame(&ctx, &mut app, size, frame, Vec::new()).shapes);
        assert!(texts.iter().any(|t| t == "Normal"), "the toolbar now names Normal");
    }

    /// Not a check — the camera for the workspace in each mode:
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … mode_workspace_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn mode_workspace_screenshots() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        for lang in [Lang::En, Lang::Zh] {
            for mode in Mode::ALL {
                let mut app = app_with_every_button(lang);
                app.mode = mode;
                // Portraits with the people found: its panel's four groups
                // open at the top of the sidebar (`panels/portrait.rs`).
                if mode == Mode::Portraits {
                    app.recipe.masks.extend(found_masks());
                }
                full_window_shot(&mut app, crate::theme::ThemePref::Dark, egui::vec2(1400.0, 880.0))
                    .save(dir.join(format!("workspace-{mode:?}-{lang:?}.png")))
                    .unwrap();
            }
        }
    }
