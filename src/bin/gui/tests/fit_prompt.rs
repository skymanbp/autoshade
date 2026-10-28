// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the question
// a whole-frame「AI generate」landing asks — reverse-fit the new ✨ card, and
// from which negative (v1.17.0).

    /// A photo with its ▣ card and a ◈ master, standing on the ◈ card — the
    /// strip where Automatic picks the master — with the negative's preview
    /// loaded, so a fit can start.
    fn app_before_a_generation(lang: Lang) -> AutoShadeApp {
        let card = |kind, id: &str, origin: Option<PathBuf>| Variant {
            id: id.into(),
            name: None,
            kind,
            recipe: EditRecipe::default(),
            base: None,
            origin,
            thumb: None,
        };
        let neg = Arc::new(image::DynamicImage::new_rgb8(4, 4));
        let mut app = AutoShadeApp {
            lang,
            src_path: Some(PathBuf::from("library/_fit_prompt_test.png")),
            variants: vec![
                card(VariantKind::Original, ORIGINAL_VARIANT_ID, None),
                card(VariantKind::Denoised, "dn", Some("out/_fit_prompt_test.denoise.png".into())),
            ],
            active: 1,
            source_preview: Some(neg.clone()),
            base_preview: Some(neg),
            ..Default::default()
        };
        app.reset_history();
        app
    }

    /// A finished worker landing through the one door every retouch takes.
    fn land(app: &mut AutoShadeApp, ctx: &egui::Context, note: RetouchNote, out: &str, kind: RetouchKind) {
        app.on_retouched(ctx, app.lang, app.gen_epoch, Ok((
            image::DynamicImage::new_rgb8(4, 4),
            note,
            PathBuf::from(out),
            kind,
        )));
    }

    fn reimagined(out: &str) -> RetouchNote {
        RetouchNote::Reimagined { out: PathBuf::from(out), divergence: 0.1, discarded: None }
    }

    /// One headless frame of the prompt alone, with `keys` pressed.
    fn prompt_frame(app: &mut AutoShadeApp, ctx: &egui::Context, keys: &[egui::Key]) {
        let input = egui::RawInput {
            events: keys
                .iter()
                .map(|&key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect(),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.fit_prompt_layer(ctx));
    }

    /// A reimagine landing asks, about the card it just made; a second one
    /// while the question is up re-points it at the newer card instead of
    /// stacking a second dialog. A fill layer, a ◈ denoise and a ✨ adjust
    /// land without asking — the question is about a generated look.
    ///
    /// MUTATION THIS CATCHES: the offer moved into the `NewGenerated` arm
    /// without the `Reimagined` match (an adjust asks), or the offer building
    /// a fresh prompt over an open one.
    #[test]
    fn a_generation_landing_asks_to_reverse_fit_the_new_card_and_nothing_else_does() {
        let ctx = egui::Context::default();
        let one = "out/_fit_prompt_test.reimagine.png";
        let two = "out/_fit_prompt_test.reimagine-2.png";
        let mut app = app_before_a_generation(Lang::En);
        land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
        assert_eq!(app.active_variant().map(|v| v.kind), Some(VariantKind::Generated), "premise: the strip switched");
        let first = app.active_variant().unwrap().id.clone();
        let p = app.fit_prompt.as_ref().expect("a reimagine landing asks");
        assert_eq!(p.target, first, "the question is about the card just made");
        app.fit_prompt.as_mut().unwrap().from = Some("dn".into());
        land(&mut app, &ctx, reimagined(two), two, RetouchKind::NewGenerated);
        let second = app.active_variant().unwrap().id.clone();
        assert_ne!(first, second);
        let p = app.fit_prompt.as_ref().expect("still one question");
        assert_eq!(p.target, second, "a newer generation takes the open question over");
        assert_eq!(p.from.as_deref(), Some("dn"), "and keeps the source the user picked in it");

        let quiet = |note: RetouchNote, kind: RetouchKind, why: &str| {
            let mut app = app_before_a_generation(Lang::En);
            land(&mut app, &ctx, note, "out/_fit_prompt_test.other.png", kind);
            assert!(app.fit_prompt.is_none(), "{why} must not ask");
        };
        quiet(RetouchNote::Filled(PathBuf::from("out/_fit_prompt_test.fill.png")), RetouchKind::Layer, "a fill layer");
        quiet(
            RetouchNote::Denoised { out: PathBuf::from("out/_fit_prompt_test.dn.png"), on_mosaic: true },
            RetouchKind::NewDenoised,
            "a ◈ denoise",
        );
        quiet(
            RetouchNote::Adjusted {
                out: PathBuf::from("out/_fit_prompt_test.adjust.png"),
                region: false,
                divergence: Some(0.1),
            },
            RetouchKind::NewGenerated,
            "a ✨ adjust",
        );

        // A workspace without the reverse-fit fold (Normal), or a running
        // job: no question. Portraits keeps the fold, so it asks.
        let mut app = app_before_a_generation(Lang::En);
        app.mode = Mode::Normal;
        land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
        assert!(app.fit_prompt.is_none(), "Normal has no reverse-fit to offer");
        let mut app = app_before_a_generation(Lang::En);
        app.mode = Mode::Portraits;
        land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
        assert!(app.fit_prompt.is_some(), "Portraits keeps the fold, so it asks");
        let mut app = app_before_a_generation(Lang::En);
        land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
        app.fit_prompt = None;
        app.busy = true;
        app.offer_fit_prompt(&ctx, 0.1);
        assert!(app.fit_prompt.is_none(), "never over a running job");
    }

    /// The prompt's「From:」is the panel's menu — one builder, both callers —
    /// and it starts on the ▣ original even where Automatic would solve from
    /// the ◈ master (the user's wording: 默认原图).
    ///
    /// MUTATION THIS CATCHES: the prompt defaulting to `None` (Automatic);
    /// either surface growing its own copy of the source list.
    #[test]
    fn the_prompt_offers_the_panels_sources_and_starts_on_the_original() {
        let one = "out/_fit_prompt_test.reimagine.png";
        for lang in [Lang::En, Lang::Zh] {
            let ctx = egui::Context::default();
            let mut app = app_before_a_generation(lang);
            land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
            assert_eq!(app.negative_index(), Some(1), "premise: Automatic would pick the ◈ master");
            let p = app.fit_prompt.as_ref().unwrap();
            assert_eq!(p.from.as_deref(), Some(ORIGINAL_VARIANT_ID), "{lang:?}: the default is the ▣ original");
            let (automatic, cards) = app.fit_source_menu();
            let ids: Vec<&str> = cards.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(ids, [ORIGINAL_VARIANT_ID, "dn"]);
            assert_eq!(cards[0].1, app.numbered_card_label(0));
            assert!(automatic.contains(&app.numbered_card_label(1)), "{automatic}");
        }
        let body = |src: &'static str, head: &str| -> &'static str {
            let from = src.find(head).unwrap_or_else(|| panic!("{head} moved"));
            let rest = &src[from..];
            &rest[..rest.find("\n    }").unwrap()]
        };
        let panel = body(include_str!("../panels/ai.rs"), "fn ai_reverse_fit(");
        assert!(panel.contains("self.fit_source_menu()"), "the panel builds its own source list again");
        let layer = body(include_str!("../actions.rs"), "pub(crate) fn fit_prompt_layer(");
        assert!(layer.contains("self.fit_source_menu()"), "the prompt builds its own source list again");
        assert!(layer.contains("self.start_fit()"), "the prompt runs a fit of its own");
    }

    /// Enter confirms: the source picked in the prompt becomes the fold's
    /// pick, the target is the generated card (a reference file chosen before
    /// is let go), and the fit starts with today's running line. Esc closes
    /// and touches nothing.
    #[test]
    fn confirming_the_prompt_fits_from_the_original_and_esc_changes_nothing() {
        let ctx = egui::Context::default();
        let one = "out/_fit_prompt_test.reimagine.png";
        let two = "out/_fit_prompt_test.reimagine-2.png";
        let mut app = app_before_a_generation(Lang::En);
        land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
        prompt_frame(&mut app, &ctx, &[egui::Key::Escape]);
        assert!(app.fit_prompt.is_none(), "Esc closes the question");
        assert_eq!(app.fit_from, None, "and leaves the fold's source alone");
        assert!(!app.busy, "and starts nothing");
        assert!(app.ask_fit_after_generate, "a plain 「Not now」 keeps asking");

        land(&mut app, &ctx, reimagined(two), two, RetouchKind::NewGenerated);
        app.fit_ref = Some(PathBuf::from("library/_fit_prompt_test.reference.jpg"));
        prompt_frame(&mut app, &ctx, &[egui::Key::Enter]);
        assert!(app.fit_prompt.is_none(), "the prompt closes before the fit runs");
        assert_eq!(app.fit_from.as_deref(), Some(ORIGINAL_VARIANT_ID), "the fit solves from the ▣ original");
        assert_eq!(app.fit_ref, None, "the fit targets the generated card, not an older reference");
        assert!(app.busy, "the fit started");
        assert_eq!(app.status, tr(Lang::En, "Reverse-fitting… (global + semantic/ranges + spatial tiles)"));
    }

    /// While the question is up it owns the keyboard (D13): a tool key does
    /// not reach the canvas behind it.
    #[test]
    fn the_open_prompt_swallows_canvas_shortcuts() {
        let ctx = egui::Context::default();
        let one = "out/_fit_prompt_test.reimagine.png";
        let mut app = app_before_a_generation(Lang::En);
        land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
        assert!(app.fit_prompt.is_some());
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::R,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        assert!(!app.crop_mode, "R is dead under the prompt");
    }

    /// 「Don't ask again」 turns the pref off when the prompt closes, the pref
    /// off means no question, and it round-trips through the prefs file — a
    /// file written before the key existed keeps asking.
    #[test]
    fn the_ask_pref_turns_the_prompt_off_and_round_trips() {
        assert!(AutoShadeApp::default().ask_fit_after_generate && Prefs::default().ask_fit_after_generate);
        let ctx = egui::Context::default();
        let one = "out/_fit_prompt_test.reimagine.png";
        let two = "out/_fit_prompt_test.reimagine-2.png";
        let mut app = app_before_a_generation(Lang::En);
        land(&mut app, &ctx, reimagined(one), one, RetouchKind::NewGenerated);
        app.fit_prompt.as_mut().unwrap().dont_ask = true;
        prompt_frame(&mut app, &ctx, &[egui::Key::Escape]);
        assert!(!app.ask_fit_after_generate, "the box is applied as the prompt closes");
        land(&mut app, &ctx, reimagined(two), two, RetouchKind::NewGenerated);
        assert!(app.fit_prompt.is_none(), "the pref off: no question");

        for ask in [false, true] {
            let mut storage = PrefsMemory::default();
            let mut app = AutoShadeApp { ask_fit_after_generate: ask, ..Default::default() };
            eframe::App::save(&mut app, &mut storage);
            let text = eframe::Storage::get_string(&storage, eframe::APP_KEY).unwrap();
            assert!(text.contains(&format!("ask_fit_after_generate:{ask}")), "{text}");
            let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
            let mut back = AutoShadeApp::default();
            back.restore_prefs(prefs);
            assert_eq!(back.ask_fit_after_generate, ask);
        }
        let mut storage = PrefsMemory::default();
        eframe::Storage::set_string(&mut storage, eframe::APP_KEY, "()".into());
        let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert!(prefs.ask_fit_after_generate, "an older prefs file keeps asking");
    }
