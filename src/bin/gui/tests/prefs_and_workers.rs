// One part of the GUI's tests (src/bin/gui/tests.rs includes it): preferences, zoom keys, toasts, the copy chord, fetch bookkeeping, workers and the panel layout.

    /// 阶段4: the ExportFormat prefs code round-trips, a pre-阶段4 prefs
    /// file (save_jpeg only) migrates to Jpeg, and ext/depth stay coherent
    /// with what render_to_file expects.
    #[test]
    fn export_format_prefs_migrate_and_roundtrip() {
        assert_eq!(
            ExportFormat::from_pref(0, true),
            ExportFormat::Jpeg,
            "legacy save_jpeg=true without a code migrates to JPEG"
        );
        assert_eq!(ExportFormat::from_pref(0, false), ExportFormat::Tiff16);
        assert_eq!(ExportFormat::from_pref(99, false), ExportFormat::Tiff16, "unknown code is safe");
        // The wire numbering is a PERSISTED contract (gui-prefs.ron) —
        // pinned literally, never derived from pref_code itself: a
        // coordinated swap of two variants' codes passed the derived
        // roundtrip while every stored preference changed meaning (L16-8).
        for (code, f) in [
            (0u8, ExportFormat::Tiff16),
            (1, ExportFormat::Tiff8),
            (2, ExportFormat::Png16),
            (3, ExportFormat::Png8),
            (4, ExportFormat::Jpeg),
        ] {
            assert_eq!(f.pref_code(), code, "persisted numbering is frozen: {f:?}");
            assert_eq!(ExportFormat::from_pref(code, false), f, "roundtrip {f:?}");
        }
        assert!(ExportFormat::Jpeg.eight_bit(), "JPEG is 8-bit by nature");
        assert!(ExportFormat::Tiff8.eight_bit() && ExportFormat::Png8.eight_bit());
        assert!(!ExportFormat::Tiff16.eight_bit() && !ExportFormat::Png16.eight_bit());
        assert_eq!(ExportFormat::Png8.ext(), "png");
        assert_eq!(ExportFormat::Tiff8.ext(), "tif");
        assert_eq!(ExportFormat::Jpeg.ext(), "jpg");
    }

    /// 阶段5: the installed theme must be the RENDERED theme, whatever the
    /// OS reports. egui 0.29 defaults to ThemePreference::System and
    /// `set_style` writes only the ACTIVE theme's style slot — before the
    /// fix, on a light-mode OS the startup install landed in the dark slot
    /// while the screen showed the STOCK light style (round-11's "亮色主题"
    /// screenshot was this bug in plain sight). The adversarial host here
    /// reports the OPPOSITE mode every frame; the app's choice must win,
    /// and the styled content (our selection stroke, not egui's default)
    /// must be what `ctx.style()` serves after a real frame.
    #[test]
    fn the_installed_theme_survives_an_opposite_mode_os() {
        for (pref, want_dark) in [(ThemePref::Dark, true), (ThemePref::Light, false)] {
            let ctx = egui::Context::default();
            let os_reports =
                if want_dark { egui::Theme::Light } else { egui::Theme::Dark };
            let input = egui::RawInput {
                system_theme: Some(os_reports),
                ..Default::default()
            };
            // Startup order: install BEFORE the first frame (system theme
            // unknown), exactly like main()'s creation closure.
            install_theme(&ctx, pref);
            // One real frame with the adversarial system theme — the moment
            // the old bug swapped the stock style in.
            let _ = ctx.run(input, |_| {});
            assert_eq!(
                ctx.style().visuals.dark_mode,
                want_dark,
                "{pref:?} must render as itself on an opposite-mode OS"
            );
            assert_eq!(
                ctx.style().visuals.selection.stroke.color,
                pref.colors().selection_stroke,
                "{pref:?}: the rendered slot must carry OUR palette, not stock"
            );
        }
    }

    /// 阶段5 手感: the zoom keys are real state transitions, not just
    /// cheat-sheet rows — `+` steps the TARGET ×1.25 (clamped ≤12, and it
    /// compounds so a key roll accumulates), `0` refits and recentres, `1`
    /// jumps to the canvas-computed 1:1 twin; the live zoom then GLIDES to
    /// the target (util::glide_step, exercised below). Driven through a
    /// headless frame so the whole tier-C gate chain (no transient, no
    /// focus) is exercised, not a hand-called helper.
    #[test]
    fn zoom_keys_step_fit_and_jump_to_one_to_one() {
        let key = |k: egui::Key| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut app = AutoShadeApp {
            src_path: Some(PathBuf::from("x.png")),
            zoom_one_to_one: 4.0, // what the canvas computed last frame
            zoom: 2.0,
            zoom_target: 2.0,
            pan: egui::vec2(0.7, 0.7),
            ..Default::default()
        };

        let ctx = egui::Context::default();
        let run_key = |app: &mut AutoShadeApp, k: egui::Key| {
            let mut input = egui::RawInput::default();
            input.events.push(key(k));
            let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        };
        run_key(&mut app, egui::Key::Plus);
        assert!((app.zoom_target - 2.5).abs() < 1e-4, "`+` retargets ×1.25, got {}", app.zoom_target);
        run_key(&mut app, egui::Key::Minus);
        assert!((app.zoom_target - 2.0).abs() < 1e-4, "`-` steps back, got {}", app.zoom_target);
        run_key(&mut app, egui::Key::Num1);
        assert_eq!(app.zoom_target, 4.0, "`1` targets the stored 1:1 zoom");
        run_key(&mut app, egui::Key::Num0);
        assert_eq!(app.zoom_target, 1.0, "`0` refits");
        assert_eq!(app.pan, egui::vec2(0.5, 0.5), "`0` recentres the pan (instant)");
        // Ceiling: from 11× one `+` press must stop at the 12× clamp.
        app.zoom_target = 11.0;
        run_key(&mut app, egui::Key::Plus);
        assert_eq!(app.zoom_target, 12.0, "zoom keys respect the 12x ceiling");
        // No photo → the keys are inert (same gate as the canvas buttons).
        app.src_path = None;
        app.zoom_target = 3.0;
        run_key(&mut app, egui::Key::Num0);
        assert_eq!(app.zoom_target, 3.0, "no photo: zoom keys must not act");
    }

    /// 阶段5 手感: the glide that carries `zoom` to `zoom_target` must move
    /// monotonically, never overshoot, terminate EXACTLY on the target
    /// (snap inside 1e-3 — a forever-almost-there zoom would repaint every
    /// frame for good), and settle in ~120 ms at 60 fps. Both directions.
    #[test]
    fn the_zoom_glide_converges_monotonically_and_terminates() {
        for (from, to) in [(1.0f32, 8.0f32), (8.0, 1.0)] {
            let mut z = from;
            let mut frames = 0;
            while z != to {
                let next = glide_step(z, to, 1.0 / 60.0);
                assert!(
                    (to - next).abs() <= (to - z).abs(),
                    "{from}->{to}: overshoot at frame {frames}: {z} -> {next}"
                );
                assert!(next != z, "{from}->{to}: stalled at {z} (frame {frames})");
                z = next;
                frames += 1;
                assert!(frames < 120, "{from}->{to}: no convergence in 2 s of frames");
            }
            assert!(frames <= 30, "{from}->{to}: settled in {frames} frames — the ~120 ms promise");
        }
        // A pathological dt (window drag hitch) must not overshoot either.
        assert_eq!(glide_step(2.0, 3.0, 10.0), glide_step(2.0, 3.0, 0.05), "dt is clamped");
    }

    /// 阶段5 手感: toast time scales with READING time — chars past the
    /// first 40 buy 35 ms each, capped per kind; short texts keep their
    /// historical 4 s / 8 s exactly, and a hanzi counts as one char, not
    /// three bytes.
    #[test]
    fn toast_ttl_scales_with_length_and_caps() {
        let mk = |text: &str, kind: ToastKind| Toast {
            text: text.into(),
            kind,
            born: Instant::now(),
        };
        assert_eq!(mk("Saved", ToastKind::Success).ttl(), Duration::from_millis(4_000));
        assert_eq!(mk("boom", ToastKind::Error).ttl(), Duration::from_millis(8_000));
        let fifty = "x".repeat(50);
        assert_eq!(
            mk(&fifty, ToastKind::Success).ttl(),
            Duration::from_millis(4_000 + 10 * 35),
            "10 chars past 40 buy 350 ms"
        );
        let hanzi = "字".repeat(50); // 150 BYTES — must still be 50 chars
        assert_eq!(
            mk(&hanzi, ToastKind::Success).ttl(),
            Duration::from_millis(4_000 + 10 * 35),
            "chars, not bytes"
        );
        let epic = "y".repeat(1000);
        assert_eq!(mk(&epic, ToastKind::Success).ttl(), Duration::from_millis(10_000), "success cap");
        assert_eq!(mk(&epic, ToastKind::Error).ttl(), Duration::from_millis(14_000), "error cap");
    }

    /// Codex 阶段5 F1 closure: egui-winit swallows every Ctrl(+Shift)+C into
    /// Event::Copy and emits NO Key event (is_copy_command returns early),
    /// so a consume_key(COMMAND, C) binding can never fire on real input —
    /// the first headless attempt pushed raw Key events and proved nothing.
    /// This test feeds exactly what winit sends: the Copy EVENT plus the
    /// modifier state. Shift is the discriminator — the bare event must
    /// pass through untouched for egui's own selected-label copy.
    #[test]
    fn recipe_copy_rides_the_shift_chord_not_the_bare_copy_event() {
        let mut app = AutoShadeApp {
            src_path: Some(PathBuf::from("x.png")),
            ..Default::default()
        };
        app.recipe.exposure_ev = 1.5;
        let ctx = egui::Context::default();
        let run_copy = |app: &mut AutoShadeApp, shift: bool| {
            let input = egui::RawInput {
                modifiers: egui::Modifiers {
                    command: true,
                    ctrl: true,
                    shift,
                    ..Default::default()
                },
                events: vec![egui::Event::Copy],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        };
        run_copy(&mut app, false);
        assert!(app.copied.is_none(), "bare Ctrl+C belongs to egui's text copy");
        run_copy(&mut app, true);
        let copied = app.copied.as_ref().expect("Ctrl+Shift+C copies the recipe");
        assert_eq!(copied.exposure_ev, 1.5, "the CURRENT recipe is what's copied");
        assert_eq!(
            app.copied_from.as_deref(),
            Some(std::path::Path::new("x.png")),
            "provenance rides along (the paste-guard identity)"
        );
    }

    /// GUI review 2026-08-12 F3: a catalogue knew WHERE its ids came from
    /// (`from_base`) but not WHEN — `Msg::Models` carried only the role, so a
    /// completion launched under key K1 landed unconditionally after the user
    /// had already replaced the key with K2, and the URL check happily kept
    /// K1's ids under K2's name. Every `clear` now bumps a generation and a
    /// completion must still match it to install.
    #[test]
    fn a_completion_from_a_superseded_fetch_is_discarded_not_installed() {
        let mut app = AutoShadeApp::default();
        // A typed loopback base + typed token keep the real worker's probe
        // off the network entirely (connection refused before any header).
        app.settings.image_base_url = "http://127.0.0.1:9/v1".into();
        app.settings.image_api_key = "typed-test-token".into();
        app.fetch_models(ModelRole::Image);
        let stale_gen = app.settings.image_models.generation;
        assert!(app.settings.image_models.fetching, "the flight is up");
        // The user replaces the key mid-flight — the key field's handler
        // clears the catalogue, and clearing bumps the generation:
        app.settings.image_models.clear();
        // The old flight lands, delivered exactly as the pump would:
        app.on_models(Lang::En, ModelRole::Image, stale_gen, Ok(vec!["stale-model".into()]));
        assert!(
            app.settings.image_models.chat.is_empty(),
            "ids fetched with the OLD credential must not be offered under the new one"
        );
        assert!(!app.settings.image_models.fetching, "stale or not, THE flight is over");
        // A fresh fetch's completion still installs normally:
        app.fetch_models(ModelRole::Image);
        let live_gen = app.settings.image_models.generation;
        app.on_models(Lang::En, ModelRole::Image, live_gen, Ok(vec!["fresh-model".into()]));
        assert_eq!(app.settings.image_models.chat, vec!["fresh-model".to_string()]);
        assert!(!app.settings.image_models.fetching);
    }

    /// Part D: what the listing SAYS about a model reaches its pickers — the
    /// effort picker offers the model's own tiers (none, for a model that
    /// takes none), the vision picker learns which model takes no image —
    /// and only for the ids the listing spoke about; the rest keep the fixed
    /// suggestions. A `clear` forgets it with the ids.
    #[test]
    fn the_listing_capabilities_reach_the_pickers_and_clear_with_the_ids() {
        use autoshade::openai_models::ModelInfo;
        let mut app = AutoShadeApp::default();
        app.settings.image_base_url = "http://127.0.0.1:9/v1".into();
        app.settings.image_api_key = "typed-test-token".into();
        app.fetch_models(ModelRole::Image);
        let live_gen = app.settings.image_models.generation;
        app.on_models(
            Lang::En,
            ModelRole::Image,
            live_gen,
            Ok(vec![
                ModelInfo {
                    id: "reasoner".into(),
                    effort_options: Some(vec!["minimal".into(), "max".into()]),
                    vision: Some(true),
                },
                ModelInfo { id: "plain".into(), effort_options: Some(vec![]), vision: Some(false) },
                ModelInfo::from("unknown"),
            ]),
        );
        let cat = &app.settings.image_models;
        assert_eq!(cat.tiers_for("reasoner", &EFFORT_TIERS_API), vec!["minimal", "max"]);
        assert_eq!(cat.tiers_for("plain", &EFFORT_TIERS_API), Vec::<&str>::new());
        assert_eq!(cat.tiers_for("unknown", &EFFORT_TIERS_API), EFFORT_TIERS_API.to_vec());
        assert_eq!(cat.tiers_for(" reasoner ", &EFFORT_TIERS_API), vec!["minimal", "max"], "trimmed like the field");
        assert_eq!(cat.vision_of("plain"), Some(false));
        assert_eq!(cat.vision_of("reasoner"), Some(true));
        assert_eq!(cat.vision_of("unknown"), None);
        app.settings.image_models.clear();
        let cat = &app.settings.image_models;
        assert!(cat.efforts.is_empty() && cat.vision.is_empty(), "forgotten with the ids");
        assert_eq!(cat.tiers_for("reasoner", &EFFORT_TIERS_API), EFFORT_TIERS_API.to_vec());
    }

    /// GUI review 2026-08-12 F6: one global "auto-fetched" boolean was
    /// consumed by the first Settings open even when NO role had a key, so a
    /// key saved five minutes later never got its convenience probe on any
    /// later open. The guard is per role and consumed at DISPATCH.
    #[test]
    fn the_autofetch_opportunity_survives_until_a_role_is_actually_eligible() {
        let mut app = AutoShadeApp::default();
        // Any dispatch stays strictly on loopback: a typed form base wins
        // over the machine's saved config, and the saved-key rule withholds
        // any real credential from an endpoint it was not saved for.
        app.settings.image_base_url = "http://127.0.0.1:9/v1".into();
        let mut cfg = autoshade::config::Config {
            openai_api_key: None,
            openai_model: "test-chat".into(),
            openai_base_url: "http://127.0.0.1:9/v1".into(),
            openai_image_model: "test-image".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 4_000_000,
            image_provider: "api".into(),
            image_effort: None,
            image_api_family: "auto".into(),
            image_gen_endpoint: None,
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:9/v1".into(),
            python_bin: "python".into(),
            denoise_model: "scunet_color_real_psnr".into(),
            denoise_script: String::new(),
            denoise_raw_script: String::new(),
            weights_dir: String::new(),
            segment_script: String::new(),
            depth_script: String::new(),
            embed_script: String::new(),
            correspond_script: String::new(),
            describe_script: String::new(),
            style_strength: 0.5,
            send_reference_image: false,
        };
        // First open: no credential — nothing to probe, and the VISIT itself
        // must not spend the session's opportunity.
        app.autofetch_models_once(&cfg);
        assert!(
            !app.settings.image_models.autofetched,
            "an ineligible open must not consume the role's probe"
        );
        assert!(!app.settings.image_models.fetching);
        // The key arrives (saved on another surface); the NEXT open probes.
        cfg.openai_api_key = Some("typed-test-token".into());
        app.autofetch_models_once(&cfg);
        assert!(app.settings.image_models.autofetched, "consumed at dispatch");
        assert!(app.settings.image_models.fetching, "the probe launched");
        // …and only once per session: a reopen never re-probes a metered
        // endpoint (the review verified this half held; it must keep holding).
        app.settings.image_models.fetching = false;
        app.autofetch_models_once(&cfg);
        assert!(!app.settings.image_models.fetching, "the one probe is spent");
    }

    /// GUI review 2026-08-12 F1 (model face): the CLI accepts full ids as
    /// well as aliases, so a provider flip may only rewrite what PROVABLY
    /// belongs to the other provider's vocabulary — a valid `claude-opus-4-6`
    /// configured against an API bridge used to be silently replaced with
    /// `opus` on every flip to OAuth.
    #[test]
    fn a_provider_flip_rewrites_only_the_other_providers_vocabulary() {
        // OAuth-ward (to_api = false):
        assert_eq!(analysis_model_on_flip("gpt-5.5", false), Some("opus"), "an OpenAI id yields");
        assert_eq!(analysis_model_on_flip("claude-opus-4-6", false), None, "a full Claude id is CLI-valid");
        assert_eq!(analysis_model_on_flip("Claude-Opus-4-6", false), None, "…case-folded, like every id test");
        assert_eq!(analysis_model_on_flip("fable", false), None, "every documented alias survives");
        // API-ward (to_api = true):
        assert_eq!(analysis_model_on_flip("opus", true), Some("gpt-5.5"), "an alias is CLI-only vocabulary");
        assert_eq!(analysis_model_on_flip("claude-opus-4-6", true), None, "a bridge legitimately serves claude-*");
        assert_eq!(analysis_model_on_flip("gpt-5.5", true), None, "already at home");
    }

    /// GUI review 2026-08-12 F4: worker completion arrives on a plain mpsc
    /// channel, which does not wake egui — and the 100 ms pump's gate
    /// (`poll_workers`) runs before any panel can start a fetch in the same
    /// frame, so with the pointer held still a click on "Fetch models" showed
    /// nothing (not even "fetching…") until the next input event. Both edges
    /// now request a repaint from inside `spawn_worker`.
    #[test]
    fn worker_spawn_and_completion_both_wake_the_event_loop() {
        let app = AutoShadeApp::default();
        // Drain the fresh context's startup frames (egui schedules a couple
        // on its own) until it reports quiet.
        for _ in 0..8 {
            let _ = app.egui_ctx.run(Default::default(), |_| {});
        }
        assert!(!app.egui_ctx.has_requested_repaint(), "sanity: context drained");
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        app.spawn_worker(
            move || {
                let _ = gate_rx.recv();
                Msg::Models(ModelRole::Image, 0, Ok(Vec::new()))
            },
            |e| Msg::Models(ModelRole::Image, 0, Err(e)),
        );
        // Edge 1: STARTING a worker schedules the next frame (the pump's
        // gate has already run this frame by the time a panel can spawn).
        assert!(app.egui_ctx.has_requested_repaint(), "spawn must wake the loop");
        // Consume that request the way real frames would; the worker is
        // still gated, so quiet must return…
        for _ in 0..8 {
            let _ = app.egui_ctx.run(Default::default(), |_| {});
        }
        assert!(!app.egui_ctx.has_requested_repaint(), "sanity: drained again");
        // …until the worker completes: edge 2, the completion itself must
        // wake the loop — the mpsc send alone never does.
        gate_tx.send(()).expect("the worker is waiting on the gate");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.egui_ctx.has_requested_repaint() {
            assert!(
                std::time::Instant::now() < deadline,
                "a completed worker never asked for a repaint — its result would sit unread until the next input event"
            );
            std::thread::yield_now();
        }
    }

    /// R14 (user report 2026-08-12): after the centering rework the variant
    /// card sat visibly LOW-or-HIGH in the strip — the air above the
    /// thumbnail must equal the air below the card column ("变成和下边一样
    /// 距离"). Headless frame over the REAL variant_strip + REAL theme
    /// style + the REAL panel height constant; the three test seams record
    /// what actually laid out.
    /// R19: the ✨ Generate button renders ONE line tall in both languages,
    /// and the row's width arithmetic is exact — two distinct failure modes,
    /// both real: the old fixed 130 px reserve was 1 px short of the
    /// English label, wrapping the button's text into a two-line button
    /// (the user report); and the first fix omitted the TextEdit's own
    /// 8 px frame margin, which — with the button in Extend mode — stopped
    /// being absorbed by wrapping and instead widened the auto-fitting
    /// side panel by 8 px EVERY frame (probed during review). Three frames
    /// pin both: a stable panel width and a one-line button.
    ///
    /// R22-6: that row MOVED into the AI panel (`ai_panel`, #4), so this test
    /// follows its host — driving `retouch_panel` alone would leave
    /// `reimagine_btn_rect` unset and the assertions vacuous (the seam's
    /// `expect` is what turns that into a red instead of a silent pass). Both
    /// panels are driven so the width witness still covers everything the side
    /// panel stacks below the AI area.
    #[test]
    fn the_generate_button_stays_one_line_and_the_panel_stays_put() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            };
            let mut widths = Vec::new();
            for _ in 0..3 {
                let _ = ctx.run(input(), |ctx| {
                    let r = egui::SidePanel::left("controls")
                        .default_width(320.0)
                        .show(ctx, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                app.ai_panel(ui);
                                app.retouch_panel(ui);
                            });
                        });
                    // the panel's rendered width, the runaway's witness
                    widths.push(r.response.rect.width());
                });
            }
            assert!(
                (widths[0] - widths[2]).abs() < 0.5,
                "{lang:?}: the side panel must not grow across frames: {widths:?}"
            );
            let btn = app.reimagine_btn_rect.expect("the button records its rect (test seam)");
            let one_line = ctx.style().spacing.interact_size.y;
            assert!(
                btn.height() <= one_line + 1.0,
                "{lang:?}: the Generate button must be one line tall ({} vs {one_line})",
                btn.height()
            );
        }
    }

    /// R22-6 (#4): the AI area's mid-open EDITABLE GATE, in its new host.
    ///
    /// L15-2: while a decode is in flight (`open_in_flight`) the panel's
    /// controls address the STASHED photo A, and B is about to land and replace
    /// the whole recipe — typing a Direction or firing Analyze there is silent
    /// input loss. The analysis block used to sit INSIDE `develop_panel`'s
    /// `add_enabled_ui(editable, …)` closure and inherited that gate for free;
    /// moving it into `ai_panel` (#4) meant re-establishing it by hand, which
    /// is exactly the kind of migration that loses a guard quietly. `busy`
    /// alone must NOT gate (a 600 s analyze keeps the panel live), so both
    /// halves are pinned. Deleting the `add_enabled_ui` wrapper in ai.rs fails
    /// phase ②.
    #[test]
    fn the_ai_panel_freezes_only_while_a_photo_is_opening() {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let frame = |app: &mut AutoShadeApp| -> Option<bool> {
            app.ai_gate_enabled = None; // this frame's evidence only
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                        app.ai_panel(ui);
                    });
                },
            );
            app.ai_gate_enabled
        };
        // ① settled: live.
        let mut app = AutoShadeApp::default();
        assert_eq!(frame(&mut app), Some(true), "a settled panel must be editable");
        // ② mid-open: frozen.
        app.open_in_flight = true;
        assert_eq!(
            frame(&mut app),
            Some(false),
            "mid-open the AI controls address the STASHED photo — they must be inert (L15-2)"
        );
        // ③ busy but NOT opening (a 600 s analyze): still live, or cancelling
        // and re-typing a direction would be impossible for ten minutes.
        app.open_in_flight = false;
        app.busy = true;
        assert_eq!(frame(&mut app), Some(true), "a long AI call must not freeze the panel");
    }

    /// R22-6 (#14a): a prompt field has a READABLE ceiling. Every one of them
    /// used to take `available_width()` — fine at the 320 px default, an
    /// 800 px single-line ribbon once the side panel is dragged wide (the panel
    /// deliberately keeps NO max width: the curve and HSL editors are better
    /// wide, so the cap belongs on the FIELDS). Laid out at 800 px, which is
    /// exactly where the old code fails: every field then measures its full
    /// available width and blows the `FIELD_W_MAX` assertion.
    ///
    /// All four prompts are pinned by ONE rule over the `prompt_rects` seam:
    /// Direction, Adjust and Generative Fill go through `util::prompt_field`, while the
    /// Reimagine row keeps its own R19 galley arithmetic and only `.min()`s the
    /// result — four sites, one ceiling, and a further field added later is
    /// covered without editing this test.
    #[test]
    fn a_prompt_field_never_grows_past_its_readable_width() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            // Text long enough that a field free to grow would want to: the
            // singleline clip keeps this from mattering, the cap is about the
            // BOX, and a filled buffer also exercises the hover-tooltip arm.
            app.guidance = "warmer and moodier, lift the shadows a lot, and keep the sky honest".into();
            app.reimagine_prompt = app.guidance.clone();
            app.adjust_prompt = app.guidance.clone();
            app.fill_prompt = app.guidance.clone();
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1200.0),
                )),
                ..Default::default()
            };
            // Three frames, because a WIDTH rule and an auto-fitting side panel
            // is exactly the pair that produced R19's +8 px/frame runaway: the
            // cap only ever asks for LESS than the row has (which cannot grow
            // the panel), and this is what says so.
            let mut widths = Vec::new();
            for _ in 0..3 {
                app.prompt_rects.clear(); // the LAST frame's evidence only
                let _ = ctx.run(input(), |ctx| {
                    // Both prompt-bearing folds are collapsed by default; egui's
                    // own test hook opens every collapsible (the same one the
                    // mask-brush width test uses), so an unopened fold cannot
                    // make this vacuous.
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    let r =
                        egui::SidePanel::left("controls").default_width(800.0).show(ctx, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                app.ai_panel(ui);
                                app.retouch_panel(ui);
                            });
                        });
                    widths.push(r.response.rect.width());
                });
            }
            assert!(
                (widths[0] - widths[2]).abs() < 0.5,
                "{lang:?}: the wide panel must not grow across frames: {widths:?}"
            );
            assert_eq!(
                app.prompt_rects.len(),
                4,
                "{lang:?}: expected the Direction / Reimagine / Adjust / Fill prompts to lay out, got {:?}",
                app.prompt_rects
            );
            for (i, r) in app.prompt_rects.iter().enumerate() {
                assert!(
                    r.width() <= crate::theme::FIELD_W_MAX + 0.5,
                    "{lang:?}: prompt field {i} is {:.1} px wide on an 800 px panel — past the \
                     {:.0} px readable ceiling (a dropped `.min(FIELD_W_MAX)`)",
                    r.width(),
                    crate::theme::FIELD_W_MAX
                );
                assert!(
                    r.width() >= crate::theme::FIELD_W_MIN,
                    "{lang:?}: prompt field {i} collapsed to {:.1} px",
                    r.width()
                );
            }
        }
    }

    #[test]
    fn the_variant_card_gets_equal_air_above_and_below() {
        let mk = |kind: crate::model::VariantKind| crate::model::Variant {
            id: String::new(),
            name: None,
            kind,
            recipe: Default::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        // Two states of the measured (first) card: the lone-Original
        // placeholder, and a non-Original card in a two-variant strip —
        // whose label row carries the ✕ delete button (review 2026-08-12
        // finding 1: the ✕ row was never laid out by the single case).
        let scenarios: [(&str, Vec<crate::model::Variant>); 2] = [
            ("lone original", vec![mk(crate::model::VariantKind::Original)]),
            (
                "deletable card with ✕",
                vec![
                    mk(crate::model::VariantKind::Generated),
                    mk(crate::model::VariantKind::Original),
                ],
            ),
        ];
        for (label, variants) in scenarios {
            let mut app = AutoShadeApp { variants, ..Default::default() };
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::TopBottomPanel::bottom("variants")
                    .exact_height(AutoShadeApp::VARIANT_STRIP_H)
                    .show(ctx, |ui| app.variant_strip(ui));
            });
            let row = app.strip_row_rect.expect("the strip records its row (test seam)");
            let card = app.strip_card_rect.expect("the strip records its card (test seam)");
            // Gaps bracket the whole CARD COLUMN (review finding 3: measuring
            // the top from the thumb keeps passing if something ever lands
            // above it), and a floor keeps "0 air on both sides" from
            // counting as centered (review finding 8).
            let top = card.top() - row.top();
            let bottom = row.bottom() - card.bottom();
            eprintln!("strip geometry [{label}]: row={row:?} card={card:?} \
                 top_gap={top:.1} bottom_gap={bottom:.1}");
            assert!(
                (top - bottom).abs() <= 1.0,
                "[{label}] unequal air around the variant card: \
                 {top:.1} above vs {bottom:.1} below"
            );
            assert!(
                top >= 4.0,
                "[{label}] the card column has no breathing room ({top:.1} px) — \
                 it outgrew VARIANT_STRIP_H"
            );
            // R16: the PAINTED title centers on the measured card column —
            // the title rect comes from variant_strip's own cursor math, the
            // card rect from egui's real layout; agreeing centers proves the
            // two geometries cohere (they diverged by ~29 px when the title
            // was a row child centered on egui's 26 px seed).
            let title =
                app.strip_title_rect.expect("the strip records its painted title (test seam)");
            let dy = title.center().y - card.center().y;
            assert!(
                dy.abs() <= 1.0,
                "[{label}] the Variants title sits {dy:.1} px off the card column's center"
            );
        }
    }

    /// #17 (user report after the v0.27 trial): a PORTRAIT photo's gallery
    /// thumbnail was squashed into the same 1.4:1 landscape box as everything
    /// else. The orientation chain (decode's EXIF transpose, render's
    /// rotation) was never at fault — the panel handed egui a
    /// `SizedTexture::new(id, (THUMB_W, THUMB_H))`, i.e. a LIE about the
    /// texture's own size, and a Texture image source is drawn at
    /// `ImageFit::Exact`. The fix keeps the SLOT constant (that is what keeps
    /// the filename column aligned) and insets the image at its true aspect.
    /// Restoring the lie (`let draw = egui::vec2(THUMB_W, THUMB_H)`) fails the
    /// aspect assertion for the portrait case.
    #[test]
    fn a_gallery_thumb_keeps_its_aspect_inside_a_constant_slot() {
        for (tw, th, tag) in [(40u32, 60u32, "portrait 2:3"), (60, 40, "landscape 3:2")] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoShadeApp::default();
            let tex = ctx.load_texture(
                "r22_thumb",
                egui::ColorImage::new([tw as usize, th as usize], egui::Color32::GRAY),
                egui::TextureOptions::LINEAR,
            );
            // One row, its texture already resident: the loaded branch.
            app.gallery = vec![PathBuf::from("r22-thumb-geometry.arw")];
            app.gallery_dir = Some(PathBuf::from("."));
            app.thumbs.insert(0, tex);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::SidePanel::left("library")
                    .default_width(260.0)
                    .show(ctx, |ui| app.gallery_panel(ui));
            });
            let slot = app.gallery_slot_rect.expect("the gallery records its thumb slot (test seam)");
            let draw = app.gallery_thumb_rect.expect("the gallery records the drawn image (test seam)");
            eprintln!("gallery thumb [{tag}]: slot={slot:?} draw={draw:?}");
            // ① the image is drawn at the TEXTURE's aspect ratio
            let want = tw as f32 / th as f32;
            let got = draw.width() / draw.height();
            assert!(
                (got - want).abs() <= 0.02,
                "[{tag}] the thumbnail is drawn at {got:.3}:1, not the texture's {want:.3}:1 \
                 — a portrait squashed into the landscape slot again"
            );
            // ② the SLOT is the constant one the placeholder branch allocates
            assert!(
                (slot.width() - THUMB_W).abs() < 0.01 && (slot.height() - THUMB_H).abs() < 0.01,
                "[{tag}] the row slot must stay a constant {THUMB_W}×{THUMB_H} (the filename \
                 column's alignment depends on it): {slot:?}"
            );
            // ③ letterboxed INSIDE that slot, centred in it
            assert!(
                slot.contains_rect(draw),
                "[{tag}] the drawn image {draw:?} escapes its slot {slot:?}"
            );
            let d = draw.center() - slot.center();
            assert!(
                d.x.abs() <= 0.5 && d.y.abs() <= 0.5,
                "[{tag}] the inset is off-centre by ({:.2}, {:.2}) px",
                d.x,
                d.y
            );
        }
    }

    /// #9 (user report after the v0.27 trial): "画笔大小的滑杆不见了". It was
    /// never deleted — the app's ONE brush radius (`self.brush`) had its only
    /// slider in the Retouch panel, next to Fill / Heal / Stamp, while the
    /// mask brush is armed from Develop → Local Masks and paints in a session
    /// whose row carried ⌫ / ✓ / ✕ and nothing else. So the control existed in
    /// a panel the user was not in. This renders the REAL develop_panel with a
    /// live session (the previous width test only ever drove retouch_panel —
    /// develop_panel had zero frame coverage) and pins two things:
    ///   ① the Brush size slider is present in that frame — deleting the
    ///      `self.brush_size_slider(ui)` call in the session block fails here;
    ///   ② the side panel's width does not grow across frames — the +8 px per
    ///      frame runaway of v0.26.1 happened in this very side panel, and this
    ///      change adds a widget to it.
    #[test]
    fn the_mask_brush_session_carries_the_brush_size_slider() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoShadeApp { lang, ..Default::default() };
            // A plate is what start_mask_brush sizes its buffers against.
            app.base_preview = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(64, 96)));
            app.start_mask_brush(None);
            assert!(app.mask_brush.is_some(), "{lang:?}: the brush session armed");
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            };
            let mut widths = Vec::new();
            for frame in 0..3 {
                app.brush_slider_rect = None; // this frame's evidence only
                let _ = ctx.run(input(), |ctx| {
                    // Local Masks is a collapsed section by default; opening
                    // every collapsible is egui's own test hook for exactly
                    // this (CollapsingState::openness).
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    let r = egui::SidePanel::left("controls")
                        .default_width(320.0)
                        .show(ctx, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                        });
                    widths.push(r.response.rect.width());
                });
                let s = app.brush_slider_rect.unwrap_or_else(|| {
                    panic!(
                        "{lang:?} frame {frame}: no Brush size slider in the mask-brush \
                         session — the radius control is back in another panel"
                    )
                });
                assert!(
                    s.height() >= ctx.style().spacing.interact_size.y - 1.0
                        && s.width().is_finite()
                        && s.width() > 0.0,
                    "{lang:?} frame {frame}: the slider occupied {s:?} — it did not lay out"
                );
            }
            assert!(
                (widths[0] - widths[2]).abs() < 0.5,
                "{lang:?}: the controls panel must not grow across frames: {widths:?}"
            );
        }
    }

    /// R22-3, same段 as #9, on the per-fold brush (2026-09-27): a live
    /// MASK-brush session paints through the same `paint_mode` flag a tool's
    /// brush uses. Putting the brush away while a session is live used to
    /// leave an ORPHAN session — brush inert, but the ⌫ / ✓ Apply / ✕ Cancel
    /// row still on screen with a buffer 「Apply」 would happily bake. It must
    /// take the session's own teardown. (Deleting the `end_mask_brush` call
    /// in `disarm_brush` fails phase 2.)
    #[test]
    fn arming_a_brush_sweeps_the_other_tools_and_putting_it_away_ends_a_mask_session() {
        // Phase 1: arming sweeps the other canvas tools and stays armed; the
        // Stamp arms through clone_mode and takes the other brush down.
        let mut app = AutoShadeApp {
            base_preview: Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(32, 48))),
            crop_mode: true,
            clone_mode: true,
            ..Default::default()
        };
        app.arm_brush(BrushOwner::Fill);
        assert!(app.paint_mode && app.brush_armed(BrushOwner::Fill), "arming must survive the mutual-exclusion sweep");
        assert!(!app.crop_mode && !app.clone_mode, "the other tools are disarmed");
        app.arm_brush(BrushOwner::Stamp);
        assert!(app.clone_mode && !app.paint_mode, "the Stamp paints through clone_mode");
        assert!(app.brush_armed(BrushOwner::Stamp) && !app.brush_armed(BrushOwner::Fill));
        // Phase 2: a live session + the brush put away = a cancel, not an orphan.
        app.start_mask_brush(None);
        assert!(
            app.mask_brush.is_some() && app.mask_brush_gray.is_some() && app.paint_mode,
            "sanity: the session is live and painting through paint_mode"
        );
        app.disarm_brush();
        assert!(
            app.mask_brush.is_none(),
            "the session outlived its own paint flag — its Apply row would bake stale weights"
        );
        assert!(app.mask_brush_gray.is_none(), "the weight buffer goes with it");
        assert!(!app.paint_mode && !app.clone_mode, "and the flags stay off");
    }
