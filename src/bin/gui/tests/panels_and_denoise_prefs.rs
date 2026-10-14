// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the mask panel, the curve editor, imported removals, HDR, the AI dot, grade strength and the denoise preference eras.

    /// R22-5 (#10): the selected mask's adjustments, in Lightroom's three
    /// groups, with the 「More (XMP/Lightroom only)」 fold GONE — R22-3 put
    /// clarity/dehaze/texture on the engine, so that title had become false
    /// while still hiding three working sliders one level down. Renders the
    /// real develop_panel and pins the group caption, the recolour disclosure
    /// (a mask carrying gains no sidecar can express), and the
    /// masks-but-none-selected hint (one click on a selected row lands there,
    /// and it used to show a list with no controls and no explanation).
    #[test]
    fn the_mask_panel_groups_its_sliders_and_says_what_is_engine_only() {
        // Every text the frame drew (nested Shape::Vec included).
        fn texts(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<String>) {
            fn walk(s: &egui::Shape, out: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    _ => {}
                }
            }
            shapes.iter().for_each(|c| walk(&c.shape, out));
        }
        for (lang, group, recolour, hint, new_sliders) in [
            (
                crate::i18n::Lang::En,
                "Tone",
                "carries reverse-fit recolour (not exported to XMP)",
                "Select a mask above to edit its adjustments",
                ["Sharpness", "Hue shift"],
            ),
            (
                crate::i18n::Lang::Zh,
                "明暗",
                "含反推重上色（不写入 XMP）",
                "选中上面任一蒙版即可编辑它的调整",
                ["局部锐化", "色相旋转"],
            ),
        ] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.recipe.masks = vec![
                autoshade::recipe::LocalAdjustment {
                    mask: autoshade::recipe::MaskGeometry::Radial {
                        top: 0.2, left: 0.2, bottom: 0.8, right: 0.8,
                        feather: 0.5, roundness: 0.0, flipped: false, angle: 15.0,
                        midpoint: 50.0, mask_version: 2,
                    },
                    color_gains: Some([1.3, 1.0, 0.7]),
                    name: "gold".into(),
                    ..Default::default()
                },
                autoshade::recipe::LocalAdjustment { name: "grad".into(), ..Default::default() },
            ];
            // TALL on purpose: egui culls shapes outside the visible clip
            // rect, and Local Masks sits below a full screen of sections — a
            // 900 px window drew nothing of it to inspect.
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 20_000.0),
                )),
                ..Default::default()
            };
            let frame = |app: &mut AutoShadeApp| -> Vec<String> {
                let mut seen = Vec::new();
                let out = ctx.run(input(), |ctx| {
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                    });
                });
                texts(&out.shapes, &mut seen);
                seen
            };
            // ① a mask selected: the group captions and the engine-only note.
            app.sel_mask = Some(0);
            let seen = frame(&mut app);
            assert!(
                seen.iter().any(|t| t == group),
                "{lang:?}: no {group:?} group caption over the mask's tone sliders: {seen:?}"
            );
            assert!(
                seen.iter().any(|t| t == recolour),
                "{lang:?}: a mask with color_gains must say the XMP will not carry them"
            );
            assert!(
                !seen.iter().any(|t| t.contains("XMP/Lightroom only")),
                "{lang:?}: the 「More (XMP/Lightroom only)」 fold is back — those three \
                 sliders render in the engine now"
            );
            // R23-1b: the two controls the recipe grew this round. A field that
            // renders, exports and is offered to the AI but has no slider is
            // reachable only by the model — the user cannot answer it.
            for label in new_sliders {
                assert!(
                    seen.iter().any(|t| t == label),
                    "{lang:?}: the mask panel has no {label:?} slider: {seen:?}"
                );
            }
            // ② nothing selected (one click on the selected row): the hint.
            app.sel_mask = None;
            let seen = frame(&mut app);
            assert!(
                seen.iter().any(|t| t == hint),
                "{lang:?}: masks exist but none is selected — the panel must say so"
            );
            assert!(
                !seen.iter().any(|t| t == recolour),
                "{lang:?}: no selection ⇒ no per-mask detail"
            );
        }
    }

    /// R25 P6: the selected mask gets the SAME curve editor the global Curves
    /// section has, pointed at its own four curves — a fourth group under
    /// Lightroom's Tone / Detail / Color.
    ///
    /// Two halves, because either alone passes vacuously. The panel half
    /// proves the group is laid out only for a selected mask; the driven half
    /// proves the editor writes to `masks[0].main_curve` and NOT to
    /// `recipe.tone_curve` — a target parameter that is ignored (or a copied
    /// `curve_points` arm) looks identical on screen and edits the wrong photo
    /// state, which is exactly the class of bug U10 caught the first time.
    #[test]
    fn the_selected_mask_offers_a_curve_editor() {
        for (lang, caption) in
            [(crate::i18n::Lang::En, "Curve"), (crate::i18n::Lang::Zh, "曲线")]
        {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.recipe.masks =
                vec![autoshade::recipe::LocalAdjustment { name: "sky".into(), ..Default::default() }];
            // ① selected → the fourth group caption is drawn.
            app.sel_mask = Some(0);
            let seen = tall_frame(&mut app, |a, ui| {
                a.develop_panel(ui);
            });
            assert!(
                seen.iter().any(|t| t == caption),
                "{lang:?}: no {caption:?} group over the selected mask's curve editor: {seen:?}"
            );
            // ② nothing selected → no per-mask curve group. (The global
            //    「Curves」 section is a different caption on purpose, so this
            //    negative cannot be satisfied by it.)
            app.sel_mask = None;
            let seen = tall_frame(&mut app, |a, ui| {
                a.develop_panel(ui);
            });
            assert!(
                !seen.iter().any(|t| t == caption),
                "{lang:?}: a {caption:?} group with no mask selected: {seen:?}"
            );
        }

        // ③ the driven half: a click in the MASK editor lands in the mask.
        let mut app = AutoShadeApp::default();
        app.recipe.masks =
            vec![autoshade::recipe::LocalAdjustment { name: "sky".into(), ..Default::default() }];
        let ctx = egui::Context::default();
        let run_pass = |app: &mut AutoShadeApp, events: Vec<egui::Event>| -> bool {
            let mut changed = false;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    changed |= app.curve_editor_for(ui, CurveTarget::Mask(0));
                });
            });
            changed
        };
        let _ = run_pass(&mut app, vec![]);
        let rect = app.curve_rect.expect("the mask editor records its square (test seam)");
        let q = egui::pos2(rect.min.x + rect.width() * 0.25, rect.min.y + rect.height() * 0.5);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: q,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let pressed = run_pass(&mut app, vec![egui::Event::PointerMoved(q), button(true)]);
        let released = run_pass(&mut app, vec![button(false)]);
        assert!(pressed || released, "the mask editor must report the edit");
        assert_eq!(
            app.recipe.masks[0].main_curve.len(),
            1,
            "the click adds one point to the MASK's master curve"
        );
        assert!(
            app.recipe.tone_curve.is_empty(),
            "the mask editor wrote into the GLOBAL tone curve: {:?}",
            app.recipe.tone_curve
        );
        // ④ a target that no longer addresses a mask draws nothing rather
        //    than falling through to the global curves.
        app.recipe.masks.clear();
        let mut drew = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                drew = app.curve_editor_for(ui, CurveTarget::Mask(0));
            });
        });
        assert!(!drew, "a stale mask index must not report an edit");
        assert!(app.recipe.tone_curve.is_empty(), "…and must not touch the global curves");
    }

    /// Every text one frame drew, nested `Shape::Vec` included — the way to read
    /// a `CollapsingHeader`'s own label (and therefore its ● or lack of one)
    /// without a seam per section.
    fn drawn_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(s: &egui::Shape, out: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    /// A tall frame: egui culls shapes outside the visible clip rect, and the
    /// lower sections sit below a full screen of siblings.
    /// v1.5.0 F9: a photograph that arrives carrying Lightroom's spot removal
    /// SAYS so, and the ✨ re-solve is offered for exactly the areas Adobe
    /// synthesised.
    ///
    /// Three arms, because two of them are the complement that makes the first
    /// mean anything: a photo with no areas draws none of this, and a photo
    /// whose areas are all plain `heal` gets the summary WITHOUT the button —
    /// there is nothing for a model to redo when Lightroom only copied pixels.
    ///
    /// MUTATION THIS CATCHES: draw the group unconditionally; offer the ✨ verb
    /// on the whole list instead of `is_synthesised`; count the areas with
    /// `masks.len()`.
    #[test]
    fn an_imported_removal_names_itself_and_offers_the_resolve_only_where_adobe_invented() {
        use autoshade::retouch::{RetouchArea, RetouchShape, SpotOrigin};
        let area = |origin| RetouchArea {
            origin,
            feather: 0.5,
            donor: None,
            shape: RetouchShape::Ellipse { cx: 0.5, cy: 0.5, size_x: 0.05, size_y: 0.05 },
        };
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            // 1) Nothing imported: not a word of it on screen.
            let mut clean = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut clean, |a, ui| a.retouch_panel(ui));
            assert!(
                !seen.iter().any(|t| t == tr(lang, "Imported removal")),
                "{lang:?}: a photo with no retouch draws no group: {seen:?}"
            );

            // 2) A plain Lightroom heal: the summary, and NO ✨ verb — the
            //    pixels were copied, so there is nothing for a model to redo.
            let mut copied = AutoShadeApp { lang, ..Default::default() };
            copied.recipe.retouch = vec![area(SpotOrigin::LightroomHeal)];
            let seen = tall_frame(&mut copied, |a, ui| a.retouch_panel(ui));
            assert!(
                seen.iter().any(|t| t == tr(lang, "Imported removal")),
                "{lang:?}: an imported area names itself: {seen:?}"
            );
            assert!(
                !seen.iter().any(|t| t == tr(lang, "✨ Regenerate those areas")),
                "{lang:?}: a copied repair gets no generative verb: {seen:?}"
            );

            // 3) One Adobe-synthesised area among them: the verb appears.
            let mut invented = AutoShadeApp { lang, ..Default::default() };
            invented.recipe.retouch = vec![
                area(SpotOrigin::LightroomHeal),
                area(SpotOrigin::LightroomGenerative),
            ];
            let seen = tall_frame(&mut invented, |a, ui| a.retouch_panel(ui));
            assert!(
                seen.iter().any(|t| t == tr(lang, "✨ Regenerate those areas")),
                "{lang:?}: a synthesised area gets the re-solve: {seen:?}"
            );
            // …and the count in the summary is the AREAS, both of them.
            assert!(
                seen.iter().any(|t| t.contains('2')),
                "{lang:?}: the summary counts the areas: {seen:?}"
            );
        }
    }

    /// v1.5.0 F8: a stored SDR control lights its collapsed section's ●, even
    /// while HDR edit mode is off and it is rendering nothing.
    ///
    /// That combination is the whole point and the reason the dot is asserted
    /// here rather than left to the family table's own test. A sidecar can
    /// carry `crs:SDRBrightness="+40"` from an HDR session the photographer
    /// later left; this program stores it, round-trips it, and deliberately
    /// does NOT render it. A value that a file holds, that a save will write
    /// back, and that no pixel reflects is exactly the kind a collapsed
    /// section hides — so the ● has to be on.
    ///
    /// MUTATION THIS CATCHES: drive the dot from `hdr_edit` alone instead of
    /// from the `hdr` family (arm 2 goes dark); drop the section from
    /// `develop_panel` (every arm loses its header).
    #[test]
    fn a_stored_sdr_control_lights_the_hdr_section_even_with_the_mode_off() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let title = tr(lang, "HDR & SDR").to_string();
            let lit = format!("{title}  ●");

            // 1) Nothing: the section is there, and dark.
            let mut clean = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut clean, |a, ui| {
                a.develop_panel(ui);
            });
            assert!(
                seen.iter().any(|t| t.starts_with(&title)),
                "{lang:?}: the section is always drawn: {seen:?}"
            );
            assert!(
                !seen.contains(&lit),
                "{lang:?}: an untouched recipe lights nothing: {seen:?}"
            );

            // 2) A stored control with the MODE OFF — renders nothing, and
            //    must still be visible.
            let mut stored = AutoShadeApp { lang, ..Default::default() };
            stored.recipe.sdr_brightness = 40.0;
            let seen = tall_frame(&mut stored, |a, ui| {
                a.develop_panel(ui);
            });
            assert!(
                seen.contains(&lit),
                "{lang:?}: a stored SDR control lights the ● with the mode off: {seen:?}"
            );

            // 3) …and so does the mode by itself.
            let mut on = AutoShadeApp { lang, ..Default::default() };
            on.recipe.hdr_edit = true;
            let seen = tall_frame(&mut on, |a, ui| {
                a.develop_panel(ui);
            });
            assert!(
                seen.contains(&lit),
                "{lang:?}: HDR edit mode alone lights the ●: {seen:?}"
            );
        }
    }

    fn tall_frame(app: &mut AutoShadeApp, f: impl Fn(&mut AutoShadeApp, &mut egui::Ui)) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 20_000.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            ctx.memory_mut(|m| m.set_everything_is_visible(true));
            egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| f(app, ui));
            });
        });
        drawn_texts(&out.shapes)
    }

    /// User decision 2026-09-20: the AI header describes this photo's verdict
    /// and Direction. Saved taste dials alone must never light it, even when
    /// all three differ from their defaults. Read the drawn header in both
    /// languages so a disconnected predicate cannot pass this test.
    #[test]
    fn the_ai_dot_reads_this_photos_state_not_the_saved_dials() {
        assert_eq!(
            AutoShadeApp::default().style_strength, STYLE_STRENGTH_DEFAULT,
            "the app must start at the shared default"
        );
        assert_eq!(
            Prefs::default().style_strength, STYLE_STRENGTH_DEFAULT,
            "a pref key missing from an older save must use the same dial default"
        );
        for lang in [Lang::En, Lang::Zh] {
            let title = tr(lang, "AI");
            let lit = format!("{title}  ●");
            for (case, has_verdict, guidance, active) in [
                ("saved dials only", false, "", false),
                ("verdict", true, "", true),
                ("Direction", false, "warmer and moodier", true),
            ] {
                let mut app = AutoShadeApp {
                    lang,
                    style_strength: 0.80,
                    grade_strength: 0.85,
                    fit_strength: 0.85,
                    verdict: has_verdict.then(|| (
                        autoshade::advisor::Decision::Accept, vec!["ok".into()]
                    )),
                    guidance: guidance.into(),
                    ..Default::default()
                };
                let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
                let headers: Vec<_> = seen.iter().map(String::as_str)
                    .filter(|t| *t == title || *t == lit.as_str()).collect();
                assert_eq!(
                    headers, vec![if active { lit.as_str() } else { title }],
                    "{lang:?}: {case}: only this photo's verdict or Direction may light the AI dot"
                );
            }
        }
    }

    /// R23-3 (feedback #5, "the AI is too timid + give me a strength slider"):
    /// the desktop SHELL of the grade-strength axis.
    ///
    /// Four properties, because a slider that fails any one of them is
    /// decoration: it must be DRAWN beside Style (the two are one pair of axes,
    /// and the reported problem was that only the style half existed), start and
    /// persist at ONE constant shared with the lib, leave the section ● quiet
    /// when moved alone, and actually reach the request the analyze worker builds.
    #[test]
    fn the_grade_strength_slider_sits_beside_style_and_rides_the_analyze_request() {
        // One definition, three consumers (app default / pref default / the
        // slider's reset target) — the R22 #16 rule, applied to the new dial.
        assert_eq!(AutoShadeApp::default().grade_strength, GRADE_STRENGTH_DEFAULT);
        assert_eq!(
            Prefs::default().grade_strength, GRADE_STRENGTH_DEFAULT,
            "a prefs file written before this key existed must decode to the SAME number — \
             serde's own f32 default (0.0) would be the most TIMID setting on a dial that \
             exists because the AI was too timid"
        );
        assert_eq!(
            GRADE_STRENGTH_DEFAULT,
            autoshade::recipe::GradeStrength::DEFAULT,
            "the GUI must not own a second copy of this number — the CLI and the web body \
             default through the lib's constant"
        );
        assert_eq!(GRADE_STRENGTH_DEFAULT, 0.65, "user decision 2026-08-17 ⑦");

        // DRAWN — at the DEFAULT 320 px side panel, which is the whole point:
        // `tall_frame` builds that width, and before this round the two dials
        // shared the verbs' row, where `egui::Slider`'s own nested (non-wrapping)
        // horizontal overflowed the clip rect. The row rendered Style's value
        // "30" with the word "Style" itself off-panel, and a second dial there
        // was invisible outright. Assert the LABELS, not just the values: the
        // label is the half that was being clipped.
        let mut app = AutoShadeApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        for label in ["Style", "Strength", "30", "65"] {
            assert!(
                seen.iter().any(|t| t == label),
                "「{label}」 was not drawn in a 320 px panel: {seen:?}"
            );
        }

        // A remembered preference stays quiet, including a move DOWN to the
        // calibration point, which differs from the shipped default.
        assert!(!app.ai_section_active(), "a fresh AI area has no state to flag");
        app.grade_strength = autoshade::recipe::GradeStrength::CALIBRATED;
        assert!(
            !app.ai_section_active(),
            "a moved Strength slider is a preference, not this photo's AI state"
        );
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "AI") && !seen.iter().any(|t| t == "AI  ●"),
            "a moved Strength slider must leave the AI header plain: {seen:?}"
        );
        app.grade_strength = GRADE_STRENGTH_DEFAULT;
        assert!(!app.ai_section_active(), "resetting a preference also leaves the dot unlit");

        // …and it reaches the worker's request, on its OWN axis (the two dials
        // must not be able to swap: `style` is a bare fraction, `strength` a
        // `GradeStrength`, so a transposed pair would not compile).
        let app = AutoShadeApp { grade_strength: 0.9, style_strength: 0.2, ..Default::default() };
        let req = autoshade::pipeline::GradeRequest {
            style: app.style_strength,
            send_reference_image: app.send_style_ref_image,
            strength: autoshade::recipe::GradeStrength::new(app.grade_strength),
            think: app.deep_think,
            adherence: autoshade::recipe::DirectionAdherence::default(),
            embed: autoshade::library::EmbeddingSwitch::resolve(None, app.style_embed),
        };
        assert_eq!(req.strength.get(), 0.9);
        assert_eq!(req.style, 0.2);
    }

    #[derive(Default)]
    struct PrefsMemory(std::collections::HashMap<String, String>);

    impl eframe::Storage for PrefsMemory {
        fn get_string(&self, key: &str) -> Option<String> { self.0.get(key).cloned() }
        fn set_string(&mut self, key: &str, value: String) { self.0.insert(key.into(), value); }
        fn flush(&mut self) {}
    }

    #[test]
    fn era_zero_denoise_prefs_reset_both_dials_and_explain_the_old_values() {
        for lang in [Lang::En, Lang::Zh] {
            let mut storage = PrefsMemory::default();
            eframe::Storage::set_string(&mut storage, eframe::APP_KEY,
                format!("(denoise_strength:0.65,save_denoise_strength:0.5,lang:{lang:?})"));
            let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
            assert_eq!(prefs.prefs_era, 0, "a missing key must not inherit the current era");
            let mut app = AutoShadeApp::default();
            app.restore_prefs(prefs);
            assert_eq!((app.denoise_strength, app.save_denoise_strength), (autoshade::denoise::DEFAULT_STRENGTH_RAW, autoshade::denoise::DEFAULT_STRENGTH_RAW),
                "era 0 must reset both denoise dials to the RAW blend default");
            for old in ["71", "65", "50"] {
                assert!(app.status.contains(old), "{old} missing from {}", app.status);
            }
            assert_eq!(app.status.matches(if lang == Lang::En { "reset to" } else { "已重置" }).count(), 1);
            assert!(!app.status.contains("{a}"));
            assert!(app.toasts.is_empty(), "the startup status is the existing disclosure surface");
        }
    }

    #[test]
    fn denoise_prefs_note_survives_the_startup_folder_scan_once() {
        let mut app = AutoShadeApp { busy: true, ..Default::default() };
        app.restore_prefs(Prefs { prefs_era: 0, denoise_strength: 0.65,
            save_denoise_strength: 0.5, ..Prefs::default() });
        assert!(app.startup_note.is_some());
        app.tx.send(Msg::Folder(Box::new(Ok((PathBuf::new(), vec![], 0))))).unwrap();
        app.poll_workers(&egui::Context::default());
        assert!(app.status.contains("no photos found"), "{}", app.status);
        assert!(app.status.contains("reset to 71"), "{}", app.status);
        assert!(app.startup_note.is_none());
        app.tx.send(Msg::Folder(Box::new(Ok((PathBuf::new(), vec![], 0))))).unwrap();
        app.poll_workers(&egui::Context::default());
        assert!(!app.status.contains("reset to"), "a later scan must not repeat the startup note");
    }

    #[test]
    fn current_era_denoise_prefs_restore_without_a_reset_note() {
        let mut app = AutoShadeApp::default();
        app.restore_prefs(Prefs { prefs_era: 2, denoise_strength: 0.65,
            save_denoise_strength: 0.5, ..Prefs::default() });
        assert_eq!((app.denoise_strength, app.save_denoise_strength), (0.65, 0.5));
        assert_eq!(app.status, tr(Lang::En, "Open a photo, or open a folder to browse your library."));
        assert!(app.startup_note.is_none());
    }

    #[test]
    fn prefs_without_denoise_keys_restore_the_raw_defaults_without_a_note() {
        let mut storage = PrefsMemory::default();
        eframe::Storage::set_string(&mut storage, eframe::APP_KEY, "()".into());
        let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert_eq!(prefs.prefs_era, 0);
        let mut app = AutoShadeApp::default();
        app.restore_prefs(prefs);
        assert_eq!((app.denoise_strength, app.save_denoise_strength), (autoshade::denoise::DEFAULT_STRENGTH_RAW, autoshade::denoise::DEFAULT_STRENGTH_RAW));
        assert_eq!(app.status, tr(Lang::En, "Open a photo, or open a folder to browse your library."));
    }

    #[test]
    fn saving_prefs_records_the_current_era_and_preserves_new_choices() {
        assert_eq!(Prefs::default().prefs_era, PREFS_ERA);
        let mut app = AutoShadeApp { denoise_strength: 0.65, save_denoise_strength: 0.5, ..Default::default() };
        let mut storage = PrefsMemory::default();
        eframe::App::save(&mut app, &mut storage);
        let saved: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert_eq!(saved.prefs_era, PREFS_ERA);
        let mut reopened = AutoShadeApp::default();
        reopened.restore_prefs(saved);
        assert_eq!((reopened.denoise_strength, reopened.save_denoise_strength), (0.65, 0.5));
        assert!(!reopened.status.contains("reset to"));
    }

    /// 2026-09-13, the same user decision applied to the denoiser's two
    /// timings: 「🤖 AI Denoise now」 (Detail fold) and 「🤖 AI Denoise on
    /// export」 (Export fold) each read a dial of their own, both starting at
    /// `denoise::DEFAULT_STRENGTH_RAW` for RAW sources, with independent
    /// baked-source SCUNet choices starting at 0.5, each on its
    /// own prefs key — an older prefs file must decode to that default and
    /// never to serde's 0.0 (the identity: every denoise would silently do
    /// nothing after an upgrade). The worker halves are pinned on their
    /// sources, like the fit's dial.
    #[test]
    fn gui_ai_denoise_has_a_dial_in_each_fold() {
        use autoshade::denoise::DEFAULT_STRENGTH_RAW as DEFAULT_STRENGTH;
        let app = AutoShadeApp::default();
        assert_eq!(DEFAULT_STRENGTH, 0.71);
        assert_eq!(app.denoise_strength, DEFAULT_STRENGTH);
        assert_eq!(app.save_denoise_strength, DEFAULT_STRENGTH);
        assert_eq!(Prefs::default().denoise_strength, DEFAULT_STRENGTH);
        assert_eq!(Prefs::default().save_denoise_strength, DEFAULT_STRENGTH);
        // Two keys, two values, and a file without them loads the default.
        let prefs = Prefs { denoise_strength: 0.8, save_denoise_strength: 0.3, ..Prefs::default() };
        let json = serde_json::to_string(&prefs).expect("prefs serialize");
        let decoded: Prefs = serde_json::from_str(&json).expect("prefs deserialize");
        assert_eq!(decoded.denoise_strength, 0.8);
        assert_eq!(decoded.save_denoise_strength, 0.3);
        let older = json
            .replace(r#""denoise_strength":0.8,"#, "")
            .replace(r#""save_denoise_strength":0.3,"#, "");
        assert!(!older.contains("\"denoise_strength\"") && !older.contains("\"save_denoise_strength\""), "both keys really were removed: {older}");
        let decoded: Prefs = serde_json::from_str(&older).expect("an older prefs file loads");
        assert_eq!(decoded.denoise_strength, DEFAULT_STRENGTH);
        assert_eq!(decoded.save_denoise_strength, DEFAULT_STRENGTH);
        // DRAWN, each in its own fold, at the default 320 px panel.
        let mut app = AutoShadeApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "AI denoise strength"),
            "the Detail fold's dial must exist: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t == "Export denoise strength"),
            "the Export fold's dial must exist: {seen:?}"
        );
        // The export echo carries the amount that will actually land.
        let app = AutoShadeApp { save_denoise: true, save_denoise_strength: 0.35, ..Default::default() };
        assert!(
            app.export_summary(Lang::En).contains("AI Denoise 35%"),
            "{}",
            app.export_summary(Lang::En)
        );
        // …and each verb reads ITS fold's dial: no literal strength survives
        // in either worker, and neither reads the other's field.
        let now = include_str!("../panels/retouch.rs");
        assert!(now.contains("let strength = self.selected_denoise_strength(false);"));
        assert!(now.contains("DenoiseOpts::from_config(&cfg, None, strength)"));
        assert!(!now.contains("save_denoise_strength"), "「AI Denoise now」 must not read the Export dial");
        let export = include_str!("../export.rs");
        assert!(export.contains("let denoise_strength = self.selected_denoise_strength(true);"));
        assert!(!export.contains("self.denoise_strength"), "the export must not read the Detail dial");
        for (name, src) in [("panels/retouch.rs", now), ("export.rs", export)] {
            assert!(
                !src.contains("None, 1.0)") && !src.contains("None, 0.5)"),
                "{name}: a literal strength bypasses the dial"
            );
        }
    }

    #[test]
    fn era_one_resets_raw_dials_but_preserves_baked_choices() {
        for lang in [Lang::En, Lang::Zh] {
            let mut app = AutoShadeApp::default();
            app.restore_prefs(Prefs { prefs_era: 1, denoise_strength: 1.0,
                save_denoise_strength: 1.0, lang, ..Prefs::default() });
            let default = autoshade::denoise::DEFAULT_STRENGTH_RAW;
            assert_eq!((app.denoise_strength, app.save_denoise_strength), (default, default),
                "era 2 must reset both RAW dials to the luminance-grain default");
            assert!(app.status.contains("100"), "old values missing: {}", app.status);
            assert!(app.status.contains("71"), "new default missing: {}", app.status);
            assert!(app.status.contains("Lightroom"));
            assert_eq!((app.baked_denoise_strength, app.baked_save_denoise_strength), (1.0, 1.0));
        }
        let mut app = AutoShadeApp::default();
        app.restore_prefs(Prefs { prefs_era: 1, denoise_strength: 0.45,
            save_denoise_strength: 0.6, ..Prefs::default() });
        app.src_path = Some(PathBuf::from("master.tif"));
        assert_eq!(app.selected_denoise_strength(false), 0.45);
        assert_eq!(app.selected_denoise_strength(true), 0.6);
        let mut storage = PrefsMemory::default();
        eframe::App::save(&mut app, &mut storage);
        let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert_eq!(prefs.prefs_era, 2);
        let mut reopened = AutoShadeApp::default();
        reopened.restore_prefs(prefs);
        reopened.src_path = Some(PathBuf::from("master.tif"));
        assert_eq!(reopened.selected_denoise_strength(false), 0.45);
        assert_eq!(reopened.selected_denoise_strength(true), 0.6);
        assert!(!reopened.status.contains("reset to"));
        reopened.src_path = Some(PathBuf::from("frame.ARW"));
        assert_eq!(reopened.selected_denoise_strength(false), autoshade::denoise::DEFAULT_STRENGTH_RAW);
    }

    /// User decision 2026-09-12 (「该在哪就在哪」): a control belongs to the fold
    /// whose function it serves, and a function two folds need gets two
    /// controls. The reverse-fit's honesty budget (F1) is therefore its OWN dial
    /// in the Reverse-fit fold: the Analysis Strength two folds up no longer
    /// reaches the fit, and the fit's dial never reaches the analyze request.
    /// The worker half is pinned textually on the worker's source (the
    /// `config.rs` literal-pin pattern) because the fit block runs on a worker
    /// thread behind a segmentation call.
    #[test]
    fn gui_reverse_fit_has_its_own_strength_dial() {
        // One constant, three consumers: app default, pref default, reset target.
        assert_eq!(AutoShadeApp::default().fit_strength, autoshade::recipe::GradeStrength::DEFAULT);
        assert_eq!(
            Prefs::default().fit_strength,
            autoshade::recipe::GradeStrength::DEFAULT,
            "a prefs file written before this key existed must decode to the byte-identical \
             budget, never to serde's 0.0"
        );
        assert_eq!(
            AutoShadeApp::default().fit_strength().get(),
            autoshade::recipe::GradeStrength::DEFAULT,
            "the shipped dial value IS the byte-identical default budget"
        );
        // Two dials, two readings: moving either leaves the other alone.
        let app = AutoShadeApp { grade_strength: 0.9, ..Default::default() };
        assert_eq!(app.analysis_strength().get(), 0.9);
        assert_eq!(
            autoshade::fit::FitBudget::for_strength(app.fit_strength()).vetoes,
            autoshade::fit::VetoPolicy::Withhold,
            "the Analysis Strength must not reach the fit"
        );
        let app = AutoShadeApp { fit_strength: 0.9, ..Default::default() };
        assert_eq!(
            autoshade::fit::FitBudget::for_strength(app.fit_strength()).vetoes,
            autoshade::fit::VetoPolicy::Disclose
        );
        assert_eq!(
            app.analysis_strength().get(),
            GRADE_STRENGTH_DEFAULT,
            "and the fit's dial never reaches the analyze request"
        );
        // DRAWN in the Reverse-fit fold at the default 320 px panel.
        let mut app = AutoShadeApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Reverse-fit strength"),
            "the dial must exist in the fold: {seen:?}"
        );
        // The saved reverse-fit dial is a preference, so it leaves the ● unlit.
        assert!(!app.ai_section_active(), "a fresh AI area has no state to flag");
        app.fit_strength = 0.85;
        assert!(!app.ai_section_active(), "a moved reverse-fit dial is still a preference");
        app.fit_strength = autoshade::recipe::GradeStrength::DEFAULT;
        assert!(!app.ai_section_active(), "resetting a preference also leaves the dot unlit");
        // Persisted on its own key; an older prefs file without it loads at the default.
        let prefs = Prefs { fit_strength: 0.85, ..Prefs::default() };
        let json = serde_json::to_string(&prefs).expect("prefs serialize");
        let decoded: Prefs = serde_json::from_str(&json).expect("prefs deserialize");
        assert_eq!(decoded.fit_strength, 0.85);
        assert_eq!(decoded.grade_strength, GRADE_STRENGTH_DEFAULT, "its own key, not a rename");
        let older = json.replace(r#""fit_strength":0.85,"#, "");
        assert!(!older.contains("fit_strength"), "the key really was removed: {older}");
        let decoded: Prefs = serde_json::from_str(&older).expect("an older prefs file loads");
        assert_eq!(decoded.fit_strength, autoshade::recipe::GradeStrength::DEFAULT);
        // …and the worker reads the fit's own dial, at both fit entry points.
        let worker = include_str!("../actions.rs");
        assert!(
            worker.contains("let fit_strength = self.fit_strength();"),
            "the reverse-fit block must read its own dial through fit_strength()"
        );
        assert_eq!(
            worker.matches("strength: fit_strength,").count(),
            2,
            "both fit entry points (zoned and global) must receive the fit's dial"
        );
        assert_eq!(
            worker.matches("GradeStrength::new(self.fit_strength)").count(),
            1,
            "exactly one reading of the fit's dial: fit_strength() itself"
        );
        assert_eq!(
            worker.matches("GradeStrength::new(self.grade_strength)").count(),
            1,
            "exactly one reading of the Analysis dial: analysis_strength() itself"
        );
        assert!(
            !worker.contains("fn panel_strength") && !worker.contains("self.panel_strength()"),
            "no shared reading is left (the name may survive in prose, never as code)"
        );
    }
