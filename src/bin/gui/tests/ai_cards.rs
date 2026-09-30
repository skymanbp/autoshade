// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the pristine AI card and what lands beside it: edits, versions, fills, adjusts, paints, retouches, reverse fits, denoises and stacks.

    /// The immutability rule (2026-09-13, user decision): an edit made while
    /// the pristine ✨ card is active continues on a new ✎ card on the same
    /// raster; the ✨ card goes back to neutral, keeps its identity and its
    /// name, and the canvas is untouched. The frame hook that applies it
    /// sits between the panels and the develop dispatch.
    #[test]
    fn an_edit_on_the_pristine_ai_card_continues_on_a_new_edited_card() {
        let (mut app, _ctx, _dir, _src, master, ai_px, _scrub) =
            ai_card_fixture("fork", EditRecipe::default());
        assert_eq!(strip_kinds(&app), vec![VariantKind::Original, VariantKind::Generated]);
        assert_eq!(app.active, 1);
        assert!(app.recipe.is_noop(), "premise: the ✨ card is pristine");
        assert!(!app.unsaved_marker_dirty(), "premise: a clean open — {}", app.status);
        assert!(!app.fork_edited_card(), "a neutral recipe on the ✨ card forks nothing");
        assert_eq!(app.variants.len(), 2);

        app.recipe.contrast = 7.0;
        app.dirty = true;
        assert!(app.fork_edited_card(), "the first edit forks");
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Generated, VariantKind::Edited]
        );
        assert_eq!(app.active, 2);
        assert_eq!(app.variants[2].recipe.contrast, 7.0);
        assert!(app.variants[1].recipe.is_noop(), "the ✨ card is pristine again");
        assert_eq!(app.variants[1].id, "gen-1", "…and keeps its identity");
        assert_eq!(app.variants[1].name.as_deref(), Some("sky"), "…and its name");
        assert!(
            !app.variants[2].id.is_empty() && app.variants[2].id != "gen-1",
            "the ✎ card is born with its own identity"
        );
        assert_eq!(app.variants[2].name, None);
        assert!(
            app.variants[2].base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &ai_px)),
            "the same pixels"
        );
        assert_eq!(app.variants[2].origin.as_deref(), Some(master.as_path()));
        assert_eq!(app.recipe.contrast, 7.0, "the canvas is untouched");
        assert!(app.dirty, "…and its pending develop still runs");
        assert!(!app.fork_edited_card(), "an ✎ card never forks");
        assert_eq!(app.variants.len(), 3);
        assert!(app.unsaved_marker_dirty(), "a new card is unsaved work");
        assert!(app.toasts.iter().any(|t| t.text.contains("✎")), "said by toast");

        // The frame hook sits between the panels (where the sliders write
        // the recipe) and the develop dispatch — pinned in the source, since
        // a headless test cannot run eframe's update().
        let frame = include_str!("../app.rs");
        let panels = frame.find("self.upd_strips_and_side_panels(ctx);").expect("the panels call");
        let hook = frame[panels..]
            .find("self.fork_edited_card();")
            .map(|i| i + panels)
            .expect("the frame hook after the panels");
        let dispatch = frame.find("if self.dirty && !self.develop_inflight {").expect("the dispatch");
        assert!(hook < dispatch, "the hook runs before the develop is dispatched");
    }

    /// A version loaded onto the pristine ✨ card is an edit like any other.
    #[test]
    fn a_version_loaded_onto_the_pristine_ai_card_lands_on_an_edited_card() {
        let (mut app, _ctx, _dir, src, _master, _ai_px, _scrub) =
            ai_card_fixture("version", EditRecipe::default());
        std::fs::write(
            autoshade::store::version_target(&src, 1),
            serde_json::to_string(&EditRecipe {
                contrast: 7.0,
                base_curve: vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        app.load_version(1);
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Generated, VariantKind::Edited],
            "{}",
            app.status
        );
        assert_eq!(app.active, 2);
        assert_eq!(app.recipe.contrast, 7.0);
        assert!(app.recipe.base_curve.is_empty(), "calibration stripped onto AI pixels");
        assert!(app.variants[1].recipe.is_noop(), "the ✨ card stays pristine");
        assert!(app.status.starts_with("Loaded version v1"), "{}", app.status);
    }

    /// The Analyze landing forks BEFORE it persists, so the record it writes
    /// already names the ✎ card — and the pristine card is saved pristine.
    #[test]
    fn an_analyze_landing_on_the_pristine_ai_card_forks_before_it_saves() {
        let (mut app, _ctx, _dir, src, master, _ai_px, _scrub) =
            ai_card_fixture("analyze", EditRecipe::default());
        let epoch = app.gen_epoch;
        app.on_analyzed(
            Lang::En,
            epoch,
            Box::new(Ok((
                EditRecipe {
                    contrast: 7.0,
                    base_curve: vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]],
                    ..Default::default()
                },
                autoshade::advisor::Verdict {
                    decision: autoshade::advisor::Decision::Accept,
                    reasons: Vec::new(),
                    revised_hint: None,
                },
                Vec::new(),
            ))),
        );
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Generated, VariantKind::Edited],
            "{}",
            app.status
        );
        assert_eq!(app.active, 2);
        assert_eq!(app.recipe.contrast, 7.0);
        assert!(app.recipe.base_curve.is_empty());
        assert!(app.variants[1].recipe.is_noop());
        let rec = saved_strip_of(&src);
        assert_eq!(rec.active_kind, "edited");
        assert_eq!(rec.active_pos, 2);
        assert_eq!(
            rec.others.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            vec!["original", "generated"]
        );
        assert!(rec.others[1].recipe.is_noop(), "the pristine card is saved pristine");
        assert_eq!(rec.others[1].id.as_deref(), Some("gen-1"));
        let disk: EditRecipe = serde_json::from_str(
            &std::fs::read_to_string(autoshade::store::recipe_target(&src)).unwrap(),
        )
        .unwrap();
        assert_eq!(disk.contrast, 7.0);
        assert!(!disk.base_curve.is_empty(), "the disk form keeps the calibration");
        let (origin, generated) = autoshade::store::read_pixel_source(&src).expect("pixels.json");
        assert!(generated);
        assert_eq!(origin, master);
        assert!(!autoshade::pipeline::xmp_target(&src).exists(), "no projection over AI pixels");
        assert!(!app.unsaved_marker_dirty(), "the landing saved the forked strip: {}", app.status);
    }

    /// 2026-09-15: a generative fill is NOT an in-place retouch — the model
    /// was shown the card's picture, so its answer is a picture whose look
    /// lives in its pixels: it lands as a NEW ✨ card (pristine recipe, the
    /// artifact as origin, auto-switched to), and the card it was filled
    /// from keeps its recipe, base and origin. The ▣ negative is untouched
    /// by construction: `negative_origin` still reads the ▣ card.
    #[test]
    fn a_fill_lands_as_a_new_generated_card_and_leaves_the_filled_card_alone() {
        let ctx = egui::Context::default();
        let src = std::path::PathBuf::from("_fill_new_card_test.ARW");
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        let b0 = std::sync::Arc::new(image::DynamicImage::new_rgba8(4, 4));
        let developed = EditRecipe { contrast: 7.0, ..Default::default() };
        app.variants = vec![Variant {
            id: ORIGINAL_VARIANT_ID.into(),
            name: None,
            kind: VariantKind::Original,
            recipe: developed.clone(),
            base: Some(b0.clone()),
            origin: None,
            thumb: None,
        }];
        app.active = 0;
        app.recipe = developed.clone();
        app.base_preview = Some(b0.clone());
        app.reset_history();
        let out = std::path::PathBuf::from("out/_fill_new_card_test.fill.png");
        let epoch = app.gen_epoch;
        app.on_retouched(
            &ctx,
            Lang::En,
            epoch,
            Ok((
                image::DynamicImage::new_rgba8(4, 4),
                RetouchNote::Filled(out.clone()),
                out.clone(),
                RetouchKind::NewGenerated,
            )),
        );
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Generated],
            "{}",
            app.status
        );
        assert_eq!(app.active, 1, "the new card is under the canvas");
        assert_eq!(app.variants[1].origin.as_deref(), Some(out.as_path()));
        assert!(app.variants[1].recipe.is_noop(), "a ✨ card's look lives in its pixels");
        assert!(app.recipe.is_noop(), "…and the canvas recipe followed the switch");
        assert_eq!(app.variants[0].recipe, developed, "the filled card keeps its develop");
        assert!(
            app.variants[0].base.as_ref().is_some_and(|b| std::sync::Arc::ptr_eq(b, &b0)),
            "…and its base"
        );
        assert_eq!(app.variants[0].origin, None, "…and its pixel source");
        assert_eq!(app.negative_origin(), None, "the negative is untouched by a fill");
        assert!(app.status.contains("new"), "{}", app.status);
        // The verb's own choices are pinned in the source (the house pattern
        // for a worker whose model call cannot run offline): the fill develops
        // the card's picture for the model and lands as a NEW card, never in
        // place. MUTATION: `NewGenerated` → `InPlace` in start_fill, or the
        // develop replaced by the neutral base, and this names it.
        let fill = include_str!("../panels/retouch.rs");
        let body = &fill[fill.find("pub(crate) fn start_fill(").expect("start_fill moved")..];
        let body = &body[..body.find("pub(crate) fn start_heal(").expect("start_heal moved")];
        assert!(
            body.contains("autoshade::generative::retouch_onto("),
            "the fill no longer hands the library its own base"
        );
        assert!(
            body.contains("developed_card_pixels(&path, &recipe, full_res)"),
            "the fill no longer develops the card's picture for the model"
        );
        assert!(body.contains("RetouchKind::NewGenerated))"), "the fill no longer lands as a new card");
        assert!(!body.contains("RetouchKind::InPlace"), "the fill went back to an in-place landing");
    }

    /// Both worker arms land through the same new-card path; the source
    /// raster/recipe survive, and the new card can be adjusted again.
    #[test]
    fn an_adjust_lands_as_a_new_generated_card_and_leaves_the_source_card_alone() {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-adjust-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("library")).unwrap();
        for lang in [Lang::En, Lang::Zh] {
            for region in [false, true] {
                let ctx = egui::Context::default();
                let src = dir.join("library").join(format!("adjust-{lang:?}-{region}.png"));
                let base = Arc::new(image::DynamicImage::new_rgb8(4, 4));
                let origin = dir.join(format!("generated-{lang:?}-{region}.png"));
                let developed = EditRecipe {
                    contrast: 7.0, straighten_deg: 2.0,
                    crop: Some(autoshade::recipe::Crop { top: 0.1, left: 0.1, bottom: 0.9, right: 0.9 }),
                    ..Default::default()
                };
                let mut app = AutoShadeApp {
                    src_path: Some(src.clone()),
                    variants: vec![Variant {
                        id: new_variant_id(), name: None, kind: VariantKind::Generated,
                        recipe: EditRecipe::default(), base: Some(base.clone()),
                        origin: Some(origin.clone()), thumb: None,
                    }],
                    base_preview: Some(base.clone()),
                    ..Default::default()
                };
                app.reset_history();
                let out = unique_out(&src, "adjust").unwrap();
                assert_eq!(out.file_name().unwrap().to_string_lossy(),
                    format!("adjust-{lang:?}-{region}.adjust.png"));
                let second = unique_out(&src, "adjust").unwrap();
                assert_eq!(second.file_name().unwrap().to_string_lossy(),
                    format!("adjust-{lang:?}-{region}.adjust-2.png"));
                release_empty_claim(&second);
                let source_recipe = app.variants[0].recipe.clone();
                app.on_retouched(&ctx, lang, app.gen_epoch, Ok((
                    image::DynamicImage::new_rgb8(4, 4),
                    RetouchNote::Adjusted { out: out.clone(), region, divergence: (!region).then_some(0.12) },
                    out.clone(), RetouchKind::NewGenerated,
                )));
                assert_eq!(strip_kinds(&app), vec![VariantKind::Generated, VariantKind::Generated]);
                assert_eq!(app.active, 1, "the new adjusted card is active");
                assert_eq!(app.variants[0].origin.as_ref(), Some(&origin), "the source keeps its pixels");
                assert_eq!(app.variants[0].recipe, source_recipe, "the source keeps its recipe");
                assert!(Arc::ptr_eq(app.variants[0].base.as_ref().unwrap(), &base));
                assert_eq!(app.variants[1].origin.as_ref(), Some(&out));
                assert_eq!(app.active_source_path().as_ref(), Some(&out), "the next adjust follows this master");
                assert!(app.recipe.is_noop() && app.variants[1].recipe.is_noop());
                assert_eq!(app.fit_target().as_ref(), Some(&out), "reverse-fit reads the new generated card");
                let note = match (lang, region) {
                    (Lang::En, true) => "adjusted → new ✨ card (painted area only)",
                    (Lang::En, false) => "adjusted → new ✨ card (whole image, divergence D = 0.12)",
                    (Lang::Zh, true) => "已调整 → 新 ✨ 卡（只改涂抹区域）",
                    (Lang::Zh, false) => "已调整 → 新 ✨ 卡（整张图，结构偏离 D = 0.12）",
                };
                assert!(app.status.starts_with(note), "landing language: {}", app.status);
                // An edited AI card's non-neutral develop is just as immutable
                // under another adjust; crop/straighten do not reach the result.
                app.variants[1].kind = VariantKind::Edited;
                app.variants[1].recipe = developed.clone();
                app.recipe = developed.clone();
                app.on_retouched(&ctx, lang, app.gen_epoch, Ok((
                    image::DynamicImage::new_rgb8(4, 4),
                    RetouchNote::Adjusted { out: second.clone(), region, divergence: (!region).then_some(0.0) },
                    second.clone(), RetouchKind::NewGenerated,
                )));
                assert_eq!(app.active, 2);
                assert_eq!(app.variants[1].recipe, developed);
                assert_eq!(app.variants[1].origin.as_ref(), Some(&out));
                assert!(app.variants[2].recipe.is_noop());
                release_empty_claim(&out);
            }
        }
        let worker = include_str!("../panels/retouch.rs");
        let start = worker.find("pub(crate) fn start_adjust(").unwrap();
        let body = &worker[start..worker[start..].find("pub(crate) fn start_heal(").unwrap() + start];
        assert!(body.contains("self.active_source_path()"), "the card supplies its own raster");
        assert!(body.contains("self.recipe.clone()"), "the live recipe is captured at the click");
        assert!(body.contains("developed_card_pixels(&path, &recipe, false)"));
        let split = body.find("if let Some(mask_png) = mask_png").unwrap();
        let arms = &body[split..];
        let whole = arms.find("} else {").unwrap();
        assert!(arms[..whole].contains("autoshade::generative::retouch_onto("), "strokes take the fill path");
        assert!(arms[whole..].contains("autoshade::generative::adjust_onto("), "no strokes take the whole-image path");
        assert!(body.contains("RetouchKind::NewGenerated))"));
        assert!(!body.contains("RetouchKind::InPlace"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn draw_adjust_panel(app: &mut AutoShadeApp) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = draw_adjust_frame(app, &ctx);
        }
        texts
    }

    fn draw_adjust_frame(app: &mut AutoShadeApp, ctx: &egui::Context) -> Vec<String> {
        let out = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 20_000.0))),
            ..Default::default()
        }, |ctx| {
            ctx.memory_mut(|m| m.set_everything_is_visible(true));
            egui::SidePanel::left("adjust-controls").default_width(320.0).show(ctx, |ui| app.ai_panel(ui));
        });
        drawn_texts(&out.shapes)
    }

    #[test]
    fn the_adjust_verb_is_disabled_off_ai_pixels_and_with_no_prompt_and_no_strokes() {
        // The fold's own area is the live canvas here.
        let mut app = AutoShadeApp { paint_owner: BrushOwner::Adjust, ..Default::default() };
        // Off AI pixels, empty AI input, and useful AI input are the three
        // states. Both Generated and Edited count; busy still disables them.
        for kind in [VariantKind::Original, VariantKind::Fitted, VariantKind::Denoised,
            VariantKind::Stacked, VariantKind::Generated, VariantKind::Edited]
        {
            app.variants = vec![Variant {
                id: new_variant_id(), name: None, kind, recipe: EditRecipe::default(),
                base: None, origin: None, thumb: None,
            }];
            for (prompt, strokes, busy) in [("bluer sky", false, false), (" \t ", false, false),
                ("", true, false), ("bluer sky", true, true)]
            {
                app.adjust_prompt = prompt.into();
                app.busy = busy;
                app.mask_paint = strokes.then(|| image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 64, 64, 160])));
                app.paint_mask_changed(None);
                draw_adjust_panel(&mut app);
                let want = !busy && kind.on_ai_pixels() && (strokes || !prompt.trim().is_empty());
                assert_eq!(app.adjust_btn_enabled, Some(want),
                    "the adjust verb must stay disabled off AI pixels or without prompt/strokes (kind={kind:?}, prompt={prompt:?}, strokes={strokes}, busy={busy})");
            }
        }
        // The starter uses the same rule before claiming output or spawning.
        app.busy = false;
        app.adjust_prompt.clear();
        app.mask_paint = None;
        app.paint_mask_changed(Some(false));
        app.src_path = Some(PathBuf::from("adjust-guard.png"));
        app.start_adjust();
        assert!(!app.busy, "blank whole-image adjustment never reaches a worker");
    }

    #[test]
    fn the_adjust_fold_says_whether_it_reads_the_painted_area() {
        for lang in [Lang::En, Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.variants = vec![Variant {
                id: new_variant_id(), name: None, kind: VariantKind::Generated,
                recipe: EditRecipe::default(), base: None, origin: None, thumb: None,
            }];
            app.paint_owner = BrushOwner::Adjust; // the fold's own area is the live canvas here
            // The same >10 alpha predicate as export: a faint/erased mask
            // is whole-image mode, even though the mask buffer exists.
            for alpha in [0, 10, 11, 160] {
                app.mask_paint = Some(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 64, 64, alpha])));
                app.paint_mask_changed(None);
                let strokes = alpha > 10;
                assert_eq!(app.has_painted_mask(), strokes);
                assert_eq!(app.export_mask_png().is_some(), strokes);
                let text = draw_adjust_panel(&mut app);
                let painted = tr(lang, "painted area only");
                let whole = tr(lang, "whole image (paint an area to limit it)");
                let (yes, no) = if strokes { (painted, whole) } else { (whole, painted) };
                assert!(text.iter().any(|t| t == yes), "status missing: {yes}");
                assert!(!text.iter().any(|t| t == no), "stale status: {no}");
            }
            app.variants[0].kind = VariantKind::Original;
            let text = draw_adjust_panel(&mut app);
            assert!(text.iter().any(|t| t == tr(lang, "select a ✨ AI generated card (or its ✎ edit) first")));
            assert!(!text.iter().any(|t| t == tr(lang, "painted area only")));
        }
        let prefs = Prefs { adjust_quality: 2, ..Prefs::default() };
        let json = serde_json::to_string(&prefs).unwrap();
        let decoded: Prefs = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.adjust_quality, 2);
        let older = json.replace(r#""adjust_quality":2,"#, "");
        assert!(!older.contains("adjust_quality"));
        let old: Prefs = serde_json::from_str(&older).unwrap();
        assert_eq!(old.adjust_quality, 0, "old prefs keep high as the default");
        assert!(!json.contains("adjust_prompt"), "prompt is transient like Reimagine's");
        assert!(include_str!("../actions.rs").contains("app.adjust_quality = prefs.adjust_quality.min(2)"));
        assert!(include_str!("../app.rs").contains("adjust_quality: self.adjust_quality"));
    }

    /// Real pointer events cover both stamp branches. The counter measures
    /// buffer walks, not elapsed time, so a small fast machine cannot hide a
    /// per-frame scan of an empty preview (the normal state of the fold).
    #[test]
    fn the_adjust_fold_scans_only_after_a_paint_mask_change() {
        fn stroke(app: &mut AutoShadeApp, drag: bool) {
            let ctx = egui::Context::default();
            let frame = |app: &mut AutoShadeApp, events: Vec<egui::Event>| {
                let mut painted_rect = egui::Rect::NOTHING;
                let _ = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 850.0))),
                    events, ..Default::default()
                }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(1024.0, 768.0), egui::Sense::click_and_drag());
                        painted_rect = rect;
                        app.handle_paint(&resp, ViewXform {
                            rect, uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        });
                    });
                });
                painted_rect
            };
            let rect = frame(app, vec![]);
            let start = rect.center();
            let end = start + egui::vec2(if drag { 48.0 } else { 0.0 }, 0.0);
            let button = |pos, pressed| egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            };
            frame(app, vec![egui::Event::PointerMoved(start), button(start, true)]);
            if drag {
                frame(app, vec![egui::Event::PointerMoved(end)]);
            }
            frame(app, vec![button(end, false)]);
        }
        fn five_frames(app: &mut AutoShadeApp, ctx: &egui::Context, painted: bool, why: &str) {
            let mut text = Vec::new();
            for _ in 0..5 {
                // A texture upload consumes this flag; it must not consume
                // or invalidate the independent paint-presence memo.
                app.mask_dirty = false;
                text = draw_adjust_frame(app, ctx);
            }
            let region = "painted area only";
            let whole = "whole image (paint an area to limit it)";
            let (yes, no) = if painted { (region, whole) } else { (whole, region) };
            assert!(text.iter().any(|t| t == yes), "{why}: expected {yes}");
            assert!(!text.iter().any(|t| t == no), "{why}: stale {no}");
            assert_eq!(app.has_painted_mask(), painted, "{why}");
        }
        for drag in [false, true] {
            let base = Arc::new(image::DynamicImage::new_rgb8(1024, 768));
            let mut app = AutoShadeApp {
                base_preview: Some(base.clone()),
                variants: (0..2).map(|_| Variant {
                    id: new_variant_id(), name: None, kind: VariantKind::Generated,
                    recipe: EditRecipe::default(), base: Some(base.clone()), origin: None, thumb: None,
                }).collect(),
                ..Default::default()
            };
            // The fold's OWN brush on a canvas sized to the plate: the Adjust
            // area is the live canvas (2026-09-27, `BrushOwner`).
            app.rebind_paint_canvas(1024, 768);
            app.arm_brush(BrushOwner::Adjust);
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            five_frames(&mut app, &ctx, false, "an empty brush reads the whole image");
            assert!(app.mask_presence_scans.get() <= 1, "five empty frames may scan only once");
            let empty_scans = app.mask_presence_scans.get();

            stroke(&mut app, drag);
            assert!(app.mask_paint.as_ref().unwrap().pixels().any(|p| p[3] > 10), "the real brush added paint");
            five_frames(&mut app, &ctx, true, "a brush stroke limits the adjust to the painted area");
            assert_eq!(app.mask_presence_scans.get(), empty_scans, "adding paint needs no presence scan");
            assert!(app.export_area_png(BrushOwner::Adjust).is_some());

            // 「Clear area」 is the fold's eraser (the retouch brushes have no
            // erase mode): the canvas is known blank, so nothing scans.
            app.clear_area(BrushOwner::Adjust);
            assert!(app.mask_paint.as_ref().unwrap().pixels().all(|p| p[3] <= 10), "clearing removed every stroke");
            five_frames(&mut app, &ctx, false, "clearing the area must return the adjust fold to whole image");
            assert_eq!(app.mask_presence_scans.get(), empty_scans, "a cleared area needs no scan");
            assert!(app.export_area_png(BrushOwner::Adjust).is_none());

            stroke(&mut app, drag);
            assert!(app.has_painted_mask());
            let before_switch = app.mask_presence_scans.get();
            app.switch_variant(1, &ctx);
            assert_eq!(app.active, 1);
            five_frames(&mut app, &ctx, false, "a fresh card starts with an empty brush");
            assert_eq!(app.mask_presence_scans.get(), before_switch, "a fresh card needs zero scans");

            // The other plate-replacement door is also known empty.
            app.rebind_paint_canvas(1024, 768);
            five_frames(&mut app, &ctx, false, "a rebound canvas starts empty");
            assert_eq!(app.mask_presence_scans.get(), before_switch, "rebinding needs zero scans");
        }
    }

    /// Census the entire GUI tree, including the raw mutable borrows. A new
    /// replacement/write site must declare its notification here; the sole
    /// texture-dirty setter is itself the presence-invalidation owner.
    #[test]
    fn every_paint_buffer_mutation_uses_the_presence_notification_door() {
        fn walk_rs(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("GUI source dir listable") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    walk_rs(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
        const CENSUS: [(&str, usize); 9] = [
            ("actions.rs::load_active", 1),
            ("actions.rs::rebind_paint_canvas", 1),
            ("masks.rs::paint_imported_removals", 2),
            ("masks.rs::paint_mask_changed", 1),
            ("masks.rs::clear_mask", 1),
            ("masks.rs::select_brush_owner", 1),
            ("masks.rs::start_mask_brush", 1),
            ("masks.rs::unstash_owner_area", 1),
            ("panels/retouch.rs::handle_paint", 1),
        ];
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bin/gui");
        let mut sources = Vec::new();
        walk_rs(&root, &mut sources);
        assert!(sources.len() >= 6, "expected the split GUI module tree");
        let replace = concat!("self.", "mask_paint = Some(");
        let dirty = concat!("self.", "mask_dirty = true");
        let mutations = [replace, dirty, concat!("self.", "mask_paint.as_mut()"), concat!("&mut self.", "mask_paint")];
        let mut found: std::collections::BTreeMap<String, usize> = Default::default();
        for path in sources {
            let text = std::fs::read_to_string(&path).expect("GUI source readable");
            let name = path.strip_prefix(&root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let lines: Vec<_> = text.lines().collect();
            let mut function = "";
            let mut start = 0;
            for (i, line) in lines.iter().enumerate() {
                if let Some(head) = line.strip_prefix("    pub(crate) fn ").or_else(|| line.strip_prefix("    fn ")) {
                    function = head.split('(').next().unwrap();
                    start = i;
                }
                if line.trim_start().starts_with("//") || !mutations.iter().any(|m| line.contains(m)) {
                    continue;
                }
                let site = format!("{name}::{function}");
                *found.entry(site.clone()).or_default() += 1;
                if line.contains(dirty) {
                    assert_eq!(site, "masks.rs::paint_mask_changed", "only the notification door may mark the overlay dirty");
                    continue;
                }
                let end = lines[start..].iter().position(|l| *l == "    }").expect("method end") + start;
                let body = lines[start..=end].join("\n");
                assert!(body.contains("self.paint_mask_changed("), "{site} bypasses paint-presence invalidation");
                if line.contains(replace) {
                    assert!(lines[i + 1].trim_start().starts_with("self.paint_mask_changed("),
                        "{site}: a replacement must immediately notify paint presence");
                }
            }
        }
        let expected = CENSUS.into_iter().map(|(site, n)| (site.to_string(), n)).collect();
        assert_eq!(found, expected, "paint-buffer mutation sites changed: wire the notification door and update the census");
        assert_eq!(found.values().sum::<usize>(), 10, "six replacements, three mutable borrows, one dirty setter");
    }

    /// v1.5.0: the canvas preview reaches its frame through the ENGINE's tail
    /// (`render::frame_and_finish` — lens geometry → straighten → crop → the
    /// post-crop vignette and grain), with `CropPolicy::Keep` so the frame
    /// stays whole for slider feedback while the finishing pass is positioned
    /// on the crop rectangle. Pinned in the source, the house pattern for a
    /// choice made inside a worker no offline test drives (the web preview's
    /// half of this pin lives in `render::finish`'s tests).
    ///
    /// MUTATION: `CropPolicy::Cut` here, or the old geometry-only chain back,
    /// and this names it — a canvas that cropped would show a different
    /// photograph from the one the sliders are moving, and one that skipped
    /// the tail would show no vignette until export.
    #[test]
    fn the_canvas_preview_finishes_through_the_engines_own_tail() {
        let src = include_str!("../util.rs");
        let head = src.find("pub(crate) fn build_preview(").expect("build_preview moved");
        let body = &src[head..head + 2000];
        assert!(
            body.contains("autoshade::render::frame_and_finish("),
            "the canvas preview no longer runs the engine's tail"
        );
        assert!(
            body.contains("autoshade::render::CropPolicy::Keep"),
            "the canvas must keep the whole frame and only POSITION the finish"
        );
        assert!(
            !body.contains("autoshade::render::apply_lens_geometry("),
            "a second copy of the geometry chain is exactly what the tail replaced"
        );
    }

    /// v1.5.0: every surface that SHOWS a photo's develop, or must see the
    /// canvas's own pixels, develops at the source's film edge
    /// (`render::FilmScale`): the canvas preview, the web preview, both
    /// Range-mask reference builds (a range is judged on the pixel as the
    /// canvas's Detail passes left it), the Point Color eyedropper's sample (a
    /// swatch is keyed to the pixel the engine will test) and the fill's
    /// picture of the card (the look the model is asked to keep). The
    /// analysis surfaces keep their
    /// raster as the film, which is why this is a named list and not a blanket
    /// rule. Pinned in the source, the house pattern for a choice made inside a
    /// UI-thread method, a worker or a request handler no offline test drives.
    ///
    /// MUTATION: the edge replaced by `None` in any of the six develops (the
    /// Detail passes then run at raster scale there, visibly stronger than the
    /// export), and this names the function.
    #[test]
    fn the_surfaces_that_show_canvas_pixels_develop_at_its_film_edge() {
        fn develops_at_film(src: &str, head: &str, edge: &str, develop: &str) {
            let body = &src[src.find(head).unwrap_or_else(|| panic!("{head} moved"))..];
            // The body ends at the next item, a method or a free function.
            let rest = &body[head.len()..];
            let next = ["\n    pub(crate) fn ", "\npub(crate) fn ", "\npub fn ", "\nfn "];
            let end = next.iter().filter_map(|m| rest.find(m)).min().map_or(body.len(), |i| i + head.len());
            let body = &body[..end];
            let call = &body[body.find(develop).unwrap_or_else(|| panic!("{head}: no {develop}"))..];
            // The ARGUMENTS, never the callee: `develop_preview_film(` spells
            // the word itself, so a whole-call match stayed green with `None`
            // in the film slot (the v1.5.0 falsification, M13).
            let args = &call[develop.len()..call.find(';').unwrap_or(call.len())];
            assert!(body.contains(edge), "{head} no longer reads the source's film edge");
            assert!(args.contains("film"), "{head} develops at raster scale: {args}");
        }
        let canvas = include_str!("../canvas.rs");
        let framed = "develop_preview_framed(";
        develops_at_film(canvas, "pub(crate) fn refresh_mask_overlay(", "self.film_short_edge()", framed);
        develops_at_film(canvas, "pub(crate) fn handle_range_pick(", "self.film_short_edge()", framed);
        develops_at_film(canvas, "pub(crate) fn handle_point_color_pick(", "self.film_short_edge()", framed);
        let fill = include_str!("../panels/retouch.rs");
        develops_at_film(fill, "fn developed_card_pixels(", "autoshade::decode::film_short_edge(path)", framed);
        let workers = include_str!("../workers.rs");
        develops_at_film(workers, "pub(crate) fn start_redevelop(", "self.film_short_edge()", "build_preview(");
        let util = include_str!("../util.rs");
        develops_at_film(util, "pub(crate) fn build_preview(", "film_short_edge", "develop_preview_film(");
        let serve = include_str!("../../../serve.rs");
        develops_at_film(serve, "fn api_develop(", "decode::film_short_edge(&src)", "develop_preview_film(");
    }

    /// The immutability rule for PIXELS: an in-place retouch (heal / clone /
    /// denoise) on the pristine ✨ card bakes into a new ✎ card; the ✨ card
    /// keeps its raster.
    #[test]
    fn a_retouch_in_place_on_the_pristine_ai_card_bakes_into_an_edited_card() {
        let (mut app, ctx, dir, _src, master, ai_px, _scrub) =
            ai_card_fixture("retouch", EditRecipe::default());
        let healed = dir.join("healed.png");
        std::fs::write(&healed, b"png").unwrap();
        let epoch = app.gen_epoch;
        app.on_retouched(
            &ctx,
            Lang::En,
            epoch,
            Ok((
                image::DynamicImage::ImageRgba8(image::RgbaImage::new(6, 4)),
                RetouchNote::Healed {
                    n: 1,
                    out: healed.clone(),
                    ai_prose: String::new(),
                    notes: Vec::new(),
                },
                healed.clone(),
                RetouchKind::InPlace,
            )),
        );
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Generated, VariantKind::Edited],
            "{}",
            app.status
        );
        assert_eq!(app.active, 2);
        assert_eq!(
            app.variants[2].origin.as_deref(),
            Some(healed.as_path()),
            "the retouch baked into the ✎ card"
        );
        assert!(app.variants[2].base.as_ref().is_some_and(|b| !Arc::ptr_eq(b, &ai_px)));
        assert_eq!(
            app.variants[1].origin.as_deref(),
            Some(master.as_path()),
            "the ✨ card keeps its raster"
        );
        assert!(app.variants[1].base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &ai_px)));
    }

    /// Every build through v1.3.2 stored an edit made on the ✨ card AS the
    /// ✨ card: recipe.json holds the edits, the record says the active card
    /// is "generated". The door splits it — once, as unsaved work — and a
    /// strip saved split reopens as it is, silently.
    #[test]
    fn a_strip_saved_with_edits_on_the_ai_card_is_split_at_the_door() {
        let (mut app, ctx, _dir, src, master, ai_px, _scrub) =
            ai_card_fixture("split", EditRecipe { contrast: 7.0, ..Default::default() });
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Generated, VariantKind::Edited],
            "{}",
            app.status
        );
        assert_eq!(app.active, 2, "the canvas follows the edits");
        assert_eq!(app.recipe.contrast, 7.0);
        assert!(app.recipe.base_curve.is_empty(), "calibration stripped on AI pixels");
        assert_eq!(app.variants[2].recipe.contrast, 7.0);
        assert!(app.variants[2].base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &ai_px)));
        assert_eq!(app.variants[2].origin.as_deref(), Some(master.as_path()));
        assert!(app.variants[1].recipe.is_noop(), "the ✨ card is pristine");
        assert_eq!(app.variants[1].id, "gen-1", "…and keeps the record's identity");
        assert_eq!(app.variants[1].name.as_deref(), Some("sky"), "…and its name");
        assert!(app.unsaved_marker_dirty(), "the split is unsaved work until Ctrl+S");
        assert_eq!(
            app.toasts.iter().filter(|t| t.text.contains("separate cards")).count(),
            1,
            "said once at the door"
        );
        assert_eq!(VariantKind::from_store_str("edited"), Some(VariantKind::Edited));
        assert_eq!(VariantKind::Edited.store_str(), "edited");
        let rec = app.current_strip_record().expect("a three-card strip has a record");
        assert_eq!(rec.active_kind, "edited");

        // Ctrl+S persists the split; the same door then reopens it as it is.
        app.save_xmp();
        assert!(!app.unsaved_marker_dirty(), "{}", app.status);
        assert_eq!(saved_strip_of(&src).active_kind, "edited");
        let mut again = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        open_onto_ai_card(&mut again, &ctx, &ai_px, &master);
        assert_eq!(
            strip_kinds(&again),
            vec![VariantKind::Original, VariantKind::Generated, VariantKind::Edited],
            "{}",
            again.status
        );
        assert_eq!(again.active, 2);
        assert_eq!(again.recipe.contrast, 7.0);
        assert!(again.variants[1].recipe.is_noop());
        assert!(!again.unsaved_marker_dirty(), "a split strip reopens clean: {}", again.status);
        assert!(
            !again.toasts.iter().any(|t| t.text.contains("separate cards")),
            "…and silently"
        );
    }

    /// Ctrl+S on a card over AI pixels saves that card's develop — no
    /// refusal (until 2026-09-13 a generated card was refused outright) —
    /// with the RAW's calibration kept on disk, the canvas stripped, and no
    /// Lightroom projection: the member CLEARS a standing one.
    #[test]
    fn ctrl_s_on_an_ai_pixel_card_saves_its_develop_and_retires_the_projection() {
        let (mut app, _ctx, _dir, src, master, _ai_px, _scrub) =
            ai_card_fixture("save", EditRecipe::default());
        let xp = autoshade::pipeline::xmp_target(&src);
        std::fs::write(
            &xp,
            autoshade::xmp::recipe_to_xmp(&EditRecipe { saturation: 90.0, ..Default::default() }),
        )
        .unwrap();
        // The pristine ✨ card saves as itself.
        app.save_xmp();
        assert!(!app.status.contains("Reverse-fit"), "no refusal: {}", app.status);
        assert!(app.status.contains("AI-generated pixels"), "{}", app.status);
        assert!(!xp.exists(), "the stale projection is retired by the commit");
        assert!(!app.unsaved_marker_dirty(), "{}", app.status);
        assert_eq!(saved_strip_of(&src).active_kind, "generated");

        // An edit → ✎; Ctrl+S saves THAT card's develop.
        app.recipe.contrast = 7.0;
        app.save_xmp(); // the boundary fork runs inside
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Generated, VariantKind::Edited],
            "{}",
            app.status
        );
        assert!(app.status.starts_with("recipe saved"), "{}", app.status);
        let disk: EditRecipe = serde_json::from_str(
            &std::fs::read_to_string(autoshade::store::recipe_target(&src)).unwrap(),
        )
        .unwrap();
        assert_eq!(disk.contrast, 7.0);
        assert_eq!(
            disk.base_curve,
            vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]],
            "the disk form keeps the RAW's saved calibration"
        );
        assert_eq!(disk.as_shot_k, Some(5653.0));
        assert!(app.recipe.base_curve.is_empty(), "the canvas stays stripped");
        let (origin, generated) = autoshade::store::read_pixel_source(&src).unwrap();
        assert!(generated);
        assert_eq!(origin, master);
        let rec = saved_strip_of(&src);
        assert_eq!(rec.active_kind, "edited");
        assert_eq!(
            rec.others.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            vec!["original", "generated"]
        );
        assert!(!xp.exists(), "still no projection over AI pixels");
        assert!(!app.unsaved_marker_dirty(), "{}", app.status);
    }

    /// The reverse-fit reads the ✨ card's raster, never an ✎ card: the fit
    /// solves pixels, and the edits are sliders the user already has.
    #[test]
    fn fit_target_is_the_pristine_ai_cards_raster_never_the_edited_cards() {
        let (mut app, ctx, _dir, _src, master, _ai_px, _scrub) =
            ai_card_fixture("fit", EditRecipe::default());
        assert_eq!(
            app.fit_target().as_deref(),
            Some(master.as_path()),
            "premise: the ✨ card's raster is the default target"
        );
        app.recipe.contrast = 7.0;
        assert!(app.fork_edited_card());
        assert_eq!(app.fit_target(), None, "an ✎ card is not a fit target");
        app.switch_variant(1, &ctx);
        assert_eq!(app.fit_target().as_deref(), Some(master.as_path()));
    }

    /// 2026-09-13 (the user's own store: ▣ `origin = …denoise.png`, the ◭
    /// card active with `origin = None`, no `pixels.json`): the ◭ card a fit
    /// lands as hangs off the NEGATIVE's in-place master — the pixels the ▣
    /// card develops and the fit was solved on — sharing the ▣ card's decoded
    /// base, and the ● pixel mirror follows the link the worker persisted.
    /// Without a master the fit keeps landing on the loaded photo, origin-free.
    #[test]
    fn a_reverse_fit_lands_on_the_negatives_master() {
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp::default();
        let master = std::path::PathBuf::from("out/_negative_master_test.denoise.png");
        let clean = std::sync::Arc::new(image::DynamicImage::new_rgba8(4, 4));
        let ai = std::sync::Arc::new(image::DynamicImage::new_rgba8(4, 4));
        app.variants = vec![
            Variant {
                id: ORIGINAL_VARIANT_ID.into(),
                name: None,
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: Some(clean.clone()),
                origin: Some(master.clone()),
                thumb: None,
            },
            Variant {
                id: "gen".into(),
                name: None,
                kind: VariantKind::Generated,
                recipe: EditRecipe::default(),
                base: Some(ai.clone()),
                origin: Some(std::path::PathBuf::from("out/_negative_master_test.reimagine.png")),
                thumb: None,
            },
        ];
        app.active = 1;
        app.base_preview = Some(ai.clone());
        assert_eq!(
            app.negative_origin().as_deref(),
            Some(master.as_path()),
            "the ▣ card's master is the negative even while the ✨ card is active"
        );
        let outcome = |negative: Option<std::path::PathBuf>| FitOutcome {
            recipe: EditRecipe { contrast: 9.0, ..Default::default() },
            err_before: 0.3,
            err_after: 0.1,
            rationale_notes: Vec::new(),
            status: Vec::new(),
            persisted: true,
            negative,
        };
        app.on_fitted(&ctx, Lang::En, Box::new(Ok(outcome(Some(master.clone())))));
        let fitted = &app.variants[app.active];
        assert_eq!(fitted.kind, VariantKind::Fitted);
        assert_eq!(
            fitted.origin.as_deref(),
            Some(master.as_path()),
            "the ◭ card hangs off the negative's master"
        );
        assert!(
            fitted.base.as_ref().is_some_and(|b| std::sync::Arc::ptr_eq(b, &clean)),
            "…and shares the ▣ card's decoded pixels"
        );
        assert_eq!(
            app.pixels_on_disk.as_deref(),
            Some(master.as_path()),
            "the ● pixel mirror follows the persisted link"
        );
        assert_eq!(app.recipe.contrast, 9.0, "the fit's develop is live on the new card");

        // No master on the negative: the fit lands on the loaded photo, as
        // it always did.
        app.variants[0].origin = None;
        app.variants[0].base = None;
        app.switch_variant(1, &ctx);
        assert_eq!(app.negative_origin(), None);
        app.on_fitted(&ctx, Lang::En, Box::new(Ok(outcome(None))));
        let fitted = &app.variants[app.active];
        assert_eq!(fitted.kind, VariantKind::Fitted);
        assert_eq!(fitted.origin, None);
        assert_eq!(app.pixels_on_disk, None);
    }

    /// 2026-09-15 (user decision): an AI denoise is NOT an in-place retouch
    /// any more — it lands as a NEW ◈ Denoised negative card whose origin is
    /// the denoised master and whose recipe is a copy of the card it was run
    /// from, auto-switched to. The ▣ card keeps its pixels, its base and its
    /// `origin = None` (the old in-place landing redefined the ▣ card and
    /// left the untouched negative unreachable). The negative then IS the ◈
    /// card's master (`negative_origin`), and `negative_path` — what the
    /// reimagine develops from — follows it.
    #[test]
    fn an_ai_denoise_lands_as_a_new_denoised_card_and_leaves_the_source_and_negative_alone() {
        let ctx = egui::Context::default();
        let src = std::path::PathBuf::from("_negative_follow_test.ARW");
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        let b0 = std::sync::Arc::new(image::DynamicImage::new_rgba8(4, 4));
        let developed = EditRecipe { contrast: 7.0, ..Default::default() };
        app.variants = vec![Variant {
            id: ORIGINAL_VARIANT_ID.into(),
            name: None,
            kind: VariantKind::Original,
            recipe: developed.clone(),
            base: Some(b0.clone()),
            origin: None,
            thumb: None,
        }];
        app.active = 0;
        app.recipe = developed.clone();
        app.base_preview = Some(b0.clone());
        app.reset_history();
        assert_eq!(app.negative_origin(), None);
        assert_eq!(app.negative_path().as_deref(), Some(src.as_path()), "no master: the loaded file");
        let out = std::path::PathBuf::from("out/_negative_follow_test.denoise.png");
        let epoch = app.gen_epoch;
        app.on_retouched(
            &ctx,
            Lang::En,
            epoch,
            Ok((
                image::DynamicImage::new_rgba8(4, 4),
                RetouchNote::Denoised { out: out.clone(), on_mosaic: true },
                out.clone(),
                RetouchKind::NewDenoised,
            )),
        );
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Denoised],
            "{}",
            app.status
        );
        assert_eq!(app.active, 1, "the new ◈ card is under the canvas");
        assert_eq!(app.variants[1].origin.as_deref(), Some(out.as_path()), "…hanging off the denoised master");
        assert_eq!(app.variants[1].recipe, developed, "…carrying the source card's develop");
        assert!(app.variants[1].kind.is_source_based(), "a ◈ card is parametric over its master");
        assert!(!app.variants[1].kind.on_ai_pixels(), "…and carries no AI-pixel caveats");
        assert_eq!(app.variants[0].recipe, developed, "the ▣ card keeps its develop");
        assert!(
            app.variants[0].base.as_ref().is_some_and(|b| std::sync::Arc::ptr_eq(b, &b0)),
            "…and its base"
        );
        assert_eq!(app.variants[0].origin, None, "…and its untouched pixel source");
        assert_eq!(app.negative_origin().as_deref(), Some(out.as_path()), "the negative is now the ◈ master");
        assert_eq!(app.negative_path().as_deref(), Some(out.as_path()), "the reimagine input follows");
        assert_eq!(
            app.active_source_path().as_deref(),
            Some(out.as_path()),
            "and the ◈ card's own source is the same file — one negative"
        );
        assert!(app.status.contains("new ◈ card"), "{}", app.status);
        assert!(app.status.contains("sensor mosaic"), "{}", app.status);
        // The verb's own choices are pinned in the source (the house pattern
        // for a worker whose sidecar cannot run offline): the denoise lands as
        // a NEW card, never in place, and always on the full frame — the
        // ≤2048 px working-copy tier is gone with the checkbox. MUTATION:
        // `NewDenoised` → `InPlace` in start_ai_denoise, and this names it.
        let now = include_str!("../panels/retouch.rs");
        let body = &now[now.find("pub(crate) fn start_ai_denoise(").expect("start_ai_denoise moved")..];
        let body = &body[..body.find("pub(crate) fn start_clone(").expect("start_clone moved")];
        assert!(body.contains("RetouchKind::NewDenoised))"), "the denoise no longer lands as a new card");
        assert!(!body.contains("RetouchKind::InPlace"), "the denoise went back to an in-place landing");
        assert!(body.contains("denoise_active(&opts, &path, &out)"), "the denoise no longer runs the full frame");
        assert!(!body.contains("2048"), "the ≤2048 px working-copy tier came back");
        assert!(!body.contains("denoise_fullres"), "the retired Full-res checkbox came back");
        // The worker halves run behind a segmentation call, so they are
        // pinned on the source (the config.rs literal-pin pattern): the fit
        // reads the negative once at the click and the persist links it.
        let fit = include_str!("../actions.rs");
        assert!(fit.contains("let negative = self.fit_negative();"));
        assert!(
            fit.contains("(Some(p), Some(master)) => ("),
            "the solve's source frame is the master when the negative carries one"
        );
        assert!(
            fit.contains("pixels: match negative.as_deref() {"),
            "the persisted pixel link follows the same capture"
        );
        assert!(now.contains("let Some(negative) = self.negative_path() else { return };"));
        assert!(
            now.contains("&cfg, &negative, &prompt, \"high\""),
            "the reimagine develops the negative, master included"
        );
    }

    /// The negative master prefers the ◈ card: while one exists, reverse-fit
    /// and reimagine read ITS master whichever card is active (the active ◈
    /// card first, then the first ◈ on the strip), and the ▣ card's own
    /// origin (a legacy in-place bake) only answers when no ◈ card exists.
    /// MUTATION: `negative_origin` back to the ▣-only definition, and the
    /// first assertion names it.
    #[test]
    fn the_negative_master_prefers_the_denoised_card() {
        let master = std::path::PathBuf::from("out/_negative_prefers_test.denoise.png");
        let legacy = std::path::PathBuf::from("out/_negative_prefers_test.legacy.png");
        let card = |kind: VariantKind, id: &str, origin: Option<std::path::PathBuf>| Variant {
            id: id.into(),
            name: None,
            kind,
            recipe: EditRecipe::default(),
            base: None,
            origin,
            thumb: None,
        };
        let mut app = AutoShadeApp {
            variants: vec![
                card(VariantKind::Original, ORIGINAL_VARIANT_ID, None),
                card(VariantKind::Generated, "gen", Some("out/_negative_prefers_test.reimagine.png".into())),
                card(VariantKind::Denoised, "dn", Some(master.clone())),
            ],
            ..Default::default()
        };
        for active in 0..3 {
            app.active = active;
            assert_eq!(
                app.negative_origin().as_deref(),
                Some(master.as_path()),
                "card {active} active: the ◈ master is the negative"
            );
        }
        // A legacy ▣ origin loses to the ◈ card…
        app.variants[0].origin = Some(legacy.clone());
        app.active = 0;
        assert_eq!(app.negative_origin().as_deref(), Some(master.as_path()));
        // …and answers once the ◈ card is gone.
        app.variants.remove(2);
        assert_eq!(app.negative_origin().as_deref(), Some(legacy.as_path()));
        app.variants[0].origin = None;
        assert_eq!(app.negative_origin(), None);
    }

    /// The reverse-fit's source (2026-09-30, user decision): automatic is the
    /// negative rule, a pick in the fold wins while that card stands and is a
    /// negative (▣ / ◈ / ▦), and a pick of anything else — or of a card since
    /// deleted — falls back to automatic. MUTATION: `fit_negative` back to
    /// `negative_origin`, and the ▣ pick names it.
    #[test]
    fn the_reverse_fit_solves_from_the_picked_source_card() {
        let one = std::path::PathBuf::from("out/_fit_source_test.denoise.png");
        let two = std::path::PathBuf::from("out/_fit_source_test.denoise-2.png");
        let card = |kind: VariantKind, id: &str, origin: Option<std::path::PathBuf>| Variant {
            id: id.into(),
            name: None,
            kind,
            recipe: EditRecipe::default(),
            base: None,
            origin,
            thumb: None,
        };
        let mut app = AutoShadeApp {
            variants: vec![
                card(VariantKind::Original, ORIGINAL_VARIANT_ID, None),
                card(VariantKind::Denoised, "dn1", Some(one.clone())),
                card(VariantKind::Denoised, "dn2", Some(two.clone())),
                card(VariantKind::Generated, "gen", Some("out/_fit_source_test.reimagine.png".into())),
            ],
            active: 3,
            ..Default::default()
        };
        // Automatic: standing on the ✨ card, the first ◈ master.
        assert_eq!(app.fit_source_index(), Some(1));
        assert_eq!(app.fit_negative().as_deref(), Some(one.as_path()));
        // The ▣ card picked: the loaded file's own frame, not a master.
        app.fit_from = Some(ORIGINAL_VARIANT_ID.into());
        assert_eq!(app.fit_source_index(), Some(0));
        assert_eq!(app.fit_negative(), None, "the ▣ pick solves from the RAW, not the ◈ master");
        // The second ◈ card picked.
        app.fit_from = Some("dn2".into());
        assert_eq!(app.fit_negative().as_deref(), Some(two.as_path()));
        // Not a source (the ✨ card), or gone: automatic again.
        app.fit_from = Some("gen".into());
        assert_eq!(app.fit_source_index(), Some(1));
        app.fit_from = Some("dn2".into());
        app.variants.remove(2);
        assert_eq!(app.fit_negative().as_deref(), Some(one.as_path()));
        // Only ▣ and ◈ / ▦ cards are offered.
        let offered: Vec<_> = app.variants.iter().filter(|v| AutoShadeApp::is_fit_source(v)).map(|v| v.id.as_str()).collect();
        assert_eq!(offered, [ORIGINAL_VARIANT_ID, "dn1"]);
    }

    /// A ◈ card round-trips through the strip record: the store admits the
    /// "denoised" spelling (`known_variant_kind`), the record carries its
    /// master as `origin`, and the spelling maps back to the kind.
    #[test]
    fn a_denoised_card_round_trips_through_the_strip_record() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-gui-denoised-card-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_denoised_card.ARW");
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]);
        let master = dir.join("denoise.png");
        let developed = EditRecipe { contrast: 4.0, ..Default::default() };
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        app.variants = vec![
            Variant {
                id: ORIGINAL_VARIANT_ID.into(),
                name: None,
                kind: VariantKind::Original,
                recipe: developed.clone(),
                base: None,
                origin: None,
                thumb: None,
            },
            Variant {
                id: "dn-1".into(),
                name: Some("clean".into()),
                kind: VariantKind::Denoised,
                recipe: developed.clone(),
                base: None,
                origin: Some(master.clone()),
                thumb: None,
            },
        ];
        app.active = 1;
        app.recipe = developed.clone();
        assert_eq!(VariantKind::Denoised.store_str(), "denoised");
        assert_eq!(VariantKind::from_store_str("denoised"), Some(VariantKind::Denoised));
        let rec = app.current_strip_record().expect("a two-card strip has a record");
        assert_eq!(rec.active_kind, "denoised");
        assert_eq!(rec.active_id.as_deref(), Some("dn-1"));
        assert_eq!(rec.active_name.as_deref(), Some("clean"));
        app.persist_strip(&src).expect("strip persists");
        let saved = saved_strip_of(&src);
        assert_eq!(saved.active_kind, "denoised");
        assert_eq!(saved.active_pos, 1);
        assert_eq!(saved.others.len(), 1);
        assert_eq!(saved.others[0].kind, "original");
        assert_eq!(saved.others[0].origin, None, "the ▣ card carries no master of its own");
        // The active card's master travels in pixels.json, the way every
        // origin-carrying active card's does, flagged by `on_ai_pixels` — and
        // a ◈ master is a camera frame, not AI pixels: it keeps calibration,
        // projects to a Lightroom XMP and never earns the AI caveats.
        assert!(!VariantKind::Denoised.on_ai_pixels());
        let save = include_str!("../actions.rs");
        assert!(
            save.contains("pixels: st.origin.clone().map(|o| (o, st.kind.on_ai_pixels())),"),
            "the save's pixel link no longer reads the card's kind"
        );
    }

    /// v1.5.0 Track S: a stack lands as a NEW ▦ card on the ◈ card's terms —
    /// the merged master is its `origin`, the develop it was run from is its
    /// recipe, the frame it was made from keeps its own pixels — and it
    /// becomes the negative. An HDR merge hands over the stops it recovered on
    /// top of that: they arrive as the SDR rendition stage turned ON with that
    /// much room, because without them the section would offer a range the
    /// frame does not have, and the recovered highlights would be unreachable.
    ///
    /// MUTATION: `NewStacked` → `NewGenerated` in start_stack; drop the
    /// headroom transfer in the landing; `is_remade_negative` back to ◈ alone.
    #[test]
    fn a_stack_lands_as_a_new_stacked_card_that_becomes_the_negative() {
        let ctx = egui::Context::default();
        let src = std::path::PathBuf::from("_stack_landing_test.ARW");
        let developed = EditRecipe { contrast: 11.0, ..Default::default() };
        let mut app = AutoShadeApp {
            src_path: Some(src.clone()),
            variants: vec![Variant {
                id: ORIGINAL_VARIANT_ID.into(),
                name: None,
                kind: VariantKind::Original,
                recipe: developed.clone(),
                base: None,
                origin: None,
                thumb: None,
            }],
            recipe: developed.clone(),
            ..Default::default()
        };
        app.reset_history();
        let out = std::path::PathBuf::from("out/_stack_landing_test.stack.png");
        let epoch = app.gen_epoch;
        app.on_retouched(
            &ctx,
            Lang::En,
            epoch,
            Ok((
                image::DynamicImage::new_rgba8(4, 4),
                RetouchNote::Stacked {
                    out: out.clone(),
                    kind: autoshade::stack::merge::StackKind::Hdr,
                    frames: 3,
                    travel: 4.2,
                    uncovered: 0.0125,
                    headroom_ev: 3.75,
                },
                out.clone(),
                RetouchKind::NewStacked,
            )),
        );
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Stacked],
            "{}",
            app.status
        );
        assert_eq!(app.active, 1, "the new ▦ card is under the canvas");
        let card = &app.variants[1];
        assert_eq!(card.origin.as_deref(), Some(out.as_path()), "…hanging off the merged master");
        assert_eq!(card.recipe.contrast, developed.contrast, "…carrying the develop it was run from");
        assert!(card.kind.is_source_based(), "a ▦ master is a camera frame, not AI pixels");
        assert!(!card.kind.on_ai_pixels(), "…so it earns none of the AI caveats");
        assert!(card.kind.is_remade_negative(), "…and it is the negative, remade");
        assert!(card.recipe.hdr_edit, "the recovered stops must reach the rendition stage");
        assert!((card.recipe.hdr_max_ev - 3.75).abs() < 1e-6, "…with the room the merge measured");
        assert_eq!(app.variants[0].recipe, developed, "the frame it was made from keeps its develop");
        assert_eq!(app.variants[0].origin, None, "…and its untouched pixel source");
        assert_eq!(
            app.negative_origin().as_deref(),
            Some(out.as_path()),
            "the negative is now the ▦ master"
        );
        assert_eq!(VariantKind::Stacked.store_str(), "stacked");
        assert_eq!(VariantKind::from_store_str("stacked"), Some(VariantKind::Stacked));
        // The landing line names the merge that ran and carries the three
        // numbers worth reading before trusting a stack.
        for want in ["3 frames", "HDR merge", "new ▦ card", "4.2 px", "1.25%", "3.75 EV"] {
            assert!(app.status.contains(want), "{want} missing from: {}", app.status);
        }
        // The verb's own choices are pinned in the source (the house pattern
        // for a worker a unit test cannot run): the stack lands as a NEW card,
        // always at full resolution (`None` cap), and the frames the user
        // picked JOIN this card's frame rather than replacing it.
        let now = include_str!("../panels/retouch.rs");
        let body = &now[now.find("pub(crate) fn start_stack(").expect("start_stack moved")..];
        let body = &body[..body.find("pub(crate) fn start_clone(").expect("start_clone moved")];
        assert!(body.contains("RetouchKind::NewStacked,"), "the stack no longer lands as a new card");
        assert!(
            body.contains("let mut inputs = vec![path.clone()];"),
            "this card's frame is no longer the stack's reference"
        );
        assert!(
            body.contains("stack_files(&inputs, &opts, None, &out)"),
            "the stack no longer runs on the full frame"
        );
    }

    /// 「🤖 AI Denoise on export」 sits out on a ◈ card — its master is already
    /// the denoised negative; a second pass would only smooth it further —
    /// and the export echo says so by carrying no amount.
    #[test]
    fn the_export_denoise_is_skipped_on_a_denoised_card() {
        let card = |kind: VariantKind| Variant {
            id: "c".into(),
            name: None,
            kind,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let mut app = AutoShadeApp { save_denoise: true, save_denoise_strength: 0.35, ..Default::default() };
        for kind in [VariantKind::Original, VariantKind::Generated, VariantKind::Fitted, VariantKind::Edited] {
            app.variants = vec![card(kind)];
            app.active = 0;
            assert!(app.export_denoise_applies(), "{kind:?}");
            assert!(app.export_summary(Lang::En).contains("AI Denoise 35%"), "{kind:?}");
        }
        app.variants = vec![card(VariantKind::Denoised)];
        assert!(!app.export_denoise_applies());
        assert!(!app.export_summary(Lang::En).contains("AI Denoise"), "{}", app.export_summary(Lang::En));
        app.save_denoise = false;
        app.variants = vec![card(VariantKind::Original)];
        assert!(!app.export_denoise_applies(), "unticked is unticked on any card");
        // The render reads the gate, not the raw checkbox.
        let export = include_str!("../export.rs");
        assert!(export.contains("let denoise = self.export_denoise_applies();"));
    }

    /// 「＋」 has nothing to snapshot on the pristine ✨ card and everything
    /// on the ✎ card — attributed to it.
    #[test]
    fn a_version_snapshot_is_refused_on_the_pristine_ai_card_and_taken_on_the_edited_one() {
        let (mut app, _ctx, _dir, src, _master, _ai_px, _scrub) =
            ai_card_fixture("snapshot", EditRecipe::default());
        app.save_version();
        assert!(app.status.contains("pristine"), "{}", app.status);
        assert!(!autoshade::store::version_target(&src, 1).exists());
        app.recipe.contrast = 7.0;
        app.save_version(); // forks at the boundary, then snapshots the ✎ card
        assert_eq!(app.variants[app.active].kind, VariantKind::Edited);
        let v1 = autoshade::store::version_target(&src, 1);
        assert!(v1.exists(), "{}", app.status);
        let snap: EditRecipe = serde_json::from_str(&std::fs::read_to_string(&v1).unwrap()).unwrap();
        assert_eq!(snap.contrast, 7.0);
        let meta = autoshade::store::read_version_meta(&src);
        assert_eq!(
            meta.iter().find(|m| m.n == 1).and_then(|m| m.from_kind.as_deref()),
            Some("edited")
        );
    }

    /// 「▣ apply to Original」 refuses both AI-pixel kinds, each for its own
    /// reason.
    #[test]
    fn apply_to_original_refuses_an_edited_card_with_its_own_reason() {
        let (mut app, ctx, _dir, _src, _master, _ai_px, _scrub) =
            ai_card_fixture("apply", EditRecipe::default());
        app.apply_to_original(1, &ctx);
        assert!(app.status.contains("lives in its pixels"), "{}", app.status);
        app.recipe.contrast = 7.0;
        assert!(app.fork_edited_card());
        app.apply_to_original(2, &ctx);
        assert!(app.status.contains("AI-generated pixels"), "{}", app.status);
        assert_eq!(app.variants[0].recipe.contrast, 4.0, "the ▣ card is untouched");
    }
