// One part of the GUI's tests (src/bin/gui/tests.rs includes it): Normal
// mode's own sidebar and the §4 hiding switches (panels/normal.rs,
// design-normal-mode.md, design-normal-oneclick.md; 2026-10-23).

    /// One full window in `app`'s mode — toolbar, status bar, strips, both
    /// side panels and the canvas, as `full_window_shot` lays them — and
    /// what it drew: every text, every vocabulary button of the last frame,
    /// and the picture.
    fn window_frame(app: &mut AutoShadeApp, size: egui::Vec2) -> (Vec<String>, Vec<crate::buttons::Drawn>, image::RgbaImage) {
        use crate::buttons::DRAWN;
        let theme = crate::theme::ThemePref::Dark;
        app.theme = theme;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        let (w, h) = (600usize, 400usize);
        let px: Vec<egui::Color32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                egui::Color32::from_rgb((60.0 + 150.0 * y) as u8, (110.0 + 40.0 * x) as u8, (190.0 - 120.0 * y) as u8)
            })
            .collect();
        app.after_tex = Some(ctx.load_texture("normal-photo", egui::ColorImage { size: [w, h], pixels: px }, egui::TextureOptions::LINEAR));
        let mut tex = ShotTextures::default();
        let mut last = None;
        for i in 0..4 {
            DRAWN.with_borrow_mut(Vec::clear);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(i as f64 / 2.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.5);
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
        let texts = drawn_texts(&out.shapes);
        let drawn = DRAWN.with_borrow(Clone::clone);
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        (texts, drawn, rasterise(&prims, &tex, size, out.pixels_per_point, egui::Color32::BLACK))
    }

    /// The photo with every button armed, entered in `mode` the way the mode
    /// page enters it, with the key state fixed (no key: the disclosure line
    /// is drawn too).
    fn app_in(mode: Mode, lang: Lang) -> AutoShadeApp {
        let mut app = app_with_every_button(lang);
        app.choose_mode(mode);
        // Armed again after the mode change put it down, with its status-bar
        // hint: Normal must show neither the tool's marks nor its words even
        // then (`put_down_hidden_tools`).
        app.start_mask_brush(None);
        app.normal.key_present = Some(false);
        app
    }

    /// Design §3 and §4 on one frame: Normal's sidebar is the four groups,
    /// and the toolbar, the library panel, the canvas and the bottom carry
    /// only what the table leaves them. Landscape and Portraits, on the same
    /// photo, still draw every item Normal hides — so each assertion below is
    /// about the mode, not about a fixture that never drew it.
    ///
    /// MUTATION THIS CATCHES: any of Normal's §4 switches in `Mode::shell`
    /// (`one_click_sidebar`, `full_toolbar`, `library_batch_rows`,
    /// `canvas_tool_marks`, `variant_strip`) flipped fails one block below.
    #[test]
    fn normal_draws_its_four_groups_and_hides_the_rest_of_the_workspace() {
        let size = egui::vec2(1400.0, 1000.0);
        let shots = std::env::var_os("AUTOSHADE_SHOT_DIR").map(PathBuf::from);
        for lang in [Lang::En, Lang::Zh] {
            let toolbar_hidden = [
                tr(lang, "Reset"),
                tr(lang, "⭯ Turn left"),
                tr(lang, "⭮ Turn right"),
                tr(lang, "Export"),
                tr(lang, "Save develop"),
                "⌨",
            ]
            .map(str::to_string);
            let library_hidden = [
                tr(lang, "Copy recipe").to_string(),
                trf(lang, "Paste to selected ({n})", &[("n", "1")]),
                trf(lang, "Render selected ({n})", &[("n", "1")]),
            ];
            let canvas_hidden = [
                tr(lang, "Mask brush — paint to select · 「Erase」 removes · 「Apply」 bakes · Esc cancels"),
                tr(lang, "▨ Bitmap mask"),
                "1280px",
            ]
            .map(str::to_string);
            let strip = tr(lang, "▣ Original").to_string();
            let groups = [tr(lang, "One-click look"), tr(lang, "Fine-tune"), tr(lang, "Noise"), tr(lang, "Save")];
            let presets = [
                tr(lang, "Natural"),
                tr(lang, "Bright & clean"),
                tr(lang, "Warm"),
                tr(lang, "Cool night"),
                tr(lang, "Film"),
                tr(lang, "Black & white"),
            ];
            let rows = [
                tr(lang, "Match my reference photos…"),
                tr(lang, "Save as jpg"),
                tr(lang, "Apply this look to every photo in the folder"),
            ];
            let sliders = [
                tr(lang, "Look strength"),
                tr(lang, "Brightness"),
                tr(lang, "Contrast"),
                tr(lang, "Warmth"),
                tr(lang, "Vividness"),
                tr(lang, "AI denoise"),
            ];
            for mode in Mode::ALL {
                let mut app = app_in(mode, lang);
                let (texts, drawn, img) = window_frame(&mut app, size);
                let labels: Vec<&str> = drawn.iter().map(|d| d.label.as_str()).collect();
                let has_text = |s: &str| texts.iter().any(|x| x == s);
                let has_button = |s: &str| labels.contains(&s);
                let percent = texts.iter().any(|x| x.ends_with('%') && x[..x.len() - 1].parse::<f32>().is_ok());
                let normal = mode == Mode::Normal;
                // Toolbar.
                for keep in [mode.name(lang), tr(lang, "Open photo…"), tr(lang, "↶ Undo"), tr(lang, "↷ Redo"), "⚙"] {
                    assert!(has_button(keep), "{mode:?} {lang:?}: the toolbar keeps {keep:?}");
                }
                assert!(has_text(tr(lang, "◫ Compare")), "{mode:?} {lang:?}: compare is drawn");
                for gone in &toolbar_hidden {
                    assert_eq!(has_button(gone.as_str()), !normal, "{mode:?} {lang:?}: toolbar {gone:?}");
                }
                assert_eq!(has_text(tr(lang, "⬛ Single")), !normal, "{mode:?} {lang:?}: one compare switch in Normal");
                // Library panel.
                assert!(has_button(tr(lang, "🗂 Open folder…")), "{mode:?} {lang:?}: open-folder stays");
                for gone in &library_hidden {
                    assert_eq!(has_button(gone.as_str()), !normal, "{mode:?} {lang:?}: library {gone:?}");
                }
                // Canvas.
                for keep in [tr(lang, "Fit"), "1:1"] {
                    assert!(has_button(keep), "{mode:?} {lang:?}: the canvas keeps {keep:?}");
                }
                for gone in &canvas_hidden {
                    assert_eq!(texts.iter().any(|x| x == gone), !normal, "{mode:?} {lang:?}: canvas {gone:?}");
                }
                assert_eq!(percent, !normal, "{mode:?} {lang:?}: the zoom readout");
                // The brush armed above: put down in Normal (no ring on the
                // canvas — it is drawn only while `paint_mode` is on), left
                // armed where its tools are drawn.
                assert_eq!(app.paint_mode, !normal, "{mode:?} {lang:?}: the armed brush");
                assert!(has_text(&app.status), "{mode:?} {lang:?}: the status bar is in the frame's texts");
                // Bottom: the variant strip.
                assert_eq!(has_text(&strip), !normal, "{mode:?} {lang:?}: the variant strip");
                // Sidebar: Normal's four groups, and nothing of the shared panels.
                for group in groups {
                    assert_eq!(has_text(group), normal, "{mode:?} {lang:?}: group {group:?}");
                }
                assert_eq!(texts.iter().any(|x| x.starts_with(tr(lang, "AI generation & reverse-fit"))), !normal, "{mode:?} {lang:?}: the shared panels");
                if normal {
                    for preset in presets {
                        assert!(has_button(preset), "{lang:?}: preset card {preset:?}");
                    }
                    for row in rows {
                        assert!(has_button(row), "{lang:?}: {row:?}");
                    }
                    for label in sliders {
                        assert!(texts.iter().any(|x| x.starts_with(label)), "{lang:?}: {label:?}");
                    }
                    assert!(has_text(tr(lang, "To make looks with AI, add a service key in Settings (⚙).")), "{lang:?}: the no-key line");
                }
                if let Some(dir) = &shots {
                    std::fs::create_dir_all(dir).unwrap();
                    img.save(dir.join(format!("window-{mode:?}-{lang:?}.png"))).unwrap();
                }
            }
        }
    }

    /// Design §5: 「没有一个术语」 — every text a Normal frame draws, in both
    /// languages, is free of the workspace's trade words. The frame is the
    /// whole window, status bar included, over the photo with every button
    /// armed and a brush armed after the mode was entered (its status-bar
    /// hint says 「蒙版」 / "mask").
    ///
    /// MUTATION THIS CATCHES: draw the shared panels in Normal (flip
    /// `one_click_sidebar`), keep the canvas tool hint or the variant strip,
    /// drop `choose_mode`'s status line, or drop `put_down_hidden_tools`
    /// (the brush hint stays in the status bar), and a word below is found.
    #[test]
    fn a_normal_frame_says_no_trade_words_in_either_language() {
        let words = ["配方", "显影", "反推", "变体", "蒙版", "API", "recipe", "develop", "reverse-fit", "variant", "mask"];
        for lang in [Lang::En, Lang::Zh] {
            let mut app = app_in(Mode::Normal, lang);
            let (mut texts, _, _) = window_frame(&mut app, egui::vec2(1400.0, 1000.0));
            assert!(texts.len() > 40, "{lang:?}: a whole window was drawn ({} texts)", texts.len());
            // The look changed: the status bar's marker is drawn and scanned,
            // and so are both its states' words and its hover (a hover is
            // drawn only under the pointer, so its text is read directly).
            app.recipe.exposure_ev += 0.5;
            let (marked, _, _) = window_frame(&mut app, egui::vec2(1400.0, 1000.0));
            assert!(marked.iter().any(|t| t == tr(lang, "unsaved changes")), "{lang:?}: the marker is drawn");
            texts.extend(marked);
            for mark in [crate::panels::normal::LookMark::Unsaved, crate::panels::normal::LookMark::Saved] {
                let (label, hover) = mark.words(lang);
                texts.push(label.to_string());
                texts.push(hover.to_string());
            }
            for text in &texts {
                let low = text.to_lowercase();
                for w in words {
                    let hit = if w.is_ascii() && w != "API" { low.contains(w) } else { text.contains(w) };
                    assert!(!hit, "{lang:?}: {text:?} says {w:?}");
                }
            }
            // …and the same check has teeth: the Landscape frame says them,
            // in its status bar too (the brush hint).
            let mut landscape = app_in(Mode::Landscape, lang);
            let (texts, _, _) = window_frame(&mut landscape, egui::vec2(1400.0, 1000.0));
            assert!(
                texts.iter().any(|t| words.iter().any(|w| t.to_lowercase().contains(&w.to_lowercase()))),
                "{lang:?}: the Landscape frame is the control"
            );
            let hint = landscape.status.to_lowercase();
            assert!(hint.contains("mask") || hint.contains("蒙版"), "{lang:?}: the brush hint: {hint}");
            assert!(texts.contains(&landscape.status), "{lang:?}: the status bar is scanned");
        }
    }

    /// A stand-in for the paid chain: the no-key path's own proposer
    /// (`advisor::HeuristicProposer`, what `produce_recipe` falls back to
    /// without an image key) over a dark frame, verified Accept.
    fn heuristic_runner(
        _photo: &std::path::Path,
        req: &autoshade::pipeline::GradeRequest,
    ) -> anyhow::Result<(EditRecipe, autoshade::advisor::Verdict, Vec<autoshade::rationale::Note>)> {
        let mut luma = vec![0u32; 256];
        luma[40] = 1000;
        let hist = autoshade::decode::Histogram {
            luma: luma.clone(),
            r: luma.clone(),
            g: luma.clone(),
            b: luma,
            clip_black_pct: 0.0,
            clip_white_pct: 0.0,
            sample_pixels: 1000,
        };
        let (recipe, note) = autoshade::advisor::HeuristicProposer::default().propose_noted(&hist, req.strength)?;
        let verdict = autoshade::advisor::Verdict {
            decision: autoshade::advisor::Decision::Accept,
            reasons: Vec::new(),
            revised_hint: None,
        };
        Ok((recipe, verdict, vec![note]))
    }

    /// Poll the app's workers until the one-click's analysis has landed.
    fn land_analysis(app: &mut AutoShadeApp, ctx: &egui::Context) {
        let start = std::time::Instant::now();
        while app.analyze_inflight {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the analysis never landed");
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.poll_workers(ctx);
        }
    }

    /// A photo the store keys by path (nothing reads the file), its develop
    /// folder scrubbed afterwards.
    fn normal_photo(tag: &str) -> (AutoShadeApp, PathBuf, Scrub) {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-normal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join(format!("_gui_normal_{tag}.jpg"));
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        app.choose_mode(Mode::Normal);
        app.normal.runner = Some(heuristic_runner);
        (app, src, Scrub(vec![dir, dev]))
    }

    /// A preset card's click builds a DIRECTION request carrying that
    /// preset's text, firm adherence, no library, no reference photos, no
    /// deep thinking, at the look strength — every field the design fixes.
    ///
    /// MUTATION THIS CATCHES: any field of `one_click_request`'s preset arm
    /// changed (the guide, the text, the adherence, the embed switch, the
    /// reference photos, the strength's source) fails an assert below.
    #[test]
    fn a_preset_click_asks_for_that_presets_direction() {
        use crate::buttons::DRAWN;
        let (mut app, _src, _scrub) = normal_photo("preset");
        app.grade_strength = 0.4;
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let size = egui::vec2(1400.0, 1000.0);
        let mut frame = 0;
        app.normal.key_present = Some(true);
        app_frame(&ctx, &mut app, size, frame, Vec::new());
        frame += 1;
        DRAWN.with_borrow_mut(Vec::clear);
        app_frame(&ctx, &mut app, size, frame, Vec::new());
        frame += 1;
        let mono = DRAWN.with_borrow(|d| d.iter().find(|b| b.label == "Black & white").map(|b| (b.rect, b.cell)));
        let (rect, in_cell) = mono.expect("the Black & white card is drawn");
        assert!(in_cell, "a preset card is a grid cell");
        click_at(&ctx, &mut app, size, &mut frame, rect.center());
        let (req, text, library) = app.normal.last_request.clone().expect("the click started an analysis");
        assert_eq!(req.guide, autoshade::pipeline::Guide::Direction);
        assert_eq!(text.as_deref(), autoshade::advisor::presets::preset_guidance("mono"));
        assert!(text.unwrap().contains("set convert_to_grayscale to true"));
        assert_eq!(req.adherence.get(), crate::panels::normal::PRESET_ADHERENCE);
        assert!(req.adherence.get() > 0.7, "a firm brief");
        assert!(!req.send_reference_image && !req.think && !req.embed.on());
        assert_eq!(req.library_style(), 0.0, "the library is not read");
        assert_eq!(req.strength.get(), 0.4, "the look strength");
        assert_eq!(library, autoshade::library::NORMAL_NAME);
        land_analysis(&mut app, &ctx);
    }

    /// The folder chain: a built Normal library hands over to the analysis
    /// at once, and the request is the design's — guided by the LIBRARY (its
    /// own, `library::NORMAL_NAME`), Style 1.0 (reproduce the references),
    /// the reference photos sent along, the search on. A failed build says
    /// why and asks nothing.
    ///
    /// MUTATION THIS CATCHES: flip the guide, lower the style below 0.85,
    /// turn `send_reference_image` off, pass the default library, or drop the
    /// hand-off in `on_one_click_built` — each fails an assert below.
    #[test]
    fn the_folder_chain_builds_the_library_request() {
        let (mut app, _src, _scrub) = normal_photo("folder");
        let ctx = egui::Context::default();
        app.normal.building = true;
        app.on_one_click_built(Lang::En, Err("no photos in that folder".into()));
        assert!(app.normal.last_request.is_none(), "a failed build asks nothing");
        assert!(!app.normal.building);
        assert!(matches!(&app.normal.line, crate::panels::normal::NormalLine::Failed(t) if t.contains("no photos")));
        app.on_one_click_built(Lang::En, Ok(3));
        let (req, text, library) = app.normal.last_request.clone().expect("the build handed over to the analysis");
        assert_eq!(req.guide, autoshade::pipeline::Guide::Library);
        assert_eq!(req.style, 1.0);
        assert!(req.style >= autoshade::library::STYLE_TARGET_MIN, "the references are the look to reproduce");
        assert!(req.send_reference_image, "the reference photos go along");
        assert!(req.embed.on(), "the library is searched");
        assert!(!req.think);
        assert_eq!(text, None, "no typed direction");
        assert_eq!(library, autoshade::library::NORMAL_NAME, "Normal's own library");
        assert_eq!(app.normal.line, crate::panels::normal::NormalLine::Working);
        land_analysis(&mut app, &ctx);
    }

    /// With no image key the chain still lands: the built-in proposer's
    /// recipe reaches `on_analyzed`, goes into the sliders, is undoable, and
    /// the one-click line says the built-in look was used.
    #[test]
    fn without_a_key_the_one_click_lands_the_built_in_look() {
        let (mut app, _src, _scrub) = normal_photo("nokey");
        let ctx = egui::Context::default();
        assert_eq!(app.recipe.exposure_ev, 0.0);
        app.start_one_click(crate::panels::normal::OneClick::Preset("natural"));
        assert!(app.busy && app.analyze_inflight);
        land_analysis(&mut app, &ctx);
        assert!(!app.busy);
        assert!(app.recipe.exposure_ev > 0.0, "the dark frame was brightened: {}", app.recipe.exposure_ev);
        assert_eq!(app.normal.line, crate::panels::normal::NormalLine::BuiltIn);
        assert_eq!(app.status, "No AI key set — built-in look used");
        assert_eq!(app.normal.key_present, Some(false));
        let (photo, kept) = app.normal.result.clone().expect("the one-click result is remembered");
        assert_eq!(Some(photo), app.src_path);
        assert_eq!(kept.exposure_ev, app.recipe.exposure_ev);
        // Undoable, like every analysis landing.
        app.commit_now();
        app.undo(&ctx);
        assert_eq!(app.recipe.exposure_ev, 0.0, "Ctrl+Z goes back");
    }

    /// 「Warmth」 is the colour temperature as an offset from the camera's own,
    /// in hundredths of an octave: 0 is the as-shot value, ±100 halve or
    /// double it, and the round trip is exact inside the track.
    #[test]
    fn warmth_is_an_octave_either_side_of_the_as_shot_temperature() {
        use crate::panels::normal::{kelvin_for_warmth, warmth_of};
        let shot = EditRecipe { as_shot_k: Some(5000.0), ..Default::default() };
        assert_eq!(warmth_of(&shot), 0.0, "no custom white balance reads 0");
        assert_eq!(kelvin_for_warmth(&shot, 0.0), 5000.0);
        assert_eq!(kelvin_for_warmth(&shot, 100.0), 10000.0);
        assert_eq!(kelvin_for_warmth(&shot, -100.0), 2500.0);
        for w in [-80.0, -33.0, 0.0, 12.0, 64.0] {
            let r = EditRecipe { temperature_k: Some(kelvin_for_warmth(&shot, w)), ..shot.clone() };
            assert!((warmth_of(&r) - w).abs() < 1e-3, "{w}: {}", warmth_of(&r));
        }
        // A rendered image carries no as-shot value: the develop panel's 5500 K.
        assert_eq!(kelvin_for_warmth(&EditRecipe::default(), 0.0), 5500.0);
        // The track's ends hold.
        let cold = EditRecipe { as_shot_k: Some(3000.0), ..Default::default() };
        assert_eq!(kelvin_for_warmth(&cold, -100.0), 2000.0);
        assert_eq!(warmth_of(&EditRecipe { temperature_k: Some(40000.0), ..shot }), 100.0);
    }

    /// The first-download size the noise switch names is the rendered-image
    /// denoiser's own: its weight plus its network file, as the sidecar pins
    /// them.
    #[test]
    fn the_noise_switch_names_the_downloads_real_size() {
        let py = include_str!("../../../../python/denoise.py");
        let number = |after: &str| -> u64 {
            let at = py.find(after).unwrap_or_else(|| panic!("{after} moved")) + after.len();
            py[at..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
        };
        let weight = {
            let block = &py[py.find("WEIGHT_BYTES = {").expect("WEIGHT_BYTES")..];
            let line = block.lines().find(|l| l.contains("\"color_real_psnr\"")).expect("the default model's size");
            line.chars().filter(char::is_ascii_digit).collect::<String>().parse::<u64>().unwrap()
        };
        let network = number("NETWORK_BYTES =");
        let mb = ((weight + network) as f64 / 1e6).round() as u32;
        assert_eq!(mb, crate::panels::normal::DENOISE_FIRST_DOWNLOAD_MB, "{weight} + {network} bytes");
    }

    /// Poll the app's workers until nothing is running.
    fn land_all(app: &mut AutoShadeApp, ctx: &egui::Context) {
        let start = std::time::Instant::now();
        while app.busy {
            assert!(start.elapsed() < std::time::Duration::from_secs(60), "the worker never landed");
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.poll_workers(ctx);
        }
    }

    /// Ctrl+S and Ctrl+E in Normal take 「Save as jpg」's own path
    /// (`start_normal_save`), never the workspace's develop save or export
    /// dialog: a Save-group delivery starts, into the destination setting's
    /// folder, as a jpg.
    ///
    /// MUTATION THIS CATCHES: route either key back to `save_xmp` /
    /// `start_export` in `upd_shortcuts` and no Save-group delivery starts.
    #[test]
    fn in_normal_the_save_keys_take_the_save_as_jpg_path() {
        for k in [egui::Key::S, egui::Key::E] {
            let tag = format!("keys-{k:?}");
            let (mut app, _src, _scrub) = normal_photo(&tag);
            let out = std::env::temp_dir().join(format!("autoshade-gui-normal-{tag}-out-{}", std::process::id()));
            let _scrub_out = Scrub(vec![out.clone()]);
            app.exp_dest = ExportDest::LastUsed;
            app.last_export_dir = Some(out.clone());
            let ctx = egui::Context::default();
            let mut input = egui::RawInput { modifiers: egui::Modifiers::COMMAND, ..Default::default() };
            input.events.push(egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            });
            let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
            assert!(app.normal.delivering, "{k:?}: the Save group's delivery started");
            assert!(app.busy, "{k:?}: rendering");
            assert!(app.status.contains(".jpg"), "{k:?}: a jpg: {}", app.status);
            assert!(app.status.contains(&out.file_name().unwrap().to_string_lossy().into_owned()), "{k:?}: {}", app.status);
            land_all(&mut app, &ctx);
        }
        // The library rows' key twins (Ctrl+Shift+C copies the look) are dead
        // where the rows are not drawn; Landscape is the control.
        for mode in [Mode::Normal, Mode::Landscape] {
            let mut app = AutoShadeApp { src_path: Some(PathBuf::from("phone.jpg")), mode, ..Default::default() };
            let ctx = egui::Context::default();
            let mods = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
            let input = egui::RawInput { modifiers: mods, events: vec![egui::Event::Copy], ..Default::default() };
            let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
            assert_eq!(app.copied.is_some(), mode != Mode::Normal, "{mode:?}: Ctrl+Shift+C");
        }
    }

    /// 「Apply this look to every photo in the folder」 settles the jpgs'
    /// folder before anything is pasted: a cancelled folder dialog changes no
    /// photo and leaves the status line empty; a picked folder is carried to
    /// the render.
    ///
    /// MUTATION THIS CATCHES: keep the old status on cancel, or paste before
    /// asking (`apply_all` would not carry the folder), and an assert fails.
    #[test]
    fn apply_to_every_photo_asks_for_the_folder_before_it_pastes() {
        let (mut app, src, _scrub) = normal_photo("applyall");
        let ctx = egui::Context::default();
        app.gallery = vec![src.clone()];
        app.exp_dest = ExportDest::Ask;
        app.status = "Recipe pasted onto 1 photo".into();
        app.normal.folder_pick = Some(None);
        app.start_apply_to_folder();
        assert_eq!(app.status, "", "a cancelled dialog leaves nothing on the line");
        assert!(!app.busy && app.normal.apply_all.is_none(), "and nothing was pasted");
        let out = std::env::temp_dir().join(format!("autoshade-gui-normal-applyall-out-{}", std::process::id()));
        let _scrub_out = Scrub(vec![out.clone()]);
        app.normal.folder_pick = Some(Some(out.clone()));
        app.start_apply_to_folder();
        assert!(app.busy, "the paste runs");
        assert_eq!(app.normal.apply_all.as_deref(), Some(out.as_path()), "into the folder picked first");
        land_all(&mut app, &ctx);
        assert!(app.normal.apply_all.is_none());
    }

    /// The Save group says where the jpg goes, in plain words: never beside
    /// the photo — the app's output folder (Settings), the folder last saved
    /// to, or a question each time.
    #[test]
    fn the_save_line_names_where_the_jpg_goes() {
        for lang in [Lang::En, Lang::Zh] {
            let mut app = app_in(Mode::Normal, lang);
            app.exp_dest = ExportDest::LastUsed;
            app.last_export_dir = Some(std::env::temp_dir().join("Deliveries"));
            let (texts, _, _) = window_frame(&mut app, egui::vec2(1400.0, 1000.0));
            let last = trf(lang, "Saves to the folder you last saved to: {folder}", &[("folder", "Deliveries")]);
            assert!(texts.contains(&last), "{lang:?}: {last:?}");
            app.exp_dest = ExportDest::OutFolder;
            let (texts, _, _) = window_frame(&mut app, egui::vec2(1400.0, 1000.0));
            let root = autoshade::config::delivery_root();
            let name = root.file_name().map_or_else(|| abs_display(&root), |n| n.to_string_lossy().into_owned());
            let out = trf(lang, "Saves to the app's output folder {folder} (change it in Settings ⚙)", &[("folder", &name)]);
            assert!(texts.contains(&out), "{lang:?}: {out:?}");
            app.exp_dest = ExportDest::Ask;
            let (texts, _, _) = window_frame(&mut app, egui::vec2(1400.0, 1000.0));
            assert!(texts.iter().any(|t| t == tr(lang, "Asks where to save each time")), "{lang:?}: ask");
        }
    }

    /// The words around the noise switch say, plainly, that it takes a
    /// moment and the preview then shows what gets saved (never "applied at
    /// save" any more); the first-download size follows while the model is
    /// not known to be on disk and the photo is a rendered image; without the
    /// helper the no-tool line stands alone. None of them says a trade word.
    #[test]
    fn the_noise_lines_say_it_takes_a_moment_and_the_first_run_downloads_the_model() {
        let words = ["配方", "显影", "反推", "变体", "蒙版", "API", "recipe", "develop", "reverse-fit", "variant", "mask", "sidecar"];
        for lang in [Lang::En, Lang::Zh] {
            let plain = tr(lang, "It takes a moment; the preview then shows the result, and that is what gets saved.").to_string();
            let mut app = AutoShadeApp { lang, src_path: Some(PathBuf::from("phone.jpg")), ..Default::default() };
            assert_eq!(app.noise_lines(lang, false), vec![tr(lang, "This download does not include the noise tool.").to_string()]);
            app.normal.denoise_ready = Some(true);
            assert_eq!(app.noise_lines(lang, true), vec![plain.clone()], "{lang:?}: the model is there");
            for ready in [Some(false), None] {
                app.normal.denoise_ready = ready;
                let lines = app.noise_lines(lang, true);
                assert_eq!(lines.len(), 2, "{lang:?} {ready:?}: {lines:?}");
                assert_eq!(lines[0], plain);
                assert!(lines[1].contains("72 MB"), "{lang:?}: {lines:?}");
            }
            app.src_path = Some(PathBuf::from("camera.arw"));
            assert_eq!(app.noise_lines(lang, true), vec![plain.clone()], "{lang:?}: a RAW downloads nothing new");
            let (moment, preview) = match lang {
                Lang::En => ("takes a moment", "preview"),
                Lang::Zh => ("稍等", "预览"),
            };
            assert!(plain.contains(moment) && plain.contains(preview), "{lang:?}: {plain}");
            let mut said = app.noise_lines(lang, true);
            app.src_path = Some(PathBuf::from("phone.jpg"));
            said.extend(app.noise_lines(lang, true));
            said.extend(
                [
                    tr(lang, "Cleans the grain out of the photo, and the preview shows the result — it helps night and dark phone photos most; it uses the graphics card when there is one. Turn it off to see the photo without it."),
                    tr(lang, "Removing the grain… this takes a moment."),
                    tr(lang, "AI denoise done. Turn the switch off to see the photo without it."),
                    tr(lang, "AI denoise on"),
                    tr(lang, "AI denoise off"),
                ]
                .map(str::to_string),
            );
            for text in &said {
                assert!(!text.contains("when you save") && !text.contains("保存时处理"), "{lang:?}: {text:?}");
                let low = text.to_lowercase();
                for w in words {
                    let hit = if w.is_ascii() && w != "API" { low.contains(w) } else { text.contains(w) };
                    assert!(!hit, "{lang:?}: {text:?} says {w:?}");
                }
            }
        }
    }

    /// One AI denoise, not two: Normal's switch IS the Retouch panel's
    /// (`start_ai_denoise`) — it meets that door's own gate, runs its worker
    /// and lands through its message (`Msg::Retouched` with a ◈ card) — so
    /// the canvas shows the result and the save renders that card. Off goes
    /// back to the card it was made from; on again shows the ◈ card already
    /// made with no second run; the look the sliders made rides across both
    /// ways.
    ///
    /// MUTATION THIS CATCHES: call anything but `start_ai_denoise` on the
    /// way on (the turned-photo gate and the worker's own failure are not
    /// met), drop the `after_normal_denoise` hook (the ◈ card is not
    /// remembered and on-again runs a second time), switch to the wrong card
    /// on the way off, or bring back the export-time denoise in
    /// `start_normal_save`.
    #[test]
    fn the_noise_switch_is_the_retouch_denoise_and_round_trips() {
        use crate::panels::normal::DenoisePair;
        let (mut app, src, _scrub) = normal_photo("denoise");
        let ctx = egui::Context::default();
        let b0 = std::sync::Arc::new(image::DynamicImage::new_rgba8(4, 4));
        app.variants = vec![Variant {
            id: ORIGINAL_VARIANT_ID.into(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(b0.clone()),
            origin: None,
            thumb: None,
        }];
        app.active = 0;
        app.base_preview = Some(b0);
        app.reset_history();
        // The Retouch door's own gate answers: a turned photo is refused
        // before any worker, with that door's words.
        app.recipe.quarter_turns = 1;
        app.set_normal_denoise(true);
        assert!(!app.busy, "refused before a worker");
        assert!(app.status.contains("this photo is turned"), "{}", app.status);
        assert!(app.normal.denoise_pair.is_none(), "a refused run remembers nothing");
        app.recipe.quarter_turns = 0;
        // The door's worker runs: this photo has no file, so it fails fast at
        // the decode — through `Msg::Retouched`, the Retouch button's message.
        app.set_normal_denoise(true);
        assert!(app.busy, "the denoise worker runs: {}", app.status);
        assert_eq!(app.status, tr(Lang::En, "Removing the grain… this takes a moment."));
        assert!(app.normal_denoise_on(), "the switch reads on while it runs");
        land_all(&mut app, &ctx);
        assert!(!app.normal_denoise_on(), "a failed run leaves the switch off");
        assert_eq!(app.variants.len(), 1, "and no card");
        // A landed run, through the same message the worker sends.
        app.normal.denoise_pair =
            Some(DenoisePair { photo: src.clone(), plain: ORIGINAL_VARIANT_ID.into(), denoised: None });
        let master = std::path::PathBuf::from("out/_gui_normal_denoise.denoise.png");
        let note = RetouchNote::Denoised { out: master.clone(), on_mosaic: false };
        let done = Ok((image::DynamicImage::new_rgba8(4, 4), note, master.clone(), RetouchKind::NewDenoised));
        app.tx.send(Msg::Retouched(app.gen_epoch, Box::new(done))).unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants[app.active].kind, VariantKind::Denoised, "the ◈ card is on the canvas");
        assert_eq!(app.active_source_path().as_deref(), Some(master.as_path()), "…and is what a save renders");
        assert!(app.normal_denoise_on());
        let made = app.variants[app.active].id.clone();
        assert_eq!(app.normal.denoise_pair.as_ref().and_then(|d| d.denoised.clone()), Some(made.clone()), "remembered");
        assert_eq!(app.status, tr(Lang::En, "AI denoise done. Turn the switch off to see the photo without it."));
        // Off: the card it was made from, the look kept.
        app.recipe.exposure_ev = 0.7;
        app.set_normal_denoise(false);
        assert!(!app.busy);
        assert_eq!(app.variants[app.active].id, ORIGINAL_VARIANT_ID, "back to the un-denoised card");
        assert_eq!(app.active_source_path().as_deref(), Some(src.as_path()));
        assert_eq!(app.recipe.exposure_ev, 0.7, "the look rides across");
        assert!(!app.normal_denoise_on());
        assert_eq!(app.status, tr(Lang::En, "AI denoise off"));
        // On again: the same ◈ card, no second run.
        app.recipe.contrast = 12.0;
        app.set_normal_denoise(true);
        assert!(!app.busy, "no second sidecar run");
        assert_eq!(app.variants.len(), 2, "no second card");
        assert_eq!(app.variants[app.active].id, made, "the master already made");
        assert_eq!((app.recipe.exposure_ev, app.recipe.contrast), (0.7, 12.0), "the look rides across");
        assert_eq!(app.status, tr(Lang::En, "AI denoise on"));
        // The switch's way on is the Retouch door, and the save carries no
        // denoise of its own (pinned on the source, the house pattern for a
        // worker whose sidecar cannot run offline).
        let now = include_str!("../panels/normal.rs");
        let on = &now[now.find("pub(crate) fn set_normal_denoise(").expect("set_normal_denoise moved")..];
        let on = &on[..on.find("pub(crate) fn after_normal_denoise(").expect("after_normal_denoise moved")];
        assert!(on.contains("self.start_ai_denoise();"), "the switch no longer runs the Retouch denoise");
        let save = &now[now.find("pub(crate) fn start_normal_save(").expect("start_normal_save moved")..];
        let save = &save[..save.find("pub(crate) fn start_apply_to_folder(").expect("start_apply_to_folder moved")];
        assert!(save.contains("start_render_with(out, export, None, false, how)"), "the save runs a denoise of its own again");
        assert!(!now.contains("default_strength_for"), "the export-time denoise came back");
    }

    /// The status bar's marker in Normal follows the jpg, not the develop
    /// record: nothing while the look is as the photo opened; 「unsaved
    /// changes」 once it changes; 「saved as jpg」 after a Save as jpg lands;
    /// 「unsaved changes」 again on the next change; and 「apply this look to
    /// every photo」 counts as saved for the open photo too. Real renders of a
    /// real (tiny) jpg into a scratch folder.
    ///
    /// MUTATION THIS CATCHES: drop the re-anchor on a landed save, draw the
    /// workspace's develop marker in Normal, or leave the open photo out of
    /// the folder delivery, and a step below fails.
    #[test]
    fn the_normal_marker_flips_with_the_jpg() {
        use crate::panels::normal::LookMark;
        let (mut app, src, _scrub) = normal_photo("mark");
        image::RgbImage::from_pixel(32, 24, image::Rgb([120, 90, 60])).save(&src).unwrap();
        let out = std::env::temp_dir().join(format!("autoshade-gui-normal-mark-out-{}", std::process::id()));
        let _scrub_out = Scrub(vec![out.clone()]);
        app.exp_dest = ExportDest::LastUsed;
        app.last_export_dir = Some(out.clone());
        app.normal.key_present = Some(false);
        let ctx = egui::Context::default();
        let size = egui::vec2(1400.0, 1000.0);
        let drawn = |app: &mut AutoShadeApp, text: &str| window_frame(app, size).0.iter().any(|t| t == text);
        assert_eq!(app.normal_look_mark(), None, "as opened: nothing to say");
        app.recipe.exposure_ev = 0.4;
        assert_eq!(app.normal_look_mark(), Some(LookMark::Unsaved));
        assert!(drawn(&mut app, tr(Lang::En, "unsaved changes")), "the status bar says so");
        app.start_normal_save();
        assert!(app.busy, "the save runs: {}", app.status);
        land_all(&mut app, &ctx);
        assert_eq!(app.normal_look_mark(), Some(LookMark::Saved), "{}", app.status);
        assert!(drawn(&mut app, tr(Lang::En, "saved as jpg")), "the status bar says so");
        assert!(!drawn(&mut app, tr(Lang::En, "unsaved changes")));
        app.recipe.contrast = 20.0;
        assert_eq!(app.normal_look_mark(), Some(LookMark::Unsaved), "changed again");
        // The folder delivery saves the open photo too (into a folder of its
        // own: a batch never overwrites the jpg the single save just wrote).
        let out2 = std::env::temp_dir().join(format!("autoshade-gui-normal-mark-all-{}", std::process::id()));
        let _scrub_out2 = Scrub(vec![out2.clone()]);
        app.exp_dest = ExportDest::Ask;
        app.normal.folder_pick = Some(Some(out2));
        app.gallery = vec![src.clone()];
        app.start_apply_to_folder();
        land_all(&mut app, &ctx);
        assert_eq!(app.normal_look_mark(), Some(LookMark::Saved), "{}", app.status);
    }

    /// A second save into a name the app made never replaces the first and
    /// is never refused: it gets a number — `-2`, then `-3` — claimed on disk
    /// (`claim_numbered`), and the finished line names the file actually
    /// written. A file the user picks in the save window (which confirms a
    /// replace itself) is theirs: it is replaced, not numbered — and the next
    /// app-made name is numbered again. 「Apply to every photo」 into a folder
    /// already saved once writes every photo as `-2`, and says how many got a
    /// number. Landscape, the control, keeps the library's refusal to replace
    /// a photo outside its output areas.
    ///
    /// MUTATION THIS CATCHES: drop `OutName::Numbered` on either Normal path
    /// (the guard refuses the second save), number the picked file
    /// (`OutName::Chosen` → `Numbered`), claim without `create_new` stepping
    /// (the first jpg is replaced), or name the bare file in the landing,
    /// and an assert below fails.
    #[test]
    fn a_second_save_gets_a_number_and_never_replaces_the_first() {
        use crate::panels::normal::LookMark;
        let (mut app, src, _scrub) = normal_photo("numbered");
        image::RgbImage::from_pixel(32, 24, image::Rgb([60, 90, 120])).save(&src).unwrap();
        let src2 = src.with_file_name("_gui_normal_numbered_b.jpg");
        image::RgbImage::from_pixel(32, 24, image::Rgb([90, 60, 120])).save(&src2).unwrap();
        let _scrub2 = Scrub(vec![autoshade::store::develop_dir(&src2)]);
        let out = std::env::temp_dir().join(format!("autoshade-gui-normal-numbered-out-{}", std::process::id()));
        let _scrub_out = Scrub(vec![out.clone()]);
        app.exp_dest = ExportDest::LastUsed;
        app.last_export_dir = Some(out.clone());
        let ctx = egui::Context::default();
        let names = ["_gui_normal_numbered.developed.jpg", "_gui_normal_numbered.developed-2.jpg", "_gui_normal_numbered.developed-3.jpg"];
        let mut sizes = Vec::new();
        for (i, name) in names.iter().enumerate() {
            app.recipe.exposure_ev = 0.2 * (i + 1) as f32;
            app.start_normal_save();
            assert!(app.busy, "save {}: {}", i + 1, app.status);
            assert!(app.status.contains(name), "save {}: the line names the file: {}", i + 1, app.status);
            land_all(&mut app, &ctx);
            let folder = out.file_name().unwrap().to_string_lossy().into_owned();
            assert_eq!(app.status, trf(Lang::En, "Saved {file} to {folder}", &[("file", name), ("folder", &folder)]));
            assert_eq!(app.normal_look_mark(), Some(LookMark::Saved), "{}", app.status);
            sizes.push(std::fs::metadata(out.join(name)).map(|m| m.len()).unwrap_or(0));
        }
        assert!(sizes.iter().all(|&n| n > 0), "three real jpgs, none replaced by the next: {sizes:?}");
        // A file picked in the save window: replaced, not numbered.
        let picked = out.join(names[0]);
        let before = std::fs::read(&picked).unwrap();
        app.exp_dest = ExportDest::Ask;
        app.normal.file_pick = Some(Some(picked.clone()));
        app.recipe.exposure_ev = -1.5;
        app.start_normal_save();
        assert!(app.busy, "the picked save runs: {}", app.status);
        land_all(&mut app, &ctx);
        let folder = out.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.status, trf(Lang::En, "Saved {file} to {folder}", &[("file", names[0]), ("folder", &folder)]));
        assert_ne!(std::fs::read(&picked).unwrap(), before, "the picked file was replaced");
        let fourth = out.join("_gui_normal_numbered.developed-4.jpg");
        assert!(!fourth.exists(), "a picked file is never numbered");
        // …while the app-made name is numbered again.
        app.exp_dest = ExportDest::LastUsed;
        app.last_export_dir = Some(out.clone());
        app.recipe.exposure_ev = 0.9;
        app.start_normal_save();
        land_all(&mut app, &ctx);
        assert!(fourth.is_file(), "the app-made name steps on to -4: {}", app.status);
        // The whole folder, twice: the second run numbers every photo.
        app.gallery = vec![src.clone(), src2.clone()];
        let all = std::env::temp_dir().join(format!("autoshade-gui-normal-numbered-all-{}", std::process::id()));
        let _scrub_all = Scrub(vec![all.clone()]);
        app.last_export_dir = Some(all.clone());
        for run in 0..2 {
            app.recipe.contrast = 10.0 * (run + 1) as f32;
            app.start_apply_to_folder();
            land_all(&mut app, &ctx);
            let folder = all.file_name().unwrap().to_string_lossy().into_owned();
            let saved = trf(Lang::En, "{n} photos saved to {folder}", &[("n", "2"), ("folder", &folder)]);
            if run == 0 {
                assert_eq!(app.status, saved);
            } else {
                let numbered = trf(Lang::En, "{k} of them have a number added to the name, so no earlier photo was replaced.", &[("k", "2")]);
                assert_eq!(app.status, format!("{saved}. {numbered}"));
            }
        }
        let mut written: Vec<String> =
            std::fs::read_dir(&all).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        written.sort();
        assert_eq!(
            written,
            [
                "_gui_normal_numbered.developed-2.jpg",
                "_gui_normal_numbered.developed.jpg",
                "_gui_normal_numbered_b.developed-2.jpg",
                "_gui_normal_numbered_b.developed.jpg",
            ],
            "every photo once bare, once with -2"
        );
        // Landscape keeps the library's refusal (outside its output areas).
        app.mode = Mode::Landscape;
        app.start_render_to(out.join(names[0]));
        assert!(!app.busy);
        assert!(app.status.contains("refusing to overwrite"), "{}", app.status);
    }

    /// The marker says 「saved as jpg」 only while the card on screen, with its
    /// current look, is what was last saved — so the noise switch, which
    /// changes the picture and not the look, flips it: saved; denoised card
    /// on → unsaved; saved again → saved; switch off → unsaved (the original
    /// card was saved, but before the denoised one); on again → saved (that
    /// card, that look, is the last jpg).
    ///
    /// MUTATION THIS CATCHES: drop the card from `look_mark`'s comparison,
    /// or record the look without the card on a landed save, and a step
    /// below fails.
    #[test]
    fn the_normal_marker_follows_the_card_on_screen() {
        use crate::panels::normal::{DenoisePair, LookMark};
        let (mut app, src, _scrub) = normal_photo("markcard");
        image::RgbImage::from_pixel(32, 24, image::Rgb([120, 90, 60])).save(&src).unwrap();
        let master = src.with_file_name("_gui_normal_markcard.denoise.png");
        image::RgbImage::from_pixel(32, 24, image::Rgb([118, 92, 61])).save(&master).unwrap();
        let out = std::env::temp_dir().join(format!("autoshade-gui-normal-markcard-out-{}", std::process::id()));
        let _scrub_out = Scrub(vec![out.clone()]);
        app.exp_dest = ExportDest::LastUsed;
        app.last_export_dir = Some(out.clone());
        let b0 = std::sync::Arc::new(image::DynamicImage::new_rgba8(4, 4));
        app.variants = vec![Variant {
            id: ORIGINAL_VARIANT_ID.into(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(b0.clone()),
            origin: None,
            thumb: None,
        }];
        app.active = 0;
        app.base_preview = Some(b0);
        app.reset_history();
        let ctx = egui::Context::default();
        assert_eq!(app.normal_look_mark(), None, "as opened");
        app.recipe.exposure_ev = 0.4;
        app.start_normal_save();
        land_all(&mut app, &ctx);
        assert_eq!(app.normal_look_mark(), Some(LookMark::Saved), "{}", app.status);
        // The switch's ◈ card lands (through the worker's message): same
        // look, another picture.
        app.normal.denoise_pair = Some(DenoisePair { photo: src.clone(), plain: ORIGINAL_VARIANT_ID.into(), denoised: None });
        let note = RetouchNote::Denoised { out: master.clone(), on_mosaic: false };
        let done = Ok((image::DynamicImage::new_rgba8(4, 4), note, master.clone(), RetouchKind::NewDenoised));
        app.tx.send(Msg::Retouched(app.gen_epoch, Box::new(done))).unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.active_source_path().as_deref(), Some(master.as_path()), "the ◈ card is on screen");
        assert_eq!(app.recipe.exposure_ev, 0.4, "the look is unchanged");
        assert_eq!(app.normal_look_mark(), Some(LookMark::Unsaved), "denoised: not what was saved");
        app.start_normal_save();
        assert!(app.busy, "{}", app.status);
        land_all(&mut app, &ctx);
        assert_eq!(app.normal_look_mark(), Some(LookMark::Saved), "the ◈ card saved: {}", app.status);
        app.set_normal_denoise(false);
        assert_eq!(app.active_source_path().as_deref(), Some(src.as_path()));
        assert_eq!(app.normal_look_mark(), Some(LookMark::Unsaved), "off: the last jpg is the denoised one");
        app.set_normal_denoise(true);
        assert_eq!(app.normal_look_mark(), Some(LookMark::Saved), "on again: that card and look are the last jpg");
    }

    /// The refusal Normal's saves still meet — into the photo's own folder,
    /// which the library guard keeps read-only (`SourceFolderRefused`) —
    /// reads as one plain sentence on both saving paths, matched by type:
    /// the single save (refused before it renders, no claim left behind) and
    /// 「apply to every photo」 (refused per photo inside the batch, nothing
    /// marked saved). Landscape, the control, keeps the library's own line.
    ///
    /// MUTATION THIS CATCHES: let a numbered save step past this refusal,
    /// stop counting it by type in the batch worker, or stop mapping it on
    /// the single path, and an assert below fails.
    #[test]
    fn a_save_into_the_photos_own_folder_is_refused_plainly_in_normal() {
        use crate::panels::normal::LookMark;
        let (mut app, src, _scrub) = normal_photo("ownfolder");
        image::RgbImage::from_pixel(32, 24, image::Rgb([60, 90, 120])).save(&src).unwrap();
        let own = src.parent().unwrap().to_path_buf();
        app.exp_dest = ExportDest::LastUsed;
        app.last_export_dir = Some(own.clone());
        let ctx = egui::Context::default();
        let plain = tr(Lang::En, "AutoShade never saves into the folder your photos are in. Pick another folder.");
        assert_eq!(app.normal_look_mark(), None, "as opened");
        app.recipe.exposure_ev = 0.3;
        app.start_normal_save();
        assert!(!app.busy);
        assert_eq!(app.status, plain);
        assert!(app.toasts.iter().any(|t| t.text == plain), "the toast says it too");
        app.gallery = vec![src.clone()];
        app.start_apply_to_folder();
        land_all(&mut app, &ctx);
        assert_eq!(app.status, plain);
        assert_eq!(app.normal_look_mark(), Some(LookMark::Unsaved), "nothing was saved");
        let files: Vec<_> = std::fs::read_dir(&own).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(files, vec![src.file_name().unwrap().to_os_string()], "nothing written beside the photo");
        app.mode = Mode::Landscape;
        app.start_render_to(own.join(app.export_file_name_as("jpg")));
        assert!(app.status.contains("refusing to write into"), "{}", app.status);
    }
